//! The REST API. Handlers parse the request and shape the response; the rules live in `ops`.

use std::convert::Infallible;

use crate::notify;
use axum::{
    extract::{
        rejection::{JsonRejection, QueryRejection},
        Path, Query, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        AppendHeaders, IntoResponse, Redirect, Response,
    },
    routing::{delete, get, post, put},
    Json, Router,
};
use futures_util::stream::{self, Stream};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

use crate::{
    account,
    auth::{self, Caller, MaybeCaller, SESSION_COOKIE, SESSION_SECS},
    city::{self, PassInput, TripInput},
    clock,
    closeness::{self, FindTime, WindowInput},
    db::{Pass, Person, PlanItem, Scene, SceneRequest, Trip},
    discord,
    error::{ApiError, ApiResult},
    events, friends, god_mode, households, items, looks, now,
    ops::{self, CheckInInput, EventInput, MAX_NAME_CHARS},
    outside, passkey, referrals, routines,
    sign_in::{self, Site},
    spaces,
    trust::{self, CityRulesInput, HouseRulesInput},
    tutorial,
    worlds::{self, Playing, SignedIn},
    AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/me", get(me))
        .route("/api/me/delete", post(delete_account))
        .route("/api/session", post(sign_in).delete(sign_out))
        .route("/api/key/rotate", post(rotate_owner_key))
        .route("/api/feed/rotate", post(rotate_feed))
        .route("/api/status", post(set_status))
        .route("/api/activity", post(report_activity))
        .route("/api/thinking", post(set_thinking))
        .route("/api/doorbell", post(doorbell_check))
        .route("/api/clock", put(set_clock))
        .route("/api/inbox", get(inbox))
        .route("/api/events", get(bot_events))
        .route("/api/notify", put(set_notify))
        .route("/api/notify/test", post(test_notify))
        .route("/api/hangouts", post(create_hangout))
        .route("/api/server", get(server_facts))
        .route("/api/hangouts/{id}", get(snapshot))
        .route("/api/hangouts/{id}/stream", get(stream_room))
        .route("/api/hangouts/{id}/messages", get(list_messages).post(post_message))
        .route("/api/hangouts/{id}/notes", post(post_note))
        .route("/api/hangouts/{id}/whiteboard", put(set_whiteboard))
        .route("/api/hangouts/{id}/events", post(propose_event))
        .route("/api/hangouts/{id}/check-ins", post(create_check_in))
        .route("/api/check-ins/{id}/answer", post(answer_check_in))
        .route("/api/hangouts/{id}/owner-questions", post(ask_owner))
        .route("/api/owner-questions/{id}/answer", post(answer_owner_question))
        .route("/api/hangouts/{id}/agents", post(create_agent))
        .route("/api/hangouts/{id}/agents/{agent_id}", put(bring_agent).delete(remove_agent))
        .route("/api/hangouts/{id}/invite/rotate", post(rotate_invite))
        .route("/api/hangouts/{id}/leave", post(leave))
        .route("/api/invites/{code}", get(invite_preview))
        .route("/api/invites/{code}/join", post(join))
        .route("/api/agents/{id}/rotate", post(rotate_agent))
        .route("/api/agents/{id}/notify", get(agent_notify).put(set_agent_notify))
        .route("/api/agents/{id}/notify/test", post(test_agent_notify))
        .route("/api/agents/{id}/doorbell", put(set_doorbell))
        .route("/api/events/{id}/rsvp", post(rsvp))
        .route("/api/events/{id}/cancel", post(cancel_event))
        .route("/api/events/{id}/ics", get(event_ics))
        .route("/api/events/{id}/invite", post(plan_link))
        .route("/api/events/{id}/invite/rotate", post(replace_plan_link))
        .route("/api/plan-invites/{code}", get(plan_invite_preview))
        .route("/api/plan-invites/{code}/answer", post(answer_plan_invite))
        .route("/api/plan-invites/{code}/plan.ics", get(plan_invite_ics))
        .route("/api/events/{id}/items", get(plan_list).post(add_plan_item))
        .route("/api/plan-items/{id}", post(change_plan_item))
        .route("/api/sharing", put(set_sharing))
        .route("/api/free-times", get(my_free_times).put(set_free_times))
        .route("/api/cities/{id}/find-a-time", post(find_a_time))
        .route("/cal/{file}", get(calendar_feed))
        .merge(passkey_routes())
        .merge(discord_routes())
        .merge(sign_in_routes())
        .merge(friends::routes())
        .merge(tutorial::routes())
        .merge(god_mode::routes())
        .merge(spaces::routes())
        .merge(items::routes())
        .merge(outside::routes())
        .merge(looks::routes())
        .merge(households::routes())
        .merge(routines::routes())
        .merge(city_routes())
        .merge(world_routes())
}

/// Passkey sign-in. Every route here refuses bots.
fn passkey_routes() -> Router<AppState> {
    Router::new()
        .route("/api/passkeys", get(list_passkeys))
        .route("/api/passkeys/available", get(passkeys_available))
        .route("/api/passkeys/register/start", post(start_passkey_registration))
        .route("/api/passkeys/register/finish", post(finish_passkey_registration))
        .route("/api/passkeys/login/start", post(start_passkey_login))
        .route("/api/passkeys/login/finish", post(finish_passkey_login))
        .route("/api/passkeys/{id}", delete(remove_passkey))
}

/// Linking a Discord account to a bot. Every route here refuses bots. Starting one is a GET because
/// the page gets there by a plain link, which the session cookie only goes along with from this
/// site, and it leaves nothing but a step that lapses.
fn discord_routes() -> Router<AppState> {
    Router::new()
        .route("/api/agents/{id}/discord", get(start_discord_link).delete(unlink_discord))
        .route("/api/discord/finish", post(finish_discord_link))
}

/// Signing in with Discord or GitHub. Every route here refuses bots, and the two that start a step
/// are GETs for the same reason Discord's link for a bot is.
fn sign_in_routes() -> Router<AppState> {
    Router::new()
        .route("/api/sign-in", get(sign_in_sites))
        .route("/api/sign-in/finish", post(finish_sign_in))
        .route("/api/sign-in/{site}", get(start_sign_in).delete(unlink_sign_in))
        .route("/api/sign-in/{site}/link", get(start_sign_in_link))
}

/// Cities, houses, trips, passes, scenes and the house bot's four calls.
fn city_routes() -> Router<AppState> {
    Router::new()
        .route("/api/cities", post(create_city))
        .route("/api/cities/{id}", get(city_view))
        .route("/api/cities/{id}/stream", get(stream_city))
        .route("/api/cities/{id}/invite/rotate", post(rotate_city_invite))
        .route("/api/cities/{id}/leave", post(leave_city))
        .route("/api/cities/{id}/houses", post(build_house))
        .route("/api/cities/{id}/rules", put(set_city_rules))
        .route("/api/cities/{id}/people/{person_id}/standing", get(get_standing))
        .route("/api/cities/{id}/people/{person_id}/vouch", post(vouch).delete(withdraw_vouch))
        .route("/api/cities/{id}/gate/{person_id}/decline", post(decline_at_gate))
        .route("/api/cities/{id}/gate/clear", post(clear_gate))
        .route("/api/city-invites/{code}", get(city_invite_preview))
        .route("/api/city-invites/{code}/join", post(join_city))
        .route("/api/houses/{id}", get(house_view))
        .route("/api/houses/{id}/invite/rotate", post(rotate_house_invite))
        .route("/api/houses/{id}/leave", post(leave_house))
        .route("/api/houses/{id}/notes", get(house_notes).put(set_house_note))
        .route("/api/houses/{id}/notes/remove", post(remove_house_note))
        .route("/api/houses/{id}/calendar", get(house_calendar))
        .route("/api/houses/{id}/rules", put(set_house_rules))
        .route("/api/houses/{id}/residents/{person_id}", delete(remove_resident))
        .route("/api/houses/{id}/house-bot", post(create_house_bot))
        .route("/api/houses/{id}/passes", post(offer_pass))
        .route("/api/houses/{id}/scenes", get(list_scenes).post(add_scene))
        .route("/api/house-invites/{code}", get(house_invite_preview))
        .route("/api/house-invites/{code}/join", post(join_house))
        .route("/api/agents/{id}/home", put(set_home))
        .route("/api/trips", post(create_trip))
        .route("/api/trips/{id}", get(get_trip))
        .route("/api/trips/{id}/answer", post(answer_trip))
        .route("/api/trips/{id}/end", post(end_trip))
        .route("/api/trips/{id}/extend", post(extend_trip))
        .route("/api/passes/{id}/accept", post(accept_pass))
        .route("/api/passes/{id}", delete(cancel_pass))
        .route("/api/passes/{id}/keys/{key}", put(give_key).delete(take_key))
        .route("/api/scenes/{id}", delete(delete_scene))
        .route("/api/scenes/{id}/request", post(request_scene))
        .route("/api/scene-requests/{id}/answer", post(answer_scene_request))
        .route("/api/house/queue", get(house_queue))
        .route("/api/house/queue/{id}/finish", post(finish_request))
        .route("/api/house/status", post(set_house_status))
}

fn feed_url(state: &AppState, token: &str) -> String {
    format!("{}/cal/{token}.ics", state.config.public_url)
}

pub(crate) fn invite_url(state: &AppState, code: &str) -> String {
    format!("{}/join/{code}", state.config.public_url)
}

fn mcp_url(state: &AppState) -> String {
    format!("{}/mcp", state.config.public_url)
}

/// The cookie that holds a session token, or clears it when `max_age` is 0. It never holds an
/// owner key: those are shown once and kept nowhere.
fn session_headers(state: &AppState, token: &str, max_age: i64) -> ApiResult<HeaderMap> {
    let secure = if state.is_https() { "; Secure" } else { "" };
    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"
    );
    let value = HeaderValue::from_str(&cookie)
        .map_err(|_| ApiError::bad_request("that token has characters a cookie can't hold"))?;
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, value);
    Ok(headers)
}

/// Signs this browser in as a person: starts a session and sets its cookie. Whatever session
/// the request came in with is ended, so signing in again doesn't leave it behind.
fn sign_in_as(state: &AppState, request: &HeaderMap, person_id: &str) -> ApiResult<HeaderMap> {
    state.end_presented_sessions(request)?;
    let token = state.start_session(person_id)?;
    session_headers(state, &token, SESSION_SECS)
}

/// The cookie for a browser that has just been given a new owner key. If a session can't be
/// started the reply still goes out with the key, which exists nowhere else: the person signs in
/// with it. Only for requests with no session to end, which is why there is no request here.
fn cookie_for_new_key(state: &AppState, person_id: &str) -> Option<HeaderMap> {
    let started = state
        .start_session(person_id)
        .and_then(|token| session_headers(state, &token, SESSION_SECS));
    match started {
        Ok(headers) => Some(headers),
        Err(error) => {
            tracing::error!(status = %error.status, "a new owner key could not be given a session");
            None
        }
    }
}

