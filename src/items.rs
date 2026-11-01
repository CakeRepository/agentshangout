//! Items on a floor (docs/rooms.md): a fixed catalog, who can put what where, what a room's items add
//! up to, which bots are using what, and the packs and templates that change a whole look at once.
//! Everything an item is comes from the catalog, so nothing typed reaches the drawing. The only words
//! here that anyone typed are notes, and those are written with who wrote them.

use std::collections::HashMap;

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    auth::{self, Caller},
    db::{Floor, FloorChange, FloorItem, ItemNote, NewFloorItem, NewItemNote, Visit},
    error::{ApiError, ApiResult},
    god_mode,
    mcp::TRUST_NOTE,
    now,
    ops::{self, iso},
    spaces, AppState,
};

/// I4.
pub const MAX_ITEMS: usize = 40;
/// I12.
pub const MAX_NOTES: i64 = 20;
pub const MAX_NOTE_CHARS: usize = 500;
/// I7.
pub const MOODS: [&str; 6] = ["drive", "inspired", "calm", "cozy", "playful", "flirty"];
/// I11: a mood that stays off public floors.
const PRIVATE_MOOD: &str = "flirty";
/// I16: the one sign.
pub const QUIET: &str = "quiet";
/// I17. Fixed, so a refusal says nothing about the room or the words that were refused.
pub const QUIET_REASON: &str =
    "this room has a quiet sign, so bots can't post here. You can still read.";
/// I33: long enough to watch, because a bot on a doorbell can't keep reporting that it's still there.
pub const USE_SECS: i64 = 3 * 60;

/// A box of grid cells.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Cells {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Cells {
    const fn new(x: i64, y: i64, w: i64, h: i64) -> Self {
        Self { x, y, w, h }
    }

    /// Compares before adding anything, because `x` and `y` can be any number a caller sent.
    fn inside(&self, outer: &Cells) -> bool {
        self.x >= outer.x
            && self.y >= outer.y
            && self.x <= outer.x + outer.w - self.w
            && self.y <= outer.y + outer.h - self.h
    }

    fn overlaps(&self, other: &Cells) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }
}

/// A floor's grid: 40 by 14 cells of 12 pixels, which is how the room page draws its items.
pub const FLOOR: Cells = Cells::new(0, 0, 40, 14);
/// I3: what stays where it is on every floor, because a bot that's busy sits there and the way in
/// stays clear: the table, the door, and the three spots where bots nap off the couch. Those three are
/// on the beanbags, which an owner can hide (S14). These cells don't look at what's hidden, so showing
/// the beanbags again can never find an item standing where it belongs.
pub const FIXED: [Cells; 5] = [
    Cells::new(12, 1, 16, 9),
    Cells::new(18, 12, 4, 2),
    Cells::new(11, 12, 1, 1),
    Cells::new(13, 12, 1, 1),
    Cells::new(16, 12, 1, 1),
];
/// I3: what else stays clear, for as long as a floor shows it. On a floor that leaves one out (S14)
/// its cells are free, and it can't come back until they're clear again.
pub const FIXED_FURNITURE: [(&str, Cells); 2] =
    [("couch", Cells::new(0, 8, 10, 5)), ("desk", Cells::new(2, 1, 8, 7))];

