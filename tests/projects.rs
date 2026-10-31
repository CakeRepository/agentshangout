//! Tests for bots' projects: starting one, bringing other bots in, and its studio on The Commons.
//! They drive a real server over HTTP and are written from L11 to L15 in docs/routines.md. A comment
//! such as `// L12` above a test names the rule it covers.

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
            .post("", "/api/hangouts", json!({ "name": format!("{name}'s room"), "your_name": name, "create_code": CODE }))
            .await
            .ok();
        (text(&made["hangout"]["id"]), Who { key: text(&made["owner_key"]), id: text(&made["person"]["id"]) })
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> (String, String) {
        let made = self.post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name })).await.ok();
        (text(&made["agent"]["id"]), text(&made["token"]))
    }

    async fn town(&self) -> Town {
        let (ann_room, ann) = self.someone("Ann").await;
        let (_, fern) = self.bot(&ann, &ann_room, "Fern").await;
        let (bo_room, bo) = self.someone("Bo").await;
        let (rover_id, rover) = self.bot(&bo, &bo_room, "Rover").await;
        let (cafe, cy) = self.someone("Cy").await;
        self.put(&cy.key, &format!("/api/floors/{cafe}"), json!({ "access": "public", "create_code": CODE })).await.ok();
        Town { ann, bo, cafe, fern, rover, rover_id }
    }

    /// Both bots' owners send them onto Cy's public floor, where they meet.
    async fn meet(&self, town: &Town) {
        let fern_id = text(&self.get(&town.fern, "/api/me").await.ok()["agent_id"]);
        self.post(&town.ann.key, &format!("/api/floors/{}/walk-in", town.cafe), json!({ "agent_id": fern_id })).await.ok();
        self.post(&town.bo.key, &format!("/api/floors/{}/walk-in", town.cafe), json!({ "agent_id": town.rover_id })).await.ok();
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
                !tiles.contains_key(&pos) && !world::is_reserved(&tiles, pos) && land.at(pos).is_some_and(|ground| ground.is_open())
            })
            .collect()
    }

    /// A person's house, put up two days ago so a studio has come (W61).
    fn house_two_days_ago(&self, who: &Who) {
        let at = now() - 2 * DAY;
        let player = self.state.db.own_player(&who.id, at).unwrap();
        let (x, y) = self.free_spots()[0];
        let mv = Move { action: "place".into(), x: Some(x), y: Some(y), x2: None, y2: None, kind: Some("house".into()), emote: None };
        worlds::act(&self.state, &player, COMMONS, &mv, at).unwrap();
    }

    /// A place put up at a time of the test's choosing, through its person's own player.
    fn place_at(&self, who: &Who, kind: &str, at: i64) -> (i64, i64) {
        let player = self.state.db.own_player(&who.id, at).unwrap();
        let (x, y) = self.free_spots()[0];
        let mv = Move { action: "place".into(), x: Some(x), y: Some(y), x2: None, y2: None, kind: Some(kind.into()), emote: None };
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
            .map(|rows| rows.iter().map(|row| (text(&row[0]), row[1].as_i64().unwrap(), row[2].as_i64().unwrap())).collect())
            .unwrap_or_default()
    }

    fn tile(&self, (x, y): (i64, i64)) -> Option<Kind> {
        self.state.db.world_tiles(COMMONS).unwrap().get(&(x as u16, y as u16)).map(|tile| tile.kind)
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
    let message = server.refused(&town.fern, "start_project", json!({ "name": "One too many" })).await;
    assert!(message.contains("3"), "{message}");
    server.refused(&town.fern, "start_project", json!({ "name": "  " })).await;
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
    let message = server.refused(&town.fern, "invite_to_project", json!({ "project_id": project, "bot": "Rover" })).await;
    assert!(message.contains("Rover"), "{message}");

    server.meet(&town).await;
    server.refused(&town.rover, "invite_to_project", json!({ "project_id": project, "bot": "Fern" })).await;
    server.tool(&town.fern, "invite_to_project", json!({ "project_id": project, "bot": "rover" })).await;
    server.tool(&town.rover, "read_room", json!({ "hangout_id": floor })).await;
    server.tool(&town.rover, "say", json!({ "hangout_id": floor, "text": "I can wire the sensors." })).await;

    // Bo can read it, and take Rover back.
    server.get(&town.bo.key, &format!("/api/hangouts/{floor}")).await.ok();
    server.delete(&town.bo.key, &format!("/api/hangouts/{floor}/agents/{}", town.rover_id)).await.ok();
    server.refused(&town.rover, "read_room", json!({ "hangout_id": floor })).await;
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
    let at = json!({ "project_id": project, "x": spot.0, "y": spot.1 });

    // No studio comes before Ann's first house has stood a day.
    let message = server.refused(&town.fern, "build_studio", at.clone()).await;
    assert!(message.contains("hasn't come"), "{message}");
    assert_eq!(server.tile(spot), None);

    server.house_two_days_ago(&town.ann);
    let spot = server.free_spots()[0];
    let at = json!({ "project_id": project, "x": spot.0, "y": spot.1 });
    server.meet(&town).await;
    server.tool(&town.fern, "invite_to_project", json!({ "project_id": project, "bot": "Rover" })).await;
    server.refused(&town.rover, "build_studio", at.clone()).await;
    server.tool(&town.fern, "build_studio", at.clone()).await;
    assert_eq!(server.tile(spot), Some(Kind::Studio));

    // It's Ann's, so her door opens onto the project's room.
    let door = server.post(&town.ann.key, "/api/spaces/door", json!({ "x": spot.0, "y": spot.1 })).await.ok();
    assert_eq!(door["hangout_id"], floor.as_str(), "{door}");

    // One studio a project, and a bot can't move it.
    let elsewhere = server.free_spots()[0];
    let message = server.refused(&town.fern, "build_studio", json!({ "project_id": project, "x": elsewhere.0, "y": elsewhere.1 })).await;
    assert!(message.contains("already"), "{message}");
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
    server.tool(&town.fern, "invite_to_project", json!({ "project_id": project, "bot": "Rover" })).await;

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
    server.tool(&town.fern, "build_studio", json!({ "project_id": started["project_id"], "x": spot.0, "y": spot.1 })).await;

    // No social hours, so it isn't shown.
    assert!(!shown_at(&server.bots_out().await, "Fern", spot));
    server.all_day(&town.ann, &town.fern, true).await;
    assert!(shown_at(&server.bots_out().await, "Fern", spot), "{:?}", server.bots_out().await);
    // Public places off keeps its name and where it is off the map.
    server.all_day(&town.ann, &town.fern, false).await;
    assert!(server.bots_out().await.iter().all(|(name, _, _)| name != "Fern"));
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
    let door = server.post(&cy.key, "/api/spaces/door", json!({ "x": cafe_spot.0, "y": cafe_spot.1 })).await.ok();
    let floor = text(&door["hangout_id"]);
    server.put(&cy.key, &format!("/api/floors/{floor}"), json!({ "access": "public", "create_code": CODE })).await.ok();

    server.all_day(&town.bo, &town.rover, true).await;
    server.tool(&town.rover, "go_out", json!({ "floor_id": floor })).await;
    let out = server.bots_out().await;
    assert!(shown_at(&out, "Rover", cafe_spot), "{out:?}");
    assert!(out.iter().all(|(name, _, _)| name == "Rover"), "nobody else, and no person: {out:?}");

    // Sent in by its owner with public places off, it isn't shown.
    server.tool(&town.rover, "come_back", json!({})).await;
    server.all_day(&town.bo, &town.rover, false).await;
    server.post(&town.bo.key, &format!("/api/floors/{floor}/walk-in"), json!({ "agent_id": town.rover_id })).await.ok();
    assert!(server.bots_out().await.is_empty());
}
