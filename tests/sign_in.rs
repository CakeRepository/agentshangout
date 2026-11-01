//! A27 to A39: signing in with Discord or GitHub, against stand-ins for both that answer as the real
//! ones do and write down everything they were sent. Almost everything here runs for each site.

mod support;

use std::sync::{Arc, Mutex};

use agenthangout::{
    app,
    db::Db,
    discord, now, passkey,
    sign_in::{self, GitHubApp, Site},
    AppState, Apps, Config,
};
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode as AxumStatus},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use reqwest::{redirect, Client, Method, StatusCode, Url};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const CLIENT_ID: &str = "1234567890";
const CLIENT_SECRET: &str = "the-client-secret";
/// Base64 of `CLIENT_ID:CLIENT_SECRET`, which is how Discord's token route is told who's asking.
const BASIC: &str = "Basic MTIzNDU2Nzg5MDp0aGUtY2xpZW50LXNlY3JldA==";

#[derive(Default)]
struct Seen {
    /// The form each code was exchanged with, with the header that came along.
    exchanged: Vec<(String, String)>,
    /// The User-Agent each read of an account came with, which GitHub insists on.
    agents: Vec<String>,
    /// The tokens someone asked to have revoked.
    revoked: Vec<String>,
    /// What notifications arrived at the address a test gave.
    pings: Vec<String>,
}

type Sites = Arc<Mutex<Seen>>;

fn field(form: &str, name: &str) -> Option<String> {
    Url::parse(&format!("http://form/?{form}"))
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

fn header_of(headers: &HeaderMap, name: header::HeaderName) -> String {
    headers.get(name).and_then(|value| value.to_str().ok()).unwrap_or_default().to_string()
}

/// A code is the name of an account here. `refused` is one the site doesn't know. GitHub says so with
/// a 200, as the real one does.
async fn github_token(State(seen): State<Sites>, form: String) -> Json<Value> {
    seen.lock().unwrap().exchanged.push((String::new(), form.clone()));
    let code = field(&form, "code").unwrap_or_default();
    if code == "refused" {
        return Json(json!({ "error": "bad_verification_code" }));
    }
    Json(
        json!({ "access_token": format!("token-for-{code}"), "token_type": "bearer", "scope": "" }),
    )
}

async fn discord_token(
    State(seen): State<Sites>,
    headers: HeaderMap,
    form: String,
) -> axum::response::Response {
    let basic = header_of(&headers, header::AUTHORIZATION);
    seen.lock().unwrap().exchanged.push((basic, form.clone()));
    let code = field(&form, "code").unwrap_or_default();
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

/// Both sites know the same people, so a test reads the same for either.
fn account(bearer: &str) -> Option<(i64, &'static str)> {
    match bearer.trim_start_matches("Bearer token-for-") {
        "ann" => Some((1001, "ann-codes")),
        "ann-renamed" => Some((1001, "ann-renamed")),
        "bo" => Some((2002, "bo-codes")),
        "taken-name" => Some((3003, "ann-codes")),
        "broken" => Some((0, "nobody")),
        _ => None,
    }
}

async fn github_user(State(seen): State<Sites>, headers: HeaderMap) -> axum::response::Response {
    seen.lock().unwrap().agents.push(header_of(&headers, header::USER_AGENT));
    let Some((id, login)) = account(&header_of(&headers, header::AUTHORIZATION)) else {
        return AxumStatus::UNAUTHORIZED.into_response();
    };
    Json(json!({ "id": id, "login": login, "email": "never-kept@example.com" })).into_response()
}

async fn discord_user(headers: HeaderMap) -> axum::response::Response {
    let Some((id, username)) = account(&header_of(&headers, header::AUTHORIZATION)) else {
        return AxumStatus::UNAUTHORIZED.into_response();
    };
    let id = if id == 0 { "../../etc".to_string() } else { id.to_string() };
    Json(json!({ "id": id, "username": username, "global_name": "Display Name" })).into_response()
}

async fn revoke(State(seen): State<Sites>, form: String) -> Json<Value> {
    seen.lock().unwrap().revoked.push(form);
    Json(json!({}))
}

async fn hook(State(seen): State<Sites>, body: String) -> Json<Value> {
    seen.lock().unwrap().pings.push(body);
    Json(json!({}))
}

async fn start_sites() -> (String, Sites) {
    let seen = Sites::default();
    let routes = Router::new()
        .route("/login/oauth/access_token", post(github_token))
        .route("/user", get(github_user))
        .route("/oauth2/token", post(discord_token))
        .route("/oauth2/token/revoke", post(revoke))
        .route("/users/@me", get(discord_user))
        .route("/hook", post(hook))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, routes).await });
    (url, seen)
}

