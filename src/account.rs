//! A27 to A45: a person deleting their own account. What's theirs goes, and what's shared with people
//! who are still there stays without their name. `Db::delete_person` does the deleting. This is who may
//! ask, and telling everyone else afterwards.

use axum::http::HeaderMap;

use crate::{
    auth::{self, Caller},
    city,
    db::AccountGone,
    error::{ApiError, ApiResult},
    now, ops, sign_in, trust, AppState,
};

/// A44: what a room that stays is told. There's no name in it.
pub const LINE: &str = "Someone deleted their account.";

/// A27, A28: only a person, from a browser, with their name typed out. Nothing is changed until all
/// of that has held.
pub fn delete(
    state: &AppState,
    caller: &Caller,
    headers: &HeaderMap,
    typed_name: &str,
) -> ApiResult<()> {
    ops::require_human(caller, "delete an account")?;
    // A54: a stolen Discord or GitHub account mustn't be able to destroy what's theirs.
    sign_in::refuse_outside_session(state, caller, "delete your account")?;
    if auth::bearer_token(headers).is_some() {
        return Err(ApiError::forbidden("an account can only be deleted from its own page"));
    }
    if typed_name.trim() != caller.name {
        return Err(ApiError::bad_request("type your name exactly to delete your account"));
    }
    // A37: a house others live in sees someone moving out, which has its own steps to go through.
    for house in state.db.houses_for_person(&caller.person_id)? {
        if state.db.resident_count(&house.id)? > 1 {
            city::move_out_for_good(state, caller, &house)?;
        }
    }
    let gone = state.db.delete_person(&caller.person_id, &caller.name)?;
    // The account has gone by now, so nothing here can tell the person it hasn't.
    if let Err(error) = tell_everyone(state, &gone) {
        tracing::error!(status = %error.status, "a deleted account left something to tell");
    }
    Ok(())
}

fn tell_everyone(state: &AppState, gone: &AccountGone) -> ApiResult<()> {
    for plan in &gone.plans {
        ops::settle(state, plan)?;
    }
    for room in &gone.rooms {
        ops::announce(state, room, LINE)?;
        ops::refresh(state, room);
    }
    // An open page finds the room gone, or the visitor, and its stream ends.
    for room in &gone.other_rooms {
        ops::refresh(state, room);
    }
    let now = now();
    for city_id in &gone.cities {
        if let Some(city) = state.db.city(city_id)? {
            // T21: one fewer person can mean one fewer vouch to find.
            trust::admit_anyone_ready(state, &city, now)?;
        }
        ops::refresh_city(state, city_id);
    }
    for house_id in &gone.house_bots {
        city::house_bot_needs_a_token(state, house_id)?;
    }
    state.worlds.mark_everything_stale();
    Ok(())
}