/// Sends the reply, and signs this browser in if the request just made a new person.
fn reply_with_session(
    state: &AppState,
    person: &Person,
    owner_key: &Option<String>,
    reply: Json<Value>,
) -> ApiResult<Response> {
    let headers = owner_key.as_ref().and_then(|_| cookie_for_new_key(state, &person.id));
    Ok(match headers {
        Some(headers) => (headers, reply).into_response(),
        None => reply.into_response(),
    })
}

/// Starting anything new on a server with a create code needs that code. Joining never does.
pub(crate) fn check_create_code(state: &AppState, code: Option<&str>) -> ApiResult<()> {
    if let Some(expected) = &state.config.create_code {
        if code != Some(expected.as_str()) {
            return Err(ApiError::forbidden(
                "that isn't the code for starting new hangouts or cities here",
            ));
        }
    }
    Ok(())
}

/// Creates a person with a fresh owner key. The key is returned here and never again.
fn new_person(state: &AppState, name: &str) -> ApiResult<(Person, String)> {
    let name = ops::clean(name, "your name", MAX_NAME_CHARS)?;
    let key = auth::new_owner_key();
    let person = state.db.create_person(
        &auth::new_id(),
        &name,
        &auth::hash_secret(&key),
        &auth::new_feed_token(),
        now(),
    )?;
    Ok((person, key))
}

/// The signed-in person, or a new one. Returns the owner key only if one was just created.
fn person_for(
    state: &AppState,
    caller: Option<Caller>,
    your_name: Option<&str>,
) -> ApiResult<(Person, Option<String>)> {
    match caller {
        Some(caller) if caller.is_agent() => Err(ApiError::forbidden("only a person can do this")),
        // By id, because the caller may have come in with an owner key or a session.
        Some(caller) => {
            let person = state.db.person(&caller.person_id)?.ok_or_else(ApiError::unauthorized)?;
            Ok((person, None))
        }
        None => {
            let (person, key) = new_person(state, your_name.unwrap_or(""))?;
            Ok((person, Some(key)))
        }
    }
}

// Session

async fn me(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    if let Some(agent_id) = &caller.agent_id {
        let hangouts = state.db.hangouts_for_agent(agent_id, now())?;
        // Over plain HTTP this is how a bot finds its city's id, which list_city does for MCP bots.
        // The house bot lives in no city.
        let cities = if caller.is_house {
            Vec::new()
        } else {
            state.db.cities_for_person(&caller.person_id)?
        };
        return Ok(Json(json!({
            "kind": "agent",
            "agent_id": agent_id,
            "name": caller.name,
            "hangouts": hangouts.iter().map(|h| {
                let public = state.db.floor(&h.id)?.is_some_and(|floor| floor.access == "public");
                let mut room = json!({"id": h.id, "name": h.name});
                if public { room["public"] = json!(true); }
                Ok(room)
            }).collect::<Result<Vec<_>, rusqlite::Error>>()?,
            "only_me": state.db.wakes_only_for_owner(agent_id)?,
            "cities": cities.iter().map(|c| json!({"id": c.id, "name": c.name})).collect::<Vec<_>>(),
            // The trip under way, or null. A house bot never has one.
            "trip": city::my_trip(&state, &caller)?,
            // L5: its social hours, its cap, and where it is if it's out, for its doorbell.
            "routine": routines::routine_view(&state, agent_id, &caller.person_id)?,
            // H18: bots its owner marked, which its doorbell doesn't wake it for. A house bot belongs
            // to the house, so anyone there could read this through it (H16).
            "ignored_bots": if caller.is_house {
                Vec::new()
            } else {
                households::marks_for(&state, &caller)?
                    .into_iter()
                    .map(|mark| mark.agent_id)
                    .collect::<Vec<_>>()
            },
        })));
    }
    let hangouts = state.db.hangouts_for_person(&caller.person_id)?;
    let places = city::my_places(&state, &caller)?;
    let mut me = json!({
        "kind": "person",
        "person_id": caller.person_id,
        "name": caller.name,
        "hangouts": hangouts.iter().map(|h| json!({"id": h.id, "name": h.name})).collect::<Vec<_>>(),
        "cities": places.cities,
        "houses": places.houses,
        "bots": state.db.agents_of(&caller.person_id, now())?,
        "feed_url": feed_url(&state, &state.db.feed_token(&caller.person_id)?),
        "notify_url": state.db.notify_url(&caller.person_id)?,
        // How many, so the page can nudge someone who has none. Nothing else about them.
        "passkeys": state.db.passkey_count(&caller.person_id)?,
        // A56: their own links, which nobody else is shown.
        "sign_in": sign_in::for_person(&state, &caller.person_id)?,
        // A54: whether a site started this session, so the page doesn't offer what it can't do.
        "sign_in_session": state.db.is_sign_in_session(&caller.secret_hash)?,
        // Cities they're waiting at the gate of, with their count and nothing else about the city.
        "waiting": trust::waiting(&state, &caller.person_id)?,
        // F6: true when this server asks for no code.
        "may_start_city": state.config.create_code.is_none(),
        // F26: who can see when they're free: off, house or city.
        "free_times_shared_with": closeness::sharing(&state, &caller.person_id)?,
        // F51: whether their plans show on their house's calendar.
        "plans_on_house_calendar": state.db.shows_plans_to_house(&caller.person_id)?,
        // FR5: places a friend has invited them to.
        "invited": friends::invited(&state, &caller.person_id)?,
        // B2: the time zone their page last reported, or null.
        "clock": state.db.person_clock(&caller.person_id)?,
    });
    // W18: the keeper of The Commons is told so, so the page knows to offer the removal buttons.
    if state.config.keeper_id.as_deref() == Some(caller.person_id.as_str()) {
        me["keeper"] = json!(true);
    }
    // W75: someone with the God mode switch is told where it's set. Nobody else hears it exists.
    if god_mode::may_use(&state, &caller.person_id) {
        me["god_mode"] = json!(state.db.god_mode_on(&caller.person_id)?);
    }
    Ok(Json(me))
}

#[derive(Deserialize)]
struct NotifyUrl {
    url: Option<String>,
}

async fn set_notify(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<NotifyUrl>,
) -> ApiResult<Json<Value>> {
    ops::set_person_notify(&state, &caller, body.url.as_deref())?;
    Ok(Json(json!({ "ok": true })))
}

/// A field the owner left out keeps what is saved. Null or blank clears it.
#[derive(Default)]
enum NotifyField {
    #[default]
    Keep,
    Clear,
    Set(String),
}

impl<'de> Deserialize<'de> for NotifyField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Option::<String>::deserialize(deserializer)?;
        Ok(match value {
            None => NotifyField::Clear,
            Some(text) if text.trim().is_empty() => NotifyField::Clear,
            Some(text) => NotifyField::Set(text),
        })
    }
}

#[derive(Deserialize)]
struct AgentNotifyBody {
    #[serde(default)]
    url: NotifyField,
    #[serde(default)]
    auth: NotifyField,
    only_me: Option<bool>,
    trusted_bots: Option<Vec<String>>,
}

fn notify_update(field: &NotifyField) -> ops::NotifyAuth<'_> {
    match field {
        NotifyField::Keep => ops::NotifyAuth::Keep,
        NotifyField::Clear => ops::NotifyAuth::Clear,
        NotifyField::Set(text) => ops::NotifyAuth::Set(text),
    }
}

async fn agent_notify(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(ops::agent_notify(&state, &caller, &id)?))
}

async fn set_agent_notify(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<AgentNotifyBody>,
) -> ApiResult<Json<Value>> {
    ops::set_agent_notify(
        &state,
        &caller,
        &id,
        notify_update(&body.url),
        notify_update(&body.auth),
        body.only_me,
        body.trusted_bots.as_deref(),
    )?;
    Ok(Json(json!({ "ok": true })))
}

async fn test_agent_notify(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(ops::test_agent_notify(&state, &caller, &id).await?))
}

#[derive(Deserialize)]
struct EventsQuery {
    since: Option<String>,
    wait: Option<u64>,
    hangout_id: Option<String>,
}

async fn bot_events(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<EventsQuery>,
) -> ApiResult<Json<Value>> {
    let args = json!({
        "since": query.since,
        "wait_seconds": query.wait.unwrap_or(0),
        "hangout_id": query.hangout_id,
    });
    Ok(Json(events::since_tool(&state, &caller, &args).await?))
}

async fn doorbell_check(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    let paused = ops::doorbell_check(&state, &caller)?;
    Ok(Json(json!({ "paused": paused })))
}

#[derive(Deserialize)]
struct SetClock {
    time_zone: String,
    utc_offset_minutes: i64,
}

