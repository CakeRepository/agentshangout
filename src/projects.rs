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

/// L35: report only the caller's owner's quota, without naming their other rooms.
pub fn capacity(state: &AppState, caller: &Caller) -> ApiResult<Value> {
    let used = state.db.third_space_count(&caller.person_id)?;
    let exempt = crate::god_mode::is_on(state, &caller.person_id)?;
    let may_start = match &caller.agent_id {
        Some(id) => may_take_part(state, id)?,
        None => false,
    };
    let remaining = (!exempt).then_some((spaces::MAX_THIRD_SPACES - used).max(0));
    let can_start = may_start && (exempt || used < spaces::MAX_THIRD_SPACES);
    let note = if !may_start {
        "Only an ordinary bot that does not stay home can start a project.".to_string()
    } else if !can_start {
        cap_message()
    } else {
        "You have room to start_project, or work in an existing project. Building its place still checks land and your owner's applicable place limits and waits.".to_string()
    };
    Ok(
        json!({ "can_start": can_start, "used": used, "limit": (!exempt).then_some(spaces::MAX_THIRD_SPACES), "remaining": remaining, "note": note }),
    )
}

fn cap_message() -> String {
    format!("your owner's normal limit is {} third spaces; projects share that limit. Work in an existing project, or ask your owner to review their third spaces before choosing whether to close one. Closing a space removes its rooms and history; do not do it automatically. The normal three-space limit also applies when a person creates a new third space", spaces::MAX_THIRD_SPACES)
}

/// L11: a bot starts a project, a third space of its owner's with the bot in its floor.
pub fn start_project(
    state: &AppState,
    caller: &Caller,
    name: &str,
    purpose: Option<&str>,
) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    if !may_take_part(state, agent_id)? {
        return Err(ApiError::forbidden("you stay home, so you can't start a project"));
    }
    let name = ops::clean(name, "name", MAX_NAME_CHARS)?;
    let purpose = purpose.map(clean_purpose).transpose()?;
    ops::note_agent_write(state, caller)?;
    let now = now();
    // W95: a bot whose owner has god mode on starts as many projects as it wants.
    let most = if crate::god_mode::is_on(state, &caller.person_id)? {
        i64::MAX
    } else {
        spaces::MAX_THIRD_SPACES
    };
    let floor = state
        .db
        .start_third_space(
            &auth::new_id(),
            &name,
            &auth::new_invite_code(),
            &caller.person_id,
            now,
            most,
        )?
        .ok_or_else(|| ApiError::conflict(cap_message()))?;
    state.db.add_agent_to_room(&floor.id, agent_id)?;
    state.db.insert_project(&floor.id, agent_id, now)?;
    // L33: what it's for, if the bot said.
    if let (Some(purpose), Some(space)) = (&purpose, state.db.space(&floor.id)?) {
        state.db.change_space(&space, None, Some(purpose))?;
    }
    ops::announce(state, &floor.id, &format!("{} started the project {name}.", caller.name))?;
    // S59: it starts out in the room it just made.
    spaces::place_bot(state, agent_id, &floor.id)?;
    Ok(json!({ "project_id": floor.id, "floor_id": floor.id, "name": name }))
}

/// The project, if the calling bot is in its floor. Anything else is the 404 of no such project.
fn project_with(
    state: &AppState,
    agent_id: &str,
    project_id: &str,
) -> ApiResult<(Space, Option<String>)> {
    let project = state.db.project(project_id)?.ok_or_else(ApiError::not_found)?;
    let in_it =
        state.db.hangouts_for_agent(agent_id, now())?.iter().any(|room| room.id == project.0.id)
            && !state.db.walked_onto(agent_id, now())?.contains(&project.0.id);
    if !in_it {
        return Err(ApiError::not_found());
    }
    Ok(project)
}

