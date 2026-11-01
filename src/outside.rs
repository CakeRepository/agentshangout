//! The outside of a place (docs/outside.md): everything on its one tile, as a document of numbers and
//! listed words. This is the catalog of what an outside can be, the checks on one and who can change
//! it. Nothing typed is part of any of it, so W6 and W8 hold, and the page draws it from these words.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    auth::Caller,
    error::{ApiError, ApiResult},
    now, ops, spaces,
    worlds::{self, COMMONS_ID},
    AppState,
};

/// E3.
pub const MAX_COLOR: u32 = 0xff_ff_ff;
/// E13: a yard is this many cells each way, and a cell is 2 of the 32 units across a tile.
pub const GRID: i64 = 16;
/// E13.
pub const MIN_BODY: i64 = 5;
pub const MAX_FLOORS: i64 = 3;
/// E17.
pub const MAX_ITEMS: usize = 40;
/// E18.
pub const MAX_PETS: usize = 4;
/// E19.
pub const MAX_RULES: usize = 6;
/// E8.
pub const MAX_CHANGES_PER_MINUTE: u32 = 60;
/// E2: a refusal is this many characters at most. It can have a word in it that the caller typed, and
/// that is not worth echoing back at any length.
pub const MAX_REFUSAL_CHARS: usize = 160;
/// E27: how long the map of outsides is kept when nothing says it changed.
const MAP_KEPT: Duration = Duration::from_secs(10);

const BUILDINGS: &[&str] =
    &["house", "cafe", "club", "studio", "lounge", "bar", "office", "workshop", "library"];
const EVERYWHERE: &[&str] = &[
    "house", "cafe", "club", "studio", "lounge", "bar", "office", "workshop", "library", "square",
    "green",
];
/// E14: the places with no building, which take 2 by 2 tiles (W91).
const OPEN_AIR: &[&str] = &["square", "green"];

// What a thing is, which is how the catalog is grouped, as a room's is (I46).
const SIZE: &str = "size";
const WALLS: &str = "walls";
const ROOF: &str = "roof";
const WINDOWS: &str = "windows";
const DOORS: &str = "doors";
const LIGHTS: &str = "lights";
const FENCES: &str = "fences";
const GROUND: &str = "ground";
const TREES: &str = "trees";
const PLANTS: &str = "plants";
const FLOWERS: &str = "flowers";
const GARDEN: &str = "garden";
const WATER: &str = "water";
const SEATING: &str = "seating";
const PLAY: &str = "play";
const DECOR: &str = "decor";
const FALL: &str = "fall";
const SEASONAL: &str = "seasonal";
const PETS: &str = "pets";

/// In the order the catalog shows them, with what each is called there.
pub const GROUPS: [(&str, &str); 19] = [
    (SIZE, "Size"),
    (WALLS, "Walls"),
    (ROOF, "Roof"),
    (WINDOWS, "Windows"),
    (DOORS, "Doors and porches"),
    (LIGHTS, "Lights"),
    (FENCES, "Fences and gates"),
    (GROUND, "Ground and paths"),
    (TREES, "Trees"),
    (PLANTS, "Plants"),
    (FLOWERS, "Flowers"),
    (GARDEN, "Garden"),
    (WATER, "Water"),
    (SEATING, "Seating and shade"),
    (PLAY, "Play"),
    (DECOR, "Decor"),
    (FALL, "Fall"),
    (SEASONAL, "Seasonal"),
    (PETS, "Pets"),
];

/// E10: what a part's `number` means, and how far it goes.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Amount {
    pub label: &'static str,
    pub min: i64,
    pub max: i64,
}

/// E10: one thing about a place. `styles` are words and what the catalog calls them. With `many`,
/// they're an `on` list and any number can be on, and otherwise the part has one `style`.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Part {
    pub part: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub kinds: &'static [&'static str],
    pub styles: &'static [(&'static str, &'static str)],
    pub many: bool,
    pub colors: bool,
    pub number: Option<Amount>,
}

impl Part {
    const fn colored(self) -> Self {
        Self { colors: true, ..self }
    }

    const fn many(self) -> Self {
        Self { many: true, ..self }
    }

    const fn counted(self, label: &'static str, min: i64, max: i64) -> Self {
        Self { number: Some(Amount { label, min, max }), ..self }
    }
}

const fn part(
    part: &'static str,
    label: &'static str,
    group: &'static str,
    kinds: &'static [&'static str],
    styles: &'static [(&'static str, &'static str)],
) -> Part {
    Part { part, label, group, kinds, styles, many: false, colors: false, number: None }
}

const NONE: (&str, &str) = ("none", "None");

