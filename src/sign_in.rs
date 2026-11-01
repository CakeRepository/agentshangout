//! Signing in with Discord or GitHub: the rules in docs/accounts-and-trust.md (A46 to A64).
//!
//! The site proves an account, a person links it to theirs or makes their account with it, and after
//! that it signs them in. A link is a way into someone's account, so what matters here is who can't
//! end up with one. Handlers in `api` only call these. Linking Discord to a bot is `discord`'s, and
//! signs nobody in.

use std::{net::IpAddr, time::Duration};

use axum::http::header;
use reqwest::{redirect, Client, StatusCode, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    auth::{self, Caller},
    db::Person,
    discord,
    error::{ApiError, ApiResult},
    notify,
    ops::{self, MAX_NAME_CHARS},
    passkey::{self, Pending},
    AppState,
};

/// The cookie that ties a sign-in to the browser that started it (A50).
pub const STEP_COOKIE: &str = "ah_sign_in_step";
const TIMEOUT: Duration = Duration::from_secs(5);
/// GitHub's API refuses a request that doesn't say who's asking.
const USER_AGENT: &str = "agentshangout";
/// Longer than any username GitHub gives out.
const MAX_LOGIN_CHARS: usize = 64;

/// The sites someone can sign in through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Site {
    Discord,
    GitHub,
}

impl Site {
    pub const ALL: [Site; 2] = [Site::Discord, Site::GitHub];

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|site| site.key() == key)
    }

    /// What's stored, and what's in a route.
    pub fn key(self) -> &'static str {
        match self {
            Site::Discord => "discord",
            Site::GitHub => "github",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Site::Discord => "Discord",
            Site::GitHub => "GitHub",
        }
    }
}

/// The OAuth app registered with GitHub that people say yes to. Discord's is `discord::App`.
pub struct GitHubApp {
    pub client_id: String,
    pub client_secret: String,
    pub authorize_url: String,
    pub token_url: String,
    pub api_url: String,
}

impl GitHubApp {
    /// The app these settings name. It takes both: without them GitHub is off (A58).
    pub fn from_settings(client_id: Option<String>, client_secret: Option<String>) -> Option<Self> {
        Some(Self {
            client_id: client_id.filter(|id| !id.is_empty())?,
            client_secret: client_secret.filter(|secret| !secret.is_empty())?,
            authorize_url: "https://github.com/login/oauth/authorize".to_string(),
            token_url: "https://github.com/login/oauth/access_token".to_string(),
            api_url: "https://api.github.com".to_string(),
        })
    }
}

/// An account as a site reported it.
struct Account {
    id: String,
    name: String,
}

/// What's read from GitHub. Everything else it sends is ignored, so it can't be kept by accident.
#[derive(Deserialize)]
struct GitHubUser {
    id: i64,
    login: String,
}

/// GitHub answers a bad code with a 200 and an error in the body, so a code that didn't work is one
/// this can't be read from.
#[derive(Deserialize)]
struct GitHubToken {
    access_token: String,
}

/// How a step ended, so the route knows whether there's a session to start.
pub enum Finished {
    Linked,
    SignedIn(Person, Site),
    /// A64: they were in already, and stay as they are.
    AlreadyIn,
    /// A51: an account nobody has linked, and what to offer whoever proved it.
    Offered(Value),
}

// Errors

fn off(site: Site) -> ApiError {
    ApiError::conflict(format!("{} sign-in isn't set up on this server", site.name()))
}

fn busy() -> ApiError {
    ApiError::too_many_requests("too many sign-ins are open right now; try again in a few minutes")
}

/// A49, A50: the same for a step that lapsed, was used, belongs elsewhere or isn't a step.
fn lapsed() -> ApiError {
    ApiError::bad_request("that sign-in step has lapsed or was already used; start again")
}

fn unconfirmed(site: Site) -> ApiError {
    ApiError::new(
        StatusCode::BAD_GATEWAY,
        format!("{} didn't confirm an account; start again", site.name()),
    )
}

fn signed_in() -> ApiError {
    ApiError::conflict(
        "you're signed in: link that account from your home page, or sign out to make a new one",
    )
}

fn someone_else() -> ApiError {
    ApiError::conflict("you're signed in as someone else: sign out to use that account")
}

