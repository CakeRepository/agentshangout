//! Adversarial tests for starting a city by invite and for a plan's link. They drive a real server
//! over HTTP and are written from the rules in docs/referrals.md (F1 to F23 and the limits table).
//! A comment such as `// F13` above a test names the rule it covers.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use agenthangout::{
    app,
    db::{Db, Invited, NewEvent},
    now, AppState, Config,
};
use reqwest::{header, Client, Method, StatusCode};
use serde_json::{json, Value};

const CODE: &str = "letmein";
const DAY: i64 = 86_400;
const LATER: &str = "2031-06-06T18:00:00-05:00";
const REFUSED: &str = "that isn't the code for starting new hangouts or cities here";
// Words written into a room, so a test can search what a link shows for them.
const ROOM_NAME: &str = "room-name-5512";
const PLAN_NOTES: &str = "plan-notes-5512";
const CHAT_WORDS: &str = "chat-words-5512";

struct Server {
    base: String,
    client: Client,
    state: AppState,
    addresses: AtomicUsize,
}

struct Reply {
    status: StatusCode,
    body: Value,
    retry_after: Option<String>,
    set_cookie: Option<String>,
}

impl Reply {
    fn show(&self) -> String {
        format!("{} {}", self.status, self.body)
    }

    #[track_caller]
    fn ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.show());
        self.body
    }

    #[track_caller]
    fn refused(&self) {
        assert_eq!(self.status, StatusCode::FORBIDDEN, "{}", self.show());
        assert_eq!(self.body["error"], REFUSED, "F5: says nothing about why");
    }
}

#[derive(Clone)]
struct Who {
    key: String,
    id: String,
}

/// Ann's room, which she started with the code. Bo came in by its link, so a friend invited him.
struct Room {
    id: String,
    invite: String,
    ann: Who,
}

impl Server {
    async fn start() -> Self {
        Self::start_with(Some(CODE)).await
    }

