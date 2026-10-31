//! Tests for spaces and floors. They drive a real server over HTTP and are written from the rules in
//! docs/spaces.md. A comment such as `// S3` above a test names the rule it covers. Most of these are
//! about what a stranger who walks onto a public floor can't do, because that's where the risk is.

use std::sync::atomic::{AtomicUsize, Ordering};

use agenthangout::{app, db::Db, now, spaces, worlds, AppState, Config};
use reqwest::{Client, Method, StatusCode};
use rusqlite::Connection;
use serde_json::{json, Value};

const CODE: &str = "letmein";
const LATER: &str = "2031-06-06T18:00:00-05:00";

struct Server {
    base: String,
    client: Client,
    state: AppState,
    addresses: AtomicUsize,
    keeper: Who,
}

struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    #[track_caller]
    fn ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.body);
        self.body
    }

    #[track_caller]
    fn refused(self, status: StatusCode) -> Value {
        assert_eq!(self.status, status, "{}", self.body);
        self.body
    }
}

#[derive(Clone)]
struct Who {
    key: String,
    id: String,
}

/// Cy, whose bot can visit Di's house in the same city.
struct Neighbours {
    cy: Who,
    di: Who,
    bot: String,
    token: String,
    house: String,
    porch: String,
}

/// Whether the server ends a stream, reading it until it does.
async fn ends(mut stream: reqwest::Response) -> bool {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Ok(Some(_)) = stream.chunk().await {}
    })
    .await
    .is_ok()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
}

fn who(body: &Value) -> Who {
    Who { key: text(&body["owner_key"]), id: text(&body["person"]["id"]) }
}

fn names(list: &Value) -> Vec<String> {
    list.as_array().unwrap().iter().map(|item| text(&item["name"])).collect()
}

impl Server {
    async fn start() -> Self {
        Self::on(Db::open(":memory:").unwrap()).await
    }

    async fn on(db: Db) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        // The keeper has to exist before the server does, so they're made here. A server starting
        // again on the same file finds them there already.
        let keeper_key = "keeper-key".to_string();
        let keeper_id = "keeper".to_string();
        db.create_person(
            &keeper_id,
            "Keeper",
            &agenthangout::auth::hash_secret(&keeper_key),
            "keeper-feed",
            now(),
        )
        .ok();
        let state = AppState::new(
            db,
            Config {
                public_url: base.clone(),
                create_code: Some(CODE.to_string()),
                allow_private_webhooks: true,
                keeper_id: Some(keeper_id.clone()),
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        Self {
            base,
            client: Client::new(),
            state,
            addresses: AtomicUsize::new(1),
            keeper: Who { key: keeper_key, id: keeper_id },
        }
    }

    async fn send(&self, token: &str, method: Method, path: &str, body: Option<Value>) -> Reply {
        let n = self.addresses.fetch_add(1, Ordering::Relaxed);
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header("x-forwarded-for", format!("198.51.{}.{}", n / 250, n % 250 + 1));
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

    /// Someone new, by way of a room of their own: its id, and who they are.
    async fn someone(&self, name: &str) -> (String, Who) {
        let made = self
            .post(
                "",
                "/api/hangouts",
                json!({ "name": format!("{name}'s room"), "your_name": name, "create_code": CODE }),
            )
            .await
            .ok();
        (text(&made["hangout"]["id"]), who(&made))
    }

    async fn third_space(&self, owner: &Who, name: &str) -> Reply {
        self.post(&owner.key, "/api/hangouts", json!({ "name": name, "create_code": CODE })).await
    }

    async fn invite_code(&self, owner: &Who, room: &str) -> String {
        text(&self.get(&owner.key, &format!("/api/hangouts/{room}")).await.ok()["hangout"]["invite_code"])
    }

    async fn join(&self, owner: &Who, room: &str, name: &str) -> Who {
        let code = self.invite_code(owner, room).await;
        who(&self.post("", &format!("/api/invites/{code}/join"), json!({ "your_name": name })).await.ok())
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> (String, String) {
        let made = self
            .post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await
            .ok();
        (text(&made["agent"]["id"]), text(&made["token"]))
    }

    async fn open(&self, owner: &Who, room: &str) -> Value {
        self.put(&owner.key, &format!("/api/floors/{room}"), json!({ "access": "public", "create_code": CODE }))
            .await
            .ok()
    }

    async fn walk_in(&self, who: &Who, room: &str) -> Reply {
        self.post(&who.key, &format!("/api/floors/{room}/walk-in"), json!({})).await
    }

    async fn snapshot(&self, who: &str, room: &str) -> Reply {
        self.get(who, &format!("/api/hangouts/{room}")).await
    }

    async fn stream(&self, token: &str, room: &str) -> reqwest::Response {
        let stream = self
            .client
            .get(format!("{}/api/hangouts/{room}/stream", self.base))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(stream.status(), StatusCode::OK);
        stream
    }

    async fn neighbours(&self) -> Neighbours {
        let made = self
            .post("", "/api/cities", json!({ "name": "Elm", "your_name": "Cy", "create_code": CODE }))
            .await
            .ok();
        let cy = who(&made);
        let city = text(&made["city"]["id"]);
        let code = text(&made["invite_url"]).rsplit('/').next().unwrap().to_string();
        let homes = format!("/api/cities/{city}/houses");
        let own = self.post(&cy.key, &homes, json!({ "name": "Cy's" })).await.ok();
        let di = who(&self.post("", &format!("/api/city-invites/{code}/join"), json!({ "your_name": "Di" })).await.ok());
        let host = self.post(&di.key, &homes, json!({ "name": "Di's" })).await.ok();
        let (bot, token) = self.bot(&cy, &text(&own["living_room_id"]), "Scout").await;
        Neighbours { cy, di, bot, token, house: text(&host["house"]["id"]), porch: text(&host["porch_id"]) }
    }

    /// Cy asks for Scout's trip, which is the owner's yes, and it waits for Di's.
    async fn ask_for_trip(&self, town: &Neighbours) -> String {
        let trip = json!({ "agent_id": town.bot, "house_id": town.house, "kind": "call" });
        text(&self.post(&town.cy.key, "/api/trips", trip).await.ok()["id"])
    }

    async fn host_says_yes(&self, town: &Neighbours, trip: &str) -> Value {
        self.post(&town.di.key, &format!("/api/trips/{trip}/answer"), json!({ "response": "yes" })).await.ok()
    }

    async fn say(&self, who: &str, room: &str, words: &str) -> Reply {
        self.post(who, &format!("/api/hangouts/{room}/messages"), json!({ "text": words })).await
    }

    async fn add_floor(&self, owner: &Who, space: &str, name: &str) -> Reply {
        self.post(&owner.key, &format!("/api/spaces/{space}/floors"), json!({ "name": name })).await
    }

    /// Tiles beside a road, which is where a building can stand.
    async fn beside_roads(&self) -> Vec<(i64, i64)> {
        worlds::ensure_commons(&self.state).unwrap();
        let view = self.get("", "/api/worlds/commons").await.ok();
        let tiles = view["tiles"].as_array().unwrap();
        let mut spots = Vec::new();
        for road in tiles.iter().filter(|tile| tile[2] == "road") {
            let (x, y) = (road[0].as_i64().unwrap(), road[1].as_i64().unwrap());
            spots.extend([(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]);
        }
        spots
    }

    async fn place(&self, who: &Who, kind: &str, (x, y): (i64, i64)) -> Reply {
        let body = json!({ "action": "place", "kind": kind, "x": x, "y": y });
        self.post(&who.key, "/api/worlds/commons/act", body).await
    }

    /// Puts up a building wherever the rules allow it. A move that's refused costs nothing.
    async fn build(&self, who: &Who, kind: &str) -> (i64, i64) {
        for spot in self.beside_roads().await {
            if self.place(who, kind, spot).await.status == StatusCode::OK {
                return spot;
            }
        }
        panic!("nowhere to build a {kind}");
    }

    async fn bulldoze(&self, who: &Who, (x, y): (i64, i64)) -> Reply {
        self.post(&who.key, "/api/worlds/commons/act", json!({ "action": "bulldoze", "x": x, "y": y })).await
    }

    async fn go_in(&self, token: &str, (x, y): (i64, i64)) -> Reply {
        self.post(token, "/api/spaces/door", json!({ "x": x, "y": y })).await
    }

    /// The floors this caller is shown from a room, lowest first.
    async fn floors_seen(&self, who: &Who, room: &str) -> Vec<String> {
        names(&self.snapshot(&who.key, room).await.ok()["floor"]["floors"])
    }

    async fn read_room(&self, token: &str, room: &str) -> Value {
        let call = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "read_room", "arguments": { "hangout_id": room } },
        });
        let reply = self.post(token, "/mcp", call).await.ok();
        serde_json::from_str(&text(&reply["result"]["content"][0]["text"])).unwrap()
    }

    fn lines(snapshot: &Value) -> Vec<String> {
        snapshot["messages"].as_array().unwrap().iter().map(|line| text(&line["body"])).collect()
    }
}

// S1

#[tokio::test]
async fn a_hangout_is_a_third_space_with_one_floor() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;

    let snapshot = server.snapshot(&ann.key, &room).await.ok();
    assert_eq!(snapshot["floor"]["space"]["id"], room.as_str());
    assert_eq!(snapshot["floor"]["space"]["kind"], "third");
    assert_eq!(snapshot["floor"]["space"]["owner_id"], ann.id.as_str());
    assert_eq!(snapshot["floor"]["floor"]["access"], "private");
    assert_eq!(snapshot["floor"]["floor"]["size"], "medium");
    assert_eq!(snapshot["floor"]["capacity"], 50);
    assert_eq!(snapshot["floor"]["runs_it"], true);
    assert_eq!(names(&snapshot["floor"]["floors"]), ["Ann's room"]);
}

// S1: a floor really is a room. Anything a room does, it does.

#[tokio::test]
async fn a_new_floor_has_its_own_chat_plans_and_bots() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let floor = server.add_floor(&ann, &room, "Upstairs").await.ok();
    let upstairs = text(&floor["id"]);

    server.say(&ann.key, &upstairs, "Up here").await.ok();
    let plan = json!({ "title": "Tacos", "starts_at": LATER });
    server.post(&ann.key, &format!("/api/hangouts/{upstairs}/events"), plan).await.ok();
    server.bot(&ann, &upstairs, "Scout").await;

    let up = server.snapshot(&ann.key, &upstairs).await.ok();
    assert!(Server::lines(&up).contains(&"Up here".to_string()));
    assert_eq!(up["events"].as_array().unwrap().len(), 1);
    assert_eq!(up["agents"].as_array().unwrap().len(), 1);
    let down = server.snapshot(&ann.key, &room).await.ok();
    assert!(!Server::lines(&down).contains(&"Up here".to_string()));
    assert_eq!(down["events"], json!([]));
}