struct Server {
    base: String,
    origin: String,
    client: Client,
    state: AppState,
    sites: Sites,
    sites_url: String,
}

struct Who {
    key: String,
    id: String,
}

#[derive(Debug)]
struct Reply {
    status: StatusCode,
    body: Value,
    /// Every cookie this reply set, as `name=value`.
    cookies: Vec<String>,
    location: Option<String>,
}

impl Reply {
    fn ok(self) -> Value {
        assert_eq!(self.status, StatusCode::OK, "{}", self.body);
        self.body
    }

    fn refused(self, status: StatusCode) {
        assert_eq!(self.status, status, "{}", self.body);
    }

    /// The value of a cookie this reply set.
    fn cookie(&self, name: &str) -> Option<String> {
        self.cookies.iter().find_map(|pair| {
            pair.strip_prefix(name).and_then(|rest| rest.strip_prefix('=')).map(str::to_string)
        })
    }

    /// The `state` in the address this reply redirects to.
    fn step(&self) -> String {
        assert_eq!(self.status, StatusCode::SEE_OTHER, "{}", self.body);
        step_of(self.location.as_ref().expect("no redirect"))
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_else(|| panic!("not text: {value}")).to_string()
}

fn who(body: &Value) -> Who {
    Who { key: text(&body["owner_key"]), id: text(&body["person"]["id"]) }
}

fn step_of(url: &str) -> String {
    field(url.split_once('?').expect("no query").1, "state").expect("no state")
}

fn sign_in_path(site: Site) -> String {
    format!("/api/sign-in/{}", site.key())
}

fn link_path(site: Site) -> String {
    format!("/api/sign-in/{}/link", site.key())
}

impl Server {
    async fn start() -> Self {
        Self::with(true).await
    }

    /// A server with both sites turned on, or with no app set up for either.
    async fn with(on: bool) -> Self {
        let (sites_url, sites) = start_sites().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let origin = format!("http://localhost:{port}");
        let state = AppState::with_apps(
            Db::open(":memory:").unwrap(),
            Config {
                public_url: origin.clone(),
                create_code: None,
                allow_private_webhooks: true,
                keeper_id: None,
                god_ids: Vec::new(),
            },
            Apps {
                discord: on.then(|| discord::App {
                    client_id: CLIENT_ID.to_string(),
                    client_secret: CLIENT_SECRET.to_string(),
                    authorize_url: format!("{sites_url}/oauth2/authorize"),
                    api_url: sites_url.clone(),
                }),
                github: on.then(|| GitHubApp {
                    client_id: CLIENT_ID.to_string(),
                    client_secret: CLIENT_SECRET.to_string(),
                    authorize_url: format!("{sites_url}/login/oauth/authorize"),
                    token_url: format!("{sites_url}/login/oauth/access_token"),
                    api_url: sites_url.clone(),
                }),
            },
        );
        let served = state.clone();
        tokio::spawn(async move { axum::serve(listener, app(served)).await });
        let base = format!("http://127.0.0.1:{port}");
        let client = Client::builder().redirect(redirect::Policy::none()).build().unwrap();
        Self { base, origin, client, state, sites, sites_url }
    }

    async fn send(
        &self,
        token: &str,
        cookie: Option<&str>,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let mut request = self.client.request(method, format!("{}{path}", self.base));
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok()?.split(';').next().map(str::to_string))
            .collect();
        let location = response
            .headers()
            .get(header::LOCATION)
            .map(|value| value.to_str().unwrap().to_string());
        let text = response.text().await.unwrap();
        let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
        Reply { status, body, cookies, location }
    }

    async fn get(&self, token: &str, path: &str) -> Reply {
        self.send(token, None, Method::GET, path, None).await
    }

    async fn post(&self, token: &str, path: &str, body: Value) -> Reply {
        self.send(token, None, Method::POST, path, Some(body)).await
    }

