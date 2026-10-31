//! The world engine for The Commons and the worlds people start: land, the highway, the moves, the
//! simulation, the founding town, names, and the crawler frogs and fetcher visitors
//! (docs/public-city.md). It is all
//! pure functions of its arguments, with no storage, clock, randomness or floating point, so a
//! world looks the same on every machine and replaying its steps gives the same tiles. Storage and
//! routes call into here.
//!
//! Changing how land is generated redraws every existing world, so the golden test at the bottom
//! fails when it happens: that should be a decision, not an accident.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use serde::{Deserialize, Serialize};

// What the rules say (W13-W28)

/// The smallest and largest land `terrain` makes. Other sizes are clamped into this range.
pub const MIN_SIZE: usize = 16;
pub const MAX_SIZE: usize = 256;
/// The Commons is 128 by 128 with a fixed seed, so it is the same world after every restart.
pub const COMMONS_SIZE: usize = 128;
pub const COMMONS_SEED: u64 = 307;
/// A world someone starts is 64 by 64.
pub const OWN_SIZE: usize = 64;

pub const MAX_LEVEL: u8 = 3;
/// A road is a straight line of at most this many tiles, and a zone at most this many on a side.
pub const MAX_ROAD_TILES: u32 = 16;
pub const MAX_ZONE_SIDE: u32 = 5;
/// A builder who hasn't been seen for this long has left their level 0 tiles for anyone to clear.
pub const GONE_SECS: i64 = 7 * 24 * 60 * 60;

pub const ROAD_COST: u32 = 1;
pub const ZONE_COST: u32 = 1;
pub const PARK_COST: u32 = 2;
pub const BULLDOZE_COST: u32 = 1;
pub const WALK_COST: u32 = 1;
pub const EMOTE_COST: u32 = 1;

/// A zone grows only within this many tiles of a road joined to the highway.
const ROAD_REACH: usize = 3;
/// Parks within this many tiles help a zone grow.
const PARK_REACH: usize = 4;
/// Workshops within this many tiles of homes hurt their growth.
const SMOKE_REACH: usize = 2;
/// W15: a side street runs this many tiles straight out from a road with no builder, and one
/// starts at every sixth tile along such a road.
pub const SIDE_STREET: i64 = 3;
const SIDE_EVERY: i64 = 6;

const PEOPLE_PER_HOME_LEVEL: u32 = 4;
const JOBS_PER_SHOP_LEVEL: u32 = 3;
const JOBS_PER_WORKSHOP_LEVEL: u32 = 5;
/// How many people one level of shops looks after. Past that, shops are wanted.
const PEOPLE_PER_SHOP_LEVEL: u32 = 6;

const VILLAGE_AT: u32 = 50;
const TOWN_AT: u32 = 600;
const CITY_AT: u32 = 3000;

/// Where something is: x across, y down, row 0 at the north edge.
pub type Pos = (u16, u16);
/// What is built, in a stable order. Land with nothing on it has no entry.
pub type Tiles = BTreeMap<Pos, Tile>;

// Hashing

const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// Spreads a number's bits (the finishing steps of splitmix64). Only wrapping integer maths, so
/// every machine agrees.
fn mix(value: u64) -> u64 {
    let mut z = value.wrapping_add(GOLDEN);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One number from several. The order of the parts matters.
fn hash(parts: &[u64]) -> u64 {
    parts
        .iter()
        .fold(0x243F_6A88_85A3_08D3, |acc, part| mix(acc ^ part))
}

fn text_hash(text: &str) -> u64 {
    text.bytes().fold(
        0x1357_9BDF_0246_8ACE,
        |acc, byte| mix(acc ^ u64::from(byte)),
    )
}

// Land

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ground {
    Grass,
    Forest,
    Water,
    Sand,
    Hill,
}

impl Ground {
    pub fn letter(self) -> char {
        match self {
            Ground::Grass => 'g',
            Ground::Forest => 'f',
            Ground::Water => 'w',
            Ground::Sand => 's',
            Ground::Hill => 'h',
        }
    }

    pub fn from_letter(letter: char) -> Option<Ground> {
        match letter {
            'g' => Some(Ground::Grass),
            'f' => Some(Ground::Forest),
            'w' => Some(Ground::Water),
            's' => Some(Ground::Sand),
            'h' => Some(Ground::Hill),
            _ => None,
        }
    }

    /// Grass and sand can be built on as they are.
    pub fn is_open(self) -> bool {
        matches!(self, Ground::Grass | Ground::Sand)
    }
}

/// The ground of a square world. Terrain isn't stored: it comes from the seed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Land {
    size: usize,
    cells: Vec<Ground>,
}

impl Land {
    fn filled(size: usize, ground: Ground) -> Land {
        Land {
            size,
            cells: vec![ground; size * size],
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn get(&self, x: usize, y: usize) -> Option<Ground> {
        (x < self.size && y < self.size).then(|| self.cells[y * self.size + x])
    }

    pub fn at(&self, (x, y): Pos) -> Option<Ground> {
        self.get(usize::from(x), usize::from(y))
    }

    /// The tile at these coordinates if they are on the map. They arrive as untrusted integers,
    /// so this is the one place they become a position.
    pub fn locate(&self, x: i64, y: i64) -> Option<Pos> {
        let size = self.size as i64;
        ((0..size).contains(&x) && (0..size).contains(&y)).then(|| (x as u16, y as u16))
    }

    pub fn centre(&self) -> Pos {
        let middle = (self.size / 2) as u16;
        (middle, middle)
    }

    fn set(&mut self, x: usize, y: usize, ground: Ground) {
        if x < self.size && y < self.size {
            self.cells[y * self.size + x] = ground;
        }
    }

    pub fn count(&self, ground: Ground) -> usize {
        self.cells.iter().filter(|cell| **cell == ground).count()
    }
}

/// One string per row, each run of the same ground as a count then a letter, like `40g3f10w`.
pub fn encode_land(land: &Land) -> Vec<String> {
    land.cells
        .chunks(land.size.max(1))
        .map(encode_row)
        .collect()
}

fn encode_row(row: &[Ground]) -> String {
    let mut text = String::new();
    let mut start = 0;
    while start < row.len() {
        let run = row[start..]
            .iter()
            .take_while(|ground| **ground == row[start])
            .count();
        text.push_str(&run.to_string());
        text.push(row[start].letter());
        start += run;
    }
    text
}

/// The other way. A row that doesn't add up, or a letter that isn't ground, is refused.
pub fn decode_land(rows: &[String]) -> Result<Land, &'static str> {
    let size = rows.len();
    if size == 0 || size > MAX_SIZE {
        return Err("land needs between 1 and 256 rows");
    }
    let mut cells = Vec::with_capacity(size * size);
    for row in rows {
        let before = cells.len();
        decode_row(row, size, &mut cells)?;
        if cells.len() - before != size {
            return Err("every row of land must be as long as there are rows");
        }
    }
    Ok(Land { size, cells })
}

fn decode_row(row: &str, size: usize, cells: &mut Vec<Ground>) -> Result<(), &'static str> {
    let (mut count, mut counted, mut filled) = (0usize, false, 0usize);
    for ch in row.chars() {
        if let Some(digit) = ch.to_digit(10) {
            // Never past the row's length, so a long run of digits can't grow without limit.
            count = count * 10 + digit as usize;
            if count > size {
                return Err("a run of land is longer than its row");
            }
            counted = true;
            continue;
        }
        let ground = Ground::from_letter(ch).ok_or("land has a letter that isn't ground")?;
        if !counted || count == 0 {
            return Err("every run of land needs a count above 0");
        }
        filled += count;
        if filled > size {
            return Err("a row of land is longer than there are rows");
        }
        cells.extend(std::iter::repeat_n(ground, count));
        (count, counted) = (0, false);
    }
    if counted {
        return Err("land ends with a count and no letter");
    }
    Ok(())
}

// Terrain: integer value noise, a few lakes and seas, and ground that follows from them

const SALT_SHAPE: u64 = 100;
const SALT_SHORE: u64 = 110;
const SALT_RELIEF: u64 = 120;
const SALT_TREES: u64 = 130;
const SALT_LAKE: u64 = 140;
const SALT_RIVER: u64 = 150;
const SALT_SEA: u64 = 160;
const SALT_SHARE: u64 = 170;
const SALT_EDGE: u64 = 180;
const SALT_NAME: u64 = 190;
const SALT_FROG: u64 = 200;
const SALT_VISITOR: u64 = 210;

/// A value for every tile, from 0 to 1023.
type Field = Vec<i64>;

/// 0 to 1024 along a smoothstep curve, so blended values have no creases.
fn smooth(along: i64, cell: i64) -> i64 {
    let t = along * 1024 / cell;
    t * t * (3 * 1024 - 2 * t) / (1024 * 1024)
}

fn lerp(from: i64, to: i64, t: i64) -> i64 {
    from + (to - from) * t / 1024
}

/// Random values on a grid `cell` tiles apart, blended smoothly in between.
fn noise_octave(seed: u64, salt: u64, size: usize, cell: usize) -> Field {
    let across = size / cell + 2;
    let lattice: Vec<i64> = (0..across * across)
        .map(|i| (hash(&[seed, salt, (i % across) as u64, (i / across) as u64]) % 1024) as i64)
        .collect();
    let mut field = Vec::with_capacity(size * size);
    for y in 0..size {
        for x in 0..size {
            let (gx, gy) = (x / cell, y / cell);
            let sx = smooth((x % cell) as i64, cell as i64);
            let sy = smooth((y % cell) as i64, cell as i64);
            let at = |dx: usize, dy: usize| lattice[(gy + dy) * across + gx + dx];
            let top = lerp(at(0, 0), at(1, 0), sx);
            let bottom = lerp(at(0, 1), at(1, 1), sx);
            field.push(lerp(top, bottom, sy));
        }
    }
    field
}

/// Three octaves added up, each twice as fine and half as strong: broad shapes with detail.
fn fractal(seed: u64, salt: u64, size: usize, cell: usize) -> Field {
    let cell = cell.max(4);
    let broad = noise_octave(seed, salt, size, cell);
    let middle = noise_octave(seed, salt + 1, size, (cell / 2).max(2));
    let fine = noise_octave(seed, salt + 2, size, (cell / 4).max(2));
    (0..size * size)
        .map(|i| (4 * broad[i] + 2 * middle[i] + fine[i]) / 7)
        .collect()
}

/// Noise along a line, for how far a river wanders.
fn line_noise(seed: u64, at: i64, cell: i64) -> i64 {
    let value = |k: i64| (hash(&[seed, SALT_RIVER, k as u64]) % 1024) as i64;
    lerp(
        value(at / cell),
        value(at / cell + 1),
        smooth(at % cell, cell),
    )
}

/// The value that the top `per_mille` of the field is at or above.
fn top_share(field: &[i64], per_mille: usize) -> i64 {
    let mut counts = [0usize; 1024];
    for value in field {
        counts[(*value).clamp(0, 1023) as usize] += 1;
    }
    let want = field.len() * per_mille / 1000;
    let mut seen = 0;
    for value in (0..1024).rev() {
        seen += counts[value];
        if seen >= want {
            return value as i64;
        }
    }
    0
}

/// The radius around the middle that is kept open for building.
fn clearing(size: usize) -> i64 {
    (size / 6) as i64
}

/// 0 in the middle of the map rising to 1024 at twice the clearing: how much nature is let in.
fn wildness(size: usize, x: i64, y: i64) -> i64 {
    let (dx, dy) = (x - size as i64 / 2, y - size as i64 / 2);
    let reach = 2 * clearing(size);
    (1024 * (dx * dx + dy * dy) / (reach * reach)).min(1024)
}

fn wilder(field: Field, size: usize) -> Field {
    field
        .iter()
        .enumerate()
        .map(|(i, value)| value * wildness(size, (i % size) as i64, (i / size) as i64) / 1024)
        .collect()
}

/// Which tiles are water while the land is still being shaped.
struct Wet {
    size: usize,
    cells: Vec<bool>,
}

impl Wet {
    fn new(size: usize) -> Wet {
        Wet {
            size,
            cells: vec![false; size * size],
        }
    }

    fn fill(&mut self, x: i64, y: i64) {
        let size = self.size as i64;
        if (0..size).contains(&x) && (0..size).contains(&y) {
            self.cells[(y * size + x) as usize] = true;
        }
    }

    fn is(&self, x: i64, y: i64) -> bool {
        let size = self.size as i64;
        (0..size).contains(&x) && (0..size).contains(&y) && self.cells[(y * size + x) as usize]
    }

    /// Whether any water is within `reach` tiles, counting corners.
    fn near(&self, x: i64, y: i64, reach: i64) -> bool {
        (y - reach..=y + reach).any(|yy| (x - reach..=x + reach).any(|xx| self.is(xx, yy)))
    }
}

struct Lake {
    x: i64,
    y: i64,
    radius: i64,
}

/// A coast along one side. 0 is north, then clockwise.
fn add_sea(wet: &mut Wet, wobble: &Field, side: u8, depth: i64) {
    let n = wet.size as i64;
    let sway = n / 10;
    for y in 0..n {
        for x in 0..n {
            let edge = match side {
                0 => y,
                1 => n - 1 - x,
                2 => n - 1 - y,
                _ => x,
            };
            let push = (wobble[(y * n + x) as usize] - 512) * sway / 512;
            if edge + push < depth {
                wet.fill(x, y);
            }
        }
    }
}

fn sea_depth(seed: u64, size: usize) -> i64 {
    let n = size as u64;
    (n * 10 / 100 + hash(&[seed, SALT_SEA]) % (n * 12 / 100 + 1)) as i64
}

/// A lake somewhere off the middle, or nothing if it can't be placed clear of it.
fn find_lake(seed: u64, size: usize) -> Option<Lake> {
    let n = size as u64;
    let radius = (n * 7 / 100 + hash(&[seed, SALT_LAKE]) % (n * 8 / 100 + 1)) as i64;
    let keep = clearing(size) + radius + 2;
    (0..16u64).find_map(|attempt| {
        let h = hash(&[seed, SALT_LAKE + 1, attempt]);
        let x = (n * 15 / 100 + h % (n * 70 / 100 + 1)) as i64;
        let y = (n * 15 / 100 + (h >> 32) % (n * 70 / 100 + 1)) as i64;
        let (dx, dy) = (x - size as i64 / 2, y - size as i64 / 2);
        (dx * dx + dy * dy > keep * keep).then_some(Lake { x, y, radius })
    })
}

fn add_lake(wet: &mut Wet, wobble: &Field, lake: &Lake) {
    let n = wet.size as i64;
    for y in 0..n {
        for x in 0..n {
            let shore = lake.radius + (wobble[(y * n + x) as usize] - 512) * lake.radius / 1200;
            let (dx, dy) = (x - lake.x, y - lake.y);
            let shore = shore.max(0);
            if dx * dx + dy * dy < shore * shore {
                wet.fill(x, y);
            }
        }
    }
}

fn edge_point(n: i64, side: u8, along: i64) -> (i64, i64) {
    match side {
        0 => (along, 0),
        1 => (n - 1, along),
        2 => (along, n - 1),
        _ => (0, along),
    }
}

/// Where a river might run: out of a lake to the sea or the edge of the map, or in from the side
/// of the map to the sea. Each attempt tries somewhere different.
fn river_ends(
    seed: u64,
    n: i64,
    sea: Option<u8>,
    lake: Option<&Lake>,
    attempt: u64,
) -> [(i64, i64); 2] {
    let h = hash(&[seed, SALT_RIVER + 1, attempt]);
    let pick =
        |low: i64, high: i64, shift: u32| low + ((h >> shift) % (high - low + 1) as u64) as i64;
    match (lake, sea) {
        (Some(lake), Some(side)) => [
            (lake.x, lake.y),
            edge_point(n, side, pick(n / 5, n * 4 / 5, 0)),
        ],
        (Some(lake), None) => {
            let (dx, dy) = (lake.x - n / 2, lake.y - n / 2);
            let side = match (dx.abs() >= dy.abs(), dx > 0, dy > 0) {
                (true, true, _) => 1,
                (true, false, _) => 3,
                (false, _, true) => 2,
                (false, _, false) => 0,
            };
            let along = if side % 2 == 0 { lake.x } else { lake.y };
            let along = (along + pick(-n / 6, n / 6, 0)).clamp(1, n - 2);
            [(lake.x, lake.y), edge_point(n, side, along)]
        }
        (None, Some(side)) => {
            // In from an adjacent side, starting on the half of the map away from the coast.
            let from_side = (side + 1 + 2 * (h & 1) as u8) % 4;
            let far = if side == 0 || side == 3 {
                (n * 55 / 100, n * 90 / 100)
            } else {
                (n / 10, n * 45 / 100)
            };
            [
                edge_point(n, from_side, pick(far.0, far.1, 8)),
                edge_point(n, side, pick(n / 5, n * 4 / 5, 16)),
            ]
        }
        (None, None) => [(0, 0), (0, 0)],
    }
}

/// A wandering line from one end to the other, the sideways wander dying away at both ends.
fn river_points(seed: u64, n: i64, [from, to]: [(i64, i64); 2]) -> Vec<(i64, i64)> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = dx.abs().max(dy.abs()).max(1);
    let sway = n / 10;
    let taper = (length / 4).max(1);
    (0..=length)
        .map(|i| {
            let wander = (line_noise(seed, i, (n / 6).max(2)) - 512) * sway / 512;
            let wander = wander * i.min(length - i).min(taper) / taper;
            let (x, y) = (from.0 + dx * i / length, from.1 + dy * i / length);
            if dx.abs() >= dy.abs() {
                (x, y + wander)
            } else {
                (x + wander, y)
            }
        })
        .collect()
}

/// Paints two tiles wide, and fills any sideways jump so the river has no gaps.
fn draw_river(wet: &mut Wet, points: &[(i64, i64)]) {
    let mut last = points[0];
    for &(x, y) in points {
        for yy in last.1.min(y)..=last.1.max(y) {
            for xx in last.0.min(x)..=last.0.max(x) {
                for (bx, by) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    wet.fill(xx + bx, yy + by);
                }
            }
        }
        last = (x, y);
    }
}

/// A river, if one can run without crossing the middle of the map.
fn add_river(wet: &mut Wet, seed: u64, sea: Option<u8>, lake: Option<&Lake>) {
    let n = wet.size as i64;
    let keep = clearing(wet.size) + 2;
    for attempt in 0..6 {
        let points = river_points(seed, n, river_ends(seed, n, sea, lake, attempt));
        let clear = points.iter().all(|&(x, y)| {
            let (dx, dy) = (x - n / 2, y - n / 2);
            dx * dx + dy * dy > keep * keep
        });
        if clear {
            draw_river(wet, &points);
            return;
        }
    }
}

/// Sand beside water, hills where the relief is high, forest in patches, grass for the rest. The
/// middle of the map is kept open so there is room to build.
fn shape_ground(seed: u64, wobble: &Field, wet: &Wet) -> Land {
    let size = wet.size;
    let relief = wilder(fractal(seed, SALT_RELIEF, size, size / 4), size);
    let trees = wilder(fractal(seed, SALT_TREES, size, size / 5), size);
    let share = hash(&[seed, SALT_SHARE]);
    // Hills can't be beside water, so the line is set among the tiles that could have one: a world
    // that is mostly lake still gets its few hills.
    let high: Field = relief
        .iter()
        .enumerate()
        .map(|(i, value)| {
            if wet.near((i % size) as i64, (i / size) as i64, 3) {
                0
            } else {
                *value
            }
        })
        .collect();
    let hill_line = top_share(&high, 30 + (share % 40) as usize);
    let tree_line = top_share(&trees, 130 + ((share >> 16) % 170) as usize);
    let mut land = Land::filled(size, Ground::Grass);
    for y in 0..size {
        for x in 0..size {
            let i = y * size + x;
            let (wx, wy) = (x as i64, y as i64);
            let beach = 1 + i64::from(wobble[i] > 600);
            let ground = if wet.is(wx, wy) {
                Ground::Water
            } else if wet.near(wx, wy, beach) {
                Ground::Sand
            } else if relief[i] >= hill_line && !wet.near(wx, wy, 3) {
                Ground::Hill
            } else if trees[i] >= tree_line {
                Ground::Forest
            } else {
                Ground::Grass
            };
            land.set(x, y, ground);
        }
    }
    keep_the_middle(&mut land);
    land
}

/// Whatever the noise did, the middle has no water or hill, and its heart no forest.
fn keep_the_middle(land: &mut Land) {
    let n = land.size as i64;
    let keep = clearing(land.size);
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x - n / 2, y - n / 2);
            let d2 = dx * dx + dy * dy;
            let ground = land.get(x as usize, y as usize);
            let remove = match ground {
                Some(Ground::Water | Ground::Hill) => d2 < keep * keep,
                Some(Ground::Forest) => d2 < (keep / 2) * (keep / 2),
                _ => false,
            };
            if remove {
                land.set(x as usize, y as usize, Ground::Grass);
            }
        }
    }
}

fn raw_terrain(seed: u64, size: usize) -> Land {
    let wobble = fractal(seed, SALT_SHORE, size, size / 4);
    let flags = hash(&[seed, SALT_SHAPE]);
    let side = ((flags >> 8) % 4) as u8;
    let mut sea = (flags % 100 < 55).then_some(side);
    let lake = if sea.is_none() || (flags >> 16) % 100 < 35 {
        find_lake(seed, size)
    } else {
        None
    };
    if sea.is_none() && lake.is_none() {
        sea = Some(side);
    }
    let mut wet = Wet::new(size);
    if let Some(side) = sea {
        add_sea(&mut wet, &wobble, side, sea_depth(seed, size));
    }
    if let Some(lake) = &lake {
        add_lake(&mut wet, &wobble, lake);
    }
    if (flags >> 24) % 100 < 70 {
        add_river(&mut wet, seed, sea, lake.as_ref());
    }
    shape_ground(seed, &wobble, &wet)
}

/// The land for a seed: a coast or lake, sometimes a river, forests, hills and sand, with the middle
/// kept open. Always joined to an edge of the map by buildable ground, so `highway` has a route.
pub fn terrain(seed: u64, size: usize) -> Land {
    let mut land = raw_terrain(seed, size.clamp(MIN_SIZE, MAX_SIZE));
    open_highway(seed, &mut land);
    land
}

// The highway: the cheapest road from an edge of the map to its middle that goes round water,
// hills and forest, preferring straight runs

const TURN_COST: u32 = 25;
const NO_STATE: u32 = u32::MAX;

/// What it costs to lay the highway across a tile, or 0 if it can't cross. When carving, anything
/// can be crossed at a price, so water costs most and the least is ever carved.
fn crossing_cost(ground: Ground, carve: bool) -> u32 {
    match (ground, carve) {
        (Ground::Grass | Ground::Sand, _) => 10,
        (Ground::Forest, true) => 40,
        (Ground::Hill, true) => 160,
        (Ground::Water, true) => 300,
        _ => 0,
    }
}

/// A heading: 0 north, 1 east, 2 south, 3 west.
fn step_to(size: usize, cell: usize, heading: usize) -> Option<usize> {
    let (x, y) = (cell % size, cell / size);
    match heading {
        0 => (y > 0).then(|| cell - size),
        1 => (x + 1 < size).then(|| cell + 1),
        2 => (y + 1 < size).then(|| cell + size),
        _ => (x > 0).then(|| cell - 1),
    }
}

/// The cheapest way to every tile from one, as a state per tile and heading so that turning costs.
struct Paths {
    cost: Vec<u32>,
    came_from: Vec<u32>,
}

fn cheapest_paths(size: usize, entry_cost: &[u32], start: usize) -> Paths {
    let mut cost = vec![u32::MAX; size * size * 4];
    let mut came_from = vec![NO_STATE; size * size * 4];
    // Ties are broken by the state number, so no two machines can choose differently.
    let mut queue = BinaryHeap::new();
    for heading in 0..4 {
        cost[start * 4 + heading] = 0;
        queue.push(Reverse((0u32, (start * 4 + heading) as u32)));
    }
    while let Some(Reverse((so_far, state))) = queue.pop() {
        let state = state as usize;
        if so_far > cost[state] {
            continue;
        }
        let (cell, heading) = (state / 4, state % 4);
        // Straight on, turn right, turn left.
        for turn in [0, 1, 3] {
            let next_heading = (heading + turn) % 4;
            let Some(next) = step_to(size, cell, next_heading) else {
                continue;
            };
            if entry_cost[next] == 0 {
                continue;
            }
            let total = so_far + entry_cost[next] + if turn == 0 { 0 } else { TURN_COST };
            let next_state = next * 4 + next_heading;
            if total < cost[next_state] {
                cost[next_state] = total;
                came_from[next_state] = state as u32;
                queue.push(Reverse((total, next_state as u32)));
            }
        }
    }
    Paths { cost, came_from }
}

fn edge_cell(size: usize, side: usize, along: usize) -> usize {
    match side {
        0 => along,
        1 => along * size + size - 1,
        2 => (size - 1) * size + along,
        _ => along * size,
    }
}

/// Picks which edge the highway comes in from: a side chosen by the seed if it can be reached, and
/// on it a tile that is cheap to reach but not always the cheapest. Returns the state to trace from.
fn pick_edge(seed: u64, size: usize, paths: &Paths) -> Option<usize> {
    let first = (hash(&[seed, SALT_EDGE]) % 4) as usize;
    (0..4).find_map(|k| {
        let side = (first + k) % 4;
        (0..size)
            .filter_map(|along| {
                let cell = edge_cell(size, side, along);
                let state = (0..4)
                    .map(|heading| cell * 4 + heading)
                    .min_by_key(|s| (paths.cost[*s], *s))?;
                let cost = u64::from(paths.cost[state]);
                if cost == u64::from(u32::MAX) {
                    return None;
                }
                let spread = hash(&[seed, SALT_EDGE + 1, side as u64, along as u64]) % (cost + 1);
                Some((cost + spread, state))
            })
            .min()
            .map(|(_, state)| state)
    })
}

/// From a state back to the start, which is the middle: so edge first, middle last.
fn trace(paths: &Paths, size: usize, mut state: usize) -> Vec<Pos> {
    let mut road = Vec::new();
    loop {
        let cell = state / 4;
        road.push(((cell % size) as u16, (cell / size) as u16));
        let before = paths.came_from[state];
        if before == NO_STATE {
            return road;
        }
        state = before as usize;
    }
}

fn route(seed: u64, land: &Land, carve: bool) -> Option<Vec<Pos>> {
    let size = land.size;
    let entry_cost: Vec<u32> = land
        .cells
        .iter()
        .map(|ground| crossing_cost(*ground, carve))
        .collect();
    let middle = size / 2 * size + size / 2;
    if entry_cost[middle] == 0 {
        return None;
    }
    let paths = cheapest_paths(size, &entry_cost, middle);
    pick_edge(seed, size, &paths).map(|state| trace(&paths, size, state))
}