    async fn start_with(create_code: Option<&str>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let state = AppState::new(
            Db::open(":memory:").unwrap(),
            Config {
                public_url: base.clone(),
                create_code: create_code.map(str::to_string),
                allow_private_webhooks: true,
                keeper_id: None,
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        Self { base, client: Client::new(), state, addresses: AtomicUsize::new(1) }
    }

    /// A different address each time, so only the tests about limits meet one.
    fn fresh_address(&self) -> String {
        let n = self.addresses.fetch_add(1, Ordering::Relaxed);
        format!("198.51.{}.{}", n / 250, n % 250 + 1)
    }

    async fn send_from(
        &self,
        address: &str,
        token: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header("x-forwarded-for", address);
        if let Some(cookie) = token.strip_prefix("cookie:") {
            request = request.header(header::COOKIE, format!("ah_session={cookie}"));
        } else if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let text_header = |name| {
            response.headers().get(name).map(|value: &header::HeaderValue| value.to_str().unwrap().to_string())
        };
        let retry_after = text_header(header::RETRY_AFTER);
        let set_cookie = text_header(header::SET_COOKIE);
        let text = response.text().await.unwrap();
        Reply { status, retry_after, set_cookie, body: serde_json::from_str(&text).unwrap_or(Value::String(text)) }
    }

    async fn get(&self, token: &str, path: &str) -> Reply {
        self.send_from(&self.fresh_address(), token, Method::GET, path, None).await
    }

    async fn post(&self, token: &str, path: &str, body: Value) -> Reply {
        self.send_from(&self.fresh_address(), token, Method::POST, path, Some(body)).await
    }

    async fn room(&self, name: &str) -> Room {
        let made = self
            .post("", "/api/hangouts", json!({ "name": ROOM_NAME, "your_name": name, "create_code": CODE }))
            .await
            .ok();
        Room {
            id: text(&made["hangout"]["id"]),
            invite: code_of(&made["invite_url"]),
            ann: Who { key: text(&made["owner_key"]), id: text(&made["person"]["id"]) },
        }
    }

    async fn join(&self, invite: &str, name: &str) -> Who {
        let joined = self.post("", &format!("/api/invites/{invite}/join"), json!({ "your_name": name })).await.ok();
        Who { key: text(&joined["owner_key"]), id: text(&joined["person"]["id"]) }
    }

    async fn bot(&self, owner: &Who, room: &str) -> String {
        let made = self.post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": "Scout" })).await.ok();
        text(&made["token"])
    }

    async fn start_city(&self, who: &Who) -> Reply {
        self.post(&who.key, "/api/cities", json!({ "name": "Elm" })).await
    }

    async fn plan(&self, who: &Who, room: &str, min_yes: i64) -> String {
        let made = self
            .post(
                &who.key,
                &format!("/api/hangouts/{room}/events"),
                json!({ "title": "Tacos", "location": "Veracruz", "notes": PLAN_NOTES, "starts_at": LATER, "min_yes": min_yes }),
            )
            .await
            .ok();
        text(&made["id"])
    }

    /// The plan's link as a code.
    async fn link(&self, who: &Who, event: &str) -> String {
        code_of(&self.post(&who.key, &format!("/api/events/{event}/invite"), json!({})).await.ok()["invite_url"])
    }

    async fn answer(&self, token: &str, code: &str, body: Value) -> Reply {
        self.post(token, &format!("/api/plan-invites/{code}/answer"), body).await
    }

    /// Someone new answers through the link.
    async fn invitee(&self, code: &str, name: &str, response: &str) -> Who {
        let body = self.answer("", code, json!({ "response": response, "your_name": name })).await.ok();
        Who { key: text(&body["owner_key"]), id: text(&body["person"]["id"]) }
    }

    async fn room_view(&self, who: &Who, room: &str) -> Value {
        self.get(&who.key, &format!("/api/hangouts/{room}")).await.ok()
    }

    async fn feed(&self, who: &Who) -> String {
        let me = self.get(&who.key, "/api/me").await.ok();
        let url = text(&me["feed_url"]);
        let token = url.rsplit('/').next().unwrap();
        match self.get("", &format!("/cal/{token}")).await.body {
            Value::String(calendar) => calendar,
            other => panic!("{other}"),
        }
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
}

fn code_of(url: &Value) -> String {
    text(url).rsplit('/').next().unwrap().to_string()
}

fn lines_about(view: &Value, words: &str) -> usize {
    view["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["body"].as_str().unwrap().contains(words))
        .count()
}

// Starting a city

// F1
#[tokio::test]
async fn someone_a_friend_brought_into_a_room_can_start_a_city_and_nobody_else_can() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    server.start_city(&room.ann).await.refused();
    server.post("", "/api/cities", json!({ "name": "Elm", "your_name": "Eve" })).await.refused();
    let wrong = server.post("", "/api/cities", json!({ "name": "Elm", "your_name": "Eve", "create_code": "guess" })).await;
    wrong.refused();

    let bo = server.join(&room.invite, "Bo").await;
    let city = server.start_city(&bo).await.ok();
    assert_eq!(city["city"]["name"], "Elm");
    assert_eq!(city["person"]["id"], bo.id.as_str());
}

// F1
#[tokio::test]
async fn joining_a_city_counts_and_so_does_answering_a_plan() {
    let server = Server::start().await;
    let made = server.post("", "/api/cities", json!({ "name": "Oak", "your_name": "Ann", "create_code": CODE })).await.ok();
    let invite = code_of(&made["invite_url"]);
    let joined = server.post("", &format!("/api/city-invites/{invite}/join"), json!({ "your_name": "Bo" })).await.ok();
    let bo = Who { key: text(&joined["owner_key"]), id: text(&joined["person"]["id"]) };
    server.start_city(&bo).await.ok();

    let room = server.room("Cy").await;
    let event = server.plan(&room.ann, &room.id, 2).await;
    let link = server.link(&room.ann, &event).await;
    let friend = server.invitee(&link, "Dee", "no").await;
    server.start_city(&friend).await.ok();
}

// F2
#[tokio::test]
async fn an_invite_covers_one_city_and_leaving_does_not_give_it_back() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let bo = server.join(&room.invite, "Bo").await;
    let city = text(&server.start_city(&bo).await.ok()["city"]["id"]);
    server.start_city(&bo).await.refused();
    server.post(&bo.key, &format!("/api/cities/{city}/leave"), json!({})).await.ok();
    server.start_city(&bo).await.refused();
    // The code still works for someone who has used theirs.
    server.post(&bo.key, "/api/cities", json!({ "name": "Ash", "create_code": CODE })).await.ok();
}

// F2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_requests_at_once_cannot_both_use_one_invite() {
    let server = Arc::new(Server::start().await);
    let room = server.room("Ann").await;
    for round in 0..15 {
        let friend = server.join(&room.invite, &format!("Friend {round}")).await;
        let tries: Vec<_> = (0..8)
            .map(|_| {
                let (server, friend) = (server.clone(), friend.clone());
                tokio::spawn(async move { server.start_city(&friend).await })
            })
            .collect();
        let mut started = 0;
        for attempt in tries {
            let reply = attempt.await.unwrap();
            if reply.status == StatusCode::OK {
                started += 1;
            }
        }
        assert_eq!(started, 1, "round {round}");
    }
}

// F3
#[tokio::test]
async fn a_bot_cannot_use_its_owners_invite() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let bo = server.join(&room.invite, "Bo").await;
    let bot = server.bot(&bo, &room.id).await;
    server.post(&bot, "/api/cities", json!({ "name": "Elm" })).await.refused();
    // And it hasn't used it up.
    server.start_city(&bo).await.ok();
}

