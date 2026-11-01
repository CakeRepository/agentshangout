//! A27 to A45: a person deleting their own account, through the real routes, with a second
//! connection to the database to see what is really left.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use agenthangout::{
    app, auth,
    db::{Db, SOMEONE_WHO_LEFT, SOMEONE_WHO_LEFT_ID},
    now,
    world::{self, Change, Kind, Tile},
    worlds, AppState, Config,
};
use reqwest::{header, Client, Method, Response, StatusCode};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

const COMMONS: &str = "commons";
const LATER: &str = "2031-06-06T18:00:00-05:00";
const LINE: &str = "Someone deleted their account.";
/// Names no test writes by accident, so finding one anywhere means it was left behind.
const ZEPH: &str = "Zephyrine";
const QUILL: &str = "Quillfeather";

struct Server {
    base: String,
    client: Client,
    state: AppState,
    path: String,
    addresses: AtomicUsize,
}

struct Who {
    name: String,
    key: String,
    id: String,
}

struct Reply {
    status: StatusCode,
    body: Value,
    cookie: Option<String>,
}

impl Reply {
    #[track_caller]
    fn ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.body);
        self.body
    }

    #[track_caller]
    fn refused(self, status: StatusCode) {
        assert_eq!(self.status, status, "{}", self.body);
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
}

fn who(name: &str, body: &Value) -> Who {
    Who { name: name.to_string(), key: text(&body["owner_key"]), id: text(&body["person"]["id"]) }
}

fn code_of(url: &Value) -> String {
    text(url).rsplit('/').next().unwrap().to_string()
}

struct City {
    id: String,
    square: String,
    code: String,
    keeper: Who,
}

struct Home {
    id: String,
    living: String,
    porch: String,
}

