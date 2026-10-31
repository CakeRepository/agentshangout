//! Tests for whose each place on The Commons is, and how your bots can get into it. They drive a real
//! server over HTTP and are written from W71 and W72 in docs/public-city.md. A comment such as
//! `// W72` above a test names the rule it covers.

use std::collections::HashMap;

use agenthangout::{
    app,
    db::Db,
    now,
    world::{self, Kind},
    worlds::{self, Move},
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
}

struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    #[track_caller]
    fn ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{} {}", self.status, self.body);
        self.body
    }
}

struct Who {
    key: String,
    id: String,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
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

    async fn someone(&self, name: &str) -> (String, Who) {
        let made = self
            .post("", "/api/hangouts", json!({ "name": format!("{name}'s room"), "your_name": name, "create_code": CODE }))
            .await
            .ok();
        (text(&made["hangout"]["id"]), Who { key: text(&made["owner_key"]), id: text(&made["person"]["id"]) })
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> (String, String) {
        let made = self.post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name })).await.ok();
        (text(&made["agent"]["id"]), text(&made["token"]))
    }

    /// Tiles a place can really go on: open land beside a road, not kept for a side street.
    fn free_spot(&self) -> (i64, i64) {
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
            .find(|&(x, y)| {
                let pos = (x as u16, y as u16);
                !tiles.contains_key(&pos) && !world::is_reserved(&tiles, pos) && land.at(pos).is_some_and(|ground| ground.is_open())
            })
            .unwrap()
    }

    /// A place put up at a time of the test's choosing, through its person's own player.
    fn place_at(&self, who: &Who, kind: &str, at: i64) -> (i64, i64) {
        let player = self.state.db.own_player(&who.id, at).unwrap();
        let (x, y) = self.free_spot();
        let mv = Move { action: "place".into(), x: Some(x), y: Some(y), x2: None, y2: None, kind: Some(kind.into()), emote: None };
        worlds::act(&self.state, &player, COMMONS, &mv, at).unwrap();
        (x, y)
    }

    /// `[x, y] -> name` from The Commons' view, after the pause that lets a change show (W29).
    async fn owners(&self) -> HashMap<(i64, i64), String> {
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let view = self.get("", &format!("/api/worlds/{COMMONS}")).await.ok();
        view["owners"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| ((row[0].as_i64().unwrap(), row[1].as_i64().unwrap()), text(&row[2])))
            .collect()
    }

    async fn place(&self, token: &str, (x, y): (i64, i64)) -> Reply {
        self.get(token, &format!("/api/places/{x}/{y}")).await
    }
}

fn access(place: &Value, bot: &str) -> String {
    let bots = place["bots"].as_array().unwrap();
    text(&bots.iter().find(|row| row["name"] == bot).unwrap_or_else(|| panic!("{bot}: {place}"))["access"])
}

// W71
#[tokio::test]
async fn every_place_on_the_commons_names_its_owner() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    let (_, bo) = server.someone("Bo").await;
    let ann_house = server.place_at(&ann, "house", now());
    let bo_house = server.place_at(&bo, "house", now());
    let owners = server.owners().await;
    assert_eq!(owners.get(&ann_house).map(String::as_str), Some("Ann"));
    assert_eq!(owners.get(&bo_house).map(String::as_str), Some("Bo"));
    assert_eq!(owners.len(), 2, "only places: {owners:?}");
}

// W72
#[tokio::test]
async fn hovering_a_place_tells_you_how_each_of_your_bots_can_get_in() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    let (bo_room, bo) = server.someone("Bo").await;
    let (rover_id, rover) = server.bot(&bo, &bo_room, "Rover").await;
    let ann_house = server.place_at(&ann, "house", now() - 8 * DAY);

    // Before Ann has been through her door it has no rooms, so an invite is the only way in.
    let place = server.place(&bo.key, ann_house).await.ok();
    assert_eq!(place["kind"], "house");
    assert_eq!(place["owner"], "Ann");
    assert_eq!(place["yours"], false);
    assert_eq!(access(&place, "Rover"), "needs_invite");
    assert_eq!(server.place(&ann.key, ann_house).await.ok()["yours"], true);

    // Ann's cafe is open to anyone, so Rover can walk in, and once it has, it's visiting.
    let cafe = server.place_at(&ann, "cafe", now());
    let door = server.post(&ann.key, "/api/spaces/door", json!({ "x": cafe.0, "y": cafe.1 })).await.ok();
    let floor = text(&door["hangout_id"]);
    // The door opened onto Ann's own room, which stays as she left it (S47), so she opens it.
    server.send(&ann.key, Method::PUT, &format!("/api/floors/{floor}"), Some(json!({ "access": "public", "create_code": CODE }))).await.ok();
    let place = server.place(&bo.key, cafe).await.ok();
    assert_eq!(place["kind"], "cafe");
    assert_eq!(access(&place, "Rover"), "can_walk_in");
    server.post(&bo.key, &format!("/api/floors/{floor}/walk-in"), json!({ "agent_id": rover_id })).await.ok();
    assert_eq!(access(&server.place(&bo.key, cafe).await.ok(), "Rover"), "visiting");

    // A bot in one of a place's rooms is in it: Ann makes her house's room and brings in her own bot.
    let house_door = server.post(&ann.key, "/api/spaces/door", json!({ "x": ann_house.0, "y": ann_house.1 })).await.ok();
    let house_room = text(&house_door["hangout_id"]);
    server.bot(&ann, &house_room, "Fern").await;
    assert_eq!(access(&server.place(&ann.key, ann_house).await.ok(), "Fern"), "in");
    assert_eq!(access(&server.place(&bo.key, ann_house).await.ok(), "Rover"), "needs_invite");

    // Nothing there is a 404, and a bot or a guest has nobody to ask about.
    let empty = server.free_spot();
    assert_eq!(server.place(&bo.key, empty).await.status, StatusCode::NOT_FOUND);
    assert!(server.place(&rover, ann_house).await.status.is_client_error());
    let guest = text(&server.post("", "/api/public/join", json!({})).await.ok()["player_token"]);
    assert_eq!(server.place(&guest, ann_house).await.status, StatusCode::UNAUTHORIZED);
}
