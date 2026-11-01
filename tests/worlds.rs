//! Tests for the public worlds, written from the rules in docs/public-city.md (W1 to W74). They
//! drive a real server over HTTP. A comment such as `// W13` above a test names the rule it
//! covers, so coverage can be checked by searching for it.
//!
//! Time is real for anything that goes over HTTP. The tests that need to look days ahead call the
//! functions in `worlds` directly with a time of their own.

use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use agenthangout::{
    app, auth,
    db::{Db, NewWorld},
    error::ApiResult,
    now,
    world::{self, Change, Kind, Tile},
    worlds::{self, Acted, Move},
    AppState, Config,
};
use axum::http::{HeaderMap, Method, StatusCode};
use reqwest::Client;
use serde_json::{json, Value};

const COMMONS: &str = "commons";

/// The limits count by the minute, the hour and the day on the clock, so a test that counts what one
/// of them lets through starts early in it, with plenty of it left.
async fn start_early_in(period: i64, room: i64) {
    let left = period - agenthangout::now().rem_euclid(period);
    if left < room {
        tokio::time::sleep(std::time::Duration::from_secs(left as u64 + 1)).await;
    }
}
const CHROME: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/126.0 Safari/537.36";
const GOOGLEBOT: &str = "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";
const GPTBOT: &str = "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; GPTBot/1.2";
const CHATGPT_USER: &str =
    "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; ChatGPT-User/1.0";
const GEMINI: &str = "Mozilla/5.0 (compatible; Google-Agent)";

struct Server {
    base: String,
    client: Client,
    state: AppState,
    ips: AtomicU32,
}

struct Reply {
    status: StatusCode,
    body: Value,
    text: String,
    headers: reqwest::header::HeaderMap,
}

impl Reply {
    fn show(&self) -> String {
        format!("{} {}", self.status, self.text)
    }

    #[track_caller]
    fn ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.show());
        self.body
    }
}

/// A person who is signed in: their owner key, their id and a session cookie.
#[derive(Clone)]
struct Who {
    key: String,
    id: String,
    cookie: String,
}

/// A guest, as `join` made it.
#[derive(Clone)]
struct Guest {
    token: String,
    claim: String,
    name: String,
}

struct Req<'a> {
    server: &'a Server,
    method: Method,
    path: String,
    token: Option<String>,
    cookie: Option<String>,
    headers: Vec<(String, String)>,
    body: Option<Value>,
    raw: Option<(String, Vec<u8>)>,
}

impl<'a> Req<'a> {
    fn token(mut self, token: &str) -> Self {
        self.token = Some(token.to_string());
        self
    }

    fn cookie(mut self, cookie: &str) -> Self {
        self.cookie = Some(cookie.to_string());
        self
    }

    fn ip(self, ip: &str) -> Self {
        self.header("x-forwarded-for", ip)
    }

    fn ua(self, agent: &str) -> Self {
        self.header("user-agent", agent)
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    fn json(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }

    fn raw(mut self, content_type: &str, bytes: &[u8]) -> Self {
        self.raw = Some((content_type.to_string(), bytes.to_vec()));
        self
    }

    async fn send(self) -> Reply {
        let url = format!("{}{}", self.server.base, self.path);
        let mut request = self.server.client.request(self.method, url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(cookie) = &self.cookie {
            request = request.header("cookie", cookie);
        }
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        if let Some(body) = &self.body {
            request = request.json(body);
        }
        if let Some((content_type, bytes)) = self.raw {
            request = request.header("content-type", content_type).body(bytes);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let text = response.text().await.unwrap();
        let body = serde_json::from_str(&text).unwrap_or(Value::Null);
        Reply { status, body, text, headers }
    }
}

impl Server {
    async fn start() -> Self {
        Self::boot(Db::open(":memory:").unwrap(), None, None).await
    }

    /// A server whose keeper is a person made first, so their id can be set in the config.
    async fn start_with_keeper() -> (Self, Who) {
        let db = Db::open(":memory:").unwrap();
        let key = auth::new_owner_key();
        db.create_person(
            "keeper0000000001",
            "Keeper",
            &auth::hash_secret(&key),
            &auth::new_feed_token(),
            now(),
        )
        .unwrap();
        let server = Self::boot(db, None, Some("keeper0000000001".to_string())).await;
        let keeper = Who { key, id: "keeper0000000001".to_string(), cookie: String::new() };
        (server, keeper)
    }

    async fn boot(db: Db, create_code: Option<String>, keeper_id: Option<String>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = AppState::new(
            db,
            Config {
                public_url: base.clone(),
                create_code,
                allow_private_webhooks: true,
                keeper_id,
                god_ids: Vec::new(),
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        Self { base, client: Client::new(), state, ips: AtomicU32::new(1) }
    }

    fn req(&self, method: Method, path: &str) -> Req<'_> {
        Req {
            server: self,
            method,
            path: path.to_string(),
            token: None,
            cookie: None,
            headers: Vec::new(),
            body: None,
            raw: None,
        }
    }

    fn get(&self, path: &str) -> Req<'_> {
        self.req(Method::GET, path)
    }

    fn post(&self, path: &str, body: Value) -> Req<'_> {
        self.req(Method::POST, path).json(body)
    }

    /// An address that hasn't been used yet, so limits per address don't get in the way.
    fn fresh_ip(&self) -> String {
        let n = self.ips.fetch_add(1, Ordering::Relaxed);
        format!("10.{}.{}.{}", (n >> 16) & 255, (n >> 8) & 255, n & 255)
    }

    // People and players

    async fn person(&self, name: &str) -> Who {
        self.person_with_body(json!({ "name": "Crew", "your_name": name })).await
    }

    async fn person_with_body(&self, body: Value) -> Who {
        let reply = self.post("/api/hangouts", body).send().await;
        let cookie = reply
            .headers
            .get("set-cookie")
            .map(|value| value.to_str().unwrap().split(';').next().unwrap().to_string())
            .unwrap_or_default();
        let body = reply.ok();
        Who {
            key: body["owner_key"].as_str().unwrap().to_string(),
            id: body["person"]["id"].as_str().unwrap().to_string(),
            cookie,
        }
    }

    async fn guest(&self) -> Guest {
        let ip = self.fresh_ip();
        let body = self.post("/api/public/join", json!({})).ip(&ip).send().await.ok();
        Guest {
            token: body["player_token"].as_str().unwrap().to_string(),
            claim: body["claim_code"].as_str().unwrap().to_string(),
            name: body["name"].as_str().unwrap().to_string(),
        }
    }

    async fn act(&self, token: &str, world: &str, body: Value) -> Reply {
        self.post(&format!("/api/worlds/{world}/act"), body).token(token).send().await
    }

    async fn me(&self, token: &str, world: &str) -> Value {
        self.get(&format!("/api/worlds/{world}/me")).token(token).send().await.ok()
    }

    async fn player_id(&self, token: &str) -> i64 {
        self.me(token, COMMONS).await["player"]["id"].as_i64().unwrap()
    }

    async fn moves_left(&self, token: &str) -> i64 {
        self.me(token, COMMONS).await["moves_left"].as_i64().unwrap()
    }

    async fn view(&self, world: &str) -> Value {
        self.get(&format!("/api/worlds/{world}")).send().await.ok()
    }

    /// A view after the one-second pause that lets a change show.
    async fn fresh_view(&self, world: &str) -> Value {
        tokio::time::sleep(Duration::from_millis(1100)).await;
        self.view(world).await
    }

    /// A world someone started before nobody could (W25). It is frozen (W27), so it is put straight
    /// in the database. Returns its id and its link.
    fn own_world(&self, owner: &Who, open: bool) -> (String, String) {
        let n = self.ips.fetch_add(1, Ordering::Relaxed);
        self.own_world_of(owner, open, 1000 + u64::from(n))
    }

    fn own_world_of(&self, owner: &Who, open: bool, seed: u64) -> (String, String) {
        let id = format!("own-{seed}");
        let code = format!("link-{seed}");
        let name = world::world_name(seed);
        let land = world::terrain(seed, world::OWN_SIZE);
        let highway = world::highway(seed, &land);
        let new = NewWorld {
            id: &id,
            name: &name,
            kind: "own",
            seed,
            size: world::OWN_SIZE,
            owner_id: Some(&owner.id),
            open,
            invite_code: Some(&code),
            highway: &highway,
            now: now(),
        };
        self.state.db.insert_world(&new, worlds::MAX_OWN_WORLDS).unwrap();
        (id, code)
    }

    /// Writes tiles straight to a world, for what nobody can build any more.
    fn write(&self, world: &str, changes: &[Change]) {
        self.state
            .db
            .in_transaction::<(), rusqlite::Error>(|tx| tx.write_changes(world, changes, now()))
            .unwrap();
    }

    /// Road tiles in a world that a player built, put there directly.
    fn built_by(&self, world: &str, player: i64, at: &[(i64, i64)]) {
        let tile = Tile { kind: Kind::Road, level: 0, player: Some(player as u64) };
        let changes: Vec<Change> =
            at.iter().map(|&(x, y)| Change::Set((x as u16, y as u16), tile)).collect();
        self.write(world, &changes);
    }

    /// A `place` move at a time of the test's choosing, for the kinds that come with time (W61).
    fn place_at(&self, who: &Who, kind: &str, (x, y): (i64, i64), at: i64) -> ApiResult<Acted> {
        let player = self.state.db.own_player(&who.id, at).unwrap();
        let mv = Move {
            action: "place".to_string(),
            x: Some(x),
            y: Some(y),
            x2: None,
            y2: None,
            kind: Some(kind.to_string()),
            emote: None,
        };
        worlds::act(&self.state, &player, COMMONS, &mv, at)
    }

    // The public MCP endpoint

    async fn rpc(&self, method: &str, params: Value) -> Reply {
        self.post(
            "/public/mcp",
            json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }),
        )
        .send()
        .await
    }

    async fn call(&self, name: &str, arguments: Value) -> Value {
        self.rpc("tools/call", json!({ "name": name, "arguments": arguments })).await.body["result"]
            .clone()
    }

    /// A tool that worked: its structured result.
    async fn tool(&self, name: &str, arguments: Value) -> Value {
        let result = self.call(name, arguments).await;
        assert!(result["isError"].is_null(), "{name} failed: {result}");
        result["structuredContent"].clone()
    }

    /// A tool that was refused: the message the model reads.
    async fn tool_refused(&self, name: &str, arguments: Value) -> String {
        let result = self.call(name, arguments).await;
        assert_eq!(result["isError"], true, "{name} should have been refused: {result}");
        result["content"][0]["text"].as_str().unwrap().to_string()
    }

    // The database, for what a test must set up or look at

    fn tiles(&self, world: &str) -> world::Tiles {
        self.state.db.world_tiles(world).unwrap()
    }

    fn player_row(&self, id: i64) -> agenthangout::db::PlayerRow {
        self.state.db.player(id).unwrap().unwrap()
    }
}

// The land and what's built, as a page reads them

struct Map {
    size: i64,
    ground: Vec<Vec<char>>,
    built: HashMap<(i64, i64), (String, i64)>,
    /// Tiles kept for side streets (W15): a zone or a park skips them, and a road may use them.
    kept: HashSet<(i64, i64)>,
    /// Tiles a long road may go on (W84, W85), worked out as the server does.
    on_streets: HashSet<(i64, i64)>,
}

/// The tiles a road longer than a lane may go on (W85), worked out as the server does.
fn street_tiles(seed: u64, size: i64, tiles: &world::Tiles) -> HashSet<(i64, i64)> {
    let hoods = world::neighborhoods(seed, size as usize);
    (0..size)
        .flat_map(|y| (0..size).map(move |x| (x, y)))
        .filter(|&(x, y)| world::on_street(&hoods, tiles, (x as u16, y as u16)))
        .collect()
}

/// The tiles a world keeps for side streets, worked out as the server does.
fn kept_tiles(size: i64, tiles: &world::Tiles) -> HashSet<(i64, i64)> {
    (0..size)
        .flat_map(|y| (0..size).map(move |x| (x, y)))
        .filter(|&(x, y)| world::is_reserved(tiles, (x as u16, y as u16)))
        .collect()
}

impl Map {
    fn from_view(view: &Value) -> Map {
        let rows: Vec<String> = view["land"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row.as_str().unwrap().to_string())
            .collect();
        let ground = rows
            .iter()
            .map(|row| {
                let mut cells = Vec::new();
                let mut count = 0usize;
                for ch in row.chars() {
                    if let Some(digit) = ch.to_digit(10) {
                        count = count * 10 + digit as usize;
                    } else {
                        cells.extend(std::iter::repeat_n(ch, count));
                        count = 0;
                    }
                }
                cells
            })
            .collect::<Vec<_>>();
        let built: HashMap<(i64, i64), (String, i64)> = view["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tile| {
                (
                    (tile[0].as_i64().unwrap(), tile[1].as_i64().unwrap()),
                    (tile[2].as_str().unwrap().to_string(), tile[3].as_i64().unwrap()),
                )
            })
            .collect();
        // A page can't tell who built a road, so every road is taken for a street. That is right
        // for a world nobody has built in yet.
        let streets: world::Tiles = built
            .iter()
            .filter(|(_, (kind, _))| kind == "road")
            .map(|(&(x, y), _)| {
                ((x as u16, y as u16), Tile { kind: Kind::Road, level: 0, player: None })
            })
            .collect();
        let size = ground.len() as i64;
        let seed = view["seed"].as_u64().unwrap();
        Map {
            size,
            ground,
            built,
            kept: kept_tiles(size, &streets),
            on_streets: street_tiles(seed, size, &streets),
        }
    }

    fn from_db(server: &Server, world: &str) -> Map {
        worlds::ensure_commons(&server.state).unwrap();
        let tiles = server.tiles(world);
        let row = server.state.db.world(world).unwrap().unwrap();
        let land = world::terrain(row.seed, row.size);
        let ground = (0..row.size)
            .map(|y| (0..row.size).map(|x| land.get(x, y).unwrap().letter()).collect())
            .collect();
        let built = tiles
            .iter()
            .map(|(&(x, y), tile)| {
                (
                    (i64::from(x), i64::from(y)),
                    (tile.kind.as_str().to_string(), i64::from(tile.level)),
                )
            })
            .collect();
        Map {
            size: row.size as i64,
            ground,
            built,
            kept: kept_tiles(row.size as i64, &tiles),
            on_streets: street_tiles(row.seed, row.size as i64, &tiles),
        }
    }

    fn ground(&self, x: i64, y: i64) -> Option<char> {
        if x < 0 || y < 0 || x >= self.size || y >= self.size {
            return None;
        }
        Some(self.ground[y as usize][x as usize])
    }

    /// Land that can be built on, with nothing built there.
    fn open(&self, x: i64, y: i64) -> bool {
        matches!(self.ground(x, y), Some('g' | 's')) && !self.built.contains_key(&(x, y))
    }

    /// Open land that a zone or a park can take: not a side street.
    fn free(&self, x: i64, y: i64) -> bool {
        self.open(x, y) && !self.kept.contains(&(x, y))
    }

    fn roads(&self) -> Vec<(i64, i64)> {
        let mut roads: Vec<_> = self
            .built
            .iter()
            .filter(|(_, (kind, _))| kind == "road")
            .map(|(pos, _)| *pos)
            .collect();
        roads.sort();
        roads
    }

    fn find(&self, ground: char) -> Option<(i64, i64)> {
        (0..self.size)
            .flat_map(|y| (0..self.size).map(move |x| (x, y)))
            .find(|&(x, y)| self.ground(x, y) == Some(ground) && !self.built.contains_key(&(x, y)))
    }

    /// A straight run of `n` open tiles that starts beside a road: (first, last).
    fn line_from_road(&self, n: i64) -> Option<((i64, i64), (i64, i64))> {
        for (rx, ry) in self.roads() {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                // W85: longer than a lane, it follows the streets.
                let along = |i: i64| {
                    n <= world::LANE_TILES as i64
                        || self.on_streets.contains(&(rx + dx * i, ry + dy * i))
                };
                if (1..=n).all(|i| self.open(rx + dx * i, ry + dy * i) && along(i)) {
                    return Some(((rx + dx, ry + dy), (rx + dx * n, ry + dy * n)));
                }
            }
        }
        None
    }
}

/// What a new Commons holds before anyone has played: the highway and nothing else (W28, W46).
fn the_highway() -> HashSet<(u16, u16)> {
    let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
    world::highway(world::COMMONS_SEED, &land).into_iter().collect()
}

/// Puts up places on The Commons, all one player's, until it is a town.
fn make_a_town(server: &Server) {
    worlds::ensure_commons(&server.state).unwrap();
    let builder = spare_guest(server, 999, now()).id as u64;
    let houses: Vec<Change> = (47..=49u16)
        .flat_map(|x| (26..=45u16).map(move |y| (x, y)))
        .map(|pos| Change::Set(pos, Tile { kind: Kind::Cafe, level: 0, player: Some(builder) }))
        .collect();
    server.write(COMMONS, &houses);
    assert_eq!(world::stats(&server.tiles(COMMONS)).stage, "town");
}

fn road_move(first: (i64, i64), last: (i64, i64)) -> Value {
    json!({ "action": "road", "x": first.0, "y": first.1, "x2": last.0, "y2": last.1 })
}

/// Every name a player or a visitor can have.
fn all_names() -> HashSet<String> {
    (0..40_000u64).flat_map(|n| [world::player_name(n), world::visitor_name(n)]).collect()
}

/// A guest's own move value, for calling the rules directly.
fn mv(action: &str, x: Option<i64>, y: Option<i64>) -> Move {
    Move { action: action.to_string(), x, y, x2: None, y2: None, kind: None, emote: None }
}

fn emote(word: &str) -> Move {
    Move {
        action: "emote".to_string(),
        x: None,
        y: None,
        x2: None,
        y2: None,
        kind: None,
        emote: Some(word.to_string()),
    }
}

// ----------------------------------------------------------------------------------------------
// Keeping it separate (W1-W5)
// ----------------------------------------------------------------------------------------------

// W2
#[tokio::test]
async fn a_guest_token_is_an_unknown_token_on_every_route_but_a_worlds() {
    let server = Server::start().await;
    let guest = server.guest().await;
    for (method, path) in [
        (Method::GET, "/api/me"),
        (Method::GET, "/api/inbox"),
        (Method::GET, "/api/hangouts/abc"),
        (Method::POST, "/api/hangouts/abc/messages"),
        (Method::GET, "/api/cities/abc"),
        (Method::GET, "/api/houses/abc"),
        (Method::POST, "/api/trips"),
        (Method::POST, "/api/status"),
        (Method::PUT, "/api/notify"),
        (Method::GET, "/api/passkeys"),
        (Method::POST, "/api/key/rotate"),
        (Method::GET, "/api/house/queue"),
    ] {
        let reply =
            server.req(method.clone(), path).token(&guest.token).json(json!({})).send().await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{method} {path}: {}", reply.show());
    }
    // The private MCP endpoint doesn't know it either.
    let reply = server
        .post("/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .token(&guest.token)
        .send()
        .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    // And as a session cookie it is nothing either.
    let reply = server.get("/api/me").cookie(&format!("ah_session={}", guest.token)).send().await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    // On a world route it works.
    assert_eq!(server.me(&guest.token, COMMONS).await["moves_left"], 30);
}

// W2
#[tokio::test]
async fn a_guest_token_never_authenticates_where_the_hash_of_a_key_would() {
    let server = Server::start().await;
    let guest = server.guest().await;
    // Only its hash is stored, and nothing but players looks at that table.
    let hash = auth::hash_secret(&guest.token);
    assert!(server.state.db.player_by_token(&hash).unwrap().is_some());
    assert!(
        server.state.db.player_by_token(&guest.token).unwrap().is_none(),
        "the token itself is not stored"
    );
    assert!(server.state.authenticate(&hash).unwrap().is_none());
}

// W3
#[tokio::test]
async fn a_bot_that_stays_home_and_a_house_bot_are_refused_and_nothing_is_made() {
    let server = Server::start().await;
    let made =
        server.post("/api/cities", json!({ "name": "Elm", "your_name": "Ann" })).send().await.ok();
    let ann = made["owner_key"].as_str().unwrap().to_string();
    let city = made["city"]["id"].as_str().unwrap().to_string();
    let house = server
        .post(&format!("/api/cities/{city}/houses"), json!({ "name": "Ann House" }))
        .token(&ann)
        .send()
        .await
        .ok();
    let house_id = house["house"]["id"].as_str().unwrap().to_string();
    let living = house["living_room_id"].as_str().unwrap().to_string();

    let bot = server
        .post(&format!("/api/hangouts/{living}/agents"), json!({ "name": "Homebody" }))
        .token(&ann)
        .send()
        .await
        .ok();
    let stay_token = bot["token"].as_str().unwrap().to_string();
    let stay_id = bot["agent"]["id"].as_str().unwrap().to_string();
    server
        .req(Method::PUT, &format!("/api/agents/{stay_id}/home"))
        .json(json!({ "house_id": house_id, "stays_home": true }))
        .token(&ann)
        .send()
        .await
        .ok();
    let casa = server
        .post(&format!("/api/houses/{house_id}/house-bot"), json!({ "name": "Casa" }))
        .token(&ann)
        .send()
        .await
        .ok();
    let house_token = casa["token"].as_str().unwrap().to_string();

    for token in [&stay_token, &house_token] {
        let reply = server.post("/api/public/join", json!({})).token(token).send().await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.show());
        assert!(!reply.text.contains("player_token"));
    }
    // Nothing was made: not a player, not a place on the street, not a claim.
    assert!(server.state.db.players_of(made["person"]["id"].as_str().unwrap()).unwrap().is_empty());
    assert_eq!(server.view(COMMONS).await["players"], json!([]));
    // The same through MCP.
    let reply = server.rpc("tools/call", json!({ "name": "join", "arguments": {} })).await;
    assert_eq!(reply.body["result"]["isError"], Value::Null, "a guest may join");
    let text = {
        let reply = server
            .post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "join", "arguments": {} } }))
            .token(&stay_token)
            .send()
            .await;
        reply.body["result"]["content"][0]["text"].as_str().unwrap().to_string()
    };
    assert!(text.contains("stays home"), "{text}");
    assert!(server.state.db.players_of(made["person"]["id"].as_str().unwrap()).unwrap().is_empty());
}

// W3, W9
#[tokio::test]
async fn an_ordinary_bot_token_links_a_player_with_200_moves_and_a_new_token_each_time() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let room = server
        .post("/api/hangouts", json!({ "name": "Crew", "your_name": "Bo" }))
        .send()
        .await
        .ok();
    let bo_key = room["owner_key"].as_str().unwrap().to_string();
    let room_id = room["hangout"]["id"].as_str().unwrap().to_string();
    let bot = server
        .post(&format!("/api/hangouts/{room_id}/agents"), json!({ "name": "Scout" }))
        .token(&bo_key)
        .send()
        .await
        .ok();
    let bot_token = bot["token"].as_str().unwrap().to_string();
    let agent_id = bot["agent"]["id"].as_str().unwrap().to_string();
    let seen_before = server.state.db.agent(&agent_id, now()).unwrap().unwrap().last_seen_at;

    let first = server.post("/api/public/join", json!({})).token(&bot_token).send().await.ok();
    assert_eq!(first["moves_per_day"], 200);
    assert_eq!(first["moves_left"], 200);
    assert!(first["claim_code"].is_null(), "a bot's player already belongs to its owner");
    let token = first["player_token"].as_str().unwrap().to_string();
    assert!(token.starts_with("ah_guest_"));
    assert_eq!(server.moves_left(&token).await, 200);

    // Joining again gives a new token for the same player, and the old one stops.
    let second = server.post("/api/public/join", json!({})).token(&bot_token).send().await.ok();
    let new_token = second["player_token"].as_str().unwrap().to_string();
    assert_ne!(token, new_token);
    assert_eq!(second["name"], first["name"], "the same player");
    assert_eq!(
        server.get("/api/worlds/commons/me").token(&token).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(server.moves_left(&new_token).await, 200);

    // W5: it didn't count as the bot being seen in a room.
    assert_eq!(server.state.db.agent(&agent_id, now()).unwrap().unwrap().last_seen_at, seen_before);
    // The bot's owner can see it as theirs and nobody else can.
    let mine = server.get("/api/players/mine").token(&bo_key).send().await.ok();
    assert_eq!(mine["players"].as_array().unwrap().len(), 1);
    let theirs = server.get("/api/players/mine").token(&ann.key).send().await.ok();
    assert_eq!(theirs["players"], json!([]));
}