/// If no edge can reach the middle over open ground, cut the cheapest way through. Bridges aren't
/// a thing, so this changes the land rather than the rules. It is rare, and mostly takes a few trees.
fn open_highway(seed: u64, land: &mut Land) {
    if route(seed, land, false).is_some() {
        return;
    }
    for (x, y) in route(seed, land, true).unwrap_or_default() {
        land.set(usize::from(x), usize::from(y), Ground::Grass);
    }
}

/// The highway for a seed's land, from a tile on the edge to the middle, each tile next to the one
/// before. Use the same seed as `terrain`. Empty only for land that didn't come from `terrain`.
pub fn highway(seed: u64, land: &Land) -> Vec<Pos> {
    route(seed, land, false).unwrap_or_default()
}

/// A new world's tiles: the highway, as ordinary roads with no builder.
pub fn start_tiles(seed: u64, land: &Land) -> Tiles {
    highway(seed, land)
        .into_iter()
        .map(|pos| {
            (
                pos,
                Tile {
                    kind: Kind::Road,
                    level: 0,
                    player: None,
                },
            )
        })
        .collect()
}

// Tiles

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Road,
    Homes,
    Shops,
    Workshops,
    Park,
    Cleared,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Road => "road",
            Kind::Homes => "homes",
            Kind::Shops => "shops",
            Kind::Workshops => "workshops",
            Kind::Park => "park",
            Kind::Cleared => "cleared",
        }
    }

    pub fn parse(word: &str) -> Option<Kind> {
        [
            Kind::Road,
            Kind::Homes,
            Kind::Shops,
            Kind::Workshops,
            Kind::Park,
            Kind::Cleared,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == word)
    }

    pub fn is_zone(self) -> bool {
        matches!(self, Kind::Homes | Kind::Shops | Kind::Workshops)
    }
}

/// One built tile. `player` is who built it, and is `None` for the highway.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    pub kind: Kind,
    pub level: u8,
    pub player: Option<u64>,
}

/// A change to the tiles: a tile is set (built, grown or shrunk) or removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Set(Pos, Tile),
    Remove(Pos),
}

impl Change {
    pub fn pos(&self) -> Pos {
        match self {
            Change::Set(pos, _) | Change::Remove(pos) => *pos,
        }
    }
}

pub fn apply(tiles: &mut Tiles, changes: &[Change]) {
    for change in changes {
        match change {
            Change::Set(pos, tile) => {
                tiles.insert(*pos, *tile);
            }
            Change::Remove(pos) => {
                tiles.remove(pos);
            }
        }
    }
}

// Moves

/// Two corners of a straight road or a zone, as they arrive: untrusted integers, first corner to
/// last, with the first at the top left.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub x: i64,
    pub y: i64,
    pub x2: i64,
    pub y2: i64,
}

impl Span {
    pub fn point(x: i64, y: i64) -> Span {
        Span { x, y, x2: x, y2: y }
    }
}

/// What a move would do and what it would cost. `skipped` counts tiles left alone, not charged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub changes: Vec<Change>,
    pub cost: u32,
    pub skipped: u32,
}

/// A plan, or a short fixed reason it can't be done. A refused move costs nothing.
pub type Planned = Result<Plan, &'static str>;

const OFF_MAP: &str = "that is off the map";
const KEPT: &str = "that strip is kept for a side street: roads only";
const NEEDED: &str = "a grown building depends on that road";

/// What is on a tile, as far as building goes.
enum Spot {
    Free,
    Road,
    Built,
    Blocked(&'static str),
}

fn spot(land: &Land, tiles: &Tiles, pos: Pos) -> Spot {
    match tiles.get(&pos) {
        Some(tile) if tile.kind == Kind::Cleared => Spot::Free,
        Some(tile) if tile.kind == Kind::Road => Spot::Road,
        Some(_) => Spot::Built,
        None => match land.at(pos) {
            Some(Ground::Grass | Ground::Sand) => Spot::Free,
            Some(Ground::Forest) => Spot::Blocked("there is forest in the way: bulldoze it first"),
            _ => Spot::Blocked("there is water or a hill in the way"),
        },
    }
}

fn neighbours((x, y): Pos) -> [Option<Pos>; 4] {
    [
        y.checked_sub(1).map(|y| (x, y)),
        x.checked_add(1).map(|x| (x, y)),
        y.checked_add(1).map(|y| (x, y)),
        x.checked_sub(1).map(|x| (x, y)),
    ]
}

fn is_street(tile: Option<Tile>) -> bool {
    matches!(
        tile,
        Some(Tile {
            kind: Kind::Road,
            player: None,
            ..
        })
    )
}

/// W15: whether a tile is kept for a side street. Beside every road with no builder (the highway
/// and the founding town's streets), each tile whose x plus y is a multiple of 6 starts a street
/// that runs 3 tiles straight out from the road. Zones and parks can't go there but roads can, so a
/// crowd of 1 by 1 zones can't wall the highway in. It follows from position alone, and a road
/// with no builder never comes down, so a tile never stops being kept.
pub fn is_reserved(tiles: &Tiles, pos: Pos) -> bool {
    reserved_by(pos, |at| is_street(tiles.get(&at).copied()))
}

/// W15: the open tiles inside a box (both corners included) that zones and parks skip because they
/// are kept for side streets, row by row. `tiles` must reach 3 tiles past the box.
pub fn reserved_in(land: &Land, tiles: &Tiles, [x0, y0, x1, y1]: [i64; 4]) -> Vec<Pos> {
    // Only the part of the box that is on the map is looked at, however big the box is.
    let last = land.size() as i64 - 1;
    let mut kept = Vec::new();
    for y in y0.max(0)..=y1.min(last) {
        for x in x0.max(0)..=x1.min(last) {
            let Some(pos) = land.locate(x, y) else { continue };
            if matches!(spot(land, tiles, pos), Spot::Free) && is_reserved(tiles, pos) {
                kept.push(pos);
            }
        }
    }
    kept
}

fn reserved_by(pos: Pos, street: impl Fn(Pos) -> bool) -> bool {
    let (x, y) = (i64::from(pos.0), i64::from(pos.1));
    [(0, -1), (1, 0), (0, 1), (-1, 0)]
        .into_iter()
        .any(|(dx, dy)| {
            (0..SIDE_STREET).any(|back| {
                // The street starts `back` tiles behind this one, with the road one more behind.
                let (sx, sy) = (x - dx * back, y - dy * back);
                let road = (u16::try_from(sx - dx), u16::try_from(sy - dy));
                (sx + sy).rem_euclid(SIDE_EVERY) == 0
                    && matches!(road, (Ok(rx), Ok(ry)) if street((rx, ry)))
            })
        })
}

/// Both corners, on the map and the right way round. Everything after this can use plain maths,
/// because a coordinate is now between 0 and 255.
fn corners(land: &Land, span: Span) -> Result<(Pos, Pos), &'static str> {
    let first = land.locate(span.x, span.y).ok_or(OFF_MAP)?;
    let last = land.locate(span.x2, span.y2).ok_or(OFF_MAP)?;
    if last.0 < first.0 || last.1 < first.1 {
        return Err("give the top left corner first");
    }
    Ok((first, last))
}

fn cells(first: Pos, last: Pos) -> impl Iterator<Item = Pos> {
    (first.1..=last.1).flat_map(move |y| (first.0..=last.0).map(move |x| (x, y)))
}

fn sides(first: Pos, last: Pos) -> (u32, u32) {
    (
        u32::from(last.0 - first.0) + 1,
        u32::from(last.1 - first.1) + 1,
    )
}

fn laid(fresh: Vec<Pos>, skipped: u32, tile: Tile, each: u32) -> Plan {
    let cost = fresh.len() as u32 * each;
    Plan {
        changes: fresh
            .into_iter()
            .map(|pos| Change::Set(pos, tile))
            .collect(),
        cost,
        skipped,
    }
}

/// Your own zone that hasn't grown: yours to lay a road over, so nobody walls themselves in.
fn own_lot(tiles: &Tiles, pos: Pos, player: u64) -> bool {
    tiles
        .get(&pos)
        .is_some_and(|tile| tile.kind.is_zone() && tile.level == 0 && tile.player == Some(player))
}

/// W15, W16: a straight line of up to 16 tiles that touches a road. Roads already on the line are
/// kept and not charged, and a road may go over your own zones that haven't grown, and over a side
/// street kept for roads. Anything else in the way, or off the map, refuses the whole line.
pub fn plan_road(land: &Land, tiles: &Tiles, player: u64, span: Span) -> Planned {
    let (first, last) = corners(land, span)?;
    if first.0 != last.0 && first.1 != last.1 {
        return Err("a road runs straight, not diagonally");
    }
    let (across, down) = sides(first, last);
    if across.max(down) > MAX_ROAD_TILES {
        return Err("a road is at most 16 tiles long");
    }
    let (mut fresh, mut joined) = (Vec::new(), false);
    for pos in cells(first, last) {
        match spot(land, tiles, pos) {
            Spot::Road => joined = true,
            Spot::Free => fresh.push(pos),
            Spot::Built if own_lot(tiles, pos, player) => fresh.push(pos),
            Spot::Built => return Err("something is built in the way"),
            Spot::Blocked(why) => return Err(why),
        }
    }
    if fresh.is_empty() {
        return Err("that road is already there");
    }
    let touches = |pos: Pos| {
        neighbours(pos)
            .into_iter()
            .flatten()
            .any(|n| tiles.get(&n).is_some_and(|tile| tile.kind == Kind::Road))
    };
    if !joined && !fresh.iter().any(|pos| touches(*pos)) {
        return Err("a road has to touch an existing road");
    }
    let skipped = across.max(down) - fresh.len() as u32;
    Ok(laid(
        fresh,
        skipped,
        Tile {
            kind: Kind::Road,
            level: 0,
            player: Some(player),
        },
        ROAD_COST,
    ))
}

/// W15, W16: a rectangle of up to 5 by 5. Tiles that are built on, can't be, or are kept for a side
/// street are skipped and not charged. Zoning needs no road: growth does (W20). `tiles` must reach
/// 3 tiles past the rectangle, to see the roads that keep a side street.
pub fn plan_zone(land: &Land, tiles: &Tiles, player: u64, kind: Kind, span: Span) -> Planned {
    if !kind.is_zone() {
        return Err("a zone is homes, shops or workshops");
    }
    let (first, last) = corners(land, span)?;
    let (across, down) = sides(first, last);
    if across.max(down) > MAX_ZONE_SIDE {
        return Err("a zone is at most 5 by 5");
    }
    let (mut fresh, mut skipped, mut kept) = (Vec::new(), 0, false);
    for pos in cells(first, last) {
        match spot(land, tiles, pos) {
            Spot::Free if is_reserved(tiles, pos) => {
                skipped += 1;
                kept = true;
            }
            Spot::Free => fresh.push(pos),
            _ => skipped += 1,
        }
    }
    if fresh.is_empty() {
        return Err(if kept {
            KEPT
        } else {
            "nothing there can be built on"
        });
    }
    Ok(laid(
        fresh,
        skipped,
        Tile {
            kind,
            level: 0,
            player: Some(player),
        },
        ZONE_COST,
    ))
}

/// W15: one park tile. `tiles` must reach 3 tiles past it, as for a zone.
pub fn plan_park(land: &Land, tiles: &Tiles, player: u64, x: i64, y: i64) -> Planned {
    let pos = land.locate(x, y).ok_or(OFF_MAP)?;
    match spot(land, tiles, pos) {
        Spot::Free if is_reserved(tiles, pos) => Err(KEPT),
        Spot::Free => Ok(laid(
            vec![pos],
            0,
            Tile {
                kind: Kind::Park,
                level: 0,
                player: Some(player),
            },
            PARK_COST,
        )),
        Spot::Road | Spot::Built => Err("that tile is already built on"),
        Spot::Blocked(why) => Err(why),
    }
}

/// W17: one tile, and only what you built. Anything else needs its builder gone for 7 days (the
/// caller says who is, by `gone`) and the tile at level 0. The highway has no builder, so it is
/// never bulldozed. A road comes down only if no grown building depends on it, so `tiles` must be
/// every tile in the world when the tile is a road. Forest is cleared by bulldozing, and built-on
/// forest goes back to cleared.
pub fn plan_bulldoze(
    land: &Land,
    tiles: &Tiles,
    player: u64,
    x: i64,
    y: i64,
    gone: &dyn Fn(u64) -> bool,
) -> Planned {
    let plan = plan_bulldoze_tile(land, tiles, player, x, y, gone)?;
    let road = land
        .locate(x, y)
        .filter(|pos| tiles.get(pos).is_some_and(|tile| tile.kind == Kind::Road));
    if road.is_some_and(|pos| strands_a_building(tiles, pos)) {
        return Err(NEEDED);
    }
    Ok(plan)
}

/// W17: whether a tile can be taken down, which needs the tile and nothing around it. A road that
/// passes this still has to pass `plan_bulldoze`'s check of the whole network, so this is for
/// refusing early, from a small read, and not for deciding.
pub fn plan_bulldoze_tile(
    land: &Land,
    tiles: &Tiles,
    player: u64,
    x: i64,
    y: i64,
    gone: &dyn Fn(u64) -> bool,
) -> Planned {
    let pos = land.locate(x, y).ok_or(OFF_MAP)?;
    let cleared = Tile {
        kind: Kind::Cleared,
        level: 0,
        player: Some(player),
    };
    let forest = land.at(pos) == Some(Ground::Forest);
    let Some(tile) = tiles.get(&pos) else {
        if !forest {
            return Err("there is nothing to bulldoze there");
        }
        return Ok(Plan {
            changes: vec![Change::Set(pos, cleared)],
            cost: BULLDOZE_COST,
            skipped: 0,
        });
    };
    if tile.kind == Kind::Cleared {
        return Err("that forest is already cleared");
    }
    let mine = tile.player == Some(player);
    let abandoned = tile.level == 0 && tile.player.is_some_and(gone);
    if !mine && !abandoned {
        return Err("you can only bulldoze what you built");
    }
    let change = if forest {
        Change::Set(pos, cleared)
    } else {
        Change::Remove(pos)
    };
    Ok(Plan {
        changes: vec![change],
        cost: BULLDOZE_COST,
        skipped: 0,
    })
}

/// W17: whether a road tile coming down would leave a grown building with no road joined to the
/// highway within reach, which a building that has one now would lose. It looks at the whole
/// network, so cutting a branch far from a building counts as cutting the building off.
fn strands_a_building(tiles: &Tiles, road: Pos) -> bool {
    let before = Grid::from_tiles(tiles);
    let mut after = Grid::sized(tiles, before.size);
    after.cells[usize::from(road.1) * after.size + usize::from(road.0)] = None;
    let (served, still) = (served_zones(&before), served_zones(&after));
    before.cells.iter().enumerate().any(|(index, cell)| {
        let grown = matches!(cell, Some(tile) if tile.kind.is_zone() && tile.level > 0);
        grown && served[index] && !still[index]
    })
}

/// For each tile, whether a road joined to the highway is within reach (W20).
fn served_zones(grid: &Grid) -> Vec<bool> {
    let joined = joined_roads(grid);
    (0..grid.cells.len())
        .map(|index| grid.count_near(index, ROAD_REACH, |i| joined[i]) > 0)
        .collect()
}

// The simulation

/// What a set of tiles adds up to.
#[derive(Default)]
struct Tally {
    counts: Counts,
    population: u32,
    jobs: u32,
    shop_levels: u32,
}

fn tally<'a>(tiles: impl Iterator<Item = &'a Tile>) -> Tally {
    let mut sum = Tally::default();
    for tile in tiles {
        let level = u32::from(tile.level.min(MAX_LEVEL));
        match tile.kind {
            Kind::Road => sum.counts.roads += 1,
            Kind::Homes => {
                sum.counts.homes += 1;
                sum.population += level * PEOPLE_PER_HOME_LEVEL;
            }
            Kind::Shops => {
                sum.counts.shops += 1;
                sum.jobs += level * JOBS_PER_SHOP_LEVEL;
                sum.shop_levels += level;
            }
            Kind::Workshops => {
                sum.counts.workshops += 1;
                sum.jobs += level * JOBS_PER_WORKSHOP_LEVEL;
            }
            Kind::Park => sum.counts.parks += 1,
            Kind::Cleared => sum.counts.cleared += 1,
        }
    }
    sum
}

/// How many tiles of each kind there are.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub roads: u32,
    pub homes: u32,
    pub shops: u32,
    pub workshops: u32,
    pub parks: u32,
    pub cleared: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Stats {
    pub population: u32,
    pub jobs: u32,
    pub parks: u32,
    pub stage: &'static str,
    pub counts: Counts,
}

/// W23: hamlet, village from 50 people, town from 300, city from 1,500.
pub fn stage_for(population: u32) -> &'static str {
    match population {
        CITY_AT.. => "city",
        TOWN_AT.. => "town",
        VILLAGE_AT.. => "village",
        _ => "hamlet",
    }
}

/// W23: population is homes' levels times 4, jobs are shops' levels times 3 plus workshops' times 5.
pub fn stats(tiles: &Tiles) -> Stats {
    let sum = tally(tiles.values());
    Stats {
        population: sum.population,
        jobs: sum.jobs,
        parks: sum.counts.parks,
        stage: stage_for(sum.population),
        counts: sum.counts,
    }
}

// Your town: what one player's own tiles add up to, and what to aim for next.

/// W48: the score a player plays for. It is the world's own sums (W23) over their tiles alone, so
/// the towns of everyone in a world add up to the world's. A lot that has been zoned counts, at
/// level 0. `best_home` is not sent: it only steers the goal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Town {
    pub roads: u32,
    pub homes: u32,
    pub shops: u32,
    pub workshops: u32,
    pub parks: u32,
    pub people: u32,
    pub jobs: u32,
    /// The highest level any of their zones has reached.
    pub best_level: u8,
    #[serde(skip)]
    pub best_home: u8,
}

/// W48: a player's town, from the tiles they built.
pub fn town(tiles: &Tiles) -> Town {
    let sum = tally(tiles.values());
    let best = |wanted: fn(Kind) -> bool| {
        let levels = tiles.values().filter(|tile| wanted(tile.kind)).map(|tile| tile.level);
        levels.max().unwrap_or(0).min(MAX_LEVEL)
    };
    Town {
        roads: sum.counts.roads,
        homes: sum.counts.homes,
        shops: sum.counts.shops,
        workshops: sum.counts.workshops,
        parks: sum.counts.parks,
        people: sum.population,
        jobs: sum.jobs,
        best_level: best(Kind::is_zone),
        best_home: best(|kind| kind == Kind::Homes),
    }
}

// Goals: one sentence, picked from this ladder by a player's town, so what a reply says never
// comes from a caller (W8).

const GOAL_ROAD: &str = "Lay your first road, starting beside the highway.";
const GOAL_SETTLERS: &str = "House your first 12 people: zone homes within 3 tiles of your road.";
const GOAL_SHOP: &str = "Open a shop: zone one near your homes.";
const GOAL_LEVEL_2: &str = "Get a home to level 2: it needs a grown shop within 5 tiles.";
const GOAL_LEVEL_3: &str = "Get a home to level 3: it needs a park within 4 tiles.";
const GOAL_FIFTY: &str = "Reach 50 people of your own.";
const GOAL_WORK: &str = "Give your people work: add workshops and shops until every one has a job.";
const GOAL_HUNDRED_FIFTY: &str = "Reach 150 people of your own.";
const GOAL_FOUR_HUNDRED: &str = "Reach 400 people of your own.";
const GOAL_OPEN: &str = "Your town is thriving. Keep growing it, or start somewhere new: it is yours to shape.";

/// Every goal there is, in the order of the ladder. A goal is always one of these.
pub const GOALS: [&str; 10] = [
    GOAL_ROAD,
    GOAL_SETTLERS,
    GOAL_SHOP,
    GOAL_LEVEL_2,
    GOAL_LEVEL_3,
    GOAL_FIFTY,
    GOAL_WORK,
    GOAL_HUNDRED_FIFTY,
    GOAL_FOUR_HUNDRED,
    GOAL_OPEN,
];

/// W49: the first rung of the ladder that a town hasn't reached. The last is an open goal, for a
/// town that has reached them all. A town that slips back (a road bulldozed, say) is sent back to
/// the rung it fell to.
pub fn goal(town: &Town) -> &'static str {
    let reached = [
        town.roads > 0,
        town.people >= 12,
        town.shops > 0,
        town.best_home >= 2,
        town.best_home >= 3,
        town.people >= 50,
        town.jobs >= town.people,
        town.people >= 150,
        town.people >= 400,
    ];
    let rung = reached.iter().position(|done| !done).unwrap_or(reached.len());
    GOALS[rung.min(GOALS.len() - 1)]
}

// Tips: one short sentence, picked from this list by the state of a player's town. The wording is
// fixed, so what a reply says never comes from a caller (W8).

const TIP_OUT_OF_MOVES: &str = "Out of moves for today: they come back at midnight UTC.";
const TIP_NO_ROAD: &str = "No road yet: lay one that starts beside the highway.";
const TIP_NO_ZONES: &str = "Zone workshops and homes within 3 tiles of your road.";
const TIP_TOO_FAR: &str = "Some of your zones are over 3 tiles from a road joined to the highway.";
const TIP_ADD_HOMES: &str = "Add homes, there are jobs.";
const TIP_ADD_WORKSHOPS: &str = "Add workshops: homes need jobs.";
const TIP_NO_SHOP: &str = "No shop near your homes: add one within 5 tiles.";
const TIP_NO_PARK: &str = "No park near your homes: add one within 4 tiles.";
const TIP_SMOKE: &str = "A workshop within 2 tiles slows your homes: keep them apart.";
const TIP_ADD_SHOPS: &str = "Add shops, there are people.";
const TIP_WAIT: &str = "All set: buildings take minutes to hours to grow, so check back later.";
const TIP_DONE: &str = "Your zones are fully grown: lay more roads and zone more land.";

/// Every tip there is. A tip is always one of these.
pub const TIPS: [&str; 12] = [
    TIP_OUT_OF_MOVES,
    TIP_NO_ROAD,
    TIP_NO_ZONES,
    TIP_TOO_FAR,
    TIP_ADD_HOMES,
    TIP_ADD_WORKSHOPS,
    TIP_NO_SHOP,
    TIP_NO_PARK,
    TIP_SMOKE,
    TIP_ADD_SHOPS,
    TIP_WAIT,
    TIP_DONE,
];

/// The one thing most worth doing next, given everything built in a world and what this player
/// built. It mirrors what the step needs (W20, W21), so it names what is holding their town back.
pub fn tip(tiles: &Tiles, player: u64, moves_left: u32) -> &'static str {
    if moves_left == 0 {
        return TIP_OUT_OF_MOVES;
    }
    let grid = Grid::from_tiles(tiles);
    // Where this player built each kind, and how grown it is.
    let own = |kind: Kind| -> Vec<(usize, u8)> {
        let ours = |tile: &&Tile| tile.kind == kind && tile.player == Some(player);
        let found = grid.cells.iter().enumerate();
        found
            .filter_map(|(index, cell)| cell.as_ref().filter(ours).map(|tile| (index, tile.level)))
            .collect()
    };
    let (homes, shops, workshops) = (own(Kind::Homes), own(Kind::Shops), own(Kind::Workshops));
    let zones: Vec<(usize, u8)> = [&homes, &shops, &workshops].into_iter().flatten().copied().collect();
    if zones.is_empty() {
        return if own(Kind::Road).is_empty() { TIP_NO_ROAD } else { TIP_NO_ZONES };
    }
    let joined = joined_roads(&grid);
    if zones
        .iter()
        .any(|(index, _)| grid.count_near(*index, ROAD_REACH, |i| joined[i]) == 0)
    {
        return TIP_TOO_FAR;
    }
    let sum = tally(grid.cells.iter().flatten());
    let room_homes = sum.jobs + SETTLERS > sum.population;
    let room_shops = sum.population + CORNER_SHOP_PEOPLE > sum.shop_levels * PEOPLE_PER_SHOP_LEVEL;
    // A lot you have zoned counts as there, though nothing has grown on it yet: asking for more of
    // what you just zoned would only be wrong.
    let lot = |zone: &[(usize, u8)]| zone.iter().any(|&(_, level)| level == 0);
    let add_workshops = if lot(&workshops) { TIP_WAIT } else { TIP_ADD_WORKSHOPS };
    if homes.is_empty() {
        return if room_homes { TIP_ADD_HOMES } else { add_workshops };
    }
    let near = |index: usize, reach: usize, kind: Kind, level: u8| {
        grid.count_near(index, reach, |i| {
            matches!(grid.cells[i], Some(tile) if tile.kind == kind && tile.level >= level)
        }) > 0
    };
    // A shop that has grown, or one of yours that is zoned, is a shop near.
    let shop_near = |index: usize| {
        grid.count_near(index, SHOP_REACH, |i| {
            matches!(grid.cells[i], Some(tile)
                if tile.kind == Kind::Shops && (tile.level >= 1 || tile.player == Some(player)))
        }) > 0
    };
    // A home stops at level 1 without a shop near, and at level 2 without a park.
    if homes.iter().any(|&(index, level)| level == 1 && !shop_near(index)) {
        return TIP_NO_SHOP;
    }
    if homes
        .iter()
        .any(|&(index, level)| level == 2 && !near(index, PARK_REACH, Kind::Park, 0))
    {
        return TIP_NO_PARK;
    }
    if homes
        .iter()
        .any(|&(index, level)| level < MAX_LEVEL && near(index, SMOKE_REACH, Kind::Workshops, 1))
    {
        return TIP_SMOKE;
    }
    let waiting = lot(&homes);
    if waiting && !room_homes {
        return add_workshops;
    }
    if !waiting && room_homes {
        return TIP_ADD_HOMES;
    }
    if room_shops && !lot(&shops) {
        return TIP_ADD_SHOPS;
    }
    if zones.iter().any(|&(_, level)| level < MAX_LEVEL) {
        TIP_WAIT
    } else {
        TIP_DONE
    }
}

/// The tiles as a square grid, so a step can look at a tile's surroundings without searching.
/// Tiles beyond the largest possible world are ignored.
struct Grid {
    size: usize,
    cells: Vec<Option<Tile>>,
}

impl Grid {
    fn from_tiles(tiles: &Tiles) -> Grid {
        let limit = MAX_SIZE as u16;
        let size = tiles
            .keys()
            .filter(|&&(x, y)| x < limit && y < limit)
            .map(|&(x, y)| usize::from(x.max(y)) + 1)
            .max()
            .unwrap_or(0);
        Grid::sized(tiles, size)
    }

    /// The tiles on a grid of exactly this size, which must cover the land they are on.
    fn sized(tiles: &Tiles, size: usize) -> Grid {
        let limit = size.min(MAX_SIZE) as u16;
        let mut cells = vec![None; size * size];
        for (&(x, y), &tile) in tiles {
            if x < limit && y < limit {
                cells[usize::from(y) * size + usize::from(x)] = Some(tile);
            }
        }
        Grid { size, cells }
    }