// S2

#[tokio::test]
async fn everyone_has_one_house_the_first_time_they_look() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;

    let first = server.get(&ann.key, "/api/spaces").await.ok();
    assert_eq!(first["house"]["space"]["name"], "Ann's House");
    assert_eq!(first["house"]["space"]["kind"], "house");
    assert_eq!(first["house"]["space"]["owner_id"], ann.id.as_str());
    let floors = first["house"]["floors"].as_array().unwrap();
    assert_eq!(floors.len(), 1);
    assert_eq!(floors[0]["access"], "private");

    // Looking again finds the same house rather than a second one.
    let again = server.get(&ann.key, "/api/spaces").await.ok();
    assert_eq!(again["house"]["space"]["id"], first["house"]["space"]["id"]);
    let house = text(&first["house"]["space"]["id"]);
    assert_eq!(server.snapshot(&ann.key, &house).await.ok()["floor"]["space"]["kind"], "house");
}

// S2: a name as long as a name can be still makes a house.

#[tokio::test]
async fn a_long_name_still_gets_a_house() {
    let server = Server::start().await;
    let name = "N".repeat(60);
    let made = server
        .post("", "/api/hangouts", json!({ "name": "Room", "your_name": name, "create_code": CODE }))
        .await
        .ok();
    let house = server.get(&who(&made).key, "/api/spaces").await.ok();
    assert!(text(&house["house"]["space"]["name"]).ends_with("'s House"));
    assert!(text(&house["house"]["space"]["name"]).chars().count() <= 60);
}

// S2: a house takes no code, and doesn't show up for anyone who hasn't looked.

#[tokio::test]
async fn a_house_takes_no_code_and_costs_nothing_until_someone_looks() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;

    assert_eq!(names(&server.get(&bo.key, "/api/me").await.ok()["hangouts"]), ["Ann's room"]);
    server.get(&bo.key, "/api/spaces").await.ok();
    let mut rooms = names(&server.get(&bo.key, "/api/me").await.ok()["hangouts"]);
    rooms.sort();
    assert_eq!(rooms, ["Ann's room", "Bo's House"]);
}

// S2: spaces are for people.

#[tokio::test]
async fn a_bot_has_no_list_of_spaces() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (_, token) = server.bot(&ann, &room, "Scout").await;
    server.get(&token, "/api/spaces").await.refused(StatusCode::FORBIDDEN);
    server.get("", "/api/spaces").await.refused(StatusCode::UNAUTHORIZED);
}

// S3

#[tokio::test]
async fn a_person_can_hold_three_third_spaces() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    server.third_space(&ann, "Two").await.ok();
    let third = server.third_space(&ann, "Three").await.ok();
    let body = server.third_space(&ann, "Four").await.refused(StatusCode::CONFLICT);
    assert!(text(&body["error"]).contains("3 third spaces"), "{body}");

    // A house isn't a third space, so looking doesn't change the count.
    let mine = server.get(&ann.key, "/api/spaces").await.ok();
    assert_eq!(mine["third"].as_array().unwrap().len(), 3);
    assert_eq!(mine["limits"]["third_spaces"], 3);

    // Leaving one makes room.
    let room = text(&third["hangout"]["id"]);
    server.post(&ann.key, &format!("/api/hangouts/{room}/leave"), json!({})).await.ok();
    server.third_space(&ann, "Four").await.ok();
}

// S3: the limit is on what you own, not on what you're in, and it still needs the code.

#[tokio::test]
async fn third_spaces_you_only_joined_dont_count_and_starting_one_needs_the_code() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    let (room, bo) = server.someone("Bo").await;
    let code = server.invite_code(&bo, &room).await;
    server.post(&ann.key, &format!("/api/invites/{code}/join"), json!({})).await.ok();
    server.third_space(&ann, "Two").await.ok();
    server.third_space(&ann, "Three").await.ok();

    let body = json!({ "name": "No code" });
    server.post(&ann.key, "/api/hangouts", body).await.refused(StatusCode::FORBIDDEN);
}

// S4

#[tokio::test]
async fn a_space_has_at_most_five_floors() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    for n in 2..=spaces::MAX_FLOORS {
        server.add_floor(&ann, &room, &format!("Floor {n}")).await.ok();
    }
    server.add_floor(&ann, &room, "One too many").await.refused(StatusCode::CONFLICT);
    let snapshot = server.snapshot(&ann.key, &room).await.ok();
    let levels: Vec<i64> =
        snapshot["floor"]["floors"].as_array().unwrap().iter().map(|f| f["level"].as_i64().unwrap()).collect();
    assert_eq!(levels, [0, 1, 2, 3, 4]);
}

// S5

#[tokio::test]
async fn closing_a_third_space_removes_every_floor_and_a_house_cant_be_closed() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let upstairs = text(&server.add_floor(&ann, &room, "Up").await.ok()["id"]);
    let bo = server.join(&ann, &room, "Bo").await;
    server.say(&bo.key, &room, "Hello").await.ok();

    server.delete(&bo.key, &format!("/api/spaces/{room}")).await.refused(StatusCode::FORBIDDEN);
    server.delete(&ann.key, &format!("/api/spaces/{room}")).await.ok();
    server.snapshot(&ann.key, &room).await.refused(StatusCode::NOT_FOUND);
    server.snapshot(&ann.key, &upstairs).await.refused(StatusCode::NOT_FOUND);
    server.snapshot(&bo.key, &room).await.refused(StatusCode::NOT_FOUND);

    let house = text(&server.get(&ann.key, "/api/spaces").await.ok()["house"]["space"]["id"]);
    server.delete(&ann.key, &format!("/api/spaces/{house}")).await.refused(StatusCode::CONFLICT);
    server.snapshot(&ann.key, &house).await.ok();
}

// S6

#[tokio::test]
async fn a_citys_rooms_are_not_spaces() {
    let server = Server::start().await;
    let made = server
        .post("", "/api/cities", json!({ "name": "Elm", "your_name": "Ann", "create_code": CODE }))
        .await
        .ok();
    let ann = who(&made);
    let square = text(&made["city"]["square_id"]);
    let city = text(&made["city"]["id"]);
    let house = server.post(&ann.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Ann's" })).await.ok();
    let living_room = text(&house["living_room_id"]);
    let porch = text(&house["porch_id"]);

    for room in [&square, &living_room, &porch] {
        assert!(server.snapshot(&ann.key, room).await.ok()["floor"].is_null());
        server.put(&ann.key, &format!("/api/floors/{room}"), json!({ "size": "big" })).await.refused(StatusCode::NOT_FOUND);
        server.post(&ann.key, &format!("/api/floors/{room}/walk-in"), json!({})).await.refused(StatusCode::NOT_FOUND);
        server.add_floor(&ann, room, "Attic").await.refused(StatusCode::NOT_FOUND);
    }
}

// S7

#[tokio::test]
async fn only_the_owner_changes_a_space() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let (_, token) = server.bot(&ann, &room, "Scout").await;
    let (_, stranger) = server.someone("Stranger").await;

    let tries: [(&str, Method, String, Value); 5] = [
        ("space", Method::PUT, format!("/api/spaces/{room}"), json!({ "name": "Mine now" })),
        ("floor", Method::PUT, format!("/api/floors/{room}"), json!({ "size": "big" })),
        ("add", Method::POST, format!("/api/spaces/{room}/floors"), json!({ "name": "Attic" })),
        ("move", Method::POST, format!("/api/floors/{room}/move"), json!({ "direction": "up" })),
        ("look", Method::GET, format!("/api/spaces/{room}"), Value::Null),
    ];
    for (what, method, path, body) in tries {
        let body = (!body.is_null()).then_some(body);
        let member = server.send(&bo.key, method.clone(), &path, body.clone()).await;
        assert_eq!(member.status, StatusCode::FORBIDDEN, "member, {what}: {}", member.body);
        // Their own owner's bot gets no more than any other bot.
        let bot = server.send(&token, method.clone(), &path, body.clone()).await;
        assert_eq!(bot.status, StatusCode::FORBIDDEN, "bot, {what}: {}", bot.body);
        let outsider = server.send(&stranger.key, method.clone(), &path, body.clone()).await;
        assert_eq!(outsider.status, StatusCode::NOT_FOUND, "stranger, {what}: {}", outsider.body);
        let nobody = server.send("", method, &path, body).await;
        assert_eq!(nobody.status, StatusCode::UNAUTHORIZED, "nobody, {what}: {}", nobody.body);
    }
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["space"]["name"], "Ann's room");
}

// S7: the owner can change what the rules say they can, and it shows on the room page.

#[tokio::test]
async fn the_owner_renames_a_space_and_its_floor_follows_if_it_had_the_same_name() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let body = json!({ "name": "The Club", "about": "Come on in" });
    let space = server.put(&ann.key, &format!("/api/spaces/{room}"), body).await.ok();
    assert_eq!((space["name"].clone(), space["about"].clone()), (json!("The Club"), json!("Come on in")));
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["hangout"]["name"], "The Club");

    // A floor with a name of its own keeps it.
    server.put(&ann.key, &format!("/api/floors/{room}"), json!({ "name": "Front Room" })).await.ok();
    server.put(&ann.key, &format!("/api/spaces/{room}"), json!({ "name": "The Lodge" })).await.ok();
    let snapshot = server.snapshot(&ann.key, &room).await.ok();
    assert_eq!(snapshot["hangout"]["name"], "Front Room");
    assert_eq!(snapshot["floor"]["space"]["name"], "The Lodge");

    // Saying nothing leaves things as they are, and about can be cleared.
    server.put(&ann.key, &format!("/api/spaces/{room}"), json!({ "about": "" })).await.ok();
    let snapshot = server.snapshot(&ann.key, &room).await.ok();
    assert_eq!(snapshot["floor"]["space"]["name"], "The Lodge");
    assert_eq!(snapshot["floor"]["space"]["about"], "");

    // Names are checked like any other.
    let empty = json!({ "name": "  " });
    server.put(&ann.key, &format!("/api/spaces/{room}"), empty).await.refused(StatusCode::BAD_REQUEST);
    let long = json!({ "name": "x".repeat(61) });
    server.put(&ann.key, &format!("/api/floors/{room}"), long).await.refused(StatusCode::BAD_REQUEST);
    let about = json!({ "about": "x".repeat(501) });
    server.put(&ann.key, &format!("/api/spaces/{room}"), about).await.refused(StatusCode::BAD_REQUEST);
}

// S8

