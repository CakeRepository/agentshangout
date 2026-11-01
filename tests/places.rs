//! Tests for what the map is since zones went: places you can go into, which come with time, the
//! worlds that froze, and disasters. They are written from the rules in
//! docs/public-city.md (W13, W20 to W28, W46 to W49, W61 to W70) and docs/spaces.md (S47, S48). A
//! comment such as `// W61` above a test names the rule it covers.
//!
//! The clock on a place is real seconds, so a test that looks days ahead calls `worlds::act` with a
//! time of its own. A disaster goes by steps, so a test jumps the world's step forward.

use std::{
    collections::HashSet,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use agenthangout::{
    app, auth,
    db::{Db, NewWorld, WorldEnd},
    error::ApiResult,
    now,
    world::{self, Change, Disaster, Kind, Tile},
    worlds::{self, Acted, Move},
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
    addresses: AtomicUsize,
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

fn temp_db() -> String {
    let unique = auth::new_id();
    std::env::temp_dir()
        .join(format!("agenthangout-test-{unique}.db"))
        .to_string_lossy()
        .into_owned()
}

impl Server {
    async fn start() -> Self {
        Self::on(Db::open(":memory:").unwrap()).await
    }

    async fn on(db: Db) -> Self {
        Self::boot(db, None).await
    }

    /// A server whose keeper is a person made first, so their id can be set in the config.
    async fn start_with_keeper() -> (Self, Who) {
        let db = Db::open(":memory:").unwrap();
        let key = auth::new_owner_key();
        db.create_person("keeper", "Keeper", &auth::hash_secret(&key), "keeper-feed", now())
            .unwrap();
        let server = Self::boot(db, Some("keeper".to_string())).await;
        (server, Who { key, id: "keeper".to_string() })
    }

    async fn boot(db: Db, keeper_id: Option<String>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let state = AppState::new(
            db,
            Config {
                public_url: base.clone(),
                create_code: Some(CODE.to_string()),
                allow_private_webhooks: true,
                keeper_id,
                god_ids: Vec::new(),
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        worlds::ensure_commons(&state).unwrap();
        Self { base, client: Client::new(), state, addresses: AtomicUsize::new(1) }
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

    /// Someone who came in on a friend's link, so they have no third space of their own yet.
    async fn person(&self, name: &str) -> Who {
        let host = self
            .post(
                "",
                "/api/hangouts",
                json!({ "name": "Host", "your_name": "Host", "create_code": CODE }),
            )
            .await
            .ok();
        let room = text(&host["hangout"]["id"]);
        let snapshot =
            self.get(&text(&host["owner_key"]), &format!("/api/hangouts/{room}")).await.ok();
        let code = text(&snapshot["hangout"]["invite_code"]);
        who(&self
            .post("", &format!("/api/invites/{code}/join"), json!({ "your_name": name }))
            .await
            .ok())
    }

    async fn guest(&self) -> String {
        text(&self.post("", "/api/public/join", json!({})).await.ok()["player_token"])
    }

    async fn act(&self, token: &str, body: Value) -> Reply {
        self.post(token, &format!("/api/worlds/{COMMONS}/act"), body).await
    }

    async fn me(&self, token: &str) -> Value {
        self.get(token, &format!("/api/worlds/{COMMONS}/me")).await.ok()
    }

    /// Tiles beside a road, which is where a place can stand.
    fn beside_roads(&self) -> Vec<(i64, i64)> {
        let tiles = self.state.db.world_tiles(COMMONS).unwrap();
        let mut spots = Vec::new();
        for (&(x, y), tile) in &tiles {
            if tile.kind == Kind::Road {
                let (x, y) = (i64::from(x), i64::from(y));
                spots.extend([(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]);
            }
        }
        spots
    }

    /// Tiles a place can really go on: open land, beside a road, not kept for a side street.
    fn free_spots(&self) -> Vec<(i64, i64)> {
        let tiles = self.state.db.world_tiles(COMMONS).unwrap();
        let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
        let free = |&(x, y): &(i64, i64)| {
            let pos = (x as u16, y as u16);
            !tiles.contains_key(&pos)
                && !world::is_reserved(&tiles, pos)
                && land.at(pos).is_some_and(|ground| ground.is_open())
        };
        self.beside_roads().into_iter().filter(free).collect()
    }

    async fn place(&self, who: &Who, kind: &str, (x, y): (i64, i64)) -> Reply {
        self.act(&who.key, json!({ "action": "place", "kind": kind, "x": x, "y": y })).await
    }

    /// Puts up a place wherever the rules allow it, now.
    async fn build(&self, who: &Who, kind: &str) -> (i64, i64) {
        let spot = self.free_spots()[0];
        self.place(who, kind, spot).await.ok();
        spot
    }

    /// A `place` move at a time of the test's choosing.
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

    /// Puts up a place this many days from now, wherever the rules allow it.
    fn build_later(&self, who: &Who, kind: &str, days: i64) -> (i64, i64) {
        let spot = self.free_spots()[0];
        self.place_at(who, kind, spot, now() + days * DAY).unwrap();
        spot
    }

    async fn go_in(&self, token: &str, (x, y): (i64, i64)) -> Reply {
        self.post(token, "/api/spaces/door", json!({ "x": x, "y": y })).await
    }

    fn tile(&self, (x, y): (i64, i64)) -> Option<Kind> {
        self.state.db.world_tiles(COMMONS).unwrap().get(&(x as u16, y as u16)).map(|tile| tile.kind)
    }

    /// A view after the pause that lets a change show (W29).
    async fn view(&self) -> Value {
        tokio::time::sleep(Duration::from_millis(1100)).await;
        self.get("", &format!("/api/worlds/{COMMONS}")).await.ok()
    }

    /// What was on The Commons before zones went: four tiles of the old game, none on the highway.
    fn old_city(&self) -> Vec<(i64, i64)> {
        let tiles = self.state.db.world_tiles(COMMONS).unwrap();
        let mut spots = (4..60u16).map(|x| (x, 6u16)).filter(|pos| !tiles.contains_key(pos));
        let kinds = [Kind::Homes, Kind::Shops, Kind::Workshops, Kind::Park];
        let tiles: Vec<Change> = kinds
            .iter()
            .map(|&kind| Change::Set(spots.next().unwrap(), Tile { kind, level: 1, player: None }))
            .collect();
        self.write(COMMONS, &tiles);
        tiles.iter().map(|change| (i64::from(change.pos().0), i64::from(change.pos().1))).collect()
    }

    fn write(&self, world: &str, changes: &[Change]) {
        self.state
            .db
            .in_transaction::<(), rusqlite::Error>(|tx| tx.write_changes(world, changes, now()))
            .unwrap();
    }

    fn step(&self) {
        worlds::step_world(&self.state, COMMONS, now()).unwrap();
    }

    fn jump_to(&self, step: u64) {
        self.state
            .db
            .in_transaction::<(), rusqlite::Error>(|tx| tx.set_step(COMMONS, step))
            .unwrap();
    }

    fn end(&self) -> Option<WorldEnd> {
        self.state.db.world_end(COMMONS).unwrap()
    }

    /// What the keeper's asking does, on this step. Returns the step it will strike.
    fn bring_the_omens(&self) -> u64 {
        let step = self.state.db.world(COMMONS).unwrap().unwrap().step;
        self.state
            .db
            .in_transaction::<(), rusqlite::Error>(|tx| tx.start_disaster(COMMONS, step))
            .unwrap();
        assert_eq!(self.end().unwrap().omen_step, Some(step), "the omens started");
        step + world::OMEN_STEPS
    }

    /// Runs a disaster from being asked for to its last step.
    fn run_a_disaster(&self) {
        self.jump_to(self.bring_the_omens());
        for _ in 0..world::STRIKE_STEPS {
            self.step();
        }
    }

    fn frozen_world(&self, owner: &Who) {
        let seed = 99;
        let land = world::terrain(seed, world::OWN_SIZE);
        let highway = world::highway(seed, &land);
        let new = NewWorld {
            id: "frozen",
            name: "Quiet Bay",
            kind: "own",
            seed,
            size: world::OWN_SIZE,
            owner_id: Some(&owner.id),
            open: true,
            invite_code: Some("frozen-code"),
            highway: &highway,
            now: now(),
        };
        self.state.db.insert_world(&new, worlds::MAX_OWN_WORLDS).unwrap();
    }
}

fn road_beside_the_highway(server: &Server) -> Value {
    let tiles = server.state.db.world_tiles(COMMONS).unwrap();
    for (&(x, y), tile) in &tiles {
        if tile.kind == Kind::Road {
            for (dx, dy) in [(1i32, 0i32), (0, 1), (-1, 0), (0, -1)] {
                let (nx, ny) = (i32::from(x) + dx, i32::from(y) + dy);
                let pos = (nx as u16, ny as u16);
                let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
                if !tiles.contains_key(&pos)
                    && land.at(pos).is_some_and(|ground| ground.letter() == 'g')
                {
                    return json!({ "action": "road", "x": nx, "y": ny });
                }
            }
        }
    }
    panic!("no room for a road");
}

// W13, W20
#[tokio::test]
async fn nobody_can_zone_or_lay_a_park_and_being_refused_costs_nothing() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let before = server.me(&guest).await["moves_left"].clone();
    for action in ["zone", "park"] {
        let body = json!({ "action": action, "kind": "homes", "x": 10, "y": 10 });
        server.act(&guest, body).await.refused(StatusCode::BAD_REQUEST);
        server
            .act(&guest, json!({ "action": action, "x": 10, "y": 10 }))
            .await
            .refused(StatusCode::BAD_REQUEST);
    }
    assert_eq!(server.me(&guest).await["moves_left"], before);
}

// W20, W46: a tile from before stays exactly as it is.
#[tokio::test]
async fn a_tile_of_the_old_city_neither_grows_nor_shrinks() {
    let server = Server::start().await;
    server.old_city();
    let before = server.state.db.world_tiles(COMMONS).unwrap();
    for _ in 0..50 {
        server.step();
    }
    assert_eq!(server.state.db.world_tiles(COMMONS).unwrap(), before);
}

// W28, W46
#[tokio::test]
async fn a_new_commons_is_the_land_and_the_highway_and_nothing_else() {
    let server = Server::start().await;
    let tiles = server.state.db.world_tiles(COMMONS).unwrap();
    let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
    let highway: HashSet<_> = world::highway(world::COMMONS_SEED, &land).into_iter().collect();
    assert_eq!(tiles.len(), highway.len());
    assert!(tiles.iter().all(|(pos, tile)| {
        highway.contains(pos) && tile.kind == Kind::Road && tile.player.is_none()
    }));
}

// W23, W48
#[tokio::test]
async fn the_map_counts_places_and_roads_and_has_no_population() {
    let server = Server::start().await;
    assert_eq!(world::stage_for(9), "hamlet");
    assert_eq!(world::stage_for(10), "village");
    assert_eq!(world::stage_for(49), "village");
    assert_eq!(world::stage_for(50), "town");
    assert_eq!(world::stage_for(200), "city");

    let ann = server.person("Ann").await;
    server.build(&ann, "house").await;
    let view = server.view().await;
    assert_eq!(view["places"], 1);
    assert!(view["roads"].as_i64().unwrap() > 0);
    assert_eq!(view["stage"], "hamlet");
    for gone in ["population", "jobs", "parks"] {
        assert!(view[gone].is_null(), "{gone} is still in the view");
    }
    let cards = server.get("", "/api/worlds").await.ok();
    assert_eq!(cards["worlds"][0]["places"], 1);
    assert!(cards["worlds"][0]["population"].is_null());
    assert_eq!(server.me(&ann.key).await["your_town"], json!({ "roads": 0, "places": 1 }));
}

// W47, W49
#[tokio::test]
async fn a_person_is_told_to_build_a_place_and_an_agent_to_lay_a_road() {
    let server = Server::start().await;
    let guest = server.guest().await;
    let first = server.me(&guest).await;
    server.act(&guest, road_beside_the_highway(&server)).await.ok();
    let then = server.me(&guest).await;
    assert_ne!(first["goal"], then["goal"], "the first road is a rung");

    let ann = server.person("Ann").await;
    let start = server.me(&ann.key).await;
    assert_ne!(start["goal"], first["goal"], "a person doesn't start with a road to lay");
    server.build(&ann, "house").await;
    let housed = server.me(&ann.key).await;
    assert_ne!(housed["goal"], start["goal"], "the house is a rung");
    assert_ne!(housed["tip"], start["tip"], "the tip moves on too");

    for reply in [first, then, start, housed] {
        assert!(world::GOALS.contains(&reply["goal"].as_str().unwrap()), "{reply}");
        assert!(world::TIPS.contains(&reply["tip"].as_str().unwrap()), "{reply}");
    }
}

// W24
#[tokio::test]
async fn only_the_commons_steps() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    server.frozen_world(&ann);
    assert_eq!(worlds::worlds_due(&server.state, now()).unwrap(), vec![COMMONS.to_string()]);
}

// W25
#[tokio::test]
async fn nobody_can_start_a_world() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    server.post(&ann.key, "/api/worlds", json!({ "open": true })).await.refused(StatusCode::GONE);
    server.post("", "/api/worlds", json!({})).await.refused(StatusCode::GONE);
    let cards = server.get("", "/api/worlds").await.ok();
    assert_eq!(cards["worlds"].as_array().unwrap().len(), 1);
}

// W27
#[tokio::test]
async fn a_world_someone_started_is_frozen_but_can_be_watched() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    server.frozen_world(&ann);
    let old = Change::Set((3, 3), Tile { kind: Kind::Homes, level: 2, player: None });
    server.write("frozen", &[old]);
    let before = server.state.db.world_tiles("frozen").unwrap();

    let path = "/api/worlds/frozen/act";
    let guest = server.guest().await;
    for (token, body) in [
        (ann.key.as_str(), json!({ "action": "road", "x": 3, "y": 4 })),
        (ann.key.as_str(), json!({ "action": "bulldoze", "x": 3, "y": 3 })),
        (guest.as_str(), json!({ "action": "walk", "x": 3, "y": 4 })),
        (guest.as_str(), json!({ "action": "emote", "emote": "wave" })),
    ] {
        server.post(token, path, body).await.refused(StatusCode::FORBIDDEN);
    }
    server
        .post("", "/api/public/join", json!({ "world_id": "frozen" }))
        .await
        .refused(StatusCode::FORBIDDEN);
    server
        .post(&guest, "/api/worlds/join/frozen-code", json!({}))
        .await
        .refused(StatusCode::FORBIDDEN);

    worlds::step_all(&server.state, now());
    assert_eq!(server.state.db.world_tiles("frozen").unwrap(), before);
    let view = server.get("", "/api/worlds/frozen").await.ok();
    assert_eq!(view["step"], 0, "it took no step");
    assert!(view["tiles"].as_array().unwrap().contains(&json!([3, 3, "homes", 2])));
    assert!(
        server.state.db.world_end("frozen").unwrap().is_none(),
        "a disaster isn't for frozen worlds"
    );
}

// W61
#[tokio::test]
async fn a_house_comes_first_and_the_rest_come_with_time() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = || server.free_spots()[0];

    // Before their first house there's no clock, however old the account.
    for kind in ["studio", "club", "cafe"] {
        server.place(&ann, kind, spot()).await.refused(StatusCode::CONFLICT);
    }
    server.build(&ann, "house").await;
    for kind in ["studio", "club", "cafe"] {
        let refused = server.place(&ann, kind, spot()).await.refused(StatusCode::CONFLICT);
        assert!(!text(&refused["error"]).contains("Ann"), "{refused}");
    }

    let soon = now();
    for (kind, days) in [("studio", 1), ("club", 3), ("cafe", 7)] {
        let early = server.place_at(&ann, kind, spot(), soon + days * DAY - 60);
        assert_eq!(early.unwrap_err().status.as_u16(), StatusCode::CONFLICT.as_u16(), "{kind}");
        server.place_at(&ann, kind, spot(), soon + days * DAY + 60).unwrap();
    }
}

// W61
#[tokio::test]
async fn the_clock_keeps_going_once_it_has_started() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let house = server.build(&ann, "house").await;
    let since = server
        .state
        .db
        .place_clock(COMMONS, server.state.db.own_player(&ann.id, now()).unwrap().id);
    assert!(since.unwrap().is_some());

    // Taking the house down and putting it back doesn't start the wait again.
    server.act(&ann.key, json!({ "action": "bulldoze", "x": house.0, "y": house.1 })).await.ok();
    server.build(&ann, "house").await;
    server.act(&ann.key, json!({ "action": "bulldoze", "x": house.0, "y": house.1 })).await;
    server.build_later(&ann, "studio", 2);

    // One person's clock is theirs.
    let bo = server.person("Bo").await;
    server.build(&bo, "house").await;
    let at = now() + DAY - 60;
    assert!(server.place_at(&bo, "studio", server.free_spots()[0], at).is_err());
}