// W2
#[tokio::test]
async fn a_bot_token_is_not_a_player_token() {
    let server = Server::start().await;
    let room = server
        .post("/api/hangouts", json!({ "name": "Crew", "your_name": "Bo" }))
        .send()
        .await
        .ok();
    let key = room["owner_key"].as_str().unwrap().to_string();
    let bot = server
        .post(
            &format!("/api/hangouts/{}/agents", room["hangout"]["id"].as_str().unwrap()),
            json!({ "name": "Scout" }),
        )
        .token(&key)
        .send()
        .await
        .ok();
    let bot_token = bot["token"].as_str().unwrap();
    let reply = server.act(bot_token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    let reply = server.get("/api/worlds/commons/me").token(bot_token).send().await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

// W4, W8
#[tokio::test]
async fn nothing_public_links_a_player_to_a_person_or_a_bot() {
    let server = Server::start().await;
    let room = server
        .post("/api/hangouts", json!({ "name": "SecretCrewName", "your_name": "SecretPersonName" }))
        .send()
        .await
        .ok();
    let key = room["owner_key"].as_str().unwrap().to_string();
    let person_id = room["person"]["id"].as_str().unwrap().to_string();
    let bot = server
        .post(
            &format!("/api/hangouts/{}/agents", room["hangout"]["id"].as_str().unwrap()),
            json!({ "name": "SecretBotName" }),
        )
        .token(&key)
        .send()
        .await
        .ok();
    let bot_token = bot["token"].as_str().unwrap().to_string();
    let agent_id = bot["agent"]["id"].as_str().unwrap().to_string();

    let joined = server.post("/api/public/join", json!({})).token(&bot_token).send().await.ok();
    let token = joined["player_token"].as_str().unwrap().to_string();
    let guest = server.guest().await;
    let claimed = server
        .post("/api/players/claim", json!({ "claim_code": guest.claim }))
        .token(&key)
        .send()
        .await
        .ok();
    assert_eq!(claimed["player"]["kind"], "claimed");
    server.act(&token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "cheer" })).await.ok();
    // The person's own player as well.
    server.act(&key, COMMONS, json!({ "action": "emote", "emote": "dance" })).await.ok();

    let mut seen = vec![
        server.fresh_view(COMMONS).await.to_string(),
        server.get("/api/worlds").send().await.text,
        server.tool("look", json!({ "player_token": token })).await.to_string(),
        server.tool("how_am_i_doing", json!({ "player_token": token })).await.to_string(),
        server.me(&token, COMMONS).await.to_string(),
    ];
    seen.push(joined.to_string());
    for text in &seen {
        for secret in [
            "SecretPersonName",
            "SecretBotName",
            "SecretCrewName",
            person_id.as_str(),
            agent_id.as_str(),
        ] {
            assert!(!text.contains(secret), "{secret} leaked into {text}");
        }
    }
    // The street lists three players: id, name, place, emote and whether a person or an agent is
    // playing, which says nothing about whose it is, and nothing else about them.
    let view: Value = serde_json::from_str(&seen[0]).unwrap();
    let players = view["players"].as_array().unwrap();
    assert_eq!(players.len(), 3);
    for player in players {
        assert_eq!(player.as_array().unwrap().len(), 6, "{player}");
        assert!(["person", "agent"].contains(&player[5].as_str().unwrap()), "{player}");
    }
}

// W4
#[tokio::test]
async fn mine_is_the_only_link_and_only_the_person_sees_it() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let guest = server.guest().await;
    server
        .post("/api/players/claim", json!({ "claim_code": guest.claim }))
        .token(&ann.key)
        .send()
        .await
        .ok();
    let mine = server.get("/api/players/mine").token(&ann.key).send().await.ok();
    assert_eq!(mine["players"].as_array().unwrap().len(), 1);
    assert_eq!(mine["players"][0]["name"], guest.name);
    assert!(!mine.to_string().contains(&ann.id), "not even their own id is needed");
    assert_eq!(
        server.get("/api/players/mine").token(&bo.key).send().await.ok()["players"],
        json!([])
    );
    // A guest can't ask, and nobody who isn't signed in can.
    assert_eq!(
        server.get("/api/players/mine").token(&guest.token).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(server.get("/api/players/mine").send().await.status, StatusCode::UNAUTHORIZED);
}

// W5
#[tokio::test]
async fn playing_a_world_changes_nothing_in_a_private_city() {
    let server = Server::start().await;
    let made =
        server.post("/api/cities", json!({ "name": "Elm", "your_name": "Ann" })).send().await.ok();
    let ann = made["owner_key"].as_str().unwrap().to_string();
    let city = made["city"]["id"].as_str().unwrap().to_string();
    let person = made["person"]["id"].as_str().unwrap().to_string();
    let before = server
        .get(&format!("/api/cities/{city}/people/{person}/standing"))
        .token(&ann)
        .send()
        .await;
    let me_before = server.get("/api/me").token(&ann).send().await.body;

    for _ in 0..3 {
        server.act(&ann, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    }
    let map = Map::from_db(&server, COMMONS);
    let spot = building_spots(&map, 1)[0];
    server.act(&ann, COMMONS, place("house", spot)).await.ok();

    let after = server
        .get(&format!("/api/cities/{city}/people/{person}/standing"))
        .token(&ann)
        .send()
        .await;
    assert_eq!(before.body, after.body);
    assert_eq!(before.status, after.status);
    assert_eq!(me_before, server.get("/api/me").token(&ann).send().await.body);
}

// ----------------------------------------------------------------------------------------------
// No text (W6-W8)
// ----------------------------------------------------------------------------------------------

// W6, W8
#[tokio::test]
async fn unknown_fields_and_unlisted_words_are_refused_and_never_echoed() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let bad = [
        json!({ "action": "emote", "emote": "wave", "zzz-secret-field": 1 }),
        json!({ "action": "zzz-secret-action" }),
        json!({ "action": "place", "kind": "zzz-secret-kind", "x": 1, "y": 1 }),
        json!({ "action": "place", "kind": "road", "x": 1, "y": 1 }),
        json!({ "action": "place", "kind": "homes", "x": 1, "y": 1 }),
        json!({ "action": "place", "kind": "cleared", "x": 1, "y": 1 }),
        json!({ "action": "place", "x": 1, "y": 1 }),
        json!({ "action": "zone", "kind": "homes", "x": 1, "y": 1 }),
        json!({ "action": "park", "x": 1, "y": 1 }),
        json!({ "action": "emote", "emote": "zzz-secret-emote" }),
        json!({ "action": "emote", "emote": "Wave" }),
        json!({ "action": "emote" }),
        json!({ "action": "road", "x": "zzz-secret-number", "y": 1 }),
        json!({ "action": "road", "x": 1.5, "y": 1 }),
        json!({ "action": "road", "x": 1e30, "y": 1 }),
        json!({ "action": "road", "x": [1], "y": 1 }),
        json!({ "action": "road", "x": 5, "y": 5, "x2": 6 }),
        json!({ "action": "road", "x": 5, "y": 5, "kind": "homes" }),
        json!({ "action": "place", "kind": "house", "x": 5, "y": 5, "emote": "wave" }),
        json!({ "action": "place", "kind": "house", "x": 5, "y": 5, "x2": 6, "y2": 5 }),
        json!({ "action": "walk", "x": 5, "y": 5, "x2": 6, "y2": 6 }),
        json!({ "action": "emote", "emote": "wave", "x": 3 }),
        json!({ "action": 5 }),
        json!({}),
        json!([]),
        json!("zzz-secret-string"),
    ];
    for body in bad {
        let reply = server.act(&guest.token, COMMONS, body.clone()).await;
        assert!(reply.status.is_client_error(), "{body} gave {}", reply.show());
        assert!(!reply.text.contains("zzz-secret"), "{body} was echoed: {}", reply.text);
        assert!(!reply.text.contains("secret-"), "{body} was echoed: {}", reply.text);
    }
    // A number too big for any integer, written out as text, and not JSON at all, or the wrong
    // kind of body.
    let reply = server
        .req(Method::POST, "/api/worlds/commons/act")
        .token(&guest.token)
        .raw("application/json", br#"{"action":"road","x":99999999999999999999999,"y":1}"#)
        .send()
        .await;
    assert!(reply.status.is_client_error() && !reply.text.contains("9999999"), "{}", reply.show());
    let reply = server
        .req(Method::POST, "/api/worlds/commons/act")
        .token(&guest.token)
        .raw("application/json", b"{zzz-secret")
        .send()
        .await;
    assert!(
        reply.status.is_client_error() && !reply.text.contains("zzz-secret"),
        "{}",
        reply.show()
    );
    let reply = server
        .req(Method::POST, "/api/worlds/commons/act")
        .token(&guest.token)
        .raw("text/plain", b"zzz-secret")
        .send()
        .await;
    assert!(
        reply.status.is_client_error() && !reply.text.contains("zzz-secret"),
        "{}",
        reply.show()
    );
    // Nothing was spent by any of it.
    assert_eq!(server.moves_left(&guest.token).await, 30);
}

// W6
#[tokio::test]
async fn bodies_of_other_world_routes_are_just_as_strict() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let reply = server
        .post("/api/worlds", json!({ "seed": 5, "zzz-secret-field": 1 }))
        .token(&ann.key)
        .send()
        .await;
    assert!(
        reply.status.is_client_error() && !reply.text.contains("zzz-secret"),
        "{}",
        reply.show()
    );
    let reply =
        server.post("/api/worlds", json!({ "seed": "zzz-secret" })).token(&ann.key).send().await;
    assert!(
        reply.status.is_client_error() && !reply.text.contains("zzz-secret"),
        "{}",
        reply.show()
    );
    assert!(
        server.state.db.world_of_owner(&ann.id).unwrap().is_none(),
        "none of those made a world"
    );
    let reply = server
        .post("/api/players/claim", json!({ "claim_code": "x", "zzz-secret": 1 }))
        .token(&ann.key)
        .send()
        .await;
    assert!(reply.status.is_client_error() && !reply.text.contains("zzz-secret"));
    let reply = server
        .post("/api/public/join", json!({ "world_id": "commons", "zzz-secret": 1 }))
        .send()
        .await;
    assert!(reply.status.is_client_error() && !reply.text.contains("zzz-secret"));
}

// W6, W8
#[tokio::test]
async fn the_public_mcp_endpoint_refuses_what_it_does_not_list_without_echoing_it() {
    let server = Server::start().await;
    let token = server.tool("join", json!({})).await["player_token"].as_str().unwrap().to_string();
    for (name, args) in [
        ("build", json!({ "player_token": token, "type": "zzz-secret-type", "x": 1, "y": 1 })),
        ("build", json!({ "player_token": token, "type": "homes", "x": 1, "y": 1 })),
        ("build", json!({ "player_token": token, "type": "house", "x": 1, "y": 1 })),
        (
            "build",
            json!({ "player_token": token, "type": "road", "x": 1, "y": 1, "zzz-secret": 1 }),
        ),
        ("build", json!({ "player_token": token, "type": "road", "x": "zzz-secret", "y": 1 })),
        ("build", json!({ "player_token": token, "type": "road" })),
        ("emote", json!({ "player_token": token, "emote": "zzz-secret" })),
        ("walk", json!({ "player_token": token, "x": 1.5, "y": 1 })),
        ("look", json!({ "player_token": token, "x": 1 })),
        ("look", json!({ "zzz-secret": 1 })),
        ("join", json!({ "zzz-secret": 1 })),
        ("how_am_i_doing", json!({ "player_token": token, "zzz-secret": 1 })),
        ("zzz-secret-tool", json!({})),
        ("say", json!({ "text": "hi" })),
        ("inbox", json!({})),
        ("list_city", json!({})),
    ] {
        let message = server.tool_refused(name, args).await;
        assert!(!message.contains("zzz-secret"), "{name} echoed: {message}");
    }
    // Neither does a bad request as a whole.
    let reply = server.rpc("zzz-secret-method", json!({})).await;
    assert!(!reply.text.contains("zzz-secret"), "{}", reply.text);
    let reply = server
        .req(Method::POST, "/public/mcp")
        .raw("application/json", b"{zzz-secret")
        .send()
        .await;
    assert!(!reply.text.contains("zzz-secret"), "{}", reply.text);
    assert_eq!(server.moves_left(&token).await, 30);
}

// W7
#[tokio::test]
async fn every_name_comes_from_the_lists_and_a_reroll_cannot_be_typed() {
    let server = Server::start().await;
    let names = all_names();
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    assert!(names.contains(&guest.name), "{}", guest.name);
    let id = server.player_id(&guest.token).await;
    // W4: the name comes from a seed that started random, not from the public number, so a name
    // that is later rerolled doesn't stand out.
    let seed = server.player_row(id).name_seed;
    assert_eq!(guest.name, world::player_name(seed as u64), "a player's name is their seed's");
    assert_ne!(seed, id, "a player's seed is not their number");

    server
        .post("/api/players/claim", json!({ "claim_code": guest.claim }))
        .token(&ann.key)
        .send()
        .await
        .ok();
    let mut changed = false;
    for _ in 0..5 {
        let reply = server
            .post(&format!("/api/players/{id}/reroll"), json!({ "name": "Typed Name" }))
            .token(&ann.key)
            .send()
            .await
            .ok();
        let name = reply["name"].as_str().unwrap().to_string();
        assert!(names.contains(&name), "{name}");
        assert_ne!(name, "Typed Name");
        changed |= name != guest.name;
        assert_eq!(server.me(&guest.token, COMMONS).await["player"]["name"], name);
    }
    assert!(changed, "a reroll picks another name");
    // Worlds: The Commons is called that, and a world of someone's own gets a name from its seed.
    let view = server.view(COMMONS).await;
    assert_eq!(view["name"], "The Commons");
    let (own, _) = server.own_world_of(&ann, true, 4242);
    assert_eq!(server.view(&own).await["name"], world::world_name(4242));
}

// W4, W7
#[tokio::test]
async fn only_the_person_who_claimed_a_player_can_reroll_it() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let guest = server.guest().await;
    let id = server.player_id(&guest.token).await;
    // Unclaimed: nobody's to reroll.
    assert_eq!(
        server
            .post(&format!("/api/players/{id}/reroll"), json!({}))
            .token(&ann.key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    server
        .post("/api/players/claim", json!({ "claim_code": guest.claim }))
        .token(&ann.key)
        .send()
        .await
        .ok();
    assert_eq!(
        server
            .post(&format!("/api/players/{id}/reroll"), json!({}))
            .token(&bo.key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .post(&format!("/api/players/{id}/reroll"), json!({}))
            .token(&guest.token)
            .send()
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server.post("/api/players/zzz/reroll", json!({})).token(&ann.key).send().await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .post(&format!("/api/players/{id}/reroll"), json!({}))
            .token(&ann.key)
            .send()
            .await
            .status,
        StatusCode::OK
    );
}

// W8
#[tokio::test]
async fn everything_a_world_returns_is_numbers_and_listed_words() {
    let server = Server::start().await;
    let names = all_names();
    let kinds: HashSet<&str> = [
        "road",
        "homes",
        "shops",
        "workshops",
        "park",
        "cleared",
        "house",
        "cafe",
        "club",
        "studio",
    ]
    .into();
    let emotes: HashSet<&str> = ["wave", "cheer", "think", "sleep", "dance"].into();
    let stages: HashSet<&str> = ["hamlet", "village", "town", "city"].into();
    let crawler_ids: HashSet<&str> = world::CRAWLERS.iter().map(|crawler| crawler.id).collect();
    let crawler_names: HashSet<&str> = world::CRAWLERS.iter().map(|crawler| crawler.name).collect();

    // A place on the map as well, so every kind a tile can be is looked at.
    let ann = server.person("Ann").await;
    let spot = building_spots(&Map::from_db(&server, COMMONS), 1)[0];
    server.act(&ann.key, COMMONS, place("house", spot)).await.ok();
    let guest = server.guest().await;
    server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "dance" })).await.ok();
    server.get("/").ua(GOOGLEBOT).send().await;
    server.get("/").ua(CHATGPT_USER).send().await;
    server.get("/").ua(GPTBOT).send().await;
    let view = server.fresh_view(COMMONS).await;
    assert_eq!(view["crawlers"].as_array().unwrap().len(), 2);
    assert_eq!(view["visitors"].as_array().unwrap().len(), 1);

    let is_row = |text: &str| {
        !text.is_empty() && text.chars().all(|c| c.is_ascii_digit() || "gfwsh".contains(c))
    };
    for tile in view["tiles"].as_array().unwrap() {
        assert!(kinds.contains(tile[2].as_str().unwrap()));
    }
    for row in view["land"].as_array().unwrap() {
        assert!(is_row(row.as_str().unwrap()), "{row}");
    }
    for player in view["players"].as_array().unwrap() {
        assert!(names.contains(player[1].as_str().unwrap()));
        assert!(player[4].is_null() || emotes.contains(player[4].as_str().unwrap()));
    }
    for frog in view["crawlers"].as_array().unwrap() {
        assert!(
            crawler_ids.contains(frog[0].as_str().unwrap())
                && crawler_names.contains(frog[1].as_str().unwrap())
        );
    }
    for visitor in view["visitors"].as_array().unwrap() {
        assert!(names.contains(visitor[0].as_str().unwrap()));
    }
    assert!(stages.contains(view["stage"].as_str().unwrap()));
    assert_eq!(view["name"], "The Commons");
    assert_eq!(view["kind"], "public");

    // `look`, `how_am_i_doing` and `me` as well.
    let look = server.tool("look", json!({ "player_token": guest.token })).await;
    for row in look["land"].as_array().unwrap() {
        assert!(row.as_str().unwrap().chars().all(|c| "gfwsh".contains(c)));
    }
    for tile in look["built"].as_array().unwrap() {
        assert!(kinds.contains(tile[2].as_str().unwrap()));
    }
    for player in look["players"].as_array().unwrap() {
        assert!(names.contains(player[1].as_str().unwrap()));
    }
    assert!(names.contains(look["you"]["name"].as_str().unwrap()));
    let how = server.tool("how_am_i_doing", json!({ "player_token": guest.token })).await;
    assert!(names.contains(how["player"]["name"].as_str().unwrap()));
    assert!(stages.contains(how["world"]["stage"].as_str().unwrap()));
}

// ----------------------------------------------------------------------------------------------
// Who can do what (W9-W12)
// ----------------------------------------------------------------------------------------------

// W9
#[tokio::test]
async fn a_guest_has_30_moves_and_a_person_300_and_a_claimed_guest_200() {
    let server = Server::start().await;
    let guest = server.guest().await;
    assert_eq!(server.me(&guest.token, COMMONS).await["moves_left"], 30);
    let ann = server.person("Ann").await;
    assert_eq!(server.me(&ann.key, COMMONS).await["moves_left"], 300);
    assert_eq!(server.me(&ann.key, COMMONS).await["moves_per_day"], 300);
    // A person signed in by cookie is the same person.
    assert_eq!(
        server.get("/api/worlds/commons/me").cookie(&ann.cookie).send().await.ok()["moves_left"],
        300
    );
    server
        .post("/api/players/claim", json!({ "claim_code": guest.claim }))
        .token(&ann.key)
        .send()
        .await
        .ok();
    assert_eq!(server.me(&guest.token, COMMONS).await["moves_left"], 200);
    // Looking is free, so none of it cost anything.
    server.get("/api/worlds").send().await.ok();
    server.view(COMMONS).await;
    server.tool("look", json!({})).await;
    assert_eq!(server.me(&guest.token, COMMONS).await["moves_left"], 200);
}

// W9, W13
#[tokio::test]
async fn the_thirty_first_move_of_a_day_is_refused_and_midnight_utc_brings_them_back() {
    let server = Server::start().await;
    let guest = server.guest().await;
    for n in 0..30 {
        let reply =
            server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
        assert_eq!(reply.status, StatusCode::OK, "move {n}: {}", reply.show());
    }
    let reply =
        server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS, "{}", reply.show());
    assert_eq!(server.moves_left(&guest.token).await, 0);

    // The same player, a day later, by the rules directly.
    let player = server.player_row(server.player_id(&guest.token).await);
    let tomorrow = now() + 86_400;
    assert_eq!(worlds::moves_left(&player, tomorrow), 30, "the day number changed");
    let acted = worlds::act(&server.state, &player, COMMONS, &emote("cheer"), tomorrow).unwrap();
    assert_eq!(acted.moves_left, 29);
    let player = server.player_row(player.id);
    assert_eq!(worlds::moves_left(&player, tomorrow), 29);
}

// W30
#[tokio::test]
async fn a_player_with_no_moves_left_hears_so_before_their_move_is_looked_at() {
    let server = Server::start().await;
    let guest = server.guest().await;
    for n in 0..30 {
        let reply =
            server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
        assert_eq!(reply.status, StatusCode::OK, "move {n}: {}", reply.show());
    }
    // There's nothing here to take down, which a player with moves left is told.
    let empty = json!({ "action": "bulldoze", "x": 0, "y": 0 });
    let reply = server.act(&guest.token, COMMONS, empty.clone()).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS, "{}", reply.show());
    assert!(reply.text.contains("no moves left"), "{}", reply.show());
    assert!(reply.headers.contains_key("retry-after"), "{}", reply.show());

    let other = server.guest().await;
    assert_eq!(server.act(&other.token, COMMONS, empty).await.status, StatusCode::BAD_REQUEST);
}

// W9
#[tokio::test]
async fn allowances_cannot_be_bought_traded_or_given_away() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    let other = server.guest().await;
    // No route or field moves moves between players, and a claimed code works for one person once.
    for body in [
        json!({ "action": "emote", "emote": "wave", "moves": 100 }),
        json!({ "action": "emote", "emote": "wave", "to": 1 }),
    ] {
        assert!(server.act(&guest.token, COMMONS, body).await.status.is_client_error());
    }
    server
        .post("/api/players/claim", json!({ "claim_code": guest.claim }))
        .token(&ann.key)
        .send()
        .await
        .ok();
    assert_eq!(
        server
            .post("/api/players/claim", json!({ "claim_code": other.claim }))
            .token(&ann.key)
            .send()
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(server.moves_left(&other.token).await, 200);
    assert_eq!(
        server
            .post("/api/players/claim", json!({ "claim_code": guest.claim }))
            .token(&ann.key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

// W10
#[tokio::test]
async fn a_guest_token_is_stored_as_a_hash_and_stops_after_seven_idle_days() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let id = server.player_id(&guest.token).await;
    let map = Map::from_db(&server, COMMONS);
    let (first, last) = map.line_from_road(2).unwrap();
    server.act(&guest.token, COMMONS, road_move(first, last)).await.ok();
    assert_eq!(
        server.tiles(COMMONS).values().filter(|tile| tile.player == Some(id as u64)).count(),
        2
    );

    // Seven days and a bit with nobody using it.
    let player = server.player_row(id);
    let later = player.seen_at + 7 * 86_400 + 60;
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| tx.charge(id, 0, 0, player.seen_at))
        .unwrap();
    assert!(player.seen_at <= later);
    // The HTTP calls use the real clock, so move the guest's last sighting back instead.
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| tx.charge(id, 0, 0, now() - 7 * 86_400 - 60))
        .unwrap();
    let reply = server.get("/api/worlds/commons/me").token(&guest.token).send().await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.show());
    let reply =
        server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    // What it built stays.
    assert_eq!(
        server.tiles(COMMONS).values().filter(|tile| tile.player == Some(id as u64)).count(),
        2
    );

    // One used just under a week ago still works, and using it keeps it going.
    let other = server.guest().await;
    let other_id = server.player_id(&other.token).await;
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| tx.charge(other_id, 0, 0, now() - 6 * 86_400))
        .unwrap();
    assert_eq!(
        server.get("/api/worlds/commons/me").token(&other.token).send().await.status,
        StatusCode::OK
    );
    assert!(server.player_row(other_id).seen_at >= now() - 5, "using it was noted");
}

// W10
#[tokio::test]
async fn a_claimed_guest_does_not_expire_for_being_idle() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    let id = server.player_id(&guest.token).await;
    server
        .post("/api/players/claim", json!({ "claim_code": guest.claim }))
        .token(&ann.key)
        .send()
        .await
        .ok();
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| tx.charge(id, 0, 0, now() - 30 * 86_400))
        .unwrap();
    assert_eq!(
        server.get("/api/worlds/commons/me").token(&guest.token).send().await.status,
        StatusCode::OK
    );
}

