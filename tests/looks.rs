//! Tests for how people and bots look. They drive a real server over HTTP and are written from the
//! rules in docs/looks.md. A comment such as `// C8` above a test names the rule it covers. Most of
//! these are about what strangers, walk-ins, bots and the public can't do.

use std::{collections::BTreeSet, time::Duration};

use agenthangout::{
    app,
    auth::hash_secret,
    db::{Db, Wearer, COLORS},
    looks::{bot_wears, person_wears, Look, Part, BOT_PARTS, MAX_CHANGES_PER_MINUTE, PERSON_PARTS},
    now,
    world::{self, Kind},
    worlds::{self, Move},
    AppState, Config,
};
use reqwest::{
    header::{HeaderMap, COOKIE, SET_COOKIE},
    Client, Method, StatusCode,
};
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

#[derive(Clone)]
struct Bot {
    id: String,
    token: String,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
}

fn who(body: &Value) -> Who {
    Who { key: text(&body["owner_key"]), id: text(&body["person"]["id"]) }
}

fn bot_path(bot: &Bot) -> String {
    format!("/api/agents/{}/look", bot.id)
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

    /// Someone new, by way of a room of their own: its id, and who they are.
    async fn someone(&self, name: &str) -> (String, Who) {
        let made = self
            .post("", "/api/hangouts", json!({ "name": format!("{name}'s room"), "your_name": name, "create_code": CODE }))
            .await
            .ok();
        (text(&made["hangout"]["id"]), who(&made))
    }

    /// Someone new in a browser: who they are, and their session cookie as a page would send it.
    async fn browser(&self, name: &str) -> (Who, String) {
        let body = json!({ "name": format!("{name}'s room"), "your_name": name, "create_code": CODE });
        let response = self.client.post(format!("{}/api/hangouts", self.base)).json(&body).send().await.unwrap();
        let cookie = response.headers()[SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
        (who(&response.json().await.unwrap()), cookie)
    }

    /// A page with The Commons open, as W52 knows one, on an address of its own.
    fn page_looks_at_the_commons(&self, cookie: &str, n: u8) {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/140".parse().unwrap());
        headers.insert("sec-fetch-site", "same-origin".parse().unwrap());
        headers.insert("sec-fetch-mode", "cors".parse().unwrap());
        headers.insert("x-forwarded-for", format!("198.51.100.{n}").parse().unwrap());
        if !cookie.is_empty() {
            headers.insert(COOKIE, cookie.parse().unwrap());
        }
        worlds::note_request(&self.state, &Method::GET, "/api/worlds/commons", &headers, now());
    }

    /// A room's snapshot. A person reading one has the floor open (S42).
    async fn snapshot(&self, token: &str, room: &str) -> Value {
        self.get(token, &format!("/api/hangouts/{room}")).await.ok()
    }

    async fn invite_code(&self, owner: &Who, room: &str) -> String {
        text(&self.snapshot(&owner.key, room).await["hangout"]["invite_code"])
    }

    async fn join(&self, owner: &Who, room: &str, name: &str) -> Who {
        let code = self.invite_code(owner, room).await;
        who(&self.post("", &format!("/api/invites/{code}/join"), json!({ "your_name": name })).await.ok())
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> Bot {
        let made = self.post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name })).await.ok();
        Bot { id: text(&made["agent"]["id"]), token: text(&made["token"]) }
    }

    async fn dress(&self, who: &Who, look: Value) -> Reply {
        self.put(&who.key, "/api/me/look", look).await
    }

    async fn dress_bot(&self, owner: &Who, bot: &Bot, look: Value) -> Reply {
        self.put(&owner.key, &bot_path(bot), look).await
    }

    async fn open(&self, owner: &Who, room: &str) {
        self.put(&owner.key, &format!("/api/floors/{room}"), json!({ "access": "public", "create_code": CODE })).await.ok();
    }

    async fn walk_in(&self, who: &Who, room: &str) {
        self.post(&who.key, &format!("/api/floors/{room}/walk-in"), json!({})).await.ok();
    }

    async fn read_room(&self, token: &str, room: &str) -> Value {
        let call = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "read_room", "arguments": { "hangout_id": room } },
        });
        self.post(token, "/mcp", call).await.ok()["result"]["structuredContent"].clone()
    }

    async fn tool(&self, token: &str, name: &str, arguments: Value) -> Value {
        let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        let result = self.post(token, "/mcp", call).await.ok()["result"].clone();
        assert!(result["isError"].is_null(), "{name} failed: {result}");
        result["structuredContent"].clone()
    }

    async fn stream(&self, token: &str, path: &str) -> reqwest::Response {
        self.client.get(format!("{}{path}", self.base)).bearer_auth(token).send().await.unwrap()
    }

    /// How many words are stored for someone, including any the wardrobe doesn't have.
    fn stored(&self, wearer: Wearer, id: &str) -> usize {
        self.state.db.looks(wearer, &[id.to_string()]).unwrap().get(id).map_or(0, |look| look.len())
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

    /// The Commons' view, after the pause that lets a change show (W29).
    async fn commons(&self) -> Value {
        tokio::time::sleep(Duration::from_millis(1100)).await;
        self.get("", &format!("/api/worlds/{COMMONS}")).await.ok()
    }
}

async fn heard_refresh(stream: &mut reqwest::Response) {
    let heard = tokio::time::timeout(Duration::from_secs(5), async {
        let mut seen = String::new();
        while !seen.contains("event: refresh") {
            let chunk = stream.chunk().await.unwrap().expect("the stream ended");
            seen.push_str(&String::from_utf8_lossy(&chunk));
        }
    })
    .await;
    assert!(heard.is_ok(), "nobody told this page");
}

