//! Adversarial tests for standing, vouching, the city gate and house requirements. They drive a
//! real server over HTTP and are written from the rules in docs/accounts-and-trust.md (T1 to T21,
//! and the limits table), with docs/city.md's R1 to R48 still holding. They are not written from
//! the code: each one says what the rule demands and tries to get around it.
//!
//! A comment such as `// T13` above a test names the rule it covers, so coverage can be checked by
//! searching for it. Tests marked `// JUDGEMENT` rest on a reading of the doc that the doc does not
//! spell out: the report that came with them says why.

mod support;

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration as StdDuration, Instant},
};

use agenthangout::{
    app, auth,
    db::{Db, NewEvent},
    now, AppState, Config,
};
use reqwest::{header, Client, Method, StatusCode};
use serde_json::{json, Value};
use support::Authenticator;

const DAY: i64 = 86_400;
const NO_SUCH_THING: &str = "zzzzzzzzzzzzzzzz";
// Words written into plans, goals and names, so a test can search every reply for them.
const PLAN_TITLE: &str = "plan-title-7731";
const PLAN_PLACE: &str = "plan-place-7731";
const TRIP_GOAL: &str = "trip-goal-7731";
const CHAT_WORDS: &str = "chat-words-7731";

struct Server {
    base: String,
    origin: String,
    client: Client,
    state: AppState,
}

struct Reply {
    status: StatusCode,
    body: Value,
    set_cookie: Option<String>,
}

impl Reply {
    fn show(&self) -> String {
        format!("{} {}", self.status, self.body)
    }
}

/// A person, by whatever they sign in with. A key starting `cookie:` is sent as the session
/// cookie, as a browser would, and anything else as a bearer token.
#[derive(Clone)]
struct Who {
    key: String,
    id: String,
}

#[derive(Clone)]
struct Home {
    id: String,
    living: String,
    porch: String,
}

#[derive(Clone)]
struct Bot {
    token: String,
    id: String,
}

/// Ann started the city Elm and built Ann House. Bo joined and built Bo House, where his bot Scout
/// lives. Nobody has set any rules yet, so everyone who comes by the link is simply in.
struct World {
    city: String,
    square: String,
    code: String,
    ann: Who,
    ann_home: Home,
    bo: Who,
    bo_home: Home,
    scout: Bot,
}

/// Someone who signed up with a passkey.
struct Account {
    who: Who,
    owner_key: String,
    #[allow(dead_code)]
    key: Authenticator,
}

impl Server {
    async fn start() -> Self {
        Self::start_with(None).await
    }

    async fn start_with(create_code: Option<&str>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // What the server believes its address is, which is what passkeys are signed for.
        let origin = format!("http://localhost:{port}");
        let state = AppState::new(
            Db::open(":memory:").unwrap(),
            Config {
                public_url: origin.clone(),
                create_code: create_code.map(str::to_string),
                allow_private_webhooks: true,
                keeper_id: None,
                god_ids: Vec::new(),
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        Self { base: format!("http://127.0.0.1:{port}"), origin, client: Client::new(), state }
    }

    async fn send(
        &self,
        token: &str,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
    ) -> Reply {
        let mut request = self.client.request(method, format!("{}{path}", self.base));
        if let Some(cookie) = token.strip_prefix("cookie:") {
            request = request.header(header::COOKIE, format!("ah_session={cookie}"));
        } else if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .map(|value| value.to_str().unwrap().to_string());
        // An open stream never ends, so it is reported rather than read.
        let streaming = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        if streaming {
            return Reply { status, body: Value::String("<event stream>".into()), set_cookie };
        }
        let text = response.text().await.unwrap();
        Reply {
            status,
            set_cookie,
            body: serde_json::from_str(&text).unwrap_or(Value::String(text)),
        }
    }

    async fn request(&self, token: &str, method: Method, path: &str, body: Option<Value>) -> Reply {
        let bytes = body.map(|body| serde_json::to_vec(&body).unwrap());
        self.send(token, method, path, bytes, Some("application/json")).await
    }

    async fn get(&self, token: &str, path: &str) -> Reply {
        self.send(token, Method::GET, path, None, None).await
    }

    async fn post(&self, token: &str, path: &str, body: Value) -> Reply {
        self.request(token, Method::POST, path, Some(body)).await
    }

    async fn put(&self, token: &str, path: &str, body: Value) -> Reply {
        self.request(token, Method::PUT, path, Some(body)).await
    }

    async fn delete(&self, token: &str, path: &str) -> Reply {
        self.request(token, Method::DELETE, path, None).await
    }

    async fn mcp(&self, token: &str, method: &str, params: Value) -> Reply {
        self.post(
            token,
            "/mcp",
            json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }),
        )
        .await
    }

    async fn tool(&self, token: &str, name: &str, arguments: Value) -> Value {
        let reply =
            self.mcp(token, "tools/call", json!({ "name": name, "arguments": arguments })).await;
        reply.body["result"].clone()
    }

    async fn stream(&self, token: &str, path: &str) -> reqwest::Response {
        let mut request = self.client.get(format!("{}{path}", self.base));
        if let Some(cookie) = token.strip_prefix("cookie:") {
            request = request.header(header::COOKIE, format!("ah_session={cookie}"));
        } else if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        request.send().await.unwrap()
    }

    // People

    async fn new_city(&self, name: &str, your_name: &str) -> (String, String, String, Who) {
        let made =
            self.post("", "/api/cities", json!({ "name": name, "your_name": your_name })).await;
        assert_eq!(made.status, StatusCode::OK, "{}", made.body);
        let person =
            Who { key: text(&made.body["owner_key"]), id: text(&made.body["person"]["id"]) };
        (
            text(&made.body["city"]["id"]),
            text(&made.body["city"]["square_id"]),
            code_of(&made.body["invite_url"]),
            person,
        )
    }

    /// Someone new uses the city's link and is simply in (the city has asked for nothing yet).
    async fn join_city(&self, code: &str, name: &str) -> Who {
        let joined =
            self.use_link(code, "", json!({ "your_name": name, "accept_rules": true })).await;
        assert_eq!(joined.status, StatusCode::OK, "{}", joined.body);
        assert_eq!(joined.body["waiting"], false, "{}", joined.body);
        Who { key: text(&joined.body["owner_key"]), id: text(&joined.body["person"]["id"]) }
    }

    /// Uses the city's link as `token` (nobody, if empty) and returns whatever came back.
    async fn use_link(&self, code: &str, token: &str, body: Value) -> Reply {
        self.post(token, &format!("/api/city-invites/{code}/join"), body).await
    }

    /// A new person arrives at the gate of a city that asks for vouches, and waits.
    async fn at_the_gate(&self, code: &str, name: &str) -> Who {
        let arrived =
            self.use_link(code, "", json!({ "your_name": name, "accept_rules": true })).await;
        assert_eq!(arrived.status, StatusCode::OK, "{}", arrived.body);
        assert_eq!(arrived.body["waiting"], true, "{}", arrived.body);
        Who { key: text(&arrived.body["owner_key"]), id: text(&arrived.body["person"]["id"]) }
    }

    /// A person with no key anyone saw, who joined the city `days` days ago.
    fn old_member(&self, city: &str, name: &str, days: i64) -> Who {
        let key = auth::new_owner_key();
        let id = auth::new_id();
        self.state
            .db
            .create_person(&id, name, &auth::hash_secret(&key), &auth::new_feed_token(), now())
            .unwrap();
        self.state.db.add_city_member(city, &id, now() - days * DAY - 60).unwrap();
        Who { key, id }
    }

    async fn key_person(&self, name: &str) -> Who {
        let made =
            self.post("", "/api/hangouts", json!({ "name": "Home", "your_name": name })).await;
        assert_eq!(made.status, StatusCode::OK, "{}", made.body);
        Who { key: text(&made.body["owner_key"]), id: text(&made.body["person"]["id"]) }
    }

    async fn build_house(&self, owner: &Who, city: &str, name: &str) -> Home {
        let built = self
            .post(&owner.key, &format!("/api/cities/{city}/houses"), json!({ "name": name }))
            .await;
        assert_eq!(built.status, StatusCode::OK, "{}", built.body);
        Home {
            id: text(&built.body["house"]["id"]),
            living: text(&built.body["living_room_id"]),
            porch: text(&built.body["porch_id"]),
        }
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> Bot {
        let made = self
            .post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await;
        assert_eq!(made.status, StatusCode::OK, "{}", made.body);
        Bot { token: text(&made.body["token"]), id: text(&made.body["agent"]["id"]) }
    }

    async fn house_bot(&self, resident: &Who, home: &Home, name: &str) -> Bot {
        let made = self
            .post(
                &resident.key,
                &format!("/api/houses/{}/house-bot", home.id),
                json!({ "name": name }),
            )
            .await;
        assert_eq!(made.status, StatusCode::OK, "{}", made.body);
        Bot { token: text(&made.body["token"]), id: text(&made.body["agent"]["id"]) }
    }

    async fn world(&self) -> World {
        let (city, square, code, ann) = self.new_city("Elm", "Ann").await;
        let ann_home = self.build_house(&ann, &city, "Ann House").await;
        let bo = self.join_city(&code, "Bo").await;
        let bo_home = self.build_house(&bo, &city, "Bo House").await;
        let scout = self.bot(&bo, &bo_home.living, "Scout").await;
        World { city, square, code, ann, ann_home, bo, bo_home, scout }
    }

    /// Signs up with a passkey, as the sign-up page does. The session is a cookie.
    async fn sign_up(&self, name: &str) -> Account {
        let mut key = Authenticator::new(&self.origin);
        let started =
            self.post("", "/api/passkeys/register/start", json!({ "your_name": name })).await;
        assert_eq!(started.status, StatusCode::OK, "{}", started.body);
        let credential = key.register(&started.body["options"]);
        let done = self
            .post(
                "",
                "/api/passkeys/register/finish",
                json!({ "ceremony_id": started.body["ceremony_id"], "credential": credential }),
            )
            .await;
        assert_eq!(done.status, StatusCode::OK, "{}", done.body);
        let cookie = done.set_cookie.as_deref().unwrap().split(';').next().unwrap();
        let token = cookie.strip_prefix("ah_session=").unwrap();
        Account {
            who: Who { key: format!("cookie:{token}"), id: text(&done.body["person"]["id"]) },
            owner_key: text(&done.body["owner_key"]),
            key,
        }
    }

    // Rules, vouches and the gate

    async fn set_city_rules(&self, token: &str, city: &str, body: Value) -> Reply {
        self.put(token, &format!("/api/cities/{city}/rules"), body).await
    }

    async fn set_house_rules(&self, token: &str, house: &str, body: Value) -> Reply {
        self.put(token, &format!("/api/houses/{house}/rules"), body).await
    }

    async fn vouch(&self, token: &str, city: &str, person: &str) -> Reply {
        self.post(token, &format!("/api/cities/{city}/people/{person}/vouch"), json!({})).await
    }

    async fn unvouch(&self, token: &str, city: &str, person: &str) -> Reply {
        self.delete(token, &format!("/api/cities/{city}/people/{person}/vouch")).await
    }

    async fn decline(&self, token: &str, city: &str, person: &str) -> Reply {
        self.post(token, &format!("/api/cities/{city}/gate/{person}/decline"), json!({})).await
    }

    async fn clear_gate(&self, token: &str, city: &str) -> Reply {
        self.post(token, &format!("/api/cities/{city}/gate/clear"), json!({})).await
    }

    async fn standing(&self, token: &str, city: &str, person: &str) -> Reply {
        self.get(token, &format!("/api/cities/{city}/people/{person}/standing")).await
    }

    async fn city_view(&self, token: &str, city: &str) -> Reply {
        self.get(token, &format!("/api/cities/{city}")).await
    }

    async fn house_view(&self, token: &str, house: &str) -> Reply {
        self.get(token, &format!("/api/houses/{house}")).await
    }

    async fn me(&self, token: &str) -> Value {
        let me = self.get(token, "/api/me").await;
        assert_eq!(me.status, StatusCode::OK, "{}", me.body);
        me.body
    }

    /// The people at the gate, as a member of the city sees them.
    async fn gate(&self, token: &str, city: &str) -> Vec<Value> {
        let view = self.city_view(token, city).await;
        assert_eq!(view.status, StatusCode::OK, "{}", view.body);
        view.body["gate"].as_array().unwrap_or_else(|| panic!("no gate in {}", view.body)).clone()
    }

    /// The city asks for `vouches` vouches, set by its keeper.
    async fn require_vouches(&self, keeper: &Who, city: &str, vouches: i64) {
        let set = self.set_city_rules(&keeper.key, city, json!({ "min_vouches": vouches })).await;
        assert_eq!(set.status, StatusCode::OK, "{}", set.body);
    }

    async fn require(&self, resident: &Who, house: &Home, body: Value) {
        let set = self.set_house_rules(&resident.key, &house.id, body).await;
        assert_eq!(set.status, StatusCode::OK, "{}", set.body);
    }

    // Trips

    async fn ask_trip(
        &self,
        token: &str,
        agent: Option<&str>,
        house: &str,
        minutes: Option<i64>,
    ) -> Reply {
        let mut body = json!({ "house_id": house, "kind": "call", "goal": TRIP_GOAL });
        if let Some(agent) = agent {
            body["agent_id"] = json!(agent);
        }
        if let Some(minutes) = minutes {
            body["minutes"] = json!(minutes);
        }
        self.post(token, "/api/trips", body).await
    }

    async fn answer_trip(&self, token: &str, trip: &str, response: &str) -> Reply {
        self.post(token, &format!("/api/trips/{trip}/answer"), json!({ "response": response }))
            .await
    }

    async fn end_trip(&self, token: &str, trip: &str) -> Reply {
        self.post(token, &format!("/api/trips/{trip}/end"), json!({})).await
    }

    /// The bot asks, its owner and a resident both say yes, and it arrives. Returns the trip.
    async fn arrive(&self, bot: &Bot, owner: &Who, host: &Who, house: &str) -> String {
        let asked = self.ask_trip(&bot.token, None, house, None).await;
        assert_eq!(asked.status, StatusCode::OK, "{}", asked.body);
        let id = text(&asked.body["id"]);
        assert_eq!(self.answer_trip(&owner.key, &id, "yes").await.status, StatusCode::OK);
        let here = self.answer_trip(&host.key, &id, "yes").await;
        assert_eq!(here.body["status"], "here", "{}", here.body);
        id
    }

    async fn offer_and_accept_pass(
        &self,
        resident: &Who,
        owner: &Who,
        house: &str,
        bot: &str,
    ) -> String {
        let offered = self
            .post(&resident.key, &format!("/api/houses/{house}/passes"), json!({ "agent_id": bot }))
            .await;
        assert_eq!(offered.status, StatusCode::OK, "{}", offered.body);
        let id = text(&offered.body["id"]);
        let accepted = self.post(&owner.key, &format!("/api/passes/{id}/accept"), json!({})).await;
        assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.body);
        id
    }

    /// A plan in a room that finished `ended_ago` seconds ago, with a yes from each person named.
    fn plan(&self, room: &str, status: &str, ended_ago: i64, yes: &[&Who]) -> String {
        let id = format!("past-plan-{}", UNIQUE.fetch_add(1, Ordering::SeqCst));
        let at = now();
        self.state
            .db
            .insert_event(&NewEvent {
                id: &id,
                hangout_id: room,
                title: PLAN_TITLE,
                location: PLAN_PLACE,
                notes: "",
                starts_at: at - ended_ago - 3600,
                ends_at: at - ended_ago,
                min_yes: 1,
                proposed_by: &yes[0].id,
                proposer: "someone",
                now: at - ended_ago - 7200,
            })
            .unwrap();
        if status != "proposed" {
            self.state.db.set_event_status(&id, status, at).unwrap();
        }
        for who in yes {
            self.state.db.upsert_rsvp(&id, &who.id, "yes", None, "", at).unwrap();
        }
        id
    }

    /// A finished, confirmed Town Square plan that every person named said yes to.
    fn gathering(&self, square: &str, yes: &[&Who]) -> String {
        self.plan(square, "confirmed", 3600, yes)
    }

    async fn notify_to(&self, who: &Who) -> Arc<Mutex<Vec<Value>>> {
        let (url, received) = start_receiver().await;
        let saved = self.put(&who.key, "/api/notify", json!({ "url": url })).await;
        assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
        received
    }
}

static UNIQUE: AtomicUsize = AtomicUsize::new(0);

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not a string: {value}")).to_string()
}

/// The code at the end of an invite link.
fn code_of(url: &Value) -> String {
    text(url).rsplit('/').next().unwrap().to_string()
}

fn has(value: &Value, needle: &str) -> bool {
    value.to_string().contains(needle)
}

/// This JSON as text, with these ids taken out. An id is random, so by chance one can spell a short
/// name such as Bo, and that gives nobody away. Pass ids the test knows from elsewhere, not ones read
/// from this reply, so that an id that shouldn't be in it is still found.
fn without_ids(value: &Value, ids: &[impl AsRef<str>]) -> String {
    ids.iter().fold(value.to_string(), |said, id| said.replace(id.as_ref(), "ID"))
}

fn keys_of(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

#[track_caller]
fn assert_ok(reply: &Reply) {
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
}

#[track_caller]
fn assert_status(reply: &Reply, status: StatusCode) {
    assert_eq!(reply.status, status, "{}", reply.body);
}

#[track_caller]
fn assert_not_found(reply: &Reply) {
    assert_status(reply, StatusCode::NOT_FOUND);
}

#[track_caller]
fn assert_forbidden(reply: &Reply) {
    assert_status(reply, StatusCode::FORBIDDEN);
}

/// Not a success of any kind.
#[track_caller]
fn assert_denied(reply: &Reply) {
    assert!(!reply.status.is_success(), "expected a refusal, got {}", reply.show());
}

/// A web server that records what's posted to it, standing in for Slack or a bot's webhook.
async fn start_receiver() -> (String, Arc<Mutex<Vec<Value>>>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let store = received.clone();
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let store = store.clone();
            async move { store.lock().unwrap().push(body) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    (url, received)
}

fn heard_of(received: &Mutex<Vec<Value>>, event: &str) -> Vec<Value> {
    received.lock().unwrap().iter().filter(|notice| notice["event"] == event).cloned().collect()
}

/// For a notice that should not arrive: waits half a second, long enough for one that was going to,
/// then returns the notices of that event. For one that should, use `expect_notices`, because a
/// fixed wait is too short on a busy machine.
async fn notices_of(received: &Mutex<Vec<Value>>, event: &str) -> Vec<Value> {
    tokio::time::sleep(StdDuration::from_millis(500)).await;
    heard_of(received, event)
}

/// Waits up to ten seconds for `count` notices of an event to arrive, then a moment more to see
/// that no extra one does, and returns them.
async fn expect_notices(received: &Mutex<Vec<Value>>, event: &str, count: usize) -> Vec<Value> {
    let deadline = Instant::now() + StdDuration::from_secs(10);
    while heard_of(received, event).len() < count && Instant::now() < deadline {
        tokio::time::sleep(StdDuration::from_millis(20)).await;
    }
    tokio::time::sleep(StdDuration::from_millis(300)).await;
    heard_of(received, event)
}

async fn all_notices(received: &Mutex<Vec<Value>>) -> Vec<Value> {
    tokio::time::sleep(StdDuration::from_millis(500)).await;
    received.lock().unwrap().clone()
}

// ----------------------------------------------------------------------------------------------
// Every id a stranger might try, so a real one and a made-up one can be compared.
// ----------------------------------------------------------------------------------------------

struct Ids {
    city: String,
    square: String,
    house: String,
    living: String,
    porch: String,
    trip: String,
    pass: String,
    person: String,
    bot: String,
    event: String,
}

fn probes(ids: &Ids) -> Vec<(Method, String, Option<Value>)> {
    let Ids { city, square, house, living, porch, trip, pass, person, bot, event } = ids;
    let mut calls = vec![
        (Method::GET, format!("/api/cities/{city}"), None),
        (Method::GET, format!("/api/cities/{city}/stream"), None),
        (Method::POST, format!("/api/cities/{city}/invite/rotate"), Some(json!({}))),
        (Method::POST, format!("/api/cities/{city}/leave"), Some(json!({}))),
        (Method::POST, format!("/api/cities/{city}/houses"), Some(json!({ "name": "Mine" }))),
        (Method::PUT, format!("/api/cities/{city}/rules"), Some(json!({ "min_vouches": 1 }))),
        (
            Method::PUT,
            format!("/api/cities/{city}/rules"),
            Some(json!({ "rules_text": "mine now" })),
        ),
        (Method::PUT, format!("/api/cities/{city}/rules"), Some(json!({ "min_vouches": "x" }))),
        (Method::GET, format!("/api/cities/{city}/people/{person}/standing"), None),
        (Method::POST, format!("/api/cities/{city}/people/{person}/vouch"), Some(json!({}))),
        (Method::DELETE, format!("/api/cities/{city}/people/{person}/vouch"), None),
        (Method::POST, format!("/api/cities/{city}/gate/{person}/decline"), Some(json!({}))),
        (Method::POST, format!("/api/cities/{city}/gate/clear"), Some(json!({}))),
        (Method::GET, format!("/api/houses/{house}"), None),
        (Method::POST, format!("/api/houses/{house}/invite/rotate"), Some(json!({}))),
        (Method::POST, format!("/api/houses/{house}/leave"), Some(json!({}))),
        (Method::PUT, format!("/api/houses/{house}/rules"), Some(json!({ "min_gatherings": 1 }))),
        (Method::PUT, format!("/api/houses/{house}/rules"), Some(json!({ "min_gatherings": "x" }))),
        (Method::DELETE, format!("/api/houses/{house}/residents/{person}"), None),
        (Method::POST, format!("/api/houses/{house}/house-bot"), Some(json!({ "name": "Casa" }))),
        (Method::POST, format!("/api/houses/{house}/passes"), Some(json!({ "agent_id": bot }))),
        (Method::GET, format!("/api/houses/{house}/scenes"), None),
        (Method::POST, format!("/api/houses/{house}/scenes"), Some(json!({ "name": "Party" }))),
        (
            Method::PUT,
            format!("/api/agents/{bot}/home"),
            Some(json!({ "house_id": house, "stays_home": false })),
        ),
        (
            Method::POST,
            "/api/trips".to_string(),
            Some(json!({ "agent_id": bot, "house_id": house, "kind": "call" })),
        ),
        (Method::GET, format!("/api/trips/{trip}"), None),
        (Method::POST, format!("/api/trips/{trip}/answer"), Some(json!({ "response": "yes" }))),
        (Method::POST, format!("/api/trips/{trip}/end"), Some(json!({}))),
        (Method::POST, format!("/api/trips/{trip}/extend"), Some(json!({ "minutes": 5 }))),
        (Method::POST, format!("/api/passes/{pass}/accept"), Some(json!({}))),
        (Method::DELETE, format!("/api/passes/{pass}"), None),
        (Method::POST, format!("/api/events/{event}/rsvp"), Some(json!({ "response": "yes" }))),
        (Method::POST, format!("/api/events/{event}/cancel"), Some(json!({}))),
        (Method::GET, format!("/api/events/{event}/ics"), None),
        (Method::POST, format!("/api/agents/{bot}/rotate"), Some(json!({}))),
        (Method::PUT, format!("/api/agents/{bot}/notify"), Some(json!({ "url": null }))),
        (Method::PUT, format!("/api/agents/{bot}/doorbell"), Some(json!({ "paused": true }))),
    ];
    for room in [square, living, porch] {
        calls.extend([
            (Method::GET, format!("/api/hangouts/{room}"), None),
            (Method::GET, format!("/api/hangouts/{room}/stream"), None),
            (Method::GET, format!("/api/hangouts/{room}/messages?after=0"), None),
            (Method::POST, format!("/api/hangouts/{room}/messages"), Some(json!({ "text": "hi" }))),
            (Method::POST, format!("/api/hangouts/{room}/notes"), Some(json!({ "text": "hi" }))),
            (
                Method::POST,
                format!("/api/hangouts/{room}/check-ins"),
                Some(json!({ "about": "hi" })),
            ),
            (Method::POST, format!("/api/hangouts/{room}/agents"), Some(json!({ "name": "Spy" }))),
            (Method::POST, format!("/api/hangouts/{room}/invite/rotate"), Some(json!({}))),
            (Method::POST, format!("/api/hangouts/{room}/leave"), Some(json!({}))),
        ]);
    }
    calls
}

impl Server {
    /// Real ids from a world with a trip, a pass and a plan, and the same shapes made up.
    async fn ids(&self, w: &World) -> (Ids, Ids) {
        let trip = self.arrive(&w.scout, &w.bo, &w.ann, &w.ann_home.id).await;
        let pass = self.offer_and_accept_pass(&w.ann, &w.bo, &w.ann_home.id, &w.scout.id).await;
        let event = self.plan(&w.ann_home.porch, "confirmed", 3600, &[&w.ann]);
        let real = Ids {
            city: w.city.clone(),
            square: w.square.clone(),
            house: w.ann_home.id.clone(),
            living: w.ann_home.living.clone(),
            porch: w.ann_home.porch.clone(),
            trip,
            pass,
            person: w.ann.id.clone(),
            bot: w.scout.id.clone(),
            event,
        };
        let nothing = NO_SUCH_THING.to_string();
        let fake = Ids {
            city: nothing.clone(),
            square: nothing.clone(),
            house: nothing.clone(),
            living: nothing.clone(),
            porch: nothing.clone(),
            trip: nothing.clone(),
            pass: nothing.clone(),
            person: nothing.clone(),
            bot: nothing.clone(),
            event: nothing,
        };
        (real, fake)
    }

    /// Every probe as `token`, against real ids and made-up ones, and insists the answers are the
    /// same byte for byte and are refusals.
    async fn assert_blind(&self, who: &str, token: &str, real: &Ids, fake: &Ids) {
        let real_calls = probes(real);
        let fake_calls = probes(fake);
        assert!(real_calls.len() > 60);
        for ((method, path, body), (_, fake_path, fake_body)) in real_calls.iter().zip(&fake_calls)
        {
            let seen = self.request(token, method.clone(), path, body.clone()).await;
            let nothing = self.request(token, method.clone(), fake_path, fake_body.clone()).await;
            assert_eq!(
                seen.status,
                nothing.status,
                "{who}: {method} {path} gave {}, a place that isn't there gave {}",
                seen.show(),
                nothing.show()
            );
            assert_eq!(
                seen.body, nothing.body,
                "{who}: {method} {path} answered differently from a place that isn't there"
            );
            assert_denied(&seen);
        }
    }
}

// ----------------------------------------------------------------------------------------------
// Standing (T1 to T5)
// ----------------------------------------------------------------------------------------------

impl Server {
    async fn stand(&self, token: &str, city: &str, person: &str) -> Value {
        let reply = self.standing(token, city, person).await;
        assert_ok(&reply);
        reply.body
    }
}

const STANDING_KEYS: [&str; 6] =
    ["gatherings", "good_trips", "member_days", "member_since", "sent_home", "vouches"];

// T1
#[tokio::test]
async fn standing_is_only_the_counts_the_doc_names_and_is_worked_out_on_every_read() {
    let server = Server::start().await;
    let w = server.world().await;

    let before = server.stand(&w.ann.key, &w.city, &w.bo.id).await;
    assert_eq!(keys_of(&before), STANDING_KEYS, "no score, level, points or rank: only the counts");
    assert_eq!((before["gatherings"].as_i64(), before["good_trips"].as_i64()), (Some(0), Some(0)));

    // Change the facts underneath and the very next read changes with them: nothing is cached.
    let plan = server.gathering(&w.square, &[&w.ann, &w.bo]);
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["gatherings"], 1);
    server.state.db.upsert_rsvp(&plan, &w.bo.id, "no", None, "", now()).unwrap();
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["gatherings"], 0);
    server.state.db.upsert_rsvp(&plan, &w.bo.id, "yes", None, "", now()).unwrap();
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["gatherings"], 1);
    server.state.db.set_event_status(&plan, "cancelled", now()).unwrap();
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["gatherings"], 0);

    // A trip that arrived and went home is a good one the moment it is over, and not before.
    let trip = server.arrive(&w.scout, &w.bo, &w.ann, &w.ann_home.id).await;
    assert_eq!(
        server.stand(&w.ann.key, &w.city, &w.bo.id).await["good_trips"],
        0,
        "still under way"
    );
    assert_ok(&server.end_trip(&w.scout.token, &trip).await);
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["good_trips"], 1);
}

