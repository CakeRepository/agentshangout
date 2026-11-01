//! Tests for bots' routines: social hours, going out, wandering and the daily cap. They drive a real
//! server over HTTP and are written from the rules in docs/routines.md. A comment such as `// L6`
//! above a test names the rule it covers.

use agenthangout::{
    app,
    db::Db,
    doorbell::{Doorbell, Seen},
    now,
    routines::WANDER_SECS,
    AppState, Config,
};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};

const CODE: &str = "letmein";
const DAY: i64 = 86_400;

/// All-day hours end at midnight UTC, and a bot's walk-in ends with them (L8), so a test that needs a
/// bot to stay out starts early in the day, with plenty of it left.
async fn start_early_in(period: i64, room: i64) {
    let left = period - now().rem_euclid(period);
    if left < room {
        tokio::time::sleep(std::time::Duration::from_secs(left as u64 + 1)).await;
    }
}

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

    #[track_caller]
    fn is(&self, status: StatusCode) {
        assert_eq!(self.status, status, "{} {}", self.status, self.body);
    }
}

struct Who {
    key: String,
}

/// Ann has a public floor, Ann's Cafe, and her bot Fern is in it. Bo has a room of his own with his
/// bot Rover, which has never been out.
struct Town {
    ann: Who,
    bo: Who,
    cafe: String,
    fern: String,
    rover: String,
    rover_id: String,
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

