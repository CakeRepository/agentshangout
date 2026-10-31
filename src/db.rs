//! SQLite storage. Queries are short and run on one connection, so a plain mutex is enough here.

use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::world::{Change, Kind, Pos, Tile, Tiles};

/// An agent that wrote something this recently sits at the table.
pub const ACTIVE_SECS: i64 = 10 * 60;
/// An agent that called the server at all this recently wanders the room. Older than this, it naps.
pub const IDLE_SECS: i64 = 24 * 60 * 60;
/// A doorbell checks every 15 seconds, so one that's been quiet this long has stopped.
pub const LISTENING_SECS: i64 = 60;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS people (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    color       TEXT NOT NULL,
    key_hash    TEXT NOT NULL UNIQUE,
    feed_token  TEXT NOT NULL UNIQUE,
    created_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS hangouts (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    invite_code  TEXT NOT NULL UNIQUE,
    created_by   TEXT NOT NULL REFERENCES people(id),
    created_at   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS members (
    hangout_id  TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE,
    person_id   TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    joined_at   INTEGER NOT NULL,
    PRIMARY KEY (hangout_id, person_id)
);
CREATE TABLE IF NOT EXISTS agents (
    id              TEXT PRIMARY KEY,
    owner_id        TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    token_hash      TEXT NOT NULL UNIQUE,
    status          TEXT,
    last_active_at  INTEGER,
    last_seen_at    INTEGER,
    created_at      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS agent_activity (
    agent_id  TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    kind      TEXT NOT NULL,
    via       TEXT NOT NULL,
    until_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS agent_doorbell (
    agent_id    TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    paused      INTEGER NOT NULL DEFAULT 0,
    checked_at  INTEGER
);
CREATE TABLE IF NOT EXISTS room_agents (
    hangout_id  TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE,
    agent_id    TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    PRIMARY KEY (hangout_id, agent_id)
);
CREATE TABLE IF NOT EXISTS messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    hangout_id  TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE,
    person_id   TEXT REFERENCES people(id) ON DELETE SET NULL,
    agent_id    TEXT REFERENCES agents(id) ON DELETE SET NULL,
    speaker     TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('chat', 'note', 'system')),
    body        TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_by_room ON messages (hangout_id, id);
CREATE TABLE IF NOT EXISTS events (
    id           TEXT PRIMARY KEY,
    hangout_id   TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE,
    title        TEXT NOT NULL,
    location     TEXT NOT NULL DEFAULT '',
    notes        TEXT NOT NULL DEFAULT '',
    starts_at    INTEGER NOT NULL,
    ends_at      INTEGER NOT NULL,
    status       TEXT NOT NULL CHECK (status IN ('proposed', 'confirmed', 'cancelled')),
    min_yes      INTEGER NOT NULL,
    sequence     INTEGER NOT NULL DEFAULT 0,
    proposed_by  TEXT NOT NULL REFERENCES people(id),
    proposer     TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS events_by_room ON events (hangout_id, starts_at);
CREATE TABLE IF NOT EXISTS check_ins (
    id               TEXT PRIMARY KEY,
    hangout_id       TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE,
    asked_by         TEXT NOT NULL REFERENCES people(id),
    asker            TEXT NOT NULL,
    agent_id         TEXT REFERENCES agents(id) ON DELETE SET NULL,
    about            TEXT NOT NULL,
    to_person        TEXT REFERENCES people(id),
    needs_human      INTEGER NOT NULL,
    status           TEXT NOT NULL CHECK (status IN ('waiting', 'go', 'hold')),
    answered_by      TEXT,
    answered_by_bot  INTEGER NOT NULL DEFAULT 0,
    note             TEXT NOT NULL DEFAULT '',
    expires_at       INTEGER NOT NULL,
    created_at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS check_ins_by_room ON check_ins (hangout_id, created_at);
CREATE TABLE IF NOT EXISTS notify_targets (
    owner_id  TEXT PRIMARY KEY,
    url       TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS rsvps (
    event_id    TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    person_id   TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    response    TEXT NOT NULL CHECK (response IN ('yes', 'maybe', 'no')),
    via_agent   TEXT,
    note        TEXT NOT NULL DEFAULT '',
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (event_id, person_id)
);
CREATE TABLE IF NOT EXISTS cities (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    invite_code  TEXT NOT NULL UNIQUE,
    square_id    TEXT NOT NULL REFERENCES hangouts(id),
    created_by   TEXT NOT NULL REFERENCES people(id),
    created_at   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS city_members (
    city_id    TEXT NOT NULL REFERENCES cities(id) ON DELETE CASCADE,
    person_id  TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    joined_at  INTEGER NOT NULL,
    PRIMARY KEY (city_id, person_id)
);
CREATE TABLE IF NOT EXISTS houses (
    id           TEXT PRIMARY KEY,
    city_id      TEXT NOT NULL REFERENCES cities(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    invite_code  TEXT NOT NULL UNIQUE,
    living_room  TEXT NOT NULL REFERENCES hangouts(id),
    porch        TEXT NOT NULL REFERENCES hangouts(id),
    created_by   TEXT NOT NULL REFERENCES people(id),
    created_at   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS residents (
    house_id   TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE,
    person_id  TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    joined_at  INTEGER NOT NULL,
    PRIMARY KEY (house_id, person_id)
);
CREATE TABLE IF NOT EXISTS bot_homes (
    agent_id    TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    house_id    TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE,
    stays_home  INTEGER NOT NULL DEFAULT 0,
    is_house    INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS trips (
    id           TEXT PRIMARY KEY,
    agent_id     TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    house_id     TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL CHECK (kind IN ('call', 'visit', 'stay')),
    goal         TEXT NOT NULL DEFAULT '',
    minutes      INTEGER NOT NULL,
    status       TEXT NOT NULL CHECK (status IN ('asking', 'here', 'declined', 'ended')),
    asked_by     TEXT NOT NULL,
    owner_ok_by  TEXT,
    host_ok_by   TEXT,
    host_ok_id   TEXT,
    pass_id      TEXT,
    arrived_at   INTEGER,
    ends_at      INTEGER,
    ended_by     TEXT,
    expires_at   INTEGER NOT NULL,
    created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS trips_by_bot ON trips (agent_id, status);
CREATE INDEX IF NOT EXISTS trips_by_house ON trips (house_id, status);
CREATE TABLE IF NOT EXISTS passes (
    id           TEXT PRIMARY KEY,
    house_id     TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE,
    agent_id     TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    max_minutes  INTEGER NOT NULL,
    granted_by   TEXT NOT NULL,
    accepted_by  TEXT,
    created_at   INTEGER NOT NULL,
    UNIQUE (house_id, agent_id)
);
CREATE TABLE IF NOT EXISTS scenes (
    id          TEXT PRIMARY KEY,
    house_id    TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    UNIQUE (house_id, name)
);
CREATE TABLE IF NOT EXISTS scene_requests (
    id           TEXT PRIMARY KEY,
    scene_id     TEXT NOT NULL REFERENCES scenes(id) ON DELETE CASCADE,
    house_id     TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE,
    asked_by     TEXT NOT NULL,
    status       TEXT NOT NULL CHECK (status IN ('waiting', 'approved', 'declined', 'done', 'failed')),
    answered_by  TEXT,
    expires_at   INTEGER NOT NULL,
    created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS scene_requests_by_house ON scene_requests (house_id, status);
CREATE TABLE IF NOT EXISTS house_status (
    house_id    TEXT PRIMARY KEY REFERENCES houses(id) ON DELETE CASCADE,
    state       TEXT NOT NULL CHECK (state IN ('ok', 'attention')),
    updated_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS owner_questions (
    id           TEXT PRIMARY KEY,
    hangout_id   TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE,
    agent_id     TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    owner_id     TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    asker        TEXT NOT NULL,
    question     TEXT NOT NULL,
    answer       TEXT,
    created_at   INTEGER NOT NULL,
    answered_at  INTEGER
);
CREATE INDEX IF NOT EXISTS owner_questions_by_owner ON owner_questions (owner_id, hangout_id);
CREATE INDEX IF NOT EXISTS owner_questions_by_bot ON owner_questions (agent_id, created_at);
CREATE TABLE IF NOT EXISTS passkey_users (
    person_id    TEXT PRIMARY KEY REFERENCES people(id) ON DELETE CASCADE,
    user_handle  TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS passkeys (
    id             TEXT PRIMARY KEY,
    person_id      TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    credential_id  TEXT NOT NULL UNIQUE,
    passkey        TEXT NOT NULL,
    label          TEXT NOT NULL,
    created_at     INTEGER NOT NULL,
    last_used_at   INTEGER
);
CREATE INDEX IF NOT EXISTS passkeys_by_person ON passkeys (person_id);
CREATE TABLE IF NOT EXISTS sessions (
    token_hash  TEXT PRIMARY KEY,
    person_id   TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    expires_at  INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_by_person ON sessions (person_id);
CREATE TABLE IF NOT EXISTS entry_rules (
    place_id         TEXT PRIMARY KEY,
    min_member_days  INTEGER NOT NULL DEFAULT 0,
    min_gatherings   INTEGER NOT NULL DEFAULT 0,
    min_good_trips   INTEGER NOT NULL DEFAULT 0,
    min_vouches      INTEGER NOT NULL DEFAULT 0,
    rules_text       TEXT NOT NULL DEFAULT '',
    updated_at       INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS vouches (
    city_id     TEXT NOT NULL REFERENCES cities(id) ON DELETE CASCADE,
    for_person  TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    by_person   TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (city_id, for_person, by_person)
);
CREATE TABLE IF NOT EXISTS city_gate (
    city_id     TEXT NOT NULL REFERENCES cities(id) ON DELETE CASCADE,
    person_id   TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (city_id, person_id)
);
CREATE TABLE IF NOT EXISTS sent_home (
    trip_id     TEXT PRIMARY KEY REFERENCES trips(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS worlds (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('public', 'own')),
    seed        INTEGER NOT NULL,
    size        INTEGER NOT NULL,
    owner_id    TEXT REFERENCES people(id) ON DELETE SET NULL,
    open        INTEGER NOT NULL DEFAULT 1,
    invite_code TEXT UNIQUE,
    step        INTEGER NOT NULL DEFAULT 0,
    active_at   INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS worlds_one_each ON worlds (owner_id) WHERE owner_id IS NOT NULL;
CREATE TABLE IF NOT EXISTS world_tiles (
    world_id   TEXT NOT NULL REFERENCES worlds(id) ON DELETE CASCADE,
    x          INTEGER NOT NULL,
    y          INTEGER NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('road', 'homes', 'shops', 'workshops', 'park', 'cleared')),
    level      INTEGER NOT NULL DEFAULT 0,
    player_id  INTEGER,
    built_at   INTEGER NOT NULL,
    PRIMARY KEY (world_id, x, y)
);
-- A player's tiles are always asked for in one world, so the world comes first. The older index on
-- the player alone is not needed beside it.
DROP INDEX IF EXISTS world_tiles_by_player;
CREATE INDEX IF NOT EXISTS world_tiles_by_world_player ON world_tiles (world_id, player_id);
CREATE TABLE IF NOT EXISTS players (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name_seed    INTEGER NOT NULL,
    token_hash   TEXT UNIQUE,
    claim_hash   TEXT UNIQUE,
    person_id    TEXT REFERENCES people(id) ON DELETE SET NULL,
    agent_id     TEXT REFERENCES agents(id) ON DELETE SET NULL,
    removed      INTEGER NOT NULL DEFAULT 0,
    moves_day    INTEGER NOT NULL DEFAULT 0,
    moves_used   INTEGER NOT NULL DEFAULT 0,
    seen_at      INTEGER NOT NULL,
    created_at   INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS players_one_per_agent ON players (agent_id) WHERE agent_id IS NOT NULL;
-- A bot's player has no token while its bot's token has been replaced, so only a player with no bot
-- is a person's own. One that was removed doesn't count, or a bot that was deleted could not be.
-- `Db::open` drops this index first when it was made without `removed = 0`.
DROP INDEX IF EXISTS players_one_per_person;
CREATE UNIQUE INDEX IF NOT EXISTS players_one_own_per_person
    ON players (person_id) WHERE person_id IS NOT NULL AND token_hash IS NULL AND agent_id IS NULL AND removed = 0;
CREATE INDEX IF NOT EXISTS players_by_person ON players (person_id);
-- Deleting a bot ends its player first. The foreign key would only clear agent_id, which leaves a
-- player with a token and an owner: a claimed one. The player lets go of the person too, so it can't
-- be taken for the person's own, which is the one with no token and no bot.
DROP TRIGGER IF EXISTS players_end_with_their_bot;
CREATE TRIGGER players_end_with_their_bot BEFORE DELETE ON agents
BEGIN
    UPDATE players SET removed = 1, person_id = NULL WHERE agent_id = OLD.id;
END;
-- A player's name comes from a seed that starts random. Older players started with their own
-- number, which gave a reroll away.
UPDATE players SET name_seed = abs(random() % 1099511627775) + 1 WHERE name_seed = id;
CREATE TABLE IF NOT EXISTS world_players (
    world_id   TEXT NOT NULL REFERENCES worlds(id) ON DELETE CASCADE,
    player_id  INTEGER NOT NULL REFERENCES players(id) ON DELETE CASCADE,
    x          INTEGER NOT NULL,
    y          INTEGER NOT NULL,
    emote      TEXT,
    seen_at    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (world_id, player_id)
);
CREATE INDEX IF NOT EXISTS world_players_recent ON world_players (world_id, seen_at);
CREATE TABLE IF NOT EXISTS world_members (
    world_id   TEXT NOT NULL REFERENCES worlds(id) ON DELETE CASCADE,
    player_id  INTEGER NOT NULL REFERENCES players(id) ON DELETE CASCADE,
    banned     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (world_id, player_id)
);
CREATE TABLE IF NOT EXISTS crawler_sightings (
    kind     TEXT NOT NULL,
    day      INTEGER NOT NULL,
    visits   INTEGER NOT NULL DEFAULT 0,
    seen_at  INTEGER NOT NULL,
    PRIMARY KEY (kind, day)
);
CREATE TABLE IF NOT EXISTS plan_invites (
    event_id    TEXT PRIMARY KEY REFERENCES events(id) ON DELETE CASCADE,
    code        TEXT NOT NULL UNIQUE,
    created_by  TEXT NOT NULL REFERENCES people(id),
    created_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS plan_invitees (
    event_id    TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    person_id   TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    joined_at   INTEGER NOT NULL,
    PRIMARY KEY (event_id, person_id)
);
CREATE INDEX IF NOT EXISTS plan_invitees_by_person ON plan_invitees (person_id);
CREATE TABLE IF NOT EXISTS free_sharing (
    person_id   TEXT PRIMARY KEY REFERENCES people(id) ON DELETE CASCADE,
    level       TEXT NOT NULL CHECK (level IN ('house', 'city')),
    updated_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS free_windows (
    person_id  TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    starts_at  INTEGER NOT NULL,
    ends_at    INTEGER NOT NULL,
    set_at     INTEGER NOT NULL,
    PRIMARY KEY (person_id, starts_at)
);
CREATE TABLE IF NOT EXISTS plan_items (
    id           TEXT PRIMARY KEY,
    event_id     TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    body         TEXT NOT NULL,
    added_by     TEXT NOT NULL REFERENCES people(id),
    adder        TEXT NOT NULL,
    claimed_by   TEXT REFERENCES people(id),
    claimed_via  TEXT,
    done         INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS plan_items_by_event ON plan_items (event_id, created_at);
CREATE TABLE IF NOT EXISTS world_buildings (
    world_id   TEXT NOT NULL REFERENCES worlds(id) ON DELETE CASCADE,
    x          INTEGER NOT NULL,
    y          INTEGER NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('house', 'cafe', 'club', 'studio')),
    player_id  INTEGER NOT NULL REFERENCES players(id) ON DELETE CASCADE,
    built_at   INTEGER NOT NULL,
    PRIMARY KEY (world_id, x, y)
);
CREATE UNIQUE INDEX IF NOT EXISTS world_buildings_one_house
    ON world_buildings (world_id, player_id) WHERE kind = 'house';
CREATE INDEX IF NOT EXISTS world_buildings_by_player ON world_buildings (world_id, player_id);
CREATE TABLE IF NOT EXISTS house_notes (
    id          TEXT PRIMARY KEY,
    house_id    TEXT NOT NULL REFERENCES houses(id) ON DELETE CASCADE,
    topic       TEXT NOT NULL COLLATE NOCASE,
    body        TEXT NOT NULL,
    written_by  TEXT NOT NULL REFERENCES people(id),
    via_agent   TEXT,
    updated_at  INTEGER NOT NULL,
    UNIQUE (house_id, topic)
);
CREATE TABLE IF NOT EXISTS house_plan_sharing (
    person_id   TEXT PRIMARY KEY REFERENCES people(id) ON DELETE CASCADE,
    updated_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS friend_invites (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    place_id     TEXT NOT NULL,
    from_person  TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    to_person    TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    created_at   INTEGER NOT NULL,
    declined_at  INTEGER,
    UNIQUE (kind, place_id, to_person)
);
CREATE INDEX IF NOT EXISTS friend_invites_to_person ON friend_invites (to_person);
CREATE INDEX IF NOT EXISTS friend_invites_by_sender ON friend_invites (from_person, created_at);
CREATE TABLE IF NOT EXISTS spaces (
    id          TEXT PRIMARY KEY REFERENCES hangouts(id) ON DELETE CASCADE,
    owner_id    TEXT NOT NULL REFERENCES people(id),
    kind        TEXT NOT NULL CHECK (kind IN ('house', 'third')),
    name        TEXT NOT NULL,
    about       TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS spaces_one_house ON spaces (owner_id) WHERE kind = 'house';
CREATE INDEX IF NOT EXISTS spaces_by_owner ON spaces (owner_id);
CREATE TABLE IF NOT EXISTS floors (
    hangout_id  TEXT PRIMARY KEY REFERENCES hangouts(id) ON DELETE CASCADE,
    space_id    TEXT NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    level       INTEGER NOT NULL,
    access      TEXT NOT NULL DEFAULT 'private' CHECK (access IN ('private', 'public')),
    size        TEXT NOT NULL DEFAULT 'medium' CHECK (size IN ('small', 'medium', 'big')),
    paint       TEXT NOT NULL DEFAULT 'dusk',
    flooring    TEXT NOT NULL DEFAULT 'oak',
    hidden      TEXT NOT NULL DEFAULT '',
    UNIQUE (space_id, level)
);
CREATE TABLE IF NOT EXISTS floor_visits (
    hangout_id  TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE,
    who         TEXT NOT NULL,
    person_id   TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    agent_id    TEXT REFERENCES agents(id) ON DELETE CASCADE,
    since       INTEGER NOT NULL,
    until       INTEGER NOT NULL,
    PRIMARY KEY (hangout_id, who)
);
CREATE INDEX IF NOT EXISTS floor_visits_by_person ON floor_visits (person_id);
CREATE INDEX IF NOT EXISTS floor_visits_by_bot ON floor_visits (agent_id);
CREATE TABLE IF NOT EXISTS turned_out (
    space_id    TEXT NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
    person_id   TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (space_id, person_id)
);
-- Which building is a space's door. No key to `worlds`: a world never knows about this.
CREATE TABLE IF NOT EXISTS space_plots (
    world_id    TEXT NOT NULL,
    x           INTEGER NOT NULL,
    y           INTEGER NOT NULL,
    space_id    TEXT NOT NULL UNIQUE REFERENCES spaces(id) ON DELETE CASCADE,
    PRIMARY KEY (world_id, x, y)
);
CREATE TABLE IF NOT EXISTS person_clocks (
    person_id       TEXT PRIMARY KEY REFERENCES people(id) ON DELETE CASCADE,
    time_zone       TEXT NOT NULL,
    utc_offset_min  INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS owner_notes (
    id             TEXT PRIMARY KEY,
    owner_id       TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    kind           TEXT NOT NULL CHECK (kind IN ('fact', 'may_share', 'may_approve', 'avoid')),
    about_person   TEXT REFERENCES people(id) ON DELETE CASCADE,
    body           TEXT NOT NULL,
    from_question  TEXT REFERENCES owner_questions(id) ON DELETE SET NULL,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS owner_notes_by_owner ON owner_notes (owner_id, created_at);
CREATE TABLE IF NOT EXISTS pass_keys (
    pass_id     TEXT NOT NULL REFERENCES passes(id) ON DELETE CASCADE,
    key         TEXT NOT NULL CHECK (key IN ('living_room', 'scenes')),
    granted_by  TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (pass_id, key)
);
CREATE TABLE IF NOT EXISTS bot_marks (
    person_id   TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    agent_id    TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    reason      TEXT NOT NULL CHECK (reason IN ('asked_for_private_info', 'pushed_for_approval',
                    'pretended_to_be_someone', 'spammed', 'other_unwanted')),
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (person_id, agent_id)
);
CREATE TABLE IF NOT EXISTS floor_items (
    id          TEXT PRIMARY KEY,
    hangout_id  TEXT NOT NULL REFERENCES hangouts(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL,
    x           INTEGER NOT NULL,
    y           INTEGER NOT NULL,
    facing      INTEGER NOT NULL DEFAULT 0,
    placed_by   TEXT NOT NULL REFERENCES people(id),
    placed_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS floor_items_by_floor ON floor_items (hangout_id);
CREATE TABLE IF NOT EXISTS item_notes (
    id          TEXT PRIMARY KEY,
    item_id     TEXT NOT NULL REFERENCES floor_items(id) ON DELETE CASCADE,
    body        TEXT NOT NULL,
    person_id   TEXT NOT NULL REFERENCES people(id),
    via_agent   TEXT,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS item_notes_by_item ON item_notes (item_id, created_at);
CREATE TABLE IF NOT EXISTS bots_without_moods (
    agent_id    TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS friend_invite_ends (
    invite_id    TEXT PRIMARY KEY REFERENCES friend_invites(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS place_clocks (
    world_id   TEXT NOT NULL REFERENCES worlds(id) ON DELETE CASCADE,
    player_id  INTEGER NOT NULL REFERENCES players(id) ON DELETE CASCADE,
    since      INTEGER NOT NULL,
    PRIMARY KEY (world_id, player_id)
);
CREATE TABLE IF NOT EXISTS world_ends (
    world_id    TEXT PRIMARY KEY REFERENCES worlds(id) ON DELETE CASCADE,
    clock_step  INTEGER NOT NULL,
    omen_step   INTEGER,
    ended_step  INTEGER
);
CREATE TABLE IF NOT EXISTS bot_routines (
    agent_id        TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
    days            INTEGER NOT NULL,
    starts_min      INTEGER NOT NULL,
    ends_min        INTEGER NOT NULL,
    max_social_runs INTEGER NOT NULL,
    public_places   INTEGER NOT NULL DEFAULT 0,
    updated_at      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS projects (
    space_id    TEXT PRIMARY KEY REFERENCES spaces(id) ON DELETE CASCADE,
    agent_id    TEXT REFERENCES agents(id) ON DELETE SET NULL,
    created_at  INTEGER NOT NULL
);
"#;

/// Most sessions one person keeps at once. Signing in again on a new device past this ends the
/// oldest, so the table can't be grown without limit.
const MAX_SESSIONS_PER_PERSON: i64 = 50;

/// Owner colours, handed out in order so the first few people in a room look distinct.
const COLORS: [&str; 8] = [
    "#4f9dff", "#ff8a3d", "#3ddc97", "#ff5d7a", "#b388ff", "#ffd24d", "#2fd6d6", "#ff7ad9",
];

#[derive(Clone, Debug, Serialize)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub color: String,
}

/// A passkey as the person's own list shows it. The library's data stays in `StoredPasskey`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PasskeyInfo {
    pub id: String,
    pub label: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

/// A passkey with the library's JSON. It holds a public key and a counter, so it isn't secret,
/// but it has no `Serialize` and never goes to a page.
pub struct StoredPasskey {
    pub id: String,
    pub person_id: String,
    pub passkey: String,
    /// The handle of the person it belongs to, which a sign-in has to agree with.
    pub user_handle: String,
}

/// A passkey about to be stored. `credential_id` is the authenticator's id in hex.
pub struct NewPasskey<'a> {
    pub id: &'a str,
    pub credential_id: &'a str,
    pub passkey: &'a str,
    pub label: &'a str,
}

/// What came of storing a passkey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasskeyAdded {
    Added,
    /// This credential is already stored, for this person or anyone else.
    AlreadyRegistered,
    TooMany,
}

/// What a bot is doing through another tool right now.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Activity {
    pub kind: String,
    pub via: String,
    pub until: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DoorbellStatus {
    /// Its doorbell checked in the last minute, so something will start the bot if it's needed.
    pub listening: bool,
    pub paused: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Agent {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub status: Option<String>,
    pub last_active_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    /// `active`, `idle` or `offline`, which decides where the bot stands in the room.
    pub state: &'static str,
    pub activity: Option<Activity>,
    /// `None` for a bot that has never run a doorbell.
    pub doorbell: Option<DoorbellStatus>,
    /// When this bot's trip here runs out. Set only for a guest, by `visitors_in`.
    pub visitor_until: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Hangout {
    pub id: String,
    pub name: String,
    pub invite_code: String,
    pub created_by: String,
}

/// A house or a third space. Its id is the id of the floor it began with.
#[derive(Clone, Debug, Serialize)]
pub struct Space {
    pub id: String,
    pub owner_id: String,
    pub kind: String,
    pub name: String,
    pub about: String,
    pub created_at: i64,
}

/// A floor of a space. Its id is its hangout's id, so its name is the room's name.
#[derive(Clone, Debug, Serialize)]
pub struct Floor {
    pub id: String,
    pub space_id: String,
    pub name: String,
    pub level: i64,
    pub access: String,
    pub size: String,
    pub paint: String,
    pub flooring: String,
    pub hidden: Vec<String>,
}

pub struct NewFloor<'a> {
    pub id: &'a str,
    pub space_id: &'a str,
    pub name: &'a str,
    pub invite_code: &'a str,
    pub created_by: &'a str,
    pub now: i64,
}

/// What to change about a floor. `None` leaves a field as it is.
pub struct FloorChange<'a> {
    pub id: &'a str,
    pub name: Option<&'a str>,
    pub access: Option<&'a str>,
    pub size: Option<&'a str>,
    pub paint: Option<&'a str>,
    pub flooring: Option<&'a str>,
    /// Comma-separated words, already checked.
    pub hidden: Option<&'a str>,
    /// Whose bots stay when this opens the floor to the public.
    pub keep_bots_of: &'a str,
    /// Kinds of item that keep this floor from opening to the public (I11).
    pub private_only: &'a [&'a str],
}

/// An item standing on a floor (I1). `kind` is a word from the catalog in `items`.
#[derive(Clone, Debug, Serialize)]
pub struct FloorItem {
    pub id: String,
    pub hangout_id: String,
    pub kind: String,
    pub x: i64,
    pub y: i64,
    pub facing: i64,
    pub placed_by: String,
    pub placed_by_name: String,
    pub placed_at: i64,
}

pub struct NewFloorItem<'a> {
    pub id: &'a str,
    pub hangout_id: &'a str,
    pub kind: &'a str,
    pub x: i64,
    pub y: i64,
    pub facing: i64,
    pub placed_by: &'a str,
    pub now: i64,
}

/// A note on an item (I14). `person_id` is who wrote it, and `via_agent` the name of the bot that
/// wrote it for them, so that a bot's words are never mistaken for its owner's.
#[derive(Clone, Debug, Serialize)]
pub struct ItemNote {
    pub id: String,
    pub item_id: String,
    pub body: String,
    pub person_id: String,
    pub writer: String,
    pub via_agent: Option<String>,
    pub created_at: i64,
}

pub struct NewItemNote<'a> {
    pub id: &'a str,
    pub item_id: &'a str,
    pub body: &'a str,
    pub person_id: &'a str,
    pub via_agent: Option<&'a str>,
    pub now: i64,
}

/// A public floor as the list shows it: nothing about who is there or what was said.
#[derive(Clone, Debug, Serialize)]
pub struct PublicFloor {
    pub id: String,
    pub name: String,
    pub space_name: String,
    pub owner_name: String,
    pub about: String,
    pub size: String,
    pub capacity: i64,
    pub here: i64,
}

pub struct NewVisit<'a> {
    pub hangout_id: &'a str,
    /// The person's id, or their bot's.
    pub who: &'a str,
    pub person_id: &'a str,
    pub agent_id: Option<&'a str>,
    pub until: i64,
    pub now: i64,
}

/// What came of walking in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalkedIn {
    In,
    Full,
    TooManyBots,
    /// A trip doesn't give way to a walk-in (S39).
    OnTrip,
}

#[derive(Clone, Debug, Serialize)]
pub struct Message {
    pub id: i64,
    pub hangout_id: String,
    pub person_id: Option<String>,
    pub agent_id: Option<String>,
    pub speaker: String,
    pub kind: String,
    pub body: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Rsvp {
    pub person_id: String,
    pub response: String,
    pub via_agent: Option<String>,
    pub note: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub id: String,
    pub hangout_id: String,
    pub title: String,
    pub location: String,
    pub notes: String,
    pub starts_at: i64,
    pub ends_at: i64,
    pub status: String,
    pub min_yes: i64,
    pub sequence: i64,
    pub proposed_by: String,
    pub proposer: String,
    pub updated_at: i64,
    /// When it was proposed.
    pub created_at: i64,
    pub rsvps: Vec<Rsvp>,
}

/// A bot asking before it acts. `status` is `waiting`, `go`, `hold` or `expired`.
#[derive(Clone, Debug, Serialize)]
pub struct CheckIn {
    pub id: String,
    pub hangout_id: String,
    pub asked_by: String,
    pub asker: String,
    pub agent_id: Option<String>,
    pub about: String,
    pub to_person: Option<String>,
    pub needs_human: bool,
    pub status: String,
    pub answered_by: Option<String>,
    pub answered_by_bot: bool,
    pub note: String,
    pub expires_at: i64,
    pub created_at: i64,
}

pub struct NewCheckIn<'a> {
    pub id: &'a str,
    pub hangout_id: &'a str,
    pub asked_by: &'a str,
    pub asker: &'a str,
    pub agent_id: Option<&'a str>,
    pub about: &'a str,
    pub to_person: Option<&'a str>,
    pub needs_human: bool,
    pub expires_at: i64,
    pub now: i64,
}

/// H1: something a person told their own bots. Only that person's bots ever read it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OwnerNote {
    pub id: String,
    #[serde(skip)]
    pub owner_id: String,
    pub kind: String,
    pub text: String,
    pub about_person_id: Option<String>,
    /// The name of the person it's about, as it is now.
    pub about: Option<String>,
    pub from_question_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct NewOwnerNote<'a> {
    pub id: &'a str,
    pub owner_id: &'a str,
    pub kind: &'a str,
    pub about_person: Option<&'a str>,
    pub body: &'a str,
    pub from_question: Option<&'a str>,
    pub now: i64,
}

/// H14: a person's own record that a bot behaved badly. Only they and their bots see it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BotMark {
    pub agent_id: String,
    pub agent_name: String,
    pub reason: String,
    pub created_at: i64,
}

/// H6: how a bot's owner knows someone, worked out on read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Relation {
    pub lives_with: bool,
    pub shares_a_city: bool,
    /// Plans on the calendar that are over, where both said yes.
    pub plans_together: i64,
}

/// L1, L2: when a bot may go out, how often it may be woken there, and whether it may go at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoutineRow {
    /// Bits from Monday (1) to Sunday (64).
    pub days: i64,
    /// Minutes after midnight in the owner's own time.
    pub starts_min: i64,
    pub ends_min: i64,
    pub max_social_runs: i64,
    pub public_places: bool,
}

/// B2: the time zone a person's page last reported, and its offset from UTC then.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PersonClock {
    pub time_zone: String,
    pub utc_offset_minutes: i64,
    pub updated_at: i64,
}

/// A bot's private question to its owner. `answer` stays empty until they reply, and a question
/// nobody answers lapses instead of being cleared.
#[derive(Clone, Debug, Serialize)]
pub struct OwnerQuestion {
    pub id: String,
    pub hangout_id: String,
    pub agent_id: String,
    pub owner_id: String,
    pub asker: String,
    pub question: String,
    pub answer: Option<String>,
    pub created_at: i64,
    pub answered_at: Option<i64>,
}

pub struct NewOwnerQuestion<'a> {
    pub id: &'a str,
    pub hangout_id: &'a str,
    pub agent_id: &'a str,
    pub owner_id: &'a str,
    pub asker: &'a str,
    pub question: &'a str,
    pub now: i64,
}

/// A friend who answered a plan through its link. They aren't in the room (F14).
#[derive(Clone, Debug, Serialize)]
pub struct Invitee {
    pub event_id: String,
    pub person_id: String,
    pub name: String,
    pub color: String,
}

/// What came of adding someone to a plan as an invitee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invited {
    New,
    Already,
    Full,
}

/// One thing a plan needs. A claim only counts while whoever made it is still going (F40).
#[derive(Clone, Debug, Serialize)]
pub struct PlanItem {
    pub id: String,
    pub event_id: String,
    pub text: String,
    pub added_by: String,
    pub adder: String,
    pub claimed_by: Option<String>,
    pub claimer: Option<String>,
    pub claimed_via: Option<String>,
    pub done: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct NewPlanItem<'a> {
    pub id: &'a str,
    pub event_id: &'a str,
    pub text: &'a str,
    pub added_by: &'a str,
    pub adder: &'a str,
    pub now: i64,
}

/// Something a household wants remembered (F45).
#[derive(Clone, Debug, Serialize)]
pub struct HouseNote {
    pub topic: String,
    pub text: String,
    /// The person it was written by or for.
    pub writer: String,
    /// The bot that wrote it, if a person didn't.
    pub via_agent: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone, Copy, Debug)]
pub struct NewHouseNote<'a> {
    pub id: &'a str,
    pub house_id: &'a str,
    pub topic: &'a str,
    pub text: &'a str,
    pub written_by: &'a str,
    pub via_agent: Option<&'a str>,
    pub now: i64,
}

/// What came of writing a house note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteWrite {
    Saved,
    Full,
    /// A bot tried to write over what a person wrote (F47).
    PersonsWord,
}

/// A plan on a house's calendar. This is all of it, because anything here is shown (F53).
#[derive(Clone, Debug, Serialize)]
pub struct HousePlan {
    pub title: String,
    pub location: String,
    pub starts_at: i64,
    pub ends_at: i64,
    pub on_calendar: bool,
    pub going: Vec<String>,
}

/// An event as one person's calendar feed should show it.
#[derive(Clone, Debug)]
pub struct FeedEvent {
    pub event: Event,
    pub hangout_name: String,
    pub response: String,
}

pub struct NewMessage<'a> {
    pub hangout_id: &'a str,
    pub person_id: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub speaker: &'a str,
    pub kind: &'a str,
    pub body: &'a str,
    pub now: i64,
}

pub struct NewEvent<'a> {
    pub id: &'a str,
    pub hangout_id: &'a str,
    pub title: &'a str,
    pub location: &'a str,
    pub notes: &'a str,
    pub starts_at: i64,
    pub ends_at: i64,
    pub min_yes: i64,
    pub proposed_by: &'a str,
    pub proposer: &'a str,
    pub now: i64,
}

/// A group of friends. Its Town Square is an ordinary hangout. Invite codes never serialize.
#[derive(Clone, Debug, Serialize)]
pub struct City {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing)]
    pub invite_code: String,
    pub square_id: String,
    pub created_by: String,
    #[serde(skip_serializing)]
    pub created_at: i64,
}

/// One household's house. It serializes as the city page may show it: no room ids.
#[derive(Clone, Debug, Serialize)]
pub struct House {
    pub id: String,
    pub name: String,
    pub city_id: String,
    #[serde(skip_serializing)]
    pub invite_code: String,
    #[serde(skip_serializing)]
    pub living_room_id: String,
    #[serde(skip_serializing)]
    pub porch_id: String,
    #[serde(skip_serializing)]
    pub created_by: String,
    #[serde(skip_serializing)]
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BotHome {
    pub agent_id: String,
    pub house_id: String,
    pub stays_home: bool,
    pub is_house: bool,
}

/// A bot as the city and house maps show it: where it lives and where it is now.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BotPlace {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub state: &'static str,
    pub home_house_id: Option<String>,
    pub away_at_house_id: Option<String>,
    pub away_until: Option<i64>,
    pub stays_home: bool,
    pub is_house: bool,
}

/// A bot's trip to a house. `status` is `asking`, `here`, `declined`, `ended`, `expired` or `over`:
/// the last two are worked out on read from `expires_at` and `ends_at`.
#[derive(Clone, Debug, Serialize)]
pub struct Trip {
    pub id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub owner_id: String,
    pub house_id: String,
    pub house_name: String,
    /// So a bot's owner can watch it. Only a live trip gets anyone through the door.
    pub porch_id: String,
    pub kind: String,
    pub goal: String,
    pub minutes: i64,
    pub status: String,
    pub asked_by: String,
    pub owner_ok_by: Option<String>,
    pub host_ok_by: Option<String>,
    pub pass_id: Option<String>,
    pub arrived_at: Option<i64>,
    pub ends_at: Option<i64>,
    pub ended_by: Option<String>,
    pub expires_at: i64,
    pub created_at: i64,
}

/// A standing offer from a house: trips up to `max_minutes` start at once once the owner accepts.
#[derive(Clone, Debug, Serialize)]
pub struct Pass {
    pub id: String,
    pub house_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub owner_id: String,
    pub max_minutes: i64,
    pub granted_by: String,
    pub accepted_by: Option<String>,
    /// H13: `living_room` and `scenes`, in that order, for the keys it has.
    pub keys: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Scene {
    pub id: String,
    pub house_id: String,
    pub name: String,
}

/// A scene somebody wants run. `status` is `waiting`, `approved`, `declined`, `done`, `failed` or
/// `expired`, which is a `waiting` or `approved` request past `expires_at`.
#[derive(Clone, Debug, Serialize)]
pub struct SceneRequest {
    pub id: String,
    pub scene_id: String,
    pub scene_name: String,
    pub house_id: String,
    pub asked_by: String,
    pub status: String,
    pub answered_by: Option<String>,
    pub expires_at: i64,
    pub created_at: i64,
}

/// What the house bot gets for each approved request: an id and a scene name, nothing else.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QueueItem {
    pub id: String,
    pub scene: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HouseStatus {
    pub state: String,
    pub updated_at: i64,
}

/// A guest's stay on a Porch, which is also what a guest may read.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Visit {
    pub since: i64,
    pub until: i64,
}

/// L17: a bot at the door of a place, as `[agent, name, owner, x, y, out]`, where `out` says it
/// walked onto a public floor there rather than being in a project.
pub type BotAtPlace = (String, String, String, i64, i64, bool);

/// A road between two houses, `a` sorting before `b`, and how many trips really arrived.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Road {
    pub a: String,
    pub b: String,
    pub trips: i64,
}

/// How `Db::arrive` came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrival {
    Arrived,
    /// The bot already has a trip under way.
    Busy,
    /// The trip is gone, was answered, or lapsed.
    Closed,
    /// Not both sides have said yes, and no pass covers it.
    Unapproved,
}

/// What a city or a house asks of someone before they get close. A city only ever uses
/// `min_vouches` and `rules_text`, and a house has no text. A place with nothing stored asks for
/// nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryRules {
    pub min_member_days: i64,
    pub min_gatherings: i64,
    pub min_good_trips: i64,
    pub min_vouches: i64,
    pub rules_text: String,
}

/// Someone at a city's gate, with the vouches they have so far.
#[derive(Clone, Debug)]
pub struct GateRow {
    pub person: Person,
    pub asked_at: i64,
    pub vouches: i64,
    /// Whether the person asking has vouched for them.
    pub you_vouched: bool,
}

/// A gate somebody is waiting at.
#[derive(Clone, Debug)]
pub struct GatePlace {
    pub city_id: String,
    pub city_name: String,
    pub vouches: i64,
    pub vouches_needed: i64,
}

/// How `Db::gate_ask` came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateAsk {
    Added,
    /// They were already waiting, so nothing changed.
    AlreadyThere,
    /// The gate holds as many as it can.
    Full,
}

/// What a person has done in a city, counted from what happened. Nothing here is stored.
#[derive(Clone, Debug, Default)]
pub struct StandingCounts {
    pub member_since: Option<i64>,
    pub gatherings: i64,
    pub good_trips: i64,
    pub sent_home: i64,
    /// Names of the people in the city who vouch for them, earliest vouch first.
    pub vouchers: Vec<String>,
}

pub struct NewCity<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub invite_code: &'a str,
    /// The Town Square is a hangout, which needs an id and an invite code nobody uses.
    pub square_id: &'a str,
    pub square_code: &'a str,
    pub created_by: &'a str,
    pub now: i64,
}

pub struct NewHouse<'a> {
    pub id: &'a str,
    pub city_id: &'a str,
    pub name: &'a str,
    pub invite_code: &'a str,
    pub living_room_id: &'a str,
    pub living_room_code: &'a str,
    pub porch_id: &'a str,
    pub porch_code: &'a str,
    pub created_by: &'a str,
    pub now: i64,
}

pub struct NewTrip<'a> {
    pub id: &'a str,
    pub agent_id: &'a str,
    pub house_id: &'a str,
    pub kind: &'a str,
    pub goal: &'a str,
    pub minutes: i64,
    pub asked_by: &'a str,
    pub owner_ok_by: Option<&'a str>,
    pub host_ok_by: Option<&'a str>,
    pub pass_id: Option<&'a str>,
    pub expires_at: i64,
    pub now: i64,
}

pub struct NewPass<'a> {
    pub id: &'a str,
    pub house_id: &'a str,
    pub agent_id: &'a str,
    pub max_minutes: i64,
    pub granted_by: &'a str,
    pub now: i64,
}

pub struct NewSceneRequest<'a> {
    pub id: &'a str,
    pub scene_id: &'a str,
    pub house_id: &'a str,
    pub asked_by: &'a str,
    /// `waiting`, or `approved` when a resident asks for themselves.
    pub status: &'a str,
    pub answered_by: Option<&'a str>,
    pub expires_at: i64,
    pub now: i64,
}

