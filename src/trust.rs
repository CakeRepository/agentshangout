//! Standing, vouching and who gets close: the rules T1 to T21 in docs/accounts-and-trust.md. The
//! REST API and the MCP tools both call these functions, so the rules live here or in `city`.
//!
//! Standing is counted on read from what happened (T1), so nothing here stores a score. What is
//! stored is what people chose: a house's requirements, a city's rules, vouches, who is waiting at
//! a city's gate, and which trips a resident ended by sending the bot home. Meeting a requirement
//! only lets someone ask (T8): a person still has to say yes.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::{
    auth::Caller,
    city,
    db::{City, EntryRules, GateAsk, House, Person, Trip},
    error::{ApiError, ApiResult},
    now,
    ops::{self, DAY_SECS},
    AppState,
};

/// Largest each requirement can be. A city only has `MAX_VOUCHES` to set, and not more than the
/// people in it.
pub const MAX_MEMBER_DAYS: i64 = 365;
pub const MAX_GATHERINGS: i64 = 100;
pub const MAX_GOOD_TRIPS: i64 = 100;
pub const MAX_VOUCHES: i64 = 5;
pub const MAX_RULES_CHARS: usize = 1_000;
/// A place at a city's gate lapses after this long (T18).
pub const GATE_LAPSE_SECS: i64 = 14 * DAY_SECS;
/// The most places a gate holds at once (T18). A leaked link can't fill it any further.
pub const GATE_CAPACITY: i64 = 50;

// What the pages are shown

/// What someone has done in a city (T1). Only counts and the names of people who vouch: no plan
/// titles, trip goals or house names (T4).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Standing {
    /// When they joined the city. Nothing for someone still at the gate.
    pub member_since: Option<i64>,
    /// Whole days since then. 0 at the gate.
    pub member_days: i64,
    /// Gatherings (R40) in this city that they said yes to.
    pub gatherings: i64,
    /// Trips by their bots to houses in this city that arrived, are over, and weren't ended by a
    /// resident sending the bot home.
    pub good_trips: i64,
    /// Trips of theirs here that a resident ended that way.
    pub sent_home: i64,
    /// People in the city now who vouch for them.
    pub vouches: Vec<String>,
}

/// What a house asks of anyone who wants to send a bot to visit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Requirements {
    pub min_member_days: i64,
    pub min_gatherings: i64,
    pub min_good_trips: i64,
    pub min_vouches: i64,
}

impl Requirements {
    fn from_rules(rules: &EntryRules) -> Self {
        Self {
            min_member_days: rules.min_member_days,
            min_gatherings: rules.min_gatherings,
            min_good_trips: rules.min_good_trips,
            min_vouches: rules.min_vouches,
        }
    }
}

/// What a city asks of anyone who comes by its link.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CityRules {
    pub min_vouches: i64,
    pub rules_text: String,
}

/// Someone waiting at a city's gate, as the people inside see them (T16).
#[derive(Debug, Serialize)]
pub struct GateEntry {
    pub person_id: String,
    pub name: String,
    pub color: String,
    pub vouches: i64,
    pub you_vouched: bool,
    pub asked_at: i64,
}

/// A city someone is waiting at the gate of, for `GET /api/me` (T20): its name and their count,
/// and nothing else about it.
#[derive(Debug, Serialize)]
pub struct Waiting {
    pub city_id: String,
    pub name: String,
    pub vouches: i64,
    pub vouches_needed: i64,
}

/// How using a city's link came out.
#[derive(Debug)]
pub struct Joined {
    pub city: City,
    /// They're at the gate, not in the city.
    pub waiting: bool,
    pub vouches: i64,
    pub vouches_needed: i64,
}

/// What a vouch did.
#[derive(Debug)]
pub struct Vouched {
    pub vouches: i64,
    pub let_in: bool,
}

/// A house's requirements as sent. A number left out stays as it was.
#[derive(Debug, Default, Deserialize)]
pub struct HouseRulesInput {
    pub min_member_days: Option<i64>,
    pub min_gatherings: Option<i64>,
    pub min_good_trips: Option<i64>,
    pub min_vouches: Option<i64>,
}

/// A city's rules as sent. A field left out stays as it was. The other requirements can be sent
/// too, but only as 0: a newcomer has no history in the city to count (T12).
#[derive(Debug, Default, Deserialize)]
pub struct CityRulesInput {
    pub min_vouches: Option<i64>,
    pub rules_text: Option<String>,
    pub min_member_days: Option<i64>,
    pub min_gatherings: Option<i64>,
    pub min_good_trips: Option<i64>,
}

fn gate_cutoff(now: i64) -> i64 {
    now - GATE_LAPSE_SECS
}

// Standing

/// Worked out now from what's in the database (T1).
pub fn standing(state: &AppState, city_id: &str, person_id: &str, now: i64) -> ApiResult<Standing> {
    let counts = state.db.standing_counts(city_id, person_id, now)?;
    Ok(Standing {
        member_days: counts.member_since.map_or(0, |since| (now - since).max(0) / DAY_SECS),
        member_since: counts.member_since,
        gatherings: counts.gatherings,
        good_trips: counts.good_trips,
        sent_home: counts.sent_home,
        vouches: counts.vouchers,
    })
}

/// Whether this person is in the city (false) or waiting at its gate (true). Anyone else, and a
/// place that has lapsed, is a 404: the city doesn't know them.
fn member_or_waiting(state: &AppState, city: &City, person_id: &str, now: i64) -> ApiResult<bool> {
    // The gate is read first. Being let in leaves the gate and joins the city in one go, so someone
    // let in between these two reads is found in the city, and never looks like a stranger.
    let waiting = state.db.gate_asked_at(&city.id, person_id, gate_cutoff(now))?.is_some();
    if state.db.is_city_member(&city.id, person_id)? {
        return Ok(false);
    }
    if waiting {
        return Ok(true);
    }
    Err(ApiError::not_found())
}

/// `GET /api/cities/{id}/people/{person_id}/standing` (T5): people in the city and their bots. A
/// stranger gets the 404 a city that doesn't exist would give.
pub fn standing_of(
    state: &AppState,
    caller: &Caller,
    city_id: &str,
    person_id: &str,
) -> ApiResult<Standing> {
    let city = city::require_city(state, caller, city_id)?;
    let now = now();
    member_or_waiting(state, &city, person_id, now)?;
    standing(state, &city.id, person_id, now)
}

/// A standing for each person asking to visit a house (T10), keyed by the bot owner's person id.
pub fn asker_standings(
    state: &AppState,
    house: &House,
    asking: &[Trip],
    now: i64,
) -> ApiResult<BTreeMap<String, Standing>> {
    let mut standings = BTreeMap::new();
    for trip in asking {
        if !standings.contains_key(&trip.owner_id) {
            let found = standing(state, &house.city_id, &trip.owner_id, now)?;
            standings.insert(trip.owner_id.clone(), found);
        }
    }
    Ok(standings)
}

// Houses

pub fn house_requirements(state: &AppState, house_id: &str) -> ApiResult<Requirements> {
    Ok(Requirements::from_rules(&state.db.entry_rules(house_id)?))
}

/// What each house in the city asks for, by house id. A house that asks for nothing isn't in it.
pub fn requirements_by_house(
    state: &AppState,
    city_id: &str,
) -> ApiResult<HashMap<String, Requirements>> {
    Ok(state
        .db
        .house_rules_in_city(city_id)?
        .into_iter()
        .map(|(house_id, rules)| (house_id, Requirements::from_rules(&rules)))
        .collect())
}