impl Server {
    async fn start() -> Self {
        let path = std::env::temp_dir()
            .join(format!("agenthangout-test-{}.db", auth::new_id()))
            .to_string_lossy()
            .into_owned();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let state = AppState::new(
            Db::open(&path).unwrap(),
            Config {
                public_url: base.clone(),
                create_code: None,
                allow_private_webhooks: true,
                keeper_id: None,
                god_ids: Vec::new(),
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        worlds::ensure_commons(&state).unwrap();
        Self { base, client: Client::new(), state, path, addresses: AtomicUsize::new(1) }
    }

    async fn send(
        &self,
        token: &str,
        cookie: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let n = self.addresses.fetch_add(1, Ordering::Relaxed);
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header("x-forwarded-for", format!("198.51.{}.{}", n / 250, n % 250 + 1));
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        if !cookie.is_empty() {
            request = request.header(header::COOKIE, format!("{}={cookie}", auth::SESSION_COOKIE));
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .map(|value| value.to_str().unwrap().to_string());
        let text = response.text().await.unwrap();
        Reply { status, body: serde_json::from_str(&text).unwrap_or(Value::String(text)), cookie }
    }

    async fn get(&self, token: &str, path: &str) -> Reply {
        self.send(token, "", Method::GET, path, None).await
    }

    async fn post(&self, token: &str, path: &str, body: Value) -> Reply {
        self.send(token, "", Method::POST, path, Some(body)).await
    }

    async fn put(&self, token: &str, path: &str, body: Value) -> Reply {
        self.send(token, "", Method::PUT, path, Some(body)).await
    }

    async fn delete(&self, token: &str, path: &str) -> Reply {
        self.send(token, "", Method::DELETE, path, None).await
    }

    // The account

    /// Signs in as a browser would and returns what's in its cookie.
    async fn sign_in(&self, who: &Who) -> String {
        let reply = self.post("", "/api/session", json!({ "owner_key": who.key })).await;
        let cookie = reply.cookie.expect("no cookie");
        let cookie = cookie.split(';').next().unwrap();
        cookie.split_once('=').unwrap().1.to_string()
    }

    async fn ask_to_delete(&self, cookie: &str, name: &str) -> Reply {
        let body = Some(json!({ "name": name }));
        self.send("", cookie, Method::POST, "/api/me/delete", body).await
    }

    /// What the page does: signed in, the name typed.
    async fn leave_for_good(&self, who: &Who) {
        let cookie = self.sign_in(who).await;
        self.ask_to_delete(&cookie, &who.name).await.ok();
    }

    // Rooms

    /// A room of a new person's own, with the code that lets others in.
    async fn room(&self, room_name: &str, person: &str) -> (String, Who, String) {
        let body = json!({ "name": room_name, "your_name": person });
        let made = self.post("", "/api/hangouts", body).await.ok();
        (text(&made["hangout"]["id"]), who(person, &made), text(&made["hangout"]["invite_code"]))
    }

    async fn room_of(&self, owner: &Who, room_name: &str) -> (String, String) {
        let made = self.post(&owner.key, "/api/hangouts", json!({ "name": room_name })).await.ok();
        (text(&made["hangout"]["id"]), text(&made["hangout"]["invite_code"]))
    }

    async fn join(&self, code: &str, name: &str) -> Who {
        let body = json!({ "your_name": name });
        who(name, &self.post("", &format!("/api/invites/{code}/join"), body).await.ok())
    }

    async fn join_as(&self, person: &Who, code: &str) {
        self.post(&person.key, &format!("/api/invites/{code}/join"), json!({})).await.ok();
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> (String, String) {
        let path = format!("/api/hangouts/{room}/agents");
        let made = self.post(&owner.key, &path, json!({ "name": name })).await.ok();
        (text(&made["agent"]["id"]), text(&made["token"]))
    }

    async fn say(&self, token: &str, room: &str, words: &str) {
        let path = format!("/api/hangouts/{room}/messages");
        self.post(token, &path, json!({ "text": words })).await.ok();
    }

    async fn note(&self, token: &str, room: &str, words: &str) {
        let path = format!("/api/hangouts/{room}/notes");
        self.post(token, &path, json!({ "text": words })).await.ok();
    }

    async fn messages(&self, token: &str, room: &str) -> Vec<String> {
        let reply = self.get(token, &format!("/api/hangouts/{room}/messages")).await.ok();
        let lines = reply["messages"].as_array().unwrap_or_else(|| panic!("{reply}"));
        lines.iter().map(|line| text(&line["body"])).collect()
    }

    async fn plan(&self, token: &str, room: &str, title: &str, min_yes: i64) -> String {
        let body = json!({ "title": title, "starts_at": LATER, "min_yes": min_yes });
        let path = format!("/api/hangouts/{room}/events");
        text(&self.post(token, &path, body).await.ok()["id"])
    }

    async fn answer(&self, token: &str, plan: &str, response: &str) {
        let path = format!("/api/events/{plan}/rsvp");
        self.post(token, &path, json!({ "response": response })).await.ok();
    }

    async fn list_item(&self, token: &str, plan: &str, words: &str) -> String {
        let path = format!("/api/events/{plan}/items");
        text(&self.post(token, &path, json!({ "text": words })).await.ok()["id"])
    }

    async fn claim(&self, token: &str, item: &str) {
        let path = format!("/api/plan-items/{item}");
        self.post(token, &path, json!({ "action": "claim" })).await.ok();
    }

    async fn check_in(&self, token: &str, room: &str, about: &str, to: Option<&str>) -> String {
        let path = format!("/api/hangouts/{room}/check-ins");
        let body = json!({ "about": about, "to": to });
        text(&self.post(token, &path, body).await.ok()["id"])
    }

    async fn item(&self, token: &str, room: &str, x: i64) -> String {
        let body = json!({ "kind": "bookshelf", "x": x, "y": 3 });
        text(&self.post(token, &format!("/api/floors/{room}/items"), body).await.ok()["id"])
    }

    async fn item_note(&self, token: &str, item: &str, words: &str) {
        let path = format!("/api/items/{item}/notes");
        self.post(token, &path, json!({ "text": words })).await.ok();
    }

    async fn add_floor(&self, owner: &Who, space: &str, name: &str) -> (String, String) {
        let path = format!("/api/spaces/{space}/floors");
        let floor = text(&self.post(&owner.key, &path, json!({ "name": name })).await.ok()["id"]);
        let code = self.state.db.hangout(&floor).unwrap().unwrap().invite_code;
        (floor, code)
    }

    async fn open_floor(&self, owner: &Who, floor: &str) {
        let path = format!("/api/floors/{floor}");
        self.put(&owner.key, &path, json!({ "access": "public" })).await.ok();
    }

    fn room_exists(&self, id: &str) -> bool {
        self.state.db.hangout(id).unwrap().is_some()
    }

    // Cities

    async fn city(&self, name: &str, keeper: &str) -> City {
        let body = json!({ "name": name, "your_name": keeper });
        let made = self.post("", "/api/cities", body).await.ok();
        City {
            id: text(&made["city"]["id"]),
            square: text(&made["city"]["square_id"]),
            code: code_of(&made["invite_url"]),
            keeper: who(keeper, &made),
        }
    }

    async fn join_city(&self, code: &str, name: &str) -> Who {
        let body = json!({ "your_name": name });
        who(name, &self.post("", &format!("/api/city-invites/{code}/join"), body).await.ok())
    }

    async fn house(&self, person: &Who, city: &str, name: &str) -> Home {
        let path = format!("/api/cities/{city}/houses");
        let built = self.post(&person.key, &path, json!({ "name": name })).await.ok();
        Home {
            id: text(&built["house"]["id"]),
            living: text(&built["living_room_id"]),
            porch: text(&built["porch_id"]),
        }
    }

    fn house_code(&self, home: &Home) -> String {
        self.state.db.house(&home.id).unwrap().unwrap().invite_code
    }

    async fn move_in(&self, home: &Home, name: &str) -> Who {
        let path = format!("/api/house-invites/{}/join", self.house_code(home));
        who(name, &self.post("", &path, json!({ "your_name": name })).await.ok())
    }

    async fn ask_trip(&self, token: &str, bot: &str, home: &Home) -> String {
        let body = json!({ "agent_id": bot, "house_id": home.id, "kind": "visit" });
        text(&self.post(token, "/api/trips", body).await.ok()["id"])
    }

    async fn answer_trip(&self, token: &str, trip: &str) {
        let path = format!("/api/trips/{trip}/answer");
        self.post(token, &path, json!({ "response": "yes" })).await.ok();
    }

    // What's left

    fn db(&self) -> Connection {
        Connection::open(&self.path).unwrap()
    }

    fn count(&self, sql: &str, value: &str) -> i64 {
        self.db().query_row(sql, [value], |row| row.get(0)).unwrap()
    }

    fn one(&self, sql: &str, value: &str) -> String {
        self.db().query_row(sql, [value], |row| row.get(0)).unwrap()
    }

    /// Every text column of every table that holds any of these words.
    fn holding(&self, words: &[&str]) -> Vec<String> {
        let db = self.db();
        let mut found = Vec::new();
        let mut tables = db
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap();
        let tables: Vec<String> =
            tables.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
        for table in tables {
            let mut columns = db.prepare("SELECT name FROM pragma_table_info(?)").unwrap();
            let columns: Vec<String> = columns
                .query_map([&table], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            for column in columns {
                let sql = format!(
                    "SELECT COUNT(*) FROM \"{table}\"
                     WHERE typeof(\"{column}\") = 'text' AND instr(\"{column}\", ?) > 0"
                );
                for word in words {
                    let held: i64 = db.query_row(&sql, [word], |row| row.get(0)).unwrap();
                    if held > 0 {
                        found.push(format!("{table}.{column} holds {word}"));
                    }
                }
            }
        }
        found
    }

    fn broken_keys(&self) -> Vec<String> {
        let db = self.db();
        let mut check = db.prepare("PRAGMA foreign_key_check").unwrap();
        let rows: Vec<String> =
            check.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
        rows
    }

    // The Commons

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
        spots.retain(|&(x, y)| {
            let pos = (x as u16, y as u16);
            !tiles.contains_key(&pos)
                && !world::is_reserved(&tiles, pos)
                && land.at(pos).is_some_and(|ground| ground.is_open())
        });
        spots
    }

    async fn put_up(&self, person: &Who, kind: &str) -> (i64, i64) {
        let (x, y) = self.free_spots()[0];
        let body = json!({ "action": "place", "kind": kind, "x": x, "y": y });
        self.post(&person.key, &format!("/api/worlds/{COMMONS}/act"), body).await.ok();
        (x, y)
    }

    fn tile(&self, (x, y): (i64, i64)) -> Option<Kind> {
        self.state.db.world_tiles(COMMONS).unwrap().get(&(x as u16, y as u16)).map(|tile| tile.kind)
    }

    /// A road tile with this player's name on it, out where nothing depends on it.
    fn road_of(&self, player: u64) -> (i64, i64) {
        let tiles = self.state.db.world_tiles(COMMONS).unwrap();
        let (x, y) = self.free_spots()[3];
        let tile = Tile { kind: Kind::Road, level: 0, player: Some(player) };
        let road = [Change::Set((x as u16, y as u16), tile)];
        self.state
            .db
            .in_transaction::<(), rusqlite::Error>(|tx| tx.write_changes(COMMONS, &road, now()))
            .unwrap();
        assert!(!tiles.contains_key(&(x as u16, y as u16)));
        (x, y)
    }
}

async fn start_receiver() -> (String, Arc<Mutex<Vec<Value>>>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let store = received.clone();
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let store = store.clone();
            async move { store.lock().unwrap().push(body) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    (url, received)
}

async fn wait_for_notice(received: &Mutex<Vec<Value>>, event: &str) -> Value {
    for _ in 0..60 {
        let found =
            received.lock().unwrap().iter().find(|notice| notice["event"] == event).cloned();
        if let Some(notice) = found {
            return notice;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no notice for {event}");
}

async fn ends(mut stream: Response) -> bool {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), stream.chunk()).await {
            Err(_) => return false,
            Ok(Ok(Some(_))) => {}
            Ok(_) => return true,
        }
    }
}

// A27

#[tokio::test]
async fn a_bot_is_refused_however_it_presents_its_token() {
    let server = Server::start().await;
    let (room, zeph, _) = server.room("Crew", ZEPH).await;
    let (_, token) = server.bot(&zeph, &room, QUILL).await;

    let body = json!({ "name": ZEPH });
    server.post(&token, "/api/me/delete", body).await.refused(StatusCode::FORBIDDEN);
    server.ask_to_delete(&token, ZEPH).await.refused(StatusCode::FORBIDDEN);

    server.get(&zeph.key, "/api/me").await.ok();
    server.get(&token, "/api/me").await.ok();
}

#[tokio::test]
async fn an_owner_key_or_a_session_sent_as_a_bearer_token_is_refused() {
    let server = Server::start().await;
    let (_, zeph, _) = server.room("Crew", ZEPH).await;
    let session = server.sign_in(&zeph).await;

    let body = json!({ "name": ZEPH });
    server.post(&zeph.key, "/api/me/delete", body.clone()).await.refused(StatusCode::FORBIDDEN);
    server.post(&session, "/api/me/delete", body.clone()).await.refused(StatusCode::FORBIDDEN);
    // A browser's cookie alongside one doesn't help.
    let both = server.send(&zeph.key, &session, Method::POST, "/api/me/delete", Some(body)).await;
    both.refused(StatusCode::FORBIDDEN);

    server.get(&zeph.key, "/api/me").await.ok();
}

#[tokio::test]
async fn nobody_signed_in_gets_a_401_and_a_session_that_ended_is_nobody() {
    let server = Server::start().await;
    let (_, zeph, _) = server.room("Crew", ZEPH).await;
    server.ask_to_delete("", ZEPH).await.refused(StatusCode::UNAUTHORIZED);
    server.ask_to_delete("ah_sess_madeup", ZEPH).await.refused(StatusCode::UNAUTHORIZED);

    let session = server.sign_in(&zeph).await;
    server.send("", &session, Method::DELETE, "/api/session", None).await.ok();
    server.ask_to_delete(&session, ZEPH).await.refused(StatusCode::UNAUTHORIZED);
    server.get(&zeph.key, "/api/me").await.ok();
}

#[tokio::test]
async fn no_tool_a_bot_can_call_deletes_an_account() {
    let server = Server::start().await;
    let (room, zeph, _) = server.room("Crew", ZEPH).await;
    let (_, token) = server.bot(&zeph, &room, QUILL).await;
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} });
    for (who, path) in [(token.as_str(), "/mcp"), ("", "/public/mcp")] {
        let listed = server.post(who, path, list.clone()).await.ok();
        let tools = listed["result"]["tools"].as_array().unwrap_or_else(|| panic!("{listed}"));
        assert!(!tools.is_empty());
        for tool in tools {
            let name = text(&tool["name"]);
            assert!(!name.contains("account") && !name.contains("delete_me"), "{name}");
        }
    }
}

// A28

#[tokio::test]
async fn anything_but_the_accounts_own_name_changes_nothing() {
    let server = Server::start().await;
    let (room, zeph, _) = server.room("Crew", ZEPH).await;
    let (_, token) = server.bot(&zeph, &room, QUILL).await;
    server.say(&zeph.key, &room, "still here").await;
    let session = server.sign_in(&zeph).await;
    let before = server.messages(&zeph.key, &room).await;

    for wrong in ["", " ", "zephyrine", "ZEPHYRINE", "Zephyrin", "Zephyrine2", "yes", "Bo"] {
        server.ask_to_delete(&session, wrong).await.refused(StatusCode::BAD_REQUEST);
    }
    // A request with no name at all is no better.
    let none = server.send("", &session, Method::POST, "/api/me/delete", Some(json!({}))).await;
    assert!(none.status.is_client_error(), "{}", none.status);

    server.get(&zeph.key, "/api/me").await.ok();
    server.get(&token, "/api/me").await.ok();
    assert_eq!(server.messages(&zeph.key, &room).await, before);
}

#[tokio::test]
async fn the_page_says_what_goes_and_stays_and_asks_for_the_name() {
    let server = Server::start().await;
    let page = text(&server.get("", "/app.js").await.ok());
    for words in [
        "/api/me/delete",
        "Delete account",
        "This can't be undone",
        "Rooms other people are in stay",
        "Type your name",
    ] {
        assert!(page.contains(words), "the page should say {words}");
    }
}

// A29

#[tokio::test]
async fn every_way_in_stops_at_once_and_the_cookie_is_cleared() {
    let server = Server::start().await;
    let (room, zeph, code) = server.room("Crew", ZEPH).await;
    server.join(&code, "Ann").await;
    let (_, token) = server.bot(&zeph, &room, QUILL).await;
    let other_device = server.sign_in(&zeph).await;
    let feed = server.state.db.feed_token(&zeph.id).unwrap();
    server.get("", &format!("/cal/{feed}.ics")).await.ok();
    let passkey = agenthangout::db::NewPasskey {
        id: "key1",
        credential_id: "abcd",
        passkey: "{}",
        label: "phone",
    };
    server.state.db.add_passkey(&zeph.id, &passkey, 20, now()).unwrap();

    let session = server.sign_in(&zeph).await;
    let reply = server.ask_to_delete(&session, ZEPH).await;
    let cookie = reply.cookie.clone().expect("the cookie should be cleared");
    assert!(cookie.contains("Max-Age=0"), "{cookie}");
    assert_eq!(reply.ok()["ok"], true);

    for token in [zeph.key.as_str(), token.as_str(), other_device.as_str()] {
        server.get(token, "/api/me").await.refused(StatusCode::UNAUTHORIZED);
    }
    let cookie_reply = server.send("", &session, Method::GET, "/api/me", None).await;
    cookie_reply.refused(StatusCode::UNAUTHORIZED);
    server.get("", &format!("/cal/{feed}.ics")).await.refused(StatusCode::NOT_FOUND);
    let by_key = server.post("", "/api/session", json!({ "owner_key": zeph.key })).await;
    by_key.refused(StatusCode::UNAUTHORIZED);
    assert_eq!(server.count("SELECT COUNT(*) FROM passkeys WHERE person_id = ?", &zeph.id), 0);
    assert_eq!(server.count("SELECT COUNT(*) FROM sessions WHERE person_id = ?", &zeph.id), 0);
}

#[tokio::test]
async fn open_streams_end_whether_the_room_stays_or_goes() {
    let server = Server::start().await;
    let (stays, zeph, code) = server.room("Crew", ZEPH).await;
    server.join(&code, "Ann").await;
    let (goes, _) = server.room_of(&zeph, "Alone").await;
    let (_, token) = server.bot(&zeph, &stays, QUILL).await;

    let open = |who: String, room: String| {
        let url = format!("{}/api/hangouts/{room}/stream", server.base);
        server.client.get(url).bearer_auth(who).send()
    };
    let bot_stream = open(token, stays.clone()).await.unwrap();
    let person_stream = open(zeph.key.clone(), stays).await.unwrap();
    let alone_stream = open(zeph.key.clone(), goes).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    server.leave_for_good(&zeph).await;

    assert!(ends(bot_stream).await, "a bot's stream should end");
    assert!(ends(person_stream).await, "a person's stream should end");
    assert!(ends(alone_stream).await, "a stream in a room that went should end");
}

// A30

#[tokio::test]
async fn their_row_goes_and_one_row_stands_for_everyone_who_left() {
    let server = Server::start().await;
    let (_, ann, code) = server.room("Crew", "Ann").await;
    let zeph = server.join(&code, ZEPH).await;
    let wren = server.join(&code, "Wren").await;
    server.leave_for_good(&zeph).await;
    server.leave_for_good(&wren).await;

    assert_eq!(server.count("SELECT COUNT(*) FROM people WHERE id = ?", &zeph.id), 0);
    assert_eq!(server.count("SELECT COUNT(*) FROM people WHERE id = ?", &wren.id), 0);
    let people = "SELECT COUNT(*) FROM people WHERE name = ?";
    assert_eq!(server.count(people, SOMEONE_WHO_LEFT), 1);
    let left = "SELECT COUNT(*) FROM people WHERE id = ? AND name = 'Someone who left'";
    assert_eq!(server.count(left, SOMEONE_WHO_LEFT_ID), 1);
    assert!(server.broken_keys().is_empty(), "{:?}", server.broken_keys());

    // It's nobody's friend, and nothing belongs to it.
    let friends = server.get(&ann.key, "/api/friends").await.ok();
    assert_eq!(friends["friends"], json!([]), "{friends}");
    for table in ["members", "sessions", "passkeys", "city_members", "residents"] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE person_id = ?");
        assert_eq!(server.count(&sql, SOMEONE_WHO_LEFT_ID), 0, "{table}");
    }
    assert_eq!(
        server.count("SELECT COUNT(*) FROM agents WHERE owner_id = ?", SOMEONE_WHO_LEFT_ID),
        0
    );
}

#[tokio::test]
async fn nobody_can_sign_in_as_the_row_that_stands_for_them() {
    let server = Server::start().await;
    let (_, ann, code) = server.room("Crew", "Ann").await;
    let zeph = server.join(&code, ZEPH).await;
    server.leave_for_good(&zeph).await;

    // A credential is only ever stored as a hash, and the calendar link was never handed out.
    let key = server.one("SELECT key_hash FROM people WHERE id = ?", SOMEONE_WHO_LEFT_ID);
    assert_eq!(key.len(), 64, "{key}");
    server
        .post("", "/api/session", json!({ "owner_key": key }))
        .await
        .refused(StatusCode::UNAUTHORIZED);
    server.get(&key, "/api/me").await.refused(StatusCode::UNAUTHORIZED);
    for guess in ["", "none", "someone-who-left"] {
        server.get("", &format!("/cal/{guess}.ics")).await.refused(StatusCode::NOT_FOUND);
    }
    assert_ne!(
        server.one("SELECT feed_token FROM people WHERE id = ?", SOMEONE_WHO_LEFT_ID),
        ann.id
    );
}

// A31

#[tokio::test]
async fn what_is_only_theirs_goes() {
    let server = Server::start().await;
    let (room, ann, code) = server.room("Crew", "Ann").await;
    let zeph = server.join(&code, ZEPH).await;
    let (annie, _) = server.bot(&ann, &room, "Annie").await;
    let (url, _) = start_receiver().await;
    let z = zeph.key.as_str();
    server.put(z, "/api/notify", json!({ "url": url })).await.ok();
    let clock = json!({ "time_zone": "America/Chicago", "utc_offset_minutes": -300 });
    server.put(z, "/api/clock", clock).await.ok();
    server.put(z, "/api/sharing", json!({ "free_times": "city" })).await.ok();
    server.put(z, "/api/me/look", json!({ "hat": "crown" })).await.ok();
    server.post(z, "/api/owner-notes", json!({ "kind": "fact", "text": "Likes tea" })).await.ok();
    server.put(z, &format!("/api/marks/{annie}"), json!({ "reason": "spammed" })).await.ok();
    let about = json!({ "kind": "fact", "text": "Tea", "about_person_id": zeph.id });
    let note = server.post(&ann.key, "/api/owner-notes", about).await.ok();
    server.state.db.set_god_mode(&zeph.id, true, now()).unwrap();

    server.leave_for_good(&zeph).await;

    for (table, column) in [
        ("notify_targets", "owner_id"),
        ("person_clocks", "person_id"),
        ("free_sharing", "person_id"),
        ("person_looks", "person_id"),
        ("owner_notes", "owner_id"),
        ("bot_marks", "person_id"),
        ("god_mode", "person_id"),
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?");
        assert_eq!(server.count(&sql, &zeph.id), 0, "{table}");
    }
    let about = "SELECT COUNT(*) FROM owner_notes WHERE id = ?";
    assert_eq!(server.count(about, &text(&note["id"])), 0, "a note about them goes too");
    // And Ann's own bots still read what she kept about anyone else.
    assert_eq!(server.get(&ann.key, "/api/owner-notes").await.ok()["notes"], json!([]));
}

#[tokio::test]
async fn vouches_gates_and_invites_go_both_ways() {
    let server = Server::start().await;
    let city = server.city("Harbour", "Ann").await;
    let zeph = server.join_city(&city.code, ZEPH).await;
    let bo = server.join_city(&city.code, "Bo").await;
    let vouch = |person: &str| format!("/api/cities/{}/people/{person}/vouch", city.id);
    server.post(&zeph.key, &vouch(&bo.id), json!({})).await.ok();
    server.post(&bo.key, &vouch(&zeph.id), json!({})).await.ok();
    let rules = json!({ "min_vouches": 3, "rules_text": "" });
    server.put(&city.keeper.key, &format!("/api/cities/{}/rules", city.id), rules).await.ok();
    let other = server.city("Bayside", "Cy").await;
    let body = json!({ "your_name": "Dee" });
    server.post(&zeph.key, &format!("/api/city-invites/{}/join", other.code), body).await.ok();
    server
        .post(
            &zeph.key,
            &format!("/api/cities/{}/invite-friend", city.id),
            json!({ "person_id": bo.id }),
        )
        .await;

    server.leave_for_good(&zeph).await;

    assert_eq!(server.count("SELECT COUNT(*) FROM vouches WHERE by_person = ?", &zeph.id), 0);
    assert_eq!(server.count("SELECT COUNT(*) FROM vouches WHERE for_person = ?", &zeph.id), 0);
    assert_eq!(server.count("SELECT COUNT(*) FROM city_gate WHERE person_id = ?", &zeph.id), 0);
    assert_eq!(
        server.count("SELECT COUNT(*) FROM friend_invites WHERE from_person = ?", &zeph.id),
        0
    );
    assert!(server.broken_keys().is_empty(), "{:?}", server.broken_keys());
}

// A32

#[tokio::test]
async fn their_bots_go_with_everything_about_them() {
    let server = Server::start().await;
    let (room, zeph, code) = server.room("Crew", ZEPH).await;
    let ann = server.join(&code, "Ann").await;
    let (quill, token) = server.bot(&zeph, &room, QUILL).await;
    server.state.db.set_discord_link(&quill, "12345", "zeph#1", now()).unwrap();
    server.put(&ann.key, &format!("/api/marks/{quill}"), json!({ "reason": "spammed" })).await.ok();
    server
        .put(&zeph.key, &format!("/api/agents/{quill}/look"), json!({ "shell": "teal" }))
        .await
        .ok();

    server.leave_for_good(&zeph).await;

    server.get(&token, "/api/me").await.refused(StatusCode::UNAUTHORIZED);
    for (table, column) in [
        ("agents", "id"),
        ("discord_links", "agent_id"),
        ("bot_marks", "agent_id"),
        ("bot_looks", "agent_id"),
        ("room_agents", "agent_id"),
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?");
        assert_eq!(server.count(&sql, &quill), 0, "{table}");
    }
}

// A33

#[tokio::test]
async fn a_house_bot_stays_with_the_house_on_a_new_token_and_the_people_there_hear() {
    let server = Server::start().await;
    let city = server.city("Harbour", "Ann").await;
    let zeph = server.join_city(&city.code, ZEPH).await;
    let home = server.house(&zeph, &city.id, "Tidewater").await;
    let bo = server.move_in(&home, "Bo").await;
    let wren = server.move_in(&home, "Wren").await;
    let path = format!("/api/houses/{}/house-bot", home.id);
    let made = server.post(&zeph.key, &path, json!({ "name": "Hearth" })).await.ok();
    let (hearth, old_token) = (text(&made["agent"]["id"]), text(&made["token"]));
    let (url, notices) = start_receiver().await;
    server.put(&bo.key, "/api/notify", json!({ "url": url })).await.ok();

    server.leave_for_good(&zeph).await;

    server.get(&old_token, "/api/me").await.refused(StatusCode::UNAUTHORIZED);
    assert_eq!(
        server.one("SELECT owner_id FROM agents WHERE id = ?", &hearth),
        bo.id,
        "longest here"
    );
    let home_row = server.state.db.bot_home(&hearth).unwrap().unwrap();
    assert!(home_row.is_house, "it stays a house bot");
    let notice = wait_for_notice(&notices, "house.bot_maker_left").await;
    assert!(!text(&notice["text"]).contains(ZEPH), "{notice}");

    // Any resident can start it again, and nobody else can.
    let rotate = format!("/api/agents/{hearth}/rotate");
    let token = text(&server.post(&wren.key, &rotate, json!({})).await.ok()["token"]);
    server.get(&token, "/api/house/queue").await.ok();
    server.post(&city.keeper.key, &rotate, json!({})).await.refused(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_house_bot_goes_with_a_house_nobody_is_left_in() {
    let server = Server::start().await;
    let city = server.city("Harbour", "Ann").await;
    let zeph = server.join_city(&city.code, ZEPH).await;
    let home = server.house(&zeph, &city.id, "Tidewater").await;
    let path = format!("/api/houses/{}/house-bot", home.id);
    let made = server.post(&zeph.key, &path, json!({ "name": "Hearth" })).await.ok();

    server.leave_for_good(&zeph).await;

    server.get(&text(&made["token"]), "/api/me").await.refused(StatusCode::UNAUTHORIZED);
    assert_eq!(
        server.count("SELECT COUNT(*) FROM agents WHERE id = ?", &text(&made["agent"]["id"])),
        0
    );
}

// A34

#[tokio::test]
async fn a_room_nobody_else_belongs_to_goes_with_everything_in_it() {
    let server = Server::start().await;
    let (room, zeph, _) = server.room("Alone", ZEPH).await;
    server.say(&zeph.key, &room, "just me").await;
    let plan = server.plan(&zeph.key, &room, "Solo", 1).await;
    let item = server.item(&zeph.key, &room, 30).await;

    server.leave_for_good(&zeph).await;

    assert!(!server.room_exists(&room));
    for (table, column, value) in [
        ("messages", "hangout_id", room.as_str()),
        ("events", "id", plan.as_str()),
        ("floor_items", "id", item.as_str()),
        ("spaces", "id", room.as_str()),
        ("floors", "hangout_id", room.as_str()),
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?");
        assert_eq!(server.count(&sql, value), 0, "{table}");
    }
}

#[tokio::test]
async fn someone_who_only_walked_in_doesnt_keep_a_room_going() {
    let server = Server::start().await;
    let (room, zeph, _) = server.room("Café", ZEPH).await;
    server.open_floor(&zeph, &room).await;
    let (_, ann, _) = server.room("Her place", "Ann").await;
    server.post(&ann.key, &format!("/api/floors/{room}/walk-in"), json!({})).await.ok();

    server.leave_for_good(&zeph).await;

    assert!(!server.room_exists(&room));
    server.get(&ann.key, &format!("/api/hangouts/{room}")).await.refused(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_room_anyone_else_belongs_to_stays_as_if_they_had_left() {
    let server = Server::start().await;
    let (room, zeph, code) = server.room("Crew", ZEPH).await;
    let ann = server.join(&code, "Ann").await;
    server.say(&ann.key, &room, "still ours").await;

    server.leave_for_good(&zeph).await;

    let snapshot = server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
    assert_eq!(snapshot["hangout"]["name"], "Crew");
    let people = snapshot["people"].as_array().unwrap();
    assert_eq!(people.len(), 1, "{snapshot}");
    assert_eq!(people[0]["name"], "Ann");
    assert!(server.messages(&ann.key, &room).await.contains(&"still ours".to_string()));
    let owner = server.one("SELECT created_by FROM hangouts WHERE id = ?", &room);
    assert_eq!(owner, SOMEONE_WHO_LEFT_ID);
    // S8: the owner has left, so whoever is on the first floor runs it.
    server.get(&ann.key, &format!("/api/spaces/{room}")).await.ok();
}

#[tokio::test]
async fn being_the_last_one_in_a_room_someone_else_started_ends_it_too() {
    let server = Server::start().await;
    let (room, ann, code) = server.room("Crew", "Ann").await;
    let zeph = server.join(&code, ZEPH).await;
    server.post(&ann.key, &format!("/api/hangouts/{room}/leave"), json!({})).await.ok();

    server.leave_for_good(&zeph).await;

    assert!(!server.room_exists(&room));
}

#[tokio::test]
async fn a_room_they_walked_away_from_with_nobody_in_it_goes() {
    let server = Server::start().await;
    let (room, zeph, _) = server.room("Crew", ZEPH).await;
    let (other, _) = server.room_of(&zeph, "Other").await;
    server.post(&zeph.key, &format!("/api/hangouts/{room}/leave"), json!({})).await.ok();

    server.leave_for_good(&zeph).await;

    assert!(!server.room_exists(&room));
    assert!(!server.room_exists(&other));
}

// A35

#[tokio::test]
async fn a_space_goes_when_nobody_else_is_on_any_floor() {
    let server = Server::start().await;
    let (space, zeph, _) = server.room("Tower", ZEPH).await;
    let (attic, _) = server.add_floor(&zeph, &space, "Attic").await;

    server.leave_for_good(&zeph).await;

    assert!(!server.room_exists(&space));
    assert!(!server.room_exists(&attic));
    assert_eq!(server.count("SELECT COUNT(*) FROM spaces WHERE id = ?", &space), 0);
}

#[tokio::test]
async fn a_space_keeps_the_floors_people_are_on_and_loses_the_rest() {
    let server = Server::start().await;
    let (space, zeph, code) = server.room("Tower", ZEPH).await;
    let (attic, _) = server.add_floor(&zeph, &space, "Attic").await;
    let (cellar, _) = server.add_floor(&zeph, &space, "Cellar").await;
    let ann = server.join(&code, "Ann").await;

    server.leave_for_good(&zeph).await;

    assert!(server.room_exists(&space));
    assert!(!server.room_exists(&attic));
    assert!(!server.room_exists(&cellar));
    server.get(&ann.key, &format!("/api/spaces/{space}")).await.ok();
}

#[tokio::test]
async fn a_first_floor_nobody_else_is_on_is_shut_when_another_floor_stays() {
    let server = Server::start().await;
    let (space, zeph, code) = server.room("Tower", ZEPH).await;
    server.open_floor(&zeph, &space).await;
    let (attic, attic_code) = server.add_floor(&zeph, &space, "Attic").await;
    let ann = server.join(&attic_code, "Ann").await;
    let (_, bo, _) = server.room("Bo's", "Bo").await;
    server.post(&bo.key, &format!("/api/floors/{space}/walk-in"), json!({})).await.ok();
    server.say(&zeph.key, &space, "gone").await;
    server.item(&zeph.key, &space, 30).await;
    server.plan(&zeph.key, &space, "Tea", 1).await;
    // Someone who left earlier wrote here, and nobody could ever read that again.
    let wren = server.join(&code, "Wren").await;
    server.say(&wren.key, &space, "gone too").await;
    server.post(&wren.key, &format!("/api/hangouts/{space}/leave"), json!({})).await.ok();

    server.leave_for_good(&zeph).await;

    assert!(server.room_exists(&space), "a space can't lose the floor it began with");
    assert!(server.room_exists(&attic));
    server.get(&ann.key, &format!("/api/hangouts/{attic}")).await.ok();
    for table in ["messages", "events", "floor_items", "floor_visits", "members"] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE hangout_id = ?");
        assert_eq!(server.count(&sql, &space), 0, "{table}");
    }
    let access = server.one("SELECT access FROM floors WHERE hangout_id = ?", &space);
    assert_eq!(access, "private");
    let joined = server.post(&ann.key, &format!("/api/invites/{code}/join"), json!({})).await;
    joined.refused(StatusCode::NOT_FOUND);
    let walked = server.post(&ann.key, &format!("/api/floors/{space}/walk-in"), json!({})).await;
    assert_ne!(walked.status, StatusCode::OK, "{}", walked.body);
    let public = server.get(&ann.key, "/api/spaces/public").await.ok();
    assert_eq!(public["floors"], json!([]), "{public}");
}

#[tokio::test]
async fn a_project_that_lost_the_bot_that_started_it_goes_on_for_the_people_in_it() {
    let server = Server::start().await;
    let (room, zeph, code) = server.room("Crew", ZEPH).await;
    server.join(&code, "Ann").await;
    let (_, token) = server.bot(&zeph, &room, QUILL).await;
    let tool = json!({ "name": "start_project", "arguments": { "name": "Lighthouse" } });
    let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": tool });
    let started = server.post(&token, "/mcp", call).await.ok();
    let reply = text(&started["result"]["content"][0]["text"]);
    let reply: Value = serde_json::from_str(&reply).unwrap_or_else(|_| panic!("{started}"));
    let project = text(&reply["project_id"]);
    let invite = server.state.db.hangout(&project).unwrap().unwrap().invite_code;
    let bo = server.join(&invite, "Bo").await;

    server.leave_for_good(&zeph).await;

    let snapshot = server.get(&bo.key, &format!("/api/hangouts/{project}")).await.ok();
    assert_eq!(snapshot["hangout"]["name"], "Lighthouse");
    server.get(&bo.key, &format!("/api/spaces/{project}")).await.ok();
}

// A36

#[tokio::test]
async fn a_house_that_stays_becomes_an_ordinary_space_without_their_name() {
    let server = Server::start().await;
    let (_, ann, code) = server.room("Crew", "Ann").await;
    let zeph = server.join(&code, ZEPH).await;
    let wren = server.join(&code, "Wren").await;
    let mut houses = Vec::new();
    for person in [&zeph, &wren] {
        let mine = server.get(&person.key, "/api/spaces").await.ok();
        let house = text(&mine["house"]["space"]["id"]);
        let invite = server.state.db.hangout(&house).unwrap().unwrap().invite_code;
        server.join_as(&ann, &invite).await;
        houses.push(house);
    }

    // Two people's houses can stay, though a person can only have one.
    server.leave_for_good(&zeph).await;
    server.leave_for_good(&wren).await;

    for house in &houses {
        let view = server.get(&ann.key, &format!("/api/spaces/{house}")).await.ok();
        assert_eq!(view["space"]["kind"], "third");
        assert_eq!(view["space"]["name"], "Someone's House");
        assert_eq!(view["space"]["owner_id"], SOMEONE_WHO_LEFT_ID);
        let room = server.get(&ann.key, &format!("/api/hangouts/{house}")).await.ok();
        assert_eq!(room["hangout"]["name"], "Someone's House");
    }
    server.delete(&ann.key, &format!("/api/spaces/{}", houses[0])).await.ok();
    assert!(!server.room_exists(&houses[0]), "S5: the people there can close it");
}

#[tokio::test]
async fn a_name_built_from_theirs_loses_it_and_any_other_name_stays() {
    let server = Server::start().await;
    let (cafe, zeph, code) = server.room("Zephyrine's Café", ZEPH).await;
    let (club, club_code) = server.room_of(&zeph, "The Harbour Club").await;
    let ann = server.join(&code, "Ann").await;
    server.join_as(&ann, &club_code).await;

    server.leave_for_good(&zeph).await;

    let name = "SELECT name FROM spaces WHERE id = ?";
    assert_eq!(server.one(name, &cafe), "Someone's Café");
    assert_eq!(server.one("SELECT name FROM hangouts WHERE id = ?", &cafe), "Someone's Café");
    assert_eq!(server.one(name, &club), "The Harbour Club");
}

// A37

#[tokio::test]
async fn a_household_others_live_in_carries_on_and_what_they_agreed_ends() {
    let server = Server::start().await;
    let city = server.city("Harbour", "Ann").await;
    let zeph = server.join_city(&city.code, ZEPH).await;
    let home = server.house(&zeph, &city.id, "Tidewater").await;
    let bo = server.move_in(&home, "Bo").await;
    let code = server.house_code(&home);
    let (bot, _) = server.bot(&city.keeper, &city.square, "Visitor").await;
    let trip = server.ask_trip(&city.keeper.key, &bot, &home).await;
    server.answer_trip(&zeph.key, &trip).await;
    let here = server.get(&city.keeper.key, &format!("/api/trips/{trip}")).await.ok();
    assert_eq!(here["status"], "here", "{here}");

    server.leave_for_good(&zeph).await;

    let view = server.get(&bo.key, &format!("/api/houses/{}", home.id)).await.ok();
    assert_eq!(view["house"]["name"], "Tidewater");
    assert_ne!(server.house_code(&home), code, "the link changes when someone goes");
    let ended = server.get(&city.keeper.key, &format!("/api/trips/{trip}")).await.ok();
    assert_eq!(ended["status"], "ended", "what they agreed to ends with them: {ended}");
    assert_eq!(ended["ended_by"], SOMEONE_WHO_LEFT);
    assert_eq!(
        server.one("SELECT created_by FROM houses WHERE id = ?", &home.id),
        SOMEONE_WHO_LEFT_ID
    );
    let residents = server.state.db.residents(&home.id).unwrap();
    assert_eq!(residents.len(), 1);
}

#[tokio::test]
async fn a_house_with_nobody_left_goes_and_any_bot_visiting_goes_home() {
    let server = Server::start().await;
    let city = server.city("Harbour", "Ann").await;
    let zeph = server.join_city(&city.code, ZEPH).await;
    let home = server.house(&zeph, &city.id, "Tidewater").await;
    let keeper = &city.keeper;
    let (bot, token) = server.bot(keeper, &city.square, "Visitor").await;
    let trip = server.ask_trip(&keeper.key, &bot, &home).await;
    server.answer_trip(&zeph.key, &trip).await;
    let here = server.get(&keeper.key, &format!("/api/trips/{trip}")).await.ok();
    assert_eq!(here["status"], "here", "{here}");
    let rules =
        json!({ "min_member_days": 1, "min_gatherings": 0, "min_good_trips": 0, "min_vouches": 0 });
    server.put(&zeph.key, &format!("/api/houses/{}/rules", home.id), rules).await.ok();

    server.leave_for_good(&zeph).await;

    server
        .get(&keeper.key, &format!("/api/houses/{}", home.id))
        .await
        .refused(StatusCode::NOT_FOUND);
    assert!(!server.room_exists(&home.living));
    assert!(!server.room_exists(&home.porch));
    assert_eq!(server.count("SELECT COUNT(*) FROM entry_rules WHERE place_id = ?", &home.id), 0);
    assert_eq!(server.count("SELECT COUNT(*) FROM trips WHERE id = ?", &trip), 0);
    server.get(&token, "/api/me").await.ok();
    server.say(&token, &city.square, "back home").await;
    assert!(server.state.db.is_city_member(&city.id, &keeper.id).unwrap(), "the city stays");
    assert_eq!(server.count("SELECT COUNT(*) FROM city_members WHERE city_id = ?", &city.id), 1);
}

#[tokio::test]
async fn a_city_with_nobody_left_goes_with_its_square_and_rules() {
    let server = Server::start().await;
    let city = server.city("Harbour", ZEPH).await;
    let home = server.house(&city.keeper, &city.id, "Tidewater").await;
    let rules = json!({ "min_vouches": 1, "rules_text": "Be kind" });
    server.put(&city.keeper.key, &format!("/api/cities/{}/rules", city.id), rules).await.ok();
    let (_, ann, _) = server.room("Her place", "Ann").await;
    // Ann was on her way in, and the city she was waiting for is gone.
    let body = json!({ "accept_rules": true });
    server.post(&ann.key, &format!("/api/city-invites/{}/join", city.code), body).await.ok();

    server.leave_for_good(&city.keeper).await;

    assert_eq!(server.count("SELECT COUNT(*) FROM cities WHERE id = ?", &city.id), 0);
    assert_eq!(server.count("SELECT COUNT(*) FROM houses WHERE id = ?", &home.id), 0);
    assert_eq!(server.count("SELECT COUNT(*) FROM entry_rules WHERE place_id = ?", &city.id), 0);
    assert_eq!(server.count("SELECT COUNT(*) FROM city_gate WHERE city_id = ?", &city.id), 0);
    assert!(!server.room_exists(&city.square));
    assert!(!server.room_exists(&home.living));
    assert!(!server.room_exists(&home.porch));
    assert!(server.broken_keys().is_empty(), "{:?}", server.broken_keys());
}

#[tokio::test]
async fn a_city_that_stays_keeps_its_rules_and_rechecks_its_gate() {
    let server = Server::start().await;
    let city = server.city("Harbour", ZEPH).await;
    let ann = server.join_city(&city.code, "Ann").await;
    let rules = json!({ "min_vouches": 2, "rules_text": "" });
    server
        .put(&ann.key, &format!("/api/cities/{}/rules", city.id), rules)
        .await
        .refused(StatusCode::FORBIDDEN);
    let rules = json!({ "min_vouches": 2, "rules_text": "" });
    server.put(&city.keeper.key, &format!("/api/cities/{}/rules", city.id), rules).await.ok();
    let cy = server.join_city(&city.code, "Cy").await;
    let path = format!("/api/cities/{}/people/{}/vouch", city.id, cy.id);
    server.post(&ann.key, &path, json!({})).await.ok();
    assert!(!server.state.db.is_city_member(&city.id, &cy.id).unwrap(), "one of two vouches");

    server.leave_for_good(&city.keeper).await;

    // T13: a city that shrinks never needs more vouches than it has people, so Cy is in (T21).
    assert!(server.state.db.is_city_member(&city.id, &cy.id).unwrap());
    // T12: its starter has gone, so any member keeps the link and the rules.
    let rules = json!({ "min_vouches": 1, "rules_text": "" });
    server.put(&ann.key, &format!("/api/cities/{}/rules", city.id), rules).await.ok();
    server.post(&ann.key, &format!("/api/cities/{}/invite/rotate", city.id), json!({})).await.ok();
}

// A38

#[tokio::test]
async fn what_they_and_their_bots_said_goes_and_what_others_said_stays() {
    let server = Server::start().await;
    let (room, zeph, code) = server.room("Crew", ZEPH).await;
    let ann = server.join(&code, "Ann").await;
    let (quill, token) = server.bot(&zeph, &room, QUILL).await;
    server.say(&zeph.key, &room, "my own words").await;
    server.say(&token, &room, "my bot's words").await;
    server.note(&zeph.key, &room, "my note").await;
    server.say(&ann.key, &room, "Zephyrine makes good tea").await;
    let question = server.check_in(&zeph.key, &room, "May I?", None).await;
    let bots_question = server.check_in(&token, &room, "May it?", None).await;
    let shelf = server.item(&ann.key, &room, 30).await;
    server.item_note(&zeph.key, &shelf, "my item note").await;
    server.item_note(&ann.key, &shelf, "her item note").await;

    server.leave_for_good(&zeph).await;

    let lines = server.messages(&ann.key, &room).await;
    for gone in ["my own words", "my bot's words", "my note"] {
        assert!(!lines.iter().any(|line| line.contains(gone)), "{gone}: {lines:?}");
    }
    assert!(lines.contains(&"Zephyrine makes good tea".to_string()), "{lines:?}");
    assert_eq!(server.count("SELECT COUNT(*) FROM check_ins WHERE id = ?", &question), 0);
    assert_eq!(server.count("SELECT COUNT(*) FROM check_ins WHERE id = ?", &bots_question), 0);
    let notes = server.get(&ann.key, &format!("/api/items/{shelf}/notes")).await.ok();
    let notes = notes["notes"].as_array().unwrap().iter().map(|note| text(&note["text"]));
    assert_eq!(notes.collect::<Vec<_>>(), ["her item note"]);
    let bot_lines = "SELECT COUNT(*) FROM messages WHERE agent_id = ?";
    assert_eq!(server.count(bot_lines, &quill), 0);
}

#[tokio::test]
async fn rooms_they_left_earlier_lose_their_words_too() {
    let server = Server::start().await;
    let (room, ann, code) = server.room("Crew", "Ann").await;
    let zeph = server.join(&code, ZEPH).await;
    server.say(&zeph.key, &room, "before I left").await;
    server.post(&zeph.key, &format!("/api/hangouts/{room}/leave"), json!({})).await.ok();
    let lines = server.messages(&ann.key, &room).await;
    assert!(lines.iter().any(|line| line.contains(ZEPH)), "{lines:?}");

    server.leave_for_good(&zeph).await;

    let lines = server.messages(&ann.key, &room).await;
    assert!(!lines.iter().any(|line| line.contains("before I left")), "{lines:?}");
    assert!(!lines.iter().any(|line| line.contains(ZEPH)), "{lines:?}");
}

#[tokio::test]
async fn the_lines_that_name_them_go_and_lines_that_only_look_like_it_stay() {
    let server = Server::start().await;
    let (room, zeph, code) = server.room("Crew", "Al").await;
    let ann = server.join(&code, "Alice").await;
    let (_, _) = server.bot(&zeph, &room, "Alf").await;

    server.leave_for_good(&zeph).await;

    let lines = server.messages(&ann.key, &room).await;
    assert!(lines.contains(&"Alice joined the hangout.".to_string()), "{lines:?}");
    assert!(!lines.iter().any(|line| line.contains("brought in")), "{lines:?}");
}

#[tokio::test]
async fn a_name_that_is_an_ordinary_word_cant_empty_anyone_elses_room() {
    let server = Server::start().await;
    let (room, ann, code) = server.room("Crew", "Ann").await;
    server.join(&code, "Bo").await;
    let before = server.messages(&ann.key, &room).await;
    assert!(before.contains(&"Bo joined the hangout.".to_string()), "{before:?}");
    let (_, joined, _) = server.room("Mine", "joined").await;
    let (_, the, _) = server.room("Ours", "the").await;

    server.leave_for_good(&joined).await;
    server.leave_for_good(&the).await;

    assert_eq!(server.messages(&ann.key, &room).await, before);
}

// A39

#[tokio::test]
async fn a_plan_nobody_else_answered_goes_with_the_lines_that_quote_it() {
    let server = Server::start().await;
    let (room, zeph, code) = server.room("Crew", ZEPH).await;
    let ann = server.join(&code, "Ann").await;
    let plan = server.plan(&zeph.key, &room, "Secret birthday", 1).await;
    server.answer(&zeph.key, &plan, "yes").await;

    server.leave_for_good(&zeph).await;

    assert_eq!(server.count("SELECT COUNT(*) FROM events WHERE id = ?", &plan), 0);
    let lines = server.messages(&ann.key, &room).await;
    assert!(!lines.iter().any(|line| line.contains("Secret birthday")), "{lines:?}");
}

#[tokio::test]
async fn a_plan_someone_answered_stays_without_their_name() {
    let server = Server::start().await;
    let (room, zeph, code) = server.room("Crew", ZEPH).await;
    let ann = server.join(&code, "Ann").await;
    let plan = server.plan(&zeph.key, &room, "Tacos", 1).await;
    server.answer(&ann.key, &plan, "yes").await;

    server.leave_for_good(&zeph).await;

    let kept = server.state.db.event(&plan).unwrap().unwrap();
    assert_eq!((kept.title.as_str(), kept.status.as_str()), ("Tacos", "confirmed"));
    assert_eq!(kept.proposer, SOMEONE_WHO_LEFT);
    assert_eq!(kept.proposed_by, SOMEONE_WHO_LEFT_ID);
    let feed = server.state.db.feed_token(&ann.id).unwrap();
    let calendar = text(&server.get("", &format!("/cal/{feed}.ics")).await.ok());
    assert!(calendar.contains("Tacos") && !calendar.contains(ZEPH), "{calendar}");
}

#[tokio::test]
async fn a_plan_that_needed_their_yes_drops_off_the_calendar() {
    let server = Server::start().await;
    let (room, ann, code) = server.room("Crew", "Ann").await;
    let zeph = server.join(&code, ZEPH).await;
    let needs_them = server.plan(&ann.key, &room, "Two of us", 2).await;
    server.answer(&ann.key, &needs_them, "yes").await;
    server.answer(&zeph.key, &needs_them, "yes").await;
    let doesnt = server.plan(&ann.key, &room, "Just me", 1).await;
    server.answer(&ann.key, &doesnt, "yes").await;
    server.answer(&zeph.key, &doesnt, "yes").await;
    assert_eq!(server.state.db.event(&needs_them).unwrap().unwrap().status, "confirmed");

    server.leave_for_good(&zeph).await;

    let dropped = server.state.db.event(&needs_them).unwrap().unwrap();
    assert_eq!(dropped.status, "proposed");
    assert_eq!(dropped.rsvps.len(), 1);
    assert_eq!(server.state.db.event(&doesnt).unwrap().unwrap().status, "confirmed");
}

// A40

#[tokio::test]
async fn what_they_added_stays_without_their_name_and_claims_are_let_go() {
    let server = Server::start().await;
    let city = server.city("Harbour", ZEPH).await;
    let ann = server.join_city(&city.code, "Ann").await;
    let home = server.house(&ann, &city.id, "Tidewater").await;
    let code = server.house_code(&home);
    let zeph = &city.keeper;
    server.post(&zeph.key, &format!("/api/house-invites/{code}/join"), json!({})).await.ok();
    let (bot, token) = server.bot(zeph, &home.living, QUILL).await;
    let _ = bot;
    let notes = format!("/api/houses/{}/notes", home.id);
    let by_person = json!({ "topic": "Wifi", "text": "harbourlight" });
    server.put(&zeph.key, &notes, by_person).await.ok();
    let by_bot = json!({ "topic": "Bins", "text": "Tuesdays" });
    server.put(&token, &notes, by_bot).await.ok();

    let (room, room_code) = server.room_of(zeph, "Crew").await;
    server.join_as(&ann, &room_code).await;
    let shelf = server.item(&zeph.key, &room, 30).await;
    let plan = server.plan(&ann.key, &room, "Tacos", 1).await;
    server.answer(&ann.key, &plan, "yes").await;
    server.answer(&zeph.key, &plan, "yes").await;
    let added = server.list_item(&zeph.key, &plan, "Salsa").await;
    let claimed = server.list_item(&ann.key, &plan, "Chips").await;
    server.claim(&zeph.key, &claimed).await;

    server.leave_for_good(zeph).await;

    let list = server.get(&ann.key, &format!("/api/events/{plan}/items")).await.ok();
    let items = list["items"].as_array().unwrap();
    let salsa = items.iter().find(|item| item["id"] == added.as_str()).unwrap();
    assert_eq!(salsa["adder"], SOMEONE_WHO_LEFT, "{salsa}");
    let chips = items.iter().find(|item| item["id"] == claimed.as_str()).unwrap();
    assert!(chips["claimed_by"].is_null(), "{chips}");
    let left = "SELECT COUNT(*) FROM plan_items WHERE claimed_by = ?";
    assert_eq!(server.count(left, &zeph.id), 0);

    let snapshot = server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
    let items = snapshot["items"].as_array().unwrap();
    let kept = items.iter().find(|item| item["id"] == shelf.as_str()).unwrap();
    assert_eq!(kept["placed_by_name"], SOMEONE_WHO_LEFT, "{kept}");

    let notes = server.get(&ann.key, &notes).await.ok();
    let notes = notes["notes"].as_array().unwrap();
    let wifi = notes.iter().find(|note| note["topic"] == "Wifi").unwrap();
    assert_eq!((&wifi["writer"], &wifi["via_agent"]), (&json!(SOMEONE_WHO_LEFT), &Value::Null));
    let bins = notes.iter().find(|note| note["topic"] == "Bins").unwrap();
    assert_eq!(bins["via_agent"], SOMEONE_WHO_LEFT, "F47: it still counts as a bot's");
}

// A41

#[tokio::test]
async fn a_check_in_addressed_to_them_passes_to_nobody_and_lapses() {
    let server = Server::start().await;
    let (room, ann, code) = server.room("Crew", "Ann").await;
    let zeph = server.join(&code, ZEPH).await;
    let bo = server.join(&code, "Bo").await;
    let (_, token) = server.bot(&ann, &room, "Annie").await;
    let asked = server.check_in(&token, &room, "May I?", Some(ZEPH)).await;

    server.leave_for_good(&zeph).await;

    let to = server.one("SELECT to_person FROM check_ins WHERE id = ?", &asked);
    assert_eq!(to, SOMEONE_WHO_LEFT_ID);
    let tried = server
        .post(&bo.key, &format!("/api/check-ins/{asked}/answer"), json!({ "response": "go" }))
        .await;
    assert_ne!(tried.status, StatusCode::OK, "{}", tried.body);
    let status = server.one("SELECT status FROM check_ins WHERE id = ?", &asked);
    assert_eq!(status, "waiting");
}

// A42

#[tokio::test]
async fn their_name_comes_off_trips_scenes_and_answers() {
    let server = Server::start().await;
    let city = server.city("Harbour", "Ann").await;
    let zeph = server.join_city(&city.code, ZEPH).await;
    let home = server.house(&zeph, &city.id, "Tidewater").await;
    let bo = server.move_in(&home, "Bo").await;
    let keeper = &city.keeper;
    let (bot, _) = server.bot(keeper, &city.square, "Visitor").await;
    let trip = server.ask_trip(&keeper.key, &bot, &home).await;
    server.answer_trip(&zeph.key, &trip).await;
    server.post(&zeph.key, &format!("/api/trips/{trip}/end"), json!({})).await.ok();
    let scene = server
        .post(
            &zeph.key,
            &format!("/api/houses/{}/scenes", home.id),
            json!({ "name": "Movie night" }),
        )
        .await
        .ok();
    server
        .post(&zeph.key, &format!("/api/scenes/{}/request", text(&scene["id"])), json!({}))
        .await
        .ok();

    let (room, code) = server.room_of(keeper, "Crew").await;
    server.join_as(&zeph, &code).await;
    let (_, quill) = server.bot(&zeph, &room, QUILL).await;
    let asked = server.check_in(&keeper.key, &room, "Tea?", None).await;
    let path = format!("/api/check-ins/{asked}/answer");
    server.post(&quill, &path, json!({ "response": "go" })).await.ok();
    let second = server.check_in(&keeper.key, &room, "Coffee?", None).await;
    let path = format!("/api/check-ins/{second}/answer");
    let answer = json!({ "response": "go", "note": "only after the 3pm call" });
    server.post(&zeph.key, &path, answer).await.ok();

    server.leave_for_good(&zeph).await;

    assert_eq!(server.holding(&[ZEPH, QUILL]), Vec::<String>::new());
    let trip_row = server.get(&keeper.key, &format!("/api/trips/{trip}")).await.ok();
    assert_eq!(trip_row["ended_by"], SOMEONE_WHO_LEFT);
    assert_eq!(trip_row["host_ok_by"], SOMEONE_WHO_LEFT);
    let by = "SELECT answered_by FROM check_ins WHERE id = ?";
    assert_eq!(server.one(by, &asked), SOMEONE_WHO_LEFT);
    assert_eq!(server.one(by, &second), SOMEONE_WHO_LEFT);
    assert_eq!(server.one("SELECT note FROM check_ins WHERE id = ?", &second), "", "A38");
    assert_eq!(server.one("SELECT status FROM check_ins WHERE id = ?", &second), "go");
    let _ = bo;
}

// A43

#[tokio::test]
async fn their_places_come_down_and_every_player_ends() {
    let server = Server::start().await;
    let (room, zeph, _) = server.room("Crew", ZEPH).await;
    let spot = server.put_up(&zeph, "house").await;
    assert_eq!(server.tile(spot), Some(Kind::House));
    let (_, bot_token) = server.bot(&zeph, &room, QUILL).await;
    let bots_player = server.post(&bot_token, "/api/public/join", json!({})).await.ok();
    let bots_player = text(&bots_player["player_token"]);
    let guest = server.post("", "/api/public/join", json!({})).await.ok();
    let (guest_token, claim) = (text(&guest["player_token"]), text(&guest["claim_code"]));
    server.post(&zeph.key, "/api/players/claim", json!({ "claim_code": claim })).await.ok();
    let me = format!("/api/worlds/{COMMONS}/me");
    server.get(&guest_token, &me).await.ok();
    server.get(&bots_player, &me).await.ok();

    server.leave_for_good(&zeph).await;

    assert_eq!(server.tile(spot), None, "their place is gone");
    server.get(&guest_token, &me).await.refused(StatusCode::UNAUTHORIZED);
    server.get(&bots_player, &me).await.refused(StatusCode::UNAUTHORIZED);
    assert_eq!(server.count("SELECT COUNT(*) FROM players WHERE person_id = ?", &zeph.id), 0);
    assert_eq!(server.count("SELECT COUNT(*) FROM world_buildings WHERE world_id = ?", COMMONS), 0);
}

#[tokio::test]
async fn rooms_behind_a_place_outlive_it_when_people_are_in_them() {
    let server = Server::start().await;
    let (_, ann, code) = server.room("Crew", "Ann").await;
    let zeph = server.join(&code, ZEPH).await;
    let spot = server.put_up(&zeph, "house").await;
    let door =
        server.post(&zeph.key, "/api/spaces/door", json!({ "x": spot.0, "y": spot.1 })).await.ok();
    let room = text(&door["hangout_id"]);
    let invite = server.state.db.hangout(&room).unwrap().unwrap().invite_code;
    server.join_as(&ann, &invite).await;

    server.leave_for_good(&zeph).await;

    assert_eq!(server.tile(spot), None);
    server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
    let through =
        server.post(&ann.key, "/api/spaces/door", json!({ "x": spot.0, "y": spot.1 })).await;
    through.refused(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn their_roads_stay_and_anyone_can_clear_one_that_cuts_nobody_off() {
    let server = Server::start().await;
    let (room, zeph, _) = server.room("Crew", ZEPH).await;
    let (bot, bot_token) = server.bot(&zeph, &room, QUILL).await;
    server.post(&bot_token, "/api/public/join", json!({})).await.ok();
    let player = server.count("SELECT id FROM players WHERE agent_id = ?", &bot);
    let spot = server.road_of(player as u64);
    let guest = server.post("", "/api/public/join", json!({})).await.ok();
    let guest = text(&guest["player_token"]);

    server.leave_for_good(&zeph).await;

    assert_eq!(server.tile(spot), Some(Kind::Road), "nothing on a road says whose it was");
    let bulldoze = json!({ "action": "bulldoze", "x": spot.0, "y": spot.1 });
    server.post(&guest, &format!("/api/worlds/{COMMONS}/act"), bulldoze).await.ok();
    assert_eq!(server.tile(spot), None);
}

#[tokio::test]
async fn a_world_of_their_own_goes() {
    let server = Server::start().await;
    let (_, zeph, _) = server.room("Crew", ZEPH).await;
    server
        .db()
        .execute(
            "INSERT INTO worlds (id, name, kind, seed, size, owner_id, active_at, created_at)
             VALUES ('own', 'Her world', 'own', 1, 64, ?, 0, 0)",
            params![zeph.id],
        )
        .unwrap();

    server.leave_for_good(&zeph).await;

    assert_eq!(server.count("SELECT COUNT(*) FROM worlds WHERE id = ?", "own"), 0);
}

// A44

#[tokio::test]
async fn each_room_that_stays_says_someone_deleted_their_account_and_not_who() {
    let server = Server::start().await;
    let city = server.city("Harbour", "Ann").await;
    let zeph = server.join_city(&city.code, ZEPH).await;
    let home = server.house(&zeph, &city.id, "Tidewater").await;
    let bo = server.move_in(&home, "Bo").await;
    let (room, code) = server.room_of(&zeph, "Crew").await;
    server.join_as(&bo, &code).await;
    let (left, left_code) = server.room_of(&bo, "Left").await;
    server.join_as(&zeph, &left_code).await;
    server.post(&zeph.key, &format!("/api/hangouts/{left}/leave"), json!({})).await.ok();

    server.leave_for_good(&zeph).await;

    let once = |lines: Vec<String>| lines.iter().filter(|line| *line == LINE).count();
    assert_eq!(once(server.messages(&bo.key, &room).await), 1, "a room");
    assert_eq!(once(server.messages(&bo.key, &city.square).await), 1, "a Town Square");
    assert_eq!(once(server.messages(&bo.key, &home.living).await), 1, "a Living Room");
    assert_eq!(once(server.messages(&bo.key, &home.porch).await), 0, "guests read a Porch");
    assert_eq!(once(server.messages(&bo.key, &left).await), 0, "they weren't there any more");
}

// A45

#[tokio::test]
async fn the_privacy_page_says_how_and_keeps_github_for_who_cant_sign_in() {
    let server = Server::start().await;
    let page = text(&server.get("", "/privacy").await.ok());
    for words in [
        "Delete account",
        "home page",
        "Someone who left",
        "can't be undone",
        "can't sign in",
        "what other people wrote",
    ] {
        assert!(page.to_lowercase().contains(&words.to_lowercase()), "it should say {words}");
    }
    assert!(!page.contains("There's no button"), "the stopgap has gone");
}

// All of it

/// One person who has done everything there is to do, among people who stay. Nothing that names them
/// may be left in any column of any table, so a table added later is held to this too.
#[tokio::test]
async fn nothing_in_any_table_names_them_afterwards() {
    let server = Server::start().await;
    let city = server.city("Harbour", "Ann").await;
    let ann = &city.keeper;
    let zeph = server.join_city(&city.code, ZEPH).await;
    let bo = server.join_city(&city.code, "Bo").await;
    let z = zeph.key.as_str();

    // A household, shared with Bo.
    let home = server.house(&zeph, &city.id, "Tidewater").await;
    let code = server.house_code(&home);
    server.post(&bo.key, &format!("/api/house-invites/{code}/join"), json!({})).await.ok();
    let (quill, quill_token) = server.bot(&zeph, &home.living, QUILL).await;
    server.state.db.set_discord_link(&quill, "12345", "zeph", now()).unwrap();
    let hearth = server
        .post(z, &format!("/api/houses/{}/house-bot", home.id), json!({ "name": "Quillhearth" }))
        .await
        .ok();
    let scene = server
        .post(z, &format!("/api/houses/{}/scenes", home.id), json!({ "name": "Lamp" }))
        .await
        .ok();
    server.post(z, &format!("/api/scenes/{}/request", text(&scene["id"])), json!({})).await.ok();
    let notes = format!("/api/houses/{}/notes", home.id);
    server.put(z, &notes, json!({ "topic": "Wifi", "text": "harbourlight" })).await.ok();
    server.put(&quill_token, &notes, json!({ "topic": "Bins", "text": "Tuesdays" })).await.ok();

    // A trip to Ann's house, and Ann's bot's trip to theirs.
    let ann_home = server.house(ann, &city.id, "Lighthouse").await;
    let (annie, annie_token) = server.bot(ann, &ann_home.living, "Annie").await;
    let visit = server.ask_trip(&annie_token, &annie, &home).await;
    server.answer_trip(z, &visit).await;
    server.answer_trip(&ann.key, &visit).await;
    server.post(z, &format!("/api/trips/{visit}/end"), json!({})).await.ok();
    let (wander, _) = server.bot(&zeph, &city.square, "Quillwander").await;
    server.ask_trip(z, &wander, &ann_home).await;

    // A room with everything in it, and a plan on the Town Square.
    let (room, room_code) = server.room_of(&zeph, "Crew").await;
    server.join_as(ann, &room_code).await;
    server.say(z, &room, "hello").await;
    server.bot(&zeph, &room, "Quillroom").await;
    server.note(z, &room, "remember this").await;
    let plan = server.plan(z, &room, "Tacos", 1).await;
    server.answer(&ann.key, &plan, "yes").await;
    server.list_item(z, &plan, "Salsa").await;
    server.answer(z, &plan, "maybe").await;
    let ann_plan = server.plan(&ann.key, &city.square, "Picnic", 2).await;
    server.answer(z, &ann_plan, "yes").await;
    let chips = server.list_item(&ann.key, &ann_plan, "Chips").await;
    server.claim(z, &chips).await;
    server.post(z, &format!("/api/events/{plan}/invite"), json!({})).await.ok();
    let shelf = server.item(z, &room, 30).await;
    server.item_note(z, &shelf, "read this").await;
    server.check_in(z, &room, "May I?", None).await;
    server.check_in(&ann.key, &room, "May we?", Some(ZEPH)).await;
    let answered = server.check_in(&ann.key, &room, "Tea?", None).await;
    let answer = json!({ "response": "go", "note": "yes" });
    server.post(z, &format!("/api/check-ins/{answered}/answer"), answer).await.ok();
    let mine = server.get(z, "/api/spaces").await.ok();
    let house = text(&mine["house"]["space"]["id"]);
    let invite = server.state.db.hangout(&house).unwrap().unwrap().invite_code;
    server.join_as(ann, &invite).await;

    // Everything about them.
    let (url, _) = start_receiver().await;
    let address = "http://zephyrine.example/hook";
    server.put(z, "/api/notify", json!({ "url": url })).await.ok();
    server.put(z, &format!("/api/agents/{quill}/notify"), json!({ "url": url })).await.ok();
    let hearth_id = text(&hearth["agent"]["id"]);
    server.state.db.set_notify_url(&hearth_id, Some(address)).unwrap();
    let clock = json!({ "time_zone": "America/Chicago", "utc_offset_minutes": -300 });
    server.put(z, "/api/clock", clock).await.ok();
    server.put(z, "/api/sharing", json!({ "free_times": "city" })).await.ok();
    server.put(z, "/api/me/look", json!({ "hat": "crown" })).await.ok();
    server.post(z, "/api/owner-notes", json!({ "kind": "fact", "text": "Likes tea" })).await.ok();
    server.put(z, &format!("/api/marks/{annie}"), json!({ "reason": "spammed" })).await.ok();
    let vouch = format!("/api/cities/{}/people/{}/vouch", city.id, zeph.id);
    server.post(&bo.key, &vouch, json!({})).await.ok();

    // The Commons.
    server.put_up(&zeph, "house").await;
    let guest = server.post("", "/api/public/join", json!({})).await.ok();
    let claim = json!({ "claim_code": text(&guest["claim_code"]) });
    server.post(z, "/api/players/claim", claim).await.ok();
    server.post(&quill_token, "/api/public/join", json!({})).await;

    let session = server.sign_in(&zeph).await;
    let held = [
        zeph.id.as_str(),
        &auth::hash_secret(&zeph.key),
        &auth::hash_secret(&session),
        &auth::hash_secret(&quill_token),
        quill.as_str(),
        wander.as_str(),
        address,
        url.as_str(),
        ZEPH,
        QUILL,
        "Quillwander",
        "Quillroom",
    ];
    assert!(!server.holding(&held).is_empty());

    server.ask_to_delete(&session, ZEPH).await.ok();

    assert_eq!(server.holding(&held), Vec::<String>::new());
    assert!(server.broken_keys().is_empty(), "{:?}", server.broken_keys());

    // What other people have is still there and still works.
    assert_eq!(server.one("SELECT owner_id FROM agents WHERE id = ?", &hearth_id), bo.id);
    server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
    server.get(&ann.key, &format!("/api/hangouts/{house}")).await.ok();
    server.get(&ann.key, &format!("/api/cities/{}", city.id)).await.ok();
    server.get(&bo.key, &format!("/api/houses/{}", home.id)).await.ok();
    server.get(&ann.key, "/api/me").await.ok();
    server.get(&bo.key, "/api/spaces/public").await.ok();
}