// F3
#[tokio::test]
async fn someone_at_a_citys_gate_has_not_been_let_in_yet() {
    let server = Server::start().await;
    let made = server.post("", "/api/cities", json!({ "name": "Oak", "your_name": "Ann", "create_code": CODE })).await.ok();
    let city = text(&made["city"]["id"]);
    let ann = text(&made["owner_key"]);
    let invite = code_of(&made["invite_url"]);
    let joined = server.post("", &format!("/api/city-invites/{invite}/join"), json!({ "your_name": "Bo" })).await.ok();
    let bo = text(&joined["owner_key"]);
    let set = server.send_from(&server.fresh_address(), &ann, Method::PUT, &format!("/api/cities/{city}/rules"), Some(json!({ "min_vouches": 1 }))).await;
    assert_eq!(set.status, StatusCode::OK, "{}", set.show());

    let arrived = server.post("", &format!("/api/city-invites/{invite}/join"), json!({ "your_name": "Cy", "accept_rules": true })).await.ok();
    assert_eq!(arrived["waiting"], true, "{arrived}");
    let cy = Who { key: text(&arrived["owner_key"]), id: text(&arrived["person"]["id"]) };
    server.start_city(&cy).await.refused();
    assert_eq!(server.get(&cy.key, "/api/me").await.ok()["may_start_city"], false);

    server.post(&bo, &format!("/api/cities/{city}/people/{}/vouch", cy.id), json!({})).await.ok();
    server.start_city(&cy).await.ok();
}

// F4
#[tokio::test]
async fn an_invite_starts_a_city_and_never_a_hangout() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let bo = server.join(&room.invite, "Bo").await;
    server.post(&bo.key, "/api/hangouts", json!({ "name": "Mine" })).await.refused();
}

// F6
#[tokio::test]
async fn the_page_is_told_when_no_code_is_needed() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let may = |who: Who| {
        let server = &server;
        async move { server.get(&who.key, "/api/me").await.ok()["may_start_city"].clone() }
    };
    assert_eq!(may(room.ann.clone()).await, false);
    let bo = server.join(&room.invite, "Bo").await;
    assert_eq!(may(bo.clone()).await, true);
    server.start_city(&bo).await.ok();
    assert_eq!(may(bo).await, false, "used");

    let open = Server::start_with(None).await;
    let room = open.room("Ann").await;
    let me = open.get(&room.ann.key, "/api/me").await.ok();
    assert_eq!(me["may_start_city"], true, "this server asks for no code");
}

