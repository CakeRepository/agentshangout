//! The public city (docs/public-city.md): The Commons, the worlds people start, the players who
//! build in them, the step that grows them, and the frogs and visitors that pass through. Every
//! rule (W1-W51) lives here. The engine in `world` is pure, `db` only stores, and the routes and
//! the MCP tools are thin callers of what is below.
//!
//! Nothing in here touches a hangout, city, house, trip or scene (W1). It reads `people` to know
//! who is signed in, and `agents` and `bot_homes` to turn away a bot that stays home (W3).
//!
//! Every word a world sends back is ours: a fixed message, a generated name or a listed word. No
//! reply echoes anything a caller sent (W8), so a refusal never says what was wrong with the
//! value, only which kind of value it was.

use std::{
    collections::{hash_map::RandomState, BTreeMap, HashMap, VecDeque},
    hash::BuildHasher,
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

use axum::{
    body::Bytes,
    extract::{FromRequestParts, Request, State},
    http::{header, request::Parts, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    auth::{self, Caller},
    db::{
        random_name_seed, BotJoined, Claimed, NewWorld, PlayerKind, PlayerRow, WorldAdded, WorldRow,
        WorldTx,
    },
    error::{ApiError, ApiResult},
    now,
    world::{self, Change, Ground, Kind, Land, Roads, Span, Tiles},
    AppState,
};

// What the rules say

/// The Commons has this id for good, so a link to it never changes.
pub const COMMONS_ID: &str = "commons";
pub const COMMONS_NAME: &str = "The Commons";

/// W9: moves in a day, by who is playing. They reset at midnight UTC.
pub const GUEST_MOVES: u32 = 30;
pub const BOT_MOVES: u32 = 200;
pub const PERSON_MOVES: u32 = 300;

/// W10: a guest nobody has claimed stops working after this long without being seen.
pub const GUEST_IDLE_SECS: i64 = 7 * 24 * 60 * 60;
/// Guests that have been idle this long are deleted, so a swarm of them can't fill the table.
const PRUNE_AFTER_SECS: i64 = 30 * 24 * 60 * 60;
/// W24: a world with no move in this long stops stepping, and a move wakes it.
pub const IDLE_WORLD_SECS: i64 = 30 * 24 * 60 * 60;
/// A player is on a world's street if they moved there in the last ten minutes.
pub const PLAYER_ACTIVE_SECS: i64 = 10 * 60;
/// W31: a world view lists this many players at most.
pub const MAX_LISTED_PLAYERS: i64 = 64;
/// W25: the server holds this many worlds of people's own.
pub const MAX_OWN_WORLDS: i64 = 200;
/// W11: a person can claim this many guests.
pub const MAX_CLAIMS: i64 = 20;
/// W56: buildings a person can have beside their house, and all the buildings The Commons holds.
pub const MAX_THIRD_SPACES: i64 = 3;
pub const MAX_BUILDINGS: i64 = 1_500;
/// A walk goes at most this many tiles.
pub const WALK_RANGE: i64 = 16;
/// W31: `look` shows at most this many tiles across and down.
pub const LOOK_MAX: usize = 32;
const LOOK_DEFAULT: usize = 24;
/// A seed is a whole number every browser can hold exactly.
pub const MAX_SEED: u64 = (1 << 53) - 1;
const PREVIEW_SEEDS: usize = 4;

/// W12, W30: how many may come in.
const GUESTS_PER_IP_PER_HOUR: u32 = 5;
const GUESTS_PER_HOUR: u32 = 600;
/// W30: no world can use more than its share of the server's moves, so one busy world can't use up
/// The Commons.
const MOVES_PER_WORLD_PER_MINUTE: u32 = 300;
const MOVES_PER_MINUTE: u32 = 600;
/// W30: views come from memory, so an office behind one address has room to poll.
const VIEWS_PER_IP_PER_MINUTE: u32 = 240;
/// W30: a refused move costs its player nothing but costs the server a read, so each player and
/// each address may have this many a minute.
const REFUSED_PER_MINUTE: u32 = 60;
/// Claims, rerolls, previews, link attempts and starting a world, per person or player.
const PERSON_ACTIONS_PER_MINUTE: u32 = 30;

/// W36, W42, W44: how long a frog or a visitor is shown, and how many visitors at once.
pub const CRAWLER_SHOW_SECS: i64 = 5 * 60;
pub const VISITOR_SECS: i64 = 10 * 60;
pub const MAX_VISITORS: usize = 30;
/// W52: how long a person stays on the street after their page last looked, which is longer than
/// the slowest a page looks, and how many people show at once.
pub const PERSON_SECS: i64 = 90;
pub const MAX_PEOPLE: usize = 60;
/// W39: each kind's count reaches the database at most this often.
const SIGHTING_FLUSH_SECS: i64 = 60;

/// W29: a world view is rebuilt at most this often when something moved, and at least this often
/// otherwise, so people who have gone from the street leave it.
const REBUILD_AFTER_CHANGE: Duration = Duration::from_secs(1);
const REBUILD_AT_LEAST: Duration = Duration::from_secs(5);
const CARDS_AFTER: Duration = Duration::from_secs(5);
const LISTED_WORLDS: i64 = 50;

/// W13: the emotes there are.
pub const EMOTES: [&str; 5] = ["wave", "cheer", "think", "sleep", "dance"];

/// Said to anyone refused for a reason that is theirs to change.
const BAD_MOVE: &str = "that isn't a move the world understands";

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn today(now: i64) -> i64 {
    now.div_euclid(86_400)
}

// Limits

/// Counts requests per key in a window, with a fixed amount of memory: a key lands in one of a
/// fixed number of counters, so a flood of addresses can't grow it, only share counters. A key that
/// shares with a busy one may be refused early, which the sizes make rare.
pub(crate) struct Limiter {
    hasher: RandomState,
    window: i64,
    max: u32,
    counters: Mutex<Vec<(i64, u32)>>,
}

impl Limiter {
    pub(crate) fn new(counters: usize, window: i64, max: u32) -> Limiter {
        Limiter {
            hasher: RandomState::new(),
            window,
            max,
            counters: Mutex::new(vec![(i64::MIN, 0); counters.max(1)]),
        }
    }

    /// Counts a request, and says whether it is within the limit.
    fn allow(&self, key: &str, now: i64) -> bool {
        let window = now.div_euclid(self.window);
        let mut counters = locked(&self.counters);
        let slot = (self.hasher.hash_one(key) % counters.len() as u64) as usize;
        let counter = &mut counters[slot];
        if counter.0 != window {
            *counter = (window, 0);
        }
        counter.1 = counter.1.saturating_add(1);
        counter.1 <= self.max
    }

    /// Counts a request like `allow`, and refuses it with how long to wait if it is over.
    pub(crate) fn take(&self, key: &str, now: i64, message: &'static str) -> ApiResult<()> {
        if self.allow(key, now) {
            return Ok(());
        }
        Err(ApiError::too_many_requests(message).retry_after(self.wait(now)))
    }

    /// Whether a key has already used up its window. Counts nothing.
    fn spent(&self, key: &str, now: i64) -> bool {
        let window = now.div_euclid(self.window);
        let counters = locked(&self.counters);
        let counter = counters[(self.hasher.hash_one(key) % counters.len() as u64) as usize];
        counter.0 == window && counter.1 >= self.max
    }

    /// Seconds until the window that `now` is in ends.
    fn wait(&self, now: i64) -> u64 {
        (self.window - now.rem_euclid(self.window)) as u64
    }
}

/// Moves counted for each world in a minute. Only worlds that exist are named, so it never holds
/// more than there are worlds, and no two worlds share a count.
struct WorldMoves {
    counts: Mutex<(i64, HashMap<String, u32>)>,
}

impl WorldMoves {
    fn new() -> WorldMoves {
        WorldMoves { counts: Mutex::new((i64::MIN, HashMap::new())) }
    }

    /// Counts a move in a world, and says whether the world is within its share.
    fn allow(&self, world_id: &str, now: i64) -> bool {
        let window = now.div_euclid(60);
        let mut counts = locked(&self.counts);
        if counts.0 != window {
            *counts = (window, HashMap::new());
        }
        let count = counts.1.entry(world_id.to_string()).or_insert(0);
        *count = count.saturating_add(1);
        *count <= MOVES_PER_WORLD_PER_MINUTE
    }
}

struct Limits {
    guests_per_ip: Limiter,
    guests: Limiter,
    moves: Limiter,
    world_moves: WorldMoves,
    /// Refused moves, counted under `player:<id>` and `address:<ip>`.
    refused: Limiter,
    views_per_ip: Limiter,
    people: Limiter,
}

impl Limits {
    fn new() -> Limits {
        Limits {
            guests_per_ip: Limiter::new(4096, 3600, GUESTS_PER_IP_PER_HOUR),
            guests: Limiter::new(1, 3600, GUESTS_PER_HOUR),
            moves: Limiter::new(1, 60, MOVES_PER_MINUTE),
            world_moves: WorldMoves::new(),
            refused: Limiter::new(8192, 60, REFUSED_PER_MINUTE),
            views_per_ip: Limiter::new(8192, 60, VIEWS_PER_IP_PER_MINUTE),
            people: Limiter::new(4096, 60, PERSON_ACTIONS_PER_MINUTE),
        }
    }
}

// What the server keeps in memory

/// A world's land and its rows as the page reads them. It never changes, so it is made once.
pub struct WorldLand {
    pub land: Land,
    pub rows: Vec<String>,
}

/// What `look` and `how_am_i_doing` need to know about a world without loading it.
#[derive(Clone, Debug)]
pub struct Summary {
    pub name: String,
    pub size: usize,
    pub step: u64,
    pub population: u32,
    pub jobs: u32,
    pub parks: u32,
    pub stage: &'static str,
}

/// A reply built once and served to everyone who asks until it is rebuilt.
pub struct Cached {
    pub body: Bytes,
    pub etag: String,
    pub summary: Summary,
    /// Every tile of the world as it was built, for what needs the whole world (a tip) without
    /// reading it from the database. Empty for the list of worlds.
    pub tiles: Arc<Tiles>,
}

struct Slot {
    /// Set by a move, a step or anyone arriving, so the next view is rebuilt as soon as it may be.
    stale: AtomicBool,
    inner: Mutex<SlotInner>,
}

#[derive(Default)]
struct SlotInner {
    built: Option<Instant>,
    cached: Option<Arc<Cached>>,
}

struct Visitor {
    key: u64,
    n: u64,
    arrived: i64,
}

struct Person {
    key: u64,
    n: u64,
    looked: i64,
}

#[derive(Default)]
struct Seen {
    last: Option<i64>,
    pending: u32,
    flushed: i64,
}

/// W36, W42, W52: who is passing through. Memory only: a restart empties it.
#[derive(Default)]
struct Walkers {
    visitors: VecDeque<Visitor>,
    people: VecDeque<Person>,
    seen: HashMap<&'static str, Seen>,
}

/// Everything the worlds keep in memory. It is all rebuilt from the database or empty after a restart.
pub struct Runtime {
    commons_ready: AtomicBool,
    lands: Mutex<HashMap<(u64, usize), Arc<WorldLand>>>,
    views: Mutex<HashMap<String, Arc<Slot>>>,
    cards: Mutex<Option<(Instant, Arc<Cached>)>>,
    cards_stale: AtomicBool,
    walkers: Mutex<Walkers>,
    /// W42: mixed into the hash of a visitor's address. Random each start, and kept nowhere.
    salt: [u8; 16],
    limits: Limits,
    builds: AtomicU64,
    last_prune: AtomicI64,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    pub fn new() -> Runtime {
        Runtime {
            commons_ready: AtomicBool::new(false),
            lands: Mutex::new(HashMap::new()),
            views: Mutex::new(HashMap::new()),
            cards: Mutex::new(None),
            cards_stale: AtomicBool::new(false),
            walkers: Mutex::new(Walkers::default()),
            salt: rand::random(),
            limits: Limits::new(),
            builds: AtomicU64::new(0),
            last_prune: AtomicI64::new(0),
        }
    }

    /// The land for a world, made the first time it is asked for. Only worlds that exist are
    /// asked for, so there are never more than there are worlds.
    pub fn land_for(&self, seed: u64, size: usize) -> Arc<WorldLand> {
        let mut lands = locked(&self.lands);
        lands
            .entry((seed, size))
            .or_insert_with(|| {
                let land = world::terrain(seed, size);
                let rows = world::encode_land(&land);
                Arc::new(WorldLand { land, rows })
            })
            .clone()
    }

    /// Keeps land that was made for a world which has just been added, so it isn't made again.
    /// Land made for a seed that didn't become a world is never kept (W25).
    fn keep_land(&self, seed: u64, size: usize, land: Land) {
        locked(&self.lands).entry((seed, size)).or_insert_with(|| {
            let rows = world::encode_land(&land);
            Arc::new(WorldLand { land, rows })
        });
    }

    /// Something in a world changed, so its view is rebuilt the next time it may be.
    pub fn mark_stale(&self, world_id: &str) {
        if let Some(slot) = locked(&self.views).get(world_id) {
            slot.stale.store(true, Ordering::Release);
        }
    }

    pub fn mark_everything_stale(&self) {
        for slot in locked(&self.views).values() {
            slot.stale.store(true, Ordering::Release);
        }
        self.cards_stale.store(true, Ordering::Release);
    }

    /// How many world views have been built since the server started. A thousand people watching
    /// should add about one a second per world, not a thousand.
    pub fn view_builds(&self) -> u64 {
        self.builds.load(Ordering::Relaxed)
    }

    // Visitors and frogs

    /// W42, W44: a visitor arrives, or is already on the street. True if it is new.
    fn note_visitor(&self, key: u64, now: i64) -> bool {
        let mut walkers = locked(&self.walkers);
        walkers.visitors.retain(|visitor| now < visitor.arrived + VISITOR_SECS);
        if walkers.visitors.iter().any(|visitor| visitor.key == key) {
            return false;
        }
        if walkers.visitors.len() >= MAX_VISITORS {
            walkers.visitors.pop_front();
        }
        walkers.visitors.push_back(Visitor {
            key,
            n: rand::random(),
            arrived: now,
        });
        true
    }

    /// W52: someone's page looked at The Commons, so they are on the street. True if they are new.
    fn note_person(&self, key: u64, now: i64) -> bool {
        let mut walkers = locked(&self.walkers);
        walkers.people.retain(|person| now < person.looked + PERSON_SECS);
        if let Some(person) = walkers.people.iter_mut().find(|person| person.key == key) {
            person.looked = now;
            return false;
        }
        if walkers.people.len() >= MAX_PEOPLE {
            walkers.people.pop_front();
        }
        walkers.people.push_back(Person {
            key,
            n: rand::random(),
            looked: now,
        });
        true
    }

    /// W36, W39: a listed agent was seen. A crawler's frog is shown for a while from now (`frog`).
    /// Returns whether a frog appeared that wasn't on the street, and how many sightings are
    /// due to be written to the database now, if it has been long enough since the last.
    fn note_agent(&self, id: &'static str, frog: bool, now: i64) -> (bool, Option<u32>) {
        let mut walkers = locked(&self.walkers);
        let seen = walkers.seen.entry(id).or_default();
        let appeared = frog && !seen.last.is_some_and(|last| now - last < CRAWLER_SHOW_SECS);
        if frog {
            seen.last = Some(now);
        }
        seen.pending = seen.pending.saturating_add(1);
        (appeared, take_due(seen, now))
    }

    /// Sightings that have waited long enough to be written.
    fn due_sightings(&self, now: i64) -> Vec<(&'static str, u32)> {
        let mut walkers = locked(&self.walkers);
        walkers
            .seen
            .iter_mut()
            .filter_map(|(id, seen)| take_due(seen, now).map(|visits| (*id, visits)))
            .collect()
    }

    /// A write failed, so its sightings go back to be counted with the next.
    fn return_sightings(&self, id: &'static str, visits: u32) {
        let mut walkers = locked(&self.walkers);
        let seen = walkers.seen.entry(id).or_default();
        seen.pending = seen.pending.saturating_add(visits);
    }

    /// The crawlers on the street, in the order of the list, with when each was last seen.
    pub fn frogs(&self, now: i64) -> Vec<(&'static world::Crawler, i64)> {
        let walkers = locked(&self.walkers);
        world::CRAWLERS
            .iter()
            .filter_map(|crawler| {
                let last = walkers.seen.get(crawler.id).and_then(|seen| seen.last)?;
                (now - last < CRAWLER_SHOW_SECS).then_some((crawler, last))
            })
            .collect()
    }

    /// The numbers of the visitors on the street, oldest first.
    pub fn visitors(&self, now: i64) -> Vec<u64> {
        let walkers = locked(&self.walkers);
        walkers
            .visitors
            .iter()
            .filter(|visitor| now < visitor.arrived + VISITOR_SECS)
            .map(|visitor| visitor.n)
            .collect()
    }

    /// The numbers of the people on the street, oldest first.
    pub fn people(&self, now: i64) -> Vec<u64> {
        let walkers = locked(&self.walkers);
        walkers
            .people
            .iter()
            .filter(|person| now < person.looked + PERSON_SECS)
            .map(|person| person.n)
            .collect()
    }

    /// W42: the address alone, so an agent that changes what it calls itself is still one visitor.
    fn visitor_key(&self, address: &str) -> u64 {
        let digest = Sha256::new()
            .chain_update(self.salt)
            .chain_update(address.as_bytes())
            .finalize();
        u64::from_be_bytes(digest[..8].try_into().unwrap_or([0; 8]))
    }
}

/// Takes a kind's waiting sightings if it is time to write them.
fn take_due(seen: &mut Seen, now: i64) -> Option<u32> {
    if seen.pending == 0 || now - seen.flushed < SIGHTING_FLUSH_SECS {
        return None;
    }
    seen.flushed = now;
    Some(std::mem::take(&mut seen.pending))
}

// Who is playing

pub fn allowance(player: &PlayerRow) -> u32 {
    match player.kind() {
        PlayerKind::Guest => GUEST_MOVES,
        PlayerKind::Claimed | PlayerKind::Bot => BOT_MOVES,
        PlayerKind::Person => PERSON_MOVES,
    }
}

/// W9: a day never goes backwards. A request that read the clock before midnight and reached the
/// database after it must not turn a day already spent into a fresh one.
fn day_of(player: &PlayerRow, now: i64) -> i64 {
    player.moves_day.max(today(now))
}

fn used_today(player: &PlayerRow, now: i64) -> u32 {
    if player.moves_day == day_of(player, now) {
        u32::try_from(player.moves_used.max(0)).unwrap_or(u32::MAX)
    } else {
        0
    }
}

pub fn moves_left(player: &PlayerRow, now: i64) -> u32 {
    allowance(player).saturating_sub(used_today(player, now))
}

pub fn name_of(player: &PlayerRow) -> String {
    world::player_name(player.name_seed.max(0) as u64)
}

fn kind_word(kind: PlayerKind) -> &'static str {
    match kind {
        PlayerKind::Guest => "guest",
        PlayerKind::Claimed => "claimed",
        PlayerKind::Bot => "bot",
        PlayerKind::Person => "person",
    }
}

/// W3: the bot that runs a house, and a bot that stays home, can't play.
fn check_bot_may_play(state: &AppState, agent_id: &str) -> ApiResult<()> {
    match state.db.bot_home(agent_id)? {
        Some(home) if home.is_house => Err(ApiError::forbidden("a house bot can't play in the public worlds")),
        Some(home) if home.stays_home => {
            Err(ApiError::forbidden("a bot that stays home can't play in the public worlds"))
        }
        _ => Ok(()),
    }
}

/// A player that may be used right now. A guest nobody claimed stops working after a week (W10),
/// and one that was removed (W18) has no token any more. Every use of a token passes through here,
/// so a bot that is set to stay home after it joined stops at once (W3).
fn usable(state: &AppState, mut player: PlayerRow, now: i64) -> ApiResult<PlayerRow> {
    if player.removed {
        return Err(ApiError::unauthorized());
    }
    if player.kind() == PlayerKind::Guest && now - player.seen_at > GUEST_IDLE_SECS {
        return Err(ApiError::unauthorized());
    }
    if let Some(agent_id) = &player.agent_id {
        check_bot_may_play(state, agent_id)?;
    }
    if now - player.seen_at >= 60 {
        state.db.touch_player(player.id, now)?;
        player.seen_at = now;
    }
    Ok(player)
}

/// A signed-in person's own player, made the first time they act (W5 of the table above).
fn person_player(state: &AppState, person_id: &str, now: i64) -> ApiResult<PlayerRow> {
    let player = state.db.own_player(person_id, now)?;
    if player.removed {
        return Err(ApiError::forbidden("you have been removed from the public worlds"));
    }
    usable(state, player, now)
}

/// W2: who is calling a world route. A guest token (as a bearer), or a signed-in person by
/// session cookie, bearer session or owner key. A bot token is not a player token, and a guest
/// token is not a credential anywhere else.
pub fn player_from_headers(state: &AppState, headers: &HeaderMap, now: i64) -> ApiResult<PlayerRow> {
    let bearer = auth::bearer_token(headers);
    let secret = bearer
        .or_else(|| auth::session_cookie(headers))
        .ok_or_else(ApiError::unauthorized)?;
    let hash = auth::hash_secret(secret);
    if bearer.is_some() {
        if let Some(player) = state.db.player_by_token(&hash)? {
            return usable(state, player, now);
        }
    }
    match state.authenticate(&hash)? {
        Some(caller) if caller.is_agent() => Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "a bot token is not a player token: join first, then use the player_token you are given",
        )),
        Some(caller) => person_player(state, &caller.person_id, now),
        None => Err(ApiError::unauthorized()),
    }
}

