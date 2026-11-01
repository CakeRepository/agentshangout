//! Passkey sign-in and real sessions, over a real server. A software authenticator (see
//! `support`) stands in for the phone or security key, so these are full ceremonies: the same
//! JSON a browser would send, signed for a `http://localhost:PORT` origin.

mod support;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use agenthangout::{app, auth, db::Db, now, passkey, AppState, Config};
use reqwest::{header, Client, Method, StatusCode};
use serde_json::{json, Value};
use support::{b64, tamper, Authenticator};

struct Server {
    /// Where requests go. It is the same server as `origin`, by its number rather than its name.
    base: String,
    /// What the server believes its address is, and so what the authenticator signs for.
    origin: String,
    client: Client,
    state: AppState,
}

struct Reply {
    status: StatusCode,
    body: Value,
    set_cookie: Option<String>,
}

/// Whoever is calling: a bearer token (an owner key, a bot token, or a session token), and a
/// browser's session cookie. A reply that sets the cookie updates it, as a browser would.
#[derive(Clone, Default)]
struct Visitor {
    bearer: String,
    cookie: String,
}

impl Visitor {
    fn bearer(token: &str) -> Self {
        Self { bearer: token.to_string(), cookie: String::new() }
    }

    fn browser_with(cookie: &str) -> Self {
        Self { bearer: String::new(), cookie: cookie.to_string() }
    }
}

/// Someone who signed up with a passkey, and the browser they did it in.
struct Account {
    browser: Visitor,
    owner_key: String,
    person_id: String,
    key: Authenticator,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not a string: {value}")).to_string()
}

/// The token in a `Set-Cookie` header, which is empty once the cookie has been cleared.
fn cookie_value(set_cookie: &str) -> String {
    let pair = set_cookie.split(';').next().unwrap();
    pair.strip_prefix("ah_session=").expect("the session cookie").to_string()
}

fn code_of(url: &Value) -> String {
    url.as_str().unwrap().rsplit('/').next().unwrap().to_string()
}

/// A5: one plain 401, which says nothing about why.
fn assert_plain_401(reply: &Reply, what: &str) {
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{what}: {}", reply.body);
    assert_eq!(reply.body, json!({ "error": "missing or invalid credentials" }), "{what}");
    assert!(reply.set_cookie.is_none(), "{what}: no cookie for a failed sign-in");
}

impl Server {
    async fn start() -> Self {
        Self::start_at("localhost").await
    }

    /// A server that believes it is at `host`, which can be a name or an IP number.
    async fn start_at(host: &str) -> Self {
        Self::start_url("http", host).await
    }

    /// The same, believing it is reached over `scheme`. The server itself only speaks plain http.
    async fn start_url(scheme: &str, host: &str) -> Self {
        Self::start_with(scheme, host, None).await
    }

    /// A server that wants `create_code` before it starts a hangout or a city.
    async fn start_with_code(create_code: &str) -> Self {
        Self::start_with("http", "localhost", Some(create_code)).await
    }

    async fn start_with(scheme: &str, host: &str, create_code: Option<&str>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let origin = format!("{scheme}://{host}:{port}");
        let state = AppState::new(
            Db::open(":memory:").unwrap(),
            Config {
                public_url: origin.clone(),
                create_code: create_code.map(str::to_string),
                allow_private_webhooks: true,
                keeper_id: None,
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        Self { base: format!("http://127.0.0.1:{port}"), origin, client: Client::new(), state }
    }

    async fn send(
        &self,
        who: &mut Visitor,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let mut request = self.client.request(method, format!("{}{path}", self.base));
        if !who.bearer.is_empty() {
            request = request.bearer_auth(&who.bearer);
        }
        if !who.cookie.is_empty() {
            request = request.header(header::COOKIE, format!("ah_session={}", who.cookie));
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .map(|value| value.to_str().unwrap().to_string());
        if let Some(set_cookie) = &set_cookie {
            who.cookie = cookie_value(set_cookie);
        }
        let text = response.text().await.unwrap();
        Reply { status, set_cookie, body: serde_json::from_str(&text).unwrap_or(Value::String(text)) }
    }

    async fn get(&self, who: &mut Visitor, path: &str) -> Reply {
        self.send(who, Method::GET, path, None).await
    }

    async fn post(&self, who: &mut Visitor, path: &str, body: Value) -> Reply {
        self.send(who, Method::POST, path, Some(body)).await
    }

    async fn delete(&self, who: &mut Visitor, path: &str) -> Reply {
        self.send(who, Method::DELETE, path, None).await
    }

    fn authenticator(&self) -> Authenticator {
        Authenticator::new(&self.origin)
    }

    /// Starts a registration and returns its ceremony id and options.
    async fn register_start(&self, who: &mut Visitor, body: Value) -> (String, Value) {
        let started = self.post(who, "/api/passkeys/register/start", body).await;
        assert_eq!(started.status, StatusCode::OK, "{}", started.body);
        (text(&started.body["ceremony_id"]), started.body["options"].clone())
    }

    async fn register_finish(&self, who: &mut Visitor, ceremony_id: &str, credential: Value) -> Reply {
        let body = json!({ "ceremony_id": ceremony_id, "credential": credential });
        self.post(who, "/api/passkeys/register/finish", body).await
    }

    async fn login_start(&self) -> (String, Value) {
        let started = self.post(&mut Visitor::default(), "/api/passkeys/login/start", json!({})).await;
        assert_eq!(started.status, StatusCode::OK, "{}", started.body);
        (text(&started.body["ceremony_id"]), started.body["options"].clone())
    }

    async fn login_finish(&self, who: &mut Visitor, ceremony_id: &str, credential: Value) -> Reply {
        let body = json!({ "ceremony_id": ceremony_id, "credential": credential });
        self.post(who, "/api/passkeys/login/finish", body).await
    }

    /// Signs in from a fresh browser with one button press.
    async fn sign_in(&self, key: &mut Authenticator) -> (Visitor, Reply) {
        let (id, options) = self.login_start().await;
        let credential = key.sign_in(&options);
        let mut browser = Visitor::default();
        let reply = self.login_finish(&mut browser, &id, credential).await;
        (browser, reply)
    }

    /// Makes a new account with a passkey, the way the sign-up page does.
    async fn sign_up(&self, name: &str) -> Account {
        let mut browser = Visitor::default();
        let mut key = self.authenticator();
        let (id, options) = self.register_start(&mut browser, json!({ "your_name": name })).await;
        let credential = key.register(&options);
        let done = self.register_finish(&mut browser, &id, credential).await;
        assert_eq!(done.status, StatusCode::OK, "{}", done.body);
        Account {
            browser,
            owner_key: text(&done.body["owner_key"]),
            person_id: text(&done.body["person"]["id"]),
            key,
        }
    }

    /// Makes a new person the old way, with an owner key and nothing else.
    async fn key_person(&self, name: &str) -> (String, String) {
        let made = self
            .post(&mut Visitor::default(), "/api/hangouts", json!({ "name": "Home", "your_name": name }))
            .await;
        assert_eq!(made.status, StatusCode::OK, "{}", made.body);
        (text(&made.body["owner_key"]), text(&made.body["person"]["id"]))
    }

    /// A hangout owned by `who`, with its id and the code on its invite link.
    async fn hangout(&self, who: &mut Visitor, name: &str) -> (String, String) {
        let made = self.post(who, "/api/hangouts", json!({ "name": name })).await;
        assert_eq!(made.status, StatusCode::OK, "{}", made.body);
        (text(&made.body["hangout"]["id"]), code_of(&made.body["invite_url"]))
    }

    async fn passkey_count(&self, who: &mut Visitor) -> i64 {
        self.get(who, "/api/me").await.body["passkeys"].as_i64().unwrap()
    }
}

#[tokio::test]
async fn a_new_account_signs_up_and_signs_back_in_with_no_name_typed() {
    let server = Server::start().await;
    let mut browser = Visitor::default();
    let mut key = server.authenticator();

    let available = server.get(&mut browser, "/api/passkeys/available").await;
    assert_eq!(available.body, json!({ "available": true }));

    // Starting a sign-up writes nothing: the account exists only once the passkey checks out (A11).
    let started = server
        .post(
            &mut browser,
            "/api/passkeys/register/start",
            json!({ "your_name": "  Ann  ", "label": "Laptop" }),
        )
        .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(started.body.as_object().unwrap().len(), 2, "{}", started.body);
    assert_eq!(server.state.db.person_count().unwrap(), 0);
    let credential = key.register(&started.body["options"]);
    let done = server.register_finish(&mut browser, &text(&started.body["ceremony_id"]), credential).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);

    // The name is the one that was cleaned at the start, and the reply holds nothing else.
    assert_eq!(done.body["person"]["name"], "Ann");
    let mut fields: Vec<_> = done.body.as_object().unwrap().keys().cloned().collect();
    fields.sort();
    assert_eq!(fields, ["owner_key", "person"]);
    let owner_key = text(&done.body["owner_key"]);
    assert!(owner_key.starts_with("ah_owner_"), "A12: the same kind of key every account has");

    // A12: a session cookie, not the owner key.
    let set_cookie = done.set_cookie.as_deref().unwrap();
    let token = cookie_value(set_cookie);
    assert!(token.starts_with("ah_sess_") && token != owner_key, "{set_cookie}");
    for part in ["HttpOnly", "SameSite=Strict", "Max-Age=7776000"] {
        assert!(set_cookie.contains(part), "A7: {part} in {set_cookie}");
    }
    assert!(!set_cookie.contains("Secure"), "plain http in tests");

    let me = server.get(&mut browser, "/api/me").await;
    assert_eq!(me.body["kind"], "person");
    assert_eq!(me.body["name"], "Ann");
    assert_eq!(me.body["passkeys"], 1);

    // Only a hash of the token is kept (A7).
    assert!(server.state.db.person_by_session(&token, now()).unwrap().is_none());
    let person = server.state.db.person_by_session(&auth::hash_secret(&token), now()).unwrap();
    assert_eq!(person.unwrap().name, "Ann");
    // Good for 90 days (A7).
    let hash = auth::hash_secret(&token);
    assert!(server.state.db.person_by_session(&hash, now() + 90 * 24 * 60 * 60 - 60).unwrap().is_some());
    assert!(server.state.db.person_by_session(&hash, now() + 90 * 24 * 60 * 60 + 60).unwrap().is_none());

    // The list says what a person needs to tell their passkeys apart, and nothing the library keeps.
    let list = server.get(&mut browser, "/api/passkeys").await;
    let passkeys = list.body["passkeys"].as_array().unwrap();
    assert_eq!(passkeys.len(), 1);
    let mut fields: Vec<_> = passkeys[0].as_object().unwrap().keys().cloned().collect();
    fields.sort();
    assert_eq!(fields, ["created_at", "id", "label", "last_used_at"]);
    assert_eq!(passkeys[0]["label"], "Laptop");
    assert!(passkeys[0]["last_used_at"].is_null());

    // The owner key is the way back in if every passkey is lost.
    let by_key = server
        .post(&mut Visitor::default(), "/api/session", json!({ "owner_key": owner_key }))
        .await;
    assert_eq!(by_key.status, StatusCode::OK, "{}", by_key.body);

    // Sign out, then back in with one press and no name.
    let out = server.delete(&mut browser, "/api/session").await;
    assert_eq!(out.status, StatusCode::OK);
    assert!(browser.cookie.is_empty(), "the cookie is cleared");
    assert_eq!(server.get(&mut Visitor::browser_with(&token), "/api/me").await.status, StatusCode::UNAUTHORIZED);

    let (mut again, signed_in) = server.sign_in(&mut key).await;
    assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.body);
    assert_eq!(signed_in.body, json!({ "person": done.body["person"] }));
    assert!(signed_in.set_cookie.as_deref().unwrap().contains("ah_session=ah_sess_"));
    assert_ne!(again.cookie, token, "a new session, not the old one");
    assert_eq!(server.get(&mut again, "/api/me").await.body["name"], "Ann");
    let list = server.get(&mut again, "/api/passkeys").await;
    assert!(list.body["passkeys"][0]["last_used_at"].is_number());
    assert_eq!(server.state.db.person_count().unwrap(), 1);
}

#[tokio::test]
async fn a_signed_in_person_adds_a_second_passkey_and_either_signs_in() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let mut phone = server.authenticator();

    // Signed in, no name is needed, and the options leave out the passkeys Ann already has.
    let (id, options) = server.register_start(&mut ann.browser, json!({ "label": "Phone" })).await;
    let exclude = options["publicKey"]["excludeCredentials"].as_array().unwrap();
    assert_eq!(exclude.len(), 1, "A2");
    assert_eq!(exclude[0]["id"], b64(&ann.key.credential_id));
    assert_eq!(
        options["publicKey"]["user"]["id"],
        b64(ann.key.user_handle.as_ref().unwrap()),
        "one stable handle for one person"
    );
    let credential = phone.register(&options);
    let done = server.register_finish(&mut ann.browser, &id, credential).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(done.body["person"]["id"], ann.person_id.as_str());
    assert!(done.body["owner_key"].is_null(), "she has her key already");
    assert!(done.set_cookie.is_none(), "she's already signed in");
    assert_eq!(server.state.db.person_count().unwrap(), 1);
    assert_eq!(server.passkey_count(&mut ann.browser).await, 2);

    for key in [&mut ann.key, &mut phone] {
        let (mut browser, signed_in) = server.sign_in(key).await;
        assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.body);
        assert_eq!(server.get(&mut browser, "/api/me").await.body["person_id"], ann.person_id.as_str());
    }

    // Removing a passkey: only her own, and even the last can go.
    let list = server.get(&mut ann.browser, "/api/passkeys").await;
    let ids: Vec<String> = list.body["passkeys"].as_array().unwrap().iter().map(|k| text(&k["id"])).collect();
    let labels: Vec<String> = list.body["passkeys"].as_array().unwrap().iter().map(|k| text(&k["label"])).collect();
    assert_eq!(labels, ["Passkey", "Phone"], "in the order they were added, with a plain name when none was given");
    let (bo_key, _) = server.key_person("Bo").await;
    let mut bo = Visitor::bearer(&bo_key);
    let not_his = server.delete(&mut bo, &format!("/api/passkeys/{}", ids[0])).await;
    assert_eq!(not_his.status, StatusCode::NOT_FOUND, "A9");
    assert_eq!(server.passkey_count(&mut ann.browser).await, 2);

    let removed = server.delete(&mut ann.browser, &format!("/api/passkeys/{}", ids[0])).await;
    assert_eq!(removed.body, json!({ "ok": true }));
    assert_eq!(server.delete(&mut ann.browser, &format!("/api/passkeys/{}", ids[0])).await.status, StatusCode::NOT_FOUND);
    assert_plain_401(&server.sign_in(&mut ann.key).await.1, "a removed passkey");
    assert_eq!(server.sign_in(&mut phone).await.1.status, StatusCode::OK);

    let last = server.delete(&mut ann.browser, &format!("/api/passkeys/{}", ids[1])).await;
    assert_eq!(last.status, StatusCode::OK, "A9: the last one can be removed");
    assert_plain_401(&server.sign_in(&mut phone).await.1, "no passkeys left");
    let by_key = server.post(&mut Visitor::default(), "/api/session", json!({ "owner_key": ann.owner_key })).await;
    assert_eq!(by_key.status, StatusCode::OK, "the owner key still gets her in");
}

#[tokio::test]
async fn the_options_insist_on_a_discoverable_passkey_and_a_plain_button() {
    let server = Server::start().await;
    let mut browser = Visitor::default();

    let (_, options) = server.register_start(&mut browser, json!({ "your_name": "Ann" })).await;
    let public_key = &options["publicKey"];
    assert_eq!(public_key["rp"]["id"], "localhost", "A3: the host of the public URL");
    assert_eq!(public_key["authenticatorSelection"]["residentKey"], "required");
    assert_eq!(public_key["authenticatorSelection"]["requireResidentKey"], true);
    assert_eq!(public_key["authenticatorSelection"]["userVerification"], "required");
    assert_eq!(public_key["attestation"], "none");

    let (_, options) = server.login_start().await;
    assert!(options.get("mediation").is_none(), "A1: it runs from a button");
    assert_eq!(options["publicKey"]["allowCredentials"], json!([]), "A1: no name, so no list");
    assert_eq!(options["publicKey"]["userVerification"], "required");
    assert_eq!(options["publicKey"]["rpId"], "localhost");
}

#[tokio::test]
async fn a_used_ceremony_cannot_be_replayed() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;

    // Signing in twice with the very same answer: the first works and the second doesn't.
    let (id, options) = server.login_start().await;
    let credential = ann.key.sign_in(&options);
    let first = server.login_finish(&mut Visitor::default(), &id, credential.clone()).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    let replay = server.login_finish(&mut Visitor::default(), &id, credential).await;
    assert_plain_401(&replay, "a replayed sign-in");

    // A ceremony that failed is used up as well: the right answer afterwards is too late.
    let (id, options) = server.login_start().await;
    let credential = ann.key.sign_in(&options);
    let mut bad = credential.clone();
    tamper(&mut bad, "signature");
    assert_plain_401(&server.login_finish(&mut Visitor::default(), &id, bad).await, "tampered");
    assert_plain_401(&server.login_finish(&mut Visitor::default(), &id, credential).await, "after a failure");

    // The same goes for a registration.
    let mut browser = Visitor::default();
    let mut key = server.authenticator();
    let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Bo" })).await;
    let credential = key.register(&options);
    let first = server.register_finish(&mut browser, &id, credential.clone()).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    let replay = server.register_finish(&mut Visitor::default(), &id, credential).await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST, "{}", replay.body);
    assert_eq!(server.state.db.person_count().unwrap(), 2, "Ann and Bo, once each");
}

#[tokio::test]
async fn a_tampered_signature_or_message_is_refused() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;

    for field in ["signature", "authenticatorData", "clientDataJSON"] {
        let (id, options) = server.login_start().await;
        let mut credential = ann.key.sign_in(&options);
        tamper(&mut credential, field);
        assert_plain_401(&server.login_finish(&mut Visitor::default(), &id, credential).await, field);
    }
    // The untouched answer still works with a fresh ceremony.
    assert_eq!(server.sign_in(&mut ann.key).await.1.status, StatusCode::OK);

    // A signature made by some other key.
    let mut impostor = server.authenticator();
    impostor.credential_id = ann.key.credential_id.clone();
    impostor.user_handle = ann.key.user_handle.clone();
    assert_plain_401(&server.sign_in(&mut impostor).await.1, "someone else's key");

    // A registration whose attestation is tampered with makes nothing.
    let mut browser = Visitor::default();
    let mut key = server.authenticator();
    let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Eve" })).await;
    let mut credential = key.register(&options);
    tamper(&mut credential, "clientDataJSON");
    let done = server.register_finish(&mut browser, &id, credential).await;
    assert_eq!(done.status, StatusCode::BAD_REQUEST, "{}", done.body);
    assert_eq!(server.state.db.person_count().unwrap(), 1, "nothing was made for Eve");
}

#[tokio::test]
async fn a_wrong_origin_or_site_is_refused() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;

    // A phishing page reports its own origin, and a passkey made for another site has another id.
    let port = server.origin.rsplit(':').next().unwrap();
    for origin in [
        "https://evil.example".to_string(),
        "http://localhost:1".to_string(),
        format!("https://localhost:{port}"),
        format!("http://localhost.evil.example:{port}"),
    ] {
        let mut phished = ann.key.clone();
        phished.origin = origin.clone();
        assert_plain_401(&server.sign_in(&mut phished).await.1, &origin);
    }
    let mut elsewhere = ann.key.clone();
    elsewhere.rp_id = "evil.example".into();
    assert_plain_401(&server.sign_in(&mut elsewhere).await.1, "another relying party");

    // Registering from the wrong origin makes no account.
    let mut browser = Visitor::default();
    let mut key = Authenticator::new("https://evil.example");
    let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Eve" })).await;
    let done = server.register_finish(&mut browser, &id, key.register(&options)).await;
    assert_eq!(done.status, StatusCode::BAD_REQUEST, "{}", done.body);
    assert_eq!(server.state.db.person_count().unwrap(), 1);

    // And the real one still works.
    assert_eq!(server.sign_in(&mut ann.key).await.1.status, StatusCode::OK);
}