/// A word from each part, all at once.
fn whole_look(parts: &[Part]) -> Value {
    Value::Object(parts.iter().map(|part| (part.part.to_string(), json!(part.choices[1].choice))).collect())
}

/// What a table in web/room.js says, from its name to its closing brace at the start of a line or
/// on the same line.
fn table<'a>(source: &'a str, name: &str) -> &'a str {
    let source = source.split_once(&format!("const {name} = {{")).unwrap_or_else(|| panic!("no table called {name}")).1;
    match source.find("\n}") {
        Some(end) if source.find("};").is_none_or(|one_line| end < one_line) => &source[..end],
        _ => &source[..source.find("};").unwrap()],
    }
}

// C1

#[tokio::test]
async fn every_word_in_the_wardrobe_can_be_worn_and_nothing_else_can() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    for part in PERSON_PARTS {
        for choice in part.choices {
            let reply = server.dress(&ann, json!({ part.part: choice.choice })).await.ok();
            assert_eq!(reply["look"][part.part], choice.choice);
        }
    }
    let (room, cy) = server.someone("Cy").await;
    let scout = server.bot(&cy, &room, "Scout").await;
    for part in BOT_PARTS {
        for choice in part.choices {
            let reply = server.dress_bot(&cy, &scout, json!({ part.part: choice.choice })).await.ok();
            assert_eq!(reply["look"][part.part], choice.choice);
        }
    }

    let (_, bo) = server.someone("Bo").await;
    let hostile = [
        json!({ "hat": "fez" }),
        json!({ "hat": "CAP" }),
        json!({ "hat": " cap" }),
        json!({ "hat": "cap; DROP TABLE people" }),
        json!({ "hat": "<img src=x onerror=alert(1)>" }),
        json!({ "hat": "" }),
        json!({ "shirt": "#ff0000" }),
        json!({ "shirt": "url(https://example.com/x)" }),
        json!({ "wings": "large" }),
        json!({ "__proto__": "cap" }),
        json!({ "": "" }),
    ];
    for body in hostile {
        let said = server.dress(&bo, body.clone()).await.refused(StatusCode::BAD_REQUEST);
        // What was sent isn't said back.
        assert_eq!(said["error"], "that isn't in the wardrobe", "{body}");
    }
    for body in [json!({ "hat": 7 }), json!({ "hat": ["cap"] }), json!({ "hat": { "a": 1 } }), json!("cap"), json!(["hat"]), json!(7)] {
        server.dress(&bo, body).await.refused(StatusCode::BAD_REQUEST);
    }
    let looks = server.get(&bo.key, "/api/looks").await.ok();
    assert_eq!(looks["you"]["look"], json!({}));
}

// C1

#[test]
fn a_wardrobe_has_each_part_and_each_word_once_and_only_listed_words() {
    let parts: Vec<&str> = PERSON_PARTS.iter().map(|part| part.part).collect();
    assert_eq!(parts, ["skin", "style", "hair", "face", "hat", "shirt", "trousers"]);
    let parts: Vec<&str> = BOT_PARTS.iter().map(|part| part.part).collect();
    assert_eq!(parts, ["shell", "head", "eyes", "chest", "hat"]);
    for part in PERSON_PARTS.iter().chain(&BOT_PARTS) {
        let words: BTreeSet<&str> = part.choices.iter().map(|each| each.choice).collect();
        assert_eq!(words.len(), part.choices.len(), "{} lists a word twice", part.part);
        assert!(part.choices.len() >= 3, "{} is hardly a choice", part.part);
        for choice in part.choices {
            assert!(!choice.label.is_empty());
            let plain = !choice.choice.is_empty() && choice.choice.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
            assert!(plain, "{} isn't a plain word", choice.choice);
        }
    }
}

// C1: nothing is in the wardrobe without art, on the page that draws it.

#[test]
fn every_word_in_the_wardrobe_is_drawn_in_a_room_and_on_the_map() {
    let room = include_str!("../web/room.js");
    let tables = [
        ("skin", "SKINS"), ("style", "STYLES"), ("hair", "HAIRS"), ("face", "FACES"), ("hat", "HATS"), ("shirt", "FABRICS"),
        ("trousers", "LEGS"), ("shell", "FABRICS"), ("head", "HEADS"), ("eyes", "EYES"), ("chest", "CHESTS"),
    ];
    for part in PERSON_PARTS.iter().chain(&BOT_PARTS) {
        let name = tables.iter().find(|(word, _)| *word == part.part).unwrap_or_else(|| panic!("no table for {}", part.part)).1;
        let art = table(room, name);
        for choice in part.choices {
            let at_start = art.lines().any(|line| {
                let line = line.trim_start();
                line.starts_with(&format!("{}:", choice.choice)) || line.starts_with(&format!("{}(", choice.choice))
            });
            assert!(at_start || art.contains(&format!(" {}: ", choice.choice)), "web/room.js has no {} {}", part.part, choice.choice);
        }
    }
    // The map draws them in its own art, with the same words.
    let map = include_str!("../web/world.js");
    for (part, name) in [("hat", "BOT_HATS"), ("chest", "BOT_CHESTS"), ("style", "PERSON_STYLES"), ("face", "PERSON_FACES")] {
        let art = table(map, name);
        let part = PERSON_PARTS.iter().chain(&BOT_PARTS).find(|each| each.part == part).unwrap();
        for choice in part.choices {
            assert!(art.contains(&format!("{}:", choice.choice)) || art.contains(&format!("{}(", choice.choice)), "web/world.js has no {}", choice.choice);
        }
    }
    let head = BOT_PARTS.iter().find(|each| each.part == "head").unwrap();
    let order: Vec<String> = head.choices.iter().map(|each| format!("'{}'", each.choice)).collect();
    assert!(map.contains(&format!("const BOT_HEADS = [{}];", order.join(", "))), "web/world.js has the heads in another order");
}