// F7
#[tokio::test]
async fn cities_started_without_a_code_are_limited_per_address_and_a_code_is_not() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let start = |who: Who, address: &'static str| {
        let server = &server;
        async move {
            server.send_from(address, &who.key, Method::POST, "/api/cities", Some(json!({ "name": "Elm" }))).await
        }
    };
    for n in 0..3 {
        let friend = server.join(&room.invite, &format!("Friend {n}")).await;
        start(friend, "203.0.113.9").await.ok();
    }
    let friend = server.join(&room.invite, "One more").await;
    let over = start(friend.clone(), "203.0.113.9").await;
    assert_eq!(over.status, StatusCode::TOO_MANY_REQUESTS, "{}", over.show());
    assert!(over.retry_after.is_some(), "says how long to wait");
    start(friend, "203.0.113.10").await.ok();

    let body = json!({ "name": "Ash", "your_name": "Dee", "create_code": CODE });
    for _ in 0..5 {
        server.send_from("203.0.113.9", "", Method::POST, "/api/cities", Some(body.clone())).await.ok();
    }
}

// F7
#[tokio::test]
async fn cities_started_without_a_code_are_limited_across_the_server() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    for n in 0..20 {
        let friend = server.join(&room.invite, &format!("Friend {n}")).await;
        server.start_city(&friend).await.ok();
    }
    let friend = server.join(&room.invite, "One more").await;
    let over = server.start_city(&friend).await;
    assert_eq!(over.status, StatusCode::TOO_MANY_REQUESTS, "{}", over.show());
}

// A plan's link

// F8, F22
#[tokio::test]
async fn only_a_person_in_the_room_can_get_a_plans_link() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 2).await;
    let bot = server.bot(&room.ann, &room.id).await;
    for path in [format!("/api/events/{event}/invite"), format!("/api/events/{event}/invite/rotate")] {
        let from_bot = server.post(&bot, &path, json!({})).await;
        assert_eq!(from_bot.status, StatusCode::FORBIDDEN, "{}", from_bot.show());
        assert_eq!(server.post("", &path, json!({})).await.status, StatusCode::UNAUTHORIZED);
        let other = server.room("Eve").await;
        assert_eq!(server.post(&other.ann.key, &path, json!({})).await.status, StatusCode::NOT_FOUND);
    }
    let made = server.post(&room.ann.key, &format!("/api/events/{event}/invite"), json!({})).await.ok();
    assert!(text(&made["invite_url"]).starts_with(&format!("{}/p/", server.base)), "{made}");
}

// F9
#[tokio::test]
async fn a_plan_has_one_link_and_replacing_it_stops_the_old_one() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let bo = server.join(&room.invite, "Bo").await;
    let event = server.plan(&room.ann, &room.id, 2).await;
    let first = server.link(&room.ann, &event).await;
    assert_eq!(server.link(&bo, &event).await, first, "asking again gives the same link");

    let replaced = server.post(&bo.key, &format!("/api/events/{event}/invite/rotate"), json!({})).await.ok();
    let second = code_of(&replaced["invite_url"]);
    assert_ne!(second, first);
    assert_eq!(server.get("", &format!("/api/plan-invites/{first}")).await.status, StatusCode::NOT_FOUND);
    let old = server.answer("", &first, json!({ "response": "yes", "your_name": "Eve" })).await;
    assert_eq!(old.status, StatusCode::NOT_FOUND);
    let preview = server.get("", &format!("/api/plan-invites/{second}")).await.ok();
    assert_eq!(preview["invited_by"], "Bo", "says who sent the link that works");
}