    async fn delete(&self, token: &str, path: &str) -> Reply {
        self.send(token, None, Method::DELETE, path, None).await
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

    /// A bot's id and its token.
    async fn bot(&self, owner: &Who, room: &str, name: &str) -> (String, String) {
        let made = self
            .post(&owner.key, &format!("/api/hangouts/{room}/agents"), json!({ "name": name }))
            .await
            .ok();
        (text(&made["agent"]["id"]), text(&made["token"]))
    }

    async fn finish(&self, token: &str, cookie: Option<&str>, step: &str, code: &str) -> Reply {
        let body = json!({ "code": code, "state": step });
        self.send(token, cookie, Method::POST, "/api/sign-in/finish", Some(body)).await
    }

    /// The whole of linking: start, say yes on the site, come back.
    async fn link(&self, person: &Who, site: Site, code: &str) {
        let step = self.get(&person.key, &link_path(site)).await.step();
        self.finish(&person.key, None, &step, code).await.ok();
    }

    /// The whole of a sign-in, in a browser that keeps its cookie.
    async fn sign_in(&self, site: Site, code: &str) -> Reply {
        let started = self.get("", &sign_in_path(site)).await;
        let browser = started.cookie(sign_in::STEP_COOKIE).expect("no step cookie");
        let cookie = format!("{}={browser}", sign_in::STEP_COOKIE);
        self.finish("", Some(&cookie), &started.step(), code).await
    }

    /// A sign-in that works, and the session token, as the cookie would hold it.
    async fn session(&self, site: Site, code: &str) -> String {
        let signed_in = self.sign_in(site, code).await;
        assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.body);
        signed_in.cookie("ah_session").expect("no session cookie")
    }

    /// A session started some other way, here with the owner key.
    async fn other_session(&self, person: &Who) -> String {
        self.post("", "/api/session", json!({ "owner_key": person.key }))
            .await
            .cookie("ah_session")
            .expect("no session cookie")
    }

    /// The name this person's account on this site was last seen with.
    fn linked_as(&self, person: &Who, site: Site) -> Option<String> {
        let links = self.state.db.sign_in_links(&person.id).unwrap();
        links.into_iter().find(|(key, _)| key == site.key()).map(|(_, name)| name)
    }

    fn exchanges(&self) -> usize {
        self.sites.lock().unwrap().exchanged.len()
    }

    fn pinged(&self) -> Vec<String> {
        self.sites.lock().unwrap().pings.clone()
    }
}

// A27

#[tokio::test]
async fn a_person_links_an_account_and_can_then_sign_in_with_it() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        server.link(&ann, site, "ann").await;
        let me = server.get(&ann.key, "/api/me").await.ok();
        let row = me["sign_in"].as_array().unwrap().iter().find(|row| row["site"] == site.key());
        assert_eq!(row.unwrap()["linked"], "ann-codes", "{site:?}");

        let session = server.session(site, "ann").await;
        let me = server.get(&session, "/api/me").await.ok();
        assert_eq!(me["person_id"], ann.id.as_str(), "{site:?}");
    }
}

// A27
#[tokio::test]
async fn a_person_can_have_one_of_each() {
    let server = Server::start().await;
    let (_, ann, _) = server.someone("Ann").await;
    server.link(&ann, Site::Discord, "ann").await;
    server.link(&ann, Site::GitHub, "bo").await;
    assert_eq!(
        server.get(&server.session(Site::Discord, "ann").await, "/api/me").await.ok()["name"],
        "Ann"
    );
    assert_eq!(
        server.get(&server.session(Site::GitHub, "bo").await, "/api/me").await.ok()["name"],
        "Ann"
    );
}

// A27
#[tokio::test]
async fn an_account_on_one_site_says_nothing_about_the_same_id_on_the_other() {
    let server = Server::start().await;
    let (_, ann, _) = server.someone("Ann").await;
    let (_, bo, _) = server.someone("Bo").await;
    server.link(&ann, Site::Discord, "ann").await;
    server.link(&bo, Site::GitHub, "ann").await;
    assert_eq!(
        server.get(&server.session(Site::Discord, "ann").await, "/api/me").await.ok()["name"],
        "Ann"
    );
    assert_eq!(
        server.get(&server.session(Site::GitHub, "ann").await, "/api/me").await.ok()["name"],
        "Bo"
    );
}

// A27
#[tokio::test]
async fn an_account_links_to_one_person() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        let (_, bo, _) = server.someone("Bo").await;
        server.link(&ann, site, "ann").await;
        let step = server.get(&bo.key, &link_path(site)).await.step();
        server.finish(&bo.key, None, &step, "ann").await.refused(StatusCode::CONFLICT);
        assert_eq!(server.linked_as(&bo, site), None);
        // And Ann still has it.
        assert_eq!(server.linked_as(&ann, site).as_deref(), Some("ann-codes"));
        assert_eq!(
            server.get(&server.session(site, "ann").await, "/api/me").await.ok()["name"],
            "Ann"
        );
    }
}

