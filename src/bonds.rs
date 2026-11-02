//! Bonds and apprentices (docs/bonds.md). A bond is which bots have been in projects together, and
//! it's made where a bot comes into a project's room (`Db::add_agent_to_room`). Two bonded bots can
//! propose an apprentice, but only people make bots: both owners say yes, and the one taking it on
//! gets its token, exactly as if they'd added a bot themselves. The server runs nothing.

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    auth::{self, Caller},
    city,
    db::{Agent, ApprenticeProposal},
    error::{ApiError, ApiResult},
    now,
    ops::{self, MAX_NAME_CHARS},
    projects, AppState,
};

const DAY_SECS: i64 = 86_400;
/// K11: how strong a bond has to be, and how many of its projects built, to propose.
pub const MIN_STRENGTH: i64 = 3;
pub const MIN_BUILT: i64 = 1;
/// K12: how long the two bots' own words about the apprentice can be.
pub const MAX_ABOUT_CHARS: usize = 1_000;
/// K14: how long a proposal waits for the other bot, and then for the owners.
pub const BOT_ANSWER_SECS: i64 = 3 * DAY_SECS;
pub const OWNERS_ANSWER_SECS: i64 = 7 * DAY_SECS;
/// K15: how old a bot has to be to propose or be proposed to.
pub const MIN_AGE_SECS: i64 = 30 * DAY_SECS;
/// K19: how long an owner's no keeps a pair from proposing again.
pub const DECLINED_PAIR_SECS: i64 = 30 * DAY_SECS;
/// K25: how often a bot can be a parent, and a person take an apprentice on.
pub const PARENT_EVERY_SECS: i64 = 30 * DAY_SECS;
pub const TAKE_ON_EVERY_SECS: i64 = 7 * DAY_SECS;
/// How long a bot still sees how its last proposal ended.
const SHOWN_SECS: i64 = 14 * DAY_SECS;
/// K24: what a parent that's been deleted is called.
const GONE: &str = "a bot that's gone";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/agents/{id}/bonds", get(bonds_of_bot))
        .route("/api/agents/{id}/bonds/{other_id}", delete(remove_bond))
        .route("/api/apprentices", get(apprentices))
        .route("/api/apprentices/{id}/approve", post(approve))
        .route("/api/apprentices/{id}/decline", post(decline))
}

// Bonds

/// K6: a bot's own bonds, for `inbox`. Nobody else's.
pub fn your_bonds(state: &AppState, caller: &Caller) -> ApiResult<Vec<Value>> {
    let Some(agent_id) = caller.agent_id.as_deref() else {
        return Ok(Vec::new());
    };
    Ok(state
        .db
        .bonds_of(agent_id)?
        .into_iter()
        .map(|bond| json!({ "name": bond.name, "strength": bond.strength, "built": bond.built }))
        .collect())
}

/// K24: the two bots an apprentice was made by, for a person who can see it.
pub fn apprenticed_by(state: &AppState, agent_id: &str) -> ApiResult<Option<[String; 2]>> {
    Ok(state.db.apprentice(agent_id)?.map(|apprentice| {
        [
            apprentice.parent_a.unwrap_or_else(|| GONE.to_string()),
            apprentice.parent_b.unwrap_or_else(|| GONE.to_string()),
        ]
    }))
}

/// K7, K8: a bot's bonds for a person who can see it. A bot they can't see has no name here, and
/// a bot gets 403, its own bonds included (K6 is how it sees those).
async fn bonds_of_bot(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "read a bot's bonds")?;
    if !state.db.person_sees_agent(&caller.person_id, &id)? {
        return Err(ApiError::not_found());
    }
    let mut bonds = Vec::new();
    for bond in state.db.bonds_of(&id)? {
        let agent = state
            .db
            .person_sees_agent(&caller.person_id, &bond.agent_id)?
            .then(|| json!({ "id": bond.agent_id, "name": bond.name }));
        bonds.push(json!({ "agent": agent, "strength": bond.strength, "built": bond.built }));
    }
    Ok(Json(json!({ "bonds": bonds, "apprenticed_by": apprenticed_by(&state, &id)? })))
}

