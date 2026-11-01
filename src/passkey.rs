//! Passkey sign-in: the rules in docs/accounts-and-trust.md (A1 to A13). The browser and the
//! authenticator do the cryptography. This holds each ceremony until it finishes, checks the
//! result with `webauthn-rs`, and decides who is signed in. Handlers in `api` only call these.

use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::{Mutex, MutexGuard},
};

use axum::http::StatusCode;
use serde_json::{json, Value};
use webauthn_rs::prelude::{
    DiscoverableAuthentication, Passkey, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Url, Uuid, Webauthn, WebauthnBuilder, WebauthnError,
};

use crate::{
    auth::{self, Caller},
    db::{NewPasskey, PasskeyAdded, PasskeyInfo, Person},
    error::{ApiError, ApiResult},
    notify,
    ops::{self, MAX_NAME_CHARS},
    AppState,
};

/// A ceremony nobody finishes lapses after this long.
pub const CEREMONY_SECS: i64 = 5 * 60;
/// Most ceremonies waiting at once, across the whole server. Past this the start routes say 429.
pub const MAX_OPEN_CEREMONIES: usize = 2_000;
/// Most one address keeps waiting at once, so one source can't use up the server's.
pub const MAX_OPEN_PER_ADDRESS: usize = 30;
/// New accounts the whole server makes by passkey in an hour.
pub const SIGNUPS_PER_HOUR: usize = 60;
const SIGNUP_WINDOW_SECS: i64 = 60 * 60;
/// Most passkeys one person keeps.
pub const MAX_PASSKEYS: i64 = 20;
const MAX_LABEL_CHARS: usize = 40;
const DEFAULT_LABEL: &str = "Passkey";

