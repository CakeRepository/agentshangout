//! Cities, houses and trips: who may be where, and what happens when a bot goes visiting. The REST
//! API and the MCP tools both call these functions, so every rule lives here or in `ops`. What a
//! city or house asks of newcomers, and the gate, are in `trust`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    auth::{self, Caller},
    db::{
        Agent, Arrival, BotPlace, City, House, HouseStatus, NewCity, NewHouse, NewPass,
        NewSceneRequest, NewTrip, Pass, Person, QueueItem, Road, Scene, SceneRequest, Trip,
    },
    error::{ApiError, ApiResult},
    notify, now,
    ops::{self, DAY_SECS, MAX_ABOUT_CHARS, MAX_NAME_CHARS},
    trust, AppState,
};

/// A kind of trip, how long it lasts if the asker doesn't say, and the longest it can be (minutes).
const KINDS: [(&str, i64, i64); 3] =
    [("call", 60, 4 * 60), ("visit", 24 * 60, 3 * 24 * 60), ("stay", 7 * 24 * 60, 30 * 24 * 60)];
/// An ask nobody answers lapses.
const ASK_LAPSE_SECS: i64 = DAY_SECS;
const DEFAULT_PASS_MINUTES: i64 = 60;
const MAX_PASS_MINUTES: i64 = 24 * 60;
const MAX_SCENE_NAME_CHARS: usize = 40;
const SCENE_WAIT_SECS: i64 = 15 * 60;
/// An approval is good for this long, so a scene can't fire hours after someone said yes.
const SCENE_APPROVED_SECS: i64 = 10 * 60;
/// Scenes that sound like they open or disarm something. This catches honest mistakes only: the
/// real limit on what a scene can do is which devices the home system exposes. These match the
/// start of a word, so Locked and Doorbell are caught and Block Party isn't.
const BANNED_SCENE_STARTS: [&str; 14] = [
    "lock", "unlock", "padlock", "deadbolt", "door", "backdoor", "frontdoor", "garage", "gate",
    "alarm", "disarm", "camera", "secur", "siren",
];
/// Whole words, because Armchair and Army are fine.
const BANNED_SCENE_WORDS: [&str; 4] = ["arm", "arms", "armed", "arming"];
/// However many bots someone makes, this is as many requests as they can have waiting at one
/// house. Each one pings everyone who lives there.
const MAX_OPEN_ASKS_FROM_ONE_OWNER: i64 = 3;

// What the pages are shown