fn needs_another_way_in(action: &str) -> ApiError {
    ApiError::forbidden(format!("sign in with a passkey or your owner key to {action}"))
}

fn taken(site: Site) -> ApiError {
    ApiError::conflict(format!(
        "that {} account is already linked to another account here",
        site.name()
    ))
}

fn is_on(state: &AppState, site: Site) -> bool {
    match site {
        Site::Discord => state.discord.is_some(),
        Site::GitHub => state.github.is_some(),
    }
}

fn require_on(state: &AppState, site: Site) -> ApiResult<()> {
    if is_on(state, site) {
        Ok(())
    } else {
        Err(off(site))
    }
}

fn return_address(state: &AppState) -> String {
    format!("{}/sign-in/back", state.config.public_url)
}

/// Long enough for PKCE, which wants at least 43 characters.
fn new_verifier() -> String {
    format!("{}{}", auth::new_ceremony_id(), auth::new_ceremony_id())
}

/// A50: whether this is the cookie the step was started with.
fn same_browser(browser: Option<&str>, browser_hash: &str) -> bool {
    browser.map(auth::hash_secret).as_deref() == Some(browser_hash)
}

/// Base64url with no padding, which is how PKCE writes a hash.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn challenge_for(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

/// The address a step sends the browser to. It asks for as little as each site lets it (A47).
/// `prompt` is Discord's: `consent` shows the screen, and `none` skips it for someone who said yes
/// when they linked, which GitHub does by itself.
fn address(
    state: &AppState,
    site: Site,
    step: &str,
    verifier: &str,
    prompt: &str,
) -> ApiResult<String> {
    let return_to = return_address(state);
    let challenge = challenge_for(verifier);
    let mut asked = vec![
        ("redirect_uri", return_to.as_str()),
        ("state", step),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ];
    let authorize_url = match (site, &state.discord, &state.github) {
        (Site::Discord, Some(app), _) => {
            asked.extend([
                ("client_id", app.client_id.as_str()),
                ("response_type", "code"),
                ("scope", "identify"),
                ("prompt", prompt),
            ]);
            &app.authorize_url
        }
        (Site::GitHub, _, Some(app)) => {
            asked.push(("client_id", app.client_id.as_str()));
            &app.authorize_url
        }
        _ => return Err(off(site)),
    };
    Ok(Url::parse_with_params(authorize_url, asked).map_err(|_| off(site))?.to_string())
}

/// A54: what a takeover of someone's Discord or GitHub account mustn't be able to do. Sessions
/// started any other way pass, and so does a key or a token, because `secret_hash` matches no mark.
pub fn refuse_outside_session(state: &AppState, caller: &Caller, action: &str) -> ApiResult<()> {
    if state.db.is_sign_in_session(&caller.secret_hash)? {
        return Err(needs_another_way_in(action));
    }
    Ok(())
}

/// A53: someone whose account gets a new way in hears of it, in case it wasn't them. It isn't about
/// a room, so the link goes to the home page, where it can be undone.
fn tell_linked(state: &AppState, site: Site, person_id: &str) {
    let text = format!(
        "A {} account was linked to your account, and it can sign in as you. \
         If that wasn't you, unlink it and replace your owner key.",
        site.name()
    );
    let notice =
        notify::Notice { event: "account.sign_in_linked", hangout_id: String::new(), text };
    notify::send(state, &[person_id.to_string()], &notice);
}

/// A56: a person's own links, for `GET /api/me`. One that's linked stays listed with its site off,
/// because it can still be unlinked (A55).
pub fn for_person(state: &AppState, person_id: &str) -> ApiResult<Vec<Value>> {
    let links = state.db.sign_in_links(person_id)?;
    Ok(Site::ALL
        .into_iter()
        .filter_map(|site| {
            let linked = links.iter().find(|(key, _)| key == site.key()).map(|(_, name)| name);
            (is_on(state, site) || linked.is_some())
                .then(|| json!({ "site": site.key(), "name": site.name(), "linked": linked }))
        })
        .collect())
}

/// The sites that are on, for the page to offer (A58).
pub fn sites_on(state: &AppState) -> Vec<Value> {
    Site::ALL
        .into_iter()
        .filter(|site| is_on(state, *site))
        .map(|site| json!({ "site": site.key(), "name": site.name() }))
        .collect()
}

