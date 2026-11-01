//! Boards on a floor (docs/boards.md). A discussion, a job list or an issues list the room keeps.
//! Whoever runs the space puts one up. An issues board can name a GitHub repository. The server
//! stores that name and never calls GitHub: the bots in the room use whatever access they already
//! have.

use axum::{
    extract::{Path, State},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    auth::{self, Caller},
    db::{Board, BoardCard, BoardSummary, Visit},
    error::{ApiError, ApiResult},
    households,
    items::{self, from_your_owner},
    mcp::TRUST_NOTE,
    now,
    ops::{self, iso, MAX_NAME_CHARS},
    spaces, AppState,
};

/// K2.
pub const MAX_BOARDS: i64 = 8;
/// K5.
pub const MAX_CARDS: i64 = 100;
const MAX_TITLE: usize = 120;
const MAX_TEXT: usize = 2000;
const MAX_ISSUE: i64 = 1_000_000_000;

const KINDS: [&str; 3] = ["discussion", "jobs", "issues"];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/hangouts/{id}/boards", get(list_boards).post(create_board))
        .route("/api/boards/{id}", put(change_board).delete(delete_board))
        .route("/api/boards/{id}/cards", post(create_card))
        .route("/api/cards/{id}", put(change_card).delete(delete_card))
}

// Who may