// T1
#[tokio::test]
async fn no_table_in_the_database_holds_a_score_or_a_copy_of_standing() {
    let path = std::env::temp_dir().join(format!("trust-schema-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    drop(Db::open(path.to_str().unwrap()).unwrap());
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut tables = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").unwrap();
    let names: Vec<String> =
        tables.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
    assert!(
        names.iter().any(|name| name == "vouches") && names.iter().any(|name| name == "sent_home")
    );
    for table in &names {
        for banned in
            ["standing", "score", "reputation", "points", "karma", "rank", "level", "credit"]
        {
            assert!(!table.contains(banned), "a table called {table} suggests stored standing");
        }
        let mut columns = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
        let cols: Vec<String> =
            columns.query_map([], |row| row.get(1)).unwrap().map(Result::unwrap).collect();
        for column in cols {
            for banned in ["score", "reputation", "points", "karma", "standing"] {
                assert!(!column.contains(banned), "{table}.{column} looks like stored standing");
            }
            // The counts themselves are worked out, never kept. (`min_gatherings` and so on are what a
            // house asks for, which is a choice and not a count.)
            for banned in
                ["good_trips", "gatherings", "member_days", "sent_home_count", "vouch_count"]
            {
                assert_ne!(column, banned, "{table}.{column} looks like a stored count");
            }
        }
    }
    drop(tables);
    drop(conn);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

// T2
#[tokio::test]
async fn chat_notes_tokens_bot_activity_and_requests_never_add_to_standing() {
    let server = Server::start().await;
    let w = server.world().await;
    let before = server.stand(&w.ann.key, &w.city, &w.bo.id).await;

    // Scout and Bo talk in the Town Square, share notes, report activity, set a status.
    let bring = server
        .put(&w.bo.key, &format!("/api/hangouts/{}/agents/{}", w.square, w.scout.id), json!({}))
        .await;
    assert_ok(&bring);
    for token in [&w.bo.key, &w.scout.token] {
        for n in 0..3 {
            let said = json!({ "text": format!("{CHAT_WORDS} {n}") });
            assert_ok(
                &server.post(token, &format!("/api/hangouts/{}/messages", w.square), said).await,
            );
            let noted = json!({ "text": CHAT_WORDS });
            assert_ok(
                &server.post(token, &format!("/api/hangouts/{}/notes", w.square), noted).await,
            );
        }
    }
    assert_ok(&server.post(&w.scout.token, "/api/status", json!({ "status": "busy" })).await);
    assert_ok(
        &server
            .post(&w.scout.token, "/api/activity", json!({ "kind": "computer", "via": "FlowRMM" }))
            .await,
    );
    // New tokens, and asking again and again, and being turned down.
    let rotated =
        server.post(&w.bo.key, &format!("/api/agents/{}/rotate", w.scout.id), json!({})).await;
    let scout = text(&rotated.body["token"]);
    for _ in 0..3 {
        let asked = server.ask_trip(&scout, None, &w.ann_home.id, None).await;
        assert_ok(&asked);
        assert_ok(&server.end_trip(&scout, &text(&asked.body["id"])).await);
    }
    let declined = server.ask_trip(&scout, None, &w.ann_home.id, None).await;
    assert_ok(&server.answer_trip(&w.ann.key, &text(&declined.body["id"]), "no").await);

    let after = server.stand(&w.ann.key, &w.city, &w.bo.id).await;
    assert_eq!(before["gatherings"], after["gatherings"]);
    assert_eq!(before["good_trips"], after["good_trips"]);
    assert_eq!(before["sent_home"], after["sent_home"]);
    assert_eq!(before["vouches"], after["vouches"]);
    assert_eq!(after["good_trips"], 0, "asking and being refused is not a trip");
}

// T2
#[tokio::test]
async fn a_bots_yes_is_its_owners_but_a_gathering_still_needs_a_second_person_and_a_finished_plan()
{
    let server = Server::start().await;
    let w = server.world().await;

    // The owner and the owner's own bot both say yes: one person, not two (a gathering needs two).
    let alone = server.plan(&w.square, "confirmed", 3600, &[&w.bo]);
    server.state.db.upsert_rsvp(&alone, &w.bo.id, "yes", Some("Scout"), "", now()).unwrap();
    assert_eq!(
        server.stand(&w.ann.key, &w.city, &w.bo.id).await["gatherings"],
        0,
        "one person is not a gathering"
    );

    // A second person's yes makes it one, and the bot's yes for Bo is Bo's.
    let together = server.plan(&w.square, "confirmed", 3600, &[&w.ann]);
    server.state.db.upsert_rsvp(&together, &w.bo.id, "yes", Some("Scout"), "", now()).unwrap();
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["gatherings"], 1);
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.ann.id).await["gatherings"], 1);

    // Still to come, cancelled, merely proposed, and in a house instead of the Town Square: none count.
    server.plan(&w.square, "confirmed", -3600, &[&w.ann, &w.bo]);
    server.plan(&w.square, "cancelled", 3600, &[&w.ann, &w.bo]);
    server.plan(&w.square, "proposed", 3600, &[&w.ann, &w.bo]);
    server.plan(&w.ann_home.living, "confirmed", 3600, &[&w.ann, &w.bo]);
    server.plan(&w.ann_home.porch, "confirmed", 3600, &[&w.ann, &w.bo]);
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["gatherings"], 1);
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.ann.id).await["gatherings"], 1);
}

// T2
#[tokio::test]
async fn a_gathering_that_has_not_happened_yet_counts_for_nobody_however_many_say_yes() {
    let server = Server::start().await;
    let w = server.world().await;
    // Through the real routes: Ann proposes a plan for tomorrow, and Bo and Ann say yes.
    let starts = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    let starts = starts.format(&time::format_description::well_known::Rfc3339).unwrap();
    let body = json!({ "title": PLAN_TITLE, "starts_at": starts });
    let proposed =
        server.post(&w.ann.key, &format!("/api/hangouts/{}/events", w.square), body).await;
    assert_ok(&proposed);
    let id = text(&proposed.body["id"]);
    assert_ok(
        &server
            .post(&w.ann.key, &format!("/api/events/{id}/rsvp"), json!({ "response": "yes" }))
            .await,
    );
    assert_ok(
        &server
            .post(&w.bo.key, &format!("/api/events/{id}/rsvp"), json!({ "response": "yes" }))
            .await,
    );
    for person in [&w.ann, &w.bo] {
        let standing = server.stand(&w.ann.key, &w.city, &person.id).await;
        assert_eq!(standing["gatherings"], 0, "a plan that hasn't happened is not a gathering");
    }
    assert_eq!(server.city_view(&w.ann.key, &w.city).await.body["growth"]["gatherings"], 0);
}

// T3
#[tokio::test]
async fn only_a_person_in_the_city_vouches_and_never_for_themselves_or_twice() {
    let server = Server::start().await;
    let w = server.world().await;
    let cy = server.join_city(&w.code, "Cy").await;
    let dan = server.join_city(&w.code, "Dan").await;
    server.require_vouches(&w.ann, &w.city, 2).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;

    // Never for yourself.
    assert_status(&server.vouch(&w.ann.key, &w.city, &w.ann.id).await, StatusCode::BAD_REQUEST);
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.ann.id).await["vouches"], json!([]));

    // Never by a bot, the bot of someone in the city included.
    assert_forbidden(&server.vouch(&w.scout.token, &w.city, &eve.id).await);
    let casa = server.house_bot(&w.ann, &w.ann_home, "Casa").await;
    assert_forbidden(&server.vouch(&casa.token, &w.city, &eve.id).await);
    assert_eq!(server.gate(&w.ann.key, &w.city).await[0]["vouches"], 0);

    // Once per person: asking again changes nothing, and two asks from Ann are not two vouches.
    let first = server.vouch(&w.ann.key, &w.city, &eve.id).await;
    assert_ok(&first);
    assert_eq!(
        (first.body["vouches"].as_i64(), first.body["let_in"].as_bool()),
        (Some(1), Some(false))
    );
    for _ in 0..4 {
        let again = server.vouch(&w.ann.key, &w.city, &eve.id).await;
        assert_ok(&again);
        assert_eq!(
            (again.body["vouches"].as_i64(), again.body["let_in"].as_bool()),
            (Some(1), Some(false))
        );
    }
    assert_eq!(server.me(&eve.key).await["waiting"][0]["vouches"], 1);
    assert_not_found(&server.get(&eve.key, &format!("/api/cities/{}", w.city)).await);

    // A different person is the second.
    let second = server.vouch(&cy.key, &w.city, &eve.id).await;
    assert_eq!(
        (second.body["vouches"].as_i64(), second.body["let_in"].as_bool()),
        (Some(2), Some(true))
    );

    // Taking a vouch back, which is only ever your own, and which can be done twice quietly.
    assert_ok(&server.vouch(&dan.key, &w.city, &w.bo.id).await);
    assert_ok(&server.vouch(&w.ann.key, &w.city, &w.bo.id).await);
    assert_ok(&server.unvouch(&w.ann.key, &w.city, &w.bo.id).await);
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["vouches"], json!(["Dan"]));
    assert_ok(&server.unvouch(&dan.key, &w.city, &w.bo.id).await);
    assert_ok(&server.unvouch(&dan.key, &w.city, &w.bo.id).await);
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["vouches"], json!([]));
    assert_forbidden(&server.unvouch(&w.scout.token, &w.city, &w.bo.id).await);
}

// T3
#[tokio::test]
async fn someone_at_the_gate_or_outside_the_city_cannot_vouch_for_anyone() {
    let server = Server::start().await;
    let w = server.world().await;
    server.require_vouches(&w.ann, &w.city, 1).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    let fay = server.at_the_gate(&w.code, "Fay").await;
    let (_, _, _, outsider) = server.new_city("Oak", "Gus").await;

    // The gate is not a place from which to vouch for the next person in the queue.
    assert_not_found(&server.vouch(&eve.key, &w.city, &fay.id).await);
    assert_not_found(&server.vouch(&fay.key, &w.city, &eve.id).await);
    assert_not_found(&server.vouch(&eve.key, &w.city, &eve.id).await);
    assert_not_found(&server.vouch(&outsider.key, &w.city, &eve.id).await);
    assert_not_found(&server.unvouch(&eve.key, &w.city, &fay.id).await);
    let waiting = server.gate(&w.ann.key, &w.city).await;
    assert_eq!(waiting.len(), 2);
    assert!(waiting.iter().all(|entry| entry["vouches"] == 0), "{waiting:?}");
    for who in [&eve, &fay] {
        assert_eq!(server.me(&who.key).await["waiting"][0]["vouches"], 0);
    }

    // And a member cannot vouch for someone the city doesn't know, so nobody can be vouched
    // before they've asked.
    assert_not_found(&server.vouch(&w.ann.key, &w.city, &outsider.id).await);
    assert_not_found(&server.vouch(&w.ann.key, &w.city, NO_SUCH_THING).await);
    assert_not_found(&server.vouch(&w.ann.key, NO_SUCH_THING, &eve.id).await);
}

// T3
#[tokio::test]
async fn a_vouch_from_someone_who_has_left_stops_counting_and_does_not_let_anyone_in_on_a_ghost() {
    let server = Server::start().await;
    let w = server.world().await;
    let cy = server.join_city(&w.code, "Cy").await;
    server.require_vouches(&w.ann, &w.city, 2).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;

    assert_ok(&server.vouch(&cy.key, &w.city, &eve.id).await);
    assert_eq!(server.gate(&w.ann.key, &w.city).await[0]["vouches"], 1);
    // Cy has no house, so Cy can leave the city.
    assert_ok(&server.post(&cy.key, &format!("/api/cities/{}/leave", w.city), json!({})).await);
    assert_eq!(
        server.gate(&w.ann.key, &w.city).await[0]["vouches"],
        0,
        "Cy's vouch no longer counts"
    );
    assert_eq!(server.me(&eve.key).await["waiting"][0]["vouches"], 0);

    // One more vouch is therefore one, not two.
    let one = server.vouch(&w.ann.key, &w.city, &eve.id).await;
    assert_eq!(
        (one.body["vouches"].as_i64(), one.body["let_in"].as_bool()),
        (Some(1), Some(false))
    );
    assert_not_found(&server.get(&eve.key, &format!("/api/cities/{}", w.city)).await);
    // Someone who left cannot vouch (they are not in the city), nor take the vouch back.
    assert_not_found(&server.vouch(&cy.key, &w.city, &eve.id).await);
    assert_not_found(&server.unvouch(&cy.key, &w.city, &eve.id).await);
}

// T17
#[tokio::test]
async fn a_voucher_coming_back_lets_in_whoever_that_was_enough_for() {
    let server = Server::start().await;
    let w = server.world().await;
    let cy = server.join_city(&w.code, "Cy").await;
    server.require_vouches(&w.ann, &w.city, 2).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    assert_ok(&server.vouch(&cy.key, &w.city, &eve.id).await);
    assert_ok(&server.post(&cy.key, &format!("/api/cities/{}/leave", w.city), json!({})).await);
    assert_ok(&server.vouch(&w.ann.key, &w.city, &eve.id).await);
    assert_not_found(&server.get(&eve.key, &format!("/api/cities/{}", w.city)).await);

    // Cy comes back by a house's link, so Cy's vouch counts again and that makes two.
    let back = format!("/api/house-invites/{}/join", server.house_code(&w.ann_home));
    assert_ok(&server.post(&cy.key, &back, json!({})).await);
    assert_ok(&server.get(&eve.key, &format!("/api/cities/{}", w.city)).await);
}

// T4
#[tokio::test]
async fn standing_never_carries_a_plan_title_a_trip_goal_a_house_or_bot_name_or_anything_said() {
    let server = Server::start().await;
    let w = server.world().await;
    server.gathering(&w.square, &[&w.ann, &w.bo]);
    let trip = server.arrive(&w.scout, &w.bo, &w.ann, &w.ann_home.id).await;
    assert_ok(&server.end_trip(&w.scout.token, &trip).await);
    let second = server.arrive(&w.scout, &w.bo, &w.ann, &w.ann_home.id).await;
    assert_ok(&server.end_trip(&w.ann.key, &second).await);
    let said = json!({ "text": CHAT_WORDS });
    assert_ok(&server.post(&w.bo.key, &format!("/api/hangouts/{}/messages", w.square), said).await);
    server.require(&w.ann, &w.ann_home, json!({ "min_gatherings": 1 })).await;
    assert_ok(&server.vouch(&w.ann.key, &w.city, &w.bo.id).await);

    for person in [&w.ann, &w.bo] {
        let standing = server.stand(&w.ann.key, &w.city, &person.id).await;
        assert_eq!(keys_of(&standing), STANDING_KEYS);
        for secret in
            [PLAN_TITLE, PLAN_PLACE, TRIP_GOAL, "Scout", "Ann House", "Bo House", CHAT_WORDS, "Elm"]
        {
            assert!(!has(&standing, secret), "{secret} in {standing}");
        }
        // The only text is the names of people who vouch.
        for name in standing["vouches"].as_array().unwrap() {
            assert!(["Ann", "Bo"].contains(&name.as_str().unwrap()), "{name}");
        }
    }
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["sent_home"], 1);
    // The same through a bot of the city.
    let by_bot = server.stand(&w.scout.token, &w.city, &w.bo.id).await;
    assert_eq!(keys_of(&by_bot), STANDING_KEYS);
    assert!(!has(&by_bot, TRIP_GOAL) && !has(&by_bot, PLAN_TITLE));
}

// T5
#[tokio::test]
async fn standing_is_for_people_in_the_city_and_their_bots_and_everyone_else_gets_the_404_a_missing_city_would(
) {
    let server = Server::start().await;
    let w = server.world().await;
    server.require_vouches(&w.ann, &w.city, 1).await;
    let gate = server.at_the_gate(&w.code, "Eve").await;
    let (oak, _, _, outsider) = server.new_city("Oak", "Gus").await;
    let oak_house = server.build_house(&outsider, &oak, "Oak House").await;
    let outsider_bot = server.bot(&outsider, &oak_house.living, "Wanderer").await;
    let casa = server.house_bot(&w.ann, &w.ann_home, "Casa").await;

    // The city's people and their bots may look, at members and at the gate.
    for token in [&w.ann.key, &w.bo.key, &w.scout.token] {
        for person in [&w.ann.id, &w.bo.id, &gate.id] {
            assert_ok(&server.standing(token, &w.city, person).await);
        }
    }
    // Anyone else gets exactly what a city that isn't there would say, whoever they ask about.
    let made_up = server.standing(&w.ann.key, NO_SUCH_THING, NO_SUCH_THING).await;
    assert_not_found(&made_up);
    for (who, token) in [
        ("a person in another city", &outsider.key),
        ("a bot in another city", &outsider_bot.token),
        ("someone at the gate", &gate.key),
    ] {
        for person in [&w.ann.id, &gate.id, &outsider.id, &NO_SUCH_THING.to_string()] {
            let reply = server.standing(token, &w.city, person).await;
            assert_eq!(reply.status, made_up.status, "{who} asking about {person}");
            assert_eq!(reply.body, made_up.body, "{who} asking about {person}");
        }
    }
    // The house bot has no business with any of it, and nobody who isn't signed in does either.
    assert_forbidden(&server.standing(&casa.token, &w.city, &w.bo.id).await);
    assert_status(&server.standing("", &w.city, &w.bo.id).await, StatusCode::UNAUTHORIZED);
    // The city's own people asking about someone it doesn't know are told the same thing.
    for person in [&outsider.id, &NO_SUCH_THING.to_string()] {
        let reply = server.standing(&w.ann.key, &w.city, person).await;
        assert_eq!((reply.status, &reply.body), (made_up.status, &made_up.body));
    }
}

