// What an outside says (docs/outside.md), for everything that draws one. A place's document came from
// the server, but anything odd in it reads as nothing, so bad data can't do harm, and a word this page
// doesn't know leaves that thing as it would otherwise be (E4).

// E13: a yard is this many cells each way, and a cell is this many of the 32 units across a tile.
export const GRID = 16;
export const CELL = 2;

// E15: a kind's size in cells, facing 0, as the catalog has it.
export const YARD_SIZES = {
  oak: [2, 2],
  pine: [2, 2],
  birch: [2, 2],
  maple: [2, 2],
  cherry_blossom: [2, 2],
  apple_tree: [2, 2],
  palm: [2, 2],
  willow: [3, 3],
  cypress: [1, 1],
  dead_tree: [2, 2],
  bush: [1, 1],
  topiary: [1, 1],
  rose_bush: [1, 1],
  hydrangea: [1, 1],
  fern: [1, 1],
  tall_grass: [1, 1],
  cactus: [1, 1],
  bamboo_clump: [1, 1],
  mushroom_ring: [2, 2],
  flower_bed: [2, 1],
  tulips: [1, 1],
  sunflowers: [1, 1],
  lavender: [1, 1],
  wildflowers: [2, 2],
  vegetable_patch: [3, 2],
  planter: [2, 1],
  compost_bin: [1, 1],
  greenhouse: [4, 3],
  garden_shed: [3, 3],
  beehive: [1, 1],
  chicken_coop: [3, 2],
  windmill: [2, 2],
  pond: [3, 3],
  koi_pond: [4, 3],
  fountain: [2, 2],
  bird_bath: [1, 1],
  well: [2, 2],
  pool: [5, 4],
  hot_tub: [3, 3],
  bench: [2, 1],
  picnic_table: [3, 2],
  patio_set: [2, 2],
  deck_chair: [1, 2],
  hammock: [3, 1],
  porch_swing: [2, 1],
  fire_pit: [2, 2],
  bbq_grill: [1, 1],
  parasol: [2, 2],
  gazebo: [4, 4],
  swing_set: [3, 2],
  slide: [3, 1],
  sandbox: [2, 2],
  trampoline: [3, 3],
  seesaw: [3, 1],
  tire_swing: [1, 1],
  basketball_hoop: [1, 1],
  tent: [3, 2],
  lamp_post: [1, 1],
  lantern: [1, 1],
  garden_light: [1, 1],
  torch: [1, 1],
  string_post: [1, 1],
  spotlight: [1, 1],
  campfire: [1, 1],
  mailbox: [1, 1],
  bird_house: [1, 1],
  gnome: [1, 1],
  flamingo: [1, 1],
  statue: [1, 1],
  sundial: [1, 1],
  rock: [1, 1],
  boulder: [2, 2],
  log_pile: [2, 1],
  barrel: [1, 1],
  flag_pole: [1, 1],
  bike: [2, 1],
  car: [4, 2],
  boat: [4, 2],
  doghouse: [2, 2],
  maple_tree: [2, 2],
  birch_tree: [2, 2],
  leaf_pile: [2, 1],
  rake: [1, 1],
  mums: [1, 1],
  corn_stalks: [1, 1],
  harvest_table: [4, 2],
  pumpkin_pie: [1, 1],
  cornucopia: [2, 1],
  apple_basket: [1, 1],
  cider_barrel: [2, 1],
  gourds: [2, 1],
  scarecrow: [1, 1],
  hay_bale: [2, 1],
  turkey: [1, 1],
  wreath: [1, 1],
  wheelbarrow: [2, 1],
  pumpkin: [1, 1],
  gravestone: [1, 1],
  skeleton: [1, 1],
  ghost: [1, 1],
  cauldron: [1, 1],
  snowman: [1, 1],
  christmas_tree: [2, 2],
  gift_pile: [1, 1],
};

