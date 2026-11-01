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
                god_ids: Vec::new(),
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

    /// `[x, y] -> owner` from The Commons' view, after the pause that lets a change show (W29).
    async fn owners(&self) -> HashMap<(i64, i64), String> {
        self.owner_rows().await.into_iter().map(|(at, owner, _)| (at, owner)).collect()
    }

    /// `[x, y] -> name` (W73).
    async fn names(&self) -> HashMap<(i64, i64), String> {
        self.owner_rows().await.into_iter().map(|(at, _, name)| (at, name)).collect()
    }

    async fn owner_rows(&self) -> Vec<((i64, i64), String, String)> {
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let view = self.get("", &format!("/api/worlds/{COMMONS}")).await.ok();
        view["owners"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| ((row[0].as_i64().unwrap(), row[1].as_i64().unwrap()), text(&row[2]), text(&row[3])))
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
    assert_eq!(place["open"], true);
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

// W73
#[tokio::test]
async fn every_place_has_a_name_its_owner_or_its_project_gave_it() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    let house = server.place_at(&ann, "house", now());
    let studio = server.place_at(&ann, "studio", now() + 2 * DAY);
    // Before her first time through the door, a place is called after her and its kind.
    let names = server.names().await;
    assert_eq!(names.get(&studio).map(String::as_str), Some("Ann's Studio"));

    // Going in gives the house its room, named after her, and she can rename it.
    let door = server.post(&ann.key, "/api/spaces/door", json!({ "x": house.0, "y": house.1 })).await.ok();
    assert_eq!(server.names().await.get(&house).map(String::as_str), Some("Ann's House"));
    let space = text(&door["space_id"]);
    server.send(&ann.key, Method::PUT, &format!("/api/spaces/{space}"), Some(json!({ "name": "Sunny Side" }))).await.ok();
    assert_eq!(server.names().await.get(&house).map(String::as_str), Some("Sunny Side"));

    // Someone else going through a door isn't told which space it is.
    let (_, bo) = server.someone("Bo").await;
    server.place_at(&bo, "house", now());
    let cafe = server.place_at(&bo, "cafe", now() + 8 * DAY);
    let mine = server.post(&bo.key, "/api/spaces/door", json!({ "x": cafe.0, "y": cafe.1 })).await.ok();
    let floor = text(&mine["hangout_id"]);
    server.send(&bo.key, Method::PUT, &format!("/api/floors/{floor}"), Some(json!({ "access": "public", "create_code": CODE }))).await.ok();
    let theirs = server.post(&ann.key, "/api/spaces/door", json!({ "x": cafe.0, "y": cafe.1 })).await.ok();
    assert!(theirs["space_id"].is_null(), "{theirs}");
}

// S49, S50, S51, S52
#[tokio::test]
async fn a_person_asks_to_let_their_bot_into_someone_elses_place_and_the_owner_decides() {
    let server = Server::start().await;
    let (_, ann) = server.someone("Ann").await;
    let (bo_room, bo) = server.someone("Bo").await;
    let (rover_id, rover) = server.bot(&bo, &bo_room, "Rover").await;
    let (scout_id, _) = server.bot(&bo, &bo_room, "Scout").await;
    let house = server.place_at(&ann, "house", now());
    let ask = |agent_id: &str| json!({ "agent_id": agent_id });
    let path = format!("/api/places/{}/{}/requests", house.0, house.1);

    // A place with no rooms yet can't take anyone.
    let shut = server.post(&bo.key, &path, ask(&rover_id)).await;
    assert_eq!(shut.status, StatusCode::CONFLICT, "{}", shut.body);
    let door = server.post(&ann.key, "/api/spaces/door", json!({ "x": house.0, "y": house.1 })).await.ok();
    let room = text(&door["hangout_id"]);

    let asked = server.post(&bo.key, &path, ask(&rover_id)).await.ok();
    assert_eq!(asked["status"], "waiting");
    let again = server.post(&bo.key, &path, ask(&rover_id)).await.ok();
    assert_eq!(again["id"], asked["id"], "asking again changes nothing");
    assert_eq!(access(&server.place(&bo.key, house).await.ok(), "Rover"), "asked");
    assert_eq!(server.post(&rover, &path, ask(&rover_id)).await.status, StatusCode::FORBIDDEN);
    let (fern_id, _) = server.bot(&ann, &room, "Fern").await;
    assert_eq!(server.post(&ann.key, &path, ask(&fern_id)).await.status, StatusCode::CONFLICT, "her own place");

    // Ann sees it, and Bo can't answer it.
    let waiting = server.get(&ann.key, "/api/place-requests").await.ok();
    let request = waiting["requests"][0].clone();
    assert_eq!(request["bot"], "Rover");
    assert_eq!(request["asked_by"], "Bo");
    assert_eq!(request["kind"], "house");
    assert_eq!(server.get(&bo.key, "/api/place-requests").await.ok()["requests"], json!([]));
    let id = text(&request["id"]);
    let answer = format!("/api/place-requests/{id}/answer");
    assert_eq!(server.post(&bo.key, &answer, json!({ "response": "yes" })).await.status, StatusCode::NOT_FOUND);

    // A yes brings Rover into the house's first floor, and Bo can read it.
    server.post(&ann.key, &answer, json!({ "response": "yes" })).await.ok();
    assert_eq!(access(&server.place(&bo.key, house).await.ok(), "Rover"), "in");
    server.get(&bo.key, &format!("/api/hangouts/{room}")).await.ok();
    assert_eq!(server.post(&bo.key, &path, ask(&rover_id)).await.status, StatusCode::CONFLICT, "already in");

    // A no is final for a week.
    let scout = server.post(&bo.key, &path, ask(&scout_id)).await.ok();
    server.post(&ann.key, &format!("/api/place-requests/{}/answer", text(&scout["id"])), json!({ "response": "no" })).await.ok();
    assert_eq!(access(&server.place(&bo.key, house).await.ok(), "Scout"), "declined");
    assert_eq!(server.post(&bo.key, &path, ask(&scout_id)).await.status, StatusCode::CONFLICT);
    assert_eq!(server.get(&ann.key, "/api/place-requests").await.ok()["requests"], json!([]));
}
