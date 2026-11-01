//! Tests for the outside of a place: everything on its one tile, and who can change it. They drive a
//! real server over HTTP and are written from the rules in docs/outside.md. A comment such as
//! `// E19` above a test names the rule it covers.
//!
//! The art tests at the bottom read the pages' source as text. They fail until every word in the
//! catalog is drawn, and that is what they are for.

use std::{
    collections::HashSet,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use agenthangout::{
    app, auth,
    db::Db,
    now,
    outside::{self, Outside},
    world::{self, Change, Kind, Tile},
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

impl Server {
    async fn start() -> Self {
        Self::boot(Db::open(":memory:").unwrap(), None, &[]).await
    }

    /// A server whose keeper is a person made first, so their id can be set in the config.
    async fn start_with_keeper() -> (Self, Who) {
        let db = Db::open(":memory:").unwrap();
        let key = auth::new_owner_key();
        db.create_person("keeper", "Keeper", &auth::hash_secret(&key), "keeper-feed", now())
            .unwrap();
        let server = Self::boot(db, Some("keeper".to_string()), &["keeper"]).await;
        (server, Who { key, id: "keeper".to_string() })
    }

    async fn boot(db: Db, keeper_id: Option<String>, god_ids: &[&str]) -> Self {
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
                god_ids: god_ids.iter().map(|id| id.to_string()).collect(),
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        worlds::ensure_commons(&state).unwrap();
        Self { base, client: Client::new(), state, addresses: AtomicUsize::new(1) }
    }

    async fn send_from(
        &self,
        ip: &str,
        token: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header("x-forwarded-for", ip);
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

    async fn send(&self, token: &str, method: Method, path: &str, body: Option<Value>) -> Reply {
        let n = self.addresses.fetch_add(1, Ordering::Relaxed);
        let ip = format!("198.51.{}.{}", n / 250, n % 250 + 1);
        self.send_from(&ip, token, method, path, body).await
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

    /// A person with a room of their own, and the id of that room.
    async fn someone(&self, name: &str) -> (String, Who) {
        let made = self
            .post(
                "",
                "/api/hangouts",
                json!({ "name": format!("{name}'s room"), "your_name": name, "create_code": CODE }),
            )
            .await
            .ok();
        (
            text(&made["hangout"]["id"]),
            Who { key: text(&made["owner_key"]), id: text(&made["person"]["id"]) },
        )
    }

    async fn person(&self, name: &str) -> Who {
        self.someone(name).await.1
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> String {
        let made = self
            .post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await
            .ok();
        text(&made["token"])
    }

    async fn guest(&self) -> String {
        text(&self.post("", "/api/public/join", json!({})).await.ok()["player_token"])
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
        spots.sort_unstable();
        spots.dedup();
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

    /// A move at a time of the test's choosing, through the person's own player.
    fn act_at(&self, who: &Who, mv: Move, at: i64) -> Acted {
        let player = self.state.db.own_player(&who.id, at).unwrap();
        worlds::act(&self.state, &player, COMMONS, &mv, at).unwrap()
    }

    fn put_up(&self, who: &Who, kind: &str, (x, y): (i64, i64), at: i64) {
        let mv = Move {
            action: "place".to_string(),
            x: Some(x),
            y: Some(y),
            x2: None,
            y2: None,
            kind: Some(kind.to_string()),
            emote: None,
        };
        self.act_at(who, mv, at);
    }

    fn take_down(&self, who: &Who, (x, y): (i64, i64), at: i64) {
        let mv = Move {
            action: "bulldoze".to_string(),
            x: Some(x),
            y: Some(y),
            x2: None,
            y2: None,
            kind: None,
            emote: None,
        };
        self.act_at(who, mv, at);
    }

    /// A house wherever the rules allow one, now.
    fn house(&self, who: &Who) -> (i64, i64) {
        let spot = self.free_spots()[0];
        self.put_up(who, "house", spot, now());
        spot
    }

    /// A place of another kind, put up this many days from now.
    fn place_later(&self, who: &Who, kind: &str, days: i64) -> (i64, i64) {
        let spot = self.free_spots()[0];
        self.put_up(who, kind, spot, now() + days * DAY);
        spot
    }

    fn player_of(&self, who: &Who) -> i64 {
        self.state.db.own_player(&who.id, now()).unwrap().id
    }

    fn write(&self, changes: &[Change]) {
        self.state
            .db
            .in_transaction::<(), rusqlite::Error>(|tx| tx.write_changes(COMMONS, changes, now()))
            .unwrap();
    }

    /// A town square through the same code a disaster and a bulldoze use.
    fn square(&self, who: &Who) -> (i64, i64) {
        let spot = self.free_spots()[0];
        let tile = Tile { kind: Kind::Square, level: 0, player: Some(self.player_of(who) as u64) };
        self.write(&[Change::Set((spot.0 as u16, spot.1 as u16), tile)]);
        spot
    }

    /// A park, the way `square` puts up a town square: a place stored on the first of its four tiles.
    fn green(&self, who: &Who, spot: (i64, i64)) {
        let tile = Tile { kind: Kind::Green, level: 0, player: Some(self.player_of(who) as u64) };
        self.write(&[Change::Set((spot.0 as u16, spot.1 as u16), tile)]);
    }

    fn run_a_disaster(&self) {
        let step = self.state.db.world(COMMONS).unwrap().unwrap().step;
        self.state
            .db
            .in_transaction::<(), rusqlite::Error>(|tx| tx.start_disaster(COMMONS, step))
            .unwrap();
        let strike = step + world::OMEN_STEPS;
        self.state
            .db
            .in_transaction::<(), rusqlite::Error>(|tx| tx.set_step(COMMONS, strike))
            .unwrap();
        for _ in 0..world::STRIKE_STEPS {
            worlds::step_world(&self.state, COMMONS, now()).unwrap();
        }
    }

    fn url((x, y): (i64, i64)) -> String {
        format!("/api/places/{x}/{y}/outside")
    }

    async fn save(&self, who: &Who, at: (i64, i64), document: Value) -> Reply {
        self.put(&who.key, &Self::url(at), document).await
    }

    /// What a signed-in person is told about a tile's outside.
    async fn outside_of(&self, who: &Who, at: (i64, i64)) -> Value {
        self.get(&who.key, &Self::url(at)).await.ok()["outside"].clone()
    }

    async fn on_the_map(&self) -> Vec<Value> {
        let reply = self.get("", &format!("/api/worlds/{COMMONS}/outsides")).await.ok();
        reply["outsides"].as_array().unwrap().clone()
    }

    /// A view after the pause that lets a change show (W29).
    async fn view(&self) -> Value {
        tokio::time::sleep(Duration::from_millis(1100)).await;
        self.get("", &format!("/api/worlds/{COMMONS}")).await.ok()
    }

    fn stamp(&self) -> (i64, i64) {
        self.state.db.outsides_stamp(COMMONS).unwrap()
    }
}

fn walls(color: i64) -> Value {
    json!({ "parts": { "walls": { "style": "brick", "color": color } } })
}

/// An outside of a house, built from one document the checks accept.
fn parse(document: Value) -> Outside {
    serde_json::from_value(document).unwrap()
}

// E1, E25
#[tokio::test]
async fn an_outside_is_stored_whole_and_saving_an_empty_one_takes_it_away() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);

    let before = server.get(&ann.key, &Server::url(spot)).await.ok();
    let (x, y) = spot;
    assert_eq!(before, json!({ "x": x, "y": y, "kind": "house", "outside": {}, "yours": true }));

    let document = json!({
        "parts": { "walls": { "style": "brick", "color": 0xaa5533 }, "rooftop": { "on": ["flag"] } },
        "body": { "x": 3, "y": 3, "w": 9, "d": 9, "floors": 2 },
        "items": [{ "kind": "oak", "x": 1, "y": 13 }],
        "pets": [{ "species": "cat", "name": "inky", "rules": [{ "when": "night", "do": "sleep" }] }],
    });
    let saved = server.save(&ann, spot, document.clone()).await.ok();
    assert_eq!(saved["outside"], document);
    assert_eq!(server.outside_of(&ann, spot).await, document);
    assert_eq!(server.state.db.outside_rows(COMMONS, server.player_of(&ann)).unwrap(), 1);

    // Each of the four is optional, and one alone is enough.
    for alone in [
        json!({ "parts": { "door": { "style": "arched" } } }),
        json!({ "body": { "x": 3, "y": 3, "w": 9, "d": 9, "floors": 1 } }),
        json!({ "items": [{ "kind": "oak", "x": 1, "y": 13 }] }),
        json!({ "pets": [{ "species": "dog", "name": "biscuit" }] }),
    ] {
        assert_eq!(server.save(&ann, spot, alone.clone()).await.ok()["outside"], alone);
    }

    // Nothing in it is no outside at all.
    for empty in [json!({}), json!({ "parts": {}, "items": [], "pets": [] })] {
        server.save(&ann, spot, walls(0)).await.ok();
        assert_eq!(server.save(&ann, spot, empty).await.ok()["outside"], json!({}));
        assert_eq!(server.outside_of(&ann, spot).await, json!({}));
        assert_eq!(server.state.db.outside_rows(COMMONS, server.player_of(&ann)).unwrap(), 0);
    }
}

// E1, E27
#[tokio::test]
async fn only_the_commons_has_outsides() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    server.save(&ann, spot, walls(1)).await.ok();
    assert_eq!(server.on_the_map().await.len(), 1);

    server.state.db.create_person("owner", "Owner", "k", "f", now()).unwrap();
    let new = agenthangout::db::NewWorld {
        id: "frozen",
        name: "Quiet Bay",
        kind: "own",
        seed: 99,
        size: world::OWN_SIZE,
        owner_id: Some("owner"),
        open: true,
        invite_code: Some("frozen-code"),
        highway: &world::highway(99, &world::terrain(99, world::OWN_SIZE)),
        now: now(),
    };
    server.state.db.insert_world(&new, worlds::MAX_OWN_WORLDS).unwrap();
    let frozen = server.get("", "/api/worlds/frozen/outsides").await.ok();
    assert_eq!(frozen, json!({ "outsides": [] }));
    server.get("", "/api/worlds/nowhere/outsides").await.refused(StatusCode::NOT_FOUND);
    let view = server.get("", "/api/worlds/frozen").await.ok();
    assert!(view.get("outsides").is_none(), "{view}");
}

// E2
#[tokio::test]
async fn anything_the_catalog_does_not_list_is_refused_and_changes_nothing() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    let kept = walls(0x336699);
    server.save(&ann, spot, kept.clone()).await.ok();
    let refused = [
        json!({ "name": "Ann's place" }),
        json!({ "parts": { "walls": { "style": "brick", "note": "hi" } } }),
        json!({ "parts": { "moat": { "style": "deep" } } }),
        json!({ "parts": { "walls": { "style": "diamond" } } }),
        json!({ "parts": { "walls": { "style": 5 } } }),
        json!({ "parts": { "walls": { "style": "<b>brick</b>" } } }),
        json!({ "parts": { "walls": "brick" } }),
        json!({ "parts": { "centerpiece": { "style": "fountain" } } }),
        json!({ "body": { "x": 3, "y": 3, "w": 9, "d": 9, "floors": 1, "roof": 1 } }),
        json!({ "items": [{ "kind": "sign_that_says_hi", "x": 1, "y": 13 }] }),
        json!({ "items": [{ "kind": "oak", "x": 1, "y": 13, "label": "tree" }] }),
        json!({ "pets": [{ "species": "dragon", "name": "biscuit" }] }),
        json!({ "pets": [{ "species": "dog", "name": "Rex" }] }),
        json!({ "pets": [{ "species": "dog", "name": "biscuit", "rules": [{ "when": "x", "do": "sit" }] }] }),
        json!({ "pets": [{ "species": "dog", "name": "biscuit", "rules": [{ "when": "always", "do": "bark" }] }] }),
        json!([1, 2]),
        json!("brick"),
    ];
    for document in refused {
        server.save(&ann, spot, document.clone()).await.refused(StatusCode::BAD_REQUEST);
        assert_eq!(server.outside_of(&ann, spot).await, kept, "after {document}");
    }
    // And a body that is not JSON at all.
    let broken = server
        .client
        .put(format!("{}{}", server.base, Server::url(spot)))
        .bearer_auth(&ann.key)
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(broken.status(), StatusCode::BAD_REQUEST);
    assert_eq!(server.outside_of(&ann, spot).await, kept);
}

// E3
#[tokio::test]
async fn a_colour_is_a_whole_number_from_0_to_16777215() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    for good in [0, 1, 255, 16_777_215] {
        server.save(&ann, spot, walls(good)).await.ok();
    }
    let kept = walls(16_777_215);
    for bad in
        [json!(16_777_216), json!(-1), json!(1.5), json!("red"), json!("#ff0000"), json!(null)]
    {
        let document = json!({ "parts": { "walls": { "style": "brick", "color": bad } } });
        let reply = server.save(&ann, spot, document).await;
        // A null is no colour, which is allowed, and everything else is refused.
        if bad.is_null() {
            reply.ok();
            server.save(&ann, spot, kept.clone()).await.ok();
        } else {
            reply.refused(StatusCode::BAD_REQUEST);
            assert_eq!(server.outside_of(&ann, spot).await, kept);
        }
    }
    // The same goes for an item's colour and a pet's.
    let item = json!({ "items": [{ "kind": "bench", "x": 1, "y": 13, "color": 16_777_216 }] });
    server.save(&ann, spot, item).await.refused(StatusCode::BAD_REQUEST);
    let pet = json!({ "pets": [{ "species": "dog", "name": "biscuit", "coat": -5 }] });
    server.save(&ann, spot, pet).await.refused(StatusCode::BAD_REQUEST);
    let collar = json!({ "pets": [{ "species": "dog", "name": "biscuit", "collar": 16_777_216 }] });
    server.save(&ann, spot, collar).await.refused(StatusCode::BAD_REQUEST);
}

// E4, E5, W5, W23
#[tokio::test]
async fn an_outside_changes_how_a_tile_looks_and_nothing_else() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    let me = server.get(&ann.key, &format!("/api/worlds/{COMMONS}/me")).await.ok();
    let before = server.view().await;
    let details = server.get(&ann.key, &format!("/api/places/{}/{}", spot.0, spot.1)).await.ok();

    let busy = json!({
        "parts": { "walls": { "style": "logs", "color": 5 } },
        "items": [{ "kind": "pond", "x": 1, "y": 12 }],
        "pets": [{ "species": "fox", "name": "kiwi" }],
    });
    server.save(&ann, spot, busy).await.ok();

    let after = server.view().await;
    for key in ["tiles", "places", "roads", "stage", "owners", "land"] {
        assert_eq!(before[key], after[key], "{key}");
    }
    // The only new thing in a view is the two numbers.
    let keys = |view: &Value| -> HashSet<String> {
        view.as_object().unwrap().keys().filter(|key| *key != "outsides").cloned().collect()
    };
    assert_eq!(keys(&before), keys(&after));
    let now_me = server.get(&ann.key, &format!("/api/worlds/{COMMONS}/me")).await.ok();
    assert_eq!(now_me["your_town"], me["your_town"]);
    assert_eq!(now_me["moves_left"], me["moves_left"]);
    // E28: what the place says about its rooms and bots is the same.
    let now_details =
        server.get(&ann.key, &format!("/api/places/{}/{}", spot.0, spot.1)).await.ok();
    assert_eq!(now_details, details);

    // A place with an outside comes down like any other.
    server.take_down(&ann, spot, now() + DAY);
    server.get(&ann.key, &Server::url(spot)).await.refused(StatusCode::NOT_FOUND);
}

