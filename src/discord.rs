//! Linking a Discord account to a bot: the rules in docs/accounts-and-trust.md (A14 to A26).
//!
//! Discord proves the account, the server keeps its id and username, and that's the end of it: this
//! never signs anyone in, and no token outlives the request that got it. Handlers in `api` only call
//! these.

use std::{net::IpAddr, time::Duration};

use reqwest::{redirect, Client, StatusCode, Url};
use serde::Deserialize;

use crate::{
    auth::Caller,
    error::{ApiError, ApiResult},
    now, ops,
    passkey::Pending,
    AppState,
};

const TIMEOUT: Duration = Duration::from_secs(5);
/// The longest username Discord gives out.
const MAX_USERNAME_CHARS: usize = 32;
/// A Discord id is a 64-bit number, which is at most this long written out.
const MAX_ID_DIGITS: usize = 20;

/// The app registered with Discord that people say yes to.
pub struct App {
    pub client_id: String,
    pub client_secret: String,
    pub authorize_url: String,
    pub api_url: String,
}

impl App {
    /// The app these settings name. It takes both: without them linking is off (A25).
    pub fn from_settings(client_id: Option<String>, client_secret: Option<String>) -> Option<Self> {
        Some(Self {
            client_id: client_id.filter(|id| !id.is_empty())?,
            client_secret: client_secret.filter(|secret| !secret.is_empty())?,
            authorize_url: "https://discord.com/oauth2/authorize".to_string(),
            api_url: "https://discord.com/api/v10".to_string(),
        })
    }
}

/// What's read from Discord. Everything else it sends is ignored, so it can't be kept by accident.
#[derive(Deserialize)]
pub(crate) struct Account {
    pub(crate) id: String,
    pub(crate) username: String,
}

#[derive(Deserialize)]
struct Token {
    access_token: String,
}

// Errors

fn off() -> ApiError {
    ApiError::conflict("Discord linking isn't set up on this server")
}

fn busy() -> ApiError {
    ApiError::too_many_requests(
        "too many Discord links are open right now; try again in a few minutes",
    )
}

/// A18: the same for a step that lapsed, was used, is someone else's or isn't a step.
fn lapsed() -> ApiError {
    ApiError::bad_request("that Discord step has lapsed or was already used; start again")
}

fn unconfirmed() -> ApiError {
    ApiError::new(StatusCode::BAD_GATEWAY, "Discord didn't confirm an account; start again")
}

fn app(state: &AppState) -> ApiResult<&App> {
    state.discord.as_ref().ok_or_else(off)
}

fn return_address(state: &AppState) -> String {
    format!("{}/discord/linked", state.config.public_url)
}

fn refresh_rooms(state: &AppState, agent_id: &str) -> ApiResult<()> {
    for hangout in state.db.hangouts_for_agent(agent_id, now())? {
        ops::refresh(state, &hangout.id);
    }
    Ok(())
}

// Routes

/// Starts a step for this person and this bot, and says where the browser goes next. The route
/// redirects there, so no script ever follows an address from the API (A17).
pub fn start(
    state: &AppState,
    caller: &Caller,
    agent_id: &str,
    from: Option<IpAddr>,
    now: i64,
) -> ApiResult<String> {
    ops::require_human(caller, "link a bot to Discord")?;
    let app = app(state)?;
    ops::own_agent(state, caller, agent_id, "link a bot to Discord")?;
    // R44: a house bot is the house's, and A14 keeps a person's name off it.
    if state.db.bot_home(agent_id)?.is_some_and(|home| home.is_house) {
        return Err(ApiError::conflict("a house bot belongs to its house, not to one person"));
    }
    let pending =
        Pending::Discord { person_id: caller.person_id.clone(), agent_id: agent_id.to_string() };
    let step = state.ceremonies.begin(pending, now, from).ok_or_else(busy)?;
    let url = Url::parse_with_params(
        &app.authorize_url,
        [
            ("response_type", "code"),
            ("client_id", app.client_id.as_str()),
            ("scope", "identify"),
            ("state", step.as_str()),
            ("redirect_uri", return_address(state).as_str()),
            ("prompt", "consent"),
        ],
    )
    .map_err(|_| off())?;
    Ok(url.to_string())
}

