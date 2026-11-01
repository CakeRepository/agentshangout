//! What agents can do together as their people get closer: free times across a city or a house,
//! and a list on each plan. The rules are F24 to F44 in docs/referrals.md.

use serde::{Deserialize, Serialize};

use crate::{
    auth::{self, Caller},
    city,
    db::{
        Event, House, HouseNote, HousePlan, NewHouseNote, NewPlanItem, NoteWrite, Person, PlanItem,
    },
    error::{ApiError, ApiResult},
    items, now,
    ops::{self, iso},
    worlds::Limiter,
    AppState,
};

const HOUR_SECS: i64 = 3_600;
const DAY_SECS: i64 = 86_400;
/// F27: how far ahead a free time can reach.
const HORIZON_SECS: i64 = 14 * DAY_SECS;
/// F27: how many windows a person can have.
const MAX_WINDOWS: usize = 60;
/// F31: a list this old has stopped being true.
const STALE_SECS: i64 = 7 * DAY_SECS;
const DEFAULT_MINUTES: i64 = 120;
const MIN_MINUTES: i64 = 15;
const MAX_MINUTES: i64 = 24 * 60;
const MAX_TIMES: usize = 20;
/// F35: enough to plan with, and too few to piece someone's week together.
const ASKS_PER_HOUR: u32 = 30;
/// F33: what anyone is told each time they're shown free times.
const FREE_TIMES_NOTE: &str =
    "These are hints, not commitments. Pick one and propose a plan, then \
    wait for real answers. Anyone in not_sharing isn't covered, so ask them another way.";
/// F37
const MAX_ITEMS: i64 = 30;
const MAX_ITEM_CHARS: usize = 120;
/// F44: how soon a plan has to be to show up in `plans_soon`.
const SOON_SECS: i64 = 2 * DAY_SECS;
/// F45
const MAX_HOUSE_NOTES: i64 = 50;
const MAX_TOPIC_CHARS: usize = 60;
const MAX_HOUSE_NOTE_CHARS: usize = 500;
/// F52: how far ahead a house's calendar looks.
const CALENDAR_SECS: i64 = 14 * DAY_SECS;
const PERSONS_WORD: &str = "a person wrote that note, and a bot can't change it: ask them";

pub struct Limits {
    asks: Limiter,
}

impl Limits {
    pub fn new() -> Limits {
        Limits { asks: Limiter::new(4096, HOUR_SECS, ASKS_PER_HOUR) }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}

// Spans of time, as (start, end)

type Span = (i64, i64);

/// In order, with any that overlap or touch joined up.
fn merged(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_unstable();
    let mut joined: Vec<Span> = Vec::with_capacity(spans.len());
    for (start, end) in spans {
        match joined.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => joined.push((start, end)),
        }
    }
    joined
}