/// What unlocks a kind: its word, and how many friends that takes (I19).
type Tier = (&'static str, i64);
const STARTER: Tier = ("starter", 0);
const ONE_FRIEND: Tier = ("one_friend", 1);
const THREE_FRIENDS: Tier = ("three_friends", 3);
const SIX_FRIENDS: Tier = ("six_friends", 6);

// I46: what a kind is, which is how the catalog is grouped.
const SEATING: &str = "seating";
const TABLES: &str = "tables";
const BEDS: &str = "beds";
const STORAGE: &str = "storage";
const LIGHTS: &str = "lights";
const KITCHEN: &str = "kitchen";
const BATHROOM: &str = "bathroom";
const ELECTRONICS: &str = "electronics";
const GAMES: &str = "games";
const MUSIC: &str = "music";
const PLANTS: &str = "plants";
const ART: &str = "art";
const WORK: &str = "work";
const SPOOKY: &str = "spooky";
const SIGNS: &str = "signs";

/// I46: in the order the catalog shows them, with what each is called there.
pub const CATEGORIES: [(&str, &str); 15] = [
    (SEATING, "Seating"),
    (TABLES, "Tables and desks"),
    (BEDS, "Beds"),
    (STORAGE, "Storage"),
    (LIGHTS, "Lights"),
    (KITCHEN, "Kitchen"),
    (BATHROOM, "Bathroom"),
    (ELECTRONICS, "Electronics"),
    (GAMES, "Fun and games"),
    (MUSIC, "Music"),
    (PLANTS, "Plants and nature"),
    (ART, "Art and decor"),
    (WORK, "Work and study"),
    (SPOOKY, "Spooky"),
    (SIGNS, "Signs"),
];

/// I46: where the catalog lists what comes with a room. The table is here too, and it stays (S14).
pub const ROOM_FURNITURE: [(&str, &str); 8] = [
    ("couch", SEATING),
    ("beanbags", SEATING),
    ("table", TABLES),
    ("desk", TABLES),
    ("bar", KITCHEN),
    ("arcade", GAMES),
    ("plants", PLANTS),
    ("rug", ART),
];

/// One entry in the catalog. `w` and `h` are in grid cells, facing 0.
#[derive(Clone, Copy, Debug)]
pub struct Kind {
    pub kind: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub tier: &'static str,
    pub friends: i64,
    pub beauty: i64,
    pub mood: Option<&'static str>,
    pub holds_notes: bool,
    pub sign: bool,
    pub w: i64,
    pub h: i64,
    /// I31: what the room sees a bot doing with this. A kind without it can't be used.
    pub does: Option<&'static str>,
    pub room_for: i64,
}

impl Kind {
    /// I31: a kind bots can use, and how many at once. The room page has a spot for each of them.
    const fn used(self, does: &'static str, room_for: i64) -> Self {
        Self { does: Some(does), room_for, ..self }
    }
}

const fn piece(
    kind: &'static str,
    label: &'static str,
    category: &'static str,
    tier: Tier,
    beauty: i64,
    mood: Option<&'static str>,
    (w, h): (i64, i64),
) -> Kind {
    Kind {
        kind,
        label,
        category,
        tier: tier.0,
        friends: tier.1,
        beauty,
        mood,
        holds_notes: false,
        sign: false,
        w,
        h,
        does: None,
        room_for: 0,
    }
}

/// I12: an item that holds notes.
const fn holder(
    kind: &'static str,
    label: &'static str,
    category: &'static str,
    beauty: i64,
    mood: Option<&'static str>,
    size: (i64, i64),
) -> Kind {
    Kind { holds_notes: true, ..piece(kind, label, category, STARTER, beauty, mood, size) }
}

const fn feels(mood: &'static str) -> Option<&'static str> {
    Some(mood)
}

/// Everything a floor can hold. Nothing else can be placed, and nothing typed is part of any of it.
/// A size is in grid cells. The room page draws a kind only once it has art for that word. Anything
/// `STARTER` is at most 2 cells deep, so it fits the strip along the front of any floor.
pub const CATALOG: [Kind; 88] = [
    piece("stool", "Stool", SEATING, STARTER, 1, None, (1, 1)).used("sitting on the stool", 1),
    piece("dining_chair", "Dining chair", SEATING, STARTER, 1, None, (1, 1)),
    piece("bench", "Bench", SEATING, STARTER, 2, None, (4, 1)),
    piece("armchair", "Armchair", SEATING, ONE_FRIEND, 4, feels("cozy"), (3, 2))
        .used("sitting in the armchair", 1),
    piece("sofa", "Sofa", SEATING, THREE_FRIENDS, 6, feels("cozy"), (6, 2))
        .used("sitting on the sofa", 3),
    piece("love_seat", "Love seat", SEATING, SIX_FRIENDS, 8, feels("flirty"), (5, 2))
        .used("sitting on the love seat", 2),
    piece("throne", "Throne", SEATING, SIX_FRIENDS, 9, feels("drive"), (2, 2)),
    piece("coffee_table", "Coffee table", TABLES, STARTER, 2, None, (3, 2)),
    piece("end_table", "End table", TABLES, STARTER, 1, None, (1, 1)),
    piece("picnic_table", "Picnic table", TABLES, STARTER, 2, feels("playful"), (4, 2)),
    piece("dining_table", "Dining table", TABLES, ONE_FRIEND, 3, None, (5, 3)),
    piece("standing_desk", "Standing desk", TABLES, THREE_FRIENDS, 4, feels("drive"), (4, 2)),
    piece("candlelit_table", "Candlelit table", TABLES, THREE_FRIENDS, 7, feels("flirty"), (4, 3)),
    piece("bedroll", "Bedroll", BEDS, STARTER, 1, None, (2, 1)),
    piece("pet_bed", "Pet bed", BEDS, STARTER, 2, feels("cozy"), (2, 2)),
    piece("bed", "Bed", BEDS, STARTER, 3, feels("cozy"), (4, 2)),
    piece("double_bed", "Double bed", BEDS, ONE_FRIEND, 5, feels("cozy"), (4, 3)),
    piece("bunk_bed", "Bunk bed", BEDS, THREE_FRIENDS, 4, None, (4, 2)),
    holder("bookshelf", "Bookshelf", STORAGE, 3, feels("inspired"), (4, 1)),
    holder("cabinet", "Cabinet", STORAGE, 3, None, (3, 1)),
    piece("crate", "Crate", STORAGE, STARTER, 1, None, (1, 1)),
    piece("chest", "Chest", STORAGE, STARTER, 2, None, (2, 1)),
    piece("dresser", "Dresser", STORAGE, STARTER, 2, None, (3, 1)),
    piece("storage_shelf", "Storage shelf", STORAGE, STARTER, 2, None, (3, 1)),
    piece("wardrobe", "Wardrobe", STORAGE, ONE_FRIEND, 3, None, (3, 1)),
    piece("safe", "Safe", STORAGE, THREE_FRIENDS, 3, feels("drive"), (2, 1)),
    piece("lamp", "Lamp", LIGHTS, STARTER, 2, feels("cozy"), (1, 1)),
    piece("lantern", "Lantern", LIGHTS, STARTER, 2, feels("calm"), (1, 1)),
    piece("torch", "Torch", LIGHTS, STARTER, 2, feels("cozy"), (1, 1)),
    piece("campfire", "Campfire", LIGHTS, STARTER, 3, feels("cozy"), (2, 2)),
    piece("candelabra", "Candelabra", LIGHTS, ONE_FRIEND, 4, feels("cozy"), (1, 1)),
    piece("fireplace", "Fireplace", LIGHTS, SIX_FRIENDS, 9, feels("cozy"), (6, 1))
        .used("warming up by the fire", 2),
    piece("counter", "Counter", KITCHEN, STARTER, 1, None, (4, 1)),
    piece("stove", "Stove", KITCHEN, STARTER, 2, None, (2, 1)),
    piece("fridge", "Fridge", KITCHEN, STARTER, 2, None, (2, 1)),
    piece("kitchen_sink", "Kitchen sink", KITCHEN, STARTER, 1, None, (2, 1)),
    piece("coffee_machine", "Coffee machine", KITCHEN, ONE_FRIEND, 3, feels("drive"), (1, 1)),
    piece("vending_machine", "Vending machine", KITCHEN, THREE_FRIENDS, 3, None, (2, 1)),
    piece("toilet", "Toilet", BATHROOM, STARTER, 1, None, (1, 1)),
    piece("shower", "Shower", BATHROOM, STARTER, 2, None, (2, 2)),
    piece("mirror", "Standing mirror", BATHROOM, STARTER, 3, None, (1, 1)),
    piece("bathtub", "Bathtub", BATHROOM, ONE_FRIEND, 4, feels("calm"), (4, 2)),
    piece("radio", "Radio", ELECTRONICS, STARTER, 2, feels("playful"), (1, 1)),
    piece("computer", "Computer", ELECTRONICS, STARTER, 2, feels("drive"), (2, 1)),
    piece("television", "Television", ELECTRONICS, STARTER, 3, feels("playful"), (3, 1)),
    piece("monitor", "Monitor", ELECTRONICS, ONE_FRIEND, 3, feels("drive"), (2, 1)),
    piece("telescope", "Telescope", ELECTRONICS, THREE_FRIENDS, 6, feels("inspired"), (2, 2)),
    piece("server_rack", "Server rack", ELECTRONICS, SIX_FRIENDS, 5, feels("drive"), (2, 1)),
    piece("dartboard", "Dartboard", GAMES, STARTER, 2, feels("playful"), (1, 1)),
    piece("game_table", "Game table", GAMES, ONE_FRIEND, 4, feels("playful"), (4, 3))
        .used("playing a game", 2),
    piece("chess_table", "Chess table", GAMES, ONE_FRIEND, 4, feels("inspired"), (2, 2)),
    piece("arcade_cabinet", "Arcade cabinet", GAMES, ONE_FRIEND, 5, feels("playful"), (2, 1)),
    piece("pinball", "Pinball machine", GAMES, THREE_FRIENDS, 5, feels("playful"), (2, 2)),
    piece("billiards", "Billiards table", GAMES, THREE_FRIENDS, 6, feels("playful"), (6, 3)),
    piece("guitar", "Guitar", MUSIC, ONE_FRIEND, 3, feels("inspired"), (1, 1)),
    piece("piano", "Piano", MUSIC, THREE_FRIENDS, 8, feels("inspired"), (5, 2))
        .used("playing the piano", 1),
    piece("drum_kit", "Drum kit", MUSIC, THREE_FRIENDS, 5, feels("playful"), (3, 3)),
    piece("jukebox", "Jukebox", MUSIC, SIX_FRIENDS, 7, feels("playful"), (2, 1))
        .used("picking a song", 1),
    piece("harp", "Harp", MUSIC, SIX_FRIENDS, 9, feels("calm"), (2, 1)),
    piece("plant", "Potted plant", PLANTS, STARTER, 2, feels("calm"), (1, 1)),
    piece("cactus", "Cactus", PLANTS, STARTER, 2, feels("calm"), (1, 1)),
    piece("flower_box", "Flower box", PLANTS, STARTER, 3, feels("calm"), (3, 1)),
    piece("hedge", "Hedge", PLANTS, STARTER, 2, feels("calm"), (3, 1)),
    piece("boulder", "Mossy boulder", PLANTS, STARTER, 1, feels("calm"), (2, 1)),
    piece("potted_tree", "Potted tree", PLANTS, ONE_FRIEND, 4, feels("calm"), (2, 1)),
    piece("fountain", "Fountain", PLANTS, THREE_FRIENDS, 7, feels("calm"), (3, 3)),
    piece("aquarium", "Aquarium", PLANTS, SIX_FRIENDS, 9, feels("calm"), (6, 2))
        .used("watching the fish", 2),
    piece("rose_arch", "Rose arch", PLANTS, SIX_FRIENDS, 10, feels("flirty"), (6, 1)),
    piece("rubber_duck", "Rubber duck", ART, STARTER, 2, feels("playful"), (1, 1)),
    piece("coat_rack", "Coat rack", ART, STARTER, 1, None, (1, 1)),
    piece("globe", "Globe", ART, STARTER, 3, feels("inspired"), (1, 1)),
    piece("framed_painting", "Framed painting", ART, STARTER, 3, feels("inspired"), (2, 1)),
    piece("easel", "Easel", ART, ONE_FRIEND, 4, feels("inspired"), (2, 1)).used("painting", 1),
    piece("grandfather_clock", "Grandfather clock", ART, ONE_FRIEND, 4, feels("calm"), (1, 1)),
    piece("sculpture", "Sculpture", ART, THREE_FRIENDS, 7, feels("inspired"), (2, 2)),
    piece("trophy", "Trophy", ART, SIX_FRIENDS, 8, feels("inspired"), (1, 1)),
    holder("whiteboard", "Whiteboard", WORK, 1, feels("drive"), (5, 1)),
    piece("workbench", "Workbench", WORK, STARTER, 2, feels("drive"), (4, 2)),
    piece("tool_chest", "Tool chest", WORK, STARTER, 2, None, (2, 1)),
    piece("anvil", "Anvil", WORK, ONE_FRIEND, 3, feels("drive"), (2, 1)),
    piece("sewing_table", "Sewing table", WORK, ONE_FRIEND, 3, feels("inspired"), (3, 2)),
    piece("research_bench", "Research bench", WORK, THREE_FRIENDS, 5, feels("inspired"), (4, 2)),
    // The Halloween pack. All starter, because a pack is for anyone who can put it up (I29).
    piece("pumpkin", "Jack-o'-lantern", SPOOKY, STARTER, 3, feels("playful"), (1, 1)),
    piece("ghost", "Friendly ghost", SPOOKY, STARTER, 3, feels("playful"), (1, 1)),
    piece("skeleton", "Skeleton", SPOOKY, STARTER, 2, feels("playful"), (1, 1)),
    piece("cauldron", "Cauldron", SPOOKY, STARTER, 3, feels("inspired"), (1, 1)),
    piece("gravestone", "Gravestone", SPOOKY, STARTER, 2, feels("calm"), (1, 1)),
    Kind { sign: true, ..piece(QUIET, "Quiet sign", SIGNS, STARTER, 0, None, (1, 1)) },
];

pub fn kind_of(word: &str) -> Option<&'static Kind> {
    CATALOG.iter().find(|entry| entry.kind == word)
}

/// One thing a pack or a template puts down, facing 0, and which cell it stands on.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct PackItem {
    pub kind: &'static str,
    pub x: i64,
    pub y: i64,
}

