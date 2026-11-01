//! A14 to A26: linking a Discord account to a bot, against a stand-in for Discord that answers as
//! the real one does and writes down everything it was sent.

use std::sync::{Arc, Mutex};

use agenthangout::{app, db::Db, discord, now, passkey, worlds, AppState, Config};
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode as AxumStatus},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use reqwest::{Client, Method, StatusCode, Url};
use serde_json::{json, Value};

const CLIENT_ID: &str = "1234567890";
const CLIENT_SECRET: &str = "the-client-secret";
/// Base64 of `CLIENT_ID:CLIENT_SECRET`, which is how the token routes are told who's asking.
const BASIC: &str = "Basic MTIzNDU2Nzg5MDp0aGUtY2xpZW50LXNlY3JldA==";

#[derive(Default)]
struct Seen {
    /// The form each code was exchanged with, with the header that came along.
    exchanged: Vec<(String, String)>,
    /// The tokens someone read an account with.
    read_with: Vec<String>,
    /// The form each revocation came with.
    revoked: Vec<(String, String)>,
}

type Discord = Arc<Mutex<Seen>>;

fn seen_basic(headers: &HeaderMap) -> String {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// A code is the name of an account here. `refused` is one Discord doesn't know, and `broken` signs
/// in an account with an id nobody could link to.
async fn token(
    State(seen): State<Discord>,
    headers: HeaderMap,
    form: String,
) -> axum::response::Response {
    seen.lock().unwrap().exchanged.push((seen_basic(&headers), form.clone()));
    let code = Url::parse(&format!("http://form/?{form}"))
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default();
    if code == "refused" {
        return (AxumStatus::BAD_REQUEST, Json(json!({ "error": "invalid_grant" })))
            .into_response();
    }
    Json(json!({
        "access_token": format!("token-for-{code}"),
        "token_type": "Bearer",
        "expires_in": 604800,
        "refresh_token": "a-refresh-token",
        "scope": "identify",
    }))
    .into_response()
}

async fn user(State(seen): State<Discord>, headers: HeaderMap) -> axum::response::Response {
    let bearer = seen_basic(&headers);
    seen.lock().unwrap().read_with.push(bearer.clone());
    let (id, username) = match bearer.trim_start_matches("Bearer token-for-") {
        "ann" => ("1000000000000000001", "ann_on_discord"),
        "ann-renamed" => ("1000000000000000001", "ann_renamed"),
        "bo" => ("2000000000000000002", "bo.on.discord"),
        "broken" => ("../../etc", "a"),
        _ => return AxumStatus::UNAUTHORIZED.into_response(),
    };
    Json(json!({
        "id": id,
        "username": username,
        "discriminator": "0",
        "global_name": "Display Name",
        "avatar": "abcdef",
        "email": "never-asked-for@example.com",
    }))
    .into_response()
}

async fn revoke(State(seen): State<Discord>, headers: HeaderMap, form: String) -> Json<Value> {
    seen.lock().unwrap().revoked.push((seen_basic(&headers), form));
    Json(json!({}))
}

async fn start_discord() -> (String, Discord) {
    let seen = Discord::default();
    let routes = Router::new()
        .route("/oauth2/token", post(token))
        .route("/oauth2/token/revoke", post(revoke))
        .route("/users/@me", get(user))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, routes).await });
    (url, seen)
}

struct Server {
    base: String,
    /// What the server calls itself. A name rather than a number, so that passkeys are on.
    origin: String,
    client: Client,
    state: AppState,
    discord: Discord,
}

struct Who {
    key: String,
    id: String,
}

struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    fn ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.body);
        self.body
    }

    fn refused(self, status: StatusCode) {
        assert_eq!(self.status, status, "{}", self.body);
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
}

fn who(body: &Value) -> Who {
    Who { key: text(&body["owner_key"]), id: text(&body["person"]["id"]) }
}

/// The `state` in the address a browser was sent to.
fn step_of(url: &str) -> String {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .expect("no state")
}

impl Server {
    async fn start() -> Self {
        Self::with(true).await
    }