// E6
#[tokio::test]
async fn only_the_owner_of_a_place_can_change_its_outside() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let (bo_room, bo) = server.someone("Bo").await;
    let spot = server.house(&ann);
    let scout = server.bot(&ann, &room, "Scout").await;
    let rover = server.bot(&bo, &bo_room, "Rover").await;
    let guest = server.guest().await;
    let unknown = auth::new_owner_key();

    // Someone else signed in.
    server.put(&bo.key, &Server::url(spot), walls(1)).await.refused(StatusCode::FORBIDDEN);
    // A bot, even its owner's own.
    server.put(&scout, &Server::url(spot), walls(1)).await.refused(StatusCode::FORBIDDEN);
    server.put(&rover, &Server::url(spot), walls(1)).await.refused(StatusCode::FORBIDDEN);
    // A guest token gets what any unknown token gets.
    let by_guest = server.put(&guest, &Server::url(spot), walls(1)).await;
    let by_unknown = server.put(&unknown, &Server::url(spot), walls(1)).await;
    assert_eq!(by_guest.status, by_unknown.status);
    assert_eq!(by_guest.status, StatusCode::UNAUTHORIZED);
    server.put("", &Server::url(spot), walls(1)).await.refused(StatusCode::UNAUTHORIZED);
    // A tile with no place.
    let empty = server.free_spots()[0];
    server.put(&ann.key, &Server::url(empty), walls(1)).await.refused(StatusCode::NOT_FOUND);
    server.get(&ann.key, &Server::url(empty)).await.refused(StatusCode::NOT_FOUND);
    // Nothing above stored anything.
    assert_eq!(server.outside_of(&ann, spot).await, json!({}));
    server.save(&ann, spot, walls(1)).await.ok();

    // Anyone signed in as a person can look, and is told whether it's theirs.
    let seen = server.get(&bo.key, &Server::url(spot)).await.ok();
    assert_eq!(seen["kind"], "house");
    assert_eq!(seen["outside"], walls(1));
    assert_eq!(seen["yours"], false);
    server.get(&scout, &Server::url(spot)).await.refused(StatusCode::FORBIDDEN);
    server.get("", &Server::url(spot)).await.refused(StatusCode::UNAUTHORIZED);
}

// E6, E7: who is asking comes before what they asked.
#[tokio::test]
async fn a_stranger_with_a_bad_body_learns_nothing_about_it() {
    let (server, keeper) = Server::start_with_keeper().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let spot = server.house(&ann);
    let empty = server.free_spots()[0];
    for bad in [json!({ "nope": 1 }), json!([]), walls(-1)] {
        server.put(&bo.key, &Server::url(spot), bad.clone()).await.refused(StatusCode::FORBIDDEN);
        server
            .put(&keeper.key, &Server::url(spot), bad.clone())
            .await
            .refused(StatusCode::FORBIDDEN);
        server.put(&bo.key, &Server::url(empty), bad).await.refused(StatusCode::NOT_FOUND);
    }
    let broken = server
        .client
        .put(format!("{}{}", server.base, Server::url(spot)))
        .bearer_auth(&bo.key)
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(broken.status(), StatusCode::FORBIDDEN);
}

// E7, W79
#[tokio::test]
async fn god_mode_cannot_change_someone_elses_outside_and_the_keeper_can_only_clear_it() {
    let (server, keeper) = Server::start_with_keeper().await;
    server.put(&keeper.key, "/api/god-mode", json!({ "on": true })).await.ok();
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let spot = server.house(&ann);
    server.save(&ann, spot, walls(7)).await.ok();

    server.put(&keeper.key, &Server::url(spot), walls(8)).await.refused(StatusCode::FORBIDDEN);
    server.delete(&bo.key, &Server::url(spot)).await.refused(StatusCode::FORBIDDEN);
    assert_eq!(server.outside_of(&ann, spot).await, walls(7));

    server.delete(&keeper.key, &Server::url(spot)).await.ok();
    assert_eq!(server.outside_of(&ann, spot).await, json!({}));
    assert_eq!(server.state.db.outside_rows(COMMONS, server.player_of(&ann)).unwrap(), 0);
    assert!(server.on_the_map().await.is_empty());

    // The owner clears their own, and a place with none says ok.
    server.save(&ann, spot, walls(9)).await.ok();
    assert_eq!(server.delete(&ann.key, &Server::url(spot)).await.ok(), json!({ "ok": true }));
    assert_eq!(server.delete(&ann.key, &Server::url(spot)).await.ok(), json!({ "ok": true }));
    let empty = server.free_spots()[0];
    server.delete(&keeper.key, &Server::url(empty)).await.refused(StatusCode::NOT_FOUND);
}

// E8
#[tokio::test]
async fn changing_an_outside_costs_nothing_and_a_person_can_save_60_a_minute() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let spot = server.house(&ann);
    let bo_spot = server.house(&bo);
    let moves = |who: &Who| {
        let server = &server;
        let key = who.key.clone();
        async move {
            server.get(&key, &format!("/api/worlds/{COMMONS}/me")).await.ok()["moves_left"].clone()
        }
    };
    let before = moves(&ann).await;
    let minute = now() / 60;
    let mut saved = 0;
    let mut first_refused = None;
    for n in 0..125 {
        let reply = server.save(&ann, spot, walls(n)).await;
        if reply.status == StatusCode::OK {
            saved += 1;
        } else {
            assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS, "{}", reply.body);
            first_refused = Some(n);
            break;
        }
    }
    let first_refused = first_refused.expect("nothing was ever refused");
    if now() / 60 == minute {
        assert_eq!((saved, first_refused), (60, 60));
    }
    assert_eq!(moves(&ann).await, before);
    // It is one person's allowance and nobody else's.
    server.save(&bo, bo_spot, walls(1)).await.ok();
}

// E9
#[tokio::test]
async fn the_catalog_is_for_people_and_lists_everything() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    server.get(&scout, "/api/outside").await.refused(StatusCode::FORBIDDEN);
    server.get("", "/api/outside").await.refused(StatusCode::UNAUTHORIZED);
    let catalog = server.get(&ann.key, "/api/outside").await.ok();
    for key in ["groups", "parts", "yard", "pets", "templates", "swatches", "limits"] {
        assert!(catalog.get(key).is_some(), "{key}");
    }
    assert_eq!(catalog["parts"].as_array().unwrap().len(), outside::PARTS.len());
    assert_eq!(catalog["yard"].as_array().unwrap().len(), outside::YARD.len());
    assert_eq!(catalog["templates"].as_array().unwrap().len(), outside::templates().len());
    assert_eq!(catalog["limits"]["items"], outside::MAX_ITEMS);
    assert_eq!(catalog["limits"]["pets"], outside::MAX_PETS);
    assert_eq!(catalog["limits"]["rules"], outside::MAX_RULES);
}

// E10, E11
#[tokio::test]
async fn a_part_takes_only_what_the_catalog_says_it_takes() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    let part = |word: &str, value: Value| json!({ "parts": { word: value } });
    let good = [
        part("rooftop", json!({ "on": ["flag", "antenna"], "color": 1 })),
        part("rooftop", json!({ "on": [] })),
        part("walls", json!({ "style": "glass", "color": 5 })),
        part("roof", json!({ "style": "dome", "color": 5, "number": 1 })),
        part("roof", json!({ "number": 3 })),
        part("windows", json!({ "style": "round", "color": 5, "number": 0 })),
        part("windows", json!({ "number": 6 })),
        part("fence", json!({ "style": "picket", "color": 5, "number": 0 })),
        part("fence", json!({ "number": 15 })),
        part("foundation", json!({ "style": "raised" })),
        part("door_side", json!({ "style": "left" })),
        part("shape", json!({ "style": "tower" })),
    ];
    for document in good {
        server.save(&ann, spot, document.clone()).await.ok();
        assert_eq!(server.outside_of(&ann, spot).await, document);
    }
    let bad = [
        // A many part takes `on` and no `style`, and any other part the other way round.
        part("rooftop", json!({ "style": "flag" })),
        part("rooftop", json!({ "on": ["flag", "flag"] })),
        part("rooftop", json!({ "on": ["fountain"] })),
        part("rooftop", json!({ "on": "flag" })),
        part("walls", json!({ "on": ["brick"] })),
        // A colour only where the catalog says.
        part("foundation", json!({ "style": "stone", "color": 5 })),
        part("roofing", json!({ "style": "slate", "color": 5 })),
        part("window_boxes", json!({ "color": 5 })),
        part("shape", json!({ "color": 5 })),
        part("door_side", json!({ "color": 5 })),
        // A number only where it says, and inside its limits.
        part("walls", json!({ "number": 1 })),
        part("door", json!({ "number": 1 })),
        part("roof", json!({ "number": 0 })),
        part("roof", json!({ "number": 4 })),
        part("windows", json!({ "number": 7 })),
        part("windows", json!({ "number": -1 })),
        part("fence", json!({ "number": 16 })),
        part("fence", json!({ "number": -1 })),
        part("fence", json!({ "number": 1.5 })),
    ];
    let kept = server.outside_of(&ann, spot).await;
    for document in bad {
        server.save(&ann, spot, document.clone()).await.refused(StatusCode::BAD_REQUEST);
        assert_eq!(server.outside_of(&ann, spot).await, kept, "after {document}");
    }
}

// E10: a town square has its own parts and a house has none of them.
#[tokio::test]
async fn a_town_square_has_its_own_parts() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.square(&ann);
    let part = |word: &str, value: Value| json!({ "parts": { word: value } });
    for good in [
        part("centerpiece", json!({ "style": "clock", "color": 5 })),
        part("fence", json!({ "style": "rope", "number": 3 })),
        part("gate", json!({ "style": "arch" })),
        part("ground", json!({ "style": "tiles" })),
        part("path", json!({ "style": "flagstone" })),
        part("string_lights", json!({ "style": "festoon", "color": 5 })),
    ] {
        server.save(&ann, spot, good).await.ok();
    }
    for bad in [
        part("walls", json!({ "style": "brick" })),
        part("roof", json!({ "style": "flat" })),
        part("door", json!({ "style": "panel" })),
        part("glow", json!({ "style": "off" })),
        part("shape", json!({ "style": "plain" })),
        part("sign", json!({ "style": "star" })),
    ] {
        server.save(&ann, spot, bad).await.refused(StatusCode::BAD_REQUEST);
    }
    assert_eq!(server.get(&ann.key, &Server::url(spot)).await.ok()["kind"], "square");
}