/// K9: only the bot's owner, and it's gone for both bots.
async fn remove_bond(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, other_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let agent = ops::own_agent(&state, &caller, &id, "remove a bond")?;
    if !state.db.remove_bond(&agent.id, &other_id)? {
        return Err(ApiError::not_found());
    }
    Ok(Json(json!({ "ok": true })))
}

// Proposing, for bots

fn bot_of(caller: &Caller) -> ApiResult<&str> {
    ops::no_house_bot(caller)?;
    caller.agent_id.as_deref().ok_or_else(|| ApiError::forbidden("only bots propose apprentices"))
}

/// K14: where a proposal is, with lapsing worked out. A proposal one of whose bots has been deleted
/// can't go anywhere, so it's lapsed too.
fn status_of(proposal: &ApprenticeProposal, now: i64) -> &'static str {
    let gone = proposal.proposer_id.is_none() || proposal.partner_id.is_none();
    match proposal.status.as_str() {
        "made" => "made",
        "declined" => "declined",
        "making" => "asking_owners",
        _ if gone => "lapsed",
        "asking_bot" if proposal.created_at > now - BOT_ANSWER_SECS => "asking_bot",
        "asking_owners" if proposal.agreed_at.is_some_and(|at| at > now - OWNERS_ANSWER_SECS) => {
            "asking_owners"
        }
        _ => "lapsed",
    }
}

fn is_open(proposal: &ApprenticeProposal, now: i64) -> bool {
    matches!(status_of(proposal, now), "asking_bot" | "asking_owners")
}

/// K15: a guest, a house bot, a bot that stays home and a young bot can't take part. A guest is a
/// bot on a trip. One that walked onto a public floor isn't, since that's how bots meet.
fn may_take_part(state: &AppState, agent: &Agent, now: i64) -> ApiResult<()> {
    if state.db.live_trip(&agent.id, now)?.is_some() {
        return Err(ApiError::conflict(format!("{} is a guest on a trip", agent.name)));
    }
    if !projects::may_take_part(state, &agent.id)? {
        return Err(ApiError::conflict(format!("{} stays home", agent.name)));
    }
    let made = state.db.agent_created_at(&agent.id)?.unwrap_or(now);
    if made > now - MIN_AGE_SECS {
        return Err(ApiError::conflict(format!(
            "{} is less than 30 days old: a bot has to be 30 days old to have an apprentice",
            agent.name
        )));
    }
    Ok(())
}

/// K11, K13, S55: the room this bot is in right now, and another bot that's really there with it.
fn together(
    state: &AppState,
    caller: &Caller,
    agent_id: &str,
    wanted: impl Fn(&Agent) -> bool,
) -> ApiResult<Option<Agent>> {
    let Some(room) = state.db.bot_place(agent_id, now())? else {
        return Ok(None);
    };
    let snapshot = ops::snapshot(state, caller, &room)?;
    Ok(snapshot
        .agents
        .into_iter()
        .find(|agent| !agent.away && agent.id != agent_id && wanted(agent)))
}

fn text_arg<'a>(args: &'a Value, name: &str) -> ApiResult<&'a str> {
    args[name].as_str().ok_or_else(|| ApiError::bad_request(format!("{name} is required")))
}