// A27
#[tokio::test]
async fn linking_a_different_account_replaces_the_old_one_and_ends_what_it_started() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        server.link(&ann, site, "ann").await;
        let through_the_old_one = server.session(site, "ann").await;
        let other = server.other_session(&ann).await;
        server.link(&ann, site, "bo").await;
        server.get(&through_the_old_one, "/api/me").await.refused(StatusCode::UNAUTHORIZED);
        server.get(&other, "/api/me").await.ok();
        assert_eq!(server.linked_as(&ann, site).as_deref(), Some("bo-codes"));
        server.sign_in(site, "ann").await.refused(StatusCode::UNAUTHORIZED);
        assert_eq!(
            server.get(&server.session(site, "bo").await, "/api/me").await.ok()["name"],
            "Ann"
        );
    }
}

// A27
#[tokio::test]
async fn a_bot_gets_403_on_every_route_here() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (room, ann, _) = server.someone("Ann").await;
        let (_, token) = server.bot(&ann, &room, "Sequence Sam").await;
        server.get(&token, &link_path(site)).await.refused(StatusCode::FORBIDDEN);
        server.get(&token, &sign_in_path(site)).await.refused(StatusCode::FORBIDDEN);
        server.delete(&token, &sign_in_path(site)).await.refused(StatusCode::FORBIDDEN);
        server.finish(&token, None, "any", "ann").await.refused(StatusCode::FORBIDDEN);
    }
}

// A27
#[tokio::test]
async fn linking_takes_someone_signed_in_and_a_site_nobody_has_heard_of_is_a_404() {
    let server = Server::start().await;
    server.get("", &link_path(Site::GitHub)).await.refused(StatusCode::UNAUTHORIZED);
    server.delete("", &sign_in_path(Site::GitHub)).await.refused(StatusCode::UNAUTHORIZED);
    server.get("", "/api/sign-in/myspace").await.refused(StatusCode::NOT_FOUND);
}

// A28

#[tokio::test]
async fn each_site_is_asked_for_as_little_as_it_allows() {
    let server = Server::start().await;
    let (_, ann, _) = server.someone("Ann").await;
    for (site, scope, consent, returning) in [
        (Site::Discord, Some("identify"), Some("consent"), Some("none")),
        (Site::GitHub, None, None, None),
    ] {
        for (reply, prompt) in [
            (server.get(&ann.key, &link_path(site)).await, consent),
            (server.get("", &sign_in_path(site)).await, returning),
        ] {
            assert_eq!(reply.status, StatusCode::SEE_OTHER);
            let url = Url::parse(reply.location.as_ref().unwrap()).unwrap();
            let asked = |name: &str| {
                url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
            };
            assert_eq!(asked("client_id").as_deref(), Some(CLIENT_ID));
            assert_eq!(asked("scope").as_deref(), scope, "{site:?}");
            assert_eq!(asked("prompt").as_deref(), prompt, "{site:?}");
            assert_eq!(asked("redirect_uri"), Some(format!("{}/sign-in/back", server.origin)));
            assert!(asked("state").is_some_and(|state| state.len() >= 32));
            assert!(!url.as_str().contains(CLIENT_SECRET));
        }
    }
}

// A28
#[tokio::test]
async fn it_goes_by_the_id_because_a_username_can_change_hands() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        server.link(&ann, site, "ann").await;
        // A different account that now has the username Ann had signs in as nobody.
        server.sign_in(site, "taken-name").await.refused(StatusCode::UNAUTHORIZED);
    }
}

// A28
#[tokio::test]
async fn nothing_but_the_id_and_username_is_kept_and_no_token_is_revoked_or_stored() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        server.link(&ann, site, "ann").await;
        server.session(site, "ann").await;
        assert!(server.sites.lock().unwrap().revoked.is_empty(), "so nobody is asked again");
        let me = server.get(&ann.key, "/api/me").await.ok().to_string();
        assert!(!me.contains("token-for"), "{me}");
        assert!(!me.contains("never-kept"), "{me}");
    }
}

// A28
#[tokio::test]
async fn github_is_told_who_is_asking() {
    let server = Server::start().await;
    let (_, ann, _) = server.someone("Ann").await;
    server.link(&ann, Site::GitHub, "ann").await;
    assert_eq!(server.sites.lock().unwrap().agents, ["agentshangout"]);
}

// A28
#[tokio::test]
async fn a_code_the_site_refuses_or_an_account_that_makes_no_sense_links_nothing() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        for code in ["refused", "broken", "nobody"] {
            let step = server.get(&ann.key, &link_path(site)).await.step();
            server.finish(&ann.key, None, &step, code).await.refused(StatusCode::BAD_GATEWAY);
            assert_eq!(server.linked_as(&ann, site), None, "{site:?} {code}");
        }
    }
}