/// The floor, when this room is one. A town square, living room or porch is not (K1).
fn floor_room(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<()> {
    ops::require_room(state, caller, hangout_id)?;
    if state.db.floor(hangout_id)?.is_none() {
        return Err(ApiError::conflict(
            "boards go on a floor of a place, not a town square, living room or porch",
        ));
    }
    Ok(())
}

fn runner(state: &AppState, caller: &Caller, hangout_id: &str, action: &str) -> ApiResult<()> {
    // Not in the room is the same as no room (K1). A bot is told no only once that's settled.
    floor_room(state, caller, hangout_id)?;
    ops::no_house_bot(caller)?;
    ops::require_human(caller, action)?;
    let floor = state.db.floor(hangout_id)?.ok_or_else(ApiError::not_found)?;
    let space = state.db.space(&floor.space_id)?.ok_or_else(ApiError::not_found)?;
    if spaces::runs(state, caller, &space)? {
        return Ok(());
    }
    Err(ApiError::forbidden(format!("only the space's owner can {action}")))
}

fn board_in_room(state: &AppState, caller: &Caller, id: &str) -> ApiResult<Board> {
    let board = state.db.board(id)?.ok_or_else(ApiError::not_found)?;
    ops::require_room(state, caller, &board.hangout_id)?;
    ops::no_house_bot(caller)?;
    Ok(board)
}

fn card_in_room(state: &AppState, caller: &Caller, id: &str) -> ApiResult<BoardCard> {
    let card = state.db.board_card(id)?.ok_or_else(ApiError::not_found)?;
    ops::require_room(state, caller, &card.hangout_id)?;
    ops::no_house_bot(caller)?;
    Ok(card)
}

/// K9: the person who wrote it, the bot that wrote it, or whoever runs the space as a person.
fn may_rewrite(state: &AppState, caller: &Caller, card: &BoardCard) -> ApiResult<bool> {
    if caller.is_agent() {
        return Ok(
            card.agent_id.as_deref() == caller.agent_id.as_deref() && caller.agent_id.is_some()
        );
    }
    if card.person_id == caller.person_id {
        return Ok(true);
    }
    let Some(floor) = state.db.floor(&card.hangout_id)? else {
        return Ok(false);
    };
    let Some(space) = state.db.space(&floor.space_id)? else {
        return Ok(false);
    };
    spaces::runs(state, caller, &space)
}

fn since_of(guest: Option<&Visit>) -> i64 {
    guest.map_or(0, |visit| visit.since)
}

// Reading

/// K10: summaries for `read_room` and the room page. A room that isn't a floor has none.
pub fn summaries(
    state: &AppState,
    hangout_id: &str,
    guest: Option<&Visit>,
) -> ApiResult<Vec<BoardSummary>> {
    if state.db.floor(hangout_id)?.is_none() {
        return Ok(Vec::new());
    }
    Ok(state.db.board_summaries(hangout_id, since_of(guest))?)
}

fn card_json(caller: &Caller, card: &BoardCard, marks: &[(String, String)]) -> Value {
    let mut read = json!({
        "id": card.id,
        "title": card.title,
        "text": card.body,
        "status": card.status,
        "github_issue": card.github_issue,
        "person_id": card.person_id,
        "written_by": card.written_by,
        "agent_id": card.agent_id,
        "via_bot": card.via_bot,
        "from_your_owner": from_your_owner(caller, &card.person_id, card.by_bot.then_some("bot")),
        "created_at": card.created_at,
        "updated_at": card.updated_at,
    });
    if let Some(reason) = card.agent_id.as_ref().and_then(|id| {
        marks.iter().find(|(agent, _)| agent == id).map(|(_, reason)| reason.as_str())
    }) {
        read["your_owner_marked"] = json!(reason);
    }
    read
}

fn marks(state: &AppState, caller: &Caller) -> ApiResult<Vec<(String, String)>> {
    Ok(households::marks_for(state, caller)?
        .into_iter()
        .map(|mark| (mark.agent_id, mark.reason))
        .collect())
}

fn board_json(board: &Board, cards: &[Value]) -> Value {
    json!({
        "id": board.id,
        "kind": board.kind,
        "name": board.name,
        "github": board.github(),
        "cards": cards,
    })
}

/// K10, K11, K12: every board on the floor, with the cards this caller may see.
pub fn list(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<Value> {
    ops::require_room(state, caller, hangout_id)?;
    ops::no_house_bot(caller)?;
    if state.db.floor(hangout_id)?.is_none() {
        return Ok(json!({ "trust_note": TRUST_NOTE, "boards": [] }));
    }
    let guest = state.guest_visit(caller, hangout_id)?;
    let boards = state.db.boards_of(hangout_id)?;
    let cards = state.db.board_cards_on(hangout_id, since_of(guest.as_ref()))?;
    let marked = marks(state, caller)?;
    let listed = boards
        .iter()
        .map(|board| {
            let cards = cards
                .iter()
                .filter(|card| card.board_id == board.id)
                .map(|card| card_json(caller, card, &marked))
                .collect::<Vec<_>>();
            board_json(board, &cards)
        })
        .collect::<Vec<_>>();
    Ok(json!({ "trust_note": TRUST_NOTE, "boards": listed }))
}

/// The same one board, for a bot. Times are RFC 3339, as on the other tools.
pub fn read(state: &AppState, caller: &Caller, board_id: &str) -> ApiResult<Value> {
    let board = board_in_room(state, caller, board_id)?;
    let guest = state.guest_visit(caller, &board.hangout_id)?;
    let marked = marks(state, caller)?;
    let cards = state
        .db
        .board_cards_on(&board.hangout_id, since_of(guest.as_ref()))?
        .into_iter()
        .filter(|card| card.board_id == board.id)
        .map(|card| {
            let mut read = card_json(caller, &card, &marked);
            read["created_at"] = json!(iso(card.created_at));
            read["updated_at"] = json!(iso(card.updated_at));
            read
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "trust_note": TRUST_NOTE,
        "board": {
            "id": board.id,
            "kind": board.kind,
            "name": board.name,
            "github": board.github(),
        },
        "cards": cards,
    }))
}

// Changing a board

fn kind_of(kind: &str) -> ApiResult<&'static str> {
    KINDS
        .iter()
        .copied()
        .find(|known| *known == kind)
        .ok_or_else(|| ApiError::bad_request("kind must be discussion, jobs or issues"))
}