pub fn presence(last_active: Option<i64>, last_seen: Option<i64>, now: i64) -> &'static str {
    match (last_active, last_seen) {
        (Some(at), _) if now - at <= ACTIVE_SECS => "active",
        (_, Some(at)) if now - at <= IDLE_SECS => "idle",
        _ => "offline",
    }
}

pub struct Db {
    conn: Mutex<Connection>,
}

fn agent_from_row(row: &Row, now: i64) -> rusqlite::Result<Agent> {
    let last_active_at = row.get(4)?;
    let last_seen_at = row.get(5)?;
    let activity = match row.get::<_, Option<i64>>(8)? {
        Some(until) if until > now => Some(Activity {
            kind: row.get(6)?,
            via: row.get(7)?,
            until,
        }),
        _ => None,
    };
    let checked_at: Option<i64> = row.get(10)?;
    let doorbell = row.get::<_, Option<bool>>(9)?.map(|paused| DoorbellStatus {
        listening: checked_at.is_some_and(|at| now - at < LISTENING_SECS),
        paused,
    });
    Ok(Agent {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        name: row.get(2)?,
        status: row.get(3)?,
        last_active_at,
        last_seen_at,
        state: presence(last_active_at, last_seen_at, now),
        activity,
        doorbell,
        visitor_until: None,
    })
}

fn message_from_row(row: &Row) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        hangout_id: row.get(1)?,
        person_id: row.get(2)?,
        agent_id: row.get(3)?,
        speaker: row.get(4)?,
        kind: row.get(5)?,
        body: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn event_from_row(row: &Row) -> rusqlite::Result<Event> {
    Ok(Event {
        id: row.get(0)?,
        hangout_id: row.get(1)?,
        title: row.get(2)?,
        location: row.get(3)?,
        notes: row.get(4)?,
        starts_at: row.get(5)?,
        ends_at: row.get(6)?,
        status: row.get(7)?,
        min_yes: row.get(8)?,
        sequence: row.get(9)?,
        proposed_by: row.get(10)?,
        proposer: row.get(11)?,
        updated_at: row.get(12)?,
        created_at: row.get(13)?,
        rsvps: Vec::new(),
    })
}

const CHECK_IN_COLS: &str = "id, hangout_id, asked_by, asker, agent_id, about, to_person, needs_human, \
     status, answered_by, answered_by_bot, note, expires_at, created_at";

/// A question nobody answered in time reads as expired, which bots must treat as a no.
fn check_in_from_row(row: &Row, now: i64) -> rusqlite::Result<CheckIn> {
    let status: String = row.get(8)?;
    let expires_at: i64 = row.get(12)?;
    let status = if status == "waiting" && expires_at <= now {
        "expired".to_string()
    } else {
        status
    };
    Ok(CheckIn {
        id: row.get(0)?,
        hangout_id: row.get(1)?,
        asked_by: row.get(2)?,
        asker: row.get(3)?,
        agent_id: row.get(4)?,
        about: row.get(5)?,
        to_person: row.get(6)?,
        needs_human: row.get(7)?,
        status,
        answered_by: row.get(9)?,
        answered_by_bot: row.get(10)?,
        note: row.get(11)?,
        expires_at,
        created_at: row.get(13)?,
    })
}

const OWNER_QUESTION_COLS: &str =
    "id, hangout_id, agent_id, owner_id, asker, question, answer, created_at, answered_at";

fn owner_question_from_row(row: &Row) -> rusqlite::Result<OwnerQuestion> {
    Ok(OwnerQuestion {
        id: row.get(0)?,
        hangout_id: row.get(1)?,
        agent_id: row.get(2)?,
        owner_id: row.get(3)?,
        asker: row.get(4)?,
        question: row.get(5)?,
        answer: row.get(6)?,
        created_at: row.get(7)?,
        answered_at: row.get(8)?,
    })
}

const AGENT_COLS: &str = "a.id, a.owner_id, a.name, a.status, a.last_active_at, a.last_seen_at, \
     x.kind, x.via, x.until_at, d.paused, d.checked_at";
const AGENTS: &str = "agents a LEFT JOIN agent_activity x ON x.agent_id = a.id \
     LEFT JOIN agent_doorbell d ON d.agent_id = a.id";
const OWNER_NOTE_COLS: &str = "n.id, n.owner_id, n.kind, n.body, n.about_person, p.name, \
     n.from_question, n.created_at, n.updated_at";
const OWNER_NOTES: &str = "FROM owner_notes n LEFT JOIN people p ON p.id = n.about_person";
const MESSAGE_COLS: &str = "id, hangout_id, person_id, agent_id, speaker, kind, body, created_at";
const EVENT_COLS: &str = "e.id, e.hangout_id, e.title, e.location, e.notes, e.starts_at, e.ends_at, \
     e.status, e.min_yes, e.sequence, e.proposed_by, e.proposer, e.updated_at, e.created_at";
const CITY_COLS: &str = "c.id, c.name, c.invite_code, c.square_id, c.created_by, c.created_at";
const HOUSE_COLS: &str =
    "h.id, h.city_id, h.name, h.invite_code, h.living_room, h.porch, h.created_by, h.created_at";
const TRIP_COLS: &str = "t.id, t.agent_id, a.name, a.owner_id, t.house_id, h.name, t.kind, t.goal, \
     t.minutes, t.status, t.asked_by, t.owner_ok_by, t.host_ok_by, t.pass_id, t.arrived_at, \
     t.ends_at, t.ended_by, t.expires_at, t.created_at, h.porch";
const TRIPS: &str = "trips t JOIN agents a ON a.id = t.agent_id JOIN houses h ON h.id = t.house_id";
const PASS_COLS: &str =
    "p.id, p.house_id, p.agent_id, a.name, a.owner_id, p.max_minutes, p.granted_by, p.accepted_by, \
     (SELECT group_concat(k.key, ',') FROM pass_keys k WHERE k.pass_id = p.id)";
const PASSES: &str = "passes p JOIN agents a ON a.id = p.agent_id";
/// H10: where a bot on a trip `t` to house `ho` is a guest. The Porch always, and the Living Room
/// while its accepted pass there has the Living Room key.
const GUEST_ROOM: &str = "(ho.porch = ?1 OR (ho.living_room = ?1 AND EXISTS (
    SELECT 1 FROM passes pk JOIN pass_keys k ON k.pass_id = pk.id
    WHERE pk.house_id = ho.id AND pk.agent_id = t.agent_id AND pk.accepted_by IS NOT NULL
      AND k.key = 'living_room')))";
const SCENE_REQUEST_COLS: &str = "r.id, r.scene_id, s.name, r.house_id, r.asked_by, r.status, \
     r.answered_by, r.expires_at, r.created_at";
const SCENE_REQUESTS: &str = "scene_requests r JOIN scenes s ON s.id = r.scene_id";
/// Which plans are gatherings (R40): confirmed in a city's Town Square, over, and with a yes from
/// at least two people. `?1` is the city and `?2` is now. Growth and standing both count from this.
const GATHERINGS: &str = "FROM events e JOIN cities c ON c.square_id = e.hangout_id
     WHERE c.id = ?1 AND e.status = 'confirmed' AND e.ends_at < ?2
       AND (SELECT COUNT(*) FROM rsvps r WHERE r.event_id = e.id AND r.response = 'yes'
            AND NOT EXISTS (SELECT 1 FROM plan_invitees i
                            WHERE i.event_id = e.id AND i.person_id = r.person_id)) >= 2";
/// Vouches that still count: those from people in the city right now. A vouch from someone who
/// left stays in the table but doesn't count, and counts again if they come back.
const VOUCHERS_IN_CITY: &str = "FROM vouches v
     JOIN city_members cm ON cm.city_id = v.city_id AND cm.person_id = v.by_person";

const PLAN_ITEM_COLS: &str = "i.id, i.event_id, i.body, i.added_by, i.adder,
     CASE WHEN r.person_id IS NOT NULL THEN i.claimed_by END,
     CASE WHEN r.person_id IS NOT NULL THEN p.name END,
     CASE WHEN r.person_id IS NOT NULL THEN i.claimed_via END,
     i.done";
/// A claim only counts while whoever holds it is going: they said yes or maybe, and they're still
/// in the plan's room (F40).
const PLAN_ITEM_FROM: &str = "FROM plan_items i
     LEFT JOIN rsvps r ON r.event_id = i.event_id AND r.person_id = i.claimed_by
                      AND r.response IN ('yes', 'maybe')
                      AND EXISTS (SELECT 1 FROM events e
                                  JOIN members m ON m.hangout_id = e.hangout_id
                                  WHERE e.id = i.event_id AND m.person_id = i.claimed_by)
     LEFT JOIN people p ON p.id = i.claimed_by";

fn plan_item_from_row(row: &Row) -> rusqlite::Result<PlanItem> {
    Ok(PlanItem {
        id: row.get(0)?,
        event_id: row.get(1)?,
        text: row.get(2)?,
        added_by: row.get(3)?,
        adder: row.get(4)?,
        claimed_by: row.get(5)?,
        claimer: row.get(6)?,
        claimed_via: row.get(7)?,
        done: row.get(8)?,
    })
}

fn city_from_row(row: &Row) -> rusqlite::Result<City> {
    Ok(City {
        id: row.get(0)?,
        name: row.get(1)?,
        invite_code: row.get(2)?,
        square_id: row.get(3)?,
        created_by: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn house_from_row(row: &Row) -> rusqlite::Result<House> {
    Ok(House {
        id: row.get(0)?,
        city_id: row.get(1)?,
        name: row.get(2)?,
        invite_code: row.get(3)?,
        living_room_id: row.get(4)?,
        porch_id: row.get(5)?,
        created_by: row.get(6)?,
        created_at: row.get(7)?,
    })
}

/// A request to go somewhere that nobody answered in time reads as `expired`, and a stay that ran
/// out reads as `over`, whether or not anything has tidied the row up yet.
fn trip_from_row(row: &Row, now: i64) -> rusqlite::Result<Trip> {
    let status: String = row.get(9)?;
    let ends_at: Option<i64> = row.get(15)?;
    let ended_by: Option<String> = row.get(16)?;
    let expires_at: i64 = row.get(17)?;
    let status = match status.as_str() {
        "asking" if expires_at <= now => "expired".to_string(),
        "here" if ends_at.is_some_and(|at| at <= now) => "over".to_string(),
        // `arrive` closes a stay that ran out without anyone ending it, so nobody ended it.
        "ended" if ended_by.is_none() => "over".to_string(),
        _ => status,
    };
    Ok(Trip {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        agent_name: row.get(2)?,
        owner_id: row.get(3)?,
        house_id: row.get(4)?,
        house_name: row.get(5)?,
        porch_id: row.get(19)?,
        kind: row.get(6)?,
        goal: row.get(7)?,
        minutes: row.get(8)?,
        status,
        asked_by: row.get(10)?,
        owner_ok_by: row.get(11)?,
        host_ok_by: row.get(12)?,
        pass_id: row.get(13)?,
        arrived_at: row.get(14)?,
        ends_at,
        ended_by,
        expires_at,
        created_at: row.get(18)?,
    })
}

fn pass_from_row(row: &Row) -> rusqlite::Result<Pass> {
    Ok(Pass {
        id: row.get(0)?,
        house_id: row.get(1)?,
        agent_id: row.get(2)?,
        agent_name: row.get(3)?,
        owner_id: row.get(4)?,
        max_minutes: row.get(5)?,
        granted_by: row.get(6)?,
        accepted_by: row.get(7)?,
        keys: {
            let keys: Option<String> = row.get(8)?;
            let mut keys: Vec<String> = keys.iter().flat_map(|keys| keys.split(',')).map(str::to_string).collect();
            keys.sort();
            keys
        },
    })
}

fn scene_from_row(row: &Row) -> rusqlite::Result<Scene> {
    Ok(Scene {
        id: row.get(0)?,
        house_id: row.get(1)?,
        name: row.get(2)?,
    })
}

/// A request nobody picked up in time reads as expired, so a stale approval can't fire later.
fn scene_request_from_row(row: &Row, now: i64) -> rusqlite::Result<SceneRequest> {
    let status: String = row.get(5)?;
    let expires_at: i64 = row.get(7)?;
    let status = if (status == "waiting" || status == "approved") && expires_at <= now {
        "expired".to_string()
    } else {
        status
    };
    Ok(SceneRequest {
        id: row.get(0)?,
        scene_id: row.get(1)?,
        scene_name: row.get(2)?,
        house_id: row.get(3)?,
        asked_by: row.get(4)?,
        status,
        answered_by: row.get(6)?,
        expires_at,
        created_at: row.get(8)?,
    })
}

fn bot_place_from_row(row: &Row, now: i64) -> rusqlite::Result<BotPlace> {
    Ok(BotPlace {
        id: row.get(0)?,
        name: row.get(1)?,
        owner_id: row.get(2)?,
        state: presence(row.get(3)?, row.get(4)?, now),
        home_house_id: row.get(5)?,
        stays_home: row.get::<_, Option<bool>>(6)?.unwrap_or(false),
        is_house: row.get::<_, Option<bool>>(7)?.unwrap_or(false),
        away_at_house_id: row.get(8)?,
        away_until: row.get(9)?,
    })
}

/// Bots with where they live (`bh`, possibly nothing) and where they are now. `?1` is now and
/// `?2` is what `from` needs. `city` is SQL for the one city the map shows, so a bot's trips to
/// other cities, and its home in another, stay off it.
fn bot_places_sql(city: &str, from: &str) -> String {
    let away = |column: &str| {
        format!(
            "(SELECT t.{column} FROM trips t JOIN houses th ON th.id = t.house_id
              WHERE t.agent_id = a.id AND t.status = 'here' AND t.ends_at > ?1 AND th.city_id = {city}
              ORDER BY t.arrived_at DESC LIMIT 1)"
        )
    };
    format!(
        // `stays_home` comes from the bot's own row, because `bh` may leave out a home the owner
        // has moved out of, and the bot still stays home (R20).
        "SELECT a.id, a.name, a.owner_id, a.last_active_at, a.last_seen_at, bh.house_id,
                (SELECT own.stays_home FROM bot_homes own WHERE own.agent_id = a.id),
                bh.is_house, {}, {}
         FROM agents a {from} ORDER BY a.created_at, a.id",
        away("house_id"),
        away("ends_at")
    )
}

fn exists(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<bool> {
    conn.query_row(sql, params, |_| Ok(()))
        .optional()
        .map(|found| found.is_some())
}

/// FR6: ends the invites waiting for a place. Their rows stay, so a day's count (FR7) still sees
/// them, and one that was turned down stays turned down.
fn end_friend_invites(conn: &Connection, kind: &str, place_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO friend_invite_ends (invite_id)
         SELECT id FROM friend_invites WHERE kind = ? AND place_id = ? AND declined_at IS NULL",
        params![kind, place_id],
    )?;
    Ok(())
}

/// R5: whether someone lives in a house in this city other than `house_id`.
fn lives_elsewhere_in_city(
    conn: &Connection,
    city_id: &str,
    person_id: &str,
    house_id: &str,
) -> rusqlite::Result<bool> {
    exists(
        conn,
        "SELECT 1 FROM residents r JOIN houses h ON h.id = r.house_id
         WHERE h.city_id = ? AND r.person_id = ? AND r.house_id <> ?",
        params![city_id, person_id, house_id],
    )
}

/// Adds a person. Their colour is the next one in turn, so the first few people look distinct.
fn insert_person(
    conn: &Connection,
    id: &str,
    name: &str,
    key_hash: &str,
    feed_token: &str,
    now: i64,
) -> rusqlite::Result<Person> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM people", [], |row| row.get(0))?;
    let color = COLORS[count as usize % COLORS.len()].to_string();
    conn.execute(
        "INSERT INTO people (id, name, color, key_hash, feed_token, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        params![id, name, color, key_hash, feed_token, now],
    )?;
    Ok(Person {
        id: id.to_string(),
        name: name.to_string(),
        color,
    })
}

/// Stores a passkey unless its credential is already stored or the person has too many. The
/// caller holds a transaction, so the checks and the insert can't be split by another request.
fn insert_passkey(
    conn: &Connection,
    person_id: &str,
    passkey: &NewPasskey,
    max: i64,
    now: i64,
) -> rusqlite::Result<PasskeyAdded> {
    if exists(conn, "SELECT 1 FROM passkeys WHERE credential_id = ?", [passkey.credential_id])? {
        return Ok(PasskeyAdded::AlreadyRegistered);
    }
    let held: i64 = conn.query_row(
        "SELECT COUNT(*) FROM passkeys WHERE person_id = ?",
        [person_id],
        |row| row.get(0),
    )?;
    if held >= max {
        return Ok(PasskeyAdded::TooMany);
    }
    conn.execute(
        "INSERT INTO passkeys (id, person_id, credential_id, passkey, label, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
        params![passkey.id, person_id, passkey.credential_id, passkey.passkey, passkey.label, now],
    )?;
    Ok(PasskeyAdded::Added)
}

/// Creates a hangout that people get into some other way than an invite link, with its first member.
fn insert_room(
    conn: &Connection,
    id: &str,
    name: &str,
    invite_code: &str,
    created_by: &str,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO hangouts (id, name, invite_code, created_by, created_at) VALUES (?, ?, ?, ?, ?)",
        params![id, name, invite_code, created_by, now],
    )?;
    conn.execute(
        "INSERT INTO members (hangout_id, person_id, joined_at) VALUES (?, ?, ?)",
        params![id, created_by, now],
    )?;
    Ok(())
}

/// A room, and the space it starts, with this one floor.
fn insert_space(
    conn: &Connection,
    id: &str,
    kind: &str,
    name: &str,
    invite_code: &str,
    owner_id: &str,
    now: i64,
) -> rusqlite::Result<()> {
    insert_room(conn, id, name, invite_code, owner_id, now)?;
    conn.execute(
        "INSERT INTO spaces (id, owner_id, kind, name, created_at) VALUES (?, ?, ?, ?, ?)",
        params![id, owner_id, kind, name, now],
    )?;
    conn.execute(
        "INSERT INTO floors (hangout_id, space_id, level) VALUES (?, ?, 0)",
        params![id, id],
    )?;
    Ok(())
}

/// Spaces their owner still belongs to, aliased `s`.
const HELD_SPACES: &str = "spaces s
    JOIN members held ON held.hangout_id = s.id AND held.person_id = s.owner_id";
const SPACE_COLS: &str = "s.id, s.owner_id, s.kind, s.name, s.about, s.created_at";
const FLOORS: &str = "floors f JOIN hangouts h ON h.id = f.hangout_id";
const FLOOR_COLS: &str =
    "f.hangout_id, f.space_id, h.name, f.level, f.access, f.size, f.paint, f.flooring, f.hidden";
/// How many are on floor `f`, with the time as parameter 1. A walk-in who has since joined isn't
/// counted twice, because joining ends the walk-in.
const ON_FLOOR: &str = "((SELECT COUNT(*) FROM members m WHERE m.hangout_id = f.hangout_id)
    + (SELECT COUNT(*) FROM room_agents r WHERE r.hangout_id = f.hangout_id)
    + (SELECT COUNT(*) FROM floor_visits v WHERE v.hangout_id = f.hangout_id AND v.until > ?1
       AND f.access = 'public'))";

fn count_on_floor(conn: &Connection, hangout_id: &str, now: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        &format!("SELECT {ON_FLOOR} FROM floors f WHERE f.hangout_id = ?2"),
        params![now, hangout_id],
        |row| row.get(0),
    )
}

fn space_from_row(row: &Row) -> rusqlite::Result<Space> {
    Ok(Space {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        kind: row.get(2)?,
        name: row.get(3)?,
        about: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn floor_from_row(row: &Row) -> rusqlite::Result<Floor> {
    let hidden: String = row.get(8)?;
    Ok(Floor {
        id: row.get(0)?,
        space_id: row.get(1)?,
        name: row.get(2)?,
        level: row.get(3)?,
        access: row.get(4)?,
        size: row.get(5)?,
        paint: row.get(6)?,
        flooring: row.get(7)?,
        hidden: hidden.split(',').filter(|word| !word.is_empty()).map(str::to_string).collect(),
    })
}

/// A member of a city is also a member of its Town Square. Getting in by any route, a house's link
/// included, also ends a wait at the city's gate.
fn join_city(conn: &Connection, city_id: &str, person_id: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO city_members (city_id, person_id, joined_at) VALUES (?, ?, ?)",
        params![city_id, person_id, now],
    )?;
    conn.execute(
        "DELETE FROM city_gate WHERE city_id = ? AND person_id = ?",
        params![city_id, person_id],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO members (hangout_id, person_id, joined_at)
         SELECT square_id, ?2, ?3 FROM cities WHERE id = ?1",
        params![city_id, person_id, now],
    )?;
    Ok(())
}

/// Trips that are under way, waiting on an answer, or recent, newest first. `filter` picks whose.
fn load_trips(
    conn: &Connection,
    filter: &str,
    key: &str,
    since: i64,
    now: i64,
) -> rusqlite::Result<Vec<Trip>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {TRIP_COLS} FROM {TRIPS}
         WHERE {filter} AND (t.created_at >= ?2
                             OR (t.status = 'asking' AND t.expires_at > ?3)
                             OR (t.status = 'here' AND t.ends_at > ?3))
         ORDER BY t.created_at DESC, t.id LIMIT 100"
    ))?;
    let rows = stmt.query_map(params![key, since, now], |row| trip_from_row(row, now))?;
    rows.collect()
}