// A29

#[tokio::test]
async fn each_step_proves_itself_with_pkce() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        let started = server.get(&ann.key, &link_path(site)).await;
        let url = Url::parse(started.location.as_ref().unwrap()).unwrap();
        let asked = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
                .unwrap()
        };
        assert_eq!(asked("code_challenge_method"), "S256");
        server.finish(&ann.key, None, &asked("state"), "ann").await.ok();

        let (basic, form) = server.sites.lock().unwrap().exchanged[0].clone();
        let verifier = field(&form, "code_verifier").expect("no verifier");
        assert!(verifier.len() >= 43);
        assert_eq!(support::b64(&Sha256::digest(verifier.as_bytes())), asked("code_challenge"));
        assert_eq!(field(&form, "code").as_deref(), Some("ann"));
        // Discord takes the app's secret in a header, and GitHub in the form.
        match site {
            Site::Discord => assert_eq!(basic, BASIC),
            Site::GitHub => {
                assert_eq!(field(&form, "client_secret").as_deref(), Some(CLIENT_SECRET));
                assert_eq!(field(&form, "client_id").as_deref(), Some(CLIENT_ID));
            }
        }
    }
}

// A29
#[tokio::test]
async fn a_step_works_once_and_lapses() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        let step = server.get(&ann.key, &link_path(site)).await.step();
        server.finish(&ann.key, None, &step, "ann").await.ok();
        server.finish(&ann.key, None, &step, "bo").await.refused(StatusCode::BAD_REQUEST);
        assert_eq!(server.linked_as(&ann, site).as_deref(), Some("ann-codes"));

        let step = server.get(&ann.key, &link_path(site)).await.step();
        let caller = agenthangout::auth::Caller {
            person_id: ann.id.clone(),
            agent_id: None,
            name: "Ann".to_string(),
            secret_hash: String::new(),
            is_house: false,
        };
        let later = now() + passkey::CEREMONY_SECS + 1;
        let Err(lapsed) =
            sign_in::finish(&server.state, Some(&caller), None, &step, "bo", later).await
        else {
            panic!("a lapsed step finished");
        };
        assert_eq!(lapsed.status.as_u16(), 400);
    }
}

// A29
#[tokio::test]
async fn steps_share_the_limit_on_what_one_address_can_keep_open() {
    let server = Server::start().await;
    let from_one_address = || async {
        server
            .client
            .get(format!("{}{}", server.base, sign_in_path(Site::GitHub)))
            .header("x-forwarded-for", "203.0.113.9")
            .send()
            .await
            .unwrap()
            .status()
    };
    for _ in 0..passkey::MAX_OPEN_PER_ADDRESS {
        assert_eq!(from_one_address().await, StatusCode::SEE_OTHER);
    }
    assert_eq!(from_one_address().await, StatusCode::TOO_MANY_REQUESTS);
}

// A29
#[tokio::test]
async fn the_address_a_site_sends_people_to_is_the_page_and_changes_nothing() {
    let server = Server::start().await;
    let (_, ann, _) = server.someone("Ann").await;
    let step = server.get(&ann.key, &link_path(Site::GitHub)).await.step();
    let page = server
        .client
        .get(format!("{}/sign-in/back?code=ann&state={step}", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.text().await.unwrap().contains("/app.js"));
    assert_eq!(server.exchanges(), 0);
    server.finish(&ann.key, None, &step, "ann").await.ok();
}

// A29
#[tokio::test]
async fn a_step_of_any_other_kind_gets_the_same_plain_400() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let unknown = server.finish(&ann.key, None, "no-such-step", "ann").await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
    let passkey = server.post("", "/api/passkeys/login/start", json!({})).await.ok();
    let other = server.finish(&ann.key, None, &text(&passkey["ceremony_id"]), "ann").await;
    assert_eq!(other.status, StatusCode::BAD_REQUEST);
    assert_eq!(other.body, unknown.body, "nothing tells them apart");

    // A step that links Discord to a bot can't be spent here, and can't sign anyone in.
    let started = server.get(&ann.key, &format!("/api/agents/{bot}/discord")).await;
    let bot_step = started.step();
    let spent = server.finish(&ann.key, None, &bot_step, "ann").await;
    assert_eq!(spent.body, unknown.body);
    assert_eq!(server.exchanges(), 0, "the site is never asked");
}

// A30

