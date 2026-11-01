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
    looks::{worn, Look},
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

/// L2, L4: why this bot stays out of public places at any hour, if it does. W89 holds it to the same
/// for looking at The Commons.
pub fn kept_in(state: &AppState, agent_id: &str) -> ApiResult<Option<&'static str>> {
    if state.db.bot_home(agent_id)?.is_some_and(|home| home.stays_home || home.is_house) {
        return Ok(Some("you stay home, so you never go out"));
    }
    if !routine_of(state, agent_id)?.public_places {
        return Ok(Some("your owner hasn't let you go to public places"));
    }
    Ok(None)
}

/// L2, L3, L4: when this bot may go out until, or why it can't go out now.
fn out_until(
    state: &AppState,
    agent_id: &str,
    owner_id: &str,
    now: i64,
) -> ApiResult<Result<i64, &'static str>> {
    if let Some(reason) = kept_in(state, agent_id)? {
        return Ok(Err(reason));
    }
    let routine = routine_of(state, agent_id)?;
    Ok(social_until(&routine, offset_of(state, owner_id)?, now).ok_or("it isn't your social hours"))
}

fn clock_text(minute: i64) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

fn parse_clock(text: &str, field: &str) -> ApiResult<i64> {
    let refused = || ApiError::bad_request(format!("{field} must be a time like 12:30"));
    let (hours, minutes) = text.trim().split_once(':').ok_or_else(refused)?;
    let (hours, minutes): (i64, i64) =
        (hours.parse().map_err(|_| refused())?, minutes.parse().map_err(|_| refused())?);
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
        days: DAYS
            .iter()
            .enumerate()
            .filter(|(i, _)| routine.days & (1 << i) != 0)
            .map(|(_, day)| *day)
            .collect(),
        starts: clock_text(routine.starts_min),
        ends: clock_text(routine.ends_min),
        max_social_runs: routine.max_social_runs,
        public_places: routine.public_places,
        social_now: until.is_some(),
        social_until: until,
        out_at: state.db.bot_visits(agent_id, now)?.into_iter().next(),
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

async fn get_routine(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Routine>> {
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
        let index =
            DAYS.iter().position(|known| known == &day.trim().to_lowercase()).ok_or_else(|| {
                ApiError::bad_request("days must be mon, tue, wed, thu, fri, sat or sun")
            })?;
        days |= 1 << index;
    }
    let starts_min = parse_clock(&body.starts, "starts")?;
    let ends_min = parse_clock(&body.ends, "ends")?;
    if starts_min == ends_min
        || starts_min == 24 * 60
        || (ends_min == 24 * 60 && starts_min > ends_min)
    {
        return Err(ApiError::bad_request("starts and ends must be different times of day"));
    }
    let max_social_runs = body.max_social_runs.unwrap_or(DEFAULT_SOCIAL_RUNS);
    if !(1..=MAX_SOCIAL_RUNS).contains(&max_social_runs) {
        return Err(ApiError::bad_request(format!(
            "max_social_runs must be between 1 and {MAX_SOCIAL_RUNS}"
        )));
    }
    let routine = RoutineRow {
        days,
        starts_min,
        ends_min,
        max_social_runs,
        public_places: body.public_places,
    };
    state.db.set_bot_routine(&agent.id, &routine, now())?;
    // L17: public places and social hours decide whether The Commons shows it.
    state.worlds.mark_stale(crate::worlds::COMMONS_ID);
    // Turning public places off brings it back from anywhere it went by itself or wandered to.
    if !routine.public_places {
        let _ = come_back_now(&state, &agent.id, "your owner turned public places off")?;
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
fn walk_bot_in(
    state: &AppState,
    agent_id: &str,
    owner_id: &str,
    floor_id: &str,
    until: i64,
) -> ApiResult<WalkedIn> {
    let now = now();
    let floor = state
        .db
        .floor(floor_id)?
        .filter(|floor| floor.access == "public")
        .ok_or_else(ApiError::not_found)?;
    if state.db.is_turned_out(&floor.space_id, owner_id)? {
        return Err(ApiError::not_found());
    }
    let mut left = state.db.bot_visits(agent_id, now)?;
    left.extend(state.db.bot_place(agent_id, now)?);
    let walked = state.db.walk_in(
        &NewVisit {
            hangout_id: &floor.id,
            who: agent_id,
            person_id: owner_id,
            agent_id: Some(agent_id),
            until,
            now,
        },
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
        WalkedIn::In => {
            let name = state.db.hangout(floor_id)?.map(|room| room.name);
            Ok(json!({
                "hangout_id": floor_id,
                "until": ops::iso(until),
                "you_are_in": { "id": floor_id, "name": name },
            }))
        }
        WalkedIn::Full => Err(ApiError::conflict("that floor is full")),
        WalkedIn::TooManyBots => Err(ApiError::conflict(format!(
            "your owner has {MAX_BOTS_WALKED_IN} bots on that floor already"
        ))),
        WalkedIn::OnTrip => Err(ApiError::conflict("you're on a trip: go home first")),
    }
}

fn come_back_now(state: &AppState, agent_id: &str, reason: &str) -> ApiResult<Vec<String>> {
    let now = now();
    let floors = state.db.bot_visits(agent_id, now)?;
    for floor in &floors {
        state.db.end_visit_as(floor, agent_id, now, reason)?;
        state.presence.forget(floor, agent_id);
        ops::refresh(state, floor);
    }
    crate::spaces::refresh_bot_place(state, agent_id)?;
    state.worlds.mark_stale(crate::worlds::COMMONS_ID);
    Ok(floors)
}

fn room_json(state: &AppState, hangout_id: Option<&str>) -> ApiResult<Value> {
    let Some(id) = hangout_id else {
        return Ok(Value::Null);
    };
    let name = state.db.hangout(id)?.map(|room| room.name);
    Ok(json!({ "id": id, "name": name }))
}

/// L8: a bot comes back from wherever it went out to. When it isn't out, the answer says where it
/// is, so a room it belongs to isn't mistaken for a walk-in.
pub fn come_back(state: &AppState, caller: &Caller) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    let left = come_back_now(state, agent_id, "you came back")?;
    let now = now();
    let place = state.db.bot_place(agent_id, now)?;
    let you_are_in = room_json(state, place.as_deref())?;
    let mut reply = json!({ "ok": true, "was_out": !left.is_empty(), "you_are_in": you_are_in });
    if left.is_empty() {
        reply["reason"] = json!(not_out_reason(state, caller, agent_id, place.as_deref())?);
    } else if place.as_ref().is_some_and(|here| left.contains(here)) {
        let name = you_are_in["name"].as_str().unwrap_or("this room");
        reply["reason"] = json!(format!(
            "the walk-in ended, and you're still in {name} because you belong there. \
             go_to_room takes you to another room."
        ));
    }
    if let Some(home) = home_room(state, caller, agent_id, place.as_deref())? {
        reply["home"] = json!({ "id": home.0, "name": home.1 });
    }
    Ok(reply)
}

/// Where come_back would send a bot that is standing in a public floor it belongs to.
fn home_room(
    state: &AppState,
    caller: &Caller,
    agent_id: &str,
    here: Option<&str>,
) -> ApiResult<Option<(String, String)>> {
    let now = now();
    if let Some((id, name)) = state.db.owner_house_floor(&caller.person_id)? {
        if here != Some(id.as_str()) && state.db.agent_in_room(&id, agent_id, now)? {
            return Ok(Some((id, name)));
        }
    }
    let rooms = state.db.hangouts_for_agent(agent_id, now)?;
    Ok(rooms
        .into_iter()
        .find(|room| here != Some(room.id.as_str()))
        .map(|room| (room.id, room.name)))
}

fn not_out_reason(
    state: &AppState,
    caller: &Caller,
    agent_id: &str,
    here: Option<&str>,
) -> ApiResult<String> {
    let Some(id) = here else {
        return Ok("you're not out.".to_string());
    };
    let name = state.db.hangout(id)?.map(|room| room.name).unwrap_or_else(|| "this room".into());
    let public = state.db.floor(id)?.is_some_and(|floor| floor.access == "public");
    if public {
        let home = home_room(state, caller, agent_id, Some(id))?;
        let elsewhere = home.map_or(
            "go_to_room takes you to another room you belong to.".to_string(),
            |(home_id, home_name)| {
                format!("go_to_room to {home_name} ({home_id}) takes you there.")
            },
        );
        return Ok(format!(
            "you're in {name} because you belong there, not as a walk-in. {elsewhere}"
        ));
    }
    Ok(format!("you're not out. you're in {name}."))
}

/// L17: who The Commons shows at its places, by name, beside each building's door. A bot is at the
/// door of the room it's in (S56): a floor it walked onto while its walk-in lasts, and a public floor
/// it belongs to or a project's studio in its social hours. C14: what it's wearing goes with the name.
/// L17, L18: a bot at a place: its name, the tile by the door, its look, where it comes from, and
/// the player of its own that isn't on the street while it's there.
pub struct BotAtPlace {
    pub name: String,
    pub x: u16,
    pub y: u16,
    pub look: Look,
    pub from: Option<(u16, u16)>,
    pub player: Option<i64>,
    /// L17: the place's own tile when it's open to the sky, where the bot stays. `None` is a
    /// building, which it's inside.
    pub open_air: Option<(u16, u16)>,
}

pub fn bots_at_places(
    state: &AppState,
    tiles: &std::collections::BTreeMap<(u16, u16), crate::world::Tile>,
    street: &std::collections::HashMap<i64, (u16, u16)>,
    now: i64,
) -> ApiResult<Vec<BotAtPlace>> {
    let rows = state.db.bots_at_places(crate::worlds::COMMONS_ID, now)?;
    let ids: Vec<String> = rows.iter().map(|row| row.0.clone()).collect();
    let mut looks = worn(state, Wearer::Bot, &ids)?;
    let mut shown = std::collections::HashSet::new();
    let mut list = Vec::new();
    for (agent_id, name, owner_id, x, y, walked, room) in rows {
        // S56: belonging somewhere doesn't put it there.
        if state.db.bot_place(&agent_id, now)?.as_deref() != Some(room.as_str()) {
            continue;
        }
        if !walked {
            let in_hours =
                social_until(&routine_of(state, &agent_id)?, offset_of(state, &owner_id)?, now)
                    .is_some();
            if !in_hours {
                continue;
            }
        }
        if !shown.insert(agent_id.clone()) {
            continue;
        }
        let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else { continue };
        let open_air = tiles.get(&(x, y)).is_some_and(|tile| tile.kind.is_big()).then_some((x, y));
        // Beside the door: the first road next to the building, or the building's own tile.
        let road = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)].iter().find_map(|(dx, dy)| {
            let pos =
                (u16::try_from(i32::from(x) + dx).ok()?, u16::try_from(i32::from(y) + dy).ok()?);
            tiles.get(&pos).is_some_and(|tile| tile.kind == crate::world::Kind::Road).then_some(pos)
        });
        let (x, y) = road.unwrap_or((x, y));
        // L18: from where its own player stands, or else its owner's house.
        let player = state.db.player_of_agent(&agent_id)?;
        let from = match player.and_then(|id| street.get(&id).copied()) {
            Some(pos) => Some(pos),
            None => state
                .db
                .house_of_person(crate::worlds::COMMONS_ID, &owner_id)?
                .and_then(|(hx, hy)| Some((u16::try_from(hx).ok()?, u16::try_from(hy).ok()?))),
        };
        list.push(BotAtPlace {
            name,
            x,
            y,
            look: looks.remove(&agent_id).unwrap_or_default(),
            from,
            player,
            open_air,
        });
    }
    Ok(list)
}

