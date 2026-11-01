//! Tests for what people can build on The Commons, the God mode switch and the town square. They are
//! written from the rules in docs/public-city.md (W16, W56, W62, W75 to W82) and docs/routines.md
//! (L18 to L20). A comment such as `// W76` above a test names the rule it covers.
//!
//! The people with the switch are made before the server starts, because `AGENTHANGOUT_GODS` is read
//! then. The switch itself is always flipped through the route.

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use agenthangout::{
    app, auth,
    db::Db,
    now, routines,
    world::{self, Ground, Kind},
    worlds::{self, Move},
    AppState, Config,
};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};

const COMMONS: &str = "commons";
const CODE: &str = "letmein";
const DAY: i64 = 86_400;
const JUSTIN: &str = "god0000000000001";
const MORG: &str = "god0000000000002";

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

/// Someone who is on the list once the server starts.
fn make_god(db: &Db, id: &str, name: &str) -> Who {
    let key = auth::new_owner_key();
    db.create_person(id, name, &auth::hash_secret(&key), &auth::new_feed_token(), now()).unwrap();
    Who { key, id: id.to_string() }
}

impl Server {
    async fn on(db: Db, god_ids: &[&str]) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let state = AppState::new(
            db,
            Config {
                public_url: base.clone(),
                create_code: Some(CODE.to_string()),
                allow_private_webhooks: true,
                keeper_id: None,
                god_ids: god_ids.iter().map(|id| id.to_string()).collect(),
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        worlds::ensure_commons(&state).unwrap();
        Self { base, client: Client::new(), state, addresses: AtomicUsize::new(1) }
    }

    /// A server where Justin and Morg have the switch, and both have left it off.
    async fn start() -> (Self, Who, Who) {
        let db = Db::open(":memory:").unwrap();
        let (justin, morg) = (make_god(&db, JUSTIN, "Justin"), make_god(&db, MORG, "Morg"));
        (Self::on(db, &[JUSTIN, MORG]).await, justin, morg)
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

    /// Someone nobody listed, and their first room.
    async fn someone(&self, name: &str) -> (String, Who) {
        let body =
            json!({ "name": format!("{name}'s room"), "your_name": name, "create_code": CODE });
        let made = self.post("", "/api/hangouts", body).await.ok();
        (
            text(&made["hangout"]["id"]),
            Who { key: text(&made["owner_key"]), id: text(&made["person"]["id"]) },
        )
    }

    async fn person(&self, name: &str) -> Who {
        self.someone(name).await.1
    }

    async fn guest(&self) -> String {
        text(&self.post("", "/api/public/join", json!({})).await.ok()["player_token"])
    }

    /// A bot of this person's, with its id and its token.
    async fn bot(&self, owner: &Who, room: &str, name: &str) -> (String, String) {
        let made = self
            .post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await
            .ok();
        (text(&made["agent"]["id"]), text(&made["token"]))
    }

    async fn switch(&self, who: &Who, on: bool) {
        let done = self.put(&who.key, "/api/god-mode", json!({ "on": on })).await.ok();
        assert_eq!(done["on"], on);
    }

    async fn act(&self, token: &str, body: Value) -> Reply {
        self.post(token, &format!("/api/worlds/{COMMONS}/act"), body).await
    }

    async fn me(&self, token: &str) -> Value {
        self.get(token, &format!("/api/worlds/{COMMONS}/me")).await.ok()
    }

    /// Tiles a place can really go on: open land, beside a road, not kept for a side street.
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

    /// W91: spots a square or a park can go on, with the three tiles across and down open too.
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

    fn highway_tile(&self) -> (i64, i64) {
        let tiles = self.state.db.world_tiles(COMMONS).unwrap();
        let (&(x, y), _) = tiles
            .iter()
            .find(|(_, tile)| tile.kind == Kind::Road && tile.player.is_none())
            .unwrap();
        (i64::from(x), i64::from(y))
    }

    /// An open tile beside a road, for a road.
    fn road_spot(&self) -> (i64, i64) {
        let tiles = self.state.db.world_tiles(COMMONS).unwrap();
        let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
        let mut beside =
            tiles.iter().filter(|(_, tile)| tile.kind == Kind::Road).flat_map(|(&(x, y), _)| {
                let (x, y) = (i64::from(x), i64::from(y));
                [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]
            });
        beside
            .find(|at| {
                let pos = (at.0 as u16, at.1 as u16);
                !tiles.contains_key(&pos) && land.at(pos).is_some_and(|ground| ground.is_open())
            })
            .expect("room for a road")
    }

    /// A straight line of open land out from a road, the first and last of it, and the open tile
    /// past its end, which is out of reach of every other road, so only that line would serve it.
    fn branch(&self) -> ((i64, i64), (i64, i64), (i64, i64)) {
        let tiles = self.state.db.world_tiles(COMMONS).unwrap();
        let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
        let open = |(x, y): (i64, i64)| {
            let pos = (x as u16, y as u16);
            !tiles.contains_key(&pos) && land.at(pos).is_some_and(|ground| ground.is_open())
        };
        let roads: Vec<(i64, i64)> = tiles
            .iter()
            .filter(|(_, tile)| tile.kind == Kind::Road)
            .map(|(&(x, y), _)| (i64::from(x), i64::from(y)))
            .collect();
        for &(x, y) in &roads {
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let line: Vec<(i64, i64)> = (1..=9).map(|i| (x + dx * i, y + dy * i)).collect();
                let end = line[8];
                let far = roads
                    .iter()
                    .all(|road| (road.0 - end.0).abs() > 5 || (road.1 - end.1).abs() > 5);
                if far && line.iter().all(|at| open(*at)) {
                    return (line[0], line[7], end);
                }
            }
        }
        panic!("no room for a branch");
    }

    async fn place(&self, who: &Who, kind: &str, (x, y): (i64, i64)) -> Reply {
        self.act(&who.key, json!({ "action": "place", "kind": kind, "x": x, "y": y })).await
    }

    /// Puts up a place wherever the rules allow it.
    async fn build(&self, who: &Who, kind: &str) -> (i64, i64) {
        let spot = if kind == "square" { self.big_spots()[0] } else { self.free_spots()[0] };
        self.place(who, kind, spot).await.ok();
        spot
    }

    /// Puts up a place this many days from now, wherever the rules allow it.
    fn build_later(&self, who: &Who, kind: &str, days: i64) -> (i64, i64) {
        let at = now() + days * DAY;
        let player = self.state.db.own_player(&who.id, at).unwrap();
        let (x, y) = self.free_spots()[0];
        let mv = Move {
            action: "place".to_string(),
            x: Some(x),
            y: Some(y),
            x2: None,
            y2: None,
            kind: Some(kind.to_string()),
            emote: None,
        };
        worlds::act(&self.state, &player, COMMONS, &mv, at).unwrap();
        (x, y)
    }

    async fn clear(&self, who: &Who, (x, y): (i64, i64)) -> Reply {
        self.act(&who.key, json!({ "action": "bulldoze", "x": x, "y": y })).await
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

    async fn all_day(&self, owner: &Who, agent_id: &str, public_places: bool) {
        let body = json!({ "days": ["mon", "tue", "wed", "thu", "fri", "sat", "sun"], "starts": "00:00", "ends": "24:00", "public_places": public_places });
        self.put(&owner.key, &format!("/api/agents/{agent_id}/routine"), body).await.ok();
    }

    async fn out_at(&self, token: &str) -> Value {
        self.get(token, "/api/me").await.ok()["routine"]["out_at"].clone()
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

    /// The town square, up and open: the god's switch is on from here.
    async fn town_square(&self, god: &Who) -> ((i64, i64), String) {
        self.switch(god, true).await;
        let spot = self.build(god, "square").await;
        (spot, text(&self.go_in(&god.key, spot).await.ok()["hangout_id"]))
    }
}

fn road_move(at: (i64, i64)) -> Value {
    json!({ "action": "road", "x": at.0, "y": at.1 })
}

// W75
#[tokio::test]
async fn only_the_people_named_have_the_switch_and_nobody_else_hears_of_it() {
    let (server, justin, morg) = Server::start().await;
    for god in [&justin, &morg] {
        assert_eq!(server.get(&god.key, "/api/me").await.ok()["god_mode"], false, "it starts off");
    }

    let (room, stranger) = server.someone("Stranger").await;
    let (_, stranger_bot) = server.bot(&stranger, &room, "Rover").await;
    let hall = server
        .post(&justin.key, "/api/hangouts", json!({ "name": "Hall", "create_code": CODE }))
        .await
        .ok();
    let (_, justins_bot) = server.bot(&justin, &text(&hall["hangout"]["id"]), "Fern").await;
    let guest = server.guest().await;

    assert!(server.get(&stranger.key, "/api/me").await.ok().get("god_mode").is_none());
    for bot in [&stranger_bot, &justins_bot] {
        assert!(server.get(bot, "/api/me").await.ok().get("god_mode").is_none());
    }

    // Everyone else gets the 404 of a route that isn't there, even Justin's own bot, and a bad body
    // doesn't tell them otherwise.
    for token in [&stranger.key, &stranger_bot, &justins_bot] {
        server
            .put(token, "/api/god-mode", json!({ "on": true }))
            .await
            .refused(StatusCode::NOT_FOUND);
        server
            .put(token, "/api/god-mode", json!({ "nonsense": 1 }))
            .await
            .refused(StatusCode::NOT_FOUND);
    }
    server
        .put(&justin.key, "/api/god-mode", json!({ "nonsense": 1 }))
        .await
        .refused(StatusCode::BAD_REQUEST);
    server
        .put(&guest, "/api/god-mode", json!({ "on": true }))
        .await
        .refused(StatusCode::UNAUTHORIZED);
    server.put("", "/api/god-mode", json!({ "on": true })).await.refused(StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&justin.key, "/api/me").await.ok()["god_mode"], false);

    // Each person's switch is their own.
    server.switch(&justin, true).await;
    assert_eq!(server.get(&justin.key, "/api/me").await.ok()["god_mode"], true);
    assert_eq!(server.get(&morg.key, "/api/me").await.ok()["god_mode"], false);
    server.switch(&justin, false).await;
    assert_eq!(server.get(&justin.key, "/api/me").await.ok()["god_mode"], false);
}

// W75
#[tokio::test]
async fn the_switch_stays_as_it_was_left_until_someone_is_off_the_list() {
    let path = std::env::temp_dir().join(format!("agenthangout-test-{}.db", auth::new_id()));
    let path = path.to_string_lossy().into_owned();
    let db = Db::open(&path).unwrap();
    let justin = make_god(&db, JUSTIN, "Justin");
    let first = Server::on(db, &[JUSTIN]).await;
    first.switch(&justin, true).await;

    let again = Server::on(Db::open(&path).unwrap(), &[JUSTIN]).await;
    assert_eq!(again.get(&justin.key, "/api/me").await.ok()["god_mode"], true, "through a restart");
    assert_eq!(again.me(&justin.key).await["god_mode"], true);

    // Off the list it's off, whatever the table says.
    let off = Server::on(Db::open(&path).unwrap(), &[]).await;
    assert!(off.get(&justin.key, "/api/me").await.ok().get("god_mode").is_none());
    assert!(off.me(&justin.key).await.get("god_mode").is_none());
    off.act(&justin.key, road_move(off.road_spot())).await.refused(StatusCode::FORBIDDEN);
    off.put(&justin.key, "/api/god-mode", json!({ "on": false }))
        .await
        .refused(StatusCode::NOT_FOUND);
}

// W16
#[tokio::test]
async fn people_dont_lay_roads_and_agents_still_do() {
    let (server, justin, _) = Server::start().await;
    let road = server.road_spot();

    let ann = server.person("Ann").await;
    for who in [&ann, &justin] {
        let before = server.me(&who.key).await["moves_left"].clone();
        server.act(&who.key, road_move(road)).await.refused(StatusCode::FORBIDDEN);
        assert_eq!(server.me(&who.key).await["moves_left"], before, "a refused move costs nothing");
    }
    assert_eq!(server.tile(road), None);

    let guest = server.guest().await;
    server.act(&guest, road_move(road)).await.ok();
    assert_eq!(server.tile(road), Some(Kind::Road));
}

// W56, W62
#[tokio::test]
async fn a_house_is_turned_off_once_someone_has_theirs_and_on_again_when_it_comes_down() {
    let (server, _, _) = Server::start().await;
    let ann = server.person("Ann").await;
    assert_eq!(server.me(&ann.key).await["full"], json!([]));

    let house = server.build(&ann, "house").await;
    assert_eq!(server.me(&ann.key).await["full"], json!(["house"]));
    let elsewhere = server.free_spots()[0];
    server.place(&ann, "house", elsewhere).await.refused(StatusCode::CONFLICT);

    server.clear(&ann, house).await.ok();
    assert_eq!(server.me(&ann.key).await["full"], json!([]));
}

// W56, W62
#[tokio::test]
async fn the_other_places_are_turned_off_at_three_and_a_guest_is_told_nothing() {
    let (server, _, _) = Server::start().await;
    let ann = server.person("Ann").await;
    server.build(&ann, "house").await;
    for kind in ["studio", "club", "studio"] {
        assert_eq!(server.me(&ann.key).await["full"], json!(["house"]));
        server.build_later(&ann, kind, 8);
    }
    // W90: and so are the newer kinds, which count toward the same 3.
    let all = [
        "house", "studio", "office", "workshop", "library", "club", "lounge", "cafe", "bar",
        "green",
    ];
    assert_eq!(server.me(&ann.key).await["full"], json!(all));

    let guest = server.guest().await;
    assert!(server.me(&guest).await.get("full").is_none());
}

// W76
#[tokio::test]
async fn god_mode_has_every_place_now_past_the_limits_and_for_nothing() {
    let (server, justin, _) = Server::start().await;
    let cafe = server.free_spots()[0];
    server.place(&justin, "cafe", cafe).await.refused(StatusCode::CONFLICT);

    server.switch(&justin, true).await;
    let me = server.me(&justin.key).await;
    assert_eq!(me["god_mode"], true);
    // W90: the newer kinds too.
    assert_eq!(
        me["places"],
        json!({ "house": 0, "studio": 0, "office": 0, "workshop": 0, "library": 0, "club": 0, "lounge": 0, "cafe": 0, "bar": 0, "green": 0 })
    );
    assert_eq!(me["moves_left"], me["moves_per_day"]);

    // The longest wait, with no house at all, and as many places as they like.
    let done = server.place(&justin, "cafe", cafe).await.ok();
    assert_eq!(done["cost"], 0);
    assert_eq!(done["moves_left"], me["moves_per_day"]);
    for kind in ["studio", "club", "cafe", "cafe"] {
        server.build(&justin, kind).await;
    }
    let after = server.me(&justin.key).await;
    assert_eq!(after["full"], json!([]));
    assert_eq!(after["moves_left"], me["moves_per_day"], "nothing was charged");
    assert_eq!(after["your_town"]["places"], 5);

    // A house is still one, and the toolbar is told so.
    server.build(&justin, "house").await;
    assert_eq!(server.me(&justin.key).await["full"], json!(["house"]));
    let elsewhere = server.free_spots()[0];
    server.place(&justin, "house", elsewhere).await.refused(StatusCode::CONFLICT);

    // Roads too, for nothing.
    let road = server.road_spot();
    let done = server.act(&justin.key, road_move(road)).await.ok();
    assert_eq!(done["cost"], 0);
    assert_eq!(server.tile(road), Some(Kind::Road));
}

// W77
#[tokio::test]
async fn god_mode_takes_down_what_anyone_built_and_their_room_waits_for_them() {
    let (server, justin, _) = Server::start().await;
    let ann = server.person("Ann").await;
    let house = server.build(&ann, "house").await;
    let room = text(&server.go_in(&ann.key, house).await.ok()["hangout_id"]);
    let guest = server.guest().await;
    let road = server.road_spot();
    server.act(&guest, road_move(road)).await.ok();

    // With it off, W17 and W57 hold.
    server.clear(&justin, house).await.refused(StatusCode::BAD_REQUEST);
    server.clear(&justin, road).await.refused(StatusCode::BAD_REQUEST);

    server.switch(&justin, true).await;
    assert_eq!(server.clear(&justin, house).await.ok()["cost"], 0);
    assert_eq!(server.tile(house), None);
    server.clear(&justin, road).await.ok();
    assert_eq!(server.tile(road), None);

    // Ann can put it up again and gets the same room (S35).
    let house = server.build(&ann, "house").await;
    assert_eq!(server.go_in(&ann.key, house).await.ok()["hangout_id"], room.as_str());
}

// W77
#[tokio::test]
async fn god_mode_leaves_the_highway_and_a_road_a_place_needs() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    let highway = server.highway_tile();
    server.clear(&justin, highway).await.refused(StatusCode::BAD_REQUEST);
    assert_eq!(server.tile(highway), Some(Kind::Road));

    let (first, last, spot) = server.branch();
    let line = json!({ "action": "road", "x": first.0, "y": first.1, "x2": last.0, "y2": last.1 });
    server.act(&justin.key, line).await.ok();
    let ann = server.person("Ann").await;
    server.place(&ann, "house", spot).await.ok();
    server.clear(&justin, first).await.refused(StatusCode::BAD_REQUEST);
    assert_eq!(server.tile(first), Some(Kind::Road));

    // The way to move a street: the place goes first.
    server.clear(&justin, spot).await.ok();
    server.clear(&justin, first).await.ok();
    assert_eq!(server.tile(first), None);
}

// W78
#[tokio::test]
async fn god_mode_doesnt_change_the_land_or_say_anything_to_anyone() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
    let size = world::COMMONS_SIZE as u16;
    let all = (0..size).flat_map(|x| (0..size).map(move |y| (x, y)));
    let water = all
        .clone()
        .find(|pos| land.at(*pos) == Some(Ground::Water))
        .expect("The Commons has water");
    let water = (i64::from(water.0), i64::from(water.1));
    server.place(&justin, "house", water).await.refused(StatusCode::BAD_REQUEST);
    server.act(&justin.key, road_move(water)).await.refused(StatusCode::BAD_REQUEST);