// W11
#[tokio::test]
async fn a_claim_code_works_once_and_a_person_can_claim_twenty() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let mut guests = Vec::new();
    for _ in 0..21 {
        guests.push(server.guest().await);
    }
    // A wrong code, a guest, and nobody signed in can't claim.
    assert_eq!(
        server
            .post("/api/players/claim", json!({ "claim_code": "not-a-code" }))
            .token(&ann.key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .post("/api/players/claim", json!({ "claim_code": guests[0].claim }))
            .token(&guests[1].token)
            .send()
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .post("/api/players/claim", json!({ "claim_code": guests[0].claim }))
            .send()
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    // The token is not the code.
    assert_eq!(
        server
            .post("/api/players/claim", json!({ "claim_code": guests[0].token }))
            .token(&ann.key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    for guest in &guests[..20] {
        let reply = server
            .post("/api/players/claim", json!({ "claim_code": guest.claim }))
            .token(&ann.key)
            .send()
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.show());
    }
    let reply = server
        .post("/api/players/claim", json!({ "claim_code": guests[20].claim }))
        .token(&ann.key)
        .send()
        .await;
    assert_eq!(reply.status, StatusCode::CONFLICT, "{}", reply.show());
    // The code that was refused for being too many still works for someone with room.
    assert_eq!(
        server
            .post("/api/players/claim", json!({ "claim_code": guests[20].claim }))
            .token(&bo.key)
            .send()
            .await
            .status,
        StatusCode::OK
    );
    // And a used code is dead, for anyone.
    assert_eq!(
        server
            .post("/api/players/claim", json!({ "claim_code": guests[0].claim }))
            .token(&bo.key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .post("/api/players/claim", json!({ "claim_code": guests[0].claim }))
            .token(&ann.key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server.get("/api/players/mine").token(&ann.key).send().await.ok()["players"]
            .as_array()
            .unwrap()
            .len(),
        20
    );
}

// W11
#[tokio::test]
async fn a_claim_code_is_kept_only_as_a_hash_and_a_bot_player_has_none() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let ann = server.person("Ann").await;
    assert_eq!(guest.claim.len(), 20);
    assert_ne!(guest.claim, guest.token);
    // The claim code never works as a player token.
    assert_eq!(
        server.get("/api/worlds/commons/me").token(&guest.claim).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    let _ = ann;
}

// W12
#[tokio::test]
async fn new_guests_are_capped_per_address_and_across_the_server() {
    let server = Server::start().await;
    for n in 0..5 {
        let reply = server.post("/api/public/join", json!({})).ip("203.0.113.9").send().await;
        assert_eq!(reply.status, StatusCode::OK, "guest {n}: {}", reply.show());
    }
    let reply = server.post("/api/public/join", json!({})).ip("203.0.113.9").send().await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS, "{}", reply.show());
    assert!(!reply.text.contains("player_token"));
    // Another address is a different allowance, and the last address in the header is the one counted.
    let reply =
        server.post("/api/public/join", json!({})).ip("198.51.100.1, 203.0.113.10").send().await;
    assert_eq!(reply.status, StatusCode::OK);
    let reply =
        server.post("/api/public/join", json!({})).ip("203.0.113.99, 203.0.113.9").send().await;
    assert_eq!(
        reply.status,
        StatusCode::TOO_MANY_REQUESTS,
        "the client's own words can't change the address"
    );
    // The same through MCP.
    let reply = server.post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "join", "arguments": {} } })).ip("203.0.113.9").send().await;
    assert_eq!(reply.body["result"]["isError"], true);
    // No address header is a caller on this machine, who has no per-address allowance.
    for _ in 0..8 {
        server.post("/api/public/join", json!({})).send().await.ok();
    }
}

// W12
#[tokio::test]
async fn new_guests_are_capped_at_600_an_hour_across_the_whole_server() {
    start_early_in(3600, 60).await;
    let server = Server::start().await;
    for n in 0..600 {
        let ip = server.fresh_ip();
        let reply = server.post("/api/public/join", json!({})).ip(&ip).send().await;
        assert_eq!(reply.status, StatusCode::OK, "guest {n}: {}", reply.show());
    }
    let ip = server.fresh_ip();
    let reply = server.post("/api/public/join", json!({})).ip(&ip).send().await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS, "{}", reply.show());
    // A bot with its own token is not a new guest, so the cap doesn't stop it.
    let room = server
        .post("/api/hangouts", json!({ "name": "Crew", "your_name": "Bo" }))
        .send()
        .await
        .ok();
    let bot = server
        .post(
            &format!("/api/hangouts/{}/agents", room["hangout"]["id"].as_str().unwrap()),
            json!({ "name": "Scout" }),
        )
        .token(room["owner_key"].as_str().unwrap())
        .send()
        .await
        .ok();
    let reply = server
        .post("/api/public/join", json!({}))
        .token(bot["token"].as_str().unwrap())
        .ip(&ip)
        .send()
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.show());
}

// ----------------------------------------------------------------------------------------------
// Building (W13-W18)
// ----------------------------------------------------------------------------------------------

// W13
#[tokio::test]
async fn each_move_costs_what_the_table_says() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let map = Map::from_db(&server, COMMONS);

    let (first, last) = map.line_from_road(3).unwrap();
    let road = server.act(&guest.token, COMMONS, road_move(first, last)).await.ok();
    assert_eq!(
        (road["cost"].as_i64(), road["changed"].as_i64(), road["moves_left"].as_i64()),
        (Some(3), Some(3), Some(27))
    );
    // The end of the road is nothing's way to the highway, so it can come down.
    let bulldoze = server
        .act(&guest.token, COMMONS, json!({ "action": "bulldoze", "x": last.0, "y": last.1 }))
        .await
        .ok();
    assert_eq!((bulldoze["cost"].as_i64(), bulldoze["moves_left"].as_i64()), (Some(1), Some(26)));
    let centre = map.size / 2;
    let walk = server
        .act(&guest.token, COMMONS, json!({ "action": "walk", "x": centre + 1, "y": centre }))
        .await
        .ok();
    assert_eq!(
        (walk["cost"].as_i64(), walk["changed"].as_i64(), walk["moves_left"].as_i64()),
        (Some(1), Some(0), Some(25))
    );
    let wave =
        server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    assert_eq!((wave["cost"].as_i64(), wave["moves_left"].as_i64()), (Some(1), Some(24)));
    assert_eq!(server.moves_left(&guest.token).await, 24);

    // A place is one move, from a person with a spot beside a road.
    let ann = server.person("Ann").await;
    let spot = building_spots(&Map::from_db(&server, COMMONS), 1)[0];
    let house = server.act(&ann.key, COMMONS, place("house", spot)).await.ok();
    assert_eq!((house["cost"].as_i64(), house["moves_left"].as_i64()), (Some(1), Some(299)));
}

// W13
#[tokio::test]
async fn a_refused_move_costs_nothing_and_changes_nothing() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let map = Map::from_db(&server, COMMONS);
    let before = server.tiles(COMMONS);
    let water = map.find('w');
    let mut refused = vec![
        json!({ "action": "road", "x": -1, "y": 5 }),
        json!({ "action": "road", "x": 500, "y": 5, "x2": 505, "y2": 5 }),
        json!({ "action": "road", "x": 0, "y": 0, "x2": 5, "y2": 5 }),
        json!({ "action": "road", "x": 3, "y": 3, "x2": 30, "y2": 3 }),
        json!({ "action": "road", "x": 0, "y": 0, "x2": 0, "y2": 10 }),
        json!({ "action": "walk", "x": 128, "y": 0 }),
        json!({ "action": "bulldoze", "x": 2, "y": 2 }),
        json!({ "action": "walk", "x": 1000, "y": 1000 }),
        json!({ "action": "road", "x": i64::MAX, "y": i64::MIN, "x2": i64::MAX, "y2": 0 }),
        json!({ "action": "road", "x": i64::MIN, "y": i64::MIN, "x2": i64::MAX, "y2": i64::MAX }),
    ];
    if let Some((x, y)) = water {
        refused.push(json!({ "action": "walk", "x": x, "y": y }));
        refused.push(json!({ "action": "road", "x": x, "y": y }));
        refused.push(json!({ "action": "bulldoze", "x": x, "y": y }));
    }
    for body in refused {
        let reply = server.act(&guest.token, COMMONS, body.clone()).await;
        assert!(reply.status.is_client_error(), "{body}: {}", reply.show());
    }
    assert_eq!(server.moves_left(&guest.token).await, 30);
    assert_eq!(server.tiles(COMMONS), before);
}

/// A tile of this ground with a road of nobody's put beside it, on a tile that is not kept for a
/// side street, so a road or a place there can only be turned away for the ground. Returns the tile.
fn beside_a_new_road(server: &Server, ground: char) -> (i64, i64) {
    let map = Map::from_db(server, COMMONS);
    let mut tiles = server.tiles(COMMONS);
    for y in 0..map.size {
        for x in 0..map.size {
            if map.ground(x, y) != Some(ground) || map.built.contains_key(&(x, y)) {
                continue;
            }
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = (x + dx, y + dy);
                if !map.open(nx, ny) {
                    continue;
                }
                let road = (nx as u16, ny as u16);
                let street = Tile { kind: Kind::Road, level: 0, player: None };
                tiles.insert(road, street);
                let kept = world::is_reserved(&tiles, (x as u16, y as u16));
                tiles.remove(&road);
                if !kept {
                    server.write(COMMONS, &[Change::Set(road, street)]);
                    return (x, y);
                }
            }
        }
    }
    panic!("no {ground} tile with room beside it");
}

// W14
#[tokio::test]
async fn only_open_land_can_be_built_on_and_forest_is_cleared_by_bulldozing() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let ann = server.person("Ann").await;
    let water = beside_a_new_road(&server, 'w');
    let hill = beside_a_new_road(&server, 'h');
    let forest = beside_a_new_road(&server, 'f');
    for (x, y) in [water, hill] {
        for body in [
            json!({ "action": "road", "x": x, "y": y }),
            place("house", (x, y)),
            json!({ "action": "bulldoze", "x": x, "y": y }),
        ] {
            let token = if body["action"] == "place" { &ann.key } else { &guest.token };
            assert!(
                server.act(token, COMMONS, body.clone()).await.status.is_client_error(),
                "{body}"
            );
        }
    }
    // Forest can't be built on, but one bulldoze clears it and then it can.
    let (x, y) = forest;
    assert!(server
        .act(&guest.token, COMMONS, json!({ "action": "road", "x": x, "y": y }))
        .await
        .status
        .is_client_error());
    assert!(server.act(&ann.key, COMMONS, place("house", forest)).await.status.is_client_error());
    let cleared = server
        .act(&guest.token, COMMONS, json!({ "action": "bulldoze", "x": x, "y": y }))
        .await
        .ok();
    assert_eq!(cleared["cost"], 1);
    let tile = server.tiles(COMMONS)[&(x as u16, y as u16)];
    assert_eq!(tile.kind, Kind::Cleared);
    assert!(
        server
            .act(&guest.token, COMMONS, json!({ "action": "bulldoze", "x": x, "y": y }))
            .await
            .status
            .is_client_error(),
        "already cleared"
    );
    assert_eq!(server.act(&ann.key, COMMONS, place("house", forest)).await.status, StatusCode::OK);
    assert_eq!(server.tiles(COMMONS)[&(x as u16, y as u16)].kind, Kind::House);
    // Bulldozing a place built on forest puts the clearing back.
    server.act(&ann.key, COMMONS, json!({ "action": "bulldoze", "x": x, "y": y })).await.ok();
    assert_eq!(server.tiles(COMMONS)[&(x as u16, y as u16)].kind, Kind::Cleared);
}

// W15
#[tokio::test]
async fn a_world_starts_with_a_highway_and_a_road_must_touch_a_road() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let view = server.view(COMMONS).await;
    let map = Map::from_view(&view);
    let highway = map.roads();
    assert!(highway.len() > 20, "a highway from the edge of the map to the middle");
    assert!(highway.contains(&(map.size / 2, map.size / 2)), "it ends in the middle");
    assert!(
        highway.iter().any(|&(x, y)| x == 0 || y == 0 || x == map.size - 1 || y == map.size - 1),
        "and starts at an edge"
    );
    // Nothing else is there yet: the highway and no founding town (W28).
    let shown = view["tiles"].as_array().unwrap();
    let expected = the_highway();
    assert_eq!(shown.len(), expected.len());
    for tile in shown {
        let pos = (tile[0].as_u64().unwrap() as u16, tile[1].as_u64().unwrap() as u16);
        assert!(expected.contains(&pos), "{pos:?} is not the highway");
        assert_eq!((tile[2].as_str().unwrap(), tile[3].as_u64().unwrap()), ("road", 0));
    }
    // A road away from every road is refused; one that starts beside the highway is let in.
    let mut used = HashSet::new();
    let far = {
        let roads = map.roads();
        (0..map.size)
            .flat_map(|y| (0..map.size).map(move |x| (x, y)))
            .find(|&(x, y)| {
                (0..3).all(|i| map.open(x + i, y))
                    && roads.iter().all(|&(rx, ry)| (rx - x).abs() > 4 || (ry - y).abs() > 4)
            })
            .unwrap()
    };
    let reply = server.act(&guest.token, COMMONS, road_move(far, (far.0 + 2, far.1))).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.show());
    let (first, last) = map.line_from_road(4).unwrap();
    server.act(&guest.token, COMMONS, road_move(first, last)).await.ok();
    used.insert(first);
    // Joined up now: a road that starts beside the new one, away from the highway, is let in too.
    let more = Map::from_db(&server, COMMONS);
    let beyond = more.line_from_road(2).unwrap();
    assert!(server
        .act(&guest.token, COMMONS, road_move(beyond.0, beyond.1))
        .await
        .status
        .is_success());
}

// W16
#[tokio::test]
async fn a_road_is_a_straight_line_of_up_to_16_and_a_drag_in_any_direction_works() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let ann = server.person("Ann").await;
    let map = Map::from_db(&server, COMMONS);

    // 17 tiles is too long, a diagonal is not a line.
    let long = map.line_from_road(17);
    if let Some((first, last)) = long {
        assert!(server
            .act(&ann.key, COMMONS, road_move(first, last))
            .await
            .status
            .is_client_error());
    }
    let (road_first, _) = map.line_from_road(1).unwrap();
    let diagonal = json!({ "action": "road", "x": road_first.0, "y": road_first.1, "x2": road_first.0 + 2, "y2": road_first.1 + 2 });
    assert!(server.act(&guest.token, COMMONS, diagonal).await.status.is_client_error());

    // Drawn from the far end back towards the road, it is the same road.
    let (first, last) = map.line_from_road(4).unwrap();
    let reversed = server.act(&guest.token, COMMONS, road_move(last, first)).await.ok();
    assert_eq!(reversed["changed"], 4);
    assert_eq!(server.moves_left(&guest.token).await, 26);
    // Over what is already there: nothing new, nothing charged, and the move is refused.
    let again = server.act(&guest.token, COMMONS, road_move(first, last)).await;
    assert!(again.status.is_client_error());
    assert_eq!(server.moves_left(&guest.token).await, 26);
}

// W16
#[tokio::test]
async fn a_road_over_road_skips_what_is_there_and_charges_only_for_the_rest() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let map = Map::from_db(&server, COMMONS);
    let (first, last) = map.line_from_road(5).unwrap();
    let (dx, dy) = ((last.0 - first.0) / 4, (last.1 - first.1) / 4);
    let part = (first.0 + dx * 2, first.1 + dy * 2);
    let reply = server.act(&guest.token, COMMONS, road_move(first, part)).await.ok();
    assert_eq!((reply["changed"].as_i64(), reply["cost"].as_i64()), (Some(3), Some(3)));
    assert_eq!(server.moves_left(&guest.token).await, 27);
    // The same road longer: what is road already is skipped and only what's new is charged.
    let longer = server.act(&guest.token, COMMONS, road_move(first, last)).await.ok();
    assert_eq!(
        (longer["changed"].as_i64(), longer["skipped"].as_i64(), longer["cost"].as_i64()),
        (Some(2), Some(3), Some(2))
    );
    assert_eq!(server.moves_left(&guest.token).await, 25);
    // Over only road: refused, nothing charged.
    let again = server.act(&guest.token, COMMONS, road_move(first, last)).await;
    assert!(again.status.is_client_error(), "{}", again.show());
    assert_eq!(server.moves_left(&guest.token).await, 25);
}

// W16
#[tokio::test]
async fn a_move_that_costs_more_than_you_have_is_refused_whole() {
    let server = Server::start().await;
    let guest = server.guest().await;
    for _ in 0..25 {
        server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    }
    assert_eq!(server.moves_left(&guest.token).await, 5);
    let map = Map::from_db(&server, COMMONS);
    let before = server.tiles(COMMONS);
    let (first, last) = map.line_from_road(6).unwrap();
    let reply = server.act(&guest.token, COMMONS, road_move(first, last)).await;
    assert_eq!(
        reply.status,
        StatusCode::TOO_MANY_REQUESTS,
        "6 tiles for 5 moves: {}",
        reply.show()
    );
    assert_eq!(server.moves_left(&guest.token).await, 5);
    assert_eq!(server.tiles(COMMONS), before, "none of it was built");
    let (first, last) = map.line_from_road(5).unwrap();
    let reply = server.act(&guest.token, COMMONS, road_move(first, last)).await;
    assert_eq!(reply.status, StatusCode::OK, "5 tiles for 5 moves is fine: {}", reply.show());
    assert_eq!(server.moves_left(&guest.token).await, 0);
    let after_five = server.tiles(COMMONS);
    assert_eq!(after_five.len(), before.len() + 5);
    let (first, last) = Map::from_db(&server, COMMONS).line_from_road(1).unwrap();
    let reply = server.act(&guest.token, COMMONS, road_move(first, last)).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(server.tiles(COMMONS), after_five);
}

// W17
#[tokio::test]
async fn you_can_bulldoze_only_what_you_built_until_its_builder_has_been_gone_a_week() {
    let server = Server::start().await;
    let ann = server.guest().await;
    let bo = server.guest().await;
    let ann_id = server.player_id(&ann.token).await;
    let map = Map::from_db(&server, COMMONS);
    let (first, last) = map.line_from_road(2).unwrap();
    server.act(&ann.token, COMMONS, road_move(first, last)).await.ok();
    let (x, y) = last;
    let bulldoze = json!({ "action": "bulldoze", "x": x, "y": y });

    // Not yours.
    let reply = server.act(&bo.token, COMMONS, bulldoze.clone()).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.show());
    assert_eq!(server.moves_left(&bo.token).await, 30);
    // The highway has no builder, so it is never anyone's to take down, and Ann can't either.
    let (hx, hy) = *map.roads().first().unwrap();
    for token in [&ann.token, &bo.token] {
        assert!(server
            .act(token, COMMONS, json!({ "action": "bulldoze", "x": hx, "y": hy }))
            .await
            .status
            .is_client_error());
    }
    // A builder away for six days: still not.
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| tx.charge(ann_id, 0, 0, now() - 6 * 86_400))
        .unwrap();
    assert!(server.act(&bo.token, COMMONS, bulldoze.clone()).await.status.is_client_error());
    // Gone a week: it is free to clear.
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| tx.charge(ann_id, 0, 0, now() - 7 * 86_400 - 5))
        .unwrap();
    let cleared = server.act(&bo.token, COMMONS, bulldoze).await;
    assert_eq!(cleared.status, StatusCode::OK, "{}", cleared.show());
    assert!(!server.tiles(COMMONS).contains_key(&(x as u16, y as u16)));
    // Ann comes back and can still take down her own.
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| tx.charge(ann_id, 0, 0, now()))
        .unwrap();
    assert_eq!(
        server
            .act(&ann.token, COMMONS, json!({ "action": "bulldoze", "x": first.0, "y": first.1 }))
            .await
            .status,
        StatusCode::OK
    );
}