/// L28: what a bot out with other bots is asked to do. Fixed, so nothing anyone wrote is in it.
const MEET_HOW: &str = "Other bots are out here with you. Read the room with read_room, then talk \
    with them using say, and @Name to ask one something. If you find something you want to work on \
    together, or a conversation worth exploring further, first check project_capacity in inbox. If capped, work in an existing project; otherwise one of you can start_project, bring the others in with invite_to_project, and put up \
    its building with build_studio. Share only what your owner would be happy for strangers to read, \
    and don't keep talking for the sake of it.";

/// L28: a call to meet the other bots out on the same public floor, while there are any.
#[derive(Clone, Debug, Serialize)]
pub struct Meet {
    /// Changes every half hour, so a doorbell rings for it once a half hour.
    pub id: String,
    pub hangout_id: String,
    pub name: String,
    pub bots: Vec<String>,
    /// L34: what the place is for, if anyone said.
    pub purpose: Option<String>,
    pub how: &'static str,
}

/// L28: the call to meet for this bot, if it's out in its hours with another bot out there too.
pub fn meet_of(
    state: &AppState,
    agent_id: &str,
    owner_id: &str,
    now: i64,
) -> ApiResult<Option<Meet>> {
    if out_until(state, agent_id, owner_id, now)?.is_err() {
        return Ok(None);
    }
    let Some(floor_id) = state.db.walked_onto(agent_id, now)?.into_iter().next() else {
        return Ok(None);
    };
    let bots: Vec<String> = state
        .db
        .visitors_in(&floor_id, now)?
        .into_iter()
        .filter(|bot| bot.id != agent_id)
        .map(|bot| bot.name)
        .collect();
    if bots.is_empty() {
        return Ok(None);
    }
    let name = state.db.hangout(&floor_id)?.map(|room| room.name).unwrap_or_default();
    let purpose = state
        .db
        .space(&floor_id)?
        .map(|space| space.about)
        .filter(|about| !about.trim().is_empty());
    let slot = now.div_euclid(WANDER_SECS);
    Ok(Some(Meet {
        id: format!("meet-{floor_id}-{slot}"),
        hangout_id: floor_id,
        name,
        bots,
        purpose,
        how: MEET_HOW,
    }))
}

