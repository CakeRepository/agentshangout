//! Attack tests for the public worlds (docs/public-city.md, W1 to W70). Round one of the security
//! loop: every test here tries to get around a rule. A comment such as `// W13` above a test names
//! the rule it attacks. A test that fails is a bug in the server, not in the test.
//!
//! Time is real for anything that goes over HTTP. Tests that need a time of their own call the
//! functions in `worlds` directly.

#![allow(dead_code, unused_imports)]

use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use agenthangout::{
    app, auth,
    db::{Db, NewWorld, WorldAdded},
    now,
    world::{self, Change, Kind, Tile},
    worlds::{self, Move},
    AppState, Config,
};
use axum::http::{HeaderMap, Method, StatusCode};
use reqwest::Client;
use serde_json::{json, Value};

const COMMONS: &str = "commons";

/// Gives every frozen world a name and a link of its own.
static FROZEN: AtomicU32 = AtomicU32::new(0);

/// The limits count by the minute, the hour and the day on the clock, so a test that counts what one
/// of them lets through starts early in it, with plenty of it left.
async fn start_early_in(period: i64, room: i64) {
    let left = period - agenthangout::now().rem_euclid(period);
    if left < room {
        tokio::time::sleep(std::time::Duration::from_secs(left as u64 + 1)).await;
    }
}
const CHROME: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/126.0 Safari/537.36";
const GOOGLEBOT: &str = "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";
const GPTBOT: &str = "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; GPTBot/1.2";
const CHATGPT_USER: &str = "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; ChatGPT-User/1.0";
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
        self.try_send().await.unwrap()
    }

    /// Like `send`, but a connection that breaks (which is what a handler that panics does) comes
    /// back as an error instead of ending the test.
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
        Self::boot(Db::open(":memory:").unwrap(), None, None).await
    }

    /// A server with a create code set, as a deployment might have.
    async fn start_with_code(code: &str) -> Self {
        Self::boot(Db::open(":memory:").unwrap(), Some(code.to_string()), None).await
    }

    /// A server whose keeper is a person made first, so their id can be set in the config.
    async fn start_with_keeper() -> (Self, Who) {
        let db = Db::open(":memory:").unwrap();
        let key = auth::new_owner_key();
        db.create_person("keeper0000000001", "Keeper", &auth::hash_secret(&key), &auth::new_feed_token(), now())
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

    /// A person made on a server that asks for a create code.
    async fn person_with_code(&self, name: &str, code: &str) -> Who {
        self.person_with_body(json!({ "name": "Crew", "your_name": name, "create_code": code })).await
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

    /// A world someone started before worlds were retired. It's frozen (W27): it can be watched and
    /// nothing else. Returns its id and the code on its link.
    async fn frozen_world(&self, owner: &Who, open: bool) -> (String, String) {
        let n = FROZEN.fetch_add(1, Ordering::Relaxed);
        let (id, code) = (format!("frozen-{n}"), format!("frozen-code-{n}"));
        let seed = 99 + u64::from(n);
        let land = world::terrain(seed, world::OWN_SIZE);
        let highway = world::highway(seed, &land);
        let new = NewWorld {
            id: &id,
            name: "Quiet Bay",
            kind: "own",
            seed,
            size: world::OWN_SIZE,
            owner_id: Some(&owner.id),
            open,
            invite_code: Some(&code),
            highway: &highway,
            now: now(),
        };
        let added = self.state.db.insert_world(&new, worlds::MAX_OWN_WORLDS).unwrap();
        assert!(matches!(added, WorldAdded::Added), "a person can have only one world, frozen or not");
        (id, code)
    }

    /// A player of The Commons put into a frozen world with two roads of their own, the way they were
    /// left when worlds stopped. Returns the two ends of the road.
    fn leave_in(&self, world: &str, player_id: i64) -> ((i64, i64), (i64, i64)) {
        let (first, last) = Map::from_db(self, world).line_from_road(2).unwrap();
        let roads: Vec<Change> = [first, last]
            .iter()
            .map(|&(x, y)| Change::Set((x as u16, y as u16), Tile { kind: Kind::Road, level: 0, player: Some(player_id as u64) }))
            .collect();
        self.state
            .db
            .in_transaction::<(), rusqlite::Error>(|tx| {
                tx.enter_world(world, player_id, (32, 32), now())?;
                tx.write_changes(world, &roads, now())
            })
            .unwrap();
        (first, last)
    }

    /// A `place` move by a signed-in person, as themself.
    async fn place(&self, who: &Who, kind: &str, (x, y): (i64, i64)) -> Reply {
        self.act(&who.key, COMMONS, place_move(kind, (x, y))).await
    }

    /// Tiles written straight into a world, the way an older server left them.
    fn write(&self, world: &str, changes: &[Change]) {
        self.state.db.in_transaction::<(), rusqlite::Error>(|tx| tx.write_changes(world, changes, now())).unwrap();
    }

    /// A signed-in person with a house of their own, put up on `spot`.
    async fn housed(&self, name: &str, spot: (i64, i64)) -> Who {
        let who = self.person(name).await;
        self.place(&who, "house", spot).await.ok();
        who
    }

    // The public MCP endpoint

    async fn rpc(&self, method: &str, params: Value) -> Reply {
        self.post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
            .send()
            .await
    }

    async fn call(&self, name: &str, arguments: Value) -> Value {
        self.rpc("tools/call", json!({ "name": name, "arguments": arguments })).await.body["result"].clone()
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
    /// Tiles kept for side streets (W15): a place can't go on them, and a road may use them.
    kept: HashSet<(i64, i64)>,
    /// Tiles a long road may go on (W76, W77), worked out as the server does.
    on_streets: HashSet<(i64, i64)>,
}

/// The tiles a road longer than a lane may go on (W77), worked out as the server does.
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
            .map(|(&(x, y), tile)| ((i64::from(x), i64::from(y)), (tile.kind.as_str().to_string(), i64::from(tile.level))))
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

    /// Open land that a place can take: not a side street.
    fn free(&self, x: i64, y: i64) -> bool {
        self.open(x, y) && !self.kept.contains(&(x, y))
    }

    fn roads(&self) -> Vec<(i64, i64)> {
        let mut roads: Vec<_> = self.built.iter().filter(|(_, (kind, _))| kind == "road").map(|(pos, _)| *pos).collect();
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
                // W77: longer than a lane, it follows the streets.
                let along = |i: i64| n <= world::LANE_TILES as i64 || self.on_streets.contains(&(rx + dx * i, ry + dy * i));
                if (1..=n).all(|i| self.open(rx + dx * i, ry + dy * i) && along(i)) {
                    return Some(((rx + dx, ry + dy), (rx + dx * n, ry + dy * n)));
                }
            }
        }
        None
    }

    /// Open tiles a place can go on: beside a road and not kept for a side street, in a steady order.
    fn house_spots(&self) -> Vec<(i64, i64)> {
        let mut spots = Vec::new();
        for (rx, ry) in self.roads() {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let at = (rx + dx, ry + dy);
                if self.free(at.0, at.1) && !spots.contains(&at) {
                    spots.push(at);
                }
            }
        }
        spots
    }

    /// A tile of this ground with a road beside it and nothing built on it, so that the ground is the
    /// only thing that can stop a place going there.
    fn find_beside_road(&self, ground: char) -> Option<(i64, i64)> {
        let roads: HashSet<(i64, i64)> = self.roads().into_iter().collect();
        (0..self.size)
            .flat_map(|y| (0..self.size).map(move |x| (x, y)))
            .filter(|&(x, y)| self.ground(x, y) == Some(ground) && !self.built.contains_key(&(x, y)))
            .find(|&(x, y)| [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| roads.contains(&(x + dx, y + dy))))
    }
}

fn road_move(first: (i64, i64), last: (i64, i64)) -> Value {
    json!({ "action": "road", "x": first.0, "y": first.1, "x2": last.0, "y2": last.1 })
}

fn place_move(kind: &str, (x, y): (i64, i64)) -> Value {
    json!({ "action": "place", "kind": kind, "x": x, "y": y })
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
    Move { action: "emote".to_string(), x: None, y: None, x2: None, y2: None, kind: None, emote: Some(word.to_string()) }
}

// ----------------------------------------------------------------------------------------------
// Tools for the attacks
// ----------------------------------------------------------------------------------------------

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;

/// Counts the bytes this thread has allocated and not yet freed, so a test that calls the server's
/// functions directly can tell whether a flood of calls left anything behind. Per thread, so tests
/// running at the same time can't confuse one another.
struct Counting;

thread_local! {
    static NET: Cell<isize> = const { Cell::new(0) };
}

fn add_net(delta: isize) {
    let _ = NET.try_with(|net| net.set(net.get() + delta));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            add_net(layout.size() as isize);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            add_net(layout.size() as isize);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        add_net(-(layout.size() as isize));
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = System.realloc(ptr, layout, new_size);
        if !moved.is_null() {
            add_net(new_size as isize - layout.size() as isize);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `work` and says how many bytes it left allocated on this thread.
fn retained(work: impl FnOnce()) -> isize {
    let before = NET.with(Cell::get);
    work();
    NET.with(Cell::get) - before
}

/// A bot made in a hangout of its own: its owner's key and id, and the bot's token and id.
#[derive(Clone)]
struct Bot {
    key: String,
    person: String,
    hangout: String,
    token: String,
    agent: String,
}

impl Server {
    async fn bot(&self, name: &str) -> Bot {
        let room = self.post("/api/hangouts", json!({ "name": "Crew", "your_name": "Owner" })).send().await.ok();
        let key = room["owner_key"].as_str().unwrap().to_string();
        let hangout = room["hangout"]["id"].as_str().unwrap().to_string();
        self.bot_in(&key, room["person"]["id"].as_str().unwrap(), &hangout, name).await
    }

    async fn bot_in(&self, key: &str, person: &str, hangout: &str, name: &str) -> Bot {
        let bot = self.post(&format!("/api/hangouts/{hangout}/agents"), json!({ "name": name })).token(key).send().await.ok();
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

    /// A city, a house in it, the house's living room, and its owner's key and id.
    async fn city_house(&self, name: &str) -> CityHouse {
        let made = self.post("/api/cities", json!({ "name": name, "your_name": "Ann" })).send().await.ok();
        let key = made["owner_key"].as_str().unwrap().to_string();
        let person = made["person"]["id"].as_str().unwrap().to_string();
        let city = made["city"]["id"].as_str().unwrap().to_string();
        let house = self.post(&format!("/api/cities/{city}/houses"), json!({ "name": "Ann House" })).token(&key).send().await.ok();
        CityHouse {
            key,
            person,
            city,
            house: house["house"]["id"].as_str().unwrap().to_string(),
            living: house["living_room_id"].as_str().unwrap().to_string(),
        }
    }

    /// A bot that stays home in its house, and the token of that house's bot.
    async fn home_bots(&self, place: &CityHouse) -> (Bot, String) {
        let bot = self.bot_in(&place.key, &place.person, &place.living, "Homebody").await;
        self.stay_home(&bot, &place.house, true).await;
        let casa = self
            .post(&format!("/api/houses/{}/house-bot", place.house), json!({ "name": "Casa" }))
            .token(&place.key)
            .send()
            .await
            .ok();
        (bot, casa["token"].as_str().unwrap().to_string())
    }

    async fn stay_home(&self, bot: &Bot, house: &str, stays: bool) {
        let reply = self
            .req(Method::PUT, &format!("/api/agents/{}/home", bot.agent))
            .json(json!({ "house_id": house, "stays_home": stays }))
            .token(&bot.key)
            .send()
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.show());
    }

    async fn acts_ok(&self, token: &str, world: &str, body: Value) -> Value {
        self.act(token, world, body).await.ok()
    }
}

struct CityHouse {
    key: String,
    person: String,
    city: String,
    house: String,
    living: String,
}

/// Every table's rows as text, read from a second connection to a database file, so a test can
/// say exactly what a series of requests changed.
fn dump(path: &str) -> BTreeMap<String, Vec<String>> {
    let conn = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
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

/// The tables the worlds own. Everything else is private life.
const WORLD_TABLES: [&str; 9] = [
    "worlds",
    "world_tiles",
    "world_buildings",
    "players",
    "world_players",
    "world_members",
    "crawler_sightings",
    "place_clocks",
    "world_ends",
];

/// What changed between two dumps, in the tables that match `keep`.
fn changed_tables(
    before: &BTreeMap<String, Vec<String>>,
    after: &BTreeMap<String, Vec<String>>,
    keep: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut out = Vec::new();
    for (table, rows) in after {
        if !keep(table) {
            continue;
        }
        let was = before.get(table).cloned().unwrap_or_default();
        if &was != rows {
            let gone: Vec<&String> = was.iter().filter(|row| !rows.contains(row)).collect();
            let new: Vec<&String> = rows.iter().filter(|row| !was.contains(row)).collect();
            out.push(format!("{table}: removed {gone:?}, now {new:?}"));
        }
    }
    out
}

fn temp_db(label: &str) -> String {
    let path = std::env::temp_dir().join(format!("attack-{label}-{}-{}.db", std::process::id(), now()));
    path.to_str().unwrap().to_string()
}

fn remove_db(path: &str) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path}{suffix}"));
    }
}

/// A made-up credential that has the same shape as a guest token.
fn made_up() -> String {
    format!("ah_guest_{}", "x".repeat(40))
}

// ----------------------------------------------------------------------------------------------
// The wall between public and private (W1-W5)
// ----------------------------------------------------------------------------------------------

/// Every route that isn't a world's, with made-up ids where they belong in the path.
fn private_routes() -> Vec<(Method, &'static str)> {
    let g = Method::GET;
    let p = Method::POST;
    let u = Method::PUT;
    let d = Method::DELETE;
    vec![
        (g.clone(), "/api/me"),
        (p.clone(), "/api/session"),
        (d.clone(), "/api/session"),
        (p.clone(), "/api/key/rotate"),
        (p.clone(), "/api/feed/rotate"),
        (p.clone(), "/api/status"),
        (p.clone(), "/api/activity"),
        (p.clone(), "/api/doorbell"),
        (g.clone(), "/api/inbox"),
        (u.clone(), "/api/notify"),
        (p.clone(), "/api/notify/test"),
        (p.clone(), "/api/hangouts"),
        (g.clone(), "/api/hangouts/abcdefghijklmnop"),
        (g.clone(), "/api/hangouts/abcdefghijklmnop/stream"),
        (g.clone(), "/api/hangouts/abcdefghijklmnop/messages"),
        (p.clone(), "/api/hangouts/abcdefghijklmnop/messages"),
        (p.clone(), "/api/hangouts/abcdefghijklmnop/notes"),
        (p.clone(), "/api/hangouts/abcdefghijklmnop/events"),
        (p.clone(), "/api/hangouts/abcdefghijklmnop/check-ins"),
        (p.clone(), "/api/check-ins/abcdefghijklmnop/answer"),
        (p.clone(), "/api/hangouts/abcdefghijklmnop/owner-questions"),
        (p.clone(), "/api/owner-questions/abcdefghijklmnop/answer"),
        (p.clone(), "/api/hangouts/abcdefghijklmnop/agents"),
        (u.clone(), "/api/hangouts/abcdefghijklmnop/agents/abcdefghijklmnop"),
        (d.clone(), "/api/hangouts/abcdefghijklmnop/agents/abcdefghijklmnop"),
        (p.clone(), "/api/hangouts/abcdefghijklmnop/invite/rotate"),
        (p.clone(), "/api/hangouts/abcdefghijklmnop/leave"),
        (g.clone(), "/api/invites/abcdefghijklmnop"),
        (p.clone(), "/api/invites/abcdefghijklmnop/join"),
        (p.clone(), "/api/agents/abcdefghijklmnop/rotate"),
        (u.clone(), "/api/agents/abcdefghijklmnop/notify"),
        (u.clone(), "/api/agents/abcdefghijklmnop/doorbell"),
        (p.clone(), "/api/events/abcdefghijklmnop/rsvp"),
        (p.clone(), "/api/events/abcdefghijklmnop/cancel"),
        (g.clone(), "/api/events/abcdefghijklmnop/ics"),
        (g.clone(), "/cal/abcdefghijklmnop.ics"),
        (g.clone(), "/api/passkeys"),
        (g.clone(), "/api/passkeys/available"),
        (p.clone(), "/api/passkeys/register/start"),
        (p.clone(), "/api/passkeys/register/finish"),
        (p.clone(), "/api/passkeys/login/start"),
        (p.clone(), "/api/passkeys/login/finish"),
        (d.clone(), "/api/passkeys/abcdefghijklmnop"),
        (p.clone(), "/api/cities"),
        (g.clone(), "/api/cities/abcdefghijklmnop"),
        (g.clone(), "/api/cities/abcdefghijklmnop/stream"),
        (p.clone(), "/api/cities/abcdefghijklmnop/invite/rotate"),
        (p.clone(), "/api/cities/abcdefghijklmnop/leave"),
        (p.clone(), "/api/cities/abcdefghijklmnop/houses"),
        (u.clone(), "/api/cities/abcdefghijklmnop/rules"),
        (g.clone(), "/api/cities/abcdefghijklmnop/people/abcdefghijklmnop/standing"),
        (p.clone(), "/api/cities/abcdefghijklmnop/people/abcdefghijklmnop/vouch"),
        (d.clone(), "/api/cities/abcdefghijklmnop/people/abcdefghijklmnop/vouch"),
        (p.clone(), "/api/cities/abcdefghijklmnop/gate/abcdefghijklmnop/decline"),
        (p.clone(), "/api/cities/abcdefghijklmnop/gate/clear"),
        (g.clone(), "/api/city-invites/abcdefghijklmnop"),
        (p.clone(), "/api/city-invites/abcdefghijklmnop/join"),
        (g.clone(), "/api/houses/abcdefghijklmnop"),
        (p.clone(), "/api/houses/abcdefghijklmnop/invite/rotate"),
        (p.clone(), "/api/houses/abcdefghijklmnop/leave"),
        (u.clone(), "/api/houses/abcdefghijklmnop/rules"),
        (d.clone(), "/api/houses/abcdefghijklmnop/residents/abcdefghijklmnop"),
        (p.clone(), "/api/houses/abcdefghijklmnop/house-bot"),
        (p.clone(), "/api/houses/abcdefghijklmnop/passes"),
        (g.clone(), "/api/houses/abcdefghijklmnop/scenes"),
        (p.clone(), "/api/houses/abcdefghijklmnop/scenes"),
        (g.clone(), "/api/house-invites/abcdefghijklmnop"),
        (p.clone(), "/api/house-invites/abcdefghijklmnop/join"),
        (u.clone(), "/api/agents/abcdefghijklmnop/home"),
        (p.clone(), "/api/trips"),
        (g.clone(), "/api/trips/abcdefghijklmnop"),
        (p.clone(), "/api/trips/abcdefghijklmnop/answer"),
        (p.clone(), "/api/trips/abcdefghijklmnop/end"),
        (p.clone(), "/api/trips/abcdefghijklmnop/extend"),
        (p.clone(), "/api/passes/abcdefghijklmnop/accept"),
        (d.clone(), "/api/passes/abcdefghijklmnop"),
        (d.clone(), "/api/scenes/abcdefghijklmnop"),
        (p.clone(), "/api/scenes/abcdefghijklmnop/request"),
        (p.clone(), "/api/scene-requests/abcdefghijklmnop/answer"),
        (g.clone(), "/api/house/queue"),
        (p.clone(), "/api/house/queue/abcdefghijklmnop/finish"),
        (p.clone(), "/api/house/status"),
        (p.clone(), "/mcp"),
    ]
}