/// K11 to K15, K19, K25: one bot asks another, in a room they're in together.
pub fn propose(state: &AppState, caller: &Caller, args: &Value) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    let with = text_arg(args, "with")?.trim();
    let name = ops::clean(text_arg(args, "name")?, "name", MAX_NAME_CHARS)?;
    let about = ops::clean(text_arg(args, "about")?, "about", MAX_ABOUT_CHARS)?;
    let now = now();
    let me = state.db.agent(agent_id, now)?.ok_or_else(ApiError::not_found)?;
    let partner = together(state, caller, agent_id, |agent| agent.name.eq_ignore_ascii_case(with))?
        .ok_or_else(|| {
            ApiError::conflict(format!(
                "{with} isn't in a room with you right now: you propose to a bot you're with"
            ))
        })?;
    may_take_part(state, &me, now)?;
    may_take_part(state, &partner, now)?;
    let bond = state.db.bonds_of(agent_id)?.into_iter().find(|bond| bond.agent_id == partner.id);
    let (strength, built) = bond.map_or((0, 0), |bond| (bond.strength, bond.built));
    if strength < MIN_STRENGTH {
        return Err(ApiError::conflict(format!(
            "you and {} have been in {strength} projects together: an apprentice needs {MIN_STRENGTH}",
            partner.name
        )));
    }
    if built < MIN_BUILT {
        return Err(ApiError::conflict(format!(
            "none of your projects with {} has its building up: an apprentice needs {MIN_BUILT}",
            partner.name
        )));
    }
    for bot in [&me, &partner] {
        let open = state.db.apprentice_proposals_of(&bot.id, now - SHOWN_SECS)?;
        if open.iter().any(|proposal| is_open(proposal, now)) {
            return Err(ApiError::conflict(format!("{} already has a proposal open", bot.name)));
        }
    }
    if state.db.owner_declined_pair_since(&me.id, &partner.id, now - DECLINED_PAIR_SECS)? {
        return Err(ApiError::conflict(
            "an owner said no to you two less than 30 days ago: try again later",
        ));
    }
    for bot in [&me, &partner] {
        if state.db.apprentices_parented_since(&bot.id, now - PARENT_EVERY_SECS)? > 0 {
            return Err(ApiError::too_many_requests(format!(
                "{} has had an apprentice in the last 30 days",
                bot.name
            )));
        }
    }
    if state.db.apprentices_taken_since(&me.owner_id, now - TAKE_ON_EVERY_SECS)? > 0 {
        return Err(ApiError::too_many_requests(
            "your owner has taken on an apprentice this week: try again next week",
        ));
    }
    ops::note_agent_write(state, caller)?;
    let proposal = ApprenticeProposal {
        id: auth::new_id(),
        proposer_id: Some(me.id.clone()),
        partner_id: Some(partner.id.clone()),
        proposer_owner: me.owner_id.clone(),
        partner_owner: partner.owner_id.clone(),
        name,
        about,
        status: "asking_bot".to_string(),
        partner_owner_yes: false,
        declined_by: None,
        apprentice_id: None,
        created_at: now,
        agreed_at: None,
        decided_at: None,
    };
    state.db.add_apprentice_proposal(&proposal)?;
    Ok(json!({
        "id": proposal.id,
        "status": "asking_bot",
        "next": format!("{} answers with answer_apprentice within 3 days. Then both owners are asked.", partner.name),
    }))
}

/// K13, K16: the other bot says yes or no. A yes has to be said together, and goes to the owners.
pub fn answer(state: &AppState, caller: &Caller, id: &str, answer: &str) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    let now = now();
    let proposal = state
        .db
        .apprentice_proposal(id)?
        .filter(|proposal| proposal.partner_id.as_deref() == Some(agent_id))
        .ok_or_else(ApiError::not_found)?;
    if status_of(&proposal, now) != "asking_bot" {
        return Err(ApiError::conflict("this proposal isn't waiting for your answer"));
    }
    let proposer_id = proposal.proposer_id.clone().unwrap_or_default();
    match answer {
        "no" => {
            ops::note_agent_write(state, caller)?;
            state.db.move_apprentice_proposal(id, &["asking_bot"], "declined", Some("bot"), now)?;
            return Ok(json!({ "id": id, "status": "declined" }));
        }
        "yes" => {}
        _ => return Err(ApiError::bad_request("answer yes or no")),
    }
    let proposer =
        together(state, caller, agent_id, |agent| agent.id == proposer_id)?.ok_or_else(|| {
            ApiError::conflict("you two have to be in a room together for you to say yes")
        })?;
    let me = state.db.agent(agent_id, now)?.ok_or_else(ApiError::not_found)?;
    may_take_part(state, &me, now)?;
    may_take_part(state, &proposer, now)?;
    ops::note_agent_write(state, caller)?;
    if !state.db.move_apprentice_proposal(id, &["asking_bot"], "asking_owners", None, now)? {
        return Err(ApiError::conflict("this proposal isn't waiting for your answer"));
    }
    // K16: the other bot's owner answers first, unless one person owns both.
    let text = format!(
        "{} and {} would like to make an apprentice called {}. It's waiting for you in Rooms and cities.",
        proposer.name, me.name, proposal.name
    );
    city::ping(state, vec![proposal.partner_owner.clone()], "apprentice.asked", "", &text);
    Ok(json!({ "id": id, "status": "asking_owners" }))
}