    fn pos(&self, index: usize) -> Pos {
        ((index % self.size) as u16, (index / self.size) as u16)
    }

    fn neighbours(&self, index: usize) -> [Option<usize>; 4] {
        let (x, y) = (index % self.size, index / self.size);
        [
            (y > 0).then(|| index - self.size),
            (x + 1 < self.size).then(|| index + 1),
            (y + 1 < self.size).then(|| index + self.size),
            (x > 0).then(|| index - 1),
        ]
    }

    fn kind_at(&self, index: usize) -> Option<Kind> {
        self.cells[index].map(|tile| tile.kind)
    }

    /// How many tiles within `reach` (a square around this one) pass `hit`.
    fn count_near(&self, index: usize, reach: usize, hit: impl Fn(usize) -> bool) -> u32 {
        let (x, y) = (index % self.size, index / self.size);
        let mut count = 0;
        for yy in y.saturating_sub(reach)..=(y + reach).min(self.size - 1) {
            for xx in x.saturating_sub(reach)..=(x + reach).min(self.size - 1) {
                if hit(yy * self.size + xx) {
                    count += 1;
                }
            }
        }
        count
    }
}

/// W20: roads count only if a walk along roads from the highway reaches them. The highway, and the
/// streets of the founding town, are the roads with no builder.
fn joined_roads(grid: &Grid) -> Vec<bool> {
    let mut joined = vec![false; grid.cells.len()];
    let mut walk = Vec::new();
    for (index, cell) in grid.cells.iter().enumerate() {
        if matches!(
            cell,
            Some(Tile {
                kind: Kind::Road,
                player: None,
                ..
            })
        ) {
            joined[index] = true;
            walk.push(index);
        }
    }
    while let Some(here) = walk.pop() {
        for next in grid.neighbours(here).into_iter().flatten() {
            if !joined[next] && grid.kind_at(next) == Some(Kind::Road) {
                joined[next] = true;
                walk.push(next);
            }
        }
    }
    joined
}

/// The odds of growing are counted out of this many, once a step, by the hash of seed, step and
/// position. Nothing else is random.
const ODDS_SCALE: u64 = 1024;
/// The odds a zone at level 0, 1 and 2 grows a level this step, out of 1,024, with no park or smoke
/// about. The first level comes in a few minutes; the next two take about an hour and several.
const LEVEL_ODDS: [u32; 3] = [512, 24, 6];
/// The odds a zone with no joined road within reach loses a level this step, out of 1,024.
const DECAY_ODDS: u32 = 4;
/// A world always has room for this many people with no job, and for a corner shop, so the first
/// homes and shops appear before there is anything for them to depend on.
const SETTLERS: u32 = 12;
const CORNER_SHOP_PEOPLE: u32 = 6;
/// A home grows past level 1 only with a shop this close, and past level 2 only with a park this close.
const SHOP_REACH: usize = 5;

/// The most the odds of growing can be, as a multiple of the base: ten quarters is two and a half.
const MAX_ODDS: u32 = 10;
const BASE_ODDS: u32 = 4;

/// W21: how many quarters of the base odds a zone has this step. Each park nearby helps (up to
/// three), and each working workshop near a home hurts it (up to three), but never to nothing.
fn growth_odds(kind: Kind, parks: u32, workshops: u32) -> u32 {
    let helped = BASE_ODDS + 2 * parks.min(3);
    let hurt = if kind == Kind::Homes {
        2 * workshops.min(3)
    } else {
        0
    };
    helped.saturating_sub(hurt).clamp(1, MAX_ODDS)
}

/// W21: levels of homes the jobs could take, and of shops the people could use, ahead of this step.
fn open_levels(wanted: u32, per_level: u32) -> usize {
    wanted.div_ceil(per_level) as usize
}

/// W19-W22: what changes in the world in one step. A pure function of the tiles, the seed and the
/// step number, so it can be replayed. Zones near a road joined to the highway may grow a level: the
/// first in a few minutes, the second in about an hour, the third in several. Demand holds them
/// back (homes need jobs, shops need people, workshops just need time, and every world has room
/// for a dozen settlers and a corner shop to start with), new buildings go up before taller ones,
/// and a home grows past level 1 only with a shop within 5 tiles and past level 2 only with a park
/// within 4. Growth is staggered by a hash of seed, step and position, so a city doesn't all change
/// at once. A zone with no such road loses a level now and then, about once in four hours, so a cut
/// road can be mended in time. Changes come in position order.
pub fn step(seed: u64, step_no: u64, tiles: &Tiles) -> Vec<Change> {
    let grid = Grid::from_tiles(tiles);
    if grid.size == 0 {
        return Vec::new();
    }
    let joined = joined_roads(&grid);
    let sum = tally(grid.cells.iter().flatten());
    let room = [
        open_levels(
            (sum.jobs + SETTLERS).saturating_sub(sum.population),
            PEOPLE_PER_HOME_LEVEL,
        ),
        open_levels(
            (sum.population + CORNER_SHOP_PEOPLE)
                .saturating_sub(sum.shop_levels * PEOPLE_PER_SHOP_LEVEL),
            PEOPLE_PER_SHOP_LEVEL,
        ),
        usize::MAX,
    ];
    let zones = [Kind::Homes, Kind::Shops, Kind::Workshops];
    let mut changes = Vec::new();
    let mut hopeful: [Vec<(u8, u64, usize)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (index, cell) in grid.cells.iter().enumerate() {
        let Some(tile) = cell else { continue };
        let Some(slot) = zones.iter().position(|kind| *kind == tile.kind) else {
            continue;
        };
        let (x, y) = grid.pos(index);
        let roll = hash(&[seed, step_no, u64::from(x), u64::from(y)]);
        let chance = (roll % ODDS_SCALE) as u32;
        if grid.count_near(index, ROAD_REACH, |i| joined[i]) == 0 {
            if tile.level > 0 && chance < DECAY_ODDS {
                changes.push(Change::Set(
                    grid.pos(index),
                    Tile {
                        level: tile.level - 1,
                        ..*tile
                    },
                ));
            }
            continue;
        }
        if tile.level >= MAX_LEVEL {
            continue;
        }
        let base = LEVEL_ODDS[usize::from(tile.level)];
        if chance >= base * MAX_ODDS / BASE_ODDS {
            continue;
        }
        let parks = grid.count_near(index, PARK_REACH, |i| grid.kind_at(i) == Some(Kind::Park));
        if tile.kind == Kind::Homes {
            let shop_near = |index| {
                grid.count_near(index, SHOP_REACH, |i| {
                    matches!(
                        grid.cells[i],
                        Some(Tile {
                            kind: Kind::Shops,
                            level: 1..,
                            ..
                        })
                    )
                }) > 0
            };
            if (tile.level == 1 && !shop_near(index)) || (tile.level == 2 && parks == 0) {
                continue;
            }
        }
        let smoke = if tile.kind == Kind::Homes {
            grid.count_near(index, SMOKE_REACH, |i| {
                matches!(
                    grid.cells[i],
                    Some(Tile {
                        kind: Kind::Workshops,
                        level: 1..,
                        ..
                    })
                )
            })
        } else {
            0
        };
        let odds = (base * growth_odds(tile.kind, parks, smoke) / BASE_ODDS).min(ODDS_SCALE as u32);
        if chance < odds {
            hopeful[slot].push((tile.level, roll / ODDS_SCALE, index));
        }
    }
    for (slot, mut picks) in hopeful.into_iter().enumerate() {
        // New buildings come before taller ones, so a fresh zoning isn't kept waiting.
        picks.sort_unstable();
        picks.truncate(room[slot]);
        for (_, _, index) in picks {
            if let Some(tile) = grid.cells[index] {
                changes.push(Change::Set(
                    grid.pos(index),
                    Tile {
                        level: tile.level + 1,
                        ..tile
                    },
                ));
            }
        }
    }
    changes.sort_by_key(Change::pos);
    changes
}

// The founders: the town The Commons starts with, which is all the server ever builds (W46). Its
// tiles have no builder, like the highway. Everything after it is a player's.

const SALT_TOWN: u64 = 220;

/// The founding town has between this many people and that many.
const FOUNDING_PEOPLE: (u32, u32) = (40, 48);

/// One tile of the founding town, in a quarter of it: how far from the street across and down (1 to
/// 5), what, and how grown.
type Plot = (i64, i64, Kind, u8);

/// The four quarters of the town, walking round it. Homes and a park to the first, homes and shops
/// to the second, shops and lots to the third, and the workshops to the fourth, three tiles or more
/// from any home so their smoke doesn't reach.
const TOWN: [&[Plot]; 4] = [
    &[
        (1, 2, Kind::Homes, 3),
        (2, 2, Kind::Homes, 2),
        (1, 3, Kind::Homes, 1),
        (2, 3, Kind::Homes, 1),
        (3, 2, Kind::Homes, 1),
        (3, 3, Kind::Homes, 0),
        (4, 4, Kind::Park, 0),
    ],
    &[
        (1, 1, Kind::Shops, 2),
        (2, 1, Kind::Shops, 1),
        (1, 2, Kind::Shops, 1),
        (3, 2, Kind::Homes, 1),
        (3, 3, Kind::Homes, 1),
        (4, 2, Kind::Homes, 0),
    ],
    &[
        (1, 1, Kind::Shops, 1),
        (2, 1, Kind::Shops, 0),
        (3, 3, Kind::Homes, 0),
        (2, 3, Kind::Park, 0),
    ],
    &[
        (3, 3, Kind::Workshops, 3),
        (4, 3, Kind::Workshops, 2),
        (3, 4, Kind::Workshops, 2),
        (4, 4, Kind::Workshops, 1),
        (5, 3, Kind::Workshops, 0),
    ],
];

/// The start of The Commons: the highway, and round the middle a small town of about fifty people
/// on a grid of streets, with homes, shops, a few workshops set apart, and parks, at mixed levels.
/// It has as many jobs as people, so a step leaves it standing and growing. Nothing in it has a
/// builder, so nobody can bulldoze it. Land that isn't open is left out, so a town on rough ground
/// is smaller, never wrong.
pub fn founding_town(seed: u64, land: &Land) -> Tiles {
    let mut tiles = start_tiles(seed, land);
    let (mx, my) = land.centre();
    let (mx, my) = (i64::from(mx), i64::from(my));
    let put = |tiles: &mut Tiles, x: i64, y: i64, kind: Kind, level: u8| {
        if let Some(pos) = land.locate(x, y) {
            if land.at(pos).is_some_and(Ground::is_open) && !tiles.contains_key(&pos) {
                tiles.insert(
                    pos,
                    Tile {
                        kind,
                        level,
                        player: None,
                    },
                );
            }
        }
    };
    // Streets: one each way through the middle, and a ring round them.
    for along in -6..=6 {
        for across in [-6, 0, 6] {
            put(&mut tiles, mx + along, my + across, Kind::Road, 0);
            put(&mut tiles, mx + across, my + along, Kind::Road, 0);
        }
    }
    let turn = (hash(&[seed, SALT_TOWN]) % 4) as usize;
    let quarters = [(-1, -1), (1, -1), (1, 1), (-1, 1)];
    for (k, plots) in TOWN.iter().enumerate() {
        let (sx, sy) = quarters[(k + turn) % 4];
        for &(a, b, kind, level) in *plots {
            put(&mut tiles, mx + sx * a, my + sy * b, kind, level);
        }
    }
    // Where the highway took a plot, the town can be short of people or of work. Move the homes up
    // or down a level at a time until between 40 and 48 people live on no more than the jobs there are.
    loop {
        let sum = tally(tiles.values());
        let homes = tiles
            .iter()
            .filter(|(_, tile)| tile.kind == Kind::Homes)
            .map(|(pos, tile)| (*pos, tile.level));
        let change = if sum.population > sum.jobs || sum.population > FOUNDING_PEOPLE.1 {
            homes
                .filter(|(_, level)| *level > 0)
                .max_by_key(|(pos, level)| (*level, Reverse(*pos)))
                .map(|(pos, level)| (pos, level - 1))
        } else if sum.population < FOUNDING_PEOPLE.0
            && sum.population + PEOPLE_PER_HOME_LEVEL <= sum.jobs
        {
            homes
                .filter(|(_, level)| *level < MAX_LEVEL)
                .min_by_key(|(pos, level)| (*level, *pos))
                .map(|(pos, level)| (pos, level + 1))
        } else {
            None
        };
        let Some((pos, level)) = change else { break };
        if let Some(tile) = tiles.get_mut(&pos) {
            tile.level = level;
        }
    }
    tiles
}

// Names: short fixed lists, all friendly, reviewed by hand. No combination should be rude, or a
// real company, product or game.

const ADJECTIVES: &[&str] = &[
    "Amber",
    "Coral",
    "Crimson",
    "Copper",
    "Indigo",
    "Jade",
    "Lilac",
    "Mauve",
    "Purple",
    "Ruby",
    "Scarlet",
    "Silver",
    "Teal",
    "Violet",
    "Brave",
    "Bright",
    "Bouncy",
    "Breezy",
    "Bubbly",
    "Calm",
    "Cheery",
    "Chirpy",
    "Clever",
    "Cosy",
    "Crisp",
    "Curious",
    "Dapper",
    "Daring",
    "Dreamy",
    "Eager",
    "Earnest",
    "Fancy",
    "Fearless",
    "Fluffy",
    "Friendly",
    "Frosty",
    "Gentle",
    "Giddy",
    "Glad",
    "Glowing",
    "Graceful",
    "Happy",
    "Hardy",
    "Helpful",
    "Honest",
    "Hopeful",
    "Humble",
    "Jolly",
    "Joyful",
    "Keen",
    "Kind",
    "Lively",
    "Lucky",
    "Merry",
    "Mellow",
    "Misty",
    "Modest",
    "Nifty",
    "Nimble",
    "Noble",
    "Patient",
    "Peppy",
    "Perky",
    "Playful",
    "Plucky",
    "Polite",
    "Quick",
    "Quiet",
    "Radiant",
    "Rosy",
    "Sandy",
    "Shy",
    "Silly",
    "Sleepy",
    "Snug",
    "Soft",
    "Speedy",
    "Spry",
    "Steady",
    "Stalwart",
    "Sunny",
    "Sweet",
    "Tender",
    "Thankful",
    "Tidy",
    "Tiny",
    "Trusty",
    "Upbeat",
    "Vivid",
    "Warm",
    "Whistling",
    "Jaunty",
    "Witty",
    "Wondrous",
    "Zany",
    "Zesty",
    "Zippy",
    "Amiable",
    "Cuddly",
    "Dainty",
    "Hearty",
];

const ANIMALS: &[&str] = &[
    "Panda",
    "Otter",
    "Badger",
    "Heron",
    "Finch",
    "Robin",
    "Wren",
    "Sparrow",
    "Hedgehog",
    "Dormouse",
    "Rabbit",
    "Hare",
    "Beaver",
    "Walrus",
    "Narwhal",
    "Seal",
    "Pelican",
    "Crane",
    "Swan",
    "Owl",
    "Osprey",
    "Magpie",
    "Starling",
    "Lark",
    "Thrush",
    "Swallow",
    "Newt",
    "Tortoise",
    "Turtle",
    "Trout",
    "Minnow",
    "Seahorse",
    "Octopus",
    "Ladybird",
    "Dragonfly",
    "Snail",
    "Ferret",
    "Goat",
    "Sheep",
    "Pony",
    "Donkey",
    "Giraffe",
    "Gazelle",
    "Capybara",
    "Lemur",
    "Quokka",
    "Wombat",
    "Platypus",
    "Toucan",
    "Parrot",
    "Flamingo",
    "Ibis",
    "Tapir",
    "Bison",
    "Moose",
    "Caribou",
    "Yak",
    "Chinchilla",
    "Hamster",
    "Meerkat",
    "Mongoose",
    "Okapi",
    "Pangolin",
    "Axolotl",
    "Marmot",
    "Vole",
    "Stoat",
    "Marten",
    "Ocelot",
    "Plover",
    "Sandpiper",
    "Curlew",
    "Egret",
    "Stork",
    "Nuthatch",
    "Wagtail",
    "Bunting",
    "Linnet",
    "Skylark",
    "Kingfisher",
    "Mallard",
    "Goose",
    "Moth",
    "Squirrel",
    "Chipmunk",
    "Porcupine",
    "Sloth",
    "Armadillo",
    "Starfish",
];

const WORLD_ADJECTIVES: &[&str] = &[
    "Quiet",
    "Misty",
    "Sunny",
    "Golden",
    "Silver",
    "Willow",
    "Maple",
    "Cedar",
    "Birch",
    "Hidden",
    "Windy",
    "Breezy",
    "Gentle",
    "Peaceful",
    "Lazy",
    "Rolling",
    "Frosty",
    "Copper",
    "Rosy",
    "Dewy",
    "Starlit",
    "Moonlit",
    "Sunlit",
    "Hazel",
    "Juniper",
    "Clover",
    "Heather",
    "Primrose",
    "Bramble",
    "Thistle",
    "Lilac",
    "Larch",
    "Alder",
    "Rowan",
    "Oaken",
    "Bonny",
    "Cheery",
    "Cosy",
    "Snug",
    "Mellow",
    "Merry",
    "Placid",
    "Serene",
    "Tranquil",
    "Calm",
    "Fair",
    "Bright",
    "Verdant",
    "Fragrant",
    "Amber",
    "Pebble",
    "Mossy",
    "Dappled",
    "Sheltered",
    "Wandering",
    "Whispering",
    "Shimmering",
    "Humble",
    "Plucky",
    "Grand",
    "Ancient",
    "Linden",
    "Sycamore",
    "Autumn",
    "Evening",
    "Morning",
    "Sunrise",
    "Daisy",
    "Poppy",
    "Laurel",
    "Gleaming",
    "Hearty",
];

const PLACES: &[&str] = &[
    "Meadow", "Harbour", "Valley", "Hollow", "Ridge", "Cove", "Glen", "Grove", "Orchard",
    "Landing", "Crossing", "Haven", "Bay", "Brook", "Green", "Heath", "Marsh", "Mill", "Park",
    "Pond", "Quay", "Shore", "Springs", "Terrace", "Vale", "Wharf", "Woods", "Hill", "Fields",
    "Fells", "Dale", "Dell", "Downs", "Forge", "Garden", "Island", "Isle", "Junction", "Mound",
    "Lagoon", "Lake", "Lane", "Lodge", "Moor", "Saddle", "Peak", "Point", "Prairie", "Reach",
    "Rise", "Run", "Square", "Station", "Steps", "Strand", "Summit", "Trail", "Yard", "Mews",
    "Clearing", "Bluff", "Copse", "Croft", "Fen", "Ford", "Gardens", "Heights", "Inlet", "Paddock",
    "Pastures", "Pines", "Spinney",
];

/// Spread neighbouring numbers far apart in the list of names. Each is a bijection on
/// `0..ADJECTIVES.len() * ANIMALS.len()` because it shares no factor with that product. Visitors
/// use another so that visitor 5 and player 5 aren't the same name.
const NAME_STRIDE: u64 = 2749;
const VISITOR_STRIDE: u64 = 1621;

/// An adjective and an animal, picked by the number.
fn friendly_name(n: u64, stride: u64) -> String {
    let total = (ADJECTIVES.len() * ANIMALS.len()) as u64;
    let pick = (n % total) * stride % total;
    let adjectives = ADJECTIVES.len() as u64;
    format!(
        "{} {}",
        ADJECTIVES[(pick % adjectives) as usize],
        ANIMALS[(pick / adjectives) as usize]
    )
}

/// A player's name, such as "Brave Otter". The number is theirs for good, such as the order they
/// joined in. Every number below the number of names (adjectives times animals) gets a different
/// one; after that names repeat.
pub fn player_name(n: u64) -> String {
    friendly_name(n, NAME_STRIDE)
}

/// A visitor's name, from the same lists as a player's: such as "Merry Otter". The number is the
/// server's for that visitor. Visitor `n` and player `n` almost always get different names, and
/// any two can share one, because the lists are short.
pub fn visitor_name(n: u64) -> String {
    friendly_name(n, VISITOR_STRIDE)
}

/// A world's name, such as "Misty Harbour", from its seed.
pub fn world_name(seed: u64) -> String {
    let h = hash(&[seed, SALT_NAME]);
    let adjective = WORLD_ADJECTIVES[(h % WORLD_ADJECTIVES.len() as u64) as usize];
    let place = PLACES[((h >> 32) % PLACES.len() as u64) as usize];
    format!("{adjective} {place}")
}

// Crawlers and fetchers: what lands on a public page that isn't a person. A crawler indexes the
// web with nobody waiting on it, and shows up as a frog. A fetcher is an agent acting for a person
// right now, and shows up as a visitor with a generated name.

/// A crawler on the list: its id is what the server stores and the page is sent, and it is also
/// the text looked for in a user agent, in lower case.
#[derive(Debug, Serialize)]
pub struct Crawler {
    pub id: &'static str,
    pub name: &'static str,
    pub colour: &'static str,
}

const fn crawler(id: &'static str, name: &'static str, colour: &'static str) -> Crawler {
    Crawler { id, name, colour }
}

/// A fetcher on the list, with the same use for its id as a crawler's. Its name is for counting
/// sightings, not for showing: a visitor is shown under a name from `visitor_name`.
#[derive(Debug, Serialize)]
pub struct Fetcher {
    pub id: &'static str,
    pub name: &'static str,
}

const fn fetcher(id: &'static str, name: &'static str) -> Fetcher {
    Fetcher { id, name }
}

/// Agents that act for a person at the moment they are asked: a chat assistant opening a page, a
/// notebook reading a source, a browsing agent. Ids are different from every crawler's id.
pub static FETCHERS: &[Fetcher] = &[
    fetcher("google-agent", "Google-Agent"),
    fetcher("google-gemininotebook", "Google-GeminiNotebook"),
    fetcher("google-notebooklm", "Google-NotebookLM"),
    fetcher("gemini-deep-research", "Gemini-Deep-Research"),
    fetcher("chatgpt-user", "ChatGPT-User"),
    fetcher("claude-user", "Claude-User"),
    fetcher("perplexity-user", "Perplexity-User"),
    fetcher("mistralai-user", "MistralAI-User"),
    fetcher("duckassistbot", "DuckAssistBot"),
    fetcher("meta-externalfetcher", "Meta-ExternalFetcher"),
];

pub static CRAWLERS: &[Crawler] = &[
    crawler("googlebot", "Googlebot", "#4f86f7"),
    crawler("bingbot", "Bingbot", "#0fb5ae"),
    crawler("gptbot", "GPTBot", "#3fbf5f"),
    crawler("oai-searchbot", "OAI-SearchBot", "#a3d63a"),
    crawler("claudebot", "ClaudeBot", "#e07a3f"),
    crawler("perplexitybot", "PerplexityBot", "#2d9cdb"),
    crawler("duckduckbot", "DuckDuckBot", "#e8603c"),
    crawler("applebot", "Applebot", "#9aa5b1"),
    crawler("yandexbot", "YandexBot", "#d94a6b"),
    crawler("baiduspider", "Baiduspider", "#6c5ce7"),
    crawler("ahrefsbot", "AhrefsBot", "#2f4b9c"),
    crawler("semrushbot", "SemrushBot", "#ff8a5b"),
    crawler("bytespider", "Bytespider", "#8e44ad"),
    crawler("ccbot", "CCBot", "#b08968"),
    crawler("meta-externalagent", "Meta-ExternalAgent", "#50c7e8"),
    crawler("amazonbot", "Amazonbot", "#c0a000"),
    crawler("petalbot", "PetalBot", "#ec6fb0"),
    crawler("mj12bot", "MJ12bot", "#4a5568"),
];

/// Real crawler names sit near the start of a user agent, so this much is read and no more. It
/// keeps the work small however long the string is.
const MAX_AGENT_BYTES: usize = 512;

/// Whether a listed id appears in the start of a user agent, whatever its case. Bytes, not text,
/// so a multi-byte character cut by the limit can't matter.
fn mentions(user_agent: &str, id: &str) -> bool {
    let head = &user_agent.as_bytes()[..user_agent.len().min(MAX_AGENT_BYTES)];
    head.windows(id.len())
        .any(|window| window.eq_ignore_ascii_case(id.as_bytes()))
}

/// Which crawler on the list a user agent is, if any. Case doesn't matter. The string is only
/// looked at: nothing from it is kept or returned, and anything not on the list gives `None`.
pub fn crawler_from_user_agent(user_agent: &str) -> Option<&'static Crawler> {
    CRAWLERS
        .iter()
        .find(|crawler| mentions(user_agent, crawler.id))
}

/// The same for fetchers.
pub fn fetcher_from_user_agent(user_agent: &str) -> Option<&'static Fetcher> {
    FETCHERS
        .iter()
        .find(|fetcher| mentions(user_agent, fetcher.id))
}

/// What a user agent on either list is.
#[derive(Clone, Copy, Debug)]
pub enum AgentKind {
    Crawler(&'static Crawler),
    Fetcher(&'static Fetcher),
}

impl AgentKind {
    /// The id to count a sighting under. No crawler and fetcher share one.
    pub fn id(&self) -> &'static str {
        match self {
            AgentKind::Crawler(crawler) => crawler.id,
            AgentKind::Fetcher(fetcher) => fetcher.id,
        }
    }
}

/// Which list a user agent is on, and which entry. A person is waiting on a fetcher and on nobody
/// else, so if a string names one of each, the fetcher wins. Same care as the two lookups it is
/// made of: safe on any input, and nothing from the string is kept.
pub fn agent_kind(user_agent: &str) -> Option<AgentKind> {
    fetcher_from_user_agent(user_agent)
        .map(AgentKind::Fetcher)
        .or_else(|| crawler_from_user_agent(user_agent).map(AgentKind::Crawler))
}

/// The roads of a world, in order, for frogs and visitors to move along.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roads {
    sorted: Vec<Pos>,
}

impl Roads {
    pub fn from_tiles(tiles: &Tiles) -> Roads {
        Roads {
            sorted: tiles
                .iter()
                .filter(|(_, tile)| tile.kind == Kind::Road)
                .map(|(pos, _)| *pos)
                .collect(),
        }
    }

    pub fn from_positions(positions: impl IntoIterator<Item = Pos>) -> Roads {
        let mut sorted: Vec<Pos> = positions.into_iter().collect();
        sorted.sort_unstable();
        sorted.dedup();
        Roads { sorted }
    }

    pub fn len(&self) -> usize {
        self.sorted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sorted.is_empty()
    }

    fn find(&self, pos: Pos) -> Option<usize> {
        self.sorted.binary_search(&pos).ok()
    }
}

/// How many tiles along its tour a frog moves each step.
const FROG_HOP: u64 = 3;

fn direction_order(frog: u64, (x, y): Pos) -> [usize; 4] {
    let mut order = [0, 1, 2, 3];
    order.sort_by_key(|direction| hash(&[frog, u64::from(x), u64::from(y), *direction as u64]));
    order
}