// C2

#[tokio::test]
async fn a_person_takes_a_persons_parts_and_a_bot_a_bots() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    for part in ["shell", "head", "eyes", "chest"] {
        let word = BOT_PARTS.iter().find(|each| each.part == part).unwrap().choices[0].choice;
        server.dress(&ann, json!({ part: word })).await.refused(StatusCode::BAD_REQUEST);
    }
    for part in ["skin", "style", "hair", "face", "shirt", "trousers"] {
        let word = PERSON_PARTS.iter().find(|each| each.part == part).unwrap().choices[0].choice;
        server.dress_bot(&ann, &scout, json!({ part: word })).await.refused(StatusCode::BAD_REQUEST);
    }
    assert_eq!(server.stored(Wearer::Person, &ann.id) + server.stored(Wearer::Bot, &scout.id), 0);
    // A hat is in both.
    server.dress(&ann, json!({ "hat": "crown" })).await.ok();
    server.dress_bot(&ann, &scout, json!({ "hat": "crown" })).await.ok();
}

// C3

#[tokio::test]
async fn nothing_is_locked_and_a_look_changes_nothing_but_the_drawing() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    // Nobody here has a friend, which is what holds back furniture (I19), and it holds back no look.
    assert_eq!(server.state.db.friend_count(&ann.id).unwrap(), 0);
    let before = server.snapshot(&ann.key, &room).await;
    let wardrobe = server.get(&ann.key, "/api/looks").await.ok().to_string();
    for word in ["unlocked", "friends_needed", "tier", "price", "cost"] {
        assert!(!wardrobe.contains(word), "{word} in {wardrobe}");
    }
    server.dress(&ann, whole_look(&PERSON_PARTS)).await.ok();
    server.dress_bot(&ann, &scout, whole_look(&BOT_PARTS)).await.ok();

    let mut after = server.snapshot(&ann.key, &room).await;
    assert_ne!(after["looks"], before["looks"]);
    let mut before = before;
    for snapshot in [&mut before, &mut after] {
        let fields = snapshot.as_object_mut().unwrap();
        fields.remove("looks");
        fields["agents"][0].as_object_mut().unwrap().remove("last_seen_at");
    }
    assert_eq!(after, before, "a look reached something else in the room");
    let catalog = server.get(&ann.key, "/api/items").await.ok();
    assert!(catalog["items"].as_array().unwrap().iter().any(|item| item["unlocked"] == false), "a look unlocked furniture");
}

// C4

#[tokio::test]
async fn only_what_someone_chose_is_stored_and_the_rest_is_worked_out() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    let snapshot = server.snapshot(&ann.key, &room).await;
    assert_eq!(snapshot["looks"]["people"][&ann.id]["skin"], "umber", "worked out, so nothing's stored");
    assert_eq!(server.stored(Wearer::Person, &ann.id), 0);

    let change = server.dress(&ann, json!({ "hat": "beanie" })).await.ok();
    assert_eq!(change["look"], json!({ "hat": "beanie" }), "what was chosen");
    assert_eq!(change["worn"]["hat"], "beanie");
    assert_eq!(change["worn"]["skin"], "umber", "and what wasn't");
    assert_eq!(server.stored(Wearer::Person, &ann.id), 1, "one word chosen is one row");
    server.dress_bot(&ann, &scout, json!({ "shell": "teal", "eyes": "amber" })).await.ok();
    assert_eq!(server.stored(Wearer::Bot, &scout.id), 2);
    assert_eq!(server.get(&ann.key, "/api/looks").await.ok()["bots"][0]["look"], json!({ "eyes": "amber", "shell": "teal" }));
}

// C4

#[test]
fn a_look_is_stored_as_words_for_parts_and_nothing_else() {
    let path = std::env::temp_dir().join(format!("looks-schema-{}-{}.db", std::process::id(), now()));
    let db = Db::open(path.to_str().unwrap()).unwrap();
    drop(db);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let columns = |table: &str| -> Vec<String> {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})")).unwrap();
        statement.query_map([], |row| row.get::<_, String>(1)).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(columns("person_looks"), ["person_id", "part", "choice"]);
    assert_eq!(columns("bot_looks"), ["agent_id", "part", "choice"]);
    drop(connection);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.to_str().unwrap()));
    }
}

// C4: the same sums pages did before the server took them over, so a figure nobody touched didn't
// change. These answers came from the page's own code, names that aren't plain ASCII included.

#[test]
fn a_part_nobody_chose_is_worked_out_from_the_name_as_it_always_was() {
    let cases = [
        ("Ann", "umber", "grey", "none", "navy", "antenna"),
        ("Bo", "cream", "blond", "none", "charcoal", "antenna"),
        ("Justin", "caramel", "black", "none", "navy", "twin"),
        ("Morgan", "tan", "blond", "none", "pine", "antenna"),
        ("Scout", "peach", "brown", "none", "charcoal", "twin"),
        ("Rover", "peach", "honey", "none", "navy", "twin"),
        ("Zo\u{eb}", "peach", "chestnut", "none", "navy", "twin"),
        ("\u{65e5}\u{672c}\u{8a9e}", "caramel", "black", "cap", "charcoal", "wide"),
        ("\u{1f916} Bot", "cream", "grey", "none", "pine", "twin"),
        ("a much longer name than anyone would type", "umber", "black", "cap", "khaki", "wide"),
    ];
    for (name, skin, hair, hat, trousers, head) in cases {
        let person = person_wears(name, COLORS[0], &Look::new());
        assert_eq!(
            (person["skin"].as_str(), person["hair"].as_str(), person["hat"].as_str(), person["trousers"].as_str()),
            (skin, hair, hat, trousers),
            "{name}"
        );
        assert_eq!((person["style"].as_str(), person["face"].as_str()), ("short", "none"), "{name}");
        let bot = bot_wears(name, COLORS[0], &Look::new());
        assert_eq!(bot["head"], head, "{name}");
        assert_eq!((bot["chest"].as_str(), bot["hat"].as_str()), ("plain", "none"), "{name}");
        assert!(!bot.contains_key("eyes"), "a bot's eyes follow its shell until someone chooses");
    }
    // What was chosen comes first.
    let chosen = Look::from([("hat".to_string(), "crown".to_string())]);
    assert_eq!(person_wears("Ann", COLORS[0], &chosen)["hat"], "crown");
    assert_eq!(bot_wears("Scout", COLORS[0], &chosen)["hat"], "crown");
}