    /// A server with linking turned on, or with no Discord app set up at all.
    async fn with(on: bool) -> Self {
        let (discord_url, discord) = start_discord().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let origin = format!("http://localhost:{port}");
        let state = AppState::with_discord(
            Db::open(":memory:").unwrap(),
            Config {
                public_url: origin.clone(),
                create_code: None,
                allow_private_webhooks: true,
                keeper_id: None,
                god_ids: Vec::new(),
            },
            on.then(|| discord::App {
                client_id: CLIENT_ID.to_string(),
                client_secret: CLIENT_SECRET.to_string(),
                authorize_url: format!("{discord_url}/oauth2/authorize"),
                api_url: discord_url.clone(),
            }),
        );
        worlds::ensure_commons(&state).unwrap();
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        let base = format!("http://127.0.0.1:{port}");
        Self { base, origin, client: Client::new(), state, discord }
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

    async fn delete(&self, token: &str, path: &str) -> Reply {
        self.send(token, Method::DELETE, path, None).await
    }

    /// Someone new with a room of their own, and the code that lets others in.
    async fn someone(&self, name: &str) -> (String, Who, String) {
        let made = self
            .post(
                "",
                "/api/hangouts",
                json!({ "name": format!("{name}'s room"), "your_name": name }),
            )
            .await
            .ok();
        (text(&made["hangout"]["id"]), who(&made), text(&made["hangout"]["invite_code"]))
    }

    async fn join(&self, code: &str, name: &str) -> Who {
        who(&self
            .post("", &format!("/api/invites/{code}/join"), json!({ "your_name": name }))
            .await
            .ok())
    }

    async fn bot(&self, owner: &Who, room: &str, name: &str) -> (String, String) {
        let made = self
            .post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await
            .ok();
        (text(&made["agent"]["id"]), text(&made["token"]))
    }

    async fn start_link(&self, token: &str, bot: &str) -> Reply {
        self.post(token, &format!("/api/agents/{bot}/discord"), json!({})).await
    }

    async fn finish_link(&self, token: &str, step: &str, code: &str) -> Reply {
        self.post(token, "/api/discord/finish", json!({ "code": code, "state": step })).await
    }

    /// The whole of it: start, say yes on Discord, come back.
    async fn link(&self, owner: &Who, bot: &str, code: &str) {
        let url = text(&self.start_link(&owner.key, bot).await.ok()["url"]);
        let finished = self.finish_link(&owner.key, &step_of(&url), code).await.ok();
        assert_eq!(finished["agent_id"], bot);
    }

    /// The bot as this caller's snapshot of the room shows it.
    async fn in_room(&self, token: &str, room: &str, bot: &str) -> Value {
        let snapshot = self.get(token, &format!("/api/hangouts/{room}")).await.ok();
        snapshot["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["id"] == bot)
            .unwrap_or_else(|| panic!("{bot} isn't in {room}"))
            .clone()
    }
}

// A14

#[tokio::test]
async fn a_bot_gets_403_on_every_route_so_it_cant_put_a_name_on_itself() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, token) = server.bot(&ann, &room, "Sequence Sam").await;
    server.start_link(&token, &bot).await.refused(StatusCode::FORBIDDEN);
    server.finish_link(&token, "any", "ann").await.refused(StatusCode::FORBIDDEN);
    server
        .delete(&token, &format!("/api/agents/{bot}/discord"))
        .await
        .refused(StatusCode::FORBIDDEN);
    assert!(server.in_room(&ann.key, &room, &bot).await.get("discord").is_none());
}

// A14
#[tokio::test]
async fn someone_elses_bot_is_a_404_and_nobody_signed_in_is_a_401() {
    let server = Server::start().await;
    let (room, ann, code) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let bo = server.join(&code, "Bo").await;
    server.start_link(&bo.key, &bot).await.refused(StatusCode::NOT_FOUND);
    server
        .delete(&bo.key, &format!("/api/agents/{bot}/discord"))
        .await
        .refused(StatusCode::NOT_FOUND);
    server.start_link("", &bot).await.refused(StatusCode::UNAUTHORIZED);
    server.finish_link("", "any", "ann").await.refused(StatusCode::UNAUTHORIZED);
}

