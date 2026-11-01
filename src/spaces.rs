//! Spaces: a person's house, their third spaces, the floors in them, and walking in to a public one
//! (docs/spaces.md). A floor is an ordinary hangout, so chat, plans and bots need nothing from here.
//! This is who runs a space, what a floor can be set to, and the walk-in, who `auth` and `ops` treat
//! as they treat a guest on a Porch: that is what keeps a floor's past from anyone who walks in.

use std::{collections::HashMap, sync::Mutex};

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    api,
    auth::{self, Caller},
    db::{Floor, FloorChange, NewFloor, NewVisit, Person, PublicFloor, Space, WalkedIn},
    error::{ApiError, ApiResult},
    god_mode, items, now,
    ops::{self, MAX_ABOUT_CHARS, MAX_NAME_CHARS},
    worlds::COMMONS_ID,
    AppState,
};

/// S3.
pub const MAX_THIRD_SPACES: i64 = 3;
/// S4.
pub const MAX_FLOORS: i64 = 5;
/// S22: long enough for an evening, short enough that a floor doesn't fill with people who left.
pub const WALK_IN_SECS: i64 = 12 * 60 * 60;
/// S24: however many bots someone makes, this is as many as they can use to fill a floor.
pub const MAX_BOTS_WALKED_IN: i64 = 3;
/// S28.
const MAX_LISTED: i64 = 50;
/// What a room is told when a floor opens.
const FLOOR_OPENED: &str =
    "This floor is public now. Anyone signed in can walk in and read what's said from here on.";
/// S13: a size and how many fit.
const SIZES: [(&str, i64); 3] = [("small", 8), ("medium", 50), ("big", 150)];
const DEFAULT_CAPACITY: i64 = 50;
/// W82: the room behind the town square, which has space for everyone who wants to meet.
const TOWN_SQUARE: &str = "Town Square";
const TOWN_SQUARE_SIZE: &str = "big";
/// S14: listed words, so nothing typed reaches the page's drawing.
pub const PAINTS: [&str; 9] =
    ["dusk", "sage", "plum", "clay", "butter", "ink", "sky", "haunted", "harvest"];
pub const FLOORINGS: [&str; 7] = ["oak", "walnut", "tile", "carpet", "grass", "crypt", "leaves"];
/// The look every floor starts with, as the table's defaults have it. I30 goes back to these.
pub const PLAIN_PAINT: &str = "dusk";
pub const PLAIN_FLOORING: &str = "oak";
/// What an owner can leave out. The table stays, because a bot that's busy sits there.
pub const FURNITURE: [&str; 7] = ["bar", "arcade", "plants", "rug", "beanbags", "couch", "desk"];
/// S37: the doors a person can try in a minute.
const MAX_DOORS_PER_MINUTE: u32 = 30;
/// S41: a page looks at least every 30 seconds, so this forgives one late look and no more.
pub const PRESENT_SECS: i64 = 45;
/// S41: however many people look at however many floors, this is all that's remembered.
pub const MAX_SIGHTINGS: usize = 4096;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/spaces", get(my_spaces))
        .route("/api/spaces/public", get(public_floors))
        .route("/api/spaces/door", post(go_through_door))
        .route("/api/places/{x}/{y}", get(place_details))
        .route("/api/hangouts/{id}/away", post(away))
        .route("/api/places/{x}/{y}/requests", post(ask_to_let_in))
        .route("/api/place-requests", get(place_requests))
        .route("/api/place-requests/{id}/answer", post(answer_place_request))
        .route("/api/spaces/{id}", get(space_view).put(change_space).delete(close_space))
        .route("/api/spaces/{id}/floors", post(add_floor))
        .route("/api/spaces/{id}/people/{person_id}/turn-out", post(turn_out).delete(let_back))
        .route("/api/floors/{id}", put(change_floor).delete(remove_floor))
        .route("/api/floors/{id}/move", post(move_floor))
        .route("/api/floors/{id}/walk-in", post(walk_in))
        .route("/api/floors/{id}/walk-out", post(walk_out))
        .route("/api/floors/{id}/close", post(keeper_close))
}

// What the pages are shown

#[derive(Debug, Serialize)]
pub struct SpaceAndFloors {
    pub space: Space,
    pub floors: Vec<Floor>,
    /// Whether its building is standing on the map (S30).
    pub on_map: bool,
}

#[derive(Debug, Serialize)]
struct Limits {
    third_spaces: i64,
    floors: i64,
}

#[derive(Debug, Serialize)]
struct SizeChoice {
    size: &'static str,
    capacity: i64,
}

#[derive(Debug, Serialize)]
struct Choices {
    sizes: Vec<SizeChoice>,
}

#[derive(Debug, Serialize)]
pub struct MySpaces {
    house: SpaceAndFloors,
    third: Vec<SpaceAndFloors>,
    limits: Limits,
    choices: Choices,
}

/// Where a room is, for the room page. Null for a Town Square, Living Room or Porch.
#[derive(Debug, Serialize)]
pub struct Place {
    pub space: Space,
    pub floor: Floor,
    /// The floors this caller can reach, lowest first (S11).
    pub floors: Vec<Floor>,
    pub capacity: i64,
    pub here: i64,
    /// Whether this caller can change it (S7, S8).
    pub runs_it: bool,
}

#[derive(Debug, Serialize)]
struct FloorDetail {
    floor: Floor,
    invite_url: String,
    here: i64,
    capacity: i64,
}

#[derive(Debug, Serialize)]
struct SpacePerson {
    #[serde(flatten)]
    person: Person,
    walked_in: bool,
}

/// What whoever runs a space sees of it.
#[derive(Debug, Serialize)]
struct SpaceView {
    space: Space,
    floors: Vec<FloorDetail>,
    people: Vec<SpacePerson>,
    turned_out: Vec<Person>,
}

// Who runs what

/// S7, S8: the owner, or once they've left, any member of the floor the space began with. Never a
/// bot.
pub(crate) fn runs(state: &AppState, caller: &Caller, space: &Space) -> ApiResult<bool> {
    Ok(!caller.is_agent()
        && state.db.is_member(&space.id, &caller.person_id)?
        && (space.owner_id == caller.person_id
            || !state.db.is_member(&space.id, &space.owner_id)?))
}