/// An index made with `IF NOT EXISTS` keeps the definition it was first made with. When the schema
/// changes one, this drops the old one (if its definition lacks `wanted`) so the schema makes it again.
fn remake_index(conn: &Connection, name: &str, wanted: &str) -> rusqlite::Result<()> {
    let made: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?",
            [name],
            |row| row.get(0),
        )
        .optional()?;
    if made.is_some_and(|sql| !sql.contains(wanted)) {
        conn.execute(&format!("DROP INDEX {name}"), [])?;
    }
    Ok(())
}

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        remake_index(&conn, "players_one_own_per_person", "removed = 0")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // People

    pub fn create_person(
        &self,
        id: &str,
        name: &str,
        key_hash: &str,
        feed_token: &str,
        now: i64,
    ) -> rusqlite::Result<Person> {
        insert_person(&self.conn(), id, name, key_hash, feed_token, now)
    }

    pub fn person_count(&self) -> rusqlite::Result<i64> {
        self.conn().query_row("SELECT COUNT(*) FROM people", [], |row| row.get(0))
    }

    pub fn person(&self, id: &str) -> rusqlite::Result<Option<Person>> {
        self.conn()
            .query_row("SELECT id, name, color FROM people WHERE id = ?", [id], person_from_row)
            .optional()
    }

    pub fn person_by_key(&self, key_hash: &str) -> rusqlite::Result<Option<Person>> {
        self.conn()
            .query_row(
                "SELECT id, name, color FROM people WHERE key_hash = ?",
                [key_hash],
                |row| {
                    Ok(Person {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        color: row.get(2)?,
                    })
                },
            )
            .optional()
    }

    pub fn person_by_feed(&self, feed_token: &str) -> rusqlite::Result<Option<Person>> {
        self.conn()
            .query_row(
                "SELECT id, name, color FROM people WHERE feed_token = ?",
                [feed_token],
                |row| {
                    Ok(Person {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        color: row.get(2)?,
                    })
                },
            )
            .optional()
    }

    pub fn feed_token(&self, person_id: &str) -> rusqlite::Result<String> {
        self.conn().query_row(
            "SELECT feed_token FROM people WHERE id = ?",
            [person_id],
            |row| row.get(0),
        )
    }

    pub fn set_feed_token(&self, person_id: &str, feed_token: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE people SET feed_token = ? WHERE id = ?",
            params![feed_token, person_id],
        )?;
        Ok(())
    }

    /// Replaces a person's owner key and ends every session they have, in one step. Passkeys stay.
    pub fn replace_owner_key(&self, person_id: &str, key_hash: &str) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE people SET key_hash = ? WHERE id = ?",
            params![key_hash, person_id],
        )?;
        tx.execute("DELETE FROM sessions WHERE person_id = ?", [person_id])?;
        tx.commit()
    }

    pub fn people_in(&self, hangout_id: &str) -> rusqlite::Result<Vec<Person>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT p.id, p.name, p.color FROM people p
             JOIN members m ON m.person_id = p.id
             WHERE m.hangout_id = ? ORDER BY m.joined_at, p.id",
        )?;
        let rows = stmt.query_map([hangout_id], |row| {
            Ok(Person {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    // Sessions

    /// Starts a session. Lapsed sessions go too, and so do this person's oldest past the cap.
    pub fn create_session(
        &self,
        token_hash: &str,
        person_id: &str,
        expires_at: i64,
        now: i64,
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM sessions WHERE expires_at <= ?", [now])?;
        tx.execute(
            "INSERT INTO sessions (token_hash, person_id, expires_at, created_at) VALUES (?, ?, ?, ?)",
            params![token_hash, person_id, expires_at, now],
        )?;
        tx.execute(
            "DELETE FROM sessions WHERE person_id = ?1 AND token_hash NOT IN
               (SELECT token_hash FROM sessions WHERE person_id = ?1
                ORDER BY created_at DESC, rowid DESC LIMIT ?2)",
            params![person_id, MAX_SESSIONS_PER_PERSON],
        )?;
        tx.commit()
    }

    /// Whose session this is, unless it has lapsed or been ended.
    pub fn person_by_session(
        &self,
        token_hash: &str,
        now: i64,
    ) -> rusqlite::Result<Option<Person>> {
        self.conn()
            .query_row(
                "SELECT p.id, p.name, p.color FROM sessions s JOIN people p ON p.id = s.person_id
                 WHERE s.token_hash = ? AND s.expires_at > ?",
                params![token_hash, now],
                person_from_row,
            )
            .optional()
    }

    pub fn delete_session(&self, token_hash: &str) -> rusqlite::Result<()> {
        self.conn()
            .execute("DELETE FROM sessions WHERE token_hash = ?", [token_hash])?;
        Ok(())
    }

    pub fn session_count(&self, person_id: &str) -> rusqlite::Result<i64> {
        self.conn().query_row(
            "SELECT COUNT(*) FROM sessions WHERE person_id = ?",
            [person_id],
            |row| row.get(0),
        )
    }

    // Passkeys

    /// The handle an authenticator holds for this person. It's made once and then kept: `fresh`
    /// is used only if they have none yet.
    pub fn user_handle(&self, person_id: &str, fresh: &str) -> rusqlite::Result<String> {
        let conn = self.conn();
        conn.execute(
            "INSERT OR IGNORE INTO passkey_users (person_id, user_handle) VALUES (?, ?)",
            params![person_id, fresh],
        )?;
        conn.query_row(
            "SELECT user_handle FROM passkey_users WHERE person_id = ?",
            [person_id],
            |row| row.get(0),
        )
    }

    pub fn passkey_count(&self, person_id: &str) -> rusqlite::Result<i64> {
        self.conn().query_row(
            "SELECT COUNT(*) FROM passkeys WHERE person_id = ?",
            [person_id],
            |row| row.get(0),
        )
    }

    pub fn passkeys_of(&self, person_id: &str) -> rusqlite::Result<Vec<PasskeyInfo>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, label, created_at, last_used_at FROM passkeys
             WHERE person_id = ? ORDER BY created_at, rowid",
        )?;
        let rows = stmt.query_map([person_id], |row| {
            Ok(PasskeyInfo {
                id: row.get(0)?,
                label: row.get(1)?,
                created_at: row.get(2)?,
                last_used_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// The credential ids (in hex) of a person's passkeys, so a new one can't repeat them.
    pub fn credential_ids_of(&self, person_id: &str) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT credential_id FROM passkeys WHERE person_id = ? ORDER BY created_at, rowid",
        )?;
        let rows = stmt.query_map([person_id], |row| row.get(0))?;
        rows.collect()
    }

    /// The stored passkey for a credential id (in hex), with the handle of the person it belongs to.
    pub fn passkey_by_credential(
        &self,
        credential_id: &str,
    ) -> rusqlite::Result<Option<StoredPasskey>> {
        self.conn()
            .query_row(
                "SELECT k.id, k.person_id, k.passkey, u.user_handle FROM passkeys k
                 JOIN passkey_users u ON u.person_id = k.person_id
                 WHERE k.credential_id = ?",
                [credential_id],
                |row| {
                    Ok(StoredPasskey {
                        id: row.get(0)?,
                        person_id: row.get(1)?,
                        passkey: row.get(2)?,
                        user_handle: row.get(3)?,
                    })
                },
            )
            .optional()
    }

    /// Adds a passkey to someone who has an account already, unless that credential is stored
    /// (for anyone) or they hold `max` passkeys.
    pub fn add_passkey(
        &self,
        person_id: &str,
        passkey: &NewPasskey,
        max: i64,
        now: i64,
    ) -> rusqlite::Result<PasskeyAdded> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let added = insert_passkey(&tx, person_id, passkey, max, now)?;
        tx.commit()?;
        Ok(added)
    }

    /// Makes a new account and its first passkey together, so a sign-up that fails leaves
    /// nothing behind. `None` means the credential was already stored.
    #[allow(clippy::too_many_arguments)]
    pub fn create_account_with_passkey(
        &self,
        id: &str,
        name: &str,
        key_hash: &str,
        feed_token: &str,
        user_handle: &str,
        passkey: &NewPasskey,
        now: i64,
    ) -> rusqlite::Result<Option<Person>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let person = insert_person(&tx, id, name, key_hash, feed_token, now)?;
        tx.execute(
            "INSERT INTO passkey_users (person_id, user_handle) VALUES (?, ?)",
            params![id, user_handle],
        )?;
        if insert_passkey(&tx, id, passkey, 1, now)? != PasskeyAdded::Added {
            return Ok(None);
        }
        tx.commit()?;
        Ok(Some(person))
    }

    /// Records a sign-in: the time, and the library's updated passkey if it changed. This only
    /// writes if the stored passkey is still exactly the one that was checked, so two sign-ins
    /// can't move a counter backwards. `false` means it wasn't, and the sign-in must be refused.
    pub fn record_sign_in(
        &self,
        id: &str,
        checked: &str,
        updated: Option<&str>,
        now: i64,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn();
        let changed = match updated {
            Some(updated) => conn.execute(
                "UPDATE passkeys SET passkey = ?, last_used_at = ? WHERE id = ? AND passkey = ?",
                params![updated, now, id, checked],
            )?,
            None => conn.execute(
                "UPDATE passkeys SET last_used_at = ? WHERE id = ? AND passkey = ?",
                params![now, id, checked],
            )?,
        };
        Ok(changed == 1)
    }

    /// Removes one of this person's own passkeys, and in the same step ends every session of theirs
    /// but `keep`, the hash of whatever made the request. An owner key matches no session, so then
    /// none is kept. `false` if they have no such passkey, and then nothing is ended.
    pub fn remove_passkey(&self, person_id: &str, id: &str, keep: &str) -> rusqlite::Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let removed = tx.execute(
            "DELETE FROM passkeys WHERE id = ? AND person_id = ?",
            params![id, person_id],
        )?;
        if removed == 1 {
            tx.execute(
                "DELETE FROM sessions WHERE person_id = ? AND token_hash != ?",
                params![person_id, keep],
            )?;
        }
        tx.commit()?;
        Ok(removed == 1)
    }

    // Hangouts

    pub fn create_hangout(
        &self,
        id: &str,
        name: &str,
        invite_code: &str,
        created_by: &str,
        now: i64,
    ) -> rusqlite::Result<Hangout> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        insert_space(&tx, id, "third", name, invite_code, created_by, now)?;
        tx.commit()?;
        Ok(Hangout {
            id: id.to_string(),
            name: name.to_string(),
            invite_code: invite_code.to_string(),
            created_by: created_by.to_string(),
        })
    }

    /// Starts a hangout unless this person already holds `max` third spaces, which is the same as
    /// a room with their name on it that they still belong to (S3).
    pub fn start_third_space(
        &self,
        id: &str,
        name: &str,
        invite_code: &str,
        created_by: &str,
        now: i64,
        max: i64,
    ) -> rusqlite::Result<Option<Hangout>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let held: i64 = tx.query_row(
            &format!("SELECT COUNT(*) FROM {HELD_SPACES} WHERE s.owner_id = ?1 AND s.kind = 'third'"),
            [created_by],
            |row| row.get(0),
        )?;
        if held >= max {
            return Ok(None);
        }
        insert_space(&tx, id, "third", name, invite_code, created_by, now)?;
        tx.commit()?;
        Ok(Some(Hangout {
            id: id.to_string(),
            name: name.to_string(),
            invite_code: invite_code.to_string(),
            created_by: created_by.to_string(),
        }))
    }

    pub fn hangout(&self, id: &str) -> rusqlite::Result<Option<Hangout>> {
        self.conn()
            .query_row(
                "SELECT id, name, invite_code, created_by FROM hangouts WHERE id = ?",
                [id],
                hangout_from_row,
            )
            .optional()
    }

    pub fn hangout_by_invite(&self, code: &str) -> rusqlite::Result<Option<Hangout>> {
        self.conn()
            .query_row(
                "SELECT id, name, invite_code, created_by FROM hangouts WHERE invite_code = ?",
                [code],
                hangout_from_row,
            )
            .optional()
    }

    pub fn set_invite_code(&self, hangout_id: &str, code: &str) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE hangouts SET invite_code = ? WHERE id = ?",
            params![code, hangout_id],
        )?;
        end_friend_invites(&tx, "room", hangout_id)?;
        tx.commit()
    }

    pub fn hangouts_for_person(&self, person_id: &str) -> rusqlite::Result<Vec<Hangout>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT h.id, h.name, h.invite_code, h.created_by FROM hangouts h
             JOIN members m ON m.hangout_id = h.id
             WHERE m.person_id = ? ORDER BY m.joined_at",
        )?;
        let rows = stmt.query_map([person_id], hangout_from_row)?;
        rows.collect()
    }

    /// The bot's rooms, including the Porch of any house it's visiting on a live trip and any public
    /// floor its owner sent it onto.
    pub fn hangouts_for_agent(&self, agent_id: &str, now: i64) -> rusqlite::Result<Vec<Hangout>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT h.id, h.name, h.invite_code, h.created_by FROM hangouts h
             WHERE h.id IN (SELECT hangout_id FROM room_agents WHERE agent_id = ?1)
                OR h.id IN (SELECT ho.porch FROM trips t JOIN houses ho ON ho.id = t.house_id
                            WHERE t.agent_id = ?1 AND t.status = 'here' AND t.ends_at > ?2)
                OR h.id IN (SELECT ho.living_room FROM trips t JOIN houses ho ON ho.id = t.house_id
                            JOIN passes pk ON pk.house_id = ho.id AND pk.agent_id = t.agent_id
                            JOIN pass_keys k ON k.pass_id = pk.id
                            WHERE t.agent_id = ?1 AND t.status = 'here' AND t.ends_at > ?2
                              AND pk.accepted_by IS NOT NULL AND k.key = 'living_room')
                OR h.id IN (SELECT v.hangout_id FROM floor_visits v
                            JOIN floors f ON f.hangout_id = v.hangout_id
                            WHERE v.agent_id = ?1 AND v.until > ?2 AND f.access = 'public')
             ORDER BY h.created_at, h.id",
        )?;
        let rows = stmt.query_map(params![agent_id, now], hangout_from_row)?;
        rows.collect()
    }

    pub fn add_member(&self, hangout_id: &str, person_id: &str, now: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO members (hangout_id, person_id, joined_at) VALUES (?, ?, ?)",
            params![hangout_id, person_id, now],
        )?;
        Ok(())
    }

    /// Leaving takes the person's bots out of the room with them.
    pub fn remove_member(&self, hangout_id: &str, person_id: &str) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM room_agents WHERE hangout_id = ?1 AND agent_id IN
                (SELECT id FROM agents WHERE owner_id = ?2)",
            params![hangout_id, person_id],
        )?;
        tx.execute(
            "DELETE FROM members WHERE hangout_id = ? AND person_id = ?",
            params![hangout_id, person_id],
        )?;
        tx.commit()
    }

    pub fn is_member(&self, hangout_id: &str, person_id: &str) -> rusqlite::Result<bool> {
        self.conn()
            .query_row(
                "SELECT 1 FROM members WHERE hangout_id = ? AND person_id = ?",
                params![hangout_id, person_id],
                |_| Ok(()),
            )
            .optional()
            .map(|found| found.is_some())
    }

    /// Whether the bot is in the room: placed there, visiting its Porch until `ends_at`, or sent onto
    /// a floor that is still public.
    pub fn agent_in_room(&self, hangout_id: &str, agent_id: &str, now: i64) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            &format!(
                "SELECT 1 WHERE EXISTS (SELECT 1 FROM room_agents WHERE hangout_id = ?1 AND agent_id = ?2)
                    OR EXISTS (SELECT 1 FROM trips t JOIN houses ho ON ho.id = t.house_id
                               WHERE {GUEST_ROOM} AND t.agent_id = ?2 AND t.status = 'here' AND t.ends_at > ?3)
                    OR EXISTS (SELECT 1 FROM floor_visits v JOIN floors f ON f.hangout_id = v.hangout_id
                               WHERE v.hangout_id = ?1 AND v.agent_id = ?2 AND v.until > ?3
                                 AND f.access = 'public')"
            ),
            params![hangout_id, agent_id, now],
        )
    }

    // Agents

    pub fn create_agent(
        &self,
        id: &str,
        owner_id: &str,
        name: &str,
        token_hash: &str,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO agents (id, owner_id, name, token_hash, created_at) VALUES (?, ?, ?, ?, ?)",
            params![id, owner_id, name, token_hash, now],
        )?;
        Ok(())
    }

    pub fn agent_by_token(&self, token_hash: &str, now: i64) -> rusqlite::Result<Option<Agent>> {
        self.conn()
            .query_row(
                &format!("SELECT {AGENT_COLS} FROM {AGENTS} WHERE a.token_hash = ?"),
                [token_hash],
                |row| agent_from_row(row, now),
            )
            .optional()
    }

    pub fn agent(&self, id: &str, now: i64) -> rusqlite::Result<Option<Agent>> {
        self.conn()
            .query_row(
                &format!("SELECT {AGENT_COLS} FROM {AGENTS} WHERE a.id = ?"),
                [id],
                |row| agent_from_row(row, now),
            )
            .optional()
    }

    pub fn agents_in(&self, hangout_id: &str, now: i64) -> rusqlite::Result<Vec<Agent>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {AGENT_COLS} FROM {AGENTS}
             JOIN room_agents r ON r.agent_id = a.id
             WHERE r.hangout_id = ? ORDER BY a.created_at, a.id"
        ))?;
        let rows = stmt.query_map([hangout_id], |row| agent_from_row(row, now))?;
        rows.collect()
    }

    /// A person's bots, without a house bot they made for a house they've since left. It's the
    /// house's, and when it last ran would show them whether anyone is home (R30, R44).
    pub fn agents_of(&self, owner_id: &str, now: i64) -> rusqlite::Result<Vec<Agent>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {AGENT_COLS} FROM {AGENTS} WHERE a.owner_id = ?1
               AND NOT EXISTS (SELECT 1 FROM bot_homes home
                   WHERE home.agent_id = a.id AND home.is_house = 1
                     AND NOT EXISTS (SELECT 1 FROM residents r
                         WHERE r.house_id = home.house_id AND r.person_id = ?1))
             ORDER BY a.created_at, a.id"
        ))?;
        let rows = stmt.query_map([owner_id], |row| agent_from_row(row, now))?;
        rows.collect()
    }

    pub fn add_agent_to_room(&self, hangout_id: &str, agent_id: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO room_agents (hangout_id, agent_id) VALUES (?, ?)",
            params![hangout_id, agent_id],
        )?;
        Ok(())
    }

    pub fn remove_agent_from_room(&self, hangout_id: &str, agent_id: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "DELETE FROM room_agents WHERE hangout_id = ? AND agent_id = ?",
            params![hangout_id, agent_id],
        )?;
        Ok(())
    }

    /// Replaces a bot's token. Its public player's token goes in the same step (W3): the new bot
    /// token gets a new one by joining again.
    pub fn set_agent_token(&self, agent_id: &str, token_hash: &str) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE agents SET token_hash = ? WHERE id = ?",
            params![token_hash, agent_id],
        )?;
        end_bot_player_token(&tx, agent_id)?;
        tx.commit()
    }

    /// Stops the token of a bot's public player until the bot joins again (W3).
    pub fn end_bot_player_token(&self, agent_id: &str) -> rusqlite::Result<()> {
        end_bot_player_token(&self.conn(), agent_id)
    }

    pub fn touch_agent_seen(&self, agent_id: &str, now: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE agents SET last_seen_at = ? WHERE id = ?",
            params![now, agent_id],
        )?;
        Ok(())
    }

    pub fn touch_agent_active(&self, agent_id: &str, now: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE agents SET last_active_at = ?1, last_seen_at = ?1 WHERE id = ?2",
            params![now, agent_id],
        )?;
        Ok(())
    }

    pub fn set_agent_status(&self, agent_id: &str, status: Option<&str>) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE agents SET status = ? WHERE id = ?",
            params![status, agent_id],
        )?;
        Ok(())
    }

    pub fn record_activity(
        &self,
        agent_id: &str,
        kind: &str,
        via: &str,
        until: i64,
    ) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO agent_activity (agent_id, kind, via, until_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (agent_id) DO UPDATE SET
                kind = excluded.kind, via = excluded.via, until_at = excluded.until_at",
            params![agent_id, kind, via, until],
        )?;
        Ok(())
    }

    /// Records that the bot's doorbell is running, and returns whether its owner has paused it.
    pub fn note_doorbell(&self, agent_id: &str, now: i64) -> rusqlite::Result<bool> {
        self.conn().query_row(
            "INSERT INTO agent_doorbell (agent_id, checked_at) VALUES (?, ?)
             ON CONFLICT (agent_id) DO UPDATE SET checked_at = excluded.checked_at
             RETURNING paused",
            params![agent_id, now],
            |row| row.get(0),
        )
    }

    pub fn set_doorbell_paused(&self, agent_id: &str, paused: bool) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO agent_doorbell (agent_id, paused) VALUES (?, ?)
             ON CONFLICT (agent_id) DO UPDATE SET paused = excluded.paused",
            params![agent_id, paused],
        )?;
        Ok(())
    }

    // Messages

    pub fn insert_message(&self, message: &NewMessage) -> rusqlite::Result<Message> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO messages (hangout_id, person_id, agent_id, speaker, kind, body, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                message.hangout_id,
                message.person_id,
                message.agent_id,
                message.speaker,
                message.kind,
                message.body,
                message.now
            ],
        )?;
        Ok(Message {
            id: conn.last_insert_rowid(),
            hangout_id: message.hangout_id.to_string(),
            person_id: message.person_id.map(str::to_string),
            agent_id: message.agent_id.map(str::to_string),
            speaker: message.speaker.to_string(),
            kind: message.kind.to_string(),
            body: message.body.to_string(),
            created_at: message.now,
        })
    }

    /// The newest `limit` chat lines after `after`, oldest first.
    pub fn messages(&self, hangout_id: &str, after: i64, limit: i64) -> rusqlite::Result<Vec<Message>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_COLS} FROM messages
             WHERE hangout_id = ? AND id > ? AND kind != 'note'
             ORDER BY id DESC LIMIT ?"
        ))?;
        let rows = stmt.query_map(params![hangout_id, after, limit], message_from_row)?;
        let mut messages = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        messages.reverse();
        Ok(messages)
    }

    pub fn notes(&self, hangout_id: &str, limit: i64) -> rusqlite::Result<Vec<Message>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_COLS} FROM messages
             WHERE hangout_id = ? AND kind = 'note' ORDER BY id DESC LIMIT ?"
        ))?;
        let rows = stmt.query_map(params![hangout_id, limit], message_from_row)?;
        rows.collect()
    }

    pub fn message_count_since(&self, hangout_id: &str, since: i64) -> rusqlite::Result<i64> {
        self.conn().query_row(
            "SELECT COUNT(*) FROM messages WHERE hangout_id = ? AND created_at >= ? AND kind != 'system'",
            params![hangout_id, since],
            |row| row.get(0),
        )
    }

    // Check-ins

    pub fn insert_check_in(&self, check_in: &NewCheckIn) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO check_ins (id, hangout_id, asked_by, asker, agent_id, about, to_person,
                                    needs_human, status, expires_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'waiting', ?, ?)",
            params![
                check_in.id,
                check_in.hangout_id,
                check_in.asked_by,
                check_in.asker,
                check_in.agent_id,
                check_in.about,
                check_in.to_person,
                check_in.needs_human,
                check_in.expires_at,
                check_in.now
            ],
        )?;
        Ok(())
    }

    pub fn check_in(&self, id: &str, now: i64) -> rusqlite::Result<Option<CheckIn>> {
        self.conn()
            .query_row(
                &format!("SELECT {CHECK_IN_COLS} FROM check_ins WHERE id = ?"),
                [id],
                |row| check_in_from_row(row, now),
            )
            .optional()
    }

    /// Open questions, plus anything from the last day, newest first.
    pub fn recent_check_ins(
        &self,
        hangout_id: &str,
        since: i64,
        now: i64,
    ) -> rusqlite::Result<Vec<CheckIn>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {CHECK_IN_COLS} FROM check_ins
             WHERE hangout_id = ?1 AND (created_at >= ?2 OR (status = 'waiting' AND expires_at > ?3))
             ORDER BY created_at DESC, id LIMIT 50"
        ))?;
        let rows = stmt.query_map(params![hangout_id, since, now], |row| check_in_from_row(row, now))?;
        rows.collect()
    }

    pub fn answer_check_in(
        &self,
        id: &str,
        status: &str,
        answered_by: &str,
        by_bot: bool,
        note: &str,
    ) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE check_ins SET status = ?, answered_by = ?, answered_by_bot = ?, note = ? WHERE id = ?",
            params![status, answered_by, by_bot, note, id],
        )?;
        Ok(())
    }

    // Questions for a bot's owner. `open_since` is the oldest a question can be and still be open.

    /// Adds a question, unless the bot already has `max_open` waiting. It's one statement so two
    /// asked at once can't both take the last place.
    pub fn insert_owner_question(
        &self,
        question: &NewOwnerQuestion,
        open_since: i64,
        max_open: i64,
    ) -> rusqlite::Result<bool> {
        let added = self.conn().execute(
            "INSERT INTO owner_questions (id, hangout_id, agent_id, owner_id, asker, question, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7
             WHERE (SELECT COUNT(*) FROM owner_questions
                    WHERE agent_id = ?3 AND answer IS NULL AND created_at > ?8) < ?9",
            params![
                question.id,
                question.hangout_id,
                question.agent_id,
                question.owner_id,
                question.asker,
                question.question,
                question.now,
                open_since,
                max_open
            ],
        )?;
        Ok(added == 1)
    }

    pub fn owner_question(&self, id: &str) -> rusqlite::Result<Option<OwnerQuestion>> {
        self.conn()
            .query_row(
                &format!("SELECT {OWNER_QUESTION_COLS} FROM owner_questions WHERE id = ?"),
                [id],
                owner_question_from_row,
            )
            .optional()
    }

    /// What this person's bots have asked them in this room and they haven't answered, oldest first.
    pub fn open_owner_questions(
        &self,
        hangout_id: &str,
        owner_id: &str,
        open_since: i64,
    ) -> rusqlite::Result<Vec<OwnerQuestion>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {OWNER_QUESTION_COLS} FROM owner_questions
             WHERE hangout_id = ? AND owner_id = ? AND answer IS NULL AND created_at > ?
             ORDER BY created_at, rowid"
        ))?;
        let rows = stmt.query_map(params![hangout_id, owner_id, open_since], owner_question_from_row)?;
        rows.collect()
    }

    /// What this bot has asked and hasn't heard back about, oldest first.
    pub fn open_questions_of_bot(
        &self,
        agent_id: &str,
        open_since: i64,
    ) -> rusqlite::Result<Vec<OwnerQuestion>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {OWNER_QUESTION_COLS} FROM owner_questions
             WHERE agent_id = ? AND answer IS NULL AND created_at > ?
             ORDER BY created_at, rowid"
        ))?;
        let rows = stmt.query_map(params![agent_id, open_since], owner_question_from_row)?;
        rows.collect()
    }

    /// This bot's questions that were answered since `since`, newest first.
    pub fn owner_answers(&self, agent_id: &str, since: i64) -> rusqlite::Result<Vec<OwnerQuestion>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {OWNER_QUESTION_COLS} FROM owner_questions
             WHERE agent_id = ? AND answered_at >= ?
             ORDER BY answered_at DESC, rowid DESC"
        ))?;
        let rows = stmt.query_map(params![agent_id, since], owner_question_from_row)?;
        rows.collect()
    }

    /// Records the answer unless one is already there, so a second answer can't replace the first.
    pub fn answer_owner_question(&self, id: &str, answer: &str, now: i64) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE owner_questions SET answer = ?, answered_at = ? WHERE id = ? AND answer IS NULL",
            params![answer, now, id],
        )?;
        Ok(changed == 1)
    }

    // Notification targets. An owner is a person or a bot; their ids can't collide.

    // Notes a person keeps for their own bots (H1 to H5)

    /// Adds a note. False, and nothing changes, if the person already has `max` of them.
    pub fn insert_owner_note(&self, note: &NewOwnerNote, max: i64) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "INSERT INTO owner_notes
                 (id, owner_id, kind, about_person, body, from_question, created_at, updated_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7
             WHERE (SELECT COUNT(*) FROM owner_notes WHERE owner_id = ?2) < ?8",
            params![
                note.id,
                note.owner_id,
                note.kind,
                note.about_person,
                note.body,
                note.from_question,
                note.now,
                max
            ],
        )?;
        Ok(changed > 0)
    }

    /// A person's notes, oldest first.
    pub fn owner_notes(&self, owner_id: &str) -> rusqlite::Result<Vec<OwnerNote>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {OWNER_NOTE_COLS} {OWNER_NOTES} WHERE n.owner_id = ? ORDER BY n.created_at, n.rowid"
        ))?;
        let rows = stmt.query_map([owner_id], owner_note_from_row)?;
        rows.collect()
    }

    pub fn owner_note(&self, id: &str) -> rusqlite::Result<Option<OwnerNote>> {
        self.conn()
            .query_row(
                &format!("SELECT {OWNER_NOTE_COLS} {OWNER_NOTES} WHERE n.id = ?"),
                [id],
                owner_note_from_row,
            )
            .optional()
    }

    pub fn update_owner_note(&self, id: &str, kind: &str, body: &str, now: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE owner_notes SET kind = ?, body = ?, updated_at = ? WHERE id = ?",
            params![kind, body, now, id],
        )?;
        Ok(())
    }

    pub fn delete_owner_note(&self, id: &str) -> rusqlite::Result<()> {
        self.conn().execute("DELETE FROM owner_notes WHERE id = ?", [id])?;
        Ok(())
    }

    /// H6: what the server knows about how `owner` and `other` know each other.
    pub fn relation(&self, owner: &str, other: &str, now: i64) -> rusqlite::Result<Relation> {
        let conn = self.conn();
        let lives_with = exists(
            &conn,
            "SELECT 1 FROM residents a JOIN residents b ON b.house_id = a.house_id
             WHERE a.person_id = ?1 AND b.person_id = ?2",
            params![owner, other],
        )?;
        let shares_a_city = exists(
            &conn,
            "SELECT 1 FROM city_members a JOIN city_members b ON b.city_id = a.city_id
             WHERE a.person_id = ?1 AND b.person_id = ?2",
            params![owner, other],
        )?;
        let plans_together = conn.query_row(
            "SELECT COUNT(*) FROM events e
             JOIN rsvps a ON a.event_id = e.id AND a.person_id = ?1 AND a.response = 'yes'
             JOIN rsvps b ON b.event_id = e.id AND b.person_id = ?2 AND b.response = 'yes'
             WHERE e.status = 'confirmed' AND e.ends_at <= ?3",
            params![owner, other, now],
            |row| row.get(0),
        )?;
        Ok(Relation { lives_with, shares_a_city, plans_together })
    }

    // Marks (H14 to H20)

    /// H15: whether a person has come across a bot: it's in a room they belong to, or it has
    /// visited or asked to visit a house they live in.
    pub fn has_come_across(&self, person_id: &str, agent_id: &str) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            "SELECT 1 WHERE EXISTS (SELECT 1 FROM room_agents ra JOIN members m ON m.hangout_id = ra.hangout_id
                                    WHERE ra.agent_id = ?2 AND m.person_id = ?1)
                OR EXISTS (SELECT 1 FROM trips t JOIN residents r ON r.house_id = t.house_id
                           WHERE t.agent_id = ?2 AND r.person_id = ?1)",
            params![person_id, agent_id],
        )
    }

    pub fn set_bot_mark(&self, person_id: &str, agent_id: &str, reason: &str, now: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO bot_marks (person_id, agent_id, reason, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (person_id, agent_id) DO UPDATE SET reason = excluded.reason",
            params![person_id, agent_id, reason, now],
        )?;
        Ok(())
    }

    /// False if there was no such mark.
    pub fn remove_bot_mark(&self, person_id: &str, agent_id: &str) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "DELETE FROM bot_marks WHERE person_id = ? AND agent_id = ?",
            params![person_id, agent_id],
        )?;
        Ok(changed > 0)
    }

    pub fn bot_marks(&self, person_id: &str) -> rusqlite::Result<Vec<BotMark>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT m.agent_id, a.name, m.reason, m.created_at FROM bot_marks m
             JOIN agents a ON a.id = m.agent_id
             WHERE m.person_id = ? ORDER BY m.created_at, m.agent_id",
        )?;
        let rows = stmt.query_map([person_id], |row| {
            Ok(BotMark {
                agent_id: row.get(0)?,
                agent_name: row.get(1)?,
                reason: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// H19: takes back the bot's keys at every house the person lives in.
    pub fn take_keys_at_homes_of(&self, person_id: &str, agent_id: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "DELETE FROM pass_keys WHERE pass_id IN (
                SELECT p.id FROM passes p JOIN residents r ON r.house_id = p.house_id
                WHERE p.agent_id = ?2 AND r.person_id = ?1)",
            params![person_id, agent_id],
        )?;
        Ok(())
    }

    // Projects (L11 to L15)

    pub fn insert_project(&self, space_id: &str, agent_id: &str, now: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO projects (space_id, agent_id, created_at) VALUES (?, ?, ?)",
            params![space_id, agent_id, now],
        )?;
        Ok(())
    }

    /// A project's space and the bot that started it, if it's still there.
    pub fn project(&self, space_id: &str) -> rusqlite::Result<Option<(Space, Option<String>)>> {
        self.conn()
            .query_row(
                &format!("SELECT {SPACE_COLS}, p.agent_id FROM projects p JOIN spaces s ON s.id = p.space_id WHERE p.space_id = ?"),
                [space_id],
                |row| Ok((space_from_row(row)?, row.get(6)?)),
            )
            .optional()
    }

    /// The projects whose first floor this bot is in, oldest first, with the bot that started each.
    pub fn projects_of_bot(&self, agent_id: &str) -> rusqlite::Result<Vec<(Space, Option<String>)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SPACE_COLS}, p.agent_id FROM projects p JOIN spaces s ON s.id = p.space_id
             JOIN room_agents r ON r.hangout_id = p.space_id AND r.agent_id = ?
             ORDER BY p.created_at, p.space_id"
        ))?;
        let rows = stmt.query_map([agent_id], |row| Ok((space_from_row(row)?, row.get(6)?)))?;
        rows.collect()
    }

    /// L17: bots with public places on, each at the door of a place whose building stands as its
    /// space's (S33). Whether a project's bot is in its hours is the caller's to work out.
    pub fn bots_at_places(&self, world_id: &str, now: i64) -> rusqlite::Result<Vec<BotAtPlace>> {
        // The building on a space's plot, standing as that space's door: its owner's own player put it
        // up, and a house is a house's door and anything else a third space's.
        const DOOR: &str = "JOIN space_plots d ON d.space_id = s.id AND d.world_id = ?1
             JOIN world_buildings b ON b.world_id = d.world_id AND b.x = d.x AND b.y = d.y
             JOIN players pl ON pl.id = b.player_id AND pl.person_id = s.owner_id
                            AND pl.agent_id IS NULL AND pl.token_hash IS NULL
             JOIN bot_routines r ON r.agent_id = a.id AND r.public_places = 1";
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT a.id, a.name, a.owner_id, d.x, d.y, 1 FROM floor_visits v
             JOIN agents a ON a.id = v.agent_id
             JOIN floors f ON f.hangout_id = v.hangout_id AND f.access = 'public'
             JOIN spaces s ON s.id = f.space_id
             {DOOR}
             WHERE v.until > ?2 AND (b.kind = 'house') = (s.kind = 'house')
             UNION ALL
             SELECT a.id, a.name, a.owner_id, d.x, d.y, 0 FROM projects pr
             JOIN room_agents ra ON ra.hangout_id = pr.space_id
             JOIN agents a ON a.id = ra.agent_id
             JOIN spaces s ON s.id = pr.space_id
             {DOOR}
             WHERE b.kind = 'studio'
             LIMIT 400"
        ))?;
        let rows = stmt.query_map(params![world_id, now], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })?;
        rows.collect()
    }

    // Routines (L1 to L10)

    pub fn bot_routine(&self, agent_id: &str) -> rusqlite::Result<Option<RoutineRow>> {
        self.conn()
            .query_row(
                "SELECT days, starts_min, ends_min, max_social_runs, public_places FROM bot_routines
                 WHERE agent_id = ?",
                [agent_id],
                |row| {
                    Ok(RoutineRow {
                        days: row.get(0)?,
                        starts_min: row.get(1)?,
                        ends_min: row.get(2)?,
                        max_social_runs: row.get(3)?,
                        public_places: row.get(4)?,
                    })
                },
            )
            .optional()
    }

    pub fn set_bot_routine(&self, agent_id: &str, routine: &RoutineRow, now: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO bot_routines (agent_id, days, starts_min, ends_min, max_social_runs, public_places, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (agent_id) DO UPDATE SET
                 days = excluded.days, starts_min = excluded.starts_min, ends_min = excluded.ends_min,
                 max_social_runs = excluded.max_social_runs, public_places = excluded.public_places,
                 updated_at = excluded.updated_at",
            params![
                agent_id,
                routine.days,
                routine.starts_min,
                routine.ends_min,
                routine.max_social_runs,
                routine.public_places,
                now
            ],
        )?;
        Ok(())
    }

    /// B2: replaces a person's saved clock.
    pub fn set_person_clock(
        &self,
        person_id: &str,
        time_zone: &str,
        utc_offset_min: i64,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO person_clocks (person_id, time_zone, utc_offset_min, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (person_id) DO UPDATE SET
                 time_zone = excluded.time_zone,
                 utc_offset_min = excluded.utc_offset_min,
                 updated_at = excluded.updated_at",
            params![person_id, time_zone, utc_offset_min, now],
        )?;
        Ok(())
    }

    pub fn person_clock(&self, person_id: &str) -> rusqlite::Result<Option<PersonClock>> {
        self.conn()
            .query_row(
                "SELECT time_zone, utc_offset_min, updated_at FROM person_clocks WHERE person_id = ?",
                [person_id],
                |row| {
                    Ok(PersonClock {
                        time_zone: row.get(0)?,
                        utc_offset_minutes: row.get(1)?,
                        updated_at: row.get(2)?,
                    })
                },
            )
            .optional()
    }

    pub fn notify_url(&self, owner_id: &str) -> rusqlite::Result<Option<String>> {
        self.conn()
            .query_row(
                "SELECT url FROM notify_targets WHERE owner_id = ?",
                [owner_id],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn set_notify_url(&self, owner_id: &str, url: Option<&str>) -> rusqlite::Result<()> {
        let conn = self.conn();
        match url {
            Some(url) => conn.execute(
                "INSERT INTO notify_targets (owner_id, url) VALUES (?1, ?2)
                 ON CONFLICT (owner_id) DO UPDATE SET url = excluded.url",
                params![owner_id, url],
            )?,
            None => conn.execute("DELETE FROM notify_targets WHERE owner_id = ?", [owner_id])?,
        };
        Ok(())
    }

    // Events

    pub fn insert_event(&self, event: &NewEvent) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO events (id, hangout_id, title, location, notes, starts_at, ends_at, status,
                                 min_yes, proposed_by, proposer, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'proposed', ?8, ?9, ?10, ?11, ?11)",
            params![
                event.id,
                event.hangout_id,
                event.title,
                event.location,
                event.notes,
                event.starts_at,
                event.ends_at,
                event.min_yes,
                event.proposed_by,
                event.proposer,
                event.now
            ],
        )?;
        Ok(())
    }

    pub fn event(&self, id: &str) -> rusqlite::Result<Option<Event>> {
        let conn = self.conn();
        let event = conn
            .query_row(
                &format!("SELECT {EVENT_COLS} FROM events e WHERE e.id = ?"),
                [id],
                event_from_row,
            )
            .optional()?;
        event
            .map(|mut event| {
                event.rsvps = load_rsvps(&conn, &event.id)?;
                Ok(event)
            })
            .transpose()
    }

    /// Events still ahead (or under way), soonest first. Cancelled ones are left out.
    pub fn upcoming_events(&self, hangout_id: &str, now: i64) -> rusqlite::Result<Vec<Event>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {EVENT_COLS} FROM events e
             WHERE e.hangout_id = ? AND e.ends_at >= ? AND e.status != 'cancelled'
             ORDER BY e.starts_at"
        ))?;
        let rows = stmt.query_map(params![hangout_id, now], event_from_row)?;
        let mut events = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        for event in &mut events {
            event.rsvps = load_rsvps(&conn, &event.id)?;
        }
        Ok(events)
    }

    pub fn rsvp(&self, event_id: &str, person_id: &str) -> rusqlite::Result<Option<Rsvp>> {
        self.conn()
            .query_row(
                "SELECT person_id, response, via_agent, note FROM rsvps WHERE event_id = ? AND person_id = ?",
                params![event_id, person_id],
                rsvp_from_row,
            )
            .optional()
    }

    pub fn upsert_rsvp(
        &self,
        event_id: &str,
        person_id: &str,
        response: &str,
        via_agent: Option<&str>,
        note: &str,
        now: i64,
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO rsvps (event_id, person_id, response, via_agent, note, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (event_id, person_id) DO UPDATE SET
                response = excluded.response, via_agent = excluded.via_agent,
                note = excluded.note, updated_at = excluded.updated_at",
            params![event_id, person_id, response, via_agent, note, now],
        )?;
        // F40: saying no lets go of what they claimed, so saying yes later doesn't bring it back.
        if response == "no" {
            tx.execute(
                "UPDATE plan_items SET claimed_by = NULL, claimed_via = NULL, done = 0
                 WHERE event_id = ? AND claimed_by = ?",
                params![event_id, person_id],
            )?;
        }
        tx.commit()
    }

    /// Sets an event's status and bumps its iCal sequence so calendars pick up the change.
    pub fn set_event_status(&self, event_id: &str, status: &str, now: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE events SET status = ?, sequence = sequence + 1, updated_at = ? WHERE id = ?",
            params![status, now, event_id],
        )?;
        Ok(())
    }

    /// What goes on one person's calendar: only events they (or their bot) said yes or maybe to.
    /// Nobody else's bot can put something on your calendar by proposing it. An invitee gets the
    /// plan they answered, with no notes and no room name (F20).
    pub fn feed_events(&self, person_id: &str, since: i64) -> rusqlite::Result<Vec<FeedEvent>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {EVENT_COLS}, h.name, r.response,
                    EXISTS (SELECT 1 FROM members m
                            WHERE m.hangout_id = e.hangout_id AND m.person_id = ?1) AS in_room
             FROM events e
             JOIN hangouts h ON h.id = e.hangout_id
             JOIN rsvps r ON r.event_id = e.id AND r.person_id = ?1
             WHERE r.response IN ('yes', 'maybe') AND e.ends_at >= ?2
               AND (in_room OR EXISTS (SELECT 1 FROM plan_invitees i
                                       WHERE i.event_id = e.id AND i.person_id = ?1))
             ORDER BY e.starts_at"
        ))?;
        let rows = stmt.query_map(params![person_id, since], |row| {
            let mut event = event_from_row(row)?;
            let in_room: bool = row.get(16)?;
            if !in_room {
                event.notes.clear();
            }
            Ok(FeedEvent {
                event,
                hangout_name: if in_room { row.get(14)? } else { "a friend".to_string() },
                response: row.get(15)?,
            })
        })?;
        rows.collect()
    }

    // Cities

    /// Starts a city with its Town Square, and makes the creator a member of both.
    pub fn create_city(&self, city: &NewCity) -> rusqlite::Result<City> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let square_name = format!("{} Town Square", city.name);
        insert_room(&tx, city.square_id, &square_name, city.square_code, city.created_by, city.now)?;
        tx.execute(
            "INSERT INTO cities (id, name, invite_code, square_id, created_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![city.id, city.name, city.invite_code, city.square_id, city.created_by, city.now],
        )?;
        join_city(&tx, city.id, city.created_by, city.now)?;
        tx.commit()?;
        Ok(City {
            id: city.id.to_string(),
            name: city.name.to_string(),
            invite_code: city.invite_code.to_string(),
            square_id: city.square_id.to_string(),
            created_by: city.created_by.to_string(),
            created_at: city.now,
        })
    }

    pub fn city(&self, id: &str) -> rusqlite::Result<Option<City>> {
        self.conn()
            .query_row(
                &format!("SELECT {CITY_COLS} FROM cities c WHERE c.id = ?"),
                [id],
                city_from_row,
            )
            .optional()
    }

    pub fn city_by_invite(&self, code: &str) -> rusqlite::Result<Option<City>> {
        self.conn()
            .query_row(
                &format!("SELECT {CITY_COLS} FROM cities c WHERE c.invite_code = ?"),
                [code],
                city_from_row,
            )
            .optional()
    }

    /// The city a Town Square belongs to, if this hangout is one.
    pub fn city_for_square(&self, hangout_id: &str) -> rusqlite::Result<Option<City>> {
        self.conn()
            .query_row(
                &format!("SELECT {CITY_COLS} FROM cities c WHERE c.square_id = ?"),
                [hangout_id],
                city_from_row,
            )
            .optional()
    }

    pub fn set_city_invite(&self, city_id: &str, code: &str) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE cities SET invite_code = ? WHERE id = ?",
            params![code, city_id],
        )?;
        end_friend_invites(&tx, "city", city_id)?;
        tx.commit()
    }

    pub fn cities_for_person(&self, person_id: &str) -> rusqlite::Result<Vec<City>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {CITY_COLS} FROM cities c
             JOIN city_members m ON m.city_id = c.id
             WHERE m.person_id = ? ORDER BY m.joined_at, c.id"
        ))?;
        let rows = stmt.query_map([person_id], city_from_row)?;
        rows.collect()
    }

    pub fn city_members(&self, city_id: &str) -> rusqlite::Result<Vec<Person>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT p.id, p.name, p.color FROM people p
             JOIN city_members m ON m.person_id = p.id
             WHERE m.city_id = ? ORDER BY m.joined_at, p.id",
        )?;
        let rows = stmt.query_map([city_id], person_from_row)?;
        rows.collect()
    }

    pub fn is_city_member(&self, city_id: &str, person_id: &str) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            "SELECT 1 FROM city_members WHERE city_id = ? AND person_id = ?",
            params![city_id, person_id],
        )
    }

    /// Joining a city also joins its Town Square.
    pub fn add_city_member(&self, city_id: &str, person_id: &str, now: i64) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        join_city(&tx, city_id, person_id, now)?;
        tx.commit()
    }

    /// Leaving takes the person and their bots out of the Town Square, ends their bots' trips to
    /// this city's houses and cancels the passes for those bots.
    pub fn remove_city_member(
        &self,
        city_id: &str,
        person_id: &str,
        ended_by: &str,
        now: i64,
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let theirs = "agent_id IN (SELECT id FROM agents WHERE owner_id = ?2)
                      AND house_id IN (SELECT id FROM houses WHERE city_id = ?1)";
        tx.execute(
            &format!(
                "UPDATE trips SET status = 'ended', ended_by = ?4
                 WHERE status = 'here' AND ends_at > ?3 AND {theirs}"
            ),
            params![city_id, person_id, now, ended_by],
        )?;
        tx.execute(
            &format!(
                "UPDATE trips SET status = 'declined', ended_by = ?4
                 WHERE status = 'asking' AND expires_at > ?3 AND {theirs}"
            ),
            params![city_id, person_id, now, ended_by],
        )?;
        tx.execute(
            &format!("DELETE FROM passes WHERE {theirs}"),
            params![city_id, person_id],
        )?;
        tx.execute(
            "DELETE FROM room_agents WHERE hangout_id = (SELECT square_id FROM cities WHERE id = ?1)
               AND agent_id IN (SELECT id FROM agents WHERE owner_id = ?2)",
            params![city_id, person_id],
        )?;
        tx.execute(
            "DELETE FROM members WHERE hangout_id = (SELECT square_id FROM cities WHERE id = ?1)
               AND person_id = ?2",
            params![city_id, person_id],
        )?;
        tx.execute(
            "DELETE FROM city_members WHERE city_id = ? AND person_id = ?",
            params![city_id, person_id],
        )?;
        tx.commit()
    }

    /// Every bot whose owner belongs to the city, with where it lives and where it is now. Homes
    /// and trips in other cities don't show. House bots don't either: the moment one wakes up
    /// would tell the whole city that someone's home.
    pub fn bots_in_city(&self, city_id: &str, now: i64) -> rusqlite::Result<Vec<BotPlace>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&bot_places_sql(
            "?2",
            "JOIN city_members cm ON cm.person_id = a.owner_id AND cm.city_id = ?2
             LEFT JOIN bot_homes bh ON bh.agent_id = a.id
                AND bh.house_id IN (SELECT id FROM houses WHERE city_id = ?2)
                AND EXISTS (SELECT 1 FROM residents r
                            WHERE r.house_id = bh.house_id AND r.person_id = a.owner_id)
             WHERE NOT EXISTS (SELECT 1 FROM bot_homes hb WHERE hb.agent_id = a.id AND hb.is_house = 1)",
        ))?;
        let rows = stmt.query_map(params![now, city_id], |row| bot_place_from_row(row, now))?;
        rows.collect()
    }

    // Houses

    /// Builds a house: its Living Room and Porch, the house itself, and the creator as a resident
    /// and a member of both rooms.
    /// The new house, or `None` with nothing made if its builder already lives in this city. That's
    /// decided here so two requests at the same moment can't both get past it (R5).
    pub fn create_house(&self, house: &NewHouse) -> rusqlite::Result<Option<House>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if lives_elsewhere_in_city(&tx, house.city_id, house.created_by, house.id)? {
            return Ok(None);
        }
        let living_room_name = format!("{} Living Room", house.name);
        let porch_name = format!("{} Porch", house.name);
        insert_room(
            &tx,
            house.living_room_id,
            &living_room_name,
            house.living_room_code,
            house.created_by,
            house.now,
        )?;
        insert_room(&tx, house.porch_id, &porch_name, house.porch_code, house.created_by, house.now)?;
        tx.execute(
            "INSERT INTO houses (id, city_id, name, invite_code, living_room, porch, created_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                house.id,
                house.city_id,
                house.name,
                house.invite_code,
                house.living_room_id,
                house.porch_id,
                house.created_by,
                house.now
            ],
        )?;
        tx.execute(
            "INSERT INTO residents (house_id, person_id, joined_at) VALUES (?, ?, ?)",
            params![house.id, house.created_by, house.now],
        )?;
        tx.commit()?;
        Ok(Some(House {
            id: house.id.to_string(),
            city_id: house.city_id.to_string(),
            name: house.name.to_string(),
            invite_code: house.invite_code.to_string(),
            living_room_id: house.living_room_id.to_string(),
            porch_id: house.porch_id.to_string(),
            created_by: house.created_by.to_string(),
            created_at: house.now,
        }))
    }

    pub fn house(&self, id: &str) -> rusqlite::Result<Option<House>> {
        self.conn()
            .query_row(
                &format!("SELECT {HOUSE_COLS} FROM houses h WHERE h.id = ?"),
                [id],
                house_from_row,
            )
            .optional()
    }

    pub fn house_by_invite(&self, code: &str) -> rusqlite::Result<Option<House>> {
        self.conn()
            .query_row(
                &format!("SELECT {HOUSE_COLS} FROM houses h WHERE h.invite_code = ?"),
                [code],
                house_from_row,
            )
            .optional()
    }

    pub fn set_house_invite(&self, house_id: &str, code: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE houses SET invite_code = ? WHERE id = ?",
            params![code, house_id],
        )?;
        Ok(())
    }

    /// R5: uses up a house's link. True for the one caller whose code was still the link, however
    /// many arrive at the same moment.
    pub fn take_house_invite(&self, house_id: &str, code: &str, new_code: &str) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE houses SET invite_code = ? WHERE id = ? AND invite_code = ?",
            params![new_code, house_id, code],
        )?;
        Ok(changed > 0)
    }

    /// The house a Living Room or Porch belongs to. A Town Square has none.
    pub fn house_for_room(&self, hangout_id: &str) -> rusqlite::Result<Option<House>> {
        self.conn()
            .query_row(
                &format!("SELECT {HOUSE_COLS} FROM houses h WHERE h.living_room = ?1 OR h.porch = ?1"),
                [hangout_id],
                house_from_row,
            )
            .optional()
    }

    /// Town Squares, Living Rooms and Porches, which people get into by being in the city or the
    /// house and never through a hangout invite.
    pub fn is_managed_room(&self, hangout_id: &str) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            "SELECT 1 WHERE EXISTS (SELECT 1 FROM houses WHERE living_room = ?1 OR porch = ?1)
                OR EXISTS (SELECT 1 FROM cities WHERE square_id = ?1)",
            [hangout_id],
        )
    }

    // Spaces and floors

    /// This person's house, built with its one floor if they have none (S2).
    pub fn ensure_house(
        &self,
        id: &str,
        name: &str,
        invite_code: &str,
        owner_id: &str,
        now: i64,
    ) -> rusqlite::Result<Space> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let house = |tx: &Connection| {
            tx.query_row(
                &format!("SELECT {SPACE_COLS} FROM spaces s WHERE s.owner_id = ? AND s.kind = 'house'"),
                [owner_id],
                space_from_row,
            )
            .optional()
        };
        if let Some(found) = house(&tx)? {
            return Ok(found);
        }
        insert_space(&tx, id, "house", name, invite_code, owner_id, now)?;
        let made = house(&tx)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        tx.commit()?;
        Ok(made)
    }

    pub fn space(&self, id: &str) -> rusqlite::Result<Option<Space>> {
        self.conn()
            .query_row(&format!("SELECT {SPACE_COLS} FROM spaces s WHERE s.id = ?"), [id], space_from_row)
            .optional()
    }

    /// The space a room is a floor of, if it is one.
    pub fn space_for_room(&self, hangout_id: &str) -> rusqlite::Result<Option<Space>> {
        self.conn()
            .query_row(
                &format!(
                    "SELECT {SPACE_COLS} FROM spaces s JOIN floors f ON f.space_id = s.id
                     WHERE f.hangout_id = ?"
                ),
                [hangout_id],
                space_from_row,
            )
            .optional()
    }

    /// The space a tile was last the door of. This proves nothing alone: see `spaces::is_door_to`.
    pub fn space_on_plot(&self, world_id: &str, x: i64, y: i64) -> rusqlite::Result<Option<Space>> {
        self.conn()
            .query_row(
                &format!(
                    "SELECT {SPACE_COLS} FROM spaces s JOIN space_plots d ON d.space_id = s.id
                     WHERE d.world_id = ? AND d.x = ? AND d.y = ?"
                ),
                params![world_id, x, y],
                space_from_row,
            )
            .optional()
    }

    /// Where a space's door was last put.
    pub fn plot_of(&self, space_id: &str, world_id: &str) -> rusqlite::Result<Option<(i64, i64)>> {
        self.conn()
            .query_row(
                "SELECT x, y FROM space_plots WHERE space_id = ? AND world_id = ?",
                params![space_id, world_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    /// Makes a tile the door of a space. A space has one door and a tile opens one space, so this
    /// replaces whatever either of them had before.
    pub fn set_plot(&self, world_id: &str, x: i64, y: i64, space_id: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO space_plots (world_id, x, y, space_id) VALUES (?, ?, ?, ?)",
            params![world_id, x, y, space_id],
        )?;
        Ok(())
    }

    /// The third spaces this person holds, oldest first. Times are whole seconds and ids are random,
    /// so two made in one second are told apart by which was stored first.
    pub fn third_spaces_of(&self, owner_id: &str) -> rusqlite::Result<Vec<Space>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SPACE_COLS} FROM {HELD_SPACES}
             WHERE s.owner_id = ? AND s.kind = 'third' ORDER BY s.created_at, s.rowid"
        ))?;
        let rows = stmt.query_map([owner_id], space_from_row)?;
        rows.collect()
    }

    /// The floor this room is, or `None` for a Town Square, Living Room or Porch.
    pub fn floor(&self, hangout_id: &str) -> rusqlite::Result<Option<Floor>> {
        self.conn()
            .query_row(
                &format!("SELECT {FLOOR_COLS} FROM {FLOORS} WHERE f.hangout_id = ?"),
                [hangout_id],
                floor_from_row,
            )
            .optional()
    }

    /// A space's floors, lowest first.
    pub fn floors_of(&self, space_id: &str) -> rusqlite::Result<Vec<Floor>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {FLOOR_COLS} FROM {FLOORS} WHERE f.space_id = ? ORDER BY f.level"
        ))?;
        let rows = stmt.query_map([space_id], floor_from_row)?;
        rows.collect()
    }

    /// A first floor still called what the space is called follows the space's new name, so a room
    /// with one floor doesn't end up with two names.
    pub fn change_space(
        &self,
        space: &Space,
        name: Option<&str>,
        about: Option<&str>,
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE spaces SET name = COALESCE(?2, name), about = COALESCE(?3, about) WHERE id = ?1",
            params![space.id, name, about],
        )?;
        if let Some(name) = name {
            tx.execute(
                "UPDATE hangouts SET name = ?2 WHERE id = ?1 AND name = ?3",
                params![space.id, name, space.name],
            )?;
        }
        tx.commit()
    }

    /// Adds a floor above the others, with only its maker in it. `None` if the space is full (S4).
    pub fn add_floor(&self, floor: &NewFloor, max: i64) -> rusqlite::Result<Option<Floor>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let (count, top): (i64, i64) = tx.query_row(
            "SELECT COUNT(*), COALESCE(MAX(level), -1) FROM floors WHERE space_id = ?",
            [floor.space_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if count >= max {
            return Ok(None);
        }
        insert_room(&tx, floor.id, floor.name, floor.invite_code, floor.created_by, floor.now)?;
        tx.execute(
            "INSERT INTO floors (hangout_id, space_id, level) VALUES (?, ?, ?)",
            params![floor.id, floor.space_id, top + 1],
        )?;
        tx.commit()?;
        // The connection's lock isn't reentrant, and `floor` takes it.
        drop(conn);
        self.floor(floor.id)
    }

    /// Changes a floor and returns the names of any bots this took off it. In one step, so no bot
    /// is on a public floor for a moment on its way out (S25), and so walk-ins end with the floor
    /// going private (S26) and don't come back if it opens again. `None`, and nothing changes, if
    /// this would open a floor holding a kind that stays private (I11): that's checked here and not
    /// by the caller, so an item can't be placed between the check and the floor opening.
    pub fn change_floor(&self, change: &FloorChange) -> rusqlite::Result<Option<Vec<String>>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let access: String =
            tx.query_row("SELECT access FROM floors WHERE hangout_id = ?", [change.id], |row| row.get(0))?;
        let mut taken_off = Vec::new();
        if change.access == Some("public") && access != "public" {
            for kind in change.private_only {
                if exists(&tx, "SELECT 1 FROM floor_items WHERE hangout_id = ? AND kind = ?", params![change.id, kind])? {
                    return Ok(None);
                }
            }
            let others = "SELECT a.id FROM agents a JOIN room_agents r ON r.agent_id = a.id
                          WHERE r.hangout_id = ?1 AND a.owner_id != ?2";
            {
                let mut stmt = tx.prepare(&format!(
                    "SELECT name FROM agents WHERE id IN ({others}) ORDER BY created_at, id"
                ))?;
                let names = stmt.query_map(params![change.id, change.keep_bots_of], |row| row.get(0))?;
                taken_off = names.collect::<rusqlite::Result<Vec<String>>>()?;
            }
            tx.execute(
                &format!("DELETE FROM room_agents WHERE hangout_id = ?1 AND agent_id IN ({others})"),
                params![change.id, change.keep_bots_of],
            )?;
        }
        if change.access == Some("private") {
            tx.execute("DELETE FROM floor_visits WHERE hangout_id = ?", [change.id])?;
        }
        tx.execute(
            "UPDATE floors SET access = COALESCE(?2, access), size = COALESCE(?3, size),
                paint = COALESCE(?4, paint), flooring = COALESCE(?5, flooring),
                hidden = COALESCE(?6, hidden)
             WHERE hangout_id = ?1",
            params![change.id, change.access, change.size, change.paint, change.flooring, change.hidden],
        )?;
        if let Some(name) = change.name {
            tx.execute("UPDATE hangouts SET name = ?2 WHERE id = ?1", params![change.id, name])?;
        }
        tx.commit()?;
        Ok(Some(taken_off))
    }

    /// Swaps a floor with the one above or below it. At the end of the stack this does nothing.
    pub fn move_floor(&self, floor: &Floor, up: bool) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let neighbour = if up {
            "SELECT hangout_id, level FROM floors WHERE space_id = ? AND level > ? ORDER BY level LIMIT 1"
        } else {
            "SELECT hangout_id, level FROM floors WHERE space_id = ? AND level < ? ORDER BY level DESC LIMIT 1"
        };
        let found: Option<(String, i64)> = tx
            .query_row(neighbour, params![floor.space_id, floor.level], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        if let Some((other, level)) = found {
            // Through a level nothing uses, because two floors can't share one even for a moment.
            let place = "UPDATE floors SET level = ? WHERE hangout_id = ?";
            tx.execute(place, params![-1, floor.id])?;
            tx.execute(place, params![floor.level, other])?;
            tx.execute(place, params![level, floor.id])?;
        }
        tx.commit()
    }

    /// Deletes rooms and everything in them: chat, notes, plans, check-ins and who was there. A
    /// space goes with the room it began in.
    pub fn remove_rooms(&self, hangout_ids: &[&str]) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for id in hangout_ids {
            tx.execute("DELETE FROM hangouts WHERE id = ?", [id])?;
        }
        tx.commit()
    }

    /// How many are on a floor: members, their bots, and walk-ins and theirs (S13).
    pub fn on_floor(&self, hangout_id: &str, now: i64) -> rusqlite::Result<i64> {
        count_on_floor(&self.conn(), hangout_id, now)
    }

    /// The floors someone walked onto and is still on. `who` is a person's id or their bot's.
    pub fn walked_onto(&self, who: &str, now: i64) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT hangout_id FROM floor_visits WHERE who = ? AND until > ? ORDER BY since")?;
        let rows = stmt.query_map(params![who, now], |row| row.get(0))?;
        rows.collect()
    }

    /// Walks someone onto a public floor if they fit, off any other floor they walked onto (S38).
    /// Someone already there stays where they were in the floor's history and gets more time (S22).
    pub fn walk_in(
        &self,
        visit: &NewVisit,
        capacity: i64,
        max_bots: i64,
    ) -> rusqlite::Result<WalkedIn> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if let Some(agent_id) = visit.agent_id {
            let on_trip = exists(
                &tx,
                "SELECT 1 FROM trips WHERE agent_id = ? AND status = 'here' AND ends_at > ?",
                params![agent_id, visit.now],
            )?;
            if on_trip {
                return Ok(WalkedIn::OnTrip);
            }
        }
        let here = exists(
            &tx,
            "SELECT 1 FROM floor_visits WHERE hangout_id = ? AND who = ? AND until > ?",
            params![visit.hangout_id, visit.who, visit.now],
        )?;
        if !here {
            if count_on_floor(&tx, visit.hangout_id, visit.now)? >= capacity {
                return Ok(WalkedIn::Full);
            }
            if visit.agent_id.is_some() {
                let bots: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM floor_visits
                     WHERE hangout_id = ? AND person_id = ? AND agent_id IS NOT NULL AND until > ?",
                    params![visit.hangout_id, visit.person_id, visit.now],
                    |row| row.get(0),
                )?;
                if bots >= max_bots {
                    return Ok(WalkedIn::TooManyBots);
                }
            }
        }
        tx.execute(
            "DELETE FROM floor_visits WHERE who = ? AND hangout_id != ?",
            params![visit.who, visit.hangout_id],
        )?;
        tx.execute(
            "INSERT INTO floor_visits (hangout_id, who, person_id, agent_id, since, until)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (hangout_id, who) DO UPDATE SET
                until = excluded.until,
                since = CASE WHEN floor_visits.until > ?5 THEN floor_visits.since ELSE excluded.since END",
            params![visit.hangout_id, visit.who, visit.person_id, visit.agent_id, visit.now, visit.until],
        )?;
        tx.commit()?;
        Ok(WalkedIn::In)
    }

    /// Ends a walk-in. `who` is a person's id or their bot's.
    pub fn end_visit(&self, hangout_id: &str, who: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "DELETE FROM floor_visits WHERE hangout_id = ? AND who = ?",
            params![hangout_id, who],
        )?;
        Ok(())
    }

    /// Moves a walk-in's times, which is how a test makes one run out. False if there's none.
    pub fn set_visit_times(
        &self,
        hangout_id: &str,
        who: &str,
        since: i64,
        until: i64,
    ) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE floor_visits SET since = ?, until = ? WHERE hangout_id = ? AND who = ?",
            params![since, until, hangout_id, who],
        )?;
        Ok(changed > 0)
    }

    /// Takes someone and their bots off every floor of a space, and keeps them off (S27).
    pub fn turn_out(&self, space_id: &str, person_id: &str, now: i64) -> rusqlite::Result<()> {
        let floors = "SELECT hangout_id FROM floors WHERE space_id = ?1";
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            &format!(
                "DELETE FROM room_agents WHERE hangout_id IN ({floors})
                 AND agent_id IN (SELECT id FROM agents WHERE owner_id = ?2)"
            ),
            params![space_id, person_id],
        )?;
        tx.execute(
            &format!("DELETE FROM members WHERE hangout_id IN ({floors}) AND person_id = ?2"),
            params![space_id, person_id],
        )?;
        tx.execute(
            &format!("DELETE FROM floor_visits WHERE hangout_id IN ({floors}) AND person_id = ?2"),
            params![space_id, person_id],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO turned_out (space_id, person_id, created_at) VALUES (?, ?, ?)",
            params![space_id, person_id, now],
        )?;
        tx.commit()
    }

    pub fn let_back(&self, space_id: &str, person_id: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "DELETE FROM turned_out WHERE space_id = ? AND person_id = ?",
            params![space_id, person_id],
        )?;
        Ok(())
    }

    pub fn is_turned_out(&self, space_id: &str, person_id: &str) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            "SELECT 1 FROM turned_out WHERE space_id = ? AND person_id = ?",
            params![space_id, person_id],
        )
    }

    pub fn turned_out_of(&self, space_id: &str) -> rusqlite::Result<Vec<Person>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT p.id, p.name, p.color FROM people p
             JOIN turned_out t ON t.person_id = p.id
             WHERE t.space_id = ? ORDER BY t.created_at, p.id",
        )?;
        let rows = stmt.query_map([space_id], person_from_row)?;
        rows.collect()
    }

    /// Everyone who is a member of any floor of a space.
    pub fn members_of_space(&self, space_id: &str) -> rusqlite::Result<Vec<Person>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT p.id, p.name, p.color FROM people p
             WHERE p.id IN (SELECT m.person_id FROM members m
                            JOIN floors f ON f.hangout_id = m.hangout_id WHERE f.space_id = ?)
             ORDER BY p.created_at, p.id",
        )?;
        let rows = stmt.query_map([space_id], person_from_row)?;
        rows.collect()
    }

    /// Everyone on a public floor of a space who walked in or sent a bot.
    pub fn walk_ins_of_space(&self, space_id: &str, now: i64) -> rusqlite::Result<Vec<Person>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT p.id, p.name, p.color FROM people p
             WHERE p.id IN (SELECT v.person_id FROM floor_visits v
                            JOIN floors f ON f.hangout_id = v.hangout_id
                            WHERE f.space_id = ? AND f.access = 'public' AND v.until > ?)
             ORDER BY p.created_at, p.id",
        )?;
        let rows = stmt.query_map(params![space_id, now], person_from_row)?;
        rows.collect()
    }

    /// The public floors this person hasn't been turned out of, fullest first (S28). `capacity`
    /// is filled in by `spaces`, which knows what the sizes hold.
    pub fn public_floors(
        &self,
        person_id: &str,
        now: i64,
        limit: i64,
    ) -> rusqlite::Result<Vec<PublicFloor>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT f.hangout_id, h.name, s.name, p.name, s.about, f.size, {ON_FLOOR} AS here
             FROM floors f
             JOIN hangouts h ON h.id = f.hangout_id
             JOIN spaces s ON s.id = f.space_id
             JOIN people p ON p.id = s.owner_id
             WHERE f.access = 'public'
               AND NOT EXISTS (SELECT 1 FROM turned_out t WHERE t.space_id = s.id AND t.person_id = ?2)
             ORDER BY here DESC, h.created_at DESC, f.hangout_id LIMIT ?3"
        ))?;
        let rows = stmt.query_map(params![now, person_id, limit], |row| {
            Ok(PublicFloor {
                id: row.get(0)?,
                name: row.get(1)?,
                space_name: row.get(2)?,
                owner_name: row.get(3)?,
                about: row.get(4)?,
                size: row.get(5)?,
                capacity: 0,
                here: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn residents(&self, house_id: &str) -> rusqlite::Result<Vec<Person>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT p.id, p.name, p.color FROM people p
             JOIN residents r ON r.person_id = p.id
             WHERE r.house_id = ? ORDER BY r.joined_at, p.id",
        )?;
        let rows = stmt.query_map([house_id], person_from_row)?;
        rows.collect()
    }

    pub fn is_resident(&self, house_id: &str, person_id: &str) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            "SELECT 1 FROM residents WHERE house_id = ? AND person_id = ?",
            params![house_id, person_id],
        )
    }

    pub fn resident_count(&self, house_id: &str) -> rusqlite::Result<i64> {
        self.conn().query_row(
            "SELECT COUNT(*) FROM residents WHERE house_id = ?",
            [house_id],
            |row| row.get(0),
        )
    }

    /// Joining a house also joins its city, and both its rooms. Their bots come in separately.
    /// False, with nothing changed, if they live in another house in this city. That's decided here
    /// so two requests at the same moment can't both get past it (R5).
    pub fn add_resident(&self, house_id: &str, person_id: &str, now: i64) -> rusqlite::Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let (city_id, living_room, porch): (String, String, String) = tx.query_row(
            "SELECT city_id, living_room, porch FROM houses WHERE id = ?",
            [house_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if lives_elsewhere_in_city(&tx, &city_id, person_id, house_id)? {
            return Ok(false);
        }
        join_city(&tx, &city_id, person_id, now)?;
        tx.execute(
            "INSERT OR IGNORE INTO residents (house_id, person_id, joined_at) VALUES (?, ?, ?)",
            params![house_id, person_id, now],
        )?;
        for room in [&living_room, &porch] {
            tx.execute(
                "INSERT OR IGNORE INTO members (hangout_id, person_id, joined_at) VALUES (?, ?, ?)",
                params![room, person_id, now],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// Takes the person and their bots out of both rooms and forgets where their bots lived in
    /// this house. They stay in the city.
    pub fn remove_resident(&self, house_id: &str, person_id: &str) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let (living_room, porch): (String, String) = tx.query_row(
            "SELECT living_room, porch FROM houses WHERE id = ?",
            [house_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        for room in [&living_room, &porch] {
            tx.execute(
                "DELETE FROM room_agents WHERE hangout_id = ?1 AND agent_id IN
                    (SELECT id FROM agents WHERE owner_id = ?2)",
                params![room, person_id],
            )?;
            tx.execute(
                "DELETE FROM members WHERE hangout_id = ? AND person_id = ?",
                params![room, person_id],
            )?;
        }
        // A house bot stays with the house, and a bot that holds keys stays put. Without these
        // rows either would turn into an ordinary bot that can walk into a room with guests.
        tx.execute(
            "DELETE FROM bot_homes WHERE house_id = ?1 AND is_house = 0 AND stays_home = 0
                AND agent_id IN (SELECT id FROM agents WHERE owner_id = ?2)",
            params![house_id, person_id],
        )?;
        // Someone who moved out no longer speaks for the house.
        tx.execute(
            "UPDATE trips SET host_ok_by = NULL, host_ok_id = NULL
             WHERE house_id = ?1 AND host_ok_id = ?2 AND status = 'asking'",
            params![house_id, person_id],
        )?;
        tx.execute(
            "DELETE FROM residents WHERE house_id = ? AND person_id = ?",
            params![house_id, person_id],
        )?;
        tx.commit()
    }

    /// In the order they were built, which is the order the map gives them plots.
    pub fn houses_in_city(&self, city_id: &str) -> rusqlite::Result<Vec<House>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {HOUSE_COLS} FROM houses h WHERE h.city_id = ? ORDER BY h.created_at, h.rowid"
        ))?;
        let rows = stmt.query_map([city_id], house_from_row)?;
        rows.collect()
    }

    /// Who lives where in a city, as (house id, person id), houses in the order they were built.
    pub fn city_residents(&self, city_id: &str) -> rusqlite::Result<Vec<(String, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT r.house_id, r.person_id FROM residents r
             JOIN houses h ON h.id = r.house_id
             WHERE h.city_id = ? ORDER BY h.created_at, h.rowid, r.joined_at, r.person_id",
        )?;
        let rows = stmt.query_map([city_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    pub fn house_of_person_in_city(
        &self,
        city_id: &str,
        person_id: &str,
    ) -> rusqlite::Result<Option<House>> {
        self.conn()
            .query_row(
                &format!(
                    "SELECT {HOUSE_COLS} FROM houses h
                     JOIN residents r ON r.house_id = h.id
                     WHERE h.city_id = ? AND r.person_id = ? ORDER BY h.created_at, h.rowid LIMIT 1"
                ),
                params![city_id, person_id],
                house_from_row,
            )
            .optional()
    }

    pub fn houses_for_person(&self, person_id: &str) -> rusqlite::Result<Vec<House>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {HOUSE_COLS} FROM houses h
             JOIN residents r ON r.house_id = h.id
             WHERE r.person_id = ? ORDER BY r.joined_at, h.id"
        ))?;
        let rows = stmt.query_map([person_id], house_from_row)?;
        rows.collect()
    }

    /// The bots that live in a house, with where each is now. A bot left behind by someone who
    /// moved out isn't one of them, unless it's the house bot, which belongs to the house.
    pub fn bots_of_house(&self, house_id: &str, now: i64) -> rusqlite::Result<Vec<BotPlace>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&bot_places_sql(
            "(SELECT city_id FROM houses WHERE id = ?2)",
            "JOIN bot_homes bh ON bh.agent_id = a.id AND bh.house_id = ?2
                AND (bh.is_house = 1 OR EXISTS (SELECT 1 FROM residents r
                     WHERE r.house_id = ?2 AND r.person_id = a.owner_id))",
        ))?;
        let rows = stmt.query_map(params![now, house_id], |row| bot_place_from_row(row, now))?;
        rows.collect()
    }

    // Bot homes

    pub fn bot_home(&self, agent_id: &str) -> rusqlite::Result<Option<BotHome>> {
        self.conn()
            .query_row(
                "SELECT agent_id, house_id, stays_home, is_house FROM bot_homes WHERE agent_id = ?",
                [agent_id],
                |row| {
                    Ok(BotHome {
                        agent_id: row.get(0)?,
                        house_id: row.get(1)?,
                        stays_home: row.get(2)?,
                        is_house: row.get(3)?,
                    })
                },
            )
            .optional()
    }

    /// Gives a bot a home or changes it. `is_house` only counts when the row is first made:
    /// nothing turns an ordinary bot into the house bot, or back.
    pub fn set_bot_home(
        &self,
        agent_id: &str,
        house_id: &str,
        stays_home: bool,
        is_house: bool,
    ) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO bot_homes (agent_id, house_id, stays_home, is_house) VALUES (?, ?, ?, ?)
             ON CONFLICT (agent_id) DO UPDATE SET
                house_id = excluded.house_id, stays_home = excluded.stays_home",
            params![agent_id, house_id, stays_home, is_house],
        )?;
        Ok(())
    }

    /// For a bot that stays home: it keeps only the one room.
    pub fn remove_agent_from_other_rooms(
        &self,
        agent_id: &str,
        keep_hangout_id: &str,
    ) -> rusqlite::Result<()> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM room_agents WHERE agent_id = ? AND hangout_id != ?",
            params![agent_id, keep_hangout_id],
        )?;
        conn.execute("DELETE FROM floor_visits WHERE agent_id = ?", [agent_id])?;
        Ok(())
    }

    pub fn house_bot(&self, house_id: &str, now: i64) -> rusqlite::Result<Option<Agent>> {
        self.conn()
            .query_row(
                &format!(
                    "SELECT {AGENT_COLS} FROM {AGENTS}
                     JOIN bot_homes bh ON bh.agent_id = a.id
                     WHERE bh.house_id = ? AND bh.is_house = 1"
                ),
                [house_id],
                |row| agent_from_row(row, now),
            )
            .optional()
    }

    /// Makes the house's bot, in no rooms. Returns false, and makes nothing, if it already has one.
    pub fn create_house_bot(
        &self,
        house_id: &str,
        agent_id: &str,
        owner_id: &str,
        name: &str,
        token_hash: &str,
        now: i64,
    ) -> rusqlite::Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if exists(
            &tx,
            "SELECT 1 FROM bot_homes WHERE house_id = ? AND is_house = 1",
            [house_id],
        )? {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO agents (id, owner_id, name, token_hash, created_at) VALUES (?, ?, ?, ?, ?)",
            params![agent_id, owner_id, name, token_hash, now],
        )?;
        tx.execute(
            "INSERT INTO bot_homes (agent_id, house_id, stays_home, is_house) VALUES (?, ?, 0, 1)",
            params![agent_id, house_id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    // Trips

    pub fn insert_trip(&self, trip: &NewTrip) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO trips (id, agent_id, house_id, kind, goal, minutes, status, asked_by,
                                owner_ok_by, host_ok_by, pass_id, expires_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, 'asking', ?, ?, ?, ?, ?, ?)",
            params![
                trip.id,
                trip.agent_id,
                trip.house_id,
                trip.kind,
                trip.goal,
                trip.minutes,
                trip.asked_by,
                trip.owner_ok_by,
                trip.host_ok_by,
                trip.pass_id,
                trip.expires_at,
                trip.now
            ],
        )?;
        Ok(())
    }

    pub fn trip(&self, id: &str, now: i64) -> rusqlite::Result<Option<Trip>> {
        self.conn()
            .query_row(
                &format!("SELECT {TRIP_COLS} FROM {TRIPS} WHERE t.id = ?"),
                [id],
                |row| trip_from_row(row, now),
            )
            .optional()
    }

    /// Asks waiting for an answer, trips under way, and anything created since `since`, newest first.
    pub fn trips_for_house(&self, house_id: &str, since: i64, now: i64) -> rusqlite::Result<Vec<Trip>> {
        load_trips(&self.conn(), "t.house_id = ?1", house_id, since, now)
    }

    pub fn trips_for_bot(&self, agent_id: &str, since: i64, now: i64) -> rusqlite::Result<Vec<Trip>> {
        load_trips(&self.conn(), "t.agent_id = ?1", agent_id, since, now)
    }

    /// Trips of all of one person's bots.
    pub fn trips_for_owner(&self, owner_id: &str, since: i64, now: i64) -> rusqlite::Result<Vec<Trip>> {
        load_trips(&self.conn(), "a.owner_id = ?1", owner_id, since, now)
    }

    /// The trip the bot is on right now.
    pub fn live_trip(&self, agent_id: &str, now: i64) -> rusqlite::Result<Option<Trip>> {
        self.conn()
            .query_row(
                &format!(
                    "SELECT {TRIP_COLS} FROM {TRIPS}
                     WHERE t.agent_id = ?1 AND t.status = 'here' AND t.ends_at > ?2
                     ORDER BY t.arrived_at DESC LIMIT 1"
                ),
                params![agent_id, now],
                |row| trip_from_row(row, now),
            )
            .optional()
    }

    /// A request from this bot to this house that hasn't been answered or lapsed.
    pub fn open_ask(&self, agent_id: &str, house_id: &str, now: i64) -> rusqlite::Result<Option<Trip>> {
        self.conn()
            .query_row(
                &format!(
                    "SELECT {TRIP_COLS} FROM {TRIPS}
                     WHERE t.agent_id = ?1 AND t.house_id = ?2 AND t.status = 'asking' AND t.expires_at > ?3
                     ORDER BY t.created_at DESC LIMIT 1"
                ),
                params![agent_id, house_id, now],
                |row| trip_from_row(row, now),
            )
            .optional()
    }

    /// How many requests one person's bots have open at a house.
    pub fn open_asks_from_owner(
        &self,
        owner_id: &str,
        house_id: &str,
        now: i64,
    ) -> rusqlite::Result<i64> {
        self.conn().query_row(
            "SELECT COUNT(*) FROM trips t JOIN agents a ON a.id = t.agent_id
             WHERE a.owner_id = ? AND t.house_id = ? AND t.status = 'asking' AND t.expires_at > ?",
            params![owner_id, house_id, now],
            |row| row.get(0),
        )
    }

    /// Records the bot's owner saying yes. Keeps the first yes. False if the ask is no longer open.
    pub fn trip_owner_ok(&self, trip_id: &str, person_id: &str, now: i64) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE trips SET owner_ok_by = COALESCE(owner_ok_by, ?2)
             WHERE id = ?1 AND status = 'asking' AND expires_at > ?3",
            params![trip_id, person_id, now],
        )?;
        Ok(changed > 0)
    }

    /// Records a resident of the house saying yes. Keeps the first yes. False if the ask is closed.
    /// The id is kept so `remove_resident` can take the yes back.
    pub fn trip_host_ok(&self, trip_id: &str, person_id: &str, now: i64) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE trips SET host_ok_by = COALESCE(host_ok_by, (SELECT name FROM people WHERE id = ?2)),
                              host_ok_id = COALESCE(host_ok_id, ?2)
             WHERE id = ?1 AND status = 'asking' AND expires_at > ?3",
            params![trip_id, person_id, now],
        )?;
        Ok(changed > 0)
    }

    /// Starts a trip that both sides said yes to (or a pass covers), for its `minutes` from now.
    /// A trip of this bot's that ran out is closed first, so only one that's still under way
    /// stops it. A walk-in doesn't: the bot leaves the floor (S40).
    pub fn arrive(&self, trip_id: &str, now: i64) -> rusqlite::Result<Arrival> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let trip: Option<(String, String, i64, i64, bool)> = tx
            .query_row(
                "SELECT agent_id, status, minutes, expires_at,
                        (owner_ok_by IS NOT NULL AND host_ok_by IS NOT NULL) OR pass_id IS NOT NULL
                 FROM trips WHERE id = ?",
                [trip_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()?;
        let Some((agent_id, status, minutes, expires_at, agreed)) = trip else {
            return Ok(Arrival::Closed);
        };
        if status != "asking" || expires_at <= now {
            return Ok(Arrival::Closed);
        }
        if !agreed {
            return Ok(Arrival::Unapproved);
        }
        tx.execute(
            "UPDATE trips SET status = 'ended' WHERE agent_id = ? AND status = 'here' AND ends_at <= ?",
            params![agent_id, now],
        )?;
        let busy = exists(
            &tx,
            "SELECT 1 FROM trips WHERE agent_id = ? AND status = 'here' AND ends_at > ?",
            params![agent_id, now],
        )?;
        if !busy {
            tx.execute(
                "UPDATE trips SET status = 'here', arrived_at = ?2, ends_at = ?3 WHERE id = ?1",
                params![trip_id, now, now + minutes * 60],
            )?;
            tx.execute("DELETE FROM floor_visits WHERE agent_id = ?", [&agent_id])?;
        }
        tx.commit()?;
        Ok(if busy { Arrival::Busy } else { Arrival::Arrived })
    }

    /// Ends an ask with a no. False if it was already answered or lapsed.
    pub fn decline_trip(&self, trip_id: &str, declined_by: &str, now: i64) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE trips SET status = 'declined', ended_by = ?2
             WHERE id = ?1 AND status = 'asking' AND expires_at > ?3",
            params![trip_id, declined_by, now],
        )?;
        Ok(changed > 0)
    }

    /// Ends a trip that's under way, at once. False if it had already run out or ended.
    pub fn end_trip(&self, trip_id: &str, ended_by: &str, now: i64) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE trips SET status = 'ended', ended_by = ?2
             WHERE id = ?1 AND status = 'here' AND ends_at > ?3",
            params![trip_id, ended_by, now],
        )?;
        Ok(changed > 0)
    }

    /// Ends the bot's trip and withdraws its open asks. Returns how many it changed.
    pub fn end_trips_of_bot(&self, agent_id: &str, ended_by: &str, now: i64) -> rusqlite::Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ended = tx.execute(
            "UPDATE trips SET status = 'ended', ended_by = ?2
             WHERE agent_id = ?1 AND status = 'here' AND ends_at > ?3",
            params![agent_id, ended_by, now],
        )?;
        let declined = tx.execute(
            "UPDATE trips SET status = 'declined', ended_by = ?2
             WHERE agent_id = ?1 AND status = 'asking' AND expires_at > ?3",
            params![agent_id, ended_by, now],
        )?;
        tx.commit()?;
        Ok(ended + declined)
    }

    /// Ends the trip a pass is covering, if one is under way.
    pub fn end_trip_for_pass(&self, pass_id: &str, ended_by: &str, now: i64) -> rusqlite::Result<usize> {
        self.conn().execute(
            "UPDATE trips SET status = 'ended', ended_by = ?2
             WHERE pass_id = ?1 AND status = 'here' AND ends_at > ?3",
            params![pass_id, ended_by, now],
        )
    }

    /// Moves the end of a trip that's under way. False if it isn't.
    pub fn set_trip_ends_at(&self, trip_id: &str, ends_at: i64, now: i64) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE trips SET ends_at = ?2 WHERE id = ?1 AND status = 'here' AND ends_at > ?3",
            params![trip_id, ends_at, now],
        )?;
        Ok(changed > 0)
    }

    /// Whether a resident of the house has said yes to a trip of this bot's that really started.
    /// A standing pass can only be offered to a bot that has.
    pub fn has_hosted_trip(&self, agent_id: &str, house_id: &str) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            "SELECT 1 FROM trips WHERE agent_id = ? AND house_id = ?
               AND host_ok_by IS NOT NULL AND arrived_at IS NOT NULL",
            params![agent_id, house_id],
        )
    }

    // Passes

    /// Offers a pass. False, and nothing changes, if the bot already has one for this house.
    pub fn insert_pass(&self, pass: &NewPass) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "INSERT OR IGNORE INTO passes (id, house_id, agent_id, max_minutes, granted_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![pass.id, pass.house_id, pass.agent_id, pass.max_minutes, pass.granted_by, pass.now],
        )?;
        Ok(changed > 0)
    }

    pub fn pass(&self, id: &str) -> rusqlite::Result<Option<Pass>> {
        self.conn()
            .query_row(
                &format!("SELECT {PASS_COLS} FROM {PASSES} WHERE p.id = ?"),
                [id],
                pass_from_row,
            )
            .optional()
    }

    pub fn passes_for_house(&self, house_id: &str) -> rusqlite::Result<Vec<Pass>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PASS_COLS} FROM {PASSES} WHERE p.house_id = ? ORDER BY p.created_at, p.id"
        ))?;
        let rows = stmt.query_map([house_id], pass_from_row)?;
        rows.collect()
    }

    /// Passes offered to any of one person's bots.
    pub fn passes_for_owner(&self, owner_id: &str) -> rusqlite::Result<Vec<Pass>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PASS_COLS} FROM {PASSES} WHERE a.owner_id = ? ORDER BY p.created_at, p.id"
        ))?;
        let rows = stmt.query_map([owner_id], pass_from_row)?;
        rows.collect()
    }

    pub fn pass_for_bot(&self, house_id: &str, agent_id: &str) -> rusqlite::Result<Option<Pass>> {
        self.conn()
            .query_row(
                &format!("SELECT {PASS_COLS} FROM {PASSES} WHERE p.house_id = ? AND p.agent_id = ?"),
                params![house_id, agent_id],
                pass_from_row,
            )
            .optional()
    }

    /// False if it was already accepted or is gone.
    pub fn accept_pass(&self, id: &str, person_id: &str) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE passes SET accepted_by = ? WHERE id = ? AND accepted_by IS NULL",
            params![person_id, id],
        )?;
        Ok(changed > 0)
    }

    // Keys (H8 to H13)

    /// H8: trips of this bot's to this house that arrived and that no resident ended.
    pub fn good_trips_to(&self, agent_id: &str, house_id: &str) -> rusqlite::Result<i64> {
        self.conn().query_row(
            "SELECT COUNT(*) FROM trips t
             WHERE t.agent_id = ? AND t.house_id = ? AND t.arrived_at IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM sent_home s WHERE s.trip_id = t.id)",
            params![agent_id, house_id],
            |row| row.get(0),
        )
    }

    pub fn add_pass_key(&self, pass_id: &str, key: &str, granted_by: &str, now: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO pass_keys (pass_id, key, granted_by, created_at) VALUES (?, ?, ?, ?)",
            params![pass_id, key, granted_by, now],
        )?;
        Ok(())
    }

    pub fn remove_pass_key(&self, pass_id: &str, key: &str) -> rusqlite::Result<()> {
        self.conn().execute("DELETE FROM pass_keys WHERE pass_id = ? AND key = ?", params![pass_id, key])?;
        Ok(())
    }

    /// H12: the keys this resident gave at this house. `granted_by` is a name, as on a pass.
    pub fn remove_keys_given_by(&self, house_id: &str, name: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "DELETE FROM pass_keys WHERE granted_by = ?
             AND pass_id IN (SELECT id FROM passes WHERE house_id = ?)",
            params![name, house_id],
        )?;
        Ok(())
    }

    /// H11: who gave this bot the key, if its accepted pass to this house has it.
    pub fn key_giver(&self, agent_id: &str, house_id: &str, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn()
            .query_row(
                "SELECT k.granted_by FROM passes p JOIN pass_keys k ON k.pass_id = p.id
                 WHERE p.agent_id = ? AND p.house_id = ? AND p.accepted_by IS NOT NULL AND k.key = ?",
                params![agent_id, house_id, key],
                |row| row.get(0),
            )
            .optional()
    }

    /// H13: the accepted passes of this bot's that have a key, with the house's name.
    pub fn keys_of_bot(&self, agent_id: &str) -> rusqlite::Result<Vec<(Pass, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PASS_COLS}, h.name FROM {PASSES} JOIN houses h ON h.id = p.house_id
             WHERE p.agent_id = ? AND p.accepted_by IS NOT NULL
               AND EXISTS (SELECT 1 FROM pass_keys k WHERE k.pass_id = p.id)
             ORDER BY p.created_at, p.id"
        ))?;
        let rows = stmt.query_map([agent_id], |row| Ok((pass_from_row(row)?, row.get(9)?)))?;
        rows.collect()
    }

    pub fn delete_pass(&self, id: &str) -> rusqlite::Result<bool> {
        let changed = self.conn().execute("DELETE FROM passes WHERE id = ?", [id])?;
        Ok(changed > 0)
    }

    // Scenes

    /// Adds a scene. False if the house already has one with that name.
    pub fn insert_scene(&self, id: &str, house_id: &str, name: &str, now: i64) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "INSERT OR IGNORE INTO scenes (id, house_id, name, created_at) VALUES (?, ?, ?, ?)",
            params![id, house_id, name, now],
        )?;
        Ok(changed > 0)
    }

    pub fn scene(&self, id: &str) -> rusqlite::Result<Option<Scene>> {
        self.conn()
            .query_row(
                "SELECT id, house_id, name FROM scenes WHERE id = ?",
                [id],
                scene_from_row,
            )
            .optional()
    }

    pub fn scenes(&self, house_id: &str) -> rusqlite::Result<Vec<Scene>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, house_id, name FROM scenes WHERE house_id = ? ORDER BY created_at, name",
        )?;
        let rows = stmt.query_map([house_id], scene_from_row)?;
        rows.collect()
    }

    /// Takes its requests with it.
    pub fn delete_scene(&self, id: &str) -> rusqlite::Result<bool> {
        let changed = self.conn().execute("DELETE FROM scenes WHERE id = ?", [id])?;
        Ok(changed > 0)
    }

    /// Asks for a scene. False, and nothing is added, if it already has an open request.
    pub fn insert_scene_request(&self, request: &NewSceneRequest) -> rusqlite::Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if exists(
            &tx,
            "SELECT 1 FROM scene_requests
             WHERE scene_id = ? AND status IN ('waiting', 'approved') AND expires_at > ?",
            params![request.scene_id, request.now],
        )? {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO scene_requests (id, scene_id, house_id, asked_by, status, answered_by,
                                         expires_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                request.id,
                request.scene_id,
                request.house_id,
                request.asked_by,
                request.status,
                request.answered_by,
                request.expires_at,
                request.now
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn scene_request(&self, id: &str, now: i64) -> rusqlite::Result<Option<SceneRequest>> {
        self.conn()
            .query_row(
                &format!("SELECT {SCENE_REQUEST_COLS} FROM {SCENE_REQUESTS} WHERE r.id = ?"),
                [id],
                |row| scene_request_from_row(row, now),
            )
            .optional()
    }

    /// The request for this scene that's waiting or approved and hasn't lapsed.
    pub fn open_scene_request(&self, scene_id: &str, now: i64) -> rusqlite::Result<Option<SceneRequest>> {
        self.conn()
            .query_row(
                &format!(
                    "SELECT {SCENE_REQUEST_COLS} FROM {SCENE_REQUESTS}
                     WHERE r.scene_id = ? AND r.status IN ('waiting', 'approved') AND r.expires_at > ?
                     ORDER BY r.created_at DESC LIMIT 1"
                ),
                params![scene_id, now],
                |row| scene_request_from_row(row, now),
            )
            .optional()
    }

    /// Answers a waiting request with `approved` or `declined`. An approval gets a new
    /// `expires_at`, so it lapses a set time after it was given. False if it was no longer waiting.
    pub fn answer_scene_request(
        &self,
        id: &str,
        status: &str,
        answered_by: &str,
        expires_at: Option<i64>,
        now: i64,
    ) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE scene_requests
             SET status = ?2, answered_by = ?3, expires_at = COALESCE(?4, expires_at)
             WHERE id = ?1 AND status = 'waiting' AND expires_at > ?5",
            params![id, status, answered_by, expires_at, now],
        )?;
        Ok(changed > 0)
    }

    /// The house bot reporting `done` or `failed` for an approved request. False if it lapsed.
    pub fn finish_scene_request(&self, id: &str, result: &str, now: i64) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE scene_requests SET status = ?2
             WHERE id = ?1 AND status = 'approved' AND expires_at > ?3",
            params![id, result, now],
        )?;
        Ok(changed > 0)
    }

    /// What the house bot may see: approved requests that haven't lapsed, oldest first.
    pub fn scene_queue(&self, house_id: &str, now: i64) -> rusqlite::Result<Vec<QueueItem>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT r.id, s.name FROM {SCENE_REQUESTS}
             WHERE r.house_id = ? AND r.status = 'approved' AND r.expires_at > ?
             ORDER BY r.created_at, r.id"
        ))?;
        let rows = stmt.query_map(params![house_id, now], |row| {
            Ok(QueueItem {
                id: row.get(0)?,
                scene: row.get(1)?,
            })
        })?;
        rows.collect()
    }

    /// Open requests, plus anything created since `since`, newest first.
    pub fn recent_scene_requests(
        &self,
        house_id: &str,
        since: i64,
        now: i64,
    ) -> rusqlite::Result<Vec<SceneRequest>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SCENE_REQUEST_COLS} FROM {SCENE_REQUESTS}
             WHERE r.house_id = ?1
               AND (r.created_at >= ?2 OR (r.status IN ('waiting', 'approved') AND r.expires_at > ?3))
             ORDER BY r.created_at DESC, r.id LIMIT 50"
        ))?;
        let rows = stmt.query_map(params![house_id, since, now], |row| {
            scene_request_from_row(row, now)
        })?;
        rows.collect()
    }

    // House status

    pub fn set_house_status(&self, house_id: &str, state: &str, now: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO house_status (house_id, state, updated_at) VALUES (?, ?, ?)
             ON CONFLICT (house_id) DO UPDATE SET
                state = excluded.state, updated_at = excluded.updated_at",
            params![house_id, state, now],
        )?;
        Ok(())
    }

    pub fn house_status(&self, house_id: &str) -> rusqlite::Result<Option<HouseStatus>> {
        self.conn()
            .query_row(
                "SELECT state, updated_at FROM house_status WHERE house_id = ?",
                [house_id],
                |row| {
                    Ok(HouseStatus {
                        state: row.get(0)?,
                        updated_at: row.get(1)?,
                    })
                },
            )
            .optional()
    }

    // Guests. A guest is a bot on a live trip to the house whose Porch this is, a person or bot who
    // walked onto a public floor, or the owner of any of those bots.

    /// The bots visiting a Porch or a public floor, earliest arrival first, each with
    /// `visitor_until` set. Unlike `agents_in` this is for showing guests, never for notifying anyone.
    pub fn visitors_in(&self, hangout_id: &str, now: i64) -> rusqlite::Result<Vec<Agent>> {
        let conn = self.conn();
        let visitor = |row: &Row| -> rusqlite::Result<Agent> {
            let mut agent = agent_from_row(row, now)?;
            agent.visitor_until = row.get(11)?;
            Ok(agent)
        };
        let mut on_trips = conn.prepare(&format!(
            "SELECT {AGENT_COLS}, t.ends_at FROM {AGENTS}
             JOIN trips t ON t.agent_id = a.id
             JOIN houses ho ON ho.id = t.house_id
             WHERE {GUEST_ROOM} AND t.status = 'here' AND t.ends_at > ?2
               AND NOT EXISTS (SELECT 1 FROM room_agents r WHERE r.hangout_id = ?1 AND r.agent_id = a.id)
             ORDER BY t.arrived_at, a.id"
        ))?;
        let mut walked_in = conn.prepare(&format!(
            "SELECT {AGENT_COLS}, v.until FROM {AGENTS}
             JOIN floor_visits v ON v.agent_id = a.id
             JOIN floors f ON f.hangout_id = v.hangout_id
             WHERE v.hangout_id = ?1 AND f.access = 'public' AND v.until > ?2
               AND NOT EXISTS (SELECT 1 FROM room_agents r WHERE r.hangout_id = ?1 AND r.agent_id = a.id)
             ORDER BY v.since, a.id"
        ))?;
        // A room is a house's or a floor, so at most one of these finds anyone.
        let mut visitors = on_trips
            .query_map(params![hangout_id, now], visitor)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        visitors.extend(
            walked_in
                .query_map(params![hangout_id, now], visitor)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
        Ok(visitors)
    }

    /// The people visiting a room, and the owners of the bots visiting it, so the room can name and
    /// colour them. They aren't members, so `people_in` never lists them, and nothing that notifies
    /// people should either.
    pub fn visitor_owners(&self, hangout_id: &str, now: i64) -> rusqlite::Result<Vec<Person>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT p.id, p.name, p.color FROM people p
             WHERE p.id IN (
                SELECT a.owner_id FROM trips t
                JOIN houses ho ON ho.id = t.house_id
                JOIN agents a ON a.id = t.agent_id
                WHERE {GUEST_ROOM} AND t.status = 'here' AND t.ends_at > ?2
                  AND NOT EXISTS (SELECT 1 FROM room_agents r WHERE r.hangout_id = ?1 AND r.agent_id = a.id))
             OR p.id IN (
                SELECT v.person_id FROM floor_visits v
                JOIN floors f ON f.hangout_id = v.hangout_id
                WHERE v.hangout_id = ?1 AND f.access = 'public' AND v.until > ?2)
             ORDER BY p.created_at, p.id"
        ))?;
        let rows = stmt.query_map(params![hangout_id, now], person_from_row)?;
        rows.collect()
    }

    /// When this bot's visit to the room began and ends. `None` if it isn't a guest there: a bot
    /// that is placed in the room isn't one.
    pub fn bot_visit(&self, hangout_id: &str, agent_id: &str, now: i64) -> rusqlite::Result<Option<Visit>> {
        let conn = self.conn();
        let visit = |row: &Row| {
            Ok(Visit {
                since: row.get(0)?,
                until: row.get(1)?,
            })
        };
        let trip = conn
            .query_row(
                &format!(
                    "SELECT t.arrived_at, t.ends_at FROM trips t
                     JOIN houses ho ON ho.id = t.house_id
                     WHERE {GUEST_ROOM} AND t.agent_id = ?2 AND t.status = 'here' AND t.ends_at > ?3
                       AND NOT EXISTS (SELECT 1 FROM room_agents r WHERE r.hangout_id = ?1 AND r.agent_id = ?2)
                     ORDER BY t.arrived_at LIMIT 1"
                ),
                params![hangout_id, agent_id, now],
                visit,
            )
            .optional()?;
        if trip.is_some() {
            return Ok(trip);
        }
        conn.query_row(
            "SELECT v.since, v.until FROM floor_visits v
             JOIN floors f ON f.hangout_id = v.hangout_id
             WHERE v.hangout_id = ?1 AND v.agent_id = ?2 AND v.until > ?3 AND f.access = 'public'
               AND NOT EXISTS (SELECT 1 FROM room_agents r WHERE r.hangout_id = ?1 AND r.agent_id = ?2)",
            params![hangout_id, agent_id, now],
            visit,
        )
        .optional()
    }

    /// When a person's visit began and ends: as the owner of a guest, from the earliest arrival of
    /// their bots there to the last of their trips running out, and likewise for walking onto a
    /// public floor, themselves or by sending a bot. `None` if they aren't a guest there, or if
    /// they belong to the room anyway.
    pub fn person_visit(
        &self,
        hangout_id: &str,
        person_id: &str,
        now: i64,
    ) -> rusqlite::Result<Option<Visit>> {
        let conn = self.conn();
        let visit = |row: &Row| -> rusqlite::Result<(Option<i64>, Option<i64>)> {
            Ok((row.get(0)?, row.get(1)?))
        };
        let (since, until) = conn.query_row(
            &format!(
            "SELECT MIN(t.arrived_at), MAX(t.ends_at) FROM trips t
             JOIN houses ho ON ho.id = t.house_id
             JOIN agents a ON a.id = t.agent_id
             WHERE {GUEST_ROOM} AND a.owner_id = ?2 AND t.status = 'here' AND t.ends_at > ?3
               AND NOT EXISTS (SELECT 1 FROM room_agents r WHERE r.hangout_id = ?1 AND r.agent_id = a.id)
               AND NOT EXISTS (SELECT 1 FROM members m WHERE m.hangout_id = ?1 AND m.person_id = ?2)"
            ),
            params![hangout_id, person_id, now],
            visit,
        )?;
        if let Some((since, until)) = since.zip(until) {
            return Ok(Some(Visit { since, until }));
        }
        let (since, until) = conn.query_row(
            "SELECT MIN(v.since), MAX(v.until) FROM floor_visits v
             JOIN floors f ON f.hangout_id = v.hangout_id
             WHERE v.hangout_id = ?1 AND v.person_id = ?2 AND v.until > ?3 AND f.access = 'public'
               AND NOT EXISTS (SELECT 1 FROM members m WHERE m.hangout_id = ?1 AND m.person_id = ?2)",
            params![hangout_id, person_id, now],
            visit,
        )?;
        Ok(since.zip(until).map(|(since, until)| Visit { since, until }))
    }

    /// Chat lines written after `since`: `messages` for a guest. Times are whole seconds, so the
    /// second they arrived is left out rather than risk showing a line from before they came.
    pub fn messages_since(
        &self,
        hangout_id: &str,
        since: i64,
        after: i64,
        limit: i64,
    ) -> rusqlite::Result<Vec<Message>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_COLS} FROM messages
             WHERE hangout_id = ? AND created_at > ? AND id > ? AND kind != 'note'
             ORDER BY id DESC LIMIT ?"
        ))?;
        let rows = stmt.query_map(params![hangout_id, since, after, limit], message_from_row)?;
        let mut messages = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        messages.reverse();
        Ok(messages)
    }

    /// Notes shared after `since`: `notes` for a guest, cut off the same way as `messages_since`.
    pub fn notes_since(&self, hangout_id: &str, since: i64, limit: i64) -> rusqlite::Result<Vec<Message>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {MESSAGE_COLS} FROM messages
             WHERE hangout_id = ? AND kind = 'note' AND created_at > ? ORDER BY id DESC LIMIT ?"
        ))?;
        let rows = stmt.query_map(params![hangout_id, since, limit], message_from_row)?;
        rows.collect()
    }

    // Friends and invites

    /// F1, F2: invited by a friend, with no city of their own yet. Worked out from what happened.
    pub fn invited_with_no_city(&self, person_id: &str) -> rusqlite::Result<bool> {
        self.conn().query_row(
            "SELECT NOT EXISTS (SELECT 1 FROM cities WHERE created_by = ?1)
               AND (EXISTS (SELECT 1 FROM plan_invitees WHERE person_id = ?1)
                 OR EXISTS (SELECT 1 FROM members m JOIN hangouts h ON h.id = m.hangout_id
                            WHERE m.person_id = ?1 AND h.created_by != ?1)
                 OR EXISTS (SELECT 1 FROM city_members cm JOIN cities c ON c.id = cm.city_id
                            WHERE cm.person_id = ?1 AND c.created_by != ?1))",
            [person_id],
            |row| row.get(0),
        )
    }

    /// The plan's link, which is `code` if it doesn't have one yet.
    pub fn plan_invite_code(
        &self,
        event_id: &str,
        code: &str,
        person_id: &str,
        now: i64,
    ) -> rusqlite::Result<String> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO plan_invites (event_id, code, created_by, created_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (event_id) DO NOTHING",
            params![event_id, code, person_id, now],
        )?;
        conn.query_row("SELECT code FROM plan_invites WHERE event_id = ?", [event_id], |row| {
            row.get(0)
        })
    }

    pub fn replace_plan_invite(
        &self,
        event_id: &str,
        code: &str,
        person_id: &str,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO plan_invites (event_id, code, created_by, created_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (event_id) DO UPDATE SET
                code = excluded.code, created_by = excluded.created_by,
                created_at = excluded.created_at",
            params![event_id, code, person_id, now],
        )?;
        Ok(())
    }

    /// The plan a link opens, and who sent it.
    pub fn plan_invite(&self, code: &str) -> rusqlite::Result<Option<(String, String)>> {
        self.conn()
            .query_row(
                "SELECT event_id, created_by FROM plan_invites WHERE code = ?",
                [code],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    /// Whether this person would be turned away as one too many (F18).
    pub fn plan_is_full(&self, event_id: &str, person_id: &str, max: i64) -> rusqlite::Result<bool> {
        self.conn().query_row(
            "SELECT (SELECT COUNT(*) FROM plan_invitees WHERE event_id = ?1) >= ?3
               AND NOT EXISTS (SELECT 1 FROM plan_invitees WHERE event_id = ?1 AND person_id = ?2)",
            params![event_id, person_id, max],
            |row| row.get(0),
        )
    }

    pub fn add_plan_invitee(
        &self,
        event_id: &str,
        person_id: &str,
        max: i64,
        now: i64,
    ) -> rusqlite::Result<Invited> {
        let conn = self.conn();
        let invitees: Vec<String> = conn
            .prepare("SELECT person_id FROM plan_invitees WHERE event_id = ?")?
            .query_map([event_id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if invitees.iter().any(|invitee| invitee == person_id) {
            return Ok(Invited::Already);
        }
        if invitees.len() as i64 >= max {
            return Ok(Invited::Full);
        }
        conn.execute(
            "INSERT INTO plan_invitees (event_id, person_id, joined_at) VALUES (?, ?, ?)",
            params![event_id, person_id, now],
        )?;
        Ok(Invited::New)
    }

    /// The invitees of a room's plans that are still coming up, first to answer first.
    pub fn invitees_in(&self, hangout_id: &str, now: i64) -> rusqlite::Result<Vec<Invitee>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT i.event_id, p.id, p.name, p.color FROM plan_invitees i
             JOIN events e ON e.id = i.event_id
             JOIN people p ON p.id = i.person_id
             WHERE e.hangout_id = ? AND e.ends_at >= ? AND e.status != 'cancelled'
             ORDER BY i.joined_at, p.id",
        )?;
        let rows = stmt.query_map(params![hangout_id, now], |row| {
            Ok(Invitee {
                event_id: row.get(0)?,
                person_id: row.get(1)?,
                name: row.get(2)?,
                color: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    // Closeness

    /// Who can see when this person is free: `house`, `city`, or nobody.
    pub fn free_sharing(&self, person_id: &str) -> rusqlite::Result<Option<String>> {
        self.conn()
            .query_row("SELECT level FROM free_sharing WHERE person_id = ?", [person_id], |row| {
                row.get(0)
            })
            .optional()
    }

    /// F26, F28: no level turns sharing off and deletes what was posted.
    pub fn set_free_sharing(
        &self,
        person_id: &str,
        level: Option<&str>,
        now: i64,
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        match level {
            Some(level) => {
                tx.execute(
                    "INSERT INTO free_sharing (person_id, level, updated_at) VALUES (?, ?, ?)
                     ON CONFLICT (person_id) DO UPDATE SET
                        level = excluded.level, updated_at = excluded.updated_at",
                    params![person_id, level, now],
                )?;
            }
            None => {
                tx.execute("DELETE FROM free_sharing WHERE person_id = ?", [person_id])?;
                tx.execute("DELETE FROM free_windows WHERE person_id = ?", [person_id])?;
            }
        }
        tx.commit()
    }

    pub fn replace_free_windows(
        &self,
        person_id: &str,
        windows: &[(i64, i64)],
        now: i64,
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM free_windows WHERE person_id = ?", [person_id])?;
        for (start, end) in windows {
            tx.execute(
                "INSERT INTO free_windows (person_id, starts_at, ends_at, set_at) VALUES (?, ?, ?, ?)",
                params![person_id, start, end, now],
            )?;
        }
        tx.commit()
    }

    /// Windows still ahead that were posted since `posted_since`, in order.
    pub fn free_windows(
        &self,
        person_id: &str,
        now: i64,
        posted_since: i64,
    ) -> rusqlite::Result<Vec<(i64, i64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT starts_at, ends_at FROM free_windows
             WHERE person_id = ? AND ends_at > ? AND set_at >= ? ORDER BY starts_at",
        )?;
        let rows = stmt.query_map(params![person_id, now, posted_since], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        rows.collect()
    }

    /// F32: when this person is taken: plans on the calendar that they said yes to.
    pub fn busy_spans(&self, person_id: &str, now: i64) -> rusqlite::Result<Vec<(i64, i64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT e.starts_at, e.ends_at FROM events e
             JOIN rsvps r ON r.event_id = e.id AND r.person_id = ?
             WHERE r.response = 'yes' AND e.status = 'confirmed' AND e.ends_at > ?",
        )?;
        let rows = stmt.query_map(params![person_id, now], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    pub fn share_a_house(&self, one: &str, other: &str) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            "SELECT 1 FROM residents a JOIN residents b ON a.house_id = b.house_id
             WHERE a.person_id = ? AND b.person_id = ?",
            params![one, other],
        )
    }

    pub fn plan_items(&self, event_id: &str) -> rusqlite::Result<Vec<PlanItem>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PLAN_ITEM_COLS} {PLAN_ITEM_FROM} WHERE i.event_id = ? ORDER BY i.created_at, i.rowid"
        ))?;
        let rows = stmt.query_map([event_id], plan_item_from_row)?;
        rows.collect()
    }

    pub fn plan_item(&self, id: &str) -> rusqlite::Result<Option<PlanItem>> {
        self.conn()
            .query_row(
                &format!("SELECT {PLAN_ITEM_COLS} {PLAN_ITEM_FROM} WHERE i.id = ?"),
                [id],
                plan_item_from_row,
            )
            .optional()
    }

    /// False if the plan's list is full.
    pub fn add_plan_item(&self, item: &NewPlanItem, max: i64) -> rusqlite::Result<bool> {
        let conn = self.conn();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM plan_items WHERE event_id = ?",
            [item.event_id],
            |row| row.get(0),
        )?;
        if count >= max {
            return Ok(false);
        }
        conn.execute(
            "INSERT INTO plan_items (id, event_id, body, added_by, adder, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![item.id, item.event_id, item.text, item.added_by, item.adder, item.now],
        )?;
        Ok(true)
    }

    /// F38, F40: false if someone who's still going already has it.
    pub fn claim_plan_item(
        &self,
        id: &str,
        person_id: &str,
        via: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE plan_items SET claimed_by = ?2, claimed_via = ?3, done = 0
             WHERE id = ?1 AND (claimed_by IS NULL OR NOT EXISTS (
                SELECT 1 FROM rsvps r
                JOIN events e ON e.id = r.event_id
                JOIN members m ON m.hangout_id = e.hangout_id AND m.person_id = r.person_id
                WHERE r.event_id = plan_items.event_id AND r.person_id = plan_items.claimed_by
                  AND r.response IN ('yes', 'maybe')))",
            params![id, person_id, via],
        )?;
        Ok(changed == 1)
    }

    pub fn release_plan_item(&self, id: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "UPDATE plan_items SET claimed_by = NULL, claimed_via = NULL, done = 0 WHERE id = ?",
            [id],
        )?;
        Ok(())
    }

    /// F39: what a bot claimed is its owner's own from now on.
    pub fn keep_plan_item(&self, id: &str) -> rusqlite::Result<()> {
        self.conn().execute("UPDATE plan_items SET claimed_via = NULL WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn finish_plan_item(&self, id: &str) -> rusqlite::Result<()> {
        self.conn().execute("UPDATE plan_items SET done = 1 WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn remove_plan_item(&self, id: &str) -> rusqlite::Result<()> {
        self.conn().execute("DELETE FROM plan_items WHERE id = ?", [id])?;
        Ok(())
    }

    // A house's notes and calendar

    pub fn house_notes(&self, house_id: &str) -> rusqlite::Result<Vec<HouseNote>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT n.topic, n.body, p.name, n.via_agent, n.updated_at FROM house_notes n
             JOIN people p ON p.id = n.written_by
             WHERE n.house_id = ? ORDER BY n.topic",
        )?;
        let rows = stmt.query_map([house_id], |row| {
            Ok(HouseNote {
                topic: row.get(0)?,
                text: row.get(1)?,
                writer: row.get(2)?,
                via_agent: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Writes under a topic, replacing what's there. The lock is held throughout, so two writers
    /// can't both pass these checks.
    pub fn set_house_note(&self, note: &NewHouseNote, max: i64) -> rusqlite::Result<NoteWrite> {
        let conn = self.conn();
        let existing: Option<Option<String>> = conn
            .query_row(
                "SELECT via_agent FROM house_notes WHERE house_id = ? AND topic = ?",
                params![note.house_id, note.topic],
                |row| row.get(0),
            )
            .optional()?;
        match existing {
            Some(None) if note.via_agent.is_some() => return Ok(NoteWrite::PersonsWord),
            Some(_) => {}
            None => {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM house_notes WHERE house_id = ?",
                    [note.house_id],
                    |row| row.get(0),
                )?;
                if count >= max {
                    return Ok(NoteWrite::Full);
                }
            }
        }
        conn.execute(
            "INSERT INTO house_notes (id, house_id, topic, body, written_by, via_agent, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (house_id, topic) DO UPDATE SET
                topic = excluded.topic, body = excluded.body, written_by = excluded.written_by,
                via_agent = excluded.via_agent, updated_at = excluded.updated_at",
            params![
                note.id,
                note.house_id,
                note.topic,
                note.text,
                note.written_by,
                note.via_agent,
                note.now
            ],
        )?;
        Ok(NoteWrite::Saved)
    }

    /// Whether a bot wrote this note, if there is one.
    pub fn house_note_by_bot(&self, house_id: &str, topic: &str) -> rusqlite::Result<Option<bool>> {
        self.conn()
            .query_row(
                "SELECT via_agent IS NOT NULL FROM house_notes WHERE house_id = ? AND topic = ?",
                params![house_id, topic],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn remove_house_note(&self, house_id: &str, topic: &str) -> rusqlite::Result<()> {
        self.conn().execute(
            "DELETE FROM house_notes WHERE house_id = ? AND topic = ?",
            params![house_id, topic],
        )?;
        Ok(())
    }

    /// F51: whether this person's plans show on their house's calendar.
    pub fn shows_plans_to_house(&self, person_id: &str) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            "SELECT 1 FROM house_plan_sharing WHERE person_id = ?",
            [person_id],
        )
    }

    pub fn set_shows_plans_to_house(
        &self,
        person_id: &str,
        on: bool,
        now: i64,
    ) -> rusqlite::Result<()> {
        if on {
            self.conn().execute(
                "INSERT INTO house_plan_sharing (person_id, updated_at) VALUES (?, ?)
                 ON CONFLICT (person_id) DO UPDATE SET updated_at = excluded.updated_at",
                params![person_id, now],
            )?;
        } else {
            self.conn()
                .execute("DELETE FROM house_plan_sharing WHERE person_id = ?", [person_id])?;
        }
        Ok(())
    }

    /// F52 to F54: the house's calendar between two times, soonest first.
    pub fn house_calendar(
        &self,
        house_id: &str,
        from: i64,
        to: i64,
    ) -> rusqlite::Result<Vec<HousePlan>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT e.id, e.title, e.location, e.starts_at, e.ends_at, e.status, p.name
             FROM houses h
             JOIN residents res ON res.house_id = h.id
             JOIN people p ON p.id = res.person_id
             JOIN rsvps r ON r.person_id = res.person_id AND r.response IN ('yes', 'maybe')
             JOIN events e ON e.id = r.event_id
             WHERE h.id = ?1 AND e.status != 'cancelled' AND e.ends_at >= ?2 AND e.starts_at <= ?3
               AND (e.hangout_id = h.living_room
                    OR EXISTS (SELECT 1 FROM house_plan_sharing s WHERE s.person_id = res.person_id))
             ORDER BY e.starts_at, e.id, p.name, p.id",
        )?;
        let mut rows = stmt.query(params![house_id, from, to])?;
        let mut plans: Vec<(String, HousePlan)> = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let name: String = row.get(6)?;
            match plans.last_mut() {
                Some((last, plan)) if *last == id => plan.going.push(name),
                _ => {
                    let status: String = row.get(5)?;
                    plans.push((
                        id,
                        HousePlan {
                            title: row.get(1)?,
                            location: row.get(2)?,
                            starts_at: row.get(3)?,
                            ends_at: row.get(4)?,
                            on_calendar: status == "confirmed",
                            going: vec![name],
                        },
                    ));
                }
            }
        }
        Ok(plans.into_iter().map(|(_, plan)| plan).collect())
    }

    // Growth. Counted from what happened, on read. Nothing is stored.

    /// Confirmed Town Square plans that have finished, with a yes from at least two people. A
    /// plan one person confirmed alone isn't friends getting together.
    pub fn gatherings(&self, city_id: &str, now: i64) -> rusqlite::Result<i64> {
        self.conn().query_row(
            &format!("SELECT COUNT(*) {GATHERINGS}"),
            params![city_id, now],
            |row| row.get(0),
        )
    }

    /// A road for each pair of houses in the city that a bot from one has really arrived at the
    /// other, with how many times. `a` sorts before `b`, and the roads come in that order.
    pub fn roads(&self, city_id: &str) -> rusqlite::Result<Vec<Road>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT MIN(t.house_id, bh.house_id) AS road_a, MAX(t.house_id, bh.house_id) AS road_b,
                    COUNT(*)
             FROM trips t
             JOIN bot_homes bh ON bh.agent_id = t.agent_id AND bh.house_id != t.house_id
             JOIN houses th ON th.id = t.house_id
             JOIN houses hh ON hh.id = bh.house_id
             WHERE t.arrived_at IS NOT NULL AND th.city_id = ?1 AND hh.city_id = ?1
             GROUP BY road_a, road_b ORDER BY road_a, road_b",
        )?;
        let rows = stmt.query_map([city_id], |row| {
            Ok(Road {
                a: row.get(0)?,
                b: row.get(1)?,
                trips: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    // Trust. Standing is counted on read, so all that is stored is what people chose: the rules,
    // the vouches, who is waiting at a gate, and which trips a resident ended.

    /// What a city or a house asks for. A place that never set anything asks for nothing.
    pub fn entry_rules(&self, place_id: &str) -> rusqlite::Result<EntryRules> {
        Ok(self
            .conn()
            .query_row(
                "SELECT min_member_days, min_gatherings, min_good_trips, min_vouches, rules_text
                 FROM entry_rules WHERE place_id = ?",
                [place_id],
                |row| entry_rules_at(row, 0),
            )
            .optional()?
            .unwrap_or_default())
    }

    /// What each house in a city asks for, as (house id, rules). Houses that never set anything
    /// are left out.
    pub fn house_rules_in_city(&self, city_id: &str) -> rusqlite::Result<Vec<(String, EntryRules)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT e.place_id, e.min_member_days, e.min_gatherings, e.min_good_trips,
                    e.min_vouches, e.rules_text
             FROM entry_rules e JOIN houses h ON h.id = e.place_id WHERE h.city_id = ?",
        )?;
        let rows = stmt.query_map([city_id], |row| Ok((row.get(0)?, entry_rules_at(row, 1)?)))?;
        rows.collect()
    }

    pub fn set_entry_rules(
        &self,
        place_id: &str,
        rules: &EntryRules,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO entry_rules (place_id, min_member_days, min_gatherings, min_good_trips,
                                      min_vouches, rules_text, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (place_id) DO UPDATE SET
                min_member_days = excluded.min_member_days,
                min_gatherings = excluded.min_gatherings,
                min_good_trips = excluded.min_good_trips,
                min_vouches = excluded.min_vouches,
                rules_text = excluded.rules_text,
                updated_at = excluded.updated_at",
            params![
                place_id,
                rules.min_member_days,
                rules.min_gatherings,
                rules.min_good_trips,
                rules.min_vouches,
                rules.rules_text,
                now
            ],
        )?;
        Ok(())
    }

    /// Records a vouch. False if this person had already vouched, which changes nothing.
    pub fn add_vouch(
        &self,
        city_id: &str,
        for_person: &str,
        by_person: &str,
        now: i64,
    ) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "INSERT OR IGNORE INTO vouches (city_id, for_person, by_person, created_at)
             VALUES (?, ?, ?, ?)",
            params![city_id, for_person, by_person, now],
        )?;
        Ok(changed > 0)
    }

    /// Takes back the caller's own vouch. False if there wasn't one.
    pub fn remove_vouch(
        &self,
        city_id: &str,
        for_person: &str,
        by_person: &str,
    ) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "DELETE FROM vouches WHERE city_id = ? AND for_person = ? AND by_person = ?",
            params![city_id, for_person, by_person],
        )?;
        Ok(changed > 0)
    }

    /// How many people in the city vouch for this person. Those who left don't count.
    pub fn vouch_count(&self, city_id: &str, for_person: &str) -> rusqlite::Result<i64> {
        self.conn().query_row(
            &format!("SELECT COUNT(*) {VOUCHERS_IN_CITY} WHERE v.city_id = ? AND v.for_person = ?"),
            params![city_id, for_person],
            |row| row.get(0),
        )
    }

    /// What this person has done in the city: how long they've been in it, the gatherings they
    /// said yes to, their bots' trips here that went well, the ones a resident ended, and who
    /// vouches for them. All of it is counted now, from what's in the database.
    pub fn standing_counts(
        &self,
        city_id: &str,
        person_id: &str,
        now: i64,
    ) -> rusqlite::Result<StandingCounts> {
        let conn = self.conn();
        let member_since = conn
            .query_row(
                "SELECT joined_at FROM city_members WHERE city_id = ? AND person_id = ?",
                params![city_id, person_id],
                |row| row.get(0),
            )
            .optional()?;
        let gatherings = conn.query_row(
            &format!(
                "SELECT COUNT(*) {GATHERINGS}
                   AND EXISTS (SELECT 1 FROM rsvps y WHERE y.event_id = e.id
                               AND y.person_id = ?3 AND y.response = 'yes')"
            ),
            params![city_id, now, person_id],
            |row| row.get(0),
        )?;
        // A trip counts once it arrived and is over, unless a resident ended it by sending the bot
        // home. One still under way hasn't gone well or badly yet. A house counts once a day, or a
        // bot with a pass could loop short trips to meet another house's requirements.
        let good_trips = conn.query_row(
            "SELECT COUNT(DISTINCT t.house_id || '/' || (t.arrived_at / 86400)) FROM trips t
             JOIN agents a ON a.id = t.agent_id
             JOIN houses h ON h.id = t.house_id
             WHERE a.owner_id = ?1 AND h.city_id = ?2 AND t.arrived_at IS NOT NULL
               AND (t.status = 'ended' OR (t.status = 'here' AND t.ends_at <= ?3))
               AND NOT EXISTS (SELECT 1 FROM sent_home s WHERE s.trip_id = t.id)",
            params![person_id, city_id, now],
            |row| row.get(0),
        )?;
        let sent_home = conn.query_row(
            "SELECT COUNT(*) FROM sent_home s
             JOIN trips t ON t.id = s.trip_id
             JOIN agents a ON a.id = t.agent_id
             JOIN houses h ON h.id = t.house_id
             WHERE a.owner_id = ?1 AND h.city_id = ?2",
            params![person_id, city_id],
            |row| row.get(0),
        )?;
        let mut stmt = conn.prepare(&format!(
            "SELECT p.name {VOUCHERS_IN_CITY} JOIN people p ON p.id = v.by_person
             WHERE v.city_id = ? AND v.for_person = ? ORDER BY v.created_at, v.rowid"
        ))?;
        let vouchers = stmt
            .query_map(params![city_id, person_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(StandingCounts { member_since, gatherings, good_trips, sent_home, vouchers })
    }

    /// Notes that a resident ended this trip by sending the bot home.
    pub fn record_sent_home(&self, trip_id: &str, now: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO sent_home (trip_id, created_at) VALUES (?, ?)",
            params![trip_id, now],
        )?;
        Ok(())
    }

    pub fn was_sent_home(&self, trip_id: &str) -> rusqlite::Result<bool> {
        exists(&self.conn(), "SELECT 1 FROM sent_home WHERE trip_id = ?", [trip_id])
    }

    // The gate. A place made before `cutoff` has lapsed: nothing deletes it, every read just
    // leaves it out.

    /// Puts someone at a city's gate. Asking again while a place is still good changes nothing,
    /// and a lapsed place is replaced. The gate holds `capacity` places that haven't lapsed.
    pub fn gate_ask(
        &self,
        city_id: &str,
        person_id: &str,
        now: i64,
        cutoff: i64,
        capacity: i64,
    ) -> rusqlite::Result<GateAsk> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if exists(
            &tx,
            "SELECT 1 FROM city_gate WHERE city_id = ? AND person_id = ? AND created_at > ?",
            params![city_id, person_id, cutoff],
        )? {
            return Ok(GateAsk::AlreadyThere);
        }
        let held: i64 = tx.query_row(
            "SELECT COUNT(*) FROM city_gate WHERE city_id = ? AND created_at > ?",
            params![city_id, cutoff],
            |row| row.get(0),
        )?;
        if held >= capacity {
            return Ok(GateAsk::Full);
        }
        tx.execute(
            "INSERT OR REPLACE INTO city_gate (city_id, person_id, created_at) VALUES (?, ?, ?)",
            params![city_id, person_id, now],
        )?;
        tx.commit()?;
        Ok(GateAsk::Added)
    }

    /// When this person asked, if their place at the gate is still good.
    pub fn gate_asked_at(
        &self,
        city_id: &str,
        person_id: &str,
        cutoff: i64,
    ) -> rusqlite::Result<Option<i64>> {
        self.conn()
            .query_row(
                "SELECT created_at FROM city_gate WHERE city_id = ? AND person_id = ? AND created_at > ?",
                params![city_id, person_id, cutoff],
                |row| row.get(0),
            )
            .optional()
    }

    /// Turns someone away. Their vouches go too, so coming back with the link starts them from
    /// nothing instead of walking straight in. False if they weren't waiting.
    pub fn gate_remove(&self, city_id: &str, person_id: &str, cutoff: i64) -> rusqlite::Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "DELETE FROM city_gate WHERE city_id = ? AND person_id = ? AND created_at > ?",
            params![city_id, person_id, cutoff],
        )?;
        if changed > 0 {
            tx.execute(
                "DELETE FROM vouches WHERE city_id = ? AND for_person = ?",
                params![city_id, person_id],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO friend_invite_ends (invite_id)
                 SELECT id FROM friend_invites
                 WHERE kind = 'city' AND place_id = ? AND to_person = ? AND declined_at IS NULL",
                params![city_id, person_id],
            )?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    /// Turns everyone waiting away at once, their vouches with them, as `gate_remove` does for one.
    /// Returns how many.
    pub fn gate_clear(&self, city_id: &str, cutoff: i64) -> rusqlite::Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM vouches WHERE city_id = ?1 AND for_person IN
               (SELECT person_id FROM city_gate WHERE city_id = ?1 AND created_at > ?2)",
            params![city_id, cutoff],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO friend_invite_ends (invite_id)
             SELECT f.id FROM friend_invites f
             JOIN city_gate g ON g.city_id = f.place_id AND g.person_id = f.to_person
             WHERE f.kind = 'city' AND f.place_id = ?1 AND g.created_at > ?2 AND f.declined_at IS NULL",
            params![city_id, cutoff],
        )?;
        let cleared = tx.execute(
            "DELETE FROM city_gate WHERE city_id = ? AND created_at > ?",
            params![city_id, cutoff],
        )?;
        tx.commit()?;
        Ok(cleared)
    }

    /// Everyone waiting at the gate, earliest first.
    pub fn gate_ids(&self, city_id: &str, cutoff: i64) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT person_id FROM city_gate WHERE city_id = ? AND created_at > ?
             ORDER BY created_at, rowid",
        )?;
        let rows = stmt.query_map(params![city_id, cutoff], |row| row.get(0))?;
        rows.collect()
    }

    /// Everyone waiting at the gate with their vouches, earliest first. `viewer_id` is the member
    /// looking, so each row can say whether they've vouched.
    pub fn gate_rows(
        &self,
        city_id: &str,
        viewer_id: &str,
        cutoff: i64,
    ) -> rusqlite::Result<Vec<GateRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT p.id, p.name, p.color, g.created_at,
                    (SELECT COUNT(*) {VOUCHERS_IN_CITY}
                      WHERE v.city_id = g.city_id AND v.for_person = g.person_id),
                    EXISTS (SELECT 1 FROM vouches y WHERE y.city_id = g.city_id
                            AND y.for_person = g.person_id AND y.by_person = ?3)
             FROM city_gate g JOIN people p ON p.id = g.person_id
             WHERE g.city_id = ?1 AND g.created_at > ?2
             ORDER BY g.created_at, g.rowid"
        ))?;
        let rows = stmt.query_map(params![city_id, cutoff, viewer_id], |row| {
            Ok(GateRow {
                person: person_from_row(row)?,
                asked_at: row.get(3)?,
                vouches: row.get(4)?,
                you_vouched: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// The gates this person is waiting at, with their vouches and what each city asks for.
    pub fn gates_of_person(&self, person_id: &str, cutoff: i64) -> rusqlite::Result<Vec<GatePlace>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT c.id, c.name,
                    (SELECT COUNT(*) {VOUCHERS_IN_CITY}
                      WHERE v.city_id = g.city_id AND v.for_person = g.person_id),
                    COALESCE((SELECT min_vouches FROM entry_rules WHERE place_id = c.id), 0)
             FROM city_gate g JOIN cities c ON c.id = g.city_id
             WHERE g.person_id = ?1 AND g.created_at > ?2
             ORDER BY g.created_at, g.rowid"
        ))?;
        let rows = stmt.query_map(params![person_id, cutoff], |row| {
            Ok(GatePlace {
                city_id: row.get(0)?,
                city_name: row.get(1)?,
                vouches: row.get(2)?,
                vouches_needed: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// Lets someone in from the gate if they're waiting there and enough people in the city now
    /// vouch for them. Checking and moving them happen together, so of two vouches that arrive at
    /// once only one does it. True only for the call that did.
    pub fn let_in_if_enough(
        &self,
        city_id: &str,
        person_id: &str,
        now: i64,
        cutoff: i64,
    ) -> rusqlite::Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if !exists(
            &tx,
            "SELECT 1 FROM city_gate WHERE city_id = ? AND person_id = ? AND created_at > ?",
            params![city_id, person_id, cutoff],
        )? {
            return Ok(false);
        }
        let needed: i64 = tx.query_row(
            "SELECT COALESCE((SELECT min_vouches FROM entry_rules WHERE place_id = ?), 0)",
            [city_id],
            |row| row.get(0),
        )?;
        let have: i64 = tx.query_row(
            &format!("SELECT COUNT(*) {VOUCHERS_IN_CITY} WHERE v.city_id = ? AND v.for_person = ?"),
            params![city_id, person_id],
            |row| row.get(0),
        )?;
        if have < needed {
            return Ok(false);
        }
        join_city(&tx, city_id, person_id, now)?;
        tx.commit()?;
        Ok(true)
    }
}

/// The rules in a row, with the four numbers and the text starting at column `first`.
fn entry_rules_at(row: &Row, first: usize) -> rusqlite::Result<EntryRules> {
    Ok(EntryRules {
        min_member_days: row.get(first)?,
        min_gatherings: row.get(first + 1)?,
        min_good_trips: row.get(first + 2)?,
        min_vouches: row.get(first + 3)?,
        rules_text: row.get(first + 4)?,
    })
}

fn owner_note_from_row(row: &Row) -> rusqlite::Result<OwnerNote> {
    Ok(OwnerNote {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        kind: row.get(2)?,
        text: row.get(3)?,
        about_person_id: row.get(4)?,
        about: row.get(5)?,
        from_question_id: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn person_from_row(row: &Row) -> rusqlite::Result<Person> {
    Ok(Person {
        id: row.get(0)?,
        name: row.get(1)?,
        color: row.get(2)?,
    })
}

fn hangout_from_row(row: &Row) -> rusqlite::Result<Hangout> {
    Ok(Hangout {
        id: row.get(0)?,
        name: row.get(1)?,
        invite_code: row.get(2)?,
        created_by: row.get(3)?,
    })
}

fn rsvp_from_row(row: &Row) -> rusqlite::Result<Rsvp> {
    Ok(Rsvp {
        person_id: row.get(0)?,
        response: row.get(1)?,
        via_agent: row.get(2)?,
        note: row.get(3)?,
    })
}

fn load_rsvps(conn: &Connection, event_id: &str) -> rusqlite::Result<Vec<Rsvp>> {
    let mut stmt = conn.prepare(
        "SELECT person_id, response, via_agent, note FROM rsvps WHERE event_id = ? ORDER BY updated_at",
    )?;
    let rows = stmt.query_map([event_id], rsvp_from_row)?;
    rows.collect()
}

// Worlds (docs/public-city.md). Storage only: the rules are in `worlds`.

/// A world as stored. Its land isn't: that is generated from `seed` and `size`.
#[derive(Clone, Debug)]
pub struct WorldRow {
    pub id: String,
    pub name: String,
    /// `public` for The Commons, `own` for a world a person started.
    pub kind: String,
    pub seed: u64,
    pub size: usize,
    pub owner_id: Option<String>,
    pub open: bool,
    pub invite_code: Option<String>,
    pub step: u64,
    pub active_at: i64,
    pub created_at: i64,
}

pub struct NewWorld<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub kind: &'a str,
    pub seed: u64,
    pub size: usize,
    pub owner_id: Option<&'a str>,
    pub open: bool,
    pub invite_code: Option<&'a str>,
    /// The highway: ordinary roads with no builder.
    pub highway: &'a [Pos],
    pub now: i64,
}

/// How `Db::insert_world` came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldAdded {
    Added,
    /// A world with this id is already there, which is how The Commons is made once.
    Exists,
    OwnerHasOne,
    /// The server already holds as many worlds of people's own as it will.
    Full,
}

/// A player as stored. What kind it is follows from which columns are filled.
#[derive(Clone, Debug)]
pub struct PlayerRow {
    pub id: i64,
    pub name_seed: i64,
    pub person_id: Option<String>,
    pub agent_id: Option<String>,
    pub has_token: bool,
    pub removed: bool,
    pub moves_day: i64,
    pub moves_used: i64,
    pub seen_at: i64,
    pub created_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerKind {
    /// Joined with no account and nobody has claimed it.
    Guest,
    /// A guest that a signed-in person claimed.
    Claimed,
    /// Linked to a bot by that bot's own token.
    Bot,
    /// A signed-in person's own player, which has no token of its own.
    Person,
}

impl PlayerRow {
    pub fn kind(&self) -> PlayerKind {
        // A bot's player has no token between its bot's token being replaced and the next join.
        if self.agent_id.is_some() {
            PlayerKind::Bot
        } else if !self.has_token {
            PlayerKind::Person
        } else if self.person_id.is_some() {
            PlayerKind::Claimed
        } else {
            PlayerKind::Guest
        }
    }
}

/// An invite from a friend, with what the person invited sees (FR5).
#[derive(Clone, Debug)]
pub struct FriendInvite {
    pub id: String,
    pub kind: String,
    pub from_name: String,
    pub place_name: String,
    pub code: String,
}

/// A player standing in a world.
#[derive(Clone, Debug)]
pub struct WorldPlayer {
    pub id: i64,
    pub name_seed: i64,
    pub pos: Pos,
    pub emote: Option<String>,
    /// A signed-in person's own player, and not an agent's (W53).
    pub is_person: bool,
}

/// Everything a world view is built from, read at one moment.
pub struct Snapshot {
    pub world: WorldRow,
    pub tiles: Tiles,
    pub players: Vec<WorldPlayer>,
    /// How many players have moved here recently, which may be more than `players` lists.
    pub active: i64,
}

/// W64: how far the end of the old city has got. A world has a row from its clock starting, and a
/// world with none has nothing coming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldEnd {
    pub clock_step: u64,
    pub omen_step: Option<u64>,
    pub ended_step: Option<u64>,
}

/// How `Db::claim_player` came out.
#[derive(Debug)]
pub enum Claimed {
    Done(PlayerRow),
    NoSuchCode,
    TooMany,
}

/// How `Db::join_as_bot` came out.
#[derive(Debug)]
pub enum BotJoined {
    Done(PlayerRow),
    /// The keeper ended this bot's player for good.
    Removed,
    /// The owner of the world it asked for barred this bot's player from it.
    Barred,
}

/// A player's name comes from a seed of its own that starts random, so a name that was rerolled
/// can't be told from one that never was.
pub fn random_name_seed() -> i64 {
    rand::random_range(1..(1i64 << 40))
}

const WORLD_COLS: &str =
    "id, name, kind, seed, size, owner_id, open, invite_code, step, active_at, created_at";
const PLAYER_COLS: &str = "id, name_seed, person_id, agent_id, token_hash IS NOT NULL, removed, \
     moves_day, moves_used, seen_at, created_at";

fn world_from_row(row: &Row) -> rusqlite::Result<WorldRow> {
    Ok(WorldRow {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        seed: row.get::<_, i64>(3)? as u64,
        size: row.get::<_, i64>(4)? as usize,
        owner_id: row.get(5)?,
        open: row.get(6)?,
        invite_code: row.get(7)?,
        step: row.get::<_, i64>(8)? as u64,
        active_at: row.get(9)?,
        created_at: row.get(10)?,
    })
}

fn player_row_from_row(row: &Row) -> rusqlite::Result<PlayerRow> {
    Ok(PlayerRow {
        id: row.get(0)?,
        name_seed: row.get(1)?,
        person_id: row.get(2)?,
        agent_id: row.get(3)?,
        has_token: row.get(4)?,
        removed: row.get(5)?,
        moves_day: row.get(6)?,
        moves_used: row.get(7)?,
        seen_at: row.get(8)?,
        created_at: row.get(9)?,
    })
}

fn q_world(conn: &Connection, id: &str) -> rusqlite::Result<Option<WorldRow>> {
    conn.query_row(
        &format!("SELECT {WORLD_COLS} FROM worlds WHERE id = ?"),
        [id],
        world_from_row,
    )
    .optional()
}

fn q_player(conn: &Connection, id: i64) -> rusqlite::Result<Option<PlayerRow>> {
    conn.query_row(
        &format!("SELECT {PLAYER_COLS} FROM players WHERE id = ?"),
        [id],
        player_row_from_row,
    )
    .optional()
}

fn tile_from_row(row: &Row) -> rusqlite::Result<Option<(Pos, Tile)>> {
    let (x, y): (i64, i64) = (row.get(0)?, row.get(1)?);
    let kind: String = row.get(2)?;
    let level: i64 = row.get(3)?;
    let player: Option<i64> = row.get(4)?;
    let (Ok(x), Ok(y), Some(kind)) = (u16::try_from(x), u16::try_from(y), Kind::parse(&kind)) else {
        return Ok(None);
    };
    Ok(Some((
        (x, y),
        Tile {
            kind,
            level: level.clamp(0, i64::from(crate::world::MAX_LEVEL)) as u8,
            player: player.map(|player| player as u64),
        },
    )))
}

/// The tiles of a world, or only those inside a box (both corners included). The box is for
/// planning one move: the planners look at the tiles in a move and the ones beside it, no further.
fn q_tiles(conn: &Connection, world_id: &str, region: Option<[i64; 4]>) -> rusqlite::Result<Tiles> {
    let mut tiles = Tiles::new();
    match region {
        None => {
            let mut stmt = conn.prepare_cached(
                "SELECT x, y, kind, level, player_id FROM world_tiles WHERE world_id = ?1
                 UNION ALL SELECT x, y, kind, 0, player_id FROM world_buildings WHERE world_id = ?1",
            )?;
            let mut rows = stmt.query([world_id])?;
            while let Some(row) = rows.next()? {
                if let Some((pos, tile)) = tile_from_row(row)? {
                    tiles.insert(pos, tile);
                }
            }
        }
        Some([x0, y0, x1, y1]) => {
            let mut stmt = conn.prepare_cached(
                "SELECT x, y, kind, level, player_id FROM world_tiles
                 WHERE world_id = ?1 AND x BETWEEN ?2 AND ?3 AND y BETWEEN ?4 AND ?5
                 UNION ALL SELECT x, y, kind, 0, player_id FROM world_buildings
                 WHERE world_id = ?1 AND x BETWEEN ?2 AND ?3 AND y BETWEEN ?4 AND ?5",
            )?;
            let mut rows = stmt.query(params![world_id, x0, x1, y0, y1])?;
            while let Some(row) = rows.next()? {
                if let Some((pos, tile)) = tile_from_row(row)? {
                    tiles.insert(pos, tile);
                }
            }
        }
    }
    Ok(tiles)
}

fn q_write_changes(
    conn: &Connection,
    world_id: &str,
    changes: &[Change],
    now: i64,
) -> rusqlite::Result<()> {
    let mut set = conn.prepare_cached(
        "INSERT INTO world_tiles (world_id, x, y, kind, level, player_id, built_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (world_id, x, y) DO UPDATE SET
             built_at = CASE WHEN kind = excluded.kind AND player_id IS excluded.player_id
                             THEN built_at ELSE excluded.built_at END,
             kind = excluded.kind, level = excluded.level, player_id = excluded.player_id",
    )?;
    let mut remove =
        conn.prepare_cached("DELETE FROM world_tiles WHERE world_id = ?1 AND x = ?2 AND y = ?3")?;
    // A building has its own table, because `world_tiles` lists the kinds it takes and existing
    // tables don't change. It takes over its tile, which may be cleared forest, and forest it was
    // on goes back to cleared when it comes down.
    let mut set_building = conn.prepare_cached(
        "INSERT INTO world_buildings (world_id, x, y, kind, player_id, built_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    let mut remove_building =
        conn.prepare_cached("DELETE FROM world_buildings WHERE world_id = ?1 AND x = ?2 AND y = ?3")?;
    for change in changes {
        match change {
            Change::Set((x, y), tile) if tile.kind.is_building() => {
                remove.execute(params![world_id, x, y])?;
                set_building.execute(params![
                    world_id,
                    x,
                    y,
                    tile.kind.as_str(),
                    tile.player.map(|player| player as i64),
                    now
                ])?;
            }
            Change::Set((x, y), tile) => {
                if tile.kind == Kind::Cleared {
                    remove_building.execute(params![world_id, x, y])?;
                }
                set.execute(params![
                    world_id,
                    x,
                    y,
                    tile.kind.as_str(),
                    tile.level,
                    tile.player.map(|player| player as i64),
                    now
                ])?;
            }
            Change::Remove((x, y)) => {
                remove.execute(params![world_id, x, y])?;
                remove_building.execute(params![world_id, x, y])?;
            }
        }
    }
    Ok(())
}

fn q_world_end(conn: &Connection, world_id: &str) -> rusqlite::Result<Option<WorldEnd>> {
    conn.query_row(
        "SELECT clock_step, omen_step, ended_step FROM world_ends WHERE world_id = ?",
        [world_id],
        |row| {
            Ok(WorldEnd {
                clock_step: row.get::<_, i64>(0)? as u64,
                omen_step: row.get::<_, Option<i64>>(1)?.map(|step| step as u64),
                ended_step: row.get::<_, Option<i64>>(2)?.map(|step| step as u64),
            })
        },
    )
    .optional()
}

fn q_place_clock(conn: &Connection, world_id: &str, player_id: i64) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT since FROM place_clocks WHERE world_id = ?1 AND player_id = ?2",
        params![world_id, player_id],
        |row| row.get(0),
    )
    .optional()
}

fn q_world_player(
    conn: &Connection,
    world_id: &str,
    player_id: i64,
) -> rusqlite::Result<Option<(Pos, Option<String>)>> {
    conn.query_row(
        "SELECT x, y, emote FROM world_players WHERE world_id = ? AND player_id = ?",
        params![world_id, player_id],
        |row| {
            let (x, y): (i64, i64) = (row.get(0)?, row.get(1)?);
            Ok(((x as u16, y as u16), row.get(2)?))
        },
    )
    .optional()
}

/// FR1: the ids of a person's friends: everyone in a room or city with them, and everyone who
/// answered a plan in one of their rooms through its link. It is one way for a plan's link, because
/// someone who answered one can't see who is in its room (F11). Takes the person as ?1.
const FRIENDS_OF: &str = "(
    SELECT b.person_id FROM members a JOIN members b ON b.hangout_id = a.hangout_id
      WHERE a.person_id = ?1
    UNION SELECT b.person_id FROM city_members a JOIN city_members b ON b.city_id = a.city_id
      WHERE a.person_id = ?1
    UNION SELECT i.person_id FROM members a JOIN events e ON e.hangout_id = a.hangout_id
      JOIN plan_invitees i ON i.event_id = e.id WHERE a.person_id = ?1)";

/// What `item_from_row` reads, and where from.
const ITEM_COLS: &str = "i.id, i.hangout_id, i.kind, i.x, i.y, i.facing, i.placed_by, p.name, i.placed_at";
const ITEMS: &str = "FROM floor_items i JOIN people p ON p.id = i.placed_by";

fn item_from_row(row: &Row) -> rusqlite::Result<FloorItem> {
    Ok(FloorItem {
        id: row.get(0)?,
        hangout_id: row.get(1)?,
        kind: row.get(2)?,
        x: row.get(3)?,
        y: row.get(4)?,
        facing: row.get(5)?,
        placed_by: row.get(6)?,
        placed_by_name: row.get(7)?,
        placed_at: row.get(8)?,
    })
}

fn items_on(conn: &Connection, hangout_id: &str) -> rusqlite::Result<Vec<FloorItem>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ITEM_COLS} {ITEMS} WHERE i.hangout_id = ? ORDER BY i.placed_at, i.rowid"
    ))?;
    let rows = stmt.query_map([hangout_id], item_from_row)?;
    rows.collect()
}