// A14
#[tokio::test]
async fn a_house_bot_belongs_to_its_house_so_it_cant_be_linked() {
    let server = Server::start().await;
    let made =
        server.post("", "/api/cities", json!({ "name": "Elm", "your_name": "Ann" })).await.ok();
    let ann = who(&made);
    let city = text(&made["city"]["id"]);
    let house = server
        .post(&ann.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Ann's" }))
        .await
        .ok();
    let house_bot = server
        .post(
            &ann.key,
            &format!("/api/houses/{}/house-bot", text(&house["house"]["id"])),
            json!({ "name": "Casa" }),
        )
        .await
        .ok();
    let bot = text(&house_bot["agent"]["id"]);
    server.start_link(&ann.key, &bot).await.refused(StatusCode::CONFLICT);
}

// A15

#[tokio::test]
async fn discord_is_asked_for_identify_and_sends_people_back_to_the_page_that_finishes_it() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let url = Url::parse(&text(&server.start_link(&ann.key, &bot).await.ok()["url"])).unwrap();
    assert_eq!(url.path(), "/oauth2/authorize");
    let asked = |name: &str| {
        url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
    };
    assert_eq!(asked("response_type").as_deref(), Some("code"));
    assert_eq!(asked("client_id").as_deref(), Some(CLIENT_ID));
    assert_eq!(asked("scope").as_deref(), Some("identify"), "nothing wider");
    assert_eq!(asked("prompt").as_deref(), Some("consent"), "it's always asked");
    assert_eq!(asked("redirect_uri"), Some(format!("{}/discord/linked", server.origin)));
    assert!(asked("state").is_some_and(|state| state.len() >= 32));
    assert!(!url.as_str().contains(CLIENT_SECRET));
}

// A15
#[tokio::test]
async fn the_token_is_used_to_read_the_account_then_revoked_and_only_two_things_are_kept() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    server.link(&ann, &bot, "ann").await;

    {
        let seen = server.discord.lock().unwrap();
        let [(basic, form)] = seen.exchanged.as_slice() else {
            panic!("{:?}", seen.exchanged);
        };
        assert_eq!(basic, BASIC);
        assert!(form.contains("grant_type=authorization_code"), "{form}");
        assert!(form.contains("code=ann"), "{form}");
        assert!(form.contains("%2Fdiscord%2Flinked"), "{form}");
        assert_eq!(seen.read_with, ["Bearer token-for-ann"]);
        let [(basic, form)] = seen.revoked.as_slice() else {
            panic!("{:?}", seen.revoked);
        };
        assert_eq!(basic, BASIC);
        assert!(form.contains("token=token-for-ann"), "{form}");
    }

    assert_eq!(
        server.in_room(&ann.key, &room, &bot).await["discord"],
        json!({ "id": "1000000000000000001", "name": "ann_on_discord" })
    );
    let link = server.state.db.discord_link(&bot).unwrap().unwrap();
    assert_eq!(
        (link.id.as_str(), link.name.as_str()),
        ("1000000000000000001", "ann_on_discord"),
        "not the display name, and nothing else"
    );
}

// A15
#[tokio::test]
async fn a_code_discord_refuses_or_an_account_that_makes_no_sense_links_nothing() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    for code in ["refused", "broken", "nobody"] {
        let url = text(&server.start_link(&ann.key, &bot).await.ok()["url"]);
        server.finish_link(&ann.key, &step_of(&url), code).await.refused(StatusCode::BAD_GATEWAY);
        assert!(server.state.db.discord_link(&bot).unwrap().is_none(), "{code}");
    }
    // Discord gave out a token for the last two, and it doesn't outlive the step.
    assert_eq!(server.discord.lock().unwrap().revoked.len(), 2);
}

// A16

