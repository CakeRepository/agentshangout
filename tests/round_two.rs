//! Round two of the security loop for the public worlds (docs/public-city.md, W1 to W70) and what
//! they touch. Round one (`worlds_attack.rs`) found real bugs and they were fixed; this round looks
//! for what is still wrong, and above all for what the fixes brought in: side streets (W15), the
//! road-cut check around places (W17), what a restart leaves alone (W28, W46), places and who may
//! put them up (W54 to W61), the worlds that froze (W25 to W27), tips (W47), the address limits
//! (W12), and a database that was made by the code that is live today.
//!
//! A comment such as `// W17` above a test names the rule it attacks. A test that fails is a bug in
//! the server, not in the test; a test called `measure_*` or `*_report` only prints what something
//! costs (`COST REPORT`) and passes. Time is real for anything that goes over HTTP; a test that
//! needs a time of its own calls the functions in `worlds` directly.

#![allow(dead_code, unused_imports)]

mod support;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

use agenthangout::{
    app, auth,
    db::{Db, PlayerKind, PlayerRow},
    now,
    world::{self, Change, Kind, Span, Tile, Tiles},
    worlds::{self, Move},
    AppState, Config,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use reqwest::Client;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const COMMONS: &str = "commons";

/// What the old game left on a map.
fn old_city(kind: Kind) -> bool {
    matches!(kind, Kind::Homes | Kind::Shops | Kind::Workshops | Kind::Park)
}
const CHROME: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/126.0 Safari/537.36";
const GOOGLEBOT: &str = "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";
const CHATGPT_USER: &str =
    "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; ChatGPT-User/1.0";
const GEMINI: &str = "Mozilla/5.0 (compatible; Google-Agent)";

// ----------------------------------------------------------------------------------------------
// The harness: the same one round one used, so a result here means the same thing there
// ----------------------------------------------------------------------------------------------

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
        self.try_send().await.unwrap()
    }

    async fn try_send(self) -> Result<Reply, reqwest::Error> {
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
        let response = request.timeout(Duration::from_secs(60)).send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        let text = response.text().await?;
        let body = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok(Reply { status, body, text, headers })
    }
}

impl Server {
    async fn start() -> Self {
        Self::boot(Db::open(":memory:").unwrap(), None, None, None).await
    }

    /// A server that believes it is at a public https address, as the live one does.
    async fn start_as_live() -> Self {
        Self::boot(Db::open(":memory:").unwrap(), None, None, Some("https://agentshangout.test"))
            .await
    }

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
        let server = Self::boot(db, None, Some("keeper0000000001".to_string()), None).await;
        let keeper = Who { key, id: "keeper0000000001".to_string(), cookie: String::new() };
        (server, keeper)
    }

    async fn on(db: Db) -> Self {
        Self::boot(db, None, None, None).await
    }

    async fn boot(
        db: Db,
        create_code: Option<String>,
        keeper_id: Option<String>,
        public_url: Option<&str>,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = AppState::new(
            db,
            Config {
                public_url: public_url.map(str::to_string).unwrap_or_else(|| base.clone()),
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

    async fn person(&self, name: &str) -> Who {
        let reply =
            self.post("/api/hangouts", json!({ "name": "Crew", "your_name": name })).send().await;
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

    async fn acts_ok(&self, token: &str, world: &str, body: Value) -> Value {
        self.act(token, world, body).await.ok()
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

    /// A world somebody started before worlds of your own were retired. It exists and it is frozen
    /// (W25, W27): nothing can be done in it, and it never steps.
    fn frozen_world(&self, id: &str, owner: Option<&str>, open: bool, seed: u64, at: i64) {
        let land = world::terrain(seed, world::OWN_SIZE);
        let highway = world::highway(seed, &land);
        let code = format!("code-{id}");
        let new = agenthangout::db::NewWorld {
            id,
            name: "Quiet Bay",
            kind: "own",
            seed,
            size: world::OWN_SIZE,
            owner_id: owner,
            open,
            invite_code: Some(&code),
            highway: &highway,
            now: at,
        };
        self.state.db.insert_world(&new, worlds::MAX_OWN_WORLDS).unwrap();
    }

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

    async fn tool(&self, name: &str, arguments: Value) -> Value {
        let result = self.call(name, arguments).await;
        assert!(result["isError"].is_null(), "{name} failed: {result}");
        result["structuredContent"].clone()
    }

    fn tiles(&self, world: &str) -> Tiles {
        self.state.db.world_tiles(world).unwrap()
    }

    fn player_row(&self, id: i64) -> PlayerRow {
        self.state.db.player(id).unwrap().unwrap()
    }

    /// A bot made in a hangout of its own.
    async fn bot(&self, name: &str) -> Bot {
        let room = self
            .post("/api/hangouts", json!({ "name": "Crew", "your_name": "Owner" }))
            .send()
            .await
            .ok();
        let key = room["owner_key"].as_str().unwrap().to_string();
        let hangout = room["hangout"]["id"].as_str().unwrap().to_string();
        let person = room["person"]["id"].as_str().unwrap().to_string();
        self.bot_in(&key, &person, &hangout, name).await
    }

    async fn bot_in(&self, key: &str, person: &str, hangout: &str, name: &str) -> Bot {
        let bot = self
            .post(&format!("/api/hangouts/{hangout}/agents"), json!({ "name": name }))
            .token(key)
            .send()
            .await
            .ok();
        Bot {
            key: key.to_string(),
            person: person.to_string(),
            hangout: hangout.to_string(),
            token: bot["token"].as_str().unwrap().to_string(),
            agent: bot["agent"]["id"].as_str().unwrap().to_string(),
        }
    }

    /// A bot that has joined the public worlds: its player token.
    async fn bot_player(&self, bot: &Bot) -> String {
        let joined = self.post("/api/public/join", json!({})).token(&bot.token).send().await.ok();
        joined["player_token"].as_str().unwrap().to_string()
    }
}

#[derive(Clone)]
struct Bot {
    key: String,
    person: String,
    hangout: String,
    token: String,
    agent: String,
}

/// A small generator, so a failing run can be run again.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn pick<T: Clone>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize].clone()
    }
}

fn temp_db(label: &str) -> String {
    let path =
        std::env::temp_dir().join(format!("round2-{label}-{}-{}.db", std::process::id(), now()));
    path.to_str().unwrap().to_string()
}

fn remove_db(path: &str) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path}{suffix}"));
    }
}

/// Every table's rows as text, read from a second connection to a database file.
fn dump(path: &str) -> BTreeMap<String, Vec<String>> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut out = BTreeMap::new();
    for name in names {
        let mut stmt = conn.prepare(&format!("SELECT * FROM \"{name}\"")).unwrap();
        let columns = stmt.column_count();
        let mut rows: Vec<String> = stmt
            .query_map([], |row| {
                let mut cells = Vec::new();
                for column in 0..columns {
                    let value: rusqlite::types::Value = row.get(column)?;
                    cells.push(format!("{value:?}"));
                }
                Ok(cells.join(" | "))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows.sort();
        out.insert(name, rows);
    }
    out
}

/// The schema as the database stores it, one entry per table, index and trigger.
fn schema_of(path: &str) -> Vec<String> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut stmt = conn
        .prepare("SELECT type || ' ' || name || ' ' || COALESCE(sql, '') FROM sqlite_master ORDER BY type, name")
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0)).unwrap().map(Result::unwrap).collect()
}

fn mv(action: &str, x: Option<i64>, y: Option<i64>) -> Move {
    Move { action: action.to_string(), x, y, x2: None, y2: None, kind: None, emote: None }
}

fn road_move(first: (i64, i64), last: (i64, i64)) -> Value {
    json!({ "action": "road", "x": first.0, "y": first.1, "x2": last.0, "y2": last.1 })
}

fn headers_with(agent: &str, address: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("user-agent", HeaderValue::from_str(agent).unwrap());
    if let Some(address) = address {
        headers.insert("x-forwarded-for", HeaderValue::from_str(address).unwrap());
    }
    headers
}

fn ip_headers(address: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", HeaderValue::from_str(address).unwrap());
    headers
}

/// Writes tiles straight to the database, as a test's way of making a situation that would take a
/// long time to reach by playing.
fn put_tiles(server: &Server, world: &str, changes: &[Change]) {
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| tx.write_changes(world, changes, now()))
        .unwrap();
}

fn tile(kind: Kind, level: u8, player: Option<u64>) -> Tile {
    Tile { kind, level, player }
}

/// A player of the server's own, who can be the builder of a place: a place's row names its
/// builder, so unlike a tile of the old city it needs one that exists.
fn make_player(db: &Db) -> u64 {
    db.create_guest(&auth::hash_secret(&auth::new_id()), &auth::hash_secret(&auth::new_id()), now())
        .unwrap()
        .id as u64
}

/// Makes `Server::state`'s own player for a person, the one `act` uses when a person builds.
fn person_player(server: &Server, person: &Who) -> PlayerRow {
    server.state.db.own_player(&person.id, now()).unwrap()
}

/// Gives a player a fresh day, whenever the test says `at` is.
fn refill(server: &Server, player: &PlayerRow, at: i64) {
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| tx.charge(player.id, worlds::today(at), 0, at))
        .unwrap();
}

/// Pretends a player was last seen `ago` seconds before `at`.
fn last_seen(server: &Server, player: &PlayerRow, at: i64, ago: i64) {
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| tx.charge(player.id, 0, 0, at - ago))
        .unwrap();
}

/// The land of a world the server holds.
fn land_of(server: &Server, world: &str) -> world::Land {
    worlds::ensure_commons(&server.state).unwrap();
    let row = server.state.db.world(world).unwrap().unwrap();
    world::terrain(row.seed, row.size)
}

/// Places that a road joined to the highway is within reach of, worked out the long way.
fn served_the_long_way(tiles: &Tiles) -> HashSet<(u16, u16)> {
    let mut joined: HashSet<(u16, u16)> = tiles
        .iter()
        .filter(|(_, t)| t.kind == Kind::Road && t.player.is_none())
        .map(|(pos, _)| *pos)
        .collect();
    let mut queue: Vec<(u16, u16)> = joined.iter().copied().collect();
    while let Some((x, y)) = queue.pop() {
        for next in [
            (x.wrapping_add(1), y),
            (x.wrapping_sub(1), y),
            (x, y.wrapping_add(1)),
            (x, y.wrapping_sub(1)),
        ] {
            if tiles.get(&next).is_some_and(|t| t.kind == Kind::Road) && joined.insert(next) {
                queue.push(next);
            }
        }
    }
    tiles
        .iter()
        .filter(|(_, t)| t.kind.is_building())
        .filter(|(pos, _)| {
            joined.iter().any(|j| j.0.abs_diff(pos.0) <= 3 && j.1.abs_diff(pos.1) <= 3)
        })
        .map(|(pos, _)| *pos)
        .collect()
}

fn cheb(a: (i64, i64), b: (i64, i64)) -> i64 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

fn open_ground(land: &world::Land, tiles: &Tiles, x: i64, y: i64) -> bool {
    match land.locate(x, y) {
        Some(pos) => {
            matches!(land.at(pos), Some(world::Ground::Grass | world::Ground::Sand))
                && !tiles.contains_key(&pos)
        }
        None => false,
    }
}

/// The limits count by the minute on the clock, so a test that counts what one minute lets through
/// starts early in a minute, with plenty of it left.
async fn start_early_in_a_minute() {
    let second = now().rem_euclid(60);
    if second > 25 {
        tokio::time::sleep(Duration::from_secs((60 - second) as u64 + 1)).await;
    }
}

// ----------------------------------------------------------------------------------------------
// W30, W13: a refused move costs nothing. Does it cost the server nothing too?
// ----------------------------------------------------------------------------------------------

/// A road that a guest owns beside the highway, and a place that only that road serves: a
/// bulldoze of the road is refused (W17), and working out why takes the whole network.
fn make_a_dependant_road(server: &Server, builder: i64) -> (i64, i64) {
    let land = land_of(server, COMMONS);
    let tiles = server.tiles(COMMONS);
    let highway = world::highway(world::COMMONS_SEED, &land);
    let road_tiles: Vec<(i64, i64)> = tiles
        .iter()
        .filter(|(_, t)| t.kind == Kind::Road)
        .map(|(pos, _)| (i64::from(pos.0), i64::from(pos.1)))
        .collect();
    for &(hx, hy) in
        highway.iter().map(|(x, y)| (i64::from(*x), i64::from(*y))).collect::<Vec<_>>().iter()
    {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let road = (hx + dx, hy + dy);
            let home = (hx + dx * 4, hy + dy * 4);
            if !open_ground(&land, &tiles, road.0, road.1)
                || !open_ground(&land, &tiles, home.0, home.1)
            {
                continue;
            }
            // Nothing else joined may reach the home.
            if road_tiles.iter().any(|&r| cheb(r, home) <= 3) {
                continue;
            }
            put_tiles(
                server,
                COMMONS,
                &[
                    Change::Set(
                        (road.0 as u16, road.1 as u16),
                        tile(Kind::Road, 0, Some(builder as u64)),
                    ),
                    Change::Set(
                        (home.0 as u16, home.1 as u16),
                        tile(Kind::House, 0, Some(make_player(&server.state.db))),
                    ),
                ],
            );
            return road;
        }
    }
    panic!("nowhere on the highway for a road with a place that depends on it");
}

fn crowd_the_commons(server: &Server, how_many: usize) {
    worlds::ensure_commons(&server.state).unwrap();
    let have = server.tiles(COMMONS);
    let mut filler = Vec::new();
    'fill: for y in 0..world::COMMONS_SIZE as u16 {
        for x in 0..world::COMMONS_SIZE as u16 {
            if (u32::from(x) * 7 + u32::from(y) * 3) % 2 == 0 && !have.contains_key(&(x, y)) {
                filler.push(Change::Set((x, y), tile(Kind::Park, 0, Some(999_002))));
                if filler.len() >= how_many {
                    break 'fill;
                }
            }
        }
    }
    put_tiles(server, COMMONS, &filler);
}

// W30, W13
#[tokio::test]
async fn a_refused_bulldoze_of_a_road_loads_the_whole_world_and_nothing_stops_a_guest_asking_again_and_again(
) {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let guest = server.guest().await;
    let guest_id = server.player_id(&guest.token).await;
    let road = make_a_dependant_road(&server, guest_id);
    crowd_the_commons(&server, 8_000);
    let tiles = server.tiles(COMMONS);
    let land = land_of(&server, COMMONS);
    let highway = world::highway(world::COMMONS_SEED, &land);
    let (hx, hy) = highway[highway.len() / 2];

    // What one refused request costs the server, in the server's own process.
    let row = server.player_row(guest_id);
    let time = |what: &str, mv: Move| {
        let started = Instant::now();
        let mut refused = 0;
        for n in 0..200 {
            let result = worlds::act(&server.state, &row, COMMONS, &mv, now() + n);
            refused += usize::from(result.is_err());
        }
        let each = started.elapsed().as_secs_f64() * 1000.0 / 200.0;
        println!("COST REPORT: {what}: {each:.3} ms each, {refused} of 200 refused, in a Commons of {} tiles", tiles.len());
        each
    };
    let off_map = time(
        "a bulldoze off the map (the cheapest refusal)",
        mv("bulldoze", Some(9_999), Some(9_999)),
    );
    let highway_tile = time(
        "a bulldoze of a highway tile, which nobody can take down",
        mv("bulldoze", Some(i64::from(hx)), Some(i64::from(hy))),
    );
    let needed = time(
        "a bulldoze of my own road that a place depends on",
        mv("bulldoze", Some(road.0), Some(road.1)),
    );
    println!(
        "COST REPORT: the dependent road costs {:.0} times a cheap refusal",
        needed / off_map.max(0.0001)
    );
    assert!(highway_tile > 0.0 && needed > 0.0);

    // The same over HTTP, the way a guest would ask. A refused move is free to the guest, and the
    // server has no limit on how often one can be asked, so the guest can keep the database busy.
    let mut limited = 0;
    let mut refused = 0;
    for _ in 0..400 {
        let reply = server
            .act(&guest.token, COMMONS, json!({ "action": "bulldoze", "x": road.0, "y": road.1 }))
            .await;
        match reply.status {
            StatusCode::BAD_REQUEST => refused += 1,
            StatusCode::TOO_MANY_REQUESTS => limited += 1,
            other => panic!("unexpected {other}: {}", reply.show()),
        }
    }
    println!("COST REPORT: 400 refused bulldozes in a row by one guest: {refused} answered in full, {limited} slowed down");
    assert_eq!(server.moves_left(&guest.token).await, 30, "a refusal cost something");
    assert!(
        limited > 0,
        "400 refused bulldozes of a road, each of which read all {} tiles of The Commons and worked out its network, were \
         all answered: a refused move is free, so there is no limit on a guest asking for one",
        tiles.len()
    );
}

// ----------------------------------------------------------------------------------------------
// W15, W17, W55: a move loads only the part of a world it needs. Does it decide as the whole map would?
// ----------------------------------------------------------------------------------------------

#[derive(Default, Debug)]
struct FuzzStats {
    done: HashMap<String, u32>,
    refused: HashMap<String, u32>,
    /// Places held back by who the player is, what they have or how long they have waited (W54 to W61).
    held: u32,
    kept: u32,
    needed: u32,
    road_cuts: u32,
    abandoned_taken: u32,
}

/// What the whole map says about a move, with no part of it left out.
fn the_whole_map_says(
    server: &Server,
    seed: u64,
    land: &world::Land,
    tiles: &Tiles,
    builder: u64,
    m: &Move,
    at: i64,
) -> Result<world::Plan, &'static str> {
    let (x, y) = (m.x.unwrap(), m.y.unwrap());
    let (x2, y2) = (m.x2.unwrap_or(x), m.y2.unwrap_or(y));
    let span = Span { x: x.min(x2), y: y.min(y2), x2: x.max(x2), y2: y.max(y2) };
    let gone = |who: u64| {
        server
            .state
            .db
            .player(who as i64)
            .unwrap()
            .is_none_or(|p| at - p.seen_at >= world::GONE_SECS)
    };
    match m.action.as_str() {
        // W85: and its new tiles follow the streets of their neighborhood, unless it's a lane.
        "road" => world::plan_road(land, tiles, builder, span).and_then(|plan| {
            let fresh: Vec<world::Pos> = plan
                .changes
                .iter()
                .filter_map(|change| match change {
                    Change::Set(pos, _) => Some(*pos),
                    _ => None,
                })
                .collect();
            world::check_streets(&world::neighborhoods(seed, land.size()), tiles, &fresh)
                .map(|()| plan)
        }),
        "place" => world::plan_place(
            land,
            tiles,
            builder,
            Kind::parse(m.kind.as_deref().unwrap()).unwrap(),
            x,
            y,
        ),
        "bulldoze" => world::plan_bulldoze(land, tiles, builder, x, y, &gone),
        other => panic!("{other}"),
    }
}

/// A random move, most of them near a road, some at the edges of the map, a good share of the
/// bulldozes aimed at what someone built, and some places on the tiles kept for side streets.
fn random_move(
    rng: &mut Rng,
    size: i64,
    roads: &[(i64, i64)],
    targets: &[(i64, i64)],
    kept: &[(i64, i64)],
) -> Move {
    let (mut x, mut y) = match rng.below(100) {
        0..=69 if !roads.is_empty() => {
            let (sx, sy) = roads[rng.below(roads.len() as u64) as usize];
            (sx + rng.below(13) as i64 - 6, sy + rng.below(13) as i64 - 6)
        }
        70..=84 => {
            let edge = [0, 1, 2, 3, size - 4, size - 3, size - 2, size - 1][rng.below(8) as usize];
            if rng.below(2) == 0 {
                (edge, rng.below(size as u64) as i64)
            } else {
                (rng.below(size as u64) as i64, edge)
            }
        }
        _ => (rng.below(size as u64) as i64, rng.below(size as u64) as i64),
    };
    x = x.clamp(-2, size + 1);
    y = y.clamp(-2, size + 1);
    match rng.below(100) {
        0..=29 => {
            let kind = rng.pick(&["house", "house", "studio", "club", "cafe"]);
            let (px, py) = if !kept.is_empty() && rng.below(100) < 25 {
                kept[rng.below(kept.len() as u64) as usize]
            } else if !roads.is_empty() && rng.below(100) < 70 {
                // Right beside a road, which is where a place can stand.
                let (rx, ry) = roads[rng.below(roads.len() as u64) as usize];
                let (dx, dy) = [(1, 0), (-1, 0), (0, 1), (0, -1)][rng.below(4) as usize];
                (rx + dx, ry + dy)
            } else {
                (x, y)
            };
            Move {
                action: "place".into(),
                x: Some(px),
                y: Some(py),
                x2: None,
                y2: None,
                kind: Some(kind.into()),
                emote: None,
            }
        }
        30..=54 if !roads.is_empty() && rng.below(100) < 60 => {
            // A road that starts beside an existing one and runs away from it.
            let (rx, ry) = roads[rng.below(roads.len() as u64) as usize];
            let (dx, dy) = [(1, 0), (-1, 0), (0, 1), (0, -1)][rng.below(4) as usize];
            let len = 1 + rng.below(17) as i64;
            let (sx, sy) = (rx + dx, ry + dy);
            Move {
                action: "road".into(),
                x: Some(sx),
                y: Some(sy),
                x2: Some(sx + dx * (len - 1)),
                y2: Some(sy + dy * (len - 1)),
                kind: None,
                emote: None,
            }
        }
        30..=64 => {
            let long = rng.below(19) as i64 - 9;
            let (x2, y2) = match rng.below(40) {
                0 => (x + long, y + rng.below(5) as i64 - 2),
                n if n % 2 == 0 => (x + long, y),
                _ => (x, y + long),
            };
            Move {
                action: "road".into(),
                x: Some(x),
                y: Some(y),
                x2: Some(x2),
                y2: Some(y2),
                kind: None,
                emote: None,
            }
        }
        _ => {
            if !targets.is_empty() && rng.below(100) < 70 {
                let (bx, by) = targets[rng.below(targets.len() as u64) as usize];
                mv("bulldoze", Some(bx), Some(by))
            } else {
                mv("bulldoze", Some(x), Some(y))
            }
        }
    }
}