/// The space, if this caller runs it. S7: anyone in it is told only its owner can, and anyone else
/// gets the same answer as for a space that doesn't exist.
fn run_space(state: &AppState, caller: &Caller, space_id: &str, action: &str) -> ApiResult<Space> {
    ops::no_house_bot(caller)?;
    let space = state.db.space(space_id)?.ok_or_else(ApiError::not_found)?;
    if runs(state, caller, &space)? {
        return Ok(space);
    }
    for floor in state.db.floors_of(&space.id)? {
        if state.in_room(caller, &floor.id)? {
            return Err(ApiError::forbidden(format!("only the space's owner can {action}")));
        }
    }
    Err(ApiError::not_found())
}

fn run_floor(
    state: &AppState,
    caller: &Caller,
    floor_id: &str,
    action: &str,
) -> ApiResult<(Space, Floor)> {
    let floor = state.db.floor(floor_id)?.ok_or_else(ApiError::not_found)?;
    Ok((run_space(state, caller, &floor.space_id, action)?, floor))
}

/// Tells every open page in a space to reload, for a change that shows on all of its floors.
fn refresh_space(state: &AppState, floors: &[Floor]) {
    for floor in floors {
        ops::refresh(state, &floor.id);
    }
}

pub fn capacity(size: &str) -> i64 {
    SIZES.iter().find(|(name, _)| *name == size).map_or(DEFAULT_CAPACITY, |(_, fits)| *fits)
}

// Starting out

/// How long "'s Studio" is, which is what naming a space after someone adds to their name.
const NAMED_AFTER_CHARS: usize = 9;

/// S2: a person's house, built if they have none. Doing it here rather than at sign-up gives one to
/// accounts from before spaces, and to nobody who never looks.
fn house_of(state: &AppState, caller: &Caller) -> ApiResult<Space> {
    let name: String = caller.name.chars().take(MAX_NAME_CHARS - NAMED_AFTER_CHARS).collect();
    Ok(state.db.ensure_house(
        &auth::new_id(),
        &format!("{name}'s House"),
        &auth::new_invite_code(),
        &caller.person_id,
        now(),
    )?)
}

fn with_floors(state: &AppState, space: Space) -> ApiResult<SpaceAndFloors> {
    Ok(SpaceAndFloors {
        floors: state.db.floors_of(&space.id)?,
        on_map: stands_on_map(state, &space)?,
        space,
    })
}

async fn my_spaces(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<MySpaces>> {
    ops::require_human(&caller, "list their spaces")?;
    let house = with_floors(&state, house_of(&state, &caller)?)?;
    let third = state
        .db
        .third_spaces_of(&caller.person_id)?
        .into_iter()
        .map(|space| with_floors(&state, space))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(MySpaces {
        house,
        third,
        limits: Limits { third_spaces: MAX_THIRD_SPACES, floors: MAX_FLOORS },
        choices: Choices {
            sizes: SIZES
                .iter()
                .map(|(size, capacity)| SizeChoice { size, capacity: *capacity })
                .collect(),
        },
    }))
}

/// S28: for people, because a bot walks in only when its owner sends it, and these are other
/// people's words.
async fn public_floors(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "list public floors")?;
    let mut floors: Vec<PublicFloor> =
        state.db.public_floors(&caller.person_id, now(), MAX_LISTED)?;
    for floor in &mut floors {
        floor.capacity = capacity(&floor.size);
    }
    Ok(Json(json!({ "floors": floors })))
}

// The door between a building and its rooms (S30 to S37, S47, S48). This is the one place a world touches a
// room (W60), and it goes one way: nothing here changes the map, and the world never reads this.

/// What stands on a plot: its kind and the person whose it is.
type Building = (String, Option<String>);

/// S33: whether this building is still the way into this space. A row in `space_plots` proves
/// nothing alone, because after a bulldoze or a wipe someone else's building can stand on that tile.
fn is_door_to(state: &AppState, building: &Building, space: &Space) -> ApiResult<bool> {
    // W82: a square opens only the room marked as the town square, and nothing else opens that one.
    Ok(building.1.as_deref() == Some(space.owner_id.as_str())
        && (building.0 == "house") == (space.kind == "house")
        && (building.0 == "square") == state.db.is_town_square_room(&space.id)?)
}

/// The building that's the way into this space now, if one is.
fn building_of(state: &AppState, space: &Space) -> ApiResult<Option<Building>> {
    let Some((x, y)) = state.db.plot_of(&space.id, COMMONS_ID)? else {
        return Ok(None);
    };
    match state.db.building_at(COMMONS_ID, x, y)? {
        Some(building) if is_door_to(state, &building, space)? => Ok(Some(building)),
        _ => Ok(None),
    }
}

fn stands_on_map(state: &AppState, space: &Space) -> ApiResult<bool> {
    Ok(building_of(state, space)?.is_some())
}

/// W72: one place on The Commons for a signed-in person: whose it is, and how each of their own
/// bots can get into its rooms.
async fn place_details(
    State(state): State<AppState>,
    caller: Caller,
    Path((x, y)): Path<(i64, i64)>,
) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "ask about a place: only people have bots of their own")?;
    let place = place_at(&state, x, y)?;
    let owner = match &place.owner_id {
        Some(id) => state.db.person(id)?.map(|person| person.name),
        None => None,
    };
    let floors = match &place.space {
        Some(space) => state.db.floors_of(&space.id)?,
        None => Vec::new(),
    };
    let now = now();
    let mut bots = Vec::new();
    for agent in state.db.agents_of(&caller.person_id, now)? {
        if state.db.bot_home(&agent.id)?.is_some_and(|home| home.is_house) {
            continue;
        }
        let rooms: Vec<String> =
            state.db.hangouts_for_agent(&agent.id, now)?.into_iter().map(|room| room.id).collect();
        let walked = state.db.walked_onto(&agent.id, now)?;
        // S56: the one room it's in. Belonging to others doesn't put it inside them.
        let at = state.db.bot_place(&agent.id, now)?;
        let asked = match &place.space {
            Some(space) => state
                .db
                .place_request_for(&space.id, &agent.id, now - REQUEST_SECS)?
                .map(|(_, status)| status),
            None => None,
        };
        let access = if floors.iter().any(|floor| walked.contains(&floor.id)) {
            "visiting"
        } else if floors.iter().any(|floor| at.as_ref() == Some(&floor.id)) {
            "in"
        } else if floors.iter().any(|floor| rooms.contains(&floor.id)) {
            "belongs"
        } else if floors.iter().any(|floor| floor.access == "public") {
            "can_walk_in"
        } else if asked.as_deref() == Some("waiting") {
            "asked"
        } else if asked.as_deref() == Some("no") {
            "declined"
        } else {
            "needs_invite"
        };
        bots.push(json!({ "id": agent.id, "name": agent.name, "access": access }));
    }
    Ok(Json(json!({
        "kind": place.kind,
        "owner": owner,
        "yours": place.owner_id.as_deref() == Some(caller.person_id.as_str()),
        // Whether anyone signed in can walk in, so the map knows whether to offer to go in.
        "open": floors.iter().any(|floor| floor.access == "public"),
        "bots": bots,
    })))
}