    // A place needs a road beside it, and a road has to touch one.
    let tiles = server.state.db.world_tiles(COMMONS).unwrap();
    let road_near = |x: i64, y: i64| {
        (-2..=2)
            .any(|dx| (-2..=2).any(|dy| tiles.contains_key(&((x + dx) as u16, (y + dy) as u16))))
    };
    let empty = all
        .map(|(x, y)| (i64::from(x), i64::from(y)))
        .find(|&(x, y)| {
            (2..126).contains(&x)
                && (2..126).contains(&y)
                && land.at((x as u16, y as u16)).is_some_and(|ground| ground.is_open())
                && !road_near(x, y)
        })
        .expect("open land away from the roads");
    server.place(&justin, "house", empty).await.refused(StatusCode::BAD_REQUEST);
    server.act(&justin.key, road_move(empty)).await.refused(StatusCode::BAD_REQUEST);

    // A place they put up is theirs like any other, and nothing says how.
    let spot = server.build(&justin, "cafe").await;
    let view = server.view().await;
    assert!(
        view["owners"].as_array().unwrap().contains(&json!([
            spot.0,
            spot.1,
            "Justin",
            "Justin's Café"
        ])),
        "{}",
        view["owners"]
    );
    assert!(!view.to_string().contains("god"));
}