// F10
#[tokio::test]
async fn a_link_stops_working_when_its_plan_is_cancelled_or_over() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 2).await;
    let link = server.link(&room.ann, &event).await;
    server.post(&room.ann.key, &format!("/api/events/{event}/cancel"), json!({})).await.ok();
    for path in [format!("/api/plan-invites/{link}"), format!("/api/plan-invites/{link}/plan.ics")] {
        assert_eq!(server.get("", &path).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    let refused = server.answer("", &link, json!({ "response": "yes", "your_name": "Eve" })).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    assert!(refused.set_cookie.is_none(), "F12: nobody was made");
    let again = server.post(&room.ann.key, &format!("/api/events/{event}/invite"), json!({})).await;
    assert_eq!(again.status, StatusCode::CONFLICT);

    let past = "over-plan-5512";
    server.state.db.insert_event(&NewEvent {
        id: past, hangout_id: &room.id, title: "Old", location: "", notes: "", starts_at: now() - 2 * DAY,
        ends_at: now() - DAY, min_yes: 1, proposed_by: &room.ann.id, proposer: "Ann", now: now() - 3 * DAY,
    }).unwrap();
    server.state.db.plan_invite_code(past, "over-code-5512", &room.ann.id, now()).unwrap();
    assert_eq!(server.get("", "/api/plan-invites/over-code-5512").await.status, StatusCode::NOT_FOUND);

    assert_eq!(server.get("", "/api/plan-invites/nope").await.status, StatusCode::NOT_FOUND);
}

// F11
#[tokio::test]
async fn a_link_shows_the_plan_and_nothing_else_from_the_room() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let bo = server.join(&room.invite, "Bo").await;
    server.post(&room.ann.key, &format!("/api/hangouts/{}/messages", room.id), json!({ "text": CHAT_WORDS })).await.ok();
    server.post(&room.ann.key, &format!("/api/hangouts/{}/notes", room.id), json!({ "text": CHAT_WORDS })).await.ok();
    let event = server.plan(&bo, &room.id, 3).await;
    let link = server.link(&room.ann, &event).await;
    let preview = server.get("", &format!("/api/plan-invites/{link}")).await.ok();

    let mut keys: Vec<&str> = preview.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["ends_at", "invited_by", "location", "on_calendar", "starts_at", "title", "yes", "yes_needed", "your_answer"]);
    assert_eq!(preview["title"], "Tacos");
    assert_eq!(preview["location"], "Veracruz");
    assert_eq!(preview["invited_by"], "Ann");
    assert_eq!(preview["yes"], 1);
    assert_eq!(preview["yes_needed"], 3);
    assert_eq!(preview["on_calendar"], false);
    assert_eq!(preview["your_answer"], Value::Null);
    let shown = preview.to_string();
    for private in [ROOM_NAME, PLAN_NOTES, CHAT_WORDS, "Bo", room.id.as_str(), event.as_str()] {
        assert!(!shown.contains(private), "the link shows {private}: {shown}");
    }

    let answered = server.answer("", &link, json!({ "response": "yes", "your_name": "Dee" })).await.ok();
    let shown = answered.to_string();
    for private in [ROOM_NAME, PLAN_NOTES, CHAT_WORDS, room.id.as_str()] {
        assert!(!shown.contains(private), "the answer shows {private}: {shown}");
    }
}

// F12
#[tokio::test]
async fn a_friend_answers_with_a_name_and_gets_an_account_with_a_session() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 3).await;
    let link = server.link(&room.ann, &event).await;
    let answered = server.answer("", &link, json!({ "response": "maybe", "your_name": "Dee" })).await;
    let cookie = answered.set_cookie.clone().expect("signed in on this browser");
    let body = answered.ok();
    assert_eq!(body["plan"]["your_answer"], "maybe");
    assert_eq!(body["person"]["name"], "Dee");
    assert!(body["owner_key"].is_string(), "shown once, like any new account");

    let session = cookie.split(';').next().unwrap().strip_prefix("ah_session=").unwrap();
    let token = format!("cookie:{session}");
    assert_eq!(server.get(&token, "/api/me").await.ok()["name"], "Dee");
    let again = server.answer(&token, &link, json!({ "response": "yes" })).await;
    assert!(again.set_cookie.is_none(), "already signed in");
    let again = again.ok();
    assert_eq!(again["plan"]["your_answer"], "yes", "they can change it");
    assert_eq!(again["owner_key"], Value::Null, "no second account");
    let preview = server.get(&token, &format!("/api/plan-invites/{link}")).await.ok();
    assert_eq!(preview["your_answer"], "yes");
}

// F12
#[tokio::test]
async fn a_refused_answer_leaves_no_account_behind() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 3).await;
    let link = server.link(&room.ann, &event).await;
    for body in [
        json!({ "response": "perhaps", "your_name": "Dee" }),
        json!({ "response": "yes" }),
        json!({ "response": "yes", "your_name": "   " }),
        json!({ "response": "yes", "your_name": "x".repeat(500) }),
    ] {
        let refused = server.answer("", &link, body.clone()).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{body}: {}", refused.show());
        assert!(refused.set_cookie.is_none(), "{body}");
    }
    let view = server.room_view(&room.ann, &room.id).await;
    assert_eq!(view["invitees"], json!([]));
}