#[tokio::test]
async fn an_unknown_credential_or_a_mismatched_handle_is_refused() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let mut bo = server.sign_up("Bo").await;

    // Never registered anywhere.
    let mut stranger = server.authenticator();
    stranger.user_handle = ann.key.user_handle.clone();
    assert_plain_401(&server.sign_in(&mut stranger).await.1, "an unknown credential");
    stranger.user_handle = None;
    assert_plain_401(&server.sign_in(&mut stranger).await.1, "no user handle at all");

    // Ann's real credential, claiming to be Bo's: the handle has to agree with whose it is.
    let mut claiming = ann.key.clone();
    claiming.user_handle = bo.key.user_handle.clone();
    assert_plain_401(&server.sign_in(&mut claiming).await.1, "someone else's handle");

    // Nothing was said about which of these accounts exist.
    assert_eq!(server.sign_in(&mut ann.key).await.1.status, StatusCode::OK);
    assert_eq!(server.sign_in(&mut bo.key).await.1.status, StatusCode::OK);

    // Not a passkey answer at all, or none of the fields.
    for credential in [json!("junk"), json!(null), json!({}), json!({ "id": 5 }), json!([])] {
        let (id, _) = server.login_start().await;
        let reply = server.login_finish(&mut Visitor::default(), &id, credential.clone()).await;
        assert_plain_401(&reply, &credential.to_string());
    }
    let nothing = server.post(&mut Visitor::default(), "/api/passkeys/login/finish", json!({})).await;
    assert_plain_401(&nothing, "an empty body");
    let unknown = server.login_finish(&mut Visitor::default(), "no-such-ceremony", json!({})).await;
    assert_plain_401(&unknown, "an unknown ceremony");
    // A registration ceremony isn't a sign-in.
    let (register_id, _) = server.register_start(&mut Visitor::default(), json!({ "your_name": "X" })).await;
    let wrong_kind = server.login_finish(&mut Visitor::default(), &register_id, json!({})).await;
    assert_plain_401(&wrong_kind, "the wrong kind of ceremony");
}

#[tokio::test]
async fn a_counter_going_backwards_is_refused_and_the_good_one_still_works() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;

    ann.key.counter = 10;
    assert_eq!(server.sign_in(&mut ann.key).await.1.status, StatusCode::OK, "counter 11");

    // A copy of the passkey that has fallen behind, or is level (A6).
    for behind in [3, 10] {
        let mut copy = ann.key.clone();
        copy.counter = behind;
        assert_plain_401(&server.sign_in(&mut copy).await.1, &format!("counter {}", behind + 1));
    }

    // The refusals didn't move the saved counter back.
    ann.key.counter = 11;
    assert_eq!(server.sign_in(&mut ann.key).await.1.status, StatusCode::OK, "counter 12");
    let mut copy = ann.key.clone();
    copy.counter = 11;
    assert_plain_401(&server.sign_in(&mut copy).await.1, "counter 12 again");
    ann.key.counter = 49;
    assert_eq!(server.sign_in(&mut ann.key).await.1.status, StatusCode::OK, "counter 50");
}

#[tokio::test]
async fn a_lapsed_ceremony_is_refused() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;

    // Sign-in: answered after the five minutes are up.
    let (id, options) = server.login_start().await;
    let credential = ann.key.sign_in(&options);
    assert_eq!(server.state.ceremonies.open_count(), 1);
    let late = now() + passkey::CEREMONY_SECS + 1;
    let refused = passkey::finish_login(&server.state, None, &id, credential.clone(), late).err().unwrap();
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
    assert_eq!(refused.message, "missing or invalid credentials");
    // It is gone: the right clock can't bring it back.
    assert_plain_401(&server.login_finish(&mut Visitor::default(), &id, credential).await, "after it lapsed");
    assert_eq!(server.state.ceremonies.open_count(), 0);

    // Just inside the time it works.
    let (id, options) = server.login_start().await;
    let credential = ann.key.sign_in(&options);
    let almost = now() + passkey::CEREMONY_SECS - 5;
    assert!(passkey::finish_login(&server.state, None, &id, credential, almost).is_ok());

    // A sign-up that lapses makes no account.
    let mut key = server.authenticator();
    let (id, options) = server.register_start(&mut Visitor::default(), json!({ "your_name": "Bo" })).await;
    let credential = key.register(&options);
    let refused = passkey::finish_registration(&server.state, None, &id, credential, late).err().unwrap();
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert_eq!(server.state.db.person_count().unwrap(), 1);

    // Starting another clears the ones that have lapsed.
    server.login_start().await;
    server.login_start().await;
    assert_eq!(server.state.ceremonies.open_count(), 2);
    server.state.ceremonies.purge(now() + passkey::CEREMONY_SECS + 1);
    assert_eq!(server.state.ceremonies.open_count(), 0);
}

#[tokio::test]
async fn at_most_two_thousand_ceremonies_are_open_and_past_that_the_start_routes_say_429() {
    let server = Server::start().await;
    let webauthn = server.state.webauthn.as_ref().unwrap();
    for _ in 0..passkey::MAX_OPEN_CEREMONIES {
        let (_, state) = webauthn.start_discoverable_authentication().unwrap();
        assert!(server.state.ceremonies.begin(passkey::Pending::Login(state), now(), None).is_some());
    }
    let mut anyone = Visitor::default();
    let login = server.post(&mut anyone, "/api/passkeys/login/start", json!({})).await;
    assert_eq!(login.status, StatusCode::TOO_MANY_REQUESTS, "{}", login.body);
    let register = server
        .post(&mut anyone, "/api/passkeys/register/start", json!({ "your_name": "Ann" }))
        .await;
    assert_eq!(register.status, StatusCode::TOO_MANY_REQUESTS, "{}", register.body);
    assert_eq!(server.state.ceremonies.open_count(), passkey::MAX_OPEN_CEREMONIES);
}

#[tokio::test]
async fn bots_get_403_on_every_passkey_route_and_use_up_no_ceremony() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let (room, _) = server.hangout(&mut ann.browser, "Crew").await;
    let made = server.post(&mut ann.browser, &format!("/api/hangouts/{room}/agents"), json!({ "name": "Scout" })).await;
    let mut scout = Visitor::bearer(&text(&made.body["token"]));
    let passkey_id = text(&server.get(&mut ann.browser, "/api/passkeys").await.body["passkeys"][0]["id"]);

    // Two real ceremonies are waiting. A bot that somehow had their ids still can't touch them.
    let (login_id, login_options) = server.login_start().await;
    let (register_id, register_options) =
        server.register_start(&mut Visitor::default(), json!({ "your_name": "Bo" })).await;
    let waiting = server.state.ceremonies.open_count();
    assert_eq!(waiting, 2);
    let login_credential = ann.key.sign_in(&login_options);
    let mut bo_key = server.authenticator();
    let register_credential = bo_key.register(&register_options);

    let routes: Vec<(Method, String, Option<Value>)> = vec![
        (Method::GET, "/api/passkeys/available".into(), None),
        (Method::POST, "/api/passkeys/register/start".into(), Some(json!({ "your_name": "Scout" }))),
        (Method::POST, "/api/passkeys/register/start".into(), Some(json!({}))),
        (Method::POST, "/api/passkeys/register/start".into(), None),
        (
            Method::POST,
            "/api/passkeys/register/finish".into(),
            Some(json!({ "ceremony_id": register_id, "credential": register_credential })),
        ),
        (Method::POST, "/api/passkeys/register/finish".into(), Some(json!({}))),
        (Method::POST, "/api/passkeys/login/start".into(), Some(json!({}))),
        (Method::POST, "/api/passkeys/login/start".into(), None),
        (
            Method::POST,
            "/api/passkeys/login/finish".into(),
            Some(json!({ "ceremony_id": login_id, "credential": login_credential })),
        ),
        (Method::POST, "/api/passkeys/login/finish".into(), Some(json!({}))),
        (Method::GET, "/api/passkeys".into(), None),
        (Method::DELETE, format!("/api/passkeys/{passkey_id}"), None),
    ];
    for (method, path, body) in routes {
        let reply = server.send(&mut scout, method.clone(), &path, body).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{method} {path}: {}", reply.body);
        assert!(reply.set_cookie.is_none());
    }
    assert_eq!(server.state.ceremonies.open_count(), waiting, "a bot started and finished nothing");
    assert_eq!(server.passkey_count(&mut ann.browser).await, 1, "and Ann's passkey is still there");

    // The ceremonies are still good for the people they belong to.
    let signed_in = server.login_finish(&mut Visitor::default(), &login_id, login_credential).await;
    assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.body);
    let signed_up = server.register_finish(&mut Visitor::default(), &register_id, register_credential).await;
    assert_eq!(signed_up.status, StatusCode::OK, "{}", signed_up.body);

    // A bot's token, a session cookie holding it, and the house bot are all bots.
    let mut cookie_bot = Visitor::browser_with(&scout.bearer);
    let reply = server.post(&mut cookie_bot, "/api/passkeys/login/start", json!({})).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn signing_out_ends_the_session_on_the_server_and_any_open_stream() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let (room, code) = server.hangout(&mut ann.browser, "Crew").await;
    let (bo_key, _) = server.key_person("Bo").await;
    let mut bo = Visitor::bearer(&bo_key);
    assert_eq!(
        server.post(&mut bo, &format!("/api/invites/{code}/join"), json!({})).await.status,
        StatusCode::OK
    );

    // Ann's page has a stream open. A message reaches it while she is signed in.
    let token = ann.browser.cookie.clone();
    let mut stream = server
        .client
        .get(format!("{}/api/hangouts/{room}/stream", server.base))
        .header(header::COOKIE, format!("ah_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    server.post(&mut bo, &format!("/api/hangouts/{room}/messages"), json!({ "text": "first" })).await;
    let chunk = tokio::time::timeout(Duration::from_secs(5), stream.chunk()).await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&chunk.unwrap()).contains("first"));

    // She signs out. The server forgets the session, not just the browser.
    let out = server.delete(&mut ann.browser, "/api/session").await;
    assert_eq!(out.status, StatusCode::OK);
    let cleared = out.set_cookie.unwrap();
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    let mut stolen = Visitor::browser_with(&token);
    assert_eq!(server.get(&mut stolen, "/api/me").await.status, StatusCode::UNAUTHORIZED);
    stolen.bearer = token.clone();
    stolen.cookie.clear();
    assert_eq!(server.get(&mut stolen, "/api/me").await.status, StatusCode::UNAUTHORIZED, "nor as a bearer token");
    assert!(server.state.db.person_by_session(&auth::hash_secret(&token), now()).unwrap().is_none());

    // The next message ends her stream instead of reaching it.
    server.post(&mut bo, &format!("/api/hangouts/{room}/messages"), json!({ "text": "second" })).await;
    let mut heard = String::new();
    let read = async {
        while let Some(chunk) = stream.chunk().await.unwrap() {
            heard.push_str(&String::from_utf8_lossy(&chunk));
        }
    };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .expect("the stream should close once the session has ended");
    assert!(!heard.contains("second"), "{heard}");

    // Signing out when already signed out is quiet, and ends nothing of anyone else's.
    let again = server.delete(&mut Visitor::default(), "/api/session").await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(server.get(&mut bo, "/api/me").await.status, StatusCode::OK);
}

#[tokio::test]
async fn a_lapsed_session_is_refused_and_ends_a_stream() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let (room, code) = server.hangout(&mut ann.browser, "Crew").await;
    let (bo_key, _) = server.key_person("Bo").await;
    let mut bo = Visitor::bearer(&bo_key);
    server.post(&mut bo, &format!("/api/invites/{code}/join"), json!({})).await;

    // Already lapsed: not a session at all.
    let old = auth::new_session_token();
    server.state.db.create_session(&auth::hash_secret(&old), &ann.person_id, now() - 1, now() - 100).unwrap();
    assert_eq!(server.get(&mut Visitor::browser_with(&old), "/api/me").await.status, StatusCode::UNAUTHORIZED);

    // Good for two more seconds, then the stream it had open ends.
    let brief = auth::new_session_token();
    server.state.db.create_session(&auth::hash_secret(&brief), &ann.person_id, now() + 2, now()).unwrap();
    assert_eq!(server.get(&mut Visitor::browser_with(&brief), "/api/me").await.status, StatusCode::OK);
    let mut stream = server
        .client
        .get(format!("{}/api/hangouts/{room}/stream", server.base))
        .header(header::COOKIE, format!("ah_session={brief}"))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(3200)).await;
    server.post(&mut bo, &format!("/api/hangouts/{room}/messages"), json!({ "text": "late" })).await;
    let read = async { while let Some(_chunk) = stream.chunk().await.unwrap() {} };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .expect("the stream should close once the session has lapsed");
    assert_eq!(server.get(&mut Visitor::browser_with(&brief), "/api/me").await.status, StatusCode::UNAUTHORIZED);
    // Her real session is untouched.
    assert_eq!(server.get(&mut ann.browser, "/api/me").await.status, StatusCode::OK);
}

#[tokio::test]
async fn replacing_the_owner_key_ends_other_sessions_and_keeps_passkeys() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let (room, _) = server.hangout(&mut ann.browser, "Crew").await;

    // A second device, signed in with the owner key, with a stream open.
    let mut tablet = Visitor::default();
    let by_key = server.post(&mut tablet, "/api/session", json!({ "owner_key": ann.owner_key })).await;
    assert_eq!(by_key.status, StatusCode::OK, "{}", by_key.body);
    assert!(tablet.cookie.starts_with("ah_sess_"), "a session, not the key: {}", tablet.cookie);
    assert_ne!(tablet.cookie, ann.browser.cookie);
    let mut tablet_stream = server
        .client
        .get(format!("{}/api/hangouts/{room}/stream", server.base))
        .header(header::COOKIE, format!("ah_session={}", tablet.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(tablet_stream.status(), StatusCode::OK);
    let old_cookie = ann.browser.cookie.clone();
    let tablet_cookie = tablet.cookie.clone();

    // A8: replaced from the first device.
    let rotated = server.post(&mut ann.browser, "/api/key/rotate", json!({})).await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.body);
    let new_key = text(&rotated.body["owner_key"]);
    assert_ne!(new_key, ann.owner_key);
    assert!(rotated.set_cookie.is_some(), "this device gets a new session");
    assert_ne!(ann.browser.cookie, old_cookie);
    assert!(ann.browser.cookie.starts_with("ah_sess_"));
    assert_ne!(ann.browser.cookie, new_key, "never the key itself");

    // This device carries on. The old session and every other one are over, and so is the old key.
    assert_eq!(server.get(&mut ann.browser, "/api/me").await.status, StatusCode::OK);
    assert_eq!(server.get(&mut Visitor::browser_with(&old_cookie), "/api/me").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&mut Visitor::browser_with(&tablet_cookie), "/api/me").await.status, StatusCode::UNAUTHORIZED);
    let old_key = server.post(&mut Visitor::default(), "/api/session", json!({ "owner_key": ann.owner_key })).await;
    assert_eq!(old_key.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&mut Visitor::bearer(&ann.owner_key), "/api/me").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&mut Visitor::bearer(&new_key), "/api/me").await.status, StatusCode::OK);
    let fresh = server.post(&mut Visitor::default(), "/api/session", json!({ "owner_key": new_key })).await;
    assert_eq!(fresh.status, StatusCode::OK);

    // The other device's open stream ends with the next event.
    let (bo_key, _) = server.key_person("Bo").await;
    let mut bo = Visitor::bearer(&bo_key);
    let invite = server.post(&mut ann.browser, &format!("/api/hangouts/{room}/invite/rotate"), json!({})).await;
    let code = code_of(&invite.body["invite_url"]);
    server.post(&mut bo, &format!("/api/invites/{code}/join"), json!({})).await;
    let read = async { while let Some(_chunk) = tablet_stream.chunk().await.unwrap() {} };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .expect("the other device's stream should close");

    // The passkey is still there, and still signs her in.
    assert_eq!(server.passkey_count(&mut ann.browser).await, 1);
    let (mut again, signed_in) = server.sign_in(&mut ann.key).await;
    assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.body);
    assert_eq!(server.get(&mut again, "/api/me").await.body["person_id"], ann.person_id.as_str());
}

#[tokio::test]
async fn an_owner_key_signs_in_for_a_session_and_an_old_cookie_holding_one_keeps_working() {
    let server = Server::start().await;
    let (key, person_id) = server.key_person("Cy").await;
    // Making Cy signed someone in already, whose cookie was thrown away.
    let at_the_start = server.state.db.session_count(&person_id).unwrap();
    assert_eq!(at_the_start, 1);

    // The old way: the cookie holds the key itself.
    let mut old_style = Visitor::browser_with(&key);
    let me = server.get(&mut old_style, "/api/me").await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.body);
    assert_eq!(me.body["person_id"], person_id.as_str());
    assert_eq!(me.body["passkeys"], 0);
    // Signing out clears the cookie and ends nothing: the key is still the key.
    let out = server.delete(&mut old_style, "/api/session").await;
    assert_eq!(out.status, StatusCode::OK);
    assert!(old_style.cookie.is_empty());
    assert_eq!(server.get(&mut Visitor::bearer(&key), "/api/me").await.status, StatusCode::OK);

    // The new way: the key is traded for a token, and a token is not a key.
    let mut browser = Visitor::default();
    let signed_in = server.post(&mut browser, "/api/session", json!({ "owner_key": format!("  {key}  ") })).await;
    assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.body);
    assert_eq!(signed_in.body, json!({ "ok": true }));
    let token = browser.cookie.clone();
    assert!(token.starts_with("ah_sess_"));
    let as_key = server.post(&mut Visitor::default(), "/api/session", json!({ "owner_key": token })).await;
    assert_eq!(as_key.status, StatusCode::UNAUTHORIZED, "a session token can't start another session");
    let wrong = server.post(&mut Visitor::default(), "/api/session", json!({ "owner_key": "ah_owner_nope" })).await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert!(wrong.set_cookie.is_none());

    // Signing in again from a browser that has a session ends that one: nothing is left behind.
    let mut again = Visitor::browser_with(&token);
    assert_eq!(server.post(&mut again, "/api/session", json!({ "owner_key": key })).await.status, StatusCode::OK);
    assert_ne!(again.cookie, token);
    assert_eq!(server.get(&mut Visitor::browser_with(&token), "/api/me").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&mut again, "/api/me").await.status, StatusCode::OK);
    assert_eq!(server.state.db.session_count(&person_id).unwrap(), at_the_start + 1);
}