// W79, W95, L13
#[tokio::test]
async fn a_bot_gets_no_limits_from_its_owners_switch_and_none_of_its_powers() {
    let (server, justin, _) = Server::start().await;
    let hall = server
        .post(&justin.key, "/api/hangouts", json!({ "name": "Hall", "create_code": CODE }))
        .await
        .ok();
    let (_, fern) = server.bot(&justin, &text(&hall["hangout"]["id"]), "Fern").await;
    server.switch(&justin, true).await;

    // W95: as many projects as it wants, each with its building at once, past Justin's 3 and
    // with no wait, through his own player.
    for n in 0..5 {
        let started =
            server.tool(&fern, "start_project", json!({ "name": format!("Project {n}") })).await;
        let spot = server.free_spots()[0];
        let at = json!({ "project_id": started["project_id"], "x": spot.0, "y": spot.1, "purpose": "A place to build a garden robot together." });
        server.tool(&fern, "build_studio", at).await;
        assert_eq!(server.tile(spot), Some(Kind::Studio), "project {n}");
    }

    let capacity = server.tool(&fern, "inbox", json!({})).await;
    assert_eq!(capacity["project_capacity"]["can_start"], true);
    assert!(capacity["project_capacity"]["limit"].is_null());
    assert!(capacity["project_capacity"].get("god_mode").is_none());
    assert!(capacity["project_capacity"].get("exempt").is_none());
    assert!(capacity["project_capacity"]["remaining"].is_null());

    // Its own player is an agent's, with no move limit, and none of god mode's powers (W79).
    let joined = server.post(&fern, "/api/public/join", json!({})).await.ok();
    let player = text(&joined["player_token"]);
    let before = server.me(&player).await;
    assert!(before.get("god_mode").is_none());
    assert_eq!(server.act(&player, road_move(server.road_spot())).await.ok()["cost"], 0);
    let spot = server.free_spots()[0];
    let place = json!({ "action": "place", "kind": "cafe", "x": spot.0, "y": spot.1 });
    server.act(&player, place).await.refused(StatusCode::FORBIDDEN);
    let highway = server.highway_tile();
    let clear = json!({ "action": "bulldoze", "x": highway.0, "y": highway.1 });
    assert!(server.act(&player, clear).await.status.is_client_error(), "it can't take the highway");

    // With the switch off, its limits are back.
    server.switch(&justin, false).await;
    let capacity = server.tool(&fern, "inbox", json!({})).await;
    assert_eq!(capacity["project_capacity"]["can_start"], false);
    assert_eq!(capacity["project_capacity"]["limit"], 3);
    assert_eq!(capacity["project_capacity"]["remaining"], 0);
    let refused = server.call(&fern, "start_project", json!({ "name": "One more" })).await;
    assert_eq!(refused["isError"], true, "{refused}");
    let wave = json!({ "action": "emote", "emote": "wave" });
    assert_eq!(server.act(&player, wave).await.ok()["cost"], 1);
}