// T5
#[tokio::test]
async fn the_bot_of_someone_at_the_gate_sees_nothing_of_the_city_either() {
    let server = Server::start().await;
    let w = server.world().await;
    // Eve has a bot, then leaves the city and comes back to a gate.
    let eve = server.join_city(&w.code, "Eve").await;
    let room = server.post(&eve.key, "/api/hangouts", json!({ "name": "Eve's own" })).await;
    let room = text(&room.body["hangout"]["id"]);
    let bot = server.bot(&eve, &room, "EveBot").await;
    assert_ok(&server.post(&eve.key, &format!("/api/cities/{}/leave", w.city), json!({})).await);
    server.require_vouches(&w.ann, &w.city, 1).await;
    let back = server.use_link(&w.code, &eve.key, json!({ "accept_rules": true })).await;
    assert_eq!(back.body["waiting"], true, "{}", back.body);

    let made_up = server.standing(&w.ann.key, NO_SUCH_THING, NO_SUCH_THING).await;
    let seen = server.standing(&bot.token, &w.city, &w.ann.id).await;
    assert_eq!((seen.status, seen.body), (made_up.status, made_up.body));
    assert_not_found(&server.city_view(&bot.token, &w.city).await);
    let listed = server.tool(&bot.token, "list_city", json!({ "city_id": w.city })).await;
    assert_eq!(listed["isError"], true, "{listed}");
    let asked = server
        .tool(&bot.token, "ask_for_trip", json!({ "house_id": w.ann_home.id, "kind": "call" }))
        .await;
    assert_eq!(asked["isError"], true, "{asked}");
    assert!(
        !has(&asked, "asks for"),
        "a bot at the gate learns nothing of the house's requirements: {asked}"
    );
}

// ----------------------------------------------------------------------------------------------
// House requirements (T6 to T11)
// ----------------------------------------------------------------------------------------------

impl Server {
    fn house_code(&self, home: &Home) -> String {
        self.state.db.house(&home.id).unwrap().unwrap().invite_code
    }

    /// A new person moves into the house through its link, accepting its city's rules if it has any.
    async fn move_in(&self, home: &Home, name: &str) -> Who {
        let path = format!("/api/house-invites/{}/join", self.house_code(home));
        let joined = self.post("", &path, json!({ "your_name": name, "accept_rules": true })).await;
        assert_eq!(joined.status, StatusCode::OK, "{}", joined.body);
        Who { key: text(&joined.body["owner_key"]), id: text(&joined.body["person"]["id"]) }
    }

    /// A new person joins the city and builds a house of their own.
    async fn neighbour(&self, w: &World, name: &str) -> (Who, Home) {
        let who = self.join_city(&w.code, name).await;
        let home = self.build_house(&who, &w.city, &format!("{name} House")).await;
        (who, home)
    }

    /// The trips waiting on an answer at this house, as its residents see them.
    async fn asking(&self, resident: &Who, house: &Home) -> Vec<Value> {
        let view = self.house_view(&resident.key, &house.id).await;
        assert_ok(&view);
        view.body["asking"].as_array().unwrap().clone()
    }
}

const NOTHING: Value = Value::Null;

// T6
#[tokio::test]
async fn a_resident_who_is_a_person_sets_what_the_house_asks_and_nobody_else_can() {
    let server = Server::start().await;
    let w = server.world().await;
    let partner = server.move_in(&w.ann_home, "Pat").await;
    let ann_bot = server.bot(&w.ann, &w.ann_home.living, "AnnBot").await;
    let casa = server.house_bot(&w.ann, &w.ann_home, "Casa").await;
    server.require_vouches(&w.ann, &w.city, 1).await;
    let gate = server.at_the_gate(&w.code, "Eve").await;
    let (oak, _, _, outsider) = server.new_city("Oak", "Gus").await;
    let oak_house = server.build_house(&outsider, &oak, "Oak House").await;
    let oak_bot = server.bot(&outsider, &oak_house.living, "Wanderer").await;
    server.require_vouches(&w.ann, &w.city, 0).await;

    let path = format!("/api/houses/{}/rules", w.ann_home.id);
    let fake = format!("/api/houses/{NO_SUCH_THING}/rules");
    let ask =
        json!({ "min_member_days": 7, "min_gatherings": 2, "min_good_trips": 1, "min_vouches": 1 });

    // Not a bot, not the house bot, not a neighbour, not a stranger, not a stranger's bot, and
    // not someone who isn't signed in: nothing changes, and a stranger can't tell the house exists.
    // A resident's own bot knows the house exists, so it is told only a person can do this.
    assert_forbidden(&server.put(&ann_bot.token, &path, ask.clone()).await);
    // The house bot is turned away from every house alike, so it learns nothing of this one.
    let refused = server.put(&casa.token, &path, ask.clone()).await;
    let on_nothing = server.put(&casa.token, &fake, ask.clone()).await;
    assert_forbidden(&refused);
    assert_eq!((refused.status, &refused.body), (on_nothing.status, &on_nothing.body));
    // A bot whose owner lives elsewhere can't tell the house from one that isn't there.
    let refused = server.put(&w.scout.token, &path, ask.clone()).await;
    let on_nothing = server.put(&w.scout.token, &fake, ask.clone()).await;
    assert_not_found(&refused);
    assert_eq!((refused.status, &refused.body), (on_nothing.status, &on_nothing.body));
    for (who, token) in [
        ("a neighbour", &w.bo.key),
        ("a stranger", &outsider.key),
        ("a stranger's bot", &oak_bot.token),
        ("someone at the gate", &gate.key),
    ] {
        let refused = server.put(token, &path, ask.clone()).await;
        let on_nothing = server.put(token, &fake, ask.clone()).await;
        assert_not_found(&refused);
        assert_eq!((refused.status, &refused.body), (on_nothing.status, &on_nothing.body), "{who}");
    }
    assert_status(&server.put("", &path, ask.clone()).await, StatusCode::UNAUTHORIZED);
    let view = server.house_view(&w.ann.key, &w.ann_home.id).await;
    assert_eq!(
        view.body["rules"],
        json!({ "min_member_days": 0, "min_gatherings": 0, "min_good_trips": 0, "min_vouches": 0 })
    );

    // Residents who are people can, and either one.
    assert_ok(&server.set_house_rules(&w.ann.key, &w.ann_home.id, ask.clone()).await);
    assert_eq!(server.house_view(&partner.key, &w.ann_home.id).await.body["rules"], ask);
    assert_ok(
        &server.set_house_rules(&partner.key, &w.ann_home.id, json!({ "min_gatherings": 3 })).await,
    );
    let now_asks = server.house_view(&w.ann.key, &w.ann_home.id).await.body["rules"].clone();
    assert_eq!(now_asks["min_gatherings"], 3);
    assert_eq!(now_asks["min_member_days"], 7, "a number left out stays as it was");

    // Everyone in the city sees it on the map, and so does a bot of theirs.
    for token in [&w.bo.key, &w.scout.token, &w.ann.key] {
        let city = server.city_view(token, &w.city).await;
        let house = city.body["houses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["id"] == w.ann_home.id.as_str())
            .unwrap()
            .clone();
        assert_eq!(house["requirements"]["min_gatherings"], 3);
    }
}

// T6
#[tokio::test]
async fn what_a_house_asks_has_limits_and_a_number_that_is_out_of_range_changes_nothing() {
    let server = Server::start().await;
    let w = server.world().await;
    for _ in 0..4 {
        server.join_city(&w.code, "Filler").await;
    }
    let house = &w.ann_home.id;

    // Exactly the largest each can be.
    let largest = json!({ "min_member_days": 365, "min_gatherings": 100, "min_good_trips": 100, "min_vouches": 5 });
    assert_ok(&server.set_house_rules(&w.ann.key, house, largest.clone()).await);
    assert_eq!(server.house_view(&w.ann.key, house).await.body["rules"], largest);
    assert_ok(&server.set_house_rules(&w.ann.key, house, json!({ "min_member_days": 0, "min_gatherings": 0, "min_good_trips": 0, "min_vouches": 0 })).await);

    // One past the largest, below zero, and the numbers that overflow: all refused, none applied.
    let steady =
        json!({ "min_member_days": 3, "min_gatherings": 2, "min_good_trips": 1, "min_vouches": 1 });
    assert_ok(&server.set_house_rules(&w.ann.key, house, steady.clone()).await);
    for (field, largest) in [
        ("min_member_days", 365i64),
        ("min_gatherings", 100),
        ("min_good_trips", 100),
        ("min_vouches", 5),
    ] {
        for bad in [
            json!(largest + 1),
            json!(-1),
            json!(i64::MAX),
            json!(i64::MIN),
            json!(u64::MAX),
            json!(1e30),
            json!(2.5),
            json!("7"),
            json!(true),
            json!([1]),
            json!({ "n": 1 }),
        ] {
            let refused =
                server.set_house_rules(&w.ann.key, house, json!({ field: bad.clone() })).await;
            assert_status(&refused, StatusCode::BAD_REQUEST);
            // Mixed with good numbers, the lot is refused.
            let mixed = server
                .set_house_rules(
                    &w.ann.key,
                    house,
                    json!({ "min_gatherings": 9, field: bad.clone() }),
                )
                .await;
            if field != "min_gatherings" {
                assert_status(&mixed, StatusCode::BAD_REQUEST);
            }
            assert_eq!(
                server.house_view(&w.ann.key, house).await.body["rules"],
                steady,
                "{field} = {bad}"
            );
        }
    }
    // Not the shape at all.
    for body in [NOTHING, json!([]), json!("rules"), json!(5)] {
        let refused = server.set_house_rules(&w.ann.key, house, body.clone()).await;
        assert!(refused.status.is_client_error(), "{body}: {}", refused.show());
    }
    let raw = server
        .send(
            &w.ann.key,
            Method::PUT,
            &format!("/api/houses/{house}/rules"),
            Some(b"{not json".to_vec()),
            Some("application/json"),
        )
        .await;
    assert_status(&raw, StatusCode::BAD_REQUEST);
    let no_type = server
        .send(
            &w.ann.key,
            Method::PUT,
            &format!("/api/houses/{house}/rules"),
            Some(b"{}".to_vec()),
            None,
        )
        .await;
    assert_status(&no_type, StatusCode::BAD_REQUEST);
    assert_eq!(server.house_view(&w.ann.key, house).await.body["rules"], steady);
}

// T7
#[tokio::test]
async fn a_bot_whose_owner_falls_short_is_told_what_is_missing_in_the_houses_terms_and_nothing_is_asked(
) {
    let server = Server::start().await;
    let w = server.world().await;
    server
        .require(&w.ann, &w.ann_home, json!({ "min_member_days": 7, "min_gatherings": 2, "min_good_trips": 1, "min_vouches": 1 }))
        .await;
    let ann_pings = server.notify_to(&w.ann).await;

    // The bot itself, its owner for it, and over MCP: the same refusal, the same words.
    let by_bot = server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await;
    let by_owner = server.ask_trip(&w.bo.key, Some(&w.scout.id), &w.ann_home.id, None).await;
    assert_status(&by_bot, StatusCode::CONFLICT);
    assert_status(&by_owner, StatusCode::CONFLICT);
    let said = text(&by_bot.body["error"]);
    assert_eq!(said, text(&by_owner.body["error"]));
    assert!(said.starts_with("Ann House asks for "), "{said}");
    for need in [
        "7 days in the city (Bo has 0)",
        "2 gatherings (Bo has 0)",
        "1 good trip (Bo has 0)",
        "1 vouch (Bo has 0)",
    ] {
        assert!(said.contains(need), "{need} missing from {said}");
    }
    let by_mcp = server
        .tool(&w.scout.token, "ask_for_trip", json!({ "house_id": w.ann_home.id, "kind": "call" }))
        .await;
    assert_eq!(by_mcp["isError"], true, "{by_mcp}");
    assert_eq!(by_mcp["content"][0]["text"], said.as_str());

    // Nothing was asked: no trip waiting, nobody pinged, the bot still at home.
    assert!(server.asking(&w.ann, &w.ann_home).await.is_empty());
    assert_eq!(server.get(&w.scout.token, "/api/me").await.body["trip"], Value::Null);
    assert!(
        all_notices(&ann_pings).await.is_empty(),
        "the house is not pinged for a request it would turn away"
    );
    // And the words are about the asker alone: no one else's numbers, no other house.
    assert!(!said.contains("Bo House") && !said.contains("Scout"), "{said}");
}

// T7
#[tokio::test]
async fn each_requirement_stops_an_ask_on_its_own_and_only_that_one_is_named() {
    let server = Server::start().await;
    let w = server.world().await;
    let kinds = [
        ("min_member_days", "day"),
        ("min_gatherings", "gathering"),
        ("min_good_trips", "good trip"),
        ("min_vouches", "vouch"),
    ];
    for (field, word) in kinds {
        server.require(&w.ann, &w.ann_home, json!({ field: 1 })).await;
        let refused = server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await;
        assert_status(&refused, StatusCode::CONFLICT);
        let said = text(&refused.body["error"]);
        assert!(
            said.contains(&format!("1 {word}")) && said.contains("Bo has 0"),
            "{field}: {said}"
        );
        for (_, other) in kinds.iter().filter(|(name, _)| *name != field) {
            if !word.contains(other) && !other.contains(word) {
                assert!(!said.contains(other), "{field} should not mention {other}: {said}");
            }
        }
        server.require(&w.ann, &w.ann_home, json!({ field: 0 })).await;
    }
    // With nothing asked for, the ask goes through.
    assert_ok(&server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await);
}

// T7
#[tokio::test]
async fn the_requirement_is_measured_on_the_bots_owner_and_each_number_counts_what_it_says() {
    let server = Server::start().await;
    let w = server.world().await;
    let dee = server.old_member(&w.city, "Dee", 10);
    let dee_home = server.build_house(&dee, &w.city, "Dee House").await;
    let dash = server.bot(&dee, &dee_home.living, "Dash").await;
    server.require(&w.ann, &w.ann_home, json!({ "min_member_days": 7 })).await;

    // Dee has been in the city for ten days. Bo, who joined a moment ago, has not.
    assert_ok(&server.ask_trip(&dash.token, None, &w.ann_home.id, None).await);
    assert_status(
        &server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await,
        StatusCode::CONFLICT,
    );
    // Days are whole days: 6 days and a bit is 6.
    let almost = server.old_member(&w.city, "Almost", 6);
    assert_eq!(server.stand(&w.ann.key, &w.city, &almost.id).await["member_days"], 6);
    assert_eq!(server.stand(&w.ann.key, &w.city, &dee.id).await["member_days"], 10);

    // Gatherings: a plan Dee said yes to counts for Dee.
    server.require(&w.ann, &w.ann_home, json!({ "min_member_days": 0, "min_gatherings": 1 })).await;
    let (eve, eve_home) = server.neighbour(&w, "Eve").await;
    let pip = server.bot(&eve, &eve_home.living, "Pip").await;
    assert_status(
        &server.ask_trip(&pip.token, None, &w.ann_home.id, None).await,
        StatusCode::CONFLICT,
    );
    server.gathering(&w.square, &[&eve, &w.ann]);
    assert_ok(&server.ask_trip(&pip.token, None, &w.ann_home.id, None).await);
    // The plan was Eve's and Ann's. Bo still has none of it.
    assert_status(
        &server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await,
        StatusCode::CONFLICT,
    );

    // Vouches: counts people in the city who vouch for the asker.
    server.require(&w.ann, &w.ann_home, json!({ "min_gatherings": 0, "min_vouches": 2 })).await;
    assert_ok(&server.vouch(&w.ann.key, &w.city, &w.bo.id).await);
    assert_status(
        &server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await,
        StatusCode::CONFLICT,
    );
    assert_ok(&server.vouch(&dee.key, &w.city, &w.bo.id).await);
    assert_ok(&server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await);
}

// T8
#[tokio::test]
async fn meeting_the_requirements_only_lets_you_ask_and_a_person_on_each_side_still_has_to_say_yes()
{
    let server = Server::start().await;
    let w = server.world().await;
    server.require(&w.ann, &w.ann_home, json!({ "min_vouches": 1 })).await;
    assert_ok(&server.vouch(&w.ann.key, &w.city, &w.bo.id).await);

    // Standing is enough to ask, by the bot or by its owner. Neither starts a trip.
    let by_bot = server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await;
    assert_ok(&by_bot);
    assert_eq!(by_bot.body["status"], "asking");
    assert!(
        by_bot.body["host_ok_by"].is_null() && by_bot.body["owner_ok_by"].is_null(),
        "{}",
        by_bot.body
    );
    let id = text(&by_bot.body["id"]);
    assert_eq!(
        server.get(&w.scout.token, "/api/me").await.body["trip"],
        Value::Null,
        "the bot is still at home"
    );

    // Not the resident's yes, not a bot's, and the owner's alone is not enough.
    assert_forbidden(&server.answer_trip(&w.scout.token, &id, "yes").await);
    let owner_yes = server.answer_trip(&w.bo.key, &id, "yes").await;
    assert_eq!(owner_yes.body["status"], "asking", "{}", owner_yes.body);
    assert_eq!(server.get(&w.scout.token, "/api/me").await.body["trip"], Value::Null);
    let ann_bot = server.bot(&w.ann, &w.ann_home.living, "AnnBot").await;
    assert_forbidden(&server.answer_trip(&ann_bot.token, &id, "yes").await);
    assert_eq!(server.get(&w.bo.key, &format!("/api/trips/{id}")).await.body["status"], "asking");
    let host_yes = server.answer_trip(&w.ann.key, &id, "yes").await;
    assert_eq!(host_yes.body["status"], "here", "{}", host_yes.body);

    // And when the owner asks for the bot, the house's yes is still to come.
    assert_ok(&server.end_trip(&w.scout.token, &id).await);
    let by_owner = server.ask_trip(&w.bo.key, Some(&w.scout.id), &w.ann_home.id, None).await;
    assert_ok(&by_owner);
    assert_eq!(by_owner.body["status"], "asking", "{}", by_owner.body);
    assert!(by_owner.body["host_ok_by"].is_null());
}

// T8
#[tokio::test]
async fn standing_that_far_exceeds_the_requirements_still_starts_nothing_on_its_own() {
    let server = Server::start().await;
    let w = server.world().await;
    let dee = server.old_member(&w.city, "Dee", 400);
    let dee_home = server.build_house(&dee, &w.city, "Dee House").await;
    let dash = server.bot(&dee, &dee_home.living, "Dash").await;
    for _ in 0..3 {
        server.gathering(&w.square, &[&dee, &w.ann]);
    }
    assert_ok(&server.vouch(&w.ann.key, &w.city, &dee.id).await);
    assert_ok(&server.vouch(&w.bo.key, &w.city, &dee.id).await);
    server
        .require(
            &w.ann,
            &w.ann_home,
            json!({ "min_member_days": 1, "min_gatherings": 1, "min_vouches": 1 }),
        )
        .await;
    let ask = server.ask_trip(&dash.token, None, &w.ann_home.id, None).await;
    assert_ok(&ask);
    assert_eq!(ask.body["status"], "asking");
    assert_eq!(server.get(&dash.token, "/api/me").await.body["trip"], Value::Null);
    assert!(
        server
            .get(&w.ann.key, &format!("/api/hangouts/{}/messages?after=0", w.ann_home.porch))
            .await
            .body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| !text(&m["body"]).contains("arrived")),
        "nobody arrived"
    );
}

// T9
#[tokio::test]
async fn a_resident_inviting_a_bot_skips_what_the_house_asks_but_the_owner_still_says_yes() {
    let server = Server::start().await;
    let w = server.world().await;
    server
        .require(
            &w.ann,
            &w.ann_home,
            json!({ "min_gatherings": 5, "min_vouches": 3, "min_member_days": 300 }),
        )
        .await;
    let (cy, _) = server.neighbour(&w, "Cy").await;
    let nobody = server.key_person("Nobody").await;

    // Asked for by the owner or the bot: turned away. Invited by a resident: through.
    assert_status(
        &server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await,
        StatusCode::CONFLICT,
    );
    assert_status(
        &server.ask_trip(&w.bo.key, Some(&w.scout.id), &w.ann_home.id, None).await,
        StatusCode::CONFLICT,
    );
    let invited = server.ask_trip(&w.ann.key, Some(&w.scout.id), &w.ann_home.id, None).await;
    assert_ok(&invited);
    assert_eq!(invited.body["status"], "asking", "an invitation is half of a trip");
    assert!(
        !invited.body["host_ok_by"].is_null() && invited.body["owner_ok_by"].is_null(),
        "{}",
        invited.body
    );
    let id = text(&invited.body["id"]);
    assert_eq!(server.get(&w.scout.token, "/api/me").await.body["trip"], Value::Null);
    assert_eq!(server.answer_trip(&w.bo.key, &id, "yes").await.body["status"], "here");

    // Only a resident of that very house can invite. Someone from the city, or from outside it, can't.
    assert_ok(&server.end_trip(&w.scout.token, &id).await);
    for who in [&cy, &nobody] {
        let refused = server.ask_trip(&who.key, Some(&w.scout.id), &w.ann_home.id, None).await;
        assert_not_found(&refused);
    }
    // And a bot can't invite on a resident's behalf: it asks for itself, and is measured.
    let ann_bot = server.bot(&w.ann, &w.ann_home.living, "AnnBot").await;
    let wrong = server.ask_trip(&ann_bot.token, Some(&w.scout.id), &w.ann_home.id, None).await;
    assert_not_found(&wrong);
}

// T9
#[tokio::test]
async fn an_accepted_pass_skips_what_the_house_asks_and_one_that_is_not_accepted_or_not_for_that_house_does_not(
) {
    let server = Server::start().await;
    let w = server.world().await;
    let (cy, cy_home) = server.neighbour(&w, "Cy").await;

    // Scout is hosted at Ann House once, with no requirements yet. Then both houses ask a lot.
    let first = server.arrive(&w.scout, &w.bo, &w.ann, &w.ann_home.id).await;
    assert_ok(&server.end_trip(&w.scout.token, &first).await);
    server.require(&w.ann, &w.ann_home, json!({ "min_gatherings": 5 })).await;
    server.require(&cy, &cy_home, json!({ "min_gatherings": 5 })).await;
    assert_status(
        &server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await,
        StatusCode::CONFLICT,
    );

    // A pass that the owner has not accepted changes nothing.
    let offered = server
        .post(
            &w.ann.key,
            &format!("/api/houses/{}/passes", w.ann_home.id),
            json!({ "agent_id": w.scout.id }),
        )
        .await;
    assert_ok(&offered);
    let pass = text(&offered.body["id"]);
    assert_status(
        &server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await,
        StatusCode::CONFLICT,
    );

    // Accepted, it covers a short trip to that house: the trip starts at once...
    assert_ok(&server.post(&w.bo.key, &format!("/api/passes/{pass}/accept"), json!({})).await);
    let covered = server.ask_trip(&w.scout.token, None, &w.ann_home.id, Some(30)).await;
    assert_ok(&covered);
    assert_eq!(covered.body["status"], "here", "{}", covered.body);
    assert_eq!(covered.body["pass_id"], pass.as_str());
    assert_ok(&server.end_trip(&w.scout.token, &text(&covered.body["id"])).await);
    // ...but not a longer one than the pass allows, and not a trip to another house.
    assert_status(
        &server.ask_trip(&w.scout.token, None, &w.ann_home.id, Some(120)).await,
        StatusCode::CONFLICT,
    );
    assert_status(
        &server.ask_trip(&w.scout.token, None, &cy_home.id, Some(30)).await,
        StatusCode::CONFLICT,
    );
    // Cancelled, it covers nothing.
    assert_ok(&server.delete(&w.ann.key, &format!("/api/passes/{pass}")).await);
    assert_status(
        &server.ask_trip(&w.scout.token, None, &w.ann_home.id, Some(30)).await,
        StatusCode::CONFLICT,
    );
}