// E13
#[tokio::test]
async fn a_body_stays_on_the_tile_with_room_for_a_fence() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    let body = |x: Value, y: Value, w: Value, d: Value, floors: Value| json!({ "body": { "x": x, "y": y, "w": w, "d": d, "floors": floors } });
    for good in [
        body(json!(1), json!(1), json!(5), json!(5), json!(1)),
        body(json!(10), json!(10), json!(5), json!(5), json!(3)),
        body(json!(1), json!(1), json!(14), json!(14), json!(2)),
    ] {
        server.save(&ann, spot, good).await.ok();
    }
    let kept = server.outside_of(&ann, spot).await;
    let big = json!(i64::MAX);
    for bad in [
        body(json!(0), json!(5), json!(5), json!(5), json!(1)),
        body(json!(5), json!(0), json!(5), json!(5), json!(1)),
        body(json!(-1), json!(5), json!(5), json!(5), json!(1)),
        body(json!(2), json!(2), json!(4), json!(5), json!(1)),
        body(json!(2), json!(2), json!(5), json!(4), json!(1)),
        body(json!(11), json!(2), json!(5), json!(5), json!(1)),
        body(json!(2), json!(11), json!(5), json!(5), json!(1)),
        body(json!(1), json!(1), json!(15), json!(5), json!(1)),
        body(json!(1), json!(1), json!(5), json!(15), json!(1)),
        body(json!(2), json!(2), json!(5), json!(5), json!(0)),
        body(json!(2), json!(2), json!(5), json!(5), json!(4)),
        body(big.clone(), json!(2), json!(5), json!(5), json!(1)),
        body(json!(2), json!(2), big.clone(), json!(5), json!(1)),
        body(json!(2), json!(2), json!(5), json!(5), big.clone()),
        body(json!(1), json!(1), json!(i64::MAX), json!(i64::MAX), json!(1)),
        body(json!(2.5), json!(2), json!(5), json!(5), json!(1)),
        body(json!("2"), json!(2), json!(5), json!(5), json!(1)),
        json!({ "body": { "x": 2, "y": 2, "w": 5, "d": 5 } }),
    ] {
        server.save(&ann, spot, bad.clone()).await.refused(StatusCode::BAD_REQUEST);
        assert_eq!(server.outside_of(&ann, spot).await, kept, "after {bad}");
    }
}

// E14
#[tokio::test]
async fn a_town_square_has_no_body_and_keeps_its_middle_clear() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.square(&ann);
    let body = json!({ "body": { "x": 3, "y": 3, "w": 6, "d": 6, "floors": 1 } });
    server.save(&ann, spot, body).await.refused(StatusCode::BAD_REQUEST);

    let item = |x: i64, y: i64| json!({ "items": [{ "kind": "oak", "x": x, "y": y }] });
    // The middle is the 4 by 4 cells from 6, 6, so an oak (2 by 2) touching it is refused.
    for (x, y) in [(6, 6), (5, 5), (9, 9), (7, 7), (5, 9)] {
        server.save(&ann, spot, item(x, y)).await.refused(StatusCode::CONFLICT);
    }
    for (x, y) in [(4, 6), (10, 6), (6, 4), (6, 10), (0, 0)] {
        server.save(&ann, spot, item(x, y)).await.ok();
    }
    // Whatever the centrepiece is, it stands there.
    let with = |style: &str| {
        json!({
            "parts": { "centerpiece": { "style": style } },
            "items": [{ "kind": "oak", "x": 7, "y": 7 }],
        })
    };
    server.save(&ann, spot, with("fountain")).await.refused(StatusCode::CONFLICT);
    server.save(&ann, spot, with("statue")).await.refused(StatusCode::CONFLICT);
    server.save(&ann, spot, with("none")).await.ok();
}

// E15
#[tokio::test]
async fn an_item_is_a_listed_kind_in_a_cell_with_a_facing_and_a_colour_only_if_it_takes_one() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    let item = |value: Value| json!({ "items": [value] });
    for good in [
        item(json!({ "kind": "bench", "x": 1, "y": 13, "color": 0 })),
        item(json!({ "kind": "bench", "x": 1, "y": 13, "facing": 3 })),
        item(json!({ "kind": "oak", "x": 14, "y": 14 })),
    ] {
        server.save(&ann, spot, good).await.ok();
    }
    for bad in [
        item(json!({ "kind": "oak", "x": 1, "y": 13, "color": 5 })),
        item(json!({ "kind": "bench", "x": 1, "y": 13, "facing": 4 })),
        item(json!({ "kind": "bench", "x": 1, "y": 13, "facing": -1 })),
        item(json!({ "kind": "bench", "x": 1, "y": 13, "facing": 1.5 })),
        item(json!({ "kind": "bench", "x": 1.5, "y": 13 })),
        item(json!({ "kind": "bench", "x": "1", "y": 13 })),
        item(json!({ "kind": "bench", "y": 13 })),
        item(json!({ "kind": "chair", "x": 1, "y": 13 })),
        item(json!({ "x": 1, "y": 13 })),
        item(json!({ "kind": 7, "x": 1, "y": 13 })),
    ] {
        server.save(&ann, spot, bad).await.refused(StatusCode::BAD_REQUEST);
    }
    // Turning an item by 1 or 3 swaps its cells: a hammock is 3 by 1.
    let hammock =
        |facing: i64| item(json!({ "kind": "hammock", "x": 0, "y": 14, "facing": facing }));
    server.save(&ann, spot, hammock(0)).await.ok();
    server.save(&ann, spot, hammock(2)).await.ok();
    server.save(&ann, spot, hammock(1)).await.refused(StatusCode::CONFLICT);
    server.save(&ann, spot, hammock(3)).await.refused(StatusCode::CONFLICT);
}

// E16
#[tokio::test]
async fn an_item_cannot_stand_on_another_off_the_tile_or_on_the_building() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    let items = |list: Value| json!({ "items": list });
    let tree = |x: i64, y: i64| json!({ "kind": "oak", "x": x, "y": y });
    // A house stands on cells 3 to 12 across and 4 to 11 down unless it says otherwise.
    server.save(&ann, spot, items(json!([tree(0, 0), tree(2, 0), tree(0, 2)]))).await.ok();
    server.save(&ann, spot, items(json!([tree(1, 13)]))).await.ok();
    for list in [
        json!([tree(0, 0), tree(1, 1)]),
        json!([tree(0, 0), tree(0, 0)]),
        json!([tree(15, 0)]),
        json!([tree(0, 15)]),
        json!([tree(-1, 0)]),
        json!([tree(0, -1)]),
        json!([tree(16, 16)]),
        json!([tree(i64::MAX, 0)]),
        json!([tree(0, i64::MIN)]),
        json!([tree(4, 5)]),
        json!([tree(2, 5)]),
        json!([tree(12, 11)]),
        json!([tree(5, 3)]),
    ] {
        server.save(&ann, spot, items(list.clone())).await.refused(StatusCode::CONFLICT);
    }
    // The whole document is checked together: a body that would cover an item is refused too.
    let both = json!({
        "body": { "x": 1, "y": 1, "w": 5, "d": 5, "floors": 1 },
        "items": [tree(8, 8)],
    });
    server.save(&ann, spot, both).await.ok();
    let covers = json!({
        "body": { "x": 6, "y": 6, "w": 5, "d": 5, "floors": 1 },
        "items": [tree(8, 8)],
    });
    server.save(&ann, spot, covers).await.refused(StatusCode::CONFLICT);
    // And 400 comes first for what isn't listed, whatever else doesn't fit.
    let both_wrong =
        json!({ "items": [tree(0, 0), tree(0, 0), { "kind": "nope", "x": 1, "y": 1 }] });
    server.save(&ann, spot, both_wrong).await.refused(StatusCode::BAD_REQUEST);
}

// E17
#[tokio::test]
async fn a_place_holds_at_most_forty_items() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    // 1 by 1 items on the cells clear of a house: the rows above and below it and the sides.
    let mut cells = Vec::new();
    for y in 0..16 {
        for x in 0..16 {
            if !(3..13).contains(&x) || !(4..12).contains(&y) {
                cells.push((x, y));
            }
        }
    }
    let rock = |&(x, y): &(i64, i64)| json!({ "kind": "rock", "x": x, "y": y });
    let forty: Vec<Value> = cells.iter().take(40).map(rock).collect();
    let forty_one: Vec<Value> = cells.iter().take(41).map(rock).collect();
    server.save(&ann, spot, json!({ "items": forty })).await.ok();
    server.save(&ann, spot, json!({ "items": forty_one })).await.refused(StatusCode::CONFLICT);
    assert_eq!(server.outside_of(&ann, spot).await["items"].as_array().unwrap().len(), 40);
}

// E18, E19
#[tokio::test]
async fn a_pet_has_a_listed_species_and_name_and_a_rule_a_listed_when_and_do() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    let pet = |name: &str| json!({ "species": "dog", "name": name });
    let with = |pets: Vec<Value>| json!({ "pets": pets });
    server.save(&ann, spot, with(vec![pet("biscuit")])).await.ok();
    server.save(&ann, spot, with((0..4).map(|n| pet(outside::PET_NAMES[n])).collect())).await.ok();
    // At most 4, the same name twice is just two pets.
    server
        .save(&ann, spot, with((0..5).map(|n| pet(outside::PET_NAMES[n])).collect()))
        .await
        .refused(StatusCode::CONFLICT);
    server.save(&ann, spot, with(vec![pet("biscuit"), pet("biscuit")])).await.ok();
    // A name can't be typed.
    for name in ["Biscuit", "rex", "", "biscuit ", "<b>biscuit</b>"] {
        server.save(&ann, spot, with(vec![pet(name)])).await.refused(StatusCode::BAD_REQUEST);
    }
    server
        .save(&ann, spot, with(vec![json!({ "species": "dog", "name": 5 })]))
        .await
        .refused(StatusCode::BAD_REQUEST);
    for species in outside::SPECIES {
        let document = with(vec![json!({ "species": species.0, "name": "biscuit" })]);
        outside::check("house", &parse(document)).unwrap();
    }

    let rule = |when: &str, action: &str| json!({ "when": when, "do": action });
    let ruled =
        |rules: Vec<Value>| with(vec![json!({ "species": "cat", "name": "inky", "rules": rules })]);
    let six: Vec<Value> = (0..6).map(|_| rule("always", "sit")).collect();
    server.save(&ann, spot, ruled(six.clone())).await.ok();
    let mut seven = six;
    seven.push(rule("always", "sit"));
    server.save(&ann, spot, ruled(seven)).await.refused(StatusCode::CONFLICT);
    for when in outside::WHEN {
        for action in outside::DO {
            outside::check("house", &parse(ruled(vec![rule(when.0, action.0)]))).unwrap();
        }
    }
    for (when, action) in [("never", "sit"), ("always", "fly"), ("", "sit"), ("always", "")] {
        server
            .save(&ann, spot, ruled(vec![rule(when, action)]))
            .await
            .refused(StatusCode::BAD_REQUEST);
    }
    let odd = ruled(vec![json!({ "when": "always" })]);
    server.save(&ann, spot, odd).await.refused(StatusCode::BAD_REQUEST);
}

// E20: a `when` is added only by reading this rule, so this list is fixed and a new word fails here.
#[test]
fn no_when_can_ask_who_is_inside_a_place() {
    let words: Vec<&str> = outside::WHEN.iter().map(|(word, _)| *word).collect();
    assert_eq!(
        words,
        [
            "always",
            "morning",
            "daytime",
            "evening",
            "night",
            "someone_near",
            "person_near",
            "bot_near",
            "frog_near",
            "street_empty",
            "owner_watching",
            "disaster",
            "now_and_then",
        ],
        "a new `when` has to be read against E20 and W59 before it is added here"
    );
    for word in &words {
        for banned in ["inside", "home", "in_", "room", "here", "visitor"] {
            assert!(!word.contains(banned), "{word}");
        }
    }
}

// E22, L35
#[tokio::test]
async fn a_bot_cannot_see_a_pets_rules_or_use_them() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let spot = server.house(&ann);
    let scout = server.bot(&ann, &room, "Scout").await;
    server
        .save(
            &ann,
            spot,
            json!({ "pets": [{ "species": "fox", "name": "pepper", "rules": [{ "when": "night", "do": "hide" }] }] }),
        )
        .await
        .ok();
    server.get(&scout, &Server::url(spot)).await.refused(StatusCode::FORBIDDEN);
    server.get(&scout, "/api/outside").await.refused(StatusCode::FORBIDDEN);
    let tools = server
        .post(
            &scout,
            "/mcp",
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
        )
        .await
        .ok();
    let names = tools["result"]["tools"].as_array().unwrap();
    for tool in names {
        let name = text(&tool["name"]);
        assert!(!name.contains("outside") && !name.contains("pet"), "{name}");
    }
    // L35: the one way a bot reaches an outside is its own project's building, and those two tools
    // read nothing of anyone else's place (tests/projects.rs covers what they do).
    let listed: Vec<String> = names.iter().map(|tool| text(&tool["name"])).collect();
    for only in ["building_catalog", "decorate_building"] {
        assert!(listed.iter().any(|name| name == only), "{only}: {listed:?}");
    }
    // What `look` shows of The Commons has no outside and no pet in it.
    let player = server.guest().await;
    let looked = server.get(&player, &format!("/api/worlds/{COMMONS}/look")).await.ok().to_string();
    let public = server
        .post(
            "",
            "/public/mcp",
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
        )
        .await
        .body
        .to_string();
    for shown in [looked, public] {
        for word in ["\"outside\"", "pepper"] {
            assert!(!shown.contains(word), "{word} in {shown}");
        }
    }
}

// E36
#[tokio::test]
async fn a_big_place_has_one_outside_and_any_of_its_tiles_reaches_it() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let (x, y) = server.square(&ann);
    let walls = json!({ "parts": { "ground": { "style": "snow" } } });
    // Saved from its far tile, it's stored on its first.
    server.save(&ann, (x + 1, y + 1), walls.clone()).await.ok();
    for tile in [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)] {
        let seen = server.get(&ann.key, &Server::url(tile)).await.ok();
        assert_eq!(seen["outside"], walls);
        assert_eq!((seen["x"].as_i64(), seen["y"].as_i64()), (Some(x), Some(y)));
        assert_eq!(seen["kind"], "square");
    }
    let listed = server.get("", "/api/worlds/commons/outsides").await.ok();
    assert_eq!(listed["outsides"], json!([[x, y, walls]]));
    // A tile beside it is nothing to do with it.
    server.get(&ann.key, &Server::url((x + 2, y))).await.refused(StatusCode::NOT_FOUND);
    server.delete(&ann.key, &Server::url((x + 1, y))).await.ok();
    assert_eq!(server.outside_of(&ann, (x, y)).await, json!({}));
}