#[tokio::test]
async fn a_step_works_once() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let url = text(&server.start_link(&ann.key, &bot).await.ok()["url"]);
    server.finish_link(&ann.key, &step_of(&url), "ann").await.ok();
    server.finish_link(&ann.key, &step_of(&url), "bo").await.refused(StatusCode::BAD_REQUEST);
    assert_eq!(server.state.db.discord_link(&bot).unwrap().unwrap().name, "ann_on_discord");
}

// A16
#[tokio::test]
async fn a_step_lapses_after_five_minutes_and_a_failed_one_is_gone_too() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let url = text(&server.start_link(&ann.key, &bot).await.ok()["url"]);
    let caller = agenthangout::auth::Caller {
        person_id: ann.id.clone(),
        agent_id: None,
        name: "Ann".to_string(),
        secret_hash: String::new(),
        is_house: false,
    };
    let later = now() + passkey::CEREMONY_SECS + 1;
    let lapsed =
        discord::finish(&server.state, &caller, &step_of(&url), "ann", later).await.unwrap_err();
    assert_eq!(lapsed.status.as_u16(), 400);
    assert!(server.state.db.discord_link(&bot).unwrap().is_none());

    let url = text(&server.start_link(&ann.key, &bot).await.ok()["url"]);
    server.finish_link(&ann.key, &step_of(&url), "refused").await.refused(StatusCode::BAD_GATEWAY);
    server.finish_link(&ann.key, &step_of(&url), "ann").await.refused(StatusCode::BAD_REQUEST);
}

// A16
#[tokio::test]
async fn steps_share_the_limit_on_what_one_address_can_keep_open() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let from_one_address = || async {
        server
            .client
            .post(format!("{}/api/agents/{bot}/discord", server.base))
            .bearer_auth(&ann.key)
            .header("x-forwarded-for", "203.0.113.9")
            .send()
            .await
            .unwrap()
            .status()
    };
    for _ in 0..passkey::MAX_OPEN_PER_ADDRESS {
        assert_eq!(from_one_address().await, StatusCode::OK);
    }
    assert_eq!(from_one_address().await, StatusCode::TOO_MANY_REQUESTS);
}

// A17

#[tokio::test]
async fn the_address_discord_sends_people_to_is_the_page_and_changes_nothing() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let url = text(&server.start_link(&ann.key, &bot).await.ok()["url"]);
    let step = step_of(&url);

    // No cookie and no token, as a redirect from another site arrives under SameSite=Strict.
    let page = server
        .client
        .get(format!("{}/discord/linked?code=ann&state={step}", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.text().await.unwrap().contains("/app.js"));
    assert!(server.state.db.discord_link(&bot).unwrap().is_none());
    assert!(server.discord.lock().unwrap().exchanged.is_empty());

    // The step is still there for the page to finish.
    server.finish_link(&ann.key, &step, "ann").await.ok();
}

// A18

#[tokio::test]
async fn someone_elses_step_cant_put_your_discord_on_their_bot() {
    let server = Server::start().await;
    let (room, mallory, code) = server.someone("Mallory").await;
    let (bot, _) = server.bot(&mallory, &room, "Not Suspicious").await;
    let ann = server.join(&code, "Ann").await;

    // Mallory starts a step and gets Ann to follow it, and Ann says yes on Discord.
    let url = text(&server.start_link(&mallory.key, &bot).await.ok()["url"]);
    server.finish_link(&ann.key, &step_of(&url), "ann").await.refused(StatusCode::BAD_REQUEST);
    assert!(server.state.db.discord_link(&bot).unwrap().is_none());
    assert!(server.discord.lock().unwrap().exchanged.is_empty(), "Discord is never asked");

    // And the step is spent, so Mallory can't try it again.
    server.finish_link(&mallory.key, &step_of(&url), "ann").await.refused(StatusCode::BAD_REQUEST);
}