#[tokio::test]
async fn someone_elses_step_cant_put_their_account_on_yours() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        let (_, mallory, _) = server.someone("Mallory").await;

        // Mallory starts a step and gets Ann to finish it, with Mallory's own account.
        let step = server.get(&mallory.key, &link_path(site)).await.step();
        server.finish(&ann.key, None, &step, "bo").await.refused(StatusCode::BAD_REQUEST);
        assert_eq!(server.linked_as(&ann, site), None);
        assert_eq!(server.exchanges(), 0, "the site is never asked");
        server.sign_in(site, "bo").await.refused(StatusCode::UNAUTHORIZED);

        // Nobody signed in can finish one either, and the step is spent by now.
        let step = server.get(&mallory.key, &link_path(site)).await.step();
        server.finish("", None, &step, "bo").await.refused(StatusCode::BAD_REQUEST);
        server.finish(&mallory.key, None, &step, "bo").await.refused(StatusCode::BAD_REQUEST);
    }
}

// A31

#[tokio::test]
async fn a_sign_in_only_finishes_in_the_browser_that_started_it() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, mallory, _) = server.someone("Mallory").await;
        server.link(&mallory, site, "bo").await;
        let linked = server.exchanges();

        // Mallory's sign-in, followed by Ann's browser, which has no such cookie.
        let step = server.get("", &sign_in_path(site)).await.step();
        let replied = server.finish("", None, &step, "bo").await;
        assert_eq!(replied.status, StatusCode::BAD_REQUEST);
        assert!(replied.cookie("ah_session").is_none());

        // Nor with a cookie from another sign-in.
        let started = server.get("", &sign_in_path(site)).await;
        let other = server.get("", &sign_in_path(site)).await;
        let wrong =
            format!("{}={}", sign_in::STEP_COOKIE, other.cookie(sign_in::STEP_COOKIE).unwrap());
        let replied = server.finish("", Some(&wrong), &started.step(), "bo").await;
        assert_eq!(replied.status, StatusCode::BAD_REQUEST);
        assert!(replied.cookie("ah_session").is_none());
        assert_eq!(server.exchanges(), linked, "the site is never asked");
    }
}

// A31
#[tokio::test]
async fn the_cookie_is_short_lived_and_scripts_cant_read_it() {
    let server = Server::start().await;
    let response = server
        .client
        .get(format!("{}{}", server.base, sign_in_path(Site::GitHub)))
        .send()
        .await
        .unwrap();
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(cookie.starts_with(sign_in::STEP_COOKIE), "{cookie}");
    for part in ["HttpOnly", "SameSite=Strict", "Path=/api/sign-in", "Max-Age=300"] {
        assert!(cookie.contains(part), "{cookie}");
    }
}

// A32

#[tokio::test]
async fn signing_in_with_an_account_nobody_linked_makes_nothing_and_says_so() {
    for site in Site::ALL {
        let server = Server::start().await;
        let replied = server.sign_in(site, "bo").await;
        assert_eq!(replied.status, StatusCode::UNAUTHORIZED);
        assert!(text(&replied.body["error"]).contains("isn't linked"), "{}", replied.body);
        assert!(replied.cookie("ah_session").is_none());
    }
}

// A32
#[tokio::test]
async fn a_discord_link_on_a_bot_signs_nobody_in() {
    let server = Server::start().await;
    let (room, ann, _) = server.someone("Ann").await;
    let (bot, _) = server.bot(&ann, &room, "Sequence Sam").await;
    let step = server.get(&ann.key, &format!("/api/agents/{bot}/discord")).await.step();
    server
        .post(&ann.key, "/api/discord/finish", json!({ "code": "ann", "state": step }))
        .await
        .ok();
    assert!(server.state.db.discord_link(&bot).unwrap().is_some());
    server.sign_in(Site::Discord, "ann").await.refused(StatusCode::UNAUTHORIZED);
    assert_eq!(server.linked_as(&ann, Site::Discord), None);
}

// A33

#[tokio::test]
async fn each_sign_in_brings_a_new_username_here() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        server.link(&ann, site, "ann").await;
        let session = server.session(site, "ann-renamed").await;
        assert_eq!(server.get(&session, "/api/me").await.ok()["person_id"], ann.id.as_str());
        assert_eq!(server.linked_as(&ann, site).as_deref(), Some("ann-renamed"));
    }
}

// A34

#[tokio::test]
async fn linking_pings_the_person_in_case_it_wasnt_them() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        let url = format!("{}/hook", server.sites_url);
        server
            .send(&ann.key, None, Method::PUT, "/api/notify", Some(json!({ "url": url })))
            .await
            .ok();
        server.link(&ann, site, "ann").await;
        for _ in 0..50 {
            if !server.pinged().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let pings = server.pinged();
        assert_eq!(pings.len(), 1, "{pings:?}");
        assert!(pings[0].contains("was linked to your account"), "{pings:?}");
        assert!(pings[0].contains(site.name()), "{pings:?}");
        assert!(!pings[0].contains("ann-codes"), "the same sentence for everyone");
    }
}