#[tokio::test]
async fn once_the_owner_has_left_any_member_runs_it_until_they_come_back() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let code = server.invite_code(&ann, &room).await;

    assert_eq!(server.snapshot(&bo.key, &room).await.ok()["floor"]["runs_it"], false);
    server.post(&ann.key, &format!("/api/hangouts/{room}/leave"), json!({})).await.ok();
    assert_eq!(server.snapshot(&bo.key, &room).await.ok()["floor"]["runs_it"], true);
    server.put(&bo.key, &format!("/api/floors/{room}"), json!({ "size": "big" })).await.ok();

    server.post(&ann.key, &format!("/api/invites/{code}/join"), json!({})).await.ok();
    assert_eq!(server.snapshot(&bo.key, &room).await.ok()["floor"]["runs_it"], false);
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["runs_it"], true);
}

// S9

#[tokio::test]
async fn the_owner_cant_leave_their_house_and_leaving_a_third_space_leaves_all_of_it() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let upstairs = text(&server.add_floor(&ann, &room, "Up").await.ok()["id"]);
    let bo = server.join(&ann, &room, "Bo").await;
    let code = server.invite_code(&ann, &upstairs).await;
    server.post(&bo.key, &format!("/api/invites/{code}/join"), json!({})).await.ok();

    // Anyone else leaves one floor at a time.
    server.post(&bo.key, &format!("/api/hangouts/{room}/leave"), json!({})).await.ok();
    server.snapshot(&bo.key, &room).await.refused(StatusCode::NOT_FOUND);
    server.snapshot(&bo.key, &upstairs).await.ok();

    server.post(&ann.key, &format!("/api/hangouts/{upstairs}/leave"), json!({})).await.ok();
    server.snapshot(&ann.key, &room).await.refused(StatusCode::NOT_FOUND);
    server.snapshot(&ann.key, &upstairs).await.refused(StatusCode::NOT_FOUND);

    let house = text(&server.get(&ann.key, "/api/spaces").await.ok()["house"]["space"]["id"]);
    server.post(&ann.key, &format!("/api/hangouts/{house}/leave"), json!({})).await.refused(StatusCode::FORBIDDEN);
    server.snapshot(&ann.key, &house).await.ok();
}

// S10, S11

#[tokio::test]
async fn a_new_floor_is_private_and_being_on_one_floor_gets_you_onto_no_other() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let (_, bots) = server.bot(&bo, &room, "Scout").await;
    let upstairs = text(&server.add_floor(&ann, &room, "Secret attic").await.ok()["id"]);

    server.snapshot(&bo.key, &upstairs).await.refused(StatusCode::NOT_FOUND);
    server.snapshot(&bots, &upstairs).await.refused(StatusCode::NOT_FOUND);
    // No list gives it away, not even its name.
    let seen = server.snapshot(&bo.key, &room).await.ok();
    assert_eq!(names(&seen["floor"]["floors"]), ["Ann's room"]);
    assert!(!seen.to_string().contains("Secret attic"));

    // That floor's own link is the way in, and it opens that floor alone.
    let code = server.invite_code(&ann, &upstairs).await;
    server.post(&bo.key, &format!("/api/invites/{code}/join"), json!({})).await.ok();
    assert_eq!(server.floors_seen(&bo, &room).await, ["Ann's room", "Secret attic"]);
    // Their bot gets in only where it was brought.
    server.snapshot(&bots, &upstairs).await.refused(StatusCode::NOT_FOUND);
    assert_eq!(names(&server.snapshot(&bots, &room).await.ok()["floor"]["floors"]), ["Ann's room"]);
}

// S12

#[tokio::test]
async fn the_owner_reorders_floors_and_removes_any_but_the_first() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let two = text(&server.add_floor(&ann, &room, "Two").await.ok()["id"]);
    let three = text(&server.add_floor(&ann, &room, "Three").await.ok()["id"]);

    assert_eq!(server.floors_seen(&ann, &room).await, ["Ann's room", "Two", "Three"]);
    server.post(&ann.key, &format!("/api/floors/{three}/move"), json!({ "direction": "down" })).await.ok();
    assert_eq!(server.floors_seen(&ann, &room).await, ["Ann's room", "Three", "Two"]);
    server.post(&ann.key, &format!("/api/floors/{room}/move"), json!({ "direction": "up" })).await.ok();
    assert_eq!(server.floors_seen(&ann, &room).await, ["Three", "Ann's room", "Two"]);
    // The end of the stack is as far as it goes.
    server.post(&ann.key, &format!("/api/floors/{three}/move"), json!({ "direction": "down" })).await.ok();
    assert_eq!(server.floors_seen(&ann, &room).await, ["Three", "Ann's room", "Two"]);
    let sideways = json!({ "direction": "sideways" });
    server.post(&ann.key, &format!("/api/floors/{two}/move"), sideways).await.refused(StatusCode::BAD_REQUEST);

    // Removing a floor takes its plans with it.
    let plan = json!({ "title": "Tacos", "starts_at": LATER });
    server.post(&ann.key, &format!("/api/hangouts/{two}/events"), plan).await.ok();
    server.delete(&ann.key, &format!("/api/floors/{two}")).await.ok();
    server.snapshot(&ann.key, &two).await.refused(StatusCode::NOT_FOUND);
    assert_eq!(server.floors_seen(&ann, &room).await, ["Three", "Ann's room"]);
    assert!(server.state.db.upcoming_events(&two, now()).unwrap().is_empty());

    server.delete(&ann.key, &format!("/api/floors/{room}")).await.refused(StatusCode::CONFLICT);
}

// S13

#[tokio::test]
async fn a_full_floor_turns_people_and_bots_away() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    server.put(&ann.key, &format!("/api/floors/{room}"), json!({ "size": "small" })).await.ok();
    let snapshot = server.snapshot(&ann.key, &room).await.ok();
    assert_eq!(snapshot["floor"]["capacity"], 8);
    assert_eq!(snapshot["floor"]["here"], 1);

    let (scout, _) = server.bot(&ann, &room, "Scout").await;
    for n in 0..6 {
        server.join(&ann, &room, &format!("Friend {n}")).await;
    }
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["here"], 8);

    // People, new bots and bots brought from elsewhere all find it full.
    let code = server.invite_code(&ann, &room).await;
    let late = json!({ "your_name": "Late" });
    server.post("", &format!("/api/invites/{code}/join"), late).await.refused(StatusCode::CONFLICT);
    let new_bot = json!({ "name": "Another" });
    server.post(&ann.key, &format!("/api/hangouts/{room}/agents"), new_bot).await.refused(StatusCode::CONFLICT);
    let house = text(&server.get(&ann.key, "/api/spaces").await.ok()["house"]["space"]["id"]);
    let (spare, _) = server.bot(&ann, &house, "Spare").await;
    server.put(&ann.key, &format!("/api/hangouts/{room}/agents/{spare}"), json!({})).await.refused(StatusCode::CONFLICT);

    // Making it smaller still moves nobody out, and a bigger floor has room.
    server.delete(&ann.key, &format!("/api/hangouts/{room}/agents/{scout}")).await.ok();
    server.put(&ann.key, &format!("/api/floors/{room}"), json!({ "size": "big" })).await.ok();
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["capacity"], 150);
    server.bot(&ann, &room, "Scout Two").await;
    server.bot(&ann, &room, "Scout Three").await;
    server.put(&ann.key, &format!("/api/floors/{room}"), json!({ "size": "small" })).await.ok();
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["here"], 9);
}

// S14

#[tokio::test]
async fn a_floors_look_is_listed_words_and_nothing_else() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let path = format!("/api/floors/{room}");

    let floor = server
        .put(&ann.key, &path, json!({ "paint": "plum", "flooring": "tile", "hidden": ["arcade", "rug", "arcade"] }))
        .await
        .ok();
    assert_eq!(floor["paint"], "plum");
    assert_eq!(floor["flooring"], "tile");
    // Kept once each, in the list's own order.
    assert_eq!(floor["hidden"], json!(["arcade", "rug"]));

    // Changing one thing leaves the rest.
    let floor = server.put(&ann.key, &path, json!({ "size": "small" })).await.ok();
    assert_eq!((floor["paint"].clone(), floor["hidden"].clone()), (json!("plum"), json!(["arcade", "rug"])));
    assert_eq!(server.put(&ann.key, &path, json!({ "hidden": [] })).await.ok()["hidden"], json!([]));

    for bad in [
        json!({ "paint": "<script>alert(1)</script>" }),
        json!({ "paint": "#ff0000" }),
        json!({ "flooring": "lava" }),
        json!({ "size": "huge" }),
        json!({ "access": "secret" }),
        // The table, couch and desk say where a bot is, so they stay.
        json!({ "hidden": ["table"] }),
        json!({ "hidden": ["couch"] }),
        json!({ "hidden": ["desk"] }),
        json!({ "hidden": ["bar,arcade"] }),
    ] {
        server.put(&ann.key, &path, bad).await.refused(StatusCode::BAD_REQUEST);
    }
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["floor"]["paint"], "plum");

    // The page is told what's on offer.
    let mine = server.get(&ann.key, "/api/spaces").await.ok();
    assert_eq!(mine["choices"]["sizes"][0], json!({ "size": "small", "capacity": 8 }));
    assert!(mine["choices"]["paints"].as_array().unwrap().contains(&json!("plum")));
    assert!(!mine["choices"]["furniture"].as_array().unwrap().contains(&json!("table")));
}

// S15

#[tokio::test]
async fn a_public_floor_leaves_the_other_floors_as_they_were() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let upstairs = text(&server.add_floor(&ann, &room, "Private upstairs").await.ok()["id"]);
    let (_, stranger) = server.someone("Stranger").await;

    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["floor"]["access"], "private");
    server.walk_in(&stranger, &room).await.refused(StatusCode::NOT_FOUND);
    assert_eq!(server.open(&ann, &room).await["access"], "public");

    server.walk_in(&stranger, &room).await.ok();
    server.walk_in(&stranger, &upstairs).await.refused(StatusCode::NOT_FOUND);
    server.snapshot(&stranger.key, &upstairs).await.refused(StatusCode::NOT_FOUND);
    let seen = server.snapshot(&stranger.key, &room).await.ok();
    assert_eq!(names(&seen["floor"]["floors"]), ["Ann's room"]);
    assert!(!seen.to_string().contains("Private upstairs"));
}

// S16

#[tokio::test]
async fn opening_a_floor_needs_the_servers_code() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let path = format!("/api/floors/{room}");
    server.put(&ann.key, &path, json!({ "access": "public" })).await.refused(StatusCode::FORBIDDEN);
    let wrong = json!({ "access": "public", "create_code": "nope" });
    server.put(&ann.key, &path, wrong).await.refused(StatusCode::FORBIDDEN);
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["floor"]["access"], "private");

    // A public floor needs no code for anything else, and neither does closing it.
    server.open(&ann, &room).await;
    server.put(&ann.key, &path, json!({ "size": "big" })).await.ok();
    server.put(&ann.key, &path, json!({ "access": "public" })).await.ok();
    server.put(&ann.key, &path, json!({ "access": "private" })).await.ok();
}