/// How a credential is presented.
#[derive(Clone)]
enum Cred {
    Bearer(String),
    Cookie(String),
    QueryToken(String),
    Header(&'static str, String),
}

/// One request: the reply as status and text, or a note that it was left hanging (a stream).
async fn probe(server: &Server, method: &Method, path: &str, cred: &Cred) -> Result<(StatusCode, String), String> {
    let mut path = path.to_string();
    if let Cred::QueryToken(token) = cred {
        path = format!("{path}?token={token}&key={token}&owner_key={token}&access_token={token}");
    }
    let mut request = server.req(method.clone(), &path);
    if *method != Method::GET && *method != Method::DELETE {
        request = request.json(json!({}));
    }
    match cred {
        Cred::Bearer(token) => request = request.token(token),
        Cred::Cookie(token) => request = request.cookie(&format!("ah_session={token}")),
        Cred::QueryToken(_) => {}
        Cred::Header(name, token) => request = request.header(name, token),
    }
    match tokio::time::timeout(Duration::from_secs(5), request.try_send()).await {
        Ok(Ok(reply)) => Ok((reply.status, reply.text)),
        Ok(Err(error)) => Err(format!("{error}")),
        Err(_) => Err("left hanging".to_string()),
    }
}

/// The same credential with a made-up value.
fn made_up_like(cred: &Cred) -> Cred {
    match cred {
        Cred::Bearer(_) => Cred::Bearer(made_up()),
        Cred::Cookie(_) => Cred::Cookie(made_up()),
        Cred::QueryToken(_) => Cred::QueryToken(made_up()),
        Cred::Header(name, _) => Cred::Header(name, made_up()),
    }
}

// W2
#[tokio::test]
async fn every_private_route_treats_a_guest_a_claim_code_and_a_claimed_token_as_a_made_up_credential() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    let claimed = server.guest().await;
    server.post("/api/players/claim", json!({ "claim_code": claimed.claim })).token(&ann.key).send().await.ok();
    let bot = server.bot("Scout").await;
    let bot_player = server.bot_player(&bot).await;
    // A guest that acted, so its token is as live as a token gets.
    server.acts_ok(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;

    let credentials: Vec<(&str, Cred)> = vec![
        ("a guest token", Cred::Bearer(guest.token.clone())),
        ("a claim code", Cred::Bearer(guest.claim.clone())),
        ("a claimed guest's token", Cred::Bearer(claimed.token.clone())),
        ("a bot's player token", Cred::Bearer(bot_player.clone())),
        ("a guest token as a session cookie", Cred::Cookie(guest.token.clone())),
        ("a claimed token as a session cookie", Cred::Cookie(claimed.token.clone())),
        ("a bot player token as a session cookie", Cred::Cookie(bot_player.clone())),
        ("a guest token in the query string", Cred::QueryToken(guest.token.clone())),
        ("a guest token in x-owner-key", Cred::Header("x-owner-key", guest.token.clone())),
        ("a guest token in x-api-key", Cred::Header("x-api-key", guest.token.clone())),
        ("a guest token in x-agent-token", Cred::Header("x-agent-token", guest.token.clone())),
    ];
    let mut wrong = Vec::new();
    let mut broken = Vec::new();
    for (method, path) in private_routes() {
        for (name, cred) in &credentials {
            let reference = match probe(&server, &method, path, &made_up_like(cred)).await {
                Ok(reply) => reply,
                Err(error) => {
                    broken.push(format!("{method} {path} with a made-up credential: {error}"));
                    continue;
                }
            };
            match probe(&server, &method, path, cred).await {
                Ok(reply) if reply == reference => {}
                Ok(reply) => wrong.push(format!(
                    "{method} {path} with {name}: {} {} (a made-up one gave {} {})",
                    reply.0, reply.1, reference.0, reference.1
                )),
                Err(error) => broken.push(format!("{method} {path} with {name}: {error}")),
            }
        }
    }
    broken.dedup();
    assert!(broken.is_empty(), "left hanging or broken: {broken:#?}");
    assert!(wrong.is_empty(), "a world credential was treated differently from a made-up one:\n{}", wrong.join("\n"));
}

// W2
#[tokio::test]
async fn a_guest_token_and_a_claim_code_reach_no_person_only_world_route() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    let claimed = server.guest().await;
    server.post("/api/players/claim", json!({ "claim_code": claimed.claim })).token(&ann.key).send().await.ok();
    let id = server.player_id(&claimed.token).await;
    let (world, _) = server.frozen_world(&ann, true).await;
    for token in [&guest.token, &guest.claim, &claimed.token, &claimed.claim] {
        for (method, path, body) in [
            (Method::GET, "/api/worlds/mine".to_string(), json!({})),
            (Method::GET, "/api/players/mine".to_string(), json!({})),
            (Method::POST, "/api/players/claim".to_string(), json!({ "claim_code": guest.claim })),
            (Method::POST, format!("/api/players/{id}/reroll"), json!({})),
            (Method::POST, format!("/api/worlds/{world}/players/{id}/remove"), json!({})),
        ] {
            let reply = server.req(method.clone(), &path).json(body).token(token).send().await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{method} {path}: {}", reply.show());
        }
    }
    // The claimed guest is still its person's: nothing above changed that.
    assert_eq!(server.get("/api/players/mine").token(&ann.key).send().await.ok()["players"].as_array().unwrap().len(), 1);
    assert!(server.state.db.world_of_owner(&ann.id).unwrap().is_some());
}

// W2, W3, W5
#[tokio::test]
async fn a_bot_token_is_never_a_player_token_in_any_tool_or_header_and_nothing_is_made() {
    let server = Server::start().await;
    let bot = server.bot("Scout").await;
    let seen_before = server.state.db.agent(&bot.agent, now()).unwrap().unwrap().last_seen_at;
    for (name, args) in [
        ("how_am_i_doing", json!({ "player_token": bot.token })),
        ("emote", json!({ "player_token": bot.token, "emote": "wave" })),
        ("walk", json!({ "player_token": bot.token, "x": 65, "y": 64 })),
        ("bulldoze", json!({ "player_token": bot.token, "x": 65, "y": 64 })),
        ("build", json!({ "player_token": bot.token, "type": "road", "x": 65, "y": 64 })),
        ("look", json!({ "player_token": bot.token })),
    ] {
        let as_argument = server.rpc("tools/call", json!({ "name": name, "arguments": args })).await;
        assert_eq!(as_argument.body["result"]["isError"], true, "{name} with the token as an argument: {}", as_argument.text);
        let mut bare = args.clone();
        bare.as_object_mut().unwrap().remove("player_token");
        let as_header = server
            .post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": bare } }))
            .token(&bot.token)
            .send()
            .await;
        if name == "look" {
            // Looking is free and needs no player, so a header changes nothing.
            continue;
        }
        assert_eq!(as_header.body["result"]["isError"], true, "{name} with the token as a header: {}", as_header.text);
    }
    assert_eq!(server.act(&bot.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get("/api/worlds/commons/me").token(&bot.token).send().await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.post("/api/worlds/join/anything", json!({})).token(&bot.token).send().await.status, StatusCode::UNAUTHORIZED);
    assert!(server.state.db.players_of(&bot.person).unwrap().is_empty(), "nothing was made for the bot");
    assert_eq!(server.view(COMMONS).await["players"], json!([]));
    // W5: none of it counted as the bot being seen.
    assert_eq!(server.state.db.agent(&bot.agent, now()).unwrap().unwrap().last_seen_at, seen_before);
}

/// Everything a visitor, a crawler, a guest, a bot or a person can do in the worlds, done once.
/// Returns the tokens it made, so a caller can carry on from them.
async fn play_every_world_route(server: &Server, ann: &Who, bot: &Bot) {
    // Guests, a claim, a reroll, and a person who plays as themself.
    let one = server.guest().await;
    let two = server.guest().await;
    server.post("/api/players/claim", json!({ "claim_code": one.claim })).token(&ann.key).send().await.ok();
    let one_id = server.player_id(&one.token).await;
    server.post(&format!("/api/players/{one_id}/reroll"), json!({})).token(&ann.key).send().await.ok();
    server.get("/api/players/mine").token(&ann.key).send().await.ok();
    let map = Map::from_db(server, COMMONS);
    let (first, last) = map.line_from_road(3).unwrap();
    server.acts_ok(&ann.key, COMMONS, road_move(first, last)).await;
    // A person's house, and what a person is told about their places.
    let spot = Map::from_db(server, COMMONS).house_spots()[0];
    server.acts_ok(&ann.key, COMMONS, place_move("house", spot)).await;
    server.me(&ann.key, COMMONS).await;
    server.acts_ok(&ann.key, COMMONS, json!({ "action": "emote", "emote": "dance" })).await;
    server.acts_ok(&one.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    server.acts_ok(&two.token, COMMONS, json!({ "action": "walk", "x": 65, "y": 64 })).await;
    // A bot that may play.
    let player = server.bot_player(bot).await;
    server.acts_ok(&player, COMMONS, json!({ "action": "emote", "emote": "cheer" })).await;
    server.me(&player, COMMONS).await;
    // A frozen world: a guest at its link is turned away, a move there is refused, and the owner
    // can still take a player out of it.
    let (world, code) = server.frozen_world(ann, false).await;
    let two_id = server.player_id(&two.token).await;
    server.leave_in(&world, two_id);
    server.post(&format!("/api/worlds/join/{code}"), json!({})).token(&two.token).send().await;
    server.act(&two.token, &world, json!({ "action": "emote", "emote": "wave" })).await;
    server.post("/api/worlds", json!({})).token(&ann.key).send().await;
    server.post(&format!("/api/worlds/{world}/players/{two_id}/remove"), json!({})).token(&ann.key).send().await.ok();
    // Looking, listing and the pages that crawlers and agents fetch.
    server.fresh_view(COMMONS).await;
    server.get("/api/worlds").send().await.ok();
    server.tool("look", json!({ "player_token": one.token })).await;
    server.tool("how_am_i_doing", json!({ "player_token": one.token })).await;
    server.get("/").ua(GOOGLEBOT).send().await;
    server.get("/").ua(CHATGPT_USER).send().await;
    server.get("/llms.txt").ua("curl/8.5").send().await;
    server.post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })).ua("curl/8.5").send().await;
    // The world steps, and the housekeeping that goes with it.
    for _ in 0..5 {
        worlds::step_all(&server.state, now());
    }
    worlds::housekeeping(&server.state, now() + 120);
    worlds::housekeeping(&server.state, now() + 4000);
}

// W1, W5
#[tokio::test]
async fn nothing_a_world_does_changes_a_row_of_private_life() {
    let path = temp_db("private-rows");
    let server = Server::boot(Db::open(&path).unwrap(), None, None).await;
    // A person with a city, a house, the bots that stay home and run it, a hangout and a bot.
    let place = server.city_house("Elm").await;
    let (stay, casa) = server.home_bots(&place).await;
    let scout = server.bot_in(&place.key, &place.person, &place.living, "Scout").await;
    let ann = Who { key: place.key.clone(), id: place.person.clone(), cookie: String::new() };
    server.post(&format!("/api/hangouts/{}/messages", place.living), json!({ "text": "hello" })).token(&place.key).send().await;
    server.post("/api/status", json!({ "status": "busy", "text": "heads down" })).token(&place.key).send().await;
    // A rest, so anything that changes a timestamp changes it to something else.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    let before = dump(&path);
    play_every_world_route(&server, &ann, &scout).await;
    // The bots that may not play try to, and are turned away.
    for token in [&stay.token, &casa] {
        assert_eq!(server.post("/api/public/join", json!({})).token(token).send().await.status, StatusCode::FORBIDDEN);
    }
    let after = dump(&path);
    let changed = changed_tables(&before, &after, |table| !WORLD_TABLES.contains(&table));
    remove_db(&path);
    assert!(changed.is_empty(), "the worlds changed private rows:\n{}", changed.join("\n"));
}

// W1, W5
#[tokio::test]
async fn nothing_in_private_life_changes_a_row_of_a_world() {
    let path = temp_db("world-rows");
    let server = Server::boot(Db::open(&path).unwrap(), None, None).await;
    let place = server.city_house("Elm").await;
    let (_stay, _casa) = server.home_bots(&place).await;
    let scout = server.bot_in(&place.key, &place.person, &place.living, "Scout").await;
    let ann = Who { key: place.key.clone(), id: place.person.clone(), cookie: String::new() };
    play_every_world_route(&server, &ann, &scout).await;
    server.view(COMMONS).await;
    let before = dump(&path);

    // Private life, all of it: talk, plans, status, a second house, a trip, a vouch, a standing.
    server.post(&format!("/api/hangouts/{}/messages", place.living), json!({ "text": "hello" })).token(&place.key).send().await.ok();
    server.post(&format!("/api/hangouts/{}/notes", place.living), json!({ "text": "a fact" })).token(&place.key).send().await;
    server.post(&format!("/api/hangouts/{}/events", place.living), json!({ "title": "Dinner", "starts_at": now() + 7200, "ends_at": now() + 10800 })).token(&place.key).send().await;
    server.post("/api/status", json!({ "status": "busy", "text": "heads down" })).token(&place.key).send().await;
    server.get(&format!("/api/cities/{}/people/{}/standing", place.city, place.person)).token(&place.key).send().await.ok();
    server.get("/api/me").token(&place.key).send().await.ok();
    server.get("/api/inbox").token(&scout.token).send().await;
    server.post("/api/activity", json!({ "kind": "computer", "via": "x" })).token(&scout.token).send().await;
    server.post(&format!("/api/hangouts/{}/messages", place.living), json!({ "text": "from the bot" })).token(&scout.token).send().await;
    server.post(&format!("/api/hangouts/{}/leave", place.living), json!({})).token(&place.key).send().await;
    // The key is replaced, a session starts and ends.
    server.post("/api/key/rotate", json!({})).token(&place.key).send().await;
    let after = dump(&path);
    let changed = changed_tables(&before, &after, |table| WORLD_TABLES.contains(&table));
    remove_db(&path);
    assert!(changed.is_empty(), "private life changed a world:\n{}", changed.join("\n"));
}

// W5
#[tokio::test]
async fn a_bot_that_asks_a_person_only_world_route_does_not_look_present_in_its_hangout() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bot = server.bot("Scout").await;
    let guest = server.guest().await;
    server.post("/api/players/claim", json!({ "claim_code": guest.claim })).token(&ann.key).send().await.ok();
    let (world, _) = server.frozen_world(&ann, true).await;
    let id = server.player_id(&guest.token).await;
    let routes: Vec<(Method, String, Value)> = vec![
        (Method::GET, "/api/players/mine".to_string(), json!({})),
        (Method::GET, "/api/worlds/mine".to_string(), json!({})),
        (Method::POST, "/api/worlds".to_string(), json!({})),
        (Method::POST, "/api/players/claim".to_string(), json!({ "claim_code": "nope" })),
        (Method::POST, format!("/api/players/{id}/reroll"), json!({})),
        (Method::POST, format!("/api/worlds/{world}/players/{id}/remove"), json!({})),
    ];
    let mut moved = Vec::new();
    for (method, path, body) in routes {
        // An old sighting, so a new one is plain to see.
        server.state.db.touch_agent_seen(&bot.agent, 1000).unwrap();
        server.req(method.clone(), &path).json(body).token(&bot.token).send().await;
        let seen = server.state.db.agent(&bot.agent, now()).unwrap().unwrap().last_seen_at;
        if seen != Some(1000) {
            moved.push(format!("{method} {path}"));
        }
    }
    assert!(moved.is_empty(), "world routes that made a bot look present in its hangout: {moved:?}");
}

// W3
#[tokio::test]
async fn a_bot_that_is_set_to_stay_home_after_it_joined_can_no_longer_play() {
    let server = Server::start().await;
    let place = server.city_house("Elm").await;
    let scout = server.bot_in(&place.key, &place.person, &place.living, "Scout").await;
    let token = server.bot_player(&scout).await;
    server.acts_ok(&token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    server.stay_home(&scout, &place.house, true).await;
    // It can't join again, and the player token it already holds must not keep working either.
    assert_eq!(server.post("/api/public/join", json!({})).token(&scout.token).send().await.status, StatusCode::FORBIDDEN);
    let reply = server.act(&token, COMMONS, json!({ "action": "emote", "emote": "cheer" })).await;
    assert!(reply.status.is_client_error(), "a bot that stays home just played: {}", reply.show());
    let tool = server.call("emote", json!({ "player_token": token, "emote": "think" })).await;
    assert_eq!(tool["isError"], true, "a bot that stays home just played through MCP: {tool}");
    let me = server.get("/api/worlds/commons/me").token(&token).send().await;
    assert!(me.status.is_client_error(), "{}", me.show());
}

// W3
#[tokio::test]
async fn the_house_bot_and_a_bot_that_stays_home_cannot_join_by_any_path_and_nothing_is_made() {
    let server = Server::start().await;
    let place = server.city_house("Elm").await;
    let (stay, casa) = server.home_bots(&place).await;
    let players_before = server.state.db.players_of(&place.person).unwrap().len();
    for token in [&stay.token, &casa] {
        // REST, MCP with the token as a header, and MCP for a world other than The Commons.
        assert_eq!(server.post("/api/public/join", json!({})).token(token).send().await.status, StatusCode::FORBIDDEN);
        assert_eq!(server.post("/api/public/join", json!({ "world_id": COMMONS })).token(token).send().await.status, StatusCode::FORBIDDEN);
        let via_mcp = server
            .post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "join", "arguments": {} } }))
            .token(token)
            .send()
            .await;
        assert_eq!(via_mcp.body["result"]["isError"], true, "{}", via_mcp.text);
        assert!(!via_mcp.text.contains("ah_guest_"));
        // Nor does either work as a player token anywhere, as a header, a cookie, or an argument.
        assert_eq!(server.act(token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.status, StatusCode::UNAUTHORIZED);
        assert_eq!(server.call("emote", json!({ "player_token": token, "emote": "wave" })).await["isError"], true);
        let cookie = server.req(Method::POST, "/api/worlds/commons/act").json(json!({ "action": "emote", "emote": "wave" })).cookie(&format!("ah_session={token}")).send().await;
        assert_eq!(cookie.status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(server.state.db.players_of(&place.person).unwrap().len(), players_before);
    assert_eq!(server.view(COMMONS).await["players"], json!([]));
    let cards = server.get("/api/worlds").send().await.ok();
    assert_eq!(cards["worlds"][0]["players_active"], 0);
    // Letting the bot out of the house is the owner's call, and then it may play.
    server.stay_home(&stay, &place.house, false).await;
    assert_eq!(server.post("/api/public/join", json!({})).token(&stay.token).send().await.status, StatusCode::OK);
}

// W3
#[tokio::test]
async fn a_rotated_bot_key_or_a_deleted_bot_takes_its_public_player_with_it() {
    let server = Server::start().await;
    let bot = server.bot("Scout").await;
    let token = server.bot_player(&bot).await;
    assert_eq!(server.get("/api/worlds/commons/me").token(&token).send().await.status, StatusCode::OK);
    // The owner replaces the bot's key because it has leaked.
    let rotated = server.post(&format!("/api/agents/{}/rotate", bot.agent), json!({})).token(&bot.key).send().await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.show());
    assert_eq!(server.get("/api/me").token(&bot.token).send().await.status, StatusCode::UNAUTHORIZED, "the old key stopped");
    let still = server.get("/api/worlds/commons/me").token(&token).send().await;
    assert_eq!(still.status, StatusCode::UNAUTHORIZED, "the leaked key's player is still playing: {}", still.show());

    // And the owner takes the bot out of the hangout altogether.
    let other = server.bot("Other").await;
    let other_token = server.bot_player(&other).await;
    let gone = server.req(Method::DELETE, &format!("/api/hangouts/{}/agents/{}", other.hangout, other.agent)).token(&other.key).send().await;
    assert!(gone.status.is_success(), "{}", gone.show());
    let still = server.get("/api/worlds/commons/me").token(&other_token).send().await;
    assert_eq!(still.status, StatusCode::UNAUTHORIZED, "a deleted bot is still playing: {}", still.show());
}