fn plural(count: i64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// What's missing, in the house's own terms with the asker's own numbers, or `None` if they meet
/// every requirement. For example "Kevin's Loft asks for 2 gatherings (Dan has 0) and 7 days in the
/// city (Dan has 3)".
fn shortfall(house: &str, who: &str, need: &Requirements, have: &Standing) -> Option<String> {
    let have_vouches = have.vouches.len() as i64;
    let mut missing = Vec::new();
    if have.gatherings < need.min_gatherings {
        let asks = plural(need.min_gatherings, "gathering", "gatherings");
        missing.push(format!("{asks} ({who} has {})", have.gatherings));
    }
    if have.member_days < need.min_member_days {
        let asks = plural(need.min_member_days, "day", "days");
        missing.push(format!("{asks} in the city ({who} has {})", have.member_days));
    }
    if have.good_trips < need.min_good_trips {
        let asks = plural(need.min_good_trips, "good trip", "good trips");
        missing.push(format!("{asks} ({who} has {})", have.good_trips));
    }
    if have_vouches < need.min_vouches {
        let asks = plural(need.min_vouches, "vouch", "vouches");
        missing.push(format!("{asks} ({who} has {have_vouches})"));
    }
    let last = missing.pop()?;
    let list = if missing.is_empty() { last } else { format!("{} and {last}", missing.join(", ")) };
    Some(format!("{house} asks for {list}"))
}

/// T7: a bot's owner has to meet the house's requirements before a trip can be asked for. A 409
/// says what's missing, the same to a bot as to a person. `create_trip` skips this when a resident
/// invites the bot or an accepted pass covers the trip (T9), because the house already chose.
pub fn check_requirements(
    state: &AppState,
    house: &House,
    owner_id: &str,
    now: i64,
) -> ApiResult<()> {
    let need = house_requirements(state, &house.id)?;
    if need == Requirements::default() {
        return Ok(());
    }
    let have = standing(state, &house.city_id, owner_id, now)?;
    let who = state.db.person(owner_id)?.map_or_else(|| "they".to_string(), |person| person.name);
    match shortfall(&house.name, &who, &need, &have) {
        Some(missing) => Err(ApiError::conflict(missing)),
        None => Ok(()),
    }
}

/// T11: a trip a resident ended is marked, so it doesn't count as a good one and shows as sent
/// home. Nothing else goes through here: not the bot going home, its owner calling it back, a
/// pass being cancelled, a person leaving a city, `stays_home` or time running out.
pub fn note_trip_ended(
    state: &AppState,
    trip_id: &str,
    by_a_resident: bool,
    now: i64,
) -> ApiResult<()> {
    if by_a_resident {
        state.db.record_sent_home(trip_id, now)?;
    }
    Ok(())
}

/// A number from the request, or the one already set if it was left out. Out of range is a 400.
fn limited(given: Option<i64>, current: i64, field: &str, max: i64) -> ApiResult<i64> {
    let value = given.unwrap_or(current);
    if !(0..=max).contains(&value) {
        return Err(ApiError::bad_request(format!("{field} must be between 0 and {max}")));
    }
    Ok(value)
}

/// T6: the house, if the caller is a resident who is a person. A bot can't set requirements, the
/// house bot included, and anyone else gets 404. This comes before reading the request body, so
/// they're turned away whatever they sent.
pub fn house_for_rules(state: &AppState, caller: &Caller, house_id: &str) -> ApiResult<House> {
    city::resident_house(state, caller, house_id, "set what a house asks for")
}

/// Sets what the house asks for. Changing it never ends a trip or a pass (T9).
pub fn set_house_rules(state: &AppState, house: &House, input: &HouseRulesInput) -> ApiResult<()> {
    let current = state.db.entry_rules(&house.id)?;
    let rules = EntryRules {
        min_member_days: limited(
            input.min_member_days,
            current.min_member_days,
            "min_member_days",
            MAX_MEMBER_DAYS,
        )?,
        min_gatherings: limited(
            input.min_gatherings,
            current.min_gatherings,
            "min_gatherings",
            MAX_GATHERINGS,
        )?,
        min_good_trips: limited(
            input.min_good_trips,
            current.min_good_trips,
            "min_good_trips",
            MAX_GOOD_TRIPS,
        )?,
        min_vouches: limited(input.min_vouches, current.min_vouches, "min_vouches", MAX_VOUCHES)?,
        rules_text: current.rules_text,
    };
    state.db.set_entry_rules(&house.id, &rules, now())?;
    city::refresh_house(state, house);
    ops::refresh_city(state, &house.city_id);
    Ok(())
}

// Cities

pub fn city_rules(state: &AppState, city_id: &str) -> ApiResult<CityRules> {
    let rules = state.db.entry_rules(city_id)?;
    Ok(CityRules { min_vouches: rules.min_vouches, rules_text: rules.rules_text })
}

/// T12: the city, if the caller looks after its invite link. A stranger gets 404, a bot or anyone
/// else in the city gets 403. This comes before reading the request body, so they're turned away
/// whatever they sent.
pub fn city_for_rules(state: &AppState, caller: &Caller, city_id: &str) -> ApiResult<City> {
    let city = city::require_city(state, caller, city_id)?;
    ops::require_human(caller, "set a city's rules")?;
    if !city::holds_city_link(state, caller, &city)? {
        return Err(ApiError::forbidden("only the person who started the city can set its rules"));
    }
    Ok(city)
}

/// T12 and T13: sets what newcomers must do. A city asks for vouches and for its rules to be
/// accepted, and nothing else. Lowering `min_vouches` lets in everyone at the gate who now has
/// enough (T21), and 0 lets in everyone.
pub fn set_city_rules(state: &AppState, city: &City, input: &CityRulesInput) -> ApiResult<()> {
    let others = [
        ("min_member_days", input.min_member_days),
        ("min_gatherings", input.min_gatherings),
        ("min_good_trips", input.min_good_trips),
    ];
    if let Some((field, _)) = others.iter().find(|(_, value)| value.unwrap_or(0) != 0) {
        return Err(ApiError::bad_request(format!(
            "a city can only ask for vouches and have rules to accept, so {field} stays 0"
        )));
    }

    let current = state.db.entry_rules(&city.id)?;
    let min_vouches = limited(input.min_vouches, current.min_vouches, "min_vouches", MAX_VOUCHES)?;
    if min_vouches != current.min_vouches {
        let people = state.db.city_members(&city.id)?.len() as i64;
        if min_vouches > people {
            return Err(ApiError::bad_request(format!(
                "{} has {}, so it can't ask for more than {}",
                city.name,
                plural(people, "person", "people"),
                plural(people, "vouch", "vouches"),
            )));
        }
    }
    let rules_text = match &input.rules_text {
        Some(text) => ops::clean_optional(Some(text), "rules_text", MAX_RULES_CHARS)?,
        None => current.rules_text,
    };
    let rules = EntryRules { min_vouches, rules_text, ..EntryRules::default() };
    let now = now();
    state.db.set_entry_rules(&city.id, &rules, now)?;
    // Anyone who already has enough gets in now, so lowering the bar never leaves people waiting.
    for person_id in state.db.gate_ids(&city.id, gate_cutoff(now))? {
        admit_if_enough(state, city, &person_id, now)?;
    }
    ops::refresh_city(state, &city.id);
    Ok(())
}

/// Lets someone in from the gate if they have enough vouches, and tells the Town Square and them.
/// Returns whether it did.
fn admit_if_enough(state: &AppState, city: &City, person_id: &str, now: i64) -> ApiResult<bool> {
    if !state.db.let_in_if_enough(&city.id, person_id, now, gate_cutoff(now))? {
        return Ok(false);
    }
    if let Some(person) = state.db.person(person_id)? {
        ops::announce(state, &city.square_id, &format!("{} joined {}.", person.name, city.name))?;
        let text = format!("You're in: you've been let into {}.", city.name);
        city::ping(state, vec![person.id], "city.let_in", &city.square_id, &text);
    }
    ops::refresh(state, &city.square_id);
    ops::refresh_city(state, &city.id);
    Ok(true)
}

/// T14: someone who isn't in the city yet has to accept its rules, if it has any, whichever link
/// they come in by. Answers 409 if they haven't.
pub fn check_rules_accepted(state: &AppState, city: &City, accept_rules: bool) -> ApiResult<()> {
    if !accept_rules && !state.db.entry_rules(&city.id)?.rules_text.is_empty() {
        return Err(ApiError::conflict(format!(
            "{} has rules: read them, then join again with accept_rules set to true",
            city.name
        )));
    }
    Ok(())
}

/// What's asked of someone before they can use a city's link, if they aren't in yet. It answers
/// 409 when the city has rules and they weren't accepted (T14), or when its gate is full (T18).
fn check_ready(
    state: &AppState,
    city: &City,
    rules: &EntryRules,
    accept_rules: bool,
    now: i64,
) -> ApiResult<()> {
    check_rules_accepted(state, city, accept_rules)?;
    if rules.min_vouches > 0
        && state.db.gate_ids(&city.id, gate_cutoff(now))?.len() as i64 >= GATE_CAPACITY
    {
        return Err(ApiError::conflict(format!(
            "the gate at {} is full: try again later",
            city.name
        )));
    }
    Ok(())
}

/// For someone who isn't a person yet: checked before one is made, so a refused join leaves no
/// account behind.
pub fn check_new_person_may_join(
    state: &AppState,
    city: &City,
    accept_rules: bool,
) -> ApiResult<()> {
    let rules = state.db.entry_rules(&city.id)?;
    check_ready(state, city, &rules, accept_rules, now())
}

/// Someone uses the city's link (T14, T15). With no vouches asked for they're in, as they always
/// were. Otherwise they wait at the gate: they have an account, but the city stays a 404 until
/// enough people in it vouch for them. Using the link again is harmless.
pub fn enter_city(
    state: &AppState,
    person: &Person,
    city: &City,
    accept_rules: bool,
) -> ApiResult<Joined> {
    let now = now();
    let rules = state.db.entry_rules(&city.id)?;
    let joined = |waiting: bool| -> ApiResult<Joined> {
        Ok(Joined {
            city: city.clone(),
            waiting,
            vouches: state.db.vouch_count(&city.id, &person.id)?,
            vouches_needed: rules.min_vouches,
        })
    };
    if state.db.is_city_member(&city.id, &person.id)? {
        return joined(false);
    }
    let waiting = state.db.gate_asked_at(&city.id, &person.id, gate_cutoff(now))?.is_some();
    if !waiting {
        check_ready(state, city, &rules, accept_rules, now)?;
    }
    if rules.min_vouches == 0 {
        state.db.add_city_member(&city.id, &person.id, now)?;
        ops::announce(state, &city.square_id, &format!("{} joined {}.", person.name, city.name))?;
        ops::refresh(state, &city.square_id);
        ops::refresh_city(state, &city.id);
        return joined(false);
    }
    match state.db.gate_ask(&city.id, &person.id, now, gate_cutoff(now), GATE_CAPACITY)? {
        GateAsk::Full => {
            return Err(ApiError::conflict(format!(
                "the gate at {} is full: try again later",
                city.name
            )));
        }
        GateAsk::Added => {
            // The keeper hears about it, not everyone: a link that leaks shouldn't buzz the whole
            // city once for each person who tries it.
            let text = format!("{} is waiting at the gate of {}.", person.name, city.name);
            city::ping(state, keepers(state, city)?, "city.gate", &city.square_id, &text);
            ops::refresh_city(state, &city.id);
        }
        GateAsk::AlreadyThere => {}
    }
    // Vouches from before still count, say if their place lapsed and they came back.
    if admit_if_enough(state, city, &person.id, now)? {
        return joined(false);
    }
    joined(true)
}

/// Who looks after the city's invite link: the person who started it, or every member once they've
/// left.
fn keepers(state: &AppState, city: &City) -> ApiResult<Vec<String>> {
    if state.db.is_city_member(&city.id, &city.created_by)? {
        return Ok(vec![city.created_by.clone()]);
    }
    Ok(state.db.city_members(&city.id)?.into_iter().map(|person| person.id).collect())
}

/// T16: who is at the gate, for the people in the city. Their bots don't see it.
pub fn gate_view(
    state: &AppState,
    caller: &Caller,
    city_id: &str,
) -> ApiResult<Option<Vec<GateEntry>>> {
    if caller.is_agent() {
        return Ok(None);
    }
    let rows = state.db.gate_rows(city_id, &caller.person_id, gate_cutoff(now()))?;
    Ok(Some(
        rows.into_iter()
            .map(|row| GateEntry {
                person_id: row.person.id,
                name: row.person.name,
                color: row.person.color,
                vouches: row.vouches,
                you_vouched: row.you_vouched,
                asked_at: row.asked_at,
            })
            .collect(),
    ))
}

/// T20: the cities this person is waiting at the gate of, for `GET /api/me`.
pub fn waiting(state: &AppState, person_id: &str) -> ApiResult<Vec<Waiting>> {
    Ok(state
        .db
        .gates_of_person(person_id, gate_cutoff(now()))?
        .into_iter()
        .map(|place| Waiting {
            city_id: place.city_id,
            name: place.city_name,
            vouches: place.vouches,
            vouches_needed: place.vouches_needed,
        })
        .collect())
}

// Vouching

/// T3: a person in the city vouches for someone in it or at its gate, once, and never for
/// themselves. When that is enough for someone at the gate they're let in at once (T17).
pub fn vouch(
    state: &AppState,
    caller: &Caller,
    city_id: &str,
    person_id: &str,
) -> ApiResult<Vouched> {
    let city = city::require_city(state, caller, city_id)?;
    ops::require_human(caller, "vouch for someone")?;
    let now = now();
    let at_gate = member_or_waiting(state, &city, person_id, now)?;
    if person_id == caller.person_id {
        return Err(ApiError::bad_request("you can't vouch for yourself"));
    }
    state.db.add_vouch(&city.id, person_id, &caller.person_id, now)?;
    let let_in = at_gate && admit_if_enough(state, &city, person_id, now)?;
    ops::refresh_city(state, &city.id);
    Ok(Vouched { vouches: state.db.vouch_count(&city.id, person_id)?, let_in })
}

/// Takes back the caller's own vouch. It never removes anyone from a city, and taking back one that
/// isn't there is harmless.
pub fn withdraw_vouch(
    state: &AppState,
    caller: &Caller,
    city_id: &str,
    person_id: &str,
) -> ApiResult<()> {
    let city = city::require_city(state, caller, city_id)?;
    ops::require_human(caller, "take back a vouch")?;
    state.db.remove_vouch(&city.id, person_id, &caller.person_id)?;
    ops::refresh_city(state, &city.id);
    Ok(())
}

/// The city, if the caller looks after its invite link, which is who turns people away from the
/// gate (T17). A stranger gets 404, a bot or anyone else in the city gets 403.
fn city_for_gate(state: &AppState, caller: &Caller, city_id: &str) -> ApiResult<City> {
    let city = city::require_city(state, caller, city_id)?;
    ops::require_human(caller, "turn people away")?;
    if !city::holds_city_link(state, caller, &city)? {
        return Err(ApiError::forbidden(
            "only the person who started the city can turn people away",
        ));
    }
    Ok(city)
}

/// T17: whoever looks after the city's invite link turns someone at the gate away.
pub fn decline(state: &AppState, caller: &Caller, city_id: &str, person_id: &str) -> ApiResult<()> {
    let city = city_for_gate(state, caller, city_id)?;
    if !state.db.gate_remove(&city.id, person_id, gate_cutoff(now()))? {
        return Err(ApiError::not_found());
    }
    ops::refresh_city(state, &city.id);
    Ok(())
}

/// T17: the same keeper turns everyone at the gate away at once, which is how a leaked link that
/// filled the gate is cleared. Returns how many.
pub fn clear_gate(state: &AppState, caller: &Caller, city_id: &str) -> ApiResult<usize> {
    let city = city_for_gate(state, caller, city_id)?;
    let cleared = state.db.gate_clear(&city.id, gate_cutoff(now()))?;
    ops::refresh_city(state, &city.id);
    Ok(cleared)
}

#[cfg(test)]
mod tests {
    //! The core flows, driven with `Caller`s: what standing counts, each requirement passing and
    //! failing, the gate, the rules and `sent_home`. A few tests go over a real socket to prove the
    //! routes reach the same rules. The adversarial suite is in `tests/`.

    use std::sync::{Arc, Mutex};

    use axum::http::StatusCode;
    use serde_json::{json, Value};

    use super::*;
    use crate::{
        auth,
        city::{
            self, accept_pass, answer_trip, build_house, create_city, create_trip, end_trip,
            join_city, join_house, leave_city, offer_pass, PassInput, TripInput,
        },
        db::{Db, NewEvent, Person},
        Config,
    };

    fn status<T: std::fmt::Debug>(result: ApiResult<T>) -> StatusCode {
        result.unwrap_err().status
    }

    fn message<T: std::fmt::Debug>(result: ApiResult<T>) -> String {
        result.unwrap_err().message
    }

    fn new_state() -> AppState {
        AppState::new(
            Db::open(":memory:").unwrap(),
            Config {
                public_url: "http://test".to_string(),
                create_code: None,
                allow_private_webhooks: true,
                keeper_id: None,
            },
        )
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

    /// Ann started the city Elm and built Ann's House. Bo joined and built Bo's House, where his
    /// bot Scout lives.
    struct World {
        state: AppState,
        city: City,
        ann_p: Person,
        ann: Caller,
        bo_p: Person,
        bo: Caller,
        hz: House,
        ha: House,
        scout: Caller,
    }

    fn world() -> World {
        let state = new_state();
        let (ann_p, ann) = person(&state, "Ann");
        let (bo_p, bo) = person(&state, "Bo");
        let city = create_city(&state, &ann_p, "Elm").unwrap();
        join_city(&state, &bo_p, &city.invite_code, false).unwrap();
        let hz = build_house(&state, &ann, &city.id, "Ann's House").unwrap();
        let ha = build_house(&state, &bo, &city.id, "Bo's House").unwrap();
        let scout = bot(&state, &bo, &ha.living_room_id, "Scout");
        World { state, city, ann_p, ann, bo_p, bo, hz, ha, scout }
    }

    impl World {
        fn trip_to(&self, house: &House, minutes: i64) -> TripInput {
            TripInput {
                agent_id: None,
                house_id: house.id.clone(),
                kind: "call".to_string(),
                goal: Some("Pair on the build".to_string()),
                minutes: Some(minutes),
            }
        }

        fn scout_id(&self) -> String {
            self.scout.agent_id.clone().unwrap()
        }

        /// Scout visits Ann's House: asks, then Bo and Ann both say yes.
        fn scout_visits(&self, minutes: i64) -> crate::db::Trip {
            let trip =
                create_trip(&self.state, &self.scout, &self.trip_to(&self.hz, minutes)).unwrap();
            answer_trip(&self.state, &self.bo, &trip.id, "yes").unwrap();
            answer_trip(&self.state, &self.ann, &trip.id, "yes").unwrap()
        }

        /// Someone who has been in the city for this many days and lives in no house.
        fn member(&self, name: &str, days: i64) -> (Person, Caller) {
            let (person, caller) = person(&self.state, name);
            let joined = now() - days * DAY_SECS - 60;
            self.state.db.add_city_member(&self.city.id, &person.id, joined).unwrap();
            (person, caller)
        }

        /// A plan in a room that ended `ended_ago` seconds ago (a negative number is still to
        /// come), with a yes from each person named.
        fn plan(&self, room: &str, status: &str, ended_ago: i64, yes: &[&str]) -> String {
            let id = auth::new_id();
            let at = now();
            self.state
                .db
                .insert_event(&NewEvent {
                    id: &id,
                    hangout_id: room,
                    title: "A secret plan title",
                    location: "",
                    notes: "",
                    starts_at: at - ended_ago - 3600,
                    ends_at: at - ended_ago,
                    min_yes: 1,
                    proposed_by: &self.ann_p.id,
                    proposer: "Ann",
                    now: at - ended_ago - 7200,
                })
                .unwrap();
            if status != "proposed" {
                self.state.db.set_event_status(&id, status, at).unwrap();
            }
            for person in yes {
                self.state.db.upsert_rsvp(&id, person, "yes", None, "", at).unwrap();
            }
            id
        }

        fn standing(&self, person_id: &str) -> Standing {
            standing(&self.state, &self.city.id, person_id, now()).unwrap()
        }

        /// A house's requirements, set the way the route sets them.
        fn house_asks(&self, who: &Caller, house: &House, input: HouseRulesInput) -> ApiResult<()> {
            let found = house_for_rules(&self.state, who, &house.id)?;
            set_house_rules(&self.state, &found, &input)
        }

        /// The city's rules, set the way the route sets them.
        fn city_asks(&self, who: &Caller, input: CityRulesInput) -> ApiResult<()> {
            let found = city_for_rules(&self.state, who, &self.city.id)?;
            set_city_rules(&self.state, &found, &input)
        }

        fn gate(&self, who: &Caller) -> Vec<GateEntry> {
            city::city_view(&self.state, who, &self.city.id).unwrap().gate.unwrap()
        }

        /// A newcomer uses the city's link, signed in as a person who isn't in the city.
        fn arrives(&self, name: &str) -> (Person, Caller, Joined) {
            let (person, caller) = person(&self.state, name);
            let city = self.state.db.city(&self.city.id).unwrap().unwrap();
            let joined = enter_city(&self.state, &person, &city, true).unwrap();
            (person, caller, joined)
        }
    }

    fn asks(input: [(&str, i64); 4]) -> HouseRulesInput {
        let get = |name: &str| input.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
        HouseRulesInput {
            min_member_days: get("days"),
            min_gatherings: get("gatherings"),
            min_good_trips: get("trips"),
            min_vouches: get("vouches"),
        }
    }

    // Standing

    #[test]
    fn standing_counts_the_gatherings_a_person_said_yes_to() {
        let w = world();
        let square = w.city.square_id.clone();
        let (dan_p, _) = w.member("Dan", 8);
        let (ann, bo, dan) = (&w.ann_p.id, &w.bo_p.id, &dan_p.id);

        w.plan(&square, "confirmed", 3600, &[ann, bo]);
        // R40: not a gathering unless two people said yes, and it's confirmed, over, in the Town
        // Square.
        w.plan(&square, "confirmed", 3600, &[ann]);
        w.plan(&square, "cancelled", 3600, &[ann, bo]);
        w.plan(&square, "proposed", 3600, &[ann, bo]);
        w.plan(&square, "confirmed", -3600, &[ann, bo]);
        w.plan(&w.hz.living_room_id, "confirmed", 3600, &[ann, bo]);
        w.plan(&w.hz.porch_id, "confirmed", 3600, &[ann, bo]);
        // Bo only said maybe to this one, so it's Ann's and Dan's.
        let maybe = w.plan(&square, "confirmed", 7200, &[ann, dan]);
        w.state.db.upsert_rsvp(&maybe, bo, "maybe", None, "", now()).unwrap();

        assert_eq!(w.state.db.gatherings(&w.city.id, now()).unwrap(), 2);
        assert_eq!(w.standing(ann).gatherings, 2);
        assert_eq!(w.standing(bo).gatherings, 1);
        assert_eq!(w.standing(dan).gatherings, 1);

        // A yes a bot gave for its owner is the owner's yes: it counts like the rest of R40.
        let by_bot = w.plan(&square, "proposed", 3600, &[ann]);
        w.state.db.upsert_rsvp(&by_bot, bo, "yes", Some("Scout"), "", now()).unwrap();
        w.state.db.set_event_status(&by_bot, "confirmed", now()).unwrap();
        assert_eq!(w.standing(bo).gatherings, 2);

        // Taking the yes back takes the gathering back.
        w.state.db.upsert_rsvp(&by_bot, bo, "no", None, "", now()).unwrap();
        assert_eq!(w.standing(bo).gatherings, 1);
    }

    #[test]
    fn standing_counts_whole_days_in_the_city() {
        let w = world();
        let (dan_p, _) = w.member("Dan", 8);
        let (eve_p, _) = w.member("Eve", 0);
        let dan = w.standing(&dan_p.id);
        assert_eq!(dan.member_days, 8);
        assert!(dan.member_since.unwrap() <= now() - 8 * DAY_SECS);
        assert_eq!(w.standing(&eve_p.id).member_days, 0);
        // Someone with no place in the city has no start and no days.
        let (zed_p, _) = person(&w.state, "Zed");
        let zed = w.standing(&zed_p.id);
        assert_eq!((zed.member_since, zed.member_days), (None, 0));
    }

    #[test]
    fn standing_counts_only_trips_that_arrived_and_finished_and_were_not_sent_home() {
        let w = world();
        let bo = &w.bo_p.id;

        // Under way: it hasn't gone well or badly yet.
        let first = w.scout_visits(60);
        assert_eq!((w.standing(bo).good_trips, w.standing(bo).sent_home), (0, 0));
        // The bot going home is a good trip.
        end_trip(&w.state, &w.scout, &first.id).unwrap();
        assert_eq!((w.standing(bo).good_trips, w.standing(bo).sent_home), (1, 0));

        // A resident sending it home is not, and is counted as sent home.
        let second = w.scout_visits(60);
        end_trip(&w.state, &w.ann, &second.id).unwrap();
        assert_eq!((w.standing(bo).good_trips, w.standing(bo).sent_home), (1, 1));
        assert!(w.state.db.was_sent_home(&second.id).unwrap());
        assert!(!w.state.db.was_sent_home(&first.id).unwrap());

        // Asking and not going doesn't count, however it ended.
        let asked = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, 30)).unwrap();
        end_trip(&w.state, &w.scout, &asked.id).unwrap();
        let declined = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, 30)).unwrap();
        answer_trip(&w.state, &w.ann, &declined.id, "no").unwrap();
        assert_eq!((w.standing(bo).good_trips, w.standing(bo).sent_home), (1, 1));
        // None of it is Ann's: she has no bots.
        assert_eq!((w.standing(&w.ann_p.id).good_trips, w.standing(&w.ann_p.id).sent_home), (0, 0));
    }

    #[test]
    fn each_way_of_finishing_without_being_sent_home_is_a_good_trip() {
        for how in ["the owner calling it back", "time running out"] {
            let w = world();
            let trip = w.scout_visits(60);
            match how {
                "time running out" => {
                    w.state.db.set_trip_ends_at(&trip.id, now() - 5, now()).unwrap();
                }
                _ => {
                    end_trip(&w.state, &w.bo, &trip.id).unwrap();
                }
            }
            let standing = w.standing(&w.bo_p.id);
            assert_eq!((standing.good_trips, standing.sent_home), (1, 0), "{how}");
        }
    }

    #[test]
    fn looping_short_trips_to_one_house_counts_once_a_day() {
        let w = world();
        for _ in 0..4 {
            let trip = w.scout_visits(1);
            end_trip(&w.state, &w.scout, &trip.id).unwrap();
        }
        assert_eq!(w.standing(&w.bo_p.id).good_trips, 1);
    }

    #[test]
    fn being_turned_away_clears_your_vouches_so_the_link_starts_you_from_nothing() {
        let w = world();
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(2), ..CityRulesInput::default() })
            .unwrap();
        let (eve_p, _) = person(&w.state, "Eve");
        join_city(&w.state, &eve_p, &w.city.invite_code, false).unwrap();
        vouch(&w.state, &w.bo, &w.city.id, &eve_p.id).unwrap();
        assert_eq!(w.standing(&eve_p.id).vouches.len(), 1);

        decline(&w.state, &w.ann, &w.city.id, &eve_p.id).unwrap();
        assert!(w.standing(&eve_p.id).vouches.is_empty());

        // Coming back puts them at the gate again, with nothing behind them.
        let again = join_city(&w.state, &eve_p, &w.city.invite_code, false).unwrap();
        assert!(again.waiting);
        assert_eq!(again.vouches, 0);
    }

    #[test]
    fn sent_home_is_recorded_only_when_a_resident_who_is_a_person_ends_a_trip() {
        let w = world();
        let sent = |trip: &crate::db::Trip| w.state.db.was_sent_home(&trip.id).unwrap();

        // Not the bot, not its owner.
        let by_bot = w.scout_visits(60);
        end_trip(&w.state, &w.scout, &by_bot.id).unwrap();
        let by_owner = w.scout_visits(60);
        end_trip(&w.state, &w.bo, &by_owner.id).unwrap();
        // Not a pass being cancelled, even by the resident who offered it.
        let covering = w.scout_visits(60);
        end_trip(&w.state, &w.bo, &covering.id).unwrap();
        let offer = PassInput { agent_id: w.scout_id(), max_minutes: Some(60) };
        let pass = offer_pass(&w.state, &w.ann, &w.hz.id, &offer).unwrap();
        accept_pass(&w.state, &w.bo, &pass.id).unwrap();
        let by_pass = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, 30)).unwrap();
        assert_eq!(by_pass.status, "here");
        city::cancel_pass(&w.state, &w.ann, &pass.id).unwrap();
        assert_eq!(w.state.db.trip(&by_pass.id, now()).unwrap().unwrap().status, "ended");
        // Not the owner turning on `stays_home`, which ends the trip.
        let home = w.scout_visits(60);
        city::set_bot_home(&w.state, &w.bo, &w.scout_id(), &w.ha.id, true).unwrap();
        assert_eq!(w.state.db.trip(&home.id, now()).unwrap().unwrap().status, "ended");
        // Not time running out, and not a request nobody answered.
        city::set_bot_home(&w.state, &w.bo, &w.scout_id(), &w.ha.id, false).unwrap();
        let ran_out = w.scout_visits(60);
        w.state.db.set_trip_ends_at(&ran_out.id, now() - 5, now()).unwrap();
        let withdrawn = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, 30)).unwrap();
        end_trip(&w.state, &w.ann, &withdrawn.id).unwrap();
        for trip in [&by_bot, &by_owner, &covering, &by_pass, &home, &ran_out, &withdrawn] {
            assert!(!sent(trip), "{} shouldn't be recorded", trip.id);
        }
        assert_eq!(w.standing(&w.bo_p.id).sent_home, 0);

        // A resident ending a trip under way is.
        let shown_out = w.scout_visits(60);
        end_trip(&w.state, &w.ann, &shown_out.id).unwrap();
        assert!(sent(&shown_out));
        assert_eq!(w.standing(&w.bo_p.id).sent_home, 1);
        // Ending it again changes nothing, and nor does anyone else's attempt.
        assert_eq!(status(end_trip(&w.state, &w.ann, &shown_out.id)), StatusCode::CONFLICT);
        assert_eq!(w.standing(&w.bo_p.id).sent_home, 1);
    }

    #[test]
    fn a_vouch_from_someone_who_left_the_city_stops_counting() {
        let w = world();
        let (dan_p, dan) = w.member("Dan", 1);
        vouch(&w.state, &dan, &w.city.id, &w.bo_p.id).unwrap();
        vouch(&w.state, &w.ann, &w.city.id, &w.bo_p.id).unwrap();
        let mut names = w.standing(&w.bo_p.id).vouches;
        names.sort();
        assert_eq!(names, ["Ann", "Dan"]);

        leave_city(&w.state, &dan, &w.city.id).unwrap();
        assert_eq!(w.standing(&w.bo_p.id).vouches, ["Ann"]);
        // The vouch wasn't deleted, only left out of the count, so it can't be taken back by them
        // either but would count if they returned.
        assert_eq!(w.state.db.vouch_count(&w.city.id, &w.bo_p.id).unwrap(), 1);
        w.state.db.add_city_member(&w.city.id, &dan_p.id, now()).unwrap();
        assert_eq!(w.state.db.vouch_count(&w.city.id, &w.bo_p.id).unwrap(), 2);
    }

    #[test]
    fn chat_notes_status_activity_tokens_and_requests_never_add_to_standing() {
        let w = world();
        let before = w.standing(&w.bo_p.id);
        let square = w.city.square_id.clone();
        ops::bring_agent(&w.state, &w.bo, &square, &w.scout_id()).unwrap();
        for who in [&w.bo, &w.scout] {
            ops::post_message(&w.state, who, &square, "chat", "Hello everyone").unwrap();
            ops::post_message(&w.state, who, &square, "note", "A note worth keeping").unwrap();
        }
        ops::set_status(&w.state, &w.scout, Some("working")).unwrap();
        ops::report_activity(&w.state, &w.scout, "computer", "FlowRMM").unwrap();
        ops::rotate_agent_token(&w.state, &w.bo, &w.scout_id()).unwrap();
        // Asking again and again, and being turned down, adds nothing either.
        for _ in 0..3 {
            let asked = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, 30)).unwrap();
            end_trip(&w.state, &w.scout, &asked.id).unwrap();
        }
        let declined = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, 30)).unwrap();
        answer_trip(&w.state, &w.ann, &declined.id, "no").unwrap();
        assert_eq!(w.standing(&w.bo_p.id), before);
    }

    #[test]
    fn standing_is_only_counts_and_the_names_of_the_people_who_vouch() {
        let w = world();
        w.scout_visits(60);
        w.plan(&w.city.square_id, "confirmed", 3600, &[&w.ann_p.id, &w.bo_p.id]);
        vouch(&w.state, &w.ann, &w.city.id, &w.bo_p.id).unwrap();
        let shown = standing_of(&w.state, &w.ann, &w.city.id, &w.bo_p.id).unwrap();
        let json = serde_json::to_value(&shown).unwrap();
        let mut keys: Vec<&String> = json.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["gatherings", "good_trips", "member_days", "member_since", "sent_home", "vouches"]
        );
        // No plan title, trip goal, house name or bot name anywhere in it (T4).
        let text = json.to_string();
        for secret in
            ["A secret plan title", "Pair on the build", "Ann's House", "Bo's House", "Scout"]
        {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        assert_eq!(shown.vouches, ["Ann"]);
    }

    // Who may see standing

    #[test]
    fn standing_is_for_people_in_the_city_and_their_bots() {
        let w = world();
        let (_, eve) = person(&w.state, "Eve");
        let outsider_room = w
            .state
            .db
            .create_hangout("eve-room", "Eve's", "eve-code", &eve.person_id, now())
            .unwrap();
        let stranger_bot = bot(&w.state, &eve, &outsider_room.id, "Stranger");
        let bo = &w.bo_p.id;

        for who in [&w.ann, &w.bo, &w.scout] {
            assert_eq!(standing_of(&w.state, who, &w.city.id, bo).unwrap().member_days, 0);
        }
        // A stranger, and a stranger's bot, get what a city that doesn't exist would give.
        for who in [&eve, &stranger_bot] {
            assert_eq!(status(standing_of(&w.state, who, &w.city.id, bo)), StatusCode::NOT_FOUND);
        }
        assert_eq!(
            status(standing_of(&w.state, &w.ann, "no-such-city", bo)),
            StatusCode::NOT_FOUND
        );
        // Someone the city doesn't know is the same 404, however the question is put.
        assert_eq!(
            status(standing_of(&w.state, &w.ann, &w.city.id, &eve.person_id)),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(standing_of(&w.state, &w.ann, &w.city.id, "nobody")),
            StatusCode::NOT_FOUND
        );
        // The house bot has no business here.
        let (_, token) = city::add_house_bot(&w.state, &w.ann, &w.hz.id, "Casa").unwrap();
        let casa = w.state.authenticate(&auth::hash_secret(&token)).unwrap().unwrap();
        assert_eq!(status(standing_of(&w.state, &casa, &w.city.id, bo)), StatusCode::FORBIDDEN);
        assert_eq!(status(vouch(&w.state, &casa, &w.city.id, bo)), StatusCode::FORBIDDEN);
        assert_eq!(status(city_for_rules(&w.state, &casa, &w.city.id)), StatusCode::FORBIDDEN);
        assert_eq!(status(house_for_rules(&w.state, &casa, &w.hz.id)), StatusCode::FORBIDDEN);
        assert_eq!(status(decline(&w.state, &casa, &w.city.id, bo)), StatusCode::FORBIDDEN);
    }

    // Houses

    #[test]
    fn the_message_names_the_house_and_the_askers_own_numbers() {
        let have = Standing {
            member_since: Some(0),
            member_days: 3,
            gatherings: 0,
            good_trips: 0,
            sent_home: 4,
            vouches: Vec::new(),
        };
        let need =
            Requirements { min_member_days: 7, min_gatherings: 2, ..Requirements::default() };
        assert_eq!(
            shortfall("Kevin's Loft", "Dan", &need, &have).unwrap(),
            "Kevin's Loft asks for 2 gatherings (Dan has 0) and 7 days in the city (Dan has 3)"
        );
        // One of each says "1 day" and "1 vouch", and three or more read as a list.
        let all = Requirements {
            min_member_days: 1,
            min_gatherings: 1,
            min_good_trips: 1,
            min_vouches: 1,
        };
        let nothing = Standing { member_days: 0, ..have.clone() };
        assert_eq!(
            shortfall("H", "Dan", &all, &nothing).unwrap(),
            "H asks for 1 gathering (Dan has 0), 1 day in the city (Dan has 0), 1 good trip (Dan has 0) and 1 vouch (Dan has 0)"
        );
        assert_eq!(
            shortfall("H", "Dan", &all, &have).unwrap(),
            "H asks for 1 gathering (Dan has 0), 1 good trip (Dan has 0) and 1 vouch (Dan has 0)"
        );
        // Exactly what's asked for is enough, and nothing asked for is nothing missing.
        let met = Standing { member_days: 7, gatherings: 2, ..have.clone() };
        assert_eq!(shortfall("H", "Dan", &need, &met), None);
        assert_eq!(shortfall("H", "Dan", &Requirements::default(), &have), None);
        // `sent_home` is shown as a fact and is never a requirement.
        let sent = Standing { sent_home: 50, ..met };
        assert_eq!(shortfall("H", "Dan", &need, &sent), None);
    }

    #[test]
    fn a_house_that_asks_for_standing_says_what_is_missing_and_a_yes_is_still_needed() {
        let w = world();
        let (dan_p, dan) = w.member("Dan", 8);
        let pip = bot(&w.state, &dan, &w.city.square_id, "Pip");
        let rules = asks([("days", 7), ("gatherings", 2), ("trips", 1), ("vouches", 1)]);
        w.house_asks(&w.ann, &w.hz, rules).unwrap();

        // Dan has been here long enough, and nothing else. A bot is told, and so is a person
        // asking for the same bot, in the same words.
        let refused = create_trip(&w.state, &pip, &w.trip_to(&w.hz, 30)).unwrap_err();
        let expected = "Ann's House asks for 2 gatherings (Dan has 0), 1 good trip (Dan has 0) and 1 vouch (Dan has 0)";
        assert_eq!(refused.message, expected);
        assert_eq!(refused.status, StatusCode::CONFLICT);
        let mut named = w.trip_to(&w.hz, 30);
        named.agent_id = pip.agent_id.clone();
        assert_eq!(message(create_trip(&w.state, &dan, &named)), expected);
        assert!(w.state.db.trips_for_owner(&dan_p.id, 0, now()).unwrap().is_empty());

        // Two gatherings, and then a vouch from Ann, close the gaps one at a time.
        for _ in 0..2 {
            w.plan(&w.city.square_id, "confirmed", 3600, &[&dan_p.id, &w.ann_p.id]);
        }
        assert_eq!(
            message(create_trip(&w.state, &pip, &w.trip_to(&w.hz, 30))),
            "Ann's House asks for 1 good trip (Dan has 0) and 1 vouch (Dan has 0)"
        );
        vouch(&w.state, &w.ann, &w.city.id, &dan_p.id).unwrap();
        assert_eq!(
            message(create_trip(&w.state, &pip, &w.trip_to(&w.hz, 30))),
            "Ann's House asks for 1 good trip (Dan has 0)"
        );

        // A good trip to a house that asks for nothing, then back home.
        let visit = create_trip(&w.state, &pip, &w.trip_to(&w.ha, 30)).unwrap();
        answer_trip(&w.state, &dan, &visit.id, "yes").unwrap();
        answer_trip(&w.state, &w.bo, &visit.id, "yes").unwrap();
        end_trip(&w.state, &pip, &visit.id).unwrap();

        // Now Dan's bot may ask, and that is all: nobody has said yes (R7).
        let ask = create_trip(&w.state, &pip, &w.trip_to(&w.hz, 30)).unwrap();
        assert_eq!(ask.status, "asking");
        assert_eq!((ask.owner_ok_by, ask.host_ok_by), (None, None));
        assert!(!w.state.in_room(&pip, &w.hz.porch_id).unwrap());
        end_trip(&w.state, &pip, &ask.id).unwrap();
        // A person who asks for the bot is the owner's yes, and still needs the house's.
        let by_owner = create_trip(&w.state, &dan, &named).unwrap();
        assert_eq!(
            (by_owner.status.as_str(), by_owner.owner_ok_by.as_deref()),
            ("asking", Some("Dan"))
        );
        assert_eq!(by_owner.host_ok_by, None);
    }

    #[test]
    fn each_requirement_on_its_own_stops_an_ask_until_it_is_met() {
        let w = world();
        let (dan_p, dan) = w.member("Dan", 8);
        let pip = bot(&w.state, &dan, &w.city.square_id, "Pip");
        let ask = || create_trip(&w.state, &pip, &w.trip_to(&w.hz, 30));
        let only = |input: [(&str, i64); 4]| w.house_asks(&w.ann, &w.hz, asks(input)).unwrap();
        // Meeting it lets the ask through, as an ask: nothing has started.
        let lets_through = |trip: ApiResult<crate::db::Trip>| {
            let trip = trip.unwrap();
            assert_eq!(trip.status, "asking");
            end_trip(&w.state, &pip, &trip.id).unwrap();
        };

        // Days: eight is enough for eight and not for nine.
        only([("days", 9), ("gatherings", 0), ("trips", 0), ("vouches", 0)]);
        assert_eq!(message(ask()), "Ann's House asks for 9 days in the city (Dan has 8)");
        only([("days", 8), ("gatherings", 0), ("trips", 0), ("vouches", 0)]);
        lets_through(ask());

        only([("days", 0), ("gatherings", 1), ("trips", 0), ("vouches", 0)]);
        assert_eq!(message(ask()), "Ann's House asks for 1 gathering (Dan has 0)");
        w.plan(&w.city.square_id, "confirmed", 3600, &[&dan_p.id, &w.ann_p.id]);
        lets_through(ask());

        only([("days", 0), ("gatherings", 0), ("trips", 0), ("vouches", 1)]);
        assert_eq!(message(ask()), "Ann's House asks for 1 vouch (Dan has 0)");
        vouch(&w.state, &w.ann, &w.city.id, &dan_p.id).unwrap();
        lets_through(ask());

        only([("days", 0), ("gatherings", 0), ("trips", 1), ("vouches", 0)]);
        assert_eq!(message(ask()), "Ann's House asks for 1 good trip (Dan has 0)");
        let visit = create_trip(&w.state, &pip, &w.trip_to(&w.ha, 30)).unwrap();
        answer_trip(&w.state, &dan, &visit.id, "yes").unwrap();
        answer_trip(&w.state, &w.bo, &visit.id, "yes").unwrap();
        // Still under way, it hasn't gone well yet.
        assert_eq!(w.standing(&dan_p.id).good_trips, 0);
        end_trip(&w.state, &pip, &visit.id).unwrap();
        lets_through(ask());

        // Asking for nothing again lets anyone ask. Being sent home is a fact for the house, not
        // a requirement anyone can be turned away for.
        only([("days", 0), ("gatherings", 0), ("trips", 0), ("vouches", 0)]);
        let sent = create_trip(&w.state, &pip, &w.trip_to(&w.hz, 30)).unwrap();
        answer_trip(&w.state, &dan, &sent.id, "yes").unwrap();
        answer_trip(&w.state, &w.ann, &sent.id, "yes").unwrap();
        end_trip(&w.state, &w.ann, &sent.id).unwrap();
        assert_eq!(w.standing(&dan_p.id).sent_home, 1);
        lets_through(ask());
    }

    #[test]
    fn a_resident_inviting_a_bot_skips_what_the_house_asks_for() {
        let w = world();
        w.house_asks(
            &w.ann,
            &w.hz,
            asks([("days", 365), ("gatherings", 100), ("trips", 100), ("vouches", 5)]),
        )
        .unwrap();
        // Bo can't ask, and neither can his bot.
        assert_eq!(
            status(create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, 30))),
            StatusCode::CONFLICT
        );
        // Ann choosing the bot is the house's yes already given.
        let mut invite = w.trip_to(&w.hz, 30);
        invite.agent_id = Some(w.scout_id());
        let trip = create_trip(&w.state, &w.ann, &invite).unwrap();
        assert_eq!((trip.status.as_str(), trip.host_ok_by.as_deref()), ("asking", Some("Ann")));
        // It still takes the bot's owner to say yes.
        assert!(!w.state.in_room(&w.scout, &w.hz.porch_id).unwrap());
        let here = answer_trip(&w.state, &w.bo, &trip.id, "yes").unwrap();
        assert_eq!(here.status, "here");
    }

    #[test]
    fn an_accepted_pass_skips_requirements_and_changing_them_ends_nothing() {
        let w = world();
        let first = w.scout_visits(30);
        end_trip(&w.state, &w.scout, &first.id).unwrap();
        let offer = PassInput { agent_id: w.scout_id(), max_minutes: Some(60) };
        let pass = offer_pass(&w.state, &w.ann, &w.hz.id, &offer).unwrap();

        // A pass nobody accepted covers nothing, so the requirements apply.
        w.house_asks(
            &w.ann,
            &w.hz,
            asks([("days", 30), ("gatherings", 0), ("trips", 0), ("vouches", 0)]),
        )
        .unwrap();
        assert_eq!(
            status(create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, 30))),
            StatusCode::CONFLICT
        );
        accept_pass(&w.state, &w.bo, &pass.id).unwrap();
        let covered = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, 30)).unwrap();
        assert_eq!(covered.status, "here");
        // Changing the requirements doesn't end the trip or the pass.
        w.house_asks(
            &w.ann,
            &w.hz,
            asks([("days", 365), ("gatherings", 100), ("trips", 0), ("vouches", 0)]),
        )
        .unwrap();
        assert_eq!(w.state.db.trip(&covered.id, now()).unwrap().unwrap().status, "here");
        assert!(w.state.db.pass(&pass.id).unwrap().is_some());
        end_trip(&w.state, &w.scout, &covered.id).unwrap();
        // A trip longer than the pass goes through the normal ask, and the requirements.
        assert_eq!(
            status(create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, 120))),
            StatusCode::CONFLICT
        );
        let again = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, 30)).unwrap();
        assert_eq!(again.status, "here");
    }

    #[test]
    fn a_house_sets_its_requirements_within_the_limits_and_only_a_resident_who_is_a_person_can() {
        let w = world();
        let rules = |days, gatherings, trips, vouches| {
            asks([
                ("days", days),
                ("gatherings", gatherings),
                ("trips", trips),
                ("vouches", vouches),
            ])
        };
        for bad in [
            rules(366, 0, 0, 0),
            rules(0, 101, 0, 0),
            rules(0, 0, 101, 0),
            rules(0, 0, 0, 6),
            rules(-1, 0, 0, 0),
            rules(0, 0, 0, -1),
        ] {
            assert_eq!(status(w.house_asks(&w.ann, &w.hz, bad)), StatusCode::BAD_REQUEST);
        }
        assert_eq!(
            w.state.db.entry_rules(&w.hz.id).unwrap(),
            EntryRules::default(),
            "a refused request changes nothing"
        );
        w.house_asks(&w.ann, &w.hz, rules(365, 100, 100, 5)).unwrap();
        let shown = city::house_view(&w.state, &w.ann, &w.hz.id).unwrap();
        assert_eq!(
            shown.rules,
            Requirements {
                min_member_days: 365,
                min_gatherings: 100,
                min_good_trips: 100,
                min_vouches: 5
            }
        );
        // A number left out stays as it was, and 0 asks for nothing again.
        w.house_asks(
            &w.ann,
            &w.hz,
            HouseRulesInput { min_gatherings: Some(0), ..HouseRulesInput::default() },
        )
        .unwrap();
        assert_eq!(house_requirements(&w.state, &w.hz.id).unwrap().min_member_days, 365);
        assert_eq!(house_requirements(&w.state, &w.hz.id).unwrap().min_gatherings, 0);

        // A resident's bot, someone from another house, and a stranger, in that order of knowing.
        let helper = bot(&w.state, &w.ann, &w.hz.living_room_id, "Helper");
        let (_, eve) = person(&w.state, "Eve");
        assert_eq!(status(w.house_asks(&helper, &w.hz, rules(1, 0, 0, 0))), StatusCode::FORBIDDEN);
        assert_eq!(status(w.house_asks(&w.bo, &w.hz, rules(1, 0, 0, 0))), StatusCode::NOT_FOUND);
        assert_eq!(status(w.house_asks(&eve, &w.hz, rules(1, 0, 0, 0))), StatusCode::NOT_FOUND);
        assert_eq!(
            status(house_for_rules(&w.state, &w.ann, "no-such-house")),
            StatusCode::NOT_FOUND
        );
        assert_eq!(house_requirements(&w.state, &w.hz.id).unwrap().min_member_days, 365);
        // Every house's requirements show on the city page, to the people in it and their bots.
        for who in [&w.bo, &w.scout] {
            let view = city::city_view(&w.state, who, &w.city.id).unwrap();
            let ann_house = view.houses.iter().find(|house| house.id == w.hz.id).unwrap();
            assert_eq!(ann_house.requirements.min_member_days, 365);
            let bo_house = view.houses.iter().find(|house| house.id == w.ha.id).unwrap();
            assert_eq!(bo_house.requirements, Requirements::default());
        }
    }

    #[test]
    fn a_resident_sees_how_each_asker_stands_beside_their_request() {
        let w = world();
        w.plan(&w.city.square_id, "confirmed", 3600, &[&w.ann_p.id, &w.bo_p.id]);
        vouch(&w.state, &w.ann, &w.city.id, &w.bo_p.id).unwrap();
        let before = city::house_view(&w.state, &w.ann, &w.hz.id).unwrap();
        assert!(before.standing.is_empty());

        let trip = create_trip(&w.state, &w.scout, &w.trip_to(&w.hz, 30)).unwrap();
        let view = city::house_view(&w.state, &w.ann, &w.hz.id).unwrap();
        assert_eq!(view.asking.len(), 1);
        let shown = &view.standing[&w.bo_p.id];
        assert_eq!((shown.gatherings, shown.vouches.clone()), (1, vec!["Ann".to_string()]));
        assert_eq!(view.standing.len(), 1);
        // Only requests waiting on an answer: once it starts, the standing goes.
        answer_trip(&w.state, &w.bo, &trip.id, "yes").unwrap();
        answer_trip(&w.state, &w.ann, &trip.id, "yes").unwrap();
        assert!(city::house_view(&w.state, &w.ann, &w.hz.id).unwrap().standing.is_empty());
    }

    // Cities and the gate

    #[test]
    fn a_city_sets_only_vouches_and_rules_text_and_only_its_keeper_can() {
        let w = world();
        let (_, dan) = w.member("Dan", 1);
        // Ann started the city. Bo, Eve and bots can't set its rules.
        let (_, eve) = person(&w.state, "Eve");
        let input = || CityRulesInput { min_vouches: Some(1), ..CityRulesInput::default() };
        assert_eq!(status(w.city_asks(&w.bo, input())), StatusCode::FORBIDDEN);
        assert_eq!(status(w.city_asks(&w.scout, input())), StatusCode::FORBIDDEN);
        assert_eq!(status(w.city_asks(&eve, input())), StatusCode::NOT_FOUND);
        assert_eq!(city_rules(&w.state, &w.city.id).unwrap(), CityRules::default());

        // T13: no more than 5, and no more than the people there are.
        let people = w.state.db.city_members(&w.city.id).unwrap().len() as i64;
        assert_eq!(people, 3);
        for bad in [4, 6, -1] {
            let asked = CityRulesInput { min_vouches: Some(bad), ..CityRulesInput::default() };
            assert_eq!(status(w.city_asks(&w.ann, asked)), StatusCode::BAD_REQUEST, "{bad}");
        }
        // T12: the other requirements stay 0.
        for other in [
            CityRulesInput { min_member_days: Some(1), ..CityRulesInput::default() },
            CityRulesInput { min_gatherings: Some(1), ..CityRulesInput::default() },
            CityRulesInput { min_good_trips: Some(1), ..CityRulesInput::default() },
        ] {
            assert_eq!(status(w.city_asks(&w.ann, other)), StatusCode::BAD_REQUEST);
        }
        let zeros = CityRulesInput {
            min_member_days: Some(0),
            min_gatherings: Some(0),
            ..CityRulesInput::default()
        };
        w.city_asks(&w.ann, zeros).unwrap();
        // The text is trimmed, at most 1,000 characters, and blank clears it.
        let text = |text: &str| CityRulesInput {
            rules_text: Some(text.to_string()),
            ..CityRulesInput::default()
        };
        assert_eq!(status(w.city_asks(&w.ann, text(&"x".repeat(1001)))), StatusCode::BAD_REQUEST);
        w.city_asks(&w.ann, text(&"x".repeat(1000))).unwrap();
        w.city_asks(&w.ann, text("  Be kind to the bots.  ")).unwrap();
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(3), ..CityRulesInput::default() })
            .unwrap();
        let rules = city_rules(&w.state, &w.city.id).unwrap();
        assert_eq!((rules.min_vouches, rules.rules_text.as_str()), (3, "Be kind to the bots."));
        // Leaving a number or the text out leaves it alone.
        assert_eq!(w.state.db.entry_rules(&w.city.id).unwrap().min_member_days, 0);
        w.city_asks(&w.ann, text("")).unwrap();
        assert_eq!(
            city_rules(&w.state, &w.city.id).unwrap(),
            CityRules { min_vouches: 3, rules_text: String::new() }
        );
        // Everyone in the city and their bots see the rules, and the preview shows them too.
        w.city_asks(&w.ann, text("No phones at dinner")).unwrap();
        for who in [&w.ann, &w.bo, &w.scout] {
            let view = city::city_view(&w.state, who, &w.city.id).unwrap();
            assert_eq!(
                (view.rules.min_vouches, view.rules.rules_text.as_str()),
                (3, "No phones at dinner")
            );
        }
        let preview = city::city_invite_preview(&w.state, &w.city.invite_code).unwrap();
        assert_eq!(
            (preview.vouches_needed, preview.rules_text.as_str()),
            (3, "No phones at dinner")
        );
        // Lowering is always allowed, even when the city has fewer people than it asks for.
        leave_city(&w.state, &dan, &w.city.id).unwrap();
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(2), ..CityRulesInput::default() })
            .unwrap();
        assert_eq!(
            status(w.city_asks(
                &w.ann,
                CityRulesInput { min_vouches: Some(3), ..CityRulesInput::default() }
            )),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn when_the_keeper_has_left_any_member_looks_after_the_rules() {
        let state = new_state();
        let (zed_p, zed) = person(&state, "Zed");
        let (yan_p, yan) = person(&state, "Yan");
        let oak = create_city(&state, &zed_p, "Oak").unwrap();
        join_city(&state, &yan_p, &oak.invite_code, false).unwrap();
        let input = || CityRulesInput { min_vouches: Some(1), ..CityRulesInput::default() };
        let keeper = |who: &Caller| city_for_rules(&state, who, &oak.id).map(|_| ());
        assert!(keeper(&zed).is_ok());
        assert_eq!(status(keeper(&yan)), StatusCode::FORBIDDEN);
        leave_city(&state, &zed, &oak.id).unwrap();
        let found = city_for_rules(&state, &yan, &oak.id).unwrap();
        set_city_rules(&state, &found, &input()).unwrap();
        assert_eq!(city_rules(&state, &oak.id).unwrap().min_vouches, 1);
        // And Zed, who has left, is a stranger to it now.
        assert_eq!(status(keeper(&zed)), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_city_with_rules_is_joined_only_by_someone_who_accepts_them() {
        let w = world();
        w.city_asks(
            &w.ann,
            CityRulesInput { rules_text: Some("Be kind".to_string()), ..CityRulesInput::default() },
        )
        .unwrap();
        let city = w.state.db.city(&w.city.id).unwrap().unwrap();
        let (cy_p, _) = person(&w.state, "Cy");
        // 409 before anyone is made, and for someone already made.
        assert_eq!(status(check_new_person_may_join(&w.state, &city, false)), StatusCode::CONFLICT);
        assert!(check_new_person_may_join(&w.state, &city, true).is_ok());
        assert_eq!(status(enter_city(&w.state, &cy_p, &city, false)), StatusCode::CONFLICT);
        assert!(!w.state.db.is_city_member(&city.id, &cy_p.id).unwrap());
        assert!(w.state.db.gate_asked_at(&city.id, &cy_p.id, 0).unwrap().is_none());
        let joined = enter_city(&w.state, &cy_p, &city, true).unwrap();
        assert!(!joined.waiting && joined.vouches_needed == 0);
        assert!(w.state.db.is_city_member(&city.id, &cy_p.id).unwrap());
        // Using the link again, by someone already in, needs nothing and does no harm.
        assert!(!enter_city(&w.state, &cy_p, &city, false).unwrap().waiting);
        assert_eq!(w.state.db.city_members(&city.id).unwrap().len(), 3);
    }

    #[test]
    fn a_house_link_needs_the_rules_accepted_by_anyone_not_in_the_city_yet() {
        let w = world();
        w.city_asks(
            &w.ann,
            CityRulesInput { rules_text: Some("Be kind".to_string()), ..CityRulesInput::default() },
        )
        .unwrap();
        let link = || w.state.db.house(&w.hz.id).unwrap().unwrap().invite_code;
        assert_eq!(city::house_invite_preview(&w.state, &link()).unwrap().rules_text, "Be kind");

        let (cy_p, _) = person(&w.state, "Cy");
        assert_eq!(status(join_house(&w.state, &cy_p, &link(), false)), StatusCode::CONFLICT);
        assert!(!w.state.db.is_city_member(&w.city.id, &cy_p.id).unwrap());
        assert!(!w.state.db.is_resident(&w.hz.id, &cy_p.id).unwrap());
        join_house(&w.state, &cy_p, &link(), true).unwrap();
        assert!(w.state.db.is_resident(&w.hz.id, &cy_p.id).unwrap());

        // Someone already in the city has accepted once, and isn't asked again.
        let (di_p, _) = w.member("Di", 1);
        join_house(&w.state, &di_p, &link(), false).unwrap();
        assert!(w.state.db.is_resident(&w.hz.id, &di_p.id).unwrap());
    }

    #[test]
    fn with_no_vouches_asked_for_joining_works_as_it_always_did() {
        let w = world();
        let (cy_p, cy, joined) = w.arrives("Cy");
        assert!(!joined.waiting);
        assert_eq!((joined.vouches, joined.vouches_needed), (0, 0));
        assert!(w.state.in_room(&cy, &w.city.square_id).unwrap());
        assert!(w.gate(&w.ann).is_empty());
        let square = w.state.db.messages(&w.city.square_id, 0, 100).unwrap();
        assert!(square.iter().any(|line| line.body == "Cy joined Elm."));
        assert!(waiting(&w.state, &cy_p.id).unwrap().is_empty());
    }

    #[test]
    fn the_gate_lets_someone_in_when_enough_people_vouch() {
        let w = world();
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(2), ..CityRulesInput::default() })
            .unwrap();
        let inbox_before = serde_json::to_value(ops::inbox(&w.state, &w.ann).unwrap()).unwrap();

        // Cy has an account, but the city is a 404 for them (R1).
        let (cy_p, cy, joined) = w.arrives("Cy");
        assert!(joined.waiting);
        assert_eq!((joined.vouches, joined.vouches_needed), (0, 2));
        assert!(!w.state.db.is_city_member(&w.city.id, &cy_p.id).unwrap());
        assert_eq!(status(city::city_view(&w.state, &cy, &w.city.id)), StatusCode::NOT_FOUND);
        assert_eq!(status(city::require_city(&w.state, &cy, &w.city.id)), StatusCode::NOT_FOUND);
        assert!(!w.state.in_room(&cy, &w.city.square_id).unwrap());
        assert!(!city::in_city(&w.state, &cy, &w.city.id).unwrap());
        // Using the link again is harmless, and doesn't start the 14 days over.
        let asked_at = w.state.db.gate_asked_at(&w.city.id, &cy_p.id, 0).unwrap();
        assert!(enter_city(&w.state, &cy_p, &w.city, true).unwrap().waiting);
        assert_eq!(w.state.db.gate_asked_at(&w.city.id, &cy_p.id, 0).unwrap(), asked_at);
        assert_eq!(w.gate(&w.ann).len(), 1);

        // Cy sees their own count and the city's name, and nothing else.
        let mine = waiting(&w.state, &cy_p.id).unwrap();
        assert_eq!(
            serde_json::to_value(&mine).unwrap(),
            json!([{ "city_id": w.city.id, "name": "Elm", "vouches": 0, "vouches_needed": 2 }])
        );
        // The people inside see who is waiting; their bots do not (T16).
        for who in [&w.ann, &w.bo] {
            let gate = w.gate(who);
            assert_eq!(gate.len(), 1);
            assert_eq!(
                (gate[0].person_id.as_str(), gate[0].name.as_str()),
                (cy_p.id.as_str(), "Cy")
            );
            assert_eq!((gate[0].vouches, gate[0].you_vouched), (0, false));
            assert_eq!(gate[0].color, cy_p.color);
        }
        let seen_by_bot = city::city_view(&w.state, &w.scout, &w.city.id).unwrap();
        assert!(seen_by_bot.gate.is_none());
        assert!(!serde_json::to_string(&seen_by_bot).unwrap().contains("\"gate\""));
        // Someone at the gate shows up for the people inside, with no days, and for their bots.
        for who in [&w.ann, &w.scout] {
            let shown = standing_of(&w.state, who, &w.city.id, &cy_p.id).unwrap();
            assert_eq!((shown.member_since, shown.member_days), (None, 0));
        }
        // And nobody rings the doorbell: the keeper's inbox is exactly what it was.
        assert_eq!(
            serde_json::to_value(ops::inbox(&w.state, &w.ann).unwrap()).unwrap(),
            inbox_before
        );

        // The first vouch isn't enough. A second from the same person changes nothing.
        let first = vouch(&w.state, &w.ann, &w.city.id, &cy_p.id).unwrap();
        assert_eq!((first.vouches, first.let_in), (1, false));
        let again = vouch(&w.state, &w.ann, &w.city.id, &cy_p.id).unwrap();
        assert_eq!((again.vouches, again.let_in), (1, false));
        assert_eq!(w.gate(&w.ann)[0].vouches, 1);
        assert!(w.gate(&w.ann)[0].you_vouched);
        assert!(!w.gate(&w.bo)[0].you_vouched);
        assert_eq!(waiting(&w.state, &cy_p.id).unwrap()[0].vouches, 1);
        // Taking it back never removes anyone, and putting it back is fine.
        withdraw_vouch(&w.state, &w.ann, &w.city.id, &cy_p.id).unwrap();
        assert_eq!(w.gate(&w.ann)[0].vouches, 0);
        withdraw_vouch(&w.state, &w.ann, &w.city.id, &cy_p.id).unwrap();
        vouch(&w.state, &w.ann, &w.city.id, &cy_p.id).unwrap();

        // The second one lets them in at once.
        let second = vouch(&w.state, &w.bo, &w.city.id, &cy_p.id).unwrap();
        assert_eq!((second.vouches, second.let_in), (2, true));
        assert!(w.state.db.is_city_member(&w.city.id, &cy_p.id).unwrap());
        assert!(w.state.in_room(&cy, &w.city.square_id).unwrap());
        assert!(city::city_view(&w.state, &cy, &w.city.id).is_ok());
        assert!(w.gate(&w.ann).is_empty());
        assert!(waiting(&w.state, &cy_p.id).unwrap().is_empty());
        let square = w.state.db.messages(&w.city.square_id, 0, 100).unwrap();
        assert_eq!(square.iter().filter(|line| line.body == "Cy joined Elm.").count(), 1);
        let shown = w.standing(&cy_p.id);
        assert_eq!(shown.member_days, 0);
        assert!(shown.member_since.is_some());
        let mut names = shown.vouches;
        names.sort();
        assert_eq!(names, ["Ann", "Bo"]);

        // Withdrawing now doesn't put them out.
        withdraw_vouch(&w.state, &w.ann, &w.city.id, &cy_p.id).unwrap();
        assert!(w.state.db.is_city_member(&w.city.id, &cy_p.id).unwrap());
        assert_eq!(w.standing(&cy_p.id).vouches, ["Bo"]);
        // A vouch for someone who's already in is recorded and lets nobody in again.
        let later = vouch(&w.state, &w.ann, &w.city.id, &cy_p.id).unwrap();
        assert_eq!((later.vouches, later.let_in), (2, false));
        assert_eq!(square.iter().filter(|line| line.body == "Cy joined Elm.").count(), 1);
    }

    #[test]
    fn only_people_in_the_city_vouch_and_only_for_someone_the_city_knows() {
        let w = world();
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(2), ..CityRulesInput::default() })
            .unwrap();
        let (cy_p, cy, _) = w.arrives("Cy");
        let (eve_p, eve) = person(&w.state, "Eve");
        let id = &cy_p.id;

        // A bot of a member is told only people can, and nothing it does counts.
        assert_eq!(status(vouch(&w.state, &w.scout, &w.city.id, id)), StatusCode::FORBIDDEN);
        assert_eq!(
            status(withdraw_vouch(&w.state, &w.scout, &w.city.id, id)),
            StatusCode::FORBIDDEN
        );
        // A stranger, and someone waiting at the gate, are not in the city (R1).
        assert_eq!(status(vouch(&w.state, &eve, &w.city.id, id)), StatusCode::NOT_FOUND);
        assert_eq!(status(vouch(&w.state, &cy, &w.city.id, &w.bo_p.id)), StatusCode::NOT_FOUND);
        assert_eq!(status(withdraw_vouch(&w.state, &eve, &w.city.id, id)), StatusCode::NOT_FOUND);
        // Neither in the city nor at its gate is a 404, for a real person and for nobody.
        assert_eq!(status(vouch(&w.state, &w.ann, &w.city.id, &eve_p.id)), StatusCode::NOT_FOUND);
        assert_eq!(status(vouch(&w.state, &w.ann, &w.city.id, "nobody")), StatusCode::NOT_FOUND);
        assert_eq!(status(vouch(&w.state, &w.ann, "no-such-city", id)), StatusCode::NOT_FOUND);
        // Never for yourself.
        assert_eq!(
            status(vouch(&w.state, &w.ann, &w.city.id, &w.ann_p.id)),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(w.standing(&w.ann_p.id).vouches, Vec::<String>::new());
        assert_eq!(w.state.db.vouch_count(&w.city.id, id).unwrap(), 0);
        // A person in the city vouches for another, and it shows.
        assert_eq!(vouch(&w.state, &w.ann, &w.city.id, &w.bo_p.id).unwrap().vouches, 1);
        assert_eq!(w.standing(&w.bo_p.id).vouches, ["Ann"]);
        // Taking back a vouch that was never given is harmless, and it can't touch anyone else's.
        withdraw_vouch(&w.state, &w.bo, &w.city.id, &w.bo_p.id).unwrap();
        withdraw_vouch(&w.state, &w.bo, &w.city.id, "nobody").unwrap();
        assert_eq!(w.standing(&w.bo_p.id).vouches, ["Ann"]);
    }

    #[test]
    fn only_the_keeper_turns_someone_away_from_the_gate() {
        let w = world();
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(2), ..CityRulesInput::default() })
            .unwrap();
        let (dee_p, dee, _) = w.arrives("Dee");
        let (eve_p, eve) = person(&w.state, "Eve");
        let id = &dee_p.id;

        assert_eq!(status(decline(&w.state, &w.bo, &w.city.id, id)), StatusCode::FORBIDDEN);
        assert_eq!(status(decline(&w.state, &w.scout, &w.city.id, id)), StatusCode::FORBIDDEN);
        assert_eq!(status(decline(&w.state, &eve, &w.city.id, id)), StatusCode::NOT_FOUND);
        assert_eq!(status(decline(&w.state, &dee, &w.city.id, id)), StatusCode::NOT_FOUND);
        // Someone who isn't waiting, in the city or not, is a 404.
        assert_eq!(status(decline(&w.state, &w.ann, &w.city.id, &eve_p.id)), StatusCode::NOT_FOUND);
        assert_eq!(
            status(decline(&w.state, &w.ann, &w.city.id, &w.bo_p.id)),
            StatusCode::NOT_FOUND
        );
        assert_eq!(w.gate(&w.ann).len(), 1);

        decline(&w.state, &w.ann, &w.city.id, id).unwrap();
        assert!(w.gate(&w.ann).is_empty());
        assert!(waiting(&w.state, id).unwrap().is_empty());
        assert_eq!(status(decline(&w.state, &w.ann, &w.city.id, id)), StatusCode::NOT_FOUND);
        assert_eq!(status(vouch(&w.state, &w.bo, &w.city.id, id)), StatusCode::NOT_FOUND);
        // Turned away isn't banned: the link is still the link.
        assert!(enter_city(&w.state, &dee_p, &w.city, true).unwrap().waiting);
    }

    #[test]
    fn the_keeper_turns_the_whole_gate_away_at_once() {
        let w = world();
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(2), ..CityRulesInput::default() })
            .unwrap();
        let (dee_p, dee, _) = w.arrives("Dee");
        let (eve_p, _, _) = w.arrives("Eve");
        vouch(&w.state, &w.bo, &w.city.id, &dee_p.id).unwrap();

        assert_eq!(status(clear_gate(&w.state, &w.bo, &w.city.id)), StatusCode::FORBIDDEN);
        assert_eq!(status(clear_gate(&w.state, &w.scout, &w.city.id)), StatusCode::FORBIDDEN);
        assert_eq!(status(clear_gate(&w.state, &dee, &w.city.id)), StatusCode::NOT_FOUND);
        assert_eq!(w.gate(&w.ann).len(), 2);

        assert_eq!(clear_gate(&w.state, &w.ann, &w.city.id).unwrap(), 2);
        assert!(w.gate(&w.ann).is_empty());
        assert_eq!(w.state.db.vouch_count(&w.city.id, &dee_p.id).unwrap(), 0);
        assert!(waiting(&w.state, &eve_p.id).unwrap().is_empty());
        assert_eq!(clear_gate(&w.state, &w.ann, &w.city.id).unwrap(), 0);
    }

    #[test]
    fn a_place_at_the_gate_lapses_after_fourteen_days_with_nothing_to_sweep_it() {
        let w = world();
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(1), ..CityRulesInput::default() })
            .unwrap();
        let (fay_p, _) = person(&w.state, "Fay");
        let (gil_p, _) = person(&w.state, "Gil");
        let long_ago = now() - 15 * DAY_SECS;
        let nearly = now() - 13 * DAY_SECS;
        for (person, at) in [(&fay_p, long_ago), (&gil_p, nearly)] {
            assert_eq!(
                w.state.db.gate_ask(&w.city.id, &person.id, at, at - 1, GATE_CAPACITY).unwrap(),
                GateAsk::Added
            );
        }
        // Fay's place has lapsed everywhere; Gil's hasn't.
        let names: Vec<String> = w.gate(&w.ann).into_iter().map(|entry| entry.name).collect();
        assert_eq!(names, ["Gil"]);
        assert!(waiting(&w.state, &fay_p.id).unwrap().is_empty());
        assert_eq!(waiting(&w.state, &gil_p.id).unwrap().len(), 1);
        assert_eq!(
            status(standing_of(&w.state, &w.ann, &w.city.id, &fay_p.id)),
            StatusCode::NOT_FOUND
        );
        assert!(standing_of(&w.state, &w.ann, &w.city.id, &gil_p.id).is_ok());
        assert_eq!(status(vouch(&w.state, &w.ann, &w.city.id, &fay_p.id)), StatusCode::NOT_FOUND);
        assert_eq!(status(decline(&w.state, &w.ann, &w.city.id, &fay_p.id)), StatusCode::NOT_FOUND);
        assert_eq!(w.state.db.vouch_count(&w.city.id, &fay_p.id).unwrap(), 0);
        // She can ask again, and a new place replaces the lapsed one.
        assert!(enter_city(&w.state, &fay_p, &w.city, true).unwrap().waiting);
        assert_eq!(w.gate(&w.ann).len(), 2);
        // Lowering the rules lets in Gil but never a lapsed place.
        let (hal_p, _) = person(&w.state, "Hal");
        w.state.db.gate_ask(&w.city.id, &hal_p.id, long_ago, long_ago - 1, GATE_CAPACITY).unwrap();
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(0), ..CityRulesInput::default() })
            .unwrap();
        assert!(w.state.db.is_city_member(&w.city.id, &gil_p.id).unwrap());
        assert!(w.state.db.is_city_member(&w.city.id, &fay_p.id).unwrap());
        assert!(!w.state.db.is_city_member(&w.city.id, &hal_p.id).unwrap());
    }

    #[test]
    fn the_gate_holds_fifty_places_that_have_not_lapsed() {
        let w = world();
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(1), ..CityRulesInput::default() })
            .unwrap();
        // Lapsed places don't take up room.
        for n in 0..3 {
            let (old, _) = person(&w.state, &format!("Old{n}"));
            let at = now() - 20 * DAY_SECS;
            w.state.db.gate_ask(&w.city.id, &old.id, at, at - 1, GATE_CAPACITY).unwrap();
        }
        let mut waiting_people = Vec::new();
        for n in 0..GATE_CAPACITY {
            let (person, _, joined) = w.arrives(&format!("Wait{n}"));
            assert!(joined.waiting);
            waiting_people.push(person);
        }
        assert_eq!(w.gate(&w.ann).len(), GATE_CAPACITY as usize);
        // The 51st is refused, before an account would be made and for someone who already has one.
        assert_eq!(
            status(check_new_person_may_join(&w.state, &w.city, true)),
            StatusCode::CONFLICT
        );
        let (late, _) = person(&w.state, "Late");
        assert_eq!(status(enter_city(&w.state, &late, &w.city, true)), StatusCode::CONFLICT);
        assert!(w.state.db.gate_asked_at(&w.city.id, &late.id, 0).unwrap().is_none());
        // Someone already waiting can use the link again, and a freed place can be taken.
        assert!(enter_city(&w.state, &waiting_people[0], &w.city, true).unwrap().waiting);
        decline(&w.state, &w.ann, &w.city.id, &waiting_people[1].id).unwrap();
        assert!(enter_city(&w.state, &late, &w.city, true).unwrap().waiting);
        // A city that asks for no vouches has no gate to fill.
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(0), ..CityRulesInput::default() })
            .unwrap();
        assert!(w.gate(&w.ann).is_empty());
        assert!(check_new_person_may_join(&w.state, &w.city, true).is_ok());
    }

    #[test]
    fn moving_into_a_house_through_its_link_skips_the_gate() {
        let w = world();
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(2), ..CityRulesInput::default() })
            .unwrap();
        let (cy_p, cy, joined) = w.arrives("Cy");
        assert!(joined.waiting);
        let code = w.state.db.house(&w.hz.id).unwrap().unwrap().invite_code;
        join_house(&w.state, &cy_p, &code, false).unwrap();
        assert!(w.state.db.is_city_member(&w.city.id, &cy_p.id).unwrap());
        assert!(w.state.db.is_resident(&w.hz.id, &cy_p.id).unwrap());
        assert!(w.gate(&w.ann).is_empty());
        assert!(waiting(&w.state, &cy_p.id).unwrap().is_empty());
        assert!(city::city_view(&w.state, &cy, &w.city.id).is_ok());
        // And someone who was never at the gate gets in the same way.
        let (di_p, _) = person(&w.state, "Di");
        let code = w.state.db.house(&w.hz.id).unwrap().unwrap().invite_code;
        join_house(&w.state, &di_p, &code, false).unwrap();
        assert!(w.state.db.is_city_member(&w.city.id, &di_p.id).unwrap());
    }

    #[test]
    fn lowering_the_vouches_asked_for_lets_in_everyone_who_now_has_enough() {
        let w = world();
        let (_, _dan) = w.member("Dan", 1);
        let (_, _eve) = w.member("Eve", 1);
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(4), ..CityRulesInput::default() })
            .unwrap();
        let (cy_p, _, _) = w.arrives("Cy");
        let (di_p, _, _) = w.arrives("Di");
        let (ed_p, _, _) = w.arrives("Ed");
        vouch(&w.state, &w.ann, &w.city.id, &cy_p.id).unwrap();
        vouch(&w.state, &w.bo, &w.city.id, &cy_p.id).unwrap();
        vouch(&w.state, &w.ann, &w.city.id, &di_p.id).unwrap();
        assert_eq!(w.gate(&w.ann).len(), 3);

        // Raising it, or leaving it, lets in no one.
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(5), ..CityRulesInput::default() })
            .unwrap_err();
        w.city_asks(
            &w.ann,
            CityRulesInput { rules_text: Some("Hi".to_string()), ..CityRulesInput::default() },
        )
        .unwrap();
        assert_eq!(w.gate(&w.ann).len(), 3);
        // Down to 2: Cy gets in at once and the others keep waiting.
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(2), ..CityRulesInput::default() })
            .unwrap();
        let waiting_now: Vec<String> = w.gate(&w.ann).into_iter().map(|entry| entry.name).collect();
        assert_eq!(waiting_now, ["Di", "Ed"]);
        assert!(w.state.db.is_city_member(&w.city.id, &cy_p.id).unwrap());
        // Each who got in is announced, once.
        let square = w.state.db.messages(&w.city.square_id, 0, 100).unwrap();
        assert_eq!(square.iter().filter(|line| line.body == "Cy joined Elm.").count(), 1);
        // Down to 0 lets in all who are left.
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(0), ..CityRulesInput::default() })
            .unwrap();
        assert!(w.gate(&w.ann).is_empty());
        for person in [&di_p, &ed_p] {
            assert!(w.state.db.is_city_member(&w.city.id, &person.id).unwrap());
        }
        let square = w.state.db.messages(&w.city.square_id, 0, 100).unwrap();
        for name in ["Cy", "Di", "Ed"] {
            let line = format!("{name} joined Elm.");
            assert_eq!(square.iter().filter(|said| said.body == line).count(), 1, "{line}");
        }
    }

    // Notifications

    /// Somewhere for notifications to land.
    async fn start_receiver() -> (String, Arc<Mutex<Vec<Value>>>) {
        let received = Arc::new(Mutex::new(Vec::new()));
        let store = received.clone();
        let app = axum::Router::new().route(
            "/hook",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let store = store.clone();
                async move { store.lock().unwrap().push(body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        (url, received)
    }

    /// The notices with this event, after giving any that are on their way a moment to land.
    async fn notices(received: &Mutex<Vec<Value>>, event: &str) -> Vec<Value> {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        received.lock().unwrap().iter().filter(|notice| notice["event"] == event).cloned().collect()
    }

    #[tokio::test]
    async fn the_keeper_hears_about_the_gate_and_the_person_who_gets_in_hears_about_that() {
        let w = world();
        let (dan_p, _) = w.member("Dan", 1);
        let mut inboxes = Vec::new();
        for person in [&w.ann_p, &w.bo_p, &dan_p] {
            let (url, received) = start_receiver().await;
            w.state.db.set_notify_url(&person.id, Some(&url)).unwrap();
            inboxes.push(received);
        }
        let (for_ann, for_bo, for_dan) = (&inboxes[0], &inboxes[1], &inboxes[2]);
        w.city_asks(&w.ann, CityRulesInput { min_vouches: Some(2), ..CityRulesInput::default() })
            .unwrap();

        // Someone at the gate pings the keeper, once, and not everyone in the city.
        let (cy_p, _, joined) = w.arrives("Cy");
        assert!(joined.waiting);
        let heard = notices(for_ann, "city.gate").await;
        assert_eq!(heard.len(), 1, "{heard:?}");
        assert!(heard[0]["text"].as_str().unwrap().contains("Cy"), "{heard:?}");
        assert_eq!(heard[0]["hangout_id"], w.city.square_id.as_str());
        enter_city(&w.state, &cy_p, &w.city, true).unwrap();
        assert_eq!(notices(for_ann, "city.gate").await.len(), 1, "using the link again is quiet");
        for quiet in [for_bo, for_dan] {
            assert!(quiet.lock().unwrap().is_empty(), "{:?}", quiet.lock().unwrap());
        }

        // Getting in pings the person who got in, and vouching pings nobody.
        let (url, for_cy) = start_receiver().await;
        w.state.db.set_notify_url(&cy_p.id, Some(&url)).unwrap();
        vouch(&w.state, &w.ann, &w.city.id, &cy_p.id).unwrap();
        assert!(notices(&for_cy, "city.let_in").await.is_empty());
        vouch(&w.state, &w.bo, &w.city.id, &cy_p.id).unwrap();
        let in_now = notices(&for_cy, "city.let_in").await;
        assert_eq!(in_now.len(), 1, "{in_now:?}");
        assert!(in_now[0]["text"].as_str().unwrap().contains("Elm"));
        assert!(for_cy.lock().unwrap().iter().all(|notice| notice["event"] == "city.let_in"));
        for quiet in [for_ann, for_bo, for_dan] {
            assert!(quiet.lock().unwrap().iter().all(|notice| notice["event"] != "city.let_in"));
        }
    }

    // Over a real socket, to prove the routes reach the same rules

    struct Http {
        base: String,
        client: reqwest::Client,
        state: AppState,
    }

    impl Http {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let state = AppState::new(
                Db::open(":memory:").unwrap(),
                Config {
                    public_url: base.clone(),
                    create_code: None,
                    allow_private_webhooks: true,
                    keeper_id: None,
                },
            );
            let served = state.clone();
            tokio::spawn(async move { axum::serve(listener, crate::app(served)).await });
            Self { base, client: reqwest::Client::new(), state }
        }

        async fn call(
            &self,
            token: &str,
            method: reqwest::Method,
            path: &str,
            body: Option<Value>,
        ) -> (StatusCode, Value) {
            let mut request = self.client.request(method, format!("{}{path}", self.base));
            if !token.is_empty() {
                request = request.bearer_auth(token);
            }
            if let Some(body) = body {
                request = request.json(&body);
            }
            let response = request.send().await.unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
        }

        async fn get(&self, token: &str, path: &str) -> (StatusCode, Value) {
            self.call(token, reqwest::Method::GET, path, None).await
        }

        async fn post(&self, token: &str, path: &str, body: Value) -> (StatusCode, Value) {
            self.call(token, reqwest::Method::POST, path, Some(body)).await
        }

        async fn put(&self, token: &str, path: &str, body: Value) -> (StatusCode, Value) {
            self.call(token, reqwest::Method::PUT, path, Some(body)).await
        }

        async fn delete(&self, token: &str, path: &str) -> (StatusCode, Value) {
            self.call(token, reqwest::Method::DELETE, path, None).await
        }

        /// A new city: its id, its Town Square, its invite code and its creator's key.
        async fn city(&self, name: &str, creator: &str) -> (String, String, String, String) {
            let (code, made) =
                self.post("", "/api/cities", json!({ "name": name, "your_name": creator })).await;
            assert_eq!(code, StatusCode::OK, "{made}");
            (
                made["city"]["id"].as_str().unwrap().to_string(),
                made["city"]["square_id"].as_str().unwrap().to_string(),
                made["invite_url"].as_str().unwrap().rsplit('/').next().unwrap().to_string(),
                made["owner_key"].as_str().unwrap().to_string(),
            )
        }
    }

    fn key_of(reply: &Value) -> String {
        reply["owner_key"].as_str().unwrap().to_string()
    }

    fn sorted_keys(value: &Value) -> Vec<String> {
        let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }

    #[tokio::test]
    async fn a_join_that_doesnt_accept_the_rules_is_refused_before_anyone_is_made() {
        let h = Http::start().await;
        let (city, _, code, ann) = h.city("Elm", "Ann").await;
        let (ok, reply) = h
            .put(
                &ann,
                &format!("/api/cities/{city}/rules"),
                json!({ "min_vouches": 0, "rules_text": "Be kind to the bots." }),
            )
            .await;
        assert_eq!((ok, reply), (StatusCode::OK, json!({ "ok": true })));
        let (_, preview) = h.get("", &format!("/api/city-invites/{code}")).await;
        assert_eq!(preview["rules_text"], "Be kind to the bots.");
        assert_eq!(preview["vouches_needed"], 0);

        let people = h.state.db.person_count().unwrap();
        for body in
            [json!({ "your_name": "Cy" }), json!({ "your_name": "Cy", "accept_rules": false })]
        {
            let (refused, said) = h.post("", &format!("/api/city-invites/{code}/join"), body).await;
            assert_eq!(refused, StatusCode::CONFLICT, "{said}");
        }
        assert_eq!(h.state.db.person_count().unwrap(), people, "no orphan account");

        // Someone signed in who isn't in yet gets the same answer, and stays out.
        let (_, _, _, eve) = h.city("Oak", "Eve").await;
        let people = h.state.db.person_count().unwrap();
        let (refused, _) = h.post(&eve, &format!("/api/city-invites/{code}/join"), json!({})).await;
        assert_eq!(refused, StatusCode::CONFLICT);
        assert_eq!(h.get(&eve, &format!("/api/cities/{city}")).await.0, StatusCode::NOT_FOUND);

        let (joined, reply) = h
            .post(
                "",
                &format!("/api/city-invites/{code}/join"),
                json!({ "your_name": "Cy", "accept_rules": true }),
            )
            .await;
        assert_eq!(joined, StatusCode::OK, "{reply}");
        assert_eq!(h.state.db.person_count().unwrap(), people + 1);
        assert_eq!(
            (&reply["city_id"], &reply["waiting"], &reply["vouches"], &reply["vouches_needed"]),
            (&json!(city), &json!(false), &json!(0), &json!(0))
        );
        // Using the link again as a member asks for nothing.
        let cy = key_of(&reply);
        let (again, reply) =
            h.post(&cy, &format!("/api/city-invites/{code}/join"), json!({})).await;
        assert_eq!(again, StatusCode::OK, "{reply}");
        assert_eq!(reply["waiting"], false);
        assert_eq!(h.get(&cy, &format!("/api/cities/{city}")).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_gate_routes_do_what_the_rules_say() {
        let h = Http::start().await;
        let (city, square, code, ann) = h.city("Elm", "Ann").await;
        let (_, reply) = h
            .post("", &format!("/api/city-invites/{code}/join"), json!({ "your_name": "Bo" }))
            .await;
        let bo = key_of(&reply);
        let bo_id = reply["person"]["id"].as_str().unwrap().to_string();
        let (_, scout) = h
            .post(&bo, &format!("/api/hangouts/{square}/agents"), json!({ "name": "Scout" }))
            .await;
        let scout = scout["token"].as_str().unwrap().to_string();
        let (_, eve) =
            h.post("", "/api/hangouts", json!({ "name": "Club", "your_name": "Eve" })).await;
        let eve = key_of(&eve);

        // Only the keeper asks for vouches, and no more than the people there are.
        let rules = format!("/api/cities/{city}/rules");
        assert_eq!(h.put(&bo, &rules, json!({ "min_vouches": 1 })).await.0, StatusCode::FORBIDDEN);
        assert_eq!(
            h.put(&scout, &rules, json!({ "min_vouches": 1 })).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(h.put(&eve, &rules, json!({ "min_vouches": 1 })).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            h.put("", &rules, json!({ "min_vouches": 1 })).await.0,
            StatusCode::UNAUTHORIZED
        );
        for bad in [
            json!({ "min_vouches": 3 }),
            json!({ "min_vouches": -1 }),
            json!({ "min_vouches": "two" }),
            json!({ "min_vouches": 1.5 }),
            json!({ "rules_text": 5 }),
            json!({ "min_gatherings": 2 }),
        ] {
            assert_eq!(h.put(&ann, &rules, bad.clone()).await.0, StatusCode::BAD_REQUEST, "{bad}");
        }
        assert_eq!(
            h.put(&ann, &rules, json!({ "min_vouches": 2, "rules_text": "" })).await.1,
            json!({ "ok": true })
        );
        assert_eq!(
            h.get(&bo, &format!("/api/cities/{city}")).await.1["rules"],
            json!({ "min_vouches": 2, "rules_text": "" })
        );

        // Cy uses the link: an account, a place at the gate, and a city that is a 404.
        let (joined, reply) = h
            .post("", &format!("/api/city-invites/{code}/join"), json!({ "your_name": "Cy" }))
            .await;
        assert_eq!(joined, StatusCode::OK, "{reply}");
        assert_eq!(
            (&reply["waiting"], &reply["vouches"], &reply["vouches_needed"]),
            (&json!(true), &json!(0), &json!(2))
        );
        let (cy, cy_id) = (key_of(&reply), reply["person"]["id"].as_str().unwrap().to_string());
        assert_eq!(h.get(&cy, &format!("/api/cities/{city}")).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            h.get(&cy, &format!("/api/cities/{city}/people/{cy_id}/standing")).await.0,
            StatusCode::NOT_FOUND
        );
        let (_, me) = h.get(&cy, "/api/me").await;
        assert_eq!(
            me["waiting"],
            json!([{ "city_id": city, "name": "Elm", "vouches": 0, "vouches_needed": 2 }])
        );
        assert_eq!(h.get(&ann, "/api/me").await.1["waiting"], json!([]));
        assert!(h.get(&scout, "/api/me").await.1.get("waiting").is_none());

        // The gate is for people in the city and not their bots.
        let (_, seen) = h.get(&ann, &format!("/api/cities/{city}")).await;
        assert_eq!(seen["gate"].as_array().unwrap().len(), 1);
        assert_eq!(
            sorted_keys(&seen["gate"][0]),
            ["asked_at", "color", "name", "person_id", "vouches", "you_vouched"]
        );
        assert!(h.get(&scout, &format!("/api/cities/{city}")).await.1.get("gate").is_none());

        // Bots read standing and nothing else; strangers read nothing.
        let vouch_path = format!("/api/cities/{city}/people/{cy_id}/vouch");
        let decline_path = format!("/api/cities/{city}/gate/{cy_id}/decline");
        let standing_path = format!("/api/cities/{city}/people/{cy_id}/standing");
        assert_eq!(h.post(&scout, &vouch_path, json!({})).await.0, StatusCode::FORBIDDEN);
        assert_eq!(h.delete(&scout, &vouch_path).await.0, StatusCode::FORBIDDEN);
        assert_eq!(h.post(&scout, &decline_path, json!({})).await.0, StatusCode::FORBIDDEN);
        assert_eq!(h.get(&scout, &standing_path).await.0, StatusCode::OK);
        for path in [&vouch_path, &decline_path] {
            assert_eq!(h.post(&eve, path, json!({})).await.0, StatusCode::NOT_FOUND, "{path}");
        }
        assert_eq!(h.get(&eve, &standing_path).await.0, StatusCode::NOT_FOUND);
        assert_eq!(h.post(&bo, &decline_path, json!({})).await.0, StatusCode::FORBIDDEN);
        assert_eq!(
            h.post(&ann, &format!("/api/cities/{city}/people/{}/vouch", "nobody"), json!({}))
                .await
                .0,
            StatusCode::NOT_FOUND
        );

        // Two vouches let Cy in, the first doesn't, and the reply says which.
        assert_eq!(
            h.post(&ann, &vouch_path, json!({})).await.1,
            json!({ "ok": true, "vouches": 1, "let_in": false })
        );
        assert_eq!(
            h.post(&ann, &vouch_path, json!({})).await.1,
            json!({ "ok": true, "vouches": 1, "let_in": false })
        );
        assert_eq!(
            h.post(&bo, &vouch_path, json!({})).await.1,
            json!({ "ok": true, "vouches": 2, "let_in": true })
        );
        assert_eq!(h.get(&cy, &format!("/api/cities/{city}")).await.0, StatusCode::OK);
        assert_eq!(h.get(&cy, "/api/me").await.1["waiting"], json!([]));
        let (_, shown) = h.get(&ann, &standing_path).await;
        assert_eq!(
            sorted_keys(&shown),
            ["gatherings", "good_trips", "member_days", "member_since", "sent_home", "vouches"]
        );
        assert_eq!(shown["vouches"].as_array().unwrap().len(), 2);
        assert_eq!(h.delete(&ann, &vouch_path).await.1, json!({ "ok": true }));
        assert_eq!(h.get(&cy, &format!("/api/cities/{city}")).await.0, StatusCode::OK);
        assert_eq!(h.get(&ann, &standing_path).await.1["vouches"], json!(["Bo"]));
        let self_vouch = format!("/api/cities/{city}/people/{bo_id}/vouch");
        assert_eq!(h.post(&bo, &self_vouch, json!({})).await.0, StatusCode::BAD_REQUEST);

        // Only the keeper turns someone away, and it takes them off the gate.
        let (_, reply) = h
            .post("", &format!("/api/city-invites/{code}/join"), json!({ "your_name": "Dee" }))
            .await;
        let (dee, dee_id) = (key_of(&reply), reply["person"]["id"].as_str().unwrap().to_string());
        let decline = format!("/api/cities/{city}/gate/{dee_id}/decline");
        assert_eq!(h.post(&bo, &decline, json!({})).await.0, StatusCode::FORBIDDEN);
        assert_eq!(h.post(&ann, &decline, json!({})).await.1, json!({ "ok": true }));
        assert_eq!(h.get(&dee, "/api/me").await.1["waiting"], json!([]));
        assert_eq!(h.post(&ann, &decline, json!({})).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_house_rules_route_takes_numbers_in_range_from_a_resident_who_is_a_person() {
        let h = Http::start().await;
        let (city, square, _, ann) = h.city("Elm", "Ann").await;
        let (_, house) = h
            .post(&ann, &format!("/api/cities/{city}/houses"), json!({ "name": "Ann's House" }))
            .await;
        let house_id = house["house"]["id"].as_str().unwrap().to_string();
        let living = house["living_room_id"].as_str().unwrap().to_string();
        let path = format!("/api/houses/{house_id}/rules");
        let (_, helper) = h
            .post(&ann, &format!("/api/hangouts/{living}/agents"), json!({ "name": "Helper" }))
            .await;
        let helper = helper["token"].as_str().unwrap().to_string();
        let (_, casa) = h
            .post(&ann, &format!("/api/houses/{house_id}/house-bot"), json!({ "name": "Casa" }))
            .await;
        let casa = casa["token"].as_str().unwrap().to_string();
        let (_, eve) =
            h.post("", "/api/hangouts", json!({ "name": "Club", "your_name": "Eve" })).await;
        let eve = key_of(&eve);
        let _ = square;

        let all = json!({ "min_member_days": 7, "min_gatherings": 2, "min_good_trips": 1, "min_vouches": 3 });
        assert_eq!(h.put(&helper, &path, all.clone()).await.0, StatusCode::FORBIDDEN);
        assert_eq!(h.put(&casa, &path, all.clone()).await.0, StatusCode::FORBIDDEN);
        assert_eq!(h.put(&eve, &path, all.clone()).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            h.put(&ann, "/api/houses/nope/rules", all.clone()).await.0,
            StatusCode::NOT_FOUND
        );
        for bad in [
            json!({ "min_member_days": 366 }),
            json!({ "min_gatherings": 101 }),
            json!({ "min_good_trips": 101 }),
            json!({ "min_vouches": 6 }),
            json!({ "min_vouches": -1 }),
            json!({ "min_gatherings": "two" }),
            json!({ "min_gatherings": 2.5 }),
        ] {
            assert_eq!(h.put(&ann, &path, bad.clone()).await.0, StatusCode::BAD_REQUEST, "{bad}");
        }
        assert_eq!(
            h.get(&ann, &format!("/api/houses/{house_id}")).await.1["rules"]["min_gatherings"],
            0
        );
        assert_eq!(h.put(&ann, &path, all.clone()).await.1, json!({ "ok": true }));

        let (_, view) = h.get(&ann, &format!("/api/houses/{house_id}")).await;
        assert_eq!(view["rules"], all);
        assert_eq!(view["standing"], json!({}));
        let (_, city_page) = h.get(&ann, &format!("/api/cities/{city}")).await;
        assert_eq!(city_page["houses"][0]["requirements"], all);
    }
}