#[tokio::test]
async fn a_registration_belongs_to_whoever_started_it() {
    let server = Server::start().await;
    let mut alice = server.sign_up("Alice").await;
    let mut bob = server.sign_up("Bob").await;
    let people = server.state.db.person_count().unwrap();

    // Alice, signed in, starts adding a passkey. Nobody else can finish it, and a failed
    // attempt uses it up.
    let (id, options) = server.register_start(&mut alice.browser, json!({})).await;
    let mut stolen = server.authenticator();
    let credential = stolen.register(&options);
    let by_anyone = server.register_finish(&mut Visitor::default(), &id, credential.clone()).await;
    assert_eq!(by_anyone.status, StatusCode::FORBIDDEN, "{}", by_anyone.body);
    let by_alice = server.register_finish(&mut alice.browser, &id, credential.clone()).await;
    assert_eq!(by_alice.status, StatusCode::BAD_REQUEST, "it was used up: {}", by_alice.body);

    let (id, options) = server.register_start(&mut alice.browser, json!({})).await;
    let credential = stolen.register(&options);
    let by_bob = server.register_finish(&mut bob.browser, &id, credential).await;
    assert_eq!(by_bob.status, StatusCode::FORBIDDEN, "{}", by_bob.body);

    // Someone signed out starts a new account. A person who is signed in can't finish it onto
    // their own account, or into a new one.
    let (id, options) = server.register_start(&mut Visitor::default(), json!({ "your_name": "Mallory" })).await;
    let mut mallory_key = server.authenticator();
    let credential = mallory_key.register(&options);
    let by_alice = server.register_finish(&mut alice.browser, &id, credential.clone()).await;
    assert_eq!(by_alice.status, StatusCode::CONFLICT, "{}", by_alice.body);
    let after = server.register_finish(&mut Visitor::default(), &id, credential).await;
    assert_eq!(after.status, StatusCode::BAD_REQUEST, "that ceremony was used up too");

    assert_eq!(server.state.db.person_count().unwrap(), people, "no Mallory");
    assert_eq!(server.passkey_count(&mut alice.browser).await, 1);
    assert_eq!(server.passkey_count(&mut bob.browser).await, 1);
    assert_plain_401(&server.sign_in(&mut stolen).await.1, "the stolen registration");
    assert_plain_401(&server.sign_in(&mut mallory_key).await.1, "Mallory");

    // Signing out first is what a new account needs, and then it works.
    server.delete(&mut alice.browser, "/api/session").await;
    let (id, options) = server.register_start(&mut alice.browser, json!({ "your_name": "Mallory" })).await;
    let done = server.register_finish(&mut alice.browser, &id, mallory_key.register(&options)).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_ne!(done.body["person"]["id"], alice.person_id.as_str());
    assert_eq!(server.state.db.person_count().unwrap(), people + 1);
}

#[tokio::test]
async fn an_owner_key_as_a_bearer_token_only_ever_acts_for_its_own_account() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let bob = server.sign_up("Bob").await;
    let mut ann_by_key = Visitor::bearer(&ann.owner_key);

    // Whatever the body says, the ceremony is for the key's own person.
    let body = json!({
        "your_name": "Bob",
        "person_id": bob.person_id,
        "user_handle": b64(bob.key.user_handle.as_ref().unwrap()),
        "label": "Not Bob's",
    });
    let (id, options) = server.register_start(&mut ann_by_key, body).await;
    assert_eq!(
        options["publicKey"]["user"]["id"],
        b64(ann.key.user_handle.as_ref().unwrap()),
        "Ann's handle, not Bob's"
    );
    let mut second = server.authenticator();
    let credential = second.register(&options);
    let done = server.register_finish(&mut ann_by_key, &id, credential).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(done.body["person"]["id"], ann.person_id.as_str());
    assert_eq!(server.passkey_count(&mut ann.browser).await, 2);
    let mut bob_browser = bob.browser.clone();
    assert_eq!(server.passkey_count(&mut bob_browser).await, 1, "Bob is untouched");

    // The new passkey signs in as Ann.
    let (mut browser, signed_in) = server.sign_in(&mut second).await;
    assert_eq!(signed_in.status, StatusCode::OK);
    assert_eq!(server.get(&mut browser, "/api/me").await.body["person_id"], ann.person_id.as_str());

    // Nor can a bearer finish a ceremony somebody else started.
    let mut bob_by_key = Visitor::bearer(&bob.owner_key);
    let (id, options) = server.register_start(&mut bob_by_key, json!({})).await;
    let mut third = server.authenticator();
    let credential = third.register(&options);
    let wrong = server.register_finish(&mut ann_by_key, &id, credential).await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN, "{}", wrong.body);
    assert_eq!(server.passkey_count(&mut bob_browser).await, 1);
}

#[tokio::test]
async fn a_duplicate_credential_is_refused_and_makes_no_account() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;

    // The very same passkey offered for a new account (an authenticator that ignores the rule).
    let mut again = ann.key.clone();
    let mut browser = Visitor::default();
    let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Ann Two" })).await;
    let done = server.register_finish(&mut browser, &id, again.register(&options)).await;
    assert_eq!(done.status, StatusCode::CONFLICT, "{}", done.body);
    assert!(done.set_cookie.is_none() && done.body.get("owner_key").is_none(), "{}", done.body);
    assert_eq!(server.state.db.person_count().unwrap(), 1);

    // And offered to Bo as an extra passkey, or to Ann herself again.
    let bo = server.sign_up("Bo").await;
    let mut bo_browser = bo.browser.clone();
    let (id, options) = server.register_start(&mut bo_browser, json!({})).await;
    let mut stolen = ann.key.clone();
    let done = server.register_finish(&mut bo_browser, &id, stolen.register(&options)).await;
    assert_eq!(done.status, StatusCode::CONFLICT, "{}", done.body);
    let (id, options) = server.register_start(&mut ann.browser, json!({})).await;
    let mut same = ann.key.clone();
    let done = server.register_finish(&mut ann.browser, &id, same.register(&options)).await;
    assert_eq!(done.status, StatusCode::BAD_REQUEST, "excluded by her own list: {}", done.body);
    assert_eq!(server.passkey_count(&mut ann.browser).await, 1);
    assert_eq!(server.passkey_count(&mut bo_browser).await, 1);

    // Ann's credential still signs in as Ann, not Bo.
    let (mut browser, signed_in) = server.sign_in(&mut ann.key).await;
    assert_eq!(signed_in.status, StatusCode::OK);
    assert_eq!(server.get(&mut browser, "/api/me").await.body["name"], "Ann");
}

#[tokio::test]
async fn a_passkey_the_browser_says_cannot_be_discovered_is_refused() {
    let server = Server::start().await;
    let mut browser = Visitor::default();
    let mut key = server.authenticator();
    let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Ann" })).await;
    let mut credential = key.register(&options);
    credential["clientExtensionResults"] = json!({ "credProps": { "rk": false } });
    let done = server.register_finish(&mut browser, &id, credential).await;
    assert_eq!(done.status, StatusCode::BAD_REQUEST, "{}", done.body);
    assert_eq!(server.state.db.person_count().unwrap(), 0);

    // One that says it can, or says nothing, is fine.
    let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Ann" })).await;
    let mut credential = key.register(&options);
    credential["clientExtensionResults"] = json!({ "credProps": { "rk": true } });
    assert_eq!(server.register_finish(&mut browser, &id, credential).await.status, StatusCode::OK);
}

#[tokio::test]
async fn new_accounts_are_capped_at_sixty_an_hour() {
    let server = Server::start().await;
    let mut first = None;
    for n in 0..passkey::SIGNUPS_PER_HOUR - 1 {
        let account = server.sign_up(&format!("Person {n}")).await;
        first.get_or_insert(account);
    }

    // Two sign-ups are under way when there is room for one more.
    let mut one = (Visitor::default(), server.authenticator());
    let mut two = (Visitor::default(), server.authenticator());
    let (id_one, options_one) = server.register_start(&mut one.0, json!({ "your_name": "One" })).await;
    let (id_two, options_two) = server.register_start(&mut two.0, json!({ "your_name": "Two" })).await;
    let credential_one = one.1.register(&options_one);
    let credential_two = two.1.register(&options_two);
    assert_eq!(server.register_finish(&mut one.0, &id_one, credential_one).await.status, StatusCode::OK);
    let people = server.state.db.person_count().unwrap();
    assert_eq!(people, passkey::SIGNUPS_PER_HOUR as i64);

    // The second finds the hour used up, and nothing was made. Neither can a third start.
    let refused = server.register_finish(&mut two.0, &id_two, credential_two).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS, "{}", refused.body);
    assert_eq!(server.state.db.person_count().unwrap(), people);
    let third = server
        .post(&mut Visitor::default(), "/api/passkeys/register/start", json!({ "your_name": "Three" }))
        .await;
    assert_eq!(third.status, StatusCode::TOO_MANY_REQUESTS, "{}", third.body);

    // Signing in, and adding a passkey to an account that exists, are not new accounts.
    let mut account = first.unwrap();
    assert_eq!(server.sign_in(&mut account.key).await.1.status, StatusCode::OK);
    let (id, options) = server.register_start(&mut account.browser, json!({})).await;
    let mut extra = server.authenticator();
    let added = server.register_finish(&mut account.browser, &id, extra.register(&options)).await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.body);

    // An hour later there is room again.
    let started = passkey::start_registration(&server.state, None, Some("Four"), None, None, now() + 3601);
    assert!(started.is_ok());
}

#[tokio::test]
async fn passkeys_are_off_at_an_ip_address() {
    let server = Server::start_at("127.0.0.1").await;
    let mut anyone = Visitor::default();

    let available = server.get(&mut anyone, "/api/passkeys/available").await;
    assert_eq!(available.body, json!({ "available": false }));
    for path in ["/api/passkeys/register/start", "/api/passkeys/login/start"] {
        let reply = server.post(&mut anyone, path, json!({ "your_name": "Ann" })).await;
        assert_eq!(reply.status, StatusCode::CONFLICT, "{path}: {}", reply.body);
    }
    for path in ["/api/passkeys/register/finish", "/api/passkeys/login/finish"] {
        let reply = server.post(&mut anyone, path, json!({ "ceremony_id": "x", "credential": {} })).await;
        assert_eq!(reply.status, StatusCode::CONFLICT, "{path}: {}", reply.body);
    }
    assert_eq!(server.state.ceremonies.open_count(), 0);

    // Everything else still works the old way.
    let (key, _) = server.key_person("Ann").await;
    let mut browser = Visitor::default();
    assert_eq!(server.post(&mut browser, "/api/session", json!({ "owner_key": key })).await.status, StatusCode::OK);
    assert_eq!(server.get(&mut browser, "/api/me").await.body["passkeys"], 0);
}

#[tokio::test]
async fn people_made_by_the_old_routes_are_signed_in_with_a_session_not_their_key() {
    let server = Server::start().await;
    let host = server.sign_up("Host").await;
    let mut host_browser = host.browser.clone();
    let city = server
        .post(&mut host_browser, "/api/cities", json!({ "name": "Elm" }))
        .await;
    assert_eq!(city.status, StatusCode::OK, "{}", city.body);
    assert!(city.body["owner_key"].is_null(), "she's signed in, so nothing is made");
    assert!(city.set_cookie.is_none());
    let city_id = text(&city.body["city"]["id"]);
    let city_code = code_of(&city.body["invite_url"]);
    let house = server
        .post(&mut host_browser, &format!("/api/cities/{city_id}/houses"), json!({ "name": "Elm House" }))
        .await;
    let house_code = code_of(&house.body["invite_url"]);
    let (_, hangout_code) = server.hangout(&mut host_browser, "Crew").await;

    // Each of these makes a person from nothing, signs them in, and shows the key once.
    let calls = [
        ("/api/hangouts".to_string(), json!({ "name": "Mine", "your_name": "A" })),
        (format!("/api/invites/{hangout_code}/join"), json!({ "your_name": "B" })),
        ("/api/cities".to_string(), json!({ "name": "Oak", "your_name": "C" })),
        (format!("/api/city-invites/{city_code}/join"), json!({ "your_name": "D" })),
        (format!("/api/house-invites/{house_code}/join"), json!({ "your_name": "E" })),
    ];
    for (path, body) in calls {
        let mut browser = Visitor::default();
        let made = server.post(&mut browser, &path, body).await;
        assert_eq!(made.status, StatusCode::OK, "{path}: {}", made.body);
        let key = text(&made.body["owner_key"]);
        assert!(key.starts_with("ah_owner_"), "{path}");
        assert!(browser.cookie.starts_with("ah_sess_"), "{path}: {}", browser.cookie);
        assert_ne!(browser.cookie, key, "{path}");
        let me = server.get(&mut browser, "/api/me").await;
        assert_eq!(me.body["person_id"], made.body["person"]["id"], "{path}");
        assert_eq!(me.body["passkeys"], 0, "{path}: they can add one later");
        // The key and the session are different credentials for the same person.
        let by_key = server.get(&mut Visitor::bearer(&key), "/api/me").await;
        assert_eq!(by_key.body["person_id"], me.body["person_id"], "{path}");
    }
}

/// A host's city, house and hangout, each with an invite link, for someone to join.
async fn world(server: &Server, name: &str) -> (String, String, String) {
    let (key, _) = server.key_person(name).await;
    let mut host = Visitor::bearer(&key);
    let city = server.post(&mut host, "/api/cities", json!({ "name": format!("{name}ville") })).await;
    let city_id = text(&city.body["city"]["id"]);
    let house = server
        .post(&mut host, &format!("/api/cities/{city_id}/houses"), json!({ "name": format!("{name} House") }))
        .await;
    let (_, hangout_code) = server.hangout(&mut host, &format!("{name} Crew")).await;
    (code_of(&city.body["invite_url"]), code_of(&house.body["invite_url"]), hangout_code)
}

/// Records a step in a day and insists it worked.
fn step(seen: &mut Vec<(String, u16)>, label: &str, reply: &Reply) {
    seen.push((label.to_string(), reply.status.as_u16()));
    assert_eq!(reply.status, StatusCode::OK, "{label}: {}", reply.body);
}

/// Everything a person does from the page, step by step, with the status each step gave.
async fn a_day_in_the_life(server: &Server, who: &mut Visitor, name: &str) -> Vec<(String, u16)> {
    let (city_code, house_code, hangout_code) = world(server, name).await;
    let mut seen = Vec::new();

    let me = server.get(who, "/api/me").await;
    step(&mut seen, "me", &me);
    let person_id = text(&me.body["person_id"]);

    // With someone signed in, they are the person who is used, and no key or cookie is made.
    let made = server.post(who, "/api/hangouts", json!({ "name": "Mine", "your_name": "Someone Else" })).await;
    step(&mut seen, "create a hangout", &made);
    assert_eq!(made.body["person"]["id"], person_id.as_str());
    assert!(made.body["owner_key"].is_null() && made.set_cookie.is_none());
    let room = text(&made.body["hangout"]["id"]);

    let joined = server.post(who, &format!("/api/invites/{hangout_code}/join"), json!({})).await;
    step(&mut seen, "join a hangout", &joined);
    assert_eq!(joined.body["person"]["id"], person_id.as_str());
    let joined = server.post(who, &format!("/api/city-invites/{city_code}/join"), json!({})).await;
    step(&mut seen, "join a city", &joined);
    assert_eq!(joined.body["person"]["id"], person_id.as_str());
    let moved = server.post(who, &format!("/api/house-invites/{house_code}/join"), json!({})).await;
    step(&mut seen, "join a house", &moved);
    assert_eq!(moved.body["person"]["id"], person_id.as_str());
    let started = server.post(who, "/api/cities", json!({ "name": format!("{name} City") })).await;
    step(&mut seen, "start a city", &started);
    assert_eq!(started.body["person"]["id"], person_id.as_str());

    let said = server.post(who, &format!("/api/hangouts/{room}/messages"), json!({ "text": "hi" })).await;
    step(&mut seen, "say something", &said);
    let bot = server.post(who, &format!("/api/hangouts/{room}/agents"), json!({ "name": "Scout" })).await;
    step(&mut seen, "bring a bot", &bot);
    step(&mut seen, "see the room", &server.get(who, &format!("/api/hangouts/{room}")).await);
    step(&mut seen, "read the inbox", &server.get(who, "/api/inbox").await);
    step(&mut seen, "see the passkeys", &server.get(who, "/api/passkeys").await);
    let notify = server.send(who, Method::PUT, "/api/notify", Some(json!({ "url": null }))).await;
    step(&mut seen, "clear the notify url", &notify);
    step(&mut seen, "replace the feed link", &server.post(who, "/api/feed/rotate", json!({})).await);

    // The stream opens.
    let mut request = server.client.get(format!("{}/api/hangouts/{room}/stream", server.base));
    if !who.bearer.is_empty() {
        request = request.bearer_auth(&who.bearer);
    }
    if !who.cookie.is_empty() {
        request = request.header(header::COOKIE, format!("ah_session={}", who.cookie));
    }
    assert_eq!(request.send().await.unwrap().status(), StatusCode::OK);
    seen.push(("open the stream".into(), 200));

    // A bot's token is refused where a person's credential is accepted.
    let mut bot_visitor = Visitor::bearer(&text(&bot.body["token"]));
    let refused = server.post(&mut bot_visitor, "/api/key/rotate", json!({})).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);

    let rotated = server.post(who, "/api/key/rotate", json!({})).await;
    step(&mut seen, "replace the owner key", &rotated);
    // Whoever came in with the key carries on with the new one. A browser was given a new session.
    if !who.bearer.is_empty() {
        who.bearer = text(&rotated.body["owner_key"]);
    }
    step(&mut seen, "carry on afterwards", &server.get(who, "/api/me").await);
    seen
}

#[tokio::test]
async fn a_person_signed_in_by_session_can_do_all_an_owner_key_can() {
    let server = Server::start().await;

    let (key, _) = server.key_person("Kay").await;
    let by_key = a_day_in_the_life(&server, &mut Visitor::bearer(&key), "Kay-host").await;

    let mut ann = server.sign_up("Ann").await;
    let by_session = a_day_in_the_life(&server, &mut ann.browser, "Ann-host").await;

    assert_eq!(by_key, by_session);
    assert!(by_session.len() >= 15, "{by_session:?}");

    // A session token also works as a bearer token, but MCP stays for bots.
    let (key, _) = server.key_person("Lee").await;
    let mut browser = Visitor::default();
    server.post(&mut browser, "/api/session", json!({ "owner_key": key })).await;
    let token = browser.cookie.clone();
    assert!(token.starts_with("ah_sess_"));
    let as_bearer = server.get(&mut Visitor::bearer(&token), "/api/me").await;
    assert_eq!(as_bearer.status, StatusCode::OK, "a session token works as a bearer token too");
    let on_mcp = server
        .post(
            &mut Visitor::bearer(&token),
            "/mcp",
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await;
    assert_eq!(on_mcp.status, StatusCode::FORBIDDEN, "MCP is for bots, however a person signs in");
}

#[tokio::test]
async fn nothing_the_library_keeps_reaches_a_page() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let (mut browser, signed_in) = server.sign_in(&mut ann.key).await;

    let replies = vec![
        signed_in.body.clone(),
        server.get(&mut browser, "/api/passkeys").await.body,
        server.get(&mut browser, "/api/me").await.body,
        server.get(&mut browser, "/api/passkeys/available").await.body,
    ];
    for body in replies {
        let text = body.to_string();
        for forbidden in ["cred_id", "counter", "backup", "COSE", "EC_EC2", "registration_policy", "attestation"] {
            assert!(!text.contains(forbidden), "{forbidden} in {text}");
        }
    }
}

#[tokio::test]
async fn the_cookie_is_secure_when_the_public_address_is_https() {
    let server = Server::start_url("https", "localhost").await;
    let mut browser = Visitor::default();
    let mut key = server.authenticator();
    let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Ann" })).await;
    assert_eq!(options["publicKey"]["rp"]["id"], "localhost");
    let done = server.register_finish(&mut browser, &id, key.register(&options)).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    let set_cookie = done.set_cookie.unwrap();
    for part in ["HttpOnly", "SameSite=Strict", "Secure", "Max-Age=7776000"] {
        assert!(set_cookie.contains(part), "A7: {part} in {set_cookie}");
    }
    let out = server.delete(&mut browser, "/api/session").await;
    assert!(out.set_cookie.unwrap().contains("Secure"));

    // A passkey made for the https origin is not good for the same host over plain http.
    let mut downgraded = key.clone();
    downgraded.origin = server.origin.replace("https", "http");
    assert_plain_401(&server.sign_in(&mut downgraded).await.1, "plain http");
    assert_eq!(server.sign_in(&mut key).await.1.status, StatusCode::OK);
}