// T9
#[tokio::test]
async fn changing_what_a_house_asks_never_ends_a_trip_or_a_pass() {
    let server = Server::start().await;
    let w = server.world().await;
    let trip = server.arrive(&w.scout, &w.bo, &w.ann, &w.ann_home.id).await;
    let pass = server.offer_and_accept_pass(&w.ann, &w.bo, &w.ann_home.id, &w.scout.id).await;

    server
        .require(&w.ann, &w.ann_home, json!({ "min_member_days": 365, "min_gatherings": 100, "min_good_trips": 100, "min_vouches": 5 }))
        .await;
    assert_eq!(server.get(&w.bo.key, &format!("/api/trips/{trip}")).await.body["status"], "here");
    let view = server.house_view(&w.ann.key, &w.ann_home.id).await;
    assert_eq!(view.body["guests"].as_array().unwrap().len(), 1);
    let passes = view.body["passes"].as_array().unwrap();
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0]["id"], pass.as_str());
    assert!(!passes[0]["accepted_by"].is_null());
    // The pass still works for the next trip once this one is over.
    assert_ok(&server.end_trip(&w.scout.token, &trip).await);
    let again = server.ask_trip(&w.scout.token, None, &w.ann_home.id, Some(30)).await;
    assert_eq!(again.body["status"], "here", "{}", again.body);
}

// T10
#[tokio::test]
async fn a_resident_sees_how_each_asker_stands_beside_their_request_and_nobody_else_sees_it() {
    let server = Server::start().await;
    let w = server.world().await;
    let (cy, cy_home) = server.neighbour(&w, "Cy").await;
    let pip = server.bot(&cy, &cy_home.living, "Pip").await;
    server.gathering(&w.square, &[&w.ann, &w.bo]);
    assert_ok(&server.vouch(&w.ann.key, &w.city, &w.bo.id).await);
    let partner = server.move_in(&w.ann_home, "Pat").await;

    let nobody_yet = server.house_view(&w.ann.key, &w.ann_home.id).await;
    assert_eq!(nobody_yet.body["standing"], json!({}), "no request, no standing shown");

    assert_ok(&server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await);
    assert_ok(&server.ask_trip(&pip.token, None, &w.ann_home.id, None).await);
    for resident in [&w.ann, &partner] {
        let view = server.house_view(&resident.key, &w.ann_home.id).await;
        let standing = view.body["standing"].as_object().unwrap();
        // Exactly the people with a request waiting: not Ann, not Pat, not everyone in the city.
        let mut shown: Vec<&String> = standing.keys().collect();
        shown.sort();
        let mut expected = vec![&w.bo.id, &cy.id];
        expected.sort();
        assert_eq!(shown, expected);
        let bo = &standing[&w.bo.id];
        assert_eq!(keys_of(bo), STANDING_KEYS);
        assert_eq!((bo["gatherings"].as_i64(), bo["vouches"].clone()), (Some(1), json!(["Ann"])));
        assert_eq!(standing[&cy.id]["gatherings"], 0);
        let said = without_ids(&view.body["standing"], &[&w.bo.id, &cy.id]);
        for secret in [PLAN_TITLE, TRIP_GOAL, "Scout", "Pip", "Bo House"] {
            assert!(!said.contains(secret), "{secret} in the standing map");
        }
    }
    // Only residents have the page.
    for token in [&w.bo.key, &cy.key, &w.scout.token] {
        assert_denied(&server.house_view(token, &w.ann_home.id).await);
    }
    // Once the request is answered, the asker's standing is no longer shown beside it.
    for trip in server.asking(&w.ann, &w.ann_home).await {
        assert_ok(&server.answer_trip(&w.ann.key, &text(&trip["id"]), "no").await);
    }
    assert_eq!(server.house_view(&w.ann.key, &w.ann_home.id).await.body["standing"], json!({}));
}

// T10
#[tokio::test]
async fn a_house_bot_never_sees_the_standing_of_an_asker_or_what_the_house_asks() {
    let server = Server::start().await;
    let w = server.world().await;
    let casa = server.house_bot(&w.ann, &w.ann_home, "Casa").await;
    server.require(&w.ann, &w.ann_home, json!({ "min_gatherings": 2 })).await;
    for (method, path, body) in [
        (Method::GET, format!("/api/houses/{}", w.ann_home.id), None),
        (Method::GET, format!("/api/cities/{}", w.city), None),
        (Method::GET, "/api/inbox".to_string(), None),
        (Method::GET, format!("/api/cities/{}/people/{}/standing", w.city, w.bo.id), None),
        (
            Method::PUT,
            format!("/api/houses/{}/rules", w.ann_home.id),
            Some(json!({ "min_gatherings": 0 })),
        ),
        (Method::PUT, format!("/api/cities/{}/rules", w.city), Some(json!({ "min_vouches": 1 }))),
        (Method::POST, format!("/api/cities/{}/people/{}/vouch", w.city, w.bo.id), Some(json!({}))),
        (Method::DELETE, format!("/api/cities/{}/people/{}/vouch", w.city, w.bo.id), None),
        (Method::POST, format!("/api/cities/{}/gate/{}/decline", w.city, w.bo.id), Some(json!({}))),
    ] {
        let reply = server.request(&casa.token, method.clone(), &path, body).await;
        if path == "/api/inbox" {
            assert!(!reply.status.is_server_error(), "{}", reply.show());
            assert!(
                !has(&reply.body, "min_gatherings")
                    && !has(&reply.body, "standing")
                    && !has(&reply.body, "requirements"),
                "{}",
                reply.body
            );
        } else {
            assert_forbidden(&reply);
        }
    }
    let me = server.get(&casa.token, "/api/me").await;
    assert!(
        !has(&me.body, "waiting") && !has(&me.body, "passkeys") && !has(&me.body, "requirements"),
        "{}",
        me.body
    );
    assert_eq!(server.tool(&casa.token, "list_city", json!({})).await["isError"], true);
    assert_eq!(
        server
            .tool(&casa.token, "ask_for_trip", json!({ "house_id": w.ann_home.id, "kind": "call" }))
            .await["isError"],
        true
    );
}

impl Server {
    async fn assert_sent_home(&self, w: &World, expected: i64) {
        let standing = self.stand(&w.ann.key, &w.city, &w.bo.id).await;
        assert_eq!(standing["sent_home"], expected, "{standing}");
    }
}

// T11
#[tokio::test]
async fn a_resident_sending_a_bot_home_is_recorded_and_nothing_else_that_ends_a_trip_is() {
    let server = Server::start().await;
    let w = server.world().await;
    let pat = server.move_in(&w.ann_home, "Pat").await;
    let ann_bot = server.bot(&w.ann, &w.ann_home.living, "AnnBot").await;
    let ann_house = &w.ann_home.id;

    // The bot going home, in the two ways it can.
    let t = server.arrive(&w.scout, &w.bo, &w.ann, ann_house).await;
    assert_ok(&server.end_trip(&w.scout.token, &t).await);
    server.assert_sent_home(&w, 0).await;
    let t = server.arrive(&w.scout, &w.bo, &w.ann, ann_house).await;
    let home = server.tool(&w.scout.token, "go_home", json!({})).await;
    assert_eq!(home["structuredContent"]["was_on_a_trip"], true, "{home}");
    assert_eq!(server.get(&w.bo.key, &format!("/api/trips/{t}")).await.body["status"], "ended");
    server.assert_sent_home(&w, 0).await;
    // Its owner calling it back.
    let t = server.arrive(&w.scout, &w.bo, &w.ann, ann_house).await;
    assert_ok(&server.end_trip(&w.bo.key, &t).await);
    server.assert_sent_home(&w, 0).await;
    // A resident's bot can't end it at all.
    let t = server.arrive(&w.scout, &w.bo, &w.ann, ann_house).await;
    assert_forbidden(&server.end_trip(&ann_bot.token, &t).await);
    assert_eq!(server.get(&w.bo.key, &format!("/api/trips/{t}")).await.body["status"], "here");
    // The owner turning on stays_home while the bot is away.
    let stay = json!({ "house_id": w.bo_home.id, "stays_home": true });
    assert_ok(&server.put(&w.bo.key, &format!("/api/agents/{}/home", w.scout.id), stay).await);
    assert_eq!(server.get(&w.bo.key, &format!("/api/trips/{t}")).await.body["status"], "ended");
    server.assert_sent_home(&w, 0).await;
    let free = json!({ "house_id": w.bo_home.id, "stays_home": false });
    assert_ok(&server.put(&w.bo.key, &format!("/api/agents/{}/home", w.scout.id), free).await);
    // Time running out, and a resident trying to end it afterwards.
    let t = server.arrive(&w.scout, &w.bo, &w.ann, ann_house).await;
    server.state.db.set_trip_ends_at(&t, now() - 5, now()).unwrap();
    assert_status(&server.end_trip(&w.ann.key, &t).await, StatusCode::CONFLICT);
    server.assert_sent_home(&w, 0).await;
    // An ask nobody answered, withdrawn by a resident, and one a resident said no to.
    let asked = server.ask_trip(&w.scout.token, None, ann_house, None).await;
    assert_ok(&server.end_trip(&w.ann.key, &text(&asked.body["id"])).await);
    let asked = server.ask_trip(&w.scout.token, None, ann_house, None).await;
    assert_ok(&server.answer_trip(&w.ann.key, &text(&asked.body["id"]), "no").await);
    server.assert_sent_home(&w, 0).await;

    // A pass being cancelled ends the trip it covers, by either side, and marks nothing.
    let pass = server.offer_and_accept_pass(&w.ann, &w.bo, ann_house, &w.scout.id).await;
    let covered = server.ask_trip(&w.scout.token, None, ann_house, Some(30)).await;
    assert_eq!(covered.body["status"], "here", "{}", covered.body);
    assert_ok(&server.delete(&w.ann.key, &format!("/api/passes/{pass}")).await);
    let covered_id = text(&covered.body["id"]);
    assert_eq!(
        server.get(&w.bo.key, &format!("/api/trips/{covered_id}")).await.body["status"],
        "ended"
    );
    let pass = server.offer_and_accept_pass(&w.ann, &w.bo, ann_house, &w.scout.id).await;
    let covered = server.ask_trip(&w.scout.token, None, ann_house, Some(30)).await;
    assert_eq!(covered.body["status"], "here", "{}", covered.body);
    assert_ok(&server.delete(&w.bo.key, &format!("/api/passes/{pass}")).await);
    let covered_id = text(&covered.body["id"]);
    assert_eq!(
        server.get(&w.bo.key, &format!("/api/trips/{covered_id}")).await.body["status"],
        "ended"
    );
    server.assert_sent_home(&w, 0).await;

    // A resident showing the bot out is. Either resident, and each trip only once.
    let t = server.arrive(&w.scout, &w.bo, &w.ann, ann_house).await;
    assert_ok(&server.end_trip(&w.ann.key, &t).await);
    server.assert_sent_home(&w, 1).await;
    assert_status(&server.end_trip(&w.ann.key, &t).await, StatusCode::CONFLICT);
    assert_status(&server.end_trip(&pat.key, &t).await, StatusCode::CONFLICT);
    server.assert_sent_home(&w, 1).await;
    let t = server.arrive(&w.scout, &w.bo, &w.ann, ann_house).await;
    assert_ok(&server.end_trip(&pat.key, &t).await);
    server.assert_sent_home(&w, 2).await;
}

// T11
#[tokio::test]
async fn a_trip_a_resident_ended_is_not_a_good_one_and_one_cut_short_by_the_owner_leaving_is_not_marked(
) {
    let server = Server::start().await;
    let w = server.world().await;
    let cy = server.join_city(&w.code, "Cy").await;
    let cy_room = text(
        &server.post(&cy.key, "/api/hangouts", json!({ "name": "Cy's own" })).await.body["hangout"]
            ["id"],
    );
    let pip = server.bot(&cy, &cy_room, "Pip").await;

    // Bo's only trip is one Ann cut short: no good trip, one sent home.
    let t = server.arrive(&w.scout, &w.bo, &w.ann, &w.ann_home.id).await;
    assert_ok(&server.end_trip(&w.ann.key, &t).await);
    let bo = server.stand(&w.ann.key, &w.city, &w.bo.id).await;
    assert_eq!((bo["good_trips"].as_i64(), bo["sent_home"].as_i64()), (Some(0), Some(1)), "{bo}");

    // Cy's bot visits and Cy leaves the city while it is away: the trip ends, and that is not Ann's doing.
    let t = server.arrive(&pip, &cy, &w.ann, &w.ann_home.id).await;
    assert_ok(&server.post(&cy.key, &format!("/api/cities/{}/leave", w.city), json!({})).await);
    assert!(!server.state.db.was_sent_home(&t).unwrap());
    let seen = server.get(&w.ann.key, &format!("/api/trips/{t}")).await;
    assert_eq!(seen.body["status"], "ended", "{}", seen.body);
}

// ----------------------------------------------------------------------------------------------
// City rules and the gate (T12 to T21)
// ----------------------------------------------------------------------------------------------

impl Server {
    /// A person who exists but has asked for nothing yet, with a gate place made `days_ago` days ago.
    fn gate_place_from(&self, city: &str, days_ago: i64, name: &str) -> Who {
        let key = auth::new_owner_key();
        let id = auth::new_id();
        self.state
            .db
            .create_person(&id, name, &auth::hash_secret(&key), &auth::new_feed_token(), now())
            .unwrap();
        let at = now() - days_ago * DAY;
        let placed = self.state.db.gate_ask(city, &id, at, at - 14 * DAY, 50).unwrap();
        assert_eq!(placed, agenthangout::db::GateAsk::Added);
        Who { key, id }
    }

    async fn city_rules_of(&self, token: &str, city: &str) -> Value {
        self.city_view(token, city).await.body["rules"].clone()
    }

    async fn town_square_says(&self, token: &str, square: &str) -> Vec<String> {
        let read =
            self.get(token, &format!("/api/hangouts/{square}/messages?after=0&limit=200")).await;
        assert_ok(&read);
        read.body["messages"].as_array().unwrap().iter().map(|m| text(&m["body"])).collect()
    }
}