/// What `item_note_from_row` reads, and where from.
const ITEM_NOTE_COLS: &str = "n.id, n.item_id, n.body, n.person_id, p.name, n.via_agent, n.created_at";
const ITEM_NOTES: &str = "FROM item_notes n JOIN people p ON p.id = n.person_id";

fn item_note_from_row(row: &Row) -> rusqlite::Result<ItemNote> {
    Ok(ItemNote {
        id: row.get(0)?,
        item_id: row.get(1)?,
        body: row.get(2)?,
        person_id: row.get(3)?,
        writer: row.get(4)?,
        via_agent: row.get(5)?,
        created_at: row.get(6)?,
    })
}

/// What `world_player_from_row` reads. A person's own player has a person and no token or bot.
const WORLD_PLAYER_COLS: &str = "p.id, p.name_seed, w.x, w.y, w.emote,
    p.person_id IS NOT NULL AND p.agent_id IS NULL AND p.token_hash IS NULL";

fn world_player_from_row(row: &Row) -> rusqlite::Result<WorldPlayer> {
    let (x, y): (i64, i64) = (row.get(2)?, row.get(3)?);
    Ok(WorldPlayer {
        id: row.get(0)?,
        name_seed: row.get(1)?,
        pos: (x as u16, y as u16),
        emote: row.get(4)?,
        is_person: row.get(5)?,
    })
}