// ----------------------------------------------------------------------------------------------
// Adversarial tests, written from docs/accounts-and-trust.md (A1 to A13). Each says which rule it
// is for in a comment such as `// A5`, so coverage can be checked by searching.
// ----------------------------------------------------------------------------------------------

impl Reply {
    fn show(&self) -> String {
        format!("{} {}", self.status, self.body)
    }
}

fn keys_of(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

/// A raw answer: status, every header but the date, and the body exactly as it came.
async fn raw_post(server: &Server, path: &str, who: &Visitor, body: &Value) -> (u16, Vec<(String, String)>, String) {
    let mut request = server.client.post(format!("{}{path}", server.base)).json(body);
    if !who.bearer.is_empty() {
        request = request.bearer_auth(&who.bearer);
    }
    if !who.cookie.is_empty() {
        request = request.header(header::COOKIE, format!("ah_session={}", who.cookie));
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let mut headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .filter(|(name, _)| name.as_str() != "date")
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_string()))
        .collect();
    headers.sort();
    (status, headers, response.text().await.unwrap())
}

impl Server {
    /// Starts a sign-in and answers it with `answer`, returning the raw reply of `login/finish`.
    async fn raw_sign_in(&self, answer: impl FnOnce(&Value) -> Value) -> (u16, Vec<(String, String)>, String) {
        let (id, options) = self.login_start().await;
        let credential = answer(&options);
        raw_post(self, "/api/passkeys/login/finish", &Visitor::default(), &json!({ "ceremony_id": id, "credential": credential })).await
    }

    async fn add_passkey(&self, who: &mut Visitor, label: &str) -> Authenticator {
        let mut key = self.authenticator();
        let (id, options) = self.register_start(who, json!({ "label": label })).await;
        let done = self.register_finish(who, &id, key.register(&options)).await;
        assert_eq!(done.status, StatusCode::OK, "{}", done.body);
        key
    }

    /// A live room stream, as a browser holding this session cookie has it open.
    async fn stream(&self, cookie: &str, room: &str) -> reqwest::Response {
        let response = self
            .client
            .get(format!("{}/api/hangouts/{room}/stream", self.base))
            .header(header::COOKIE, format!("ah_session={cookie}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response
    }

    /// Starts a ceremony as a request that came through the proxy from `forwarded`, which is what
    /// the `X-Forwarded-For` header holds, or straight from this machine if it is `None`.
    async fn start_from(&self, path: &str, forwarded: Option<&str>) -> Reply {
        let mut request = self.client.post(format!("{}{path}", self.base)).json(&json!({ "your_name": "Eve" }));
        if let Some(forwarded) = forwarded {
            request = request.header("X-Forwarded-For", forwarded);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        Reply { status, set_cookie: None, body: response.json().await.unwrap_or(Value::Null) }
    }

    async fn set_notify(&self, who: &mut Visitor, path: &str, url: &str) {
        let saved = self.send(who, Method::PUT, path, Some(json!({ "url": url }))).await;
        assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    }
}

/// A web server that records what's posted to it, standing in for a person's Slack or a bot's webhook.
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

/// Everything that arrived, once long enough has passed for anything on its way to have got there.
async fn arrived(received: &Mutex<Vec<Value>>) -> Vec<Value> {
    tokio::time::sleep(Duration::from_millis(500)).await;
    received.lock().unwrap().clone()
}

// A1
#[tokio::test]
async fn nothing_the_caller_types_picks_the_account_a_sign_in_opens() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let bo = server.sign_up("Bo").await;

    // A sign-in names no one. Anything put in the body is not looked at, and the list of passkeys
    // it will accept stays empty, so a page can't be talked into asking for Bo's.
    let started = server
        .post(
            &mut Visitor::default(),
            "/api/passkeys/login/start",
            json!({
                "username": "bo", "email": "bo@example.com", "password": "hunter2", "person_id": bo.person_id,
                "allowCredentials": [{ "type": "public-key", "id": b64(&bo.key.credential_id) }],
            }),
        )
        .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(started.body["options"]["publicKey"]["allowCredentials"], json!([]));
    let credential = ann.key.sign_in(&started.body["options"]);

    // Finishing with Ann's passkey and a pile of claims to be Bo: it is Ann who is signed in.
    let mut browser = Visitor::default();
    let finish = json!({
        "ceremony_id": started.body["ceremony_id"], "credential": credential,
        "person_id": bo.person_id, "username": "Bo", "user_handle": b64(bo.key.user_handle.as_ref().unwrap()),
        "owner_key": bo.owner_key,
    });
    let done = server.post(&mut browser, "/api/passkeys/login/finish", finish).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    assert_eq!(done.body["person"]["id"], ann.person_id.as_str());
    assert_eq!(server.get(&mut browser, "/api/me").await.body["person_id"], ann.person_id.as_str());

    // A new account is a name, and nothing more: no email or password is read or kept.
    let mut fresh = Visitor::default();
    let mut key = server.authenticator();
    let body = json!({ "your_name": "Cy", "email": "cy@example.com", "password": "hunter2", "username": "cy" });
    let (id, options) = server.register_start(&mut fresh, body).await;
    let made = server.register_finish(&mut fresh, &id, key.register(&options)).await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.body);
    let me = server.get(&mut fresh, "/api/me").await.body.to_string();
    for stored in ["cy@example.com", "hunter2", "\"email\"", "\"password\"", "\"username\""] {
        assert!(!me.contains(stored), "{stored} in {me}");
    }
}

// A2
#[tokio::test]
async fn registering_and_signing_in_both_insist_on_user_verification() {
    let server = Server::start().await;

    // An authenticator that doesn't verify the user can't make an account.
    let mut browser = Visitor::default();
    let mut lax = server.authenticator();
    lax.user_verified = false;
    let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Eve" })).await;
    let done = server.register_finish(&mut browser, &id, lax.register(&options)).await;
    assert_eq!(done.status, StatusCode::BAD_REQUEST, "{}", done.body);
    assert_eq!(server.state.db.person_count().unwrap(), 0);

    // Nor sign in as an account whose passkey was made properly: a stolen, unlocked device.
    let mut ann = server.sign_up("Ann").await;
    let mut lazy = ann.key.clone();
    lazy.user_verified = false;
    assert_plain_401(&server.sign_in(&mut lazy).await.1, "no user verification");
    assert_eq!(server.sign_in(&mut ann.key).await.1.status, StatusCode::OK, "the same passkey, verified, works");

    // Nor add a passkey that won't verify the user to an existing account.
    let mut also_lax = server.authenticator();
    also_lax.user_verified = false;
    let (id, options) = server.register_start(&mut ann.browser, json!({})).await;
    let added = server.register_finish(&mut ann.browser, &id, also_lax.register(&options)).await;
    assert_eq!(added.status, StatusCode::BAD_REQUEST, "{}", added.body);
    assert_eq!(server.passkey_count(&mut ann.browser).await, 1);
}

// A2
#[tokio::test]
async fn the_options_exclude_exactly_the_passkeys_of_the_person_asking_and_no_one_elses() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let mut bo = server.sign_up("Bo").await;
    let second = server.add_passkey(&mut ann.browser, "Phone").await;
    let third = server.add_passkey(&mut ann.browser, "Key").await;

    let ids = |options: &Value| -> Vec<String> {
        options["publicKey"]["excludeCredentials"]
            .as_array()
            .map(|list| list.iter().map(|item| text(&item["id"])).collect())
            .unwrap_or_default()
    };
    let (_, options) = server.register_start(&mut ann.browser, json!({})).await;
    let mut want = vec![b64(&ann.key.credential_id), b64(&second.credential_id), b64(&third.credential_id)];
    want.sort();
    let mut got = ids(&options);
    got.sort();
    assert_eq!(got, want, "all of Ann's and none of Bo's");
    let (_, options) = server.register_start(&mut bo.browser, json!({})).await;
    assert_eq!(ids(&options), [b64(&bo.key.credential_id)]);
    // Someone signed out is making a new account: nothing to exclude, so nothing is learned about anyone.
    let (_, options) = server.register_start(&mut Visitor::default(), json!({ "your_name": "Cy" })).await;
    assert!(ids(&options).is_empty());
    // The owner key as a bearer token is the person it belongs to, no more.
    let (_, options) = server.register_start(&mut Visitor::bearer(&bo.owner_key), json!({})).await;
    assert_eq!(ids(&options), [b64(&bo.key.credential_id)]);
}

// A3
#[tokio::test]
async fn passkeys_are_off_at_every_kind_of_ip_address_and_on_for_a_name() {
    for host in ["127.0.0.1", "[::1]", "203.0.113.9", "10.1.2.3", "[2001:db8::1]"] {
        let server = Server::start_at(host).await;
        let mut anyone = Visitor::default();
        assert_eq!(server.get(&mut anyone, "/api/passkeys/available").await.body, json!({ "available": false }), "{host}");
        for path in ["/api/passkeys/register/start", "/api/passkeys/login/start"] {
            let reply = server.post(&mut anyone, path, json!({ "your_name": "Ann" })).await;
            assert_eq!(reply.status, StatusCode::CONFLICT, "{host} {path}: {}", reply.body);
        }
        assert_eq!(server.state.ceremonies.open_count(), 0, "{host}");
    }
    for (scheme, host) in [("http", "localhost"), ("https", "agentshangout.com"), ("https", "app.example.co.uk")] {
        let server = Server::start_url(scheme, host).await;
        let available = server.get(&mut Visitor::default(), "/api/passkeys/available").await;
        assert_eq!(available.body, json!({ "available": true }), "{scheme}://{host}");
        let (_, options) = server.login_start().await;
        assert_eq!(options["publicKey"]["rpId"], host);
    }
}

// A3
#[tokio::test]
async fn a_neighbouring_origin_or_a_parent_domain_is_not_this_site() {
    let server = Server::start_url("https", "app.example.com").await;
    let mut ann = {
        let mut browser = Visitor::default();
        let mut key = server.authenticator();
        let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Ann" })).await;
        assert_eq!(options["publicKey"]["rp"]["id"], "app.example.com");
        let done = server.register_finish(&mut browser, &id, key.register(&options)).await;
        assert_eq!(done.status, StatusCode::OK, "{}", done.body);
        (browser, key)
    };
    let port = server.origin.rsplit(':').next().unwrap();
    for origin in [
        format!("https://example.com:{port}"),
        format!("https://www.app.example.com:{port}"),
        format!("https://deep.app.example.com:{port}"),
        format!("https://app.example.com.evil.example:{port}"),
        format!("https://evil.example/https://app.example.com:{port}"),
        "https://app.example.com".to_string(),
        format!("http://app.example.com:{port}"),
        format!("https://APP.example.com:{port}.evil"),
    ] {
        let mut phished = ann.1.clone();
        phished.origin = origin.clone();
        assert_plain_401(&server.sign_in(&mut phished).await.1, &origin);
    }
    // A credential made for the parent domain is another site's credential, even though a browser
    // would let a page here ask for it.
    let mut parent = ann.1.clone();
    parent.rp_id = "example.com".into();
    assert_plain_401(&server.sign_in(&mut parent).await.1, "the parent domain's rp id");
    // Registering from any of them makes no account either.
    for origin in [format!("https://example.com:{port}"), format!("https://evil.example:{port}")] {
        let mut browser = Visitor::default();
        let mut key = Authenticator::new(&origin);
        let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Eve" })).await;
        let done = server.register_finish(&mut browser, &id, key.register(&options)).await;
        assert_eq!(done.status, StatusCode::BAD_REQUEST, "{origin}: {}", done.body);
    }
    assert_eq!(server.state.db.person_count().unwrap(), 1);
    assert_eq!(server.sign_in(&mut ann.1).await.1.status, StatusCode::OK);
    let _ = &mut ann.0;
}

// A4
#[tokio::test]
async fn ceremony_ids_are_unguessable_one_kind_never_finishes_as_another_and_no_state_leaves_the_server() {
    let server = Server::start().await;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..40 {
        let started = server.post(&mut Visitor::default(), "/api/passkeys/login/start", json!({})).await;
        assert_eq!(keys_of(&started.body), ["ceremony_id", "options"], "nothing but the id and the options");
        let id = text(&started.body["ceremony_id"]);
        assert!(id.len() >= 32 && id.chars().all(|c| c.is_ascii_alphanumeric()), "{id}");
        assert!(seen.insert(id), "an id repeated");
    }
    let started = server.post(&mut Visitor::default(), "/api/passkeys/register/start", json!({ "your_name": "Ann" })).await;
    assert_eq!(keys_of(&started.body), ["ceremony_id", "options"]);
    let shown = started.body.to_string();
    for state in ["PasskeyRegistration", "DiscoverableAuthentication", "\"state\"", "\"rs\"", "\"policy\"", "user_handle"] {
        assert!(!shown.contains(state), "{state} in {shown}");
    }

    // A sign-in's id can't finish a registration, and the attempt uses it up.
    let mut key = server.authenticator();
    let (login_id, login_options) = server.login_start().await;
    let (register_id, register_options) = server.register_start(&mut Visitor::default(), json!({ "your_name": "Eve" })).await;
    let registered = key.register(&register_options);
    let wrong = server.register_finish(&mut Visitor::default(), &login_id, registered.clone()).await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST, "{}", wrong.body);
    let mut ann = server.sign_up("Ann").await;
    assert_plain_401(&server.login_finish(&mut Visitor::default(), &login_id, ann.key.sign_in(&login_options)).await, "used up by the wrong route");
    // And a registration's id can't sign anyone in.
    let (second_login, second_options) = server.login_start().await;
    let _ = second_login;
    assert_plain_401(&server.login_finish(&mut Visitor::default(), &register_id, ann.key.sign_in(&second_options)).await, "a registration id");
    assert_eq!(server.state.db.person_count().unwrap(), 1, "Eve was never made: her ceremony was spent");
}

// A4
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_ceremony_answered_many_times_at_once_succeeds_once() {
    let server = std::sync::Arc::new(Server::start().await);
    let ann = server.sign_up("Ann").await;
    for round in 0..3u32 {
        let (id, options) = server.login_start().await;
        let mut key = ann.key.clone();
        key.counter = 100 + 1000 * round;
        let credential = key.sign_in(&options);
        let tasks: Vec<_> = (0..12)
            .map(|_| {
                let server = server.clone();
                let (id, credential) = (id.clone(), credential.clone());
                tokio::spawn(async move { server.login_finish(&mut Visitor::default(), &id, credential).await })
            })
            .collect();
        let mut ok = 0;
        for task in tasks {
            let reply = task.await.unwrap();
            if reply.status == StatusCode::OK {
                ok += 1;
            } else {
                assert_plain_401(&reply, "a copy of the same answer");
            }
        }
        assert_eq!(ok, 1, "one answer, one sign-in");
        // Whatever counter that left behind, only a later one signs in again.
        let mut behind = ann.key.clone();
        behind.counter = 5;
        assert_plain_401(&server.sign_in(&mut behind).await.1, "behind");
    }

    // The same for a registration: one account, not twelve.
    let mut key = server.authenticator();
    let (id, options) = server.register_start(&mut Visitor::default(), json!({ "your_name": "Bo" })).await;
    let credential = key.register(&options);
    let tasks: Vec<_> = (0..12)
        .map(|_| {
            let server = server.clone();
            let (id, credential) = (id.clone(), credential.clone());
            tokio::spawn(async move { server.register_finish(&mut Visitor::default(), &id, credential).await })
        })
        .collect();
    let mut made = 0;
    for task in tasks {
        if task.await.unwrap().status == StatusCode::OK {
            made += 1;
        }
    }
    assert_eq!(made, 1);
    assert_eq!(server.state.db.person_count().unwrap(), 2);
}

// A4
#[tokio::test]
async fn one_address_keeps_at_most_thirty_ceremonies_open_and_forging_earlier_forwarded_for_entries_does_not_dodge_it() {
    let server = Server::start().await;
    let register = "/api/passkeys/register/start";
    let login = "/api/passkeys/login/start";

    // Thirty from one address, each claiming to have come through a different one first.
    let mut first = None;
    for n in 0..passkey::MAX_OPEN_PER_ADDRESS {
        let path = if n % 2 == 0 { register } else { login };
        let started = server.start_from(path, Some(&format!("198.51.100.{n}, 203.0.113.7"))).await;
        assert_eq!(started.status, StatusCode::OK, "{n}: {}", started.body);
        first.get_or_insert((text(&started.body["ceremony_id"]), started.body["options"].clone()));
    }
    // The thirty-first is refused whichever route it uses and whatever it says about itself: the
    // last address is the one the proxy saw.
    for (path, forwarded) in [
        (register, "203.0.113.7"),
        (login, "203.0.113.7"),
        (register, "192.0.2.1, 203.0.113.7"),
        (login, "203.0.113.8, 203.0.113.7"),
        (register, "198.51.100.200, 198.51.100.201, 203.0.113.7"),
    ] {
        let refused = server.start_from(path, Some(forwarded)).await;
        assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS, "{path} as {forwarded}: {}", refused.body);
    }
    assert_eq!(server.state.ceremonies.open_count(), passkey::MAX_OPEN_PER_ADDRESS);

    // Another last address is counted on its own, however much it says about the first one, and so
    // is an IPv6 one. A request that went through no proxy is this machine's and isn't limited.
    for last in ["203.0.113.8", "2001:db8::9"] {
        for n in 0..passkey::MAX_OPEN_PER_ADDRESS {
            let started = server.start_from(register, Some(&format!("203.0.113.7, {last}"))).await;
            assert_eq!(started.status, StatusCode::OK, "{last} {n}: {}", started.body);
        }
        let refused = server.start_from(login, Some(&format!("203.0.113.7, {last}"))).await;
        assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS, "{last}: {}", refused.body);
    }
    for n in 0..passkey::MAX_OPEN_PER_ADDRESS + 10 {
        let started = server.start_from(register, None).await;
        assert_eq!(started.status, StatusCode::OK, "no header, try {n}: {}", started.body);
    }

    // Finishing one gives that address a place back. Nothing else does.
    let mut key = server.authenticator();
    let (id, options) = first.unwrap();
    let credential = key.register(&options);
    let done = server.register_finish(&mut Visitor::default(), &id, credential).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    let again = server.start_from(register, Some("203.0.113.7")).await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    let refused = server.start_from(register, Some("203.0.113.7")).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS, "{}", refused.body);
}

// A4
#[tokio::test]
async fn ceremonies_count_an_ipv6_network_as_one_address_and_a_header_nobody_can_read_as_one_too() {
    let server = Server::start().await;
    let register = "/api/passkeys/register/start";
    // Thirty from one IPv6 network, each from another address inside it.
    for n in 0..passkey::MAX_OPEN_PER_ADDRESS {
        let started = server.start_from(register, Some(&format!("2001:db8:abcd:1234::{:x}", n + 1))).await;
        assert_eq!(started.status, StatusCode::OK, "{n}: {}", started.body);
    }
    let refused = server.start_from(register, Some("2001:db8:abcd:1234:ffff::1")).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS, "{}", refused.body);
    // The next network along, and an IPv4-mapped address, are counted on their own.
    for last in ["2001:db8:abcd:1235::1", "::ffff:203.0.113.7"] {
        let started = server.start_from(register, Some(last)).await;
        assert_eq!(started.status, StatusCode::OK, "{last}: {}", started.body);
    }
    // A header with no address in it is limited, and everything like it shares one count.
    for (n, junk) in ["unknown", "203.0.113.7,", "[2001:db8::1]", "-"].iter().cycle().take(passkey::MAX_OPEN_PER_ADDRESS).enumerate() {
        let started = server.start_from(register, Some(junk)).await;
        assert_eq!(started.status, StatusCode::OK, "{n} {junk}: {}", started.body);
    }
    for junk in ["unknown", "nope", ""] {
        let refused = server.start_from("/api/passkeys/login/start", Some(junk)).await;
        assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS, "{junk:?}: {}", refused.body);
    }
}