// W4
#[tokio::test]
async fn nothing_a_stranger_can_read_names_a_person_a_bot_a_room_a_city_or_a_house() {
    let server = Server::start().await;
    let made = server.post("/api/cities", json!({ "name": "ZqCityMark", "your_name": "ZqPersonMark" })).send().await.ok();
    let key = made["owner_key"].as_str().unwrap().to_string();
    let person = made["person"]["id"].as_str().unwrap().to_string();
    let city = made["city"]["id"].as_str().unwrap().to_string();
    let house = server.post(&format!("/api/cities/{city}/houses"), json!({ "name": "ZqHouseMark" })).token(&key).send().await.ok();
    let living = house["living_room_id"].as_str().unwrap().to_string();
    let house_id = house["house"]["id"].as_str().unwrap().to_string();
    let bot = server.post(&format!("/api/hangouts/{living}/agents"), json!({ "name": "ZqBotMark" })).token(&key).send().await.ok();
    let bot_token = bot["token"].as_str().unwrap().to_string();
    let agent = bot["agent"]["id"].as_str().unwrap().to_string();
    let me = server.get("/api/me").token(&key).send().await.ok();
    let feed = me.to_string();

    let joined = server.post("/api/public/join", json!({})).token(&bot_token).send().await.ok();
    let token = joined["player_token"].as_str().unwrap().to_string();
    let guest = server.guest().await;
    server.post("/api/players/claim", json!({ "claim_code": guest.claim })).token(&key).send().await.ok();
    server.acts_ok(&token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    server.acts_ok(&guest.token, COMMONS, json!({ "action": "emote", "emote": "cheer" })).await;
    server.acts_ok(&key, COMMONS, json!({ "action": "emote", "emote": "dance" })).await;
    let owner = Who { key: key.clone(), id: person.clone(), cookie: String::new() };
    let (own_id, invite) = server.frozen_world(&owner, false).await;

    let mut secrets: Vec<String> = ["ZqCityMark", "ZqPersonMark", "ZqHouseMark", "ZqBotMark", "Crew"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    secrets.extend([person.clone(), agent.clone(), city.clone(), house_id.clone(), living.clone(), key.clone(), bot_token.clone(), invite.clone()]);
    for id in me["houses"].as_array().into_iter().flatten().chain(me["cities"].as_array().into_iter().flatten()) {
        if let Some(text) = id["id"].as_str() {
            secrets.push(text.to_string());
        }
    }
    if let Some(token) = feed.split('"').find(|part| part.len() == 40 && part.chars().all(|c| c.is_ascii_alphanumeric())) {
        secrets.push(token.to_string());
    }

    // What a stranger reads: both views, the list, a look and a how-am-i-doing, and the headers.
    let strangers = server.guest().await;
    let bot_player_id = server.player_id(&token).await;
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut note = |what: &str, reply: Reply| {
        let headers = format!("{:?}", reply.headers);
        seen.push((format!("{what} body"), reply.text));
        seen.push((format!("{what} headers"), headers));
    };
    tokio::time::sleep(Duration::from_millis(1100)).await;
    note("commons view", server.get("/api/worlds/commons").send().await);
    note("own view", server.get(&format!("/api/worlds/{own_id}")).send().await);
    note("list", server.get("/api/worlds").send().await);
    note("guest me", server.get("/api/worlds/commons/me").token(&strangers.token).send().await);
    note("claimed me", server.get("/api/worlds/commons/me").token(&guest.token).send().await);
    note("bot player me", server.get("/api/worlds/commons/me").token(&token).send().await);
    note("look", server.rpc("tools/call", json!({ "name": "look", "arguments": { "player_token": token } })).await);
    note("how", server.rpc("tools/call", json!({ "name": "how_am_i_doing", "arguments": { "player_token": token } })).await);
    note("join as bot", server.post("/api/public/join", json!({})).token(&bot_token).send().await);
    note("a guest's mine", server.get("/api/players/mine").token(&strangers.token).send().await);
    note("another person's mine", server.get("/api/players/mine").token(&server.person("Eve").await.key).send().await);
    note("claim an unknown code", server.post("/api/players/claim", json!({ "claim_code": "zzz" })).token(&server.person("Fay").await.key).send().await);
    note("reroll someone else's", server.post(&format!("/api/players/{}/reroll", bot_player_id), json!({})).token(&server.person("Gus").await.key).send().await);
    note("remove as a stranger", server.post(&format!("/api/worlds/{own_id}/players/1/remove"), json!({})).token(&server.person("Hal").await.key).send().await);
    for (what, text) in &seen {
        for secret in &secrets {
            assert!(!text.contains(secret.as_str()), "{what} contains {secret:?}: {text}");
        }
    }
    // What the claiming person sees is theirs alone, and still names no account.
    let theirs = server.get("/api/players/mine").token(&key).send().await.text;
    for secret in [&person, &agent, &city, &house_id, &key, &bot_token, &invite] {
        assert!(!theirs.contains(secret.as_str()), "mine contains {secret}: {theirs}");
    }
    // No id in a path or a name is built from an account's: the world's id and a player's name
    // share nothing with the person they belong to.
    assert!(!own_id.contains(&person[..6]) && !own_id.contains(&person[10..]));
}

// ----------------------------------------------------------------------------------------------
// Free text getting in (W6-W8) and numbers going wrong
// ----------------------------------------------------------------------------------------------

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

/// Numbers that break code: the ends of every integer size, the places a number wraps, and the
/// edges of the map.
const EXTREMES: [i64; 36] = [
    i64::MIN,
    i64::MIN + 1,
    i64::MIN / 2,
    -4_294_967_297,
    -4_294_967_296,
    -2_147_483_649,
    -2_147_483_648,
    -65_537,
    -65_536,
    -257,
    -256,
    -1,
    0,
    1,
    15,
    16,
    63,
    64,
    65,
    127,
    128,
    129,
    255,
    256,
    32_767,
    32_768,
    65_535,
    65_536,
    2_147_483_647,
    2_147_483_648,
    4_294_967_295,
    4_294_967_296,
    4_294_967_296 + 64,
    i64::MAX / 2,
    i64::MAX - 1,
    i64::MAX,
];

fn is_broken(result: &Result<Reply, reqwest::Error>) -> Option<String> {
    match result {
        Err(error) => Some(format!("the connection broke: {error}")),
        Ok(reply) if reply.status.is_server_error() => Some(reply.show()),
        Ok(_) => None,
    }
}

// W6, W13
#[tokio::test]
async fn extreme_numbers_in_every_field_of_every_move_break_nothing() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut broken = Vec::new();
    let mut sent = 0;
    for action in ["road", "place", "zone", "park", "bulldoze", "walk", "emote"] {
        for _ in 0..600 {
            let mut body = json!({ "action": action });
            let fields = ["x", "y", "x2", "y2"];
            for field in fields {
                if rng.below(4) != 0 || field == "x" || field == "y" {
                    body[field] = json!(rng.pick(&EXTREMES));
                }
            }
            if action == "zone" {
                body["kind"] = json!(rng.pick(&["homes", "shops", "workshops", "park", "road"]));
            }
            if action == "place" {
                body["kind"] = json!(rng.pick(&["house", "cafe", "club", "studio", "homes", "road", "nope"]));
            }
            if action == "emote" {
                body["emote"] = json!(rng.pick(&["wave", "dance", "nope"]));
            }
            let result = server.req(Method::POST, "/api/worlds/commons/act").token(&ann.key).json(body.clone()).try_send().await;
            sent += 1;
            if let Some(why) = is_broken(&result) {
                broken.push(format!("{body}: {why}"));
            }
        }
        let alive = server.get("/api/worlds/commons/me").token(&ann.key).try_send().await;
        assert!(alive.is_ok(), "the server is gone after {action}: {broken:#?}");
    }
    assert!(sent > 3000);
    assert!(broken.is_empty(), "{} of {sent} moves broke something:\n{}", broken.len(), broken.iter().take(10).cloned().collect::<Vec<_>>().join("\n"));
}

// W6, W13
#[tokio::test]
async fn every_way_to_write_a_number_that_is_not_a_small_whole_one_is_refused_and_breaks_nothing() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let ann = server.person("Ann").await;
    let numbers = [
        "9223372036854775807",
        "9223372036854775808",
        "-9223372036854775808",
        "-9223372036854775809",
        "18446744073709551615",
        "18446744073709551616",
        "1e19",
        "-1e19",
        "1e400",
        "1E2",
        "0.5",
        "-0",
        "-0.0",
        "0x40",
        "+64",
        "064",
        ".5",
        "5.",
        "NaN",
        "Infinity",
        "-Infinity",
        "null",
        "true",
        "\"64\"",
        "[64]",
        "{\"a\":64}",
        "\"\"",
        "99999999999999999999999999999999999999999999999999999999999999999999999999999999999999999999",
    ];
    let mut broken = Vec::new();
    let mut accepted = Vec::new();
    for number in numbers {
        for body in [
            format!(r#"{{"action":"walk","x":{number},"y":65}}"#),
            format!(r#"{{"action":"walk","x":65,"y":{number}}}"#),
            format!(r#"{{"action":"road","x":65,"y":65,"x2":{number},"y2":65}}"#),
            format!(r#"{{"action":"place","kind":"house","x":{number},"y":{number}}}"#),
            format!(r#"{{"action":"place","kind":"house","x":{number},"y":65}}"#),
            format!(r#"{{"action":"zone","kind":"homes","x":{number},"y":{number},"x2":{number},"y2":{number}}}"#),
            format!(r#"{{"action":"park","x":{number},"y":{number}}}"#),
            format!(r#"{{"action":"bulldoze","x":{number},"y":{number}}}"#),
        ] {
            // A place is a person's move, so a person tries it as well as a guest.
            let token = if body.contains(r#""place""#) { &ann.key } else { &guest.token };
            let result = server
                .req(Method::POST, "/api/worlds/commons/act")
                .token(token)
                .raw("application/json", body.as_bytes())
                .try_send()
                .await;
            if let Some(why) = is_broken(&result) {
                broken.push(format!("{body}: {why}"));
            } else if let Ok(reply) = &result {
                if reply.status.is_success() {
                    accepted.push(body);
                }
            }
        }
    }
    assert!(broken.is_empty(), "{broken:#?}");
    // Only `-0` is a whole number among them (it is 0), so only a body that holds it may be done.
    for body in &accepted {
        assert!(body.contains(":-0"), "{body} was accepted");
    }
}

// W14, W16, W55
#[tokio::test]
async fn a_coordinate_that_wraps_to_a_tile_on_the_map_is_still_off_the_map() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let other = server.guest().await;
    let ann = server.person("Ann").await;
    let map = Map::from_db(&server, COMMONS);
    let (road_first, road_last) = map.line_from_road(2).unwrap();
    server.acts_ok(&guest.token, COMMONS, road_move(road_first, road_last)).await;
    // A tile a place could really go on, beside the road that was just laid.
    let spot = Map::from_db(&server, COMMONS).house_spots().into_iter().find(|&(x, y)| {
        [road_first, road_last].iter().any(|&(rx, ry)| (rx - x).abs() + (ry - y).abs() == 1)
    }).unwrap();
    let before = server.tiles(COMMONS);
    let left = server.moves_left(&guest.token).await;
    let offsets: [i64; 11] = [
        256,
        65_536,
        16_777_216,
        2_147_483_648,
        4_294_967_296,
        4_294_967_296 + 65_536,
        1 << 40,
        1 << 62,
        -256,
        -65_536,
        -(1 << 32),
    ];
    let mut done = Vec::new();
    for k in offsets {
        let mut attempts: Vec<Value> = vec![
            json!({ "action": "bulldoze", "x": road_first.0 + k, "y": road_first.1 }),
            json!({ "action": "bulldoze", "x": road_first.0, "y": road_first.1 + k }),
            json!({ "action": "road", "x": road_last.0 + k, "y": road_last.1, "x2": road_last.0 + k, "y2": road_last.1 }),
            json!({ "action": "road", "x": road_first.0, "y": road_first.1 + k, "x2": road_first.0, "y2": road_first.1 + k }),
            json!({ "action": "walk", "x": 65 + k, "y": 64 }),
            json!({ "action": "walk", "x": 65, "y": 64 + k }),
        ];
        // The far end of a road or a zone that wraps to a legal span.
        attempts.push(json!({ "action": "road", "x": road_first.0, "y": road_first.1, "x2": road_last.0 + k, "y2": road_last.1 + k }));
        for body in attempts {
            let reply = server.act(&guest.token, COMMONS, body.clone()).await;
            if reply.status.is_success() {
                done.push(format!("{body}"));
            }
        }
        // A person's place, whose tile is one beside a road, wrapped onto the map.
        for body in [
            place_move("house", (spot.0 + k, spot.1)),
            place_move("house", (spot.0, spot.1 + k)),
            place_move("house", (spot.0 + k, spot.1 + k)),
        ] {
            let reply = server.act(&ann.key, COMMONS, body.clone()).await;
            if reply.status.is_success() {
                done.push(format!("{body}"));
            }
        }
        // `look` and the other tools take the same numbers.
        for args in [
            json!({ "player_token": guest.token, "x": 64 + k, "y": 64 }),
            json!({ "player_token": guest.token, "x": 64, "y": 64 + k }),
        ] {
            let result = server.call("look", args.clone()).await;
            if result["isError"].is_null() {
                done.push(format!("look {args}"));
            }
        }
        for (name, args) in [
            ("walk", json!({ "player_token": other.token, "x": 65 + k, "y": 64 })),
            ("bulldoze", json!({ "player_token": other.token, "x": road_first.0 + k, "y": road_first.1 })),
            ("build", json!({ "player_token": other.token, "type": "road", "x": spot.0 + k, "y": spot.1 })),
        ] {
            let result = server.call(name, args.clone()).await;
            if result["isError"].is_null() {
                done.push(format!("{name} {args}"));
            }
        }
    }
    assert!(done.is_empty(), "numbers that only wrap onto the map were accepted:\n{}", done.join("\n"));
    assert_eq!(server.tiles(COMMONS), before);
    assert_eq!(server.moves_left(&guest.token).await, left);
    assert_eq!(server.moves_left(&other.token).await, 30);
    assert_eq!(server.moves_left(&ann.key).await, 300);
}

// W6, W8
#[tokio::test]
async fn every_shape_of_a_json_rpc_request_gets_an_answer_and_none_echoes_it() {
    let server = Server::start().await;
    let token = server.tool("join", json!({})).await["player_token"].as_str().unwrap().to_string();
    let mark = "zzq-marker-4471";
    let bodies: Vec<String> = vec![
        "[]".into(),
        "[[]]".into(),
        format!("[{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"{mark}\"}}}}]"),
        "null".into(),
        "123".into(),
        format!("\"{mark}\""),
        "{}".into(),
        format!("{{\"{mark}\":\"{mark}\"}}"),
        format!("{{\"jsonrpc\":\"{mark}\",\"id\":1,\"method\":\"ping\"}}"),
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":5}".into(),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"{mark}\"}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":\"{mark}\"}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":[\"{mark}\"]}}"),
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":5}}".to_string(),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"{mark}\"}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":[\"{mark}\"]}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"join\",\"arguments\":\"{mark}\"}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"join\",\"arguments\":[\"{mark}\"]}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"join\",\"arguments\":{{\"world_id\":\"{mark}\"}}}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"join\",\"arguments\":{{\"world_id\":[\"{mark}\"]}}}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"look\",\"arguments\":{{\"world_id\":\"{mark}\",\"player_token\":\"{mark}\"}}}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"look\",\"arguments\":{{\"x\":\"{mark}\"}}}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"build\",\"arguments\":{{\"player_token\":\"{token}\",\"type\":\"{mark}\",\"x\":1,\"y\":1}}}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"build\",\"arguments\":{{\"player_token\":\"{token}\",\"type\":\"road\",\"x\":1,\"y\":1,\"{mark}\":1}}}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"emote\",\"arguments\":{{\"player_token\":\"{token}\",\"emote\":\"{mark}\"}}}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"emote\",\"arguments\":{{\"player_token\":\"{mark}\",\"emote\":\"wave\"}}}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"how_am_i_doing\",\"arguments\":{{\"player_token\":\"{token}\",\"world_id\":\"{mark}\"}}}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"{mark}\"}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":[\"{mark}\"],\"clientInfo\":{{\"name\":\"{mark}\"}}}}}}"),
        format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\",\"params\":{{\"cursor\":\"{mark}\"}}}}"),
        "{\"jsonrpc\":\"2.0\",\"id\":null,\"method\":\"ping\"}".into(),
        "{\"jsonrpc\":\"2.0\",\"id\":1.5e300,\"method\":\"ping\"}".into(),
        "{\"jsonrpc\":\"2.0\",\"id\":[[[[[]]]]],\"method\":\"ping\"}".into(),
        "{\"jsonrpc\":\"2.0\",\"method\":\"ping\"}".into(),
        "\u{feff}{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}".into(),
    ];
    let mut echoed = Vec::new();
    let mut broken = Vec::new();
    for body in &bodies {
        let result = server.req(Method::POST, "/public/mcp").raw("application/json", body.as_bytes()).try_send().await;
        match result {
            Err(error) => broken.push(format!("{body}: {error}")),
            Ok(reply) => {
                if reply.status.is_server_error() {
                    broken.push(format!("{body}: {}", reply.show()));
                }
                if reply.text.contains(mark) || format!("{:?}", reply.headers).contains(mark) {
                    echoed.push(format!("{body} -> {}", reply.text));
                }
            }
        }
    }
    // Not JSON at all, in every encoding a client might get wrong.
    for raw in [b"\xff\xfe{}".as_slice(), b"{", b"", b"\0\0\0", b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"pi\xffng\"}"] {
        let result = server.req(Method::POST, "/public/mcp").raw("application/json", raw).try_send().await;
        match result {
            Err(error) => broken.push(format!("{raw:?}: {error}")),
            Ok(reply) if reply.status.is_server_error() => broken.push(format!("{raw:?}: {}", reply.show())),
            Ok(_) => {}
        }
    }
    // A very deep body is refused, not recursed into until the stack runs out.
    let deep = format!("{}{}", "[".repeat(60_000), "]".repeat(60_000));
    let result = server.req(Method::POST, "/public/mcp").raw("application/json", deep.as_bytes()).try_send().await;
    assert!(result.is_ok(), "a deeply nested body broke the connection");
    let deep_object = format!("{}1{}", "{\"a\":".repeat(10_000), "}".repeat(10_000));
    let result = server.req(Method::POST, "/public/mcp").raw("application/json", deep_object.as_bytes()).try_send().await;
    assert!(result.is_ok(), "a deeply nested object broke the connection");
    assert!(broken.is_empty(), "{broken:#?}");
    assert!(echoed.is_empty(), "a request came back in the reply:\n{}", echoed.join("\n"));
    assert_eq!(server.moves_left(&token).await, 30, "none of it cost a move");
}

// W6, W8
#[tokio::test]
async fn nothing_a_caller_sends_comes_back_in_any_world_reply_or_header() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    let (world, code) = server.frozen_world(&ann, false).await;
    let mark = "zzq-marker-4471";
    let mut tokens: Vec<Option<String>> = vec![None, Some(guest.token.clone()), Some(ann.key.clone()), Some(mark.to_string()), Some(code.clone())];
    tokens.dedup();
    let gets = [
        format!("/api/worlds/{mark}"),
        format!("/api/worlds/{mark}/me"),
        format!("/api/worlds/commons/{mark}"),
        format!("/api/worlds/{mark}/{mark}"),
        format!("/api/worlds/join/{mark}"),
        format!("/api/players/{mark}"),
        format!("/api/players/mine?{mark}={mark}"),
        format!("/api/worlds?{mark}={mark}"),
        format!("/api/worlds/commons?{mark}"),
        format!("/api/worlds/{world}?{mark}={mark}"),
        format!("/api/worlds/{world}/me?{mark}={mark}"),
        format!("/world/{mark}"),
        format!("/worlds?{mark}={mark}"),
        format!("/{mark}"),
        format!("/llms.txt?{mark}"),
        format!("/agents.md?{mark}={mark}"),
        format!("/api/worlds/preview?{mark}"),
        format!("/api/worlds/mine?x={mark}"),
        format!("/public/mcp?{mark}"),
    ];
    let posts = [
        format!("/api/worlds/{mark}/act"),
        "/api/worlds/commons/act".to_string(),
        format!("/api/worlds/{world}/act"),
        format!("/api/worlds/{mark}/players/{mark}/remove"),
        format!("/api/worlds/commons/players/{mark}/remove"),
        format!("/api/worlds/{world}/players/{mark}/remove"),
        format!("/api/worlds/join/{mark}"),
        format!("/api/players/{mark}/reroll"),
        "/api/players/claim".to_string(),
        "/api/public/join".to_string(),
        "/api/worlds".to_string(),
    ];
    let bodies: Vec<Value> = vec![
        json!({}),
        json!({ "action": mark }),
        json!({ "action": "emote", "emote": mark }),
        json!({ "action": "zone", "kind": mark, "x": 1, "y": 1 }),
        json!({ "action": "place", "kind": mark, "x": 1, "y": 1 }),
        json!({ "action": "walk", "x": mark, "y": 1 }),
        json!({ mark: mark }),
        json!({ "action": "emote", "emote": "wave", mark: 1 }),
        json!({ "claim_code": mark }),
        json!({ "claim_code": [mark] }),
        json!({ "world_id": mark }),
        json!({ "world_id": [mark] }),
        json!({ "seed": mark }),
        json!({ "seed": 1, "open": mark }),
        json!([mark]),
        json!(mark),
    ];
    let headers: Vec<(&str, String)> = vec![
        ("user-agent", mark.to_string()),
        ("referer", format!("https://{mark}.example/")),
        ("origin", format!("https://{mark}.example")),
        ("x-forwarded-for", mark.to_string()),
        ("x-forwarded-for", format!("{mark}, 203.0.113.9")),
        ("x-forwarded-host", mark.to_string()),
        ("accept", mark.to_string()),
        ("accept-language", mark.to_string()),
        ("if-none-match", format!("\"{mark}\"")),
        ("content-type", mark.to_string()),
        ("x-anything", mark.to_string()),
    ];
    let mut echoed: Vec<String> = Vec::new();
    let mut broken: Vec<String> = Vec::new();
    let mut check = |what: String, result: Result<Reply, reqwest::Error>| match result {
        Err(error) => broken.push(format!("{what}: {error}")),
        Ok(reply) => {
            if reply.status.is_server_error() {
                broken.push(format!("{what}: {}", reply.show()));
            }
            if reply.text.contains(mark) || format!("{:?}", reply.headers).contains(mark) {
                echoed.push(format!("{what} -> {} {:?}", reply.text, reply.headers));
            }
        }
    };
    for token in &tokens {
        for path in &gets {
            let mut request = server.get(path);
            if let Some(token) = token {
                request = request.token(token);
            }
            check(format!("GET {path} as {token:?}"), request.try_send().await);
            for (name, value) in &headers {
                let mut request = server.get(path).header(name, value);
                if let Some(token) = token {
                    request = request.token(token);
                }
                check(format!("GET {path} with {name}: {value} as {token:?}"), request.try_send().await);
            }
        }
        for path in &posts {
            for body in &bodies {
                let mut request = server.post(path, body.clone());
                if let Some(token) = token {
                    request = request.token(token);
                }
                check(format!("POST {path} {body} as {token:?}"), request.try_send().await);
            }
            for (name, value) in &headers {
                let mut request = server.post(path, json!({ "action": "emote", "emote": "wave" })).header(name, value);
                if let Some(token) = token {
                    request = request.token(token);
                }
                check(format!("POST {path} with {name}: {value} as {token:?}"), request.try_send().await);
            }
        }
        // A cookie in place of a header.
        let mut request = server.post("/api/worlds/commons/act", json!({ "action": "emote", "emote": "wave" })).cookie(&format!("ah_session={mark}"));
        if let Some(token) = token {
            request = request.header("x-token", token);
        }
        check("POST act with a made-up session cookie".to_string(), request.try_send().await);
    }
    // And the public MCP endpoint, with the marker in every position there is.
    for name in ["join", "look", "build", "bulldoze", "walk", "emote", "how_am_i_doing", mark] {
        for args in [
            json!({ "player_token": mark }),
            json!({ "player_token": guest.token, "world_id": mark }),
            json!({ "world_id": mark }),
            json!({ "player_token": guest.token, "type": mark, "x": 1, "y": 1, "emote": mark }),
            json!({ mark: mark }),
            json!({ "player_token": guest.token, mark: 1 }),
        ] {
            let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
            check(format!("MCP {call}"), server.req(Method::POST, "/public/mcp").json(call.clone()).try_send().await);
            for (header, value) in &headers {
                check(
                    format!("MCP {call} with {header}: {value}"),
                    server.req(Method::POST, "/public/mcp").json(call.clone()).header(header, value).try_send().await,
                );
            }
        }
    }
    assert!(broken.is_empty(), "{} requests broke something:\n{}", broken.len(), broken.iter().take(10).cloned().collect::<Vec<_>>().join("\n"));
    assert!(echoed.is_empty(), "{} replies carried what was sent:\n{}", echoed.len(), echoed.iter().take(10).cloned().collect::<Vec<_>>().join("\n"));
}

// W7
#[tokio::test]
async fn the_name_lists_give_every_number_its_own_name_and_no_combination_is_rude() {
    // Every number below the number of names gets a different one, and then the names go round.
    let first = world::player_name(0);
    let total = (1..100_000u64).find(|&n| world::player_name(n) == first).expect("names repeat");
    let names: HashSet<String> = (0..total).map(world::player_name).collect();
    assert_eq!(names.len() as u64, total, "a name is given to two numbers before the list is used up");
    let visitors: HashSet<String> = (0..total).map(world::visitor_name).collect();
    assert_eq!(visitors, names, "visitors use the same lists");
    let worlds: HashSet<String> = (0..20_000u64).map(world::world_name).collect();
    assert!(worlds.len() > 500, "world names come from the lists, and there are plenty");
    // A few words that must not appear in any name, even across the join of two words.
    let rude = [
        "fuck", "shit", "cunt", "dick", "cock", "piss", "nazi", "rape", "slut", "whore", "anal", "anus", "porn", "hitler", "kkk",
        "nigg", "fag", "crap", "damn", "hell", "sex", "nude", "drug", "kill", "dead", "suicide", "gun", "bomb", "jew", "isis",
        "boob", "tit ", " tit", "arse", "bitch", "bastard", "twat", "wank", "rapist", "pedo", "kike", "spic", "chink", "coon",
    ];
    for name in names.iter().chain(worlds.iter()) {
        let flat = name.to_lowercase();
        let joined = flat.replace(' ', "");
        for word in rude {
            assert!(!flat.contains(word) && !joined.contains(word.trim()), "{name:?} contains {word:?}");
        }
        assert!(name.chars().all(|c| c.is_ascii_alphabetic() || c == ' '), "{name:?}");
        assert!(name.len() <= 40, "{name:?} is long");
    }
}

// ----------------------------------------------------------------------------------------------
// Spending more than you have (W9-W12) and the limits (W30)
// ----------------------------------------------------------------------------------------------

/// A way to send a request from a task of its own, for races.
#[derive(Clone)]
struct Wire {
    client: Client,
    base: String,
}

impl Server {
    fn wire(&self) -> Wire {
        Wire { client: self.client.clone(), base: self.base.clone() }
    }
}

impl Wire {
    /// The status and body. A connection that broke is 599.
    async fn send(&self, method: Method, path: &str, token: Option<&str>, ip: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let mut request = self.client.request(method, format!("{}{}", self.base, path));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(ip) = ip {
            request = request.header("x-forwarded-for", ip);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                (status, serde_json::from_str(&text).unwrap_or(Value::Null))
            }
            Err(_) => (StatusCode::from_u16(599).unwrap(), Value::Null),
        }
    }
}


fn count(replies: &[(StatusCode, Value)], status: StatusCode) -> usize {
    replies.iter().filter(|(s, _)| *s == status).count()
}

// W3, W9
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bot_joining_in_parallel_ends_up_with_one_player_one_token_and_one_allowance() {
    let server = Server::start().await;
    let bot = server.bot("Scout").await;
    let wire = server.wire();
    let mut tasks = Vec::new();
    for _ in 0..24 {
        let (wire, token) = (wire.clone(), bot.token.clone());
        tasks.push(tokio::spawn(async move { wire.send(Method::POST, "/api/public/join", Some(&token), None, Some(json!({}))).await }));
    }
    let mut tokens = Vec::new();
    for task in tasks {
        let (status, body) = task.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        tokens.push(body["player_token"].as_str().unwrap().to_string());
    }
    let mut live = 0;
    for token in &tokens {
        if server.get("/api/worlds/commons/me").token(token).send().await.status == StatusCode::OK {
            live += 1;
        }
    }
    assert_eq!(live, 1, "only the last token given can be live");
    assert_eq!(server.state.db.players_of(&bot.person).unwrap().len(), 1);
    // Everyone who raced is the same player, so they share one allowance.
    let survivor = tokens.iter().find(|token| server.state.db.player_by_token(&auth::hash_secret(token)).unwrap().is_some()).unwrap();
    assert_eq!(server.moves_left(survivor).await, 200);
}

// W9
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_acting_for_the_first_time_in_many_requests_at_once_gets_one_player_and_one_allowance() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let wire = server.wire();
    let mut tasks = Vec::new();
    for _ in 0..100 {
        let (wire, key) = (wire.clone(), ann.key.clone());
        tasks.push(tokio::spawn(async move {
            wire.send(Method::POST, "/api/worlds/commons/act", Some(&key), None, Some(json!({ "action": "emote", "emote": "wave" }))).await
        }));
    }
    let mut done = 0;
    for task in tasks {
        let (status, body) = task.await.unwrap();
        assert!(status == StatusCode::OK || status == StatusCode::TOO_MANY_REQUESTS, "{status} {body}");
        if status == StatusCode::OK {
            done += 1;
        }
    }
    let mine = server.get("/api/players/mine").token(&ann.key).send().await.ok();
    assert_eq!(mine["players"].as_array().unwrap().len(), 1, "one player of their own");
    assert_eq!(mine["players"][0]["moves_left"], 300 - done);
}

// W11
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_claiming_thirty_guests_at_once_gets_twenty() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let mut guests = Vec::new();
    for _ in 0..30 {
        guests.push(server.guest().await);
    }
    let wire = server.wire();
    let mut tasks = Vec::new();
    for guest in &guests {
        let (wire, key, claim) = (wire.clone(), ann.key.clone(), guest.claim.clone());
        tasks.push(tokio::spawn(async move { wire.send(Method::POST, "/api/players/claim", Some(&key), None, Some(json!({ "claim_code": claim }))).await }));
    }
    let mut replies = Vec::new();
    for task in tasks {
        replies.push(task.await.unwrap());
    }
    assert_eq!(count(&replies, StatusCode::OK), 20, "{replies:?}");
    assert_eq!(count(&replies, StatusCode::CONFLICT), 10);
    assert_eq!(server.get("/api/players/mine").token(&ann.key).send().await.ok()["players"].as_array().unwrap().len(), 20);
    // The ten that were turned away are still unclaimed, and someone else can have them.
    let mut taken = 0;
    for (guest, reply) in guests.iter().zip(&replies) {
        if reply.0 == StatusCode::CONFLICT && server.post("/api/players/claim", json!({ "claim_code": guest.claim })).token(&bo.key).send().await.status == StatusCode::OK {
            taken += 1;
        }
    }
    assert_eq!(taken, 10);
}

// W11
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_people_racing_for_one_claim_code_leave_it_with_one() {
    let server = Server::start().await;
    let people = [server.person("Ann").await, server.person("Bo").await, server.person("Cy").await];
    for _ in 0..5 {
        let guest = server.guest().await;
        let wire = server.wire();
        let mut tasks = Vec::new();
        for person in &people {
            for _ in 0..3 {
                let (wire, key, claim) = (wire.clone(), person.key.clone(), guest.claim.clone());
                tasks.push(tokio::spawn(async move { wire.send(Method::POST, "/api/players/claim", Some(&key), None, Some(json!({ "claim_code": claim }))).await }));
            }
        }
        let mut won = 0;
        for task in tasks {
            if task.await.unwrap().0 == StatusCode::OK {
                won += 1;
            }
        }
        assert_eq!(won, 1);
    }
    let total: usize = futures_util::future::join_all(
        people.iter().map(|person| async { server.get("/api/players/mine").token(&person.key).send().await.ok()["players"].as_array().unwrap().len() }),
    )
    .await
    .iter()
    .sum();
    assert_eq!(total, 5, "each guest belongs to exactly one person");
}

// W25: nobody can start a world, so a flood of tries, in any number and from anyone, makes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flood_of_tries_to_start_a_world_is_refused_and_makes_nothing() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let ann = server.person("Ann").await;
    let wire = server.wire();
    let mut tasks = Vec::new();
    for n in 0..60u64 {
        let (wire, key) = (wire.clone(), ann.key.clone());
        let body = if n % 3 == 0 { json!({}) } else { json!({ "seed": 1000 + n, "open": n % 2 == 0 }) };
        tasks.push(tokio::spawn(async move { wire.send(Method::POST, "/api/worlds", Some(&key), None, Some(body)).await }));
    }
    let mut replies = Vec::new();
    for task in tasks {
        replies.push(task.await.unwrap());
    }
    assert_eq!(count(&replies, StatusCode::GONE), 60, "{replies:?}");
    assert!(server.state.db.world_of_owner(&ann.id).unwrap().is_none());
    let rows = server.state.db.busiest_worlds(0, 100).unwrap();
    assert_eq!(rows.len(), 1, "a world was made");
    // The same with no sign-in at all, and with a guest.
    let guest = server.guest().await;
    for token in [None, Some(guest.token.as_str())] {
        let (status, _) = wire.send(Method::POST, "/api/worlds", token, None, Some(json!({ "seed": 5 }))).await;
        assert_eq!(status, StatusCode::GONE);
    }
    assert_eq!(server.state.db.busiest_worlds(0, 100).unwrap().len(), 1);
}

// W30
#[tokio::test]
async fn moves_that_are_refused_for_any_reason_never_use_up_the_citys_300_a_minute() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let (closed, _) = server.frozen_world(&ann, false).await;
    let spent = server.guest().await;
    for _ in 0..30 {
        server.acts_ok(&spent.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    }
    assert_eq!(server.moves_left(&spent.token).await, 0);
    let stranger = server.guest().await;
    let map = Map::from_db(&server, COMMONS);
    let far = (0..map.size)
        .flat_map(|y| (0..map.size).map(move |x| (x, y)))
        .find(|&(x, y)| map.open(x, y) && map.roads().iter().all(|&(rx, ry)| (rx - x).abs() > 5 || (ry - y).abs() > 5))
        .unwrap();
    let attempts: Vec<(&str, String, Option<String>, Value)> = vec![
        ("no token", COMMONS.to_string(), None, json!({ "action": "emote", "emote": "wave" })),
        ("a made-up token", COMMONS.to_string(), Some(made_up()), json!({ "action": "emote", "emote": "wave" })),
        ("no moves left", COMMONS.to_string(), Some(spent.token.clone()), json!({ "action": "emote", "emote": "wave" })),
        ("too dear", COMMONS.to_string(), Some(spent.token.clone()), road_move(far, far)),
        ("a place by a guest", COMMONS.to_string(), Some(stranger.token.clone()), place_move("house", far)),
        ("a frozen world", closed.clone(), Some(stranger.token.clone()), json!({ "action": "emote", "emote": "wave" })),
        ("a road in a frozen world", closed.clone(), Some(stranger.token.clone()), road_move(far, far)),
        ("zoning", COMMONS.to_string(), Some(stranger.token.clone()), json!({ "action": "zone", "kind": "homes", "x": far.0, "y": far.1 })),
        ("no such world", "nowhere".to_string(), Some(stranger.token.clone()), json!({ "action": "emote", "emote": "wave" })),
        ("not a move", COMMONS.to_string(), Some(stranger.token.clone()), json!({ "action": "nope" })),
        ("off the map", COMMONS.to_string(), Some(stranger.token.clone()), json!({ "action": "park", "x": -1, "y": 5 })),
        ("a place off the map", COMMONS.to_string(), Some(spent.token.clone()), place_move("house", (-1, 5))),
        ("a road in the air", COMMONS.to_string(), Some(stranger.token.clone()), json!({ "action": "road", "x": far.0, "y": far.1 })),
        ("nothing to bulldoze", COMMONS.to_string(), Some(stranger.token.clone()), json!({ "action": "bulldoze", "x": far.0, "y": far.1 })),
        ("a walk too far", COMMONS.to_string(), Some(stranger.token.clone()), json!({ "action": "walk", "x": 0, "y": 0 })),
    ];
    for (what, world, token, body) in attempts {
        for _ in 0..40 {
            let mut request = server.post(&format!("/api/worlds/{world}/act"), body.clone());
            if let Some(token) = &token {
                request = request.token(token);
            }
            let reply = request.send().await;
            assert!(reply.status.is_client_error(), "{what}: {}", reply.show());
        }
    }
    // The city's whole allowance is still there: a third of a thousand refusals used none of it.
    let mut served = 0;
    for _ in 0..10 {
        let guest = server.guest().await;
        for _ in 0..30 {
            if server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.status == StatusCode::OK {
                served += 1;
            }
        }
    }
    assert!(served >= 270, "{served} of the 270 moves that were left got through after refused ones");
}

// W12
#[tokio::test]
async fn one_address_hammering_join_cannot_use_up_the_servers_new_guests_for_everyone_else() {
    let server = Server::start().await;
    let mut ok = 0;
    for _ in 0..200 {
        if server.post("/api/public/join", json!({})).ip("203.0.113.9").send().await.status == StatusCode::OK {
            ok += 1;
        }
    }
    assert_eq!(ok, 5);
    // Through MCP as well.
    for _ in 0..100 {
        server.post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "join", "arguments": {} } })).ip("203.0.113.9").send().await;
    }
    let mut others = 0;
    for _ in 0..60 {
        let ip = server.fresh_ip();
        if server.post("/api/public/join", json!({})).ip(&ip).send().await.status == StatusCode::OK {
            others += 1;
        }
    }
    assert_eq!(others, 60, "{others} other addresses got in");
}

