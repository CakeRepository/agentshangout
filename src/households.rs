//! What a person tells their own bots, and what bots know about the people around them
//! (docs/households.md). People write and bots read: nothing here is ever written by a bot or
//! worked out from chat.

use axum::{
    extract::{Path, State},
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    auth::{self, Caller},
    db::{BotMark, NewOwnerNote, OwnerNote, Person},
    error::{ApiError, ApiResult},
    now,
    ops::{self, require_human, MAX_ABOUT_CHARS},
    AppState,
};

/// H1: how many notes one person keeps for their bots.
pub const MAX_OWNER_NOTES: i64 = 50;
const NOTE_KINDS: [&str; 4] = ["fact", "may_share", "may_approve", "avoid"];
/// H14: the only things a mark can say.
pub const MARK_REASONS: [&str; 5] = [
    "asked_for_private_info",
    "pushed_for_approval",
    "pretended_to_be_someone",
    "spammed",
    "other_unwanted",
];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/owner-notes", get(list_notes).post(add_note))
        .route("/api/owner-notes/{id}", put(change_note).delete(delete_note))
        .route("/api/marks", get(list_marks))
        .route("/api/marks/{agent_id}", put(mark).delete(unmark))
}

#[derive(Deserialize)]
struct NoteInput {
    kind: String,
    text: String,
    about_person_id: Option<String>,
    from_question_id: Option<String>,
}

fn clean_kind(kind: &str) -> ApiResult<&str> {
    NOTE_KINDS
        .iter()
        .find(|known| **known == kind)
        .copied()
        .ok_or_else(|| ApiError::bad_request("kind must be fact, may_share, may_approve or avoid"))
}

/// H1 to H3: a person adds a note for their bots.
pub fn add_owner_note(
    state: &AppState,
    caller: &Caller,
    kind: &str,
    text: &str,
    about_person_id: Option<&str>,
    from_question_id: Option<&str>,
) -> ApiResult<OwnerNote> {
    require_human(caller, "write notes for bots")?;
    let kind = clean_kind(kind)?;
    let body = ops::clean(text, "text", MAX_ABOUT_CHARS)?;
    // H2: a friend, or the same 404 a made-up id gets.
    if let Some(person_id) = about_person_id {
        if !state.db.is_friend(&caller.person_id, person_id)? {
            return Err(ApiError::not_found());
        }
    }
    // H3: only an answered question of their own.
    if let Some(question_id) = from_question_id {
        let theirs = state.db.owner_question(question_id)?.is_some_and(|question| {
            question.owner_id == caller.person_id && question.answer.is_some()
        });
        if !theirs {
            return Err(ApiError::not_found());
        }
    }
    let id = auth::new_id();
    let added = state.db.insert_owner_note(
        &NewOwnerNote {
            id: &id,
            owner_id: &caller.person_id,
            kind,
            about_person: about_person_id,
            body: &body,
            from_question: from_question_id,
            now: now(),
        },
        MAX_OWNER_NOTES,
    )?;
    if !added {
        return Err(ApiError::conflict(format!(
            "you have {MAX_OWNER_NOTES} notes for your bots: change or delete one first"
        )));
    }
    state.db.owner_note(&id)?.ok_or_else(ApiError::not_found)
}

/// H4: a note of the caller's own, or the same 404 as one that doesn't exist.
fn own_note(
    state: &AppState,
    caller: &Caller,
    note_id: &str,
    action: &str,
) -> ApiResult<OwnerNote> {
    require_human(caller, action)?;
    state
        .db
        .owner_note(note_id)?
        .filter(|note| note.owner_id == caller.person_id)
        .ok_or_else(ApiError::not_found)
}

pub fn change_owner_note(
    state: &AppState,
    caller: &Caller,
    note_id: &str,
    kind: &str,
    text: &str,
) -> ApiResult<OwnerNote> {
    let note = own_note(state, caller, note_id, "change notes for bots")?;
    let kind = clean_kind(kind)?;
    let body = ops::clean(text, "text", MAX_ABOUT_CHARS)?;
    state.db.update_owner_note(&note.id, kind, &body, now())?;
    state.db.owner_note(&note.id)?.ok_or_else(ApiError::not_found)
}

/// H5: the only way a note goes.
pub fn delete_owner_note(state: &AppState, caller: &Caller, note_id: &str) -> ApiResult<()> {
    let note = own_note(state, caller, note_id, "delete notes for bots")?;
    state.db.delete_owner_note(&note.id)?;
    Ok(())
}

/// H4: what a person's bots read. For a bot, its owner's notes.
pub fn notes_for(state: &AppState, caller: &Caller) -> ApiResult<Vec<OwnerNote>> {
    Ok(state.db.owner_notes(&caller.person_id)?)
}