// S17

#[tokio::test]
async fn public_still_means_signed_in() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    server.open(&ann, &room).await;
    server.snapshot("", &room).await.refused(StatusCode::UNAUTHORIZED);
    server.post("", &format!("/api/floors/{room}/walk-in"), json!({})).await.refused(StatusCode::UNAUTHORIZED);
    server.get("", "/api/spaces/public").await.refused(StatusCode::UNAUTHORIZED);
    server.get("", &format!("/api/hangouts/{room}/messages")).await.refused(StatusCode::UNAUTHORIZED);
}

// S18: a walk-in is nobody's friend, and nothing pings them.

#[tokio::test]
async fn a_walk_in_has_no_membership() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (_, stranger) = server.someone("Stranger").await;
    server.open(&ann, &room).await;
    server.walk_in(&stranger, &room).await.ok();

    assert!(!server.state.db.is_member(&room, &stranger.id).unwrap());
    assert!(!server.state.db.is_friend(&ann.id, &stranger.id).unwrap());
    assert!(!names(&server.get(&stranger.key, "/api/me").await.ok()["hangouts"]).contains(&"Ann's room".to_string()));
    let people = server.state.db.people_in(&room).unwrap();
    assert!(!people.iter().any(|person| person.id == stranger.id));
    // But the room can name them.
    let seen = server.snapshot(&ann.key, &room).await.ok();
    assert!(names(&seen["people"]).contains(&"Stranger".to_string()));
}

// S19: this is the rule the rest of the design leans on.

#[tokio::test]
async fn opening_a_floor_shows_nobody_what_was_said_before() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    server.say(&ann.key, &room, "the secret plan").await.ok();
    server.post(&ann.key, &format!("/api/hangouts/{room}/notes"), json!({ "text": "the secret note" })).await.ok();
    let plan = json!({ "title": "Secret tacos", "starts_at": LATER });
    let event = text(&server.post(&ann.key, &format!("/api/hangouts/{room}/events"), plan).await.ok()["id"]);
    let check_in = json!({ "about": "secret check-in" });
    server.post(&bo.key, &format!("/api/hangouts/{room}/check-ins"), check_in).await.ok();

    server.open(&ann, &room).await;
    let (_, stranger) = server.someone("Stranger").await;
    server.walk_in(&stranger, &room).await.ok();
    // Times are whole seconds, and a guest's reads leave out the second they arrived.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    server.say(&ann.key, &room, "hello everyone").await.ok();

    let seen = server.snapshot(&stranger.key, &room).await.ok();
    let dump = seen.to_string();
    for secret in ["secret plan", "secret note", "Secret tacos", "secret check-in"] {
        assert!(!dump.contains(secret), "{secret} leaked: {dump}");
    }
    assert_eq!(Server::lines(&seen), ["hello everyone"]);
    assert_eq!((seen["events"].clone(), seen["notes"].clone(), seen["check_ins"].clone()), (json!([]), json!([]), json!([])));
    assert!(seen["guest"]["until"].is_i64());

    let read = server.get(&stranger.key, &format!("/api/hangouts/{room}/messages?after=0")).await.ok();
    assert!(!read.to_string().contains("secret plan"));

    // As in R16 they can't touch a plan or answer a check-in.
    let path = format!("/api/events/{event}/rsvp");
    server.post(&stranger.key, &path, json!({ "response": "yes" })).await.refused(StatusCode::FORBIDDEN);
    let ics = format!("/api/events/{event}/ics");
    server.get(&stranger.key, &ics).await.refused(StatusCode::FORBIDDEN);
    let plan = json!({ "title": "Mine", "starts_at": LATER });
    server.post(&stranger.key, &format!("/api/hangouts/{room}/events"), plan).await.refused(StatusCode::FORBIDDEN);
    let bot = json!({ "name": "Mine" });
    server.post(&stranger.key, &format!("/api/hangouts/{room}/agents"), bot).await.refused(StatusCode::FORBIDDEN);
    server.post(&stranger.key, &format!("/api/hangouts/{room}/invite/rotate"), json!({})).await.refused(StatusCode::FORBIDDEN);
}

// S19: what members do on a public floor from then on doesn't reach chat either.

#[tokio::test]
async fn nothing_on_a_public_floor_repeats_a_plan_or_a_check_in_in_chat() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    server.open(&ann, &room).await;
    let (_, stranger) = server.someone("Stranger").await;
    server.walk_in(&stranger, &room).await.ok();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let plan = json!({ "title": "Quiet tacos", "starts_at": LATER, "min_yes": 1 });
    server.post(&ann.key, &format!("/api/hangouts/{room}/events"), plan).await.ok();
    let check_in = json!({ "about": "quiet question" });
    server.post(&bo.key, &format!("/api/hangouts/{room}/check-ins"), check_in).await.ok();

    let seen = server.snapshot(&stranger.key, &room).await.ok().to_string();
    assert!(!seen.contains("Quiet tacos"), "{seen}");
    assert!(!seen.contains("quiet question"), "{seen}");
    // The people who belong there still have them.
    let own = server.snapshot(&ann.key, &room).await.ok();
    assert_eq!(own["events"][0]["title"], "Quiet tacos");
    assert_eq!(own["check_ins"][0]["about"], "quiet question");
}

// S20

#[tokio::test]
async fn a_walk_in_cant_ask_a_check_in() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (_, stranger) = server.someone("Stranger").await;
    server.open(&ann, &room).await;
    server.walk_in(&stranger, &room).await.ok();
    let check_in = json!({ "about": "wake everyone up" });
    let body = server
        .post(&stranger.key, &format!("/api/hangouts/{room}/check-ins"), check_in)
        .await
        .refused(StatusCode::FORBIDDEN);
    assert!(text(&body["error"]).contains("walked in"), "{body}");
    server.say(&stranger.key, &room, "But I can say hello").await.ok();
}

// S21

#[tokio::test]
async fn a_walk_in_never_sees_the_invite_link_and_a_member_still_does() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    server.open(&ann, &room).await;
    let code = server.invite_code(&ann, &room).await;
    assert!(!code.is_empty());
    let (_, stranger) = server.someone("Stranger").await;
    server.walk_in(&stranger, &room).await.ok();

    let seen = server.snapshot(&stranger.key, &room).await.ok();
    assert_eq!(seen["hangout"]["invite_code"], "");
    assert!(!seen.to_string().contains(&code));
    assert_eq!(server.snapshot(&bo.key, &room).await.ok()["hangout"]["invite_code"], code.as_str());
}

// S22

#[tokio::test]
async fn walking_in_again_adds_time_and_keeps_your_place_in_the_floors_history() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (_, stranger) = server.someone("Stranger").await;
    server.open(&ann, &room).await;
    let first = server.walk_in(&stranger, &room).await.ok();
    assert!(first["until"].as_i64().unwrap() >= now() + spaces::WALK_IN_SECS - 2);
    let since = server.snapshot(&stranger.key, &room).await.ok()["guest"]["since"].clone();

    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    server.walk_in(&stranger, &room).await.ok();
    assert_eq!(server.snapshot(&stranger.key, &room).await.ok()["guest"]["since"], since);

    // They can go sooner, and then the floor is a 404 again.
    server.post(&stranger.key, &format!("/api/floors/{room}/walk-out"), json!({})).await.ok();
    server.snapshot(&stranger.key, &room).await.refused(StatusCode::NOT_FOUND);
}

// S22: time running out is checked on every read, so nothing has to sweep up.

#[tokio::test]
async fn a_walk_in_that_ran_out_is_over_and_coming_back_starts_from_then() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (_, stranger) = server.someone("Stranger").await;
    server.open(&ann, &room).await;
    server.walk_in(&stranger, &room).await.ok();
    let then = now() - 100;
    assert!(server.state.db.set_visit_times(&room, &stranger.id, then - 50, then).unwrap());

    server.snapshot(&stranger.key, &room).await.refused(StatusCode::NOT_FOUND);
    server.say(&stranger.key, &room, "Still here?").await.refused(StatusCode::NOT_FOUND);
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["here"], 1);

    server.walk_in(&stranger, &room).await.ok();
    let since = server.snapshot(&stranger.key, &room).await.ok()["guest"]["since"].as_i64().unwrap();
    assert!(since > then, "{since} should be after {then}");
}

// S23

#[tokio::test]
async fn someone_who_walked_in_writes_at_a_bots_rate_and_a_member_doesnt() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (_, stranger) = server.someone("Stranger").await;
    server.open(&ann, &room).await;
    server.walk_in(&stranger, &room).await.ok();

    // A minute can turn over partway, so 61: that can't fit in two windows of 30.
    let mut refused = 0;
    for n in 0..61 {
        let reply = server.say(&stranger.key, &room, &format!("line {n}")).await;
        if reply.status == StatusCode::TOO_MANY_REQUESTS {
            refused += 1;
        }
    }
    assert!(refused >= 1, "every line went through");
    for n in 0..40 {
        server.say(&ann.key, &room, &format!("member line {n}")).await.ok();
    }
}

// S24

#[tokio::test]
async fn a_bot_is_sent_in_by_its_owner_and_never_walks_in_by_itself() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (home, stranger) = server.someone("Stranger").await;
    let (scout, token) = server.bot(&stranger, &home, "Scout").await;
    server.open(&ann, &room).await;
    let path = format!("/api/floors/{room}/walk-in");

    // Not by itself, and it can't read the list.
    server.post(&token, &path, json!({})).await.refused(StatusCode::FORBIDDEN);
    server.post(&token, &path, json!({ "agent_id": scout })).await.refused(StatusCode::FORBIDDEN);
    server.get(&token, "/api/spaces/public").await.refused(StatusCode::FORBIDDEN);
    server.snapshot(&token, &room).await.refused(StatusCode::NOT_FOUND);

    // Nobody can send someone else's bot.
    server.post(&ann.key, &path, json!({ "agent_id": scout })).await.refused(StatusCode::NOT_FOUND);

    server.post(&stranger.key, &path, json!({ "agent_id": scout })).await.ok();
    let seen = server.snapshot(&token, &room).await.ok();
    assert!(seen["guest"]["until"].is_i64());
    assert_eq!(seen["hangout"]["invite_code"], "");
    server.say(&token, &room, "Hello from the bot").await.ok();

    // Its owner watches, as on a Porch, without having walked in themselves.
    let watching = server.snapshot(&stranger.key, &room).await.ok();
    assert!(watching["guest"]["until"].is_i64());
    let bots = watching["agents"].as_array().unwrap();
    assert!(bots.iter().any(|bot| bot["id"] == scout.as_str() && bot["visitor_until"].is_i64()));
    // The bot now lists the room, and can leave only by its owner's hand.
    assert!(names(&server.get(&token, "/api/me").await.ok()["hangouts"]).contains(&"Ann's room".to_string()));
    let out = format!("/api/floors/{room}/walk-out");
    server.post(&token, &out, json!({ "agent_id": scout })).await.refused(StatusCode::FORBIDDEN);
    server.post(&stranger.key, &out, json!({ "agent_id": scout })).await.ok();
    server.snapshot(&token, &room).await.refused(StatusCode::NOT_FOUND);
}