// W12, W30
#[tokio::test]
async fn an_address_header_that_cannot_be_read_is_still_one_limited_address() {
    start_early_in(60, 30).await;
    let server = Server::start().await;
    for junk in ["unknown", "203.0.113.7,", "203.0.113.7:443", "203.0.113.7, nope", ",", "[2001:db8::1]", "2001:db8::1%eth0", "-", "0", "not an address", "203.0.113.7;"] {
        let mut joined = 0;
        for _ in 0..12 {
            if server.post("/api/public/join", json!({})).header("x-forwarded-for", junk).send().await.status == StatusCode::OK {
                joined += 1;
            }
        }
        assert!(joined <= 5, "{joined} of 12 guests joined from the address {junk:?}, which no limit can count");
        let mut views = 0;
        for _ in 0..250 {
            if server.get("/api/worlds/commons").header("x-forwarded-for", junk).send().await.status == StatusCode::OK {
                views += 1;
            }
        }
        assert!(views <= 240, "{views} of 250 views were served to the address {junk:?}, which no limit can count");
    }
}

// W12, W30
#[tokio::test]
async fn a_client_cannot_slip_past_a_limit_by_how_it_writes_its_own_address() {
    let server = Server::start().await;
    // The last address is the proxy's word. These all end in the same real address.
    let real = "198.51.100.77";
    let forged = [
        real.to_string(),
        format!("203.0.113.1, {real}"),
        format!("::1, {real}"),
        format!("{real} "),
        format!(" {real}"),
        format!("203.0.113.2,{real}"),
        format!("203.0.113.3 , {real}"),
    ];
    let mut joined = 0;
    for n in 0..14 {
        let forge = &forged[n % forged.len()];
        if server.post("/api/public/join", json!({})).header("x-forwarded-for", forge).send().await.status == StatusCode::OK {
            joined += 1;
        }
    }
    assert_eq!(joined, 5, "one address, {joined} guests");
    // An IPv6 address written long or short, in capitals or not, is the same address.
    let spellings = ["2001:db8::78", "2001:0db8:0:0:0:0:0:78", "2001:DB8::78", "2001:db8:0000::0078"];
    let mut joined = 0;
    for spelling in spellings.iter().cycle().take(12) {
        if server.post("/api/public/join", json!({})).header("x-forwarded-for", spelling).send().await.status == StatusCode::OK {
            joined += 1;
        }
    }
    assert_eq!(joined, 5, "one address written four ways let {joined} guests in");
    // Two header lines: the proxy's is the last.
    let mut joined = 0;
    for _ in 0..8 {
        let reply = server.post("/api/public/join", json!({})).header("x-forwarded-for", "203.0.113.200").header("x-forwarded-for", real).send().await;
        if reply.status == StatusCode::OK {
            joined += 1;
        }
    }
    assert_eq!(joined, 0, "the proxy's line says this address has had its five");
}

// W9
#[tokio::test]
async fn a_late_request_from_before_midnight_cannot_refill_a_day_already_spent() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let guest = server.guest().await;
    let id = server.player_id(&guest.token).await;
    let before_midnight = now();
    let after_midnight = before_midnight + 86_400;
    // Today's moves, all spent after midnight.
    for _ in 0..30 {
        worlds::act(&server.state, &server.player_row(id), COMMONS, &emote("wave"), after_midnight).unwrap();
    }
    assert_eq!(worlds::moves_left(&server.player_row(id), after_midnight), 0);
    // A request that read the clock before midnight and got the database's lock after, which is
    // what a busy server does. It must not turn the day back.
    let late = worlds::act(&server.state, &server.player_row(id), COMMONS, &emote("wave"), before_midnight);
    assert!(late.is_err(), "a move stamped before midnight bought a whole new day: {late:?}");
    let again = worlds::act(&server.state, &server.player_row(id), COMMONS, &emote("wave"), after_midnight);
    assert!(again.is_err(), "and then the day after got it again: {again:?}");
}

// W9
#[tokio::test]
async fn rejoining_or_being_claimed_does_not_bring_back_moves_already_spent() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bot = server.bot("Scout").await;
    let token = server.bot_player(&bot).await;
    for _ in 0..5 {
        server.acts_ok(&token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    }
    let again = server.bot_player(&bot).await;
    assert_eq!(server.moves_left(&again).await, 195, "a new token is not a new day");
    // A guest that spent its 30 and is then claimed has 170 of the 200, not 200.
    let guest = server.guest().await;
    for _ in 0..30 {
        server.acts_ok(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    }
    server.post("/api/players/claim", json!({ "claim_code": guest.claim })).token(&ann.key).send().await.ok();
    assert_eq!(server.moves_left(&guest.token).await, 170);
    // And a person who plays as themself and through every player they hold spends from each.
    server.acts_ok(&ann.key, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    assert_eq!(server.moves_left(&guest.token).await, 170);
    assert_eq!(server.moves_left(&ann.key).await, 299);
}

// W9
#[tokio::test]
async fn how_many_moves_one_person_can_have_in_a_day_is_what_the_table_says_and_no_more() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    server.acts_ok(&ann.key, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    let mut guests = Vec::new();
    for _ in 0..25 {
        guests.push(server.guest().await);
    }
    let mut claimed = 0;
    for guest in &guests {
        if server.post("/api/players/claim", json!({ "claim_code": guest.claim })).token(&ann.key).send().await.status == StatusCode::OK {
            claimed += 1;
        }
    }
    assert_eq!(claimed, 20);
    // Bots: how many can a person have? Count the ones that get a player.
    let room = server.post("/api/hangouts", json!({ "name": "Crew", "your_name": "Dee" })).send().await.ok();
    let key = room["owner_key"].as_str().unwrap().to_string();
    let person = room["person"]["id"].as_str().unwrap().to_string();
    let hangout = room["hangout"]["id"].as_str().unwrap().to_string();
    let mut bots = 0;
    for n in 0..60 {
        let made = server.post(&format!("/api/hangouts/{hangout}/agents"), json!({ "name": format!("Bot{n}") })).token(&key).send().await;
        if made.status == StatusCode::OK {
            let token = made.body["token"].as_str().unwrap().to_string();
            if server.post("/api/public/join", json!({})).token(&token).send().await.status == StatusCode::OK {
                bots += 1;
            }
        }
    }
    let mine = server.get("/api/players/mine").token(&key).send().await.ok();
    let bot_moves: i64 = mine["players"].as_array().unwrap().iter().map(|player| player["moves_left"].as_i64().unwrap()).sum();
    let ann_moves: i64 = server.get("/api/players/mine").token(&ann.key).send().await.ok()["players"]
        .as_array()
        .unwrap()
        .iter()
        .map(|player| player["moves_left"].as_i64().unwrap())
        .sum();
    println!("COST REPORT: one person with a hangout of their own can have {bots} bots at 200 moves each = {bot_moves} moves a day; Ann holds {ann_moves} (299 + 20 x 200). Person {person}");
    assert_eq!(ann_moves, 299 + 20 * 200);
    assert!(bots <= 60);
}

// ----------------------------------------------------------------------------------------------
// Staying up (W29-W32): memory, the cache and the database
// ----------------------------------------------------------------------------------------------

// W29, W42, W44
#[tokio::test]
async fn a_flood_of_addresses_agents_and_tokens_leaves_the_memory_a_server_keeps_where_it_was() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let t = now();
    let flood = |from: u32, to: u32| {
        for n in from..to {
            let mut headers = HeaderMap::new();
            // Every kind of agent, from a different address each time.
            let agents = [GOOGLEBOT, GPTBOT, CHATGPT_USER, GEMINI, "curl/8.5", "python-requests/2.31", CHROME];
            headers.insert("user-agent", agents[n as usize % agents.len()].parse().unwrap());
            headers.insert("x-forwarded-for", format!("10.{}.{}.{}", (n >> 16) & 255, (n >> 8) & 255, n & 255).parse().unwrap());
            for path in ["/", "/worlds", "/world/commons", "/llms.txt", "/agents.md", "/robots.txt"] {
                worlds::note_request(&server.state, &Method::GET, path, &headers, t + i64::from(n % 500));
            }
            worlds::note_request(&server.state, &Method::POST, "/public/mcp", &headers, t + i64::from(n % 500));
            let _ = worlds::allow_view(&server.state, &headers, t + i64::from(n % 500));
        }
    };
    flood(0, 500);
    let left = retained(|| flood(500, 20_500));
    assert!(left < 128 * 1024, "20,000 visitors from different addresses left {left} bytes in memory");
    assert!(server.state.worlds.visitors(t + 499).len() <= 30);
    assert!(server.state.worlds.frogs(t + 499).len() <= world::CRAWLERS.len());
}

// W12, W29
#[tokio::test]
async fn a_flood_of_new_guests_from_different_addresses_leaves_memory_alone() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let t = now();
    let join = |n: u32| {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", format!("2001:db8::{n:x}").parse().unwrap());
        let _ = worlds::join(&server.state, &headers, None, t);
    };
    for n in 0..50 {
        join(n);
    }
    let left = retained(|| {
        for n in 50..3050 {
            join(n);
        }
    });
    // Rows go to the database; the server keeps nothing for an address that was refused.
    assert!(left < 256 * 1024, "3,000 refused joins left {left} bytes");
}

// W29
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cache_hit_never_waits_for_the_database() {
    let server = Server::start().await;
    server.view(COMMONS).await;
    server.get("/api/worlds").send().await.ok();
    let state = server.state.clone();
    let (held, held_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        state
            .db
            .in_transaction::<(), rusqlite::Error>(|_tx| {
                held.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(1500));
                Ok(())
            })
            .unwrap();
    });
    held_rx.recv().unwrap();
    // The database is held for a second and a half. These are all hits: the view and the list are
    // fresh, and so are the 304s and the pages that need nothing.
    let started = std::time::Instant::now();
    let tag = {
        let first = tokio::time::timeout(Duration::from_millis(900), server.get("/api/worlds/commons").send()).await;
        assert!(first.is_ok(), "a fresh view waited for the database");
        first.unwrap().headers.get("etag").unwrap().to_str().unwrap().to_string()
    };
    for (path, tag) in [("/api/worlds/commons", Some(tag.as_str())), ("/api/worlds", None), ("/robots.txt", None), ("/llms.txt", None), ("/agents.md", None)] {
        let mut request = server.get(path).ip("203.0.113.5");
        if let Some(tag) = tag {
            request = request.header("if-none-match", tag);
        }
        let reply = tokio::time::timeout(Duration::from_millis(900), request.send()).await;
        assert!(reply.is_ok(), "{path} waited for the database");
    }
    assert!(started.elapsed() < Duration::from_millis(1400), "the hits took {:?} while the database was held", started.elapsed());
    holder.join().unwrap();
}