// C4: a shirt or shell nobody chose is the owner's colour, exactly, and it's a word like any other.

#[test]
fn every_colour_a_person_is_handed_is_a_word_for_the_same_colour() {
    let page = include_str!("../web/room.js");
    let fabrics = table(page, "FABRICS");
    let wardrobe = PERSON_PARTS.iter().find(|part| part.part == "shirt").unwrap();
    for color in COLORS {
        let worn = person_wears("Ann", color, &Look::new());
        let word = worn["shirt"].as_str();
        assert!(wardrobe.choices.iter().any(|choice| choice.choice == word), "{word} isn't in the wardrobe");
        assert!(fabrics.contains(&format!("{word}: '{color}'")), "{word} isn't {color} in web/room.js");
        assert_eq!(bot_wears("Scout", color, &Look::new())["shell"], word);
    }
    // A colour from before these words came along is left for the page, as a part nobody has.
    assert!(!person_wears("Ann", "#123456", &Look::new()).contains_key("shirt"));
    assert!(!bot_wears("Scout", "#123456", &Look::new()).contains_key("shell"));
}

// C16

#[test]
fn no_page_works_out_a_look_for_anyone_with_an_account() {
    let room = include_str!("../web/room.js");
    for sum in ["hash(name)", "hash(data.name)", "SKIN[", "HAIR[", "TROUSERS["] {
        assert!(!room.contains(sum), "web/room.js still works out a look with {sum}");
    }
    assert!(room.contains("personLook(data.look)") && room.contains("botLook(data.look)"), "a room draws what it's told");
    // The city's map hands on what the server said.
    assert!(include_str!("../web/city.js").contains("this.view.bot_looks[bot.id]"));
    let map = include_str!("../web/world.js");
    assert!(map.contains("wornIn(row[6])"), "a player's row isn't read");
    assert!(map.contains("wornIn(row[3])"), "a row on the street isn't read");
}

// C5

#[tokio::test]
async fn a_change_names_what_it_changes_and_null_takes_a_part_back() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    server.dress(&ann, json!({ "hat": "crown", "shirt": "red" })).await.ok();
    // Left out stays.
    let look = server.dress(&ann, json!({ "shirt": "blue" })).await.ok();
    assert_eq!(look["look"], json!({ "hat": "crown", "shirt": "blue" }));
    let look = server.dress(&ann, json!({})).await.ok();
    assert_eq!(look["look"], json!({ "hat": "crown", "shirt": "blue" }));
    let look = server.dress(&ann, json!({ "hat": null })).await.ok();
    assert_eq!(look["look"], json!({ "shirt": "blue" }));
    // Taking back what was never chosen is nothing.
    let look = server.dress(&ann, json!({ "hat": null, "face": null })).await.ok();
    assert_eq!(look["look"], json!({ "shirt": "blue" }));
    assert_eq!(server.stored(Wearer::Person, &ann.id), 1);
    // A part still has to be a part.
    server.dress(&ann, json!({ "wings": null })).await.refused(StatusCode::BAD_REQUEST);
}

// C5

#[tokio::test]
async fn a_change_thats_refused_changes_nothing_even_if_some_of_it_was_fine() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    server.dress(&ann, json!({ "hat": "crown" })).await.ok();
    server.dress(&ann, json!({ "hat": null, "shirt": "red", "skin": "not-a-word" })).await.refused(StatusCode::BAD_REQUEST);
    server.dress(&ann, json!({ "shirt": "red", "wings": null })).await.refused(StatusCode::BAD_REQUEST);
    server.dress_bot(&ann, &scout, json!({ "shell": "teal", "hair": "red" })).await.refused(StatusCode::BAD_REQUEST);
    let looks = server.get(&ann.key, "/api/looks").await.ok();
    assert_eq!(looks["you"]["look"], json!({ "hat": "crown" }));
    assert_eq!(looks["bots"][0]["look"], json!({}));
}

// C6

#[tokio::test]
async fn a_stored_word_the_wardrobe_doesnt_have_is_left_out_on_read() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    // As an older or newer release might have left it.
    server.state.db.change_look(Wearer::Person, &ann.id, &[("hat", Some("fez")), ("cape", Some("red")), ("shirt", Some("red"))]).unwrap();
    server.state.db.change_look(Wearer::Bot, &scout.id, &[("hat", Some("fez")), ("shell", Some("mauve"))]).unwrap();

    let snapshot = server.snapshot(&ann.key, &room).await;
    let worn = &snapshot["looks"]["people"][&ann.id];
    assert_eq!((&worn["shirt"], &worn["hat"]), (&json!("red"), &json!("none")), "{worn}");
    assert!(worn.get("cape").is_none(), "{worn}");
    assert_eq!(snapshot["looks"]["bots"][&scout.id]["hat"], "none");
    assert_eq!(snapshot["looks"]["bots"][&scout.id]["shell"], "sky", "its owner's colour instead");
    let looks = server.get(&ann.key, "/api/looks").await.ok();
    assert_eq!(looks["you"]["look"], json!({ "shirt": "red" }));
    assert_eq!(looks["bots"][0]["look"], json!({}));
    // And a change's reply doesn't bring it back.
    assert_eq!(server.dress(&ann, json!({ "face": "beard" })).await.ok()["look"], json!({ "face": "beard", "shirt": "red" }));
}