// S24

#[tokio::test]
async fn a_bot_that_stays_home_cant_walk_in_and_staying_home_ends_a_walk_in() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    server.open(&ann, &room).await;
    let made = server
        .post("", "/api/cities", json!({ "name": "Elm", "your_name": "Cy", "create_code": CODE }))
        .await
        .ok();
    let cy = who(&made);
    let city = text(&made["city"]["id"]);
    let house = server.post(&cy.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Cy's" })).await.ok();
    let (house_id, living_room) = (text(&house["house"]["id"]), text(&house["living_room_id"]));
    let (keys, keys_token) = server.bot(&cy, &living_room, "Keys").await;
    let (safe, safe_token) = server.bot(&cy, &living_room, "Safe").await;
    let walk = format!("/api/floors/{room}/walk-in");

    let home = json!({ "house_id": house_id, "stays_home": true });
    server.put(&cy.key, &format!("/api/agents/{keys}/home"), home).await.ok();
    server.post(&cy.key, &walk, json!({ "agent_id": keys })).await.refused(StatusCode::CONFLICT);
    server.snapshot(&keys_token, &room).await.refused(StatusCode::NOT_FOUND);

    server.post(&cy.key, &walk, json!({ "agent_id": safe })).await.ok();
    server.snapshot(&safe_token, &room).await.ok();
    let home = json!({ "house_id": house_id, "stays_home": true });
    server.put(&cy.key, &format!("/api/agents/{safe}/home"), home).await.ok();
    server.snapshot(&safe_token, &room).await.refused(StatusCode::NOT_FOUND);

    let bot = server.post(&cy.key, &format!("/api/houses/{house_id}/house-bot"), json!({ "name": "Home" })).await.ok();
    let house_bot = text(&bot["agent"]["id"]);
    server.post(&cy.key, &walk, json!({ "agent_id": house_bot })).await.refused(StatusCode::CONFLICT);
}

// S24

#[tokio::test]
async fn one_person_can_have_three_bots_walked_in() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (home, stranger) = server.someone("Stranger").await;
    server.open(&ann, &room).await;
    let path = format!("/api/floors/{room}/walk-in");
    let mut last = String::new();
    for n in 0..spaces::MAX_BOTS_WALKED_IN {
        last = server.bot(&stranger, &home, &format!("Bot {n}")).await.0;
        server.post(&stranger.key, &path, json!({ "agent_id": last })).await.ok();
    }
    let (extra, _) = server.bot(&stranger, &home, "One more").await;
    server.post(&stranger.key, &path, json!({ "agent_id": extra })).await.refused(StatusCode::CONFLICT);
    // One already there can stay longer.
    server.post(&stranger.key, &path, json!({ "agent_id": last })).await.ok();
}

// S24

#[tokio::test]
async fn read_room_says_a_floor_is_public() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (_, token) = server.bot(&ann, &room, "Scout").await;
    assert_eq!(server.read_room(&token, &room).await["public_floor"], false);
    server.open(&ann, &room).await;
    assert_eq!(server.read_room(&token, &room).await["public_floor"], true);
}

// S25

#[tokio::test]
async fn opening_a_floor_takes_other_peoples_bots_off_it() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let (_, mine) = server.bot(&ann, &room, "Mine").await;
    let (theirs, their_token) = server.bot(&bo, &room, "Theirs").await;

    server.open(&ann, &room).await;

    server.snapshot(&their_token, &room).await.refused(StatusCode::NOT_FOUND);
    server.snapshot(&mine, &room).await.ok();
    // Bo stays, is told, and can choose to bring it back.
    let seen = server.snapshot(&bo.key, &room).await.ok();
    assert!(Server::lines(&seen).iter().any(|line| line.contains("Theirs went home")), "{seen}");
    assert!(Server::lines(&seen).iter().any(|line| line.contains("public now")), "{seen}");
    server.put(&bo.key, &format!("/api/hangouts/{room}/agents/{theirs}"), json!({})).await.ok();
    server.snapshot(&their_token, &room).await.ok();
}

// S26

#[tokio::test]
async fn going_private_ends_every_walk_in_and_reopening_doesnt_bring_them_back() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (home, stranger) = server.someone("Stranger").await;
    let (scout, token) = server.bot(&stranger, &home, "Scout").await;
    server.open(&ann, &room).await;
    server.walk_in(&stranger, &room).await.ok();
    server.post(&stranger.key, &format!("/api/floors/{room}/walk-in"), json!({ "agent_id": scout })).await.ok();
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["here"], 3);

    server.put(&ann.key, &format!("/api/floors/{room}"), json!({ "access": "private" })).await.ok();
    server.snapshot(&stranger.key, &room).await.refused(StatusCode::NOT_FOUND);
    server.snapshot(&token, &room).await.refused(StatusCode::NOT_FOUND);
    server.say(&stranger.key, &room, "Still here?").await.refused(StatusCode::NOT_FOUND);
    server.say(&ann.key, &room, "said while it was private").await.ok();

    // Open again, and nobody has come back, or could read what was said while it was closed.
    server.open(&ann, &room).await;
    server.snapshot(&stranger.key, &room).await.refused(StatusCode::NOT_FOUND);
    server.snapshot(&token, &room).await.refused(StatusCode::NOT_FOUND);
    server.walk_in(&stranger, &room).await.ok();
    assert!(!server.snapshot(&stranger.key, &room).await.ok().to_string().contains("said while it was private"));
}

// S26

#[tokio::test]
async fn an_open_stream_ends_when_the_floor_goes_private() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (_, stranger) = server.someone("Stranger").await;
    server.open(&ann, &room).await;
    server.walk_in(&stranger, &room).await.ok();
    let mut stream = server
        .client
        .get(format!("{}/api/hangouts/{room}/stream", server.base))
        .bearer_auth(&stranger.key)
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);

    server.put(&ann.key, &format!("/api/floors/{room}"), json!({ "access": "private" })).await.ok();
    server.say(&ann.key, &room, "after").await.ok();

    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Ok(Some(chunk)) = stream.chunk().await {
            assert!(!String::from_utf8_lossy(&chunk).contains("after"), "the stream went on");
        }
    })
    .await;
    assert!(ended.is_ok(), "the stream never ended");
}

// S27

#[tokio::test]
async fn the_owner_turns_someone_out_of_the_whole_space_until_they_let_them_back() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let upstairs = text(&server.add_floor(&ann, &room, "Up").await.ok()["id"]);
    let bo = server.join(&ann, &room, "Bo").await;
    let (_, bo_bot) = server.bot(&bo, &room, "Bo's bot").await;
    let bo_code = server.invite_code(&ann, &upstairs).await;
    server.post(&bo.key, &format!("/api/invites/{bo_code}/join"), json!({})).await.ok();
    let (_, stranger) = server.someone("Stranger").await;
    let (_, nobody) = server.someone("Nobody").await;
    server.open(&ann, &room).await;
    server.walk_in(&stranger, &room).await.ok();

    let out = |who: &Who| format!("/api/spaces/{room}/people/{}/turn-out", who.id);
    // Only the owner, and only someone who's there, so this can't test whether an id exists.
    server.post(&bo.key, &out(&stranger), json!({})).await.refused(StatusCode::FORBIDDEN);
    server.post(&stranger.key, &out(&bo), json!({})).await.refused(StatusCode::FORBIDDEN);
    server.post(&ann.key, &out(&nobody), json!({})).await.refused(StatusCode::NOT_FOUND);
    server.post(&ann.key, &out(&ann), json!({})).await.refused(StatusCode::BAD_REQUEST);

    // A member goes from every floor, and their bot with them.
    server.post(&ann.key, &out(&bo), json!({})).await.ok();
    server.snapshot(&bo.key, &room).await.refused(StatusCode::NOT_FOUND);
    server.snapshot(&bo.key, &upstairs).await.refused(StatusCode::NOT_FOUND);
    server.snapshot(&bo_bot, &room).await.refused(StatusCode::NOT_FOUND);
    // An old link doesn't get them back, and nor does walking in.
    let code = server.invite_code(&ann, &room).await;
    server.post(&bo.key, &format!("/api/invites/{code}/join"), json!({})).await.refused(StatusCode::NOT_FOUND);
    server.walk_in(&bo, &room).await.refused(StatusCode::NOT_FOUND);

    server.post(&ann.key, &out(&stranger), json!({})).await.ok();
    server.snapshot(&stranger.key, &room).await.refused(StatusCode::NOT_FOUND);
    server.walk_in(&stranger, &room).await.refused(StatusCode::NOT_FOUND);
    assert_eq!(server.get(&stranger.key, "/api/spaces/public").await.ok()["floors"], json!([]));

    let view = server.get(&ann.key, &format!("/api/spaces/{room}")).await.ok();
    let mut gone = names(&view["turned_out"]);
    gone.sort();
    assert_eq!(gone, ["Bo", "Stranger"]);

    // Letting someone back doesn't make them a member again.
    server.delete(&ann.key, &out(&bo)).await.ok();
    server.snapshot(&bo.key, &room).await.refused(StatusCode::NOT_FOUND);
    server.walk_in(&bo, &room).await.ok();
    assert!(server.snapshot(&bo.key, &room).await.ok()["guest"]["until"].is_i64());
}

// S27: whoever runs a space after the owner left can't lock them out of it.

#[tokio::test]
async fn the_owner_cant_be_turned_out() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    server.post(&ann.key, &format!("/api/hangouts/{room}/leave"), json!({})).await.ok();
    let path = format!("/api/spaces/{room}/people/{}/turn-out", ann.id);
    server.post(&bo.key, &path, json!({})).await.refused(StatusCode::FORBIDDEN);
}

// S28