// W62
#[tokio::test]
async fn a_person_is_told_when_each_place_comes_and_a_bot_is_told_nothing() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let before = server.me(&ann.key).await;
    assert_eq!(before["places"]["house"], 0);
    assert_eq!(before["places"]["studio"], DAY);
    assert_eq!(before["places"]["club"], 3 * DAY);
    assert_eq!(before["places"]["cafe"], 7 * DAY);

    server.build(&ann, "house").await;
    let after = server.me(&ann.key).await;
    let left = after["places"]["studio"].as_i64().unwrap();
    assert!(left <= DAY && left > DAY - 60, "{after}");
    assert!(after["places"]["cafe"].as_i64().unwrap() > 6 * DAY, "{after}");

    let guest = server.guest().await;
    assert!(server.me(&guest).await["places"].is_null());
}

// W63, S47, W92: every place but a house is shared unless its owner closes it.
#[tokio::test]
async fn every_place_but_a_house_is_shared_and_a_house_is_private_unless_you_are_invited() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let stranger = server.person("Stranger").await;
    let house = server.build(&ann, "house").await;
    let studio = server.build_later(&ann, "studio", 8);
    let club = server.build_later(&ann, "club", 8);
    let cafe = server.build_later(&ann, "cafe", 8);

    server.go_in(&ann.key, house).await.ok();
    server.go_in(&stranger.key, house).await.refused(StatusCode::NOT_FOUND);

    for spot in [studio, club, cafe] {
        let room = text(&server.go_in(&ann.key, spot).await.ok()["hangout_id"]);
        assert_eq!(text(&server.go_in(&stranger.key, spot).await.ok()["hangout_id"]), room);
        server.post(&stranger.key, &format!("/api/floors/{room}/walk-in"), json!({})).await.ok();
        let listed = server.get(&stranger.key, "/api/spaces/public").await.ok();
        let ids: Vec<String> =
            listed["floors"].as_array().unwrap().iter().map(|f| text(&f["id"])).collect();
        assert!(ids.contains(&room), "{listed}");
    }

    // The map doesn't say which is which (W59).
    let view = server.view().await.to_string();
    assert!(!view.contains("access") && !view.contains("private"), "{view}");
}

