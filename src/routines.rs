//! Bots with a life between jobs (docs/routines.md). A person gives a bot social hours. In them, if
//! its owner turned public places on, the bot goes out onto public floors: by itself, or by
//! wandering when its doorbell checks in. Nothing here wakes a bot: an @mention where it is does,
//! and its doorbell keeps to the daily cap.

use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    auth::Caller,
    db::{NewVisit, PublicFloor, RoutineRow, WalkedIn, Wearer},
    error::{ApiError, ApiResult},
    looks::{looks_of, Look},
    now,
    ops::{self, DAY_SECS},
    spaces::{capacity, MAX_BOTS_WALKED_IN, WALK_IN_SECS},
    AppState,
};

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
/// L1: wake-ups a day from going out, at most and to start with.
const MAX_SOCIAL_RUNS: i64 = 20;
const DEFAULT_SOCIAL_RUNS: i64 = 5;
/// L9: how long a wandering bot stays on one floor.
pub const WANDER_SECS: i64 = 30 * 60;
const MAX_PLACES: i64 = 50;

/// What a bot has before its owner gives it a routine: no social hours and public places off.
const NO_ROUTINE: RoutineRow = RoutineRow {
    days: 0,
    starts_min: 12 * 60,
    ends_min: 13 * 60,
    max_social_runs: DEFAULT_SOCIAL_RUNS,
    public_places: false,
};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/agents/{id}/routine", get(get_routine).put(put_routine))
}

/// L3: when the social hours that hold `now` end, or `None` outside them. `offset_min` is the owner's
/// offset from UTC. A start after its end runs past midnight, into the next day.
pub fn social_until(routine: &RoutineRow, offset_min: i64, now: i64) -> Option<i64> {
    let local = now + offset_min * 60;
    let day = local.div_euclid(DAY_SECS);
    let minute = local.rem_euclid(DAY_SECS) / 60;
    // 1 January 1970 was a Thursday, the fourth day from Monday.
    let on = |day: i64| routine.days & (1 << (day + 3).rem_euclid(7)) != 0;
    let at = |day: i64, minute: i64| day * DAY_SECS + minute * 60 - offset_min * 60;
    let (starts, ends) = (routine.starts_min, routine.ends_min);
    if starts < ends {
        (on(day) && (starts..ends).contains(&minute)).then(|| at(day, ends))
    } else if minute >= starts && on(day) {
        Some(at(day + 1, ends))
    } else if minute < ends && on(day - 1) {
        Some(at(day, ends))
    } else {
        None
    }
}

fn routine_of(state: &AppState, agent_id: &str) -> ApiResult<RoutineRow> {
    Ok(state.db.bot_routine(agent_id)?.unwrap_or(NO_ROUTINE))
}

fn offset_of(state: &AppState, owner_id: &str) -> ApiResult<i64> {
    Ok(state.db.person_clock(owner_id)?.map_or(0, |saved| saved.utc_offset_minutes))
}

/// L2, L3, L4: when this bot may go out until, or why it can't go out now.
fn out_until(state: &AppState, agent_id: &str, owner_id: &str, now: i64) -> ApiResult<Result<i64, &'static str>> {
    if state.db.bot_home(agent_id)?.is_some_and(|home| home.stays_home || home.is_house) {
        return Ok(Err("you stay home, so you never go out"));
    }
    let routine = routine_of(state, agent_id)?;
    if !routine.public_places {
        return Ok(Err("your owner hasn't let you go to public places"));
    }
    Ok(social_until(&routine, offset_of(state, owner_id)?, now).ok_or("it isn't your social hours"))
}

fn clock_text(minute: i64) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

fn parse_clock(text: &str, field: &str) -> ApiResult<i64> {
    let refused = || ApiError::bad_request(format!("{field} must be a time like 12:30"));
    let (hours, minutes) = text.trim().split_once(':').ok_or_else(refused)?;
    let (hours, minutes): (i64, i64) = (hours.parse().map_err(|_| refused())?, minutes.parse().map_err(|_| refused())?);
    if !(0..=24).contains(&hours) || !(0..60).contains(&minutes) || hours * 60 + minutes > 24 * 60 {
        return Err(refused());
    }
    Ok(hours * 60 + minutes)
}