/// Everything about a place that isn't in its yard (E10). A new style is a line here and its art.
pub const PARTS: [Part; 25] = [
    part(
        "door_side",
        "Door side",
        DOORS,
        BUILDINGS,
        &[("auto", "Facing the road"), ("left", "On the left"), ("right", "On the right")],
    ),
    part(
        "walls",
        "Walls",
        WALLS,
        BUILDINGS,
        &[
            ("siding", "Siding"),
            ("brick", "Brick"),
            ("stucco", "Stucco"),
            ("stone", "Stone"),
            ("timber", "Timber boards"),
            ("logs", "Logs"),
            ("shingles", "Shingles"),
            ("steel", "Steel"),
            ("concrete", "Concrete"),
            ("tudor", "Tudor beams"),
            ("glass", "Glass"),
        ],
    )
    .colored(),
    part(
        "foundation",
        "Foundation",
        WALLS,
        BUILDINGS,
        &[NONE, ("stone", "Stone"), ("brick", "Brick"), ("raised", "Raised")],
    ),
    part(
        "trim",
        "Trim",
        WALLS,
        BUILDINGS,
        &[("plain", "Plain"), ("bold", "Bold"), ("gingerbread", "Gingerbread"), NONE],
    )
    .colored(),
    part(
        "roof",
        "Roof",
        ROOF,
        BUILDINGS,
        &[
            ("gable", "Gable"),
            ("cross_gable", "Cross gable"),
            ("hip", "Hip"),
            ("pyramid", "Pyramid"),
            ("flat", "Flat"),
            ("shed", "Shed"),
            ("mansard", "Mansard"),
            ("gambrel", "Gambrel"),
            ("dome", "Dome"),
            ("sawtooth", "Sawtooth"),
        ],
    )
    .colored()
    .counted("Pitch", 1, 3),
    part(
        "roofing",
        "Roofing",
        ROOF,
        BUILDINGS,
        &[
            ("shingles", "Shingles"),
            ("tiles", "Clay tiles"),
            ("slate", "Slate"),
            ("metal", "Metal seams"),
            ("thatch", "Thatch"),
            ("shakes", "Wood shakes"),
            ("moss", "Mossy"),
            ("solar", "Solar tiles"),
        ],
    ),
    part(
        "rooftop",
        "On the roof",
        ROOF,
        BUILDINGS,
        &[
            ("weathervane", "Weathervane"),
            ("antenna", "Antenna"),
            ("dish", "Satellite dish"),
            ("solar_panels", "Solar panels"),
            ("skylights", "Skylights"),
            ("dormers", "Dormers"),
            ("flag", "Flag"),
            ("ac_unit", "Air conditioner"),
            ("roof_garden", "Roof garden"),
            ("water_tank", "Water tank"),
        ],
    )
    .many()
    .colored(),
    part(
        "chimney",
        "Chimney",
        ROOF,
        BUILDINGS,
        &[
            NONE,
            ("brick", "Brick"),
            ("stone", "Stone"),
            ("metal", "Metal flue"),
            ("pipe", "Stove pipe"),
        ],
    )
    .colored(),
    part(
        "windows",
        "Windows",
        WINDOWS,
        BUILDINGS,
        &[
            ("square", "Square"),
            ("tall", "Tall"),
            ("wide", "Wide"),
            ("arched", "Arched"),
            ("round", "Round"),
            ("paned", "Many panes"),
            ("picture", "Picture window"),
            ("diamond", "Diamond panes"),
            ("stained", "Stained glass"),
        ],
    )
    .colored()
    .counted("On each wall", 0, 6),
    part(
        "shutters",
        "Shutters",
        WINDOWS,
        BUILDINGS,
        &[NONE, ("louvered", "Louvered"), ("board", "Board"), ("panel", "Panel")],
    )
    .colored(),
    part(
        "curtains",
        "Curtains",
        WINDOWS,
        BUILDINGS,
        &[
            NONE,
            ("open", "Drawn back"),
            ("closed", "Drawn"),
            ("blinds", "Blinds"),
            ("lace", "Lace"),
        ],
    )
    .colored(),
    part(
        "window_boxes",
        "Window boxes",
        WINDOWS,
        BUILDINGS,
        &[NONE, ("flowers", "Flowers"), ("herbs", "Herbs"), ("ivy", "Ivy"), ("mums", "Mums")],
    ),
    part(
        "door",
        "Door",
        DOORS,
        BUILDINGS,
        &[
            ("panel", "Panel"),
            ("glass", "Glass"),
            ("arched", "Arched"),
            ("double", "Double"),
            ("barn", "Barn"),
            ("round", "Round"),
            ("dutch", "Dutch"),
            ("sliding", "Sliding"),
        ],
    )
    .colored(),
    part(
        "porch",
        "Porch",
        DOORS,
        BUILDINGS,
        &[
            NONE,
            ("stoop", "Stoop"),
            ("covered", "Covered"),
            ("columns", "Columns"),
            ("deck", "Deck"),
            ("veranda", "Veranda"),
        ],
    )
    .colored(),
    part(
        "awning",
        "Awning",
        DOORS,
        BUILDINGS,
        &[
            NONE,
            ("plain", "Plain"),
            ("striped", "Striped"),
            ("scalloped", "Scalloped"),
            ("canopy", "Canopy"),
        ],
    )
    .colored(),
    part(
        "sign",
        "Sign",
        DOORS,
        BUILDINGS,
        &[
            NONE,
            ("heart", "Heart"),
            ("star", "Star"),
            ("note", "Music note"),
            ("cup", "Coffee cup"),
            ("moon", "Moon"),
            ("paw", "Paw"),
            ("leaf", "Leaf"),
            ("bolt", "Lightning"),
            ("flower", "Flower"),
            ("wreath", "Fall wreath"),
        ],
    )
    .colored(),
    part(
        "glow",
        "Window light",
        LIGHTS,
        BUILDINGS,
        &[("evening", "Most windows"), ("all", "Every window"), ("few", "A few"), ("off", "Dark")],
    )
    .colored(),
    part(
        "wall_lights",
        "Wall lights",
        LIGHTS,
        BUILDINGS,
        &[
            ("porch_lamp", "Porch lamp"),
            NONE,
            ("sconces", "Sconces"),
            ("uplights", "Uplights"),
            ("spotlights", "Spotlights"),
        ],
    )
    .colored(),
    part(
        "string_lights",
        "String lights",
        LIGHTS,
        EVERYWHERE,
        &[
            NONE,
            ("eaves", "Along the edge"),
            ("fairy", "Fairy lights"),
            ("festoon", "Festoon bulbs"),
            ("icicle", "Icicles"),
            ("neon", "Neon edge"),
            ("garland", "Leaf garland"),
        ],
    )
    .colored(),
    part(
        "fence",
        "Fence",
        FENCES,
        EVERYWHERE,
        &[
            NONE,
            ("picket", "Picket"),
            ("rail", "Post and rail"),
            ("hedge", "Hedge"),
            ("stone", "Stone wall"),
            ("brick", "Brick wall"),
            ("iron", "Wrought iron"),
            ("bamboo", "Bamboo"),
            ("rope", "Rope"),
            ("wattle", "Wattle"),
            ("glass", "Glass"),
        ],
    )
    .colored()
    .counted("Sides", 0, 15),
    part(
        "gate",
        "Gate",
        FENCES,
        EVERYWHERE,
        &[
            NONE,
            ("gap", "Just a gap"),
            ("wooden", "Wooden"),
            ("iron", "Iron"),
            ("arch", "Arch"),
            ("pillars", "Pillars"),
        ],
    )
    .colored(),
    part(
        "ground",
        "Ground",
        GROUND,
        EVERYWHERE,
        &[
            ("grass", "Grass"),
            ("clover", "Clover"),
            ("meadow", "Meadow"),
            ("dirt", "Dirt"),
            ("gravel", "Gravel"),
            ("sand", "Sand"),
            ("paving", "Paving"),
            ("cobbles", "Cobbles"),
            ("boards", "Decking"),
            ("moss", "Moss"),
            ("snow", "Snow"),
            ("tiles", "Tiles"),
            ("leaves", "Fallen leaves"),
        ],
    )
    .colored(),
    part(
        "path",
        "Path",
        GROUND,
        EVERYWHERE,
        &[
            NONE,
            ("flagstone", "Flagstone"),
            ("brick", "Brick"),
            ("gravel", "Gravel"),
            ("stepping_stones", "Stepping stones"),
            ("boards", "Boardwalk"),
            ("cobbles", "Cobbles"),
        ],
    )
    .colored(),
    part(
        "centerpiece",
        "Centrepiece",
        GROUND,
        OPEN_AIR,
        &[
            ("fountain", "Fountain"),
            ("statue", "Statue"),
            ("bandstand", "Bandstand"),
            ("clock", "Clock tower"),
            ("tree", "Old tree"),
            ("obelisk", "Obelisk"),
            NONE,
        ],
    )
    .colored(),
    part(
        "shape",
        "Shape",
        SIZE,
        BUILDINGS,
        &[
            ("plain", "Plain"),
            ("ell", "L-shaped"),
            ("tower", "Corner tower"),
            ("stepped", "Stepped"),
        ],
    ),
];

pub fn part_of(word: &str) -> Option<&'static Part> {
    PARTS.iter().find(|entry| entry.part == word)
}

/// E15: one thing that can stand in a yard. `w` and `h` are in cells, facing 0.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct YardKind {
    pub kind: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub w: i64,
    pub h: i64,
    pub tint: bool,
}

const fn thing(
    kind: &'static str,
    label: &'static str,
    group: &'static str,
    (w, h): (i64, i64),
) -> YardKind {
    YardKind { kind, label, group, w, h, tint: false }
}

/// E15: a kind that takes a `color`.
const fn paint(
    kind: &'static str,
    label: &'static str,
    group: &'static str,
    size: (i64, i64),
) -> YardKind {
    YardKind { tint: true, ..thing(kind, label, group, size) }
}