// W63, S47: a room that comes back through a new place's door stays as its owner left it.
#[tokio::test]
async fn a_cafe_is_shut_until_its_owner_has_been_through_its_door() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let stranger = server.person("Stranger").await;
    server.build(&ann, "house").await;
    let cafe = server.build_later(&ann, "cafe", 8);
    server.go_in(&stranger.key, cafe).await.refused(StatusCode::NOT_FOUND);
}

// S47
#[tokio::test]
async fn placing_a_cafe_never_opens_a_room_someone_already_has() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let stranger = server.person("Stranger").await;
    server.build(&ann, "house").await;

    // A studio's room, with something said in it, and then the studio comes down.
    let studio = server.build_later(&ann, "studio", 8);
    let room = text(&server.go_in(&ann.key, studio).await.ok()["hangout_id"]);
    server
        .post(&ann.key, &format!("/api/hangouts/{room}/messages"), json!({ "text": "ours" }))
        .await
        .ok();
    // W92: it started open, and Ann closed it.
    server.put(&ann.key, &format!("/api/floors/{room}"), json!({ "access": "private" })).await.ok();
    server.act(&ann.key, json!({ "action": "bulldoze", "x": studio.0, "y": studio.1 })).await.ok();

    let cafe = server.build_later(&ann, "cafe", 8);
    assert_eq!(
        text(&server.go_in(&ann.key, cafe).await.ok()["hangout_id"]),
        room,
        "the room came back"
    );
    server.go_in(&stranger.key, cafe).await.refused(StatusCode::NOT_FOUND);
    let floor = server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
    assert_eq!(floor["floor"]["floor"]["access"], "private", "{floor}");
}

