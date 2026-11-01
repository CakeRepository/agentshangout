//! The world engine for The Commons: land, the highway, the moves, disasters, names,
//! and the crawler frogs and fetcher visitors (docs/public-city.md). It is all pure functions of its
//! arguments, with no storage, clock, randomness or floating point, so a world looks the same on
//! every machine and replaying its steps gives the same tiles. Storage and routes call into here.
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

/// A tile from the old city was at most this level.
pub const MAX_LEVEL: u8 = 3;
/// A road is a straight line of at most this many tiles.
pub const MAX_ROAD_TILES: u32 = 16;
/// A builder who hasn't been seen for this long has left their level 0 tiles for anyone to clear.
pub const GONE_SECS: i64 = 7 * 24 * 60 * 60;

pub const ROAD_COST: u32 = 1;
pub const BULLDOZE_COST: u32 = 1;
pub const WALK_COST: u32 = 1;
pub const EMOTE_COST: u32 = 1;
pub const PLACE_COST: u32 = 1;

/// W17: a place is served by a road joined to the highway within this many tiles.
const ROAD_REACH: usize = 3;
/// W15: a side street runs this many tiles straight out from a road with no builder, and one
/// starts at every sixth tile along such a road.
pub const SIDE_STREET: i64 = 3;
const SIDE_EVERY: i64 = 6;

/// W23: the stage comes from how many places stand.
const VILLAGE_AT: u32 = 10;
const TOWN_AT: u32 = 50;
const CITY_AT: u32 = 200;

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
    parts.iter().fold(0x243F_6A88_85A3_08D3, |acc, part| mix(acc ^ part))
}

fn text_hash(text: &str) -> u64 {
    text.bytes().fold(0x1357_9BDF_0246_8ACE, |acc, byte| mix(acc ^ u64::from(byte)))
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
        Land { size, cells: vec![ground; size * size] }
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
        ((0..size).contains(&x) && (0..size).contains(&y)).then_some((x as u16, y as u16))
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
    land.cells.chunks(land.size.max(1)).map(encode_row).collect()
}

fn encode_row(row: &[Ground]) -> String {
    let mut text = String::new();
    let mut start = 0;
    while start < row.len() {
        let run = row[start..].iter().take_while(|ground| **ground == row[start]).count();
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
    (0..size * size).map(|i| (4 * broad[i] + 2 * middle[i] + fine[i]) / 7).collect()
}

/// Noise along a line, for how far a river wanders.
fn line_noise(seed: u64, at: i64, cell: i64) -> i64 {
    let value = |k: i64| (hash(&[seed, SALT_RIVER, k as u64]) % 1024) as i64;
    lerp(value(at / cell), value(at / cell + 1), smooth(at % cell, cell))
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
        Wet { size, cells: vec![false; size * size] }
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
        (Some(lake), Some(side)) => {
            [(lake.x, lake.y), edge_point(n, side, pick(n / 5, n * 4 / 5, 0))]
        }
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
        .map(
            |(i, value)| {
                if wet.near((i % size) as i64, (i / size) as i64, 3) {
                    0
                } else {
                    *value
                }
            },
        )
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
    let lake = if sea.is_none() || (flags >> 16) % 100 < 35 { find_lake(seed, size) } else { None };
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
    let entry_cost: Vec<u32> =
        land.cells.iter().map(|ground| crossing_cost(*ground, carve)).collect();
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
        .map(|pos| (pos, Tile { kind: Kind::Road, level: 0, player: None }))
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
    House,
    Cafe,
    Club,
    Studio,
    /// W81: the town square. A world has one.
    Square,
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
            Kind::House => "house",
            Kind::Cafe => "cafe",
            Kind::Club => "club",
            Kind::Studio => "studio",
            Kind::Square => "square",
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
            Kind::House,
            Kind::Cafe,
            Kind::Club,
            Kind::Studio,
            Kind::Square,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == word)
    }

    /// W54: what a person places by hand: somewhere they can go into.
    pub fn is_building(self) -> bool {
        matches!(self, Kind::House | Kind::Cafe | Kind::Club | Kind::Studio | Kind::Square)
    }
}

/// One built tile. `player` is who built it, and is `None` for the highway.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    pub kind: Kind,
    pub level: u8,
    pub player: Option<u64>,
}

/// A change to the tiles: a tile is set or removed.
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

/// Two ends of a straight road, as they arrive: untrusted integers, first corner to last, with the
/// first at the top left.
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
const NEEDED: &str = "a place depends on that road";

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
    matches!(tile, Some(Tile { kind: Kind::Road, player: None, .. }))
}

/// W15: whether a tile is kept for a side street. Beside every road with no builder, each tile whose
/// x plus y is a multiple of 6 starts a street that runs 3 tiles straight out from the road. A place
/// can't go there but a road can, so a crowd of places can't wall the highway in. It follows from
/// position alone, and a road with no builder never comes down, so a tile never stops being kept.
pub fn is_reserved(tiles: &Tiles, pos: Pos) -> bool {
    reserved_by(pos, |at| is_street(tiles.get(&at).copied()))
}

/// W15: the open tiles inside a box (both corners included) that a place can't use because they are
/// kept for side streets, row by row. `tiles` must reach 3 tiles past the box.
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
    [(0, -1), (1, 0), (0, 1), (-1, 0)].into_iter().any(|(dx, dy)| {
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
    (u32::from(last.0 - first.0) + 1, u32::from(last.1 - first.1) + 1)
}

fn laid(fresh: Vec<Pos>, skipped: u32, tile: Tile, each: u32) -> Plan {
    let cost = fresh.len() as u32 * each;
    Plan { changes: fresh.into_iter().map(|pos| Change::Set(pos, tile)).collect(), cost, skipped }
}

/// W15, W16: a straight line of up to 16 tiles that touches a road. Roads already on the line are
/// kept and not charged, and a road may go over a side street kept for roads. Anything else in the
/// way, or off the map, refuses the whole line.
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
    Ok(laid(fresh, skipped, Tile { kind: Kind::Road, level: 0, player: Some(player) }, ROAD_COST))
}

/// W55: one place on open land with a road beside it. `tiles` must reach 3 tiles past it, for the
/// roads that keep a side street (W15).
pub fn plan_place(land: &Land, tiles: &Tiles, player: u64, kind: Kind, x: i64, y: i64) -> Planned {
    if !kind.is_building() {
        return Err("a building is a house, cafe, club or studio");
    }
    let pos = land.locate(x, y).ok_or(OFF_MAP)?;
    let (px, py) = pos;
    let beside_road =
        [(px.wrapping_sub(1), py), (px + 1, py), (px, py.wrapping_sub(1)), (px, py + 1)]
            .iter()
            .any(|at| tiles.get(at).is_some_and(|tile| tile.kind == Kind::Road));
    match spot(land, tiles, pos) {
        Spot::Free if is_reserved(tiles, pos) => Err(KEPT),
        Spot::Free if !beside_road => Err("a building needs a road beside it"),
        Spot::Free => {
            Ok(laid(vec![pos], 0, Tile { kind, level: 0, player: Some(player) }, PLACE_COST))
        }
        Spot::Road | Spot::Built => Err("that tile is already built on"),
        Spot::Blocked(why) => Err(why),
    }
}