// A5
#[tokio::test]
async fn every_way_a_sign_in_can_fail_looks_the_same_down_to_the_header() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let mut bo = server.sign_up("Bo").await;
    let mut removed = server.add_passkey(&mut ann.browser, "Old").await;
    let list = server.get(&mut ann.browser, "/api/passkeys").await;
    let old_id = text(&list.body["passkeys"][1]["id"]);
    server.delete(&mut ann.browser, &format!("/api/passkeys/{old_id}")).await;
    // Counters: Ann's passkey has been used up to 20.
    ann.key.counter = 19;
    assert_eq!(server.sign_in(&mut ann.key).await.1.status, StatusCode::OK);

    let mut replay_source = Vec::new();
    let mut failures = Vec::new();
    // Not a ceremony at all, and a body with nothing in it.
    failures.push(("no such ceremony", raw_post(&server, "/api/passkeys/login/finish", &Visitor::default(), &json!({ "ceremony_id": "nope", "credential": {} })).await));
    failures.push(("empty", raw_post(&server, "/api/passkeys/login/finish", &Visitor::default(), &json!({})).await));
    failures.push(("junk", server.raw_sign_in(|_| json!("junk")).await));
    // A passkey nobody has heard of, and one that was removed.
    let mut stranger = server.authenticator();
    stranger.user_handle = ann.key.user_handle.clone();
    failures.push(("unknown credential", server.raw_sign_in(|o| stranger.sign_in(o)).await));
    removed.user_handle = ann.key.user_handle.clone();
    failures.push(("removed passkey", server.raw_sign_in(|o| removed.sign_in(o)).await));
    // Ann's own passkey, wrongly: bad signature, someone else's handle, wrong site, wrong place, behind.
    failures.push(("bad signature", server.raw_sign_in(|o| {
        let mut credential = ann.key.clone().sign_in(o);
        tamper(&mut credential, "signature");
        credential
    }).await));
    failures.push(("someone else's handle", server.raw_sign_in(|o| {
        let mut key = ann.key.clone();
        key.counter += 50;
        key.user_handle = bo.key.user_handle.clone();
        key.sign_in(o)
    }).await));
    failures.push(("wrong origin", server.raw_sign_in(|o| {
        let mut key = ann.key.clone();
        key.counter += 60;
        key.origin = "https://evil.example".into();
        key.sign_in(o)
    }).await));
    failures.push(("wrong rp", server.raw_sign_in(|o| {
        let mut key = ann.key.clone();
        key.counter += 70;
        key.rp_id = "evil.example".into();
        key.sign_in(o)
    }).await));
    failures.push(("no user verification", server.raw_sign_in(|o| {
        let mut key = ann.key.clone();
        key.counter += 80;
        key.user_verified = false;
        key.sign_in(o)
    }).await));
    failures.push(("counter behind", server.raw_sign_in(|o| {
        let mut key = ann.key.clone();
        key.counter = 3;
        key.sign_in(o)
    }).await));
    // A used ceremony: the same answer twice.
    let (id, options) = server.login_start().await;
    let answer = bo.key.sign_in(&options);
    replay_source.push((id.clone(), answer.clone()));
    assert_eq!(server.login_finish(&mut Visitor::default(), &id, answer.clone()).await.status, StatusCode::OK);
    failures.push(("replayed", raw_post(&server, "/api/passkeys/login/finish", &Visitor::default(), &json!({ "ceremony_id": id, "credential": answer })).await));
    // A challenge from some other ceremony.
    let (_, other_options) = server.login_start().await;
    let (id, _) = server.login_start().await;
    let mut key = ann.key.clone();
    key.counter += 90;
    let wrong_challenge = key.assert_challenge(other_options["publicKey"]["challenge"].as_str().unwrap());
    failures.push(("someone else's challenge", raw_post(&server, "/api/passkeys/login/finish", &Visitor::default(), &json!({ "ceremony_id": id, "credential": wrong_challenge })).await));

    let (first_name, first) = &failures[0];
    assert_eq!(first.0, 401, "{first_name}: {first:?}");
    assert!(first.2.contains("missing or invalid credentials"));
    for (name, reply) in &failures {
        assert_eq!(reply, first, "{name} is told apart from {first_name}");
        assert!(!reply.1.iter().any(|(header, _)| header == "set-cookie" || header == "www-authenticate"), "{name}");
    }
    assert!(failures.len() >= 13);
    let _ = replay_source;
}

// A5
#[tokio::test]
async fn an_answer_made_for_one_ceremony_does_not_sign_in_under_another() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;

    // Captured on the wire: a good answer to ceremony one, used.
    let (one, options_one) = server.login_start().await;
    let captured = ann.key.sign_in(&options_one);
    assert_eq!(server.login_finish(&mut Visitor::default(), &one, captured.clone()).await.status, StatusCode::OK);
    // Replayed against a later ceremony: the challenge is not the one it signed.
    let (two, _) = server.login_start().await;
    assert_plain_401(&server.login_finish(&mut Visitor::default(), &two, captured.clone()).await, "replayed under a new ceremony");
    // Two open at once: the answer to A given under B is refused, and A is still good for A.
    let (a, options_a) = server.login_start().await;
    let (b, _) = server.login_start().await;
    let answer_a = ann.key.sign_in(&options_a);
    assert_plain_401(&server.login_finish(&mut Visitor::default(), &b, answer_a.clone()).await, "A's answer under B");
    assert_eq!(server.login_finish(&mut Visitor::default(), &a, answer_a).await.status, StatusCode::OK);

    // The same for registering: the options from one start can't be answered under another's id.
    let mut key = server.authenticator();
    let (first, options_first) = server.register_start(&mut Visitor::default(), json!({ "your_name": "Eve" })).await;
    let (second, _) = server.register_start(&mut Visitor::default(), json!({ "your_name": "Eve" })).await;
    let done = server.register_finish(&mut Visitor::default(), &second, key.register(&options_first)).await;
    assert_eq!(done.status, StatusCode::BAD_REQUEST, "{}", done.body);
    let _ = first;
    assert_eq!(server.state.db.person_count().unwrap(), 1);
}

// A6
#[tokio::test]
async fn an_authenticator_whose_counter_never_moves_keeps_working_and_one_that_goes_back_does_not() {
    let server = Server::start().await;

    // Platform passkeys that report 0 every time must not be locked out.
    let flat = server.sign_up("Flat").await;
    for _ in 0..4 {
        let (id, options) = server.login_start().await;
        let credential = flat.key.assert_challenge(options["publicKey"]["challenge"].as_str().unwrap());
        let done = server.login_finish(&mut Visitor::default(), &id, credential).await;
        assert_eq!(done.status, StatusCode::OK, "counter 0 again: {}", done.body);
    }
    // A counter that has started to count can't go back to zero (a cloned key that doesn't count).
    let mut counting = server.sign_up("Counting").await;
    counting.key.counter = 7;
    assert_eq!(server.sign_in(&mut counting.key).await.1.status, StatusCode::OK);
    let (id, options) = server.login_start().await;
    let zero = counting.key.clone();
    let credential = {
        let mut zero = zero;
        zero.counter = 0;
        zero.assert_challenge(options["publicKey"]["challenge"].as_str().unwrap())
    };
    assert_plain_401(&server.login_finish(&mut Visitor::default(), &id, credential).await, "back to zero");
    // A big jump forward is fine; the way back from it is not.
    counting.key.counter = 4_000_000_000;
    assert_eq!(server.sign_in(&mut counting.key).await.1.status, StatusCode::OK);
    let mut old = counting.key.clone();
    old.counter = 100;
    assert_plain_401(&server.sign_in(&mut old).await.1, "after a jump");
    // Nothing above moved the saved counter backwards or wedged the passkey.
    counting.key.counter += 1;
    assert_eq!(server.sign_in(&mut counting.key).await.1.status, StatusCode::OK);
}

// A6
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_sign_ins_with_the_same_counter_at_once_cannot_both_be_let_in() {
    let server = std::sync::Arc::new(Server::start().await);
    let ann = server.sign_up("Ann").await;
    for round in 0..3u32 {
        let mut key = ann.key.clone();
        key.counter = 1000 * (round + 1);
        // Two ceremonies, two answers carrying the very same counter: a cloned passkey.
        let (id_a, options_a) = server.login_start().await;
        let (id_b, options_b) = server.login_start().await;
        let answer_a = key.assert_challenge(options_a["publicKey"]["challenge"].as_str().unwrap());
        let answer_b = key.assert_challenge(options_b["publicKey"]["challenge"].as_str().unwrap());
        let (s1, s2) = (server.clone(), server.clone());
        let first = tokio::spawn(async move { s1.login_finish(&mut Visitor::default(), &id_a, answer_a).await });
        let second = tokio::spawn(async move { s2.login_finish(&mut Visitor::default(), &id_b, answer_b).await });
        let (first, second) = (first.await.unwrap(), second.await.unwrap());
        let oks = [&first, &second].iter().filter(|r| r.status == StatusCode::OK).count();
        assert_eq!(oks, 1, "{} / {}", first.body, second.body);
    }
}

// A7
#[tokio::test]
async fn a_session_is_a_fresh_random_token_and_no_reply_ever_shows_it_again() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let (room, _) = server.hangout(&mut ann.browser, "Crew").await;
    let mut tokens = std::collections::HashSet::new();
    let mut cookies_seen = Vec::new();
    for _ in 0..12 {
        let (browser, signed_in) = server.sign_in(&mut ann.key).await;
        let set_cookie = signed_in.set_cookie.clone().unwrap();
        cookies_seen.push(set_cookie.clone());
        let token = browser.cookie.clone();
        assert!(token.starts_with("ah_sess_") && token.len() == "ah_sess_".len() + 40, "{token}");
        assert!(token[8..].chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(tokens.insert(token), "a session token repeated");
        for flag in ["HttpOnly", "SameSite=Strict", "Path=/", "Max-Age=7776000"] {
            assert!(set_cookie.contains(flag), "{flag} in {set_cookie}");
        }
        assert!(!signed_in.body.to_string().contains("ah_sess_"), "{}", signed_in.body);
    }
    // Every way of reading what the browser could be shown, as the session itself.
    let mut browser = Visitor::browser_with(tokens.iter().next().unwrap());
    let token = browser.cookie.clone();
    let hash = auth::hash_secret(&token);
    for path in [
        "/api/me".to_string(), "/api/passkeys".to_string(), "/api/passkeys/available".to_string(), "/api/inbox".to_string(),
        format!("/api/hangouts/{room}"), format!("/api/hangouts/{room}/messages?after=0"),
    ] {
        let reply = server.get(&mut browser, &path).await;
        assert_eq!(reply.status, StatusCode::OK, "{path}");
        let shown = reply.body.to_string();
        for secret in [token.as_str(), hash.as_str(), ann.owner_key.as_str(), "ah_sess_", "ah_owner_"] {
            assert!(!shown.contains(secret), "{secret} in {path}: {shown}");
        }
        assert!(reply.set_cookie.is_none(), "{path} set a cookie");
    }
    // Stored only as a hash: the raw token isn't a row, its hash is.
    assert!(server.state.db.person_by_session(&token, now()).unwrap().is_none());
    assert!(server.state.db.person_by_session(&hash, now()).unwrap().is_some());
    // The URL: nothing in a redirect or a link carries it. The sign-in reply names no location.
    for cookie in cookies_seen {
        assert!(!cookie.to_lowercase().contains("domain="), "{cookie}");
    }
}

// A7
#[tokio::test]
async fn a_cookie_that_still_holds_an_owner_key_stops_working_the_moment_the_key_is_replaced() {
    let server = Server::start().await;
    let (key, person_id) = server.key_person("Cy").await;
    let mut old_style = Visitor::browser_with(&key);
    assert_eq!(server.get(&mut old_style, "/api/me").await.body["person_id"], person_id.as_str());
    // Another device, signed in the new way, replaces the key.
    let mut tablet = Visitor::default();
    assert_eq!(server.post(&mut tablet, "/api/session", json!({ "owner_key": key })).await.status, StatusCode::OK);
    let rotated = server.post(&mut tablet, "/api/key/rotate", json!({})).await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.body);
    // The old cookie, the old key as a bearer token, and the old key as a sign-in: all dead.
    assert_eq!(server.get(&mut Visitor::browser_with(&key), "/api/me").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&mut Visitor::bearer(&key), "/api/me").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.post(&mut Visitor::default(), "/api/session", json!({ "owner_key": key })).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&mut tablet, "/api/me").await.body["person_id"], person_id.as_str());
}

// A7
#[tokio::test]
async fn a_person_keeps_at_most_fifty_sessions_and_the_oldest_is_dropped_first() {
    let server = Server::start().await;
    let (key, person_id) = server.key_person("Cy").await;
    let mut tokens = Vec::new();
    for _ in 0..55 {
        let mut browser = Visitor::default();
        assert_eq!(server.post(&mut browser, "/api/session", json!({ "owner_key": key })).await.status, StatusCode::OK);
        tokens.push(browser.cookie.clone());
    }
    assert_eq!(server.state.db.session_count(&person_id).unwrap(), 50);
    // The first five (and the one made along with the account) are gone, the newest fifty are not.
    for token in &tokens[..5] {
        assert_eq!(server.get(&mut Visitor::browser_with(token), "/api/me").await.status, StatusCode::UNAUTHORIZED);
    }
    for token in &tokens[5..] {
        assert_eq!(server.get(&mut Visitor::browser_with(token), "/api/me").await.status, StatusCode::OK);
    }
    // Another person's sessions are not touched by any of it.
    let (other_key, _) = server.key_person("Dee").await;
    let mut dee = Visitor::default();
    server.post(&mut dee, "/api/session", json!({ "owner_key": other_key })).await;
    assert_eq!(server.get(&mut dee, "/api/me").await.status, StatusCode::OK);
}

// A7 A8 A10
#[tokio::test]
async fn token_types_are_not_interchangeable_and_a_bot_token_can_never_start_a_session() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let (room, _) = server.hangout(&mut ann.browser, "Crew").await;
    let made = server.post(&mut ann.browser, &format!("/api/hangouts/{room}/agents"), json!({ "name": "Scout" })).await;
    let bot = text(&made.body["token"]);
    let session = ann.browser.cookie.clone();

    // None of the three is accepted as another when asking for a session.
    for wrong in [bot.as_str(), session.as_str(), "ah_bot_nonsense", ""] {
        let reply = server.post(&mut Visitor::default(), "/api/session", json!({ "owner_key": wrong })).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{wrong}");
        assert!(reply.set_cookie.is_none());
    }
    // A bot's token in the cookie is still a bot: it gets no person's powers.
    let mut cookie_bot = Visitor::browser_with(&bot);
    assert_eq!(server.post(&mut cookie_bot, "/api/key/rotate", json!({})).await.status, StatusCode::FORBIDDEN);
    assert_eq!(server.get(&mut cookie_bot, "/api/passkeys").await.status, StatusCode::FORBIDDEN);
    assert_eq!(server.get(&mut cookie_bot, "/api/me").await.body["kind"], "agent");
    // MCP is for bots alone: a session token or an owner key there is 403, however it is sent.
    let rpc = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    for visitor in [Visitor::bearer(&session), Visitor::browser_with(&session), Visitor::bearer(&ann.owner_key), Visitor::browser_with(&ann.owner_key)] {
        let mut visitor = visitor;
        assert_eq!(server.post(&mut visitor, "/mcp", rpc.clone()).await.status, StatusCode::FORBIDDEN);
    }
    assert_eq!(server.post(&mut Visitor::bearer(&bot), "/mcp", rpc.clone()).await.status, StatusCode::OK);
    // A session token as a bearer works everywhere a key does, but a bot-only route is still a bot's.
    assert_eq!(server.get(&mut Visitor::bearer(&session), "/api/house/queue").await.status, StatusCode::FORBIDDEN);
    assert_eq!(server.get(&mut Visitor::bearer(&ann.owner_key), "/api/house/queue").await.status, StatusCode::FORBIDDEN);
}

// A8
#[tokio::test]
async fn signing_out_one_device_leaves_the_others_and_other_people_alone() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let bo = server.sign_up("Bo").await;
    let (mut tablet, _) = server.sign_in(&mut ann.key).await;
    let (mut phone, _) = server.sign_in(&mut ann.key).await;
    let mut bo_browser = bo.browser.clone();
    let (room, _) = server.hangout(&mut ann.browser, "Crew").await;

    // Signing out the tablet is the tablet's alone.
    let tablet_token = tablet.cookie.clone();
    let out = server.delete(&mut tablet, "/api/session").await;
    assert_eq!(out.status, StatusCode::OK);
    assert_eq!(server.get(&mut Visitor::browser_with(&tablet_token), "/api/me").await.status, StatusCode::UNAUTHORIZED);
    for who in [&mut ann.browser, &mut phone, &mut bo_browser] {
        assert_eq!(server.get(who, "/api/me").await.status, StatusCode::OK);
    }
    assert_eq!(server.get(&mut ann.browser, &format!("/api/hangouts/{room}")).await.status, StatusCode::OK);
    // A request that carries nothing, or only a key, or a bot token, ends nothing of anyone's.
    assert_eq!(server.delete(&mut Visitor::default(), "/api/session").await.status, StatusCode::OK);
    assert_eq!(server.delete(&mut Visitor::bearer(&ann.owner_key), "/api/session").await.status, StatusCode::OK);
    assert_eq!(server.get(&mut phone, "/api/me").await.status, StatusCode::OK);
    assert_eq!(server.get(&mut Visitor::bearer(&ann.owner_key), "/api/me").await.status, StatusCode::OK);
    // Presenting a session as a bearer token ends it as well as the cookie would.
    let token = phone.cookie.clone();
    assert_eq!(server.delete(&mut Visitor::bearer(&token), "/api/session").await.status, StatusCode::OK);
    assert_eq!(server.get(&mut Visitor::browser_with(&token), "/api/me").await.status, StatusCode::UNAUTHORIZED);
}

// A8
#[tokio::test]
async fn replacing_the_owner_key_twice_in_a_row_and_from_a_bearer_session_leaves_one_working_session() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let (phone, _) = server.sign_in(&mut ann.key).await;
    let phone_token = phone.cookie.clone();

    // From the phone, carrying its session as a bearer token rather than a cookie.
    let first = server.post(&mut Visitor::bearer(&phone_token), "/api/key/rotate", json!({})).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    let first_key = text(&first.body["owner_key"]);
    let first_cookie = cookie_value(first.set_cookie.as_deref().unwrap());
    assert_eq!(server.get(&mut Visitor::browser_with(&phone_token), "/api/me").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&mut ann.browser, "/api/me").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&mut Visitor::browser_with(&first_cookie), "/api/me").await.status, StatusCode::OK);
    // And again: now the first key and the first session are gone too.
    let second = server.post(&mut Visitor::bearer(&first_key), "/api/key/rotate", json!({})).await;
    assert_eq!(second.status, StatusCode::OK);
    assert_ne!(text(&second.body["owner_key"]), first_key);
    assert_eq!(server.get(&mut Visitor::bearer(&first_key), "/api/me").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&mut Visitor::browser_with(&first_cookie), "/api/me").await.status, StatusCode::UNAUTHORIZED);
    let second_cookie = cookie_value(second.set_cookie.as_deref().unwrap());
    assert_eq!(server.get(&mut Visitor::browser_with(&second_cookie), "/api/me").await.status, StatusCode::OK);
    assert_eq!(server.state.db.session_count(&ann.person_id).unwrap(), 1);
    // The passkey never noticed.
    assert_eq!(server.sign_in(&mut ann.key).await.1.status, StatusCode::OK);
}