/// A road that starts beside an existing one and runs along a street (W84) for 4 to 16 tiles, as an
/// agent reading `streets` in look would lay it.
fn street_road(
    rng: &mut Rng,
    hoods: &[world::Hood],
    tiles: &Tiles,
    roads: &[(i64, i64)],
    size: i64,
) -> Option<Move> {
    let (rx, ry) = *roads.get(rng.below(roads.len().max(1) as u64) as usize)?;
    let (dx, dy) = [(1, 0), (-1, 0), (0, 1), (0, -1)][rng.below(4) as usize];
    let want = 4 + rng.below(13) as i64;
    let mut len = 0;
    while len < want {
        let (x, y) = (rx + dx * (len + 1), ry + dy * (len + 1));
        if !(0..size).contains(&x) || !(0..size).contains(&y) {
            break;
        }
        let pos = (x as u16, y as u16);
        if tiles.contains_key(&pos) || !world::on_street(hoods, tiles, pos) {
            break;
        }
        len += 1;
    }
    (len >= 4).then(|| {
        let (sx, sy) = (rx + dx, ry + dy);
        Move {
            action: "road".into(),
            x: Some(sx),
            y: Some(sy),
            x2: Some(sx + dx * (len - 1)),
            y2: Some(sy + dy * (len - 1)),
            kind: None,
            emote: None,
        }
    })
}

/// Plays `rounds` random moves by `players` in a world, with time passing, and checks every one
/// against the whole map.
fn fuzz_a_world(
    server: &Server,
    world: &str,
    players: &[PlayerRow],
    rounds: usize,
    seed: u64,
) -> FuzzStats {
    let row = server.state.db.world(world).unwrap().unwrap();
    let land = world::terrain(row.seed, row.size);
    let size = row.size as i64;
    let mut rng = Rng(seed);
    let mut stats = FuzzStats::default();
    let hoods = world::neighborhoods(row.seed, row.size);
    let mut at = now();
    let mut before = server.tiles(world);
    for round in 0..rounds {
        at += 2;
        // A person's places come with days (W61), so now and then a week goes by.
        if round % 400 == 399 {
            at += 8 * 86_400;
        }
        let roads: Vec<(i64, i64)> = before
            .iter()
            .filter(|(_, t)| t.kind == Kind::Road)
            .map(|(pos, _)| (i64::from(pos.0), i64::from(pos.1)))
            .collect();
        let targets: Vec<(i64, i64)> = before
            .iter()
            .filter(|(_, t)| t.player.is_some() && (t.kind == Kind::Road || t.kind.is_building()))
            .map(|(pos, _)| (i64::from(pos.0), i64::from(pos.1)))
            .collect();
        // Open tiles kept for a side street, near a few of the roads, so a place is often tried on one.
        let mut kept: Vec<(i64, i64)> = Vec::new();
        for _ in 0..20.min(roads.len()) {
            let (rx, ry) = roads[rng.below(roads.len() as u64) as usize];
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                for i in 1..=3 {
                    let (px, py) = (rx + dx * i, ry + dy * i);
                    if (0..size).contains(&px)
                        && (0..size).contains(&py)
                        && !before.contains_key(&(px as u16, py as u16))
                        && world::is_reserved(&before, (px as u16, py as u16))
                    {
                        kept.push((px, py));
                    }
                }
            }
        }
        let who = &players[rng.below(players.len() as u64) as usize];
        let person = who.kind() == PlayerKind::Person;
        let m = random_move(&mut rng, size, &roads, &targets, &kept);
        // W87: an agent builds along the streets look shows it, so half the roads tried do too.
        let m = if m.action == "road" && rng.below(2) == 0 {
            street_road(&mut rng, &hoods, &before, &roads, size).unwrap_or(m)
        } else {
            m
        };
        refill(server, who, at);
        let world_seed = server.state.db.world(world).unwrap().unwrap().seed;
        let expected =
            the_whole_map_says(server, world_seed, &land, &before, who.id as u64, &m, at);
        let got = worlds::act(&server.state, who, world, &m, at);
        let after = server.tiles(world);
        let text = format!("round {round}: {m:?} by player {} in {world}", who.id);
        let is_place = m.action == "place";
        match (expected, got) {
            // W54 to W61: who the player is, what they have and how long they have waited are not the
            // map's to decide, and a guest is always told no. Whatever is said, nothing changes.
            (_, Err(error))
                if is_place
                    && matches!(error.status, StatusCode::FORBIDDEN | StatusCode::CONFLICT) =>
            {
                assert!(
                    person || error.status == StatusCode::FORBIDDEN,
                    "{text}: a guest was told {}",
                    error.status
                );
                assert_eq!(after, before, "{text}: refused and changed tiles");
                stats.held += 1;
            }
            // W16: a road isn't a person's to lay, and being told so changes nothing.
            (_, Err(error)) if m.action == "road" && error.status == StatusCode::FORBIDDEN => {
                assert!(person, "{text}: a guest was told no road");
                assert_eq!(after, before, "{text}: refused and changed tiles");
                stats.held += 1;
            }
            (Ok(plan), Ok(acted)) => {
                assert!(person || !is_place, "{text}: somebody who is not a person put up a place");
                assert!(!person || m.action != "road", "{text}: a person laid a road");
                // W13: a walk or an emote builds nothing and says it changed 1.
                let changed = if matches!(m.action.as_str(), "walk" | "emote") {
                    1
                } else {
                    plan.changes.len()
                };
                assert_eq!(
                    (acted.cost, acted.changed as usize, acted.skipped),
                    (plan.cost, changed, plan.skipped),
                    "{text}"
                );
                let mut should = before.clone();
                world::apply(&mut should, &plan.changes);
                assert_eq!(
                    after, should,
                    "{text}: the tiles are not what the whole map's plan makes"
                );
                *stats.done.entry(m.action.clone()).or_default() += 1;
                if m.action == "bulldoze" {
                    let (x, y) = (m.x.unwrap() as u16, m.y.unwrap() as u16);
                    let was = before.get(&(x, y)).copied();
                    if was.is_some_and(|tile| tile.kind == Kind::Road) {
                        stats.road_cuts += 1;
                        // W17: no place that had a road lost it.
                        let (served, still) =
                            (served_the_long_way(&before), served_the_long_way(&after));
                        assert!(
                            served.is_subset(&still),
                            "{text}: a road came down and left {:?} without one",
                            served.difference(&still).collect::<Vec<_>>()
                        );
                    }
                    if was.is_some_and(|tile| {
                        tile.player.is_some() && tile.player != Some(who.id as u64)
                    }) {
                        stats.abandoned_taken += 1;
                        // W57: somebody else's place is never anyone's to take down.
                        assert!(
                            !was.is_some_and(|tile| tile.kind.is_building()),
                            "{text}: a place was taken by someone else"
                        );
                    }
                }
            }
            (Err(why), Err(error)) => {
                assert_eq!(error.status, StatusCode::BAD_REQUEST, "{text}: {}", error.message);
                assert_eq!(error.message, why, "{text}");
                assert_eq!(after, before, "{text}: refused and changed tiles");
                *stats.refused.entry(m.action.clone()).or_default() += 1;
                stats.kept += u32::from(why.contains("side street"));
                stats.needed += u32::from(why.contains("depends"));
            }
            (Ok(plan), Err(error)) => {
                panic!(
                    "{text}: the whole map allows {plan:?} and the server said {} {}",
                    error.status, error.message
                )
            }
            (Err(why), Ok(acted)) => {
                panic!("{text}: the whole map says {why:?} and the server did {acted:?}")
            }
        }
        // W46: nothing with no builder is ever taken down or changed into something else by a move.
        for (pos, was) in before.iter().filter(|(_, t)| t.player.is_none()) {
            let there = after
                .get(pos)
                .unwrap_or_else(|| panic!("{text}: the street at {pos:?} with no builder is gone"));
            assert_eq!((there.kind, there.player), (was.kind, None), "{text}: {pos:?}");
        }
        before = after;
        // Time passes. A step changes no tile (W20, W46).
        if round % 6 == 5 {
            worlds::step_world(&server.state, world, at).unwrap();
            assert_eq!(server.tiles(world), before, "round {round}: a step changed a tile");
        }
        if round % 45 == 44 {
            // One of the others has not been seen for eight days: what they left that isn't a place is anyone's.
            let idle = &players[rng.below(players.len() as u64) as usize];
            last_seen(server, idle, at, 8 * 86_400);
        }
    }
    println!("COST REPORT: {world}: {stats:?}");
    stats
}

async fn some_players(server: &Server, people: usize, guests: usize) -> Vec<PlayerRow> {
    let mut out = Vec::new();
    for n in 0..people {
        let person = server.person(&format!("Fuzz {n}")).await;
        out.push(person_player(server, &person));
    }
    for _ in 0..guests {
        let guest = server.guest().await;
        let id = server.player_id(&guest.token).await;
        out.push(server.player_row(id));
    }
    out
}

// W15, W17, W46, W55
#[tokio::test]
async fn in_the_commons_every_move_is_decided_as_the_whole_map_would_decide_it() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let players = some_players(&server, 4, 2).await;
    let stats = fuzz_a_world(&server, COMMONS, &players, 3_000, 0x5EED_0001);
    for action in ["place", "road", "bulldoze"] {
        let least = if action == "place" { 5 } else { 30 };
        assert!(
            stats.done.get(action).copied().unwrap_or(0) >= least,
            "too few {action} moves worked to say anything: {stats:?}"
        );
    }
    assert!(stats.held >= 10, "who may place was hardly tried: {stats:?}");
    assert!(stats.kept >= 3, "side streets were hardly tried: {stats:?}");
    assert!(
        stats.road_cuts >= 5 && stats.needed >= 1,
        "the road-cut check was hardly tried: {stats:?}"
    );
}

// W25, W27
#[tokio::test]
async fn nothing_can_be_done_in_a_frozen_world_whatever_is_asked_and_nothing_in_it_changes() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let owner = server.person("Owner").await;
    let id = "frozen0000000001";
    server.frozen_world(id, Some(&owner.id), true, 4242, now());
    // What it held when it froze, the old kinds among it.
    let land = land_of_world(&server, id);
    let (kinds, builder) = (
        [
            Kind::Road,
            Kind::Homes,
            Kind::Shops,
            Kind::Workshops,
            Kind::Park,
            Kind::Cafe,
            Kind::House,
        ],
        make_player(&server.state.db),
    );
    let old: Vec<Change> = (0..40u16)
        .map(|n| (20 + n % 8, 10 + n / 8))
        .filter(|&pos| land.at(pos).is_some_and(|ground| ground.is_open()))
        .enumerate()
        .map(|(n, pos)| {
            // The house is the one a person can have.
            let kind = if n == 3 { Kind::House } else { kinds[n % (kinds.len() - 1)] };
            Change::Set(pos, tile(kind, u8::from(n % 2 == 0), Some(builder)))
        })
        .collect();
    put_tiles(&server, id, &old);
    let mut players = vec![person_player(&server, &owner)];
    players.extend(some_players(&server, 2, 2).await);
    let (tiles, active) = (server.tiles(id), server.state.db.world(id).unwrap().unwrap().active_at);
    let size = world::OWN_SIZE as i64;
    let roads: Vec<(i64, i64)> = tiles
        .iter()
        .filter(|(_, t)| t.kind == Kind::Road)
        .map(|(pos, _)| (i64::from(pos.0), i64::from(pos.1)))
        .collect();
    let targets: Vec<(i64, i64)> =
        tiles.keys().map(|pos| (i64::from(pos.0), i64::from(pos.1))).collect();
    let mut rng = Rng(0x5EED_0003);
    let mut at = now();
    for round in 0..400 {
        // Each try is a minute later than the last, so the limit on refused moves never gets in the way.
        at += 61;
        let who = &players[rng.below(players.len() as u64) as usize];
        let m = match round % 5 {
            0 => {
                mv("walk", Some(rng.below(size as u64) as i64), Some(rng.below(size as u64) as i64))
            }
            1 => Move { emote: Some("wave".into()), ..mv("emote", None, None) },
            _ => random_move(&mut rng, size, &roads, &targets, &[]),
        };
        refill(&server, who, at);
        let error = worlds::act(&server.state, who, id, &m, at)
            .expect_err(&format!("round {round}: {m:?} worked in a frozen world"));
        assert_eq!(error.status, StatusCode::FORBIDDEN, "round {round}: {m:?}: {}", error.message);
        assert!(
            server.state.db.world_player(id, who.id).unwrap().is_none(),
            "round {round}: {m:?} put a player in a frozen world"
        );
    }
    assert_eq!(server.tiles(id), tiles, "a refused move changed a frozen world");
    assert_eq!(
        server.state.db.world(id).unwrap().unwrap().active_at,
        active,
        "a refused move woke a frozen world"
    );
}

// ----------------------------------------------------------------------------------------------
// W28, W46, W24: the highway is all the server builds, restarts, and what steps
// ----------------------------------------------------------------------------------------------

/// A Commons as a new server makes it: its row and the highway, and nothing else.
fn a_bare_commons(db: &Db) -> (world::Land, Vec<(u16, u16)>) {
    let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
    let highway = world::highway(world::COMMONS_SEED, &land);
    db.insert_world(
        &agenthangout::db::NewWorld {
            id: COMMONS,
            name: worlds::COMMONS_NAME,
            kind: "public",
            seed: world::COMMONS_SEED,
            size: world::COMMONS_SIZE,
            owner_id: None,
            open: true,
            invite_code: None,
            highway: &highway,
            now: now(),
        },
        worlds::MAX_OWN_WORLDS,
    )
    .unwrap();
    (land, highway)
}

fn land_of_world(server: &Server, world: &str) -> world::Land {
    let row = server.state.db.world(world).unwrap().unwrap();
    world::terrain(row.seed, row.size)
}

// W28, W46
#[tokio::test]
async fn a_new_commons_is_the_highway_and_nothing_else_and_starting_the_server_touches_nothing_players_built(
) {
    // A new server makes the land and the highway, and no town.
    let fresh = Server::start().await;
    worlds::ensure_commons(&fresh.state).unwrap();
    let land = land_of(&fresh, COMMONS);
    let highway: HashSet<(u16, u16)> =
        world::highway(world::COMMONS_SEED, &land).into_iter().collect();
    let tiles = fresh.tiles(COMMONS);
    assert_eq!(
        tiles.keys().copied().collect::<HashSet<_>>(),
        highway,
        "a new Commons has more on it than the highway"
    );
    assert!(
        tiles.values().all(|t| *t == tile(Kind::Road, 0, None)),
        "something on the highway is not a road with no builder"
    );
    assert!(
        fresh.state.db.world_end(COMMONS).unwrap().is_none(),
        "a new Commons has a clock running"
    );

    // A Commons that already exists, with players' tiles of every kind on it, old and new, on the
    // ground where a founding town used to be added.
    let path = temp_db("town");
    let (_, highway) = a_bare_commons(&Db::open(&path).unwrap());
    let highway: HashSet<(u16, u16)> = highway.into_iter().collect();
    let kinds = [
        Kind::Homes,
        Kind::Shops,
        Kind::Workshops,
        Kind::Park,
        Kind::Road,
        Kind::House,
        Kind::Cafe,
        Kind::Club,
        Kind::Studio,
    ];
    // Each builder has one of each kind, as a person's house is the one they can have.
    let builders: Vec<u64> = (0..10).map(|_| make_player(&Db::open(&path).unwrap())).collect();
    let theirs: Vec<Change> = (0..90u16)
        .map(|n| (48 + n % 12, 60 + n / 12))
        .filter(|pos| !highway.contains(pos))
        .enumerate()
        .map(|(n, pos)| {
            let kind = kinds[n % kinds.len()];
            Change::Set(
                pos,
                tile(
                    kind,
                    if old_city(kind) { (n % 4) as u8 } else { 0 },
                    Some(builders[n / kinds.len()]),
                ),
            )
        })
        .collect();
    {
        let db = Db::open(&path).unwrap();
        db.in_transaction::<(), rusqlite::Error>(|tx| tx.write_changes(COMMONS, &theirs, now()))
            .unwrap();
    }
    // The server starts, and starts again, as a server does every day: nothing changes.
    let first = Server::on(Db::open(&path).unwrap()).await;
    let before = first.tiles(COMMONS);
    assert!(
        before.len() >= highway.len() + theirs.len() - 5,
        "the tiles the test made are not all there"
    );
    worlds::ensure_commons(&first.state).unwrap();
    assert_eq!(first.tiles(COMMONS), before, "starting the server changed what was on The Commons");
    let second = Server::on(Db::open(&path).unwrap()).await;
    worlds::ensure_commons(&second.state).unwrap();
    assert_eq!(second.tiles(COMMONS), before, "a second start changed the tiles");
    drop((first, second));
    remove_db(&path);
}

// W19, W28, W46
#[tokio::test]
async fn a_long_run_of_steps_changes_no_tile_and_starts_no_clock_on_a_commons_with_nothing_of_the_old_city(
) {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let players = some_players(&server, 2, 2).await;
    let land = land_of(&server, COMMONS);
    let size = land.size() as i64;
    let mut rng = Rng(0xC0FFEE);
    let mut at = now();
    let mut before = server.tiles(COMMONS);
    let mut player_moves = 0;
    for step in 0..3_000 {
        at += 60;
        // Roads follow the streets now (W85), so more of a random player's moves are refused: one
        // every 5 steps keeps enough of them landing for the run to prove something.
        if step % 5 == 0 {
            // A player makes a move near the middle, where the highway ends.
            let roads: Vec<(i64, i64)> = before
                .iter()
                .filter(|(pos, t)| {
                    t.kind == Kind::Road
                        && cheb((i64::from(pos.0), i64::from(pos.1)), (size / 2, size / 2)) < 30
                })
                .map(|(pos, _)| (i64::from(pos.0), i64::from(pos.1)))
                .collect();
            let who = &players[rng.below(players.len() as u64) as usize];
            refill(&server, who, at);
            let m = random_move(&mut rng, size, &roads, &[], &[]);
            player_moves += usize::from(worlds::act(&server.state, who, COMMONS, &m, at).is_ok());
            before = server.tiles(COMMONS);
        }
        worlds::step_world(&server.state, COMMONS, at).unwrap();
        // W46: a step builds nothing and takes nothing. Only a move does.
        assert_eq!(server.tiles(COMMONS), before, "step {step} changed a tile");
    }
    println!("COST REPORT: 3,000 steps with {player_moves} moves from the players");
    assert!(player_moves >= 20, "the run proved little: {player_moves} moves");
    // W64: nothing of the old city stands, so no clock ever started and no disaster is on the way.
    assert!(
        server.state.db.world_end(COMMONS).unwrap().is_none(),
        "a clock started on a Commons with nothing of the old city"
    );
    assert_eq!(server.state.db.world(COMMONS).unwrap().unwrap().step, 3_000);
}

// W19, W27, W46
#[tokio::test]
async fn a_step_changes_nothing_in_a_world_somebody_started_and_never_starts_the_end_of_the_old_city_there(
) {
    let server = Server::start().await;
    let owner = server.person("Owner").await;
    let id = "frozen0000000001";
    server.frozen_world(id, Some(&owner.id), true, 4242, now());
    // What it held when it froze: the old kinds and a place.
    let land = land_of_world(&server, id);
    let (kinds, builder) = (
        [Kind::Homes, Kind::Shops, Kind::Workshops, Kind::Park, Kind::Road, Kind::Cafe],
        make_player(&server.state.db),
    );
    let old: Vec<Change> = (0..60u16)
        .map(|n| (16 + n % 10, 8 + n / 10))
        .filter(|&pos| land.at(pos).is_some_and(|ground| ground.is_open()))
        .enumerate()
        .map(|(n, pos)| {
            Change::Set(
                pos,
                tile(
                    kinds[n % kinds.len()],
                    if old_city(kinds[n % kinds.len()]) { (n % 4) as u8 } else { 0 },
                    Some(builder),
                ),
            )
        })
        .collect();
    assert!(old.iter().any(|c| matches!(c, Change::Set(_, t) if old_city(t.kind))));
    put_tiles(&server, id, &old);
    let before = server.tiles(id);
    let at0 = now();
    // Far past the days after which the old city would end on The Commons.
    for step in 1..=2_500 {
        worlds::step_world(&server.state, id, at0 + step * 60).unwrap();
    }
    assert_eq!(server.tiles(id), before, "stepping changed a world in somebody's name");
    assert!(
        server.state.db.world_end(id).unwrap().is_none(),
        "a clock started in a world that is not The Commons"
    );
}

