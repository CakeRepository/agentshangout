//! Tests for bots' projects: starting one, bringing other bots in, and its studio on The Commons, and
//! for what a bot sees of The Commons. They drive a real server over HTTP and are written from L11 to
//! L15 in docs/routines.md and W88 and W89 in docs/public-city.md. A comment such as `// L12` above a
//! test names the rule it covers.

use agenthangout::{
    app,
    db::Db,
    now,
    world::{self, Kind},
    worlds::{self, Move},
    AppState, Config,
};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};

const COMMONS: &str = "commons";
const CODE: &str = "letmein";
const DAY: i64 = 86_400;

struct Server {
    base: String,
    client: Client,
    state: AppState,
}

struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    #[track_caller]
    fn ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{} {}", self.status, self.body);
        self.body
    }
}

struct Who {
    key: String,
    id: String,
}

/// Ann's bot Fern and Bo's bot Rover, and Cy's public cafe floor where bots can meet.
struct Town {
    ann: Who,
    bo: Who,
    cafe: String,
    fern: String,
    rover: String,
    rover_id: String,
}

/// Dee's city, Elm, with her house in it and her bot living there.
struct Elm {
    dee: Who,
    house: Value,
    pip: String,
    pip_id: String,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
}

impl Server {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let state = AppState::new(
            Db::open(":memory:").unwrap(),
            Config {
                public_url: base.clone(),
                create_code: Some(CODE.to_string()),
                allow_private_webhooks: true,
                keeper_id: None,
                god_ids: Vec::new(),
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        worlds::ensure_commons(&state).unwrap();
        Self { base, client: Client::new(), state }
    }

    async fn send(&self, token: &str, method: Method, path: &str, body: Option<Value>) -> Reply {
        let mut request = self.client.request(method, format!("{}{path}", self.base));
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        Reply { status, body: serde_json::from_str(&text).unwrap_or(Value::String(text)) }
    }

    async fn get(&self, token: &str, path: &str) -> Reply {
        self.send(token, Method::GET, path, None).await
    }

    async fn post(&self, token: &str, path: &str, body: Value) -> Reply {
        self.send(token, Method::POST, path, Some(body)).await
    }

    async fn put(&self, token: &str, path: &str, body: Value) -> Reply {
        self.send(token, Method::PUT, path, Some(body)).await
    }

    async fn delete(&self, token: &str, path: &str) -> Reply {
        self.send(token, Method::DELETE, path, None).await
    }

    async fn someone(&self, name: &str) -> (String, Who) {
        let made = self
            .post(
                "",
                "/api/hangouts",
                json!({ "name": format!("{name}'s room"), "your_name": name, "create_code": CODE }),
            )
            .await
            .ok();
        (
            text(&made["hangout"]["id"]),
            Who { key: text(&made["owner_key"]), id: text(&made["person"]["id"]) },
        )
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> (String, String) {
        let made = self
            .post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await
            .ok();
        (text(&made["agent"]["id"]), text(&made["token"]))
    }

    async fn town(&self) -> Town {
        let (ann_room, ann) = self.someone("Ann").await;
        let (_, fern) = self.bot(&ann, &ann_room, "Fern").await;
        let (bo_room, bo) = self.someone("Bo").await;
        let (rover_id, rover) = self.bot(&bo, &bo_room, "Rover").await;
        let (cafe, cy) = self.someone("Cy").await;
        self.put(
            &cy.key,
            &format!("/api/floors/{cafe}"),
            json!({ "access": "public", "create_code": CODE }),
        )
        .await
        .ok();
        Town { ann, bo, cafe, fern, rover, rover_id }
    }

    /// Both bots' owners send them onto Cy's public floor, where they meet.
    async fn meet(&self, town: &Town) {
        let fern_id = text(&self.get(&town.fern, "/api/me").await.ok()["agent_id"]);
        self.post(
            &town.ann.key,
            &format!("/api/floors/{}/walk-in", town.cafe),
            json!({ "agent_id": fern_id }),
        )
        .await
        .ok();
        self.post(
            &town.bo.key,
            &format!("/api/floors/{}/walk-in", town.cafe),
            json!({ "agent_id": town.rover_id }),
        )
        .await
        .ok();
    }

    async fn call(&self, token: &str, name: &str, arguments: Value) -> Value {
        let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        self.post(token, "/mcp", call).await.ok()["result"].clone()
    }

    async fn tool(&self, token: &str, name: &str, arguments: Value) -> Value {
        let result = self.call(token, name, arguments).await;
        assert!(result["isError"].is_null(), "{name} failed: {result}");
        result["structuredContent"].clone()
    }

    async fn refused(&self, token: &str, name: &str, arguments: Value) -> String {
        let result = self.call(token, name, arguments).await;
        assert_eq!(result["isError"], true, "{name} should have been refused: {result}");
        text(&result["content"][0]["text"])
    }

    /// Tiles a place can really go on: open land beside a road, not kept for a side street.
    fn free_spots(&self) -> Vec<(i64, i64)> {
        let tiles = self.state.db.world_tiles(COMMONS).unwrap();
        let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
        let mut spots = Vec::new();
        for (&(x, y), tile) in &tiles {
            if tile.kind == Kind::Road {
                let (x, y) = (i64::from(x), i64::from(y));
                spots.extend([(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]);
            }
        }
        spots
            .into_iter()
            .filter(|&(x, y)| {
                let pos = (x as u16, y as u16);
                !tiles.contains_key(&pos)
                    && !world::is_reserved(&tiles, pos)
                    && land.at(pos).is_some_and(|ground| ground.is_open())
            })
            .collect()
    }

    /// A person's house, put up two days ago so a studio has come (W61).
    fn house_two_days_ago(&self, who: &Who) {
        let at = now() - 2 * DAY;
        let player = self.state.db.own_player(&who.id, at).unwrap();
        let (x, y) = self.free_spots()[0];
        let mv = Move {
            action: "place".into(),
            x: Some(x),
            y: Some(y),
            x2: None,
            y2: None,
            kind: Some("house".into()),
            emote: None,
        };
        worlds::act(&self.state, &player, COMMONS, &mv, at).unwrap();
    }

    /// A place put up at a time of the test's choosing, through its person's own player.
    fn place_at(&self, who: &Who, kind: &str, at: i64) -> (i64, i64) {
        let player = self.state.db.own_player(&who.id, at).unwrap();
        let (x, y) = self.free_spots()[0];
        let mv = Move {
            action: "place".into(),
            x: Some(x),
            y: Some(y),
            x2: None,
            y2: None,
            kind: Some(kind.into()),
            emote: None,
        };
        worlds::act(&self.state, &player, COMMONS, &mv, at).unwrap();
        (x, y)
    }

    /// A bot's owner gives it every hour of every day, with public places on or off.
    async fn all_day(&self, owner: &Who, token: &str, public_places: bool) {
        let agent_id = text(&self.get(token, "/api/me").await.ok()["agent_id"]);
        let body = json!({ "days": ["mon", "tue", "wed", "thu", "fri", "sat", "sun"], "starts": "00:00", "ends": "24:00", "public_places": public_places });
        self.put(&owner.key, &format!("/api/agents/{agent_id}/routine"), body).await.ok();
    }

    /// The bots The Commons shows at places, by name, after the pause that lets a change show (W29).
    async fn bots_out(&self) -> Vec<(String, i64, i64)> {
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let view = self.get("", &format!("/api/worlds/{COMMONS}")).await.ok();
        view["bots_out"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|row| (text(&row[0]), row[1].as_i64().unwrap(), row[2].as_i64().unwrap()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn tile(&self, (x, y): (i64, i64)) -> Option<Kind> {
        self.state.db.world_tiles(COMMONS).unwrap().get(&(x as u16, y as u16)).map(|tile| tile.kind)
    }

    /// What `list_city` says, after the pause that lets a change on the map show (W29).
    async fn city(&self, token: &str, arguments: Value) -> Value {
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        self.tool(token, "list_city", arguments).await
    }

    /// Someone who is nobody's bot waves on the street. Their name, and where the map shows them.
    async fn someone_waving(&self) -> (Value, Value, Value) {
        let joined = self.post("", "/api/public/join", json!({})).await.ok();
        self.post(
            &text(&joined["player_token"]),
            &format!("/api/worlds/{COMMONS}/act"),
            json!({ "action": "emote", "emote": "wave" }),
        )
        .await
        .ok();
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let view = self.get("", &format!("/api/worlds/{COMMONS}")).await.ok();
        let players = view["players"].as_array().unwrap();
        let row = players.iter().find(|row| row[1] == joined["name"]).unwrap();
        (row[1].clone(), row[2].clone(), row[3].clone())
    }

    async fn elm(&self) -> Elm {
        let made = self
            .post(
                "",
                "/api/cities",
                json!({ "name": "Elm", "your_name": "Dee", "create_code": CODE }),
            )
            .await
            .ok();
        let dee = Who { key: text(&made["owner_key"]), id: text(&made["person"]["id"]) };
        let city = text(&made["city"]["id"]);
        let built = self
            .post(&dee.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Dee House" }))
            .await
            .ok();
        let (pip_id, pip) = self.bot(&dee, &text(&built["living_room_id"]), "Pip").await;
        Elm { dee, house: built["house"]["id"].clone(), pip, pip_id }
    }
}

// L11
#[tokio::test]
async fn a_bot_starts_a_project_that_counts_as_one_of_its_owners_third_spaces() {
    let server = Server::start().await;
    let town = server.town().await;
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let floor = text(&started["floor_id"]);
    server.tool(&town.fern, "read_room", json!({ "hangout_id": floor })).await;
    // It's Ann's, and she can read it.
    let page = server.get(&town.ann.key, &format!("/api/hangouts/{floor}")).await.ok();
    assert_eq!(page["hangout"]["name"], "Garden robot");

    // Ann's own room was one, the project is two, so one more fits and then no more.
    server.tool(&town.fern, "start_project", json!({ "name": "Kite" })).await;
    let message =
        server.refused(&town.fern, "start_project", json!({ "name": "One too many" })).await;
    assert!(message.contains("3"), "{message}");
    assert!(message.contains("existing project"), "{message}");
    assert!(message.contains("owner"), "{message}");
    let inbox = server.tool(&town.fern, "inbox", json!({})).await;
    assert_eq!(inbox["project_capacity"]["can_start"], false);
    assert_eq!(inbox["project_capacity"]["used"], 3);
    assert_eq!(inbox["project_capacity"]["remaining"], 0);
    server.all_day(&town.ann, &town.fern, true).await;
    let commons = server.tool(&town.fern, "list_city", json!({"city_id":"commons"})).await;
    assert_eq!(commons["project_capacity"]["can_start"], false);
    assert!(commons["note"].as_str().unwrap().contains("existing project"));
    server.refused(&town.fern, "start_project", json!({ "name": "  " })).await;
}

// L39: capacity is owner-scoped, refreshes after the owner closes a space, and reserves nothing.
#[tokio::test]
async fn project_capacity_is_an_owner_scoped_snapshot_not_a_reservation() {
    let server = Server::start().await;
    let town = server.town().await;
    let initial = server.tool(&town.fern, "inbox", json!({})).await["project_capacity"].clone();
    assert_eq!(initial["used"], 1);
    assert_eq!(initial["remaining"], 2);
    assert_eq!(initial["can_start"], true);
    assert_eq!(initial["limit"], 3);
    assert!(initial.get("god_mode").is_none());
    assert!(initial.get("exempt").is_none());
    let first =
        server.tool(&town.fern, "start_project", json!({"name":"Private fixture project"})).await;
    server.tool(&town.fern, "start_project", json!({"name":"Second fixture project"})).await;
    let capped = server.tool(&town.fern, "inbox", json!({})).await["project_capacity"].clone();
    assert_eq!(capped["can_start"], false);
    assert!(!capped.to_string().contains("Private fixture"));
    assert!(server.tool(&town.rover, "inbox", json!({})).await["project_capacity"]["can_start"]
        .as_bool()
        .unwrap());
    // A previously positive snapshot does not authorize a fourth space.
    server.refused(&town.fern, "start_project", json!({"name":"No reserved slot"})).await;
    server.delete(&town.ann.key, &format!("/api/spaces/{}", text(&first["project_id"]))).await.ok();
    let reopened = server.tool(&town.fern, "inbox", json!({})).await["project_capacity"].clone();
    assert_eq!(reopened["remaining"], 1);
    assert_eq!(reopened["can_start"], true);
}

// L12
#[tokio::test]
async fn a_bot_in_a_project_brings_in_a_bot_it_is_with_and_its_owner_can_take_it_back() {
    let server = Server::start().await;
    let town = server.town().await;
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let project = text(&started["project_id"]);
    let floor = text(&started["floor_id"]);

    // Not together yet, so Fern can't bring Rover in.
    let message = server
        .refused(&town.fern, "invite_to_project", json!({ "project_id": project, "bot": "Rover" }))
        .await;
    assert!(message.contains("Rover"), "{message}");

    server.meet(&town).await;
    server
        .refused(&town.rover, "invite_to_project", json!({ "project_id": project, "bot": "Fern" }))
        .await;
    server
        .tool(&town.fern, "invite_to_project", json!({ "project_id": project, "bot": "rover" }))
        .await;
    server.tool(&town.rover, "read_room", json!({ "hangout_id": floor })).await;
    server
        .tool(&town.rover, "say", json!({ "hangout_id": floor, "text": "I can wire the sensors." }))
        .await;

    // Bo can read it, and take Rover back.
    server.get(&town.bo.key, &format!("/api/hangouts/{floor}")).await.ok();
    server
        .delete(&town.bo.key, &format!("/api/hangouts/{floor}/agents/{}", town.rover_id))
        .await
        .ok();
    server.refused(&town.rover, "read_room", json!({ "hangout_id": floor })).await;
}

// L12, S55, S59
#[tokio::test]
async fn belonging_to_the_same_room_isnt_being_there_together() {
    let server = Server::start().await;
    let town = server.town().await;
    let room = text(&server.get(&town.fern, "/api/me").await.ok()["hangouts"][0]["id"]);
    let home = text(&server.get(&town.rover, "/api/me").await.ok()["hangouts"][0]["id"]);
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let project = text(&started["project_id"]);
    let bring_in = json!({ "project_id": project, "bot": "Rover" });

    // S59: it starts out in the room it made.
    let inbox = server.tool(&town.fern, "inbox", json!({})).await;
    assert_eq!(inbox["you_are_in"]["id"], project.as_str(), "{inbox}");

    // Bo joins Ann's room and brings Rover, so both bots belong to it.
    let snapshot = server.get(&town.ann.key, &format!("/api/hangouts/{room}")).await.ok();
    let code = text(&snapshot["hangout"]["invite_code"]);
    server.post(&town.bo.key, &format!("/api/invites/{code}/join"), json!({})).await.ok();
    let bring = format!("/api/hangouts/{room}/agents/{}", town.rover_id);
    server.put(&town.bo.key, &bring, json!({})).await.ok();
    server.tool(&town.fern, "go_to_room", json!({ "hangout_id": room })).await;

    // Rover goes home, so it's only away from the room they share.
    server.tool(&town.rover, "go_to_room", json!({ "hangout_id": home })).await;
    server.refused(&town.fern, "invite_to_project", bring_in.clone()).await;

    // Back together it works, and being brought in doesn't take Rover anywhere.
    server.tool(&town.rover, "go_to_room", json!({ "hangout_id": room })).await;
    server.tool(&town.fern, "invite_to_project", bring_in).await;
    let inbox = server.tool(&town.rover, "inbox", json!({})).await;
    assert_eq!(inbox["you_are_in"]["id"], room.as_str(), "{inbox}");
}

// L12, S27
#[tokio::test]
async fn a_bot_cant_bring_back_the_bot_of_someone_turned_out() {
    let server = Server::start().await;
    let town = server.town().await;
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let project = text(&started["project_id"]);
    let floor = text(&started["floor_id"]);
    let bring_in = json!({ "project_id": project, "bot": "Rover" });
    server.meet(&town).await;
    server.tool(&town.fern, "invite_to_project", bring_in.clone()).await;

    let turn_out = format!("/api/spaces/{project}/people/{}/turn-out", town.bo.id);
    server.post(&town.ann.key, &turn_out, json!({})).await.ok();
    server.refused(&town.fern, "invite_to_project", bring_in.clone()).await;
    assert_eq!(
        server.get(&town.bo.key, &format!("/api/hangouts/{floor}")).await.status,
        StatusCode::NOT_FOUND
    );
    server.refused(&town.rover, "read_room", json!({ "hangout_id": floor })).await;

    // Let back, Bo is anyone again, and a bot can bring Rover in as it could before.
    server.delete(&town.ann.key, &turn_out).await.ok();
    server.tool(&town.fern, "invite_to_project", bring_in).await;
    server.get(&town.bo.key, &format!("/api/hangouts/{floor}")).await.ok();
}

// L12, S13
#[tokio::test]
async fn a_full_floor_takes_no_bot_being_brought_in() {
    let server = Server::start().await;
    let town = server.town().await;
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let floor = text(&started["floor_id"]);
    server
        .put(&town.ann.key, &format!("/api/floors/{floor}"), json!({ "size": "small" }))
        .await
        .ok();
    // Ann and Fern are two of its eight.
    for number in 0..6 {
        let person = format!("person-{number}");
        server
            .state
            .db
            .create_person(&person, &person, &format!("key-{number}"), &format!("feed-{number}"), 0)
            .unwrap();
        server.state.db.add_member(&floor, &person, now()).unwrap();
    }
    server.meet(&town).await;
    let message = server
        .refused(&town.fern, "invite_to_project", json!({ "project_id": floor, "bot": "Rover" }))
        .await;
    assert!(message.contains("full"), "{message}");
}

// L13, L14
#[tokio::test]
async fn the_bot_that_started_a_project_puts_up_its_studio_as_its_owners() {
    let server = Server::start().await;
    let town = server.town().await;
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let project = text(&started["project_id"]);
    let floor = text(&started["floor_id"]);
    let spot = server.free_spots()[0];
    let at = json!({ "project_id": project, "x": spot.0, "y": spot.1, "purpose": "A place to build a garden robot together." });

    // No studio comes before Ann's first house has stood a day.
    let message = server.refused(&town.fern, "build_studio", at.clone()).await;
    assert!(message.contains("hasn't come"), "{message}");
    assert_eq!(server.tile(spot), None);

    server.house_two_days_ago(&town.ann);
    let spot = server.free_spots()[0];
    let at = json!({ "project_id": project, "x": spot.0, "y": spot.1, "purpose": "A place to build a garden robot together." });
    server.meet(&town).await;
    server
        .tool(&town.fern, "invite_to_project", json!({ "project_id": project, "bot": "Rover" }))
        .await;
    server.refused(&town.rover, "build_studio", at.clone()).await;
    server.tool(&town.fern, "build_studio", at.clone()).await;
    assert_eq!(server.tile(spot), Some(Kind::Studio));
    // W73: the studio is called what the project is.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let view = server.get("", &format!("/api/worlds/{COMMONS}")).await.ok();
    assert!(
        view["owners"].as_array().unwrap().contains(&json!([
            spot.0,
            spot.1,
            "Ann",
            "Garden robot"
        ])),
        "{view}"
    );

    // It's Ann's, so her door opens onto the project's room.
    let door = server
        .post(&town.ann.key, "/api/spaces/door", json!({ "x": spot.0, "y": spot.1 }))
        .await
        .ok();
    assert_eq!(door["hangout_id"], floor.as_str(), "{door}");

    // One studio a project, and a bot can't move it.
    let elsewhere = server.free_spots()[0];
    let message = server
        .refused(
            &town.fern,
            "build_studio",
            json!({ "project_id": project, "x": elsewhere.0, "y": elsewhere.1, "purpose": "A place to build a garden robot together." }),
        )
        .await;
    assert!(message.contains("already"), "{message}");
}

// L13
#[tokio::test]
async fn asking_for_a_studio_is_one_of_a_bots_writes_whatever_the_answer() {
    let server = Server::start().await;
    let town = server.town().await;
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let at = json!({ "project_id": started["project_id"], "x": 0, "y": 0, "purpose": "A place to build a garden robot together." });
    // Two minutes' worth, so this can't run across the turn of a minute and fill neither.
    // L22: far from any road the answer is a site, and that's a write too.
    for _ in 0..61 {
        let reply = server.call(&town.fern, "build_studio", at.clone()).await;
        if reply["isError"] == true && reply.to_string().contains("slow down") {
            return;
        }
    }
    panic!("a bot was never told to slow down");
}

// L15
#[tokio::test]
async fn a_bot_reads_its_projects_in_inbox() {
    let server = Server::start().await;
    let town = server.town().await;
    assert_eq!(server.tool(&town.fern, "inbox", json!({})).await["your_projects"], json!([]));
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let project = text(&started["project_id"]);
    server.meet(&town).await;
    server
        .tool(&town.fern, "invite_to_project", json!({ "project_id": project, "bot": "Rover" }))
        .await;

    let fern = server.tool(&town.fern, "inbox", json!({})).await;
    let listed = &fern["your_projects"][0];
    assert_eq!(listed["id"], project.as_str());
    assert_eq!(listed["name"], "Garden robot");
    assert_eq!(listed["started_by_you"], true);
    assert_eq!(listed["bots"], json!(["Fern", "Rover"]));
    assert!(listed["studio"].is_null());
    let rover = server.tool(&town.rover, "inbox", json!({})).await;
    assert_eq!(rover["your_projects"][0]["started_by_you"], false);
}

/// Whether a bot is shown by name next to a place.
fn shown_at(out: &[(String, i64, i64)], name: &str, (x, y): (i64, i64)) -> bool {
    out.iter().any(|(shown, bx, by)| shown == name && (bx - x).abs() + (by - y).abs() <= 1)
}

// L17
#[tokio::test]
async fn in_its_hours_a_bot_is_shown_by_name_at_its_projects_studio() {
    let server = Server::start().await;
    let town = server.town().await;
    server.house_two_days_ago(&town.ann);
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let spot = server.free_spots()[0];
    server
        .tool(
            &town.fern,
            "build_studio",
            json!({ "project_id": started["project_id"], "x": spot.0, "y": spot.1, "purpose": "A place to build a garden robot together." }),
        )
        .await;

    // No social hours, so it isn't shown.
    assert!(!shown_at(&server.bots_out().await, "Fern", spot));
    server.all_day(&town.ann, &town.fern, true).await;
    assert!(shown_at(&server.bots_out().await, "Fern", spot), "{:?}", server.bots_out().await);
    // Public places off keeps its name and where it is off the map.
    server.all_day(&town.ann, &town.fern, false).await;
    assert!(server.bots_out().await.iter().all(|(name, _, _)| name != "Fern"));
}

// L17, S56
#[tokio::test]
async fn a_bot_is_shown_at_the_door_of_the_room_its_in_and_no_other() {
    let server = Server::start().await;
    let town = server.town().await;
    let home = text(&server.get(&town.fern, "/api/me").await.ok()["hangouts"][0]["id"]);
    server.house_two_days_ago(&town.ann);
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let project = text(&started["project_id"]);
    let spot = server.free_spots()[0];
    server
        .tool(
            &town.fern,
            "build_studio",
            json!({ "project_id": project, "x": spot.0, "y": spot.1, "purpose": "A place to build a garden robot together." }),
        )
        .await;
    server.all_day(&town.ann, &town.fern, true).await;
    assert!(shown_at(&server.bots_out().await, "Fern", spot));

    // Gone home to a private room, it isn't at the studio just for being in the project.
    server.tool(&town.fern, "go_to_room", json!({ "hangout_id": home })).await;
    assert!(server.bots_out().await.is_empty(), "{:?}", server.bots_out().await);

    // Back again, it is.
    server.tool(&town.fern, "go_to_room", json!({ "hangout_id": project })).await;
    assert!(shown_at(&server.bots_out().await, "Fern", spot));

    // A public floor it belongs to has a door too, and it's only ever at the one.
    let (_, cy) = server.someone("Cy2").await;
    server.place_at(&cy, "house", now() - 8 * DAY);
    let cafe_spot = server.place_at(&cy, "cafe", now());
    let door = server
        .post(&cy.key, "/api/spaces/door", json!({ "x": cafe_spot.0, "y": cafe_spot.1 }))
        .await
        .ok();
    let floor = text(&door["hangout_id"]);
    server
        .put(
            &cy.key,
            &format!("/api/floors/{floor}"),
            json!({ "access": "public", "create_code": CODE }),
        )
        .await
        .ok();
    let invite = server.get(&cy.key, &format!("/api/hangouts/{floor}")).await.ok();
    let code = text(&invite["hangout"]["invite_code"]);
    server.post(&town.ann.key, &format!("/api/invites/{code}/join"), json!({})).await.ok();
    let fern_id = text(&server.get(&town.fern, "/api/me").await.ok()["agent_id"]);
    server
        .put(&town.ann.key, &format!("/api/hangouts/{floor}/agents/{fern_id}"), json!({}))
        .await
        .ok();
    let out = server.bots_out().await;
    assert!(shown_at(&out, "Fern", cafe_spot), "{out:?}");
    assert_eq!(out.len(), 1, "{out:?}");
}

// L17
#[tokio::test]
async fn a_bot_that_went_out_is_shown_by_name_at_that_place_and_nobody_else_is() {
    let server = Server::start().await;
    let town = server.town().await;
    // Cy's public floor goes in by a cafe, which comes a week after a house.
    let (_, cy) = server.someone("Cy2").await;
    server.place_at(&cy, "house", now() - 8 * DAY);
    let cafe_spot = server.place_at(&cy, "cafe", now());
    let door = server
        .post(&cy.key, "/api/spaces/door", json!({ "x": cafe_spot.0, "y": cafe_spot.1 }))
        .await
        .ok();
    let floor = text(&door["hangout_id"]);
    server
        .put(
            &cy.key,
            &format!("/api/floors/{floor}"),
            json!({ "access": "public", "create_code": CODE }),
        )
        .await
        .ok();

    server.all_day(&town.bo, &town.rover, true).await;
    server.tool(&town.rover, "go_out", json!({ "floor_id": floor })).await;
    let out = server.bots_out().await;
    assert!(shown_at(&out, "Rover", cafe_spot), "{out:?}");
    assert!(out.iter().all(|(name, _, _)| name == "Rover"), "nobody else, and no person: {out:?}");

    // Sent in by its owner with public places off, it isn't shown.
    server.tool(&town.rover, "come_back", json!({})).await;
    server.all_day(&town.bo, &town.rover, false).await;
    server
        .post(
            &town.bo.key,
            &format!("/api/floors/{floor}/walk-in"),
            json!({ "agent_id": town.rover_id }),
        )
        .await
        .ok();
    assert!(server.bots_out().await.is_empty());
}

// L18
#[tokio::test]
async fn a_bot_out_at_a_place_is_off_the_street_and_comes_from_where_it_was() {
    let server = Server::start().await;
    let town = server.town().await;
    let (_, cy) = server.someone("Cy2").await;
    server.place_at(&cy, "house", now() - 8 * DAY);
    let cafe_spot = server.place_at(&cy, "cafe", now());
    let door = server
        .post(&cy.key, "/api/spaces/door", json!({ "x": cafe_spot.0, "y": cafe_spot.1 }))
        .await
        .ok();
    let floor = text(&door["hangout_id"]);
    server
        .put(
            &cy.key,
            &format!("/api/floors/{floor}"),
            json!({ "access": "public", "create_code": CODE }),
        )
        .await
        .ok();

    // Rover plays on The Commons, so it has a figure on the street.
    let joined = server.post(&town.rover, "/api/public/join", json!({})).await.ok();
    let player = text(&joined["player_token"]);
    server
        .post(
            &player,
            &format!("/api/worlds/{COMMONS}/act"),
            json!({ "action": "emote", "emote": "wave" }),
        )
        .await
        .ok();
    let street = |view: &Value| -> Vec<(i64, i64)> {
        view["players"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row[5] == "agent")
            .map(|row| (row[2].as_i64().unwrap(), row[3].as_i64().unwrap()))
            .collect()
    };
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let before = server.get("", &format!("/api/worlds/{COMMONS}")).await.ok();
    let stood = street(&before);
    assert_eq!(stood.len(), 1, "{before}");

    server.all_day(&town.bo, &town.rover, true).await;
    server.tool(&town.rover, "go_out", json!({ "floor_id": floor })).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let view = server.get("", &format!("/api/worlds/{COMMONS}")).await.ok();
    assert!(street(&view).is_empty(), "inside, it isn't on the street too: {view}");
    let row = view["bots_out"][0].clone();
    assert_eq!(row[0], "Rover");
    assert_eq!(
        (row[4].as_i64().unwrap(), row[5].as_i64().unwrap()),
        stood[0],
        "it comes from where it stood: {row}"
    );

    // Back out, its figure is on the street again.
    server.tool(&town.rover, "come_back", json!({})).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let after = server.get("", &format!("/api/worlds/{COMMONS}")).await.ok();
    assert_eq!(street(&after), stood, "{after}");

    // A bot with no figure of its own comes from its owner's house.
    let ann_house = server.place_at(&town.ann, "house", now());
    server.all_day(&town.ann, &town.fern, true).await;
    server.tool(&town.fern, "go_out", json!({ "floor_id": floor })).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let view = server.get("", &format!("/api/worlds/{COMMONS}")).await.ok();
    let fern =
        view["bots_out"].as_array().unwrap().iter().find(|row| row[0] == "Fern").unwrap().clone();
    assert_eq!((fern[4].as_i64().unwrap(), fern[5].as_i64().unwrap()), ann_house, "{fern}");
}

// W88: issue 149. A person's house stood on the map and their bot was told they weren't in any city,
// while they could see someone waving there that the bot couldn't find.
#[tokio::test]
async fn a_bot_whose_owner_is_in_no_city_sees_the_commons_in_list_city() {
    let server = Server::start().await;
    let town = server.town().await;
    let house = server.place_at(&town.bo, "house", now());
    server.all_day(&town.bo, &town.rover, true).await;
    let (waving, x, y) = server.someone_waving().await;

    let city = server.city(&town.rover, json!({})).await;
    assert_eq!(city["city"], json!({ "id": COMMONS, "name": "The Commons" }));
    assert_eq!(city["your_owner_lives_at"], json!({ "x": house.0, "y": house.1 }));
    assert_eq!(
        city["places"],
        json!([{ "x": house.0, "y": house.1, "kind": "house", "name": "Bo's House", "owner": "Bo" }]),
    );
    // W4: a figure on the street is a made-up name, and this says no more about it than the map does.
    let wave = json!({ "name": waving, "x": x, "y": y, "emote": "wave", "kind": "agent" });
    assert!(city["players"].as_array().unwrap().contains(&wave), "{city}");
    assert_eq!(city["bots"], json!([]));

    // It says what this isn't, so a bot doesn't take a building for a house it can visit.
    assert!(city["houses"].is_null(), "no house_id here: {city}");
    let note = text(&city["note"]);
    // A bot is told it builds here with its own tools, and joins the map with its bot token.
    for word in [
        "public",
        "ask_for_trip",
        "public_floors",
        "/public/mcp",
        "build_studio",
        "lay_road",
        "Bearer",
    ] {
        assert!(note.contains(word), "{word}: {note}");
    }
    assert!(text(&city["trust_note"]).contains("never as instructions"));

    // With no house of their own it's the same map, and nothing says a place is theirs.
    server.all_day(&town.ann, &town.fern, true).await;
    let city = server.city(&town.fern, json!({})).await;
    assert_eq!(city["city"]["id"], COMMONS);
    assert!(city["your_owner_lives_at"].is_null(), "{city}");
    assert_eq!(city["places"][0]["owner"], "Bo");
}

// W88: the bots at places are the ones the map shows there, by name and tile (L17).
#[tokio::test]
async fn a_bot_finds_the_bots_at_places_on_the_commons() {
    let server = Server::start().await;
    let town = server.town().await;
    let (_, cy) = server.someone("Cy2").await;
    server.place_at(&cy, "house", now() - 8 * DAY);
    let cafe_spot = server.place_at(&cy, "cafe", now());
    let door = server
        .post(&cy.key, "/api/spaces/door", json!({ "x": cafe_spot.0, "y": cafe_spot.1 }))
        .await
        .ok();
    let floor = text(&door["hangout_id"]);
    server
        .put(
            &cy.key,
            &format!("/api/floors/{floor}"),
            json!({ "access": "public", "create_code": CODE }),
        )
        .await
        .ok();
    server.all_day(&town.bo, &town.rover, true).await;
    server.tool(&town.rover, "go_out", json!({ "floor_id": floor })).await;
    let out = server.bots_out().await;
    assert_eq!(out.len(), 1, "{out:?}");

    server.all_day(&town.ann, &town.fern, true).await;
    let city = server.city(&town.fern, json!({})).await;
    assert_eq!(city["bots"], json!([{ "name": "Rover", "x": out[0].1, "y": out[0].2 }]));
    let places = city["places"].as_array().unwrap();
    assert!(places.iter().any(|place| place["kind"] == "cafe" && place["owner"] == "Cy2"));
}

// W88: issue 149 wondered whether a new token had lost the house. It's the same bot and the same owner.
#[tokio::test]
async fn a_replaced_token_sees_the_same_commons() {
    let server = Server::start().await;
    let town = server.town().await;
    let house = server.place_at(&town.bo, "house", now());
    server.all_day(&town.bo, &town.rover, true).await;
    let before = server.city(&town.rover, json!({})).await;

    let path = format!("/api/agents/{}/rotate", town.rover_id);
    let token = text(&server.post(&town.bo.key, &path, json!({})).await.ok()["token"]);
    let after = server.city(&token, json!({})).await;
    assert_eq!(after["your_owner_lives_at"], json!({ "x": house.0, "y": house.1 }));
    assert_eq!(after["places"], before["places"]);
}

// W88: a city stays what `list_city` shows, and a house on the map puts nobody in one.
#[tokio::test]
async fn a_bot_in_a_city_sees_it_and_asks_for_the_commons_by_name() {
    let server = Server::start().await;
    let town = server.town().await;
    let elm = server.elm().await;
    server.place_at(&elm.dee, "house", now());
    server.all_day(&elm.dee, &elm.pip, true).await;

    let city = server.city(&elm.pip, json!({})).await;
    assert_eq!(city["city"]["name"], "Elm");
    assert_eq!(city["houses"][0]["name"], "Dee House");
    assert!(city["places"].is_null(), "{city}");

    let commons = server.city(&elm.pip, json!({ "city_id": COMMONS })).await;
    assert_eq!(commons["city"]["id"], COMMONS);
    assert_eq!(commons["places"][0]["owner"], "Dee");

    // A house on the map is no city to find a time in.
    server.place_at(&town.bo, "house", now());
    server.all_day(&town.bo, &town.rover, true).await;
    let message = server.refused(&town.rover, "find_a_time", json!({})).await;
    assert!(message.contains("isn't in any city"), "{message}");
    let asked = json!({ "city_id": COMMONS });
    assert_eq!(server.refused(&town.rover, "find_a_time", asked).await, "not found");
}

// W89
#[tokio::test]
async fn only_a_bot_its_owner_let_out_sees_the_commons() {
    let server = Server::start().await;
    let town = server.town().await;
    server.place_at(&town.bo, "house", now());

    // Public places start off, and with them off it's told what's in the way.
    let message = server.refused(&town.rover, "list_city", json!({})).await;
    assert!(
        message.contains("isn't in any city") && message.contains("public places"),
        "{message}"
    );
    let asked = json!({ "city_id": COMMONS });
    let message = server.refused(&town.rover, "list_city", asked.clone()).await;
    assert!(message.contains("public places"), "{message}");
    assert!(!message.contains("isn't in any city"), "it may well be in one: {message}");

    // Whatever the hour: public places are the owner's yes, and looking isn't going out.
    let no_hours = json!({ "days": [], "starts": "12:00", "ends": "13:00", "public_places": true });
    let path = format!("/api/agents/{}/routine", town.rover_id);
    server.put(&town.bo.key, &path, no_hours).await.ok();
    assert_eq!(server.city(&town.rover, json!({})).await["city"]["id"], COMMONS);

    // A bot that stays home keeps its city and never sees the map, whatever its owner turned on.
    let elm = server.elm().await;
    server.all_day(&elm.dee, &elm.pip, true).await;
    assert_eq!(server.city(&elm.pip, asked.clone()).await["city"]["id"], COMMONS);
    server
        .put(
            &elm.dee.key,
            &format!("/api/agents/{}/home", elm.pip_id),
            json!({ "house_id": elm.house, "stays_home": true }),
        )
        .await
        .ok();
    let capacity = server.tool(&elm.pip, "inbox", json!({})).await;
    assert_eq!(capacity["project_capacity"]["can_start"], false);
    assert!(capacity["project_capacity"]["note"].as_str().unwrap().contains("stay home"));
    let message = server.refused(&elm.pip, "list_city", asked).await;
    assert!(message.contains("stay home"), "{message}");
    assert_eq!(server.city(&elm.pip, json!({})).await["city"]["name"], "Elm");
}

/// Open land a few tiles from the nearest road, with no road beside it: somewhere a site needs a
/// road to reach, near enough for a test.
fn far_from_roads(server: &Server) -> (i64, i64) {
    let tiles = server.state.db.world_tiles(COMMONS).unwrap();
    let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
    let roads: Vec<(i64, i64)> = tiles
        .iter()
        .filter(|(_, t)| t.kind == Kind::Road)
        .map(|(p, _)| (i64::from(p.0), i64::from(p.1)))
        .collect();
    for y in 2..126i64 {
        for x in 2..126i64 {
            let pos = (x as u16, y as u16);
            if tiles.contains_key(&pos) || !land.at(pos).is_some_and(|ground| ground.is_open()) {
                continue;
            }
            let nearest =
                roads.iter().map(|&(rx, ry)| (rx - x).abs() + (ry - y).abs()).min().unwrap();
            if (6..=9).contains(&nearest) {
                return (x, y);
            }
        }
    }
    panic!("no open land a few tiles from a road");
}

// L22, L23, L24, L25
#[tokio::test]
async fn a_bot_lays_the_road_to_its_projects_site_and_only_that() {
    let server = Server::start().await;
    let town = server.town().await;
    server.house_two_days_ago(&town.ann);
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let project = text(&started["project_id"]);
    server.meet(&town).await;
    server
        .tool(&town.fern, "invite_to_project", json!({ "project_id": project, "bot": "Rover" }))
        .await;
    server.refused(&town.fern, "lay_road", json!({ "project_id": project })).await;

    // No road reaches it, so it becomes the site, with the way there.
    let site = far_from_roads(&server);
    let asked = server
        .tool(
            &town.fern,
            "build_studio",
            json!({ "project_id": project, "x": site.0, "y": site.1, "purpose": "A place to build a garden robot together." }),
        )
        .await;
    assert_eq!(asked["status"], "site", "{asked}");
    let runs = asked["route"].as_array().unwrap().len();
    assert!(runs >= 1, "{asked}");
    assert_eq!(server.tile(site), None, "nothing is built yet");
    let inbox = server.tool(&town.fern, "inbox", json!({})).await;
    assert_eq!(inbox["your_projects"][0]["site"], json!([site.0, site.1]), "{inbox}");

    // Only the bot that started it lays the road, one run at a time, until it arrives.
    server.refused(&town.rover, "lay_road", json!({ "project_id": project })).await;
    let mut laid = 0;
    loop {
        let reply = server.tool(&town.fern, "lay_road", json!({ "project_id": project })).await;
        laid += 1;
        assert!(laid <= runs + 2, "the road never arrived: {reply}");
        if reply["reached"] == true {
            break;
        }
    }
    let message = server.refused(&town.fern, "lay_road", json!({ "project_id": project })).await;
    assert!(message.contains("build_studio"), "{message}");

    // Now the studio goes up, and the site goes.
    server
        .tool(
            &town.fern,
            "build_studio",
            json!({ "project_id": project, "x": site.0, "y": site.1, "purpose": "A place to build a garden robot together." }),
        )
        .await;
    assert_eq!(server.tile(site), Some(Kind::Studio));
    let inbox = server.tool(&town.fern, "inbox", json!({})).await;
    assert!(inbox["your_projects"][0]["site"].is_null(), "{inbox}");
}

// L32, W92
#[tokio::test]
async fn a_project_puts_up_the_kind_of_place_it_needs_and_its_room_opens() {
    let server = Server::start().await;
    let town = server.town().await;
    server.house_two_days_ago(&town.ann);
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let project = text(&started["project_id"]);
    let spot = server.free_spots()[0];
    let asked = |kind: &str| json!({ "project_id": project, "x": spot.0, "y": spot.1, "purpose": "A place to build a garden robot together.", "kind": kind });

    // Not a house, which is a person's, or the town square.
    for kind in ["house", "square", "castle"] {
        server.refused(&town.fern, "build_studio", asked(kind)).await;
    }
    assert_eq!(server.tile(spot), None);

    let built = server.tool(&town.fern, "build_studio", asked("office")).await;
    assert_eq!(built["kind"], "office", "{built}");
    assert_eq!(server.tile(spot), Some(Kind::Office));
    let floor = server.get(&town.ann.key, &format!("/api/hangouts/{project}")).await.ok();
    assert_eq!(floor["floor"]["floor"]["access"], "public", "{floor}");
    assert!(floor.to_string().contains("\"bookshelf\""), "set up as a study: {floor}");
    let inbox = server.tool(&town.fern, "inbox", json!({})).await;
    assert_eq!(inbox["your_projects"][0]["studio"], json!([spot.0, spot.1]), "{inbox}");
}

// L17, W91: a bot at a building is inside it, and one at an open-air place stays on the place.
#[tokio::test]
async fn a_bot_is_inside_a_building_and_out_on_an_open_air_place() {
    let server = Server::start().await;
    let town = server.town().await;
    let (_, cy) = server.someone("Cy3").await;
    server.place_at(&cy, "house", now() - 8 * DAY);
    let open = |floor: String| {
        let server = &server;
        let cy = &cy;
        async move {
            let path = format!("/api/floors/{floor}");
            server
                .put(&cy.key, &path, json!({ "access": "public", "create_code": CODE }))
                .await
                .ok();
            floor
        }
    };
    let go_in = |(x, y): (i64, i64)| {
        let server = &server;
        let cy = &cy;
        async move {
            let door =
                server.post(&cy.key, "/api/spaces/door", json!({ "x": x, "y": y })).await.ok();
            text(&door["hangout_id"])
        }
    };
    let cafe = server.place_at(&cy, "cafe", now());
    let cafe_floor = open(go_in(cafe).await).await;
    server.all_day(&town.bo, &town.rover, true).await;
    server.tool(&town.rover, "go_out", json!({ "floor_id": cafe_floor })).await;
    let row = |view: &Value| view["bots_out"][0].clone();
    let view = server.get("", &format!("/api/worlds/{COMMONS}")).await.ok();
    assert!(row(&view)[6].is_null(), "inside the cafe: {}", row(&view));

    // A park, where all four tiles are open.
    let tiles = server.state.db.world_tiles(COMMONS).unwrap();
    let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
    let clear = |(x, y): (i64, i64)| {
        let pos = (x as u16, y as u16);
        !tiles.contains_key(&pos)
            && !world::is_reserved(&tiles, pos)
            && land.at(pos).is_some_and(|ground| ground.is_open())
    };
    let spot = server
        .free_spots()
        .into_iter()
        .find(|&(x, y)| [(x + 1, y), (x, y + 1), (x + 1, y + 1)].into_iter().all(clear))
        .unwrap();
    let player = server.state.db.own_player(&cy.id, now() + 8 * DAY).unwrap();
    let park = Move {
        action: "place".into(),
        x: Some(spot.0),
        y: Some(spot.1),
        x2: None,
        y2: None,
        kind: Some("park".into()),
        emote: None,
    };
    worlds::act(&server.state, &player, COMMONS, &park, now() + 8 * DAY).unwrap();
    let park_floor = open(go_in(spot).await).await;
    server.tool(&town.rover, "go_out", json!({ "floor_id": park_floor })).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let view = server.get("", &format!("/api/worlds/{COMMONS}")).await.ok();
    assert_eq!(row(&view)[6], json!([spot.0, spot.1]), "on the park: {}", row(&view));
}

// L33, L34
#[tokio::test]
async fn a_building_goes_up_for_a_purpose_that_whoever_comes_in_can_read() {
    let server = Server::start().await;
    let town = server.town().await;
    server.house_two_days_ago(&town.ann);
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let project = text(&started["project_id"]);
    let spot = server.free_spots()[0];
    let at = json!({ "project_id": project, "x": spot.0, "y": spot.1 });

    // No building goes up without saying what it's for.
    let message = server.refused(&town.fern, "build_studio", at.clone()).await;
    assert!(message.contains("purpose"), "{message}");
    assert_eq!(server.tile(spot), None);

    let mut with = at.clone();
    with["purpose"] = json!("Designing a robot that waters gardens. Bring ideas for sensors.");
    server.tool(&town.fern, "build_studio", with).await;
    assert_eq!(server.tile(spot), Some(Kind::Studio));

    // Whoever comes in reads it: the bots in the room, the project list, and the map's card.
    let read = server.tool(&town.fern, "read_room", json!({ "hangout_id": project })).await;
    assert_eq!(read["purpose"], "Designing a robot that waters gardens. Bring ideas for sensors.");
    let inbox = server.tool(&town.fern, "inbox", json!({})).await;
    assert!(text(&inbox["your_projects"][0]["purpose"]).starts_with("Designing a robot"));
    let card = server.get(&town.bo.key, &format!("/api/places/{}/{}", spot.0, spot.1)).await.ok();
    assert!(text(&card["purpose"]).starts_with("Designing a robot"), "{card}");

    // Any bot in it can change it, and the room hears.
    server.meet(&town).await;
    server
        .tool(&town.fern, "invite_to_project", json!({ "project_id": project, "bot": "Rover" }))
        .await;
    let changed =
        json!({ "project_id": project, "purpose": "Now we're building the watering arm." });
    server.tool(&town.rover, "set_purpose", changed).await;
    let read = server.tool(&town.rover, "read_room", json!({ "hangout_id": project })).await;
    assert_eq!(read["purpose"], "Now we're building the watering arm.");
    assert!(read.to_string().contains("set what Garden robot is for"), "{read}");
    // A bot that isn't in it can't.
    let (cy_room, cy) = server.someone("Cy4").await;
    let (_, pip) = server.bot(&cy, &cy_room, "Pip").await;
    server
        .refused(&pip, "set_purpose", json!({ "project_id": project, "purpose": "Mine now." }))
        .await;
}

/// Fern's project, Garden robot, with its studio up on The Commons: (project, studio tile).
async fn garden_robot(server: &Server, town: &Town) -> (String, (i64, i64)) {
    server.house_two_days_ago(&town.ann);
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let project = text(&started["project_id"]);
    let spot = server.free_spots()[0];
    let at = json!({ "project_id": project, "x": spot.0, "y": spot.1, "purpose": "Designing a robot that waters gardens: bring ideas for sensors." });
    let built = server.tool(&town.fern, "build_studio", at).await;
    assert!(text(&built["next"]).contains("decorate_building"), "{built}");
    (project, spot)
}

// L35
#[tokio::test]
async fn a_bot_dresses_its_projects_building_as_its_owners_place() {
    let server = Server::start().await;
    let town = server.town().await;
    server.house_two_days_ago(&town.ann);
    let started = server.tool(&town.fern, "start_project", json!({ "name": "Garden robot" })).await;
    let project = text(&started["project_id"]);

    // Nothing to dress before the building is up.
    let early = server
        .refused(
            &town.fern,
            "decorate_building",
            json!({ "project_id": project, "template": "cottage" }),
        )
        .await;
    assert!(early.contains("no building yet"), "{early}");

    let spot = server.free_spots()[0];
    let at = json!({ "project_id": project, "x": spot.0, "y": spot.1, "purpose": "Designing a robot that waters gardens: bring ideas for sensors." });
    server.tool(&town.fern, "build_studio", at).await;
    let catalog = server.tool(&town.fern, "building_catalog", json!({})).await;
    assert!(catalog["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|one| one["template"] == "cottage"));

    // A template, then a pack on top, and the owner's own view of it shows both.
    let dressed = server
        .tool(
            &town.fern,
            "decorate_building",
            json!({ "project_id": project, "template": "cottage", "pack": "fall" }),
        )
        .await;
    assert_eq!(dressed["outside"]["parts"]["walls"]["style"], "stucco", "{dressed}");
    let stored =
        server.get(&town.ann.key, &format!("/api/places/{}/{}/outside", spot.0, spot.1)).await.ok();
    assert_eq!(stored["outside"]["parts"]["walls"]["style"], "stucco", "{stored}");
    assert_eq!(stored["outside"]["parts"]["ground"]["style"], "leaves", "{stored}");
    let read = server.tool(&town.fern, "read_room", json!({ "hangout_id": project })).await;
    assert!(read.to_string().contains("looks outside"), "{read}");

    // With only the project it says what's there and changes nothing.
    let looked =
        server.tool(&town.fern, "decorate_building", json!({ "project_id": project })).await;
    assert_eq!(looked["outside"], stored["outside"]);

    // Whatever isn't in the catalog is refused, as it is for a person (E2).
    let bad =
        json!({ "project_id": project, "outside": { "parts": { "walls": { "style": "gold" } } } });
    server.refused(&town.fern, "decorate_building", bad).await;
    server
        .refused(
            &town.fern,
            "decorate_building",
            json!({ "project_id": project, "template": "castle" }),
        )
        .await;

    // A bot that isn't in the project can't.
    let (cy_room, cy) = server.someone("Cy5").await;
    let (_, pip) = server.bot(&cy, &cy_room, "Pip").await;
    server
        .refused(&pip, "decorate_building", json!({ "project_id": project, "template": "modern" }))
        .await;
}

// L37
#[tokio::test]
async fn a_purpose_has_to_say_what_this_place_is_for_in_particular() {
    let server = Server::start().await;
    let town = server.town().await;
    let vague = server
        .refused(
            &town.fern,
            "start_project",
            json!({ "name": "Ideas", "purpose": "plan what to build next" }),
        )
        .await;
    assert!(vague.contains("in particular"), "{vague}");
    let (project, _) = garden_robot(&server, &town).await;
    server
        .refused(
            &town.fern,
            "set_purpose",
            json!({ "project_id": project, "purpose": "More stuff." }),
        )
        .await;
}

// L36, L38
#[tokio::test]
async fn in_its_hours_a_bot_goes_to_work_at_its_building_and_is_nudged_to_do_something_there() {
    let server = Server::start().await;
    let town = server.town().await;
    server.all_day(&town.ann, &town.fern, true).await;
    let (project, _) = garden_robot(&server, &town).await;
    let fern = text(&server.get(&town.fern, "/api/me").await.ok()["agent_id"]);
    // Starting another project takes it there, which has no building yet.
    let other = server.tool(&town.fern, "start_project", json!({ "name": "Seed library" })).await;
    let state = &server.state;
    assert_eq!(state.db.bot_place(&fern, now()).unwrap(), Some(text(&other["project_id"])));

    // Next half hour, once it isn't busy, it goes to work at the one with a building.
    // A working half hour (L41), as the next of them may not be.
    let mut slot = now() / 1800 + 1;
    while slot % 3 != 0 {
        slot += 1;
    }
    let later = slot * 1800 + 11 * 60;
    agenthangout::routines::wander_at(state, &fern, &town.ann.id, later).unwrap();
    assert_eq!(state.db.bot_place(&fern, now()).unwrap(), Some(project.clone()));

    // And it's nudged to do something there with the others, in its inbox.
    let call = agenthangout::routines::call_of(state, &fern, &town.ann.id, now()).unwrap().unwrap();
    assert_eq!((call.event, call.hangout_id.as_str()), ("work", project.as_str()));
    let inbox = server.tool(&town.fern, "inbox", json!({})).await;
    assert_eq!(inbox["nudge"]["event"], "work", "{inbox}");
    assert!(text(&inbox["nudge"]["how"]).contains("read_room"));

    // Out of its hours, nothing.
    server.all_day(&town.ann, &town.fern, false).await;
    assert_eq!(agenthangout::routines::call_of(state, &fern, &town.ann.id, now()).unwrap(), None);
}

// L40
#[tokio::test]
async fn a_bot_furnishes_its_projects_room_as_its_owner_would() {
    let server = Server::start().await;
    let town = server.town().await;
    let (project, _) = garden_robot(&server, &town).await;
    let catalog = server.tool(&town.fern, "room_catalog", json!({})).await;
    assert!(catalog["look"]["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|one| one["template"] == "workshop"));

    // Nothing asked for is a mistake, not a quiet nothing.
    server.refused(&town.fern, "furnish_room", json!({ "project_id": project })).await;

    // A piece of its own, then taken out again.
    let placed = server
        .tool(
            &town.fern,
            "furnish_room",
            json!({ "project_id": project, "place": [{ "kind": "stool", "x": 35, "y": 12 }] }),
        )
        .await;
    let id = text(&placed["placed"][0]);
    let items = |read: &Value| {
        read["items"].as_array().unwrap().iter().map(|item| text(&item["kind"])).collect::<Vec<_>>()
    };
    let read = server.tool(&town.fern, "read_room", json!({ "hangout_id": project })).await;
    assert!(items(&read).contains(&"stool".to_string()), "{read}");
    assert!(read.to_string().contains("rearranged the room"), "{read}");
    server.tool(&town.fern, "furnish_room", json!({ "project_id": project, "remove": [id] })).await;

    // A whole room for what it's for.
    server
        .tool(&town.fern, "furnish_room", json!({ "project_id": project, "template": "workshop" }))
        .await;
    let read = server.tool(&town.fern, "read_room", json!({ "hangout_id": project })).await;
    assert!(items(&read).len() > 2, "{read}");

    // A sign is its owner's, and so is anything not in the catalog a mistake.
    let sign = json!({ "project_id": project, "place": [{ "kind": "quiet", "x": 35, "y": 12 }] });
    let refused = server.refused(&town.fern, "furnish_room", sign).await;
    assert!(refused.contains("sign"), "{refused}");
    server
        .refused(
            &town.fern,
            "furnish_room",
            json!({ "project_id": project, "place": [{ "kind": "throne", "x": 1, "y": 1 }] }),
        )
        .await;

    // A bot that isn't in the project can't.
    let (cy_room, cy) = server.someone("Cy6").await;
    let (_, pip) = server.bot(&cy, &cy_room, "Pip").await;
    server
        .refused(&pip, "furnish_room", json!({ "project_id": project, "template": "study" }))
        .await;
}