// A9
#[tokio::test]
async fn removing_a_passkey_needs_a_person_signed_in_and_touches_only_their_own() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let mut bo = server.sign_up("Bo").await;
    let (room, _) = server.hangout(&mut ann.browser, "Crew").await;
    let made = server.post(&mut ann.browser, &format!("/api/hangouts/{room}/agents"), json!({ "name": "Scout" })).await;
    let bot = text(&made.body["token"]);
    let bo_passkey = text(&server.get(&mut bo.browser, "/api/passkeys").await.body["passkeys"][0]["id"]);
    let ann_passkey = text(&server.get(&mut ann.browser, "/api/passkeys").await.body["passkeys"][0]["id"]);

    // Nobody, a bot, and someone else's id: nothing is removed, and the last is no different from a made-up id.
    let made_up = server.delete(&mut ann.browser, "/api/passkeys/no-such-id").await;
    assert_eq!(made_up.status, StatusCode::NOT_FOUND);
    let theirs = server.delete(&mut ann.browser, &format!("/api/passkeys/{bo_passkey}")).await;
    assert_eq!((theirs.status, &theirs.body), (made_up.status, &made_up.body));
    assert_eq!(server.delete(&mut Visitor::default(), &format!("/api/passkeys/{ann_passkey}")).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.delete(&mut Visitor::bearer(&bot), &format!("/api/passkeys/{ann_passkey}")).await.status, StatusCode::FORBIDDEN);
    assert_eq!(server.delete(&mut Visitor::browser_with("ah_sess_nonsense"), &format!("/api/passkeys/{ann_passkey}")).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.passkey_count(&mut ann.browser).await, 1);
    assert_eq!(server.passkey_count(&mut bo.browser).await, 1);
    assert_eq!(server.sign_in(&mut bo.key).await.1.status, StatusCode::OK);

    // The owner key, as a bearer, can remove its own person's passkeys and still only theirs.
    let by_key = server.delete(&mut Visitor::bearer(&ann.owner_key), &format!("/api/passkeys/{bo_passkey}")).await;
    assert_eq!(by_key.status, StatusCode::NOT_FOUND);
    assert_eq!(server.delete(&mut Visitor::bearer(&ann.owner_key), &format!("/api/passkeys/{ann_passkey}")).await.status, StatusCode::OK);
    assert_plain_401(&server.sign_in(&mut ann.key).await.1, "removed");
    // Ann is not locked out: her key still works. The request was made with it and not with her
    // browser's session, so that session was one of the others the removal ended.
    assert_eq!(server.get(&mut ann.browser, "/api/me").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&mut Visitor::bearer(&ann.owner_key), "/api/me").await.status, StatusCode::OK);
    // A sign-in under way when the passkey goes does not get through.
    let mut bo_key = bo.key.clone();
    let (id, options) = server.login_start().await;
    let credential = bo_key.sign_in(&options);
    server.delete(&mut bo.browser, &format!("/api/passkeys/{bo_passkey}")).await;
    assert_plain_401(&server.login_finish(&mut Visitor::default(), &id, credential).await, "removed mid-ceremony");
}

// A9
#[tokio::test]
async fn removing_a_passkey_ends_every_other_session_and_stream_of_that_person_and_not_the_one_that_asked() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let mut bo = server.sign_up("Bo").await;
    let phone = server.add_passkey(&mut ann.browser, "Phone").await;
    let (room, _) = server.hangout(&mut ann.browser, "Crew").await;
    let ids = |list: &Reply| list.body["passkeys"].as_array().unwrap().iter().map(|k| text(&k["id"])).collect::<Vec<_>>();
    let ann_passkeys = ids(&server.get(&mut ann.browser, "/api/passkeys").await);
    let bo_passkey = ids(&server.get(&mut bo.browser, "/api/passkeys").await).remove(0);

    // Her tablet signs in with the phone's passkey and her laptop with the owner key. This browser
    // and the tablet have the room open.
    let (mut tablet, signed_in) = server.sign_in(&mut phone.clone()).await;
    assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.body);
    let mut laptop = Visitor::default();
    let by_key = server.post(&mut laptop, "/api/session", json!({ "owner_key": ann.owner_key })).await;
    assert_eq!(by_key.status, StatusCode::OK, "{}", by_key.body);
    let mut here = server.stream(&ann.browser.cookie, &room).await;
    let mut there = server.stream(&tablet.cookie, &room).await;

    // A removal that finds nothing of hers ends nothing.
    let not_hers = server.delete(&mut ann.browser, &format!("/api/passkeys/{bo_passkey}")).await;
    assert_eq!(not_hers.status, StatusCode::NOT_FOUND);
    assert_eq!(server.get(&mut tablet, "/api/me").await.status, StatusCode::OK);
    assert_eq!(server.get(&mut laptop, "/api/me").await.status, StatusCode::OK);

    // Removing the phone's passkey signs out the tablet and the laptop, and Bo is no concern of it.
    let removed = server.delete(&mut ann.browser, &format!("/api/passkeys/{}", ann_passkeys[1])).await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.body);
    assert_eq!(server.get(&mut ann.browser, "/api/me").await.status, StatusCode::OK, "this browser carries on");
    assert_eq!(server.get(&mut tablet, "/api/me").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&mut laptop, "/api/me").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(server.get(&mut bo.browser, "/api/me").await.status, StatusCode::OK);
    assert_eq!(server.state.db.session_count(&ann.person_id).unwrap(), 1);
    // What was removed is gone and what wasn't still signs in. The owner key was never a session.
    assert_plain_401(&server.sign_in(&mut phone.clone()).await.1, "the removed passkey");
    assert_eq!(server.get(&mut Visitor::bearer(&ann.owner_key), "/api/me").await.status, StatusCode::OK);

    // The tablet's stream ends at the next event, and this browser's keeps running.
    server.post(&mut ann.browser, &format!("/api/hangouts/{room}/messages"), json!({ "text": "still here" })).await;
    let mut heard = String::new();
    let ended = async {
        while let Some(chunk) = there.chunk().await.unwrap() {
            heard.push_str(&String::from_utf8_lossy(&chunk));
        }
    };
    tokio::time::timeout(Duration::from_secs(5), ended).await.expect("the other device's stream should close");
    assert!(!heard.contains("still here"), "{heard}");
    let mut own = String::new();
    while !own.contains("still here") {
        let chunk = tokio::time::timeout(Duration::from_secs(5), here.chunk()).await.unwrap().unwrap();
        own.push_str(&String::from_utf8_lossy(&chunk.expect("this browser's stream stays open")));
    }

    // Asked for with the owner key, which is no session, nothing is kept: every browser is signed out.
    let key_removes = server.delete(&mut Visitor::bearer(&ann.owner_key), &format!("/api/passkeys/{}", ann_passkeys[0])).await;
    assert_eq!(key_removes.status, StatusCode::OK, "{}", key_removes.body);
    assert_eq!(server.get(&mut ann.browser, "/api/me").await.status, StatusCode::UNAUTHORIZED);
    let back = server.post(&mut Visitor::default(), "/api/session", json!({ "owner_key": ann.owner_key })).await;
    assert_eq!(back.status, StatusCode::OK, "the owner key still gets her back in");
    assert_eq!(server.get(&mut bo.browser, "/api/me").await.status, StatusCode::OK);
}

// A9
#[tokio::test]
async fn adding_a_passkey_pings_the_person_and_not_their_bots_and_a_sign_up_or_sign_in_pings_no_one() {
    let server = Server::start().await;
    let (bo_url, bo_pings) = start_receiver().await;
    let (ann_url, ann_pings) = start_receiver().await;
    let (scout_url, scout_pings) = start_receiver().await;

    // Bo is listening already when Ann makes her account, which pings no one: Ann has nowhere to
    // be pinged yet, and nothing about her is Bo's business.
    let mut bo = server.sign_up("Bo").await;
    server.set_notify(&mut bo.browser, "/api/notify", &bo_url).await;
    let mut ann = server.sign_up("Ann").await;
    server.set_notify(&mut ann.browser, "/api/notify", &ann_url).await;
    let (room, _) = server.hangout(&mut ann.browser, "Crew").await;
    let made = server.post(&mut ann.browser, &format!("/api/hangouts/{room}/agents"), json!({ "name": "Scout" })).await;
    let scout_id = text(&made.body["agent"]["id"]);
    server.set_notify(&mut ann.browser, &format!("/api/agents/{scout_id}/notify"), &scout_url).await;
    assert!(arrived(&bo_pings).await.is_empty() && arrived(&ann_pings).await.is_empty());

    // Signing in is not adding one.
    assert_eq!(server.sign_in(&mut ann.key).await.1.status, StatusCode::OK);
    assert!(arrived(&ann_pings).await.is_empty());

    // Adding one pings Ann once, with the same sentence whatever it's called.
    let _phone = server.add_passkey(&mut ann.browser, "SECRET-LABEL-4471").await;
    let pings = arrived(&ann_pings).await;
    assert_eq!(pings.len(), 1, "{pings:?}");
    let ping = &pings[0];
    assert_eq!(ping["event"], "account.passkey_added");
    assert!(text(&ping["text"]).starts_with("A passkey was added to your account."), "{ping}");
    assert_eq!(ping["link"], format!("{}/", server.origin), "the home page, where passkeys are kept");
    assert!(ping["hangout_id"].is_null(), "it isn't about a room: {ping}");
    for secret in ["SECRET-LABEL-4471", "Ann", ann.owner_key.as_str(), ann.person_id.as_str()] {
        assert!(!ping.to_string().contains(secret), "{secret} in {ping}");
    }
    // Her bot and Bo are not told.
    assert!(arrived(&scout_pings).await.is_empty(), "a bot is not pinged");
    assert!(arrived(&bo_pings).await.is_empty());

    // An attempt that fails, a removal and a rejected duplicate ping no one more.
    let (id, options) = server.register_start(&mut ann.browser, json!({ "label": "Twin" })).await;
    let twin = server.register_finish(&mut ann.browser, &id, ann.key.clone().register(&options)).await;
    assert_eq!(twin.status, StatusCode::BAD_REQUEST, "{}", twin.body);
    let first = text(&server.get(&mut ann.browser, "/api/passkeys").await.body["passkeys"][0]["id"]);
    assert_eq!(server.delete(&mut ann.browser, &format!("/api/passkeys/{first}")).await.status, StatusCode::OK);
    assert_eq!(arrived(&ann_pings).await.len(), 1);

    // Bo adding one is Bo's ping alone.
    server.add_passkey(&mut bo.browser, "Phone").await;
    assert_eq!(arrived(&bo_pings).await.len(), 1);
    assert_eq!(arrived(&ann_pings).await.len(), 1);
    assert!(arrived(&scout_pings).await.is_empty());
}

// A10
#[tokio::test]
async fn the_house_bot_and_a_bot_in_a_cookie_get_403_on_every_passkey_route_and_leave_no_trace() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let city = server.post(&mut ann.browser, "/api/cities", json!({ "name": "Elm" })).await;
    let city_id = text(&city.body["city"]["id"]);
    let house = server.post(&mut ann.browser, &format!("/api/cities/{city_id}/houses"), json!({ "name": "Ann House" })).await;
    let house_id = text(&house.body["house"]["id"]);
    let casa = server.post(&mut ann.browser, &format!("/api/houses/{house_id}/house-bot"), json!({ "name": "Casa" })).await;
    let casa_token = text(&casa.body["token"]);
    let people = server.state.db.person_count().unwrap();

    let routes: Vec<(Method, String, Option<Value>)> = vec![
        (Method::GET, "/api/passkeys/available".into(), None),
        (Method::GET, "/api/passkeys".into(), None),
        (Method::POST, "/api/passkeys/register/start".into(), Some(json!({ "your_name": "Casa 2" }))),
        (Method::POST, "/api/passkeys/register/finish".into(), Some(json!({ "ceremony_id": "x", "credential": {} }))),
        (Method::POST, "/api/passkeys/login/start".into(), Some(json!({}))),
        (Method::POST, "/api/passkeys/login/finish".into(), Some(json!({ "ceremony_id": "x", "credential": {} }))),
        (Method::DELETE, "/api/passkeys/anything".into(), None),
    ];
    for visitor in [Visitor::bearer(&casa_token), Visitor::browser_with(&casa_token)] {
        for (method, path, body) in &routes {
            let mut visitor = visitor.clone();
            let reply = server.send(&mut visitor, method.clone(), path, body.clone()).await;
            assert_eq!(reply.status, StatusCode::FORBIDDEN, "{method} {path}: {}", reply.body);
            assert!(reply.set_cookie.is_none());
        }
    }
    assert_eq!(server.state.ceremonies.open_count(), 0);
    assert_eq!(server.state.db.person_count().unwrap(), people);
}

// A11
#[tokio::test]
async fn abandoned_and_failed_sign_ups_leave_no_account_and_do_not_use_up_the_hours_cap() {
    let server = Server::start().await;
    // Two hundred people start and walk away.
    for n in 0..200 {
        server.register_start(&mut Visitor::default(), json!({ "your_name": format!("Ghost {n}") })).await;
    }
    assert_eq!(server.state.db.person_count().unwrap(), 0);
    assert_eq!(server.state.ceremonies.open_count(), 200);
    // Failures at the last step make nothing either: bad attestation, wrong site, no user verification.
    for failure in 0..3 {
        let mut browser = Visitor::default();
        let mut key = server.authenticator();
        let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Eve" })).await;
        let mut credential = key.register(&options);
        match failure {
            0 => tamper(&mut credential, "attestationObject"),
            1 => key.origin = "https://evil.example".into(),
            _ => key.user_verified = false,
        }
        let credential = if failure == 0 { credential } else { key.register(&options) };
        let done = server.register_finish(&mut browser, &id, credential).await;
        assert!(done.status.is_client_error(), "{}", done.body);
        assert!(done.set_cookie.is_none() && done.body.get("owner_key").is_none());
    }
    assert_eq!(server.state.db.person_count().unwrap(), 0);
    // Many failures in the hour, and the whole hour's sign-ups are still there to use.
    for _ in 0..(passkey::SIGNUPS_PER_HOUR + 5) {
        let mut browser = Visitor::default();
        let mut key = server.authenticator();
        let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Eve" })).await;
        let mut credential = key.register(&options);
        tamper(&mut credential, "clientDataJSON");
        assert_eq!(server.register_finish(&mut browser, &id, credential).await.status, StatusCode::BAD_REQUEST);
    }
    for n in 0..passkey::SIGNUPS_PER_HOUR {
        server.sign_up(&format!("Real {n}")).await;
    }
    assert_eq!(server.state.db.person_count().unwrap(), passkey::SIGNUPS_PER_HOUR as i64);
}

// A11
#[tokio::test]
async fn a_passkey_refused_as_a_duplicate_does_not_use_up_one_of_the_hours_new_accounts() {
    let server = Server::start().await;
    let mut first = None;
    for n in 0..passkey::SIGNUPS_PER_HOUR - 3 {
        let account = server.sign_up(&format!("Person {n}")).await;
        first.get_or_insert(account);
    }
    let ann = first.unwrap();
    // Five tries to register Ann's own passkey as a brand-new account. Each is refused as a duplicate.
    for n in 0..5 {
        let mut again = ann.key.clone();
        let mut browser = Visitor::default();
        let (id, options) = server.register_start(&mut browser, json!({ "your_name": format!("Copy {n}") })).await;
        let done = server.register_finish(&mut browser, &id, again.register(&options)).await;
        assert_eq!(done.status, StatusCode::CONFLICT, "try {n}: {}", done.body);
    }
    // No account was made, so the three still due are still there to make.
    assert_eq!(server.state.db.person_count().unwrap(), (passkey::SIGNUPS_PER_HOUR - 3) as i64);
    for n in 0..3 {
        server.sign_up(&format!("Late {n}")).await;
    }
    assert_eq!(server.state.db.person_count().unwrap(), passkey::SIGNUPS_PER_HOUR as i64);
}

// A11
// The hour's cap is on passkey sign-ups. Every other way to make an account needs the server's code
// or someone's invite, so a stranger has no second door to make accounts by the hundred.
#[tokio::test]
async fn the_hourly_cap_holds_for_passkey_sign_ups_and_the_other_routes_refuse_without_a_code_or_an_invite() {
    let server = Server::start_with_code("letmein").await;
    for n in 0..passkey::SIGNUPS_PER_HOUR {
        server.sign_up(&format!("Person {n}")).await;
    }
    let people = passkey::SIGNUPS_PER_HOUR as i64;
    assert_eq!(server.state.db.person_count().unwrap(), people);
    let sixty_first = server
        .post(&mut Visitor::default(), "/api/passkeys/register/start", json!({ "your_name": "One too many" }))
        .await;
    assert_eq!(sixty_first.status, StatusCode::TOO_MANY_REQUESTS, "{}", sixty_first.body);

    // A stranger with just a name gets no account from a hangout or a city without the code, or from a
    // link that isn't a real invite.
    for (path, body) in [
        ("/api/hangouts".to_string(), json!({ "name": "Home", "your_name": "Eve" })),
        ("/api/hangouts".to_string(), json!({ "name": "Home", "your_name": "Eve", "create_code": "guess" })),
        ("/api/cities".to_string(), json!({ "name": "Elm", "your_name": "Eve" })),
        ("/api/invites/nonsense/join".to_string(), json!({ "your_name": "Eve" })),
        ("/api/city-invites/nonsense/join".to_string(), json!({ "your_name": "Eve" })),
        ("/api/house-invites/nonsense/join".to_string(), json!({ "your_name": "Eve" })),
    ] {
        let refused = server.post(&mut Visitor::default(), &path, body).await;
        assert!(refused.status.is_client_error(), "{path}: {}", refused.show());
        assert!(refused.set_cookie.is_none() && refused.body.get("owner_key").is_none(), "{path}: {}", refused.body);
    }
    assert_eq!(server.state.db.person_count().unwrap(), people, "no account was made");

    // The code is what the host hands out, and with it the route works.
    let invited = server
        .post(&mut Visitor::default(), "/api/hangouts", json!({ "name": "Home", "your_name": "Eve", "create_code": "letmein" }))
        .await;
    assert_eq!(invited.status, StatusCode::OK, "{}", invited.body);
    assert_eq!(server.state.db.person_count().unwrap(), people + 1);
}