// E14, E36
#[tokio::test]
async fn a_park_has_the_parts_of_a_square_and_starts_with_nothing_in_its_middle() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.free_spots()[0];
    server.green(&ann, spot);
    assert_eq!(server.get(&ann.key, &Server::url(spot)).await.ok()["kind"], "green");
    let body = json!({ "body": { "x": 3, "y": 3, "w": 6, "d": 6, "floors": 1 } });
    server.save(&ann, spot, body).await.refused(StatusCode::BAD_REQUEST);
    let walls = json!({ "parts": { "walls": { "style": "brick" } } });
    server.save(&ann, spot, walls).await.refused(StatusCode::BAD_REQUEST);
    // Nothing chosen, so its middle is free. Choosing something takes it up.
    let oak = json!({ "kind": "oak", "x": 7, "y": 7 });
    server.save(&ann, spot, json!({ "items": [oak] })).await.ok();
    let statue = json!({ "parts": { "centerpiece": { "style": "statue" } }, "items": [oak] });
    server.save(&ann, spot, statue).await.refused(StatusCode::CONFLICT);
}

// E10, E30
#[tokio::test]
async fn every_kind_of_place_has_an_outside_and_the_newer_ones_wear_a_waiting_one() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    server.house(&ann);
    let document = json!({ "parts": { "ground": { "style": "leaves" } } });
    // Each one is a newer place, stored in `world_more_places`, as `place_later` puts it up.
    let bar = server.place_later(&ann, "bar", 8);
    server.save(&ann, bar, document.clone()).await.ok();
    assert_eq!(server.get(&ann.key, &Server::url(bar)).await.ok()["kind"], "bar");
    assert_eq!(server.outside_of(&ann, bar).await, document);
    let listed = server.get("", "/api/worlds/commons/outsides").await.ok();
    assert_eq!(listed["outsides"].as_array().unwrap().len(), 1);
    // It comes down, and the next bar wears it.
    server.take_down(&ann, bar, now());
    let next = server.free_spots()[1];
    server.put_up(&ann, "bar", next, now() + 8 * DAY);
    assert_ne!(next, bar);
    assert_eq!(server.outside_of(&ann, next).await, document);
}

// E23, E24
#[test]
fn every_template_passes_the_checks_on_an_empty_tile_of_each_kind_it_lists() {
    let templates = outside::templates();
    assert!(templates.len() >= 8);
    let kinds: HashSet<&str> =
        templates.iter().flat_map(|template| template.kinds.iter().copied()).collect();
    assert_eq!(kinds, HashSet::from(KINDS));
    for template in &templates {
        let document: Outside = serde_json::from_value(template.outside.clone())
            .unwrap_or_else(|error| panic!("{}: {error}", template.template));
        for kind in template.kinds {
            if let Err(error) = outside::check(kind, &document) {
                panic!("{} on a {kind}: {} {}", template.template, error.status, error.message);
            }
        }
    }
}

// E23, E24
#[tokio::test]
async fn a_template_can_be_saved_on_a_place_of_a_kind_it_lists() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let house = server.house(&ann);
    let cafe = server.place_later(&ann, "cafe", 8);
    let square = server.square(&ann);
    for template in outside::templates() {
        for (kind, spot) in [("house", house), ("cafe", cafe), ("square", square)] {
            let answer = server.save(&ann, spot, template.outside.clone()).await;
            if template.kinds.contains(&kind) {
                answer.ok();
            } else {
                answer.refused(StatusCode::BAD_REQUEST);
            }
        }
    }
}

const KINDS: [&str; 11] = [
    "house", "cafe", "club", "studio", "lounge", "bar", "office", "workshop", "library", "square",
    "green",
];

/// E34: the pack's parts that a place of `kind` has, and all its items, as a document.
fn pack_on(pack: &outside::Pack, kind: &str) -> Value {
    let parts: serde_json::Map<String, Value> = pack.outside["parts"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(word, _)| {
            outside::PARTS
                .iter()
                .any(|part| part.part == word.as_str() && part.kinds.contains(&kind))
        })
        .map(|(word, chosen)| (word.clone(), chosen.clone()))
        .collect();
    json!({ "parts": parts, "items": pack.outside["items"] })
}

// E33
#[tokio::test]
async fn there_is_one_pack_and_the_catalog_lists_it_for_people() {
    let packs = outside::packs();
    assert_eq!(packs.len(), 1);
    assert_eq!(packs[0].pack, "fall");
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let catalog = server.get(&ann.key, "/api/outside").await.ok();
    let listed = catalog["packs"].as_array().expect("packs");
    assert_eq!(listed.len(), 1, "{catalog}");
    assert_eq!(listed[0]["pack"], "fall");
    assert_eq!(listed[0]["outside"], packs[0].outside);
    // Nothing in it that anyone typed: every value is a number or a listed word.
    for (word, chosen) in packs[0].outside["parts"].as_object().unwrap() {
        assert!(chosen.as_object().unwrap().values().all(|value| !value.is_array()), "{word}");
    }
}

// E34: a pack saved on an empty tile of any kind is accepted, with the parts that kind has.
#[tokio::test]
async fn a_pack_the_page_puts_up_is_taken_as_any_other_change() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let house = server.house(&ann);
    let square = server.square(&ann);
    for pack in outside::packs() {
        for (kind, spot) in [("house", house), ("square", square)] {
            let document = pack_on(&pack, kind);
            let answer = server.save(&ann, spot, document.clone()).await.ok();
            assert_eq!(
                answer["outside"]["items"].as_array().map(Vec::len),
                document["items"].as_array().map(Vec::len)
            );
        }
    }
}

// E34, E35: a pack passes the checks on an empty tile of every kind, with the parts that kind has.
#[test]
fn every_pack_passes_the_checks_on_an_empty_tile_of_every_kind() {
    for pack in outside::packs() {
        for kind in KINDS {
            let document = parse(pack_on(&pack, kind));
            if let Err(error) = outside::check(kind, &document) {
                panic!("{} on a {kind}: {} {}", pack.pack, error.status, error.message);
            }
        }
        // A square has no sign, no window boxes: those are left out and nothing else is.
        let on_a_square = pack_on(&pack, "square");
        assert!(on_a_square["parts"].get("window_boxes").is_none());
        assert!(on_a_square["parts"].get("sign").is_none());
        assert!(on_a_square["parts"].get("ground").is_some());
        assert!(pack_on(&pack, "house")["parts"].get("window_boxes").is_some());
    }
}

// E35: a kind is in one pack and no template, and every word is in the catalog.
#[test]
fn a_kind_in_a_pack_is_in_one_pack_and_no_template() {
    let packs = outside::packs();
    let mut seen: Vec<String> = Vec::new();
    for pack in &packs {
        let items = pack.outside["items"].as_array().unwrap();
        assert!(!items.is_empty(), "{}", pack.pack);
        let mut mine: HashSet<String> = HashSet::new();
        for item in items {
            let kind = text(&item["kind"]);
            assert!(
                outside::YARD.iter().any(|yard| yard.kind == kind),
                "{kind} is not in the catalog"
            );
            mine.insert(kind);
        }
        for kind in &mine {
            assert!(!seen.contains(kind), "{kind} is in two packs");
        }
        seen.extend(mine);
        for (word, chosen) in pack.outside["parts"].as_object().unwrap() {
            let part = outside::PARTS
                .iter()
                .find(|part| part.part == word)
                .unwrap_or_else(|| panic!("{word} is not a part"));
            if let Some(style) = chosen.get("style") {
                assert!(
                    part.styles.iter().any(|(each, _)| *each == style.as_str().unwrap()),
                    "{word} {style}"
                );
            }
        }
    }
    for template in outside::templates() {
        for item in template.outside["items"].as_array().into_iter().flatten() {
            let kind = text(&item["kind"]);
            assert!(
                !seen.contains(&kind),
                "{kind} is in a pack and in the {} template",
                template.template
            );
        }
    }
}

// E34: a template that is offered is in season, and Halloween is over.
#[tokio::test]
async fn harvest_is_offered_as_a_template_and_haunted_is_not() {
    let words: Vec<&str> = outside::templates().iter().map(|template| template.template).collect();
    assert!(words.contains(&"harvest"), "{words:?}");
    assert!(!words.contains(&"haunted"), "{words:?}");
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let catalog = server.get(&ann.key, "/api/outside").await.ok();
    let offered: Vec<String> = catalog["templates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|each| text(&each["template"]))
        .collect();
    assert!(offered.iter().any(|word| word == "harvest"), "{offered:?}");
    assert!(!offered.iter().any(|word| word == "haunted"), "{offered:?}");
}

// E26
#[tokio::test]
async fn the_view_says_how_many_outsides_there_are_and_a_stamp_that_changes_with_each() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let first = server.view().await;
    assert_eq!(first["outsides"], json!([0, 0]));

    let spot = server.house(&ann);
    server.save(&ann, spot, walls(1)).await.ok();
    let one = server.view().await;
    assert_eq!(one["outsides"][0], 1);
    assert_ne!(one["outsides"], first["outsides"]);
    // Two numbers, and nothing else of an outside is in a view.
    assert_eq!(one["outsides"].as_array().unwrap().len(), 2);
    assert!(!one.to_string().contains("brick"));

    server.save(&ann, spot, walls(2)).await.ok();
    let edited = server.view().await;
    assert_eq!(edited["outsides"][0], 1);
    assert_ne!(edited["outsides"], one["outsides"]);

    let bo_spot = server.house(&bo);
    server.save(&bo, bo_spot, walls(3)).await.ok();
    let two = server.view().await;
    assert_eq!(two["outsides"][0], 2);

    // A place coming down takes its outside off the map, and the stamp moves.
    server.take_down(&ann, spot, now() + DAY);
    let down = server.view().await;
    assert_eq!(down["outsides"][0], 1);
    assert_ne!(down["outsides"], two["outsides"]);

    // And a place going up brings the one that was waiting back.
    server.put_up(&ann, "house", spot, now() + 2 * DAY);
    let up = server.view().await;
    assert_eq!(up["outsides"][0], 2);
    assert_ne!(up["outsides"], down["outsides"]);

    server.delete(&bo.key, &Server::url(bo_spot)).await.ok();
    let cleared = server.view().await;
    assert_eq!(cleared["outsides"][0], 1);
    assert_ne!(cleared["outsides"], up["outsides"]);
}

// E26: a stamp is never the same twice, whatever is deleted or added between.
#[tokio::test]
async fn a_stamp_does_not_come_back_after_a_row_goes_and_another_arrives() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    let mut seen = HashSet::new();
    for n in 0..20 {
        server.save(&ann, spot, walls(n)).await.ok();
        assert!(seen.insert(server.stamp()), "{:?} came back", server.stamp());
        server.delete(&ann.key, &Server::url(spot)).await.ok();
        assert_eq!(server.stamp(), (0, 0));
    }
}

// E27
#[tokio::test]
async fn the_map_lists_every_outside_that_has_its_place_for_anyone() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let a = server.house(&ann);
    let b = server.house(&bo);
    server.save(&ann, a, walls(1)).await.ok();
    server.save(&bo, b, walls(2)).await.ok();
    let listed = server.on_the_map().await;
    assert_eq!(listed.len(), 2);
    let mut expected = vec![json!([a.0, a.1, walls(1)]), json!([b.0, b.1, walls(2)])];
    expected.sort_by_key(|row| (row[1].as_i64().unwrap(), row[0].as_i64().unwrap()));
    assert_eq!(listed, expected);

    // One that is waiting isn't in it.
    server.take_down(&ann, a, now() + DAY);
    let listed = server.on_the_map().await;
    assert_eq!(listed, vec![json!([b.0, b.1, walls(2)])]);

    // It is limited like a view, by address.
    let mut served = 0;
    let mut refused = 0;
    for _ in 0..260 {
        let reply = server
            .send_from(
                "203.0.113.9",
                "",
                Method::GET,
                &format!("/api/worlds/{COMMONS}/outsides"),
                None,
            )
            .await;
        match reply.status {
            StatusCode::OK => served += 1,
            StatusCode::TOO_MANY_REQUESTS => refused += 1,
            other => panic!("{other}"),
        }
    }
    assert!(refused > 0 && served <= 240, "{served} served, {refused} refused");
}

// E30
#[tokio::test]
async fn when_a_place_comes_down_its_outside_waits_for_the_next_of_that_kind() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    server.save(&ann, spot, walls(11)).await.ok();
    let player = server.player_of(&ann);

    server.take_down(&ann, spot, now() + DAY);
    assert_eq!(server.state.db.outside_rows(COMMONS, player).unwrap(), 1, "it waits");
    assert!(server.on_the_map().await.is_empty());

    // The next house wears it, wherever it goes.
    let next = server.free_spots().into_iter().find(|&at| at != spot).unwrap();
    server.put_up(&ann, "house", next, now() + 2 * DAY);
    assert_eq!(server.outside_of(&ann, next).await, walls(11));
    assert_eq!(server.state.db.outside_rows(COMMONS, player).unwrap(), 1, "it moved");
    assert_eq!(server.on_the_map().await.len(), 1);

    // Coming down again and going up in the very same place shows it again, with nothing to move.
    server.take_down(&ann, next, now() + 3 * DAY);
    server.put_up(&ann, "house", next, now() + 4 * DAY);
    assert_eq!(server.outside_of(&ann, next).await, walls(11));
    assert_eq!(server.state.db.outside_rows(COMMONS, player).unwrap(), 1);
}