// Routes

/// Starts a step that links an account to this person, and says where the browser goes next.
pub fn start_link(
    state: &AppState,
    caller: &Caller,
    site: Site,
    from: Option<IpAddr>,
    now: i64,
) -> ApiResult<String> {
    ops::require_human(caller, "link an account")?;
    require_on(state, site)?;
    refuse_outside_session(state, caller, "link an account")?;
    let verifier = new_verifier();
    let pending = Pending::SignInLink {
        site,
        person_id: caller.person_id.clone(),
        verifier: verifier.clone(),
    };
    let step = state.ceremonies.begin(pending, now, from).ok_or_else(busy)?;
    address(state, site, &step, &verifier, "consent")
}

/// Starts a sign-in, and gives back where the browser goes next and the value of its cookie.
pub fn start_sign_in(
    state: &AppState,
    caller: Option<&Caller>,
    site: Site,
    new: bool,
    from: Option<IpAddr>,
    now: i64,
) -> ApiResult<(String, String)> {
    passkey::refuse_bots(caller)?;
    require_on(state, site)?;
    let browser = auth::new_ceremony_id();
    let verifier = new_verifier();
    let pending = Pending::SignIn {
        site,
        browser_hash: auth::hash_secret(&browser),
        verifier: verifier.clone(),
    };
    let step = state.ceremonies.begin(pending, now, from).ok_or_else(busy)?;
    // A59: nobody makes an account without seeing the site's screen.
    let prompt = if new { "consent" } else { "none" };
    Ok((address(state, site, &step, &verifier, prompt)?, browser))
}

/// Finishes a step with what the site handed back. `browser` is the step cookie, if there is one.
pub async fn finish(
    state: &AppState,
    caller: Option<&Caller>,
    browser: Option<&str>,
    from: Option<IpAddr>,
    step: &str,
    code: &str,
    now: i64,
) -> ApiResult<Finished> {
    passkey::refuse_bots(caller)?;
    match state.ceremonies.take(step, now) {
        Some(Pending::SignInLink { site, person_id, verifier }) => {
            require_on(state, site)?;
            // A49: otherwise your yes could put someone else's account on yours.
            if caller.is_none_or(|caller| caller.person_id != person_id) {
                return Err(lapsed());
            }
            let account = read_account(state, site, code, &verifier)
                .await
                .ok_or_else(|| unconfirmed(site))?;
            let key = site.key();
            if !state.db.set_sign_in_link(&person_id, key, &account.id, &account.name, now)? {
                return Err(taken(site));
            }
            tell_linked(state, site, &person_id);
            Ok(Finished::Linked)
        }
        Some(Pending::SignIn { site, browser_hash, verifier }) => {
            require_on(state, site)?;
            // A50: otherwise a link could sign you in to someone else's account.
            if !same_browser(browser, &browser_hash) {
                return Err(lapsed());
            }
            let account = read_account(state, site, code, &verifier)
                .await
                .ok_or_else(|| unconfirmed(site))?;
            let person = state.db.person_by_sign_in(site.key(), &account.id, &account.name)?;
            match (person, caller) {
                (None, _) => offer_account(state, caller, site, browser_hash, account, from, now),
                (Some(person), None) => Ok(Finished::SignedIn(person, site)),
                (Some(person), Some(caller)) if person.id == caller.person_id => {
                    Ok(Finished::AlreadyIn)
                }
                (Some(_), Some(_)) => Err(someone_else()),
            }
        }
        _ => Err(lapsed()),
    }
}

/// A59: holds what the site proved until the person says yes, and says what to offer them.
fn offer_account(
    state: &AppState,
    caller: Option<&Caller>,
    site: Site,
    browser_hash: String,
    account: Account,
    from: Option<IpAddr>,
    now: i64,
) -> ApiResult<Finished> {
    if caller.is_some() {
        return Err(signed_in());
    }
    if state.ceremonies.signups_full(now) {
        return Err(passkey::too_many_signups());
    }
    let shown = account.name.clone();
    let pending =
        Pending::SignUp { site, browser_hash, account_id: account.id, account_name: account.name };
    let step = state.ceremonies.begin(pending, now, from).ok_or_else(busy)?;
    Ok(Finished::Offered(
        json!({ "step": step, "site": site.key(), "name": site.name(), "account": shown }),
    ))
}