// A35

#[tokio::test]
async fn a_session_a_site_started_cant_change_how_the_account_is_signed_in_to() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        server.link(&ann, site, "ann").await;
        let session = server.session(site, "ann").await;

        server.post(&session, "/api/key/rotate", json!({})).await.refused(StatusCode::FORBIDDEN);
        server
            .post(&session, "/api/passkeys/register/start", json!({}))
            .await
            .refused(StatusCode::FORBIDDEN);
        server.delete(&session, "/api/passkeys/any").await.refused(StatusCode::FORBIDDEN);
        for either in Site::ALL {
            server.get(&session, &link_path(either)).await.refused(StatusCode::FORBIDDEN);
        }

        // Everything else is an ordinary session, and the page is told not to offer those.
        let me = server.get(&session, "/api/me").await.ok();
        assert_eq!(
            (me["name"].clone(), me["sign_in_session"].clone()),
            (json!("Ann"), json!(true))
        );
        assert_eq!(server.get(&ann.key, "/api/me").await.ok()["sign_in_session"], false);
    }
}

// A35
#[tokio::test]
async fn any_other_way_in_can_still_do_those() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        server.link(&ann, site, "ann").await;
        let other = server.other_session(&ann).await;
        server.get(&other, &link_path(site)).await.step();
        server.post(&other, "/api/passkeys/register/start", json!({})).await.ok();
        let replaced = server.post(&other, "/api/key/rotate", json!({})).await.ok();
        assert!(replaced["owner_key"].is_string());
        // A8: that ends every session, and the link stays.
        server.get(&other, "/api/me").await.refused(StatusCode::UNAUTHORIZED);
        assert_eq!(
            server.get(&server.session(site, "ann").await, "/api/me").await.ok()["name"],
            "Ann"
        );
    }
}

// A35
#[tokio::test]
async fn signing_in_another_way_on_top_leaves_an_ordinary_session() {
    let server = Server::start().await;
    let (_, ann, _) = server.someone("Ann").await;
    server.link(&ann, Site::GitHub, "ann").await;
    let through_a_site = server.session(Site::GitHub, "ann").await;
    let replied = server
        .send(
            &through_a_site,
            None,
            Method::POST,
            "/api/session",
            Some(json!({ "owner_key": ann.key })),
        )
        .await;
    let session = replied.cookie("ah_session").expect("no session cookie");
    server.get(&session, &link_path(Site::GitHub)).await.step();
    server.post(&session, "/api/passkeys/register/start", json!({})).await.ok();
}

// A36

#[tokio::test]
async fn unlinking_deletes_it_and_ends_only_the_sessions_that_site_started() {
    let server = Server::start().await;
    let (_, ann, _) = server.someone("Ann").await;
    server.link(&ann, Site::GitHub, "ann").await;
    server.link(&ann, Site::Discord, "ann").await;
    let through_github = server.session(Site::GitHub, "ann").await;
    let through_discord = server.session(Site::Discord, "ann").await;
    let other = server.other_session(&ann).await;

    server.delete(&other, &sign_in_path(Site::GitHub)).await.ok();
    assert_eq!(server.linked_as(&ann, Site::GitHub), None);
    assert!(server.linked_as(&ann, Site::Discord).is_some());
    server.get(&through_github, "/api/me").await.refused(StatusCode::UNAUTHORIZED);
    server.get(&through_discord, "/api/me").await.ok();
    server.get(&other, "/api/me").await.ok();
    server.sign_in(Site::GitHub, "ann").await.refused(StatusCode::UNAUTHORIZED);
    // Doing it twice is fine.
    server.delete(&other, &sign_in_path(Site::GitHub)).await.ok();
}

// A36
#[tokio::test]
async fn a_session_a_site_started_can_unlink_and_that_ends_it_too() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (_, ann, _) = server.someone("Ann").await;
        server.link(&ann, site, "ann").await;
        let session = server.session(site, "ann").await;
        server.delete(&session, &sign_in_path(site)).await.ok();
        server.get(&session, "/api/me").await.refused(StatusCode::UNAUTHORIZED);
    }
}