/// S50, S52: how long a request waits, and how long a no lasts.
const REQUEST_SECS: i64 = 7 * 86_400;
/// S50: open requests one person can have.
const MAX_OPEN_REQUESTS: i64 = 20;

/// A place on The Commons: its kind, its owner, and its space if it still stands as that space's door.
struct PlaceAt {
    kind: String,
    owner_id: Option<String>,
    space: Option<Space>,
}

fn place_at(state: &AppState, x: i64, y: i64) -> ApiResult<PlaceAt> {
    // W91: any tile of a big place is that place.
    let (x, y) = state.db.place_anchor_at(COMMONS_ID, x, y)?;
    let (kind, owner_id) =
        state.db.building_at(COMMONS_ID, x, y)?.ok_or_else(ApiError::not_found)?;
    let building = (kind.clone(), owner_id.clone());
    let space = match state.db.space_on_plot(COMMONS_ID, x, y)? {
        Some(space) if is_door_to(state, &building, &space)? => Some(space),
        _ => None,
    };
    Ok(PlaceAt { kind, owner_id, space })
}

#[derive(Deserialize)]
struct AskIn {
    agent_id: String,
}

/// S49, S50: a person asks to let one of their own bots into someone else's place.
async fn ask_to_let_in(
    State(state): State<AppState>,
    caller: Caller,
    Path((x, y)): Path<(i64, i64)>,
    Json(body): Json<AskIn>,
) -> ApiResult<Json<Value>> {
    ops::no_house_bot(&caller)?;
    ops::require_human(&caller, "ask to let a bot in: your owner asks")?;
    let agent = ops::own_agent(&state, &caller, &body.agent_id, "ask to let a bot in")?;
    if state.db.bot_home(&agent.id)?.is_some_and(|home| home.stays_home || home.is_house) {
        return Err(ApiError::conflict(format!("{} stays home", agent.name)));
    }
    let place = place_at(&state, x, y)?;
    if place.owner_id.as_deref() == Some(caller.person_id.as_str()) {
        return Err(ApiError::conflict("this is your own place: bring your bot in from inside"));
    }
    let space = place.space.ok_or_else(|| {
        ApiError::conflict("nobody has opened this place yet, so there's nowhere to let a bot in")
    })?;
    // S27: it's a 404 to them, so the owner hears nothing from someone they turned out.
    if state.db.is_turned_out(&space.id, &caller.person_id)? {
        return Err(ApiError::not_found());
    }
    let now = now();
    let floors = state.db.floors_of(&space.id)?;
    let rooms: Vec<String> =
        state.db.hangouts_for_agent(&agent.id, now)?.into_iter().map(|room| room.id).collect();
    if floors.iter().any(|floor| rooms.contains(&floor.id)) {
        return Err(ApiError::conflict(format!("{} is already in", agent.name)));
    }
    match state.db.place_request_for(&space.id, &agent.id, now - REQUEST_SECS)? {
        Some((id, status)) if status == "waiting" => {
            return Ok(Json(json!({ "id": id, "status": "waiting" })))
        }
        Some(_) => {
            return Err(ApiError::conflict("the owner said no: you can ask again in a week"))
        }
        None => {}
    }
    let id = auth::new_id();
    if !state.db.insert_place_request(
        &id,
        &space.id,
        &agent.id,
        &caller.person_id,
        now,
        now - REQUEST_SECS,
        MAX_OPEN_REQUESTS,
    )? {
        return Err(ApiError::conflict(format!(
            "you have {MAX_OPEN_REQUESTS} requests waiting already"
        )));
    }
    if let Some(owner_id) = &place.owner_id {
        let notice = crate::notify::Notice {
            event: "place.asked",
            hangout_id: space.id.clone(),
            text: format!(
                "{} asks to let their bot {} into {}.",
                caller.name, agent.name, space.name
            ),
        };
        crate::notify::send(&state, std::slice::from_ref(owner_id), &notice);
    }
    Ok(Json(json!({ "id": id, "status": "waiting" })))
}

/// S51: the requests waiting on this person's places.
async fn place_requests(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "see requests to come in")?;
    let now = now();
    let mut list = Vec::new();
    for (id, space_id, place, bot, asker, at) in
        state.db.place_requests_for_owner(&caller.person_id, now - REQUEST_SECS)?
    {
        let kind = match state.db.space(&space_id)? {
            Some(space) => building_of(&state, &space)?.map(|(kind, _)| kind),
            None => None,
        };
        list.push(json!({ "id": id, "place": place, "kind": kind, "bot": bot, "asked_by": asker, "asked_at": at }));
    }
    Ok(Json(json!({ "requests": list })))
}

#[derive(Deserialize)]
struct Answer {
    response: String,
}

/// S51: the place's owner says yes or no. A yes brings the bot into the first floor.
async fn answer_place_request(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<Answer>,
) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "answer a request to come in")?;
    let now = now();
    let (space_id, agent_id, asked_by) =
        state.db.waiting_place_request(&id, now - REQUEST_SECS)?.ok_or_else(ApiError::not_found)?;
    let space = state
        .db
        .space(&space_id)?
        .filter(|space| space.owner_id == caller.person_id)
        .ok_or_else(ApiError::not_found)?;
    let status = match body.response.as_str() {
        "yes" => "yes",
        "no" => "no",
        _ => return Err(ApiError::bad_request("response must be yes or no")),
    };
    // S12: the floor it began with, because whoever runs a space chooses which floor is lowest.
    let floor = state.db.floor(&space.id)?.ok_or_else(ApiError::not_found)?;
    // S13, S27: a yes is a way of joining, so it's no way round either of them. R20: its owner may have
    // told it to stay home since they asked. Refused, it still waits.
    if status == "yes" {
        check_can_join(&state, &asked_by, &floor.id)?;
        if state.db.bot_home(&agent_id)?.is_some_and(|home| home.stays_home || home.is_house) {
            return Err(ApiError::conflict("that bot stays home now"));
        }
    }
    if !state.db.answer_place_request(&id, status, now)? {
        return Err(ApiError::conflict("that request was just answered"));
    }
    if status == "yes" {
        state.db.add_agent_to_room(&floor.id, &agent_id)?;
        state.db.add_member(&floor.id, &asked_by, now)?;
        let name =
            state.db.agent(&agent_id, now)?.map_or_else(|| "a bot".to_string(), |agent| agent.name);
        ops::announce(&state, &floor.id, &format!("{} let {name} in.", caller.name))?;
        ops::refresh(&state, &floor.id);
    }
    Ok(Json(json!({ "ok": true, "status": status })))
}