// T12
#[tokio::test]
async fn the_person_who_started_the_city_sets_its_rules_and_nobody_else_does() {
    let server = Server::start().await;
    let w = server.world().await;
    server.join_city(&w.code, "Cy").await;
    let casa = server.house_bot(&w.ann, &w.ann_home, "Casa").await;
    let ann_bot = server.bot(&w.ann, &w.ann_home.living, "AnnBot").await;
    let (oak, _, _, outsider) = server.new_city("Oak", "Gus").await;
    let _ = oak;
    server.require_vouches(&w.ann, &w.city, 2).await;
    let gate = server.at_the_gate(&w.code, "Eve").await;
    let asked = json!({ "min_vouches": 1, "rules_text": "mine now" });
    let before = server.city_rules_of(&w.ann.key, &w.city).await;
    assert_eq!(before, json!({ "min_vouches": 2, "rules_text": "" }));

    // Another member, a bot (even the keeper's own), the house bot: refused, and nothing changes.
    assert_forbidden(&server.set_city_rules(&w.bo.key, &w.city, asked.clone()).await);
    assert_forbidden(&server.set_city_rules(&w.scout.token, &w.city, asked.clone()).await);
    assert_forbidden(&server.set_city_rules(&ann_bot.token, &w.city, asked.clone()).await);
    assert_forbidden(&server.set_city_rules(&casa.token, &w.city, asked.clone()).await);
    // Nobody who isn't in the city can tell it from one that isn't there.
    let nowhere = server.set_city_rules(&outsider.key, NO_SUCH_THING, asked.clone()).await;
    for token in [&outsider.key, &gate.key] {
        let reply = server.set_city_rules(token, &w.city, asked.clone()).await;
        assert_not_found(&reply);
        assert_eq!((reply.status, &reply.body), (nowhere.status, &nowhere.body));
    }
    assert_status(
        &server.set_city_rules("", &w.city, asked.clone()).await,
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(server.city_rules_of(&w.ann.key, &w.city).await, before);
    assert_eq!(server.get(&gate.key, "/api/me").await.body["waiting"][0]["vouches_needed"], 2);

    // The keeper can, and the other requirements stay at nothing.
    assert_ok(&server.set_city_rules(&w.ann.key, &w.city, asked.clone()).await);
    assert_eq!(
        server.city_rules_of(&w.bo.key, &w.city).await,
        json!({ "min_vouches": 1, "rules_text": "mine now" })
    );
    for field in ["min_member_days", "min_gatherings", "min_good_trips"] {
        let refused = server.set_city_rules(&w.ann.key, &w.city, json!({ field: 1 })).await;
        assert_status(&refused, StatusCode::BAD_REQUEST);
        assert_ok(&server.set_city_rules(&w.ann.key, &w.city, json!({ field: 0 })).await);
    }
    assert_eq!(
        server.city_rules_of(&w.bo.key, &w.city).await,
        json!({ "min_vouches": 1, "rules_text": "mine now" })
    );
    // Leaving a field out leaves it as it was.
    assert_ok(&server.set_city_rules(&w.ann.key, &w.city, json!({ "min_vouches": 2 })).await);
    assert_eq!(
        server.city_rules_of(&w.ann.key, &w.city).await,
        json!({ "min_vouches": 2, "rules_text": "mine now" })
    );
    assert_ok(&server.set_city_rules(&w.ann.key, &w.city, json!({ "rules_text": "" })).await);
    assert_eq!(
        server.city_rules_of(&w.ann.key, &w.city).await,
        json!({ "min_vouches": 2, "rules_text": "" })
    );
}

// T12
#[tokio::test]
async fn once_the_keeper_has_left_any_member_looks_after_the_rules_and_the_keeper_holds_them_again_on_returning(
) {
    let server = Server::start().await;
    let w = server.world().await;
    let pat = server.move_in(&w.ann_home, "Pat").await;
    // Ann leaves her house (Pat stays), then the city.
    assert_ok(
        &server.post(&w.ann.key, &format!("/api/houses/{}/leave", w.ann_home.id), json!({})).await,
    );
    assert_ok(&server.post(&w.ann.key, &format!("/api/cities/{}/leave", w.city), json!({})).await);

    // Now it is anyone's, so a link can never be left with nobody to replace it.
    assert_ok(
        &server
            .set_city_rules(
                &w.bo.key,
                &w.city,
                json!({ "min_vouches": 1, "rules_text": "Be kind." }),
            )
            .await,
    );
    assert_ok(&server.set_city_rules(&pat.key, &w.city, json!({ "min_vouches": 0 })).await);
    let view = server.city_view(&w.bo.key, &w.city).await;
    assert!(view.body["city"]["invite_url"].is_string(), "any member holds the link now");
    // Ann is out of it entirely.
    assert_not_found(
        &server.set_city_rules(&w.ann.key, &w.city, json!({ "min_vouches": 0 })).await,
    );
    assert_not_found(&server.decline(&w.ann.key, &w.city, &w.bo.id).await);

    // She comes back by the link, and the rules are hers to look after again.
    let again = server.use_link(&w.code, &w.ann.key, json!({ "accept_rules": true })).await;
    assert_ok(&again);
    assert_eq!(again.body["waiting"], false);
    assert_forbidden(&server.set_city_rules(&w.bo.key, &w.city, json!({ "min_vouches": 1 })).await);
    assert_ok(&server.set_city_rules(&w.ann.key, &w.city, json!({ "min_vouches": 1 })).await);
}

// T13
#[tokio::test]
async fn a_city_cannot_ask_for_more_vouches_than_it_has_people_or_more_than_five() {
    let server = Server::start().await;
    let (city, _, code, ann) = server.new_city("Elm", "Ann").await;
    // One person: the one vouch can only come from them.
    assert_status(
        &server.set_city_rules(&ann.key, &city, json!({ "min_vouches": 2 })).await,
        StatusCode::BAD_REQUEST,
    );
    assert_ok(&server.set_city_rules(&ann.key, &city, json!({ "min_vouches": 1 })).await);
    assert_ok(&server.set_city_rules(&ann.key, &city, json!({ "min_vouches": 0 })).await);

    server.join_city(&code, "Bo").await;
    server.join_city(&code, "Cy").await;
    // Three people.
    assert_status(
        &server.set_city_rules(&ann.key, &city, json!({ "min_vouches": 4 })).await,
        StatusCode::BAD_REQUEST,
    );
    assert_ok(&server.set_city_rules(&ann.key, &city, json!({ "min_vouches": 3 })).await);
    assert_eq!(server.city_rules_of(&ann.key, &city).await["min_vouches"], 3);

    // Plenty of people, still no more than five.
    assert_ok(&server.set_city_rules(&ann.key, &city, json!({ "min_vouches": 0 })).await);
    for n in 0..8 {
        server.join_city(&code, &format!("Filler {n}")).await;
    }
    assert_ok(&server.set_city_rules(&ann.key, &city, json!({ "min_vouches": 5 })).await);
    for bad in [
        json!(6),
        json!(7),
        json!(-1),
        json!(i64::MAX),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(1e30),
        json!(2.5),
        json!("2"),
        json!(true),
        json!([2]),
        json!({ "n": 2 }),
    ] {
        let refused =
            server.set_city_rules(&ann.key, &city, json!({ "min_vouches": bad.clone() })).await;
        assert_status(&refused, StatusCode::BAD_REQUEST);
        assert_eq!(server.city_rules_of(&ann.key, &city).await["min_vouches"], 5, "{bad}");
    }
    // The same for the other three, which can only ever be 0.
    for field in ["min_member_days", "min_gatherings", "min_good_trips"] {
        for bad in [
            json!(1),
            json!(-1),
            json!(i64::MAX),
            json!(i64::MIN),
            json!(2.5),
            json!("0"),
            json!(1e30),
        ] {
            let refused =
                server.set_city_rules(&ann.key, &city, json!({ field: bad.clone() })).await;
            assert!(refused.status.is_client_error(), "{field} = {bad}: {}", refused.show());
        }
    }
}

// T13 (Limits table)
#[tokio::test]
async fn rules_text_is_at_most_a_thousand_characters_and_whatever_it_holds_stays_text() {
    let server = Server::start().await;
    let (city, _, code, ann) = server.new_city("Elm", "Ann").await;
    let longest = "x".repeat(1000);
    assert_ok(&server.set_city_rules(&ann.key, &city, json!({ "rules_text": longest })).await);
    assert_eq!(
        text(&server.city_rules_of(&ann.key, &city).await["rules_text"]).chars().count(),
        1000
    );
    assert_status(
        &server.set_city_rules(&ann.key, &city, json!({ "rules_text": "x".repeat(1001) })).await,
        StatusCode::BAD_REQUEST,
    );
    // A thousand characters is a thousand characters, however many bytes each is.
    let wide = "é".repeat(1000);
    assert_ok(&server.set_city_rules(&ann.key, &city, json!({ "rules_text": wide })).await);
    let emoji = "🙂".repeat(1000);
    assert_ok(&server.set_city_rules(&ann.key, &city, json!({ "rules_text": emoji })).await);
    assert_status(
        &server.set_city_rules(&ann.key, &city, json!({ "rules_text": "🙂".repeat(1001) })).await,
        StatusCode::BAD_REQUEST,
    );
    // Not text at all.
    for bad in [json!(5), json!(true), json!(["a"]), json!({ "a": 1 })] {
        let refused =
            server.set_city_rules(&ann.key, &city, json!({ "rules_text": bad.clone() })).await;
        assert_status(&refused, StatusCode::BAD_REQUEST);
    }
    // Whatever it says is handed back as it was written, trimmed, and as text.
    let markup = "  <script>alert(1)</script> & \"quotes\" \\ \n new line  ";
    assert_ok(&server.set_city_rules(&ann.key, &city, json!({ "rules_text": markup })).await);
    let preview = server.get("", &format!("/api/city-invites/{code}")).await;
    assert_eq!(preview.body["rules_text"], markup.trim());
    // Blank clears it.
    assert_ok(&server.set_city_rules(&ann.key, &city, json!({ "rules_text": "   \n  " })).await);
    assert_eq!(server.get("", &format!("/api/city-invites/{code}")).await.body["rules_text"], "");
}

// T14
#[tokio::test]
async fn the_invite_page_shows_the_rules_and_the_vouches_needed_and_joining_needs_them_accepted() {
    let server = Server::start().await;
    let w = server.world().await;
    let plain = server.get("", &format!("/api/city-invites/{}", w.code)).await;
    assert_eq!(keys_of(&plain.body), ["name", "people", "rules_text", "vouches_needed"]);
    assert_eq!(
        (plain.body["rules_text"].as_str(), plain.body["vouches_needed"].as_i64()),
        (Some(""), Some(0))
    );

    assert_ok(
        &server
            .set_city_rules(
                &w.ann.key,
                &w.city,
                json!({ "min_vouches": 2, "rules_text": "Be kind. Ask first." }),
            )
            .await,
    );
    let shown = server.get("", &format!("/api/city-invites/{}", w.code)).await;
    assert_eq!(shown.body["rules_text"], "Be kind. Ask first.");
    assert_eq!(shown.body["vouches_needed"], 2);
    assert_eq!(keys_of(&shown.body), ["name", "people", "rules_text", "vouches_needed"]);

    // Nobody signed in, no `accept_rules`: 409, and no account is left behind.
    let people = server.state.db.person_count().unwrap();
    for body in [
        json!({ "your_name": "Eve" }),
        json!({ "your_name": "Eve", "accept_rules": false }),
        json!({ "your_name": "Eve", "accept_rules": null }),
    ] {
        let refused = server.use_link(&w.code, "", body.clone()).await;
        assert_status(&refused, StatusCode::CONFLICT);
        assert!(refused.body["error"].as_str().unwrap().contains("rules"), "{}", refused.body);
        assert!(
            refused.set_cookie.is_none() && refused.body.get("owner_key").is_none(),
            "{}",
            refused.body
        );
    }
    assert_eq!(server.state.db.person_count().unwrap(), people, "a refused join makes no account");
    assert!(server.gate(&w.ann.key, &w.city).await.is_empty());
    // Not the right type is not a yes either.
    for body in [
        json!({ "your_name": "Eve", "accept_rules": "true" }),
        json!({ "your_name": "Eve", "accept_rules": 1 }),
    ] {
        let refused = server.use_link(&w.code, "", body).await;
        assert!(refused.status.is_client_error(), "{}", refused.show());
    }
    assert_eq!(server.state.db.person_count().unwrap(), people);

    // Someone with an account, signed in, is asked the same.
    let cy = server.key_person("Cy").await;
    assert_status(&server.use_link(&w.code, &cy.key, json!({})).await, StatusCode::CONFLICT);
    assert!(
        server.gate(&w.ann.key, &w.city).await.is_empty(),
        "refused people don't reach the gate"
    );
    assert_eq!(server.me(&cy.key).await["waiting"], json!([]));
    let accepted = server.use_link(&w.code, &cy.key, json!({ "accept_rules": true })).await;
    assert_ok(&accepted);
    assert_eq!(
        (accepted.body["waiting"].as_bool(), accepted.body["vouches_needed"].as_i64()),
        (Some(true), Some(2))
    );

    // Someone already in doesn't have to accept anything to use the link again.
    assert_ok(&server.use_link(&w.code, &w.bo.key, json!({})).await);
}

// T14
// Rules are for newcomers: someone already at the gate when they change isn't asked again, and
// someone arriving after is.
#[tokio::test]
async fn rules_added_after_someone_reached_the_gate_are_not_asked_of_them_again_but_a_newcomer_is_asked(
) {
    let server = Server::start().await;
    let w = server.world().await;
    let cy = server.join_city(&w.code, "Cy").await;
    server.require_vouches(&w.ann, &w.city, 2).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    assert_ok(&server.vouch(&w.ann.key, &w.city, &eve.id).await);
    assert_ok(
        &server
            .set_city_rules(&w.ann.key, &w.city, json!({ "rules_text": "Read this first." }))
            .await,
    );

    // She uses the link again without accepting anything, and isn't turned back.
    assert_eq!(server.use_link(&w.code, &eve.key, json!({})).await.body["waiting"], true);
    // The second vouch lets her in.
    let second = server.vouch(&cy.key, &w.city, &eve.id).await;
    assert_eq!(second.body["let_in"], true, "{}", second.body);
    assert_ok(&server.city_view(&eve.key, &w.city).await);

    // Someone arriving now is asked, and is refused until they accept.
    let people = server.state.db.person_count().unwrap();
    let asked = server.use_link(&w.code, "", json!({ "your_name": "Fay" })).await;
    assert_status(&asked, StatusCode::CONFLICT);
    assert_eq!(server.state.db.person_count().unwrap(), people, "no account left behind");
    let fay =
        server.use_link(&w.code, "", json!({ "your_name": "Fay", "accept_rules": true })).await;
    assert_eq!(fay.body["waiting"], true, "{}", fay.body);
}

// T14
#[tokio::test]
async fn someone_at_the_gate_is_not_asked_about_new_rules_on_a_house_link_either() {
    let server = Server::start().await;
    let w = server.world().await;
    server.require_vouches(&w.ann, &w.city, 2).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    assert_ok(
        &server.set_city_rules(&w.ann.key, &w.city, json!({ "rules_text": "Be kind." })).await,
    );
    let path = format!("/api/house-invites/{}/join", server.house_code(&w.ann_home));
    assert_ok(&server.post(&eve.key, &path, json!({})).await);
}

// T14
// R5 says joining a house also joins its city, and T14 says joining a city with rules needs them
// accepted, so a house link carries `accept_rules` too, for anyone who isn't in the city yet.
#[tokio::test]
async fn joining_a_house_into_a_city_with_rules_needs_the_rules_accepted_too() {
    let server = Server::start().await;
    let w = server.world().await;
    let rules = "Adults only. Ask before you send a bot.";
    assert_ok(&server.set_city_rules(&w.ann.key, &w.city, json!({ "rules_text": rules })).await);
    let code = server.house_code(&w.ann_home);
    let path = format!("/api/house-invites/{code}/join");
    let preview = server.get("", &format!("/api/house-invites/{code}")).await;
    assert_eq!(preview.body["rules_text"], rules, "the page can show them");
    assert_eq!(keys_of(&preview.body), ["city", "name", "people", "rules_text"]);

    // Someone new, without accepting: refused, no account made, and the link is still good.
    let people = server.state.db.person_count().unwrap();
    for body in [
        json!({ "your_name": "Newcomer" }),
        json!({ "your_name": "Newcomer", "accept_rules": false }),
    ] {
        let refused = server.post("", &path, body).await;
        assert_status(&refused, StatusCode::CONFLICT);
        assert!(refused.body["error"].as_str().unwrap().contains("rules"), "{}", refused.body);
        assert!(
            refused.set_cookie.is_none() && refused.body.get("owner_key").is_none(),
            "{}",
            refused.body
        );
    }
    assert_eq!(server.state.db.person_count().unwrap(), people, "no account left behind");
    assert!(server
        .post("", &path, json!({ "your_name": "Newcomer", "accept_rules": "yes" }))
        .await
        .status
        .is_client_error());

    // Someone with an account who isn't in the city is asked the same, and stays outside until they accept.
    let cy = server.key_person("Cy").await;
    assert_status(&server.post(&cy.key, &path, json!({})).await, StatusCode::CONFLICT);
    assert_eq!(server.me(&cy.key).await["houses"], json!([]));
    assert_not_found(&server.city_view(&cy.key, &w.city).await);
    assert_ok(&server.post(&cy.key, &path, json!({ "accept_rules": true })).await);
    assert_ok(&server.city_view(&cy.key, &w.city).await);

    // Someone already in the city has no need to accept anything again.
    let fresh = server.house_code(&w.ann_home);
    assert_ne!(fresh, code, "a link works once");
    let dee = server.old_member(&w.city, "Dee", 3);
    assert_ok(&server.post(&dee.key, &format!("/api/house-invites/{fresh}/join"), json!({})).await);
}

// T15
#[tokio::test]
async fn with_vouches_asked_for_the_link_leads_to_the_gate_and_the_city_stays_a_404() {
    let server = Server::start().await;
    let w = server.world().await;
    server.require_vouches(&w.ann, &w.city, 2).await;

    let arrived =
        server.use_link(&w.code, "", json!({ "your_name": "Eve", "accept_rules": true })).await;
    assert_ok(&arrived);
    assert_eq!(arrived.body["waiting"], true);
    assert_eq!(
        (arrived.body["vouches"].as_i64(), arrived.body["vouches_needed"].as_i64()),
        (Some(0), Some(2))
    );
    assert_eq!(arrived.body["city_id"], w.city.as_str());
    // She has an account, a key and a session, and a name.
    assert!(arrived.body["owner_key"].as_str().unwrap().starts_with("ah_owner_"));
    assert!(arrived.set_cookie.as_deref().unwrap().contains("ah_session=ah_sess_"));
    let eve =
        Who { key: text(&arrived.body["owner_key"]), id: text(&arrived.body["person"]["id"]) };
    assert_eq!(server.me(&eve.key).await["name"], "Eve");

    // But the city is a 404, the Town Square is a 404, and so is everything in it, to her and her bots.
    let nowhere = server.get(&eve.key, &format!("/api/cities/{NO_SUCH_THING}")).await;
    let city = server.city_view(&eve.key, &w.city).await;
    assert_eq!((city.status, &city.body), (nowhere.status, &nowhere.body));
    let stream = server.stream(&eve.key, &format!("/api/cities/{}/stream", w.city)).await;
    assert_eq!(stream.status(), StatusCode::NOT_FOUND);
    for path in [
        format!("/api/hangouts/{}", w.square),
        format!("/api/hangouts/{}/messages?after=0", w.square),
        format!("/api/hangouts/{}/stream", w.square),
        format!("/api/houses/{}", w.ann_home.id),
        format!("/api/cities/{}/people/{}/standing", w.city, w.ann.id),
    ] {
        assert_not_found(&server.get(&eve.key, &path).await);
    }
    assert_not_found(
        &server
            .post(
                &eve.key,
                &format!("/api/hangouts/{}/messages", w.square),
                json!({ "text": "let me in" }),
            )
            .await,
    );
    assert_not_found(
        &server
            .post(&eve.key, &format!("/api/cities/{}/houses", w.city), json!({ "name": "Mine" }))
            .await,
    );
    assert_eq!(server.me(&eve.key).await["cities"], json!([]));
    // A person in the city already is not sent to the gate by a rule that came later.
    let again = server.use_link(&w.code, &w.bo.key, json!({})).await;
    assert_eq!(again.body["waiting"], false);
    assert_ok(&server.city_view(&w.bo.key, &w.city).await);
}

// T16
#[tokio::test]
async fn the_gate_is_seen_by_people_in_the_city_and_not_by_their_bots_nor_the_people_waiting() {
    let server = Server::start().await;
    let w = server.world().await;
    let casa = server.house_bot(&w.ann, &w.ann_home, "Casa").await;
    server.require_vouches(&w.ann, &w.city, 2).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    let fay = server.at_the_gate(&w.code, "Fay").await;
    assert_ok(&server.vouch(&w.bo.key, &w.city, &eve.id).await);

    for member in [&w.ann, &w.bo] {
        let gate = server.gate(&member.key, &w.city).await;
        assert_eq!(gate.len(), 2);
        for entry in &gate {
            // Name, colour, how many vouch, and when they asked. Nothing that is theirs alone.
            let allowed = ["asked_at", "color", "name", "person_id", "vouches", "you_vouched"];
            assert_eq!(keys_of(entry), allowed, "{entry}");
            assert!(
                entry["asked_at"].as_i64().unwrap() > 0
                    && entry["color"].as_str().unwrap().starts_with('#')
            );
        }
        let mine = gate.iter().find(|entry| entry["name"] == "Eve").unwrap();
        assert_eq!(mine["vouches"], 1);
        assert_eq!(mine["you_vouched"], member.id == w.bo.id);
    }
    // Their bots don't see it, in the page's reply or through MCP.
    let by_bot = server.city_view(&w.scout.token, &w.city).await;
    assert_ok(&by_bot);
    assert!(by_bot.body.get("gate").is_none(), "{}", by_bot.body);
    let mut people: Vec<String> =
        by_bot.body["people"].as_array().unwrap().iter().map(|p| text(&p["name"])).collect();
    people.sort();
    assert_eq!(people, ["Ann", "Bo"], "the people waiting are not people in the city");
    let listed = server.tool(&w.scout.token, "list_city", json!({})).await;
    assert!(listed["structuredContent"].get("gate").is_none(), "{listed}");
    let homes = [&w.ann_home.id, &w.bo_home.id];
    assert!(
        !without_ids(&listed["structuredContent"]["bots"], &homes).contains("Eve")
            && !without_ids(&listed["structuredContent"]["houses"], &homes).contains("Eve"),
        "{listed}"
    );
    // The house bot has no city at all, and the people waiting can't see who else is.
    assert_forbidden(&server.city_view(&casa.token, &w.city).await);
    assert_not_found(&server.city_view(&eve.key, &w.city).await);
    assert_not_found(&server.city_view(&fay.key, &w.city).await);
    // And none of it is said aloud in the Town Square.
    for line in server.town_square_says(&w.ann.key, &w.square).await {
        assert!(!line.contains("Eve") && !line.contains("Fay") && !line.contains("gate"), "{line}");
    }
}

// T17
#[tokio::test]
async fn enough_vouches_let_someone_in_at_once_and_they_are_pinged_and_nobody_else_is() {
    let server = Server::start().await;
    let w = server.world().await;
    let cy = server.join_city(&w.code, "Cy").await;
    server.require_vouches(&w.ann, &w.city, 2).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    let eve_pings = server.notify_to(&eve).await;
    let ann_pings = server.notify_to(&w.ann).await;
    let bo_pings = server.notify_to(&w.bo).await;

    let first = server.vouch(&w.ann.key, &w.city, &eve.id).await;
    assert_eq!(first.body["let_in"], false);
    assert!(notices_of(&eve_pings, "city.let_in").await.is_empty(), "one of two is not in");
    let second = server.vouch(&w.bo.key, &w.city, &eve.id).await;
    assert_eq!(
        (second.body["let_in"].as_bool(), second.body["vouches"].as_i64()),
        (Some(true), Some(2))
    );

    // At once: the city, the Town Square and the people in it are hers now.
    assert_ok(&server.city_view(&eve.key, &w.city).await);
    assert_ok(&server.get(&eve.key, &format!("/api/hangouts/{}", w.square)).await);
    let me = server.me(&eve.key).await;
    assert_eq!((me["waiting"].clone(), me["cities"][0]["id"].clone()), (json!([]), json!(w.city)));
    assert!(server.gate(&w.ann.key, &w.city).await.is_empty());
    assert_eq!(server.stand(&w.ann.key, &w.city, &eve.id).await["member_days"], 0);
    assert!(server.stand(&w.ann.key, &w.city, &eve.id).await["member_since"].is_i64());

    // Pinged once; the Town Square is told once, and the others are not pinged for it.
    let let_in = expect_notices(&eve_pings, "city.let_in", 1).await;
    assert_eq!(let_in.len(), 1, "{let_in:?}");
    assert!(text(&let_in[0]["text"]).contains("Elm"));
    assert!(notices_of(&ann_pings, "city.let_in").await.is_empty());
    assert!(notices_of(&bo_pings, "city.let_in").await.is_empty());
    let joined: Vec<_> = server
        .town_square_says(&w.ann.key, &w.square)
        .await
        .into_iter()
        .filter(|l| l.contains("Eve"))
        .collect();
    assert_eq!(joined, ["Eve joined Elm."]);
    // Asking again, or a third vouch, does nothing more.
    assert_ok(&server.use_link(&w.code, &eve.key, json!({})).await);
    assert_eq!(server.vouch(&cy.key, &w.city, &eve.id).await.body["let_in"], false);
    assert_eq!(expect_notices(&eve_pings, "city.let_in", 1).await.len(), 1);
}

// T17
#[tokio::test]
async fn only_the_keeper_of_the_link_turns_someone_away_and_it_clears_their_vouches() {
    let server = Server::start().await;
    let w = server.world().await;
    let cy = server.join_city(&w.code, "Cy").await;
    let casa = server.house_bot(&w.ann, &w.ann_home, "Casa").await;
    let (_, _, _, outsider) = server.new_city("Oak", "Gus").await;
    server.require_vouches(&w.ann, &w.city, 3).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    let fay = server.at_the_gate(&w.code, "Fay").await;
    assert_ok(&server.vouch(&w.bo.key, &w.city, &eve.id).await);
    assert_ok(&server.vouch(&cy.key, &w.city, &eve.id).await);

    // Not another member, a bot (the keeper's own included), the house bot, someone waiting.
    for token in [&w.bo.key, &cy.key] {
        assert_forbidden(&server.decline(token, &w.city, &eve.id).await);
    }
    let ann_bot = server.bot(&w.ann, &w.ann_home.living, "AnnBot").await;
    for token in [&w.scout.token, &ann_bot.token, &casa.token] {
        assert_forbidden(&server.decline(token, &w.city, &eve.id).await);
    }
    let nowhere = server.decline(&outsider.key, NO_SUCH_THING, &eve.id).await;
    for token in [&outsider.key, &fay.key, &eve.key] {
        let reply = server.decline(token, &w.city, &eve.id).await;
        assert_eq!((reply.status, &reply.body), (nowhere.status, &nowhere.body));
    }
    assert_eq!(server.gate(&w.ann.key, &w.city).await.len(), 2);
    assert_eq!(server.me(&eve.key).await["waiting"][0]["vouches"], 2);

    // The keeper can, but only someone who is waiting: a member is not turned out of the city.
    assert_not_found(&server.decline(&w.ann.key, &w.city, &w.bo.id).await);
    assert_not_found(&server.decline(&w.ann.key, &w.city, &outsider.id).await);
    assert_not_found(&server.decline(&w.ann.key, &w.city, NO_SUCH_THING).await);
    assert_ok(&server.city_view(&w.bo.key, &w.city).await);
    assert_ok(&server.decline(&w.ann.key, &w.city, &eve.id).await);
    assert_not_found(&server.decline(&w.ann.key, &w.city, &eve.id).await);

    // She is gone from the gate and has nothing waiting, and her vouches went with her place.
    assert_eq!(server.gate(&w.ann.key, &w.city).await.len(), 1);
    assert_eq!(server.me(&eve.key).await["waiting"], json!([]));
    assert_not_found(&server.vouch(&w.ann.key, &w.city, &eve.id).await);
    assert_eq!(
        server.state.db.vouch_count(&w.city, &eve.id).unwrap(),
        0,
        "the vouches are gone, not just hidden"
    );
    assert_not_found(&server.city_view(&eve.key, &w.city).await);
}

// T17
#[tokio::test]
async fn someone_turned_away_starts_again_from_nothing_and_a_new_link_keeps_them_out() {
    let server = Server::start().await;
    let w = server.world().await;
    let cy = server.join_city(&w.code, "Cy").await;
    server.require_vouches(&w.ann, &w.city, 3).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    assert_ok(&server.vouch(&w.bo.key, &w.city, &eve.id).await);
    assert_ok(&server.vouch(&cy.key, &w.city, &eve.id).await);
    assert_ok(&server.decline(&w.ann.key, &w.city, &eve.id).await);

    // The old vouchers' vouches don't come back with her: she needs all three again.
    let back = server.use_link(&w.code, &eve.key, json!({ "accept_rules": true })).await;
    assert_eq!(
        (back.body["waiting"].as_bool(), back.body["vouches"].as_i64()),
        (Some(true), Some(0)),
        "{}",
        back.body
    );
    let one = server.vouch(&w.bo.key, &w.city, &eve.id).await;
    assert_eq!(
        (one.body["vouches"].as_i64(), one.body["let_in"].as_bool()),
        (Some(1), Some(false))
    );
    // Walking back in by asking is the same as the first time: nothing is kept against her, and no further vouch is owed.
    assert_ok(&server.decline(&w.ann.key, &w.city, &eve.id).await);

    // Replacing the link is how to keep someone out: the old one is a 404.
    let rotated =
        server.post(&w.ann.key, &format!("/api/cities/{}/invite/rotate", w.city), json!({})).await;
    assert_ok(&rotated);
    let new_code = code_of(&rotated.body["invite_url"]);
    assert_not_found(&server.use_link(&w.code, &eve.key, json!({ "accept_rules": true })).await);
    assert_not_found(
        &server
            .use_link(&w.code, "", json!({ "your_name": "Eve again", "accept_rules": true }))
            .await,
    );
    assert_eq!(server.me(&eve.key).await["waiting"], json!([]));
    assert_eq!(
        server.get("", &format!("/api/city-invites/{}", w.code)).await.status,
        StatusCode::NOT_FOUND
    );
    assert_ok(&server.get("", &format!("/api/city-invites/{new_code}")).await);
}

// T17
#[tokio::test]
async fn only_the_keeper_clears_the_whole_gate_at_once_and_every_vouch_for_those_waiting_goes_with_it(
) {
    let server = Server::start().await;
    let w = server.world().await;
    let cy = server.join_city(&w.code, "Cy").await;
    let casa = server.house_bot(&w.ann, &w.ann_home, "Casa").await;
    let ann_bot = server.bot(&w.ann, &w.ann_home.living, "AnnBot").await;
    let (_, _, _, outsider) = server.new_city("Oak", "Gus").await;
    server.require_vouches(&w.ann, &w.city, 3).await;
    let stale = server.gate_place_from(&w.city, 15, "Stale");
    let eve = server.at_the_gate(&w.code, "Eve").await;
    let fay = server.at_the_gate(&w.code, "Fay").await;
    assert_ok(&server.vouch(&w.bo.key, &w.city, &eve.id).await);
    assert_ok(&server.vouch(&cy.key, &w.city, &eve.id).await);
    assert_ok(&server.vouch(&w.bo.key, &w.city, &fay.id).await);
    assert_ok(&server.vouch(&w.ann.key, &w.city, &w.bo.id).await);

    // Not another member, not a bot (the keeper's own and the house bot included), not nobody.
    for token in [&w.bo.key, &cy.key, &w.scout.token, &ann_bot.token, &casa.token] {
        assert_forbidden(&server.clear_gate(token, &w.city).await);
    }
    assert_status(&server.clear_gate("", &w.city).await, StatusCode::UNAUTHORIZED);
    // A stranger, and those waiting, can't tell the city from one that isn't there.
    let nowhere = server.clear_gate(&outsider.key, NO_SUCH_THING).await;
    assert_not_found(&nowhere);
    for token in [&outsider.key, &eve.key, &fay.key] {
        let reply = server.clear_gate(token, &w.city).await;
        assert_eq!((reply.status, &reply.body), (nowhere.status, &nowhere.body));
    }
    assert_eq!(server.gate(&w.ann.key, &w.city).await.len(), 2, "nothing changed");
    assert_eq!(server.state.db.vouch_count(&w.city, &eve.id).unwrap(), 2);

    // The keeper turns everyone away, and a place that had already lapsed isn't counted.
    let cleared = server.clear_gate(&w.ann.key, &w.city).await;
    assert_ok(&cleared);
    assert_eq!(cleared.body, json!({ "ok": true, "cleared": 2 }));
    assert!(server.gate(&w.ann.key, &w.city).await.is_empty());
    for person in [&eve, &fay, &stale] {
        assert_eq!(server.me(&person.key).await["waiting"], json!([]));
        assert_not_found(&server.city_view(&person.key, &w.city).await);
    }
    for person in [&eve, &fay] {
        assert_eq!(
            server.state.db.vouch_count(&w.city, &person.id).unwrap(),
            0,
            "the vouches are gone, not just hidden"
        );
    }
    // The people in the city are where they were, with the vouch given to one of them.
    assert_ok(&server.city_view(&w.bo.key, &w.city).await);
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["vouches"], json!(["Ann"]));
    // With nobody waiting there is nothing to clear, and it says so.
    assert_eq!(
        server.clear_gate(&w.ann.key, &w.city).await.body,
        json!({ "ok": true, "cleared": 0 })
    );

    // Turned away isn't banned: Eve can ask again with the link, from nothing.
    let back = server.use_link(&w.code, &eve.key, json!({ "accept_rules": true })).await;
    assert_eq!(
        (back.body["waiting"].as_bool(), back.body["vouches"].as_i64()),
        (Some(true), Some(0)),
        "{}",
        back.body
    );
}