/// Passkeys for the host of `public_url`, or `None` if that address can't have them. A passkey
/// belongs to a name, so an IP number gets none (A3). The only origin accepted is `public_url`'s.
pub fn webauthn_for(public_url: &str) -> Option<Webauthn> {
    let url = Url::parse(public_url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    // `domain` is `None` for an IP number.
    let rp_id = url.domain()?;
    let origin = Url::parse(&url.origin().ascii_serialization()).ok()?;
    WebauthnBuilder::new(rp_id, &origin).ok()?.rp_name("Agents Hangout").build().ok()
}

/// A ceremony that has been started and is waiting on the browser. Each says whose it is, so it
/// can only be finished the way it was started.
pub enum Pending {
    /// A signed-in person adding a passkey. Only that person can finish it.
    Add {
        person_id: String,
        label: String,
        registration: PasskeyRegistration,
    },
    /// Someone new. Nothing about them exists until this finishes, and only someone who isn't
    /// signed in can finish it.
    NewAccount {
        name: String,
        label: String,
        user_handle: Uuid,
        registration: PasskeyRegistration,
    },
    Login(DiscoverableAuthentication),
}

/// A ceremony that has been started, with when it lapses and the address that started it.
struct Waiting {
    expires_at: i64,
    from: Option<IpAddr>,
    pending: Pending,
}

/// Ceremonies waiting on a browser, and how many new accounts were made lately. All in memory:
/// a restart costs people a retry and nothing else (A4). Neither field's lock is held while
/// anything slow happens.
#[derive(Default)]
pub struct Ceremonies {
    open: Mutex<HashMap<String, Waiting>>,
    signups: Mutex<VecDeque<i64>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Ceremonies {
    /// Holds a ceremony and returns the id the browser finishes it with. Lapsed ones are cleared
    /// first. `None` means too many are open, in all or from `from`. A caller with no address is
    /// one on this machine, and only the total limits them.
    pub fn begin(&self, pending: Pending, now: i64, from: Option<IpAddr>) -> Option<String> {
        let mut open = locked(&self.open);
        open.retain(|_, waiting| waiting.expires_at > now);
        if open.len() >= MAX_OPEN_CEREMONIES {
            return None;
        }
        if from.is_some()
            && open.values().filter(|waiting| waiting.from == from).count() >= MAX_OPEN_PER_ADDRESS
        {
            return None;
        }
        let id = auth::new_ceremony_id();
        open.insert(id.clone(), Waiting { expires_at: now + CEREMONY_SECS, from, pending });
        Some(id)
    }

    /// Takes a ceremony out. It is gone from here on, found or not and lapsed or not, so it can
    /// never be finished twice (A4).
    pub fn take(&self, id: &str, now: i64) -> Option<Pending> {
        let waiting = locked(&self.open).remove(id)?;
        (waiting.expires_at > now).then_some(waiting.pending)
    }

    pub fn purge(&self, now: i64) {
        locked(&self.open).retain(|_, waiting| waiting.expires_at > now);
    }

    pub fn open_count(&self) -> usize {
        locked(&self.open).len()
    }

    /// Whether the hour's new accounts are used up. A look only: `take_signup` is what counts one.
    pub fn signups_full(&self, now: i64) -> bool {
        let mut signups = locked(&self.signups);
        forget_old_signups(&mut signups, now);
        signups.len() >= SIGNUPS_PER_HOUR
    }

    /// Holds a place in the hour for a new account, if there's room (A11). Held before the account
    /// is made so a crowd arriving at once can't pass the limit together.
    pub fn take_signup(&self, now: i64) -> bool {
        let mut signups = locked(&self.signups);
        forget_old_signups(&mut signups, now);
        if signups.len() >= SIGNUPS_PER_HOUR {
            return false;
        }
        signups.push_back(now);
        true
    }

    /// Gives back a place taken at `now` for an account that wasn't made: only one that is really
    /// made counts (A11).
    pub fn give_back_signup(&self, now: i64) {
        let mut signups = locked(&self.signups);
        if let Some(at) = signups.iter().rposition(|&at| at == now) {
            signups.remove(at);
        }
    }
}

fn forget_old_signups(signups: &mut VecDeque<i64>, now: i64) {
    while signups.front().is_some_and(|&at| at <= now - SIGNUP_WINDOW_SECS) {
        signups.pop_front();
    }
}

/// What a start route sends the page: the ceremony's id, and the options to hand the browser.
pub struct Started {
    pub ceremony_id: String,
    pub options: Value,
}

/// Who a finished registration is for. The key is set only for a new account (A12).
pub struct Registered {
    pub person: Person,
    pub owner_key: Option<String>,
}

// Errors

fn broken(error: impl std::fmt::Debug) -> ApiError {
    tracing::error!(?error, "the passkey library failed");
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "something went wrong")
}

fn off() -> ApiError {
    ApiError::conflict("passkeys aren't available at this address")
}

fn busy() -> ApiError {
    ApiError::too_many_requests(
        "too many passkey steps are open right now; try again in a few minutes",
    )
}

fn too_many_signups() -> ApiError {
    ApiError::too_many_requests("too many new accounts just now; try again later")
}

fn lapsed() -> ApiError {
    ApiError::bad_request("that passkey step has lapsed or was already used; start again")
}

fn unverified() -> ApiError {
    ApiError::bad_request("that passkey couldn't be verified; start again")
}

fn already_registered() -> ApiError {
    ApiError::conflict("that passkey is already registered")
}

fn webauthn(state: &AppState) -> ApiResult<&Webauthn> {
    state.webauthn.as_ref().ok_or_else(off)
}

/// A10: a bot never reaches any of these routes, signed in or not.
pub fn refuse_bots(caller: Option<&Caller>) -> ApiResult<()> {
    if caller.is_some_and(Caller::is_agent) {
        return Err(ApiError::forbidden("bots don't use passkeys"));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() & 1 == 1 || !text.is_ascii() {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok()).collect()
}

// Routes

/// Whether the page should offer passkeys at all.
pub fn available(state: &AppState, caller: Option<&Caller>) -> ApiResult<bool> {
    refuse_bots(caller)?;
    Ok(state.webauthn.is_some())
}

/// Options for `navigator.credentials.create`. The library leaves the credential to the
/// authenticator's discretion, but sign-in with no name typed needs one it keeps (A2), so the
/// request is changed to insist.
fn creation_options(
    challenge: &webauthn_rs::prelude::CreationChallengeResponse,
) -> ApiResult<Value> {
    let mut options = serde_json::to_value(challenge).map_err(broken)?;
    let selection = options
        .pointer_mut("/publicKey/authenticatorSelection")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| broken("no authenticator selection in the options"))?;
    selection.insert("residentKey".into(), json!("required"));
    selection.insert("requireResidentKey".into(), json!(true));
    selection.insert("userVerification".into(), json!("required"));
    Ok(options)
}

/// Starts adding a passkey. Someone signed in adds one to their own account, and nothing in the
/// request can name another. Someone signed out is making a new account, which needs a name now
/// and gets written only when the passkey checks out (A11).
pub fn start_registration(
    state: &AppState,
    caller: Option<&Caller>,
    your_name: Option<&str>,
    label: Option<&str>,
    from: Option<IpAddr>,
    now: i64,
) -> ApiResult<Started> {
    refuse_bots(caller)?;
    let webauthn = webauthn(state)?;
    let label = ops::clean_optional(label, "passkey label", MAX_LABEL_CHARS)?;
    let label = if label.is_empty() { DEFAULT_LABEL.to_string() } else { label };

    let (name, user_handle, exclude) = match caller {
        Some(caller) => {
            if state.db.passkey_count(&caller.person_id)? >= MAX_PASSKEYS {
                return Err(ApiError::conflict(
                    "that's as many passkeys as one person can keep; remove one first",
                ));
            }
            let handle = state.db.user_handle(&caller.person_id, &Uuid::new_v4().to_string())?;
            let handle = Uuid::parse_str(&handle).map_err(broken)?;
            // A2: their own passkeys, so the same one isn't made twice.
            let exclude: Vec<Vec<u8>> = state
                .db
                .credential_ids_of(&caller.person_id)?
                .iter()
                .filter_map(|id| unhex(id))
                .collect();
            (caller.name.clone(), handle, exclude)
        }
        None => {
            let name = ops::clean(your_name.unwrap_or(""), "your name", MAX_NAME_CHARS)?;
            if state.ceremonies.signups_full(now) {
                return Err(too_many_signups());
            }
            (name, Uuid::new_v4(), Vec::new())
        }
    };

    let (challenge, registration) = webauthn
        .start_passkey_registration(
            user_handle,
            &name,
            &name,
            (!exclude.is_empty()).then_some(exclude),
        )
        .map_err(broken)?;
    let options = creation_options(&challenge)?;
    let pending = match caller {
        Some(caller) => Pending::Add { person_id: caller.person_id.clone(), label, registration },
        None => Pending::NewAccount { name, label, user_handle, registration },
    };
    let ceremony_id = state.ceremonies.begin(pending, now, from).ok_or_else(busy)?;
    Ok(Started { ceremony_id, options })
}

/// Finishes a registration the way it was started, or refuses. The ceremony is used up first, so
/// no refusal can be retried.
pub fn finish_registration(
    state: &AppState,
    caller: Option<&Caller>,
    ceremony_id: &str,
    credential: Value,
    now: i64,
) -> ApiResult<Registered> {
    refuse_bots(caller)?;
    let webauthn = webauthn(state)?;
    let pending = state.ceremonies.take(ceremony_id, now).ok_or_else(lapsed)?;

    // A registration belongs to whoever started it: a signed-in person's can't be finished by
    // anyone else, and a new account's can't be finished onto someone who is signed in.
    let (label, registration, owner) = match pending {
        Pending::Login(_) => return Err(lapsed()),
        Pending::Add { person_id, label, registration } => match caller {
            Some(caller) if caller.person_id == person_id => {
                (label, registration, Owner::Existing(person_id))
            }
            _ => return Err(ApiError::forbidden("that passkey was started by someone else")),
        },
        Pending::NewAccount { name, label, user_handle, registration } => {
            if caller.is_some() {
                return Err(ApiError::conflict(
                    "you're signed in: sign out to make a new account, or add a passkey to this one",
                ));
            }
            (label, registration, Owner::New { name, user_handle })
        }
    };

    let credential: RegisterPublicKeyCredential =
        serde_json::from_value(credential).map_err(|_| unverified())?;
    // The browser says whether the passkey is discoverable. It can't be trusted to say yes, but
    // a no is a passkey that could never sign anyone in without a name.
    if credential.extensions.cred_props.as_ref().and_then(|props| props.rk) == Some(false) {
        return Err(ApiError::bad_request(
            "that passkey can't sign you in without a name; use a device that keeps passkeys",
        ));
    }
    let passkey =
        webauthn.finish_passkey_registration(&credential, &registration).map_err(|error| {
            tracing::debug!(?error, "passkey registration refused");
            unverified()
        })?;

    let id = auth::new_id();
    let credential_id = hex(passkey.cred_id());
    let json = serde_json::to_string(&passkey).map_err(broken)?;
    let new = NewPasskey { id: &id, credential_id: &credential_id, passkey: &json, label: &label };
    match owner {
        Owner::Existing(person_id) => {
            match state.db.add_passkey(&person_id, &new, MAX_PASSKEYS, now)? {
                PasskeyAdded::Added => {
                    tell_passkey_added(state, &person_id);
                    Ok(Registered {
                        person: state.db.person(&person_id)?.ok_or_else(ApiError::not_found)?,
                        owner_key: None,
                    })
                }
                PasskeyAdded::AlreadyRegistered => Err(already_registered()),
                PasskeyAdded::TooMany => Err(ApiError::conflict(
                    "that's as many passkeys as one person can keep; remove one first",
                )),
            }
        }
        Owner::New { name, user_handle } => {
            if !state.ceremonies.take_signup(now) {
                return Err(too_many_signups());
            }
            // A12: the same owner key every account has, shown once, for the day every passkey is lost.
            let key = auth::new_owner_key();
            let made = state.db.create_account_with_passkey(
                &auth::new_id(),
                &name,
                &auth::hash_secret(&key),
                &auth::new_feed_token(),
                &user_handle.to_string(),
                &new,
                now,
            );
            if !matches!(made, Ok(Some(_))) {
                state.ceremonies.give_back_signup(now);
            }
            match made? {
                Some(person) => Ok(Registered { person, owner_key: Some(key) }),
                None => Err(already_registered()),
            }
        }
    }
}

/// A9: someone whose account gets a new way in hears of it, in case it wasn't them. People only,
/// not their bots, and the same sentence for everyone: nothing the new passkey was called. It
/// isn't about a room, so the link goes to the home page, where passkeys are listed and removed.
fn tell_passkey_added(state: &AppState, person_id: &str) {
    let text = "A passkey was added to your account. \
                If that wasn't you, remove it and replace your owner key.";
    let notice = notify::Notice {
        event: "account.passkey_added",
        hangout_id: String::new(),
        text: text.to_string(),
    };
    notify::send(state, &[person_id.to_string()], &notice);
}

enum Owner {
    Existing(String),
    New { name: String, user_handle: Uuid },
}

/// Starts a sign-in with no name typed (A1). The page runs it from a button, so the library's
/// request for the browser's autofill prompt is cleared.
pub fn start_login(
    state: &AppState,
    caller: Option<&Caller>,
    from: Option<IpAddr>,
    now: i64,
) -> ApiResult<Started> {
    refuse_bots(caller)?;
    let webauthn = webauthn(state)?;
    let (mut challenge, authentication) =
        webauthn.start_discoverable_authentication().map_err(broken)?;
    challenge.mediation = None;
    let options = serde_json::to_value(&challenge).map_err(broken)?;
    let ceremony_id =
        state.ceremonies.begin(Pending::Login(authentication), now, from).ok_or_else(busy)?;
    Ok(Started { ceremony_id, options })
}

/// Finishes a sign-in. Every way of failing is the same plain 401 (A5): no ceremony, a used or
/// lapsed one, something that isn't a passkey, an unknown credential, a handle that doesn't
/// match, a bad signature, a wrong origin, a cross-origin frame, a counter that went backwards.
pub fn finish_login(
    state: &AppState,
    caller: Option<&Caller>,
    ceremony_id: &str,
    credential: Value,
    now: i64,
) -> ApiResult<Person> {
    refuse_bots(caller)?;
    let webauthn = webauthn(state)?;
    check_login(state, webauthn, ceremony_id, credential, now)?.ok_or_else(ApiError::unauthorized)
}

/// `None` is a refusal. Only a failing database is an error.
fn check_login(
    state: &AppState,
    webauthn: &Webauthn,
    ceremony_id: &str,
    credential: Value,
    now: i64,
) -> ApiResult<Option<Person>> {
    let Some(Pending::Login(authentication)) = state.ceremonies.take(ceremony_id, now) else {
        return Ok(None);
    };
    let Ok(credential) = serde_json::from_value::<PublicKeyCredential>(credential) else {
        return Ok(None);
    };
    // A3: the library refuses a cross-origin frame when a passkey is made, but not at sign-in.
    let client_data: Value =
        serde_json::from_slice(&credential.response.client_data_json).unwrap_or_default();
    if client_data["crossOrigin"] == true {
        return Ok(None);
    }
    let Ok((user_handle, credential_id)) =
        webauthn.identify_discoverable_authentication(&credential)
    else {
        return Ok(None);
    };
    let Some(stored) = state.db.passkey_by_credential(&hex(credential_id))? else {
        return Ok(None);
    };
    // The handle isn't signed, so it proves nothing alone: it only has to agree with whose
    // passkey this is.
    if stored.user_handle != user_handle.to_string() {
        return Ok(None);
    }
    let Ok(mut passkey) = serde_json::from_str::<Passkey>(&stored.passkey) else {
        tracing::error!("a stored passkey could not be read");
        return Ok(None);
    };
    let result =
        match webauthn.finish_discoverable_authentication_v2(&credential, authentication, &passkey)
        {
            Ok(result) => result,
            // A6: the counter only goes up, so one that doesn't is a copy of the passkey.
            Err(WebauthnError::CredentialPossibleCompromise) => {
                tracing::warn!("a passkey sign-in was refused because its counter went backwards");
                return Ok(None);
            }
            Err(error) => {
                tracing::debug!(?error, "passkey sign-in refused");
                return Ok(None);
            }
        };
    let updated = match passkey.update_credential(&result) {
        Some(true) => match serde_json::to_string(&passkey) {
            Ok(json) => Some(json),
            Err(_) => return Ok(None),
        },
        Some(false) => None,
        None => return Ok(None),
    };
    // Only if nothing else has moved this passkey since it was read.
    if !state.db.record_sign_in(&stored.id, &stored.passkey, updated.as_deref(), now)? {
        return Ok(None);
    }
    Ok(state.db.person(&stored.person_id)?)
}

/// The signed-in person's own passkeys, with nothing the library keeps.
pub fn list(state: &AppState, caller: &Caller) -> ApiResult<Vec<PasskeyInfo>> {
    ops::require_human(caller, "keep passkeys")?;
    Ok(state.db.passkeys_of(&caller.person_id)?)
}

/// Removes one of the caller's own passkeys. Anyone else's is not found (A9). Even the last can
/// go, because the owner key still gets them in. Their other devices are signed out with it, so a
/// lost phone loses its session as well as its passkey: only the one that asked stays.
pub fn remove(state: &AppState, caller: &Caller, id: &str) -> ApiResult<()> {
    ops::require_human(caller, "remove a passkey")?;
    if state.db.remove_passkey(&caller.person_id, id, &caller.secret_hash)? {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::Db, Config};

    fn state(public_url: &str) -> AppState {
        AppState::new(
            Db::open(":memory:").unwrap(),
            Config {
                public_url: public_url.to_string(),
                create_code: None,
                allow_private_webhooks: false,
                keeper_id: None,
                god_ids: Vec::new(),
            },
        )
    }

    /// A ceremony to put in the store, which doesn't matter beyond being one.
    fn some_ceremony() -> Pending {
        let webauthn = webauthn_for("https://example.com").unwrap();
        Pending::Login(webauthn.start_discoverable_authentication().unwrap().1)
    }

    #[test]
    fn passkeys_belong_to_a_name_and_never_an_address() {
        assert!(webauthn_for("https://agentshangout.com").is_some());
        assert!(webauthn_for("http://localhost:8787").is_some());
        assert!(webauthn_for("http://127.0.0.1:8787").is_none());
        assert!(webauthn_for("http://[::1]:8787").is_none());
        assert!(webauthn_for("https://203.0.113.9").is_none());
        assert!(webauthn_for("").is_none());
        assert!(webauthn_for("not a url").is_none());
        assert!(webauthn_for("ftp://example.com").is_none());

        let webauthn = webauthn_for("https://agentshangout.com/").unwrap();
        let origins: Vec<_> =
            webauthn.get_allowed_origins().iter().map(|url| url.as_str()).collect();
        assert_eq!(origins, ["https://agentshangout.com/"], "the one origin is the public URL");
    }

    #[test]
    fn a_ceremony_is_used_once() {
        let store = Ceremonies::default();
        let id = store.begin(some_ceremony(), 1_000, None).unwrap();
        assert_eq!(store.open_count(), 1);
        assert!(store.take(&id, 1_001).is_some());
        assert!(store.take(&id, 1_001).is_none(), "the second try finds nothing");
        assert_eq!(store.open_count(), 0);
    }

    #[test]
    fn a_failed_look_still_uses_the_ceremony_up() {
        let store = Ceremonies::default();
        let id = store.begin(some_ceremony(), 1_000, None).unwrap();
        // Lapsed: refused, and gone, so a clock that moves back can't revive it.
        assert!(store.take(&id, 1_000 + CEREMONY_SECS).is_none());
        assert!(store.take(&id, 1_001).is_none());
        assert!(store.take("never-started", 1_001).is_none());
    }

    #[test]
    fn a_ceremony_lapses_after_five_minutes() {
        let store = Ceremonies::default();
        let early = store.begin(some_ceremony(), 1_000, None).unwrap();
        let late = store.begin(some_ceremony(), 1_000, None).unwrap();
        assert!(store.take(&early, 1_000 + CEREMONY_SECS - 1).is_some());
        assert!(store.take(&late, 1_000 + CEREMONY_SECS).is_none());
    }

    #[test]
    fn lapsed_ceremonies_are_cleared_when_another_starts() {
        let store = Ceremonies::default();
        for _ in 0..10 {
            store.begin(some_ceremony(), 1_000, None).unwrap();
        }
        assert_eq!(store.open_count(), 10);
        store.begin(some_ceremony(), 1_000 + CEREMONY_SECS, None).unwrap();
        assert_eq!(store.open_count(), 1, "the ten that lapsed are gone");
        store.purge(1_000 + 2 * CEREMONY_SECS);
        assert_eq!(store.open_count(), 0);
    }

    #[test]
    fn at_most_two_thousand_ceremonies_are_open() {
        let store = Ceremonies::default();
        let pending = || some_ceremony();
        for _ in 0..MAX_OPEN_CEREMONIES {
            assert!(store.begin(pending(), 1_000, None).is_some());
        }
        assert!(store.begin(pending(), 1_000, None).is_none(), "past the cap, nothing starts");
        assert!(
            store.begin(pending(), 1_000 + CEREMONY_SECS, None).is_some(),
            "room again once they lapse"
        );
    }

    #[test]
    fn sixty_new_accounts_an_hour() {
        let store = Ceremonies::default();
        for _ in 0..SIGNUPS_PER_HOUR {
            assert!(!store.signups_full(5_000));
            assert!(store.take_signup(5_000));
        }
        assert!(store.signups_full(5_000));
        assert!(!store.take_signup(5_000 + SIGNUP_WINDOW_SECS - 1));
        assert!(store.take_signup(5_000 + SIGNUP_WINDOW_SECS), "an hour later there is room");
    }

    #[test]
    fn a_place_given_back_is_room_for_another_account() {
        let store = Ceremonies::default();
        for _ in 0..SIGNUPS_PER_HOUR {
            assert!(store.take_signup(5_000));
        }
        assert!(!store.take_signup(5_000));
        store.give_back_signup(5_000);
        assert!(store.take_signup(5_000), "the account that wasn't made left its place free");
        assert!(!store.take_signup(5_000));
        // Giving back one that isn't there changes nothing.
        store.give_back_signup(9_999);
        assert!(store.signups_full(5_000));
    }

    #[test]
    fn thirty_ceremonies_an_address_and_a_local_caller_is_not_counted() {
        let store = Ceremonies::default();
        let here: IpAddr = "203.0.113.7".parse().unwrap();
        let there: IpAddr = "2001:db8::1".parse().unwrap();
        let ids: Vec<String> = (0..MAX_OPEN_PER_ADDRESS)
            .map(|_| store.begin(some_ceremony(), 1_000, Some(here)).unwrap())
            .collect();
        assert!(store.begin(some_ceremony(), 1_000, Some(here)).is_none());
        assert!(
            store.begin(some_ceremony(), 1_000, Some(there)).is_some(),
            "each address is counted on its own"
        );
        for _ in 0..MAX_OPEN_PER_ADDRESS + 5 {
            assert!(store.begin(some_ceremony(), 1_000, None).is_some(), "no address, no limit");
        }
        // Finishing one frees a place for that address, and so does time.
        store.take(&ids[0], 1_001).unwrap();
        assert!(store.begin(some_ceremony(), 1_001, Some(here)).is_some());
        assert!(store.begin(some_ceremony(), 1_001, Some(here)).is_none());
        assert!(store.begin(some_ceremony(), 1_000 + CEREMONY_SECS, Some(here)).is_some());
    }

    #[test]
    fn hex_round_trips_and_refuses_junk() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
        assert_eq!(unhex("000fff"), Some(vec![0, 15, 255]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
        assert_eq!(unhex("é1"), None);
    }

    #[test]
    fn registration_insists_on_a_discoverable_passkey_with_verification() {
        let state = state("http://localhost:8787");
        let started = start_registration(&state, None, Some("Ann"), None, None, 1_000).unwrap();
        let key = &started.options["publicKey"];
        assert_eq!(key["authenticatorSelection"]["residentKey"], "required");
        assert_eq!(key["authenticatorSelection"]["requireResidentKey"], true);
        assert_eq!(key["authenticatorSelection"]["userVerification"], "required");
        assert_eq!(key["rp"]["id"], "localhost");
        assert_eq!(key["user"]["name"], "Ann");
        assert_eq!(key["attestation"], "none");
        assert!(key.get("excludeCredentials").is_none(), "a new account has none to exclude");
    }

    #[test]
    fn a_sign_in_runs_from_a_button_not_the_autofill_prompt() {
        let state = state("http://localhost:8787");
        let started = start_login(&state, None, None, 1_000).unwrap();
        assert!(started.options.get("mediation").is_none());
        assert_eq!(started.options["publicKey"]["allowCredentials"], json!([]));
        assert_eq!(started.options["publicKey"]["userVerification"], "required");
        assert_eq!(started.options["publicKey"]["rpId"], "localhost");
    }

    #[test]
    fn starting_a_new_account_writes_nothing() {
        let state = state("http://localhost:8787");
        start_registration(&state, None, Some("Ann"), Some("Phone"), None, 1_000).unwrap();
        assert_eq!(state.db.person_count().unwrap(), 0);
        assert_eq!(state.ceremonies.open_count(), 1);
    }

    #[test]
    fn a_new_account_needs_a_clean_name_before_anything_starts() {
        let state = state("http://localhost:8787");
        let too_long = "x".repeat(MAX_NAME_CHARS + 1);
        for name in [None, Some(""), Some("   "), Some(too_long.as_str())] {
            let refused = start_registration(&state, None, name, None, None, 1_000).err().unwrap();
            assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{name:?}");
        }
        assert_eq!(state.ceremonies.open_count(), 0);
    }

    #[test]
    fn passkeys_off_means_every_start_says_409() {
        let state = state("http://127.0.0.1:8787");
        assert!(!available(&state, None).unwrap());
        let register =
            start_registration(&state, None, Some("Ann"), None, None, 1_000).err().unwrap();
        let login = start_login(&state, None, None, 1_000).err().unwrap();
        assert_eq!(register.status, StatusCode::CONFLICT);
        assert_eq!(login.status, StatusCode::CONFLICT);
    }
}