/// S54: the page closed this room, so its person is out of it at once, and it reloads.
async fn away(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "leave a room's presence: only people are present")?;
    ops::require_room(&state, &caller, &id)?;
    if state.presence.has_seen(&id, &caller.person_id, now()) {
        state.presence.forget(&id, &caller.person_id);
        ops::refresh(&state, &id);
    }
    Ok(Json(json!({ "ok": true })))
}

/// S48: whether this space's building is a café or the town square (W82), whose owner needs no code
/// to open its floors.
fn is_cafe(state: &AppState, space: &Space) -> ApiResult<bool> {
    // W92: every place but a house is open unless its owner closes it, so its owner needs no code.
    Ok(building_of(state, space)?.is_some_and(|(kind, _)| kind != "house"))
}

/// W92: what a place is called on the map and in its room's name, by its kind.
pub fn place_title(kind: &str) -> &'static str {
    match kind {
        "cafe" => "Café",
        "club" => "Club",
        "green" => "Park",
        "lounge" => "Lounge",
        "bar" => "Bar",
        "office" => "Office",
        "workshop" => "Workshop",
        "library" => "Library",
        "square" => "Town Square",
        "house" => "House",
        _ => "Studio",
    }
}

/// S34: the third space a new building opens onto: the oldest with no building standing, or a new
/// one. W56 keeps a person to 3 such buildings, so there's always room for the one they just placed.
/// Someone with the god mode switch, on or off, isn't held to that limit (W79). S47: a new one behind a café starts
/// open. A room that came back stays as its owner left it, so placing a café never opens a room
/// someone already has. W82: the town square has a room of its own, marked as such, and no other
/// place takes it, whatever either is called.
fn third_space_for_building(state: &AppState, caller: &Caller, kind: &str) -> ApiResult<Space> {
    let square = kind == "square";
    for space in state.db.third_spaces_of(&caller.person_id)? {
        if state.db.is_town_square_room(&space.id)? == square && !stands_on_map(state, &space)? {
            return Ok(space);
        }
    }
    let place = kind;
    let kind = place_title(kind);
    let name: String = caller.name.chars().take(MAX_NAME_CHARS - NAMED_AFTER_CHARS).collect();
    let name = if square { TOWN_SQUARE.to_string() } else { format!("{name}'s {kind}") };
    // Only a person gets here (the door refuses bots), so the switch is theirs to have, on or off.
    let most = if square || god_mode::may_use(state, &caller.person_id) {
        i64::MAX
    } else {
        MAX_THIRD_SPACES
    };
    let id = auth::new_id();
    state
        .db
        .start_third_space(&id, &name, &auth::new_invite_code(), &caller.person_id, now(), most)?
        .ok_or_else(|| ApiError::conflict(format!("you have {MAX_THIRD_SPACES} third spaces")))?;
    if square {
        state.db.mark_town_square_room(&id)?;
    }
    // W92: every place's room starts open, and dressed for what the place is.
    {
        state.db.change_floor(&FloorChange {
            id: &id,
            name: None,
            access: Some("public"),
            size: square.then_some(TOWN_SQUARE_SIZE),
            paint: None,
            flooring: None,
            keep_bots_of: &caller.person_id,
            private_only: &items::private_only(),
        })?;
        ops::announce(state, &id, FLOOR_OPENED)?;
    }
    if !square {
        items::dress_for_place(state, caller, &id, place)?;
    }
    state.db.space(&id)?.ok_or_else(ApiError::not_found)
}

#[derive(Deserialize)]
struct Door {
    x: i64,
    y: i64,
}

/// S31: for people only, so a bot's token gets what a token nobody knows gets. S32: anyone who can't
/// go in gets the same answer as for a tile with nothing on it.
async fn go_through_door(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<Door>,
) -> ApiResult<Json<Value>> {
    if caller.is_agent() {
        return Err(ApiError::unauthorized());
    }
    if !state.allow_writes(&format!("door:{}", caller.person_id), MAX_DOORS_PER_MINUTE) {
        return Err(ApiError::too_many_requests(format!(
            "slow down: {MAX_DOORS_PER_MINUTE} doors a minute"
        )));
    }
    // W91: a door on any tile of a big place is that place's door.
    let (x, y) = match state.db.building_at(COMMONS_ID, body.x, body.y)? {
        Some(_) => (body.x, body.y),
        None => state.db.place_anchor_at(COMMONS_ID, body.x, body.y)?,
    };
    let body = Door { x, y };
    let building =
        state.db.building_at(COMMONS_ID, body.x, body.y)?.ok_or_else(ApiError::not_found)?;
    let found = match state.db.space_on_plot(COMMONS_ID, body.x, body.y)? {
        Some(space) if is_door_to(&state, &building, &space)? => Some(space),
        _ => None,
    };
    let space = match found {
        Some(space) => space,
        // S34: only here does a door make anything, and only for the person whose building it is.
        None if building.1.as_deref() == Some(caller.person_id.as_str()) => {
            let space = match building.0.as_str() {
                "house" => house_of(&state, &caller)?,
                kind => third_space_for_building(&state, &caller, kind)?,
            };
            state.db.set_plot(COMMONS_ID, body.x, body.y, &space.id)?;
            state.worlds.mark_stale(COMMONS_ID);
            space
        }
        None => return Err(ApiError::not_found()),
    };
    let turned_out = state.db.is_turned_out(&space.id, &caller.person_id)?;
    for floor in state.db.floors_of(&space.id)? {
        if state.in_room(&caller, &floor.id)? || (floor.access == "public" && !turned_out) {
            // W73: its owner names it from here, so they're told which space it is.
            let space_id = (space.owner_id == caller.person_id).then(|| space.id.clone());
            return Ok(Json(json!({ "hangout_id": floor.id, "space_id": space_id })));
        }
    }
    Err(ApiError::not_found())
}

// A room's place in its space