// T17
#[tokio::test]
async fn once_the_keeper_has_left_any_member_can_clear_the_gate() {
    let server = Server::start().await;
    let w = server.world().await;
    server.move_in(&w.ann_home, "Pat").await;
    server.require_vouches(&w.ann, &w.city, 1).await;
    server.at_the_gate(&w.code, "Eve").await;
    assert_forbidden(&server.clear_gate(&w.bo.key, &w.city).await);
    // Ann leaves her house (Pat stays) and then the city, so the link is every member's to look after.
    assert_ok(
        &server.post(&w.ann.key, &format!("/api/houses/{}/leave", w.ann_home.id), json!({})).await,
    );
    assert_ok(&server.post(&w.ann.key, &format!("/api/cities/{}/leave", w.city), json!({})).await);
    assert_not_found(&server.clear_gate(&w.ann.key, &w.city).await);
    assert_eq!(
        server.clear_gate(&w.bo.key, &w.city).await.body,
        json!({ "ok": true, "cleared": 1 })
    );
}

// T17
#[tokio::test]
async fn the_keeper_hears_of_each_person_at_the_gate_and_nobody_else_does() {
    let server = Server::start().await;
    let w = server.world().await;
    server.require_vouches(&w.ann, &w.city, 1).await;
    let ann_pings = server.notify_to(&w.ann).await;
    let bo_pings = server.notify_to(&w.bo).await;

    let eve = server.at_the_gate(&w.code, "Eve").await;
    // Asking again is harmless and quiet.
    for _ in 0..3 {
        assert_ok(&server.use_link(&w.code, &eve.key, json!({ "accept_rules": true })).await);
    }
    let heard = expect_notices(&ann_pings, "city.gate", 1).await;
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert!(text(&heard[0]["text"]).contains("Eve") && text(&heard[0]["text"]).contains("Elm"));
    assert!(!text(&heard[0]["text"]).contains("vouch"), "no counts in the ping");
    assert!(all_notices(&bo_pings).await.is_empty(), "a leaked link must not buzz the whole city");

    // Turning her away tells no one anything, and neither does her next try, except the keeper again.
    assert_ok(&server.decline(&w.ann.key, &w.city, &eve.id).await);
    assert!(notices_of(&bo_pings, "city.gate").await.is_empty());
    assert_ok(&server.use_link(&w.code, &eve.key, json!({ "accept_rules": true })).await);
    assert_eq!(expect_notices(&ann_pings, "city.gate", 2).await.len(), 2);
    assert!(all_notices(&bo_pings).await.is_empty());
}

// T18
#[tokio::test]
async fn a_place_at_the_gate_lapses_after_fourteen_days_and_a_lapsed_place_cannot_be_vouched_in() {
    let server = Server::start().await;
    let w = server.world().await;
    server.require_vouches(&w.ann, &w.city, 1).await;
    let stale = server.gate_place_from(&w.city, 15, "Stale");
    let fresh = server.gate_place_from(&w.city, 13, "Fresh");

    // Seen by the city and by themselves only while it is good.
    let waiting = server.gate(&w.ann.key, &w.city).await;
    assert_eq!(waiting.iter().map(|e| text(&e["name"])).collect::<Vec<_>>(), ["Fresh"]);
    assert_eq!(server.me(&stale.key).await["waiting"], json!([]));
    assert_eq!(server.me(&fresh.key).await["waiting"][0]["city_id"], w.city.as_str());
    // A lapsed place can't be vouched in, turned away, or looked up.
    let nobody = server.vouch(&w.ann.key, &w.city, NO_SUCH_THING).await;
    for reply in [
        server.vouch(&w.ann.key, &w.city, &stale.id).await,
        server.decline(&w.ann.key, &w.city, &stale.id).await,
        server.standing(&w.ann.key, &w.city, &stale.id).await,
    ] {
        assert_eq!((reply.status, &reply.body), (nobody.status, &nobody.body));
    }
    assert_not_found(&server.city_view(&stale.key, &w.city).await);

    // A place that is still good can be, and is let in.
    let let_in = server.vouch(&w.ann.key, &w.city, &fresh.id).await;
    assert_eq!(let_in.body["let_in"], true, "{}", let_in.body);

    // The lapsed one asks again with the link, and gets a new place.
    let again = server.use_link(&w.code, &stale.key, json!({ "accept_rules": true })).await;
    assert_eq!(again.body["waiting"], true, "{}", again.body);
    assert_eq!(server.gate(&w.ann.key, &w.city).await.len(), 1);
    assert_eq!(server.vouch(&w.bo.key, &w.city, &stale.id).await.body["let_in"], true);
}

// T18
#[tokio::test]
async fn the_gate_holds_fifty_and_a_lapsed_place_does_not_count_against_that() {
    let server = Server::start().await;
    let w = server.world().await;
    server.require_vouches(&w.ann, &w.city, 1).await;
    // Five places have lapsed. Forty-five more are good.
    for n in 0..5 {
        server.gate_place_from(&w.city, 20, &format!("Old {n}"));
    }
    let mut waiting = Vec::new();
    for n in 0..45 {
        waiting.push(server.gate_place_from(&w.city, 1, &format!("Queue {n}")));
    }
    // Five more fit, through the link: fifty good places, and not fifty-five.
    for n in 0..5 {
        waiting.push(server.at_the_gate(&w.code, &format!("Late {n}")).await);
    }
    assert_eq!(server.gate(&w.ann.key, &w.city).await.len(), 50);

    // The fifty-first is told to try later, and no account is made for them.
    let people = server.state.db.person_count().unwrap();
    let full = server
        .use_link(&w.code, "", json!({ "your_name": "One too many", "accept_rules": true }))
        .await;
    assert_status(&full, StatusCode::CONFLICT);
    assert!(full.body["error"].as_str().unwrap().contains("full"), "{}", full.body);
    assert_eq!(server.state.db.person_count().unwrap(), people);
    // The same for someone who has an account already.
    let cy = server.key_person("Cy").await;
    assert_status(
        &server.use_link(&w.code, &cy.key, json!({ "accept_rules": true })).await,
        StatusCode::CONFLICT,
    );
    assert_eq!(server.me(&cy.key).await["waiting"], json!([]));
    assert_eq!(server.gate(&w.ann.key, &w.city).await.len(), 50);
    // People already there can still use the link without being turned away.
    assert_ok(&server.use_link(&w.code, &waiting[0].key, json!({ "accept_rules": true })).await);

    // Turning one away, or letting one in, makes room for exactly one.
    assert_ok(&server.decline(&w.ann.key, &w.city, &waiting[0].id).await);
    assert_ok(&server.use_link(&w.code, &cy.key, json!({ "accept_rules": true })).await);
    assert_status(
        &server.use_link(&w.code, &waiting[0].key, json!({ "accept_rules": true })).await,
        StatusCode::CONFLICT,
    );
    assert_eq!(server.vouch(&w.ann.key, &w.city, &waiting[1].id).await.body["let_in"], true);
    assert_ok(&server.use_link(&w.code, &waiting[0].key, json!({ "accept_rules": true })).await);
    assert_eq!(server.gate(&w.ann.key, &w.city).await.len(), 50);
}

// T19
#[tokio::test]
async fn moving_into_a_house_through_its_link_skips_the_gate_and_the_city_link_alone_does_not() {
    let server = Server::start().await;
    let w = server.world().await;
    server.require_vouches(&w.ann, &w.city, 2).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    let fay = server.at_the_gate(&w.code, "Fay").await;

    // The city link again, however many times, still leaves her at the gate.
    for _ in 0..3 {
        assert_eq!(
            server.use_link(&w.code, &eve.key, json!({ "accept_rules": true })).await.body
                ["waiting"],
            true
        );
    }
    assert_not_found(&server.city_view(&eve.key, &w.city).await);

    // A house link is a resident's yes, so it skips the gate and ends the wait.
    let first_link = server.house_code(&w.bo_home);
    let joined =
        server.post(&eve.key, &format!("/api/house-invites/{first_link}/join"), json!({})).await;
    assert_ok(&joined);
    assert_eq!(joined.body["city_id"], w.city.as_str());
    assert_ok(&server.city_view(&eve.key, &w.city).await);
    assert_ok(&server.house_view(&eve.key, &w.bo_home.id).await);
    assert_eq!(server.me(&eve.key).await["waiting"], json!([]));
    let gate = server.gate(&w.ann.key, &w.city).await;
    assert_eq!(gate.len(), 1);
    assert_eq!(gate[0]["name"], "Fay");
    // Fay is untouched, and no house link the house didn't give her works.
    assert_not_found(&server.city_view(&fay.key, &w.city).await);
    let old_link =
        server.post(&fay.key, &format!("/api/house-invites/{NO_SUCH_THING}/join"), json!({})).await;
    assert_not_found(&old_link);
    assert_eq!(server.me(&fay.key).await["waiting"][0]["city_id"], w.city.as_str());

    // A house link works once: it changed when Eve moved in, so the one Eve used lets nobody else in.
    let used =
        server.post(&fay.key, &format!("/api/house-invites/{first_link}/join"), json!({})).await;
    assert_not_found(&used);
    assert_not_found(&server.city_view(&fay.key, &w.city).await);
    // Someone brand new with a house link is simply in, with an account made on the spot.
    let newcomer = server.move_in(&w.bo_home, "Newcomer").await;
    assert_ok(&server.city_view(&newcomer.key, &w.city).await);
    assert_eq!(server.me(&newcomer.key).await["waiting"], json!([]));
    // Someone who was turned away is let in by a house too: a resident chose them.
    assert_ok(&server.decline(&w.ann.key, &w.city, &fay.id).await);
    let fresh = server.at_the_gate(&w.code, "Gus").await;
    assert_ok(&server.decline(&w.ann.key, &w.city, &fresh.id).await);
    let moved = server
        .post(
            &fresh.key,
            &format!("/api/house-invites/{}/join", server.house_code(&w.bo_home)),
            json!({}),
        )
        .await;
    assert_ok(&moved);
}

// T20
#[tokio::test]
async fn someone_at_the_gate_sees_only_their_own_count_in_me_and_nothing_else_of_the_city() {
    let server = Server::start().await;
    let w = server.world().await;
    let cy = server.join_city(&w.code, "Cy").await;
    assert_ok(
        &server
            .set_city_rules(
                &w.ann.key,
                &w.city,
                json!({ "min_vouches": 3, "rules_text": "Secret handshake." }),
            )
            .await,
    );
    let eve = server.at_the_gate(&w.code, "Eve").await;

    let me = server.me(&eve.key).await;
    assert_eq!(me["waiting"].as_array().unwrap().len(), 1);
    let place = &me["waiting"][0];
    assert_eq!(keys_of(place), ["city_id", "name", "vouches", "vouches_needed"]);
    assert_eq!(
        (place["name"].as_str(), place["vouches"].as_i64(), place["vouches_needed"].as_i64()),
        (Some("Elm"), Some(0), Some(3))
    );
    assert_eq!(place["city_id"], w.city.as_str());
    let said = without_ids(&me["waiting"], &[&w.city]);
    for secret in
        ["Ann", "Bo", "Cy", "Secret handshake", w.square.as_str(), "Ann House", "Bo House", "Scout"]
    {
        assert!(!said.contains(secret), "{secret} in {}", me["waiting"]);
    }
    assert!(!has(&me, "Secret handshake") && !has(&me, w.square.as_str()), "{me}");
    assert_eq!(me["cities"], json!([]));
    // It follows the vouches, and counts only people in the city.
    assert_ok(&server.vouch(&w.ann.key, &w.city, &eve.id).await);
    assert_ok(&server.vouch(&cy.key, &w.city, &eve.id).await);
    assert_eq!(server.me(&eve.key).await["waiting"][0]["vouches"], 2);
    // It's a person's: a bot of Eve's sees no `waiting`, nor does anyone's passkeys list leak it.
    let room = text(
        &server.post(&eve.key, "/api/hangouts", json!({ "name": "Eve's own" })).await.body
            ["hangout"]["id"],
    );
    let bot = server.bot(&eve, &room, "EveBot").await;
    let bot_me = server.get(&bot.token, "/api/me").await;
    assert!(
        bot_me.body.get("waiting").is_none() && bot_me.body["cities"] == json!([]),
        "{}",
        bot_me.body
    );
    // Two gates are two entries, each with its own count.
    let (oak, _, oak_code, oak_keeper) = server.new_city("Oak", "Gus").await;
    server.require_vouches(&oak_keeper, &oak, 1).await;
    assert_ok(&server.use_link(&oak_code, &eve.key, json!({ "accept_rules": true })).await);
    let waiting = server.me(&eve.key).await["waiting"].clone();
    assert_eq!(waiting.as_array().unwrap().len(), 2, "{waiting}");
    // Let in, it is gone from `waiting` and in `cities`.
    assert_eq!(server.vouch(&w.bo.key, &w.city, &eve.id).await.body["let_in"], true);
    let me = server.me(&eve.key).await;
    assert_eq!(me["waiting"].as_array().unwrap().len(), 1);
    assert_eq!(me["waiting"][0]["name"], "Oak");
    assert_eq!(me["cities"][0]["id"], w.city.as_str());
}

// T21
#[tokio::test]
async fn lowering_the_vouches_asked_for_lets_in_everyone_who_now_has_enough_and_only_them() {
    let server = Server::start().await;
    let w = server.world().await;
    let cy = server.join_city(&w.code, "Cy").await;
    let dan = server.join_city(&w.code, "Dan").await;
    server.require_vouches(&w.ann, &w.city, 3).await;
    let none = server.at_the_gate(&w.code, "None").await;
    let one = server.at_the_gate(&w.code, "One").await;
    let two = server.at_the_gate(&w.code, "Two").await;
    assert_ok(&server.vouch(&w.ann.key, &w.city, &one.id).await);
    assert_ok(&server.vouch(&w.ann.key, &w.city, &two.id).await);
    assert_ok(&server.vouch(&w.bo.key, &w.city, &two.id).await);
    let one_pings = server.notify_to(&one).await;
    let two_pings = server.notify_to(&two).await;
    let none_pings = server.notify_to(&none).await;

    // Raising it again lets nobody in and puts nobody out.
    assert_ok(&server.set_city_rules(&w.ann.key, &w.city, json!({ "min_vouches": 4 })).await);
    assert_eq!(server.gate(&w.ann.key, &w.city).await.len(), 3);
    // Lowering to two: Two is in, at once, and the other two are not.
    assert_ok(&server.set_city_rules(&w.ann.key, &w.city, json!({ "min_vouches": 2 })).await);
    assert_ok(&server.city_view(&two.key, &w.city).await);
    assert_not_found(&server.city_view(&one.key, &w.city).await);
    assert_not_found(&server.city_view(&none.key, &w.city).await);
    assert_eq!(server.me(&one.key).await["waiting"][0]["vouches_needed"], 2);
    assert_eq!(expect_notices(&two_pings, "city.let_in", 1).await.len(), 1);
    assert!(notices_of(&one_pings, "city.let_in").await.is_empty());
    // Setting the same number again does it no more than once.
    assert_ok(&server.set_city_rules(&w.ann.key, &w.city, json!({ "min_vouches": 2 })).await);
    assert_eq!(expect_notices(&two_pings, "city.let_in", 1).await.len(), 1);
    // Lowering to one lets One in, and to nothing lets in everyone who is left.
    assert_ok(&server.set_city_rules(&w.ann.key, &w.city, json!({ "min_vouches": 1 })).await);
    assert_ok(&server.city_view(&one.key, &w.city).await);
    assert_not_found(&server.city_view(&none.key, &w.city).await);
    assert_ok(&server.set_city_rules(&w.ann.key, &w.city, json!({ "min_vouches": 0 })).await);
    assert_ok(&server.city_view(&none.key, &w.city).await);
    assert_eq!(expect_notices(&none_pings, "city.let_in", 1).await.len(), 1);
    assert!(server.gate(&w.ann.key, &w.city).await.is_empty());
    // Each was announced once in the Town Square.
    let lines = server.town_square_says(&w.ann.key, &w.square).await;
    for name in ["None", "One", "Two"] {
        assert_eq!(
            lines.iter().filter(|l| l.as_str() == format!("{name} joined Elm.")).count(),
            1,
            "{lines:?}"
        );
    }
    let _ = (cy, dan);
}

// ----------------------------------------------------------------------------------------------
// Getting in with nothing: the gate, a brand-new account, and what a house bot's world holds (R1, T15, A11)
// ----------------------------------------------------------------------------------------------

// R1 T15
#[tokio::test]
async fn someone_at_the_gate_and_their_bot_can_tell_nothing_in_the_city_from_a_place_that_is_not_there(
) {
    let server = Server::start().await;
    let w = server.world().await;
    let (real, fake) = server.ids(&w).await;
    server.require_vouches(&w.ann, &w.city, 1).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    // Eve has an own room and a bot, made while she waits.
    let room = text(
        &server.post(&eve.key, "/api/hangouts", json!({ "name": "Eve's own" })).await.body
            ["hangout"]["id"],
    );
    let bot = server.bot(&eve, &room, "EveBot").await;
    assert_eq!(server.me(&eve.key).await["waiting"][0]["vouches"], 0);

    server.assert_blind("someone at the gate", &eve.key, &real, &fake).await;
    server.assert_blind("the bot of someone at the gate", &bot.token, &real, &fake).await;
    // Nothing in all of that moved anything: she is still waiting and the city is as it was.
    assert_eq!(server.gate(&w.ann.key, &w.city).await.len(), 1);
    assert_eq!(server.city_rules_of(&w.ann.key, &w.city).await["min_vouches"], 1);
}

// R1 A13
#[tokio::test]
async fn a_brand_new_passkey_account_with_no_invite_can_tell_nothing_from_a_place_that_is_not_there(
) {
    let server = Server::start().await;
    let w = server.world().await;
    let (real, fake) = server.ids(&w).await;
    let newbie = server.sign_up("Newbie").await;
    let session = newbie.who.key.strip_prefix("cookie:").unwrap().to_string();

    // As a browser holding the cookie, as a bearer session token, and with the owner key it was shown.
    server.assert_blind("a new account with a cookie", &newbie.who.key, &real, &fake).await;
    server.assert_blind("a new account with a session token", &session, &real, &fake).await;
    server.assert_blind("a new account with its owner key", &newbie.owner_key, &real, &fake).await;

    // What they can see is their own.
    let me = server.me(&newbie.who.key).await;
    assert_eq!(
        (me["cities"].clone(), me["houses"].clone(), me["hangouts"].clone()),
        (json!([]), json!([]), json!([]))
    );
    assert_eq!(me["waiting"], json!([]));
    assert_eq!(me["bots"], json!([]));
    // The home page and the join pages are public, and hold only the app.
    for path in
        ["/", "/c/anything", "/house/anything", "/join-city/anything", "/join-house/anything"]
    {
        assert_eq!(server.get(&newbie.who.key, path).await.status, StatusCode::OK, "{path}");
    }
}