// W79, I19
#[tokio::test]
async fn every_item_is_unlocked_while_it_is_on_and_what_was_placed_stays() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    let house = server.build(&justin, "house").await;
    let room = text(&server.go_in(&justin.key, house).await.ok()["hangout_id"]);
    let items = format!("/api/floors/{room}/items");
    let catalog = server.get(&justin.key, "/api/items").await.ok();
    let locked =
        catalog["items"].as_array().unwrap().iter().filter(|item| item["unlocked"] != true).count();
    assert_eq!(locked, 0, "{catalog}");
    server.post(&justin.key, &items, json!({ "kind": "fireplace", "x": 30, "y": 2 })).await.ok();

    server.switch(&justin, false).await;
    let catalog = server.get(&justin.key, "/api/items").await.ok();
    let fireplace = catalog["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "fireplace")
        .unwrap()
        .clone();
    assert_eq!(fireplace["unlocked"], false);
    assert_eq!(fireplace["friends_needed"], 6);
    server
        .post(&justin.key, &items, json!({ "kind": "fireplace", "x": 34, "y": 2 }))
        .await
        .refused(StatusCode::FORBIDDEN);
    let snapshot = server.get(&justin.key, &format!("/api/hangouts/{room}")).await.ok();
    assert_eq!(snapshot["items"].as_array().map(Vec::len), Some(1), "{snapshot}");
}

