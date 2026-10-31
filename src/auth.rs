//! Credentials. Keys are long random strings shown once; only their SHA-256 hash is stored.
//! A slow password hash isn't needed because these can't be guessed from a wordlist.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, HeaderMap},
};
use sha2::{Digest, Sha256};

use crate::{db::Visit, error::ApiError, now, AppState};

pub const SESSION_COOKIE: &str = "ah_session";

/// How long a sign-in lasts before the person has to do it again.
pub const SESSION_SECS: i64 = 90 * 24 * 60 * 60;

const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

fn random_string(len: usize) -> String {
    (0..len)
        .map(|_| ALPHABET[rand::random_range(0..ALPHABET.len())] as char)
        .collect()
}

/// Short enough for URLs, long enough that guessing one isn't practical.
pub fn new_id() -> String {
    random_string(16)
}

pub fn new_invite_code() -> String {
    random_string(24)
}

pub fn new_feed_token() -> String {
    random_string(40)
}

pub fn new_owner_key() -> String {
    format!("ah_owner_{}", random_string(40))
}

pub fn new_agent_token() -> String {
    format!("ah_bot_{}", random_string(40))
}

/// What a guest in a public world holds. Nothing here ever checks one: `authenticate` doesn't know
/// them, so on every route but a world's they are an unknown token.
pub fn new_guest_token() -> String {
    format!("ah_guest_{}", random_string(40))
}

/// The one-time code a person enters to claim a guest.
pub fn new_claim_code() -> String {
    random_string(20)
}

/// What a signed-in browser holds. Unlike an owner key it can be ended without replacing anything
/// else, and it is the only thing a passkey sign-in can hand out.
pub fn new_session_token() -> String {
    format!("ah_sess_{}", random_string(40))
}

/// Names a passkey ceremony that is waiting on the browser. Only the browser that started it knows it.
pub fn new_ceremony_id() -> String {
    random_string(32)
}

pub fn hash_secret(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whoever is making this request: a human or one of their bots.
#[derive(Clone, Debug)]
pub struct Caller {
    pub person_id: String,
    pub agent_id: Option<String>,
    /// The display name written next to anything this caller says.
    pub name: String,
    /// The hash of whatever this caller presented: a bot token, an owner key or a session token.
    /// An open stream hands it back to `authenticate` to check it again, and nothing else may
    /// assume which of the three it is.
    pub secret_hash: String,
    /// The bot that runs a house's scenes. It is in no rooms and has four calls of its own.
    pub is_house: bool,
}

impl Caller {
    pub fn is_agent(&self) -> bool {
        self.agent_id.is_some()
    }
}

impl AppState {
    /// Who a credential belongs to: a bot's token, then a person's owner key, then a session that
    /// hasn't lapsed. Checking it again later is how a replaced key or an ended session also
    /// ends the streams it had open.
    pub fn authenticate(&self, secret_hash: &str) -> Result<Option<Caller>, ApiError> {
        let now = now();
        if let Some(agent) = self.db.agent_by_token(secret_hash, now)? {
            let is_house = self.db.bot_home(&agent.id)?.is_some_and(|home| home.is_house);
            return Ok(Some(Caller {
                person_id: agent.owner_id,
                agent_id: Some(agent.id),
                name: agent.name,
                secret_hash: secret_hash.to_string(),
                is_house,
            }));
        }
        let person = match self.db.person_by_key(secret_hash)? {
            Some(person) => Some(person),
            None => self.db.person_by_session(secret_hash, now)?,
        };
        Ok(person.map(|person| Caller {
            person_id: person.id,
            agent_id: None,
            name: person.name,
            secret_hash: secret_hash.to_string(),
            is_house: false,
        }))
    }

    /// Signs a person in on one device. Returns the token for its cookie, which is shown nowhere
    /// else: only its hash is kept.
    pub fn start_session(&self, person_id: &str) -> Result<String, ApiError> {
        let token = new_session_token();
        let now = now();
        self.db
            .create_session(&hash_secret(&token), person_id, now + SESSION_SECS, now)?;
        Ok(token)
    }

    /// Ends the sessions this request carries, whether in its cookie or as a bearer token. An
    /// owner key or a bot token matches no session, so it is left alone.
    pub fn end_presented_sessions(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        for secret in [bearer_token(headers), session_cookie(headers)].into_iter().flatten() {
            self.db.delete_session(&hash_secret(secret))?;
        }
        Ok(())
    }

    /// Whether this caller may be in this hangout right now. A bot needs its own place in the
    /// room: its owner being a member doesn't let it in. A person who only owns a visiting bot is
    /// let in too, to watch, so this is true for guests: actions a guest mustn't take check
    /// `guest_visit` as well (see `ops::no_guests`).
    pub fn in_room(&self, caller: &Caller, hangout_id: &str) -> Result<bool, ApiError> {
        if caller.is_house {
            return Ok(false);
        }
        Ok(match &caller.agent_id {
            Some(agent_id) => self.db.agent_in_room(hangout_id, agent_id, now())?,
            None => {
                self.db.is_member(hangout_id, &caller.person_id)?
                    || self.db.person_visit(hangout_id, &caller.person_id, now())?.is_some()
            }
        })
    }

    /// When this caller's visit here began and ends, if they're only a guest: a bot on a trip to
    /// this Porch, or a person whose bot is. Members of the room are never guests.
    pub fn guest_visit(
        &self,
        caller: &Caller,
        hangout_id: &str,
    ) -> Result<Option<Visit>, ApiError> {
        let now = now();
        Ok(match &caller.agent_id {
            Some(agent_id) => self.db.bot_visit(hangout_id, agent_id, now)?,
            None => self.db.person_visit(hangout_id, &caller.person_id, now)?,
        })
    }
}

/// The one address a header that can't be read is counted as. No client has it, so everything that
/// can't be read shares one count and is limited like any other address.
pub const UNKNOWN_CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

/// Where a request really came from, for limits per address. The server listens on localhost behind
/// Caddy, which appends the client's address to `X-Forwarded-For`, so the last one is Caddy's word
/// and every earlier one is whatever the client said. No header means a caller on this machine,
/// who isn't limited. A header with no readable address in it is `UNKNOWN_CLIENT`, never free,
/// because only a caller that isn't behind the proxy can send one. An IPv6 address counts as its
/// /64, since one network hands out 2^64 of them, and an IPv4-mapped one as the IPv4 address.
pub fn client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    let last = headers.get_all("x-forwarded-for").iter().next_back()?;
    let address = last
        .to_str()
        .ok()
        .and_then(|line| line.rsplit(',').next()?.trim().parse().ok());
    Some(address.map_or(UNKNOWN_CLIENT, network_of))
}