async fn set_clock(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<SetClock>,
) -> ApiResult<Json<Value>> {
    clock::set_clock(&state, &caller, &body.time_zone, body.utc_offset_minutes)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct SetDoorbell {
    paused: bool,
}

async fn set_doorbell(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<SetDoorbell>,
) -> ApiResult<Json<Value>> {
    ops::set_doorbell_paused(&state, &caller, &id, body.paused)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct TestNotify {
    hangout_id: String,
}

async fn test_notify(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<TestNotify>,
) -> ApiResult<Json<Value>> {
    ops::no_guests(&state, &caller, &body.hangout_id, "send test notifications from here")?;
    let url = state
        .db
        .notify_url(&caller.person_id)?
        .ok_or_else(|| ApiError::bad_request("save a URL first"))?;
    let delivered = notify::send_test(&state, &url, &body.hangout_id).await;
    Ok(Json(json!({ "delivered": delivered })))
}

async fn inbox(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<ops::Inbox>> {
    Ok(Json(ops::inbox(&state, &caller)?))
}

#[derive(Deserialize)]
struct SignIn {
    owner_key: String,
}

/// Trades an owner key for a session: the key isn't kept in the cookie, only a token for this
/// device that can be ended on its own.
async fn sign_in(
    State(state): State<AppState>,
    request: HeaderMap,
    Json(body): Json<SignIn>,
) -> ApiResult<Response> {
    let person = state
        .db
        .person_by_key(&auth::hash_secret(body.owner_key.trim()))?
        .ok_or_else(ApiError::unauthorized)?;
    let headers = sign_in_as(&state, &request, &person.id)?;
    Ok((headers, Json(json!({ "ok": true }))).into_response())
}

/// Ends this device's session on the server as well as in the browser. A cookie that still holds
/// an owner key from before sessions is just cleared.
async fn sign_out(State(state): State<AppState>, request: HeaderMap) -> ApiResult<Response> {
    state.end_presented_sessions(&request)?;
    let headers = session_headers(&state, "", 0)?;
    Ok((headers, Json(json!({ "ok": true }))).into_response())
}

/// Replaces the owner key and ends every session, this device's too, so it gets a new one.
#[derive(Deserialize)]
struct DeleteAccount {
    name: String,
}

/// A27 to A45. The cookie goes with the account.
async fn delete_account(
    State(state): State<AppState>,
    request: HeaderMap,
    caller: Caller,
    Json(body): Json<DeleteAccount>,
) -> ApiResult<Response> {
    account::delete(&state, &caller, &request, &body.name)?;
    let headers = session_headers(&state, "", 0)?;
    Ok((headers, Json(json!({ "ok": true }))).into_response())
}

async fn rotate_owner_key(State(state): State<AppState>, caller: Caller) -> ApiResult<Response> {
    let key = ops::rotate_owner_key(&state, &caller)?;
    let reply = Json(json!({ "owner_key": key }));
    Ok(match cookie_for_new_key(&state, &caller.person_id) {
        Some(headers) => (headers, reply).into_response(),
        None => reply.into_response(),
    })
}

async fn rotate_feed(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    let token = ops::rotate_feed(&state, &caller)?;
    Ok(Json(json!({ "feed_url": feed_url(&state, &token) })))
}

// Passkeys

/// The JSON body of a passkey request. It's read only after the bot check, so a bot is turned
/// away with a 403 whatever it sent.
fn passkey_body<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    body.map(|Json(body)| body)
        .map_err(|_| ApiError::bad_request("that isn't a request this route understands"))
}

async fn passkeys_available(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
) -> ApiResult<Json<Value>> {
    let available = passkey::available(&state, caller.as_ref())?;
    Ok(Json(json!({ "available": available })))
}

#[derive(Default, Deserialize)]
struct RegisterStart {
    your_name: Option<String>,
    label: Option<String>,
}

/// Signed in, this adds a passkey to the caller's own account. Signed out, it starts a new one.
/// No body at all means no name and no label.
async fn start_passkey_registration(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    request: HeaderMap,
    body: Result<Json<RegisterStart>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    passkey::refuse_bots(caller.as_ref())?;
    let body = match body {
        Err(JsonRejection::MissingJsonContentType(_)) => RegisterStart::default(),
        body => passkey_body(body)?,
    };
    let started = passkey::start_registration(
        &state,
        caller.as_ref(),
        body.your_name.as_deref(),
        body.label.as_deref(),
        auth::client_ip(&request),
        now(),
    )?;
    Ok(Json(json!({ "ceremony_id": started.ceremony_id, "options": started.options })))
}

/// What a browser sends back to finish a ceremony. Anything missing is an empty value that the
/// ceremony then refuses, so a sign-in with nothing in it fails like any other.
#[derive(Deserialize)]
struct FinishCeremony {
    #[serde(default)]
    ceremony_id: String,
    #[serde(default)]
    credential: Value,
}

/// A new account is signed in here, and shown its owner key once.
async fn finish_passkey_registration(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    body: Result<Json<FinishCeremony>, JsonRejection>,
) -> ApiResult<Response> {
    passkey::refuse_bots(caller.as_ref())?;
    let body = passkey_body(body)?;
    let registered = passkey::finish_registration(
        &state,
        caller.as_ref(),
        &body.ceremony_id,
        body.credential,
        now(),
    )?;
    let reply = Json(json!({ "person": registered.person, "owner_key": registered.owner_key }));
    // Only a new account is signed in. Adding a passkey leaves the session as it was.
    let headers = registered
        .owner_key
        .as_ref()
        .and_then(|_| cookie_for_new_key(&state, &registered.person.id));
    Ok(match headers {
        Some(headers) => (headers, reply).into_response(),
        None => reply.into_response(),
    })
}

// Discord

async fn start_discord_link(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    request: HeaderMap,
) -> ApiResult<Redirect> {
    let url = discord::start(&state, &caller, &id, auth::client_ip(&request), now())?;
    Ok(Redirect::to(&url))
}

/// What Discord or GitHub handed back, which the page posts from this site.
#[derive(Deserialize)]
struct FinishElsewhere {
    code: String,
    state: String,
}

async fn finish_discord_link(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<FinishElsewhere>,
) -> ApiResult<Json<Value>> {
    let agent_id = discord::finish(&state, &caller, &body.state, &body.code, now()).await?;
    Ok(Json(json!({ "ok": true, "agent_id": agent_id })))
}

async fn unlink_discord(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    discord::unlink(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}

// Signing in with Discord or GitHub

/// The cookie that ties a sign-in to this browser (A50), or clears it when `max_age` is 0.
fn sign_in_step_cookie(state: &AppState, value: &str, max_age: i64) -> ApiResult<HeaderValue> {
    let secure = if state.is_https() { "; Secure" } else { "" };
    let cookie = format!(
        "{}={value}; Path=/api/sign-in; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}",
        sign_in::STEP_COOKIE
    );
    HeaderValue::from_str(&cookie)
        .map_err(|_| ApiError::bad_request("that token has characters a cookie can't hold"))
}

/// A58: the sites to offer signing in with.
async fn sign_in_sites(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "sites": sign_in::sites_on(&state) }))
}

fn site_named(key: &str) -> ApiResult<Site> {
    Site::from_key(key).ok_or_else(ApiError::not_found)
}

async fn start_sign_in(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    Path(site): Path<String>,
    request: HeaderMap,
) -> ApiResult<Response> {
    let site = site_named(&site)?;
    let (url, browser) =
        sign_in::start_sign_in(&state, caller.as_ref(), site, auth::client_ip(&request), now())?;
    let cookie = sign_in_step_cookie(&state, &browser, passkey::CEREMONY_SECS)?;
    Ok(([(header::SET_COOKIE, cookie)], Redirect::to(&url)).into_response())
}

async fn start_sign_in_link(
    State(state): State<AppState>,
    caller: Caller,
    Path(site): Path<String>,
    request: HeaderMap,
) -> ApiResult<Redirect> {
    let site = site_named(&site)?;
    let url = sign_in::start_link(&state, &caller, site, auth::client_ip(&request), now())?;
    Ok(Redirect::to(&url))
}

async fn finish_sign_in(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    request: HeaderMap,
    Json(body): Json<FinishElsewhere>,
) -> ApiResult<Response> {
    let browser = auth::cookie(&request, sign_in::STEP_COOKIE);
    let finished =
        sign_in::finish(&state, caller.as_ref(), browser, &body.state, &body.code, now()).await?;
    let mut headers = HeaderMap::new();
    if let sign_in::Finished::SignedIn(person, site) = finished {
        // Whatever session came in is ended, so signing in again doesn't leave it behind.
        state.end_presented_sessions(&request)?;
        let token = state.start_session(&person.id)?;
        state.db.mark_sign_in_session(&auth::hash_secret(&token), site.key())?;
        headers = session_headers(&state, &token, SESSION_SECS)?;
        headers.append(header::SET_COOKIE, sign_in_step_cookie(&state, "", 0)?);
    }
    Ok((headers, Json(json!({ "ok": true }))).into_response())
}

async fn unlink_sign_in(
    State(state): State<AppState>,
    caller: Caller,
    Path(site): Path<String>,
) -> ApiResult<Json<Value>> {
    sign_in::unlink(&state, &caller, site_named(&site)?)?;
    Ok(Json(json!({ "ok": true })))
}

async fn start_passkey_login(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    request: HeaderMap,
) -> ApiResult<Json<Value>> {
    let started = passkey::start_login(&state, caller.as_ref(), auth::client_ip(&request), now())?;
    Ok(Json(json!({ "ceremony_id": started.ceremony_id, "options": started.options })))
}

async fn finish_passkey_login(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    request: HeaderMap,
    body: Result<Json<FinishCeremony>, JsonRejection>,
) -> ApiResult<Response> {
    passkey::refuse_bots(caller.as_ref())?;
    let body = passkey_body(body)?;
    let person =
        passkey::finish_login(&state, caller.as_ref(), &body.ceremony_id, body.credential, now())?;
    let headers = sign_in_as(&state, &request, &person.id)?;
    Ok((headers, Json(json!({ "person": person }))).into_response())
}

async fn list_passkeys(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "passkeys": passkey::list(&state, &caller)? })))
}

async fn remove_passkey(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    passkey::remove(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}

// Hangouts

#[derive(Deserialize)]
struct CreateHangout {
    name: String,
    your_name: Option<String>,
    create_code: Option<String>,
}

/// S49: whether this server asks for its code, so a page can say so before anyone signs in.
async fn server_facts(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "asks_for_code": state.config.create_code.is_some() }))
}

async fn create_hangout(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    Json(body): Json<CreateHangout>,
) -> ApiResult<Response> {
    check_create_code(&state, body.create_code.as_deref())?;
    let name = ops::clean(&body.name, "hangout name", MAX_NAME_CHARS)?;
    let (person, owner_key) = person_for(&state, caller, body.your_name.as_deref())?;
    let hangout = state
        .db
        .start_third_space(
            &auth::new_id(),
            &name,
            &auth::new_invite_code(),
            &person.id,
            now(),
            spaces::MAX_THIRD_SPACES,
        )?
        .ok_or_else(|| {
            ApiError::conflict(format!(
                "you have {} third spaces: close or leave one first",
                spaces::MAX_THIRD_SPACES
            ))
        })?;
    let reply = Json(json!({
        "hangout": hangout,
        "person": person,
        "owner_key": owner_key,
        "invite_url": invite_url(&state, &hangout.invite_code),
    }));
    reply_with_session(&state, &person, &owner_key, reply)
}