// W79, W80, S34
#[tokio::test]
async fn turning_it_off_takes_nothing_down_and_the_limits_come_back() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    let house = server.build(&justin, "house").await;
    let mut places = Vec::new();
    for kind in ["studio", "club", "cafe", "cafe", "club"] {
        places.push(server.build(&justin, kind).await);
    }
    // A place past the limit opens onto a room of its own.
    let mut rooms = Vec::new();
    for place in &places {
        rooms.push(text(&server.go_in(&justin.key, *place).await.ok()["hangout_id"]));
    }
    rooms.sort();
    rooms.dedup();
    assert_eq!(rooms.len(), 5, "{rooms:?}");

    server.switch(&justin, false).await;
    assert_eq!(server.tile(house), Some(Kind::House));
    for place in &places {
        assert!(server.tile(*place).is_some());
        server.go_in(&justin.key, *place).await.ok();
    }
    let me = server.me(&justin.key).await;
    assert!(me.get("god_mode").is_none());
    // W90: the newer kinds count toward the same 3.
    let all = [
        "house", "studio", "office", "workshop", "library", "club", "lounge", "cafe", "bar",
        "green",
    ];
    assert_eq!(me["full"], json!(all));
    assert!(me["places"]["cafe"].as_i64().unwrap() > 0, "their wait is what it would have been");
    let spot = server.free_spots()[0];
    server.place(&justin, "studio", spot).await.refused(StatusCode::CONFLICT);
    server.place(&justin, "house", spot).await.refused(StatusCode::CONFLICT);
    server.act(&justin.key, road_move(server.road_spot())).await.refused(StatusCode::FORBIDDEN);
}

// W81
#[tokio::test]
async fn only_god_mode_puts_up_the_town_square_and_a_world_has_one() {
    let (server, justin, morg) = Server::start().await;
    let spot = server.big_spots()[0];
    let ann = server.person("Ann").await;
    let guest = server.guest().await;
    server.place(&ann, "square", spot).await.refused(StatusCode::FORBIDDEN);
    server.place(&justin, "square", spot).await.refused(StatusCode::FORBIDDEN);
    let place = json!({ "action": "place", "kind": "square", "x": spot.0, "y": spot.1 });
    server.act(&guest, place).await.refused(StatusCode::FORBIDDEN);
    assert_eq!(server.tile(spot), None);

    server.switch(&justin, true).await;
    assert_eq!(server.me(&justin.key).await["full"], json!([]));
    assert_eq!(server.place(&justin, "square", spot).await.ok()["cost"], 0);
    assert_eq!(server.tile(spot), Some(Kind::Square));
    assert_eq!(server.me(&justin.key).await["full"], json!(["square"]));

    // One to a world, for anyone with the switch.
    server.switch(&morg, true).await;
    let elsewhere = server.big_spots()[0];
    server.place(&justin, "square", elsewhere).await.refused(StatusCode::CONFLICT);
    server.place(&morg, "square", elsewhere).await.refused(StatusCode::CONFLICT);

    // It's a place like any other on the map, whose owner the view names.
    let view = server.view().await;
    assert!(
        view["tiles"].as_array().unwrap().contains(&json!([spot.0, spot.1, "square", 0])),
        "the view shows it"
    );
    assert!(
        view["owners"].as_array().unwrap().contains(&json!([
            spot.0,
            spot.1,
            "Justin",
            "Town Square"
        ])),
        "{}",
        view["owners"]
    );

    // Their own 3 aren't touched by it.
    server.switch(&justin, false).await;
    assert_eq!(server.me(&justin.key).await["full"], json!([]));

    // Once it's down there's room for another, anywhere.
    server.clear(&justin, spot).await.ok();
    assert_eq!(server.me(&morg.key).await["full"], json!([]));
    server.place(&morg, "square", elsewhere).await.ok();
}

// W81, W18
#[tokio::test]
async fn a_town_square_comes_down_with_its_builder() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    let spot = server.build(&justin, "square").await;

    let player = server.state.db.own_player(&justin.id, now()).unwrap();
    assert_eq!(server.state.db.remove_from_world(COMMONS, player.id, false, true).unwrap(), 1);
    assert_eq!(server.tile(spot), None);
}