/// L12: a bot in a project brings in a bot it's in a room with right now.
pub fn invite_to_project(
    state: &AppState,
    caller: &Caller,
    project_id: &str,
    bot: &str,
) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    let (space, _) = project_with(state, agent_id, project_id)?;
    let wanted = bot.trim();
    // S55: only the room it's in, and only a bot that's really there.
    let room = state.db.bot_place(agent_id, now())?.filter(|room| *room != space.id);
    let here = room.map(|room| ops::snapshot(state, caller, &room)).transpose()?;
    let found = here.into_iter().flat_map(|snapshot| snapshot.agents).find(|agent| {
        !agent.away && agent.id != agent_id && agent.name.eq_ignore_ascii_case(wanted)
    });
    let invitee = found.ok_or_else(|| {
        ApiError::conflict(format!(
            "{wanted} isn't in a room with you right now: you bring in bots you're with"
        ))
    })?;
    if !may_take_part(state, &invitee.id)? {
        return Err(ApiError::conflict(format!("{} stays home", invitee.name)));
    }
    ops::note_agent_write(state, caller)?;
    // S13, S27: being brought in is a way of joining, so it's no way round either of them.
    spaces::check_can_join(state, &invitee.owner_id, &space.id)?;
    state.db.add_agent_to_room(&space.id, &invitee.id)?;
    crate::events::note_place(state, &invitee.id, &space.id, "moved");
    // Its owner can read where it went and take it back (S9, ops::remove_agent).
    if invitee.owner_id != space.owner_id {
        state.db.add_member(&space.id, &invitee.owner_id, now())?;
    }
    ops::announce(state, &space.id, &format!("{} brought in {}.", caller.name, invitee.name))?;
    state.worlds.mark_stale(COMMONS_ID);
    ops::refresh(state, &space.id);
    Ok(json!({ "ok": true, "project_id": space.id, "bot": invitee.name }))
}

/// Where a project's studio stands, if it still does. Only that one tile is read, because `inbox`
/// asks this of every project and nothing limits how often a bot reads it (W30).
fn studio_of(state: &AppState, space: &Space) -> ApiResult<Option<(i64, i64)>> {
    Ok(building_of(state, space)?.map(|(x, y, _)| (x, y)))
}

/// L32: a project's building, of whatever kind, and where it stands, if it still does.
fn building_of(state: &AppState, space: &Space) -> ApiResult<Option<(i64, i64, String)>> {
    let Some((x, y)) = state.db.plot_of(&space.id, COMMONS_ID)? else {
        return Ok(None);
    };
    Ok(state
        .db
        .building_at(COMMONS_ID, x, y)?
        .filter(|(kind, _)| kind != "house" && kind != "square")
        .map(|(kind, _)| (x, y, kind)))
}

/// L33: a purpose is a sentence or two, and never empty.
fn clean_purpose(text: &str) -> ApiResult<String> {
    ops::clean(text, "purpose", ops::MAX_ABOUT_CHARS)
}

/// L33: any bot in a project says what it's for, and the room hears it changed.
pub fn set_purpose(
    state: &AppState,
    caller: &Caller,
    project_id: &str,
    purpose: &str,
) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    let (space, _) = project_with(state, agent_id, project_id)?;
    let purpose = clean_purpose(purpose)?;
    ops::note_agent_write(state, caller)?;
    state.db.change_space(&space, None, Some(&purpose))?;
    ops::announce(
        state,
        &space.id,
        &format!("{} set what {} is for: {purpose}", caller.name, space.name),
    )?;
    ops::refresh(state, &space.id);
    Ok(json!({ "ok": true, "project_id": space.id, "purpose": purpose }))
}

/// L32: what a project's building can be: any place but a house, which is a person's, or the town
/// square, which is the server's.
fn project_place(kind: Option<&str>) -> ApiResult<crate::world::Kind> {
    use crate::world::Kind;
    match kind.map_or(Some(Kind::Studio), Kind::parse_place) {
        Some(Kind::House | Kind::Square) => Err(ApiError::bad_request(
            "a house is a person's own, and the town square the server's",
        )),
        Some(kind) => Ok(kind),
        None => Err(ApiError::bad_request(crate::world::NOT_A_PLACE)),
    }
}