// A11 T15
#[tokio::test]
async fn a_new_passkey_account_cannot_start_a_hangout_or_a_city_without_the_server_code() {
    let server = Server::start_with(Some("letmein")).await;
    let newbie = server.sign_up("Newbie").await;
    let session = newbie.who.key.strip_prefix("cookie:").unwrap().to_string();

    for token in [&newbie.who.key, &session, &newbie.owner_key, &String::new()] {
        for code in [None, Some("wrong"), Some("LETMEIN"), Some(""), Some(" letmein")] {
            let mut hangout = json!({ "name": "Mine", "your_name": "Newbie" });
            let mut city = json!({ "name": "Mine", "your_name": "Newbie" });
            if let Some(code) = code {
                hangout["create_code"] = json!(code);
                city["create_code"] = json!(code);
            }
            assert_forbidden(&server.post(token, "/api/hangouts", hangout).await);
            assert_forbidden(&server.post(token, "/api/cities", city).await);
        }
    }
    assert_eq!(server.state.db.person_count().unwrap(), 1, "refusals made no one new");
    // Not a number or the wrong type either.
    for code in [json!(5), json!(true), json!(["letmein"]), json!(null)] {
        let reply = server
            .post(
                &newbie.who.key,
                "/api/cities",
                json!({ "name": "Mine", "create_code": code.clone() }),
            )
            .await;
        assert_denied(&reply);
    }

    // With the code it works, and joining anything by an invite never needed it.
    let made = server
        .post(&newbie.who.key, "/api/cities", json!({ "name": "Mine", "create_code": "letmein" }))
        .await;
    assert_ok(&made);
    assert!(made.body["owner_key"].is_null(), "she is signed in, so nothing new is made");
    let code = code_of(&made.body["invite_url"]);
    let other = server.sign_up("Other").await;
    assert_ok(&server.use_link(&code, &other.who.key, json!({})).await);
    let anyone = server.use_link(&code, "", json!({ "your_name": "Anyone" })).await;
    assert_ok(&anyone);
    // And a bot of theirs can't start anything with or without the code.
    let room = server
        .post(&newbie.who.key, "/api/hangouts", json!({ "name": "Room", "create_code": "letmein" }))
        .await;
    let room = text(&room.body["hangout"]["id"]);
    let made_bot = server
        .post(&newbie.who.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": "Bot" }))
        .await;
    let bot = text(&made_bot.body["token"]);
    assert_forbidden(
        &server
            .post(&bot, "/api/cities", json!({ "name": "Bots", "create_code": "letmein" }))
            .await,
    );
    assert_forbidden(
        &server
            .post(&bot, "/api/hangouts", json!({ "name": "Bots", "create_code": "letmein" }))
            .await,
    );
}

// A11 T15
#[tokio::test]
async fn a_new_account_with_no_invite_reaches_a_city_only_through_its_link_or_a_house_link() {
    let server = Server::start().await;
    let w = server.world().await;
    let newbie = server.sign_up("Newbie").await;

    // The three links a stranger might hold, and a wrong one, each give exactly what they should.
    assert_not_found(&server.use_link("not-a-code", &newbie.who.key, json!({})).await);
    assert_not_found(&server.get(&newbie.who.key, &format!("/api/hangouts/{}", w.square)).await);
    let preview = server.get("", &format!("/api/city-invites/{}", w.code)).await;
    assert_eq!(preview.body["name"], "Elm");
    assert_not_found(&server.get(&newbie.who.key, &format!("/api/cities/{}", w.city)).await);
    let joined = server.use_link(&w.code, &newbie.who.key, json!({})).await;
    assert_ok(&joined);
    assert_eq!(joined.body["waiting"], false);
    assert_ok(&server.city_view(&newbie.who.key, &w.city).await);
    // In the city, a passkey account still has no house, no resident rights, no say in anyone's.
    assert_not_found(&server.house_view(&newbie.who.key, &w.ann_home.id).await);
    assert_not_found(
        &server
            .set_house_rules(&newbie.who.key, &w.ann_home.id, json!({ "min_gatherings": 1 }))
            .await,
    );
    assert_forbidden(
        &server.set_city_rules(&newbie.who.key, &w.city, json!({ "min_vouches": 1 })).await,
    );
    assert_forbidden(&server.decline(&newbie.who.key, &w.city, &w.bo.id).await);
    // And no standing that passkeys could have given them (A13).
    let standing = server.stand(&w.ann.key, &w.city, &newbie.who.id).await;
    assert_eq!(
        (
            standing["gatherings"].as_i64(),
            standing["good_trips"].as_i64(),
            standing["sent_home"].as_i64()
        ),
        (Some(0), Some(0), Some(0))
    );
    assert_eq!(
        (standing["member_days"].as_i64(), standing["vouches"].clone()),
        (Some(0), json!([]))
    );
}

// R24 T6 T10
#[tokio::test]
async fn a_house_bots_world_stays_three_tools_and_every_trust_route_refuses_its_token() {
    let server = Server::start().await;
    let w = server.world().await;
    let casa = server.house_bot(&w.ann, &w.ann_home, "Casa").await;
    server.require(&w.ann, &w.ann_home, json!({ "min_gatherings": 1 })).await;

    let listed = server.mcp(&casa.token, "tools/list", json!({})).await;
    let mut names: Vec<String> = listed.body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| text(&t["name"]))
        .collect();
    names.sort();
    assert_eq!(names, ["finish_request", "house_queue", "set_house_status"]);
    // Every other name, the new ones and invented ones among them, is refused with nothing in it.
    for name in [
        "list_city",
        "ask_for_trip",
        "go_home",
        "read_room",
        "say",
        "inbox",
        "list_calendar",
        "vouch",
        "unvouch",
        "standing",
        "get_standing",
        "decline",
        "set_house_rules",
        "set_city_rules",
        "requirements",
        "gate",
        "list_gate",
        "register_passkey",
        "sign_in",
        "rotate_owner_key",
    ] {
        let result = server
            .tool(
                &casa.token,
                name,
                json!({ "house_id": w.ann_home.id, "city_id": w.city, "person_id": w.bo.id }),
            )
            .await;
        assert_eq!(result["isError"], true, "{name}: {result}");
        assert!(!has(&result, "min_gatherings") && !has(&result, "Ann House"), "{name}: {result}");
    }
    // 403 on every route this feature added, and on the passkey and session ones.
    let routes: Vec<(Method, String, Option<Value>)> = vec![
        (Method::GET, format!("/api/cities/{}/people/{}/standing", w.city, w.bo.id), None),
        (Method::POST, format!("/api/cities/{}/people/{}/vouch", w.city, w.bo.id), Some(json!({}))),
        (Method::DELETE, format!("/api/cities/{}/people/{}/vouch", w.city, w.bo.id), None),
        (Method::POST, format!("/api/cities/{}/gate/{}/decline", w.city, w.bo.id), Some(json!({}))),
        (Method::PUT, format!("/api/cities/{}/rules", w.city), Some(json!({ "min_vouches": 0 }))),
        (
            Method::PUT,
            format!("/api/houses/{}/rules", w.ann_home.id),
            Some(json!({ "min_gatherings": 0 })),
        ),
        (Method::GET, "/api/passkeys".into(), None),
        (Method::GET, "/api/passkeys/available".into(), None),
        (Method::POST, "/api/passkeys/register/start".into(), Some(json!({ "your_name": "Casa" }))),
        (Method::POST, "/api/passkeys/register/finish".into(), Some(json!({}))),
        (Method::POST, "/api/passkeys/login/start".into(), Some(json!({}))),
        (Method::POST, "/api/passkeys/login/finish".into(), Some(json!({}))),
        (Method::DELETE, "/api/passkeys/anything".into(), None),
        (Method::POST, "/api/key/rotate".into(), Some(json!({}))),
        (Method::POST, format!("/api/city-invites/{}/join", w.code), Some(json!({}))),
        (
            Method::POST,
            "/api/trips".into(),
            Some(json!({ "house_id": w.ann_home.id, "kind": "call" })),
        ),
    ];
    for (method, path, body) in routes {
        let reply = server.request(&casa.token, method.clone(), &path, body).await;
        assert_forbidden(&reply);
    }
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["vouches"], json!([]));
    assert_eq!(server.city_rules_of(&w.ann.key, &w.city).await["min_vouches"], 0);
}

// T3 T16 T20
#[tokio::test]
async fn an_ordinary_bot_has_no_trust_tools_and_every_trust_route_that_changes_something_refuses_it(
) {
    let server = Server::start().await;
    let w = server.world().await;
    let listed = server.mcp(&w.scout.token, "tools/list", json!({})).await;
    let names: Vec<String> = listed.body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| text(&t["name"]))
        .collect();
    for banned in [
        "vouch",
        "standing",
        "gate",
        "rules",
        "decline",
        "passkey",
        "session",
        "owner_key",
        "requirement_set",
    ] {
        assert!(names.iter().all(|name| !name.contains(banned)), "{banned} among {names:?}");
    }
    assert!(
        names.contains(&"list_city".to_string()) && names.contains(&"ask_for_trip".to_string())
    );
    server.require_vouches(&w.ann, &w.city, 1).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    for (method, path, body) in [
        (Method::POST, format!("/api/cities/{}/people/{}/vouch", w.city, eve.id), Some(json!({}))),
        (Method::DELETE, format!("/api/cities/{}/people/{}/vouch", w.city, eve.id), None),
        (Method::POST, format!("/api/cities/{}/gate/{}/decline", w.city, eve.id), Some(json!({}))),
        (Method::PUT, format!("/api/cities/{}/rules", w.city), Some(json!({ "min_vouches": 0 }))),
        (
            Method::PUT,
            format!("/api/houses/{}/rules", w.bo_home.id),
            Some(json!({ "min_gatherings": 0 })),
        ),
        (Method::POST, format!("/api/city-invites/{}/join", w.code), Some(json!({}))),
    ] {
        assert_forbidden(&server.request(&w.scout.token, method, &path, body).await);
    }
    assert_eq!(server.gate(&w.ann.key, &w.city).await.len(), 1);
    // It may read standing (T5) and the map; the map holds the requirements but no gate.
    assert_ok(&server.standing(&w.scout.token, &w.city, &eve.id).await);
    let city = server.city_view(&w.scout.token, &w.city).await;
    assert!(
        city.body["houses"][0].get("requirements").is_some() && city.body.get("gate").is_none()
    );
}

// T7 T9
#[tokio::test]
async fn over_mcp_list_city_shows_what_each_house_asks_and_ask_for_trip_returns_the_shortfall() {
    let server = Server::start().await;
    let w = server.world().await;
    server.require(&w.ann, &w.ann_home, json!({ "min_member_days": 3, "min_gatherings": 2 })).await;
    let listed = server.tool(&w.scout.token, "list_city", json!({})).await;
    let houses = listed["structuredContent"]["houses"].as_array().unwrap().clone();
    let ann = houses.iter().find(|h| h["name"] == "Ann House").unwrap();
    let bo = houses.iter().find(|h| h["name"] == "Bo House").unwrap();
    assert_eq!(
        ann["requirements"],
        json!({ "min_member_days": 3, "min_gatherings": 2, "min_good_trips": 0, "min_vouches": 0 })
    );
    assert_eq!(
        bo["requirements"],
        json!({ "min_member_days": 0, "min_gatherings": 0, "min_good_trips": 0, "min_vouches": 0 })
    );
    assert!(listed["structuredContent"].get("gate").is_none());

    let refused = server
        .tool(&w.scout.token, "ask_for_trip", json!({ "house_id": w.ann_home.id, "kind": "call" }))
        .await;
    assert_eq!(refused["isError"], true);
    let said = text(&refused["content"][0]["text"]);
    assert!(
        said.contains("2 gatherings (Bo has 0)") && said.contains("3 days in the city (Bo has 0)"),
        "{said}"
    );
    // A resident's invitation goes through MCP-free, and the bot then answers nothing: its owner does.
    let invited = server.ask_trip(&w.ann.key, Some(&w.scout.id), &w.ann_home.id, None).await;
    assert_ok(&invited);
}