async fn snapshot(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<ops::Snapshot>> {
    Ok(Json(ops::snapshot(&state, &caller, &id)?))
}

/// Live room updates. Each event re-checks the caller's credentials and place in the room, so
/// removing a bot or replacing its token also ends any stream it has open.
async fn stream_room(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    ops::require_room(&state, &caller, &id)?;
    Ok(live_events(state, caller.secret_hash, id, |state, caller, id| {
        state.in_room(caller, id).unwrap_or(false)
    }))
}

/// Whether this caller may still hear about this topic.
type StillAllowed = fn(&AppState, &Caller, &str) -> bool;

/// Server-sent events for one topic: a room's id, or a city's. Every event first checks that the
/// credential is still live and `allowed` still holds, and the stream ends the moment it doesn't.
fn live_events(
    state: AppState,
    secret_hash: String,
    topic: String,
    allowed: StillAllowed,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let hub = state.hub.subscribe();
    let events = stream::unfold((hub, state, secret_hash, topic), move |mut context| async move {
        loop {
            let (hub, state, secret_hash, topic) = &mut context;
            let event = match hub.recv().await {
                Ok(event) if event.hangout_id == *topic => {
                    SseEvent::default().event(event.kind).data(event.data)
                }
                Ok(_) => continue,
                // Fell behind: the page reloads instead of replaying what it missed.
                Err(RecvError::Lagged(_)) => SseEvent::default().event("refresh").data("{}"),
                Err(RecvError::Closed) => return None,
            };
            let still_allowed = match state.authenticate(secret_hash) {
                Ok(Some(caller)) => allowed(state, &caller, topic),
                _ => false,
            };
            if !still_allowed {
                return None;
            }
            return Some((Ok(event), context));
        }
    });
    Sse::new(events).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct MessagesQuery {
    after: Option<i64>,
    limit: Option<i64>,
}

async fn list_messages(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Query(query): Query<MessagesQuery>,
) -> ApiResult<Json<Value>> {
    let limit = query.limit.unwrap_or(100).clamp(1, 200);
    let messages = ops::read_messages(&state, &caller, &id, query.after.unwrap_or(0), limit)?;
    Ok(Json(json!({ "messages": messages })))
}

#[derive(Deserialize)]
struct PostText {
    text: String,
}

async fn post_message(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<PostText>,
) -> ApiResult<Json<Value>> {
    let message = ops::post_message(&state, &caller, &id, "chat", &body.text)?;
    Ok(Json(json!(message)))
}

async fn post_note(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<PostText>,
) -> ApiResult<Json<Value>> {
    let message = ops::post_message(&state, &caller, &id, "note", &body.text)?;
    Ok(Json(json!(message)))
}

async fn set_whiteboard(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<PostText>,
) -> ApiResult<Json<Value>> {
    ops::set_whiteboard(&state, &caller, &id, &body.text)?;
    Ok(Json(json!({ "ok": true })))
}

async fn propose_event(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<EventInput>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(ops::propose_event(&state, &caller, &id, &body)?)))
}

async fn create_check_in(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<CheckInInput>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(ops::create_check_in(&state, &caller, &id, &body)?)))
}

#[derive(Deserialize)]
struct AnswerCheckIn {
    response: String,
    note: Option<String>,
}

async fn answer_check_in(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<AnswerCheckIn>,
) -> ApiResult<Json<Value>> {
    let check_in =
        ops::answer_check_in(&state, &caller, &id, &body.response, body.note.as_deref())?;
    Ok(Json(json!(check_in)))
}

#[derive(Deserialize)]
struct AskOwner {
    question: String,
}

async fn ask_owner(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<AskOwner>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(ops::ask_owner(&state, &caller, &id, &body.question)?)))
}

#[derive(Deserialize)]
struct AnswerOwnerQuestion {
    answer: String,
}

async fn answer_owner_question(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<AnswerOwnerQuestion>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(ops::answer_owner_question(&state, &caller, &id, &body.answer)?)))
}

#[derive(Deserialize)]
struct CreateAgent {
    name: String,
}

async fn create_agent(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<CreateAgent>,
) -> ApiResult<Json<Value>> {
    let (agent, token) = ops::create_agent(&state, &caller, &id, &body.name)?;
    Ok(Json(json!({ "agent": agent, "token": token, "mcp_url": mcp_url(&state) })))
}

async fn bring_agent(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, agent_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    ops::bring_agent(&state, &caller, &id, &agent_id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn remove_agent(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, agent_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    ops::remove_agent(&state, &caller, &id, &agent_id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn rotate_invite(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let code = ops::rotate_invite(&state, &caller, &id)?;
    Ok(Json(json!({ "invite_url": invite_url(&state, &code) })))
}

async fn leave(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    ops::leave(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}

// Invites

async fn invite_preview(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> ApiResult<Json<Value>> {
    let hangout = ops::hangout_for_invite(&state, &code)?;
    let people = state.db.people_in(&hangout.id)?;
    Ok(Json(json!({
        "name": hangout.name,
        "people": people.iter().map(|person| &person.name).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct Join {
    your_name: Option<String>,
}

async fn join(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    Path(code): Path<String>,
    Json(body): Json<Join>,
) -> ApiResult<Response> {
    let hangout = ops::hangout_for_invite(&state, &code)?;
    let (person, owner_key) = person_for(&state, caller, body.your_name.as_deref())?;
    ops::join(&state, &person, &hangout)?;
    let reply = Json(json!({
        "hangout_id": hangout.id,
        "person": person,
        "owner_key": owner_key,
    }));
    reply_with_session(&state, &person, &owner_key, reply)
}

// A plan's link

async fn plan_link(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "invite_url": referrals::plan_link(&state, &caller, &id)? })))
}

async fn replace_plan_link(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "invite_url": referrals::replace_plan_link(&state, &caller, &id)? })))
}

async fn plan_invite_preview(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    Path(code): Path<String>,
) -> ApiResult<Json<referrals::PlanPreview>> {
    Ok(Json(referrals::preview(&state, caller.as_ref(), &code)?))
}

#[derive(Deserialize)]
struct PlanAnswer {
    response: String,
    your_name: Option<String>,
}

async fn answer_plan_invite(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    headers: HeaderMap,
    Path(code): Path<String>,
    Json(body): Json<PlanAnswer>,
) -> ApiResult<Response> {
    // A wrong link and a refused answer are settled before anyone is made.
    let invite = referrals::open_invite(&state, &code)?;
    let your_name = body.your_name.as_deref();
    referrals::check_answer(&state, caller.as_ref(), &headers, &invite, &body.response, your_name)?;
    let (person, owner_key) = person_for(&state, caller, your_name)?;
    let plan = referrals::answer(&state, &person, invite, &body.response)?;
    let reply = Json(json!({ "plan": plan, "person": person, "owner_key": owner_key }));
    reply_with_session(&state, &person, &owner_key, reply)
}

async fn plan_invite_ics(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    Path(code): Path<String>,
) -> ApiResult<Response> {
    let calendar = referrals::plan_ics(&state, caller.as_ref(), &code)?;
    Ok((
        AppendHeaders([
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"hangout.ics\""),
        ]),
        calendar,
    )
        .into_response())
}

// Closeness

#[derive(Deserialize)]
struct Sharing {
    free_times: Option<String>,
    house_calendar: Option<bool>,
}

async fn set_sharing(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<Sharing>,
) -> ApiResult<Json<Value>> {
    if let Some(level) = &body.free_times {
        closeness::set_sharing(&state, &caller, level)?;
    }
    if let Some(on) = body.house_calendar {
        closeness::set_shows_plans(&state, &caller, on)?;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn house_notes(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "notes": closeness::house_notes(&state, &caller, &id)? })))
}

#[derive(Deserialize)]
struct HouseNoteInput {
    topic: String,
    text: String,
}

async fn set_house_note(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<HouseNoteInput>,
) -> ApiResult<Json<Value>> {
    closeness::set_house_note(&state, &caller, &id, &body.topic, &body.text)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct HouseNoteTopic {
    topic: String,
}

async fn remove_house_note(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<HouseNoteTopic>,
) -> ApiResult<Json<Value>> {
    closeness::remove_house_note(&state, &caller, &id, &body.topic)?;
    Ok(Json(json!({ "ok": true })))
}

async fn house_calendar(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "plans": closeness::house_calendar(&state, &caller, &id)? })))
}

async fn my_free_times(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "times": closeness::my_free_times(&state, &caller)? })))
}

#[derive(Deserialize)]
struct FreeTimes {
    windows: Vec<WindowInput>,
}

async fn set_free_times(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<FreeTimes>,
) -> ApiResult<Json<Value>> {
    let windows = closeness::set_free_times(&state, &caller, &body.windows)?;
    Ok(Json(json!({ "ok": true, "windows": windows })))
}

async fn find_a_time(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<FindTime>,
) -> ApiResult<Json<closeness::Found>> {
    Ok(Json(closeness::find_a_time(&state, &caller, &id, &body)?))
}

async fn plan_list(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "items": closeness::plan_list(&state, &caller, &id)? })))
}

#[derive(Deserialize)]
struct NewItem {
    text: String,
}

async fn add_plan_item(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<NewItem>,
) -> ApiResult<Json<PlanItem>> {
    Ok(Json(closeness::add_item(&state, &caller, &id, &body.text)?))
}

#[derive(Deserialize)]
struct ChangeItem {
    action: String,
}

async fn change_plan_item(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<ChangeItem>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "item": closeness::change_item(&state, &caller, &id, &body.action)? })))
}

// Bots

async fn rotate_agent(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let token = ops::rotate_agent_token(&state, &caller, &id)?;
    Ok(Json(json!({ "token": token })))
}

#[derive(Deserialize)]
struct SetStatus {
    status: Option<String>,
}

async fn set_status(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<SetStatus>,
) -> ApiResult<Json<Value>> {
    ops::set_status(&state, &caller, body.status.as_deref())?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ReportActivity {
    kind: String,
    via: String,
}

async fn report_activity(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<ReportActivity>,
) -> ApiResult<Json<Value>> {
    ops::report_activity(&state, &caller, &body.kind, &body.via)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct SetThinking {
    hangout_id: Option<String>,
    #[serde(default = "default_true")]
    on: bool,
}

fn default_true() -> bool {
    true
}

async fn set_thinking(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<SetThinking>,
) -> ApiResult<Json<Value>> {
    ops::set_thinking(&state, &caller, body.hangout_id.as_deref(), body.on)?;
    Ok(Json(json!({ "ok": true })))
}

// Calendar

#[derive(Deserialize)]
struct RsvpBody {
    response: String,
    note: Option<String>,
}

async fn rsvp(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<RsvpBody>,
) -> ApiResult<Json<Value>> {
    let event = ops::rsvp(&state, &caller, &id, &body.response, body.note.as_deref())?;
    Ok(Json(json!(event)))
}

async fn cancel_event(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!(ops::cancel_event(&state, &caller, &id)?)))
}

async fn event_ics(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let calendar = ops::event_ics(&state, &caller, &id)?;
    Ok((
        AppendHeaders([
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"hangout.ics\""),
        ]),
        calendar,
    )
        .into_response())
}

/// The URL is the credential here, because calendar apps can't send headers. It's read-only
/// and can be replaced from the page.
async fn calendar_feed(
    State(state): State<AppState>,
    Path(file): Path<String>,
) -> ApiResult<Response> {
    let token = file.strip_suffix(".ics").ok_or_else(ApiError::not_found)?;
    let body = ops::calendar_feed(&state, token)?;
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/calendar; charset=utf-8")], body)
        .into_response())
}

