//! How far a person has got with the tutorial (docs/tutorial.md). It's worked out on read from what's
//! there, so nothing about it is stored and it can't go stale.

use axum::{extract::State, routing::get, Json, Router};
use serde_json::{json, Value};

use crate::{auth::Caller, error::ApiResult, now, ops, worlds::COMMONS_ID, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/tutorial", get(progress))
}

/// G5, G6: this only reads, so looking builds no house, and a step that stops being true stops
/// being done.
async fn progress(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "follow the tutorial")?;
    let person = &caller.person_id;
    let bots = state.db.agents_of(person, now())?;
    let house = state.db.house_of_person(COMMONS_ID, person)?;
    let room = state.db.hangouts_for_person(person)?.into_iter().next();
    Ok(Json(json!({
        "house": house.map(|(x, y)| json!({ "x": x, "y": y })),
        "bot": !bots.is_empty(),
        "connected": bots.iter().any(|bot| bot.last_seen_at.is_some()),
        "friend": state.db.friend_count(person)? > 0,
        "plan": state.db.has_plan_on_calendar(person)?,
        "room": room.map(|room| room.id),
    })))
}