/// W17: one tile, and only what you built. Anything else needs its builder gone for 7 days (the
/// caller says who is, by `gone`), and a tile of the old city that grew stays. The highway has no
/// builder, so it is never bulldozed. A road comes down only if no place depends on it, so `tiles`
/// must be every tile in the world when the tile is a road. Forest is cleared by bulldozing, and built-on
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
    leaves_every_place_a_road(land, tiles, x, y)?;
    Ok(plan)
}

/// W77: what god mode takes down: anything built, whoever built it. `tiles` must be every tile in the
/// world when the tile is a road, because a road a place depends on stays, as in W17.
pub fn plan_god_bulldoze(land: &Land, tiles: &Tiles, player: u64, x: i64, y: i64) -> Planned {
    let plan = plan_god_bulldoze_tile(land, tiles, player, x, y)?;
    leaves_every_place_a_road(land, tiles, x, y)?;
    Ok(plan)
}

/// W17: refuses to take down a road that a place depends on.
fn leaves_every_place_a_road(
    land: &Land,
    tiles: &Tiles,
    x: i64,
    y: i64,
) -> Result<(), &'static str> {
    let road =
        land.locate(x, y).filter(|pos| tiles.get(pos).is_some_and(|tile| tile.kind == Kind::Road));
    if road.is_some_and(|pos| strands_a_building(tiles, pos)) {
        return Err(NEEDED);
    }
    Ok(())
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
    let allowed = |tile: &Tile| {
        // W57: a building stays on the map however long its owner is away.
        let abandoned =
            tile.level == 0 && !tile.kind.is_building() && tile.player.is_some_and(gone);
        tile.player == Some(player) || abandoned
    };
    clear_tile(land, tiles, player, (x, y), &allowed, "you can only bulldoze what you built")
}

/// W77: the same for god mode, from the tile alone. The highway has no builder, and every road joins it.
pub fn plan_god_bulldoze_tile(land: &Land, tiles: &Tiles, player: u64, x: i64, y: i64) -> Planned {
    let allowed = |tile: &Tile| tile.kind != Kind::Road || tile.player.is_some();
    clear_tile(land, tiles, player, (x, y), &allowed, "the highway stays where it is")
}

/// What taking a tile down does, if `allowed` says it may come down.
fn clear_tile(
    land: &Land,
    tiles: &Tiles,
    player: u64,
    (x, y): (i64, i64),
    allowed: &dyn Fn(&Tile) -> bool,
    refused: &'static str,
) -> Planned {
    let pos = land.locate(x, y).ok_or(OFF_MAP)?;
    let cleared = Tile { kind: Kind::Cleared, level: 0, player: Some(player) };
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
    if !allowed(tile) {
        return Err(refused);
    }
    let change = if forest { Change::Set(pos, cleared) } else { Change::Remove(pos) };
    Ok(Plan { changes: vec![change], cost: BULLDOZE_COST, skipped: 0 })
}

/// W17: whether a road tile coming down would leave a place with no road joined to the highway
/// within reach, which a place that has one now would lose. It looks at the whole network, so
/// cutting a branch far from a place counts as cutting the place off.
fn strands_a_building(tiles: &Tiles, road: Pos) -> bool {
    let before = Grid::from_tiles(tiles);
    let mut after = Grid::sized(tiles, before.size);
    after.cells[usize::from(road.1) * after.size + usize::from(road.0)] = None;
    let (served, still) = (served_tiles(&before), served_tiles(&after));
    before.cells.iter().enumerate().any(|(index, cell)| {
        let place = matches!(cell, Some(tile) if tile.kind.is_building());
        place && served[index] && !still[index]
    })
}

/// For each tile, whether a road joined to the highway is within reach.
fn served_tiles(grid: &Grid) -> Vec<bool> {
    let joined = joined_roads(grid);
    (0..grid.cells.len())
        .map(|index| grid.count_near(index, ROAD_REACH, |i| joined[i]) > 0)
        .collect()
}

// What a world adds up to

/// Roads and places among a set of tiles.
fn count<'a>(tiles: impl Iterator<Item = &'a Tile>) -> (u32, u32) {
    tiles.fold((0, 0), |(roads, places), tile| {
        (roads + u32::from(tile.kind == Kind::Road), places + u32::from(tile.kind.is_building()))
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Stats {
    pub places: u32,
    pub roads: u32,
    pub stage: &'static str,
}

/// W23: hamlet, village from 10 places, town from 50, city from 200.
pub fn stage_for(places: u32) -> &'static str {
    match places {
        CITY_AT.. => "city",
        TOWN_AT.. => "town",
        VILLAGE_AT.. => "village",
        _ => "hamlet",
    }
}

pub fn stats(tiles: &Tiles) -> Stats {
    let (roads, places) = count(tiles.values());
    Stats { places, roads, stage: stage_for(places) }
}

// Your town: what one player's own tiles add up to, and what to aim for next.

/// W48: what a player has built, from their own tiles.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Town {
    pub roads: u32,
    pub places: u32,
}

pub fn town(tiles: &Tiles) -> Town {
    let (roads, places) = count(tiles.values());
    Town { roads, places }
}

// Goals: one sentence, picked from this ladder by a player's town, so what a reply says never
// comes from a caller (W8).

const GOAL_HOUSE: &str = "Put up your house on a tile beside a road.";
const GOAL_SECOND_PLACE: &str =
    "Put up a second place: a studio comes a day after your house, a club after 3 and a cafe after 7.";
const GOAL_ALL_PLACES: &str = "Fill out your places: a person can have a house and 3 others.";
const GOAL_PERSON_OPEN: &str =
    "You have all the places a person can. Run them, and invite people in.";
const GOAL_ROAD: &str = "Lay your first road, starting beside the highway.";
const GOAL_TEN_ROADS: &str = "Lay 10 tiles of road: a place needs a road beside it.";
const GOAL_THIRTY_ROADS: &str = "Lay 30 tiles of road, out to where there is room to build.";
const GOAL_AGENT_OPEN: &str =
    "Your streets are open. Keep them that way, and go where people are building.";

/// A person's rungs in order, then an agent's. A goal is always one of these.
pub const GOALS: [&str; 8] = [
    GOAL_HOUSE,
    GOAL_SECOND_PLACE,
    GOAL_ALL_PLACES,
    GOAL_PERSON_OPEN,
    GOAL_ROAD,
    GOAL_TEN_ROADS,
    GOAL_THIRTY_ROADS,
    GOAL_AGENT_OPEN,
];

/// W49: the first rung of the ladder that a town hasn't reached, a person's or an agent's. The last
/// of each is an open goal. A town that slips back (a road bulldozed, say) is sent back to the rung
/// it fell to.
pub fn goal(town: &Town, person: bool) -> &'static str {
    let (ladder, reached) = if person {
        (&GOALS[..4], [town.places >= 1, town.places >= 2, town.places >= 4])
    } else {
        (&GOALS[4..], [town.roads >= 1, town.roads >= 10, town.roads >= 30])
    };
    ladder[reached.iter().position(|done| !done).unwrap_or(reached.len())]
}