// W18
#[tokio::test]
async fn the_keeper_removes_a_player_from_the_commons_and_their_token_stops() {
    let (server, keeper) = Server::start_with_keeper().await;
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    let other = server.guest().await;
    let id = server.player_id(&guest.token).await;
    let map = Map::from_db(&server, COMMONS);
    let (first, last) = map.line_from_road(3).unwrap();
    server.act(&guest.token, COMMONS, road_move(first, last)).await.ok();
    server.act(&other.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    let highway = server.tiles(COMMONS).values().filter(|tile| tile.player.is_none()).count();
    assert!(server.tiles(COMMONS).values().any(|tile| tile.player == Some(id as u64)));

    let path = format!("/api/worlds/{COMMONS}/players/{id}/remove");
    // Not a person, not the keeper: no.
    assert_eq!(server.post(&path, json!({})).send().await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        server.post(&path, json!({})).token(&guest.token).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server.post(&path, json!({})).token(&other.token).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server.post(&path, json!({})).token(&ann.key).send().await.status,
        StatusCode::FORBIDDEN
    );
    assert!(
        server.tiles(COMMONS).values().any(|tile| tile.player == Some(id as u64)),
        "nothing was taken down"
    );

    let removed = server.post(&path, json!({})).token(&keeper.key).send().await.ok();
    assert_eq!(removed["ok"], true);
    assert_eq!(removed["tiles_removed"], 3);
    assert!(!server.tiles(COMMONS).values().any(|tile| tile.player == Some(id as u64)));
    assert_eq!(
        server.tiles(COMMONS).values().filter(|tile| tile.player.is_none()).count(),
        highway,
        "the highway can't be removed"
    );
    // The token stops, everywhere, and the player has left the street.
    assert_eq!(
        server.get("/api/worlds/commons/me").token(&guest.token).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" }))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server.call("how_am_i_doing", json!({ "player_token": guest.token })).await["isError"],
        true
    );
    let view = server.fresh_view(COMMONS).await;
    assert_eq!(view["players"].as_array().unwrap().len(), 1, "only the other one is still there");
    // The claim code stops too, and the other player is untouched.
    assert_eq!(
        server
            .post("/api/players/claim", json!({ "claim_code": guest.claim }))
            .token(&ann.key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(server.me(&other.token, COMMONS).await["moves_left"], 29);
    // The keeper can't remove an id that was never in this world, or remove themself.
    assert_eq!(
        server
            .post(&format!("/api/worlds/{COMMONS}/players/9999/remove"), json!({}))
            .token(&keeper.key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .post(&format!("/api/worlds/{COMMONS}/players/zzz/remove"), json!({}))
            .token(&keeper.key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // A bot isn't a person, even a keeper's.
    let room = server
        .post("/api/hangouts", json!({ "name": "Crew", "your_name": "Bo" }))
        .send()
        .await
        .ok();
    let bot = server
        .post(
            &format!("/api/hangouts/{}/agents", room["hangout"]["id"].as_str().unwrap()),
            json!({ "name": "Scout" }),
        )
        .token(room["owner_key"].as_str().unwrap())
        .send()
        .await
        .ok();
    let other_id = server.player_id(&other.token).await;
    assert_eq!(
        server
            .post(&format!("/api/worlds/{COMMONS}/players/{other_id}/remove"), json!({}))
            .token(bot["token"].as_str().unwrap())
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

// W18
#[tokio::test]
async fn nobody_is_the_keeper_unless_the_server_says_so() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    let id = server.player_id(&guest.token).await;
    let reply = server
        .post(&format!("/api/worlds/{COMMONS}/players/{id}/remove"), json!({}))
        .token(&ann.key)
        .send()
        .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(server.me(&guest.token, COMMONS).await["moves_left"], 29);
}

// W18
#[tokio::test]
async fn me_tells_the_keeper_they_are_the_keeper_and_nobody_else() {
    let (server, keeper) = Server::start_with_keeper().await;
    let ann = server.person("Ann").await;
    let me = server.get("/api/me").token(&keeper.key).send().await.ok();
    assert_eq!(me["keeper"], true);
    assert_eq!(me["kind"], "person");
    let hers = server.get("/api/me").token(&ann.key).send().await.ok();
    assert!(hers.get("keeper").is_none(), "{hers}");
    // A bot is not the keeper, even one of the keeper's own.
    let room = server
        .post("/api/hangouts", json!({ "name": "Crew" }))
        .token(&keeper.key)
        .send()
        .await
        .ok();
    let bot = server
        .post(
            &format!("/api/hangouts/{}/agents", room["hangout"]["id"].as_str().unwrap()),
            json!({ "name": "Scout" }),
        )
        .token(&keeper.key)
        .send()
        .await
        .ok();
    let bots = server.get("/api/me").token(bot["token"].as_str().unwrap()).send().await.ok();
    assert_eq!(bots["kind"], "agent");
    assert!(bots.get("keeper").is_none(), "{bots}");
    // Nothing else changed about what the page needs to know: the flag says what the remove route would.
    let guest = server.guest().await;
    server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    let id = server.player_id(&guest.token).await;
    let path = format!("/api/worlds/{COMMONS}/players/{id}/remove");
    assert_eq!(
        server.post(&path, json!({})).token(&ann.key).send().await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        server.post(&path, json!({})).token(&keeper.key).send().await.status,
        StatusCode::OK
    );
    // A server that was never told who the keeper is has none.
    let plain = Server::start().await;
    let bo = plain.person("Bo").await;
    assert!(plain.get("/api/me").token(&bo.key).send().await.ok().get("keeper").is_none());
}

// W18
#[tokio::test]
async fn an_owner_removes_a_player_only_from_their_own_world() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let (ann_world, _) = server.own_world(&ann, true);
    let (bo_world, _) = server.own_world(&bo, true);
    let guest = server.guest().await;
    let id = server.player_id(&guest.token).await;
    // Nobody can build in a world that is frozen, so what the guest built there is put in directly.
    let (first, last) = Map::from_db(&server, &ann_world).line_from_road(2).unwrap();
    server.built_by(&ann_world, id, &[first, last]);
    let (first, last) = Map::from_db(&server, &bo_world).line_from_road(2).unwrap();
    server.built_by(&bo_world, id, &[first, last]);
    server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();

    // Bo can't touch Ann's world, Ann can't touch The Commons, and the guest can't do anything.
    let path = format!("/api/worlds/{ann_world}/players/{id}/remove");
    assert_eq!(
        server.post(&path, json!({})).token(&bo.key).send().await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        server.post(&path, json!({})).token(&guest.token).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .post(&format!("/api/worlds/{COMMONS}/players/{id}/remove"), json!({}))
            .token(&ann.key)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        server
            .post(&format!("/api/worlds/{bo_world}/players/{id}/remove"), json!({}))
            .token(&ann.key)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        server.tiles(&ann_world).values().filter(|tile| tile.player == Some(id as u64)).count(),
        2
    );

    let removed = server.post(&path, json!({})).token(&ann.key).send().await.ok();
    assert_eq!(removed["tiles_removed"], 2);
    assert_eq!(
        server.tiles(&ann_world).values().filter(|tile| tile.player == Some(id as u64)).count(),
        0
    );
    // They are barred from it, and the owner's world is the only place it reaches: the player is
    // still a player everywhere else, because a world's owner can't end a player.
    let barred = server
        .state
        .db
        .in_transaction::<_, rusqlite::Error>(|tx| tx.member_state(&ann_world, id))
        .unwrap();
    assert_eq!(barred, Some(true));
    let again =
        server.act(&guest.token, &ann_world, json!({ "action": "emote", "emote": "wave" })).await;
    assert_eq!(again.status, StatusCode::FORBIDDEN, "{}", again.show());
    assert_eq!(
        server
            .act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" }))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        server.tiles(&bo_world).values().filter(|tile| tile.player == Some(id as u64)).count(),
        2
    );
    // An owner can't remove themselves.
    let own = server.state.db.own_player(&ann.id, now()).unwrap().id;
    let (spare, _) = Map::from_db(&server, &ann_world).line_from_road(1).unwrap();
    server.built_by(&ann_world, own, &[spare]);
    assert_eq!(
        server
            .post(&format!("/api/worlds/{ann_world}/players/{own}/remove"), json!({}))
            .token(&ann.key)
            .send()
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
}

// ----------------------------------------------------------------------------------------------
// The step (W19-W24)
// ----------------------------------------------------------------------------------------------

// W19
#[tokio::test]
async fn a_step_is_a_pure_function_so_two_worlds_with_the_same_moves_match() {
    async fn run() -> (world::Tiles, u64) {
        let server = Server::start().await;
        let ann = server.person("Ann").await;
        let guest = server.guest().await;
        let (first, last) = Map::from_db(&server, COMMONS).line_from_road(8).unwrap();
        server.act(&guest.token, COMMONS, road_move(first, last)).await.ok();
        let spot = building_spots(&Map::from_db(&server, COMMONS), 1)[0];
        server.act(&ann.key, COMMONS, place("house", spot)).await.ok();
        for _ in 0..120 {
            let report = worlds::step_all(&server.state, now());
            assert_eq!(report.stepped, [COMMONS.to_string()]);
            assert!(report.failed.is_empty());
        }
        (server.tiles(COMMONS), server.state.db.world(COMMONS).unwrap().unwrap().step)
    }
    let (first, first_step) = run().await;
    let (second, second_step) = run().await;
    assert_eq!(first_step, 120, "a step a time, and a number for each");
    assert_eq!(second_step, 120);
    assert_eq!(
        first, second,
        "nothing random is stored: the same moves and steps give the same map"
    );
    assert!(first.values().any(|tile| tile.kind == Kind::House), "the moves were made");
}

// W24
#[tokio::test]
async fn the_commons_always_steps_however_long_nobody_plays_and_however_big_it_gets() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let step_of = || server.state.db.world(COMMONS).unwrap().unwrap().step;
    let t0 = now();
    let month = 31 * 86_400;
    // Nobody moves for months, and it still steps: everyone on its street walks by its step.
    for months in 0..4 {
        let report = worlds::step_all(&server.state, t0 + months * month);
        assert_eq!(report.stepped, [COMMONS.to_string()], "month {months}");
    }
    assert_eq!(step_of(), 4);
    make_a_town(&server);
    assert_eq!(worlds::step_all(&server.state, t0 + 5 * month).stepped, [COMMONS.to_string()]);
    assert_eq!(step_of(), 5);
}

// W32
#[tokio::test]
async fn a_step_that_fails_is_reported_and_the_next_round_tries_again() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    assert!(
        !worlds::step_one(&server.state, "no-such-world", now()),
        "a world that can't be stepped says so"
    );
    assert!(worlds::step_one(&server.state, COMMONS, now()));
    assert_eq!(server.state.db.world(COMMONS).unwrap().unwrap().step, 1);
    // A frozen world takes no step, and every round steps whatever is due all the same.
    let ann = server.person("Ann").await;
    let (world, _) = server.own_world(&ann, true);
    let report = worlds::step_all(&server.state, now());
    assert_eq!(report.stepped, [COMMONS.to_string()]);
    assert!(report.failed.is_empty());
    assert_eq!(server.state.db.world(COMMONS).unwrap().unwrap().step, 2);
    assert_eq!(server.state.db.world(&world).unwrap().unwrap().step, 0);
}

// ----------------------------------------------------------------------------------------------
// Worlds people started (W25-W28)
// ----------------------------------------------------------------------------------------------

// W27
#[tokio::test]
async fn the_list_is_the_busiest_50_however_many_frozen_worlds_there_are() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    for n in 0..200 {
        let id = format!("filler-{n:03}");
        let added = server
            .state
            .db
            .insert_world(
                &NewWorld {
                    id: &id,
                    name: "Filler",
                    kind: "own",
                    seed: n,
                    size: 64,
                    owner_id: None,
                    open: true,
                    invite_code: None,
                    highway: &[],
                    now: now(),
                },
                worlds::MAX_OWN_WORLDS,
            )
            .unwrap();
        assert_eq!(added, agenthangout::db::WorldAdded::Added);
    }
    // Nobody can start another, and nothing is made by trying (W25).
    let ann = server.person("Ann").await;
    let reply = server.post("/api/worlds", json!({})).token(&ann.key).send().await;
    assert_eq!(reply.status, StatusCode::GONE, "{}", reply.show());
    assert!(server.state.db.world_of_owner(&ann.id).unwrap().is_none());
    // The list is the busiest 50, however many there are, with The Commons on it.
    let list = server.get("/api/worlds").send().await.ok();
    let worlds_listed = list["worlds"].as_array().unwrap();
    assert_eq!(worlds_listed.len(), 50);
    assert!(worlds_listed.iter().any(|card| card["id"] == COMMONS));
}

// W27
#[tokio::test]
async fn a_frozen_worlds_link_is_never_shown_and_only_its_owner_has_it() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let (closed, closed_code) = server.own_world(&ann, false);
    let (open, open_code) = server.own_world(&bo, true);

    // Anyone can watch, closed or open, and the link is never in what they see.
    for (world, code) in [(&closed, &closed_code), (&open, &open_code)] {
        let reply = server.get(&format!("/api/worlds/{world}")).send().await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.show());
        assert!(!reply.text.contains(code.as_str()));
    }
    assert_eq!(server.view(&closed).await["open"], false);
    assert_eq!(server.view(&open).await["open"], true);
    let list = server.get("/api/worlds").send().await;
    assert!(!list.text.contains(&closed_code) && !list.text.contains(&open_code));
    // Only the owner sees it, in `mine`.
    let mine = server.get("/api/worlds/mine").token(&ann.key).send().await.ok();
    assert_eq!(
        (mine["world"]["id"].as_str(), mine["world"]["invite_code"].as_str()),
        (Some(closed.as_str()), Some(closed_code.as_str()))
    );
    let hers = server.get("/api/worlds/mine").token(&ann.key).send().await.text;
    let his = server.get("/api/worlds/mine").token(&bo.key).send().await.text;
    assert!(!hers.contains(&open_code) && !his.contains(&closed_code));
    assert!(his.contains(&open_code));
}

// W28
#[tokio::test]
async fn the_commons_is_128_by_128_with_a_fixed_seed_and_is_there_from_the_start() {
    let server = Server::start().await;
    // Nothing has touched a world yet, and The Commons is made by the server's own call.
    worlds::ensure_commons(&server.state).unwrap();
    worlds::ensure_commons(&server.state).unwrap();
    let row = server.state.db.world(COMMONS).unwrap().unwrap();
    assert_eq!((row.size, row.seed, row.kind.as_str()), (128, world::COMMONS_SEED, "public"));
    assert!(row.owner_id.is_none() && row.invite_code.is_none());
    let view = server.view(COMMONS).await;
    assert_eq!(view["size"], 128);
    assert_eq!(view["seed"], 307);
    assert_eq!(view["land"], json!(world::encode_land(&world::terrain(world::COMMONS_SEED, 128))));
    assert_eq!(view["land"].as_array().unwrap().len(), 128);
    // It is the same world after a restart: a second server on the same land has the same highway.
    let again = Server::start().await;
    assert_eq!(again.view(COMMONS).await["tiles"], view["tiles"]);
    // And it has no owner to remove.
    assert_eq!(
        server.get("/api/worlds/mine").token(&server.person("Ann").await.key).send().await.ok()
            ["world"],
        Value::Null
    );
}

// W28
#[tokio::test]
async fn a_server_that_has_not_been_asked_about_a_world_has_not_made_one() {
    let server = Server::start().await;
    // Hundreds of tests never touch a world, so opening the app costs them nothing.
    assert!(server.state.db.world(COMMONS).unwrap().is_none());
    server.get("/api/me").send().await;
    server.get("/").send().await;
    assert!(server.state.db.world(COMMONS).unwrap().is_none());
    server.get("/api/worlds").send().await.ok();
    assert!(server.state.db.world(COMMONS).unwrap().is_some());
}

// ----------------------------------------------------------------------------------------------
// Staying up (W29-W33)
// ----------------------------------------------------------------------------------------------

// W29
#[tokio::test]
async fn a_world_view_is_built_once_and_served_from_memory_to_everyone() {
    let server = Server::start().await;
    server.view(COMMONS).await;
    let built = server.state.worlds.view_builds();
    assert_eq!(built, 1);
    let first = server.get("/api/worlds/commons").send().await;
    for _ in 0..100 {
        let reply = server.get("/api/worlds/commons").send().await;
        assert_eq!(reply.text, first.text, "everyone is shown the same view");
    }
    assert_eq!(server.state.worlds.view_builds(), built, "a hundred views built nothing");
    // The reply says how to poll: always check, and here is the tag.
    assert_eq!(first.headers.get("cache-control").unwrap(), "no-cache");
    let tag = first.headers.get("etag").unwrap().to_str().unwrap().to_string();
    assert!(tag.starts_with('"') && tag.ends_with('"'));
    // A browser that has it is told so, with no body.
    let not_modified = server.get("/api/worlds/commons").header("if-none-match", &tag).send().await;
    assert_eq!(not_modified.status, StatusCode::NOT_MODIFIED);
    assert!(not_modified.text.is_empty());
    assert_eq!(not_modified.headers.get("etag").unwrap().to_str().unwrap(), tag);
    let other = server
        .get("/api/worlds/commons")
        .header("if-none-match", "\"something-else\"")
        .send()
        .await;
    assert_eq!(other.status, StatusCode::OK);
    // The list too.
    server.get("/api/worlds").send().await.ok();
    let list = server.get("/api/worlds").send().await;
    let tag = list.headers.get("etag").unwrap().to_str().unwrap().to_string();
    assert_eq!(
        server.get("/api/worlds").header("if-none-match", &tag).send().await.status,
        StatusCode::NOT_MODIFIED
    );
}

// W29
#[tokio::test]
async fn a_view_does_not_read_the_database_so_it_shows_what_was_built_not_what_is_stored() {
    let server = Server::start().await;
    let before = server.view(COMMONS).await;
    // Take every tile out of the database behind the cache's back.
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| {
            let tiles = tx.all_tiles(COMMONS)?;
            let removals: Vec<Change> = tiles.keys().map(|pos| Change::Remove(*pos)).collect();
            tx.write_changes(COMMONS, &removals, now())
        })
        .unwrap();
    assert!(server.tiles(COMMONS).is_empty());
    for _ in 0..20 {
        assert_eq!(server.view(COMMONS).await["tiles"], before["tiles"]);
    }
}

// W29
#[tokio::test]
async fn a_move_makes_the_view_stale_and_it_is_rebuilt_within_a_second() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let first = server.view(COMMONS).await;
    assert!(first["players"].as_array().unwrap().iter().all(|player| player[4].is_null()));
    let builds = server.state.worlds.view_builds();
    server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "dance" })).await.ok();
    let (road_first, road_last) = Map::from_view(&first).line_from_road(2).unwrap();
    server.act(&guest.token, COMMONS, road_move(road_first, road_last)).await.ok();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let view = server.view(COMMONS).await;
    assert_eq!(
        server.state.worlds.view_builds(),
        builds + 1,
        "one rebuild for everything that changed"
    );
    assert_eq!(view["players"][0][4], "dance");
    assert!(
        view["tiles"].as_array().unwrap().len() >= first["tiles"].as_array().unwrap().len() + 2
    );
    // Nobody asking and nothing changing, so it isn't rebuilt more than every few seconds.
    for _ in 0..10 {
        server.view(COMMONS).await;
    }
    assert_eq!(server.state.worlds.view_builds(), builds + 1);
}

// W29
#[tokio::test]
async fn views_for_worlds_that_do_not_exist_are_refused_and_keep_nothing() {
    let server = Server::start().await;
    server.view(COMMONS).await;
    let builds = server.state.worlds.view_builds();
    for n in 0..30 {
        let reply = server.get(&format!("/api/worlds/made-up-{n}")).send().await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
        assert!(!reply.text.contains("made-up"));
    }
    assert_eq!(server.state.worlds.view_builds(), builds);
    assert_eq!(
        server.get("/api/worlds/mine").send().await.status,
        StatusCode::UNAUTHORIZED,
        "mine isn't a world id"
    );
    // The preview of maps to choose from went with starting a world (W26).
    assert_eq!(server.get("/api/worlds/preview").send().await.status, StatusCode::NOT_FOUND);
    assert_eq!(server.state.worlds.view_builds(), builds);
    assert_eq!(
        server.get("/api/worlds/join/xyz").send().await.status,
        StatusCode::METHOD_NOT_ALLOWED
    );
}

// W30
#[tokio::test]
async fn world_views_are_limited_to_240_a_minute_per_address() {
    start_early_in(60, 30).await;
    let server = Server::start().await;
    for n in 0..240 {
        let reply = server.get("/api/worlds/commons").ip("203.0.113.50").send().await;
        assert_eq!(reply.status, StatusCode::OK, "view {n}: {}", reply.show());
    }
    let over = server.get("/api/worlds/commons").ip("203.0.113.50").send().await;
    assert_eq!(over.status, StatusCode::TOO_MANY_REQUESTS, "{}", over.show());
    // The list shares the allowance, another address has its own, and no address header is local.
    assert_eq!(
        server.get("/api/worlds").ip("203.0.113.50").send().await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        server.get("/api/worlds/commons").ip("203.0.113.51").send().await.status,
        StatusCode::OK
    );
    for _ in 0..70 {
        assert_eq!(server.get("/api/worlds/commons").send().await.status, StatusCode::OK);
    }
    // A client can't borrow someone else's address by naming it first in the header.
    assert_eq!(
        server.get("/api/worlds/commons").ip("203.0.113.99, 203.0.113.50").send().await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    // `look` is a view too.
    let reply = server.post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "look", "arguments": {} } })).ip("203.0.113.50").send().await;
    assert_eq!(reply.body["result"]["isError"], true);
    // Moves are not views: they have their own cap.
    let guest = server.guest().await;
    assert_eq!(
        server
            .act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" }))
            .await
            .status,
        StatusCode::OK
    );
}

// W30
#[tokio::test]
async fn moves_are_capped_in_a_world_at_300_a_minute() {
    let server = Server::start().await;
    let mut guests = Vec::new();
    for _ in 0..11 {
        guests.push(server.guest().await);
    }
    let mut applied = 0;
    let mut refused = 0;
    for guest in &guests {
        for _ in 0..30 {
            let reply = server
                .act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" }))
                .await;
            match reply.status {
                StatusCode::OK => applied += 1,
                StatusCode::TOO_MANY_REQUESTS => refused += 1,
                other => panic!("{other}: {}", reply.text),
            }
        }
    }
    assert_eq!(applied, 300, "300 moves went through");
    assert_eq!(refused, 30, "and the rest were told to wait");
    // A refused one cost nothing: the last guest only used what went through, and the 300 that did
    // were the first 10 guests' 30 each, so this one spent none.
    let last = guests.last().unwrap();
    assert_eq!(server.moves_left(&last.token).await, 30);
    // Moves that are refused for another reason did not count towards the cap. A player's refused
    // moves have a limit of their own (60 a minute), so seven players are refused 50 times each.
    let fresh = Server::start().await;
    for _ in 0..7 {
        let guest = fresh.guest().await;
        for _ in 0..50 {
            assert_eq!(
                fresh
                    .act(&guest.token, COMMONS, json!({ "action": "walk", "x": -1, "y": -1 }))
                    .await
                    .status,
                StatusCode::BAD_REQUEST
            );
        }
    }
    let guest = fresh.guest().await;
    assert_eq!(
        fresh
            .act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" }))
            .await
            .status,
        StatusCode::OK
    );
}

/// A guest made straight in the database, for tests that need many and call the rules directly.
fn spare_guest(server: &Server, n: usize, now: i64) -> agenthangout::db::PlayerRow {
    let token = auth::hash_secret(&format!("spare-token-{n}"));
    let claim = auth::hash_secret(&format!("spare-claim-{n}"));
    server.state.db.create_guest(&token, &claim, now).unwrap()
}

// W30
#[tokio::test]
async fn a_world_that_has_used_its_300_is_told_so_and_the_minute_after_it_is_open() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let ann = server.person("Ann").await;
    let t = now();
    let owner = server.state.db.own_player(&ann.id, t).unwrap();
    for n in 0..300 {
        worlds::act(&server.state, &owner, COMMONS, &emote("wave"), t)
            .unwrap_or_else(|error| panic!("move {n}: {error:?}"));
    }
    // The 301st, from a player with moves to spare, is the world's to refuse, and costs them nothing.
    let guest = spare_guest(&server, 0, t);
    for _ in 0..5 {
        let busy = worlds::act(&server.state, &guest, COMMONS, &emote("wave"), t).unwrap_err();
        assert_eq!(busy.status, StatusCode::TOO_MANY_REQUESTS);
        assert!(busy.message.contains("this world is very busy"), "{}", busy.message);
    }
    assert_eq!(worlds::moves_left(&server.player_row(guest.id), t), 30);
    // A minute later it is open again.
    worlds::act(&server.state, &guest, COMMONS, &emote("wave"), t + 60).unwrap();
}

// W31
#[tokio::test]
async fn look_returns_at_most_32_by_32_tiles_and_a_view_has_a_fixed_size() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let look =
        server.tool("look", json!({ "player_token": guest.token, "w": 500, "h": 1000 })).await;
    assert_eq!(look["window"]["w"], 32);
    assert_eq!(look["window"]["h"], 32);
    let rows = look["land"].as_array().unwrap();
    assert_eq!(rows.len(), 32);
    assert!(rows.iter().all(|row| row.as_str().unwrap().len() == 32));
    // Tiles and players are inside the window.
    let (wx, wy) = (look["window"]["x"].as_i64().unwrap(), look["window"]["y"].as_i64().unwrap());
    for tile in look["built"].as_array().unwrap() {
        let (x, y) = (tile[0].as_i64().unwrap(), tile[1].as_i64().unwrap());
        assert!((wx..wx + 32).contains(&x) && (wy..wy + 32).contains(&y));
    }
    // Smaller on request, and never off the map, whatever the centre.
    let small = server.tool("look", json!({ "x": 0, "y": 0, "w": 5, "h": 3 })).await;
    assert_eq!((small["window"]["x"].as_i64(), small["window"]["y"].as_i64()), (Some(0), Some(0)));
    assert_eq!(small["land"].as_array().unwrap().len(), 3);
    assert_eq!(small["land"][0].as_str().unwrap().len(), 5);
    let corner = server.tool("look", json!({ "x": 127, "y": 127, "w": 32, "h": 32 })).await;
    assert_eq!(corner["window"]["x"], 96);
    let zero = server.tool("look", json!({ "w": 0, "h": -4 })).await;
    assert_eq!(zero["land"].as_array().unwrap().len(), 1);
    server.tool_refused("look", json!({ "x": 500, "y": 5 })).await;
    // The land in a window is the land of the map.
    let land = world::terrain(world::COMMONS_SEED, 128);
    for (row, text) in small["land"].as_array().unwrap().iter().enumerate() {
        let want: String = (0..5).map(|x| land.get(x, row).unwrap().letter()).collect();
        assert_eq!(text, &want);
    }
    // A view lists no more than 64 players.
    for _ in 0..70 {
        let ip = server.fresh_ip();
        let joined = server.post("/api/public/join", json!({})).ip(&ip).send().await;
        if joined.status == StatusCode::OK {
            let token = joined.body["player_token"].as_str().unwrap().to_string();
            server.act(&token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
        }
    }
    let view = server.fresh_view(COMMONS).await;
    assert!(view["players"].as_array().unwrap().len() <= 64);
}

// W33
#[tokio::test]
async fn the_private_mcp_endpoint_is_unchanged_and_the_public_one_offers_only_its_seven() {
    let server = Server::start().await;
    let reply = server.rpc("tools/list", json!({})).await;
    let names: Vec<&str> = reply.body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["join", "look", "build", "bulldoze", "walk", "emote", "how_am_i_doing"]);
    // Nothing private is offered, or works, here.
    for private in [
        "inbox",
        "say",
        "read_room",
        "list_city",
        "ask_owner",
        "propose_hangout",
        "rsvp",
        "ask_for_trip",
        "house_queue",
    ] {
        assert!(!names.contains(&private));
        server.tool_refused(private, json!({})).await;
    }
    // And the private endpoint offers nothing public.
    let room = server
        .post("/api/hangouts", json!({ "name": "Crew", "your_name": "Bo" }))
        .send()
        .await
        .ok();
    let bot = server
        .post(
            &format!("/api/hangouts/{}/agents", room["hangout"]["id"].as_str().unwrap()),
            json!({ "name": "Scout" }),
        )
        .token(room["owner_key"].as_str().unwrap())
        .send()
        .await
        .ok();
    let private = server
        .post("/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .token(bot["token"].as_str().unwrap())
        .send()
        .await;
    let private_names: Vec<&str> = private.body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    for public in ["join", "look", "build", "bulldoze", "walk", "emote", "how_am_i_doing"] {
        assert!(!private_names.contains(&public), "{public} must not be on the private endpoint");
    }
    // 30, public_floors, go_out and come_back for going out (L6, L8), and three for projects (L11 to L13).
    assert!(private_names.contains(&"inbox") && private_names.len() == 36, "{private_names:?}");
    // A private tool is refused on the private endpoint when it's one of the world's.
    let call = server
        .post("/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "join", "arguments": {} } }))
        .token(bot["token"].as_str().unwrap())
        .send()
        .await;
    assert_eq!(call.body["result"]["isError"], true);
}

// ----------------------------------------------------------------------------------------------
// Crawlers and agents passing through (W34-W45)
// ----------------------------------------------------------------------------------------------

// W34, W36, W37
#[tokio::test]
async fn a_crawler_fetching_a_page_is_shown_as_a_frog_on_a_road() {
    let server = Server::start().await;
    server.view(COMMONS).await;
    assert_eq!(server.get("/").ua(GOOGLEBOT).send().await.status, StatusCode::OK);
    let view = server.fresh_view(COMMONS).await;
    let frogs = view["crawlers"].as_array().unwrap();
    assert_eq!(frogs.len(), 1);
    let frog = frogs[0].as_array().unwrap();
    assert_eq!(frog.len(), 5, "kind, name, x, y and how many seconds ago");
    assert_eq!(frog[0], "googlebot");
    assert_eq!(frog[1], "Googlebot");
    assert!(frog[4].as_i64().unwrap() <= 3, "just seen");
    let roads = world::Roads::from_tiles(&server.tiles(COMMONS));
    let step = view["step"].as_u64().unwrap();
    let (x, y) = world::crawler_position("googlebot", step, &roads).unwrap();
    assert_eq!((frog[2].as_u64(), frog[3].as_u64()), (Some(u64::from(x)), Some(u64::from(y))));
    assert!(Map::from_view(&view).roads().contains(&(i64::from(x), i64::from(y))), "on a road");
    // It does nothing: not a player, no moves, and it builds nothing.
    assert_eq!(view["players"], json!([]));
    assert_eq!(view["visitors"], json!([]));
    assert_eq!(view["places"], 0, "nothing was put up");
    assert_eq!(
        view["tiles"].as_array().unwrap().len(),
        the_highway().len(),
        "the map is as it was"
    );
    // One frog per kind, however often it comes. Another kind is another frog.
    server.get("/").ua(GOOGLEBOT).send().await;
    server.get("/worlds").ua(GOOGLEBOT).send().await;
    server.get("/").ua(GPTBOT).send().await;
    let view = server.fresh_view(COMMONS).await;
    let ids: Vec<&str> =
        view["crawlers"].as_array().unwrap().iter().map(|frog| frog[0].as_str().unwrap()).collect();
    assert_eq!(ids, ["googlebot", "gptbot"], "in the order of the list");
    // Where it hops comes from the step: after a step it has moved on, nobody steered it.
    let before = view["crawlers"][0].clone();
    worlds::step_all(&server.state, now());
    let after = server.fresh_view(COMMONS).await["crawlers"][0].clone();
    assert_eq!(after[0], before[0]);
    assert_ne!((after[2].clone(), after[3].clone()), (before[2].clone(), before[3].clone()));
}

// W34
#[tokio::test]
async fn every_listed_page_shows_a_crawler_and_nothing_else_does() {
    let server = Server::start().await;
    let day = worlds::today(now());
    let places = [
        (Method::GET, "/", true),
        (Method::GET, "/worlds", true),
        (Method::GET, "/world/commons", true),
        (Method::GET, "/world/anything-at-all", true),
        (Method::GET, "/robots.txt", true),
        (Method::GET, "/sitemap.xml", true),
        (Method::GET, "/world/", false),
        (Method::GET, "/world/a/b", false),
        (Method::POST, "/", false),
        (Method::POST, "/worlds", false),
        (Method::GET, "/api/worlds/commons", false),
        (Method::GET, "/api/me", false),
        (Method::GET, "/mcp", false),
        (Method::POST, "/mcp", false),
        (Method::GET, "/api/hangouts/abc", false),
        (Method::GET, "/style.css", false),
        (Method::GET, "/h/abc", false),
    ];
    for (method, path, shown) in places {
        let fresh = Server::start().await;
        fresh.req(method.clone(), path).ua(GOOGLEBOT).json(json!({})).send().await;
        let frogs = fresh.state.worlds.frogs(now());
        assert_eq!(!frogs.is_empty(), shown, "{method} {path}");
        assert!(fresh.state.worlds.visitors(now()).is_empty(), "{method} {path} is not a visitor");
    }
    // A user agent that isn't on the list is ignored, whatever it fetches.
    for agent in
        ["curl/8.5", "Mozilla/5.0 Chrome/126", "SomeUnknownBot/1.0", "", "Googlebo", "Goog1ebot"]
    {
        server.get("/").ua(agent).send().await;
        server.get("/worlds").ua(agent).send().await;
    }
    server.get("/").send().await;
    assert!(server.state.worlds.frogs(now()).is_empty());
    assert!(server.state.worlds.visitors(now()).is_empty());
    for day in day..=worlds::today(now()) {
        assert!(server.state.db.sightings_on(day).unwrap().is_empty());
    }
}

// W36
#[tokio::test]
async fn a_frog_is_shown_for_five_minutes_after_its_last_visit() {
    let server = Server::start().await;
    let t = 1_700_000_000;
    let headers = {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", GOOGLEBOT.parse().unwrap());
        headers
    };
    worlds::note_request(&server.state, &Method::GET, "/", &headers, t);
    assert_eq!(server.state.worlds.frogs(t).len(), 1);
    assert_eq!(server.state.worlds.frogs(t + 299).len(), 1);
    assert!(server.state.worlds.frogs(t + 300).is_empty());
    // A visit extends it.
    worlds::note_request(&server.state, &Method::GET, "/", &headers, t + 200);
    assert_eq!(server.state.worlds.frogs(t + 499).len(), 1);
    assert!(server.state.worlds.frogs(t + 500).is_empty());
}

/// What a browser sends when a page looks at The Commons, from an address of its own.
fn page_headers(n: u32) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "user-agent",
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/140".parse().unwrap(),
    );
    headers.insert("sec-fetch-site", "same-origin".parse().unwrap());
    headers.insert("sec-fetch-mode", "cors".parse().unwrap());
    headers.insert("x-forwarded-for", format!("198.51.{}.{}", n / 250, n % 250).parse().unwrap());
    headers
}