/// Makes the account someone said yes to, with its link, and gives back its owner key (A59, A63).
/// The name is read before the step is taken, so a bad one can be fixed (A60).
pub fn make_account(
    state: &AppState,
    caller: Option<&Caller>,
    browser: Option<&str>,
    step: &str,
    your_name: &str,
    now: i64,
) -> ApiResult<(Person, String)> {
    passkey::refuse_bots(caller)?;
    if caller.is_some() {
        return Err(signed_in());
    }
    let name = ops::clean(your_name, "your name", MAX_NAME_CHARS)?;
    let Some(Pending::SignUp { site, browser_hash, account_id, account_name }) =
        state.ceremonies.take(step, now)
    else {
        return Err(lapsed());
    };
    if !same_browser(browser, &browser_hash) {
        return Err(lapsed());
    }
    if !state.ceremonies.take_signup(now) {
        return Err(passkey::too_many_signups());
    }
    let key = auth::new_owner_key();
    let made = state.db.create_account_with_sign_in(
        &auth::new_id(),
        &name,
        &auth::hash_secret(&key),
        &auth::new_feed_token(),
        site.key(),
        &account_id,
        &account_name,
        now,
    );
    if !matches!(made, Ok(Some(_))) {
        state.ceremonies.give_back_signup(now);
    }
    match made? {
        Some(person) => Ok((person, key)),
        None => Err(taken(site)),
    }
}

/// A55: works with the site off, because taking a way in back shouldn't depend on a setting. It
/// ends the session doing it, if that site started that one.
pub fn unlink(state: &AppState, caller: &Caller, site: Site) -> ApiResult<()> {
    ops::require_human(caller, "unlink an account")?;
    // A54: a session a site started can give that site up, which ends it, but no other way in.
    let started_by = state.db.sign_in_session_site(&caller.secret_hash)?;
    if started_by.is_some_and(|key| key != site.key()) {
        return Err(needs_another_way_in("unlink another site"));
    }
    state.db.remove_sign_in_link(&caller.person_id, site.key())?;
    Ok(())
}

/// Swaps a code for the account behind it. `None` means no account came of it, and nothing here
/// logs a code or a token. The token goes no further than this (A47).
async fn read_account(state: &AppState, site: Site, code: &str, verifier: &str) -> Option<Account> {
    let return_to = return_address(state);
    match site {
        Site::Discord => {
            let app = state.discord.as_ref()?;
            let account = discord::account_for_sign_in(app, code, &return_to, verifier).await?;
            Some(Account { id: account.id, name: account.username })
        }
        Site::GitHub => {
            let app = state.github.as_ref()?;
            let user = read_github_user(app, code, &return_to, verifier).await?;
            Some(Account { id: user.id.to_string(), name: user.login })
        }
    }
}

async fn read_github_user(
    app: &GitHubApp,
    code: &str,
    return_to: &str,
    verifier: &str,
) -> Option<GitHubUser> {
    // Nothing here is a person's URL, but a redirect would still take the secret somewhere else.
    let client = Client::builder()
        .redirect(redirect::Policy::none())
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .ok()?;
    let token = client
        .post(&app.token_url)
        .header(header::ACCEPT, "application/json")
        .form(&[
            ("client_id", app.client_id.as_str()),
            ("client_secret", app.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", return_to),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<GitHubToken>()
        .await
        .ok()?;
    let user = client
        .get(format!("{}/user", app.api_url))
        .bearer_auth(&token.access_token)
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<GitHubUser>()
        .await
        .ok()?;
    let login_fits = (1..=MAX_LOGIN_CHARS).contains(&user.login.chars().count())
        && !user.login.chars().any(char::is_control);
    (user.id > 0 && login_fits).then_some(user)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The worked example in RFC 7636, appendix B.
    #[test]
    fn a_challenge_is_what_pkce_says_it_is() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_verifier_is_long_enough_for_pkce() {
        assert!(new_verifier().len() >= 43);
    }

    #[test]
    fn a_site_is_known_by_its_key() {
        for site in Site::ALL {
            assert_eq!(Site::from_key(site.key()), Some(site));
        }
        assert_eq!(Site::from_key("myspace"), None);
    }
}
