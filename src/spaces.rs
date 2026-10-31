//! Spaces: a person's house, their third spaces, the floors in them, and walking in to a public one
//! (docs/spaces.md). A floor is an ordinary hangout, so chat, plans and bots need nothing from here.
//! This is who runs a space, what a floor can be set to, and the walk-in, who `auth` and `ops` treat
//! as they treat a guest on a Porch: that is what keeps a floor's past from anyone who walks in.

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
    now,
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
/// S14: listed words, so nothing typed reaches the page's drawing.
const PAINTS: [&str; 6] = ["dusk", "sage", "plum", "clay", "butter", "ink"];
const FLOORINGS: [&str; 4] = ["oak", "walnut", "tile", "carpet"];
/// What an owner can leave out. The table, couch and desk stay, because where a bot stands says what
/// it's doing.
const FURNITURE: [&str; 5] = ["bar", "arcade", "plants", "rug", "beanbags"];
/// S37: the doors a person can try in a minute.
const MAX_DOORS_PER_MINUTE: u32 = 30;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/spaces", get(my_spaces))
        .route("/api/spaces/public", get(public_floors))
        .route("/api/spaces/door", post(go_through_door))
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
    paints: &'static [&'static str],
    floorings: &'static [&'static str],
    furniture: &'static [&'static str],
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
fn runs(state: &AppState, caller: &Caller, space: &Space) -> ApiResult<bool> {
    Ok(!caller.is_agent()
        && state.db.is_member(&space.id, &caller.person_id)?
        && (space.owner_id == caller.person_id || !state.db.is_member(&space.id, &space.owner_id)?))
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
    Ok(SpaceAndFloors { floors: state.db.floors_of(&space.id)?, on_map: stands_on_map(state, &space)?, space })
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
            sizes: SIZES.iter().map(|(size, capacity)| SizeChoice { size, capacity: *capacity }).collect(),
            paints: &PAINTS,
            floorings: &FLOORINGS,
            furniture: &FURNITURE,
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

// The door between a building and its rooms (S30 to S37). This is the one place a world touches a
// room (W60), and it goes one way: nothing here changes the map, and the world never reads this.

/// What stands on a plot: its kind and the person whose it is.
type Building = (String, Option<String>);

/// S33: whether this building is still the way into this space. A row in `space_plots` proves
/// nothing alone, because after a bulldoze or a wipe someone else's building can stand on that tile.
fn is_door_to(building: &Building, space: &Space) -> bool {
    building.1.as_deref() == Some(space.owner_id.as_str())
        && (building.0 == "house") == (space.kind == "house")
}

/// The building that's the way into this space now, if one is.
fn building_of(state: &AppState, space: &Space) -> ApiResult<Option<Building>> {
    let Some((x, y)) = state.db.plot_of(&space.id, COMMONS_ID)? else {
        return Ok(None);
    };
    Ok(state.db.building_at(COMMONS_ID, x, y)?.filter(|building| is_door_to(building, space)))
}

fn stands_on_map(state: &AppState, space: &Space) -> ApiResult<bool> {
    Ok(building_of(state, space)?.is_some())
}

/// S39: whether this space's building is a café, whose owner needs no code to open its floors.
fn is_cafe(state: &AppState, space: &Space) -> ApiResult<bool> {
    Ok(building_of(state, space)?.is_some_and(|(kind, _)| kind == "cafe"))
}

/// S34: the third space a new building opens onto: the oldest with no building standing, or a new
/// one. W56 keeps a person to 3 such buildings, so there's always room for the one they just placed.
/// S38: a new one behind a café starts open. A room that came back stays as its owner left it, so
/// placing a café never opens a room someone already has.
fn third_space_for_building(state: &AppState, caller: &Caller, kind: &str) -> ApiResult<Space> {
    for space in state.db.third_spaces_of(&caller.person_id)? {
        if !stands_on_map(state, &space)? {
            return Ok(space);
        }
    }
    let kind = match kind {
        "cafe" => "Café",
        "club" => "Club",
        _ => "Studio",
    };
    let name: String = caller.name.chars().take(MAX_NAME_CHARS - NAMED_AFTER_CHARS).collect();
    let id = auth::new_id();
    state
        .db
        .start_third_space(
            &id,
            &format!("{name}'s {kind}"),
            &auth::new_invite_code(),
            &caller.person_id,
            now(),
            MAX_THIRD_SPACES,
        )?
        .ok_or_else(|| ApiError::conflict(format!("you have {MAX_THIRD_SPACES} third spaces")))?;
    if kind == "Café" {
        state.db.change_floor(&FloorChange {
            id: &id,
            name: None,
            access: Some("public"),
            size: None,
            paint: None,
            flooring: None,
            hidden: None,
            keep_bots_of: &caller.person_id,
        })?;
        ops::announce(state, &id, FLOOR_OPENED)?;
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
    let building =
        state.db.building_at(COMMONS_ID, body.x, body.y)?.ok_or_else(ApiError::not_found)?;
    let found = state
        .db
        .space_on_plot(COMMONS_ID, body.x, body.y)?
        .filter(|space| is_door_to(&building, space));
    let space = match found {
        Some(space) => space,
        // S34: only here does a door make anything, and only for the person whose building it is.
        None if building.1.as_deref() == Some(caller.person_id.as_str()) => {
            let space = match building.0.as_str() {
                "house" => house_of(&state, &caller)?,
                kind => third_space_for_building(&state, &caller, kind)?,
            };
            state.db.set_plot(COMMONS_ID, body.x, body.y, &space.id)?;
            space
        }
        None => return Err(ApiError::not_found()),
    };
    let turned_out = state.db.is_turned_out(&space.id, &caller.person_id)?;
    for floor in state.db.floors_of(&space.id)? {
        if state.in_room(&caller, &floor.id)? || (floor.access == "public" && !turned_out) {
            return Ok(Json(json!({ "hangout_id": floor.id })));
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

/// Before someone joins by an invite link. S27: a link gets nobody back in after being turned out.
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
    let Some(space) = state
        .db
        .space_for_room(hangout_id)?
        .filter(|space| space.owner_id == caller.person_id)
    else {
        return Ok(false);
    };
    if space.kind == "house" {
        return Err(ApiError::forbidden("this is your house, so it stays yours"));
    }
    for floor in state.db.floors_of(&space.id)? {
        if state.db.is_member(&floor.id, &caller.person_id)? {
            state.db.remove_member(&floor.id, &caller.person_id)?;
            ops::announce(state, &floor.id, &format!("{} left the hangout.", caller.name))?;
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
    if state.guest_visit(caller, hangout_id)?.is_some() && !state.allow_agent_write(&caller.person_id) {
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
        return Err(ApiError::bad_request(format!("{field} must be one of {}", choices.join(", "))));
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
        let code = state.db.hangout(&floor.id)?.map(|room| room.invite_code).unwrap_or_default();
        floors.push(FloorDetail {
            invite_url: api::invite_url(&state, &code),
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
    let name = body.name.as_deref().map(|name| ops::clean(name, "name", MAX_NAME_CHARS)).transpose()?;
    let about = body
        .about
        .as_deref()
        .map(|about| ops::clean_optional(Some(about), "about", MAX_ABOUT_CHARS))
        .transpose()?;
    state.db.change_space(&space, name.as_deref(), about.as_deref())?;
    refresh_space(&state, &state.db.floors_of(&space.id)?);
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
    let name = body.name.as_deref().map(|name| ops::clean(name, "name", MAX_NAME_CHARS)).transpose()?;
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
    let hidden = match &body.hidden {
        Some(words) => {
            for word in words {
                one_of(word, "hidden", &FURNITURE)?;
            }
            // Kept in the list's own order, once each, so nothing can be stored twice.
            let kept: Vec<&str> =
                FURNITURE.iter().copied().filter(|item| words.iter().any(|word| word == item)).collect();
            Some(kept.join(","))
        }
        None => None,
    };
    let opening = body.access.as_deref() == Some("public") && floor.access != "public";
    // S16: the same code as starting a hangout, so a server that gates one gates the other. S39: a
    // café doesn't ask its owner, who waited for it. Whoever runs it once they've left (S8) didn't.
    if opening && !(caller.person_id == space.owner_id && is_cafe(&state, &space)?) {
        api::check_create_code(&state, body.create_code.as_deref())?;
    }
    let taken_off = state.db.change_floor(&FloorChange {
        id: &floor.id,
        name: name.as_deref(),
        access: body.access.as_deref(),
        size: body.size.as_deref(),
        paint: body.paint.as_deref(),
        flooring: body.flooring.as_deref(),
        hidden: hidden.as_deref(),
        keep_bots_of: &caller.person_id,
    })?;
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
    state.db.turn_out(&space.id, &person.id, now())?;
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
    let walked = state.db.walk_in(
        &NewVisit {
            hangout_id: &floor.id,
            who: agent.as_ref().map_or(caller.person_id.as_str(), |agent| agent.id.as_str()),
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
    }
    ops::refresh(&state, &floor.id);
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
    state.db.end_visit(&floor.id, &who)?;
    ops::refresh(&state, &floor.id);
    Ok(Json(json!({ "ok": true })))
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
    state.db.change_floor(&FloorChange {
        id: &floor.id,
        name: None,
        access: Some("private"),
        size: None,
        paint: None,
        flooring: None,
        hidden: None,
        keep_bots_of: &caller.person_id,
    })?;
    ops::announce(&state, &floor.id, "This floor was closed to the public.")?;
    refresh_space(&state, &state.db.floors_of(&floor.space_id)?);
    Ok(Json(state.db.floor(&id)?.ok_or_else(ApiError::not_found)?))
}
