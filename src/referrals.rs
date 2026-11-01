//! Friends bringing friends: a plan's link. The rules are F6 to F23 in docs/referrals.md.

use axum::http::HeaderMap;
use serde::Serialize;

use crate::{
    auth::{self, Caller},
    db::{Event, Invited, Person},
    error::{ApiError, ApiResult},
    ical, now,
    ops::{self, MAX_NAME_CHARS},
    worlds::{self, Limiter},
    AppState,
};

const HOUR_SECS: i64 = 3_600;
/// F18: past this a plan's link stops taking new people.
const MAX_INVITEES: i64 = 30;
const NEW_PEOPLE_PER_ADDRESS_PER_HOUR: u32 = 10;
const NEW_PEOPLE_PER_HOUR: u32 = 120;

/// The limits on plan links. Counted in memory, so a restart starts them again.
pub struct Limits {
    new_people_per_address: Limiter,
    new_people: Limiter,
}

impl Limits {
    pub fn new() -> Limits {
        Limits {
            new_people_per_address: Limiter::new(4096, HOUR_SECS, NEW_PEOPLE_PER_ADDRESS_PER_HOUR),
            new_people: Limiter::new(1, HOUR_SECS, NEW_PEOPLE_PER_HOUR),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}

// A plan's link

fn plan_url(state: &AppState, code: &str) -> String {
    format!("{}/p/{code}", state.config.public_url)
}

fn coming_up(event: &Event, now: i64) -> bool {
    event.status != "cancelled" && event.ends_at >= now
}

/// F8: a plan someone in its room may hand out a link to.
fn plan_to_share(state: &AppState, caller: &Caller, event_id: &str) -> ApiResult<Event> {
    ops::require_human(caller, "invite a friend to a plan")?;
    let event = state.db.event(event_id)?.ok_or_else(ApiError::not_found)?;
    ops::no_guests(state, caller, &event.hangout_id, "invite a friend to a plan")?;
    if !coming_up(&event, now()) {
        return Err(ApiError::conflict("that plan is over or was cancelled"));
    }
    Ok(event)
}

pub fn plan_link(state: &AppState, caller: &Caller, event_id: &str) -> ApiResult<String> {
    let event = plan_to_share(state, caller, event_id)?;
    let code = state.db.plan_invite_code(
        &event.id,
        &auth::new_invite_code(),
        &caller.person_id,
        now(),
    )?;
    Ok(plan_url(state, &code))
}

/// F9: the old link stops working at once.
pub fn replace_plan_link(state: &AppState, caller: &Caller, event_id: &str) -> ApiResult<String> {
    let event = plan_to_share(state, caller, event_id)?;
    let code = auth::new_invite_code();
    state.db.replace_plan_invite(&event.id, &code, &caller.person_id, now())?;
    Ok(plan_url(state, &code))
}

/// A link that still works, and the plan it opens.
pub struct OpenInvite {
    event: Event,
    invited_by: String,
}

/// F10: a wrong code, and a plan that's over or cancelled, all look the same.
pub fn open_invite(state: &AppState, code: &str) -> ApiResult<OpenInvite> {
    let (event_id, created_by) = state.db.plan_invite(code)?.ok_or_else(ApiError::not_found)?;
    let event = state.db.event(&event_id)?.ok_or_else(ApiError::not_found)?;
    if !coming_up(&event, now()) {
        return Err(ApiError::not_found());
    }
    let sender = state.db.person(&created_by)?.ok_or_else(ApiError::not_found)?;
    Ok(OpenInvite { event, invited_by: sender.name })
}

/// F11: all that a link shows. Anything added here is shown to whoever holds the link.
#[derive(Debug, Serialize)]
pub struct PlanPreview {
    title: String,
    location: String,
    starts_at: i64,
    ends_at: i64,
    on_calendar: bool,
    invited_by: String,
    yes: i64,
    yes_needed: i64,
    your_answer: Option<String>,
}

fn preview_of(
    state: &AppState,
    invite: OpenInvite,
    person_id: Option<&str>,
) -> ApiResult<PlanPreview> {
    let event = invite.event;
    let your_answer = match person_id {
        Some(person_id) => state.db.rsvp(&event.id, person_id)?.map(|rsvp| rsvp.response),
        None => None,
    };
    Ok(PlanPreview {
        yes: event.rsvps.iter().filter(|rsvp| rsvp.response == "yes").count() as i64,
        title: event.title,
        location: event.location,
        starts_at: event.starts_at,
        ends_at: event.ends_at,
        on_calendar: event.status == "confirmed",
        invited_by: invite.invited_by,
        yes_needed: event.min_yes,
        your_answer,
    })
}

pub fn preview(state: &AppState, caller: Option<&Caller>, code: &str) -> ApiResult<PlanPreview> {
    let person_id = caller.filter(|caller| !caller.is_agent()).map(|caller| caller.person_id.as_str());
    preview_of(state, open_invite(state, code)?, person_id)
}

/// Everything that can refuse an answer, checked before anyone is made (F12).
pub fn check_answer(
    state: &AppState,
    caller: Option<&Caller>,
    headers: &HeaderMap,
    invite: &OpenInvite,
    response: &str,
    your_name: Option<&str>,
) -> ApiResult<()> {
    if caller.is_some_and(Caller::is_agent) {
        return Err(ApiError::forbidden("only a person can answer through a plan's link"));
    }
    if !matches!(response, "yes" | "maybe" | "no") {
        return Err(ApiError::bad_request("response must be yes, maybe or no"));
    }
    let person_id = caller.map_or("", |caller| caller.person_id.as_str());
    if !state.db.is_member(&invite.event.hangout_id, person_id)?
        && state.db.plan_is_full(&invite.event.id, person_id, MAX_INVITEES)?
    {
        return Err(ApiError::conflict("that plan has all the guests it can take"));
    }
    if caller.is_none() {
        ops::clean(your_name.unwrap_or(""), "your name", MAX_NAME_CHARS)?;
        allow_new_person(state, headers)?;
    }
    Ok(())
}

fn allow_new_person(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    let now = now();
    let limits = &state.referrals;
    if let Some(ip) = worlds::address_of(state, headers) {
        limits.new_people_per_address.take(
            &ip.to_string(),
            now,
            "too many new people from your address: try again in an hour",
        )?;
    }
    limits.new_people.take("all", now, "a lot of people have joined this hour: try again soon")
}

/// F13 to F16, F19. Someone in the room answers as themselves, and anyone else becomes an invitee
/// of this plan and nothing more. Returns what the link now shows them.
pub fn answer(
    state: &AppState,
    person: &Person,
    invite: OpenInvite,
    response: &str,
) -> ApiResult<PlanPreview> {
    let event = &invite.event;
    let invited = if state.db.is_member(&event.hangout_id, &person.id)? {
        Invited::Already
    } else {
        state.db.add_plan_invitee(&event.id, &person.id, MAX_INVITEES, now())?
    };
    if matches!(invited, Invited::Full) {
        return Err(ApiError::conflict("that plan has all the guests it can take"));
    }
    let note = state.db.rsvp(&event.id, &person.id)?.map(|rsvp| rsvp.note).unwrap_or_default();
    state.db.upsert_rsvp(&event.id, &person.id, response, None, &note, now())?;
    if matches!(invited, Invited::New) {
        ops::announce_in_house_only(
            state,
            &event.hangout_id,
            &format!("{} said {response} to \"{}\" through its link.", person.name, event.title),
        )?;
    }
    ops::settle(state, &event.id)?;
    ops::refresh(state, &event.hangout_id);
    let event = state.db.event(&event.id)?.ok_or_else(ApiError::not_found)?;
    preview_of(state, OpenInvite { event, ..invite }, Some(&person.id))
}

/// F20: the plan as a calendar file, with nothing a link doesn't show.
pub fn plan_ics(state: &AppState, code: &str) -> ApiResult<String> {
    let mut event = open_invite(state, code)?.event;
    event.notes.clear();
    Ok(ical::single(&event, now()))
}