/// The id of the stand-in for a signed-in person who hasn't made a move. No real player has it.
const STAND_IN: i64 = 0;

/// A signed-in person with no player yet. Only a move makes one, so looking saves nothing.
fn not_playing_yet(state: &AppState, headers: &HeaderMap) -> ApiResult<Option<String>> {
    let Some(secret) = auth::bearer_token(headers).or_else(|| auth::session_cookie(headers)) else {
        return Ok(None);
    };
    let Some(caller) = state.authenticate(&auth::hash_secret(secret))? else {
        return Ok(None);
    };
    Ok((!caller.is_agent() && !state.db.has_own_player(&caller.person_id)?).then_some(caller.person_id))
}

/// Who `GET /api/worlds/{id}/me` is about. A person who hasn't made a move gets a stand-in with
/// their allowance and an empty town, and nothing is saved.
pub fn player_or_stand_in(state: &AppState, headers: &HeaderMap, now: i64) -> ApiResult<PlayerRow> {
    let Some(person_id) = not_playing_yet(state, headers)? else {
        return player_from_headers(state, headers, now);
    };
    Ok(PlayerRow {
        id: STAND_IN,
        name_seed: 0,
        person_id: Some(person_id),
        agent_id: None,
        has_token: false,
        removed: false,
        moves_day: 0,
        moves_used: 0,
        seen_at: now,
        created_at: now,
    })
}