// A36
#[tokio::test]
async fn unlinking_works_with_the_site_turned_off() {
    for site in Site::ALL {
        let server = Server::with(false).await;
        let (_, ann, _) = server.someone("Ann").await;
        assert!(server
            .state
            .db
            .set_sign_in_link(&ann.id, site.key(), "1001", "ann-codes", now())
            .unwrap());
        // It's still listed, or there would be no way to get at it.
        let me = server.get(&ann.key, "/api/me").await.ok();
        assert_eq!(me["sign_in"][0]["linked"], "ann-codes");
        server.delete(&ann.key, &sign_in_path(site)).await.ok();
        assert_eq!(server.linked_as(&ann, site), None);
    }
}

// A37

#[tokio::test]
async fn only_the_person_sees_their_link() {
    for site in Site::ALL {
        let server = Server::start().await;
        let (room, ann, code) = server.someone("Ann").await;
        let bo = who(&server
            .post("", &format!("/api/invites/{code}/join"), json!({ "your_name": "Bo" }))
            .await
            .ok());
        let (_, token) = server.bot(&ann, &room, "Sequence Sam").await;
        server.link(&ann, site, "ann").await;

        for reader in [&bo.key, &ann.key] {
            let snapshot = server.get(reader, &format!("/api/hangouts/{room}")).await.ok();
            assert!(!snapshot.to_string().contains("ann-codes"), "not in a room at all");
        }
        for path in ["/api/me".to_string(), format!("/api/hangouts/{room}")] {
            let read = server.get(&token, &path).await.ok().to_string();
            assert!(!read.contains("ann-codes"), "{path}: {read}");
        }
        let me = server.get(&bo.key, "/api/me").await.ok().to_string();
        assert!(!me.contains("ann-codes"), "{me}");
    }
}

// A38

#[tokio::test]
async fn a_link_gives_no_standing_and_changes_no_name() {
    for site in Site::ALL {
        let server = Server::start().await;
        let made =
            server.post("", "/api/cities", json!({ "name": "Elm", "your_name": "Ann" })).await.ok();
        let ann = who(&made);
        let city = text(&made["city"]["id"]);
        let path = format!("/api/cities/{city}/people/{}/standing", ann.id);
        let before = server.get(&ann.key, &path).await.ok();
        server.link(&ann, site, "ann").await;
        assert_eq!(server.get(&ann.key, &path).await.ok(), before);
        assert_eq!(server.get(&ann.key, "/api/me").await.ok()["name"], "Ann");
    }
}

// A39

#[tokio::test]
async fn with_no_app_set_up_nothing_is_offered() {
    for site in Site::ALL {
        let server = Server::with(false).await;
        let (_, ann, _) = server.someone("Ann").await;
        assert_eq!(server.get("", "/api/server").await.ok()["sign_in_with"], json!([]));
        assert_eq!(server.get(&ann.key, "/api/me").await.ok()["sign_in"], json!([]));
        server.get(&ann.key, &link_path(site)).await.refused(StatusCode::CONFLICT);
        server.get("", &sign_in_path(site)).await.refused(StatusCode::CONFLICT);
    }
}

// A39
#[tokio::test]
async fn with_both_set_up_both_are_offered() {
    let server = Server::start().await;
    let (_, ann, _) = server.someone("Ann").await;
    assert_eq!(
        server.get("", "/api/server").await.ok()["sign_in_with"],
        json!([{ "site": "discord", "name": "Discord" }, { "site": "github", "name": "GitHub" }])
    );
    assert_eq!(server.get(&ann.key, "/api/me").await.ok()["sign_in"].as_array().unwrap().len(), 2);
}

// A39
#[test]
fn it_takes_both_settings_to_turn_github_on() {
    assert!(GitHubApp::from_settings(None, None).is_none());
    assert!(GitHubApp::from_settings(Some("id".into()), None).is_none());
    assert!(GitHubApp::from_settings(None, Some("secret".into())).is_none());
    assert!(GitHubApp::from_settings(Some(String::new()), Some("secret".into())).is_none());
    assert!(GitHubApp::from_settings(Some("id".into()), Some(String::new())).is_none());
    let app = GitHubApp::from_settings(Some("id".into()), Some("secret".into())).unwrap();
    assert_eq!(app.authorize_url, "https://github.com/login/oauth/authorize");
    assert_eq!(app.token_url, "https://github.com/login/oauth/access_token");
    assert_eq!(app.api_url, "https://api.github.com");
}

#[tokio::test]
async fn the_privacy_page_says_what_a_link_keeps() {
    let server = Server::start().await;
    let page = server.get("", "/privacy").await.ok().to_string();
    for words in ["Signing in with Discord or GitHub", "Unlink", "five minutes"] {
        assert!(page.contains(words), "the privacy page should mention {words}");
    }
}