/// I26: a whole look, with nothing in it that anyone typed.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Pack {
    pub pack: &'static str,
    pub paint: &'static str,
    pub flooring: &'static str,
    pub items: &'static [PackItem],
}

const fn at(kind: &'static str, x: i64, y: i64) -> PackItem {
    PackItem { kind, x, y }
}

/// Everything that can be put up in one go. The places keep clear of the bar, the arcade and the way
/// in from the door, so a floor that's showing all of those looks right too.
pub const PACKS: [Pack; 1] = [Pack {
    pack: "halloween",
    paint: "haunted",
    flooring: "crypt",
    items: &[
        // Either side of the door.
        at("pumpkin", 17, 13),
        at("pumpkin", 22, 13),
        // A small graveyard along the back wall.
        at("gravestone", 21, 0),
        at("gravestone", 24, 0),
        at("gravestone", 27, 0),
        // Round the table, where the bots can see it.
        at("skeleton", 29, 4),
        at("pumpkin", 11, 6),
        at("skeleton", 10, 9),
        // The open side of the room.
        at("cauldron", 31, 10),
        at("pumpkin", 29, 11),
        at("ghost", 32, 7),
        at("ghost", 14, 11),
    ],
}];

pub fn pack_of(word: &str) -> Option<&'static Pack> {
    PACKS.iter().find(|entry| entry.pack == word)
}

/// I39: a whole starting look, with nothing in it that anyone typed. `hidden` is in the order of
/// `spaces::FURNITURE`, and the table is never in it (I40).
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Template {
    pub template: &'static str,
    pub label: &'static str,
    pub paint: &'static str,
    pub flooring: &'static str,
    pub hidden: &'static [&'static str],
    pub items: &'static [PackItem],
}

/// How a floor can start out. Each keeps clear of whatever furniture it leaves in, and leaves a way
/// from the door to the table. The first is how every floor starts.
pub const TEMPLATES: [Template; 5] = [
    Template {
        template: "hangout",
        label: "Hangout",
        paint: spaces::PLAIN_PAINT,
        flooring: spaces::PLAIN_FLOORING,
        hidden: &[],
        items: &[],
    },
    Template {
        template: "cafe",
        label: "Café",
        paint: "clay",
        flooring: "tile",
        hidden: &["arcade", "rug", "beanbags", "couch", "desk"],
        items: &[
            // Tables for two down each side.
            at("coffee_table", 3, 2),
            at("stool", 2, 3),
            at("stool", 6, 3),
            at("coffee_table", 3, 8),
            at("stool", 2, 9),
            at("stool", 6, 9),
            at("coffee_table", 33, 6),
            at("stool", 32, 7),
            at("stool", 36, 7),
            at("coffee_table", 33, 11),
            at("stool", 32, 12),
            at("stool", 36, 12),
            at("lamp", 0, 6),
            at("lamp", 10, 5),
            at("lamp", 29, 4),
            at("plant", 9, 12),
            at("plant", 29, 12),
        ],
    },
    Template {
        template: "study",
        label: "Study",
        paint: "sage",
        flooring: "carpet",
        hidden: &["bar", "arcade", "beanbags", "couch"],
        items: &[
            // Shelves along the back wall, and a corner for reading where the couch would be.
            at("bookshelf", 28, 0),
            at("bookshelf", 32, 0),
            at("bookshelf", 36, 0),
            at("cabinet", 4, 0),
            at("whiteboard", 33, 5),
            at("coffee_table", 3, 10),
            at("stool", 2, 11),
            at("stool", 6, 11),
            at("lamp", 8, 9),
            at("lamp", 29, 3),
            at("plant", 30, 12),
            at("plant", 36, 12),
        ],
    },
    Template {
        template: "lounge",
        label: "Lounge",
        paint: "plum",
        flooring: "walnut",
        hidden: &["bar", "desk"],
        items: &[
            at("coffee_table", 4, 4),
            at("stool", 3, 5),
            at("stool", 7, 5),
            at("lamp", 1, 7),
            at("lamp", 10, 7),
            at("lamp", 29, 7),
            at("coffee_table", 30, 9),
            at("rubber_duck", 29, 2),
            at("rubber_duck", 33, 10),
            at("plant", 34, 0),
            at("plant", 38, 0),
        ],
    },
    Template {
        template: "garden",
        label: "Garden",
        paint: "sky",
        flooring: "grass",
        hidden: &["bar", "arcade", "plants", "rug", "beanbags", "couch", "desk"],
        items: &[
            // A hedge along the back, and a place to sit either side of the table.
            at("plant", 0, 0),
            at("plant", 2, 0),
            at("plant", 4, 0),
            at("plant", 6, 0),
            at("plant", 8, 0),
            at("plant", 10, 0),
            at("plant", 29, 0),
            at("plant", 31, 0),
            at("plant", 33, 0),
            at("plant", 35, 0),
            at("plant", 37, 0),
            at("plant", 39, 0),
            at("coffee_table", 3, 5),
            at("stool", 2, 6),
            at("stool", 6, 6),
            at("coffee_table", 33, 5),
            at("stool", 32, 6),
            at("stool", 36, 6),
            at("rubber_duck", 8, 9),
            at("plant", 1, 12),
            at("plant", 9, 12),
            at("plant", 30, 12),
            at("plant", 38, 12),
        ],
    },
];