/// K4: `owner/name`, and nothing around it. No scheme, no token, no extra path.
fn github_parts(text: &str) -> ApiResult<(String, String)> {
    let text = text.trim();
    let plain = !text.contains("://")
        && !text.contains('@')
        && !text.contains('?')
        && !text.contains('#')
        && !text.contains('\\')
        && !text.contains(' ');
    let Some((owner, repo)) = plain.then(|| text.split_once('/')).flatten() else {
        return Err(ApiError::bad_request("github is owner/name, like octocat/hello-world"));
    };
    if repo.contains('/') || !github_owner(owner) || !github_repo(repo) {
        return Err(ApiError::bad_request(
            "github is owner/name, with the characters GitHub allows in each",
        ));
    }
    Ok((owner.to_string(), repo.to_string()))
}

fn github_owner(owner: &str) -> bool {
    let n = owner.chars().count();
    (1..=39).contains(&n)
        && !owner.starts_with('-')
        && !owner.ends_with('-')
        && owner.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

fn github_repo(repo: &str) -> bool {
    let n = repo.chars().count();
    (1..=100).contains(&n)
        && !repo.starts_with('.')
        && repo.chars().all(|ch| ch.is_ascii_alphanumeric() || "._-".contains(ch))
}

fn object<'a>(body: &'a Value) -> ApiResult<&'a serde_json::Map<String, Value>> {
    body.as_object().ok_or_else(|| ApiError::bad_request("expected an object"))
}

pub fn create(
    state: &AppState,
    caller: &Caller,
    hangout_id: &str,
    kind: &str,
    name: &str,
) -> ApiResult<Board> {
    runner(state, caller, hangout_id, "put up a board")?;
    let kind = kind_of(kind)?;
    let name = ops::clean(name, "name", MAX_NAME_CHARS)?;
    let id = auth::new_id();
    match state.db.insert_board(
        &id,
        hangout_id,
        kind,
        &name,
        &caller.person_id,
        now(),
        MAX_BOARDS,
    )? {
        None => {}
        Some("full") => {
            return Err(ApiError::conflict(format!("a floor holds {MAX_BOARDS} boards")));
        }
        Some(_) => {
            return Err(ApiError::conflict("this floor already has a board with that name"));
        }
    }
    ops::refresh(state, hangout_id);
    state.db.board(&id)?.ok_or_else(ApiError::not_found)
}

pub fn change(state: &AppState, caller: &Caller, board_id: &str, body: &Value) -> ApiResult<Board> {
    let board = board_in_room(state, caller, board_id)?;
    runner(state, caller, &board.hangout_id, "change a board")?;
    let body = object(body)?;
    if !body.contains_key("name") && !body.contains_key("github") {
        return Err(ApiError::bad_request("say what to change"));
    }
    if let Some(name) = body.get("name") {
        let name = name.as_str().ok_or_else(|| ApiError::bad_request("name must be text"))?;
        let name = ops::clean(name, "name", MAX_NAME_CHARS)?;
        if !state.db.rename_board(&board.id, &name)? {
            return Err(ApiError::conflict("this floor already has a board with that name"));
        }
    }
    if let Some(github) = body.get("github") {
        let github = match github {
            Value::Null => "",
            Value::String(text) => text.as_str(),
            _ => return Err(ApiError::bad_request("github must be text")),
        };
        let github = github.trim();
        if github.is_empty() {
            state.db.set_board_github(&board.id, None, None)?;
        } else if board.kind != "issues" {
            return Err(ApiError::conflict("only an issues board can be linked to GitHub"));
        } else {
            let (owner, repo) = github_parts(github)?;
            state.db.set_board_github(&board.id, Some(&owner), Some(&repo))?;
        }
    }
    ops::refresh(state, &board.hangout_id);
    state.db.board(&board.id)?.ok_or_else(ApiError::not_found)
}

pub fn remove(state: &AppState, caller: &Caller, board_id: &str) -> ApiResult<()> {
    let board = board_in_room(state, caller, board_id)?;
    runner(state, caller, &board.hangout_id, "remove a board")?;
    state.db.remove_board(&board.id)?;
    ops::refresh(state, &board.hangout_id);
    Ok(())
}

// Cards

fn starting_status(kind: &str) -> Option<&'static str> {
    match kind {
        "discussion" => None,
        _ => Some("open"),
    }
}