// F13
#[tokio::test]
async fn someone_in_the_room_answers_as_themselves() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let bo = server.join(&room.invite, "Bo").await;
    let event = server.plan(&room.ann, &room.id, 2).await;
    let link = server.link(&room.ann, &event).await;
    let answered = server.answer(&bo.key, &link, json!({ "response": "yes" })).await.ok();
    assert_eq!(answered["plan"]["on_calendar"], true);
    let view = server.room_view(&room.ann, &room.id).await;
    assert_eq!(view["invitees"], json!([]), "he's in the room, so he's no invitee");
    assert_eq!(lines_about(&view, "through its link"), 0);
}

// F13
#[tokio::test]
async fn a_persons_answer_through_the_link_stands_over_their_bots() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let bo = server.join(&room.invite, "Bo").await;
    let bot = server.bot(&bo, &room.id).await;
    let event = server.plan(&room.ann, &room.id, 3).await;
    server.post(&bot, &format!("/api/events/{event}/rsvp"), json!({ "response": "yes" })).await.ok();
    let link = server.link(&room.ann, &event).await;
    server.answer(&bo.key, &link, json!({ "response": "no" })).await.ok();
    let late = server.post(&bot, &format!("/api/events/{event}/rsvp"), json!({ "response": "yes" })).await;
    assert_eq!(late.status, StatusCode::CONFLICT, "{}", late.show());
}

// F14
#[tokio::test]
async fn an_invitee_gets_nothing_of_the_room() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 2).await;
    let other = server.plan(&room.ann, &room.id, 2).await;
    let link = server.link(&room.ann, &event).await;
    let dee = server.invitee(&link, "Dee", "yes").await;

    for path in [
        format!("/api/hangouts/{}", room.id),
        format!("/api/hangouts/{}/messages", room.id),
        format!("/api/hangouts/{}/stream", room.id),
        format!("/api/events/{event}/ics"),
        format!("/api/events/{other}/ics"),
    ] {
        assert_eq!(server.get(&dee.key, &path).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    let said = server.post(&dee.key, &format!("/api/hangouts/{}/messages", room.id), json!({ "text": "hi" })).await;
    assert_eq!(said.status, StatusCode::NOT_FOUND);
    let answered = server.post(&dee.key, &format!("/api/events/{other}/rsvp"), json!({ "response": "yes" })).await;
    assert_eq!(answered.status, StatusCode::NOT_FOUND, "another plan in the room isn't theirs");
    let share = server.post(&dee.key, &format!("/api/events/{event}/invite"), json!({})).await;
    assert_eq!(share.status, StatusCode::NOT_FOUND, "they can't hand out the link either");
    let me = server.get(&dee.key, "/api/me").await.ok();
    assert_eq!(me["hangouts"], json!([]));
}

// F15, F22
#[tokio::test]
async fn a_bot_cannot_answer_through_a_link() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let bot = server.bot(&room.ann, &room.id).await;
    let event = server.plan(&room.ann, &room.id, 2).await;
    let link = server.link(&room.ann, &event).await;
    let refused = server.answer(&bot, &link, json!({ "response": "no" })).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.show());
    assert!(refused.set_cookie.is_none());
    let preview = server.get(&bot, &format!("/api/plan-invites/{link}")).await.ok();
    assert_eq!(preview["yes"], 1, "Ann's yes still stands");
}

// F16
#[tokio::test]
async fn an_invitees_yes_confirms_the_plan_and_reaches_their_calendar() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 2).await;
    let link = server.link(&room.ann, &event).await;
    let dee = server.invitee(&link, "Dee", "yes").await;
    let view = server.room_view(&room.ann, &room.id).await;
    assert_eq!(view["events"][0]["status"], "confirmed", "{view}");
    assert_eq!(view["stats"]["on_calendar"], 1);
    assert!(server.feed(&dee).await.contains("SUMMARY:Tacos"));

    // A no takes it off again, and off their calendar.
    server.answer(&dee.key, &link, json!({ "response": "no" })).await.ok();
    let view = server.room_view(&room.ann, &room.id).await;
    assert_eq!(view["events"][0]["status"], "proposed");
    assert!(!server.feed(&dee).await.contains("SUMMARY:Tacos"));
}