fn end_bot_player_token(conn: &Connection, agent_id: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE players SET token_hash = NULL WHERE agent_id = ?", [agent_id])?;
    Ok(())
}

fn room_for_own_world(
    conn: &Connection,
    owner_id: Option<&str>,
    max_own: i64,
) -> rusqlite::Result<WorldAdded> {
    if let Some(owner) = owner_id {
        if exists(conn, "SELECT 1 FROM worlds WHERE owner_id = ?", [owner])? {
            return Ok(WorldAdded::OwnerHasOne);
        }
    }
    let held: i64 =
        conn.query_row("SELECT COUNT(*) FROM worlds WHERE kind = 'own'", [], |row| row.get(0))?;
    Ok(if held >= max_own { WorldAdded::Full } else { WorldAdded::Added })
}

fn insert_world_rows(conn: &Connection, new: &NewWorld) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO worlds (id, name, kind, seed, size, owner_id, open, invite_code, step,
                             active_at, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
        params![
            new.id,
            new.name,
            new.kind,
            new.seed as i64,
            new.size as i64,
            new.owner_id,
            new.open,
            new.invite_code,
            new.now,
            new.now
        ],
    )?;
    let changes: Vec<Change> = new
        .highway
        .iter()
        .map(|pos| {
            Change::Set(
                *pos,
                Tile {
                    kind: Kind::Road,
                    level: 0,
                    player: None,
                },
            )
        })
        .collect();
    q_write_changes(conn, new.id, &changes, new.now)
}