/// Everything that can stand in a yard. A new kind is a line here and its art in `web/`.
pub const YARD: [YardKind; 104] = [
    thing("oak", "Oak", TREES, (2, 2)),
    thing("pine", "Pine", TREES, (2, 2)),
    thing("birch", "Birch", TREES, (2, 2)),
    thing("maple", "Maple", TREES, (2, 2)),
    thing("cherry_blossom", "Cherry blossom", TREES, (2, 2)),
    thing("apple_tree", "Apple tree", TREES, (2, 2)),
    thing("palm", "Palm", TREES, (2, 2)),
    thing("willow", "Willow", TREES, (3, 3)),
    thing("cypress", "Cypress", TREES, (1, 1)),
    thing("dead_tree", "Bare tree", TREES, (2, 2)),
    paint("bush", "Bush", PLANTS, (1, 1)),
    paint("topiary", "Topiary", PLANTS, (1, 1)),
    paint("rose_bush", "Rose bush", PLANTS, (1, 1)),
    paint("hydrangea", "Hydrangea", PLANTS, (1, 1)),
    thing("fern", "Fern", PLANTS, (1, 1)),
    thing("tall_grass", "Tall grass", PLANTS, (1, 1)),
    thing("cactus", "Cactus", PLANTS, (1, 1)),
    thing("bamboo_clump", "Bamboo", PLANTS, (1, 1)),
    thing("mushroom_ring", "Mushroom ring", PLANTS, (2, 2)),
    paint("flower_bed", "Flower bed", FLOWERS, (2, 1)),
    paint("tulips", "Tulips", FLOWERS, (1, 1)),
    thing("sunflowers", "Sunflowers", FLOWERS, (1, 1)),
    thing("lavender", "Lavender", FLOWERS, (1, 1)),
    thing("wildflowers", "Wildflowers", FLOWERS, (2, 2)),
    thing("vegetable_patch", "Vegetable patch", GARDEN, (3, 2)),
    paint("planter", "Planter", GARDEN, (2, 1)),
    thing("compost_bin", "Compost bin", GARDEN, (1, 1)),
    paint("greenhouse", "Greenhouse", GARDEN, (4, 3)),
    paint("garden_shed", "Garden shed", GARDEN, (3, 3)),
    thing("beehive", "Beehive", GARDEN, (1, 1)),
    paint("chicken_coop", "Chicken coop", GARDEN, (3, 2)),
    paint("windmill", "Windmill", GARDEN, (2, 2)),
    thing("pond", "Pond", WATER, (3, 3)),
    thing("koi_pond", "Koi pond", WATER, (4, 3)),
    paint("fountain", "Fountain", WATER, (2, 2)),
    paint("bird_bath", "Bird bath", WATER, (1, 1)),
    thing("well", "Well", WATER, (2, 2)),
    paint("pool", "Pool", WATER, (5, 4)),
    paint("hot_tub", "Hot tub", WATER, (3, 3)),
    paint("bench", "Bench", SEATING, (2, 1)),
    paint("picnic_table", "Picnic table", SEATING, (3, 2)),
    paint("patio_set", "Patio set", SEATING, (2, 2)),
    paint("deck_chair", "Deck chair", SEATING, (1, 2)),
    paint("hammock", "Hammock", SEATING, (3, 1)),
    paint("porch_swing", "Swing seat", SEATING, (2, 1)),
    thing("fire_pit", "Fire pit", SEATING, (2, 2)),
    thing("bbq_grill", "Barbecue", SEATING, (1, 1)),
    paint("parasol", "Parasol", SEATING, (2, 2)),
    paint("gazebo", "Gazebo", SEATING, (4, 4)),
    paint("swing_set", "Swing set", PLAY, (3, 2)),
    paint("slide", "Slide", PLAY, (3, 1)),
    thing("sandbox", "Sandbox", PLAY, (2, 2)),
    paint("trampoline", "Trampoline", PLAY, (3, 3)),
    paint("seesaw", "Seesaw", PLAY, (3, 1)),
    thing("tire_swing", "Tire swing", PLAY, (1, 1)),
    thing("basketball_hoop", "Basketball hoop", PLAY, (1, 1)),
    paint("tent", "Tent", PLAY, (3, 2)),
    paint("lamp_post", "Lamp post", LIGHTS, (1, 1)),
    paint("lantern", "Lantern", LIGHTS, (1, 1)),
    paint("garden_light", "Garden light", LIGHTS, (1, 1)),
    thing("torch", "Torch", LIGHTS, (1, 1)),
    paint("string_post", "String light post", LIGHTS, (1, 1)),
    paint("spotlight", "Spotlight", LIGHTS, (1, 1)),
    thing("campfire", "Campfire", LIGHTS, (1, 1)),
    paint("mailbox", "Mailbox", DECOR, (1, 1)),
    paint("bird_house", "Bird house", DECOR, (1, 1)),
    paint("gnome", "Gnome", DECOR, (1, 1)),
    paint("flamingo", "Flamingo", DECOR, (1, 1)),
    paint("statue", "Statue", DECOR, (1, 1)),
    thing("sundial", "Sundial", DECOR, (1, 1)),
    thing("rock", "Rock", DECOR, (1, 1)),
    thing("boulder", "Boulder", DECOR, (2, 2)),
    thing("log_pile", "Log pile", DECOR, (2, 1)),
    paint("barrel", "Barrel", DECOR, (1, 1)),
    paint("flag_pole", "Flag pole", DECOR, (1, 1)),
    paint("bike", "Bike", DECOR, (2, 1)),
    paint("car", "Car", DECOR, (4, 2)),
    paint("boat", "Boat", DECOR, (4, 2)),
    paint("doghouse", "Doghouse", DECOR, (2, 2)),
    thing("maple_tree", "Red maple", FALL, (2, 2)),
    thing("birch_tree", "Golden birch", FALL, (2, 2)),
    thing("leaf_pile", "Leaf pile", FALL, (2, 1)),
    thing("rake", "Leaf rake", FALL, (1, 1)),
    paint("mums", "Potted mums", FALL, (1, 1)),
    thing("corn_stalks", "Corn stalks", FALL, (1, 1)),
    thing("harvest_table", "Harvest table", FALL, (4, 2)),
    thing("pumpkin_pie", "Pumpkin pie", FALL, (1, 1)),
    thing("cornucopia", "Cornucopia", FALL, (2, 1)),
    thing("apple_basket", "Apple basket", FALL, (1, 1)),
    thing("cider_barrel", "Cider barrel", FALL, (2, 1)),
    thing("gourds", "Pumpkins and gourds", FALL, (2, 1)),
    thing("scarecrow", "Scarecrow", FALL, (1, 1)),
    thing("hay_bale", "Hay bale", FALL, (2, 1)),
    thing("turkey", "Turkey", FALL, (1, 1)),
    thing("wreath", "Fall wreath", FALL, (1, 1)),
    paint("wheelbarrow", "Wheelbarrow", FALL, (2, 1)),
    thing("pumpkin", "Jack-o'-lantern", SEASONAL, (1, 1)),
    thing("gravestone", "Gravestone", SEASONAL, (1, 1)),
    thing("skeleton", "Skeleton", SEASONAL, (1, 1)),
    thing("ghost", "Friendly ghost", SEASONAL, (1, 1)),
    thing("cauldron", "Cauldron", SEASONAL, (1, 1)),
    thing("snowman", "Snowman", SEASONAL, (1, 1)),
    thing("christmas_tree", "Christmas tree", SEASONAL, (2, 2)),
    paint("gift_pile", "Presents", SEASONAL, (1, 1)),
];

pub fn yard_kind_of(word: &str) -> Option<&'static YardKind> {
    YARD.iter().find(|entry| entry.kind == word)
}

/// E18: what can live on a tile.
pub const SPECIES: [(&str, &str); 15] = [
    ("dog", "Dog"),
    ("cat", "Cat"),
    ("rabbit", "Rabbit"),
    ("chicken", "Chicken"),
    ("duck", "Duck"),
    ("pig", "Pig"),
    ("sheep", "Sheep"),
    ("goat", "Goat"),
    ("fox", "Fox"),
    ("hedgehog", "Hedgehog"),
    ("turtle", "Turtle"),
    ("parrot", "Parrot"),
    ("frog", "Frog"),
    ("robot_dog", "Robot dog"),
    ("turkey", "Turkey"),
];

/// E18, W7: names come from here, so nobody can type one.
pub const PET_NAMES: [&str; 60] = [
    "biscuit",
    "mochi",
    "pickle",
    "waffle",
    "pixel",
    "socks",
    "noodle",
    "peanut",
    "clover",
    "maple",
    "pepper",
    "ziggy",
    "bean",
    "olive",
    "tofu",
    "nugget",
    "sprout",
    "button",
    "cosmo",
    "dot",
    "fig",
    "ginger",
    "hazel",
    "juniper",
    "kiwi",
    "lentil",
    "marble",
    "nacho",
    "oats",
    "pancake",
    "quill",
    "radish",
    "sesame",
    "truffle",
    "umber",
    "velvet",
    "wren",
    "yam",
    "zest",
    "acorn",
    "bramble",
    "crumpet",
    "dumpling",
    "echo",
    "fennel",
    "gizmo",
    "honey",
    "inky",
    "jellybean",
    "kernel",
    "lint",
    "mittens",
    "nimbus",
    "orbit",
    "pudding",
    "rusty",
    "scout",
    "toast",
    "whiskers",
    "bumble",
];

/// E19, E20: what a pet's rule can ask about, and what the catalog calls it. Nothing here can ask who's
/// inside a place or whether anyone's home (W59).
pub const WHEN: [(&str, &str); 13] = [
    ("always", "Always"),
    ("morning", "In the morning"),
    ("daytime", "In the daytime"),
    ("evening", "In the evening"),
    ("night", "At night"),
    ("someone_near", "When anyone's near"),
    ("person_near", "When a person's near"),
    ("bot_near", "When a bot's near"),
    ("frog_near", "When a frog's near"),
    ("street_empty", "When the street's empty"),
    ("owner_watching", "When you're watching"),
    ("disaster", "When a disaster comes"),
    ("now_and_then", "Now and then"),
];