// F17
#[tokio::test]
async fn an_invitees_yes_never_grows_a_city() {
    let server = Server::start().await;
    let made = server.post("", "/api/cities", json!({ "name": "Oak", "your_name": "Ann", "create_code": CODE })).await.ok();
    let city = text(&made["city"]["id"]);
    let square = text(&made["city"]["square_id"]);
    let ann = text(&made["person"]["id"]);
    let db = &server.state.db;
    let past = |id: &'static str| NewEvent {
        id, hangout_id: &square, title: "Old", location: "", notes: "", starts_at: now() - 2 * DAY,
        ends_at: now() - DAY, min_yes: 2, proposed_by: &ann, proposer: "Ann", now: now() - 3 * DAY,
    };

    db.insert_event(&past("with-invitee")).unwrap();
    db.upsert_rsvp("with-invitee", &ann, "yes", None, "", now()).unwrap();
    let dee = server.room("Dee").await.ann.id;
    assert_eq!(db.add_plan_invitee("with-invitee", &dee, 30, now()).unwrap(), Invited::New);
    db.upsert_rsvp("with-invitee", &dee, "yes", None, "", now()).unwrap();
    db.set_event_status("with-invitee", "confirmed", now()).unwrap();
    assert_eq!(db.gatherings(&city, now()).unwrap(), 0, "one person and a link is no gathering");

    let joined = server.post("", &format!("/api/city-invites/{}/join", code_of(&made["invite_url"])), json!({ "your_name": "Bo" })).await.ok();
    db.upsert_rsvp("with-invitee", &text(&joined["person"]["id"]), "yes", None, "", now()).unwrap();
    assert_eq!(db.gatherings(&city, now()).unwrap(), 1, "two people in the city and a friend is");
}

// F18
#[tokio::test]
async fn a_plan_takes_thirty_invitees() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 50).await;
    let link = server.link(&room.ann, &event).await;
    let existing = server.invitee(&link, "First", "yes").await;
    for n in 1..30 {
        server.invitee(&link, &format!("Friend {n}"), "yes").await;
    }
    let full = server.answer("", &link, json!({ "response": "yes", "your_name": "One more" })).await;
    assert_eq!(full.status, StatusCode::CONFLICT, "{}", full.show());
    assert!(full.set_cookie.is_none(), "F12: nobody was made");

    // Those already on it, and people in the room, aren't turned away.
    server.answer(&existing.key, &link, json!({ "response": "no" })).await.ok();
    let bo = server.join(&room.invite, "Bo").await;
    server.answer(&bo.key, &link, json!({ "response": "yes" })).await.ok();
}

// F18
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_last_places_on_a_plan_cannot_be_taken_twice() {
    let server = Arc::new(Server::start().await);
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 50).await;
    let link = server.link(&room.ann, &event).await;
    for n in 0..28 {
        server.invitee(&link, &format!("Friend {n}"), "yes").await;
    }
    let tries: Vec<_> = (0..12)
        .map(|n| {
            let (server, link) = (server.clone(), link.clone());
            tokio::spawn(async move {
                server.answer("", &link, json!({ "response": "yes", "your_name": format!("Late {n}") })).await
            })
        })
        .collect();
    for attempt in tries {
        attempt.await.unwrap();
    }
    let view = server.room_view(&room.ann, &room.id).await;
    assert_eq!(view["invitees"].as_array().unwrap().len(), 30);
}

// F18
#[tokio::test]
async fn the_cap_holds_where_an_invitee_is_written_too() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 3).await;
    let bo = server.room("Bo").await.ann.id;
    let cy = server.room("Cy").await.ann.id;
    let db = &server.state.db;
    assert_eq!(db.add_plan_invitee(&event, &bo, 1, now()).unwrap(), Invited::New);
    assert_eq!(db.add_plan_invitee(&event, &bo, 1, now()).unwrap(), Invited::Already);
    assert_eq!(db.add_plan_invitee(&event, &cy, 1, now()).unwrap(), Invited::Full);
}