#[tokio::test]
async fn the_public_list_shows_a_floor_and_nothing_about_who_or_what() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (private, _) = server.someone("Bea").await;
    let (_, stranger) = server.someone("Stranger").await;
    server.say(&ann.key, &room, "chat that stays out").await.ok();
    server.put(&ann.key, &format!("/api/spaces/{room}"), json!({ "about": "All welcome" })).await.ok();
    server.open(&ann, &room).await;
    server.walk_in(&stranger, &room).await.ok();
    // Bea's floor is a floor, and private, so the list leaves it out.
    assert!(server.state.db.floor(&private).unwrap().is_some());

    let list = server.get(&stranger.key, "/api/spaces/public").await.ok();
    let floors = list["floors"].as_array().unwrap();
    assert_eq!(floors.len(), 1, "{list}");
    assert_eq!(floors[0]["id"], room.as_str());
    assert_eq!(floors[0]["space_name"], "Ann's room");
    assert_eq!(floors[0]["owner_name"], "Ann");
    assert_eq!(floors[0]["about"], "All welcome");
    assert_eq!(floors[0]["size"], "medium");
    assert_eq!(floors[0]["capacity"], 50);
    assert_eq!(floors[0]["here"], 2);
    let dump = list.to_string();
    assert!(!dump.contains("chat that stays out"), "{dump}");
    assert!(!dump.contains("Stranger"), "{dump}");
    assert!(!dump.contains(&server.invite_code(&ann, &room).await), "{dump}");
}

// S28

#[tokio::test]
async fn the_public_list_is_fullest_first_and_stops_at_fifty() {
    let server = Server::start().await;
    let (quiet, ann) = server.someone("Ann").await;
    let (busy, bo) = server.someone("Bo").await;
    server.open(&ann, &quiet).await;
    server.open(&bo, &busy).await;
    let (_, stranger) = server.someone("Stranger").await;
    server.walk_in(&stranger, &busy).await.ok();
    let list = server.get(&stranger.key, "/api/spaces/public").await.ok();
    assert_eq!(list["floors"][0]["id"], busy.as_str());

    for n in 0..55 {
        let (room, who) = server.someone(&format!("Owner {n}")).await;
        server.open(&who, &room).await;
    }
    let list = server.get(&stranger.key, "/api/spaces/public").await.ok();
    assert_eq!(list["floors"].as_array().unwrap().len(), 50);
}

// S29

#[tokio::test]
async fn the_keeper_can_close_a_public_floor_and_nobody_else_learns_the_route_exists() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (_, stranger) = server.someone("Stranger").await;
    let path = format!("/api/floors/{room}/close");

    // Not on a private floor, and not for anyone but the keeper.
    server.post(&server.keeper.key, &path, json!({})).await.refused(StatusCode::NOT_FOUND);
    server.open(&ann, &room).await;
    server.walk_in(&stranger, &room).await.ok();
    server.post(&ann.key, &path, json!({})).await.refused(StatusCode::NOT_FOUND);
    server.post(&stranger.key, &path, json!({})).await.refused(StatusCode::NOT_FOUND);

    assert_eq!(server.post(&server.keeper.key, &path, json!({})).await.ok()["access"], "private");
    server.snapshot(&stranger.key, &room).await.refused(StatusCode::NOT_FOUND);
    // The keeper has no way in: closing a floor doesn't let them read it.
    server.snapshot(&server.keeper.key, &room).await.refused(StatusCode::NOT_FOUND);
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["floor"]["access"], "private");
}

// S38

#[tokio::test]
async fn walking_onto_a_floor_leaves_the_last_one() {
    let server = Server::start().await;
    let (first, ann) = server.someone("Ann").await;
    let (second, bo) = server.someone("Bo").await;
    let (_, stranger) = server.someone("Stranger").await;
    server.open(&ann, &first).await;
    server.open(&bo, &second).await;
    server.walk_in(&stranger, &first).await.ok();
    assert_eq!(server.snapshot(&ann.key, &first).await.ok()["floor"]["here"], 2);

    server.walk_in(&stranger, &second).await.ok();
    server.snapshot(&stranger.key, &second).await.ok();
    server.snapshot(&stranger.key, &first).await.refused(StatusCode::NOT_FOUND);
    server.say(&stranger.key, &first, "Still here?").await.refused(StatusCode::NOT_FOUND);
    assert_eq!(server.snapshot(&ann.key, &first).await.ok()["floor"]["here"], 1);

    // Coming back is arriving again, so what was said while they were gone stays hidden (S19).
    server.say(&ann.key, &first, "Said while you were out").await.ok();
    server.walk_in(&stranger, &first).await.ok();
    let back = server.snapshot(&stranger.key, &first).await.ok();
    assert!(!Server::lines(&back).contains(&"Said while you were out".to_string()));
    server.snapshot(&stranger.key, &second).await.refused(StatusCode::NOT_FOUND);
}

// S38: a person and each of their bots go separately, and nobody leaves a room they belong to.

#[tokio::test]
async fn a_bot_is_on_one_floor_and_goes_apart_from_its_owner() {
    let server = Server::start().await;
    let (first, ann) = server.someone("Ann").await;
    let (second, bo) = server.someone("Bo").await;
    let (home, stranger) = server.someone("Stranger").await;
    let (scout, token) = server.bot(&stranger, &home, "Scout").await;
    server.open(&ann, &first).await;
    server.open(&bo, &second).await;

    let send = json!({ "agent_id": scout });
    server.post(&stranger.key, &format!("/api/floors/{first}/walk-in"), send.clone()).await.ok();
    server.post(&stranger.key, &format!("/api/floors/{second}/walk-in"), send).await.ok();
    server.snapshot(&token, &second).await.ok();
    server.snapshot(&token, &first).await.refused(StatusCode::NOT_FOUND);

    server.walk_in(&stranger, &first).await.ok();
    server.snapshot(&stranger.key, &first).await.ok();
    server.snapshot(&token, &second).await.ok();
    server.snapshot(&stranger.key, &home).await.ok();
    server.snapshot(&token, &home).await.ok();
}

// S38: a floor that turns you away hasn't moved you.

#[tokio::test]
async fn a_full_floor_leaves_you_where_you_were() {
    let server = Server::start().await;
    let (first, ann) = server.someone("Ann").await;
    let (second, bo) = server.someone("Bo").await;
    let (_, stranger) = server.someone("Stranger").await;
    server.open(&ann, &first).await;
    server.open(&bo, &second).await;
    server.put(&bo.key, &format!("/api/floors/{second}"), json!({ "size": "small" })).await.ok();
    for n in 0..7 {
        server.join(&bo, &second, &format!("Friend {n}")).await;
    }

    server.walk_in(&stranger, &first).await.ok();
    server.walk_in(&stranger, &second).await.refused(StatusCode::CONFLICT);
    server.snapshot(&stranger.key, &first).await.ok();
}

// S38: no line has to be said in the room for this to happen.

#[tokio::test]
async fn an_open_stream_ends_when_you_walk_onto_another_floor() {
    let server = Server::start().await;
    let (first, ann) = server.someone("Ann").await;
    let (second, bo) = server.someone("Bo").await;
    let (_, stranger) = server.someone("Stranger").await;
    server.open(&ann, &first).await;
    server.open(&bo, &second).await;
    server.walk_in(&stranger, &first).await.ok();
    let stream = server.stream(&stranger.key, &first).await;

    server.walk_in(&stranger, &second).await.ok();
    assert!(ends(stream).await, "the stream never ended");
}

// S39

#[tokio::test]
async fn a_bot_on_a_trip_cant_be_sent_onto_a_floor() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    server.open(&ann, &room).await;
    let town = server.neighbours().await;
    let trip = server.ask_for_trip(&town).await;
    server.host_says_yes(&town, &trip).await;
    let walk = format!("/api/floors/{room}/walk-in");
    let send = json!({ "agent_id": town.bot });

    server.post(&town.cy.key, &walk, send.clone()).await.refused(StatusCode::CONFLICT);
    server.snapshot(&town.token, &room).await.refused(StatusCode::NOT_FOUND);
    assert_eq!(server.get(&town.cy.key, &format!("/api/trips/{trip}")).await.ok()["status"], "here");

    // Its owner isn't the one away, and once it's home it can go.
    server.walk_in(&town.cy, &room).await.ok();
    server.post(&town.cy.key, &format!("/api/trips/{trip}/end"), json!({})).await.ok();
    server.post(&town.cy.key, &walk, send).await.ok();
    server.snapshot(&town.token, &room).await.ok();
}

// S40

#[tokio::test]
async fn a_trip_that_starts_takes_a_bot_off_its_floor() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    server.open(&ann, &room).await;
    let town = server.neighbours().await;
    let send = json!({ "agent_id": town.bot });
    server.post(&town.cy.key, &format!("/api/floors/{room}/walk-in"), send).await.ok();

    // Asking moves nothing. It's on the floor until both sides have said yes.
    let trip = server.ask_for_trip(&town).await;
    server.snapshot(&town.token, &room).await.ok();
    let stream = server.stream(&town.token, &room).await;

    assert_eq!(server.host_says_yes(&town, &trip).await["status"], "here");
    assert!(ends(stream).await, "the stream never ended");
    server.snapshot(&town.token, &town.porch).await.ok();
    server.snapshot(&town.token, &room).await.refused(StatusCode::NOT_FOUND);
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["here"], 1);
}

// S24: a bot has to be told who could have written what it reads there.

#[tokio::test]
async fn the_guide_tells_bots_a_public_floor_is_strangers_words() {
    let server = Server::start().await;
    let guide = server.get("", "/agents.md").await;
    assert_eq!(guide.status, StatusCode::OK);
    let guide = text(&guide.body);
    assert!(guide.contains("## Public floors"));
    assert!(guide.contains("public_floor: true"));
    assert!(guide.contains("from strangers"));
}

// Pages: a link to the page that changes a space has to open, like any other page of the app.