/// E19: what a pet's rule can have it do.
pub const DO: [(&str, &str); 12] = [
    ("wander", "Wander the yard"),
    ("patrol", "Walk the fence"),
    ("guard", "Guard the door"),
    ("sit", "Sit"),
    ("sleep", "Sleep"),
    ("play", "Play"),
    ("dig", "Dig"),
    ("chase", "Chase them"),
    ("greet", "Say hello"),
    ("fuss", "Make a fuss"),
    ("dance", "Dance"),
    ("hide", "Hide round the back"),
];

/// E3: suggestions. Any colour goes.
pub const SWATCHES: [u32; 24] = [
    0xff_ff_ff, 0xf7_f1_e3, 0xf2_d3_a0, 0xd9_b4_8f, 0xa9_57_3f, 0x7b_4a_2a, 0x3e_27_23, 0xe5_48_4d,
    0xff_8a_65, 0xff_b7_4d, 0xff_d5_4f, 0xc5_e1_a5, 0x6f_9d_3d, 0x2f_7a_58, 0x4d_b6_ac, 0x8f_cb_ec,
    0x2f_5d_9a, 0x1a_23_7e, 0xb3_9d_db, 0x6b_3f_86, 0xf4_8f_b1, 0x9e_9e_9e, 0x5d_64_74, 0x21_21_21,
];

// The document (E1)

/// E10: what's chosen for one part.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Chosen {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<Vec<String>>,
}

/// E13: where a building stands in its yard, in cells.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub d: i64,
    pub floors: i64,
}

fn is_unturned(facing: &i64) -> bool {
    *facing == 0
}

/// E15. A facing of 0 is left out, as it is when it's not sent.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct YardItem {
    pub kind: String,
    pub x: i64,
    pub y: i64,
    #[serde(default, skip_serializing_if = "is_unturned")]
    pub facing: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<i64>,
}

/// E19.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub when: String,
    #[serde(rename = "do")]
    pub action: String,
}

/// E18.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Pet {
    pub species: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coat: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collar: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
}

/// E1.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Outside {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parts: BTreeMap<String, Chosen>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Body>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<YardItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pets: Vec<Pet>,
}

/// E23: a whole look, with nothing in it that anyone typed.
#[derive(Clone, Debug, Serialize)]
pub struct Template {
    pub template: &'static str,
    pub label: &'static str,
    pub kinds: &'static [&'static str],
    pub outside: Value,
}

fn template(
    template: &'static str,
    label: &'static str,
    kinds: &'static [&'static str],
    outside: Value,
) -> Template {
    Template { template, label, kinds, outside }
}

/// E33: decorations that go up and come down in one go, with nothing in them that anyone typed.
#[derive(Clone, Debug, Serialize)]
pub struct Pack {
    pub pack: &'static str,
    pub label: &'static str,
    pub outside: Value,
}

/// E33: what can be put up. The places keep clear of the building of every kind and of a town square's
/// middle, and of the path from every kind's door: x 4 to 10 along the front, and y 6 to 10 down the side.
pub fn packs() -> Vec<Pack> {
    vec![Pack {
        pack: "fall",
        label: "Fall",
        outside: json!({
            "parts": {
                "ground": { "style": "leaves" },
                "string_lights": { "style": "garland", "color": 0xff_d5_4f },
                "window_boxes": { "style": "mums" },
                "sign": { "style": "wreath" },
            },
            "items": [
                // The trees, standing up behind the roof.
                { "kind": "maple_tree", "x": 1, "y": 1 },
                { "kind": "birch_tree", "x": 13, "y": 1 },
                // The harvest, on the left of the front.
                { "kind": "scarecrow", "x": 1, "y": 13 },
                { "kind": "corn_stalks", "x": 3, "y": 13 },
                { "kind": "wheelbarrow", "x": 0, "y": 14 },
                { "kind": "gourds", "x": 0, "y": 15 },
                { "kind": "hay_bale", "x": 2, "y": 15 },
                // Somewhere for dinner, on the right of the front, with the food in front of it.
                { "kind": "harvest_table", "x": 12, "y": 13 },
                { "kind": "cornucopia", "x": 2, "y": 14 },
                { "kind": "pumpkin_pie", "x": 13, "y": 15 },
                { "kind": "turkey", "x": 14, "y": 15 },
                { "kind": "mums", "x": 15, "y": 15 },
                // Down the side, clear of a door on that wall.
                { "kind": "leaf_pile", "x": 14, "y": 11 },
                { "kind": "rake", "x": 14, "y": 12 },
                { "kind": "cider_barrel", "x": 14, "y": 4 },
                { "kind": "apple_basket", "x": 14, "y": 5 },
                { "kind": "wreath", "x": 15, "y": 5 },
            ],
        }),
    }]
}