// S48
#[tokio::test]
async fn a_cafe_owner_opens_and_closes_its_floors_with_no_code_and_nobody_else_gets_that() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let stranger = server.person("Stranger").await;
    let house = server.build(&ann, "house").await;
    let cafe = server.build_later(&ann, "cafe", 8);
    let room = text(&server.go_in(&ann.key, cafe).await.ok()["hangout_id"]);
    let path = format!("/api/floors/{room}");

    server.put(&ann.key, &path, json!({ "access": "private" })).await.ok();
    server.go_in(&stranger.key, cafe).await.refused(StatusCode::NOT_FOUND);
    server.put(&ann.key, &path, json!({ "access": "public" })).await.ok();
    server.go_in(&stranger.key, cafe).await.ok();

    // W92: any place but a house: a studio's owner needs no code either, and a house's does (S16).
    let studio = server.build_later(&ann, "studio", 8);
    let other = text(&server.go_in(&ann.key, studio).await.ok()["hangout_id"]);
    server
        .put(&ann.key, &format!("/api/floors/{other}"), json!({ "access": "private" }))
        .await
        .ok();
    server.put(&ann.key, &format!("/api/floors/{other}"), json!({ "access": "public" })).await.ok();
    let home = text(&server.go_in(&ann.key, house).await.ok()["hangout_id"]);
    let refused =
        server.put(&ann.key, &format!("/api/floors/{home}"), json!({ "access": "public" })).await;
    assert_ne!(refused.status, StatusCode::OK, "{}", refused.body);

    // And a café that's gone from the map is any other third space.
    server.put(&ann.key, &path, json!({ "access": "private" })).await.ok();
    server.act(&ann.key, json!({ "action": "bulldoze", "x": cafe.0, "y": cafe.1 })).await.ok();
    let refused = server.put(&ann.key, &path, json!({ "access": "public" })).await;
    assert_ne!(refused.status, StatusCode::OK, "{}", refused.body);
    server.put(&ann.key, &path, json!({ "access": "public", "create_code": CODE })).await.ok();
}

// S49
#[tokio::test]
async fn on_a_server_with_a_code_a_room_starts_from_the_map() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    assert_eq!(server.get("", "/api/server").await.ok(), json!({ "asks_for_code": true }));

    server
        .post(&ann.key, "/api/hangouts", json!({ "name": "Off the map" }))
        .await
        .refused(StatusCode::FORBIDDEN);
    server
        .post(&ann.key, "/api/cities", json!({ "name": "Elm" }))
        .await
        .refused(StatusCode::FORBIDDEN);

    let house = server.build(&ann, "house").await;
    let room = text(&server.go_in(&ann.key, house).await.ok()["hangout_id"]);
    server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
}