// W29
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_hundred_parallel_views_of_a_stale_world_build_it_once() {
    let server = Server::start().await;
    server.view(COMMONS).await;
    let guest = server.guest().await;
    server.acts_ok(&guest.token, COMMONS, json!({ "action": "emote", "emote": "dance" })).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let builds = server.state.worlds.view_builds();
    let wire = server.wire();
    let mut tasks = Vec::new();
    for n in 0..200u32 {
        let wire = wire.clone();
        let ip = format!("10.7.{}.{}", n / 250, n % 250 + 1);
        tasks.push(tokio::spawn(async move { wire.send(Method::GET, "/api/worlds/commons", None, Some(&ip), None).await }));
    }
    let mut dancers = 0;
    for task in tasks {
        let (status, body) = task.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        if body["players"][0][4] == "dance" {
            dancers += 1;
        }
    }
    let built = server.state.worlds.view_builds() - builds;
    assert!(built <= 2, "{built} builds for 200 views");
    assert_eq!(dancers, 200, "everyone saw the change");
}

// W29, W32
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn measure_how_far_a_database_stall_reaches_pages_that_need_nothing() {
    let server = Server::start().await;
    server.view(COMMONS).await;
    let guest = server.guest().await;
    let state = server.state.clone();
    let (held, held_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        state
            .db
            .in_transaction::<(), rusqlite::Error>(|_tx| {
                held.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(2500));
                Ok(())
            })
            .unwrap();
    });
    held_rx.recv().unwrap();
    // Fifty callers who do need the database arrive, which is more than the server has threads.
    let wire = server.wire();
    let mut waiting = Vec::new();
    for _ in 0..300 {
        let (wire, token) = (wire.clone(), guest.token.clone());
        waiting.push(tokio::spawn(async move { wire.send(Method::GET, "/api/worlds/commons/me", Some(&token), None, None).await }));
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    // Someone else, from a different thread and runtime, asks for a page that is only text.
    let base = server.base.clone();
    let probe = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async move {
            let started = std::time::Instant::now();
            let reply = Client::new().get(format!("{base}/robots.txt")).timeout(Duration::from_secs(10)).send().await;
            (reply.map(|r| r.status()), started.elapsed())
        })
    });
    let (status, took) = probe.join().unwrap();
    println!("MEASURED: a page that is only text took {took:?} while 300 requests waited on a database held for 2.5 s (it takes about 2 ms when nothing is held)");
    holder.join().unwrap();
    for task in waiting {
        task.await.unwrap();
    }
    assert_eq!(status.unwrap(), StatusCode::OK);
}

/// Every kind of tile a world can hold in bulk, the old city's and the new places. A player has only
/// one house, so a world full of them is not a thing that can be made.
const ALL_KINDS: [Kind; 8] =
    [Kind::Homes, Kind::Shops, Kind::Workshops, Kind::Park, Kind::Road, Kind::Cafe, Kind::Club, Kind::Studio];

// W31
#[tokio::test]
async fn a_view_and_a_look_stay_a_fixed_size_however_full_the_world_and_the_street_are() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    // Every open tile built, and 300 players standing in the middle, put there directly.
    let owner = server.state.db.create_guest(&auth::hash_secret("owner-token"), &auth::hash_secret("owner-claim"), now()).unwrap().id as u64;
    let map = Map::from_db(&server, COMMONS);
    let mut changes = Vec::new();
    for y in 0..map.size {
        for x in 0..map.size {
            if map.open(x, y) {
                let kind = ALL_KINDS[((x * 7 + y * 3) % 8) as usize];
                changes.push(Change::Set((x as u16, y as u16), Tile { kind, level: ((x + y) % 4) as u8, player: Some(owner) }));
            }
        }
    }
    let built = changes.len();
    server.state.db.in_transaction::<(), rusqlite::Error>(|tx| tx.write_changes(COMMONS, &changes, now())).unwrap();
    // Three hundred players standing in the middle, made directly.
    for n in 0..300 {
        let player = server.state.db.create_guest(&auth::hash_secret(&format!("token-{n}")), &auth::hash_secret(&format!("claim-{n}")), now()).unwrap();
        server.state.db.in_transaction::<(), rusqlite::Error>(|tx| tx.enter_world(COMMONS, player.id, (64, 64), now())).unwrap();
    }
    let guests = [server.guest().await.token];
    // A crowd of crawlers and visitors too.
    for agent in [GOOGLEBOT, GPTBOT, "Mozilla/5.0 (compatible; bingbot/2.0)", "ClaudeBot/1.0", "PerplexityBot/1.0", "DuckDuckBot/1.0", "Applebot/0.1"] {
        server.get("/").ua(agent).send().await;
    }
    for n in 0..80 {
        server.get("/llms.txt").ua("curl/8.5").ip(&format!("198.51.100.{n}")).send().await;
    }
    let started = std::time::Instant::now();
    let view = server.fresh_view(COMMONS).await;
    let first_view = server.get("/api/worlds/commons").send().await;
    let size = first_view.text.len();
    println!("MEASURED: a full {built}-tile Commons view is {size} bytes ({:?} to build and fetch)", started.elapsed());
    assert!(view["players"].as_array().unwrap().len() <= 64);
    assert!(view["visitors"].as_array().unwrap().len() <= 30);
    assert!(view["crawlers"].as_array().unwrap().len() <= world::CRAWLERS.len());
    assert!(size < 1_500_000, "a view of a full world is {size} bytes");
    let look = server.tool("look", json!({ "player_token": guests[0], "x": 64, "y": 64, "w": 500, "h": 1000 })).await;
    assert_eq!((look["window"]["w"].as_i64(), look["window"]["h"].as_i64()), (Some(32), Some(32)), "a look is at most 32 by 32 whatever size is asked for");
    assert!(look["players"].as_array().unwrap().len() <= 64, "a look lists {} players", look["players"].as_array().unwrap().len());
    assert!(look["built"].as_array().unwrap().len() <= 32 * 32);
    assert!(look.to_string().len() < 80_000, "a look is {} bytes", look.to_string().len());
    let me = server.tool("how_am_i_doing", json!({ "player_token": guests[0] })).await;
    assert!(me.to_string().len() < 4_000);
}

// W19, W31, W32, W64
#[tokio::test]
async fn a_step_of_a_full_commons_is_quick_and_does_not_hold_the_database_long() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let owner = server.state.db.create_guest(&auth::hash_secret("owner-token"), &auth::hash_secret("owner-claim"), now()).unwrap().id as u64;
    let map = Map::from_db(&server, COMMONS);
    let mut changes = Vec::new();
    for y in 0..map.size {
        for x in 0..map.size {
            if map.open(x, y) {
                let kind = ALL_KINDS[((x * 7 + y * 3) % 8) as usize];
                changes.push(Change::Set((x as u16, y as u16), Tile { kind, level: ((x + y) % 4) as u8, player: Some(owner) }));
            }
        }
    }
    let built = changes.len();
    server.state.db.in_transaction::<(), rusqlite::Error>(|tx| tx.write_changes(COMMONS, &changes, now())).unwrap();
    // The length of the longest wait anyone in the database queue has, while steps are taken.
    let state = server.state.clone();
    let stepper = std::thread::spawn(move || {
        let mut longest = Duration::ZERO;
        for _ in 0..6 {
            let started = std::time::Instant::now();
            worlds::step_world(&state, COMMONS, now()).unwrap();
            longest = longest.max(started.elapsed());
        }
        longest
    });
    let longest = stepper.join().unwrap();
    println!("MEASURED: a step of a Commons with {built} tiles built takes up to {longest:?}, all of it with the database held");
    let _ = server.view(COMMONS).await;
    assert!(longest < Duration::from_millis(1500), "a step holds the database for {longest:?}");
    // And a view of it, built while the roads number in the thousands.
    server.state.worlds.mark_stale(COMMONS);
    for agent in [GOOGLEBOT, GPTBOT, "ClaudeBot/1.0", "PerplexityBot/1.0", "DuckDuckBot/1.0", "Applebot/0.1", "bingbot/2.0", "YandexBot/3", "Baiduspider/2", "AhrefsBot/7", "SemrushBot/7", "Bytespider", "CCBot/2", "Amazonbot/0.1", "PetalBot", "MJ12bot/1.4", "OAI-SearchBot/1", "meta-externalagent/1"] {
        server.get("/").ua(agent).send().await;
    }
    for n in 0..60 {
        server.get("/llms.txt").ua("curl/8.5").ip(&format!("198.51.100.{n}")).send().await;
    }
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let started = std::time::Instant::now();
    let reply = server.get("/api/worlds/commons").send().await;
    let build = started.elapsed();
    println!("MEASURED: building and fetching the view of that Commons with 18 frogs and 30 visitors took {build:?} ({} bytes)", reply.text.len());
    assert!(build < Duration::from_millis(3000), "a view of a world full of roads takes {build:?} to build");
}

// W29
#[tokio::test]
async fn the_list_of_worlds_is_cheap_even_when_every_world_is_full() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let mut changes = Vec::new();
    for y in 0..64u16 {
        for x in 0..64u16 {
            changes.push(Change::Set((x, y), Tile { kind: Kind::Homes, level: 1, player: Some(1) }));
        }
    }
    for n in 0..60 {
        let id = format!("full-{n:03}");
        server
            .state
            .db
            .insert_world(
                &agenthangout::db::NewWorld {
                    id: &id,
                    name: "Full",
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
        server.state.db.in_transaction::<(), rusqlite::Error>(|tx| tx.write_changes(&id, &changes, now())).unwrap();
    }
    let started = std::time::Instant::now();
    let reply = server.get("/api/worlds").send().await.ok();
    let build = started.elapsed();
    println!("MEASURED: the list of the busiest 50 worlds, each with 4096 tiles, took {build:?}");
    assert_eq!(reply["worlds"].as_array().unwrap().len(), 50);
    assert!(build < Duration::from_millis(1500), "{build:?}");
}

// W29
#[tokio::test]
async fn views_of_worlds_that_are_not_there_are_counted_against_the_address_and_not_looked_up_for_free() {
    start_early_in(60, 30).await;
    let server = Server::start().await;
    server.view(COMMONS).await;
    let mut refused = 0;
    for n in 0..300 {
        let reply = server.get(&format!("/api/worlds/not-a-world-{n}")).ip("203.0.113.61").send().await;
        if reply.status == StatusCode::TOO_MANY_REQUESTS {
            refused += 1;
        }
    }
    assert!(refused >= 50, "only {refused} of 300 looks at worlds that don't exist were refused");
}

// ----------------------------------------------------------------------------------------------
// Griefing (W15, W17)
// ----------------------------------------------------------------------------------------------

fn cheb(a: (i64, i64), b: (i64, i64)) -> i64 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

/// A road that leaves the highway and goes `len` tiles, and open tiles beside it that are more than
/// three tiles from the highway, so a place there has a road only because of this one.
struct Branch {
    stub: Vec<(i64, i64)>,
    dependants: Vec<(i64, i64)>,
}

fn find_branch(map: &Map, len: i64, wanted: usize) -> Option<Branch> {
    let highway = map.roads();
    for &(rx, ry) in &highway {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let stub: Vec<(i64, i64)> = (1..=len).map(|i| (rx + dx * i, ry + dy * i)).collect();
            if !stub.iter().all(|&(x, y)| map.open(x, y)) {
                continue;
            }
            let mut dependants: Vec<(i64, i64)> = Vec::new();
            for &(sx, sy) in &stub {
                for (nx, ny) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let at = (sx + nx, sy + ny);
                    if !map.free(at.0, at.1) || stub.contains(&at) || dependants.contains(&at) || highway.iter().any(|&h| cheb(h, at) <= 3) {
                        continue;
                    }
                    dependants.push(at);
                }
            }
            if dependants.len() >= wanted {
                return Some(Branch { stub, dependants });
            }
        }
    }
    None
}

/// How many of these tiles still have a place on them.
fn standing(server: &Server, tiles: &[(i64, i64)]) -> usize {
    let built = server.tiles(COMMONS);
    tiles.iter().filter(|&&(x, y)| built.get(&(x as u16, y as u16)).is_some_and(|tile| tile.kind.is_building())).count()
}

/// A street of houses along the far end of a branch, each put up by a person of their own, because
/// there is a road there.
async fn house_a_street(server: &Server, branch: &Branch, houses: usize) -> Vec<(i64, i64)> {
    let mut housed = Vec::new();
    for (n, &spot) in branch.dependants.iter().take(houses).enumerate() {
        let neighbour = server.person(&format!("Neighbour{n}")).await;
        let reply = server.place(&neighbour, "house", spot).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.show());
        housed.push(spot);
    }
    assert_eq!(housed.len(), houses, "the branch has room for too few houses: the test has nothing to protect");
    housed
}

// W15, W17
#[tokio::test]
async fn one_move_that_cuts_the_road_a_whole_street_hangs_on_does_not_wreck_it() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let map = Map::from_db(&server, COMMONS);
    let branch = find_branch(&map, 8, 6).expect("a place for a branch");
    let attacker = server.guest().await;
    // The attacker lays a road that looks like a good place to build, which costs 8 moves.
    let laid = server.acts_ok(&attacker.token, COMMONS, road_move(branch.stub[0], *branch.stub.last().unwrap())).await;
    assert_eq!(laid["cost"], 8);
    // People put up houses on it, because it is a road.
    let housed = house_a_street(&server, &branch, 6).await;
    // One move, a bulldoze of the tile beside the highway, which the attacker built.
    let cut = server.act(&attacker.token, COMMONS, json!({ "action": "bulldoze", "x": branch.stub[0].0, "y": branch.stub[0].1 })).await;
    let moves = 30 - server.moves_left(&attacker.token).await;
    // W17: a road a place hangs on can't be taken down, and saying no costs nothing.
    assert_eq!(cut.status, StatusCode::BAD_REQUEST, "{}", cut.show());
    assert!(cut.text.contains("depends on that road"), "{}", cut.text);
    assert_eq!(moves, 8, "only the road was paid for");
    for _ in 0..6 {
        worlds::step_all(&server.state, now());
    }
    let after = standing(&server, &housed);
    println!("COST REPORT: {moves} moves by one guest could not take down a street of {} houses (the cut was {})", housed.len(), cut.status);
    assert_eq!(after, housed.len(), "{moves} moves by a stranger took a street of houses down to {after}: {}", cut.show());
    assert!(server.tiles(COMMONS).contains_key(&(branch.stub[0].0 as u16, branch.stub[0].1 as u16)), "the road came down");
}

// W15, W17
#[tokio::test]
async fn the_road_a_builder_left_behind_a_week_ago_cannot_be_cut_by_a_stranger_under_a_street_of_houses() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let map = Map::from_db(&server, COMMONS);
    let branch = find_branch(&map, 8, 6).expect("a place for a branch");
    let builder = server.guest().await;
    let builder_id = server.player_id(&builder.token).await;
    let stranger = server.guest().await;
    server.acts_ok(&builder.token, COMMONS, road_move(branch.stub[0], *branch.stub.last().unwrap())).await;
    let housed = house_a_street(&server, &branch, 6).await;
    // The builder wandered off a week and a bit ago, as most guests do.
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| tx.charge(builder_id, 0, 0, now() - 7 * 86_400 - 60))
        .unwrap();
    let cut = server.act(&stranger.token, COMMONS, json!({ "action": "bulldoze", "x": branch.stub[0].0, "y": branch.stub[0].1 })).await;
    // W17: nobody can, a builder who has gone for a week included.
    assert_eq!(cut.status, StatusCode::BAD_REQUEST, "{}", cut.show());
    assert!(cut.text.contains("depends on that road"), "{}", cut.text);
    for _ in 0..6 {
        worlds::step_all(&server.state, now());
    }
    let after = standing(&server, &housed);
    println!("COST REPORT: one move by a stranger could not take down a street of {} houses (the cut was {})", housed.len(), cut.status);
    assert_eq!(after, housed.len(), "one move by a stranger took a street of houses down to {after}: {}", cut.show());
    assert!(server.tiles(COMMONS).contains_key(&(branch.stub[0].0 as u16, branch.stub[0].1 as u16)), "the road came down");
}

// W15, W17, W55
#[tokio::test]
async fn a_crowd_of_people_putting_up_houses_cannot_seal_off_the_highway_for_everyone_else() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let map = Map::from_db(&server, COMMONS);
    // The highway proper, as the server lays it.
    let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
    let highway: Vec<(i64, i64)> = world::highway(world::COMMONS_SEED, &land).iter().map(|&(x, y)| (i64::from(x), i64::from(y))).collect();
    let mut frontage: Vec<(i64, i64)> = Vec::new();
    for &(rx, ry) in &highway {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let at = (rx + dx, ry + dy);
            if map.open(at.0, at.1) && !frontage.contains(&at) {
                frontage.push(at);
            }
        }
    }
    let kept = frontage.iter().filter(|at| map.kept.contains(at)).count();
    assert!(kept >= frontage.len() / 8, "{kept} of {} tiles beside the highway are kept for side streets", frontage.len());

    // People are free to start, and each may have a house. A crowd of them puts every house on a
    // tile beside the highway, the way a crowd of zones used to seal it, and tries the kept tiles
    // as well. A kept tile is turned away, and turning it away costs nothing.
    let crowd = 40;
    let prober = server.person("Prober").await;
    let (mut housed, mut refused) = (0, 0);
    for &(x, y) in &frontage {
        if map.kept.contains(&(x, y)) {
            if refused < 12 {
                let reply = server.place(&prober, "house", (x, y)).await;
                assert_eq!(reply.status, StatusCode::BAD_REQUEST, "({x}, {y}): {}", reply.show());
                assert!(reply.text.contains("side street"), "({x}, {y}) was refused for another reason: {}", reply.show());
                refused += 1;
            }
            continue;
        }
        if housed < crowd {
            let neighbour = server.person(&format!("Builder{housed}")).await;
            server.place(&neighbour, "house", (x, y)).await.ok();
            housed += 1;
        }
    }
    assert_eq!(housed, crowd, "the highway has too little frontage for the test");
    assert!(refused > 0);
    assert_eq!(server.moves_left(&prober.key).await, 300, "refused moves were charged");

    // Every kept tile is still open, for anyone to start a road from.
    let after = Map::from_db(&server, COMMONS);
    let starts: Vec<(i64, i64)> = frontage.iter().copied().filter(|&(x, y)| after.kept.contains(&(x, y)) && after.open(x, y)).collect();
    println!(
        "COST REPORT: the highway has {} tiles and {} open tiles beside it; {kept} are kept for side streets. {crowd} people put a house each on tiles beside it, were turned away from {refused} kept ones, and left {} places a road could start",
        highway.len(),
        frontage.len(),
        starts.len(),
    );
    assert_eq!(starts.len(), kept, "a kept tile was built on");
    assert!(!starts.is_empty(), "{crowd} houses left no way for anyone else to start a road: the highway is walled in and The Commons can't grow");
    // Someone else lays a road out of one of them, straight out from the highway for three tiles.
    let honest = server.guest().await;
    let leave = starts.iter().find_map(|&(x, y)| {
        let (rx, ry) = highway.iter().copied().find(|&(rx, ry)| (rx - x).abs() + (ry - y).abs() == 1)?;
        let (dx, dy) = (x - rx, y - ry);
        let out = [(x, y), (x + dx, y + dy), (x + 2 * dx, y + 2 * dy)];
        out.iter().all(|&(ox, oy)| after.open(ox, oy)).then_some((out[0], out[2]))
    });
    let (first, last) = leave.expect("a side street left open for a road");
    let laid = server.act(&honest.token, COMMONS, road_move(first, last)).await;
    assert_eq!(laid.status, StatusCode::OK, "{}", laid.show());
    assert_eq!(laid.body["cost"], 3);
}

/// Places that a road joined to the highway is within reach of, worked out the long way.
fn served_the_long_way(tiles: &world::Tiles) -> HashSet<(u16, u16)> {
    let mut joined: HashSet<(u16, u16)> =
        tiles.iter().filter(|(_, t)| t.kind == Kind::Road && t.player.is_none()).map(|(pos, _)| *pos).collect();
    let mut queue: Vec<(u16, u16)> = joined.iter().copied().collect();
    while let Some((x, y)) = queue.pop() {
        for next in [(x.wrapping_add(1), y), (x.wrapping_sub(1), y), (x, y.wrapping_add(1)), (x, y.wrapping_sub(1))] {
            if tiles.get(&next).is_some_and(|t| t.kind == Kind::Road) && joined.insert(next) {
                queue.push(next);
            }
        }
    }
    tiles
        .iter()
        .filter(|(_, t)| t.kind.is_building())
        .filter(|(pos, _)| joined.iter().any(|j| j.0.abs_diff(pos.0) <= 3 && j.1.abs_diff(pos.1) <= 3))
        .map(|(pos, _)| *pos)
        .collect()
}