/// The same for an MCP tool: the `player_token` argument, or else a bearer token that is one.
pub fn player_from_token(
    state: &AppState,
    argument: Option<&str>,
    headers: &HeaderMap,
    now: i64,
) -> ApiResult<PlayerRow> {
    let token = argument
        .or_else(|| auth::bearer_token(headers))
        .ok_or_else(|| unauthorized_because("give the player_token that join gave you"))?;
    let player = state
        .db
        .player_by_token(&auth::hash_secret(token))?
        .ok_or_else(|| unauthorized_because("that player_token isn't valid: join again for a new one"))?;
    usable(state, player, now)
}

/// A caller that is a signed-in person, not a bot.
pub fn require_person(caller: &Caller) -> ApiResult<()> {
    if caller.is_agent() {
        return Err(ApiError::forbidden("only a signed-in person can do this"));
    }
    Ok(())
}

/// Extracts the player making a request, so it is checked before anything about the request is.
pub struct Playing(pub PlayerRow);

impl FromRequestParts<AppState> for Playing {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        player_from_headers(state, &parts.headers, now()).map(Playing)
    }
}

/// Extracts a signed-in person for a world route that is only for people. A bot is refused before
/// anything of it is touched, so asking such a route doesn't make a bot look present in its hangout
/// (W5), which `Caller` does for the routes that are for bots.
pub struct SignedIn(pub Caller);

impl FromRequestParts<AppState> for SignedIn {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let caller = auth::identify(&parts.headers, state)?;
        require_person(&caller)?;
        Ok(SignedIn(caller))
    }
}

/// W12: a request from a page on another site is turned away, so that page can't spend a visitor's
/// guests. A request with no `Origin` didn't come from a page. One whose `Origin` can't be read
/// isn't ours.
pub fn refuse_other_sites(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    match headers.get(header::ORIGIN) {
        Some(origin) if origin.as_bytes() != state.config.public_url.as_bytes() => {
            Err(ApiError::forbidden("origin not allowed"))
        }
        _ => Ok(()),
    }
}

// Worlds

fn not_found() -> ApiError {
    ApiError::not_found()
}

fn unauthorized_because(message: &'static str) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, message)
}

/// W28: The Commons is 128 by 128 with a fixed seed, and is made if it isn't there, with its
/// founding town. Every world route calls this first, and the server calls it when it starts, so it
/// exists before anyone asks. It is safe to run on a Commons that already exists: it adds the town
/// only where nothing is built yet, and touches nothing else.
pub fn ensure_commons(state: &AppState) -> ApiResult<()> {
    if state.worlds.commons_ready.load(Ordering::Acquire) {
        return Ok(());
    }
    let land = state.worlds.land_for(world::COMMONS_SEED, world::COMMONS_SIZE);
    let highway = world::highway(world::COMMONS_SEED, &land.land);
    state.db.insert_world(
        &NewWorld {
            id: COMMONS_ID,
            name: COMMONS_NAME,
            kind: "public",
            seed: world::COMMONS_SEED,
            size: world::COMMONS_SIZE,
            owner_id: None,
            open: true,
            invite_code: None,
            highway: &highway,
            now: now(),
        },
        MAX_OWN_WORLDS,
    )?;
    found_town(state, &land.land)?;
    state.worlds.commons_ready.store(true, Ordering::Release);
    Ok(())
}

/// Puts the founding town on The Commons wherever the tile is still empty. A tile with no player
/// is stored with a null builder, like the highway.
fn found_town(state: &AppState, land: &Land) -> ApiResult<()> {
    let town = world::founding_town(world::COMMONS_SEED, land);
    state.db.in_transaction(|tx| -> ApiResult<()> {
        let there = tx.all_tiles(COMMONS_ID)?;
        let fresh: Vec<Change> = town
            .into_iter()
            .filter(|(pos, _)| !there.contains_key(pos))
            .map(|(pos, tile)| Change::Set(pos, tile))
            .collect();
        Ok(tx.write_changes(COMMONS_ID, &fresh, now())?)
    })
}

fn require_world(state: &AppState, id: &str) -> ApiResult<WorldRow> {
    ensure_commons(state)?;
    state.db.world(id)?.ok_or_else(not_found)
}

fn is_owner(world: &WorldRow, player: &PlayerRow) -> bool {
    world.owner_id.is_some() && world.owner_id == player.person_id
}

/// W27, W18: whether this player may build in this world.
fn may_build(tx: &WorldTx, world: &WorldRow, player: &PlayerRow) -> ApiResult<()> {
    match tx.member_state(&world.id, player.id)? {
        Some(true) => Err(ApiError::forbidden("you were removed from this world")),
        Some(false) => Ok(()),
        None if world.open || is_owner(world, player) => Ok(()),
        None => Err(ApiError::forbidden(
            "this world is invite-only: open its link first, and then you can build",
        )),
    }
}

/// W54, W56: a building is for a person, in The Commons, within what they and the world can hold.
fn may_place(tx: &WorldTx, world: &WorldRow, player: &PlayerRow, kind: Kind) -> ApiResult<()> {
    if world.kind != "public" {
        return Err(ApiError::bad_request("buildings are only in The Commons for now"));
    }
    if player.kind() != PlayerKind::Person {
        return Err(ApiError::forbidden("only a person can place a building: sign in on the site"));
    }
    let (houses, others, everyone) = tx.building_counts(&world.id, player.id)?;
    if kind == Kind::House && houses >= 1 {
        return Err(ApiError::conflict("you already have a house"));
    }
    if kind != Kind::House && others >= MAX_THIRD_SPACES {
        return Err(ApiError::conflict("you already have as many of those as a person can"));
    }
    if everyone >= MAX_BUILDINGS {
        return Err(ApiError::conflict("The Commons has no room for more buildings right now"));
    }
    Ok(())
}

// The moves

/// W6: a move is numbers and listed words. Anything else in it is refused, and nothing it says is
/// ever sent back.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Move {
    pub action: String,
    pub x: Option<i64>,
    pub y: Option<i64>,
    pub x2: Option<i64>,
    pub y2: Option<i64>,
    pub kind: Option<String>,
    pub emote: Option<String>,
}

enum Parsed {
    Road(Span),
    Zone(Kind, Span),
    Park(i64, i64),
    Place(Kind, i64, i64),
    Bulldoze(i64, i64),
    Walk(i64, i64),
    Emote(&'static str),
}

/// A drag in any direction is the same span, with its top left corner first as the engine wants.
fn span_of(x: i64, y: i64, x2: Option<i64>, y2: Option<i64>) -> Result<Span, &'static str> {
    let (x2, y2) = match (x2, y2) {
        (None, None) => (x, y),
        (Some(x2), Some(y2)) => (x2, y2),
        _ => return Err("x2 and y2 come together"),
    };
    Ok(Span {
        x: x.min(x2),
        y: y.min(y2),
        x2: x.max(x2),
        y2: y.max(y2),
    })
}

fn parse_move(mv: &Move) -> Result<Parsed, &'static str> {
    let point = || match (mv.x, mv.y) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err("that move needs x and y"),
    };
    let only = |wanted: &[&str]| {
        let given = [
            ("x", mv.x.is_some()),
            ("y", mv.y.is_some()),
            ("x2", mv.x2.is_some()),
            ("y2", mv.y2.is_some()),
            ("kind", mv.kind.is_some()),
            ("emote", mv.emote.is_some()),
        ];
        if given.iter().any(|(name, present)| *present && !wanted.contains(name)) {
            return Err("that move doesn't take all of those");
        }
        Ok(())
    };
    match mv.action.as_str() {
        "road" => {
            only(&["x", "y", "x2", "y2"])?;
            let (x, y) = point()?;
            Ok(Parsed::Road(span_of(x, y, mv.x2, mv.y2)?))
        }
        "zone" => {
            only(&["x", "y", "x2", "y2", "kind"])?;
            let (x, y) = point()?;
            let kind = mv
                .kind
                .as_deref()
                .and_then(Kind::parse)
                .filter(|kind| kind.is_zone())
                .ok_or("a zone needs a kind: homes, shops or workshops")?;
            Ok(Parsed::Zone(kind, span_of(x, y, mv.x2, mv.y2)?))
        }
        "park" => {
            only(&["x", "y"])?;
            let (x, y) = point()?;
            Ok(Parsed::Park(x, y))
        }
        "place" => {
            only(&["x", "y", "kind"])?;
            let (x, y) = point()?;
            let kind = mv
                .kind
                .as_deref()
                .and_then(Kind::parse)
                .filter(|kind| kind.is_building())
                .ok_or("a building needs a kind: house, cafe, club or studio")?;
            Ok(Parsed::Place(kind, x, y))
        }
        "bulldoze" => {
            only(&["x", "y"])?;
            let (x, y) = point()?;
            Ok(Parsed::Bulldoze(x, y))
        }
        "walk" => {
            only(&["x", "y"])?;
            let (x, y) = point()?;
            Ok(Parsed::Walk(x, y))
        }
        "emote" => {
            only(&["emote"])?;
            let word = mv.emote.as_deref().ok_or("an emote needs an emote")?;
            let word = EMOTES
                .iter()
                .find(|emote| **emote == word)
                .ok_or("that isn't an emote: wave, cheer, think, sleep or dance")?;
            Ok(Parsed::Emote(word))
        }
        _ => Err("that isn't a move: road, zone, park, place, bulldoze, walk or emote"),
    }
}

/// What a move did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Acted {
    pub moves_left: u32,
    /// Tiles built, grown or taken down.
    pub changed: u32,
    /// Tiles left alone and not charged for.
    pub skipped: u32,
    pub cost: u32,
}