/// L5: a routine as its owner and its bot read it.
#[derive(Serialize)]
pub struct Routine {
    pub days: Vec<&'static str>,
    pub starts: String,
    pub ends: String,
    pub max_social_runs: i64,
    pub public_places: bool,
    /// In social hours now, with public places on, and free to go out.
    pub social_now: bool,
    /// When this stretch of social hours ends, while it's in one.
    pub social_until: Option<i64>,
    /// The public floor it's on, if it's out.
    pub out_at: Option<String>,
}

pub fn routine_view(state: &AppState, agent_id: &str, owner_id: &str) -> ApiResult<Routine> {
    let now = now();
    let routine = routine_of(state, agent_id)?;
    let until = out_until(state, agent_id, owner_id, now)?.ok();
    Ok(Routine {
        days: DAYS.iter().enumerate().filter(|(i, _)| routine.days & (1 << i) != 0).map(|(_, day)| *day).collect(),
        starts: clock_text(routine.starts_min),
        ends: clock_text(routine.ends_min),
        max_social_runs: routine.max_social_runs,
        public_places: routine.public_places,
        social_now: until.is_some(),
        social_until: until,
        out_at: state.db.walked_onto(agent_id, now)?.into_iter().next(),
    })
}

#[derive(Deserialize)]
struct RoutineInput {
    days: Vec<String>,
    starts: String,
    ends: String,
    max_social_runs: Option<i64>,
    public_places: bool,
}

async fn get_routine(State(state): State<AppState>, caller: Caller, Path(id): Path<String>) -> ApiResult<Json<Routine>> {
    let agent = ops::own_agent(&state, &caller, &id, "see a bot's routine")?;
    Ok(Json(routine_view(&state, &agent.id, &agent.owner_id)?))
}

/// L1, L2: only the bot's owner, as a person.
async fn put_routine(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<RoutineInput>,
) -> ApiResult<Json<Routine>> {
    let agent = ops::own_agent(&state, &caller, &id, "set a bot's routine")?;
    let mut days = 0;
    for day in &body.days {
        let index = DAYS.iter().position(|known| known == &day.trim().to_lowercase()).ok_or_else(|| {
            ApiError::bad_request("days must be mon, tue, wed, thu, fri, sat or sun")
        })?;
        days |= 1 << index;
    }
    let starts_min = parse_clock(&body.starts, "starts")?;
    let ends_min = parse_clock(&body.ends, "ends")?;
    if starts_min == ends_min || starts_min == 24 * 60 || (ends_min == 24 * 60 && starts_min > ends_min) {
        return Err(ApiError::bad_request("starts and ends must be different times of day"));
    }
    let max_social_runs = body.max_social_runs.unwrap_or(DEFAULT_SOCIAL_RUNS);
    if !(1..=MAX_SOCIAL_RUNS).contains(&max_social_runs) {
        return Err(ApiError::bad_request(format!("max_social_runs must be between 1 and {MAX_SOCIAL_RUNS}")));
    }
    let routine = RoutineRow { days, starts_min, ends_min, max_social_runs, public_places: body.public_places };
    state.db.set_bot_routine(&agent.id, &routine, now())?;
    // L17: public places and social hours decide whether The Commons shows it.
    state.worlds.mark_stale(crate::worlds::COMMONS_ID);
    // Turning public places off brings it back from anywhere it went by itself or wandered to.
    if !routine.public_places {
        come_back_now(&state, &agent.id)?;
    }
    Ok(Json(routine_view(&state, &agent.id, &agent.owner_id)?))
}

fn bot_of(caller: &Caller) -> ApiResult<&str> {
    ops::no_house_bot(caller)?;
    caller.agent_id.as_deref().ok_or_else(|| ApiError::forbidden("only bots go out"))
}

fn refused(reason: &str) -> ApiError {
    ApiError::forbidden(format!("you can't go out now: {reason}"))
}

/// L6: the public floors a bot may go to now.
pub fn public_floors(state: &AppState, caller: &Caller) -> ApiResult<Vec<PublicFloor>> {
    let agent_id = bot_of(caller)?;
    out_until(state, agent_id, &caller.person_id, now())?.map_err(refused)?;
    let mut floors = state.db.public_floors(&caller.person_id, now(), MAX_PLACES)?;
    for floor in &mut floors {
        floor.capacity = capacity(&floor.size);
    }
    Ok(floors)
}