// W17
#[tokio::test]
async fn a_branch_a_street_of_houses_hangs_on_can_be_cut_nowhere_that_leaves_a_place_without_a_road() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let map = Map::from_db(&server, COMMONS);
    let branch = find_branch(&map, 8, 6).expect("a place for a branch");
    let builder = server.guest().await;
    let end = *branch.stub.last().unwrap();
    server.acts_ok(&builder.token, COMMONS, road_move(branch.stub[0], end)).await;
    // Nothing is built beside it yet, so the far end is a dead end that nothing depends on: it comes
    // down for its builder, and goes back up.
    server.acts_ok(&builder.token, COMMONS, json!({ "action": "bulldoze", "x": end.0, "y": end.1 })).await;
    server.acts_ok(&builder.token, COMMONS, road_move(end, end)).await;
    let housed = house_a_street(&server, &branch, 6).await;
    // Now each tile of the branch comes down only if no place would be left without a road, whether
    // it is the tile beside a place or one far up the branch, by the builder or by a stranger. Every
    // refusal costs nothing, and every tile that does come down is put back.
    let before = server.tiles(COMMONS);
    let stranger = server.guest().await;
    let (mut refused, mut allowed) = (0, 0);
    for &(x, y) in &branch.stub {
        let mut without = before.clone();
        without.remove(&(x as u16, y as u16));
        let cuts = !served_the_long_way(&before).is_subset(&served_the_long_way(&without));
        let down = json!({ "action": "bulldoze", "x": x, "y": y });
        let by_stranger = server.act(&stranger.token, COMMONS, down.clone()).await;
        assert_eq!(by_stranger.status, StatusCode::BAD_REQUEST, "a stranger took down a builder's road: {}", by_stranger.show());
        let reply = server.act(&builder.token, COMMONS, down).await;
        if cuts {
            refused += 1;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "({x}, {y}) cut a place off and came down: {}", reply.show());
            assert!(reply.text.contains("depends on that road"), "{}", reply.text);
        } else {
            allowed += 1;
            assert_eq!(reply.status, StatusCode::OK, "({x}, {y}) cut nothing off and was refused: {}", reply.show());
            server.acts_ok(&builder.token, COMMONS, road_move((x, y), (x, y))).await;
        }
        assert_eq!(server.tiles(COMMONS), before, "({x}, {y})");
    }
    println!("COST REPORT: of {} tiles of a branch beside {} houses, {refused} could not come down and {allowed} could", branch.stub.len(), housed.len());
    assert!(refused >= 4, "only {refused} of the branch was protected");
    assert_eq!(server.moves_left(&stranger.token).await, 30, "a refusal cost something");
}

// W16, W55
#[tokio::test]
async fn a_road_cannot_go_through_a_place_not_even_its_owners() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let spot = Map::from_db(&server, COMMONS).house_spots()[0];
    let ann = server.housed("Ann", spot).await;
    let bo = server.guest().await;
    let at = (spot.0 as u16, spot.1 as u16);
    let left = server.moves_left(&ann.key).await;
    assert_eq!(left, 299);
    let ends = [(spot.0 + 2, spot.1), (spot.0, spot.1 + 2), (spot.0 - 2, spot.1), (spot.0, spot.1 - 2), spot];
    for end in ends {
        let road = road_move(spot, end);
        let by_owner = server.act(&ann.key, COMMONS, road.clone()).await;
        assert!(by_owner.status.is_client_error(), "a road went through the owner's own house: {}", by_owner.show());
        let by_stranger = server.act(&bo.token, COMMONS, road).await;
        assert!(by_stranger.status.is_client_error(), "a road went through a house: {}", by_stranger.show());
    }
    assert_eq!(server.tiles(COMMONS)[&at].kind, Kind::House);
    assert_eq!(server.moves_left(&ann.key).await, left, "a refused road cost something");
    assert_eq!(server.moves_left(&bo.token).await, 30);
}

// W17
#[tokio::test]
async fn the_highway_and_the_middle_of_the_commons_can_never_be_taken_down_by_anyone() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    let gone = server.guest().await;
    let gone_id = server.player_id(&gone.token).await;
    let map = Map::from_db(&server, COMMONS);
    // Nobody has been seen for a year, and nobody built the highway.
    let long_ago = now() - 365 * 86_400;
    server.state.db.in_transaction::<(), rusqlite::Error>(|tx| tx.charge(gone_id, 0, 0, long_ago)).unwrap();
    let before = server.tiles(COMMONS);
    let centre = (map.size / 2, map.size / 2);
    let mut targets = map.roads();
    targets.push(centre);
    for (x, y) in targets {
        for token in [&ann.key, &guest.token] {
            let reply = server.act(token, COMMONS, json!({ "action": "bulldoze", "x": x, "y": y })).await;
            assert!(reply.status.is_client_error(), "({x}, {y}) was taken down: {}", reply.show());
            let via_mcp = server.call("bulldoze", json!({ "player_token": guest.token, "x": x, "y": y })).await;
            assert_eq!(via_mcp["isError"], true);
        }
    }
    assert_eq!(server.tiles(COMMONS), before);
    // The centre is a road with no builder, so it can't be built over either.
    let over = server.act(&ann.key, COMMONS, place_move("house", centre)).await;
    assert!(over.status.is_client_error());
    // A bulldoze of a tile that was cleared forest does not take the highway under it.
    assert_eq!(server.tiles(COMMONS), before);
}

// W17, W57
#[tokio::test]
async fn nobody_can_bulldoze_a_place_or_a_tile_of_the_old_city_even_when_its_builder_is_long_gone() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let ann = server.person("Ann").await;
    let spots = Map::from_db(&server, COMMONS).house_spots();
    let owner = server.housed("Owen", spots[0]).await;
    let owner_id = server.state.db.own_player(&owner.id, now()).unwrap().id;
    let builder = server.guest().await;
    let builder_id = server.player_id(&builder.token).await;
    // A guest's road, which is theirs only as long as they stay.
    let (road, _) = Map::from_db(&server, COMMONS).line_from_road(1).unwrap();
    server.acts_ok(&builder.token, COMMONS, road_move(road, road)).await;
    // A tile of the old city that grew, built by the same guest.
    let old = (4..60u16).map(|x| (x, 6u16)).find(|pos| !server.tiles(COMMONS).contains_key(pos)).unwrap();
    server.write(COMMONS, &[Change::Set(old, Tile { kind: Kind::Homes, level: 1, player: Some(builder_id as u64) })]);
    // Both of them gone for a hundred days.
    server
        .state
        .db
        .in_transaction::<(), rusqlite::Error>(|tx| {
            tx.charge(owner_id, 0, 0, now() - 100 * 86_400)?;
            tx.charge(builder_id, 0, 0, now() - 100 * 86_400)
        })
        .unwrap();
    let down = |at: (i64, i64)| json!({ "action": "bulldoze", "x": at.0, "y": at.1 });
    let gone = server.act(&ann.key, COMMONS, down(road)).await;
    assert_eq!(gone.status, StatusCode::OK, "a guest's road is anyone's after a hundred days: {}", gone.show());
    let house = (spots[0].0, spots[0].1);
    let kept =[house, (i64::from(old.0), i64::from(old.1))];
    for &(x, y) in &kept {
        let reply = server.act(&ann.key, COMMONS, down((x, y))).await;
        assert!(reply.status.is_client_error(), "({x}, {y}) was taken down: {}", reply.show());
    }
    // A guest who was cleared away entirely leaves the same: still nobody can.
    worlds::housekeeping(&server.state, now() + 40 * 86_400);
    for &(x, y) in &kept {
        let reply = server.act(&ann.key, COMMONS, down((x, y))).await;
        assert!(reply.status.is_client_error(), "({x}, {y}) was taken down after a clean-up: {}", reply.show());
    }
    let tiles = server.tiles(COMMONS);
    assert_eq!(tiles[&(spots[0].0 as u16, spots[0].1 as u16)].kind, Kind::House);
    assert_eq!(tiles[&old].kind, Kind::Homes);
}

// W16
#[tokio::test]
async fn skipped_tiles_are_never_charged_and_a_move_costs_exactly_what_it_changes() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let ann = server.person("Ann").await;
    let ann_id = server.state.db.own_player(&ann.id, now()).unwrap().id as u64;
    let map = Map::from_db(&server, COMMONS);
    let mut rng = Rng(77);
    let mut expected_left = 300;
    let (mut checked, mut refused) = (0, 0);
    for _ in 0..200 {
        // Too many refused moves in a minute are turned away at the door (W30), and the moves of the
        // day run out, so stop before either gets in the way.
        if refused >= 50 || expected_left < 8 {
            break;
        }
        let before = server.tiles(COMMONS);
        let mut roads: Vec<(u16, u16)> = before.iter().filter(|(_, tile)| tile.kind == Kind::Road).map(|(pos, _)| *pos).collect();
        roads.sort();
        let mut own: Vec<(u16, u16)> = before.iter().filter(|(_, tile)| tile.player == Some(ann_id)).map(|(pos, _)| *pos).collect();
        own.sort();
        let base = roads[rng.below(roads.len() as u64) as usize];
        let (dx, dy) = [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)][rng.below(4) as usize];
        let len = rng.below(4) as i64;
        let (x, y) = (i64::from(base.0) + dx, i64::from(base.1) + dy);
        let body = match rng.below(4) {
            0 => json!({ "action": "place", "kind": rng.pick(&["house", "studio", "club", "cafe"]), "x": x, "y": y }),
            1 if !own.is_empty() => {
                let at = own[rng.below(own.len() as u64) as usize];
                json!({ "action": "bulldoze", "x": at.0, "y": at.1 })
            }
            1 => json!({ "action": "bulldoze", "x": x, "y": y }),
            _ => json!({ "action": "road", "x": x, "y": y, "x2": x + dx * len, "y2": y + dy * len }),
        };
        let reply = server.act(&ann.key, COMMONS, body.clone()).await;
        let after = server.tiles(COMMONS);
        if reply.status == StatusCode::OK {
            let cost = reply.body["cost"].as_i64().unwrap();
            let changed = reply.body["changed"].as_i64().unwrap();
            let skipped = reply.body["skipped"].as_i64().unwrap();
            assert_eq!(cost, changed, "{body}: cost {cost} for {changed} changed");
            let differing = before.keys().chain(after.keys()).collect::<HashSet<_>>().into_iter().filter(|pos| before.get(*pos) != after.get(*pos)).count() as i64;
            assert_eq!(differing, changed, "{body}: {changed} said, {differing} really changed");
            expected_left -= cost;
            assert_eq!(reply.body["moves_left"], expected_left, "{body}");
            assert!(skipped >= 0);
            checked += 1;
        } else {
            assert_eq!(before, after, "{body} was refused but changed tiles");
            refused += 1;
        }
        assert_eq!(server.moves_left(&ann.key).await, expected_left, "{body}: {}", reply.show());
    }
    assert!(checked > 20, "too few moves worked ({checked}) to say anything");
    let tiles = server.tiles(COMMONS);
    // Nothing is built on water or a hill, and a tile of forest is only ever `cleared` or built on after.
    for ((x, y), tile) in &tiles {
        if tile.player.is_some() {
            let ground = map.ground(i64::from(*x), i64::from(*y)).unwrap();
            assert!(matches!(ground, 'g' | 's' | 'f'), "{:?} built on {ground}", tile.kind);
        }
    }
}

// ----------------------------------------------------------------------------------------------
// Worlds people started, which are frozen (W18, W27)
// ----------------------------------------------------------------------------------------------

// W18
#[tokio::test]
async fn the_keeper_is_the_keeper_of_the_commons_and_not_of_anyones_own_world() {
    let (server, keeper) = Server::start_with_keeper().await;
    let ann = server.person("Ann").await;
    let (world, _) = server.frozen_world(&ann, true).await;
    let guest = server.guest().await;
    let id = server.player_id(&guest.token).await;
    server.leave_in(&world, id);
    let reply = server.post(&format!("/api/worlds/{world}/players/{id}/remove"), json!({})).token(&keeper.key).send().await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.show());
    assert_eq!(server.tiles(&world).values().filter(|tile| tile.player == Some(id as u64)).count(), 2, "nothing came down");
    // The owner of a frozen world can still take a player out of it.
    let reply = server.post(&format!("/api/worlds/{world}/players/{id}/remove"), json!({})).token(&ann.key).send().await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.show());
    assert_eq!(server.tiles(&world).values().filter(|tile| tile.player == Some(id as u64)).count(), 0);
}

// W18
#[tokio::test]
async fn an_owner_cannot_remove_a_player_who_has_nothing_to_do_with_their_world() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let (ann_world, _) = server.frozen_world(&ann, true).await;
    let (bo_world, _) = server.frozen_world(&bo, true).await;
    let stranger = server.guest().await;
    let bos_guest = server.guest().await;
    let stranger_id = server.player_id(&stranger.token).await;
    let bos_id = server.player_id(&bos_guest.token).await;
    server.leave_in(&bo_world, bos_id);
    server.acts_ok(&stranger.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    // A player who has never been in Ann's world, and one who has only been in Bo's.
    for id in [stranger_id, bos_id] {
        let reply = server.post(&format!("/api/worlds/{ann_world}/players/{id}/remove"), json!({})).token(&ann.key).send().await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.show());
    }
    // Nothing happened to either of them, there or anywhere.
    assert_eq!(server.act(&stranger.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.status, StatusCode::OK);
    assert_eq!(server.tiles(&bo_world).values().filter(|tile| tile.player == Some(bos_id as u64)).count(), 2);
}

// W27
#[tokio::test]
async fn nobody_acts_in_a_frozen_world_open_or_closed_whoever_they_are_and_whatever_link_they_hold() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let guest = server.guest().await;
    let bot = server.bot("Scout").await;
    let bot_player = server.bot_player(&bot).await;
    let open = server.frozen_world(&ann, true).await;
    let closed = server.frozen_world(&bo, false).await;
    for (world, code) in [&open, &closed] {
        let (first, last) = Map::from_db(&server, world).line_from_road(2).unwrap();
        let spot = Map::from_db(&server, world).house_spots()[0];
        for token in [&ann.key, &bo.key, &guest.token, &bot_player] {
            // The link first, as the owner and as a stranger: it lets nobody in.
            let link = server.post(&format!("/api/worlds/join/{code}"), json!({})).token(token).send().await;
            assert_eq!(link.status, StatusCode::FORBIDDEN, "{}", link.show());
            for body in [
                road_move(first, last),
                json!({ "action": "emote", "emote": "wave" }),
                json!({ "action": "walk", "x": 33, "y": 32 }),
                json!({ "action": "bulldoze", "x": first.0, "y": first.1 }),
                place_move("house", spot),
            ] {
                let reply = server.act(token, world, body.clone()).await;
                assert_eq!(reply.status, StatusCode::FORBIDDEN, "{body} in {world}: {}", reply.show());
            }
        }
        // Joining it by its id is refused too, and so is looking for its link by guessing.
        let joined = server.post("/api/public/join", json!({ "world_id": world })).ip(&server.fresh_ip()).send().await;
        assert_eq!(joined.status, StatusCode::FORBIDDEN, "{}", joined.show());
        assert_eq!(server.tiles(world).values().filter(|tile| tile.player.is_some()).count(), 0, "something was built in {world}");
        assert_eq!(server.fresh_view(world).await["players"], json!([]));
    }
    // Watching is allowed for everyone, and says what the world is.
    assert_eq!(server.view(&open.0).await["kind"], "own");
}

// W27
#[tokio::test]
async fn no_tool_acts_in_a_frozen_world_and_none_gives_away_its_link() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let (world, code) = server.frozen_world(&ann, false).await;
    let guest = server.tool("join", json!({})).await["player_token"].as_str().unwrap().to_string();
    let map = Map::from_db(&server, &world);
    let (first, last) = map.line_from_road(2).unwrap();
    let left = server.moves_left(&guest).await;
    for (name, args) in [
        ("build", json!({ "player_token": guest, "world_id": world, "type": "road", "x": first.0, "y": first.1, "x2": last.0, "y2": last.1 })),
        ("build", json!({ "player_token": guest, "world_id": world, "type": "homes", "x": first.0, "y": first.1 })),
        ("build", json!({ "player_token": guest, "world_id": world, "type": "park", "x": first.0, "y": first.1 })),
        ("bulldoze", json!({ "player_token": guest, "world_id": world, "x": first.0, "y": first.1 })),
        ("walk", json!({ "player_token": guest, "world_id": world, "x": 33, "y": 32 })),
        ("emote", json!({ "player_token": guest, "world_id": world, "emote": "wave" })),
        ("join", json!({ "world_id": world })),
    ] {
        let message = server.tool_refused(name, args).await;
        assert!(!message.contains(&code) && !message.contains(&world), "{message}");
    }
    assert_eq!(server.moves_left(&guest).await, left);
    assert_eq!(server.tiles(&world).values().filter(|tile| tile.player.is_some()).count(), 0);
    // Watching is allowed for everyone.
    server.tool("look", json!({ "world_id": world })).await;
    server.tool("look", json!({ "world_id": world, "player_token": guest })).await;
    // And the link, opened through the route, lets the same tools in no further.
    let link = server.post(&format!("/api/worlds/join/{code}"), json!({})).token(&guest).send().await;
    assert_eq!(link.status, StatusCode::FORBIDDEN, "{}", link.show());
    server
        .tool_refused("build", json!({ "player_token": guest, "world_id": world, "type": "road", "x": first.0, "y": first.1, "x2": last.0, "y2": last.1 }))
        .await;
    assert_eq!(server.tiles(&world).values().filter(|tile| tile.player.is_some()).count(), 0);
}

// W27
#[tokio::test]
async fn guessing_links_is_slow_and_costs_nothing_but_the_guess() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let mut limited = 0;
    for n in 0..120 {
        let reply = server.post(&format!("/api/worlds/join/guess{n:020}"), json!({})).token(&guest.token).send().await;
        match reply.status {
            StatusCode::NOT_FOUND => {}
            StatusCode::TOO_MANY_REQUESTS => limited += 1,
            other => panic!("{other}: {}", reply.text),
        }
    }
    assert!(limited >= 50, "{limited} of 120 guesses were slowed down");
    assert_eq!(server.moves_left(&guest.token).await, 30);
}

// W28
#[tokio::test]
async fn the_engine_makes_land_and_a_highway_for_thousands_of_seeds_without_panicking_and_quickly() {
    // The largest seed a world was ever started with, and the largest a browser can write exactly.
    const MAX_SEED: u64 = (1 << 53) - 1;
    let started = std::time::Instant::now();
    let mut slowest = Duration::ZERO;
    let mut rng = Rng(0xFEED);
    let mut seeds: Vec<u64> = (0..300).collect();
    for _ in 0..300 {
        seeds.push(rng.next() & MAX_SEED);
    }
    seeds.extend([MAX_SEED, MAX_SEED - 1, 1 << 32, (1 << 32) - 1, 1 << 31]);
    for seed in seeds {
        let at = std::time::Instant::now();
        let land = world::terrain(seed, world::OWN_SIZE);
        let highway = world::highway(seed, &land);
        slowest = slowest.max(at.elapsed());
        assert_eq!(land.size(), 64);
        assert!(highway.len() >= 20, "seed {seed}: a highway of {} tiles", highway.len());
        assert_eq!(*highway.last().unwrap(), (32, 32), "seed {seed}");
        for pair in highway.windows(2) {
            assert_eq!(pair[0].0.abs_diff(pair[1].0) + pair[0].1.abs_diff(pair[1].1), 1, "seed {seed}: not a joined road");
        }
        assert!(highway.iter().all(|&pos| land.at(pos).is_some_and(|g| g.is_open())), "seed {seed}: a highway over something that isn't open ground");
        let rows = world::encode_land(&land);
        assert_eq!(world::decode_land(&rows).unwrap(), land);
    }
    println!("MEASURED: 600 seeds took {:?}, the slowest {slowest:?} (a debug build)", started.elapsed());
}
/// A road and a house beside it, built by a person: what a step has on The Commons to leave alone.
async fn lay_out_a_town(server: &Server, ann: &Who) {
    let (first, last) = Map::from_db(server, COMMONS).line_from_road(3).unwrap();
    server.acts_ok(&ann.key, COMMONS, road_move(first, last)).await;
    let spot = Map::from_db(server, COMMONS).house_spots()[0];
    server.acts_ok(&ann.key, COMMONS, place_move("house", spot)).await;
}

/// What the keeper's asking does (W64).
fn ask_for_a_disaster(server: &Server) {
    server.state.db.in_transaction::<(), rusqlite::Error>(|tx| tx.start_disaster(COMMONS, 0)).unwrap();
}

// ----------------------------------------------------------------------------------------------
// Crawlers and visitors (W34-W45), and a page on another site
// ----------------------------------------------------------------------------------------------

fn headers_with(agent: &str, address: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(value) = agent.parse() {
        headers.insert("user-agent", value);
    }
    if let Some(address) = address {
        headers.insert("x-forwarded-for", address.parse().unwrap());
    }
    headers
}

// W37, W43, W64
#[tokio::test]
async fn crawlers_and_visitors_change_nothing_about_the_world_or_its_clock() {
    async fn run(traffic: bool, disaster: bool) -> (world::Tiles, u64, Option<(u64, Option<u64>, Option<u64>)>) {
        let server = Server::start().await;
        let ann = server.person("Ann").await;
        lay_out_a_town(&server, &ann).await;
        if disaster {
            ask_for_a_disaster(&server);
        }
        let t = now();
        for round in 0..120 {
            if traffic {
                for (n, agent) in [GOOGLEBOT, GPTBOT, CHATGPT_USER, GEMINI, "curl/8.5"].iter().enumerate() {
                    let headers = headers_with(agent, Some(&format!("198.51.100.{}", (round * 5 + n) % 250)));
                    for path in ["/", "/worlds", "/world/commons", "/llms.txt", "/agents.md"] {
                        worlds::note_request(&server.state, &Method::GET, path, &headers, t + round as i64);
                    }
                    worlds::note_request(&server.state, &Method::POST, "/public/mcp", &headers, t + round as i64);
                }
                server.get("/").ua(GOOGLEBOT).send().await;
                server.get("/llms.txt").ua("curl/8.5").ip("203.0.113.9").send().await;
            }
            worlds::step_all(&server.state, now());
        }
        let row = server.state.db.world(COMMONS).unwrap().unwrap();
        let end = server.state.db.world_end(COMMONS).unwrap().map(|end| (end.clock_step, end.omen_step, end.ended_step));
        (server.tiles(COMMONS), row.step, end)
    }
    let (quiet, quiet_step, quiet_end) = run(false, false).await;
    let (busy, busy_step, busy_end) = run(true, false).await;
    assert_eq!(quiet_step, busy_step);
    assert_eq!(quiet, busy, "frogs and visitors changed the tiles");
    assert_eq!((quiet_end, busy_end), (None, None), "traffic started a disaster");
    assert!(quiet.values().any(|tile| tile.kind.is_building()), "the house is gone");

    // With a disaster under way, 120 steps see it through, the same way with or without them.
    let (quiet, quiet_step, quiet_end) = run(false, true).await;
    let (busy, busy_step, busy_end) = run(true, true).await;
    assert_eq!(quiet_step, busy_step);
    assert_eq!(quiet, busy, "frogs and visitors changed what the disaster took");
    assert_eq!(quiet_end, busy_end, "frogs and visitors changed when the disaster ended");
    assert!(quiet_end.is_some_and(|(_, _, ended)| ended.is_some()), "the disaster never ended");
}