// W19, W46, W29
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn players_moving_while_the_world_steps_never_lose_a_tile_to_each_other_or_to_a_step() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let players = some_players(&server, 2, 2).await;
    let size = world::COMMONS_SIZE as i64;
    let roads: Vec<(i64, i64)> = server
        .tiles(COMMONS)
        .iter()
        .filter(|(pos, t)| {
            t.kind == Kind::Road
                && cheb((i64::from(pos.0), i64::from(pos.1)), (size / 2, size / 2)) < 30
        })
        .map(|(pos, _)| (i64::from(pos.0), i64::from(pos.1)))
        .collect();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let base = now();
    let watcher = {
        let (state, stop) = (server.state.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut owners: HashMap<(u16, u16), Option<u64>> = HashMap::new();
            let mut bad = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                for (pos, t) in state.db.world_tiles(COMMONS).unwrap() {
                    match owners.get(&pos) {
                        Some(owner) if *owner != t.player => bad.push((pos, *owner, t.player)),
                        None => {
                            owners.insert(pos, t.player);
                        }
                        _ => {}
                    }
                }
                std::thread::sleep(Duration::from_millis(3));
            }
            bad
        })
    };
    let stepper = {
        let state = server.state.clone();
        std::thread::spawn(move || {
            for n in 0..1_500i64 {
                worlds::step_world(&state, COMMONS, base + n * 60).unwrap();
            }
        })
    };
    let movers: Vec<_> = players
        .iter()
        .cloned()
        .enumerate()
        .map(|(n, who)| {
            let (state, roads) = (server.state.clone(), roads.clone());
            std::thread::spawn(move || {
                let mut rng = Rng(0xFACE + n as u64);
                let mut done = 0;
                for i in 0..500i64 {
                    let m = loop {
                        let m = random_move(&mut rng, size, &roads, &[], &[]);
                        // Nothing is bulldozed, so a tile can only ever change hands by being overwritten.
                        if m.action != "bulldoze" {
                            break m;
                        }
                    };
                    let at = base + i * 4 + n as i64;
                    let _ = state.db.in_transaction::<(), rusqlite::Error>(|tx| {
                        tx.charge(who.id, worlds::today(at), 0, at)
                    });
                    done += usize::from(worlds::act(&state, &who, COMMONS, &m, at).is_ok());
                }
                done
            })
        })
        .collect();
    let done: usize = movers.into_iter().map(|mover| mover.join().unwrap()).sum();
    stepper.join().unwrap();
    stop.store(true, Ordering::Relaxed);
    let bad = watcher.join().unwrap();
    println!("COST REPORT: {done} moves went in while the world stepped 1,500 times");
    assert!(done >= 40, "too few moves went in to say anything ({done})");
    assert!(bad.is_empty(), "tiles changed hands: {bad:?}");
    assert!(
        server.state.db.world_end(COMMONS).unwrap().is_none(),
        "a clock started on a Commons with nothing of the old city"
    );
}

// W24
#[tokio::test]
async fn only_the_commons_steps_whatever_happens_to_the_worlds_that_froze() {
    let server = Server::start().await;
    let long_ago = now() - 40 * 86_400;
    // Worlds nobody has touched for forty days, one closed and one open.
    server.frozen_world("sleepy0000000001", None, false, 4242, long_ago);
    server.frozen_world("awake00000000001", None, true, 4243, long_ago);
    let due = |at: i64| worlds::worlds_due(&server.state, at).unwrap();
    assert_eq!(due(now()), [COMMONS.to_string()], "a world asleep for forty days is stepping");
    let stranger = server.guest().await;
    let id = server.player_id(&stranger.token).await;
    let row = server.player_row(id);
    // Nothing a stranger does in either of them is allowed, and none of it wakes anything.
    let wave_move = || Move { emote: Some("wave".into()), ..mv("emote", None, None) };
    for world in ["sleepy0000000001", "awake00000000001"] {
        for m in [wave_move(), mv("walk", Some(5), Some(5)), mv("bulldoze", Some(5), Some(5))] {
            assert!(worlds::act(&server.state, &row, world, &m, now()).is_err());
        }
    }
    assert_eq!(due(now()), [COMMONS.to_string()], "a refused move woke a world");
    // The Commons steps whenever anyone last moved, because its street walks by its step.
    let commons_active = server.state.db.world(COMMONS).unwrap().unwrap().active_at;
    assert_eq!(
        due(commons_active + 31 * 86_400),
        [COMMONS.to_string()],
        "The Commons went to sleep"
    );
    // Even an emote, which woke an open world once, does nothing in one now.
    let owner = server.person("Owner").await;
    let player = person_player(&server, &owner);
    let later = now() + 45 * 86_400;
    refill(&server, &player, later);
    let refused =
        worlds::act(&server.state, &player, "awake00000000001", &wave_move(), later).unwrap_err();
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.message);
    assert_eq!(due(later), [COMMONS.to_string()], "a move woke a frozen world");
    // And a round of steps takes only The Commons.
    let report = worlds::step_all(&server.state, later);
    assert_eq!(report.stepped, [COMMONS.to_string()]);
    assert!(report.failed.is_empty());
}

// W24, W25, W32
#[tokio::test]
async fn measure_a_round_of_steps_when_two_hundred_frozen_worlds_are_full_and_only_the_commons_steps(
) {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let at = now();
    let started = Instant::now();
    for n in 0..200u64 {
        let seed = 10_000 + n * 37;
        let land = world::terrain(seed, world::OWN_SIZE);
        let id = format!("full{n:012}");
        server.frozen_world(&id, None, true, seed, at);
        let mut fill = Vec::new();
        for y in 0..world::OWN_SIZE as u16 {
            for x in 0..world::OWN_SIZE as u16 {
                if (x + y) % 3 != 0
                    && matches!(land.at((x, y)), Some(world::Ground::Grass | world::Ground::Sand))
                {
                    let kind =
                        [Kind::Homes, Kind::Shops, Kind::Workshops, Kind::Park][usize::from(x % 4)];
                    fill.push(Change::Set((x, y), tile(kind, (y % 3) as u8, Some(5))));
                }
            }
        }
        put_tiles(&server, &id, &fill);
    }
    println!(
        "COST REPORT: 200 full frozen worlds made in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    let started = Instant::now();
    let report = worlds::step_all(&server.state, at + 60);
    let took = started.elapsed().as_secs_f64();
    println!(
        "COST REPORT: one round of steps over {} awake worlds took {took:.2} s",
        report.stepped.len()
    );
    assert!(report.failed.is_empty());
    assert_eq!(report.stepped, [COMMONS.to_string()], "a frozen world took a step");
    assert!(
        took < 60.0,
        "a round of steps took {took:.1} s, which is longer than the minute it has"
    );
}

// ----------------------------------------------------------------------------------------------
// W47, W8: tips
// ----------------------------------------------------------------------------------------------

fn random_town(rng: &mut Rng, side: u16, players: u64) -> Tiles {
    let mut tiles = Tiles::new();
    for x in 0..side {
        tiles.insert((x, side / 2), tile(Kind::Road, 0, None));
    }
    for _ in 0..rng.below(140) + 10 {
        let pos = (rng.below(u64::from(side)) as u16, rng.below(u64::from(side)) as u16);
        let kind = rng.pick(&[
            Kind::Road,
            Kind::Homes,
            Kind::Shops,
            Kind::Workshops,
            Kind::Park,
            Kind::Cleared,
            Kind::House,
            Kind::Studio,
            Kind::Club,
            Kind::Cafe,
            Kind::Road,
            Kind::House,
        ]);
        let level = if old_city(kind) { rng.below(4) as u8 } else { 0 };
        let owner = if rng.below(10) == 0 { None } else { Some(1 + rng.below(players)) };
        tiles.insert(pos, tile(kind, level, owner));
    }
    tiles
}

/// What one player built themselves, which is all a tip is worked out from.
fn own_tiles(tiles: &Tiles, player: u64) -> Tiles {
    tiles.iter().filter(|(_, t)| t.player == Some(player)).map(|(pos, t)| (*pos, *t)).collect()
}

// W47, W8
#[test]
fn a_tip_is_always_one_of_the_fixed_sentences_for_any_town_and_any_player() {
    let mut rng = Rng(0x71B5);
    let mut seen = HashSet::new();
    for _ in 0..1_500 {
        let tiles = random_town(&mut rng, 24, 4);
        for player in 0..=5u64 {
            let own = own_tiles(&tiles, player);
            for person in [false, true] {
                for moves in [0u32, 1, 30] {
                    let tip = world::tip(&own, person, moves);
                    assert!(world::TIPS.contains(&tip), "{tip:?} is not a fixed tip");
                    // The one for a disaster is said by the server, never worked out from tiles.
                    assert_ne!(tip, world::TIP_STRIKING);
                    if moves == 0 {
                        assert_eq!(tip, world::TIPS[0], "out of moves says so first");
                    }
                    seen.insert(tip);
                }
            }
        }
    }
    assert_eq!(seen.len(), 5, "only {} of the tips were ever picked: {seen:?}", seen.len());
    // An empty world and a player who has built nothing in it.
    for person in [false, true] {
        assert!(world::TIPS.contains(&world::tip(&Tiles::new(), person, 30)));
    }
}

// W47, W48, W49, W4
#[tokio::test]
async fn a_tip_a_goal_and_your_town_depend_on_what_is_yours_and_never_on_what_the_others_build() {
    let mut rng = Rng(0x71B6);
    for round in 0..1_200 {
        let tiles = random_town(&mut rng, 24, 4);
        let me = 1 + rng.below(4);
        // The same buildings of mine, with another builder's name on them: nothing changes, because
        // what a player's own tiles say is the whole of it.
        let mine = own_tiles(&tiles, me);
        let renamed: Tiles =
            mine.iter().map(|(pos, t)| (*pos, Tile { player: Some(1_000 + me), ..*t })).collect();
        for person in [false, true] {
            assert_eq!(
                world::tip(&mine, person, 30),
                world::tip(&renamed, person, 30),
                "round {round}: a tip changed with a name"
            );
            assert_eq!(
                world::goal(&world::town(&mine), person),
                world::goal(&world::town(&renamed), person),
                "round {round}: a goal changed with a name"
            );
        }
        // A stranger who has built nothing hears a listed sentence and has an empty town.
        let stranger = own_tiles(&tiles, 9_999);
        assert!(world::TIPS.contains(&world::tip(&stranger, true, 30)));
        assert_eq!(world::town(&stranger), world::Town::default());
    }
    // And over HTTP: what other people put on the map, of every kind, changes nothing in my answer.
    let server = Server::start().await;
    let (ann, bob) = (server.guest().await, server.guest().await);
    let (ann_id, bob_id) = (server.player_id(&ann.token).await, server.player_id(&bob.token).await);
    lay(&server, ann_id, 110, &[(Kind::Road, 0), (Kind::Homes, 2), (Kind::House, 0)]);
    let before = server.me(&ann.token, COMMONS).await;
    lay(
        &server,
        bob_id,
        112,
        &[
            (Kind::Road, 0),
            (Kind::House, 0),
            (Kind::Studio, 0),
            (Kind::Club, 0),
            (Kind::Cafe, 0),
            (Kind::Homes, 3),
            (Kind::Park, 0),
        ],
    );
    let after = server.me(&ann.token, COMMONS).await;
    for key in ["your_town", "goal", "tip"] {
        assert_eq!(before[key], after[key], "{key} changed when somebody else built");
    }
}

// W47, W30
#[tokio::test]
async fn how_am_i_doing_is_limited_per_address_in_mcp_and_everything_it_says_is_listed() {
    start_early_in_a_minute().await;
    let server = Server::start().await;
    let guest = server.guest().await;
    let call = |ip: &'static str| {
        let token = guest.token.clone();
        let server = &server;
        async move {
            server
                .post(
                    "/public/mcp",
                    json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                            "params": { "name": "how_am_i_doing", "arguments": { "player_token": token } } }),
                )
                .ip(ip)
                .send()
                .await
        }
    };
    let (mut refused, mut told) = (0, 0);
    for _ in 0..300 {
        let reply = call("198.51.100.20").await;
        let result = &reply.body["result"];
        if result["isError"] == true {
            refused += 1;
            assert!(
                result["content"][0]["text"].as_str().unwrap().contains("too often"),
                "{}",
                reply.text
            );
        } else {
            told += 1;
            let tip = result["structuredContent"]["tip"].as_str().unwrap();
            assert!(world::TIPS.contains(&tip), "{tip}");
            let goal = result["structuredContent"]["goal"].as_str().unwrap();
            assert!(world::GOALS.contains(&goal), "{goal}");
            // Everything else in the answer is a number, a listed word or a name from the lists.
            let allowed_keys = [
                "player",
                "moves_left",
                "moves_per_day",
                "kind",
                "built",
                "world",
                "tip",
                "your_town",
                "goal",
            ];
            for key in result["structuredContent"].as_object().unwrap().keys() {
                assert!(allowed_keys.contains(&key.as_str()), "an unexpected field {key}");
            }
        }
    }
    println!("COST REPORT: 300 how_am_i_doing calls from one address in a minute: {told} answered, {refused} slowed down");
    assert!(
        (235..=245).contains(&told),
        "{told} calls were answered: the limit is a view's 240 a minute"
    );
    assert!(
        call("198.51.100.21").await.body["result"]["isError"].is_null(),
        "another address was held up too"
    );
}

// W47, W30
#[tokio::test]
async fn joining_again_and_again_with_a_bot_token_reads_the_whole_world_for_its_tip_each_time_and_is_never_slowed_down(
) {
    // Joins are counted by the minute on the clock, and 300 must all fall in one of them to pass 240.
    start_early_in_a_minute().await;
    let server = Server::start().await;
    crowd_the_commons(&server, 8_000);
    let bot = server.bot("Looper").await;
    let started = Instant::now();
    let (mut limited, mut joined) = (0, 0);
    // More than the 240 views a minute that an address is allowed, which a join now counts as.
    for _ in 0..300 {
        let reply = server
            .post("/api/public/join", json!({}))
            .token(&bot.token)
            .ip("198.51.100.30")
            .send()
            .await;
        match reply.status {
            StatusCode::OK => {
                joined += 1;
                let tip = reply.body["tip"].as_str().unwrap().to_string();
                assert!(world::TIPS.contains(&tip.as_str()), "{tip}");
            }
            StatusCode::TOO_MANY_REQUESTS => limited += 1,
            other => panic!("{other}: {}", reply.show()),
        }
    }
    println!(
        "COST REPORT: 300 joins by one bot in {:.1} s: {joined} worked, {limited} were slowed down; the tip comes from the last view of The Commons and the bot's own tiles",
        started.elapsed().as_secs_f64()
    );
    assert!(
        limited > 0,
        "{joined} joins in a row with one bot token were all answered: a join is counted like a view, which is limited to \
         240 a minute"
    );
}

// ----------------------------------------------------------------------------------------------
// W12, W30: whose address is whose
// ----------------------------------------------------------------------------------------------

// W12
#[tokio::test]
async fn measure_how_few_addresses_it_takes_to_leave_every_new_guest_outside_for_an_hour() {
    let server = Server::start().await;
    let at = now();
    // 120 networks with five guests each are every place the server has for new guests this hour.
    // A few more are tried, because networks that share the limiter's counters lose guests to it.
    let mut joined = 0;
    for network in 0..150 {
        for _ in 0..5 {
            let headers = ip_headers(&format!("2001:db8:{network:x}::1"));
            joined += usize::from(worlds::join(&server.state, &headers, None, at).is_ok());
        }
    }
    let honest = worlds::join(&server.state, &ip_headers("203.0.113.99"), None, at);
    println!(
        "COST REPORT: 150 IPv6 networks (a single /48 has 65,536) made {joined} guests in one hour; the next honest guest got: {}",
        honest.as_ref().map(|_| "in".to_string()).unwrap_or_else(|e| format!("{} {}", e.status, e.message))
    );
    assert_eq!(joined, 600);
    assert!(honest.is_err());
    // A bot with its own token, and a signed-in person, are not kept out.
    let bot = server.bot("Welcome").await;
    let reply = server
        .post("/api/public/join", json!({}))
        .token(&bot.token)
        .ip("203.0.113.99")
        .send()
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.show());
}

// W12, W30
#[test]
fn measure_how_many_networks_it_takes_to_get_honest_addresses_refused_views() {
    let state = AppState::new(
        Db::open(":memory:").unwrap(),
        Config {
            public_url: "https://agentshangout.test".into(),
            create_code: None,
            allow_private_webhooks: false,
            keeper_id: None,
            god_ids: Vec::new(),
        },
    );
    let at = now();
    let mut sent = 0u64;
    let mut line = String::new();
    for target in [100_000u64, 300_000, 600_000, 1_200_000] {
        while sent < target {
            let a = sent;
            // Every request from a different IPv6 network, so a different address as far as the limit sees.
            let headers = ip_headers(&format!(
                "2a0{:x}:{:x}:{:x}::1",
                a >> 32,
                (a >> 16) & 0xffff,
                a & 0xffff
            ));
            let _ = worlds::allow_view(&state, &headers, at);
            sent += 1;
        }
        let mut refused = 0;
        for victim in 0..200u32 {
            let headers = ip_headers(
                &format!("203.0.113.{}", victim % 250 + 1)
                    .replace("203.0.113", &format!("198.{}.113", victim / 250 + 18)),
            );
            refused += usize::from(worlds::allow_view(&state, &headers, at).is_err());
        }
        line += &format!(" after {sent} requests: {refused} of 200 honest addresses refused;");
    }
    println!("COST REPORT: views are 240 a minute per address and the counters are shared by 8,192 addresses.{line}");
}

// W12
#[tokio::test]
async fn measure_what_a_missing_address_header_is_worth_on_a_server_that_thinks_it_is_live() {
    start_early_in_a_minute().await;
    let server = Server::start_as_live().await;
    let mut views = 0;
    for _ in 0..300 {
        views +=
            usize::from(server.get("/api/worlds/commons").send().await.status == StatusCode::OK);
    }
    let mut joins = 0;
    for _ in 0..20 {
        joins += usize::from(
            server.post("/api/public/join", json!({})).send().await.status == StatusCode::OK,
        );
    }
    println!("COST REPORT: with no X-Forwarded-For on a server whose public address is https: {views} of 300 views and {joins} of 20 joins were answered, from one caller");
    // Caddy always adds the header, so a request without one is the shared unknown address: it has
    // a view's 240 a minute and a guest's 5 an hour, between all the callers like it.
    assert_eq!(views, 240);
    assert_eq!(joins, 5);
    // And over plain http, which is local development, no header still means a caller on this machine.
    let local = Server::start().await;
    for _ in 0..300 {
        assert_eq!(local.get("/api/worlds/commons").send().await.status, StatusCode::OK);
    }
    for _ in 0..20 {
        assert_eq!(local.post("/api/public/join", json!({})).send().await.status, StatusCode::OK);
    }
}

// W30
#[tokio::test]
async fn measure_how_many_browsers_behind_one_address_the_view_limit_leaves_room_for() {
    start_early_in_a_minute().await;
    let server = Server::start().await;
    // The page asks every 4 seconds (5 on the front page): 15 a minute, plus one after every move.
    let mut served = 0;
    let mut first_refused = None;
    for n in 1..=300 {
        let reply = server.get("/api/worlds/commons").ip("203.0.113.250").send().await;
        if reply.status == StatusCode::OK {
            served += 1;
        } else if first_refused.is_none() {
            first_refused = Some(n);
        }
    }
    println!(
        "COST REPORT: one address gets {served} views a minute, which is {} browsers on the world page polling every 4 s; a school or an office behind one address shares them",
        served / 15
    );
    assert_eq!(served, 240);
}

// ----------------------------------------------------------------------------------------------
// W38, W41, W1: what the middleware watches
// ----------------------------------------------------------------------------------------------

// W1, W38, W41
#[tokio::test]
async fn no_visit_to_a_private_page_or_route_is_ever_noticed_whatever_the_visitor_calls_itself() {
    let server = Server::start().await;
    let private = [
        "/h/abcdefghijklmnop",
        "/join/abcdefghijklmnopqrstuvwx",
        "/c/abcdefghijklmnop",
        "/house/abcdefghijklmnop",
        "/join-city/abcdefghijklmnop",
        "/join-house/abcdefghijklmnop",
        "/api/hangouts/abcdefghijklmnop",
        "/api/me",
        "/api/cities/abcdefghijklmnop",
        "/api/city-invites/abcdefghijklmnop",
        "/api/house-invites/abcdefghijklmnop",
        "/api/invites/abcdefghijklmnop",
        "/cal/abcdefghijklmnop.ics",
        "/world/commons/extra",
        "/world/",
        "/world",
        "/%77orld/commons",
        "/worlds/extra",
        "/api/worlds/commons",
        "/api/worlds",
        "/api/worlds/commons/me",
        "/style.css",
        "/app.js",
        "/world.js",
        "/favicon.svg",
        "/indexnow.txt",
        "/mcp",
        "/public",
        "/public/mcp/",
        "/api/public/join",
        "/?token=secret&join=abcdefghijklmnop",
        "/h/abcdefghijklmnop?key=ah_owner_secret",
    ];
    for agent in [GEMINI, CHATGPT_USER, GOOGLEBOT, CHROME, ""] {
        for path in private {
            for method in [Method::GET, Method::POST] {
                // A `/` with a query is a page; the other two with queries are not pages at all.
                if path.starts_with("/?") && method == Method::GET {
                    continue;
                }
                let mut request = server.req(method, path).ip("198.51.100.77");
                if !agent.is_empty() {
                    request = request.ua(agent);
                }
                let _ = request.try_send().await;
            }
        }
    }
    let at = now();
    assert!(
        server.state.worlds.visitors(at).is_empty(),
        "a visit to a private path put a visitor on the street"
    );
    assert!(
        server.state.worlds.frogs(at).is_empty(),
        "a visit to a private path put a frog on the street"
    );
    // The same callers do show up where they should, so the check above could have failed.
    let mut asked = server.get("/world/commons").ip("198.51.100.78").ua(GEMINI);
    asked = asked.header("accept", "text/html");
    asked.send().await;
    assert_eq!(server.state.worlds.visitors(now()).len(), 1);
    server.get("/").ip("198.51.100.79").ua(GOOGLEBOT).send().await;
    assert_eq!(server.state.worlds.frogs(now()).len(), 1);
    server
        .post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .ip("198.51.100.80")
        .send()
        .await;
    assert_eq!(server.state.worlds.visitors(now()).len(), 2);
}

// ----------------------------------------------------------------------------------------------
// Headers, enumeration, errors and methods
// ----------------------------------------------------------------------------------------------