// S48, S8: whoever runs a cafe after its owner has left isn't the person who waited for it.
#[tokio::test]
async fn only_the_owner_of_a_cafe_opens_it_with_no_code() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    server.build(&ann, "house").await;
    let cafe = server.build_later(&ann, "cafe", 8);
    let room = text(&server.go_in(&ann.key, cafe).await.ok()["hangout_id"]);
    let snapshot = server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
    let code = text(&snapshot["hangout"]["invite_code"]);
    let bo = who(&server
        .post("", &format!("/api/invites/{code}/join"), json!({ "your_name": "Bo" }))
        .await
        .ok());
    server.post(&ann.key, &format!("/api/hangouts/{room}/leave"), json!({})).await.ok();

    let path = format!("/api/floors/{room}");
    server.put(&bo.key, &path, json!({ "access": "private" })).await.ok();
    server.put(&bo.key, &path, json!({ "access": "public" })).await.refused(StatusCode::FORBIDDEN);
    server.put(&bo.key, &path, json!({ "access": "public", "create_code": CODE })).await.ok();
}

// W64
#[tokio::test]
async fn nothing_starts_a_disaster_but_the_keeper_asking() {
    let server = Server::start().await;
    server.old_city();
    let ann = server.person("Ann").await;
    server.build(&ann, "house").await;
    let before = server.state.db.world_tiles(COMMONS).unwrap();
    server.jump_to(100_000);
    for _ in 0..50 {
        server.step();
    }
    assert!(server.end().is_none(), "steps and old tiles started one");
    assert_eq!(server.state.db.world_tiles(COMMONS).unwrap(), before);
    assert!(server.view().await["disaster"].is_null());
}

// W64, W65, W69
#[tokio::test]
async fn only_the_keeper_can_ask_for_a_disaster_and_only_when_none_is_under_way() {
    let (server, keeper) = Server::start_with_keeper().await;
    let path = format!("/api/worlds/{COMMONS}/disaster");
    let ann = server.person("Ann").await;
    let guest = server.guest().await;

    server.post(&ann.key, &path, json!({})).await.refused(StatusCode::FORBIDDEN);
    server.post(&guest, &path, json!({})).await.refused(StatusCode::UNAUTHORIZED);
    server.post("", &path, json!({})).await.refused(StatusCode::UNAUTHORIZED);
    assert!(server.end().is_none(), "a refused ask started something");

    let tools = server
        .post("", "/public/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .await
        .ok();
    assert!(!tools.to_string().contains("\"disaster\""), "{tools}");

    server.frozen_world(&keeper);
    server
        .post(&keeper.key, "/api/worlds/frozen/disaster", json!({}))
        .await
        .refused(StatusCode::FORBIDDEN);
    assert!(server.state.db.world_end("frozen").unwrap().is_none());

    let asked = server.post(&keeper.key, &path, json!({})).await.ok();
    let kind = world::disaster_for(world::COMMONS_SEED, 0);
    assert_eq!(asked, json!({ "ok": true, "kind": kind.as_str() }));
    server.post(&keeper.key, &path, json!({})).await.refused(StatusCode::CONFLICT);
    assert_eq!(server.view().await["disaster"]["stage"], "omen");

    // Once it's over it can be asked for again.
    server.jump_to(world::OMEN_STEPS);
    for _ in 0..world::STRIKE_STEPS {
        server.step();
    }
    assert!(server.end().unwrap().ended_step.is_some());
    server.post(&keeper.key, &path, json!({})).await.ok();
    assert_eq!(server.end().unwrap().ended_step, None);
}

// W65
#[test]
fn which_disaster_it_is_is_random_and_can_be_worked_out_again() {
    let mut seen = HashSet::new();
    for step in 0..3_000 {
        seen.insert(world::disaster_for(world::COMMONS_SEED, step));
        assert_eq!(
            world::disaster_for(world::COMMONS_SEED, step),
            world::disaster_for(world::COMMONS_SEED, step)
        );
    }
    assert_eq!(seen.len(), Disaster::ALL.len(), "every disaster can come");
}

// W19, W66, W67: 5 minutes of warning, then steps of 5 seconds until it's over in about 2 minutes.
#[tokio::test]
async fn a_disaster_gives_five_minutes_of_warning_and_is_over_in_two() {
    assert_eq!((world::OMEN_STEPS, world::STRIKE_STEPS), (5, 24));
    let server = Server::start().await;
    assert_eq!(worlds::step_every(&server.state).unwrap(), 60);
    let strikes = server.bring_the_omens();
    assert_eq!(worlds::step_every(&server.state).unwrap(), 60, "omens go at the street's pace");
    server.jump_to(strikes);
    assert_eq!(worlds::step_every(&server.state).unwrap(), 5);
    for _ in 0..world::STRIKE_STEPS {
        server.step();
    }
    assert_eq!(worlds::step_every(&server.state).unwrap(), 60);
}

// W66
#[tokio::test]
async fn omens_say_what_is_coming_and_moves_still_work() {
    let server = Server::start().await;
    server.old_city();
    let strikes = server.bring_the_omens();
    let asked = server.end().unwrap().clock_step;
    let kind = world::disaster_for(world::COMMONS_SEED, asked);
    let view = server.view().await;
    assert_eq!(view["disaster"], json!({ "stage": "omen", "kind": kind.as_str() }));

    let guest = server.guest().await;
    server.act(&guest, road_beside_the_highway(&server)).await.ok();

    // The last step of the omens still isn't the first of the strike.
    server.jump_to(strikes - 1);
    server.step();
    assert!(server
        .state
        .db
        .world_tiles(COMMONS)
        .unwrap()
        .values()
        .any(|tile| tile.kind == Kind::Homes));
}

// W67
#[tokio::test]
async fn it_strikes_step_by_step_and_nobody_can_move_while_it_does() {
    let server = Server::start().await;
    server.old_city();
    let guest = server.guest().await;
    let strikes = server.bring_the_omens();
    server.jump_to(strikes);

    // Nothing has struck yet, so there's nothing to draw.
    let view = server.view().await;
    assert_eq!(view["disaster"]["stage"], "striking");
    assert_eq!(view["disaster"]["done"], 0);
    assert_eq!(view["disaster"]["hits"], json!([]));

    // A tile at the heart of the first step's circles, off the highway, for the page to find gone.
    let kind = world::disaster_for(world::COMMONS_SEED, server.end().unwrap().clock_step);
    let hits = world::strike(world::COMMONS_SEED, kind, 0, world::COMMONS_SIZE);
    let tiles = server.state.db.world_tiles(COMMONS).unwrap();
    let (x, y) = hits
        .iter()
        .flat_map(|&(x, y, _)| {
            [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1)].map(|(dx, dy)| (x + dx, y + dy))
        })
        .find(|&(x, y)| !tiles.contains_key(&(x as u16, y as u16)))
        .expect("a circle with room in it");
    let target = Tile { kind: Kind::Homes, level: 2, player: None };
    server.write(COMMONS, &[Change::Set((x as u16, y as u16), target)]);

    server.step();
    assert_eq!(server.tile((x, y)), None, "it came down");

    let view = server.view().await;
    let disaster = &view["disaster"];
    assert_eq!(disaster["stage"], "striking");
    assert_eq!(disaster["done"], 1);
    assert_eq!(disaster["of"], world::STRIKE_STEPS);
    // What a page draws has already happened.
    assert_eq!(disaster["hits"], json!(hits), "{disaster}");

    server
        .act(&guest, json!({ "action": "emote", "emote": "wave" }))
        .await
        .refused(StatusCode::CONFLICT);
    server.act(&guest, road_beside_the_highway(&server)).await.refused(StatusCode::CONFLICT);
    let reply = server.get(&guest, &format!("/api/worlds/{COMMONS}/look")).await;
    reply.ok();

    let tip = server.me(&guest).await;
    assert!(world::TIPS.contains(&tip["tip"].as_str().unwrap()));
    assert!(text(&tip["tip"]).contains("striking"), "{tip}");
}