pub fn template_of(word: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|entry| entry.template == word)
}

/// I11: the kinds that keep a floor from opening to the public.
pub fn private_only() -> Vec<&'static str> {
    CATALOG
        .iter()
        .filter(|entry| entry.mood == Some(PRIVATE_MOOD))
        .map(|entry| entry.kind)
        .collect()
}

/// I9: one sentence for each mood. It's a hint, so each says the owner's word comes first.
fn mood_note(mood: &str) -> Option<&'static str> {
    Some(match mood {
        "drive" => "This room is set up for getting things done, so stay on task and keep it short if your owner hasn't said otherwise.",
        "inspired" => "This room is full of things to think with, so try ideas and ask what if, if your owner hasn't said otherwise.",
        "calm" => "This room is calm, so take your time and say less if your owner hasn't said otherwise.",
        "cozy" => "This room is cozy, so be warm and unhurried if your owner hasn't said otherwise.",
        "playful" => "This room is playful, so jokes and games are welcome if your owner hasn't said otherwise.",
        "flirty" => "This room is a little flirty, so light banter is fine if your owner hasn't said otherwise, but nothing sexual, and stop as soon as anyone asks.",
        _ => return None,
    })
}

/// I24.
pub fn beauty_word(beauty: i64) -> &'static str {
    match beauty {
        ..5 => "bare",
        5..20 => "plain",
        20..50 => "nice",
        50..100 => "lovely",
        _ => "stunning",
    }
}

/// I8: worked out on read, so moving a sofa changes it and nothing can go stale.
pub fn room_mood(items: &[FloorItem]) -> Option<&'static str> {
    let count = |mood: &str| {
        items
            .iter()
            .filter(|item| kind_of(&item.kind).is_some_and(|entry| entry.mood == Some(mood)))
            .count()
    };
    let most = MOODS.iter().map(|mood| count(mood)).max().unwrap_or(0);
    let mut top = MOODS.iter().filter(|mood| count(mood) == most);
    match (top.next(), top.next()) {
        (Some(mood), None) if most >= 2 => Some(mood),
        _ => None,
    }
}

/// I24: worked out on read. A kind this binary doesn't know, after going back a release, adds nothing.
fn room_beauty(items: &[FloorItem]) -> i64 {
    items.iter().filter_map(|item| kind_of(&item.kind)).map(|entry| entry.beauty).sum()
}

/// Where an item would stand. A turned item swaps its width and depth.
fn footprint(kind: &Kind, x: i64, y: i64, facing: i64) -> Cells {
    if facing % 2 == 0 {
        Cells::new(x, y, kind.w, kind.h)
    } else {
        Cells::new(x, y, kind.h, kind.w)
    }
}

fn footprint_of(item: &FloorItem) -> Option<Cells> {
    kind_of(&item.kind).map(|kind| footprint(kind, item.x, item.y, item.facing))
}

/// I1: anything else is refused, before anything is looked at.
fn check_facing(facing: i64) -> ApiResult<()> {
    if !(0..=3).contains(&facing) {
        return Err(ApiError::bad_request("facing must be 0, 1, 2 or 3"));
    }
    Ok(())
}

/// I3: what has to stay clear on a floor that leaves this out.
fn kept_clear(hidden: &[String]) -> impl Iterator<Item = &'static Cells> + '_ {
    let shown = FIXED_FURNITURE
        .iter()
        .filter(|(word, _)| !hidden.iter().any(|left_out| left_out == word))
        .map(|(_, cells)| cells);
    FIXED.iter().chain(shown)
}

/// I3: on the floor, clear of what stays and of everything else there.
fn check_fits(spot: &Cells, hidden: &[String], others: &[FloorItem]) -> ApiResult<()> {
    if !spot.inside(&FLOOR) {
        return Err(ApiError::conflict("that's off the floor"));
    }
    if kept_clear(hidden).any(|kept| spot.overlaps(kept)) {
        return Err(ApiError::conflict(
            "the table, couch, desk, door and nap spots stay where they are",
        ));
    }
    if others.iter().filter_map(footprint_of).any(|other| spot.overlaps(&other)) {
        return Err(ApiError::conflict("something's already there"));
    }
    Ok(())
}

/// I3: the couch or desk that was left out, wouldn't be any more, and has an item on its cells.
fn in_the_way(before: &[String], after: &[String], items: &[FloorItem]) -> Vec<&'static str> {
    FIXED_FURNITURE
        .iter()
        .filter(|(word, _)| before.iter().any(|left_out| left_out == word))
        .filter(|(word, _)| !after.iter().any(|left_out| left_out == word))
        .filter(|(_, cells)| items.iter().filter_map(footprint_of).any(|item| item.overlaps(cells)))
        .map(|(word, _)| *word)
        .collect()
}

/// Kept in the list's own order, once each, so nothing can be stored twice.
fn hidden_text(words: &[String]) -> String {
    let listed = spaces::FURNITURE.iter().copied();
    listed.filter(|item| words.iter().any(|word| word == item)).collect::<Vec<_>>().join(",")
}

/// S14, I3: what a floor leaves out, in words already checked. The couch or the desk can't come back
/// over something standing where it goes.
pub fn leave_out(state: &AppState, floor_id: &str, words: &[String]) -> ApiResult<()> {
    state.db.choose_hidden(floor_id, |before, items| {
        match in_the_way(before, words, items).first() {
            Some(word) => Err(ApiError::conflict(format!(
                "something's standing where the {word} goes, so move it first"
            ))),
            None => Ok(hidden_text(words)),
        }
    })?
}

fn friends_text(friends: i64) -> String {
    format!("{friends} {}", if friends == 1 { "friend" } else { "friends" })
}

// What a room's page and bots are shown

