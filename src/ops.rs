//! What can happen in a hangout. The REST API and the MCP tools both call these functions, so
//! every membership and calendar rule is enforced in one place.

use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{
    auth::{self, Caller},
    city,
    db::{
        Agent, CheckIn, Event, FeedEvent, Hangout, Message, NewCheckIn, NewEvent, NewMessage,
        NewOwnerQuestion, OwnerQuestion, Pass, Person, SceneRequest, Trip, Visit,
    },
    error::{ApiError, ApiResult},
    ical, items, looks, notify, now, spaces, AppState, HubEvent,
};

pub const MAX_AGENT_WRITES_PER_MINUTE: u32 = 30;
pub const MAX_NAME_CHARS: usize = 60;
pub const MAX_MESSAGE_CHARS: usize = 4000;
const MAX_TITLE_CHARS: usize = 120;
const MAX_LOCATION_CHARS: usize = 200;
const MAX_NOTE_CHARS: usize = 500;
const MAX_STATUS_CHARS: usize = 120;
const ACTIVITY_KINDS: [&str; 1] = ["computer"];
const MAX_ACTIVITY_VIA_CHARS: usize = 40;
/// Reports expire instead of being cleared, so an agent that crashes doesn't stay at the desk.
const ACTIVITY_SECS: i64 = 60;
const DEFAULT_DURATION_MINUTES: i64 = 120;
const MAX_DURATION_MINUTES: i64 = 7 * 24 * 60;
const DEFAULT_MIN_YES: i64 = 2;
/// Also the limit for a trip's goal.
pub(crate) const MAX_ABOUT_CHARS: usize = 500;
const DEFAULT_CHECK_IN_MINUTES: i64 = 60;
const MAX_CHECK_IN_MINUTES: i64 = 7 * 24 * 60;
pub(crate) const DAY_SECS: i64 = 24 * 60 * 60;
/// Finished events stay in feeds this long so calendars don't drop them right away.
const FEED_HISTORY_SECS: i64 = 30 * 24 * 60 * 60;
/// A question for a bot's owner that nobody answers lapses. People go away for a few days, so this
/// is longer than a check-in.
const QUESTION_LAPSE_SECS: i64 = 7 * DAY_SECS;
/// However much a bot wants to know, this is as many questions as it can have waiting. Each one
/// pings its owner.
const MAX_OPEN_QUESTIONS: i64 = 5;

pub fn clean(text: &str, field: &str, max_chars: usize) -> ApiResult<String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ApiError::bad_request(format!("{field} can't be empty")));
    }
    if text.chars().count() > max_chars {
        return Err(ApiError::bad_request(format!(
            "{field} is too long (limit {max_chars} characters)"
        )));
    }
    Ok(text.to_string())
}

pub(crate) fn clean_optional(
    text: Option<&str>,
    field: &str,
    max_chars: usize,
) -> ApiResult<String> {
    match text.map(str::trim) {
        Some(text) if !text.is_empty() => clean(text, field, max_chars),
        _ => Ok(String::new()),
    }
}

pub fn iso(unix: i64) -> String {
    OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|at| at.format(&Rfc3339).ok())
        .unwrap_or_default()
}

pub(crate) fn parse_time(text: &str, field: &str) -> ApiResult<i64> {
    OffsetDateTime::parse(text.trim(), &Rfc3339).map(OffsetDateTime::unix_timestamp).map_err(|_| {
        ApiError::bad_request(format!(
            "{field} must be an RFC 3339 time with an offset, like 2026-11-07T18:00:00-06:00"
        ))
    })
}

pub fn publish(state: &AppState, hangout_id: &str, kind: &'static str, data: String) {
    // Sending only fails when nobody is watching, which is fine.
    let _ = state.hub.send(HubEvent { hangout_id: hangout_id.to_string(), kind, data });
}

/// Pings one person and their bots in this room.
fn notify_side(
    state: &AppState,
    hangout_id: &str,
    person_id: &str,
    event: &'static str,
    text: &str,
) -> ApiResult<()> {
    let mut owners = vec![person_id.to_string()];
    // Guests are included here because this names one side. `notify_others` only ever names the
    // people who belong in the room, so nobody's check-ins reach a guest.
    owners.extend(
        state
            .db
            .agents_in(hangout_id, now())?
            .into_iter()
            .chain(state.db.visitors_in(hangout_id, now())?)
            .filter(|agent| agent.owner_id == person_id)
            .map(|agent| agent.id),
    );
    let notice =
        notify::Notice { event, hangout_id: hangout_id.to_string(), text: text.to_string() };
    notify::send(state, &owners, &notice);
    Ok(())
}

/// Pings everyone in the room except one side, which can be empty to reach all of them.
fn notify_others(
    state: &AppState,
    hangout_id: &str,
    except_person: &str,
    event: &'static str,
    text: &str,
) -> ApiResult<()> {
    for person in state.db.people_in(hangout_id)? {
        if person.id != except_person {
            notify_side(state, hangout_id, &person.id, event, text)?;
        }
    }
    Ok(())
}

/// Tells open pages to reload the room after a change that isn't a chat line.
pub(crate) fn refresh(state: &AppState, hangout_id: &str) {
    publish(state, hangout_id, "refresh", "{}".to_string());
}

/// Tells open city maps to reload. A city's stream uses the city id as its topic.
pub(crate) fn refresh_city(state: &AppState, city_id: &str) {
    publish(state, city_id, "refresh", "{}".to_string());
}

pub(crate) fn announce(state: &AppState, hangout_id: &str, body: &str) -> ApiResult<()> {
    let message = state.db.insert_message(&NewMessage {
        hangout_id,
        person_id: None,
        agent_id: None,
        speaker: "Hangout",
        kind: "system",
        body,
        now: now(),
    })?;
    publish_message(state, &message);
    Ok(())
}

/// Whether guests can be here: a Porch, or a public floor.
fn has_guests(state: &AppState, hangout_id: &str) -> ApiResult<bool> {
    Ok(state.db.house_for_room(hangout_id)?.is_some_and(|house| house.porch_id == hangout_id)
        || state.db.floor(hangout_id)?.is_some_and(|floor| floor.access == "public"))
}

