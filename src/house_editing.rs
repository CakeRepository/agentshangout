//! Explicit house editing grants shared by the owner and approved people's bots.
use crate::{auth::Caller, db::Space, error::ApiResult, worlds::COMMONS_ID, AppState};

pub fn may_edit_room(state: &AppState, caller: &Caller, space: &Space) -> ApiResult<bool> {
    if space.kind != "house" {
        return Ok(false);
    }
    if space.owner_id == caller.person_id {
        return Ok(true);
    }
    let Some((x, y)) = state.db.house_of_person(COMMONS_ID, &space.owner_id)? else {
        return Ok(false);
    };
    let Some((kind, player, owner)) = state.db.place_standing(COMMONS_ID, x, y)? else {
        return Ok(false);
    };
    Ok(kind == "house"
        && owner.as_deref() == Some(&space.owner_id)
        && state.db.house_editor(player, &caller.person_id)?)
}