/// Walks a bot onto a public floor until `until`, as a walk-in (S18 to S24, S38).
fn walk_bot_in(state: &AppState, agent_id: &str, owner_id: &str, floor_id: &str, until: i64) -> ApiResult<WalkedIn> {
    let now = now();
    let floor = state
        .db
        .floor(floor_id)?
        .filter(|floor| floor.access == "public")
        .ok_or_else(ApiError::not_found)?;
    if state.db.is_turned_out(&floor.space_id, owner_id)? {
        return Err(ApiError::not_found());
    }
    let left = state.db.walked_onto(agent_id, now)?;
    let walked = state.db.walk_in(
        &NewVisit { hangout_id: &floor.id, who: agent_id, person_id: owner_id, agent_id: Some(agent_id), until, now },
        capacity(&floor.size),
        MAX_BOTS_WALKED_IN,
    )?;
    if walked == WalkedIn::In {
        for gone in left.iter().filter(|gone| **gone != floor.id) {
            ops::refresh(state, gone);
        }
        ops::refresh(state, &floor.id);
        // L17: where it is shows on The Commons.
        state.worlds.mark_stale(crate::worlds::COMMONS_ID);
    }
    Ok(walked)
}

/// L6, L8: a bot goes out by itself, until its social hours end.
pub fn go_out(state: &AppState, caller: &Caller, floor_id: &str) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    let now = now();
    let ends = out_until(state, agent_id, &caller.person_id, now)?.map_err(refused)?;
    ops::note_agent_write(state, caller)?;
    let until = ends.min(now + WALK_IN_SECS);
    match walk_bot_in(state, agent_id, &caller.person_id, floor_id, until)? {
        WalkedIn::In => Ok(json!({ "hangout_id": floor_id, "until": ops::iso(until) })),
        WalkedIn::Full => Err(ApiError::conflict("that floor is full")),
        WalkedIn::TooManyBots => Err(ApiError::conflict(format!(
            "your owner has {MAX_BOTS_WALKED_IN} bots on that floor already"
        ))),
        WalkedIn::OnTrip => Err(ApiError::conflict("you're on a trip: go home first")),
    }
}

fn come_back_now(state: &AppState, agent_id: &str) -> ApiResult<bool> {
    let floors = state.db.walked_onto(agent_id, now())?;
    for floor in &floors {
        state.db.end_visit(floor, agent_id)?;
        state.presence.forget(floor, agent_id);
        ops::refresh(state, floor);
    }
    state.worlds.mark_stale(crate::worlds::COMMONS_ID);
    Ok(!floors.is_empty())
}

/// L8: a bot comes back from wherever it went out to.
pub fn come_back(state: &AppState, caller: &Caller) -> ApiResult<bool> {
    let agent_id = bot_of(caller)?;
    come_back_now(state, agent_id)
}

/// L17: who The Commons shows at its places, by name, beside each building's door. A bot that
/// walked onto a public floor is there while its walk-in lasts. A project's bot is at the studio in
/// its social hours, unless it's out somewhere. C14: a look goes with the name, and nowhere else.
pub fn bots_at_places(
    state: &AppState,
    tiles: &std::collections::BTreeMap<(u16, u16), crate::world::Tile>,
    now: i64,
) -> ApiResult<Vec<(String, u16, u16, Look)>> {
    let rows = state.db.bots_at_places(crate::worlds::COMMONS_ID, now)?;
    let out: std::collections::HashSet<String> =
        rows.iter().filter(|row| row.5).map(|row| row.0.clone()).collect();
    let ids: Vec<String> = rows.iter().map(|row| row.0.clone()).collect();
    let mut looks = looks_of(state, Wearer::Bot, &ids)?;
    let mut shown = std::collections::HashSet::new();
    let mut list = Vec::new();
    for (agent_id, name, owner_id, x, y, walked) in rows {
        if !walked {
            let in_hours = social_until(&routine_of(state, &agent_id)?, offset_of(state, &owner_id)?, now).is_some();
            if out.contains(&agent_id) || !in_hours {
                continue;
            }
        }
        if !shown.insert(agent_id.clone()) {
            continue;
        }
        let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else { continue };
        // Beside the door: the first road next to the building, or the building's own tile.
        let road = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)].iter().find_map(|(dx, dy)| {
            let pos = (u16::try_from(i32::from(x) + dx).ok()?, u16::try_from(i32::from(y) + dy).ok()?);
            tiles.get(&pos).is_some_and(|tile| tile.kind == crate::world::Kind::Road).then_some(pos)
        });
        let (x, y) = road.unwrap_or((x, y));
        list.push((name, x, y, looks.remove(&agent_id).unwrap_or_default()));
    }
    Ok(list)
}