// A12
#[tokio::test]
async fn the_owner_key_is_shown_once_at_sign_up_and_is_the_way_back_in_after_every_passkey_is_gone() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let mut other = server.sign_up("Bo").await;
    assert_ne!(ann.owner_key, other.owner_key);
    assert!(ann.owner_key.starts_with("ah_owner_") && ann.owner_key.len() == "ah_owner_".len() + 40);

    // Never in a later reply: not in me, not in the list, not in a sign-in, not when adding a passkey.
    let phone = server.add_passkey(&mut ann.browser, "Phone").await;
    let mut shown = Vec::new();
    shown.push(server.get(&mut ann.browser, "/api/me").await.body.to_string());
    shown.push(server.get(&mut ann.browser, "/api/passkeys").await.body.to_string());
    shown.push(server.sign_in(&mut ann.key).await.1.body.to_string());
    let (id, options) = server.register_start(&mut ann.browser, json!({})).await;
    let mut third = server.authenticator();
    let added = server.register_finish(&mut ann.browser, &id, third.register(&options)).await;
    assert!(added.body["owner_key"].is_null(), "{}", added.body);
    shown.push(added.body.to_string());
    // Finishing the very same sign-up again says nothing.
    let again = server.register_finish(&mut ann.browser, &id, third.register(&options)).await;
    shown.push(again.body.to_string());
    for page in shown {
        assert!(!page.contains(&ann.owner_key) && !page.contains("ah_owner_"), "{page}");
    }
    // Only a hash is kept: the key itself is not a row, its hash is.
    assert!(server.state.db.person_by_key(&ann.owner_key).unwrap().is_none());
    assert_eq!(server.state.db.person_by_key(&auth::hash_secret(&ann.owner_key)).unwrap().unwrap().id, ann.person_id);

    // Lose every passkey and every session: the key is still the way back.
    let list = server.get(&mut ann.browser, "/api/passkeys").await;
    for passkey in list.body["passkeys"].as_array().unwrap() {
        let removed = server.delete(&mut ann.browser, &format!("/api/passkeys/{}", text(&passkey["id"]))).await;
        assert_eq!(removed.status, StatusCode::OK);
    }
    assert_plain_401(&server.sign_in(&mut ann.key).await.1, "first passkey");
    assert_plain_401(&server.sign_in(&mut phone.clone()).await.1, "second passkey");
    let mut back = Visitor::default();
    let by_key = server.post(&mut back, "/api/session", json!({ "owner_key": ann.owner_key })).await;
    assert_eq!(by_key.status, StatusCode::OK, "{}", by_key.body);
    assert_eq!(server.get(&mut back, "/api/me").await.body["person_id"], ann.person_id.as_str());
    // And a new passkey can be added from there, so the account is whole again.
    let fresh = server.add_passkey(&mut back, "New").await;
    assert_eq!(server.sign_in(&mut fresh.clone()).await.1.status, StatusCode::OK);
    let _ = &mut other;
}

// A13
#[tokio::test]
async fn a_passkey_proves_nothing_about_who_someone_is_and_gives_no_standing() {
    let server = Server::start().await;
    // Two accounts with the very same name, and with one person's name in someone else's.
    let first = server.sign_up("Ann").await;
    let second = server.sign_up("Ann").await;
    assert_ne!(first.person_id, second.person_id);
    assert_eq!(server.state.db.person_count().unwrap(), 2);
    let (key, _) = server.key_person("Ann").await;
    let mut as_key = Visitor::bearer(&key);
    assert_eq!(server.get(&mut as_key, "/api/me").await.body["name"], "Ann");

    // What a passkey account is, on the page, is a name and a count of passkeys: no badge.
    let mut browser = first.browser.clone();
    let me = server.get(&mut browser, "/api/me").await.body;
    for word in ["verified", "trusted", "trust", "standing", "level", "score", "identity", "badge"] {
        assert!(me.as_object().unwrap().keys().all(|key| !key.contains(word)), "{word} in {me}");
    }
    // A passkey account in a city is a newcomer like any other: no days, no gatherings, no trips.
    let mut ann_browser = first.browser.clone();
    let city = server.post(&mut ann_browser, "/api/cities", json!({ "name": "Elm" })).await;
    let city_id = text(&city.body["city"]["id"]);
    let code = code_of(&city.body["invite_url"]);
    let mut joiner = second.browser.clone();
    assert_eq!(server.post(&mut joiner, &format!("/api/city-invites/{code}/join"), json!({})).await.status, StatusCode::OK);
    let standing = server.get(&mut ann_browser, &format!("/api/cities/{city_id}/people/{}/standing", second.person_id)).await;
    assert_eq!(standing.status, StatusCode::OK);
    for count in ["member_days", "gatherings", "good_trips", "sent_home"] {
        assert_eq!(standing.body[count], 0, "{count}: {}", standing.body);
    }
    assert_eq!(standing.body["vouches"], json!([]));
    // And the same person can be two accounts: one vouches for the other, as the doc says they can.
    // Their name is on the vouch, and that is all there is to check.
    assert_eq!(
        server.post(&mut ann_browser, &format!("/api/cities/{city_id}/people/{}/vouch", second.person_id), json!({})).await.status,
        StatusCode::OK
    );
    let standing = server.get(&mut ann_browser, &format!("/api/cities/{city_id}/people/{}/standing", second.person_id)).await;
    assert_eq!(standing.body["vouches"], json!(["Ann"]));
}

// ----------------------------------------------------------------------------------------------
// Abuse: sizes, shapes, broken credentials and an authenticator that bends the rules.
// ----------------------------------------------------------------------------------------------

/// The status of a raw request, with the body and content type exactly as given. Never panics on
/// a refusal: a server that closes the connection is reported as 0.
async fn bytes_to(server: &Server, method: Method, path: &str, who: &Visitor, content_type: Option<&str>, body: Vec<u8>) -> u16 {
    let mut request = server.client.request(method, format!("{}{path}", server.base)).body(body);
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    if !who.bearer.is_empty() {
        request = request.bearer_auth(&who.bearer);
    }
    if !who.cookie.is_empty() {
        request = request.header(header::COOKIE, format!("ah_session={}", who.cookie));
    }
    match request.send().await {
        Ok(response) => response.status().as_u16(),
        Err(_) => 0,
    }
}

async fn alive(server: &Server) {
    let up = server.get(&mut Visitor::default(), "/api/passkeys/available").await;
    assert_eq!(up.status, StatusCode::OK, "the server stopped answering");
}

// A4 A5
#[tokio::test]
async fn huge_deep_and_malformed_bodies_on_every_passkey_and_session_route_never_panic_or_500() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let bot = {
        let (room, _) = server.hangout(&mut ann.browser, "Crew").await;
        let made = server.post(&mut ann.browser, &format!("/api/hangouts/{room}/agents"), json!({ "name": "B" })).await;
        text(&made.body["token"])
    };
    let routes = [
        (Method::POST, "/api/passkeys/register/start"),
        (Method::POST, "/api/passkeys/register/finish"),
        (Method::POST, "/api/passkeys/login/start"),
        (Method::POST, "/api/passkeys/login/finish"),
        (Method::POST, "/api/session"),
        (Method::DELETE, "/api/passkeys/anything"),
    ];
    let deep_array = format!("{}{}", "[".repeat(60_000), "]".repeat(60_000));
    let deep_object = format!("{}1{}", "{\"a\":".repeat(10_000), "}".repeat(10_000));
    let long_string = format!("{{\"ceremony_id\":\"{}\",\"credential\":{{}}}}", "x".repeat(60_000));
    let long_credential = format!("{{\"ceremony_id\":\"x\",\"credential\":{{\"id\":\"{}\"}}}}", "A".repeat(60_000));
    let bodies: Vec<(&str, Option<&str>, Vec<u8>)> = vec![
        ("empty", Some("application/json"), vec![]),
        ("no content type", None, b"{\"your_name\":\"x\"}".to_vec()),
        ("text", Some("text/plain"), b"{\"your_name\":\"x\"}".to_vec()),
        ("form", Some("application/x-www-form-urlencoded"), b"your_name=x".to_vec()),
        ("null", Some("application/json"), b"null".to_vec()),
        ("a number", Some("application/json"), b"12345678901234567890123".to_vec()),
        ("an array", Some("application/json"), b"[1,2,3]".to_vec()),
        ("a string", Some("application/json"), b"\"your_name\"".to_vec()),
        ("not json", Some("application/json"), b"{not json".to_vec()),
        ("bad utf-8", Some("application/json"), vec![b'{', b'"', 0xff, 0xfe, b'"', b':', b'1', b'}']),
        ("nul bytes", Some("application/json"), b"{\"your_name\":\"a\\u0000b\"}".to_vec()),
        ("deep arrays", Some("application/json"), deep_array.into_bytes()),
        ("deep objects", Some("application/json"), deep_object.into_bytes()),
        ("a long id", Some("application/json"), long_string.into_bytes()),
        ("a long credential", Some("application/json"), long_credential.into_bytes()),
        ("just over the limit", Some("application/json"), vec![b' '; 70_000]),
        ("far over the limit", Some("application/json"), vec![b' '; 5_000_000]),
    ];
    let visitors = [Visitor::default(), Visitor::bearer(&bot), Visitor::bearer(&ann.owner_key), Visitor::browser_with(&ann.browser.cookie)];
    for (method, path) in &routes {
        for who in &visitors {
            for (what, content_type, body) in &bodies {
                let status = bytes_to(&server, method.clone(), path, who, *content_type, body.clone()).await;
                // The server may answer a body over its 64 KB limit and close before the client has sent it
                // all, which is a connection that is closed, not a reply. That is any body over the limit,
                // such as the 120 KB of deep arrays, and not only the two named for it.
                let closed_early = status == 0 && body.len() > 64 * 1024;
                assert!(closed_early || (status != 0 && status < 500), "{method} {path} with {what} as {:?}: {status}", (!who.bearer.is_empty(), !who.cookie.is_empty()));
            }
        }
        alive(&server).await;
    }
    assert_eq!(server.passkey_count(&mut ann.browser).await, 1);
}

// A1 A2 A11
#[tokio::test]
async fn odd_names_and_labels_are_made_or_refused_and_never_a_500() {
    let server = Server::start().await;
    let names: Vec<String> = [
        "Ann", "  Ann  ", "A", "tab\there", "new\nline", "nul\u{0}byte", "\u{202e}rtl", "<script>alert(1)</script>",
        "Robert'); DROP TABLE people;--", "a\u{200b}b", "\u{feff}bom", "   ", "", "\u{0301}combining",
    ]
    .iter()
    .map(|name| name.to_string())
    .chain(["x".repeat(60), "x".repeat(61), "é".repeat(60), "é".repeat(61), "🙂".repeat(60), "🙂".repeat(61)])
    .collect();
    for name in names.iter() {
        let mut browser = Visitor::default();
        let mut key = server.authenticator();
        let started = server.post(&mut browser, "/api/passkeys/register/start", json!({ "your_name": name })).await;
        assert!(started.status == StatusCode::OK || started.status == StatusCode::BAD_REQUEST, "{name:?}: {}", started.show());
        if started.status == StatusCode::OK {
            let done = server.register_finish(&mut browser, &text(&started.body["ceremony_id"]), key.register(&started.body["options"])).await;
            assert_eq!(done.status, StatusCode::OK, "{name:?}: {}", done.body);
            assert_eq!(done.body["person"]["name"], name.trim(), "kept as typed, trimmed");
        }
    }
    let limit = server.sign_up("Limits").await;
    let mut browser = limit.browser.clone();
    let good_labels: Vec<String> =
        vec![String::new(), "x".into(), "x".repeat(40), "é".repeat(40), "🙂".repeat(40), "<b>".into(), "\u{0}".into()];
    for label in &good_labels {
        let (id, options) = server.register_start(&mut browser, json!({ "label": label })).await;
        let mut key = server.authenticator();
        let done = server.register_finish(&mut browser, &id, key.register(&options)).await;
        assert_eq!(done.status, StatusCode::OK, "{label:?}: {}", done.body);
    }
    let bad_labels: Vec<String> = vec!["x".repeat(41), "🙂".repeat(41), "x".repeat(5000)];
    for label in &bad_labels {
        let refused = server.post(&mut browser, "/api/passkeys/register/start", json!({ "label": label })).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{} chars: {}", label.chars().count(), refused.body);
    }
    for bad in [json!(5), json!(true), json!(["a"]), json!({ "a": 1 }), json!(1e300)] {
        let refused = server.post(&mut browser, "/api/passkeys/register/start", json!({ "label": bad.clone() })).await;
        assert!(refused.status.is_client_error(), "{bad}: {}", refused.show());
        let refused = server.post(&mut Visitor::default(), "/api/passkeys/register/start", json!({ "your_name": bad.clone() })).await;
        assert!(refused.status.is_client_error(), "{bad}: {}", refused.show());
    }
    alive(&server).await;
}

fn broken_values() -> Vec<Value> {
    vec![
        Value::Null, json!(""), json!("AA"), json!("!!!not base64!!!"), json!("A".repeat(50_000)), json!(5), json!(-1), json!(1.5),
        json!(true), json!([]), json!([1, 2]), json!({}), json!({ "a": { "b": 1 } }), json!("AAAA=="), json!("-_-_"),
    ]
}

// A5
#[tokio::test]
async fn a_sign_in_with_any_part_of_the_answer_broken_is_the_same_plain_401() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let places: [&[&str]; 9] = [
        &["id"], &["rawId"], &["type"], &["response"], &["response", "authenticatorData"], &["response", "clientDataJSON"],
        &["response", "signature"], &["response", "userHandle"], &["clientExtensionResults"],
    ];
    let mut tried = 0;
    for place in places {
        for broken in broken_values() {
            for remove in [false, true] {
                let (id, options) = server.login_start().await;
                let mut credential = ann.key.sign_in(&options);
                let parent = if place.len() == 2 { &mut credential["response"] } else { &mut credential };
                let last = place[place.len() - 1];
                if remove {
                    parent.as_object_mut().unwrap().remove(last);
                } else {
                    parent[last] = broken.clone();
                }
                let reply = server.login_finish(&mut Visitor::default(), &id, credential).await;
                let short = broken.to_string().chars().take(30).collect::<String>();
                // `clientExtensionResults` and a few others may be ignored without harm, in which
                // case the very same answer is simply good: nothing else may let anyone in.
                if reply.status == StatusCode::OK {
                    let harmless = matches!(place[0], "clientExtensionResults" | "type" | "id")
                        || (place == ["response", "userHandle"] && (remove || broken.is_null()));
                    assert!(harmless, "{place:?} = {short} (removed: {remove}) let someone in");
                } else {
                    assert_plain_401(&reply, &format!("{place:?} = {short} (removed: {remove})"));
                }
                tried += 1;
            }
        }
    }
    assert!(tried > 250);
    alive(&server).await;
    // A broken try uses its ceremony up and nothing more: Ann can still sign in properly.
    ann.key.counter += 10;
    assert_eq!(server.sign_in(&mut ann.key).await.1.status, StatusCode::OK);
}

// A2 A11
#[tokio::test]
async fn a_registration_with_any_part_of_the_answer_broken_is_refused_and_makes_no_account() {
    let server = Server::start().await;
    let places: [&[&str]; 7] = [
        &["id"], &["rawId"], &["type"], &["response"], &["response", "attestationObject"], &["response", "clientDataJSON"],
        &["clientExtensionResults"],
    ];
    let mut tried = 0;
    for place in places {
        for broken in broken_values() {
            for remove in [false, true] {
                let mut browser = Visitor::default();
                let mut key = server.authenticator();
                let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Eve" })).await;
                let mut credential = key.register(&options);
                let parent = if place.len() == 2 { &mut credential["response"] } else { &mut credential };
                let last = place[place.len() - 1];
                if remove {
                    parent.as_object_mut().unwrap().remove(last);
                } else {
                    parent[last] = broken.clone();
                }
                let done = server.register_finish(&mut browser, &id, credential).await;
                let short = broken.to_string().chars().take(30).collect::<String>();
                assert!(done.status.is_client_error() || done.status == StatusCode::OK, "{place:?} = {short}: {}", done.show());
                if done.status == StatusCode::OK {
                    // Only fields the server doesn't read may be broken without harm.
                    assert!(matches!(place[0], "clientExtensionResults" | "type" | "id" | "rawId"), "{place:?} = {short} (removed: {remove})");
                }
                tried += 1;
            }
        }
    }
    assert!(tried > 190);
    alive(&server).await;
    let made = server.state.db.person_count().unwrap();
    assert!(made <= tried, "{made}");
    // Whatever was made, nobody is made twice from one ceremony and the cap was not blown by failures.
    assert!(server.state.ceremonies.signups_full(now()) == (made >= passkey::SIGNUPS_PER_HOUR as i64));
}

/// An authenticator that can bend the rules the support one keeps: odd flags, a cross-origin frame,
/// the wrong kind of ceremony. Built from the same pieces.
struct Rogue {
    key: p256::ecdsa::SigningKey,
    credential_id: Vec<u8>,
    handle: Vec<u8>,
    origin: String,
    rp_id: String,
}

struct Bend {
    flags: u8,
    kind: &'static str,
    cross_origin: bool,
    counter: u32,
}

impl Rogue {
    fn new(origin: &str) -> Self {
        let key = loop {
            if let Ok(key) = p256::ecdsa::SigningKey::from_slice(&rand::random::<[u8; 32]>()) {
                break key;
            }
        };
        let host = origin.split("://").nth(1).unwrap().split([':', '/']).next().unwrap().to_string();
        Self { key, credential_id: rand::random::<[u8; 32]>().to_vec(), handle: vec![], origin: origin.into(), rp_id: host }
    }

    fn client_data(&self, kind: &str, challenge: &str, cross_origin: bool) -> Vec<u8> {
        json!({ "type": kind, "challenge": challenge, "origin": self.origin, "crossOrigin": cross_origin }).to_string().into_bytes()
    }

    fn rp_hash(&self) -> Vec<u8> {
        use sha2::{Digest, Sha256};
        Sha256::digest(self.rp_id.as_bytes()).to_vec()
    }

    fn register(&mut self, options: &Value, bend: &Bend) -> Value {
        use ciborium::value::Value as Cbor;
        let public_key = &options["publicKey"];
        let challenge = public_key["challenge"].as_str().unwrap();
        self.handle = support::unb64(public_key["user"]["id"].as_str().unwrap());
        let point = self.key.verifying_key().to_sec1_point(false);
        let cose = Cbor::Map(vec![
            (Cbor::Integer(1.into()), Cbor::Integer(2.into())),
            (Cbor::Integer(3.into()), Cbor::Integer((-7).into())),
            (Cbor::Integer((-1).into()), Cbor::Integer(1.into())),
            (Cbor::Integer((-2).into()), Cbor::Bytes(point.x().unwrap().to_vec())),
            (Cbor::Integer((-3).into()), Cbor::Bytes(point.y().unwrap().to_vec())),
        ]);
        let mut auth_data = self.rp_hash();
        auth_data.push(bend.flags);
        auth_data.extend(bend.counter.to_be_bytes());
        auth_data.extend([0u8; 16]);
        auth_data.extend((self.credential_id.len() as u16).to_be_bytes());
        auth_data.extend(&self.credential_id);
        ciborium::into_writer(&cose, &mut auth_data).unwrap();
        let attestation = Cbor::Map(vec![
            (Cbor::Text("fmt".into()), Cbor::Text("none".into())),
            (Cbor::Text("attStmt".into()), Cbor::Map(vec![])),
            (Cbor::Text("authData".into()), Cbor::Bytes(auth_data)),
        ]);
        let mut object = Vec::new();
        ciborium::into_writer(&attestation, &mut object).unwrap();
        json!({
            "id": b64(&self.credential_id), "rawId": b64(&self.credential_id), "type": "public-key",
            "response": {
                "attestationObject": b64(&object),
                "clientDataJSON": b64(&self.client_data(bend.kind, challenge, bend.cross_origin)),
            },
            "clientExtensionResults": {},
        })
    }