const WANT_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; \
                        base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

#[track_caller]
fn secure(reply: &Reply, what: &str) {
    let get = |name: &str| reply.headers.get(name).map(|v| v.to_str().unwrap().to_string());
    assert_eq!(
        get("content-security-policy").as_deref(),
        Some(WANT_CSP),
        "{what}: {}",
        reply.status
    );
    assert_eq!(get("x-content-type-options").as_deref(), Some("nosniff"), "{what}");
    assert_eq!(get("referrer-policy").as_deref(), Some("no-referrer"), "{what}");
    assert_eq!(get("x-frame-options").as_deref(), Some("DENY"), "{what}");
    assert!(get("permissions-policy").is_some_and(|policy| policy.contains("camera=()")), "{what}");
    assert!(get("server").is_none() && get("x-powered-by").is_none(), "{what}: names its software");
    assert!(get("access-control-allow-origin").is_none(), "{what}: lets another site read it");
}

const LEAKS: [&str; 18] = [
    "src/",
    "src\\",
    ".rs:",
    "rusqlite",
    "sqlite",
    "SELECT ",
    "INSERT ",
    "UPDATE ",
    "panicked",
    "thread '",
    "RUST_BACKTRACE",
    "C:\\",
    "/home/",
    "/var/lib",
    "Result::unwrap",
    "called `",
    "stack backtrace",
    "axum",
];

#[track_caller]
fn leaks_nothing(text: &str, what: &str) {
    for leak in LEAKS {
        assert!(!text.contains(leak), "{what}: the answer shows {leak:?}: {text}");
    }
}

// W2, W6, W29
#[tokio::test]
async fn every_answer_to_every_method_on_every_page_asset_and_world_route_carries_the_security_headers_and_none_is_a_server_error(
) {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let guest = server.guest().await;
    let paths = [
        "/",
        "/worlds",
        "/world/commons",
        "/world/nope",
        "/h/abc",
        "/join/abc",
        "/c/abc",
        "/house/abc",
        "/join-city/abc",
        "/join-house/abc",
        "/p/abc",
        "/style.css",
        "/app.js",
        "/room.js",
        "/city.js",
        "/world.js",
        "/passkey.js",
        "/plan-invite.js",
        "/favicon.svg",
        "/og.jpg",
        "/llms-full.txt",
        "/.well-known/mcp-registry-auth",
        "/agents.md",
        "/robots.txt",
        "/sitemap.xml",
        "/llms.txt",
        "/docs",
        "/docs/",
        "/docs/getting-started",
        "/privacy",
        "/indexnow.txt",
        "/api/worlds",
        "/api/worlds/commons",
        "/api/worlds/commons/me",
        "/api/worlds/commons/look",
        "/api/worlds/commons/act",
        "/api/worlds/preview",
        "/api/worlds/mine",
        "/api/worlds/join/abc",
        "/api/worlds/commons/players/1/remove",
        "/api/public/join",
        "/api/players/claim",
        "/api/players/mine",
        "/api/players/1",
        "/api/players/1/reroll",
        "/api/plan-invites/abc",
        "/api/plan-invites/abc/answer",
        "/api/plan-invites/abc/plan.ics",
        "/public/mcp",
        "/mcp",
        "/api/passkeys/available",
        "/nothing/here",
        "/api/nothing",
        "/world/commons/",
        "//",
        "/%00",
        "/%ff%fe",
        "/world/%E0%A4%A",
    ];
    let methods = [
        Method::GET,
        Method::HEAD,
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::PATCH,
        Method::OPTIONS,
    ];
    let mut count = 0;
    for path in paths {
        for method in &methods {
            let mut request = server.req(method.clone(), path).ip(&server.fresh_ip());
            if *method == Method::POST || *method == Method::PUT || *method == Method::PATCH {
                request = request.json(json!({}));
            }
            if path.contains("/me") || path.contains("/act") || path.contains("/join/") {
                request = request.token(&guest.token);
            }
            let reply = request.try_send().await.unwrap_or_else(|e| panic!("{method} {path}: {e}"));
            let what = format!("{method} {path}");
            secure(&reply, &what);
            assert!(!reply.status.is_server_error(), "{what}: {}", reply.show());
            leaks_nothing(&reply.text, &what);
            if *method == Method::HEAD {
                assert!(reply.text.is_empty(), "{what}: a HEAD answer has a body");
            }
            count += 1;
        }
    }
    // Answers that are made before any route runs.
    let big = server
        .post("/api/worlds/commons/act", json!({ "action": "emote", "emote": "x".repeat(200_000) }))
        .token(&guest.token)
        .send()
        .await;
    assert_eq!(big.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", big.show());
    secure(&big, "a body over the limit");
    let wrong = server
        .req(Method::POST, "/api/worlds/commons/act")
        .token(&guest.token)
        .raw("text/plain", b"{\"action\":\"walk\"}")
        .send()
        .await;
    assert!(wrong.status.is_client_error(), "{}", wrong.show());
    secure(&wrong, "the wrong content type");
    let garbage = server
        .req(Method::POST, "/public/mcp")
        .raw("application/json", &[0xff, 0xfe, 0x00, 0x7b])
        .send()
        .await;
    secure(&garbage, "bytes that are not text");
    leaks_nothing(&garbage.text, "bytes that are not text");
    println!("COST REPORT: {count} requests checked");
}

// W2, W29
#[tokio::test]
async fn nothing_that_should_not_be_there_is_served_and_the_refusals_say_nothing() {
    let server = Server::start().await;
    let paths = [
        "/.git/HEAD",
        "/.git/config",
        "/.git/index",
        "/.gitignore",
        "/.env",
        "/.env.local",
        "/.env.production",
        "/Cargo.toml",
        "/Cargo.lock",
        "/src/main.rs",
        "/src/lib.rs",
        "/src/db.rs",
        "/agenthangout.db",
        "/agenthangout.db-wal",
        "/demo.db",
        "/demo.db-wal",
        "/db.sqlite",
        "/backup.sql",
        "/backup.zip",
        "/index.html.bak",
        "/app.js.bak",
        "/app.js.map",
        "/world.js.map",
        "/style.css.map",
        "/room.js.map",
        "/web/app.js",
        "/web/home.html",
        "/web/index.html",
        "/home.html",
        "/index.html",
        "/examples/demo_bots.rs",
        "/hooks/doorbell.ps1",
        "/deploy/provision.sh",
        "/deploy/azure.local.env",
        "/docs/city.md",
        "/docs/public-city.md",
        "/README.md",
        "/target/debug/agenthangout.exe",
        "/debug",
        "/debug/pprof",
        "/metrics",
        "/health",
        "/healthz",
        "/status",
        "/admin",
        "/api/admin",
        "/api/debug",
        "/api/health",
        "/api/version",
        "/swagger.json",
        "/openapi.json",
        "/.well-known/security.txt",
        "/server-status",
        "/actuator/health",
        "/console",
        "/phpinfo.php",
        "/wp-login.php",
        "/.DS_Store",
        "/api/",
        "/api",
        "/public",
        "/public/",
        "/robots.txt.bak",
        "/sitemap.xml.gz",
        "/api/worlds/../me",
        "/api/worlds/%2e%2e/me",
        "/world/commons/../../.env",
        "/..%2f..%2fCargo.toml",
        "/%2e%2e/%2e%2e/Cargo.toml",
        "/static/../Cargo.toml",
        "/app.js/../Cargo.toml",
        "/api/keeper",
        "/api/export",
        "/api/dump",
    ];
    let mut served = Vec::new();
    for path in paths {
        let reply = server.get(path).ip(&server.fresh_ip()).send().await;
        secure(&reply, path);
        leaks_nothing(&reply.text, path);
        if reply.status.is_success() {
            served.push(format!("{path}: {}", reply.status));
        }
        assert!(
            reply.text.len() < 400,
            "{path}: a refusal with {} bytes of explanation: {}",
            reply.text.len(),
            reply.text
        );
    }
    assert!(served.is_empty(), "paths that were served: {served:?}");
    // What is served on purpose is the same for everyone, and is not a dump of anything.
    let robots = server.get("/robots.txt").send().await;
    assert!(robots.text.contains("Disallow: /api/"));
}

// W6, W8
#[tokio::test]
async fn a_broken_request_to_any_route_gets_no_server_error_and_no_path_no_sql_and_no_stack() {
    let server = Server::start().await;
    let owner = server.person("Owner").await;
    let bot = server.bot("Scout").await;
    let routes = vec![
        (Method::POST, "/api/session"),
        (Method::POST, "/api/hangouts"),
        (Method::POST, "/api/cities"),
        (Method::PUT, "/api/notify"),
        (Method::POST, "/api/status"),
        (Method::POST, "/api/activity"),
        (Method::POST, "/api/thinking"),
        (Method::POST, "/api/trips"),
        (Method::POST, "/api/worlds"),
        (Method::POST, "/api/worlds/commons/act"),
        (Method::POST, "/api/public/join"),
        (Method::POST, "/api/players/claim"),
        (Method::POST, "/api/passkeys/register/start"),
        (Method::POST, "/api/passkeys/register/finish"),
        (Method::POST, "/api/passkeys/login/start"),
        (Method::POST, "/api/passkeys/login/finish"),
        (Method::POST, "/public/mcp"),
        (Method::POST, "/mcp"),
        (Method::POST, "/api/key/rotate"),
        (Method::POST, "/api/houses/abcdefghijklmnop/scenes"),
        (Method::PUT, "/api/cities/abcdefghijklmnop/rules"),
        (Method::PUT, "/api/houses/abcdefghijklmnop/rules"),
        (Method::POST, "/api/hangouts/abcdefghijklmnop/events"),
        (Method::POST, "/api/hangouts/abcdefghijklmnop/messages"),
        (Method::POST, "/api/events/abcdefghijklmnop/rsvp"),
        (Method::POST, "/api/check-ins/abcdefghijklmnop/answer"),
    ];
    let bodies: Vec<(&str, Vec<u8>)> = vec![
        ("application/json", b"{".to_vec()),
        ("application/json", b"[1,2,3]".to_vec()),
        (
            "application/json",
            b"{\"name\": 5, \"seed\": \"x\", \"action\": [], \"x\": 1e999}".to_vec(),
        ),
        ("application/json", b"{\"seed\": 18446744073709551616, \"open\": \"maybe\"}".to_vec()),
        ("application/json", vec![0xff, 0xfe, 0xfd]),
        ("application/json", format!("{{\"a\": \"{}\"}}", "z".repeat(70_000)).into_bytes()),
        ("text/plain", b"hello".to_vec()),
        ("application/x-www-form-urlencoded", b"a=b&c=d".to_vec()),
    ];
    let creds: Vec<Option<String>> =
        vec![None, Some(made_up_token()), Some(owner.key.clone()), Some(bot.token.clone())];
    let mut count = 0;
    for (method, path) in &routes {
        for (content_type, body) in &bodies {
            for cred in &creds {
                let mut request =
                    server.req(method.clone(), path).raw(content_type, body).ip(&server.fresh_ip());
                if let Some(cred) = cred {
                    request = request.token(cred);
                }
                let what = format!(
                    "{method} {path} as {} with {content_type} {} bytes",
                    cred.as_deref().map_or("nobody", |c| &c[..8]),
                    body.len()
                );
                let reply = request.try_send().await.unwrap_or_else(|e| panic!("{what}: {e}"));
                assert!(!reply.status.is_server_error(), "{what}: {}", reply.show());
                leaks_nothing(&reply.text, &what);
                secure(&reply, &what);
                count += 1;
            }
        }
    }
    println!("COST REPORT: {count} broken requests answered");
}

fn made_up_token() -> String {
    format!("ah_guest_{}", "q".repeat(40))
}

/// The start of a POST as it goes over the wire, with no body yet.
fn raw_post(path: &str, length: usize) -> Vec<u8> {
    format!(
        "POST {path} HTTP/1.1\r\nhost: test\r\ncontent-type: application/json\r\n\
         content-length: {length}\r\n\r\n"
    )
    .into_bytes()
}

/// Reads one whole answer off a socket and says its status.
async fn read_answer(stream: &mut TcpStream) -> std::io::Result<u16> {
    let mut read = Vec::new();
    let mut piece = [0; 4096];
    loop {
        if let Some(end) = read.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&read[..end]).to_lowercase();
            let length = head.lines().find_map(|line| line.strip_prefix("content-length: "));
            if read.len() >= end + 4 + length.map_or(0, |value| value.parse().unwrap()) {
                return Ok(head.split(' ').nth(1).unwrap().parse().unwrap());
            }
        }
        match stream.read(&mut piece).await? {
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            count => read.extend_from_slice(&piece[..count]),
        }
    }
}

// W88
#[tokio::test]
async fn a_refused_request_gets_its_answer_while_the_rest_of_its_body_is_still_coming() {
    let server = Server::start().await;
    let address = server.base.trim_start_matches("http://");
    let cases = [
        ("/api/status", 5_000, 401),
        ("/api/status", 70_000, 401),
        ("/api/worlds/commons/act", 70_000, 401),
        ("/api/passkeys/login/start", 70_000, 409),
        ("/no/such/route", 70_000, 404),
        ("/api/public/join", 200_000, 413),
    ];
    for (path, length, wanted) in cases {
        let what = format!("POST {path} with {length} bytes");
        let body = vec![b'z'; length];
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream.write_all(&[raw_post(path, length), body[..1000].to_vec()].concat()).await.unwrap();
        // A body comes in pieces over a real network, so this answer can be ready before the rest
        // has arrived.
        tokio::time::sleep(Duration::from_millis(100)).await;
        stream
            .write_all(&body[1000..])
            .await
            .unwrap_or_else(|e| panic!("{what}: cut off while sending: {e}"));
        let status = read_answer(&mut stream)
            .await
            .unwrap_or_else(|e| panic!("{what}: cut off before the answer: {e}"));
        assert_eq!(status, wanted, "{what}");
        // All of it was read, so the connection is as good as it was.
        stream.write_all(b"GET /robots.txt HTTP/1.1\r\nhost: test\r\n\r\n").await.unwrap();
        assert_eq!(read_answer(&mut stream).await.unwrap(), 200, "{what}: the next request");
    }
}

// W88
#[tokio::test]
async fn a_sender_that_asks_first_is_refused_without_being_told_to_send_the_body() {
    let server = Server::start().await;
    let address = server.base.trim_start_matches("http://");
    let mut stream = TcpStream::connect(address).await.unwrap();
    let head = "POST /api/status HTTP/1.1\r\nhost: test\r\nexpect: 100-continue\r\n\
                content-type: application/json\r\ncontent-length: 70000\r\n\r\n";
    stream.write_all(head.as_bytes()).await.unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(20), read_answer(&mut stream));
    assert_eq!(answer.await.expect("left hanging").unwrap(), 401);
}

// W88
#[tokio::test]
async fn a_sender_that_stops_or_never_stops_is_answered_and_let_go() {
    let server = Server::start().await;
    let address = server.base.trim_start_matches("http://");

    // It says a million bytes are coming, sends a thousand and goes quiet.
    let mut stopped = TcpStream::connect(address).await.unwrap();
    stopped.write_all(&raw_post("/api/status", 1_000_000)).await.unwrap();
    stopped.write_all(&[b'z'; 1000]).await.unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(20), read_answer(&mut stopped));
    assert_eq!(answer.await.expect("left hanging").unwrap(), 401);
    let mut after = [0; 64];
    let last = tokio::time::timeout(Duration::from_secs(20), stopped.read(&mut after));
    let last = last.await.expect("left hanging");
    assert!(matches!(last, Ok(0) | Err(_)), "the connection stays open: {last:?}");

    // It would send far more than anyone reads. What goes before the server stops is the 1 MB it
    // takes and what the two systems will hold for it.
    let length = 256 * 1024 * 1024;
    let mut flood = TcpStream::connect(address).await.unwrap();
    flood.write_all(&raw_post("/api/status", length)).await.unwrap();
    let piece = vec![b'z'; 64 * 1024];
    let sending = async {
        let mut sent = 0;
        while sent < length && flood.write_all(&piece).await.is_ok() {
            sent += piece.len();
        }
        sent
    };
    let sent = tokio::time::timeout(Duration::from_secs(20), sending).await.expect("left hanging");
    assert!(sent < 32 * 1024 * 1024, "the server read {sent} bytes of a refused body");
}

// W2
#[tokio::test]
async fn the_wrong_method_on_a_world_route_never_does_the_work_and_never_breaks() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let guest = server.guest().await;
    let ann = server.person("Ann").await;
    let before_tiles = server.tiles(COMMONS);
    let before_moves = server.moves_left(&guest.token).await;
    let cases: Vec<(Method, String, Vec<u16>)> = vec![
        (Method::GET, "/api/worlds/commons/act".into(), vec![405]),
        (Method::PUT, "/api/worlds/commons/act".into(), vec![405]),
        (Method::DELETE, "/api/worlds/commons/act".into(), vec![405]),
        (Method::PATCH, "/api/worlds/commons/act".into(), vec![405]),
        (Method::POST, "/api/worlds/commons".into(), vec![405]),
        (Method::PUT, "/api/worlds/commons".into(), vec![405]),
        (Method::DELETE, "/api/worlds/commons".into(), vec![405]),
        (Method::PUT, "/api/worlds".into(), vec![405]),
        (Method::DELETE, "/api/worlds".into(), vec![405]),
        (Method::GET, "/api/worlds/commons/players/1/remove".into(), vec![405]),
        (Method::DELETE, "/api/worlds/commons/players/1/remove".into(), vec![405]),
        (Method::GET, "/api/public/join".into(), vec![405]),
        (Method::GET, "/api/players/claim".into(), vec![405]),
        (Method::PUT, "/api/players/1".into(), vec![405]),
        (Method::GET, "/api/players/1".into(), vec![405]),
        (Method::GET, "/public/mcp".into(), vec![405]),
        (Method::DELETE, "/public/mcp".into(), vec![405]),
        (Method::GET, "/api/worlds/join/abc".into(), vec![405]),
        (Method::POST, "/api/worlds/commons/look".into(), vec![405]),
        (Method::PUT, "/api/worlds/commons/look".into(), vec![405]),
        (Method::DELETE, "/api/worlds/commons/look".into(), vec![405]),
        (Method::POST, "/api/worlds/commons/me".into(), vec![405]),
    ];
    for (method, path, wanted) in cases {
        for cred in [None, Some(guest.token.clone()), Some(ann.key.clone())] {
            let mut request = server
                .req(method.clone(), &path)
                .ip(&server.fresh_ip())
                .json(json!({ "action": "walk", "x": 70, "y": 70 }));
            if let Some(cred) = &cred {
                request = request.token(cred);
            }
            let reply = request.send().await;
            assert!(
                wanted.contains(&reply.status.as_u16()),
                "{method} {path}: {} (wanted one of {wanted:?})",
                reply.show()
            );
        }
    }
    assert_eq!(server.tiles(COMMONS), before_tiles);
    assert_eq!(
        server.moves_left(&guest.token).await,
        before_moves,
        "a move was charged for the wrong method"
    );
    // HEAD is the one other method that reaches a handler, and a view is counted like any other.
    let head = server.req(Method::HEAD, "/api/worlds/commons").ip("198.51.100.90").send().await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.text.is_empty());
}

// ----------------------------------------------------------------------------------------------
// The pages (web/*.js): any place a string from the API could be read as markup or a link
// ----------------------------------------------------------------------------------------------