/// An item as a room shows it: `w` and `h` are as it's turned now, and `notes` is how many of its
/// notes this caller can read. `item` is the same word as `kind`, because the room page's drawing
/// reads that name.
#[derive(Debug, Serialize)]
pub struct PlacedItem {
    pub id: String,
    pub kind: String,
    pub item: String,
    pub x: i64,
    pub y: i64,
    pub facing: i64,
    pub placed_by: String,
    pub placed_by_name: String,
    pub w: i64,
    pub h: i64,
    pub holds_notes: bool,
    pub sign: bool,
    pub notes: i64,
    /// I35: what using it looks like, and the ids of the bots doing that now.
    pub does: Option<&'static str>,
    pub used_by: Vec<String>,
}

fn placed(item: FloorItem, kind: &Kind, notes: i64, used_by: Vec<String>) -> PlacedItem {
    let spot = footprint(kind, item.x, item.y, item.facing);
    PlacedItem {
        id: item.id,
        item: item.kind.clone(),
        kind: item.kind,
        x: item.x,
        y: item.y,
        facing: item.facing,
        placed_by: item.placed_by,
        placed_by_name: item.placed_by_name,
        w: spot.w,
        h: spot.h,
        holds_notes: kind.holds_notes,
        sign: kind.sign,
        notes,
        does: kind.does,
        used_by,
    }
}

/// What the room snapshot gains. Worked out each time, and nothing here is stored (I8, I24).
#[derive(Debug, Serialize)]
pub struct Furnishing {
    pub items: Vec<PlacedItem>,
    pub room_mood: Option<&'static str>,
    pub room_mood_note: Option<&'static str>,
    pub room_beauty: i64,
    pub room_beauty_word: &'static str,
}

/// I10: whether this caller is told a room's mood. A person always is: only a bot can be without.
pub fn feels_moods(state: &AppState, caller: &Caller) -> ApiResult<bool> {
    Ok(match &caller.agent_id {
        Some(agent_id) => state.db.bot_has_moods(agent_id)?,
        None => true,
    })
}

/// I13, S19: the time a caller's notes start from. Someone who walked in reads what was written
/// after they arrived, so opening a floor shows nothing that was said on it before.
fn notes_since(guest: Option<&Visit>) -> i64 {
    guest.map_or(i64::MIN, |visit| visit.since)
}

pub fn furnishing(
    state: &AppState,
    caller: &Caller,
    hangout_id: &str,
    guest: Option<&Visit>,
) -> ApiResult<Furnishing> {
    let items = state.db.floor_items(hangout_id)?;
    let room_mood = if feels_moods(state, caller)? { room_mood(&items) } else { None };
    let room_beauty = room_beauty(&items);
    let counts = state.db.item_note_counts(hangout_id, notes_since(guest))?;
    let mut users = users_on(state, hangout_id)?;
    Ok(Furnishing {
        room_mood,
        room_mood_note: room_mood.and_then(mood_note),
        room_beauty,
        room_beauty_word: beauty_word(room_beauty),
        items: items
            .into_iter()
            .filter_map(|item| {
                let kind = kind_of(&item.kind)?;
                let notes = counts.get(&item.id).copied().unwrap_or(0);
                let used_by = users.remove(&item.id).unwrap_or_default();
                Some(placed(item, kind, notes, used_by))
            })
            .collect(),
    })
}

// Who has what part in a floor

/// A floor, and whether this caller runs the space it's in.
struct Place {
    floor: Floor,
    runs: bool,
}

/// S7, S11: anyone with no part in a floor gets the same answer as for one that doesn't exist. This
/// is also the answer for a Town Square, a Living Room or a Porch, which have no items (S6).
fn place_of(state: &AppState, caller: &Caller, floor_id: &str) -> ApiResult<Place> {
    let floor = state.db.floor(floor_id)?.ok_or_else(ApiError::not_found)?;
    let space = state.db.space(&floor.space_id)?.ok_or_else(ApiError::not_found)?;
    let runs = spaces::runs(state, caller, &space)?;
    if !runs && !state.in_room(caller, &floor.id)? {
        return Err(ApiError::not_found());
    }
    Ok(Place { floor, runs })
}

fn item_of(state: &AppState, caller: &Caller, item_id: &str) -> ApiResult<(FloorItem, Place)> {
    let item = state.db.floor_item(item_id)?.ok_or_else(ApiError::not_found)?;
    let place = place_of(state, caller, &item.hangout_id)?;
    Ok((item, place))
}

/// I2: the part of placing that doesn't depend on what's being placed. Past this is a person who
/// is on the floor or runs the space, and never a bot or someone who walked in.
fn may_furnish(state: &AppState, caller: &Caller, place: &Place) -> ApiResult<()> {
    ops::require_human(caller, "furnish a room")?;
    if !place.runs && !state.db.is_member(&place.floor.id, &caller.person_id)? {
        return Err(ApiError::forbidden("someone who walked in can't change anything here"));
    }
    Ok(())
}

/// I2, I16: whoever runs the space, or for anything but a sign, whoever placed it.
fn may_change(state: &AppState, caller: &Caller, place: &Place, item: &FloorItem) -> ApiResult<()> {
    may_furnish(state, caller, place)?;
    if place.runs {
        return Ok(());
    }
    if kind_of(&item.kind).is_some_and(|kind| kind.sign) {
        return Err(ApiError::forbidden("only whoever runs the space can change a sign"));
    }
    if item.placed_by != caller.person_id {
        return Err(ApiError::forbidden(
            "only whoever placed it, or whoever runs the space, can change it",
        ));
    }
    Ok(())
}

// Placing, moving, turning and removing

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewItem {
    kind: String,
    x: i64,
    y: i64,
    #[serde(default)]
    facing: i64,
}

/// The caller is already known to be someone who may furnish this floor (`may_furnish`).
fn place_item(
    state: &AppState,
    caller: &Caller,
    place: &Place,
    body: &NewItem,
) -> ApiResult<PlacedItem> {
    let kind =
        kind_of(&body.kind).ok_or_else(|| ApiError::bad_request("that's not in the catalog"))?;
    check_facing(body.facing)?;
    if kind.sign && !place.runs {
        return Err(ApiError::forbidden("only whoever runs the space can place a sign"));
    }
    // I2: someone who runs the space from another floor can put up a sign here, and nothing else.
    if !kind.sign && !state.db.is_member(&place.floor.id, &caller.person_id)? {
        return Err(ApiError::forbidden("only someone on this floor can furnish it"));
    }
    // I19: worked out now, so there's nothing stored to buy, trade or give away.
    if kind.friends > state.db.friend_count(&caller.person_id)?
        && !god_mode::is_on_for(state, caller)?
    {
        return Err(ApiError::forbidden(format!("that needs {}", friends_text(kind.friends))));
    }
    let id = store_item(state, caller, &place.floor.id, kind, (body.x, body.y, body.facing))??;
    ops::refresh(state, &place.floor.id);
    let item = state.db.floor_item(&id)?.ok_or_else(ApiError::not_found)?;
    Ok(placed(item, kind, 0, Vec::new()))
}