/// The tiles a planner may look at for a span: its own, and `margin` more all round. A road looks
/// one tile out for a road to touch, and a zone or a park three, for the roads that keep a side
/// street (W15). A span that is off the map or too long gets none, because the planners refuse
/// those before they look at any.
fn tiles_near(
    tx: &WorldTx,
    world_id: &str,
    land: &Land,
    span: Span,
    longest: u32,
    margin: i64,
) -> rusqlite::Result<Tiles> {
    let (Some(first), Some(last)) = (land.locate(span.x, span.y), land.locate(span.x2, span.y2)) else {
        return Ok(Tiles::new());
    };
    if last.0 < first.0 || last.1 < first.1 {
        return Ok(Tiles::new());
    }
    if u32::from(last.0 - first.0) >= longest || u32::from(last.1 - first.1) >= longest {
        return Ok(Tiles::new());
    }
    tx.tiles_in(
        world_id,
        [
            i64::from(first.0) - margin,
            i64::from(first.1) - margin,
            i64::from(last.0) + margin,
            i64::from(last.1) + margin,
        ],
    )
}

/// One move, planned and applied in one transaction. Any refusal leaves everything as it was and
/// charges nothing.
fn apply_move(
    state: &AppState,
    tx: &WorldTx,
    world: &WorldRow,
    land: &Land,
    player_id: i64,
    parsed: &Parsed,
    now: i64,
) -> ApiResult<Acted> {
    let player = tx
        .player(player_id)?
        .filter(|player| !player.removed)
        .ok_or_else(ApiError::unauthorized)?;
    may_build(tx, world, &player)?;
    let left = moves_left(&player, now);
    // W30: this answer isn't counted as a refused move, so it comes before any of the work.
    if left == 0 {
        return Err(out_of_moves("you have no moves left today", now));
    }
    let builder = player.id as u64;

    let (changes, cost, skipped) = match parsed {
        Parsed::Walk(x, y) => {
            let to = land.locate(*x, *y).ok_or("that is off the map");
            let to = to.map_err(ApiError::bad_request)?;
            if land.at(to) == Some(Ground::Water) {
                return Err(ApiError::bad_request("you can't walk on water"));
            }
            let from = tx
                .world_player(&world.id, player.id)?
                .map_or_else(|| land.centre(), |(pos, _)| pos);
            let far = i64::from(to.0.abs_diff(from.0)).max(i64::from(to.1.abs_diff(from.1)));
            if far == 0 {
                return Err(ApiError::bad_request("you are already there"));
            }
            if far > WALK_RANGE {
                return Err(ApiError::bad_request("that is too far to walk: at most 16 tiles"));
            }
            (Vec::new(), world::WALK_COST, 0)
        }
        Parsed::Emote(_) => (Vec::new(), world::EMOTE_COST, 0),
        Parsed::Road(span) => {
            let tiles = tiles_near(tx, &world.id, land, *span, world::MAX_ROAD_TILES, 1)?;
            plan_parts(world::plan_road(land, &tiles, builder, *span))?
        }
        Parsed::Zone(kind, span) => {
            let tiles =
                tiles_near(tx, &world.id, land, *span, world::MAX_ZONE_SIDE, world::SIDE_STREET)?;
            plan_parts(world::plan_zone(land, &tiles, builder, *kind, *span))?
        }
        Parsed::Park(x, y) => {
            let tiles =
                tiles_near(tx, &world.id, land, Span::point(*x, *y), 1, world::SIDE_STREET)?;
            plan_parts(world::plan_park(land, &tiles, builder, *x, *y))?
        }
        Parsed::Place(kind, x, y) => {
            may_place(tx, world, &player, *kind)?;
            let tiles =
                tiles_near(tx, &world.id, land, Span::point(*x, *y), 1, world::SIDE_STREET)?;
            plan_parts(world::plan_place(land, &tiles, builder, *kind, *x, *y))?
        }
        Parsed::Bulldoze(x, y) => {
            let near = tiles_near(tx, &world.id, land, Span::point(*x, *y), 1, 1)?;
            let pos = land.locate(*x, *y);
            // W17: the builder of whatever is there, and whether they have been gone for a week.
            let there = pos.and_then(|pos| near.get(&pos)).and_then(|tile| tile.player);
            let gone = match there {
                Some(who) => tx
                    .player_seen_at(who as i64)?
                    .is_none_or(|seen| now - seen >= world::GONE_SECS),
                None => false,
            };
            let is_gone = |who: u64| Some(who) == there && gone;
            // W17: a tile with no builder, or one this player can't take down, is refused from the
            // small read. A road that may come down also needs the whole network, to be sure
            // nothing grown depends on it, so that is read only once the rest has said yes.
            let planned = world::plan_bulldoze_tile(land, &near, builder, *x, *y, &is_gone);
            let is_road =
                pos.and_then(|pos| near.get(&pos)).is_some_and(|tile| tile.kind == Kind::Road);
            if planned.is_ok() && is_road {
                let tiles = tx.all_tiles(&world.id)?;
                plan_parts(world::plan_bulldoze(land, &tiles, builder, *x, *y, &is_gone))?
            } else {
                plan_parts(planned)?
            }
        }
    };

    if cost > left {
        return Err(out_of_moves(
            &format!("that costs {cost} moves and you have {left} left today"),
            now,
        ));
    }
    // W30: counted last, so a move that was refused above doesn't use up any share. The world is
    // counted first, so a busy world is turned away before it touches the server's.
    let limits = &state.worlds.limits;
    if !limits.world_moves.allow(&world.id, now) {
        return Err(ApiError::too_many_requests(
            "this world is very busy right now: try again in a minute",
        )
        .retry_after(limits.moves.wait(now)));
    }
    limits.moves.take("all", now, "the city is very busy right now: try again in a minute")?;

    let start = land.centre();
    tx.enter_world(&world.id, player.id, start, now)?;
    match parsed {
        Parsed::Walk(x, y) => {
            if let Some(to) = land.locate(*x, *y) {
                tx.set_position(&world.id, player.id, to)?;
            }
        }
        Parsed::Emote(word) => tx.set_emote(&world.id, player.id, word)?,
        _ => tx.write_changes(&world.id, &changes, now)?,
    }
    let used = used_today(&player, now) + cost;
    tx.charge(player.id, day_of(&player, now), i64::from(used), now)?;
    tx.wake_world(&world.id, now)?;
    Ok(Acted {
        moves_left: left - cost,
        changed: changes.len() as u32,
        skipped,
        cost,
    })
}

/// A move the player can't pay for. Moves come back at midnight UTC, so that's when to try again.
fn out_of_moves(why: &str, now: i64) -> ApiError {
    ApiError::too_many_requests(format!("{why}: they come back at midnight UTC"))
        .retry_after((86_400 - now.rem_euclid(86_400)) as u64)
}

fn plan_parts(planned: world::Planned) -> ApiResult<(Vec<Change>, u32, u32)> {
    let plan = planned.map_err(ApiError::bad_request)?;
    Ok((plan.changes, plan.cost, plan.skipped))
}

/// W13: a player makes a move in a world. This is for a caller whose address isn't known, which
/// is a test or the server itself; a request goes through `act_from`.
pub fn act(
    state: &AppState,
    player: &PlayerRow,
    world_id: &str,
    mv: &Move,
    now: i64,
) -> ApiResult<Acted> {
    act_as(state, player, None, world_id, mv, now)
}

/// W13, W30: a player makes a move in a world, from the address a request came from.
pub fn act_from(
    state: &AppState,
    headers: &HeaderMap,
    player: &PlayerRow,
    world_id: &str,
    mv: &Move,
    now: i64,
) -> ApiResult<Acted> {
    act_as(state, player, address_of(state, headers), world_id, mv, now)
}

/// W30: a refused move costs its player nothing, so it needs a limit of its own, per player and per
/// address. Past it, every move is turned away at the door until the minute is up, before any of the
/// work is done, because that is the cost it protects.
fn act_as(
    state: &AppState,
    player: &PlayerRow,
    address: Option<IpAddr>,
    world_id: &str,
    mv: &Move,
    now: i64,
) -> ApiResult<Acted> {
    let refused = &state.worlds.limits.refused;
    let mut keys = vec![format!("player:{}", player.id)];
    keys.extend(address.map(|ip| format!("address:{ip}")));
    if keys.iter().any(|key| refused.spent(key, now)) {
        return Err(ApiError::too_many_requests(
            "too many of your moves were refused just now: wait a little, and read the reasons",
        )
        .retry_after(refused.wait(now)));
    }
    let acted = make_move(state, player, world_id, mv, now);
    if acted.as_ref().is_err_and(|error| was_refused(error.status)) {
        for key in &keys {
            refused.allow(key, now);
        }
    }
    acted
}

/// Whether an answer was a refusal of the move itself: the caller was heard and said no to.
fn was_refused(status: StatusCode) -> bool {
    status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS
}

fn make_move(
    state: &AppState,
    player: &PlayerRow,
    world_id: &str,
    mv: &Move,
    now: i64,
) -> ApiResult<Acted> {
    let parsed = parse_move(mv).map_err(ApiError::bad_request)?;
    let world = require_world(state, world_id)?;
    let land = state.worlds.land_for(world.seed, world.size);
    let acted = state
        .db
        .in_transaction(|tx| apply_move(state, tx, &world, &land.land, player.id, &parsed, now))?;
    state.worlds.mark_stale(&world.id);
    Ok(acted)
}

/// Who a player is, where they stand, and what they have built, worked out from their own tiles and
/// the world's as the last view saw it. Their own tiles are read fresh, so what they built a moment
/// ago counts at once; the rest of the world is memory (W29). Returns the reply, the view it used
/// and their own tiles.
fn standing(
    state: &AppState,
    player: &PlayerRow,
    world_id: &str,
    now: i64,
) -> ApiResult<(Value, Arc<Cached>, Tiles)> {
    let world = require_world(state, world_id)?;
    let land = state.worlds.land_for(world.seed, world.size);
    let (pos, emote) = state
        .db
        .world_player(&world.id, player.id)?
        .unwrap_or((land.land.centre(), None));
    let cached = view_for(state, &world.id)?;
    let own = state.db.player_tiles(&world.id, player.id)?;
    let town = world::town(&own);
    let reply = json!({
        "player": {
            "id": player.id,
            "name": name_of(player),
            "x": pos.0,
            "y": pos.1,
            "emote": listed_emote(emote.as_deref()),
        },
        "moves_left": moves_left(player, now),
        "moves_per_day": allowance(player),
        "your_town": town,
        "goal": world::goal(&town),
        "tip": tip_for(&cached, player, &own, now),
    });
    Ok((reply, cached, own))
}

/// W47: the tip for a player. The world is the last view's, with this player's own tiles put in
/// over it, so a tip that follows what they just did doesn't wait for the next view.
fn tip_for(cached: &Cached, player: &PlayerRow, own: &Tiles, now: i64) -> &'static str {
    let me = player.id as u64;
    let mut tiles = (*cached.tiles).clone();
    tiles.retain(|_, tile| tile.player != Some(me));
    tiles.extend(own.iter().map(|(pos, tile)| (*pos, *tile)));
    world::tip(&tiles, me, moves_left(player, now))
}