// A18
#[tokio::test]
async fn a_step_of_any_other_kind_gets_the_same_plain_400() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let unknown = server.finish_link(&ann.key, "no-such-step", "ann").await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);

    let passkey = server.post("", "/api/passkeys/login/start", json!({})).await.ok();
    let passkey_step = server.finish_link(&ann.key, &text(&passkey["ceremony_id"]), "ann").await;
    assert_eq!(passkey_step.status, StatusCode::BAD_REQUEST);
    assert_eq!(passkey_step.body, unknown.body, "nothing tells them apart");

    let url = text(&server.start_link(&ann.key, &bot).await.ok()["url"]);
    server.finish_link(&ann.key, &step_of(&url), "ann").await.ok();
    let used = server.finish_link(&ann.key, &step_of(&url), "ann").await;
    assert_eq!(used.body, unknown.body, "nothing tells them apart");

    // And it doesn't work the other way round.
    let url = text(&server.start_link(&ann.key, &bot).await.ok()["url"]);
    let as_a_passkey = json!({ "ceremony_id": step_of(&url), "credential": {} });
    server
        .post("", "/api/passkeys/login/finish", as_a_passkey)
        .await
        .refused(StatusCode::UNAUTHORIZED);
}

// A19

#[tokio::test]
async fn linking_again_replaces_the_account_and_brings_a_new_username_here() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    server.link(&ann, &bot, "ann").await;
    server.link(&ann, &bot, "ann-renamed").await;
    assert_eq!(server.in_room(&ann.key, &room, &bot).await["discord"]["name"], "ann_renamed");
    server.link(&ann, &bot, "bo").await;
    assert_eq!(
        server.in_room(&ann.key, &room, &bot).await["discord"],
        json!({ "id": "2000000000000000002", "name": "bo.on.discord" })
    );
}