// W24, W27, W37, W43
#[tokio::test]
async fn only_the_commons_steps_and_nothing_in_or_near_a_frozen_world_makes_it_step() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    // A world someone started, which is frozen: The Commons is the only one that steps (W24).
    let (own, _) = server.frozen_world(&ann, true).await;
    let t0 = now();
    let asleep = t0 + 31 * 86_400;
    let stepped = || worlds::step_all(&server.state, asleep).stepped;
    // A month on with nobody having moved, The Commons still steps, and the frozen world does not.
    let first = stepped();
    assert!(first.contains(&COMMONS.to_string()), "The Commons stopped: {first:?}");
    assert!(!first.contains(&own), "a frozen world was stepped: {first:?}");
    // Everything else that happens in or near a world.
    let guest_headers = headers_with("", Some("198.51.100.20"));
    let joined = worlds::join(&server.state, &guest_headers, None, asleep).unwrap();
    for (agent, path) in [(GOOGLEBOT, "/"), (GPTBOT, "/worlds"), (CHATGPT_USER, "/world/commons"), ("curl/8.5", "/llms.txt")] {
        worlds::note_request(&server.state, &Method::GET, path, &headers_with(agent, Some("198.51.100.21")), asleep);
    }
    worlds::note_request(&server.state, &Method::POST, "/public/mcp", &headers_with("curl/8.5", Some("198.51.100.22")), asleep);
    server.view(&own).await;
    server.get("/api/worlds").send().await.ok();
    server.tool("look", json!({ "player_token": joined.token })).await;
    server.tool("how_am_i_doing", json!({ "player_token": joined.token })).await;
    server.get("/api/worlds/commons/me").token(&joined.token).send().await.ok();
    let claim = joined.claim_code.clone().unwrap();
    server.post("/api/players/claim", json!({ "claim_code": claim })).token(&ann.key).send().await;
    // Moves in the frozen world, refused whatever they are, and none of them makes it step.
    let player = server.state.db.player(joined.player.id).unwrap().unwrap();
    let moves_before = worlds::moves_left(&player, asleep);
    for body in [mv("walk", Some(-1), Some(-1)), mv("place", Some(-5), Some(999)), mv("bulldoze", Some(-1), Some(-1)), mv("nope", None, None), emote("dance")] {
        let refused = worlds::act(&server.state, &player, &own, &body, asleep);
        assert!(refused.is_err(), "{body:?} worked in a frozen world");
    }
    assert!(!stepped().contains(&own), "something in a frozen world made it step");
    let player = server.state.db.player(joined.player.id).unwrap().unwrap();
    assert_eq!(worlds::moves_left(&player, asleep), moves_before, "a refused move cost something");
}

// W35, W39
#[tokio::test]
async fn a_flood_of_user_agents_and_addresses_keeps_only_a_kind_a_day_and_a_count() {
    let path = temp_db("agents");
    let server = Server::boot(Db::open(&path).unwrap(), None, None).await;
    worlds::ensure_commons(&server.state).unwrap();
    let t = now();
    let mut rng = Rng(0xBEEF);
    let real = ["Googlebot", "bingbot", "GPTBot", "ClaudeBot", "PerplexityBot", "DuckDuckBot", "ChatGPT-User", "Claude-User", "Perplexity-User", "Google-Agent"];
    for n in 0..6000u32 {
        let filler: String = (0..rng.below(300)).map(|_| ["a", "b", "ua-marker-9921", " ", ";", "/", "é"][rng.below(7) as usize]).collect();
        let name = real[rng.below(real.len() as u64) as usize];
        let agent = match rng.below(4) {
            0 => format!("{filler}{name}{filler}"),
            1 => format!("{filler}{}", &name[..name.len() - 1]),
            2 => format!("Mozilla/5.0 ua-marker-9921 {name}/2.1 {filler}"),
            _ => filler.clone(),
        };
        let address = format!("2001:db8:ffff::{n:x}");
        let headers = headers_with(&agent, Some(&address));
        for place in ["/", "/world/secret-path-5531", "/llms.txt", "/agents.md", "/worlds", "/api/me", "/mcp"] {
            worlds::note_request(&server.state, &Method::GET, place, &headers, t + i64::from(n % 120));
        }
        worlds::note_request(&server.state, &Method::POST, "/public/mcp", &headers, t + i64::from(n % 120));
        if n % 500 == 0 {
            worlds::housekeeping(&server.state, t + i64::from(n % 120) + 61);
        }
    }
    worlds::housekeeping(&server.state, t + 400);
    let rows = server.state.db.sightings_on(worlds::today(t)).unwrap();
    let allowed: HashSet<&str> = world::CRAWLERS.iter().map(|c| c.id).chain(world::FETCHERS.iter().map(|f| f.id)).collect();
    assert!(rows.len() <= allowed.len());
    for row in &rows {
        assert!(allowed.contains(row.0.as_str()), "{} isn't on a list", row.0);
    }
    assert!(server.state.worlds.visitors(t + 100).len() <= worlds::MAX_VISITORS);
    // Nothing a request said is anywhere in the database, in any table.
    let dumped = dump(&path);
    remove_db(&path);
    for (table, rows) in &dumped {
        for row in rows {
            for secret in ["ua-marker-9921", "secret-path-5531", "2001:db8:ffff"] {
                assert!(!row.contains(secret), "{table} holds {secret}: {row}");
            }
        }
    }
    let view = server.fresh_view(COMMONS).await.to_string();
    for secret in ["ua-marker-9921", "secret-path-5531", "2001:db8:ffff", "é"] {
        assert!(!view.contains(secret));
    }
}

// W34, W38
#[tokio::test]
async fn every_reply_is_the_same_for_every_kind_of_agent_in_every_header_and_every_byte() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let (world, _) = server.frozen_world(&ann, true).await;
    let guest = server.guest().await;
    server.view(COMMONS).await;
    let mut agents: Vec<String> = vec![CHROME.to_string(), "curl/8.5".to_string(), String::new(), "x".repeat(3000), "weird\tagent ; Googlebot-ish".to_string()];
    agents.extend(world::CRAWLERS.iter().map(|c| format!("Mozilla/5.0 (compatible; {}/1.0)", c.name)));
    agents.extend(world::FETCHERS.iter().map(|f| format!("Mozilla/5.0 (compatible; {}/1.0)", f.name)));
    let requests: Vec<(Method, String, Option<String>, Option<Value>)> = vec![
        (Method::GET, "/".into(), None, None),
        (Method::GET, "/worlds".into(), None, None),
        (Method::GET, "/world/commons".into(), None, None),
        (Method::GET, format!("/world/{world}"), None, None),
        (Method::GET, "/world/nowhere".into(), None, None),
        (Method::GET, "/llms.txt".into(), None, None),
        (Method::GET, "/agents.md".into(), None, None),
        (Method::GET, "/robots.txt".into(), None, None),
        (Method::GET, "/sitemap.xml".into(), None, None),
        (Method::GET, "/api/worlds".into(), None, None),
        (Method::GET, "/api/worlds/commons".into(), None, None),
        (Method::GET, format!("/api/worlds/{world}"), None, None),
        (Method::GET, "/api/worlds/nowhere".into(), None, None),
        (Method::GET, "/api/worlds/commons/me".into(), Some(guest.token.clone()), None),
        (Method::GET, "/api/worlds/commons/me".into(), None, None),
        (Method::GET, "/api/me".into(), None, None),
        (Method::GET, "/api/hangouts/abc".into(), None, None),
        (Method::POST, "/api/worlds/commons/act".into(), Some(guest.token.clone()), Some(json!({ "action": "walk", "x": 400, "y": 0 }))),
        (Method::POST, "/api/worlds/commons/act".into(), None, Some(json!({ "action": "emote", "emote": "wave" }))),
        (Method::POST, "/public/mcp".into(), None, Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))),
        (Method::POST, "/public/mcp".into(), None, Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "look", "arguments": { "w": 4, "h": 4 } } }))),
        (Method::POST, "/mcp".into(), None, Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))),
        (Method::GET, "/style.css".into(), None, None),
        (Method::GET, "/nowhere".into(), None, None),
    ];
    let same = |reply: &Reply| {
        let mut headers: Vec<(String, String)> = reply
            .headers
            .iter()
            .filter(|(name, _)| name.as_str() != "date")
            .map(|(name, value)| (name.to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
            .collect();
        headers.sort();
        (reply.status, reply.text.clone(), headers)
    };
    for (method, path, token, body) in &requests {
        let send = |agent: &str| {
            let mut request = server.req(method.clone(), path).ua(agent);
            if let Some(token) = token {
                request = request.token(token);
            }
            if let Some(body) = body {
                request = request.json(body.clone());
            }
            request.ip(&server.fresh_ip())
        };
        let reference = same(&send(CHROME).send().await);
        for agent in &agents {
            let reply = send(agent).send().await;
            // The same reply as a browser's, whoever asks. (A view shows the frogs and visitors who
            // came before it, which is a change in the world and not in who is looking.)
            let mut got = same(&reply);
            let mut want = reference.clone();
            if path.starts_with("/api/worlds/commons") && method == Method::GET && !path.ends_with("/me") {
                got.1 = String::new();
                want.1 = String::new();
                got.2.retain(|(name, _)| name != "etag");
                want.2.retain(|(name, _)| name != "etag");
            }
            assert_eq!(got, want, "{method} {path} for {agent:?}");
        }
    }
}

// W36
#[tokio::test]
async fn a_crawler_cannot_choose_where_its_frog_stands_or_what_it_is_called() {
    let server = Server::start().await;
    server.view(COMMONS).await;
    let mut seen = HashSet::new();
    for (agent, path, ip) in [
        (GOOGLEBOT, "/", "203.0.113.1"),
        ("Googlebot", "/worlds", "203.0.113.2"),
        ("Mozilla/5.0 Googlebot/2.1 (+http://www.google.com/bot.html) lots of other words and 9999 numbers", "/world/commons?x=5&y=9", "2001:db8::1"),
        ("GOOGLEBOT", "/world/secret", "198.51.100.200"),
        ("googlebot-image/1.0", "/robots.txt", "203.0.113.3"),
    ] {
        server.get(path).ua(agent).ip(ip).send().await;
        let view = server.fresh_view(COMMONS).await;
        let frogs = view["crawlers"].as_array().unwrap();
        assert_eq!(frogs.len(), 1, "one frog for one kind");
        seen.insert((frogs[0][0].clone(), frogs[0][1].clone(), frogs[0][2].clone(), frogs[0][3].clone()));
    }
    assert_eq!(seen.len(), 1, "the same frog in the same place, whoever called: {seen:?}");
    assert_eq!(seen.iter().next().unwrap().1, "Googlebot");
}

// W29
#[tokio::test]
async fn a_world_is_polled_never_streamed() {
    let server = Server::start().await;
    for path in [
        "/api/worlds/commons/stream",
        "/api/worlds/commons/events",
        "/api/worlds/commons/watch",
        "/api/worlds/stream",
        "/api/worlds/commons/ws",
        "/api/worlds/commons.json",
    ] {
        let reply = server.get(path).header("accept", "text/event-stream").send().await;
        assert!(reply.status == StatusCode::NOT_FOUND || reply.status == StatusCode::METHOD_NOT_ALLOWED, "{path}: {}", reply.show());
    }
    let reply = server.get("/api/worlds/commons").header("accept", "text/event-stream").send().await;
    assert_eq!(reply.headers.get("content-type").unwrap(), "application/json");
    assert!(reply.headers.get("transfer-encoding").is_none() || reply.body.is_object());
}

// W12, W30
#[tokio::test]
async fn a_page_on_another_site_cannot_make_a_visitors_browser_use_up_their_guests() {
    let server = Server::start().await;
    // The browser says where the page that made the request is from. The MCP endpoint turns these away.
    let mut made = 0;
    for _ in 0..8 {
        let reply = server
            .post("/api/public/join", json!({}))
            .ip("203.0.113.61")
            .header("origin", "https://evil.example")
            .header("sec-fetch-site", "cross-site")
            .send()
            .await;
        if reply.status == StatusCode::OK {
            made += 1;
        }
    }
    assert_eq!(made, 0, "{made} guests were made for a request that came from a page on another site");
    // The same page, with a body that needs no preflight.
    let reply = server
        .req(Method::POST, "/api/public/join")
        .ip("203.0.113.62")
        .header("origin", "https://evil.example")
        .raw("text/plain", b"{}")
        .send()
        .await;
    assert_ne!(reply.status, StatusCode::OK, "{}", reply.show());
    // The site's own pages are still let in.
    let own = server.post("/api/public/join", json!({})).ip("203.0.113.63").header("origin", &server.base).send().await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.show());
}

// W23
#[test]
fn the_stages_begin_at_10_50_and_200_places() {
    for (places, stage) in [(0, "hamlet"), (9, "hamlet"), (10, "village"), (49, "village"), (50, "town"), (199, "town"), (200, "city"), (u32::MAX, "city")] {
        assert_eq!(world::stage_for(places), stage, "{places} places");
    }
}

// W19, W65, W67
#[test]
fn a_disaster_depends_on_the_seed_and_the_step_number_and_nothing_else() {
    let land = world::terrain(4242, 64);
    let highway = world::highway(4242, &land);
    let start = world::start_tiles(4242, &land);
    let (hx, hy) = *start.keys().next().unwrap();
    let mut tiles = start.clone();
    for (n, (dx, dy)) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)].into_iter().enumerate() {
        let pos = ((i32::from(hx) + dx) as u16, (i32::from(hy) + dy) as u16);
        if land.at(pos).is_some_and(|g| g.is_open()) && !tiles.contains_key(&pos) {
            let kind = [Kind::House, Kind::Homes, Kind::Shops, Kind::Park][n];
            tiles.insert(pos, Tile { kind, level: 1, player: Some(7) });
        }
    }
    let kind = world::disaster_for(4242, 17);
    let first: Vec<_> = (0..world::STRIKE_STEPS).map(|done| world::strike(4242, kind, done, 64)).collect();
    for _ in 0..5 {
        std::thread::sleep(Duration::from_millis(3));
        assert_eq!(world::disaster_for(4242, 17), kind, "the same step gave a different disaster");
        let again: Vec<_> = (0..world::STRIKE_STEPS).map(|done| world::strike(4242, kind, done, 64)).collect();
        assert_eq!(again, first, "the same step gave different circles");
    }
    // Replaying a whole strike from the same start lands on the same map, and never takes the highway.
    let play = |mut tiles: world::Tiles| {
        for done in 0..world::STRIKE_STEPS {
            let hits = world::strike(4242, kind, done, 64);
            let changes = world::strike_changes(&tiles, &highway, &hits, done + 1 == world::STRIKE_STEPS);
            world::apply(&mut tiles, &changes);
        }
        tiles
    };
    let (one, two) = (play(tiles.clone()), play(tiles));
    assert_eq!(one, two);
    assert!(highway.iter().all(|pos| one.contains_key(pos)), "a strike took the highway");
    assert!(one.len() <= highway.len(), "the last step left something standing: {} tiles", one.len());
}

// ----------------------------------------------------------------------------------------------
// More ways round the owner's powers and the city's budget
// ----------------------------------------------------------------------------------------------

// W3, W18, W27
#[tokio::test]
async fn a_player_the_owner_removed_cannot_walk_back_onto_the_street_by_joining_again() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let (world, _) = server.frozen_world(&ann, true).await;
    let bot = server.bot("Scout").await;
    // The bot was in the owner's world when it froze, with roads there, and is removed.
    let token = server.bot_player(&bot).await;
    let id = server.player_id(&token).await;
    server.leave_in(&world, id);
    assert_eq!(server.fresh_view(&world).await["players"].as_array().unwrap().len(), 1, "it is on the street");
    let removed = server.post(&format!("/api/worlds/{world}/players/{id}/remove"), json!({})).token(&ann.key).send().await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.show());
    assert_eq!(server.tiles(&world).values().filter(|tile| tile.player == Some(id as u64)).count(), 0);
    assert_eq!(server.fresh_view(&world).await["players"], json!([]), "it left the street");

    // Joining again, with the bot's own token or through the tool, must not put it back.
    let again = server.post("/api/public/join", json!({ "world_id": world })).token(&bot.token).send().await;
    let via_tool = server
        .post("/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "join", "arguments": { "world_id": world } } }))
        .token(&bot.token)
        .send()
        .await;
    let view = server.fresh_view(&world).await;
    assert!(
        view["players"].as_array().unwrap().is_empty(),
        "a player the owner removed is back on the owner's street (join said {} and {})",
        again.status,
        via_tool.body["result"]["isError"]
    );
    assert!(again.status.is_client_error(), "{}", again.show());
    assert_eq!(via_tool.body["result"]["isError"], true, "{}", via_tool.text);
    // And it can't act there either, with the token it had.
    let act = server.act(&token, &world, json!({ "action": "emote", "emote": "wave" })).await;
    assert!(act.status.is_client_error(), "{}", act.show());
}

// W30
#[tokio::test]
async fn measure_how_much_of_the_commons_move_budget_one_person_can_use_in_a_minute() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let ann = server.person("Ann").await;
    let t = now();
    let player = server.state.db.own_player(&ann.id, t).unwrap();
    let mut done = 0;
    for _ in 0..300 {
        if worlds::act(&server.state, &player, COMMONS, &emote("wave"), t).is_ok() {
            done += 1;
        }
    }
    // A guest in The Commons in the same minute, who has moves of their own left.
    let guest = server.guest().await;
    let guest_player = server.player_row(server.player_id(&guest.token).await);
    let one_more = worlds::act(&server.state, &guest_player, COMMONS, &emote("wave"), t);
    println!(
        "COST REPORT: one person's {done} emotes in The Commons, in one minute, used all of its 300 moves a minute and half of the server's 600; a guest was then told: {}",
        one_more.as_ref().err().map_or("nothing".to_string(), |error| error.message.clone())
    );
    assert_eq!(done, 300);
    // The world went past its 300 a minute, so it turns away a guest who has moves left, and says why.
    let refused = one_more.expect_err("a world went past its 300 a minute");
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(refused.message.contains("this world is very busy"), "{}", refused.message);
    // And a minute later it is open again.
    let later = worlds::act(&server.state, &guest_player, COMMONS, &emote("wave"), t + 61);
    assert!(later.is_ok(), "the minute's share did not come back: {later:?}");
}