/// A player's own view of themselves in a world: who they are, where, what they have left, their
/// town and goal (W48, W49), and a tip.
pub fn me(state: &AppState, player: &PlayerRow, world_id: &str, now: i64) -> ApiResult<Value> {
    let mut reply = standing(state, player, world_id, now)?.0;
    if player.id == STAND_IN {
        reply["player"] = Value::Null;
    }
    Ok(reply)
}

fn listed_emote(emote: Option<&str>) -> Option<&'static str> {
    emote.and_then(|word| EMOTES.iter().find(|listed| **listed == word).copied())
}

/// The longer answer to "how am I doing": `me`, and what they have built here and how the world is.
pub fn how_am_i_doing(
    state: &AppState,
    player: &PlayerRow,
    world_id: &str,
    now: i64,
) -> ApiResult<Value> {
    let (mut out, cached, own) = standing(state, player, world_id, now)?;
    let mut built: BTreeMap<&str, u32> = BTreeMap::new();
    for tile in own.values() {
        *built.entry(tile.kind.as_str()).or_default() += 1;
    }
    out["kind"] = json!(kind_word(player.kind()));
    out["built"] = json!(built);
    out["world"] = summary_json(&cached.summary);
    Ok(out)
}

fn summary_json(summary: &Summary) -> Value {
    json!({
        "name": summary.name,
        "size": summary.size,
        "step": summary.step,
        "population": summary.population,
        "jobs": summary.jobs,
        "parks": summary.parks,
        "stage": summary.stage,
    })
}

// Joining

/// What a player is given when they join.
pub struct Joined {
    pub player: PlayerRow,
    pub token: String,
    /// Only a guest has one: a bot's player already belongs to its owner.
    pub claim_code: Option<String>,
    pub world: WorldRow,
    /// One fixed sentence on what to do first, from `world::TIPS`.
    pub tip: &'static str,
}

/// Who is asking, for the limits per address (W12, W30). On a server reached over https every request
/// has come through Caddy, which always adds the header, so one without it is as suspect as one that
/// can't be read: it counts as the shared unknown address. Over plain http, which is local
/// development, no header means a caller on this machine, who has no address and isn't limited.
pub(crate) fn address_of(state: &AppState, headers: &HeaderMap) -> Option<IpAddr> {
    match auth::client_ip(headers) {
        None if state.is_https() => Some(auth::UNKNOWN_CLIENT),
        found => found,
    }
}

/// W12: new guests per address and across the server. A header that can't be read is one address
/// for all.
fn allow_new_guest(state: &AppState, headers: &HeaderMap, now: i64) -> ApiResult<()> {
    let limits = &state.worlds.limits;
    if let Some(ip) = address_of(state, headers) {
        limits.guests_per_ip.take(
            &ip.to_string(),
            now,
            "too many new players from your address: try again in an hour",
        )?;
    }
    limits.guests.take("all", now, "a lot of new players have joined this hour: try again soon")
}

/// Joining: `world_id` is where they start, and defaults to The Commons. With a bot's own token
/// (W3) it links a player to that bot and gives it a fresh `player_token`. With none it makes a
/// guest (W10, W12). Nothing a caller sends is stored.
pub fn join(
    state: &AppState,
    headers: &HeaderMap,
    world_id: Option<&str>,
    now: i64,
) -> ApiResult<Joined> {
    let world = require_world(state, world_id.unwrap_or(COMMONS_ID))?;
    if !world.open {
        return Err(ApiError::forbidden(
            "this world is invite-only: open its link to get in",
        ));
    }
    let token = auth::new_guest_token();
    let token_hash = auth::hash_secret(&token);
    let (player, claim_code) = match auth::bearer_token(headers) {
        Some(bearer) => {
            // W30: a bot coming back is looked at like a view, so it can't be made to do the work
            // of one over and over.
            allow_view(state, headers, now)?;
            let hash = auth::hash_secret(bearer);
            if state.db.player_by_token(&hash)?.is_some() {
                return Err(ApiError::bad_request("you already have a player: use your player_token"));
            }
            let caller = state.authenticate(&hash)?.ok_or_else(ApiError::unauthorized)?;
            let Some(agent_id) = caller.agent_id.as_deref() else {
                return Err(ApiError::bad_request(
                    "people who are signed in don't join: just build, and a player is made for you",
                ));
            };
            check_bot_may_play(state, agent_id)?;
            // W18: a player the owner removed from this world can't come back by joining again.
            let joined =
                state.db.join_as_bot(agent_id, &caller.person_id, &token_hash, &world.id, now)?;
            let player = match joined {
                BotJoined::Done(player) => player,
                BotJoined::Removed => {
                    return Err(ApiError::forbidden("this bot was removed from the public worlds"))
                }
                BotJoined::Barred => {
                    return Err(ApiError::forbidden("you were removed from this world"))
                }
            };
            (player, None)
        }
        None => {
            allow_new_guest(state, headers, now)?;
            let claim = auth::new_claim_code();
            let player = state.db.create_guest(&token_hash, &auth::hash_secret(&claim), now)?;
            (player, Some(claim))
        }
    };
    let land = state.worlds.land_for(world.seed, world.size);
    let centre = land.land.centre();
    // The ban is checked again with the step that puts them on the street, so a removal that lands
    // in between still keeps them off it.
    state.db.in_transaction(|tx| -> ApiResult<()> {
        if tx.member_state(&world.id, player.id)? == Some(true) {
            return Err(ApiError::forbidden("you were removed from this world"));
        }
        Ok(tx.enter_world(&world.id, player.id, centre, now)?)
    })?;
    state.worlds.mark_stale(&world.id);
    state.worlds.cards_stale.store(true, Ordering::Release);
    // A new guest has built nothing. A bot that comes back may have a town, which is read the way
    // `me` reads it: its own tiles, and the world from memory.
    let tip = match claim_code {
        Some(_) => world::tip(&Tiles::new(), player.id as u64, moves_left(&player, now)),
        None => {
            let own = state.db.player_tiles(&world.id, player.id)?;
            let cached = view_for(state, &world.id)?;
            tip_for(&cached, &player, &own, now)
        }
    };
    Ok(Joined {
        player,
        token,
        claim_code,
        world,
        tip,
    })
}

/// W27: a player opens a world's link, and may build there from then on.
pub fn join_by_code(state: &AppState, player: &PlayerRow, code: &str, now: i64) -> ApiResult<String> {
    ensure_commons(state)?;
    state.worlds.limits.people.take(&format!("code:{}", player.id), now, "slow down")?;
    let world = state.db.world_by_code(code)?.ok_or_else(not_found)?;
    let land = state.worlds.land_for(world.seed, world.size);
    let centre = land.land.centre();
    let allowed = state.db.in_transaction(|tx| -> ApiResult<bool> {
        if !tx.add_member(&world.id, player.id)? {
            return Ok(false);
        }
        tx.enter_world(&world.id, player.id, centre, now)?;
        Ok(true)
    })?;
    if !allowed {
        return Err(ApiError::forbidden("you were removed from this world"));
    }
    state.worlds.mark_stale(&world.id);
    Ok(world.id)
}

// Claiming, listing and renaming

/// W11: a signed-in person enters a claim code, and the guest becomes theirs.
pub fn claim(state: &AppState, caller: &Caller, code: &str, now: i64) -> ApiResult<PlayerRow> {
    require_person(caller)?;
    state.worlds.limits.people.take(&format!("claim:{}", caller.person_id), now, "slow down")?;
    let claimed = state.db.claim_player(
        &auth::hash_secret(code),
        &caller.person_id,
        MAX_CLAIMS,
        now - GUEST_IDLE_SECS,
    )?;
    match claimed {
        Claimed::Done(player) => Ok(player),
        Claimed::NoSuchCode => Err(ApiError::new(StatusCode::NOT_FOUND, "that claim code doesn't work")),
        Claimed::TooMany => Err(ApiError::conflict("you have claimed as many players as you can")),
    }
}

/// W4: the one place a player is tied to a person, and only to that person.
pub fn my_players(state: &AppState, caller: &Caller, now: i64) -> ApiResult<Value> {
    require_person(caller)?;
    let players: Vec<Value> = state
        .db
        .players_of(&caller.person_id)?
        .iter()
        .map(|player| {
            json!({
                "id": player.id,
                "name": name_of(player),
                "kind": kind_word(player.kind()),
                "moves_left": moves_left(player, now),
                "moves_per_day": allowance(player),
            })
        })
        .collect();
    Ok(json!({ "players": players }))
}

/// W7: a new name from the same lists, for a player the person claimed.
pub fn reroll(state: &AppState, caller: &Caller, player_id: &str, now: i64) -> ApiResult<String> {
    require_person(caller)?;
    let id: i64 = player_id.parse().map_err(|_| not_found())?;
    let player = state
        .db
        .player(id)?
        .filter(|player| !player.removed && player.person_id.as_deref() == Some(caller.person_id.as_str()))
        .ok_or_else(not_found)?;
    state.worlds.limits.people.take(&format!("reroll:{}", caller.person_id), now, "slow down")?;
    let current = name_of(&player);
    let mut name_seed = player.name_seed;
    for _ in 0..16 {
        name_seed = random_name_seed();
        if world::player_name(name_seed as u64) != current {
            break;
        }
    }
    state.db.set_name_seed(player.id, name_seed)?;
    for world_id in state.db.worlds_of_player(player.id)? {
        state.worlds.mark_stale(&world_id);
    }
    Ok(world::player_name(name_seed as u64))
}

/// W3: the person who claimed a player ends it. Its token stops and it leaves the street, and what
/// it built stays. A person's own player and a bot's can't be ended this way: the first is theirs
/// for good, and the second ends when its bot's token is replaced.
pub fn revoke(state: &AppState, caller: &Caller, player_id: &str) -> ApiResult<()> {
    require_person(caller)?;
    let id: i64 = player_id.parse().map_err(|_| not_found())?;
    let player = state
        .db
        .player(id)?
        .filter(|player| !player.removed && player.person_id.as_deref() == Some(caller.person_id.as_str()))
        .ok_or_else(not_found)?;
    if player.kind() != PlayerKind::Claimed {
        return Err(ApiError::bad_request(
            "only a player you claimed can be ended: replace a bot's token to end its player",
        ));
    }
    let streets = state.db.worlds_of_player(player.id)?;
    if !state.db.end_claimed_player(player.id, &caller.person_id)? {
        return Err(not_found());
    }
    for world_id in streets {
        state.worlds.mark_stale(&world_id);
    }
    state.worlds.cards_stale.store(true, Ordering::Release);
    Ok(())
}