// W67: meteors fall where the city is, not on the empty edges of the map.
#[test]
fn meteors_fall_on_the_middle_half_of_the_map() {
    let size = world::COMMONS_SIZE as i64;
    for done in 0..world::STRIKE_STEPS {
        for (x, y, _) in
            world::strike(world::COMMONS_SEED, Disaster::Meteors, done, world::COMMONS_SIZE)
        {
            assert!(
                (size / 4..size * 3 / 4).contains(&x) && (size / 4..size * 3 / 4).contains(&y),
                "{x},{y}"
            );
        }
    }
}

// W67: a meteor's fire keeps spreading from where it came down, and more of them come down.
#[test]
fn a_meteors_fire_spreads_until_it_has_taken_the_city() {
    let at =
        |done| world::strike(world::COMMONS_SEED, Disaster::Meteors, done, world::COMMONS_SIZE);
    assert_eq!(at(0).len(), 1, "one comes down first");
    assert!(at(7).len() > at(3).len(), "and more follow");
    for done in 1..world::STRIKE_STEPS {
        let (before, now) = (at(done - 1), at(done));
        for (old, new) in before.iter().zip(&now) {
            assert_eq!((old.0, old.1), (new.0, new.1), "a fire stays where it started");
            assert!(new.2 > old.2, "and it grows");
        }
    }
    // By half way the fire has reached everywhere the middle of the map was built.
    let reach = at(world::STRIKE_STEPS / 2);
    let city = (0..=127i64).flat_map(|y| (0..=127i64).map(move |x| (x, y)));
    let covered = city
        .filter(|&(x, y)| (38..=77).contains(&x) && (45..=78).contains(&y))
        .all(|(x, y)| reach.iter().any(|&(hx, hy, r)| (x - hx).pow(2) + (y - hy).pow(2) <= r * r));
    assert!(covered, "{reach:?}");
}

// W67: a fire's edge is rough, and it never goes past its circle.
#[test]
fn a_fire_has_a_rough_edge_inside_its_circle() {
    let road = Tile { kind: Kind::Road, level: 0, player: Some(1) };
    let mut tiles = world::Tiles::new();
    for x in 0..=40u16 {
        for y in 0..=40u16 {
            tiles.insert((x, y), road);
        }
    }
    let gone: HashSet<_> = world::strike_changes(&tiles, &[], &[(20, 20, 12)], false)
        .iter()
        .map(Change::pos)
        .collect();
    let distance = |&(x, y): &(u16, u16)| (i64::from(x) - 20).pow(2) + (i64::from(y) - 20).pow(2);
    assert!(gone.iter().all(|pos| distance(pos) <= 144), "past its circle");
    assert!(
        tiles.keys().filter(|pos| distance(pos) <= 100).all(|pos| gone.contains(pos)),
        "the middle went"
    );
    let edge = tiles.keys().filter(|pos| (101..=144).contains(&distance(pos)));
    let (burned, left) =
        edge.fold(
            (0, 0),
            |(burned, left), pos| {
                if gone.contains(pos) {
                    (burned + 1, left)
                } else {
                    (burned, left + 1)
                }
            },
        );
    assert!(burned > 0 && left > 0, "{burned} burned and {left} left of a perfect ring");
}

// W67, W31
#[test]
fn every_disaster_strikes_on_the_map_in_a_fixed_number_of_circles() {
    let size = world::COMMONS_SIZE as i64;
    for kind in Disaster::ALL {
        let mut sum = 0;
        for done in 0..world::STRIKE_STEPS {
            let hits = world::strike(world::COMMONS_SEED, kind, done, world::COMMONS_SIZE);
            assert_eq!(hits, world::strike(world::COMMONS_SEED, kind, done, world::COMMONS_SIZE));
            assert!(!hits.is_empty() && hits.len() <= world::MAX_HITS, "{kind:?} {}", hits.len());
            for (x, y, r) in hits.iter().copied() {
                assert!((0..size).contains(&x) && (0..size).contains(&y), "{kind:?} at {x},{y}");
                assert!((1..=size).contains(&r), "{kind:?} r {r}");
                sum += x + y;
            }
        }
        assert!(sum > 0);
    }
}