    fn assert(&self, options: &Value, bend: &Bend) -> Value {
        use p256::ecdsa::{signature::Signer, Signature};
        use sha2::{Digest, Sha256};
        let challenge = options["publicKey"]["challenge"].as_str().unwrap();
        let client_data = self.client_data(bend.kind, challenge, bend.cross_origin);
        let mut auth_data = self.rp_hash();
        auth_data.push(bend.flags);
        auth_data.extend(bend.counter.to_be_bytes());
        let mut signed = auth_data.clone();
        signed.extend(Sha256::digest(&client_data));
        let signature: Signature = self.key.sign(&signed);
        json!({
            "id": b64(&self.credential_id), "rawId": b64(&self.credential_id), "type": "public-key",
            "response": {
                "authenticatorData": b64(&auth_data), "clientDataJSON": b64(&client_data),
                "signature": b64(signature.to_der().as_bytes()), "userHandle": b64(&self.handle),
            },
            "clientExtensionResults": {},
        })
    }
}

const UP: u8 = 0x01;
const UV: u8 = 0x04;
const AT: u8 = 0x40;

// A2 A3
#[tokio::test]
async fn an_authenticator_that_bends_the_rules_of_a_ceremony_gets_nowhere() {
    let server = Server::start().await;
    let good_make = Bend { flags: UP | UV | AT, kind: "webauthn.create", cross_origin: false, counter: 0 };
    let good_get = |counter: u32| Bend { flags: UP | UV, kind: "webauthn.get", cross_origin: false, counter };

    // Registering, bent in each way: no user present, no user verified, the wrong kind of ceremony,
    // a page in a cross-origin frame, no attested data. None makes an account.
    let bends = [
        ("no user present", Bend { flags: UV | AT, ..good_make_clone(&good_make) }),
        ("no user verification", Bend { flags: UP | AT, ..good_make_clone(&good_make) }),
        ("no attested data", Bend { flags: UP | UV, ..good_make_clone(&good_make) }),
        ("a sign-in's kind", Bend { kind: "webauthn.get", ..good_make_clone(&good_make) }),
        ("an invented kind", Bend { kind: "webauthn.nothing", ..good_make_clone(&good_make) }),
        ("a cross-origin frame", Bend { cross_origin: true, ..good_make_clone(&good_make) }),
    ];
    for (what, bend) in bends {
        let mut browser = Visitor::default();
        let mut rogue = Rogue::new(&server.origin);
        let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Eve" })).await;
        let done = server.register_finish(&mut browser, &id, rogue.register(&options, &bend)).await;
        assert_eq!(done.status, StatusCode::BAD_REQUEST, "{what}: {}", done.body);
    }
    assert_eq!(server.state.db.person_count().unwrap(), 0);

    // The control: every flag right makes the account, and the same key signs in.
    let mut browser = Visitor::default();
    let mut rogue = Rogue::new(&server.origin);
    let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Eve" })).await;
    let done = server.register_finish(&mut browser, &id, rogue.register(&options, &good_make)).await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.body);
    let sign = |bend: Bend| {
        let server = &server;
        let rogue = &rogue;
        async move {
            let (id, options) = server.login_start().await;
            server.login_finish(&mut Visitor::default(), &id, rogue.assert(&options, &bend)).await
        }
    };
    assert_eq!(sign(good_get(1)).await.status, StatusCode::OK, "the control");

    // Signing in, bent each way. Every one is the same plain 401.
    for (what, bend) in [
        ("no user present", Bend { flags: UV, ..good_get(2) }),
        ("no user verification", Bend { flags: UP, ..good_get(3) }),
        ("a registration's kind", Bend { kind: "webauthn.create", ..good_get(4) }),
        ("an invented kind", Bend { kind: "webauthn.nothing", ..good_get(5) }),
        ("attested data on a sign-in", Bend { flags: UP | UV | AT, ..good_get(6) }),
    ] {
        assert_plain_401(&sign(bend).await, what);
    }
    assert_eq!(sign(good_get(7)).await.status, StatusCode::OK, "none of that wedged the passkey");
}

// A3
// The library refuses a cross-origin frame when a passkey is made but not when one signs in, so the
// server looks for it itself. The page's frame-ancestors header is the second line of defence.
#[tokio::test]
async fn a_sign_in_from_a_cross_origin_frame_is_refused_as_a_sign_up_from_one_is() {
    let server = Server::start().await;
    let make = Bend { flags: UP | UV | AT, kind: "webauthn.create", cross_origin: false, counter: 0 };
    let mut browser = Visitor::default();
    let mut rogue = Rogue::new(&server.origin);
    let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Eve" })).await;
    assert_eq!(server.register_finish(&mut browser, &id, rogue.register(&options, &make)).await.status, StatusCode::OK);
    let (id, options) = server.login_start().await;
    let framed = Bend { flags: UP | UV, kind: "webauthn.get", cross_origin: true, counter: 1 };
    let reply = server.login_finish(&mut Visitor::default(), &id, rogue.assert(&options, &framed)).await;
    assert_plain_401(&reply, "a cross-origin frame");
    // The same passkey outside a frame signs in, so the frame was all that was wrong.
    let (id, options) = server.login_start().await;
    let plain = Bend { flags: UP | UV, kind: "webauthn.get", cross_origin: false, counter: 1 };
    let reply = server.login_finish(&mut Visitor::default(), &id, rogue.assert(&options, &plain)).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    // The page does forbid being framed, which is what makes this a matter of depth and not a hole.
    let page = server.client.get(format!("{}/", server.base)).send().await.unwrap();
    let csp = page.headers().get("content-security-policy").unwrap().to_str().unwrap().to_string();
    assert!(csp.contains("frame-ancestors 'none'") && page.headers().get("x-frame-options").unwrap() == "DENY");
}

fn good_make_clone(bend: &Bend) -> Bend {
    Bend { flags: bend.flags, kind: bend.kind, cross_origin: bend.cross_origin, counter: bend.counter }
}

// A2 A5 A11
#[tokio::test]
async fn crafted_attestations_with_hostile_structure_are_refused_without_a_crash() {
    use ciborium::value::Value as Cbor;
    let server = Server::start().await;
    let rp_hash = {
        use sha2::{Digest, Sha256};
        Sha256::digest(b"localhost").to_vec()
    };
    let header = |flags: u8| {
        let mut data = rp_hash.clone();
        data.push(flags);
        data.extend(0u32.to_be_bytes());
        data
    };
    let attested = |flags: u8, id_len: u16, id: &[u8], key: Vec<u8>| {
        let mut data = header(flags);
        data.extend([0u8; 16]);
        data.extend(id_len.to_be_bytes());
        data.extend(id);
        data.extend(key);
        data
    };
    let cbor = |value: &Cbor| {
        let mut out = Vec::new();
        ciborium::into_writer(value, &mut out).unwrap();
        out
    };
    let object = |fmt: &str, statement: Cbor, auth_data: Vec<u8>| {
        cbor(&Cbor::Map(vec![
            (Cbor::Text("fmt".into()), Cbor::Text(fmt.into())),
            (Cbor::Text("attStmt".into()), statement),
            (Cbor::Text("authData".into()), Cbor::Bytes(auth_data)),
        ]))
    };
    let empty = || Cbor::Map(vec![]);
    let flags = UP | UV | AT;
    let nested = (0..300).fold(Cbor::Null, |inner, _| Cbor::Array(vec![inner]));
    let objects: Vec<(&str, Vec<u8>)> = vec![
        ("nothing", vec![]),
        ("not cbor", vec![0xff, 0xff, 0xff, 0xff]),
        ("a cbor number", cbor(&Cbor::Integer(5.into()))),
        ("a cbor array", cbor(&Cbor::Array(vec![]))),
        ("no authData", cbor(&Cbor::Map(vec![(Cbor::Text("fmt".into()), Cbor::Text("none".into()))]))),
        ("empty authData", object("none", empty(), vec![])),
        ("authData of 36 bytes", object("none", empty(), vec![0; 36])),
        ("authData of 37 bytes with the attested flag", object("none", empty(), header(flags))),
        ("attested data with 55 bytes only", object("none", empty(), { let mut d = header(flags); d.extend([0u8; 18]); d })),
        ("a credential id longer than the data", object("none", empty(), attested(flags, 65535, &[1, 2, 3], vec![0xa0]))),
        ("a zero length credential id", object("none", empty(), attested(flags, 0, &[], cbor(&empty())))),
        ("a key that is not a map", object("none", empty(), attested(flags, 2, &[1, 2], cbor(&Cbor::Integer(1.into()))))),
        ("a key with no parts", object("none", empty(), attested(flags, 2, &[1, 2], cbor(&empty())))),
        ("a key of an unsupported kind", object("none", empty(), attested(flags, 2, &[1, 2], cbor(&Cbor::Map(vec![(Cbor::Integer(1.into()), Cbor::Integer(3.into())), (Cbor::Integer(3.into()), Cbor::Integer((-257).into()))]))))),
        ("a key with huge coordinates", object("none", empty(), attested(flags, 2, &[1, 2], cbor(&Cbor::Map(vec![(Cbor::Integer(1.into()), Cbor::Integer(2.into())), (Cbor::Integer(3.into()), Cbor::Integer((-7).into())), (Cbor::Integer((-1).into()), Cbor::Integer(1.into())), (Cbor::Integer((-2).into()), Cbor::Bytes(vec![0xff; 5000])), (Cbor::Integer((-3).into()), Cbor::Bytes(vec![0xff; 5000]))])))) ),
        ("a key with a point that is not on the curve", object("none", empty(), attested(flags, 2, &[1, 2], cbor(&Cbor::Map(vec![(Cbor::Integer(1.into()), Cbor::Integer(2.into())), (Cbor::Integer(3.into()), Cbor::Integer((-7).into())), (Cbor::Integer((-1).into()), Cbor::Integer(1.into())), (Cbor::Integer((-2).into()), Cbor::Bytes(vec![1; 32])), (Cbor::Integer((-3).into()), Cbor::Bytes(vec![2; 32]))]))))),
        ("a packed statement with nothing in it", object("packed", empty(), attested(flags, 2, &[1, 2], cbor(&empty())))),
        ("a tpm statement with nothing in it", object("tpm", empty(), attested(flags, 2, &[1, 2], cbor(&empty())))),
        ("an android-key statement with nothing in it", object("android-key", empty(), attested(flags, 2, &[1, 2], cbor(&empty())))),
        ("an apple statement with nothing in it", object("apple", empty(), attested(flags, 2, &[1, 2], cbor(&empty())))),
        ("a fido-u2f statement with nothing in it", object("fido-u2f", empty(), attested(flags, 2, &[1, 2], cbor(&empty())))),
        ("an unknown format", object("hologram", empty(), attested(flags, 2, &[1, 2], cbor(&empty())))),
        ("a format that is not text", cbor(&Cbor::Map(vec![(Cbor::Text("fmt".into()), Cbor::Integer(1.into())), (Cbor::Text("attStmt".into()), empty()), (Cbor::Text("authData".into()), Cbor::Bytes(header(flags)))]))),
        ("a statement nested three hundred deep", object("none", nested, header(flags))),
        ("forty thousand bytes of padding", { let mut o = object("none", empty(), header(flags)); o.extend(vec![0u8; 40_000]); o }),
    ];
    let mut tried = 0;
    for (what, attestation) in objects {
        let mut browser = Visitor::default();
        let (id, options) = server.register_start(&mut browser, json!({ "your_name": "Eve" })).await;
        let challenge = options["publicKey"]["challenge"].as_str().unwrap();
        let client_data = json!({ "type": "webauthn.create", "challenge": challenge, "origin": server.origin, "crossOrigin": false }).to_string();
        let credential = json!({
            "id": "AQID", "rawId": "AQID", "type": "public-key",
            "response": { "attestationObject": b64(&attestation), "clientDataJSON": b64(client_data.as_bytes()) },
            "clientExtensionResults": {},
        });
        let done = server.register_finish(&mut browser, &id, credential).await;
        assert!(done.status == StatusCode::BAD_REQUEST || done.status == StatusCode::CONFLICT, "{what}: {}", done.show());
        tried += 1;
    }
    assert!(tried > 20);
    assert_eq!(server.state.db.person_count().unwrap(), 0, "no account came of any of it");
    alive(&server).await;
}

// A9 (Limits)
#[tokio::test]
async fn a_person_keeps_at_most_twenty_passkeys_whether_the_last_is_added_at_a_time_or_in_a_race() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    for n in 1..passkey::MAX_PASSKEYS {
        server.add_passkey(&mut ann.browser, &format!("Key {n}")).await;
    }
    assert_eq!(server.passkey_count(&mut ann.browser).await, passkey::MAX_PASSKEYS);
    // The twenty-first isn't started.
    let refused = server.post(&mut ann.browser, "/api/passkeys/register/start", json!({ "label": "One too many" })).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    // Remove one and a start works; two started at nineteen cannot both finish.
    let list = server.get(&mut ann.browser, "/api/passkeys").await;
    let first = text(&list.body["passkeys"][0]["id"]);
    assert_eq!(server.delete(&mut ann.browser, &format!("/api/passkeys/{first}")).await.status, StatusCode::OK);
    let (id_a, options_a) = server.register_start(&mut ann.browser, json!({ "label": "A" })).await;
    let (id_b, options_b) = server.register_start(&mut ann.browser, json!({ "label": "B" })).await;
    let (mut key_a, mut key_b) = (server.authenticator(), server.authenticator());
    let credential_a = key_a.register(&options_a);
    let credential_b = key_b.register(&options_b);
    assert_eq!(server.register_finish(&mut ann.browser, &id_a, credential_a).await.status, StatusCode::OK);
    let second = server.register_finish(&mut ann.browser, &id_b, credential_b).await;
    assert_eq!(second.status, StatusCode::CONFLICT, "{}", second.body);
    assert_eq!(server.passkey_count(&mut ann.browser).await, passkey::MAX_PASSKEYS);
    // The one that didn't make it can't sign in.
    assert_plain_401(&server.sign_in(&mut key_b).await.1, "the twenty-first");
}

// A7 A8
#[tokio::test]
async fn signing_out_or_replacing_the_key_ends_a_city_stream_at_its_next_event() {
    let server = Server::start().await;
    let mut ann = server.sign_up("Ann").await;
    let city = server.post(&mut ann.browser, "/api/cities", json!({ "name": "Elm" })).await;
    let city_id = text(&city.body["city"]["id"]);
    let code = code_of(&city.body["invite_url"]);
    let joined = server.post(&mut Visitor::default(), &format!("/api/city-invites/{code}/join"), json!({ "your_name": "Bo" })).await;
    let mut bo = Visitor::bearer(&text(&joined.body["owner_key"]));

    let open = |token: String| {
        let (client, base, city_id) = (server.client.clone(), server.base.clone(), city_id.clone());
        async move {
            client
                .get(format!("{base}/api/cities/{city_id}/stream"))
                .header(header::COOKIE, format!("ah_session={token}"))
                .send()
                .await
                .unwrap()
        }
    };
    // Two streams of Ann's: one on her session, one on a second device's.
    let (mut tablet, _) = server.sign_in(&mut ann.key).await;
    let mut here = open(ann.browser.cookie.clone()).await;
    let mut there = open(tablet.cookie.clone()).await;
    assert_eq!((here.status(), there.status()), (StatusCode::OK, StatusCode::OK));
    let house = server.post(&mut bo, &format!("/api/cities/{city_id}/houses"), json!({ "name": "Bo House" })).await;
    assert_eq!(house.status, StatusCode::OK, "{}", house.body);
    for stream in [&mut here, &mut there] {
        let heard = tokio::time::timeout(Duration::from_secs(5), async {
            let mut heard = String::new();
            while !heard.contains("refresh") {
                heard.push_str(&String::from_utf8_lossy(&stream.chunk().await.unwrap().unwrap()));
            }
            heard
        })
        .await
        .expect("a refresh");
        assert!(heard.contains("refresh"));
    }
    // The tablet signs out; the next change closes its stream and leaves the other one running.
    server.delete(&mut tablet, "/api/session").await;
    let new_member = server.post(&mut Visitor::default(), &format!("/api/city-invites/{code}/join"), json!({ "your_name": "Cy" })).await;
    assert_eq!(new_member.status, StatusCode::OK, "{}", new_member.body);
    let ended = async { while let Some(_chunk) = there.chunk().await.unwrap() {} };
    tokio::time::timeout(Duration::from_secs(5), ended).await.expect("the signed-out device's stream should close");
    let still = tokio::time::timeout(Duration::from_secs(5), here.chunk()).await.unwrap().unwrap();
    assert!(still.is_some(), "Ann's own stream is untouched");
    // Replacing the key ends the rest, her own too, at the next event.
    let rotated = server.post(&mut Visitor::bearer(&ann.owner_key), "/api/key/rotate", json!({})).await;
    assert_eq!(rotated.status, StatusCode::OK);
    let cy = server.post(&mut Visitor::default(), &format!("/api/city-invites/{code}/join"), json!({ "your_name": "Dee" })).await;
    assert_eq!(cy.status, StatusCode::OK, "{}", cy.body);
    let ended = async { while let Some(_chunk) = here.chunk().await.unwrap() {} };
    tokio::time::timeout(Duration::from_secs(5), ended).await.expect("the old session's stream should close");
}

// A7
#[tokio::test]
async fn odd_cookies_and_authorization_headers_are_401s_and_never_a_crash() {
    let server = Server::start().await;
    let ann = server.sign_up("Ann").await;
    let good = ann.browser.cookie.clone();
    let cookies: Vec<(&str, Vec<u8>)> = vec![
        ("empty value", b"ah_session=".to_vec()),
        ("no value", b"ah_session".to_vec()),
        ("only junk", b";;;===;;".to_vec()),
        ("a huge value", format!("ah_session={}", "A".repeat(7_000)).into_bytes()),
        ("high bytes", vec![b'a', b'h', b'_', b's', b'e', b's', b's', b'i', b'o', b'n', b'=', 0xe9, 0xff, 0x80]),
        ("a quoted token", format!("ah_session=\"{good}\"").into_bytes()),
        ("a token with a space", format!("ah_session= {good} ").into_bytes()),
        ("a token with a suffix", format!("ah_session={good}x").into_bytes()),
        ("a token with a prefix", format!("ah_session=x{good}").into_bytes()),
        ("lower case", format!("AH_SESSION={good}").into_bytes()),
        ("another cookie's name", format!("session={good}").into_bytes()),
    ];
    for (what, cookie) in cookies {
        let value = header::HeaderValue::from_bytes(&cookie).unwrap();
        let response = server.client.get(format!("{}/api/me", server.base)).header(header::COOKIE, value).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{what}");
    }
    // Two cookies of the same name: either way it is one of them and nothing breaks.
    for both in [format!("ah_session={good}; ah_session=junk"), format!("ah_session=junk; ah_session={good}")] {
        let response = server.client.get(format!("{}/api/me", server.base)).header(header::COOKIE, both).send().await.unwrap();
        assert!(response.status() == StatusCode::OK || response.status() == StatusCode::UNAUTHORIZED);
    }
    // The cookie is read from a header with other cookies in it, and the Authorization header
    // that is not a bearer one is ignored in favour of it.
    let mixed = format!("theme=dark; ah_session={good}; other=1");
    let response = server.client.get(format!("{}/api/me", server.base)).header(header::COOKIE, mixed).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let basic = server.client.get(format!("{}/api/me", server.base)).header(header::AUTHORIZATION, "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==").header(header::COOKIE, format!("ah_session={good}")).send().await.unwrap();
    assert_eq!(basic.status(), StatusCode::OK);
    for authorization in ["Bearer", "Bearer ", "bearer x", "Bearer  ", "Token abc", "Bearer ah_sess_", &format!("Bearer {}", "A".repeat(9_000))] {
        let response = server.client.get(format!("{}/api/me", server.base)).header(header::AUTHORIZATION, authorization).send().await.unwrap();
        assert!(response.status().is_client_error(), "{authorization:?}: {}", response.status());
    }
    alive(&server).await;
}