// Where each kind's building stands with no `body`, in units, as the map has always drawn it.
const RECTS = {
  house: [7, 9, 25, 23],
  cafe: [6, 8, 26, 24],
  club: [6, 7, 26, 25],
  studio: [6, 8, 26, 24],
  lounge: [5, 8, 27, 24],
  bar: [6, 8, 26, 24],
  office: [7, 8, 25, 24],
  workshop: [5, 8, 27, 25],
  library: [6, 8, 26, 24],
};
// And in cells, whole cells that cover that, as the server has it.
const BODIES = {
  house: { x: 3, y: 4, w: 10, d: 8, floors: 1 },
  cafe: { x: 3, y: 4, w: 10, d: 8, floors: 1 },
  club: { x: 3, y: 3, w: 10, d: 10, floors: 2 },
  studio: { x: 3, y: 4, w: 10, d: 8, floors: 1 },
  lounge: { x: 2, y: 4, w: 12, d: 8, floors: 1 },
  bar: { x: 3, y: 4, w: 10, d: 8, floors: 2 },
  office: { x: 3, y: 4, w: 10, d: 8, floors: 2 },
  workshop: { x: 2, y: 4, w: 12, d: 9, floors: 2 },
  library: { x: 3, y: 4, w: 10, d: 8, floors: 2 },
};
// E36: how many tiles a place takes each way. Everything on a big one is drawn this much bigger.
export const spanOf = (kind) => (kind === 'square' || kind === 'green' ? 2 : 1);
// E14: a town square's centrepiece, in cells.
export const CENTERPIECE = { x: 6, y: 6, w: 4, h: 4 };

const isWhole = (value) => Number.isInteger(value);

export const keyOf = (outside) => JSON.stringify(outside);

// E3: a colour number as the page writes one, or `fallback`.
export function hexOf(value, fallback) {
  if (!isWhole(value) || value < 0 || value > 0xffffff) return fallback;
  return `#${value.toString(16).padStart(6, '0')}`;
}

// E10: what's chosen for a part, or an empty one.
export function partOf(outside, word) {
  const part = outside?.parts?.[word];
  return part && typeof part === 'object' ? part : {};
}

// A part's style if it's one of `known`, or `fallback`.
export function styleOf(outside, word, known, fallback) {
  const { style } = partOf(outside, word);
  return known.includes(style) ? style : fallback;
}

export const colorOf = (outside, word, fallback) => hexOf(partOf(outside, word).color, fallback);

export function numberOf(outside, word, fallback) {
  const { number } = partOf(outside, word);
  return isWhole(number) ? number : fallback;
}

export function isOn(outside, word, style) {
  const { on } = partOf(outside, word);
  return Array.isArray(on) && on.includes(style);
}

// E10: `door_side`, as the map's L and R, or null to face the road.
const DOOR_SIDES = new Map([['auto', null], ['left', 'L'], ['right', 'R']]);
export const doorSideOf = (outside) => DOOR_SIDES.get(partOf(outside, 'door_side').style) ?? null;

// E13: where a building stands, in cells, or null for a town square.
export function bodyOf(kind, outside) {
  const fallback = Object.hasOwn(BODIES, kind) ? BODIES[kind] : null;
  if (!fallback) return null;
  const body = outside?.body;
  if (!body || typeof body !== 'object') return fallback;
  const { x, y, w, d, floors } = body;
  if (![x, y, w, d, floors].every(isWhole)) return fallback;
  return { x, y, w, d, floors };
}

// Where a building stands, in units: [u0, v0, u1, v1]. With no `body` it's where it has always stood.
export function rectOf(kind, outside) {
  const body = outside?.body ? bodyOf(kind, outside) : null;
  if (!body) return Object.hasOwn(RECTS, kind) ? RECTS[kind] : null;
  return [body.x * CELL, body.y * CELL, (body.x + body.w) * CELL, (body.y + body.d) * CELL];
}

// E15: a yard's items, each with where it stands in units as it's turned now. An item of a kind this
// page doesn't know is left out (E4). `u` and `v` are its middle, on the ground.
export function itemsOf(outside) {
  const items = Array.isArray(outside?.items) ? outside.items : [];
  const out = [];
  for (const item of items) {
    if (!item || !Object.hasOwn(YARD_SIZES, item.kind) || !isWhole(item.x) || !isWhole(item.y)) continue;
    const size = YARD_SIZES[item.kind];
    const facing = isWhole(item.facing) ? item.facing & 3 : 0;
    const [w, h] = facing % 2 ? [size[1], size[0]] : size;
    const u0 = item.x * CELL;
    const v0 = item.y * CELL;
    const u1 = u0 + w * CELL;
    const v1 = v0 + h * CELL;
    out.push({ kind: item.kind, x: item.x, y: item.y, facing, w, h, color: item.color, u0, v0, u1, v1, u: (u0 + u1) / 2, v: (v0 + v1) / 2 });
  }
  return out;
}

export const petsOf = (outside) => (Array.isArray(outside?.pets) ? outside.pets.filter((pet) => pet && typeof pet === 'object') : []);