/// What the room page shows of where it is. S11: a private floor this caller isn't on isn't here.
pub fn place(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<Option<Place>> {
    let Some(floor) = state.db.floor(hangout_id)? else {
        return Ok(None);
    };
    let space = state.db.space(&floor.space_id)?.ok_or_else(ApiError::not_found)?;
    let turned_out = state.db.is_turned_out(&space.id, &caller.person_id)?;
    let mut floors = Vec::new();
    for other in state.db.floors_of(&space.id)? {
        let open = !caller.is_agent() && !turned_out && other.access == "public";
        if open || state.in_room(caller, &other.id)? {
            floors.push(other);
        }
    }
    Ok(Some(Place {
        capacity: capacity(&floor.size),
        here: state.db.on_floor(&floor.id, now())?,
        runs_it: runs(state, caller, &space)?,
        space,
        floor,
        floors,
    }))
}

/// S13: refused when a floor has no room for one more. A room that isn't a floor has no size.
pub fn check_fits(state: &AppState, hangout_id: &str) -> ApiResult<()> {
    if let Some(floor) = state.db.floor(hangout_id)? {
        if state.db.on_floor(hangout_id, now())? >= capacity(&floor.size) {
            return Err(ApiError::conflict("this floor is full"));
        }
    }
    Ok(())
}

/// Before someone joins, however they came to. S27: nothing gets them back in after being turned out.
pub fn check_can_join(state: &AppState, person_id: &str, hangout_id: &str) -> ApiResult<()> {
    if let Some(space) = state.db.space_for_room(hangout_id)? {
        if state.db.is_turned_out(&space.id, person_id)? {
            return Err(ApiError::not_found());
        }
    }
    check_fits(state, hangout_id)
}

/// S9. True if this was the owner leaving their third space, which this has dealt with.
pub fn leave_own_space(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<bool> {
    let Some(space) =
        state.db.space_for_room(hangout_id)?.filter(|space| space.owner_id == caller.person_id)
    else {
        return Ok(false);
    };
    if space.kind == "house" {
        return Err(ApiError::forbidden("this is your house, so it stays yours"));
    }
    for floor in state.db.floors_of(&space.id)? {
        if state.db.is_member(&floor.id, &caller.person_id)? {
            state.db.remove_member(&floor.id, &caller.person_id)?;
            ops::announce(state, &floor.id, &ops::left_line(&caller.name))?;
            ops::refresh(state, &floor.id);
        }
    }
    Ok(true)
}

/// S20: a check-in pings every member and rings their bots' doorbells.
pub fn no_walk_ins(
    state: &AppState,
    caller: &Caller,
    hangout_id: &str,
    action: &str,
) -> ApiResult<()> {
    if state.db.floor(hangout_id)?.is_some() && state.guest_visit(caller, hangout_id)?.is_some() {
        return Err(ApiError::forbidden(format!("someone who walked in can't {action}")));
    }
    Ok(())
}

/// S23: a stranger typing into someone else's room gets the limit a bot gets. Members don't, and a
/// bot is already counted.
pub fn note_walk_in_write(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<()> {
    if caller.is_agent() || state.db.floor(hangout_id)?.is_none() {
        return Ok(());
    }
    if state.guest_visit(caller, hangout_id)?.is_some()
        && !state.allow_agent_write(&caller.person_id)
    {
        return Err(ApiError::too_many_requests(format!(
            "slow down: someone who walked in gets {} writes a minute",
            ops::MAX_AGENT_WRITES_PER_MINUTE
        )));
    }
    Ok(())
}

// Changing a space

fn one_of(value: &str, field: &str, choices: &[&str]) -> ApiResult<()> {
    if !choices.contains(&value) {
        return Err(ApiError::bad_request(format!(
            "{field} must be one of {}",
            choices.join(", ")
        )));
    }
    Ok(())
}

/// A body that may be left out altogether.
fn optional<T: Default>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    match body {
        Ok(Json(body)) => Ok(body),
        Err(JsonRejection::MissingJsonContentType(_)) => Ok(T::default()),
        Err(rejection) => Err(ApiError::bad_request(rejection.body_text())),
    }
}

async fn space_view(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<SpaceView>> {
    let space = run_space(&state, &caller, &id, "see this page")?;
    let now = now();
    let mut floors = Vec::new();
    for floor in state.db.floors_of(&space.id)? {
        // S8, S10: running a space gives nobody a way onto a floor they aren't on.
        let on_it = space.owner_id == caller.person_id
            || state.db.is_member(&floor.id, &caller.person_id)?;
        let code = state.db.hangout(&floor.id)?.map(|room| room.invite_code).unwrap_or_default();
        floors.push(FloorDetail {
            invite_url: if on_it { api::invite_url(&state, &code) } else { String::new() },
            here: state.db.on_floor(&floor.id, now)?,
            capacity: capacity(&floor.size),
            floor,
        });
    }
    let mut people: Vec<SpacePerson> = state
        .db
        .members_of_space(&space.id)?
        .into_iter()
        .map(|person| SpacePerson { person, walked_in: false })
        .collect();
    for person in state.db.walk_ins_of_space(&space.id, now)? {
        if !people.iter().any(|known| known.person.id == person.id) {
            people.push(SpacePerson { person, walked_in: true });
        }
    }
    Ok(Json(SpaceView { floors, people, turned_out: state.db.turned_out_of(&space.id)?, space }))
}

#[derive(Default, Deserialize)]
struct ChangeSpace {
    name: Option<String>,
    about: Option<String>,
}

async fn change_space(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<ChangeSpace>,
) -> ApiResult<Json<Space>> {
    let space = run_space(&state, &caller, &id, "change it")?;
    let name =
        body.name.as_deref().map(|name| ops::clean(name, "name", MAX_NAME_CHARS)).transpose()?;
    let about = body
        .about
        .as_deref()
        .map(|about| ops::clean_optional(Some(about), "about", MAX_ABOUT_CHARS))
        .transpose()?;
    state.db.change_space(&space, name.as_deref(), about.as_deref())?;
    refresh_space(&state, &state.db.floors_of(&space.id)?);
    // W73: its building on The Commons is called what the space is.
    state.worlds.mark_stale(COMMONS_ID);
    Ok(Json(state.db.space(&id)?.ok_or_else(ApiError::not_found)?))
}

/// S5: this deletes every floor's chat, notes and plans, so the page asks first.
async fn close_space(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let space = run_space(&state, &caller, &id, "close it")?;
    if space.kind == "house" {
        return Err(ApiError::conflict("a house can't be closed"));
    }
    let floors = state.db.floors_of(&space.id)?;
    state.db.remove_rooms(&floors.iter().map(|floor| floor.id.as_str()).collect::<Vec<_>>())?;
    // An open page finds the room gone, and its stream ends.
    refresh_space(&state, &floors);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct AddFloor {
    name: String,
}

async fn add_floor(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<AddFloor>,
) -> ApiResult<Json<Floor>> {
    let space = run_space(&state, &caller, &id, "add a floor")?;
    let name = ops::clean(&body.name, "name", MAX_NAME_CHARS)?;
    let floor = state
        .db
        .add_floor(
            &NewFloor {
                id: &auth::new_id(),
                space_id: &space.id,
                name: &name,
                invite_code: &auth::new_invite_code(),
                created_by: &caller.person_id,
                now: now(),
            },
            MAX_FLOORS,
        )?
        .ok_or_else(|| ApiError::conflict(format!("a space can have {MAX_FLOORS} floors")))?;
    refresh_space(&state, &state.db.floors_of(&space.id)?);
    Ok(Json(floor))
}

#[derive(Default, Deserialize)]
struct ChangeFloor {
    name: Option<String>,
    access: Option<String>,
    size: Option<String>,
    paint: Option<String>,
    flooring: Option<String>,
    hidden: Option<Vec<String>>,
    create_code: Option<String>,
}

async fn change_floor(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<ChangeFloor>,
) -> ApiResult<Json<Floor>> {
    let (space, floor) = run_floor(&state, &caller, &id, "change it")?;
    let name =
        body.name.as_deref().map(|name| ops::clean(name, "name", MAX_NAME_CHARS)).transpose()?;
    if let Some(access) = &body.access {
        one_of(access, "access", &["private", "public"])?;
    }
    if let Some(size) = &body.size {
        let sizes: Vec<&str> = SIZES.iter().map(|(name, _)| *name).collect();
        one_of(size, "size", &sizes)?;
    }
    if let Some(paint) = &body.paint {
        one_of(paint, "paint", &PAINTS)?;
    }
    if let Some(flooring) = &body.flooring {
        one_of(flooring, "flooring", &FLOORINGS)?;
    }
    if let Some(words) = &body.hidden {
        for word in words {
            one_of(word, "hidden", &FURNITURE)?;
        }
    }
    let opening = body.access.as_deref() == Some("public") && floor.access != "public";
    // S8, S15: opening a floor is a way onto it, and running a space gives nobody that.
    if opening
        && caller.person_id != space.owner_id
        && !state.db.is_member(&floor.id, &caller.person_id)?
    {
        return Err(ApiError::forbidden("only someone on this floor can open it"));
    }
    let closing = body.access.as_deref() == Some("private") && floor.access == "public";
    let at = now();
    // Read first: opening drops other owners' bots, and closing drops whoever walked in.
    let removed: Vec<String> = if opening {
        state
            .db
            .agents_in(&floor.id, at)?
            .into_iter()
            .filter(|agent| agent.owner_id != caller.person_id)
            .map(|agent| agent.id)
            .collect()
    } else {
        Vec::new()
    };
    let sent_home = if closing { state.db.visiting_bot_ids(&floor.id, at)? } else { Vec::new() };
    // S16: the same code as starting a hangout, so a server that gates one gates the other. S48: a
    // café doesn't ask its owner, who waited for it. Whoever runs it once they've left (S8) didn't.
    if opening && !(caller.person_id == space.owner_id && is_cafe(&state, &space)?) {
        api::check_create_code(&state, body.create_code.as_deref())?;
    }
    if let Some(words) = &body.hidden {
        items::leave_out(&state, &floor.id, words)?;
    }
    let taken_off = state
        .db
        .change_floor(&FloorChange {
            id: &floor.id,
            name: name.as_deref(),
            access: body.access.as_deref(),
            size: body.size.as_deref(),
            paint: body.paint.as_deref(),
            flooring: body.flooring.as_deref(),
            keep_bots_of: &caller.person_id,
            private_only: &items::private_only(),
        })?
        // I11: flirty items are for private floors.
        .ok_or_else(|| {
            ApiError::conflict("take the flirty items off this floor before opening it")
        })?;
    for agent_id in &removed {
        crate::events::note_place(&state, agent_id, &floor.id, "removed");
    }
    for agent_id in &sent_home {
        crate::events::note_place(&state, agent_id, &floor.id, "sent_home");
    }
    if opening {
        ops::announce(&state, &floor.id, FLOOR_OPENED)?;
        // S25: no bot goes in front of strangers without its owner choosing it.
        if !taken_off.is_empty() {
            let line = format!(
                "{} went home because this floor is public. Their owners can bring them back.",
                taken_off.join(", ")
            );
            ops::announce(&state, &floor.id, &line)?;
        }
    } else if body.access.as_deref() == Some("private") && floor.access == "public" {
        ops::announce(&state, &floor.id, "This floor is private again.")?;
    }
    refresh_space(&state, &state.db.floors_of(&space.id)?);
    Ok(Json(state.db.floor(&id)?.ok_or_else(ApiError::not_found)?))
}

#[derive(Deserialize)]
struct MoveFloor {
    direction: String,
}

async fn move_floor(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<MoveFloor>,
) -> ApiResult<Json<Value>> {
    let (space, floor) = run_floor(&state, &caller, &id, "move a floor")?;
    one_of(&body.direction, "direction", &["up", "down"])?;
    state.db.move_floor(&floor, body.direction == "up")?;
    refresh_space(&state, &state.db.floors_of(&space.id)?);
    Ok(Json(json!({ "ok": true })))
}

/// S12: this deletes the floor's chat, notes and plans, so the page asks first.
async fn remove_floor(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let (space, floor) = run_floor(&state, &caller, &id, "remove a floor")?;
    if floor.id == space.id {
        return Err(ApiError::conflict("a space can't lose the floor it began with"));
    }
    state.db.remove_rooms(&[floor.id.as_str()])?;
    ops::refresh(&state, &floor.id);
    refresh_space(&state, &state.db.floors_of(&space.id)?);
    Ok(Json(json!({ "ok": true })))
}

fn someone_in_space(state: &AppState, space: &Space, person_id: &str) -> ApiResult<Person> {
    let mut people = state.db.members_of_space(&space.id)?;
    people.extend(state.db.walk_ins_of_space(&space.id, now())?);
    people.into_iter().find(|person| person.id == person_id).ok_or_else(ApiError::not_found)
}

/// S27. Only someone in the space, so this can't be used to find out who exists.
async fn turn_out(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, person_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let space = run_space(&state, &caller, &id, "turn someone out")?;
    if person_id == caller.person_id {
        return Err(ApiError::bad_request("you can't turn yourself out"));
    }
    if person_id == space.owner_id {
        return Err(ApiError::forbidden("the owner can't be turned out of their own space"));
    }
    let person = someone_in_space(&state, &space, &person_id)?;
    let floors = state.db.floors_of(&space.id)?;
    let left = ops::left_line(&person.name);
    let answered = state.db.turn_out(&space.id, &person.id, &left, now())?;
    // They're out by now, so a plan that won't settle is no reason to say they aren't.
    for plan in &answered {
        if let Err(error) = ops::settle(&state, plan) {
            tracing::error!(status = %error.status, "a plan didn't settle after a turn-out");
        }
    }
    for floor in &floors {
        state.presence.forget(&floor.id, &person.id);
    }
    refresh_space(&state, &floors);
    Ok(Json(json!({ "ok": true })))
}

async fn let_back(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, person_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let space = run_space(&state, &caller, &id, "let someone back")?;
    state.db.let_back(&space.id, &person_id)?;
    Ok(Json(json!({ "ok": true })))
}

// Walking in

/// The floor, if this person can walk onto it. Any other floor is one that doesn't exist.
fn open_floor(state: &AppState, caller: &Caller, floor_id: &str) -> ApiResult<Floor> {
    let floor = state
        .db
        .floor(floor_id)?
        .filter(|floor| floor.access == "public")
        .ok_or_else(ApiError::not_found)?;
    if state.db.is_turned_out(&floor.space_id, &caller.person_id)? {
        return Err(ApiError::not_found());
    }
    Ok(floor)
}

#[derive(Default, Deserialize)]
struct Walker {
    agent_id: Option<String>,
}

/// S18, S24: a person walks in, or sends their own bot. A bot can't send itself.
async fn walk_in(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    body: Result<Json<Walker>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = optional(body)?;
    ops::no_house_bot(&caller)?;
    ops::require_human(&caller, "walk in: your owner sends you")?;
    let floor = open_floor(&state, &caller, &id)?;
    let is_member = state.db.is_member(&floor.id, &caller.person_id)?;
    let now = now();
    let agent = match &body.agent_id {
        Some(agent_id) => {
            let agent = ops::own_agent(&state, &caller, agent_id, "send a bot in")?;
            if state.db.bot_home(agent_id)?.is_some_and(|home| home.stays_home || home.is_house) {
                return Err(ApiError::conflict(format!("{} stays home", agent.name)));
            }
            if is_member {
                return Err(ApiError::conflict("you're on this floor, so bring it in instead"));
            }
            Some(agent)
        }
        // Someone on the floor already has no walking in to do.
        None if is_member => return Ok(Json(json!({ "hangout_id": floor.id, "until": null }))),
        None => None,
    };
    let who = agent.as_ref().map_or(caller.person_id.as_str(), |agent| agent.id.as_str());
    // Read first: once they've walked in, nothing says which floor they left, or which room a bot.
    let mut left = state.db.walked_onto(who, now)?;
    if let Some(agent) = &agent {
        left.extend(state.db.bot_place(&agent.id, now)?);
    }
    let walked = state.db.walk_in(
        &NewVisit {
            hangout_id: &floor.id,
            who,
            person_id: &caller.person_id,
            agent_id: agent.as_ref().map(|agent| agent.id.as_str()),
            until: now + WALK_IN_SECS,
            now,
        },
        capacity(&floor.size),
        MAX_BOTS_WALKED_IN,
    )?;
    match walked {
        WalkedIn::In => {}
        WalkedIn::Full => return Err(ApiError::conflict("this floor is full")),
        WalkedIn::TooManyBots => {
            return Err(ApiError::conflict(format!(
                "you have {MAX_BOTS_WALKED_IN} bots on this floor already"
            )))
        }
        WalkedIn::OnTrip => {
            return Err(ApiError::conflict("that bot is on a trip: bring it home first"))
        }
    }
    for gone in left.iter().filter(|gone| **gone != floor.id) {
        ops::refresh(&state, gone);
    }
    ops::refresh(&state, &floor.id);
    // L17: a bot with public places on shows at the place it walked into.
    if let Some(agent) = &agent {
        crate::events::note_place(&state, &agent.id, &floor.id, "moved");
        state.worlds.mark_stale(COMMONS_ID);
    }
    Ok(Json(json!({ "hangout_id": floor.id, "until": now + WALK_IN_SECS })))
}

async fn walk_out(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    body: Result<Json<Walker>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = optional(body)?;
    ops::no_house_bot(&caller)?;
    ops::require_human(&caller, "walk out")?;
    let floor = state.db.floor(&id)?.ok_or_else(ApiError::not_found)?;
    let who = match &body.agent_id {
        Some(agent_id) => ops::own_agent(&state, &caller, agent_id, "bring a bot back")?.id,
        None => caller.person_id.clone(),
    };
    let at = now();
    let visiting = body.agent_id.is_some() && state.db.bot_visits(&who, at)?.contains(&floor.id);
    let reason = match body.agent_id.is_some() {
        true => "your owner brought you back",
        false => "you left",
    };
    state.db.end_visit_as(&floor.id, &who, at, reason)?;
    state.presence.forget(&floor.id, &who);
    ops::refresh(&state, &floor.id);
    refresh_bot_place(&state, &who)?;
    if visiting {
        crate::events::note_place(&state, &who, &floor.id, "sent_home");
    }
    state.worlds.mark_stale(COMMONS_ID);
    Ok(Json(json!({ "ok": true })))
}

/// S56: the room a bot is in now reloads, because it just got back to it.
pub fn refresh_bot_place(state: &AppState, agent_id: &str) -> ApiResult<()> {
    if let Some(room) = state.db.bot_place(agent_id, now())? {
        ops::refresh(state, &room);
    }
    Ok(())
}

/// S58, S59: puts a bot in a room, off any floor it walked onto. On a trip it stays on the trip, and
/// this is where it is once it's home. Both rooms reload.
pub fn place_bot(state: &AppState, agent_id: &str, hangout_id: &str) -> ApiResult<()> {
    let now = now();
    let was = state.db.bot_place(agent_id, now)?;
    let dest = state
        .db
        .hangout(hangout_id)?
        .map(|room| room.name)
        .unwrap_or_else(|| "another room".to_string());
    let reason = format!("you went to {dest}");
    for floor in state.db.bot_visits(agent_id, now)? {
        if floor != hangout_id {
            state.db.end_visit_as(&floor, agent_id, now, &reason)?;
            state.presence.forget(&floor, agent_id);
        }
    }
    state.db.set_bot_place(agent_id, hangout_id, now)?;
    for room in was.iter().map(String::as_str).chain([hangout_id]) {
        ops::refresh(state, room);
    }
    // L17: where it is shows on The Commons.
    state.worlds.mark_stale(COMMONS_ID);
    Ok(())
}

/// S57, S58: a bot takes itself to a room it can be in, before it does something there. A person
/// has nowhere to go, and neither has a bot that's already there.
pub fn go_to(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<()> {
    let Some(agent_id) = caller.agent_id.as_deref() else {
        return Ok(());
    };
    let now = now();
    if state.db.bot_place(agent_id, now)?.as_deref() == Some(hangout_id) {
        // S61: it's doing something here, so it stays.
        state.db.note_bot_here(agent_id, hangout_id, now)?;
        return Ok(());
    }
    if state.db.on_trip_away_from(hangout_id, agent_id, now)? {
        return Err(ApiError::conflict("you're on a trip: go home first"));
    }
    place_bot(state, agent_id, hangout_id)
}

/// S57: `go_to_room`, for a bot going somewhere with nothing to do there yet.
pub fn go_to_room(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<Value> {
    ops::no_house_bot(caller)?;
    if !caller.is_agent() {
        return Err(ApiError::forbidden("only bots go to rooms"));
    }
    let hangout = ops::require_room(state, caller, hangout_id)?;
    ops::note_agent_write(state, caller)?;
    go_to(state, caller, &hangout.id)?;
    Ok(json!({ "hangout_id": hangout.id, "name": hangout.name }))
}

/// S29: anyone but the keeper finds no such floor.
async fn keeper_close(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Floor>> {
    if caller.is_agent() || state.config.keeper_id.as_deref() != Some(caller.person_id.as_str()) {
        return Err(ApiError::not_found());
    }
    let floor = state
        .db
        .floor(&id)?
        .filter(|floor| floor.access == "public")
        .ok_or_else(ApiError::not_found)?;
    let visitors = state.db.visiting_bot_ids(&floor.id, now())?;
    state.db.change_floor(&FloorChange {
        id: &floor.id,
        name: None,
        access: Some("private"),
        size: None,
        paint: None,
        flooring: None,
        keep_bots_of: &caller.person_id,
        private_only: &[],
    })?;
    ops::announce(&state, &floor.id, "This floor was closed to the public.")?;
    for agent_id in &visitors {
        crate::events::note_place(&state, agent_id, &floor.id, "sent_home");
    }
    refresh_space(&state, &state.db.floors_of(&floor.space_id)?);
    Ok(Json(state.db.floor(&id)?.ok_or_else(ApiError::not_found)?))
}

// Who is in the room (S41 to S46). This is memory and it stays on the floor. Nothing here goes to a
// world, a list or another room, and a person is on the street only by opening the map (W52).

/// When each person last looked at each floor, by (floor, person).
#[derive(Default)]
pub struct Presence {
    sightings: Mutex<HashMap<(String, String), i64>>,
}

impl Presence {
    fn sightings(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), i64>> {
        self.sightings.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Notes a look at a floor. S41: anything too old to count goes first, and if it's still full of
    /// people who really are here, the oldest of them goes, so a flood costs nothing but a few
    /// sightings.
    /// S53: a person is on one floor at a time, so this takes them off any other. Returns the floors
    /// they were on, which should reload so the people there see them go.
    pub fn saw(&self, floor_id: &str, person_id: &str, now: i64) -> Vec<String> {
        let mut sightings = self.sightings();
        sightings.retain(|_, seen| now - *seen <= PRESENT_SECS);
        let left: Vec<String> = sightings
            .keys()
            .filter(|(floor, person)| person == person_id && floor != floor_id)
            .map(|(floor, _)| floor.clone())
            .collect();
        for floor in &left {
            sightings.remove(&(floor.clone(), person_id.to_string()));
        }
        let key = (floor_id.to_string(), person_id.to_string());
        if sightings.len() >= MAX_SIGHTINGS && !sightings.contains_key(&key) {
            let oldest =
                sightings.iter().min_by_key(|(_, seen)| **seen).map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                sightings.remove(&oldest);
            }
        }
        sightings.insert(key, now);
        left
    }

    /// S53: off every floor, because they opened The Commons. Returns the floors they were on.
    pub fn forget_everywhere(&self, person_id: &str) -> Vec<String> {
        let mut sightings = self.sightings();
        let left: Vec<String> = sightings
            .keys()
            .filter(|(_, person)| person == person_id)
            .map(|(floor, _)| floor.clone())
            .collect();
        sightings.retain(|(_, person), _| person != person_id);
        left
    }

    /// Whether this person has looked at this floor lately enough to count.
    pub fn has_seen(&self, floor_id: &str, person_id: &str, now: i64) -> bool {
        let key = (floor_id.to_string(), person_id.to_string());
        self.sightings().get(&key).is_some_and(|seen| now - *seen <= PRESENT_SECS)
    }

    /// S45: gone at once, rather than when the look runs out.
    pub fn forget(&self, floor_id: &str, person_id: &str) {
        self.sightings().remove(&(floor_id.to_string(), person_id.to_string()));
    }
}

/// Notes this look, and says who else has this floor open. S42: only on a floor. S43: a bot's look
/// counts for nobody, though its caller carries its owner's id, and a bot isn't told. S45: only the
/// people the room lists right now, so anyone who has gone is gone, however they went.
pub fn present(
    state: &AppState,
    caller: &Caller,
    floor: &Option<Place>,
    people: &[Person],
    now: i64,
) -> Vec<String> {
    let Some(place) = floor.as_ref().filter(|_| !caller.is_agent()) else {
        return Vec::new();
    };
    // S53: the rooms they were in reload, so the people there see them go.
    for left in state.presence.saw(&place.floor.id, &caller.person_id, now) {
        ops::refresh(state, &left);
    }
    people
        .iter()
        .filter(|person| state.presence.has_seen(&place.floor.id, &person.id, now))
        .map(|person| person.id.clone())
        .collect()
}
