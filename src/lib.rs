pub mod api;
pub mod auth;
pub mod city;
pub mod closeness;
pub mod db;
pub mod doorbell;
pub mod error;
pub mod friends;
pub mod ical;
pub mod mcp;
pub mod notify;
pub mod ops;
pub mod passkey;
pub mod public_mcp;
pub mod referrals;
pub mod trust;
pub mod web;
pub mod world;
pub mod worlds;

use std::{
    collections::HashMap,
    ops::Deref,
    sync::{Arc, Mutex},
};

use axum::{extract::DefaultBodyLimit, middleware, routing::post, Router};
use tokio::sync::broadcast;

use db::Db;

/// Bots are chatty, not bulky: nothing an agent sends here should come close to this.
const MAX_BODY_BYTES: usize = 64 * 1024;

pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[derive(Clone, Debug)]
pub struct HubEvent {
    pub hangout_id: String,
    pub kind: &'static str,
    pub data: String,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// The address people and bots reach this server on, with no trailing slash.
    pub public_url: String,
    /// When set, starting a new hangout requires this code. Joining by invite never does.
    pub create_code: Option<String>,
    /// Lets notifications reach local addresses. For tests only: it has no environment setting,
    /// so a deployed server can't turn it on.
    pub allow_private_webhooks: bool,
    /// The person who looks after The Commons and can remove a player from it (W18): their id,
    /// from `AGENTHANGOUT_KEEPER`. Nobody is, when it isn't set.
    pub keeper_id: Option<String>,
}

pub struct Shared {
    pub db: Db,
    pub config: Config,
    pub hub: broadcast::Sender<HubEvent>,
    /// Passkeys, built from `config.public_url`. `None` when that address is an IP number, which
    /// a passkey can't belong to, so the passkey routes refuse.
    pub webauthn: Option<webauthn_rs::Webauthn>,
    /// Passkey ceremonies that are waiting on a browser, and the cap on new sign-ups.
    pub ceremonies: passkey::Ceremonies,
    /// The public worlds' memory: land, built views, limits, and who is passing through.
    pub worlds: worlds::Runtime,
    /// The limits on plan links and on cities started without a code.
    pub referrals: referrals::Limits,
    /// The limit on asking when people are free.
    pub closeness: closeness::Limits,
    write_windows: Mutex<HashMap<String, (i64, u32)>>,
}

#[derive(Clone)]
pub struct AppState(Arc<Shared>);

impl Deref for AppState {
    type Target = Shared;

    fn deref(&self) -> &Shared {
        &self.0
    }
}

impl AppState {
    pub fn new(db: Db, config: Config) -> Self {
        let (hub, _) = broadcast::channel(256);
        let webauthn = passkey::webauthn_for(&config.public_url);
        Self(Arc::new(Shared {
            db,
            config,
            hub,
            webauthn,
            ceremonies: passkey::Ceremonies::default(),
            worlds: worlds::Runtime::new(),
            referrals: referrals::Limits::new(),
            closeness: closeness::Limits::new(),
            write_windows: Mutex::new(HashMap::new()),
        }))
    }

    pub fn is_https(&self) -> bool {
        self.config.public_url.starts_with("https://")
    }

    /// Counts a write against this bot's one-minute window. Two bots replying to each other
    /// forever is the runaway to expect here, and this caps what it can cost.
    pub fn allow_agent_write(&self, agent_id: &str) -> bool {
        let minute = now() / 60;
        let mut windows = self
            .write_windows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        windows.retain(|_, (window, _)| *window == minute);
        let (_, count) = windows.entry(agent_id.to_string()).or_insert((minute, 0));
        *count += 1;
        *count <= ops::MAX_AGENT_WRITES_PER_MINUTE
    }
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .merge(api::routes())
        .merge(web::routes())
        .route("/mcp", post(mcp::handle))
        .route("/public/mcp", post(public_mcp::handle))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), web::security_headers))
        // Outermost, so it sees every request. It only watches (W38).
        .layer(middleware::from_fn_with_state(state.clone(), worlds::observe))
        .with_state(state)
}