// F19
#[tokio::test]
async fn the_room_sees_who_answered_and_gets_one_line_for_it() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 3).await;
    let link = server.link(&room.ann, &event).await;
    let dee = server.invitee(&link, "Dee", "yes").await;
    server.answer(&dee.key, &link, json!({ "response": "maybe" })).await.ok();
    server.answer(&dee.key, &link, json!({ "response": "no" })).await.ok();

    let view = server.room_view(&room.ann, &room.id).await;
    assert_eq!(view["invitees"].as_array().unwrap().len(), 1, "{view}");
    assert_eq!(view["invitees"][0]["name"], "Dee");
    assert_eq!(view["invitees"][0]["event_id"], event.as_str());
    let rsvp = view["events"][0]["rsvps"].as_array().unwrap().iter().find(|r| r["person_id"] == dee.id.as_str()).unwrap().clone();
    assert_eq!(rsvp["response"], "no");
    assert_eq!(lines_about(&view, "through its link"), 1);
    assert_eq!(lines_about(&view, "Dee said yes to \"Tacos\""), 1);
}

// F19
#[tokio::test]
async fn a_cancelled_plan_stops_showing_its_invitees() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 3).await;
    let link = server.link(&room.ann, &event).await;
    server.invitee(&link, "Dee", "yes").await;
    server.post(&room.ann.key, &format!("/api/events/{event}/cancel"), json!({})).await.ok();
    assert_eq!(server.room_view(&room.ann, &room.id).await["invitees"], json!([]));
}

// F20
#[tokio::test]
async fn an_invitees_calendar_leaves_out_the_notes_and_the_room() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 3).await;
    let link = server.link(&room.ann, &event).await;
    let dee = server.invitee(&link, "Dee", "maybe").await;
    let theirs = server.feed(&dee).await;
    assert!(theirs.contains("SUMMARY:Tacos"), "{theirs}");
    assert!(theirs.contains("LOCATION:Veracruz"), "{theirs}");
    for private in [PLAN_NOTES, ROOM_NAME] {
        assert!(!theirs.contains(private), "{private}: {theirs}");
    }

    let file = server.get("", &format!("/api/plan-invites/{link}/plan.ics")).await;
    assert_eq!(file.status, StatusCode::OK);
    let file = file.body.as_str().unwrap();
    assert!(file.contains("SUMMARY:Tacos"), "{file}");
    for private in [PLAN_NOTES, ROOM_NAME] {
        assert!(!file.contains(private), "{private}: {file}");
    }

    let hers = server.feed(&room.ann).await;
    assert!(hers.contains(PLAN_NOTES) && hers.contains(ROOM_NAME), "someone in the room still gets it all: {hers}");
}

// F21
#[tokio::test]
async fn a_plans_page_is_noindex_and_holds_nothing_about_the_plan() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 3).await;
    let link = server.link(&room.ann, &event).await;
    let page = server.get("", &format!("/p/{link}")).await;
    assert_eq!(page.status, StatusCode::OK);
    let page = page.body.as_str().unwrap();
    assert!(page.contains("noindex"));
    for private in ["Tacos", "Veracruz", "Ann", ROOM_NAME, PLAN_NOTES] {
        assert!(!page.contains(private), "the page's HTML holds {private}");
    }
}

// Limits
#[tokio::test]
async fn new_accounts_from_plan_links_are_limited_per_address() {
    let server = Server::start().await;
    let room = server.room("Ann").await;
    let event = server.plan(&room.ann, &room.id, 50).await;
    let link = server.link(&room.ann, &event).await;
    let path = format!("/api/plan-invites/{link}/answer");
    let answer = |n: usize, address: &'static str| {
        let (server, path) = (&server, &path);
        async move {
            let body = json!({ "response": "yes", "your_name": format!("Friend {n}") });
            server.send_from(address, "", Method::POST, path, Some(body)).await
        }
    };
    for n in 0..10 {
        answer(n, "203.0.113.20").await.ok();
    }
    let over = answer(10, "203.0.113.20").await;
    assert_eq!(over.status, StatusCode::TOO_MANY_REQUESTS, "{}", over.show());
    assert!(over.retry_after.is_some());
    assert!(over.set_cookie.is_none());
    answer(11, "203.0.113.21").await.ok();
}