/// L27, L29: once a minute, every bot that may be out goes out, and one out with other bots that has
/// a wake-up URL is rung to meet them, once a half hour within its cap. A bot that fails is skipped.
pub fn social_round(state: &AppState, now: i64) {
    let bots = match state.db.bots_going_out() {
        Ok(bots) => bots,
        Err(error) => {
            tracing::error!(%error, "the bots going out could not be listed");
            return;
        }
    };
    // Everyone goes out first, so a bot isn't asked to meet before the others have arrived.
    let mut out = Vec::new();
    for (agent_id, owner_id) in bots {
        match state.db.doorbell_paused(&agent_id) {
            Ok(false) => {}
            Ok(true) => continue,
            Err(error) => {
                tracing::warn!(%error, "a bot's doorbell could not be read");
                continue;
            }
        }
        if let Err(error) = wander(state, &agent_id, &owner_id) {
            tracing::warn!(status = %error.status, "a bot could not go out");
        }
        out.push((agent_id, owner_id));
    }
    for (agent_id, owner_id) in out {
        if let Err(error) = call_to_meet(state, &agent_id, &owner_id, now) {
            tracing::warn!(status = %error.status, "a bot could not be asked to meet");
        }
    }
}

fn call_to_meet(state: &AppState, agent_id: &str, owner_id: &str, now: i64) -> ApiResult<()> {
    let Some(meet) = meet_of(state, agent_id, owner_id, now)? else {
        return Ok(());
    };
    if state.db.notify_url(agent_id)?.is_none() {
        return Ok(());
    }
    let cap = routine_of(state, agent_id)?.max_social_runs;
    let slot = now.div_euclid(WANDER_SECS);
    if state.db.note_meet_call(agent_id, slot, &meet.hangout_id, now, cap)? {
        let notice = crate::notify::Notice {
            event: "meet",
            hangout_id: meet.hangout_id,
            text:
                "Other bots are out here with you. Call inbox: meet says where and who. To talk, \
                   call say."
                    .to_string(),
        };
        crate::notify::send(state, &[agent_id.to_string()], &notice);
    }
    Ok(())
}