/// W18: an owner takes a player out of their own world, or the keeper out of The Commons. Their
/// tiles there come down. In The Commons the player is ended: the token stops. In a world of
/// someone's own they're only barred from it, because a player belongs to the whole server and
/// one person's world must not be able to end it. Returns how many tiles came down.
pub fn remove_player(
    state: &AppState,
    caller: &Caller,
    world_id: &str,
    player_id: &str,
) -> ApiResult<usize> {
    require_person(caller)?;
    let world = require_world(state, world_id)?;
    let keeper = state.config.keeper_id.as_deref() == Some(caller.person_id.as_str());
    let owner = world.owner_id.as_deref() == Some(caller.person_id.as_str());
    let commons = world.kind == "public";
    if !(commons && keeper || !commons && owner) {
        return Err(ApiError::forbidden("only this world's owner can remove players from it"));
    }
    let id: i64 = player_id.parse().map_err(|_| not_found())?;
    let target = state.db.player(id)?.ok_or_else(not_found)?;
    if !state.db.player_known_in(&world.id, id)? {
        return Err(not_found());
    }
    if target.person_id.as_deref() == Some(caller.person_id.as_str()) {
        return Err(ApiError::bad_request("you can't remove yourself"));
    }
    let down = state.db.remove_from_world(&world.id, id, !commons, commons)?;
    state.worlds.mark_stale(&world.id);
    Ok(down)
}

// Starting a world

/// W26: a few generated maps to choose from. They are made and sent, and nothing is kept: only a
/// world that exists keeps its land.
pub fn preview(state: &AppState, caller: &Caller, now: i64) -> ApiResult<Value> {
    require_person(caller)?;
    state.worlds.limits.people.take(&format!("preview:{}", caller.person_id), now, "slow down")?;
    let seeds: Vec<Value> = (0..PREVIEW_SEEDS)
        .map(|_| {
            let seed = rand::random_range(1..(1u64 << 32));
            json!({
                "seed": seed,
                "name": world::world_name(seed),
                "land": world::encode_land(&world::terrain(seed, world::OWN_SIZE)),
            })
        })
        .collect();
    Ok(json!({ "seeds": seeds }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewWorldBody {
    pub seed: Option<u64>,
    pub open: Option<bool>,
}

/// Why a world couldn't be added, or `None` if it could.
fn why_no_world(added: WorldAdded) -> Option<ApiError> {
    match added {
        WorldAdded::Added => None,
        WorldAdded::OwnerHasOne => Some(ApiError::conflict("you already have a world")),
        WorldAdded::Full => Some(ApiError::conflict("this server has all the worlds it can hold")),
        WorldAdded::Exists => Some(ApiError::conflict("try that again")),
    }
}

/// W25: a signed-in person starts a world, with no server code. One each, and the server holds 200.
/// The cheap answers come first and no land is made for them, then the person's limit, and only
/// then the land, which is kept once the world is there.
pub fn create_world(
    state: &AppState,
    caller: &Caller,
    body: &NewWorldBody,
    now: i64,
) -> ApiResult<Arc<Cached>> {
    require_person(caller)?;
    ensure_commons(state)?;
    let seed = body.seed.unwrap_or_else(|| rand::random_range(1..(1u64 << 32)));
    if seed > MAX_SEED {
        return Err(ApiError::bad_request("that seed is too big"));
    }
    if let Some(refused) = why_no_world(state.db.room_for_world(&caller.person_id, MAX_OWN_WORLDS)?) {
        return Err(refused);
    }
    state.worlds.limits.people.take(&format!("world:{}", caller.person_id), now, "slow down")?;
    let land = world::terrain(seed, world::OWN_SIZE);
    let highway = world::highway(seed, &land);
    let id = auth::new_id();
    let invite = auth::new_invite_code();
    let added = state.db.insert_world(
        &NewWorld {
            id: &id,
            name: &world::world_name(seed),
            kind: "own",
            seed,
            size: world::OWN_SIZE,
            owner_id: Some(&caller.person_id),
            open: body.open.unwrap_or(true),
            invite_code: Some(&invite),
            highway: &highway,
            now,
        },
        MAX_OWN_WORLDS,
    )?;
    if let Some(refused) = why_no_world(added) {
        return Err(refused);
    }
    state.worlds.keep_land(seed, world::OWN_SIZE, land);
    state.worlds.cards_stale.store(true, Ordering::Release);
    view_for(state, &id)
}

/// The caller's own world, with the link that lets others build in it. Only they see the link.
pub fn my_world(state: &AppState, caller: &Caller) -> ApiResult<Value> {
    require_person(caller)?;
    ensure_commons(state)?;
    Ok(match state.db.world_of_owner(&caller.person_id)? {
        None => json!({ "world": null }),
        Some(world) => json!({
            "world": {
                "id": world.id,
                "name": world.name,
                "open": world.open,
                "invite_code": world.invite_code,
            }
        }),
    })
}

// Views

#[derive(Serialize)]
struct ViewOut<'a> {
    id: &'a str,
    name: &'a str,
    kind: &'a str,
    open: bool,
    size: usize,
    seed: u64,
    step: u64,
    land: &'a [String],
    tiles: Vec<(u16, u16, &'static str, u8)>,
    /// `[id, name, x, y, emote, kind]`, where `kind` is `person` or `agent` (W53).
    players: Vec<(i64, String, u16, u16, Option<&'static str>, &'static str)>,
    population: u32,
    jobs: u32,
    stage: &'static str,
    parks: u32,
    updated_at: i64,
    /// `[kind, name, x, y, age_s]`, where `age_s` is the seconds since that crawler was last seen.
    #[serde(skip_serializing_if = "Option::is_none")]
    crawlers: Option<Vec<(&'static str, &'static str, u16, u16, i64)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    visitors: Option<Vec<(String, u16, u16)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    people: Option<Vec<(String, u16, u16)>>,
}

fn etag_of(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let hex: String = digest[..12].iter().map(|byte| format!("{byte:02x}")).collect();
    format!("\"{hex}\"")
}

/// W29, W31: builds a world's view: the land, everything built, the street, the sums, and in The
/// Commons the frogs, visitors and people. Built from the database and the engine, never from a
/// request.
fn build_view(state: &AppState, world_id: &str, now: i64) -> ApiResult<Cached> {
    let snapshot = state
        .db
        .world_snapshot(world_id, now - PLAYER_ACTIVE_SECS, MAX_LISTED_PLAYERS)?
        .ok_or_else(not_found)?;
    state.worlds.builds.fetch_add(1, Ordering::Relaxed);
    let world = &snapshot.world;
    let land = state.worlds.land_for(world.seed, world.size);
    let stats = world::stats(&snapshot.tiles);

    let mut tiles: Vec<(u16, u16, &'static str, u8)> = snapshot
        .tiles
        .iter()
        .map(|(&(x, y), tile)| (x, y, tile.kind.as_str(), tile.level))
        .collect();
    tiles.sort_unstable_by_key(|&(x, y, _, _)| (y, x));
    let players = snapshot
        .players
        .iter()
        .map(|player| {
            (
                player.id,
                world::player_name(player.name_seed.max(0) as u64),
                player.pos.0,
                player.pos.1,
                listed_emote(player.emote.as_deref()),
                if player.is_person { "person" } else { "agent" },
            )
        })
        .collect();

    let (crawlers, visitors, people) = if world.id == COMMONS_ID {
        let roads = Roads::from_tiles(&snapshot.tiles);
        let on_the_street = |numbers: Vec<u64>| -> Vec<(String, u16, u16)> {
            numbers
                .into_iter()
                .filter_map(|n| {
                    world::visitor_position(n, world.step, &roads)
                        .map(|(x, y)| (world::visitor_name(n), x, y))
                })
                .collect()
        };
        let crawlers = state
            .worlds
            .frogs(now)
            .into_iter()
            .filter_map(|(crawler, last)| {
                world::crawler_position(crawler.id, world.step, &roads)
                    .map(|(x, y)| (crawler.id, crawler.name, x, y, (now - last).max(0)))
            })
            .collect();
        let visitors = on_the_street(state.worlds.visitors(now));
        let people = on_the_street(state.worlds.people(now));
        (Some(crawlers), Some(visitors), Some(people))
    } else {
        (None, None, None)
    };

    let mut out = ViewOut {
        id: &world.id,
        name: &world.name,
        kind: &world.kind,
        open: world.open,
        size: world.size,
        seed: world.seed,
        step: world.step,
        land: &land.rows,
        tiles,
        players,
        population: stats.population,
        jobs: stats.jobs,
        stage: stats.stage,
        parks: stats.parks,
        updated_at: 0,
        crawlers,
        visitors,
        people,
    };
    // The tag leaves out the time it was built and how long ago each frog was seen, because both
    // tick on their own. A view that didn't otherwise change keeps its tag, and a browser that
    // asks again is told so instead of being sent it all again.
    let ages: Vec<i64> = out
        .crawlers
        .as_mut()
        .map(|frogs| frogs.iter_mut().map(|frog| std::mem::take(&mut frog.4)).collect())
        .unwrap_or_default();
    let etag = etag_of(&serde_json::to_vec(&out).map_err(internal)?);
    out.updated_at = now;
    for (frog, age) in out.crawlers.iter_mut().flatten().zip(ages) {
        frog.4 = age;
    }
    let body = Bytes::from(serde_json::to_vec(&out).map_err(internal)?);
    Ok(Cached {
        body,
        etag,
        summary: Summary {
            name: world.name.clone(),
            size: world.size,
            step: world.step,
            population: stats.population,
            jobs: stats.jobs,
            parks: stats.parks,
            stage: stats.stage,
        },
        tiles: Arc::new(snapshot.tiles),
    })
}

fn internal(error: serde_json::Error) -> ApiError {
    tracing::error!(%error, "a world view could not be written");
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "something went wrong")
}

/// W29: the view of a world, from memory. It is rebuilt at most once a second after something
/// changed, and at least every few seconds, whoever asks and however many do. One caller builds
/// while the others wait for it and then share what it made.
pub fn view_for(state: &AppState, world_id: &str) -> ApiResult<Arc<Cached>> {
    ensure_commons(state)?;
    let existing = locked(&state.worlds.views).get(world_id).cloned();
    let slot = match existing {
        Some(slot) => slot,
        None => {
            // Only worlds that exist get a place, so ids a caller makes up can't fill memory.
            if state.db.world(world_id)?.is_none() {
                return Err(not_found());
            }
            locked(&state.worlds.views)
                .entry(world_id.to_string())
                .or_insert_with(|| {
                    Arc::new(Slot {
                        stale: AtomicBool::new(true),
                        inner: Mutex::new(SlotInner::default()),
                    })
                })
                .clone()
        }
    };
    let mut inner = locked(&slot.inner);
    if let (Some(built), Some(cached)) = (inner.built, &inner.cached) {
        let age = built.elapsed();
        let stale = slot.stale.load(Ordering::Acquire);
        if age < REBUILD_AT_LEAST && !(stale && age >= REBUILD_AFTER_CHANGE) {
            return Ok(cached.clone());
        }
    }
    // Cleared first, so a change that lands while this is being built asks for another.
    slot.stale.store(false, Ordering::Release);
    let built = match build_view(state, world_id, now()) {
        Ok(built) => Arc::new(built),
        Err(error) => {
            slot.stale.store(true, Ordering::Release);
            return Err(error);
        }
    };
    inner.built = Some(Instant::now());
    inner.cached = Some(built.clone());
    Ok(built)
}

#[derive(Serialize)]
struct CardOut<'a> {
    id: &'a str,
    name: &'a str,
    kind: &'a str,
    open: bool,
    population: u32,
    jobs: u32,
    stage: &'static str,
    players_active: i64,
}

/// W29: the busiest worlds, from memory like the views. The Commons is always on the list.
pub fn cards(state: &AppState, now: i64) -> ApiResult<Arc<Cached>> {
    ensure_commons(state)?;
    let mut cache = locked(&state.worlds.cards);
    if let Some((built, cached)) = cache.as_ref() {
        let stale = state.worlds.cards_stale.load(Ordering::Acquire);
        if built.elapsed() < CARDS_AFTER && !(stale && built.elapsed() >= REBUILD_AFTER_CHANGE) {
            return Ok(cached.clone());
        }
    }
    state.worlds.cards_stale.store(false, Ordering::Release);
    let mut busiest = state.db.busiest_worlds(now - PLAYER_ACTIVE_SECS, LISTED_WORLDS)?;
    if !busiest.iter().any(|(world, _)| world.id == COMMONS_ID) {
        if let Some(commons) = state.db.world(COMMONS_ID)? {
            busiest.insert(0, (commons, 0));
            busiest.truncate(LISTED_WORLDS as usize);
        }
    }
    let mut out = Vec::with_capacity(busiest.len());
    for (world, active) in &busiest {
        let stats = world::stats(&state.db.world_tiles(&world.id)?);
        out.push(CardOut {
            id: &world.id,
            name: &world.name,
            kind: &world.kind,
            open: world.open,
            population: stats.population,
            jobs: stats.jobs,
            stage: stats.stage,
            players_active: *active,
        });
    }
    #[derive(Serialize)]
    struct CardsOut<'a> {
        worlds: Vec<CardOut<'a>>,
    }
    let body = Bytes::from(serde_json::to_vec(&CardsOut { worlds: out }).map_err(internal)?);
    let cached = Arc::new(Cached {
        etag: etag_of(&body),
        body,
        summary: Summary {
            name: String::new(),
            size: 0,
            step: 0,
            population: 0,
            jobs: 0,
            parks: 0,
            stage: "hamlet",
        },
        tiles: Arc::new(Tiles::new()),
    });
    *cache = Some((Instant::now(), cached.clone()));
    Ok(cached)
}

/// The reply for a cached body: 304 if the caller already has it, so polling is cheap.
pub fn respond(cached: &Cached, headers: &HeaderMap) -> Response {
    let etag = HeaderValue::from_str(&cached.etag).unwrap_or(HeaderValue::from_static("\"\""));
    let has_it = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|tags| {
            tags.split(',').any(|tag| {
                let tag = tag.trim();
                tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == cached.etag
            })
        });
    if has_it {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, etag), (header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))],
        )
            .into_response();
    }
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/json")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            (header::ETAG, etag),
        ],
        cached.body.clone(),
    )
        .into_response()
}