// E30
#[tokio::test]
async fn a_place_of_another_kind_does_not_take_a_waiting_outside_and_the_newest_one_wins() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let house = server.house(&ann);
    server.save(&ann, house, walls(1)).await.ok();
    let cafe = server.place_later(&ann, "cafe", 8);
    let club = server.place_later(&ann, "club", 8);
    server.save(&ann, cafe, walls(2)).await.ok();
    server.save(&ann, club, walls(3)).await.ok();

    // A cafe and a club come down, and a studio goes up: it takes neither of them.
    server.take_down(&ann, cafe, now() + 9 * DAY);
    server.take_down(&ann, club, now() + 9 * DAY);
    let studio = server.place_later(&ann, "studio", 10);
    assert_eq!(server.outside_of(&ann, studio).await, json!({}));
    assert_eq!(server.outside_of(&ann, house).await, walls(1));

    // A club goes up, and it is the club's outside that it wears.
    let next_club = server.place_later(&ann, "club", 12);
    assert_eq!(server.outside_of(&ann, next_club).await, walls(3));
    let next_cafe = server.place_later(&ann, "cafe", 12);
    assert_eq!(server.outside_of(&ann, next_cafe).await, walls(2));
}

// E30, W68: a disaster brings places down through the same code.
#[tokio::test]
async fn a_disaster_leaves_an_outside_waiting_for_the_owners_next_place() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    server.save(&ann, spot, walls(21)).await.ok();
    let square = server.square(&ann);
    server.save(&ann, square, json!({ "parts": { "ground": { "style": "snow" } } })).await.ok();
    assert_eq!(server.on_the_map().await.len(), 2);

    server.run_a_disaster();
    assert!(server.on_the_map().await.is_empty());
    assert_eq!(server.stamp().0, 0);
    assert_eq!(server.state.db.outside_rows(COMMONS, server.player_of(&ann)).unwrap(), 2);

    let next = server.free_spots()[0];
    server.put_up(&ann, "house", next, now() + 20 * DAY);
    assert_eq!(server.outside_of(&ann, next).await, walls(21));
    // And a new square wears the old square's.
    let again = server.free_spots()[0];
    let tile = Tile { kind: Kind::Square, level: 0, player: Some(server.player_of(&ann) as u64) };
    server.write(&[Change::Set((again.0 as u16, again.1 as u16), tile)]);
    assert_eq!(
        server.outside_of(&ann, again).await,
        json!({ "parts": { "ground": { "style": "snow" } } })
    );
}

// E31, S33
#[tokio::test]
async fn a_row_for_a_tile_shows_only_while_its_owner_and_kind_stand_on_it() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let spot = server.house(&ann);
    server.save(&ann, spot, walls(31)).await.ok();
    server.take_down(&ann, spot, now() + DAY);

    // Bo puts a house on Ann's old tile: it wears nothing of hers.
    server.put_up(&bo, "house", spot, now());
    assert_eq!(server.outside_of(&bo, spot).await, json!({}));
    assert!(server.on_the_map().await.is_empty());
    assert_eq!(server.stamp().0, 0);
    assert_eq!(server.state.db.outside_at(COMMONS, spot.0, spot.1).unwrap(), None);

    // And Bo's own outside there is his, apart from hers.
    server.save(&bo, spot, walls(32)).await.ok();
    assert_eq!(server.outside_of(&bo, spot).await, walls(32));
    assert_eq!(server.on_the_map().await.len(), 1);

    // Hers wasn't lost: her next house takes it.
    let hers = server.free_spots().into_iter().find(|&at| at != spot).unwrap();
    server.put_up(&ann, "house", hers, now() + 2 * DAY);
    assert_eq!(server.outside_of(&ann, hers).await, walls(31));
    assert_eq!(server.outside_of(&bo, spot).await, walls(32));
    assert_eq!(server.on_the_map().await.len(), 2);

    // A tile with her old house's kind gone: a cafe on it wears no house outside.
    server.take_down(&bo, spot, now() + 3 * DAY);
    server.take_down(&ann, hers, now() + 3 * DAY);
    let cafe = server.free_spots().into_iter().find(|&at| at != spot && at != hers).unwrap();
    server.put_up(&ann, "cafe", cafe, now() + 20 * DAY);
    assert_eq!(server.outside_of(&ann, cafe).await, json!({}));
}

// E30, E32: a person cannot pile rows up by placing, editing and bulldozing.
#[tokio::test]
async fn placing_editing_and_bulldozing_keep_a_row_per_outside_not_a_row_per_try() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let player = server.player_of(&ann);
    let mut spot = server.house(&ann);
    for round in 0..8 {
        server.save(&ann, spot, walls(round)).await.ok();
        assert_eq!(server.state.db.outside_rows(COMMONS, player).unwrap(), 1, "round {round}");
        server.take_down(&ann, spot, now() + (round + 1) * DAY);
        let next = server.free_spots().into_iter().find(|&at| at != spot).unwrap();
        server.put_up(&ann, "house", next, now() + (round + 1) * DAY + 1);
        assert_eq!(server.outside_of(&ann, next).await, walls(round));
        assert_eq!(server.state.db.outside_rows(COMMONS, player).unwrap(), 1);
        spot = next;
    }

    // And what waits with nothing to take it is held to a few.
    for n in 0..20 {
        server
            .state
            .db
            .save_outside(COMMONS, player, "cafe", (n, 100), &walls(n).to_string(), now() + n)
            .unwrap();
    }
    let waiting = server.state.db.outside_rows(COMMONS, player).unwrap() - 1;
    assert_eq!(waiting, agenthangout::db::MAX_WAITING_OUTSIDES);
}

// E32
#[tokio::test]
async fn the_keeper_removing_a_player_and_a_person_deleting_their_account_delete_what_they_made() {
    let (server, keeper) = Server::start_with_keeper().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let cy = server.person("Cy").await;
    let (a, b, c) = (server.house(&ann), server.house(&bo), server.house(&cy));
    for (who, at) in [(&ann, a), (&bo, b), (&cy, c)] {
        server.save(who, at, walls(41)).await.ok();
    }

    let player = server.player_of(&ann);
    server
        .post(&keeper.key, &format!("/api/worlds/{COMMONS}/players/{player}/remove"), json!({}))
        .await
        .ok();
    assert_eq!(server.state.db.outside_rows(COMMONS, player).unwrap(), 0);
    assert_eq!(server.on_the_map().await.len(), 2);

    let bo_player = server.player_of(&bo);
    server.state.db.delete_person(&bo.id, "Bo").unwrap();
    assert_eq!(server.state.db.outside_rows(COMMONS, bo_player).unwrap(), 0);
    let left = server.on_the_map().await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0][2], walls(41));
}

// E32: starting a world again goes with its row.
#[tokio::test]
async fn deleting_a_worlds_row_takes_its_outsides_with_it() {
    let server = Server::start().await;
    let bo = server.person("Bo").await;
    server.state.db.create_person("owner", "Owner", "k", "f", now()).unwrap();
    let new = agenthangout::db::NewWorld {
        id: "scratch",
        name: "Scratch",
        kind: "own",
        seed: 99,
        size: world::OWN_SIZE,
        owner_id: Some("owner"),
        open: true,
        invite_code: Some("scratch-code"),
        highway: &world::highway(99, &world::terrain(99, world::OWN_SIZE)),
        now: now(),
    };
    server.state.db.insert_world(&new, worlds::MAX_OWN_WORLDS).unwrap();
    let player = server.player_of(&bo);
    server
        .state
        .db
        .save_outside("scratch", player, "house", (5, 5), &walls(1).to_string(), now())
        .unwrap();
    assert_eq!(server.state.db.outside_rows("scratch", player).unwrap(), 1);
    // Their world's owner leaves, which deletes its row and nothing of Bo's.
    server.state.db.delete_person("owner", "Owner").unwrap();
    assert_eq!(server.state.db.outside_rows("scratch", player).unwrap(), 0);
    assert!(server.state.db.player(player).unwrap().is_some());
}

// ----------------------------------------------------------------------------------------------
// The checks alone (E2, E3, E10, E13 to E19)
// ----------------------------------------------------------------------------------------------

// E13, E14, E16
#[test]
fn the_checks_use_the_default_body_for_a_building_with_none_and_never_overflow() {
    for kind in ["house", "cafe", "club", "studio"] {
        let body = outside::default_body(kind).unwrap();
        assert!(body.x >= 1 && body.y >= 1);
        assert!(body.w >= outside::MIN_BODY && body.d >= outside::MIN_BODY);
        assert!(body.x + body.w < outside::GRID && body.y + body.d < outside::GRID);
        assert_eq!(body.floors, 1);
    }
    assert!(outside::default_body("square").is_none());

    // A tree on the building's default cells is refused, and one just off them is not.
    let tree = |x: i64, y: i64| parse(json!({ "items": [{ "kind": "oak", "x": x, "y": y }] }));
    let body = outside::default_body("club").unwrap();
    assert_eq!(
        outside::check("club", &tree(body.x, body.y)).unwrap_err().status,
        StatusCode::CONFLICT
    );
    assert!(outside::check("club", &tree(body.x + body.w, body.y)).is_ok());
    // Numbers of any size are an answer and not a panic.
    let huge = parse(json!({ "items": [{ "kind": "oak", "x": i64::MAX, "y": i64::MIN }] }));
    assert_eq!(outside::check("house", &huge).unwrap_err().status, StatusCode::CONFLICT);
    let wide = parse(
        json!({ "body": { "x": i64::MAX, "y": i64::MAX, "w": i64::MAX, "d": i64::MAX, "floors": 1 } }),
    );
    assert_eq!(outside::check("house", &wide).unwrap_err().status, StatusCode::BAD_REQUEST);
    assert!(outside::check("tavern", &parse(json!({}))).is_err());
}

// E10: every part answers to its own catalog entry.
#[test]
fn every_style_in_the_catalog_passes_the_checks_on_every_kind_that_has_the_part() {
    for part in outside::PARTS {
        for kind in part.kinds {
            for (style, _) in part.styles {
                let chosen =
                    if part.many { json!({ "on": [style] }) } else { json!({ "style": style }) };
                let document = parse(json!({ "parts": { part.part: chosen } }));
                outside::check(kind, &document).unwrap_or_else(|error| {
                    panic!("{} {style} on a {kind}: {}", part.part, error.message)
                });
            }
        }
        for kind in KINDS {
            if part.kinds.contains(&kind) {
                continue;
            }
            let document = parse(json!({ "parts": { part.part: {} } }));
            assert_eq!(
                outside::check(kind, &document).unwrap_err().status,
                StatusCode::BAD_REQUEST,
                "{} on a {kind}",
                part.part
            );
        }
    }
    for yard in outside::YARD {
        let item = if yard.tint {
            json!({ "kind": yard.kind, "x": 0, "y": 0, "color": 255 })
        } else {
            json!({ "kind": yard.kind, "x": 0, "y": 0 })
        };
        let document = parse(json!({ "items": [item] }));
        outside::check("house", &document)
            .unwrap_or_else(|error| panic!("{}: {}", yard.kind, error.message));
    }
}

// ----------------------------------------------------------------------------------------------
// The art: every word in the catalog is drawn (E12, E4)
// ----------------------------------------------------------------------------------------------