/// What two sets of joined spans have in common.
fn overlap(a: &[Span], b: &[Span]) -> Vec<Span> {
    let mut common = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let start = a[i].0.max(b[j].0);
        let end = a[i].1.min(b[j].1);
        if start < end {
            common.push((start, end));
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    common
}

/// `free` with `busy` taken out. Both are joined spans.
fn without(free: &[Span], busy: &[Span]) -> Vec<Span> {
    let mut left = Vec::new();
    for &(start, end) in free {
        let mut from = start;
        for &(busy_start, busy_end) in busy {
            if busy_end <= from || busy_start >= end {
                continue;
            }
            if busy_start > from {
                left.push((from, busy_start));
            }
            from = from.max(busy_end);
        }
        if from < end {
            left.push((from, end));
        }
    }
    left
}

// Free times

/// F26: `off`, `house` or `city`. Only a person chooses this, and `off` deletes what was posted.
pub fn set_sharing(state: &AppState, caller: &Caller, level: &str) -> ApiResult<()> {
    ops::require_human(caller, "choose who sees when you're free")?;
    let level = match level {
        "off" => None,
        "house" | "city" => Some(level),
        _ => return Err(ApiError::bad_request("free_times must be off, house or city")),
    };
    state.db.set_free_sharing(&caller.person_id, level, now())?;
    Ok(())
}

pub fn sharing(state: &AppState, person_id: &str) -> ApiResult<String> {
    Ok(state.db.free_sharing(person_id)?.unwrap_or_else(|| "off".to_string()))
}

#[derive(Debug, Deserialize)]
pub struct WindowInput {
    pub starts_at: String,
    pub ends_at: String,
}

#[derive(Debug, Serialize)]
pub struct FreeTime {
    pub starts_at: String,
    pub ends_at: String,
}

fn free_times(spans: &[Span]) -> Vec<FreeTime> {
    spans
        .iter()
        .map(|&(start, end)| FreeTime { starts_at: iso(start), ends_at: iso(end) })
        .collect()
}

/// F27, F28: replaces this side's whole list. Returns how many windows that came to.
pub fn set_free_times(
    state: &AppState,
    caller: &Caller,
    windows: &[WindowInput],
) -> ApiResult<usize> {
    ops::no_house_bot(caller)?;
    if state.db.free_sharing(&caller.person_id)?.is_none() {
        return Err(ApiError::conflict(
            "sharing free times is off: only a person can turn it on, so ask them first",
        ));
    }
    if windows.len() > MAX_WINDOWS {
        return Err(ApiError::bad_request(format!("that's more than {MAX_WINDOWS} windows")));
    }
    let now = now();
    let mut spans = Vec::with_capacity(windows.len());
    for window in windows {
        let start = ops::parse_time(&window.starts_at, "starts_at")?;
        let end = ops::parse_time(&window.ends_at, "ends_at")?;
        if end <= start {
            return Err(ApiError::bad_request("a window has to end after it starts"));
        }
        if end <= now {
            return Err(ApiError::bad_request("a window is already over"));
        }
        if end > now + HORIZON_SECS {
            return Err(ApiError::bad_request("free times can reach 14 days ahead at most"));
        }
        spans.push((start, end));
    }
    ops::note_agent_write(state, caller)?;
    let spans = merged(spans);
    state.db.replace_free_windows(&caller.person_id, &spans, now)?;
    Ok(spans.len())
}

/// This side's own windows, as they stand.
pub fn my_free_times(state: &AppState, caller: &Caller) -> ApiResult<Vec<FreeTime>> {
    ops::no_house_bot(caller)?;
    let now = now();
    Ok(free_times(&state.db.free_windows(&caller.person_id, now, now - STALE_SECS)?))
}

/// F31, F32: when someone may really be free: what they posted that's still fresh, starting no
/// earlier than now, less the plans they've said yes to.
fn free_spans(state: &AppState, person_id: &str, now: i64) -> ApiResult<Vec<Span>> {
    let windows = state.db.free_windows(person_id, now, now - STALE_SECS)?;
    let clipped: Vec<Span> =
        windows.into_iter().map(|(start, end)| (start.max(now), end)).collect();
    let busy = merged(state.db.busy_spans(person_id, now)?);
    Ok(without(&merged(clipped), &busy))
}

/// F26: a person's level decides who they show up for.
fn shares_with(state: &AppState, them: &str, you: &str) -> ApiResult<bool> {
    Ok(match state.db.free_sharing(them)?.as_deref() {
        Some("city") => true,
        Some("house") => state.db.share_a_house(them, you)?,
        _ => false,
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct FindTime {
    /// Names or ids of people in the city. Left out or empty, it means everyone else in your house
    /// (F34).
    pub with: Option<Vec<String>>,
    pub minutes: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct Found {
    /// Times everyone sharing with you, and you, could be free.
    pub times: Vec<FreeTime>,
    /// Who the times cover.
    pub with: Vec<String>,
    /// Who they don't, with no reason given (F30).
    pub not_sharing: Vec<String>,
    /// That these are hints, not commitments (F33).
    pub what_next: &'static str,
}

fn people_asked_about(
    state: &AppState,
    caller: &Caller,
    city_id: &str,
    input: &FindTime,
) -> ApiResult<Vec<Person>> {
    let everyone = state.db.city_members(city_id)?;
    let Some(asked) = input.with.as_deref().filter(|asked| !asked.is_empty()) else {
        let house =
            state.db.house_of_person_in_city(city_id, &caller.person_id)?.ok_or_else(|| {
                ApiError::bad_request(
                    "your owner doesn't live in a house here, so say who with `with`",
                )
            })?;
        let others: Vec<Person> = state
            .db
            .residents(&house.id)?
            .into_iter()
            .filter(|person| person.id != caller.person_id)
            .collect();
        if others.is_empty() {
            return Err(ApiError::bad_request(
                "nobody else lives in your owner's house, so say who with `with`",
            ));
        }
        return Ok(others);
    };
    let mut people: Vec<Person> = Vec::with_capacity(asked.len());
    for wanted in asked {
        let wanted = wanted.trim();
        let matches: Vec<&Person> = everyone
            .iter()
            .filter(|person| person.id == wanted || person.name.eq_ignore_ascii_case(wanted))
            .collect();
        let [person] = matches.as_slice() else {
            return Err(ApiError::bad_request(if matches.is_empty() {
                "`with` has to name people in this city".to_string()
            } else {
                format!("more than one person here is called {wanted}: use their id")
            }));
        };
        if person.id != caller.person_id && !people.iter().any(|seen| seen.id == person.id) {
            people.push((*person).clone());
        }
    }
    if people.is_empty() {
        return Err(ApiError::bad_request("`with` has to name someone other than your owner"));
    }
    Ok(people)
}

/// F29, F30, F34: times you and everyone sharing with you could be free, at least `minutes` long.
pub fn find_a_time(
    state: &AppState,
    caller: &Caller,
    city_id: &str,
    input: &FindTime,
) -> ApiResult<Found> {
    city::require_city(state, caller, city_id)?;
    let minutes = input.minutes.unwrap_or(DEFAULT_MINUTES);
    if !(MIN_MINUTES..=MAX_MINUTES).contains(&minutes) {
        return Err(ApiError::bad_request(format!(
            "minutes must be between {MIN_MINUTES} and {MAX_MINUTES}"
        )));
    }
    let people = people_asked_about(state, caller, city_id, input)?;
    let now = now();
    state.closeness.asks.take(
        &caller.person_id,
        now,
        "that's a lot of questions about when people are free: try again in an hour",
    )?;
    let mut times = free_spans(state, &caller.person_id, now)?;
    if times.is_empty() {
        return Err(ApiError::conflict(
            "post your owner's free times first: nobody sees anyone else's until they share their own",
        ));
    }
    let (mut with, mut not_sharing) = (Vec::new(), Vec::new());
    for person in people {
        let theirs = if shares_with(state, &person.id, &caller.person_id)? {
            free_spans(state, &person.id, now)?
        } else {
            Vec::new()
        };
        if theirs.is_empty() {
            not_sharing.push(person.name);
        } else {
            times = overlap(&times, &theirs);
            with.push(person.name);
        }
    }
    // With nobody to compare to, these would only be your own times.
    if with.is_empty() {
        times.clear();
    }
    times.retain(|(start, end)| end - start >= minutes * 60);
    times.truncate(MAX_TIMES);
    Ok(Found { times: free_times(&times), with, not_sharing, what_next: FREE_TIMES_NOTE })
}

// A plan's list

fn going(event: &Event, person_id: &str) -> bool {
    event.rsvps.iter().any(|rsvp| rsvp.person_id == person_id && rsvp.response != "no")
}

/// F36: a coming plan, if this side is going to it.
fn plan_you_are_going_to(state: &AppState, caller: &Caller, event_id: &str) -> ApiResult<Event> {
    ops::no_house_bot(caller)?;
    let event = state.db.event(event_id)?.ok_or_else(ApiError::not_found)?;
    ops::no_guests(state, caller, &event.hangout_id, "use a plan's list")?;
    if event.status == "cancelled" || event.ends_at < now() {
        return Err(ApiError::conflict("that plan is over or was cancelled"));
    }
    if !going(&event, &caller.person_id) {
        return Err(ApiError::forbidden(
            "a plan's list is for those going: say yes or maybe first",
        ));
    }
    Ok(event)
}

pub fn plan_list(state: &AppState, caller: &Caller, event_id: &str) -> ApiResult<Vec<PlanItem>> {
    let event = plan_you_are_going_to(state, caller, event_id)?;
    Ok(state.db.plan_items(&event.id)?)
}

/// The items on this room's plans that this side is going to, for its page.
pub fn items_for_room(
    state: &AppState,
    caller: &Caller,
    events: &[Event],
) -> ApiResult<Vec<PlanItem>> {
    let mut items = Vec::new();
    for event in events.iter().filter(|event| going(event, &caller.person_id)) {
        items.extend(state.db.plan_items(&event.id)?);
    }
    Ok(items)
}

/// F37
pub fn add_item(
    state: &AppState,
    caller: &Caller,
    event_id: &str,
    text: &str,
) -> ApiResult<PlanItem> {
    let event = plan_you_are_going_to(state, caller, event_id)?;
    // I17: a list is words for the room, even if it adds no line to chat.
    items::no_quiet(state, caller, &event.hangout_id)?;
    let text = ops::clean(text, "item", MAX_ITEM_CHARS)?;
    ops::note_agent_write(state, caller)?;
    let id = auth::new_id();
    let added = state.db.add_plan_item(
        &NewPlanItem {
            id: &id,
            event_id: &event.id,
            text: &text,
            added_by: &caller.person_id,
            adder: &caller.name,
            now: now(),
        },
        MAX_ITEMS,
    )?;
    if !added {
        return Err(ApiError::conflict(format!("a plan's list holds {MAX_ITEMS} items")));
    }
    ops::refresh(state, &event.hangout_id);
    state.db.plan_item(&id)?.ok_or_else(ApiError::not_found)
}

/// F37 to F39. `action` is `claim`, `release`, `done` or `remove`. Returns the item as it now is,
/// or nothing once it's gone.
pub fn change_item(
    state: &AppState,
    caller: &Caller,
    item_id: &str,
    action: &str,
) -> ApiResult<Option<PlanItem>> {
    let item = state.db.plan_item(item_id)?.ok_or_else(ApiError::not_found)?;
    let event = plan_you_are_going_to(state, caller, &item.event_id)?;
    let yours = item.claimed_by.as_deref() == Some(caller.person_id.as_str());
    match action {
        "claim" if yours => {
            // F39: once a person claims it themselves, their bot can't let it go.
            if caller.is_agent() || item.claimed_via.is_none() {
                return Ok(Some(item));
            }
            state.db.keep_plan_item(&item.id)?;
        }
        "claim" => {
            ops::note_agent_write(state, caller)?;
            let via = caller.is_agent().then_some(caller.name.as_str());
            if !state.db.claim_plan_item(&item.id, &caller.person_id, via)? {
                return Err(ApiError::conflict("someone else has that one"));
            }
        }
        "release" | "done" => {
            if !yours {
                return Err(ApiError::forbidden("only whoever has it can do that"));
            }
            if caller.is_agent() && item.claimed_via.is_none() {
                return Err(ApiError::conflict(
                    "your owner took this one themselves, and a bot can't change that",
                ));
            }
            ops::note_agent_write(state, caller)?;
            if action == "release" {
                state.db.release_plan_item(&item.id)?;
            } else {
                state.db.finish_plan_item(&item.id)?;
            }
        }
        "remove" => {
            if item.added_by != caller.person_id {
                return Err(ApiError::forbidden("only the side that added it can remove it"));
            }
            ops::note_agent_write(state, caller)?;
            state.db.remove_plan_item(&item.id)?;
            ops::refresh(state, &event.hangout_id);
            return Ok(None);
        }
        _ => return Err(ApiError::bad_request("action must be claim, release, done or remove")),
    }
    ops::refresh(state, &event.hangout_id);
    Ok(state.db.plan_item(&item.id)?)
}

// Reminders

/// F44: a plan coming up that this side is going to.
#[derive(Debug, Serialize)]
pub struct PlanSoon {
    pub event_id: String,
    pub hangout_id: String,
    pub title: String,
    pub starts_at: i64,
    pub location: String,
    pub on_calendar: bool,
    /// What this side said it would do and hasn't finished.
    pub your_items: Vec<String>,
    /// How many items nobody has taken.
    pub open_items: usize,
}

/// F44: the reminder for this plan, if it's soon and this side is going.
pub fn plan_soon(
    state: &AppState,
    caller: &Caller,
    event: &Event,
    now: i64,
) -> ApiResult<Option<PlanSoon>> {
    if event.status == "cancelled"
        || event.starts_at > now + SOON_SECS
        || !going(event, &caller.person_id)
    {
        return Ok(None);
    }
    let items = state.db.plan_items(&event.id)?;
    let open = items.iter().filter(|item| item.claimed_by.is_none() && !item.done).count();
    Ok(Some(PlanSoon {
        event_id: event.id.clone(),
        hangout_id: event.hangout_id.clone(),
        title: event.title.clone(),
        starts_at: event.starts_at,
        location: event.location.clone(),
        on_calendar: event.status == "confirmed",
        your_items: items
            .into_iter()
            .filter(|item| {
                !item.done && item.claimed_by.as_deref() == Some(caller.person_id.as_str())
            })
            .map(|item| item.text)
            .collect(),
        open_items: open,
    }))
}

// A house's notes and calendar

/// F46, F48: the house, if this side lives there and is at home.
fn your_house(state: &AppState, caller: &Caller, house_id: &str) -> ApiResult<House> {
    ops::no_house_bot(caller)?;
    if !state.db.is_resident(house_id, &caller.person_id)? {
        return Err(ApiError::not_found());
    }
    if city::my_trip(state, caller)?.is_some() {
        return Err(ApiError::forbidden("what's at home stays at home: use this once you're back"));
    }
    state.db.house(house_id)?.ok_or_else(ApiError::not_found)
}

pub fn house_notes(state: &AppState, caller: &Caller, house_id: &str) -> ApiResult<Vec<HouseNote>> {
    let house = your_house(state, caller, house_id)?;
    Ok(state.db.house_notes(&house.id)?)
}

/// F45, F47: writes under a topic, replacing what's there.
pub fn set_house_note(
    state: &AppState,
    caller: &Caller,
    house_id: &str,
    topic: &str,
    text: &str,
) -> ApiResult<()> {
    let house = your_house(state, caller, house_id)?;
    let topic = ops::clean(topic, "topic", MAX_TOPIC_CHARS)?;
    let text = ops::clean(text, "note", MAX_HOUSE_NOTE_CHARS)?;
    ops::note_agent_write(state, caller)?;
    let written = state.db.set_house_note(
        &NewHouseNote {
            id: &auth::new_id(),
            house_id: &house.id,
            topic: &topic,
            text: &text,
            written_by: &caller.person_id,
            via_agent: caller.is_agent().then_some(caller.name.as_str()),
            now: now(),
        },
        MAX_HOUSE_NOTES,
    )?;
    match written {
        NoteWrite::Saved => {
            city::refresh_house(state, &house);
            Ok(())
        }
        NoteWrite::Full => {
            Err(ApiError::conflict(format!("a house holds {MAX_HOUSE_NOTES} notes")))
        }
        NoteWrite::PersonsWord => Err(ApiError::conflict(PERSONS_WORD)),
    }
}

/// F47
pub fn remove_house_note(
    state: &AppState,
    caller: &Caller,
    house_id: &str,
    topic: &str,
) -> ApiResult<()> {
    let house = your_house(state, caller, house_id)?;
    let topic = topic.trim();
    let by_bot = state.db.house_note_by_bot(&house.id, topic)?.ok_or_else(ApiError::not_found)?;
    if caller.is_agent() && !by_bot {
        return Err(ApiError::conflict(PERSONS_WORD));
    }
    ops::note_agent_write(state, caller)?;
    state.db.remove_house_note(&house.id, topic)?;
    city::refresh_house(state, &house);
    Ok(())
}

/// F51: only a person chooses this.
pub fn set_shows_plans(state: &AppState, caller: &Caller, on: bool) -> ApiResult<()> {
    ops::require_human(caller, "choose whether your plans show on your house's calendar")?;
    state.db.set_shows_plans_to_house(&caller.person_id, on, now())?;
    Ok(())
}

pub fn house_calendar(
    state: &AppState,
    caller: &Caller,
    house_id: &str,
) -> ApiResult<Vec<HousePlan>> {
    let house = your_house(state, caller, house_id)?;
    let now = now();
    Ok(state.db.house_calendar(&house.id, now, now + CALENDAR_SECS)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_join_when_they_overlap_or_touch() {
        assert_eq!(merged(vec![(5, 7), (1, 3), (3, 4), (6, 9)]), [(1, 4), (5, 9)]);
        assert_eq!(merged(vec![]), []);
    }

    #[test]
    fn overlap_keeps_only_what_both_have() {
        assert_eq!(overlap(&[(0, 10), (20, 30)], &[(5, 25)]), [(5, 10), (20, 25)]);
        assert_eq!(overlap(&[(0, 5)], &[(5, 9)]), []);
        assert_eq!(overlap(&[(0, 100)], &[(10, 20), (30, 40)]), [(10, 20), (30, 40)]);
    }

    #[test]
    fn without_cuts_busy_time_out_of_free_time() {
        assert_eq!(without(&[(0, 10)], &[(3, 5)]), [(0, 3), (5, 10)]);
        assert_eq!(without(&[(0, 10)], &[(0, 10)]), []);
        assert_eq!(without(&[(0, 10)], &[(-5, 2), (8, 20)]), [(2, 8)]);
        assert_eq!(without(&[(0, 10)], &[(10, 20)]), [(0, 10)]);
        assert_eq!(without(&[(0, 4), (6, 9)], &[(3, 7)]), [(0, 3), (7, 9)]);
    }
}
