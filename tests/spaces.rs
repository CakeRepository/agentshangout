//! Tests for spaces and floors. They drive a real server over HTTP and are written from the rules in
//! docs/spaces.md. A comment such as `// S3` above a test names the rule it covers. Most of these are
//! about what a stranger who walks onto a public floor can't do, because that's where the risk is.

use std::sync::atomic::{AtomicUsize, Ordering};

use agenthangout::{
    app,
    db::{Db, NewCity, NewHouse},
    now, spaces, AppState, Config,
};
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        // The keeper has to exist before the server does, so they're made here.
        let db = Db::open(":memory:").unwrap();
        let keeper_key = "keeper-key".to_string();
        let keeper_id = "keeper".to_string();
        db.create_person(
            &keeper_id,
            "Keeper",
            &agenthangout::auth::hash_secret(&keeper_key),
            "keeper-feed",
            now(),
        )
        .unwrap();
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

    async fn say(&self, who: &str, room: &str, words: &str) -> Reply {
        self.post(who, &format!("/api/hangouts/{room}/messages"), json!({ "text": words })).await
    }

    async fn add_floor(&self, owner: &Who, space: &str, name: &str) -> Reply {
        self.post(&owner.key, &format!("/api/spaces/{space}/floors"), json!({ "name": name })).await
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

// docs/spaces.md, Deleting the old hangouts: a person runs this script, once, on the live database, and
// it can't be taken back, so it has to take exactly what it says.

#[test]
fn the_wipe_takes_the_old_hangouts_and_nothing_else() {
    let path = std::env::temp_dir().join(format!("spaces-wipe-{}.db", std::process::id()));
    let path_text = path.to_str().unwrap().to_string();
    let db = Db::open(&path_text).unwrap();
    db.create_person("ann", "Ann", "hash", "feed", 100).unwrap();
    db.create_hangout("space", "A space", "code-space", "ann", 100).unwrap();
    db.create_city(&NewCity {
        id: "city",
        name: "Elm",
        invite_code: "code-city",
        square_id: "square",
        square_code: "code-square",
        created_by: "ann",
        now: 100,
    })
    .unwrap();
    db.create_house(&NewHouse {
        id: "house",
        city_id: "city",
        name: "Ann's",
        invite_code: "code-house",
        living_room_id: "living",
        living_room_code: "code-living",
        porch_id: "porch",
        porch_code: "code-porch",
        created_by: "ann",
        now: 100,
    })
    .unwrap();
    db.create_agent("bot", "ann", "Scout", "bot-hash", 100).unwrap();
    db.add_agent_to_room("space", "bot").unwrap();
    drop(db);

    // A room from before spaces, with something of everything in it. The older binary wrote these.
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "INSERT INTO hangouts (id, name, invite_code, created_by, created_at) VALUES ('old', 'Old', 'code-old', 'ann', 50);
             INSERT INTO members (hangout_id, person_id, joined_at) VALUES ('old', 'ann', 50);
             INSERT INTO room_agents (hangout_id, agent_id) VALUES ('old', 'bot');
             INSERT INTO messages (hangout_id, speaker, kind, body, created_at) VALUES ('old', 'Ann', 'chat', 'old words', 50);
             INSERT INTO messages (hangout_id, speaker, kind, body, created_at) VALUES ('space', 'Ann', 'chat', 'new words', 150);
             INSERT INTO events (id, hangout_id, title, starts_at, ends_at, status, min_yes, proposed_by, proposer, created_at, updated_at)
                VALUES ('plan', 'old', 'Tacos', 200, 300, 'confirmed', 1, 'ann', 'Ann', 50, 50);
             INSERT INTO rsvps (event_id, person_id, response, updated_at) VALUES ('plan', 'ann', 'yes', 50);
             INSERT INTO check_ins (id, hangout_id, asked_by, asker, about, needs_human, status, expires_at, created_at)
                VALUES ('check', 'old', 'ann', 'Ann', 'Go?', 0, 'waiting', 60, 50);
             INSERT INTO friend_invites (id, kind, place_id, from_person, to_person, created_at) VALUES ('to-old', 'room', 'old', 'ann', 'ann', 50);
             INSERT INTO friend_invites (id, kind, place_id, from_person, to_person, created_at) VALUES ('to-city', 'city', 'city', 'ann', 'ann', 50);",
        )
        .unwrap();
    drop(connection);

    let script = include_str!("../docs/wipe-old-hangouts.sql");
    // Run as the sqlite3 command would, on a connection that starts with foreign keys off.
    let connection = Connection::open(&path).unwrap();
    connection.pragma_update(None, "foreign_keys", false).unwrap();
    connection.execute_batch(script).unwrap();
    let count = |sql: &str| -> i64 { connection.query_row(sql, [], |row| row.get(0)).unwrap() };

    let mut rooms: Vec<String> = connection
        .prepare("SELECT id FROM hangouts")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rooms.sort();
    assert_eq!(rooms, ["living", "porch", "space", "square"]);
    for leftover in [
        "SELECT COUNT(*) FROM messages WHERE hangout_id = 'old'",
        "SELECT COUNT(*) FROM events",
        "SELECT COUNT(*) FROM rsvps",
        "SELECT COUNT(*) FROM check_ins",
        "SELECT COUNT(*) FROM members WHERE hangout_id = 'old'",
        "SELECT COUNT(*) FROM room_agents WHERE hangout_id = 'old'",
        "SELECT COUNT(*) FROM friend_invites WHERE id = 'to-old'",
    ] {
        assert_eq!(count(leftover), 0, "{leftover}");
    }
    for kept in [
        "SELECT COUNT(*) FROM people",
        "SELECT COUNT(*) FROM agents",
        "SELECT COUNT(*) FROM spaces",
        "SELECT COUNT(*) FROM cities",
        "SELECT COUNT(*) FROM houses",
        "SELECT COUNT(*) FROM friend_invites WHERE id = 'to-city'",
        "SELECT COUNT(*) FROM messages WHERE body = 'new words'",
        "SELECT COUNT(*) FROM room_agents WHERE hangout_id = 'space'",
    ] {
        assert_eq!(count(kept), 1, "{kept}");
    }

    // Running it again finds nothing more to take, and the database still opens.
    connection.execute_batch(script).unwrap();
    assert_eq!(count("SELECT COUNT(*) FROM hangouts"), 4);
    drop(connection);
    Db::open(&path_text).unwrap();
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path_text}{suffix}"));
    }
}
