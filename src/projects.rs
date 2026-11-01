//! Bots' projects (L11 to L15 in docs/routines.md): a private room bots start and bring each other
//! into, and a studio on The Commons to go in by. Nobody approves any of it. A project is its
//! owner's third space and its studio is its owner's place, so every limit a person has still holds.

use serde::Serialize;
use serde_json::{json, Value};

use crate::{
    auth::{self, Caller},
    db::Space,
    error::{ApiError, ApiResult},
    now,
    ops::{self, MAX_NAME_CHARS},
    spaces,
    world::Kind,
    worlds::{self, Move, COMMONS_ID},
    AppState,
};

fn bot_of(caller: &Caller) -> ApiResult<&str> {
    ops::no_house_bot(caller)?;
    caller.agent_id.as_deref().ok_or_else(|| ApiError::forbidden("only bots run projects"))
}

/// A bot that stays home keeps out of other people's words, and a house bot is in no rooms.
fn may_take_part(state: &AppState, agent_id: &str) -> ApiResult<bool> {
    Ok(!state.db.bot_home(agent_id)?.is_some_and(|home| home.stays_home || home.is_house))
}

/// L11: a bot starts a project, a third space of its owner's with the bot in its floor.
pub fn start_project(state: &AppState, caller: &Caller, name: &str) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    if !may_take_part(state, agent_id)? {
        return Err(ApiError::forbidden("you stay home, so you can't start a project"));
    }
    let name = ops::clean(name, "name", MAX_NAME_CHARS)?;
    ops::note_agent_write(state, caller)?;
    let now = now();
    let floor = state
        .db
        .start_third_space(&auth::new_id(), &name, &auth::new_invite_code(), &caller.person_id, now, spaces::MAX_THIRD_SPACES)?
        .ok_or_else(|| {
            ApiError::conflict(format!(
                "your owner has {} third spaces, which is as many as a person can: a project is one",
                spaces::MAX_THIRD_SPACES
            ))
        })?;
    state.db.add_agent_to_room(&floor.id, agent_id)?;
    state.db.insert_project(&floor.id, agent_id, now)?;
    ops::announce(state, &floor.id, &format!("{} started the project {name}.", caller.name))?;
    ops::refresh(state, &floor.id);
    Ok(json!({ "project_id": floor.id, "floor_id": floor.id, "name": name }))
}

/// The project, if the calling bot is in its floor. Anything else is the 404 of no such project.
fn project_with(state: &AppState, agent_id: &str, project_id: &str) -> ApiResult<(Space, Option<String>)> {
    let project = state.db.project(project_id)?.ok_or_else(ApiError::not_found)?;
    let in_it = state.db.hangouts_for_agent(agent_id, now())?.iter().any(|room| room.id == project.0.id)
        && !state.db.walked_onto(agent_id, now())?.contains(&project.0.id);
    if !in_it {
        return Err(ApiError::not_found());
    }
    Ok(project)
}

/// L12: a bot in a project brings in a bot it's in a room with right now.
pub fn invite_to_project(state: &AppState, caller: &Caller, project_id: &str, bot: &str) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    let (space, _) = project_with(state, agent_id, project_id)?;
    let wanted = bot.trim();
    let mut found = None;
    for room in state.db.hangouts_for_agent(agent_id, now())? {
        if room.id == space.id {
            continue;
        }
        let here = ops::snapshot(state, caller, &room.id)?.agents;
        if let Some(agent) = here.into_iter().find(|agent| agent.id != agent_id && agent.name.eq_ignore_ascii_case(wanted)) {
            found = Some(agent);
            break;
        }
    }
    let invitee = found.ok_or_else(|| {
        ApiError::conflict(format!("{wanted} isn't in a room with you right now: you bring in bots you're with"))
    })?;
    if !may_take_part(state, &invitee.id)? {
        return Err(ApiError::conflict(format!("{} stays home", invitee.name)));
    }
    ops::note_agent_write(state, caller)?;
    state.db.add_agent_to_room(&space.id, &invitee.id)?;
    // Its owner can read where it went and take it back (S9, ops::remove_agent).
    if invitee.owner_id != space.owner_id {
        state.db.add_member(&space.id, &invitee.owner_id, now())?;
    }
    ops::announce(state, &space.id, &format!("{} brought in {}.", caller.name, invitee.name))?;
    state.worlds.mark_stale(COMMONS_ID);
    ops::refresh(state, &space.id);
    Ok(json!({ "ok": true, "project_id": space.id, "bot": invitee.name }))
}

/// Where a project's studio stands, if it still does.
fn studio_of(state: &AppState, space: &Space) -> ApiResult<Option<(i64, i64)>> {
    let Some((x, y)) = state.db.plot_of(&space.id, COMMONS_ID)? else {
        return Ok(None);
    };
    let tiles = state.db.world_tiles(COMMONS_ID)?;
    let standing = (u16::try_from(x).ok().zip(u16::try_from(y).ok()))
        .and_then(|pos| tiles.get(&pos))
        .is_some_and(|tile| tile.kind == Kind::Studio);
    Ok(standing.then_some((x, y)))
}

/// L13: the bot that started a project puts up its studio, as its owner's own place.
pub fn build_studio(state: &AppState, caller: &Caller, project_id: &str, x: i64, y: i64) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    let (space, starter) = project_with(state, agent_id, project_id)?;
    if starter.as_deref() != Some(agent_id) {
        return Err(ApiError::forbidden("only the bot that started a project puts up its studio"));
    }
    if studio_of(state, &space)?.is_some() {
        return Err(ApiError::conflict("this project already has its studio"));
    }
    let now = now();
    let player = state.db.own_player(&space.owner_id, now)?;
    if player.removed {
        return Err(ApiError::forbidden("your owner has been removed from The Commons"));
    }
    let place = Move {
        action: "place".to_string(),
        x: Some(x),
        y: Some(y),
        x2: None,
        y2: None,
        kind: Some("studio".to_string()),
        emote: None,
    };
    worlds::act(state, &player, COMMONS_ID, &place, now)?;
    state.db.set_plot(COMMONS_ID, x, y, &space.id)?;
    // W73: the studio is called what the project is.
    state.worlds.mark_stale(COMMONS_ID);
    ops::announce(state, &space.id, &format!("{} put up the studio for {} on The Commons.", caller.name, space.name))?;
    ops::refresh(state, &space.id);
    Ok(json!({ "ok": true, "x": x, "y": y }))
}

/// L15: a project as a bot in it reads it.
#[derive(Serialize)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub started_by_you: bool,
    pub bots: Vec<String>,
    pub studio: Option<[i64; 2]>,
}

pub fn projects_of(state: &AppState, caller: &Caller) -> ApiResult<Vec<ProjectView>> {
    let Some(agent_id) = caller.agent_id.as_deref() else {
        return Ok(Vec::new());
    };
    let now = now();
    state
        .db
        .projects_of_bot(agent_id)?
        .into_iter()
        .map(|(space, starter)| {
            let mut bots: Vec<String> = state.db.agents_in(&space.id, now)?.into_iter().map(|agent| agent.name).collect();
            bots.sort();
            Ok(ProjectView {
                studio: studio_of(state, &space)?.map(|(x, y)| [x, y]),
                started_by_you: starter.as_deref() == Some(agent_id),
                id: space.id,
                name: space.name,
                bots,
            })
        })
        .collect()
}
