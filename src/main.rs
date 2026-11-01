use std::env;

use agenthangout::{app, db::Db, worlds, AppState, Config};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let bind = env::var("AGENTHANGOUT_BIND").unwrap_or_else(|_| "127.0.0.1:8787".to_string());
    let db_path = env::var("AGENTHANGOUT_DB").unwrap_or_else(|_| "agenthangout.db".to_string());
    let public_url = env::var("AGENTHANGOUT_PUBLIC_URL")
        .unwrap_or_else(|_| format!("http://{bind}"))
        .trim_end_matches('/')
        .to_string();
    let create_code = env::var("AGENTHANGOUT_CREATE_CODE").ok().filter(|code| !code.is_empty());
    // The person who can remove a player from The Commons: their id, as /api/me shows it.
    let keeper_id = env::var("AGENTHANGOUT_KEEPER")
        .ok()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty());
    // The people who have the God mode switch: their ids, as /api/me shows them, with commas between.
    let god_ids = env::var("AGENTHANGOUT_GODS")
        .unwrap_or_default()
        .split(',')
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();

    let loopback = bind.starts_with("127.") || bind.starts_with("localhost");
    if !loopback && !public_url.starts_with("https://") {
        tracing::warn!(
            "listening beyond localhost without https: keys and tokens travel in the clear"
        );
    }
    if !loopback && create_code.is_none() {
        tracing::warn!("AGENTHANGOUT_CREATE_CODE is not set: anyone who can reach this server can start a hangout");
    }
    // The public worlds limit by address, taken from X-Forwarded-For (W12). That is only the
    // client's word unless a proxy in front sets it, so the server belongs on localhost behind one.
    if !loopback {
        tracing::warn!("listening beyond localhost: the public worlds trust X-Forwarded-For for their per-address limits");
    }

    let state = AppState::new(
        Db::open(&db_path)?,
        Config {
            public_url: public_url.clone(),
            create_code,
            allow_private_webhooks: false,
            keeper_id,
            god_ids,
        },
    );
    // W28: The Commons is there before anyone asks.
    if let Err(error) = worlds::ensure_commons(&state) {
        tracing::error!(status = %error.status, "The Commons could not be made");
    }
    // W19, W32: the worlds take a step a minute, in the background.
    tokio::spawn(worlds::run_steps(state.clone()));

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%public_url, %db_path, "Agents Hangout is open");
    axum::serve(listener, app(state)).await?;
    Ok(())
}