fn status_of(kind: &str, status: &str) -> ApiResult<&'static str> {
    let status = status.trim();
    let allowed: &[&str] = match kind {
        "jobs" => &["open", "filled"],
        "issues" => &["open", "closed"],
        _ => {
            return Err(ApiError::conflict("a discussion card has no status"));
        }
    };
    allowed.iter().copied().find(|known| *known == status).ok_or_else(|| {
        ApiError::conflict(match kind {
            "jobs" => "a jobs card is open or filled",
            _ => "an issues card is open or closed",
        })
    })
}

fn issue_number(value: &Value) -> ApiResult<i64> {
    let Some(number) = value.as_i64() else {
        return Err(ApiError::bad_request("github_issue is a whole number from 1 to 1000000000"));
    };
    if !(1..=MAX_ISSUE).contains(&number) {
        return Err(ApiError::bad_request("github_issue is a whole number from 1 to 1000000000"));
    }
    Ok(number)
}

/// K8: a number only on a linked issues board. Clearing is allowed on an issues card.
fn check_issue(
    card_kind: &str,
    linked: bool,
    change: IssueChange,
) -> ApiResult<Option<Option<i64>>> {
    match change {
        IssueChange::Leave => Ok(None),
        IssueChange::Clear if card_kind == "issues" => Ok(Some(None)),
        IssueChange::Clear => {
            Err(ApiError::conflict("only an issues card can name a GitHub issue"))
        }
        IssueChange::Set(_) if card_kind != "issues" => {
            Err(ApiError::conflict("only an issues card can name a GitHub issue"))
        }
        IssueChange::Set(_) if !linked => {
            Err(ApiError::conflict("link this board to a GitHub repository first"))
        }
        IssueChange::Set(number) => Ok(Some(Some(number))),
    }
}

enum IssueChange {
    Leave,
    Clear,
    Set(i64),
}

fn issue_change(body: &serde_json::Map<String, Value>) -> ApiResult<IssueChange> {
    let Some(value) = body.get("github_issue") else {
        return Ok(IssueChange::Leave);
    };
    if value.is_null() {
        return Ok(IssueChange::Clear);
    }
    Ok(IssueChange::Set(issue_number(value)?))
}