// W82, S34, S48
#[tokio::test]
async fn the_town_square_opens_onto_one_big_public_room_that_comes_back() {
    let (server, justin, _) = Server::start().await;
    let ann = server.person("Ann").await;
    server.switch(&justin, true).await;
    let spot = server.build(&justin, "square").await;

    // Its door works for its owner first, with no code and with their switch off by then.
    server.go_in(&ann.key, spot).await.refused(StatusCode::NOT_FOUND);
    server.switch(&justin, false).await;
    let room = text(&server.go_in(&justin.key, spot).await.ok()["hangout_id"]);

    // Then anyone signed in can go in.
    assert_eq!(server.go_in(&ann.key, spot).await.ok()["hangout_id"], room.as_str());
    let floors = server.get(&ann.key, "/api/spaces/public").await.ok();
    let floor = floors["floors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|floor| floor["id"] == room.as_str())
        .unwrap()
        .clone();
    assert_eq!(floor["space_name"], "Town Square");
    assert_eq!(floor["size"], "big");
    assert_eq!(floor["capacity"], 150);

    // An ordinary place doesn't take over its room, and the square gets it back wherever it goes.
    server.switch(&justin, true).await;
    server.clear(&justin, spot).await.ok();
    let elsewhere = |taken: &[(i64, i64)]| {
        server.free_spots().into_iter().find(|at| !taken.contains(at)).unwrap()
    };
    let cafe = elsewhere(&[spot]);
    server.place(&justin, "cafe", cafe).await.ok();
    assert_ne!(server.go_in(&justin.key, cafe).await.ok()["hangout_id"], room.as_str());
    // W91: somewhere with room for all four of its tiles.
    let moved = server.big_spots().into_iter().find(|at| ![spot, cafe].contains(at)).unwrap();
    server.place(&justin, "square", moved).await.ok();
    assert_eq!(server.go_in(&justin.key, moved).await.ok()["hangout_id"], room.as_str());
}

// L18, S28
#[tokio::test]
async fn public_floors_list_the_town_square_first_while_it_stands() {
    let (server, justin, _) = Server::start().await;
    let (spot, square) = server.town_square(&justin).await;

    // A newer floor with as many people on it still comes after it.
    let (busy, cy) = server.someone("Cy").await;
    server
        .put(
            &cy.key,
            &format!("/api/floors/{busy}"),
            json!({ "access": "public", "create_code": CODE }),
        )
        .await
        .ok();

    let (room, bo) = server.someone("Bo").await;
    let (rover_id, rover) = server.bot(&bo, &room, "Rover").await;
    server.all_day(&bo, &rover_id, true).await;

    let listed = server.get(&bo.key, "/api/spaces/public").await.ok()["floors"].clone();
    assert_eq!(listed[0]["id"], square.as_str());
    assert_eq!(listed[0]["town_square"], true);
    assert_eq!(listed[1]["id"], busy.as_str());
    assert_eq!(listed[1]["town_square"], false);

    let tool = server.tool(&rover, "public_floors", json!({})).await;
    assert_eq!(tool["places"][0]["id"], square.as_str(), "{tool}");
    assert_eq!(tool["places"][0]["town_square"], true, "{tool}");

    // With its building down the room is only a room.
    server.clear(&justin, spot).await.ok();
    let listed = server.get(&bo.key, "/api/spaces/public").await.ok()["floors"].clone();
    let room = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|floor| floor["id"] == square.as_str())
        .unwrap()
        .clone();
    assert_eq!(room["town_square"], false);
}

/// A time in a half hour whose number leaves `rest` over `turns`, from the next one on, 11 minutes
/// in so a bot that just did something isn't busy (L31).
fn half_hour(turns: i64, rest: i64) -> i64 {
    let mut slot = now() / 1800 + 1;
    while slot % turns != rest {
        slot += 1;
    }
    slot * 1800 + 11 * 60
}

// L19, L41
#[tokio::test]
async fn a_bot_with_no_project_meets_at_the_town_square_and_explores_in_turn() {
    let (server, justin, _) = Server::start().await;
    let (cafe, cy) = server.someone("Cy").await;
    server
        .put(
            &cy.key,
            &format!("/api/floors/{cafe}"),
            json!({ "access": "public", "create_code": CODE }),
        )
        .await
        .ok();
    let (_, square) = server.town_square(&justin).await;

    // In a meeting half hour, however many bots there are, they all come here.
    let meeting = half_hour(2, 0);
    let (room, bo) = server.someone("Bo").await;
    for name in ["Rover", "Fido", "Patch"] {
        let (bot_id, bot) = server.bot(&bo, &room, name).await;
        server.all_day(&bo, &bot_id, true).await;
        routines::wander_at(&server.state, &bot_id, &bo.id, meeting).unwrap();
        assert_eq!(server.out_at(&bot).await, square.as_str(), "{name}");
    }

    // That goes for a bot whose owner is a member there too, because a square isn't their room.
    let hall = server
        .post(&justin.key, "/api/hangouts", json!({ "name": "Hall", "create_code": CODE }))
        .await
        .ok();
    let (fern_id, fern) = server.bot(&justin, &text(&hall["hangout"]["id"]), "Fern").await;
    server.all_day(&justin, &fern_id, true).await;
    routines::wander_at(&server.state, &fern_id, &justin.id, meeting).unwrap();
    assert_eq!(server.out_at(&fern).await, square.as_str());

    // The half hour after, it explores: here the only other place is Cy's cafe.
    let (scout_id, scout) = server.bot(&bo, &room, "Scout").await;
    server.all_day(&bo, &scout_id, true).await;
    routines::wander_at(&server.state, &scout_id, &bo.id, half_hour(2, 1)).unwrap();
    assert_eq!(server.out_at(&scout).await, cafe.as_str());
}