// C7

#[tokio::test]
async fn a_person_changes_their_own_look_and_their_own_bots_and_nobody_elses() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    server.dress(&ann, json!({ "hat": "crown" })).await.ok();
    server.dress_bot(&ann, &scout, json!({ "shell": "teal" })).await.ok();

    // Someone else's bot is one that doesn't exist, even to whoever is in the room with it.
    let body = json!({ "shell": "red" });
    let nothing = server.put(&bo.key, "/api/agents/no-such-bot/look", body.clone()).await.refused(StatusCode::NOT_FOUND);
    assert_eq!(server.dress_bot(&bo, &scout, body.clone()).await.refused(StatusCode::NOT_FOUND), nothing);
    // A bad body tells a stranger nothing more.
    assert_eq!(server.dress_bot(&bo, &scout, json!({ "wings": "large" })).await.refused(StatusCode::NOT_FOUND), nothing);
    server.put("", &bot_path(&scout), body.clone()).await.refused(StatusCode::UNAUTHORIZED);
    server.put("", "/api/me/look", json!({ "hat": "cap" })).await.refused(StatusCode::UNAUTHORIZED);

    let looks = server.get(&ann.key, "/api/looks").await.ok();
    assert_eq!(looks["bots"][0]["look"], json!({ "shell": "teal" }));
    // There's no route that names another person.
    for path in [format!("/api/people/{}/look", ann.id), format!("/api/me/{}/look", ann.id)] {
        let reply = server.put(&bo.key, &path, json!({ "hat": "cap" })).await;
        assert!(reply.status.is_client_error(), "{path}: {}", reply.status);
    }
    assert_eq!(server.get(&ann.key, "/api/looks").await.ok()["you"]["look"], json!({ "hat": "crown" }));
}

// C7

#[tokio::test]
async fn someone_who_walked_in_can_change_their_own_look_though_not_the_floor() {
    let server = Server::start().await;
    let (floor, ann) = server.someone("Ann").await;
    server.open(&ann, &floor).await;
    let (_, walker) = server.someone("Walker").await;
    server.walk_in(&walker, &floor).await;
    server.post(&walker.key, &format!("/api/floors/{floor}/items"), json!({ "kind": "plant", "x": 28, "y": 0 }))
        .await
        .refused(StatusCode::FORBIDDEN);
    server.dress(&walker, json!({ "hat": "party" })).await.ok();
    let snapshot = server.snapshot(&walker.key, &floor).await;
    assert_eq!(snapshot["looks"]["people"][&walker.id]["hat"], "party");
    let snapshot = server.snapshot(&ann.key, &floor).await;
    assert_eq!(snapshot["looks"]["people"][&walker.id]["hat"], "party", "the room sees it too");
}

// C7, R44