fn read_web(name: &str) -> String {
    std::fs::read_to_string(format!("{}/web/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

// W6, W8
#[test]
fn no_page_script_can_turn_a_string_from_the_api_into_markup_script_or_a_link_that_runs() {
    for name in
        ["app.js", "world.js", "city.js", "room.js", "passkey.js", "plan-invite.js", "tutorial.js"]
    {
        let source = read_web(name);
        for banned in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
            "new Function",
            "srcdoc",
            "javascript:",
            "createContextualFragment",
            "DOMParser",
            "setAttribute('on",
            "setAttribute(\"on",
            "setAttribute('style'",
            "location.href =",
            "location.assign",
            "location.replace",
            "window.open(",
            "postMessage(",
            "importScripts",
            ".innerText =",
        ] {
            assert!(!source.contains(banned), "web/{name} uses {banned}");
        }
    }
    // Names go through text nodes and the canvas: the helper that builds elements sets text, never markup.
    let app = read_web("app.js");
    assert!(app.contains("// Builds DOM with text nodes only."));
    // Every path built from a value goes through encodeURIComponent, unless the value is an id the server
    // made (a random string) or a path that was itself built that way. A name is never one of those.
    let mut checked = 0;
    for (n, line) in app.lines().enumerate() {
        for call in ["api(`", "go(`", "link(`"] {
            let mut rest = line;
            while let Some(at) = rest.find(call) {
                let inner = &rest[at + call.len()..];
                let template = &inner[..inner.find('`').unwrap_or(inner.len())];
                let mut left = template;
                while let Some(open) = left.find("${") {
                    let after = &left[open + 2..];
                    let end = after.find('}').unwrap_or(after.len());
                    let expression = &after[..end];
                    let made_by_the_server = expression.ends_with(".id")
                        || expression.ends_with("_id")
                        || matches!(expression, "id" | "path" | "url" | "who" | "standingPath");
                    assert!(
                        expression.starts_with("encodeURIComponent(") || made_by_the_server,
                        "web/app.js line {}: `${{{expression}}}` goes into a path as it is: {line}",
                        n + 1
                    );
                    assert!(
                        !expression.contains("name"),
                        "web/app.js line {}: a name goes into a path: {line}",
                        n + 1
                    );
                    checked += 1;
                    left = &after[end..];
                }
                rest = &inner[template.len()..];
            }
        }
    }
    assert!(checked > 60, "only {checked} path values were looked at");
}

// W6, W8
#[test]
fn a_name_in_a_private_city_can_only_reach_the_screen_as_text() {
    // The plates over houses are drawn on a canvas with fillText, cut to 28 characters, and a name
    // only ever reaches the rest of the page through textContent or a text node.
    let world = read_web("world.js");
    assert!(world.contains("fillText(item.text"));
    assert!(world.contains("slice(0, 28)"));
    let city = read_web("city.js");
    assert!(city.contains("setLabels"));
    for source in [&world, &city, &read_web("app.js"), &read_web("room.js")] {
        // `.textContent =` and text nodes are fine; assigning to anything that parses is not.
        assert!(!source.contains(".innerHTML"));
        assert!(!source.contains("createContextualFragment"));
    }
}

// ----------------------------------------------------------------------------------------------
// W15, W17: side streets and road cuts, at the edges and in two steps
// ----------------------------------------------------------------------------------------------

/// Builds a situation on an edge of The Commons: a street with no builder along the edge (as a
/// highway is), a road of the owner's beside it, and a place of someone else's that only the owner's
/// road serves. `along` is the direction of the edge, `inward` the direction away from it.
fn an_edge_town(
    server: &Server,
    owner: u64,
    start: (i64, i64),
    along: (i64, i64),
    inward: (i64, i64),
    second_road: bool,
) -> ((i64, i64), (i64, i64), (i64, i64)) {
    let at = |a: i64, b: i64| {
        (
            (start.0 + along.0 * a + inward.0 * b) as u16,
            (start.1 + along.1 * a + inward.1 * b) as u16,
        )
    };
    let mut changes = Vec::new();
    for a in 0..12 {
        changes.push(Change::Set(at(a, 0), tile(Kind::Road, 0, None)));
    }
    changes.push(Change::Set(at(5, 1), tile(Kind::Road, 0, Some(owner))));
    if second_road {
        changes.push(Change::Set(at(5, 2), tile(Kind::Road, 0, Some(owner))));
    }
    changes.push(Change::Set(at(5, 4), tile(Kind::House, 0, Some(make_player(&server.state.db)))));
    put_tiles(server, COMMONS, &changes);
    let pos = |t: (u16, u16)| (i64::from(t.0), i64::from(t.1));
    (pos(at(5, 1)), pos(at(5, 2)), pos(at(5, 4)))
}

// W17
#[tokio::test]
async fn a_road_along_any_edge_of_the_map_that_a_place_depends_on_cannot_be_taken_down() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let size = world::COMMONS_SIZE as i64;
    let mut tested = 0;
    for (n, (start, along, inward)) in [
        ((0, 20), (0, 1), (1, 0)),
        ((size - 1, 20), (0, 1), (-1, 0)),
        ((20, 0), (1, 0), (0, 1)),
        ((20, size - 1), (1, 0), (0, -1)),
        ((0, 0), (1, 0), (0, 1)),
        ((size - 12, size - 1), (1, 0), (0, -1)),
    ]
    .into_iter()
    .enumerate()
    {
        let owner = server.person(&format!("Edge {n}")).await;
        let player = person_player(&server, &owner);
        let (road, _, home) = an_edge_town(&server, player.id as u64, start, along, inward, false);
        let before = server.tiles(COMMONS);
        // The highway may pass close enough to serve the place as well: then there is nothing to protect.
        let others = before.iter().filter(|(p, t)| {
            t.kind == Kind::Road
                && (i64::from(p.0), i64::from(p.1)) != road
                && cheb((i64::from(p.0), i64::from(p.1)), home) <= 3
        });
        if others
            .filter(|(p, _)| ![(road.0 as u16, road.1 as u16)].contains(p))
            .any(|(_, t)| t.player != Some(player.id as u64))
        {
            continue;
        }
        tested += 1;
        assert!(
            served_the_long_way(&before).contains(&(home.0 as u16, home.1 as u16)),
            "the situation at {start:?} has nothing to protect"
        );
        refill(&server, &player, now());
        let cut = worlds::act(
            &server.state,
            &player,
            COMMONS,
            &mv("bulldoze", Some(road.0), Some(road.1)),
            now(),
        );
        let error = cut.expect_err(&format!(
            "the road at {road:?} came down at the edge {start:?} and left a place without a road"
        ));
        assert!(error.message.contains("depends on that road"), "{}", error.message);
        assert_eq!(server.tiles(COMMONS), before);
    }
    assert!(tested >= 4, "only {tested} of 6 edges had a place that only the road served");
}

// W16, W17, W57
#[tokio::test]
async fn taking_a_road_down_in_two_steps_that_are_each_allowed_still_cannot_leave_a_place_without_a_road(
) {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let owner = server.person("Owner").await;
    let player = person_player(&server, &owner);
    // The edge at the west, where The Commons' highway doesn't come.
    let (first, second, home) =
        an_edge_town(&server, player.id as u64, (0, 24), (0, 1), (1, 0), true);
    let now_t = now();
    refill(&server, &player, now_t);
    // The tile further out serves the place as well as the first does, so it can go.
    worlds::act(
        &server.state,
        &player,
        COMMONS,
        &mv("bulldoze", Some(second.0), Some(second.1)),
        now_t,
    )
    .unwrap();
    // The first is what is left, and now it cannot.
    let cut = worlds::act(
        &server.state,
        &player,
        COMMONS,
        &mv("bulldoze", Some(first.0), Some(first.1)),
        now_t + 1,
    );
    assert!(cut.is_err(), "the second step left the place at {home:?} without a road");
    // The place is somebody else's, so it can't be taken down to get round that either (W57).
    let theirs = worlds::act(
        &server.state,
        &player,
        COMMONS,
        &mv("bulldoze", Some(home.0), Some(home.1)),
        now_t + 2,
    );
    assert!(theirs.is_err(), "somebody else's place was taken down");
    // Once its owner has taken it down, the road serves nothing and the cut is allowed.
    put_tiles(&server, COMMONS, &[Change::Remove((home.0 as u16, home.1 as u16))]);
    let freed = worlds::act(
        &server.state,
        &player,
        COMMONS,
        &mv("bulldoze", Some(first.0), Some(first.1)),
        now_t + 3,
    );
    assert!(freed.is_ok(), "a road that serves no place was refused: {freed:?}");
    // A road can't be laid over a place, the owner's own or anyone's (W16).
    put_tiles(
        &server,
        COMMONS,
        &[Change::Set(
            (first.0 as u16, first.1 as u16),
            tile(Kind::House, 0, Some(player.id as u64)),
        )],
    );
    let agent = server.guest().await;
    let laid = server.act(&agent.token, COMMONS, road_move(first, first)).await;
    assert_eq!(laid.status, StatusCode::BAD_REQUEST, "{}", laid.show());
    assert_eq!(server.tiles(COMMONS)[&(first.0 as u16, first.1 as u16)].kind, Kind::House);
}

// W15, W54
#[tokio::test]
async fn measure_how_many_people_it_takes_to_turn_every_side_street_of_the_highway_into_a_dead_end()
{
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let land = land_of(&server, COMMONS);
    let tiles = server.tiles(COMMONS);
    let highway: Vec<(i64, i64)> = world::highway(world::COMMONS_SEED, &land)
        .iter()
        .map(|&(x, y)| (i64::from(x), i64::from(y)))
        .collect();
    let kept = |p: (i64, i64)| world::is_reserved(&tiles, (p.0 as u16, p.1 as u16));
    let free = |p: (i64, i64)| open_ground(&land, &tiles, p.0, p.1);
    let mut alleys = Vec::new();
    for &(hx, hy) in &highway {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let street: Vec<(i64, i64)> = (1..=3).map(|i| (hx + dx * i, hy + dy * i)).collect();
            if (street[0].0 + street[0].1).rem_euclid(6) == 0
                && street.iter().all(|&p| free(p) && kept(p))
            {
                alleys.push((street, (dx, dy)));
            }
        }
    }
    let mut walls: HashSet<(i64, i64)> = HashSet::new();
    for (street, (dx, dy)) in &alleys {
        let end = (street[2].0 + dx, street[2].1 + dy);
        let sides = if *dx == 0 { [(1, 0), (-1, 0)] } else { [(0, 1), (0, -1)] };
        for p in street.iter().chain(std::iter::once(&end)) {
            for (sx, sy) in sides {
                walls.insert((p.0 + sx, p.1 + sy));
            }
        }
        walls.insert(end);
    }
    let walls: Vec<(i64, i64)> = walls.into_iter().filter(|&p| free(p) && !kept(p)).collect();
    println!(
        "COST REPORT: the highway has {} side streets of 3 tiles; closing the end and both sides of every one takes {} single-tile places, and a person can have only 4, so {} people each signed in",
        alleys.len(),
        walls.len(),
        walls.len().div_ceil(4)
    );
    // A wall has to be a place, so it takes a person. A guest can't put one up at all.
    let (street, (dx, dy)) = alleys.first().expect("a side street").clone();
    let sides = if dx == 0 { [(1, 0), (-1, 0)] } else { [(0, 1), (0, -1)] };
    let wall = server.guest().await;
    let end = (street[2].0 + dx, street[2].1 + dy);
    let mut to_block: Vec<(i64, i64)> = vec![end];
    for p in &street {
        for (sx, sy) in sides {
            to_block.push((p.0 + sx, p.1 + sy));
        }
    }
    let to_block: Vec<(i64, i64)> = to_block.into_iter().filter(|&p| free(p) && !kept(p)).collect();
    for &p in &to_block {
        let reply = server
            .act(
                &wall.token,
                COMMONS,
                json!({ "action": "place", "kind": "house", "x": p.0, "y": p.1 }),
            )
            .await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.show());
    }
    assert_eq!(server.moves_left(&wall.token).await, 30, "a refused wall cost something");
    assert_eq!(server.tiles(COMMONS), tiles, "a guest built a wall");
    // And with every wall of that street standing anyway, the stub can be laid and cannot go any further.
    let wall_builder = make_player(&server.state.db);
    let builders: Vec<Change> = to_block
        .iter()
        .map(|&p| Change::Set((p.0 as u16, p.1 as u16), tile(Kind::Cafe, 0, Some(wall_builder))))
        .collect();
    put_tiles(&server, COMMONS, &builders);
    let honest = server.guest().await;
    let stub = server.act(&honest.token, COMMONS, road_move(street[0], street[2])).await;
    assert_eq!(stub.status, StatusCode::OK, "{}", stub.show());
    let beyond = server
        .act(
            &honest.token,
            COMMONS,
            road_move(street[0], (street[0].0 + dx * 6, street[0].1 + dy * 6)),
        )
        .await;
    println!("COST REPORT: with {} places around it the side street takes a 3 tile road and a road past it is refused: {}", to_block.len(), beyond.show());
    assert_eq!(beyond.status, StatusCode::BAD_REQUEST);
}

// ----------------------------------------------------------------------------------------------
// A person's sessions, their players, and moving between worlds and private life
// ----------------------------------------------------------------------------------------------

async fn session_for(server: &Server, key: &str) -> String {
    let reply = server.post("/api/session", json!({ "owner_key": key })).send().await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.show());
    reply
        .headers
        .get("set-cookie")
        .expect("a session cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

fn wave() -> Value {
    json!({ "action": "emote", "emote": "wave" })
}

// A7, A8, A9, W2, W9
#[tokio::test]
async fn ending_a_session_replacing_the_key_or_removing_a_passkey_stops_exactly_what_it_should_in_a_world(
) {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let (a, b, c) = (
        session_for(&server, &ann.key).await,
        session_for(&server, &ann.key).await,
        session_for(&server, &ann.key).await,
    );
    let acts = |cookie: &str| {
        let cookie = cookie.to_string();
        let server = &server;
        async move { server.post("/api/worlds/commons/act", wave()).cookie(&cookie).send().await.status }
    };
    let mut spent = 0;
    for cookie in [&a, &b, &c] {
        assert_eq!(acts(cookie).await, StatusCode::OK);
        spent += 1;
    }
    // Signing out ends that one device, and only it.
    assert_eq!(
        server.req(Method::DELETE, "/api/session").cookie(&a).send().await.status,
        StatusCode::OK
    );
    assert_eq!(
        acts(&a).await,
        StatusCode::UNAUTHORIZED,
        "a session that was signed out still builds"
    );
    assert_eq!(acts(&b).await, StatusCode::OK);
    spent += 1;
    // A guest claimed on the way: it is Ann's now, and it has a token of its own.
    let guest = server.guest().await;
    let claimed = server
        .post("/api/players/claim", json!({ "claim_code": guest.claim }))
        .cookie(&b)
        .send()
        .await;
    assert_eq!(claimed.status, StatusCode::OK, "{}", claimed.show());
    let claimed_id = claimed.body["player"]["id"].as_i64().unwrap();
    assert_eq!(server.act(&guest.token, COMMONS, wave()).await.status, StatusCode::OK);
    // Replacing the key ends every session, the old key, and starts one for the device that did it.
    let rotated = server.post("/api/key/rotate", json!({})).cookie(&b).send().await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.show());
    let new_key = rotated.body["owner_key"].as_str().unwrap().to_string();
    assert_eq!(
        acts(&b).await,
        StatusCode::UNAUTHORIZED,
        "the old session of the device that rotated still builds"
    );
    assert_eq!(
        acts(&c).await,
        StatusCode::UNAUTHORIZED,
        "another device's session survived a new key"
    );
    assert_eq!(
        server.post("/api/worlds/commons/act", wave()).token(&ann.key).send().await.status,
        StatusCode::UNAUTHORIZED,
        "the old owner key still builds"
    );
    assert_eq!(
        server.post("/api/worlds/commons/act", wave()).token(&new_key).send().await.status,
        StatusCode::OK
    );
    spent += 1;
    // What the new key does not end: the claimed player keeps its token. The person can end it, one by one.
    let survives = server.act(&guest.token, COMMONS, wave()).await.status == StatusCode::OK;
    println!(
        "COST REPORT: after the owner key was replaced, the claimed player's token {}",
        if survives { "still works (the person must revoke it by hand)" } else { "stopped" }
    );
    // Removing a passkey ends the other devices' sessions and keeps the one that did it.
    let (d, e) = (session_for(&server, &new_key).await, session_for(&server, &new_key).await);
    server
        .state
        .db
        .add_passkey(
            &ann.id,
            &agenthangout::db::NewPasskey {
                id: "pk0000000000001",
                credential_id: "cred1",
                passkey: "{}",
                label: "phone",
            },
            20,
            now(),
        )
        .unwrap();
    assert_eq!(
        server.req(Method::DELETE, "/api/passkeys/pk0000000000001").cookie(&d).send().await.status,
        StatusCode::OK
    );
    assert_eq!(
        acts(&e).await,
        StatusCode::UNAUTHORIZED,
        "a passkey was removed and another device's session still builds"
    );
    assert_eq!(acts(&d).await, StatusCode::OK);
    spent += 1;
    // None of it refilled a day: the person has the moves they had, less what they spent.
    let left = server.post("/api/worlds/commons/act", wave()).token(&new_key).send().await.ok()
        ["moves_left"]
        .as_i64()
        .unwrap();
    assert_eq!(left, 300 - spent - 1, "signing in again changed how many moves a person has");
    // The person ends the claimed player; it can't build, can't be claimed again, and what it built stays.
    let revoked = server
        .req(Method::DELETE, &format!("/api/players/{claimed_id}"))
        .token(&new_key)
        .send()
        .await;
    assert_eq!(revoked.status, StatusCode::OK, "{}", revoked.show());
    assert_eq!(server.act(&guest.token, COMMONS, wave()).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        server
            .req(Method::DELETE, &format!("/api/players/{claimed_id}"))
            .token(&new_key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .post("/api/players/claim", json!({ "claim_code": guest.claim }))
            .token(&new_key)
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

// W10, W11
#[tokio::test]
async fn a_claim_code_is_dead_once_its_guest_has_idled_out_and_a_claimed_guest_never_idles_out() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let (stale, nearly, fresh) = (server.guest().await, server.guest().await, server.guest().await);
    let at = now();
    for (guest, ago) in [(&stale, 7 * 86_400 + 60), (&nearly, 7 * 86_400 - 120)] {
        let id = server.player_id(&guest.token).await;
        last_seen(&server, &server.player_row(id), at, ago);
    }
    // A guest that idled out can't be claimed, and its token is dead.
    let late = server
        .post("/api/players/claim", json!({ "claim_code": stale.claim }))
        .token(&ann.key)
        .send()
        .await;
    assert_eq!(late.status, StatusCode::NOT_FOUND, "{}", late.show());
    assert_eq!(server.act(&stale.token, COMMONS, wave()).await.status, StatusCode::UNAUTHORIZED);
    // One that was about to is claimed in time, and from then on it is the person's.
    let in_time = server
        .post("/api/players/claim", json!({ "claim_code": nearly.claim }))
        .token(&ann.key)
        .send()
        .await;
    assert_eq!(in_time.status, StatusCode::OK, "{}", in_time.show());
    let month_on = at + 30 * 86_400;
    let headers = |token: &str| {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_str(&format!("Bearer {token}")).unwrap());
        headers
    };
    assert!(
        worlds::player_from_headers(&server.state, &headers(&nearly.token), month_on).is_ok(),
        "a claimed player idled out"
    );
    assert!(
        worlds::player_from_headers(&server.state, &headers(&fresh.token), month_on).is_err(),
        "a guest nobody claimed never idled out"
    );
    // Claimed, a guest still plays in The Commons with a bigger day.
    server
        .post("/api/players/claim", json!({ "claim_code": fresh.claim }))
        .token(&ann.key)
        .send()
        .await
        .ok();
    assert_eq!(server.me(&fresh.token, COMMONS).await["moves_per_day"], 200);
}

// W1, W3, W4, W18
#[tokio::test]
async fn an_owner_removing_a_bots_player_from_their_frozen_world_changes_nothing_in_private_life_and_says_nothing_of_it(
) {
    let path = temp_db("removal");
    let server = Server::on(Db::open(&path).unwrap()).await;
    // Alice has a city and a house and a bot called Pixel in its living room.
    let made = server
        .post("/api/cities", json!({ "name": "Friends", "your_name": "Alice" }))
        .send()
        .await
        .ok();
    let (alice_key, alice_id, city) = (
        made["owner_key"].as_str().unwrap().to_string(),
        made["person"]["id"].as_str().unwrap().to_string(),
        made["city"]["id"].as_str().unwrap().to_string(),
    );
    let house = server
        .post(&format!("/api/cities/{city}/houses"), json!({ "name": "Alice House" }))
        .token(&alice_key)
        .send()
        .await
        .ok();
    let living = house["living_room_id"].as_str().unwrap().to_string();
    let pixel = server.bot_in(&alice_key, &alice_id, &living, "Pixel").await;
    // Carol's world froze with some of Pixel's tiles in it, from back when it could build there.
    let carol = server.person("Carol").await;
    let world = "frozen0000000001";
    server.frozen_world(world, Some(&carol.id), true, 4242, now());
    let token = server.bot_player(&pixel).await;
    let player_id = server.player_id(&token).await;
    put_tiles(
        &server,
        world,
        &[
            Change::Set((20, 20), tile(Kind::Road, 0, Some(player_id as u64))),
            Change::Set((21, 20), tile(Kind::Homes, 1, Some(player_id as u64))),
            Change::Set((22, 20), tile(Kind::Road, 0, Some(player_id as u64))),
        ],
    );
    server.fresh_view(world).await;
    let before = dump(&path);

    let removed = server
        .post(&format!("/api/worlds/{world}/players/{player_id}/remove"), json!({}))
        .token(&carol.key)
        .send()
        .await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.show());
    assert_eq!(removed.body["tiles_removed"], 3);
    // Carol's answer is a count of tiles, and nothing that names the bot, its owner, their city or house.
    for secret in [
        alice_id.as_str(),
        pixel.agent.as_str(),
        city.as_str(),
        pixel.token.as_str(),
        alice_key.as_str(),
        "Pixel",
        "Alice",
        "Friends",
    ] {
        assert!(!removed.text.contains(secret), "the answer shows {secret}: {}", removed.text);
    }
    let after = dump(&path);
    let world_tables = [
        "worlds",
        "world_tiles",
        "world_buildings",
        "players",
        "world_players",
        "world_members",
        "crawler_sightings",
        "place_clocks",
        "world_ends",
        "world_squares",
    ];
    for (table, rows) in &after {
        if !world_tables.contains(&table.as_str()) {
            assert_eq!(rows, &before[table], "removing a player from a world changed {table}");
        }
    }
    assert!(
        server.tiles(world).values().all(|t| t.player != Some(player_id as u64)),
        "what the bot built stayed"
    );
    assert_eq!(after["world_members"].len(), 1, "the bot was not barred from that world");
    // The bot is only barred from that world, and is a player everywhere else, The Commons included.
    assert_eq!(server.act(&token, world, wave()).await.status, StatusCode::FORBIDDEN);
    assert_eq!(
        server.act(&token, COMMONS, wave()).await.status,
        StatusCode::OK,
        "one world's owner ended a player for the whole server"
    );
    assert_eq!(
        server
            .post("/api/public/join", json!({ "world_id": world }))
            .token(&pixel.token)
            .ip("198.51.100.5")
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let elsewhere = server
        .post("/api/public/join", json!({}))
        .token(&pixel.token)
        .ip("198.51.100.5")
        .send()
        .await;
    assert_eq!(elsewhere.status, StatusCode::OK, "{}", elsewhere.show());
    // And Carol can't tell from this which of her visitors belongs to a person she knows.
    let again = server
        .post(&format!("/api/worlds/{world}/players/{player_id}/remove"), json!({}))
        .token(&carol.key)
        .send()
        .await;
    assert!(again.status.is_success() || again.status == StatusCode::NOT_FOUND, "{}", again.show());
    drop(server);
    remove_db(&path);
}

impl Server {
    async fn fresh_view(&self, world: &str) -> Value {
        tokio::time::sleep(Duration::from_millis(1100)).await;
        self.view(world).await
    }
}

// W3
#[tokio::test]
async fn a_bot_can_be_deleted_whatever_became_of_its_public_player() {
    // No route deletes a bot today, but the database says it can be done, and the trigger that ends
    // a bot's player has to leave that possible. A person with a player of their own, whose bot joined
    // and then had its token replaced, is the case with two players that would look the same afterwards.
    let path = temp_db("trigger");
    let server = Server::on(Db::open(&path).unwrap()).await;
    let bot = server.bot("Looper").await;
    assert_eq!(
        server.act(&bot.key, COMMONS, wave()).await.status,
        StatusCode::OK,
        "the person's own player"
    );
    let _token = server.bot_player(&bot).await;
    let rotated = server
        .post(&format!("/api/agents/{}/rotate", bot.agent), json!({}))
        .token(&bot.key)
        .send()
        .await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.show());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.busy_timeout(Duration::from_secs(5)).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    let deleted = conn.execute("DELETE FROM agents WHERE id = ?", [&bot.agent]);
    assert!(deleted.is_ok(), "deleting a bot failed: {deleted:?}");
    drop(conn);
    drop(server);
    remove_db(&path);
}