fn web(name: &str) -> String {
    std::fs::read_to_string(format!("{}/web/{name}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|error| panic!("web/{name}: {error}"))
}

/// Whether `word` is in `source` as an object key, a method name or a quoted string.
fn mentions(source: &str, word: &str) -> bool {
    let bytes = source.as_bytes();
    let part_of_a_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut from = 0;
    while let Some(found) = source[from..].find(word) {
        let start = from + found;
        let end = start + word.len();
        let before = start.checked_sub(1).map(|at| bytes[at]);
        let after = bytes.get(end).copied();
        let quoted = matches!(before, Some(b'\'' | b'"' | b'`'))
            && matches!(after, Some(b'\'' | b'"' | b'`'));
        let named = !before.is_some_and(part_of_a_word) && {
            let rest = source[end..].trim_start_matches(' ');
            rest.starts_with(':') || rest.starts_with('(')
        };
        if quoted || named {
            return true;
        }
        from = end;
    }
    false
}

/// Says which words nothing in `files` draws, and which list they belong to. None if all are drawn.
fn undrawn(what: &str, words: Vec<String>, files: &[&str]) -> Option<String> {
    let source: String = files.iter().map(|file| web(file)).collect::<Vec<_>>().join("\n");
    let mut seen = HashSet::new();
    let missing: Vec<&String> =
        words.iter().filter(|word| seen.insert(*word) && !mentions(&source, word)).collect();
    (!missing.is_empty())
        .then(|| format!("{what}: nothing in web/{} draws {missing:?}", files.join(" or web/")))
}

#[track_caller]
fn all_drawn(lists: Vec<Option<String>>) {
    let missing: Vec<String> = lists.into_iter().flatten().collect();
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

// E12
#[test]
fn every_style_of_every_part_has_art() {
    let mut building = Vec::new();
    let mut spec = Vec::new();
    let mut ground = Vec::new();
    for part in outside::PARTS {
        let words = part.styles.iter().map(|(word, _)| *word).filter(|word| *word != "none");
        let list = match part.part {
            "door_side" => &mut spec,
            "string_lights" | "fence" | "gate" | "ground" | "path" | "centerpiece" => &mut ground,
            _ => &mut building,
        };
        list.extend(words.map(str::to_string));
    }
    all_drawn(vec![
        undrawn("styles of the building's parts", building, &["outside-building.js"]),
        undrawn(
            "styles of the ground, fence, gate, path, centrepiece and string lights",
            ground,
            &["outside-yard.js"],
        ),
        undrawn("styles of door_side", spec, &["outside-spec.js"]),
    ]);
}

// E12
#[test]
fn every_kind_of_thing_in_a_yard_has_art() {
    let words = outside::YARD.iter().map(|kind| kind.kind.to_string()).collect();
    all_drawn(vec![undrawn("yard kinds", words, &["outside-yard.js", "outside-decor.js"])]);
}

// E12, E18, E19
#[test]
fn every_species_and_every_when_and_do_has_art_or_a_rule_in_the_pets_page() {
    let species = outside::SPECIES.iter().map(|(word, _)| word.to_string()).collect();
    let when = outside::WHEN.iter().map(|(word, _)| word.to_string()).collect();
    let acts = outside::DO.iter().map(|(word, _)| word.to_string()).collect();
    all_drawn(vec![
        undrawn("pet species", species, &["pets.js"]),
        undrawn("pet rule whens", when, &["pets.js"]),
        undrawn("pet rule dos", acts, &["pets.js"]),
    ]);
}

/// The numbers after each `name:` in a block of `outside-spec.js` that begins at `marker`.
fn table(source: &str, marker: &str) -> Vec<(String, Vec<i64>)> {
    let start =
        source.find(marker).unwrap_or_else(|| panic!("web/outside-spec.js has no {marker}"));
    let block = &source[start..];
    let block = &block[..block.find("};").unwrap()];
    block
        .lines()
        .skip(1)
        .filter_map(|line| {
            let (name, rest) = line.trim().split_once(':')?;
            let numbers = rest
                .split(|c: char| !(c.is_ascii_digit() || c == '-'))
                .filter(|piece| !piece.is_empty() && *piece != "-")
                .map(|piece| piece.parse().unwrap())
                .collect();
            Some((name.trim().to_string(), numbers))
        })
        .collect()
}

// E15
#[test]
fn the_yard_sizes_in_the_page_match_the_catalog() {
    let sizes = table(&web("outside-spec.js"), "export const YARD_SIZES = {");
    assert_eq!(sizes.len(), outside::YARD.len(), "YARD_SIZES has a different number of kinds");
    for (kind, numbers) in &sizes {
        let yard = outside::yard_kind_of(kind).unwrap_or_else(|| {
            panic!("web/outside-spec.js YARD_SIZES has {kind}, which the catalog doesn't")
        });
        assert_eq!(numbers, &vec![yard.w, yard.h], "{kind}");
    }
    for yard in outside::YARD {
        assert!(sizes.iter().any(|(kind, _)| kind == yard.kind), "YARD_SIZES lacks {}", yard.kind);
    }
}

// E13
#[test]
fn the_default_bodies_in_the_page_match_default_body() {
    let bodies = table(&web("outside-spec.js"), "const BODIES = {");
    assert_eq!(bodies.len(), 9);
    for (kind, numbers) in bodies {
        let body = outside::default_body(&kind).unwrap_or_else(|| panic!("BODIES has {kind}"));
        // The numbers in the line are x, y, w, d and floors, in that order.
        assert_eq!(numbers, vec![body.x, body.y, body.w, body.d, body.floors], "{kind}");
    }
}

// ----------------------------------------------------------------------------------------------
// Pulling it apart: strangers, bots, storage, the stamp, going back a release
// ----------------------------------------------------------------------------------------------

impl Server {
    /// A server on a database file, so a test can also open the file as an older release would.
    async fn start_on(path: &str) -> Self {
        Self::boot(Db::open(path).unwrap(), None, &[]).await
    }

    /// A body sent as it is, whatever it holds.
    async fn put_text(&self, who: &Who, at: (i64, i64), body: &str) -> Reply {
        let response = self
            .client
            .put(format!("{}{}", self.base, Self::url(at)))
            .bearer_auth(&who.key)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        Reply { status, body: serde_json::from_str(&text).unwrap_or(Value::String(text)) }
    }

    fn live(&self) -> Vec<(i64, i64, String)> {
        self.state.db.live_outsides(COMMONS).unwrap()
    }
}

// E2, W8: a refusal never says back a long word that was sent.
#[tokio::test]
async fn a_refusal_does_not_say_a_long_word_back() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    let long = "x".repeat(30_000);
    let item = |extra: Value| {
        let mut item = json!({ "kind": "oak", "x": 1, "y": 13 });
        item.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        json!({ "items": [item] })
    };
    let rule = |when: &str, action: &str| json!({ "pets": [{ "species": "dog", "name": "biscuit", "rules": [{ "when": when, "do": action }] }] });
    let documents = [
        json!({ "parts": { long.clone(): {} } }),
        json!({ "parts": { "walls": { "style": long.clone() } } }),
        json!({ "parts": { "rooftop": { "on": [long.clone()] } } }),
        json!({ "parts": { "walls": { "style": 1, "color": long.clone() } } }),
        json!({ long.clone(): 1 }),
        json!({ "parts": { "walls": { long.clone(): 1 } } }),
        item(json!({ "kind": long.clone() })),
        item(json!({ long.clone(): 1 })),
        rule(&long, "sit"),
        rule("always", &long),
        json!({ "pets": [{ "species": long.clone(), "name": "biscuit" }] }),
        json!({ "pets": [{ "species": "dog", "name": long.clone() }] }),
    ];
    for document in documents {
        let reply = server.save(&ann, spot, document).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.body.to_string().len());
        let said = reply.body.to_string();
        assert!(said.len() < 500, "a refusal of {} bytes: {}", said.len(), &said[..200]);
        assert!(!said.contains(&"x".repeat(200)));
    }
}

// E2, E3, E13: a number too big for any integer, and nesting too deep for anyone, are a plain 400 and
// change nothing.
#[tokio::test]
async fn numbers_that_overflow_and_documents_nested_deep_are_a_clean_400() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    server.save(&ann, spot, walls(7)).await.ok();
    let deep_array = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    let deep_object = format!("{}1{}", "{\"parts\":".repeat(3_000), "}".repeat(3_000));
    let deep_part = format!(
        "{{\"parts\":{{\"rooftop\":{{\"on\":{}{}}}}}}}",
        "[".repeat(5_000),
        "]".repeat(5_000)
    );
    let bodies = [
        deep_array,
        deep_object,
        deep_part,
        r#"{"body":{"x":99999999999999999999999,"y":1,"w":5,"d":5,"floors":1}}"#.to_string(),
        r#"{"body":{"x":1,"y":1,"w":5,"d":5,"floors":18446744073709551616}}"#.to_string(),
        r#"{"items":[{"kind":"oak","x":9223372036854775808,"y":1}]}"#.to_string(),
        r#"{"items":[{"kind":"oak","x":-9223372036854775809,"y":1}]}"#.to_string(),
        r#"{"items":[{"kind":"oak","x":1,"y":1,"facing":-9223372036854775808}]}"#.to_string(),
        r#"{"items":[{"kind":"oak","x":1,"y":1,"facing":9223372036854775807}]}"#.to_string(),
        r#"{"parts":{"walls":{"color":1e400}}}"#.to_string(),
        r#"{"parts":{"walls":{"color":18446744073709551616}}}"#.to_string(),
        r#"{"parts":{"fence":{"number":-9223372036854775808}}}"#.to_string(),
        r#"{"pets":[{"species":"dog","name":"biscuit","coat":9223372036854775807}]}"#.to_string(),
        String::new(),
        "null".to_string(),
    ];
    for body in bodies {
        let reply = server.put_text(&ann, spot, &body).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", &body[..body.len().min(60)]);
        assert!(reply.body.to_string().len() < 500, "{}", reply.body);
    }
    assert_eq!(server.outside_of(&ann, spot).await, walls(7));
}

/// The biggest document the checks let through for a house, built from the catalog's own limits:
/// every part at its longest, 40 items, 4 pets with 6 rules each.
fn largest_document() -> Value {
    let mut parts = serde_json::Map::new();
    for part in outside::PARTS.iter().filter(|part| part.kinds.contains(&"house")) {
        let words: Vec<&str> = part.styles.iter().map(|(word, _)| *word).collect();
        let mut chosen = serde_json::Map::new();
        if part.many {
            chosen.insert("on".to_string(), json!(words));
        } else {
            let longest = words.iter().max_by_key(|word| word.len()).unwrap();
            chosen.insert("style".to_string(), json!(longest));
        }
        if part.colors {
            chosen.insert("color".to_string(), json!(outside::MAX_COLOR));
        }
        if let Some(amount) = part.number {
            chosen.insert("number".to_string(), json!(amount.max));
        }
        parts.insert(part.part.to_string(), Value::Object(chosen));
    }
    let kind = outside::YARD
        .iter()
        .filter(|kind| kind.tint && kind.w == 1 && kind.h == 1)
        .max_by_key(|kind| kind.kind.len())
        .unwrap()
        .kind;
    // The cells clear of a house's default building, as in E17.
    let mut items = Vec::new();
    for y in 0..16 {
        for x in 0..16 {
            if (!(3..13).contains(&x) || !(4..12).contains(&y)) && items.len() < outside::MAX_ITEMS
            {
                items.push(json!({ "kind": kind, "x": x, "y": y, "facing": 3, "color": outside::MAX_COLOR }));
            }
        }
    }
    let species = outside::SPECIES.iter().map(|(word, _)| *word).max_by_key(|word| word.len());
    let name = outside::PET_NAMES.iter().max_by_key(|word| word.len()).unwrap();
    let when = outside::WHEN.iter().map(|(word, _)| *word).max_by_key(|word| word.len()).unwrap();
    let action = outside::DO.iter().map(|(word, _)| *word).max_by_key(|word| word.len()).unwrap();
    let rules: Vec<Value> =
        (0..outside::MAX_RULES).map(|_| json!({ "when": when, "do": action })).collect();
    let pets: Vec<Value> = (0..outside::MAX_PETS)
        .map(|_| {
            json!({ "species": species, "name": name, "coat": outside::MAX_COLOR,
                    "collar": outside::MAX_COLOR, "rules": rules })
        })
        .collect();
    json!({ "parts": parts, "items": items, "pets": pets })
}

// E1, E17, E19: however much is asked for, a stored document is a few kilobytes, so a map of every
// place on The Commons stays a size a page can take.
#[tokio::test]
async fn the_largest_document_the_checks_allow_is_a_few_kilobytes() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let spot = server.house(&ann);
    let largest = largest_document();
    let saved = server.save(&ann, spot, largest.clone()).await.ok();
    assert_eq!(saved["outside"], largest);
    let bytes = serde_json::to_string(&saved["outside"]).unwrap().len();
    assert!(bytes > 4_000, "{bytes}: the test isn't building the largest one any more");
    assert!(bytes <= 8_192, "{bytes} bytes in one document");
    let stored = server.live();
    assert_eq!(stored.len(), 1);
    assert!(stored[0].2.len() <= 8_192);
    // On a map of The Commons' 1,500 places that is the most the answer can come to.
    assert!(1_500 * bytes < 16 * 1024 * 1024);
}

// E8: a refused save counts, a stranger's turned-away one doesn't, and each person has their own.
#[tokio::test]
async fn refused_saves_count_against_a_person_and_a_strangers_do_not() {
    let server = Server::start().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let spot = server.house(&ann);
    let bo_spot = server.house(&bo);
    let minute = now() / 60;
    // Bo knocking on Ann's place 70 times is turned away each time and uses none of his own.
    for _ in 0..70 {
        server.save(&bo, spot, walls(1)).await.refused(StatusCode::FORBIDDEN);
    }
    server.save(&bo, bo_spot, walls(2)).await.ok();
    // Ann sends 60 bad ones, and the next good one is past the limit.
    for _ in 0..60 {
        server.save(&ann, spot, json!({ "nope": 1 })).await.refused(StatusCode::BAD_REQUEST);
    }
    let after = server.save(&ann, spot, walls(3)).await;
    if now() / 60 == minute {
        after.refused(StatusCode::TOO_MANY_REQUESTS);
        // And so is clearing one, which is the same kind of change.
        server.delete(&ann.key, &Server::url(spot)).await.refused(StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(server.outside_of(&ann, spot).await, json!({}));
    }
    // Bo's own allowance is untouched by all of that.
    server.save(&bo, bo_spot, walls(4)).await.ok();
}

// E22, E27, W8: a map that is for anyone shows a pet's rules to anyone, a bot included. Every word in
// it is still one from the catalog.
#[tokio::test]
async fn the_map_of_outsides_is_open_to_anyone_and_is_only_listed_words_and_numbers() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let scout = server.bot(&ann, &room, "Scout").await;
    let spot = server.house(&ann);
    let largest = largest_document();
    server.save(&ann, spot, largest).await.ok();

    let mut words: HashSet<String> = HashSet::new();
    for part in outside::PARTS {
        words.insert(part.part.to_string());
        words.extend(part.styles.iter().map(|(word, _)| word.to_string()));
    }
    words.extend(outside::YARD.iter().map(|kind| kind.kind.to_string()));
    words.extend(outside::SPECIES.iter().map(|(word, _)| word.to_string()));
    words.extend(outside::PET_NAMES.iter().map(|word| word.to_string()));
    words.extend(outside::WHEN.iter().map(|(word, _)| word.to_string()));
    words.extend(outside::DO.iter().map(|(word, _)| word.to_string()));
    for key in
        ["parts", "body", "items", "pets", "style", "color", "number", "on", "kind", "x", "y"]
    {
        words.insert(key.to_string());
    }
    for key in ["facing", "species", "name", "coat", "collar", "rules", "when", "do"] {
        words.insert(key.to_string());
    }

    fn strings(value: &Value, into: &mut Vec<String>) {
        match value {
            Value::String(text) => into.push(text.clone()),
            Value::Array(list) => list.iter().for_each(|item| strings(item, into)),
            Value::Object(map) => map.iter().for_each(|(key, item)| {
                into.push(key.clone());
                strings(item, into);
            }),
            _ => {}
        }
    }
    let path = format!("/api/worlds/{COMMONS}/outsides");
    let guest = server.guest().await;
    for token in ["", scout.as_str(), guest.as_str(), ann.key.as_str()] {
        let reply = server.get(token, &path).await.ok();
        let mut seen = Vec::new();
        strings(&reply, &mut seen);
        for word in &seen {
            assert!(words.contains(word) || word == "outsides", "{word} isn't from the catalog");
        }
        assert!(reply.to_string().contains("\"rules\""), "the rules are there for anyone to read");
    }
}