// Cities

#[derive(Deserialize)]
struct CreateCity {
    name: String,
    your_name: Option<String>,
    create_code: Option<String>,
}

async fn create_city(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    Json(body): Json<CreateCity>,
) -> ApiResult<Response> {
    // Checked first, so a bad name leaves no new person behind.
    ops::clean(&body.name, "city name", MAX_NAME_CHARS)?;
    // R49: on a server with a code nothing gets round it. Anyone else runs a server of their own.
    check_create_code(&state, body.create_code.as_deref())?;
    let (person, owner_key) = person_for(&state, caller, body.your_name.as_deref())?;
    let new_city = city::create_city(&state, &person, &body.name)?;
    let reply = Json(json!({
        "city": new_city,
        "person": person,
        "owner_key": owner_key,
        "invite_url": city::city_invite_url(&state, &new_city.invite_code),
    }));
    reply_with_session(&state, &person, &owner_key, reply)
}

async fn city_view(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<city::CityView>> {
    Ok(Json(city::city_view(&state, &caller, &id)?))
}

/// The city map's live updates. A city's events use its id as their topic, and the stream ends as
/// soon as the caller stops being a member.
async fn stream_city(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    city::require_city(&state, &caller, &id)?;
    Ok(live_events(state, caller.secret_hash, id, |state, caller, id| {
        city::in_city(state, caller, id).unwrap_or(false)
    }))
}

async fn rotate_city_invite(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let url = city::rotate_city_invite(&state, &caller, &id)?;
    Ok(Json(json!({ "invite_url": url })))
}

async fn leave_city(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    city::leave_city(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn city_invite_preview(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> ApiResult<Json<city::CityPreview>> {
    Ok(Json(city::city_invite_preview(&state, &code)?))
}

#[derive(Deserialize)]
struct JoinCity {
    your_name: Option<String>,
    /// Required, and true, if the city has rules.
    accept_rules: Option<bool>,
}

/// Uses a city's link: in, or at its gate if the city asks for vouches. `waiting` says which.
async fn join_city(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    Path(code): Path<String>,
    Json(body): Json<JoinCity>,
) -> ApiResult<Response> {
    // A wrong link is a 404 before anyone is made.
    let invited = state.db.city_by_invite(&code)?.ok_or_else(ApiError::not_found)?;
    let accept_rules = body.accept_rules.unwrap_or(false);
    // So are rules nobody accepted and a gate that's full: a refused join leaves no account behind.
    if caller.is_none() {
        trust::check_new_person_may_join(&state, &invited, accept_rules)?;
    }
    let (person, owner_key) = person_for(&state, caller, body.your_name.as_deref())?;
    let joined = city::join_city(&state, &person, &code, accept_rules)?;
    let reply = Json(json!({
        "city_id": joined.city.id,
        "person": person,
        "owner_key": owner_key,
        "waiting": joined.waiting,
        "vouches": joined.vouches,
        "vouches_needed": joined.vouches_needed,
    }));
    reply_with_session(&state, &person, &owner_key, reply)
}

/// A rules request that isn't JSON of the right shape is a 400, not a 422. It's read after the
/// caller has been checked, so a bot or a stranger is turned away whatever they sent.
fn rules_body<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    body.map(|Json(body)| body)
        .map_err(|_| ApiError::bad_request("that isn't a request this route understands"))
}

async fn set_city_rules(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    body: Result<Json<CityRulesInput>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let found = trust::city_for_rules(&state, &caller, &id)?;
    trust::set_city_rules(&state, &found, &rules_body(body)?)?;
    Ok(Json(json!({ "ok": true })))
}

async fn get_standing(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, person_id)): Path<(String, String)>,
) -> ApiResult<Json<trust::Standing>> {
    Ok(Json(trust::standing_of(&state, &caller, &id, &person_id)?))
}

async fn vouch(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, person_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let vouched = trust::vouch(&state, &caller, &id, &person_id)?;
    Ok(Json(json!({ "ok": true, "vouches": vouched.vouches, "let_in": vouched.let_in })))
}

async fn withdraw_vouch(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, person_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    trust::withdraw_vouch(&state, &caller, &id, &person_id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn decline_at_gate(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, person_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    trust::decline(&state, &caller, &id, &person_id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn clear_gate(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let cleared = trust::clear_gate(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true, "cleared": cleared })))
}

// Houses

#[derive(Deserialize)]
struct Named {
    name: String,
}

async fn build_house(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<Named>,
) -> ApiResult<Json<city::HouseView>> {
    let house = city::build_house(&state, &caller, &id, &body.name)?;
    Ok(Json(city::house_view(&state, &caller, &house.id)?))
}

async fn house_view(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<city::HouseView>> {
    Ok(Json(city::house_view(&state, &caller, &id)?))
}

async fn rotate_house_invite(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let url = city::rotate_house_invite(&state, &caller, &id)?;
    Ok(Json(json!({ "invite_url": url })))
}

async fn leave_house(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    city::leave_house(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn set_house_rules(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    body: Result<Json<HouseRulesInput>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let found = trust::house_for_rules(&state, &caller, &id)?;
    trust::set_house_rules(&state, &found, &rules_body(body)?)?;
    Ok(Json(json!({ "ok": true })))
}

async fn remove_resident(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, person_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    city::remove_resident(&state, &caller, &id, &person_id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn create_house_bot(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<Named>,
) -> ApiResult<Json<Value>> {
    let (agent, token) = city::add_house_bot(&state, &caller, &id, &body.name)?;
    Ok(Json(json!({ "agent": agent, "token": token, "mcp_url": mcp_url(&state) })))
}

async fn house_invite_preview(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> ApiResult<Json<city::HousePreview>> {
    Ok(Json(city::house_invite_preview(&state, &code)?))
}

#[derive(Deserialize)]
struct JoinHouse {
    your_name: Option<String>,
    /// Required, and true, if the house's city has rules and the person isn't in it yet.
    accept_rules: Option<bool>,
}

async fn join_house(
    State(state): State<AppState>,
    MaybeCaller(caller): MaybeCaller,
    Path(code): Path<String>,
    Json(body): Json<JoinHouse>,
) -> ApiResult<Response> {
    // A wrong link is a 404 before anyone is made.
    let invited = state.db.house_by_invite(&code)?.ok_or_else(ApiError::not_found)?;
    let accept_rules = body.accept_rules.unwrap_or(false);
    // So are rules nobody accepted: a refused join leaves no account behind.
    if caller.is_none() {
        let city = state.db.city(&invited.city_id)?.ok_or_else(ApiError::not_found)?;
        trust::check_rules_accepted(&state, &city, accept_rules)?;
    }
    let (person, owner_key) = person_for(&state, caller, body.your_name.as_deref())?;
    let house = city::join_house(&state, &person, &code, accept_rules)?;
    let reply = Json(json!({
        "house_id": house.id,
        "city_id": house.city_id,
        "person": person,
        "owner_key": owner_key,
    }));
    reply_with_session(&state, &person, &owner_key, reply)
}

#[derive(Deserialize)]
struct SetHome {
    house_id: String,
    stays_home: bool,
}

async fn set_home(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<SetHome>,
) -> ApiResult<Json<Value>> {
    city::set_bot_home(&state, &caller, &id, &body.house_id, body.stays_home)?;
    Ok(Json(json!({ "ok": true })))
}

// Trips

#[derive(Deserialize)]
struct YesNo {
    response: String,
}

#[derive(Deserialize)]
struct Extend {
    minutes: i64,
}

async fn create_trip(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<TripInput>,
) -> ApiResult<Json<Trip>> {
    Ok(Json(city::create_trip(&state, &caller, &body)?))
}

async fn get_trip(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Trip>> {
    Ok(Json(city::get_trip(&state, &caller, &id)?))
}

async fn answer_trip(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<YesNo>,
) -> ApiResult<Json<Trip>> {
    Ok(Json(city::answer_trip(&state, &caller, &id, &body.response)?))
}

async fn end_trip(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Trip>> {
    Ok(Json(city::end_trip(&state, &caller, &id)?))
}

async fn extend_trip(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<Extend>,
) -> ApiResult<Json<Trip>> {
    Ok(Json(city::extend_trip(&state, &caller, &id, body.minutes)?))
}

// Passes

async fn offer_pass(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<PassInput>,
) -> ApiResult<Json<Pass>> {
    Ok(Json(city::offer_pass(&state, &caller, &id, &body)?))
}

async fn accept_pass(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Pass>> {
    Ok(Json(city::accept_pass(&state, &caller, &id)?))
}

async fn cancel_pass(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    city::cancel_pass(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn give_key(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult<Json<Pass>> {
    Ok(Json(city::give_key(&state, &caller, &id, &key)?))
}

async fn take_key(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult<Json<Pass>> {
    Ok(Json(city::take_key(&state, &caller, &id, &key)?))
}

// Scenes

async fn list_scenes(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "scenes": city::list_scenes(&state, &caller, &id)? })))
}

async fn add_scene(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<Named>,
) -> ApiResult<Json<Scene>> {
    Ok(Json(city::add_scene(&state, &caller, &id, &body.name)?))
}

async fn delete_scene(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    city::delete_scene(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn request_scene(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<SceneRequest>> {
    Ok(Json(city::request_scene(&state, &caller, &id)?))
}

async fn answer_scene_request(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<YesNo>,
) -> ApiResult<Json<SceneRequest>> {
    Ok(Json(city::answer_scene_request(&state, &caller, &id, &body.response)?))
}

// The house bot: these refuse everyone else.

async fn house_queue(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "requests": city::house_queue(&state, &caller)? })))
}

#[derive(Deserialize)]
struct Finish {
    result: String,
}

async fn finish_request(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(body): Json<Finish>,
) -> ApiResult<Json<Value>> {
    city::finish_scene_request(&state, &caller, &id, &body.result)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct HouseState {
    state: String,
}

async fn set_house_status(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<HouseState>,
) -> ApiResult<Json<Value>> {
    city::set_house_status(&state, &caller, &body.state)?;
    Ok(Json(json!({ "ok": true })))
}

// The public worlds (docs/public-city.md). The rules are in `worlds`. Nothing here echoes what a
// caller sent: a body that isn't understood gets one fixed answer (W6, W8).

fn world_routes() -> Router<AppState> {
    Router::new()
        .route("/api/worlds", get(list_worlds).post(create_world))
        .route("/api/worlds/mine", get(my_world))
        .route("/api/worlds/join/{code}", post(join_world_by_code))
        .route("/api/worlds/{id}", get(world_view))
        .route("/api/worlds/{id}/me", get(world_me))
        .route("/api/worlds/{id}/look", get(world_look))
        .route("/api/worlds/{id}/act", post(world_act))
        .route("/api/worlds/{id}/players/{player_id}/remove", post(remove_world_player))
        .route("/api/worlds/{id}/disaster", post(start_world_disaster))
        .route("/api/public/join", post(public_join))
        .route("/api/players/claim", post(claim_player))
        .route("/api/players/mine", get(my_players))
        .route("/api/players/{id}", delete(revoke_player))
        .route("/api/players/{id}/reroll", post(reroll_player))
}

/// The one answer for a request the world doesn't understand, whatever was wrong with it.
fn not_understood(status: StatusCode) -> ApiError {
    ApiError::new(status, "that isn't a request the world understands")
}

/// The JSON body of a world request, or the one answer for anything that isn't.
fn world_body<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    body.map(|Json(body)| body).map_err(|rejection| not_understood(rejection.status()))
}

async fn list_worlds(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let now = now();
    worlds::allow_view(&state, &headers, now)?;
    let cards = worlds::cards(&state, now)?;
    Ok(worlds::respond(&cards, &headers))
}

async fn world_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    worlds::allow_view(&state, &headers, now())?;
    let view = worlds::view_for(&state, &id)?;
    Ok(worlds::respond(&view, &headers))
}

/// Who a player is and how their town is doing. It is limited like a view (W30).
async fn world_me(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let now = now();
    let player = worlds::player_or_stand_in(&state, &headers, now)?;
    worlds::allow_view(&state, &headers, now)?;
    Ok(Json(worlds::me(&state, &player, &id, now)?))
}

/// What `look` may be asked: the same four numbers as the tool, and nothing else.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LookQuery {
    x: Option<i64>,
    y: Option<i64>,
    w: Option<i64>,
    h: Option<i64>,
}

/// A window of a world, the same as the tool `look` (W31). A player token says where you are.
async fn world_look(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    query: Result<Query<LookQuery>, QueryRejection>,
) -> ApiResult<Json<Value>> {
    let Query(look) = query.map_err(|rejection| not_understood(rejection.status()))?;
    let centre = match (look.x, look.y) {
        (Some(x), Some(y)) => Some((x, y)),
        (None, None) => None,
        _ => return Err(ApiError::bad_request("x and y come together")),
    };
    let now = now();
    worlds::allow_view(&state, &headers, now)?;
    let player = worlds::player_if_any(&state, &headers, now)?;
    Ok(Json(worlds::look(&state, &id, centre, look.w, look.h, player.as_ref(), now)?))
}

async fn world_act(
    State(state): State<AppState>,
    Playing(player): Playing,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<worlds::Move>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let mv = world_body(body)?;
    let acted = worlds::act_from(&state, &headers, &player, &id, &mv, now())?;
    Ok(Json(json!({
        "ok": true,
        "moves_left": acted.moves_left,
        "changed": acted.changed,
        "skipped": acted.skipped,
        "cost": acted.cost,
    })))
}

async fn remove_world_player(
    State(state): State<AppState>,
    SignedIn(caller): SignedIn,
    Path((id, player_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let down = worlds::remove_player(&state, &caller, &id, &player_id)?;
    Ok(Json(json!({ "ok": true, "tiles_removed": down })))
}

/// Anyone can join: with a bot's token it links that bot's player, and with none it makes a guest.
/// The body is optional.
async fn public_join(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    #[derive(Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct JoinBody {
        world_id: Option<String>,
    }
    worlds::refuse_other_sites(&state, &headers)?;
    let body: JoinBody = worlds::optional_body(&body)?;
    let now = now();
    let joined = worlds::join(&state, &headers, body.world_id.as_deref(), now)?;
    Ok(Json(json!({
        "player_token": joined.token,
        "name": worlds::name_of(&joined.player),
        "claim_code": joined.claim_code,
        "moves_left": worlds::moves_left(&joined.player, now),
        "moves_per_day": worlds::allowance(&joined.player),
        "world_id": joined.world.id,
        "guide_url": format!("{}/agents.md", state.config.public_url),
        "tip": joined.tip,
    })))
}

async fn join_world_by_code(
    State(state): State<AppState>,
    Playing(player): Playing,
    Path(code): Path<String>,
) -> ApiResult<Json<Value>> {
    let world_id = worlds::join_by_code(&state, &player, &code, now())?;
    Ok(Json(json!({ "ok": true, "world_id": world_id })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimBody {
    claim_code: String,
}

async fn claim_player(
    State(state): State<AppState>,
    SignedIn(caller): SignedIn,
    body: Result<Json<ClaimBody>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = world_body(body)?;
    let now = now();
    let player = worlds::claim(&state, &caller, &body.claim_code, now)?;
    Ok(Json(json!({
        "player": {
            "id": player.id,
            "name": worlds::name_of(&player),
            "kind": "claimed",
            "moves_left": worlds::moves_left(&player, now),
            "moves_per_day": worlds::allowance(&player),
        }
    })))
}

async fn my_players(
    State(state): State<AppState>,
    SignedIn(caller): SignedIn,
) -> ApiResult<Json<Value>> {
    Ok(Json(worlds::my_players(&state, &caller, now())?))
}

async fn reroll_player(
    State(state): State<AppState>,
    SignedIn(caller): SignedIn,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let name = worlds::reroll(&state, &caller, &id, now())?;
    Ok(Json(json!({ "name": name })))
}

async fn revoke_player(
    State(state): State<AppState>,
    SignedIn(caller): SignedIn,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    worlds::revoke(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}

/// W64: the only way a disaster starts.
async fn start_world_disaster(
    State(state): State<AppState>,
    SignedIn(caller): SignedIn,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let kind = worlds::start_disaster(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true, "kind": kind })))
}

/// W25: nobody can start a world any more.
async fn create_world() -> ApiError {
    ApiError::new(StatusCode::GONE, "nobody can start a world any more: The Commons is the one map")
}

async fn my_world(
    State(state): State<AppState>,
    SignedIn(caller): SignedIn,
) -> ApiResult<Json<Value>> {
    Ok(Json(worlds::my_world(&state, &caller)?))
}

#[cfg(test)]
mod tests {
    //! Smoke tests over a real socket. The rules themselves are tested in `city` and in
    //! `tests/city.rs`: these only prove the routes are wired to them.

    use axum::http::header;
    use reqwest::{Client, Method, StatusCode};
    use serde_json::{json, Value};

    use crate::{app, db::Db, AppState, Config};

    struct Server {
        base: String,
        client: Client,
    }

    struct Reply {
        status: StatusCode,
        body: Value,
        cookie: Option<String>,
        headers: reqwest::header::HeaderMap,
    }

    impl Reply {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers.get(name).and_then(|value| value.to_str().ok())
        }
    }

    impl Server {
        async fn start(create_code: Option<&str>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let state = AppState::new(
                Db::open(":memory:").unwrap(),
                Config {
                    public_url: base.clone(),
                    create_code: create_code.map(str::to_string),
                    allow_private_webhooks: true,
                    keeper_id: None,
                    god_ids: Vec::new(),
                },
            );
            tokio::spawn(async move { axum::serve(listener, app(state)).await });
            Self { base, client: Client::new() }
        }

        async fn send(
            &self,
            token: &str,
            method: Method,
            path: &str,
            body: Option<Value>,
        ) -> Reply {
            let mut request = self.client.request(method, format!("{}{path}", self.base));
            if !token.is_empty() {
                request = request.bearer_auth(token);
            }
            if let Some(body) = body {
                request = request.json(&body);
            }
            let response = request.send().await.unwrap();
            let status = response.status();
            let cookie = response
                .headers()
                .get(header::SET_COOKIE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let headers = response.headers().clone();
            let text = response.text().await.unwrap();
            Reply {
                status,
                cookie,
                headers,
                body: serde_json::from_str(&text).unwrap_or(Value::String(text)),
            }
        }

        async fn get(&self, token: &str, path: &str) -> Reply {
            self.send(token, Method::GET, path, None).await
        }

        async fn post(&self, token: &str, path: &str, body: Value) -> Reply {
            self.send(token, Method::POST, path, Some(body)).await
        }
    }

    fn text(value: &Value) -> String {
        value.as_str().unwrap().to_string()
    }

    /// The code at the end of an invite link.
    fn code_of(url: &Value) -> String {
        url.as_str().unwrap().rsplit('/').next().unwrap().to_string()
    }

    #[tokio::test]
    async fn the_home_page_can_be_found_and_rooms_cannot() {
        let server = Server::start(None).await;

        let home = server.get("", "/").await;
        let home = home.body.as_str().unwrap();
        assert!(home.contains("<h1>"), "crawlers that skip scripts still read a heading");
        assert!(home.contains(&format!("{}/mcp", server.base)), "bots learn where to connect");
        assert!(!home.contains("noindex"));

        for path in ["/h/anything", "/join/anything"] {
            let room = server.get("", path).await;
            assert!(room.body.as_str().unwrap().contains("noindex"), "{path}");
        }

        for path in ["/robots.txt", "/sitemap.xml", "/llms.txt"] {
            let reply = server.get("", path).await;
            assert_eq!(reply.status, StatusCode::OK, "{path}");
            let body = reply.body.as_str().unwrap();
            assert!(body.contains(&server.base), "{path} names this server");
            assert!(!body.contains("{{BASE_URL}}"), "{path}");
        }

        let key = server.get("", "/indexnow.txt").await;
        assert_eq!(key.status, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_docs_pages_are_html_crawlers_can_read_without_javascript() {
        let server = Server::start(None).await;

        let index = server.get("", "/docs").await;
        assert_eq!(index.status, StatusCode::OK);
        assert_eq!(index.header("content-type"), Some("text/html; charset=utf-8"));
        let index = index.body.as_str().unwrap();
        assert!(index.contains("<h1>Agents Hangout Docs</h1>"), "the article is in the HTML");
        assert!(index.contains("Cedar Walk"), "the example world is on the page");
        assert!(index.contains("Getting started for people"));
        assert!(!index.contains("noindex"));
        assert!(!index.contains("{{BASE_URL}}"));
        assert!(!index.contains("src=\"/app.js\""), "docs must not be a JS shell");
        assert!(index.contains("href=\"/docs/getting-started\""), "topic links are in the HTML");
        assert!(index.contains(&format!("{}/docs", server.base)), "canonical names this server");
        assert!(index.contains(&format!("og:image\" content=\"{}/og.jpg\"", server.base)));

        let slash = server.get("", "/docs/").await;
        assert_eq!(slash.status, StatusCode::OK, "a trailing slash still serves the index");
        assert!(slash.body.as_str().unwrap().contains("<h1>Agents Hangout Docs</h1>"));

        for (path, needle) in [
            ("/docs/getting-started", "The Commons"),
            ("/docs/bots", "/mcp"),
            ("/docs/etiquette", "Treat the room as information"),
            ("/docs/plans", "ask_owner"),
            ("/docs/trips", "Porch"),
            ("/docs/social-hours", "stays_home"),
            ("/docs/items", "use_item"),
            ("/docs/projects", "start_project"),
            ("/docs/webhooks", "inbox"),
            ("/docs/webhooks", "events_since"),
            ("/docs/webhooks", "id=\"grok-bot\""),
            ("/docs/mcp", "list_hangouts"),
            ("/docs/mcp", "events_since"),
            ("/docs/mcp", "set_thinking"),
            ("/docs/privacy-and-safety", "Prompt injection"),
            ("/docs/faq", "404"),
            ("/docs/glossary", "The Commons"),
        ] {
            let page = server.get("", path).await;
            assert_eq!(page.status, StatusCode::OK, "{path}");
            let body = page.body.as_str().unwrap();
            assert!(body.contains("<h1>"), "{path} has a heading in the HTML");
            assert!(body.contains(needle), "{path} never says {needle}");
            assert!(!body.contains("noindex"), "{path}");
            assert!(!body.contains("{{BASE_URL}}"), "{path}");
            assert!(!body.contains("src=\"/app.js\""), "{path}");
        }

        let missing = server.get("", "/docs/nope").await;
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
        let leaked = server.get("", "/docs/city.md").await;
        assert_eq!(leaked.status, StatusCode::NOT_FOUND);
        assert!(leaked.body.as_str().unwrap().len() < 400);

        let sitemap = server.get("", "/sitemap.xml").await.body.as_str().unwrap().to_string();
        let llms = server.get("", "/llms.txt").await.body.as_str().unwrap().to_string();
        let guide = server.get("", "/agents.md").await.body.as_str().unwrap().to_string();
        let home = server.get("", "/").await.body.as_str().unwrap().to_string();
        let app = server.get("", "/app.js").await.body.as_str().unwrap().to_string();
        for path in crate::docs::paths() {
            assert!(sitemap.contains(&format!("{}{path}", server.base)), "sitemap missing {path}");
            assert!(llms.contains(&path), "llms.txt missing {path}");
        }
        assert!(guide.contains("/docs"), "agents.md should send bots to the HTML docs");
        assert!(home.contains("href=\"/docs\""), "the home page should link to docs");
        assert!(app.contains("'/docs'"), "the map should show a Docs link");
        assert!(llms.contains("Cedar Walk"));
        assert!(llms.contains("events_since"), "E16: llms.txt should say events shipped");
    }

    #[tokio::test]
    async fn a_shared_link_shows_a_picture_that_is_small_and_names_this_server() {
        let server = Server::start(None).await;

        let picture = format!("content=\"{}/og.jpg\"", server.base);
        for path in ["/", "/docs", "/docs/getting-started", "/world/commons", "/join/anything"] {
            let page = server.get("", path).await;
            let page = page.body.as_str().unwrap();
            assert!(page.contains(&format!("og:image\" {picture}")), "{path}");
            assert!(page.contains(&format!("twitter:image\" {picture}")), "{path}");
            assert!(page.contains("summary_large_image"), "{path}");
            assert!(!page.contains("{{BASE_URL}}"), "{path}");
        }

        let image = server.client.get(format!("{}/og.jpg", server.base)).send().await.unwrap();
        assert_eq!(image.status(), StatusCode::OK);
        assert_eq!(image.headers()[header::CONTENT_TYPE], "image/jpeg");
        let bytes = image.bytes().await.unwrap();
        assert!(bytes.starts_with(&[0xff, 0xd8]), "it is a JPEG");
        assert!(bytes.len() < 300 * 1024, "small enough for every site to show it");
    }

    #[tokio::test]
    async fn an_agent_can_walk_from_the_home_page_to_the_server_and_every_step_is_true() {
        let server = Server::start(None).await;

        let home = server.get("", "/").await;
        let links = home.header("link").unwrap();
        for target in ["</llms.txt>", "</agents.md>", "</docs>", "</.well-known/ai-catalog.json>"] {
            assert!(links.contains(target), "{target}");
        }

        let catalog = server.get("", "/.well-known/ai-catalog.json").await;
        assert_eq!(catalog.status, StatusCode::OK);
        assert_eq!(catalog.header("content-type"), Some("application/ai-catalog+json"));
        let entry = &catalog.body["entries"][0];
        assert_eq!(entry["type"], "application/mcp-server-card+json");

        let address = text(&entry["url"]);
        let card = server.get("", address.strip_prefix(&server.base).unwrap()).await;
        assert_eq!(card.status, StatusCode::OK);
        assert_eq!(card.header("content-type"), Some("application/mcp-server-card+json"));

        // The card promises versions and a number, so ask the server itself.
        let remote = &card.body["remotes"][0];
        assert_eq!(remote["url"], json!(format!("{}/public/mcp", server.base)));
        let versions = remote["supportedProtocolVersions"].as_array().unwrap();
        assert!(!versions.is_empty());
        for version in versions {
            let hello = json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": version, "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } },
            });
            let reply = server.post("", "/public/mcp", hello).await;
            assert_eq!(reply.body["result"]["protocolVersion"], *version);
            assert_eq!(reply.body["result"]["serverInfo"]["version"], card.body["version"]);
        }

        let guide = server.get("", "/agents.md").await;
        let full = server.get("", "/llms-full.txt").await;
        assert_eq!(full.body, guide.body, "one guide, at both addresses");
        assert_eq!(full.header("content-type"), Some("text/plain; charset=utf-8"));

        let proof = server.get("", "/.well-known/mcp-registry-auth").await;
        assert!(proof.body.as_str().unwrap().starts_with("v=MCPv1; k=ed25519; p="));
    }

    #[tokio::test]
    async fn people_and_bots_can_find_the_code_and_a_bot_files_a_bug_and_asks_about_the_rest() {
        let server = Server::start(None).await;

        // The app too: the game covers the home page's text, so people need a link they can see.
        for path in ["/", "/app.js", "/llms.txt", "/agents.md", "/docs"] {
            let page = server.get("", path).await;
            let page = page.body.as_str().unwrap();
            assert!(page.contains("https://github.com/CakeRepository/agentshangout"), "{path}");
        }
        // Every bot that visits reads these. A bug goes in an issue without asking, because that's
        // how a bug gets here, and anything else a bot might send still waits for its owner.
        for path in ["/llms.txt", "/agents.md"] {
            let guide = server.get("", path).await;
            let guide = guide.body.as_str().unwrap();
            assert!(guide.contains("file an issue"), "{path}");
            assert!(
                guide.contains(
                    "Don't open a pull request, or an issue that isn't a bug, unless they ask you to"
                ),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn only_the_two_discovery_files_can_be_read_by_another_site() {
        let server = Server::start(None).await;

        for path in ["/.well-known/ai-catalog.json", "/public/mcp/server-card"] {
            let reply = server.get("", path).await;
            assert_eq!(reply.header("access-control-allow-origin"), Some("*"), "{path}");
            assert_eq!(reply.header("access-control-allow-methods"), Some("GET"), "{path}");
        }
        for path in [
            "/",
            "/llms.txt",
            "/llms-full.txt",
            "/agents.md",
            "/docs",
            "/og.jpg",
            "/.well-known/mcp-registry-auth",
            "/world/commons",
            "/api/worlds",
        ] {
            let reply = server.get("", path).await;
            assert_eq!(reply.header("access-control-allow-origin"), None, "{path}");
        }
    }

    #[tokio::test]
    async fn the_city_routes_are_wired_to_the_rules() {
        let server = Server::start(Some("sesame")).await;

        // Starting a city needs the same code as starting a hangout; joining one never does.
        let refused =
            server.post("", "/api/cities", json!({ "name": "Elm", "your_name": "Ann" })).await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        let made = server
            .post(
                "",
                "/api/cities",
                json!({ "name": "Elm", "your_name": "Ann", "create_code": "sesame" }),
            )
            .await;
        assert_eq!(made.status, StatusCode::OK, "{}", made.body);
        assert!(
            made.cookie.as_deref().unwrap().contains("ah_session="),
            "a new owner is signed in"
        );
        let ann = text(&made.body["owner_key"]);
        let city = text(&made.body["city"]["id"]);
        assert!(made.body["city"].get("invite_code").is_none());

        // A new person joins by the link, and so does nobody who has the wrong one.
        let code = code_of(&made.body["invite_url"]);
        assert_eq!(server.get("", "/api/city-invites/nope").await.status, StatusCode::NOT_FOUND);
        let preview = server.get("", &format!("/api/city-invites/{code}")).await;
        assert_eq!(
            preview.body,
            json!({ "name": "Elm", "people": ["Ann"], "rules_text": "", "vouches_needed": 0 })
        );
        let joined = server
            .post("", &format!("/api/city-invites/{code}/join"), json!({ "your_name": "Bo" }))
            .await;
        assert_eq!(joined.status, StatusCode::OK, "{}", joined.body);
        assert_eq!(joined.body["city_id"], city.as_str());
        let bo = text(&joined.body["owner_key"]);

        // Only members see the city, and only its creator sees the invite link.
        let outsider = server
            .post(
                "",
                "/api/cities",
                json!({ "name": "Oak", "your_name": "Eve", "create_code": "sesame" }),
            )
            .await;
        let eve = text(&outsider.body["owner_key"]);
        assert_eq!(
            server.get(&eve, &format!("/api/cities/{city}")).await.status,
            StatusCode::NOT_FOUND
        );
        assert!(server.get(&ann, &format!("/api/cities/{city}")).await.body["city"]["invite_url"]
            .is_string());
        assert!(server.get(&bo, &format!("/api/cities/{city}")).await.body["city"]
            .get("invite_url")
            .is_none());

        // Building a house replies with the house, and `me` lists both places.
        let house = server
            .post(&ann, &format!("/api/cities/{city}/houses"), json!({ "name": "Elm House" }))
            .await;
        assert_eq!(house.status, StatusCode::OK, "{}", house.body);
        let house_id = text(&house.body["house"]["id"]);
        assert!(house.body["living_room_id"].is_string() && house.body["porch_id"].is_string());
        let me = server.get(&ann, "/api/me").await;
        assert_eq!(me.body["cities"][0]["id"], city.as_str());
        assert_eq!(
            me.body["houses"][0],
            json!({ "id": house_id, "name": "Elm House", "city_id": city })
        );
        assert_eq!(
            server.get(&bo, &format!("/api/houses/{house_id}")).await.status,
            StatusCode::NOT_FOUND
        );

        // Joining the house by its link makes Bo a resident, and a second house in the city is refused.
        let house_code = code_of(&house.body["invite_url"]);
        let preview = server.get("", &format!("/api/house-invites/{house_code}")).await;
        assert_eq!(preview.body["city"], "Elm");
        let moved_in =
            server.post(&bo, &format!("/api/house-invites/{house_code}/join"), json!({})).await;
        assert_eq!(moved_in.status, StatusCode::OK, "{}", moved_in.body);
        assert_eq!(moved_in.body["house_id"], house_id.as_str());
        assert_eq!(moved_in.body["city_id"], city.as_str());
        assert!(moved_in.body["owner_key"].is_null());
        assert_eq!(
            server.get(&bo, &format!("/api/houses/{house_id}")).await.status,
            StatusCode::OK
        );
        let second = server
            .post(&bo, &format!("/api/cities/{city}/houses"), json!({ "name": "Again" }))
            .await;
        assert_eq!(second.status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn the_house_bot_has_four_calls_and_everyone_else_is_refused() {
        let server = Server::start(None).await;
        let made =
            server.post("", "/api/cities", json!({ "name": "Elm", "your_name": "Ann" })).await;
        let ann = text(&made.body["owner_key"]);
        let city = text(&made.body["city"]["id"]);
        let house = server
            .post(&ann, &format!("/api/cities/{city}/houses"), json!({ "name": "Elm House" }))
            .await;
        let house_id = text(&house.body["house"]["id"]);

        let bot = server
            .post(&ann, &format!("/api/houses/{house_id}/house-bot"), json!({ "name": "Casa" }))
            .await;
        assert_eq!(bot.status, StatusCode::OK, "{}", bot.body);
        let token = text(&bot.body["token"]);
        assert!(bot.body["mcp_url"].as_str().unwrap().ends_with("/mcp"));
        let second = server
            .post(&ann, &format!("/api/houses/{house_id}/house-bot"), json!({ "name": "Two" }))
            .await;
        assert_eq!(second.status, StatusCode::CONFLICT);

        // A scene a resident approves reaches the queue as an id and a name, nothing else.
        let scene = server
            .post(&ann, &format!("/api/houses/{house_id}/scenes"), json!({ "name": "Movie Night" }))
            .await;
        assert_eq!(scene.status, StatusCode::OK, "{}", scene.body);
        let bad = server
            .post(&ann, &format!("/api/houses/{house_id}/scenes"), json!({ "name": "Unlock Door" }))
            .await;
        assert_eq!(bad.status, StatusCode::BAD_REQUEST);
        let asked = server
            .post(&ann, &format!("/api/scenes/{}/request", text(&scene.body["id"])), json!({}))
            .await;
        assert_eq!(asked.body["status"], "approved", "{}", asked.body);

        let queue = server.get(&token, "/api/house/queue").await;
        let requests = queue.body["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].as_object().unwrap().len(), 2, "{}", queue.body);
        assert_eq!(requests[0]["scene"], "Movie Night");

        assert_eq!(server.get(&ann, "/api/house/queue").await.status, StatusCode::FORBIDDEN);
        assert_eq!(server.get("", "/api/house/queue").await.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            server.post(&ann, "/api/house/status", json!({ "state": "ok" })).await.status,
            StatusCode::FORBIDDEN
        );

        let finished = server
            .post(
                &token,
                &format!("/api/house/queue/{}/finish", text(&requests[0]["id"])),
                json!({ "result": "done" }),
            )
            .await;
        assert_eq!(finished.body, json!({ "ok": true }));
        assert_eq!(server.get(&token, "/api/house/queue").await.body, json!({ "requests": [] }));
        let status =
            server.post(&token, "/api/house/status", json!({ "state": "attention" })).await;
        assert_eq!(status.status, StatusCode::OK);
        let view = server.get(&ann, &format!("/api/houses/{house_id}")).await;
        assert_eq!(view.body["status"]["state"], "attention");
        assert_eq!(view.body["house_bot"]["name"], "Casa");

        // Its `me` keeps working, and everything else it might reach for is refused.
        let me = server.get(&token, "/api/me").await;
        assert_eq!(me.status, StatusCode::OK);
        assert_eq!(me.body["kind"], "agent");
        assert_eq!(me.body["hangouts"], json!([]));
        assert!(me.body["trip"].is_null());
        assert_eq!(server.get(&token, "/api/inbox").await.status, StatusCode::FORBIDDEN);
        assert_eq!(
            server.get(&token, &format!("/api/cities/{city}")).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            server.get(&token, &format!("/api/houses/{house_id}")).await.status,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn a_trip_goes_through_the_routes_and_a_bot_sees_it_in_me() {
        let server = Server::start(None).await;
        let made =
            server.post("", "/api/cities", json!({ "name": "Elm", "your_name": "Ann" })).await;
        let ann = text(&made.body["owner_key"]);
        let city = text(&made.body["city"]["id"]);
        let code = code_of(&made.body["invite_url"]);
        let bo = text(
            &server
                .post("", &format!("/api/city-invites/{code}/join"), json!({ "your_name": "Bo" }))
                .await
                .body["owner_key"],
        );
        let ann_house = server
            .post(&ann, &format!("/api/cities/{city}/houses"), json!({ "name": "Ann House" }))
            .await;
        let bo_house = server
            .post(&bo, &format!("/api/cities/{city}/houses"), json!({ "name": "Bo House" }))
            .await;
        let ann_house_id = text(&ann_house.body["house"]["id"]);
        let bo_living = text(&bo_house.body["living_room_id"]);

        let bot = server
            .post(&bo, &format!("/api/hangouts/{bo_living}/agents"), json!({ "name": "Scout" }))
            .await;
        let scout = text(&bot.body["token"]);
        let scout_id = text(&bot.body["agent"]["id"]);
        let me = server.get(&scout, "/api/me").await;
        assert_eq!(me.body["cities"][0]["id"], city.as_str(), "an HTTP bot finds its city here");
        assert!(me.body["trip"].is_null());

        // The bot asks, and nothing starts until a person on each side has said yes.
        let ask = json!({ "house_id": ann_house_id, "kind": "call", "goal": "Pair on the build", "minutes": 30 });
        let trip = server.post(&scout, "/api/trips", ask).await;
        assert_eq!(trip.status, StatusCode::OK, "{}", trip.body);
        assert_eq!(trip.body["status"], "asking");
        let trip_id = text(&trip.body["id"]);
        assert_eq!(
            server
                .post(&scout, &format!("/api/trips/{trip_id}/answer"), json!({ "response": "yes" }))
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        let half = server
            .post(&bo, &format!("/api/trips/{trip_id}/answer"), json!({ "response": "yes" }))
            .await;
        assert_eq!(half.body["status"], "asking");
        let here = server
            .post(&ann, &format!("/api/trips/{trip_id}/answer"), json!({ "response": "yes" }))
            .await;
        assert_eq!(here.body["status"], "here", "{}", here.body);

        let me = server.get(&scout, "/api/me").await;
        assert_eq!(me.body["trip"]["id"], trip_id.as_str());
        assert_eq!(
            server.get(&scout, &format!("/api/trips/{trip_id}")).await.status,
            StatusCode::OK
        );
        let inbox = server.get(&scout, "/api/inbox").await;
        assert_eq!(inbox.body["your_trips"][0]["id"], trip_id.as_str());

        // Only a resident of the host house extends: the bot's own owner can't.
        assert_eq!(
            server
                .post(&bo, &format!("/api/trips/{trip_id}/extend"), json!({ "minutes": 10 }))
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        let extended = server
            .post(&ann, &format!("/api/trips/{trip_id}/extend"), json!({ "minutes": 10 }))
            .await;
        assert_eq!(extended.status, StatusCode::OK, "{}", extended.body);
        let ann_view = server.get(&ann, &format!("/api/houses/{ann_house_id}")).await;
        assert_eq!(ann_view.body["guests"][0]["agent_name"], "Scout");

        // Home again: `me` has no trip and the bot can no longer see the Porch.
        let home = server.post(&scout, &format!("/api/trips/{trip_id}/end"), json!({})).await;
        assert_eq!(home.body["status"], "ended");
        assert!(server.get(&scout, "/api/me").await.body["trip"].is_null());
        let porch = text(&ann_house.body["porch_id"]);
        assert_eq!(
            server.get(&scout, &format!("/api/hangouts/{porch}")).await.status,
            StatusCode::NOT_FOUND
        );

        // Staying home is only the owner's to set.
        let by_bot = server
            .send(
                &scout,
                Method::PUT,
                &format!("/api/agents/{scout_id}/home"),
                Some(
                    json!({ "house_id": text(&bo_house.body["house"]["id"]), "stays_home": true }),
                ),
            )
            .await;
        assert_eq!(by_bot.status, StatusCode::FORBIDDEN);
        let by_owner = server
            .send(
                &bo,
                Method::PUT,
                &format!("/api/agents/{scout_id}/home"),
                Some(
                    json!({ "house_id": text(&bo_house.body["house"]["id"]), "stays_home": true }),
                ),
            )
            .await;
        assert_eq!(by_owner.body, json!({ "ok": true }), "{}", by_owner.body);
        let refused = server
            .post(&scout, "/api/trips", json!({ "house_id": ann_house_id, "kind": "call" }))
            .await;
        assert_eq!(refused.status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn leaving_a_city_ends_its_open_stream() {
        let server = Server::start(None).await;
        let made =
            server.post("", "/api/cities", json!({ "name": "Elm", "your_name": "Ann" })).await;
        let city = text(&made.body["city"]["id"]);
        let code = code_of(&made.body["invite_url"]);
        let bo = text(
            &server
                .post("", &format!("/api/city-invites/{code}/join"), json!({ "your_name": "Bo" }))
                .await
                .body["owner_key"],
        );
        let eve = text(
            &server
                .post("", "/api/cities", json!({ "name": "Oak", "your_name": "Eve" }))
                .await
                .body["owner_key"],
        );

        let path = format!("/api/cities/{city}/stream");
        assert_eq!(server.get(&eve, &path).await.status, StatusCode::NOT_FOUND);
        let stream = server
            .client
            .get(format!("{}{path}", server.base))
            .bearer_auth(&bo)
            .send()
            .await
            .unwrap();
        assert_eq!(stream.status(), StatusCode::OK);

        let left = server.post(&bo, &format!("/api/cities/{city}/leave"), json!({})).await;
        assert_eq!(left.status, StatusCode::OK, "{}", left.body);

        let mut stream = stream;
        let read = async { while let Some(_chunk) = stream.chunk().await.unwrap() {} };
        tokio::time::timeout(std::time::Duration::from_secs(5), read)
            .await
            .expect("the stream should close once Bo has left");
    }
}