/// E23: every way a place can start out. E24 has each of them pass the checks on an empty tile.
pub fn templates() -> Vec<Template> {
    vec![
        template(
            "cottage",
            "Cottage",
            BUILDINGS,
            json!({
                "parts": {
                    "walls": { "style": "stucco", "color": 0xf2_e6_cf },
                    "roof": { "style": "gable", "color": 0xc8_a8_5a, "number": 3 },
                    "roofing": { "style": "thatch" },
                    "trim": { "style": "plain", "color": 0x6b_4a_2f },
                    "door": { "style": "arched", "color": 0x2f_7a_58 },
                    "windows": { "style": "paned", "number": 2 },
                    "shutters": { "style": "board", "color": 0x2f_7a_58 },
                    "window_boxes": { "style": "flowers" },
                    "chimney": { "style": "stone" },
                    "fence": { "style": "picket", "color": 0xf7_f1_e3, "number": 15 },
                    "gate": { "style": "wooden", "color": 0xf7_f1_e3 },
                    "path": { "style": "flagstone" },
                    "ground": { "style": "clover" },
                },
                "items": [
                    { "kind": "apple_tree", "x": 1, "y": 13 },
                    { "kind": "rose_bush", "x": 13, "y": 13, "color": 0xe5_48_4d },
                    { "kind": "flower_bed", "x": 11, "y": 14, "color": 0xf4_8f_b1 },
                    { "kind": "flower_bed", "x": 14, "y": 3, "color": 0xff_d5_4f, "facing": 1 },
                ],
            }),
        ),
        template(
            "modern",
            "Modern",
            BUILDINGS,
            json!({
                "parts": {
                    "walls": { "style": "concrete", "color": 0xe8_e8_e4 },
                    "roof": { "style": "flat", "color": 0x5d_64_74 },
                    "trim": { "style": "none" },
                    "door": { "style": "glass", "color": 0x21_21_21 },
                    "windows": { "style": "picture", "number": 2 },
                    "wall_lights": { "style": "uplights", "color": 0xff_f4_d6 },
                    "fence": { "style": "glass", "color": 0x8f_cb_ec, "number": 6 },
                    "gate": { "style": "gap" },
                    "ground": { "style": "gravel", "color": 0xc9_c5_bd },
                    "path": { "style": "stepping_stones" },
                },
                "items": [
                    { "kind": "cypress", "x": 1, "y": 13 },
                    { "kind": "cypress", "x": 1, "y": 14 },
                    { "kind": "deck_chair", "x": 14, "y": 11, "color": 0xff_ff_ff },
                ],
            }),
        ),
        template(
            "cabin",
            "Cabin",
            BUILDINGS,
            json!({
                "parts": {
                    "walls": { "style": "logs", "color": 0x8a_5a_36 },
                    "roof": { "style": "gable", "color": 0x4f_5a_4a, "number": 2 },
                    "roofing": { "style": "shakes" },
                    "door": { "style": "barn", "color": 0x5a_3a_22 },
                    "windows": { "style": "square", "number": 1 },
                    "chimney": { "style": "stone" },
                    "porch": { "style": "deck", "color": 0x8a_5a_36 },
                    "fence": { "style": "rail", "color": 0x7b_4a_2a, "number": 15 },
                    "gate": { "style": "gap" },
                    "ground": { "style": "meadow" },
                    "path": { "style": "gravel" },
                },
                "items": [
                    { "kind": "pine", "x": 1, "y": 1 },
                    { "kind": "pine", "x": 14, "y": 1 },
                    { "kind": "log_pile", "x": 1, "y": 14 },
                    { "kind": "fire_pit", "x": 14, "y": 3 },
                ],
            }),
        ),
        template(
            "seaside",
            "Seaside",
            BUILDINGS,
            json!({
                "parts": {
                    "walls": { "style": "siding", "color": 0xa8_d8_ea },
                    "roof": { "style": "hip", "color": 0xf7_f1_e3, "number": 1 },
                    "roofing": { "style": "tiles" },
                    "trim": { "style": "bold", "color": 0xff_ff_ff },
                    "door": { "style": "dutch", "color": 0xff_8a_65 },
                    "awning": { "style": "striped", "color": 0x2f_5d_9a },
                    "porch": { "style": "veranda", "color": 0xff_ff_ff },
                    "ground": { "style": "sand" },
                    "path": { "style": "boards" },
                },
                "items": [
                    { "kind": "palm", "x": 1, "y": 13 },
                    { "kind": "deck_chair", "x": 15, "y": 13, "color": 0xff_8a_65 },
                    { "kind": "parasol", "x": 14, "y": 11, "color": 0x2f_5d_9a },
                    { "kind": "boat", "x": 11, "y": 14, "color": 0xff_ff_ff },
                ],
            }),
        ),
        template(
            "victorian",
            "Victorian",
            BUILDINGS,
            json!({
                "parts": {
                    "walls": { "style": "shingles", "color": 0xb3_9d_db },
                    "roof": { "style": "mansard", "color": 0x5d_64_74, "number": 3 },
                    "roofing": { "style": "slate" },
                    "trim": { "style": "gingerbread", "color": 0xf7_f1_e3 },
                    "door": { "style": "double", "color": 0x6b_3f_86 },
                    "windows": { "style": "tall", "number": 2 },
                    "curtains": { "style": "lace", "color": 0xff_ff_ff },
                    "porch": { "style": "columns", "color": 0xf7_f1_e3 },
                    "shape": { "style": "tower" },
                    "fence": { "style": "iron", "color": 0x21_21_21, "number": 15 },
                    "gate": { "style": "iron", "color": 0x21_21_21 },
                    "path": { "style": "brick" },
                },
                "body": { "x": 3, "y": 3, "w": 10, "d": 9, "floors": 2 },
                "items": [
                    { "kind": "topiary", "x": 1, "y": 13, "color": 0x2f_7a_58 },
                    { "kind": "topiary", "x": 13, "y": 13, "color": 0x2f_7a_58 },
                    { "kind": "lamp_post", "x": 14, "y": 2, "color": 0x21_21_21 },
                ],
            }),
        ),
        template(
            "farmhouse",
            "Farmhouse",
            BUILDINGS,
            json!({
                "parts": {
                    "walls": { "style": "timber", "color": 0xb8_3a_2c },
                    "roof": { "style": "gambrel", "color": 0x5d_64_74, "number": 2 },
                    "roofing": { "style": "metal" },
                    "trim": { "style": "bold", "color": 0xff_ff_ff },
                    "door": { "style": "barn", "color": 0xb8_3a_2c },
                    "windows": { "style": "square", "number": 2 },
                    "rooftop": { "on": ["weathervane"], "color": 0x21_21_21 },
                    "fence": { "style": "rail", "color": 0xf7_f1_e3, "number": 15 },
                    "gate": { "style": "wooden", "color": 0xf7_f1_e3 },
                    "ground": { "style": "meadow" },
                    "path": { "style": "gravel" },
                },
                "items": [
                    { "kind": "vegetable_patch", "x": 0, "y": 13 },
                    { "kind": "chicken_coop", "x": 12, "y": 13, "color": 0xb8_3a_2c },
                    { "kind": "beehive", "x": 3, "y": 13 },
                    { "kind": "compost_bin", "x": 14, "y": 2 },
                ],
                "pets": [{
                    "species": "chicken",
                    "name": "nugget",
                    "rules": [{ "when": "night", "do": "sleep" }, { "when": "someone_near", "do": "fuss" }],
                }],
            }),
        ),
        template(
            "zen",
            "Zen garden",
            BUILDINGS,
            json!({
                "parts": {
                    "walls": { "style": "timber", "color": 0x6b_4a_2f },
                    "roof": { "style": "hip", "color": 0x3a_3f_4f, "number": 2 },
                    "roofing": { "style": "tiles" },
                    "door": { "style": "sliding", "color": 0xf7_f1_e3 },
                    "windows": { "style": "wide", "number": 1 },
                    "fence": { "style": "bamboo", "number": 15 },
                    "gate": { "style": "gap" },
                    "ground": { "style": "gravel", "color": 0xe0_dc_d0 },
                    "path": { "style": "stepping_stones" },
                },
                "items": [
                    { "kind": "koi_pond", "x": 0, "y": 13 },
                    { "kind": "bamboo_clump", "x": 15, "y": 5 },
                    { "kind": "cherry_blossom", "x": 13, "y": 2 },
                    { "kind": "boulder", "x": 11, "y": 13 },
                ],
            }),
        ),
        template(
            "neon",
            "Neon",
            BUILDINGS,
            json!({
                "parts": {
                    "walls": { "style": "steel", "color": 0x2a_24_36 },
                    "roof": { "style": "flat", "color": 0x21_21_21 },
                    "door": { "style": "sliding", "color": 0xf4_8f_b1 },
                    "sign": { "style": "bolt", "color": 0xf4_8f_b1 },
                    "glow": { "style": "all", "color": 0xb3_9d_db },
                    "string_lights": { "style": "neon", "color": 0xf4_8f_b1 },
                    "fence": { "style": "none" },
                    "awning": { "style": "none" },
                    "wall_lights": { "style": "spotlights", "color": 0x4d_b6_ac },
                    "ground": { "style": "tiles", "color": 0x3a_3f_4f },
                },
                "items": [
                    { "kind": "spotlight", "x": 1, "y": 13, "color": 0x4d_b6_ac },
                    { "kind": "spotlight", "x": 13, "y": 13, "color": 0xf4_8f_b1 },
                ],
            }),
        ),
        template(
            "harvest",
            "Harvest",
            BUILDINGS,
            json!({
                "parts": {
                    "walls": { "style": "timber", "color": 0x8c_4a_2a },
                    "trim": { "style": "bold", "color": 0x3e_2a_1c },
                    "roof": { "style": "gable", "color": 0x6b_3a_22, "number": 2 },
                    "roofing": { "style": "shakes" },
                    "door": { "style": "dutch", "color": 0x3e_2a_1c },
                    "windows": { "style": "paned", "number": 2 },
                    "shutters": { "style": "board", "color": 0x3e_2a_1c },
                    "window_boxes": { "style": "mums" },
                    "sign": { "style": "wreath" },
                    "chimney": { "style": "stone" },
                    "string_lights": { "style": "garland", "color": 0xff_d5_4f },
                    "fence": { "style": "rail", "color": 0x3e_2a_1c, "number": 15 },
                    "gate": { "style": "wooden", "color": 0x3e_2a_1c },
                    "ground": { "style": "leaves" },
                    "path": { "style": "gravel" },
                },
                "items": [
                    { "kind": "apple_tree", "x": 1, "y": 13 },
                    { "kind": "log_pile", "x": 13, "y": 13 },
                    { "kind": "barrel", "x": 15, "y": 13, "color": 0x7b_4a_2a },
                ],
            }),
        ),
        template(
            "fairground",
            "Fairground",
            OPEN_AIR,
            json!({
                "parts": {
                    "centerpiece": { "style": "bandstand", "color": 0xe5_48_4d },
                    "ground": { "style": "cobbles" },
                    "string_lights": { "style": "festoon", "color": 0xff_d5_4f },
                },
                "items": [
                    { "kind": "picnic_table", "x": 1, "y": 1, "color": 0xa9_57_3f },
                    { "kind": "picnic_table", "x": 12, "y": 13, "color": 0xa9_57_3f },
                    { "kind": "lamp_post", "x": 1, "y": 14, "color": 0x21_21_21 },
                    { "kind": "lamp_post", "x": 14, "y": 1, "color": 0x21_21_21 },
                ],
            }),
        ),
        template(
            "park",
            "Park",
            OPEN_AIR,
            json!({
                "parts": {
                    "centerpiece": { "style": "tree" },
                    "ground": { "style": "clover" },
                    "path": { "style": "gravel" },
                },
                "items": [
                    { "kind": "bench", "x": 3, "y": 2, "color": 0x7b_4a_2a },
                    { "kind": "bench", "x": 11, "y": 13, "color": 0x7b_4a_2a },
                    { "kind": "oak", "x": 1, "y": 13 },
                    { "kind": "maple", "x": 13, "y": 1 },
                    { "kind": "wildflowers", "x": 12, "y": 4 },
                ],
            }),
        ),
    ]
}