// Tips: one short sentence, picked from this list. The wording is fixed, so what a reply says never
// comes from a caller (W8).

const TIP_OUT_OF_MOVES: &str = "Out of moves for today: they come back at midnight UTC.";
const TIP_NO_HOUSE: &str = "No house yet: put one on a tile beside a road.";
const TIP_PLACES_COME: &str =
    "More places come with time: a studio after a day, a club after 3 and a cafe after 7.";
const TIP_NO_ROAD: &str = "No road yet: lay one that starts beside the highway.";
const TIP_MORE_ROADS: &str =
    "Roads give people somewhere to build: lay them out towards open land.";
/// W47: said instead of any other while a disaster strikes.
pub const TIP_STRIKING: &str =
    "A disaster is striking The Commons: nothing can be built until it is over.";

/// Every tip there is. A tip is always one of these.
pub const TIPS: [&str; 6] =
    [TIP_OUT_OF_MOVES, TIP_NO_HOUSE, TIP_PLACES_COME, TIP_NO_ROAD, TIP_MORE_ROADS, TIP_STRIKING];

/// W47: the one thing most worth doing next, given what this player built themselves.
pub fn tip(own: &Tiles, person: bool, moves_left: u32) -> &'static str {
    if moves_left == 0 {
        return TIP_OUT_OF_MOVES;
    }
    let has = |kind: Kind| own.values().any(|tile| tile.kind == kind);
    match (person, has(if person { Kind::House } else { Kind::Road })) {
        (true, false) => TIP_NO_HOUSE,
        (true, true) => TIP_PLACES_COME,
        (false, false) => TIP_NO_ROAD,
        (false, true) => TIP_MORE_ROADS,
    }
}

/// The tiles as a square grid, so a road's network can be walked without searching. Tiles beyond the
/// largest possible world are ignored.
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

/// Roads count only if a walk along roads from the highway reaches them. The highway is the roads
/// with no builder.
fn joined_roads(grid: &Grid) -> Vec<bool> {
    let mut joined = vec![false; grid.cells.len()];
    let mut walk = Vec::new();
    for (index, cell) in grid.cells.iter().enumerate() {
        if matches!(cell, Some(Tile { kind: Kind::Road, player: None, .. })) {
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

// Disasters (W64 to W70). Everything about one comes from the seed and the step it was asked for on,
// so it can be worked out again and nothing random is stored (W19).

/// W66: 5 minutes of omens, so nobody's map goes without a word.
pub const OMEN_STEPS: u64 = 5;
/// W67: it strikes for this many steps, and the last takes everything left.
pub const STRIKE_STEPS: u64 = 24;
/// W69: a view says it's over for an hour.
pub const OVER_STEPS: u64 = 60;
/// W31: a view has a fixed size, so a step strikes in at most this many circles.
pub const MAX_HITS: usize = 16;

const SALT_DISASTER: u64 = 240;
const SALT_STRIKE: u64 = 250;

/// W67: this many meteors, one a step. Each starts a fire this wide, which spreads this far a step.
const METEORS: u64 = 8;
const FIRE_START: i64 = 3;
const FIRE_SPREAD: i64 = 4;
const TWISTER_STOPS_A_STEP: u64 = 3;
const TWISTER_STRIDE: i64 = 5;
const TWISTER_REACH: i64 = 5;
const FROG_STRIDE: i64 = 14;
const FROG_REACH: i64 = 8;
const FLOOD_FRONT: i64 = 16;

/// W65: what ends the old city. It's listed words, and no name from any game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Disaster {
    Meteors,
    Flood,
    Twister,
    Frog,
}

impl Disaster {
    pub const ALL: [Disaster; 4] =
        [Disaster::Meteors, Disaster::Flood, Disaster::Twister, Disaster::Frog];

    pub fn as_str(self) -> &'static str {
        match self {
            Disaster::Meteors => "meteors",
            Disaster::Flood => "flood",
            Disaster::Twister => "twister",
            Disaster::Frog => "frog",
        }
    }
}

/// W65: which disaster it is, from the step it was asked for on.
pub fn disaster_for(seed: u64, asked_step: u64) -> Disaster {
    let pick = hash(&[seed, asked_step, SALT_DISASTER]) % Disaster::ALL.len() as u64;
    Disaster::ALL[pick as usize]
}

/// Where a disaster has got to at a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing to show: none was asked for, or it's long over.
    Quiet,
    Omen(Disaster),
    /// How many of its steps have struck.
    Striking(Disaster, u64),
    Over(Disaster),
}

/// W66, W67, W69: the phase at a step, from the steps a world keeps. A restart picks up where it
/// was, because this is all there is to know (W70).
pub fn phase(
    seed: u64,
    asked_step: u64,
    omen_step: Option<u64>,
    ended_step: Option<u64>,
    step: u64,
) -> Phase {
    let disaster = disaster_for(seed, asked_step);
    match (omen_step, ended_step) {
        (_, Some(ended)) if step < ended + OVER_STEPS => Phase::Over(disaster),
        (_, Some(_)) | (None, None) => Phase::Quiet,
        (Some(omen), None) if step < omen + OMEN_STEPS => Phase::Omen(disaster),
        (Some(omen), None) => {
            Phase::Striking(disaster, (step - (omen + OMEN_STEPS)).min(STRIKE_STEPS - 1))
        }
    }
}

/// A circle a disaster strikes: its middle and its radius, in tiles.
pub type Hit = (i64, i64, i64);

/// Bounces a coordinate off the edges of the map, so anything that wanders stays on it.
fn bounce(value: i64, size: i64) -> i64 {
    let there_and_back = 2 * (size - 1);
    let along = value.rem_euclid(there_and_back);
    if along < size {
        along
    } else {
        there_and_back - along
    }
}

/// Where something that wanders has got to after this many stops. It sets off from a corner for the
/// one opposite, and drifts as it goes.
fn wander(seed: u64, disaster: Disaster, stops: u64, stride: i64, size: i64) -> (i64, i64) {
    let start = hash(&[seed, SALT_STRIKE, disaster as u64]);
    let way = |bit: u64| if start >> bit & 1 == 0 { 1 } else { -1 };
    let (way_x, way_y) = (way(0), way(1));
    let corner = |way: i64| if way > 0 { 0 } else { size - 1 };
    let (mut x, mut y) = (corner(way_x), corner(way_y));
    for stop in 0..stops {
        let roll = hash(&[seed, SALT_STRIKE, disaster as u64, stop]);
        let drift = |bits: u64| ((roll >> bits) % (stride as u64 + 1)) as i64 - stride / 2;
        x += way_x * stride + drift(0);
        y += way_y * stride + drift(16);
    }
    (bounce(x, size), bounce(y, size))
}

