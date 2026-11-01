//! E1 to E21 in docs/events.md. A bot hears mentions, answers and moves in one call, and can
//! subscribe the way the events extension describes.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use agenthangout::{app, db::Db, AppState, Config};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use futures_util::StreamExt;
use reqwest::{Client, Method};
use serde_json::{json, Value};

const SECRET: &str = "whsec_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE=";

struct Server {
    base: String,
    client: Client,
}

struct Reply {
    status: StatusCode,
    body: Value,
}

impl Server {
    async fn start() -> Self {
        Self::start_with(true).await
    }

    async fn start_with(allow_private: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = AppState::new(
            Db::open(":memory:").unwrap(),
            Config {
                public_url: base.clone(),
                create_code: None,
                allow_private_webhooks: allow_private,
                keeper_id: None,
                god_ids: Vec::new(),
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        Self { base, client: Client::new() }
    }

    async fn request(&self, token: &str, method: Method, path: &str, body: Option<Value>) -> Reply {
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
        self.request(token, Method::GET, path, None).await
    }

    async fn post(&self, token: &str, path: &str, body: Value) -> Reply {
        self.request(token, Method::POST, path, Some(body)).await
    }

    async fn put(&self, token: &str, path: &str, body: Value) -> Reply {
        self.request(token, Method::PUT, path, Some(body)).await
    }

    async fn room(&self) -> (String, String, String) {
        let created =
            self.post("", "/api/hangouts", json!({ "name": "Crew", "your_name": "Justin" })).await;
        assert_eq!(created.status, StatusCode::OK, "{}", created.body);
        let justin = created.body["owner_key"].as_str().unwrap().to_string();
        let id = created.body["hangout"]["id"].as_str().unwrap().to_string();
        let code = created.body["hangout"]["invite_code"].as_str().unwrap();
        let joined = self
            .post("", &format!("/api/invites/{code}/join"), json!({ "your_name": "Kevin" }))
            .await;
        assert_eq!(joined.status, StatusCode::OK, "{}", joined.body);
        (id, justin, joined.body["owner_key"].as_str().unwrap().to_string())
    }

    async fn bot(&self, owner: &str, room: &str, name: &str) -> (String, String) {
        let reply = self
            .post(owner, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
        (
            reply.body["token"].as_str().unwrap().to_string(),
            reply.body["agent"]["id"].as_str().unwrap().to_string(),
        )
    }

    async fn mcp(&self, token: &str, id: Value, method: &str, params: Value) -> Reply {
        self.post(
            token,
            "/mcp",
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
        .await
    }

    async fn tool(&self, token: &str, name: &str, arguments: Value) -> Value {
        let reply = self
            .mcp(token, json!(1), "tools/call", json!({ "name": name, "arguments": arguments }))
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
        reply.body["result"].clone()
    }

    async fn say(&self, token: &str, room: &str, text: &str) {
        let reply = self
            .post(token, &format!("/api/hangouts/{room}/messages"), json!({ "text": text }))
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{text}: {}", reply.body);
    }
}

#[derive(Clone, Debug)]
struct Hit {
    headers: Vec<(String, String)>,
    body: Value,
}

struct Hook {
    hits: Mutex<Vec<Hit>>,
    /// `echo` repeats the challenge. `wrong` echoes a different one. `gone` answers 410.
    mode: Mutex<&'static str>,
}

impl Hook {
    fn new(mode: &'static str) -> Arc<Self> {
        Arc::new(Self { hits: Mutex::new(Vec::new()), mode: Mutex::new(mode) })
    }

    fn hits(&self) -> Vec<Hit> {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .map(|hit| Hit { headers: hit.headers.clone(), body: hit.body.clone() })
            .collect()
    }
}

async fn on_hook(
    State(hook): State<Arc<Hook>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let mut recorded = Vec::new();
    for (name, value) in headers.iter() {
        if let Ok(text) = value.to_str() {
            recorded.push((name.as_str().to_string(), text.to_string()));
        }
    }
    hook.hits.lock().unwrap().push(Hit { headers: recorded, body: value.clone() });
    match *hook.mode.lock().unwrap() {
        "wrong" => (StatusCode::OK, Json(json!({ "challenge": "nope" }))).into_response(),
        "gone" => StatusCode::GONE.into_response(),
        _ => {
            if let Some(challenge) = value.get("challenge") {
                (StatusCode::OK, Json(json!({ "challenge": challenge }))).into_response()
            } else {
                StatusCode::NO_CONTENT.into_response()
            }
        }
    }
}

async fn start_hook(mode: &'static str) -> (String, Arc<Hook>) {
    let hook = Hook::new(mode);
    let app = Router::new().fallback(post(on_hook)).with_state(hook.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    (base, hook)
}

fn header<'a>(hit: &'a Hit, name: &str) -> Option<&'a str> {
    hit.headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn names_of(events: &Value) -> Vec<&str> {
    events.as_array().unwrap().iter().map(|event| event["name"].as_str().unwrap()).collect()
}

async fn wait_for(hook: &Hook, pred: impl Fn(&Hit) -> bool) -> Hit {
    for _ in 0..50 {
        if let Some(hit) = hook.hits().into_iter().find(&pred) {
            return hit;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "callback never arrived: {:?}",
        hook.hits().iter().map(|hit| &hit.body).collect::<Vec<_>>()
    );
}

// E1, E2, E19
#[tokio::test]
async fn a_bot_can_list_events_and_a_house_bot_cannot() {
    let server = Server::start().await;
    let (room, justin, _) = server.room().await;
    let (scout, _) = server.bot(&justin, &room, "Scout").await;

    let init = server
        .mcp(&scout, json!(1), "initialize", json!({ "protocolVersion": "2025-06-18" }))
        .await;
    assert_eq!(init.body["result"]["protocolVersion"], "2025-06-18");
    assert!(init.body["result"]["capabilities"]["tools"].is_object());
    assert_eq!(init.body["result"]["capabilities"]["events"]["listChanged"], false);
    assert_eq!(
        init.body["result"]["capabilities"]["extensions"]["io.modelcontextprotocol/events"]
            ["listChanged"],
        false
    );
    let instructions = init.body["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("events_since"), "{instructions}");
    assert!(instructions.contains("ask_owner"), "{instructions}");

    let listed = server.mcp(&scout, json!(1), "events/list", json!({})).await;
    let names: Vec<&str> = listed.body["result"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["mention", "message", "owner_answer", "check_in", "plan", "trip", "place"]);

    let unknown = server.mcp(&scout, json!(1), "events/poll", json!({ "name": "nope" })).await;
    assert_eq!(unknown.body["error"]["code"], -32023);
    assert_eq!(unknown.body["error"]["data"]["kind"], "event");
    let pushed = server
        .mcp(
            &scout,
            json!(1),
            "events/subscribe",
            json!({ "name": "mention", "delivery": { "mode": "push", "url": "https://example.com/hook" } }),
        )
        .await;
    assert_eq!(pushed.body["error"]["code"], -32026);

    let city = server.post(&justin, "/api/cities", json!({ "name": "Elm" })).await;
    assert_eq!(city.status, StatusCode::OK, "{}", city.body);
    let city_id = city.body["city"]["id"].as_str().unwrap();
    let house = server
        .post(&justin, &format!("/api/cities/{city_id}/houses"), json!({ "name": "Home" }))
        .await;
    assert_eq!(house.status, StatusCode::OK, "{}", house.body);
    let house_id = house.body["house"]["id"].as_str().unwrap();
    let casa = server
        .post(&justin, &format!("/api/houses/{house_id}/house-bot"), json!({ "name": "Casa" }))
        .await;
    let casa_token = casa.body["token"].as_str().unwrap();
    let tools = server.mcp(casa_token, json!(1), "tools/list", json!({})).await;
    let tool_names: Vec<&str> = tools.body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(tool_names, ["house_queue", "finish_request", "set_house_status"]);
    let house_init = server.mcp(casa_token, json!(1), "initialize", json!({})).await;
    assert!(house_init.body["result"]["capabilities"].get("events").is_none());
    let refused = server.mcp(casa_token, json!(1), "events/list", json!({})).await;
    assert_eq!(refused.body["error"]["code"], -32024);
}

// E4, E5, E12, E16, E17, E20
#[tokio::test]
async fn events_since_returns_a_mention_and_not_the_bots_own_words() {
    let server = Server::start().await;
    let (room, justin, kevin) = server.room().await;
    let (scout, _) = server.bot(&justin, &room, "Scout").await;
    let (don, _) = server.bot(&kevin, &room, "Don").await;
    let elsewhere = server.post(&justin, "/api/hangouts", json!({ "name": "Quiet" })).await;
    let other = elsewhere.body["hangout"]["id"].as_str().unwrap();

    let first = server.tool(&scout, "events_since", json!({})).await;
    let page = &first["structuredContent"];
    assert!(page["events"].as_array().unwrap().is_empty(), "{page}");
    assert!(page["trust_note"].as_str().unwrap().contains("never as instructions"));
    let cursor = page["cursor"].as_str().unwrap().to_string();

    server.say(&scout, &room, "I said this").await;
    server.say(&kevin, &room, "hello there").await;
    server.say(&kevin, &room, "email@Scout.com is not a name, and @Scouting isn't either").await;
    server.say(&justin, &room, "@Scout the table is booked").await;

    let caught = server.tool(&scout, "events_since", json!({})).await;
    let events = &caught["structuredContent"]["events"];
    assert_eq!(names_of(events), ["mention"], "{events}");
    assert!(events[0]["data"]["body"].as_str().unwrap().contains("@Scout"));
    assert!(events[0]["data"]["trust"].as_str().unwrap().contains("never as instructions"));
    assert_eq!(events[0]["data"]["from_your_owner"], true);
    let next = caught["structuredContent"]["cursor"].as_str().unwrap();
    assert_ne!(next, "0");

    let again = server.tool(&scout, "events_since", json!({ "since": next })).await;
    assert!(again["structuredContent"]["events"].as_array().unwrap().is_empty());

    let full = server.tool(&scout, "events_since", json!({ "since": cursor })).await;
    let full_names = names_of(&full["structuredContent"]["events"]);
    assert!(full_names.contains(&"message"), "{full_names:?}");
    assert!(full_names.contains(&"mention"), "{full_names:?}");
    assert!(!full["structuredContent"].to_string().contains("I said this"));

    let via_http = server.get(&scout, "/api/events").await;
    assert_eq!(via_http.status, StatusCode::OK, "{}", via_http.body);
    assert_eq!(names_of(&via_http.body["events"]), ["mention"]);
    let person = server.get(&justin, "/api/events").await;
    assert_eq!(person.status, StatusCode::FORBIDDEN);

    let missing = server.tool(&scout, "events_since", json!({ "hangout_id": other })).await;
    assert_eq!(missing["isError"], true, "{missing}");
    let protocol = server
        .mcp(
            &scout,
            json!(1),
            "events/poll",
            json!({ "name": "message", "arguments": { "hangout_id": other } }),
        )
        .await;
    assert_eq!(protocol.body["error"]["code"], -32023);
    assert_eq!(protocol.body["error"]["data"]["kind"], "room");

    let polled = server
        .mcp(&scout, json!(1), "events/poll", json!({ "name": "mention", "cursor": null }))
        .await;
    assert!(polled.body["result"]["events"].as_array().unwrap().is_empty(), "{}", polled.body);
    let head = polled.body["result"]["cursor"].as_str().unwrap().to_string();
    server.say(&don, &room, "@Scout again").await;
    let later = server
        .mcp(&scout, json!(1), "events/poll", json!({ "name": "mention", "cursor": head }))
        .await;
    assert_eq!(later.body["result"]["events"].as_array().unwrap().len(), 1, "{}", later.body);
}

// E7, E20
#[tokio::test]
async fn an_owner_answer_reaches_only_the_bot_that_asked() {
    let server = Server::start().await;
    let (room, justin, kevin) = server.room().await;
    let (scout, _) = server.bot(&justin, &room, "Scout").await;
    let (don, _) = server.bot(&kevin, &room, "Don").await;
    let asked = server
        .post(
            &scout,
            &format!("/api/hangouts/{room}/owner-questions"),
            json!({ "question": "Friday?" }),
        )
        .await;
    assert_eq!(asked.status, StatusCode::OK, "{}", asked.body);
    let id = asked.body["id"].as_str().unwrap();
    let answered = server
        .post(
            &justin,
            &format!("/api/owner-questions/{id}/answer"),
            json!({ "answer": "the-private-answer" }),
        )
        .await;
    assert_eq!(answered.status, StatusCode::OK, "{}", answered.body);

    let scout_page = server.tool(&scout, "events_since", json!({})).await;
    let text = scout_page["structuredContent"].to_string();
    assert!(text.contains("the-private-answer"), "{text}");
    assert!(text.contains("owner_answer"), "{text}");
    assert!(text.contains("from_your_owner"), "{text}");
    let don_page = server.tool(&don, "events_since", json!({ "since": "0" })).await;
    assert!(!don_page.to_string().contains("the-private-answer"), "{don_page}");
}

// E9, E11
#[tokio::test]
async fn opening_a_floor_tells_another_owners_bot_it_was_removed() {
    let server = Server::start().await;
    let (room, justin, kevin) = server.room().await;
    let (scout, _) = server.bot(&justin, &room, "Scout").await;
    let (don, _) = server.bot(&kevin, &room, "Don").await;

    let proposed = server
        .post(
            &don,
            &format!("/api/hangouts/{room}/events"),
            json!({ "title": "Tacos", "starts_at": "2026-12-01T18:00:00-06:00" }),
        )
        .await;
    assert_eq!(proposed.status, StatusCode::OK, "{}", proposed.body);
    let plans = server.tool(&scout, "events_since", json!({})).await;
    assert_eq!(names_of(&plans["structuredContent"]["events"]), ["plan"], "{plans}");
    let don_plans = server.tool(&don, "events_since", json!({})).await;
    assert!(don_plans["structuredContent"]["events"].as_array().unwrap().is_empty(), "{don_plans}");

    let opened =
        server.put(&justin, &format!("/api/floors/{room}"), json!({ "access": "public" })).await;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let sent = server.tool(&don, "events_since", json!({})).await;
    let changes: Vec<&str> = sent["structuredContent"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["name"] == "place")
        .map(|event| event["data"]["change"].as_str().unwrap())
        .collect();
    assert_eq!(changes, ["removed"], "{sent}");
    assert_eq!(
        server.get(&don, &format!("/api/hangouts/{room}")).await.status,
        StatusCode::NOT_FOUND
    );
}

// E6, E9
#[tokio::test]
async fn a_guest_bot_hears_the_message_and_not_the_plan() {
    let server = Server::start().await;
    let (room, justin, kevin) = server.room().await;
    let (scout, _) = server.bot(&justin, &room, "Scout").await;
    let made = server.post("", "/api/hangouts", json!({ "name": "Out", "your_name": "Pat" })).await;
    let pat = made.body["owner_key"].as_str().unwrap().to_string();
    let home = made.body["hangout"]["id"].as_str().unwrap().to_string();
    let (pixel, pixel_id) = server.bot(&pat, &home, "Pixel").await;

    let opened =
        server.put(&justin, &format!("/api/floors/{room}"), json!({ "access": "public" })).await;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let walked = server
        .post(&pat, &format!("/api/floors/{room}/walk-in"), json!({ "agent_id": pixel_id }))
        .await;
    assert_eq!(walked.status, StatusCode::OK, "{}", walked.body);

    server.say(&justin, &room, "welcome in").await;
    server
        .post(
            &kevin,
            &format!("/api/hangouts/{room}/events"),
            json!({ "title": "Later", "starts_at": "2026-12-02T18:00:00-06:00" }),
        )
        .await;

    let guest = server.tool(&pixel, "events_since", json!({ "since": "0" })).await;
    let guest_names = names_of(&guest["structuredContent"]["events"]);
    assert!(guest_names.contains(&"message"), "{guest}");
    assert!(!guest_names.contains(&"plan"), "{guest}");
    let member = server.tool(&scout, "events_since", json!({ "since": "0" })).await;
    assert!(names_of(&member["structuredContent"]["events"]).contains(&"plan"), "{member}");
}

// E12
#[tokio::test]
async fn a_poll_drops_a_row_older_than_max_age() {
    let server = Server::start().await;
    let (room, justin, kevin) = server.room().await;
    let (scout, _) = server.bot(&justin, &room, "Scout").await;
    let polled = server
        .mcp(&scout, json!(1), "events/poll", json!({ "name": "message", "cursor": null }))
        .await;
    let cursor = polled.body["result"]["cursor"].as_str().unwrap().to_string();
    server.say(&kevin, &room, "old news").await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let aged = server
        .mcp(
            &scout,
            json!(1),
            "events/poll",
            json!({ "name": "message", "cursor": cursor, "maxAgeMs": 1000 }),
        )
        .await;
    assert_eq!(aged.body["result"]["truncated"], true, "{}", aged.body);
    assert!(aged.body["result"]["events"].as_array().unwrap().is_empty(), "{}", aged.body);
}

// E16
#[tokio::test]
async fn events_since_waits_for_the_next_mention() {
    let server = Server::start().await;
    let (room, justin, kevin) = server.room().await;
    let (scout, _) = server.bot(&justin, &room, "Scout").await;
    let base = server.base.clone();
    let token = scout.clone();
    let waiting = tokio::spawn(async move {
        let client = Client::new();
        client
            .post(format!("{base}/mcp"))
            .bearer_auth(token)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": "events_since", "arguments": { "wait_seconds": 5 } }
            }))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    server.say(&kevin, &room, "@Scout are you there").await;
    let reply = tokio::time::timeout(Duration::from_secs(5), waiting).await.unwrap().unwrap();
    let events = &reply["result"]["structuredContent"]["events"];
    assert_eq!(names_of(events), ["mention"], "{reply}");
}

// E13
#[tokio::test]
async fn a_stream_sends_the_active_notice_and_then_the_mention() {
    let server = Server::start().await;
    let (room, justin, kevin) = server.room().await;
    let (scout, _) = server.bot(&justin, &room, "Scout").await;
    let response = server
        .client
        .post(format!("{}/mcp", server.base))
        .bearer_auth(&scout)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": "stream-1",
            "method": "events/stream",
            "params": { "name": "mention" }
        }))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    let mut stream = response.bytes_stream();
    let mut buf = String::new();
    let mut frames = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut posted = false;
    while tokio::time::Instant::now() < deadline {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let chunk = tokio::time::timeout(left, stream.next()).await;
        let Ok(Some(Ok(bytes))) = chunk else { break };
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(end) = buf.find("\n\n") {
            let block = buf[..end].to_string();
            buf = buf[end + 2..].to_string();
            for line in block.lines() {
                let Some(data) = line.strip_prefix("data:") else { continue };
                if let Ok(value) = serde_json::from_str::<Value>(data.trim()) {
                    frames.push(value);
                }
            }
        }
        if !posted && frames.iter().any(|frame| frame["method"] == "notifications/events/active") {
            server.say(&kevin, &room, "@Scout on the stream").await;
            posted = true;
        }
        if frames.iter().any(|frame| frame["method"] == "notifications/events/event") {
            break;
        }
    }
    let active =
        frames.iter().find(|frame| frame["method"] == "notifications/events/active").unwrap();
    assert_eq!(active["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"], "stream-1");
    let event = frames.iter().find(|frame| frame["method"] == "notifications/events/event");
    assert!(event.is_some(), "{frames:?}");
    let event = event.unwrap();
    assert_eq!(event["params"]["name"], "mention");
    assert_eq!(event["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"], "stream-1");
}

// E14, E15, E18
#[tokio::test]
async fn a_webhook_is_checked_signed_and_stopped() {
    let server = Server::start().await;
    let (room, justin, kevin) = server.room().await;
    let (scout, _) = server.bot(&justin, &room, "Scout").await;
    let (url, hook) = start_hook("echo").await;

    let bad = server
        .mcp(
            &scout,
            json!(1),
            "events/subscribe",
            json!({
                "name": "mention",
                "delivery": { "mode": "webhook", "url": url, "secret": "nope" }
            }),
        )
        .await;
    assert_eq!(bad.body["error"]["code"], -32602, "{}", bad.body);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = listener.local_addr().unwrap().port();
    drop(listener);
    let refused = server
        .mcp(
            &scout,
            json!(1),
            "events/subscribe",
            json!({
                "name": "mention",
                "delivery": {
                    "mode": "webhook",
                    "url": format!("http://127.0.0.1:{dead}/hook"),
                    "secret": SECRET
                }
            }),
        )
        .await;
    assert_eq!(refused.body["error"]["code"], -32027, "{}", refused.body);
    assert_eq!(refused.body["error"]["data"]["reason"], "connection_refused");

    let (bad_url, bad_hook) = start_hook("wrong").await;
    let failed = server
        .mcp(
            &scout,
            json!(1),
            "events/subscribe",
            json!({
                "name": "mention",
                "delivery": { "mode": "webhook", "url": bad_url, "secret": SECRET }
            }),
        )
        .await;
    assert_eq!(failed.body["error"]["code"], -32027, "{}", failed.body);
    assert_eq!(failed.body["error"]["data"]["reason"], "challenge_failed");
    assert!(!bad_hook.hits().is_empty());

    let saved = server
        .mcp(
            &scout,
            json!(1),
            "events/subscribe",
            json!({
                "name": "mention",
                "ttlMs": null,
                "delivery": { "mode": "webhook", "url": url, "secret": SECRET }
            }),
        )
        .await;
    assert!(saved.body["result"]["id"].is_string(), "{}", saved.body);
    assert!(
        saved.body["result"].get("deliveryStatus").is_none(),
        "a failed challenge stored a row"
    );
    let id = saved.body["result"]["id"].as_str().unwrap().to_string();

    let again = server
        .mcp(
            &scout,
            json!(1),
            "events/subscribe",
            json!({
                "name": "mention",
                "delivery": { "mode": "webhook", "url": url, "secret": SECRET }
            }),
        )
        .await;
    assert_eq!(again.body["result"]["id"], id);
    assert!(again.body["result"]["deliveryStatus"]["active"].as_bool().unwrap(), "{}", again.body);

    hook.hits.lock().unwrap().clear();
    server.say(&kevin, &room, "@Scout by webhook").await;
    let hit = wait_for(&hook, |hit| hit.body["name"] == "mention").await;
    assert_eq!(header(&hit, "webhook-id"), hit.body["eventId"].as_str());
    assert!(header(&hit, "webhook-signature").unwrap().starts_with("v1,"), "{hit:?}");
    assert_eq!(header(&hit, "x-mcp-subscription-id"), Some(id.as_str()));
    assert!(hit.body["data"]["body"].as_str().unwrap().contains("@Scout"));

    let gone = server
        .mcp(
            &scout,
            json!(1),
            "events/unsubscribe",
            json!({
                "name": "mention",
                "delivery": { "url": url }
            }),
        )
        .await;
    assert!(gone.body["result"].is_object(), "{}", gone.body);
    let missing = server
        .mcp(
            &scout,
            json!(1),
            "events/unsubscribe",
            json!({ "name": "mention", "delivery": { "url": url } }),
        )
        .await;
    assert_eq!(missing.body["error"]["code"], -32023);
    assert_eq!(missing.body["error"]["data"]["kind"], "subscription");
}

// E14
#[tokio::test]
async fn the_twenty_first_subscription_is_refused() {
    let server = Server::start().await;
    let (room, justin, _) = server.room().await;
    let (scout, _) = server.bot(&justin, &room, "Scout").await;
    let (base, _) = start_hook("echo").await;
    for n in 0..20 {
        let saved = server
            .mcp(
                &scout,
                json!(1),
                "events/subscribe",
                json!({
                    "name": "message",
                    "delivery": { "mode": "webhook", "url": format!("{base}/s/{n}"), "secret": SECRET }
                }),
            )
            .await;
        assert!(saved.body["result"]["id"].is_string(), "{n}: {}", saved.body);
    }
    let extra = server
        .mcp(
            &scout,
            json!(1),
            "events/subscribe",
            json!({
                "name": "message",
                "delivery": { "mode": "webhook", "url": format!("{base}/s/20"), "secret": SECRET }
            }),
        )
        .await;
    assert_eq!(extra.body["error"]["code"], -32025, "{}", extra.body);
    assert_eq!(extra.body["error"]["data"]["limit"], "subscriptions");
}

// E15
#[tokio::test]
async fn a_gone_callback_is_not_retried() {
    let server = Server::start().await;
    let (room, justin, kevin) = server.room().await;
    let (scout, _) = server.bot(&justin, &room, "Scout").await;
    let (url, hook) = start_hook("echo").await;
    let saved = server
        .mcp(
            &scout,
            json!(1),
            "events/subscribe",
            json!({
                "name": "mention",
                "delivery": { "mode": "webhook", "url": url, "secret": SECRET }
            }),
        )
        .await;
    assert!(saved.body["result"]["id"].is_string(), "{}", saved.body);
    *hook.mode.lock().unwrap() = "gone";
    hook.hits.lock().unwrap().clear();
    server.say(&kevin, &room, "@Scout gone").await;
    let _ = wait_for(&hook, |hit| hit.body["name"] == "mention").await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let events: Vec<_> =
        hook.hits().into_iter().filter(|hit| hit.body["name"] == "mention").collect();
    assert_eq!(events.len(), 1, "a 410 was retried");
}

// E14
#[tokio::test]
async fn a_private_callback_is_refused_when_the_server_requires_https() {
    let server = Server::start_with(false).await;
    let (room, justin, _) = server.room().await;
    let (scout, _) = server.bot(&justin, &room, "Scout").await;
    let reply = server
        .mcp(
            &scout,
            json!(1),
            "events/subscribe",
            json!({
                "name": "mention",
                "delivery": { "mode": "webhook", "url": "http://127.0.0.1:9/hook", "secret": SECRET }
            }),
        )
        .await;
    assert_eq!(reply.body["error"]["code"], -32602, "{}", reply.body);
}

// E21
#[tokio::test]
async fn a_wake_up_url_can_carry_a_key_and_the_key_is_never_shown() {
    let server = Server::start().await;
    let (room, justin, kevin) = server.room().await;
    let (scout, scout_id) = server.bot(&justin, &room, "Scout").await;
    let (url, hook) = start_hook("echo").await;
    let path = format!("/api/agents/{scout_id}/notify");
    let key = "super-secret-key-zz";

    let saved = server.put(&justin, &path, json!({ "url": url })).await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    ask_and_answer(&server, &scout, &justin, &room, "one?").await;
    let plain = wait_for(&hook, |hit| hit.body["event"] == "owner_question.answered").await;
    assert!(header(&plain, "authorization").is_none(), "{:?}", plain.headers);

    hook.hits.lock().unwrap().clear();
    let with_key = server.put(&justin, &path, json!({ "url": url, "auth": key })).await;
    assert_eq!(with_key.status, StatusCode::OK, "{}", with_key.body);
    let seen = server.get(&justin, &path).await;
    assert_eq!(seen.body["url"], url);
    assert_eq!(seen.body["auth"], true);
    assert!(!seen.body.to_string().contains(key), "{}", seen.body);
    assert_eq!(server.get(&kevin, &path).await.status, StatusCode::NOT_FOUND);
    let snapshot = server.get(&justin, &format!("/api/hangouts/{room}")).await;
    assert!(!snapshot.body.to_string().contains(key), "the room shows the key");
    let bot_read = server.get(&scout, &format!("/api/hangouts/{room}")).await;
    assert!(!bot_read.body.to_string().contains(key));

    ask_and_answer(&server, &scout, &justin, &room, "two?").await;
    let bearer = wait_for(&hook, |hit| hit.body["event"] == "owner_question.answered").await;
    let expected = format!("Bearer {key}");
    assert_eq!(header(&bearer, "authorization"), Some(expected.as_str()));
    assert!(!bearer.body.to_string().contains(key));
    assert!(!bearer.body.to_string().contains("two-answer"));

    hook.hits.lock().unwrap().clear();
    server.put(&justin, &path, json!({ "url": url })).await;
    ask_and_answer(&server, &scout, &justin, &room, "three?").await;
    let kept = wait_for(&hook, |hit| hit.body["event"] == "owner_question.answered").await;
    assert_eq!(header(&kept, "authorization"), Some(expected.as_str()));

    hook.hits.lock().unwrap().clear();
    server.put(&justin, &path, json!({ "url": url, "auth": null })).await;
    ask_and_answer(&server, &scout, &justin, &room, "four?").await;
    let cleared = wait_for(&hook, |hit| hit.body["event"] == "owner_question.answered").await;
    assert!(header(&cleared, "authorization").is_none(), "{:?}", cleared.headers);

    hook.hits.lock().unwrap().clear();
    let named = "hook-key-zz";
    server.put(&justin, &path, json!({ "url": url, "auth": format!("X-Api-Key: {named}") })).await;
    ask_and_answer(&server, &scout, &justin, &room, "five?").await;
    let custom = wait_for(&hook, |hit| hit.body["event"] == "owner_question.answered").await;
    assert_eq!(header(&custom, "x-api-key"), Some(named));

    let by_bot = server.put(&scout, &path, json!({ "url": url, "auth": "nope" })).await;
    assert_eq!(by_bot.status, StatusCode::FORBIDDEN);

    server.put(&justin, &path, json!({ "url": null })).await;
    let gone = server.get(&justin, &path).await;
    assert!(gone.body["url"].is_null(), "{}", gone.body);
    assert_eq!(gone.body["auth"], false);

    let (person_url, person_hook) = start_hook("echo").await;
    server.put(&kevin, "/api/notify", json!({ "url": person_url })).await;
    server
        .post(
            &scout,
            &format!("/api/hangouts/{room}/check-ins"),
            json!({ "about": "Booking the table", "to": "Kevin" }),
        )
        .await;
    let ping =
        wait_for(&person_hook, |hit| hit.body["text"].as_str().unwrap_or("").contains("Booking"))
            .await;
    assert!(header(&ping, "authorization").is_none(), "a person's notification gained a key");
    assert!(header(&ping, "x-api-key").is_none());
}

async fn ask_and_answer(server: &Server, bot: &str, owner: &str, room: &str, question: &str) {
    let asked = server
        .post(
            bot,
            &format!("/api/hangouts/{room}/owner-questions"),
            json!({ "question": question }),
        )
        .await;
    assert_eq!(asked.status, StatusCode::OK, "{}", asked.body);
    let id = asked.body["id"].as_str().unwrap();
    let answered = server
        .post(
            owner,
            &format!("/api/owner-questions/{id}/answer"),
            json!({ "answer": "two-answer" }),
        )
        .await;
    assert_eq!(answered.status, StatusCode::OK, "{question}: {}", answered.body);
}