// The checks (E2, E3, E10, E11, E13 to E19)

/// E13: where a building stands when its outside doesn't say, the same numbers as `BODIES` in
/// `web/outside-spec.js`, and the floors its kind starts with (E38). A town square has no body (E14).
pub fn default_body(kind: &str) -> Option<Body> {
    let (x, y, w, d) = match kind {
        "house" | "cafe" | "studio" | "bar" | "office" | "library" => (3, 4, 10, 8),
        "club" => (3, 3, 10, 10),
        "lounge" => (2, 4, 12, 8),
        "workshop" => (2, 4, 12, 9),
        _ => return None,
    };
    Some(Body { x, y, w, d, floors: spaces::default_floors(kind) })
}

/// E38: whether a kind of place is a building with floors. A town square and a park have none.
pub fn has_floors(kind: &str) -> bool {
    default_body(kind).is_some()
}

/// A rectangle of yard cells.
#[derive(Clone, Copy)]
struct Cells {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
}

impl Cells {
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

/// E13: the whole yard.
const TILE: Cells = Cells { x: 0, y: 0, w: GRID, h: GRID };
/// E14: the middle of a town square.
const CENTREPIECE: Cells = Cells { x: 6, y: 6, w: 4, h: 4 };

fn refuse(message: impl Into<String>) -> ApiError {
    let message: String = message.into();
    if message.chars().count() <= MAX_REFUSAL_CHARS {
        return ApiError::bad_request(message);
    }
    let short: String = message.chars().take(MAX_REFUSAL_CHARS).collect();
    ApiError::bad_request(format!("{short}..."))
}

/// E3.
fn check_color(color: i64, what: &str) -> ApiResult<()> {
    if !(0..=i64::from(MAX_COLOR)).contains(&color) {
        return Err(refuse(format!("{what} must be a whole number from 0 to {MAX_COLOR}")));
    }
    Ok(())
}

/// E10, E11.
fn check_part(kind: &str, word: &str, chosen: &Chosen) -> ApiResult<()> {
    let part = part_of(word)
        .filter(|part| part.kinds.contains(&kind))
        .ok_or_else(|| refuse(format!("a {kind} has no part called {word}")))?;
    let known = |style: &str| part.styles.iter().any(|(listed, _)| *listed == style);
    if part.many {
        if chosen.style.is_some() {
            return Err(refuse(format!("{word} takes an on list and no style")));
        }
        if let Some(on) = &chosen.on {
            for (at, style) in on.iter().enumerate() {
                if !known(style) {
                    return Err(refuse(format!("{style} isn't something {word} can have")));
                }
                if on[..at].contains(style) {
                    return Err(refuse(format!("{style} is on {word} twice")));
                }
            }
        }
    } else {
        if chosen.on.is_some() {
            return Err(refuse(format!("{word} takes a style and no on list")));
        }
        if let Some(style) = &chosen.style {
            if !known(style) {
                return Err(refuse(format!("{style} isn't a style of {word}")));
            }
        }
    }
    if let Some(color) = chosen.color {
        if !part.colors {
            return Err(refuse(format!("{word} takes no colour")));
        }
        check_color(color, "a colour")?;
    }
    if let Some(number) = chosen.number {
        let amount = part.number.ok_or_else(|| refuse(format!("{word} takes no number")))?;
        if !(amount.min..=amount.max).contains(&number) {
            return Err(refuse(format!(
                "{word}'s number must be from {} to {}",
                amount.min, amount.max
            )));
        }
    }
    Ok(())
}

/// E13.
fn check_body(body: &Body) -> ApiResult<()> {
    let Body { x, y, w, d, floors } = *body;
    if x < 1 || y < 1 {
        return Err(refuse("a building stands at least 1 cell in from the edge"));
    }
    if w < MIN_BODY || d < MIN_BODY {
        return Err(refuse(format!("a building is at least {MIN_BODY} cells each way")));
    }
    // Compared this way round so a huge number can't overflow.
    if x > GRID - 1 || y > GRID - 1 || w > GRID - 1 - x || d > GRID - 1 - y {
        return Err(refuse("a building stops at least 1 cell from the far edges"));
    }
    if !(1..=MAX_FLOORS).contains(&floors) {
        return Err(refuse(format!("floors must be from 1 to {MAX_FLOORS}")));
    }
    Ok(())
}

/// E15: where an item stands, turned by its `facing`.
fn footprint(kind: &YardKind, item: &YardItem) -> Cells {
    if item.facing % 2 == 0 {
        Cells { x: item.x, y: item.y, w: kind.w, h: kind.h }
    } else {
        Cells { x: item.x, y: item.y, w: kind.h, h: kind.w }
    }
}

/// E15, E17.
fn check_items(items: &[YardItem]) -> ApiResult<()> {
    if items.len() > MAX_ITEMS {
        return Err(ApiError::conflict(format!("a place holds at most {MAX_ITEMS} items")));
    }
    for item in items {
        let kind = yard_kind_of(&item.kind)
            .ok_or_else(|| refuse(format!("{} isn't something a yard can hold", item.kind)))?;
        if !(0..=3).contains(&item.facing) {
            return Err(refuse("facing must be 0, 1, 2 or 3"));
        }
        if let Some(color) = item.color {
            if !kind.tint {
                return Err(refuse(format!("{} takes no colour", item.kind)));
            }
            check_color(color, "a colour")?;
        }
    }
    Ok(())
}

/// E18, E19.
fn check_pets(pets: &[Pet]) -> ApiResult<()> {
    if pets.len() > MAX_PETS {
        return Err(ApiError::conflict(format!("a place holds at most {MAX_PETS} pets")));
    }
    for pet in pets {
        if !SPECIES.iter().any(|(species, _)| *species == pet.species) {
            return Err(refuse(format!("{} isn't a species", pet.species)));
        }
        if !PET_NAMES.contains(&pet.name.as_str()) {
            return Err(refuse("a pet's name comes from the list"));
        }
        for color in [pet.coat, pet.collar].into_iter().flatten() {
            check_color(color, "a colour")?;
        }
        if pet.rules.len() > MAX_RULES {
            return Err(ApiError::conflict(format!("a pet has at most {MAX_RULES} rules")));
        }
        for rule in &pet.rules {
            if !WHEN.iter().any(|(word, _)| *word == rule.when) {
                return Err(refuse(format!("{} isn't something a rule can wait for", rule.when)));
            }
            if !DO.iter().any(|(word, _)| *word == rule.action) {
                return Err(refuse(format!("{} isn't something a pet can do", rule.action)));
            }
        }
    }
    Ok(())
}

/// E16: the items fit together with the building or the centrepiece. Everything is already known to
/// be in range, so this is where the document is checked as a whole.
fn check_fit(kind: &str, outside: &Outside) -> ApiResult<()> {
    let mut standing: Vec<Cells> = Vec::new();
    if OPEN_AIR.contains(&kind) {
        // E14: a park starts with nothing in its middle, and a square with its fountain.
        let style = outside.parts.get("centerpiece").and_then(|chosen| chosen.style.as_deref());
        if style.unwrap_or(if kind == "green" { "none" } else { "fountain" }) != "none" {
            standing.push(CENTREPIECE);
        }
    } else if let Some(body) = outside.body.or_else(|| default_body(kind)) {
        standing.push(Cells { x: body.x, y: body.y, w: body.w, h: body.d });
    }
    for item in &outside.items {
        let Some(yard) = yard_kind_of(&item.kind) else { continue };
        let spot = footprint(yard, item);
        if !spot.inside(&TILE) {
            return Err(ApiError::conflict(format!("{} would stand off the tile", item.kind)));
        }
        if standing.iter().any(|other| spot.overlaps(other)) {
            return Err(ApiError::conflict(format!("{} would stand on something", item.kind)));
        }
        standing.push(spot);
    }
    Ok(())
}

/// Everything E2 to E19 say about a document for a place of this kind. It looks at nothing but the
/// document: 400 for what isn't listed or is out of range, and 409 for what doesn't fit (E16).
pub fn check(kind: &str, outside: &Outside) -> ApiResult<()> {
    if !EVERYWHERE.contains(&kind) {
        return Err(refuse("that isn't a kind of place"));
    }
    for (word, chosen) in &outside.parts {
        check_part(kind, word, chosen)?;
    }
    if let Some(body) = &outside.body {
        if OPEN_AIR.contains(&kind) {
            return Err(refuse("a town square or a park has no building"));
        }
        check_body(body)?;
    }
    check_items(&outside.items)?;
    check_pets(&outside.pets)?;
    check_fit(kind, outside)
}

/// E1: an outside with nothing in it is no outside at all.
fn is_empty(outside: &Outside) -> bool {
    outside.parts.is_empty()
        && outside.body.is_none()
        && outside.items.is_empty()
        && outside.pets.is_empty()
}

/// E38: how many floors the building on a tile has, as its outside says: its body's, or its kind's.
pub fn floors_at(state: &AppState, x: i64, y: i64) -> ApiResult<i64> {
    let place = standing(state, x, y)?;
    let outside = current(state, place.x, place.y)?;
    Ok(outside.body.map_or_else(|| spaces::default_floors(&place.kind), |body| body.floors))
}

/// E38: sets how many floors a building has and keeps the rest of its outside. Its rooms are the
/// caller's to match, so this doesn't look at them.
pub fn set_floors(state: &AppState, x: i64, y: i64, floors: i64) -> ApiResult<()> {
    let place = standing(state, x, y)?;
    let mut outside = current(state, place.x, place.y)?;
    let Some(body) = outside.body.or_else(|| default_body(&place.kind)) else {
        return Ok(());
    };
    outside.body = Some(Body { floors: floors.clamp(1, MAX_FLOORS), ..body });
    let text =
        serde_json::to_string(&outside).map_err(|_| ApiError::bad_request("not an outside"))?;
    state.db.save_outside(
        COMMONS_ID,
        place.player_id,
        &place.kind,
        (place.x, place.y),
        &text,
        now(),
    )?;
    state.worlds.mark_stale(COMMONS_ID);
    Ok(())
}

/// E38, W96: stores a checked outside and holds the rooms of the building on its tile to its floors. A
/// save with fewer floors than there are rooms is refused before anything changes, and more floors than
/// rooms bring up the rooms they lack.
fn store(state: &AppState, actor: &Caller, place: &Standing, outside: &Outside) -> ApiResult<()> {
    let floors =
        outside.body.map_or_else(|| spaces::default_floors(&place.kind), |body| body.floors);
    let room =
        if has_floors(&place.kind) { spaces::room_on(state, place.x, place.y)? } else { None };
    if let Some(space) = &room {
        spaces::check_floors(state, space, floors)?;
    }
    if is_empty(outside) {
        state.db.clear_outside(COMMONS_ID, place.player_id, &place.kind, (place.x, place.y))?;
    } else {
        let text =
            serde_json::to_string(outside).map_err(|_| ApiError::bad_request("not an outside"))?;
        state.db.save_outside(
            COMMONS_ID,
            place.player_id,
            &place.kind,
            (place.x, place.y),
            &text,
            now(),
        )?;
    }
    state.worlds.mark_stale(COMMONS_ID);
    if let Some(space) = &room {
        spaces::raise_rooms(state, actor, space, &place.kind, floors)?;
    }
    Ok(())
}

// Routes

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/outside", get(catalog))
        .route("/api/places/{x}/{y}/outside", get(read).put(change).delete(clear))
        .route("/api/worlds/{id}/outsides", get(world_outsides))
        .route("/api/places/{x}/{y}/editors", get(editors))
        .route(
            "/api/places/{x}/{y}/editors/{person_id}",
            axum::routing::put(approve_editor).delete(remove_editor),
        )
}