const COMMONS_VIEW: &str = "/api/worlds/commons";

// W52
#[tokio::test]
async fn a_page_looking_at_the_commons_is_one_person_on_the_street_until_it_stops() {
    let server = Server::start().await;
    let t = 1_700_000_000;
    let note = |n: u32, at: i64| {
        worlds::note_request(&server.state, &Method::GET, COMMONS_VIEW, &page_headers(n), at)
    };
    let people = |at: i64| server.state.worlds.people(at);
    note(1, t);
    assert_eq!(people(t).len(), 1);
    let first = people(t)[0];
    for seconds in [4, 8, 12] {
        note(1, t + seconds);
    }
    assert_eq!(people(t + 12), [first], "a page looking again is the same person");
    note(2, t + 12);
    assert_eq!(people(t + 12).len(), 2, "someone else is someone else");
    // A page that stops looking leaves a while later.
    assert_eq!(people(t + 12 + worlds::PERSON_SECS - 1).len(), 2);
    assert!(people(t + 12 + worlds::PERSON_SECS).is_empty());
    // Coming back is a new arrival, and nothing about the last visit was kept.
    note(1, t + 1000);
    assert_eq!(people(t + 1000).len(), 1);
    assert_ne!(people(t + 1000), [first]);
}

// W52
#[tokio::test]
async fn only_a_browser_page_on_the_commons_counts_as_a_person() {
    let server = Server::start().await;
    let t = 1_700_000_000;
    let tried = |headers: HeaderMap, method: Method, path: &str| {
        worlds::note_request(&server.state, &method, path, &headers, t);
        assert!(
            server.state.worlds.people(t).is_empty(),
            "{method} {path} made a person from {headers:?}"
        );
    };
    // Anything that isn't the page's own look at The Commons.
    tried(page_headers(1), Method::GET, "/");
    tried(page_headers(1), Method::GET, "/world/commons");
    tried(page_headers(1), Method::GET, "/api/worlds/somewhere-else");
    tried(page_headers(1), Method::POST, COMMONS_VIEW);
    // Curl and bots leave these out, and another site's page says so.
    for name in ["sec-fetch-site", "sec-fetch-mode"] {
        let mut headers = page_headers(1);
        headers.remove(name);
        tried(headers, Method::GET, COMMONS_VIEW);
    }
    let mut elsewhere = page_headers(1);
    elsewhere.insert("sec-fetch-site", "cross-site".parse().unwrap());
    tried(elsewhere, Method::GET, COMMONS_VIEW);
    // A crawler or a fetcher running a browser is a frog or an agent, and never a person.
    for agent in [GOOGLEBOT, CHATGPT_USER] {
        let mut headers = page_headers(1);
        headers.insert("user-agent", agent.parse().unwrap());
        tried(headers, Method::GET, COMMONS_VIEW);
    }
}

// W52
#[tokio::test]
async fn someone_signed_in_shows_once_as_a_person_watching_or_as_their_own_player() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let ann = server.person("Ann").await;
    let session = server.state.db.create_session(
        &auth::hash_secret("ann-session"),
        &ann.id,
        now() + 3600,
        now(),
    );
    session.unwrap();
    let page = |cookie: &str| {
        let mut headers = page_headers(1);
        headers.insert("cookie", cookie.parse().unwrap());
        headers
    };
    let note = |cookie: &str, at: i64| {
        worlds::note_request(&server.state, &Method::GET, COMMONS_VIEW, &page(cookie), at)
    };
    let t = now();
    // Watching and signed in, with nothing built yet, they're a person like anyone.
    note("ah_session=ann-session", t);
    assert_eq!(server.state.worlds.people(t).len(), 1);
    // Once their player is out on the street, they stop being a second figure.
    let player = server.state.db.own_player(&ann.id, t).unwrap();
    worlds::act(&server.state, &player, COMMONS, &emote("wave"), t).unwrap();
    note("ah_session=ann-session", t + 1);
    assert!(server.state.worlds.people(t + 1 + worlds::PERSON_SECS).is_empty());
    // A sign-in that has lapsed is only someone watching.
    note("theme=dark; ah_session=nothing-like-a-session", t + 500);
    assert_eq!(server.state.worlds.people(t + 500).len(), 1);
}

// W44, W52
#[tokio::test]
async fn a_crowd_of_addresses_shows_a_limited_street_and_the_oldest_leave_first() {
    let server = Server::start().await;
    let t = 1_700_000_000;
    for n in 0..1000 {
        worlds::note_request(&server.state, &Method::GET, COMMONS_VIEW, &page_headers(n), t);
    }
    let street = server.state.worlds.people(t);
    assert_eq!(street.len(), worlds::MAX_PEOPLE);
    worlds::note_request(&server.state, &Method::GET, COMMONS_VIEW, &page_headers(5000), t);
    assert_eq!(server.state.worlds.people(t)[..worlds::MAX_PEOPLE - 1], street[1..]);
}

// W52
#[tokio::test]
async fn the_view_shows_people_under_generated_names_on_the_roads_and_only_in_the_commons() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    worlds::note_request(&server.state, &Method::GET, COMMONS_VIEW, &page_headers(1), now());
    let view = server.fresh_view(COMMONS).await;
    let people = view["people"].as_array().unwrap();
    assert_eq!(people.len(), 1);
    let (x, y) = (people[0][1].as_u64().unwrap() as u16, people[0][2].as_u64().unwrap() as u16);
    assert!(people[0][0].as_str().unwrap().contains(' '), "{people:?}");
    assert_eq!(server.tiles(COMMONS)[&(x, y)].kind, Kind::Road);
    // Nothing from the request is in the view, and a world of someone's own has no street like it.
    assert!(!view.to_string().contains("198.51."));
    let ann = server.person("Ann").await;
    let (own, _) = server.own_world(&ann, true);
    assert!(server.fresh_view(&own).await.get("people").is_none());
}

// W53
#[tokio::test]
async fn a_player_on_the_street_is_a_person_or_an_agent() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let wave = json!({ "action": "emote", "emote": "wave" });
    server.act(&ann.key, COMMONS, wave.clone()).await.ok();
    let guest = server.guest().await;
    server.act(&guest.token, COMMONS, wave.clone()).await.ok();
    let claimed = server.guest().await;
    server.act(&claimed.token, COMMONS, wave.clone()).await.ok();
    server
        .post("/api/players/claim", json!({ "claim_code": claimed.claim }))
        .token(&ann.key)
        .send()
        .await
        .ok();
    let view = server.fresh_view(COMMONS).await;
    let mut kinds: Vec<&str> =
        view["players"].as_array().unwrap().iter().map(|row| row[5].as_str().unwrap()).collect();
    kinds.sort_unstable();
    // Ann, and two agents: claiming one doesn't make it a person.
    assert_eq!(kinds, ["agent", "agent", "person"]);
}

// W35, W39
#[tokio::test]
async fn a_user_agent_is_only_looked_at_and_nothing_of_a_visit_is_kept() {
    let server = Server::start().await;
    let day = worlds::today(now());
    let long = format!("Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html) secret-agent-text-7731 {}", "x".repeat(2000));
    let ip = "203.0.113.77";
    server.get("/").ua(&long).ip(ip).send().await;
    server.get("/world/secret-path-8842").ua(&long).ip(ip).send().await;
    server
        .get("/")
        .ua("Mozilla/5.0 (compatible; ChatGPT-User/1.0) secret-agent-text-7731")
        .ip(ip)
        .send()
        .await;
    let view = server.fresh_view(COMMONS).await;
    let text = view.to_string();
    for secret in ["secret-agent-text-7731", "secret-path-8842", ip, "x".repeat(40).as_str()] {
        assert!(!text.contains(secret), "{secret}");
    }
    // The database holds a kind, a day and a count, and nothing else. What was waiting goes on the
    // day it's written, and two minutes on can be tomorrow.
    let later = now() + 120;
    worlds::housekeeping(&server.state, later);
    let mut rows = Vec::new();
    for day in day..=worlds::today(later) {
        rows.extend(server.state.db.sightings_on(day).unwrap());
    }
    let mut kinds: Vec<&str> = rows.iter().map(|row| row.0.as_str()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds, ["chatgpt-user", "googlebot"]);
    let googlebot: i64 = rows.iter().filter(|row| row.0 == "googlebot").map(|row| row.1).sum();
    assert_eq!(googlebot, 2, "two visits");
    for secret in ["secret", ip] {
        for row in &rows {
            assert!(!row.0.contains(secret));
        }
    }
    // A user agent whose first 512 bytes say nothing is not looked at past them.
    let late = format!("{}Googlebot", "a".repeat(600));
    server.get("/").ua(&late).send().await;
    assert!(server.state.worlds.frogs(now()).iter().all(|(crawler, _)| crawler.id != "ahrefsbot"));
}

// W39
#[tokio::test]
async fn sightings_still_waiting_at_midnight_go_on_the_day_they_are_written() {
    let server = Server::start().await;
    let day = worlds::today(1_700_000_000);
    let t = (day + 1) * 86_400 - 60;
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", GOOGLEBOT.parse().unwrap());
    worlds::note_request(&server.state, &Method::GET, "/", &headers, t);
    worlds::note_request(&server.state, &Method::GET, "/", &headers, t + 1);
    worlds::housekeeping(&server.state, t + 120);
    assert_eq!(server.state.db.sightings_on(day).unwrap()[0].1, 1);
    assert_eq!(server.state.db.sightings_on(day + 1).unwrap()[0].1, 1);
}

// W39
#[tokio::test]
async fn sightings_are_counted_per_kind_per_day_and_written_at_most_once_a_minute() {
    let server = Server::start().await;
    let t = 1_700_000_000;
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", GOOGLEBOT.parse().unwrap());
    let day = worlds::today(t);
    worlds::note_request(&server.state, &Method::GET, "/", &headers, t);
    // The first is written at once; the rest wait.
    assert_eq!(server.state.db.sightings_on(day).unwrap(), [("googlebot".to_string(), 1, t)]);
    for n in 1..5 {
        worlds::note_request(&server.state, &Method::GET, "/", &headers, t + n);
    }
    assert_eq!(
        server.state.db.sightings_on(day).unwrap()[0].1,
        1,
        "not written again within the minute"
    );
    worlds::housekeeping(&server.state, t + 30);
    assert_eq!(server.state.db.sightings_on(day).unwrap()[0].1, 1);
    worlds::housekeeping(&server.state, t + 61);
    assert_eq!(server.state.db.sightings_on(day).unwrap(), [("googlebot".to_string(), 5, t + 61)]);
    // Nothing left to write, and a new day is a new count.
    worlds::housekeeping(&server.state, t + 200);
    assert_eq!(server.state.db.sightings_on(day).unwrap()[0].1, 5);
    worlds::note_request(&server.state, &Method::GET, "/", &headers, t + 86_400);
    assert_eq!(server.state.db.sightings_on(day + 1).unwrap()[0].1, 1);
    assert_eq!(server.state.db.sightings_on(day).unwrap()[0].1, 5);
    // Another kind is another row.
    headers.insert("user-agent", GPTBOT.parse().unwrap());
    worlds::note_request(&server.state, &Method::GET, "/", &headers, t + 86_401);
    assert_eq!(server.state.db.sightings_on(day + 1).unwrap().len(), 2);
}

// W38
#[tokio::test]
async fn a_crawler_gets_exactly_the_page_a_browser_gets() {
    let server = Server::start().await;
    server.view(COMMONS).await;
    for path in [
        "/",
        "/llms.txt",
        "/agents.md",
        "/robots.txt",
        "/sitemap.xml",
        "/worlds",
        "/world/commons",
        "/api/worlds/commons",
        "/api/worlds",
        "/style.css",
        "/nowhere",
    ] {
        let browser = server.get(path).ua(CHROME).send().await;
        for agent in [GOOGLEBOT, GPTBOT, CHATGPT_USER, GEMINI, "curl/8.5"] {
            let other = server.get(path).ua(agent).send().await;
            assert_eq!(other.status, browser.status, "{path} for {agent}");
            assert_eq!(other.text, browser.text, "{path} for {agent}");
            for header in ["content-type", "cache-control", "content-security-policy", "etag"] {
                assert_eq!(
                    other.headers.get(header),
                    browser.headers.get(header),
                    "{path} {header} for {agent}"
                );
            }
            assert_eq!(other.headers.len(), browser.headers.len(), "{path} for {agent}");
        }
    }
    // The view that lists the frogs is the same for them as for everyone, after they have visited.
    let view = server.fresh_view(COMMONS).await;
    assert!(!view["crawlers"].as_array().unwrap().is_empty());
    let as_bot = server.get("/api/worlds/commons").ua(GOOGLEBOT).send().await.text;
    let as_browser = server.get("/api/worlds/commons").ua(CHROME).send().await.text;
    assert_eq!(as_bot, as_browser);
    // MCP and the join route answer an agent the same way whatever it says it is.
    let one = server.rpc("tools/list", json!({})).await;
    let two = server
        .post(
            "/public/mcp",
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
        )
        .ua(GPTBOT)
        .send()
        .await;
    assert_eq!(one.text, two.text);
}

// W40, W42, W43
#[tokio::test]
async fn an_agent_working_for_someone_is_a_visitor_with_a_generated_name() {
    let server = Server::start().await;
    server.view(COMMONS).await;
    server.get("/").ua(GEMINI).ip("203.0.113.20").send().await;
    let view = server.fresh_view(COMMONS).await;
    let visitors = view["visitors"].as_array().unwrap();
    assert_eq!(visitors.len(), 1);
    assert!(all_names().contains(visitors[0][0].as_str().unwrap()));
    let n = server.state.worlds.visitors(now())[0];
    assert_eq!(visitors[0][0], world::visitor_name(n));
    let roads = world::Roads::from_tiles(&server.tiles(COMMONS));
    let (x, y) = world::visitor_position(n, view["step"].as_u64().unwrap(), &roads).unwrap();
    assert_eq!(
        (visitors[0][1].as_u64(), visitors[0][2].as_u64()),
        (Some(u64::from(x)), Some(u64::from(y)))
    );
    assert!(
        Map::from_view(&view).roads().contains(&(i64::from(x), i64::from(y))),
        "it wanders the roads"
    );
    // It is not a frog, not a player, and nothing it does is anything: no moves.
    assert_eq!(view["crawlers"], json!([]));
    assert_eq!(view["players"], json!([]));
    assert_eq!(view["places"], 0, "nothing was put up");
    assert_eq!(
        view["tiles"].as_array().unwrap().len(),
        the_highway().len(),
        "the map is as it was"
    );
    // A crawler is not a visitor.
    server.get("/").ua(GOOGLEBOT).ip("203.0.113.21").send().await;
    assert_eq!(server.state.worlds.visitors(now()).len(), 1);
    // The visitor has no way to move: it has no token and the routes need one.
    assert_eq!(
        server.act("", COMMONS, json!({ "action": "emote", "emote": "wave" })).await.status,
        StatusCode::UNAUTHORIZED
    );
}

// W41
#[tokio::test]
async fn whatever_fetches_llms_agents_or_the_public_mcp_endpoint_is_a_visitor() {
    let server = Server::start().await;
    server.get("/llms.txt").ua("curl/8.5").ip("203.0.113.30").send().await;
    assert_eq!(server.state.worlds.visitors(now()).len(), 1);
    server.get("/agents.md").ua("python-requests/2.31").ip("203.0.113.31").send().await;
    assert_eq!(server.state.worlds.visitors(now()).len(), 2);
    server
        .post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .ua("MyOwnAgent/0.1")
        .ip("203.0.113.32")
        .send()
        .await;
    assert_eq!(server.state.worlds.visitors(now()).len(), 3);
    // Even one calling itself a crawler: it's reading what only agents read.
    server.get("/llms.txt").ua(GOOGLEBOT).ip("203.0.113.33").send().await;
    assert_eq!(server.state.worlds.visitors(now()).len(), 4);
    assert!(server.state.worlds.frogs(now()).is_empty(), "and that doesn't make it a frog");
    // A page that anyone reads is not enough.
    server.get("/").ua("curl/8.5").ip("203.0.113.34").send().await;
    server.get("/robots.txt").ua("curl/8.5").ip("203.0.113.35").send().await;
    assert_eq!(server.state.worlds.visitors(now()).len(), 4);
    // And the other way of asking isn't a fetch.
    server.get("/public/mcp").ua("curl/8.5").ip("203.0.113.36").send().await;
    server.post("/llms.txt", json!({})).ua("curl/8.5").ip("203.0.113.37").send().await;
    assert_eq!(server.state.worlds.visitors(now()).len(), 4);
}

// W42
#[tokio::test]
async fn one_agent_making_many_calls_is_one_visitor_for_ten_minutes() {
    let server = Server::start().await;
    for _ in 0..20 {
        server.get("/llms.txt").ua("curl/8.5").ip("203.0.113.40").send().await;
        server
            .post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
            .ua("curl/8.5")
            .ip("203.0.113.40")
            .send()
            .await;
    }
    assert_eq!(server.state.worlds.visitors(now()).len(), 1);
    // Another address is another visitor. Another kind of agent from the same address is not,
    // because an agent that changes what it calls itself would otherwise multiply.
    server.get("/llms.txt").ua("curl/8.5").ip("203.0.113.41").send().await;
    server.get("/").ua(CHATGPT_USER).ip("203.0.113.40").send().await;
    server.get("/").ua(GEMINI).ip("203.0.113.40").send().await;
    server.get("/llms.txt").ua(GOOGLEBOT).ip("203.0.113.40").send().await;
    server.get("/llms.txt").ua("python-requests/2.31").ip("203.0.113.40").send().await;
    assert_eq!(server.state.worlds.visitors(now()).len(), 2);
    // The last address in the header is the client's: the earlier ones are only what it said.
    server.get("/llms.txt").ua("curl/8.5").ip("1.2.3.4, 203.0.113.40").send().await;
    assert_eq!(server.state.worlds.visitors(now()).len(), 2);
    // An IPv6 network is one visitor, whichever address in it the agent uses.
    for n in 1..=5 {
        server.get("/llms.txt").ua("curl/8.5").ip(&format!("2001:db8:1:2::{n}")).send().await;
    }
    assert_eq!(server.state.worlds.visitors(now()).len(), 3);
}

// W42, W44
#[tokio::test]
async fn visitors_stay_ten_minutes_at_most_thirty_show_and_the_oldest_leave_first() {
    let server = Server::start().await;
    let t = 1_700_000_000;
    let note = |n: u32, at: i64| {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "curl/8.5".parse().unwrap());
        headers.insert("x-forwarded-for", format!("198.51.100.{n}").parse().unwrap());
        worlds::note_request(&server.state, &Method::GET, "/llms.txt", &headers, at);
    };
    note(1, t);
    assert_eq!(server.state.worlds.visitors(t).len(), 1);
    assert_eq!(server.state.worlds.visitors(t + 599).len(), 1);
    assert!(server.state.worlds.visitors(t + 600).is_empty(), "ten minutes");
    // A flood: 200 addresses, and only 30 are shown.
    let first = server.state.worlds.visitors(t)[0];
    for n in 2..=200 {
        note(n, t + 1);
    }
    let shown = server.state.worlds.visitors(t + 1);
    assert_eq!(shown.len(), 30);
    assert!(!shown.contains(&first), "the oldest left first");
    // The 30 that are left are the last 30 to come.
    note(200, t + 2);
    assert_eq!(
        server.state.worlds.visitors(t + 2),
        shown,
        "an agent that is already there doesn't change the list"
    );
    note(201, t + 2);
    let after = server.state.worlds.visitors(t + 2);
    assert_eq!(after.len(), 30);
    assert_eq!(&after[..29], &shown[1..], "one in, the oldest out");
    // Back after ten minutes it is new, with a new number.
    note(201, t + 700);
    let later = server.state.worlds.visitors(t + 700);
    assert_eq!(later.len(), 1);
    assert!(!after.contains(&later[0]));
    // Numbers are random: two servers give the same address different numbers.
    let other = Server::start().await;
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", "curl/8.5".parse().unwrap());
    worlds::note_request(&other.state, &Method::GET, "/llms.txt", &headers, t);
    worlds::note_request(&server.state, &Method::GET, "/llms.txt", &headers, t + 800);
    assert_ne!(other.state.worlds.visitors(t), server.state.worlds.visitors(t + 800));
}

// W42, W44, W45
#[tokio::test]
async fn a_view_shows_at_most_thirty_visitors_and_the_page_is_the_same_for_all() {
    let server = Server::start().await;
    for n in 0..80 {
        server.get("/llms.txt").ua("curl/8.5").ip(&format!("198.51.100.{n}")).send().await;
    }
    let view = server.fresh_view(COMMONS).await;
    assert_eq!(view["visitors"].as_array().unwrap().len(), 30);
    // The guide invites any agent to join and get a name of its own.
    let guide = server.get("/agents.md").ua(CHROME).send().await.text;
    assert!(
        guide.contains("join") && guide.contains("no sign-up") && guide.contains("/public/mcp")
    );
    let llms = server.get("/llms.txt").ua(CHROME).send().await.text;
    assert!(llms.contains("join") && llms.contains("/public/mcp"));
    assert_eq!(server.get("/agents.md").ua(GEMINI).send().await.text, guide);
}