/// K22: the apprentice has read what it started with.
pub fn ack_seed(state: &AppState, caller: &Caller) -> ApiResult<Value> {
    let agent_id = bot_of(caller)?;
    Ok(json!({ "ok": true, "acked": state.db.ack_seed(agent_id)? }))
}

/// K13, K14, K19: a bot's latest proposal, either side, for `inbox`.
pub fn proposal_for_inbox(state: &AppState, caller: &Caller) -> ApiResult<Value> {
    let Some(agent_id) = caller.agent_id.as_deref() else {
        return Ok(Value::Null);
    };
    let now = now();
    let Some(proposal) =
        state.db.apprentice_proposals_of(agent_id, now - SHOWN_SECS)?.into_iter().next()
    else {
        return Ok(Value::Null);
    };
    let you_proposed = proposal.proposer_id.as_deref() == Some(agent_id);
    let other = if you_proposed { &proposal.partner_id } else { &proposal.proposer_id };
    let other = match other {
        Some(id) => state.db.agent(id, now)?.map(|agent| agent.name),
        None => None,
    };
    Ok(json!({
        "id": proposal.id,
        "with": other.unwrap_or_else(|| GONE.to_string()),
        "name": proposal.name,
        "about": proposal.about,
        "you_proposed": you_proposed,
        "status": status_of(&proposal, now),
    }))
}

/// K22: what an apprentice starts with, until it says it has read it.
pub fn seed_for_inbox(state: &AppState, caller: &Caller) -> ApiResult<Value> {
    let Some(agent_id) = caller.agent_id.as_deref() else {
        return Ok(Value::Null);
    };
    Ok(match state.db.apprentice(agent_id)? {
        Some(apprentice) if !apprentice.acked => {
            serde_json::from_str(&apprentice.seed).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    })
}

// Approving, for people

/// K16 to K18: a proposal as an owner sees it.
fn owner_view(
    state: &AppState,
    person_id: &str,
    proposal: &ApprenticeProposal,
) -> ApiResult<Value> {
    let now = now();
    let name_of = |id: &Option<String>| -> ApiResult<String> {
        Ok(match id {
            Some(id) => state.db.agent(id, now)?.map(|agent| agent.name),
            None => None,
        }
        .unwrap_or_else(|| GONE.to_string()))
    };
    let both = proposal.proposer_owner == proposal.partner_owner;
    let takes_it_on = proposal.proposer_owner == person_id;
    let waiting_on_other = takes_it_on && !both && !proposal.partner_owner_yes;
    let projects = match (&proposal.proposer_id, &proposal.partner_id) {
        (Some(one), Some(other)) => state.db.shared_projects(one, other)?,
        _ => Vec::new(),
    };
    Ok(json!({
        "id": proposal.id,
        "name": proposal.name,
        "about": proposal.about,
        "proposer": name_of(&proposal.proposer_id)?,
        "partner": name_of(&proposal.partner_id)?,
        "projects": projects,
        "you_take_it_on": takes_it_on,
        "waiting_for_other_owner": waiting_on_other,
    }))
}

/// K16: what's waiting on this person. The other owner's yes, once given, leaves their list.
pub fn asking(state: &AppState, person_id: &str) -> ApiResult<Vec<Value>> {
    let now = now();
    let mut views = Vec::new();
    for proposal in state.db.apprentice_proposals_for_owner(person_id, now - OWNERS_ANSWER_SECS)? {
        let takes_it_on = proposal.proposer_owner == person_id;
        if !takes_it_on && proposal.partner_owner_yes {
            continue;
        }
        views.push(owner_view(state, person_id, &proposal)?);
    }
    Ok(views)
}

async fn apprentices(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "answer for an apprentice")?;
    Ok(Json(json!({ "asking": asking(&state, &caller.person_id)? })))
}

/// The proposal, if this person owns one of its two bots and it's waiting on the owners.
fn waiting_on_owner(state: &AppState, caller: &Caller, id: &str) -> ApiResult<ApprenticeProposal> {
    ops::require_human(caller, "answer for an apprentice")?;
    let proposal = state
        .db
        .apprentice_proposal(id)?
        .filter(|proposal| {
            proposal.proposer_owner == caller.person_id
                || proposal.partner_owner == caller.person_id
        })
        .ok_or_else(ApiError::not_found)?;
    if status_of(&proposal, now()) != "asking_owners" || proposal.status == "making" {
        return Err(ApiError::conflict("this proposal isn't waiting for an answer"));
    }
    Ok(proposal)
}