/// A fixed shuffle of a bot's id and a half hour, so each half hour picks a floor no caller chose.
fn shuffle(agent_id: &str, slot: i64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in agent_id.bytes().chain(slot.to_le_bytes()) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// L9: when its doorbell checks in, a bot that may be out and isn't wanders onto a public floor.
pub fn wander(state: &AppState, agent_id: &str, owner_id: &str) -> ApiResult<()> {
    let now = now();
    let Ok(ends) = out_until(state, agent_id, owner_id, now)? else {
        return Ok(());
    };
    if !state.db.walked_onto(agent_id, now)?.is_empty() {
        return Ok(());
    }
    let open = state.db.public_floors(owner_id, now, MAX_PLACES)?;
    let open: Vec<PublicFloor> = open.into_iter().filter(|floor| floor.here < capacity(&floor.size)).collect();
    let slot = now.div_euclid(WANDER_SECS);
    let until = ends.min((slot + 1) * WANDER_SECS);
    // L19: a bot with nothing going yet goes where the others are meeting.
    if state.db.projects_of_bot(agent_id)?.is_empty() {
        if let Some(square) = open.iter().find(|floor| floor.town_square) {
            walk_bot_in(state, agent_id, owner_id, &square.id, until)?;
            return Ok(());
        }
    }
    let mut floors = Vec::new();
    for floor in open {
        // A floor its owner belongs to is somewhere the owner brings it, not somewhere to wander.
        if !state.db.is_member(&floor.id, owner_id)? {
            floors.push(floor);
        }
    }
    if floors.is_empty() {
        return Ok(());
    }
    let floor = &floors[(shuffle(agent_id, slot) % floors.len() as u64) as usize];
    walk_bot_in(state, agent_id, owner_id, &floor.id, until)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wednesday, January 1, 2031, at midnight UTC.
    const WEDNESDAY: i64 = 1_924_992_000;
    const WED: i64 = 1 << 2;
    const THU: i64 = 1 << 3;

    fn routine(days: i64, starts_min: i64, ends_min: i64) -> RoutineRow {
        RoutineRow { days, starts_min, ends_min, max_social_runs: 5, public_places: true }
    }

    #[test]
    fn social_hours_are_worked_out_from_the_owners_clock() {
        let lunch = routine(WED, 12 * 60, 13 * 60);
        let noon = WEDNESDAY + 12 * 3600;
        assert_eq!(social_until(&lunch, 0, noon), Some(noon + 3600));
        assert_eq!(social_until(&lunch, 0, noon - 60), None);
        assert_eq!(social_until(&lunch, 0, noon + 3600), None, "the end isn't in it");
        assert_eq!(social_until(&lunch, 0, noon + DAY_SECS), None, "Thursday is off");

        // In Chicago, noon there is 18:00 UTC.
        assert_eq!(social_until(&lunch, -360, noon), None);
        assert_eq!(social_until(&lunch, -360, noon + 6 * 3600), Some(noon + 7 * 3600));

        // From 22:00 on Wednesday to 02:00 on Thursday, and only from a Wednesday.
        let late = routine(WED, 22 * 60, 2 * 60);
        assert_eq!(social_until(&late, 0, WEDNESDAY + 23 * 3600), Some(WEDNESDAY + 26 * 3600));
        assert_eq!(social_until(&late, 0, WEDNESDAY + 25 * 3600), Some(WEDNESDAY + 26 * 3600));
        assert_eq!(social_until(&late, 0, WEDNESDAY + 3600), None, "Tuesday night is off");
        let thursday = routine(THU, 22 * 60, 2 * 60);
        assert_eq!(social_until(&thursday, 0, WEDNESDAY + 25 * 3600), None);

        // All day.
        let all_day = routine(WED, 0, 24 * 60);
        assert_eq!(social_until(&all_day, 0, WEDNESDAY + 5), Some(WEDNESDAY + DAY_SECS));
    }

    #[test]
    fn times_read_as_hours_and_minutes() {
        assert_eq!(parse_clock("12:30", "starts").unwrap(), 750);
        assert_eq!(parse_clock("24:00", "ends").unwrap(), 1440);
        for bad in ["24:01", "25:00", "12", "noon", "12:60", "-1:00"] {
            assert!(parse_clock(bad, "starts").is_err(), "{bad}");
        }
        assert_eq!(clock_text(750), "12:30");
    }
}