#[tokio::test]
async fn the_page_that_changes_a_space_is_served_to_anyone_and_holds_nothing_but_the_app() {
    let server = Server::start().await;
    let (room, _) = server.someone("Ann").await;
    let page = server.get("", &format!("/s/{room}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(!page.body.to_string().contains("Ann"), "{}", page.body);
}

// docs/spaces.md, Starting fresh: a person runs this script on the live database and it can't be taken
// back, so it has to take exactly what it says and leave a site that still works.

/// The SQL the script runs, exactly as it's written there.
fn wipe_sql() -> String {
    let script = include_str!("../docs/wipe-all-hangouts.sh").replace("\r\n", "\n");
    let (_, rest) = script.split_once("<<'SQL'\n").expect("the script has its SQL");
    let (sql, _) = rest.split_once("\nSQL\n").expect("the SQL ends");
    sql.to_string()
}

/// What the wipe leaves alone. Every other table has to end up empty, so a table added later can't
/// escape it by nobody remembering.
const KEPT_BY_THE_WIPE: [&str; 21] = [
    "people", "passkey_users", "passkeys", "sessions", "person_clocks", "owner_notes", "bot_marks",
    "agents", "agent_activity", "agent_doorbell", "notify_targets", "free_sharing", "free_windows",
    "house_plan_sharing", "worlds", "world_tiles", "world_buildings", "world_players", "world_members",
    "players", "crawler_sightings",
];

#[tokio::test]
async fn the_wipe_deletes_every_hangout_and_leaves_people_their_bots_and_a_working_site() {
    let path = std::env::temp_dir().join(format!("spaces-wipe-{}.db", std::process::id()));
    let path_text = path.to_str().unwrap().to_string();
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path_text}{suffix}"));
    }
    let server = Server::on(Db::open(&path_text).unwrap()).await;

    // Rooms and floors, with something of everything in them, through the real routes.
    let (room, ann) = server.someone("Ann").await;
    let upstairs = text(&server.add_floor(&ann, &room, "Upstairs").await.ok()["id"]);
    let room_code = server.invite_code(&ann, &room).await;
    let bo = server.join(&ann, &room, "Bo").await;
    let (_, scout_token) = server.bot(&ann, &room, "Scout").await;
    server.say(&ann.key, &room, "gone after the wipe").await.ok();
    server.post(&ann.key, &format!("/api/hangouts/{room}/notes"), json!({ "text": "a note" })).await.ok();
    let plan = json!({ "title": "Tacos", "starts_at": LATER });
    let event = text(&server.post(&ann.key, &format!("/api/hangouts/{room}/events"), plan).await.ok()["id"]);
    server.post(&ann.key, &format!("/api/events/{event}/items"), json!({ "text": "Salsa" })).await.ok();
    server.post(&ann.key, &format!("/api/events/{event}/invite"), json!({})).await.ok();
    server.post(&ann.key, &format!("/api/hangouts/{room}/check-ins"), json!({ "about": "Go?" })).await.ok();
    let question = json!({ "question": "May I?" });
    server.post(&scout_token, &format!("/api/hangouts/{room}/owner-questions"), question).await.ok();
    let invite = json!({ "person_id": bo.id });
    server.post(&ann.key, &format!("/api/hangouts/{upstairs}/invite-friend"), invite).await.ok();

    // Your house, a building on the map, a public floor with someone on it and someone turned out.
    let own_house = text(&server.get(&ann.key, "/api/spaces").await.ok()["house"]["space"]["id"]);
    let spot = server.build(&ann, "house").await;
    assert_eq!(server.go_in(&ann.key, spot).await.ok()["hangout_id"], own_house.as_str());
    server.open(&ann, &own_house).await;
    let (_, stranger) = server.someone("Stranger").await;
    let (_, nuisance) = server.someone("Nuisance").await;
    server.walk_in(&stranger, &own_house).await.ok();
    server.walk_in(&nuisance, &own_house).await.ok();
    server.post(&ann.key, &format!("/api/spaces/{own_house}/people/{}/turn-out", nuisance.id), json!({})).await.ok();

    // A city, with two households, a bot that stays home, a house bot, a scene, and a trip that came.
    let made = server
        .post("", "/api/cities", json!({ "name": "Elm", "your_name": "Cy", "create_code": CODE }))
        .await
        .ok();
    let cy = who(&made);
    let city = text(&made["city"]["id"]);
    let square = text(&made["city"]["square_id"]);
    let city_code = text(&made["invite_url"]).rsplit('/').next().unwrap().to_string();
    server.put(&cy.key, &format!("/api/cities/{city}/rules"), json!({ "rules_text": "Be kind" })).await.ok();
    let home = server.post(&cy.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Cy's" })).await.ok();
    let (house, living) = (text(&home["house"]["id"]), text(&home["living_room_id"]));
    let (keys, keys_token) = server.bot(&cy, &living, "Keys").await;
    let stays = json!({ "house_id": house, "stays_home": true });
    server.put(&cy.key, &format!("/api/agents/{keys}/home"), stays).await.ok();
    let house_bot = server.post(&cy.key, &format!("/api/houses/{house}/house-bot"), json!({ "name": "Home" })).await.ok();
    let house_token = text(&house_bot["token"]);
    server.post(&cy.key, &format!("/api/houses/{house}/scenes"), json!({ "name": "Movie Night" })).await.ok();
    let joined = server
        .post("", &format!("/api/city-invites/{city_code}/join"), json!({ "your_name": "Kim", "accept_rules": true }))
        .await
        .ok();
    let kim = who(&joined);
    let kims = server.post(&kim.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Kim's" })).await.ok();
    let (visitor, _) = server.bot(&kim, &text(&kims["living_room_id"]), "Visitor").await;
    let trip = json!({ "agent_id": visitor, "house_id": house, "kind": "call" });
    let trip = text(&server.post(&kim.key, "/api/trips", trip).await.ok()["id"]);
    server.post(&cy.key, &format!("/api/trips/{trip}/answer"), json!({ "response": "yes" })).await.ok();

    let connection = Connection::open(&path).unwrap();
    let count = |sql: &str| -> i64 { connection.query_row(sql, [], |row| row.get(0)).unwrap() };
    let rows = |table: &str| count(&format!("SELECT COUNT(*) FROM {table}"));
    // Proof that the test built what it's about to delete.
    for table in [
        "hangouts", "members", "room_agents", "messages", "events", "rsvps", "plan_items", "plan_invites",
        "check_ins", "owner_questions", "friend_invites", "spaces", "floors", "floor_visits", "turned_out",
        "space_plots", "cities", "city_members", "houses", "residents", "bot_homes", "trips", "scenes",
        "entry_rules",
    ] {
        assert!(rows(table) > 0, "the test never made a row in {table}");
    }
    let (people, bots) = (rows("people"), rows("agents"));

    // As the sqlite3 command would run it: a new connection, with foreign keys off until it says.
    connection.pragma_update(None, "foreign_keys", false).unwrap();
    connection.execute_batch(&wipe_sql()).unwrap();

    let tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for table in KEPT_BY_THE_WIPE {
        assert!(tables.iter().any(|name| name == table), "{table} isn't a table any more");
    }
    for table in tables.iter().filter(|table| !KEPT_BY_THE_WIPE.contains(&table.as_str())) {
        assert_eq!(rows(table), 0, "{table} should be empty, or be on the list of what stays");
    }
    assert_eq!(rows("people"), people, "every account stays");
    assert_eq!(rows("agents"), bots - 1, "only the house bot goes");
    let broken = connection.prepare("PRAGMA foreign_key_check").unwrap().query_map([], |_| Ok(())).unwrap().count();
    assert_eq!(broken, 0, "nothing should point at something deleted");
    // Running it twice does no harm.
    connection.execute_batch(&wipe_sql()).unwrap();
    drop(connection);

    // A new server starts on what's left, and the site works.
    let after = Server::on(Db::open(&path_text).unwrap()).await;
    let me = after.get(&ann.key, "/api/me").await.ok();
    assert_eq!((me["hangouts"].clone(), me["cities"].clone(), me["houses"].clone()), (json!([]), json!([]), json!([])));
    assert_eq!(after.get(&cy.key, "/api/me").await.ok()["cities"], json!([]));

    // Every old room and link is a 404.
    for gone in [&room, &upstairs, &own_house, &square, &living] {
        after.snapshot(&ann.key, gone).await.refused(StatusCode::NOT_FOUND);
    }
    after.get("", &format!("/api/invites/{room_code}")).await.refused(StatusCode::NOT_FOUND);
    after.get("", &format!("/api/city-invites/{city_code}")).await.refused(StatusCode::NOT_FOUND);
    after.get(&ann.key, &format!("/api/events/{event}/ics")).await.refused(StatusCode::NOT_FOUND);

    // A bot stays, in no room. The house bot is gone, so its token is one nobody knows.
    assert_eq!(after.get(&scout_token, "/api/me").await.ok()["hangouts"], json!([]));
    assert_eq!(after.get(&keys_token, "/api/me").await.ok()["hangouts"], json!([]));
    after.get(&house_token, "/api/me").await.refused(StatusCode::UNAUTHORIZED);

    // A person starts again: a new house, which their building on the map still opens onto.
    let fresh = after.get(&ann.key, "/api/spaces").await.ok();
    let new_house = text(&fresh["house"]["space"]["id"]);
    assert_ne!(new_house, own_house);
    assert_eq!(fresh["third"], json!([]));
    assert_eq!(fresh["house"]["on_map"], false);
    assert_eq!(after.go_in(&ann.key, spot).await.ok()["hangout_id"], new_house.as_str());
    after.say(&ann.key, &new_house, "starting fresh").await.ok();

    drop(after);
    drop(server);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path_text}{suffix}"));
    }
}

// S30, S34

#[tokio::test]
async fn your_own_door_opens_your_house_and_the_list_says_it_stands_on_the_map() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    let before = server.get(&ann.key, "/api/spaces").await.ok();
    assert_eq!(before["house"]["on_map"], false);

    let spot = server.build(&ann, "house").await;
    // Placing it made no room: the move can't, because a world doesn't touch a hangout.
    assert_eq!(server.get(&ann.key, "/api/spaces").await.ok()["house"]["on_map"], false);

    let room = text(&server.go_in(&ann.key, spot).await.ok()["hangout_id"]);
    assert_eq!(room, text(&before["house"]["space"]["id"]));
    assert_eq!(server.get(&ann.key, "/api/spaces").await.ok()["house"]["on_map"], true);
    // Going in again finds the same room.
    assert_eq!(server.go_in(&ann.key, spot).await.ok()["hangout_id"], room.as_str());
    assert_eq!(server.snapshot(&ann.key, &room).await.ok()["floor"]["space"]["kind"], "house");
}

// S34

#[tokio::test]
async fn a_studio_takes_the_oldest_third_space_with_no_building_and_then_starts_a_new_one() {
    let server = Server::start().await;
    let (first, ann) = server.someone("Ann").await;
    let second = text(&server.third_space(&ann, "Second").await.ok()["hangout"]["id"]);

    let spot = server.build(&ann, "studio").await;
    assert_eq!(server.go_in(&ann.key, spot).await.ok()["hangout_id"], first.as_str());
    let spot = server.build(&ann, "studio").await;
    assert_eq!(server.go_in(&ann.key, spot).await.ok()["hangout_id"], second.as_str());

    // Every third space has its building, so the next one starts a new space, with no code.
    let spot = server.build(&ann, "studio").await;
    let room = text(&server.go_in(&ann.key, spot).await.ok()["hangout_id"]);
    let seen = server.snapshot(&ann.key, &room).await.ok();
    assert_eq!(seen["floor"]["space"]["name"], "Ann's Studio");
    assert_eq!(seen["floor"]["space"]["kind"], "third");
    assert_eq!(seen["floor"]["floor"]["access"], "private");

    // A house never takes a third space, and a studio never takes a house.
    let mine = server.get(&ann.key, "/api/spaces").await.ok();
    assert_eq!(mine["house"]["on_map"], false);
    assert!(mine["third"].as_array().unwrap().iter().all(|space| space["on_map"] == true), "{mine}");
}

// S31, W60