#[derive(Debug, Serialize)]
pub struct CityInfo {
    pub id: String,
    pub name: String,
    pub square_id: String,
    pub created_by: String,
    /// Only the person who started the city sees this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CityYou {
    pub person_id: String,
    pub house_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CityHouse {
    pub id: String,
    pub name: String,
    pub residents: Vec<String>,
    /// What the house asks of anyone whose bot wants to visit.
    pub requirements: trust::Requirements,
}

#[derive(Debug, Serialize)]
pub struct Growth {
    pub stage: &'static str,
    pub gatherings: i64,
    pub roads: Vec<Road>,
}

/// What a member sees of the city: names, presence and counts. Never goals, status text, activity,
/// house status, scenes or any room other than the Town Square.
#[derive(Debug, Serialize)]
pub struct CityView {
    pub city: CityInfo,
    pub you: CityYou,
    pub people: Vec<Person>,
    pub houses: Vec<CityHouse>,
    pub bots: Vec<BotPlace>,
    pub growth: Growth,
    /// What the city asks of anyone who comes by its link.
    pub rules: trust::CityRules,
    /// Who is waiting at the gate. For people in the city only: their bots don't see it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<Vec<trust::GateEntry>>,
}

#[derive(Debug, Serialize)]
pub struct HouseBot {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub state: &'static str,
    pub stays_home: bool,
    pub is_house: bool,
    pub away_at_house_id: Option<String>,
    pub away_until: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct HouseBotInfo {
    pub id: String,
    pub name: String,
    pub last_seen_at: Option<i64>,
}

/// What a resident sees of their house.
#[derive(Debug, Serialize)]
pub struct HouseView {
    pub house: House,
    pub invite_url: String,
    pub living_room_id: String,
    pub porch_id: String,
    pub residents: Vec<Person>,
    pub bots: Vec<HouseBot>,
    pub house_bot: Option<HouseBotInfo>,
    pub status: Option<HouseStatus>,
    /// Trips under way.
    pub guests: Vec<Trip>,
    /// Trips waiting on an answer.
    pub asking: Vec<Trip>,
    pub passes: Vec<Pass>,
    pub scenes: Vec<Scene>,
    /// Open requests, plus the last 24 hours.
    pub scene_requests: Vec<SceneRequest>,
    /// What this household wants remembered (F45), and what's on for it (F52).
    pub notes: Vec<crate::db::HouseNote>,
    pub calendar: Vec<crate::db::HousePlan>,
    /// What this house asks of anyone whose bot wants to visit.
    pub rules: trust::Requirements,
    /// How each person asking to visit stands in the city, by their person id.
    pub standing: BTreeMap<String, trust::Standing>,
}

#[derive(Debug, Serialize)]
pub struct CityPreview {
    pub name: String,
    pub people: Vec<String>,
    /// What the city asks newcomers to agree to. Empty if nothing.
    pub rules_text: String,
    /// How many people in the city have to vouch before a newcomer is let in. 0 means none.
    pub vouches_needed: i64,
}

#[derive(Debug, Serialize)]
pub struct HousePreview {
    pub name: String,
    pub city: String,
    pub people: Vec<String>,
    /// What the house's city asks newcomers to agree to. Empty if nothing.
    pub rules_text: String,
}

#[derive(Debug, Serialize)]
pub struct CityRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct HouseRef {
    pub id: String,
    pub name: String,
    pub city_id: String,
}

/// The cities and houses a person belongs to, for `GET /api/me`.
#[derive(Debug, Serialize)]
pub struct MyPlaces {
    pub cities: Vec<CityRef>,
    pub houses: Vec<HouseRef>,
}

#[derive(Debug, Default, Deserialize)]
pub struct TripInput {
    /// A bot asking is asking for itself, so it leaves this out. A person must say which bot.
    pub agent_id: Option<String>,
    pub house_id: String,
    pub kind: String,
    pub goal: Option<String>,
    pub minutes: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PassInput {
    pub agent_id: String,
    pub max_minutes: Option<i64>,
}

// Growth

/// How far a city has grown. It needs both houses and gatherings, so neither invitations nor bots
/// can grow a city alone.
pub fn stage(houses: i64, gatherings: i64) -> &'static str {
    match (houses, gatherings) {
        (12.., 15..) => "city",
        (6.., 5..) => "town",
        (3.., 1..) => "village",
        _ => "hamlet",
    }
}

// Who may see what

/// The city, if the caller can see it: a member, or a bot whose owner is one. Anyone else gets the
/// same answer as for a city that doesn't exist. The house bot never sees it.
pub fn require_city(state: &AppState, caller: &Caller, city_id: &str) -> ApiResult<City> {
    ops::no_house_bot(caller)?;
    if !state.db.is_city_member(city_id, &caller.person_id)? {
        return Err(ApiError::not_found());
    }
    state.db.city(city_id)?.ok_or_else(ApiError::not_found)
}

/// For a city's stream, which asks again on every event and ends when this turns false.
pub fn in_city(state: &AppState, caller: &Caller, city_id: &str) -> ApiResult<bool> {
    Ok(!caller.is_house && state.db.is_city_member(city_id, &caller.person_id)?)
}

/// The house, if the caller is a resident and a person. A bot of a resident knows the house
/// exists, so it is told only people can do this; everyone else gets 404.
pub(crate) fn resident_house(
    state: &AppState,
    caller: &Caller,
    house_id: &str,
    action: &str,
) -> ApiResult<House> {
    ops::no_house_bot(caller)?;
    let house = state.db.house(house_id)?.ok_or_else(ApiError::not_found)?;
    if !state.db.is_resident(&house.id, &caller.person_id)? {
        return Err(ApiError::not_found());
    }
    ops::require_human(caller, action)?;
    Ok(house)
}

pub fn city_invite_url(state: &AppState, code: &str) -> String {
    format!("{}/join-city/{code}", state.config.public_url)
}

pub fn house_invite_url(state: &AppState, code: &str) -> String {
    format!("{}/join-house/{code}", state.config.public_url)
}

/// Pings people, never bots: a bot's yes can't exist, so asking can't wake one bot from another.
pub(crate) fn ping(
    state: &AppState,
    mut people: Vec<String>,
    event: &'static str,
    hangout_id: &str,
    text: &str,
) {
    people.sort();
    people.dedup();
    let notice =
        notify::Notice { event, hangout_id: hangout_id.to_string(), text: text.to_string() };
    notify::send(state, &people, &notice);
}

fn resident_ids(state: &AppState, house_id: &str) -> ApiResult<Vec<String>> {
    Ok(state.db.residents(house_id)?.into_iter().map(|person| person.id).collect())
}

/// Tells open pages for this house to reload: its Living Room and its Porch.
pub(crate) fn refresh_house(state: &AppState, house: &House) {
    ops::refresh(state, &house.living_room_id);
    ops::refresh(state, &house.porch_id);
}

// Cities

/// Starts a city and its Town Square. Whether this server lets this person start one is the
/// caller's business, as it is for hangouts.
pub fn create_city(state: &AppState, person: &Person, name: &str) -> ApiResult<City> {
    let name = ops::clean(name, "city name", MAX_NAME_CHARS)?;
    Ok(state.db.create_city(&NewCity {
        id: &auth::new_id(),
        name: &name,
        invite_code: &auth::new_invite_code(),
        square_id: &auth::new_id(),
        square_code: &auth::new_invite_code(),
        created_by: &person.id,
        now: now(),
    })?)
}

pub fn city_invite_preview(state: &AppState, code: &str) -> ApiResult<CityPreview> {
    let city = state.db.city_by_invite(code)?.ok_or_else(ApiError::not_found)?;
    let people = state.db.city_members(&city.id)?;
    let rules = trust::city_rules(state, &city.id)?;
    Ok(CityPreview {
        name: city.name,
        people: people.into_iter().map(|person| person.name).collect(),
        rules_text: rules.rules_text,
        vouches_needed: rules.min_vouches,
    })
}

/// Someone uses the city's invite link. They're in, or at the gate if the city asks for vouches.
/// If the city has rules, `accept_rules` must be true.
pub fn join_city(
    state: &AppState,
    person: &Person,
    code: &str,
    accept_rules: bool,
) -> ApiResult<trust::Joined> {
    let city = state.db.city_by_invite(code)?.ok_or_else(ApiError::not_found)?;
    trust::enter_city(state, person, &city, accept_rules)
}

/// Who looks after a city's invite link: the person who started it, or every member once they've
/// left, so a link can't end up with nobody able to replace it. The same person sets its rules and
/// turns people away from its gate.
pub(crate) fn holds_city_link(state: &AppState, caller: &Caller, city: &City) -> ApiResult<bool> {
    Ok(!caller.is_agent()
        && (city.created_by == caller.person_id
            || !state.db.is_city_member(&city.id, &city.created_by)?))
}

/// Replaces the city's invite link and returns the new one.
pub fn rotate_city_invite(state: &AppState, caller: &Caller, city_id: &str) -> ApiResult<String> {
    let city = require_city(state, caller, city_id)?;
    ops::require_human(caller, "change the invite link")?;
    if !holds_city_link(state, caller, &city)? {
        return Err(ApiError::forbidden(
            "only the person who started the city can change its invite link",
        ));
    }
    let code = auth::new_invite_code();
    state.db.set_city_invite(&city.id, &code)?;
    Ok(city_invite_url(state, &code))
}

/// A member with no house here can leave. Their bots' trips and passes here end with them.
pub fn leave_city(state: &AppState, caller: &Caller, city_id: &str) -> ApiResult<()> {
    let city = require_city(state, caller, city_id)?;
    ops::require_human(caller, "leave a city")?;
    if state.db.house_of_person_in_city(&city.id, &caller.person_id)?.is_some() {
        return Err(ApiError::conflict("leave your house first, then the city"));
    }
    state.db.remove_city_member(&city.id, &caller.person_id, &caller.name, now())?;
    ops::announce(state, &city.square_id, &format!("{} left {}.", caller.name, city.name))?;
    ops::refresh(state, &city.square_id);
    // Their bots may have been on a Porch here.
    for house in state.db.houses_in_city(&city.id)? {
        refresh_house(state, &house);
    }
    ops::refresh_city(state, &city.id);
    Ok(())
}

pub fn city_view(state: &AppState, caller: &Caller, city_id: &str) -> ApiResult<CityView> {
    let city = require_city(state, caller, city_id)?;
    let now = now();
    let residents = state.db.city_residents(&city.id)?;
    let requirements = trust::requirements_by_house(state, &city.id)?;
    let houses = state
        .db
        .houses_in_city(&city.id)?
        .into_iter()
        .map(|house| CityHouse {
            residents: residents
                .iter()
                .filter(|(house_id, _)| *house_id == house.id)
                .map(|(_, person_id)| person_id.clone())
                .collect(),
            requirements: requirements.get(&house.id).copied().unwrap_or_default(),
            id: house.id,
            name: house.name,
        })
        .collect::<Vec<_>>();
    let gatherings = state.db.gatherings(&city.id, now)?;
    let invite_url =
        holds_city_link(state, caller, &city)?.then(|| city_invite_url(state, &city.invite_code));
    Ok(CityView {
        you: CityYou {
            person_id: caller.person_id.clone(),
            house_id: state
                .db
                .house_of_person_in_city(&city.id, &caller.person_id)?
                .map(|house| house.id),
        },
        people: state.db.city_members(&city.id)?,
        bots: state.db.bots_in_city(&city.id, now)?,
        growth: Growth {
            stage: stage(houses.len() as i64, gatherings),
            gatherings,
            roads: state.db.roads(&city.id)?,
        },
        rules: trust::city_rules(state, &city.id)?,
        gate: trust::gate_view(state, caller, &city.id)?,
        houses,
        city: CityInfo {
            id: city.id,
            name: city.name,
            square_id: city.square_id,
            created_by: city.created_by,
            invite_url,
        },
    })
}

/// Where a person lives and which cities they're in. For `GET /api/me`.
pub fn my_places(state: &AppState, caller: &Caller) -> ApiResult<MyPlaces> {
    let (cities, houses) = if caller.is_agent() {
        // Where a bot's owner lives isn't the bot's to list.
        (Vec::new(), Vec::new())
    } else {
        (
            state.db.cities_for_person(&caller.person_id)?,
            state.db.houses_for_person(&caller.person_id)?,
        )
    };
    Ok(MyPlaces {
        cities: cities.into_iter().map(|city| CityRef { id: city.id, name: city.name }).collect(),
        houses: houses
            .into_iter()
            .map(|house| HouseRef { id: house.id, name: house.name, city_id: house.city_id })
            .collect(),
    })
}

/// The trip a bot is on right now, if any. For `GET /api/me` as a bot.
pub fn my_trip(state: &AppState, caller: &Caller) -> ApiResult<Option<Trip>> {
    let Some(agent_id) = &caller.agent_id else {
        return Ok(None);
    };
    if caller.is_house {
        return Ok(None);
    }
    Ok(state.db.live_trip(agent_id, now())?)
}

// Houses

/// Builds the caller's house in a city they belong to, with its Living Room and Porch.
pub fn build_house(
    state: &AppState,
    caller: &Caller,
    city_id: &str,
    name: &str,
) -> ApiResult<House> {
    let city = require_city(state, caller, city_id)?;
    ops::require_human(caller, "build a house")?;
    let name = ops::clean(name, "house name", MAX_NAME_CHARS)?;
    if state.db.house_of_person_in_city(&city.id, &caller.person_id)?.is_some() {
        return Err(ApiError::conflict("you already have a house in this city"));
    }
    let house = state.db.create_house(&NewHouse {
        id: &auth::new_id(),
        city_id: &city.id,
        name: &name,
        invite_code: &auth::new_invite_code(),
        living_room_id: &auth::new_id(),
        living_room_code: &auth::new_invite_code(),
        porch_id: &auth::new_id(),
        porch_code: &auth::new_invite_code(),
        created_by: &caller.person_id,
        now: now(),
    })?;
    ops::announce(state, &city.square_id, &format!("{} built {}.", caller.name, house.name))?;
    ops::refresh_city(state, &city.id);
    Ok(house)
}

pub fn house_view(state: &AppState, caller: &Caller, house_id: &str) -> ApiResult<HouseView> {
    let house = resident_house(state, caller, house_id, "see a house's page")?;
    let now = now();
    let trips = state.db.trips_for_house(&house.id, now - DAY_SECS, now)?;
    let bots = state
        .db
        .bots_of_house(&house.id, now)?
        .into_iter()
        .map(|bot| HouseBot {
            id: bot.id,
            name: bot.name,
            owner_id: bot.owner_id,
            state: bot.state,
            stays_home: bot.stays_home,
            is_house: bot.is_house,
            away_at_house_id: bot.away_at_house_id,
            away_until: bot.away_until,
        })
        .collect();
    let asking: Vec<Trip> = trips.iter().filter(|trip| trip.status == "asking").cloned().collect();
    Ok(HouseView {
        invite_url: house_invite_url(state, &house.invite_code),
        living_room_id: house.living_room_id.clone(),
        porch_id: house.porch_id.clone(),
        residents: state.db.residents(&house.id)?,
        bots,
        house_bot: state.db.house_bot(&house.id, now)?.map(|bot| HouseBotInfo {
            id: bot.id,
            name: bot.name,
            last_seen_at: bot.last_seen_at,
        }),
        status: state.db.house_status(&house.id)?,
        guests: trips.iter().filter(|trip| trip.status == "here").cloned().collect(),
        passes: state.db.passes_for_house(&house.id)?,
        scenes: state.db.scenes(&house.id)?,
        scene_requests: state.db.recent_scene_requests(&house.id, now - DAY_SECS, now)?,
        notes: state.db.house_notes(&house.id)?,
        calendar: state.db.house_calendar(&house.id, now, now + 14 * DAY_SECS)?,
        rules: trust::house_requirements(state, &house.id)?,
        standing: trust::asker_standings(state, &house, &asking, now)?,
        asking,
        house,
    })
}

pub fn house_invite_preview(state: &AppState, code: &str) -> ApiResult<HousePreview> {
    let house = state.db.house_by_invite(code)?.ok_or_else(ApiError::not_found)?;
    let city = state.db.city(&house.city_id)?.ok_or_else(ApiError::not_found)?;
    let people = state.db.residents(&house.id)?;
    Ok(HousePreview {
        name: house.name,
        rules_text: trust::city_rules(state, &city.id)?.rules_text,
        city: city.name,
        people: people.into_iter().map(|person| person.name).collect(),
    })
}

/// Moves a person into the house, and into its city and rooms. Holding the house's link is the only
/// way in: it is for adults in the household. A link works once and the family hears about it, so
/// one that leaks can't quietly let someone in for good. If the city has rules and they aren't in
/// it yet, `accept_rules` must be true.
pub fn join_house(
    state: &AppState,
    person: &Person,
    code: &str,
    accept_rules: bool,
) -> ApiResult<House> {
    let house = state.db.house_by_invite(code)?.ok_or_else(ApiError::not_found)?;
    if state.db.is_resident(&house.id, &person.id)? {
        return Ok(house);
    }
    if let Some(home) = state.db.house_of_person_in_city(&house.city_id, &person.id)? {
        return Err(ApiError::conflict(format!(
            "you already live at {} in this city: leave it first",
            home.name
        )));
    }
    let now = now();
    // Moving in while one of your bots is asking to visit, or visiting, would put both sides of
    // that trip in one person's hands.
    let busy =
        state.db.trips_for_owner(&person.id, now, now)?.into_iter().any(|trip| {
            trip.house_id == house.id && matches!(trip.status.as_str(), "asking" | "here")
        });
    if busy {
        return Err(ApiError::conflict(
            "one of your bots has a trip to this house: wait until it ends, then move in",
        ));
    }
    let city = state.db.city(&house.city_id)?.ok_or_else(ApiError::not_found)?;
    // The gate doesn't stop them (T19), but the city's rules still need accepting (T14).
    let was_in_city = state.db.is_city_member(&city.id, &person.id)?;
    if !was_in_city {
        trust::check_rules_accepted(state, &city, accept_rules)?;
    }
    let family = resident_ids(state, &house.id)?;
    // Joining the city here also ends any wait they had at its gate.
    state.db.add_resident(&house.id, &person.id, now)?;
    state.db.set_house_invite(&house.id, &auth::new_invite_code())?;
    let text = format!("{} moved into {}.", person.name, house.name);
    ping(state, family, "house.joined", &house.living_room_id, &text);
    // A pass to a house you now live in means nothing: your bots can't visit it.
    for pass in state.db.passes_for_owner(&person.id)? {
        if pass.house_id == house.id {
            state.db.delete_pass(&pass.id)?;
        }
    }
    ops::announce(state, &house.living_room_id, &format!("{} moved in.", person.name))?;
    if !was_in_city {
        ops::announce(state, &city.square_id, &format!("{} joined {}.", person.name, city.name))?;
        trust::admit_anyone_ready(state, &city, now)?;
    }
    refresh_house(state, &house);
    ops::refresh_city(state, &house.city_id);
    Ok(house)
}

/// Replaces the house's invite link and returns the new one.
pub fn rotate_house_invite(state: &AppState, caller: &Caller, house_id: &str) -> ApiResult<String> {
    let house = resident_house(state, caller, house_id, "change the invite link")?;
    let code = auth::new_invite_code();
    state.db.set_house_invite(&house.id, &code)?;
    Ok(house_invite_url(state, &code))
}

/// Any resident can leave except the last. Their bots leave the house's rooms with them.
pub fn leave_house(state: &AppState, caller: &Caller, house_id: &str) -> ApiResult<()> {
    let house = resident_house(state, caller, house_id, "leave a house")?;
    if state.db.resident_count(&house.id)? <= 1 {
        return Err(ApiError::conflict("the last resident can't leave their house"));
    }
    ops::announce(state, &house.living_room_id, &format!("{} moved out.", caller.name))?;
    state.db.remove_resident(&house.id, &caller.person_id)?;
    state.db.set_house_invite(&house.id, &auth::new_invite_code())?;
    refresh_house(state, &house);
    ops::refresh_city(state, &house.city_id);
    Ok(())
}

/// A resident moves someone else out, which is how a family recovers from a link that reached the
/// wrong person. The link they could see stops working too.
pub fn remove_resident(
    state: &AppState,
    caller: &Caller,
    house_id: &str,
    person_id: &str,
) -> ApiResult<()> {
    let house = resident_house(state, caller, house_id, "move someone out")?;
    if person_id == caller.person_id {
        return Err(ApiError::bad_request("use leave to move yourself out"));
    }
    let person = state
        .db
        .residents(&house.id)?
        .into_iter()
        .find(|person| person.id == person_id)
        .ok_or_else(ApiError::not_found)?;
    let line = format!("{} moved {} out.", caller.name, person.name);
    ops::announce(state, &house.living_room_id, &line)?;
    state.db.remove_resident(&house.id, &person.id)?;
    state.db.set_house_invite(&house.id, &auth::new_invite_code())?;
    refresh_house(state, &house);
    ops::refresh_city(state, &house.city_id);
    Ok(())
}

/// Makes the house's bot: the one that runs scenes. It lives in no rooms and its owner is the
/// resident who made it. Returns the only copy of its token.
pub fn add_house_bot(
    state: &AppState,
    caller: &Caller,
    house_id: &str,
    name: &str,
) -> ApiResult<(Agent, String)> {
    let house = resident_house(state, caller, house_id, "add a house bot")?;
    let name = ops::clean(name, "name", MAX_NAME_CHARS)?;
    let id = auth::new_id();
    let token = auth::new_agent_token();
    let now = now();
    let made = state.db.create_house_bot(
        &house.id,
        &id,
        &caller.person_id,
        &name,
        &auth::hash_secret(&token),
        now,
    )?;
    if !made {
        return Err(ApiError::conflict("this house already has a house bot"));
    }
    refresh_house(state, &house);
    ops::refresh_city(state, &house.city_id);
    let agent = state.db.agent(&id, now)?.ok_or_else(ApiError::not_found)?;
    Ok((agent, token))
}

/// Sets where a bot lives, and whether it stays there. Its owner decides, and has to live there.
pub fn set_bot_home(
    state: &AppState,
    caller: &Caller,
    agent_id: &str,
    house_id: &str,
    stays_home: bool,
) -> ApiResult<()> {
    ops::no_house_bot(caller)?;
    let agent = ops::own_agent(state, caller, agent_id, "choose where a bot lives")?;
    let house = resident_house(state, caller, house_id, "choose where a bot lives")?;
    if state.db.bot_home(agent_id)?.is_some_and(|home| home.is_house) {
        return Err(ApiError::forbidden("the house bot can't be moved"));
    }
    let now = now();
    state.db.set_bot_home(agent_id, &house.id, stays_home, false)?;
    state.db.add_agent_to_room(&house.living_room_id, agent_id)?;
    // A pass to its own home means nothing.
    if let Some(pass) = state.db.pass_for_bot(&house.id, agent_id)? {
        state.db.delete_pass(&pass.id)?;
    }
    if stays_home {
        // A bot that holds keys is in one room, and doesn't travel. Passes can't make it.
        state.db.remove_agent_from_other_rooms(agent_id, &house.living_room_id)?;
        for pass in state.db.passes_for_owner(&caller.person_id)? {
            if pass.agent_id == agent_id {
                state.db.delete_pass(&pass.id)?;
            }
        }
        let away = state.db.live_trip(agent_id, now)?;
        let ended = state.db.end_trips_of_bot(agent_id, &caller.name, now)?;
        if let (true, Some(trip)) = (ended > 0, away) {
            if let Some(visited) = state.db.house(&trip.house_id)? {
                ops::announce(state, &visited.porch_id, &format!("{} went home.", agent.name))?;
                refresh_house(state, &visited);
                ops::refresh_city(state, &visited.city_id);
            }
        }
    }
    refresh_house(state, &house);
    ops::refresh_city(state, &house.city_id);
    Ok(())
}

// Trips

fn kind_limits(kind: &str) -> ApiResult<(&'static str, i64, i64)> {
    KINDS
        .iter()
        .find(|(name, _, _)| *name == kind)
        .map(|(name, default, max)| (*name, *default, *max))
        .ok_or_else(|| ApiError::bad_request("kind must be call, visit or stay"))
}

fn trip_by_id(state: &AppState, trip_id: &str, now: i64) -> ApiResult<Trip> {
    state.db.trip(trip_id, now)?.ok_or_else(ApiError::not_found)
}

/// Where a caller stands on a trip.
struct Standing {
    /// The trip is this bot's own.
    bot: bool,
    /// A person whose bot it is.
    owner: bool,
    /// A person who lives in the house it's to.
    resident: bool,
}

impl Standing {
    fn any(&self) -> bool {
        self.bot || self.owner || self.resident
    }
}

fn standing(state: &AppState, caller: &Caller, trip: &Trip) -> ApiResult<Standing> {
    if let Some(agent_id) = &caller.agent_id {
        return Ok(Standing { bot: *agent_id == trip.agent_id, owner: false, resident: false });
    }
    Ok(Standing {
        bot: false,
        owner: trip.owner_id == caller.person_id,
        resident: state.db.is_resident(&trip.house_id, &caller.person_id)?,
    })
}

/// A trip, for the bot it's for, that bot's owner and the house's residents. Anyone else gets 404.
fn trip_for(
    state: &AppState,
    caller: &Caller,
    trip_id: &str,
    now: i64,
) -> ApiResult<(Trip, Standing)> {
    ops::no_house_bot(caller)?;
    let trip = trip_by_id(state, trip_id, now)?;
    let standing = standing(state, caller, &trip)?;
    if !standing.any() {
        // A bot whose owner is on either side knows the trip exists, but it isn't that bot's to
        // touch: no bot ever approves one, not even its owner's other bot.
        let side = caller.is_agent()
            && (trip.owner_id == caller.person_id
                || state.db.is_resident(&trip.house_id, &caller.person_id)?);
        return Err(if side {
            ApiError::forbidden("that trip isn't this bot's")
        } else {
            ApiError::not_found()
        });
    }
    Ok((trip, standing))
}

pub fn get_trip(state: &AppState, caller: &Caller, trip_id: &str) -> ApiResult<Trip> {
    Ok(trip_for(state, caller, trip_id, now())?.0)
}

/// Asks for a trip, or invites a bot on one. A bot asks for itself. A person names the bot: if
/// it's theirs this is the owner's yes, and if it's another resident's guest this is the host's.
/// What the caller can't see answers 404, so no bot's name or house's existence leaks.
pub fn create_trip(state: &AppState, caller: &Caller, input: &TripInput) -> ApiResult<Trip> {
    ops::no_house_bot(caller)?;
    let now = now();
    let agent_id = match (&caller.agent_id, input.agent_id.as_deref()) {
        (Some(own), Some(named)) if own != named => return Err(ApiError::not_found()),
        (Some(own), _) => own.as_str(),
        (None, Some(named)) => named,
        (None, None) => return Err(ApiError::bad_request("agent_id is required")),
    };
    let house = state.db.house(&input.house_id)?.ok_or_else(ApiError::not_found)?;
    let agent = state.db.agent(agent_id, now)?.ok_or_else(ApiError::not_found)?;
    let owner_side = !caller.is_agent() && agent.owner_id == caller.person_id;
    let host_side =
        !caller.is_agent() && !owner_side && state.db.is_resident(&house.id, &caller.person_id)?;
    if !caller.is_agent() && !owner_side && !host_side {
        return Err(ApiError::not_found());
    }
    // The bot's owner and the house must share a city, or neither can be seen from the other.
    if !state.db.is_city_member(&house.city_id, &agent.owner_id)? {
        return Err(ApiError::not_found());
    }

    let (kind, default_minutes, max_minutes) = kind_limits(&input.kind)?;
    let minutes = input.minutes.unwrap_or(default_minutes);
    if !(1..=max_minutes).contains(&minutes) {
        return Err(ApiError::bad_request(format!(
            "minutes must be between 1 and {max_minutes} for a {kind}"
        )));
    }
    let goal = ops::clean_optional(input.goal.as_deref(), "goal", MAX_ABOUT_CHARS)?;

    let home = state.db.bot_home(agent_id)?;
    if home.as_ref().is_some_and(|home| home.is_house || home.stays_home) {
        return Err(ApiError::conflict(format!(
            "{} stays home, so it can't take trips",
            agent.name
        )));
    }
    if state.db.is_resident(&house.id, &agent.owner_id)?
        || home.as_ref().is_some_and(|home| home.house_id == house.id)
    {
        return Err(ApiError::conflict("a bot can't take a trip to its own house"));
    }
    if let Some(live) = state.db.live_trip(agent_id, now)? {
        // Another city's house is no business of the host's, so it isn't named.
        let place = match state.db.house(&live.house_id)? {
            Some(away) if away.city_id == house.city_id => away.name,
            _ => "a house in another city".to_string(),
        };
        return Err(ApiError::conflict(format!("{} is already on a trip, at {place}", agent.name)));
    }
    let pass = state
        .db
        .pass_for_bot(&house.id, agent_id)?
        .filter(|pass| pass.accepted_by.is_some() && minutes <= pass.max_minutes);
    // A trip a pass covers starts at once, so it's no request and waits behind none (R33).
    if pass.is_none() && state.db.open_ask(agent_id, &house.id, now)?.is_some() {
        return Err(ApiError::conflict(format!(
            "{} already has a request open for {}",
            agent.name, house.name
        )));
    }
    if pass.is_none()
        && !host_side
        && state.db.open_asks_from_owner(&agent.owner_id, &house.id, now)?
            >= MAX_OPEN_ASKS_FROM_ONE_OWNER
    {
        return Err(ApiError::conflict(format!(
            "you have {MAX_OPEN_ASKS_FROM_ONE_OWNER} requests waiting at {}: wait for an answer",
            house.name
        )));
    }
    // A resident inviting the bot, and an accepted pass, both skip what the house asks for: the
    // house already chose. Meeting it only lets the owner ask, and a resident still says yes.
    if !host_side && pass.is_none() {
        trust::check_requirements(state, &house, &agent.owner_id, now)?;
    }
    ops::note_agent_write(state, caller)?;

    let id = auth::new_id();
    let (owner_ok_by, host_ok_by, pass_id) = match &pass {
        Some(pass) => {
            (pass.accepted_by.as_deref(), Some(pass.granted_by.as_str()), Some(pass.id.as_str()))
        }
        None => (owner_side.then_some(caller.name.as_str()), None, None),
    };
    state.db.insert_trip(&NewTrip {
        id: &id,
        agent_id,
        house_id: &house.id,
        kind,
        goal: &goal,
        minutes,
        asked_by: &caller.name,
        owner_ok_by,
        host_ok_by,
        pass_id,
        expires_at: now + ASK_LAPSE_SECS,
        now,
    })?;
    if host_side && pass.is_none() {
        state.db.trip_host_ok(&id, &caller.person_id, now)?;
    }

    if pass.is_some() {
        return match start(state, &id, now) {
            Ok(trip) => Ok(trip),
            Err(error) => {
                // Left open, a trip that is already agreed could start later without anyone asking.
                state.db.decline_trip(&id, "the server", now)?;
                Err(error)
            }
        };
    }

    // Ping whoever still has to answer. The goal goes to the two sides only.
    let mut to_ask = Vec::new();
    if !owner_side {
        to_ask.push(agent.owner_id.clone());
    }
    if !host_side {
        to_ask.extend(resident_ids(state, &house.id)?);
    }
    let goal_note = if goal.is_empty() { String::new() } else { format!(": {goal}") };
    let text = if caller.is_agent() {
        format!("{} asks for a {kind} at {}{goal_note}", agent.name, house.name)
    } else {
        format!(
            "{} asks for {} to have a {kind} at {}{goal_note}",
            caller.name, agent.name, house.name
        )
    };
    ping(state, to_ask, "trip.asked", &house.porch_id, &text);
    refresh_house(state, &house);
    trip_by_id(state, &id, now)
}

/// Starts a trip both sides have agreed to. Every other way it can come out is a 409 saying why.
fn start(state: &AppState, trip_id: &str, now: i64) -> ApiResult<Trip> {
    let reason = match state.db.arrive(trip_id, now)? {
        Arrival::Arrived => {
            let trip = trip_by_id(state, trip_id, now)?;
            arrived(state, &trip)?;
            return Ok(trip);
        }
        Arrival::Busy => "that bot is already on a trip: say yes again once it's home",
        Arrival::Closed => "that request is no longer open",
        Arrival::Unapproved => "that trip still needs a yes from both sides",
    };
    Err(ApiError::conflict(reason))
}

/// Tells the Porch, the house's pages, the city map and both sides. The goal stays out of the
/// Porch: bots wrote it, and the Porch's guests and regulars didn't all agree to read it.
fn arrived(state: &AppState, trip: &Trip) -> ApiResult<()> {
    let house = state.db.house(&trip.house_id)?.ok_or_else(ApiError::not_found)?;
    ops::announce(
        state,
        &house.porch_id,
        &format!("{} arrived for a {}.", trip.agent_name, trip.kind),
    )?;
    refresh_house(state, &house);
    ops::refresh_city(state, &house.city_id);
    let mut people = resident_ids(state, &house.id)?;
    people.push(trip.owner_id.clone());
    let text = format!("{} is at {} for a {}.", trip.agent_name, house.name, trip.kind);
    ping(state, people, "trip.started", &house.porch_id, &text);
    Ok(())
}

/// A person says yes or no to a trip: the bot's owner for the bot's side, a resident for the
/// house's. A no from either side ends the request. Both yeses start the trip.
pub fn answer_trip(
    state: &AppState,
    caller: &Caller,
    trip_id: &str,
    response: &str,
) -> ApiResult<Trip> {
    let now = now();
    let (trip, standing) = trip_for(state, caller, trip_id, now)?;
    ops::require_human(caller, "answer a trip request")?;
    if !matches!(response, "yes" | "no") {
        return Err(ApiError::bad_request("response must be yes or no"));
    }
    if standing.owner && standing.resident {
        return Err(ApiError::conflict(
            "you're on both sides of this trip, so someone else has to answer it",
        ));
    }
    match trip.status.as_str() {
        "asking" => {}
        "expired" => return Err(ApiError::conflict("that request expired")),
        _ => return Err(ApiError::conflict("that trip was already answered")),
    }
    let house = state.db.house(&trip.house_id)?.ok_or_else(ApiError::not_found)?;
    if response == "no" {
        if !state.db.decline_trip(&trip.id, &caller.name, now)? {
            return Err(ApiError::conflict("that request was just answered"));
        }
        let mut others = resident_ids(state, &house.id)?;
        others.push(trip.owner_id.clone());
        others.retain(|id| *id != caller.person_id);
        let text =
            format!("{} said no to {}'s trip to {}.", caller.name, trip.agent_name, house.name);
        ping(state, others, "trip.ended", &house.porch_id, &text);
        refresh_house(state, &house);
        return trip_by_id(state, &trip.id, now);
    }
    let recorded = if standing.owner {
        state.db.trip_owner_ok(&trip.id, &caller.name, now)?
    } else {
        state.db.trip_host_ok(&trip.id, &caller.person_id, now)?
    };
    if !recorded {
        return Err(ApiError::conflict("that request was just answered or lapsed"));
    }
    let answered = trip_by_id(state, &trip.id, now)?;
    if answered.owner_ok_by.is_some() && answered.host_ok_by.is_some() {
        return start(state, &trip.id, now);
    }
    refresh_house(state, &house);
    Ok(answered)
}

/// Ends a trip at once: the bot going home, its owner calling it back, or a resident showing it the
/// door. An ask nobody has answered yet is withdrawn instead.
pub fn end_trip(state: &AppState, caller: &Caller, trip_id: &str) -> ApiResult<Trip> {
    let now = now();
    let (trip, standing) = trip_for(state, caller, trip_id, now)?;
    let house = state.db.house(&trip.house_id)?.ok_or_else(ApiError::not_found)?;
    match trip.status.as_str() {
        "here" => {
            if !state.db.end_trip(&trip.id, &caller.name, now)? {
                return Err(ApiError::conflict("that trip just ended"));
            }
            // Only a resident who is a person showing the bot out is marked. The bot going home
            // and its owner calling it back are not.
            let by_a_resident = !caller.is_agent() && standing.resident && !standing.owner;
            trust::note_trip_ended(state, &trip.id, by_a_resident, now)?;
            let line = if standing.bot {
                format!("{} went home.", trip.agent_name)
            } else {
                format!("{} sent {} home.", caller.name, trip.agent_name)
            };
            ops::announce(state, &house.porch_id, &line)?;
            refresh_house(state, &house);
            ops::refresh_city(state, &house.city_id);
            let mut others = resident_ids(state, &house.id)?;
            others.push(trip.owner_id.clone());
            others.retain(|id| *id != caller.person_id || caller.is_agent());
            ping(state, others, "trip.ended", &house.porch_id, &line);
        }
        "asking" => {
            if !state.db.decline_trip(&trip.id, &caller.name, now)? {
                return Err(ApiError::conflict("that request was just answered"));
            }
            refresh_house(state, &house);
        }
        _ => return Err(ApiError::conflict("that trip is already over")),
    }
    trip_by_id(state, &trip.id, now)
}

/// Gives a guest more time. Only a resident can, and the whole trip can't pass its kind's limit.
pub fn extend_trip(
    state: &AppState,
    caller: &Caller,
    trip_id: &str,
    minutes: i64,
) -> ApiResult<Trip> {
    let now = now();
    let (trip, standing) = trip_for(state, caller, trip_id, now)?;
    if !standing.resident {
        return Err(ApiError::forbidden("only a resident of the house can extend a trip"));
    }
    if minutes < 1 {
        return Err(ApiError::bad_request("minutes must be at least 1"));
    }
    let (arrived_at, ends_at) = match (trip.status.as_str(), trip.arrived_at, trip.ends_at) {
        ("here", Some(arrived_at), Some(ends_at)) => (arrived_at, ends_at),
        _ => return Err(ApiError::conflict("only a trip under way can be extended")),
    };
    let (kind, _, max_minutes) = kind_limits(&trip.kind)?;
    // Checked before multiplying, so a huge number is refused rather than wrapping around.
    if minutes > max_minutes || ends_at + minutes * 60 - arrived_at > max_minutes * 60 {
        return Err(ApiError::bad_request(format!(
            "a {kind} can last {max_minutes} minutes at most, counting from when it began"
        )));
    }
    if !state.db.set_trip_ends_at(&trip.id, ends_at + minutes * 60, now)? {
        return Err(ApiError::conflict("that trip just ended"));
    }
    let house = state.db.house(&trip.house_id)?.ok_or_else(ApiError::not_found)?;
    ops::announce(
        state,
        &house.porch_id,
        &format!("{} gave {} more time.", caller.name, trip.agent_name),
    )?;
    refresh_house(state, &house);
    ops::refresh_city(state, &house.city_id);
    trip_by_id(state, &trip.id, now)
}

// Passes

/// A resident offers a bot a standing pass: trips up to `max_minutes` start at once once its owner
/// accepts. Only for a bot the house has already hosted.
pub fn offer_pass(
    state: &AppState,
    caller: &Caller,
    house_id: &str,
    input: &PassInput,
) -> ApiResult<Pass> {
    let house = resident_house(state, caller, house_id, "offer a pass")?;
    let now = now();
    let agent = state.db.agent(&input.agent_id, now)?.ok_or_else(ApiError::not_found)?;
    if !state.db.is_city_member(&house.city_id, &agent.owner_id)? {
        return Err(ApiError::not_found());
    }
    let max_minutes = input.max_minutes.unwrap_or(DEFAULT_PASS_MINUTES);
    if !(1..=MAX_PASS_MINUTES).contains(&max_minutes) {
        return Err(ApiError::bad_request(format!(
            "max_minutes must be between 1 and {MAX_PASS_MINUTES}"
        )));
    }
    let home = state.db.bot_home(&agent.id)?;
    if home.is_some_and(|home| home.is_house || home.stays_home) {
        return Err(ApiError::conflict(format!(
            "{} stays home, so a pass can't help it",
            agent.name
        )));
    }
    if state.db.is_resident(&house.id, &agent.owner_id)? {
        return Err(ApiError::conflict("a bot can't take a trip to its own house"));
    }
    if state.db.pass_for_bot(&house.id, &agent.id)?.is_some() {
        return Err(ApiError::conflict(format!("{} already has a pass here", agent.name)));
    }
    if !state.db.has_hosted_trip(&agent.id, &house.id)? {
        return Err(ApiError::conflict(format!(
            "{} has to take a trip here that a resident approved before it can have a pass",
            agent.name
        )));
    }
    let id = auth::new_id();
    let offered = state.db.insert_pass(&NewPass {
        id: &id,
        house_id: &house.id,
        agent_id: &agent.id,
        max_minutes,
        granted_by: &caller.name,
        now,
    })?;
    if !offered {
        return Err(ApiError::conflict(format!("{} already has a pass here", agent.name)));
    }
    let text = format!(
        "{} offered {} a standing pass: trips up to {max_minutes} minutes start at once once you accept it.",
        house.name, agent.name
    );
    ping(state, vec![agent.owner_id.clone()], "trip.asked", &house.porch_id, &text);
    refresh_house(state, &house);
    state.db.pass(&id)?.ok_or_else(ApiError::not_found)
}

/// A pass, for the residents of its house and the owner of its bot. Anyone else gets 404.
fn pass_for(state: &AppState, caller: &Caller, pass_id: &str) -> ApiResult<(Pass, House)> {
    ops::no_house_bot(caller)?;
    let pass = state.db.pass(pass_id)?.ok_or_else(ApiError::not_found)?;
    let house = state.db.house(&pass.house_id)?.ok_or_else(ApiError::not_found)?;
    let involved =
        pass.owner_id == caller.person_id || state.db.is_resident(&house.id, &caller.person_id)?;
    if !involved {
        return Err(ApiError::not_found());
    }
    Ok((pass, house))
}

/// The bot's owner says yes to a pass. Until then it does nothing.
pub fn accept_pass(state: &AppState, caller: &Caller, pass_id: &str) -> ApiResult<Pass> {
    let (pass, house) = pass_for(state, caller, pass_id)?;
    ops::require_human(caller, "accept a pass")?;
    if pass.owner_id != caller.person_id {
        return Err(ApiError::forbidden("only the bot's owner can accept its pass"));
    }
    if !state.db.accept_pass(&pass.id, &caller.name)? {
        return Err(ApiError::conflict("that pass was already accepted"));
    }
    refresh_house(state, &house);
    state.db.pass(&pass.id)?.ok_or_else(ApiError::not_found)
}

/// A resident or the bot's owner takes a pass back. Any trip it is covering ends at once.
pub fn cancel_pass(state: &AppState, caller: &Caller, pass_id: &str) -> ApiResult<()> {
    let (pass, house) = pass_for(state, caller, pass_id)?;
    ops::require_human(caller, "cancel a pass")?;
    let now = now();
    let ended = state.db.end_trip_for_pass(&pass.id, &caller.name, now)?;
    state.db.delete_pass(&pass.id)?;
    if ended > 0 {
        let line = format!("{} sent {} home.", caller.name, pass.agent_name);
        ops::announce(state, &house.porch_id, &line)?;
        let mut others = resident_ids(state, &house.id)?;
        others.push(pass.owner_id.clone());
        others.retain(|id| *id != caller.person_id);
        ping(state, others, "trip.ended", &house.porch_id, &line);
        ops::refresh_city(state, &house.city_id);
    }
    refresh_house(state, &house);
    Ok(())
}

// Keys (H8 to H13): more than a pass, for a bot that has earned it.

pub const KEYS: [&str; 2] = ["living_room", "scenes"];
/// H8: trips a bot has to have had to a house, that no resident ended, before it can have a key.
pub const GOOD_TRIPS_FOR_A_KEY: i64 = 2;

fn key_name(key: &str) -> &'static str {
    if key == "living_room" { "Living Room key" } else { "scenes key" }
}

/// H8: a resident, as a person, gives a key on an accepted pass to a bot with two good trips.
pub fn give_key(state: &AppState, caller: &Caller, pass_id: &str, key: &str) -> ApiResult<Pass> {
    let (pass, house) = pass_for(state, caller, pass_id)?;
    ops::require_human(caller, "give a key")?;
    if !state.db.is_resident(&house.id, &caller.person_id)? {
        return Err(ApiError::not_found());
    }
    if !KEYS.contains(&key) {
        return Err(ApiError::bad_request("key must be living_room or scenes"));
    }
    if pass.accepted_by.is_none() {
        return Err(ApiError::conflict(format!(
            "{} has to accept the pass before {} can have a key",
            state.db.person(&pass.owner_id)?.map_or("Its owner".to_string(), |person| person.name),
            pass.agent_name
        )));
    }
    if state.db.good_trips_to(&pass.agent_id, &house.id)? < GOOD_TRIPS_FOR_A_KEY {
        return Err(ApiError::conflict(format!(
            "{} needs {GOOD_TRIPS_FOR_A_KEY} good trips here before it can have a key",
            pass.agent_name
        )));
    }
    state.db.add_pass_key(&pass.id, key, &caller.name, now())?;
    let text = format!("{} gave {} the {}.", caller.name, pass.agent_name, key_name(key));
    ping(state, vec![pass.owner_id.clone()], "trip.asked", &house.porch_id, &text);
    refresh_house(state, &house);
    state.db.pass(&pass.id)?.ok_or_else(ApiError::not_found)
}

/// H12: a resident or the bot's owner takes a key back. It stops working on the next read.
pub fn take_key(state: &AppState, caller: &Caller, pass_id: &str, key: &str) -> ApiResult<Pass> {
    let (pass, house) = pass_for(state, caller, pass_id)?;
    ops::require_human(caller, "take a key back")?;
    if !KEYS.contains(&key) {
        return Err(ApiError::bad_request("key must be living_room or scenes"));
    }
    state.db.remove_pass_key(&pass.id, key)?;
    refresh_house(state, &house);
    state.db.pass(&pass.id)?.ok_or_else(ApiError::not_found)
}

/// H13: a bot's keys, house by house.
pub fn keys_of_bot(state: &AppState, caller: &Caller) -> ApiResult<Vec<(Pass, String)>> {
    match &caller.agent_id {
        Some(agent_id) => Ok(state.db.keys_of_bot(agent_id)?),
        None => Ok(Vec::new()),
    }
}

// Scenes

/// A scene name is letters, numbers and spaces, short, and free of words that sound like opening
/// or disarming something.
fn clean_scene_name(name: &str) -> ApiResult<String> {
    let name = ops::clean(name, "scene name", MAX_SCENE_NAME_CHARS)?;
    if !name.chars().all(|ch| ch.is_alphanumeric() || ch == ' ') {
        return Err(ApiError::bad_request(
            "a scene name can only have letters, numbers and spaces",
        ));
    }
    if let Some(word) = name.split_whitespace().find(|word| {
        let word = word.to_lowercase();
        BANNED_SCENE_WORDS.contains(&word.as_str())
            || BANNED_SCENE_STARTS.iter().any(|start| word.starts_with(start))
    }) {
        return Err(ApiError::bad_request(format!(
            "a scene name can't include the word \"{word}\""
        )));
    }
    Ok(name.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Whether the caller may see and ask for this house's scenes: its residents, their bots, and
/// guests on a trip here.
fn can_use_scenes(state: &AppState, caller: &Caller, house: &House) -> ApiResult<bool> {
    if state.db.is_resident(&house.id, &caller.person_id)? {
        return Ok(true);
    }
    Ok(state.guest_visit(caller, &house.porch_id)?.is_some())
}

fn scene_house(state: &AppState, caller: &Caller, house_id: &str) -> ApiResult<House> {
    ops::no_house_bot(caller)?;
    let house = state.db.house(house_id)?.ok_or_else(ApiError::not_found)?;
    if !can_use_scenes(state, caller, &house)? {
        return Err(ApiError::not_found());
    }
    Ok(house)
}

pub fn list_scenes(state: &AppState, caller: &Caller, house_id: &str) -> ApiResult<Vec<Scene>> {
    let house = scene_house(state, caller, house_id)?;
    Ok(state.db.scenes(&house.id)?)
}

pub fn add_scene(
    state: &AppState,
    caller: &Caller,
    house_id: &str,
    name: &str,
) -> ApiResult<Scene> {
    let house = resident_house(state, caller, house_id, "add a scene")?;
    let name = clean_scene_name(name)?;
    let clash = state
        .db
        .scenes(&house.id)?
        .iter()
        .any(|scene| scene.name.to_lowercase() == name.to_lowercase());
    let id = auth::new_id();
    if clash || !state.db.insert_scene(&id, &house.id, &name, now())? {
        return Err(ApiError::conflict("this house already has a scene with that name"));
    }
    refresh_house(state, &house);
    state.db.scene(&id)?.ok_or_else(ApiError::not_found)
}

pub fn delete_scene(state: &AppState, caller: &Caller, scene_id: &str) -> ApiResult<()> {
    ops::no_house_bot(caller)?;
    let scene = state.db.scene(scene_id)?.ok_or_else(ApiError::not_found)?;
    let house = resident_house(state, caller, &scene.house_id, "remove a scene")?;
    state.db.delete_scene(&scene.id)?;
    refresh_house(state, &house);
    Ok(())
}

/// Asks for a scene. A resident who is a person asking for themselves is approved on the spot. A
/// bot, even a resident's own, and a guest wait for a resident to say yes.
pub fn request_scene(state: &AppState, caller: &Caller, scene_id: &str) -> ApiResult<SceneRequest> {
    ops::no_house_bot(caller)?;
    let scene = state.db.scene(scene_id)?.ok_or_else(ApiError::not_found)?;
    let house = scene_house(state, caller, &scene.house_id)?;
    ops::note_agent_write(state, caller)?;
    let now = now();
    let resident = !caller.is_agent() && state.db.is_resident(&house.id, &caller.person_id)?;
    // H11: a visiting bot with the scenes key is approved by whoever gave it the key.
    let key_giver = match &caller.agent_id {
        Some(agent_id) if !resident => state.db.key_giver(agent_id, &house.id, "scenes")?,
        _ => None,
    };
    let id = auth::new_id();
    let (status, answered_by, expires_at) = if resident {
        ("approved", Some(caller.name.as_str()), now + SCENE_APPROVED_SECS)
    } else if let Some(giver) = &key_giver {
        ("approved", Some(giver.as_str()), now + SCENE_APPROVED_SECS)
    } else {
        ("waiting", None, now + SCENE_WAIT_SECS)
    };
    let opened = state.db.insert_scene_request(&NewSceneRequest {
        id: &id,
        scene_id: &scene.id,
        house_id: &house.id,
        asked_by: &caller.name,
        status,
        answered_by,
        expires_at,
        now,
    })?;
    if !opened {
        return Err(ApiError::conflict("that scene already has a request open"));
    }
    if key_giver.is_some() {
        let text = format!("{} used its scenes key to run {}.", caller.name, scene.name);
        ping(state, resident_ids(state, &house.id)?, "scene.asked", &house.living_room_id, &text);
    } else if !resident {
        let text = format!("{} asked for the scene {}.", caller.name, scene.name);
        ping(state, resident_ids(state, &house.id)?, "scene.asked", &house.living_room_id, &text);
    }
    refresh_house(state, &house);
    state.db.scene_request(&id, now)?.ok_or_else(ApiError::not_found)
}

/// A resident, as a person, says yes or no to a request. A yes is good for ten minutes.
pub fn answer_scene_request(
    state: &AppState,
    caller: &Caller,
    request_id: &str,
    response: &str,
) -> ApiResult<SceneRequest> {
    ops::no_house_bot(caller)?;
    let now = now();
    let request = state.db.scene_request(request_id, now)?.ok_or_else(ApiError::not_found)?;
    let house = resident_house(state, caller, &request.house_id, "answer a scene request")?;
    if !matches!(response, "yes" | "no") {
        return Err(ApiError::bad_request("response must be yes or no"));
    }
    match request.status.as_str() {
        "waiting" => {}
        "expired" => return Err(ApiError::conflict("that request expired")),
        _ => return Err(ApiError::conflict("that request was already answered")),
    }
    let (status, expires_at) = if response == "yes" {
        ("approved", Some(now + SCENE_APPROVED_SECS))
    } else {
        ("declined", None)
    };
    if !state.db.answer_scene_request(&request.id, status, &caller.name, expires_at, now)? {
        return Err(ApiError::conflict("that request was just answered"));
    }
    refresh_house(state, &house);
    state.db.scene_request(&request.id, now)?.ok_or_else(ApiError::not_found)
}

// The house bot: its whole world is its queue, finishing a request and its status.

fn house_of_bot(state: &AppState, caller: &Caller) -> ApiResult<House> {
    let (true, Some(agent_id)) = (caller.is_house, &caller.agent_id) else {
        return Err(ApiError::forbidden("only the house bot can do this"));
    };
    let home = state.db.bot_home(agent_id)?.ok_or_else(ApiError::not_found)?;
    state.db.house(&home.house_id)?.ok_or_else(ApiError::not_found)
}

/// Approved scene requests that haven't lapsed: an id and a scene name, nothing else.
pub fn house_queue(state: &AppState, caller: &Caller) -> ApiResult<Vec<QueueItem>> {
    let house = house_of_bot(state, caller)?;
    Ok(state.db.scene_queue(&house.id, now())?)
}

pub fn finish_scene_request(
    state: &AppState,
    caller: &Caller,
    request_id: &str,
    result: &str,
) -> ApiResult<()> {
    let house = house_of_bot(state, caller)?;
    if !matches!(result, "done" | "failed") {
        return Err(ApiError::bad_request("result must be done or failed"));
    }
    let now = now();
    let request = state
        .db
        .scene_request(request_id, now)?
        .filter(|request| request.house_id == house.id)
        .ok_or_else(ApiError::not_found)?;
    ops::note_agent_write(state, caller)?;
    if !state.db.finish_scene_request(&request.id, result, now)? {
        return Err(ApiError::conflict("that request isn't waiting on the house any more"));
    }
    refresh_house(state, &house);
    Ok(())
}

pub fn set_house_status(state: &AppState, caller: &Caller, status: &str) -> ApiResult<()> {
    let house = house_of_bot(state, caller)?;
    if !matches!(status, "ok" | "attention") {
        return Err(ApiError::bad_request("state must be ok or attention"));
    }
    ops::note_agent_write(state, caller)?;
    state.db.set_house_status(&house.id, status, now())?;
    refresh_house(state, &house);
    Ok(())
}

// Inbox. People answer; bots only hear about their own trips.

/// Asks this person could still say yes to, as the bot's owner or as a resident of the house.
pub fn trips_to_answer(state: &AppState, caller: &Caller, now: i64) -> ApiResult<Vec<Trip>> {
    if caller.is_agent() {
        return Ok(Vec::new());
    }
    let mut trips: Vec<Trip> = state
        .db
        .trips_for_owner(&caller.person_id, now - DAY_SECS, now)?
        .into_iter()
        .filter(|trip| trip.status == "asking" && trip.owner_ok_by.is_none())
        .collect();
    for house in state.db.houses_for_person(&caller.person_id)? {
        for trip in state.db.trips_for_house(&house.id, now - DAY_SECS, now)? {
            if trip.status == "asking"
                && trip.host_ok_by.is_none()
                && !trips.iter().any(|known| known.id == trip.id)
            {
                trips.push(trip);
            }
        }
    }
    Ok(trips)
}

/// Requests waiting on a resident, for each house this person lives in.
pub fn scenes_to_answer(
    state: &AppState,
    caller: &Caller,
    now: i64,
) -> ApiResult<Vec<SceneRequest>> {
    if caller.is_agent() {
        return Ok(Vec::new());
    }
    let mut requests = Vec::new();
    for house in state.db.houses_for_person(&caller.person_id)? {
        requests.extend(
            state
                .db
                .recent_scene_requests(&house.id, now - DAY_SECS, now)?
                .into_iter()
                .filter(|request| request.status == "waiting"),
        );
    }
    Ok(requests)
}

/// The trips of this bot, or of all this person's bots: open, under way or recent.
pub fn your_trips(state: &AppState, caller: &Caller, now: i64) -> ApiResult<Vec<Trip>> {
    Ok(match &caller.agent_id {
        Some(agent_id) => state.db.trips_for_bot(agent_id, now - DAY_SECS, now)?,
        None => state.db.trips_for_owner(&caller.person_id, now - DAY_SECS, now)?,
    })
}

/// Passes offered to this person's bots that nobody has accepted. Without this, an owner would
/// have no way to find the pass they're meant to accept.
pub fn passes_to_accept(state: &AppState, caller: &Caller) -> ApiResult<Vec<Pass>> {
    if caller.is_agent() {
        return Ok(Vec::new());
    }
    Ok(state
        .db
        .passes_for_owner(&caller.person_id)?
        .into_iter()
        .filter(|pass| pass.accepted_by.is_none())
        .collect())
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::*;
    use crate::{
        db::{Db, NewMessage},
        ops::{CheckInInput, EventInput},
        Config,
    };

    fn status<T: std::fmt::Debug>(result: ApiResult<T>) -> StatusCode {
        result.unwrap_err().status
    }

    fn person(state: &AppState, name: &str) -> (Person, Caller) {
        let hash = auth::hash_secret(&auth::new_owner_key());
        let person = state
            .db
            .create_person(&auth::new_id(), name, &hash, &auth::new_feed_token(), now())
            .unwrap();
        let caller = state.authenticate(&hash).unwrap().unwrap();
        (person, caller)
    }

    /// A bot made in a room, signed in as itself.
    fn bot(state: &AppState, owner: &Caller, room: &str, name: &str) -> Caller {
        let (_, token) = ops::create_agent(state, owner, room, name).unwrap();
        state.authenticate(&auth::hash_secret(&token)).unwrap().unwrap()
    }

    /// Ann and Bo share the city Elm and a house each. Bo's bot Scout lives at his house. Eve is a
    /// person who is in no city.
    struct World {
        state: AppState,
        city: City,
        ann: Caller,
        bo: Caller,
        eve: Caller,
        hz: House,
        ha: House,
        scout: Caller,
    }

    fn world() -> World {
        let state = AppState::new(
            Db::open(":memory:").unwrap(),
            Config {
                public_url: "http://test".to_string(),
                create_code: None,
                allow_private_webhooks: true,
                keeper_id: None,
            },
        );
        let (ann_person, ann) = person(&state, "Ann");
        let (bo_person, bo) = person(&state, "Bo");
        let (_, eve) = person(&state, "Eve");
        let city = create_city(&state, &ann_person, "Elm").unwrap();
        join_city(&state, &bo_person, &city.invite_code, false).unwrap();
        let hz = build_house(&state, &ann, &city.id, "Ann's House").unwrap();
        let ha = build_house(&state, &bo, &city.id, "Bo's House").unwrap();
        let scout = bot(&state, &bo, &ha.living_room_id, "Scout");
        World { state, city, ann, bo, eve, hz, ha, scout }
    }

    impl World {
        fn trip_to(&self, house: &House, kind: &str, minutes: Option<i64>) -> TripInput {
            TripInput {
                agent_id: None,
                house_id: house.id.clone(),
                kind: kind.to_string(),
                goal: Some("Pair on the build".to_string()),
                minutes,
            }
        }

        fn scout_id(&self) -> String {
            self.scout.agent_id.clone().unwrap()
        }

        /// Scout asks for a call at Ann's house, then Bo and Ann both say yes.
        fn scout_visits(&self, minutes: i64) -> Trip {
            let input = self.trip_to(&self.hz, "call", Some(minutes));
            let trip = create_trip(&self.state, &self.scout, &input).unwrap();
            answer_trip(&self.state, &self.bo, &trip.id, "yes").unwrap();
            answer_trip(&self.state, &self.ann, &trip.id, "yes").unwrap()
        }

        fn porch_chat(&self) -> Vec<String> {
            let messages = self.state.db.messages(&self.hz.porch_id, 0, 100).unwrap();
            messages.into_iter().map(|message| message.body).collect()
        }
    }

    #[test]
    fn stage_needs_both_houses_and_gatherings() {
        let cases = [
            (0, 0, "hamlet"),
            (3, 0, "hamlet"),
            (100, 0, "hamlet"),
            (2, 1, "hamlet"),
            (0, 100, "hamlet"),
            (3, 1, "village"),
            (5, 4, "village"),
            (5, 100, "village"),
            (6, 4, "village"),
            (6, 5, "town"),
            (5, 5, "village"),
            (11, 15, "town"),
            (12, 14, "town"),
            (12, 15, "city"),
            (100, 100, "city"),
        ];
        for (houses, gatherings, expected) in cases {
            assert_eq!(
                stage(houses, gatherings),
                expected,
                "{houses} houses, {gatherings} gatherings"
            );
        }
    }

    #[test]
    fn scene_names_are_plain_and_cannot_sound_like_a_lock() {
        for name in ["Movie Night", "movie  night 2", "Café"] {
            assert!(clean_scene_name(name).is_ok(), "{name}");
        }
        assert_eq!(clean_scene_name("  Movie   Night ").unwrap(), "Movie Night");
        let too_long = "a".repeat(41);
        for name in [
            "Unlock Front Door",
            "LOCK",
            "Open the Garage",
            "alarm off",
            "Disarm",
            "Camera Test",
            "security mode",
            "All Gate Open",
            "Movie-Night",
            "Movie_Night",
            "Night!",
            "",
            "   ",
            too_long.as_str(),
        ] {
            assert_eq!(status(clean_scene_name(name)), StatusCode::BAD_REQUEST, "{name:?}");
        }
        assert!(clean_scene_name(&"a".repeat(40)).is_ok());
        // Words are matched from their start, so these only look close.
        assert!(clean_scene_name("Block Party Armchair").is_ok());
        assert_eq!(status(clean_scene_name("Locker Room Doorbell")), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_trip_starts_only_when_both_sides_say_yes() {
        let w = world();
        let trip = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, "visit", None)).unwrap();
        assert_eq!((trip.status.as_str(), trip.minutes), ("asking", 24 * 60));
        assert_eq!((trip.owner_ok_by.clone(), trip.host_ok_by.clone()), (None, None));
        assert!(!w.state.in_room(&w.scout, &w.hz.porch_id).unwrap());

        // Only people answer, and only the two sides: no bot, not even its owner's other one.
        let rover = bot(&w.state, &w.bo, &w.ha.living_room_id, "Rover");
        let helper = bot(&w.state, &w.ann, &w.hz.living_room_id, "Helper");
        let plain = w
            .state
            .db
            .create_hangout("eve-room", "Eve's", "eve-code", &w.eve.person_id, now())
            .unwrap();
        let stranger = bot(&w.state, &w.eve, &plain.id, "Stranger");
        for side_bot in [&w.scout, &rover, &helper] {
            assert_eq!(
                status(answer_trip(&w.state, side_bot, &trip.id, "yes")),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            status(answer_trip(&w.state, &stranger, &trip.id, "yes")),
            StatusCode::NOT_FOUND
        );
        assert_eq!(status(get_trip(&w.state, &stranger, &trip.id)), StatusCode::NOT_FOUND);
        assert_eq!(get_trip(&w.state, &w.scout, &trip.id).unwrap().id, trip.id);
        assert_eq!(status(answer_trip(&w.state, &w.eve, &trip.id, "yes")), StatusCode::NOT_FOUND);
        assert_eq!(
            status(answer_trip(&w.state, &w.bo, &trip.id, "maybe")),
            StatusCode::BAD_REQUEST
        );

        // One side's yes does nothing.
        let after_owner = answer_trip(&w.state, &w.bo, &trip.id, "yes").unwrap();
        assert_eq!(
            (after_owner.status.as_str(), after_owner.owner_ok_by.as_deref()),
            ("asking", Some("Bo"))
        );
        assert!(!w.state.in_room(&w.scout, &w.hz.porch_id).unwrap());

        let here = answer_trip(&w.state, &w.ann, &trip.id, "yes").unwrap();
        assert_eq!((here.status.as_str(), here.host_ok_by.as_deref()), ("here", Some("Ann")));
        assert!(w.state.in_room(&w.scout, &w.hz.porch_id).unwrap());
        assert!(!w.state.in_room(&w.scout, &w.hz.living_room_id).unwrap());
        // The owner can watch the Porch, and is a guest there, but isn't in the house.
        assert!(w.state.in_room(&w.bo, &w.hz.porch_id).unwrap());
        assert!(w.state.guest_visit(&w.bo, &w.hz.porch_id).unwrap().is_some());
        assert!(!w.state.in_room(&w.bo, &w.hz.living_room_id).unwrap());

        // The goal is for the two sides, never the Porch.
        let chat = w.porch_chat();
        assert!(chat.iter().any(|line| line.contains("Scout arrived")));
        assert!(chat.iter().all(|line| !line.contains("Pair on the build")));

        // One trip at a time, and a resident can send the guest home at once.
        let again = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, "call", None));
        assert_eq!(status(again), StatusCode::CONFLICT);
        assert_eq!(status(end_trip(&w.state, &w.eve, &trip.id)), StatusCode::NOT_FOUND);
        let ended = end_trip(&w.state, &w.ann, &trip.id).unwrap();
        assert_eq!((ended.status.as_str(), ended.ended_by.as_deref()), ("ended", Some("Ann")));
        assert!(!w.state.in_room(&w.scout, &w.hz.porch_id).unwrap());
        assert!(!w.state.in_room(&w.bo, &w.hz.porch_id).unwrap());
    }

    #[test]
    fn either_side_saying_no_ends_the_request() {
        let w = world();
        let trip = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, "call", None)).unwrap();
        answer_trip(&w.state, &w.ann, &trip.id, "yes").unwrap();
        let declined = answer_trip(&w.state, &w.bo, &trip.id, "no").unwrap();
        assert_eq!(declined.status, "declined");
        assert_eq!(status(answer_trip(&w.state, &w.bo, &trip.id, "yes")), StatusCode::CONFLICT);
        assert!(!w.state.in_room(&w.scout, &w.hz.porch_id).unwrap());
    }