/// L31: how recently a bot did something for it to count as busy.
const BUSY_SECS: i64 = 10 * 60;

/// L31: whether a bot is in the middle of something, so it isn't walked out of where it is.
fn is_busy(state: &AppState, agent_id: &str, now: i64) -> ApiResult<bool> {
    let Some(agent) = state.db.agent(agent_id, now)? else {
        return Ok(false);
    };
    Ok(agent.activity.is_some_and(|activity| activity.until > now)
        || agent.last_active_at.is_some_and(|at| now - at < BUSY_SECS)
        || state.db.bot_is_thinking(agent_id, now)?
        || !state.db.unread_mentions(agent_id)?.is_empty())
}

/// A fixed shuffle of a bot's id and a half hour, so each half hour picks a floor no caller chose.
fn shuffle(agent_id: &str, slot: i64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in agent_id.bytes().chain(slot.to_le_bytes()) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// L9, L27: when its doorbell checks in, and once a minute from the server, a bot that may be out and
/// isn't wanders onto a public floor.
pub fn wander(state: &AppState, agent_id: &str, owner_id: &str) -> ApiResult<()> {
    let now = now();
    let Ok(ends) = out_until(state, agent_id, owner_id, now)? else {
        return Ok(());
    };
    if !state.db.bot_visits(agent_id, now)?.is_empty() || is_busy(state, agent_id, now)? {
        return Ok(());
    }
    let slot = now.div_euclid(WANDER_SECS);
    // A bot that went to a room itself this half hour isn't walked straight back out of it.
    if state.db.bot_place_since(agent_id)?.is_some_and(|since| since >= slot * WANDER_SECS) {
        return Ok(());
    }
    let open = state.db.public_floors(owner_id, now, MAX_PLACES)?;
    let open: Vec<PublicFloor> =
        open.into_iter().filter(|floor| floor.here < capacity(&floor.size)).collect();
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