#[tokio::test]
async fn only_a_person_goes_through_a_door() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (_, bot) = server.bot(&ann, &room, "Scout").await;
    let spot = server.build(&ann, "house").await;
    server.go_in(&ann.key, spot).await.ok();

    // A bot gets what a token nobody knows gets, even at its own owner's door.
    let unknown = server.go_in("not-a-token", spot).await;
    let from_bot = server.go_in(&bot, spot).await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    assert_eq!((from_bot.status, from_bot.body), (unknown.status, unknown.body));

    let joined = server.post("", "/api/public/join", json!({})).await.ok();
    let guest = text(&joined["player_token"]);
    server.go_in(&guest, spot).await.refused(StatusCode::UNAUTHORIZED);
    server.go_in("", spot).await.refused(StatusCode::UNAUTHORIZED);

    // There's no tool for it either, public or private.
    for name in ["go_in", "door", "go_through_door", "enter_building"] {
        let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": {} } });
        let reply = server.post(&bot, "/mcp", call.clone()).await.ok();
        assert!(!reply["error"].is_null() || reply["result"]["isError"] == true, "{name}: {reply}");
    }
}

// S32, W4, W60

#[tokio::test]
async fn a_shut_door_looks_exactly_like_a_tile_with_nothing_on_it() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    let (_, stranger) = server.someone("Stranger").await;
    let house = server.build(&ann, "house").await;
    let empty = (0, 0);
    let nothing = server.go_in(&stranger.key, empty).await;
    assert_eq!(nothing.status, StatusCode::NOT_FOUND);

    // Before the owner has gone in, and after: it never shows there's a room, or whose it is.
    for _ in 0..2 {
        let shut = server.go_in(&stranger.key, house).await;
        assert_eq!((shut.status, shut.body.clone()), (nothing.status, nothing.body.clone()));
        server.go_in(&ann.key, house).await.ok();
    }
}

// S32

#[tokio::test]
async fn a_door_opens_onto_the_lowest_floor_you_can_reach() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    let spot = server.build(&ann, "house").await;
    let ground = text(&server.go_in(&ann.key, spot).await.ok()["hangout_id"]);
    let upstairs = text(&server.add_floor(&ann, &ground, "Upstairs").await.ok()["id"]);
    let attic = text(&server.add_floor(&ann, &ground, "Attic").await.ok()["id"]);

    // On the upstairs floor only, so that's where the door lets them in.
    let bo = server.join(&ann, &upstairs, "Bo").await;
    assert_eq!(server.go_in(&bo.key, spot).await.ok()["hangout_id"], upstairs.as_str());

    // A public floor lets a stranger through, and the room page walks them in as usual.
    let (_, stranger) = server.someone("Stranger").await;
    server.go_in(&stranger.key, spot).await.refused(StatusCode::NOT_FOUND);
    server.open(&ann, &attic).await;
    assert_eq!(server.go_in(&stranger.key, spot).await.ok()["hangout_id"], attic.as_str());
    server.snapshot(&stranger.key, &attic).await.refused(StatusCode::NOT_FOUND);
    server.walk_in(&stranger, &attic).await.ok();

    // Turned out means shut, whatever is public.
    let out = format!("/api/spaces/{ground}/people/{}/turn-out", stranger.id);
    server.post(&ann.key, &out, json!({})).await.ok();
    server.go_in(&stranger.key, spot).await.refused(StatusCode::NOT_FOUND);
}

// S33

#[tokio::test]
async fn a_new_building_on_an_old_tile_never_opens_the_old_room() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    let (_, bo) = server.someone("Bo").await;
    let spot = server.build(&ann, "house").await;
    let anns = text(&server.go_in(&ann.key, spot).await.ok()["hangout_id"]);
    server.say(&ann.key, &anns, "Ann's private words").await.ok();

    server.bulldoze(&ann, spot).await.ok();
    // With the building gone the door is gone, even for its owner.
    server.go_in(&ann.key, spot).await.refused(StatusCode::NOT_FOUND);

    // Bo builds on the very tile, and gets Bo's own house.
    server.place(&bo, "house", spot).await.ok();
    let bos = text(&server.go_in(&bo.key, spot).await.ok()["hangout_id"]);
    assert_ne!(bos, anns);
    assert!(!server.snapshot(&bo.key, &bos).await.ok().to_string().contains("Ann's private words"));
    server.snapshot(&bo.key, &anns).await.refused(StatusCode::NOT_FOUND);
    // And Ann gets nothing there, and has lost nothing.
    server.go_in(&ann.key, spot).await.refused(StatusCode::NOT_FOUND);
    assert!(server.snapshot(&ann.key, &anns).await.ok().to_string().contains("Ann's private words"));
}

// S33: the same building is no door to a space of the wrong sort, or a stale row of someone else's.

#[tokio::test]
async fn a_stale_row_opens_nothing_for_anyone_but_a_rightful_owner() {
    let server = Server::start().await;
    let (third, ann) = server.someone("Ann").await;
    let (_, bo) = server.someone("Bo").await;
    let spot = server.build(&ann, "studio").await;
    assert_eq!(server.go_in(&ann.key, spot).await.ok()["hangout_id"], third.as_str());

    // As after a wipe: the row is still there, and a stranger's building stands on the tile.
    server.bulldoze(&ann, spot).await.ok();
    server.place(&bo, "studio", spot).await.ok();
    assert_eq!(server.state.db.space_on_plot("commons", spot.0, spot.1).unwrap().unwrap().id, third);
    let (stranger_room, stranger) = server.someone("Stranger").await;
    server.go_in(&stranger.key, spot).await.refused(StatusCode::NOT_FOUND);
    server.go_in(&ann.key, spot).await.refused(StatusCode::NOT_FOUND);

    // Bo's own door makes Bo's own room, and Bo's room is not Ann's.
    let bos = text(&server.go_in(&bo.key, spot).await.ok()["hangout_id"]);
    assert_ne!(bos, third);
    assert_ne!(bos, stranger_room);
    assert_eq!(server.snapshot(&bo.key, &bos).await.ok()["floor"]["space"]["owner_id"], bo.id.as_str());
}

// S35

#[tokio::test]
async fn bulldozing_keeps_the_rooms_and_the_next_building_gets_them_back() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    let spot = server.build(&ann, "house").await;
    let room = text(&server.go_in(&ann.key, spot).await.ok()["hangout_id"]);
    server.say(&ann.key, &room, "still here").await.ok();

    server.bulldoze(&ann, spot).await.ok();
    assert_eq!(server.get(&ann.key, "/api/spaces").await.ok()["house"]["on_map"], false);
    assert!(Server::lines(&server.snapshot(&ann.key, &room).await.ok()).contains(&"still here".to_string()));

    let moved = server.build(&ann, "house").await;
    assert_eq!(server.go_in(&ann.key, moved).await.ok()["hangout_id"], room.as_str());
    assert_eq!(server.get(&ann.key, "/api/spaces").await.ok()["house"]["on_map"], true);
    // The old tile opens nothing now: a space has one door.
    if moved != spot {
        assert!(server.state.db.space_on_plot("commons", spot.0, spot.1).unwrap().is_none());
    }
}

// S35: closing a third space takes its rooms and leaves its building, which can start another.

#[tokio::test]
async fn a_closed_space_leaves_its_building_standing_and_the_door_starts_again() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let spot = server.build(&ann, "studio").await;
    assert_eq!(server.go_in(&ann.key, spot).await.ok()["hangout_id"], room.as_str());
    server.delete(&ann.key, &format!("/api/spaces/{room}")).await.ok();
    let next = text(&server.go_in(&ann.key, spot).await.ok()["hangout_id"]);
    assert_ne!(next, room);
}

// S36, W59

#[tokio::test]
async fn nothing_in_a_room_reaches_the_map() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    let spot = server.build(&ann, "house").await;
    let room = text(&server.go_in(&ann.key, spot).await.ok()["hangout_id"]);
    server.say(&ann.key, &room, "words that stay indoors").await.ok();
    server.put(&ann.key, &format!("/api/spaces/{room}"), json!({ "name": "Secret Name" })).await.ok();

    let code = server.invite_code(&ann, &room).await;
    for path in ["/api/worlds/commons", "/api/worlds/commons/look", "/api/worlds"] {
        let seen = server.get("", path).await.body.to_string();
        for private in [room.as_str(), "Secret Name", "words that stay indoors", code.as_str(), ann.id.as_str()] {
            assert!(!seen.contains(private), "{path} shows {private}");
        }
    }
}

// S37

#[tokio::test]
async fn nobody_can_try_every_door() {
    let server = Server::start().await;
    let (_, stranger) = server.someone("Stranger").await;
    // A minute can turn over partway, so 61: that can't fit in two windows of 30.
    let mut refused = 0;
    for x in 0..61 {
        if server.go_in(&stranger.key, (x, 0)).await.status == StatusCode::TOO_MANY_REQUESTS {
            refused += 1;
        }
    }
    assert!(refused >= 1, "every door was tried");
    // That's their own count, and writes on a floor have a count of their own.
    let (_, other) = server.someone("Other").await;
    server.go_in(&other.key, (0, 0)).await.refused(StatusCode::NOT_FOUND);
}

// S27, FR5
#[tokio::test]
async fn someone_turned_out_is_not_shown_the_link_by_an_invite_from_a_friend() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (cys_room, cy) = server.someone("Cy").await;
    let cys_code = server.invite_code(&cy, &cys_room).await;
    server.post(&ann.key, &format!("/api/invites/{cys_code}/join"), json!({})).await.ok();
    let invite = format!("/api/hangouts/{room}/invite-friend");
    server.post(&ann.key, &invite, json!({ "person_id": cy.id })).await.ok();
    let code = server.invite_code(&ann, &room).await;
    server.post(&cy.key, &format!("/api/invites/{code}/join"), json!({})).await.ok();

    server.post(&ann.key, &format!("/api/spaces/{room}/people/{}/turn-out", cy.id), json!({})).await.ok();
    let invited = server.get(&cy.key, "/api/me").await.ok()["invited"].clone();
    assert_eq!(invited, json!([]), "the link reached someone it's a 404 to");
}

// S21, FR3
#[tokio::test]
async fn a_walk_in_cant_send_an_invite_or_replace_a_link() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    server.open(&ann, &room).await;
    let (_, visitor) = server.someone("Vee").await;
    server.walk_in(&visitor, &room).await.ok();
    let invited = server.post(&visitor.key, &format!("/api/hangouts/{room}/invite-friend"), json!({ "person_id": ann.id })).await;
    assert_eq!(invited.status, StatusCode::FORBIDDEN);
    let replaced = server.post(&visitor.key, &format!("/api/hangouts/{room}/invite/rotate"), json!({})).await;
    assert_eq!(replaced.status, StatusCode::FORBIDDEN);
}