// E4, E30: a database that an older release has been through. It knows nothing of outsides, so it
// takes places down and puts them up as it always did, and the newer one shows only what is true.
#[tokio::test]
async fn a_database_an_older_release_has_changed_shows_only_what_is_still_true() {
    let path = std::env::temp_dir().join(format!("outside-old-{}.db", std::process::id()));
    let path_text = path.to_string_lossy().to_string();
    let server = Server::start_on(&path_text).await;
    let ann = server.person("Ann").await;
    let cy = server.person("Cy").await;
    let spot = server.house(&ann);
    server.save(&ann, spot, walls(61)).await.ok();
    let ann_player = server.player_of(&ann);
    let cy_player = server.player_of(&cy);
    let with_it = server.stamp();
    assert_eq!(with_it.0, 1);

    let older = rusqlite::Connection::open(&path).unwrap();
    let take_down = |older: &rusqlite::Connection| {
        older
            .execute(
                "DELETE FROM world_buildings WHERE world_id = ?1 AND x = ?2 AND y = ?3",
                rusqlite::params![COMMONS, spot.0, spot.1],
            )
            .unwrap();
    };
    let put_up = |older: &rusqlite::Connection, player: i64, at: (i64, i64)| {
        older
            .execute(
                "INSERT INTO world_buildings (world_id, x, y, kind, player_id, built_at)
                 VALUES (?1, ?2, ?3, 'house', ?4, 0)",
                rusqlite::params![COMMONS, at.0, at.1, player],
            )
            .unwrap();
    };
    // It takes Ann's house down, which leaves her row, and Cy puts a house on the tile.
    take_down(&older);
    assert_eq!(server.stamp(), (0, 0));
    assert!(server.live().is_empty());
    put_up(&older, cy_player, spot);
    assert_eq!(server.outside_of(&cy, spot).await, json!({}));
    assert!(server.live().is_empty());
    assert_eq!(server.stamp(), (0, 0));
    // And Cy takes it down and Ann puts hers back on the same tile: it is hers again, and the count says so.
    take_down(&older);
    put_up(&older, ann_player, spot);
    assert_eq!(server.outside_of(&ann, spot).await, walls(61));
    assert_eq!(server.stamp(), with_it);

    // Ann's house goes down and up somewhere else without the newer release noticing: nothing shows
    // there, because a row for a tile proves nothing alone.
    take_down(&older);
    let elsewhere = server.free_spots().into_iter().find(|&at| at != spot).unwrap();
    put_up(&older, ann_player, elsewhere);
    assert_eq!(server.outside_of(&ann, elsewhere).await, json!({}));
    assert!(server.live().is_empty());
    // The newer release does notice the next time she takes one down and puts one up.
    server.take_down(&ann, elsewhere, now() + DAY);
    let third = server.free_spots().into_iter().find(|&at| at != spot && at != elsewhere).unwrap();
    server.put_up(&ann, "house", third, now() + 2 * DAY);
    assert_eq!(server.outside_of(&ann, third).await, walls(61));
    drop(older);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path_text}{suffix}"));
    }
}

// E30: at most a few outsides wait for each kind of place, and what stands is never what goes.
#[tokio::test]
async fn the_cap_on_waiting_outsides_never_takes_one_that_shows_nor_another_kinds() {
    let path = std::env::temp_dir().join(format!("outside-cap-{}.db", std::process::id()));
    let path_text = path.to_string_lossy().to_string();
    let server = Server::start_on(&path_text).await;
    let ann = server.person("Ann").await;
    let player = server.player_of(&ann);
    let house = server.house(&ann);
    let cafe = server.place_later(&ann, "cafe", 8);
    server.save(&ann, house, walls(1)).await.ok();
    server.save(&ann, cafe, walls(2)).await.ok();
    // Six houses waiting, saved one after another, and six cafes put in beside them without the trim.
    let beside = rusqlite::Connection::open(&path).unwrap();
    for n in 0..6 {
        let text = walls(10 + n).to_string();
        server
            .state
            .db
            .save_outside(COMMONS, player, "house", (n, 100), &text, now() + 100 + n)
            .unwrap();
        beside
            .execute(
                "INSERT INTO place_outsides (world_id, player_id, kind, x, y, outside, stamp, changed_at)
                 VALUES (?1, ?2, 'cafe', ?3, 101, ?4, ?5, ?6)",
                rusqlite::params![COMMONS, player, n, text, 1_000 + n, now() + 100 + n],
            )
            .unwrap();
    }
    drop(beside);
    let rows = |kind: &str| -> Vec<(i64, i64)> {
        let reader = rusqlite::Connection::open(&path).unwrap();
        let mut statement = reader
            .prepare("SELECT x, y FROM place_outsides WHERE kind = ?1 ORDER BY y, x")
            .unwrap();
        statement
            .query_map([kind], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    // The houses that wait are the newest four whatever is saved, the one that shows is never one of them,
    // and saving a house leaves the cafes alone.
    server.save(&ann, house, walls(3)).await.ok();
    let mut expected: Vec<(i64, i64)> = (2..6).map(|n| (n, 100)).collect();
    expected.push(house);
    expected.sort_by_key(|&(x, y)| (y, x));
    assert_eq!(rows("house"), expected);
    assert_eq!(rows("cafe").len(), 7);
    assert_eq!(server.outside_of(&ann, house).await, walls(3));
    assert_eq!(server.outside_of(&ann, cafe).await, walls(2));
    // Saving the cafe does the same for cafes.
    server.save(&ann, cafe, walls(4)).await.ok();
    assert_eq!(rows("cafe").len(), 5);
    assert_eq!(server.outside_of(&ann, house).await, walls(3));
    assert_eq!(server.live().len(), 2);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path_text}{suffix}"));
    }
}

/// The two numbers in a view, and what is stored for the map.
type Stamp = (i64, i64);
type Stored = Vec<(i64, i64, String)>;

// E26, E27: through every way an outside can change or a place can come and go, the two numbers differ
// whenever what is on the map does, and the map of outsides is always what is stored.
#[tokio::test]
async fn the_stamp_tells_apart_every_state_the_map_has_been_in() {
    let (server, keeper) = Server::start_with_keeper().await;
    let ann = server.person("Ann").await;
    let bo = server.person("Bo").await;
    let cy = server.person("Cy").await;
    let dan = server.person("Dan").await;
    let mut taken: Vec<(i64, i64)> = Vec::new();
    let mut pick = |server: &Server| {
        let spot = server.free_spots().into_iter().find(|spot| !taken.contains(spot)).unwrap();
        taken.push(spot);
        spot
    };
    let (a, b, c) = (pick(&server), pick(&server), pick(&server));
    let mut day = 1;
    let mut when = || {
        day += 1;
        now() + day * DAY
    };
    server.put_up(&ann, "house", a, now());
    server.put_up(&bo, "house", b, now());
    server.put_up(&cy, "house", c, now());

    let mut history: Vec<(Stamp, Stored, &str)> = Vec::new();
    macro_rules! step {
        ($what:expr) => {{
            let listed = server.on_the_map().await;
            let stored: Vec<Value> = server
                .live()
                .into_iter()
                .map(|(x, y, text)| json!([x, y, serde_json::from_str::<Value>(&text).unwrap()]))
                .collect();
            assert_eq!(listed, stored, "the map isn't what is stored after {}", $what);
            history.push((server.stamp(), server.live(), $what));
        }};
    }
    step!("nothing");
    server.save(&ann, a, walls(1)).await.ok();
    step!("ann saves");
    server.save(&bo, b, walls(2)).await.ok();
    step!("bo saves");
    server.save(&cy, c, walls(3)).await.ok();
    step!("cy saves");
    server.save(&ann, a, walls(4)).await.ok();
    step!("ann edits");
    server.take_down(&ann, a, when());
    step!("ann's house comes down");
    server.put_up(&ann, "house", a, when());
    step!("it goes up on the same tile");
    server.take_down(&ann, a, when());
    let d = pick(&server);
    server.put_up(&ann, "house", d, when());
    step!("it goes up elsewhere");
    server.delete(&keeper.key, &Server::url(b)).await.ok();
    step!("the keeper clears bo's");
    server.save(&bo, b, walls(5)).await.ok();
    step!("bo saves again");
    server.take_down(&bo, b, when());
    server.put_up(&dan, "house", b, when());
    step!("dan builds where bo's was");
    server.save(&dan, b, walls(6)).await.ok();
    step!("dan saves there");
    let e = pick(&server);
    server.put_up(&bo, "house", e, when());
    step!("bo builds elsewhere and wears his");
    server.take_down(&dan, b, when());
    let f = pick(&server);
    server.put_up(&dan, "house", f, when());
    step!("dan moves and wears his");
    server.save(&ann, d, walls(1)).await.ok();
    step!("ann edits at her new house");
    server.run_a_disaster();
    step!("a disaster");
    let g = pick(&server);
    if server.state.db.house_of_person(COMMONS, &ann.id).unwrap().is_none() {
        server.put_up(&ann, "house", g, now() + 400 * DAY);
        step!("ann builds after it and wears hers");
    }

    // A change in what is on the map always shows in the two numbers, whenever the two states were.
    for (i, (stamp_i, content_i, what_i)) in history.iter().enumerate() {
        for (stamp_j, content_j, what_j) in history.iter().skip(i + 1) {
            if content_i != content_j {
                assert_ne!(stamp_i, stamp_j, "{what_i} and {what_j} look the same");
            }
        }
    }
    // And the states really were different from one another, so this checked something.
    let kinds: HashSet<&Stored> = history.iter().map(|(_, c, _)| c).collect();
    assert!(kinds.len() >= 8, "{} different states", kinds.len());
}

// W59, E22, B1: a bot is told nothing of an outside by anything it can read: not the map it is shown, not
// `look`, not `how_am_i_doing`, not the places it can list.
#[tokio::test]
async fn nothing_a_bot_reads_has_an_outside_a_pet_or_a_stamp_in_it() {
    let server = Server::start().await;
    let (room, ann) = server.someone("Ann").await;
    let spot = server.house(&ann);
    let scout = server.bot(&ann, &room, "Scout").await;
    let marked = json!({
        "parts": { "walls": { "style": "brick", "color": 12_345_678 } },
        "pets": [{ "species": "fox", "name": "pepper", "coat": 7_654_321,
                   "rules": [{ "when": "owner_watching", "do": "hide" }] }],
    });
    server.save(&ann, spot, marked).await.ok();
    let view = server.view().await;
    assert!(view["outsides"].is_array(), "the view has the numbers");
    let stamp = server.stamp().1.to_string();

    let guest = server.guest().await;
    let rpc = |name: &str, arguments: Value| {
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": name, "arguments": arguments } })
    };
    let mut read = Vec::new();
    for (name, arguments) in [
        ("look", json!({})),
        ("look", json!({ "player_token": guest, "x": spot.0, "y": spot.1, "w": 9, "h": 9 })),
        ("how_am_i_doing", json!({ "player_token": guest })),
    ] {
        read.push(server.post("", "/public/mcp", rpc(name, arguments)).await.body.to_string());
    }
    for path in [
        format!("/api/worlds/{COMMONS}/look?x={}&y={}&w=9&h=9", spot.0, spot.1),
        format!("/api/worlds/{COMMONS}/me"),
    ] {
        read.push(server.get(&guest, &path).await.body.to_string());
    }
    read.push(worlds::commons_for_bot(&server.state, &ann.id).unwrap().to_string());
    for tool in ["list_city", "inbox"] {
        let called = server.post(&scout, "/mcp", rpc(tool, json!({}))).await;
        read.push(called.body.to_string());
    }
    for shown in read {
        for word in [
            "outsides",
            "\"outside\"",
            "\\\"outside\\\"",
            "pepper",
            "12345678",
            "7654321",
            "owner_watching",
            &stamp,
        ] {
            assert!(!shown.contains(word), "{word} in {}", &shown[..shown.len().min(300)]);
        }
    }
}