// A19
#[tokio::test]
async fn one_discord_account_can_be_linked_to_several_bots_whoever_owns_them() {
    let server = Server::start().await;
    let (room, ann, code) = server.someone("Ann").await;
    let (first, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let (second, _) = server.bot(&ann, &room, "Calendar Cal").await;
    let other_account = server.join(&code, "Also Ann").await;
    let (third, _) = server.bot(&other_account, &room, "Third").await;
    server.link(&ann, &first, "ann").await;
    server.link(&ann, &second, "ann").await;
    server.link(&other_account, &third, "ann").await;
    for bot in [&first, &second, &third] {
        assert_eq!(server.in_room(&ann.key, &room, bot).await["discord"]["name"], "ann_on_discord");
    }
}

// A20

#[tokio::test]
async fn unlinking_deletes_it_straight_away_and_touches_only_that_bot() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (first, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let (second, _) = server.bot(&ann, &room, "Calendar Cal").await;
    server.link(&ann, &first, "ann").await;
    server.link(&ann, &second, "ann").await;
    server.delete(&ann.key, &format!("/api/agents/{first}/discord")).await.ok();
    assert!(server.state.db.discord_link(&first).unwrap().is_none());
    assert!(server.in_room(&ann.key, &room, &first).await.get("discord").is_none());
    assert!(server.state.db.discord_link(&second).unwrap().is_some());
    // Doing it twice is fine.
    server.delete(&ann.key, &format!("/api/agents/{first}/discord")).await.ok();
}

// A20
#[tokio::test]
async fn unlinking_works_with_linking_turned_off() {
    let server = Server::with(false).await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    server.state.db.set_discord_link(&bot, "1000000000000000001", "ann_on_discord", now()).unwrap();
    server.delete(&ann.key, &format!("/api/agents/{bot}/discord")).await.ok();
    assert!(server.state.db.discord_link(&bot).unwrap().is_none());
}

// A21

#[tokio::test]
async fn people_in_the_room_see_a_link_beside_the_rooms_own_bot() {
    let server = Server::start().await;
    let (room, ann, code) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let bo = server.join(&code, "Bo").await;
    server.link(&ann, &bot, "ann").await;
    assert_eq!(server.in_room(&bo.key, &room, &bot).await["discord"]["name"], "ann_on_discord");
}

// A21
#[tokio::test]
async fn someone_who_walked_onto_a_public_floor_sees_no_link() {
    let server = Server::start().await;
    let (_, ann, _) = server.someone("Ann").await;
    let made = server.post(&ann.key, "/api/hangouts", json!({ "name": "The Cafe" })).await.ok();
    let room = text(&made["hangout"]["id"]);
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    server.link(&ann, &bot, "ann").await;
    server
        .send(
            &ann.key,
            Method::PUT,
            &format!("/api/floors/{room}"),
            Some(json!({ "access": "public" })),
        )
        .await
        .ok();
    let (_, stranger, _) = server.someone("Stranger").await;
    server.post(&stranger.key, &format!("/api/floors/{room}/walk-in"), json!({})).await.ok();
    assert!(server.in_room(&stranger.key, &room, &bot).await.get("discord").is_none());
    let snapshot = server.get(&stranger.key, &format!("/api/hangouts/{room}")).await.ok();
    assert!(!snapshot.to_string().contains("ann_on_discord"));
    assert_eq!(snapshot["discord_linking"], false);
    // Ann still sees it.
    assert!(server.in_room(&ann.key, &room, &bot).await.get("discord").is_some());
}

// A21
#[tokio::test]
async fn a_bot_thats_only_visiting_shows_no_link() {
    let server = Server::start().await;
    let (home, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &home, "Sequence Sam").await;
    server.link(&ann, &bot, "ann").await;
    let (_, bo, _) = server.someone("Bo").await;
    let made = server.post(&bo.key, "/api/hangouts", json!({ "name": "The Cafe" })).await.ok();
    let room = text(&made["hangout"]["id"]);
    server
        .send(
            &bo.key,
            Method::PUT,
            &format!("/api/floors/{room}"),
            Some(json!({ "access": "public" })),
        )
        .await
        .ok();
    server
        .post(&ann.key, &format!("/api/floors/{room}/walk-in"), json!({ "agent_id": bot }))
        .await
        .ok();
    let visiting = server.in_room(&bo.key, &room, &bot).await;
    assert!(visiting["visitor_until"].is_number(), "{visiting}");
    assert!(visiting.get("discord").is_none());
}

// A21, A22
#[tokio::test]
async fn people_in_a_city_see_a_link_and_their_bots_dont() {
    let server = Server::start().await;
    let made =
        server.post("", "/api/cities", json!({ "name": "Elm", "your_name": "Ann" })).await.ok();
    let ann = who(&made);
    let city = text(&made["city"]["id"]);
    let house = server
        .post(&ann.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Ann's" }))
        .await
        .ok();
    let (bot, token) = server.bot(&ann, &text(&house["living_room_id"]), "Sequence Sam").await;
    server.link(&ann, &bot, "ann").await;
    let path = format!("/api/cities/{city}");

    let view = server.get(&ann.key, &path).await.ok();
    assert_eq!(view["bots"][0]["discord"]["name"], "ann_on_discord");
    let view = server.get(&token, &path).await.ok();
    assert_eq!(view["bots"][0]["id"], bot.as_str());
    assert!(!view.to_string().contains("ann_on_discord"));
}

// A22

#[tokio::test]
async fn a_bot_never_reads_a_link_not_even_its_own() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, token) = server.bot(&ann, &room, "Sequence Sam").await;
    server.link(&ann, &bot, "ann").await;
    for path in [format!("/api/hangouts/{room}"), "/api/me".to_string(), "/api/inbox".to_string()] {
        let read = server.get(&token, &path).await.ok().to_string();
        assert!(!read.contains("ann_on_discord"), "{path}: {read}");
        assert!(!read.contains("1000000000000000001"), "{path}: {read}");
    }
    let snapshot = server.get(&token, &format!("/api/hangouts/{room}")).await.ok();
    assert_eq!(snapshot["discord_linking"], false);
    for tool in ["read_room", "inbox"] {
        let call = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": { "hangout_id": room } },
        });
        let read = server.post(&token, "/mcp", call).await.ok();
        assert_eq!(read["result"]["isError"], json!(null), "{tool}: {read}");
        assert!(read.to_string().contains("clock"), "{tool} should have read something: {read}");
        assert!(!read.to_string().contains("ann_on_discord"), "{tool}: {read}");
    }
}