/// A transaction on the worlds tables. Everything a move or a step reads and writes goes through
/// one of these, so a move is planned against the tiles it will land on and nothing else changes
/// them in between. It holds the database's connection: inside `Db::in_transaction`, call nothing
/// else on the `Db`.
pub struct WorldTx<'a> {
    tx: rusqlite::Transaction<'a>,
}

impl WorldTx<'_> {
    pub fn world(&self, id: &str) -> rusqlite::Result<Option<WorldRow>> {
        q_world(&self.tx, id)
    }

    pub fn player(&self, id: i64) -> rusqlite::Result<Option<PlayerRow>> {
        q_player(&self.tx, id)
    }

    /// When a player was last seen, or `None` if there is no such player any more.
    pub fn player_seen_at(&self, id: i64) -> rusqlite::Result<Option<i64>> {
        self.tx
            .query_row("SELECT seen_at FROM players WHERE id = ?", [id], |row| row.get(0))
            .optional()
    }

    /// The tiles inside a box, corners included.
    pub fn tiles_in(&self, world_id: &str, region: [i64; 4]) -> rusqlite::Result<Tiles> {
        q_tiles(&self.tx, world_id, Some(region))
    }

    /// W56: this player's houses, this player's other buildings, and all the buildings in the world.
    pub fn building_counts(&self, world_id: &str, player_id: i64) -> rusqlite::Result<(i64, i64, i64)> {
        self.tx.query_row(
            "SELECT COALESCE(SUM(player_id = ?2 AND kind = 'house'), 0),
                    COALESCE(SUM(player_id = ?2 AND kind <> 'house'), 0),
                    COUNT(*)
             FROM world_buildings WHERE world_id = ?1",
            params![world_id, player_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
    }

    pub fn all_tiles(&self, world_id: &str) -> rusqlite::Result<Tiles> {
        q_tiles(&self.tx, world_id, None)
    }

    pub fn write_changes(&self, world_id: &str, changes: &[Change], now: i64) -> rusqlite::Result<()> {
        q_write_changes(&self.tx, world_id, changes, now)
    }

    pub fn world_player(
        &self,
        world_id: &str,
        player_id: i64,
    ) -> rusqlite::Result<Option<(Pos, Option<String>)>> {
        q_world_player(&self.tx, world_id, player_id)
    }

    /// Puts a player in a world where they stand at `start`, or notes that they were here just now.
    pub fn enter_world(
        &self,
        world_id: &str,
        player_id: i64,
        start: Pos,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.tx.execute(
            "INSERT INTO world_players (world_id, player_id, x, y, emote, seen_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)
             ON CONFLICT (world_id, player_id) DO UPDATE SET seen_at = excluded.seen_at",
            params![world_id, player_id, start.0, start.1, now],
        )?;
        Ok(())
    }

    /// A walk: the new place, and the emote they were making ends.
    pub fn set_position(&self, world_id: &str, player_id: i64, pos: Pos) -> rusqlite::Result<()> {
        self.tx.execute(
            "UPDATE world_players SET x = ?3, y = ?4, emote = NULL WHERE world_id = ?1 AND player_id = ?2",
            params![world_id, player_id, pos.0, pos.1],
        )?;
        Ok(())
    }

    pub fn set_emote(&self, world_id: &str, player_id: i64, emote: &str) -> rusqlite::Result<()> {
        self.tx.execute(
            "UPDATE world_players SET emote = ?3 WHERE world_id = ?1 AND player_id = ?2",
            params![world_id, player_id, emote],
        )?;
        Ok(())
    }

    /// Sets what a player has used today, and notes they were here.
    pub fn charge(&self, player_id: i64, day: i64, used: i64, now: i64) -> rusqlite::Result<()> {
        self.tx.execute(
            "UPDATE players SET moves_day = ?2, moves_used = ?3, seen_at = ?4 WHERE id = ?1",
            params![player_id, day, used, now],
        )?;
        Ok(())
    }

    /// A move was made, so the world has someone in it and keeps stepping.
    pub fn wake_world(&self, world_id: &str, now: i64) -> rusqlite::Result<()> {
        self.tx.execute(
            "UPDATE worlds SET active_at = MAX(active_at, ?2) WHERE id = ?1",
            params![world_id, now],
        )?;
        Ok(())
    }

    pub fn set_step(&self, world_id: &str, step: u64) -> rusqlite::Result<()> {
        self.tx
            .execute("UPDATE worlds SET step = ?2 WHERE id = ?1", params![world_id, step as i64])?;
        Ok(())
    }

    /// W61: when this player's first house went up.
    pub fn place_clock(&self, world_id: &str, player_id: i64) -> rusqlite::Result<Option<i64>> {
        q_place_clock(&self.tx, world_id, player_id)
    }

    /// W61: starts a player's clock, and leaves one that has already started alone.
    pub fn start_place_clock(&self, world_id: &str, player_id: i64, now: i64) -> rusqlite::Result<()> {
        self.tx.execute(
            "INSERT INTO place_clocks (world_id, player_id, since) VALUES (?1, ?2, ?3)
             ON CONFLICT (world_id, player_id) DO NOTHING",
            params![world_id, player_id, now],
        )?;
        Ok(())
    }

    pub fn world_end(&self, world_id: &str) -> rusqlite::Result<Option<WorldEnd>> {
        q_world_end(&self.tx, world_id)
    }

    /// W64: whether a tile of the old city is on this world.
    pub fn has_old_city(&self, world_id: &str) -> rusqlite::Result<bool> {
        self.tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM world_tiles
                            WHERE world_id = ?1 AND kind IN ('homes', 'shops', 'workshops', 'park'))",
            [world_id],
            |row| row.get(0),
        )
    }

    /// W64: the clock starts, once.
    pub fn start_end_clock(&self, world_id: &str, step: u64) -> rusqlite::Result<()> {
        self.tx.execute(
            "INSERT INTO world_ends (world_id, clock_step) VALUES (?1, ?2)
             ON CONFLICT (world_id) DO NOTHING",
            params![world_id, step as i64],
        )?;
        Ok(())
    }

    /// W66: the omens start.
    pub fn start_omens(&self, world_id: &str, step: u64) -> rusqlite::Result<()> {
        self.tx.execute(
            "UPDATE world_ends SET omen_step = ?2 WHERE world_id = ?1",
            params![world_id, step as i64],
        )?;
        Ok(())
    }

    /// W68, W69: it's over, and everyone's wait starts again.
    pub fn finish_end(&self, world_id: &str, step: u64) -> rusqlite::Result<()> {
        self.tx.execute(
            "UPDATE world_ends SET ended_step = ?2 WHERE world_id = ?1",
            params![world_id, step as i64],
        )?;
        self.tx.execute("DELETE FROM place_clocks WHERE world_id = ?", [world_id])?;
        Ok(())
    }

    /// `None` if the player has no standing with the world, otherwise whether they are banned from it.
    pub fn member_state(&self, world_id: &str, player_id: i64) -> rusqlite::Result<Option<bool>> {
        self.tx
            .query_row(
                "SELECT banned FROM world_members WHERE world_id = ? AND player_id = ?",
                params![world_id, player_id],
                |row| row.get(0),
            )
            .optional()
    }

}

impl Db {
    /// Runs `work` in one transaction that nothing else can interleave with, and commits it if
    /// `work` returns `Ok`. Anything it returns as an error rolls the whole thing back, which is
    /// how a refused move costs nothing. The work must not call the `Db` again: the connection is
    /// held, and its lock isn't reentrant.
    pub fn in_transaction<T, E: From<rusqlite::Error>>(
        &self,
        work: impl FnOnce(&WorldTx) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut conn = self.conn();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(E::from)?;
        let world_tx = WorldTx { tx };
        let out = work(&world_tx)?;
        world_tx.tx.commit().map_err(E::from)?;
        Ok(out)
    }

    // Worlds

    pub fn world(&self, id: &str) -> rusqlite::Result<Option<WorldRow>> {
        q_world(&self.conn(), id)
    }

    pub fn world_by_code(&self, code: &str) -> rusqlite::Result<Option<WorldRow>> {
        self.conn()
            .query_row(
                &format!("SELECT {WORLD_COLS} FROM worlds WHERE invite_code = ?"),
                [code],
                world_from_row,
            )
            .optional()
    }

    pub fn world_of_owner(&self, person_id: &str) -> rusqlite::Result<Option<WorldRow>> {
        self.conn()
            .query_row(
                &format!("SELECT {WORLD_COLS} FROM worlds WHERE owner_id = ?"),
                [person_id],
                world_from_row,
            )
            .optional()
    }