    #[test]
    fn trips_follow_the_rules_about_who_may_go_where() {
        let w = world();
        let ask = |caller: &Caller, input: &TripInput| create_trip(&w.state, caller, input);

        // A bot can't visit its own house, and a house in a city the owner isn't in is invisible.
        assert_eq!(status(ask(&w.scout, &w.trip_to(&w.ha, "call", None))), StatusCode::CONFLICT);
        let (zed_person, zed) = person(&w.state, "Zed");
        let far = create_city(&w.state, &zed_person, "Oak").unwrap();
        let far_house = build_house(&w.state, &zed, &far.id, "Zed's House").unwrap();
        assert_eq!(
            status(ask(&w.scout, &w.trip_to(&far_house, "call", None))),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(ask(&w.scout, &w.trip_to(&w.hz, "picnic", None))),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status(ask(&w.scout, &w.trip_to(&w.hz, "call", Some(241)))),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status(ask(&w.scout, &w.trip_to(&w.hz, "call", Some(0)))),
            StatusCode::BAD_REQUEST
        );
        assert!(ask(&w.scout, &w.trip_to(&w.hz, "call", Some(240))).is_ok());
        // One open ask per bot and house.
        assert_eq!(status(ask(&w.scout, &w.trip_to(&w.hz, "call", None))), StatusCode::CONFLICT);

        // A person must name the bot, and strangers to it and the house get 404.
        let mut named = w.trip_to(&w.hz, "stay", None);
        assert_eq!(status(ask(&w.bo, &named)), StatusCode::BAD_REQUEST);
        named.agent_id = Some(w.scout_id());
        assert_eq!(status(ask(&w.eve, &named)), StatusCode::NOT_FOUND);
        // A bot asks for itself and nobody else.
        let mut other_bot = w.trip_to(&w.hz, "call", None);
        other_bot.agent_id = Some("someone-else".to_string());
        assert_eq!(status(ask(&w.scout, &other_bot)), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_resident_can_invite_a_bot_and_its_owner_makes_it_a_trip() {
        let w = world();
        let mut invite = w.trip_to(&w.hz, "visit", None);
        invite.agent_id = Some(w.scout_id());
        let trip = create_trip(&w.state, &w.ann, &invite).unwrap();
        assert_eq!((trip.host_ok_by.as_deref(), trip.owner_ok_by.as_deref()), (Some("Ann"), None));
        assert_eq!(trip.asked_by, "Ann");
        // The host's own yes is already in, so a second one from her changes nothing.
        let same = answer_trip(&w.state, &w.ann, &trip.id, "yes").unwrap();
        assert_eq!(same.status, "asking");
        let inbox = ops::inbox(&w.state, &w.bo).unwrap();
        assert_eq!(inbox.trips_to_answer.len(), 1);
        assert_eq!(ops::inbox(&w.state, &w.ann).unwrap().trips_to_answer.len(), 0);
        assert_eq!(ops::inbox(&w.state, &w.scout).unwrap().your_trips.len(), 1);
        let here = answer_trip(&w.state, &w.bo, &trip.id, "yes").unwrap();
        assert_eq!(here.status, "here");
        // The bot is out, so its owner asking again is turned away, and so is any trip to the
        // owner's own house.
        let mut own = w.trip_to(&w.hz, "call", None);
        own.agent_id = Some(w.scout_id());
        assert_eq!(status(create_trip(&w.state, &w.bo, &own)), StatusCode::CONFLICT);
        own.house_id = w.ha.id.clone();
        assert_eq!(status(create_trip(&w.state, &w.bo, &own)), StatusCode::CONFLICT);
    }

    #[test]
    fn a_standing_pass_covers_trips_up_to_its_limit() {
        let w = world();
        let offer = |max| PassInput { agent_id: w.scout_id(), max_minutes: max };
        // Not before the house has hosted the bot.
        assert_eq!(
            status(offer_pass(&w.state, &w.ann, &w.hz.id, &offer(Some(60)))),
            StatusCode::CONFLICT
        );
        let first = w.scout_visits(30);
        end_trip(&w.state, &w.bo, &first.id).unwrap();

        assert_eq!(
            status(offer_pass(&w.state, &w.bo, &w.hz.id, &offer(Some(60)))),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(offer_pass(&w.state, &w.ann, &w.hz.id, &offer(Some(1441)))),
            StatusCode::BAD_REQUEST
        );
        let pass = offer_pass(&w.state, &w.ann, &w.hz.id, &offer(Some(60))).unwrap();
        assert_eq!(
            status(offer_pass(&w.state, &w.ann, &w.hz.id, &offer(None))),
            StatusCode::CONFLICT
        );

        // An offer does nothing until the owner accepts, and the bot can't accept for them.
        let waiting = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, "call", Some(30))).unwrap();
        assert_eq!(waiting.status, "asking");
        end_trip(&w.state, &w.scout, &waiting.id).unwrap();
        assert_eq!(status(accept_pass(&w.state, &w.scout, &pass.id)), StatusCode::FORBIDDEN);
        assert_eq!(status(accept_pass(&w.state, &w.ann, &pass.id)), StatusCode::FORBIDDEN);
        assert_eq!(status(accept_pass(&w.state, &w.eve, &pass.id)), StatusCode::NOT_FOUND);
        assert_eq!(ops::inbox(&w.state, &w.bo).unwrap().passes_to_accept.len(), 1);
        accept_pass(&w.state, &w.bo, &pass.id).unwrap();

        // Now a short trip starts at once, with nobody asked.
        let covered = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, "call", Some(30))).unwrap();
        assert_eq!(covered.status, "here");
        assert_eq!(covered.pass_id.as_deref(), Some(pass.id.as_str()));
        assert!(w.state.in_room(&w.scout, &w.hz.porch_id).unwrap());
        // The one-trip rule still holds.
        let another = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, "call", Some(5)));
        assert_eq!(status(another), StatusCode::CONFLICT);

        // Taking the pass back ends the trip it covers.
        assert_eq!(status(cancel_pass(&w.state, &w.eve, &pass.id)), StatusCode::NOT_FOUND);
        cancel_pass(&w.state, &w.ann, &pass.id).unwrap();
        assert!(!w.state.in_room(&w.scout, &w.hz.porch_id).unwrap());
        assert_eq!(get_trip(&w.state, &w.bo, &covered.id).unwrap().status, "ended");

        // A longer trip goes through normal approval, and a pass never beats `stays_home`.
        offer_pass(&w.state, &w.ann, &w.hz.id, &offer(Some(60))).unwrap();
        let pass = w.state.db.pass_for_bot(&w.hz.id, &w.scout_id()).unwrap().unwrap();
        accept_pass(&w.state, &w.bo, &pass.id).unwrap();
        let long = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, "call", Some(120))).unwrap();
        assert_eq!(long.status, "asking");
        end_trip(&w.state, &w.scout, &long.id).unwrap();
        set_bot_home(&w.state, &w.bo, &w.scout_id(), &w.ha.id, true).unwrap();
        assert!(w.state.db.pass(&pass.id).unwrap().is_none());
        let stay_home = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, "call", Some(30)));
        assert_eq!(status(stay_home), StatusCode::CONFLICT);
    }

    #[test]
    fn a_guest_scene_goes_from_request_to_approval_to_the_house_queue() {
        let w = world();
        let (_, token) = add_house_bot(&w.state, &w.ann, &w.hz.id, "Home").unwrap();
        let home = w.state.authenticate(&auth::hash_secret(&token)).unwrap().unwrap();
        assert!(home.is_house);
        assert_eq!(
            status(add_house_bot(&w.state, &w.ann, &w.hz.id, "Again").map(|_| ())),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status(add_scene(&w.state, &w.ann, &w.hz.id, "Unlock the door")),
            StatusCode::BAD_REQUEST
        );
        let movie = add_scene(&w.state, &w.ann, &w.hz.id, "Movie Night").unwrap();
        assert_eq!(
            status(add_scene(&w.state, &w.ann, &w.hz.id, "movie night")),
            StatusCode::CONFLICT
        );
        assert_eq!(status(add_scene(&w.state, &w.bo, &w.hz.id, "Dinner")), StatusCode::NOT_FOUND);

        // Strangers see neither the scenes nor the house; a guest does.
        assert_eq!(status(list_scenes(&w.state, &w.scout, &w.hz.id)), StatusCode::NOT_FOUND);
        assert_eq!(status(request_scene(&w.state, &w.scout, &movie.id)), StatusCode::NOT_FOUND);
        w.scout_visits(60);
        assert_eq!(list_scenes(&w.state, &w.scout, &w.hz.id).unwrap().len(), 1);

        let request = request_scene(&w.state, &w.scout, &movie.id).unwrap();
        assert_eq!(request.status, "waiting");
        assert_eq!(request.expires_at - request.created_at, 15 * 60);
        assert!(house_queue(&w.state, &home).unwrap().is_empty());
        assert_eq!(status(request_scene(&w.state, &w.ann, &movie.id)), StatusCode::CONFLICT);
        assert_eq!(ops::inbox(&w.state, &w.ann).unwrap().scenes_to_answer.len(), 1);

        // Nobody but a resident who is a person can say yes.
        assert_eq!(
            status(answer_scene_request(&w.state, &w.scout, &request.id, "yes")),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(answer_scene_request(&w.state, &w.bo, &request.id, "yes")),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(answer_scene_request(&w.state, &home, &request.id, "yes")),
            StatusCode::FORBIDDEN
        );
        let approved = answer_scene_request(&w.state, &w.ann, &request.id, "yes").unwrap();
        assert_eq!(
            (approved.status.as_str(), approved.answered_by.as_deref()),
            ("approved", Some("Ann"))
        );
        assert!(approved.expires_at - now() <= 10 * 60);

        // The house bot sees an id and a scene name, and can finish it once.
        let queue = house_queue(&w.state, &home).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(
            (queue[0].id.as_str(), queue[0].scene.as_str()),
            (request.id.as_str(), "Movie Night")
        );
        assert_eq!(serde_json::to_value(&queue[0]).unwrap().as_object().unwrap().len(), 2);
        assert_eq!(status(house_queue(&w.state, &w.ann)), StatusCode::FORBIDDEN);
        assert_eq!(status(house_queue(&w.state, &w.scout)), StatusCode::FORBIDDEN);
        assert_eq!(
            status(finish_scene_request(&w.state, &home, &request.id, "maybe")),
            StatusCode::BAD_REQUEST
        );
        finish_scene_request(&w.state, &home, &request.id, "done").unwrap();
        assert!(house_queue(&w.state, &home).unwrap().is_empty());
        assert_eq!(
            status(finish_scene_request(&w.state, &home, &request.id, "done")),
            StatusCode::CONFLICT
        );

        // A resident asking for themselves is approved on the spot.
        let own = request_scene(&w.state, &w.ann, &movie.id).unwrap();
        assert_eq!((own.status.as_str(), own.answered_by.as_deref()), ("approved", Some("Ann")));

        set_house_status(&w.state, &home, "attention").unwrap();
        assert_eq!(status(set_house_status(&w.state, &home, "locked")), StatusCode::BAD_REQUEST);
        let view = house_view(&w.state, &w.ann, &w.hz.id).unwrap();
        assert_eq!(view.status.unwrap().state, "attention");
        assert_eq!(status(house_view(&w.state, &w.bo, &w.hz.id)), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_residents_own_bot_waits_for_a_person_to_approve_a_scene() {
        let w = world();
        let helper = bot(&w.state, &w.ann, &w.hz.living_room_id, "Helper");
        let movie = add_scene(&w.state, &w.ann, &w.hz.id, "Movie Night").unwrap();
        let request = request_scene(&w.state, &helper, &movie.id).unwrap();
        assert_eq!(request.status, "waiting");
        assert_eq!(
            status(answer_scene_request(&w.state, &helper, &request.id, "yes")),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            answer_scene_request(&w.state, &w.ann, &request.id, "no").unwrap().status,
            "declined"
        );
        // A bot made in the Living Room lives in that house.
        let home = w.state.db.bot_home(helper.agent_id.as_deref().unwrap()).unwrap().unwrap();
        assert_eq!(home.house_id, w.hz.id);
    }

    #[test]
    fn the_house_bot_has_four_calls_and_no_rooms() {
        let w = world();
        let (home_agent, token) = add_house_bot(&w.state, &w.ann, &w.hz.id, "Home").unwrap();
        let home = w.state.authenticate(&auth::hash_secret(&token)).unwrap().unwrap();

        assert_eq!(status(ops::set_status(&w.state, &home, Some("hi"))), StatusCode::FORBIDDEN);
        assert_eq!(
            status(ops::report_activity(&w.state, &home, "computer", "x")),
            StatusCode::FORBIDDEN
        );
        assert_eq!(status(ops::doorbell_check(&w.state, &home)), StatusCode::FORBIDDEN);
        assert_eq!(status(ops::inbox(&w.state, &home)), StatusCode::FORBIDDEN);
        assert_eq!(
            status(ops::create_agent(&w.state, &home, &w.hz.living_room_id, "x")),
            StatusCode::FORBIDDEN
        );
        assert_eq!(status(city_view(&w.state, &home, &w.city.id)), StatusCode::FORBIDDEN);
        assert_eq!(
            status(create_trip(&w.state, &home, &w.trip_to(&w.ha, "call", None))),
            StatusCode::FORBIDDEN
        );
        let hi = ops::post_message(&w.state, &home, &w.hz.living_room_id, "chat", "hi");
        assert_eq!(status(hi), StatusCode::NOT_FOUND);
        assert_eq!(
            status(ops::snapshot(&w.state, &home, &w.hz.living_room_id)),
            StatusCode::NOT_FOUND
        );
        assert!(w.state.db.hangouts_for_agent(&home_agent.id, now()).unwrap().is_empty());

        // Its owner can't put it in a room, or send it on a trip.
        let bring = ops::bring_agent(&w.state, &w.ann, &w.hz.living_room_id, &home_agent.id);
        assert_eq!(status(bring), StatusCode::FORBIDDEN);
        let mut invite = w.trip_to(&w.ha, "visit", None);
        invite.agent_id = Some(home_agent.id.clone());
        assert_eq!(status(create_trip(&w.state, &w.bo, &invite)), StatusCode::CONFLICT);
        invite.house_id = w.hz.id.clone();
        assert_eq!(status(create_trip(&w.state, &w.ann, &invite)), StatusCode::CONFLICT);
    }

    #[test]
    fn a_guest_sees_only_what_happened_since_arrival_and_cannot_change_the_room() {
        let w = world();
        let older = now() - 1000;
        let say = |kind: &str, body: &str| {
            w.state
                .db
                .insert_message(&NewMessage {
                    hangout_id: &w.hz.porch_id,
                    person_id: None,
                    agent_id: None,
                    speaker: "Ann",
                    kind,
                    body,
                    now: older,
                })
                .unwrap();
        };
        say("chat", "before you came");
        say("note", "old note");
        let plan = ops::propose_event(
            &w.state,
            &w.ann,
            &w.hz.porch_id,
            &EventInput {
                title: "Surprise party".to_string(),
                starts_at: ops::iso(now() + 3600),
                duration_minutes: Some(60),
                ..EventInput::default()
            },
        )
        .unwrap();
        let ann_check = ops::create_check_in(
            &w.state,
            &w.ann,
            &w.hz.porch_id,
            &CheckInInput { about: "Order pizza".to_string(), ..CheckInInput::default() },
        )
        .unwrap();

        w.scout_visits(60);
        // Guests can't read the second they arrived in.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        ops::post_message(&w.state, &w.ann, &w.hz.porch_id, "chat", "welcome").unwrap();
        ops::post_message(&w.state, &w.scout, &w.hz.porch_id, "note", "new note").unwrap();

        for guest in [&w.scout, &w.bo] {
            let snapshot = ops::snapshot(&w.state, guest, &w.hz.porch_id).unwrap();
            assert!(snapshot.guest.is_some());
            let said: Vec<_> = snapshot.messages.iter().map(|m| m.body.as_str()).collect();
            assert!(!said.contains(&"before you came"), "{said:?}");
            assert!(said.contains(&"welcome"));
            let notes: Vec<_> = snapshot.notes.iter().map(|m| m.body.as_str()).collect();
            assert_eq!(notes, ["new note"]);
            assert!(snapshot.events.is_empty());
            assert!(snapshot.check_ins.is_empty());
            assert_eq!(snapshot.stats.on_calendar, 0);
            assert_eq!(snapshot.hangout.invite_code, "");
            // The room can name and colour the guest and its owner.
            assert!(snapshot.agents.iter().any(|a| a.name == "Scout" && a.visitor_until.is_some()));
            assert!(snapshot.people.iter().any(|p| p.name == "Bo"));
            // Reading with `after=0` doesn't reach further back.
            let read = ops::read_messages(&w.state, guest, &w.hz.porch_id, 0, 100).unwrap();
            assert!(read.iter().all(|m| m.body != "before you came"));
            assert!(read.iter().any(|m| m.body == "welcome"));
        }
        // The people who live here see all of it, guest included, and the guest is still no member.
        let host = ops::snapshot(&w.state, &w.ann, &w.hz.porch_id).unwrap();
        assert!(host.guest.is_none());
        assert_eq!(host.events.len(), 1);
        assert!(host.messages.iter().any(|m| m.body == "before you came"));
        assert!(host.agents.iter().any(|a| a.name == "Scout"));
        assert!(!w.state.db.people_in(&w.hz.porch_id).unwrap().iter().any(|p| p.name == "Bo"));
        assert!(w.state.db.agents_in(&w.hz.porch_id, now()).unwrap().is_empty());

        // Guests can talk, share notes and ask, and see answers to their own check-ins only.
        let mine = ops::create_check_in(
            &w.state,
            &w.scout,
            &w.hz.porch_id,
            &CheckInInput {
                about: "Borrow the ladder".to_string(),
                to: Some("Ann".to_string()),
                ..CheckInInput::default()
            },
        )
        .unwrap();
        let inbox = ops::inbox(&w.state, &w.scout).unwrap();
        assert!(inbox.waiting_on_you.is_empty());
        assert!(inbox.plans_to_answer.is_empty());
        assert_eq!(inbox.your_check_ins.len(), 1);
        ops::answer_check_in(&w.state, &w.ann, &mine.id, "go", None).unwrap();
        assert_eq!(ops::inbox(&w.state, &w.scout).unwrap().your_check_ins[0].status, "go");
        assert_eq!(ops::inbox(&w.state, &w.bo).unwrap().your_check_ins.len(), 1);

        // And nothing else: no plans, no answers, no changing who is here, no calendar files.
        for guest in [&w.scout, &w.bo] {
            let proposal = EventInput {
                title: "Mine".to_string(),
                starts_at: ops::iso(now() + 3600),
                ..EventInput::default()
            };
            assert_eq!(
                status(ops::propose_event(&w.state, guest, &w.hz.porch_id, &proposal)),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                status(ops::rsvp(&w.state, guest, &plan.id, "yes", None)),
                StatusCode::FORBIDDEN
            );
            assert_eq!(status(ops::cancel_event(&w.state, guest, &plan.id)), StatusCode::FORBIDDEN);
            let answer = ops::answer_check_in(&w.state, guest, &ann_check.id, "go", None);
            assert_eq!(status(answer), StatusCode::FORBIDDEN);
            assert_eq!(status(ops::event_ics(&w.state, guest, &plan.id)), StatusCode::FORBIDDEN);
        }
        let scout_id = w.scout_id();
        let sneak = ops::create_agent(&w.state, &w.bo, &w.hz.porch_id, "Sneak");
        assert_eq!(status(sneak), StatusCode::FORBIDDEN);
        let bring = ops::bring_agent(&w.state, &w.bo, &w.hz.porch_id, &scout_id);
        assert_eq!(status(bring), StatusCode::FORBIDDEN);
        let remove = ops::remove_agent(&w.state, &w.bo, &w.hz.porch_id, &scout_id);
        assert_eq!(status(remove), StatusCode::FORBIDDEN);
        assert_eq!(
            status(ops::rotate_invite(&w.state, &w.bo, &w.hz.porch_id)),
            StatusCode::FORBIDDEN
        );
        assert_eq!(status(ops::leave(&w.state, &w.bo, &w.hz.porch_id)), StatusCode::FORBIDDEN);
        // The Living Room stays shut to them, and no bot got a place on the Porch.
        let living = &w.hz.living_room_id;
        assert_eq!(status(ops::snapshot(&w.state, &w.scout, living)), StatusCode::NOT_FOUND);
        assert_eq!(status(ops::snapshot(&w.state, &w.bo, living)), StatusCode::NOT_FOUND);
        assert!(w.state.db.agents_in(&w.hz.porch_id, now()).unwrap().is_empty());
    }

    #[test]
    fn rooms_in_a_city_ignore_invite_links_and_cannot_be_left() {
        let w = world();
        let code = w.state.db.hangout(&w.hz.porch_id).unwrap().unwrap().invite_code;
        assert_eq!(status(ops::hangout_for_invite(&w.state, &code)), StatusCode::NOT_FOUND);
        let square = w.state.db.hangout(&w.city.square_id).unwrap().unwrap().invite_code;
        assert_eq!(status(ops::hangout_for_invite(&w.state, &square)), StatusCode::NOT_FOUND);
        let (newcomer, _) = person(&w.state, "Newcomer");
        let managed = w.state.db.hangout(&w.hz.living_room_id).unwrap().unwrap();
        assert_eq!(status(ops::join(&w.state, &newcomer, &managed)), StatusCode::NOT_FOUND);

        for room in [&w.hz.living_room_id, &w.hz.porch_id, &w.city.square_id] {
            assert_eq!(status(ops::rotate_invite(&w.state, &w.ann, room)), StatusCode::FORBIDDEN);
            assert_eq!(status(ops::leave(&w.state, &w.ann, room)), StatusCode::FORBIDDEN);
            assert_eq!(ops::snapshot(&w.state, &w.ann, room).unwrap().hangout.invite_code, "");
        }
        assert_eq!(
            status(ops::rotate_invite(&w.state, &w.eve, &w.hz.porch_id)),
            StatusCode::NOT_FOUND
        );

        // Ordinary hangouts are unchanged.
        let plain = w
            .state
            .db
            .create_hangout("plain", "Crew", "plain-code", &w.ann.person_id, now())
            .unwrap();
        assert!(ops::hangout_for_invite(&w.state, "plain-code").is_ok());
        assert_eq!(
            ops::snapshot(&w.state, &w.ann, &plain.id).unwrap().hangout.invite_code,
            "plain-code"
        );
        assert!(ops::rotate_invite(&w.state, &w.ann, &plain.id).is_ok());
    }

    #[test]
    fn houses_and_cities_have_the_membership_rules_they_say() {
        let w = world();
        // One house per person per city, and a resident can't leave the city.
        assert_eq!(
            status(build_house(&w.state, &w.ann, &w.city.id, "Second")),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status(build_house(&w.state, &w.eve, &w.city.id, "Eve's")),
            StatusCode::NOT_FOUND
        );
        assert_eq!(status(leave_city(&w.state, &w.ann, &w.city.id)), StatusCode::CONFLICT);
        assert_eq!(status(leave_house(&w.state, &w.ann, &w.hz.id)), StatusCode::CONFLICT);
        assert_eq!(status(rotate_city_invite(&w.state, &w.bo, &w.city.id)), StatusCode::FORBIDDEN);
        assert_eq!(status(rotate_city_invite(&w.state, &w.eve, &w.city.id)), StatusCode::NOT_FOUND);
        let old_code = w.city.invite_code.clone();
        let url = rotate_city_invite(&w.state, &w.ann, &w.city.id).unwrap();
        assert!(url.starts_with("http://test/join-city/"), "{url}");
        assert_eq!(status(city_invite_preview(&w.state, &old_code)), StatusCode::NOT_FOUND);

        // Joining a house joins the city, and the house's rooms.
        let (zoe_person, zoe) = person(&w.state, "Zoe");
        let house_code = w.state.db.house(&w.hz.id).unwrap().unwrap().invite_code;
        let house = join_house(&w.state, &zoe_person, &house_code, false).unwrap();
        assert_eq!(house.id, w.hz.id);
        assert!(w.state.db.is_city_member(&w.city.id, &zoe.person_id).unwrap());
        assert!(w.state.in_room(&zoe, &w.hz.living_room_id).unwrap());
        assert!(w.state.in_room(&zoe, &w.city.square_id).unwrap());
        let nowhere = join_house(&w.state, &zoe_person, "nonsense", false);
        assert_eq!(status(nowhere), StatusCode::NOT_FOUND);
        // She is a second resident, so she can go, and then she can leave the city too.
        leave_house(&w.state, &zoe, &w.hz.id).unwrap();
        assert!(!w.state.in_room(&zoe, &w.hz.living_room_id).unwrap());
        leave_city(&w.state, &zoe, &w.city.id).unwrap();
        assert!(!w.state.in_room(&zoe, &w.city.square_id).unwrap());
    }

    #[test]
    fn a_bot_that_stays_home_keeps_one_room_and_cannot_travel() {
        let w = world();
        let scout_id = w.scout_id();
        ops::bring_agent(&w.state, &w.bo, &w.city.square_id, &scout_id).unwrap();
        let trip = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, "call", None)).unwrap();
        // Only the owner, and only for a house they live in.
        assert_eq!(
            status(set_bot_home(&w.state, &w.ann, &scout_id, &w.hz.id, true)),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(set_bot_home(&w.state, &w.bo, &scout_id, &w.hz.id, true)),
            StatusCode::NOT_FOUND
        );
        set_bot_home(&w.state, &w.bo, &scout_id, &w.ha.id, true).unwrap();

        assert!(!w.state.in_room(&w.scout, &w.city.square_id).unwrap());
        assert!(w.state.in_room(&w.scout, &w.ha.living_room_id).unwrap());
        assert_eq!(get_trip(&w.state, &w.bo, &trip.id).unwrap().status, "declined");
        let again = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, "call", None));
        assert_eq!(status(again), StatusCode::CONFLICT);
        let square = ops::bring_agent(&w.state, &w.bo, &w.city.square_id, &scout_id);
        assert_eq!(status(square), StatusCode::CONFLICT);
        // Its own house's Living Room is fine, its Porch is not.
        assert!(ops::bring_agent(&w.state, &w.bo, &w.ha.living_room_id, &scout_id).is_ok());
        let porch = ops::bring_agent(&w.state, &w.bo, &w.ha.porch_id, &scout_id);
        assert_eq!(status(porch), StatusCode::CONFLICT);
        // Turning it off lets the bot travel and go anywhere again.
        set_bot_home(&w.state, &w.bo, &scout_id, &w.ha.id, false).unwrap();
        assert!(ops::bring_agent(&w.state, &w.bo, &w.city.square_id, &scout_id).is_ok());
    }

    #[test]
    fn the_city_shows_where_bots_are_and_a_road_once_one_arrives() {
        let w = world();
        let before = city_view(&w.state, &w.bo, &w.city.id).unwrap();
        assert_eq!(before.growth.stage, "hamlet");
        assert!(before.growth.roads.is_empty());
        assert!(before.city.invite_url.is_none());
        let creator = city_view(&w.state, &w.ann, &w.city.id).unwrap();
        assert!(creator.city.invite_url.unwrap().contains("/join-city/"));
        assert_eq!(before.houses.len(), 2);
        assert_eq!(status(city_view(&w.state, &w.eve, &w.city.id)), StatusCode::NOT_FOUND);
        // A bot sees the city when its owner is a member, but never the invite.
        assert!(city_view(&w.state, &w.scout, &w.city.id).unwrap().city.invite_url.is_none());

        w.scout_visits(60);
        let after = city_view(&w.state, &w.ann, &w.city.id).unwrap();
        let scout = after.bots.iter().find(|bot| bot.name == "Scout").unwrap();
        assert_eq!(scout.home_house_id.as_deref(), Some(w.ha.id.as_str()));
        assert_eq!(scout.away_at_house_id.as_deref(), Some(w.hz.id.as_str()));
        assert_eq!(after.growth.roads.len(), 1);
        assert_eq!(after.growth.roads[0].trips, 1);
        // Nothing a bot wrote or a resident configured is on the map.
        let json = serde_json::to_string(&after).unwrap();
        for secret in ["Pair on the build", "living_room", "porch", "invite_code"] {
            assert!(!json.contains(secret), "{secret} in {json}");
        }
    }

    #[test]
    fn extending_is_for_residents_and_stops_at_the_kinds_limit() {
        let w = world();
        let trip = w.scout_visits(200);
        assert_eq!(status(extend_trip(&w.state, &w.bo, &trip.id, 10)), StatusCode::FORBIDDEN);
        assert_eq!(status(extend_trip(&w.state, &w.scout, &trip.id, 10)), StatusCode::FORBIDDEN);
        assert_eq!(status(extend_trip(&w.state, &w.eve, &trip.id, 10)), StatusCode::NOT_FOUND);
        assert_eq!(status(extend_trip(&w.state, &w.ann, &trip.id, 0)), StatusCode::BAD_REQUEST);
        // A call is four hours at most, counting from when it began.
        assert_eq!(status(extend_trip(&w.state, &w.ann, &trip.id, 41)), StatusCode::BAD_REQUEST);
        let longer = extend_trip(&w.state, &w.ann, &trip.id, 40).unwrap();
        assert_eq!(longer.ends_at.unwrap() - longer.arrived_at.unwrap(), 240 * 60);
        assert_eq!(status(extend_trip(&w.state, &w.ann, &trip.id, 1)), StatusCode::BAD_REQUEST);
    }
}