// W67
#[test]
fn a_strike_takes_what_is_in_its_circles_and_the_last_takes_the_rest_but_never_the_highway() {
    let road = |player| Tile { kind: Kind::Road, level: 0, player };
    let mut tiles = world::Tiles::new();
    tiles.insert((10, 10), road(None));
    tiles.insert((11, 10), road(Some(1)));
    tiles.insert((30, 30), Tile { kind: Kind::House, level: 0, player: Some(1) });
    let highway = [(10, 10)];

    let hits = [(10, 10, 3)];
    assert_eq!(
        world::strike_changes(&tiles, &highway, &hits, false),
        vec![Change::Remove((11, 10))]
    );
    assert_eq!(
        world::strike_changes(&tiles, &highway, &[], true),
        vec![Change::Remove((11, 10)), Change::Remove((30, 30))],
    );
}

// W68
#[tokio::test]
async fn everything_goes_but_the_highway() {
    let server = Server::start().await;
    let old = server.old_city();
    let ann = server.person("Ann").await;
    let house = server.build(&ann, "house").await;
    let guest = server.guest().await;
    server.act(&guest, road_beside_the_highway(&server)).await.ok();
    let cleared = Change::Set((9, 9), Tile { kind: Kind::Cleared, level: 0, player: Some(1) });
    server.write(COMMONS, &[cleared]);

    server.run_a_disaster();

    let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
    let highway: HashSet<_> = world::highway(world::COMMONS_SEED, &land).into_iter().collect();
    let tiles = server.state.db.world_tiles(COMMONS).unwrap();
    assert_eq!(tiles.keys().copied().collect::<HashSet<_>>(), highway);
    assert_eq!(server.tile(old[0]), None);
    assert_eq!(server.tile(house), None);
}

// W68, W81: the town square has a table of its own, and goes with everything else.
#[tokio::test]
async fn a_disaster_takes_the_town_square_and_leaves_room_for_another() {
    let server = Server::start().await;
    server.old_city();
    let ann = server.person("Ann").await;
    let player = server.state.db.own_player(&ann.id, now()).unwrap();
    let spot = server.free_spots()[0];
    let tile = Tile { kind: Kind::Square, level: 0, player: Some(player.id as u64) };
    server.write(COMMONS, &[Change::Set((spot.0 as u16, spot.1 as u16), tile)]);
    assert_eq!(server.tile(spot), Some(Kind::Square));

    server.run_a_disaster();

    assert_eq!(server.tile(spot), None);
    assert!(!server.state.db.has_square(COMMONS).unwrap());
}

// W68
#[tokio::test]
async fn accounts_tokens_rooms_and_unlocks_all_stay() {
    let server = Server::start().await;
    server.old_city();
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    let house = server.build(&ann, "house").await;
    let room = text(&server.go_in(&ann.key, house).await.ok()["hangout_id"]);
    server
        .post(&ann.key, &format!("/api/hangouts/{room}/messages"), json!({ "text": "still here" }))
        .await
        .ok();
    let later = server.build_later(&ann, "studio", 8);
    let studio = text(&server.go_in(&ann.key, later).await.ok()["hangout_id"]);

    server.run_a_disaster();

    assert!(server.me(&guest).await["player"]["id"].is_i64(), "a token still works");
    server.get(&ann.key, "/api/me").await.ok();
    server.go_in(&ann.key, house).await.refused(StatusCode::NOT_FOUND);

    // Every room is still there, with what was in it, and opens from its owner's list.
    let snapshot = server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
    assert!(snapshot.to_string().contains("still here"), "{snapshot}");
    server.get(&ann.key, &format!("/api/hangouts/{studio}")).await.ok();
    let spaces = server.get(&ann.key, "/api/spaces").await.ok();
    assert_eq!(spaces["house"]["on_map"], false, "{spaces}");

    // The next house is the same room's door.
    let there = server.build(&ann, "house").await;
    assert_eq!(text(&server.go_in(&ann.key, there).await.ok()["hangout_id"]), room);

    // Nobody waits again for what they had. If the clock had started again with that house, a
    // studio wouldn't come for a day.
    assert!(server.me(&ann.key).await["places"]["studio"].as_i64().unwrap() <= DAY);
    let at = now() + DAY - 3_600;
    let spot = server.free_spots()[0];
    assert!(server.place_at(&ann, "studio", spot, at).is_err(), "the first house went up just now");
    let at = now() + DAY + 60;
    server.place_at(&ann, "studio", spot, at).unwrap();
    assert_eq!(text(&server.go_in(&ann.key, spot).await.ok()["hangout_id"]), studio);
}

// W69
#[tokio::test]
async fn it_says_so_for_an_hour_and_then_says_nothing() {
    let server = Server::start().await;
    server.old_city();
    server.run_a_disaster();
    let ended = server.end().unwrap().ended_step.expect("it finished");
    let view = server.view().await;
    assert_eq!(view["disaster"]["stage"], "over");
    assert!(view["disaster"]["hits"].is_null());

    let guest = server.guest().await;
    server.act(&guest, road_beside_the_highway(&server)).await.ok();

    server.jump_to(ended + world::OVER_STEPS);
    server.step();
    assert!(server.view().await["disaster"].is_null());

    // Old tiles on The Commons don't start another.
    server.old_city();
    let before = server.state.db.world_tiles(COMMONS).unwrap();
    for _ in 0..5 {
        server.step();
    }
    assert_eq!(server.end().unwrap().ended_step, Some(ended));
    assert_eq!(server.state.db.world_tiles(COMMONS).unwrap(), before);
}

// W70
#[tokio::test]
async fn a_restart_in_the_middle_of_a_strike_carries_on_where_it_was() {
    let path = temp_db();
    let server = Server::on(Db::open(&path).unwrap()).await;
    server.old_city();
    server.jump_to(server.bring_the_omens());
    server.step();
    server.step();
    let step = server.state.db.world(COMMONS).unwrap().unwrap().step;

    let again = Server::on(Db::open(&path).unwrap()).await;
    assert_eq!(again.state.db.world(COMMONS).unwrap().unwrap().step, step);
    for _ in 2..world::STRIKE_STEPS {
        again.step();
    }
    assert!(again.end().unwrap().ended_step.is_some());
    let _ = std::fs::remove_file(&path);
}