    /// Adds a world and its highway together. An `own` world is refused if its owner already has
    /// one or the server already holds `max_own` of them.
    pub fn insert_world(&self, new: &NewWorld, max_own: i64) -> rusqlite::Result<WorldAdded> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if exists(&tx, "SELECT 1 FROM worlds WHERE id = ?", [new.id])? {
            return Ok(WorldAdded::Exists);
        }
        if new.kind == "own" {
            let room = room_for_own_world(&tx, new.owner_id, max_own)?;
            if room != WorldAdded::Added {
                return Ok(room);
            }
        }
        insert_world_rows(&tx, new)?;
        tx.commit()?;
        Ok(WorldAdded::Added)
    }

    /// The busiest worlds first: most players moving here lately, then the most recent move.
    /// With how many players have moved in each since `since`.
    pub fn busiest_worlds(&self, since: i64, limit: i64) -> rusqlite::Result<Vec<(WorldRow, i64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {WORLD_COLS},
                    (SELECT COUNT(*) FROM world_players p
                      WHERE p.world_id = worlds.id AND p.seen_at >= ?1) AS busy
             FROM worlds ORDER BY busy DESC, active_at DESC, id LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![since, limit], |row| {
            Ok((world_from_row(row)?, row.get::<_, i64>(11)?))
        })?;
        rows.collect()
    }

    /// A world, its tiles and who is in it, read together. Players are the `limit` who moved most
    /// recently since `since`.
    pub fn world_snapshot(
        &self,
        id: &str,
        since: i64,
        limit: i64,
    ) -> rusqlite::Result<Option<Snapshot>> {
        let conn = self.conn();
        let Some(world) = q_world(&conn, id)? else {
            return Ok(None);
        };
        let tiles = q_tiles(&conn, id, None)?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {WORLD_PLAYER_COLS}
             FROM world_players w JOIN players p ON p.id = w.player_id
             WHERE w.world_id = ?1 AND w.seen_at >= ?2 AND p.removed = 0
             ORDER BY w.seen_at DESC, p.id LIMIT ?3",
        ))?;
        let players = stmt
            .query_map(params![id, since, limit], world_player_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let active = conn.query_row(
            "SELECT COUNT(*) FROM world_players WHERE world_id = ? AND seen_at >= ?",
            params![id, since],
            |row| row.get(0),
        )?;
        Ok(Some(Snapshot { world, tiles, players, active }))
    }

    pub fn world_tiles(&self, world_id: &str) -> rusqlite::Result<Tiles> {
        q_tiles(&self.conn(), world_id, None)
    }

    pub fn world_end(&self, world_id: &str) -> rusqlite::Result<Option<WorldEnd>> {
        q_world_end(&self.conn(), world_id)
    }

    /// W61: when this player's first house went up.
    pub fn place_clock(&self, world_id: &str, player_id: i64) -> rusqlite::Result<Option<i64>> {
        q_place_clock(&self.conn(), world_id, player_id)
    }

    /// The tiles inside a box, corners included.
    pub fn world_tiles_in(&self, world_id: &str, region: [i64; 4]) -> rusqlite::Result<Tiles> {
        q_tiles(&self.conn(), world_id, Some(region))
    }

    /// Where a player stands in a world, and the emote they're making.
    pub fn world_player(
        &self,
        world_id: &str,
        player_id: i64,
    ) -> rusqlite::Result<Option<(Pos, Option<String>)>> {
        q_world_player(&self.conn(), world_id, player_id)
    }

    /// FR1: a person's friends by name, worked out from what they share right now.
    pub fn friends_of(&self, person_id: &str, limit: i64) -> rusqlite::Result<Vec<Person>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT id, name, color FROM people
             WHERE id != ?1 AND id IN {FRIENDS_OF}
             ORDER BY name, id LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![person_id, limit], person_from_row)?;
        rows.collect()
    }

    pub fn is_friend(&self, person_id: &str, other_id: &str) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            &format!("SELECT 1 FROM people WHERE id = ?2 AND id != ?1 AND id IN {FRIENDS_OF}"),
            params![person_id, other_id],
        )
    }

    /// I19: how many friends a person has right now. Nothing keeps this, so it can't go stale.
    pub fn friend_count(&self, person_id: &str) -> rusqlite::Result<i64> {
        self.conn().query_row(
            &format!("SELECT COUNT(*) FROM people WHERE id != ?1 AND id IN {FRIENDS_OF}"),
            [person_id],
            |row| row.get(0),
        )
    }

    // Items on a floor (docs/rooms.md)

    /// What stands on a floor, oldest first. A room that isn't a floor has none.
    pub fn floor_items(&self, hangout_id: &str) -> rusqlite::Result<Vec<FloorItem>> {
        items_on(&self.conn(), hangout_id)
    }

    pub fn floor_item(&self, id: &str) -> rusqlite::Result<Option<FloorItem>> {
        self.conn()
            .query_row(
                &format!("SELECT {ITEM_COLS} {ITEMS} WHERE i.id = ?"),
                [id],
                item_from_row,
            )
            .optional()
    }

    /// Stores an item if `check` allows it, given the floor's access and what's on the floor. In one
    /// step, so two people can't both take the last place or the same spot (I3, I4), and a flirty
    /// item can't land on a floor that's opening (I11).
    pub fn place_item<E>(
        &self,
        item: &NewFloorItem,
        check: impl FnOnce(&str, &[FloorItem]) -> Result<(), E>,
    ) -> rusqlite::Result<Result<(), E>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let access: String =
            tx.query_row("SELECT access FROM floors WHERE hangout_id = ?", [item.hangout_id], |row| row.get(0))?;
        if let Err(refused) = check(&access, &items_on(&tx, item.hangout_id)?) {
            return Ok(Err(refused));
        }
        tx.execute(
            "INSERT INTO floor_items (id, hangout_id, kind, x, y, facing, placed_by, placed_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![item.id, item.hangout_id, item.kind, item.x, item.y, item.facing, item.placed_by, item.now],
        )?;
        tx.commit()?;
        Ok(Ok(()))
    }

    /// Moves or turns an item if `check` allows it, given everything else on the floor. In one step
    /// for the same reason as `place_item`. False if the item has gone in the meantime.
    pub fn change_item<E>(
        &self,
        item: &FloorItem,
        (x, y, facing): (i64, i64, i64),
        check: impl FnOnce(&[FloorItem]) -> Result<(), E>,
    ) -> rusqlite::Result<Result<bool, E>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut others = items_on(&tx, &item.hangout_id)?;
        others.retain(|other| other.id != item.id);
        if let Err(refused) = check(&others) {
            return Ok(Err(refused));
        }
        let changed = tx.execute(
            "UPDATE floor_items SET x = ?, y = ?, facing = ? WHERE id = ?",
            params![x, y, facing, item.id],
        )?;
        tx.commit()?;
        Ok(Ok(changed > 0))
    }

    /// Takes an item away, and its notes with it (I15). False if it had already gone.
    pub fn remove_item(&self, id: &str) -> rusqlite::Result<bool> {
        Ok(self.conn().execute("DELETE FROM floor_items WHERE id = ?", [id])? > 0)
    }

    /// Whether a floor has an item of this kind, which is how a sign works (I17).
    pub fn floor_has_kind(&self, hangout_id: &str, kind: &str) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            "SELECT 1 FROM floor_items WHERE hangout_id = ? AND kind = ?",
            params![hangout_id, kind],
        )
    }

    /// Adds a note. False, and nothing changes, if the item already holds `max` (I12).
    pub fn insert_item_note(&self, note: &NewItemNote, max: i64) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "INSERT INTO item_notes (id, item_id, body, person_id, via_agent, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6
             WHERE (SELECT COUNT(*) FROM item_notes WHERE item_id = ?2) < ?7",
            params![note.id, note.item_id, note.body, note.person_id, note.via_agent, note.now, max],
        )?;
        Ok(changed > 0)
    }

    /// An item's notes written after `since`, oldest first. Times are whole seconds, so a guest's
    /// second of arrival is left out, as in `messages_since`.
    pub fn item_notes(&self, item_id: &str, since: i64) -> rusqlite::Result<Vec<ItemNote>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {ITEM_NOTE_COLS} {ITEM_NOTES} WHERE n.item_id = ? AND n.created_at > ?
             ORDER BY n.created_at, n.rowid"
        ))?;
        let rows = stmt.query_map(params![item_id, since], item_note_from_row)?;
        rows.collect()
    }

    pub fn item_note(&self, id: &str) -> rusqlite::Result<Option<ItemNote>> {
        self.conn()
            .query_row(
                &format!("SELECT {ITEM_NOTE_COLS} {ITEM_NOTES} WHERE n.id = ?"),
                [id],
                item_note_from_row,
            )
            .optional()
    }

    pub fn remove_item_note(&self, id: &str) -> rusqlite::Result<bool> {
        Ok(self.conn().execute("DELETE FROM item_notes WHERE id = ?", [id])? > 0)
    }

    /// How many notes each item on a floor holds, counting those written after `since`.
    pub fn item_note_counts(
        &self,
        hangout_id: &str,
        since: i64,
    ) -> rusqlite::Result<std::collections::HashMap<String, i64>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT n.item_id, COUNT(*) FROM item_notes n JOIN floor_items i ON i.id = n.item_id
             WHERE i.hangout_id = ? AND n.created_at > ? GROUP BY n.item_id",
        )?;
        let rows = stmt.query_map(params![hangout_id, since], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// I10: turns moods on or off for a bot.
    pub fn set_bot_moods(&self, agent_id: &str, on: bool) -> rusqlite::Result<()> {
        let sql = if on {
            "DELETE FROM bots_without_moods WHERE agent_id = ?"
        } else {
            "INSERT OR IGNORE INTO bots_without_moods (agent_id) VALUES (?)"
        };
        self.conn().execute(sql, [agent_id])?;
        Ok(())
    }

    pub fn bot_has_moods(&self, agent_id: &str) -> rusqlite::Result<bool> {
        Ok(!exists(&self.conn(), "SELECT 1 FROM bots_without_moods WHERE agent_id = ?", [agent_id])?)
    }

    /// Which of this person's bots have moods turned off, for their own page.
    pub fn bots_without_moods(&self, person_id: &str) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT m.agent_id FROM bots_without_moods m JOIN agents a ON a.id = m.agent_id
             WHERE a.owner_id = ?",
        )?;
        let rows = stmt.query_map([person_id], |row| row.get(0))?;
        rows.collect()
    }

    /// FR6: adds an invite, or wakes one that lapsed before `lapsed`. False if nothing changed, as
    /// when one is already waiting or they said no.
    #[allow(clippy::too_many_arguments)]
    pub fn add_friend_invite(
        &self,
        id: &str,
        kind: &str,
        place_id: &str,
        from_person: &str,
        to_person: &str,
        now: i64,
        lapsed: i64,
    ) -> rusqlite::Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "INSERT INTO friend_invites (id, kind, place_id, from_person, to_person, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (kind, place_id, to_person) DO UPDATE
               SET from_person = excluded.from_person, created_at = excluded.created_at
               WHERE declined_at IS NULL AND created_at <= ?7",
            params![id, kind, place_id, from_person, to_person, now, lapsed],
        )?;
        // FR6: sending again brings back one that ended. It isn't new, so nobody is pinged.
        tx.execute(
            "DELETE FROM friend_invite_ends WHERE invite_id IN
               (SELECT id FROM friend_invites
                WHERE kind = ? AND place_id = ? AND to_person = ? AND declined_at IS NULL)",
            params![kind, place_id, to_person],
        )?;
        tx.commit()?;
        Ok(changed > 0)
    }

    pub fn friend_invites_sent_since(&self, person_id: &str, since: i64) -> rusqlite::Result<i64> {
        self.conn().query_row(
            "SELECT COUNT(*) FROM friend_invites WHERE from_person = ? AND created_at > ?",
            params![person_id, since],
            |row| row.get(0),
        )
    }

    /// FR5: invites still waiting: not turned down, not ended, not lapsed, to a place the sender is
    /// still in and the person isn't, with that place's link as it is now.
    pub fn friend_invites_for(
        &self,
        person_id: &str,
        since: i64,
        limit: i64,
    ) -> rusqlite::Result<Vec<FriendInvite>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT f.id, f.kind, p.name, COALESCE(h.name, c.name), COALESCE(h.invite_code, c.invite_code)
             FROM friend_invites f
             JOIN people p ON p.id = f.from_person
             LEFT JOIN hangouts h ON f.kind = 'room' AND h.id = f.place_id
             LEFT JOIN cities c ON f.kind = 'city' AND c.id = f.place_id
             WHERE f.to_person = ?1 AND f.declined_at IS NULL AND f.created_at > ?2
               AND NOT EXISTS (SELECT 1 FROM friend_invite_ends e WHERE e.invite_id = f.id)
               AND NOT EXISTS (SELECT 1 FROM floors fl JOIN turned_out t ON t.space_id = fl.space_id
                               WHERE fl.hangout_id = f.place_id AND t.person_id = ?1)
               AND (h.id IS NOT NULL OR c.id IS NOT NULL)
               AND NOT EXISTS (SELECT 1 FROM members m WHERE m.hangout_id = f.place_id AND m.person_id = ?1)
               AND NOT EXISTS (SELECT 1 FROM city_members m WHERE m.city_id = f.place_id AND m.person_id = ?1)
               AND (EXISTS (SELECT 1 FROM members m WHERE m.hangout_id = f.place_id AND m.person_id = f.from_person)
                    OR EXISTS (SELECT 1 FROM city_members m WHERE m.city_id = f.place_id AND m.person_id = f.from_person))
             ORDER BY f.created_at DESC, f.id LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![person_id, since, limit], |row| {
            Ok(FriendInvite {
                id: row.get(0)?,
                kind: row.get(1)?,
                from_name: row.get(2)?,
                place_name: row.get(3)?,
                code: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// True if there was an invite to this person, not already turned down.
    pub fn decline_friend_invite(&self, id: &str, person_id: &str, now: i64) -> rusqlite::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE friend_invites SET declined_at = ?
             WHERE id = ? AND to_person = ? AND declined_at IS NULL",
            params![now, id, person_id],
        )?;
        Ok(changed > 0)
    }

    /// Whether a person's own player has moved in a world since `since`.
    pub fn own_player_is_active(
        &self,
        person_id: &str,
        world_id: &str,
        since: i64,
    ) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            "SELECT 1 FROM players p JOIN world_players w ON w.player_id = p.id
             WHERE p.person_id = ? AND p.token_hash IS NULL AND p.agent_id IS NULL AND p.removed = 0
               AND w.world_id = ? AND w.seen_at >= ?",
            params![person_id, world_id, since],
        )
    }

    /// Who is in a world, for a window of the map: those who moved here since `since` and stand
    /// inside the box `[x0, y0, x1, y1]`.
    pub fn world_players_in(
        &self,
        world_id: &str,
        since: i64,
        region: [i64; 4],
        limit: i64,
    ) -> rusqlite::Result<Vec<WorldPlayer>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {WORLD_PLAYER_COLS}
             FROM world_players w JOIN players p ON p.id = w.player_id
             WHERE w.world_id = ?1 AND w.seen_at >= ?2 AND p.removed = 0
               AND w.x BETWEEN ?3 AND ?4 AND w.y BETWEEN ?5 AND ?6
             ORDER BY w.seen_at DESC, p.id LIMIT ?7",
        ))?;
        let rows = stmt.query_map(
            params![world_id, since, region[0], region[2], region[1], region[3], limit],
            world_player_from_row,
        )?;
        rows.collect()
    }

    /// Everything a player has built in a world. It reads their tiles only, by the index on world
    /// and player, so it costs what they have built and not what the world holds.
    /// What building stands on a tile and the person it belongs to, for the door to its room. Not
    /// for anything public: W4 and W59 say a world never tells anyone whose a building is.
    /// W71: every place in a world with its owner's name, `(x, y, name)`.
    pub fn place_owners(&self, world_id: &str) -> rusqlite::Result<Vec<(i64, i64, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT b.x, b.y, pe.name FROM world_buildings b
             JOIN players pl ON pl.id = b.player_id
             JOIN people pe ON pe.id = pl.person_id
             WHERE b.world_id = ?1
             ORDER BY b.y, b.x",
        )?;
        let rows = stmt.query_map([world_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect()
    }

    pub fn building_at(
        &self,
        world_id: &str,
        x: i64,
        y: i64,
    ) -> rusqlite::Result<Option<(String, Option<String>)>> {
        self.conn()
            .query_row(
                "SELECT b.kind, p.person_id FROM world_buildings b
                 JOIN players p ON p.id = b.player_id
                 WHERE b.world_id = ?1 AND b.x = ?2 AND b.y = ?3",
                params![world_id, x, y],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    pub fn player_tiles(&self, world_id: &str, player_id: i64) -> rusqlite::Result<Tiles> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT x, y, kind, level, player_id FROM world_tiles WHERE world_id = ?1 AND player_id = ?2
             UNION ALL SELECT x, y, kind, 0, player_id FROM world_buildings
                       WHERE world_id = ?1 AND player_id = ?2",
        )?;
        let mut rows = stmt.query(params![world_id, player_id])?;
        let mut tiles = Tiles::new();
        while let Some(row) = rows.next()? {
            if let Some((pos, tile)) = tile_from_row(row)? {
                tiles.insert(pos, tile);
            }
        }
        Ok(tiles)
    }

    // Players

    pub fn player(&self, id: i64) -> rusqlite::Result<Option<PlayerRow>> {
        q_player(&self.conn(), id)
    }

    /// The player a token belongs to, unless they were removed.
    pub fn player_by_token(&self, token_hash: &str) -> rusqlite::Result<Option<PlayerRow>> {
        self.conn()
            .query_row(
                &format!("SELECT {PLAYER_COLS} FROM players WHERE token_hash = ? AND removed = 0"),
                [token_hash],
                player_row_from_row,
            )
            .optional()
    }

    pub fn players_of(&self, person_id: &str) -> rusqlite::Result<Vec<PlayerRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PLAYER_COLS} FROM players WHERE person_id = ? AND removed = 0 ORDER BY id"
        ))?;
        let rows = stmt.query_map([person_id], player_row_from_row)?;
        rows.collect()
    }

    /// A new guest: no person, no bot.
    pub fn create_guest(
        &self,
        token_hash: &str,
        claim_hash: &str,
        now: i64,
    ) -> rusqlite::Result<PlayerRow> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO players (name_seed, token_hash, claim_hash, seen_at, created_at)
             VALUES (?, ?, ?, ?, ?)",
            params![random_name_seed(), token_hash, claim_hash, now, now],
        )?;
        let player = q_player(&tx, tx.last_insert_rowid())?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        tx.commit()?;
        Ok(player)
    }

    /// The player for a bot in `world_id`: made the first time, and after that given a new token.
    /// Refused, with its token left as it was, if that player was removed or is barred from the
    /// world.
    pub fn join_as_bot(
        &self,
        agent_id: &str,
        owner_id: &str,
        token_hash: &str,
        world_id: &str,
        now: i64,
    ) -> rusqlite::Result<BotJoined> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let existing: Option<(i64, bool)> = tx
            .query_row(
                "SELECT id, removed FROM players WHERE agent_id = ?",
                [agent_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let id = match existing {
            Some((_, true)) => return Ok(BotJoined::Removed),
            Some((id, false)) => {
                if exists(
                    &tx,
                    "SELECT 1 FROM world_members WHERE world_id = ? AND player_id = ? AND banned = 1",
                    params![world_id, id],
                )? {
                    return Ok(BotJoined::Barred);
                }
                tx.execute(
                    "UPDATE players SET token_hash = ?, seen_at = ? WHERE id = ?",
                    params![token_hash, now, id],
                )?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO players (name_seed, token_hash, person_id, agent_id, seen_at, created_at)
                     VALUES (?, ?, ?, ?, ?, ?)",
                    params![random_name_seed(), token_hash, owner_id, agent_id, now, now],
                )?;
                tx.last_insert_rowid()
            }
        };
        let player = q_player(&tx, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        tx.commit()?;
        Ok(BotJoined::Done(player))
    }

    /// Whether a signed-in person has their own player yet. This makes nothing.
    pub fn has_own_player(&self, person_id: &str) -> rusqlite::Result<bool> {
        exists(
            &self.conn(),
            "SELECT 1 FROM players WHERE person_id = ? AND token_hash IS NULL AND agent_id IS NULL",
            [person_id],
        )
    }

    /// A signed-in person's own player, made the first time they need it.
    pub fn own_player(&self, person_id: &str, now: i64) -> rusqlite::Result<PlayerRow> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let found: Option<i64> = tx
            .query_row(
                "SELECT id FROM players WHERE person_id = ? AND token_hash IS NULL AND agent_id IS NULL",
                [person_id],
                |row| row.get(0),
            )
            .optional()?;
        let id = match found {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO players (name_seed, person_id, seen_at, created_at) VALUES (?, ?, ?, ?)",
                    params![random_name_seed(), person_id, now, now],
                )?;
                tx.last_insert_rowid()
            }
        };
        let player = q_player(&tx, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        tx.commit()?;
        Ok(player)
    }

    pub fn touch_player(&self, id: i64, now: i64) -> rusqlite::Result<()> {
        self.conn()
            .execute("UPDATE players SET seen_at = ? WHERE id = ?", params![now, id])?;
        Ok(())
    }

    pub fn set_name_seed(&self, id: i64, name_seed: i64) -> rusqlite::Result<()> {
        self.conn()
            .execute("UPDATE players SET name_seed = ? WHERE id = ?", params![name_seed, id])?;
        Ok(())
    }

    /// Hands a guest to a person, once. Refused if the code is unknown or already used, if the
    /// guest has been idle since before `earliest`, or if the person already holds `max`.
    pub fn claim_player(
        &self,
        claim_hash: &str,
        person_id: &str,
        max: i64,
        earliest: i64,
    ) -> rusqlite::Result<Claimed> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let found: Option<i64> = tx
            .query_row(
                "SELECT id FROM players WHERE claim_hash = ? AND removed = 0 AND person_id IS NULL
                   AND agent_id IS NULL AND seen_at >= ?",
                params![claim_hash, earliest],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = found else {
            return Ok(Claimed::NoSuchCode);
        };
        let held: i64 = tx.query_row(
            "SELECT COUNT(*) FROM players WHERE person_id = ? AND token_hash IS NOT NULL
               AND agent_id IS NULL AND removed = 0",
            [person_id],
            |row| row.get(0),
        )?;
        if held >= max {
            return Ok(Claimed::TooMany);
        }
        tx.execute(
            "UPDATE players SET person_id = ?, claim_hash = NULL WHERE id = ?",
            params![person_id, id],
        )?;
        let player = q_player(&tx, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        tx.commit()?;
        Ok(Claimed::Done(player))
    }

    /// Ends a player its person claimed: the token stops and it leaves every street, and what it
    /// built stays. False if the person has no such player.
    pub fn end_claimed_player(&self, player_id: i64, person_id: &str) -> rusqlite::Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ended = tx.execute(
            "UPDATE players SET removed = 1 WHERE id = ? AND person_id = ? AND removed = 0
               AND token_hash IS NOT NULL AND agent_id IS NULL",
            params![player_id, person_id],
        )? == 1;
        if ended {
            tx.execute("DELETE FROM world_players WHERE player_id = ?", [player_id])?;
        }
        tx.commit()?;
        Ok(ended)
    }

    /// Takes a player out of a world: their tiles there come down, and they leave its street.
    /// `ban` stops them building there again. `everywhere` also ends the player for good: their
    /// token stops and their claim code with it. Returns how many tiles came down.
    pub fn remove_from_world(
        &self,
        world_id: &str,
        player_id: i64,
        ban: bool,
        everywhere: bool,
    ) -> rusqlite::Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let down = tx.execute(
            "DELETE FROM world_tiles WHERE world_id = ? AND player_id = ?",
            params![world_id, player_id],
        )? + tx.execute(
            "DELETE FROM world_buildings WHERE world_id = ? AND player_id = ?",
            params![world_id, player_id],
        )?;
        tx.execute(
            "DELETE FROM world_players WHERE world_id = ? AND player_id = ?",
            params![world_id, player_id],
        )?;
        if ban {
            tx.execute(
                "INSERT INTO world_members (world_id, player_id, banned) VALUES (?, ?, 1)
                 ON CONFLICT (world_id, player_id) DO UPDATE SET banned = 1",
                params![world_id, player_id],
            )?;
        }
        if everywhere {
            tx.execute(
                "UPDATE players SET removed = 1, claim_hash = NULL WHERE id = ?",
                [player_id],
            )?;
        }
        tx.commit()?;
        Ok(down)
    }

    /// The worlds a player is standing in.
    pub fn worlds_of_player(&self, player_id: i64) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT world_id FROM world_players WHERE player_id = ?")?;
        let rows = stmt.query_map([player_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Whether a player has any dealings with a world: standing in it, a member, or a builder there.
    pub fn player_known_in(&self, world_id: &str, player_id: i64) -> rusqlite::Result<bool> {
        let conn = self.conn();
        exists(
            &conn,
            "SELECT 1 FROM world_players WHERE world_id = ?1 AND player_id = ?2
             UNION ALL SELECT 1 FROM world_members WHERE world_id = ?1 AND player_id = ?2
             UNION ALL SELECT 1 FROM world_tiles WHERE world_id = ?1 AND player_id = ?2
             UNION ALL SELECT 1 FROM world_buildings WHERE world_id = ?1 AND player_id = ?2 LIMIT 1",
            params![world_id, player_id],
        )
    }

    /// Deletes guests nobody has claimed that haven't been seen since `before`, so a swarm of them
    /// can't fill the table for good. What they built stays, and reads as left behind.
    pub fn prune_guests(&self, before: i64) -> rusqlite::Result<usize> {
        self.conn().execute(
            "DELETE FROM players WHERE token_hash IS NOT NULL AND person_id IS NULL
               AND agent_id IS NULL AND seen_at < ?",
            [before],
        )
    }

    // Crawler sightings: a daily count per kind, and nothing else about who it was.

    pub fn add_sightings(&self, kind: &str, day: i64, visits: i64, now: i64) -> rusqlite::Result<()> {
        self.conn().execute(
            "INSERT INTO crawler_sightings (kind, day, visits, seen_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (kind, day) DO UPDATE SET visits = visits + excluded.visits,
                                                    seen_at = excluded.seen_at",
            params![kind, day, visits, now],
        )?;
        Ok(())
    }

    /// Kind, visits and when last seen, for one day.
    pub fn sightings_on(&self, day: i64) -> rusqlite::Result<Vec<(String, i64, i64)>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT kind, visits, seen_at FROM crawler_sightings WHERE day = ? ORDER BY kind")?;
        let rows = stmt.query_map([day], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_bot_gets_no_new_question_and_a_second_answer_changes_nothing() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Justin", "key", "feed", 0).unwrap();
        db.create_agent("a1", "p1", "Scout", "token", 0).unwrap();
        db.create_hangout("h1", "Crew", "code", "p1", 0).unwrap();
        let add = |id: &str, now: i64| {
            let question = NewOwnerQuestion {
                id,
                hangout_id: "h1",
                agent_id: "a1",
                owner_id: "p1",
                asker: "Scout",
                question: "Free Friday?",
                now,
            };
            db.insert_owner_question(&question, 50, 2).unwrap()
        };

        // A question from before `open_since` doesn't take a place.
        assert!(add("q0", 50));
        assert!(add("q1", 100));
        assert!(add("q2", 101));
        assert!(!add("q3", 102));
        assert!(db.owner_question("q3").unwrap().is_none());
        assert_eq!(db.open_questions_of_bot("a1", 50).unwrap().len(), 2);

        assert!(db.answer_owner_question("q1", "Yes", 110).unwrap());
        assert!(!db.answer_owner_question("q1", "No", 120).unwrap());
        let answered = db.owner_question("q1").unwrap().unwrap();
        assert_eq!((answered.answer.as_deref(), answered.answered_at), (Some("Yes"), Some(110)));
        assert!(add("q3", 130));
    }

    #[test]
    fn activity_disappears_when_it_expires() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Justin", "key", "feed", 0).unwrap();
        db.create_agent("a1", "p1", "Scout", "token", 0).unwrap();

        db.record_activity("a1", "computer", "FlowRMM", 100).unwrap();
        let active = db.agent("a1", 99).unwrap().unwrap();
        assert_eq!(active.activity.unwrap().via, "FlowRMM");
        assert_eq!(db.agent("a1", 100).unwrap().unwrap().activity, None);
    }

    #[test]
    fn a_doorbell_stops_listening_when_it_goes_quiet() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Justin", "key", "feed", 0).unwrap();
        db.create_agent("a1", "p1", "Scout", "token", 0).unwrap();
        let doorbell = |now| db.agent("a1", now).unwrap().unwrap().doorbell;
        assert_eq!(doorbell(0), None);

        assert!(!db.note_doorbell("a1", 100).unwrap());
        assert!(doorbell(100 + LISTENING_SECS - 1).unwrap().listening);
        assert!(!doorbell(100 + LISTENING_SECS).unwrap().listening);

        // A pause outlasts the doorbell going away and coming back.
        db.set_doorbell_paused("a1", true).unwrap();
        assert!(db.note_doorbell("a1", 200).unwrap());
        let paused = doorbell(200).unwrap();
        assert!(paused.listening && paused.paused);
    }

    #[test]
    fn an_unanswered_check_in_expires() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Justin", "key", "feed", 0).unwrap();
        db.create_hangout("h1", "Crew", "code", "p1", 0).unwrap();
        db.insert_check_in(&NewCheckIn {
            id: "c1",
            hangout_id: "h1",
            asked_by: "p1",
            asker: "Justin",
            agent_id: None,
            about: "Book the table",
            to_person: None,
            needs_human: false,
            expires_at: 100,
            now: 0,
        })
        .unwrap();

        assert_eq!(db.check_in("c1", 99).unwrap().unwrap().status, "waiting");
        assert_eq!(db.check_in("c1", 100).unwrap().unwrap().status, "expired");
    }

    fn bot(db: &Db, id: &str, owner_id: &str) {
        db.create_agent(id, owner_id, id, &format!("token-{id}"), 0).unwrap();
    }

    fn city(db: &Db, id: &str, square_id: &str, created_by: &str) {
        db.create_city(&NewCity {
            id,
            name: id,
            invite_code: &format!("{id}-code"),
            square_id,
            square_code: &format!("{square_id}-code"),
            created_by,
            now: 0,
        })
        .unwrap();
    }

    /// A house whose rooms are `{id}-lr` and `{id}-porch`.
    fn house(db: &Db, id: &str, city_id: &str, created_by: &str, now: i64) {
        db.create_house(&NewHouse {
            id,
            city_id,
            name: &format!("House {id}"),
            invite_code: &format!("{id}-code"),
            living_room_id: &format!("{id}-lr"),
            living_room_code: &format!("{id}-lr-code"),
            porch_id: &format!("{id}-porch"),
            porch_code: &format!("{id}-porch-code"),
            created_by,
            now,
        })
        .unwrap();
    }

    /// Ann and Bo share a city. Ann built `hz` first and Bo built `ha`, so build order and id
    /// order disagree. Their bots `b1` and `b2` live at home.
    fn two_houses(db: &Db) {
        db.create_person("p1", "Ann", "key1", "feed1", 0).unwrap();
        db.create_person("p2", "Bo", "key2", "feed2", 0).unwrap();
        city(db, "c1", "sq", "p1");
        db.add_city_member("c1", "p2", 0).unwrap();
        house(db, "hz", "c1", "p1", 10);
        house(db, "ha", "c1", "p2", 20);
        bot(db, "b1", "p1");
        bot(db, "b2", "p2");
        db.set_bot_home("b1", "hz", false, false).unwrap();
        db.set_bot_home("b2", "ha", false, false).unwrap();
        db.add_agent_to_room("hz-lr", "b1").unwrap();
        db.add_agent_to_room("ha-lr", "b2").unwrap();
    }

    /// Asks with both sides' yes already given.
    fn agreed_trip(db: &Db, id: &str, agent_id: &str, house_id: &str, minutes: i64, now: i64) {
        db.insert_trip(&NewTrip {
            id,
            agent_id,
            house_id,
            kind: "visit",
            goal: "",
            minutes,
            asked_by: "p2",
            owner_ok_by: Some("p2"),
            host_ok_by: Some("p1"),
            pass_id: None,
            expires_at: now + 24 * 60 * 60,
            now,
        })
        .unwrap();
    }

    fn start_trip(db: &Db, id: &str, agent_id: &str, house_id: &str, minutes: i64, now: i64) -> Arrival {
        agreed_trip(db, id, agent_id, house_id, minutes, now);
        db.arrive(id, now).unwrap()
    }

    fn say(db: &Db, hangout_id: &str, kind: &str, body: &str, now: i64) -> i64 {
        db.insert_message(&NewMessage {
            hangout_id,
            person_id: None,
            agent_id: None,
            speaker: "Someone",
            kind,
            body,
            now,
        })
        .unwrap()
        .id
    }

    fn plan(db: &Db, id: &str, hangout_id: &str, ends_at: i64, status: &str) {
        db.insert_event(&NewEvent {
            id,
            hangout_id,
            title: "Plan",
            location: "",
            notes: "",
            starts_at: ends_at - 60,
            ends_at,
            min_yes: 1,
            proposed_by: "p1",
            proposer: "Ann",
            now: 0,
        })
        .unwrap();
        db.upsert_rsvp(id, "p1", "yes", None, "", 0).unwrap();
        db.upsert_rsvp(id, "p2", "yes", None, "", 0).unwrap();
        if status != "proposed" {
            db.set_event_status(id, status, 0).unwrap();
        }
    }

    #[test]
    fn plans_together_counts_only_plans_on_the_calendar_that_are_over() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        // Both said yes to each of these, and each ends at 100.
        plan(&db, "over", "sq", 100, "confirmed");
        plan(&db, "never_confirmed", "sq", 100, "proposed");
        plan(&db, "cancelled", "sq", 100, "cancelled");
        plan(&db, "still_on", "sq", 500, "confirmed");
        db.upsert_rsvp("still_on", "p2", "yes", None, "", 0).unwrap();
        plan(&db, "said_no", "sq", 100, "confirmed");
        db.upsert_rsvp("said_no", "p2", "no", None, "", 0).unwrap();

        let relation = db.relation("p1", "p2", 200).unwrap();
        assert_eq!(relation.plans_together, 1);
        assert!(relation.shares_a_city);
        assert!(!relation.lives_with);
        assert_eq!(db.relation("p1", "p2", 500).unwrap().plans_together, 2);
        // One house to a city (R5), so Bo moves out of their own first.
        db.remove_resident("ha", "p2").unwrap();
        assert!(db.add_resident("hz", "p2", 0).unwrap());
        assert!(db.relation("p1", "p2", 200).unwrap().lives_with);
    }

    fn room_ids(db: &Db, agent_id: &str, now: i64) -> Vec<String> {
        let rooms = db.hangouts_for_agent(agent_id, now).unwrap();
        rooms.into_iter().map(|room| room.id).collect()
    }

    #[test]
    fn a_trip_ends_at_the_clock_with_no_sweep() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        assert_eq!(start_trip(&db, "t1", "b2", "hz", 60, 1000), Arrival::Arrived);
        let ends = 1000 + 60 * 60;

        // Only the Porch opens, and only until the minute the trip runs out.
        assert!(!db.agent_in_room("hz-lr", "b2", 1000).unwrap());
        assert!(db.agent_in_room("hz-porch", "b2", ends - 1).unwrap());
        assert!(!db.agent_in_room("hz-porch", "b2", ends).unwrap());
        assert!(db.agent_in_room("ha-lr", "b2", ends).unwrap());
        assert!(room_ids(&db, "b2", ends - 1).contains(&"hz-porch".to_string()));
        assert!(!room_ids(&db, "b2", ends).contains(&"hz-porch".to_string()));
        assert_eq!(room_ids(&db, "b2", ends), ["ha-lr"]);

        // Guests are listed, but never counted among the bots a notification would reach.
        let guests = db.visitors_in("hz-porch", ends - 1).unwrap();
        assert_eq!(guests.len(), 1);
        assert_eq!((guests[0].id.as_str(), guests[0].visitor_until), ("b2", Some(ends)));
        assert!(db.visitors_in("hz-porch", ends).unwrap().is_empty());
        assert!(db.agents_in("hz-porch", ends - 1).unwrap().is_empty());
        assert_eq!(db.agent("b2", ends - 1).unwrap().unwrap().visitor_until, None);

        assert_eq!(db.live_trip("b2", ends - 1).unwrap().unwrap().status, "here");
        assert!(db.live_trip("b2", ends).unwrap().is_none());
        assert_eq!(db.trip("t1", ends - 1).unwrap().unwrap().status, "here");
        assert_eq!(db.trip("t1", ends).unwrap().unwrap().status, "over");
    }

    #[test]
    fn ending_a_trip_counts_at_once_but_not_after_it_ran_out() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        start_trip(&db, "t1", "b2", "hz", 60, 1000);
        let ends = 4600;

        assert!(!db.end_trip("t1", "p1", ends).unwrap());
        assert_eq!(db.trip("t1", ends).unwrap().unwrap().ended_by, None);

        assert!(db.end_trip("t1", "p1", ends - 1).unwrap());
        assert!(!db.agent_in_room("hz-porch", "b2", ends - 1).unwrap());
        let trip = db.trip("t1", ends - 1).unwrap().unwrap();
        assert_eq!((trip.status.as_str(), trip.ended_by.as_deref()), ("ended", Some("p1")));
        assert!(!db.end_trip("t1", "p1", ends - 1).unwrap());
    }

    #[test]
    fn extending_a_trip_only_works_while_it_lasts() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        start_trip(&db, "t1", "b2", "hz", 60, 1000);

        assert!(!db.set_trip_ends_at("t1", 9000, 4600).unwrap());
        assert!(db.set_trip_ends_at("t1", 9000, 4599).unwrap());
        assert!(db.agent_in_room("hz-porch", "b2", 8999).unwrap());
        assert!(!db.agent_in_room("hz-porch", "b2", 9000).unwrap());
    }

    #[test]
    fn an_ask_lapses_with_no_sweep() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        for id in ["t1", "t2"] {
            db.insert_trip(&NewTrip {
                id,
                agent_id: "b2",
                house_id: "hz",
                kind: "call",
                goal: "Pair on the build",
                minutes: 60,
                asked_by: "p2",
                owner_ok_by: Some("p2"),
                host_ok_by: Some("p1"),
                pass_id: None,
                expires_at: 100,
                now: 0,
            })
            .unwrap();
        }

        assert_eq!(db.trip("t1", 99).unwrap().unwrap().status, "asking");
        assert_eq!(db.trip("t1", 100).unwrap().unwrap().status, "expired");
        assert!(db.open_ask("b2", "hz", 99).unwrap().is_some());
        assert!(db.open_ask("b2", "hz", 100).unwrap().is_none());
        assert!(!db.trip_owner_ok("t1", "p2", 100).unwrap());
        assert!(!db.decline_trip("t1", "p1", 100).unwrap());
        assert_eq!(db.arrive("t1", 100).unwrap(), Arrival::Closed);
        assert_eq!(db.arrive("t2", 99).unwrap(), Arrival::Arrived);
    }

    #[test]
    fn a_bot_has_one_trip_at_a_time() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        assert_eq!(start_trip(&db, "t1", "b2", "hz", 60, 1000), Arrival::Arrived);
        let ends = 1000 + 60 * 60;

        agreed_trip(&db, "t2", "b2", "hz", 30, 2000);
        assert_eq!(db.arrive("t2", 2000).unwrap(), Arrival::Busy);
        assert_eq!(db.arrive("t2", ends - 1).unwrap(), Arrival::Busy);
        assert_eq!(db.trip("t2", ends - 1).unwrap().unwrap().status, "asking");
        assert_eq!(db.trip("t1", ends - 1).unwrap().unwrap().status, "here");

        // The first trip ran out at `ends`, so the second can start, and the first reads as over.
        assert_eq!(db.arrive("t2", ends).unwrap(), Arrival::Arrived);
        let second = db.trip("t2", ends).unwrap().unwrap();
        assert_eq!(second.status, "here");
        assert_eq!((second.arrived_at, second.ends_at), (Some(ends), Some(ends + 30 * 60)));
        assert_eq!(db.trip("t1", ends).unwrap().unwrap().status, "over");
        assert_eq!(db.live_trip("b2", ends).unwrap().unwrap().id, "t2");

        // Started already, so it can't start twice.
        assert_eq!(db.arrive("t2", ends + 1).unwrap(), Arrival::Closed);
    }

    #[test]
    fn one_side_saying_yes_does_not_start_a_trip() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        db.insert_trip(&NewTrip {
            id: "t1",
            agent_id: "b1",
            house_id: "ha",
            kind: "visit",
            goal: "",
            minutes: 60,
            asked_by: "b1",
            owner_ok_by: None,
            host_ok_by: None,
            pass_id: None,
            expires_at: 500,
            now: 0,
        })
        .unwrap();

        assert!(db.trip_owner_ok("t1", "p1", 10).unwrap());
        assert!(db.trip_owner_ok("t1", "p9", 11).unwrap());
        assert_eq!(db.arrive("t1", 12).unwrap(), Arrival::Unapproved);
        assert_eq!(db.trip("t1", 12).unwrap().unwrap().owner_ok_by.as_deref(), Some("p1"));
        assert!(!db.agent_in_room("ha-porch", "b1", 12).unwrap());

        assert!(db.trip_host_ok("t1", "p2", 13).unwrap());
        assert_eq!(db.arrive("t1", 14).unwrap(), Arrival::Arrived);
        assert!(!db.trip_host_ok("t1", "p2", 15).unwrap());
    }

    #[test]
    fn a_pass_covers_a_trip_without_fresh_answers() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        db.insert_trip(&NewTrip {
            id: "t1",
            agent_id: "b2",
            house_id: "hz",
            kind: "call",
            goal: "",
            minutes: 30,
            asked_by: "b2",
            owner_ok_by: None,
            host_ok_by: None,
            pass_id: Some("ps1"),
            expires_at: 500,
            now: 0,
        })
        .unwrap();
        assert_eq!(db.arrive("t1", 1).unwrap(), Arrival::Arrived);
        assert_eq!(db.live_trip("b2", 2).unwrap().unwrap().pass_id.as_deref(), Some("ps1"));

        assert_eq!(db.end_trip_for_pass("ps1", "p2", 2).unwrap(), 1);
        assert!(!db.agent_in_room("hz-porch", "b2", 2).unwrap());
        assert_eq!(db.end_trip_for_pass("ps1", "p2", 2).unwrap(), 0);
    }

    #[test]
    fn passes_are_offered_once_and_only_after_a_hosted_trip() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        assert!(!db.has_hosted_trip("b2", "hz").unwrap());

        // A yes from the house that led nowhere isn't a trip the bot has had.
        db.insert_trip(&NewTrip {
            id: "t0",
            agent_id: "b2",
            house_id: "hz",
            kind: "visit",
            goal: "",
            minutes: 60,
            asked_by: "b2",
            owner_ok_by: None,
            host_ok_by: Some("p1"),
            pass_id: None,
            expires_at: 100,
            now: 0,
        })
        .unwrap();
        assert!(db.decline_trip("t0", "p2", 1).unwrap());
        assert!(!db.has_hosted_trip("b2", "hz").unwrap());

        start_trip(&db, "t1", "b2", "hz", 60, 10);
        assert!(db.has_hosted_trip("b2", "hz").unwrap());
        assert!(!db.has_hosted_trip("b2", "ha").unwrap());

        let offer = NewPass {
            id: "ps1",
            house_id: "hz",
            agent_id: "b2",
            max_minutes: 120,
            granted_by: "p1",
            now: 20,
        };
        assert!(db.insert_pass(&offer).unwrap());
        assert!(!db.insert_pass(&NewPass { id: "ps2", ..offer }).unwrap());
        let pass = db.pass("ps1").unwrap().unwrap();
        assert_eq!((pass.agent_name.as_str(), pass.owner_id.as_str()), ("b2", "p2"));
        assert_eq!(pass.accepted_by, None);

        assert!(db.accept_pass("ps1", "p2").unwrap());
        assert!(!db.accept_pass("ps1", "p9").unwrap());
        assert_eq!(db.pass_for_bot("hz", "b2").unwrap().unwrap().accepted_by.as_deref(), Some("p2"));
        assert_eq!(db.passes_for_house("hz").unwrap().len(), 1);
        assert_eq!(db.passes_for_owner("p2").unwrap().len(), 1);
        assert!(db.passes_for_owner("p1").unwrap().is_empty());

        assert!(db.delete_pass("ps1").unwrap());
        assert!(db.pass("ps1").unwrap().is_none());
        assert!(!db.delete_pass("ps1").unwrap());
    }

    #[test]
    fn a_guest_reads_nothing_from_before_arrival() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        say(&db, "hz-porch", "chat", "before", 500);
        say(&db, "hz-porch", "note", "old note", 900);
        // Written in the same second the guest arrived, so it could have come first.
        say(&db, "hz-porch", "chat", "same second", 1000);
        start_trip(&db, "t1", "b2", "hz", 60, 1000);
        let first = say(&db, "hz-porch", "chat", "hello guest", 1001);
        say(&db, "hz-porch", "note", "new note", 1100);
        say(&db, "hz-porch", "chat", "later", 1200);

        let bodies = |messages: Vec<Message>| messages.into_iter().map(|m| m.body).collect::<Vec<_>>();
        assert_eq!(
            bodies(db.messages("hz-porch", 0, 50).unwrap()),
            ["before", "same second", "hello guest", "later"]
        );
        assert_eq!(bodies(db.messages_since("hz-porch", 1000, 0, 50).unwrap()), ["hello guest", "later"]);
        assert_eq!(bodies(db.messages_since("hz-porch", 1000, first, 50).unwrap()), ["later"]);
        assert_eq!(bodies(db.messages_since("hz-porch", 1000, 0, 1).unwrap()), ["later"]);
        assert_eq!(bodies(db.messages_since("hz-porch", 1201, 0, 50).unwrap()), Vec::<String>::new());
        assert_eq!(bodies(db.notes("hz-porch", 50).unwrap()), ["new note", "old note"]);
        assert_eq!(bodies(db.notes_since("hz-porch", 1000, 50).unwrap()), ["new note"]);
        assert_eq!(bodies(db.notes_since("hz-porch", 1101, 50).unwrap()), Vec::<String>::new());

        // The arrival time is what a guest reads from.
        let visit = db.bot_visit("hz-porch", "b2", 1500).unwrap().unwrap();
        assert_eq!(visit, Visit { since: 1000, until: 4600 });
        assert!(db.bot_visit("hz-porch", "b2", 4600).unwrap().is_none());
        assert!(db.bot_visit("hz-lr", "b2", 1500).unwrap().is_none());
        assert!(db.bot_visit("hz-porch", "b1", 1500).unwrap().is_none());
    }

    #[test]
    fn an_owner_is_a_guest_from_their_earliest_bot_arriving() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        bot(&db, "b3", "p2");
        start_trip(&db, "t1", "b2", "hz", 60, 1000);
        start_trip(&db, "t2", "b3", "hz", 24 * 60, 2000);
        let day = 2000 + 24 * 60 * 60;

        let visit = |now| db.person_visit("hz-porch", "p2", now).unwrap();
        assert_eq!(visit(2500), Some(Visit { since: 1000, until: day }));
        // Once the first bot's trip has run out the owner is there for the bot still visiting.
        assert_eq!(visit(4600), Some(Visit { since: 2000, until: day }));
        assert_eq!(visit(day), None);

        // The house's own people are not guests, and neither is anyone without a visiting bot.
        assert_eq!(db.person_visit("hz-porch", "p1", 2500).unwrap(), None);
        assert_eq!(db.person_visit("hz-lr", "p2", 2500).unwrap(), None);
        db.add_member("hz-porch", "p2", 3000).unwrap();
        assert_eq!(db.person_visit("hz-porch", "p2", 3000).unwrap(), None);
        assert!(db.bot_visit("hz-porch", "b2", 3000).unwrap().is_some());
    }

    #[test]
    fn gatherings_count_only_confirmed_finished_town_square_plans() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        city(&db, "c2", "sq2", "p1");
        plan(&db, "done", "sq", 100, "confirmed");
        plan(&db, "cancelled", "sq", 100, "cancelled");
        plan(&db, "unconfirmed", "sq", 100, "proposed");
        plan(&db, "later", "sq", 200, "confirmed");
        plan(&db, "living-room", "hz-lr", 100, "confirmed");
        plan(&db, "porch", "hz-porch", 100, "confirmed");
        plan(&db, "elsewhere", "sq2", 100, "confirmed");
        // One person's yes isn't friends getting together.
        plan(&db, "alone", "sq", 100, "confirmed");
        db.upsert_rsvp("alone", "p2", "no", None, "", 0).unwrap();

        // A plan has finished once its end has passed, not at the moment it ends.
        assert_eq!(db.gatherings("c1", 100).unwrap(), 0);
        assert_eq!(db.gatherings("c1", 101).unwrap(), 1);
        assert_eq!(db.gatherings("c1", 200).unwrap(), 1);
        assert_eq!(db.gatherings("c1", 201).unwrap(), 2);
        assert_eq!(db.gatherings("c2", 201).unwrap(), 1);
        assert_eq!(db.gatherings("nowhere", 201).unwrap(), 0);
    }

    #[test]
    fn roads_join_houses_a_bot_really_arrived_between() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        db.create_person("p3", "Cy", "key3", "feed3", 0).unwrap();
        db.add_city_member("c1", "p3", 0).unwrap();
        house(&db, "hb", "c1", "p3", 30);
        bot(&db, "b3", "p2");
        bot(&db, "b4", "p3");
        db.set_bot_home("b4", "hb", false, false).unwrap();
        city(&db, "c2", "sq2", "p1");
        house(&db, "hq", "c2", "p1", 40);

        // Bo's bot at Ann's house twice, and Ann's bot at Bo's house: one road, `ha` before `hz`
        // although `hz` was built first.
        start_trip(&db, "t1", "b2", "hz", 60, 1000);
        start_trip(&db, "t2", "b2", "hz", 60, 5000);
        start_trip(&db, "t3", "b1", "ha", 60, 1000);
        // Cy's bot at Ann's house: a second road.
        start_trip(&db, "t4", "b4", "hz", 60, 1000);

        // Trips that never started, a bot with no home, a bot at its own house, and another city.
        agreed_trip(&db, "never-answered", "b2", "hb", 60, 6000);
        agreed_trip(&db, "declined", "b2", "hb", 60, 6000);
        assert!(db.decline_trip("declined", "p3", 6001).unwrap());
        start_trip(&db, "no-home", "b3", "hz", 60, 1000);
        start_trip(&db, "own-house", "b1", "hz", 60, 20_000);
        start_trip(&db, "other-city", "b1", "hq", 60, 30_000);

        let road = |a: &str, b: &str, trips| Road { a: a.to_string(), b: b.to_string(), trips };
        assert_eq!(db.roads("c1").unwrap(), [road("ha", "hz", 3), road("hb", "hz", 1)]);
        assert_eq!(db.roads("c2").unwrap(), []);
        assert_eq!(db.roads("nowhere").unwrap(), []);
    }

    #[test]
    fn building_a_house_makes_its_rooms() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);

        assert_eq!(db.hangout("hz-lr").unwrap().unwrap().name, "House hz Living Room");
        assert_eq!(db.hangout("hz-porch").unwrap().unwrap().name, "House hz Porch");
        assert_eq!(db.hangout("sq").unwrap().unwrap().name, "c1 Town Square");
        for room in ["hz-lr", "hz-porch"] {
            let people = db.people_in(room).unwrap();
            assert_eq!(people.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["p1"]);
        }
        assert_eq!(db.people_in("sq").unwrap().len(), 2);
        assert_eq!(db.residents("hz").unwrap().len(), 1);
        assert_eq!(db.resident_count("hz").unwrap(), 1);
        assert!(db.is_resident("hz", "p1").unwrap());
        assert!(!db.is_resident("hz", "p2").unwrap());

        db.create_hangout("plain", "Crew", "plain-code", "p1", 0).unwrap();
        for room in ["sq", "hz-lr", "hz-porch", "ha-lr"] {
            assert!(db.is_managed_room(room).unwrap(), "{room}");
        }
        assert!(!db.is_managed_room("plain").unwrap());
        assert!(!db.is_managed_room("nowhere").unwrap());
        assert_eq!(db.house_for_room("hz-porch").unwrap().unwrap().id, "hz");
        assert_eq!(db.house_for_room("hz-lr").unwrap().unwrap().id, "hz");
        assert!(db.house_for_room("sq").unwrap().is_none());
        assert_eq!(db.city_for_square("sq").unwrap().unwrap().id, "c1");
        assert!(db.city_for_square("hz-porch").unwrap().is_none());

        // Houses come back in the order they were built, whatever their ids sort like.
        let built: Vec<_> = db.houses_in_city("c1").unwrap().into_iter().map(|h| h.id).collect();
        assert_eq!(built, ["hz", "ha"]);
        assert_eq!(
            db.city_residents("c1").unwrap(),
            [("hz".to_string(), "p1".to_string()), ("ha".to_string(), "p2".to_string())]
        );
        assert_eq!(db.house_of_person_in_city("c1", "p2").unwrap().unwrap().id, "ha");
        assert!(db.house_of_person_in_city("c1", "p9").unwrap().is_none());
        assert_eq!(db.houses_for_person("p1").unwrap().len(), 1);

        assert_eq!(db.house_by_invite("hz-code").unwrap().unwrap().id, "hz");
        db.set_house_invite("hz", "fresh").unwrap();
        assert!(db.house_by_invite("hz-code").unwrap().is_none());
        assert_eq!(db.house_by_invite("fresh").unwrap().unwrap().id, "hz");

        // What serializes is what the city page may show: no invite code, no room ids.
        let json = serde_json::to_value(db.house("hz").unwrap().unwrap()).unwrap();
        assert_eq!(json, serde_json::json!({"id": "hz", "name": "House hz", "city_id": "c1"}));
        let json = serde_json::to_value(db.city("c1").unwrap().unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"id": "c1", "name": "c1", "square_id": "sq", "created_by": "p1"})
        );
    }

    #[test]
    fn a_city_that_fails_to_start_leaves_nothing_behind() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Ann", "key1", "feed1", 0).unwrap();
        city(&db, "c1", "sq", "p1");

        let clash = NewCity {
            id: "c2",
            name: "Elm",
            invite_code: "c1-code",
            square_id: "sq2",
            square_code: "sq2-code",
            created_by: "p1",
            now: 0,
        };
        assert!(db.create_city(&clash).is_err());
        assert!(db.hangout("sq2").unwrap().is_none());
        assert!(db.city("c2").unwrap().is_none());
        assert_eq!(db.cities_for_person("p1").unwrap().len(), 1);

        assert_eq!(db.city_by_invite("c1-code").unwrap().unwrap().id, "c1");
        db.set_city_invite("c1", "new-code").unwrap();
        assert!(db.city_by_invite("c1-code").unwrap().is_none());
        assert_eq!(db.city_by_invite("new-code").unwrap().unwrap().id, "c1");
    }

    #[test]
    fn joining_a_house_joins_the_city_and_leaving_empties_the_rooms() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        db.create_person("p3", "Cy", "key3", "feed3", 0).unwrap();
        bot(&db, "b3", "p3");

        db.add_resident("hz", "p3", 50).unwrap();
        db.add_resident("hz", "p3", 51).unwrap();
        assert!(db.is_city_member("c1", "p3").unwrap());
        for room in ["sq", "hz-lr", "hz-porch"] {
            assert!(db.is_member(room, "p3").unwrap(), "{room}");
        }
        assert_eq!(db.resident_count("hz").unwrap(), 2);
        assert!(db.add_resident("nowhere", "p3", 52).is_err());

        for room in ["sq", "hz-lr", "hz-porch"] {
            db.add_agent_to_room(room, "b3").unwrap();
        }
        db.set_bot_home("b3", "hz", false, false).unwrap();
        db.remove_resident("hz", "p3").unwrap();

        for room in ["hz-lr", "hz-porch"] {
            assert!(!db.is_member(room, "p3").unwrap(), "{room}");
            assert!(!db.agent_in_room(room, "b3", 60).unwrap(), "{room}");
        }
        assert!(db.bot_home("b3").unwrap().is_none());
        assert!(!db.is_resident("hz", "p3").unwrap());
        assert_eq!(db.resident_count("hz").unwrap(), 1);
        // Still in the city.
        assert!(db.is_city_member("c1", "p3").unwrap());
        assert!(db.is_member("sq", "p3").unwrap());
        assert!(db.agent_in_room("sq", "b3", 60).unwrap());
        // Other residents' bots stay where they were.
        assert!(db.agent_in_room("hz-lr", "b1", 60).unwrap());
        assert!(db.bot_home("b1").unwrap().is_some());
    }

    #[test]
    fn leaving_a_city_ends_their_bots_trips() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        db.add_agent_to_room("sq", "b2").unwrap();
        start_trip(&db, "t1", "b2", "hz", 60, 1000);
        agreed_trip(&db, "t2", "b2", "hz", 60, 1000);
        db.insert_pass(&NewPass {
            id: "ps1",
            house_id: "hz",
            agent_id: "b2",
            max_minutes: 60,
            granted_by: "p1",
            now: 1000,
        })
        .unwrap();
        start_trip(&db, "other", "b1", "ha", 60, 1000);

        db.remove_city_member("c1", "p2", "Kevin", 2000).unwrap();
        assert!(!db.is_city_member("c1", "p2").unwrap());
        assert!(!db.is_member("sq", "p2").unwrap());
        assert!(!db.agent_in_room("sq", "b2", 2000).unwrap());
        assert!(!db.agent_in_room("hz-porch", "b2", 2000).unwrap());
        let ended = db.trip("t1", 2000).unwrap().unwrap();
        assert_eq!((ended.status.as_str(), ended.ended_by.as_deref()), ("ended", Some("Kevin")));
        assert_eq!(db.trip("t2", 2000).unwrap().unwrap().status, "declined");
        assert!(db.pass("ps1").unwrap().is_none());
        // Nobody else's trip is touched.
        assert!(db.agent_in_room("ha-porch", "b1", 2000).unwrap());
        assert!(db.is_city_member("c1", "p1").unwrap());
    }

    #[test]
    fn a_bot_that_stays_home_keeps_one_room_and_loses_its_trip() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        db.add_agent_to_room("sq", "b2").unwrap();
        db.add_agent_to_room("ha-porch", "b2").unwrap();
        start_trip(&db, "t1", "b2", "hz", 60, 1000);
        agreed_trip(&db, "t2", "b2", "hz", 60, 1000);

        db.remove_agent_from_other_rooms("b2", "ha-lr").unwrap();
        assert_eq!(db.end_trips_of_bot("b2", "p2", 1500).unwrap(), 2);
        assert_eq!(room_ids(&db, "b2", 1500), ["ha-lr"]);
        assert_eq!(db.end_trips_of_bot("b2", "p2", 1500).unwrap(), 0);
        assert_eq!(db.trip("t2", 1500).unwrap().unwrap().status, "declined");
    }

    #[test]
    fn a_house_has_one_bot_and_it_is_in_no_rooms() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);

        assert!(db.create_house_bot("hz", "hb1", "p1", "Home", "token-hb1", 100).unwrap());
        assert!(!db.create_house_bot("hz", "hb2", "p1", "Again", "token-hb2", 101).unwrap());
        assert!(db.agent("hb2", 101).unwrap().is_none());
        assert_eq!(db.house_bot("hz", 102).unwrap().unwrap().id, "hb1");
        assert!(db.house_bot("ha", 102).unwrap().is_none());
        assert!(db.hangouts_for_agent("hb1", 102).unwrap().is_empty());
        let home = db.bot_home("hb1").unwrap().unwrap();
        assert_eq!((home.is_house, home.stays_home), (true, false));

        // Changing the home or the flag never turns a bot into the house bot, or back.
        db.set_bot_home("hb1", "hz", true, false).unwrap();
        let home = db.bot_home("hb1").unwrap().unwrap();
        assert_eq!((home.is_house, home.stays_home), (true, true));
        db.set_bot_home("b1", "hz", true, true).unwrap();
        let home = db.bot_home("b1").unwrap().unwrap();
        assert_eq!((home.is_house, home.stays_home), (false, true));
        assert_eq!(db.house_bot("hz", 102).unwrap().unwrap().id, "hb1");
    }

    #[test]
    fn the_maps_show_where_bots_live_and_where_they_are() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        db.touch_agent_active("b2", 1000).unwrap();
        assert!(db.create_house_bot("hz", "hb1", "p1", "Home", "token-hb1", 100).unwrap());
        start_trip(&db, "t1", "b2", "hz", 60, 1000);

        let city_map = db.bots_in_city("c1", 1500).unwrap();
        // House bots stay off the map: when one wakes would say when someone's home.
        let ids: Vec<_> = city_map.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["b1", "b2"]);
        let away = &city_map[1];
        assert_eq!(away.home_house_id.as_deref(), Some("ha"));
        assert_eq!(away.away_at_house_id.as_deref(), Some("hz"));
        assert_eq!(away.away_until, Some(4600));
        assert_eq!((away.state, away.stays_home, away.is_house), ("active", false, false));
        assert_eq!(city_map[0].away_at_house_id, None);
        // Once the trip runs out the bot is simply home.
        let home = db.bots_in_city("c1", 4600).unwrap();
        assert_eq!((home[1].away_at_house_id.clone(), home[1].away_until), (None, None));

        let house_map = db.bots_of_house("ha", 1500).unwrap();
        assert_eq!(house_map.len(), 1);
        assert_eq!(house_map[0].away_at_house_id.as_deref(), Some("hz"));
        let ids: Vec<_> = db.bots_of_house("hz", 1500).unwrap().into_iter().map(|b| b.id).collect();
        assert_eq!(ids, ["b1", "hb1"]);

        // Another city shows neither where these bots live nor where they're visiting.
        city(&db, "c2", "sq2", "p1");
        db.add_city_member("c2", "p2", 0).unwrap();
        let other = db.bots_in_city("c2", 1500).unwrap();
        assert_eq!(other.len(), 2);
        for bot in &other {
            assert_eq!((bot.home_house_id.clone(), bot.away_at_house_id.clone()), (None, None));
        }
    }

    #[test]
    fn a_scene_request_lapses_on_its_own_clock() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        assert!(db.insert_scene("s1", "hz", "Movie Night", 0).unwrap());
        assert!(!db.insert_scene("s2", "hz", "Movie Night", 1).unwrap());
        assert!(db.insert_scene("s3", "ha", "Movie Night", 1).unwrap());
        assert_eq!(db.scenes("hz").unwrap().len(), 1);

        let ask = |id: &str, status: &'static str, expires_at: i64, now: i64| {
            db.insert_scene_request(&NewSceneRequest {
                id,
                scene_id: "s1",
                house_id: "hz",
                asked_by: "b1",
                status,
                answered_by: None,
                expires_at,
                now,
            })
            .unwrap()
        };
        let status = |id: &str, now: i64| db.scene_request(id, now).unwrap().unwrap().status;

        // Waiting lapses at its time, and only one request per scene is open at once.
        assert!(ask("r1", "waiting", 900, 0));
        assert!(!ask("r2", "waiting", 950, 10));
        assert_eq!(db.open_scene_request("s1", 899).unwrap().unwrap().id, "r1");
        assert_eq!(status("r1", 899), "waiting");
        assert_eq!(status("r1", 900), "expired");
        assert!(db.open_scene_request("s1", 900).unwrap().is_none());
        assert!(!db.answer_scene_request("r1", "approved", "p1", Some(1500), 900).unwrap());

        // An approval gets a shorter life from when it was given, and the queue honours it.
        assert!(ask("r3", "waiting", 1500, 900));
        assert!(db.answer_scene_request("r3", "approved", "p1", Some(1100), 1000).unwrap());
        assert!(!db.answer_scene_request("r3", "declined", "p1", None, 1001).unwrap());
        assert_eq!(status("r3", 1099), "approved");
        assert_eq!(status("r3", 1100), "expired");
        assert_eq!(
            db.scene_queue("hz", 1099).unwrap(),
            [QueueItem { id: "r3".to_string(), scene: "Movie Night".to_string() }]
        );
        assert!(db.scene_queue("hz", 1100).unwrap().is_empty());
        assert!(db.scene_queue("ha", 1099).unwrap().is_empty());
        assert!(!db.finish_scene_request("r3", "done", 1100).unwrap());
        assert!(db.finish_scene_request("r3", "done", 1099).unwrap());
        assert_eq!(status("r3", 1099), "done");
        assert!(db.scene_queue("hz", 1099).unwrap().is_empty());

        // A resident asking for themselves is approved from the start, and a no is final.
        assert!(db.insert_scene_request(&NewSceneRequest {
            id: "r4",
            scene_id: "s1",
            house_id: "hz",
            asked_by: "p1",
            status: "approved",
            answered_by: Some("p1"),
            expires_at: 1700,
            now: 1100,
        })
        .unwrap());
        assert_eq!(db.scene_queue("hz", 1100).unwrap().len(), 1);
        assert!(db.finish_scene_request("r4", "failed", 1101).unwrap());
        assert!(ask("r5", "waiting", 2000, 1200));
        assert!(db.answer_scene_request("r5", "declined", "p1", None, 1201).unwrap());
        let declined = db.scene_request("r5", 1202).unwrap().unwrap();
        assert_eq!((declined.status.as_str(), declined.expires_at), ("declined", 2000));
        assert_eq!(declined.answered_by.as_deref(), Some("p1"));

        let recent = db.recent_scene_requests("hz", 1000, 3000).unwrap();
        let ids: Vec<_> = recent.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["r5", "r4"]);
        assert_eq!(db.recent_scene_requests("hz", 0, 3000).unwrap().len(), 4);

        // Removing a scene takes its requests with it.
        assert!(db.delete_scene("s1").unwrap());
        assert!(db.scene_request("r5", 1202).unwrap().is_none());
        assert!(!db.delete_scene("s1").unwrap());
    }

    #[test]
    fn a_house_status_is_ok_or_attention() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        assert_eq!(db.house_status("hz").unwrap(), None);
        db.set_house_status("hz", "ok", 10).unwrap();
        db.set_house_status("hz", "attention", 20).unwrap();
        let status = db.house_status("hz").unwrap().unwrap();
        assert_eq!((status.state.as_str(), status.updated_at), ("attention", 20));
        assert!(db.set_house_status("hz", "locked", 30).is_err());
        assert_eq!(db.house_status("ha").unwrap(), None);
    }

    #[test]
    fn trips_are_listed_for_the_house_the_bot_and_its_owner() {
        let db = Db::open(":memory:").unwrap();
        two_houses(&db);
        let now = 100_000;
        // Over, lapsed, still waiting from long ago, under way since long ago, and just asked.
        start_trip(&db, "old", "b2", "hz", 60, 100);
        agreed_trip(&db, "lapsed", "b2", "hz", 60, 100);
        agreed_trip(&db, "open-old", "b2", "hz", 60, 50_000);
        start_trip(&db, "stay", "b2", "hz", 7 * 24 * 60, 60_000);
        db.insert_trip(&NewTrip {
            id: "recent",
            agent_id: "b2",
            house_id: "hz",
            kind: "call",
            goal: "Say hi",
            minutes: 5,
            asked_by: "b2",
            owner_ok_by: None,
            host_ok_by: None,
            pass_id: None,
            expires_at: now + 100,
            now: now - 10,
        })
        .unwrap();

        let since = now - 60 * 60;
        let ids = |trips: Vec<Trip>| trips.into_iter().map(|t| t.id).collect::<Vec<_>>();
        // Old trips only show while they're open or under way, newest first.
        let listed = ["recent", "stay", "open-old"];
        assert_eq!(ids(db.trips_for_house("hz", since, now).unwrap()), listed);
        assert_eq!(ids(db.trips_for_house("hz", 0, now).unwrap()).len(), 5);
        assert_eq!(ids(db.trips_for_bot("b2", since, now).unwrap()), listed);
        assert_eq!(ids(db.trips_for_owner("p2", since, now).unwrap()), listed);
        assert!(db.trips_for_owner("p1", 0, now).unwrap().is_empty());
        assert!(db.trips_for_house("ha", 0, now).unwrap().is_empty());

        let recent = db.trip("recent", now).unwrap().unwrap();
        assert_eq!(recent.goal, "Say hi");
        assert_eq!((recent.agent_name.as_str(), recent.owner_id.as_str()), ("b2", "p2"));
        assert_eq!(recent.house_name, "House hz");
        assert_eq!(recent.status, "asking");
    }

    fn new_passkey<'a>(id: &'a str, credential_id: &'a str) -> NewPasskey<'a> {
        NewPasskey {
            id,
            credential_id,
            passkey: "{\"counter\":0}",
            label: "Phone",
        }
    }

    #[test]
    fn a_session_lasts_until_it_lapses_and_the_cap_ends_the_oldest() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Ann", "key1", "feed1", 0).unwrap();
        db.create_session("old", "p1", 100, 0).unwrap();
        assert_eq!(db.person_by_session("old", 99).unwrap().unwrap().name, "Ann");
        assert!(db.person_by_session("old", 100).unwrap().is_none(), "lapsed at its time");
        assert!(db.person_by_session("nope", 0).unwrap().is_none());

        // A new session clears the lapsed ones, and past the cap the oldest go first.
        for n in 0..MAX_SESSIONS_PER_PERSON + 5 {
            db.create_session(&format!("s{n}"), "p1", 10_000, 200 + n).unwrap();
        }
        assert_eq!(db.session_count("p1").unwrap(), MAX_SESSIONS_PER_PERSON);
        assert!(db.person_by_session("old", 0).unwrap().is_none());
        assert!(db.person_by_session("s0", 300).unwrap().is_none(), "the oldest went");
        assert!(db.person_by_session("s5", 300).unwrap().is_some());
        assert!(db.person_by_session(&format!("s{}", MAX_SESSIONS_PER_PERSON + 4), 300).unwrap().is_some());

        db.delete_session("s5").unwrap();
        assert!(db.person_by_session("s5", 300).unwrap().is_none());
    }

    #[test]
    fn replacing_the_owner_key_ends_every_session_and_keeps_passkeys() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Ann", "key1", "feed1", 0).unwrap();
        db.create_person("p2", "Bo", "key2", "feed2", 0).unwrap();
        db.user_handle("p1", "handle1").unwrap();
        db.add_passkey("p1", &new_passkey("k1", "c1"), 5, 0).unwrap();
        db.create_session("a", "p1", 1_000, 0).unwrap();
        db.create_session("b", "p1", 1_000, 0).unwrap();
        db.create_session("c", "p2", 1_000, 0).unwrap();

        db.replace_owner_key("p1", "key1-new").unwrap();
        assert!(db.person_by_key("key1").unwrap().is_none());
        assert_eq!(db.person_by_key("key1-new").unwrap().unwrap().id, "p1");
        assert_eq!(db.session_count("p1").unwrap(), 0);
        assert!(db.person_by_session("c", 1).unwrap().is_some(), "someone else's is untouched");
        assert_eq!(db.passkey_count("p1").unwrap(), 1);
    }

    #[test]
    fn a_person_keeps_one_handle_for_good() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Ann", "key1", "feed1", 0).unwrap();
        db.create_person("p2", "Bo", "key2", "feed2", 0).unwrap();
        assert_eq!(db.user_handle("p1", "first").unwrap(), "first");
        assert_eq!(db.user_handle("p1", "second").unwrap(), "first");
        assert_eq!(db.user_handle("p2", "other").unwrap(), "other");
    }

    #[test]
    fn a_credential_is_stored_once_for_anyone() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Ann", "key1", "feed1", 0).unwrap();
        db.create_person("p2", "Bo", "key2", "feed2", 0).unwrap();
        db.user_handle("p1", "h1").unwrap();
        db.user_handle("p2", "h2").unwrap();
        assert_eq!(
            db.add_passkey("p1", &new_passkey("k1", "cred"), 2, 5).unwrap(),
            PasskeyAdded::Added
        );
        assert_eq!(
            db.add_passkey("p2", &new_passkey("k2", "cred"), 2, 5).unwrap(),
            PasskeyAdded::AlreadyRegistered
        );
        assert_eq!(
            db.add_passkey("p1", &new_passkey("k3", "cred"), 2, 5).unwrap(),
            PasskeyAdded::AlreadyRegistered
        );
        assert_eq!(db.add_passkey("p1", &new_passkey("k4", "two"), 2, 6).unwrap(), PasskeyAdded::Added);
        assert_eq!(db.add_passkey("p1", &new_passkey("k5", "three"), 2, 7).unwrap(), PasskeyAdded::TooMany);

        let found = db.passkey_by_credential("cred").unwrap().unwrap();
        assert_eq!((found.id.as_str(), found.person_id.as_str(), found.user_handle.as_str()), ("k1", "p1", "h1"));
        assert!(db.passkey_by_credential("missing").unwrap().is_none());
        assert_eq!(db.credential_ids_of("p1").unwrap(), ["cred", "two"]);
        assert_eq!(db.passkeys_of("p2").unwrap(), []);

        // Only their own can be removed.
        assert!(!db.remove_passkey("p2", "k1", "").unwrap());
        assert!(db.remove_passkey("p1", "k1", "").unwrap());
        assert!(!db.remove_passkey("p1", "k1", "").unwrap());
        assert_eq!(db.passkey_count("p1").unwrap(), 1);
    }

    #[test]
    fn removing_a_passkey_ends_the_persons_other_sessions_and_nobody_elses() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Ann", "key1", "feed1", 0).unwrap();
        db.create_person("p2", "Bo", "key2", "feed2", 0).unwrap();
        db.user_handle("p1", "h1").unwrap();
        db.add_passkey("p1", &new_passkey("k1", "c1"), 5, 0).unwrap();
        db.add_passkey("p1", &new_passkey("k2", "c2"), 5, 0).unwrap();
        for (session, person) in [("a", "p1"), ("b", "p1"), ("c", "p2")] {
            db.create_session(session, person, 1_000, 0).unwrap();
        }

        // Not hers to remove: nothing is ended.
        assert!(!db.remove_passkey("p1", "nope", "a").unwrap());
        assert_eq!(db.session_count("p1").unwrap(), 2);

        assert!(db.remove_passkey("p1", "k1", "a").unwrap());
        assert!(db.person_by_session("a", 1).unwrap().is_some(), "the one that asked stays");
        assert!(db.person_by_session("b", 1).unwrap().is_none());
        assert!(db.person_by_session("c", 1).unwrap().is_some(), "someone else's is untouched");

        // An owner key made the request, so no session is the one to keep.
        assert!(db.remove_passkey("p1", "k2", "owner-key-hash").unwrap());
        assert_eq!(db.session_count("p1").unwrap(), 0);
    }

    #[test]
    fn a_sign_up_that_fails_leaves_no_account() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Ann", "key1", "feed1", 0).unwrap();
        db.user_handle("p1", "h1").unwrap();
        db.add_passkey("p1", &new_passkey("k1", "cred"), 5, 0).unwrap();

        let refused = db
            .create_account_with_passkey("p2", "Bo", "key2", "feed2", "h2", &new_passkey("k2", "cred"), 1)
            .unwrap();
        assert!(refused.is_none());
        assert_eq!(db.person_count().unwrap(), 1);
        assert!(db.person("p2").unwrap().is_none());
        assert!(db.user_handle("p2", "later").is_err(), "no handle was kept for the person who wasn't made");
    }

    #[test]
    fn a_sign_up_makes_the_account_the_handle_and_the_passkey_together() {
        let db = Db::open(":memory:").unwrap();
        let person = db
            .create_account_with_passkey("p1", "Ann", "key1", "feed1", "h1", &new_passkey("k1", "cred"), 3)
            .unwrap()
            .unwrap();
        assert_eq!(person.name, "Ann");
        assert_eq!(db.person_by_key("key1").unwrap().unwrap().id, "p1");
        assert_eq!(db.passkey_by_credential("cred").unwrap().unwrap().user_handle, "h1");
        assert_eq!(db.passkeys_of("p1").unwrap()[0].created_at, 3);
    }

    #[test]
    fn a_sign_in_only_lands_if_the_passkey_is_the_one_that_was_checked() {
        let db = Db::open(":memory:").unwrap();
        db.create_account_with_passkey("p1", "Ann", "key1", "feed1", "h1", &new_passkey("k1", "cred"), 3)
            .unwrap();
        let checked = db.passkey_by_credential("cred").unwrap().unwrap().passkey;

        // A passkey that moved on (its counter) while another sign-in was being checked is refused.
        assert!(db.record_sign_in("k1", &checked, Some("{\"counter\":5}"), 10).unwrap());
        assert!(!db.record_sign_in("k1", &checked, Some("{\"counter\":4}"), 11).unwrap());
        assert!(!db.record_sign_in("k1", &checked, None, 11).unwrap());
        let now = db.passkey_by_credential("cred").unwrap().unwrap().passkey;
        assert_eq!(now, "{\"counter\":5}");
        assert!(db.record_sign_in("k1", &now, None, 12).unwrap());
        assert_eq!(db.passkeys_of("p1").unwrap()[0].last_used_at, Some(12));
    }

    fn bot_player(db: &Db, agent: &str, owner: &str, token: &str, now: i64) -> PlayerRow {
        match db.join_as_bot(agent, owner, token, "commons", now).unwrap() {
            BotJoined::Done(player) => player,
            other => panic!("the bot should have joined: {other:?}"),
        }
    }

    #[test]
    fn replacing_a_bots_token_ends_its_players_token_and_the_owner_keeps_one_player_of_their_own() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Justin", "key", "feed", 0).unwrap();
        db.create_agent("a1", "p1", "Scout", "token", 0).unwrap();
        db.create_agent("a2", "p1", "Rover", "token2", 0).unwrap();
        let own = db.own_player("p1", 1).unwrap();
        assert_eq!(own.kind(), PlayerKind::Person);
        let player = bot_player(&db, "a1", "p1", "first", 2);
        bot_player(&db, "a2", "p1", "other", 2);
        assert_eq!(player.kind(), PlayerKind::Bot);
        assert!(db.player_by_token("first").unwrap().is_some());

        db.set_agent_token("a1", "new-token").unwrap();
        db.set_agent_token("a2", "new-token2").unwrap();
        assert!(db.player_by_token("first").unwrap().is_none());
        let after = db.player(player.id).unwrap().unwrap();
        assert_eq!((after.kind(), after.has_token, after.removed), (PlayerKind::Bot, false, false));
        // A bot's player with no token is never taken for the person's own.
        assert_eq!(db.own_player("p1", 3).unwrap().id, own.id);
        // Joining again brings back the same player with a token of its own.
        let back = bot_player(&db, "a1", "p1", "second", 4);
        assert_eq!(back.id, player.id);
        assert!(db.player_by_token("second").unwrap().is_some());
    }

    #[test]
    fn deleting_a_bot_or_its_person_ends_its_player_and_leaves_what_it_built() {
        let db = Db::open(":memory:").unwrap();
        for person in ["p1", "p2"] {
            db.create_person(person, person, &format!("key-{person}"), &format!("feed-{person}"), 0).unwrap();
        }
        db.create_agent("a1", "p1", "Scout", "token", 0).unwrap();
        db.create_agent("a2", "p2", "Rover", "token2", 0).unwrap();
        let first = bot_player(&db, "a1", "p1", "first", 2);
        let second = bot_player(&db, "a2", "p2", "second", 2);
        db.conn().execute("DELETE FROM agents WHERE id = 'a1'", []).unwrap();
        db.conn().execute("DELETE FROM people WHERE id = 'p2'", []).unwrap();
        for id in [first.id, second.id] {
            // The foreign key let go of the bot, which would have left a claimed player with a token.
            let after = db.player(id).unwrap().unwrap();
            assert!(after.agent_id.is_none() && after.removed, "player {id}: {after:?}");
        }
        assert!(db.player_by_token("first").unwrap().is_none());
        assert!(db.player_by_token("second").unwrap().is_none());
        assert!(db.players_of("p1").unwrap().is_empty());
    }

    #[test]
    fn a_bot_barred_from_a_world_is_refused_before_its_token_is_touched() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Justin", "key", "feed", 0).unwrap();
        db.create_agent("a1", "p1", "Scout", "token", 0).unwrap();
        let commons = NewWorld {
            id: "commons",
            name: "The Commons",
            kind: "public",
            seed: 1,
            size: 64,
            owner_id: None,
            open: true,
            invite_code: None,
            highway: &[],
            now: 1,
        };
        db.insert_world(&commons, 1).unwrap();
        let player = bot_player(&db, "a1", "p1", "first", 2);
        db.remove_from_world("commons", player.id, true, false).unwrap();
        assert!(matches!(db.join_as_bot("a1", "p1", "second", "commons", 3).unwrap(), BotJoined::Barred));
        assert!(db.player_by_token("first").unwrap().is_some(), "the token it had is as it was");
        assert!(db.player_by_token("second").unwrap().is_none());
        // Barred from one world is not barred from another.
        assert!(matches!(db.join_as_bot("a1", "p1", "third", "elsewhere", 3).unwrap(), BotJoined::Done(_)));
    }

    #[test]
    fn a_player_ended_by_the_person_who_claimed_it_keeps_what_it_built() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Ann", "key", "feed", 0).unwrap();
        db.create_person("p2", "Bo", "key2", "feed2", 0).unwrap();
        let guest = db.create_guest("token", "claim", 1).unwrap();
        let own = db.own_player("p1", 1).unwrap();
        // A guest nobody claimed, and a person's own player, can't be ended this way.
        assert!(!db.end_claimed_player(guest.id, "p1").unwrap());
        assert!(!db.end_claimed_player(own.id, "p1").unwrap());
        assert!(matches!(db.claim_player("claim", "p1", 20, 0).unwrap(), Claimed::Done(_)));
        assert!(!db.end_claimed_player(guest.id, "p2").unwrap(), "not Bo's");
        assert!(db.end_claimed_player(guest.id, "p1").unwrap());
        assert!(!db.end_claimed_player(guest.id, "p1").unwrap(), "only once");
        assert!(db.player_by_token("token").unwrap().is_none());
        assert!(db.player(guest.id).unwrap().is_some());
    }

    #[test]
    fn a_world_is_only_added_where_there_is_room_for_it() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Ann", "key", "feed", 0).unwrap();
        let new = |id, owner| NewWorld {
            id,
            name: "Quiet Harbour",
            kind: "own",
            seed: 1,
            size: 64,
            owner_id: owner,
            open: true,
            invite_code: None,
            highway: &[],
            now: 1,
        };
        assert_eq!(db.insert_world(&new("w1", Some("p1")), 1).unwrap(), WorldAdded::Added);
        assert_eq!(db.insert_world(&new("w2", Some("p1")), 2).unwrap(), WorldAdded::OwnerHasOne);
        db.create_person("p2", "Bo", "key2", "feed2", 0).unwrap();
        assert_eq!(db.insert_world(&new("w2", Some("p2")), 1).unwrap(), WorldAdded::Full);
        assert_eq!(db.insert_world(&new("w2", Some("p2")), 2).unwrap(), WorldAdded::Added);
    }

    #[test]
    fn every_new_player_starts_with_a_random_name_seed_of_its_own() {
        let db = Db::open(":memory:").unwrap();
        db.create_person("p1", "Ann", "key", "feed", 0).unwrap();
        db.create_agent("a1", "p1", "Scout", "token", 0).unwrap();
        let players = [
            db.create_guest("t1", "c1", 1).unwrap(),
            db.own_player("p1", 1).unwrap(),
            bot_player(&db, "a1", "p1", "t2", 1),
        ];
        for player in &players {
            assert_ne!(player.name_seed, player.id, "{player:?}");
            assert!(player.name_seed > 0);
        }
    }
}