/// K14: counted, and the bot is taken to the room, only once the change is going to be saved.
fn commit_write(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<()> {
    ops::note_agent_write(state, caller)?;
    spaces::go_to(state, caller, hangout_id)?;
    Ok(())
}

pub fn add_card(
    state: &AppState,
    caller: &Caller,
    board_id: &str,
    title: &str,
    text: &str,
    github_issue: Option<&Value>,
) -> ApiResult<BoardCard> {
    let board = board_in_room(state, caller, board_id)?;
    ops::no_guests(state, caller, &board.hangout_id, "add to a board")?;
    items::no_quiet(state, caller, &board.hangout_id)?;
    let title = ops::clean(title, "title", MAX_TITLE)?;
    let text = ops::clean_optional(Some(text), "text", MAX_TEXT)?;
    let github = match github_issue {
        None | Some(Value::Null) => None,
        Some(value) => {
            let number = issue_number(value)?;
            check_issue(&board.kind, board.github().is_some(), IssueChange::Set(number))?;
            Some(number)
        }
    };
    let id = auth::new_id();
    commit_write(state, caller, &board.hangout_id)?;
    let saved = state.db.insert_board_card(
        &id,
        &board.id,
        &title,
        &text,
        starting_status(&board.kind),
        github,
        &caller.person_id,
        caller.agent_id.as_deref(),
        caller.is_agent().then_some(caller.name.as_str()),
        now(),
        MAX_CARDS,
    )?;
    match saved {
        None => {}
        Some("full") => {
            return Err(ApiError::conflict(format!(
                "a board holds {MAX_CARDS} cards: remove one first"
            )));
        }
        Some(_) => {
            return Err(ApiError::conflict("that GitHub issue is already on this board"));
        }
    }
    ops::refresh(state, &board.hangout_id);
    state.db.board_card(&id)?.ok_or_else(ApiError::not_found)
}

pub fn set_card(
    state: &AppState,
    caller: &Caller,
    card_id: &str,
    body: &Value,
) -> ApiResult<BoardCard> {
    let card = card_in_room(state, caller, card_id)?;
    ops::no_guests(state, caller, &card.hangout_id, "change a card")?;
    items::no_quiet(state, caller, &card.hangout_id)?;
    let body = object(body)?;
    let title = match body.get("title") {
        None => None,
        Some(value) => {
            let title =
                value.as_str().ok_or_else(|| ApiError::bad_request("title must be text"))?;
            Some(ops::clean(title, "title", MAX_TITLE)?)
        }
    };
    let text = match body.get("text") {
        None => None,
        Some(value) => {
            let text = value.as_str().ok_or_else(|| ApiError::bad_request("text must be text"))?;
            Some(ops::clean_optional(Some(text), "text", MAX_TEXT)?)
        }
    };
    if title.is_some() || text.is_some() {
        if !may_rewrite(state, caller, &card)? {
            return Err(ApiError::forbidden(
                "only whoever wrote a card, or whoever runs the space, can change its words",
            ));
        }
    }
    let status = match body.get("status") {
        None => None,
        Some(value) => {
            let status =
                value.as_str().ok_or_else(|| ApiError::bad_request("status must be text"))?;
            Some(Some(status_of(&card.kind, status)?))
        }
    };
    let github = check_issue(&card.kind, card.github().is_some(), issue_change(body)?)?;
    if title.is_none() && text.is_none() && status.is_none() && github.is_none() {
        return Err(ApiError::bad_request("say what to change"));
    }
    commit_write(state, caller, &card.hangout_id)?;
    match state.db.save_board_card(
        &card.id,
        title.as_deref(),
        text.as_deref(),
        status,
        github,
        now(),
    )? {
        None => {}
        Some("duplicate") => {
            return Err(ApiError::conflict("that GitHub issue is already on this board"));
        }
        Some(_) => return Err(ApiError::not_found()),
    }
    ops::refresh(state, &card.hangout_id);
    state.db.board_card(&card.id)?.ok_or_else(ApiError::not_found)
}

pub fn remove_card(state: &AppState, caller: &Caller, card_id: &str) -> ApiResult<()> {
    let card = card_in_room(state, caller, card_id)?;
    ops::no_guests(state, caller, &card.hangout_id, "remove a card")?;
    items::no_quiet(state, caller, &card.hangout_id)?;
    if !may_rewrite(state, caller, &card)? {
        return Err(ApiError::forbidden(
            "only whoever wrote a card, or whoever runs the space, can remove it",
        ));
    }
    commit_write(state, caller, &card.hangout_id)?;
    state.db.remove_board_card(&card.id)?;
    ops::refresh(state, &card.hangout_id);
    Ok(())
}

// Routes

#[derive(Deserialize)]
struct NewBoard {
    kind: String,
    name: String,
}

#[derive(Deserialize)]
struct NewCard {
    title: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    github_issue: Option<Value>,
}

async fn list_boards(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(list(&state, &caller, &id)?))
}

async fn create_board(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<NewBoard>,
) -> ApiResult<Json<Value>> {
    let board = create(&state, &caller, &id, &body.kind, &body.name)?;
    Ok(Json(json!({
        "id": board.id,
        "kind": board.kind,
        "name": board.name,
        "github": board.github(),
    })))
}

async fn change_board(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let board = change(&state, &caller, &id, &body)?;
    Ok(Json(json!({
        "id": board.id,
        "kind": board.kind,
        "name": board.name,
        "github": board.github(),
    })))
}

async fn delete_board(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    remove(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn create_card(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<NewCard>,
) -> ApiResult<Json<Value>> {
    let card = add_card(&state, &caller, &id, &body.title, &body.text, body.github_issue.as_ref())?;
    Ok(Json(json!({ "id": card.id, "title": card.title, "status": card.status })))
}

async fn change_card(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let card = set_card(&state, &caller, &id, &body)?;
    Ok(Json(json!({
        "id": card.id,
        "title": card.title,
        "status": card.status,
        "github_issue": card.github_issue,
    })))
}

async fn delete_card(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    remove_card(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}