// W1: a disaster touches no room, so the rows that say where a room stood are left alone (S33).
#[tokio::test]
async fn a_disaster_leaves_the_rooms_tables_alone() {
    let server = Server::start().await;
    server.old_city();
    let ann = server.person("Ann").await;
    let house = server.build(&ann, "house").await;
    let room = text(&server.go_in(&ann.key, house).await.ok()["hangout_id"]);
    server.run_a_disaster();
    let plot = server.state.db.plot_of(&room, COMMONS).unwrap();
    assert_eq!(plot, Some(house));
}

impl Server {
    /// W91: spots with room for a park or a square: the three tiles across and down are open too.
    fn big_spots(&self) -> Vec<(i64, i64)> {
        let tiles = self.state.db.world_tiles(COMMONS).unwrap();
        let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
        let open = |(x, y): (i64, i64)| {
            let pos = (x as u16, y as u16);
            !tiles.contains_key(&pos)
                && !world::is_reserved(&tiles, pos)
                && land.at(pos).is_some_and(|ground| ground.is_open())
        };
        self.free_spots()
            .into_iter()
            .filter(|&(x, y)| [(x + 1, y), (x, y + 1), (x + 1, y + 1)].into_iter().all(open))
            .collect()
    }
}

// W90, W92
#[tokio::test]
async fn the_newer_places_go_up_open_named_and_set_up_for_what_they_are() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let stranger = server.person("Stranger").await;
    server.build(&ann, "house").await;
    let early = server.free_spots()[0];
    server.place(&ann, "bar", early).await.refused(StatusCode::CONFLICT);

    for (kind, title, item) in [
        ("office", "Office", "bookshelf"),
        ("workshop", "Workshop", "workbench"),
        ("bar", "Bar", "coffee_table"),
    ] {
        let spot = server.build_later(&ann, kind, 8);
        assert_eq!(server.tile(spot).map(|k| k.as_str()), Some(kind));
        let room = text(&server.go_in(&ann.key, spot).await.ok()["hangout_id"]);
        let floor = server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
        assert_eq!(floor["floor"]["space"]["name"], format!("Ann's {title}"), "{floor}");
        assert_eq!(floor["floor"]["floor"]["access"], "public", "{floor}");
        assert!(
            floor.to_string().contains(&format!("\"{item}\"")),
            "{kind} has no {item}: {floor}"
        );
        server.go_in(&stranger.key, spot).await.ok();
    }
    // That's Ann's 3, whatever their kind (W56).
    let spot = server.free_spots()[0];
    assert!(server.place_at(&ann, "library", spot, now() + 8 * DAY).is_err());
}

// W91
#[tokio::test]
async fn a_park_takes_two_by_two_tiles_and_any_of_them_is_the_park() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    server.build(&ann, "house").await;
    let (x, y) = server.big_spots()[0];
    server.place_at(&ann, "park", (x, y), now() + 8 * DAY).unwrap();

    let view = server.view().await;
    let tiles = view["tiles"].as_array().unwrap();
    assert!(tiles.contains(&json!([x, y, "green", 0])), "{}", view["tiles"]);
    for part in [(x + 1, y), (x, y + 1), (x + 1, y + 1)] {
        assert!(tiles.contains(&json!([part.0, part.1, "green", 1])), "{part:?}");
    }
    assert_eq!(view["places"], 2, "a house and one park");

    // Nothing else goes on it, and its door is on all four.
    assert!(server.place_at(&ann, "cafe", (x + 1, y + 1), now() + 9 * DAY).is_err());
    let room = text(&server.go_in(&ann.key, (x, y)).await.ok()["hangout_id"]);
    let floor = server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
    assert_eq!(floor["floor"]["space"]["name"], "Ann's Park", "{floor}");
    assert_eq!(text(&server.go_in(&ann.key, (x + 1, y + 1)).await.ok()["hangout_id"]), room);

    // Taking down any of its tiles takes the park.
    server.act(&ann.key, json!({ "action": "bulldoze", "x": x + 1, "y": y })).await.ok();
    assert_eq!(server.tile((x, y)), None);
    assert_eq!(server.tile((x + 1, y + 1)), None);
}

// W91: the map keeps a big place's level, so it draws each tile as its own piece of one place and
// a hover on any of them finds the place. With every level read as 0, a square's second tile drew
// as a second square, and hovering it named no owner.
#[test]
fn the_map_keeps_the_level_of_a_big_places_tiles() {
    let map = include_str!("../web/world.js").replace("\r\n", "\n");
    let start = map.find("  applyTiles(list, animate) {").unwrap();
    let body = &map[start..start + map[start..].find("\n  }\n").unwrap()];
    let kept = body
        .lines()
        .find(|line| line.contains("nl[i] ="))
        .expect("applyTiles sets each tile's level");
    assert!(kept.contains("isBig(k)"), "a big place's level is read as 0: {kept}");
}

// W91: a big place is one place to look at. Hovering any tile outlines all four, and the square is
// one design across them, with one jet, not a fountain on each tile.
#[test]
fn a_big_place_is_outlined_whole_and_the_square_has_one_fountain() {
    let map = include_str!("../web/world.js").replace("\r\n", "\n");
    let hover = map.find("hovering a place outlines all of it").expect("the hover outline");
    assert!(map[hover..hover + 400].contains("this.placeAt(this.tile.x, this.tile.y)"));
    let start = map.find("function squarePiece(corner) {").unwrap();
    let piece = &map[start..start + map[start..].find("\n}\n").unwrap()];
    assert!(!piece.contains("fountain("), "each quarter draws a whole fountain");
    assert_eq!(piece.matches("if (corner === 3)").count(), 1, "one jet, on the last quarter drawn");
}