/// W30: world views per address, a minute at a time. No address header means a caller on this
/// machine, who isn't limited, unless the server is reached over https (see `address_of`). A header
/// that can't be read is one address for all of them.
pub fn allow_view(state: &AppState, headers: &HeaderMap, now: i64) -> ApiResult<()> {
    match address_of(state, headers) {
        Some(ip) => state.worlds.limits.views_per_ip.take(
            &ip.to_string(),
            now,
            "you are looking too often: wait a few seconds and try again",
        ),
        None => Ok(()),
    }
}

/// The player behind a request that carries a bearer token, or nobody if it carries none. `look` is
/// for anyone, and says a little more to a player. A cookie doesn't count: a page that is only
/// looking shouldn't make a player of the person who is signed in.
pub fn player_if_any(
    state: &AppState,
    headers: &HeaderMap,
    now: i64,
) -> ApiResult<Option<PlayerRow>> {
    if auth::bearer_token(headers).is_none() || not_playing_yet(state, headers)?.is_some() {
        return Ok(None);
    }
    player_from_headers(state, headers, now).map(Some)
}

/// W31: a window of a world, at most 32 by 32 tiles, for an agent that wants to read part of it.
/// Land is one letter a tile (g grass, f forest, w water, s sand, h hill), and what's built comes
/// as `[x, y, kind, level]`. `reserved` lists the open tiles in it that are kept for side streets
/// (W15). Everything in it is a number or a listed word.
pub fn look(
    state: &AppState,
    world_id: &str,
    centre: Option<(i64, i64)>,
    width: Option<i64>,
    height: Option<i64>,
    you: Option<&PlayerRow>,
    now: i64,
) -> ApiResult<Value> {
    let world = require_world(state, world_id)?;
    let cached = view_for(state, &world.id)?;
    let land = state.worlds.land_for(world.seed, world.size);
    let size = land.land.size();
    let wide = clamp_window(width, size);
    let high = clamp_window(height, size);
    let own_pos = match you {
        Some(player) => state.db.world_player(&world.id, player.id)?.map(|(pos, _)| pos),
        None => None,
    };
    let (cx, cy) = match centre {
        Some((x, y)) => {
            let pos = land
                .land
                .locate(x, y)
                .ok_or_else(|| ApiError::bad_request("that is off the map"))?;
            (usize::from(pos.0), usize::from(pos.1))
        }
        None => {
            let pos = own_pos.unwrap_or_else(|| land.land.centre());
            (usize::from(pos.0), usize::from(pos.1))
        }
    };
    let x0 = cx.saturating_sub(wide / 2).min(size - wide);
    let y0 = cy.saturating_sub(high / 2).min(size - high);
    let rows: Vec<String> = (y0..y0 + high)
        .map(|y| {
            (x0..x0 + wide)
                .map(|x| land.land.get(x, y).map_or('w', Ground::letter))
                .collect()
        })
        .collect();
    let region = [x0 as i64, y0 as i64, (x0 + wide - 1) as i64, (y0 + high - 1) as i64];
    // A side street is told by the roads up to 3 tiles from a tile, so those are read as well.
    let reach = world::SIDE_STREET;
    let around = [region[0] - reach, region[1] - reach, region[2] + reach, region[3] + reach];
    let nearby = state.db.world_tiles_in(&world.id, around)?;
    let reserved = world::reserved_in(&land.land, &nearby, region);
    let mut built: Vec<(u16, u16, &'static str, u8)> = nearby
        .iter()
        .filter(|(&(x, y), _)| {
            (region[0]..=region[2]).contains(&i64::from(x))
                && (region[1]..=region[3]).contains(&i64::from(y))
        })
        .map(|(&(x, y), tile)| (x, y, tile.kind.as_str(), tile.level))
        .collect();
    built.sort_unstable_by_key(|&(x, y, _, _)| (y, x));
    let players: Vec<Value> = state
        .db
        .world_players_in(&world.id, now - PLAYER_ACTIVE_SECS, region, MAX_LISTED_PLAYERS)?
        .iter()
        .map(|player| {
            json!([
                player.id,
                world::player_name(player.name_seed.max(0) as u64),
                player.pos.0,
                player.pos.1,
                listed_emote(player.emote.as_deref()),
            ])
        })
        .collect();
    let mut out = json!({
        "world": summary_json(&cached.summary),
        "window": { "x": x0, "y": y0, "w": wide, "h": high },
        "legend": "land rows are one letter a tile: g grass, f forest, w water, s sand, h hill. \
                   Row 0 of land is the window's top row, and its first letter is its left column. \
                   built is [x, y, kind, level], and a kind of house, cafe, club or studio is a \
                   building a person placed. players is [id, name, x, y, emote]. reserved is \
                   [x, y] for each open tile kept for a side street: zones and parks skip it and \
                   roads may use it.",
        "land": rows,
        "built": built,
        "reserved": reserved,
        "players": players,
    });
    if let Some(player) = you {
        let pos = own_pos.unwrap_or_else(|| land.land.centre());
        out["you"] = json!({
            "name": name_of(player),
            "x": pos.0,
            "y": pos.1,
            "moves_left": moves_left(player, now),
        });
    }
    Ok(out)
}

/// A window's side: the default if none is asked for, never more than 32 or the map, never less than 1.
fn clamp_window(asked: Option<i64>, map: usize) -> usize {
    let side = match asked {
        Some(side) => side.clamp(1, LOOK_MAX as i64) as usize,
        None => LOOK_DEFAULT,
    };
    side.min(map).max(1)
}

// The step

/// W19: one world, one step. Pure: the tiles, the seed and the step number decide it. The whole
/// step is saved in one transaction, and a move can't land between reading the tiles and saving.
pub fn step_world(state: &AppState, world_id: &str, now: i64) -> ApiResult<usize> {
    let changed = state.db.in_transaction(|tx| -> ApiResult<usize> {
        let world = tx.world(world_id)?.ok_or_else(not_found)?;
        let tiles = tx.all_tiles(&world.id)?;
        let changes = world::step(world.seed, world.step, &tiles);
        tx.write_changes(&world.id, &changes, now)?;
        tx.set_step(&world.id, world.step + 1)?;
        Ok(changes.len())
    })?;
    state.worlds.mark_stale(world_id);
    Ok(changed)
}

/// W24: the worlds worth stepping now, which are the ones someone has moved in lately, and always
/// The Commons, because the people, agents and frogs on its street walk by its step.
pub fn worlds_due(state: &AppState, now: i64) -> ApiResult<Vec<String>> {
    ensure_commons(state)?;
    let mut due = state.db.worlds_awake(now - IDLE_WORLD_SECS)?;
    if !due.iter().any(|id| id == COMMONS_ID) {
        due.push(COMMONS_ID.to_string());
        due.sort();
    }
    Ok(due)
}

/// Steps one world and says so if it fails. A failure is logged and retried on the next tick,
/// because the world is still due then (W32).
pub fn step_one(state: &AppState, world_id: &str, now: i64) -> bool {
    match step_world(state, world_id, now) {
        Ok(_) => true,
        Err(error) => {
            tracing::error!(status = %error.status, "a world could not take its step");
            false
        }
    }
}

/// What a round of steps did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StepReport {
    pub stepped: Vec<String>,
    pub failed: Vec<String>,
}

/// Everything a tick does that isn't a world's step: counts sightings that are due, and clears out
/// guests that were never seen again.
pub fn housekeeping(state: &AppState, now: i64) {
    write_due_sightings(state, now);
    let last = state.worlds.last_prune.load(Ordering::Relaxed);
    if now - last >= 3600
        && state
            .worlds
            .last_prune
            .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    {
        if let Err(error) = state.db.prune_guests(now - PRUNE_AFTER_SECS) {
            tracing::error!(%error, "old guests could not be cleared");
        }
    }
}

/// W32: steps every world that is due, one at a time. The server runs this once a minute from a
/// background task that does a world at a time and yields in between; a test calls it directly.
pub fn step_all(state: &AppState, now: i64) -> StepReport {
    let mut report = StepReport::default();
    let due = match worlds_due(state, now) {
        Ok(due) => due,
        Err(error) => {
            tracing::error!(status = %error.status, "the worlds due a step could not be listed");
            return report;
        }
    };
    for id in due {
        if step_one(state, &id, now) {
            report.stepped.push(id);
        } else {
            report.failed.push(id);
        }
        std::thread::yield_now();
    }
    housekeeping(state, now);
    report
}