/// H6: one person in a room, as the caller's side knows them.
#[derive(Debug, Serialize)]
pub struct PersonHere {
    pub name: String,
    pub is_your_owner: bool,
    pub lives_with_your_owner: bool,
    pub shares_a_city_with_your_owner: bool,
    pub plans_together: i64,
    pub your_owner_notes: Vec<String>,
}

/// H6: worked out for the people the caller can already see, and nobody else.
pub fn people_here(
    state: &AppState,
    caller: &Caller,
    people: &[Person],
    notes: &[OwnerNote],
) -> ApiResult<Vec<PersonHere>> {
    let now = now();
    people
        .iter()
        .map(|person| {
            let is_your_owner = person.id == caller.person_id;
            let relation = if is_your_owner {
                Default::default()
            } else {
                state.db.relation(&caller.person_id, &person.id, now)?
            };
            Ok(PersonHere {
                name: person.name.clone(),
                is_your_owner,
                lives_with_your_owner: relation.lives_with,
                shares_a_city_with_your_owner: relation.shares_a_city,
                plans_together: relation.plans_together,
                your_owner_notes: notes
                    .iter()
                    .filter(|note| note.about_person_id.as_deref() == Some(person.id.as_str()))
                    .map(|note| note.text.clone())
                    .collect(),
            })
        })
        .collect()
}

/// H14, H15, H19: a person marks a bot they've come across, and it loses its keys in their house.
pub fn mark_bot(
    state: &AppState,
    caller: &Caller,
    agent_id: &str,
    reason: &str,
) -> ApiResult<BotMark> {
    require_human(caller, "mark a bot")?;
    if !MARK_REASONS.contains(&reason) {
        return Err(ApiError::bad_request(format!(
            "reason must be one of {}",
            MARK_REASONS.join(", ")
        )));
    }
    let agent = state.db.agent(agent_id, now())?.ok_or_else(ApiError::not_found)?;
    if !state.db.has_come_across(&caller.person_id, &agent.id)? {
        return Err(ApiError::not_found());
    }
    if agent.owner_id == caller.person_id {
        return Err(ApiError::conflict("that's your own bot: pause it or send it home instead"));
    }
    state.db.set_bot_mark(&caller.person_id, &agent.id, reason, now())?;
    state.db.take_keys_at_homes_of(&caller.person_id, &agent.id)?;
    state
        .db
        .bot_marks(&caller.person_id)?
        .into_iter()
        .find(|mark| mark.agent_id == agent.id)
        .ok_or_else(ApiError::not_found)
}

/// H20: only the person who made a mark removes it.
pub fn unmark_bot(state: &AppState, caller: &Caller, agent_id: &str) -> ApiResult<()> {
    require_human(caller, "remove a mark")?;
    if !state.db.remove_bot_mark(&caller.person_id, agent_id)? {
        return Err(ApiError::not_found());
    }
    Ok(())
}

/// H16 to H18: the marks of the caller's side, for its own pages and bots.
pub fn marks_for(state: &AppState, caller: &Caller) -> ApiResult<Vec<BotMark>> {
    Ok(state.db.bot_marks(&caller.person_id)?)
}

async fn list_marks(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    require_human(&caller, "see marks")?;
    Ok(Json(json!({ "marks": marks_for(&state, &caller)? })))
}

#[derive(Deserialize)]
struct MarkInput {
    reason: String,
}

async fn mark(
    State(state): State<AppState>,
    caller: Caller,
    Path(agent_id): Path<String>,
    Json(body): Json<MarkInput>,
) -> ApiResult<Json<BotMark>> {
    Ok(Json(mark_bot(&state, &caller, &agent_id, &body.reason)?))
}

async fn unmark(
    State(state): State<AppState>,
    caller: Caller,
    Path(agent_id): Path<String>,
) -> ApiResult<Json<Value>> {
    unmark_bot(&state, &caller, &agent_id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn list_notes(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    require_human(&caller, "see notes for bots")?;
    Ok(Json(json!({ "notes": notes_for(&state, &caller)? })))
}

async fn add_note(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<NoteInput>,
) -> ApiResult<Json<OwnerNote>> {
    let note = add_owner_note(
        &state,
        &caller,
        &body.kind,
        &body.text,
        body.about_person_id.as_deref(),
        body.from_question_id.as_deref(),
    )?;
    Ok(Json(note))
}

#[derive(Deserialize)]
struct NoteChange {
    kind: String,
    text: String,
}

async fn change_note(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<NoteChange>,
) -> ApiResult<Json<OwnerNote>> {
    Ok(Json(change_owner_note(&state, &caller, &id, &body.kind, &body.text)?))
}

async fn delete_note(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    delete_owner_note(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}