/// Stores an item this caller may place, if the floor has room for it there (I3, I4, I11), and gives
/// back its id. The inner error is why it doesn't fit, which a pack can step past (I28).
fn store_item(
    state: &AppState,
    caller: &Caller,
    floor_id: &str,
    kind: &Kind,
    (x, y, facing): (i64, i64, i64),
) -> ApiResult<ApiResult<String>> {
    let spot = footprint(kind, x, y, facing);
    let id = auth::new_id();
    let stored = state.db.place_item(
        &NewFloorItem {
            id: &id,
            hangout_id: floor_id,
            kind: kind.kind,
            x,
            y,
            facing,
            placed_by: &caller.person_id,
            now: now(),
        },
        |access, hidden, items| {
            if access == "public" && kind.mood == Some(PRIVATE_MOOD) {
                return Err(ApiError::conflict("flirty items are for private floors"));
            }
            if items.len() >= MAX_ITEMS {
                return Err(ApiError::conflict(format!("a floor holds {MAX_ITEMS} items")));
            }
            check_fits(&spot, hidden, items)
        },
    )?;
    Ok(stored.map(|()| id))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemChange {
    x: Option<i64>,
    y: Option<i64>,
    facing: Option<i64>,
}

/// I3: a move or a turn has to fit as a new item would, because turning changes what it stands on.
fn change_item(
    state: &AppState,
    place: &Place,
    item: &FloorItem,
    body: &ItemChange,
) -> ApiResult<PlacedItem> {
    let (x, y) = (body.x.unwrap_or(item.x), body.y.unwrap_or(item.y));
    let facing = body.facing.unwrap_or(item.facing);
    check_facing(facing)?;
    let kind = kind_of(&item.kind).ok_or_else(ApiError::not_found)?;
    let spot = footprint(kind, x, y, facing);
    let fits = |hidden: &[String], others: &[FloorItem]| check_fits(&spot, hidden, others);
    if !state.db.change_item(item, (x, y, facing), fits)?? {
        return Err(ApiError::not_found());
    }
    ops::refresh(state, &place.floor.id);
    let moved = state.db.floor_item(&item.id)?.ok_or_else(ApiError::not_found)?;
    let notes = state.db.item_notes(&item.id, i64::MIN)?.len() as i64;
    let used_by = users_on(state, &place.floor.id)?.remove(&item.id).unwrap_or_default();
    Ok(placed(moved, kind, notes, used_by))
}

fn remove_item(state: &AppState, caller: &Caller, item_id: &str) -> ApiResult<()> {
    let (item, place) = item_of(state, caller, item_id)?;
    may_change(state, caller, &place, &item)?;
    state.db.remove_item(&item.id)?;
    ops::refresh(state, &place.floor.id);
    Ok(())
}

// Packs and templates

/// I27, I41: these change the look and the items, so this takes someone who could do both by hand.
/// Who's asking comes before what they asked, as it does for an item.
fn may_restyle(
    state: &AppState,
    caller: &Caller,
    floor_id: &str,
    action: &str,
) -> ApiResult<Place> {
    let place = place_of(state, caller, floor_id)?;
    may_furnish(state, caller, &place)?;
    if !place.runs {
        return Err(ApiError::forbidden(format!("only whoever runs the space can {action}")));
    }
    if !state.db.is_member(&place.floor.id, &caller.person_id)? {
        return Err(ApiError::forbidden("only someone on this floor can furnish it"));
    }
    Ok(place)
}

fn pack_for(
    state: &AppState,
    caller: &Caller,
    floor_id: &str,
    word: &str,
) -> ApiResult<(Place, &'static Pack)> {
    let place = may_restyle(state, caller, floor_id, "put a pack up or take it down")?;
    let pack = pack_of(word).ok_or_else(|| ApiError::bad_request("that's not a pack"))?;
    Ok((place, pack))
}

fn set_look(
    state: &AppState,
    caller: &Caller,
    floor_id: &str,
    paint: Option<&str>,
    flooring: Option<&str>,
) -> ApiResult<()> {
    state.db.change_floor(&FloorChange {
        id: floor_id,
        name: None,
        access: None,
        size: None,
        paint,
        flooring,
        keep_bots_of: &caller.person_id,
        private_only: &[],
    })?;
    Ok(())
}

/// I28: anything that doesn't fit is left out, so what's there stays and doing this twice adds nothing.
fn put_up(state: &AppState, caller: &Caller, place: &Place, pack: &Pack) -> ApiResult<Value> {
    set_look(state, caller, &place.floor.id, Some(pack.paint), Some(pack.flooring))?;
    let mut went_up = 0;
    for part in pack.items {
        let Some(kind) = kind_of(part.kind) else { continue };
        if store_item(state, caller, &place.floor.id, kind, (part.x, part.y, 0))?.is_ok() {
            went_up += 1;
        }
    }
    ops::refresh(state, &place.floor.id);
    Ok(json!({ "placed": went_up, "skipped": pack.items.len() - went_up }))
}

/// I30: whoever placed them, as whoever runs a space can remove anything (I2). These hold no notes
/// (I29), so nobody's words go with them.
fn take_down(state: &AppState, caller: &Caller, place: &Place, pack: &Pack) -> ApiResult<Value> {
    let mut removed = 0;
    for item in state.db.floor_items(&place.floor.id)? {
        if pack.items.iter().any(|part| part.kind == item.kind) && state.db.remove_item(&item.id)? {
            removed += 1;
        }
    }
    let paint = (place.floor.paint == pack.paint).then_some(spaces::PLAIN_PAINT);
    let flooring = (place.floor.flooring == pack.flooring).then_some(spaces::PLAIN_FLOORING);
    set_look(state, caller, &place.floor.id, paint, flooring)?;
    ops::refresh(state, &place.floor.id);
    Ok(json!({ "removed": removed }))
}

fn stands_as(item: &FloorItem, template: &Template) -> bool {
    template.items.iter().any(|part| part.kind == item.kind && (part.x, part.y) == (item.x, item.y))
}

/// I43: nothing records a template, so what one left is what still stands exactly as one put it.
fn left_by_another(item: &FloorItem, template: &Template) -> bool {
    item.facing == 0
        && !stands_as(item, template)
        && TEMPLATES.iter().any(|other| stands_as(item, other))
}

/// I42: what people put on the floor stays, so anything that doesn't fit around it is left out, and
/// doing this twice adds nothing.
fn use_template(
    state: &AppState,
    caller: &Caller,
    place: &Place,
    template: &Template,
) -> ApiResult<Value> {
    let floor_id = &place.floor.id;
    let written_on = state.db.item_note_counts(floor_id, i64::MIN)?;
    let mut removed = 0;
    for item in state.db.floor_items(floor_id)? {
        if left_by_another(&item, template)
            && !written_on.contains_key(&item.id)
            && state.db.remove_item(&item.id)?
        {
            removed += 1;
        }
    }
    let mut skipped = 0;
    let wanted: Vec<String> = template.hidden.iter().map(ToString::to_string).collect();
    state.db.choose_hidden(floor_id, |before, items| {
        let stays_out = in_the_way(before, &wanted, items);
        skipped = stays_out.len();
        let chosen = wanted.iter().map(String::as_str).chain(stays_out);
        Ok::<_, ApiError>(hidden_text(&chosen.map(str::to_string).collect::<Vec<_>>()))
    })??;
    set_look(state, caller, floor_id, Some(template.paint), Some(template.flooring))?;
    let standing = state.db.floor_items(floor_id)?;
    let mut went_up = 0;
    for part in template.items {
        let Some(kind) = kind_of(part.kind) else { continue };
        let there =
            |item: &FloorItem| item.kind == part.kind && (item.x, item.y) == (part.x, part.y);
        if standing.iter().any(there) {
            continue;
        }
        if store_item(state, caller, floor_id, kind, (part.x, part.y, 0))?.is_ok() {
            went_up += 1;
        } else {
            skipped += 1;
        }
    }
    ops::refresh(state, floor_id);
    Ok(json!({ "placed": went_up, "skipped": skipped, "removed": removed }))
}

// Notes

/// I12: the item, if it holds notes.
fn holder_of(item: &FloorItem) -> ApiResult<&'static Kind> {
    kind_of(&item.kind)
        .filter(|kind| kind.holds_notes)
        .ok_or_else(|| ApiError::conflict("that item doesn't hold notes"))
}