/// Finishes a step with what Discord handed back, and says which bot was linked.
pub async fn finish(
    state: &AppState,
    caller: &Caller,
    step: &str,
    code: &str,
    now: i64,
) -> ApiResult<String> {
    ops::require_human(caller, "link a bot to Discord")?;
    let app = app(state)?;
    let Some(Pending::Discord { person_id, agent_id }) = state.ceremonies.take(step, now) else {
        return Err(lapsed());
    };
    // A18: otherwise your yes on Discord could land on the bot of whoever sent you the link.
    if person_id != caller.person_id {
        return Err(lapsed());
    }
    let account = read_account(state, app, code).await.ok_or_else(unconfirmed)?;
    // Discord may have sat waiting for a while, so this is checked last.
    ops::own_agent(state, caller, &agent_id, "link a bot to Discord")?;
    state.db.set_discord_link(&agent_id, &account.id, &account.username, now)?;
    refresh_rooms(state, &agent_id)?;
    Ok(agent_id)
}

/// A20: works with linking off, because taking your name back shouldn't depend on a setting.
pub fn unlink(state: &AppState, caller: &Caller, agent_id: &str) -> ApiResult<()> {
    ops::own_agent(state, caller, agent_id, "unlink a bot from Discord")?;
    state.db.remove_discord_link(agent_id)?;
    refresh_rooms(state, agent_id)
}

// Nothing here is a person's URL, but a redirect would still take the secret somewhere else.
fn client() -> Option<Client> {
    Client::builder().redirect(redirect::Policy::none()).timeout(TIMEOUT).build().ok()
}

/// Swaps a code for a token. `verifier` is for a step that used PKCE.
async fn swap_code(
    client: &Client,
    app: &App,
    code: &str,
    return_to: &str,
    verifier: Option<&str>,
) -> Option<Token> {
    let mut form =
        vec![("grant_type", "authorization_code"), ("code", code), ("redirect_uri", return_to)];
    form.extend(verifier.map(|verifier| ("code_verifier", verifier)));
    client
        .post(format!("{}/oauth2/token", app.api_url))
        .basic_auth(&app.client_id, Some(&app.client_secret))
        .form(&form)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Token>()
        .await
        .ok()
}

/// Swaps a code for the account behind it. The token is revoked here, whatever the read found, so
/// none outlives this. `None` means no account came of it, and nothing here logs a code or a token.
async fn read_account(state: &AppState, app: &App, code: &str) -> Option<Account> {
    let client = client()?;
    let token = swap_code(&client, app, code, &return_address(state), None).await?;
    let account = fetch_account(&client, app, &token.access_token).await;
    let revoked = client
        .post(format!("{}/oauth2/token/revoke", app.api_url))
        .basic_auth(&app.client_id, Some(&app.client_secret))
        .form(&[("token", token.access_token.as_str()), ("token_type_hint", "access_token")])
        .send()
        .await;
    // It lapses by itself within a week, and the link is good either way.
    if !revoked.is_ok_and(|reply| reply.status().is_success()) {
        tracing::warn!("a Discord token could not be revoked");
    }
    account
}

/// The same read for signing in (A47). The token is dropped rather than revoked, because revoking it
/// would take back the person's yes, and Discord would ask again at every sign-in.
pub(crate) async fn account_for_sign_in(
    app: &App,
    code: &str,
    return_to: &str,
    verifier: &str,
) -> Option<Account> {
    let client = client()?;
    let token = swap_code(&client, app, code, return_to, Some(verifier)).await?;
    fetch_account(&client, app, &token.access_token).await
}

async fn fetch_account(client: &Client, app: &App, token: &str) -> Option<Account> {
    let account = client
        .get(format!("{}/users/@me", app.api_url))
        .bearer_auth(token)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Account>()
        .await
        .ok()?;
    let id_is_a_number = !account.id.is_empty()
        && account.id.len() <= MAX_ID_DIGITS
        && account.id.bytes().all(|byte| byte.is_ascii_digit());
    let name_fits = (1..=MAX_USERNAME_CHARS).contains(&account.username.chars().count())
        && !account.username.chars().any(char::is_control);
    (id_is_a_number && name_fits).then_some(account)
}