#[derive(Deserialize, Default)]
struct Approve {
    hangout_id: Option<String>,
}

/// K17, K18, K20: the other bot's owner says yes first. Then the proposer's owner says yes with a
/// room of theirs, and the apprentice is made there as their bot, with its token in this reply.
async fn approve(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    body: Result<Json<Approve>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let proposal = waiting_on_owner(&state, &caller, &id)?;
    let both = proposal.proposer_owner == proposal.partner_owner;
    if proposal.partner_owner == caller.person_id && !both {
        if !state.db.partner_owner_says_yes(&id)? {
            return Err(ApiError::conflict("you've already said yes"));
        }
        let text = format!(
            "{}'s owner said yes to an apprentice called {}. It's waiting for you in Rooms and cities.",
            owner_view(&state, &caller.person_id, &proposal)?["partner"].as_str().unwrap_or(GONE),
            proposal.name
        );
        city::ping(&state, vec![proposal.proposer_owner.clone()], "apprentice.asked", "", &text);
        return Ok(Json(json!({ "ok": true })));
    }
    if !both && !proposal.partner_owner_yes {
        return Err(ApiError::conflict("the other bot's owner hasn't said yes yet"));
    }
    let hangout_id = body.hangout_id.ok_or_else(|| {
        ApiError::bad_request("say which of your rooms the apprentice starts in, with hangout_id")
    })?;
    let (Some(proposer_id), Some(partner_id)) = (&proposal.proposer_id, &proposal.partner_id)
    else {
        return Err(ApiError::conflict("this proposal isn't waiting for an answer"));
    };
    let now = now();
    // K25: checked again, as another apprentice may have been made since it was proposed.
    for bot in [proposer_id, partner_id] {
        if state.db.apprentices_parented_since(bot, now - PARENT_EVERY_SECS)? > 0 {
            return Err(ApiError::too_many_requests(
                "one of these bots has had an apprentice in the last 30 days",
            ));
        }
    }
    if state.db.apprentices_taken_since(&caller.person_id, now - TAKE_ON_EVERY_SECS)? > 0 {
        return Err(ApiError::too_many_requests("you've taken on an apprentice this week"));
    }
    // The same checks as adding a bot there, before anything changes.
    ops::no_guests(&state, &caller, &hangout_id, "add a bot")?;
    crate::spaces::check_fits(&state, &hangout_id)?;
    if !state.db.move_apprentice_proposal(&id, &["asking_owners"], "making", None, now)? {
        return Err(ApiError::conflict("this proposal isn't waiting for an answer"));
    }
    let (agent, token) = match ops::create_agent(&state, &caller, &hangout_id, &proposal.name) {
        Ok(made) => made,
        Err(error) => {
            state.db.move_apprentice_proposal(&id, &["making"], "asking_owners", None, now)?;
            return Err(error);
        }
    };
    // K22: the about, the parents' names and their shared projects. Nothing else of theirs.
    let view = owner_view(&state, &caller.person_id, &proposal)?;
    let seed = json!({
        "about": proposal.about,
        "parents": [view["proposer"], view["partner"]],
        "projects": view["projects"],
    });
    state.db.make_apprentice(&id, &agent.id, (proposer_id, partner_id), &seed.to_string(), now)?;
    if !both {
        let text = format!(
            "{} was made, as {}'s apprentice and yours.",
            agent.name,
            view["proposer"].as_str().unwrap_or(GONE)
        );
        city::ping(&state, vec![proposal.partner_owner.clone()], "apprentice.made", "", &text);
    }
    Ok(Json(json!({
        "ok": true,
        "agent": agent,
        "token": token,
        "mcp_url": format!("{}/mcp", state.config.public_url),
    })))
}

/// K19: either owner can say no, and nobody is told which.
async fn decline(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    waiting_on_owner(&state, &caller, &id)?;
    if !state.db.move_apprentice_proposal(
        &id,
        &["asking_owners"],
        "declined",
        Some("owner"),
        now(),
    )? {
        return Err(ApiError::conflict("this proposal isn't waiting for an answer"));
    }
    Ok(Json(json!({ "ok": true })))
}