/// A walk that goes along every road reachable from a starting one and comes back, in an order
/// only this frog's id decides. Each tile is next to the one before it, so a frog that moves along
/// it never jumps. It is a loop: the last tile is next to the first.
fn frog_tour(roads: &Roads, frog: u64) -> Vec<Pos> {
    let root = (hash(&[frog, SALT_FROG]) % roads.len() as u64) as usize;
    let mut seen = vec![false; roads.len()];
    seen[root] = true;
    let mut tour = vec![roads.sorted[root]];
    // Road, how many directions have been tried from it, and the order to try them in.
    let mut path = vec![(root, 0usize, direction_order(frog, roads.sorted[root]))];
    while let Some(top) = path.last_mut() {
        let (here, tried, order) = *top;
        if tried == 4 {
            path.pop();
            if let Some((back, _, _)) = path.last() {
                tour.push(roads.sorted[*back]);
            }
            continue;
        }
        top.1 += 1;
        let next = neighbours(roads.sorted[here])[order[tried]].and_then(|pos| roads.find(pos));
        if let Some(next) = next.filter(|next| !seen[*next]) {
            seen[next] = true;
            tour.push(roads.sorted[next]);
            path.push((next, 0, direction_order(frog, roads.sorted[next])));
        }
    }
    if tour.len() > 1 {
        tour.pop();
    }
    tour
}

/// Where a walker is at a step: `hop` tiles further along its tour than a step ago, from a start
/// that only it decides. `None` when there are no roads.
fn walker_position(walker: u64, hop: u64, step: u64, roads: &Roads) -> Option<Pos> {
    if roads.is_empty() {
        return None;
    }
    let tour = frog_tour(roads, walker);
    let length = tour.len() as u64;
    let start = hash(&[walker, SALT_FROG + 1]) % length;
    let at = (start + (step % length) * hop) % length;
    Some(tour[at as usize])
}

/// Where a crawler's frog is at a step: on a road, a few tiles on from where it was a step ago,
/// going the way only its id and the roads decide. `None` for an id that isn't on the list, or
/// when there are no roads. Nothing a visitor sends can move it.
pub fn crawler_position(id: &str, step: u64, roads: &Roads) -> Option<Pos> {
    let crawler = CRAWLERS.iter().find(|crawler| crawler.id == id)?;
    walker_position(text_hash(crawler.id), FROG_HOP, step, roads)
}

/// How many tiles along its tour a visitor moves each step. A frog hops, and a visitor strolls.
const VISITOR_HOP: u64 = 2;