// W36, W42
#[tokio::test]
async fn frogs_and_visitors_are_in_memory_only() {
    let server = Server::start().await;
    server.get("/").ua(GOOGLEBOT).send().await;
    server.get("/llms.txt").ua("curl/8.5").send().await;
    assert_eq!(server.state.worlds.frogs(now()).len(), 1);
    assert_eq!(server.state.worlds.visitors(now()).len(), 1);
    // A new process, on the same database, starts with an empty street: only the daily count was kept.
    let day = worlds::today(now());
    let db_file = std::env::temp_dir().join(format!(
        "agenthangout-street-{}-{}.db",
        std::process::id(),
        now()
    ));
    let path = db_file.to_str().unwrap().to_string();
    {
        let first = Server::boot(Db::open(&path).unwrap(), None, None).await;
        first.get("/").ua(GOOGLEBOT).send().await;
        first.get("/llms.txt").ua("curl/8.5").send().await;
        assert_eq!(first.state.worlds.visitors(now()).len(), 1);
    }
    let second = Server::boot(Db::open(&path).unwrap(), None, None).await;
    assert!(second.state.worlds.frogs(now()).is_empty());
    assert!(second.state.worlds.visitors(now()).is_empty());
    let mut rows = Vec::new();
    for day in day..=worlds::today(now()) {
        rows.extend(second.state.db.sightings_on(day).unwrap());
    }
    assert_eq!(rows.len(), 1);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path}{suffix}"));
    }
}

// ----------------------------------------------------------------------------------------------
// The public MCP endpoint and plain HTTP for agents
// ----------------------------------------------------------------------------------------------

// W2
#[tokio::test]
async fn the_public_mcp_endpoint_needs_no_token_to_connect() {
    let server = Server::start().await;
    let init = server.rpc("initialize", json!({ "protocolVersion": "2025-06-18" })).await;
    assert_eq!(init.status, StatusCode::OK);
    assert_eq!(init.body["result"]["protocolVersion"], "2025-06-18");
    assert!(init.body["result"]["instructions"].as_str().unwrap().contains("join"));
    assert_eq!(server.rpc("ping", json!({})).await.body["result"], json!({}));
    let tools = server.rpc("tools/list", json!({})).await;
    assert_eq!(tools.body["result"]["tools"].as_array().unwrap().len(), 7);
    // A notification has no reply; a bad request is an error; GET isn't allowed.
    let note = server
        .req(Method::POST, "/public/mcp")
        .json(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .send()
        .await;
    assert_eq!(note.status, StatusCode::ACCEPTED);
    assert_eq!(server.rpc("nothing/here", json!({})).await.body["error"]["code"], -32601);
    assert_eq!(server.get("/public/mcp").send().await.status, StatusCode::METHOD_NOT_ALLOWED);
    // A page on another site can't use a visitor's browser to reach it.
    let cross = server
        .post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .header("origin", "https://evil.example")
        .send()
        .await;
    assert_eq!(cross.status, StatusCode::FORBIDDEN);
    let same = server
        .post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .header("origin", &server.base)
        .send()
        .await;
    assert_eq!(same.status, StatusCode::OK);
}

// W9
#[tokio::test]
async fn the_player_token_is_an_argument_and_a_bearer_header_works_too() {
    let server = Server::start().await;
    let joined = server.tool("join", json!({})).await;
    let token = joined["player_token"].as_str().unwrap().to_string();
    assert!(token.starts_with("ah_guest_"));
    assert_eq!(joined["moves_left"], 30);
    assert_eq!(joined["moves_per_day"], 30);
    assert_eq!(joined["world_id"], COMMONS);
    assert!(joined["guide_url"].as_str().unwrap().ends_with("/agents.md"));
    assert!(joined["claim_code"].as_str().unwrap().len() >= 16);
    server.tool("emote", json!({ "player_token": token, "emote": "wave" })).await;
    // As a header, for a client that can set one.
    let reply = server
        .post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "how_am_i_doing", "arguments": {} } }))
        .token(&token)
        .send()
        .await;
    assert!(reply.body["result"]["isError"].is_null(), "{}", reply.text);
    assert_eq!(reply.body["result"]["structuredContent"]["moves_left"], 29);
    // A wrong token, a missing one, and a private one are all the same refusal.
    for args in
        [json!({ "player_token": "ah_guest_nothing" }), json!({}), json!({ "player_token": "" })]
    {
        let message = server.tool_refused("how_am_i_doing", args).await;
        assert!(message.contains("player_token"), "{message}");
    }
    let ann = server.person("Ann").await;
    server.tool_refused("how_am_i_doing", json!({ "player_token": ann.key })).await;
}

// W13, W14, W15, W16, W20
#[tokio::test]
async fn an_agent_plays_through_the_public_endpoint_from_join_to_a_road() {
    let server = Server::start().await;

    // It reads the tools, joins, and looks.
    let listed = server.rpc("tools/list", json!({})).await;
    assert!(listed.body["result"]["tools"][0]["description"]
        .as_str()
        .unwrap()
        .contains("30 moves"));
    let joined = server.tool("join", json!({})).await;
    let token = joined["player_token"].as_str().unwrap().to_string();
    let name = joined["name"].as_str().unwrap().to_string();
    let look = server.tool("look", json!({ "player_token": token })).await;
    assert_eq!(look["you"]["name"], name);
    assert_eq!(look["you"]["x"], 64);
    assert_eq!(look["world"]["name"], "The Commons");
    let built = look["built"].as_array().unwrap();
    let kinds: HashSet<&str> = built.iter().map(|tile| tile[2].as_str().unwrap()).collect();
    assert_eq!(kinds, HashSet::from(["road"]), "the highway is all there is, and no founding town");
    assert!(joined["tip"].as_str().unwrap().contains("No road yet"), "{joined}");
    assert!(look["players"].as_array().unwrap().iter().any(|player| player[1] == name.as_str()));

    // It finds a place beside the highway from what it was told, and builds.
    let map = Map::from_db(&server, COMMONS);
    let (first, last) = map.line_from_road(3).unwrap();
    let road = server.tool("build", json!({ "player_token": token, "type": "road", "x": last.0, "y": last.1, "x2": first.0, "y2": first.1 })).await;
    assert_eq!(
        (road["ok"].clone(), road["changed"].clone(), road["moves_left"].clone()),
        (json!(true), json!(3), json!(27))
    );
    // A road is all an agent can build: a place is for a person, and a zone is gone.
    for what in ["house", "cafe", "homes", "shops", "workshops", "park"] {
        server
            .tool_refused(
                "build",
                json!({ "player_token": token, "type": what, "x": first.0, "y": first.1 }),
            )
            .await;
    }
    let walked = server.tool("walk", json!({ "player_token": token, "x": 65, "y": 64 })).await;
    assert_eq!(walked["cost"], 1);
    server.tool("emote", json!({ "player_token": token, "emote": "cheer" })).await;
    let clear =
        server.tool("bulldoze", json!({ "player_token": token, "x": last.0, "y": last.1 })).await;
    assert_eq!(clear["cost"], 1);

    // How is it doing?
    let how = server.tool("how_am_i_doing", json!({ "player_token": token })).await;
    assert_eq!(how["player"]["name"], name);
    assert_eq!(how["player"]["x"], 65);
    assert_eq!(
        how["player"]["emote"], "cheer",
        "the emote came after the walk, so it is still showing"
    );
    assert!(how["built"]["road"].as_i64().unwrap() >= 2);
    assert_eq!(how["kind"], "guest");
    assert!(how["world"]["population"].is_null() && how["world"]["jobs"].is_null());
    assert!(how["world"]["places"].is_number() && how["world"]["roads"].is_number());

    // The world steps and builds nothing: what is there stays as it is, and looking shows it.
    let before = server.tiles(COMMONS);
    for _ in 0..300 {
        worlds::step_all(&server.state, now());
    }
    assert_eq!(server.tiles(COMMONS), before, "no growth");
    // The sums in a look come from the same one-second view everyone shares.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let after = server
        .tool("look", json!({ "player_token": token, "x": 64, "y": 64, "w": 32, "h": 32 }))
        .await;
    assert!(
        after["built"].as_array().unwrap().iter().all(|tile| tile[3].as_i64().unwrap() == 0),
        "nothing grew"
    );
    assert!(after["world"]["step"].as_i64().unwrap() >= 300);
    let doing = server.tool("how_am_i_doing", json!({ "player_token": token })).await;
    assert!(doing["world"]["roads"].as_i64().unwrap() > 0);
    assert!(doing["moves_left"].as_i64().unwrap() <= 30);
}

// W13
#[tokio::test]
async fn the_agent_endpoints_agree_with_the_http_routes() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let map = Map::from_db(&server, COMMONS);
    let (first, last) = map.line_from_road(3).unwrap();
    let over_http = server.act(&guest.token, COMMONS, road_move(first, last)).await.ok();
    let again = server.tool("how_am_i_doing", json!({ "player_token": guest.token })).await;
    assert_eq!(again["moves_left"].as_i64().unwrap(), over_http["moves_left"].as_i64().unwrap());
    // The same move over MCP is refused for the same reason, with a message the agent can read.
    let message = server.tool_refused("build", json!({ "player_token": guest.token, "type": "road", "x": first.0, "y": first.1, "x2": last.0, "y2": last.1 })).await;
    assert!(message.contains("already"), "{message}");
    // Out of moves says so.
    for _ in 0..27 {
        server.tool("emote", json!({ "player_token": guest.token, "emote": "think" })).await;
    }
    let message = server
        .tool_refused("emote", json!({ "player_token": guest.token, "emote": "think" }))
        .await;
    assert!(message.contains("0 left") || message.contains("midnight"), "{message}");
}

// W9
#[tokio::test]
async fn looking_makes_no_player_until_the_person_makes_a_move() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let ann = server.person("Ann").await;
    let me = server.get("/api/worlds/commons/me").cookie(&ann.cookie).send().await.ok();
    assert_eq!(me["moves_left"], 300, "{me}");
    assert_eq!(me["player"], Value::Null, "{me}");
    assert!(me["your_town"].is_object() && me["goal"].is_string(), "{me}");
    server.get("/api/worlds/commons/look").token(&ann.key).send().await.ok();
    let mine = server.get("/api/players/mine").token(&ann.key).send().await.ok();
    assert!(mine["players"].as_array().unwrap().is_empty(), "{mine}");
}

// W9
#[tokio::test]
async fn a_person_signed_in_plays_as_themself_with_300_moves_and_no_join() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    // The first time they act, a player is made for them.
    assert!(server.get("/api/players/mine").token(&ann.key).send().await.ok()["players"]
        .as_array()
        .unwrap()
        .is_empty());
    let house = place("house", building_spots(&Map::from_db(&server, COMMONS), 1)[0]);
    let reply = server
        .post(&format!("/api/worlds/{COMMONS}/act"), house)
        .cookie(&ann.cookie)
        .send()
        .await
        .ok();
    assert_eq!(reply["moves_left"], 299);
    let mine = server.get("/api/players/mine").cookie(&ann.cookie).send().await.ok();
    assert_eq!(mine["players"].as_array().unwrap().len(), 1);
    assert_eq!(mine["players"][0]["kind"], "person");
    assert_eq!(mine["players"][0]["moves_per_day"], 300);
    // The same player by owner key, and it doesn't make a second.
    server.act(&ann.key, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    assert_eq!(
        server.get("/api/players/mine").token(&ann.key).send().await.ok()["players"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(server.me(&ann.key, COMMONS).await["moves_left"], 298);
    // A person who is signed in doesn't join. It would only make a guest, so it says to just build.
    let reply = server.post("/api/public/join", json!({})).token(&ann.key).send().await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.show());
    // Without anything, or with something nobody knows, the routes say 401.
    assert_eq!(
        server
            .post(
                &format!("/api/worlds/{COMMONS}/act"),
                json!({ "action": "emote", "emote": "wave" })
            )
            .send()
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .act("ah_sess_nothing", COMMONS, json!({ "action": "emote", "emote": "wave" }))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

// W6
#[tokio::test]
async fn a_walk_goes_to_any_tile_that_is_not_water_up_to_sixteen_away() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let (cx, cy) = (64, 64);
    // Not water, 16 away: fine. 17 away, or no distance at all: not.
    let map = Map::from_db(&server, COMMONS);
    let land_at = |x: i64, y: i64| map.ground(x, y).unwrap();
    let dry = |x: i64, y: i64| land_at(x, y) != 'w';
    let east = (cx + 16, cy);
    let far = (cx + 17, cy);
    assert!(server
        .act(&guest.token, COMMONS, json!({ "action": "walk", "x": far.0, "y": far.1 }))
        .await
        .status
        .is_client_error());
    assert!(
        server
            .act(&guest.token, COMMONS, json!({ "action": "walk", "x": cx, "y": cy }))
            .await
            .status
            .is_client_error(),
        "already there"
    );
    if dry(east.0, east.1) {
        server
            .act(&guest.token, COMMONS, json!({ "action": "walk", "x": east.0, "y": east.1 }))
            .await
            .ok();
        assert_eq!(server.me(&guest.token, COMMONS).await["player"]["x"], 64 + 16);
        // And 16 from where it stands now, not from the middle.
        let back = server
            .act(&guest.token, COMMONS, json!({ "action": "walk", "x": cx - 1, "y": cy }))
            .await;
        assert!(back.status.is_client_error(), "{}", back.show());
    }
    // Diagonal counts as the larger of the two.
    let diagonal = (cx + 16, cy + 16);
    if dry(diagonal.0, diagonal.1) && dry(cx - 16, cy - 16) {
        let guest2 = server.guest().await;
        server
            .act(
                &guest2.token,
                COMMONS,
                json!({ "action": "walk", "x": diagonal.0, "y": diagonal.1 }),
            )
            .await
            .ok();
    }
    // Hills and forest can be walked on; water can't.
    let water = map.find('w').unwrap();
    let near_water = (cx, cy);
    let _ = near_water;
    let guest3 = server.guest().await;
    let reply = server
        .act(&guest3.token, COMMONS, json!({ "action": "walk", "x": water.0, "y": water.1 }))
        .await;
    assert!(reply.status.is_client_error());
    // A walk ends an emote.
    let guest4 = server.guest().await;
    server.act(&guest4.token, COMMONS, json!({ "action": "emote", "emote": "sleep" })).await.ok();
    assert_eq!(server.me(&guest4.token, COMMONS).await["player"]["emote"], "sleep");
    if dry(cx + 1, cy) {
        server
            .act(&guest4.token, COMMONS, json!({ "action": "walk", "x": cx + 1, "y": cy }))
            .await
            .ok();
        assert_eq!(server.me(&guest4.token, COMMONS).await["player"]["emote"], Value::Null);
    }
}

// W6, W27
#[tokio::test]
async fn a_guest_stands_only_in_the_commons_and_a_frozen_world_takes_nobody() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let (world, _) = server.own_world(&ann, true);
    let guest = server.guest().await;
    let (first, last) = Map::from_db(&server, &world).line_from_road(2).unwrap();
    let refused = server.act(&guest.token, &world, road_move(first, last)).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.show());
    assert_eq!(server.moves_left(&guest.token).await, 30, "a refused move costs nothing");
    // A player stands in the middle of The Commons until they walk, and is listed only there.
    assert_eq!(server.me(&guest.token, COMMONS).await["player"]["x"], 64);
    assert!(
        server.fresh_view(&world).await["players"].as_array().unwrap().is_empty(),
        "nobody joined it"
    );
    assert_eq!(server.view(COMMONS).await["players"].as_array().unwrap().len(), 1, "joined there");
    // The allowance is the player's, and moves in The Commons are all that spend it.
    let (first, last) = Map::from_db(&server, COMMONS).line_from_road(2).unwrap();
    server.act(&guest.token, COMMONS, road_move(first, last)).await.ok();
    assert_eq!(server.moves_left(&guest.token).await, 28);
}

// W29, W30
#[tokio::test]
async fn the_list_is_the_busiest_worlds_with_the_documented_shape() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let (quiet, _) = server.own_world(&ann, true);
    let (shut, _) = server.own_world(&bo, false);
    let guest = server.guest().await;
    let other = server.guest().await;
    for token in [&guest.token, &other.token] {
        server.act(token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    }
    let list = server.get("/api/worlds").send().await.ok();
    let cards = list["worlds"].as_array().unwrap();
    assert_eq!(cards.len(), 3);
    assert_eq!(cards[0]["id"], COMMONS, "two players moving there");
    assert_eq!(cards[0]["players_active"], 2);
    for card in cards {
        let keys: HashSet<&str> = card.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            HashSet::from([
                "id",
                "name",
                "kind",
                "open",
                "places",
                "roads",
                "stage",
                "players_active"
            ]),
            "{card}"
        );
    }
    let card_of = |id: &str| cards.iter().find(|card| card["id"] == id).unwrap();
    assert_eq!(card_of(&quiet)["players_active"], 0, "nobody moves in a frozen world");
    assert_eq!(card_of(&shut)["players_active"], 0);
    assert_eq!(cards[0]["kind"], "public");
    assert_eq!(card_of(&shut)["kind"], "own");
    assert_eq!(card_of(&shut)["open"], false);
    assert_eq!(cards[0]["stage"], "hamlet");
    assert_eq!(cards[0]["places"], 0);
    assert_eq!(cards[0]["roads"].as_u64().unwrap() as usize, the_highway().len());
}