/// The place on a tile of The Commons, or 404 (E6).
struct Standing {
    x: i64,
    y: i64,
    kind: String,
    player_id: i64,
    owner_id: Option<String>,
}

fn standing(state: &AppState, x: i64, y: i64) -> ApiResult<Standing> {
    // W91: any tile of a big place is that place, and it's stored on its first.
    let (x, y) = state.db.place_anchor_at(COMMONS_ID, x, y)?;
    let (kind, player_id, owner_id) =
        state.db.place_standing(COMMONS_ID, x, y)?.ok_or_else(ApiError::not_found)?;
    Ok(Standing { x, y, kind, player_id, owner_id })
}

/// E8: a person's changes in a minute, whatever they change.
fn allow_change(state: &AppState, caller: &Caller) -> ApiResult<()> {
    if !state.allow_writes(&format!("outside:{}", caller.person_id), MAX_CHANGES_PER_MINUTE) {
        return Err(ApiError::too_many_requests("that's a lot of changes: try again in a minute"));
    }
    Ok(())
}

/// What's stored as the page reads it. A row is a document this code wrote.
fn document(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!({}))
}

/// E25: the outside of a place and its kind, for someone signed in as a person.
async fn read(
    State(state): State<AppState>,
    caller: Caller,
    Path((x, y)): Path<(i64, i64)>,
) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "see an outside: it's for people")?;
    let place = standing(&state, x, y)?;
    let outside = match state.db.outside_at(COMMONS_ID, place.x, place.y)? {
        Some(text) => document(&text),
        None => json!({}),
    };
    Ok(Json(json!({
        "x": place.x,
        "y": place.y,
        "kind": place.kind,
        "outside": outside,
        "floors": floors_at(&state, place.x, place.y)?,
        "yours": place.owner_id.as_deref() == Some(caller.person_id.as_str()),
        "can_edit": may_edit_house(&state, &caller, &place)? || place.owner_id.as_deref() == Some(caller.person_id.as_str()),
    })))
}

/// E6, E25: the owner replaces the whole document. Who's asking comes before what they asked, so a
/// stranger's bad body tells them nothing.
async fn change(
    State(state): State<AppState>,
    caller: Caller,
    Path((x, y)): Path<(i64, i64)>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "change an outside")?;
    let place = standing(&state, x, y)?;
    if place.owner_id.as_deref() != Some(caller.person_id.as_str())
        && !may_edit_house(&state, &caller, &place)?
    {
        return Err(ApiError::forbidden(
            "only the owner or an approved house editor can change its outside",
        ));
    }
    allow_change(
        &state,
        &Caller {
            person_id: place.owner_id.clone().unwrap_or_else(|| caller.person_id.clone()),
            ..caller.clone()
        },
    )?;
    let Json(body) = body.map_err(|rejection| refuse(rejection.body_text()))?;
    let outside: Outside =
        serde_json::from_value(body).map_err(|error| refuse(format!("not an outside: {error}")))?;
    check(&place.kind, &outside)?;
    store(&state, &caller, &place, &outside)?;
    Ok(Json(json!({ "outside": outside })))
}

/// E7: the owner or the keeper takes away what's stored.
async fn clear(
    State(state): State<AppState>,
    caller: Caller,
    Path((x, y)): Path<(i64, i64)>,
) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "clear an outside")?;
    let place = standing(&state, x, y)?;
    let keeper = state.config.keeper_id.as_deref() == Some(caller.person_id.as_str());
    if place.owner_id.as_deref() != Some(caller.person_id.as_str()) && !keeper {
        return Err(ApiError::forbidden("only the owner of a place or the keeper can clear it"));
    }
    allow_change(&state, &caller)?;
    store(&state, &caller, &place, &Outside::default())?;
    Ok(Json(json!({ "ok": true })))
}

/// E37: grants belong to the owning player, never to whichever building next uses this tile.
fn may_edit_house(state: &AppState, caller: &Caller, place: &Standing) -> ApiResult<bool> {
    if place.kind != "house" {
        return Ok(false);
    }
    Ok(place.owner_id.as_deref() == Some(&caller.person_id)
        || state.db.house_editor(place.player_id, &caller.person_id)?)
}