#[tokio::test]
async fn a_house_bot_goes_with_its_house_as_it_does_for_everything_else() {
    let server = Server::start().await;
    let made = server.post("", "/api/cities", json!({ "name": "Elm", "your_name": "Ann", "create_code": CODE })).await.ok();
    let ann = who(&made);
    let city = text(&made["city"]["id"]);
    let house = server.post(&ann.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Ann's" })).await.ok();
    let house = text(&house["house"]["id"]);
    let code = server.state.db.house(&house).unwrap().unwrap().invite_code;
    let zed = who(&server.post("", &format!("/api/house-invites/{code}/join"), json!({ "your_name": "Zed" })).await.ok());
    let home = server.post(&zed.key, &format!("/api/houses/{house}/house-bot"), json!({ "name": "Home" })).await.ok();
    let home = Bot { id: text(&home["agent"]["id"]), token: text(&home["token"]) };
    server.dress_bot(&zed, &home, json!({ "hat": "top_hat" })).await.ok();
    assert_eq!(server.get(&zed.key, "/api/looks").await.ok()["bots"][0]["id"], home.id.as_str());

    server.delete(&ann.key, &format!("/api/houses/{house}/residents/{}", zed.id)).await.ok();
    server.dress_bot(&zed, &home, json!({ "hat": "crown" })).await.refused(StatusCode::NOT_FOUND);
    assert_eq!(server.get(&zed.key, "/api/looks").await.ok()["bots"], json!([]));
    assert_eq!(server.stored(Wearer::Bot, &home.id), 1, "it's still the house's, in what it wore");
}

// C8

#[tokio::test]
async fn a_bot_cant_change_any_look_and_has_no_tool_for_it() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    let rover = server.bot(&ann, &room, "Rover").await;
    let body = json!({ "hat": "crown" });
    // Its own, the bot beside it, its owner, and with a body that's no good.
    server.put(&scout.token, &bot_path(&scout), body.clone()).await.refused(StatusCode::FORBIDDEN);
    server.put(&scout.token, &bot_path(&rover), body.clone()).await.refused(StatusCode::FORBIDDEN);
    server.put(&scout.token, "/api/me/look", body.clone()).await.refused(StatusCode::FORBIDDEN);
    server.put(&scout.token, "/api/me/look", json!({ "wings": 7 })).await.refused(StatusCode::FORBIDDEN);
    assert_eq!(server.stored(Wearer::Person, &ann.id) + server.stored(Wearer::Bot, &scout.id), 0);
    assert_eq!(server.stored(Wearer::Bot, &rover.id), 0);

    let tools = server.post(&scout.token, "/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).await.ok();
    let names: Vec<String> = tools["result"]["tools"].as_array().unwrap().iter().map(|tool| text(&tool["name"])).collect();
    assert!(!names.is_empty());
    assert!(names.iter().all(|name| !name.contains("look") && !name.contains("dress") && !name.contains("wardrobe")), "{names:?}");
}

// C9

#[tokio::test]
async fn a_person_can_make_sixty_changes_a_minute_across_themselves_and_their_bots() {
    assert_eq!(MAX_CHANGES_PER_MINUTE, 60);
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    let (_, bo) = server.someone("Bo").await;
    // The count goes by the clock's minute, so this starts well inside one.
    if now() % 60 > 40 {
        tokio::time::sleep(Duration::from_secs((61 - now() % 60) as u64)).await;
    }
    for n in 0..MAX_CHANGES_PER_MINUTE {
        if n % 2 == 0 {
            server.dress(&ann, json!({ "hat": "cap" })).await.ok();
        } else {
            server.dress_bot(&ann, &scout, json!({ "hat": "cap" })).await.ok();
        }
    }
    let said = server.dress(&ann, json!({ "hat": "crown" })).await.refused(StatusCode::TOO_MANY_REQUESTS);
    assert!(text(&said["error"]).contains("60"), "{said}");
    server.dress_bot(&ann, &scout, json!({ "hat": "crown" })).await.refused(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(server.get(&ann.key, "/api/looks").await.ok()["you"]["look"], json!({ "hat": "cap" }));
    // It's one person's count.
    server.dress(&bo, json!({ "hat": "crown" })).await.ok();
}

// C10

#[tokio::test]
async fn a_change_refreshes_every_room_the_figure_is_in() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let (floor, cy) = server.someone("Cy").await;
    server.open(&cy, &floor).await;
    let scout = server.bot(&ann, &room, "Scout").await;
    server.walk_in(&ann, &floor).await;
    let path = |room: &str| format!("/api/hangouts/{room}/stream");
    let mut bos = server.stream(&bo.key, &path(&room)).await;
    let mut cys = server.stream(&cy.key, &path(&floor)).await;

    server.dress(&ann, json!({ "hat": "cap" })).await.ok();
    heard_refresh(&mut bos).await;
    heard_refresh(&mut cys).await;
    server.dress_bot(&ann, &scout, json!({ "hat": "cap" })).await.ok();
    heard_refresh(&mut bos).await;
}

// C10, C13

#[tokio::test]
async fn a_bots_change_refreshes_its_owners_city_maps() {
    let server = Server::start().await;
    let made = server.post("", "/api/cities", json!({ "name": "Elm", "your_name": "Ann", "create_code": CODE })).await.ok();
    let ann = who(&made);
    let city = text(&made["city"]["id"]);
    let home = server.post(&ann.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Ann's" })).await.ok();
    let scout = server.bot(&ann, &text(&home["living_room_id"]), "Scout").await;
    let mut map = server.stream(&ann.key, &format!("/api/cities/{city}/stream")).await;
    assert_eq!(map.status(), StatusCode::OK);
    server.dress_bot(&ann, &scout, json!({ "shell": "pink" })).await.ok();
    heard_refresh(&mut map).await;

    // C13
    let view = server.get(&ann.key, &format!("/api/cities/{city}")).await.ok();
    assert_eq!(view["bot_looks"][&scout.id]["shell"], "pink");
    assert_eq!(view["bot_looks"][&scout.id]["head"], "twin", "and what nobody chose, the same as in a room");
    let seen = server.get(&scout.token, &format!("/api/cities/{city}")).await.ok();
    assert_eq!(seen["bot_looks"], json!({}), "a bot has nothing to draw");
}

// C11

#[tokio::test]
async fn a_room_has_the_looks_of_those_drawn_in_it_and_nobody_elses() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    server.dress(&ann, json!({ "hat": "cap" })).await.ok();
    server.dress(&bo, json!({ "hat": "crown" })).await.ok();
    server.dress_bot(&ann, &scout, json!({ "shell": "teal" })).await.ok();

    // Bo hasn't got the floor open, so Ann's page has nothing to draw for them.
    let snapshot = server.snapshot(&ann.key, &room).await;
    assert_eq!(snapshot["looks"]["people"].as_object().unwrap().len(), 1, "{}", snapshot["looks"]);
    assert_eq!(snapshot["looks"]["people"][&ann.id]["hat"], "cap");
    assert_eq!(snapshot["looks"]["bots"][&scout.id]["shell"], "teal");
    server.snapshot(&bo.key, &room).await;
    let snapshot = server.snapshot(&ann.key, &room).await;
    assert_eq!(snapshot["looks"]["people"][&bo.id]["hat"], "crown");

    // A look isn't something a bot reads, however it asks.
    assert_eq!(server.snapshot(&scout.token, &room).await["looks"], json!({ "people": {}, "bots": {} }));
    let read = server.read_room(&scout.token, &room).await;
    assert!(read.is_object(), "{read}");
    assert!(read.get("looks").is_none(), "{read}");
    let said = read.to_string();
    assert!(!said.contains("teal") && !said.contains("crown"), "{said}");
}

// C11

#[tokio::test]
async fn a_guest_bot_in_a_room_is_drawn_in_its_look() {
    let server = Server::start().await;
    let (floor, cy) = server.someone("Cy").await;
    server.open(&cy, &floor).await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    server.dress_bot(&ann, &scout, json!({ "hat": "witch" })).await.ok();
    server.post(&ann.key, &format!("/api/floors/{floor}/walk-in"), json!({ "agent_id": scout.id })).await.ok();
    let snapshot = server.snapshot(&cy.key, &floor).await;
    assert_eq!(snapshot["looks"]["bots"][&scout.id]["hat"], "witch");
}

// C12

#[tokio::test]
async fn a_name_tag_keeps_its_owners_colour_whatever_the_figure_wears() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    let colour = |snapshot: &Value| snapshot["people"][0]["color"].clone();
    let before = colour(&server.snapshot(&ann.key, &room).await);
    server.dress(&ann, json!({ "shirt": "red" })).await.ok();
    server.dress_bot(&ann, &scout, json!({ "shell": "red" })).await.ok();
    let snapshot = server.snapshot(&ann.key, &room).await;
    assert_eq!(colour(&snapshot), before);
    assert!(snapshot["agents"][0].get("color").is_none(), "a bot has no colour of its own to change");

    // The tag's dot comes from the owner, and only the drawing looks at what's worn.
    let page = include_str!("../web/room.js");
    assert!(page.contains("dot.style.background = data.color;"), "a tag has lost its owner's colour");
    assert!(page.contains("const color = coat || bot.color;"), "a bot is drawn in a colour nobody chose");
    assert!(page.contains("const color = shirt || person.color;"), "a person is drawn in a colour nobody chose");
    let app = include_str!("../web/app.js");
    assert!(app.contains("color: colorOf(agent.owner_id),"), "a bot's tag isn't its owner's colour");
}

