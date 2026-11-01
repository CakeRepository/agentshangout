//! Friends, and inviting them by name (docs/friends.md). Who your friends are is worked out on read
//! from the rooms and cities you share, so nobody keeps a list. An invite only points someone at a
//! place's normal invite page, so everything that page asks of them still applies.

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    auth::{self, Caller},
    city,
    db::Person,
    error::{ApiError, ApiResult},
    now, ops, AppState,
};

/// FR6: how long an invite waits to be opened.
pub const INVITE_SECS: i64 = 14 * 86_400;
/// FR7: how many invites one person can send in a day.
pub const INVITES_PER_DAY: i64 = 20;
const DAY_SECS: i64 = 86_400;
const MAX_FRIENDS: i64 = 200;
const MAX_INVITED: i64 = 50;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/friends", get(friends))
        .route("/api/hangouts/{id}/invite-friend", post(invite_to_room))
        .route("/api/cities/{id}/invite-friend", post(invite_to_city))
        .route("/api/friend-invites/{id}/decline", post(decline))
}

#[derive(Deserialize)]
struct InviteFriend {
    person_id: String,
}

/// An invite waiting for someone, as `/api/me` shows it.
#[derive(Serialize)]
pub struct Invited {
    id: String,
    kind: String,
    from: String,
    name: String,
    /// The place's normal invite page.
    path: String,
}

/// FR5: the invites waiting for this person, with today's link for each place.
pub fn invited(state: &AppState, person_id: &str) -> ApiResult<Vec<Invited>> {
    let waiting = state.db.friend_invites_for(person_id, now() - INVITE_SECS, MAX_INVITED)?;
    Ok(waiting
        .into_iter()
        .map(|invite| Invited {
            path: match invite.kind.as_str() {
                "city" => format!("/join-city/{}", invite.code),
                _ => format!("/join/{}", invite.code),
            },
            id: invite.id,
            kind: invite.kind,
            from: invite.from_name,
            name: invite.place_name,
        })
        .collect())
}

/// FR2: bots don't get a list of people, because agents never reach past a room (F22).
async fn friends(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "see a list of friends")?;
    let friends = state.db.friends_of(&caller.person_id, MAX_FRIENDS)?;
    Ok(Json(json!({ "friends": friends })))
}

/// FR3: the friend, or the same 404 for a stranger as for nobody at all, so ids can't be tried out.
fn friend_of(state: &AppState, caller: &Caller, person_id: &str) -> ApiResult<Person> {
    if !state.db.is_friend(&caller.person_id, person_id)? {
        return Err(ApiError::not_found());
    }
    state.db.person(person_id)?.ok_or_else(ApiError::not_found)
}

fn body_of(body: Result<Json<InviteFriend>, JsonRejection>) -> ApiResult<InviteFriend> {
    body.map(|Json(body)| body)
        .map_err(|_| ApiError::bad_request("say which friend with person_id"))
}

fn already_in() -> ApiError {
    ApiError::conflict("they're already in")
}

/// FR3: anyone who can see a room's link can invite a friend to it.
async fn invite_to_room(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    body: Result<Json<InviteFriend>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = body_of(body)?;
    ops::require_human(&caller, "invite a friend")?;
    let room = ops::no_guests(&state, &caller, &id, "invite anyone")?;
    if state.db.is_managed_room(&id)? {
        return Err(ApiError::forbidden("people come into this room through its house or city"));
    }
    let friend = friend_of(&state, &caller, &body.person_id)?;
    if state.db.is_member(&id, &friend.id)? {
        return Err(already_in());
    }
    send(&state, &caller, "room", &id, &room.name, &friend, &id)
}

/// FR3: only whoever looks after a city's link can invite a friend to it.
async fn invite_to_city(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    body: Result<Json<InviteFriend>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = body_of(body)?;
    ops::require_human(&caller, "invite a friend")?;
    let place = city::require_city(&state, &caller, &id)?;
    if !city::holds_city_link(&state, &caller, &place)? {
        return Err(ApiError::forbidden("only the person who started the city can invite people"));
    }
    let friend = friend_of(&state, &caller, &body.person_id)?;
    if state.db.is_city_member(&id, &friend.id)? {
        return Err(already_in());
    }
    send(&state, &caller, "city", &id, &place.name, &friend, &place.square_id)
}

/// FR6, FR7: asking again changes nothing, so the answer never shows whether someone said no.
fn send(
    state: &AppState,
    caller: &Caller,
    kind: &str,
    place_id: &str,
    place_name: &str,
    friend: &Person,
    ping_room: &str,
) -> ApiResult<Json<Value>> {
    let now = now();
    if state.db.friend_invites_sent_since(&caller.person_id, now - DAY_SECS)? >= INVITES_PER_DAY {
        return Err(ApiError::too_many_requests(
            "that's a lot of invites for one day: try again tomorrow",
        ));
    }
    let id = auth::new_id();
    let fresh = state.db.add_friend_invite(
        &id,
        kind,
        place_id,
        &caller.person_id,
        &friend.id,
        now,
        now - INVITE_SECS,
    )?;
    if fresh {
        let text = format!("{} invited you to {place_name}.", caller.name);
        city::ping(state, vec![friend.id.clone()], "friend.invite", ping_room, &text);
    }
    Ok(Json(json!({ "ok": true })))
}

/// FR6: no thanks. It stays that way, and only the person invited can say it.
async fn decline(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "turn down an invite")?;
    if !state.db.decline_friend_invite(&id, &caller.person_id, now())? {
        return Err(ApiError::not_found());
    }
    Ok(Json(json!({ "ok": true })))
}