fn own_house(state: &AppState, caller: &Caller, x: i64, y: i64) -> ApiResult<Standing> {
    ops::require_human(caller, "choose approved house editors")?;
    let place = standing(state, x, y)?;
    if place.kind != "house" || place.owner_id.as_deref() != Some(&caller.person_id) {
        return Err(ApiError::forbidden("only the house owner chooses approved editors"));
    }
    Ok(place)
}

async fn editors(
    State(state): State<AppState>,
    caller: Caller,
    Path((x, y)): Path<(i64, i64)>,
) -> ApiResult<Json<Value>> {
    let place = own_house(&state, &caller, x, y)?;
    Ok(Json(json!({"editors":state.db.house_editors(place.player_id)?})))
}

async fn approve_editor(
    State(state): State<AppState>,
    caller: Caller,
    Path((x, y, person_id)): Path<(i64, i64, String)>,
) -> ApiResult<Json<Value>> {
    let place = own_house(&state, &caller, x, y)?;
    if state.db.person(&person_id)?.is_none() {
        return Err(ApiError::not_found());
    }
    allow_change(&state, &caller)?;
    state.db.set_house_editor(place.player_id, &person_id, true)?;
    Ok(Json(json!({"ok":true})))
}

async fn remove_editor(
    State(state): State<AppState>,
    caller: Caller,
    Path((x, y, person_id)): Path<(i64, i64, String)>,
) -> ApiResult<Json<Value>> {
    let place = own_house(&state, &caller, x, y)?;
    allow_change(&state, &caller)?;
    state.db.set_house_editor(place.player_id, &person_id, false)?;
    Ok(Json(json!({"ok":true})))
}

pub fn decorate_house(
    state: &AppState,
    caller: &Caller,
    x: i64,
    y: i64,
    outside: Option<&Value>,
) -> ApiResult<Value> {
    ops::no_house_bot(caller)?;
    if caller.agent_id.is_none() {
        return Err(ApiError::forbidden("this tool is for bots"));
    }
    let place = standing(state, x, y)?;
    if !may_edit_house(state, caller, &place)? {
        return Err(ApiError::forbidden(
            "only the house owner's bots and approved people's bots may edit this house",
        ));
    }
    let Some(value) = outside else {
        return Ok(json!({"x":place.x,"y":place.y,"outside":current(state,place.x,place.y)?}));
    };
    ops::note_agent_write(state, caller)?;
    let outside: Outside = serde_json::from_value(value.clone())
        .map_err(|error| refuse(format!("not an outside: {error}")))?;
    let owner = place.owner_id.as_deref().ok_or_else(ApiError::not_found)?;
    save_for_project(state, caller, owner, place.x, place.y, &outside)?;
    Ok(json!({"ok":true,"outside":outside}))
}

/// E27: every outside that has its place, for anyone, limited like a view. Only The Commons has any.
async fn world_outsides(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    worlds::allow_view(&state, &headers, now())?;
    worlds::ensure_commons(&state)?;
    if state.db.world(&id)?.is_none() {
        return Err(ApiError::not_found());
    }
    let body = if id == COMMONS_ID {
        the_map(&state)?
    } else {
        Arc::new(json!({ "outsides": [] }).to_string())
    };
    let json = HeaderValue::from_static("application/json");
    Ok(([(header::CONTENT_TYPE, json)], body.as_str().to_owned()).into_response())
}

/// E27: the answer for The Commons. Anyone can ask as often as a view allows and it can hold every
/// outside on the map, so it is built once for each stamp (E26) and not once for each ask. The stamp is
/// read first: a change after that only means the next ask builds again.
fn the_map(state: &AppState) -> ApiResult<Arc<String>> {
    let stamp = state.db.outsides_stamp(COMMONS_ID)?;
    if let Some(body) = state.db.kept_outside_map(stamp, MAP_KEPT) {
        return Ok(body);
    }
    let outsides: Vec<Value> = state
        .db
        .live_outsides(COMMONS_ID)?
        .into_iter()
        .map(|(x, y, text)| json!([x, y, document(&text)]))
        .collect();
    let body = Arc::new(json!({ "outsides": outsides }).to_string());
    state.db.keep_outside_map(stamp, body.clone());
    Ok(body)
}

/// E9: everything an outside can be. For people, as `/api/items` is (I22), and for a bot dressing
/// its project's building (L35).
async fn catalog(State(_state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    ops::require_human(&caller, "see the catalog")?;
    Ok(Json(catalog_json()))
}

/// E9, L35: the catalog as one document.
pub fn catalog_json() -> Value {
    json!({
        "groups": GROUPS.iter().map(|(group, label)| {
            json!({ "group": group, "label": label })
        }).collect::<Vec<_>>(),
        "parts": PARTS.iter().map(|part| json!({
            "part": part.part,
            "label": part.label,
            "group": part.group,
            "kinds": part.kinds,
            "styles": part.styles.iter().map(|(style, label)| {
                json!({ "style": style, "label": label })
            }).collect::<Vec<_>>(),
            "many": part.many,
            "colors": part.colors,
            "number": part.number,
        })).collect::<Vec<_>>(),
        "yard": YARD.as_slice(),
        "pets": {
            "species": SPECIES.iter().map(|(species, label)| {
                json!({ "species": species, "label": label })
            }).collect::<Vec<_>>(),
            "names": PET_NAMES.as_slice(),
            "when": WHEN.iter().map(|(word, label)| {
                json!({ "when": word, "label": label })
            }).collect::<Vec<_>>(),
            "do": DO.iter().map(|(word, label)| {
                json!({ "do": word, "label": label })
            }).collect::<Vec<_>>(),
        },
        "templates": templates(),
        "packs": packs(),
        "swatches": SWATCHES,
        "limits": {
            "grid": GRID,
            "min_body": MIN_BODY,
            "max_floors": MAX_FLOORS,
            "items": MAX_ITEMS,
            "pets": MAX_PETS,
            "rules": MAX_RULES,
        },
    })
}

/// L35: a place's outside as it's stored, or an empty one.
pub fn current(state: &AppState, x: i64, y: i64) -> ApiResult<Outside> {
    let place = standing(state, x, y)?;
    Ok(match state.db.outside_at(COMMONS_ID, place.x, place.y)? {
        Some(text) => serde_json::from_str(&text).unwrap_or_default(),
        None => Outside::default(),
    })
}

/// L35: a template or a pack laid over an outside, as the page lays one (E23, E34): its parts and
/// any body are set, and each of its items goes in where it fits round what's there.
pub fn lay_over(kind: &str, outside: &mut Outside, look: &Value) -> ApiResult<()> {
    let look: Outside =
        serde_json::from_value(look.clone()).map_err(|_| refuse("that look doesn't fit"))?;
    for (word, chosen) in look.parts {
        if part_of(&word).is_some_and(|part| part.kinds.contains(&kind)) {
            outside.parts.insert(word, chosen);
        }
    }
    if look.body.is_some() && !OPEN_AIR.contains(&kind) {
        outside.body = look.body;
    }
    for item in look.items {
        if outside.items.len() >= MAX_ITEMS {
            break;
        }
        outside.items.push(item);
        if check_fit(kind, outside).is_err() {
            outside.items.pop();
        }
    }
    Ok(())
}

/// L35: a bot saves its project's outside, held to every check a person's change is (E2 to E19), and
/// to its owner's 60 changes a minute (E8).
pub fn save_for_project(
    state: &AppState,
    actor: &Caller,
    owner_id: &str,
    x: i64,
    y: i64,
    outside: &Outside,
) -> ApiResult<()> {
    let place = standing(state, x, y)?;
    if place.owner_id.as_deref() != Some(owner_id) {
        return Err(ApiError::forbidden("only the owner of a place can change its outside"));
    }
    if !state.allow_writes(&format!("outside:{owner_id}"), MAX_CHANGES_PER_MINUTE) {
        return Err(ApiError::too_many_requests("that's a lot of changes: try again in a minute"));
    }
    check(&place.kind, outside)?;
    store(state, actor, &place, outside)
}