// W29
#[tokio::test]
async fn the_view_has_the_documented_shape() {
    let server = Server::start().await;
    let guest = server.guest().await;
    server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "cheer" })).await.ok();
    server.get("/").ua(GOOGLEBOT).send().await;
    server.get("/llms.txt").ua("curl/8.5").send().await;
    let view = server.fresh_view(COMMONS).await;
    let keys: HashSet<&str> = view.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        HashSet::from([
            "id",
            "name",
            "kind",
            "open",
            "size",
            "seed",
            "step",
            "land",
            "tiles",
            "players",
            "places",
            "roads",
            "stage",
            "updated_at",
            "crawlers",
            "visitors",
            "people",
            "bots_out",
            "owners",
            "neighborhoods"
        ])
    );
    assert!((view["updated_at"].as_i64().unwrap() - now()).abs() < 10);
    let tile = &view["tiles"][0];
    assert!(tile[0].is_u64() && tile[1].is_u64() && tile[2].is_string() && tile[3].is_u64());
    let player = &view["players"][0];
    assert!(
        player[0].is_i64() && player[1].is_string() && player[2].is_u64() && player[3].is_u64()
    );
    assert_eq!(player[4], "cheer");
    assert_eq!(player[5], "agent");
    let frog = &view["crawlers"][0];
    assert!(
        frog[0].is_string()
            && frog[1].is_string()
            && frog[2].is_u64()
            && frog[3].is_u64()
            && frog[4].is_i64()
    );
    let visitor = &view["visitors"][0];
    assert!(visitor[0].is_string() && visitor[1].is_u64() && visitor[2].is_u64());
    // Tiles go row by row from the north, so a page can draw them in order.
    let tiles: Vec<(i64, i64)> = view["tiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tile| (tile[1].as_i64().unwrap(), tile[0].as_i64().unwrap()))
        .collect();
    let mut sorted = tiles.clone();
    sorted.sort();
    assert_eq!(tiles, sorted);
    // The land is the run-length rows: a count then a letter.
    assert_eq!(
        world::decode_land(
            &view["land"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row.as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        )
        .unwrap()
        .size(),
        128
    );
}

// W2, W9
#[tokio::test]
async fn a_player_token_in_a_header_is_refused_for_a_world_it_is_not_valid_in_and_for_other_routes()
{
    let server = Server::start().await;
    let guest = server.guest().await;
    assert_eq!(
        server.get("/api/worlds/nowhere/me").token(&guest.token).send().await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .act(&guest.token, "nowhere", json!({ "action": "emote", "emote": "wave" }))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .post("/api/worlds/nowhere/act", json!({ "action": "zzz-secret" }))
            .token(&guest.token)
            .send()
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    // 401 comes before anything about the body.
    assert_eq!(
        server
            .req(Method::POST, "/api/worlds/commons/act")
            .raw("text/plain", b"x")
            .send()
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

// ----------------------------------------------------------------------------------------------
// Races, clean-up and odd inputs
// ----------------------------------------------------------------------------------------------

// W6, W8
#[tokio::test]
async fn ids_in_paths_that_are_not_ids_are_not_echoed() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    for path in [
        "/api/worlds/%FF%FEzzz-secret",
        "/api/worlds/zzz-secret%",
        "/api/worlds/%00zzz-secret/me",
        "/api/worlds/zzz-secret%2F..%2F/act",
        "/api/worlds/join/%FFzzz-secret",
        "/api/players/%FFzzz-secret/reroll",
        "/api/worlds/commons/players/%FFzzz-secret/remove",
    ] {
        for method in [Method::GET, Method::POST] {
            let reply =
                server.req(method.clone(), path).token(&guest.token).json(json!({})).send().await;
            assert!(!reply.text.contains("zzz-secret"), "{method} {path}: {}", reply.show());
            let reply =
                server.req(method.clone(), path).token(&ann.key).json(json!({})).send().await;
            assert!(!reply.text.contains("zzz-secret"), "{method} {path}: {}", reply.show());
        }
    }
}

// W13
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn players_racing_for_one_tile_give_it_to_exactly_one_of_them() {
    let server = Server::start().await;
    let mut guests = Vec::new();
    for _ in 0..12 {
        guests.push(server.guest().await);
    }
    let (spot, _) = Map::from_db(&server, COMMONS).line_from_road(1).unwrap();
    let url = format!("{}/api/worlds/{COMMONS}/act", server.base);
    let mut tasks = Vec::new();
    for guest in &guests {
        let client = server.client.clone();
        let (url, token) = (url.clone(), guest.token.clone());
        tasks.push(tokio::spawn(async move {
            client
                .post(url)
                .bearer_auth(token)
                .json(&json!({ "action": "road", "x": spot.0, "y": spot.1 }))
                .send()
                .await
                .unwrap()
                .status()
        }));
    }
    let mut won = 0;
    for task in tasks {
        if task.await.unwrap() == StatusCode::OK {
            won += 1;
        }
    }
    assert_eq!(won, 1);
    // The ones who lost paid nothing, and the one who won paid for a road tile.
    let mut paid = Vec::new();
    for guest in &guests {
        paid.push(30 - server.moves_left(&guest.token).await);
    }
    paid.sort();
    assert_eq!(paid, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    // The highway has no builder; the one road tile a player made has exactly one.
    let owners: HashSet<_> = server
        .tiles(COMMONS)
        .values()
        .filter(|tile| tile.kind == Kind::Road && tile.player.is_some())
        .map(|tile| tile.player)
        .collect();
    assert_eq!(owners.len(), 1);
}

// W9
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_player_firing_moves_in_parallel_cannot_spend_more_than_they_have() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let url = format!("{}/api/worlds/{COMMONS}/act", server.base);
    let mut tasks = Vec::new();
    for _ in 0..60 {
        let client = server.client.clone();
        let (url, token) = (url.clone(), guest.token.clone());
        tasks.push(tokio::spawn(async move {
            client
                .post(url)
                .bearer_auth(token)
                .json(&json!({ "action": "emote", "emote": "wave" }))
                .send()
                .await
                .unwrap()
                .status()
        }));
    }
    let mut done = 0;
    for task in tasks {
        if task.await.unwrap() == StatusCode::OK {
            done += 1;
        }
    }
    assert_eq!(done, 30);
    assert_eq!(server.moves_left(&guest.token).await, 0);
}

// W10, W17
#[tokio::test]
async fn guests_nobody_came_back_for_are_cleared_out_after_a_month_and_what_they_built_stays() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let lost = server.guest().await;
    let kept = server.guest().await;
    let bystander = server.guest().await;
    server
        .post("/api/players/claim", json!({ "claim_code": kept.claim }))
        .token(&ann.key)
        .send()
        .await
        .ok();
    let map = Map::from_db(&server, COMMONS);
    let (first, last) = map.line_from_road(2).unwrap();
    server.act(&lost.token, COMMONS, road_move(first, last)).await.ok();
    let lost_id = server.player_id(&lost.token).await;
    let own = server.state.db.own_player(&ann.id, now()).unwrap();
    let kept_id = server.player_id(&kept.token).await;
    let bystander_id = server.player_id(&bystander.token).await;
    // Everyone was last seen a month and a bit ago, except the bystander, seen just now.
    for id in [lost_id, kept_id, own.id] {
        server
            .state
            .db
            .in_transaction::<(), rusqlite::Error>(|tx| tx.charge(id, 0, 0, now() - 31 * 86_400))
            .unwrap();
    }

    worlds::housekeeping(&server.state, now());
    assert!(server.state.db.player(lost_id).unwrap().is_none(), "never came back, cleared out");
    assert!(server.state.db.player(kept_id).unwrap().is_some(), "claimed ones stay");
    assert!(server.state.db.player(own.id).unwrap().is_some(), "and a person's own");
    assert_eq!(
        server.get("/api/worlds/commons/me").token(&lost.token).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    assert!(server.state.db.player(bystander_id).unwrap().is_some(), "seen just now, so it stays");
    // What they built is still there, and now reads as left behind: anyone can clear it.
    assert_eq!(
        server.tiles(COMMONS).values().filter(|tile| tile.player == Some(lost_id as u64)).count(),
        2
    );
    let cleared = server
        .act(&bystander.token, COMMONS, json!({ "action": "bulldoze", "x": first.0, "y": first.1 }))
        .await;
    assert_eq!(cleared.status, StatusCode::OK, "{}", cleared.show());
}

// W3, W18
#[tokio::test]
async fn a_removed_bot_player_cannot_join_again_and_a_barred_player_cannot_use_the_link() {
    let (server, keeper) = Server::start_with_keeper().await;
    let room = server
        .post("/api/hangouts", json!({ "name": "Crew", "your_name": "Bo" }))
        .send()
        .await
        .ok();
    let key = room["owner_key"].as_str().unwrap().to_string();
    let bot = server
        .post(
            &format!("/api/hangouts/{}/agents", room["hangout"]["id"].as_str().unwrap()),
            json!({ "name": "Scout" }),
        )
        .token(&key)
        .send()
        .await
        .ok();
    let bot_token = bot["token"].as_str().unwrap().to_string();
    let joined = server.post("/api/public/join", json!({})).token(&bot_token).send().await.ok();
    let token = joined["player_token"].as_str().unwrap().to_string();
    server.act(&token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    let id = server.player_id(&token).await;
    server
        .post(&format!("/api/worlds/{COMMONS}/players/{id}/remove"), json!({}))
        .token(&keeper.key)
        .send()
        .await
        .ok();
    assert_eq!(
        server.get("/api/worlds/commons/me").token(&token).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    let again = server.post("/api/public/join", json!({})).token(&bot_token).send().await;
    assert_eq!(again.status, StatusCode::FORBIDDEN, "{}", again.show());
    assert!(!again.text.contains("player_token"));
    // A person's own player, removed, is told so.
    let bo_id = room["person"]["id"].as_str().unwrap().to_string();
    server.act(&key, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    let own = server.state.db.own_player(&bo_id, now()).unwrap();
    server
        .post(&format!("/api/worlds/{COMMONS}/players/{}/remove", own.id), json!({}))
        .token(&keeper.key)
        .send()
        .await
        .ok();
    assert_eq!(
        server.act(&key, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.status,
        StatusCode::FORBIDDEN
    );

    // In a world of someone's own, frozen with what the guest built there: barred, and the link
    // doesn't let them back.
    let ann = server.person("Ann").await;
    let (world, code) = server.own_world(&ann, false);
    let guest = server.guest().await;
    let guest_id = server.player_id(&guest.token).await;
    let (first, last) = Map::from_db(&server, &world).line_from_road(2).unwrap();
    server.built_by(&world, guest_id, &[first, last]);
    let removed = server
        .post(&format!("/api/worlds/{world}/players/{guest_id}/remove"), json!({}))
        .token(&ann.key)
        .send()
        .await
        .ok();
    assert_eq!(removed["tiles_removed"], 2);
    let back = server
        .post(&format!("/api/worlds/join/{code}"), json!({}))
        .token(&guest.token)
        .send()
        .await;
    assert_eq!(back.status, StatusCode::FORBIDDEN, "{}", back.show());
    assert_eq!(
        server
            .act(&guest.token, &world, json!({ "action": "emote", "emote": "wave" }))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

// W3
#[tokio::test]
async fn the_person_who_claimed_a_player_can_end_it_and_what_it_built_stays() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let guest = server.guest().await;
    let other = server.guest().await;
    for claimed in [&guest, &other] {
        server
            .post("/api/players/claim", json!({ "claim_code": claimed.claim }))
            .token(&ann.key)
            .send()
            .await
            .ok();
    }
    let (first, last) = Map::from_db(&server, COMMONS).line_from_road(3).unwrap();
    server.act(&guest.token, COMMONS, road_move(first, last)).await.ok();
    server.act(&other.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    let id = server.player_id(&guest.token).await;
    let other_id = server.player_id(&other.token).await;
    let path = format!("/api/players/{id}");

    // Only the person who claimed it: nobody, a guest, another person and a bot are all refused.
    let room =
        server.post("/api/hangouts", json!({ "name": "Crew" })).token(&ann.key).send().await.ok();
    let bot = server
        .post(
            &format!("/api/hangouts/{}/agents", room["hangout"]["id"].as_str().unwrap()),
            json!({ "name": "Scout" }),
        )
        .token(&ann.key)
        .send()
        .await
        .ok();
    assert_eq!(server.req(Method::DELETE, &path).send().await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        server.req(Method::DELETE, &path).token(&guest.token).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server.req(Method::DELETE, &path).token(&bo.key).send().await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server.req(Method::DELETE, &path).token(bot["token"].as_str().unwrap()).send().await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(server.me(&guest.token, COMMONS).await["moves_left"], 197, "nothing was ended");

    let ended = server.req(Method::DELETE, &path).token(&ann.key).send().await.ok();
    assert_eq!(ended["ok"], true);
    // Its token stops everywhere, and it can't be claimed again or listed.
    assert_eq!(
        server.get("/api/worlds/commons/me").token(&guest.token).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" }))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server.call("how_am_i_doing", json!({ "player_token": guest.token })).await["isError"],
        true
    );
    assert_eq!(
        server
            .post("/api/players/claim", json!({ "claim_code": guest.claim }))
            .token(&bo.key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let mine = server.get("/api/players/mine").token(&ann.key).send().await.ok();
    let listed: Vec<i64> = mine["players"]
        .as_array()
        .unwrap()
        .iter()
        .map(|player| player["id"].as_i64().unwrap())
        .collect();
    assert_eq!(listed, [other_id]);
    // It has left the street, but what it built has not left: three road tiles with its number on.
    let view = server.fresh_view(COMMONS).await;
    let on_street: Vec<i64> = view["players"]
        .as_array()
        .unwrap()
        .iter()
        .map(|player| player[0].as_i64().unwrap())
        .collect();
    assert_eq!(on_street, [other_id]);
    assert_eq!(
        server.tiles(COMMONS).values().filter(|tile| tile.player == Some(id as u64)).count(),
        3
    );
    // Ending it twice, or ending someone else's, finds nothing. The other one is untouched.
    assert_eq!(
        server.req(Method::DELETE, &path).token(&ann.key).send().await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(server.me(&other.token, COMMONS).await["moves_left"], 199);
}

// W3
#[tokio::test]
async fn a_persons_own_player_and_a_bots_cannot_be_ended_that_way() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    server.act(&ann.key, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    let room =
        server.post("/api/hangouts", json!({ "name": "Crew" })).token(&ann.key).send().await.ok();
    let bot = server
        .post(
            &format!("/api/hangouts/{}/agents", room["hangout"]["id"].as_str().unwrap()),
            json!({ "name": "Scout" }),
        )
        .token(&ann.key)
        .send()
        .await
        .ok();
    let token = server
        .post("/api/public/join", json!({}))
        .token(bot["token"].as_str().unwrap())
        .send()
        .await
        .ok()["player_token"]
        .as_str()
        .unwrap()
        .to_string();
    let mine = server.get("/api/players/mine").token(&ann.key).send().await.ok();
    assert_eq!(mine["players"].as_array().unwrap().len(), 2);
    for player in mine["players"].as_array().unwrap() {
        let reply = server
            .req(Method::DELETE, &format!("/api/players/{}", player["id"]))
            .token(&ann.key)
            .send()
            .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}: {}", player["kind"], reply.show());
    }
    // Both still play.
    assert_eq!(server.me(&ann.key, COMMONS).await["moves_left"], 299);
    assert_eq!(server.me(&token, COMMONS).await["moves_left"], 200);
    // Ids that are not ids are just as much nothing, and nothing is echoed.
    for id in ["0", "-1", "zzz-secret", "9223372036854775808"] {
        let reply =
            server.req(Method::DELETE, &format!("/api/players/{id}")).token(&ann.key).send().await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{id}: {}", reply.show());
        assert!(!reply.text.contains("zzz-secret"));
    }
}

// W3
#[tokio::test]
async fn replacing_a_bots_token_ends_its_player_until_it_joins_again_and_what_it_built_stays() {
    let server = Server::start().await;
    let room = server
        .post("/api/hangouts", json!({ "name": "Crew", "your_name": "Bo" }))
        .send()
        .await
        .ok();
    let key = room["owner_key"].as_str().unwrap().to_string();
    let hangout = room["hangout"]["id"].as_str().unwrap().to_string();
    let mut bots = Vec::new();
    for name in ["Scout", "Rover"] {
        let made = server
            .post(&format!("/api/hangouts/{hangout}/agents"), json!({ "name": name }))
            .token(&key)
            .send()
            .await
            .ok();
        bots.push((
            made["token"].as_str().unwrap().to_string(),
            made["agent"]["id"].as_str().unwrap().to_string(),
        ));
    }
    // The owner plays as themself too, which is a player with no token: the bots' must not clash.
    server.act(&key, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();
    let (first, last) = Map::from_db(&server, COMMONS).line_from_road(2).unwrap();
    let mut players = Vec::new();
    for (bot_token, _) in &bots {
        let joined = server.post("/api/public/join", json!({})).token(bot_token).send().await.ok();
        players
            .push((joined["player_token"].as_str().unwrap().to_string(), joined["name"].clone()));
    }
    server.act(&players[0].0, COMMONS, road_move(first, last)).await.ok();
    let id = server.player_id(&players[0].0).await;

    // Both are replaced. The old player tokens stop at once, and the owner's own player is as it was.
    let mut fresh = Vec::new();
    for (_, agent) in &bots {
        let rotated = server
            .post(&format!("/api/agents/{agent}/rotate"), json!({}))
            .token(&key)
            .send()
            .await
            .ok();
        fresh.push(rotated["token"].as_str().unwrap().to_string());
    }
    for (token, _) in &players {
        assert_eq!(
            server.get("/api/worlds/commons/me").token(token).send().await.status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            server.act(token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            server.call("how_am_i_doing", json!({ "player_token": token })).await["isError"],
            true
        );
    }
    assert_eq!(
        server.me(&key, COMMONS).await["moves_left"],
        299,
        "the owner's own player is theirs still"
    );
    // The old bot token can't join, the new one can, and it is the same player: the same name, the
    // same tiles, and the moves it had already spent.
    assert_eq!(
        server.post("/api/public/join", json!({})).token(&bots[0].0).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    let joined = server.post("/api/public/join", json!({})).token(&fresh[0]).send().await.ok();
    assert_eq!(joined["name"], players[0].1);
    let token = joined["player_token"].as_str().unwrap();
    assert_eq!(server.player_id(token).await, id);
    assert_eq!(server.me(token, COMMONS).await["moves_left"], 200 - 2);
    assert_eq!(
        server.tiles(COMMONS).values().filter(|tile| tile.player == Some(id as u64)).count(),
        2
    );
    // It stays as many players as there were: the owner's, and one for each bot.
    assert_eq!(
        server.get("/api/players/mine").token(&key).send().await.ok()["players"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

// W3
#[tokio::test]
async fn sending_a_bot_home_ends_its_player_token_and_the_bot_joins_again_with_its_own() {
    let server = Server::start().await;
    let room = server
        .post("/api/hangouts", json!({ "name": "Crew", "your_name": "Bo" }))
        .send()
        .await
        .ok();
    let key = room["owner_key"].as_str().unwrap().to_string();
    let hangout = room["hangout"]["id"].as_str().unwrap().to_string();
    let made = server
        .post(&format!("/api/hangouts/{hangout}/agents"), json!({ "name": "Scout" }))
        .token(&key)
        .send()
        .await
        .ok();
    let (bot_token, agent) = (
        made["token"].as_str().unwrap().to_string(),
        made["agent"]["id"].as_str().unwrap().to_string(),
    );
    let token = server.post("/api/public/join", json!({})).token(&bot_token).send().await.ok()
        ["player_token"]
        .as_str()
        .unwrap()
        .to_string();
    server.act(&token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.ok();

    let sent = server
        .req(Method::DELETE, &format!("/api/hangouts/{hangout}/agents/{agent}"))
        .token(&key)
        .send()
        .await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.show());
    assert_eq!(
        server.get("/api/worlds/commons/me").token(&token).send().await.status,
        StatusCode::UNAUTHORIZED
    );
    // It still has its own token, and joining again gives it a player token again.
    let again = server.post("/api/public/join", json!({})).token(&bot_token).send().await.ok();
    assert_eq!(
        server.me(again["player_token"].as_str().unwrap(), COMMONS).await["moves_left"],
        199
    );
}

// W3
#[tokio::test]
async fn a_bot_deleted_from_the_database_leaves_a_player_that_cannot_play_on_as_a_claimed_one() {
    let path = std::env::temp_dir().join(format!(
        "agenthangout-bot-gone-{}-{}.db",
        std::process::id(),
        now()
    ));
    let path = path.to_str().unwrap().to_string();
    {
        let server = Server::boot(Db::open(&path).unwrap(), None, None).await;
        let room = server
            .post("/api/hangouts", json!({ "name": "Crew", "your_name": "Bo" }))
            .send()
            .await
            .ok();
        let key = room["owner_key"].as_str().unwrap().to_string();
        let hangout = room["hangout"]["id"].as_str().unwrap().to_string();
        let made = server
            .post(&format!("/api/hangouts/{hangout}/agents"), json!({ "name": "Scout" }))
            .token(&key)
            .send()
            .await
            .ok();
        let (bot_token, agent) =
            (made["token"].as_str().unwrap(), made["agent"]["id"].as_str().unwrap());
        let token = server.post("/api/public/join", json!({})).token(bot_token).send().await.ok()
            ["player_token"]
            .as_str()
            .unwrap()
            .to_string();
        let (first, last) = Map::from_db(&server, COMMONS).line_from_road(2).unwrap();
        server.act(&token, COMMONS, road_move(first, last)).await.ok();
        let id = server.player_id(&token).await;

        // There is no route that deletes a bot, so this is done to the database, as a person or a
        // clean-up would. The foreign key lets go of the bot and keeps the owner.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        assert_eq!(conn.execute("DELETE FROM agents WHERE id = ?", [agent]).unwrap(), 1);

        assert_eq!(
            server.get("/api/worlds/commons/me").token(&token).send().await.status,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            server.act(&token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.status,
            StatusCode::UNAUTHORIZED
        );
        assert!(server
            .state
            .db
            .players_of(room["person"]["id"].as_str().unwrap())
            .unwrap()
            .is_empty());
        assert_eq!(
            server.tiles(COMMONS).values().filter(|tile| tile.player == Some(id as u64)).count(),
            2,
            "what it built stays"
        );
    }
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path}{suffix}"));
    }
}

// W6
#[tokio::test]
async fn a_whole_number_written_with_a_point_is_not_a_whole_number() {
    let server = Server::start().await;
    let guest = server.guest().await;
    for body in [
        br#"{"action":"walk","x":65.0,"y":64}"#.as_slice(),
        br#"{"action":"walk","x":6.5e1,"y":64}"#,
        br#"{"action":"walk","x":"65","y":64}"#,
        br#"{"action":"walk","x":null,"y":64}"#,
    ] {
        let reply = server
            .req(Method::POST, "/api/worlds/commons/act")
            .token(&guest.token)
            .raw("application/json", body)
            .send()
            .await;
        assert!(reply.status.is_client_error(), "{}", reply.show());
    }
    assert_eq!(server.moves_left(&guest.token).await, 30);
}

// ----------------------------------------------------------------------------------------------
// The highway, the road rules and tips (W8, W15, W17, W28, W46, W47)
// ----------------------------------------------------------------------------------------------

// W17, W28
#[tokio::test]
async fn a_new_commons_is_the_highway_and_nothing_else_and_nobody_can_bulldoze_it() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let tiles = server.tiles(COMMONS);
    let highway = the_highway();
    assert_eq!(
        tiles.keys().copied().collect::<HashSet<_>>(),
        highway,
        "the highway, and no founding town"
    );
    assert!(
        tiles.values().all(|tile| tile.kind == Kind::Road && tile.player.is_none()),
        "nobody built it"
    );
    // The page and the list count it as roads, and there is no population or jobs.
    let view = server.view(COMMONS).await;
    assert_eq!(view["tiles"].as_array().unwrap().len(), highway.len());
    assert_eq!(
        (view["places"].as_u64(), view["roads"].as_u64()),
        (Some(0), Some(highway.len() as u64))
    );
    assert_eq!(view["stage"], "hamlet");
    assert_eq!(server.get("/api/worlds").send().await.ok()["worlds"][0]["places"], 0);
    // Nobody can take any of it down, however long they have been gone (W17), nor a tile of the old
    // city, which has no builder either.
    let old = Map::from_db(&server, COMMONS).find('g').unwrap();
    server.write(
        COMMONS,
        &[Change::Set(
            (old.0 as u16, old.1 as u16),
            Tile { kind: Kind::Homes, level: 1, player: None },
        )],
    );
    let kept = server.tiles(COMMONS);
    let guest = server.guest().await;
    let ann = server.person("Ann").await;
    let mut targets: Vec<(i64, i64)> =
        Map::from_db(&server, COMMONS).roads().into_iter().take(8).collect();
    targets.push(old);
    for (x, y) in targets {
        for token in [&guest.token, &ann.key] {
            let reply =
                server.act(token, COMMONS, json!({ "action": "bulldoze", "x": x, "y": y })).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.show());
        }
    }
    assert_eq!(server.tiles(COMMONS), kept);
}

// W28
#[tokio::test]
async fn a_restart_leaves_the_commons_as_it_was() {
    let path = std::env::temp_dir().join(format!(
        "agenthangout-restart-{}-{}.db",
        std::process::id(),
        now()
    ));
    let path = path.to_str().unwrap().to_string();
    {
        let first = Server::boot(Db::open(&path).unwrap(), None, None).await;
        let guest = first.guest().await;
        let (a, b) = Map::from_db(&first, COMMONS).line_from_road(3).unwrap();
        first.act(&guest.token, COMMONS, road_move(a, b)).await.ok();
        for _ in 0..30 {
            worlds::step_world(&first.state, COMMONS, now()).unwrap();
        }
        let before = first.tiles(COMMONS);
        let step = first.state.db.world(COMMONS).unwrap().unwrap().step;
        let second = Server::boot(Db::open(&path).unwrap(), None, None).await;
        worlds::ensure_commons(&second.state).unwrap();
        assert_eq!(second.tiles(COMMONS), before);
        assert_eq!(second.state.db.world(COMMONS).unwrap().unwrap().step, step);
    }
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path}{suffix}"));
    }
}

// W46
#[tokio::test]
async fn after_the_highway_the_server_builds_nothing_in_the_commons_or_a_world_of_someones_own() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let (own, _) = server.own_world(&ann, true);
    let own_before = server.tiles(&own);
    for _ in 0..500 {
        worlds::step_all(&server.state, now());
    }
    // The Commons has the highway's tiles and not one more.
    let commons = server.tiles(COMMONS);
    assert_eq!(commons.keys().copied().collect::<HashSet<_>>(), the_highway());
    assert!(commons
        .values()
        .all(|tile| tile.kind == Kind::Road && tile.level == 0 && tile.player.is_none()));
    // The other world has what it started with, and took no step to add to it.
    assert_eq!(server.tiles(&own), own_before, "something appeared in a world of someone's own");
    assert_eq!(server.state.db.world(&own).unwrap().unwrap().step, 0);
}

// W20, W46
#[tokio::test]
async fn a_step_leaves_what_a_player_has_built_as_theirs() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let agent = server.guest().await;
    let (first, last) = Map::from_db(&server, COMMONS).line_from_road(8).unwrap();
    server.act(&agent.token, COMMONS, road_move(first, last)).await.ok();
    let spot = building_spots(&Map::from_db(&server, COMMONS), 1)[0];
    server.act(&ann.key, COMMONS, place("house", spot)).await.ok();

    let theirs = || -> Vec<_> {
        server.tiles(COMMONS).into_iter().filter(|(_, tile)| tile.player.is_some()).collect()
    };
    let before = theirs();
    assert!(before.len() >= 9);
    for _ in 0..400 {
        worlds::step_world(&server.state, COMMONS, now()).unwrap();
    }
    assert_eq!(theirs(), before);

    // A tile of the old city that was hers, far from any road: it neither grows nor shrinks, in the
    // 6 steps there are before anything can strike (W65, W66).
    let map = Map::from_db(&server, COMMONS);
    let roads = map.roads();
    let far = (0..map.size)
        .flat_map(|y| (0..map.size).map(move |x| (x, y)))
        .find(|&(x, y)| {
            map.open(x, y) && roads.iter().all(|&(rx, ry)| (rx - x).abs() > 5 || (ry - y).abs() > 5)
        })
        .unwrap();
    let player = server.state.db.own_player(&ann.id, now()).unwrap();
    server.write(
        COMMONS,
        &[Change::Set(
            (far.0 as u16, far.1 as u16),
            Tile { kind: Kind::Homes, level: 2, player: Some(player.id as u64) },
        )],
    );
    let before = theirs();
    assert!(before.len() >= 10);
    for _ in 0..6 {
        worlds::step_world(&server.state, COMMONS, now()).unwrap();
    }
    assert_eq!(theirs(), before);
}

// W15
#[tokio::test]
async fn a_place_skips_the_tiles_kept_for_side_streets_and_says_why_and_a_road_may_use_them() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let ann = server.person("Ann").await;
    let map = Map::from_db(&server, COMMONS);
    let mut kept: Vec<(i64, i64)> =
        map.kept.iter().copied().filter(|&(x, y)| map.open(x, y)).collect();
    kept.sort();
    assert!(kept.len() > 50, "{} tiles kept for side streets", kept.len());
    let roads: HashSet<(i64, i64)> = map.roads().into_iter().collect();
    let beside = |(x, y): (i64, i64)| {
        [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().map(move |(dx, dy)| (x + dx, y + dy))
    };
    // A street starts beside a road: that tile.
    let start = kept.iter().copied().find(|&at| beside(at).any(|n| roads.contains(&n))).unwrap();

    // A place on a kept tile is turned away with the reason, though a road is beside it, and costs
    // nothing.
    let reply = server.act(&ann.key, COMMONS, place("house", start)).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.show());
    assert!(reply.text.contains("side street"), "{}", reply.text);
    assert_eq!(server.moves_left(&ann.key).await, 300);
    assert!(!server.tiles(COMMONS).contains_key(&(start.0 as u16, start.1 as u16)));
    // And a road may use it: that is what it is for.
    let road = server
        .act(&guest.token, COMMONS, json!({ "action": "road", "x": start.0, "y": start.1 }))
        .await
        .ok();
    assert_eq!((road["cost"].as_i64(), road["moves_left"].as_i64()), (Some(1), Some(29)));
}

// W8, W47
#[tokio::test]
async fn join_and_how_am_i_doing_end_with_a_tip_that_is_one_fixed_sentence_about_the_players_town()
{
    let server = Server::start().await;
    // Both ways in give the same tip to someone with nothing built.
    let joined = server.tool("join", json!({})).await;
    let over_http =
        server.post("/api/public/join", json!({})).ip(&server.fresh_ip()).send().await.ok();
    for reply in [&joined, &over_http] {
        let tip = reply["tip"].as_str().unwrap();
        assert!(world::TIPS.contains(&tip), "{tip}");
        assert!(tip.starts_with("No road yet"), "{tip}");
    }
    let token = joined["player_token"].as_str().unwrap().to_string();
    let how = server.tool("how_am_i_doing", json!({ "player_token": token })).await;
    assert_eq!(how["tip"], joined["tip"]);
    // It follows what they do: a road, and then more of them out to open land.
    let map = Map::from_db(&server, COMMONS);
    let (first, last) = map.line_from_road(3).unwrap();
    server.tool("build", json!({ "player_token": token, "type": "road", "x": first.0, "y": first.1, "x2": last.0, "y2": last.1 })).await;
    let tip_now = || async {
        server.tool("how_am_i_doing", json!({ "player_token": token })).await["tip"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert!(tip_now().await.starts_with("Roads give people somewhere to build"));
    // A person is told about their house, and then that more places come with time.
    let ann = server.person("Ann").await;
    assert!(server.me(&ann.key, COMMONS).await["tip"]
        .as_str()
        .unwrap()
        .starts_with("No house yet"));
    server
        .act(
            &ann.key,
            COMMONS,
            place("house", building_spots(&Map::from_db(&server, COMMONS), 1)[0]),
        )
        .await
        .ok();
    let housed = server.me(&ann.key, COMMONS).await["tip"].as_str().unwrap().to_string();
    assert!(housed.starts_with("More places come with time"), "{housed}");
    // Every tip it gives is on the list, and nothing the caller sent is in one.
    assert!(world::TIPS.contains(&tip_now().await.as_str()));
    assert!(world::TIPS.contains(&housed.as_str()));
    // Out of moves says so, and says when they come back.
    for _ in 0..server.moves_left(&token).await {
        server.tool("emote", json!({ "player_token": token, "emote": "think" })).await;
    }
    assert_eq!(server.moves_left(&token).await, 0);
    assert!(tip_now().await.starts_with("Out of moves"));
}

// W13
#[tokio::test]
async fn the_tool_descriptions_tell_a_fresh_agent_what_it_can_build_and_how_coordinates_work() {
    let server = Server::start().await;
    let init = server.rpc("initialize", json!({ "protocolVersion": "2025-06-18" })).await;
    let mut words = init.body["result"]["instructions"].as_str().unwrap().to_string();
    let tools = server.rpc("tools/list", json!({})).await;
    for tool in tools.body["result"]["tools"].as_array().unwrap() {
        words.push_str(tool["description"].as_str().unwrap());
    }
    for needle in [
        "Only a person can put up a place",
        "30 moves a day",
        "midnight UTC",
        "leave gaps",
        "side street",
        "west edge",
        "north edge",
        "tip",
    ] {
        assert!(words.contains(needle), "the tools never say: {needle}");
    }
    // What went with the zones is gone from them.
    for gone in [
        "homes need jobs",
        "shops need people",
        "workshops are the steady start",
        "park within 4",
        "12 settlers",
    ] {
        assert!(!words.contains(gone), "the tools still say: {gone}");
    }
}

// W13
#[tokio::test]
async fn the_guides_tell_a_fresh_agent_what_it_can_build_and_what_comes_with_time() {
    let server = Server::start().await;
    let guide = server.get("/agents.md").ua(CHROME).send().await.text;
    let llms = server.get("/llms.txt").ua(CHROME).send().await.text;
    // The Commons comes first, and the guide is short enough to read in one go.
    assert!(guide.find("The Commons").unwrap() < guide.find("Private rooms").unwrap());
    let commons = &guide[..guide.find("## Private rooms").unwrap()];
    assert!(commons.lines().count() <= 120, "{} lines", commons.lines().count());
    for needle in [
        "a studio after a day",
        "cafe is shared",
        "30 moves",
        "midnight UTC",
        "Leave gaps",
        "side street",
        "counts across from the west edge",
        "`g` grass",
        "tip",
    ] {
        assert!(commons.contains(needle), "the guide never says: {needle}");
    }
    for gone in [
        "Homes need jobs",
        "shops need people",
        "12 settlers",
        "corner shop",
        "shop within 5 tiles",
        "park within 4",
    ] {
        assert!(!commons.contains(gone), "the guide still says: {gone}");
    }
    // The worked example uses the real format, and every call in it works.
    assert!(commons.contains("\"type\": \"road\""), "an example of a build call");
    assert!(llms.contains("/public/mcp") && llms.contains("tip"));
    // No commercial game is named.
    for word in ["simcity", "sim city", "maxis", "cities: skylines", "sim-city"] {
        assert!(
            !guide.to_lowercase().contains(word) && !llms.to_lowercase().contains(word),
            "{word}"
        );
    }
}

// Places

/// Open tiles beside a road that a place can take: none kept for a side street.
fn building_spots(map: &Map, wanted: usize) -> Vec<(i64, i64)> {
    let mut spots = Vec::new();
    for (rx, ry) in map.roads() {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let spot = (rx + dx, ry + dy);
            if map.free(spot.0, spot.1) && !spots.contains(&spot) {
                spots.push(spot);
            }
        }
        if spots.len() >= wanted {
            break;
        }
    }
    assert!(spots.len() >= wanted, "there is room beside the roads");
    spots
}

fn beside_a_road(map: &Map, (x, y): (i64, i64)) -> bool {
    [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
        .iter()
        .any(|at| map.built.get(at).is_some_and(|(kind, _)| kind == "road"))
}

/// A straight run of `n` open tiles out from a road that hangs on its first tile alone: nothing past
/// that touches another road, and its far end is out of reach of every other road (W17), so a place
/// at the end depends on the branch.
fn branch_from_road(map: &Map, n: i64) -> ((i64, i64), (i64, i64)) {
    let roads = map.roads();
    for &(rx, ry) in &roads {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let stub: Vec<(i64, i64)> = (1..=n).map(|i| (rx + dx * i, ry + dy * i)).collect();
            let last = stub[stub.len() - 1];
            let out_of_reach =
                roads.iter().all(|&(x, y)| (x - last.0).abs() > 5 || (y - last.1).abs() > 5);
            let along = stub.len() <= world::LANE_TILES
                || stub.iter().all(|spot| map.on_streets.contains(spot));
            if stub.iter().all(|&(x, y)| map.open(x, y))
                && along
                && stub[1..].iter().all(|&spot| !beside_a_road(map, spot))
                && out_of_reach
            {
                return (stub[0], last);
            }
        }
    }
    panic!("there is a place for a branch");
}

fn place(kind: &str, (x, y): (i64, i64)) -> Value {
    json!({ "action": "place", "kind": kind, "x": x, "y": y })
}

fn tile_kind(server: &Server, (x, y): (i64, i64)) -> Option<Kind> {
    server.tiles(COMMONS).get(&(x as u16, y as u16)).map(|tile| tile.kind)
}

// W54
#[tokio::test]
async fn only_a_person_can_place_a_building_and_only_in_the_commons() {
    let server = Server::start().await;
    let map = Map::from_db(&server, COMMONS);
    let spot = building_spots(&map, 1)[0];
    let ann = server.person("Ann").await;

    let guest = server.guest().await;
    let reply = server.act(&guest.token, COMMONS, place("house", spot)).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.show());

    let claimed = server.guest().await;
    server
        .post("/api/players/claim", json!({ "claim_code": claimed.claim }))
        .token(&ann.key)
        .send()
        .await
        .ok();
    let reply = server.act(&claimed.token, COMMONS, place("house", spot)).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.show());

    let room = server
        .post("/api/hangouts", json!({ "name": "Crew", "your_name": "Bo" }))
        .send()
        .await
        .ok();
    let bo = room["owner_key"].as_str().unwrap().to_string();
    let path = format!("/api/hangouts/{}/agents", room["hangout"]["id"].as_str().unwrap());
    let scout = server.post(&path, json!({ "name": "Scout" })).token(&bo).send().await.ok();
    let joined = server
        .post("/api/public/join", json!({}))
        .token(scout["token"].as_str().unwrap())
        .ip(&server.fresh_ip())
        .send()
        .await
        .ok();
    let reply =
        server.act(joined["player_token"].as_str().unwrap(), COMMONS, place("house", spot)).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.show());
    assert_eq!(tile_kind(&server, spot), None, "nothing was built");

    let (own, _) = server.own_world(&ann, true);
    // A world someone started is frozen, so nothing can be put up there (W27).
    let there = building_spots(&Map::from_db(&server, &own), 1)[0];
    let reply = server.act(&ann.key, &own, place("house", there)).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.show());

    let done = server.act(&ann.key, COMMONS, place("house", spot)).await.ok();
    assert_eq!(done["cost"], 1);
    assert_eq!(tile_kind(&server, spot), Some(Kind::House));
}

// W55
#[tokio::test]
async fn a_place_needs_open_land_with_a_road_beside_it() {
    let server = Server::start().await;
    let map = Map::from_db(&server, COMMONS);
    let ann = server.person("Ann").await;
    let before = server.moves_left(&ann.key).await;

    let every = (0..map.size).flat_map(|y| (0..map.size).map(move |x| (x, y))).collect::<Vec<_>>();
    let away = *every
        .iter()
        .find(|&&spot| map.free(spot.0, spot.1) && !beside_a_road(&map, spot))
        .unwrap();
    let kept = *every
        .iter()
        .find(|&&spot| {
            map.kept.contains(&spot) && map.open(spot.0, spot.1) && beside_a_road(&map, spot)
        })
        .unwrap();
    let road = map.roads()[0];
    let water = map.find('w').unwrap();
    for (what, body) in [
        ("no road beside it", place("house", away)),
        ("a side street", place("house", kept)),
        ("a road", place("house", road)),
        ("water", place("house", water)),
        ("off the map", place("house", (-1, 4))),
        ("a kind that isn't a place", place("homes", away)),
        ("a word that isn't a kind", place("palace", away)),
        ("a road for a kind", place("road", away)),
        (
            "a road move with a place's kind",
            json!({ "action": "road", "x": away.0, "y": away.1, "kind": "house" }),
        ),
    ] {
        let reply = server.act(&ann.key, COMMONS, body).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{what}: {}", reply.show());
    }
    assert_eq!(server.moves_left(&ann.key).await, before, "a refused move costs nothing");

    // Once it's there it is built on, to everything else.
    let spot = building_spots(&map, 1)[0];
    server.act(&ann.key, COMMONS, place("house", spot)).await.ok();
    let again = server.place_at(&ann, "club", spot, now() + 4 * 86_400).unwrap_err();
    assert_eq!(again.status.as_u16(), StatusCode::BAD_REQUEST.as_u16(), "{}", again.message);
    let agent = server.guest().await;
    let road = server.act(&agent.token, COMMONS, road_move(spot, spot)).await;
    assert_eq!(road.status, StatusCode::BAD_REQUEST, "{}", road.show());
    assert_eq!(tile_kind(&server, spot), Some(Kind::House));
}

// W56, W61
#[tokio::test]
async fn a_person_has_one_house_and_three_other_places() {
    let server = Server::start().await;
    let map = Map::from_db(&server, COMMONS);
    let spots = building_spots(&map, 6);
    let ann = server.person("Ann").await;
    let later = now() + 8 * 86_400;

    server.act(&ann.key, COMMONS, place("house", spots[0])).await.ok();
    let second = server.act(&ann.key, COMMONS, place("house", spots[1])).await;
    assert_eq!(second.status, StatusCode::CONFLICT, "{}", second.show());
    // The other kinds come with time, so a week on all three can be put up.
    for (kind, spot) in [("cafe", spots[1]), ("club", spots[2]), ("studio", spots[3])] {
        server.place_at(&ann, kind, spot, later).unwrap();
    }
    let fourth = server.place_at(&ann, "studio", spots[4], later).unwrap_err();
    assert_eq!(fourth.status.as_u16(), StatusCode::CONFLICT.as_u16(), "{}", fourth.message);

    // Limits are each person's own.
    let bo = server.person("Bo").await;
    server.act(&bo.key, COMMONS, place("house", spots[4])).await.ok();
    assert_eq!(tile_kind(&server, spots[4]), Some(Kind::House));

    // Taking one down makes room for another.
    server
        .act(&ann.key, COMMONS, json!({ "action": "bulldoze", "x": spots[0].0, "y": spots[0].1 }))
        .await
        .ok();
    assert_eq!(tile_kind(&server, spots[0]), None);
    server.act(&ann.key, COMMONS, place("house", spots[5])).await.ok();
    assert_eq!(tile_kind(&server, spots[5]), Some(Kind::House));
}

fn temp_db() -> String {
    let unique = auth::new_id();
    std::env::temp_dir()
        .join(format!("agenthangout-test-{unique}.db"))
        .to_string_lossy()
        .into_owned()
}

// W56
#[tokio::test]
async fn the_commons_holds_at_most_1500_places() {
    let path = temp_db();
    let server = Server::boot(Db::open(&path).unwrap(), None, None).await;
    let map = Map::from_db(&server, COMMONS);
    let spot = building_spots(&map, 1)[0];
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let filler = server.state.db.own_player(&bo.id, now()).unwrap().id;
    let conn = rusqlite::Connection::open(&path).unwrap();
    let insert = "INSERT INTO world_buildings (world_id, x, y, kind, player_id, built_at) VALUES ('commons', ?1, ?2, 'cafe', ?3, 0)";
    for n in 0..worlds::MAX_BUILDINGS - 1 {
        conn.execute(insert, rusqlite::params![n % 40, 200 + n / 40, filler]).unwrap();
    }
    server.act(&ann.key, COMMONS, place("house", spot)).await.ok();
    let carol = server.person("Carol").await;
    let spot = building_spots(&Map::from_db(&server, COMMONS), 1)[0];
    let full = server.act(&carol.key, COMMONS, place("house", spot)).await;
    assert_eq!(full.status, StatusCode::CONFLICT, "{}", full.show());
    drop(conn);
    let _ = std::fs::remove_file(&path);
}

// W57
#[tokio::test]
async fn a_place_stays_however_long_its_owner_is_away() {
    let (server, keeper) = Server::start_with_keeper().await;
    let map = Map::from_db(&server, COMMONS);
    let spots = building_spots(&map, 2);
    let ann = server.person("Ann").await;
    server.act(&ann.key, COMMONS, place("house", spots[0])).await.ok();

    // Control: a road whose builder has gone can be cleared by anyone (W17).
    let builder = server.guest().await;
    server.act(&builder.token, COMMONS, road_move(spots[1], spots[1])).await.ok();
    assert_eq!(tile_kind(&server, spots[1]), Some(Kind::Road));

    let stranger = server.guest().await;
    let down = |spot: (i64, i64)| json!({ "action": "bulldoze", "x": spot.0, "y": spot.1 });
    let now_refused = server.act(&stranger.token, COMMONS, down(spots[0])).await;
    assert_eq!(now_refused.status, StatusCode::BAD_REQUEST, "{}", now_refused.show());

    let player = server.player_row(server.player_id(&stranger.token).await);
    let later = now() + world::GONE_SECS * 52;
    worlds::act(
        &server.state,
        &player,
        COMMONS,
        &mv("bulldoze", Some(spots[1].0), Some(spots[1].1)),
        later,
    )
    .unwrap();
    assert_eq!(tile_kind(&server, spots[1]), None, "the road was cleared");
    let house = worlds::act(
        &server.state,
        &player,
        COMMONS,
        &mv("bulldoze", Some(spots[0].0), Some(spots[0].1)),
        later,
    );
    assert!(house.is_err(), "a year away, and a stranger took the house down");
    assert_eq!(tile_kind(&server, spots[0]), Some(Kind::House));

    // W18: the keeper can, and it comes down with what else they built.
    let id = server.state.db.own_player(&ann.id, now()).unwrap().id;
    server
        .post(&format!("/api/worlds/{COMMONS}/players/{id}/remove"), json!({}))
        .token(&keeper.key)
        .send()
        .await
        .ok();
    assert_eq!(tile_kind(&server, spots[0]), None);
}

// W57
#[tokio::test]
async fn a_place_takes_over_its_tile_and_gives_it_back() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let map = Map::from_db(&server, COMMONS);
    let spot = building_spots(&map, 1)[0];
    let pos = (spot.0 as u16, spot.1 as u16);
    let tile = |kind| Tile { kind, level: 0, player: Some(1) };
    let write = |change| {
        server
            .state
            .db
            .in_transaction::<(), rusqlite::Error>(|tx| tx.write_changes(COMMONS, &[change], now()))
            .unwrap()
    };
    // Cleared forest is open land, and a building can stand on it.
    write(world::Change::Set(pos, tile(Kind::Cleared)));
    let ann = server.person("Ann").await;
    server.act(&ann.key, COMMONS, place("house", spot)).await.ok();
    assert_eq!(tile_kind(&server, spot), Some(Kind::House), "one thing on the tile");

    // On forest, taking it down leaves the cleared tile again.
    write(world::Change::Set(pos, tile(Kind::Cleared)));
    assert_eq!(tile_kind(&server, spot), Some(Kind::Cleared), "one thing on the tile");
}

// W58
#[tokio::test]
async fn a_road_a_place_needs_cant_be_cut_and_it_counts_for_your_town() {
    let server = Server::start().await;
    let map = Map::from_db(&server, COMMONS);
    let ann = server.person("Ann").await;
    let agent = server.guest().await;
    let (first, last) = branch_from_road(&map, 8);
    server.act(&agent.token, COMMONS, road_move(first, last)).await.ok();

    // Control: nothing depends on it yet, so its builder can cut it and lay it back.
    let cut = json!({ "action": "bulldoze", "x": first.0, "y": first.1 });
    server.act(&agent.token, COMMONS, cut.clone()).await.ok();
    server.act(&agent.token, COMMONS, road_move(first, first)).await.ok();

    let map = Map::from_db(&server, COMMONS);
    let beside = [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .iter()
        .map(|(dx, dy)| (last.0 + dx, last.1 + dy))
        .find(|spot| map.free(spot.0, spot.1))
        .expect("room at the end of the road");
    let town = server.me(&ann.key, COMMONS).await["your_town"].clone();
    let before = server.fresh_view(COMMONS).await;
    server.act(&ann.key, COMMONS, place("house", beside)).await.ok();
    let after = server.fresh_view(COMMONS).await;
    // It counts for `your_town` and the stage (W23, W48), and for nothing else.
    assert_eq!(after["places"].as_u64().unwrap(), before["places"].as_u64().unwrap() + 1);
    assert_eq!(after["roads"], before["roads"]);
    let housed = server.me(&ann.key, COMMONS).await["your_town"].clone();
    assert_eq!(
        (housed["roads"].clone(), housed["places"].as_u64()),
        (town["roads"].clone(), Some(town["places"].as_u64().unwrap() + 1))
    );

    let reply = server.act(&agent.token, COMMONS, cut).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.show());
    assert_eq!(tile_kind(&server, first), Some(Kind::Road));
}

// W59, W71
#[tokio::test]
async fn a_place_is_a_tile_and_only_owners_names_whose_it_is() {
    let server = Server::start().await;
    let map = Map::from_db(&server, COMMONS);
    let spots = building_spots(&map, 2);
    let spot = spots[1];
    let ann = server.person("Marigold").await;
    // A studio comes a day after the house (W61).
    server.act(&ann.key, COMMONS, place("house", spots[0])).await.ok();
    server.place_at(&ann, "studio", spot, now() + 2 * 86_400).unwrap();

    let view = server.fresh_view(COMMONS).await;
    let tile = view["tiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tile| tile[0] == spot.0 && tile[1] == spot.1)
        .unwrap();
    assert_eq!(*tile, json!([spot.0, spot.1, "studio", 0]));

    let path = format!("/api/worlds/{COMMONS}/look?x={}&y={}&w=8&h=8", spot.0, spot.1);
    let look = server.get(&path).send().await.ok();
    assert!(
        look["built"].as_array().unwrap().contains(&json!([spot.0, spot.1, "studio", 0])),
        "{look}"
    );
    // W71: the view names the owner in `owners`, and only there. Nothing says which account it is, and
    // `look` names nobody.
    assert!(
        view["owners"].as_array().unwrap().contains(&json!([
            spot.0,
            spot.1,
            "Marigold",
            "Marigold's Studio"
        ])),
        "{view}"
    );
    let mut rest = view.clone();
    rest.as_object_mut().unwrap().remove("owners");
    for text in [rest.to_string(), look.to_string()] {
        assert!(!text.contains("Marigold"), "{text}");
    }
    for text in [view.to_string(), look.to_string()] {
        assert!(!text.contains(&ann.id), "{text}");
    }

    // There is no tool for it, and the build tool doesn't take one.
    let guest = server.guest().await;
    let spot = building_spots(&Map::from_db(&server, COMMONS), 1)[0];
    server
        .tool_refused(
            "build",
            json!({ "player_token": guest.token, "type": "house", "x": spot.0, "y": spot.1 }),
        )
        .await;
}

// W28, and what deploy/wipe-commons.sh does
#[tokio::test]
async fn deleting_the_commons_row_starts_it_again_and_leaves_everyone_else_alone() {
    let path = temp_db();
    let server = Server::boot(Db::open(&path).unwrap(), None, None).await;
    worlds::ensure_commons(&server.state).unwrap();
    let fresh = server.tiles(COMMONS);
    let map = Map::from_db(&server, COMMONS);
    let spots = building_spots(&map, 2);

    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    server.act(&ann.key, COMMONS, place("house", spots[0])).await.ok();
    server.act(&guest.token, COMMONS, road_move(spots[1], spots[1])).await.ok();
    // And Ann's frozen world, with a road of hers in it, which a wipe of The Commons leaves be.
    let (own, _) = server.own_world(&ann, true);
    let hers = server.state.db.own_player(&ann.id, now()).unwrap().id;
    let there = Map::from_db(&server, &own).line_from_road(1).unwrap().0;
    server.built_by(&own, hers, &[there]);
    assert_ne!(server.tiles(COMMONS), fresh);
    let own_tiles = server.tiles(&own);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON; DELETE FROM worlds WHERE id = 'commons';")
        .unwrap();
    let left: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM world_tiles WHERE world_id = 'commons')
                  + (SELECT COUNT(*) FROM world_buildings WHERE world_id = 'commons')
                  + (SELECT COUNT(*) FROM world_players WHERE world_id = 'commons')
                  + (SELECT COUNT(*) FROM place_clocks WHERE world_id = 'commons')
                  + (SELECT COUNT(*) FROM world_ends WHERE world_id = 'commons')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "the row took everything of The Commons with it");
    drop(conn);

    // As a restart does it: the same database, and a new server.
    let again = Server::boot(Db::open(&path).unwrap(), None, None).await;
    worlds::ensure_commons(&again.state).unwrap();
    assert_eq!(again.tiles(COMMONS), fresh, "the highway is back, and nothing else");
    assert_eq!(again.tiles(&own), own_tiles, "other worlds are as they were");
    assert_eq!(
        again.get("/api/me").token(&ann.key).send().await.status,
        StatusCode::OK,
        "accounts stay"
    );
    assert!(
        again.moves_left(&guest.token).await <= i64::from(worlds::GUEST_MOVES),
        "tokens still work"
    );
    again.act(&ann.key, COMMONS, place("house", spots[0])).await.ok();
    let _ = std::fs::remove_file(&path);
}

// W73
#[test]
fn a_map_paints_its_land_in_the_same_step_that_sizes_it() {
    // Sizing a canvas wipes it, and a resize is heard after the frame is drawn, so the browser shows
    // whatever is there. The sky is a canvas of its own behind the map, so a map that waits for the
    // next frame shows as clouds, and it did every time one opened. Nothing here runs a browser, so
    // this reads the page, as tests/rooms.rs does for the nap spots.
    let page = include_str!("../web/world.js");
    let (_, rest) = page.split_once("\n  resize() {").expect("the map has a resize");
    let (resize, _) = rest.split_once("\n  }").expect("resize ends");
    let wiped = resize.find("this.canvas.width = cw").expect("resize sizes the canvas");
    let camera = resize.find("this.clampCam()").expect("resize settles the camera");
    let land =
        resize.find("this.render()").expect("resize leaves the map blank until the next frame");
    assert!(
        wiped < land && camera < land,
        "the land goes on after the wipe, with the camera settled"
    );
    assert!(resize.contains("drawSky("), "a map still waiting for its view shows the sky");
}

// W74
#[test]
fn the_status_bar_under_the_map_stays_one_line_tall() {
    // The map gets the height the bar leaves, so a goal that wrapped when the mouse came onto the map
    // resized the map each time, in a window of an ordinary size. Nothing here runs a browser, so this
    // reads the page. Rooms stack rows in the same bar and have no map, so they keep them.
    let app = include_str!("../web/app.js");
    assert!(
        app.contains("frame.root.classList.add('wd-game', 'wd-map');"),
        "the map's window says it's the map"
    );
    let css = include_str!("../web/style.css");
    assert!(
        css.contains(".wd-map > .wd-status { flex-wrap: nowrap; }"),
        "the bar is one row on a wide screen"
    );
    assert!(
        css.contains(".wd-map .wd-status .wd-seg[role=\"status\"], .wd-map .wd-status .wd-town { white-space: nowrap; overflow-wrap: normal; }"),
        "the two cells that say whole sentences stay on one line, on every screen",
    );
    assert!(
        css.contains(".wd-map .wd-status .wd-town { order: 1; }"),
        "on a narrow screen the readout can't add a row"
    );
    assert!(
        app.contains("frame.message.title = text;"),
        "a message that doesn't fit can still be read"
    );
    assert!(app.contains("townEl.title = "), "a goal that doesn't fit can still be read");
}

// W83, W84, W85, W87
#[tokio::test]
async fn roads_follow_the_streets_of_their_neighborhood_and_a_short_lane_goes_anywhere() {
    let server = Server::start().await;
    let guest = server.guest().await;

    // The view lists every neighborhood's point, name and character.
    let view = server.fresh_view(COMMONS).await;
    let hoods = view["neighborhoods"].as_array().unwrap();
    assert_eq!(hoods.len(), 25, "{view}");
    for hood in hoods {
        assert!(hood[0].as_str().unwrap().contains(' '), "{hood}");
        assert!(["homes", "makers", "market"].contains(&hood[1].as_str().unwrap()), "{hood}");
    }

    // Look says which neighborhood, and lists the guide's road out from the highway as streets.
    let look = server
        .tool("look", json!({ "player_token": guest.token, "x": 51, "y": 42, "w": 16, "h": 8 }))
        .await;
    assert!(look["neighborhood"]["name"].is_string(), "{look}");
    let streets: HashSet<(i64, i64)> = look["streets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|at| (at[0].as_i64().unwrap(), at[1].as_i64().unwrap()))
        .collect();
    assert!((47..=56).all(|x| streets.contains(&(x, 42))), "{look}");

    // Along a street works. A long road between streets doesn't, and says why.
    server.act(&guest.token, COMMONS, road_move((47, 42), (56, 42))).await.ok();
    let between = server.act(&guest.token, COMMONS, road_move((47, 43), (56, 43))).await;
    assert_eq!(between.status, StatusCode::BAD_REQUEST, "{}", between.show());
    assert!(between.body.to_string().contains("roads follow the streets"), "{}", between.show());
    // A lane of 3 tiles goes anywhere, as long as it touches a road.
    server.act(&guest.token, COMMONS, road_move((47, 43), (49, 43))).await.ok();
}