/// L13: the bot that started a project puts up its studio, as its owner's own place.
pub fn build_studio(
    state: &AppState,
    caller: &Caller,
    project_id: &str,
    x: i64,
    y: i64,
    kind: Option<&str>,
    purpose: Option<&str>,
) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    let (space, starter) = project_with(state, agent_id, project_id)?;
    if starter.as_deref() != Some(agent_id) {
        return Err(ApiError::forbidden(
            "only the bot that started a project puts up its building",
        ));
    }
    let purpose = purpose.map(clean_purpose).transpose()?;
    // L33: a building goes up for a reason, so whoever walks in knows what it's for.
    if purpose.is_none() && space.about.trim().is_empty() {
        return Err(ApiError::bad_request(
            "say what this building is for: give a purpose, a sentence or two on what it was made to do or talk about",
        ));
    }
    ops::note_agent_write(state, caller)?;
    if let Some(purpose) = &purpose {
        state.db.change_space(&space, None, Some(purpose))?;
    }
    let place_kind = project_place(kind)?;
    if studio_of(state, &space)?.is_some() {
        return Err(ApiError::conflict("this project already has its building"));
    }
    let now = now();
    // L22: with no road beside the tile yet, it becomes the project's site, with the way to it.
    let (tiles, land, hoods) = map_of(state)?;
    let pos = land.land.locate(x, y).ok_or_else(|| ApiError::bad_request("that is off the map"))?;
    // W91: a big place is reached from beside any of its tiles.
    let beside_road = crate::world::footprint(place_kind, pos).into_iter().any(|cell| {
        [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
            let at = (u16::try_from(i32::from(cell.0) + dx), u16::try_from(i32::from(cell.1) + dy));
            matches!(at, (Ok(ax), Ok(ay)) if tiles.get(&(ax, ay)).is_some_and(|tile| tile.kind == crate::world::Kind::Road))
        })
    });
    if !beside_road {
        if tiles.contains_key(&pos) || !land.land.at(pos).is_some_and(|ground| ground.is_open()) {
            return Err(ApiError::bad_request("a studio needs open land"));
        }
        let path = crate::world::route_to(&land.land, &hoods, &tiles, pos).ok_or_else(|| {
            ApiError::conflict(
                "no road can reach that tile along the streets: pick somewhere nearer one",
            )
        })?;
        state.db.set_project_site(&space.id, x, y, now)?;
        return Ok(
            json!({ "status": "site", "x": x, "y": y, "route": runs_json(&hoods, &tiles, &path) }),
        );
    }
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
        kind: Some(place_kind.as_str().to_string()),
        emote: None,
    };
    worlds::act(state, &player, COMMONS_ID, &place, now)?;
    state.db.set_plot(COMMONS_ID, x, y, &space.id)?;
    // L24: the studio is up, so the site has done its job.
    state.db.clear_project_site(&space.id)?;
    // W73: the building is called what the project is.
    state.worlds.mark_stale(COMMONS_ID);
    // W92: it's open unless the project's owner closes it, and dressed for what it is.
    open_for_everyone(state, &space)?;
    crate::items::dress_for_place(state, caller, &space.id, place_kind.as_str())?;
    let title = crate::spaces::place_title(place_kind.as_str()).to_lowercase();
    ops::announce(
        state,
        &space.id,
        &format!("{} put up the {title} for {} on The Commons.", caller.name, space.name),
    )?;
    ops::refresh(state, &space.id);
    Ok(json!({ "ok": true, "x": x, "y": y, "kind": place_kind.as_str() }))
}

/// W92: a project's room opens to everyone when its building goes up. Its owner can close it.
fn open_for_everyone(state: &AppState, space: &Space) -> ApiResult<()> {
    state.db.change_floor(&crate::db::FloorChange {
        id: &space.id,
        name: None,
        access: Some("public"),
        size: None,
        paint: None,
        flooring: None,
        keep_bots_of: &space.owner_id,
        private_only: &crate::items::private_only(),
    })?;
    Ok(())
}

/// The Commons as a route needs it: everything built, the land and the neighborhoods.
fn map_of(
    state: &AppState,
) -> ApiResult<(crate::world::Tiles, std::sync::Arc<worlds::WorldLand>, Vec<crate::world::Hood>)> {
    let world = state.db.world(COMMONS_ID)?.ok_or_else(ApiError::not_found)?;
    let land = state.worlds.land_for(world.seed, world.size);
    Ok((
        state.db.world_tiles(COMMONS_ID)?,
        land,
        crate::world::neighborhoods(world.seed, world.size),
    ))
}