    /// Someone new, by way of a room of their own: the room's id, and who they are.
    async fn someone(&self, name: &str) -> (String, Who) {
        let made = self
            .post(
                "",
                "/api/hangouts",
                json!({ "name": format!("{name}'s Cafe"), "your_name": name, "create_code": CODE }),
            )
            .await
            .ok();
        (text(&made["hangout"]["id"]), Who { key: text(&made["owner_key"]) })
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> (String, String) {
        let made = self
            .post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await
            .ok();
        (text(&made["agent"]["id"]), text(&made["token"]))
    }

    async fn town(&self) -> Town {
        let (cafe, ann) = self.someone("Ann").await;
        self.put(
            &ann.key,
            &format!("/api/floors/{cafe}"),
            json!({ "access": "public", "create_code": CODE }),
        )
        .await
        .ok();
        let (_, fern) = self.bot(&ann, &cafe, "Fern").await;
        let (bo_room, bo) = self.someone("Bo").await;
        let (rover_id, rover) = self.bot(&bo, &bo_room, "Rover").await;
        Town { ann, bo, cafe, fern, rover, rover_id }
    }

    async fn set_routine(&self, who: &str, agent_id: &str, body: Value) -> Reply {
        self.put(who, &format!("/api/agents/{agent_id}/routine"), body).await
    }

    /// Rover may go out all day, every day, with a cap of `cap`.
    async fn always_out(&self, town: &Town, cap: i64) {
        let body = json!({ "days": ["mon", "tue", "wed", "thu", "fri", "sat", "sun"], "starts": "00:00", "ends": "24:00", "max_social_runs": cap, "public_places": true });
        self.set_routine(&town.bo.key, &town.rover_id, body).await.ok();
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

    async fn refused(&self, token: &str, name: &str, arguments: Value) -> String {
        let result = self.call(token, name, arguments).await;
        assert_eq!(result["isError"], true, "{name} should have been refused: {result}");
        text(&result["content"][0]["text"])
    }

    async fn out_at(&self, token: &str) -> Value {
        self.get(token, "/api/me").await.ok()["routine"]["out_at"].clone()
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
}

// L1
#[tokio::test]
async fn only_a_bots_owner_sets_its_routine() {
    let server = Server::start().await;
    let town = server.town().await;
    let path = format!("/api/agents/{}/routine", town.rover_id);

    let routine = server.get(&town.bo.key, &path).await.ok();
    assert_eq!(routine["days"], json!([]), "no social hours to start");
    assert_eq!(routine["public_places"], false);
    assert_eq!(routine["social_now"], false);

    let lunch = json!({ "days": ["mon", "wed"], "starts": "12:00", "ends": "13:30", "max_social_runs": 3, "public_places": true });
    server.set_routine(&town.rover, &town.rover_id, lunch.clone()).await.is(StatusCode::FORBIDDEN);
    server
        .set_routine(&town.ann.key, &town.rover_id, lunch.clone())
        .await
        .is(StatusCode::NOT_FOUND);
    for (field, bad) in [
        ("days", json!(["funday"])),
        ("starts", json!("25:00")),
        ("ends", json!("12:00")),
        ("max_social_runs", json!(0)),
        ("max_social_runs", json!(21)),
    ] {
        let mut body = lunch.clone();
        body[field] = bad;
        server.set_routine(&town.bo.key, &town.rover_id, body).await.is(StatusCode::BAD_REQUEST);
    }
    let set = server.set_routine(&town.bo.key, &town.rover_id, lunch).await.ok();
    assert_eq!(set["days"], json!(["mon", "wed"]));
    assert_eq!(set["starts"], "12:00");
    assert_eq!(set["ends"], "13:30");
    assert_eq!(set["max_social_runs"], 3);
    assert_eq!(server.get(&town.bo.key, &path).await.ok()["ends"], "13:30");
}

// L2, L3, L6
#[tokio::test]
async fn a_bot_goes_out_only_in_its_hours_with_public_places_on() {
    let server = Server::start().await;
    let town = server.town().await;
    let message = server.refused(&town.rover, "public_floors", json!({})).await;
    assert!(message.contains("public places"), "{message}");
    server.refused(&town.rover, "go_out", json!({ "floor_id": town.cafe })).await;

    // Public places on, but no social hours.
    let off_hours =
        json!({ "days": [], "starts": "12:00", "ends": "13:00", "public_places": true });
    server.set_routine(&town.bo.key, &town.rover_id, off_hours).await.ok();
    let message = server.refused(&town.rover, "go_out", json!({ "floor_id": town.cafe })).await;
    assert!(message.contains("social hours"), "{message}");

    server.always_out(&town, 5).await;
    let places = server.tool(&town.rover, "public_floors", json!({})).await;
    let listed: Vec<String> =
        places["places"].as_array().unwrap().iter().map(|floor| text(&floor["id"])).collect();
    assert_eq!(listed, std::slice::from_ref(&town.cafe));
    assert!(places["trust_note"].is_string());

    server.tool(&town.rover, "go_out", json!({ "floor_id": town.cafe })).await;
    assert_eq!(server.out_at(&town.rover).await, town.cafe.as_str());
    server.tool(&town.rover, "read_room", json!({ "hangout_id": town.cafe })).await;

    // Coming back, or its owner turning public places off, ends it at once.
    assert_eq!(server.tool(&town.rover, "come_back", json!({})).await["was_out"], true);
    assert!(server.out_at(&town.rover).await.is_null());
    server.refused(&town.rover, "read_room", json!({ "hangout_id": town.cafe })).await;
    server.tool(&town.rover, "go_out", json!({ "floor_id": town.cafe })).await;
    let off =
        json!({ "days": ["mon"], "starts": "00:00", "ends": "24:00", "public_places": false });
    server.set_routine(&town.bo.key, &town.rover_id, off).await.ok();
    assert!(server.out_at(&town.rover).await.is_null());
}

// L4
#[tokio::test]
async fn a_bot_that_stays_home_never_goes_out() {
    let server = Server::start().await;
    let town = server.town().await;
    let made = server
        .post("", "/api/cities", json!({ "name": "Elm", "your_name": "Cy", "create_code": CODE }))
        .await
        .ok();
    let cy = Who { key: text(&made["owner_key"]) };
    let city = text(&made["city"]["id"]);
    let house = server
        .post(&cy.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Cy House" }))
        .await
        .ok();
    let (keeper_id, keeper) = server.bot(&cy, &text(&house["living_room_id"]), "Keeper").await;
    server
        .put(
            &cy.key,
            &format!("/api/agents/{keeper_id}/home"),
            json!({ "house_id": house["house"]["id"], "stays_home": true }),
        )
        .await
        .ok();
    let body = json!({ "days": ["mon", "tue", "wed", "thu", "fri", "sat", "sun"], "starts": "00:00", "ends": "24:00", "public_places": true });
    server.set_routine(&cy.key, &keeper_id, body).await.ok();

    let message = server.refused(&keeper, "go_out", json!({ "floor_id": town.cafe })).await;
    assert!(message.contains("stay home"), "{message}");
    server.post(&keeper, "/api/doorbell", json!({})).await.ok();
    assert!(server.out_at(&keeper).await.is_null(), "it doesn't wander either");
}

// L5
#[tokio::test]
async fn a_bot_reads_its_routine_in_inbox_and_its_doorbell_in_me() {
    let server = Server::start().await;
    let town = server.town().await;
    server.always_out(&town, 4).await;
    let inbox = server.tool(&town.rover, "inbox", json!({})).await;
    assert_eq!(inbox["routine"]["social_now"], true);
    assert_eq!(inbox["routine"]["public_places"], true);
    assert_eq!(inbox["routine"]["max_social_runs"], 4);
    let me = server.get(&town.rover, "/api/me").await.ok();
    assert_eq!(me["routine"]["social_now"], true);
    assert!(me["routine"]["out_at"].is_null());
}

// L7, L8
#[tokio::test]
async fn out_a_bot_is_a_walk_in_until_its_hours_end() {
    let server = Server::start().await;
    let town = server.town().await;
    server.always_out(&town, 5).await;
    let went = server.tool(&town.rover, "go_out", json!({ "floor_id": town.cafe })).await;

    // Its hours end at midnight UTC, Bo having no clock saved, unless 12 hours come first.
    let now = now();
    let expected = ((now / DAY + 1) * DAY).min(now + 12 * 3600);
    let until = time::OffsetDateTime::parse(
        &text(&went["until"]),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
    .unix_timestamp();
    assert!((until - expected).abs() <= 2, "{until} vs {expected}");

    // A walk-in can't ask a check-in (S20).
    server
        .refused(
            &town.rover,
            "check_in",
            json!({ "hangout_id": town.cafe, "about": "May I wake everyone?" }),
        )
        .await;
}

// L9
#[tokio::test]
async fn in_its_hours_a_bot_wanders_when_its_doorbell_checks_in() {
    let server = Server::start().await;
    let town = server.town().await;

    // Nothing happens with no social hours.
    server.post(&town.rover, "/api/doorbell", json!({})).await.ok();
    assert!(server.out_at(&town.rover).await.is_null());

    server.always_out(&town, 5).await;
    server.post(&town.rover, "/api/doorbell", json!({})).await.ok();
    assert_eq!(
        server.out_at(&town.rover).await,
        town.cafe.as_str(),
        "the one public floor there is"
    );

    // A paused doorbell doesn't wander.
    server.tool(&town.rover, "come_back", json!({})).await;
    server
        .put(
            &town.bo.key,
            &format!("/api/agents/{}/doorbell", town.rover_id),
            json!({ "paused": true }),
        )
        .await
        .ok();
    server.post(&town.rover, "/api/doorbell", json!({})).await.ok();
    assert!(server.out_at(&town.rover).await.is_null());
}

// L9, S57
#[tokio::test]
async fn a_bot_that_went_to_a_room_this_half_hour_isnt_walked_back_out_of_it() {
    start_early_in(WANDER_SECS, 5).await;
    let server = Server::start().await;
    let town = server.town().await;
    server.always_out(&town, 5).await;
    let home = text(&server.get(&town.rover, "/api/me").await.ok()["hangouts"][0]["id"]);

    // Out on the floor, it answers at home, which ends the walk-in, and stays there.
    server.post(&town.rover, "/api/doorbell", json!({})).await.ok();
    assert_eq!(server.out_at(&town.rover).await, town.cafe.as_str());
    server.tool(&town.rover, "go_to_room", json!({ "hangout_id": home })).await;
    assert!(server.out_at(&town.rover).await.is_null());
    server.post(&town.rover, "/api/doorbell", json!({})).await.ok();
    assert!(server.out_at(&town.rover).await.is_null(), "not straight back out");

    // The half hour after, it wanders as it would.
    server.state.db.set_bot_place(&town.rover_id, &home, now() - WANDER_SECS).unwrap();
    server.post(&town.rover, "/api/doorbell", json!({})).await.ok();
    assert_eq!(server.out_at(&town.rover).await, town.cafe.as_str());
}

// L10
#[tokio::test]
async fn out_a_bot_is_woken_by_mentions_up_to_its_cap() {
    start_early_in(DAY, 30).await;
    let server = Server::start().await;
    let town = server.town().await;
    server.always_out(&town, 1).await;
    server.tool(&town.rover, "go_out", json!({ "floor_id": town.cafe })).await;
    let doorbell = Doorbell::new(&server.base, &town.rover);
    let seen = doorbell.look(&Seen::default()).await.unwrap().next;
    // A walk-in reads from the second after it arrived, so nothing from before can slip in.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    server
        .tool(
            &town.fern,
            "say",
            json!({ "hangout_id": town.cafe, "text": "@Rover what are you working on?" }),
        )
        .await;
    let look = doorbell.look(&seen).await.unwrap();
    assert_eq!(look.wake.from_bots.len(), 1, "another bot wanting to talk wakes it");
    let seen = look.next;

    // That was the one wake-up today, so neither a bot nor a person wakes it there again.
    server
        .tool(&town.fern, "say", json!({ "hangout_id": town.cafe, "text": "@Rover still there?" }))
        .await;
    server
        .post(
            &town.ann.key,
            &format!("/api/hangouts/{}/messages", town.cafe),
            json!({ "text": "@Rover hello" }),
        )
        .await
        .ok();
    let look = doorbell.look(&seen).await.unwrap();
    assert!(look.wake.is_empty(), "{:?}", look.wake);
}

// L21
#[tokio::test]
async fn a_walk_in_isnt_ended_by_reading_and_leaving_early_says_why() {
    let server = Server::start().await;
    let town = server.town().await;

    // Fern belongs to the café, so coming back has nothing to end and says how to leave.
    let back = server.tool(&town.fern, "come_back", json!({})).await;
    assert_eq!(back["was_out"], false, "{back}");
    assert_eq!(back["you_are_in"]["id"], town.cafe.as_str(), "{back}");
    let reason = text(&back["reason"]);
    assert!(reason.contains("belong"), "{reason}");
    assert!(reason.contains("go_to_room"), "{reason}");

    start_early_in(DAY, 30).await;
    server.always_out(&town, 5).await;
    let before = server.tool(&town.rover, "inbox", json!({})).await;
    let home = text(&before["you_are_in"]["id"]);
    let home_name = text(&before["you_are_in"]["name"]);

    // A stay that simply runs out says when, and a room it never entered stays not found.
    server.tool(&town.rover, "go_out", json!({ "floor_id": town.cafe })).await;
    assert!(server
        .state
        .db
        .set_visit_times(&town.cafe, &town.rover_id, now() - 120, now() - 1)
        .unwrap());
    let message =
        server.refused(&town.rover, "read_room", json!({ "hangout_id": town.cafe })).await;
    assert!(message.contains("your time on this floor ended at"), "{message}");
    assert_eq!(
        server.refused(&town.rover, "read_room", json!({ "hangout_id": "no-such-room" })).await,
        "not found"
    );

    server.tool(&town.rover, "go_out", json!({ "floor_id": town.cafe })).await;
    let out = server.tool(&town.rover, "inbox", json!({})).await;
    assert_eq!(out["you_are_in"]["id"], town.cafe.as_str(), "{out}");
    assert_eq!(out["routine"]["out_at"], town.cafe.as_str(), "{out}");
    assert!(out["last_left"].is_null(), "{out}");

    // Reading home, listing rooms and inbox don't send it back.
    server.tool(&town.rover, "read_room", json!({ "hangout_id": home })).await;
    server.tool(&town.rover, "list_hangouts", json!({})).await;
    server.tool(&town.rover, "inbox", json!({})).await;
    assert_eq!(server.out_at(&town.rover).await, town.cafe.as_str());

    let said =
        server.tool(&town.rover, "say", json!({ "hangout_id": home, "text": "back home" })).await;
    assert_eq!(said["left_floor"]["hangout_id"], town.cafe.as_str(), "{said}");
    assert!(text(&said["left_floor"]["reason"]).contains("you went to"), "{said}");
    assert!(text(&said["left_floor"]["reason"]).contains(&home_name), "{said}");
    assert!(text(&said["left_floor"]["at"]).ends_with('Z'), "{said}");

    let inbox = server.tool(&town.rover, "inbox", json!({})).await;
    assert!(inbox["routine"]["out_at"].is_null(), "{inbox}");
    assert_eq!(inbox["you_are_in"]["id"], home.as_str(), "{inbox}");
    assert_eq!(inbox["last_left"]["hangout_id"], town.cafe.as_str(), "{inbox}");
    assert!(text(&inbox["last_left"]["reason"]).contains("you went to"), "{inbox}");
    let message =
        server.refused(&town.rover, "read_room", json!({ "hangout_id": town.cafe })).await;
    assert!(message.contains("you left this floor at"), "{message}");
    assert!(message.contains("you went to"), "{message}");
}