/// The background task: a step a minute, each world on a blocking thread of its own so the
/// server's other work never waits on one, with a pause between worlds.
pub async fn run_steps(state: AppState) {
    let mut ticks = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(60),
        Duration::from_secs(60),
    );
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticks.tick().await;
        let due_state = state.clone();
        let due = tokio::task::spawn_blocking(move || worlds_due(&due_state, now())).await;
        let due = match due {
            Ok(Ok(due)) => due,
            _ => {
                tracing::error!("the worlds due a step could not be listed");
                continue;
            }
        };
        for id in due {
            let step_state = state.clone();
            if tokio::task::spawn_blocking(move || step_one(&step_state, &id, now()))
                .await
                .is_err()
            {
                tracing::error!("a world's step stopped half way");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let done_state = state.clone();
        let _ = tokio::task::spawn_blocking(move || housekeeping(&done_state, now())).await;
    }
}

// Crawlers and visitors

/// W39: writes the sightings that have waited long enough. Nothing but a kind, a day and a count.
fn write_due_sightings(state: &AppState, now: i64) {
    for (id, visits) in state.worlds.due_sightings(now) {
        record_sightings(state, id, visits, now);
    }
}

fn record_sightings(state: &AppState, id: &'static str, visits: u32, now: i64) {
    if state
        .db
        .add_sightings(id, today(now), i64::from(visits), now)
        .is_err()
    {
        // Nothing but a kind is said, so a log can't be made to hold what a visitor sent.
        tracing::warn!(kind = id, "sightings could not be written");
        state.worlds.return_sightings(id, visits);
    }
}

/// What a request is, as far as who passes through: a page, something only agents read, or a page
/// that is open and looking at The Commons.
enum Place {
    Page,
    AgentOnly,
    Commons,
}

/// W34, W41, W52: the places that are watched. Nothing else is, and private routes never are.
fn place_of(method: &Method, path: &str) -> Option<Place> {
    if method == Method::GET {
        match path {
            "/" | "/worlds" | "/robots.txt" | "/sitemap.xml" => return Some(Place::Page),
            "/llms.txt" | "/llms-full.txt" | "/agents.md" | "/.well-known/ai-catalog.json"
            | "/public/mcp/server-card" => return Some(Place::AgentOnly),
            "/api/worlds/commons" => return Some(Place::Commons),
            _ => {}
        }
        if path
            .strip_prefix("/world/")
            .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
        {
            return Some(Place::Page);
        }
    }
    if method == Method::POST && path == "/public/mcp" {
        return Some(Place::AgentOnly);
    }
    None
}

/// The start of a user agent, which is as much as the lists look at. It is looked at and dropped.
fn agent_of(headers: &HeaderMap) -> Option<world::AgentKind> {
    let value = headers.get(header::USER_AGENT)?;
    let head = &value.as_bytes()[..value.as_bytes().len().min(512)];
    world::agent_kind(&String::from_utf8_lossy(head))
}

/// W52: a browser says this about a page's own fetch, and curl and bots don't. It is easy to fake,
/// so a person on the street is a sighting, as a frog is (W39).
fn is_a_page_watching(headers: &HeaderMap) -> bool {
    let says = |name: &str, value: &str| headers.get(name).is_some_and(|got| got == value);
    says("sec-fetch-site", "same-origin") && says("sec-fetch-mode", "cors")
}

/// W52: whether this page belongs to someone already on the street as their own player, so they
/// don't show twice. A sign-in that has lapsed, or no sign-in, is anyone else watching.
fn is_on_the_street_as_a_player(state: &AppState, headers: &HeaderMap, now: i64) -> bool {
    auth::session_cookie(headers).is_some()
        && auth::identify(headers, state).is_ok_and(|caller| {
            state
                .db
                .own_player_is_active(&caller.person_id, COMMONS_ID, now - PLAYER_ACTIVE_SECS)
                .unwrap_or(false)
        })
}

/// W34-W45, W52: notes who passed through. Looks at the method, path, user agent and address, keeps
/// none of them, and changes nothing about the reply (W38).
pub fn note_request(state: &AppState, method: &Method, path: &str, headers: &HeaderMap, now: i64) {
    let Some(place) = place_of(method, path) else {
        return;
    };
    let agent = agent_of(headers);
    let street = state.worlds.clone_commons_slot();
    match (place, agent) {
        (Place::Commons, None)
            if is_a_page_watching(headers) && !is_on_the_street_as_a_player(state, headers, now) =>
        {
            person(state, headers, now, street);
        }
        (Place::Commons, _) => {}
        // A crawler fetching a page is a frog.
        (Place::Page, Some(world::AgentKind::Crawler(crawler))) => {
            let (appeared, due) = state.worlds.note_agent(crawler.id, true, now);
            if let Some(visits) = due {
                record_sightings(state, crawler.id, visits, now);
            }
            if appeared {
                if let Some(slot) = street {
                    slot.stale.store(true, Ordering::Release);
                }
            }
        }
        // An agent working for someone right now is a visitor, on a page or off it.
        (_, Some(world::AgentKind::Fetcher(fetcher))) => {
            visitor(state, headers, now, street);
            let (_, due) = state.worlds.note_agent(fetcher.id, false, now);
            if let Some(visits) = due {
                record_sightings(state, fetcher.id, visits, now);
            }
        }
        // W41: only agents read these, so whatever it calls itself, it's a visitor.
        (Place::AgentOnly, Some(world::AgentKind::Crawler(_))) | (Place::AgentOnly, None) => {
            visitor(state, headers, now, street);
        }
        (Place::Page, None) => {}
    }
}

fn visitor(state: &AppState, headers: &HeaderMap, now: i64, street: Option<Arc<Slot>>) {
    let key = address_key(state, headers);
    if state.worlds.note_visitor(key, now) {
        if let Some(slot) = street {
            slot.stale.store(true, Ordering::Release);
        }
    }
}

fn person(state: &AppState, headers: &HeaderMap, now: i64, street: Option<Arc<Slot>>) {
    let key = address_key(state, headers);
    if state.worlds.note_person(key, now) {
        if let Some(slot) = street {
            slot.stale.store(true, Ordering::Release);
        }
    }
}

fn address_key(state: &AppState, headers: &HeaderMap) -> u64 {
    let address =
        address_of(state, headers).map_or_else(|| "local".to_string(), |ip| ip.to_string());
    state.worlds.visitor_key(&address)
}

impl Runtime {
    /// The Commons' view slot if it has one yet, to say it should be rebuilt.
    fn clone_commons_slot(&self) -> Option<Arc<Slot>> {
        locked(&self.views).get(COMMONS_ID).cloned()
    }
}

/// The middleware that watches. It runs before the request is answered, doesn't read the body,
/// and returns what the rest of the server returns, untouched.
pub async fn observe(State(state): State<AppState>, request: Request, next: Next) -> Response {
    note_request(&state, request.method(), request.uri().path(), request.headers(), now());
    next.run(request).await
}

/// Reads the body of a request that may be empty: nothing is the same as `{}`.
pub fn optional_body<T: for<'de> Deserialize<'de> + Default>(body: &Bytes) -> ApiResult<T> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|_| ApiError::bad_request(BAD_MOVE))
}

#[cfg(test)]
mod tests {
    //! The small parts that need no server: the limiter's windows and size, and reading a move.

    use super::*;

    #[test]
    fn a_limiter_counts_per_key_per_window_in_a_fixed_amount_of_memory() {
        let limiter = Limiter::new(8, 60, 3);
        assert!((0..3).all(|_| limiter.allow("a", 100)));
        assert!(!limiter.allow("a", 101), "the fourth in the window");
        assert!(limiter.allow("a", 160), "a new window starts at the minute");
        // A flood of different keys shares the same eight counters, and can't grow them.
        for n in 0..10_000 {
            limiter.allow(&format!("address-{n}"), 200);
        }
        assert_eq!(locked(&limiter.counters).len(), 8);
        // A window a long way off in either direction is just a different window.
        assert!(limiter.allow("b", i64::MAX));
        assert!(limiter.allow("b", i64::MIN + 1));
    }

    fn parse(json: serde_json::Value) -> Result<Parsed, &'static str> {
        parse_move(&serde_json::from_value::<Move>(json).unwrap())
    }

    #[test]
    fn a_drag_in_any_direction_is_the_same_span_with_the_top_left_corner_first() {
        for (x, y, x2, y2) in [(9, 9, 5, 5), (5, 5, 9, 9), (9, 5, 5, 9), (5, 9, 9, 5)] {
            let Ok(Parsed::Zone(Kind::Shops, span)) =
                parse(json!({ "action": "zone", "kind": "shops", "x": x, "y": y, "x2": x2, "y2": y2 }))
            else {
                panic!("a zone");
            };
            assert_eq!((span.x, span.y, span.x2, span.y2), (5, 5, 9, 9));
        }
        let Ok(Parsed::Road(span)) = parse(json!({ "action": "road", "x": 7, "y": 2 })) else {
            panic!("a road");
        };
        assert_eq!((span.x, span.y, span.x2, span.y2), (7, 2, 7, 2), "no far end is one tile");
        // The extremes of a number are only ever compared, never added.
        let Ok(Parsed::Road(span)) = parse(json!({ "action": "road", "x": i64::MAX, "y": 0, "x2": i64::MIN, "y2": 0 }))
        else {
            panic!("a road");
        };
        assert_eq!((span.x, span.x2), (i64::MIN, i64::MAX));
    }

    #[test]
    fn a_move_takes_only_what_its_action_takes() {
        assert!(parse(json!({ "action": "park", "x": 1, "y": 2 })).is_ok());
        assert!(parse(json!({ "action": "park", "x": 1, "y": 2, "x2": 3, "y2": 4 })).is_err());
        assert!(parse(json!({ "action": "zone", "x": 1, "y": 2 })).is_err(), "a zone needs a kind");
        assert!(parse(json!({ "action": "zone", "kind": "park", "x": 1, "y": 2 })).is_err());
        assert!(parse(json!({ "action": "road", "x": 1, "y": 2, "x2": 3 })).is_err());
        assert!(parse(json!({ "action": "emote", "emote": "dance" })).is_ok());
        assert!(parse(json!({ "action": "emote", "emote": "dance", "x": 1 })).is_err());
        assert!(parse(json!({ "action": "walk", "x": 1 })).is_err());
        assert!(serde_json::from_value::<Move>(json!({ "action": "walk", "x": 1, "y": 2, "z": 3 })).is_err());
    }

    #[test]
    fn the_day_number_changes_at_midnight_utc() {
        assert_eq!(today(0), 0);
        assert_eq!(today(86_399), 0);
        assert_eq!(today(86_400), 1);
        assert_eq!(today(-1), -1);
    }
}
