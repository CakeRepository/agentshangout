// The Commons, drawn. A detailed isometric town painted in code on a canvas, in a late-90s
// isometric look: 2:1 dimetric tiles 64 by 32, pre-rendered looking sprites full of small details,
// warm light from one side, soft drop shadows. No images, fonts or libraries.
//
// How it stays quick: the screen is cut into cells (384 by 320 art pixels) that tile it exactly. The
// land, roads and buildings in a cell are painted into a cached canvas the first time it comes on
// screen, and repainted only when a tile in it changes. Every sprite is painted once at the
// screen's own resolution, so nothing is scaled up and edges stay clean, and tall sprites are
// trimmed to what they cover. Each frame blits the visible cells, then draws people, robots (in their
// cars when they go far), frogs and smoke live on top. Sprites for tiles just off screen are made in spare moments, offscreen
// things aren't drawn, and the loop stops when the page is hidden or scrolled out of view.

import { EYES, FABRICS, HAIRS, LEGS, SKINS, hash, skyFor } from './room.js';

// ---------------------------------------------------------------------------------------------
// Numbers

const HW = 32; // half a tile's width in art pixels
const HH = 16; // half a tile's height
const CELL_W = 384; // the screen is cut into cells this big (art pixels), each a cached canvas
const CELL_H = 320;
const CELL_M = 2; // cells overlap by this much, so no seam shows where they meet
const CW = CELL_W + CELL_M * 2;
const CH = CELL_H + CELL_M * 2;
const REACH_L = 92; // how far a tile's painting can reach left, right, up and down of its top vertex
const REACH_R = 72;
const REACH_U = 176;
const REACH_D = 72;
const CHUNK_PIXELS = 42e6; // canvas pixels the cell cache may hold
const SLAB = 22; // how thick the island's edge is
const WATER_Z = -5; // water sits a little below the land
const MIN_ZOOM = 0.5;
const MAX_ZOOM = 3;
const ZOOM_STEPS = [0.5, 0.65, 0.8, 1, 1.25, 1.5, 2, 2.5, 3];

const LAND = { g: 0, f: 1, w: 2, s: 3, h: 4 };
const GRASS_L = 0;
const FOREST_L = 1;
const WATER_L = 2;
const SAND_L = 3;
const HILL_L = 4;

const K_ROAD = 1;
const K_HOMES = 2;
const K_SHOPS = 3;
const K_WORKS = 4;
const K_PARK = 5;
const K_CLEARED = 6;
const K_HOUSE = 7;
const K_CAFE = 8;
const K_CLUB = 9;
const K_STUDIO = 10;
const K_SQUARE = 11;
// W90: the newer places. A green is a park; the old city's park above was a zone.
const K_GREEN = 12;
const K_LOUNGE = 13;
const K_BAR = 14;
const K_OFFICE = 15;
const K_WORKSHOP = 16;
const K_LIBRARY = 17;
const KIND_NAME = ['', 'road', 'homes', 'shops', 'workshops', 'park', 'cleared', 'house', 'cafe', 'club', 'studio', 'square', 'green', 'lounge', 'bar', 'office', 'workshop', 'library'];
const KIND_ID = Object.fromEntries(KIND_NAME.map((name, id) => [name, id]).slice(1));
const KIND_LABEL = ['', 'Road', 'Homes', 'Shops', 'Workshops', 'Park', 'Cleared land', 'House', 'Cafe', 'Club', 'Studio', 'Town square', 'Park', 'Lounge', 'Bar', 'Office', 'Workshop', 'Library'];
// W91: places that take 2 by 2 tiles, drawn a piece a tile.
const isBig = (k) => k === K_SQUARE || k === K_GREEN;
const isZone = (k) => k >= K_HOMES && k <= K_WORKS;

const EMOTES = new Set(['wave', 'cheer', 'think', 'sleep', 'dance']);
const STAGE_WORD = { hamlet: 'a hamlet', village: 'a village', town: 'a town', city: 'a city' };

// ---------------------------------------------------------------------------------------------
// Small tools

const clamp = (v, lo, hi) => (v < lo ? lo : v > hi ? hi : v);
const lerp = (a, b, t) => a + (b - a) * t;
const ease = (t) => t * t * (3 - 2 * t);
const easeOut = (t) => 1 - (1 - t) * (1 - t) * (1 - t);

// A fast integer hash of two numbers and a salt.
function ih(x, y, s = 0) {
  let h = Math.imul(x | 0, 0x27d4eb2d) ^ Math.imul(y | 0, 0x165667b1) ^ Math.imul(s | 0, 0x9e3779b1);
  h = Math.imul(h ^ (h >>> 15), 0x85ebca6b);
  h = Math.imul(h ^ (h >>> 13), 0xc2b2ae35);
  return (h ^ (h >>> 16)) >>> 0;
}
const rnd = (x, y, s = 0) => ih(x, y, s) / 4294967296;

function vnoise(x, y, seed) {
  const x0 = Math.floor(x);
  const y0 = Math.floor(y);
  const fx = ease(x - x0);
  const fy = ease(y - y0);
  const a = lerp(rnd(x0, y0, seed), rnd(x0 + 1, y0, seed), fx);
  const b = lerp(rnd(x0, y0 + 1, seed), rnd(x0 + 1, y0 + 1, seed), fx);
  return lerp(a, b, fy);
}

function seeded(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const colourCache = new Map();
function parse(hex) {
  let c = colourCache.get(hex);
  if (!c) {
    const n = parseInt(hex.slice(1), 16);
    c = [n >> 16, (n >> 8) & 255, n & 255];
    colourCache.set(hex, c);
  }
  return c;
}
function toHex(r, g, b) {
  return `#${((1 << 24) | (Math.round(r) << 16) | (Math.round(g) << 8) | Math.round(b)).toString(16).slice(1)}`;
}
function mixHex(a, b, t) {
  const p = parse(a);
  const q = parse(b);
  return toHex(lerp(p[0], q[0], t), lerp(p[1], q[1], t), lerp(p[2], q[2], t));
}
// Warm light, cool shadow: lighter colours lean to cream, darker ones to a dusky blue.
const lite = (c, t) => mixHex(c, '#fff4d6', t);
const dark = (c, t) => mixHex(c, '#241f3c', t);
function rgba(hex, a) {
  const [r, g, b] = parse(hex);
  return `rgba(${r},${g},${b},${a})`;
}

function hslHex(h, s, l) {
  h = ((h % 360) + 360) % 360;
  s /= 100;
  l /= 100;
  const k = (n) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  const f = (n) => l - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
  return toHex(f(0) * 255, f(8) * 255, f(4) * 255);
}

function makeCanvas(w, h) {
  const c = document.createElement('canvas');
  c.width = w;
  c.height = h;
  return c;
}

// ---------------------------------------------------------------------------------------------
// Drawing tools. Everything is vector-painted at the sprite's own resolution.

// Iso projection: u runs down-right, v runs down-left, z runs up. A tile is 32 by 32 in u and v,
// which makes it 64 wide and 32 tall on the screen.
const P = (u, v, z = 0) => [u - v, (u + v) / 2 - z];

function pathOf(g, pts) {
  g.beginPath();
  g.moveTo(pts[0][0], pts[0][1]);
  for (let i = 1; i < pts.length; i++) g.lineTo(pts[i][0], pts[i][1]);
  g.closePath();
}
function fp(g, fill, pts) {
  pathOf(g, pts);
  g.fillStyle = fill;
  g.fill();
}
function fp3(g, fill, ...pts) {
  fp(g, fill, pts.map((p) => P(p[0], p[1], p[2] || 0)));
}
function ell(g, fill, cx, cy, rx, ry) {
  g.beginPath();
  g.ellipse(cx, cy, rx, ry, 0, 0, 6.2832);
  g.fillStyle = fill;
  g.fill();
}
function grad(g, x0, y0, x1, y1, ...stops) {
  const gr = g.createLinearGradient(x0, y0, x1, y1);
  stops.forEach((c, i) => gr.addColorStop(i / (stops.length - 1), c));
  return gr;
}
function rr(g, fill, x, y, w, h) {
  g.fillStyle = fill;
  g.fillRect(x, y, w, h);
}
function line(g, color, w, x0, y0, x1, y1) {
  g.strokeStyle = color;
  g.lineWidth = w;
  g.beginPath();
  g.moveTo(x0, y0);
  g.lineTo(x1, y1);
  g.stroke();
}

// Film grain: a fixed speckle laid over a clip, to take the flat look off big surfaces.
let grainCanvas = null;
function grain(g, alpha, pts) {
  if (!grainCanvas) {
    grainCanvas = makeCanvas(64, 64);
    const gg = grainCanvas.getContext('2d');
    const img = gg.createImageData(64, 64);
    const r = seeded(99);
    for (let i = 0; i < 64 * 64; i++) {
      const light = r() < 0.5;
      img.data[i * 4] = light ? 255 : 18;
      img.data[i * 4 + 1] = light ? 246 : 20;
      img.data[i * 4 + 2] = light ? 222 : 40;
      img.data[i * 4 + 3] = r() < 0.55 ? Math.floor(r() * 130) : 0;
    }
    gg.putImageData(img, 0, 0);
  }
  g.save();
  if (pts) {
    pathOf(g, pts);
    g.clip();
  }
  g.globalAlpha = alpha;
  g.fillStyle = g.createPattern(grainCanvas, 'repeat');
  g.fillRect(-400, -400, 900, 900);
  g.restore();
}

// Draw in the flat plane of a wall: x runs along it, y runs down, so a wall stands at negative y.
// A left face looks down-left (plane v = v1, running along u). A right face looks down-right
// (plane u = u1, running along v from v1 backwards).
function onL(g, u0, v1, fn) {
  const [x, y] = P(u0, v1, 0);
  g.save();
  g.transform(1, 0.5, 0, 1, x, y);
  fn();
  g.restore();
}
function onR(g, u1, v1, fn) {
  const [x, y] = P(u1, v1, 0);
  g.save();
  g.transform(1, -0.5, 0, 1, x, y);
  fn();
  g.restore();
}

// ---------------------------------------------------------------------------------------------
// Sprites: painted once in code at the screen's own resolution, then stamped everywhere.

let LOD = 1;
const sprites = new Map();
let spriteMs = 0;
let spritePixels = 0;
let spriteStamp = 0;
const SPRITE_PIXELS = 30e6; // canvas pixels the sprite cache may hold before the stalest go

// Changes the resolution sprites are painted at. Sprites of other resolutions are dropped from
// the cache (anything still holding one keeps working).
function useLod(n) {
  if (n === LOD) return false;
  LOD = n;
  for (const [k, v] of [...sprites]) {
    if (k.charCodeAt(0) - 48 !== n) {
      spritePixels -= v.c.width * v.c.height;
      sprites.delete(k);
    }
  }
  return true;
}

// Once a frame: note the time, and if the cache has grown too big, drop what hasn't been used lately.
function sweepSprites() {
  spriteStamp++;
  if (spritePixels <= SPRITE_PIXELS || spriteStamp % 20) return;
  const old = [...sprites].sort((p, q) => p[1].used - q[1].used);
  for (const [k, v] of old) {
    if (spritePixels <= SPRITE_PIXELS * 0.75 || v.used >= spriteStamp - 30) break;
    spritePixels -= v.c.width * v.c.height;
    sprites.delete(k);
  }
}

// A stand-in for a canvas context that paints nothing and only works out where paint would land.
// Running a sprite's drawing through this first tells us how big its canvas needs to be, for the
// price of the drawing code alone, with no pixels and no readback.
class Extent {
  constructor() {
    this.m = [1, 0, 0, 1, 0, 0];
    this.stack = [];
    this.pts = [];
    this.clipBox = null;
    this.lineWidth = 1;
    this.fillStyle = '#000';
    this.strokeStyle = '#000';
    this.globalAlpha = 1;
    this.globalCompositeOperation = 'source-over';
    this.lineCap = 'butt';
    this.lineJoin = 'miter';
    this.x0 = Infinity;
    this.y0 = Infinity;
    this.x1 = -Infinity;
    this.y1 = -Infinity;
  }

  save() {
    this.stack.push([this.m.slice(), this.clipBox, this.lineWidth]);
  }

  restore() {
    const top = this.stack.pop();
    if (top) [this.m, this.clipBox, this.lineWidth] = top;
  }

  setTransform(a, b, c, d, e, f) {
    this.m = [a, b, c, d, e, f];
  }

  transform(a, b, c, d, e, f) {
    const m = this.m;
    this.m = [m[0] * a + m[2] * b, m[1] * a + m[3] * b, m[0] * c + m[2] * d, m[1] * c + m[3] * d, m[0] * e + m[2] * f + m[4], m[1] * e + m[3] * f + m[5]];
  }

  translate(x, y) {
    this.transform(1, 0, 0, 1, x, y);
  }

  scale(x, y) {
    this.transform(x, 0, 0, y, 0, 0);
  }

  rotate(r) {
    const c = Math.cos(r);
    const s = Math.sin(r);
    this.transform(c, s, -s, c, 0, 0);
  }

  at(x, y) {
    const m = this.m;
    return [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
  }

  beginPath() {
    this.pts = [];
  }

  add(x, y) {
    this.pts.push(this.at(x, y));
  }

  moveTo(x, y) {
    this.add(x, y);
  }

  lineTo(x, y) {
    this.add(x, y);
  }

  closePath() {}

  quadraticCurveTo(cx, cy, x, y) {
    this.add(cx, cy);
    this.add(x, y);
  }

  bezierCurveTo(a, b, c, d, x, y) {
    this.add(a, b);
    this.add(c, d);
    this.add(x, y);
  }

  arcTo(x1, y1, x2, y2) {
    this.add(x1, y1);
    this.add(x2, y2);
  }

  arc(x, y, r) {
    this.ellipse(x, y, r, r);
  }

  ellipse(x, y, rx, ry) {
    const r = Math.max(rx, ry);
    this.add(x - r, y - r);
    this.add(x + r, y - r);
    this.add(x + r, y + r);
    this.add(x - r, y + r);
  }

  rect(x, y, w, h) {
    this.add(x, y);
    this.add(x + w, y);
    this.add(x + w, y + h);
    this.add(x, y + h);
  }

  box(pts, pad) {
    let x0 = Infinity;
    let y0 = Infinity;
    let x1 = -Infinity;
    let y1 = -Infinity;
    for (const [x, y] of pts) {
      if (x < x0) x0 = x;
      if (x > x1) x1 = x;
      if (y < y0) y0 = y;
      if (y > y1) y1 = y;
    }
    return [x0 - pad, y0 - pad, x1 + pad, y1 + pad];
  }

  land(b) {
    const c = this.clipBox;
    if (c) b = [Math.max(b[0], c[0]), Math.max(b[1], c[1]), Math.min(b[2], c[2]), Math.min(b[3], c[3])];
    if (b[0] >= b[2] || b[1] >= b[3]) return;
    this.x0 = Math.min(this.x0, b[0]);
    this.y0 = Math.min(this.y0, b[1]);
    this.x1 = Math.max(this.x1, b[2]);
    this.y1 = Math.max(this.y1, b[3]);
  }

  fill() {
    if (this.pts.length) this.land(this.box(this.pts, 0));
  }

  stroke() {
    if (this.pts.length) this.land(this.box(this.pts, this.lineWidth * 0.75 * Math.hypot(this.m[0], this.m[1])));
  }

  clip() {
    if (!this.pts.length) return;
    const b = this.box(this.pts, 0);
    const c = this.clipBox;
    this.clipBox = c ? [Math.max(b[0], c[0]), Math.max(b[1], c[1]), Math.min(b[2], c[2]), Math.min(b[3], c[3])] : b;
  }

  fillRect(x, y, w, h) {
    this.land(this.box([this.at(x, y), this.at(x + w, y), this.at(x + w, y + h), this.at(x, y + h)], 0));
  }

  drawImage(img, ...a) {
    const [x, y, w, h] = a.length >= 8 ? a.slice(4, 8) : a.length >= 4 ? a.slice(0, 4) : [a[0], a[1], img.width, img.height];
    this.fillRect(x, y, w, h);
  }

  clearRect() {}

  createLinearGradient() {
    return { addColorStop() {} };
  }

  createRadialGradient() {
    return { addColorStop() {} };
  }

  createPattern() {
    return {};
  }
}

// w and h are in art pixels, (ax, ay) is where the tile's top vertex sits inside the sprite. With
// `trim`, the canvas is cut down to what the painting actually covers, which saves a lot of memory
// for tall things that are mostly empty space.
function sprite(key, w, h, ax, ay, draw, trim = false) {
  const k = `${LOD}|${key}`;
  let s = sprites.get(k);
  if (s) {
    s.used = spriteStamp;
    return s;
  }
  const t0 = performance.now();
  let x0 = 0;
  let y0 = 0;
  let sw = w;
  let sh = h;
  if (trim) {
    const probe = new Extent();
    probe.translate(ax, ay);
    draw(probe);
    if (probe.x1 > probe.x0) {
      // Back into the sprite's own coordinates, with a margin for soft edges.
      x0 = clamp(Math.floor(probe.x0) - 3, 0, w - 1);
      y0 = clamp(Math.floor(probe.y0) - 3, 0, h - 1);
      sw = clamp(Math.ceil(probe.x1) + 3, x0 + 1, w) - x0;
      sh = clamp(Math.ceil(probe.y1) + 3, y0 + 1, h) - y0;
    } else {
      sw = sh = 1;
    }
  }
  const c = makeCanvas(Math.ceil(sw * LOD), Math.ceil(sh * LOD));
  const g = c.getContext('2d');
  g.setTransform(LOD, 0, 0, LOD, (ax - x0) * LOD, (ay - y0) * LOD);
  draw(g);
  s = { c, ax: ax - x0, ay: ay - y0, w: sw, h: sh, lod: LOD, used: spriteStamp };
  spritePixels += c.width * c.height;
  sprites.set(k, s);
  spriteMs += performance.now() - t0;
  return s;
}

// ---------------------------------------------------------------------------------------------
// Land

const BLEED = 1.6; // ground tiles spill a hair over their edges so chunks never show seams
const DIA = [[0, -BLEED], [HW + BLEED * 2, HH], [0, 32 + BLEED], [-HW - BLEED * 2, HH]];

function tile(key, draw) {
  return sprite(key, 72, 40, 36, 4, (g) => {
    g.save();
    pathOf(g, DIA);
    g.clip();
    draw(g);
    g.restore();
  });
}

const GRASS = ['#6f9d3d', '#74a141', '#79a645', '#72a03f'];
// Open meadow runs through ten close shades, so large soft patches of lighter and darker grass roll
// across the land and no single tile stands out from its neighbours.
const GRASS_RAMP = Array.from({ length: 10 }, (_, i) => mixHex('#5f8d33', '#86b44c', i / 9));
const SAND = ['#dcc488', '#d5ba7a', '#e3cf98'];
const FOREST_GROUND = ['#527f35', '#57863a', '#5b8b3e'];
const HILL_TOP = ['#74934a', '#7c9b50', '#849f58'];
const FLOWER_COLORS = ['#fffbe8', '#ffd84a', '#ff8fb3', '#c9a8ff', '#ff7a5a'];

function flowerBunch(g, rand, x, y, n) {
  for (let i = 0; i < n; i++) {
    const fx = x + (rand() - 0.5) * 7;
    const fy = y + (rand() - 0.5) * 3.5;
    line(g, '#3d6f2a', 0.7, fx, fy, fx, fy - 2.4);
    ell(g, FLOWER_COLORS[Math.floor(rand() * FLOWER_COLORS.length)], fx, fy - 2.8, 1.25, 1.05);
    rr(g, '#e9a21a', fx - 0.3, fy - 3, 0.6, 0.6);
  }
}

function grassSprite(tone, variant) {
  return tile(`grass:${tone}:${variant}`, (g) => {
    const base = GRASS_RAMP[tone];
    const rand = seeded(tone * 131 + variant * 17 + 3);
    // Nearly flat, so a row of tiles reads as one meadow and not a lattice of lit and shaded diamonds.
    g.fillStyle = grad(g, -32, 0, 32, 32, lite(base, 0.012), dark(base, 0.014));
    g.fillRect(-40, -6, 80, 46);
    // Soft mottling, like a mown lawn, kept off the tile's edges so no seam shows between two tiles.
    for (let i = 0; i < 28; i++) {
      const [x, y] = [(rand() - 0.5) * 40, 7 + rand() * 18];
      const r = 2 + rand() * 5;
      ell(g, rand() < 0.55 ? rgba(lite(base, 0.34), 0.15) : rgba(dark(base, 0.36), 0.16), x, y, r * 1.8, r * 0.9);
    }
    g.lineWidth = 0.8;
    for (let i = 0; i < 46; i++) {
      const x = (rand() - 0.5) * 64;
      const y = 3 + rand() * 29;
      g.strokeStyle = rand() < 0.5 ? rgba(dark(base, 0.42), 0.6) : rgba(lite(base, 0.45), 0.6);
      g.beginPath();
      g.moveTo(x, y);
      g.lineTo(x + (rand() - 0.5) * 1.8, y - 1.6 - rand() * 1.6);
      g.stroke();
    }
    grain(g, 0.06);
    if (variant === 1) flowerBunch(g, rand, (rand() - 0.5) * 30, 14 + rand() * 6, 5);
    else if (variant === 2) {
      for (let i = 0; i < 6; i++) {
        const x = (rand() - 0.5) * 36;
        const y = 8 + rand() * 18;
        ell(g, rgba(lite(base, 0.4), 0.55), x, y, 2.4, 1.3);
        ell(g, rgba(dark(base, 0.3), 0.5), x + 1.6, y + 0.8, 1.6, 0.8);
      }
    } else if (variant === 3) {
      for (let i = 0; i < 4; i++) {
        const x = (rand() - 0.5) * 30;
        const y = 10 + rand() * 14;
        ell(g, 'rgba(30,30,40,.28)', x + 0.8, y + 1, 2.4, 1.2);
        ell(g, '#9da09c', x, y, 2.2, 1.5);
        ell(g, '#c9ccc6', x - 0.6, y - 0.5, 1.2, 0.7);
      }
    } else if (variant === 4) {
      for (let i = 0; i < 6; i++) ell(g, rgba('#b8a45a', 0.22), (rand() - 0.5) * 36, 8 + rand() * 18, 4 + rand() * 4, 1.6 + rand() * 1.5);
    } else if (variant === 5) {
      const x = (rand() - 0.5) * 22;
      const y = 15 + rand() * 8;
      for (let i = 0; i < 9; i++) {
        const a = -1.2 + i * 0.3;
        line(g, i % 2 ? '#4d7f2c' : '#93bf55', 1, x, y, x + Math.sin(a) * 4, y - 3.5 - Math.cos(a) * 3);
      }
    }
  });
}

function sandSprite(tone, variant) {
  return tile(`sand:${tone}:${variant}`, (g) => {
    const base = SAND[tone];
    const rand = seeded(tone * 13 + variant * 5 + 9);
    g.fillStyle = grad(g, -32, 0, 32, 32, lite(base, 0.04), dark(base, 0.04));
    g.fillRect(-40, -6, 80, 46);
    for (let i = 0; i < 14; i++) {
      ell(g, rand() < 0.5 ? rgba(lite(base, 0.5), 0.25) : rgba(dark(base, 0.3), 0.2), (rand() - 0.5) * 66, rand() * 34, 3 + rand() * 6, 1 + rand() * 2);
    }
    g.lineWidth = 0.7;
    g.strokeStyle = rgba(dark(base, 0.3), 0.35);
    for (let i = 0; i < 5; i++) {
      const x = (rand() - 0.5) * 40;
      const y = 6 + rand() * 22;
      g.beginPath();
      g.moveTo(x, y);
      g.quadraticCurveTo(x + 3, y - 1.2, x + 7, y);
      g.stroke();
    }
    for (let i = 0; i < 18; i++) rr(g, rand() < 0.5 ? rgba('#9a8452', 0.7) : rgba('#fff3cf', 0.7), (rand() - 0.5) * 60, 2 + rand() * 28, 1, 1);
    grain(g, 0.06);
    if (variant === 2) {
      const x = (rand() - 0.5) * 24;
      const y = 14 + rand() * 8;
      ell(g, '#f0b9a6', x, y, 1.8, 1.3);
      ell(g, '#fff0e6', x - 0.5, y - 0.4, 0.8, 0.5);
    }
  });
}

// The surface of the water. Edges: 1 north-east, 2 south-east, 4 south-west, 8 north-west.
function waterSprite(depth, variant, edges) {
  return tile(`water:${depth}:${variant}:${edges}`, (g) => {
    const shades = [['#55a6d4', '#4c9dd0'], ['#4a98cb', '#4290c4'], ['#3f89be', '#3881b8']][depth];
    const rand = seeded(depth * 91 + variant * 17 + 3);
    g.fillStyle = grad(g, -20, 0, 20, 34, shades[0], shades[1]);
    g.fillRect(-40, -6, 80, 46);
    for (let i = 0; i < 10; i++) {
      const x = (rand() - 0.5) * 56;
      const y = 3 + rand() * 27;
      g.lineWidth = 0.9;
      g.strokeStyle = rand() < 0.65 ? 'rgba(255,255,255,.34)' : 'rgba(10,50,100,.22)';
      g.beginPath();
      g.moveTo(x, y);
      g.quadraticCurveTo(x + 3, y - 1.4, x + 6, y);
      g.stroke();
    }
    grain(g, 0.05);
    // Foam along any edge that meets land: a few soft strokes, widest and faintest first.
    const edge = (x0, y0, x1, y1) => {
      for (const [w, a] of [[15, 0.1], [10, 0.16], [6, 0.3], [3, 0.55], [1.4, 0.9]]) {
        g.lineWidth = w;
        g.strokeStyle = `rgba(247,252,255,${a})`;
        g.beginPath();
        g.moveTo(x0, y0);
        g.lineTo(x1, y1);
        g.stroke();
      }
      for (let i = 0; i < 9; i++) {
        const t = rand();
        const px = lerp(x0, x1, t);
        const py = lerp(y0, y1, t);
        ell(g, 'rgba(255,255,255,.8)', px + (rand() - 0.5) * 3, py + (rand() - 0.5) * 3 + (y0 < 16 ? 2.5 : -2.5), 0.9 + rand() * 0.9, 0.7);
      }
    };
    if (edges & 1) edge(0, 0, 32, 16);
    if (edges & 2) edge(32, 16, 0, 32);
    if (edges & 4) edge(0, 32, -32, 16);
    if (edges & 8) edge(-32, 16, 0, 0);
  });
}

function forestGround(tone, variant) {
  return tile(`fground:${tone}:${variant}`, (g) => {
    const base = FOREST_GROUND[tone];
    const rand = seeded(tone * 19 + variant * 3 + 5);
    g.fillStyle = grad(g, -32, 0, 32, 32, lite(base, 0.012), dark(base, 0.02));
    g.fillRect(-40, -6, 80, 46);
    for (let i = 0; i < 24; i++) {
      ell(g, rand() < 0.5 ? rgba(dark(base, 0.5), 0.26) : rgba('#8b6f3a', 0.18), (rand() - 0.5) * 48, 3 + rand() * 26, 2 + rand() * 5, 1 + rand() * 2.5);
    }
    for (let i = 0; i < 18; i++) rr(g, rand() < 0.5 ? rgba('#b88b3a', 0.7) : rgba('#7c5a2a', 0.7), (rand() - 0.5) * 60, 3 + rand() * 27, 1.4, 0.8);
    grain(g, 0.07);
  });
}

function boulder(g, x, y, s) {
  ell(g, 'rgba(25,25,35,.3)', x + s * 0.5, y + 0.5, s * 1.3, s * 0.5);
  const pts = [[x - s, y], [x - s * 0.8, y - s * 0.8], [x - s * 0.1, y - s * 1.1], [x + s * 0.8, y - s * 0.7], [x + s * 1.05, y]];
  fp(g, grad(g, x - s, y - s, x + s, y, '#b9b8ac', '#7d7d78'), pts);
  fp(g, 'rgba(255,255,255,.3)', [[x - s * 0.8, y - s * 0.8], [x - s * 0.1, y - s * 1.1], [x + s * 0.2, y - s * 0.7], [x - s * 0.5, y - s * 0.45]]);
  line(g, 'rgba(40,40,50,.35)', 0.7, x - s * 0.1, y - s * 0.9, x + s * 0.1, y - s * 0.2);
}

function hillTop(tone, variant) {
  return tile(`hilltop:${tone}:${variant}`, (g) => {
    const base = HILL_TOP[tone];
    const rand = seeded(tone * 23 + variant * 11 + 2);
    g.fillStyle = grad(g, -32, 0, 32, 32, lite(base, 0.04), dark(base, 0.05));
    g.fillRect(-40, -6, 80, 46);
    for (let i = 0; i < 24; i++) {
      ell(g, rand() < 0.5 ? rgba(lite(base, 0.3), 0.2) : rgba('#7b6a45', 0.2), (rand() - 0.5) * 66, rand() * 34, 2 + rand() * 6, 1 + rand() * 2.5);
    }
    g.lineWidth = 0.8;
    for (let i = 0; i < 34; i++) {
      const x = (rand() - 0.5) * 62;
      const y = 3 + rand() * 28;
      g.strokeStyle = rand() < 0.5 ? rgba(dark(base, 0.4), 0.55) : rgba(lite(base, 0.45), 0.55);
      g.beginPath();
      g.moveTo(x, y);
      g.lineTo(x + (rand() - 0.5) * 1.6, y - 1.6 - rand() * 1.4);
      g.stroke();
    }
    grain(g, 0.07);
    if (variant === 1) boulder(g, (rand() - 0.5) * 14, 18 + rand() * 4, 3.5);
  });
}

// The side of a block of land, from height `from` down to `to`. Material picks the colours.
const MATERIALS = {
  bank: { top: '#8f7249', bottom: '#654c30', lip: '#5f8a34' },
  sand: { top: '#dac48c', bottom: '#b19a66', lip: '#e6d29b' },
  rock: { top: '#a3a296', bottom: '#6b6c66', lip: '#6f9a45' },
  earth: { top: '#8d6c46', bottom: '#4d3a27', lip: '#5f8a34' },
  wall: { top: '#5aa5d4', bottom: '#245a8c', lip: null },
};

function faceSprite(side, mat, from, to, seed) {
  const rows = from - to;
  return sprite(`face:${side}:${mat}:${from}:${to}:${seed}`, 40, 24 + rows, side === 'L' ? 36 : 2, from - 12, (g) => {
    const m = MATERIALS[mat];
    const rand = seeded(seed * 5 + from * 3 + to + 1);
    const poly = side === 'L'
      ? [[-32, 16 - from], [0, 32 - from], [0, 32 - to], [-32, 16 - to]]
      : [[0, 32 - from], [32, 16 - from], [32, 16 - to], [0, 32 - to]];
    g.save();
    pathOf(g, poly);
    g.clip();
    if (side === 'L') g.transform(1, 0.5, 0, 1, -32, 16);
    else g.transform(1, -0.5, 0, 1, 0, 32);
    // Local plane: x along the face (0..32), y = minus the height.
    g.fillStyle = grad(g, 0, -from, 0, -to, m.top, mat === 'earth' ? mixHex(m.top, m.bottom, 0.45) : m.bottom, m.bottom);
    g.fillRect(-2, -from - 2, 36, rows + 4);
    // Strata and stones.
    for (let y = -from + 5; y < -to; y += 4 + Math.floor(rand() * 4)) {
      line(g, 'rgba(15,10,25,.22)', 1, 0, y, 32, y + (rand() - 0.5) * 1.5);
      line(g, 'rgba(255,240,200,.14)', 1, 0, y + 1, 32, y + 1);
    }
    for (let i = 0; i < Math.max(6, rows * 0.9); i++) {
      const sx = rand() * 32;
      const sy = -from + 3 + rand() * rows;
      if (mat === 'wall') ell(g, 'rgba(255,255,255,.14)', sx, sy, 2.5, 0.6);
      else ell(g, rand() < 0.5 ? 'rgba(255,240,205,.2)' : 'rgba(20,12,10,.22)', sx, sy, 1.2 + rand() * 1.8, 0.8 + rand());
    }
    grain(g, 0.08);
    if (m.lip) {
      // A ragged edge of turf along the top.
      for (let x = -1; x < 34; x += 2.2) {
        ell(g, rand() < 0.5 ? m.lip : lite(m.lip, 0.2), x, -from + 0.6, 1.9 + rand() * 0.8, 1.3 + rand() * 1.8);
      }
    } else {
      line(g, 'rgba(235,248,255,.7)', 1.2, 0, -from + 0.8, 32, -from + 0.8);
    }
    if (side === 'R') {
      g.fillStyle = 'rgba(30,20,60,.3)';
      g.fillRect(-2, -from - 2, 36, rows + 4);
    }
    g.restore();
  });
}

// ---------------------------------------------------------------------------------------------
// Trees and rocks: cauliflower canopies built from many leaf clusters.

function canopy(g, cx, cy, rx, ry, cols, rand, n, size = 1) {
  const blobs = [];
  for (let i = 0; i < n; i++) {
    const a = rand() * 6.2832;
    const d = Math.sqrt(rand());
    blobs.push([cx + Math.cos(a) * d * rx * 0.7, cy + Math.sin(a) * d * ry * 0.7, (4.2 + rand() * 3.4) * size]);
  }
  blobs.sort((a, b) => a[1] - b[1]);
  for (const [x, y, r] of blobs) ell(g, cols[0], x + 0.5, y + 1.2, r * 1.14, r * 1.02);
  for (const [x, y, r] of blobs) ell(g, cols[1], x - 0.4, y - 0.4, r, r * 0.92);
  for (const [x, y, r] of blobs) {
    ell(g, rgba(cols[2], 0.9), x - r * 0.3, y - r * 0.38, r * 0.58, r * 0.46);
    ell(g, rgba(cols[3] || cols[2], 0.7), x - r * 0.42, y - r * 0.5, r * 0.28, r * 0.2);
  }
  for (let i = 0; i < n * 1.3; i++) {
    const a = rand() * 6.2832;
    const d = Math.sqrt(rand());
    rr(g, rgba(cols[0], 0.7), cx + Math.cos(a) * d * rx * 0.8, cy + Math.sin(a) * d * ry * 0.8 + 2, 1.4, 1);
  }
}

const LEAF = {
  oak: ['#2d5f26', '#4a8a36', '#7db84a', '#b4dd70'],
  oak2: ['#365f20', '#58862d', '#8eb83e', '#c2dc68'],
  small: ['#2f6a2c', '#4c9440', '#82c25a', '#bfe58a'],
  blossom: ['#b7507a', '#e07fa6', '#f7b6cf', '#ffe0ec'],
  autumn: ['#9a4a1c', '#cf7a26', '#eab34a', '#f6d98a'],
};

function drawTree(g, x, y, type, seed) {
  const rand = seeded(seed * 13 + 1);
  if (type === 'pine') {
    ell(g, 'rgba(25,38,20,.3)', x + 7, y + 0.5, 11, 3.6);
    rr(g, grad(g, x - 2, 0, x + 2, 0, '#7a5530', '#4f3820'), x - 1.8, y - 8, 3.6, 8);
    const tiers = [[7, 17, 12], [14, 14, 11], [21, 11, 10], [27, 8, 9], [32, 5, 8]];
    for (const [dy, hw, h] of tiers) {
      const by = y - dy;
      const pts = [[x - hw, by]];
      for (let k = 1; k <= 6; k++) pts.push([x - hw + (hw * 2 * k) / 6, by + (k % 2 ? 2 : -0.5)]);
      pts.push([x, by - h]);
      fp(g, grad(g, x - hw, by, x + hw, by, '#3f7a3c', '#1f4a2c'), pts);
      fp(g, 'rgba(180,230,120,.28)', [[x - hw, by], [x - hw * 0.1, by - h * 0.9], [x, by - h], [x - hw * 0.45, by - 1]]);
      for (let k = 0; k < 4; k++) line(g, 'rgba(20,50,25,.4)', 0.8, x - hw * 0.7 + k * hw * 0.5, by - 1, x - hw * 0.5 + k * hw * 0.4, by - h * 0.45);
    }
    return;
  }
  if (type === 'bush') {
    ell(g, 'rgba(25,38,20,.3)', x + 3, y + 0.5, 8.5, 2.6);
    canopy(g, x, y - 5, 9, 6, LEAF.small, rand, 7, 0.8);
    if (seed % 3 === 0) for (let i = 0; i < 5; i++) ell(g, '#ff6f7e', x + (rand() - 0.5) * 11, y - 3 - rand() * 6, 0.9, 0.9);
    return;
  }
  if (type === 'rocks') {
    boulder(g, x - 4, y, 5);
    boulder(g, x + 5, y + 1, 3.5);
    return;
  }
  const big = type !== 'small';
  const cols = type === 'blossom' ? LEAF.blossom : type === 'autumn' ? LEAF.autumn : seed % 2 ? LEAF.oak2 : big ? LEAF.oak : LEAF.small;
  const th = big ? 12 : 8;
  ell(g, 'rgba(25,38,20,.3)', x + 7, y + 0.5, big ? 13 : 9, big ? 4 : 3);
  // Trunk, flaring at the foot, with a branch or two.
  const tw = big ? 2.6 : 1.8;
  fp(g, grad(g, x - tw, 0, x + tw, 0, '#8a6238', '#4a321c'), [[x - tw - 1.4, y], [x - tw, y - th * 0.5], [x - tw * 0.7, y - th - 3], [x + tw * 0.7, y - th - 3], [x + tw, y - th * 0.5], [x + tw + 1.4, y]]);
  line(g, '#5a3d22', 1.2, x, y - th, x - 5, y - th - 6);
  line(g, '#5a3d22', 1.2, x, y - th, x + 5, y - th - 5);
  canopy(g, x, y - th - (big ? 11 : 8), big ? 17 : 11, big ? 14 : 10, cols, rand, big ? 17 : 11, big ? 1 : 0.8);
}

// A tile of forest: three or four trees of mixed kinds, different for every variant.
function forestObject(variant) {
  return sprite(`forest:${variant}`, 80, 96, 40, 70, (g) => {
    const rand = seeded(variant * 101 + 7);
    const slots = [[8, 8], [24, 8], [8, 24], [24, 24], [16, 16]];
    const count = 3 + (variant % 2);
    const chosen = slots.slice().sort(() => rand() - 0.5).slice(0, count);
    chosen.sort((a, b) => a[0] + a[1] - (b[0] + b[1]));
    const pines = rand() < 0.4;
    for (const [u, v] of chosen) {
      const [x, y] = P(u + (rand() - 0.5) * 3, v + (rand() - 0.5) * 3);
      const roll = rand();
      drawTree(g, x, y, pines || roll < 0.3 ? 'pine' : roll < 0.72 ? 'oak' : 'small', variant * 9 + u);
    }
  }, true);
}

// Sprinkled on empty land so the countryside isn't bare.
function decoObject(type, variant) {
  return sprite(`deco:${type}:${variant}`, 80, 96, 40, 70, (g) => {
    const rand = seeded(variant * 37 + 3);
    if (type === 'pines') {
      const [x1, y1] = P(9 + rand() * 4, 12 + rand() * 6);
      drawTree(g, x1, y1, 'pine', variant);
      const [x2, y2] = P(20 + rand() * 4, 18 + rand() * 6);
      drawTree(g, x2, y2, 'pine', variant + 4);
      return;
    }
    const [x, y] = P(12 + rand() * 8, 12 + rand() * 8);
    drawTree(g, x, y, type, variant);
  }, true);
}
// @@ART2

// ---------------------------------------------------------------------------------------------
// Roads, parks, empty lots and street furniture

// Objects (buildings, trees, signs) share one canvas size. The tile's top vertex sits at (56, 130).
const OW = 112;
const OH = 176;
const OAX = 56;
const OAY = 130;
const object = (key, draw) => sprite(key, OW, OH, OAX, OAY, draw, true);

// A circle lying on the ground, seen from the iso angle.
function disc(g, fill, u, v, r, z = 0) {
  const [x, y] = P(u, v, z);
  ell(g, fill, x, y, r * 1.414, r * 0.707);
}

// A soft shadow cast to the right and down, for a thing on the ground.
function castShadow(g, pts, alpha = 0.3) {
  // pts: footprint corners in u,v plus a shadow-throw (du, dv) to stretch along.
  fp(g, `rgba(34,26,58,${alpha})`, pts);
}

const PAVE = '#bcb6a6';
const KERB = '#ddd8ca';
const ASPH = '#4a4d57';
const LANE = '#f0cc58';
const ZEBRA = '#eef0f3';

// Bits: 1 north-east neighbour, 2 south-east, 4 south-west, 8 north-west.
function roadSprite(mask, variant) {
  return tile(`road:${mask}:${variant}`, (g) => {
    const rand = seeded(mask * 7 + variant * 13 + 4);
    // Pavement with expansion joints.
    fp3(g, grad(g, -32, 0, 32, 32, lite(PAVE, 0.08), dark(PAVE, 0.07)), [-2, -2], [34, -2], [34, 34], [-2, 34]);
    g.lineWidth = 0.7;
    g.strokeStyle = 'rgba(70,62,52,.3)';
    for (const k of [8, 16, 24]) {
      const [x0, y0] = P(k, 0);
      const [x1, y1] = P(k, 32);
      g.beginPath();
      g.moveTo(x0, y0);
      g.lineTo(x1, y1);
      g.stroke();
      const [a0, b0] = P(0, k);
      const [a1, b1] = P(32, k);
      g.beginPath();
      g.moveTo(a0, b0);
      g.lineTo(a1, b1);
      g.stroke();
    }
    for (let i = 0; i < 10; i++) {
      const [x, y] = P(rand() * 32, rand() * 32);
      ell(g, rand() < 0.5 ? 'rgba(255,255,240,.12)' : 'rgba(60,50,40,.1)', x, y, 2 + rand() * 3, 1 + rand() * 1.5);
    }
    const arms = (c, lo, hi) => {
      fp3(g, c, [lo, lo], [hi, lo], [hi, hi], [lo, hi]);
      if (mask & 1) fp3(g, c, [lo, -2], [hi, -2], [hi, lo], [lo, lo]);
      if (mask & 2) fp3(g, c, [hi, lo], [34, lo], [34, hi], [hi, hi]);
      if (mask & 4) fp3(g, c, [lo, hi], [hi, hi], [hi, 34], [lo, 34]);
      if (mask & 8) fp3(g, c, [-2, lo], [lo, lo], [lo, hi], [-2, hi]);
    };
    arms(KERB, 7, 25);
    arms('rgba(30,26,40,.55)', 8, 24);
    arms(grad(g, -32, 0, 32, 32, '#555862', ASPH), 8.8, 23.2);
    // Wear: patches, cracks and a bit of grain.
    for (let i = 0; i < 8; i++) {
      const [x, y] = P(10 + rand() * 12, 10 + rand() * 12);
      ell(g, rand() < 0.5 ? 'rgba(20,20,30,.14)' : 'rgba(255,255,255,.05)', x, y, 3 + rand() * 4, 1.5 + rand() * 2);
    }
    g.lineWidth = 0.6;
    g.strokeStyle = 'rgba(20,20,30,.4)';
    for (let i = 0; i < 2; i++) {
      const u = 10 + rand() * 12;
      const v = 10 + rand() * 12;
      const [x, y] = P(u, v);
      g.beginPath();
      g.moveTo(x, y);
      g.lineTo(x + 2 + rand() * 3, y + 0.8 + rand());
      g.lineTo(x + 5 + rand() * 4, y + rand() * 2 - 1);
      g.stroke();
    }
    const count = (mask & 1) + ((mask >> 1) & 1) + ((mask >> 2) & 1) + ((mask >> 3) & 1);
    const dash = (alongU, a, b) => {
      if (alongU) fp3(g, LANE, [a, 15.5], [b, 15.5], [b, 16.5], [a, 16.5]);
      else fp3(g, LANE, [15.5, a], [16.5, a], [16.5, b], [15.5, b]);
    };
    if (count >= 3) {
      // Crossings on every arm of a junction.
      const stripes = (alongU, a) => {
        for (let k = 0; k < 6; k++) {
          const c = 9.6 + k * 2.3;
          if (alongU) fp3(g, ZEBRA, [a, c], [a + 3.6, c], [a + 3.6, c + 1.3], [a, c + 1.3]);
          else fp3(g, ZEBRA, [c, a], [c + 1.3, a], [c + 1.3, a + 3.6], [c, a + 3.6]);
        }
      };
      if (mask & 2) stripes(true, 25.5);
      if (mask & 8) stripes(true, 2.9);
      if (mask & 4) stripes(false, 25.5);
      if (mask & 1) stripes(false, 2.9);
    } else if (count >= 1) {
      const eastWest = (mask & 10) === 10 && !(mask & 5);
      const northSouth = (mask & 5) === 5 && !(mask & 10);
      if (eastWest) for (const a of [2, 10, 18, 26]) dash(true, a, a + 4);
      else if (northSouth) for (const a of [2, 10, 18, 26]) dash(false, a, a + 4);
      else {
        if (mask & 2) dash(true, 26, 30);
        if (mask & 8) dash(true, 2, 6);
        if (mask & 4) dash(false, 26, 30);
        if (mask & 1) dash(false, 2, 6);
      }
    }
    grain(g, 0.07);
    if (variant === 1 && count >= 2) {
      disc(g, 'rgba(15,15,22,.6)', 16, 12.5, 3.2);
      disc(g, '#6a6d77', 16, 12.5, 2.7);
      disc(g, '#7d808a', 16, 12.2, 2);
    } else if (variant === 2 && count >= 2) {
      fp3(g, '#2c2e36', [20, 22], [24, 22], [24, 24], [20, 24]);
      for (let k = 0; k < 4; k++) fp3(g, '#5d606a', [20.4 + k, 22.2], [20.9 + k, 22.2], [20.9 + k, 23.8], [20.4 + k, 23.8]);
    }
  });
}

// --- Street furniture: placed by hash on the pavement of straight roads ------------------------

function lampPost(g, x, y, h = 40) {
  ell(g, 'rgba(25,25,35,.3)', x + 4, y + 0.6, 6, 2);
  rr(g, '#3b3f4f', x - 2.5, y - 3, 5, 3);
  rr(g, grad(g, x - 1, 0, x + 1.2, 0, '#6a6f82', '#2f3342'), x - 1, y - h, 2.2, h - 2);
  g.strokeStyle = '#3b3f4f';
  g.lineWidth = 1.6;
  g.beginPath();
  g.moveTo(x, y - h);
  g.quadraticCurveTo(x, y - h - 6, x + 6, y - h - 5);
  g.stroke();
  fp(g, '#2b2f3e', [[x + 3, y - h - 6], [x + 11, y - h - 6], [x + 10, y - h - 3], [x + 4, y - h - 3]]);
  rr(g, '#fff0b8', x + 4.5, y - h - 3, 5, 1.6);
}

function streetObject(type, u, v, variant) {
  const key = `street:${type}:${u}:${v}:${variant}`;
  const found = sprites.get(`${LOD}|${key}`);
  if (found) return found;
  const s = object(key, (g) => {
    const [x, y] = P(u, v);
    if (type === 'lamp') lampPost(g, x, y, 42);
    else if (type === 'tree') {
      disc(g, '#7b6a52', u, v, 3.4);
      disc(g, '#4f3a26', u, v, 2.4);
      drawTree(g, x, y, variant === 0 ? 'blossom' : 'small', variant * 3 + 1);
    } else if (type === 'bench') {
      ell(g, 'rgba(25,25,35,.28)', x + 2, y + 1, 9, 2.6);
      onL(g, u - 6, v, () => {
        for (const px of [1, 10]) rr(g, '#3a3d4a', px, -6, 1.6, 6);
        rr(g, '#9a6a3c', 0, -7, 12, 2);
        rr(g, '#b98350', 0, -7, 12, 0.8);
        rr(g, '#7d522c', 0, -12, 12, 1.6);
        rr(g, '#8a5d33', 0, -10, 12, 1.6);
      });
    } else if (type === 'hydrant') {
      ell(g, 'rgba(25,25,35,.28)', x + 1.5, y + 0.6, 4, 1.4);
      rr(g, '#c3392f', x - 2, y - 7, 4, 7);
      ell(g, '#d9483c', x, y - 7, 2.4, 1.3);
      rr(g, '#e5675b', x - 2, y - 7, 1, 7);
      rr(g, '#a52a22', x - 3.4, y - 5, 6.8, 1.6);
    } else if (type === 'bin') {
      ell(g, 'rgba(25,25,35,.28)', x + 1.5, y + 0.6, 5, 1.6);
      rr(g, grad(g, x - 3, 0, x + 3, 0, '#5e8a66', '#355b43'), x - 3, y - 9, 6, 9);
      ell(g, '#6f9c78', x, y - 9, 3, 1.3);
      rr(g, '#2d4a37', x - 3, y - 6, 6, 0.8);
    } else if (type === 'post') {
      ell(g, 'rgba(25,25,35,.28)', x + 1.5, y + 0.6, 5, 1.6);
      rr(g, '#2c5faa', x - 2.4, y - 11, 4.8, 11);
      ell(g, '#3a73c4', x, y - 11, 2.4, 1.1);
      rr(g, '#e8d9a0', x - 1.4, y - 8, 2.8, 0.9);
    }
  });
  if (type === 'lamp') {
    s.lit = () => object(`${key}:lit`, (g) => {
      const [x, y] = P(u, v);
      const gr = g.createRadialGradient(x + 7, y - 45, 1, x + 7, y - 45, 26);
      gr.addColorStop(0, 'rgba(255,225,150,.75)');
      gr.addColorStop(1, 'rgba(255,200,110,0)');
      g.fillStyle = gr;
      g.fillRect(x - 24, y - 75, 62, 62);
      disc(g, 'rgba(255,205,120,.2)', u + 3, v, 12);
      disc(g, 'rgba(255,215,140,.2)', u + 3, v, 7);
      rr(g, '#fff6c8', x + 4.5, y - 46, 5, 1.8);
    });
  }
  return s;
}

// --- Parks --------------------------------------------------------------------------------------

const PARK_GRASS = ['#74b042', '#7db84a', '#86c053'];

function parkTop(mask, tone, variant) {
  return tile(`parktop:${mask}:${tone}:${variant}`, (g) => {
    const base = PARK_GRASS[tone];
    const rand = seeded(mask * 5 + tone * 17 + variant * 3 + 8);
    g.fillStyle = grad(g, -32, 0, 32, 32, lite(base, 0.04), dark(base, 0.04));
    g.fillRect(-40, -6, 80, 46);
    for (let i = 0; i < 26; i++) {
      const r = 2 + rand() * 6;
      ell(g, rand() < 0.55 ? rgba(lite(base, 0.34), 0.2) : rgba(dark(base, 0.34), 0.2), (rand() - 0.5) * 68, rand() * 34, r * 1.8, r * 0.9);
    }
    // Mowing stripes.
    for (let k = 0; k < 4; k++) fp3(g, 'rgba(255,255,220,.06)', [k * 8, 0], [k * 8 + 4, 0], [k * 8 + 4, 32], [k * 8, 32]);
    g.lineWidth = 0.8;
    for (let i = 0; i < 30; i++) {
      const x = (rand() - 0.5) * 64;
      const y = 3 + rand() * 28;
      g.strokeStyle = rand() < 0.5 ? rgba(dark(base, 0.4), 0.5) : rgba(lite(base, 0.45), 0.5);
      g.beginPath();
      g.moveTo(x, y);
      g.lineTo(x + (rand() - 0.5) * 1.6, y - 1.5 - rand() * 1.4);
      g.stroke();
    }
    grain(g, 0.05);
    // Gravel paths, edged, running to any neighbouring park or road.
    const paths = (c, lo, hi, plaza) => {
      if (mask & 1) fp3(g, c, [lo, -2], [hi, -2], [hi, 16], [lo, 16]);
      if (mask & 2) fp3(g, c, [16, lo], [34, lo], [34, hi], [16, hi]);
      if (mask & 4) fp3(g, c, [lo, 16], [hi, 16], [hi, 34], [lo, 34]);
      if (mask & 8) fp3(g, c, [-2, lo], [16, lo], [16, hi], [-2, hi]);
      disc(g, c, 16, 16, plaza);
    };
    paths('#b3a174', 12.2, 19.8, 8.4);
    paths('#e0d2a8', 13, 19, 7.6);
    for (let i = 0; i < 26; i++) {
      const a = rand() * 6.2832;
      const d = rand() * 6.5;
      const [x, y] = P(16 + Math.cos(a) * d, 16 + Math.sin(a) * d);
      rr(g, rand() < 0.5 ? 'rgba(130,112,76,.5)' : 'rgba(255,250,230,.5)', x, y, 1, 0.8);
    }
    // Flower beds in the corners.
    const bloom = [['#ff8fb8', '#ffd75e'], ['#c9a9ff', '#fff3a8'], ['#ff9f7a', '#ffe6f0']][(variant + tone) % 3];
    for (const [cu, cv] of [[6, 6], [26, 6], [6, 26], [26, 26]]) {
      if ((cu === 6 && cv === 6 && (mask & 9)) || (cu === 26 && cv === 6 && (mask & 3)) || (cu === 6 && cv === 26 && (mask & 12)) || (cu === 26 && cv === 26 && (mask & 6))) {
        // paths nearby: skip the bed
        if (rand() < 0.5) continue;
      }
      disc(g, '#6b5133', cu, cv, 3.8);
      disc(g, '#4f3a26', cu, cv, 3.3);
      for (let i = 0; i < 9; i++) {
        const a = rand() * 6.2832;
        const d = rand() * 3;
        const [x, y] = P(cu + Math.cos(a) * d, cv + Math.sin(a) * d);
        ell(g, bloom[i % 2], x, y - 1.4, 1.1, 0.9);
        line(g, '#3d6f2a', 0.6, x, y, x, y - 1);
      }
    }
  });
}

function bench(g, u, v, along = 'u') {
  const [x, y] = P(u, v);
  ell(g, 'rgba(25,25,35,.26)', x + 2, y + 1, 9, 2.6);
  if (along === 'u') {
    onL(g, u - 6, v, () => {
      for (const px of [1, 10]) rr(g, '#3a3d4a', px, -6, 1.6, 6);
      rr(g, '#a06f3e', 0, -7, 12, 2);
      rr(g, '#c08a52', 0, -7, 12, 0.8);
      rr(g, '#7d522c', 0, -12, 12, 1.6);
      rr(g, '#8a5d33', 0, -10, 12, 1.6);
    });
  } else {
    onR(g, u, v + 6, () => {
      for (const px of [1, 10]) rr(g, '#3a3d4a', px, -6, 1.6, 6);
      rr(g, '#8a5a30', 0, -7, 12, 2);
      rr(g, '#a97440', 0, -7, 12, 0.8);
      rr(g, '#70482a', 0, -12, 12, 1.6);
    });
  }
}

function duck(g, x, y) {
  ell(g, '#f4f1e8', x, y, 2.6, 1.5);
  ell(g, '#2f7d3a', x + 2, y - 1.6, 1.3, 1.2);
  rr(g, '#f0a030', x + 3.1, y - 1.8, 1.4, 0.7);
  dot(g, '#111', x + 2.3, y - 1.9);
}
const dot = (g, c, x, y) => rr(g, c, x - 0.4, y - 0.4, 0.8, 0.8);

// A stone fountain in the middle of a tile.
function fountain(g) {
  disc(g, 'rgba(25,25,35,.25)', 18, 18, 8.4);
  disc(g, '#9c9a8f', 16, 16, 7.2, 0);
  disc(g, '#c9c6b8', 16, 16, 7.2, 3);
  disc(g, '#7e7d72', 16, 16, 6, 3);
  disc(g, '#4fa6d8', 16, 16, 5.6, 2.2);
  disc(g, '#7bc4ec', 15.4, 15.4, 3.6, 2.3);
  const [x, y] = P(16, 16, 3);
  rr(g, '#d3d0c2', x - 1.6, y - 11, 3.2, 11);
  ell(g, '#c9c6b8', x, y - 11, 4, 1.7);
  ell(g, '#8ccaf0', x, y - 11.4, 3.2, 1.2);
  for (let k = 0; k < 7; k++) {
    const a = -1.4 + k * 0.47;
    g.strokeStyle = 'rgba(210,240,255,.85)';
    g.lineWidth = 0.9;
    g.beginPath();
    g.moveTo(x, y - 12);
    g.quadraticCurveTo(x + Math.sin(a) * 7, y - 20, x + Math.sin(a) * 11, y - 4);
    g.stroke();
  }
}

function parkObject(variant, mask) {
  return object(`park:${variant}:${mask}`, (g) => {
    const tree = (u, v, type, s) => {
      const [x, y] = P(u, v);
      drawTree(g, x, y, type, s + variant);
    };
    const free = (b) => !((mask >> b) & 1);
    if (variant === 0) {
      // A fountain in the middle of the plaza.
      fountain(g);
      if (free(1)) tree(27, 5, 'small', 3);
      if (free(2)) tree(5, 27, 'blossom', 5);
      if (free(3)) tree(27, 27, 'bush', 7);
      if (free(0)) tree(5, 5, 'bush', 9);
    } else if (variant === 1) {
      // A pond with reeds, lily pads and a duck, and a willow-ish tree.
      disc(g, '#9aa66a', 17, 16, 10.5);
      disc(g, '#5d8b9a', 17, 16, 9.6);
      disc(g, '#4aa0d0', 17, 16, 9);
      disc(g, '#6cc0e8', 15.5, 14.5, 6);
      for (const [u, v] of [[21, 18], [13, 19], [19, 13]]) {
        disc(g, '#3f8a3c', u, v, 1.7);
        const [x, y] = P(u, v);
        dot(g, '#ff9fc4', x, y);
      }
      duck(g, ...P(14, 15));
      for (const [u, v] of [[8, 22], [9, 23.5], [25, 9]]) {
        const [x, y] = P(u, v);
        for (let k = 0; k < 4; k++) line(g, k % 2 ? '#6b8f3a' : '#4d7a2a', 0.9, x + k - 1.5, y, x + k * 0.8 - 1.2, y - 7 - k);
        rr(g, '#7a5530', x - 0.4, y - 9, 1, 3);
      }
      tree(5, 5, 'oak', 11);
      bench(g, 17, 28, 'u');
      if (free(3)) tree(28, 27, 'bush', 2);
    } else if (variant === 2) {
      // A playground: sandpit, slide, swings.
      fp3(g, '#7a5a38', [4, 6], [16, 6], [16, 18], [4, 18]);
      fp3(g, '#e8d29a', [5, 7], [15, 7], [15, 17], [5, 17]);
      for (let i = 0; i < 12; i++) {
        const [x, y] = P(5.5 + (i * 7) % 9, 7.5 + (i * 5) % 9);
        dot(g, i % 2 ? '#b89a60' : '#fff2c8', x, y);
      }
      const [bx, by] = P(9, 12);
      ell(g, '#7aa8ea', bx, by - 1, 2.6, 1.2);
      rr(g, '#e05a4a', bx - 2, by - 3, 4, 2);
      // Slide.
      onL(g, 18, 20, () => {
        rr(g, '#e0b13c', 0, -14, 2, 14);
        rr(g, '#e0b13c', 8, -14, 2, 14);
        rr(g, '#c24b3f', 0, -16, 10, 2);
      });
      fp3(g, '#f0675a', [20, 21, 14], [22, 21, 14], [27, 22, 2], [25, 22, 2]);
      fp3(g, '#c7473b', [22, 21, 14], [22.6, 21, 14], [27.6, 22, 2], [27, 22, 2]);
      // Swings.
      onR(g, 28, 14, () => {
        rr(g, '#3b6fb5', 0, -20, 2, 20);
        rr(g, '#3b6fb5', 14, -20, 2, 20);
        rr(g, '#3b6fb5', -1, -21, 18, 2);
        line(g, '#6a6f82', 0.7, 5, -19, 5, -7);
        line(g, '#6a6f82', 0.7, 11, -19, 11, -7);
        rr(g, '#e05a4a', 3.5, -7, 3, 1.2);
        rr(g, '#e8b64a', 9.5, -7, 3, 1.2);
      });
      tree(27, 5, 'oak', 17);
      tree(5, 27, 'small', 19);
    } else {
      // A gazebo among flowering trees.
      const base = [10, 10, 22, 22];
      castShadow(g, [P(10, 22), P(22, 22), P(30, 24), P(31, 30), P(12, 30)].map((p) => [p[0], p[1]]), 0.18);
      fp3(g, '#d7ccb0', [9, 9, 2], [23, 9, 2], [23, 23, 2], [9, 23, 2]);
      fp3(g, '#9b8f74', [9, 23, 0], [23, 23, 0], [23, 23, 2], [9, 23, 2]);
      fp3(g, '#7c715b', [23, 23, 0], [23, 9, 0], [23, 9, 2], [23, 23, 2]);
      for (const [u, v] of [[10, 22], [22, 22], [22, 10], [10, 10]]) {
        const [x, y] = P(u, v, 2);
        rr(g, grad(g, x - 1.4, 0, x + 1.4, 0, '#f4efe2', '#b8b09c'), x - 1.4, y - 17, 2.8, 17);
      }
      const apex = [16, 16, 30];
      fp3(g, '#3a8f84', [8, 24, 19], [24, 24, 19], apex);
      fp3(g, '#2c6f66', [24, 24, 19], [24, 8, 19], apex);
      fp3(g, '#5cb5a8', [8, 24, 19], [10, 24, 19], apex);
      line(g, '#e8dcc0', 1, ...P(8, 24, 19), ...P(16, 16, 30));
      rr(g, '#e8dcc0', ...P(16, 16, 30).map((p, i) => (i ? p - 4 : p - 0.6)), 1.2, 4);
      bench(g, 16, 21, 'u');
      tree(4, 5, 'blossom', 23);
      tree(28, 27, 'blossom', 29);
      for (const [u, v] of [[27, 12], [5, 20], [28, 20]]) {
        const [x, y] = P(u, v);
        ell(g, '#3a8f4d', x, y - 1, 3.4, 1.8);
        dot(g, '#ff7fa6', x - 1, y - 2);
        dot(g, '#ffd75e', x + 1, y - 1.6);
        dot(g, '#ffffff', x + 2, y - 2.4);
      }
    }
  });
}

// --- Empty lots and cleared land ---------------------------------------------------------------

const ZONE_SIGN = { [K_HOMES]: '#4ea24a', [K_SHOPS]: '#2f86d4', [K_WORKS]: '#e08a2b' };
const ZONE_PAD = { [K_HOMES]: '#9bb36a', [K_SHOPS]: '#9cb0b4', [K_WORKS]: '#b9a46c' };

// A levelled plot of bare earth on the grass, ready for building.
function lotTop(kind, tone) {
  return tile(`lot:${kind}:${tone}`, (g) => {
    const base = GRASS[tone];
    const rand = seeded(kind * 11 + tone);
    g.fillStyle = grad(g, -32, 0, 32, 32, lite(base, 0.03), dark(base, 0.035));
    g.fillRect(-40, -6, 80, 46);
    for (let i = 0; i < 20; i++) {
      const r = 2 + rand() * 5;
      ell(g, rand() < 0.5 ? rgba(lite(base, 0.3), 0.2) : rgba(dark(base, 0.35), 0.2), (rand() - 0.5) * 68, rand() * 34, r * 1.8, r * 0.9);
    }
    fp3(g, '#8f7a52', [3, 3], [29, 3], [29, 29], [3, 29]);
    fp3(g, grad(g, -30, 0, 30, 32, lite(ZONE_PAD[kind], 0.1), dark(ZONE_PAD[kind], 0.15)), [4, 4], [28, 4], [28, 28], [4, 28]);
    for (let i = 0; i < 40; i++) {
      const [x, y] = P(5 + rand() * 22, 5 + rand() * 22);
      rr(g, rand() < 0.5 ? 'rgba(90,70,40,.45)' : 'rgba(255,245,210,.35)', x, y, 1.2, 0.8);
    }
    for (let i = 0; i < 4; i++) {
      const [x, y] = P(6 + rand() * 20, 6 + rand() * 20);
      ell(g, 'rgba(70,55,35,.2)', x, y, 3 + rand() * 3, 1.3 + rand());
    }
    grain(g, 0.07);
  });
}

function zoneIcon(g, kind) {
  // A little picture on the sign, drawn in the sign's own flat plane, 20 by 14.
  if (kind === K_HOMES) {
    fp(g, '#c2503c', [[10, -13], [18, -7], [2, -7]]);
    rr(g, '#f7f0dc', 4, -7, 12, 6);
    rr(g, '#7a5530', 8.6, -5.6, 2.8, 4.6);
    rr(g, '#8fcbec', 5, -5.5, 2.4, 2.4);
    rr(g, '#8fcbec', 12.6, -5.5, 2.4, 2.4);
  } else if (kind === K_SHOPS) {
    for (let i = 0; i < 6; i++) rr(g, i % 2 ? '#fff7ea' : '#e5484d', 2 + i * 2.7, -12, 2.7, 4);
    rr(g, '#f7f0dc', 3, -8, 14, 7);
    rr(g, '#8fcbec', 4.5, -7, 5, 4);
    rr(g, '#7a5530', 12, -7.5, 3.4, 6.5);
  } else {
    g.fillStyle = '#4a5166';
    g.beginPath();
    g.arc(10, -7, 5.4, 0, 6.2832);
    g.fill();
    for (let i = 0; i < 8; i++) {
      const a = (i / 8) * 6.2832;
      rr(g, '#4a5166', 10 + Math.cos(a) * 6.2 - 1.2, -7 + Math.sin(a) * 6.2 - 1.2, 2.4, 2.4);
    }
    g.fillStyle = '#e9edf5';
    g.beginPath();
    g.arc(10, -7, 2.2, 0, 6.2832);
    g.fill();
  }
}

function lotObject(kind) {
  return object(`lotsign:${kind}`, (g) => {
    const col = ZONE_SIGN[kind];
    // Surveyor's stakes, with string between them.
    const tops = [];
    for (const [u, v] of [[5, 5], [27, 5], [27, 27], [5, 27]]) {
      const [x, y] = P(u, v);
      ell(g, 'rgba(25,25,35,.25)', x + 1.2, y + 0.4, 2.2, 0.9);
      rr(g, '#a07a4a', x - 0.8, y - 9, 1.6, 9);
      rr(g, col, x - 0.8, y - 9, 1.6, 2.4);
      tops.push([x, y - 8.5]);
    }
    g.strokeStyle = 'rgba(255,255,255,.75)';
    g.lineWidth = 0.7;
    g.beginPath();
    g.moveTo(...tops[0]);
    for (const t of tops.slice(1)) g.lineTo(...t);
    g.closePath();
    g.stroke();
    // A pile of bricks and a cone.
    const [px, py] = P(22, 22);
    ell(g, 'rgba(25,25,35,.25)', px + 2, py + 1, 8, 2.4);
    onL(g, 18, 25, () => {
      for (let r = 0; r < 3; r++) for (let c = 0; c < 4 - r; c++) rr(g, (r + c) % 2 ? '#b25a40' : '#c46a4c', c * 3 + r * 1.5, -(r + 1) * 2.2, 2.8, 2);
    });
    const [cx, cy] = P(10, 24);
    fp(g, '#f07a2a', [[cx - 2, cy], [cx + 2, cy], [cx + 0.7, cy - 6], [cx - 0.7, cy - 6]]);
    rr(g, '#ffffff', cx - 1.4, cy - 3.4, 2.8, 1.1);
    rr(g, '#3b3f4f', cx - 2.6, cy - 0.5, 5.2, 0.9);
    // The sign board on two posts, facing us.
    onL(g, 7, 29, () => {
      rr(g, '#6a4a2a', 3, -17, 2, 17);
      rr(g, '#6a4a2a', 15, -17, 2, 17);
      ell(g, 'rgba(25,25,35,.2)', 10, 1, 9, 1.4);
      rr(g, '#2a3045', 0, -26, 20, 14);
      rr(g, '#fbf6e8', 1, -25, 18, 12);
      rr(g, col, 1, -25, 18, 3);
      g.save();
      g.translate(0, -1);
      zoneIcon(g, kind);
      g.restore();
    });
  });
}

function clearedTop(tone) {
  return tile(`cleared:${tone}`, (g) => {
    const base = '#7d9a4e';
    const rand = seeded(tone * 5 + 21);
    g.fillStyle = grad(g, -32, 0, 32, 32, lite(base, 0.06), dark(base, 0.1));
    g.fillRect(-40, -6, 80, 46);
    for (let i = 0; i < 9; i++) ell(g, i % 2 ? '#8e7248' : '#a18455', (rand() - 0.5) * 50, 5 + rand() * 24, 3 + rand() * 6, 1.4 + rand() * 2.4);
    for (let i = 0; i < 34; i++) rr(g, rand() < 0.5 ? 'rgba(235,205,140,.6)' : 'rgba(90,70,40,.5)', (rand() - 0.5) * 56, 3 + rand() * 28, 1.3, 0.8);
    grain(g, 0.07);
  });
}

function stumpObject(variant) {
  return object(`stump:${variant}`, (g) => {
    const stump = (u, v, r) => {
      const [x, y] = P(u, v);
      ell(g, 'rgba(25,25,35,.28)', x + 2, y + 0.8, r * 1.9, r * 0.8);
      fp(g, grad(g, x - r, 0, x + r, 0, '#8a6238', '#52391f'), [[x - r - 1, y], [x - r, y - 6], [x + r, y - 6], [x + r + 1, y]]);
      ell(g, '#d2ad74', x, y - 6, r, r * 0.5);
      ell(g, '#b78d54', x, y - 6, r * 0.62, r * 0.3);
      ell(g, '#8a6238', x, y - 6, r * 0.22, r * 0.12);
    };
    stump(10 + (variant % 3) * 4, 12 + (variant % 2) * 6, 4.4);
    if (variant % 2) stump(22, 20, 3.2);
    const [x, y] = P(20, 11);
    line(g, '#8a6238', 2, x - 5, y, x + 4, y - 2);
    ell(g, '#d2ad74', x + 4, y - 2, 1.2, 1.6);
  });
}
// @@ART3

// ---------------------------------------------------------------------------------------------
// Buildings. Each is built from boxes and roof planes in u, v, z, with detail painted flat onto
// the walls. Light comes from the upper left, so left walls are bright and right walls are in
// shade. Every look is painted once, plus a twin showing only the lit windows for the night.

const SIDING = ['#ecdcb4', '#dcc08e', '#aec6d8', '#bcd3ac', '#e6bcae', '#f1e6aa'];
const BRICKS = ['#a9573f', '#9c4b38', '#b86b4b', '#8e5b49'];
const ROOFS = ['#a5402f', '#506b97', '#4f7c67', '#7c5b3f', '#6c5c74', '#8b4c3b'];
const TRIM = '#f7f1e3';
const DOORC = ['#7b4a2a', '#2f5d9a', '#a83a30', '#2f7a58', '#6b3f86', '#c48a22'];
const AWN = [['#d9433f', '#fff4e4'], ['#2f78d0', '#fff4e4'], ['#2a9560', '#fff4e4'], ['#e59a22', '#fff4e4'], ['#8a52d0', '#fff4e4'], ['#1fa8b8', '#fff4e4']];
const GLASS_TOP = '#bfdff2';
const GLASS_BOT = '#5c8fb6';

// Paints a wall into the current face plane: x from 0 to len, and the wall stands at -z.
function paintWall(g, len, z0, z1, mat, seed) {
  const rand = seeded(seed);
  const h = z1 - z0;
  const top = -z1;
  const base = mat.c;
  g.save();
  g.beginPath();
  g.rect(0, top, len, h);
  g.clip();
  if (mat.k === 'brick') {
    g.fillStyle = '#cdbfa6';
    g.fillRect(0, top, len, h);
    const bh = 2.7;
    const bw = 5.6;
    let row = 0;
    for (let y = -z0 - bh; y > top - bh; y -= bh, row++) {
      for (let x = -(row % 2) * bw * 0.5; x < len; x += bw) {
        g.fillStyle = mixHex(base, rand() < 0.5 ? dark(base, 0.35) : lite(base, 0.3), rand() * 0.5);
        g.fillRect(x + 0.35, y + 0.35, bw - 0.7, bh - 0.7);
      }
    }
    g.fillStyle = grad(g, 0, top, 0, -z0, 'rgba(255,240,210,.14)', 'rgba(0,0,0,0)', 'rgba(20,10,30,.14)');
    g.fillRect(0, top, len, h);
  } else {
    g.fillStyle = grad(g, 0, top, 0, -z0, lite(base, 0.1), base, dark(base, 0.1));
    g.fillRect(0, top, len, h);
    if (mat.k === 'siding') {
      for (let y = -z0 - 3; y > top; y -= 3) {
        line(g, rgba(dark(base, 0.55), 0.4), 0.8, 0, y, len, y);
        line(g, 'rgba(255,255,255,.24)', 0.6, 0, y + 0.8, len, y + 0.8);
      }
    } else if (mat.k === 'steel') {
      for (let x = 0; x < len; x += 2.4) {
        line(g, rgba(dark(base, 0.5), 0.45), 0.9, x, top, x, -z0);
        line(g, 'rgba(255,255,255,.3)', 0.7, x + 1, top, x + 1, -z0);
      }
    } else if (mat.k === 'concrete') {
      for (let x = 8; x < len; x += 8) line(g, 'rgba(30,30,40,.22)', 0.8, x, top, x, -z0);
      for (let y = -z0 - 8; y > top; y -= 8) line(g, 'rgba(30,30,40,.16)', 0.8, 0, y, len, y);
      for (let i = 0; i < 6; i++) ell(g, 'rgba(30,30,40,.07)', rand() * len, top + rand() * h, 3 + rand() * 4, 6 + rand() * 6);
    } else {
      for (let i = 0; i < len * h * 0.05; i++) rr(g, rand() < 0.5 ? 'rgba(255,255,240,.22)' : 'rgba(60,40,30,.16)', rand() * len, top + rand() * h, 1, 1);
    }
  }
  // Dirt gathers at the foot of a wall.
  g.fillStyle = grad(g, 0, -z0 - 5, 0, -z0, 'rgba(25,15,35,0)', 'rgba(25,15,35,.28)');
  g.fillRect(0, -z0 - 5, len, 5);
  g.restore();
}

function shadeFace(g, len, z0, z1) {
  g.fillStyle = 'rgba(36,26,70,.34)';
  g.fillRect(-0.5, -z1 - 0.5, len + 1, z1 - z0 + 1);
}

// A wall box. `detail(side, len)` paints windows, doors and so on into each face's own plane.
function box(S, R, z0, z1, mat, detail) {
  const [u0, v0, u1, v1] = R;
  const g = S.g;
  const seed = S.seed + (u0 * 7 + v0 * 13) | 0;
  if (S.day) {
    onL(g, u0, v1, () => {
      paintWall(g, u1 - u0, z0, z1, mat, seed);
      if (detail) detail('L', u1 - u0);
    });
    onR(g, u1, v1, () => {
      paintWall(g, v1 - v0, z0, z1, mat, seed + 5);
      if (detail) detail('R', v1 - v0);
      shadeFace(g, v1 - v0, z0, z1);
    });
    // The front corner catches the light.
    const [x0, y0] = P(u1, v1, z0);
    const [x1, y1] = P(u1, v1, z1);
    line(g, 'rgba(255,248,225,.5)', 0.8, x0, y0, x1, y1);
  } else if (detail) {
    onL(g, u0, v1, () => detail('L', u1 - u0));
    onR(g, u1, v1, () => detail('R', v1 - v0));
  }
}

// A flat top for a box (a roof you can see, or the top of a counter).
function topFace(S, R, z, fill, edge) {
  if (!S.day) return;
  const [u0, v0, u1, v1] = R;
  fp3(S.g, fill, [u0, v0, z], [u1, v0, z], [u1, v1, z], [u0, v1, z]);
  if (edge) {
    S.g.strokeStyle = edge;
    S.g.lineWidth = 0.7;
    S.g.stroke();
  }
}

// The shadow a thing throws on the ground, towards the right and down.
function shadow(S, R, H) {
  if (!S.info) return;
  S.info.shadows.push([R, H]);
}

// --- Windows, doors and fittings, painted into the current face plane ---------------------------

function win(S, x, z, w, h, o = {}) {
  const g = S.g;
  if (!S.day) {
    if (ih(S.seed, S.n++, 5) % 100 < (o.lit ?? 64)) {
      const t = -(z + h);
      rr(g, grad(g, 0, t, 0, t + h, '#fff0b8', '#ffc260'), x, t, w, h);
      g.strokeStyle = 'rgba(120,70,20,.55)';
      g.lineWidth = 0.6;
      g.beginPath();
      g.moveTo(x + w / 2, t);
      g.lineTo(x + w / 2, t + h);
      if (h > 7) {
        g.moveTo(x, t + h / 2);
        g.lineTo(x + w, t + h / 2);
      }
      g.stroke();
      if (o.curtain) rr(g, 'rgba(255,255,255,.35)', x, t, w * 0.32, h * 0.7);
    }
    return;
  }
  const t = -(z + h);
  const frame = o.frame || TRIM;
  rr(g, 'rgba(25,15,35,.3)', x - 1.2, t - 1, w + 2.4, h + 2.4);
  rr(g, frame, x - 1, t - 1, w + 2, h + 2);
  rr(g, grad(g, 0, t, 0, t + h, GLASS_TOP, '#8ab8d6', GLASS_BOT), x, t, w, h);
  // Curtains and a glimpse of room.
  if (o.curtain) {
    fp(g, rgba(o.curtain, 0.9), [[x, t], [x + w * 0.38, t], [x + w * 0.1, t + h * 0.62], [x, t + h * 0.62]]);
    fp(g, rgba(o.curtain, 0.9), [[x + w, t], [x + w * 0.62, t], [x + w * 0.9, t + h * 0.62], [x + w, t + h * 0.62]]);
  }
  fp(g, 'rgba(255,255,255,.3)', [[x + w * 0.1, t], [x + w * 0.5, t], [x + w * 0.1, t + h * 0.7]]);
  g.strokeStyle = frame;
  g.lineWidth = 0.9;
  g.beginPath();
  g.moveTo(x + w / 2, t);
  g.lineTo(x + w / 2, t + h);
  if (h > 7) {
    g.moveTo(x, t + h * 0.45);
    g.lineTo(x + w, t + h * 0.45);
  }
  g.stroke();
  // Sill and lintel.
  rr(g, lite(frame, 0.1), x - 1.8, -z, w + 3.6, 1.5);
  rr(g, 'rgba(25,15,35,.28)', x - 1.4, -z + 1.5, w + 2.8, 1);
  rr(g, dark(frame, 0.2), x - 1.4, t - 1.8, w + 2.8, 0.9);
  if (o.shutters) {
    const sw = Math.max(1.8, w * 0.42);
    rr(g, o.shutters, x - 1 - sw, t - 0.6, sw, h + 1.2);
    rr(g, o.shutters, x + w + 1, t - 0.6, sw, h + 1.2);
    for (let k = 1; k < h / 1.7; k++) {
      rr(g, 'rgba(0,0,0,.25)', x - 1 - sw, t - 0.6 + k * 1.7, sw, 0.5);
      rr(g, 'rgba(0,0,0,.25)', x + w + 1, t - 0.6 + k * 1.7, sw, 0.5);
    }
  }
  if (o.box) {
    rr(g, '#7a4b2a', x - 1.4, -z + 1.5, w + 2.8, 2.6);
    rr(g, '#9a6a40', x - 1.4, -z + 1.5, w + 2.8, 0.8);
    for (let k = 0; k < w + 1; k += 1.6) {
      ell(g, ['#ff7fa6', '#ffd75e', '#ffffff', '#ff9a5a'][Math.floor(k) % 4], x - 0.4 + k, -z + 0.4, 1, 0.9);
      rr(g, '#3f8a3c', x - 0.5 + k, -z + 0.8, 0.8, 1.2);
    }
  }
}

function door(S, x, w, h, color, o = {}) {
  const g = S.g;
  if (!S.day) {
    if (o.lamp !== false) {
      const lx = x + w + 2.5;
      rr(g, '#fff4b8', lx - 0.9, -h - 1, 1.8, 2.6);
      ell(g, 'rgba(255,220,130,.28)', lx, -h, 6, 4);
    }
    return;
  }
  rr(g, '#d5cebd', x - 2.4, -1.5, w + 4.8, 1.5);
  rr(g, '#ebe5d4', x - 2.4, -2, w + 4.8, 0.6);
  rr(g, TRIM, x - 1.4, -h - 1.4, w + 2.8, h + 1.4);
  rr(g, grad(g, x, 0, x + w, 0, lite(color, 0.12), dark(color, 0.12)), x, -h, w, h - 1.5);
  const pw = w / 2 - 1.2;
  rr(g, dark(color, 0.22), x + 0.8, -h + 1, pw, h * 0.34);
  rr(g, dark(color, 0.22), x + w / 2 + 0.4, -h + 1, pw, h * 0.34);
  rr(g, dark(color, 0.2), x + 0.8, -h * 0.55, pw, h * 0.35);
  rr(g, dark(color, 0.2), x + w / 2 + 0.4, -h * 0.55, pw, h * 0.35);
  rr(g, grad(g, 0, -h + 1, 0, -h + 4, GLASS_TOP, GLASS_BOT), x + 1, -h + 1.4, w - 2, 2.6);
  ell(g, '#e8c860', x + w - 1.4, -h * 0.45, 0.6, 0.6);
  rr(g, 'rgba(25,15,35,.3)', x - 0.2, -h - 0.4, w + 0.4, 0.7);
  if (o.lamp !== false) {
    const lx = x + w + 2.5;
    rr(g, '#2c2f3c', lx - 1, -h - 1.4, 2, 3);
    rr(g, '#ffe9a0', lx - 0.7, -h - 1, 1.4, 2.2);
  }
}

function ledge(S, x, z, w, c = TRIM) {
  if (!S.day) return;
  rr(S.g, c, x, -z, w, 1.4);
  rr(S.g, 'rgba(25,15,35,.28)', x, -z + 1.4, w, 1.2);
}

// Window layout along a face: evenly spaced, with the ground row's slot optionally a door.
function layout(len, w, gap, margin) {
  const count = Math.max(1, Math.floor((len - margin * 2 + gap) / (w + gap)));
  const span = count * w + (count - 1) * gap;
  return { count, start: (len - span) / 2, step: w + gap };
}

// --- Roofs ---------------------------------------------------------------------------------------

function shingleLine(g, a, b, dark1 = 0.32) {
  g.strokeStyle = `rgba(30,14,20,${dark1})`;
  g.lineWidth = 0.7;
  g.beginPath();
  g.moveTo(a[0], a[1]);
  g.lineTo(b[0], b[1]);
  g.stroke();
  g.strokeStyle = 'rgba(255,225,190,.14)';
  g.beginPath();
  g.moveTo(a[0], a[1] + 0.8);
  g.lineTo(b[0], b[1] + 0.8);
  g.stroke();
}

// A sloped roof plane: rows run along the eave and shingle joints go down the slope.
function slope(g, base, [aTop0, aTop1], [aEdge0, aEdge1], rows, joint = 3, bright = 0) {
  // Each argument is a pair of 3D points: the two ends of the ridge line and the two ends of the eave.
  const pts = [P(...aTop0), P(...aTop1), P(...aEdge1), P(...aEdge0)];
  g.save();
  pathOf(g, pts);
  g.clip();
  const mid0 = P(...aTop0);
  const mid1 = P(...aEdge0);
  g.fillStyle = grad(g, mid0[0], mid0[1], mid1[0], mid1[1], lite(base, 0.16 + bright), base, dark(base, 0.14));
  g.fillRect(-120, -120, 320, 320);
  const at = (t) => [0, 1, 2].map((k) => lerp(aTop0[k], aEdge0[k], t));
  const at1 = (t) => [0, 1, 2].map((k) => lerp(aTop1[k], aEdge1[k], t));
  for (let r = 1; r < rows; r++) {
    const t = r / rows;
    shingleLine(g, P(...at(t)), P(...at1(t)));
  }
  const len = Math.hypot(aTop1[0] - aTop0[0], aTop1[1] - aTop0[1]);
  for (let r = 0; r < rows; r++) {
    const t0 = r / rows;
    const t1 = (r + 1) / rows;
    for (let s = (r % 2) * joint * 0.5 + joint * 0.5; s < len; s += joint) {
      const q = s / len;
      const p0 = [0, 1, 2].map((k) => lerp(at(t0)[k], at1(t0)[k], q));
      const p1 = [0, 1, 2].map((k) => lerp(at(t1)[k], at1(t1)[k], q));
      g.strokeStyle = 'rgba(30,14,20,.2)';
      g.lineWidth = 0.6;
      g.beginPath();
      g.moveTo(...P(...p0));
      g.lineTo(...P(...p1));
      g.stroke();
    }
  }
  grain(g, 0.08);
  g.restore();
}

// A gabled roof. axis 'u' means the ridge runs along u, so the slope that faces us runs down to +v.
function gable(S, R, zE, rise, base, o, axis, mat) {
  if (!S.day) return;
  const g = S.g;
  const [u0, v0, u1, v1] = R;
  const zR = zE + rise;
  const rows = Math.max(5, Math.round(rise * 0.9));
  if (axis === 'u') {
    const vm = (v0 + v1) / 2;
    const ve = v1 + o;
    const zA = (v) => zE + (v1 - v) * (rise / (v1 - vm));
    // The gable end, in wall material.
    onR(g, u1, v1, () => {
      const len = v1 - v0;
      g.save();
      g.beginPath();
      g.moveTo(-0.5, -zE + 0.5);
      g.lineTo(len + 0.5, -zE + 0.5);
      g.lineTo(len / 2, -zR - 0.6);
      g.closePath();
      g.clip();
      paintWall(g, len, zE - 1, zR + 1, mat, 41);
      shadeFace(g, len, zE - 1, zR + 1);
      g.restore();
      rr(g, 'rgba(30,20,50,.4)', len / 2 - 2, -zE - rise * 0.45, 4, 5);
      rr(g, 'rgba(255,255,255,.25)', len / 2 - 1.4, -zE - rise * 0.45 + 0.6, 2.8, 3.8);
    });
    slope(g, base, [[u0 - o, vm, zR], [u1 + o, vm, zR]], [[u0 - o, ve, zA(ve)], [u1 + o, ve, zA(ve)]], rows, 3.2);
    fp3(g, dark(base, 0.38), [u1 + o, vm, zR], [u1 + o, ve, zA(ve)], [u1 + o, ve, zA(ve) - 1.8], [u1 + o, vm, zR - 1.8]);
    fp3(g, '#efe8d6', [u0 - o, ve, zA(ve)], [u1 + o, ve, zA(ve)], [u1 + o, ve, zA(ve) - 1.8], [u0 - o, ve, zA(ve) - 1.8]);
    fp3(g, 'rgba(30,20,50,.26)', [u0 - o, ve, zA(ve) - 1.8], [u1 + o, ve, zA(ve) - 1.8], [u1 + o, ve, zA(ve) - 2.8], [u0 - o, ve, zA(ve) - 2.8]);
    line(g, lite(base, 0.45), 1.2, ...P(u0 - o, vm, zR), ...P(u1 + o, vm, zR));
  } else {
    const um = (u0 + u1) / 2;
    const ue = u1 + o;
    const zA = (u) => zE + (u1 - u) * (rise / (u1 - um));
    onL(g, u0, v1, () => {
      const len = u1 - u0;
      g.save();
      g.beginPath();
      g.moveTo(-0.5, -zE + 0.5);
      g.lineTo(len + 0.5, -zE + 0.5);
      g.lineTo(len / 2, -zR - 0.6);
      g.closePath();
      g.clip();
      paintWall(g, len, zE - 1, zR + 1, mat, 43);
      g.restore();
      rr(g, 'rgba(30,20,50,.4)', len / 2 - 2, -zE - rise * 0.45, 4, 5);
      rr(g, 'rgba(255,255,255,.3)', len / 2 - 1.4, -zE - rise * 0.45 + 0.6, 2.8, 3.8);
    });
    slope(g, dark(base, 0.14), [[um, v0 - o, zR], [um, v1 + o, zR]], [[ue, v0 - o, zA(ue)], [ue, v1 + o, zA(ue)]], rows, 3.2);
    fp3(g, dark(base, 0.4), [um, v1 + o, zR], [ue, v1 + o, zA(ue)], [ue, v1 + o, zA(ue) - 1.8], [um, v1 + o, zR - 1.8]);
    fp3(g, '#d9d2c0', [ue, v0 - o, zA(ue)], [ue, v1 + o, zA(ue)], [ue, v1 + o, zA(ue) - 1.8], [ue, v0 - o, zA(ue) - 1.8]);
    line(g, lite(base, 0.3), 1.2, ...P(um, v0 - o, zR), ...P(um, v1 + o, zR));
  }
}

// A hipped roof: slopes on all four sides, the ridge along the longer way.
function hip(S, R, zE, rise, base, o) {
  if (!S.day) return;
  const g = S.g;
  const [u0, v0, u1, v1] = R;
  const wu = u1 - u0;
  const wv = v1 - v0;
  const zR = zE + rise;
  const rows = Math.max(5, Math.round(rise * 0.9));
  const eaveZ = (d) => zE - d * (rise / (Math.min(wu, wv) / 2));
  const dz = eaveZ(o);
  let ra;
  let rb;
  if (wu >= wv) {
    ra = [u0 + wv / 2, (v0 + v1) / 2];
    rb = [u1 - wv / 2, (v0 + v1) / 2];
  } else {
    ra = [(u0 + u1) / 2, v0 + wu / 2];
    rb = [(u0 + u1) / 2, v1 - wu / 2];
  }
  // Front (+v) slope, then right (+u) slope.
  slope(g, base, [[ra[0], ra[1], zR], [rb[0], rb[1], zR]], [[u0 - o, v1 + o, dz], [u1 + o, v1 + o, dz]], rows, 3.2);
  slope(g, dark(base, 0.16), [[rb[0], rb[1], zR], [rb[0], rb[1], zR]], [[u1 + o, v1 + o, dz], [u1 + o, v0 - o, dz]], rows, 3.2);
  fp3(g, '#efe8d6', [u0 - o, v1 + o, dz], [u1 + o, v1 + o, dz], [u1 + o, v1 + o, dz - 1.8], [u0 - o, v1 + o, dz - 1.8]);
  fp3(g, '#cfc7b3', [u1 + o, v1 + o, dz], [u1 + o, v0 - o, dz], [u1 + o, v0 - o, dz - 1.8], [u1 + o, v1 + o, dz - 1.8]);
  line(g, lite(base, 0.4), 1.1, ...P(ra[0], ra[1], zR), ...P(rb[0], rb[1], zR));
}

// A flat roof with a parapet, and a little rooftop clutter.
function flatRoof(S, R, z, mat, o = {}) {
  if (!S.day) return;
  const g = S.g;
  const [u0, v0, u1, v1] = R;
  const t = o.parapet ?? 2.5;
  // Parapet walls, then the tarred floor inside them.
  onL(g, u0, v1, () => {
    rr(g, grad(g, 0, -z - t, 0, -z, lite(o.coping || '#e8e2d2', 0.1), o.coping || '#d8d1c0'), -0.3, -z - t, u1 - u0 + 0.6, t);
    rr(g, 'rgba(25,15,35,.24)', 0, -z - 0.8, u1 - u0, 0.8);
  });
  onR(g, u1, v1, () => {
    rr(g, grad(g, 0, -z - t, 0, -z, '#c8c1af', '#aaa391'), -0.3, -z - t, v1 - v0 + 0.6, t);
    rr(g, 'rgba(36,26,70,.34)', -0.3, -z - t, v1 - v0 + 0.6, t);
  });
  const zt = z + t;
  fp3(g, '#e5dfce', [u0, v0, zt], [u1, v0, zt], [u1, v1, zt], [u0, v1, zt]);
  fp3(g, grad(g, ...P(u0, v0, zt), ...P(u1, v1, zt), '#747b8c', '#5d6474'), [u0 + 1, v0 + 1, zt], [u1 - 1, v0 + 1, zt], [u1 - 1, v1 - 1, zt], [u0 + 1, v1 - 1, zt]);
  fp3(g, 'rgba(25,20,40,.3)', [u0 + 1, v0 + 1, zt], [u1 - 1, v0 + 1, zt], [u1 - 1, v0 + 2.6, zt], [u0 + 1, v0 + 2.6, zt]);
  fp3(g, 'rgba(25,20,40,.3)', [u0 + 1, v0 + 1, zt], [u0 + 2.6, v0 + 1, zt], [u0 + 2.6, v1 - 1, zt], [u0 + 1, v1 - 1, zt]);
}

function brickChimney(S, u, v, z0, z1, w = 4, smoke = false) {
  if (!S.day) return;
  const g = S.g;
  const R = [u, v, u + w, v + w];
  box(S, R, z0, z1, { k: 'brick', c: '#a95e45' });
  fp3(g, '#7a6a62', [u - 0.8, v - 0.8, z1], [u + w + 0.8, v - 0.8, z1], [u + w + 0.8, v + w + 0.8, z1], [u - 0.8, v + w + 0.8, z1]);
  fp3(g, '#2b2528', [u + 0.6, v + 0.6, z1], [u + w - 0.6, v + 0.6, z1], [u + w - 0.6, v + w - 0.6, z1], [u + 0.6, v + w - 0.6, z1]);
  fp3(g, '#9a8d86', [u - 0.8, v + w + 0.8, z1 - 1.2], [u + w + 0.8, v + w + 0.8, z1 - 1.2], [u + w + 0.8, v + w + 0.8, z1], [u - 0.8, v + w + 0.8, z1]);
  if (smoke && S.info) {
    const [x, y] = P(u + w / 2, v + w / 2, z1);
    S.info.smoke.push([Math.round(x), Math.round(y)]);
  }
  shadow(S, R, z1 - z0);
}

function acUnit(S, u, v, z) {
  if (!S.day) return;
  const g = S.g;
  const R = [u, v, u + 5, v + 4];
  box(S, R, z, z + 3.6, { k: 'steel', c: '#c9ced8' });
  topFace(S, R, z + 3.6, '#dde1ea');
  onL(g, u, v + 4, () => {
    ell(g, '#3a3f50', 2.5, -z - 1.8, 1.7, 1.5);
    for (let k = 0; k < 4; k++) line(g, '#8a90a2', 0.5, 2.5 - 1.6, -z - 1.8 + k * 0.5 - 0.8, 2.5 + 1.6, -z - 1.8 + k * 0.5 - 0.8);
  });
}

function bush(S, u, v, r = 1) {
  if (!S.day) return;
  const [x, y] = P(u, v);
  drawTree(S.g, x, y, 'bush', Math.round(u * 5 + v * 3));
  void r;
}

function smallTree(S, u, v, type = 'small') {
  if (!S.day) return;
  const [x, y] = P(u, v);
  drawTree(S.g, x, y, type, Math.round(u * 5 + v * 3));
}

// A picket fence along a line in the ground plane.
function picket(S, u0, v0, u1, v1, h = 5, color = '#f6f1e4') {
  if (!S.day) return;
  const g = S.g;
  const len = Math.hypot(u1 - u0, v1 - v0);
  const n = Math.max(2, Math.round(len / 2));
  for (const z of [1.6, h - 1.4]) {
    const a = P(u0, v0, z);
    const b = P(u1, v1, z);
    line(g, '#e3dccb', 1, a[0], a[1], b[0], b[1]);
  }
  for (let i = 0; i <= n; i++) {
    const [x, y] = P(lerp(u0, u1, i / n), lerp(v0, v1, i / n), 0);
    rr(g, color, x - 0.7, y - h, 1.4, h);
    fp(g, color, [[x - 0.7, y - h], [x, y - h - 1.2], [x + 0.7, y - h]]);
    rr(g, 'rgba(25,15,35,.18)', x + 0.2, y - h, 0.5, h);
  }
}

// A path or driveway across the ground.
function paving(S, R, fill = '#d8cfb4', edge = '#b5aa8c') {
  if (!S.day) return;
  const [u0, v0, u1, v1] = R;
  fp3(S.g, edge, [u0 - 0.6, v0 - 0.6], [u1 + 0.6, v0 - 0.6], [u1 + 0.6, v1 + 0.6], [u0 - 0.6, v1 + 0.6]);
  fp3(S.g, fill, [u0, v0], [u1, v0], [u1, v1], [u0, v1]);
}

// A garden bed on the ground with flowers.
function bed(S, u, v, r, kind = 0) {
  if (!S.day) return;
  const g = S.g;
  disc(g, '#6e5436', u, v, r + 0.8);
  disc(g, '#4f3a26', u, v, r);
  const rand = seeded(Math.round(u * 17 + v * 5));
  const pal = [['#ff7fa6', '#ffd75e', '#fff'], ['#c9a9ff', '#ff9a5a', '#fff3a8'], ['#ff6f6f', '#ffe27a', '#ffffff']][kind % 3];
  for (let i = 0; i < 11; i++) {
    const a = rand() * 6.2832;
    const d = rand() * r * 0.85;
    const [x, y] = P(u + Math.cos(a) * d, v + Math.sin(a) * d);
    line(g, '#3d6f2a', 0.7, x, y, x, y - 1.6);
    ell(g, pal[i % 3], x, y - 2, 1.2, 1);
  }
}

// A parked car, painted into a building sprite (same car as the traffic, but still).
function parkedCar(S, u, v, alongU, colorIdx) {
  if (!S.day) return;
  const [x, y] = P(u, v);
  const s = carSprite(alongU ? 0 : 1, colorIdx);
  S.g.drawImage(s.c, x - s.ax, y - s.ay, s.w, s.h);
}

// --- Homes ---------------------------------------------------------------------------------------

function porch(S, side, R, at, w, z) {
  // A small covered porch in front of the door: a deck, two posts and a lean-to roof.
  if (!S.day) return;
  const g = S.g;
  const [u0, v0, u1, v1] = R;
  if (side === 'L') {
    const a = u0 + at;
    const b = a + w;
    const vd = v1 + 5;
    fp3(g, '#b6a889', [a - 1, v1, 1.4], [b + 1, v1, 1.4], [b + 1, vd, 1.4], [a - 1, vd, 1.4]);
    fp3(g, '#8d806a', [a - 1, vd, 0], [b + 1, vd, 0], [b + 1, vd, 1.4], [a - 1, vd, 1.4]);
    for (const u of [a - 0.2, b + 0.2]) {
      const [x, y] = P(u, vd - 0.4, 1.4);
      rr(g, grad(g, x - 0.8, 0, x + 0.8, 0, '#fffaf0', '#cfc7b3'), x - 0.8, y - (z - 1.4), 1.6, z - 1.4);
    }
    fp3(g, '#9a4a38', [a - 2, v1 - 0.5, z + 2], [b + 2, v1 - 0.5, z + 2], [b + 2, vd + 1, z - 0.6], [a - 2, vd + 1, z - 0.6]);
    fp3(g, '#c0654f', [a - 2, v1 - 0.5, z + 2], [b + 2, v1 - 0.5, z + 2], [b + 2, v1 + 1.6, z + 1.2], [a - 2, v1 + 1.6, z + 1.2]);
    fp3(g, '#7a3a2c', [a - 2, vd + 1, z - 0.6], [b + 2, vd + 1, z - 0.6], [b + 2, vd + 1, z - 1.9], [a - 2, vd + 1, z - 1.9]);
  } else {
    const a = v1 - at - w;
    const b = a + w;
    const ud = u1 + 5;
    fp3(g, '#a79a7c', [u1, a - 1, 1.4], [ud, a - 1, 1.4], [ud, b + 1, 1.4], [u1, b + 1, 1.4]);
    fp3(g, '#7a6e5a', [ud, a - 1, 0], [ud, b + 1, 0], [ud, b + 1, 1.4], [ud, a - 1, 1.4]);
    for (const v of [a - 0.2, b + 0.2]) {
      const [x, y] = P(ud - 0.4, v, 1.4);
      rr(g, '#cfc7b3', x - 0.8, y - (z - 1.4), 1.6, z - 1.4);
    }
    fp3(g, '#7f3a2c', [u1 - 0.5, a - 2, z + 2], [ud + 1, a - 2, z - 0.6], [ud + 1, b + 2, z - 0.6], [u1 - 0.5, b + 2, z + 2]);
    fp3(g, '#6a3023', [ud + 1, a - 2, z - 0.6], [ud + 1, b + 2, z - 0.6], [ud + 1, b + 2, z - 1.9], [ud + 1, a - 2, z - 1.9]);
  }
}

function mailbox(S, u, v) {
  if (!S.day) return;
  const [x, y] = P(u, v);
  rr(S.g, '#5a4a3a', x - 0.4, y - 6, 0.8, 6);
  rr(S.g, '#2f5d9a', x - 2, y - 9, 4, 3);
  ell(S.g, '#3a6fb5', x, y - 9, 2, 0.8);
  rr(S.g, '#e5484d', x + 2, y - 9, 0.8, 2);
}

function homeOne(S, c) {
  const g = S.g;
  const R = [8, 10, 24, 22];
  const brick = S.v === 1;
  const mat = brick ? { k: 'brick', c: BRICKS[S.pal % 4] } : { k: 'siding', c: c.wall };
  const H = 12;
  const doorL = S.door === 'L';
  // Path and garden first.
  const doorU = 12;
  paving(S, doorL ? [doorU - 1.6, 22, doorU + 1.6, 31] : [24, 14.4, 31, 17.6]);
  bed(S, 10, 26, 3, S.v);
  bed(S, 21, 26, 2.6, S.v + 1);
  shadow(S, R, H + 8);
  const detail = (side, len) => {
    const isDoor = (side === 'L') === doorL;
    const lay = layout(len, 4.4, 3.4, 2);
    for (let k = 0; k < lay.count; k++) {
      const x = lay.start + k * lay.step;
      if (isDoor && k === Math.floor((lay.count - 1) / 2)) door(S, x + (4.4 - 4.6) / 2, 4.6, 8.4, c.door);
      else win(S, x, 4, 4.4, 5.4, { shutters: S.v === 0 ? c.door : null, box: S.v === 2, curtain: k % 2 ? '#e8c8d8' : '#d8e4f4' });
    }
  };
  box(S, R, 0, H, mat, detail);
  if (S.v === 2) {
    hip(S, R, H, 9, c.roof, 1.8);
    brickChimney(S, 17, 12, 17, 24, 3.4);
  } else {
    gable(S, R, H, 9, c.roof, 1.8, S.v === 1 ? 'v' : 'u', mat);
    brickChimney(S, S.v === 1 ? 10 : 18, S.v === 1 ? 12 : 12, S.v === 1 ? 17 : 17, S.v === 1 ? 24 : 24, 3.4);
  }
  if (S.v === 0) porch(S, doorL ? 'L' : 'R', R, doorU - 8 + 4 + (doorL ? 0 : 0), 6, 11);
  picket(S, 4, 29, 11, 29);
  picket(S, 14, 29, 28, 29);
  picket(S, 4, 29, 4, 25);
  picket(S, 28, 29, 28, 25);
  mailbox(S, 29, 27);
  bush(S, 7, 24, 1);
  bush(S, 25, 25, 1);
  if (S.v === 1) smallTree(S, 5, 8, 'small');
  void g;
}

function homeTwo(S, c) {
  const g = S.g;
  const brick = S.v === 1;
  const mat = brick ? { k: 'brick', c: BRICKS[(S.pal + 1) % 4] } : { k: 'siding', c: c.wall };
  const R = [6, 6, 21, 22];
  const H = 23;
  const doorL = S.door === 'L';
  const doorU = 11;
  // Driveway and garage wing on the right, with a car.
  const G = [21, 12, 29, 25];
  if (S.v !== 1) paving(S, [21, 25, 29, 32], '#bdb8aa', '#9d9788');
  paving(S, doorL ? [doorU - 1.5, 22, doorU + 1.5, 32] : [21, 9, 32, 12.2]);
  bed(S, 10, 26, 3.2, 0);
  shadow(S, R, H + 10);
  const detail = (side, len) => {
    const isDoor = (side === 'L') === doorL;
    const lay = layout(len, 4.4, 3.6, 2.4);
    for (let k = 0; k < lay.count; k++) {
      const x = lay.start + k * lay.step;
      if (isDoor && k === Math.floor((lay.count - 1) / 2)) door(S, x - 0.2, 4.8, 9, c.door);
      else win(S, x, 4, 4.4, 5.6, { box: S.v === 2, shutters: S.v === 1 ? c.door : null, curtain: '#f1e2c0' });
      win(S, x, 15, 4.4, 5.6, { curtain: k % 2 ? '#e8c8d8' : '#d8e4f4', shutters: S.v === 1 ? c.door : null });
    }
    ledge(S, 0, 12.6, len, '#e8e1cd');
  };
  if (S.v !== 1) {
    box(S, G, 0, 11, mat, (side, len) => {
      if (side === 'L') {
        // A garage door.
        rr(g, '#f4efe2', 1.2, -9.4, len - 2.4, 9.4);
        rr(g, grad(g, 0, -8.6, 0, 0, '#f0ece0', '#d6d0c0'), 1.8, -8.6, len - 3.6, 8.6);
        for (let y = -2; y > -8.6; y -= 2.1) line(g, 'rgba(60,50,40,.3)', 0.7, 1.8, y, len - 1.8, y);
        for (let k = 0; k < 4; k++) rr(g, 'rgba(60,50,40,.25)', 2.6 + k * 2, -7.2, 1.2, 1);
      } else win(S, 3, 3, 3.6, 4.4, {});
    });
    gable(S, G, 11, 5, c.roof, 1.2, 'v', mat);
  }
  box(S, R, 0, H, mat, detail);
  if (S.v === 1) {
    hip(S, R, H, 11, c.roof, 2);
    box(S, [4, 22, 11, 27], 0, 11, mat, (side) => { if (side === 'L') { win(S, 1.4, 3, 4.2, 5.2, { shutters: c.door }); } });
    topFace(S, [4, 22, 11, 27], 11, lite(c.roof, 0.1));
    brickChimney(S, 9, 8, 26, 34, 3.6);
  } else if (S.v === 2) {
    gable(S, R, H, 11, c.roof, 2, 'v', mat);
    brickChimney(S, 7, 15, 25, 33, 3.4, false);
    brickChimney(S, 17, 8, 28, 36, 3.4, false);
  } else {
    gable(S, R, H, 11, c.roof, 2, 'u', mat);
    brickChimney(S, 16, 11, 28, 35, 3.6, false);
  }
  if (S.v === 0) parkedCar(S, 25, 29, true, S.pal);
  porch(S, doorL ? 'L' : 'R', R, doorL ? doorU - 6 + 6 - 1 : 0, 6, 11);
  picket(S, 3, 31, 11, 31);
  picket(S, 14, 31, 21, 31);
  bush(S, 6, 29, 1);
  bush(S, 12, 24, 1);
  smallTree(S, 4, 6, S.v === 2 ? 'oak' : 'small');
  mailbox(S, 3, 28);
}

function homeThree(S, c) {
  const g = S.g;
  const brick = S.v !== 1;
  const mat = brick ? { k: 'brick', c: BRICKS[S.pal % 4] } : { k: 'siding', c: lite(c.wall, 0.05) };
  const doorL = S.door === 'L';
  let R = [5, 5, 27, 27];
  let H = 54;
  const rows = [4, 15, 26, 37];
  const detail = (R2) => (side, len) => {
    const isDoor = (side === 'L') === doorL;
    const lay = layout(len, 4.2, 3.2, 2.2);
    const zTop = R2.H;
    for (let k = 0; k < lay.count; k++) {
      const x = lay.start + k * lay.step;
      if (isDoor && k === Math.floor((lay.count - 1) / 2)) door(S, x - 0.3, 4.8, 9, c.door);
      else win(S, x, 4, 4.2, 6, { curtain: '#f1e2c0' });
      for (let r = 1; r < rows.length; r++) {
        if (rows[r] + 8 > zTop - 3) continue;
        win(S, x, rows[r], 4.2, 6, { curtain: (k + r) % 3 === 0 ? '#e8c8d8' : '#d8e4f4', box: side === 'L' && S.v === 1 && r > 0 });
        if (S.v === 1 && side === 'L' && r > 0 && (k + r) % 2 === 0) {
          // A balcony rail.
          rr(g, 'rgba(25,15,35,.32)', x - 1.6, -rows[r] + 0.2, 7.4, 3.2);
          rr(g, '#eeeae0', x - 1.6, -rows[r] - 3, 7.4, 0.9);
          for (let q = 0; q < 6; q++) rr(g, '#eeeae0', x - 1.2 + q * 1.3, -rows[r] - 3, 0.5, 3.2);
        }
      }
    }
    for (const z of [13.2, 24.2, 35.2]) if (z + 6 < zTop) ledge(S, 0, z, len, '#e2dac6');
  };
  if (S.v === 2) {
    // A tall block behind and a lower block in front.
    const back = [5, 5, 21, 17];
    shadow(S, back, 62);
    box(S, back, 0, 62, mat, (side, len) => {
      const lay = layout(len, 4.2, 3.2, 2.2);
      for (let k = 0; k < lay.count; k++) for (let r = 0; r < 5; r++) win(S, lay.start + k * lay.step, 4 + r * 11, 4.2, 6, { curtain: (k + r) % 3 === 0 ? '#e8c8d8' : '#d8e4f4' });
    });
    flatRoof(S, back, 62, mat);
    R = [8, 15, 27, 27];
    H = 38;
  }
  paving(S, doorL ? [R[0] + 6, R[3], R[0] + 11, 32] : [R[2], 18, 32, 22]);
  shadow(S, R, H + 4);
  box(S, R, 0, H, mat, detail({ H }));
  flatRoof(S, R, H, mat);
  const zt = H + 2.5;
  // The roof: a tank, vents, aerials, a sunny terrace or two.
  acUnit(S, R[0] + 3, R[1] + 3, zt);
  acUnit(S, R[0] + 9, R[1] + 3, zt);
  if (S.v === 0) {
    const T = [R[2] - 9, R[1] + 3, R[2] - 4, R[1] + 8];
    box(S, T, zt, zt + 6, { k: 'steel', c: '#b57b4f' });
    topFace(S, T, zt + 6, '#d89a64');
    for (const [u, v] of [[R[2] - 8.5, R[1] + 3.5], [R[2] - 4.5, R[1] + 7.5]]) {
      const [x, y] = P(u, v, zt);
      rr(g, '#6a4a2a', x - 0.5, y - 7, 1, 7);
    }
  } else if (S.v === 1) {
    for (const [u, v] of [[R[2] - 7, R[1] + 6], [R[2] - 11, R[1] + 11]]) {
      const [x, y] = P(u, v, zt);
      line(g, '#8f96a8', 1, x, y, x, y - 14);
      line(g, '#8f96a8', 0.8, x - 4, y - 11, x + 4, y - 11);
      line(g, '#8f96a8', 0.8, x - 3, y - 8, x + 3, y - 8);
    }
    smallTree(S, R[0] + 6, R[3] - 7, 'small');
  } else {
    const [x, y] = P(R[2] - 8, R[3] - 8, zt);
    ell(g, '#d8dce6', x, y - 4, 4.5, 4.5);
    ell(g, '#f3f5fa', x - 1, y - 5, 2.4, 2.4);
    line(g, '#8f96a8', 1, x, y - 2, x, y);
    if (S.info) S.info.flag = [x + 20, y - 34, c.roof];
  }
  smallTree(S, 3, 29, 'small');
  bush(S, 28, 29, 1);
}

// --- Shops ---------------------------------------------------------------------------------------

function awning(S, R, z, depth, colors) {
  // A striped awning over the left face, sloping down and out, with a scalloped edge.
  if (!S.day) return;
  const g = S.g;
  const [u0, , u1, v1] = R;
  const n = Math.round((u1 - u0) / 2.6);
  const sw = (u1 - u0) / n;
  for (let k = 0; k < n; k++) {
    const a = u0 + k * sw;
    const b = a + sw;
    const col = colors[k % 2];
    fp3(g, grad(g, ...P(a, v1, z), ...P(a, v1 + depth, z - 4), lite(col, 0.12), dark(col, 0.1)), [a, v1, z], [b, v1, z], [b, v1 + depth, z - 4], [a, v1 + depth, z - 4]);
    const [x, y] = P((a + b) / 2, v1 + depth, z - 4);
    ell(g, col, x, y + 0.8, sw * 0.7, 1.7);
    ell(g, rgba('#000', 0.14), x, y + 1.2, sw * 0.7, 1.3);
  }
  fp3(g, 'rgba(25,15,35,.22)', [u0, v1, z - 5], [u1, v1, z - 5], [u1, v1 + depth, z - 9], [u0, v1 + depth, z - 9]);
}

function lettering(g, x, y, w, h, color) {
  // Little blocks that read as a shop's name from a distance.
  const rand = seeded(Math.round(x * 13 + w * 7));
  let cx = x;
  while (cx < x + w - 1) {
    const lw = 1.6 + rand() * 2.4;
    rr(g, color, cx, y, Math.min(lw, x + w - cx - 0.6), h);
    cx += lw + 1;
  }
}

function sign(S, x, z, w, h, color, o = {}) {
  const g = S.g;
  if (!S.day) {
    rr(g, rgba(lite(color, 0.4), 0.9), x, -(z + h), w, h);
    rr(g, '#ffffff', x + 1.2, -(z + h) + h * 0.3, w - 2.4, h * 0.4);
    return;
  }
  rr(g, '#2a3045', x - 1, -(z + h) - 1, w + 2, h + 2);
  rr(g, grad(g, 0, -(z + h), 0, -z, lite(color, 0.2), color), x, -(z + h), w, h);
  rr(g, 'rgba(255,255,255,.25)', x, -(z + h), w, 0.8);
  if (o.name !== false) lettering(g, x + 2, -(z + h) + h * 0.32, w - 4, h * 0.36, '#fff8ea');
}

function shopFront(S, len, z, h, glassCount, doorSlot, c, goods = true) {
  // Large shop windows with goods behind and a door.
  const g = S.g;
  const gw = (len - 4) / glassCount;
  if (!S.day) {
    for (let k = 0; k < glassCount; k++) {
      if (k === doorSlot) continue;
      const x = 2 + k * gw;
      rr(g, grad(g, 0, -(z + h), 0, -z, '#fff0b8', '#ffc260'), x + 0.5, -(z + h), gw - 1, h);
    }
    return;
  }
  rr(g, '#454c60', 0.5, -z - 1, len - 1, 2);
  for (let k = 0; k < glassCount; k++) {
    const x = 2 + k * gw;
    if (k === doorSlot) {
      door(S, x + 1, gw - 2, h + 1, c.door, { lamp: false });
      continue;
    }
    rr(g, '#3a4156', x, -(z + h) - 0.6, gw, h + 1.4);
    rr(g, grad(g, 0, -(z + h), 0, -z, GLASS_TOP, '#9cc7e0', GLASS_BOT), x + 0.8, -(z + h), gw - 1.6, h);
    if (goods) {
      const rand = seeded(Math.round(x * 3 + c.hue));
      for (let i = 0; i < 6; i++) rr(g, ['#e5484d', '#f0b030', '#58b368', '#4d86d8', '#f7e7b6'][Math.floor(rand() * 5)], x + 1.6 + rand() * (gw - 5), -z - 1 - rand() * 2.4, 1.6 + rand() * 2.6, 1 + rand() * 2.4);
      rr(g, 'rgba(70,50,30,.45)', x + 0.8, -z - 1.2, gw - 1.6, 1.2);
    }
    fp(g, 'rgba(255,255,255,.28)', [[x + 1.4, -(z + h)], [x + gw * 0.5, -(z + h)], [x + 1.4, -(z + h) + h * 0.8]]);
  }
}

function shopOne(S, c) {
  const g = S.g;
  const [a, b] = c.awn;
  const R = [9, 11, 23, 23];
  const H = 11;
  const wood = { k: 'siding', c: '#c89b62' };
  paving(S, [5, 7, 27, 29], '#d6ccb1', '#b3a887');
  shadow(S, R, H + 12);
  box(S, R, 0, H, wood, (side, len) => {
    if (side === 'L') {
      // A serving hatch with a counter.
      rr(g, '#2a2f3e', 3, -9, len - 6, 6.4);
      rr(g, grad(g, 0, -8.4, 0, -3, '#f4d9a0', '#c99a52'), 3.6, -8.4, len - 7.2, 5.2);
      for (let k = 0; k < 4; k++) rr(g, ['#e5484d', '#f0b030', '#58b368', '#f7e7b6'][k], 5 + k * 2.4, -4.2, 1.8, 1.4);
      rr(g, '#8a5a30', 1.6, -3.2, len - 3.2, 1.2);
      rr(g, '#b98350', 1.6, -3.2, len - 3.2, 0.5);
    } else win(S, 4, 3, 3.6, 4, {});
  });
  // A shallow pitched roof and a big striped awning over the hatch.
  if (S.day) {
    slope(g, c.roof, [[8, 12, 17], [24, 12, 17]], [[8, 25, 11], [24, 25, 11]], 6, 3.2);
    fp3(g, dark(c.roof, 0.4), [24, 12, 17], [24, 25, 11], [24, 25, 9.6], [24, 12, 15.6]);
  }
  awning(S, [9, 11, 23, 23], 11, 6, c.awn);
  // Crates of fruit and a table with a parasol.
  for (const [u, v, w2] of [[10, 25, 3], [14, 25.4, 3.4], [18, 25, 3]]) {
    const R2 = [u, v, u + w2, v + 2.6];
    box(S, R2, 0, 3.4, { k: 'siding', c: '#b98350' });
    topFace(S, R2, 3.4, '#d7a368');
    const rand = seeded(Math.round(u * 7));
    for (let k = 0; k < 6; k++) {
      const [x, y] = P(u + 0.5 + rand() * (w2 - 1), v + 0.4 + rand() * 1.8, 3.4);
      ell(g, ['#e5484d', '#f08a30', '#7ec850', '#f5d84a'][Math.floor(rand() * 4)], x, y - 0.6, 1.1, 0.9);
    }
  }
  const [px, py] = P(26, 22);
  rr(g, '#6a4a2a', px - 0.6, py - 14, 1.2, 14);
  fp(g, a, [[px - 9, py - 11], [px + 9, py - 11], [px, py - 18]]);
  fp(g, b, [[px - 3, py - 11], [px + 3, py - 11], [px, py - 18]]);
  ell(g, rgba('#000', 0.15), px, py - 10.5, 9, 1.3);
  bush(S, 6, 9, 1);
}

function shopTwo(S, c) {
  const g = S.g;
  const mat = S.v === 1 ? { k: 'brick', c: BRICKS[S.pal % 4] } : { k: 'siding', c: c.wall };
  const R = [5, 8, 27, 25];
  const H = 24;
  paving(S, [5, 25, 27, 32], '#d4cbb2', '#b5aa8c');
  shadow(S, R, H + 4);
  box(S, R, 0, H, mat, (side, len) => {
    if (side === 'L') {
      shopFront(S, len, 2, 10, 3, 1, c);
      const lay = layout(len, 4.4, 3.6, 2.2);
      for (let k = 0; k < lay.count; k++) win(S, lay.start + k * lay.step, 14, 4.4, 6, { box: true, curtain: '#f1e2c0' });
      sign(S, 3, 11.4, len - 6, 3, c.awn[0]);
    } else {
      const lay = layout(len, 4.4, 3.4, 2.2);
      for (let k = 0; k < lay.count; k++) {
        win(S, lay.start + k * lay.step, 4, 4.4, 6, { curtain: '#d8e4f4' });
        win(S, lay.start + k * lay.step, 14, 4.4, 6, { curtain: '#e8c8d8' });
      }
    }
    ledge(S, 0, 21.4, len, '#e8e1cd');
  });
  awning(S, R, 15, 5.2, c.awn);
  flatRoof(S, R, H, mat);
  const zt = H + 2.5;
  acUnit(S, 8, 11, zt);
  brickChimney(S, 19, 11, zt, zt + 8, 3, false);
  // Pavement tables with a parasol, a bike, potted plants.
  const [px, py] = P(22, 29.5);
  ell(g, 'rgba(25,25,35,.28)', px + 2, py + 1, 8, 2.4);
  rr(g, '#5a4a3a', px - 0.5, py - 6, 1, 6);
  ell(g, '#e9dfc4', px, py - 6.5, 5.4, 2.2);
  ell(g, '#fffaf0', px, py - 7, 5.4, 2.2);
  rr(g, '#6a4a2a', px - 0.5, py - 16, 1, 10);
  fp(g, c.awn[0], [[px - 8, py - 14], [px + 8, py - 14], [px, py - 20]]);
  fp(g, c.awn[1], [[px - 3, py - 14], [px + 3, py - 14], [px, py - 20]]);
  for (const [u, v] of [[18.5, 30.5], [28.5, 30]]) {
    const [x, y] = P(u, v);
    ell(g, '#8a5a3a', x, y - 1, 2.2, 1.1);
    rr(g, '#a56a44', x - 2, y - 4, 4, 3);
    ell(g, '#4d9a4a', x, y - 6, 2.8, 2.4);
    dot(g, '#ff7fa6', x - 1, y - 7);
  }
  mailbox(S, 4, 29);
}

function shopThree(S, c) {
  const g = S.g;
  const mat = { k: 'concrete', c: lite(c.wall, 0.15) };
  const R = [3, 5, 29, 29];
  const H = 46;
  paving(S, [3, 29, 29, 33], '#d4cbb2', '#b5aa8c');
  shadow(S, R, H + 14);
  box(S, R, 0, H, mat, (side, len) => {
    if (side === 'L') {
      shopFront(S, len, 2, 14, 5, 2, c);
      // Bands of windows above.
      for (const z of [20, 30, 39]) {
        const lay = layout(len, 5.2, 2.6, 2);
        for (let k = 0; k < lay.count; k++) win(S, lay.start + k * lay.step, z, 5.2, 6, { frame: '#cfd5e0', curtain: z === 30 ? '#f1e2c0' : null });
      }
      sign(S, 4, 17, len - 8, 3.2, c.awn[0]);
    } else {
      const lay = layout(len, 5.2, 2.6, 2);
      for (const z of [5, 14, 23, 32, 40]) for (let k = 0; k < lay.count; k++) win(S, lay.start + k * lay.step, z, 5.2, 5.6, { frame: '#cfd5e0' });
    }
    ledge(S, 0, 17.2, len, '#d2d6df');
    ledge(S, 0, 44, len, '#d2d6df');
  });
  awning(S, [3, 5, 29, 29], 18, 5, c.awn);
  flatRoof(S, R, H, mat);
  const zt = H + 2.5;
  // A big sign standing on the roof, with flags and plant on it.
  const B = [6, 22, 26, 23.4];
  box(S, B, zt + 2, zt + 15, { k: 'steel', c: '#2a3045' }, (side, len) => {
    if (side === 'L') {
      rr(g, grad(g, 0, -zt - 15, 0, -zt - 2, lite(c.awn[0], 0.2), c.awn[0]), 1, -zt - 14, len - 2, 11);
      lettering(g, 3, -zt - 11, len - 6, 3, '#fff8ea');
      lettering(g, 6, -zt - 6.5, len - 12, 2, '#ffe9a0');
      ell(g, '#ffe9a0', len - 4, -zt - 11, 2, 2);
    }
  });
  acUnit(S, 8, 8, zt);
  acUnit(S, 15, 8, zt);
  for (const [u, v] of [[8, 25], [26, 25]]) {
    const [x, y] = P(u, v, zt + 2);
    rr(g, '#8f96a8', x - 0.5, y - 12, 1, 12);
    fp(g, c.awn[1], [[x + 0.5, y - 12], [x + 7, y - 10], [x + 0.5, y - 8]]);
  }
  if (S.info) S.info.flag = [...P(26, 25, zt + 14), c.awn[0]];
  smallTree(S, 2, 31, 'small');
  smallTree(S, 30, 31, 'blossom');
}

// --- Workshops -----------------------------------------------------------------------------------

function drum(g, x, y, col, h = 7) {
  ell(g, 'rgba(25,25,35,.3)', x + 2, y + 0.6, 4.4, 1.5);
  rr(g, grad(g, x - 3, 0, x + 3, 0, lite(col, 0.2), dark(col, 0.25)), x - 3, y - h, 6, h);
  ell(g, dark(col, 0.2), x, y, 3, 1.3);
  ell(g, lite(col, 0.3), x, y - h, 3, 1.3);
  rr(g, 'rgba(0,0,0,.25)', x - 3, y - h * 0.65, 6, 0.8);
  rr(g, 'rgba(0,0,0,.25)', x - 3, y - h * 0.3, 6, 0.8);
}

function lorry(S, u, v, alongU, color) {
  if (!S.day) return;
  const g = S.g;
  const [x, y] = P(u, v);
  const s = carSprite(alongU ? 0 : 1, color);
  g.drawImage(s.c, x - s.ax * 1.4, y - s.ay * 1.4, s.w * 1.4, s.h * 1.4);
}

function workOne(S, c) {
  const g = S.g;
  const mat = { k: 'steel', c: '#9fb0c4' };
  const R = [6, 9, 26, 25];
  const H = 14;
  paving(S, [3, 6, 29, 31], '#b9b3a2', '#968f7d');
  shadow(S, R, H + 8);
  box(S, R, 0, H, mat, (side, len) => {
    if (side === 'L') {
      // A roller door, half up, and a window.
      rr(g, '#2a2f3e', 2, -11.4, 10, 11.4);
      rr(g, grad(g, 0, -11, 0, 0, '#e3e8f0', '#aab3c2'), 2.6, -11, 8.8, 7);
      for (let y = -10; y < -4; y += 1.5) line(g, 'rgba(60,70,90,.4)', 0.6, 2.6, y, 11.4, y);
      rr(g, '#1a1d28', 2.6, -4, 8.8, 4);
      rr(g, 'rgba(255,255,255,.2)', 2.6, -4, 8.8, 0.6);
      win(S, 13.5, 5, 4.4, 4.6, {});
      rr(g, '#f0c040', 17, -3.4, 1, 3.4);
    } else win(S, 4, 4.4, 4, 4.2, {});
  });
  // A lean-to roof of corrugated sheet.
  if (S.day) {
    slope(g, '#8493ab', [[5, 10, 22], [27, 10, 22]], [[5, 27, 11], [27, 27, 11]], 9, 2.4, 0.04);
    for (let k = 0; k < 11; k++) {
      const u = 5 + k * 2;
      line(g, 'rgba(255,255,255,.2)', 0.6, ...P(u, 10, 22), ...P(u, 27, 11));
    }
    fp3(g, dark('#8493ab', 0.4), [27, 10, 22], [27, 27, 11], [27, 27, 9.4], [27, 10, 20.4]);
    fp3(g, dark('#9fb0c4', 0.3), [26, 25, 14], [26, 10, 14], [26, 10, 21.4], [26, 25, 11.6]);
  }
  brickChimney(S, 10, 12, 18, 27, 2.6, true);
  // Oil drums, a stack of tyres and a pallet of boxes in the yard.
  const [dx, dy] = P(26, 13);
  drum(g, dx, dy, '#d0622f');
  drum(g, dx + 6, dy + 2, '#2f6f9a');
  drum(g, dx + 3, dy + 5, '#7a8a52');
  const [tx, ty] = P(10, 28);
  for (let k = 0; k < 3; k++) {
    ell(g, '#22242c', tx, ty - k * 3, 4.2, 1.7);
    ell(g, '#3b3e4a', tx, ty - k * 3 - 0.5, 4.2, 1.7);
    ell(g, '#14151a', tx, ty - k * 3 - 0.5, 2, 0.8);
  }
  const P2 = [17, 27, 23, 30.5];
  box(S, P2, 0, 1.8, { k: 'siding', c: '#a8794a' });
  box(S, [18, 27.4, 22, 30], 1.8, 6.2, { k: 'siding', c: '#c89860' });
  topFace(S, [18, 27.4, 22, 30], 6.2, '#dcb078');
  if (S.v === 1) lorry(S, 8, 18, false, 3);
  picket(S, 3, 31, 29, 31, 6, '#7a8090');
}

function workTwo(S, c) {
  const g = S.g;
  const mat = { k: 'brick', c: BRICKS[S.pal % 4] };
  const R = [4, 6, 28, 26];
  const H = 22;
  paving(S, [2, 26, 30, 32], '#b9b3a2', '#968f7d');
  shadow(S, R, H + 34);
  box(S, R, 0, H, mat, (side, len) => {
    if (side === 'L') {
      // A big loading door and tall factory windows.
      rr(g, '#2a2f3e', 2, -13.4, 9.4, 13.4);
      rr(g, grad(g, 0, -13, 0, 0, '#cdd3de', '#8791a6'), 2.6, -13, 8.2, 12.6);
      for (let y = -12; y < -1; y += 2) line(g, 'rgba(40,50,70,.4)', 0.7, 2.6, y, 10.8, y);
      for (let k = 0; k < 3; k++) win(S, 11.4 + k * 3.8, 5, 2.8, 9, { frame: '#46506a', lit: 80 });
      for (let k = 0; k < 4; k++) win(S, 3 + k * 5.6, 15.5, 3.6, 4.4, { frame: '#46506a', lit: 80 });
      sign(S, 2, 18.6, len - 4, 2.4, '#e08a2b', { name: false });
    } else {
      for (let k = 0; k < 4; k++) {
        win(S, 3 + k * 4.6, 6, 3, 8, { frame: '#46506a', lit: 80 });
      }
      for (let k = 0; k < 4; k++) win(S, 3 + k * 4.6, 16.4, 3, 3.6, { frame: '#46506a', lit: 80 });
    }
    ledge(S, 0, 14.6, len, '#8a8a8a');
  });
  // A sawtooth roof of four teeth, each rising towards the back, with glass.
  if (S.day) {
    const teeth = 4;
    const tw = 24 / teeth;
    for (let k = 0; k < teeth; k++) {
      const ua = 4 + k * tw;
      const ub = ua + tw;
      slope(g, k % 2 ? '#7a879f' : '#6f7c95', [[ua, 6, H + 10], [ub, 6, H + 10]], [[ua, 26, H], [ub, 26, H]], 8, 2.4);
      fp3(g, 'rgba(160,215,240,.85)', [ua + 0.5, 21, H + 2.4], [ub - 0.5, 21, H + 2.4], [ub - 0.5, 15, H + 5.8], [ua + 0.5, 15, H + 5.8]);
    }
    fp3(g, dark('#7a879f', 0.4), [28, 6, H + 10], [28, 26, H], [28, 26, H - 1.8], [28, 6, H + 8.2]);
  }
  brickChimney(S, 6, 8, H + 7, H + 44, 4.6, true);
  box(S, [19, 27, 27, 30], 0, 6, { k: 'steel', c: '#d6dbe3' });
  topFace(S, [19, 27, 27, 30], 6, '#e8ecf2');
  lorry(S, 12, 29, true, S.pal + 2);
  const [ox, oy] = P(2, 6);
  rr(g, '#8f96a8', ox - 1, oy - 14, 2, 14);
}

function silo(S, u, v, r, z0, z1, color = '#c9ced9') {
  if (!S.day) return;
  const g = S.g;
  const [x, y] = P(u, v);
  const rx = r * 1.414;
  const ry = r * 0.707;
  ell(g, 'rgba(25,25,35,.3)', x + 6, y + 2, rx * 1.3, ry * 1.1);
  g.fillStyle = grad(g, x - rx, 0, x + rx, 0, lite(color, 0.28), color, dark(color, 0.3));
  g.beginPath();
  g.moveTo(x - rx, y - z0);
  g.lineTo(x - rx, y - z1);
  g.lineTo(x + rx, y - z1);
  g.lineTo(x + rx, y - z0);
  g.ellipse(x, y - z0, rx, ry, 0, 0, Math.PI);
  g.closePath();
  g.fill();
  for (let z = z0 + 6; z < z1; z += 6) {
    g.strokeStyle = 'rgba(30,30,45,.25)';
    g.lineWidth = 0.8;
    g.beginPath();
    g.ellipse(x, y - z, rx, ry, 0, 0, Math.PI);
    g.stroke();
  }
  ell(g, lite(color, 0.4), x, y - z1, rx, ry);
  ell(g, color, x, y - z1, rx * 0.8, ry * 0.8);
  ell(g, dark(color, 0.2), x, y - z1 - 4, rx * 0.5, ry * 0.5);
  rr(g, '#8f96a8', x - 0.5, y - z1 - 9, 1, 6);
}

function workThree(S, c) {
  const g = S.g;
  const mat = { k: 'brick', c: BRICKS[(S.pal + 2) % 4] };
  const R = [3, 8, 30, 29];
  const H = 28;
  paving(S, [1, 29, 31, 32], '#b9b3a2', '#968f7d');
  shadow(S, R, H + 40);
  // Silos and a tank, behind.
  silo(S, 6, 5, 3.4, 2, 34);
  silo(S, 13, 4, 3.4, 2, 30, '#d3d7e0');
  box(S, R, 0, H, mat, (side, len) => {
    if (side === 'L') {
      rr(g, '#2a2f3e', 3, -15.4, 12, 15.4);
      rr(g, grad(g, 0, -15, 0, 0, '#cdd3de', '#8791a6'), 3.6, -15, 10.8, 14.6);
      for (let y = -14; y < -1; y += 2) line(g, 'rgba(40,50,70,.4)', 0.7, 3.6, y, 14.4, y);
      for (let k = 0; k < 3; k++) win(S, 17 + k * 4, 4, 2.8, 10, { frame: '#46506a', lit: 85 });
      for (let k = 0; k < 6; k++) win(S, 2.6 + k * 4.2, 18.6, 3, 6, { frame: '#46506a', lit: 85 });
      sign(S, 2, 26, len - 4, 1.6, '#e08a2b', { name: false });
    } else {
      for (let k = 0; k < 5; k++) win(S, 2 + k * 4, 6, 2.8, 9, { frame: '#46506a', lit: 85 });
      for (let k = 0; k < 5; k++) win(S, 2 + k * 4, 18.6, 2.8, 6, { frame: '#46506a', lit: 85 });
    }
    ledge(S, 0, 16.4, len, '#8a8a8a');
  });
  if (S.day) {
    // Roof vents and a sawtooth strip along the hall.
    flatRoof(S, R, H, mat, { coping: '#8a8f9c', parapet: 2 });
    const zt = H + 2;
    for (const [u, v] of [[8, 14], [14, 18], [20, 22], [24, 12]]) {
      const V = [u, v, u + 3.4, v + 3.4];
      box(S, V, zt, zt + 4, { k: 'steel', c: '#aab3c2' });
      topFace(S, V, zt + 4, '#d3dae6');
    }
  }
  brickChimney(S, 22, 10, H + 2, H + 52, 4.4, true);
  brickChimney(S, 27, 12, H + 2, H + 40, 3.6, true);
  // Pipework and a gantry down the right wall.
  if (S.day) {
    onR(g, 30, 29, () => {
      rr(g, '#c9ced9', 2, -22, 18, 2.2);
      rr(g, 'rgba(36,26,70,.34)', 2, -22, 18, 2.2);
      for (let x = 4; x < 18; x += 5) rr(g, '#7a8090', x, -22, 1.2, 14);
    });
  }
  lorry(S, 9, 30, true, S.pal + 3);
  lorry(S, 20, 30.5, true, S.pal + 5);
}

// The buildings people place by hand (W54). Each stands on one tile, and none looks like the old
// city's, so a person can find theirs on the map.
function placedHouse(S, c) {
  const R = [7, 9, 25, 23];
  const H = 11;
  const mat = S.v === 1 ? { k: 'brick', c: BRICKS[S.pal % 4] } : { k: 'siding', c: c.wall };
  shadow(S, R, H + 9);
  box(S, R, 0, H, mat, (side, len) => {
    win(S, 3, 4, 4, 5, { curtain: '#f1e2c0' });
    if (side === S.door) door(S, len - 8, 4.4, 8, c.door, {});
    else win(S, len - 7, 4, 4, 5, {});
  });
  gable(S, R, H, 9, c.roof, 1.8, 'u', mat);
  brickChimney(S, 18, 12, 17, 24, 3.4);
  picket(S, 4, 29, 11, 29);
}

function placedCafe(S, c) {
  const R = [6, 8, 26, 24];
  const H = 13;
  const mat = { k: 'siding', c: '#e8d9b8' };
  paving(S, [4, 6, 28, 30], '#d6ccb1', '#b3a887');
  shadow(S, R, H + 2);
  box(S, R, 0, H, mat, (side, len) => {
    if (side === S.door) {
      door(S, len / 2 - 2.2, 4.4, 8.5, c.door, {});
      win(S, 2.5, 4, 5, 6, {});
      win(S, len - 7.5, 4, 5, 6, {});
    } else win(S, 4, 4, len - 8, 6, { curtain: '#f1e2c0' });
  });
  flatRoof(S, R, H, mat);
  awning(S, R, H - 3, 5, c.awn);
}

function placedClub(S, c) {
  const R = [6, 7, 26, 25];
  const H = 17;
  const mat = { k: 'brick', c: '#5a4668' };
  shadow(S, R, H + 2);
  box(S, R, 0, H, mat, (side, len) => {
    if (side === S.door) door(S, len / 2 - 2.2, 4.4, 8.5, '#2a2436', {});
    sign(S, 3, 11.4, len - 6, 3, c.awn[0]);
  });
  flatRoof(S, R, H, mat);
}

function placedStudio(S, c) {
  const R = [6, 8, 26, 24];
  const H = 15;
  const mat = { k: 'brick', c: BRICKS[S.pal % 4] };
  shadow(S, R, H + 6);
  box(S, R, 0, H, mat, (side, len) => {
    if (side === S.door) door(S, 3, 4.4, 8.5, c.door, {});
    const lay = layout(len, 4.4, 3.4, 2.2);
    for (let k = side === S.door ? 1 : 0; k < lay.count; k++) win(S, lay.start + k * lay.step, 5, 4.4, 7, {});
  });
  flatRoof(S, R, H, mat);
  // Roof lights, for the people working underneath.
  if (S.day) {
    for (let k = 0; k < 3; k++) {
      const ua = 7 + k * 6.3;
      slope(S.g, '#8fb7d0', [[ua, 9, 21], [ua + 5.6, 9, 21]], [[ua, 23, 15], [ua + 5.6, 23, 15]], 6, 2.4);
    }
  }
}

// W91: a quarter of the town square. Each piece is paved right up to the sides it shares with the
// others, so the four read as one plaza, with the fountain on the first and benches round the rest.
function squarePiece(corner) {
  return (S) => {
    const right = corner === 1 || corner === 3;
    const down = corner === 2 || corner === 3;
    const R = [right ? 0 : 1, down ? 0 : 1, right ? 31 : 32, down ? 31 : 32];
    paving(S, R, '#d6ccb1', '#b3a887');
    if (!S.day) return;
    const inner = [right ? 0 : 12, down ? 0 : 12, right ? 20 : 32, down ? 20 : 32];
    paving(S, inner, '#c7bc9f', '#a79c7e');
    if (corner === 0) fountain(S.g);
    else if (corner === 1) bench(S.g, 16, 4);
    else if (corner === 2) bench(S.g, 4, 16, 'v');
    else {
      bench(S.g, 16, 28);
      bench(S.g, 28, 16, 'v');
    }
  };
}

// W90: a lounge: low and wide, warm walls, long curtained windows and plants on the roof.
function placedLounge(S, c) {
  const R = [5, 8, 27, 24];
  const H = 11;
  const mat = { k: 'siding', c: '#e6bcae' };
  shadow(S, R, H + 2);
  box(S, R, 0, H, mat, (side, len) => {
    if (side === S.door) {
      door(S, len / 2 - 2.2, 4.4, 8, c.door, {});
      win(S, 2.5, 3.5, len / 2 - 6, 5.5, { curtain: '#b45a6a', lit: 90 });
      win(S, len / 2 + 3, 3.5, len / 2 - 6, 5.5, { curtain: '#b45a6a', lit: 90 });
    } else win(S, 3, 3.5, len - 6, 5.5, { curtain: '#b45a6a', lit: 90 });
  });
  flatRoof(S, R, H, mat);
  if (S.day) {
    for (const [u, v] of [[9, 12], [22, 12], [15, 20]]) {
      const [x, y] = P(u, v, H);
      drawTree(S.g, x, y, 'bush', u + v);
    }
  }
}

// W90: a bar: dark brick, a bright sign over the door, and every window lit at night.
function placedBar(S, c) {
  const R = [6, 8, 26, 24];
  const H = 13;
  const mat = { k: 'brick', c: '#4a3a3a' };
  shadow(S, R, H + 2);
  box(S, R, 0, H, mat, (side, len) => {
    if (side === S.door) {
      door(S, len / 2 - 2.2, 4.4, 8.5, '#1e1a22', {});
      sign(S, 3, 10, len - 6, 2.4, c.awn[0]);
    }
    win(S, 2.5, 4, 3.6, 4.4, { lit: 100 });
    win(S, len - 6.1, 4, 3.6, 4.4, { lit: 100 });
  });
  flatRoof(S, R, H, mat);
}

// W90: an office: tall, with rows of windows and a flat roof.
function placedOffice(S, c) {
  const R = [7, 8, 25, 24];
  const H = 30;
  const mat = { k: 'concrete', c: '#c9ccd2' };
  paving(S, [5, 6, 27, 26], '#d1d4d9', '#aeb2b9');
  shadow(S, R, H + 4);
  box(S, R, 0, H, mat, (side, len) => {
    if (side === S.door) door(S, len / 2 - 2.6, 4.4, 9, '#3a5a7a', {});
    for (let row = 0; row < 3; row++) {
      if (row === 0 && side === S.door) continue;
      const lay = layout(len, 3.6, 2.2, 1.8);
      for (let k = 0; k < lay.count; k++) win(S, lay.start + k * lay.step, 3.5 + row * 8.5, 3.6, 5.5, { frame: '#5a6a80', lit: 55 });
    }
    ledge(S, 0, 11.5, len, '#a9adb5');
    ledge(S, 0, 20, len, '#a9adb5');
  });
  flatRoof(S, R, H, mat, { coping: '#9aa0aa', parapet: 1.5 });
}

// W90: a workshop: a wide brick shed with a big door and a chimney.
function placedWorkshop(S, c) {
  const R = [5, 8, 27, 25];
  const H = 14;
  const mat = { k: 'brick', c: BRICKS[(S.pal + 1) % 4] };
  paving(S, [3, 6, 29, 29], '#c9c2b0', '#a59d88');
  shadow(S, R, H + 8);
  box(S, R, 0, H, mat, (side, len) => {
    if (side === S.door) {
      rr(S.g, '#5b4a3a', 3, -10, len / 2, 10);
      for (let y = -9; y < 0; y += 2) line(S.g, 'rgba(30,20,10,.35)', 0.6, 3, y, 3 + len / 2, y);
      door(S, len - 7, 4.4, 8, c.door, {});
    } else {
      for (let k = 0; k < 3; k++) win(S, 3 + k * 6, 6, 4, 5, { frame: '#4a4a4a', lit: 70 });
    }
  });
  gable(S, R, H, 6, '#6a6f78', 1.2, 'u', mat);
  brickChimney(S, 22, 11, H + 2, H + 16, 3, true);
}

// W90: a library: pale stone, tall windows, a pitched roof and steps up to the door.
function placedLibrary(S, c) {
  const R = [6, 8, 26, 24];
  const H = 16;
  const mat = { k: 'siding', c: '#ece4cf' };
  paving(S, [4, 6, 28, 28], '#dcd5c2', '#b9b19c');
  shadow(S, R, H + 8);
  box(S, R, 0, H, mat, (side, len) => {
    if (side === S.door) door(S, len / 2 - 2.4, 4.4, 10, '#5a3a24', {});
    const lay = layout(len, 2.8, 2.6, 2);
    for (let k = 0; k < lay.count; k++) {
      const u = lay.start + k * lay.step;
      if (side === S.door && Math.abs(u - len / 2) < 4) continue;
      win(S, u, 4, 2.8, 9, { frame: '#7a6a50', lit: 75 });
    }
  });
  gable(S, R, H, 7, '#5f6f7f', 1.4, 'u', mat);
}

const BUILDERS = {
  [K_HOMES]: [null, homeOne, homeTwo, homeThree],
  [K_SHOPS]: [null, shopOne, shopTwo, shopThree],
  [K_WORKS]: [null, workOne, workTwo, workThree],
  [K_HOUSE]: [placedHouse],
  [K_CAFE]: [placedCafe],
  [K_CLUB]: [placedClub],
  [K_STUDIO]: [placedStudio],
  [K_SQUARE]: [squarePiece(0), squarePiece(1), squarePiece(2), squarePiece(3)],
  [K_LOUNGE]: [placedLounge],
  [K_BAR]: [placedBar],
  [K_OFFICE]: [placedOffice],
  [K_WORKSHOP]: [placedWorkshop],
  [K_LIBRARY]: [placedLibrary],
};

// A building as a sprite, with its lit-windows twin, its shadow, and the places smoke rises.
function buildingObject(kind, level, variant, door, pal) {
  const key = `b:${kind}:${level}:${variant}:${door}:${pal}`;
  let s = sprites.get(`${LOD}|${key}`);
  if (s) return s;
  const build = BUILDERS[kind][level];
  const colors = {
    wall: SIDING[pal],
    roof: ROOFS[(pal + variant + 1) % 6],
    door: DOORC[(pal + variant * 2) % 6],
    awn: AWN[(pal + variant) % 6],
    hue: pal * 7 + variant,
  };
  const info = { smoke: [], flag: null, shadows: [] };
  const run = (day) => (g) => {
    if (day) {
      info.smoke.length = 0;
      info.shadows.length = 0;
    }
    build({ g, day, v: variant, pal, door, seed: ih(kind, level, variant * 11 + pal), n: 0, info: day ? info : null }, colors);
  };
  s = object(key, run(true));
  s.info = info;
  let lit = null;
  let shade = null;
  s.lit = () => lit || (lit = object(`${key}:lit`, (g) => {
    // A warm pool of light by the door, then the lit windows.
    const [px, py] = P(16, 22);
    const pool = g.createRadialGradient(px, py, 2, px, py, 34);
    pool.addColorStop(0, 'rgba(255,196,110,.3)');
    pool.addColorStop(1, 'rgba(255,196,110,0)');
    g.fillStyle = pool;
    g.fillRect(px - 36, py - 20, 72, 40);
    run(false)(g);
  }));
  // The shadow, cast as one shape so overlapping parts don't double up.
  s.shadow = () => shade || (shade = sprite(`${key}:sh`, 190, 150, 70, 40, (g) => {
    g.beginPath();
    for (const [R, H] of info.shadows) {
      const throwLen = Math.min(H * 0.85, 30);
      const du = throwLen;
      const dv = throwLen * 0.3;
      const [u0, v0, u1, v1] = R;
      const pts = [[u0, v0], [u1, v0], [u1, v1], [u0, v1], [u0 + du, v0 + dv], [u1 + du, v0 + dv], [u1 + du, v1 + dv], [u0 + du, v1 + dv]].map((p) => P(p[0], p[1]));
      const hull = convexHull(pts);
      g.moveTo(hull[0][0], hull[0][1]);
      for (let i = 1; i < hull.length; i++) g.lineTo(hull[i][0], hull[i][1]);
      g.closePath();
    }
    g.fillStyle = 'rgba(38,28,66,.3)';
    g.fill();
  }, true));
  return s;
}

function convexHull(points) {
  const pts = points.slice().sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const cross = (o, a, b) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
  const lower = [];
  for (const p of pts) {
    while (lower.length >= 2 && cross(lower[lower.length - 2], lower[lower.length - 1], p) <= 0) lower.pop();
    lower.push(p);
  }
  const upper = [];
  for (let i = pts.length - 1; i >= 0; i--) {
    const p = pts[i];
    while (upper.length >= 2 && cross(upper[upper.length - 2], upper[upper.length - 1], p) <= 0) upper.pop();
    upper.push(p);
  }
  upper.pop();
  lower.pop();
  return lower.concat(upper);
}
// @@ART4

// ---------------------------------------------------------------------------------------------
// Reading the server's land

// Rows come run-length encoded: a count then a letter, such as 40g3f10w. Anything odd becomes
// water, which nobody can build on or walk over, so bad data can't do harm.
function decodeLand(rows, n) {
  const out = new Uint8Array(n * n).fill(WATER_L);
  for (let y = 0; y < n && y < rows.length; y++) {
    const re = /(\d*)([gfwsh])/g;
    let x = 0;
    let m;
    while ((m = re.exec(rows[y])) !== null && x < n) {
      const count = Math.min(m[1] === '' ? 1 : parseInt(m[1], 10) || 0, n - x);
      out.fill(LAND[m[2]], y * n + x, y * n + x + count);
      x += count;
    }
  }
  return out;
}

export function seenText(age) {
  if (!Number.isFinite(age) || age < 90) return 'seen just now';
  const minutes = Math.round(age / 60);
  return minutes < 2 ? 'seen a minute ago' : `seen ${minutes} minutes ago`;
}

// ---------------------------------------------------------------------------------------------
// Small characters

function rrect(g, x, y, w, h, r) {
  g.beginPath();
  g.moveTo(x + r, y);
  g.arcTo(x + w, y, x + w, y + h, r);
  g.arcTo(x + w, y + h, x, y + h, r);
  g.arcTo(x, y + h, x, y, r);
  g.arcTo(x, y, x + w, y, r);
  g.closePath();
}
function rfill(g, fill, x, y, w, h, r, stroke) {
  rrect(g, x, y, w, h, r);
  g.fillStyle = fill;
  g.fill();
  if (stroke) {
    g.strokeStyle = stroke;
    g.lineWidth = 0.8;
    g.stroke();
  }
}
function radial(g, x, y, r, c0, c1, c2) {
  const gr = g.createRadialGradient(x - r * 0.35, y - r * 0.4, r * 0.1, x, y, r);
  gr.addColorStop(0, c0);
  gr.addColorStop(c2 ? 0.55 : 1, c1);
  if (c2) gr.addColorStop(1, c2);
  return gr;
}

const INK = 'rgba(28,22,48,.6)';

// --- Players: little robots with a pale shell and a dark visor ----------------------------------

const BOT_COLORS = ['#ff5b5b', '#ff9a3c', '#ffcb2e', '#5bd16d', '#2fcfa0', '#4aa3f5', '#6f86f7', '#c96ef0', '#f56fa6', '#9ed42f', '#22b8d4', '#ff7d7d'];

// A robot's shell is the colour its name starts with, if it starts with one (Purple Panda is
// purple). Otherwise it is picked from its hash.
function botColour(name, h) {
  const first = String(name).split(/\s+/)[0].toLowerCase();
  return COLOUR_WORDS[first] || BOT_COLORS[h % BOT_COLORS.length];
}

// What a figure wears, in the same words as a room's (docs/looks.md). The server says what anyone with
// an account has on, so they're the same someone here and indoors (C14, C16). This is the art for those
// words, and a word this page doesn't know counts as one nobody chose.
const BOT_HEADS = ['antenna', 'twin', 'wide'];
const BOT_HATS = {
  none: null,
  cap: (g, cx, hy, hw, color) => {
    rfill(g, grad(g, cx - hw / 2, 0, cx + hw / 2, 0, lite(color, 0.25), color, dark(color, 0.25)), cx - hw * 0.46, hy - 3.2, hw * 0.92, 5.6, 2.6, INK);
    rfill(g, dark(color, 0.3), cx - hw * 0.46, hy + 1.6, hw * 0.92 + 3, 1.5, 0.7);
  },
  beanie: (g, cx, hy, hw, color) => {
    ell(g, '#f4f1e8', cx, hy - 5, 1.8, 1.8);
    rfill(g, grad(g, cx - hw / 2, 0, cx + hw / 2, 0, lite(color, 0.25), color, dark(color, 0.25)), cx - hw * 0.46, hy - 4, hw * 0.92, 6.5, 3, INK);
    rfill(g, dark(color, 0.25), cx - hw * 0.5, hy + 0.4, hw, 2.6, 1);
  },
  top_hat: (g, cx, hy, hw) => {
    rfill(g, grad(g, cx - hw / 2, 0, cx + hw / 2, 0, '#3a3a4c', '#22222e', '#14141c'), cx - hw * 0.38, hy - 8.4, hw * 0.76, 9.4, 1, INK);
    rr(g, '#b23a48', cx - hw * 0.38, hy - 2.2, hw * 0.76, 1.6);
    rfill(g, '#22222e', cx - hw * 0.56, hy - 0.2, hw * 1.12, 1.8, 0.8);
  },
  witch: (g, cx, hy, hw) => {
    fp(g, '#2b2140', [[cx - hw * 0.4, hy + 0.4], [cx + 1, hy - 9], [cx + 4.4, hy - 11.4], [cx + 2.8, hy - 8], [cx + hw * 0.4, hy + 0.4]]);
    rr(g, '#e8a02e', cx - hw * 0.36, hy - 1.6, hw * 0.72, 1.5);
    ell(g, '#2b2140', cx, hy + 0.6, hw * 0.7, 1.5);
  },
  crown: (g, cx, hy, hw) => {
    const w = hw * 0.4;
    fp(g, '#f0c23a', [[cx - w, hy + 1], [cx - w - 0.6, hy - 4.4], [cx - w / 2, hy - 1.4], [cx, hy - 5.2], [cx + w / 2, hy - 1.4], [cx + w + 0.6, hy - 4.4], [cx + w, hy + 1]]);
    rr(g, '#c8921e', cx - w, hy, w * 2, 1);
    ell(g, '#d6453f', cx, hy - 1, 0.9, 0.9);
  },
  party: (g, cx, hy, hw) => {
    fp(g, '#e96fa4', [[cx - hw * 0.34, hy + 0.8], [cx, hy - 9.4], [cx + hw * 0.34, hy + 0.8]]);
    fp(g, '#ffd24d', [[cx - hw * 0.2, hy - 2.4], [cx + hw * 0.2, hy - 2.4], [cx + hw * 0.12, hy - 4.6], [cx - hw * 0.12, hy - 4.6]]);
    ell(g, '#f4f1e8', cx, hy - 9.6, 1.5, 1.5);
  },
};
// On the dark plate on a bot's chest. Lights are what it has with nothing chosen.
const BOT_CHESTS = {
  plain: (g, by, color) => {
    ell(g, lite(color, 0.7), -1, by + 4.8, 0.9, 0.9);
    ell(g, '#8be0ff', 1.2, by + 4.8, 0.9, 0.9);
  },
  heart: (g, by) => {
    ell(g, '#ff5d7a', -0.9, by + 4.2, 1, 1);
    ell(g, '#ff5d7a', 0.9, by + 4.2, 1, 1);
    fp(g, '#ff5d7a', [[-1.8, by + 4.5], [1.8, by + 4.5], [0, by + 6.3]]);
  },
  star: (g, by) => {
    const points = [];
    for (let i = 0; i < 10; i++) {
      const a = -Math.PI / 2 + (i * Math.PI) / 5;
      const r = i % 2 ? 0.8 : 1.9;
      points.push([Math.cos(a) * r, by + 4.8 + Math.sin(a) * r]);
    }
    fp(g, '#ffd24d', points);
  },
  bolt: (g, by) => {
    fp(g, '#8be0ff', [[0.6, by + 3.2], [-1.3, by + 5], [-0.1, by + 5], [-0.6, by + 6.4], [1.3, by + 4.6], [0.1, by + 4.6]]);
  },
};

// A person's head. `crown` is the top of their hair, which a hat covers, `sides` is what hangs beside
// the face, and `back` is all of it from behind.
const SHORT_CROWN = (g, hair) => fp(g, hair, [[-3.9, -23], [-3, -26.4], [0, -27], [3, -26.4], [3.9, -23], [2, -24.6], [-2, -24.6]]);
const SHORT_BACK = (g, hair) => ell(g, hair, 0, -24.2, 4.2, 3.1);
const PERSON_STYLES = {
  short: { crown: SHORT_CROWN, sides: () => {}, back: SHORT_BACK },
  long: {
    crown: SHORT_CROWN,
    sides: (g, hair) => {
      rfill(g, hair, -4.7, -24.4, 1.7, 8.4, 0.8);
      rfill(g, hair, 3, -24.4, 1.7, 8.4, 0.8);
    },
    back: (g, hair) => {
      SHORT_BACK(g, hair);
      rfill(g, hair, -4.3, -24.6, 8.6, 9.4, 2.4);
    },
  },
  curly: {
    crown: (g, hair) => ell(g, hair, 0, -25.6, 4.9, 2.8),
    sides: (g, hair) => {
      ell(g, hair, -4, -23.6, 1.6, 1.8);
      ell(g, hair, 4, -23.6, 1.6, 1.8);
    },
    back: (g, hair) => ell(g, hair, 0, -23.8, 5, 4.2),
  },
  bun: {
    crown: (g, hair) => {
      ell(g, hair, 0, -28, 1.9, 1.8);
      SHORT_CROWN(g, hair);
    },
    sides: () => {},
    back: (g, hair) => {
      ell(g, hair, 0, -28, 1.9, 1.8);
      SHORT_BACK(g, hair);
    },
  },
  mohawk: {
    crown: (g, hair) => rfill(g, hair, -0.9, -29.6, 1.8, 4.6, 0.8),
    sides: () => {},
    back: (g, hair) => rfill(g, hair, -0.9, -29.6, 1.8, 9.4, 0.8),
  },
  bald: { crown: () => {}, sides: () => {}, back: () => {} },
};
const PERSON_FACES = {
  none: null,
  glasses: (g) => {
    g.strokeStyle = '#20242f';
    g.lineWidth = 0.55;
    for (const x of [-1.6, 1.6]) {
      g.beginPath();
      g.ellipse(x, -22.2, 1.25, 1.2, 0, 0, 6.2832);
      g.stroke();
    }
  },
  shades: (g) => {
    rfill(g, '#15161c', -3.3, -23.1, 6.6, 1.9, 0.8);
    rr(g, '#6d7890', -2.5, -22.8, 1, 0.5);
  },
  moustache: (g, hair) => rfill(g, hair, -1.8, -21, 3.6, 0.9, 0.4),
  beard: (g, hair) => fp(g, hair, [[-3.8, -22], [-2.8, -19.2], [0, -18], [2.8, -19.2], [3.8, -22], [1.9, -20.8], [-1.9, -20.8]]),
};

// A cap is a person's own colour and sits low. Any other hat is a bot's, scaled to a smaller head.
function personHat(g, hat, color) {
  if (hat === 'cap') {
    ell(g, color, 0, -25.4, 4.2, 2.1);
    rr(g, dark(color, 0.2), -4.4, -24.6, 8.8, 1.1);
  } else if (BOT_HATS[hat]) {
    g.save();
    g.translate(0, -26.4);
    g.scale(0.72, 0.72);
    BOT_HATS[hat](g, 0, 0, 12, color);
    g.restore();
  }
}

// What a person's row says they have on, over what a generated name gives anyone else.
function clothes(worn) {
  const on = {};
  for (const [part, table] of [['shirt', FABRICS], ['skin', SKINS], ['hair', HAIRS], ['trousers', LEGS]]) {
    if (Object.hasOwn(table, worn[part])) on[part] = table[worn[part]];
  }
  for (const [part, table] of [['style', PERSON_STYLES], ['face', PERSON_FACES], ['hat', BOT_HATS]]) {
    if (Object.hasOwn(table, worn[part])) on[part] = worn[part];
  }
  return on;
}

// How a figure looks, from its name and what it is, and from what the server says it's wearing if it
// has an account behind it. A person is drawn as a human, an agent as a robot. `kind` is null where
// the map doesn't say (the private city, where everyone is a bot).
function dress(p, name, kind, worn = {}) {
  p.name = name;
  p.kind = kind === 'person' ? 'person' : 'agent';
  p.human = p.kind === 'person' ? { ...visitorLook(name), ...clothes(worn) } : null;
  p.color = Object.hasOwn(FABRICS, worn.shell) ? FABRICS[worn.shell]
    : kind ? botColour(name, p.h) : BOT_COLORS[p.h % BOT_COLORS.length];
  p.look = BOT_HEADS.includes(worn.head) ? BOT_HEADS.indexOf(worn.head) : (p.h >>> 3) % 3;
  p.eyes = Object.hasOwn(EYES, worn.eyes) ? EYES[worn.eyes] : null;
  p.chest = Object.hasOwn(BOT_CHESTS, worn.chest) ? worn.chest : 'plain';
  p.hat = Object.hasOwn(BOT_HATS, worn.hat) ? worn.hat : 'none';
  p.worn = JSON.stringify(worn);
  p.known = Boolean(kind);
}

// What a row of a view says this figure wears, if anything.
const wornIn = (value) => (value && typeof value === 'object' && !Array.isArray(value) ? value : {});

function makePlayer(id, name, x, y, kind, worn) {
  const h = hash(id);
  const ox = ((ih(h, 1, 1) % 100) / 100 - 0.5) * 0.4;
  const oy = ((ih(h, 2, 2) % 100) / 100 - 0.5) * 0.4;
  const p = {
    type: 'player',
    id,
    h,
    name,
    x,
    y,
    px: x + 0.5 + ox,
    py: y + 0.5 + oy,
    ox,
    oy,
    path: [],
    speed: 2.3 + (h % 7) * 0.12,
    face: ih(h, 4, 4) & 1 ? 'fr' : 'fl',
    walking: false,
    emote: null,
    emoteAt: 0,
    leaving: 0,
    born: -10,
    phase: (h % 1000) / 1000,
    sx: undefined,
    sy: undefined,
  };
  dress(p, name, kind, worn);
  return p;
}

function drawBot(g, o) {
  const { color, look, eyes, chest, hat, back, step, arms, shut, sitting, tilt } = o;
  const c0 = lite(color, 0.34);
  const c2 = dark(color, 0.34);
  const lift = sitting ? 6 : 0;
  const shell = '#f1f4fa';
  const shell2 = '#b6bfd2';
  // Legs and feet.
  if (!sitting) {
    const a = step ? 1.8 : 0;
    for (const [lx, up] of [[-3.3, a], [3.3, 1.8 - a]]) {
      rfill(g, grad(g, lx - 1.6, 0, lx + 1.6, 0, '#58607a', '#2b3045'), lx - 1.6, -7 - up, 3.2, 6.4 + up, 1.2);
      ell(g, '#20243a', lx + 0.4, -1 - up, 2.9, 1.6);
      ell(g, '#454d68', lx - 0.1, -1.6 - up, 1.8, 0.8);
    }
  } else {
    ell(g, '#20243a', -3.6, -1.4, 3, 1.6);
    ell(g, '#20243a', 3.6, -1.4, 3, 1.6);
  }
  const by = -17 + lift;
  // Arms, behind the body.
  const arm = (x, a) => {
    const [dx, dy, len] = a;
    g.save();
    g.translate(x, by + 2.2);
    g.rotate(dx);
    rfill(g, grad(g, -1.4, 0, 1.4, 0, lite(color, 0.1), c2), -1.4, 0, 2.8, len, 1.3, INK);
    ell(g, '#eaeef6', 0, len + 0.6, 1.9, 1.9);
    g.restore();
    void dy;
  };
  const down = [0.12, 0, 8];
  if (arms === 'up') {
    arm(-6.6, [2.7, 0, 8.4]);
    arm(6.6, [-2.7, 0, 8.4]);
  } else if (arms === 'wave1') {
    arm(-6.6, down);
    arm(6.6, [-2.5, 0, 8.4]);
  } else if (arms === 'wave2') {
    arm(-6.6, down);
    arm(6.6, [-1.8, 0, 8.4]);
  } else if (arms === 'dance') {
    arm(-6.6, [2.3, 0, 8.4]);
    arm(6.6, [-0.5, 0, 8]);
  } else if (arms === 'dance2') {
    arm(-6.6, [0.5, 0, 8]);
    arm(6.6, [-2.3, 0, 8.4]);
  } else {
    arm(-6.6, [0.1, 0, 8]);
    arm(6.6, [-0.1, 0, 8]);
  }
  // Body.
  rfill(g, grad(g, -5.5, by, 5.5, by + 11, c0, color, c2), -5.5, by, 11, 11.5, 3, INK);
  rfill(g, 'rgba(255,255,255,.22)', -4.6, by + 0.8, 4, 4.5, 1.6);
  rr(g, c2, -5.2, by + 8.2, 10.4, 1.3);
  if (!back) {
    rfill(g, '#2a3046', -2.6, by + 3, 5.2, 3.6, 1);
    BOT_CHESTS[chest](g, by, color);
  } else {
    for (let k = 0; k < 3; k++) rr(g, c2, -3, by + 3 + k * 1.8, 6, 0.8);
  }
  // Head.
  const hw = look === 2 ? 14 : 12.6;
  const hy = by - 11 + 1.5 + (sitting ? 1 : 0);
  const hx = -hw / 2 + tilt;
  rfill(g, grad(g, hx, hy, hx + hw, hy + 10, '#ffffff', shell, shell2), hx, hy, hw, 10.4, 4, INK);
  rr(g, 'rgba(255,255,255,.65)', hx + 2, hy + 0.8, hw * 0.35, 1);
  if (back) {
    rfill(g, shell2, hx + 2, hy + 2.4, hw - 4, 5, 2);
    line(g, 'rgba(60,70,100,.4)', 0.7, hx + 3, hy + 4, hx + hw - 3, hy + 4);
  } else {
    rfill(g, grad(g, 0, hy + 2, 0, hy + 8, '#3b4260', '#141828'), hx + 1.4, hy + 2.2, hw - 2.8, 6, 2.6);
    rr(g, 'rgba(255,255,255,.2)', hx + 2.6, hy + 2.8, hw - 5.2, 0.9);
    const glow = eyes || lite(color, 0.7);
    if (shut) {
      for (const ex of [hx + hw * 0.3, hx + hw * 0.7]) line(g, glow, 1.1, ex - 1.6, hy + 6.2, ex + 1.6, hy + 6.2);
    } else {
      for (const ex of [hx + hw * 0.3, hx + hw * 0.7]) {
        ell(g, rgba(glow, 0.35), ex, hy + 5.2, 3, 2.6);
        ell(g, glow, ex, hy + 5.2, 1.8, 2);
        ell(g, '#ffffff', ex - 0.5, hy + 4.5, 0.7, 0.7);
      }
    }
  }
  // Antenna or ears, or a hat where they'd be.
  if (BOT_HATS[hat]) {
    BOT_HATS[hat](g, tilt, hy, hw, color);
  } else if (look === 1) {
    for (const ex of [hx + 1.6, hx + hw - 1.6]) {
      rr(g, '#8f98b0', ex - 0.5, hy - 4, 1, 4.4);
      ell(g, color, ex, hy - 4.2, 1.7, 1.7);
      ell(g, '#ffffff', ex - 0.5, hy - 4.8, 0.6, 0.6);
    }
  } else {
    rr(g, '#8f98b0', -0.5 + tilt, hy - 4.6, 1, 5);
    ell(g, radial(g, tilt, hy - 5.4, 2.6, '#ffffff', lite(color, 0.4), color), tilt, hy - 5.4, 2.1, 2.1);
  }
  if (look === 2) {
    rfill(g, c2, hx - 1.6, hy + 3, 2, 4, 1);
    rfill(g, c2, hx + hw - 0.4, hy + 3, 2, 4, 1);
  }
}

function botSprite(o) {
  const key = `bot:${o.color}:${o.look}:${o.eyes}:${o.chest}:${o.hat}:${o.back}:${o.flip}:${o.step}:${o.arms}:${o.shut}:${o.sitting}:${o.tilt}`;
  return sprite(key, 34, 46, 17, 41, (g) => {
    if (o.flip) g.scale(-1, 1);
    drawBot(g, o);
  });
}

// Small pictures that float over a character's head.
function drawEmote(bg, emote, X, Y, t, calm) {
  const bob = calm ? 0 : Math.sin(t * 4) * 1.2;
  const y = Y + bob;
  if (emote === 'sleep') {
    for (let i = 0; i < 3; i++) {
      const rise = calm ? i * 9 : (t * 11 + i * 10) % 30;
      bg.globalAlpha = Math.max(0, 1 - rise / 30);
      const zx = X + 5 + rise * 0.3 + i * 2;
      const zy = Y + 8 - rise * 0.8;
      const s = 4 + i * 1.4;
      bg.fillStyle = '#ffffff';
      bg.strokeStyle = '#4a5578';
      bg.lineWidth = 0.9;
      bg.beginPath();
      bg.moveTo(zx, zy);
      bg.lineTo(zx + s, zy);
      bg.lineTo(zx, zy + s);
      bg.lineTo(zx + s, zy + s);
      bg.stroke();
    }
    bg.globalAlpha = 1;
    return;
  }
  // A speech bubble with a tail.
  const bx = X - 9;
  const by = y - 20;
  bg.fillStyle = 'rgba(25,20,45,.28)';
  rrect(bg, bx + 1, by + 2, 18, 15, 5);
  bg.fill();
  rfill(bg, '#ffffff', bx, by, 18, 15, 5, '#3a4262');
  fp(bg, '#ffffff', [[X - 2.5, by + 14.4], [X + 2.5, by + 14.4], [X, by + 19]]);
  line(bg, '#3a4262', 0.8, X - 2.5, by + 15, X, by + 19);
  line(bg, '#3a4262', 0.8, X + 2.5, by + 15, X, by + 19);
  rr(bg, '#ffffff', X - 2.2, by + 14, 4.4, 1.4);
  const cx = X;
  const cy = by + 7.5;
  if (emote === 'think') {
    const lit = calm ? 3 : Math.floor(t * 2.4) % 4;
    for (let i = 0; i < 3; i++) ell(bg, i < lit ? '#4a5578' : '#c3cadb', cx - 5 + i * 5, cy, 1.5, 1.5);
  } else if (emote === 'wave') {
    const wob = calm ? 0 : Math.sin(t * 9) * 0.35;
    bg.save();
    bg.translate(cx, cy + 3);
    bg.rotate(wob);
    ell(bg, '#ffcf3a', 0, -1, 3.8, 3.6);
    for (let i = 0; i < 4; i++) rfill(bg, '#ffcf3a', -3.7 + i * 2.1, -7, 1.7, 5, 0.8);
    ell(bg, '#ffd96a', -1, -1.5, 1.5, 1.5);
    rfill(bg, '#ffcf3a', 3.2, -3, 1.6, 3.6, 0.8);
    bg.restore();
  } else if (emote === 'cheer') {
    const k = 0.9 + (calm ? 0 : Math.sin(t * 8) * 0.12);
    bg.save();
    bg.translate(cx, cy);
    bg.scale(k, k);
    bg.fillStyle = '#ffc933';
    bg.strokeStyle = '#d98a14';
    bg.lineWidth = 0.7;
    bg.beginPath();
    for (let i = 0; i < 10; i++) {
      const a = -Math.PI / 2 + (i * Math.PI) / 5;
      const r = i % 2 ? 2.4 : 5.8;
      bg.lineTo(Math.cos(a) * r, Math.sin(a) * r);
    }
    bg.closePath();
    bg.fill();
    bg.stroke();
    bg.restore();
  } else if (emote === 'dance') {
    const sw = calm ? 0 : Math.sin(t * 6) * 1;
    bg.fillStyle = '#e84a8c';
    bg.strokeStyle = '#e84a8c';
    bg.lineWidth = 1.5;
    bg.beginPath();
    bg.ellipse(cx - 2 + sw, cy + 3.4, 2.4, 1.8, -0.4, 0, 6.2832);
    bg.fill();
    bg.beginPath();
    bg.moveTo(cx + 0.2 + sw, cy + 3);
    bg.lineTo(cx + 0.2 + sw, cy - 4.4);
    bg.quadraticCurveTo(cx + 3 + sw, cy - 3.6, cx + 3.6 + sw, cy - 1);
    bg.stroke();
  }
}

function drawPlayer(bg, p, t, clock, hovered, calm) {
  const X = p.dx;
  const Y = p.dy;
  const age = clock - p.born;
  let k = 1;
  if (age < 0.5) k = clamp(1 - Math.pow(1 - age / 0.5, 3) + Math.sin((age / 0.5) * Math.PI * 2.5) * 0.1 * (1 - age / 0.5), 0.02, 1.12);
  if (p.leaving) k = Math.max(0.02, 1 - (clock - p.leaving) / 0.5);
  const emote = p.emote;
  let step = 0;
  let bob = 0;
  let arms = 'down';
  let shut = Math.sin(clock * 1.9 + p.phase * 11) > 0.986;
  let sitting = false;
  let tilt = 0;
  let face = p.face;
  if (p.walking && !calm) {
    step = Math.floor(t * 6 + p.phase * 8) % 2;
    bob = step ? 0 : 1;
  }
  const beat = calm ? 0 : t * 3.4 + p.phase * 6;
  if (emote === 'wave') arms = Math.floor(calm ? 0 : t * 3 + p.phase * 4) % 2 ? 'wave2' : 'wave1';
  else if (emote === 'cheer') {
    arms = 'up';
    bob = calm ? 0 : Math.abs(Math.sin(beat * 1.6)) * 3;
  } else if (emote === 'dance') {
    arms = Math.floor(beat * 2) % 2 ? 'dance' : 'dance2';
    bob = calm ? 0 : Math.abs(Math.sin(beat * 2)) * 2;
    if (!calm && Math.floor(beat) % 2) face = face === 'fr' ? 'fl' : face === 'fl' ? 'fr' : face === 'br' ? 'bl' : 'br';
  } else if (emote === 'sleep') {
    sitting = true;
    shut = true;
    tilt = 1;
    if (face === 'bl') face = 'fl';
    else if (face === 'br') face = 'fr';
  } else if (emote === 'think') {
    tilt = calm ? 0 : Math.floor(t * 0.7 + p.phase * 5) % 2 ? 1 : 0;
  }
  const back = face === 'bl' || face === 'br';
  const flip = face === 'bl' || face === 'fl';
  // A person is drawn as a human and an agent as a robot. Both take the same emotes.
  const human = p.kind === 'person';
  const s = human
    ? visitorSprite(p.human, back, flip, step, arms, shut, sitting)
    : botSprite({ color: p.color, look: p.look, eyes: p.eyes, chest: p.chest, hat: p.hat, back, flip, step, arms, shut, sitting, tilt });
  const sway = emote === 'dance' && !calm ? Math.sin(beat * 2) * 1.4 : 0;
  if (hovered) ell(bg, 'rgba(255,255,255,.4)', X, Y, human ? 11 : 13, human ? 4.6 : 5.4);
  ell(bg, 'rgba(24,20,44,.3)', X + 3, Y + 0.6, (human ? 7.4 : 9) - bob * 0.5, human ? 2.8 : 3.4);
  const h = s.h * k;
  bg.drawImage(s.c, X - s.ax + sway, Y - (s.ay + bob) * k, s.w, h);
  if (emote && !p.leaving && k > 0.9) drawEmote(bg, emote, X, Y - (human ? 36 : 42) - bob, t, calm);
  return [X - 14, Y - (human ? 46 : 52), X + 14, Y + 4];
}

// --- People and visitors. A person on the street is someone with the page open, and is drawn as a
// human. A visitor is an agent that read the guide without joining, and is a robot like the rest.

const COLOUR_WORDS = {
  purple: '#8e56c9', red: '#d6453f', blue: '#3a78d4', green: '#3fa35a', orange: '#ec8a2c', yellow: '#e8c22e',
  pink: '#e96fa4', teal: '#25a4a0', brown: '#8a5a3c', grey: '#8a909e', gray: '#8a909e', gold: '#d8a928',
  silver: '#aab2c0', crimson: '#b8283c', violet: '#7b54d6', indigo: '#4a52b8', scarlet: '#d8402c', amber: '#e3992a',
  lilac: '#b393e0', mint: '#58c8a0', coral: '#ee7a62', navy: '#2f4a8a', lime: '#8ac836', cyan: '#2cb4d8', rose: '#e4688c',
};
// Quiet, cool shirts for names that don't start with a colour.
const VISITOR_COLORS = ['#7a62c4', '#4f86b8', '#3f9c94', '#8a6fa8', '#5e78c8', '#6aa0a8', '#9a7ab4', '#4a8a78'];
const SKIN = ['#f1c9a4', '#d9a77c', '#b27a54', '#8a5a3c', '#f4d6b8'];
const HAIR = ['#2b2118', '#6a4326', '#b9822f', '#d8c08a', '#4a2a22', '#8a8f9c'];

function visitorLook(name) {
  const h = hash(name);
  const first = name.split(/\s+/)[0].toLowerCase();
  return {
    shirt: COLOUR_WORDS[first] || VISITOR_COLORS[h % VISITOR_COLORS.length],
    skin: SKIN[(h >>> 3) % SKIN.length],
    hair: HAIR[(h >>> 6) % HAIR.length],
    hat: (h >>> 9) % 3 === 0 ? 'cap' : 'none',
    style: 'short',
    face: 'none',
    trousers: ['#394666', '#4a4a52', '#6a5a46', '#2f4a48'][(h >>> 11) % 4],
    phase: (h % 1000) / 1000,
    h,
  };
}

// Someone with the page open, strolling the street, in what they're wearing if they're signed in.
function makePerson(id, name, x, y, worn) {
  const look = visitorLook(name);
  const ox = ((ih(look.h, 1, 7) % 100) / 100 - 0.5) * 0.4;
  const oy = ((ih(look.h, 2, 7) % 100) / 100 - 0.5) * 0.4;
  const p = {
    type: 'person',
    id,
    h: look.h,
    name,
    x,
    y,
    px: x + 0.5 + ox,
    py: y + 0.5 + oy,
    ox,
    oy,
    path: [],
    speed: 1.5 + (look.h % 5) * 0.1,
    face: ih(look.h, 4, 4) & 1 ? 'fr' : 'fl',
    walking: false,
    emote: null,
    leaving: 0,
    born: -10,
    phase: look.phase,
    wander: 4 + (look.h % 7),
    sx: undefined,
    sy: undefined,
  };
  dress(p, name, 'person', worn);
  return p;
}

// An agent that read the guide without joining: a robot, in the colour its name starts with. A bot
// out at a place comes this way too, in what it's wearing (C14).
function makeVisitor(id, name, x, y, worn) {
  const h = hash(name);
  const ox = ((ih(h, 1, 7) % 100) / 100 - 0.5) * 0.4;
  const oy = ((ih(h, 2, 7) % 100) / 100 - 0.5) * 0.4;
  const p = {
    type: 'visitor',
    id,
    h,
    name,
    x,
    y,
    px: x + 0.5 + ox,
    py: y + 0.5 + oy,
    ox,
    oy,
    path: [],
    speed: 1.5 + (h % 5) * 0.1,
    face: ih(h, 4, 4) & 1 ? 'fr' : 'fl',
    walking: false,
    emote: null,
    leaving: 0,
    born: -10,
    phase: (h % 1000) / 1000,
    wander: 4 + (h % 7),
    sx: undefined,
    sy: undefined,
  };
  dress(p, name, 'agent', worn);
  return p;
}

// `arms` is one of the robot's poses, `shut` closes the eyes, and `sitting` lowers the figure and
// hides the legs.
const HUMAN_ARMS = {
  down: [0, 0],
  up: [2.6, -2.6],
  wave1: [0.1, -2.5],
  wave2: [0.1, -2.1],
  dance: [2.2, -0.4],
  dance2: [0.4, -2.2],
};

function drawPerson(g, o) {
  const { shirt, skin, hair, hat, style, face, trousers, back, step, small, arms = 'down', shut = false, sitting = false } = o;
  const arm = step ? 1 : -1;
  // Legs, or just feet when sitting.
  if (sitting) {
    ell(g, '#2b2420', -2.6, -1.1, 2.4, 1.3);
    ell(g, '#2b2420', 2.6, -1.1, 2.4, 1.3);
    g.save();
    g.translate(0, 4);
  } else {
    for (const [lx, d] of [[-2, step ? 1 : 0], [2, step ? 0 : 1]]) {
      rfill(g, trousers, lx - 1.5, -8 - d, 3, 8 + d - 1, 1);
      ell(g, '#2b2420', lx + 0.4, -0.8 - d, 2.4, 1.3);
    }
  }
  // Arms and torso. An arm hangs from its shoulder, turned by an angle to raise it.
  const [left, right] = HUMAN_ARMS[arms] || HUMAN_ARMS.down;
  const limb = (x, angle, dy) => {
    g.save();
    g.translate(x, -16.6 + dy);
    g.rotate(angle);
    rfill(g, dark(shirt, 0.15), -1.2, -0.4, 2.4, 8, 1.2, INK);
    ell(g, skin, 0, 8, 1.3, 1.3);
    g.restore();
  };
  limb(-4.2, left, 0);
  limb(4.2, right, -arm * 0.4);
  rfill(g, grad(g, -4, -18, 4, -8, lite(shirt, 0.3), shirt, dark(shirt, 0.25)), -4.2, -18, 8.4, 10.4, 2.6, INK);
  if (!small) rr(g, 'rgba(255,255,255,.85)', -1, -15.5, 2, 2.4);
  // Head.
  ell(g, grad(g, -3, -26, 3, -20, lite(skin, 0.2), skin, dark(skin, 0.2)), 0, -22.4, 3.9, 4.2);
  g.strokeStyle = INK;
  g.lineWidth = 0.7;
  g.beginPath();
  g.ellipse(0, -22.4, 3.9, 4.2, 0, 0, 6.2832);
  g.stroke();
  const hairdo = PERSON_STYLES[style];
  if (back) {
    hairdo.back(g, hair);
  } else {
    if (hat === 'none') hairdo.crown(g, hair);
    hairdo.sides(g, hair);
    if (shut) {
      line(g, '#2a2433', 0.6, -2.2, -22.2, -0.8, -22.2);
      line(g, '#2a2433', 0.6, 0.8, -22.2, 2.2, -22.2);
    } else {
      ell(g, '#2a2433', -1.5, -22.2, 0.55, 0.7);
      ell(g, '#2a2433', 1.5, -22.2, 0.55, 0.7);
    }
    if (face !== 'moustache' && face !== 'beard') line(g, 'rgba(120,50,50,.7)', 0.6, -1, -20.4, 1, -20.4);
    PERSON_FACES[face]?.(g, hair);
  }
  personHat(g, hat, shirt);
  if (sitting) g.restore();
}

function visitorSprite(look, back, flip, step, arms = 'down', shut = false, sitting = false) {
  const key = `vis:${look.shirt}:${look.skin}:${look.hair}:${look.hat}:${look.style}:${look.face}:${look.trousers}:${back}:${flip}:${step}:${arms}:${shut}:${sitting}`;
  return sprite(key, 30, 46, 15, 41, (g) => {
    if (flip) g.scale(-1, 1);
    drawPerson(g, { ...look, back, step, arms, shut, sitting });
  });
}

// ---------------------------------------------------------------------------------------------
// Frogs. One for each kind of crawler: round, wide-eyed and a bit smug. The colour comes from a
// hash of the kind, so Googlebot is always the same frog.

function frogPalette(kind) {
  const hue = ih(hash(kind), 7, 3) % 360;
  return {
    body: hslHex(hue, 58, 54),
    dark: hslHex(hue, 54, 33),
    belly: hslHex(hue + 8, 70, 86),
    spot: hslHex(hue + 22, 56, 40),
    accent: hslHex(hue + 170, 80, 66),
    hue,
  };
}

function makeFrog(kind, name, x, y) {
  const h = hash(kind);
  return {
    type: 'frog',
    id: kind,
    name,
    x,
    y,
    px: x + 0.5,
    py: y + 0.5,
    path: [],
    state: 'sit',
    t: 0,
    idle: Math.random() * 2,
    nextFidget: 2 + Math.random() * 5,
    from: [x + 0.5, y + 0.5],
    to: [x + 0.5, y + 0.5],
    dur: 0.4,
    small: false,
    flip: h % 2 === 0,
    blink: 0,
    seed: h % 1000,
    pal: frogPalette(kind),
    accessory: h % 3,
    age: null,
    leaving: 0,
    born: -10,
    sx: undefined,
    sy: undefined,
  };
}

// o.pose: sit, sit2 (breathing in), crouch, up, down, land.  o.look: -1, 0 or 1.  o.eyes: open, shut, happy.
function drawFrogShape(g, pal, o) {
  const { pose, look, eyes, accessory, mouth } = o;
  const squash = pose === 'crouch' ? 1 : pose === 'land' ? 1.3 : 0;
  const stretch = pose === 'up' || pose === 'down';
  const breath = pose === 'sit2' ? 0.8 : 0;
  const rx = 10 + squash * 1.8 - (stretch ? 2.2 : 0);
  const ry = 7 - squash * 1.8 + (stretch ? 2.4 : 0) + breath;
  const cy = -ry - (stretch ? 3 : 1.2);
  const body = pal.body;
  const dk = pal.dark;
  // Back legs: big folded thighs, with webbed feet.
  const leg = (s) => {
    if (pose === 'up') {
      g.save();
      g.translate(s * (rx - 2), cy + ry * 0.6);
      g.rotate(s * 0.5);
      ell(g, dk, 0, 3.5, 2.4, 5.5);
      ell(g, body, 0, 3, 1.5, 4.4);
      ell(g, dk, s * 0.6, 9.5, 3.2, 1.2);
      g.restore();
      return;
    }
    if (pose === 'down') {
      ell(g, dk, s * (rx - 0.5), cy + ry * 0.4, 3.6, 4.4);
      ell(g, body, s * (rx - 0.5), cy + ry * 0.2, 2.6, 3.4);
      ell(g, dk, s * (rx + 2), cy + ry + 3, 3.4, 1.3);
      return;
    }
    const wide = squash * 1.2;
    const x = s * (rx - 0.6 + wide);
    ell(g, 'rgba(25,30,20,.25)', x + s * 1, -0.5, 5, 1.8);
    g.save();
    g.translate(x, -4.4);
    g.rotate(s * 0.3);
    ell(g, dk, 0, 0, 4.4, 5.4);
    ell(g, radial(g, -1, -1.5, 5, lite(body, 0.28), body, dk), 0, -0.4, 3.7, 4.6);
    g.restore();
    // Foot with three toes.
    ell(g, dk, s * (rx + 3.2 + wide), -1.3, 3.8, 1.7);
    for (let k = -1; k <= 1; k++) ell(g, lite(body, 0.12), s * (rx + 5.4 + wide) + k * 0.4, -1.6 + k * 1.3, 1.4, 0.95);
  };
  leg(-1);
  leg(1);
  // Body.
  ell(g, 'rgba(0,0,0,0)', 0, cy, rx, ry);
  g.beginPath();
  g.ellipse(0, cy, rx, ry, 0, 0, 6.2832);
  g.fillStyle = radial(g, -2, cy - 1, rx * 1.15, lite(body, 0.34), body, dk);
  g.fill();
  g.strokeStyle = rgba(dk, 0.85);
  g.lineWidth = 0.9;
  g.stroke();
  // Belly and spots.
  ell(g, grad(g, 0, cy, 0, cy + ry, lite(pal.belly, 0.2), pal.belly), 0, cy + ry * 0.42, rx * 0.62, ry * 0.56);
  g.strokeStyle = rgba(dk, 0.18);
  g.lineWidth = 0.6;
  for (let k = 1; k < 3; k++) {
    g.beginPath();
    g.ellipse(0, cy + ry * 0.35, rx * 0.5, ry * (0.2 + k * 0.14), 0, 0.15, Math.PI - 0.15);
    g.stroke();
  }
  ell(g, rgba(pal.spot, 0.55), -4.5, cy - ry * 0.4, 1.9, 1.3);
  ell(g, rgba(pal.spot, 0.5), 4.8, cy - ry * 0.3, 2.2, 1.4);
  ell(g, rgba(pal.spot, 0.45), 0.4, cy - ry * 0.7, 1.5, 1);
  // Front feet.
  if (pose !== 'up') {
    for (const s of [-1, 1]) {
      const fy = pose === 'down' ? cy + ry - 1 : -1;
      ell(g, dk, s * 4.2, fy, 2.8, 1.5);
      for (let k = -1; k <= 1; k++) ell(g, lite(body, 0.1), s * 4.2 + k * 1.5, fy + 0.5, 0.85, 0.85);
    }
  }
  // Eyes on bumps.
  const ey = cy - ry + 1.4;
  for (const s of [-1, 1]) {
    const ex = s * 5.4;
    ell(g, radial(g, ex, ey, 5, lite(body, 0.3), body, dk), ex, ey, 4.3, 3.9);
    g.strokeStyle = rgba(dk, 0.8);
    g.lineWidth = 0.7;
    g.beginPath();
    g.ellipse(ex, ey, 4.3, 3.9, 0, 0, 6.2832);
    g.stroke();
    if (eyes === 'open') {
      ell(g, radial(g, ex, ey, 3.2, '#ffffff', '#f2f4f8', '#cfd4de'), ex, ey + 0.2, 3.2, 3.1);
      const px = ex + look * 1.1;
      ell(g, '#171a2a', px, ey + 0.5, 1.9, 2);
      ell(g, '#3a4a78', px + 0.2, ey + 1, 1.2, 1.1);
      ell(g, '#ffffff', px - 0.7, ey - 0.4, 0.8, 0.8);
      ell(g, 'rgba(255,255,255,.7)', px + 0.8, ey + 1.2, 0.4, 0.4);
      // A heavy lid, for attitude.
      g.fillStyle = rgba(body, 0.95);
      g.beginPath();
      g.ellipse(ex, ey - 0.2, 3.4, 3.2, 0, Math.PI * 1.08, Math.PI * 1.92);
      g.lineTo(ex + 3.2, ey - 1.2);
      g.lineTo(ex - 3.2, ey - 1.2);
      g.fill();
      line(g, rgba(dk, 0.9), 0.7, ex - 3.2, ey - 1.1, ex + 3.2, ey - 1.1);
    } else if (eyes === 'shut') {
      line(g, rgba(dk, 0.95), 1, ex - 2.8, ey + 0.8, ex + 2.8, ey + 0.8);
    } else {
      g.strokeStyle = rgba(dk, 0.95);
      g.lineWidth = 1.1;
      g.beginPath();
      g.arc(ex, ey + 1.4, 2.5, Math.PI * 1.1, Math.PI * 1.9);
      g.stroke();
    }
  }
  // Cheeks, nostrils and a smile.
  const my = cy + ry * 0.12;
  ell(g, 'rgba(255,120,150,.55)', -rx * 0.62, my + 0.6, 2.1, 1.3);
  ell(g, 'rgba(255,120,150,.55)', rx * 0.62, my + 0.6, 2.1, 1.3);
  rr(g, rgba(dk, 0.7), -1.5, my - 2, 0.8, 0.8);
  rr(g, rgba(dk, 0.7), 0.8, my - 2, 0.8, 0.8);
  if (pose === 'land' || mouth === 'open') {
    g.beginPath();
    g.ellipse(0, my + 1.4, 3.2, 2.2, 0, 0, Math.PI);
    g.fillStyle = '#6a1f35';
    g.fill();
    ell(g, '#ff8aa5', 0, my + 2.6, 1.9, 0.9);
  } else {
    g.strokeStyle = rgba(dk, 0.95);
    g.lineWidth = 0.95;
    g.lineCap = 'round';
    g.beginPath();
    g.moveTo(-4.4, my);
    g.quadraticCurveTo(0, my + 3.4, 4.4, my);
    g.stroke();
    g.lineCap = 'butt';
  }
  // A small something on its head.
  const hx = o.flip ? -4 : 4;
  if (accessory === 0) {
    g.save();
    g.translate(hx, ey - 4);
    g.rotate(0.5);
    ell(g, '#3f9a4a', 0, 0, 3.2, 1.5);
    ell(g, '#6fcf6f', -0.4, -0.3, 2, 0.8);
    g.restore();
  } else if (accessory === 1) {
    for (let k = 0; k < 5; k++) {
      const a = (k / 5) * 6.2832;
      ell(g, pal.accent, hx + Math.cos(a) * 1.9, ey - 4.4 + Math.sin(a) * 1.9, 1.3, 1.3);
    }
    ell(g, '#ffe27a', hx, ey - 4.4, 1.1, 1.1);
  }
}

function frogSprite(f, pose, look, eyes, mouth) {
  const flip = f.flip;
  const key = `frog:${f.pal.hue}:${f.accessory}:${pose}:${look}:${eyes}:${mouth}:${flip}`;
  return sprite(key, 50, 42, 25, 34, (g) => {
    if (flip) g.scale(-1, 1);
    drawFrogShape(g, f.pal, { pose, look, eyes, accessory: f.accessory, flip, mouth });
  });
}

function drawFrog(bg, f, t, clock, hovered, calm) {
  const X = f.dx;
  const Y = f.dy;
  const age = clock - f.born;
  let k = 1;
  if (age < 0.6) k = clamp(1 - Math.pow(1 - age / 0.6, 3), 0.05, 1);
  if (f.leaving) k = Math.max(0.05, 1 - (clock - f.leaving) / 0.6);
  let pose = 'sit';
  let z = 0;
  let eyes = Math.sin(clock * 1.25 + f.seed) > 0.965 ? 'shut' : 'open';
  let mouth = 'closed';
  let look = 0;
  if (f.state === 'crouch') pose = 'crouch';
  else if (f.state === 'land') {
    pose = 'land';
    eyes = 'happy';
  } else if (f.state === 'air') {
    const p = Math.min(1, f.t / f.dur);
    const dist = Math.hypot(f.to[0] - f.from[0], f.to[1] - f.from[1]);
    z = (f.small ? 8 : 11 + dist * 5) * 4 * p * (1 - p);
    pose = p < 0.5 ? 'up' : 'down';
  } else if (!calm) {
    pose = Math.floor(clock * 1.5 + f.seed) % 2 ? 'sit2' : 'sit';
  }
  // A fly to watch, now and then, and a flick of the tongue.
  let fly = null;
  let tongue = null;
  if (f.state === 'sit' && !calm && !f.leaving) {
    const period = 9 + (f.seed % 5);
    const c = (clock + f.seed * 1.7) % period;
    if (c > period - 4) {
      const q = (c - (period - 4)) / 4;
      const ang = q * 14 + f.seed;
      const rad = 26 - q * 12;
      fly = [Math.cos(ang) * rad, -24 + Math.sin(ang * 1.3) * 8 - q * 4];
      look = fly[0] > 2 ? 1 : fly[0] < -2 ? -1 : 0;
      if (q > 0.92) {
        tongue = fly;
        mouth = 'open';
        fly = null;
      }
    }
  }
  if (hovered && f.state === 'sit') {
    z = calm ? 0 : Math.abs(Math.sin(clock * 9)) * 5;
    eyes = 'happy';
    mouth = 'open';
  }
  const s = frogSprite(f, pose, look, eyes, mouth);
  const sh = Math.max(4, 12 - z * 0.3);
  ell(bg, 'rgba(18,28,22,.3)', X + 2, Y + 0.6, sh, sh * 0.4);
  if (hovered) ell(bg, 'rgba(255,255,255,.35)', X, Y, 17, 6.5);
  bg.drawImage(s.c, X - s.ax, Y - z - s.ay * k, s.w, s.h * k);
  if (fly) {
    const fx = X + fly[0];
    const fy = Y + fly[1];
    const flap = Math.floor(clock * 30) % 2;
    ell(bg, 'rgba(255,255,255,.7)', fx - 1.2, fy - 1 - flap, 1.4, 0.8);
    ell(bg, 'rgba(255,255,255,.7)', fx + 1.2, fy - 1 - (1 - flap), 1.4, 0.8);
    ell(bg, '#20263a', fx, fy, 1, 0.8);
  }
  if (tongue) {
    const x0 = X;
    const y0 = Y - 14;
    const tx = X + tongue[0];
    const ty = Y + tongue[1];
    line(bg, '#ff7d9a', 1.6, x0, y0, tx, ty);
    ell(bg, '#ff5d86', tx, ty, 2, 1.5);
  }
  return [X - 25, Y - z - 38, X + 25, Y + 4];
}

// ---------------------------------------------------------------------------------------------
// Cars. There's no traffic for show: a car is a bot or a person on its way somewhere far.

const CAR_COLORS = ['#d9433f', '#2f78d0', '#e8a822', '#eeeae0', '#2a9560', '#8a52d0', '#e5772a', '#4a5568'];

function carSprite(heading, colorIdx) {
  const alongU = heading === 0 || heading === 2;
  const front = heading === 0 || heading === 1; // the end that faces us has headlights
  return sprite(`car:${alongU}:${front}:${colorIdx}`, 44, 36, 22, 26, (g) => {
    const c = CAR_COLORS[colorIdx % CAR_COLORS.length];
    const L = 13;
    const W = 6.4;
    const u0 = alongU ? -L / 2 : -W / 2;
    const u1 = alongU ? L / 2 : W / 2;
    const v0 = alongU ? -W / 2 : -L / 2;
    const v1 = alongU ? W / 2 : L / 2;
    ell(g, 'rgba(20,18,36,.32)', 2.4, 1.4, 12, 4.2);
    const S = { g, day: true, seed: 3, n: 0, info: null };
    // Lower body.
    const slab = (R, z0, z1, mat, detail) => {
      onL(g, R[0], R[3], () => {
        g.fillStyle = grad(g, 0, -z1, 0, -z0, lite(mat, 0.22), mat, dark(mat, 0.18));
        g.fillRect(0, -z1, R[2] - R[0], z1 - z0);
        if (detail) detail('L', R[2] - R[0]);
      });
      onR(g, R[2], R[3], () => {
        g.fillStyle = grad(g, 0, -z1, 0, -z0, mat, dark(mat, 0.25), dark(mat, 0.4));
        g.fillRect(0, -z1, R[3] - R[1], z1 - z0);
        if (detail) detail('R', R[3] - R[1]);
        shadeFace(g, R[3] - R[1], z0, z1);
      });
      fp3(g, lite(mat, 0.3), [R[0], R[1], z1], [R[2], R[1], z1], [R[2], R[3], z1], [R[0], R[3], z1]);
    };
    const wheelsOn = (len) => {
      for (const wx of [2.6, len - 2.6]) {
        ell(g, '#14151c', wx, -1.8, 2.3, 2.3);
        ell(g, '#8a8f9c', wx, -1.8, 1.1, 1.1);
      }
    };
    const lightOn = (len) => {
      for (const lx of [0.9, len - 2.1]) {
        rr(g, front ? '#fff6c4' : '#ff4a4a', lx, -4.8, 1.3, 1.5);
        if (front) ell(g, 'rgba(255,240,170,.5)', lx + 0.6, -4.1, 1.6, 1.4);
      }
    };
    slab([u0, v0, u1, v1], 1.4, 5.8, c, (side, len) => {
      const long = alongU ? side === 'L' : side === 'R';
      if (long) wheelsOn(len);
      else lightOn(len);
    });
    // Cabin and windows.
    const cu0 = alongU ? u0 + 3 : u0 + 0.7;
    const cu1 = alongU ? u1 - 2.2 : u1 - 0.7;
    const cv0 = alongU ? v0 + 0.7 : v0 + 3;
    const cv1 = alongU ? v1 - 0.7 : v1 - 2.2;
    slab([cu0, cv0, cu1, cv1], 5.8, 9.6, lite(c, 0.04), (side, len) => {
      g.fillStyle = grad(g, 0, -9, 0, -6.2, '#a9d4ec', '#35506e');
      g.fillRect(0.8, -9, len - 1.6, 2.8);
      line(g, c, 0.8, len / 2, -9, len / 2, -6.2);
    });
  });
}

// Which way a car points for each way a figure can face: 0 along +x, 1 along +y, 2 along -x, 3 along -y.
const FACE_HEADING = { fr: 0, fl: 1, bl: 2, br: 3 };

function drawCar(bg, car, night) {
  const X = car.dx;
  const Y = car.dy;
  const s = carSprite(car.heading, car.color);
  bg.drawImage(s.c, X - s.ax, Y - s.ay, s.w, s.h);
  void night;
  return [X - 20, Y - 28, X + 20, Y + 6];
}

function carLights(bg, car) {
  const X = car.dx;
  const Y = car.dy;
  const fwd = [[1, 0.5], [-1, 0.5], [-1, -0.5], [1, -0.5]][car.heading];
  const grd = bg.createRadialGradient(X + fwd[0] * 14, Y + fwd[1] * 14 - 3, 1, X + fwd[0] * 14, Y + fwd[1] * 14 - 3, 16);
  grd.addColorStop(0, 'rgba(255,240,170,.5)');
  grd.addColorStop(1, 'rgba(255,240,170,0)');
  bg.fillStyle = grd;
  bg.fillRect(X + fwd[0] * 14 - 18, Y + fwd[1] * 14 - 21, 36, 36);
}

// ---------------------------------------------------------------------------------------------
// Sky: the viewer's own clock, the sun and moon, stars, and clouds that drift past.

function skyAt(hour) {
  const t = hour - 0.5;
  const i = Math.floor(t);
  const f = t - i;
  const a = skyFor(((i % 24) + 24) % 24);
  const b = skyFor((((i + 1) % 24) + 24) % 24);
  return {
    top: mixHex(a.top, b.top, f),
    low: mixHex(a.low, b.low, f),
    stars: lerp(a.stars ? 1 : 0, b.stars ? 1 : 0, f),
    dim: lerp(a.dim, b.dim, f),
    hour,
  };
}

// The colour laid over the whole town as the sun gets low: gold, then orange, then rose, then
// dark blue. It is a list of [hour, colour, strength], blended between the two nearest.
const LIGHT_KEYS = [
  [0, '#0b1440', 0.48],
  [4.6, '#0b1440', 0.46],
  [5.6, '#6a4a9e', 0.26],
  [6.5, '#ff9446', 0.24],
  [8, '#ffcf86', 0.1],
  [9.2, '#ffdda0', 0.05],
  [16, '#ffdda0', 0.05],
  [17.2, '#ffb050', 0.17],
  [18.2, '#ff8c3c', 0.27],
  [19, '#e0605a', 0.26],
  [19.8, '#6a4a9e', 0.31],
  [20.6, '#2c3c88', 0.4],
  [21.4, '#0b1440', 0.47],
  [24, '#0b1440', 0.48],
];

function tintFor(sky) {
  const h = ((sky.hour % 24) + 24) % 24;
  let i = 1;
  while (i < LIGHT_KEYS.length - 1 && LIGHT_KEYS[i][0] < h) i++;
  const [h0, c0, a0] = LIGHT_KEYS[i - 1];
  const [h1, c1, a1] = LIGHT_KEYS[i];
  const f = clamp((h - h0) / (h1 - h0), 0, 1);
  const alpha = lerp(a0, a1, f);
  if (alpha < 0.005) return null;
  const [r, g, b] = parse(mixHex(c0, c1, f));
  return `rgba(${r},${g},${b},${alpha.toFixed(3)})`;
}

const cloudCache = new Map();
// A puffy cloud: a flat-bottomed heap of soft discs, lit from the top left and shaded underneath.
function cloudSprite(shape, color) {
  const key = `${shape}:${color}`;
  let c = cloudCache.get(key);
  if (c) return c;
  const w = 280;
  const h = 120;
  c = makeCanvas(w, h);
  const g = c.getContext('2d');
  const rand = seeded(shape * 977 + 5);
  const under = mixHex(color, '#7d8db0', 0.42);
  const blobs = [];
  const n = 11 + shape * 2;
  for (let i = 0; i < n; i++) {
    const t = (i + rand() * 0.6) / n;
    const body = Math.sin(t * Math.PI);
    const r = 12 + body * 26 + rand() * 12;
    blobs.push([22 + t * (w - 44), h - 26 - r * 0.55 - rand() * body * 16, r]);
  }
  // The whole cloud in one flat colour first, so no circle edges show.
  g.fillStyle = color;
  for (const [x, y, r] of blobs) {
    g.beginPath();
    g.arc(x, y, r, 0, 6.2832);
    g.fill();
  }
  g.fillRect(14, h - 40, w - 28, 22);
  g.globalCompositeOperation = 'source-atop';
  const shade = g.createLinearGradient(0, 8, 0, h - 14);
  shade.addColorStop(0, 'rgba(255,255,255,.55)');
  shade.addColorStop(0.45, 'rgba(255,255,255,0)');
  shade.addColorStop(1, rgba(under, 0.95));
  g.fillStyle = shade;
  g.fillRect(0, 0, w, h);
  for (const [x, y, r] of blobs) {
    const gr = g.createRadialGradient(x - r * 0.35, y - r * 0.5, 1, x - r * 0.2, y - r * 0.3, r * 0.9);
    gr.addColorStop(0, 'rgba(255,255,255,.55)');
    gr.addColorStop(1, 'rgba(255,255,255,0)');
    g.fillStyle = gr;
    g.fillRect(x - r, y - r, r * 2, r * 2);
  }
  g.globalCompositeOperation = 'destination-out';
  const fade = g.createLinearGradient(0, h - 22, 0, h - 8);
  fade.addColorStop(0, 'rgba(0,0,0,0)');
  fade.addColorStop(1, 'rgba(0,0,0,1)');
  g.fillStyle = fade;
  g.fillRect(0, h - 22, w, 22);
  cloudCache.set(key, c);
  if (cloudCache.size > 40) cloudCache.delete(cloudCache.keys().next().value);
  return c;
}

function drawSky(g, w, h, dpr, sky, t) {
  g.setTransform(1, 0, 0, 1, 0, 0);
  const grad2 = g.createLinearGradient(0, 0, 0, h);
  grad2.addColorStop(0, sky.top);
  grad2.addColorStop(0.65, mixHex(sky.top, sky.low, 0.7));
  grad2.addColorStop(1, sky.low);
  g.fillStyle = grad2;
  g.fillRect(0, 0, w, h);
  const night = clamp((sky.dim - 0.04) / 0.2, 0, 1);
  if (sky.stars > 0.02) {
    for (let i = 0; i < 110; i++) {
      const x = ih(i, 1, 21) % w;
      const y = ih(i, 2, 21) % Math.floor(h * 0.8);
      const tw = 0.5 + 0.5 * Math.sin(t * (1 + (i % 5) * 0.3) + i);
      g.globalAlpha = sky.stars * tw * (i % 7 === 0 ? 1 : 0.65);
      g.fillStyle = i % 9 === 0 ? '#ffe9b0' : '#ffffff';
      const s = (i % 6 === 0 ? 2 : 1.2) * dpr;
      g.fillRect(x, y, s, s);
    }
    g.globalAlpha = 1;
  }
  // The sun by day and the moon by night, each rising in the east and setting in the west.
  const hr = sky.hour;
  const dayP = (hr - 6) / 12;
  if (dayP > -0.1 && dayP < 1.1) {
    const sx = w * (0.1 + 0.8 * dayP);
    const sy = h * (0.36 - 0.26 * Math.sin(clamp(dayP, 0, 1) * Math.PI));
    const r = 22 * dpr;
    const glow = g.createRadialGradient(sx, sy, r * 0.3, sx, sy, r * 4);
    glow.addColorStop(0, 'rgba(255,238,170,.6)');
    glow.addColorStop(1, 'rgba(255,238,170,0)');
    g.fillStyle = glow;
    g.fillRect(sx - r * 4, sy - r * 4, r * 8, r * 8);
    g.fillStyle = radial(g, sx, sy, r, '#ffffff', '#ffe58a', '#ffcf5a');
    g.beginPath();
    g.arc(sx, sy, r, 0, 6.2832);
    g.fill();
  }
  const nightP = hr >= 18 ? (hr - 18) / 12 : (hr + 6) / 12;
  if (hr >= 17.5 || hr < 6.5) {
    const mx = w * (0.1 + 0.8 * clamp(nightP, 0, 1));
    const my = h * (0.34 - 0.24 * Math.sin(clamp(nightP, 0, 1) * Math.PI));
    const r = 16 * dpr;
    g.globalAlpha = clamp(night * 1.4, 0, 1);
    const glow = g.createRadialGradient(mx, my, r * 0.4, mx, my, r * 4);
    glow.addColorStop(0, 'rgba(210,225,255,.3)');
    glow.addColorStop(1, 'rgba(210,225,255,0)');
    g.fillStyle = glow;
    g.fillRect(mx - r * 4, my - r * 4, r * 8, r * 8);
    g.fillStyle = radial(g, mx, my, r, '#ffffff', '#efeedd', '#c9c8b8');
    g.beginPath();
    g.arc(mx, my, r, 0, 6.2832);
    g.fill();
    g.fillStyle = mixHex(sky.top, sky.low, 0.2);
    g.beginPath();
    g.arc(mx + r * 0.5, my - r * 0.2, r * 0.82, 0, 6.2832);
    g.fill();
    g.globalAlpha = 1;
  }
  // Clouds, pale by day, dark and dim by night, warm at dusk.
  const q = Math.round(night * 6) / 6;
  const dusk = clamp(1 - Math.abs(sky.dim - 0.12) / 0.07, 0, 1);
  const color = mixHex(mixHex('#ffffff', '#ffc9a8', dusk * 0.7), '#4a5590', q * 0.9);
  const qc = color;
  for (let i = 0; i < 8; i++) {
    const shape = i % 3;
    const sc = cloudSprite(shape, qc);
    const scale = (0.7 + (i % 4) * 0.28) * dpr;
    const cw = sc.width * scale;
    const total = w + cw * 2;
    const speed = (3 + (i % 4) * 2.2) * dpr;
    const x = (((ih(i, 5, 31) % total) + t * speed) % total) - cw;
    const y = h * (0.04 + 0.78 * ((ih(i, 6, 31) % 1000) / 1000));
    g.globalAlpha = (night > 0.5 ? 0.5 : 0.92) * (0.6 + 0.4 * ((i * 37) % 10) / 10);
    g.drawImage(sc, x, y, cw, sc.height * scale);
  }
  g.globalAlpha = 1;
}

// ---------------------------------------------------------------------------------------------
// Icons for the toolbar, painted the same way. Returns a canvas of the given size.

const PLACE_ICONS = {
  lounge: placedLounge,
  bar: placedBar,
  office: placedOffice,
  workshop: placedWorkshop,
  library: placedLibrary,
};

export function toolIcon(tool, size = 32) {
  const c = makeCanvas(size * 2, size * 2);
  const g = c.getContext('2d');
  g.scale((size * 2) / 32, (size * 2) / 32);
  g.translate(16, 20);
  const S = { g, day: true, seed: 1, n: 0, info: null };
  g.lineJoin = 'round';
  if (tool === 'road') {
    fp3(g, '#bcb6a6', [-5, -5], [37, -5], [37, 37], [-5, 37]);
    const K = (c2, lo, hi) => {
      fp3(g, c2, [lo, -6], [hi, -6], [hi, 38], [lo, 38]);
      fp3(g, c2, [-6, lo], [38, lo], [38, hi], [-6, hi]);
    };
    g.save();
    g.scale(0.62, 0.62);
    g.translate(-16, -22);
    fp3(g, '#d6d1c3', [-8, -8], [40, -8], [40, 40], [-8, 40]);
    K('#4a4d57', 9, 23);
    for (let a = 0; a < 40; a += 8) fp3(g, LANE, [a - 4, 15.5], [a, 15.5], [a, 16.5], [a - 4, 16.5]);
    for (let a = 0; a < 40; a += 8) fp3(g, LANE, [15.5, a - 4], [16.5, a - 4], [16.5, a], [15.5, a]);
    g.restore();
  } else if (tool === 'house') {
    g.save();
    g.scale(0.72, 0.72);
    g.translate(-16, -14);
    const R = [7, 9, 25, 23];
    const mat = { k: 'siding', c: '#f2d3a0' };
    box(S, R, 0, 11, mat, (side, len) => {
      win(S, 3, 4, 4, 5, {});
      if (side === 'L') door(S, len - 8, 4.4, 8, '#3f7a6b', { lamp: false });
    });
    gable(S, R, 11, 9, '#b4533c', 1.8, 'u', mat);
    g.restore();
  } else if (tool === 'cafe') {
    g.save();
    g.scale(0.72, 0.72);
    g.translate(-16, -14);
    const R = [6, 8, 26, 24];
    const mat = { k: 'siding', c: '#e8d9b8' };
    box(S, R, 0, 13, mat, (side, len) => {
      if (side === 'L') {
        door(S, len / 2 - 2.2, 4.4, 8.5, '#7b4a2a', { lamp: false });
        win(S, 2.5, 4, 5, 6, {});
        win(S, len - 7.5, 4, 5, 6, {});
      }
    });
    flatRoof(S, R, 13, mat);
    awning(S, R, 10, 5, AWN[0]);
    g.restore();
  } else if (tool === 'club') {
    g.save();
    g.scale(0.72, 0.72);
    g.translate(-16, -14);
    const R = [6, 7, 26, 25];
    const mat = { k: 'brick', c: '#5a4668' };
    box(S, R, 0, 17, mat, (side, len) => {
      if (side === 'L') door(S, len / 2 - 2.2, 4.4, 8.5, '#2a2436', { lamp: false });
    });
    flatRoof(S, R, 17, mat);
    g.restore();
  } else if (tool === 'studio') {
    g.save();
    g.scale(0.72, 0.72);
    g.translate(-16, -14);
    const R = [6, 8, 26, 24];
    const mat = { k: 'brick', c: '#a9573f' };
    box(S, R, 0, 15, mat, (side) => {
      if (side === 'L') door(S, 3, 4.4, 8.5, '#2f5c8a', { lamp: false });
      win(S, 11, 5, 4.4, 7, {});
    });
    flatRoof(S, R, 15, mat);
    for (let k = 0; k < 3; k++) {
      const ua = 7 + k * 6.3;
      slope(g, '#8fb7d0', [[ua, 9, 21], [ua + 5.6, 9, 21]], [[ua, 23, 15], [ua + 5.6, 23, 15]], 6, 2.4);
    }
    g.restore();
  } else if (tool === 'square') {
    g.save();
    g.scale(0.72, 0.72);
    g.translate(-16, -14);
    paving(S, [1, 1, 31, 31], '#d6ccb1', '#b3a887');
    fountain(g);
    g.restore();
  } else if (tool === 'green') {
    g.save();
    g.scale(0.72, 0.72);
    g.translate(-16, -14);
    fp3(g, '#7fb25a', [1, 1], [31, 1], [31, 31], [1, 31]);
    fountain(g);
    for (const [u, v, type] of [[5, 5, 'oak'], [27, 6, 'blossom'], [6, 27, 'bush']]) {
      const [x, y] = P(u, v);
      drawTree(g, x, y, type, u);
    }
    g.restore();
  } else if (PLACE_ICONS[tool]) {
    // W90: the newer places, as the map draws them.
    g.save();
    g.scale(0.72, 0.72);
    g.translate(-16, -14);
    Object.assign(S, { door: 'L', pal: 0, v: 0 });
    PLACE_ICONS[tool](S, { wall: SIDING[0], roof: ROOFS[1], door: DOORC[0], awn: AWN[4], hue: 0 });
    g.restore();
  } else {
    // A bulldozer blade and a cross.
    g.save();
    g.translate(-16, -22);
    rfill(g, grad(g, 0, 8, 0, 28, '#f6b630', '#c07a10'), 2, 14, 22, 10, 2, '#5a3a08');
    rfill(g, '#3a3f4f', 4, 24, 18, 4, 2);
    for (let k = 0; k < 4; k++) ell(g, '#1a1d28', 6 + k * 5, 27.5, 2.3, 2.3);
    rfill(g, '#f9cb58', 12, 8, 10, 8, 1.5, '#5a3a08');
    rfill(g, '#8fcbec', 14, 10, 6, 4, 1);
    rfill(g, grad(g, 26, 10, 30, 30, '#cfd4de', '#7a8294'), 25, 12, 5, 16, 1.5, '#3a4050');
    g.restore();
  }
  void S;
  return c;
}

// ---------------------------------------------------------------------------------------------
// @@WORLD
// ---------------------------------------------------------------------------------------------
// The world

const TOOL_COLOR = {
  road: '#ffe27a',
  house: '#ffd27a',
  studio: '#9ad0ff',
  club: '#c9a6ff',
  cafe: '#ffb04d',
  square: '#8fe3c0',
  bulldoze: '#ff6b6b',
};
const TOOLS = new Set(Object.keys(TOOL_COLOR));
const DIRS4 = [[0, -1], [1, 0], [0, 1], [-1, 0]]; // bit order: north-east, south-east, south-west, north-west
const BUILD_TIME = 0.8;
const MAX_ANIMS = 60;
const MAX_PLAYERS = 400;
// A figure going this many tiles or more takes a car instead of walking. A ride is only how the move
// looks: the server has already put them where they're going.
const DRIVE_TILES = 8;
// How much faster a ride is than the figure's own walk. Slow enough to watch where it goes.
const DRIVE_FACTOR = 1.75;

const screenOf = (tx, ty) => [(tx - ty) * HW, (tx + ty) * HH];

// How a disaster looks (W65). A step of one is 5 seconds, and the art plays out over about that.
const DOOM_STEP = 5;
const DOOM_HOP = 3.5;
const DOOM_TRAIL = 180;
// How many tiles a fire spreads in a step, so the ring keeps creeping between one view and the next.
const FIRE_CREEP = 4;
const DOOM_LOOK = {
  meteors: { tint: '#7a2a12', trail: 'rgba(40, 26, 20, 0.34)' },
  flood: { tint: '#1f4f8a', trail: 'rgba(46, 112, 184, 0.34)' },
  twister: { tint: '#4c5a4a', trail: 'rgba(92, 84, 66, 0.26)' },
  frog: { tint: '#2f5e2a', trail: 'rgba(38, 52, 28, 0.26)' },
};

// Where a disaster's circle falls on the map. A circle of tiles is an ellipse on an isometric map.
function doomSpot([x, y, r]) {
  const [sx, sy] = screenOf(x + 0.5, y + 0.5);
  return { sx, sy, r, rx: r * HW * Math.SQRT2, ry: r * HH * Math.SQRT2 };
}

export class World {
  constructor(container, options = {}) {
    this.container = container;
    this.onTile = typeof options.onTile === 'function' ? options.onTile : null;
    this.onHover = typeof options.onHover === 'function' ? options.onHover : null;
    // Asked for when someone clicks a figure, right-clicks, or presses and holds. See `pickAt`.
    this.onPick = typeof options.onPick === 'function' ? options.onPick : null;
    // Told when the camera stops following a figure, and why: 'moved', 'gone' or 'other'.
    this.onFollow = typeof options.onFollow === 'function' ? options.onFollow : null;
    this.interactive = options.interactive !== false;
    this.limits = { road: 16, ...(options.limits || {}) };
    this.framing = options.framing || {};
    this.labels = [];
    // The disaster, as the last view told it (W66, W67).
    this.doom = null;
    this.doomKey = '';
    this.doomAt = 0;
    this.doomTrail = [];
    this.frogFrom = null;

    container.classList.add('world');
    if (this.interactive) container.classList.add('world-live');
    this.canvas = document.createElement('canvas');
    this.canvas.setAttribute('role', 'img');
    this.canvas.setAttribute('aria-label', 'A town is loading.');
    if (this.interactive) this.canvas.tabIndex = 0;
    this.g = this.canvas.getContext('2d');
    this.skyCanvas = document.createElement('canvas');
    this.skyCanvas.className = 'world-sky';
    this.skyCanvas.setAttribute('aria-hidden', 'true');
    this.sg = this.skyCanvas.getContext('2d', { alpha: false });
    this.skyDirty = true;
    this.tip = document.createElement('div');
    this.tip.className = 'world-tip';
    this.tip.hidden = true;
    this.tip.setAttribute('aria-hidden', 'true');
    container.append(this.skyCanvas, this.canvas, this.tip);

    this.destroyed = false;
    this.ready = false;
    this.n = 0;
    this.id = '';
    this.landKey = '';
    this.chunks = new Map();
    this.frameNo = 0;
    this.players = new Map();
    this.frogs = new Map();
    this.people = new Map();
    this.visitors = new Map();
    this.anims = new Map();
    this.puffs = [];
    this.ents = [];
    this.time = 0;
    this.last = 0;
    this.raf = 0;
    this.dirty = true;
    this.inView = true;
    this.hourOverride = null;
    this.forceCalm = null;
    this.tool = null;
    this.tile = null;
    this.hoverEnt = null;
    this.hoverKey = '';
    this.gesture = null;
    this.pointers = new Map();
    this.panning = false;
    this.pinch = null;
    this.wheelAt = 0;
    this.pulse = null;
    this.following = null;
    this.pressTimer = 0;
    this.pressed = false;
    this.menuAt = 0;
    this.stageName = '';
    this.counts = { buildings: 0, roads: 0 };
    this.times = new Float32Array(180);
    this.timeAt = 0;
    this.timeN = 0;
    this.builds = 0;
    this.drawnChunks = 0;
    this.cam = { x: 0, y: 0, zoom: 1.25, tz: 1.25, vx: 0, vy: 0, move: null, anchor: null };
    this.cssW = 0;
    this.cssH = 0;
    this.dpr = 1;
    this.view = { x0: 0, y0: 0, x1: 1, y1: 1, scale: 1 };

    this.mq = window.matchMedia ? window.matchMedia('(prefers-reduced-motion: reduce)') : null;
    this.systemCalm = Boolean(this.mq && this.mq.matches);
    this.onMq = (e) => {
      this.systemCalm = e.matches;
      this.kick();
    };
    if (this.mq && this.mq.addEventListener) this.mq.addEventListener('change', this.onMq);

    this.tick = this.tick.bind(this);
    this.onVisible = () => {
      this.last = 0;
      this.kick();
    };
    document.addEventListener('visibilitychange', this.onVisible);
    if (typeof ResizeObserver !== 'undefined') {
      this.resizer = new ResizeObserver(() => this.resize());
      this.resizer.observe(container);
    } else {
      this.onResize = () => this.resize();
      window.addEventListener('resize', this.onResize);
    }
    if (typeof IntersectionObserver !== 'undefined') {
      this.watcher = new IntersectionObserver((entries) => {
        this.inView = entries[entries.length - 1].isIntersecting;
        if (this.inView) {
          this.last = 0;
          this.kick();
        }
      });
      this.watcher.observe(container);
    }
    this.bindEvents();
    this.resize();
  }

  get calm() {
    return this.forceCalm === null ? this.systemCalm : this.forceCalm;
  }

  destroy() {
    this.destroyed = true;
    clearTimeout(this.pressTimer);
    if (this.raf) cancelAnimationFrame(this.raf);
    this.raf = 0;
    if (this.mq && this.mq.removeEventListener) this.mq.removeEventListener('change', this.onMq);
    document.removeEventListener('visibilitychange', this.onVisible);
    if (this.resizer) this.resizer.disconnect();
    if (this.onResize) window.removeEventListener('resize', this.onResize);
    if (this.watcher) this.watcher.disconnect();
    for (const ch of this.chunks.values()) this.freeChunk(ch);
    this.chunks.clear();
    this.canvas.remove();
    this.tip.remove();
    this.skyCanvas.remove();
    this.container.classList.remove('world', 'world-live', 'world-grabbing', 'world-tool');
  }

  // --- Public ------------------------------------------------------------------------------

  setTool(tool) {
    this.tool = TOOLS.has(tool) ? tool : null;
    this.gesture = null;
    this.container.classList.toggle('world-tool', Boolean(this.tool));
    this.kick();
  }

  focus(x, y, options = {}) {
    if (!this.ready) return;
    // The first size a map gets frames the town, so going to a tile before then would be undone.
    if (!this.cssW) {
      this.focusOnceSized = [x, y, options];
      return;
    }
    this.stopFollow('other');
    x = clamp(Number(x) || 0, 0, this.n - 1);
    y = clamp(Number(y) || 0, 0, this.n - 1);
    const [sx, sy] = screenOf(x + 0.5, y + 0.5);
    const target = this.clampPoint(sx, sy);
    if (options.zoom) this.cam.tz = clamp(options.zoom, MIN_ZOOM, MAX_ZOOM);
    this.cam.vx = 0;
    this.cam.vy = 0;
    if (options.instant || this.calm) {
      this.cam.x = target[0];
      this.cam.y = target[1];
      this.cam.zoom = this.cam.tz;
      this.cam.move = null;
    } else {
      this.cam.move = { x0: this.cam.x, y0: this.cam.y, x1: target[0], y1: target[1], t: 0, dur: 0.75 };
    }
    this.pulse = { x, y, t0: this.time };
    this.kick();
  }

  setZoom(zoom, options = {}) {
    this.cam.tz = clamp(Number(zoom) || 1, MIN_ZOOM, MAX_ZOOM);
    if (options.instant || this.calm) this.cam.zoom = this.cam.tz;
    this.kick();
  }

  // For testing a day and night: an hour (0 to 24), or null to follow the viewer's clock.
  setHour(hour) {
    this.hourOverride = hour === null || hour === undefined ? null : Number(hour);
    this.kick();
  }

  // For testing reduced motion: true or false to force it, null to follow the system setting.
  setCalm(calm) {
    this.forceCalm = calm === null || calm === undefined ? null : Boolean(calm);
    this.kick();
  }

  // Keeps the camera on a figure until it is dragged away, told to look elsewhere, or the figure
  // leaves. `type` and `id` are what `onPick` gave, so the figure is found again whenever the view
  // changes under it.
  follow(type, id) {
    if (!this.ready || !this.lookup(type, id)) return false;
    this.cam.move = null;
    this.cam.vx = this.cam.vy = 0;
    this.following = { type, id: String(id) };
    this.kick();
    return true;
  }

  isFollowing(type, id) {
    return Boolean(this.following && this.following.type === type && this.following.id === String(id));
  }

  stopFollow(why = 'other') {
    if (!this.following) return;
    this.following = null;
    if (this.onFollow) this.onFollow(why);
  }

  lookup(type, id) {
    const maps = { player: this.players, person: this.people, visitor: this.visitors, frog: this.frogs };
    const found = maps[type] && maps[type].get(String(id));
    return found && !found.leaving ? found : null;
  }

  // What a figure or tile is, as plain data for whoever builds a menu from it.
  figureInfo(e) {
    return {
      type: e.type,
      id: String(e.id),
      name: e.name,
      kind: e.type === 'frog' ? null : e.kind,
      emote: e.emote || null,
      age: e.type === 'frog' && Number.isFinite(e.age) ? e.age : null,
      x: e.x,
      y: e.y,
    };
  }

  tileInfo(x, y) {
    const i = y * this.n + x;
    return {
      x,
      y,
      kind: this.kindName(i),
      level: this.level[i],
      land: ['grass', 'forest', 'water', 'sand', 'hill'][this.land[i]] || 'grass',
    };
  }

  // Asks `onPick` about what is at a point on the page: the figure there if there is one, and the
  // tile. `via` is how it was asked for: 'click', 'context' or 'press'.
  pickAt(clientX, clientY, via) {
    if (!this.onPick || !this.ready || performance.now() - this.menuAt < 400) return false;
    const r = this.canvas.getBoundingClientRect();
    const px = clientX - r.left;
    const py = clientY - r.top;
    const ent = this.pickEntity(px, py);
    const tile = this.tileAtArt(...this.toArt(px, py));
    if (via === 'click' && !ent) return false;
    if (!ent && !tile) return false;
    this.menuAt = performance.now();
    this.hideTip();
    this.onPick({
      via,
      clientX,
      clientY,
      figure: ent ? this.figureInfo(ent) : null,
      tile: tile ? this.tileInfo(tile.x, tile.y) : null,
    });
    return true;
  }

  stats() {
    const count = Math.min(this.timeN, this.times.length);
    const sorted = Array.from(this.times.slice(0, count)).sort((a, b) => a - b);
    const avg = count ? sorted.reduce((a, b) => a + b, 0) / count : 0;
    const frames = this.fpsFrames || 0;
    return {
      avgMs: avg,
      p95Ms: count ? sorted[Math.min(count - 1, Math.floor(count * 0.95))] : 0,
      maxMs: count ? sorted[count - 1] : 0,
      fps: this.fps || 0,
      frames,
      chunksDrawn: this.drawnChunks,
      chunksCached: this.chunks.size,
      chunkBuilds: this.builds,
      sprites: sprites.size,
      spriteMB: Math.round((spritePixels * 4) / 1048576),
      chunkMB: Math.round(([...this.chunks.values()].reduce((a, c) => a + c.canvas.width * c.canvas.height + (c.lights ? c.lights.width * c.lights.height : 0), 0) * 4) / 1048576),
      bots: this.players.size,
      frogs: this.frogs.size,
      cars: [...this.players.values(), ...this.people.values(), ...this.visitors.values()].filter((p) => p.driving).length,
      zoom: this.cam.zoom,
      buffer: `lod ${LOD}`,
      people: this.people.size,
      visitors: this.visitors.size,
      spriteMs,
    };
  }

  // Runs frames back to back and times them: the script time of each one, and the real time for
  // all of them once the graphics card has caught up. `pan` sweeps the camera that many art pixels.
  benchmark(frames = 120, pan = 0) {
    const cpu = [];
    const saved = { x: this.cam.x, y: this.cam.y };
    this.g.getImageData(0, 0, 1, 1);
    const start = performance.now();
    for (let i = 0; i < frames; i++) {
      const t0 = performance.now();
      this.time += 1 / 60;
      this.cam.x = saved.x + Math.sin(i / 20) * pan;
      this.cam.y = saved.y + Math.cos(i / 30) * pan * 0.5;
      this.update(1 / 60);
      this.render();
      cpu.push(performance.now() - t0);
    }
    this.g.getImageData(0, 0, 1, 1);
    const total = performance.now() - start;
    this.cam.x = saved.x;
    this.cam.y = saved.y;
    cpu.sort((a, b) => a - b);
    return {
      frames,
      avgMs: total / frames,
      cpuAvgMs: cpu.reduce((a, b) => a + b, 0) / frames,
      cpuP95Ms: cpu[Math.floor(frames * 0.95)],
      cpuMaxMs: cpu[frames - 1],
    };
  }

  setView(view) {
    if (this.destroyed || !view || typeof view !== 'object') return;
    const n = clamp(Math.floor(Number(view.size)) || 0, 4, 256);
    const rows = Array.isArray(view.land) ? view.land.map((row) => (typeof row === 'string' ? row : '')) : [];
    const landKey = `${view.id}|${n}|${view.seed}|${rows.join(',')}`;
    const fresh = !this.ready || n !== this.n || landKey !== this.landKey;
    if (fresh) this.reset(view, n, rows, landKey);
    const animate = !fresh && !this.calm && this.inView && !document.hidden;
    this.applyTiles(view.tiles, animate);
    this.applyPlayers(view.players, animate);
    this.applyCrawlers(view.crawlers, animate);
    this.applyPeople(view.people, animate);
    // L17: bots at The Commons' places, by name, stroll by their door and walk or drive to the next.
    this.applyVisitors([...(Array.isArray(view.visitors) ? view.visitors : []), ...(Array.isArray(view.bots_out) ? view.bots_out : [])], animate);
    this.applyDoom(view.disaster);
    this.describe(view);
    if (fresh) this.frameTown();
    this.kick();
  }

  // Opens a world looking at its town, and not at the middle of the map. A small town on a big map
  // would otherwise sit in a corner of a lot of empty grass: the view centres on where the buildings
  // are, and comes closer when they're few. A big town keeps the usual zoom. Roads on their own
  // don't count, because a highway runs through nothing.
  frameTown(options) {
    const to = this.townView(options || this.framing);
    this.cam.x = to.x;
    this.cam.y = to.y;
    this.cam.vx = this.cam.vy = 0;
    this.cam.move = null;
    this.cam.zoom = this.cam.tz = to.zoom;
  }

  // Name plates that stand over tiles, for a map whose houses have names: a list of {x, y, text}.
  // They are drawn on the canvas, so whatever the text says it can only ever be text.
  setLabels(list) {
    this.labels = (Array.isArray(list) ? list : []).slice(0, 80).map((item) => ({
      x: clamp(item.x | 0, 0, this.n - 1),
      y: clamp(item.y | 0, 0, this.n - 1),
      text: String(item.text ?? '').slice(0, 28),
      rise: Number(item.rise) || 64,
    })).filter((item) => item.text);
    this.kick();
  }

  drawLabels(g) {
    const list = this.labels;
    const v = this.view;
    if (!list.length || this.cam.zoom < 0.75) return;
    const dpr = this.dpr;
    const size = Math.round(11.5 * dpr);
    g.save();
    g.setTransform(1, 0, 0, 1, 0, 0);
    g.font = `700 ${size}px Tahoma, Verdana, "Segoe UI", sans-serif`;
    g.textAlign = 'center';
    g.textBaseline = 'middle';
    const cw = this.canvas.width;
    const ch = this.canvas.height;
    const taken = [];
    for (const item of list) {
      const [sx, sy] = screenOf(item.x + 0.5, item.y + 0.5);
      const X = (sx - v.x0) * v.scale;
      const Y = (sy - this.height[item.y * this.n + item.x] - item.rise - v.y0) * v.scale;
      const w = g.measureText(item.text).width + 16 * dpr;
      const h = size + 9 * dpr;
      if (X + w / 2 < 0 || X - w / 2 > cw || Y + h < 0 || Y - h > ch) continue;
      const x = Math.round(X - w / 2);
      const y = Math.round(Y - h);
      // A plate that would sit on another one is left out, so names never pile into a smear.
      if (taken.some(([a, b, c, d]) => x < c + 4 * dpr && x + w > a - 4 * dpr && y < d + 4 * dpr && y + h > b - 4 * dpr)) continue;
      taken.push([x, y, x + w, y + h]);
      // A little wooden plate on a short post: shadow, cream face, a dark edge and a lit top edge.
      g.fillStyle = 'rgba(30,18,6,.35)';
      g.fillRect(x + 2 * dpr, y + 2 * dpr, w, h);
      g.fillStyle = '#4a3320';
      g.fillRect(x - dpr, y - dpr, w + 2 * dpr, h + 2 * dpr);
      const face = g.createLinearGradient(0, y, 0, y + h);
      face.addColorStop(0, '#fff6d6');
      face.addColorStop(1, '#ecd9a6');
      g.fillStyle = face;
      g.fillRect(x, y, w, h);
      g.fillStyle = 'rgba(255,255,255,.7)';
      g.fillRect(x, y, w, dpr);
      g.fillStyle = '#4a3320';
      g.fillRect(Math.round(X - dpr), y + h + dpr, 2 * dpr, 7 * dpr);
      g.fillStyle = '#2b1d10';
      g.fillText(item.text, X, y + h / 2 + dpr * 0.5);
    }
    g.restore();
  }

  // The same, but the camera glides there: for a button that says "take me back to the town".
  recentre(options) {
    if (!this.ready) return;
    this.stopFollow('other');
    const to = this.townView(options || this.framing);
    this.cam.vx = this.cam.vy = 0;
    this.cam.tz = to.zoom;
    if (this.calm) {
      this.cam.x = to.x;
      this.cam.y = to.y;
      this.cam.zoom = to.zoom;
      this.cam.move = null;
      this.cam.anchor = null;
    } else {
      this.cam.move = { x0: this.cam.x, y0: this.cam.y, x1: to.x, y1: to.y, t: 0, dur: 0.7 };
    }
    this.kick();
  }

  // Where the camera should be to look at the town: the middle of it, and how close to stand.
  townView({ margin = [420, 380], maxZoom = 1.5 } = {}) {
    const n = this.n;
    const xs = [];
    const ys = [];
    for (let i = 0; i < n * n; i++) {
      const k = this.kind[i];
      if (!k || k === K_ROAD || k === K_CLEARED) continue;
      const x = i % n;
      xs.push(x + 0.5);
      ys.push((i - x) / n + 0.5);
    }
    const base = this.startZoom();
    if (xs.length < 3) {
      // Nothing built yet but roads: stand a little closer, on the road nearest the middle of the
      // map, so the street and the land round it read as the start of a place.
      let best = null;
      let bestD = Infinity;
      for (let i = 0; i < n * n; i++) {
        if (this.kind[i] !== K_ROAD) continue;
        const x = i % n;
        const y = (i - x) / n;
        const d = Math.hypot(x - n / 2, y - n / 2);
        if (d < bestD) {
          bestD = d;
          best = [x + 0.5, y + 0.5];
        }
      }
      const [cx, cy] = best && bestD < n / 3 ? screenOf(...best) : screenOf(n / 2, n / 2);
      const [px, py] = this.clampPoint(cx, cy);
      return { x: px, y: py, zoom: best ? Math.max(base, 1.25) : base };
    }
    // The spread across the screen, ignoring a few stragglers.
    const sx = xs.map((x, i) => (x - ys[i]) * HW).sort((a, b) => a - b);
    const sy = xs.map((x, i) => (x + ys[i]) * HH).sort((a, b) => a - b);
    const at = (list, p) => list[Math.min(list.length - 1, Math.max(0, Math.round(p * (list.length - 1))))];
    const x0 = at(sx, 0.04);
    const x1 = at(sx, 0.96);
    const y0 = at(sy, 0.04);
    const y1 = at(sy, 0.96);
    const [px, py] = this.clampPoint((x0 + x1) / 2, (y0 + y1) / 2 - 24);
    const w = this.cssW || 800;
    const h = this.cssH || 500;
    const fit = Math.min(w / (x1 - x0 + margin[0]), h / (y1 - y0 + margin[1]));
    return { x: px, y: py, zoom: clamp(Math.max(base, Math.min(maxZoom, fit)), MIN_ZOOM, MAX_ZOOM) };
  }

  // --- Loading a world ----------------------------------------------------------------------

  reset(view, n, rows, landKey) {
    for (const ch of this.chunks.values()) this.freeChunk(ch);
    this.chunks.clear();
    this.players.clear();
    this.frogs.clear();
    this.people.clear();
    this.visitors.clear();
    this.stopFollow('other');
    this.anims.clear();
    this.puffs.length = 0;
    this.n = n;
    this.warmQ = null;
    this.id = String(view.id || '');
    this.landKey = landKey;
    this.land = decodeLand(rows, n);
    this.kind = new Uint8Array(n * n);
    this.level = new Uint8Array(n * n);
    this.objs = new Array(n * n).fill(null);
    this.height = new Int8Array(n * n);
    this.shore = new Uint8Array(n * n);
    const land = this.land;
    for (let y = 0; y < n; y++) {
      for (let x = 0; x < n; x++) {
        const i = y * n + x;
        const c = land[i];
        if (c === WATER_L) {
          this.height[i] = WATER_Z;
        } else if (c === HILL_L) {
          let around = 0;
          for (const [dx, dy] of DIRS4) {
            const nx = x + dx;
            const ny = y + dy;
            if (nx >= 0 && ny >= 0 && nx < n && ny < n && land[ny * n + nx] === HILL_L) around++;
          }
          this.height[i] = 8 + around * 4;
        }
      }
    }
    // How far each water tile is from land, up to 2, for deeper colours away from the shore.
    for (let i = 0; i < n * n; i++) this.shore[i] = land[i] === WATER_L ? 2 : 0;
    for (let pass = 0; pass < 2; pass++) {
      for (let y = 0; y < n; y++) {
        for (let x = 0; x < n; x++) {
          const i = y * n + x;
          if (land[i] !== WATER_L) continue;
          for (const [dx, dy] of DIRS4) {
            const nx = x + dx;
            const ny = y + dy;
            if (nx < 0 || ny < 0 || nx >= n || ny >= n) continue;
            const j = ny * n + nx;
            if (land[j] !== WATER_L) this.shore[i] = Math.min(this.shore[i], 0);
            else this.shore[i] = Math.min(this.shore[i], this.shore[j] + 1);
          }
        }
      }
    }
    const [cx, cy] = screenOf(n / 2, n / 2);
    this.cam.x = cx;
    this.cam.y = cy;
    this.cam.vx = this.cam.vy = 0;
    this.cam.move = null;
    this.cam.zoom = this.cam.tz = this.startZoom();
    this.ready = true;
    this.tile = null;
    this.hoverEnt = null;
    this.hideTip();
  }

  startZoom() {
    const w = this.cssW || 800;
    return clamp(w / 1000, 0.8, 1.1);
  }

  applyTiles(list, animate) {
    const n = this.n;
    const nk = new Uint8Array(n * n);
    const nl = new Uint8Array(n * n);
    if (Array.isArray(list)) {
      for (const t of list) {
        if (!Array.isArray(t)) continue;
        const x = t[0] | 0;
        const y = t[1] | 0;
        if (x < 0 || y < 0 || x >= n || y >= n) continue;
        const k = KIND_ID[t[2]];
        if (!k) continue;
        const i = y * n + x;
        nk[i] = k;
        // W91: a big place keeps its level, which says which of its tiles is the one it stands on.
        nl[i] = isZone(k) || isBig(k) ? clamp(t[3] | 0, 0, 3) : 0;
      }
    }
    const changed = [];
    for (let i = 0; i < n * n; i++) {
      if (nk[i] !== this.kind[i] || nl[i] !== this.level[i]) changed.push(i);
    }
    const old = { kind: this.kind, level: this.level };
    this.kind = nk;
    this.level = nl;
    const few = changed.length <= MAX_ANIMS;
    for (const i of changed) {
      const x = i % n;
      const y = (i - x) / n;
      if (animate && few && this.isNear(x, y)) this.startAnim(i, x, y, old.kind[i], old.level[i], nk[i], nl[i]);
      this.markDirty(x, y);
    }
    let roads = 0;
    let buildings = 0;
    for (let i = 0; i < n * n; i++) {
      if (nk[i] === K_ROAD) roads++;
      else if (isZone(nk[i])) buildings += nl[i] + 1;
    }
    this.counts = { buildings, roads };
  }

  isNear(x, y) {
    // Only tiles somewhere near what the viewer is looking at get the building show.
    const [sx, sy] = screenOf(x + 0.5, y + 0.5);
    const v = this.view;
    return sx > v.x0 - 120 && sx < v.x1 + 120 && sy > v.y0 - 120 && sy < v.y1 + 120;
  }

  startAnim(i, x, y, ok, ol, nk, nl) {
    const newObj = nk !== K_ROAD && nk !== 0;
    const oldObj = ok !== K_ROAD && ok !== 0;
    if (!newObj && !oldObj && nk !== K_ROAD) return;
    const [sx, sy] = screenOf(x + 0.5, y + 0.5);
    this.anims.set(i, { type: 'anim', t0: this.time, x, y, ok, ol, nk, nl, old: oldObj, now: newObj });
    for (let k = 0; k < 7; k++) this.puff(sx + (Math.random() - 0.5) * 30, sy + 4 + Math.random() * 6, nk === K_ROAD ? '#d9d0b8' : '#efe6d2');
  }

  puff(x, y, color) {
    if (this.calm || this.puffs.length > 220) return;
    this.puffs.push({
      x,
      y,
      vx: (Math.random() - 0.5) * 30,
      vy: -14 - Math.random() * 18,
      life: 0,
      max: 0.6 + Math.random() * 0.4,
      size: 5 + Math.random() * 5,
      color,
    });
  }

  // Marks every cell that has any of this tile's painting in it, and its neighbours' (roads join up).
  markDirty(x, y) {
    for (const [dx, dy] of [[0, 0], [1, 0], [-1, 0], [0, 1], [0, -1]]) {
      const nx = x + dx;
      const ny = y + dy;
      if (nx < 0 || ny < 0 || nx >= this.n || ny >= this.n) continue;
      const sx = (nx - ny) * HW;
      const sy = (nx + ny) * HH;
      const cx0 = Math.floor((sx - REACH_L - CELL_M) / CELL_W);
      const cx1 = Math.floor((sx + REACH_R + CELL_M) / CELL_W);
      const cy0 = Math.floor((sy - REACH_U - CELL_M) / CELL_H);
      const cy1 = Math.floor((sy + REACH_D + CELL_M) / CELL_H);
      for (let cy = cy0; cy <= cy1; cy++) {
        for (let cx = cx0; cx <= cx1; cx++) {
          const ch = this.chunks.get(this.cellKey(cx, cy));
          if (ch) ch.dirty = true;
        }
      }
    }
  }

  describe(view) {
    const stage = STAGE_WORD[view.stage] || 'a town';
    const whole = (value) => Math.max(0, Math.floor(Number(value) || 0));
    // A private city counts its people, and a public world its places.
    const size = view.places === undefined
      ? `a population of ${whole(view.population)}`
      : `${whole(view.places)} ${whole(view.places) === 1 ? 'place' : 'places'}`;
    const name = typeof view.name === 'string' && view.name ? view.name : 'This world';
    // Who is on the street: a person is a human, an agent is a robot, and a frog is a crawler.
    let people = [...this.people.values()].filter((p) => !p.leaving).length;
    let agents = [...this.visitors.values()].filter((p) => !p.leaving).length;
    for (const p of this.players.values()) {
      if (p.leaving) continue;
      if (p.kind === 'person') people++;
      else agents++;
    }
    const frogs = [...this.frogs.values()].filter((f) => !f.leaving).length;
    const parts = [];
    if (people) parts.push(`${people} ${people === 1 ? 'person' : 'people'}`);
    if (agents) parts.push(`${agents} ${agents === 1 ? 'agent' : 'agents'}`);
    if (frogs) parts.push(`${frogs} ${frogs === 1 ? 'frog' : 'frogs'}`);
    const list = parts.length > 1 ? `${parts.slice(0, -1).join(', ')} and ${parts[parts.length - 1]}` : parts[0];
    let text = `${name}, ${stage} with ${size}.`;
    if (list) text += ` On the street: ${list}.`;
    if (text !== this.label) {
      this.label = text;
      this.canvas.setAttribute('aria-label', text);
    }
  }

  // --- Walking ------------------------------------------------------------------------------

  walkable(x, y) {
    if (x < 0 || y < 0 || x >= this.n || y >= this.n) return false;
    const i = y * this.n + x;
    const c = this.land[i];
    const k = this.kind[i];
    if (c === WATER_L || c === HILL_L) return false;
    if (c === FOREST_L && k !== K_CLEARED && k !== K_ROAD) return false;
    if (isZone(k) && this.level[i] > 0) return false;
    return true;
  }

  // Cheapest route over walkable tiles, preferring roads. Returns tiles after the start.
  findPath(sx, sy, tx, ty, maxLength = 90) {
    const n = this.n;
    if (sx === tx && sy === ty) return [];
    const pad = 8;
    const x0 = Math.max(0, Math.min(sx, tx) - pad);
    const y0 = Math.max(0, Math.min(sy, ty) - pad);
    const x1 = Math.min(n - 1, Math.max(sx, tx) + pad);
    const y1 = Math.min(n - 1, Math.max(sy, ty) + pad);
    const w = x1 - x0 + 1;
    const h = y1 - y0 + 1;
    if (w * h > 9000) return null;
    const cost = new Float32Array(w * h).fill(Infinity);
    const from = new Int32Array(w * h).fill(-1);
    const start = (sy - y0) * w + (sx - x0);
    const goal = (ty - y0) * w + (tx - x0);
    cost[start] = 0;
    const heap = [[0, start]];
    const push = (item) => {
      heap.push(item);
      let i = heap.length - 1;
      while (i > 0) {
        const p = (i - 1) >> 1;
        if (heap[p][0] <= heap[i][0]) break;
        [heap[p], heap[i]] = [heap[i], heap[p]];
        i = p;
      }
    };
    const pop = () => {
      const top = heap[0];
      const last = heap.pop();
      if (heap.length) {
        heap[0] = last;
        let i = 0;
        for (;;) {
          const l = i * 2 + 1;
          const r = l + 1;
          let m = i;
          if (l < heap.length && heap[l][0] < heap[m][0]) m = l;
          if (r < heap.length && heap[r][0] < heap[m][0]) m = r;
          if (m === i) break;
          [heap[m], heap[i]] = [heap[i], heap[m]];
          i = m;
        }
      }
      return top;
    };
    while (heap.length) {
      const [c, cur] = pop();
      if (cur === goal) break;
      if (c > cost[cur]) continue;
      const cx = cur % w;
      const cy = (cur - cx) / w;
      for (const [dx, dy] of DIRS4) {
        const nx = cx + dx;
        const ny = cy + dy;
        if (nx < 0 || ny < 0 || nx >= w || ny >= h) continue;
        const ax = nx + x0;
        const ay = ny + y0;
        const isGoal = ax === tx && ay === ty;
        if (!isGoal && !this.walkable(ax, ay)) continue;
        const step = this.kind[ay * n + ax] === K_ROAD ? 1 : 1.7;
        const j = ny * w + nx;
        if (c + step < cost[j]) {
          cost[j] = c + step;
          from[j] = cur;
          push([c + step, j]);
        }
      }
    }
    if (from[goal] < 0) return null;
    const path = [];
    for (let cur = goal; cur !== start; cur = from[cur]) {
      const cx = cur % w;
      path.push([cx + x0, (cur - cx) / w + y0]);
      if (path.length > maxLength) return null;
    }
    return path.reverse();
  }

  applyPlayers(list, animate) {
    const seen = new Set();
    const n = this.n;
    if (Array.isArray(list)) {
      for (const row of list) {
        if (!Array.isArray(row) || this.players.size >= MAX_PLAYERS) continue;
        const id = String(row[0]);
        const x = clamp(Math.round(Number(row[2])) || 0, 0, n - 1);
        const y = clamp(Math.round(Number(row[3])) || 0, 0, n - 1);
        const emote = EMOTES.has(row[4]) ? row[4] : null;
        // The server says what each player is. A map that doesn't, like the private city, has only bots.
        const kind = row[5] === 'person' || row[5] === 'agent' ? row[5] : null;
        // A guest's row has nothing here (C14).
        const worn = wornIn(row[6]);
        seen.add(id);
        let p = this.players.get(id);
        if (!p) {
          p = makePlayer(id, String(row[1] ?? ''), x, y, kind, worn);
          p.born = animate ? this.time : -10;
          this.players.set(id, p);
          if (animate) this.sparkle(p.px, p.py, 6);
        } else {
          const name = String(row[1] ?? p.name);
          if (name !== p.name || kind !== (p.known ? p.kind : null) || JSON.stringify(worn) !== p.worn) dress(p, name, kind, worn);
          p.leaving = 0;
          if (p.x !== x || p.y !== y) this.walkTo(p, x, y);
        }
        if (p.emote !== emote) p.emoteAt = this.time;
        p.emote = emote;
      }
    }
    for (const [id, p] of this.players) {
      if (seen.has(id)) continue;
      if (animate && !p.leaving) p.leaving = this.time;
      else if (!animate) this.players.delete(id);
    }
  }

  // The people with the page open, and the agents that read the guide without joining. Both come as
  // [name, x, y] and both stroll about near where they were put.
  applyPeople(list, animate) {
    this.applyStreet(this.people, list, makePerson, animate);
  }

  applyVisitors(list, animate) {
    this.applyStreet(this.visitors, list, makeVisitor, animate);
  }

  applyStreet(map, list, make, animate) {
    const seen = new Set();
    const n = this.n;
    if (Array.isArray(list)) {
      list.forEach((row, index) => {
        if (!Array.isArray(row) || map.size >= 200) return;
        const name = String(row[0] ?? '');
        if (!name) return;
        const id = seen.has(name) ? `${name}#${index}` : name;
        const x = clamp(Math.round(Number(row[1])) || 0, 0, n - 1);
        const y = clamp(Math.round(Number(row[2])) || 0, 0, n - 1);
        seen.add(id);
        // A fourth field is what someone signed in, or a bot out at a place, is wearing (C14).
        const worn = wornIn(row[3]);
        // L17: a bot at a place says whether it's inside a building (null) or on an open-air place.
        const atPlace = row.length >= 7;
        const area = Array.isArray(row[6]) ? [clamp(Number(row[6][0]) || 0, 0, n - 1), clamp(Number(row[6][1]) || 0, 0, n - 1)] : null;
        let p = map.get(id);
        if (!p) {
          // L18: a bot that just reached a place sets out from where it was, and goes there.
          const travels = animate && row[4] != null && row[5] != null && Number.isFinite(Number(row[4])) && Number.isFinite(Number(row[5]));
          const fx = travels ? clamp(Math.round(Number(row[4])), 0, n - 1) : x;
          const fy = travels ? clamp(Math.round(Number(row[5])), 0, n - 1) : y;
          p = make(id, name, fx, fy, worn);
          p.born = animate ? this.time : -10;
          map.set(id, p);
          this.settle(p, atPlace, area);
          if (animate && !travels) this.sparkle(p.px, p.py, 5);
          const [tx, ty] = this.standAt(p, x, y);
          if (!animate) {
            // On a page that has just opened, it's already where it is.
            p.x = tx;
            p.y = ty;
            p.px = tx + 0.5 + p.ox;
            p.py = ty + 0.5 + p.oy;
          } else if (fx !== tx || fy !== ty) this.walkTo(p, tx, ty);
        } else {
          p.leaving = 0;
          if (JSON.stringify(worn) !== p.worn) dress(p, name, p.kind, worn);
          this.settle(p, atPlace, area);
          const [tx, ty] = this.standAt(p, x, y);
          if (!p.area && (p.x !== tx || p.y !== ty)) this.walkTo(p, tx, ty);
          else if (p.area && !this.inArea(p, p.x, p.y)) this.walkTo(p, tx, ty);
        }
      });
    }
    for (const [id, p] of map) {
      if (seen.has(id)) continue;
      if (animate && !p.leaving) p.leaving = this.time;
      else if (!animate) map.delete(id);
    }
  }

  // L17: a bot at a building is inside it, and one at an open-air place keeps to the place's tiles.
  settle(p, atPlace, area) {
    p.inside = atPlace && !area;
    p.area = area;
    if (atPlace) p.wander = area ? p.wander || 3 : 0;
  }

  // Where a figure stands: on an open-air place, a tile of it of its own, and anywhere else, there.
  standAt(p, x, y) {
    if (!p.area) return [x, y];
    const k = this.kind[p.area[1] * this.n + p.area[0]];
    const big = k === K_SQUARE || k === K_GREEN;
    const spot = big ? ih(p.h, 7, 3) % 4 : 0;
    return [p.area[0] + (spot & 1), p.area[1] + (spot >> 1)];
  }

  inArea(p, x, y) {
    return Boolean(p.area) && x >= p.area[0] && x <= p.area[0] + 1 && y >= p.area[1] && y <= p.area[1] + 1;
  }

  // L17: inside a building, once it has got there, a bot isn't drawn on the map.
  indoors(p) {
    return Boolean(p.inside) && !p.path.length;
  }

  sparkle(tx, ty, count) {
    if (this.calm) return;
    const [sx, sy] = screenOf(tx, ty);
    for (let i = 0; i < count; i++) this.puff(sx + (Math.random() - 0.5) * 16, sy - 8 + Math.random() * 8, i % 2 ? '#fff6b0' : '#ffffff');
  }

  walkTo(p, x, y) {
    p.x = x;
    p.y = y;
    this.routeTo(p, x, y);
  }

  routeTo(p, x, y) {
    const sx = clamp(Math.round(p.px - 0.5), 0, this.n - 1);
    const sy = clamp(Math.round(p.py - 0.5), 0, this.n - 1);
    if (this.calm) {
      p.px = x + 0.5 + p.ox;
      p.py = y + 0.5 + p.oy;
      p.path.length = 0;
      return;
    }
    const route = this.findPath(sx, sy, x, y);
    if (route) {
      // A car keeps to the road. Only the legs of the route from one road tile to the next are
      // driven, and only when there are enough of them: the walk to the road and from it is on foot.
      const onRoad = route.map(([tx, ty]) => this.roadAt(tx, ty));
      const drives = onRoad.filter(Boolean).length >= DRIVE_TILES;
      p.path = route.map(([tx, ty], i) => {
        const fromRoad = i === 0 ? this.roadAt(sx, sy) : onRoad[i - 1];
        return [tx + 0.5 + p.ox, ty + 0.5 + p.oy, drives && onRoad[i] && fromRoad];
      });
      p.ride = drives ? { color: ih(p.h, 4, 9) % 8 } : null;
    } else if (Math.abs(sx - x) + Math.abs(sy - y) < 14) {
      p.path = [[x + 0.5 + p.ox, y + 0.5 + p.oy, false]];
      p.ride = null;
    } else {
      // No way there, and too far to walk straight: gone from here, and there.
      this.sparkle(p.px, p.py, 6);
      p.px = x + 0.5 + p.ox;
      p.py = y + 0.5 + p.oy;
      p.path.length = 0;
      p.ride = null;
      this.sparkle(p.px, p.py, 6);
    }
  }

  applyCrawlers(list, animate) {
    const seen = new Set();
    const n = this.n;
    if (Array.isArray(list)) {
      for (const row of list) {
        if (!Array.isArray(row) || this.frogs.size >= 40) continue;
        const kind = String(row[0]);
        const x = clamp(Math.round(Number(row[2])) || 0, 0, n - 1);
        const y = clamp(Math.round(Number(row[3])) || 0, 0, n - 1);
        const age = Number(row[4]);
        seen.add(kind);
        let f = this.frogs.get(kind);
        if (!f) {
          f = makeFrog(kind, String(row[1] ?? kind), x, y);
          f.born = animate ? this.time : -10;
          this.frogs.set(kind, f);
          if (animate) this.splash(f.px, f.py);
        } else {
          f.name = String(row[1] ?? f.name);
          f.leaving = 0;
          if (f.x !== x || f.y !== y) this.hopTo(f, x, y);
        }
        f.age = Number.isFinite(age) ? age : null;
      }
    }
    for (const [kind, f] of this.frogs) {
      if (seen.has(kind)) continue;
      if (animate && !f.leaving) f.leaving = this.time;
      else if (!animate) this.frogs.delete(kind);
    }
  }

  splash(tx, ty) {
    if (this.calm) return;
    const [sx, sy] = screenOf(tx, ty);
    for (let i = 0; i < 7; i++) this.puff(sx + (Math.random() - 0.5) * 20, sy - 4, i % 2 ? '#d6f1ff' : '#ffffff');
  }

  hopTo(f, x, y) {
    const sx = clamp(Math.round(f.px - 0.5), 0, this.n - 1);
    const sy = clamp(Math.round(f.py - 0.5), 0, this.n - 1);
    f.x = x;
    f.y = y;
    if (this.calm) {
      f.px = x + 0.5;
      f.py = y + 0.5;
      f.path.length = 0;
      f.state = 'sit';
      return;
    }
    const route = this.findPath(sx, sy, x, y, 70);
    if (route) f.path = route;
    else {
      f.path = [[x, y]];
    }
  }

  // --- Roads --------------------------------------------------------------------------------

  roadAt(x, y) {
    return x >= 0 && y >= 0 && x < this.n && y < this.n && this.kind[y * this.n + x] === K_ROAD;
  }

  cellKey(cx, cy) {
    return (cx + 1024) * 4096 + (cy + 1024);
  }

  getChunk(cx, cy) {
    const idx = this.cellKey(cx, cy);
    let ch = this.chunks.get(idx);
    if (!ch) {
      ch = {
        cx,
        cy,
        idx,
        canvas: makeCanvas(1, 1),
        lights: null,
        lg: null,
        lod: 0,
        lightsLod: 0,
        dirty: true,
        built: false,
        empty: false,
        lightsDirty: true,
        used: 0,
        smokers: [],
        sparkles: [],
        tiles: null,
        wx: cx * CELL_W - CELL_M,
        wy: cy * CELL_H - CELL_M,
      };
      ch.g = ch.canvas.getContext('2d');
      this.chunks.set(idx, ch);
    }
    return ch;
  }

  // The tiles whose painting can land in a cell, back to front, as x, y pairs.
  cellTiles(ch) {
    const n = this.n;
    const dMin = Math.floor((ch.wx - REACH_R) / HW);
    const dMax = Math.ceil((ch.wx + CW + REACH_L) / HW);
    const sMin = Math.max(0, Math.floor((ch.wy - REACH_D) / HH));
    const sMax = Math.min(2 * n - 2, Math.ceil((ch.wy + CH + REACH_U) / HH));
    const out = [];
    for (let sum = sMin; sum <= sMax; sum++) {
      let d = dMin;
      if ((sum + d) & 1) d++;
      for (; d <= dMax; d += 2) {
        const x = (sum + d) >> 1;
        const y = (sum - d) >> 1;
        if (x < 0 || y < 0 || x >= n || y >= n) continue;
        out.push(x, y);
      }
    }
    return out;
  }

  // Only the tile whose middle is in a cell's own area adds its sparkles and smoke, so none repeat.
  owns(ch, lx, ly) {
    const y = ly + HH;
    return lx >= ch.wx + CELL_M && lx < ch.wx + CELL_M + CELL_W && y >= ch.wy + CELL_M && y < ch.wy + CELL_M + CELL_H;
  }

  freeChunk(ch) {
    ch.canvas.width = 0;
    if (ch.lights) ch.lights.width = 0;
  }

  evict() {
    let total = 0;
    for (const c of this.chunks.values()) total += c.canvas.width * c.canvas.height + (c.lights ? c.lights.width * c.lights.height : 0);
    if (total > CHUNK_PIXELS) {
      const old = [...this.chunks.values()].filter((c) => c.used < this.frameNo - 3).sort((a, b) => a.used - b.used);
      for (const ch of old) {
        if (total <= CHUNK_PIXELS * 0.8) break;
        total -= ch.canvas.width * ch.canvas.height + (ch.lights ? ch.lights.width * ch.lights.height : 0);
        this.chunks.delete(ch.idx);
        this.freeChunk(ch);
      }
    }
    // Now and then, let go of what stands on tiles far from the view, so their sprites can be dropped.
    if (this.frameNo % 90 === 0 && spritePixels > SPRITE_PIXELS * 0.5) {
      const v = this.view;
      const n = this.n;
      for (let y = 0; y < n; y++) {
        for (let x = 0; x < n; x++) {
          const i = y * n + x;
          if (!this.objs[i]) continue;
          const sx = (x - y) * HW;
          const sy = (x + y) * HH;
          if (sx < v.x0 - 500 || sx > v.x1 + 500 || sy < v.y0 - 500 || sy > v.y1 + 500) this.objs[i] = null;
        }
      }
    }
  }

  tileTone(x, y) {
    const v = vnoise(x / 5.5, y / 5.5, 11) * 0.7 + vnoise(x / 2.2, y / 2.2, 5) * 0.3;
    return clamp(Math.floor(v * 4.2 - 0.1), 0, 3);
  }

  // Which of the ten shades of meadow a tile is: mostly slow, wide changes, and a little fine grain.
  grassTone(x, y) {
    const v = vnoise(x / 8, y / 8, 17) * 0.72 + vnoise(x / 3.1, y / 3.1, 23) * 0.28;
    return clamp(Math.floor(v * 11 - 0.6), 0, 9);
  }

  roadMask(x, y) {
    let mask = 0;
    for (let b = 0; b < 4; b++) {
      const nx = x + DIRS4[b][0];
      const ny = y + DIRS4[b][1];
      if (this.roadAt(nx, ny)) mask |= 1 << b;
    }
    // A road that runs to the edge of the map carries on over it.
    const n = this.n;
    if (x === 0 && (mask & 2) && !(mask & 8)) mask |= 8;
    if (y === 0 && (mask & 4) && !(mask & 1)) mask |= 1;
    if (x === n - 1 && (mask & 8) && !(mask & 2)) mask |= 2;
    if (y === n - 1 && (mask & 1) && !(mask & 4)) mask |= 4;
    return mask;
  }

  // W91: which piece of a big place a tile is: 0 where it's stored, then across, down, and both.
  cornerOf(x, y, k) {
    const at = (tx, ty) => tx >= 0 && ty >= 0 && this.kind[ty * this.n + tx] === k && this.level[ty * this.n + tx] === 0;
    if (this.level[y * this.n + x] === 0) return 0;
    if (at(x - 1, y)) return 1;
    if (at(x, y - 1)) return 2;
    return 3;
  }

  // W91: a park's sides that run on into more of it, or onto a road, so no hedge goes there.
  greenMask(x, y) {
    let mask = 0;
    for (let b = 0; b < 4; b++) {
      const nx = x + DIRS4[b][0];
      const ny = y + DIRS4[b][1];
      if (nx < 0 || ny < 0 || nx >= this.n || ny >= this.n) continue;
      const k = this.kind[ny * this.n + nx];
      if (k === K_GREEN || k === K_ROAD) mask |= 1 << b;
    }
    return mask;
  }

  parkMask(x, y) {
    let mask = 0;
    for (let b = 0; b < 4; b++) {
      const nx = x + DIRS4[b][0];
      const ny = y + DIRS4[b][1];
      if (nx < 0 || ny < 0 || nx >= this.n || ny >= this.n) continue;
      const k = this.kind[ny * this.n + nx];
      if (k === K_PARK || k === K_ROAD) mask |= 1 << b;
    }
    return mask;
  }

  topSprite(x, y, i) {
    const code = this.land[i];
    const k = this.kind[i];
    const tone = this.tileTone(x, y);
    const roll = ih(x, y, 1);
    if (k === K_ROAD) return roadSprite(this.roadMask(x, y), roll % 5 === 0 ? 1 : 0);
    if (k === K_PARK) return parkTop(this.parkMask(x, y), tone % 3, roll % 3);
    if (k === K_CLEARED) return clearedTop(tone % 3);
    if (isZone(k) && this.level[i] === 0) return lotTop(k, tone);
    switch (code) {
      case WATER_L: {
        let edges = 0;
        for (let b = 0; b < 4; b++) {
          const nx = x + DIRS4[b][0];
          const ny = y + DIRS4[b][1];
          if (nx >= 0 && ny >= 0 && nx < this.n && ny < this.n && this.land[ny * this.n + nx] !== WATER_L) edges |= 1 << b;
        }
        return waterSprite(this.shore[i], roll % 2, edges);
      }
      case SAND_L:
        return sandSprite(tone % 3, roll % 9 === 0 ? 1 + (roll >>> 4) % 3 : 0);
      case FOREST_L:
        return forestGround(tone % 3, roll % 3);
      case HILL_L:
        return hillTop(tone % 3, roll % 7 === 0 ? 1 : 0);
      default:
        return grassSprite(this.grassTone(x, y), roll % 4 === 0 ? 1 + (roll >>> 3) % 5 : 0);
    }
  }

  objectAt(x, y, i, k, lv) {
    const code = this.land[i];
    const roll = ih(x, y, 2);
    if (k === K_ROAD) {
      // Lamps, trees, benches and the like along the pavement of straight roads.
      const mask = this.roadMask(x, y);
      const ew = mask === 10;
      if (!ew && mask !== 5) return null;
      const r = roll % 100;
      const side = (roll >> 8) & 1;
      const edge = side ? 3.5 : 28.5;
      const at = (a) => (ew ? [a, edge] : [edge, a]);
      if (r < 17) return streetObject('lamp', ...at(16), 0);
      if (r < 24) return streetObject('tree', ...at(16), (roll >> 12) & 3);
      if (r < 28 && ew && !side) return streetObject('bench', 16, 28.5, 0);
      if (r < 31) return streetObject('hydrant', ...at(8), 0);
      if (r < 34) return streetObject('bin', ...at(24), 0);
      if (r < 36) return streetObject('post', ...at(8), 0);
      return null;
    }
    if (k === K_PARK) return parkObject(roll % 4, this.parkMask(x, y));
    if (k === K_CLEARED) return stumpObject(roll % 4);
    // W91: a big place is four pieces, each for its corner of it.
    if (k === K_GREEN) return parkObject(this.cornerOf(x, y, k), this.greenMask(x, y));
    if (k === K_SQUARE) return this.buildingAt(x, y, k, this.cornerOf(x, y, k));
    if (k >= K_HOUSE) return this.buildingAt(x, y, k, 0);
    if (isZone(k)) {
      if (lv === 0) return lotObject(k);
      return this.buildingAt(x, y, k, lv);
    }
    if (code === FOREST_L) return forestObject(roll % 14);
    if (code === HILL_L) return roll % 4 === 0 ? decoObject('pines', roll % 6) : roll % 11 === 0 ? decoObject('rocks', roll % 4) : null;
    if (code === GRASS_L) {
      const r = roll % 100;
      if (r < 3) return decoObject('bush', roll % 5);
      if (r < 6) return decoObject(roll % 3 === 0 ? 'autumn' : roll % 2 ? 'oak' : 'small', roll % 5);
      if (r < 7) return decoObject('rocks', roll % 4);
    }
    if (code === SAND_L && roll % 100 < 2) return decoObject('rocks', roll % 4);
    return null;
  }

  buildingAt(x, y, k, lv) {
    // The door faces a road if there is one on a side that can be seen.
    let door = ih(x, y, 6) & 1 ? 'L' : 'R';
    if (this.roadAt(x, y + 1)) door = 'L';
    else if (this.roadAt(x + 1, y)) door = 'R';
    const h = ih(x, y, 3);
    return buildingObject(k, lv, h % 3, door, (h >>> 4) % 6);
  }

  faceSegments(code, h, hn) {
    // The pieces of a side wall, from the top of this tile down to its neighbour.
    const segs = [];
    if (hn <= -SLAB) {
      if (code === WATER_L) segs.push(['wall', h, hn]);
      else if (code === HILL_L) {
        segs.push(['rock', h, 0]);
        segs.push(['earth', 0, hn]);
      } else segs.push(['earth', h, hn]);
      return segs;
    }
    if (code === HILL_L) {
      segs.push(['rock', h, Math.max(hn, 0)]);
      if (hn < 0) segs.push(['bank', 0, hn]);
    } else if (code === SAND_L) segs.push(['sand', h, hn]);
    else segs.push(['bank', h, hn]);
    return segs;
  }

  // The land under a tile, and the cliffs or banks on its two sides that face us.
  paintGround(g, ch, x, y, lx, ly) {
    const n = this.n;
    const i = y * n + x;
    const code = this.land[i];
    const h = this.height[i];
    const top = this.topSprite(x, y, i);
    g.drawImage(top.c, lx - top.ax, ly - h - top.ay, top.w, top.h);
    const hr = x + 1 < n ? this.height[i + 1] : -SLAB;
    const hl = y + 1 < n ? this.height[i + n] : -SLAB;
    const seed = ih(x, y, 4) & 3;
    if (hl < h) {
      for (const [mat, from, to] of this.faceSegments(code, h, hl)) {
        if (from <= to) continue;
        const f = faceSprite('L', mat, from, to, seed);
        g.drawImage(f.c, lx - f.ax, ly - f.ay, f.w, f.h);
      }
    }
    if (hr < h) {
      for (const [mat, from, to] of this.faceSegments(code, h, hr)) {
        if (from <= to) continue;
        const f = faceSprite('R', mat, from, to, seed);
        g.drawImage(f.c, lx - f.ax, ly - f.ay, f.w, f.h);
      }
    }
    if (code === WATER_L && this.owns(ch, lx, ly)) ch.sparkles.push(lx + ((ih(x, y, 8) % 31) - 15), ly + 12 + (ih(x, y, 9) % 10), ih(x, y, 10) % 300);
  }

  paintShadow(g, x, y, lx, ly) {
    const i = y * this.n + x;
    const o = this.objs[i];
    if (!o || !o.shadow || this.anims.has(i)) return;
    const s = o.shadow();
    if (s) g.drawImage(s.c, lx - s.ax, ly - this.height[i] - s.ay, s.w, s.h);
  }

  paintObject(g, ch, x, y, lx, ly) {
    const i = y * this.n + x;
    const o = this.objs[i];
    if (!o || this.anims.has(i)) return;
    g.drawImage(o.c, lx - o.ax, ly - this.height[i] - o.ay, o.w, o.h);
    const info = o.info;
    if (info && this.owns(ch, lx, ly)) for (const s of info.smoke) ch.smokers.push(lx + s[0], ly + s[1] - this.height[i], ih(x, y, 12) % 997, 2);
  }

  buildChunk(ch) {
    const tiles = this.cellTiles(ch);
    ch.tiles = tiles;
    ch.smokers.length = 0;
    ch.sparkles.length = 0;
    if (!tiles.length) {
      // Nothing of the map reaches here.
      ch.empty = true;
      ch.canvas.width = 1;
      ch.canvas.height = 1;
      ch.lod = LOD;
      ch.dirty = false;
      ch.built = true;
      ch.lightsDirty = true;
      return;
    }
    ch.empty = false;
    if (ch.lod !== LOD || ch.canvas.width <= 1) {
      ch.canvas.width = Math.ceil(CW * LOD);
      ch.canvas.height = Math.ceil(CH * LOD);
      ch.g = ch.canvas.getContext('2d');
      ch.lod = LOD;
    }
    const g = ch.g;
    g.setTransform(1, 0, 0, 1, 0, 0);
    g.clearRect(0, 0, ch.canvas.width, ch.canvas.height);
    // Paint in whole-world coordinates, shifted so this cell's corner is the canvas's corner.
    g.setTransform(LOD, 0, 0, LOD, -ch.wx * LOD, -ch.wy * LOD);
    g.imageSmoothingEnabled = true;
    for (let k = 0; k < tiles.length; k += 2) {
      const i = tiles[k + 1] * this.n + tiles[k];
      this.objs[i] = this.objectAt(tiles[k], tiles[k + 1], i, this.kind[i], this.level[i]);
    }
    // The land first, then shadows on it, then everything that stands up, back to front.
    for (let k = 0; k < tiles.length; k += 2) this.paintGround(g, ch, tiles[k], tiles[k + 1], (tiles[k] - tiles[k + 1]) * HW, (tiles[k] + tiles[k + 1]) * HH);
    for (let k = 0; k < tiles.length; k += 2) this.paintShadow(g, tiles[k], tiles[k + 1], (tiles[k] - tiles[k + 1]) * HW, (tiles[k] + tiles[k + 1]) * HH);
    for (let k = 0; k < tiles.length; k += 2) this.paintObject(g, ch, tiles[k], tiles[k + 1], (tiles[k] - tiles[k + 1]) * HW, (tiles[k] + tiles[k + 1]) * HH);
    ch.dirty = false;
    ch.built = true;
    ch.lightsDirty = true;
    this.builds++;
  }

  buildLights(ch) {
    if (ch.empty) {
      ch.lightsDirty = false;
      return;
    }
    if (!ch.lights || ch.lightsLod !== ch.lod) {
      ch.lights = makeCanvas(Math.ceil(CW * ch.lod), Math.ceil(CH * ch.lod));
      ch.lg = ch.lights.getContext('2d');
      ch.lightsLod = ch.lod;
    }
    const g = ch.lg;
    g.setTransform(1, 0, 0, 1, 0, 0);
    g.clearRect(0, 0, ch.lights.width, ch.lights.height);
    g.setTransform(ch.lod, 0, 0, ch.lod, -ch.wx * ch.lod, -ch.wy * ch.lod);
    const tiles = ch.tiles || [];
    for (let k = 0; k < tiles.length; k += 2) {
      const x = tiles[k];
      const y = tiles[k + 1];
      const idx = y * this.n + x;
      const obj = this.objs[idx];
      if (!obj || typeof obj.lit !== 'function' || this.anims.has(idx)) continue;
      const lit = obj.lit();
      g.drawImage(lit.c, (x - y) * HW - lit.ax, (x + y) * HH - this.height[idx] - lit.ay, lit.w, lit.h);
    }
    ch.lightsDirty = false;
  }

  // In spare moments, paints the sprites for tiles just beyond the screen, nearest first, so that a
  // cell built as the view moves finds them ready and doesn't stall the frame.
  warmUp(t0) {
    if (!this.ready || this.calm || performance.now() - t0 > 7) return;
    const v = this.view;
    const cx = (v.x0 + v.x1) / 2;
    const cy = (v.y0 + v.y1) / 2;
    if (!this.warmQ || this.warmLod !== LOD || Math.hypot(cx - this.warmC[0], cy - this.warmC[1]) > 160) {
      const n = this.n;
      const mx = Math.floor(cy / 32 + cx / 64);
      const my = Math.floor(cy / 32 - cx / 64);
      const reach = Math.min(30, Math.ceil(Math.max(v.x1 - v.x0, v.y1 - v.y0) / 48) + 8);
      const q = [];
      for (let y = Math.max(0, my - reach); y <= Math.min(n - 1, my + reach); y++) {
        for (let x = Math.max(0, mx - reach); x <= Math.min(n - 1, mx + reach); x++) q.push([(x - mx) * (x - mx) + (y - my) * (y - my), x, y]);
      }
      q.sort((a, b) => a[0] - b[0]);
      this.warmQ = q;
      this.warmI = 0;
      this.warmC = [cx, cy];
      this.warmLod = LOD;
    }
    const q = this.warmQ;
    const n = this.n;
    const night = this.sky ? clamp((this.sky.dim - 0.04) / 0.2, 0, 1) > 0.1 : false;
    const stop = performance.now() + 2;
    while (this.warmI < q.length && performance.now() < stop) {
      const [, x, y] = q[this.warmI++];
      const i = y * n + x;
      this.topSprite(x, y, i);
      const h = this.height[i];
      const seed = ih(x, y, 4) & 3;
      const code = this.land[i];
      const hr = x + 1 < n ? this.height[i + 1] : -SLAB;
      const hl = y + 1 < n ? this.height[i + n] : -SLAB;
      for (const [side, hn] of [['L', hl], ['R', hr]]) {
        if (hn >= h) continue;
        for (const [mat, from, to] of this.faceSegments(code, h, hn)) if (from > to) faceSprite(side, mat, from, to, seed);
      }
      const o = this.objectAt(x, y, i, this.kind[i], this.level[i]);
      if (o) {
        if (o.shadow) o.shadow();
        if (night && typeof o.lit === 'function') o.lit();
      }
    }
  }

  hourNow() {
    if (this.hourOverride !== null) return this.hourOverride;
    const d = new Date();
    return d.getHours() + d.getMinutes() / 60 + d.getSeconds() / 3600;
  }

  // --- Sizing -------------------------------------------------------------------------------

  resize() {
    if (this.destroyed) return;
    const w = this.container.clientWidth;
    const h = this.container.clientHeight;
    if (!w || !h) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const first = !this.cssW;
    this.cssW = w;
    this.cssH = h;
    this.dpr = dpr;
    const cw = Math.round(w * dpr);
    const ch = Math.round(h * dpr);
    const wiped = this.canvas.width !== cw || this.canvas.height !== ch;
    if (wiped) {
      this.canvas.width = cw;
      this.canvas.height = ch;
      this.skyCanvas.width = cw;
      this.skyCanvas.height = ch;
      this.skyDirty = true;
    }
    if (first && this.ready) this.frameTown();
    if (first && this.focusOnceSized) {
      this.focus(...this.focusOnceSized);
      this.focusOnceSized = null;
    }
    this.clampCam();
    // W73: a canvas that is resized is cleared, and the browser shows that before the next frame comes.
    // So it's all painted now, or the sky shows where the map was. The sky is opaque, so with no view
    // yet it still goes on, alone.
    if (wiped && this.ready) this.render();
    else if (wiped) drawSky(this.sg, cw, ch, dpr, skyAt(this.hourNow()), this.calm ? 0 : this.time);
    this.kick();
  }

  minZoom() {
    return MIN_ZOOM;
  }

  clampPoint(x, y) {
    if (!this.n) return [x, y];
    // Keep the middle of the screen over the map: in tile terms, inside 0..n.
    let tx = y / (2 * HH) + x / (2 * HW);
    let ty = y / (2 * HH) - x / (2 * HW);
    tx = clamp(tx, 0, this.n);
    ty = clamp(ty, 0, this.n);
    return [(tx - ty) * HW, (tx + ty) * HH];
  }

  clampCam() {
    const [x, y] = this.clampPoint(this.cam.x, this.cam.y);
    this.cam.x = x;
    this.cam.y = y;
    const min = this.minZoom();
    if (this.cam.tz < min) this.cam.tz = min;
    if (this.cam.zoom < min) this.cam.zoom = min;
  }

  // --- The loop -----------------------------------------------------------------------------

  kick() {
    this.dirty = true;
    this.schedule();
  }

  live() {
    return !this.calm && !document.hidden && this.inView && this.ready;
  }

  schedule() {
    if (this.raf || this.destroyed || !this.ready || !this.cssW) return;
    if (!this.live() && !this.dirty) return;
    if (document.hidden) return;
    this.raf = requestAnimationFrame(this.tick);
  }

  tick(now) {
    this.raf = 0;
    if (this.destroyed) return;
    const t0 = performance.now();
    let dt = this.last ? Math.min(0.1, (now - this.last) / 1000) : 0;
    this.last = now;
    if (this.calm) dt = 0;
    this.fpsAcc = (this.fpsAcc || 0) + (dt || 0.016);
    this.fpsFrames = (this.fpsFrames || 0) + 1;
    if (this.fpsAcc >= 1) {
      this.fps = this.fpsFrames / this.fpsAcc;
      this.fpsAcc = 0;
      this.fpsFrames = 0;
    }
    this.time += dt;
    this.update(dt);
    this.render();
    this.warmUp(t0);
    this.dirty = false;
    this.times[this.timeAt] = performance.now() - t0;
    this.timeAt = (this.timeAt + 1) % this.times.length;
    this.timeN++;
    if (this.live() || this.dirty || this.cam.move || this.cam.anchor || this.following || Math.abs(this.cam.tz - this.cam.zoom) > 0.002) this.schedule();
    else if (this.pendingWork) this.schedule();
  }

  update(dt) {
    const cam = this.cam;
    const calm = this.calm;
    // Camera: eased moves, eased zoom, a little glide after a drag.
    if (cam.move) {
      cam.move.t += dt / cam.move.dur;
      const e = ease(Math.min(1, cam.move.t));
      cam.x = lerp(cam.move.x0, cam.move.x1, e);
      cam.y = lerp(cam.move.y0, cam.move.y1, e);
      if (cam.move.t >= 1) cam.move = null;
    }
    if (Math.abs(cam.tz - cam.zoom) > 0.0005) {
      const k = calm ? 1 : 1 - Math.exp(-dt * 14);
      cam.zoom += (cam.tz - cam.zoom) * (dt ? k : 1);
      if (Math.abs(cam.tz - cam.zoom) < 0.002) cam.zoom = cam.tz;
      if (cam.anchor) {
        cam.x = cam.anchor.ax - cam.anchor.sx / cam.zoom;
        cam.y = cam.anchor.ay - cam.anchor.sy / cam.zoom;
      }
    } else if (cam.anchor) {
      cam.anchor = null;
    }
    if ((cam.vx || cam.vy) && !this.panning) {
      cam.x += cam.vx * dt;
      cam.y += cam.vy * dt;
      const f = Math.exp(-dt * 5);
      cam.vx *= f;
      cam.vy *= f;
      if (Math.abs(cam.vx) < 4 && Math.abs(cam.vy) < 4) cam.vx = cam.vy = 0;
    }
    this.followCamera(dt);
    this.clampCam();
    this.pendingWork = false;
    this.updateAnims();
    if (!dt) return;
    this.updatePlayers(dt);
    this.updateFrogs(dt);
    for (let i = this.puffs.length - 1; i >= 0; i--) {
      const p = this.puffs[i];
      p.life += dt;
      p.x += p.vx * dt;
      p.y += p.vy * dt;
      p.vy += 4 * dt;
      if (p.life >= p.max) this.puffs.splice(i, 1);
    }
  }

  // The camera eases after a figure it is following, and lets go if the figure has gone.
  followCamera(dt) {
    const f = this.following;
    if (!f) return;
    const e = this.lookup(f.type, f.id);
    if (!e) {
      this.stopFollow('gone');
      return;
    }
    const [sx, sy] = screenOf(e.px, e.py);
    const [tx, ty] = this.clampPoint(sx, sy - (e.type === 'frog' ? 8 : 14));
    const cam = this.cam;
    const k = this.calm || !dt ? 1 : 1 - Math.exp(-dt * 6);
    cam.move = null;
    cam.vx = cam.vy = 0;
    cam.x += (tx - cam.x) * k;
    cam.y += (ty - cam.y) * k;
  }

  updateAnims() {
    for (const [i, a] of this.anims) {
      if (this.time - a.t0 >= BUILD_TIME || this.calm) {
        this.anims.delete(i);
        this.markDirty(a.x, a.y);
      }
    }
  }

  updatePlayers(dt) {
    for (const map of [this.players, this.people, this.visitors]) {
      for (const [id, p] of map) {
        if (p.leaving && this.time - p.leaving > 0.5) {
          map.delete(id);
          continue;
        }
        if (p.path.length) {
          let budget = p.speed * (p.ride && p.path[0][2] ? DRIVE_FACTOR : 1) * dt;
          while (budget > 0 && p.path.length) {
            const [tx, ty] = p.path[0];
            const dx = tx - p.px;
            const dy = ty - p.py;
            const dist = Math.hypot(dx, dy);
            if (dist > 0.001) {
              if (Math.abs(dx) > Math.abs(dy)) p.face = dx > 0 ? 'fr' : 'bl';
              else p.face = dy > 0 ? 'fl' : 'br';
            }
            if (dist <= budget) {
              p.px = tx;
              p.py = ty;
              p.path.shift();
              budget -= dist;
            } else {
              p.px += (dx / dist) * budget;
              p.py += (dy / dist) * budget;
              budget = 0;
            }
          }
          const driving = Boolean(p.ride && p.path.length && p.path[0][2]);
          // In and out of the car where the road starts and ends, in a little dust.
          if (driving !== Boolean(p.driving)) this.sparkle(p.px, p.py, 3);
          p.driving = driving;
          p.walking = !driving;
          if (!p.path.length) p.ride = null;
        } else {
          p.walking = false;
          // People and visitors potter about a little near where they were seen, then drift back.
          if (p.wander && !p.leaving && !this.calm) {
            p.idle = (p.idle || 0) + dt;
            if (p.idle > p.wander) {
              p.idle = 0;
              this.stroll(p);
            }
          }
        }
      }
    }
  }

  stroll(p) {
    if (p.area) {
      // L17: round the open-air place, and never off it onto the street.
      const tx = p.area[0] + (Math.random() < 0.5 ? 0 : 1);
      const ty = p.area[1] + (Math.random() < 0.5 ? 0 : 1);
      p.path = [[tx + 0.2 + Math.random() * 0.6, ty + 0.2 + Math.random() * 0.6, false]];
      return;
    }
    const cx = clamp(Math.round(p.px - 0.5), 0, this.n - 1);
    const cy = clamp(Math.round(p.py - 0.5), 0, this.n - 1);
    if (cx !== p.x || cy !== p.y) {
      this.routeTo(p, p.x, p.y);
      return;
    }
    for (let tries = 0; tries < 6; tries++) {
      const tx = clamp(p.x + Math.floor(Math.random() * 7) - 3, 0, this.n - 1);
      const ty = clamp(p.y + Math.floor(Math.random() * 7) - 3, 0, this.n - 1);
      if ((tx !== p.x || ty !== p.y) && this.walkable(tx, ty)) {
        this.routeTo(p, tx, ty);
        return;
      }
    }
  }

  updateFrogs(dt) {
    for (const [kind, f] of this.frogs) {
      if (f.leaving && this.time - f.leaving > 0.6) {
        this.frogs.delete(kind);
        continue;
      }
      f.t += dt;
      switch (f.state) {
        case 'sit':
          f.idle += dt;
          if (f.path.length && !f.leaving) {
            // Short paths are hopped a tile at a time, long ones two.
            const take = f.path.length > 8 ? 2 : 1;
            let target = null;
            for (let k = 0; k < take && f.path.length; k++) target = f.path.shift();
            f.from = [f.px, f.py];
            f.to = [target[0] + 0.5, target[1] + 0.5];
            f.dur = 0.34 + 0.06 * Math.hypot(f.to[0] - f.from[0], f.to[1] - f.from[1]);
            f.state = 'crouch';
            f.t = 0;
            const sdx = f.to[0] - f.from[0] - (f.to[1] - f.from[1]);
            if (sdx > 0.01) f.flip = false;
            else if (sdx < -0.01) f.flip = true;
          } else if (f.idle > f.nextFidget && !this.calm) {
            // A small hop on the spot, for the joy of it.
            f.from = [f.px, f.py];
            f.to = [f.px, f.py];
            f.dur = 0.3;
            f.state = 'crouch';
            f.t = 0;
            f.small = true;
            f.idle = 0;
            f.nextFidget = 3 + Math.random() * 6;
          }
          break;
        case 'crouch':
          if (f.t >= 0.11) {
            f.state = 'air';
            f.t = 0;
          }
          break;
        case 'air': {
          const p = Math.min(1, f.t / f.dur);
          f.px = lerp(f.from[0], f.to[0], p);
          f.py = lerp(f.from[1], f.to[1], p);
          if (p >= 1) {
            f.px = f.to[0];
            f.py = f.to[1];
            f.state = 'land';
            f.t = 0;
            if (!f.small) this.ripple(f);
            f.small = false;
          }
          break;
        }
        case 'land':
          if (f.t >= (f.path.length ? 0.07 : 0.14)) {
            f.state = 'sit';
            f.t = 0;
            f.idle = 0;
            f.x = Math.floor(f.px);
            f.y = Math.floor(f.py);
          }
          break;
        default:
          f.state = 'sit';
      }
      f.blink = Math.sin(this.time * 1.3 + f.seed) > 0.97 ? 1 : 0;
    }
  }

  ripple(f) {
    const [sx, sy] = screenOf(f.px, f.py);
    for (let i = 0; i < 2; i++) this.puff(sx + (i ? 6 : -6), sy, '#f4efe0');
  }

  render() {
    if (!this.ready || !this.cssW) return;
    const t0 = performance.now();
    this.frameNo++;
    sweepSprites();
    const g = this.g;
    const cam = this.cam;
    const cw = this.canvas.width;
    const ch = this.canvas.height;
    const scale = cam.zoom * this.dpr;
    // Sprites are painted at the resolution they're shown at, so they are never scaled up.
    const lod = scale <= 1.15 ? 1 : scale <= 2.3 ? 2 : scale <= 3.4 ? 3 : 4;
    if (lod !== LOD) useLod(lod);
    const left = cam.x - cw / scale / 2;
    const top = cam.y - ch / scale / 2;
    const tx = Math.round(-left * scale);
    const ty = Math.round(-top * scale);
    const v = this.view;
    v.x0 = left;
    v.y0 = top;
    v.x1 = left + cw / scale;
    v.y1 = top + ch / scale;
    v.scale = scale;
    const sky = skyAt(this.hourNow());
    this.sky = sky;
    const night = clamp((sky.dim - 0.04) / 0.2, 0, 1);
    const calm = this.calm;
    const t = calm ? 0 : this.time;
    if (this.skyDirty || calm || this.frameNo % 2 === 0) {
      drawSky(this.sg, cw, ch, this.dpr, sky, t);
      this.skyDirty = false;
    }

    g.setTransform(1, 0, 0, 1, 0, 0);
    g.globalAlpha = 1;
    g.globalCompositeOperation = 'source-over';
    g.clearRect(0, 0, cw, ch);
    // Chunks are painted at the resolution they're shown at, so most of the time this is a plain
    // copy. Smoothing only matters in between (or while a chunk waits to be repainted).
    const ratio = scale / lod;
    g.imageSmoothingEnabled = Math.abs(ratio - 1) > 0.001;
    g.imageSmoothingQuality = ratio < 0.8 ? 'high' : 'low';
    g.setTransform(scale, 0, 0, scale, tx, ty);

    // Which cells are on screen, limited to where the map can reach.
    const n = this.n;
    const vis = [];
    const cx0 = Math.max(Math.floor((v.x0 - CELL_M) / CELL_W), Math.floor((-n * HW - REACH_L) / CELL_W));
    const cx1 = Math.min(Math.floor((v.x1 + CELL_M) / CELL_W), Math.floor((n * HW + REACH_R) / CELL_W));
    const cy0 = Math.max(Math.floor((v.y0 - CELL_M) / CELL_H), Math.floor((-REACH_U) / CELL_H));
    const cy1 = Math.min(Math.floor((v.y1 + CELL_M) / CELL_H), Math.floor(((2 * n - 2) * HH + REACH_D) / CELL_H));
    for (let cy = cy0; cy <= cy1; cy++) for (let cx = cx0; cx <= cx1; cx++) vis.push([cx, cy]);

    // Paint the ones that need it, nearest the middle first, within a time budget.
    const midX = (v.x0 + v.x1) / 2;
    const midY = (v.y0 + v.y1) / 2;
    const need = [];
    for (const [cx, cy] of vis) {
      const c = this.chunks.get(this.cellKey(cx, cy));
      if (!c || c.dirty || (c.lod !== LOD && !c.empty)) {
        const dist = Math.hypot((cx + 0.5) * CELL_W - midX, (cy + 0.5) * CELL_H - midY);
        need.push([dist - (!c || !c.built ? 1e5 : 0), cx, cy]);
      }
    }
    if (need.length) {
      need.sort((a, b) => a[0] - b[0]);
      const budget = this.firstPaint ? 10 : 45;
      for (const [, cx, cy] of need) {
        const c = this.getChunk(cx, cy);
        if (performance.now() - t0 > budget && c.built) continue;
        if (performance.now() - t0 > budget * 3 && !c.built) {
          this.pendingWork = true;
          continue;
        }
        this.buildChunk(c);
      }
      this.pendingWork = this.pendingWork || need.some(([, cx, cy]) => {
        const c = this.chunks.get(this.cellKey(cx, cy));
        return !c || !c.built || c.dirty || (c.lod !== LOD && !c.empty);
      });
      this.firstPaint = true;
    }
    let drawn = 0;
    const drawList = [];
    for (const [cx, cy] of vis) {
      const c = this.chunks.get(this.cellKey(cx, cy));
      if (!c || !c.built) continue;
      c.used = this.frameNo;
      if (c.empty) continue;
      g.drawImage(c.canvas, c.wx, c.wy, CW, CH);
      drawList.push(c);
      drawn++;
    }
    this.drawnChunks = drawn;
    this.evict();

    this.drawGroundFx(g, drawList, t, scale);
    this.drawEntities(g, drawList, t, night, v);
    this.drawSmoke(g, drawList, t, night);

    // Dusk and night over everything in the world, then the lights that shine in it.
    const tint = tintFor(sky);
    if (tint) {
      g.setTransform(1, 0, 0, 1, 0, 0);
      g.globalCompositeOperation = 'source-atop';
      g.fillStyle = tint;
      g.fillRect(0, 0, cw, ch);
      g.globalCompositeOperation = 'source-over';
      g.setTransform(scale, 0, 0, scale, tx, ty);
    }
    if (night > 0.02) {
      g.globalCompositeOperation = 'lighter';
      g.globalAlpha = night;
      for (const c of drawList) {
        if (c.lightsDirty || !c.lights) this.buildLights(c);
        // Only each cell's own area: lights add up, so the overlap would show as a brighter seam.
        if (c.lights) {
          const m = CELL_M * c.lightsLod;
          g.drawImage(c.lights, m, m, c.lights.width - 2 * m, c.lights.height - 2 * m, c.wx + CELL_M, c.wy + CELL_M, CELL_W, CELL_H);
        }
      }
      this.drawLights(g, t, night);
      g.globalAlpha = 1;
      g.globalCompositeOperation = 'source-over';
    }
    this.drawDoom(g, t, scale, tx, ty, cw, ch);
    this.drawOverlay(g, t);
    this.drawLabels(g);
    this.placeTip();
  }

  // --- Disasters -----------------------------------------------------------------------------

  // W66, W67: a disaster is painted over the map from what the view says. Nothing here changes a tile:
  // those come down in the next view, and a step takes a minute, so what's painted plays over that.
  applyDoom(disaster) {
    const next = disaster && typeof disaster === 'object' ? disaster : null;
    const key = next ? `${next.stage}|${next.done ?? ''}` : '';
    if (key === this.doomKey) {
      this.doom = next;
      return;
    }
    const was = this.doom;
    if (was?.stage === 'striking' && Array.isArray(was.hits)) {
      this.doomTrail.push(...was.hits.filter(Array.isArray));
      this.doomTrail.splice(0, Math.max(0, this.doomTrail.length - DOOM_TRAIL));
      this.frogFrom = was.hits[0] || null;
    }
    if (!next || next.stage !== 'striking') this.doomTrail = [];
    this.doomKey = key;
    this.doomAt = this.time;
    this.doom = next;
  }

  drawDoom(bg, t, scale, tx, ty, cw, ch) {
    const doom = this.doom;
    if (!doom) return;
    const look = DOOM_LOOK[doom.kind] || DOOM_LOOK.meteors;
    const pulse = doom.stage === 'striking' ? 0.3 : doom.stage === 'omen' ? 0.16 + 0.06 * Math.sin(t * 0.9) : 0.08;
    bg.setTransform(1, 0, 0, 1, 0, 0);
    bg.globalCompositeOperation = 'source-atop';
    bg.globalAlpha = pulse;
    bg.fillStyle = look.tint;
    bg.fillRect(0, 0, cw, ch);
    bg.globalCompositeOperation = 'source-over';
    bg.globalAlpha = 1;
    bg.setTransform(scale, 0, 0, scale, tx, ty);
    if (doom.kind === 'meteors' && doom.stage !== 'over') this.drawFallingStars(bg, t);
    if (doom.stage !== 'striking' || !Array.isArray(doom.hits)) return;
    const hits = doom.hits.filter(Array.isArray).map(doomSpot);
    const age = this.calm ? DOOM_STEP / 2 : Math.max(0, t - this.doomAt);
    // All in one path, so overlapping circles read as one mark and not as a pile of discs.
    bg.fillStyle = look.trail;
    bg.beginPath();
    for (const mark of doom.kind === 'meteors' ? [] : this.doomTrail.map(doomSpot)) {
      bg.moveTo(mark.sx + mark.rx, mark.sy);
      bg.ellipse(mark.sx, mark.sy, mark.rx, mark.ry, 0, 0, Math.PI * 2);
    }
    bg.fill();
    if (doom.kind === 'flood') this.drawFlood(bg, hits, t);
    else if (doom.kind === 'twister') this.drawTwister(bg, hits, age, t);
    else if (doom.kind === 'frog') this.drawGiantFrog(bg, hits[0], age, t);
    else this.drawFire(bg, hits, age, t);
  }

  // Omens of meteors: bright streaks high over wherever you're looking.
  drawFallingStars(bg, t) {
    const v = this.view;
    const wide = v.x1 - v.x0;
    const high = v.y1 - v.y0;
    bg.lineCap = 'round';
    for (let k = 0; k < 4; k++) {
      const along = (t * (0.11 + k * 0.03) + k * 0.27) % 1;
      const x = v.x0 + wide * ((k * 0.31 + along) % 1);
      const y = v.y0 + high * (0.08 + 0.3 * along);
      bg.strokeStyle = `rgba(255, 226, 170, ${0.7 * (1 - along)})`;
      bg.lineWidth = 2;
      bg.beginPath();
      bg.moveTo(x, y);
      bg.lineTo(x - 26, y - 13);
      bg.stroke();
    }
  }

  // Fire spreading from where each meteor came down. Scorched ground inside, and flames only where a
  // fire meets what hasn't burned, so overlapping fires read as one.
  drawFire(bg, hits, age, t) {
    const creep = (clamp(age / DOOM_STEP, 0, 1) * FIRE_CREEP) / Math.max(1, ...hits.map((hit) => hit.r));
    const fires = hits.map((hit) => {
      const grown = 1 + creep * (hit.r ? 1 : 0);
      return { ...hit, rx: hit.rx * grown, ry: hit.ry * grown };
    });
    const ring = (lay) => {
      bg.beginPath();
      for (const fire of fires) {
        bg.moveTo(fire.sx + fire.rx, fire.sy);
        bg.ellipse(fire.sx, fire.sy, fire.rx, fire.ry, 0, 0, Math.PI * 2);
      }
      lay();
    };
    ring(() => {
      bg.fillStyle = 'rgba(34, 22, 14, 0.36)';
      bg.fill();
    });
    const v = this.view;
    for (const [n, fire] of fires.entries()) {
      // About a flame every 26 pixels round the edge, however far the fire has got.
      const flames = clamp(Math.round((fire.rx + fire.ry) * Math.PI / 26), 12, 320);
      for (let k = 0; k < flames; k++) {
        const angle = (k / flames) * Math.PI * 2 + n;
        const x = fire.sx + Math.cos(angle) * fire.rx;
        const y = fire.sy + Math.sin(angle) * fire.ry;
        if (x < v.x0 || x > v.x1 || y < v.y0 - 40 || y > v.y1 + 40) continue;
        // Inside another fire there's nothing left to burn.
        if (fires.some((other) => other !== fire && ((x - other.sx) / other.rx) ** 2 + ((y - other.sy) / other.ry) ** 2 < 1)) continue;
        const lick = 16 + 9 * Math.sin(t * 8 + k * 1.9 + n);
        bg.fillStyle = 'rgba(255, 96, 24, 0.28)';
        bg.beginPath();
        bg.ellipse(x, y - lick * 0.3, 24, 20 + lick * 0.6, 0, 0, Math.PI * 2);
        bg.fill();
        bg.fillStyle = 'rgba(255, 112, 28, 0.9)';
        bg.beginPath();
        bg.ellipse(x, y - lick * 0.6, 7, lick, 0, 0, Math.PI * 2);
        bg.fill();
        bg.fillStyle = 'rgba(255, 224, 120, 0.95)';
        bg.beginPath();
        bg.ellipse(x, y - lick * 0.35, 3.5, lick * 0.5, 0, 0, Math.PI * 2);
        bg.fill();
      }
    }
    // The newest meteor, coming down into the middle of its fire.
    const newest = fires[fires.length - 1];
    if (newest && newest.r <= 3 && age < 1) {
      const fall = 1 - age;
      bg.strokeStyle = 'rgba(255, 170, 70, 0.85)';
      bg.lineWidth = 7;
      bg.lineCap = 'round';
      bg.beginPath();
      bg.moveTo(newest.sx + fall * 230, newest.sy - fall * 460);
      bg.lineTo(newest.sx + fall * 230 + 70, newest.sy - fall * 460 - 140);
      bg.stroke();
    }
  }

  // One wall of water: the foam goes on first, so the water covers all of it but the leading edge.
  drawFlood(bg, hits, t) {
    bg.beginPath();
    for (const hit of hits) {
      const swell = 1 + 0.06 * Math.sin(t * 2 + hit.sx * 0.01);
      bg.moveTo(hit.sx + hit.rx * swell, hit.sy);
      bg.ellipse(hit.sx, hit.sy, hit.rx * swell, hit.ry * swell, 0, 0, Math.PI * 2);
    }
    bg.strokeStyle = 'rgba(236, 246, 255, 0.85)';
    bg.lineWidth = 12;
    bg.stroke();
    bg.fillStyle = 'rgba(46, 112, 184, 0.9)';
    bg.fill();
  }

  drawTwister(bg, hits, age, t) {
    if (hits.length < 2) return;
    const along = clamp(age / DOOM_STEP, 0, 1) * (hits.length - 1);
    const at = Math.min(hits.length - 2, Math.floor(along));
    const x = lerp(hits[at].sx, hits[at + 1].sx, along - at);
    const y = lerp(hits[at].sy, hits[at + 1].sy, along - at);
    bg.fillStyle = 'rgba(40, 36, 30, 0.32)';
    bg.beginPath();
    bg.ellipse(x, y, hits[0].rx * 0.7, hits[0].ry * 0.7, 0, 0, Math.PI * 2);
    bg.fill();
    for (let k = 0; k < 16; k++) {
      const up = k / 15;
      const sway = Math.sin(t * 5 + k * 0.7) * (6 + 34 * up);
      const wide = 10 + 96 * up * up + Math.sin(t * 9 + k) * 3;
      bg.strokeStyle = `rgba(${74 + k * 5}, ${72 + k * 5}, ${64 + k * 5}, 0.78)`;
      bg.lineWidth = 7;
      bg.beginPath();
      bg.ellipse(x + sway, y - up * 330, wide, wide * 0.34, 0, 0, Math.PI * 2);
      bg.stroke();
    }
  }

  // The frogs on the street are web crawlers. This one isn't.
  drawGiantFrog(bg, hit, age, t) {
    if (!hit) return;
    const from = this.frogFrom ? doomSpot(this.frogFrom) : hit;
    const hop = clamp(age / DOOM_HOP, 0, 1);
    const x = lerp(from.sx, hit.sx, ease(hop));
    const y = lerp(from.sy, hit.sy, ease(hop));
    const lift = Math.sin(hop * Math.PI) * 340;
    const size = hit.rx * 0.36;
    const landed = hop >= 1;
    const squash = landed ? 1 + Math.max(0, 0.28 - (age - DOOM_HOP) * 0.5) : 0.92;
    bg.fillStyle = 'rgba(20, 30, 16, 0.38)';
    bg.beginPath();
    bg.ellipse(x, y, size * 1.15, size * 0.5, 0, 0, Math.PI * 2);
    bg.fill();
    if (landed && age - DOOM_HOP < 1.2) {
      const ring = (age - DOOM_HOP) / 1.2;
      bg.strokeStyle = `rgba(240, 232, 200, ${1 - ring})`;
      bg.lineWidth = 5;
      bg.beginPath();
      bg.ellipse(x, y, hit.rx * ring, hit.ry * ring, 0, 0, Math.PI * 2);
      bg.stroke();
    }
    const body = y - lift - size * 0.55;
    const part = (color, ox, oy, rx, ry) => {
      bg.fillStyle = color;
      bg.beginPath();
      bg.ellipse(x + ox * size, body + oy * size, rx * size, ry * size, 0, 0, Math.PI * 2);
      bg.fill();
    };
    part('#3f8f3a', -0.82, 0.42, 0.42, 0.26);
    part('#3f8f3a', 0.82, 0.42, 0.42, 0.26);
    part('#58ad47', 0, 0.1, 0.95 * squash, 0.68 / squash);
    part('#cfe8a2', 0, 0.36, 0.62 * squash, 0.34 / squash);
    part('#58ad47', -0.46, -0.52, 0.28, 0.27);
    part('#58ad47', 0.46, -0.52, 0.28, 0.27);
    const blink = Math.sin(t * 1.3) > 0.97 ? 0.03 : 0.19;
    part('#fbf6dc', -0.46, -0.56, 0.19, 0.19);
    part('#fbf6dc', 0.46, -0.56, 0.19, 0.19);
    part('#17201a', -0.46, -0.54, 0.1, blink * 0.6);
    part('#17201a', 0.46, -0.54, 0.1, blink * 0.6);
    bg.strokeStyle = '#2c6028';
    bg.lineWidth = Math.max(2, size * 0.04);
    bg.lineCap = 'round';
    bg.beginPath();
    bg.moveTo(x - size * 0.55, body - size * 0.04);
    bg.quadraticCurveTo(x, body + size * 0.2, x + size * 0.55, body - size * 0.04);
    bg.stroke();
  }



  // Water sparkle.
  drawGroundFx(bg, list, t, scale) {
    if (scale < 0.7) return;
    for (const c of list) {
      const s = c.sparkles;
      for (let i = 0; i < s.length; i += 3) {
        const ph = (t * 1.5 + s[i + 2] * 0.037) % 4;
        if (ph < 0.3) {
          bg.fillStyle = `rgba(255,255,255,${(0.9 * Math.sin((ph / 0.3) * Math.PI)).toFixed(2)})`;
          bg.beginPath();
          bg.ellipse(s[i], s[i + 1], 2.6, 0.9, 0, 0, 6.2832);
          bg.fill();
        }
      }
    }
  }



  drawEntities(bg, list, t, night, v) {
    const ents = this.ents;
    ents.length = 0;
    const scale = v.scale;
    const snap = (x) => Math.round(x * scale) / scale;
    const inView = (X, Y, r = 60) => X > v.x0 - r && X < v.x1 + r && Y > v.y0 - r * 1.6 && Y < v.y1 + r;
    const place = (e, X, Y) => {
      e.sx = X;
      e.sy = Y;
      e.dx = snap(X);
      e.dy = snap(Y);
    };
    const waiting = this.pendingWork;
    const ready = (X, Y) => {
      if (!waiting) return true;
      const c = this.chunks.get(this.cellKey(Math.floor(X / CELL_W), Math.floor(Y / CELL_H)));
      return Boolean(c && c.built);
    };
    for (const p of this.players.values()) {
      place(p, ...screenOf(p.px, p.py));
      if (inView(p.sx, p.sy) && ready(p.sx, p.sy)) ents.push(p);
    }
    for (const map of [this.people, this.visitors]) {
      for (const p of map.values()) {
        if (this.indoors(p)) continue;
        place(p, ...screenOf(p.px, p.py));
        if (inView(p.sx, p.sy) && ready(p.sx, p.sy)) ents.push(p);
      }
    }
    for (const f of this.frogs.values()) {
      place(f, ...screenOf(f.px, f.py));
      if (inView(f.sx, f.sy) && ready(f.sx, f.sy)) ents.push(f);
    }
    for (const a of this.anims.values()) {
      const [X, Y] = screenOf(a.x + 0.5, a.y + 0.5);
      a.sx = X;
      a.sy = Y + 8;
      if (inView(X, Y, 120)) ents.push(a);
    }
    ents.sort((a, b) => a.sy - b.sy);
    for (const e of ents) {
      let r4 = null;
      switch (e.type) {
        case 'player':
        case 'person':
        case 'visitor':
          // Someone on a ride is drawn as their car, facing the way they're going.
          r4 = e.driving
            ? drawCar(bg, { dx: e.dx, dy: e.dy, heading: FACE_HEADING[e.face] ?? 0, color: e.ride.color }, night)
            : drawPlayer(bg, e, t, this.time, this.hoverEnt === e, this.calm);
          break;
        case 'frog':
          r4 = drawFrog(bg, e, t, this.time, this.hoverEnt === e, this.calm);
          break;
        default:
          r4 = this.drawBuildAnim(bg, e);
      }
      if (r4) this.occlude(bg, e, r4);
    }
    // Dust and sparkles on top.
    for (const p of this.puffs) {
      const k = p.life / p.max;
      bg.globalAlpha = (1 - k) * 0.8;
      const r = p.size * (0.6 + k * 0.8);
      const gr = bg.createRadialGradient(p.x, p.y, 0, p.x, p.y, r);
      gr.addColorStop(0, p.color);
      gr.addColorStop(1, 'rgba(255,255,255,0)');
      bg.fillStyle = gr;
      bg.fillRect(p.x - r, p.y - r, r * 2, r * 2);
    }
    bg.globalAlpha = 1;
  }

  // Anything standing in front of a figure is painted over it again, so people walk behind houses.
  occlude(bg, e, r) {
    const n = this.n;
    const tile = e.type === 'player' || e.type === 'frog' || e.type === 'visitor' || e.type === 'person';
    const tx = tile ? Math.floor(e.px) : e.x;
    const ty = tile ? Math.floor(e.py) : e.y;
    for (let s = 1; s <= 5; s++) {
      for (let a = 0; a <= s; a++) {
        const b = s - a;
        if (Math.abs(a - b) > 1) continue;
        const x = tx + a;
        const y = ty + b;
        if (x < 0 || y < 0 || x >= n || y >= n) continue;
        const i = y * n + x;
        const o = this.objs[i];
        if (!o || this.anims.has(i)) continue;
        const [sx, sy] = screenOf(x, y);
        const ox = sx - o.ax;
        const oy = sy - this.height[i] - o.ay;
        if (ox > r[2] || oy > r[3] || ox + o.w < r[0] || oy + o.h < r[1]) continue;
        bg.drawImage(o.c, ox, oy, o.w, o.h);
      }
    }
  }

  drawBuildAnim(bg, a) {
    const age = this.time - a.t0;
    const i = a.y * this.n + a.x;
    const [sx, sy] = screenOf(a.x, a.y);
    const base = sy - this.height[i] + HH;
    let r4 = null;
    // Phase one: the old thing shrinks away. Phase two: the new thing springs up.
    const gone = a.old ? 0.28 : 0;
    const draw = (obj, k) => {
      if (!obj || k <= 0.02) return;
      const hh = Math.max(1, Math.round(obj.h * k));
      const dx = sx - obj.ax;
      const dy = Math.round(base - (obj.ay + HH) * k);
      bg.drawImage(obj.c, dx, dy, obj.w, hh);
      r4 = [dx, dy, dx + obj.w, dy + hh];
    };
    if (a.old && age < gone) draw(this.spriteFor(a.x, a.y, a.ok, a.ol), 1 - ease(age / gone));
    if (a.now && age >= gone) {
      const k = clamp((age - gone) / (BUILD_TIME - gone), 0, 1);
      const spring = 1 - Math.pow(1 - k, 3) + Math.sin(k * Math.PI * 2.5) * 0.12 * (1 - k);
      draw(this.spriteFor(a.x, a.y, a.nk, a.nl), clamp(spring, 0.02, 1.15));
    }
    return r4;
  }

  spriteFor(x, y, k, lv) {
    if (!k) return null;
    const i = y * this.n + x;
    return this.objectAt(x, y, i, k, lv);
  }

  drawSmoke(bg, list, t, night) {
    const tint = night > 0.4 ? [150, 160, 196] : [244, 245, 250];
    for (const c of list) {
      const s = c.smokers;
      for (let i = 0; i < s.length; i += 4) {
        for (let k = 0; k < 5; k++) {
          const p = (t * 0.17 + s[i + 2] * 0.013 + k * 0.2) % 1;
          const r = 3.5 + p * 9;
          const x = s[i] + p * 16 + Math.sin(p * 6 + k * 1.7) * 2.5;
          const y = s[i + 1] - p * 44;
          const a = (1 - p) * 0.55 * Math.min(1, p * 6);
          const gr = bg.createRadialGradient(x, y, 0, x, y, r);
          gr.addColorStop(0, `rgba(${tint[0]},${tint[1]},${tint[2]},${a.toFixed(3)})`);
          gr.addColorStop(1, `rgba(${tint[0]},${tint[1]},${tint[2]},0)`);
          bg.fillStyle = gr;
          bg.fillRect(x - r, y - r, r * 2, r * 2);
        }
      }
    }
  }

  // Headlights on the cars of anyone driving somewhere.
  drawLights(bg, t, night) {
    for (const map of [this.players, this.people, this.visitors]) {
      for (const p of map.values()) {
        if (!p.driving || p.dx === undefined) continue;
        carLights(bg, { dx: p.dx, dy: p.dy, heading: FACE_HEADING[p.face] ?? 0 });
      }
    }
  }

  // The tool's highlight, and the ring that shows where `focus` was pointed.
  drawOverlay(bg, t) {
    const strip = (c, u0, v0, u1, v1) => fp(bg, c, [P(u0, v0), P(u1, v0), P(u1, v1), P(u0, v1)]);
    const pulse = this.pulse;
    if (pulse) {
      const age = this.time - pulse.t0;
      if (age > 1.8) this.pulse = null;
      else {
        // Two rings of light spreading out from the tile.
        const grow = this.calm ? 0.35 : easeOut(Math.min(1, age / 0.9));
        const a = this.calm ? 0.9 : Math.max(0, 1 - age / 1.8);
        for (let r = 0; r < 2; r++) {
          const spread = (r ? 8 : 0) + grow * 44;
          const u0 = pulse.x * 32 - spread;
          const v0 = pulse.y * 32 - spread;
          const u1 = (pulse.x + 1) * 32 + spread;
          const v1 = (pulse.y + 1) * 32 + spread;
          bg.globalAlpha = a * (r ? 0.5 : 0.95);
          const c = '#ffffff';
          strip(c, u0, v0, u1, v0 + 2.4);
          strip(c, u0, v1 - 2.4, u1, v1);
          strip(c, u0, v0, u0 + 2.4, v1);
          strip(c, u1 - 2.4, v0, u1, v1);
        }
        bg.globalAlpha = 1;
      }
    }
    const shape = this.previewShape();
    if (!shape) return;
    const color = TOOL_COLOR[this.tool];
    const wave = this.calm ? 0.5 : 0.5 + 0.5 * Math.sin(this.time * 6);
    const [r, g2, b] = parse(color);
    for (const [x, y] of shape.tiles) {
      const [sx, sy] = screenOf(x, y);
      const h = this.height[y * this.n + x];
      const oy = sy - h;
      fp(bg, `rgba(${r},${g2},${b},${(0.3 + wave * 0.14).toFixed(2)})`, [[sx, oy], [sx + HW, oy + HH], [sx, oy + 32], [sx - HW, oy + HH]]);
    }
    // An outline round the whole shape.
    const xs = shape.tiles.map((t2) => t2[0]);
    const ys = shape.tiles.map((t2) => t2[1]);
    const x0 = Math.min(...xs) * 32;
    const x1 = (Math.max(...xs) + 1) * 32;
    const y0 = Math.min(...ys) * 32;
    const y1 = (Math.max(...ys) + 1) * 32;
    const th = 1.8;
    strip(color, x0, y0, x1, y0 + th);
    strip(color, x0, y1 - th, x1, y1);
    strip(color, x0, y0, x0 + th, y1);
    strip(color, x1 - th, y0, x1, y1);
  }

  previewShape() {
    if (!this.tool) return null;
    const g = this.gesture;
    if (g) return this.shapeFor(g.sx, g.sy, g.ex, g.ey);
    if (this.tile && this.hoverIsMouse) return this.shapeFor(this.tile.x, this.tile.y, this.tile.x, this.tile.y);
    return null;
  }

  shapeFor(sx, sy, ex, ey) {
    const tiles = [];
    if (this.tool === 'road') {
      const max = Math.max(1, this.limits.road) - 1;
      const dx = ex - sx;
      const dy = ey - sy;
      if (Math.abs(dx) >= Math.abs(dy)) {
        ex = sx + clamp(dx, -max, max);
        ey = sy;
      } else {
        ex = sx;
        ey = sy + clamp(dy, -max, max);
      }
    } else if (this.tool === 'square' || this.tool === 'green') {
      // W91: a park or a square takes the tile and the three across and down from it.
      sx = ex;
      sy = ey;
      const tiles2 = [[sx, sy], [sx + 1, sy], [sx, sy + 1], [sx + 1, sy + 1]];
      return { tiles: tiles2, x: sx, y: sy, x2: sx, y2: sy };
    } else {
      sx = ex;
      sy = ey;
    }
    for (let y = Math.min(sy, ey); y <= Math.max(sy, ey); y++) {
      for (let x = Math.min(sx, ex); x <= Math.max(sx, ex); x++) tiles.push([x, y]);
    }
    return { tiles, x: sx, y: sy, x2: ex, y2: ey };
  }

  // --- Pointing -----------------------------------------------------------------------------

  bindEvents() {
    const c = this.canvas;
    this.onDown = (e) => this.pointerDown(e);
    this.onMove = (e) => this.pointerMove(e);
    this.onUp = (e) => this.pointerUp(e);
    this.onLeave = () => {
      if (!this.pointers.size) this.clearHover();
    };
    this.onWheel = (e) => this.wheel(e);
    this.onKey = (e) => this.key(e);
    // The browser's own menu is never wanted on the map. Where a page asked for picks, a right click,
    // the menu key and a touch-screen long press open its menu instead.
    this.onMenu = (e) => {
      if (!this.interactive) return;
      e.preventDefault();
      if (!this.onPick) return;
      this.gesture = null;
      this.panning = false;
      this.setCursorState();
      // The menu key comes with no pointer: use the middle of the map.
      let x = e.clientX;
      let y = e.clientY;
      if (!e.clientX && !e.clientY) {
        const r = this.canvas.getBoundingClientRect();
        x = r.left + r.width / 2;
        y = r.top + r.height / 2;
      }
      this.pickAt(x, y, 'context');
      this.kick();
    };
    c.addEventListener('pointerdown', this.onDown);
    c.addEventListener('pointermove', this.onMove);
    c.addEventListener('pointerup', this.onUp);
    c.addEventListener('pointercancel', this.onUp);
    c.addEventListener('pointerleave', this.onLeave);
    c.addEventListener('contextmenu', this.onMenu);
    if (this.interactive) {
      c.addEventListener('wheel', this.onWheel, { passive: false });
      c.addEventListener('keydown', this.onKey);
    }
  }

  local(e) {
    const r = this.canvas.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  toArt(px, py) {
    const z = this.cam.zoom;
    return [this.cam.x + (px - this.cssW / 2) / z, this.cam.y + (py - this.cssH / 2) / z];
  }

  tileAtArt(X, Y) {
    if (!this.ready) return null;
    const n = this.n;
    let h = 0;
    let x = 0;
    let y = 0;
    for (let k = 0; k < 3; k++) {
      const tx = (Y + h) / (2 * HH) + X / (2 * HW);
      const ty = (Y + h) / (2 * HH) - X / (2 * HW);
      x = Math.floor(tx);
      y = Math.floor(ty);
      if (x < 0 || y < 0 || x >= n || y >= n) return null;
      const nh = this.height[y * n + x];
      if (nh === h) break;
      h = nh;
    }
    return { x, y };
  }

  pickEntity(px, py) {
    const [X, Y] = this.toArt(px, py);
    const reach = Math.max(14, 22 / this.cam.zoom);
    let best = null;
    let bestD = Infinity;
    const test = (e, lift, r) => {
      if (e.sx === undefined || e.leaving) return;
      const d = Math.hypot(e.sx - X, e.sy - lift - Y);
      if (d < r && d < bestD) {
        best = e;
        bestD = d;
      }
    };
    for (const map of [this.players, this.people, this.visitors]) {
      for (const p of map.values()) if (!this.indoors(p)) test(p, p.kind === 'person' ? 13 : 16, reach);
    }
    for (const f of this.frogs.values()) test(f, 9, reach * 1.1);
    return best;
  }

  setCursorState() {
    this.container.classList.toggle('world-grabbing', this.panning);
  }

  pointerDown(e) {
    if (!this.interactive || !this.ready) return;
    if (e.pointerType === 'mouse' && e.button > 2) return;
    // A right click is for the menu, and doesn't drag the map.
    if (e.pointerType === 'mouse' && e.button === 2 && this.onPick) {
      this.canvas.focus({ preventScroll: true });
      return;
    }
    try {
      this.canvas.setPointerCapture(e.pointerId);
    } catch {
      // Some browsers refuse for synthetic events; dragging still works without it.
    }
    const p = this.local(e);
    this.pointers.set(e.pointerId, { x: p.x, y: p.y, sx: p.x, sy: p.y, button: e.button, type: e.pointerType });
    this.canvas.focus({ preventScroll: true });
    this.cam.vx = this.cam.vy = 0;
    this.cam.move = null;
    clearTimeout(this.pressTimer);
    this.pressed = false;
    if (this.pointers.size >= 2) {
      this.gesture = null;
      this.panning = false;
      this.stopFollow('moved');
      this.beginPinch();
      return;
    }
    // A finger held down in one place is a right click.
    if (this.onPick && e.pointerType !== 'mouse') {
      const id = e.pointerId;
      this.pressTimer = setTimeout(() => {
        const rec = this.pointers.get(id);
        if (!rec || this.moved || this.pinch || this.destroyed) return;
        this.pressed = true;
        this.gesture = null;
        this.panning = false;
        this.setCursorState();
        const r = this.canvas.getBoundingClientRect();
        this.pickAt(r.left + rec.x, r.top + rec.y, 'press');
        this.kick();
      }, 520);
    }
    this.moved = false;
    this.vel = { x: 0, y: 0, t: performance.now() };
    const wantsTool = this.tool && e.button === 0 && !e.shiftKey && !e.altKey;
    if (wantsTool) {
      const tile = this.tileAtArt(...this.toArt(p.x, p.y));
      if (tile) {
        this.gesture = { sx: tile.x, sy: tile.y, ex: tile.x, ey: tile.y };
        this.tile = tile;
      }
    } else {
      this.panning = true;
      this.setCursorState();
    }
    this.kick();
  }

  beginPinch() {
    const [a, b] = [...this.pointers.values()];
    const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
    const [ax, ay] = this.toArt(mid.x, mid.y);
    this.pinch = { d0: Math.hypot(a.x - b.x, a.y - b.y) || 1, z0: this.cam.zoom, ax, ay };
    this.cam.tz = this.cam.zoom;
    this.cam.anchor = null;
  }

  pointerMove(e) {
    if (!this.ready) return;
    const p = this.local(e);
    const rec = this.pointers.get(e.pointerId);
    if (!rec) {
      this.hoverIsMouse = e.pointerType === 'mouse';
      if (e.pointerType === 'mouse') this.updateHover(p);
      return;
    }
    const dx = p.x - rec.x;
    const dy = p.y - rec.y;
    rec.x = p.x;
    rec.y = p.y;
    if (Math.hypot(p.x - rec.sx, p.y - rec.sy) > (rec.type === 'mouse' ? 5 : 9)) {
      this.moved = true;
      clearTimeout(this.pressTimer);
    }
    if (this.pinch && this.pointers.size >= 2) {
      const [a, b] = [...this.pointers.values()];
      const d = Math.hypot(a.x - b.x, a.y - b.y) || 1;
      const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
      const zoom = clamp(this.pinch.z0 * (d / this.pinch.d0), this.minZoom(), MAX_ZOOM);
      this.cam.zoom = this.cam.tz = zoom;
      this.cam.x = this.pinch.ax - (mid.x - this.cssW / 2) / zoom;
      this.cam.y = this.pinch.ay - (mid.y - this.cssH / 2) / zoom;
      this.clampCam();
      this.kick();
      return;
    }
    if (this.gesture) {
      const tile = this.tileAtArt(...this.toArt(p.x, p.y));
      if (tile) {
        this.gesture.ex = tile.x;
        this.gesture.ey = tile.y;
        this.tile = tile;
      }
      this.kick();
      return;
    }
    if (this.panning) {
      if (this.moved) this.stopFollow('moved');
      const z = this.cam.zoom;
      this.cam.x -= dx / z;
      this.cam.y -= dy / z;
      const now = performance.now();
      const dt = Math.max(1, now - this.vel.t) / 1000;
      this.vel.x = lerp(this.vel.x, -dx / z / dt, 0.4);
      this.vel.y = lerp(this.vel.y, -dy / z / dt, 0.4);
      this.vel.t = now;
      this.clampCam();
      this.hideTip();
      this.kick();
    }
  }

  pointerUp(e) {
    const rec = this.pointers.get(e.pointerId);
    if (!rec) return;
    this.pointers.delete(e.pointerId);
    clearTimeout(this.pressTimer);
    try {
      this.canvas.releasePointerCapture(e.pointerId);
    } catch {
      // Nothing to release.
    }
    // The press already opened its menu. Lifting the finger is not a click.
    if (this.pressed) {
      this.pressed = false;
      this.gesture = null;
      this.panning = false;
      this.setCursorState();
      this.kick();
      return;
    }
    if (this.pinch) {
      if (this.pointers.size < 2) this.pinch = null;
      if (this.pointers.size === 1) {
        // The finger left behind carries on as a pan.
        this.panning = true;
        this.vel = { x: 0, y: 0, t: performance.now() };
      }
      this.setCursorState();
      this.kick();
      return;
    }
    const clicked = this.panning && !this.moved && e.type === 'pointerup' && rec.button === 0;
    if (this.gesture && e.type === 'pointerup') {
      const shape = this.shapeFor(this.gesture.sx, this.gesture.sy, this.gesture.ex, this.gesture.ey);
      this.gesture = null;
      if (this.onTile) this.onTile({ x: shape.x, y: shape.y, x2: shape.x2, y2: shape.y2 });
    } else {
      this.gesture = null;
    }
    // With no tool in hand, a click on a figure opens its menu.
    if (clicked && !this.tool) {
      const r = this.canvas.getBoundingClientRect();
      this.pickAt(r.left + rec.x, r.top + rec.y, 'click');
    }
    if (this.panning) {
      this.panning = false;
      const speed = Math.hypot(this.vel.x, this.vel.y);
      if (!this.calm && this.moved && speed > 40 && performance.now() - this.vel.t < 80) {
        this.cam.vx = clamp(this.vel.x, -1500, 1500);
        this.cam.vy = clamp(this.vel.y, -1500, 1500);
      }
    }
    this.setCursorState();
    this.kick();
  }

  wheel(e) {
    if (!this.ready) return;
    e.preventDefault();
    const p = this.local(e);
    let dy = e.deltaY;
    if (e.deltaMode === 1) dy *= 16;
    else if (e.deltaMode === 2) dy *= 100;
    if (e.ctrlKey) {
      this.zoomTo(this.cam.tz * Math.exp(-dy * 0.012), p);
      return;
    }
    const now = performance.now();
    if (now - this.wheelAt < 90 && Math.abs(dy) < 40) return;
    if (Math.abs(dy) < 4) return;
    this.wheelAt = now;
    this.zoomStep(dy < 0 ? 1 : -1, p);
  }

  zoomStep(dir, p) {
    const z = this.cam.tz;
    let target = z;
    if (dir > 0) target = ZOOM_STEPS.find((s) => s > z * 1.02) ?? MAX_ZOOM;
    else target = [...ZOOM_STEPS].reverse().find((s) => s < z * 0.98) ?? MIN_ZOOM;
    this.zoomTo(target, p);
  }

  zoomTo(zoom, p) {
    const at = p || { x: this.cssW / 2, y: this.cssH / 2 };
    const next = clamp(zoom, this.minZoom(), MAX_ZOOM);
    const [ax, ay] = this.toArt(at.x, at.y);
    this.cam.tz = next;
    this.cam.anchor = { ax, ay, sx: at.x - this.cssW / 2, sy: at.y - this.cssH / 2 };
    if (this.calm) {
      this.cam.zoom = next;
      this.cam.x = ax - this.cam.anchor.sx / next;
      this.cam.y = ay - this.cam.anchor.sy / next;
      this.cam.anchor = null;
      this.clampCam();
    }
    this.kick();
  }

  key(e) {
    if (!this.ready) return;
    const step = 90 / this.cam.zoom;
    let handled = true;
    switch (e.key) {
      case 'ArrowLeft':
        this.nudge(-step, 0);
        break;
      case 'ArrowRight':
        this.nudge(step, 0);
        break;
      case 'ArrowUp':
        this.nudge(0, -step);
        break;
      case 'ArrowDown':
        this.nudge(0, step);
        break;
      case '+':
      case '=':
        this.zoomStep(1);
        break;
      case '-':
      case '_':
        this.zoomStep(-1);
        break;
      case '0': {
        const [cx, cy] = screenOf(this.n / 2, this.n / 2);
        this.cam.tz = this.startZoom();
        this.cam.move = { x0: this.cam.x, y0: this.cam.y, x1: cx, y1: cy, t: 0, dur: 0.6 };
        break;
      }
      case 'Escape':
        this.gesture = null;
        break;
      default:
        handled = false;
    }
    if (handled) {
      e.preventDefault();
      this.kick();
    }
  }

  nudge(dx, dy) {
    this.stopFollow('moved');
    const to = this.clampPoint(this.cam.x + dx, this.cam.y + dy);
    this.cam.move = { x0: this.cam.x, y0: this.cam.y, x1: to[0], y1: to[1], t: 0, dur: this.calm ? 0.0001 : 0.25 };
  }

  // --- Hovering -----------------------------------------------------------------------------

  updateHover(p) {
    const ent = this.pickEntity(p.x, p.y);
    const tile = this.tileAtArt(...this.toArt(p.x, p.y));
    this.tile = tile;
    this.hoverEnt = ent;
    const key = ent ? `${ent.type}:${ent.id}` : tile ? `t:${tile.x},${tile.y}` : '';
    if (ent) this.showTip(ent);
    else this.hideTip();
    if (key !== this.hoverKey) {
      this.hoverKey = key;
      this.container.classList.toggle('world-over', Boolean(ent));
      if (this.onHover) {
        const i = tile ? tile.y * this.n + tile.x : -1;
        this.onHover(
          key
            ? {
                x: tile ? tile.x : null,
                y: tile ? tile.y : null,
                tile: tile ? { kind: KIND_LABEL[this.kind[i]] ? this.kindName(i) : null, level: this.level[i] } : null,
                player: ent && ent.type === 'player' ? { id: ent.id, name: ent.name, emote: ent.emote, kind: ent.kind } : null,
                person: ent && ent.type === 'person' ? { name: ent.name } : null,
                visitor: ent && ent.type === 'visitor' ? { name: ent.name } : null,
                crawler: ent && ent.type === 'frog' ? { kind: ent.id, name: ent.name } : null,
              }
            : null,
        );
      }
    }
    this.kick();
  }

  kindName(i) {
    return KIND_NAME[this.kind[i]] || null;
  }

  clearHover() {
    this.tile = null;
    this.hoverEnt = null;
    this.hoverIsMouse = false;
    if (this.hoverKey && this.onHover) this.onHover(null);
    this.hoverKey = '';
    this.container.classList.remove('world-over');
    this.hideTip();
    this.kick();
  }

  showTip(ent) {
    const text = ent.type === 'frog' ? `${ent.name}, ${seenText(ent.age)}` : ent.name;
    if (this.tip.textContent !== text) this.tip.textContent = text;
    this.tip.dataset.kind = ent.type;
    this.tip.hidden = false;
    this.tipEnt = ent;
  }

  hideTip() {
    this.tip.hidden = true;
    this.tipEnt = null;
  }

  placeTip() {
    const e = this.tipEnt;
    if (!e || this.tip.hidden) return;
    if (e.sx === undefined) return;
    const lift = e.type === 'frog' ? 32 : e.kind === 'person' ? 40 : 54;
    const z = this.cam.zoom;
    const x = (e.sx - this.cam.x) * z + this.cssW / 2;
    const y = (e.sy - lift - this.cam.y) * z + this.cssH / 2;
    this.tip.style.left = `${Math.round(clamp(x, 8, this.cssW - 8))}px`;
    this.tip.style.top = `${Math.round(Math.max(18, y))}px`;
  }

  // @@METHODS
}

// @@WORLD_END