fn network_of(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !(u128::MAX >> 64))),
        },
        v4 => v4,
    }
}

pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

pub(crate) fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value)
}

/// Who a request's credential belongs to, touching nothing.
pub(crate) fn identify(headers: &HeaderMap, state: &AppState) -> Result<Caller, ApiError> {
    let secret = bearer_token(headers)
        .or_else(|| session_cookie(headers))
        .ok_or_else(ApiError::unauthorized)?;
    state
        .authenticate(&hash_secret(secret))?
        .ok_or_else(ApiError::unauthorized)
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let caller = identify(&parts.headers, state)?;
        if let Some(agent_id) = &caller.agent_id {
            state.db.touch_agent_seen(agent_id, now())?;
        }
        Ok(caller)
    }
}

/// Like `Caller`, for routes that also accept people who haven't signed in yet.
pub struct MaybeCaller(pub Option<Caller>);

impl FromRequestParts<AppState> for MaybeCaller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match Caller::from_request_parts(parts, state).await {
            Ok(caller) => Ok(Self(Some(caller))),
            Err(error) if error.status == axum::http::StatusCode::UNAUTHORIZED => Ok(Self(None)),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn forwarded(lines: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for line in lines {
            headers.append("x-forwarded-for", HeaderValue::from_str(line).unwrap());
        }
        headers
    }

    fn ip(text: &str) -> Option<IpAddr> {
        Some(text.parse().unwrap())
    }

    #[test]
    fn the_client_is_the_last_address_because_the_proxy_appends_it() {
        assert_eq!(client_ip(&forwarded(&["203.0.113.7"])), ip("203.0.113.7"));
        assert_eq!(client_ip(&forwarded(&["2001:db8::1"])), ip("2001:db8::"), "an IPv6 address is its /64");
        // Anything before it is what the client claimed.
        assert_eq!(client_ip(&forwarded(&["10.0.0.1, 198.51.100.2 ,203.0.113.7"])), ip("203.0.113.7"));
        assert_eq!(client_ip(&forwarded(&["10.0.0.1", "203.0.113.7"])), ip("203.0.113.7"));
    }

    #[test]
    fn no_header_is_this_machine_and_a_header_with_no_address_in_it_is_one_shared_client() {
        assert_eq!(client_ip(&HeaderMap::new()), None);
        for junk in ["", "unknown", "203.0.113.7,", "203.0.113.7, nope", "203.0.113.7:443", "[2001:db8::1]"] {
            assert_eq!(client_ip(&forwarded(&[junk])), Some(UNKNOWN_CLIENT), "{junk:?}");
        }
        // A value that is not even text is no better, and neither is a good line before a bad one.
        let mut headers = HeaderMap::new();
        headers.append("x-forwarded-for", HeaderValue::from_bytes(b"203.0.113.7\xff").unwrap());
        assert_eq!(client_ip(&headers), Some(UNKNOWN_CLIENT));
        assert_eq!(client_ip(&forwarded(&["203.0.113.7", "nope"])), Some(UNKNOWN_CLIENT));
    }

    #[test]
    fn an_ipv6_address_counts_as_its_64_and_a_mapped_ipv4_one_as_the_ipv4_address() {
        let same = [
            "2001:db8:abcd:1234::1",
            "2001:DB8:abcd:1234:ffff:ffff:ffff:ffff",
            "2001:db8:abcd:1234:0:0:0:0",
            "10.0.0.1, 2001:db8:abcd:1234:1:2:3:4",
        ];
        for line in same {
            assert_eq!(client_ip(&forwarded(&[line])), ip("2001:db8:abcd:1234::"), "{line}");
        }
        assert_ne!(client_ip(&forwarded(&["2001:db8:abcd:1235::1"])), ip("2001:db8:abcd:1234::"));
        for line in ["::ffff:203.0.113.7", "::FFFF:cb00:7107", "0:0:0:0:0:ffff:203.0.113.7"] {
            assert_eq!(client_ip(&forwarded(&[line])), ip("203.0.113.7"), "{line}");
        }
        // A real client is never the shared one for addresses that can't be read.
        assert_ne!(client_ip(&forwarded(&["203.0.113.7"])), Some(UNKNOWN_CLIENT));
    }
}