// ----------------------------------------------------------------------------------------------
// The pages, read as source: what E4, E21, E28 and E29 can honestly be tested by here
// ----------------------------------------------------------------------------------------------

/// A page's source with its comments taken out, so a rule can name a call without tripping a test.
fn code_of(name: &str) -> String {
    let mut out = String::new();
    let mut in_block = false;
    for line in web(name).lines() {
        let mut rest = line;
        let mut kept = String::new();
        while !rest.is_empty() {
            if in_block {
                match rest.find("*/") {
                    Some(end) => {
                        rest = &rest[end + 2..];
                        in_block = false;
                    }
                    None => rest = "",
                }
            } else if let Some(start) = rest.find("//") {
                let (before, after) = rest.split_at(start);
                if let Some(block) = before.find("/*") {
                    kept.push_str(&before[..block]);
                    rest = &rest[block + 2..];
                    in_block = true;
                } else if before.matches(['\'', '"', '`']).count() % 2 == 1 {
                    // The two slashes are inside a string, such as a web address.
                    kept.push_str(&rest[..start + 2]);
                    rest = &after[2..];
                } else {
                    kept.push_str(before);
                    rest = "";
                }
            } else if let Some(block) = rest.find("/*") {
                kept.push_str(&rest[..block]);
                rest = &rest[block + 2..];
                in_block = true;
            } else {
                kept.push_str(rest);
                rest = "";
            }
        }
        out.push_str(&kept);
        out.push('\n');
    }
    out
}

/// Everything that draws an outside or runs a pet. Only `outside.js`, the builder, asks the server.
const DRAWING: [&str; 6] = [
    "pets.js",
    "outside-spec.js",
    "outside-art.js",
    "outside-building.js",
    "outside-yard.js",
    "outside-decor.js",
];

// E21: no move, nothing stored, nothing posted, nobody woken and no sound.
#[test]
fn nothing_that_draws_an_outside_or_runs_a_pet_talks_to_anyone_or_makes_a_sound() {
    for name in DRAWING {
        let code = code_of(name);
        for banned in [
            "fetch(",
            "api(",
            "WebSocket",
            "EventSource",
            "XMLHttpRequest",
            "sendBeacon",
            "postMessage",
            "localStorage",
            "sessionStorage",
            "indexedDB",
            "document.cookie",
            "navigator",
            "window.location",
            "Notification",
            "AudioContext",
            "new Audio",
            "speechSynthesis",
            "vibrate",
            "eval(",
            "new Function",
            "import(",
        ] {
            assert!(!code.contains(banned), "web/{name} has {banned}");
        }
        // The only thing in a document or a pet that reaches the page is a canvas to paint on.
        for (at, _) in code.match_indices("document.") {
            assert!(code[at..].starts_with("document.createElement('canvas')"), "web/{name}");
        }
        // And nothing reaches the markup or a link.
        for banned in ["innerHTML", "outerHTML", "insertAdjacentHTML", "href", "srcdoc"] {
            assert!(!code.contains(banned), "web/{name} has {banned}");
        }
    }
    // The builder is the one page that asks, and only for these.
    let builder = code_of("outside.js");
    assert!(builder.contains("api("));
    for path in ["/api/outside", "/api/places/"] {
        assert!(builder.contains(path), "{path}");
    }
    assert!(!builder.contains("WebSocket") && !builder.contains("sendBeacon"));
}

// W6, E21: nothing on the map is words, and nothing in a pet is drawn as text, a name included.
#[test]
fn nothing_that_draws_an_outside_or_a_pet_draws_text() {
    for name in DRAWING {
        let code = code_of(name);
        for banned in [
            "fillText",
            "strokeText",
            "measureText",
            "strokeRect",
            "roundRect",
            "setLineDash",
            "createConicGradient",
            "shadowBlur",
            ".filter =",
            ".font =",
            "textAlign",
        ] {
            assert!(!code.contains(banned), "web/{name} has {banned}");
        }
    }
    // And it reads nothing but what it is given: no clock, no page, no random number.
    let pets = code_of("pets.js");
    for banned in ["Date.now", "new Date", "performance.", "Math.random", "window", "localStorage"]
    {
        assert!(!pets.contains(banned), "web/pets.js has {banned}");
    }
}

// E4, E19, E20: a word that is on every object, such as `constructor`, is still a word the page doesn't
// know. A pet's pace came out as not a number, and then so did its place, until a table was read only for
// the words it lists.
#[test]
fn the_pets_page_looks_words_up_only_in_the_tables_that_list_them() {
    let code = code_of("pets.js");
    for banned in [
        "PACE[p.species]",
        "HOPPERS[p.species]",
        "USUAL[species] ||",
        "ACTION[rule.do]",
        "ACTS[rule.do]",
        "ARTS[rule.",
    ] {
        assert!(!code.contains(banned), "web/pets.js reads a table with {banned}");
    }
    // Every table that a document's word can reach is asked with `hasOwn` first.
    for table in ["PACE", "HOPPERS", "USUAL", "ACTION", "ARTS"] {
        assert!(
            code.contains(&format!("Object.hasOwn({table},")),
            "web/pets.js never asks whether {table} has the word"
        );
    }
    // A rule's `do` must be text, so an array that reads like a word is not one.
    assert!(code.contains("typeof rule.do === 'string'"));
    // A `when` that isn't listed never holds, and the first that holds wins.
    let holds = &code[code.find("export function holds").unwrap()..];
    let holds = &holds[..holds.find("export function chooseRule").unwrap()];
    assert!(holds.contains("default:\n      return false;"));
    // The builder reads items with the same care.
    let spec = code_of("outside-spec.js");
    assert!(spec.contains("Object.hasOwn(YARD_SIZES, item.kind)"));
    assert!(spec.contains("known.includes(style)"));
}

// E19, E20: the words a rule can ask are all that the page's `holds` answers, and none of them is
// answered from anything but the facts it is given.
#[test]
fn every_when_is_answered_from_the_facts_alone() {
    let code = code_of("pets.js");
    let holds = &code[code.find("export function holds").unwrap()..];
    let holds = &holds[..holds.find("export function chooseRule").unwrap()];
    for (word, _) in outside::WHEN {
        assert!(holds.contains(&format!("case '{word}':")), "holds() has no case for {word}");
    }
    let cases = holds.matches("case '").count();
    assert_eq!(cases, outside::WHEN.len(), "holds() answers a word the catalog doesn't list");
    // Nothing in it reaches for anything but `f`, `HOURS`, `pulse` and the numbers it works out.
    for banned in ["this.", "world", "players", "people", "room", "inside", "home", "visitors"] {
        assert!(!holds.contains(banned), "holds() mentions {banned}");
    }
}

// E28: a room and an outside don't touch. Nothing about a room reads an outside and nothing about an
// outside reads a room, so this adds nothing to W1 and W60.
#[test]
fn nothing_in_a_room_reads_an_outside_and_nothing_in_an_outside_reads_a_room() {
    let root = env!("CARGO_MANIFEST_DIR");
    let read = |path: &str| {
        std::fs::read_to_string(format!("{root}/{path}"))
            .unwrap_or_else(|error| panic!("{path}: {error}"))
    };
    for path in
        ["src/spaces.rs", "src/items.rs", "src/ops.rs", "src/city.rs", "web/room.js", "web/city.js"]
    {
        let source = read(path);
        for word in
            ["place_outsides", "outside::", "outside.js", "outside-spec", "/outside", "pets.js"]
        {
            assert!(!source.contains(word), "{path} has {word}");
        }
    }
    let ours = read("src/outside.rs");
    for word in [
        "spaces::",
        "items::",
        "floor_items",
        "town_square_rooms",
        "space_plots",
        "hangout",
        "room_agents",
        "bot_places",
    ] {
        assert!(!ours.contains(word), "src/outside.rs has {word}");
    }
    // The storage calls for outsides don't read a room's tables either.
    let db = read("src/db.rs");
    let from = db.find("pub fn place_standing").unwrap();
    let to = db.find("/// Everything a player has built in a world.").unwrap();
    for word in ["floor_items", "town_square_rooms", "space_plots", "spaces", "hangouts", "floors"]
    {
        assert!(!db[from..to].contains(word), "the outside calls in src/db.rs have {word}");
    }
    for name in DRAWING {
        let code = code_of(name);
        for word in ["/api/spaces", "floor_items", "hangout", "spaces.js"] {
            assert!(!code.contains(word), "web/{name} has {word}");
        }
    }
}

// E29: the map draws an outside in the tile's own picture and a pet over the map, as it does anyone on
// the street.
#[test]
fn the_map_paints_an_outside_into_the_tile_and_draws_a_pet_like_anyone_on_the_street() {
    let map = code_of("world.js");
    assert!(map.contains("from './outside-art.js'"));
    assert!(map.contains("paintOutsideGround") && map.contains("paintOutsideStanding"));
    assert!(map.contains("from './pets.js'"));
    // The outside is part of what a building's picture is made from, and is part of its key.
    assert!(map.contains("outside.key"));
    // A pet is sorted and drawn with the figures, and not stored in the picture of a tile.
    assert!(map.contains("drawPet(bg, e, t, this.calm)"));
    // Reduced motion: it neither moves nor is stepped.
    let step = &map[map.find("updatePets(dt) {").unwrap()..];
    let step = &step[..step.find("nearby(x, y)").unwrap()];
    assert!(step.contains("this.calm"));
    let pets = code_of("pets.js");
    assert!(pets.contains("calm ? 'sit' : p.pose"), "a calm pet doesn't sit still");
    // Zoomed far out, there are none to see.
    assert!(map.contains("scale >= PET_SCALE"), "pets are drawn at every zoom");
}

// E34: the page reads the packs and says what pressing one does.
#[test]
fn the_builder_offers_packs_and_puts_them_up_and_takes_them_down() {
    let page = web("outside.js").replace("\r\n", "\n");
    assert!(page.contains("catalog.packs"));
    assert!(page.contains("catalogGroup('Packs'"));
    for words in ["Press to put it up", "Up now. Press to take it down", "didn't fit"] {
        assert!(page.contains(words), "{words}");
    }
    // Up is any of the pack's kinds in the yard, with nothing recorded.
    assert!(page.contains("const isUp = (pack, doc = shown) => (doc.items || []).some("));
    // Putting up sets only the parts this kind of place has and adds items where they fit.
    let body_of = |name: &str| {
        let start = format!("function {name}(");
        page.split(start.as_str()).nth(1).unwrap().split("\n  }\n").next().unwrap().to_string()
    };
    let up = body_of("putUp");
    assert!(up.contains("part.kinds.includes(kind)"));
    assert!(up.contains("nearestSpot(doc, item.kind, item)"));
    assert!(!up.contains("pets"));
    // Taking down removes the pack's kinds and a part only while it has the pack's style, and
    // touches no pet.
    let down = body_of("takeDown");
    assert!(down.contains("gone.has(item.kind)"));
    assert!(down.contains("=== chosen.style"));
    assert!(!down.contains("pets"));
}

// Each rule in docs/outside.md has a test above that names it.
#[test]
fn every_rule_in_the_outside_doc_is_named_by_a_test() {
    let root = env!("CARGO_MANIFEST_DIR");
    let doc = std::fs::read_to_string(format!("{root}/docs/outside.md")).unwrap();
    let rules: Vec<u32> = doc
        .lines()
        .filter_map(|line| {
            line.trim_start().strip_prefix("- **E")?.split_once(".**")?.0.parse().ok()
        })
        .collect();
    // Rules are written beside what they're about, so this only wants every number once.
    let mut sorted = rules.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, (1..=36).collect::<Vec<u32>>(), "the rules should be E1 to E36");

    let tests = std::fs::read_to_string(format!("{root}/tests/outside.rs")).unwrap();
    let mut named: HashSet<u32> = HashSet::new();
    for line in tests.lines().map(str::trim_start).filter(|line| line.starts_with("// ")) {
        let mut before: Option<u32> = None;
        let mut range = false;
        for word in line.split(|c: char| !c.is_ascii_alphanumeric()).filter(|word| !word.is_empty())
        {
            if let Some(number) =
                word.strip_prefix('E').and_then(|digits| digits.parse::<u32>().ok())
            {
                match (range, before) {
                    (true, Some(from)) => named.extend(from..=number),
                    _ => {
                        named.insert(number);
                    }
                }
                before = Some(number);
                range = false;
            } else {
                range = word == "to" && before.is_some();
                if !range {
                    before = None;
                }
            }
        }
    }
    let missing: Vec<u32> = rules.into_iter().filter(|rule| !named.contains(rule)).collect();
    assert!(missing.is_empty(), "no test names {missing:?}");
}

// E13, E16: a bigger building goes where it fits round the yard, nearest where it stood, so choosing
// a size never sends a body the server would refuse and snap back to the old one. With nowhere it
// fits, the builder says so and sends nothing.
#[test]
fn a_new_size_goes_where_it_fits_round_the_yard() {
    let page = web("outside.js").replace("\r\n", "\n");
    let start = page.find("const resized = (doc, w, d) => {").expect("the resize");
    let resized = &page[start..start + page[start..].find("\n    };\n").unwrap()];
    assert!(resized.contains("bodyFits(doc, "), "it looks for room: {resized}");
    assert!(resized.contains("return null"), "and says when there's none: {resized}");
    assert!(page.contains("isn't room for a"), "the builder tells you");
}