/// L22: a route as the runs to lay, `[x, y, x2, y2]` each, first to last.
fn runs_json(
    hoods: &[crate::world::Hood],
    tiles: &crate::world::Tiles,
    path: &[crate::world::Pos],
) -> Vec<[u16; 4]> {
    crate::world::route_runs(hoods, tiles, path)
        .into_iter()
        .map(|(first, last)| [first.0, first.1, last.0, last.1])
        .collect()
}

/// L23: the bot that started a project lays the first run of the road to its site, as its owner's.
pub fn lay_road(state: &AppState, caller: &Caller, project_id: &str) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    let (space, starter) = project_with(state, agent_id, project_id)?;
    if starter.as_deref() != Some(agent_id) {
        return Err(ApiError::forbidden("only the bot that started a project lays its road"));
    }
    let (x, y) = state.db.project_site(&space.id)?.ok_or_else(|| {
        ApiError::conflict("this project has no site: pick one with build_studio")
    })?;
    let (tiles, land, hoods) = map_of(state)?;
    let site = land.land.locate(x, y).ok_or_else(ApiError::not_found)?;
    let path = crate::world::route_to(&land.land, &hoods, &tiles, site).ok_or_else(|| {
        ApiError::conflict("no road can reach the site any more: pick another with build_studio")
    })?;
    let Some(&(first, last)) = crate::world::route_runs(&hoods, &tiles, &path).first() else {
        return Err(ApiError::conflict(
            "the road reaches the site: call build_studio to put the studio up",
        ));
    };
    let now = now();
    // L23: the bot lays it itself, as its own player, so the map shows it out there building. One is
    // made for it if it hasn't joined, and a player it has keeps its token.
    let player = match state.db.player_of_agent(agent_id)? {
        Some(id) => state.db.player(id)?.ok_or_else(ApiError::not_found)?,
        None => {
            let token_hash = crate::auth::hash_secret(&crate::auth::new_guest_token());
            match state.db.join_as_bot(agent_id, &space.owner_id, &token_hash, COMMONS_ID, now)? {
                crate::db::BotJoined::Done(player) => player,
                _ => return Err(ApiError::forbidden("you were removed from The Commons")),
            }
        }
    };
    let road = Move {
        action: "road".to_string(),
        x: Some(i64::from(first.0)),
        y: Some(i64::from(first.1)),
        x2: Some(i64::from(last.0)),
        y2: Some(i64::from(last.1)),
        kind: None,
        emote: None,
    };
    worlds::act(state, &player, COMMONS_ID, &road, now)?;
    let (tiles, _, _) = map_of(state)?;
    let left = crate::world::route_to(&land.land, &hoods, &tiles, site).unwrap_or_default();
    Ok(json!({
        "laid": [first.0, first.1, last.0, last.1],
        "reached": left.is_empty(),
        "route": runs_json(&hoods, &tiles, &left),
    }))
}

/// L15: a project as a bot in it reads it.
#[derive(Serialize)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub started_by_you: bool,
    pub bots: Vec<String>,
    pub studio: Option<[i64; 2]>,
    /// L33: what it's for, in the bots' own words.
    pub purpose: String,
    /// L25: where its studio will go once a road reaches it, and the runs of road still to lay.
    pub site: Option<[i64; 2]>,
    pub route: Vec<[u16; 4]>,
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
            let mut bots: Vec<String> =
                state.db.agents_in(&space.id, now)?.into_iter().map(|agent| agent.name).collect();
            bots.sort();
            let site = state.db.project_site(&space.id)?;
            let route = match site {
                Some((x, y)) => {
                    let (tiles, land, hoods) = map_of(state)?;
                    land.land
                        .locate(x, y)
                        .and_then(|pos| crate::world::route_to(&land.land, &hoods, &tiles, pos))
                        .map(|path| runs_json(&hoods, &tiles, &path))
                        .unwrap_or_default()
                }
                None => Vec::new(),
            };
            Ok(ProjectView {
                purpose: space.about.clone(),
                site: site.map(|(x, y)| [x, y]),
                route,
                studio: studio_of(state, &space)?.map(|(x, y)| [x, y]),
                started_by_you: starter.as_deref() == Some(agent_id),
                id: space.id,
                name: space.name,
                bots,
            })
        })
        .collect()
}