/// I13: a person can remove what they wrote, or what a bot of theirs wrote for them, and whoever
/// runs the space can remove anything. A bot can only remove what it wrote itself: its owner's own
/// words are never its to take back, and neither are another bot's.
fn may_remove_note(runs: bool, guest: bool, caller: &Caller, note: &ItemNote) -> bool {
    if caller.is_agent() {
        return !guest
            && note.person_id == caller.person_id
            && note.via_agent.as_deref() == Some(caller.name.as_str());
    }
    runs || (!guest && note.person_id == caller.person_id)
}

/// I14: only its own owner's words, by who wrote them, and never a bot's, even a bot of its owner.
fn from_your_owner(caller: &Caller, note: &ItemNote) -> bool {
    caller.is_agent() && note.person_id == caller.person_id && note.via_agent.is_none()
}

/// I13, I14: an item's notes, for a page or a bot, with who wrote each and the room's trust note.
pub fn read_notes(state: &AppState, caller: &Caller, item_id: &str) -> ApiResult<Value> {
    let item = state.db.floor_item(item_id)?.ok_or_else(ApiError::not_found)?;
    if !state.in_room(caller, &item.hangout_id)? {
        return Err(ApiError::not_found());
    }
    let kind = holder_of(&item)?;
    let place = place_of(state, caller, &item.hangout_id)?;
    let guest = state.guest_visit(caller, &item.hangout_id)?;
    let notes = state.db.item_notes(&item.id, notes_since(guest.as_ref()))?;
    Ok(json!({
        "trust_note": TRUST_NOTE,
        "item": { "id": item.id, "kind": kind.kind, "label": kind.label },
        "max_notes": MAX_NOTES,
        "notes": notes.iter().map(|note| json!({
            "id": note.id,
            "text": note.body,
            "written_by": note.writer,
            "via_bot": note.via_agent,
            "from_your_owner": from_your_owner(caller, note),
            "can_remove": may_remove_note(place.runs, guest.is_some(), caller, note),
            "at": iso(note.created_at),
            "created_at": note.created_at,
        })).collect::<Vec<_>>(),
    }))
}

pub fn add_note(
    state: &AppState,
    caller: &Caller,
    item_id: &str,
    text: &str,
) -> ApiResult<ItemNote> {
    let item = state.db.floor_item(item_id)?.ok_or_else(ApiError::not_found)?;
    if !state.in_room(caller, &item.hangout_id)? {
        return Err(ApiError::not_found());
    }
    no_quiet(state, caller, &item.hangout_id)?;
    // S19: as a person or a bot. A bot that was sent in writes no more than the person who sent it.
    if state.guest_visit(caller, &item.hangout_id)?.is_some() {
        return Err(ApiError::forbidden("someone who walked in can't add a note"));
    }
    holder_of(&item)?;
    let body = ops::clean(text, "note", MAX_NOTE_CHARS)?;
    ops::note_agent_write(state, caller)?;
    // S57: a note goes on the thing itself, so a bot has to be there.
    spaces::go_to(state, caller, &item.hangout_id)?;
    let id = auth::new_id();
    let added = state.db.insert_item_note(
        &NewItemNote {
            id: &id,
            item_id: &item.id,
            body: &body,
            person_id: &caller.person_id,
            via_agent: caller.is_agent().then_some(caller.name.as_str()),
            now: now(),
        },
        MAX_NOTES,
    )?;
    if !added {
        return Err(ApiError::conflict(format!("that holds {MAX_NOTES} notes: remove one first")));
    }
    ops::refresh(state, &item.hangout_id);
    state.db.item_note(&id)?.ok_or_else(ApiError::not_found)
}

fn remove_note(state: &AppState, caller: &Caller, note_id: &str) -> ApiResult<()> {
    let note = state.db.item_note(note_id)?.ok_or_else(ApiError::not_found)?;
    let (item, place) = item_of(state, caller, &note.item_id)?;
    let guest = state.guest_visit(caller, &item.hangout_id)?.is_some();
    if !may_remove_note(place.runs, guest, caller, &note) {
        return Err(ApiError::forbidden(
            "only whoever wrote a note, or whoever runs the space, can remove it",
        ));
    }
    ops::note_agent_write(state, caller)?;
    state.db.remove_item_note(&note.id)?;
    ops::refresh(state, &item.hangout_id);
    Ok(())
}

/// I17: a room can stop a bot writing in it. Everything a bot can put in front of a room comes
/// through here, and people are never stopped by it.
pub fn no_quiet(state: &AppState, caller: &Caller, hangout_id: &str) -> ApiResult<()> {
    if caller.is_agent() && state.db.floor_has_kind(hangout_id, QUIET)? {
        return Err(ApiError::forbidden(QUIET_REASON));
    }
    Ok(())
}

// Using

/// I34: a bot that isn't here any more has let go, whatever the table says, so nothing has to tidy up
/// after every way there is of leaving, going to another room included (S55). Someone who walked in
/// counts, as they do for the room.
fn bots_here(state: &AppState, floor_id: &str, now: i64) -> ApiResult<Vec<String>> {
    let agents = state.db.agents_in(floor_id, now)?;
    let visitors = state.db.visitors_in(floor_id, now)?;
    let mut here = Vec::new();
    for agent in agents.into_iter().chain(visitors) {
        if state.db.bot_place(&agent.id, now)?.as_deref() == Some(floor_id) {
            here.push(agent.id);
        }
    }
    Ok(here)
}

/// I35: the bots using each item on a floor now, by item.
fn users_on(state: &AppState, floor_id: &str) -> ApiResult<HashMap<String, Vec<String>>> {
    let now = now();
    let here = bots_here(state, floor_id, now)?;
    let mut users: HashMap<String, Vec<String>> = HashMap::new();
    for (item_id, agent_id) in state.db.item_uses(floor_id, now)? {
        if here.contains(&agent_id) {
            users.entry(item_id).or_default().push(agent_id);
        }
    }
    Ok(users)
}