/// W67: the circles a disaster strikes on its `done`th step. Meteors come down on the middle of the
/// map, one a step, and the fire each one starts keeps spreading, so a circle is how far it has got.
/// A flood comes in as a wall from one side, and a twister and a giant frog cross the map and turn
/// at its edges.
pub fn strike(seed: u64, disaster: Disaster, done: u64, size: usize) -> Vec<Hit> {
    let done = done.min(STRIKE_STEPS - 1);
    let n = size as i64;
    match disaster {
        Disaster::Meteors => (0..METEORS.min(done + 1))
            .map(|meteor| {
                let roll = |salt: u64| hash(&[seed, SALT_STRIKE, meteor, salt]);
                // Two rolls added fall nearer the middle, and all of them in the middle half.
                let half = (size as u64 / 2).max(1);
                let near_middle = |a: u64, b: u64| n / 4 + ((a % half + b % half) / 2) as i64;
                let reach = FIRE_START + (done - meteor) as i64 * FIRE_SPREAD;
                (near_middle(roll(0), roll(1)), near_middle(roll(2), roll(3)), reach)
            })
            .collect(),
        Disaster::Flood => {
            let side = hash(&[seed, SALT_STRIKE, 4]) % 4;
            let front = n * (done as i64 + 1) / STRIKE_STEPS as i64 - 1;
            let reach = (n / FLOOD_FRONT).clamp(1, 16);
            (0..FLOOD_FRONT)
                .map(|k| {
                    let along = (2 * k + 1) * n / (2 * FLOOD_FRONT);
                    let (x, y) = match side {
                        0 => (front, along),
                        1 => (n - 1 - front, along),
                        2 => (along, front),
                        _ => (along, n - 1 - front),
                    };
                    (x, y, reach)
                })
                .collect()
        }
        Disaster::Twister => (0..TWISTER_STOPS_A_STEP)
            .map(|stop| {
                let stops = done * TWISTER_STOPS_A_STEP + stop;
                let (x, y) = wander(seed, disaster, stops, TWISTER_STRIDE, n);
                (x, y, TWISTER_REACH)
            })
            .collect(),
        Disaster::Frog => {
            let (x, y) = wander(seed, disaster, done, FROG_STRIDE, n);
            vec![(x, y, FROG_REACH)]
        }
    }
}

/// W67, W68: what comes down on a step of a strike: everything in its circles, or on the last step
/// everything at all, but never the highway. A circle's edge is rough, so nothing burns in a perfect
/// ring. Changes come in position order.
pub fn strike_changes(tiles: &Tiles, highway: &[Pos], hits: &[Hit], last: bool) -> Vec<Change> {
    let inside = |&(x, y): &Pos| {
        let rough = (hash(&[u64::from(x), u64::from(y)]) % 3) as i64;
        hits.iter().any(|&(hx, hy, reach)| {
            let (dx, dy) = (i64::from(x) - hx, i64::from(y) - hy);
            let reach = (reach - rough).max(0);
            dx * dx + dy * dy <= reach * reach
        })
    };
    tiles
        .keys()
        .filter(|pos| !highway.contains(pos) && (last || inside(pos)))
        .map(|pos| Change::Remove(*pos))
        .collect()
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
    head.windows(id.len()).any(|window| window.eq_ignore_ascii_case(id.as_bytes()))
}

/// Which crawler on the list a user agent is, if any. Case doesn't matter. The string is only
/// looked at: nothing from it is kept or returned, and anything not on the list gives `None`.
pub fn crawler_from_user_agent(user_agent: &str) -> Option<&'static Crawler> {
    CRAWLERS.iter().find(|crawler| mentions(user_agent, crawler.id))
}