/// Where visitor number `n` is at a step, the same way: on a road, a couple of tiles on from a
/// step ago, going the way only `n` and the roads decide. The number is whatever the server gave
/// that visitor, so the same visitor is in the same place for as long as the roads are. `None`
/// when there are no roads. Nothing a visitor sends can move it.
pub fn visitor_position(n: u64, step: u64, roads: &Roads) -> Option<Pos> {
    walker_position(hash(&[SALT_VISITOR, n]), VISITOR_HOP, step, roads)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};
    use std::ops::RangeInclusive;
    use std::time::Instant;

    use super::*;

    // What the tests build with

    /// A land drawn as text, one letter per tile, row 0 first.
    fn picture(rows: &[&str]) -> Land {
        let cells: Vec<Ground> = rows
            .iter()
            .flat_map(|row| row.chars().map(|c| Ground::from_letter(c).unwrap()))
            .collect();
        assert_eq!(
            cells.len(),
            rows.len() * rows.len(),
            "a picture must be square"
        );
        Land {
            size: rows.len(),
            cells,
        }
    }

    fn lawn(size: usize) -> Land {
        Land::filled(size, Ground::Grass)
    }

    /// Forest at 4-6 across and 3-4 down, water at 5-6 and 5-6, sand at 10-11 on row 5, hills at
    /// 10-11 on row 6, and grass everywhere else.
    const YARD: [&str; 12] = [
        "gggggggggggg",
        "gggggggggggg",
        "gggggggggggg",
        "ggggfffggggg",
        "ggggfffggggg",
        "gggggwwgggss",
        "gggggwwggghh",
        "gggggggggggg",
        "gggggggggggg",
        "gggggggggggg",
        "gggggggggggg",
        "gggggggggggg",
    ];

    fn tile(kind: Kind, level: u8, player: Option<u64>) -> Tile {
        Tile {
            kind,
            level,
            player,
        }
    }

    /// Highway: roads with no builder, along a row.
    fn highway_row(y: u16, xs: RangeInclusive<u16>) -> Tiles {
        xs.map(|x| ((x, y), tile(Kind::Road, 0, None))).collect()
    }

    /// Zone tiles dropped in directly, as if they had grown there.
    fn block(kind: Kind, level: u8, xs: RangeInclusive<u16>, ys: RangeInclusive<u16>) -> Tiles {
        let mut tiles = Tiles::new();
        for y in ys {
            for x in xs.clone() {
                tiles.insert((x, y), tile(kind, level, Some(1)));
            }
        }
        tiles
    }

    fn span(x: i64, y: i64, x2: i64, y2: i64) -> Span {
        Span { x, y, x2, y2 }
    }

    fn done(planned: Planned, tiles: &mut Tiles) -> Plan {
        let plan = planned.expect("that move should be allowed");
        apply(tiles, &plan.changes);
        plan
    }

    fn nobody_gone(_: u64) -> bool {
        false
    }

    fn run(seed: u64, tiles: &mut Tiles, steps: std::ops::Range<u64>) {
        for step_no in steps {
            let changes = step(seed, step_no, tiles);
            apply(tiles, &changes);
        }
    }

    fn levels(tiles: &Tiles, kind: Kind) -> u32 {
        tiles
            .values()
            .filter(|tile| tile.kind == kind)
            .map(|tile| u32::from(tile.level))
            .sum()
    }

    /// A hash that doesn't depend on the Rust version, for the golden checks.
    fn fnv(parts: impl IntoIterator<Item = String>) -> u64 {
        let mut hash = 0xCBF2_9CE4_8422_2325u64;
        for part in parts {
            for byte in part.bytes().chain(std::iter::once(b'\n')) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01B3);
            }
        }
        hash
    }

    fn land_hash(land: &Land) -> u64 {
        fnv(encode_land(land))
    }

    fn highway_hash(road: &[Pos]) -> u64 {
        fnv(road.iter().map(|(x, y)| format!("{x},{y}")))
    }

    fn is_edge(land: &Land, (x, y): Pos) -> bool {
        let last = (land.size() - 1) as u16;
        x == 0 || y == 0 || x == last || y == last
    }

    // Land

    #[test]
    fn terrain_is_the_same_every_time() {
        for seed in [0, 1, 307, u64::MAX] {
            assert_eq!(terrain(seed, 64), terrain(seed, 64));
        }
        assert_eq!(
            encode_land(&terrain(COMMONS_SEED, COMMONS_SIZE)),
            encode_land(&terrain(COMMONS_SEED, COMMONS_SIZE))
        );
    }

    #[test]
    fn sizes_outside_the_range_are_clamped() {
        assert_eq!(terrain(5, 0).size(), MIN_SIZE);
        assert_eq!(terrain(5, 3).size(), MIN_SIZE);
        assert_eq!(terrain(5, 10_000).size(), MAX_SIZE);
        assert_eq!(terrain(5, usize::MAX).size(), MAX_SIZE);
        assert_eq!(terrain(5, 77).size(), 77);
    }

    /// Changing the generator redraws every world that exists. If this fails after a deliberate
    /// change, that is what you chose: update the numbers, and expect old worlds to look different.
    #[test]
    fn the_land_a_seed_makes_has_not_changed() {
        // Seed, size, then the hash of the land's text rows and of the highway's tiles.
        let golden: [(u64, usize, u64, u64); 6] = [
            (1, 64, 0x32ea3138aa496792, 0x56f6b1128d5eb646),
            (2, 64, 0x104512d937ec4a45, 0x268f99483ae8ad72),
            (42, 64, 0x0369f4cb7ada082c, 0xb9ba13971ca8a664),
            (
                COMMONS_SEED,
                COMMONS_SIZE,
                0xd57f374e207f8e77,
                0x757c14b7155003bf,
            ),
            (0xDEAD_BEEF, 128, 0xd871dd7b76e642ca, 0xa1523549c99e8d60),
            (u64::MAX, 64, 0x77a94f99e287ecf5, 0xa16a54eb13a48561),
        ];
        for (seed, size, land_expected, road_expected) in golden {
            let land = terrain(seed, size);
            let road = highway(seed, &land);
            assert_eq!(
                land_hash(&land),
                land_expected,
                "land for seed {seed} at {size}"
            );
            assert_eq!(
                highway_hash(&road),
                road_expected,
                "highway for seed {seed} at {size}"
            );
        }
    }

    #[test]
    fn different_seeds_look_different() {
        let lands: Vec<Land> = (0..40).map(|seed| terrain(seed, 64)).collect();
        let mut least = 100;
        for (i, a) in lands.iter().enumerate() {
            for b in &lands[i + 1..] {
                let same = a.cells.iter().zip(&b.cells).filter(|(x, y)| x == y).count();
                least = least.min(100 - same * 100 / a.cells.len());
            }
        }
        eprintln!("two of forty worlds differ in at least {least}% of their tiles");
        assert!(
            least >= 12,
            "two worlds differ in only {least}% of their tiles"
        );
        let drawn: HashSet<u64> = (0..500).map(|seed| land_hash(&terrain(seed, 64))).collect();
        assert_eq!(drawn.len(), 500);
    }

    #[test]
    fn worlds_look_like_places() {
        let (mut sand, mut sand_by_water) = (0, 0);
        for seed in 0..200 {
            let land = terrain(seed, 64);
            let total = land.cells.len();
            let share = |ground| land.count(ground) * 100 / total;
            let (water, forest, hill) = (
                share(Ground::Water),
                share(Ground::Forest),
                share(Ground::Hill),
            );
            assert!((1..=45).contains(&water), "seed {seed} is {water}% water");
            assert!(
                (2..=35).contains(&forest),
                "seed {seed} is {forest}% forest"
            );
            assert!(
                land.count(Ground::Hill) > 0 && hill <= 12,
                "seed {seed} has {hill}% hill"
            );
            assert!(
                share(Ground::Grass) >= 35,
                "seed {seed} is only {}% grass",
                share(Ground::Grass)
            );
            assert!(land.count(Ground::Sand) > 0, "seed {seed} has no beach");
            // The middle half of the map is mostly somewhere to build.
            let middle: Vec<Ground> = (16..48)
                .flat_map(|y| (16..48).map(move |x| (x, y)))
                .filter_map(|(x, y)| land.get(x, y))
                .collect();
            let open = middle.iter().filter(|ground| ground.is_open()).count();
            assert!(
                open * 100 / middle.len() >= 70,
                "seed {seed}: only {open} of {} middle tiles open",
                middle.len()
            );
            // Nothing but grass for a few tiles round the very middle, which is where the highway ends.
            for (x, y) in [
                (32, 32),
                (30, 32),
                (34, 32),
                (32, 30),
                (32, 34),
                (28, 28),
                (36, 36),
            ] {
                assert!(
                    land.get(x, y).unwrap().is_open(),
                    "seed {seed} has no room at {x},{y}"
                );
            }
            let wet = Wet {
                size: 64,
                cells: land.cells.iter().map(|g| *g == Ground::Water).collect(),
            };
            for y in 0..64usize {
                for x in 0..64usize {
                    if land.get(x, y) == Some(Ground::Sand) {
                        sand += 1;
                        sand_by_water += usize::from(wet.near(x as i64, y as i64, 3));
                    }
                }
            }
        }
        assert!(
            sand_by_water * 100 >= sand * 98,
            "{sand_by_water} of {sand} sand tiles are beside water"
        );
    }

    #[test]
    fn land_round_trips_through_text() {
        for seed in 0..60 {
            let land = terrain(seed, 64);
            let rows = encode_land(&land);
            assert_eq!(rows.len(), 64);
            for row in &rows {
                // A count, a letter, a count, a letter...
                let letters = row.chars().filter(|c| c.is_ascii_alphabetic()).count();
                assert!(
                    letters >= 1
                        && row
                            .chars()
                            .all(|c| c.is_ascii_digit() || "gfwsh".contains(c)),
                    "{row}"
                );
                assert!(
                    row.starts_with(|c: char| c.is_ascii_digit())
                        && row.ends_with(|c: char| c.is_ascii_alphabetic())
                );
            }
            assert_eq!(decode_land(&rows), Ok(land));
        }
        let rows = encode_land(&terrain(COMMONS_SEED, COMMONS_SIZE));
        assert_eq!(decode_land(&rows).unwrap().size(), COMMONS_SIZE);
    }

    #[test]
    fn land_text_looks_like_the_documented_format() {
        let land = picture(&["gggg", "ffww", "gssg", "hhhh"]);
        assert_eq!(encode_land(&land), ["4g", "2f2w", "1g2s1g", "4h"]);
        let wide = decode_land(&["40g3f10w".to_string()]);
        assert!(wide.is_err(), "one row can't be 53 across");
        let rows: Vec<String> = ["2g", "1g1w"].iter().map(|row| row.to_string()).collect();
        let land = decode_land(&rows).unwrap();
        assert_eq!(land.get(1, 1), Some(Ground::Water));
        assert_eq!(land.get(2, 0), None);
    }

    #[test]
    fn bad_land_text_is_refused() {
        let rows = |texts: &[&str]| {
            texts
                .iter()
                .map(|text| text.to_string())
                .collect::<Vec<String>>()
        };
        let huge = "9".repeat(10_000);
        let bad = [
            rows(&[]),
            rows(&["2g", "2"]),
            rows(&["2g", "g2"]),
            rows(&["2g", "0g2g"]),
            rows(&["2g", "1g1x"]),
            rows(&["2g", "1g1G"]),
            rows(&["2g", "3g"]),
            rows(&["2g", "1g"]),
            rows(&["2g", "1g 1g"]),
            rows(&["2g", "1g-1g"]),
            rows(&["2g", &huge]),
            rows(&["2g", &format!("{huge}g")]),
            rows(&["٢g", "2g"]),
            rows(&["", ""]),
            vec!["1g".to_string(); MAX_SIZE + 1],
            vec!["x".repeat(2_000_000); 2],
        ];
        for case in bad {
            assert!(
                decode_land(&case).is_err(),
                "{:?}",
                case.iter().map(|row| row.len()).collect::<Vec<_>>()
            );
        }
    }

    // The highway

    fn check_highway(seed: u64, size: usize) {
        let land = terrain(seed, size);
        let road = highway(seed, &land);
        assert!(
            road.len() >= size / 2,
            "seed {seed}: highway of {} tiles",
            road.len()
        );
        assert!(
            is_edge(&land, road[0]),
            "seed {seed}: starts at {:?}",
            road[0]
        );
        assert_eq!(*road.last().unwrap(), land.centre(), "seed {seed}");
        for pair in road.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let apart =
                (i32::from(a.0) - i32::from(b.0)).abs() + (i32::from(a.1) - i32::from(b.1)).abs();
            assert_eq!(apart, 1, "seed {seed}: {a:?} then {b:?}");
        }
        let distinct: BTreeSet<&Pos> = road.iter().collect();
        assert_eq!(
            distinct.len(),
            road.len(),
            "seed {seed}: the highway crosses itself"
        );
        for pos in &road {
            assert!(
                land.at(*pos).unwrap().is_open(),
                "seed {seed}: highway over {:?} at {pos:?}",
                land.at(*pos)
            );
        }
        assert_eq!(highway(seed, &land), road);
    }

    #[test]
    fn the_highway_reaches_the_middle_from_an_edge_for_many_seeds() {
        for seed in 0..500 {
            check_highway(seed, 64);
        }
        for seed in [
            0,
            1,
            2,
            3,
            4,
            5,
            6,
            7,
            307,
            999,
            u64::MAX,
            u64::MAX - 1,
            0xDEAD_BEEF,
        ] {
            check_highway(seed, 128);
        }
        for size in [16, 17, 20, 33, 50, 100, 200] {
            for seed in 0..6 {
                check_highway(seed * 1_000_003, size);
            }
        }
        check_highway(3, 256);
    }

    #[test]
    fn the_highway_is_laid_as_ordinary_roads_with_no_builder() {
        let land = terrain(COMMONS_SEED, COMMONS_SIZE);
        let tiles = start_tiles(COMMONS_SEED, &land);
        assert_eq!(tiles.len(), highway(COMMONS_SEED, &land).len());
        assert!(tiles.values().all(|tile| *tile == tile_of_road()));
        let stats = stats(&tiles);
        assert_eq!(
            (stats.population, stats.jobs, stats.stage),
            (0, 0, "hamlet")
        );
        assert_eq!(stats.counts.roads as usize, tiles.len());
    }

    fn tile_of_road() -> Tile {
        tile(Kind::Road, 0, None)
    }

    #[test]
    fn highways_differ_between_seeds() {
        let roads: HashSet<u64> = (0..100)
            .map(|seed| highway_hash(&highway(seed, &terrain(seed, 64))))
            .collect();
        // On open land a highway is settled by where on the edge it starts, and a 64 by 64 map has
        // about 250 such places, so a few seeds out of a hundred will share one.
        assert!(
            roads.len() >= 75,
            "only {} different highways in 100 seeds",
            roads.len()
        );
        let sides: HashSet<usize> = (0..100)
            .map(|seed| {
                let land = terrain(seed, 64);
                let (x, y) = highway(seed, &land)[0];
                usize::from(x == 0)
                    + 2 * usize::from(x == 63)
                    + 4 * usize::from(y == 0)
                    + 8 * usize::from(y == 63)
            })
            .collect();
        assert!(
            sides.len() >= 4,
            "the highway always comes in from the same few places: {sides:?}"
        );
    }

    #[test]
    fn carving_a_way_in_is_rare_and_always_enough() {
        let mut carved = 0;
        for seed in 0..500 {
            let raw = raw_terrain(seed, 64);
            if route(seed, &raw, false).is_none() {
                carved += 1;
                let land = terrain(seed, 64);
                assert!(!highway(seed, &land).is_empty());
                let changed = raw
                    .cells
                    .iter()
                    .zip(&land.cells)
                    .filter(|(a, b)| a != b)
                    .count();
                assert!(changed <= 64, "seed {seed} carved {changed} tiles");
            }
        }
        eprintln!("{carved} of 500 seeds needed land cut for the highway");
        assert!(
            carved <= 25,
            "{carved} of 500 seeds needed land cut for the highway"
        );
    }

    #[test]
    fn a_walled_in_middle_is_opened_with_the_least_cut() {
        let mut land = lawn(33);
        for y in 0..33usize {
            for x in 0..33usize {
                let ring = x.abs_diff(16).max(y.abs_diff(16));
                if ring == 6 || ring == 7 {
                    land.set(x, y, Ground::Water);
                }
                if ring == 8 {
                    land.set(x, y, Ground::Hill);
                }
            }
        }
        assert!(highway(5, &land).is_empty(), "no road without bridges");
        let before = land.clone();
        open_highway(5, &mut land);
        let road = highway(5, &land);
        assert_eq!(road.last(), Some(&(16, 16)));
        assert!(is_edge(&land, road[0]));
        let cut = before
            .cells
            .iter()
            .zip(&land.cells)
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            cut, 3,
            "one hill and two water tiles, the narrowest way through"
        );
    }

    #[test]
    fn highway_needs_open_ground_at_the_middle() {
        let mut land = lawn(20);
        land.set(10, 10, Ground::Water);
        assert!(highway(1, &land).is_empty());
    }

    // Moves: roads

    #[test]
    fn a_road_costs_a_move_a_tile_and_must_touch_a_road() {
        let land = picture(&YARD);
        let mut tiles = highway_row(1, 0..=8);
        let plan = plan_road(&land, &tiles, 7, span(8, 2, 8, 6)).unwrap();
        assert_eq!((plan.cost, plan.skipped, plan.changes.len()), (5, 0, 5));
        assert_eq!(
            plan.changes[0],
            Change::Set((8, 2), tile(Kind::Road, 0, Some(7)))
        );
        assert_eq!(plan.changes[4].pos(), (8, 6));
        // Along from the end of the highway, and out from its side.
        assert_eq!(
            plan_road(&land, &tiles, 7, span(9, 1, 11, 1)).unwrap().cost,
            3
        );
        assert_eq!(
            plan_road(&land, &tiles, 7, span(4, 2, 4, 2)).unwrap().cost,
            1
        );
        // Only the corner touches, and corners aren't touching.
        assert_eq!(
            plan_road(&land, &tiles, 7, span(9, 2, 9, 4)),
            Err("a road has to touch an existing road")
        );
        assert_eq!(
            plan_road(&land, &tiles, 7, span(9, 3, 9, 5)),
            Err("a road has to touch an existing road")
        );
        assert_eq!(
            plan_road(&land, &tiles, 7, span(0, 8, 5, 8)),
            Err("a road has to touch an existing road")
        );
        // Once the first is down, the next can join it.
        done(Ok(plan), &mut tiles);
        assert_eq!(
            plan_road(&land, &tiles, 9, span(9, 2, 10, 2)).unwrap().cost,
            2
        );
    }

    #[test]
    fn roads_already_there_are_kept_and_not_charged() {
        let land = picture(&YARD);
        let tiles = highway_row(1, 0..=8);
        let plan = plan_road(&land, &tiles, 7, span(6, 1, 11, 1)).unwrap();
        assert_eq!((plan.cost, plan.skipped, plan.changes.len()), (3, 3, 3));
        assert_eq!(
            plan.changes.iter().map(Change::pos).collect::<Vec<_>>(),
            [(9, 1), (10, 1), (11, 1)]
        );
        assert_eq!(
            plan_road(&land, &tiles, 7, span(2, 1, 5, 1)),
            Err("that road is already there")
        );
        // A line that starts on a road is joined by it, even when no new tile touches another road.
        let tiles = highway_row(1, 0..=8);
        assert!(plan_road(&land, &tiles, 7, span(8, 1, 8, 2)).is_ok());
    }

    #[test]
    fn a_road_is_a_straight_line_of_at_most_sixteen_tiles() {
        let land = lawn(32);
        let tiles = highway_row(5, 0..=2);
        assert_eq!(
            plan_road(&land, &tiles, 1, span(3, 5, 18, 5)).unwrap().cost,
            16
        );
        assert_eq!(
            plan_road(&land, &tiles, 1, span(3, 5, 19, 5)),
            Err("a road is at most 16 tiles long")
        );
        let tiles = highway_row(5, 0..=2);
        assert_eq!(
            plan_road(&land, &tiles, 1, span(2, 6, 2, 21)).unwrap().cost,
            16
        );
        assert_eq!(
            plan_road(&land, &tiles, 1, span(2, 6, 2, 22)),
            Err("a road is at most 16 tiles long")
        );
        assert_eq!(
            plan_road(&land, &tiles, 1, span(3, 5, 3, 5)).unwrap().cost,
            1
        );
        assert_eq!(
            plan_road(&land, &tiles, 1, span(3, 5, 4, 6)),
            Err("a road runs straight, not diagonally")
        );
        assert_eq!(
            plan_road(&land, &tiles, 1, span(3, 5, 2, 5)),
            Err("give the top left corner first")
        );
        assert_eq!(
            plan_road(&land, &tiles, 1, span(3, 6, 3, 5)),
            Err("give the top left corner first")
        );
    }

    #[test]
    fn a_road_stops_at_anything_in_the_way() {
        let land = picture(&YARD);
        let mut tiles = highway_row(1, 0..=8);
        done(plan_road(&land, &tiles, 7, span(3, 2, 3, 8)), &mut tiles);
        assert_eq!(
            plan_road(&land, &tiles, 7, span(4, 5, 7, 5)),
            Err("there is water or a hill in the way")
        );
        assert_eq!(
            plan_road(&land, &tiles, 7, span(4, 2, 4, 4)),
            Err("there is forest in the way: bulldoze it first")
        );
        assert_eq!(
            plan_road(&land, &tiles, 7, span(9, 1, 9, 1)).unwrap().cost,
            1
        );
        assert_eq!(
            plan_road(&land, &tiles, 7, span(11, 1, 11, 5)),
            Err("a road has to touch an existing road")
        );
        // Hill and sand, built zone and park.
        done(plan_road(&land, &tiles, 7, span(9, 1, 11, 1)), &mut tiles);
        assert_eq!(
            plan_road(&land, &tiles, 7, span(11, 2, 11, 6)),
            Err("there is water or a hill in the way")
        );
        assert!(
            plan_road(&land, &tiles, 7, span(11, 2, 11, 5)).is_ok(),
            "sand is fine"
        );
        done(
            plan_zone(&land, &tiles, 8, Kind::Homes, span(1, 2, 2, 3)),
            &mut tiles,
        );
        assert_eq!(
            plan_road(&land, &tiles, 7, span(0, 2, 2, 2)),
            Err("something is built in the way")
        );
        // Cleared forest can be built on.
        tiles.insert((4, 3), tile(Kind::Cleared, 0, Some(1)));
        tiles.insert((4, 2), tile(Kind::Road, 0, Some(1)));
        assert!(plan_road(&land, &tiles, 7, span(4, 3, 4, 3)).is_ok());
    }

    // Moves: zones, parks, bulldozing

    #[test]
    fn a_zone_covers_a_rectangle_of_up_to_five_by_five() {
        let land = picture(&YARD);
        let tiles = Tiles::new();
        let plan = plan_zone(&land, &tiles, 3, Kind::Shops, span(0, 7, 4, 11)).unwrap();
        assert_eq!((plan.cost, plan.skipped, plan.changes.len()), (25, 0, 25));
        assert_eq!(
            plan.changes[0],
            Change::Set((0, 7), tile(Kind::Shops, 0, Some(3)))
        );
        assert_eq!(
            plan_zone(&land, &tiles, 3, Kind::Homes, span(0, 7, 5, 7)),
            Err("a zone is at most 5 by 5")
        );
        assert_eq!(
            plan_zone(&land, &tiles, 3, Kind::Homes, span(0, 6, 0, 11)),
            Err("a zone is at most 5 by 5")
        );
        assert_eq!(
            plan_zone(&land, &tiles, 3, Kind::Homes, span(4, 4, 3, 4)),
            Err("give the top left corner first")
        );
        assert_eq!(
            plan_zone(&land, &tiles, 3, Kind::Homes, span(4, 4, 4, 3)),
            Err("give the top left corner first")
        );
        assert_eq!(
            plan_zone(&land, &tiles, 3, Kind::Homes, span(11, 11, 12, 12)),
            Err("that is off the map")
        );
        for kind in [Kind::Road, Kind::Park, Kind::Cleared] {
            assert_eq!(
                plan_zone(&land, &tiles, 3, kind, span(0, 7, 0, 7)),
                Err("a zone is homes, shops or workshops")
            );
        }
        // No road needed to zone.
        assert!(plan_zone(&land, &tiles, 3, Kind::Workshops, span(0, 11, 0, 11)).is_ok());
    }

    #[test]
    fn zones_skip_what_is_built_or_cannot_be_and_do_not_charge_for_it() {
        let land = picture(&YARD);
        let mut tiles = Tiles::new();
        // 20 tiles: six of forest and four of water are left alone.
        let plan = plan_zone(&land, &tiles, 3, Kind::Homes, span(3, 3, 7, 6)).unwrap();
        assert_eq!((plan.cost, plan.skipped, plan.changes.len()), (10, 10, 10));
        // Nine built, then a bigger rectangle over them.
        done(
            plan_zone(&land, &tiles, 3, Kind::Homes, span(0, 7, 2, 9)),
            &mut tiles,
        );
        let plan = plan_zone(&land, &tiles, 4, Kind::Shops, span(0, 7, 4, 9)).unwrap();
        assert_eq!((plan.cost, plan.skipped), (6, 9));
        assert!(plan
            .changes
            .iter()
            .all(|change| matches!(change, Change::Set(_, t) if t.player == Some(4))));
        assert_eq!(
            plan_zone(&land, &tiles, 4, Kind::Shops, span(0, 7, 2, 9)),
            Err("nothing there can be built on")
        );
        // A road counts as built too.
        tiles.extend(highway_row(10, 0..=3));
        assert_eq!(
            plan_zone(&land, &tiles, 4, Kind::Shops, span(0, 10, 3, 10)),
            Err("nothing there can be built on")
        );
        // Cleared forest takes a zone.
        tiles.insert((4, 3), tile(Kind::Cleared, 0, Some(1)));
        assert_eq!(
            plan_zone(&land, &tiles, 3, Kind::Homes, span(4, 3, 4, 3))
                .unwrap()
                .cost,
            1
        );
    }

    #[test]
    fn a_park_is_one_tile_for_two_moves() {
        let land = picture(&YARD);
        let mut tiles = highway_row(1, 0..=8);
        let plan = plan_park(&land, &tiles, 5, 2, 7).unwrap();
        assert_eq!(
            (plan.cost, plan.changes.clone()),
            (2, vec![Change::Set((2, 7), tile(Kind::Park, 0, Some(5)))])
        );
        assert_eq!(
            plan_park(&land, &tiles, 5, 10, 5).unwrap().cost,
            2,
            "sand is fine"
        );
        assert_eq!(
            plan_park(&land, &tiles, 5, 4, 3),
            Err("there is forest in the way: bulldoze it first")
        );
        assert_eq!(
            plan_park(&land, &tiles, 5, 5, 5),
            Err("there is water or a hill in the way")
        );
        assert_eq!(
            plan_park(&land, &tiles, 5, 10, 6),
            Err("there is water or a hill in the way")
        );
        assert_eq!(
            plan_park(&land, &tiles, 5, 2, 1),
            Err("that tile is already built on")
        );
        assert_eq!(
            plan_park(&land, &tiles, 5, 12, 1),
            Err("that is off the map")
        );
        done(Ok(plan), &mut tiles);
        assert_eq!(
            plan_park(&land, &tiles, 6, 2, 7),
            Err("that tile is already built on")
        );
        tiles.insert((5, 3), tile(Kind::Cleared, 0, Some(1)));
        assert!(plan_park(&land, &tiles, 5, 5, 3).is_ok());
    }

    #[test]
    fn you_bulldoze_what_you_built_and_nothing_else() {
        let land = picture(&YARD);
        let mut tiles = highway_row(1, 0..=8);
        done(plan_road(&land, &tiles, 7, span(3, 2, 3, 4)), &mut tiles);
        done(
            plan_zone(&land, &tiles, 8, Kind::Homes, span(0, 7, 1, 7)),
            &mut tiles,
        );
        let plan = plan_bulldoze(&land, &tiles, 7, 3, 3, &nobody_gone).unwrap();
        assert_eq!((plan.cost, plan.changes), (1, vec![Change::Remove((3, 3))]));
        assert_eq!(
            plan_bulldoze(&land, &tiles, 8, 3, 3, &nobody_gone),
            Err("you can only bulldoze what you built")
        );
        assert_eq!(
            plan_bulldoze(&land, &tiles, 8, 0, 7, &nobody_gone)
                .unwrap()
                .changes,
            [Change::Remove((0, 7))]
        );
        // A zone that has grown is still yours to bulldoze.
        tiles.insert((0, 7), tile(Kind::Homes, 3, Some(8)));
        assert!(plan_bulldoze(&land, &tiles, 8, 0, 7, &nobody_gone).is_ok());
        // Nothing there, or nothing to do.
        assert_eq!(
            plan_bulldoze(&land, &tiles, 8, 9, 9, &nobody_gone),
            Err("there is nothing to bulldoze there")
        );
        assert_eq!(
            plan_bulldoze(&land, &tiles, 8, 5, 5, &nobody_gone),
            Err("there is nothing to bulldoze there")
        );
        assert_eq!(
            plan_bulldoze(&land, &tiles, 8, 10, 6, &nobody_gone),
            Err("there is nothing to bulldoze there")
        );
        assert_eq!(
            plan_bulldoze(&land, &tiles, 8, -1, 6, &nobody_gone),
            Err("that is off the map")
        );
    }

    #[test]
    fn what_a_builder_left_behind_can_be_cleared_if_it_never_grew() {
        let land = lawn(10);
        let mut tiles = Tiles::new();
        tiles.insert((1, 1), tile(Kind::Homes, 0, Some(7)));
        tiles.insert((2, 1), tile(Kind::Homes, 1, Some(7)));
        tiles.insert((3, 1), tile(Kind::Road, 0, Some(7)));
        tiles.insert((4, 1), tile(Kind::Park, 0, Some(7)));
        tiles.insert((5, 1), tile(Kind::Road, 0, None));
        let gone = |player: u64| player == 7;
        for x in [1, 3, 4] {
            let plan = plan_bulldoze(&land, &tiles, 9, x, 1, &gone).unwrap();
            assert_eq!(
                (plan.cost, plan.changes),
                (1, vec![Change::Remove((x as u16, 1))])
            );
        }
        assert_eq!(
            plan_bulldoze(&land, &tiles, 9, 2, 1, &gone),
            Err("you can only bulldoze what you built")
        );
        assert_eq!(
            plan_bulldoze(&land, &tiles, 9, 1, 1, &nobody_gone),
            Err("you can only bulldoze what you built")
        );
        // The highway has no builder to be gone, so nobody can ever take it down.
        let everyone_gone = |_: u64| true;
        assert_eq!(
            plan_bulldoze(&land, &tiles, 9, 5, 1, &everyone_gone),
            Err("you can only bulldoze what you built")
        );
        assert_eq!(
            plan_bulldoze(&land, &tiles, 7, 5, 1, &everyone_gone),
            Err("you can only bulldoze what you built")
        );
    }

    #[test]
    fn forest_is_cleared_by_bulldozing_and_stays_clearable_ground() {
        let land = picture(&YARD);
        let mut tiles = highway_row(1, 0..=8);
        let plan = plan_bulldoze(&land, &tiles, 7, 4, 3, &nobody_gone).unwrap();
        assert_eq!(
            (plan.cost, plan.changes.clone()),
            (
                1,
                vec![Change::Set((4, 3), tile(Kind::Cleared, 0, Some(7)))]
            )
        );
        done(Ok(plan), &mut tiles);
        assert_eq!(
            plan_bulldoze(&land, &tiles, 7, 4, 3, &nobody_gone),
            Err("that forest is already cleared")
        );
        // Build on it, then pull the building down: the ground is cleared, not forest again.
        done(plan_road(&land, &tiles, 8, span(4, 2, 4, 3)), &mut tiles);
        assert_eq!(tiles[&(4, 3)], tile(Kind::Road, 0, Some(8)));
        let plan = plan_bulldoze(&land, &tiles, 8, 4, 3, &nobody_gone).unwrap();
        assert_eq!(
            plan.changes,
            [Change::Set((4, 3), tile(Kind::Cleared, 0, Some(8)))]
        );
        // Someone else's road on forest isn't yours to clear.
        assert!(plan_bulldoze(&land, &tiles, 7, 4, 3, &nobody_gone).is_err());
    }

    #[test]
    fn untrusted_numbers_never_panic_and_never_escape_the_map() {
        let land = picture(&YARD);
        let mut tiles = highway_row(1, 0..=8);
        tiles.insert((9, 9), tile(Kind::Homes, 2, Some(7)));
        let nasty = [
            i64::MIN,
            i64::MIN + 1,
            -(1 << 40),
            -65_537,
            -1,
            0,
            1,
            4,
            11,
            12,
            255,
            256,
            65_535,
            65_536,
            1 << 32,
            i64::MAX - 1,
            i64::MAX,
        ];
        let size = land.size() as i64;
        let on_map = |change: &Change| {
            let (x, y) = change.pos();
            i64::from(x) < size && i64::from(y) < size
        };
        let (mut allowed, mut refused) = (0, 0);
        for &x in &nasty {
            for &y in &nasty {
                let single = [
                    plan_park(&land, &tiles, 7, x, y),
                    plan_bulldoze(&land, &tiles, 7, x, y, &|player| player == 7),
                ];
                for &x2 in &nasty {
                    for &y2 in &nasty {
                        let s = span(x, y, x2, y2);
                        let batch = [
                            plan_road(&land, &tiles, 7, s),
                            plan_zone(&land, &tiles, 7, Kind::Homes, s),
                            plan_zone(&land, &tiles, 7, Kind::Road, s),
                        ];
                        for planned in &batch {
                            check_plan(planned, &on_map, &mut allowed, &mut refused);
                        }
                    }
                }
                for planned in &single {
                    check_plan(planned, &on_map, &mut allowed, &mut refused);
                }
            }
        }
        assert!(
            allowed > 0 && refused > 1000,
            "{allowed} allowed, {refused} refused"
        );
    }

    fn check_plan(
        planned: &Planned,
        on_map: &dyn Fn(&Change) -> bool,
        allowed: &mut u32,
        refused: &mut u32,
    ) {
        match planned {
            Ok(plan) => {
                *allowed += 1;
                assert!(plan.changes.iter().all(on_map), "{plan:?}");
                assert!(plan.cost >= 1 && plan.cost <= 25 * 2, "{plan:?}");
                assert!(!plan.changes.is_empty() && plan.changes.len() <= 25);
            }
            Err(reason) => {
                *refused += 1;
                assert!(!reason.is_empty() && reason.len() < 60, "{reason}");
            }
        }
    }

    #[test]
    fn a_refused_move_leaves_the_world_alone() {
        let land = picture(&YARD);
        let tiles = highway_row(1, 0..=8);
        let before = tiles.clone();
        assert!(plan_road(&land, &tiles, 7, span(0, 8, 5, 8)).is_err());
        assert!(plan_zone(&land, &tiles, 7, Kind::Homes, span(0, 1, 2, 1)).is_err());
        assert_eq!(tiles, before);
    }

    // Moves: side streets, your own lots, and roads that grown buildings hang on

    fn kept(tiles: &Tiles, x: u16, y: u16) -> bool {
        is_reserved(tiles, (x, y))
    }

    // W15
    #[test]
    fn a_side_street_starts_at_every_sixth_tile_beside_a_street_and_runs_three_tiles_out() {
        let tiles = highway_row(10, 0..=40);
        // North of the road a street starts where x + 9 is a multiple of 6, and south where x + 11 is.
        for x in 0..=40u16 {
            let north = (x + 9) % 6 == 0;
            let south = (x + 11) % 6 == 0;
            for (y, starts) in [(9, north), (8, north), (7, north), (11, south), (12, south), (13, south)] {
                assert_eq!(kept(&tiles, x, y), starts, "({x}, {y})");
            }
            // Not the fourth tile out.
            assert!(!kept(&tiles, x, 6) && !kept(&tiles, x, 14), "({x}) is past the end");
        }
        // Every sixth tile, so about a sixth of the tiles beside the road.
        let beside = (0..=40u16).filter(|x| kept(&tiles, *x, 9)).count();
        assert!((6..=7).contains(&beside), "{beside} streets start on the north side");
    }

    // W15
    #[test]
    fn only_a_road_with_no_builder_keeps_a_side_street() {
        let mut tiles = Tiles::new();
        // A player's road, long and straight, keeps nothing at all.
        for x in 10..=40u16 {
            tiles.insert((x, 10), tile(Kind::Road, 0, Some(3)));
        }
        // A zone with no builder keeps nothing either: only streets do.
        tiles.insert((20, 20), tile(Kind::Homes, 1, None));
        for y in 0..64u16 {
            for x in 0..64u16 {
                assert!(!kept(&tiles, x, y), "({x}, {y}) is kept by something that is no street");
            }
        }
        // The edge of the map is no trouble, nor are positions that can't exist.
        let edge = highway_row(0, 0..=5);
        assert!(is_reserved(&edge, (5, 1)) && !is_reserved(&edge, (4, 1)));
        assert!(!is_reserved(&Tiles::new(), (0, 0)));
        assert!(!is_reserved(&edge, (u16::MAX, u16::MAX)) && !is_reserved(&edge, (0, u16::MAX)));
    }

    // W15
    #[test]
    fn zones_and_parks_skip_a_side_street_without_charging_and_say_so_when_nothing_is_left() {
        let land = lawn(32);
        let tiles = highway_row(10, 0..=20);
        // (3, 9) starts a street north of the road: (3, 9), (3, 8) and (3, 7) are kept.
        assert!(kept(&tiles, 3, 9) && kept(&tiles, 3, 8) && kept(&tiles, 3, 7));
        let all_kept = Err("that strip is kept for a side street: roads only");
        assert_eq!(plan_zone(&land, &tiles, 1, Kind::Homes, span(3, 7, 3, 9)), all_kept);
        assert_eq!(plan_zone(&land, &tiles, 1, Kind::Shops, span(3, 8, 3, 8)), all_kept);
        assert_eq!(plan_park(&land, &tiles, 1, 3, 9), all_kept);
        assert_eq!(plan_park(&land, &tiles, 1, 3, 7), all_kept);
        // A rectangle half on the street builds the other half and charges for that.
        let plan = plan_zone(&land, &tiles, 1, Kind::Homes, span(2, 5, 4, 8)).unwrap();
        assert_eq!((plan.cost, plan.skipped, plan.changes.len()), (10, 2, 10));
        assert!(plan.changes.iter().all(|change| change.pos() != (3, 8) && change.pos() != (3, 7)));
        // Next door is fine.
        assert!(plan_park(&land, &tiles, 1, 4, 9).is_ok());
        assert!(plan_zone(&land, &tiles, 1, Kind::Homes, span(4, 7, 4, 9)).is_ok());
        // And a road may use the street: it is what it is for.
        let road = plan_road(&land, &tiles, 1, span(3, 7, 3, 9)).unwrap();
        assert_eq!((road.cost, road.skipped, road.changes.len()), (3, 0, 3));
        // A street someone has built on is built on, and says so.
        let mut built = tiles.clone();
        built.insert((3, 9), tile(Kind::Road, 0, Some(2)));
        assert_eq!(plan_park(&land, &built, 1, 3, 9), Err("that tile is already built on"));
    }

    // W15
    #[test]
    fn a_crowd_of_one_tile_zones_cannot_wall_a_highway_in_and_a_road_can_still_leave_it() {
        let land = lawn(64);
        let mut tiles = highway_row(30, 0..=40);
        let mut beside: Vec<Pos> = Vec::new();
        for x in 0..=40u16 {
            beside.extend([(x, 29), (x, 31)]);
        }
        beside.push((41, 30));
        let (mut built, mut refused) = (0, 0);
        for (n, &(x, y)) in beside.iter().enumerate() {
            let kind = [Kind::Homes, Kind::Shops, Kind::Workshops][n % 3];
            let at = span(i64::from(x), i64::from(y), i64::from(x), i64::from(y));
            match plan_zone(&land, &tiles, 1, kind, at) {
                Ok(plan) => {
                    built += 1;
                    apply(&mut tiles, &plan.changes);
                }
                Err(_) => refused += 1,
            }
        }
        assert!(built > 60 && refused >= 10, "{built} built, {refused} refused");
        // What is left beside the highway is the start of every side street.
        let open: Vec<Pos> = beside.iter().copied().filter(|pos| !tiles.contains_key(pos)).collect();
        assert_eq!(open.len(), refused);
        assert!(open.iter().all(|pos| is_reserved(&tiles, *pos)));
        // Someone else can still leave the highway there, three tiles out, and nothing was charged
        // for the zones that were refused.
        let (x, y) = open[0];
        let (top, bottom) = if y < 30 { (y - 2, y) } else { (y, y + 2) };
        let road = plan_road(&land, &tiles, 2, span(i64::from(x), i64::from(top), i64::from(x), i64::from(bottom))).unwrap();
        assert_eq!(road.cost, 3);
        apply(&mut tiles, &road.changes);
        assert!(is_reserved(&tiles, (x, y)), "it stays kept after a road is built on it");
    }

    // W15
    #[test]
    fn the_founding_town_stands_on_land_it_keeps_and_is_not_touched() {
        let (_, tiles) = founded(307, 128);
        let on_kept: Vec<Pos> = tiles
            .iter()
            .filter(|(pos, t)| t.kind != Kind::Road && is_reserved(&tiles, **pos))
            .map(|(pos, _)| *pos)
            .collect();
        eprintln!("{} tiles of the town stand on land kept for a side street", on_kept.len());
        let land = terrain(307, 128);
        for (x, y) in on_kept {
            // Built is built: it is not skipped as a street, and nobody can take it away.
            assert_eq!(
                plan_park(&land, &tiles, 1, i64::from(x), i64::from(y)),
                Err("that tile is already built on")
            );
            assert!(plan_bulldoze(&land, &tiles, 1, i64::from(x), i64::from(y), &|_| true).is_err());
        }
    }

    // W17
    #[test]
    fn a_road_under_a_grown_building_cannot_be_bulldozed_by_anyone_directly_or_further_up_the_branch() {
        let land = lawn(32);
        let needed = Err("a grown building depends on that road");
        // A branch of three tiles off the end of the highway. The workshop is served by (7, 10) alone.
        let mut tiles = highway_row(10, 0..=4);
        for x in 5..=7u16 {
            tiles.insert((x, 10), tile(Kind::Road, 0, Some(1)));
        }
        tiles.insert((10, 13), tile(Kind::Workshops, 1, Some(2)));
        // Directly: the builder takes down the one road tile in reach of the building.
        assert_eq!(plan_bulldoze(&land, &tiles, 1, 7, 10, &nobody_gone), needed);
        // Further up the branch: the middle and the first tile both leave the end cut off.
        assert_eq!(plan_bulldoze(&land, &tiles, 1, 6, 10, &nobody_gone), needed);
        assert_eq!(plan_bulldoze(&land, &tiles, 1, 5, 10, &nobody_gone), needed);
        // Nobody else can do what the builder can't, not even someone who may clear a gone builder's roads.
        let everyone_gone = |_: u64| true;
        for x in 5..=7 {
            assert_eq!(plan_bulldoze(&land, &tiles, 9, x, 10, &everyone_gone), needed, "{x}");
        }
        // It is the building that counts: level 0 depends on nothing, and then the road goes.
        let mut lot = tiles.clone();
        lot.insert((10, 13), tile(Kind::Workshops, 0, Some(2)));
        for x in [7, 6, 5] {
            assert!(plan_bulldoze(&land, &lot, 1, x, 10, &nobody_gone).is_ok(), "{x}");
        }
        // A park isn't a building.
        let mut park = tiles.clone();
        park.insert((10, 13), tile(Kind::Park, 0, Some(2)));
        assert!(plan_bulldoze(&land, &park, 1, 7, 10, &nobody_gone).is_ok());
        // A building with another road in reach loses nothing, so the spare road can go.
        let mut spare = tiles.clone();
        spare.insert((10, 10), tile(Kind::Road, 0, None));
        assert!(plan_bulldoze(&land, &spare, 1, 7, 10, &nobody_gone).is_ok());
        // A building the branch no longer serves depends on nothing that is left of it.
        let mut cut = tiles.clone();
        cut.remove(&(5, 10));
        assert!(plan_bulldoze(&land, &cut, 1, 7, 10, &nobody_gone).is_ok());
    }

    // W17
    #[test]
    fn a_dead_end_road_that_serves_nothing_grown_can_still_be_taken_down_by_its_builder() {
        let land = lawn(32);
        let mut tiles = highway_row(10, 0..=4);
        done(plan_road(&land, &tiles, 1, span(5, 10, 12, 10)), &mut tiles);
        // Lots that have not grown, and a grown shop that the highway serves by itself.
        tiles.extend(block(Kind::Homes, 0, 10..=12, 11..=12));
        tiles.insert((2, 12), tile(Kind::Shops, 3, Some(4)));
        for x in (5..=12).rev() {
            let plan = plan_bulldoze(&land, &tiles, 1, x, 10, &nobody_gone).unwrap();
            apply(&mut tiles, &plan.changes);
        }
        assert!(tiles.values().all(|t| t.kind != Kind::Road || t.player.is_none()));
        // With one grown home beside it, the road can be taken back to the last tile in reach of it.
        let mut tiles = highway_row(10, 0..=4);
        done(plan_road(&land, &tiles, 1, span(5, 10, 12, 10)), &mut tiles);
        tiles.insert((12, 12), tile(Kind::Homes, 1, Some(3)));
        for x in [12, 11, 10] {
            let plan = plan_bulldoze(&land, &tiles, 1, x, 10, &nobody_gone);
            apply(&mut tiles, &plan.expect("the home is still served").changes);
        }
        let needed = Err("a grown building depends on that road");
        assert_eq!(plan_bulldoze(&land, &tiles, 1, 9, 10, &nobody_gone), needed);
        assert_eq!(plan_bulldoze(&land, &tiles, 1, 8, 10, &nobody_gone), needed);
    }

    /// Grown buildings that a road joined to the highway is within reach of, worked out the long way:
    /// a flood along roads from the streets, then a look at every tile.
    fn served_the_long_way(tiles: &Tiles) -> BTreeSet<Pos> {
        let mut joined: BTreeSet<Pos> = tiles
            .iter()
            .filter(|(_, t)| t.kind == Kind::Road && t.player.is_none())
            .map(|(pos, _)| *pos)
            .collect();
        let mut queue: Vec<Pos> = joined.iter().copied().collect();
        while let Some(here) = queue.pop() {
            for next in neighbours(here).into_iter().flatten() {
                if tiles.get(&next).is_some_and(|t| t.kind == Kind::Road) && joined.insert(next) {
                    queue.push(next);
                }
            }
        }
        tiles
            .iter()
            .filter(|(_, t)| t.kind.is_zone() && t.level > 0)
            .filter(|(pos, _)| joined.iter().any(|j| j.0.abs_diff(pos.0) <= 3 && j.1.abs_diff(pos.1) <= 3))
            .map(|(pos, _)| *pos)
            .collect()
    }

    // W17
    #[test]
    fn a_road_is_refused_exactly_when_taking_it_down_would_cut_a_grown_building_off() {
        let land = lawn(48);
        let mut refused = 0;
        let mut allowed = 0;
        for seed in 0..40u64 {
            let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            let mut next = move || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                rng
            };
            let mut tiles = highway_row(24, 0..=30);
            for _ in 0..40 {
                let (x, y) = ((next() % 40) as i64, (next() % 12) as i64 + 18);
                let (x2, y2) = if next() % 2 == 0 {
                    (x + (next() % 8) as i64, y)
                } else {
                    (x, y + (next() % 8) as i64)
                };
                if let Ok(plan) = plan_road(&land, &tiles, 1, span(x, y, x2, y2)) {
                    apply(&mut tiles, &plan.changes);
                }
                if let Ok(plan) = plan_zone(&land, &tiles, 2, Kind::Homes, span(x, y, x + 1, y + 1)) {
                    apply(&mut tiles, &plan.changes);
                }
            }
            for t in tiles.values_mut().filter(|t| t.kind.is_zone()) {
                t.level = (next() % 2) as u8;
            }
            let before = served_the_long_way(&tiles);
            let roads: Vec<Pos> = tiles
                .iter()
                .filter(|(_, t)| t.kind == Kind::Road && t.player == Some(1))
                .map(|(pos, _)| *pos)
                .collect();
            for (x, y) in roads {
                let mut without = tiles.clone();
                without.remove(&(x, y));
                let cuts = !before.is_subset(&served_the_long_way(&without));
                match plan_bulldoze(&land, &tiles, 1, i64::from(x), i64::from(y), &nobody_gone) {
                    Ok(_) => {
                        allowed += 1;
                        assert!(!cuts, "seed {seed}: ({x}, {y}) was taken down and cut a building off");
                    }
                    Err(why) => {
                        refused += 1;
                        assert_eq!(why, "a grown building depends on that road");
                        assert!(cuts, "seed {seed}: ({x}, {y}) was refused and cut nothing off");
                    }
                }
            }
        }
        eprintln!("{allowed} roads could come down and {refused} could not");
        assert!(allowed > 100 && refused > 100, "{allowed} allowed, {refused} refused");
    }

    // W17
    #[test]
    fn you_can_lay_a_road_over_your_own_empty_lots_and_nothing_else() {
        let land = picture(&YARD);
        let mut tiles = highway_row(1, 0..=8);
        // Yours: two lots, a grown home, a park. Theirs: a lot.
        tiles.insert((2, 2), tile(Kind::Homes, 0, Some(7)));
        tiles.insert((3, 2), tile(Kind::Shops, 0, Some(7)));
        tiles.insert((5, 2), tile(Kind::Workshops, 0, Some(8)));
        tiles.insert((6, 2), tile(Kind::Homes, 1, Some(7)));
        tiles.insert((7, 2), tile(Kind::Park, 0, Some(7)));
        let plan = plan_road(&land, &tiles, 7, span(2, 2, 3, 2)).unwrap();
        assert_eq!((plan.cost, plan.skipped, plan.changes.len()), (2, 0, 2), "charged like any road");
        assert_eq!(plan.changes[0], Change::Set((2, 2), tile(Kind::Road, 0, Some(7))));
        for (x1, x2) in [(2, 5), (5, 5), (3, 6), (6, 6), (7, 7), (2, 7)] {
            assert_eq!(
                plan_road(&land, &tiles, 7, span(x1, 2, x2, 2)),
                Err("something is built in the way"),
                "{x1} to {x2}"
            );
        }
        // Nobody else's lot is yours.
        assert_eq!(
            plan_road(&land, &tiles, 9, span(2, 2, 3, 2)),
            Err("something is built in the way")
        );
        // The lot is gone and a road stands there, joined to the roads beside it.
        done(Ok(plan), &mut tiles);
        assert_eq!(tiles[&(3, 2)], tile(Kind::Road, 0, Some(7)));
        assert_eq!(levels(&tiles, Kind::Homes), 1);
        // A road along a row of your own lots is how someone who walled themselves in gets out.
        let mut walled = highway_row(5, 0..=3);
        walled.extend(block(Kind::Homes, 0, 4..=8, 4..=6));
        let out = plan_road(&lawn(16), &walled, 1, span(4, 5, 8, 5)).unwrap();
        assert_eq!((out.cost, out.skipped), (5, 0));
    }

    // The simulation

    #[test]
    fn zones_grow_beside_a_road_joined_to_the_highway() {
        let mut tiles = highway_row(10, 0..=20);
        tiles.extend(block(Kind::Workshops, 0, 5..=9, 11..=11));
        run(1, &mut tiles, 0..60);
        assert!(
            tiles
                .values()
                .filter(|t| t.kind == Kind::Workshops)
                .all(|t| t.level >= 1),
            "{tiles:?}"
        );
        run(1, &mut tiles, 60..4000);
        assert!(tiles
            .values()
            .filter(|t| t.kind == Kind::Workshops)
            .all(|t| t.level == MAX_LEVEL));
    }

    #[test]
    fn zones_grow_only_within_three_tiles_of_such_a_road() {
        let mut tiles = highway_row(10, 0..=20);
        tiles.extend(block(Kind::Workshops, 0, 5..=5, 13..=13));
        tiles.extend(block(Kind::Workshops, 0, 8..=8, 14..=14));
        tiles.extend(block(Kind::Workshops, 0, 24..=24, 10..=10));
        tiles.extend(block(Kind::Workshops, 0, 23..=23, 10..=10));
        tiles.extend(block(Kind::Workshops, 0, 25..=25, 8..=8));
        run(1, &mut tiles, 0..4000);
        assert_eq!(
            tiles[&(5, 13)].level,
            MAX_LEVEL,
            "three tiles from the road"
        );
        assert_eq!(tiles[&(8, 14)].level, 0, "four tiles away");
        assert_eq!(
            tiles[&(24, 10)].level,
            0,
            "four along from the end of the road"
        );
        assert_eq!(
            tiles[&(23, 10)].level,
            MAX_LEVEL,
            "three along from the end"
        );
        assert_eq!(tiles[&(25, 8)].level, 0);
    }

    #[test]
    fn a_road_that_is_not_joined_to_the_highway_does_not_count() {
        let mut tiles = highway_row(10, 0..=10);
        // A road by itself, with workshops beside it.
        for x in 20..=24 {
            tiles.insert((x, 20), tile(Kind::Road, 0, Some(1)));
        }
        tiles.extend(block(Kind::Workshops, 0, 20..=24, 21..=21));
        run(1, &mut tiles, 0..500);
        assert_eq!(levels(&tiles, Kind::Workshops), 0);
        // Join it up with a road from the highway, the long way round, and they grow.
        for y in 11..=20 {
            tiles.insert((15, y), tile(Kind::Road, 0, Some(1)));
        }
        for x in 16..=19 {
            tiles.insert((x, 20), tile(Kind::Road, 0, Some(1)));
        }
        tiles.insert((11, 10), tile(Kind::Road, 0, Some(1)));
        for x in 12..=15 {
            tiles.insert((x, 10), tile(Kind::Road, 0, Some(1)));
        }
        run(1, &mut tiles, 500..4000);
        assert_eq!(levels(&tiles, Kind::Workshops), 5 * u32::from(MAX_LEVEL));
    }

    #[test]
    fn a_diagonal_touch_does_not_join_roads() {
        let mut tiles = highway_row(10, 0..=10);
        tiles.insert((11, 11), tile(Kind::Road, 0, Some(1)));
        tiles.insert((12, 11), tile(Kind::Road, 0, Some(1)));
        tiles.extend(block(Kind::Workshops, 0, 14..=16, 12..=12));
        run(1, &mut tiles, 0..300);
        assert_eq!(levels(&tiles, Kind::Workshops), 0);
    }

    #[test]
    fn cutting_a_road_starves_what_it_served_slowly_and_mending_it_saves_them() {
        let land = lawn(32);
        let mut tiles = highway_row(10, 0..=4);
        done(plan_road(&land, &tiles, 1, span(5, 10, 15, 10)), &mut tiles);
        tiles.extend(block(Kind::Workshops, 3, 13..=14, 11..=11));
        assert!(
            step(1, 0, &tiles).is_empty(),
            "nothing to do while it is joined and full"
        );
        // W17 stops a bulldoze here, so the road is lost another way, as if its builder were removed.
        assert_eq!(
            plan_bulldoze(&land, &tiles, 1, 8, 10, &nobody_gone),
            Err("a grown building depends on that road")
        );
        tiles.remove(&(8, 10));
        // Not at once: a stranded zone loses a level now and then, about one in 256 steps.
        let mut worst_step = 0;
        let mut before = levels(&tiles, Kind::Workshops);
        for step_no in 0..30 {
            let changes = step(1, step_no, &tiles);
            apply(&mut tiles, &changes);
            let now = levels(&tiles, Kind::Workshops);
            worst_step = worst_step.max(before - now);
            before = now;
        }
        assert!(worst_step <= 1, "{worst_step} levels lost in one step");
        assert!(
            levels(&tiles, Kind::Workshops) >= 4,
            "half an hour without the road cost {} of 6 levels",
            6 - levels(&tiles, Kind::Workshops)
        );
        // Left alone for good, they all go.
        let mut abandoned = tiles.clone();
        run(1, &mut abandoned, 30..6000);
        assert_eq!(levels(&abandoned, Kind::Workshops), 0);
        // Mend it in time and they grow back to full.
        done(plan_road(&land, &tiles, 1, span(8, 10, 8, 10)), &mut tiles);
        run(1, &mut tiles, 30..4000);
        assert_eq!(levels(&tiles, Kind::Workshops), 2 * u32::from(MAX_LEVEL));
    }

    #[test]
    fn losing_the_highway_itself_shrinks_everything() {
        let mut tiles = block(Kind::Homes, 3, 0..=4, 0..=1);
        tiles.extend(block(Kind::Shops, 2, 0..=4, 2..=2));
        run(1, &mut tiles, 0..6000);
        assert_eq!(levels(&tiles, Kind::Homes), 0);
        assert_eq!(levels(&tiles, Kind::Shops), 0);
    }

    #[test]
    fn homes_need_jobs_and_shops_need_people_and_workshops_just_need_time() {
        // Homes alone: the first settlers come, so a player sees something, and then nobody to
        // employ anyone else.
        let mut tiles = highway_row(10, 0..=20);
        tiles.extend(block(Kind::Homes, 0, 3..=12, 11..=11));
        run(1, &mut tiles, 0..1000);
        assert_eq!(stats(&tiles).population, SETTLERS);
        // Shops alone: a corner shop, and nobody to serve.
        let mut tiles = highway_row(10, 0..=20);
        tiles.extend(block(Kind::Shops, 0, 3..=12, 11..=11));
        run(1, &mut tiles, 0..1000);
        assert_eq!(
            levels(&tiles, Kind::Shops),
            CORNER_SHOP_PEOPLE / PEOPLE_PER_SHOP_LEVEL
        );
        // Workshops alone grow, and then homes can, and then shops.
        let mut tiles = highway_row(10, 0..=40);
        tiles.extend(block(Kind::Workshops, 0, 3..=6, 11..=11));
        tiles.extend(block(Kind::Homes, 0, 20..=29, 11..=11));
        tiles.extend(block(Kind::Shops, 0, 30..=39, 11..=11));
        run(1, &mut tiles, 0..4000);
        assert_eq!(levels(&tiles, Kind::Workshops), 4 * u32::from(MAX_LEVEL));
        assert!(
            levels(&tiles, Kind::Homes) > SETTLERS / PEOPLE_PER_HOME_LEVEL,
            "homes never grew past the settlers"
        );
        assert!(
            levels(&tiles, Kind::Shops) > CORNER_SHOP_PEOPLE / PEOPLE_PER_SHOP_LEVEL,
            "shops never grew past the corner shop"
        );
        let sum = stats(&tiles);
        assert!(
            sum.population <= sum.jobs + SETTLERS + 3,
            "{} people for {} jobs",
            sum.population,
            sum.jobs
        );
    }

    #[test]
    fn demand_limits_how_much_grows_in_one_step() {
        // One level of one workshop is 5 jobs, and a world has room for 12 settlers more: room for
        // five levels of homes, however many want it.
        let mut tiles = highway_row(10, 0..=40);
        tiles.insert((1, 11), tile(Kind::Workshops, 1, Some(1)));
        tiles.extend(block(Kind::Homes, 0, 10..=39, 11..=12));
        let mut most = 0;
        for step_no in 0..40 {
            let grown = step(1, step_no, &tiles)
                .iter()
                .filter(|change| matches!(change, Change::Set(_, t) if t.kind == Kind::Homes))
                .count();
            most = most.max(grown);
            if grown > 0 {
                break;
            }
        }
        assert_eq!(
            most, 5,
            "{most} home levels grew into 5 jobs and 12 settlers"
        );
        // However it runs, people never get ahead of jobs and settlers by more than one level of homes.
        run(9, &mut tiles, 0..1500);
        let sum = stats(&tiles);
        assert!(
            sum.population <= sum.jobs + SETTLERS + 3,
            "{} people for {} jobs",
            sum.population,
            sum.jobs
        );
        assert!(
            sum.population + 4 >= sum.jobs + SETTLERS,
            "{} people for {} jobs",
            sum.population,
            sum.jobs
        );
    }

    #[test]
    fn a_step_changes_only_some_tiles_and_not_the_same_ones_each_time() {
        let mut tiles = highway_row(10, 0..=60);
        tiles.extend(block(Kind::Workshops, 0, 0..=59, 11..=13));
        let first = step(1, 0, &tiles);
        assert!(
            (20..=100).contains(&first.len()),
            "{} of 180 tiles grew",
            first.len()
        );
        let second = step(1, 1, &tiles);
        assert_ne!(first, second);
        let again: BTreeSet<Pos> = first.iter().map(Change::pos).collect();
        let other: BTreeSet<Pos> = second.iter().map(Change::pos).collect();
        assert!(
            again.intersection(&other).count() < again.len(),
            "the same tiles grew twice"
        );
        assert_ne!(step(2, 0, &tiles), first, "another seed grows other tiles");
        assert!(
            first.windows(2).all(|pair| pair[0].pos() < pair[1].pos()),
            "changes come in position order"
        );
    }

    #[test]
    fn growth_odds_follow_the_rules() {
        assert_eq!(growth_odds(Kind::Homes, 0, 0), BASE_ODDS);
        assert!(growth_odds(Kind::Homes, 1, 0) > growth_odds(Kind::Homes, 0, 0));
        assert!(growth_odds(Kind::Homes, 3, 0) > growth_odds(Kind::Homes, 2, 0));
        assert_eq!(growth_odds(Kind::Homes, 3, 0), MAX_ODDS);
        assert_eq!(growth_odds(Kind::Homes, 50, 0), MAX_ODDS);
        assert!(growth_odds(Kind::Homes, 0, 1) < growth_odds(Kind::Homes, 0, 0));
        assert_eq!(growth_odds(Kind::Homes, 0, 3), 1, "hurt, never stopped");
        assert_eq!(growth_odds(Kind::Homes, 0, 50), 1);
        assert_eq!(
            growth_odds(Kind::Homes, 2, 1),
            growth_odds(Kind::Homes, 1, 0)
        );
        // Workshops hurt homes, not shops or each other.
        assert_eq!(growth_odds(Kind::Shops, 0, 5), BASE_ODDS);
        assert_eq!(growth_odds(Kind::Workshops, 0, 5), BASE_ODDS);
        assert!(growth_odds(Kind::Shops, 2, 0) > BASE_ODDS);
        assert!(growth_odds(Kind::Workshops, 2, 5) > BASE_ODDS);
    }

    #[test]
    fn parks_help_and_workshops_hurt() {
        // Three groups of homes on one road, each far from the others, with plenty of jobs.
        let mut tiles = highway_row(10, 0..=95);
        tiles.extend(block(Kind::Workshops, 3, 82..=91, 7..=9));
        let (plain, parked, smoky) = (0..=19, 26..=45, 54..=73);
        for xs in [plain.clone(), parked.clone(), smoky.clone()] {
            tiles.extend(block(Kind::Homes, 0, xs, 12..=13));
        }
        tiles.extend(block(Kind::Park, 0, 24..=47, 15..=15));
        tiles.extend(block(Kind::Workshops, 1, 52..=75, 14..=15));
        // The workshops are a road away too, so they keep their level.
        tiles.extend(highway_row(16, 52..=75));
        tiles.extend(highway_row(17, 52..=75));
        for (x, y) in [
            (51, 10),
            (51, 11),
            (51, 12),
            (51, 13),
            (51, 14),
            (51, 15),
            (51, 16),
        ] {
            tiles.insert((x, y), tile(Kind::Road, 0, None));
        }
        run(1, &mut tiles, 0..1);
        let homes = |xs: RangeInclusive<u16>| -> u32 {
            xs.flat_map(|x| (12..=13).map(move |y| (x, y)))
                .map(|pos| u32::from(tiles[&pos].level))
                .sum()
        };
        let (plain, parked, smoky) = (homes(plain), homes(parked), homes(smoky));
        eprintln!("after one step: {parked} levels with parks, {plain} with neither, {smoky} beside workshops");
        assert!(
            parked > plain + 10,
            "parks should help: {parked} against {plain}"
        );
        assert!(
            plain > smoky + 10,
            "workshops should hurt: {plain} against {smoky}"
        );
    }

    #[test]
    fn replaying_a_world_from_scratch_gives_the_same_tiles() {
        // Players build beside the highway as it nears the middle of the map, a move every ten
        // steps, and plenty of those moves are refused: that is part of what gets replayed.
        fn play(seed: u64) -> Tiles {
            let land = terrain(seed, 64);
            let mut tiles = start_tiles(seed, &land);
            let road = highway(seed, &land);
            for step_no in 0..600u64 {
                if step_no % 10 == 0 {
                    let k = step_no / 10;
                    let player = 1 + k % 3;
                    let (hx, hy) = road[road.len() - 1 - (k as usize % 20)];
                    let (x, y) = (i64::from(hx), i64::from(hy));
                    let kind = [Kind::Workshops, Kind::Homes, Kind::Shops][(k % 3) as usize];
                    for planned in [
                        plan_zone(
                            &land,
                            &tiles,
                            player,
                            kind,
                            span(x + 1, y + 1, x + 3, y + 3),
                        ),
                        plan_zone(
                            &land,
                            &tiles,
                            player,
                            kind,
                            span(x - 3, y - 3, x - 1, y - 1),
                        ),
                        plan_road(&land, &tiles, player, span(x, y + 1, x, y + 5)),
                        plan_park(&land, &tiles, player, x - 1, y + 1),
                        plan_bulldoze(&land, &tiles, player, x + 1, y + 1, &nobody_gone),
                    ] {
                        if let Ok(plan) = planned {
                            apply(&mut tiles, &plan.changes);
                        }
                    }
                }
                let changes = step(seed, step_no, &tiles);
                apply(&mut tiles, &changes);
            }
            tiles
        }
        let first = play(11);
        assert_eq!(first, play(11));
        let sum = stats(&first);
        eprintln!("the replayed city: {sum:?}");
        assert!(sum.population > 0 && sum.jobs > 0, "the city never grew");
        assert_ne!(first, play(12));
        assert!(first.values().all(|tile| tile.level <= MAX_LEVEL));
    }

    #[test]
    fn levels_stay_between_zero_and_three() {
        let mut tiles = highway_row(10, 0..=60);
        tiles.extend(block(Kind::Workshops, 0, 0..=59, 11..=12));
        tiles.extend(block(Kind::Homes, 0, 0..=59, 8..=9));
        tiles.extend(block(Kind::Shops, 0, 0..=59, 13..=14));
        for step_no in 0..300 {
            let changes = step(5, step_no, &tiles);
            apply(&mut tiles, &changes);
            assert!(tiles.values().all(|tile| tile.level <= MAX_LEVEL));
        }
    }

    #[test]
    fn stages_come_from_population() {
        for (population, stage) in [
            (0, "hamlet"),
            (49, "hamlet"),
            (50, "village"),
            (599, "village"),
            (600, "town"),
            (2999, "town"),
            (3000, "city"),
            (u32::MAX, "city"),
        ] {
            assert_eq!(stage_for(population), stage, "{population}");
        }
        // Population is homes' levels times four.
        let town = |levels: u32| -> Tiles {
            (0..levels)
                .map(|i| {
                    (
                        ((i % 60) as u16, (i / 60) as u16),
                        tile(Kind::Homes, 1, Some(1)),
                    )
                })
                .collect()
        };
        for (levels, people, stage) in [
            (12, 48, "hamlet"),
            (13, 52, "village"),
            (149, 596, "village"),
            (150, 600, "town"),
            (749, 2996, "town"),
            (750, 3000, "city"),
        ] {
            let sum = stats(&town(levels));
            assert_eq!((sum.population, sum.stage), (people, stage));
        }
    }

    #[test]
    fn stats_add_up_people_jobs_parks_and_tiles() {
        let mut tiles = highway_row(0, 0..=9);
        tiles.extend(block(Kind::Homes, 3, 0..=1, 1..=1));
        tiles.extend(block(Kind::Homes, 1, 2..=2, 1..=1));
        tiles.extend(block(Kind::Shops, 2, 0..=2, 2..=2));
        tiles.extend(block(Kind::Workshops, 3, 0..=0, 3..=3));
        tiles.extend(block(Kind::Workshops, 1, 1..=1, 3..=3));
        tiles.extend(block(Kind::Park, 0, 0..=3, 4..=4));
        tiles.extend(block(Kind::Cleared, 0, 5..=6, 4..=4));
        let sum = stats(&tiles);
        assert_eq!(sum.population, (3 + 3 + 1) * 4);
        assert_eq!(sum.jobs, 3 * 2 * 3 + (3 + 1) * 5);
        assert_eq!(sum.parks, 4);
        assert_eq!(sum.stage, "hamlet");
        assert_eq!(
            sum.counts,
            Counts {
                roads: 10,
                homes: 3,
                shops: 3,
                workshops: 2,
                parks: 4,
                cleared: 2
            }
        );
        assert_eq!(stats(&Tiles::new()).population, 0);
        // A level that should never be there doesn't count for more than three.
        tiles.insert((20, 20), tile(Kind::Homes, 200, Some(1)));
        assert_eq!(stats(&tiles).population, (3 + 3 + 1 + 3) * 4);
    }

    #[test]
    fn a_step_copes_with_odd_worlds() {
        assert!(step(1, 1, &Tiles::new()).is_empty());
        assert!(step(u64::MAX, u64::MAX, &Tiles::new()).is_empty());
        // Tiles beyond the biggest world are ignored, and don't make a huge grid.
        let mut tiles = highway_row(10, 0..=20);
        tiles.insert((u16::MAX, u16::MAX), tile(Kind::Homes, 3, Some(1)));
        tiles.insert((256, 0), tile(Kind::Shops, 3, Some(1)));
        tiles.insert((0, 300), tile(Kind::Park, 0, Some(1)));
        assert!(step(1, 1, &tiles).is_empty());
        tiles.extend(block(Kind::Workshops, 0, 255..=255, 255..=255));
        assert!(step(1, 1, &tiles)
            .iter()
            .all(|change| change.pos() == (255, 255)));
        // A world of one tile, and one that is all zones.
        let mut one: Tiles = [((0, 0), tile(Kind::Homes, 2, Some(1)))]
            .into_iter()
            .collect();
        run(1, &mut one, 0..6000);
        assert_eq!(one[&(0, 0)], tile(Kind::Homes, 0, Some(1)));
        let mut all = Tiles::new();
        all.extend(block(Kind::Homes, 3, 0..=9, 0..=9));
        run(3, &mut all, 0..8000);
        assert_eq!(levels(&all, Kind::Homes), 0);
    }

    #[test]
    fn a_step_on_a_busy_world_is_quick() {
        // 128 by 128, built on almost everywhere: a road every fourth row and column, with homes,
        // shops, workshops and parks between, at every level.
        let mut tiles = Tiles::new();
        for y in 0..128u16 {
            for x in 0..128u16 {
                let roll = hash(&[u64::from(x), u64::from(y), 99]);
                let (kind, level) = if x % 4 == 0 || y % 4 == 0 {
                    (Kind::Road, 0)
                } else {
                    match roll % 8 {
                        0..=3 => (Kind::Homes, (roll >> 8) as u8 % 4),
                        4 | 5 => (Kind::Shops, (roll >> 8) as u8 % 4),
                        6 => (Kind::Workshops, (roll >> 8) as u8 % 4),
                        _ => (Kind::Park, 0),
                    }
                };
                let player = if y == 64 { None } else { Some(1) };
                tiles.insert((x, y), tile(kind, level, player));
            }
        }
        let start = Instant::now();
        let mut changed = 0;
        for step_no in 0..10 {
            changed += step(1, step_no, &tiles).len();
        }
        let each = start.elapsed() / 10;
        eprintln!("a step on a full 128 by 128 world ({} tiles) takes {each:?}, {changed} changes in 10 steps", tiles.len());
        assert!(changed > 0);
        assert!(each.as_millis() < 2000, "a step took {each:?}");
        let start = Instant::now();
        let sum = stats(&tiles);
        eprintln!("stats on it take {:?}: {sum:?}", start.elapsed());
    }

    #[test]
    fn the_first_level_comes_in_minutes_and_the_next_two_take_hours() {
        let mut tiles = highway_row(10, 0..=100);
        tiles.extend(block(Kind::Workshops, 0, 0..=99, 11..=11));
        let at_least = |tiles: &Tiles, level: u8| {
            tiles
                .values()
                .filter(|t| t.kind == Kind::Workshops && t.level >= level)
                .count()
        };
        run(4, &mut tiles, 0..5);
        let (first, second) = (at_least(&tiles, 1), at_least(&tiles, 2));
        eprintln!("after five minutes: {first} of 100 have a building, {second} a second level");
        assert!(
            first >= 90,
            "only {first} of 100 had a building after five minutes"
        );
        assert!(
            second <= 25,
            "{second} of 100 had a second level after five minutes"
        );
        run(4, &mut tiles, 5..60);
        let second = at_least(&tiles, 2);
        eprintln!(
            "after an hour: {second} have a second level, {} a third",
            at_least(&tiles, 3)
        );
        assert!(
            (40..=95).contains(&second),
            "{second} second levels after an hour"
        );
        assert!(at_least(&tiles, 3) <= 40);
        run(4, &mut tiles, 60..300);
        let third = at_least(&tiles, 3);
        eprintln!("after five hours: {third} have a third level");
        assert!(
            (10..=90).contains(&third),
            "{third} third levels after five hours"
        );
        run(4, &mut tiles, 300..3000);
        assert!(at_least(&tiles, 3) >= 97);
    }

    #[test]
    fn homes_grow_past_level_one_only_beside_a_shop_and_past_level_two_only_beside_a_park() {
        // Plenty of jobs, far away. Three groups of homes that start at level 1: one with nothing
        // near, one with a shop on its street, and one with a shop and a park.
        let mut tiles = highway_row(10, 0..=95);
        tiles.extend(block(Kind::Workshops, 3, 82..=91, 7..=9));
        let (alone, shopped, parked) = (0..=9, 30..=39, 55..=64);
        for xs in [alone.clone(), shopped.clone(), parked.clone()] {
            tiles.extend(block(Kind::Homes, 1, xs, 12..=13));
        }
        for xs in [shopped.clone(), parked.clone()] {
            tiles.extend(block(Kind::Shops, 1, xs, 11..=11));
        }
        tiles.extend(block(Kind::Park, 0, parked.clone(), 14..=14));
        run(1, &mut tiles, 0..4000);
        let homes = |xs: RangeInclusive<u16>| -> u32 {
            xs.flat_map(|x| (12..=13).map(move |y| (x, y)))
                .map(|pos| u32::from(tiles[&pos].level))
                .sum()
        };
        assert_eq!(homes(alone), 20, "homes with nothing near stay at level 1");
        assert_eq!(
            homes(shopped),
            40,
            "a shop takes homes to level 2 and no further"
        );
        assert_eq!(homes(parked), 60, "a shop and a park take them to level 3");
    }

    #[test]
    fn a_stranded_zone_loses_a_level_about_once_in_four_hours() {
        let mut tiles = block(Kind::Homes, 3, 0..=31, 0..=31);
        run(1, &mut tiles, 0..60);
        let lost = tiles.values().filter(|t| t.level < 3).count();
        eprintln!("after an hour without a road, {lost} of 1024 homes have lost a level");
        assert!(
            (150..=280).contains(&lost),
            "{lost} of 1024 lost a level in an hour"
        );
        let twice = tiles.values().filter(|t| t.level < 2).count();
        assert!(twice <= 60, "{twice} lost two levels or more in an hour");
    }

    #[test]
    fn a_new_building_is_built_before_a_taller_one() {
        // Room for one level of homes. Twenty lots and a dozen homes that could go up a level
        // want it, and the lots must always win.
        let mut tiles = highway_row(10, 0..=40);
        tiles.extend(block(Kind::Workshops, 3, 30..=33, 11..=11));
        tiles.insert((20, 11), tile(Kind::Shops, 3, Some(1)));
        tiles.extend(block(Kind::Homes, 1, 12..=21, 12..=13));
        tiles.extend(block(Kind::Homes, 0, 22..=31, 12..=13));
        let sum = stats(&tiles);
        assert_eq!((sum.jobs, sum.population), (69, 80));
        for step_no in 0..40 {
            for change in step(7, step_no, &tiles) {
                if let Change::Set(_, grown) = change {
                    if grown.kind == Kind::Homes {
                        assert_eq!(grown.level, 1, "a taller home was built while lots waited");
                    }
                }
            }
        }
    }

    // The founders

    fn founded(seed: u64, size: usize) -> (Land, Tiles) {
        let land = terrain(seed, size);
        let tiles = founding_town(seed, &land);
        (land, tiles)
    }

    #[test]
    fn the_founding_town_is_a_small_balanced_town_with_nobody_to_bulldoze_it() {
        for size in [64usize, 128] {
            for seed in [1u64, 2, 3, 7, 11, 29, 100, 307, 1001, 4242, 99_999] {
                let (land, tiles) = founded(seed, size);
                for (pos, road) in start_tiles(seed, &land) {
                    assert_eq!(tiles.get(&pos), Some(&road), "the highway is part of it");
                }
                assert!(tiles.values().all(|t| t.player.is_none()), "{seed}");
                assert!(tiles
                    .keys()
                    .all(|pos| land.at(*pos).is_some_and(Ground::is_open)));
                let sum = stats(&tiles);
                eprintln!(
                    "{size} {seed}: {} people, {} jobs, {:?}",
                    sum.population, sum.jobs, sum.counts
                );
                assert!(
                    (30..=60).contains(&sum.population),
                    "{seed} at {size}: {} people",
                    sum.population
                );
                assert!(sum.jobs >= sum.population, "{seed} at {size}");
                assert!(sum.counts.homes >= 4 && sum.counts.shops >= 2, "{seed}");
                assert!(sum.counts.workshops >= 2 && sum.counts.parks >= 1, "{seed}");
                assert!(["hamlet", "village"].contains(&sum.stage));
                let grown: BTreeSet<u8> = tiles
                    .values()
                    .filter(|t| t.kind.is_zone())
                    .map(|t| t.level)
                    .collect();
                assert!(grown.len() >= 3, "levels are mixed: {grown:?}");
                assert_eq!(tiles, founding_town(seed, &land), "the same every time");
                // Workshops stand three tiles or more from any home, out of smoke's reach.
                for (&(wx, wy), work) in &tiles {
                    if work.kind != Kind::Workshops {
                        continue;
                    }
                    for (&(hx, hy), home) in &tiles {
                        if home.kind == Kind::Homes {
                            assert!(
                                wx.abs_diff(hx).max(wy.abs_diff(hy)) >= 3,
                                "{seed}: workshop at {wx},{wy} beside home at {hx},{hy}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_founding_town_stands_and_grows_under_the_step() {
        for seed in [11u64, 307, 1001] {
            let (_, mut tiles) = founded(seed, 64);
            let start = stats(&tiles).population;
            for step_no in 0..1500 {
                let changes = step(seed, step_no, &tiles);
                for change in &changes {
                    if let Change::Set(pos, grown) = change {
                        let before = tiles.get(pos).map_or(0, |t| t.level);
                        assert!(
                            grown.level > before,
                            "{seed}: {pos:?} shrank at step {step_no}"
                        );
                    }
                }
                apply(&mut tiles, &changes);
            }
            let end = stats(&tiles).population;
            eprintln!("{seed}: {start} people grew to {end} in a day");
            assert!(end > start + 30, "{seed}: {start} people and then {end}");
        }
    }

    #[test]
    fn the_founding_town_copes_with_tiny_land() {
        for size in [16usize, 17, 24, 32, 48] {
            for seed in 0..12u64 {
                let (land, mut tiles) = founded(seed, size);
                for (pos, road) in start_tiles(seed, &land) {
                    assert_eq!(tiles.get(&pos), Some(&road));
                }
                let sum = stats(&tiles);
                assert!(sum.jobs >= sum.population, "{seed} at {size}");
                for step_no in 0..400 {
                    let changes = step(seed, step_no, &tiles);
                    apply(&mut tiles, &changes);
                }
                assert!(tiles.values().all(|t| t.level <= MAX_LEVEL));
            }
        }
        // Land with nothing open anywhere near the middle gives just a highway, or nothing.
        let sea = picture(&["wwww"; 4]);
        assert!(founding_town(1, &sea).is_empty());
    }

    // W46
    #[test]
    fn after_the_founding_town_nothing_new_is_built_but_by_a_player() {
        let (_, mut tiles) = founded(307, 128);
        let town: BTreeSet<Pos> = tiles.keys().copied().collect();
        for step_no in 0..3000 {
            let changes = step(307, step_no, &tiles);
            apply(&mut tiles, &changes);
        }
        let later: BTreeSet<Pos> = tiles.keys().copied().collect();
        assert_eq!(later, town, "a step built on a tile nobody zoned");
    }

    // Tips

    /// A tile of the given kind and level built by player 5.
    fn mine(tiles: &mut Tiles, kind: Kind, level: u8, x: u16, y: u16) {
        tiles.insert((x, y), tile(kind, level, Some(5)));
    }

    // W8, W47
    #[test]
    fn a_tip_follows_what_is_holding_the_players_town_back() {
        const ME: u64 = 5;
        let street = || highway_row(10, 0..=40);
        assert_eq!(tip(&Tiles::new(), ME, 30), TIP_NO_ROAD);
        assert_eq!(tip(&street(), ME, 30), TIP_NO_ROAD, "a highway isn't yours");
        assert_eq!(tip(&street(), ME, 0), TIP_OUT_OF_MOVES, "out of moves comes first");
        assert_eq!(tip(&Tiles::new(), ME, 0), TIP_OUT_OF_MOVES);

        let mut tiles = street();
        mine(&mut tiles, Kind::Road, 0, 41, 10);
        assert_eq!(tip(&tiles, ME, 30), TIP_NO_ZONES);

        // A zone with no road within 3 tiles of it.
        mine(&mut tiles, Kind::Homes, 0, 20, 30);
        assert_eq!(tip(&tiles, ME, 30), TIP_TOO_FAR);

        // A workshop beside the highway and nobody's homes: there is room for people.
        let mut tiles = street();
        mine(&mut tiles, Kind::Workshops, 0, 5, 12);
        assert_eq!(tip(&tiles, ME, 30), TIP_ADD_HOMES);
        // Others' homes fill the room, so what's wanted is jobs. A workshop you have zoned is as good
        // as there, and the tip waits for it; one that has grown and still isn't enough asks for more.
        for x in [30, 32, 34] {
            tiles.insert((x, 12), tile(Kind::Homes, 3, Some(9)));
        }
        assert_eq!(tip(&tiles, ME, 30), TIP_WAIT);
        mine(&mut tiles, Kind::Workshops, 1, 5, 12);
        assert_eq!(tip(&tiles, ME, 30), TIP_ADD_WORKSHOPS);

        // Homes at level 1 stop there without a shop, and at level 2 without a park.
        let mut tiles = street();
        mine(&mut tiles, Kind::Workshops, 3, 5, 12);
        mine(&mut tiles, Kind::Homes, 1, 20, 12);
        assert_eq!(tip(&tiles, ME, 30), TIP_NO_SHOP);
        tiles.insert((22, 12), tile(Kind::Shops, 0, Some(9)));
        assert_eq!(tip(&tiles, ME, 30), TIP_NO_SHOP, "a shop that hasn't grown doesn't count");
        mine(&mut tiles, Kind::Shops, 0, 22, 12);
        assert_ne!(tip(&tiles, ME, 30), TIP_NO_SHOP, "a shop you have zoned does");
        tiles.insert((22, 12), tile(Kind::Shops, 1, Some(9)));
        assert_ne!(tip(&tiles, ME, 30), TIP_NO_SHOP);
        mine(&mut tiles, Kind::Homes, 2, 20, 12);
        assert_eq!(tip(&tiles, ME, 30), TIP_NO_PARK);
        tiles.insert((20, 13), tile(Kind::Park, 0, Some(9)));
        assert_ne!(tip(&tiles, ME, 30), TIP_NO_PARK);

        // A working workshop within 2 tiles of homes that could still grow.
        mine(&mut tiles, Kind::Workshops, 1, 22, 13);
        assert_eq!(tip(&tiles, ME, 30), TIP_SMOKE);

        // Homes waiting to grow with no room for them: more jobs.
        let mut tiles = street();
        mine(&mut tiles, Kind::Homes, 0, 20, 12);
        for x in [30, 32, 34] {
            tiles.insert((x, 12), tile(Kind::Homes, 3, Some(9)));
        }
        assert_eq!(tip(&tiles, ME, 30), TIP_ADD_WORKSHOPS);

        // All homes grown with jobs to spare: more homes.
        let mut tiles = street();
        mine(&mut tiles, Kind::Homes, 3, 20, 12);
        tiles.extend(block(Kind::Workshops, 3, 30..=31, 12..=12));
        assert_eq!(tip(&tiles, ME, 30), TIP_ADD_HOMES);

        // Homes and no shops, and no room for more homes: shops.
        let mut tiles = street();
        for x in [20, 24, 28] {
            mine(&mut tiles, Kind::Homes, 3, x, 12);
        }
        assert_eq!(tip(&tiles, ME, 30), TIP_ADD_SHOPS);

        // A town in balance. One shop still to grow means waiting, and none means there's nothing
        // to wait for.
        let balanced = |last_shop: u8| {
            let mut tiles = street();
            for x in [20, 24, 28, 32] {
                mine(&mut tiles, Kind::Homes, 3, x, 12);
            }
            for (x, level) in [(21, 3), (25, 3), (29, last_shop)] {
                mine(&mut tiles, Kind::Shops, level, x, 13);
            }
            tiles
        };
        assert_eq!(tip(&balanced(0), ME, 30), TIP_WAIT);
        assert_eq!(tip(&balanced(3), ME, 30), TIP_DONE);
    }

    // W8, W47
    #[test]
    fn a_tip_is_always_one_of_the_fixed_sentences_whatever_the_world_holds() {
        let mut rng = 0x1234_5678_9ABC_DEF1u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let kinds = [Kind::Road, Kind::Homes, Kind::Shops, Kind::Workshops, Kind::Park, Kind::Cleared];
        let mut seen = BTreeSet::new();
        for round in 0..300 {
            let spread = if round % 30 == 0 { 256 } else { [8u64, 24, 24][round % 3] };
            let mut tiles = Tiles::new();
            for _ in 0..(next() % 120) {
                let pos = ((next() % spread) as u16, (next() % spread) as u16);
                let player = [None, Some(1), Some(2)][(next() % 3) as usize];
                tiles.insert(pos, tile(kinds[(next() % 6) as usize], (next() % 4) as u8, player));
            }
            for player in [0u64, 1, 2] {
                for moves in [0u32, 30] {
                    let given = tip(&tiles, player, moves);
                    assert!(TIPS.contains(&given), "{given}");
                    seen.insert(given);
                }
            }
        }
        assert!(seen.len() >= 4, "only {} different tips came up", seen.len());
        // The list has no repeats, and each is short enough to read at a glance.
        let all: BTreeSet<&str> = TIPS.iter().copied().collect();
        assert_eq!(all.len(), TIPS.len());
        assert!(TIPS.iter().all(|t| t.len() <= 80 && t.is_ascii() && t.ends_with('.')));
    }

    // Names

    const RUDE: [&str; 54] = [
        "ass", "tit", "cock", "dick", "cum", "fuck", "shit", "piss", "cunt", "slut", "whor",
        "bitch", "nazi", "rape", "sex", "porn", "anal", "anus", "poo", "turd", "fart", "boob",
        "wank", "damn", "hell", "crap", "fag", "gay", "nig", "kill", "dead", "die", "hate", "ugly",
        "stupid", "fat", "dumb", "idiot", "drunk", "drug", "gun", "bomb", "nude", "naked", "bum",
        "butt", "arse", "hoe", "pimp", "kkk", "lust", "vag", "dyke", "tard",
    ];

    /// Other people's brands: games and products that a name shouldn't be taken for.
    const BRANDS: [&str; 22] = [
        "sim",
        "sims",
        "simcity",
        "simtown",
        "skyline",
        "skylines",
        "tycoon",
        "tropico",
        "metropolis",
        "civilization",
        "townscaper",
        "banished",
        "anno",
        "minecraft",
        "roblox",
        "fortnite",
        "cities",
        "city",
        "village",
        "town",
        "hamlet",
        "commons",
    ];

    const FAMOUS_PAIRS: [&str; 18] = [
        "golden goose",
        "mighty duck",
        "blue jay",
        "red bull",
        "black swan",
        "busy bee",
        "angry bird",
        "silver fox",
        "sleepy hollow",
        "emerald city",
        "lucky charms",
        "lucky duck",
        "happy meal",
        "happy hippo",
        "lazy susan",
        "wise owl",
        "sly fox",
        "green giant",
    ];

    fn check_words(words: &[&str], what: &str) {
        assert!(words.len() >= 60, "{what} has only {} words", words.len());
        let distinct: HashSet<&&str> = words.iter().collect();
        assert_eq!(distinct.len(), words.len(), "{what} repeats a word");
        for word in words {
            assert!((3..=12).contains(&word.len()), "{word}");
            assert!(word.chars().all(|c| c.is_ascii_alphabetic()), "{word}");
            assert!(word.starts_with(|c: char| c.is_ascii_uppercase()), "{word}");
            assert!(word[1..].chars().all(|c| c.is_ascii_lowercase()), "{word}");
            let lower = word.to_lowercase();
            assert!(
                !BRANDS.contains(&lower.as_str()),
                "{word} is someone else's name"
            );
            for bad in RUDE {
                assert!(!lower.contains(bad), "{word} contains {bad}");
            }
        }
    }

    /// Every pair that is a problem, so a bad word can be fixed in one go.
    fn check_pairs(first: &[&str], second: &[&str], what: &str) {
        let mut problems = Vec::new();
        for a in first {
            for b in second {
                let spaced = format!("{a} {b}").to_lowercase();
                let joined = spaced.replace(' ', "");
                for bad in RUDE {
                    if joined.contains(bad) {
                        problems.push(format!("{a} {b} contains {bad}"));
                    }
                }
                for brand in BRANDS.iter().filter(|brand| brand.len() >= 6) {
                    if joined.contains(brand) {
                        problems.push(format!("{a} {b} contains {brand}"));
                    }
                }
                if FAMOUS_PAIRS.contains(&spaced.as_str()) {
                    problems.push(format!("{a} {b} is a known name"));
                }
            }
        }
        assert!(problems.is_empty(), "{what}: {problems:?}");
    }

    #[test]
    fn name_lists_are_long_clean_and_friendly() {
        check_words(ADJECTIVES, "adjectives");
        check_words(ANIMALS, "animals");
        check_words(WORLD_ADJECTIVES, "world adjectives");
        check_words(PLACES, "places");
        let animals: HashSet<&&str> = ANIMALS.iter().collect();
        assert!(ADJECTIVES.iter().all(|word| !animals.contains(word)));
        let places: HashSet<&&str> = PLACES.iter().collect();
        assert!(
            WORLD_ADJECTIVES.iter().all(|word| !places.contains(word)),
            "a place named twice"
        );
        // Crawlers are frogs, so no player is one.
        assert!(!ANIMALS.iter().any(|word| ["Frog", "Toad"].contains(word)));
    }

    #[test]
    fn no_pair_of_words_is_rude_or_someone_elses_name() {
        check_pairs(ADJECTIVES, ANIMALS, "players");
        check_pairs(WORLD_ADJECTIVES, PLACES, "worlds");
    }

    #[test]
    fn player_names_are_stable_and_all_different_for_a_long_while() {
        assert_eq!(player_name(0), player_name(0));
        let total = (ADJECTIVES.len() * ANIMALS.len()) as u64;
        // The stride shares no factor with the list sizes, so it visits every name once.
        let (mut a, mut b) = (NAME_STRIDE, total);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        assert_eq!(a, 1);
        let names: HashSet<String> = (0..total).map(player_name).collect();
        assert_eq!(names.len() as u64, total);
        assert_eq!(player_name(total), player_name(0));
        assert_eq!(player_name(u64::MAX), player_name(u64::MAX % total));
        for n in [0, 1, 2, 3, 1000, u64::MAX] {
            let name = player_name(n);
            let (adjective, animal) = name.split_once(' ').unwrap();
            assert!(
                ADJECTIVES.contains(&adjective) && ANIMALS.contains(&animal),
                "{name}"
            );
        }
        assert_ne!(
            player_name(1).split(' ').next(),
            player_name(2).split(' ').next()
        );
    }

    #[test]
    fn world_names_come_from_the_seed() {
        assert_eq!(world_name(307), world_name(307));
        let names: HashSet<String> = (0..2000).map(world_name).collect();
        assert!(
            names.len() > 1200,
            "only {} different names in 2000 seeds",
            names.len()
        );
        for seed in [0, 1, u64::MAX, COMMONS_SEED] {
            let name = world_name(seed);
            let (adjective, place) = name.split_once(' ').unwrap();
            assert!(
                WORLD_ADJECTIVES.contains(&adjective) && PLACES.contains(&place),
                "{name}"
            );
        }
    }

    #[test]
    fn visitor_names_come_from_the_player_lists_in_another_order() {
        let total = (ADJECTIVES.len() * ANIMALS.len()) as u64;
        let (mut a, mut b) = (VISITOR_STRIDE, total);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        assert_eq!(
            a, 1,
            "the stride has to share no factor with the number of names"
        );
        assert_eq!(visitor_name(7), visitor_name(7));
        assert_eq!(visitor_name(total + 7), visitor_name(7));
        assert_eq!(visitor_name(u64::MAX), visitor_name(u64::MAX % total));
        let visitors: HashSet<String> = (0..total).map(visitor_name).collect();
        let players: HashSet<String> = (0..total).map(player_name).collect();
        assert_eq!(
            visitors.len() as u64,
            total,
            "every number gets its own name for a long while"
        );
        assert_eq!(visitors, players, "the same names, from the same lists");
        let alike = (0..total)
            .filter(|n| visitor_name(*n) == player_name(*n))
            .count() as u64;
        assert!(
            alike * 20 < total,
            "{alike} of {total} numbers give a visitor and a player the same name"
        );
        for n in [0, 1, 2, 1000, u64::MAX] {
            let name = visitor_name(n);
            let (adjective, animal) = name.split_once(' ').unwrap();
            assert!(
                ADJECTIVES.contains(&adjective) && ANIMALS.contains(&animal),
                "{name}"
            );
        }
    }

    // Crawlers and fetchers

    /// A user agent for each crawler, in the shape its maker documents, and the id it should give.
    const CRAWLER_AGENTS: &[(&str, &str)] = &[
        ("Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)", "googlebot"),
        ("Mozilla/5.0 (Linux; Android 6.0.1; Nexus 5X Build/MMB29P) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/W.X.Y.Z Mobile Safari/537.36 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)", "googlebot"),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; bingbot/2.0; +http://www.bing.com/bingbot.htm) Chrome/116.0.1938.76 Safari/537.36", "bingbot"),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; GPTBot/1.1; +https://openai.com/gptbot", "gptbot"),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; OAI-SearchBot/1.0; +https://openai.com/searchbot", "oai-searchbot"),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; ClaudeBot/1.0; +claudebot@anthropic.com)", "claudebot"),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; PerplexityBot/1.0; +https://perplexity.ai/perplexitybot)", "perplexitybot"),
        ("DuckDuckBot/1.1; (+http://duckduckgo.com/duckduckbot.html)", "duckduckbot"),
        ("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_5) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/13.1.1 Safari/605.1.15 (Applebot/0.1; +http://www.apple.com/go/applebot)", "applebot"),
        ("Mozilla/5.0 (compatible; YandexBot/3.0; +http://yandex.com/bots)", "yandexbot"),
        ("Mozilla/5.0 (compatible; Baiduspider/2.0; +http://www.baidu.com/search/spider.html)", "baiduspider"),
        ("Mozilla/5.0 (compatible; AhrefsBot/7.0; +http://ahrefs.com/robot/)", "ahrefsbot"),
        ("Mozilla/5.0 (compatible; SemrushBot/7~bl; +http://www.semrush.com/bot.html)", "semrushbot"),
        ("Mozilla/5.0 (Linux; Android 5.0) AppleWebKit/537.36 (KHTML, like Gecko) Mobile Safari/537.36 (compatible; Bytespider; spider-feedback@bytedance.com)", "bytespider"),
        ("CCBot/2.0 (https://commoncrawl.org/faq/)", "ccbot"),
        ("meta-externalagent/1.1 (+https://developers.facebook.com/docs/sharing/webmasters/crawler)", "meta-externalagent"),
        ("Mozilla/5.0 (Linux; Android 5.0) AppleWebKit/537.36 (KHTML, like Gecko) Mobile Safari/537.36 (compatible; Amazonbot/0.1; +https://developer.amazon.com/support/amazonbot)", "amazonbot"),
        ("Mozilla/5.0 (Linux; Android 7.0;) AppleWebKit/537.36 (KHTML, like Gecko) Mobile Safari/537.36 (compatible; PetalBot;+https://webmaster.petalsearch.com/site/petalbot)", "petalbot"),
        ("Mozilla/5.0 (compatible; MJ12bot/v1.4.8; http://mj12bot.com/)", "mj12bot"),
    ];

    /// The same for each fetcher. Where a maker's exact string wasn't to hand, this is its shape.
    const FETCHER_AGENTS: &[(&str, &str)] = &[
        ("Mozilla/5.0 (Linux; Android 6.0.1; Nexus 5X Build/MMB29P) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/W.X.Y.Z Mobile Safari/537.36 (compatible; Google-Agent; +https://developers.google.com/crawling/docs/crawlers-fetchers/google-user-triggered-fetchers)", "google-agent"),
        ("Mozilla/5.0 (compatible; Google-GeminiNotebook)", "google-gemininotebook"),
        ("Mozilla/5.0 (compatible; Google-NotebookLM)", "google-notebooklm"),
        ("Mozilla/5.0 (compatible; Gemini-Deep-Research; +https://gemini.google/overview/deep-research/)", "gemini-deep-research"),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko); compatible; ChatGPT-User/1.0; +https://openai.com/bot", "chatgpt-user"),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Claude-User/1.0; +Claude-User@anthropic.com)", "claude-user"),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; Perplexity-User/1.0; +https://perplexity.ai/perplexity-user)", "perplexity-user"),
        ("Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko; compatible; MistralAI-User/1.0; +https://docs.mistral.ai/robots)", "mistralai-user"),
        ("Mozilla/5.0 (compatible; DuckAssistBot/1.2; +http://duckduckgo.com/duckassistbot.html)", "duckassistbot"),
        ("meta-externalfetcher/1.1 (+https://developers.facebook.com/docs/sharing/webmasters/crawler)", "meta-externalfetcher"),
    ];

    #[test]
    fn real_user_agents_are_recognised_as_the_right_kind() {
        for &(agent, id) in CRAWLER_AGENTS {
            assert_eq!(
                crawler_from_user_agent(agent).map(|c| c.id),
                Some(id),
                "{agent}"
            );
            assert!(fetcher_from_user_agent(agent).is_none(), "{agent}");
            assert!(
                matches!(agent_kind(agent), Some(AgentKind::Crawler(c)) if c.id == id),
                "{agent}"
            );
            assert_eq!(agent_kind(agent).map(|kind| kind.id()), Some(id));
        }
        for &(agent, id) in FETCHER_AGENTS {
            assert_eq!(
                fetcher_from_user_agent(agent).map(|f| f.id),
                Some(id),
                "{agent}"
            );
            assert!(crawler_from_user_agent(agent).is_none(), "{agent}");
            assert!(
                matches!(agent_kind(agent), Some(AgentKind::Fetcher(f)) if f.id == id),
                "{agent}"
            );
            assert_eq!(agent_kind(agent).map(|kind| kind.id()), Some(id));
        }
        let crawlers: HashSet<&str> = CRAWLER_AGENTS.iter().map(|(_, id)| *id).collect();
        let fetchers: HashSet<&str> = FETCHER_AGENTS.iter().map(|(_, id)| *id).collect();
        assert_eq!(
            crawlers.len(),
            CRAWLERS.len(),
            "a crawler has no user agent here"
        );
        assert_eq!(
            fetchers.len(),
            FETCHERS.len(),
            "a fetcher has no user agent here"
        );
    }

    #[test]
    fn chat_and_claude_users_are_fetchers_not_crawlers() {
        for id in ["chatgpt-user", "claude-user"] {
            assert!(!CRAWLERS.iter().any(|crawler| crawler.id == id));
            assert!(FETCHERS.iter().any(|fetcher| fetcher.id == id));
            assert!(crawler_from_user_agent(id).is_none());
            assert_eq!(agent_kind(id).map(|kind| kind.id()), Some(id));
        }
        // The crawlers from the same makers are still crawlers.
        for agent in [
            "GPTBot/1.1",
            "OAI-SearchBot/1.0",
            "ClaudeBot/1.0",
            "PerplexityBot/1.0",
        ] {
            assert!(
                matches!(agent_kind(agent), Some(AgentKind::Crawler(_))),
                "{agent}"
            );
        }
    }

    #[test]
    fn a_string_naming_both_is_a_fetcher_because_a_person_is_waiting() {
        for agent in [
            "Mozilla/5.0 (compatible; Googlebot/2.1) Claude-User/1.0",
            "Claude-User/1.0 Googlebot/2.1",
            "bingbot ChatGPT-User bingbot",
        ] {
            assert!(
                matches!(agent_kind(agent), Some(AgentKind::Fetcher(_))),
                "{agent}"
            );
        }
    }

    #[test]
    fn people_and_other_programs_are_not_listed() {
        for agent in [
            "",
            " ",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
            "curl/8.4.0",
            "python-requests/2.31.0",
            "Google-InspectionTool/1.0",
            "Google-Read-Aloud",
            "AdsBot-Google (+http://www.google.com/adsbot.html)",
            "bing",
            "googleb",
            "g o o g l e b o t",
            "ChatGPT/1.2024.10 (iOS 17.0)",
            "Claude/1.0",
            "Perplexity",
            "MistralAI",
            "google agent",
            "Google-Agen",
            "\u{FF27}\u{FF2F}\u{FF2F}\u{FF27}\u{FF2C}\u{FF25}\u{FF22}\u{FF2F}\u{FF34}",
        ] {
            let listed = agent_kind(agent).map(|kind| kind.id());
            assert!(listed.is_none(), "{agent:?} is {listed:?}");
            assert!(crawler_from_user_agent(agent).is_none() && fetcher_from_user_agent(agent).is_none());
        }
    }

    #[test]
    fn case_does_not_matter_and_odd_strings_are_safe() {
        for agent in [
            "GOOGLEBOT",
            "googlebot",
            "gOoGlEbOt",
            "x GoogleBot x",
            "\0googlebot\0",
            "é googlebot é",
        ] {
            assert_eq!(
                crawler_from_user_agent(agent).map(|c| c.id),
                Some("googlebot"),
                "{agent:?}"
            );
            assert_eq!(agent_kind(agent).map(|kind| kind.id()), Some("googlebot"));
        }
        for agent in [
            "CLAUDE-USER",
            "claude-user",
            "cLaUdE-uSeR",
            "x Claude-User x",
            "\0claude-user\0",
            "é claude-user é",
        ] {
            assert_eq!(
                fetcher_from_user_agent(agent).map(|f| f.id),
                Some("claude-user"),
                "{agent:?}"
            );
            assert_eq!(agent_kind(agent).map(|kind| kind.id()), Some("claude-user"));
        }
        // Only the start of a long string is read: the token has to be in the first 512 bytes.
        for token in ["googlebot", "claude-user"] {
            let filler = "a".repeat(MAX_AGENT_BYTES - token.len());
            assert!(agent_kind(&format!("{filler}{token}")).is_some());
            assert!(agent_kind(&format!("{filler}X{token}")).is_none());
            assert!(agent_kind(&format!("{filler}{}", &token[..token.len() - 1])).is_none());
            assert!(agent_kind(&format!("{filler}\u{1F438}")).is_none());
            // Multi-byte characters straddling the cut.
            assert!(agent_kind(&format!("{}{token}", "é".repeat(1000))).is_none());
        }
        // Huge strings, and every control character.
        let start = Instant::now();
        assert!(agent_kind(&"a".repeat(20_000_000)).is_none());
        assert!(agent_kind(&"\u{1F438}".repeat(5_000_000)).is_none());
        assert!(
            start.elapsed().as_millis() < 1000,
            "a huge string took {:?}",
            start.elapsed()
        );
        let controls: String = (0u8..=127).map(char::from).collect();
        assert!(agent_kind(&controls).is_none());
        // A string with listed names in it, cut at every place it can be, panics nowhere.
        let mixed = "é\u{1F438}Googlebot Claude-User ü";
        for end in 0..=mixed.len() {
            if mixed.is_char_boundary(end) {
                let _ = agent_kind(&mixed[..end]);
            }
        }
    }

    fn check_ids(ids: &[&str], names: &[&str]) {
        for (id, name) in ids.iter().zip(names) {
            assert!(!id.is_empty() && *id == id.to_ascii_lowercase(), "{id}");
            assert!(
                id.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{id}"
            );
            assert_eq!(
                *id,
                name.to_ascii_lowercase(),
                "a name is its id with capitals"
            );
        }
    }

    #[test]
    fn the_crawler_list_is_complete_and_tidy() {
        for id in [
            "googlebot",
            "bingbot",
            "gptbot",
            "oai-searchbot",
            "claudebot",
            "perplexitybot",
            "duckduckbot",
            "applebot",
            "yandexbot",
            "baiduspider",
            "ahrefsbot",
            "semrushbot",
            "bytespider",
            "ccbot",
            "meta-externalagent",
        ] {
            assert!(
                CRAWLERS.iter().any(|crawler| crawler.id == id),
                "{id} is missing"
            );
        }
        let ids: Vec<&str> = CRAWLERS.iter().map(|c| c.id).collect();
        let names: Vec<&str> = CRAWLERS.iter().map(|c| c.name).collect();
        let colours: HashSet<&str> = CRAWLERS.iter().map(|c| c.colour).collect();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), CRAWLERS.len());
        assert_eq!(names.iter().collect::<HashSet<_>>().len(), CRAWLERS.len());
        assert_eq!(colours.len(), CRAWLERS.len(), "two crawlers share a colour");
        check_ids(&ids, &names);
        for crawler in CRAWLERS {
            assert!(crawler.colour.len() == 7 && crawler.colour.starts_with('#'));
            assert!(
                crawler.colour[1..].bytes().all(|b| b.is_ascii_hexdigit()),
                "{}",
                crawler.colour
            );
            assert!(std::ptr::eq(
                crawler_from_user_agent(crawler.name).unwrap(),
                crawler
            ));
        }
    }

    #[test]
    fn the_fetcher_list_is_complete_and_tidy() {
        for id in [
            "google-agent",
            "google-gemininotebook",
            "google-notebooklm",
            "chatgpt-user",
            "claude-user",
            "perplexity-user",
            "mistralai-user",
            "duckassistbot",
        ] {
            assert!(
                FETCHERS.iter().any(|fetcher| fetcher.id == id),
                "{id} is missing"
            );
        }
        let ids: Vec<&str> = FETCHERS.iter().map(|f| f.id).collect();
        let names: Vec<&str> = FETCHERS.iter().map(|f| f.name).collect();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), FETCHERS.len());
        assert_eq!(names.iter().collect::<HashSet<_>>().len(), FETCHERS.len());
        check_ids(&ids, &names);
        for fetcher in FETCHERS {
            assert!(std::ptr::eq(
                fetcher_from_user_agent(fetcher.name).unwrap(),
                fetcher
            ));
        }
        // One namespace, so a sighting counted under an id is only ever one kind of visitor, and
        // no id hides inside another, which would make the first on the list win.
        let all: Vec<&str> = CRAWLERS
            .iter()
            .map(|c| c.id)
            .chain(FETCHERS.iter().map(|f| f.id))
            .collect();
        assert_eq!(all.iter().collect::<HashSet<_>>().len(), all.len());
        for a in &all {
            for b in &all {
                assert!(a == b || !a.contains(b), "{a} contains {b}");
            }
        }
    }

    fn straight_road(length: u16) -> Roads {
        Roads::from_positions((0..length).map(|x| (x, 5)))
    }

    /// A plus sign, a loop and a spur, so a frog has choices.
    fn tangled_roads() -> Roads {
        let mut spots: Vec<Pos> = (0..=20).map(|x| (x, 10)).collect();
        spots.extend((0..=20).map(|y| (10, y)));
        spots.extend((4..=16).flat_map(|i| [(i, 4), (i, 16)]));
        spots.extend((4..=16).flat_map(|i| [(4, i), (16, i)]));
        spots.extend((21..=25).map(|x| (x, 10)));
        Roads::from_positions(spots)
    }

    fn apart((ax, ay): Pos, (bx, by): Pos) -> u32 {
        (i32::from(ax) - i32::from(bx)).unsigned_abs()
            + (i32::from(ay) - i32::from(by)).unsigned_abs()
    }

    #[test]
    fn a_frog_stays_on_the_roads_and_hops_along_them() {
        for roads in [
            straight_road(100),
            tangled_roads(),
            straight_road(1),
            straight_road(2),
        ] {
            for crawler in CRAWLERS {
                let mut last = crawler_position(crawler.id, 0, &roads).unwrap();
                for step_no in 1..400 {
                    let now = crawler_position(crawler.id, step_no, &roads).unwrap();
                    assert!(
                        roads.find(now).is_some(),
                        "{} is off the roads at {now:?}",
                        crawler.id
                    );
                    assert!(
                        apart(last, now) as u64 <= FROG_HOP,
                        "{} jumped from {last:?} to {now:?}",
                        crawler.id
                    );
                    last = now;
                }
            }
        }
    }

    #[test]
    fn a_frogs_tour_visits_every_connected_road_and_never_jumps() {
        for roads in [straight_road(40), tangled_roads()] {
            for frog in 0..30u64 {
                let tour = frog_tour(&roads, frog);
                let seen: BTreeSet<&Pos> = tour.iter().collect();
                assert_eq!(seen.len(), roads.len(), "frog {frog} missed a road");
                for pair in tour.windows(2) {
                    assert_eq!(
                        apart(pair[0], pair[1]),
                        1,
                        "frog {frog}: {:?} then {:?}",
                        pair[0],
                        pair[1]
                    );
                }
                assert_eq!(
                    apart(tour[0], *tour.last().unwrap()),
                    1,
                    "the tour is a loop"
                );
            }
        }
        let orders: HashSet<Vec<Pos>> = (0..30)
            .map(|frog| frog_tour(&tangled_roads(), frog))
            .collect();
        assert!(orders.len() > 20, "frogs all take the same tour");
    }

    #[test]
    fn frogs_are_generated_not_steered() {
        let roads = tangled_roads();
        assert_eq!(
            crawler_position("googlebot", 12, &roads),
            crawler_position("googlebot", 12, &roads)
        );
        assert_eq!(
            crawler_position("googlebot", 12, &roads),
            crawler_position(
                "googlebot",
                12,
                &Roads::from_positions(roads.sorted.iter().rev().copied())
            )
        );
        let at_zero: HashSet<Option<Pos>> = CRAWLERS
            .iter()
            .map(|c| crawler_position(c.id, 0, &roads))
            .collect();
        assert!(at_zero.len() >= 10, "frogs start in the same few places");
        let moves: HashSet<Option<Pos>> = (0..50)
            .map(|step_no| crawler_position("claudebot", step_no, &roads))
            .collect();
        assert!(moves.len() > 20, "a frog doesn't get about");
        assert_eq!(crawler_position("nonsense", 1, &roads), None);
        assert_eq!(crawler_position("", 1, &roads), None);
        assert_eq!(
            crawler_position("GoogleBot", 1, &roads),
            None,
            "ids are exact: only a user agent is matched loosely"
        );
        assert_eq!(crawler_position("googlebot", 1, &Roads::default()), None);
    }

    #[test]
    fn frogs_cope_with_huge_steps_one_tile_and_cut_roads() {
        let roads = tangled_roads();
        for step_no in [u64::MAX, u64::MAX - 1, 1 << 63, 1 << 32] {
            let pos = crawler_position("gptbot", step_no, &roads).unwrap();
            assert!(roads.find(pos).is_some());
        }
        let lonely = Roads::from_positions([(7, 7)]);
        for step_no in [0, 1, 99, u64::MAX] {
            assert_eq!(crawler_position("ccbot", step_no, &lonely), Some((7, 7)));
        }
        let cut = Roads::from_positions((0..10).chain(20..30).map(|x| (x, 3)));
        for crawler in CRAWLERS {
            let pos = crawler_position(crawler.id, 5, &cut).unwrap();
            assert!(cut.find(pos).is_some());
        }
        // A very long road is walked without recursion.
        let long = Roads::from_positions((0..60_000u16).map(|x| (x, 0)));
        let start = Instant::now();
        let pos = crawler_position("applebot", 1234, &long).unwrap();
        assert_eq!(pos.1, 0);
        eprintln!(
            "a frog's tour of 60,000 road tiles takes {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn roads_come_from_tiles_or_positions() {
        let mut tiles = highway_row(3, 0..=4);
        tiles.extend(block(Kind::Homes, 0, 0..=4, 4..=4));
        assert_eq!(
            Roads::from_tiles(&tiles),
            Roads::from_positions((0..=4).rev().map(|x| (x, 3)))
        );
        assert_eq!(Roads::from_positions([(1, 1), (1, 1), (0, 0)]).len(), 2);
        assert!(Roads::from_tiles(&Tiles::new()).is_empty());
        // The frog on a real world is on its highway.
        let land = terrain(COMMONS_SEED, COMMONS_SIZE);
        let roads = Roads::from_tiles(&start_tiles(COMMONS_SEED, &land));
        let pos = crawler_position("bingbot", 77, &roads).unwrap();
        assert!(highway(COMMONS_SEED, &land).contains(&pos));
    }

    #[test]
    fn kinds_speak_the_words_the_pages_use() {
        for kind in [
            Kind::Road,
            Kind::Homes,
            Kind::Shops,
            Kind::Workshops,
            Kind::Park,
            Kind::Cleared,
        ] {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        for word in ["", "Road", "house", "homes ", "walk", "zone"] {
            assert_eq!(Kind::parse(word), None, "{word:?}");
        }
        assert_eq!(Kind::Workshops.as_str(), "workshops");
        // And serde takes the same words, so a request body can hold one.
        use serde::de::IntoDeserializer;
        let read = |word: &'static str| {
            Kind::deserialize(IntoDeserializer::<serde::de::value::Error>::into_deserializer(word))
        };
        assert_eq!(read("workshops"), Ok(Kind::Workshops));
        assert_eq!(read("park"), Ok(Kind::Park));
        assert!(read("Workshops").is_err() && read("zone").is_err() && read("").is_err());
    }

    #[test]
    fn a_visitor_stays_on_the_roads_and_strolls_along_them() {
        for roads in [
            straight_road(100),
            tangled_roads(),
            straight_road(1),
            straight_road(2),
        ] {
            for n in [0, 1, 2, 77, 1 << 40, u64::MAX] {
                let mut last = visitor_position(n, 0, &roads).unwrap();
                for step_no in 1..400 {
                    let now = visitor_position(n, step_no, &roads).unwrap();
                    assert!(
                        roads.find(now).is_some(),
                        "visitor {n} is off the roads at {now:?}"
                    );
                    assert!(
                        apart(last, now) as u64 <= VISITOR_HOP,
                        "visitor {n} jumped from {last:?} to {now:?}"
                    );
                    last = now;
                }
            }
        }
    }

    #[test]
    fn visitors_are_generated_not_steered() {
        let roads = tangled_roads();
        assert_eq!(
            visitor_position(5, 12, &roads),
            visitor_position(5, 12, &roads)
        );
        assert_eq!(
            visitor_position(5, 12, &roads),
            visitor_position(
                5,
                12,
                &Roads::from_positions(roads.sorted.iter().rev().copied())
            )
        );
        let at_zero: HashSet<Option<Pos>> =
            (0..30).map(|n| visitor_position(n, 0, &roads)).collect();
        assert!(at_zero.len() >= 12, "visitors start in the same few places");
        let walk: HashSet<Option<Pos>> = (0..60)
            .map(|step_no| visitor_position(9, step_no, &roads))
            .collect();
        assert!(walk.len() > 20, "a visitor doesn't get about");
        // A visitor isn't a frog, so the same number is somewhere else's business.
        let frogs = CRAWLERS.len() as u64;
        let same = (0..frogs)
            .filter(|n| {
                visitor_position(*n, 3, &roads)
                    == crawler_position(CRAWLERS[*n as usize].id, 3, &roads)
            })
            .count();
        assert!(same < CRAWLERS.len() / 2);
        assert_eq!(visitor_position(1, 1, &Roads::default()), None);
    }

    #[test]
    fn visitors_cope_with_huge_numbers_and_steps_and_small_roads() {
        let roads = tangled_roads();
        for n in [0, u64::MAX, 1 << 63] {
            for step_no in [0, u64::MAX, u64::MAX - 1, 1 << 63, 1 << 32] {
                let pos = visitor_position(n, step_no, &roads).unwrap();
                assert!(roads.find(pos).is_some());
            }
        }
        let lonely = Roads::from_positions([(7, 7)]);
        for step_no in [0, 1, 99, u64::MAX] {
            assert_eq!(visitor_position(4, step_no, &lonely), Some((7, 7)));
        }
        // On a real world, a visitor walks the highway.
        let land = terrain(COMMONS_SEED, COMMONS_SIZE);
        let roads = Roads::from_tiles(&start_tiles(COMMONS_SEED, &land));
        for step_no in [0, 1, 500] {
            assert!(highway(COMMONS_SEED, &land)
                .contains(&visitor_position(31, step_no, &roads).unwrap()));
        }
    }

    // Your town and its goal

    /// A town for player 5: a road if asked, then homes, shops and workshops of the levels given and
    /// parks, one tile each.
    fn town_with(road: bool, homes: &[u8], shops: &[u8], workshops: &[u8], parks: u8) -> Tiles {
        let mut tiles = Tiles::new();
        let mut put = |kind: Kind, level: u8| {
            let at = tiles.len() as u16;
            tiles.insert((at % 60, at / 60), tile(kind, level, Some(5)));
        };
        if road {
            put(Kind::Road, 0);
        }
        for &level in homes {
            put(Kind::Homes, level);
        }
        for &level in shops {
            put(Kind::Shops, level);
        }
        for &level in workshops {
            put(Kind::Workshops, level);
        }
        for _ in 0..parks {
            put(Kind::Park, 0);
        }
        tiles
    }

    // W48
    #[test]
    fn a_players_town_is_the_worlds_sums_over_their_own_tiles_and_the_towns_add_up_to_the_world() {
        let mut rng = 0x7041_5EED_u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let kinds = [Kind::Road, Kind::Homes, Kind::Shops, Kind::Workshops, Kind::Park, Kind::Cleared];
        for _ in 0..200 {
            let mut tiles = Tiles::new();
            for _ in 0..(next() % 150) {
                let pos = ((next() % 20) as u16, (next() % 20) as u16);
                let player = [None, Some(1), Some(2), Some(3)][(next() % 4) as usize];
                tiles.insert(pos, tile(kinds[(next() % 6) as usize], (next() % 4) as u8, player));
            }
            let owned = |player: Option<u64>| -> Tiles {
                tiles.iter().filter(|(_, t)| t.player == player).map(|(pos, t)| (*pos, *t)).collect()
            };
            let towns: Vec<Town> = [None, Some(1), Some(2), Some(3)].into_iter().map(|p| town(&owned(p))).collect();
            let world = stats(&tiles);
            assert_eq!(towns.iter().map(|t| t.people).sum::<u32>(), world.population);
            assert_eq!(towns.iter().map(|t| t.jobs).sum::<u32>(), world.jobs);
            assert_eq!(towns.iter().map(|t| t.parks).sum::<u32>(), world.parks);
            assert_eq!(towns.iter().map(|t| t.roads).sum::<u32>(), world.counts.roads);
            assert_eq!(towns.iter().map(|t| t.homes).sum::<u32>(), world.counts.homes);
            assert_eq!(towns.iter().map(|t| t.shops).sum::<u32>(), world.counts.shops);
            assert_eq!(towns.iter().map(|t| t.workshops).sum::<u32>(), world.counts.workshops);
            let tallest = tiles.values().filter(|t| t.kind.is_zone()).map(|t| t.level).max().unwrap_or(0);
            assert_eq!(towns.iter().map(|t| t.best_level).max().unwrap(), tallest);
        }
    }

    // W48
    #[test]
    fn a_town_says_what_is_built_with_lots_counted_and_sends_exactly_eight_numbers() {
        let mine = town(&town_with(true, &[0, 1, 3], &[2], &[1, 0], 2));
        assert_eq!(
            (mine.roads, mine.homes, mine.shops, mine.workshops, mine.parks, mine.people, mine.jobs),
            (1, 3, 1, 2, 2, 16, 11)
        );
        assert_eq!((mine.best_level, mine.best_home), (3, 3));
        let sent = serde_json::to_value(&mine).unwrap();
        let mut keys: Vec<&str> = sent.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["best_level", "homes", "jobs", "parks", "people", "roads", "shops", "workshops"]);
        assert!(sent.as_object().unwrap().values().all(|v| v.is_u64()), "{sent}");
        // Nothing built is a town of zeros, and a level a tile claims past the top is held to it.
        assert_eq!(town(&Tiles::new()), Town::default());
        let mut odd = Tiles::new();
        odd.insert((0, 0), tile(Kind::Homes, 250, Some(1)));
        assert_eq!((town(&odd).best_level, town(&odd).people), (MAX_LEVEL, 12));
    }

    // W49
    #[test]
    fn the_goal_ladder_goes_through_every_rung_in_order_and_each_one_can_be_reached() {
        let steps: [(Tiles, &str); 10] = [
            (Tiles::new(), GOAL_ROAD),
            (town_with(true, &[], &[], &[], 0), GOAL_SETTLERS),
            // Three homes of one level each house 12, the settlers a world always has room for.
            (town_with(true, &[1, 1, 1], &[], &[], 0), GOAL_SHOP),
            // A shop that is only zoned is a shop opened, as a tip counts a lot.
            (town_with(true, &[1, 1, 1], &[0], &[], 0), GOAL_LEVEL_2),
            (town_with(true, &[1, 1, 2], &[1], &[], 0), GOAL_LEVEL_3),
            (town_with(true, &[1, 2, 3], &[1], &[], 1), GOAL_FIFTY),
            (town_with(true, &[3; 5], &[1], &[], 1), GOAL_WORK),
            (town_with(true, &[3; 5], &[1], &[3; 4], 1), GOAL_HUNDRED_FIFTY),
            (town_with(true, &[3; 13], &[1], &[3; 11], 1), GOAL_FOUR_HUNDRED),
            (town_with(true, &[3; 34], &[1], &[3; 28], 1), GOAL_OPEN),
        ];
        let reached: Vec<&str> = steps.iter().map(|(tiles, _)| goal(&town(tiles))).collect();
        for (n, ((_, wanted), got)) in steps.iter().zip(&reached).enumerate() {
            assert_eq!(got, wanted, "step {n}");
        }
        // Every sentence on the ladder is reached, in the order of the list, and none is repeated.
        assert_eq!(reached, GOALS);
        let all: BTreeSet<&str> = GOALS.iter().copied().collect();
        assert_eq!(all.len(), GOALS.len());
        assert!(GOALS.iter().all(|g| g.len() <= 110 && g.is_ascii() && g.ends_with('.')));
        // A town that slips back is sent back to the rung it fell to.
        assert_eq!(goal(&town(&town_with(false, &[3; 34], &[1], &[3; 28], 1))), GOAL_ROAD);
        assert_eq!(goal(&town(&town_with(true, &[3; 34], &[1], &[3; 10], 1))), GOAL_WORK);
    }

    // W8, W49
    #[test]
    fn a_goal_is_always_one_of_the_fixed_sentences_whatever_the_town() {
        let mut rng = 0x60A1_u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let mut seen = BTreeSet::new();
        for _ in 0..3000 {
            let mut levels = |most: u64| -> Vec<u8> {
                let count = next() % most;
                (0..count).map(|_| (next() % 4) as u8).collect()
            };
            let (homes, shops, works) = (levels(40), levels(6), levels(40));
            let (road, parks) = (next() % 5 != 0, (next() % 3) as u8);
            let given = goal(&town(&town_with(road, &homes, &shops, &works, parks)));
            assert!(GOALS.contains(&given), "{given}");
            seen.insert(given);
        }
        assert!(seen.len() >= 6, "only {} different goals came up", seen.len());
    }

    // W15
    #[test]
    fn the_reserved_tiles_in_a_window_are_the_open_ones_a_zone_would_skip_and_no_others() {
        let mut land = lawn(32);
        land.set(3, 12, Ground::Water);
        let mut tiles = highway_row(10, 0..=20);
        // A road already on a kept tile, and a lot beside the highway, are not open to a zone.
        tiles.insert((3, 11), tile(Kind::Road, 0, Some(2)));
        tiles.insert((9, 9), tile(Kind::Homes, 0, Some(2)));
        let window = [2, 6, 15, 14];
        let listed = reserved_in(&land, &tiles, window);
        assert!(listed.len() >= 6, "{listed:?}");
        let mut expected = Vec::new();
        for y in 6..=14 {
            for x in 2..=15 {
                let open = !tiles.contains_key(&(x, y)) && land.at((x, y)) == Some(Ground::Grass);
                if open && is_reserved(&tiles, (x, y)) {
                    expected.push((x, y));
                }
            }
        }
        assert_eq!(listed, expected, "the same tiles a zone would skip, row by row");
        // A zone over a block in the window leaves out exactly these.
        let plan = plan_zone(&land, &tiles, 1, Kind::Homes, span(2, 6, 6, 10)).unwrap();
        assert!(plan.changes.iter().all(|c| !listed.contains(&c.pos())));
        assert!(listed.iter().all(|&(x, y)| matches!(spot(&land, &tiles, (x, y)), Spot::Free)));
        // A window anywhere else on the map, or off it, lists nothing and does no harm.
        assert!(reserved_in(&land, &tiles, [20, 20, 30, 30]).is_empty());
        assert!(reserved_in(&land, &tiles, [-40, -40, -30, -30]).is_empty());
        assert!(reserved_in(&land, &tiles, [30, 30, 5000, 5000]).len() <= 4);
    }

    // W17
    #[test]
    fn a_bulldoze_with_no_builder_or_not_yours_is_refused_from_the_tile_alone() {
        let land = lawn(16);
        let mut tiles = highway_row(8, 0..=15);
        // My road up from the highway, and a grown home that only the top of it serves.
        for y in 4..=7u16 {
            tiles.insert((5, y), tile(Kind::Road, 0, Some(7)));
        }
        tiles.insert((5, 1), tile(Kind::Homes, 2, Some(9)));
        // A dead end that serves nothing.
        tiles.insert((10, 7), tile(Kind::Road, 0, Some(7)));
        let mine = "you can only bulldoze what you built";
        // Only the tile and its neighbours are given, as the small read gives them.
        let near_only = |x: i64, y: i64| {
            let near: Tiles = tiles
                .iter()
                .filter(|(&(tx, ty), _)| (i64::from(tx) - x).abs() <= 1 && (i64::from(ty) - y).abs() <= 1)
                .map(|(pos, t)| (*pos, *t))
                .collect();
            plan_bulldoze_tile(&land, &near, 7, x, y, &nobody_gone)
        };
        assert_eq!(near_only(3, 8), Err(mine), "the highway has no builder");
        assert_eq!(near_only(5, 1), Err(mine), "a home someone else built");
        assert_eq!(near_only(0, 0), Err("there is nothing to bulldoze there"));
        assert_eq!(near_only(-1, 0), Err("that is off the map"));
        // My own road is allowed by the tile alone, whether or not something depends on it...
        assert!(near_only(5, 4).is_ok() && near_only(10, 7).is_ok());
        // ...which only the whole network can tell.
        assert_eq!(plan_bulldoze(&land, &tiles, 7, 5, 4, &nobody_gone), Err(NEEDED));
        assert_eq!(plan_bulldoze(&land, &tiles, 7, 5, 7, &nobody_gone), Err(NEEDED));
        assert!(plan_bulldoze(&land, &tiles, 7, 10, 7, &nobody_gone).is_ok());
        // The two never differ about anything but a road that something depends on.
        for y in 0..16 {
            for x in 0..16 {
                let whole = plan_bulldoze(&land, &tiles, 7, x, y, &nobody_gone);
                let alone = plan_bulldoze_tile(&land, &tiles, 7, x, y, &nobody_gone);
                if whole != alone {
                    let kind = tiles.get(&(x as u16, y as u16)).map(|t| t.kind);
                    assert_eq!((whole, kind), (Err(NEEDED), Some(Kind::Road)), "({x}, {y})");
                }
            }
        }
    }

    // W47
    #[test]
    fn a_tip_counts_a_lot_you_have_zoned_as_there_and_never_asks_for_more_of_it() {
        const ME: u64 = 5;
        let street = || highway_row(10, 0..=40);
        let mine = |tiles: &mut Tiles, kind: Kind, level: u8, x: u16, y: u16| {
            tiles.insert((x, y), tile(kind, level, Some(ME)));
        };
        // Homes grown to level 1 and no shop near: a shop is asked for. Zone one, and it isn't.
        let mut tiles = street();
        mine(&mut tiles, Kind::Homes, 1, 20, 12);
        mine(&mut tiles, Kind::Workshops, 3, 5, 12);
        assert_eq!(tip(&tiles, ME, 30), TIP_NO_SHOP);
        mine(&mut tiles, Kind::Shops, 0, 24, 12);
        assert_ne!(tip(&tiles, ME, 30), TIP_NO_SHOP);
        // One too far for the homes to reach doesn't hide the need.
        let mut far = street();
        mine(&mut far, Kind::Homes, 1, 20, 12);
        mine(&mut far, Kind::Workshops, 3, 5, 12);
        mine(&mut far, Kind::Shops, 0, 27, 12);
        assert_eq!(tip(&far, ME, 30), TIP_NO_SHOP);
        // Homes wait for jobs that other people's homes have used up: more work is asked for, until
        // a workshop is zoned, and then the tip waits for it.
        let mut tiles = street();
        mine(&mut tiles, Kind::Homes, 0, 20, 12);
        for x in [30, 32, 34] {
            tiles.insert((x, 12), tile(Kind::Homes, 3, Some(9)));
        }
        assert_eq!(tip(&tiles, ME, 30), TIP_ADD_WORKSHOPS);
        mine(&mut tiles, Kind::Workshops, 0, 5, 12);
        assert_eq!(tip(&tiles, ME, 30), TIP_WAIT);
        // Shops are the same: once one is zoned, "add shops" waits for it.
        let mut tiles = street();
        for x in [20, 24, 28] {
            mine(&mut tiles, Kind::Homes, 3, x, 12);
        }
        assert_eq!(tip(&tiles, ME, 30), TIP_ADD_SHOPS);
        mine(&mut tiles, Kind::Shops, 0, 21, 13);
        assert_eq!(tip(&tiles, ME, 30), TIP_WAIT);
    }
}