// A23

#[tokio::test]
async fn nothing_public_carries_a_link() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, token) = server.bot(&ann, &room, "Sequence Sam").await;
    server.link(&ann, &bot, "ann").await;
    let joined = server.post(&token, "/api/public/join", json!({})).await.ok();
    worlds::housekeeping(&server.state, now());
    for path in ["/api/worlds/commons", "/api/worlds", "/worlds"] {
        let shown = server.get("", path).await.ok().to_string();
        assert!(!shown.contains("ann_on_discord"), "{path}");
    }
    let player = text(&joined["player_token"]);
    let mine = server.get(&player, "/api/worlds/commons/me").await.ok().to_string();
    assert!(!mine.contains("ann_on_discord"), "{mine}");
    let look = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "look", "arguments": { "player_token": player } },
    });
    let read = server.post("", "/public/mcp", look).await.ok();
    assert_eq!(read["result"]["isError"], json!(null), "{read}");
    assert!(!read.to_string().contains("ann_on_discord"), "{read}");
}

// A24

#[tokio::test]
async fn a_link_gives_no_standing() {
    let server = Server::start().await;
    let made =
        server.post("", "/api/cities", json!({ "name": "Elm", "your_name": "Ann" })).await.ok();
    let ann = who(&made);
    let city = text(&made["city"]["id"]);
    let house = server
        .post(&ann.key, &format!("/api/cities/{city}/houses"), json!({ "name": "Ann's" }))
        .await
        .ok();
    let (bot, _) = server.bot(&ann, &text(&house["living_room_id"]), "Sequence Sam").await;
    let path = format!("/api/cities/{city}/people/{}/standing", ann.id);
    let before = server.get(&ann.key, &path).await.ok();
    server.link(&ann, &bot, "ann").await;
    assert_eq!(server.get(&ann.key, &path).await.ok(), before);
    let me = server.get(&ann.key, "/api/me").await.ok();
    assert_eq!(me["name"], "Ann", "their name here stays their own");
}

// A25

#[tokio::test]
async fn with_no_discord_app_set_up_nothing_is_offered() {
    let server = Server::with(false).await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let snapshot = server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
    assert_eq!(snapshot["discord_linking"], false);
    server.start_link(&ann.key, &bot).await.refused(StatusCode::CONFLICT);
    server.finish_link(&ann.key, "any", "ann").await.refused(StatusCode::CONFLICT);
}

// A25
#[tokio::test]
async fn with_one_set_up_a_person_is_offered_it() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let snapshot = server.get(&ann.key, &format!("/api/hangouts/{room}")).await.ok();
    assert_eq!(snapshot["discord_linking"], true);
}

// A25
#[test]
fn it_takes_both_settings_to_turn_it_on() {
    assert!(discord::App::from_settings(None, None).is_none());
    assert!(discord::App::from_settings(Some("id".into()), None).is_none());
    assert!(discord::App::from_settings(None, Some("secret".into())).is_none());
    assert!(discord::App::from_settings(Some(String::new()), Some("secret".into())).is_none());
    assert!(discord::App::from_settings(Some("id".into()), Some(String::new())).is_none());
    let app = discord::App::from_settings(Some("id".into()), Some("secret".into())).unwrap();
    assert_eq!(app.authorize_url, "https://discord.com/oauth2/authorize");
    assert_eq!(app.api_url, "https://discord.com/api/v10");
}

// A26

#[tokio::test]
async fn the_privacy_page_says_what_a_link_keeps_and_the_home_page_points_at_it() {
    let server = Server::start().await;
    let page = server.get("", "/privacy").await.ok().to_string();
    for words in ["Discord", "username", "Unlink", "backup", "owner key", "hash"] {
        assert!(page.contains(words), "the privacy page should mention {words}");
    }
    let home = server.get("", "/").await.ok().to_string();
    assert!(home.contains("href=\\\"/privacy\\\""), "the home page should link to it");
    let sitemap = server.get("", "/sitemap.xml").await.ok().to_string();
    assert!(sitemap.contains("/privacy"));
}