// L41, L9
#[tokio::test]
async fn a_bot_explores_its_owners_places_too_and_meets_when_there_is_nowhere_else() {
    let (server, justin, _) = Server::start().await;
    let (_, square) = server.town_square(&justin).await;
    let hall = server
        .post(&justin.key, "/api/hangouts", json!({ "name": "Hall", "create_code": CODE }))
        .await
        .ok();
    let (fern_id, fern) = server.bot(&justin, &text(&hall["hangout"]["id"]), "Fern").await;
    server.tool(&fern, "start_project", json!({ "name": "Garden robot" })).await;
    server.all_day(&justin, &fern_id, true).await;

    // Exploring, the only public floor is the square, so it meets there: its owner belonging to it
    // doesn't keep it away.
    routines::wander_at(&server.state, &fern_id, &justin.id, half_hour(2, 1)).unwrap();
    assert_eq!(server.out_at(&fern).await, square.as_str());
    server.tool(&fern, "come_back", json!({})).await;

    // It can still go there itself (L6).
    server.tool(&fern, "go_out", json!({ "floor_id": square })).await;
    assert_eq!(server.out_at(&fern).await, square.as_str());
}

// L20
#[tokio::test]
async fn the_town_square_brings_nobody_out() {
    let (server, justin, _) = Server::start().await;
    server.town_square(&justin).await;
    let (room, bo) = server.someone("Bo").await;

    // No social hours.
    let (_, rover) = server.bot(&bo, &room, "Rover").await;
    server.post(&rover, "/api/doorbell", json!({})).await.ok();
    assert!(server.out_at(&rover).await.is_null());

    // All day, with public places off.
    let (fido_id, fido) = server.bot(&bo, &room, "Fido").await;
    server.all_day(&bo, &fido_id, false).await;
    server.post(&fido, "/api/doorbell", json!({})).await.ok();
    assert!(server.out_at(&fido).await.is_null());
}

// L18, W81: a square that hasn't been through its door has no room, so nothing lists it.
#[tokio::test]
async fn a_town_square_nobody_has_gone_into_has_no_room_to_list() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    server.build(&justin, "square").await;
    let ann = server.person("Ann").await;
    assert_eq!(server.get(&ann.key, "/api/spaces/public").await.ok()["floors"], json!([]));
}

// W73, W82: the town square is called Town Square, before and after its owner has been through.
#[tokio::test]
async fn the_town_square_is_called_town_square_with_or_without_a_visit() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    let spot = server.build(&justin, "square").await;
    let name_at = |view: &Value| {
        let row = view["owners"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row[0] == spot.0 && row[1] == spot.1)
            .unwrap()
            .clone();
        (text(&row[2]), text(&row[3]))
    };

    // Nobody has been through its door, so the name isn't "Justin's Studio" or "Justin's Town Square".
    let (owner, name) = name_at(&server.view().await);
    assert_eq!((owner.as_str(), name.as_str()), ("Justin", "Town Square"));

    // After the visit it is the room's name, and the owner changing that changes the map (S7).
    let door = server.go_in(&justin.key, spot).await.ok();
    assert_eq!(name_at(&server.view().await).1, "Town Square");
    let space = text(&door["space_id"]);
    server
        .put(&justin.key, &format!("/api/spaces/{space}"), json!({ "name": "Meeting Green" }))
        .await
        .ok();
    assert_eq!(name_at(&server.view().await).1, "Meeting Green");
}

/// The floors in the public list that say they are the town square's.
async fn square_floors(server: &Server, token: &str) -> Vec<String> {
    let floors = server.get(token, "/api/spaces/public").await.ok();
    floors["floors"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|floor| floor["town_square"] == true)
        .map(|floor| text(&floor["id"]))
        .collect()
}

// W82: a square on a tile that was a cafe opens a room of its own, not the cafe's.
#[tokio::test]
async fn a_square_on_a_cafes_tile_opens_a_new_room_and_not_the_cafes() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    let spot = server.build(&justin, "cafe").await;
    let cafe_room = text(&server.go_in(&justin.key, spot).await.ok()["hangout_id"]);
    server.clear(&justin, spot).await.ok();
    server.place(&justin, "square", spot).await.ok();
    let room = text(&server.go_in(&justin.key, spot).await.ok()["hangout_id"]);
    assert_ne!(room, cafe_room);
    assert_eq!(square_floors(&server, &justin.key).await, vec![room.clone()]);
    let floors = server.get(&justin.key, "/api/spaces/public").await.ok();
    let floor = floors["floors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|floor| floor["id"] == room.as_str())
        .unwrap()
        .clone();
    assert_eq!(floor["size"], "big");
    assert_eq!(floor["town_square"], true);
}

// W82: a cafe on the tile the square stood on never opens the square's room.
#[tokio::test]
async fn a_cafe_on_the_squares_old_tile_doesnt_get_its_room() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    let spot = server.build(&justin, "square").await;
    let door = server.go_in(&justin.key, spot).await.ok();
    let (square_room, square_space) = (text(&door["hangout_id"]), text(&door["space_id"]));
    server.clear(&justin, spot).await.ok();
    server.place(&justin, "cafe", spot).await.ok();
    let door = server.go_in(&justin.key, spot).await.ok();
    assert_ne!(text(&door["hangout_id"]), square_room);
    assert!(!server.state.db.is_town_square_room(&text(&door["space_id"])).unwrap());

    server.clear(&justin, spot).await.ok();
    let elsewhere = server.free_spots().into_iter().find(|at| *at != spot).unwrap();
    server.place(&justin, "square", elsewhere).await.ok();
    let door = server.go_in(&justin.key, elsewhere).await.ok();
    assert_eq!(door["hangout_id"], square_room.as_str());
    assert!(server.state.db.is_town_square_room(&square_space).unwrap());
    assert_eq!(square_floors(&server, &justin.key).await, vec![square_room]);
}