/// The same for fetchers.
pub fn fetcher_from_user_agent(user_agent: &str) -> Option<&'static Fetcher> {
    FETCHERS.iter().find(|fetcher| mentions(user_agent, fetcher.id))
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
        assert_eq!(cells.len(), rows.len() * rows.len(), "a picture must be square");
        Land { size: rows.len(), cells }
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
        Tile { kind, level, player }
    }

    /// Highway: roads with no builder, along a row.
    fn highway_row(y: u16, xs: RangeInclusive<u16>) -> Tiles {
        xs.map(|x| ((x, y), tile(Kind::Road, 0, None))).collect()
    }

    /// Tiles of the old city, dropped in directly.
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
            (COMMONS_SEED, COMMONS_SIZE, 0xd57f374e207f8e77, 0x757c14b7155003bf),
            (0xDEAD_BEEF, 128, 0xd871dd7b76e642ca, 0xa1523549c99e8d60),
            (u64::MAX, 64, 0x77a94f99e287ecf5, 0xa16a54eb13a48561),
        ];
        for (seed, size, land_expected, road_expected) in golden {
            let land = terrain(seed, size);
            let road = highway(seed, &land);
            assert_eq!(land_hash(&land), land_expected, "land for seed {seed} at {size}");
            assert_eq!(highway_hash(&road), road_expected, "highway for seed {seed} at {size}");
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
        assert!(least >= 12, "two worlds differ in only {least}% of their tiles");
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
            let (water, forest, hill) =
                (share(Ground::Water), share(Ground::Forest), share(Ground::Hill));
            assert!((1..=45).contains(&water), "seed {seed} is {water}% water");
            assert!((2..=35).contains(&forest), "seed {seed} is {forest}% forest");
            assert!(land.count(Ground::Hill) > 0 && hill <= 12, "seed {seed} has {hill}% hill");
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
            for (x, y) in [(32, 32), (30, 32), (34, 32), (32, 30), (32, 34), (28, 28), (36, 36)] {
                assert!(land.get(x, y).unwrap().is_open(), "seed {seed} has no room at {x},{y}");
            }
            let wet =
                Wet { size: 64, cells: land.cells.iter().map(|g| *g == Ground::Water).collect() };
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
                    letters >= 1 && row.chars().all(|c| c.is_ascii_digit() || "gfwsh".contains(c)),
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
        let rows =
            |texts: &[&str]| texts.iter().map(|text| text.to_string()).collect::<Vec<String>>();
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
        assert!(road.len() >= size / 2, "seed {seed}: highway of {} tiles", road.len());
        assert!(is_edge(&land, road[0]), "seed {seed}: starts at {:?}", road[0]);
        assert_eq!(*road.last().unwrap(), land.centre(), "seed {seed}");
        for pair in road.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let apart =
                (i32::from(a.0) - i32::from(b.0)).abs() + (i32::from(a.1) - i32::from(b.1)).abs();
            assert_eq!(apart, 1, "seed {seed}: {a:?} then {b:?}");
        }
        let distinct: BTreeSet<&Pos> = road.iter().collect();
        assert_eq!(distinct.len(), road.len(), "seed {seed}: the highway crosses itself");
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
        for seed in [0, 1, 2, 3, 4, 5, 6, 7, 307, 999, u64::MAX, u64::MAX - 1, 0xDEAD_BEEF] {
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
        assert_eq!((stats.places, stats.stage), (0, "hamlet"));
        assert_eq!(stats.roads as usize, tiles.len());
    }

    fn tile_of_road() -> Tile {
        tile(Kind::Road, 0, None)
    }

    #[test]
    fn highways_differ_between_seeds() {
        let roads: HashSet<u64> =
            (0..100).map(|seed| highway_hash(&highway(seed, &terrain(seed, 64)))).collect();
        // On open land a highway is settled by where on the edge it starts, and a 64 by 64 map has
        // about 250 such places, so a few seeds out of a hundred will share one.
        assert!(roads.len() >= 75, "only {} different highways in 100 seeds", roads.len());
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
                let changed = raw.cells.iter().zip(&land.cells).filter(|(a, b)| a != b).count();
                assert!(changed <= 64, "seed {seed} carved {changed} tiles");
            }
        }
        eprintln!("{carved} of 500 seeds needed land cut for the highway");
        assert!(carved <= 25, "{carved} of 500 seeds needed land cut for the highway");
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
        let cut = before.cells.iter().zip(&land.cells).filter(|(a, b)| a != b).count();
        assert_eq!(cut, 3, "one hill and two water tiles, the narrowest way through");
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
        assert_eq!(plan.changes[0], Change::Set((8, 2), tile(Kind::Road, 0, Some(7))));
        assert_eq!(plan.changes[4].pos(), (8, 6));
        // Along from the end of the highway, and out from its side.
        assert_eq!(plan_road(&land, &tiles, 7, span(9, 1, 11, 1)).unwrap().cost, 3);
        assert_eq!(plan_road(&land, &tiles, 7, span(4, 2, 4, 2)).unwrap().cost, 1);
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
        assert_eq!(plan_road(&land, &tiles, 9, span(9, 2, 10, 2)).unwrap().cost, 2);
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
        assert_eq!(plan_road(&land, &tiles, 1, span(3, 5, 18, 5)).unwrap().cost, 16);
        assert_eq!(
            plan_road(&land, &tiles, 1, span(3, 5, 19, 5)),
            Err("a road is at most 16 tiles long")
        );
        let tiles = highway_row(5, 0..=2);
        assert_eq!(plan_road(&land, &tiles, 1, span(2, 6, 2, 21)).unwrap().cost, 16);
        assert_eq!(
            plan_road(&land, &tiles, 1, span(2, 6, 2, 22)),
            Err("a road is at most 16 tiles long")
        );
        assert_eq!(plan_road(&land, &tiles, 1, span(3, 5, 3, 5)).unwrap().cost, 1);
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
    fn a_road_only_has_to_touch_a_road_not_reach_the_highway() {
        let land = picture(&YARD);
        let mut tiles = highway_row(1, 0..=8);
        done(plan_road(&land, &tiles, 7, span(3, 2, 3, 8)), &mut tiles);
        // Cut off from the highway, the far end still counts as a road.
        tiles.remove(&(3, 4));
        assert_eq!(plan_road(&land, &tiles, 7, span(0, 8, 2, 8)).unwrap().cost, 3);
        // Touching nothing at all is still refused.
        assert_eq!(
            plan_road(&land, &tiles, 7, span(0, 10, 2, 10)),
            Err("a road has to touch an existing road")
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
        assert_eq!(plan_road(&land, &tiles, 7, span(9, 1, 9, 1)).unwrap().cost, 1);
        assert_eq!(
            plan_road(&land, &tiles, 7, span(11, 1, 11, 5)),
            Err("a road has to touch an existing road")
        );
        // Hill and sand, and something built.
        done(plan_road(&land, &tiles, 7, span(9, 1, 11, 1)), &mut tiles);
        assert_eq!(
            plan_road(&land, &tiles, 7, span(11, 2, 11, 6)),
            Err("there is water or a hill in the way")
        );
        assert!(plan_road(&land, &tiles, 7, span(11, 2, 11, 5)).is_ok(), "sand is fine");
        tiles.insert((1, 2), tile(Kind::House, 0, Some(8)));
        assert_eq!(
            plan_road(&land, &tiles, 7, span(0, 2, 2, 2)),
            Err("something is built in the way")
        );
        // Cleared forest can be built on.
        tiles.insert((4, 3), tile(Kind::Cleared, 0, Some(1)));
        tiles.insert((4, 2), tile(Kind::Road, 0, Some(1)));
        assert!(plan_road(&land, &tiles, 7, span(4, 3, 4, 3)).is_ok());
    }

    // Moves: bulldozing

    #[test]
    fn you_bulldoze_what_you_built_and_nothing_else() {
        let land = picture(&YARD);
        let mut tiles = highway_row(1, 0..=8);
        done(plan_road(&land, &tiles, 7, span(3, 2, 3, 4)), &mut tiles);
        tiles.insert((0, 9), tile(Kind::House, 0, Some(8)));
        let plan = plan_bulldoze(&land, &tiles, 7, 3, 3, &nobody_gone).unwrap();
        assert_eq!((plan.cost, plan.changes), (1, vec![Change::Remove((3, 3))]));
        assert_eq!(
            plan_bulldoze(&land, &tiles, 8, 3, 3, &nobody_gone),
            Err("you can only bulldoze what you built")
        );
        assert_eq!(
            plan_bulldoze(&land, &tiles, 8, 0, 9, &nobody_gone).unwrap().changes,
            [Change::Remove((0, 9))]
        );
        // A tile of the old city that grew is still yours to bulldoze.
        tiles.insert((0, 9), tile(Kind::Homes, 3, Some(8)));
        assert!(plan_bulldoze(&land, &tiles, 8, 0, 9, &nobody_gone).is_ok());
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
            assert_eq!((plan.cost, plan.changes), (1, vec![Change::Remove((x as u16, 1))]));
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
            (1, vec![Change::Set((4, 3), tile(Kind::Cleared, 0, Some(7)))])
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
        assert_eq!(plan.changes, [Change::Set((4, 3), tile(Kind::Cleared, 0, Some(8)))]);
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
                    plan_place(&land, &tiles, 7, Kind::House, x, y),
                    plan_bulldoze(&land, &tiles, 7, x, y, &|player| player == 7),
                ];
                for &x2 in &nasty {
                    for &y2 in &nasty {
                        let s = span(x, y, x2, y2);
                        let road = plan_road(&land, &tiles, 7, s);
                        check_plan(&road, &on_map, &mut allowed, &mut refused);
                    }
                }
                for planned in &single {
                    check_plan(planned, &on_map, &mut allowed, &mut refused);
                }
            }
        }
        assert!(allowed > 0 && refused > 1000, "{allowed} allowed, {refused} refused");
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
                assert!(plan.cost >= 1 && plan.cost <= MAX_ROAD_TILES, "{plan:?}");
                assert!(!plan.changes.is_empty() && plan.changes.len() <= 16);
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
        assert!(plan_place(&land, &tiles, 7, Kind::House, 0, 1).is_err());
        assert_eq!(tiles, before);
    }

    // Moves: side streets, and roads that places hang on

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
            for (y, starts) in
                [(9, north), (8, north), (7, north), (11, south), (12, south), (13, south)]
            {
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
        // A tile of the old city with no builder keeps nothing either: only streets do.
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

    // W15, W55
    #[test]
    fn a_place_cant_stand_on_a_side_street_and_a_road_can() {
        let land = lawn(32);
        let tiles = highway_row(10, 0..=20);
        // (3, 9) starts a street north of the road: (3, 9), (3, 8) and (3, 7) are kept.
        assert!(kept(&tiles, 3, 9) && kept(&tiles, 3, 8) && kept(&tiles, 3, 7));
        let all_kept = Err("that strip is kept for a side street: roads only");
        assert_eq!(plan_place(&land, &tiles, 1, Kind::House, 3, 9), all_kept);
        // Next door is fine.
        assert!(plan_place(&land, &tiles, 1, Kind::House, 4, 9).is_ok());
        // And a road may use the street: it is what it is for.
        let road = plan_road(&land, &tiles, 1, span(3, 7, 3, 9)).unwrap();
        assert_eq!((road.cost, road.skipped, road.changes.len()), (3, 0, 3));
        // A street someone has built on is built on, and says so.
        let mut built = tiles.clone();
        built.insert((3, 9), tile(Kind::Road, 0, Some(2)));
        assert_eq!(
            plan_place(&land, &built, 1, Kind::House, 3, 9),
            Err("that tile is already built on")
        );
    }

    // W15
    #[test]
    fn a_crowd_of_places_cannot_wall_a_highway_in_and_a_road_can_still_leave_it() {
        let land = lawn(64);
        let mut tiles = highway_row(30, 0..=40);
        let mut beside: Vec<Pos> = Vec::new();
        for x in 0..=40u16 {
            beside.extend([(x, 29), (x, 31)]);
        }
        beside.push((41, 30));
        let (mut built, mut refused) = (0, 0);
        for (n, &(x, y)) in beside.iter().enumerate() {
            match plan_place(&land, &tiles, n as u64, Kind::House, i64::from(x), i64::from(y)) {
                Ok(plan) => {
                    built += 1;
                    apply(&mut tiles, &plan.changes);
                }
                Err(_) => refused += 1,
            }
        }
        assert!(built > 60 && refused >= 10, "{built} built, {refused} refused");
        // What is left beside the highway is the start of every side street.
        let open: Vec<Pos> =
            beside.iter().copied().filter(|pos| !tiles.contains_key(pos)).collect();
        assert_eq!(open.len(), refused);
        assert!(open.iter().all(|pos| is_reserved(&tiles, *pos)));
        // Someone else can still leave the highway there, three tiles out.
        let (x, y) = open[0];
        let (top, bottom) = if y < 30 { (y - 2, y) } else { (y, y + 2) };
        let road = plan_road(
            &land,
            &tiles,
            2,
            span(i64::from(x), i64::from(top), i64::from(x), i64::from(bottom)),
        )
        .unwrap();
        assert_eq!(road.cost, 3);
        apply(&mut tiles, &road.changes);
        assert!(is_reserved(&tiles, (x, y)), "it stays kept after a road is built on it");
    }

    // W17
    #[test]
    fn a_road_a_place_needs_cannot_be_bulldozed_by_anyone_directly_or_further_up_the_branch() {
        let land = lawn(32);
        let needed = Err(NEEDED);
        // A branch of three tiles off the end of the highway. The studio is served by (7, 10) alone.
        let mut tiles = highway_row(10, 0..=4);
        for x in 5..=7u16 {
            tiles.insert((x, 10), tile(Kind::Road, 0, Some(1)));
        }
        tiles.insert((10, 13), tile(Kind::Studio, 0, Some(2)));
        // Directly: the builder takes down the one road tile in reach of the place.
        assert_eq!(plan_bulldoze(&land, &tiles, 1, 7, 10, &nobody_gone), needed);
        // Further up the branch: the middle and the first tile both leave the end cut off.
        assert_eq!(plan_bulldoze(&land, &tiles, 1, 6, 10, &nobody_gone), needed);
        assert_eq!(plan_bulldoze(&land, &tiles, 1, 5, 10, &nobody_gone), needed);
        // Nobody else can do what the builder can't, not even someone who may clear a gone builder's roads.
        let everyone_gone = |_: u64| true;
        for x in 5..=7 {
            assert_eq!(plan_bulldoze(&land, &tiles, 9, x, 10, &everyone_gone), needed, "{x}");
        }
        // It is the place that counts: a tile of the old city depends on nothing, and the road goes.
        let mut old = tiles.clone();
        old.insert((10, 13), tile(Kind::Workshops, 3, Some(2)));
        for x in [7, 6, 5] {
            assert!(plan_bulldoze(&land, &old, 1, x, 10, &nobody_gone).is_ok(), "{x}");
        }
        // A place with another road in reach loses nothing, so the spare road can go.
        let mut spare = tiles.clone();
        spare.insert((10, 10), tile(Kind::Road, 0, None));
        assert!(plan_bulldoze(&land, &spare, 1, 7, 10, &nobody_gone).is_ok());
        // A place the branch no longer serves depends on nothing that is left of it.
        let mut cut = tiles.clone();
        cut.remove(&(5, 10));
        assert!(plan_bulldoze(&land, &cut, 1, 7, 10, &nobody_gone).is_ok());
    }

    // W17
    #[test]
    fn a_dead_end_road_that_serves_no_place_can_still_be_taken_down_by_its_builder() {
        let land = lawn(32);
        let mut tiles = highway_row(10, 0..=4);
        done(plan_road(&land, &tiles, 1, span(5, 10, 12, 10)), &mut tiles);
        // Tiles of the old city, and a place that the highway serves by itself.
        tiles.extend(block(Kind::Homes, 2, 10..=12, 11..=12));
        tiles.insert((2, 12), tile(Kind::House, 0, Some(4)));
        for x in (5..=12).rev() {
            let plan = plan_bulldoze(&land, &tiles, 1, x, 10, &nobody_gone).unwrap();
            apply(&mut tiles, &plan.changes);
        }
        assert!(tiles.values().all(|t| t.kind != Kind::Road || t.player.is_none()));
        // With one place beside it, the road can be taken back to the last tile in reach of it.
        let mut tiles = highway_row(10, 0..=4);
        done(plan_road(&land, &tiles, 1, span(5, 10, 12, 10)), &mut tiles);
        tiles.insert((12, 12), tile(Kind::Club, 0, Some(3)));
        for x in [12, 11, 10] {
            let plan = plan_bulldoze(&land, &tiles, 1, x, 10, &nobody_gone);
            apply(&mut tiles, &plan.expect("the club is still served").changes);
        }
        assert_eq!(plan_bulldoze(&land, &tiles, 1, 9, 10, &nobody_gone), Err(NEEDED));
        assert_eq!(plan_bulldoze(&land, &tiles, 1, 8, 10, &nobody_gone), Err(NEEDED));
    }

    /// Places that a road joined to the highway is within reach of, worked out the long way:
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
            .filter(|(_, t)| t.kind.is_building())
            .filter(|(pos, _)| {
                joined.iter().any(|j| j.0.abs_diff(pos.0) <= 3 && j.1.abs_diff(pos.1) <= 3)
            })
            .map(|(pos, _)| *pos)
            .collect()
    }

    // W17
    #[test]
    fn a_road_is_refused_exactly_when_taking_it_down_would_cut_a_place_off() {
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
                for (dx, dy) in [(0, 1), (1, 0), (0, -1), (-1, 0)] {
                    if let Ok(plan) = plan_place(&land, &tiles, 2, Kind::Cafe, x + dx, y + dy) {
                        apply(&mut tiles, &plan.changes);
                    }
                }
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
                        assert!(
                            !cuts,
                            "seed {seed}: ({x}, {y}) was taken down and cut a building off"
                        );
                    }
                    Err(why) => {
                        refused += 1;
                        assert_eq!(why, NEEDED);
                        assert!(cuts, "seed {seed}: ({x}, {y}) was refused and cut nothing off");
                    }
                }
            }
        }
        eprintln!("{allowed} roads could come down and {refused} could not");
        assert!(allowed > 100 && refused > 100, "{allowed} allowed, {refused} refused");
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
            assert!(!BRANDS.contains(&lower.as_str()), "{word} is someone else's name");
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
        assert!(WORLD_ADJECTIVES.iter().all(|word| !places.contains(word)), "a place named twice");
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
            assert!(ADJECTIVES.contains(&adjective) && ANIMALS.contains(&animal), "{name}");
        }
        assert_ne!(player_name(1).split(' ').next(), player_name(2).split(' ').next());
    }

    #[test]
    fn world_names_come_from_the_seed() {
        assert_eq!(world_name(307), world_name(307));
        let names: HashSet<String> = (0..2000).map(world_name).collect();
        assert!(names.len() > 1200, "only {} different names in 2000 seeds", names.len());
        for seed in [0, 1, u64::MAX, COMMONS_SEED] {
            let name = world_name(seed);
            let (adjective, place) = name.split_once(' ').unwrap();
            assert!(WORLD_ADJECTIVES.contains(&adjective) && PLACES.contains(&place), "{name}");
        }
    }

    #[test]
    fn visitor_names_come_from_the_player_lists_in_another_order() {
        let total = (ADJECTIVES.len() * ANIMALS.len()) as u64;
        let (mut a, mut b) = (VISITOR_STRIDE, total);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        assert_eq!(a, 1, "the stride has to share no factor with the number of names");
        assert_eq!(visitor_name(7), visitor_name(7));
        assert_eq!(visitor_name(total + 7), visitor_name(7));
        assert_eq!(visitor_name(u64::MAX), visitor_name(u64::MAX % total));
        let visitors: HashSet<String> = (0..total).map(visitor_name).collect();
        let players: HashSet<String> = (0..total).map(player_name).collect();
        assert_eq!(visitors.len() as u64, total, "every number gets its own name for a long while");
        assert_eq!(visitors, players, "the same names, from the same lists");
        let alike = (0..total).filter(|n| visitor_name(*n) == player_name(*n)).count() as u64;
        assert!(
            alike * 20 < total,
            "{alike} of {total} numbers give a visitor and a player the same name"
        );
        for n in [0, 1, 2, 1000, u64::MAX] {
            let name = visitor_name(n);
            let (adjective, animal) = name.split_once(' ').unwrap();
            assert!(ADJECTIVES.contains(&adjective) && ANIMALS.contains(&animal), "{name}");
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
            assert_eq!(crawler_from_user_agent(agent).map(|c| c.id), Some(id), "{agent}");
            assert!(fetcher_from_user_agent(agent).is_none(), "{agent}");
            assert!(
                matches!(agent_kind(agent), Some(AgentKind::Crawler(c)) if c.id == id),
                "{agent}"
            );
            assert_eq!(agent_kind(agent).map(|kind| kind.id()), Some(id));
        }
        for &(agent, id) in FETCHER_AGENTS {
            assert_eq!(fetcher_from_user_agent(agent).map(|f| f.id), Some(id), "{agent}");
            assert!(crawler_from_user_agent(agent).is_none(), "{agent}");
            assert!(
                matches!(agent_kind(agent), Some(AgentKind::Fetcher(f)) if f.id == id),
                "{agent}"
            );
            assert_eq!(agent_kind(agent).map(|kind| kind.id()), Some(id));
        }
        let crawlers: HashSet<&str> = CRAWLER_AGENTS.iter().map(|(_, id)| *id).collect();
        let fetchers: HashSet<&str> = FETCHER_AGENTS.iter().map(|(_, id)| *id).collect();
        assert_eq!(crawlers.len(), CRAWLERS.len(), "a crawler has no user agent here");
        assert_eq!(fetchers.len(), FETCHERS.len(), "a fetcher has no user agent here");
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
        for agent in ["GPTBot/1.1", "OAI-SearchBot/1.0", "ClaudeBot/1.0", "PerplexityBot/1.0"] {
            assert!(matches!(agent_kind(agent), Some(AgentKind::Crawler(_))), "{agent}");
        }
    }

    #[test]
    fn a_string_naming_both_is_a_fetcher_because_a_person_is_waiting() {
        for agent in [
            "Mozilla/5.0 (compatible; Googlebot/2.1) Claude-User/1.0",
            "Claude-User/1.0 Googlebot/2.1",
            "bingbot ChatGPT-User bingbot",
        ] {
            assert!(matches!(agent_kind(agent), Some(AgentKind::Fetcher(_))), "{agent}");
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
        assert!(start.elapsed().as_millis() < 1000, "a huge string took {:?}", start.elapsed());
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
                id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{id}"
            );
            assert_eq!(*id, name.to_ascii_lowercase(), "a name is its id with capitals");
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
            assert!(CRAWLERS.iter().any(|crawler| crawler.id == id), "{id} is missing");
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
            assert!(std::ptr::eq(crawler_from_user_agent(crawler.name).unwrap(), crawler));
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
            assert!(FETCHERS.iter().any(|fetcher| fetcher.id == id), "{id} is missing");
        }
        let ids: Vec<&str> = FETCHERS.iter().map(|f| f.id).collect();
        let names: Vec<&str> = FETCHERS.iter().map(|f| f.name).collect();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), FETCHERS.len());
        assert_eq!(names.iter().collect::<HashSet<_>>().len(), FETCHERS.len());
        check_ids(&ids, &names);
        for fetcher in FETCHERS {
            assert!(std::ptr::eq(fetcher_from_user_agent(fetcher.name).unwrap(), fetcher));
        }
        // One namespace, so a sighting counted under an id is only ever one kind of visitor, and
        // no id hides inside another, which would make the first on the list win.
        let all: Vec<&str> =
            CRAWLERS.iter().map(|c| c.id).chain(FETCHERS.iter().map(|f| f.id)).collect();
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
        for roads in [straight_road(100), tangled_roads(), straight_road(1), straight_road(2)] {
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
                assert_eq!(apart(tour[0], *tour.last().unwrap()), 1, "the tour is a loop");
            }
        }
        let orders: HashSet<Vec<Pos>> =
            (0..30).map(|frog| frog_tour(&tangled_roads(), frog)).collect();
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
        let at_zero: HashSet<Option<Pos>> =
            CRAWLERS.iter().map(|c| crawler_position(c.id, 0, &roads)).collect();
        assert!(at_zero.len() >= 10, "frogs start in the same few places");
        let moves: HashSet<Option<Pos>> =
            (0..50).map(|step_no| crawler_position("claudebot", step_no, &roads)).collect();
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
        eprintln!("a frog's tour of 60,000 road tiles takes {:?}", start.elapsed());
    }

    #[test]
    fn roads_come_from_tiles_or_positions() {
        let mut tiles = highway_row(3, 0..=4);
        tiles.extend(block(Kind::Homes, 0, 0..=4, 4..=4));
        assert_eq!(Roads::from_tiles(&tiles), Roads::from_positions((0..=4).rev().map(|x| (x, 3))));
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
            Kind::House,
            Kind::Cafe,
            Kind::Club,
            Kind::Studio,
        ] {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        for word in ["", "Road", "palace", "homes ", "walk", "zone"] {
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
        for roads in [straight_road(100), tangled_roads(), straight_road(1), straight_road(2)] {
            for n in [0, 1, 2, 77, 1 << 40, u64::MAX] {
                let mut last = visitor_position(n, 0, &roads).unwrap();
                for step_no in 1..400 {
                    let now = visitor_position(n, step_no, &roads).unwrap();
                    assert!(roads.find(now).is_some(), "visitor {n} is off the roads at {now:?}");
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
        assert_eq!(visitor_position(5, 12, &roads), visitor_position(5, 12, &roads));
        assert_eq!(
            visitor_position(5, 12, &roads),
            visitor_position(5, 12, &Roads::from_positions(roads.sorted.iter().rev().copied()))
        );
        let at_zero: HashSet<Option<Pos>> =
            (0..30).map(|n| visitor_position(n, 0, &roads)).collect();
        assert!(at_zero.len() >= 12, "visitors start in the same few places");
        let walk: HashSet<Option<Pos>> =
            (0..60).map(|step_no| visitor_position(9, step_no, &roads)).collect();
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

    // What a world adds up to

    // W23, W48
    #[test]
    fn a_world_and_a_town_count_roads_and_places_and_nothing_else() {
        let mut tiles = highway_row(10, 0..=4);
        tiles.insert((1, 11), tile(Kind::House, 0, Some(1)));
        tiles.insert((2, 11), tile(Kind::Cafe, 0, Some(1)));
        tiles.insert((3, 11), tile(Kind::Homes, 3, Some(1)));
        tiles.insert((4, 11), tile(Kind::Park, 0, Some(1)));
        tiles.insert((5, 11), tile(Kind::Cleared, 0, Some(1)));
        assert_eq!(stats(&tiles), Stats { places: 2, roads: 5, stage: "hamlet" });
        assert_eq!(town(&tiles), Town { roads: 5, places: 2 });
        let sent = serde_json::to_value(town(&tiles)).unwrap();
        assert_eq!(sent, serde_json::json!({ "roads": 5, "places": 2 }));
    }

    // W49
    #[test]
    fn each_ladder_goes_through_its_rungs_in_order_and_a_town_that_slips_goes_back() {
        let at = |roads, places, person| goal(&Town { roads, places }, person);
        let climbed =
            [at(0, 0, true), at(0, 1, true), at(0, 2, true), at(0, 3, true), at(0, 4, true)];
        assert_eq!(
            climbed,
            [GOAL_HOUSE, GOAL_SECOND_PLACE, GOAL_ALL_PLACES, GOAL_ALL_PLACES, GOAL_PERSON_OPEN]
        );
        let climbed =
            [at(0, 0, false), at(1, 0, false), at(9, 0, false), at(10, 0, false), at(30, 0, false)];
        assert_eq!(
            climbed,
            [GOAL_ROAD, GOAL_TEN_ROADS, GOAL_TEN_ROADS, GOAL_THIRTY_ROADS, GOAL_AGENT_OPEN]
        );
        assert!(
            GOALS.contains(&at(u32::MAX, u32::MAX, true))
                && GOALS.contains(&at(u32::MAX, 0, false))
        );
    }

    // W47
    #[test]
    fn a_tip_says_what_a_person_or_an_agent_can_do_next() {
        let road = highway_row(1, 0..=1);
        let mut house = Tiles::new();
        house.insert((1, 2), tile(Kind::House, 0, Some(1)));
        assert_eq!(tip(&Tiles::new(), true, 9), TIP_NO_HOUSE);
        assert_eq!(tip(&road, true, 9), TIP_NO_HOUSE, "a road isn't a house");
        assert_eq!(tip(&house, true, 9), TIP_PLACES_COME);
        assert_eq!(tip(&Tiles::new(), false, 9), TIP_NO_ROAD);
        assert_eq!(tip(&road, false, 9), TIP_MORE_ROADS);
        for (own, person) in [(&road, false), (&house, true)] {
            assert_eq!(tip(own, person, 0), TIP_OUT_OF_MOVES);
        }
    }

    // W15
    #[test]
    fn the_reserved_tiles_in_a_window_are_the_open_ones_a_place_cant_use_and_no_others() {
        let mut land = lawn(32);
        land.set(3, 12, Ground::Water);
        let mut tiles = highway_row(10, 0..=20);
        // A road already on a kept tile, and a place beside the highway, are not open.
        tiles.insert((3, 11), tile(Kind::Road, 0, Some(2)));
        tiles.insert((9, 9), tile(Kind::House, 0, Some(2)));
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
        assert_eq!(listed, expected, "the same tiles a place can't use, row by row");
        for &(x, y) in &listed {
            let refused = plan_place(&land, &tiles, 1, Kind::House, i64::from(x), i64::from(y));
            assert_eq!(refused, Err(KEPT));
        }
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
        // My road up from the highway, and a place that only the top of it serves.
        for y in 4..=7u16 {
            tiles.insert((5, y), tile(Kind::Road, 0, Some(7)));
        }
        tiles.insert((5, 1), tile(Kind::House, 0, Some(9)));
        // A dead end that serves nothing.
        tiles.insert((10, 7), tile(Kind::Road, 0, Some(7)));
        let mine = "you can only bulldoze what you built";
        // Only the tile and its neighbours are given, as the small read gives them.
        let near_only = |x: i64, y: i64| {
            let near: Tiles = tiles
                .iter()
                .filter(|(&(tx, ty), _)| {
                    (i64::from(tx) - x).abs() <= 1 && (i64::from(ty) - y).abs() <= 1
                })
                .map(|(pos, t)| (*pos, *t))
                .collect();
            plan_bulldoze_tile(&land, &near, 7, x, y, &nobody_gone)
        };
        assert_eq!(near_only(3, 8), Err(mine), "the highway has no builder");
        assert_eq!(near_only(5, 1), Err(mine), "a house someone else built");
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
}