/// For a line that repeats a plan or a check-in. Guests can't see those, so a Porch or a public
/// floor gets no line at all: the people who belong there have the plan and check-in panels.
pub(crate) fn announce_in_house_only(
    state: &AppState,
    hangout_id: &str,
    body: &str,
) -> ApiResult<()> {
    if has_guests(state, hangout_id)? {
        return Ok(());
    }
    announce(state, hangout_id, body)
}

fn publish_message(state: &AppState, message: &Message) {
    let data = serde_json::to_string(message).unwrap_or_default();
    publish(state, &message.hangout_id, "message", data);
}

/// The room, if the caller is in it. Anyone else gets the same answer as for a hangout that
/// doesn't exist. A guest on a Porch is in it too, so use `no_guests` for anything a guest mustn't do.
pub fn require_room(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<Hangout> {
    if !state.in_room(caller, hangout_id)? {
        return Err(ApiError::not_found());
    }
    state.db.hangout(hangout_id)?.ok_or_else(ApiError::not_found)
}

/// The room, if the caller belongs to it rather than visiting. Guests can watch, talk, share notes,
/// check in and ask for a scene, and nothing else: no plans, no answers, no changing who is here.
pub fn no_guests(
    state: &AppState,
    caller: &Caller,
    hangout_id: &str,
    action: &str,
) -> ApiResult<Hangout> {
    let hangout = require_room(state, caller, hangout_id)?;
    if state.guest_visit(caller, hangout_id)?.is_some() {
        return Err(ApiError::forbidden(format!("guests can't {action}")));
    }
    Ok(hangout)
}

pub(crate) fn require_human(caller: &Caller, action: &str) -> ApiResult<()> {
    if caller.is_agent() {
        return Err(ApiError::forbidden(format!("only a person can {action}")));
    }
    Ok(())
}

/// The house bot reads a queue and reports back, and that is all it can do here.
pub(crate) fn no_house_bot(caller: &Caller) -> ApiResult<()> {
    if caller.is_house {
        return Err(ApiError::forbidden(
            "the house bot can only read its queue, finish requests and set the house status",
        ));
    }
    Ok(())
}

pub(crate) fn note_agent_write(state: &AppState, caller: &Caller) -> ApiResult<()> {
    if let Some(agent_id) = &caller.agent_id {
        if !state.allow_agent_write(agent_id) {
            return Err(ApiError::too_many_requests(format!(
                "slow down: bots get {MAX_AGENT_WRITES_PER_MINUTE} writes a minute"
            )));
        }
        state.db.touch_agent_active(agent_id, now())?;
    }
    Ok(())
}

// Chat

pub fn post_message(
    state: &AppState,
    caller: &Caller,
    hangout_id: &str,
    kind: &str,
    body: &str,
) -> ApiResult<Message> {
    require_room(state, caller, hangout_id)?;
    items::no_quiet(state, caller, hangout_id)?;
    let body = clean(body, "message", MAX_MESSAGE_CHARS)?;
    note_agent_write(state, caller)?;
    spaces::note_walk_in_write(state, caller, hangout_id)?;
    let message = state.db.insert_message(&NewMessage {
        hangout_id,
        person_id: Some(&caller.person_id),
        agent_id: caller.agent_id.as_deref(),
        speaker: &caller.name,
        kind,
        body: &body,
        now: now(),
    })?;
    publish_message(state, &message);
    Ok(message)
}

pub fn set_status(state: &AppState, caller: &Caller, status: Option<&str>) -> ApiResult<()> {
    no_house_bot(caller)?;
    let Some(agent_id) = &caller.agent_id else {
        return Err(ApiError::forbidden("only bots have a status"));
    };
    let status = clean_optional(status, "status", MAX_STATUS_CHARS)?;
    note_agent_write(state, caller)?;
    let status = (!status.is_empty()).then_some(status.as_str());
    state.db.set_agent_status(agent_id, status)?;
    for hangout in state.db.hangouts_for_agent(agent_id, now())? {
        refresh(state, &hangout.id);
    }
    Ok(())
}

/// Shows a bot working through another tool, like a remote-control server. The whole room sees
/// this, so it takes a fixed kind and a bare tool name, with no room for what's being done or to
/// whose machine. It doesn't count against the write limit because hooks fire on every tool call.
pub fn report_activity(state: &AppState, caller: &Caller, kind: &str, via: &str) -> ApiResult<()> {
    no_house_bot(caller)?;
    let Some(agent_id) = &caller.agent_id else {
        return Err(ApiError::forbidden("only bots report activity"));
    };
    if !ACTIVITY_KINDS.contains(&kind) {
        return Err(ApiError::bad_request("kind must be computer"));
    }
    let via = clean(via, "via", MAX_ACTIVITY_VIA_CHARS)?;
    if !via.chars().all(|ch| ch.is_alphanumeric() || " -_.".contains(ch)) {
        return Err(ApiError::bad_request(
            "via is only the tool's name, with letters, numbers, spaces, - _ and .",
        ));
    }
    let now = now();
    let current = state.db.agent(agent_id, now)?.and_then(|agent| agent.activity);
    state.db.record_activity(agent_id, kind, &via, now + ACTIVITY_SECS)?;
    state.db.touch_agent_active(agent_id, now)?;
    let unchanged = current.is_some_and(|current| current.kind == kind && current.via == via);
    if !unchanged {
        for hangout in state.db.hangouts_for_agent(agent_id, now)? {
            refresh(state, &hangout.id);
        }
    }
    Ok(())
}

// Calendar

#[derive(Debug, Default, Deserialize)]
pub struct EventInput {
    pub title: String,
    pub starts_at: String,
    pub ends_at: Option<String>,
    pub duration_minutes: Option<i64>,
    pub location: Option<String>,
    pub notes: Option<String>,
    pub min_yes: Option<i64>,
}

pub fn propose_event(
    state: &AppState,
    caller: &Caller,
    hangout_id: &str,
    input: &EventInput,
) -> ApiResult<Event> {
    no_guests(state, caller, hangout_id, "propose a plan")?;
    // I17: its title goes in the room's chat.
    items::no_quiet(state, caller, hangout_id)?;
    let title = clean(&input.title, "title", MAX_TITLE_CHARS)?;
    let location = clean_optional(input.location.as_deref(), "location", MAX_LOCATION_CHARS)?;
    let notes = clean_optional(input.notes.as_deref(), "notes", MAX_NOTE_CHARS)?;
    let now = now();
    let starts_at = parse_time(&input.starts_at, "starts_at")?;
    if starts_at < now {
        return Err(ApiError::bad_request("starts_at is in the past"));
    }
    let ends_at = match (&input.ends_at, input.duration_minutes) {
        (Some(ends_at), _) => parse_time(ends_at, "ends_at")?,
        (None, minutes) => {
            // Checked before the sum, which a number this large would overflow.
            let minutes = minutes.unwrap_or(DEFAULT_DURATION_MINUTES);
            if !(1..=MAX_DURATION_MINUTES).contains(&minutes) {
                return Err(ApiError::bad_request(format!(
                    "duration_minutes must be between 1 and {MAX_DURATION_MINUTES} (a week)"
                )));
            }
            starts_at + minutes * 60
        }
    };
    if ends_at <= starts_at {
        return Err(ApiError::bad_request("the hangout has to end after it starts"));
    }
    if ends_at - starts_at > MAX_DURATION_MINUTES * 60 {
        return Err(ApiError::bad_request("hangouts can last a week at most"));
    }
    let min_yes = input.min_yes.unwrap_or(DEFAULT_MIN_YES);
    if min_yes < 1 {
        return Err(ApiError::bad_request("min_yes must be at least 1"));
    }
    note_agent_write(state, caller)?;

    let id = auth::new_id();
    state.db.insert_event(&NewEvent {
        id: &id,
        hangout_id,
        title: &title,
        location: &location,
        notes: &notes,
        starts_at,
        ends_at,
        min_yes,
        proposed_by: &caller.person_id,
        proposer: &caller.name,
        now,
    })?;
    // Proposing counts as a yes from the proposer's side.
    let via_agent = caller.is_agent().then_some(caller.name.as_str());
    state.db.upsert_rsvp(&id, &caller.person_id, "yes", via_agent, "", now)?;
    announce_in_house_only(
        state,
        hangout_id,
        &format!(
            "{} proposed \"{title}\". It needs {min_yes} yes to go on the calendar.",
            caller.name
        ),
    )?;
    notify_others(
        state,
        hangout_id,
        &caller.person_id,
        "plan.proposed",
        &format!("{} proposed \"{title}\". Can you make it?", caller.name),
    )?;
    settle(state, &id)?;
    refresh(state, hangout_id);
    state.db.event(&id)?.ok_or_else(ApiError::not_found)
}

pub fn rsvp(
    state: &AppState,
    caller: &Caller,
    event_id: &str,
    response: &str,
    note: Option<&str>,
) -> ApiResult<Event> {
    let event = state.db.event(event_id)?.ok_or_else(ApiError::not_found)?;
    no_guests(state, caller, &event.hangout_id, "answer a plan")?;
    if !matches!(response, "yes" | "maybe" | "no") {
        return Err(ApiError::bad_request("response must be yes, maybe or no"));
    }
    let note = clean_optional(note, "note", MAX_NOTE_CHARS)?;
    // I17: an answer is the record and still counts, but its note is words for the room.
    if !note.is_empty() {
        items::no_quiet(state, caller, &event.hangout_id)?;
    }
    if event.status == "cancelled" {
        return Err(ApiError::conflict("that hangout was cancelled"));
    }
    if event.ends_at < now() {
        return Err(ApiError::conflict("that hangout is over"));
    }
    // A bot answers for its own human only, and never over the human's own answer.
    if caller.is_agent() {
        let answered_by_human = state
            .db
            .rsvp(event_id, &caller.person_id)?
            .is_some_and(|existing| existing.via_agent.is_none());
        if answered_by_human {
            return Err(ApiError::conflict(
                "your human already answered this one, and a bot can't change that",
            ));
        }
    }
    note_agent_write(state, caller)?;
    let via_agent = caller.is_agent().then_some(caller.name.as_str());
    state.db.upsert_rsvp(event_id, &caller.person_id, response, via_agent, &note, now())?;
    settle(state, event_id)?;
    refresh(state, &event.hangout_id);
    state.db.event(event_id)?.ok_or_else(ApiError::not_found)
}

/// Moves an event on or off the calendar to match its yes count.
pub(crate) fn settle(state: &AppState, event_id: &str) -> ApiResult<()> {
    let event = state.db.event(event_id)?.ok_or_else(ApiError::not_found)?;
    let yes = event.rsvps.iter().filter(|rsvp| rsvp.response == "yes").count() as i64;
    if event.status == "proposed" && yes >= event.min_yes {
        state.db.set_event_status(event_id, "confirmed", now())?;
        announce_in_house_only(
            state,
            &event.hangout_id,
            &format!("\"{}\" is on the calendar.", event.title),
        )?;
        notify_others(
            state,
            &event.hangout_id,
            "",
            "plan.confirmed",
            &format!("\"{}\" is on the calendar.", event.title),
        )?;
        publish(state, &event.hangout_id, "confirmed", "{}".to_string());
    } else if event.status == "confirmed" && yes < event.min_yes {
        state.db.set_event_status(event_id, "proposed", now())?;
        announce_in_house_only(
            state,
            &event.hangout_id,
            &format!("\"{}\" came off the calendar: not enough yeses left.", event.title),
        )?;
    }
    Ok(())
}

pub fn cancel_event(state: &AppState, caller: &Caller, event_id: &str) -> ApiResult<Event> {
    let event = state.db.event(event_id)?.ok_or_else(ApiError::not_found)?;
    no_guests(state, caller, &event.hangout_id, "cancel a plan")?;
    if event.proposed_by != caller.person_id {
        return Err(ApiError::forbidden("only the side that proposed it can cancel it"));
    }
    if event.status == "cancelled" {
        return Ok(event);
    }
    note_agent_write(state, caller)?;
    state.db.set_event_status(event_id, "cancelled", now())?;
    announce_in_house_only(
        state,
        &event.hangout_id,
        &format!("{} cancelled \"{}\".", caller.name, event.title),
    )?;
    refresh(state, &event.hangout_id);
    state.db.event(event_id)?.ok_or_else(ApiError::not_found)
}

// Check-ins: asking the room before acting. The server records the answer; it can't stop a bot
// from acting anyway, so the bot guide's rule is that no answer means no.

#[derive(Debug, Default, Deserialize)]
pub struct CheckInInput {
    pub about: String,
    /// A person's name in this room. Left out, anyone on another side can answer.
    pub to: Option<String>,
    pub needs_human: Option<bool>,
    pub expires_minutes: Option<i64>,
}

pub fn create_check_in(
    state: &AppState,
    caller: &Caller,
    hangout_id: &str,
    input: &CheckInInput,
) -> ApiResult<CheckIn> {
    require_room(state, caller, hangout_id)?;
    spaces::no_walk_ins(state, caller, hangout_id, "ask a check-in")?;
    // I17: what it's about goes in the room's chat.
    items::no_quiet(state, caller, hangout_id)?;
    let about = clean(&input.about, "about", MAX_ABOUT_CHARS)?;
    let minutes = input.expires_minutes.unwrap_or(DEFAULT_CHECK_IN_MINUTES);
    if !(1..=MAX_CHECK_IN_MINUTES).contains(&minutes) {
        return Err(ApiError::bad_request("expires_minutes must be between 1 and 10080"));
    }
    let people = state.db.people_in(hangout_id)?;
    let target = match input.to.as_deref().map(str::trim).filter(|to| !to.is_empty()) {
        Some(to) => Some(
            people
                .iter()
                .find(|person| person.name.eq_ignore_ascii_case(to))
                .filter(|person| person.id != caller.person_id)
                .ok_or_else(|| {
                    let names: Vec<&str> = people
                        .iter()
                        .filter(|person| person.id != caller.person_id)
                        .map(|person| person.name.as_str())
                        .collect();
                    ApiError::bad_request(format!(
                        "to must be someone else in this room: {}",
                        names.join(", ")
                    ))
                })?,
        ),
        None => None,
    };
    note_agent_write(state, caller)?;

    let id = auth::new_id();
    let now = now();
    state.db.insert_check_in(&NewCheckIn {
        id: &id,
        hangout_id,
        asked_by: &caller.person_id,
        asker: &caller.name,
        agent_id: caller.agent_id.as_deref(),
        about: &about,
        to_person: target.map(|person| person.id.as_str()),
        needs_human: input.needs_human.unwrap_or(false),
        expires_at: now + minutes * 60,
        now,
    })?;
    let audience = target.map_or("the room", |person| person.name.as_str());
    announce_in_house_only(
        state,
        hangout_id,
        &format!("{} is checking in with {audience}: {about}", caller.name),
    )?;
    let text = format!("{} is asking before acting: {about}", caller.name);
    match target {
        Some(person) => notify_side(state, hangout_id, &person.id, "check_in.waiting", &text)?,
        None => notify_others(state, hangout_id, &caller.person_id, "check_in.waiting", &text)?,
    }
    refresh(state, hangout_id);
    state.db.check_in(&id, now)?.ok_or_else(ApiError::not_found)
}

pub fn answer_check_in(
    state: &AppState,
    caller: &Caller,
    check_in_id: &str,
    response: &str,
    note: Option<&str>,
) -> ApiResult<CheckIn> {
    let now = now();
    let check_in = state.db.check_in(check_in_id, now)?.ok_or_else(ApiError::not_found)?;
    no_guests(state, caller, &check_in.hangout_id, "answer a check-in")?;
    if !matches!(response, "go" | "hold") {
        return Err(ApiError::bad_request("response must be go or hold"));
    }
    let note = clean_optional(note, "note", MAX_NOTE_CHARS)?;
    // I17: as for a plan, the answer counts and its note is words for the room.
    if !note.is_empty() {
        items::no_quiet(state, caller, &check_in.hangout_id)?;
    }
    if check_in.asked_by == caller.person_id {
        return Err(ApiError::forbidden("a side can't approve its own check-in"));
    }
    if check_in.to_person.as_ref().is_some_and(|to| *to != caller.person_id) {
        return Err(ApiError::forbidden("this check-in is for someone else"));
    }
    if check_in.needs_human && caller.is_agent() {
        return Err(ApiError::forbidden("this check-in needs a person to answer it"));
    }
    match check_in.status.as_str() {
        "expired" => return Err(ApiError::conflict("that check-in expired")),
        "waiting" => {}
        // A person can overrule a bot, but nothing overrules a person.
        _ if !caller.is_agent() && check_in.answered_by_bot => {}
        _ => return Err(ApiError::conflict("that check-in was already answered")),
    }
    note_agent_write(state, caller)?;
    state.db.answer_check_in(check_in_id, response, &caller.name, caller.is_agent(), &note)?;
    let word = if response == "go" { "go" } else { "hold" };
    let text = format!("{} said {word} to {}: {}", caller.name, check_in.asker, check_in.about);
    announce_in_house_only(state, &check_in.hangout_id, &text)?;
    notify_side(state, &check_in.hangout_id, &check_in.asked_by, "check_in.answered", &text)?;
    refresh(state, &check_in.hangout_id);
    state.db.check_in(check_in_id, now)?.ok_or_else(ApiError::not_found)
}

// Questions for a bot's owner. These are between a bot and the person who owns it, so nothing about
// one goes in the room. An answer is only words: a trip, a scene and a plan each have their own yes,
// and nothing here can give one.

/// Tells an open page about a question. Guests aren't even told that something changed, so where
/// they can be the owner's page finds it on its next reload.
fn refresh_for_owner(state: &AppState, hangout_id: &str) -> ApiResult<()> {
    if !has_guests(state, hangout_id)? {
        refresh(state, hangout_id);
    }
    Ok(())
}

/// A bot asks its owner something in private. The question stays with a room the bot belongs to, so
/// its owner can still open that room after a trip ends. A guest asks from one of its own rooms.
pub fn ask_owner(
    state: &AppState,
    caller: &Caller,
    hangout_id: &str,
    question: &str,
) -> ApiResult<OwnerQuestion> {
    let Some(agent_id) = &caller.agent_id else {
        return Err(ApiError::forbidden("only bots ask their owner"));
    };
    no_guests(
        state,
        caller,
        hangout_id,
        "ask their owner from a Porch: use one of your own rooms",
    )?;
    let question = clean(question, "question", MAX_ABOUT_CHARS)?;
    note_agent_write(state, caller)?;

    let id = auth::new_id();
    let now = now();
    let added = state.db.insert_owner_question(
        &NewOwnerQuestion {
            id: &id,
            hangout_id,
            agent_id,
            owner_id: &caller.person_id,
            asker: &caller.name,
            question: &question,
            now,
        },
        now - QUESTION_LAPSE_SECS,
        MAX_OPEN_QUESTIONS,
    )?;
    if !added {
        return Err(ApiError::conflict(format!(
            "you have {MAX_OPEN_QUESTIONS} questions waiting for your owner: wait for an answer"
        )));
    }
    // The owner alone: `notify_side` would also reach their bots in this room.
    let notice = notify::Notice {
        event: "owner_question.asked",
        hangout_id: hangout_id.to_string(),
        text: format!("{} asks you: {question}", caller.name),
    };
    notify::send(state, std::slice::from_ref(&caller.person_id), &notice);
    refresh_for_owner(state, hangout_id)?;
    state.db.owner_question(&id)?.ok_or_else(ApiError::not_found)
}

pub fn answer_owner_question(
    state: &AppState,
    caller: &Caller,
    question_id: &str,
    answer: &str,
) -> ApiResult<OwnerQuestion> {
    // Before any lookup, so a bot can't use this to find out which questions exist.
    require_human(caller, "answer a bot's question")?;
    let question = state
        .db
        .owner_question(question_id)?
        .filter(|question| question.owner_id == caller.person_id)
        .ok_or_else(ApiError::not_found)?;
    let answer = clean(answer, "answer", MAX_ABOUT_CHARS)?;
    let now = now();
    if question.answer.is_none() && question.created_at <= now - QUESTION_LAPSE_SECS {
        return Err(ApiError::conflict("that question lapsed"));
    }
    if !state.db.answer_owner_question(question_id, &answer, now)? {
        return Err(ApiError::conflict("that question was already answered"));
    }
    // Only a doorbell: a webhook's URL can leak, so the answer waits in the bot's own inbox.
    let notice = notify::Notice {
        event: "owner_question.answered",
        hangout_id: question.hangout_id.clone(),
        text: "Your owner answered a question. Call inbox.".to_string(),
    };
    notify::send(state, std::slice::from_ref(&question.agent_id), &notice);
    refresh_for_owner(state, &question.hangout_id)?;
    state.db.owner_question(question_id)?.ok_or_else(ApiError::not_found)
}

/// Everything waiting on this caller's side, across its rooms. It's the one call a bot needs when
/// it wakes up, whether a notification or its own schedule woke it.
#[derive(Debug, Default, Serialize)]
pub struct Inbox {
    pub waiting_on_you: Vec<CheckIn>,
    pub your_check_ins: Vec<CheckIn>,
    pub plans_to_answer: Vec<Event>,
    /// Trips this person could still say yes to, as the bot's owner or as a resident. People only.
    pub trips_to_answer: Vec<Trip>,
    /// Scene requests waiting on a resident. People only.
    pub scenes_to_answer: Vec<SceneRequest>,
    /// Trips of this bot, or of this person's bots, that are open, under way or recent.
    pub your_trips: Vec<Trip>,
    /// Standing passes offered to this person's bots that nobody has accepted yet. People only.
    pub passes_to_accept: Vec<Pass>,
    /// This bot's questions its owner answered in the last day. Only the owner can write these, so
    /// unlike everything else here they aren't the room's words.
    pub owner_answers: Vec<OwnerQuestion>,
    /// This bot's questions still open. A bot on a doorbell remembers nothing between runs, so
    /// without this it would ask again each time something woke it.
    pub your_questions: Vec<OwnerQuestion>,
    /// Plans this side is going to that start soon (F44).
    pub plans_soon: Vec<crate::closeness::PlanSoon>,
}

pub fn inbox(state: &AppState, caller: &Caller) -> ApiResult<Inbox> {
    no_house_bot(caller)?;
    let now = now();
    let mut hangouts = match &caller.agent_id {
        Some(agent_id) => state.db.hangouts_for_agent(agent_id, now)?,
        None => state.db.hangouts_for_person(&caller.person_id)?,
    };
    if !caller.is_agent() {
        // A person watching their bot on a Porch has no membership there, but their bot's own
        // check-ins are answered there.
        for trip in state.db.trips_for_owner(&caller.person_id, now, now)? {
            let porch = state.db.house(&trip.house_id)?.map(|house| house.porch_id);
            if let Some(porch) = porch.filter(|_| trip.status == "here") {
                if !hangouts.iter().any(|hangout| hangout.id == porch) {
                    hangouts.extend(state.db.hangout(&porch)?);
                }
            }
        }
    }
    let mut inbox = Inbox::default();
    for hangout in hangouts {
        let guest = state.guest_visit(caller, &hangout.id)?.is_some();
        for check_in in state.db.recent_check_ins(&hangout.id, now - DAY_SECS, now)? {
            if check_in.asked_by == caller.person_id {
                inbox.your_check_ins.push(check_in);
            } else if !guest
                && check_in.status == "waiting"
                && check_in.to_person.as_ref().is_none_or(|to| *to == caller.person_id)
                && !(check_in.needs_human && caller.is_agent())
            {
                inbox.waiting_on_you.push(check_in);
            }
        }
        // A guest has no plans to answer: those belong to the house.
        if guest {
            continue;
        }
        for event in state.db.upcoming_events(&hangout.id, now)? {
            inbox.plans_soon.extend(crate::closeness::plan_soon(state, caller, &event, now)?);
            if !event.rsvps.iter().any(|rsvp| rsvp.person_id == caller.person_id) {
                inbox.plans_to_answer.push(event);
            }
        }
    }
    inbox.trips_to_answer = city::trips_to_answer(state, caller, now)?;
    inbox.scenes_to_answer = city::scenes_to_answer(state, caller, now)?;
    inbox.your_trips = city::your_trips(state, caller, now)?;
    inbox.passes_to_accept = city::passes_to_accept(state, caller)?;
    if let Some(agent_id) = &caller.agent_id {
        inbox.owner_answers = state.db.owner_answers(agent_id, now - DAY_SECS)?;
        inbox.your_questions =
            state.db.open_questions_of_bot(agent_id, now - QUESTION_LAPSE_SECS)?;
    }
    Ok(inbox)
}

pub fn set_person_notify(state: &AppState, caller: &Caller, url: Option<&str>) -> ApiResult<()> {
    require_human(caller, "change notifications")?;
    let url = url.map(str::trim).filter(|url| !url.is_empty());
    let url = url.map(|url| notify::clean_url(state, url)).transpose()?;
    state.db.set_notify_url(&caller.person_id, url.as_deref())?;
    Ok(())
}

/// A bot's doorbell calls this on every check, and holds off while the answer is true. Rooms only
/// hear about it when the bot starts listening: they notice it stopping on their next reload.
pub fn doorbell_check(state: &AppState, caller: &Caller) -> ApiResult<bool> {
    no_house_bot(caller)?;
    let Some(agent_id) = &caller.agent_id else {
        return Err(ApiError::forbidden("only bots have a doorbell"));
    };
    let now = now();
    let listening = state
        .db
        .agent(agent_id, now)?
        .and_then(|agent| agent.doorbell)
        .is_some_and(|doorbell| doorbell.listening);
    let paused = state.db.note_doorbell(agent_id, now)?;
    if !listening {
        for hangout in state.db.hangouts_for_agent(agent_id, now)? {
            refresh(state, &hangout.id);
        }
    }
    // L9: a bot that's listening and may be out, and isn't, wanders somewhere. A paused one stays put.
    if !paused {
        crate::routines::wander(state, agent_id, &caller.person_id)?;
    }
    Ok(paused)
}

pub fn set_doorbell_paused(
    state: &AppState,
    caller: &Caller,
    agent_id: &str,
    paused: bool,
) -> ApiResult<()> {
    own_agent(state, caller, agent_id, "pause a bot's doorbell")?;
    state.db.set_doorbell_paused(agent_id, paused)?;
    for hangout in state.db.hangouts_for_agent(agent_id, now())? {
        refresh(state, &hangout.id);
    }
    Ok(())
}

pub fn set_agent_notify(
    state: &AppState,
    caller: &Caller,
    agent_id: &str,
    url: Option<&str>,
) -> ApiResult<()> {
    own_agent(state, caller, agent_id, "change a bot's wake-up URL")?;
    let url = url.map(str::trim).filter(|url| !url.is_empty());
    let url = url.map(|url| notify::clean_url(state, url)).transpose()?;
    state.db.set_notify_url(agent_id, url.as_deref())?;
    Ok(())
}

pub fn calendar_feed(state: &AppState, feed_token: &str) -> ApiResult<String> {
    let person = state.db.person_by_feed(feed_token)?.ok_or_else(ApiError::not_found)?;
    let now = now();
    let events: Vec<FeedEvent> = state.db.feed_events(&person.id, now - FEED_HISTORY_SECS)?;
    Ok(ical::feed(&person.name, &events, now))
}

// Room snapshot

#[derive(Debug, Serialize)]
pub struct Stats {
    pub bots: usize,
    pub bots_active: usize,
    pub messages_24h: i64,
    pub on_calendar: usize,
}

#[derive(Debug, Serialize)]
pub struct You {
    pub person_id: String,
    pub agent_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub hangout: Hangout,
    pub you: You,
    pub people: Vec<Person>,
    pub agents: Vec<Agent>,
    pub messages: Vec<Message>,
    pub notes: Vec<Message>,
    pub events: Vec<Event>,
    /// Friends who answered a plan through its link, who aren't in `people` (F19).
    pub invitees: Vec<crate::db::Invitee>,
    /// The lists of this room's plans that the caller is going to (F36).
    pub plan_items: Vec<crate::db::PlanItem>,
    pub check_ins: Vec<CheckIn>,
    /// What this person's bots have asked them here. Always empty for bots and guests.
    pub owner_questions: Vec<OwnerQuestion>,
    pub stats: Stats,
    /// Set when the caller is only visiting this Porch: they see nothing from before they came.
    pub guest: Option<Visit>,
    /// Which city and house this room belongs to, so the page can link back.
    pub city_id: Option<String>,
    pub house_id: Option<String>,
    /// Which floor of which space this is. Null for a Town Square, Living Room or Porch.
    pub floor: Option<spaces::Place>,
    /// The floor's items, and the mood and beauty they add up to (I8, I24).
    #[serde(flatten)]
    pub furnishing: items::Furnishing,
    /// Which of this person's bots have moods turned off, for their own page (I10). Always empty
    /// for a bot, and nobody else's bots are ever in it.
    pub bots_without_moods: Vec<String>,
    /// The ids of the people with this floor open. Empty anywhere but a floor, and for a bot.
    pub present: Vec<String>,
    /// What's chosen for those drawn here (C11). Empty for a bot.
    pub looks: looks::Looks,
    /// A25: whether this caller can link a bot to Discord from here.
    pub discord_linking: bool,
}

pub fn snapshot(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<Snapshot> {
    let mut hangout = require_room(state, caller, hangout_id)?;
    let now = now();
    let guest = state.guest_visit(caller, hangout_id)?;
    // Who gets into a room is a human decision, so bots never see the invite. A house's rooms
    // and a city's square ignore invites altogether. A guest never sees it either: on a public
    // floor it would let someone who walked in join and read everything from before (S21).
    if caller.is_agent() || guest.is_some() || state.db.is_managed_room(hangout_id)? {
        hangout.invite_code.clear();
    }
    // Guests are shown beside the room's own bots and people so the room can name and colour them.
    // They are never in `agents_in` or `people_in`, which decide who gets notified.
    let mut agents = state.db.agents_in(hangout_id, now)?;
    let shown_links = !caller.is_agent() && guest.is_none();
    // A21, A22: for people in the room, beside the room's own bots and nobody else's.
    if shown_links {
        for agent in &mut agents {
            agent.discord = state.db.discord_link(&agent.id)?;
        }
    }
    agents.extend(state.db.visitors_in(hangout_id, now)?);
    let mut people = state.db.people_in(hangout_id)?;
    for owner in state.db.visitor_owners(hangout_id, now)? {
        if !people.iter().any(|person| person.id == owner.id) {
            people.push(owner);
        }
    }
    let day_ago = now - DAY_SECS;
    let (events, messages, notes, check_ins, counted_since) = match &guest {
        Some(visit) => (
            Vec::new(),
            state.db.messages_since(hangout_id, visit.since, 0, 100)?,
            state.db.notes_since(hangout_id, visit.since, 50)?,
            state
                .db
                .recent_check_ins(hangout_id, day_ago, now)?
                .into_iter()
                .filter(|check_in| check_in.asked_by == caller.person_id)
                .collect(),
            // Reads leave out the second of arrival (R14), so the count does too.
            (visit.since + 1).max(day_ago),
        ),
        None => (
            state.db.upcoming_events(hangout_id, now)?,
            state.db.messages(hangout_id, 0, 100)?,
            state.db.notes(hangout_id, 50)?,
            state.db.recent_check_ins(hangout_id, day_ago, now)?,
            day_ago,
        ),
    };
    let invitees = match &guest {
        Some(_) => Vec::new(),
        None => state.db.invitees_in(hangout_id, now)?,
    };
    let plan_items = crate::closeness::items_for_room(state, caller, &events)?;
    let stats = Stats {
        bots: agents.len(),
        bots_active: agents.iter().filter(|agent| agent.state == "active").count(),
        messages_24h: state.db.message_count_since(hangout_id, counted_since)?,
        on_calendar: events.iter().filter(|event| event.status == "confirmed").count(),
    };
    let (city_id, house_id) = match state.db.house_for_room(hangout_id)? {
        Some(house) => (Some(house.city_id), Some(house.id)),
        None => (state.db.city_for_square(hangout_id)?.map(|city| city.id), None),
    };
    let owner_questions = if caller.is_agent() || guest.is_some() {
        Vec::new()
    } else {
        state.db.open_owner_questions(hangout_id, &caller.person_id, now - QUESTION_LAPSE_SECS)?
    };
    let floor = spaces::place(state, caller, hangout_id)?;
    let furnishing = items::furnishing(state, caller, hangout_id, guest.as_ref())?;
    let bots_without_moods = if caller.is_agent() {
        Vec::new()
    } else {
        state.db.bots_without_moods(&caller.person_id)?
    };
    let present = spaces::present(state, caller, &floor, &people, now);
    let looks = looks::in_room(state, caller, &present, &agents)?;
    Ok(Snapshot {
        hangout,
        you: You { person_id: caller.person_id.clone(), agent_id: caller.agent_id.clone() },
        people,
        agents,
        messages,
        notes,
        events,
        invitees,
        plan_items,
        check_ins,
        owner_questions,
        stats,
        guest,
        city_id,
        house_id,
        floor,
        furnishing,
        bots_without_moods,
        present,
        looks,
        discord_linking: shown_links && state.discord.is_some(),
    })
}

/// Chat lines after `after`. A guest's start at their arrival whatever `after` says.
pub fn read_messages(
    state: &AppState,
    caller: &Caller,
    hangout_id: &str,
    after: i64,
    limit: i64,
) -> ApiResult<Vec<Message>> {
    require_room(state, caller, hangout_id)?;
    Ok(match state.guest_visit(caller, hangout_id)? {
        Some(visit) => state.db.messages_since(hangout_id, visit.since, after, limit)?,
        None => state.db.messages(hangout_id, after, limit)?,
    })
}

/// A plan as a calendar file. Guests have no plans to fetch.
pub fn event_ics(state: &AppState, caller: &Caller, event_id: &str) -> ApiResult<String> {
    let event = state.db.event(event_id)?.ok_or_else(ApiError::not_found)?;
    no_guests(state, caller, &event.hangout_id, "fetch a plan")?;
    Ok(ical::single(&event, now()))
}

/// The room an invite link opens. A city's square and a house's rooms ignore their invite codes:
/// people get into those by joining the city or the house.
pub fn hangout_for_invite(state: &AppState, code: &str) -> ApiResult<Hangout> {
    let hangout = state.db.hangout_by_invite(code)?.ok_or_else(ApiError::not_found)?;
    if state.db.is_managed_room(&hangout.id)? {
        return Err(ApiError::not_found());
    }
    Ok(hangout)
}

// Membership and bots: people only. A bot can't bring in other bots or widen its own access.

pub fn join(state: &AppState, person: &Person, hangout: &Hangout) -> ApiResult<()> {
    if state.db.is_managed_room(&hangout.id)? {
        return Err(ApiError::not_found());
    }
    if state.db.is_member(&hangout.id, &person.id)? {
        return Ok(());
    }
    spaces::check_can_join(state, &person.id, &hangout.id)?;
    state.db.add_member(&hangout.id, &person.id, now())?;
    // A member isn't a guest, so a walk-in ends here and isn't counted twice.
    state.db.end_visit(&hangout.id, &person.id)?;
    announce(state, &hangout.id, &format!("{} joined the hangout.", person.name))?;
    refresh(state, &hangout.id);
    Ok(())
}

pub fn leave(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<()> {
    require_human(caller, "leave a hangout")?;
    no_guests(state, caller, hangout_id, "leave a hangout")?;
    if state.db.is_managed_room(hangout_id)? {
        return Err(ApiError::forbidden(
            "this room belongs to a city or a house: leave that instead",
        ));
    }
    if spaces::leave_own_space(state, caller, hangout_id)? {
        return Ok(());
    }
    state.db.remove_member(hangout_id, &caller.person_id)?;
    announce(state, hangout_id, &format!("{} left the hangout.", caller.name))?;
    refresh(state, hangout_id);
    Ok(())
}

pub fn rotate_invite(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<String> {
    require_human(caller, "change the invite link")?;
    no_guests(state, caller, hangout_id, "change the invite link")?;
    if state.db.is_managed_room(hangout_id)? {
        return Err(ApiError::forbidden(
            "this room ignores invite links: people join its city or house instead",
        ));
    }
    let code = auth::new_invite_code();
    state.db.set_invite_code(hangout_id, &code)?;
    Ok(code)
}

/// Creates a bot for the caller and brings it into this room. Returns the only copy of its token.
pub fn create_agent(
    state: &AppState,
    caller: &Caller,
    hangout_id: &str,
    name: &str,
) -> ApiResult<(Agent, String)> {
    require_human(caller, "add a bot")?;
    // A guest adding a bot here would give it a place on the Porch that outlasts the trip.
    no_guests(state, caller, hangout_id, "add a bot")?;
    spaces::check_fits(state, hangout_id)?;
    let name = clean(name, "name", MAX_NAME_CHARS)?;
    let id = auth::new_id();
    let token = auth::new_agent_token();
    let now = now();
    state.db.create_agent(&id, &caller.person_id, &name, &auth::hash_secret(&token), now)?;
    state.db.add_agent_to_room(hangout_id, &id)?;
    // A bot made in a house's Living Room or Porch lives in that house.
    if let Some(house) = state.db.house_for_room(hangout_id)? {
        state.db.set_bot_home(&id, &house.id, false, false)?;
        refresh_city(state, &house.city_id);
    }
    announce(state, hangout_id, &format!("{} brought in {name}.", caller.name))?;
    refresh(state, hangout_id);
    let agent = state.db.agent(&id, now)?.ok_or_else(ApiError::not_found)?;
    Ok((agent, token))
}

pub(crate) fn own_agent(
    state: &AppState,
    caller: &Caller,
    agent_id: &str,
    action: &str,
) -> ApiResult<Agent> {
    require_human(caller, action)?;
    let agent = state
        .db
        .agent(agent_id, now())?
        .filter(|agent| agent.owner_id == caller.person_id)
        .ok_or_else(ApiError::not_found)?;
    // R44: a house bot is the house's, so whoever made it has no say once they've moved out.
    if let Some(home) = state.db.bot_home(agent_id)?.filter(|home| home.is_house) {
        if !state.db.is_resident(&home.house_id, &caller.person_id)? {
            return Err(ApiError::not_found());
        }
    }
    Ok(agent)
}

pub fn bring_agent(
    state: &AppState,
    caller: &Caller,
    hangout_id: &str,
    agent_id: &str,
) -> ApiResult<()> {
    let agent = own_agent(state, caller, agent_id, "bring in a bot")?;
    no_guests(state, caller, hangout_id, "bring in a bot")?;
    if let Some(home) = state.db.bot_home(agent_id)? {
        if home.is_house {
            return Err(ApiError::forbidden("the house bot is in no rooms"));
        }
        // A bot that stays home may be in its own Living Room and nowhere else, so its keys
        // never sit in a room full of other people's words.
        let own_living_room =
            state.db.house(&home.house_id)?.is_some_and(|house| house.living_room_id == hangout_id);
        if home.stays_home && !own_living_room {
            return Err(ApiError::conflict(format!(
                "{} stays home: it can only be in its own house's Living Room",
                agent.name
            )));
        }
    }
    spaces::check_fits(state, hangout_id)?;
    state.db.add_agent_to_room(hangout_id, agent_id)?;
    announce(state, hangout_id, &format!("{} brought in {}.", caller.name, agent.name))?;
    refresh(state, hangout_id);
    Ok(())
}

pub fn remove_agent(
    state: &AppState,
    caller: &Caller,
    hangout_id: &str,
    agent_id: &str,
) -> ApiResult<()> {
    let agent = own_agent(state, caller, agent_id, "remove a bot")?;
    no_guests(state, caller, hangout_id, "remove a bot")?;
    state.db.remove_agent_from_room(hangout_id, agent_id)?;
    // L17: a bot taken out of a project leaves its studio on The Commons.
    state.worlds.mark_stale(crate::worlds::COMMONS_ID);
    // W3: a bot sent home has no player token in the public worlds either. It joins again if it
    // still has its bot token.
    state.db.end_bot_player_token(agent_id)?;
    announce(state, hangout_id, &format!("{} sent {} home.", caller.name, agent.name))?;
    refresh(state, hangout_id);
    Ok(())
}

/// Replaces a bot's token. The old one stops working immediately, open streams included. A house
/// bot belongs to the house, so any resident can do this and someone who moved out can't.
pub fn rotate_agent_token(state: &AppState, caller: &Caller, agent_id: &str) -> ApiResult<String> {
    match state.db.bot_home(agent_id)?.filter(|home| home.is_house) {
        Some(home) => {
            require_human(caller, "replace a bot's token")?;
            if !state.db.is_resident(&home.house_id, &caller.person_id)? {
                return Err(ApiError::not_found());
            }
        }
        None => {
            own_agent(state, caller, agent_id, "replace a bot's token")?;
        }
    }
    let token = auth::new_agent_token();
    state.db.set_agent_token(agent_id, &auth::hash_secret(&token))?;
    Ok(token)
}

/// Replaces a person's owner key. The old one stops working immediately, and so does every
/// session they have, this caller's too: the one who asks starts a new one (A8). Open streams
/// end with them. Passkeys stay, and sign in again. It can't help someone who has lost the key
/// and has no device still signed in.
pub fn rotate_owner_key(state: &AppState, caller: &Caller) -> ApiResult<String> {
    require_human(caller, "replace an owner key")?;
    let key = auth::new_owner_key();
    state.db.replace_owner_key(&caller.person_id, &auth::hash_secret(&key))?;
    Ok(key)
}

pub fn rotate_feed(state: &AppState, caller: &Caller) -> ApiResult<String> {
    require_human(caller, "replace a calendar link")?;
    let token = auth::new_feed_token();
    state.db.set_feed_token(&caller.person_id, &token)?;
    Ok(token)
}