// ----------------------------------------------------------------------------------------------
// Races (T3, T17, T18, T21)
// ----------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_vouches_at_once_let_someone_in_exactly_once() {
    let server = Arc::new(Server::start().await);
    let w = server.world().await;
    let mut crowd = Vec::new();
    for n in 0..9 {
        crowd.push(server.join_city(&w.code, &format!("Member {n}")).await);
    }
    server.require_vouches(&w.ann, &w.city, 3).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    let eve_pings = server.notify_to(&eve).await;

    let voters: Vec<Who> = crowd.iter().cloned().chain([w.ann.clone(), w.bo.clone()]).collect();
    let tasks: Vec<_> = voters
        .into_iter()
        .map(|voter| {
            let server = server.clone();
            let city = w.city.clone();
            let eve_id = eve.id.clone();
            tokio::spawn(async move { server.vouch(&voter.key, &city, &eve_id).await })
        })
        .collect();
    let mut let_in = 0;
    for task in tasks {
        let reply = task.await.unwrap();
        assert_ok(&reply);
        if reply.body["let_in"] == true {
            let_in += 1;
        }
    }
    assert_eq!(let_in, 1, "exactly one vouch is the one that lets her in");
    assert_ok(&server.city_view(&eve.key, &w.city).await);
    let lines: Vec<_> = server
        .town_square_says(&w.ann.key, &w.square)
        .await
        .into_iter()
        .filter(|l| l.contains("Eve"))
        .collect();
    assert_eq!(lines, ["Eve joined Elm."], "told the Town Square once");
    assert_eq!(expect_notices(&eve_pings, "city.let_in", 1).await.len(), 1, "pinged once");
    assert_eq!(
        server.stand(&w.ann.key, &w.city, &eve.id).await["vouches"].as_array().unwrap().len(),
        11
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_vouch_and_a_lowering_of_the_bar_at_the_same_moment_let_someone_in_once() {
    for _ in 0..5 {
        let server = Arc::new(Server::start().await);
        let w = server.world().await;
        server.require_vouches(&w.ann, &w.city, 2).await;
        let eve = server.at_the_gate(&w.code, "Eve").await;
        assert_ok(&server.vouch(&w.ann.key, &w.city, &eve.id).await);
        let (vouched, lowered) = tokio::join!(
            server.vouch(&w.bo.key, &w.city, &eve.id),
            server.set_city_rules(&w.ann.key, &w.city, json!({ "min_vouches": 1 })),
        );
        assert_ok(&vouched);
        assert_ok(&lowered);
        assert_ok(&server.city_view(&eve.key, &w.city).await);
        let lines: Vec<_> = server
            .town_square_says(&w.ann.key, &w.square)
            .await
            .into_iter()
            .filter(|l| l.contains("Eve"))
            .collect();
        assert_eq!(lines, ["Eve joined Elm."]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sixty_people_arriving_at_once_fill_the_gate_to_fifty_and_no_further() {
    let server = Arc::new(Server::start().await);
    let (city, _, code, ann) = server.new_city("Elm", "Ann").await;
    server.require_vouches(&ann, &city, 1).await;
    let tasks: Vec<_> = (0..60)
        .map(|n| {
            let server = server.clone();
            let code = code.clone();
            tokio::spawn(async move {
                server
                    .use_link(
                        &code,
                        "",
                        json!({ "your_name": format!("Visitor {n}"), "accept_rules": true }),
                    )
                    .await
            })
        })
        .collect();
    let (mut waiting, mut turned_away) = (0, 0);
    for task in tasks {
        let reply = task.await.unwrap();
        match reply.status {
            StatusCode::OK => waiting += 1,
            StatusCode::CONFLICT => turned_away += 1,
            other => panic!("neither in nor told it was full: {} {}", other, reply.body),
        }
    }
    assert_eq!((waiting, turned_away), (50, 10));
    assert_eq!(server.gate(&ann.key, &city).await.len(), 50, "never over fifty");
    // Whether the ten were made accounts before being turned away is for the report to say.
    let people = server.state.db.person_count().unwrap();
    eprintln!(
        "accounts after the race: {people} (1 keeper + 50 waiting + {} left behind)",
        people - 51
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_person_using_the_link_twenty_times_at_once_takes_one_place_and_pings_once() {
    let server = Arc::new(Server::start().await);
    let w = server.world().await;
    server.require_vouches(&w.ann, &w.city, 1).await;
    let ann_pings = server.notify_to(&w.ann).await;
    let eve = server.key_person("Eve").await;
    let tasks: Vec<_> = (0..20)
        .map(|_| {
            let server = server.clone();
            let code = w.code.clone();
            let key = eve.key.clone();
            tokio::spawn(async move {
                server.use_link(&code, &key, json!({ "accept_rules": true })).await
            })
        })
        .collect();
    for task in tasks {
        let reply = task.await.unwrap();
        assert_ok(&reply);
        assert_eq!(reply.body["waiting"], true);
    }
    assert_eq!(server.gate(&w.ann.key, &w.city).await.len(), 1);
    assert_eq!(expect_notices(&ann_pings, "city.gate", 1).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_vouch_sent_twenty_times_at_once_is_one_vouch() {
    let server = Arc::new(Server::start().await);
    let w = server.world().await;
    server.require_vouches(&w.ann, &w.city, 2).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    let tasks: Vec<_> = (0..20)
        .map(|_| {
            let server = server.clone();
            let (key, city, id) = (w.ann.key.clone(), w.city.clone(), eve.id.clone());
            tokio::spawn(async move { server.vouch(&key, &city, &id).await })
        })
        .collect();
    for task in tasks {
        let reply = task.await.unwrap();
        assert_ok(&reply);
        assert_eq!(
            (reply.body["vouches"].as_i64(), reply.body["let_in"].as_bool()),
            (Some(1), Some(false))
        );
    }
    assert_not_found(&server.city_view(&eve.key, &w.city).await);
}

// ----------------------------------------------------------------------------------------------
// What a notification may say (T16, T17), and what a log line may hold (A7)
// ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn no_notification_from_the_gate_or_the_rules_carries_standing_requirements_or_a_secret() {
    let server = Server::start().await;
    let w = server.world().await;
    let cy = server.join_city(&w.code, "Cy").await;
    let pings: Vec<_> = {
        let mut all = Vec::new();
        for who in [&w.ann, &w.bo, &cy] {
            all.push(server.notify_to(who).await);
        }
        all
    };
    server.require(&w.ann, &w.ann_home, json!({ "min_gatherings": 3, "min_vouches": 2 })).await;
    assert_status(
        &server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await,
        StatusCode::CONFLICT,
    );
    assert_ok(
        &server
            .set_city_rules(
                &w.ann.key,
                &w.city,
                json!({ "min_vouches": 2, "rules_text": "Rules-text-9921" }),
            )
            .await,
    );
    let eve = server.at_the_gate(&w.code, "Eve").await;
    let fay = server.at_the_gate(&w.code, "Fay").await;
    let eve_pings = server.notify_to(&eve).await;
    assert_ok(&server.vouch(&w.ann.key, &w.city, &eve.id).await);
    assert_ok(&server.vouch(&w.bo.key, &w.city, &eve.id).await);
    assert_ok(&server.decline(&w.ann.key, &w.city, &fay.id).await);
    assert_ok(&server.set_city_rules(&w.ann.key, &w.city, json!({ "min_vouches": 1 })).await);

    // Wait for what is coming, so what is checked below is all of it.
    expect_notices(&pings[0], "city.gate", 2).await;
    expect_notices(&eve_pings, "city.let_in", 1).await;
    let mut everything = Vec::new();
    for received in pings.iter().chain([&eve_pings]) {
        everything.extend(all_notices(received).await);
    }
    assert!(!everything.is_empty());
    for notice in &everything {
        // The text ends in a link to the Town Square, with its id in it.
        let said = without_ids(&notice["text"], &[&w.square]);
        for forbidden in [
            "min_",
            "requirement",
            "standing",
            "gatherings",
            "good_trips",
            "sent_home",
            "Rules-text-9921",
            "asks for",
            "ah_owner_",
            "ah_sess_",
            "ah_bot_",
            "owner_key",
            PLAN_TITLE,
            TRIP_GOAL,
            "Fay",
        ] {
            // Fay is named to her keeper when she arrives, and to no one else.
            if forbidden == "Fay" && notice["event"] == "city.gate" {
                continue;
            }
            assert!(!said.contains(forbidden), "{forbidden} in {said}");
        }
        assert!(
            ["city.gate", "city.let_in", "house.joined", "trip.asked"]
                .contains(&notice["event"].as_str().unwrap()),
            "{notice}"
        );
    }
    // Only the keeper heard of the gate.
    for (name, received) in ["Ann", "Bo", "Cy"].into_iter().zip(&pings) {
        let gate = expect_notices(received, "city.gate", if name == "Ann" { 2 } else { 0 }).await;
        assert_eq!(gate.len(), if name == "Ann" { 2 } else { 0 }, "{name}: {gate:?}");
    }
}

// A7
#[test]
fn no_log_line_or_print_in_the_server_holds_a_token_a_key_a_cookie_or_a_hash() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut files = vec![];
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("doorbell.rs") {
            files.push(path);
        }
    }
    assert!(files.len() > 8);
    let mut calls = 0;
    for path in files {
        let source = std::fs::read_to_string(&path).unwrap();
        // Everything from the first test module on is test code.
        let source = source.split("#[cfg(test)]").next().unwrap();
        for marker in [
            "tracing::error!(",
            "tracing::warn!(",
            "tracing::info!(",
            "tracing::debug!(",
            "tracing::trace!(",
            "println!(",
            "eprintln!(",
            "dbg!(",
        ] {
            let mut rest = source;
            while let Some(at) = rest.find(marker) {
                rest = &rest[at + marker.len()..];
                // The call runs to its closing bracket; take the arguments outside the quotes.
                let (mut depth, mut in_string, mut escaped) = (1, false, false);
                let mut args = String::new();
                for ch in rest.chars() {
                    if in_string {
                        if escaped {
                            escaped = false;
                        } else if ch == '\\' {
                            escaped = true;
                        } else if ch == '"' {
                            in_string = false;
                        }
                        continue;
                    }
                    match ch {
                        '"' => in_string = true,
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => args.push(ch),
                    }
                }
                calls += 1;
                for banned in [
                    "token",
                    "secret",
                    "cookie",
                    "owner_key",
                    "key_hash",
                    "hash",
                    "credential",
                    "password",
                    "session",
                ] {
                    assert!(
                        !args.contains(banned),
                        "{}: {marker} logs {banned}: {args}",
                        path.display()
                    );
                }
            }
        }
    }
    assert!(calls > 5, "the scan found only {calls} log calls: has the code moved?");
}

// ----------------------------------------------------------------------------------------------
// Numbers, shapes and the ways around the rules the doc names (T1 to T21, limits)
// ----------------------------------------------------------------------------------------------

impl Server {
    /// The status of a request with these exact bytes as its JSON body, or 0 if the connection was
    /// closed on it, as a server may when it refuses a body that is too big.
    async fn raw_status(&self, token: &str, method: Method, path: &str, body: Vec<u8>) -> u16 {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body);
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        match request.send().await {
            Ok(response) => response.status().as_u16(),
            Err(_) => 0,
        }
    }

    /// The status of a request, or 0 if the server dropped the connection (a panic does that).
    async fn status_of(&self, token: &str, method: Method, path: &str, body: Option<Value>) -> u16 {
        let mut request = self.client.request(method, format!("{}{path}", self.base));
        if let Some(cookie) = token.strip_prefix("cookie:") {
            request = request.header(header::COOKIE, format!("ah_session={cookie}"));
        } else if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        match request.send().await {
            Ok(response) => response.status().as_u16(),
            Err(_) => 0,
        }
    }
}

fn extreme_numbers() -> Vec<Value> {
    vec![
        json!(i64::MAX),
        json!(i64::MIN),
        json!(i64::MAX - 1),
        json!(i64::MIN + 1),
        json!(u64::MAX),
        json!(i32::MAX),
        json!(i32::MIN),
        json!(u32::MAX),
        json!(-1),
        json!(0),
        json!(1),
        json!(9_007_199_254_740_993i64),
        json!(1e300),
        json!(-1e300),
        json!(0.5),
        json!(f64::MAX),
        json!("9223372036854775807"),
        json!(null),
        json!(true),
        json!([9223372036854775807i64]),
    ]
}

// T6 T13 (Limits)
#[tokio::test]
async fn extreme_numbers_in_every_numeric_field_are_refused_or_clamped_and_never_crash_the_server()
{
    let server = Server::start().await;
    let w = server.world().await;
    let trip = server.arrive(&w.scout, &w.bo, &w.ann, &w.ann_home.id).await;
    let (cy, cy_home) = server.neighbour(&w, "Cy").await;
    let pip = server.bot(&cy, &cy_home.living, "Pip").await;
    let mut tried = 0;
    let mut problems: Vec<String> = Vec::new();
    for value in extreme_numbers() {
        // The new fields, one at a time and then all of them together.
        for field in ["min_member_days", "min_gatherings", "min_good_trips", "min_vouches"] {
            let status = server
                .status_of(
                    &w.ann.key,
                    Method::PUT,
                    &format!("/api/houses/{}/rules", w.ann_home.id),
                    Some(json!({ field: value })),
                )
                .await;
            if !(200..500).contains(&status) {
                problems.push(format!("house {field} = {value}: {status}"));
            }
            let status = server
                .status_of(
                    &w.ann.key,
                    Method::PUT,
                    &format!("/api/cities/{}/rules", w.city),
                    Some(json!({ field: value })),
                )
                .await;
            if !(200..500).contains(&status) {
                problems.push(format!("city {field} = {value}: {status}"));
            }
            tried += 2;
        }
        let all = json!({ "min_member_days": value, "min_gatherings": value, "min_good_trips": value, "min_vouches": value, "rules_text": value });
        for (path, who) in [
            (format!("/api/houses/{}/rules", w.ann_home.id), &w.ann),
            (format!("/api/cities/{}/rules", w.city), &w.ann),
        ] {
            let status = server.status_of(&who.key, Method::PUT, &path, Some(all.clone())).await;
            if !(200..500).contains(&status) {
                problems.push(format!("{path} = {value}: {status}"));
            }
        }
        // The numbers that were already there and were never tried so hard.
        let routes: Vec<(Method, String, Value, &Who)> = vec![
            (
                Method::POST,
                "/api/trips".into(),
                json!({ "agent_id": pip.id, "house_id": w.ann_home.id, "kind": "call", "minutes": value }),
                &cy,
            ),
            (
                Method::POST,
                "/api/trips".into(),
                json!({ "house_id": w.ann_home.id, "kind": "visit", "minutes": value }),
                &cy,
            ),
            (
                Method::POST,
                format!("/api/trips/{trip}/extend"),
                json!({ "minutes": value }),
                &w.ann,
            ),
            (
                Method::POST,
                format!("/api/houses/{}/passes", w.ann_home.id),
                json!({ "agent_id": w.scout.id, "max_minutes": value }),
                &w.ann,
            ),
            (
                Method::POST,
                format!("/api/hangouts/{}/events", w.square),
                json!({ "title": "T", "starts_at": "2030-01-01T10:00:00Z", "duration_minutes": value }),
                &w.ann,
            ),
            (
                Method::POST,
                format!("/api/hangouts/{}/events", w.square),
                json!({ "title": "T", "starts_at": "2030-01-01T10:00:00Z", "min_yes": value }),
                &w.ann,
            ),
            (
                Method::POST,
                format!("/api/hangouts/{}/check-ins", w.square),
                json!({ "about": "a", "expires_minutes": value }),
                &w.ann,
            ),
        ];
        for (method, path, body, who) in routes {
            let status = server.status_of(&who.key, method.clone(), &path, Some(body)).await;
            if !(200..500).contains(&status) {
                problems.push(format!(
                    "{method} {path} with {value}: {status} (0 means the connection was dropped)"
                ));
            }
            tried += 1;
        }
        for query in [
            format!("after={value}"),
            format!("limit={value}"),
            format!("after={value}&limit={value}"),
        ] {
            let status = server
                .status_of(
                    &w.ann.key,
                    Method::GET,
                    &format!("/api/hangouts/{}/messages?{query}", w.square),
                    None,
                )
                .await;
            if !(200..500).contains(&status) {
                problems.push(format!("messages?{query}: {status}"));
            }
            tried += 1;
        }
        // Over MCP, where a number is read with as_i64 and a bad one is simply missing.
        let result = server
            .tool(
                &pip.token,
                "ask_for_trip",
                json!({ "house_id": w.ann_home.id, "kind": "call", "minutes": value }),
            )
            .await;
        if !result.is_object() {
            problems.push(format!("ask_for_trip with {value}: {result}"));
        }
        for tool in ["propose_hangout", "check_in"] {
            let reply = server.mcp(&w.scout.token, "tools/call", json!({ "name": tool, "arguments": { "hangout_id": w.square, "title": "x", "starts_at": "2030-01-01T10:00:00Z", "about": "a", "duration_minutes": value, "min_yes": value, "expires_minutes": value } })).await;
            if reply.status.is_server_error() {
                problems.push(format!("{tool} with {value}: {}", reply.show()));
            }
        }
        tried += 3;
    }
    assert!(tried > 300);
    problems.sort();
    problems.dedup();
    assert!(
        problems.is_empty(),
        "{} problems:
{}",
        problems.len(),
        problems.join(
            "
"
        )
    );
    // Nothing above left the rules at a number outside the limits.
    for house in [&w.ann_home.id, &w.bo_home.id] {
        let rules = server
            .house_view(if house == &w.ann_home.id { &w.ann.key } else { &w.bo.key }, house)
            .await
            .body["rules"]
            .clone();
        for (field, largest) in [
            ("min_member_days", 365),
            ("min_gatherings", 100),
            ("min_good_trips", 100),
            ("min_vouches", 5),
        ] {
            let set = rules[field].as_i64().unwrap();
            assert!((0..=largest).contains(&set), "{field} = {set}");
        }
    }
    let rules = server.city_rules_of(&w.ann.key, &w.city).await;
    assert!((0..=5).contains(&rules["min_vouches"].as_i64().unwrap()));
    assert!(server.status_of("", Method::GET, "/api/passkeys/available", None).await == 200);
}

// T6 T13 T17
#[tokio::test]
async fn ids_and_bodies_that_are_hostile_in_the_new_routes_are_refused_without_a_crash() {
    let server = Server::start().await;
    let w = server.world().await;
    server.require_vouches(&w.ann, &w.city, 1).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;
    let hostile_ids = [
        "".to_string(),
        " ".to_string(),
        "%00".to_string(),
        "..".to_string(),
        "../x".to_string(),
        "%2e%2e%2f".to_string(),
        "a b".to_string(),
        "é".to_string(),
        "🙂".to_string(),
        "x".repeat(10_000),
        "'; DROP TABLE vouches;--".to_string(),
        "%".to_string(),
        "\\".to_string(),
        "{{BASE_URL}}".to_string(),
        "null".to_string(),
        "-1".to_string(),
    ];
    for id in &hostile_ids {
        for (method, template) in [
            (Method::GET, "/api/cities/{city}/people/{id}/standing"),
            (Method::POST, "/api/cities/{city}/people/{id}/vouch"),
            (Method::DELETE, "/api/cities/{city}/people/{id}/vouch"),
            (Method::POST, "/api/cities/{city}/gate/{id}/decline"),
        ] {
            let path = template.replace("{city}", &w.city).replace("{id}", id);
            let status = server
                .status_of(
                    &w.ann.key,
                    method.clone(),
                    &path,
                    if method == Method::POST { Some(json!({})) } else { None },
                )
                .await;
            assert!(status != 0 && status < 500, "{method} {path:.80}: {status}");
        }
        for (method, template) in [
            (Method::PUT, "/api/cities/{id}/rules"),
            (Method::GET, "/api/cities/{id}/people/{id}/standing"),
            (Method::PUT, "/api/houses/{id}/rules"),
            (Method::GET, "/api/city-invites/{id}"),
            (Method::POST, "/api/city-invites/{id}/join"),
        ] {
            let path = template.replace("{id}", id);
            let status = server.status_of(&w.ann.key, method.clone(), &path, Some(json!({}))).await;
            assert!(status != 0 && status < 500, "{method} {path:.80}: {status}");
        }
    }
    // Nothing in all that disturbed the real gate.
    assert_eq!(server.gate(&w.ann.key, &w.city).await.len(), 1);
    assert_eq!(server.me(&eve.key).await["waiting"][0]["vouches"], 0);
    // Deep or huge bodies on the rules routes: 400 or 413, never a crash.
    let deep = format!("{}{}", "[".repeat(50_000), "]".repeat(50_000));
    let big = format!("{{\"rules_text\":\"{}\"}}", "x".repeat(100_000));
    for body in [deep, big, "{\"min_vouches\":".to_string(), String::new(), "null".to_string()] {
        for path in [
            format!("/api/cities/{}/rules", w.city),
            format!("/api/houses/{}/rules", w.ann_home.id),
        ] {
            let status =
                server.raw_status(&w.ann.key, Method::PUT, &path, body.clone().into_bytes()).await;
            assert!(status == 0 || (400..500).contains(&status), "{path}: {status}");
        }
    }
    assert_eq!(server.city_rules_of(&w.ann.key, &w.city).await["min_vouches"], 1);
}

// T3
#[tokio::test]
async fn a_second_account_is_the_one_way_to_vouch_for_yourself_and_a_second_login_is_not() {
    let server = Server::start().await;
    let w = server.world().await;
    // Mallory is in the city with a passkey account. Then the city asks for two vouches, and she makes
    // a second account, which arrives at the gate.
    let first = server.sign_up("Mallory").await;
    assert_ok(&server.use_link(&w.code, &first.who.key, json!({})).await);
    server.require_vouches(&w.ann, &w.city, 2).await;
    let second = server.sign_up("Mallory").await;
    let arrived = server.use_link(&w.code, &second.who.key, json!({})).await;
    assert_eq!(arrived.body["waiting"], true);

    // One person with three ways of being signed in is still one vouch.
    let session = first.who.key.strip_prefix("cookie:").unwrap().to_string();
    for credential in [&first.who.key, &session, &first.owner_key] {
        let vouched = server.vouch(credential, &w.city, &second.who.id).await;
        assert_ok(&vouched);
        assert_eq!(
            (vouched.body["vouches"].as_i64(), vouched.body["let_in"].as_bool()),
            (Some(1), Some(false)),
            "{}",
            vouched.body
        );
    }
    // The new account can't vouch for itself or for anyone while it waits.
    assert_not_found(&server.vouch(&second.who.key, &w.city, &second.who.id).await);
    assert_not_found(&server.vouch(&second.who.key, &w.city, &first.who.id).await);
    assert_not_found(&server.city_view(&second.who.key, &w.city).await);
    // So with two asked for, one person is stuck at one. With one asked for, the sock puppet is enough,
    // as the doc says it is: their name is on the vouch, and that is all there is to check.
    assert_ok(&server.set_city_rules(&w.ann.key, &w.city, json!({ "min_vouches": 1 })).await);
    assert_eq!(
        server.stand(&w.ann.key, &w.city, &second.who.id).await["vouches"],
        json!(["Mallory"])
    );
    assert_ok(&server.city_view(&second.who.key, &w.city).await);
}

// T4 T11
#[tokio::test]
async fn a_house_counts_once_a_day_however_many_short_trips_a_bot_loops_and_each_house_counts_on_its_own(
) {
    let server = Server::start().await;
    let w = server.world().await;
    let (cy, cy_home) = server.neighbour(&w, "Cy").await;
    let first = server.arrive(&w.scout, &w.bo, &w.ann, &w.ann_home.id).await;
    assert_ok(&server.end_trip(&w.scout.token, &first).await);
    // Looping short trips on a pass earns nothing more.
    let pass = server.offer_and_accept_pass(&w.ann, &w.bo, &w.ann_home.id, &w.scout.id).await;
    for _ in 0..6 {
        let trip = server.ask_trip(&w.scout.token, None, &w.ann_home.id, Some(1)).await;
        assert_eq!(trip.body["status"], "here", "{}", trip.body);
        assert_ok(&server.end_trip(&w.scout.token, &text(&trip.body["id"])).await);
    }
    assert_ok(&server.delete(&w.ann.key, &format!("/api/passes/{pass}")).await);
    assert_eq!(
        server.stand(&w.ann.key, &w.city, &w.bo.id).await["good_trips"],
        1,
        "seven trips to one house in a day"
    );
    // Another house is another count, and a third person's house too.
    let second = server.arrive(&w.scout, &w.bo, &cy, &cy_home.id).await;
    assert_ok(&server.end_trip(&w.scout.token, &second).await);
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["good_trips"], 2);
    // A requirement of two is met by two houses, and one house alone could never meet it today.
    server.require(&w.ann, &w.ann_home, json!({ "min_good_trips": 2 })).await;
    let (dee, dee_home) = server.neighbour(&w, "Dee").await;
    let dee_bot = server.bot(&dee, &dee_home.living, "Dash").await;
    let only = server.arrive(&dee_bot, &dee, &cy, &cy_home.id).await;
    assert_ok(&server.end_trip(&dee_bot.token, &only).await);
    assert_status(
        &server.ask_trip(&dee_bot.token, None, &w.ann_home.id, None).await,
        StatusCode::CONFLICT,
    );
    assert_ok(&server.ask_trip(&w.scout.token, None, &w.ann_home.id, None).await);
}

// R36 T16
#[tokio::test]
async fn the_city_page_holds_the_rules_and_requirements_but_nothing_private_to_a_house() {
    let server = Server::start().await;
    let w = server.world().await;
    server.require(&w.ann, &w.ann_home, json!({ "min_gatherings": 2 })).await;
    server.require_vouches(&w.ann, &w.city, 1).await;
    let eve = server.at_the_gate(&w.code, "Eve").await;

    let view = server.city_view(&w.bo.key, &w.city).await.body;
    assert_eq!(
        keys_of(&view),
        [
            "bot_looks",
            "bots",
            "city",
            "gate",
            "growth",
            "houses",
            "people",
            "rules",
            "vouches_needed",
            "you",
            "your_passes"
        ]
    );
    // C13: nobody here chose a look.
    assert_eq!(view["bot_looks"], json!({}));
    // H21: only passes of Bo's own bots, and he has none here.
    assert_eq!(view["your_passes"], json!([]));
    assert_eq!(keys_of(&view["rules"]), ["min_vouches", "rules_text"]);
    for house in view["houses"].as_array().unwrap() {
        assert_eq!(keys_of(house), ["id", "name", "requirements", "residents"]);
        assert_eq!(
            keys_of(&house["requirements"]),
            ["min_gatherings", "min_good_trips", "min_member_days", "min_vouches"]
        );
    }
    for person in view["people"].as_array().unwrap() {
        assert_eq!(keys_of(person), ["color", "id", "name"]);
    }
    for secret in [
        w.ann_home.living.as_str(),
        w.ann_home.porch.as_str(),
        w.bo_home.living.as_str(),
        w.bo_home.porch.as_str(),
        TRIP_GOAL,
        "owner_key",
        "invite_code",
        &eve.key,
    ] {
        assert!(!has(&view, secret), "{secret} in the city page");
    }
    // The gate shows Eve to the people in the city, and the bot's page has neither the gate nor her.
    assert_eq!(view["gate"][0]["name"], "Eve");
    let by_bot = server.city_view(&w.scout.token, &w.city).await.body;
    assert_eq!(
        keys_of(&by_bot),
        [
            "bot_looks",
            "bots",
            "city",
            "growth",
            "houses",
            "people",
            "rules",
            "vouches_needed",
            "you",
            "your_passes"
        ]
    );
    assert_eq!(by_bot["your_passes"], json!([]));
}

// A7
#[tokio::test]
async fn an_expired_session_is_a_401_on_every_kind_of_route_and_is_never_a_bot() {
    let server = Server::start().await;
    let w = server.world().await;
    // Sessions that ended a second ago, and at this very second, for a person who is in a city.
    let token = auth::new_session_token();
    server
        .state
        .db
        .create_session(&auth::hash_secret(&token), &w.ann.id, now() - 1, now() - 100)
        .unwrap();
    let edge = auth::new_session_token();
    server
        .state
        .db
        .create_session(&auth::hash_secret(&edge), &w.ann.id, now(), now() - 100)
        .unwrap();
    let routes: Vec<(Method, String, Option<Value>)> = vec![
        (Method::GET, "/api/me".into(), None),
        (Method::GET, "/api/inbox".into(), None),
        (Method::GET, "/api/passkeys".into(), None),
        (Method::DELETE, "/api/passkeys/anything".into(), None),
        (Method::POST, "/api/key/rotate".into(), Some(json!({}))),
        (Method::GET, format!("/api/cities/{}", w.city), None),
        (Method::GET, format!("/api/cities/{}/stream", w.city), None),
        (Method::GET, format!("/api/hangouts/{}/stream", w.square), None),
        (Method::GET, format!("/api/houses/{}", w.ann_home.id), None),
        (
            Method::PUT,
            format!("/api/houses/{}/rules", w.ann_home.id),
            Some(json!({ "min_gatherings": 1 })),
        ),
        (Method::PUT, format!("/api/cities/{}/rules", w.city), Some(json!({ "min_vouches": 1 }))),
        (Method::POST, format!("/api/cities/{}/people/{}/vouch", w.city, w.bo.id), Some(json!({}))),
        (
            Method::POST,
            "/mcp".into(),
            Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })),
        ),
    ];
    for session in [&token, &edge] {
        for (method, path, body) in &routes {
            for carrier in [format!("cookie:{session}"), session.to_string()] {
                let reply = server.request(&carrier, method.clone(), path, body.clone()).await;
                assert_eq!(
                    reply.status,
                    StatusCode::UNAUTHORIZED,
                    "{method} {path}: {}",
                    reply.show()
                );
            }
        }
    }
    // Nothing the expired ones tried changed anything.
    assert_eq!(server.city_rules_of(&w.ann.key, &w.city).await["min_vouches"], 0);
    assert_eq!(server.stand(&w.ann.key, &w.city, &w.bo.id).await["vouches"], json!([]));
}

// A9
#[tokio::test]
async fn a_passkey_cannot_be_added_to_another_account_by_a_cookie_a_ceremony_id_or_a_body() {
    let server = Server::start().await;
    let ann = server.sign_up("Ann").await;
    let bo = server.sign_up("Bo").await;
    let mut eve_key = Authenticator::new(&server.origin);

    // Ann starts adding one. Eve, with nothing, with Bo's session, with Bo's key, and naming Bo in the
    // body, can't finish it, and trying spends it so not even Ann can finish it afterwards.
    for (who, why) in
        [("", "nobody"), (bo.who.key.as_str(), "Bo's session"), (bo.owner_key.as_str(), "Bo's key")]
    {
        let started = server
            .post(&ann.who.key, "/api/passkeys/register/start", json!({ "label": "Mine" }))
            .await;
        assert_ok(&started);
        let id = text(&started.body["ceremony_id"]);
        let credential = eve_key.register(&started.body["options"]);
        let body = json!({ "ceremony_id": id, "credential": credential, "person_id": bo.who.id, "user_handle": "x", "owner_key": bo.owner_key });
        let reply = server.post(who, "/api/passkeys/register/finish", body).await;
        assert!(reply.status.is_client_error(), "{why}: {}", reply.show());
        let late = server
            .post(
                &ann.who.key,
                "/api/passkeys/register/finish",
                json!({ "ceremony_id": id, "credential": credential }),
            )
            .await;
        assert_status(&late, StatusCode::BAD_REQUEST);
    }
    // Nothing was added anywhere, and Eve's passkey signs nobody in.
    for who in [&ann.who, &bo.who] {
        assert_eq!(server.me(&who.key).await["passkeys"], 1);
    }
    let login = server.post("", "/api/passkeys/login/start", json!({})).await;
    let answer = eve_key.sign_in(&login.body["options"]);
    let reply = server
        .post(
            "",
            "/api/passkeys/login/finish",
            json!({ "ceremony_id": login.body["ceremony_id"], "credential": answer }),
        )
        .await;
    assert_status(&reply, StatusCode::UNAUTHORIZED);
}

// T17 T18
// A leaked link can fill the gate with fifty strangers who then hold their places for fourteen days.
// Replacing the link stops more coming but doesn't clear them, so the keeper clears the gate, and then
// real newcomers with the new link can reach it.
#[tokio::test]
async fn replacing_the_link_after_a_leak_lets_real_newcomers_through_a_gate_that_strangers_filled()
{
    let server = Server::start().await;
    let (city, _, code, ann) = server.new_city("Elm", "Ann").await;
    server.join_city(&code, "Bo").await;
    server.require_vouches(&ann, &city, 1).await;
    for n in 0..50 {
        server.at_the_gate(&code, &format!("Stranger {n}")).await;
    }
    assert_eq!(server.gate(&ann.key, &city).await.len(), 50);

    let rotated =
        server.post(&ann.key, &format!("/api/cities/{city}/invite/rotate"), json!({})).await;
    assert_ok(&rotated);
    let new_code = code_of(&rotated.body["invite_url"]);
    let friend = json!({ "your_name": "A real friend", "accept_rules": true });
    assert_not_found(&server.use_link(&code, "", friend.clone()).await);

    // The strangers still fill the gate, so the new link finds it full and makes no account.
    let people = server.state.db.person_count().unwrap();
    assert_status(&server.use_link(&new_code, "", friend.clone()).await, StatusCode::CONFLICT);
    assert_eq!(server.state.db.person_count().unwrap(), people);

    // The keeper turns them all away at once, and the friend reaches the gate.
    let cleared = server.clear_gate(&ann.key, &city).await;
    assert_eq!(cleared.body, json!({ "ok": true, "cleared": 50 }));
    assert!(server.gate(&ann.key, &city).await.is_empty());
    let arrived = server.use_link(&new_code, "", friend).await;
    assert_ok(&arrived);
    assert_eq!(arrived.body["waiting"], true, "{}", arrived.body);
    let waiting = server.gate(&ann.key, &city).await;
    assert_eq!(
        waiting.iter().map(|entry| text(&entry["name"])).collect::<Vec<_>>(),
        ["A real friend"]
    );
}