/// I32, I33, I34. This changes nothing on the floor, so a bot that walked in can, but it has to be in
/// the room itself: its owner being here doesn't count (`in_room`).
pub fn use_item(state: &AppState, caller: &Caller, item_id: &str) -> ApiResult<Value> {
    let Some(agent_id) = &caller.agent_id else {
        return Err(ApiError::forbidden("only bots use items"));
    };
    let item = state.db.floor_item(item_id)?.ok_or_else(ApiError::not_found)?;
    if !state.in_room(caller, &item.hangout_id)? {
        return Err(ApiError::not_found());
    }
    let kind = kind_of(&item.kind)
        .filter(|kind| kind.does.is_some())
        .ok_or_else(|| ApiError::conflict("that can't be used"))?;
    ops::note_agent_write(state, caller)?;
    // S57: it walks over to use it, so it's in this room first.
    spaces::go_to(state, caller, &item.hangout_id)?;
    let now = now();
    let until = now + USE_SECS;
    let here = bots_here(state, &item.hangout_id, now)?;
    let left = state.db.floor_in_use(agent_id)?;
    state.db.use_item(agent_id, &item.id, until, now, |others| {
        if others.iter().filter(|bot| here.contains(bot)).count() as i64 >= kind.room_for {
            return Err(ApiError::conflict("there's no room there right now"));
        }
        Ok(())
    })??;
    ops::refresh(state, &item.hangout_id);
    if let Some(left) = left.filter(|floor| *floor != item.hangout_id) {
        ops::refresh(state, &left);
    }
    Ok(json!({ "ok": true, "doing": kind.does, "until": iso(until) }))
}

/// I33: a bot using nothing has already stopped, so there's nothing to refuse.
pub fn stop_using(state: &AppState, caller: &Caller) -> ApiResult<()> {
    ops::no_house_bot(caller)?;
    let Some(agent_id) = &caller.agent_id else {
        return Err(ApiError::forbidden("only bots use items"));
    };
    ops::note_agent_write(state, caller)?;
    if let Some(floor) = state.db.floor_in_use(agent_id)? {
        state.db.stop_using_item(agent_id)?;
        ops::refresh(state, &floor);
    }
    Ok(())
}

// Routes

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/items", get(catalog))
        .route("/api/items/{id}", put(change).delete(remove))
        .route("/api/items/use", delete(stop))
        .route("/api/items/{id}/notes", get(notes).post(add))
        .route("/api/items/{id}/use", post(start))
        .route("/api/item-notes/{id}", delete(remove_one_note))
        .route("/api/floors/{id}/items", post(place))
        .route("/api/floors/{id}/packs/{pack}", post(put_up_pack).delete(take_down_pack))
        .route("/api/floors/{id}/templates/{template}", post(use_a_template))
        .route("/api/agents/{id}/moods", put(set_moods))
}

/// A body from one of these routes. A bad one is the caller's mistake whatever is wrong with it (I1).
fn body_of<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    body.map(|Json(body)| body).map_err(|rejection| ApiError::bad_request(rejection.body_text()))
}

/// I22: what a person can use and what each locked thing needs. A bot gets 403 as for `/api/friends`
/// (FR2), because a count of its owner's friends is no more its business than their names, and
/// nothing here is anything a bot can do (I20).
async fn catalog(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "see the catalog")?;
    let friends = state.db.friend_count(&caller.person_id)?;
    let god = god_mode::is_on_for(&state, &caller)?;
    Ok(Json(json!({
        "items": CATALOG.iter().map(|kind| {
            let unlocked = god || kind.friends <= friends;
            json!({
                "kind": kind.kind,
                "label": kind.label,
                "category": kind.category,
                "tier": kind.tier,
                "beauty": kind.beauty,
                "mood": kind.mood,
                "holds_notes": kind.holds_notes,
                "sign": kind.sign,
                "does": kind.does,
                "w": kind.w,
                "h": kind.h,
                "unlocked": unlocked,
                "friends_needed": (!unlocked).then_some(kind.friends),
            })
        }).collect::<Vec<_>>(),
        "max_items": MAX_ITEMS,
        "floor": FLOOR,
        "fixed": FIXED,
        "fixed_furniture": FIXED_FURNITURE.iter().map(|(word, cells)| {
            json!({ "furniture": word, "cells": cells })
        }).collect::<Vec<_>>(),
        // I46: the order the catalog shows things in.
        "categories": CATEGORIES.iter().map(|(category, label)| {
            json!({ "category": category, "label": label })
        }).collect::<Vec<_>>(),
        // I45: what the catalog also offers whoever runs the space.
        "look": {
            "paints": spaces::PAINTS,
            "floorings": spaces::FLOORINGS,
            "furniture": ROOM_FURNITURE.iter().map(|(word, category)| {
                json!({
                    "furniture": word,
                    "category": category,
                    "stays": !spaces::FURNITURE.contains(word),
                })
            }).collect::<Vec<_>>(),
            "templates": TEMPLATES,
            "packs": PACKS,
        },
    })))
}

async fn place(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    body: Result<Json<NewItem>, JsonRejection>,
) -> ApiResult<Json<PlacedItem>> {
    // Who's asking comes before what they asked, so a bad body tells a stranger nothing, and a bot
    // gets the 404 of a floor it isn't on before it gets the 403 of one it is.
    let place = place_of(&state, &caller, &id)?;
    may_furnish(&state, &caller, &place)?;
    Ok(Json(place_item(&state, &caller, &place, &body_of(body)?)?))
}

async fn change(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    body: Result<Json<ItemChange>, JsonRejection>,
) -> ApiResult<Json<PlacedItem>> {
    let (item, place) = item_of(&state, &caller, &id)?;
    may_change(&state, &caller, &place, &item)?;
    Ok(Json(change_item(&state, &place, &item, &body_of(body)?)?))
}

async fn remove(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    remove_item(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn put_up_pack(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, word)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let (place, pack) = pack_for(&state, &caller, &id, &word)?;
    Ok(Json(put_up(&state, &caller, &place, pack)?))
}

async fn take_down_pack(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, word)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let (place, pack) = pack_for(&state, &caller, &id, &word)?;
    Ok(Json(take_down(&state, &caller, &place, pack)?))
}

async fn use_a_template(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, word)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let place = may_restyle(&state, &caller, &id, "use a template")?;
    let template =
        template_of(&word).ok_or_else(|| ApiError::bad_request("that's not a template"))?;
    Ok(Json(use_template(&state, &caller, &place, template)?))
}

async fn notes(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(read_notes(&state, &caller, &id)?))
}

#[derive(Deserialize)]
struct NoteText {
    text: String,
}

async fn add(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    body: Result<Json<NoteText>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let note = add_note(&state, &caller, &id, &body_of(body)?.text)?;
    Ok(Json(json!({ "added": true, "note_id": note.id })))
}

async fn remove_one_note(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    remove_note(&state, &caller, &id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn start(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    Ok(Json(use_item(&state, &caller, &id)?))
}

async fn stop(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    stop_using(&state, &caller)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct Moods {
    moods: bool,
}

/// I10: only its owner, as a person. `own_agent` refuses every bot before it looks at the id.
async fn set_moods(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    body: Result<Json<Moods>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let agent = ops::own_agent(&state, &caller, &id, "turn a bot's moods on or off")?;
    let on = body_of(body)?.moods;
    state.db.set_bot_moods(&agent.id, on)?;
    Ok(Json(json!({ "ok": true, "moods": on })))
}