// W9, W11, W30
#[tokio::test]
async fn measure_how_many_moves_a_day_a_swarm_of_free_accounts_adds_up_to() {
    let server = Server::start().await;
    // People are free to start, and each can claim twenty guests.
    let mut total = 0i64;
    let started = std::time::Instant::now();
    for n in 0..5 {
        let person = server.person(&format!("P{n}")).await;
        server.acts_ok(&person.key, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
        for _ in 0..20 {
            let ip = server.fresh_ip();
            let guest = server.post("/api/public/join", json!({})).ip(&ip).send().await.ok();
            server.post("/api/players/claim", json!({ "claim_code": guest["claim_code"] })).token(&person.key).send().await.ok();
        }
        let mine = server.get("/api/players/mine").token(&person.key).send().await.ok();
        total += mine["players"].as_array().unwrap().iter().map(|p| p["moves_left"].as_i64().unwrap()).sum::<i64>();
    }
    println!(
        "COST REPORT: 5 people made in {:?} and 100 guests (the server's hourly cap is 600) came to {total} moves for the day, with no sign-up beyond a name",
        started.elapsed()
    );
    assert!(total > 0);
}

// ----------------------------------------------------------------------------------------------
// Last sweeps: every tool, every id, the address, and what a name gives away
// ----------------------------------------------------------------------------------------------

// W6, W31
#[tokio::test]
async fn extreme_numbers_in_every_mcp_tool_and_every_id_in_a_path_break_nothing() {
    let server = Server::start().await;
    let (keeper_server, keeper) = Server::start_with_keeper().await;
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    let mut rng = Rng(0xAB12);
    let mut broken = Vec::new();
    let mut sent = 0;
    for round in 0..900 {
        let tool = rng.pick(&["look", "build", "bulldoze", "walk", "emote", "how_am_i_doing", "join"]);
        let mut args = json!({ "player_token": guest.token });
        for field in ["x", "y", "x2", "y2", "w", "h"] {
            if rng.below(3) != 0 {
                args[field] = json!(rng.pick(&EXTREMES));
            }
        }
        if tool == "build" {
            args["type"] = json!(rng.pick(&["road", "homes", "shops", "workshops", "park"]));
            args["x"] = json!(rng.pick(&EXTREMES));
            args["y"] = json!(rng.pick(&EXTREMES));
        }
        if tool == "bulldoze" || tool == "walk" {
            args["x"] = json!(rng.pick(&EXTREMES));
            args["y"] = json!(rng.pick(&EXTREMES));
            args.as_object_mut().unwrap().retain(|key, _| ["player_token", "x", "y"].contains(&key.as_str()));
        }
        if tool == "emote" {
            args = json!({ "player_token": guest.token, "emote": "wave" });
        }
        if tool == "how_am_i_doing" || tool == "join" {
            args = json!({});
            if tool == "how_am_i_doing" {
                args = json!({ "player_token": guest.token });
            }
        }
        let call = json!({ "jsonrpc": "2.0", "id": rng.pick(&EXTREMES), "method": "tools/call", "params": { "name": tool, "arguments": args } });
        let result = server.req(Method::POST, "/public/mcp").ip(&format!("10.200.{}.{}", round / 250, round % 250 + 1)).json(call.clone()).try_send().await;
        sent += 1;
        if let Some(why) = is_broken(&result) {
            broken.push(format!("{call}: {why}"));
        }
    }
    // Ids in paths: whole numbers of every size, and things that are not numbers.
    let ids = [
        "0", "-1", "1", "9223372036854775807", "9223372036854775808", "-9223372036854775808", "-9223372036854775809", "18446744073709551615",
        "18446744073709551616", "1e5", "0x10", "+1", " 1", "1 ", "01", "1.0", "١", "%31", "1%00", "", "null", "undefined", "NaN", "../1",
    ];
    for id in ids {
        for (method, path, key) in [
            (Method::POST, format!("/api/players/{id}/reroll"), &ann.key),
            (Method::POST, format!("/api/worlds/commons/players/{id}/remove"), &keeper.key),
            (Method::POST, format!("/api/worlds/commons/players/{id}/remove"), &ann.key),
            (Method::GET, format!("/api/worlds/{id}"), &ann.key),
            (Method::GET, format!("/api/worlds/{id}/me"), &guest.token),
            (Method::POST, format!("/api/worlds/join/{id}"), &guest.token),
        ] {
            let target = if path.contains("commons/players") && key == &keeper.key { &keeper_server } else { &server };
            let result = target.req(method.clone(), &path).token(key).json(json!({})).try_send().await;
            sent += 1;
            if let Some(why) = is_broken(&result) {
                broken.push(format!("{method} {path}: {why}"));
            }
        }
    }
    assert!(sent > 1000);
    assert!(broken.is_empty(), "{} of {sent} requests broke something:\n{}", broken.len(), broken.iter().take(10).cloned().collect::<Vec<_>>().join("\n"));
    // Still there, and nothing was spent that the tools did not say they spent.
    assert!(server.get("/api/worlds/commons/me").token(&guest.token).try_send().await.is_ok());
    assert!(server.moves_left(&guest.token).await <= 30);
}

// W12
#[tokio::test]
async fn new_guests_are_counted_per_ipv6_network_and_not_per_address_inside_it() {
    start_early_in(60, 30).await;
    let server = Server::start().await;
    // A home or a server with one IPv6 network can write 2^64 addresses. They are one place.
    let mut joined = 0;
    for n in 0..30u32 {
        let address = format!("2001:db8:abcd:1234::{:x}", n + 1);
        if server.post("/api/public/join", json!({})).header("x-forwarded-for", &address).send().await.status == StatusCode::OK {
            joined += 1;
        }
    }
    assert!(joined <= 5, "{joined} guests joined from 30 addresses in one IPv6 network");
    let mut views = 0;
    for n in 0..300u32 {
        let address = format!("2001:db8:beef:1234:{:x}::1", n + 1);
        if server.get("/api/worlds/commons").header("x-forwarded-for", &address).send().await.status == StatusCode::OK {
            views += 1;
        }
    }
    assert!(views <= 240, "{views} views were served to 300 addresses in one IPv6 network");
}

// W4, W7
#[tokio::test]
async fn a_stranger_cannot_tell_a_claimed_player_by_a_name_that_is_not_its_numbers() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let claimed = server.guest().await;
    let plain = server.guest().await;
    server.post("/api/players/claim", json!({ "claim_code": claimed.claim })).token(&ann.key).send().await.ok();
    let id = server.player_id(&claimed.token).await;
    // The person rerolls, which is what they are for.
    for _ in 0..3 {
        server.post(&format!("/api/players/{id}/reroll"), json!({})).token(&ann.key).send().await.ok();
    }
    for token in [&claimed.token, &plain.token] {
        server.acts_ok(token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await;
    }
    // Every name comes from a seed of its own, which began random: the public number gives nothing
    // away, so a player whose name was rerolled looks like one whose name never was. (The seeds
    // are compared, not the names, because two seeds can share a name.)
    let view = server.fresh_view(COMMONS).await;
    let players = view["players"].as_array().unwrap();
    assert_eq!(players.len(), 2);
    for player in players {
        let id = player[0].as_i64().unwrap();
        let seed = server.player_row(id).name_seed;
        assert_ne!(seed, id, "player {id} still has the name its number gives, which a reroll would stand out against");
        assert_eq!(player[1], world::player_name(seed as u64).as_str());
    }
    // A brand new player of every kind starts the same way.
    let bot = server.bot("Scout").await;
    server.bot_player(&bot).await;
    for row in server.state.db.players_of(&bot.person).unwrap() {
        assert_ne!(row.name_seed, row.id);
    }
    let own = server.state.db.own_player(&ann.id, now()).unwrap();
    assert_ne!(own.name_seed, own.id);
}

// W12
#[test]
fn the_server_listens_on_localhost_unless_told_otherwise() {
    let main = include_str!("../src/main.rs");
    assert!(main.contains("AGENTHANGOUT_BIND"), "where it listens is a setting");
    let line = main.lines().find(|line| line.contains("AGENTHANGOUT_BIND") && line.contains("unwrap_or_else")).expect("a default");
    assert!(line.contains("\"127.0.0.1:") || line.contains("\"localhost:") || line.contains("\"[::1]:"), "the default is not loopback: {line}");
}

// ----------------------------------------------------------------------------------------------
// The edges of the building rules, tried from the outside (W6, W10, W14-W17)
// ----------------------------------------------------------------------------------------------

// W17
#[tokio::test]
async fn a_builder_gone_six_days_keeps_their_tiles_and_one_gone_a_week_loses_them_to_anyone() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let builder = server.guest().await;
    let stranger = server.guest().await;
    let builder_id = server.player_id(&builder.token).await;
    let map = Map::from_db(&server, COMMONS);
    // Three roads in a row, off the highway: a dead end that nothing depends on.
    let (first, last) = map.line_from_road(3).unwrap();
    server.acts_ok(&builder.token, COMMONS, road_move(first, last)).await;
    let (step_x, step_y) = ((last.0 - first.0).signum(), (last.1 - first.1).signum());
    let tile = |n: i64| json!({ "action": "bulldoze", "x": last.0 - step_x * n, "y": last.1 - step_y * n });
    let set_seen = |ago: i64| {
        server.state.db.in_transaction::<(), rusqlite::Error>(|tx| tx.charge(builder_id, 0, 0, now() - ago)).unwrap();
    };
    set_seen(7 * 86_400 - 120);
    assert!(server.act(&stranger.token, COMMONS, tile(0)).await.status.is_client_error(), "six days and 23 hours is not a week");
    set_seen(7 * 86_400 + 30);
    assert_eq!(server.act(&stranger.token, COMMONS, tile(0)).await.status, StatusCode::OK, "a week and a bit is");
    // The builder coming back stops it for the rest.
    set_seen(0);
    assert!(server.act(&stranger.token, COMMONS, tile(1)).await.status.is_client_error());
    // The stranger's own work is still theirs to take down, and nobody else's is.
    let mine = server.act(&stranger.token, COMMONS, tile(1)).await;
    assert!(mine.status.is_client_error());
}

// W10
#[tokio::test]
async fn a_guest_stops_working_a_week_after_it_was_last_used_and_not_a_minute_before() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let guest = server.guest().await;
    let id = server.player_id(&guest.token).await;
    let idle = |seconds: i64| {
        server.state.db.in_transaction::<(), rusqlite::Error>(|tx| tx.charge(id, 0, 0, now() - seconds)).unwrap();
    };
    idle(7 * 86_400 - 120);
    assert_eq!(server.get("/api/worlds/commons/me").token(&guest.token).send().await.status, StatusCode::OK);
    idle(7 * 86_400 + 120);
    for status in [
        server.get("/api/worlds/commons/me").token(&guest.token).send().await.status,
        server.act(&guest.token, COMMONS, json!({ "action": "emote", "emote": "wave" })).await.status,
    ] {
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(server.call("how_am_i_doing", json!({ "player_token": guest.token })).await["isError"], true);
    assert_eq!(server.call("emote", json!({ "player_token": guest.token, "emote": "wave" })).await["isError"], true);
    // Nor can anyone claim it, and no door opens by being asked again.
    let ann = server.person("Ann").await;
    assert_eq!(server.post("/api/players/claim", json!({ "claim_code": guest.claim })).token(&ann.key).send().await.status, StatusCode::NOT_FOUND);
    assert_eq!(server.get("/api/worlds/commons/me").token(&guest.token).send().await.status, StatusCode::UNAUTHORIZED);
}

// W6
#[tokio::test]
async fn unknown_fields_are_refused_by_every_world_body_and_cost_nothing() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    let bot = server.bot("Scout").await;
    let extra = json!({ "also": 1 });
    let refused = [
        server.post("/api/worlds/commons/act", json!({ "action": "emote", "emote": "wave", "also": 1 })).token(&guest.token).send().await,
        server.post("/api/worlds/commons/act", json!({ "action": "walk", "x": 65, "y": 64, "z": 0 })).token(&guest.token).send().await,
        server.post("/api/public/join", json!({ "world_id": "commons", "also": 1 })).send().await,
        server.post("/api/public/join", json!({ "also": 1 })).token(&bot.token).send().await,
        server.post("/api/worlds", json!({ "seed": 4, "open": true, "also": 1 })).token(&ann.key).send().await,
        server.post("/api/worlds", extra.clone()).token(&ann.key).send().await,
        server.post("/api/players/claim", json!({ "claim_code": guest.claim, "also": 1 })).token(&ann.key).send().await,
    ];
    for reply in &refused {
        assert!(reply.status.is_client_error(), "{}", reply.show());
    }
    for (name, args) in [
        ("join", json!({ "also": 1 })),
        ("look", json!({ "also": 1 })),
        ("build", json!({ "player_token": guest.token, "type": "road", "x": 70, "y": 70, "also": 1 })),
        ("bulldoze", json!({ "player_token": guest.token, "x": 70, "y": 70, "also": 1 })),
        ("walk", json!({ "player_token": guest.token, "x": 70, "y": 70, "also": 1 })),
        ("emote", json!({ "player_token": guest.token, "emote": "wave", "also": 1 })),
        ("how_am_i_doing", json!({ "player_token": guest.token, "also": 1 })),
    ] {
        server.tool_refused(name, args).await;
    }
    assert_eq!(server.moves_left(&guest.token).await, 30);
    assert!(server.state.db.world_of_owner(&ann.id).unwrap().is_none());
    assert!(server.state.db.players_of(&ann.id).unwrap().is_empty());
    assert!(server.state.db.players_of(&bot.person).unwrap().is_empty());
}

// W15
#[tokio::test]
async fn a_road_must_touch_a_road_on_a_side_and_a_corner_does_not_count() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let guest = server.guest().await;
    let map = Map::from_db(&server, COMMONS);
    let roads: HashSet<(i64, i64)> = map.roads().into_iter().collect();
    let touches_side = |x: i64, y: i64| [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| roads.contains(&(x + dx, y + dy)));
    // A tile that touches the highway at a corner only.
    let corner = roads
        .iter()
        .flat_map(|&(rx, ry)| [(rx + 1, ry + 1), (rx - 1, ry - 1), (rx + 1, ry - 1), (rx - 1, ry + 1)])
        .find(|&(x, y)| map.open(x, y) && !touches_side(x, y))
        .expect("a corner");
    let far = (0..map.size)
        .flat_map(|y| (0..map.size).map(move |x| (x, y)))
        .find(|&(x, y)| map.open(x, y) && roads.iter().all(|&(rx, ry)| cheb((rx, ry), (x, y)) > 4))
        .expect("a place far from the highway");
    let before = server.tiles(COMMONS);
    let single = |x: i64, y: i64| json!({ "action": "road", "x": x, "y": y });
    for body in [
        single(corner.0, corner.1),
        single(far.0, far.1),
        json!({ "action": "road", "x": far.0, "y": far.1, "x2": far.0 + 1, "y2": far.1 }),
        json!({ "action": "road", "x": far.0, "y": far.1, "x2": far.0, "y2": far.1 + 1 }),
    ] {
        let reply = server.act(&guest.token, COMMONS, body.clone()).await;
        assert!(reply.status.is_client_error(), "{body} was let in: {}", reply.show());
    }
    assert_eq!(server.tiles(COMMONS), before);
    assert_eq!(server.moves_left(&guest.token).await, 30);
    // A line that begins on a side that touches is let in, and so is the next one beyond it.
    let (first, last) = map.line_from_road(2).unwrap();
    server.acts_ok(&guest.token, COMMONS, road_move(first, last)).await;
}

// W14, W55
#[tokio::test]
async fn no_move_builds_on_water_a_hill_or_forest_that_is_not_cleared() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let ann = server.person("Ann").await;
    let map = Map::from_db(&server, COMMONS);
    let before = server.tiles(COMMONS);
    for ground in ['w', 'h', 'f'] {
        // Beside a road if there is such a tile, so that the ground is all that can stop a place.
        let (x, y) = map.find_beside_road(ground).or_else(|| map.find(ground)).unwrap_or_else(|| panic!("The Commons has no {ground}"));
        for body in [
            place_move("house", (x, y)),
            place_move("cafe", (x, y)),
            json!({ "action": "park", "x": x, "y": y }),
            json!({ "action": "zone", "kind": "homes", "x": x, "y": y }),
            json!({ "action": "road", "x": x, "y": y }),
        ] {
            let reply = server.act(&ann.key, COMMONS, body.clone()).await;
            assert!(reply.status.is_client_error(), "{ground}: {body}: {}", reply.show());
        }
        if ground != 'f' {
            assert!(server.act(&ann.key, COMMONS, json!({ "action": "bulldoze", "x": x, "y": y })).await.status.is_client_error(), "{ground} can't be bulldozed");
        }
    }
    // Water can't be walked on, a hill can.
    let (wx, wy) = map.find('w').unwrap();
    assert!(server.act(&ann.key, COMMONS, json!({ "action": "walk", "x": wx, "y": wy })).await.status.is_client_error());
    assert_eq!(server.tiles(COMMONS), before);
    assert_eq!(server.moves_left(&ann.key).await, 300);
}

// W16
#[tokio::test]
async fn a_road_is_at_most_sixteen_whichever_way_it_is_drawn() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let ann = server.person("Ann").await;
    let map = Map::from_db(&server, COMMONS);
    // A road of 17 along any side from the highway, if there is room for one, is refused whole.
    if let Some((first, last)) = map.line_from_road(17) {
        let reply = server.act(&ann.key, COMMONS, road_move(first, last)).await;
        assert!(reply.status.is_client_error(), "{}", reply.show());
        let reply = server.act(&ann.key, COMMONS, road_move(last, first)).await;
        assert!(reply.status.is_client_error(), "{}", reply.show());
    }
    assert_eq!(server.moves_left(&ann.key).await, 300);
    if let Some((first, last)) = map.line_from_road(16) {
        let reply = server.act(&ann.key, COMMONS, road_move(first, last)).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.show());
        assert_eq!(reply.body["cost"], 16);
    }
}

// W4, W7, W11
#[tokio::test]
async fn nobody_but_the_person_who_claimed_a_player_can_rename_it_and_a_used_code_is_dead_for_everyone() {
    let (server, keeper) = Server::start_with_keeper().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let guest = server.guest().await;
    let other = server.guest().await;
    server.post("/api/players/claim", json!({ "claim_code": guest.claim })).token(&ann.key).send().await.ok();
    let id = server.player_id(&guest.token).await;
    let other_id = server.player_id(&other.token).await;
    let name = server.me(&guest.token, COMMONS).await["player"]["name"].clone();
    for who in [&bo.key, &keeper.key] {
        let reply = server.post(&format!("/api/players/{id}/reroll"), json!({})).token(who).send().await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.show());
    }
    // A guest nobody has claimed can't be renamed by anyone, and neither can a bot's player by a stranger.
    for who in [&ann.key, &bo.key] {
        assert_eq!(server.post(&format!("/api/players/{other_id}/reroll"), json!({})).token(who).send().await.status, StatusCode::NOT_FOUND);
    }
    assert_eq!(server.me(&guest.token, COMMONS).await["player"]["name"], name);
    assert_eq!(server.me(&other.token, COMMONS).await["player"]["name"], other.name.as_str());
    // The claim code is spent for everyone, however it is tried, and does not move the player on.
    for who in [&bo.key, &ann.key, &keeper.key] {
        assert_eq!(server.post("/api/players/claim", json!({ "claim_code": guest.claim })).token(who).send().await.status, StatusCode::NOT_FOUND);
    }
    assert_eq!(server.get("/api/players/mine").token(&bo.key).send().await.ok()["players"], json!([]));
    assert_eq!(server.get("/api/players/mine").token(&ann.key).send().await.ok()["players"].as_array().unwrap().len(), 1);
    // The token is not the code, and the code is not a token.
    assert_eq!(server.post("/api/players/claim", json!({ "claim_code": other.token })).token(&bo.key).send().await.status, StatusCode::NOT_FOUND);
    assert_eq!(server.get("/api/worlds/commons/me").token(&other.claim).send().await.status, StatusCode::UNAUTHORIZED);
}

// ----------------------------------------------------------------------------------------------
// The rules not yet attacked: W28, W33, W40, W41
// ----------------------------------------------------------------------------------------------

// W25, W28
#[tokio::test]
async fn nothing_a_caller_does_makes_a_second_commons_or_changes_the_first() {
    let server = Server::start().await;
    worlds::ensure_commons(&server.state).unwrap();
    let before = server.view(COMMONS).await;
    let ann = server.person("Ann").await;
    // A world started with the Commons' own seed is refused, and nothing is made.
    let own = server.post("/api/worlds", json!({ "seed": world::COMMONS_SEED })).token(&ann.key).send().await;
    assert_eq!(own.status, StatusCode::GONE, "{}", own.show());
    let after = server.fresh_view(COMMONS).await;
    assert_eq!(after["land"], before["land"]);
    assert_eq!(after["size"], 128);
    assert_eq!(after["seed"], 307);
    assert_eq!(after["kind"], "public");
    // The server's own call, again and again, and a restart on the same file, leave it alone.
    for _ in 0..3 {
        worlds::ensure_commons(&server.state).unwrap();
    }
    let rows = server.state.db.busiest_worlds(0, 100).unwrap();
    assert_eq!(rows.len(), 1, "something other than The Commons was made");
    assert_eq!(rows.iter().filter(|(world, _)| world.kind == "public").count(), 1);
    assert_eq!(rows.iter().filter(|(world, _)| world.id == COMMONS).count(), 1);
    // And no id a caller can give is the Commons' except its own.
    for id in ["Commons", "COMMONS", "commons%20", "%20commons", "commons%00", "comm%6Fns"] {
        let reply = server.get(&format!("/api/worlds/{id}")).send().await;
        assert!(reply.status == StatusCode::NOT_FOUND || reply.body["id"] == COMMONS, "{id}: {}", reply.show());
        if reply.status == StatusCode::OK {
            assert_eq!(id, "comm%6Fns", "only the percent-encoded spelling of the same word is the same id");
        }
    }
}

// W33
#[tokio::test]
async fn the_private_mcp_endpoint_has_no_world_tool_for_anyone_who_can_reach_it() {
    let server = Server::start().await;
    let place = server.city_house("Elm").await;
    let (stay, casa) = server.home_bots(&place).await;
    let scout = server.bot_in(&place.key, &place.person, &place.living, "Scout").await;
    let guest = server.guest().await;
    let world_tools = ["join", "look", "build", "bulldoze", "walk", "emote", "how_am_i_doing"];
    for token in [&place.key, &scout.token, &stay.token, &casa, &guest.token] {
        for tool in world_tools {
            let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": tool, "arguments": { "player_token": guest.token } } });
            let reply = server.post("/mcp", call).token(token).send().await;
            let refused = reply.status.is_client_error() || reply.body["result"]["isError"] == true || reply.body["error"].is_object();
            assert!(refused, "/mcp ran {tool} for a caller: {}", reply.show());
            assert!(!reply.text.contains("structuredContent"), "{}", reply.text);
        }
        let listed = server.post("/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).token(token).send().await;
        for tool in world_tools {
            assert!(!listed.text.contains(&format!("\"name\":\"{tool}\"")), "/mcp lists {tool}");
        }
    }
    // And the public endpoint has nothing of the private one's, for anyone, with any token.
    for token in [&place.key, &scout.token, &casa] {
        for tool in ["inbox", "say", "read_room", "list_city", "ask_for_trip", "go_home", "list_scenes", "request_scene", "share_note", "propose_hangout"] {
            let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": tool, "arguments": {} } });
            let reply = server.post("/public/mcp", call).token(token).send().await;
            assert_eq!(reply.body["result"]["isError"], true, "{tool}: {}", reply.text);
        }
    }
}

// W40, W41
#[tokio::test]
async fn an_agent_that_names_both_a_crawler_and_a_fetcher_is_a_visitor_and_never_a_frog() {
    let server = Server::start().await;
    for agent in [
        "Mozilla/5.0 (compatible; Googlebot/2.1) Google-Agent",
        "ChatGPT-User/1.0 GPTBot/1.2",
        "Googlebot Claude-User",
        "Perplexity-User PerplexityBot",
        "Mozilla/5.0 AppleWebKit (KHTML, like Gecko); compatible; ChatGPT-User/1.0; +https://openai.com/bot",
    ] {
        let fresh = Server::start().await;
        fresh.get("/").ua(agent).ip("198.51.100.7").send().await;
        assert!(fresh.state.worlds.frogs(now()).is_empty(), "{agent} is a frog");
        assert_eq!(fresh.state.worlds.visitors(now()).len(), 1, "{agent} is not a visitor");
    }
    // Whatever fetches the files and the endpoint only agents read is a visitor, and a call that is
    // made with no user agent at all is one too.
    for (method, path) in [
        (Method::GET, "/llms.txt"),
        (Method::GET, "/llms-full.txt"),
        (Method::GET, "/agents.md"),
        (Method::GET, "/.well-known/ai-catalog.json"),
        (Method::GET, "/public/mcp/server-card"),
        (Method::POST, "/public/mcp"),
    ] {
        for agent in ["", CHROME, GOOGLEBOT, GPTBOT, "Googlebot", "totally-unknown/0.1"] {
            let fresh = Server::start().await;
            let mut request = fresh.req(method.clone(), path).ip("198.51.100.8");
            if !agent.is_empty() {
                request = request.ua(agent);
            }
            if method == Method::POST {
                request = request.json(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }));
            }
            request.send().await;
            assert_eq!(fresh.state.worlds.visitors(now()).len(), 1, "{method} {path} as {agent:?} made no visitor");
            assert!(fresh.state.worlds.frogs(now()).is_empty(), "{method} {path} as {agent:?} made a frog");
        }
    }
    // None of it is a player, and none of it moves.
    assert_eq!(server.view(COMMONS).await["players"], json!([]));
}