// C14, C16

#[tokio::test]
async fn a_person_and_their_bot_wear_on_the_commons_what_they_wear_in_a_room() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    // Each with a player, and each on the map. Ann dressed, and their bot left as it came.
    server.house_two_days_ago(&ann);
    let own = server.state.db.own_player(&ann.id, now()).unwrap();
    let wave = Move { action: "emote".into(), x: None, y: None, x2: None, y2: None, kind: None, emote: Some("wave".into()) };
    worlds::act(&server.state, &own, COMMONS, &wave, now()).unwrap();
    let joined = server.post(&scout.token, "/api/public/join", json!({})).await.ok();
    let token = text(&joined["player_token"]);
    server.post(&token, &format!("/api/worlds/{COMMONS}/act"), json!({ "action": "emote", "emote": "wave" })).await.ok();
    server.dress(&ann, json!({ "hat": "witch", "trousers": "khaki", "skin": "espresso", "shirt": "teal" })).await.ok();

    let view = server.commons().await;
    let rows = view["players"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let row = |kind: &str| rows.iter().find(|row| row[5] == kind).unwrap().as_array().unwrap().clone();
    let inside = server.snapshot(&ann.key, &room).await;
    assert_eq!(row("person")[6], inside["looks"]["people"][&ann.id], "Ann outside isn't Ann inside");
    assert_eq!(row("agent")[6], inside["looks"]["bots"][&scout.id], "their bot outside isn't their bot inside");
    assert_eq!(row("person")[6]["hat"], "witch");
    assert_eq!(row("person")[6]["shirt"], "teal");

    // Words, and nothing to say whose. A player's name stays generated, and it's the same as before.
    let said = view.to_string();
    for secret in [ann.id.as_str(), scout.id.as_str(), "Scout", "Ann's room"] {
        assert!(!said.contains(secret), "{secret} reached The Commons");
    }
    assert_eq!(row("person")[1], world::player_name(own.name_seed.max(0) as u64).as_str());
    // Ann's house names them (W71), and a place's row wears nothing.
    let owners = view["owners"].as_array().unwrap();
    assert!(!owners.is_empty(), "{view}");
    assert!(owners.iter().all(|row| row.as_array().unwrap().len() == 3), "{owners:?}");

    // A change shows out there without a move.
    server.dress(&ann, json!({ "hat": "crown" })).await.ok();
    let view = server.commons().await;
    let rows = view["players"].as_array().unwrap();
    assert_eq!(rows.iter().find(|row| row[5] == "person").unwrap()[6]["hat"], "crown");
}

// C14

#[tokio::test]
async fn a_guest_and_a_player_someone_claimed_have_no_look() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    server.dress(&ann, json!({ "hat": "witch" })).await.ok();
    let wave = json!({ "action": "emote", "emote": "wave" });
    let guest = server.post("", "/api/public/join", json!({})).await.ok();
    let claimed = server.post("", "/api/public/join", json!({})).await.ok();
    for joined in [&guest, &claimed] {
        server.post(&text(&joined["player_token"]), &format!("/api/worlds/{COMMONS}/act"), wave.clone()).await.ok();
    }
    server.post(&ann.key, "/api/players/claim", json!({ "claim_code": claimed["claim_code"] })).await.ok();

    let view = server.commons().await;
    let rows = view["players"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    // Claiming a player doesn't make it someone (W53), so Ann's look stays Ann's.
    assert!(rows.iter().all(|row| row[5] == "agent" && row[6].is_null()), "{rows:?}");
    assert!(!view.to_string().contains("witch"));
}

// C14, W52