// ----------------------------------------------------------------------------------------------
// The database the live server has today, and the code that will open it
// ----------------------------------------------------------------------------------------------

/// The schema of the code that is live: every table of private life, and nothing for accounts or worlds.
const LIVE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS people (id TEXT PRIMARY KEY, name TEXT NOT NULL, color TEXT NOT NULL, key_hash TEXT NOT NULL UNIQUE, feed_token TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS hangouts (id TEXT PRIMARY KEY, name TEXT NOT NULL, invite_code TEXT NOT NULL UNIQUE, created_by TEXT NOT NULL REFERENCES people(id), created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS members (hangout_id TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE, person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE, joined_at INTEGER NOT NULL, PRIMARY KEY (hangout_id, person_id));
CREATE TABLE IF NOT EXISTS agents (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE, name TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE, status TEXT, last_active_at INTEGER, last_seen_at INTEGER, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS agent_activity (agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE, kind TEXT NOT NULL, via TEXT NOT NULL, until_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS agent_doorbell (agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE, paused INTEGER NOT NULL DEFAULT 0, checked_at INTEGER);
CREATE TABLE IF NOT EXISTS room_agents (hangout_id TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE, agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE, PRIMARY KEY (hangout_id, agent_id));
CREATE TABLE IF NOT EXISTS messages (id INTEGER PRIMARY KEY AUTOINCREMENT, hangout_id TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE, person_id TEXT REFERENCES people(id) ON DELETE SET NULL, agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL, speaker TEXT NOT NULL, kind TEXT NOT NULL CHECK (kind IN ('chat', 'note', 'system')), body TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS messages_by_room ON messages (hangout_id, id);
CREATE TABLE IF NOT EXISTS events (id TEXT PRIMARY KEY, hangout_id TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE, title TEXT NOT NULL, location TEXT NOT NULL DEFAULT '', notes TEXT NOT NULL DEFAULT '', starts_at INTEGER NOT NULL, ends_at INTEGER NOT NULL, status TEXT NOT NULL CHECK (status IN ('proposed', 'confirmed', 'cancelled')), min_yes INTEGER NOT NULL, sequence INTEGER NOT NULL DEFAULT 0, proposed_by TEXT NOT NULL REFERENCES people(id), proposer TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS events_by_room ON events (hangout_id, starts_at);
CREATE TABLE IF NOT EXISTS check_ins (id TEXT PRIMARY KEY, hangout_id TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE, asked_by TEXT NOT NULL REFERENCES people(id), asker TEXT NOT NULL, agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL, about TEXT NOT NULL, to_person TEXT REFERENCES people(id), needs_human INTEGER NOT NULL, status TEXT NOT NULL CHECK (status IN ('waiting', 'go', 'hold')), answered_by TEXT, answered_by_bot INTEGER NOT NULL DEFAULT 0, note TEXT NOT NULL DEFAULT '', expires_at INTEGER NOT NULL, created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS check_ins_by_room ON check_ins (hangout_id, created_at);
CREATE TABLE IF NOT EXISTS notify_targets (owner_id TEXT PRIMARY KEY, url TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS rsvps (event_id TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE, person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE, response TEXT NOT NULL CHECK (response IN ('yes', 'maybe', 'no')), via_agent TEXT, note TEXT NOT NULL DEFAULT '', updated_at INTEGER NOT NULL, PRIMARY KEY (event_id, person_id));
CREATE TABLE IF NOT EXISTS cities (id TEXT PRIMARY KEY, name TEXT NOT NULL, invite_code TEXT NOT NULL UNIQUE, square_id TEXT NOT NULL REFERENCES hangouts(id), created_by TEXT NOT NULL REFERENCES people(id), created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS city_members (city_id TEXT NOT NULL REFERENCES cities(id) ON DELETE CASCADE, person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE, joined_at INTEGER NOT NULL, PRIMARY KEY (city_id, person_id));
CREATE TABLE IF NOT EXISTS houses (id TEXT PRIMARY KEY, city_id TEXT NOT NULL REFERENCES cities(id) ON DELETE CASCADE, name TEXT NOT NULL, invite_code TEXT NOT NULL UNIQUE, living_room TEXT NOT NULL REFERENCES hangouts(id), porch TEXT NOT NULL REFERENCES hangouts(id), created_by TEXT NOT NULL REFERENCES people(id), created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS residents (house_id TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE, person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE, joined_at INTEGER NOT NULL, PRIMARY KEY (house_id, person_id));
CREATE TABLE IF NOT EXISTS bot_homes (agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE, house_id TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE, stays_home INTEGER NOT NULL DEFAULT 0, is_house INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS trips (id TEXT PRIMARY KEY, agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE, house_id TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE, kind TEXT NOT NULL CHECK (kind IN ('call', 'visit', 'stay')), goal TEXT NOT NULL DEFAULT '', minutes INTEGER NOT NULL, status TEXT NOT NULL CHECK (status IN ('asking', 'here', 'declined', 'ended')), asked_by TEXT NOT NULL, owner_ok_by TEXT, host_ok_by TEXT, host_ok_id TEXT, pass_id TEXT, arrived_at INTEGER, ends_at INTEGER, ended_by TEXT, expires_at INTEGER NOT NULL, created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS trips_by_bot ON trips (agent_id, status);
CREATE INDEX IF NOT EXISTS trips_by_house ON trips (house_id, status);
CREATE TABLE IF NOT EXISTS passes (id TEXT PRIMARY KEY, house_id TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE, agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE, max_minutes INTEGER NOT NULL, granted_by TEXT NOT NULL, accepted_by TEXT, created_at INTEGER NOT NULL, UNIQUE (house_id, agent_id));
CREATE TABLE IF NOT EXISTS scenes (id TEXT PRIMARY KEY, house_id TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE, name TEXT NOT NULL, created_at INTEGER NOT NULL, UNIQUE (house_id, name));
CREATE TABLE IF NOT EXISTS scene_requests (id TEXT PRIMARY KEY, scene_id TEXT NOT NULL REFERENCES scenes(id) ON DELETE CASCADE, house_id TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE, asked_by TEXT NOT NULL, status TEXT NOT NULL CHECK (status IN ('waiting', 'approved', 'declined', 'done', 'failed')), answered_by TEXT, expires_at INTEGER NOT NULL, created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS scene_requests_by_house ON scene_requests (house_id, status);
CREATE TABLE IF NOT EXISTS house_status (house_id TEXT PRIMARY KEY REFERENCES houses(id) ON DELETE CASCADE, state TEXT NOT NULL CHECK (state IN ('ok', 'attention')), updated_at INTEGER NOT NULL);
"#;

/// A database as the live server has it, with the sort of rows a few months of use leave: people,
/// rooms, bots, a long chat, plans and answers, a city with a house, a house bot, trips and scenes.
/// The keys and tokens it returns are the plain ones whose hashes are stored.
struct LiveRows {
    justin_key: String,
    dana_key: String,
    claude_token: String,
    casa_token: String,
}

fn make_the_live_database(path: &str) -> LiveRows {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    conn.execute_batch(LIVE_SCHEMA).unwrap();
    let t0 = now() - 60 * 86_400;
    let rows = LiveRows {
        justin_key: auth::new_owner_key(),
        dana_key: auth::new_owner_key(),
        claude_token: auth::new_agent_token(),
        casa_token: auth::new_agent_token(),
    };
    let h = auth::hash_secret;
    let q = |sql: &str, params: &[&dyn rusqlite::ToSql]| {
        conn.execute(sql, params).unwrap_or_else(|e| panic!("{sql}: {e}"));
    };
    q("INSERT INTO people VALUES ('justin0000000001', 'Justin', '#4f9dff', ?, 'feedjustin0000000000000000000000000000001', ?)", &[&h(&rows.justin_key), &t0]);
    q("INSERT INTO people VALUES ('dana00000000002', 'Dana', '#ff8a3d', ?, 'feeddana000000000000000000000000000000002', ?)", &[&h(&rows.dana_key), &t0]);
    q("INSERT INTO hangouts VALUES ('flowhaus0000001', 'FlowHaus', 'invitecodeflowhaus0000001', 'justin0000000001', ?)", &[&t0]);
    q("INSERT INTO hangouts VALUES ('square000000001', 'Friends Square', 'invitecodesquare0000000001', 'justin0000000001', ?)", &[&t0]);
    q("INSERT INTO hangouts VALUES ('casaliving00001', 'Casa Living Room', 'invitecodecasaliving00001', 'justin0000000001', ?)", &[&t0]);
    q("INSERT INTO hangouts VALUES ('casaporch00001a', 'Casa Porch', 'invitecodecasaporch000001', 'justin0000000001', ?)", &[&t0]);
    for (hangout, person) in [
        ("flowhaus0000001", "justin0000000001"),
        ("flowhaus0000001", "dana00000000002"),
        ("square000000001", "justin0000000001"),
        ("square000000001", "dana00000000002"),
        ("casaliving00001", "justin0000000001"),
        ("casaporch00001a", "justin0000000001"),
    ] {
        q("INSERT INTO members VALUES (?, ?, ?)", &[&hangout, &person, &t0]);
    }
    q("INSERT INTO agents VALUES ('claude0000000001', 'justin0000000001', 'Claude', ?, 'Checking calendars', ?, ?, ?)", &[&h(&rows.claude_token), &(t0 + 100), &(t0 + 200), &t0]);
    q("INSERT INTO agents VALUES ('grok00000000002a', 'dana00000000002', 'Grok', ?, NULL, NULL, NULL, ?)", &[&h(&auth::new_agent_token()), &t0]);
    q("INSERT INTO agents VALUES ('casa00000000003a', 'justin0000000001', 'Casa', ?, NULL, NULL, NULL, ?)", &[&h(&rows.casa_token), &t0]);
    q(
        "INSERT INTO agent_activity VALUES ('claude0000000001', 'computer', 'FlowRMM', ?)",
        &[&(t0 + 400)],
    );
    q("INSERT INTO agent_doorbell VALUES ('claude0000000001', 0, ?)", &[&(t0 + 500)]);
    for (room, agent) in [
        ("flowhaus0000001", "claude0000000001"),
        ("flowhaus0000001", "grok00000000002a"),
        ("casaliving00001", "claude0000000001"),
    ] {
        q("INSERT INTO room_agents VALUES (?, ?)", &[&room, &agent]);
    }
    for n in 0..25i64 {
        let (speaker, kind) = if n % 7 == 6 {
            ("Justin", "note")
        } else if n % 2 == 0 {
            ("Claude", "chat")
        } else {
            ("Grok", "chat")
        };
        let body = format!("Message {n}: can we do tacos on Friday? <b>bold</b> \"quoted\" 'single' ; DROP TABLE people; --");
        q("INSERT INTO messages (hangout_id, person_id, agent_id, speaker, kind, body, created_at) VALUES ('flowhaus0000001', NULL, NULL, ?, ?, ?, ?)", &[&speaker, &kind, &body, &(t0 + 1_000 + n * 60)]);
    }
    q("INSERT INTO events VALUES ('event00000000001', 'flowhaus0000001', 'Tacos', 'Veracruz', 'Bring cash', ?, ?, 'confirmed', 2, 1, 'justin0000000001', 'Claude', ?, ?)", &[&(now() + 86_400), &(now() + 86_400 + 7_200), &t0, &t0]);
    q("INSERT INTO events VALUES ('event00000000002', 'flowhaus0000001', 'Hike', '', '', ?, ?, 'proposed', 2, 0, 'dana00000000002', 'Grok', ?, ?)", &[&(now() + 2 * 86_400), &(now() + 2 * 86_400 + 3_600), &t0, &t0]);
    q(
        "INSERT INTO rsvps VALUES ('event00000000001', 'justin0000000001', 'yes', 'Claude', '', ?)",
        &[&t0],
    );
    q("INSERT INTO rsvps VALUES ('event00000000001', 'dana00000000002', 'yes', NULL, 'see you', ?)", &[&t0]);
    q("INSERT INTO check_ins VALUES ('checkin00000001a', 'flowhaus0000001', 'justin0000000001', 'Claude', 'claude0000000001', 'Booking the table', 'dana00000000002', 0, 'go', 'Dana', 0, 'fine', ?, ?)", &[&(t0 + 3_600), &t0]);
    q("INSERT INTO notify_targets VALUES ('justin0000000001', 'https://example.com/hook')", &[]);
    q("INSERT INTO cities VALUES ('city00000000001a', 'Friends', 'invitecodecity000000001a', 'square000000001', 'justin0000000001', ?)", &[&t0]);
    for person in ["justin0000000001", "dana00000000002"] {
        q("INSERT INTO city_members VALUES ('city00000000001a', ?, ?)", &[&person, &t0]);
    }
    q("INSERT INTO houses VALUES ('house0000000001a', 'city00000000001a', 'Casa', 'invitecodehouse00000001a', 'casaliving00001', 'casaporch00001a', 'justin0000000001', ?)", &[&t0]);
    q("INSERT INTO residents VALUES ('house0000000001a', 'justin0000000001', ?)", &[&t0]);
    q("INSERT INTO bot_homes VALUES ('claude0000000001', 'house0000000001a', 0, 0)", &[]);
    q("INSERT INTO bot_homes VALUES ('casa00000000003a', 'house0000000001a', 0, 1)", &[]);
    q("INSERT INTO trips VALUES ('trip00000000001a', 'grok00000000002a', 'house0000000001a', 'visit', 'Pair on the build', 1440, 'ended', 'grok00000000002a', 'dana00000000002', 'justin0000000001', NULL, NULL, ?, ?, 'justin0000000001', ?, ?)", &[&(t0 + 10), &(t0 + 20), &(t0 + 100), &t0]);
    q("INSERT INTO passes VALUES ('pass00000000001a', 'house0000000001a', 'grok00000000002a', 60, 'justin0000000001', 'dana00000000002', ?)", &[&t0]);
    q(
        "INSERT INTO scenes VALUES ('scene00000000001a', 'house0000000001a', 'Movie Night', ?)",
        &[&t0],
    );
    q("INSERT INTO scene_requests VALUES ('screq00000000001a', 'scene00000000001a', 'house0000000001a', 'justin0000000001', 'done', 'justin0000000001', ?, ?)", &[&(t0 + 900), &t0]);
    q("INSERT INTO house_status VALUES ('house0000000001a', 'ok', ?)", &[&t0]);
    let fk: i64 = conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(fk, 0, "the live database as the test makes it is not consistent");
    rows
}

// R38
#[tokio::test]
async fn the_database_the_live_server_has_upgrades_safely_and_the_same_way_every_time() {
    let path = temp_db("live");
    let rows = make_the_live_database(&path);
    let old_tables = dump(&path);
    assert!(old_tables.keys().all(|t| ![
        "players",
        "worlds",
        "sessions",
        "passkeys",
        "world_tiles"
    ]
    .contains(&t.as_str())));

    // The new code opens it. Nothing that was there changes, and the new tables are empty.
    drop(Db::open(&path).unwrap());
    let upgraded = dump(&path);
    for (table, was) in &old_tables {
        assert_eq!(&upgraded[table], was, "opening the database changed {table}");
    }
    let new_tables: Vec<&String> =
        upgraded.keys().filter(|t| !old_tables.contains_key(*t)).collect();
    assert!(new_tables.len() >= 13, "{new_tables:?}");
    for table in &new_tables {
        assert!(
            upgraded[*table].is_empty(),
            "{table} has rows nobody put there: {:?}",
            upgraded[*table]
        );
    }
    // Starting it again and again changes nothing: same schema, same rows.
    let schema = schema_of(&path);
    for _ in 0..3 {
        drop(Db::open(&path).unwrap());
    }
    assert_eq!(schema_of(&path), schema, "opening the database again changed its schema");
    assert_eq!(dump(&path), upgraded, "opening the database again changed its rows");
    for needed in [
        "players_end_with_their_bot",
        "players_one_own_per_person",
        "worlds_one_each",
        "sessions_by_person",
    ] {
        assert!(schema.iter().any(|s| s.contains(needed)), "{needed} is missing");
    }

    // The server runs on it. The people who were there get in, with everything they had.
    let server = Server::on(Db::open(&path).unwrap()).await;
    let me = server.get("/api/me").token(&rows.justin_key).send().await.ok();
    assert_eq!(me["name"], "Justin");
    let room =
        server.get("/api/hangouts/flowhaus0000001").token(&rows.justin_key).send().await.ok();
    assert_eq!(
        room["messages"].as_array().unwrap().len() + room["notes"].as_array().unwrap().len(),
        25,
        "the chat did not all arrive"
    );
    assert!(
        room["messages"][0]["body"].as_str().unwrap().contains("DROP TABLE people"),
        "a message was changed"
    );
    assert_eq!(room["events"].as_array().unwrap().len(), 2);
    let city = server.get("/api/cities/city00000000001a").token(&rows.dana_key).send().await.ok();
    assert_eq!(city["houses"].as_array().unwrap().len(), 1);
    let inbox = server.get("/api/inbox").token(&rows.claude_token).send().await;
    assert_eq!(inbox.status, StatusCode::OK, "{}", inbox.show());
    let queue = server.get("/api/house/queue").token(&rows.casa_token).send().await;
    assert_eq!(queue.status, StatusCode::OK, "{}", queue.show());
    // A bot that is the house's bot still can't play, and one that is Justin's can.
    assert_eq!(
        server.post("/api/public/join", json!({})).token(&rows.casa_token).send().await.status,
        StatusCode::FORBIDDEN
    );
    let claude_player =
        server.post("/api/public/join", json!({})).token(&rows.claude_token).send().await;
    assert_eq!(claude_player.status, StatusCode::OK, "{}", claude_player.show());
    // The public worlds work on it, and so do passkey sign-ins and sessions for the people who were there.
    let cookie = session_for(&server, &rows.justin_key).await;
    assert_eq!(
        server.post("/api/worlds/commons/act", wave()).cookie(&cookie).send().await.status,
        StatusCode::OK
    );
    let guest = server.guest().await;
    assert_eq!(server.act(&guest.token, COMMONS, wave()).await.status, StatusCode::OK);
    let (_, commons) = (0, server.tiles(COMMONS));
    assert!(
        commons.len() > 40 && commons.values().all(|t| t.player.is_none()),
        "The Commons is not the highway"
    );
    drop(server);

    // And a restart on top of all that: the same Commons, the same tiles, and still no complaints.
    let before_restart = dump(&path);
    let again = Server::on(Db::open(&path).unwrap()).await;
    worlds::ensure_commons(&again.state).unwrap();
    drop(again);
    let after_restart = dump(&path);
    for table in ["world_tiles", "worlds", "players", "people", "messages", "agents", "hangouts"] {
        let (b, a) = (&before_restart[table], &after_restart[table]);
        if table == "players" || table == "worlds" {
            // A restart touches nothing about who plays; the world's step is the one thing that moves.
            assert_eq!(b.len(), a.len(), "{table}");
        } else {
            assert_eq!(b, a, "a restart changed {table}");
        }
    }
    let conn = rusqlite::Connection::open(&path).unwrap();
    let broken: i64 = conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| row.get(0))
        .unwrap();
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0)).unwrap();
    assert_eq!((broken, integrity.as_str()), (0, "ok"));
    remove_db(&path);
}

// R38
#[test]
fn an_old_database_with_the_one_player_a_person_index_from_before_the_swap_opens_and_keeps_its_rows(
) {
    // The first version of the worlds tables had one index on players per person. A database that has
    // it, with rows that the new index would refuse if they were counted the same way, must still open.
    let path = temp_db("indexswap");
    {
        drop(Db::open(&path).unwrap());
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP INDEX IF EXISTS players_one_own_per_person;
             CREATE UNIQUE INDEX players_one_per_person ON players (person_id) WHERE person_id IS NOT NULL AND token_hash IS NULL;",
        )
        .unwrap();
        conn.execute("INSERT INTO people VALUES ('p1', 'A', '#fff', 'k1', 'f1', 0)", []).unwrap();
        conn.execute("INSERT INTO players (name_seed, person_id, seen_at, created_at) VALUES (5, 'p1', 0, 0)", []).unwrap();
    }
    let before = dump(&path);
    drop(Db::open(&path).expect("the new code could not open a database that had the old index"));
    drop(Db::open(&path).unwrap());
    let after = dump(&path);
    assert_eq!(before["players"], after["players"]);
    let schema = schema_of(&path);
    assert!(
        !schema.iter().any(|s| s.contains("players_one_per_person")),
        "the old index is still there"
    );
    assert!(schema.iter().any(|s| s.contains("players_one_own_per_person")));
    remove_db(&path);
}

// ----------------------------------------------------------------------------------------------
// The guide, as written
// ----------------------------------------------------------------------------------------------

// W45, W47
#[tokio::test]
async fn every_road_the_guide_shows_works_as_written_and_costs_a_move_a_tile_and_no_other_build_is_taught(
) {
    let server = Server::start().await;
    let guide = server.get("/agents.md").send().await.text;
    // Every `build` the guide shows, as the `{"type": ..., ...}` it is written as.
    let mut calls = Vec::new();
    let mut rest = guide.as_str();
    while let Some(at) = rest.find("{\"type\": \"") {
        let from = &rest[at..];
        let end = from.find('}').expect("a call in the guide that never ends");
        calls.push(from[..=end].to_string());
        rest = &from[end..];
    }
    assert!(!calls.is_empty(), "the guide shows no build call");
    let joined = server.tool("join", json!({})).await;
    let token = joined["player_token"].as_str().unwrap().to_string();
    assert_eq!(joined["moves_left"], 30);
    let mut left = 30;
    for call in &calls {
        let mut arguments: Value = serde_json::from_str(call)
            .unwrap_or_else(|e| panic!("the guide shows {call}, which is not JSON: {e}"));
        // W13: a road is all an agent can build now, so it is all the guide may show.
        assert_eq!(
            arguments["type"], "road",
            "the guide shows {call}, and only a road can be built"
        );
        let tiles = 1
            + (arguments["x2"].as_i64().unwrap_or(arguments["x"].as_i64().unwrap())
                - arguments["x"].as_i64().unwrap())
            .abs()
            + (arguments["y2"].as_i64().unwrap_or(arguments["y"].as_i64().unwrap())
                - arguments["y"].as_i64().unwrap())
            .abs();
        arguments["player_token"] = json!(token);
        let done = server.tool("build", arguments).await;
        let cost = done["cost"].as_i64().unwrap();
        left -= cost;
        assert_eq!(done["moves_left"].as_i64(), Some(left), "{call}: {done}");
        assert_eq!(
            cost + done["skipped"].as_i64().unwrap(),
            tiles,
            "{call}: a road costs a move a tile, and a tile already there is skipped: {done}"
        );
    }
    assert!(left >= 0, "the guide's first moves cost more than a guest's 30");
    // The highway is where the guide says it is.
    let land = land_of(&server, COMMONS);
    let highway: HashSet<(i64, i64)> = world::highway(world::COMMONS_SEED, &land)
        .iter()
        .map(|&(x, y)| (i64::from(x), i64::from(y)))
        .collect();
    assert!(highway.iter().any(|&(_, y)| y == 0), "the highway does not come in from the top edge");
    assert!(
        (24..=64).all(|y| highway.contains(&(46, y))),
        "the highway does not run down x 46 to y 64"
    );
    assert!(
        (46..=64).all(|x| highway.contains(&(x, 64))),
        "the highway does not run east along y 64 to the middle"
    );
}

// W45, W87
#[tokio::test]
async fn first_moves_still_work_once_the_road_the_guide_shows_is_built() {
    let server = Server::start().await;
    let guide = server.get("/agents.md").send().await.text;
    let joined = server.tool("join", json!({})).await;
    let token = joined["player_token"].as_str().unwrap().to_string();

    // Somebody has laid the road the guide shows, as they have on the live map.
    let from = &guide[guide.find("{\"type\": \"").unwrap()..];
    let mut shown: Value = serde_json::from_str(&from[..=from.find('}').unwrap()]).unwrap();
    shown["player_token"] = json!(token);
    assert_eq!(server.tool("build", shown).await["cost"], 10);

    // An agent does what First moves says: a straight run of streets with one end beside a road.
    let look = server
        .tool("look", json!({ "player_token": token, "x": 52, "y": 42, "w": 24, "h": 16 }))
        .await;
    let at = |tile: &Value| (tile[0].as_i64().unwrap(), tile[1].as_i64().unwrap());
    let streets: Vec<(i64, i64)> = look["streets"].as_array().unwrap().iter().map(at).collect();
    let built = look["built"].as_array().unwrap().iter();
    let roads: Vec<(i64, i64)> = built.filter(|tile| tile[2] == "road").map(at).collect();
    // Longer than a lane, so it's the streets that let it through (W85).
    let run =
        |(x, y): (i64, i64), (dx, dy): (i64, i64)| (0..4).map(move |i| (x + dx * i, y + dy * i));
    let (first, step) = streets
        .iter()
        .flat_map(|&tile| [(1, 0), (-1, 0), (0, 1), (0, -1)].map(|step| (tile, step)))
        .find(|&(tile, step)| {
            roads.contains(&(tile.0 - step.0, tile.1 - step.1))
                && run(tile, step).all(|next| streets.contains(&next))
        })
        .expect("no run of streets starts beside a road");
    let last = run(first, step).next_back().unwrap();
    let done = server
        .tool(
            "build",
            json!({ "player_token": token, "type": "road", "x": first.0, "y": first.1, "x2": last.0, "y2": last.1 }),
        )
        .await;
    assert_eq!((done["cost"].as_i64(), done["changed"].as_i64()), (Some(4), Some(4)), "{done}");
}

// W45
#[tokio::test]
async fn every_number_the_public_pages_promise_is_the_one_the_server_uses() {
    let server = Server::start().await;
    let home = server.get("/").send().await.text;
    let llms = server.get("/llms.txt").send().await.text;
    let guide = server.get("/agents.md").send().await.text;
    let joined = server.tool("join", json!({})).await;
    let moves = joined["moves_per_day"].as_i64().unwrap();
    assert_eq!(moves, i64::from(worlds::GUEST_MOVES));
    for (name, page) in [("home", &home), ("llms.txt", &llms), ("agents.md", &guide)] {
        assert!(
            page.contains(&format!("{moves} moves")),
            "{name} does not say {moves} moves a day"
        );
        assert!(!page.contains("{{BASE_URL}}"), "{name} still has a placeholder");
    }
    assert!(guide.contains("200 moves a day") && llms.len() > 100);
    // The tool descriptions say the same.
    let tools = server.rpc("tools/list", json!({})).await.body["result"]["tools"].to_string();
    assert!(tools.contains("30 moves a day") && tools.contains("200 moves a day"));
    // A person who is signed in gets what the table in the design says.
    let ann = server.person("Ann").await;
    assert_eq!(server.me(&ann.key, COMMONS).await["moves_per_day"], 300);
    // And what the front page says about the sign-up it asks for is true: no account to watch or to send an agent.
    assert_eq!(server.get("/api/worlds/commons").send().await.status, StatusCode::OK);
    assert_eq!(server.post("/api/public/join", json!({})).send().await.status, StatusCode::OK);
}

// W9, W11, W30
#[tokio::test]
async fn measure_how_many_moves_a_day_one_person_can_have_by_making_bots() {
    let server = Server::start().await;
    let first = server.bot("Bot 0").await;
    let mut tokens = vec![server.bot_player(&first).await];
    for n in 1..30 {
        let bot =
            server.bot_in(&first.key, &first.person, &first.hangout, &format!("Bot {n}")).await;
        tokens.push(server.bot_player(&bot).await);
    }
    let mut total = 0;
    for token in &tokens {
        total += server.me(token, COMMONS).await["moves_per_day"].as_i64().unwrap();
    }
    let own = server.me(&first.key, COMMONS).await["moves_per_day"].as_i64().unwrap();
    println!("COST REPORT: one person made 30 bots in one room with no limit on bots, and has {total} moves a day through them, besides the {own} their own player has; the cap on claims (20) does not apply to bots");
    assert_eq!(total, 30 * 200);
}

// W1, W3, W4, W18
#[tokio::test]
async fn the_keeper_ending_a_bots_player_in_the_commons_changes_nothing_in_private_life_and_tells_nobody_whose_it_was(
) {
    let path = temp_db("keeper");
    let db = Db::open(&path).unwrap();
    let keeper_key = auth::new_owner_key();
    db.create_person(
        "keeper0000000001",
        "Keeper",
        &auth::hash_secret(&keeper_key),
        &auth::new_feed_token(),
        now(),
    )
    .unwrap();
    let server = Server::boot(db, None, Some("keeper0000000001".to_string()), None).await;
    let made = server
        .post("/api/cities", json!({ "name": "Friends", "your_name": "Alice" }))
        .send()
        .await
        .ok();
    let (alice_key, alice_id, city) = (
        made["owner_key"].as_str().unwrap().to_string(),
        made["person"]["id"].as_str().unwrap().to_string(),
        made["city"]["id"].as_str().unwrap().to_string(),
    );
    let house = server
        .post(&format!("/api/cities/{city}/houses"), json!({ "name": "Alice House" }))
        .token(&alice_key)
        .send()
        .await
        .ok();
    let living = house["living_room_id"].as_str().unwrap().to_string();
    let pixel = server.bot_in(&alice_key, &alice_id, &living, "Pixel").await;
    let token = server.bot_player(&pixel).await;
    let player_id = server.player_id(&token).await;
    let land = land_of(&server, COMMONS);
    let tiles = server.tiles(COMMONS);
    // A tile of road, beside the highway, which is what an agent can build.
    let spot = tiles
        .keys()
        .flat_map(|&(x, y)| {
            let (x, y) = (i64::from(x), i64::from(y));
            [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]
        })
        .find(|&(x, y)| open_ground(&land, &tiles, x, y))
        .unwrap();
    server.acts_ok(&token, COMMONS, road_move(spot, spot)).await;

    let before = dump(&path);
    let removed = server
        .post(&format!("/api/worlds/commons/players/{player_id}/remove"), json!({}))
        .token(&keeper_key)
        .send()
        .await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.show());
    for secret in [
        alice_id.as_str(),
        pixel.agent.as_str(),
        city.as_str(),
        pixel.token.as_str(),
        alice_key.as_str(),
        "Pixel",
        "Alice",
        "Friends",
    ] {
        assert!(!removed.text.contains(secret), "the answer shows {secret}: {}", removed.text);
    }
    let after = dump(&path);
    let world_tables =
        ["worlds", "world_tiles", "players", "world_players", "world_members", "crawler_sightings"];
    for (table, rows) in &after {
        if !world_tables.contains(&table.as_str()) {
            assert_eq!(rows, &before[table], "ending a player changed {table}");
        }
    }
    // The bot is out of the public worlds for good, and is still the same bot at home.
    assert_eq!(server.act(&token, COMMONS, wave()).await.status, StatusCode::UNAUTHORIZED);
    let again = server
        .post("/api/public/join", json!({}))
        .token(&pixel.token)
        .ip("198.51.100.6")
        .send()
        .await;
    assert_eq!(again.status, StatusCode::FORBIDDEN, "{}", again.show());
    for secret in [alice_id.as_str(), pixel.agent.as_str(), city.as_str()] {
        assert!(!again.text.contains(secret), "{}", again.text);
    }
    assert_eq!(server.get("/api/inbox").token(&pixel.token).send().await.status, StatusCode::OK);
    assert_eq!(server.get("/api/me").token(&pixel.token).send().await.status, StatusCode::OK);
    assert_eq!(
        server.get(&format!("/api/cities/{city}")).token(&pixel.token).send().await.status,
        StatusCode::OK,
        "removal from the Commons took the bot out of its own city"
    );
    // The tiles it built came down, and the keeper's call says how many and nothing else.
    assert!(removed.body["tiles_removed"].as_i64().unwrap() >= 1);
    drop(server);
    remove_db(&path);
}

// W29
#[tokio::test]
async fn enormous_headers_are_answered_or_refused_and_leave_nothing_behind() {
    let server = Server::start().await;
    let long_agent = format!("Mozilla/5.0 (compatible; GPTBot/1.2) {}", "A".repeat(60_000));
    let many_commas = format!("{}203.0.113.7", "1.1.1.1, ".repeat(7_000));
    let mut answered = 0;
    for (method, path) in [
        (Method::GET, "/"),
        (Method::GET, "/api/worlds/commons"),
        (Method::POST, "/public/mcp"),
        (Method::GET, "/world/commons"),
    ] {
        let mut request = server.req(method.clone(), path).ua(&long_agent).ip(&many_commas);
        if path == "/public/mcp" {
            request = request.json(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }));
        }
        match request.try_send().await {
            Ok(reply) => {
                assert!(!reply.status.is_server_error(), "{method} {path}: {}", reply.status);
                answered += 1;
            }
            Err(error) => {
                println!("COST REPORT: {method} {path}: the connection was closed: {error}")
            }
        }
    }
    println!("COST REPORT: {answered} of 4 requests with a 60 KB user agent and a 60 KB address list were answered");
    // Nothing of it was kept: the only thing a crawler leaves is its kind.
    assert!(server.state.worlds.frogs(now()).len() <= 1);
    // And the server is still itself.
    assert_eq!(server.get("/api/worlds/commons").send().await.status, StatusCode::OK);
}

// ----------------------------------------------------------------------------------------------
// The last round of fixes before the city opens: refused moves, limits, headers, a score and a goal
// ----------------------------------------------------------------------------------------------

/// How long a header says to wait, in seconds.
#[track_caller]
fn retry_after(reply: &Reply) -> u64 {
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS, "{}", reply.show());
    reply
        .headers
        .get("retry-after")
        .expect("a 429 says when to come back")
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

// W30, W13
#[tokio::test]
async fn a_bulldoze_with_no_builder_or_not_the_players_is_refused_without_reading_the_world() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let guest = server.guest().await;
    let guest_id = server.player_id(&guest.token).await;
    let road = make_a_dependant_road(&server, guest_id);
    crowd_the_commons(&server, 8_000);
    let land = land_of(&server, COMMONS);
    let highway = world::highway(world::COMMONS_SEED, &land);
    let (hx, hy) = highway[highway.len() / 2];
    // A place that somebody else built, so it is not up for the taking.
    let theirs = server
        .tiles(COMMONS)
        .iter()
        .find(|(_, t)| t.kind == Kind::House)
        .map(|(pos, _)| *pos)
        .unwrap();
    let row = server.player_row(guest_id);
    // Each try is a minute later than the last, so the limit on refused moves never gets in the way.
    let mut at = now();
    let mut time = |mv: Move, tries: i64| {
        let started = Instant::now();
        for _ in 0..tries {
            at += 61;
            assert_eq!(
                worlds::act(&server.state, &row, COMMONS, &mv, at).unwrap_err().status,
                StatusCode::BAD_REQUEST
            );
        }
        started.elapsed().as_secs_f64() * 1000.0 / tries as f64
    };
    let no_builder = time(mv("bulldoze", Some(i64::from(hx)), Some(i64::from(hy))), 60);
    let not_yours = time(mv("bulldoze", Some(i64::from(theirs.0)), Some(i64::from(theirs.1))), 60);
    let depended_on = time(mv("bulldoze", Some(road.0), Some(road.1)), 20);
    println!("COST REPORT: a refused bulldoze costs {no_builder:.2} ms with no builder, {not_yours:.2} ms when it is someone else's, and {depended_on:.2} ms for a road a place depends on");
    assert!(
        depended_on > 8.0 * no_builder.max(not_yours),
        "a bulldoze that could never be allowed took {no_builder:.2} and {not_yours:.2} ms, as long as one that needed the whole network ({depended_on:.2} ms)"
    );
}

// W30, W13
#[tokio::test]
async fn refused_moves_are_limited_per_address_as_well_as_per_player_and_a_429_says_when_to_come_back(
) {
    start_early_in_a_minute().await;
    let server = Server::start().await;
    let (one, two, three, elsewhere) =
        (server.guest().await, server.guest().await, server.guest().await, server.guest().await);
    let nonsense = json!({ "action": "bulldoze", "x": 9999, "y": 9999 });
    // Two players behind one address, 40 refusals each: the address runs out, and neither player has.
    let (mut refused, mut limited) = (0, 0);
    for n in 0..80 {
        let who = if n % 2 == 0 { &one } else { &two };
        let reply = server
            .post("/api/worlds/commons/act", nonsense.clone())
            .token(&who.token)
            .ip("198.51.100.77")
            .send()
            .await;
        match reply.status {
            StatusCode::BAD_REQUEST => refused += 1,
            StatusCode::TOO_MANY_REQUESTS => {
                limited += 1;
                assert!((1..=60).contains(&retry_after(&reply)));
            }
            other => panic!("{other}: {}", reply.show()),
        }
    }
    println!("COST REPORT: 80 refused moves from two players at one address in a minute: {refused} answered, {limited} turned away");
    assert_eq!((refused, limited), (60, 20));
    // From that address even a good move is turned away at the door, and nothing was charged.
    let good = server
        .post("/api/worlds/commons/act", wave())
        .token(&one.token)
        .ip("198.51.100.77")
        .send()
        .await;
    assert_eq!(good.status, StatusCode::TOO_MANY_REQUESTS, "{}", good.show());
    assert_eq!(server.moves_left(&one.token).await, 30);
    // Another address is not held up, and nor is the same player from another address: 40 is below their own 60.
    let fine = server
        .post("/api/worlds/commons/act", wave())
        .token(&elsewhere.token)
        .ip("198.51.100.78")
        .send()
        .await;
    assert_eq!(fine.status, StatusCode::OK, "{}", fine.show());
    let moved = server
        .post("/api/worlds/commons/act", wave())
        .token(&one.token)
        .ip("198.51.100.79")
        .send()
        .await;
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.show());
    // One player's own 60 are counted wherever they come from.
    for n in 0..60 {
        let reply = server
            .post("/api/worlds/commons/act", nonsense.clone())
            .token(&three.token)
            .ip(&format!("203.0.113.{}", 100 + n))
            .send()
            .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{n}: {}", reply.show());
    }
    let after = server
        .post("/api/worlds/commons/act", wave())
        .token(&three.token)
        .ip("203.0.113.250")
        .send()
        .await;
    assert!((1..=60).contains(&retry_after(&after)), "{}", after.show());
    // With no address header on plain http, the player's count is all there is.
    let alone = server
        .post("/api/worlds/commons/act", nonsense.clone())
        .token(&elsewhere.token)
        .send()
        .await;
    assert_eq!(alone.status, StatusCode::BAD_REQUEST);
    // The tools are held to the same, and say so as a refusal the model can read.
    let said = server.call("emote", json!({ "player_token": three.token, "emote": "wave" })).await;
    assert_eq!(said["isError"], true);
    assert!(said["content"][0]["text"].as_str().unwrap().contains("refused"), "{said}");
    assert!(said["content"][0]["text"].as_str().unwrap().contains("Seconds to wait: "), "{said}");
}

// The discovery files say a browser on another site may read them, so it has to be able to ask.
#[tokio::test]
async fn a_browser_on_another_site_can_ask_first_and_nothing_promises_a_missing_header() {
    let server = Server::start().await;
    for path in ["/.well-known/ai-catalog.json", "/public/mcp/server-card"] {
        let asked = server
            .req(Method::OPTIONS, path)
            .header("origin", "https://example.com")
            .header("access-control-request-method", "GET")
            .header("access-control-request-headers", "if-none-match")
            .send()
            .await;
        assert_eq!(asked.status, StatusCode::NO_CONTENT, "{path}: {}", asked.show());
        let said = |name: &str| {
            asked.headers.get(name).map(|value| value.to_str().unwrap().to_lowercase())
        };
        assert_eq!(said("access-control-allow-origin").as_deref(), Some("*"), "{path}");
        assert_eq!(said("access-control-allow-methods").as_deref(), Some("get"), "{path}");
        assert!(said("access-control-allow-headers").unwrap().contains("if-none-match"), "{path}");
        let read = server.get(path).send().await;
        assert!(
            read.headers.get("access-control-expose-headers").is_none(),
            "{path}: no ETag is sent"
        );
    }
}

// A wrong address gets a way home, and everything a bot reads keeps its empty 404.
#[tokio::test]
async fn a_wrong_address_gets_a_short_page_and_the_api_keeps_its_empty_404() {
    let server = Server::start().await;
    for path in ["/definitely-not-a-page", "/worlds/", "/world/commons/"] {
        let page = server.get(path).send().await;
        assert_eq!(page.status, StatusCode::NOT_FOUND, "{path}: {}", page.show());
        assert!(page.text.contains("href=\"/\""), "{path}: {}", page.text);
    }
    for path in ["/api", "/api/", "/api/nothing"] {
        let refused = server.get(path).send().await;
        assert_eq!(refused.status, StatusCode::NOT_FOUND, "{path}");
        assert!(refused.text.is_empty(), "{path}: {}", refused.text);
    }
    let posted = server.post("/definitely-not-a-page", json!({})).send().await;
    assert_eq!(posted.status, StatusCode::NOT_FOUND);
    assert!(posted.text.is_empty(), "{}", posted.text);
}

// W30
#[tokio::test]
async fn every_429_says_how_many_seconds_to_wait_and_the_wait_fits_the_limit() {
    // The limits count by the minute, the hour and the day on the clock, so none of them may end mid-test.
    start_early_in_a_minute().await;
    for period in [3600, 86_400] {
        let left = period - now().rem_euclid(period);
        if left < 20 {
            tokio::time::sleep(Duration::from_secs(left as u64 + 1)).await;
        }
    }
    let server = Server::start().await;
    // Views: the 241st in a minute waits for the end of the minute.
    let mut last = server.get("/api/worlds/commons").ip("203.0.113.40").send().await;
    for _ in 0..240 {
        last = server.get("/api/worlds/commons").ip("203.0.113.40").send().await;
    }
    assert!((1..=60).contains(&retry_after(&last)), "{}", last.show());
    // Guests: the sixth from one address in an hour waits for the end of the hour.
    for _ in 0..5 {
        server.post("/api/public/join", json!({})).ip("203.0.113.41").send().await.ok();
    }
    let sixth = server.post("/api/public/join", json!({})).ip("203.0.113.41").send().await;
    let wait = retry_after(&sixth);
    let left = 3600 - now().rem_euclid(3600);
    assert!(
        wait >= 1 && wait.abs_diff(left as u64) <= 5,
        "{wait} s, with {left} s of the hour left"
    );
    // Moves: when a day's are spent, they come back at midnight UTC.
    let guest = server.guest().await;
    for _ in 0..30 {
        server.acts_ok(&guest.token, COMMONS, wave()).await;
    }
    let spent = server.act(&guest.token, COMMONS, wave()).await;
    let wait = retry_after(&spent);
    let left = 86_400 - now().rem_euclid(86_400);
    assert!(
        wait >= 1 && wait.abs_diff(left as u64) <= 5,
        "{wait} s, with {left} s of the day left"
    );
    // Anything else that says 429 says something, and nothing else says it.
    assert!(server
        .get("/api/worlds/commons")
        .ip("203.0.113.42")
        .send()
        .await
        .headers
        .get("retry-after")
        .is_none());
}

// W51
#[tokio::test]
async fn every_answer_says_what_the_browser_may_not_use_and_an_https_server_says_to_keep_to_https()
{
    let live = Server::start_as_live().await;
    let plain = Server::start().await;
    let guest = plain.guest().await;
    for path in [
        "/",
        "/world/commons",
        "/api/worlds/commons",
        "/nothing/here",
        "/agents.md",
        "/public/mcp",
        "/style.css",
    ] {
        for (server, https) in [(&live, true), (&plain, false)] {
            let reply = server.get(path).ip("203.0.113.9").send().await;
            let policy = reply
                .headers
                .get("permissions-policy")
                .unwrap_or_else(|| panic!("{path}: no Permissions-Policy"))
                .to_str()
                .unwrap();
            for want in [
                "camera=()",
                "microphone=()",
                "geolocation=()",
                "payment=()",
                "publickey-credentials-get=(self)",
                "publickey-credentials-create=(self)",
            ] {
                assert!(policy.contains(want), "{path}: {policy}");
            }
            let hsts = reply.headers.get("strict-transport-security").map(|v| v.to_str().unwrap());
            assert_eq!(hsts, https.then_some("max-age=86400"), "{path}");
        }
    }
    // A refusal carries them too.
    let big = plain
        .post("/api/worlds/commons/act", json!({ "action": "emote", "emote": "x".repeat(200_000) }))
        .token(&guest.token)
        .send()
        .await;
    assert_eq!(big.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(big.headers.get("permissions-policy").is_some());
}

// W3
#[test]
fn the_index_that_keeps_one_player_for_each_person_is_remade_to_leave_out_removed_players_and_keeps_every_row(
) {
    let path = temp_db("removedindex");
    drop(Db::open(&path).unwrap());
    {
        // The database as the live server has it: the index as it was made before this fix, and
        // the older index on a player's tiles.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP INDEX players_one_own_per_person;
             CREATE UNIQUE INDEX players_one_own_per_person ON players (person_id)
                 WHERE person_id IS NOT NULL AND token_hash IS NULL AND agent_id IS NULL;
             DROP INDEX world_tiles_by_world_player;
             CREATE INDEX world_tiles_by_player ON world_tiles (player_id);",
        )
        .unwrap();
        conn.execute("INSERT INTO people VALUES ('p1', 'A', '#fff', 'k1', 'f1', 0)", []).unwrap();
        conn.execute("INSERT INTO players (name_seed, person_id, seen_at, created_at) VALUES (5, 'p1', 0, 0)", []).unwrap();
    }
    let before = dump(&path);
    drop(Db::open(&path).expect("the new code could not open the database as it is"));
    let schema = schema_of(&path);
    let index = schema.iter().find(|s| s.starts_with("index players_one_own_per_person")).unwrap();
    assert!(index.contains("removed = 0"), "the index was not remade: {index}");
    assert!(schema.iter().any(|s| s.contains("world_tiles_by_world_player")));
    assert!(
        !schema.iter().any(|s| s.contains("world_tiles_by_player")),
        "the index on the player alone is still there"
    );
    // Opening it again, as every start does, changes nothing: schema or rows.
    for _ in 0..3 {
        drop(Db::open(&path).unwrap());
    }
    assert_eq!(schema_of(&path), schema);
    assert_eq!(dump(&path)["players"], before["players"]);
    // A player that was removed no longer stands in the way of another for the same person, and
    // two that are not removed still can't both be theirs.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("UPDATE players SET removed = 1", []).unwrap();
    let add = |seed: i64| {
        conn.execute("INSERT INTO players (name_seed, person_id, seen_at, created_at) VALUES (?, 'p1', 0, 0)", [seed])
    };
    assert!(add(6).is_ok());
    assert!(add(7).is_err(), "a person got two players of their own");
    drop(conn);
    remove_db(&path);
}

// W3
#[tokio::test]
async fn a_deleted_bots_player_lets_go_of_the_person_so_it_is_never_taken_for_the_persons_own() {
    let path = temp_db("exbot");
    let server = Server::on(Db::open(&path).unwrap()).await;
    let bot = server.bot("Looper").await;
    // The bot plays first and has its token replaced, so its player has no token and no person's
    // own player has been made yet. Then the bot is deleted.
    let _token = server.bot_player(&bot).await;
    let rotated = server
        .post(&format!("/api/agents/{}/rotate", bot.agent), json!({}))
        .token(&bot.key)
        .send()
        .await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.show());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.busy_timeout(Duration::from_secs(5)).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    conn.execute("DELETE FROM agents WHERE id = ?", [&bot.agent]).unwrap();
    let left: (i64, Option<String>) = conn
        .query_row("SELECT removed, person_id FROM players", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(left, (1, None), "the bot's player is ended and belongs to nobody");
    drop(conn);
    // The person has never played as themselves, and is not told they were removed.
    let moved = server.act(&bot.key, COMMONS, wave()).await;
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.show());
    assert_eq!(server.me(&bot.key, COMMONS).await["moves_per_day"], 300);
    drop(server);
    remove_db(&path);
}

/// Puts tiles in The Commons for a builder, far from anything, as a test's way of making a town.
fn lay(server: &Server, builder: i64, row: u16, built: &[(Kind, u8)]) {
    let changes: Vec<Change> = built
        .iter()
        .enumerate()
        .map(|(n, (kind, level))| {
            Change::Set((n as u16, row), tile(*kind, *level, Some(builder as u64)))
        })
        .collect();
    put_tiles(server, COMMONS, &changes);
}

// W47, W48, W49, W62
#[tokio::test]
async fn me_gives_your_town_a_goal_and_a_tip_worked_out_from_your_own_tiles_and_nobody_elses() {
    let server = Server::start().await;
    let (ann, bob) = (server.guest().await, server.guest().await);
    let (ann_id, bob_id) = (server.player_id(&ann.token).await, server.player_id(&bob.token).await);
    let sorted_keys = |value: &Value| {
        let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    };

    // An agent starts with nothing, and its ladder is roads. A guest can't place anything, so no `places`.
    let fresh = server.me(&ann.token, COMMONS).await;
    assert_eq!(
        sorted_keys(&fresh),
        ["goal", "moves_left", "moves_per_day", "player", "tip", "your_town"]
    );
    assert_eq!(fresh["your_town"], json!({ "roads": 0, "places": 0 }));
    assert_eq!(fresh["goal"], world::GOALS[4]);
    assert!(fresh["tip"].as_str().unwrap().starts_with("No road yet"), "{fresh}");

    // Ann has a road, and tiles of the old city that count for nothing. Bob has twelve places.
    lay(
        &server,
        ann_id,
        110,
        &[
            (Kind::Road, 0),
            (Kind::Homes, 2),
            (Kind::Shops, 0),
            (Kind::Workshops, 1),
            (Kind::Park, 0),
        ],
    );
    lay(
        &server,
        bob_id,
        112,
        &[
            (Kind::House, 0),
            (Kind::Cafe, 0),
            (Kind::Cafe, 0),
            (Kind::Cafe, 0),
            (Kind::Club, 0),
            (Kind::Club, 0),
            (Kind::Studio, 0),
            (Kind::Studio, 0),
            (Kind::Studio, 0),
            (Kind::Studio, 0),
            (Kind::Studio, 0),
            (Kind::Studio, 0),
        ],
    );
    let mine = server.me(&ann.token, COMMONS).await;
    let wanted = json!({ "roads": 1, "places": 0 });
    assert_eq!(mine["your_town"], wanted, "{mine}");
    assert_eq!(mine["goal"], world::GOALS[5], "one road and the next rung is 10 tiles");
    assert_eq!(mine["tip"], world::TIPS[4], "{mine}");
    let his = server.me(&bob.token, COMMONS).await;
    assert_eq!(his["your_town"], json!({ "roads": 0, "places": 12 }));
    assert_eq!(
        his["goal"],
        world::GOALS[4],
        "a guest's ladder is roads, whatever stands on its tiles"
    );
    // The tool says the same, and a little more.
    let how = server.tool("how_am_i_doing", json!({ "player_token": ann.token })).await;
    for key in ["player", "moves_left", "moves_per_day", "your_town", "goal", "tip"] {
        assert_eq!(how[key], mine[key], "{key}");
    }
    assert_eq!(
        how["built"],
        json!({ "homes": 1, "park": 1, "road": 1, "shops": 1, "workshops": 1 })
    );
    assert_eq!(how["kind"], "guest");
    // The same maths as the world's, over her tiles: that is the whole of it.
    let hers: Tiles = server
        .tiles(COMMONS)
        .into_iter()
        .filter(|(_, t)| t.player == Some(ann_id as u64))
        .collect();
    assert_eq!(serde_json::to_value(world::town(&hers)).unwrap(), wanted);
    // Everything in it is a number, a listed sentence, or her generated name.
    let numbers_only = mine["your_town"].as_object().unwrap().values().all(|v| v.is_u64());
    assert!(
        numbers_only
            && world::GOALS.contains(&mine["goal"].as_str().unwrap())
            && world::TIPS.contains(&mine["tip"].as_str().unwrap())
    );

    // A person's ladder is places, and they are told when each comes (W62).
    let dee = server.person("Dee").await;
    let dee_id = person_player(&server, &dee).id;
    let start = server.me(&dee.key, COMMONS).await;
    assert_eq!(
        sorted_keys(&start),
        ["full", "goal", "moves_left", "moves_per_day", "places", "player", "tip", "your_town"]
    );
    assert_eq!(start["goal"], world::GOALS[0]);
    assert_eq!(start["tip"], world::TIPS[1], "{start}");
    // W90: the newer kinds of place are listed with the first four.
    assert_eq!(
        sorted_keys(&start["places"]),
        [
            "bar", "cafe", "club", "green", "house", "library", "lounge", "office", "studio",
            "workshop"
        ]
    );
    lay(&server, dee_id, 114, &[(Kind::House, 0)]);
    let one = server.me(&dee.key, COMMONS).await;
    assert_eq!(
        (&one["goal"], &one["tip"]),
        (&json!(world::GOALS[1]), &json!(world::TIPS[2])),
        "{one}"
    );
    lay(&server, dee_id, 116, &[(Kind::Studio, 0)]);
    assert_eq!(server.me(&dee.key, COMMONS).await["goal"], world::GOALS[2]);
    lay(&server, dee_id, 118, &[(Kind::Club, 0), (Kind::Cafe, 0)]);
    let all = server.me(&dee.key, COMMONS).await;
    assert_eq!(all["goal"], world::GOALS[3]);
    assert_eq!(all["your_town"], json!({ "roads": 0, "places": 4 }), "{all}");
}

// W30, W48
#[tokio::test]
async fn me_is_limited_like_a_view_and_a_player_who_is_not_one_is_turned_away_first() {
    start_early_in_a_minute().await;
    let server = Server::start().await;
    let guest = server.guest().await;
    let mut told = 0;
    for _ in 0..300 {
        let reply = server
            .get("/api/worlds/commons/me")
            .token(&guest.token)
            .ip("198.51.100.55")
            .send()
            .await;
        match reply.status {
            StatusCode::OK => told += 1,
            StatusCode::TOO_MANY_REQUESTS => {
                assert!(
                    reply.body["error"].as_str().unwrap().contains("too often"),
                    "{}",
                    reply.show()
                );
                assert!(retry_after(&reply) <= 60);
            }
            other => panic!("{other}"),
        }
    }
    assert!((235..=245).contains(&told), "{told} answers: a view's 240 a minute");
    // Another address is fine, and a stranger is still a 401, whoever they are.
    assert_eq!(
        server
            .get("/api/worlds/commons/me")
            .token(&guest.token)
            .ip("198.51.100.56")
            .send()
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        server
            .get("/api/worlds/commons/me")
            .token(&made_up_token())
            .ip("198.51.100.57")
            .send()
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server.get("/api/worlds/commons/me").ip("198.51.100.57").send().await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .get("/api/worlds/nowhere/me")
            .token(&guest.token)
            .ip("198.51.100.57")
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

// W31, W30, W15, W50, W54
#[tokio::test]
async fn look_over_http_is_the_tool_with_the_same_limits_and_it_lists_the_tiles_kept_for_side_streets(
) {
    start_early_in_a_minute().await;
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let guest = server.guest().await;
    let land = land_of(&server, COMMONS);
    let tiles = server.tiles(COMMONS);

    // The end of the highway, in a window as big as it is allowed to be.
    let http = server
        .get("/api/worlds/commons/look?x=64&y=64&w=40&h=40")
        .ip("198.51.100.60")
        .send()
        .await
        .ok();
    assert_eq!((&http["window"]["w"], &http["window"]["h"]), (&json!(32), &json!(32)));
    assert_eq!(http["land"].as_array().unwrap().len(), 32);
    let tool = server.tool("look", json!({ "x": 64, "y": 64, "w": 40, "h": 40 })).await;
    assert_eq!(http, tool, "the tool and the route say the same");
    let (x0, y0) = (http["window"]["x"].as_i64().unwrap(), http["window"]["y"].as_i64().unwrap());
    let listed: HashSet<(i64, i64)> = http["reserved"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p[0].as_i64().unwrap(), p[1].as_i64().unwrap()))
        .collect();
    assert!(listed.len() >= 6, "the highway keeps hardly any side streets: {}", listed.len());
    // Every tile it lists is open ground in the window that a place would be refused on, and it lists them all.
    let mut every = HashSet::new();
    for y in y0..y0 + 32 {
        for x in x0..x0 + 32 {
            if open_ground(&land, &tiles, x, y) && world::is_reserved(&tiles, (x as u16, y as u16))
            {
                every.insert((x, y));
            }
        }
    }
    assert_eq!(listed, every);
    // And a place on one is refused and costs nothing, which is what the list is for. Only a person
    // can ask, so a guest is told so first.
    let &(rx, ry) = listed.iter().next().unwrap();
    let house = json!({ "action": "place", "kind": "house", "x": rx, "y": ry });
    assert_eq!(
        server.act(&guest.token, COMMONS, house.clone()).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(server.moves_left(&guest.token).await, 30);
    let dee = server.person("Dee").await;
    let place = server.act(&dee.key, COMMONS, house).await;
    assert_eq!(place.status, StatusCode::BAD_REQUEST, "{}", place.show());
    assert!(place.body["error"].as_str().unwrap().contains("side street"));
    assert_eq!(server.me(&dee.key, COMMONS).await["moves_left"], 300);

    // A player token says where you are, and a made-up one is turned away. Nobody needs either.
    let you = server
        .get("/api/worlds/commons/look?w=4&h=4")
        .token(&guest.token)
        .ip("198.51.100.60")
        .send()
        .await
        .ok();
    assert_eq!(you["you"]["moves_left"], 30);
    assert!(http["you"].is_null());
    let bad_token = server
        .get("/api/worlds/commons/look")
        .token(&made_up_token())
        .ip("198.51.100.60")
        .send()
        .await;
    assert_eq!(bad_token.status, StatusCode::UNAUTHORIZED);
    // A signed-in person's cookie is not a player token: looking makes no player of them.
    let ann = server.person("Ann").await;
    let with_cookie = server
        .get("/api/worlds/commons/look")
        .cookie(&ann.cookie)
        .ip("198.51.100.60")
        .send()
        .await
        .ok();
    assert!(with_cookie["you"].is_null());
    assert!(server.state.db.players_of(&ann.id).unwrap().is_empty());
    // The sizes are the tool's: never more than 32, never less than 1.
    let tiny = server
        .get("/api/worlds/commons/look?x=5&y=5&w=0&h=-9")
        .ip("198.51.100.60")
        .send()
        .await
        .ok();
    assert_eq!((&tiny["window"]["w"], &tiny["window"]["h"]), (&json!(1), &json!(1)));
    // Anything that is not a window is one fixed answer that repeats nothing that was sent.
    for (query, status) in [
        ("?x=5", 400),
        ("?y=5", 400),
        ("?x=zzmarker&y=1", 400),
        ("?x=&y=", 400),
        ("?w=1.5", 400),
        ("?zzmarker=1", 400),
        ("?x=1&y=2&z=3", 400),
        ("?x=99999&y=1", 400),
        ("?x=-1&y=-1", 400),
        ("?x=9223372036854775808&y=1", 400),
    ] {
        let reply = server
            .get(&format!("/api/worlds/commons/look{query}"))
            .ip("198.51.100.60")
            .send()
            .await;
        assert_eq!(reply.status.as_u16(), status, "{query}: {}", reply.show());
        assert!(
            !reply.text.contains("zzmarker")
                && !reply.text.contains("99999")
                && !reply.text.contains("9223372036854775808"),
            "{query}: {}",
            reply.text
        );
    }
    assert_eq!(
        server.get("/api/worlds/nowhere/look").ip("198.51.100.60").send().await.status,
        StatusCode::NOT_FOUND
    );
    for method in [Method::POST, Method::PUT, Method::DELETE] {
        let reply = server
            .req(method.clone(), "/api/worlds/commons/look")
            .ip("198.51.100.60")
            .json(json!({}))
            .send()
            .await;
        assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED, "{method}");
    }
    // It is limited like a view: 240 a minute for an address, all of the above counted.
    let mut served = 0;
    for _ in 0..300 {
        served += usize::from(
            server.get("/api/worlds/commons/look").ip("198.51.100.61").send().await.status
                == StatusCode::OK,
        );
    }
    assert!((235..=245).contains(&served), "{served} windows in a minute");
}

// W45, W47, W48, W49
#[tokio::test]
async fn the_guides_explain_the_land_the_score_the_goal_side_streets_visitors_and_midnight() {
    let server = Server::start().await;
    let guide = server.get("/agents.md").send().await.text;
    let llms = server.get("/llms.txt").send().await.text;
    let commons = &guide[..guide.find("## Private rooms").unwrap()];
    for needle in [
        "`12g3f`",
        "run-length",
        "rows from y=0",
        "`your_town`",
        "`goal`",
        "ladder",
        "`reserved`",
        "multiple of 6",
        "Visitors",
        "frogs",
        "midnight UTC",
        "don't carry over",
        "from y 24",
        "/look?x=",
        "Retry-After",
    ] {
        assert!(commons.contains(needle), "the guide never says: {needle}");
    }
    for needle in ["`your_town`", "`goal`", "`12g3f`", "`reserved`", "midnight UTC", "frogs"] {
        assert!(llms.contains(needle), "llms.txt never says: {needle}");
    }
    // The highway does not run down x 46 from the top edge, so the guide doesn't say it does.
    assert!(!commons.contains("runs down x 46 and then"), "the highway claim is back");
    assert!(commons.lines().count() <= 120, "{} lines", commons.lines().count());
    // The tools say what they give.
    let tools = server.rpc("tools/list", json!({})).await.body["result"]["tools"].to_string();
    assert!(tools.contains("your_town") && tools.contains("goal") && tools.contains("reserved"));
    // The page says it too, as text and never as markup.
    let page = read_web("app.js");
    assert!(page.contains("your_town") && page.contains("Goal:") && page.contains("/me`"));
}

// W16, W45
#[tokio::test]
async fn the_guides_send_a_new_agent_to_look_and_tell_a_bot_to_file_a_bug() {
    let server = Server::start().await;
    let guide = server.get("/agents.md").send().await.text;
    let llms = server.get("/llms.txt").send().await.text;
    let commons = &guide[..guide.find("## Private rooms").unwrap()];
    // First moves sends an agent to streets, and doesn't read as a road to copy.
    let first = commons.split("### First moves").nth(1).unwrap().split("### ").next().unwrap();
    assert!(first.contains("`streets`"), "First moves never mentions streets: {first}");
    assert!(
        first.contains("use the ends you read from `streets`"),
        "First moves still reads as a road to copy: {first}"
    );
    // A line with nothing new on it is refused, in the guide and in the tool.
    assert!(
        commons.contains("nothing new"),
        "the guide never says a line with nothing new is refused"
    );
    let tools = server.rpc("tools/list", json!({})).await.body["result"]["tools"].clone();
    let describe = |name: &str| {
        tools
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .map(|tool| tool["description"].as_str().unwrap().to_string())
            .unwrap_or_default()
    };
    assert!(describe("build").contains("nothing new"), "{}", describe("build"));
    assert!(describe("look").contains("streets"), "{}", describe("look"));
    // Nobody reads an issue before it goes up, so both guides say what stays out of it. Anything
    // that isn't a bug still waits for the owner.
    for (name, text) in [("agents.md", &guide), ("llms.txt", &llms)] {
        for needle in [
            "file an issue",
            "tell your owner you did",
            "never goes in an issue",
            "An issue is public",
            "your tokens, anything about",
            "your owner and anything from a private room",
            "Don't open a pull request, or an issue that isn't a bug, unless they ask you to",
        ] {
            assert!(text.contains(needle), "{name} never says: {needle}");
        }
        let saw = text.contains("ile only what you saw yourself");
        assert!(saw, "{name} lets a bot file what it was told to");
    }
}