// W82: which room is the square's is stored, not worked out from its name.
#[tokio::test]
async fn renaming_the_squares_room_keeps_it_the_squares_and_a_cafe_called_town_square_isnt() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    let spot = server.build(&justin, "square").await;
    let door = server.go_in(&justin.key, spot).await.ok();
    let (room, space) = (text(&door["hangout_id"]), text(&door["space_id"]));
    server
        .put(&justin.key, &format!("/api/spaces/{space}"), json!({ "name": "Meeting Green" }))
        .await
        .ok();
    server.clear(&justin, spot).await.ok();
    let elsewhere = server.free_spots().into_iter().find(|at| *at != spot).unwrap();
    server.place(&justin, "square", elsewhere).await.ok();
    assert_eq!(server.go_in(&justin.key, elsewhere).await.ok()["hangout_id"], room.as_str());

    // Calling an ordinary room "Town Square" doesn't make it the square's, and it still comes back.
    let ann = server.person("Ann").await;
    server.build(&ann, "house").await;
    let cafe = server.build_later(&ann, "cafe", 8);
    let door = server.go_in(&ann.key, cafe).await.ok();
    let (cafe_room, cafe_space) = (text(&door["hangout_id"]), text(&door["space_id"]));
    server
        .put(&ann.key, &format!("/api/spaces/{cafe_space}"), json!({ "name": "Town Square" }))
        .await
        .ok();
    server.clear(&ann, cafe).await.ok();
    let again = server.free_spots().into_iter().find(|at| *at != spot && *at != elsewhere).unwrap();
    let at = now() + 9 * DAY;
    let player = server.state.db.own_player(&ann.id, at).unwrap();
    let mv = Move {
        action: "place".to_string(),
        x: Some(again.0),
        y: Some(again.1),
        x2: None,
        y2: None,
        kind: Some("cafe".to_string()),
        emote: None,
    };
    worlds::act(&server.state, &player, COMMONS, &mv, at).unwrap();
    assert_eq!(server.go_in(&ann.key, again).await.ok()["hangout_id"], cafe_room.as_str());
    assert!(!server.state.db.is_town_square_room(&cafe_space).unwrap());
}

// W79, W80, S34: a place past the limit, built with the switch on, gets its room with it off.
#[tokio::test]
async fn a_place_past_the_limit_opens_a_room_after_the_switch_goes_off() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    let first = server.build(&justin, "cafe").await;
    let second = server.build(&justin, "club").await;
    let third = server.build(&justin, "studio").await;
    let fourth = server.build(&justin, "cafe").await;
    for at in [first, second, third] {
        server.go_in(&justin.key, at).await.ok();
    }
    server.switch(&justin, false).await;
    server.go_in(&justin.key, fourth).await.ok();
}

// W79, S34: holding the square and the 3 other places, a third other room still opens.
#[tokio::test]
async fn the_square_and_three_other_places_all_get_rooms_with_the_switch_off() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    server.build(&justin, "house").await;
    let square = server.build(&justin, "square").await;
    let cafe = server.build(&justin, "cafe").await;
    let club = server.build(&justin, "club").await;
    server.go_in(&justin.key, square).await.ok();
    server.go_in(&justin.key, cafe).await.ok();
    server.go_in(&justin.key, club).await.ok();
    server.switch(&justin, false).await;
    let studio = server.build_later(&justin, "studio", 8);
    server.go_in(&justin.key, studio).await.ok();
}

// W75, W81: someone without the switch is told nothing about it when they try the square.
#[tokio::test]
async fn placing_the_square_without_the_switch_is_refused_without_saying_why() {
    let (server, _, _) = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.free_spots()[0];
    let reply = server.place(&ann, "square", spot).await.refused(StatusCode::FORBIDDEN);
    assert!(!reply.to_string().to_lowercase().contains("god"), "{reply}");
}

// W93: god mode lays roads anywhere, but not right alongside another.
#[tokio::test]
async fn god_mode_still_cant_lay_a_road_right_alongside_another() {
    let (server, justin, _) = Server::start().await;
    server.switch(&justin, true).await;
    let tiles = server.state.db.world_tiles(COMMONS).unwrap();
    let land = world::terrain(world::COMMONS_SEED, world::COMMONS_SIZE);
    let road =
        |x: i64, y: i64| tiles.get(&(x as u16, y as u16)).is_some_and(|t| t.kind == Kind::Road);
    let open = |x: i64, y: i64| {
        let pos = (x as u16, y as u16);
        !tiles.contains_key(&pos) && land.at(pos).is_some_and(|ground| ground.is_open())
    };
    // Two tiles of highway, one above the other, with open land to their right.
    let (x, y) = tiles
        .keys()
        .map(|&(x, y)| (i64::from(x), i64::from(y)))
        .find(|&(x, y)| {
            road(x, y)
                && road(x, y + 1)
                && (y..=y + 1).all(|v| open(x + 1, v))
                && !road(x + 1, y - 1)
                && !road(x + 1, y + 2)
        })
        .expect("a straight bit of highway");
    server.act(&justin.key, road_move((x + 1, y))).await.ok();
    let refused = server.act(&justin.key, road_move((x + 1, y + 1))).await;
    refused.refused(StatusCode::BAD_REQUEST);
    assert_eq!(server.tile((x + 1, y + 1)), None);
}