#[tokio::test]
async fn someone_signed_in_is_on_the_street_in_their_look_and_nobody_else_is_changed() {
    let server = Server::start().await;
    let (ann, cookie) = server.browser("Ann").await;
    server.dress(&ann, json!({ "hat": "witch", "shirt": "teal" })).await.ok();
    // Ann hasn't made a move, so they have no player: it's looking that puts them on the street.
    assert!(!server.state.db.has_own_player(&ann.id).unwrap());
    server.page_looks_at_the_commons(&cookie, 1);
    server.page_looks_at_the_commons("", 2);
    assert!(!server.state.db.has_own_player(&ann.id).unwrap(), "looking saves nothing");

    let view = server.commons().await;
    let rows = view["people"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let signed = rows.iter().find(|row| row.as_array().unwrap().len() == 4).expect("nobody is dressed");
    assert_eq!((&signed[3]["hat"], &signed[3]["shirt"]), (&json!("witch"), &json!("teal")));
    assert!(signed[0].as_str().unwrap().contains(' '), "still a generated name: {signed}");
    let anyone = rows.iter().find(|row| row.as_array().unwrap().len() == 3).expect("someone not signed in has no look");
    assert!(anyone[0].is_string());
    assert!(!view.to_string().contains(&ann.id));

    // Their next look shows what they changed.
    server.dress(&ann, json!({ "hat": "crown" })).await.ok();
    server.page_looks_at_the_commons(&cookie, 1);
    let view = server.commons().await;
    let rows = view["people"].as_array().unwrap();
    assert_eq!(rows.iter().find(|row| row.as_array().unwrap().len() == 4).unwrap()[3]["hat"], "crown");

    // A sign-in that doesn't check out is anyone.
    let (_, stale) = server.browser("Bo").await;
    server.state.db.delete_session(&hash_secret(stale.trim_start_matches("ah_session="))).unwrap();
    server.page_looks_at_the_commons(&stale, 3);
    let view = server.commons().await;
    assert_eq!(view["people"].as_array().unwrap().iter().filter(|row| row.as_array().unwrap().len() == 4).count(), 1);
}

// C14, L17

#[tokio::test]
async fn a_bot_shown_by_name_at_a_place_wears_its_own_look_and_not_its_owners() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    server.house_two_days_ago(&ann);
    let started = server.tool(&scout.token, "start_project", json!({ "name": "Garden robot" })).await;
    let spot = server.free_spots()[0];
    server.tool(&scout.token, "build_studio", json!({ "project_id": started["project_id"], "x": spot.0, "y": spot.1 })).await;
    let hours = |public_places: bool| json!({ "days": ["mon", "tue", "wed", "thu", "fri", "sat", "sun"], "starts": "00:00", "ends": "24:00", "public_places": public_places });
    server.put(&ann.key, &format!("/api/agents/{}/routine", scout.id), hours(true)).await.ok();
    server.dress(&ann, json!({ "hat": "witch" })).await.ok();

    // Nothing chosen is still what it wears in a room.
    let out = server.commons().await["bots_out"].clone();
    let inside = server.snapshot(&ann.key, &room).await;
    assert_eq!((&out[0][0], &out[0][3]), (&json!("Scout"), &inside["looks"]["bots"][&scout.id]), "{out}");

    server.dress_bot(&ann, &scout, json!({ "hat": "crown", "shell": "teal" })).await.ok();
    let view = server.commons().await;
    assert_eq!(view["bots_out"][0].as_array().unwrap().len(), 4, "{}", view["bots_out"]);
    assert_eq!(view["bots_out"][0][3]["hat"], "crown");
    assert_eq!(view["bots_out"][0][3]["shell"], "teal");
    assert!(!view["bots_out"].to_string().contains("witch"), "its owner's look isn't its own");

    // With public places off it's not shown, so neither is its look.
    server.put(&ann.key, &format!("/api/agents/{}/routine", scout.id), hours(false)).await.ok();
    let view = server.commons().await;
    assert_eq!(view["bots_out"], json!([]));
    assert!(!view.to_string().contains("crown"));
}

// C15

#[tokio::test]
async fn the_wardrobe_gives_a_person_their_own_looks_and_only_theirs() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let bo = server.join(&ann, &room, "Bo").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    let rover = server.bot(&bo, &room, "Rover").await;
    server.dress(&ann, json!({ "hat": "cap" })).await.ok();
    server.dress(&bo, json!({ "hat": "crown" })).await.ok();
    server.dress_bot(&ann, &scout, json!({ "shell": "teal" })).await.ok();
    server.dress_bot(&bo, &rover, json!({ "shell": "pink" })).await.ok();

    let looks = server.get(&ann.key, "/api/looks").await.ok();
    let keys: BTreeSet<&str> = looks.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, BTreeSet::from(["bots", "parts", "you"]));
    assert_eq!(looks["you"]["id"], ann.id.as_str());
    assert_eq!(looks["you"]["name"], "Ann");
    assert_eq!(looks["you"]["look"], json!({ "hat": "cap" }));
    assert!(looks["you"]["color"].is_string());
    assert_eq!(looks["you"]["worn"]["hat"], "cap");
    assert_eq!(looks["you"]["worn"]["skin"], "umber", "and what's worked out, for the page to show");
    assert_eq!(looks["bots"][0]["id"], scout.id.as_str());
    assert_eq!(looks["bots"][0]["name"], "Scout");
    assert_eq!(looks["bots"][0]["look"], json!({ "shell": "teal" }));
    assert_eq!(looks["bots"][0]["worn"]["shell"], "teal");
    assert_eq!(looks["bots"].as_array().unwrap().len(), 1);
    let said = looks.to_string();
    assert!(!said.contains("\"Bo\"") && !said.contains(&bo.id) && !said.contains(&rover.id), "{said}");

    let person = &looks["parts"]["person"];
    assert_eq!(person.as_array().unwrap().len(), PERSON_PARTS.len());
    assert_eq!(person[4]["part"], "hat");
    assert_eq!(person[4]["choices"][1], json!({ "choice": "cap", "label": "Cap" }));
    assert_eq!(looks["parts"]["bot"].as_array().unwrap().len(), BOT_PARTS.len());

    server.get(&scout.token, "/api/looks").await.refused(StatusCode::FORBIDDEN);
    server.get("", "/api/looks").await.refused(StatusCode::UNAUTHORIZED);
}
