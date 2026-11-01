// The outside view and its Build panel (docs/outside.md). Pressing Build outside on your own place lays
// a stage over the map: the one tile, large, with the Build catalog beside it, the same tiles in the same
// kind of list as a room's (I45, E12). The page holds the document. Every change makes a new one and
// sends the whole of it, and only the server's word counts (E25), so a refusal puts back the last one it
// took and says why.
//
// What this page works out (where a thing would fit, which tile is pressed) only decides what to offer
// and what colour to draw a ghost. A click on a red cell still goes to the server (E16).

import { paintPlace } from './world.js';
import { CELL, CENTERPIECE, GRID, YARD_SIZES, bodyOf, doorSideOf, hexOf, keyOf, partOf } from './outside-spec.js';
import { defaultStyles, doorPoint } from './outside-building.js';
import { drawPet, makePet, petPicture, stepPet } from './pets.js';

// E3: a colour is a whole number from 0 to 16777215.
const MAX_COLOR = 0xffffff;
const MAX_UNDO = 30;
// How long a dragged colour picker is left alone before it is saved.
const SAVE_WAIT = 450;
const UNIT = CELL;

const NUDGES = [['←', 'left', -1, 0], ['↑', 'back', 0, -1], ['↓', 'forward', 0, 1], ['→', 'right', 1, 0]];
const NUDGE_KEYS = { ArrowLeft: [-1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1], ArrowRight: [1, 0] };
// E11: the bits of a fence's `number`, named by where the edge is on the screen. The tile's top corner
// is the back, so the edge from it to the right corner is the back right.
const SIDES = [[1, 'Back right'], [2, 'Front right'], [4, 'Front left'], [8, 'Back left']];
// E13: the sizes offered, as width and depth in cells. `null` is what a place has with no `body`.
// E14: a square and a park have no building, and both take 2 by 2 tiles (E36).
const hasNoBuilding = (kind) => kind === 'square' || kind === 'green';
const SIZES = [['small', 'Small', 7, 6], ['standard', 'Standard', null, null], ['wide', 'Wide', 12, 8], ['deep', 'Deep', 8, 12], ['large', 'Large', 13, 12]];

const SWATCH_NAMES = new Map([
  [0xffffff, 'White'], [0xf7f1e3, 'Cream'], [0xf2d3a0, 'Sand'], [0xd9b48f, 'Tan'], [0xa9573f, 'Clay'], [0x7b4a2a, 'Brown'],
  [0x3e2723, 'Dark brown'], [0xe5484d, 'Red'], [0xff8a65, 'Coral'], [0xffb74d, 'Orange'], [0xffd54f, 'Yellow'],
  [0xc5e1a5, 'Pale green'], [0x6f9d3d, 'Green'], [0x2f7a58, 'Dark green'], [0x4db6ac, 'Teal'], [0x8fcbec, 'Sky blue'],
  [0x2f5d9a, 'Blue'], [0x1a237e, 'Navy'], [0xb39ddb, 'Lavender'], [0x6b3f86, 'Purple'], [0xf48fb1, 'Pink'],
  [0x9e9e9e, 'Grey'], [0x5d6474, 'Slate'], [0x212121, 'Black'],
]);

// What a catalog picture shows of a tile, as [x0, y0, x1, y1] in the units a tile is painted in (the top
// corner is 0, 0), so the pictures of one part are all cropped the same and differ only in the thing.
const WHOLE = [-44, -54, 44, 42];
const SIZES_IN_VIEW = [-36, -34, 36, 38];
const ROOFS = [-26, -33, 26, 3];
const FRONT = [-24, -18, 24, 30];
const DOORWAY = [-22, -12, 22, 30];
const YARD = [-36, -4, 36, 40];
const CROPS = {
  roof: ROOFS, roofing: ROOFS, rooftop: ROOFS, chimney: ROOFS,
  door: DOORWAY, porch: DOORWAY, awning: DOORWAY, door_side: DOORWAY, sign: DOORWAY, gate: DOORWAY,
  fence: YARD, ground: YARD, path: YARD, centerpiece: [-30, -26, 30, 34], string_lights: WHOLE,
};
// The size of a tile's picture, in CSS pixels.
const PIC_W = 96;
const PIC_H = 66;

const P = (u, v) => [u - v, (u + v) / 2];
const overlaps = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
const clamp = (value, low, high) => Math.max(low, Math.min(high, value));
const clone = (doc) => JSON.parse(JSON.stringify(doc || {}));
const wordName = (word) => String(word).charAt(0).toUpperCase() + String(word).slice(1).replaceAll('_', ' ');
const hexText = (value) => `#${value.toString(16).padStart(6, '0')}`;
const colorName = (value) => SWATCH_NAMES.get(value) || hexText(value);
const isWhole = (value) => Number.isInteger(value);
const same = (a, b) => keyOf(a) === keyOf(b);

// E1: an outside with nothing in it is no outside at all, so whatever has emptied is taken out.
function tidy(doc) {
  const out = { ...doc };
  if (out.parts) {
    out.parts = Object.fromEntries(Object.entries(out.parts).filter(([, part]) => part && Object.keys(part).length));
    if (!Object.keys(out.parts).length) delete out.parts;
  }
  for (const word of ['items', 'pets']) if (!out[word]?.length) delete out[word];
  if (!out.body) delete out.body;
  return out;
}

function setPart(doc, word, field, value) {
  doc.parts ||= {};
  doc.parts[word] ||= {};
  if (value === undefined) delete doc.parts[word][field];
  else doc.parts[word][field] = value;
}

// A native colour picker gives '#rrggbb', and E3 wants red times 65536 plus green times 256 plus blue.
const numberOfHex = (text) => (/^#[0-9a-f]{6}$/i.test(text) ? parseInt(text.slice(1), 16) : null);

// Everything the stage needs from the page that holds it:
//   frame, world, panels      the map's window, the map itself and its panels
//   beside                    a media query: wide enough for a panel to stand beside the stage
//   current(x, y)             what the map has for a tile, for when the server can't be asked
//   changed(x, y, outside)    told what the server stored, so the map shows it at once (E27)
export function outsideBuilder({ h, api, modal, popupMenu, formError, frame, world, panels, beside, say, current, changed }) {
  const coarse = matchMedia('(pointer: coarse)');
  const reduced = matchMedia('(prefers-reduced-motion: reduce)');
  const well = frame.host.parentElement;
  const menu = popupMenu(frame.root, { refocus: () => doneButton.focus() });

  let active = false;
  let tile = null;
  let kind = 'house';
  let look = { door: 'L', variant: 0, pal: 0 };
  let catalog = null;
  let yardKinds = new Map();
  // The last document the server took, and the one on the stage now, which is ahead of it while a change
  // is on its way.
  let accepted = {};
  let shown = {};
  let undo = [];
  let chain = Promise.resolve();
  let flying = 0;
  let timer = null;
  let night = false;
  let hand = null;
  let chosen = null;
  let press = { touch: false, onHand: false };
  let busy = false;
  // True while a native colour picker is open, because drawing the panel again would shut it. A picker
  // that was closed without a change leaves the flag up, so it only counts while the input has the focus.
  let picking = false;
  const pickerOpen = () => picking && document.activeElement?.type === 'color';
  // Each opening is its own. A reply that comes back to a later one is about a different tile (E25).
  let session = 0;

  const dpr = () => Math.min(2, window.devicePixelRatio || 1);
  const urlOf = () => `/api/places/${encodeURIComponent(tile.x)}/${encodeURIComponent(tile.y)}/outside`;

  // The dock under the stage says what's going on and holds what's needed for it (as a room's does).
  const sayHere = h('span', { class: 'build-say', role: 'status' });
  const tell = (text) => {
    sayHere.textContent = text;
  };
  const dockButton = (label, onclick) => h('button', { type: 'button', class: 'wd-btn wide', onclick }, label);
  const turnButton = dockButton('Turn', () => turnHand());
  const putButton = dockButton('Put it here', () => putDown());
  const cancelButton = dockButton('Cancel', () => letGo());
  const catalogButton = dockButton('Catalog', () => panels.open('outside'));
  const seeButton = dockButton('See the place', () => clearPanel());
  const undoButton = dockButton('Undo', () => undoLast());
  const nightButton = dockButton('See it at night', () => setNight(!night));
  const doneButton = dockButton('Done', () => finish());
  const nudges = h('span', { class: 'build-nudge', role: 'group', 'aria-label': 'Move it one cell' },
    NUDGES.map(([glyph, word, dx, dy]) => h('button', {
      type: 'button',
      class: 'wd-btn',
      'aria-label': `Move it ${word}`,
      onclick: () => nudge(dx, dy),
    }, glyph)));
  const dock = h('div', { class: 'wd-bar build-dock outside-dock', role: 'toolbar', 'aria-label': 'Build outside', hidden: true },
    sayHere,
    h('span', { class: 'build-dock-buttons' }, nudges, turnButton, putButton, cancelButton, catalogButton, seeButton, undoButton, nightButton, doneButton));
  frame.root.append(dock);

  // The stage: one canvas over the map's well. The map keeps going underneath.
  const canvas = h('canvas', { class: 'outside-canvas', role: 'img', 'aria-label': 'The outside of your place' });
  const stage = h('div', { class: 'outside-stage', hidden: true }, canvas);
  well.append(stage);

  // The panel
  const figures = h('p', { class: 'build-figures' });
  const buildError = formError();
  const catalogEl = h('div', { class: 'build-catalog' }, h('p', { class: 'muted' }, 'Opening the catalog…'));
  const yardEl = h('div', { class: 'build-placed outside-yard', hidden: true });
  const tab = (label, shows, pressed) => h('button', {
    type: 'button',
    class: 'wd-btn wide',
    'aria-pressed': String(pressed),
    onclick: () => showTab(shows),
  }, label);
  const catalogTab = tab('Catalog', catalogEl, true);
  const yardTab = tab('In this yard', yardEl, false);
  const startOver = h('button', { type: 'button', class: 'link danger', onclick: () => askStartOver() }, 'Start over');
  panels.add('outside', 'Build', { onOpen: () => render(true) },
    figures,
    h('div', { class: 'build-tabs' }, catalogTab, yardTab),
    buildError,
    catalogEl,
    yardEl,
    h('div', { class: 'outside-foot' }, startOver));
  const toggle = panels.toggle('outside', 'Build', { short: 'Build' });
  toggle.hidden = true;
  frame.actions.prepend(toggle);

  function showTab(shows) {
    catalogEl.hidden = shows !== catalogEl;
    yardEl.hidden = shows !== yardEl;
    catalogTab.setAttribute('aria-pressed', String(shows === catalogEl));
    yardTab.setAttribute('aria-pressed', String(shows === yardEl));
  }

  // Wide enough, the stage leaves room for the panel. Narrower, the panel covers it.
  beside.addEventListener('change', () => renderDock());
  const priorChange = panels.onChange;
  panels.onChange = () => {
    priorChange?.();
    frame.root.classList.toggle('outside-panel', active && panels.current === 'outside');
    if (active) renderDock();
  };

  // --- The document ---------------------------------------------------------------------------

  const edit = (change) => {
    const next = clone(shown);
    change(next);
    return tidy(next);
  };

  const limits = () => catalog?.limits || { items: 40, pets: 4, rules: 6 };
  const sizeOf = (word) => {
    const known = yardKinds.get(word);
    return known ? [known.w, known.h] : YARD_SIZES[word] || [1, 1];
  };
  const labelOfKind = (word) => yardKinds.get(word)?.label || wordName(word);

  // Where each item stands, in cells as it's turned now, with its place in the list.
  function footprints(doc, skip = -1) {
    const out = [];
    (Array.isArray(doc.items) ? doc.items : []).forEach((item, index) => {
      if (index === skip || !item || !isWhole(item.x) || !isWhole(item.y)) return;
      const [w, tall] = sizeOf(item.kind);
      const turned = (isWhole(item.facing) ? item.facing : 0) & 1;
      out.push({ index, kind: item.kind, x: item.x, y: item.y, w: turned ? tall : w, h: turned ? w : tall });
    });
    return out;
  }

  // E14, E16: what is already standing on the tile, which nothing else can stand on.
  function solidOf(doc) {
    if (hasNoBuilding(kind)) {
      // A square has a fountain unless it says `none`, and a park has nothing until one is chosen.
      const style = partOf(doc, 'centerpiece').style;
      return (typeof style === 'string' ? style : kind === 'green' ? 'none' : 'fountain') === 'none' ? null : { x: CENTERPIECE.x, y: CENTERPIECE.y, w: CENTERPIECE.w, h: CENTERPIECE.h };
    }
    const body = bodyOf(kind, doc);
    return body && { x: body.x, y: body.y, w: body.w, h: body.d };
  }

  function itemFits(doc, spot, skip = -1) {
    if (spot.x < 0 || spot.y < 0 || spot.x + spot.w > GRID || spot.y + spot.h > GRID) return false;
    const solid = solidOf(doc);
    if (solid && overlaps(spot, solid)) return false;
    return !footprints(doc, skip).some((each) => overlaps(spot, each));
  }

  // E13, E16: a building stays a cell in from the edges and off the yard's things.
  function bodyFits(doc, spot) {
    return spot.x >= 1 && spot.y >= 1 && spot.x + spot.w <= GRID - 1 && spot.y + spot.h <= GRID - 1
      && !footprints(doc).some((each) => overlaps(spot, each));
  }

  const fits = (spot) => (hand.type === 'building' ? bodyFits(shown, spot) : itemFits(shown, spot, hand.index ?? -1));

  // The first place something fits, so a finger has something to start from, or else the corner.
  function firstSpot(w, tall) {
    for (let y = 0; y + tall <= GRID; y++) {
      for (let x = 0; x + w <= GRID; x++) if (itemFits(shown, { x, y, w, h: tall })) return { x, y };
    }
    return { x: 0, y: 0 };
  }

  // E23: the spot nearest where a template had it that fits, or nothing.
  function nearestSpot(doc, kindWord, want) {
    const [w, tall] = sizeOf(kindWord);
    let best = null;
    let bestD = Infinity;
    for (let y = 0; y + tall <= GRID; y++) {
      for (let x = 0; x + w <= GRID; x++) {
        const d = (x - want.x) ** 2 + (y - want.y) ** 2;
        if (d < bestD && itemFits(doc, { x, y, w: want.facing & 1 ? tall : w, h: want.facing & 1 ? w : tall })) {
          best = { x, y };
          bestD = d;
        }
      }
    }
    return best;
  }

  // --- Saving ---------------------------------------------------------------------------------

  function fail(error) {
    buildError.textContent = error.message;
    buildError.hidden = false;
    tell(error.message);
  }

  function adopt(doc, where = tile) {
    accepted = doc;
    changed(where.x, where.y, doc);
  }

  // E6: the place has gone, so there is nothing left to change.
  function gone() {
    if (!active) return;
    const { x, y } = tile;
    changed(x, y, {});
    close();
    say('That place is gone, so there is nothing to build on.');
  }

  // One change at a time, in order. Resolves true once the server has taken it.
  function save(next, { remember = true } = {}) {
    clearTimeout(timer);
    timer = null;
    buildError.hidden = true;
    shown = next;
    repaint();
    render();
    flying += 1;
    const era = session;
    const where = tile;
    const url = urlOf();
    const mine = chain.then(async () => {
      try {
        const reply = await api(url, { method: 'PUT', body: next });
        const stored = reply.outside && typeof reply.outside === 'object' ? reply.outside : next;
        // Opened again since: this is about the last tile, and the new one isn't its business.
        if (era !== session) {
          changed(where.x, where.y, stored);
          return true;
        }
        if (remember && !same(accepted, stored)) undo = [...undo, accepted].slice(-MAX_UNDO);
        adopt(stored, where);
        flying -= 1;
        // A colour still being dragged is ahead of this reply and goes on being shown.
        if (!flying && !timer && !same(shown, stored)) {
          shown = stored;
          repaint();
        }
        render();
        return true;
      } catch (error) {
        if (era !== session) return false;
        flying -= 1;
        if (!flying && !timer) {
          shown = accepted;
          repaint();
        }
        if (error.status === 404) {
          gone();
          return false;
        }
        fail(error.status === 401 ? new Error('You have been signed out, so this was not saved. Sign in again to go on.') : error);
        render();
        return false;
      }
    });
    chain = mine;
    return mine;
  }

  // A colour being dragged is drawn at once and saved when it settles.
  function later(next) {
    shown = next;
    repaint();
    clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      save(next);
    }, SAVE_WAIT);
  }

  async function flushed() {
    if (timer) {
      clearTimeout(timer);
      timer = null;
      save(shown);
    }
    await chain;
  }

  async function commit(next, message) {
    const ok = await save(next);
    if (ok && message) tell(message);
    return ok;
  }

  async function undoLast() {
    await flushed();
    if (!undo.length) return;
    const back = undo[undo.length - 1];
    undo = undo.slice(0, -1);
    const ok = await save(back, { remember: false });
    if (ok) tell('Undone.');
    else undo = [...undo, back];
  }

  function askStartOver() {
    const dialog = modal('Start over?',
      h('p', {}, 'Everything you made on this tile goes: the look of the building, the yard and the pets. Undo can bring it back until you leave.'),
      h('div', { class: 'modal-actions' },
        h('button', { type: 'button', onclick: () => dialog.close() }, 'Keep it'),
        h('button', {
          type: 'button',
          class: 'primary',
          // An empty document is what clearing is (E1), and it goes in line with the rest.
          onclick: () => {
            dialog.close();
            clearTimeout(timer);
            timer = null;
            commit({}, 'Back to how it started.');
          },
        }, 'Start over')));
  }

  // --- The stage ------------------------------------------------------------------------------

  let view = { s: 3, ox: 0, oy: 0, w: 1, h: 1, q: 1 };
  let layers = null;
  let raf = 0;
  let last = 0;
  let clock = 0;
  let again = true;
  let pets = [];
  let petKey = '';
  let ghost = null;
  // Where the ghost's picture is painted, in cells, and then moved from.
  const REF = 6;
  const resizer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => fit());

  // The document as the stage draws it: what's in hand isn't also standing in its old place.
  function stageDoc() {
    if (hand?.index === undefined || hand.index === null || hand.type !== 'item') return shown;
    return edit((doc) => {
      doc.items.splice(hand.index, 1);
    });
  }

  // How far up and out this design reaches, found by painting it small and looking at what was painted, so
  // a one-floor house isn't fitted as if it were the tallest thing a building could be. It is only worked
  // out again when the shape changes, and not for a colour. In tile units: `top` is negative.
  let reach = { key: '', top: -30, half: 38 };
  function measure() {
    const doc = stageDoc();
    const key = [kind, look.door, look.pal, look.variant, JSON.stringify(doc, (word, value) => (word === 'color' || word === 'coat' || word === 'collar' || word === 'pets' ? undefined : value))].join('|');
    if (reach.key === key) return reach;
    const S = 2;
    const [W, H, OX, OY] = [240, 380, 120, 270];
    let top = -30;
    let half = 38;
    try {
      const c = document.createElement('canvas');
      c.width = W;
      c.height = H;
      const g = c.getContext('2d', { willReadFrequently: true });
      g.setTransform(S, 0, 0, S, OX, OY);
      paintPlace(g, { kind, outside: doc, door: doorSideOf(doc) || look.door, pal: look.pal, variant: look.variant, day: true, layer: 'standing' });
      const data = g.getImageData(0, 0, W, H).data;
      let [y0, x0, x1] = [H, W, -1];
      for (let y = 0; y < H; y++) {
        for (let x = 0; x < W; x++) {
          if (data[(y * W + x) * 4 + 3] < 24) continue;
          if (y < y0) y0 = y;
          if (x < x0) x0 = x;
          if (x > x1) x1 = x;
        }
      }
      if (x1 >= 0) {
        // Rounded up to a step of 8, so small changes don't make the stage jump about.
        top = Math.min(-30, Math.floor(((y0 - OY) / S - 3) / 8) * 8);
        half = Math.max(38, Math.ceil(Math.max(OX - x0, x1 + 1 - OX) / S) + 3);
      }
    } catch (error) {
      console.error(error);
    }
    reach = { key, top, half };
    return reach;
  }

  function fit() {
    if (!active) return;
    const box = stage.getBoundingClientRect();
    const w = Math.floor(box.width);
    const tall = Math.floor(box.height);
    if (w < 8 || tall < 8) return;
    // Big screens with a sharp display would hold a lot of pixels, so the quality gives way first.
    const q = Math.max(1, Math.min(dpr(), Math.sqrt(2.6e6 / (w * tall))));
    // The whole tile in view, with its edge under it and just room over it for what stands on it.
    const { top: up, half } = measure();
    const s = clamp(Math.min((w - 16) / (half * 2), (tall - 16) / (44 - up)), 1.2, 9);
    const top = up * s;
    const bottom = 44 * s;
    view = { s, ox: w / 2, oy: (tall - (bottom - top)) / 2 - top, w, h: tall, q };
    canvas.width = Math.round(w * q);
    canvas.height = Math.round(tall * q);
    canvas.style.width = `${w}px`;
    canvas.style.height = `${tall}px`;
    layers = null;
    ghost = null;
    kick();
  }

  function repaint() {
    if (!active) return;
    layers = null;
    const before = reach;
    resync();
    // A change of height or width fits the stage again (and a colour never does).
    if (measure() !== before) fit();
    kick();
  }

  function kick() {
    again = true;
    if (!raf && active) raf = requestAnimationFrame(tick);
  }

  // A tile's top, as the diamond it is.
  function diamond(g, color) {
    g.fillStyle = color;
    g.beginPath();
    g.moveTo(0, 0);
    g.lineTo(32, 16);
    g.lineTo(0, 32);
    g.lineTo(-32, 16);
    g.closePath();
    g.fill();
  }

  // Cut out of the land like a piece of a diorama: the grass, and the earth under it.
  function paintEdge(g) {
    const depth = 7;
    g.fillStyle = '#5b3d22';
    g.beginPath();
    g.moveTo(-32, 16);
    g.lineTo(0, 32);
    g.lineTo(0, 32 + depth);
    g.lineTo(-32, 16 + depth);
    g.closePath();
    g.fill();
    g.fillStyle = '#7a5530';
    g.beginPath();
    g.moveTo(0, 32);
    g.lineTo(32, 16);
    g.lineTo(32, 16 + depth);
    g.lineTo(0, 32 + depth);
    g.closePath();
    g.fill();
    // The turf along the top of it.
    g.fillStyle = '#4f7a2c';
    g.beginPath();
    g.moveTo(-32, 16);
    g.lineTo(0, 32);
    g.lineTo(0, 33.6);
    g.lineTo(-32, 17.6);
    g.closePath();
    g.fill();
    g.fillStyle = '#6a9a38';
    g.beginPath();
    g.moveTo(0, 32);
    g.lineTo(32, 16);
    g.lineTo(32, 17.6);
    g.lineTo(0, 33.6);
    g.closePath();
    g.fill();
    // A few stones in the earth.
    g.fillStyle = 'rgba(0, 0, 0, .18)';
    for (const [x, y] of [[-24, 22.5], [-12, 29.5], [-4, 35], [9, 33], [20, 25.5], [27, 21.5]]) g.fillRect(x, y, 2.4, 1.4);
    g.fillStyle = 'rgba(255, 235, 190, .16)';
    for (const [x, y] of [[-20, 24.5], [-8, 31.5], [4, 36.5], [14, 30.5], [25, 23.5]]) g.fillRect(x, y, 1.8, 1);
  }

  function paintSky(g, w, tall) {
    const sky = g.createLinearGradient(0, 0, 0, tall);
    sky.addColorStop(0, '#79b7e0');
    sky.addColorStop(0.7, '#cfe8f1');
    sky.addColorStop(1, '#e9f1dc');
    g.fillStyle = sky;
    g.fillRect(0, 0, w, tall);
    g.fillStyle = 'rgba(255, 255, 255, .55)';
    for (const [fx, fy, size] of [[0.14, 0.16, 1], [0.8, 0.1, 1.3], [0.62, 0.3, 0.8], [0.3, 0.38, 0.7]]) {
      const x = w * fx;
      const y = tall * fy;
      const r = Math.min(w, tall) * 0.032 * size;
      g.beginPath();
      g.ellipse(x, y, r * 2.4, r * 0.9, 0, 0, 6.2832);
      g.ellipse(x - r, y - r * 0.5, r * 1.3, r * 0.9, 0, 0, 6.2832);
      g.ellipse(x + r * 1.1, y - r * 0.4, r * 1.5, r, 0, 0, 6.2832);
      g.fill();
    }
  }

  // Stars, for the night pass: they are light, and go over the dark picture.
  function paintStars(g, w, tall) {
    let seed = 7;
    const rand = () => {
      seed = (seed * 16807) % 2147483647;
      return seed / 2147483647;
    };
    for (let i = 0; i < 46; i++) {
      const x = rand() * w;
      const y = rand() * tall * 0.55;
      const size = rand() < 0.15 ? 2.4 : 1.4;
      g.fillStyle = `rgba(255, 250, 225, ${0.35 + rand() * 0.5})`;
      g.fillRect(x, y, size, size);
    }
  }

  // The parts that don't move are painted once into canvases the size of the stage, and again when the
  // design changes, because painting a tile is slow.
  function buildLayers() {
    const doc = stageDoc();
    const key = [keyOf(doc), view.w, view.h, view.q, kind, look.door, look.pal, look.variant].join('|');
    if (layers?.key === key) return;
    const make = () => {
      const c = document.createElement('canvas');
      c.width = canvas.width;
      c.height = canvas.height;
      return c;
    };
    const set = (g) => g.setTransform(view.s * view.q, 0, 0, view.s * view.q, view.ox * view.q, view.oy * view.q);
    const spot = { kind, outside: doc, door: doorSideOf(doc) || look.door, pal: look.pal, variant: look.variant };
    const safe = (g, extra) => {
      try {
        paintPlace(g, { ...spot, ...extra });
      } catch (error) {
        console.error(error);
      }
    };
    const back = make();
    let g = back.getContext('2d');
    g.setTransform(view.q, 0, 0, view.q, 0, 0);
    paintSky(g, view.w, view.h);
    set(g);
    paintEdge(g);
    diamond(g, '#74a141');
    safe(g, { day: true, layer: 'ground' });
    const stand = make();
    g = stand.getContext('2d');
    set(g);
    safe(g, { day: true, layer: 'standing' });
    const lights = make();
    g = lights.getContext('2d');
    g.setTransform(view.q, 0, 0, view.q, 0, 0);
    paintStars(g, view.w, view.h);
    set(g);
    safe(g, { day: false, layer: 'standing' });
    layers = { key, back, stand, lights };
  }

  // Where a pet on guard stands, on whichever side this door is.
  function doorOf() {
    const doc = stageDoc();
    const side = doorSideOf(doc) || look.door;
    return { side, at: doorPoint(kind, doc, side) };
  }

  // E18 to E21: the pets that live here, started again when the pets or what they walk round change.
  function resync() {
    const doc = stageDoc();
    const list = Array.isArray(doc.pets) ? doc.pets : [];
    const key = keyOf([doc.pets, doc.items, doc.body, partOf(doc, 'centerpiece').style, kind]);
    if (key === petKey) return;
    petKey = key;
    pets = list.filter((pet) => pet && typeof pet === 'object').map((pet, index) => makePet(`stage:${index}`, tile.x, tile.y, pet, doc, key, kind, doorOf()));
  }

  // E20: all a pet is told here is the time of day on this sky. Nobody's near and it's the owner's own page.
  const factsNow = () => ({ hour: night ? 23 : 12, disaster: false, person: false, bot: false, frog: false, owner: true });

  function tick(now) {
    raf = 0;
    if (!active) return;
    const dt = last ? Math.min(0.1, (now - last) / 1000) : 0;
    last = now;
    clock += dt;
    const calm = reduced.matches;
    const moving = pets.length > 0 && !calm && !document.hidden;
    if (moving) {
      const facts = factsNow();
      for (const pet of pets) {
        try {
          stepPet(pet, dt, facts);
        } catch (error) {
          console.error(error);
        }
      }
    }
    if (again || moving) {
      again = false;
      draw(calm);
    }
    if (moving) raf = requestAnimationFrame(tick);
    else last = 0;
  }

  function drawPets(g, behind, calm) {
    for (const pet of pets) {
      if (Boolean(pet.behind) !== behind) continue;
      // E36: the stage draws the yard at single size, so a pet goes where its own `u` and `v` say. On the
      // map those are spread over the tiles a big place takes.
      [pet.dx, pet.dy] = P(pet.u, pet.v);
      try {
        drawPet(g, pet, clock, calm);
      } catch (error) {
        console.error(error);
      }
    }
  }

  // The footprint of something, in cells, as a flat shape on the ground.
  function footprint(g, spot, fill, line) {
    const [u0, v0, u1, v1] = [spot.x * UNIT, spot.y * UNIT, (spot.x + spot.w) * UNIT, (spot.y + spot.h) * UNIT];
    g.beginPath();
    g.moveTo(...P(u0, v0));
    g.lineTo(...P(u1, v0));
    g.lineTo(...P(u1, v1));
    g.lineTo(...P(u0, v1));
    g.closePath();
    if (fill) {
      g.fillStyle = fill;
      g.fill();
    }
    if (line) {
      g.strokeStyle = line;
      g.lineWidth = 0.7;
      g.stroke();
    }
  }

  // The thing in hand, as a drawing of it that is moved about and not painted again each time.
  function ghostFor() {
    if (!hand || hand.type !== 'item') return null;
    const key = [hand.kind, hand.facing, hand.color, view.s, view.q, view.w, view.h].join('|');
    if (ghost?.key === key) return ghost;
    const c = document.createElement('canvas');
    c.width = canvas.width;
    c.height = canvas.height;
    const g = c.getContext('2d');
    g.setTransform(view.s * view.q, 0, 0, view.s * view.q, view.ox * view.q, view.oy * view.q);
    const item = { kind: hand.kind, x: REF, y: REF, facing: hand.facing };
    if (hand.color !== undefined) item.color = hand.color;
    try {
      paintPlace(g, {
        kind: 'square',
        outside: { parts: { centerpiece: { style: 'none' } }, items: [item] },
        door: 'L',
        pal: 0,
        variant: 0,
        day: true,
        layer: 'standing',
      });
    } catch (error) {
      console.error(error);
    }
    ghost = { key, c };
    return ghost;
  }
  function draw(calm) {
    buildLayers();
    const g = canvas.getContext('2d');
    g.setTransform(1, 0, 0, 1, 0, 0);
    g.globalAlpha = 1;
    g.globalCompositeOperation = 'source-over';
    g.clearRect(0, 0, canvas.width, canvas.height);
    g.drawImage(layers.back, 0, 0);
    g.setTransform(view.s * view.q, 0, 0, view.s * view.q, view.ox * view.q, view.oy * view.q);
    drawPets(g, true, calm);
    g.setTransform(1, 0, 0, 1, 0, 0);
    g.drawImage(layers.stand, 0, 0);
    g.setTransform(view.s * view.q, 0, 0, view.s * view.q, view.ox * view.q, view.oy * view.q);
    drawPets(g, false, calm);
    if (night) {
      g.setTransform(1, 0, 0, 1, 0, 0);
      g.fillStyle = 'rgba(16, 14, 50, .72)';
      g.fillRect(0, 0, canvas.width, canvas.height);
      g.globalCompositeOperation = 'lighter';
      g.drawImage(layers.lights, 0, 0);
      g.globalCompositeOperation = 'source-over';
    }
    g.setTransform(view.s * view.q, 0, 0, view.s * view.q, view.ox * view.q, view.oy * view.q);
    if (chosen !== null && shown.items?.[chosen]) {
      const mine = footprints(shown).find((each) => each.index === chosen);
      if (mine) footprint(g, mine, 'rgba(255, 255, 255, .28)', 'rgba(255, 255, 255, .95)');
    }
    if (hand) {
      const ok = fits(hand);
      const spot = { x: hand.x, y: hand.y, w: hand.w, h: hand.h };
      if (hand.type === 'building') {
        footprint(g, spot, ok ? 'rgba(80, 200, 90, .5)' : 'rgba(225, 70, 55, .5)', ok ? '#cfffd0' : '#ffd0c8');
      } else {
        const art = ghostFor();
        if (art) {
          const [du, dv] = [(hand.x - REF) * UNIT, (hand.y - REF) * UNIT];
          const [ax, ay] = P(du, dv);
          g.setTransform(1, 0, 0, 1, 0, 0);
          g.globalAlpha = 0.78;
          g.drawImage(art.c, Math.round(ax * view.s * view.q), Math.round(ay * view.s * view.q));
          g.globalAlpha = 1;
          g.setTransform(view.s * view.q, 0, 0, view.s * view.q, view.ox * view.q, view.oy * view.q);
        }
        footprint(g, spot, ok ? 'rgba(80, 200, 90, .42)' : 'rgba(225, 70, 55, .45)', ok ? '#cfffd0' : '#ffd0c8');
      }
    }
  }

  function setNight(on) {
    night = on;
    nightButton.textContent = on ? 'See it by day' : 'See it at night';
    tell(on ? 'Night. What shines is lit.' : 'Daytime.');
    kick();
  }

  // --- The pointer ----------------------------------------------------------------------------

  // Where the pointer is on the ground, in units. `lift` is how high up it is pointing, so the crown of
  // a tree reads as the tree.
  function ground(event, lift = 0) {
    const box = canvas.getBoundingClientRect();
    const ax = (event.clientX - box.left - view.ox) / view.s;
    const ay = (event.clientY - box.top - view.oy) / view.s + lift;
    return { u: ay + ax / 2, v: ay - ax / 2 };
  }

  function cellAt(event) {
    const { u, v } = ground(event);
    // Off the yard counts as its nearest cell, so a finger just past the edge still finds what it holds.
    return { x: clamp(Math.floor(u / UNIT), 0, GRID - 1), y: clamp(Math.floor(v / UNIT), 0, GRID - 1) };
  }

  // Front first, and a point on the ground before one up in the air.
  function itemUnder(event) {
    const list = footprints(shown).sort((a, b) => b.x + b.y - (a.x + a.y));
    for (const lift of [0, 5, 12, 22, 34]) {
      const { u, v } = ground(event, lift);
      const found = list.find((each) => u >= each.x * UNIT && u < (each.x + each.w) * UNIT && v >= each.y * UNIT && v < (each.y + each.h) * UNIT);
      if (found) return found;
    }
    return null;
  }

  function onBuilding(event) {
    const solid = solidOf(shown);
    if (!solid) return false;
    for (const lift of [0, 8, 18, 30]) {
      const { u, v } = ground(event, lift);
      if (u >= solid.x * UNIT && u < (solid.x + solid.w) * UNIT && v >= solid.y * UNIT && v < (solid.y + solid.h) * UNIT) return true;
    }
    return false;
  }

  canvas.addEventListener('pointerdown', (event) => {
    const at = cellAt(event);
    press = {
      touch: event.pointerType === 'touch',
      onHand: Boolean(hand && overlaps({ x: at.x - 1, y: at.y - 1, w: 3, h: 3 }, hand)),
    };
  });
  canvas.addEventListener('pointermove', (event) => {
    if (!hand) {
      stage.classList.toggle('over-item', Boolean(itemUnder(event)));
      return;
    }
    handTo(cellAt(event));
  });
  canvas.addEventListener('pointerleave', () => stage.classList.remove('over-item'));
  canvas.addEventListener('click', (event) => {
    if (hand) {
      const at = cellAt(event);
      // A finger can't hover, so its first tap shows the spot. Pressing what's in hand puts it down.
      if (press.touch && !press.onHand) {
        handTo(at);
        return;
      }
      if (!press.touch) handTo(at);
      putDown();
      return;
    }
    const found = itemUnder(event);
    if (found) {
      choose(found.index, event);
      return;
    }
    choose(null);
    if (onBuilding(event)) tell('That\'s the building. Its walls, roof, windows and the rest are in the catalog, under Size and the groups after it.');
  });

  // --- Holding a thing --------------------------------------------------------------------------

  function moveHand(x, y) {
    if (hand.type === 'building') {
      hand.x = clamp(x, 1, GRID - 1 - hand.w);
      hand.y = clamp(y, 1, GRID - 1 - hand.h);
    } else {
      hand.x = clamp(x, 0, GRID - hand.w);
      hand.y = clamp(y, 0, GRID - hand.h);
    }
    showHand();
  }

  // The middle of it goes where the pointer is, with the cell under the pointer as the one to go by.
  const handTo = (at) => moveHand(at.x - Math.floor((hand.w - 1) / 2), at.y - Math.floor((hand.h - 1) / 2));
  const nudge = (dx, dy) => moveHand(hand.x + dx, hand.y + dy);

  // Below BESIDE a panel covers the stage, so it goes before anyone's asked to touch the stage.
  function clearPanel() {
    if (beside.matches || !panels.current) return;
    panels.close(false);
  }

  const idleHint = () => (coarse.matches
    ? 'Tap something in the yard to change it, or open the catalog.'
    : 'Pick from the catalog, or click something in the yard to change it.');

  function hold(next) {
    if (!catalog) return;
    hand = next;
    choose(null);
    clearPanel();
    showHand();
    putButton.focus({ preventScroll: true });
    tell(coarse.matches
      ? `${next.label}: tap the yard to choose a spot, then Put it here.`
      : `${next.label}: move over the yard and click to put it down.`);
  }

  function holdNew(word) {
    const [w, tall] = sizeOf(word);
    const spot = firstSpot(w, tall);
    hold({ type: 'item', kind: word, label: labelOfKind(word), x: spot.x, y: spot.y, w, h: tall, facing: 0 });
  }

  function holdItem(index) {
    const item = shown.items?.[index];
    if (!item) return;
    const mine = footprints(shown).find((each) => each.index === index);
    hold({
      type: 'item', kind: item.kind, label: labelOfKind(item.kind), x: item.x, y: item.y, w: mine.w, h: mine.h,
      facing: (item.facing || 0) & 3, index, color: item.color,
    });
    repaint();
  }

  function holdBuilding() {
    const body = bodyOf(kind, shown);
    if (!body) return;
    hold({ type: 'building', kind: 'building', label: 'The building', x: body.x, y: body.y, w: body.w, h: body.d, floors: body.floors, facing: 0 });
  }

  function turnHand() {
    if (!hand || hand.type !== 'item') return;
    hand.facing = (hand.facing + 1) % 4;
    [hand.w, hand.h] = [hand.h, hand.w];
    moveHand(hand.x, hand.y);
  }

  // The hand's buttons go away when it lets go, and focus would go with them.
  function focusBack(word) {
    const to = [tiles.get(word), catalogButton, doneButton].find((button) => button?.offsetParent);
    to?.focus({ preventScroll: true });
  }

  function letGo() {
    const lifted = hand?.index !== undefined && hand?.index !== null;
    const word = hand?.kind;
    hand = null;
    if (lifted) repaint();
    showHand();
    focusBack(word);
    tell(idleHint());
  }

  function showHand() {
    stage.classList.toggle('holding', Boolean(hand));
    kick();
    renderDock();
  }

  // Only the server's word counts, so what's in hand stays in hand until it has said yes.
  async function putDown() {
    if (!hand || busy) return;
    const was = hand;
    busy = true;
    try {
      const next = edit((doc) => {
        if (was.type === 'building') {
          doc.body = { x: was.x, y: was.y, w: was.w, d: was.h, floors: was.floors };
        } else {
          doc.items ||= [];
          const item = { kind: was.kind, x: was.x, y: was.y };
          if (was.facing) item.facing = was.facing;
          if (was.color !== undefined) item.color = was.color;
          if (was.index !== undefined && was.index !== null && was.index < doc.items.length) doc.items[was.index] = item;
          else doc.items.push(item);
        }
      });
      hand = null;
      const ok = await save(next);
      if (ok) {
        showHand();
        tell(`${was.label} ${was.index !== undefined && was.index !== null || was.type === 'building' ? 'moved' : 'placed'}.`);
        focusBack(was.kind);
      } else {
        hand = was;
        repaint();
        showHand();
        tell(buildError.textContent);
      }
    } finally {
      busy = false;
    }
  }

  // --- What can be done with something that's there --------------------------------------------

  function choose(index, event) {
    chosen = index;
    kick();
    if (index === null || index === undefined || !event) return;
    const item = shown.items?.[index];
    if (!item) return;
    const items = [
      { label: 'Move', run: () => holdItem(index) },
      { label: 'Turn', run: () => turnItem(index) },
      ...(yardKinds.get(item.kind)?.tint ? [{ label: 'Colour', run: () => colourItem(index) }] : []),
      { label: 'Remove', danger: true, run: () => removeItem(index) },
    ];
    menu.open({ title: labelOfKind(item.kind), sub: 'In this yard', items }, event.clientX, event.clientY, press.touch ? 'press' : 'click');
  }

  async function turnItem(index) {
    const item = shown.items?.[index];
    if (!item) return;
    const ok = await commit(edit((doc) => {
      doc.items[index].facing = ((item.facing || 0) + 1) % 4;
    }), `${labelOfKind(item.kind)} turned.`);
    if (ok) chosen = null;
  }

  async function removeItem(index) {
    const item = shown.items?.[index];
    if (!item) return;
    chosen = null;
    await commit(edit((doc) => {
      doc.items.splice(index, 1);
    }), `${labelOfKind(item.kind)} removed.`);
  }

  function colourItem(index) {
    const item = shown.items?.[index];
    if (!item) return;
    const word = labelOfKind(item.kind);
    colourDialog(`Colour of the ${word.toLowerCase()}`, () => shown.items?.[index]?.color, (value, live) => {
      const next = edit((doc) => {
        if (!doc.items[index]) return;
        if (value === undefined) delete doc.items[index].color;
        else doc.items[index].color = value;
      });
      return live ? later(next) : commit(next);
    });
  }

  // --- Colours ----------------------------------------------------------------------------------

  // A row of swatches, a native picker for any other, and a way back to how it was. `set(value, live)`
  // is told a number, or undefined for "back to usual", and `live` while a picker is still being dragged.
  function colorRow({ label, value, set, name, plain = 'Back to usual' }) {
    const swatches = (catalog?.swatches || []).filter((each) => isWhole(each) && each >= 0 && each <= MAX_COLOR);
    const here = hexOf(value, null);
    return h('div', { class: 'outside-colors', role: 'group', 'aria-label': `${label} colour` },
      h('span', { class: 'wardrobe-swatches' }, swatches.map((each) => {
        const button = h('button', {
          type: 'button',
          class: 'wardrobe-swatch',
          name: `${name}-${each}`,
          title: colorName(each),
          'aria-label': `${label}: ${colorName(each)}`,
          'aria-pressed': String(value === each),
          onclick: () => set(each, false),
        });
        button.style.background = hexText(each);
        return button;
      })),
      h('span', { class: 'outside-color-more' },
        h('label', { class: 'wd-btn outside-any' },
          h('span', {}, 'Any colour'),
          h('input', {
            type: 'color',
            name: `${name}-any`,
            value: here || '#ffffff',
            'aria-label': `${label}: any colour`,
            oninput: (event) => {
              picking = true;
              const number = numberOfHex(event.target.value);
              if (number !== null) set(number, true);
            },
            onchange: (event) => {
              picking = false;
              const number = numberOfHex(event.target.value);
              if (number !== null) set(number, false);
            },
            onblur: () => {
              picking = false;
            },
          })),
        h('button', {
          type: 'button',
          class: 'wd-btn outside-usual',
          name: `${name}-usual`,
          'aria-label': `${label}: ${plain.toLowerCase()}`,
          disabled: value === undefined,
          onclick: () => set(undefined, false),
        }, plain)));
  }

  function colourDialog(title, get, set) {
    const holder = h('div', { class: 'outside-dialog' });
    const fill = () => holder.replaceChildren(colorRow({
      label: title,
      value: get(),
      name: 'dialog',
      plain: 'No colour of its own',
      set: async (value, live) => {
        await set(value, live);
        if (!live) fill();
      },
    }));
    fill();
    const dialog = modal(title, holder,
      h('div', { class: 'modal-actions' }, h('button', { type: 'button', class: 'primary', onclick: () => dialog.close() }, 'Done')));
  }

  // --- Pictures -----------------------------------------------------------------------------

  const pictures = new Map();
  const stale = new Map();
  const queue = [];
  const jobs = new Map();
  let observer = null;
  let pumping = 0;

  function remember(key, picture) {
    pictures.set(key, picture);
    if (pictures.size > 700) pictures.delete(pictures.keys().next().value);
    return picture;
  }

  function pump() {
    if (pumping || !queue.length) return;
    pumping = requestAnimationFrame(() => {
      pumping = 0;
      const started = performance.now();
      while (queue.length && performance.now() - started < 10) queue.shift()();
      pump();
    });
  }

  function watch(target, run) {
    if (typeof IntersectionObserver === 'undefined') {
      queue.push(run);
      pump();
      return;
    }
    observer ||= new IntersectionObserver((entries) => {
      for (const entry of entries) {
        const run_ = entry.isIntersecting && jobs.get(entry.target);
        if (!run_) continue;
        jobs.delete(entry.target);
        observer.unobserve(entry.target);
        queue.push(run_);
      }
      pump();
    }, { rootMargin: '160px' });
    jobs.set(target, run);
    observer.observe(target);
  }

  // A small canvas that fills in when it is on screen. What was there for the same tile is shown meanwhile,
  // so a change doesn't blank every picture.
  function lazyPicture(id, key, wide, tall, make, fluid = false) {
    const target = h('canvas', { width: Math.round(wide * dpr()), height: Math.round(tall * dpr()) });
    // A picture of a whole place takes the width of its tile, so CSS sizes it.
    if (!fluid) {
      target.style.width = `${wide}px`;
      target.style.height = `${tall}px`;
    }
    const g = target.getContext('2d');
    const show = (source) => {
      g.clearRect(0, 0, target.width, target.height);
      g.drawImage(source, 0, 0, target.width, target.height);
      stale.set(id, source);
    };
    const done = pictures.get(key);
    if (done) {
      show(done);
      return target;
    }
    if (stale.has(id)) show(stale.get(id));
    watch(target, () => {
      if (!target.isConnected) return;
      try {
        show(pictures.get(key) || remember(key, make()));
      } catch (error) {
        console.error(error);
      }
    });
    return target;
  }

  // This tile with `doc` on it, as the crop `crop` shows of it.
  function snapshot(doc, crop, wide, tall, ground = true) {
    const picture = document.createElement('canvas');
    picture.width = Math.round(wide * dpr());
    picture.height = Math.round(tall * dpr());
    const g = picture.getContext('2d');
    const [x0, y0, x1, y1] = crop;
    const s = Math.min(wide / (x1 - x0), tall / (y1 - y0));
    g.setTransform(s * dpr(), 0, 0, s * dpr(), (wide / 2 - ((x0 + x1) / 2) * s) * dpr(), (tall / 2 - ((y0 + y1) / 2) * s) * dpr());
    if (ground) diamond(g, '#74a141');
    try {
      paintPlace(g, { kind, outside: doc, door: doorSideOf(doc) || look.door, pal: look.pal, variant: look.variant, day: true });
    } catch (error) {
      console.error(error);
    }
    return picture;
  }

  // What a pair of pictures of this place differ by is the part, so the rest of it stays: the parts and
  // the body, and not the yard, which would only slow them down.
  const baseDoc = () => {
    const { parts, body } = shown;
    return clone({ parts, body });
  };

  const placeKey = () => [kind, look.door, look.pal, look.variant].join(',');

  function partPicture(part, change, id, tag) {
    const doc = baseDoc();
    change(doc);
    const crop = CROPS[part.part] || (part.group === 'size' ? SIZES_IN_VIEW : FRONT);
    const key = `${placeKey()}|${part.part}|${tag}|${keyOf(doc)}`;
    return lazyPicture(id, key, PIC_W, PIC_H, () => snapshot(doc, crop, PIC_W, PIC_H));
  }

  // A yard kind on its own, found by painting it and cutting to what was painted.
  function itemPicture(word) {
    return lazyPicture(`item:${word}`, `item|${word}`, PIC_W, PIC_H, () => {
      const work = document.createElement('canvas');
      work.width = 300;
      work.height = 460;
      const g = work.getContext('2d');
      const S = 5;
      g.setTransform(S, 0, 0, S, 150, 330);
      const [w, tall] = sizeOf(word);
      try {
        paintPlace(g, {
          kind: 'square',
          outside: { parts: { centerpiece: { style: 'none' } }, items: [{ kind: word, x: Math.floor(8 - w / 2), y: Math.floor(8 - tall / 2) }] },
          door: 'L',
          pal: 0,
          variant: 0,
          day: true,
          layer: 'standing',
        });
      } catch (error) {
        console.error(error);
      }
      const data = g.getImageData(0, 0, work.width, work.height).data;
      let [x0, y0, x1, y1] = [work.width, work.height, -1, -1];
      for (let y = 0; y < work.height; y++) {
        for (let x = 0; x < work.width; x++) {
          if (data[(y * work.width + x) * 4 + 3] < 10) continue;
          if (x < x0) x0 = x;
          if (x > x1) x1 = x;
          if (y < y0) y0 = y;
          if (y > y1) y1 = y;
        }
      }
      const out = document.createElement('canvas');
      out.width = Math.round(PIC_W * dpr());
      out.height = Math.round(PIC_H * dpr());
      if (x1 < 0) return out;
      const o = out.getContext('2d');
      const room = Math.min((out.width - 6) / (x1 - x0 + 1), (out.height - 6) / (y1 - y0 + 1));
      const fit_ = Math.min(room, 2.2 * dpr());
      const dw = (x1 - x0 + 1) * fit_;
      const dh = (y1 - y0 + 1) * fit_;
      o.drawImage(work, x0, y0, x1 - x0 + 1, y1 - y0 + 1, (out.width - dw) / 2, out.height - dh - 3, dw, dh);
      return out;
    });
  }

  function templatePicture(template) {
    const doc = clone(template.outside);
    return lazyPicture(`template:${template.template}`, `${placeKey()}|template|${template.template}`, 168, 120, () => snapshot(doc, WHOLE, 168, 120), true);
  }

  function speciesPicture(species, coat, collar, size = 56) {
    const canvas_ = petPicture(species, coat, collar, size * 2);
    canvas_.style.width = `${size}px`;
    canvas_.style.height = `${size}px`;
    return canvas_;
  }

  // --- The catalog ----------------------------------------------------------------------------

  let tiles = new Map();
  let renderedKey = null;
  let yardKey = null;

  const defaultsNow = () => {
    try {
      return defaultStyles(kind, look.variant);
    } catch {
      return {};
    }
  };
  const listOf = (value) => (Array.isArray(value) ? value : typeof value === 'string' ? [value] : []);
  const styleIn = (value) => (value && typeof value === 'object' && !Array.isArray(value) ? value.style : value);

  // Everything in the catalog is one of these, whatever pressing it does (I45). `whole` is a picture of
  // a whole place, which takes the width of the tile.
  function catalogTile({ name, picture, whole, label, lines = [], why, pressed = false, onclick }) {
    return h('button', {
      type: 'button',
      name,
      class: `wd-btn build-tile${why ? ' locked' : ''}`,
      'aria-pressed': String(pressed),
      disabled: Boolean(why),
      onclick,
    },
    whole
      ? h('span', { class: 'template-pic outside-whole', 'aria-hidden': 'true' }, whole)
      : h('span', { class: 'build-pic outside-pic', 'aria-hidden': 'true' }, picture),
    h('span', { class: 'build-name' }, label),
    lines.map((line) => h('small', {}, line)),
    why ? h('small', { class: 'build-why' }, why) : null);
  }

  const catalogGroup = (title, members) => h('section', { class: 'build-group', 'aria-label': title },
    h('h3', {}, title),
    members);
  const tilesOf = (members, whole = false) => h('div', { class: `build-tiles${whole ? ' build-templates' : ''}` }, members);

  // E10: a tile for each style a part has. Pressing one in use takes the choice away, so the place goes
  // back to how it would otherwise look.
  function styleTiles(part) {
    const chosenHere = partOf(shown, part.part);
    const defaults = defaultsNow()[part.part];
    return part.styles.map(({ style, label }) => {
      let pressed;
      if (part.many) pressed = (Array.isArray(chosenHere.on) ? chosenHere.on : listOf(defaults).map(styleIn)).includes(style);
      else pressed = chosenHere.style !== undefined ? chosenHere.style === style : styleIn(defaults) === style;
      const explicit = !part.many && chosenHere.style === style;
      return catalogTile({
        name: `style-${part.part}-${style}`,
        picture: partPicture(part, (doc) => {
          if (part.many) setPart(doc, part.part, 'on', [style]);
          else setPart(doc, part.part, 'style', style);
        }, `style:${part.part}:${style}`, style),
        label,
        pressed,
        onclick: () => commit(edit((doc) => {
          if (part.many) {
            const now = Array.isArray(partOf(doc, part.part).on) ? partOf(doc, part.part).on : listOf(defaults).map(styleIn);
            setPart(doc, part.part, 'on', pressed ? now.filter((each) => each !== style) : [...now, style]);
          } else {
            setPart(doc, part.part, 'style', explicit ? undefined : style);
          }
        }), `${part.label}: ${pressed && !part.many ? 'back to usual' : pressed ? `${label} off` : label}.`),
      });
    });
  }

  function numberRow(part) {
    const amount = part.number;
    const current = partOf(shown, part.part).number;
    if (part.part === 'fence') {
      const sides = isWhole(current) ? current : 15;
      return h('div', { class: 'outside-numbers', role: 'group', 'aria-label': 'Which sides have a fence' },
        h('span', { class: 'outside-numbers-label' }, amount.label),
        SIDES.map(([bit, word]) => h('button', {
          type: 'button',
          class: 'wd-btn outside-number',
          name: `side-${bit}`,
          'aria-pressed': String(Boolean(sides & bit)),
          onclick: () => {
            const now = sides ^ bit;
            commit(edit((doc) => setPart(doc, 'fence', 'number', now === 15 ? undefined : now)), `Fence: ${word.toLowerCase()} ${sides & bit ? 'taken away' : 'put up'}.`);
          },
        }, word)));
    }
    const choices = [];
    for (let n = amount.min; n <= amount.max; n++) choices.push(n);
    return h('div', { class: 'outside-numbers', role: 'group', 'aria-label': `${part.label}: ${amount.label.toLowerCase()}` },
      h('span', { class: 'outside-numbers-label' }, amount.label),
      choices.map((n) => h('button', {
        type: 'button',
        class: 'wd-btn outside-number',
        name: `number-${part.part}-${n}`,
        'aria-label': `${part.label}: ${amount.label.toLowerCase()} ${n}`,
        'aria-pressed': String(current === n),
        onclick: () => commit(edit((doc) => setPart(doc, part.part, 'number', current === n ? undefined : n)), `${part.label}: ${amount.label.toLowerCase()} ${current === n ? 'back to usual' : n}.`),
      }, String(n))));
  }

  function partBlock(part) {
    const value = partOf(shown, part.part);
    return h('div', { class: 'outside-part' },
      h('h4', {}, part.label),
      tilesOf(styleTiles(part)),
      part.number ? numberRow(part) : null,
      part.colors
        ? colorRow({
          label: part.label,
          value: isWhole(value.color) ? value.color : undefined,
          name: `color-${part.part}`,
          set: (color, live) => {
            const next = edit((doc) => setPart(doc, part.part, 'color', color));
            return live ? later(next) : commit(next, `${part.label} colour ${color === undefined ? 'back to usual' : 'changed'}.`);
          },
        })
        : null);
  }

  // E13: sizes are tiles that set `body`.
  function sizeTiles() {
    const body = bodyOf(kind, shown);
    const still = (change) => {
      const doc = baseDoc();
      change(doc);
      return doc;
    };
    // E13, E16: where it fits round what's in the yard, nearest where it stands now, or null.
    const resized = (doc, w, d) => {
      const now = bodyOf(kind, doc);
      const cx = now.x + now.w / 2 - w / 2;
      const cy = now.y + now.d / 2 - d / 2;
      let best = null;
      for (let y = 1; y + d <= GRID - 1; y++) {
        for (let x = 1; x + w <= GRID - 1; x++) {
          if (!bodyFits(doc, { x, y, w, h: d })) continue;
          const far = (x - cx) ** 2 + (y - cy) ** 2;
          if (!best || far < best.far) best = { x, y, far };
        }
      }
      if (!best) return null;
      return { x: best.x, y: best.y, w, d, floors: now.floors };
    };
    const usualBody = bodyOf(kind, null);
    const members = SIZES.map(([word, label, w, d]) => {
      const dims = w === null ? bodyOf(kind, null) : { w, d };
      const make = (doc) => {
        if (w === null) {
          const usual = bodyOf(kind, null);
          if ((bodyOf(kind, doc).floors || 1) > 1) doc.body = { ...usual, floors: bodyOf(kind, doc).floors };
          else delete doc.body;
        } else {
          doc.body = resized(doc, w, d) || doc.body;
        }
      };
      return catalogTile({
        name: `size-${word}`,
        picture: lazyPicture(`size:${word}`, `${placeKey()}|size|${word}|${keyOf(baseDoc())}`, PIC_W, PIC_H, () => snapshot(still(make), SIZES_IN_VIEW, PIC_W, PIC_H)),
        label,
        lines: [`${dims.w} by ${dims.d} cells`],
        // A size that is what the place has anyway is only the Standard one (a lounge is 12 wide already).
        pressed: body.w === dims.w && body.d === dims.d && (w === null || !(usualBody.w === w && usualBody.d === d)),
        onclick: () => {
          if (w !== null && !resized(baseDoc(), w, d)) {
            tell(`There isn't room for a ${label.toLowerCase()} building here: move or take out something in the yard first.`);
            return;
          }
          commit(edit(make), `${label}.`);
        },
      });
    });
    for (const floors of [1, 2, 3]) {
      const make = (doc) => {
        const now = bodyOf(kind, doc);
        const usual = bodyOf(kind, null);
        const next = { ...now, floors };
        if (floors === 1 && now.x === usual.x && now.y === usual.y && now.w === usual.w && now.d === usual.d) delete doc.body;
        else doc.body = next;
      };
      members.push(catalogTile({
        name: `floors-${floors}`,
        picture: lazyPicture(`floors:${floors}`, `${placeKey()}|floors|${floors}|${keyOf(baseDoc())}`, PIC_W, PIC_H, () => snapshot(still(make), SIZES_IN_VIEW, PIC_W, PIC_H)),
        label: floors === 1 ? '1 floor' : `${floors} floors`,
        pressed: body.floors === floors,
        onclick: () => commit(edit(make), floors === 1 ? 'One floor.' : `${floors} floors.`),
      }));
    }
    members.push(catalogTile({
      name: 'move-building',
      picture: lazyPicture('move', `${placeKey()}|move|${keyOf(baseDoc())}`, PIC_W, PIC_H, () => snapshot(baseDoc(), SIZES_IN_VIEW, PIC_W, PIC_H)),
      label: 'Move',
      lines: ['Hold the building'],
      pressed: hand?.type === 'building',
      onclick: () => holdBuilding(),
    }));
    return tilesOf(members);
  }

  // E23: the template's parts replace the place's, its body replaces the body if it has one, and its
  // items and pets are added where they fit. What was in the yard stays.
  function applyTemplate(template) {
    let skipped = 0;
    const next = edit((doc) => {
      const set = clone(template.outside);
      doc.parts = set.parts || {};
      if (set.body) doc.body = set.body;
      doc.items ||= [];
      for (const item of set.items || []) {
        if (doc.items.some((each) => each.kind === item.kind && each.x === item.x && each.y === item.y)) continue;
        const spot = doc.items.length < limits().items && nearestSpot(doc, item.kind, item);
        if (!spot) {
          skipped += 1;
          continue;
        }
        doc.items.push({ ...item, x: spot.x, y: spot.y });
      }
      doc.pets ||= [];
      for (const pet of set.pets || []) {
        if (doc.pets.some((each) => each.species === pet.species && each.name === pet.name)) continue;
        if (doc.pets.length >= limits().pets) skipped += 1;
        else doc.pets.push(pet);
      }
    });
    return commit(next, `${template.label} is set up.${skipped ? ` ${skipped} didn't fit around what's here.` : ''}`);
  }

  function templateTile(template) {
    return catalogTile({
      name: `template-${template.template}`,
      whole: templatePicture(template),
      label: template.label,
      pressed: same(template.outside.parts || {}, shown.parts || {}),
      onclick: () => applyTemplate(template),
    });
  }

  // E34: a pack is up while any of its kinds is in the yard, and nothing else records it.
  const kindsOf = (pack) => new Set((pack.outside.items || []).map((item) => item.kind));
  const isUp = (pack, doc = shown) => (doc.items || []).some((item) => kindsOf(pack).has(item.kind));

  // E34: puts `pack` up on `doc`. Each part this kind of place has is set, and each item goes in where
  // it fits around what's there. Says how many went up and how many didn't.
  function putUp(doc, pack) {
    const have = new Set(catalog.parts.filter((part) => part.kinds.includes(kind)).map((part) => part.part));
    doc.parts ||= {};
    for (const [word, chosen] of Object.entries(pack.outside.parts || {})) if (have.has(word)) doc.parts[word] = clone(chosen);
    doc.items ||= [];
    let placed = 0;
    let skipped = 0;
    for (const item of pack.outside.items || []) {
      const spot = doc.items.length < limits().items && nearestSpot(doc, item.kind, item);
      if (!spot) {
        skipped += 1;
        continue;
      }
      doc.items.push({ ...item, x: spot.x, y: spot.y });
      placed += 1;
    }
    return { placed, skipped };
  }

  // E34: takes out every item of the pack's kinds, whoever put it there, and a part that still has the
  // pack's style goes, so the place looks as it otherwise would. Pets stay.
  function takeDown(doc, pack) {
    const gone = kindsOf(pack);
    const before = (doc.items || []).length;
    doc.items = (doc.items || []).filter((item) => !gone.has(item.kind));
    for (const [word, chosen] of Object.entries(pack.outside.parts || {})) {
      if (doc.parts && Object.hasOwn(doc.parts, word) && styleIn(doc.parts[word]) === chosen.style) delete doc.parts[word];
    }
    return before - doc.items.length;
  }

  function packPicture(pack) {
    const doc = clone(shown);
    delete doc.pets;
    putUp(doc, pack);
    return lazyPicture(`pack:${pack.pack}`, `${placeKey()}|pack|${pack.pack}|${keyOf(doc)}`, 168, 120, () => snapshot(doc, WHOLE, 168, 120), true);
  }

  function packTile(pack) {
    const up = isUp(pack);
    return catalogTile({
      name: `pack-${pack.pack}`,
      whole: packPicture(pack),
      label: `${pack.label} pack`,
      lines: [up ? 'Up now. Press to take it down' : 'Press to put it up'],
      pressed: up,
      onclick: () => {
        if (up) {
          let removed = 0;
          const next = edit((doc) => {
            removed = takeDown(doc, pack);
          });
          return commit(next, `Took down ${removed} from the ${pack.label} pack.`);
        }
        let result = { placed: 0, skipped: 0 };
        const next = edit((doc) => {
          result = putUp(doc, pack);
        });
        const left = result.skipped ? `, and ${result.skipped} didn't fit` : '';
        return commit(next, `Put up the ${pack.label} pack: ${result.placed} went up${left}.`);
      },
    });
  }

  function yardTile(entry) {
    const full = (shown.items?.length || 0) >= limits().items;
    const button = catalogTile({
      name: `yard-${entry.kind}`,
      picture: itemPicture(entry.kind),
      label: entry.label,
      lines: entry.tint ? ['Takes a colour'] : [],
      why: full ? 'the yard is full' : null,
      onclick: () => holdNew(entry.kind),
    });
    tiles.set(entry.kind, button);
    return button;
  }

  function petName(except) {
    const names = catalog?.pets?.names || [];
    const used = new Set((shown.pets || []).map((pet) => pet.name));
    const free = names.filter((each) => each !== except && !used.has(each));
    const from = free.length ? free : names.filter((each) => each !== except);
    return from.length ? from[Math.floor(Math.random() * from.length)] : except || 'pickle';
  }

  function speciesTile({ species, label }) {
    const full = (shown.pets?.length || 0) >= limits().pets;
    return catalogTile({
      name: `pet-${species}`,
      picture: speciesPicture(species, undefined, undefined, 56),
      label,
      why: full ? `a yard holds ${limits().pets}` : null,
      onclick: () => {
        const name = petName();
        commit(edit((doc) => {
          doc.pets ||= [];
          doc.pets.push({ species, name });
        }), `${wordName(name)} the ${label.toLowerCase()} lives here now.`);
      },
    });
  }

  // I46: the groups, in the catalog's order. A group with nothing for this kind of place isn't shown.
  function renderCatalog() {
    if (!catalog) return;
    const focused = catalogEl.contains(document.activeElement) ? document.activeElement.name : null;
    tiles = new Map();
    const parts = catalog.parts.filter((part) => part.kinds.includes(kind));
    const sections = [];
    const templates = catalog.templates.filter((each) => each.kinds.includes(kind));
    if (templates.length) sections.push(catalogGroup('Templates', tilesOf(templates.map(templateTile), true)));
    const packs = (catalog.packs || []).filter((pack) => pack && pack.outside);
    if (packs.length) sections.push(catalogGroup('Packs', tilesOf(packs.map(packTile), true)));
    for (const { group, label } of catalog.groups) {
      const mine = parts.filter((part) => part.group === group);
      const yard = catalog.yard.filter((entry) => entry.group === group);
      const members = [];
      if (group === 'size' && !hasNoBuilding(kind)) members.push(h('div', { class: 'outside-part' }, h('h4', {}, 'How big'), sizeTiles()));
      members.push(...mine.map(partBlock));
      if (yard.length) {
        members.push(h('div', { class: 'outside-part' },
          mine.length ? h('h4', {}, 'For the yard') : null,
          tilesOf(yard.map(yardTile))));
      }
      if (group === 'pets') members.push(tilesOf(catalog.pets.species.map(speciesTile)));
      if (members.length) sections.push(catalogGroup(label, members));
    }
    catalogEl.replaceChildren(...sections);
    if (focused) catalogEl.querySelector(`[name="${focused}"]`)?.focus({ preventScroll: true });
  }

  // --- In this yard -----------------------------------------------------------------------------

  const whenLabel = (word) => catalog?.pets.when.find((each) => each.when === word)?.label || wordName(word);
  const doLabel = (word) => catalog?.pets.do.find((each) => each.do === word)?.label || wordName(word);
  const speciesLabel = (word) => catalog?.pets.species.find((each) => each.species === word)?.label || wordName(word);
  const linkButton = (label, name, run, props = {}) => h('button', { type: 'button', class: 'link', name, onclick: run, ...props }, label);

  function itemRow(item, index) {
    const entry = yardKinds.get(item.kind);
    return h('li', { class: 'row' },
      h('span', { class: 'row-main' }, h('span', {}, labelOfKind(item.kind))),
      h('span', { class: 'row-actions' },
        linkButton('Move', `item-move-${index}`, () => holdItem(index), { 'aria-label': `Move the ${labelOfKind(item.kind).toLowerCase()}` }),
        linkButton('Turn', `item-turn-${index}`, () => turnItem(index), { 'aria-label': `Turn the ${labelOfKind(item.kind).toLowerCase()}` }),
        entry?.tint ? linkButton('Colour', `item-colour-${index}`, () => colourItem(index), { 'aria-label': `Colour the ${labelOfKind(item.kind).toLowerCase()}` }) : null,
        linkButton('Remove', `item-remove-${index}`, () => removeItem(index), { class: 'link danger', 'aria-label': `Remove the ${labelOfKind(item.kind).toLowerCase()}` })));
  }

  const changePet = (index, change, message) => commit(edit((doc) => change(doc.pets[index], doc)), message);

  function removePet(index) {
    const pet = shown.pets[index];
    const go = () => changePet(index, (_, doc) => doc.pets.splice(index, 1), `${wordName(pet.name)} is gone.`);
    if (!pet.rules?.length) {
      go();
      return;
    }
    const dialog = modal(`Remove ${wordName(pet.name)}?`,
      h('p', {}, 'Its automations go with it.'),
      h('div', { class: 'modal-actions' },
        h('button', { type: 'button', onclick: () => dialog.close() }, 'Keep it'),
        h('button', {
          type: 'button',
          class: 'primary',
          onclick: () => {
            dialog.close();
            go();
          },
        }, 'Remove pet')));
  }

  // E19: the words come from the catalog's lists, one half at a time.
  function addRule(index, button) {
    const box = button.getBoundingClientRect();
    const at = [box.left, box.bottom];
    const name = wordName(shown.pets[index].name);
    menu.open({
      title: 'Add an automation',
      sub: `When should ${name}…`,
      items: catalog.pets.when.map(({ when, label }) => ({
        label,
        run: () => menu.open({
          title: label,
          sub: `${name} should…`,
          items: catalog.pets.do.map(({ do: action, label: doing }) => ({
            label: doing,
            run: () => changePet(index, (pet) => {
              pet.rules = [...(pet.rules || []), { when, do: action }];
            }, `${name}: ${label} → ${doing}.`),
          })),
        }, ...at, 'click'),
      })),
    }, ...at, 'click');
  }

  function ruleRow(pet, index, rule, k) {
    const sentence = `${whenLabel(rule.when)} → ${doLabel(rule.do)}`;
    const total = pet.rules.length;
    const move = (to) => changePet(index, (each) => {
      const rules = [...each.rules];
      [rules[k], rules[to]] = [rules[to], rules[k]];
      each.rules = rules;
    }, `${sentence} is ${to < k ? 'earlier' : 'later'}.`);
    return h('li', { class: 'row outside-rule' },
      h('span', { class: 'row-main' }, h('span', {}, sentence)),
      h('span', { class: 'row-actions' },
        linkButton('↑', `rule-up-${index}-${k}`, () => move(k - 1), { 'aria-label': `Move "${sentence}" earlier`, disabled: k === 0 }),
        linkButton('↓', `rule-down-${index}-${k}`, () => move(k + 1), { 'aria-label': `Move "${sentence}" later`, disabled: k === total - 1 }),
        linkButton('Remove', `rule-remove-${index}-${k}`, () => changePet(index, (each) => {
          each.rules = each.rules.filter((_, at) => at !== k);
        }, 'Automation removed.'), { class: 'link danger', 'aria-label': `Remove "${sentence}"` })));
  }

  function petCard(pet, index) {
    const name = wordName(pet.name);
    const rules = Array.isArray(pet.rules) ? pet.rules : [];
    const add = h('button', {
      type: 'button',
      class: 'wd-btn wide outside-add',
      name: `rule-add-${index}`,
      disabled: rules.length >= limits().rules,
      onclick: (event) => addRule(index, event.currentTarget),
    }, rules.length >= limits().rules ? `That's ${limits().rules} automations` : 'Add an automation');
    return h('section', { class: 'outside-pet', 'aria-label': `${name}, ${speciesLabel(pet.species).toLowerCase()}` },
      h('div', { class: 'outside-pet-head' },
        h('span', { class: 'build-pic outside-pet-pic', 'aria-hidden': 'true' }, speciesPicture(pet.species, pet.coat, pet.collar, 48)),
        h('span', { class: 'row-main' }, h('strong', {}, name), h('small', { class: 'muted' }, speciesLabel(pet.species)))),
      h('div', { class: 'row-actions' },
        linkButton('Another name', `pet-name-${index}`, () => {
          const next = petName(pet.name);
          changePet(index, (each) => {
            each.name = next;
          }, `Now called ${wordName(next)}.`);
        }),
        linkButton('Remove pet', `pet-remove-${index}`, () => removePet(index), { class: 'link danger', 'aria-label': `Remove ${name}` })),
      ...['coat', 'collar'].map((field) => h('div', { class: 'outside-pet-color' },
        h('span', { class: 'outside-numbers-label' }, wordName(field)),
        colorRow({
          label: `${name}'s ${field}`,
          value: isWhole(pet[field]) ? pet[field] : undefined,
          name: `pet-${field}-${index}`,
          set: (color, live) => {
            const next = edit((doc) => {
              if (color === undefined) delete doc.pets[index][field];
              else doc.pets[index][field] = color;
            });
            return live ? later(next) : commit(next);
          },
        }))),
      h('h4', {}, 'Automations'),
      h('p', { class: 'muted fine' }, 'The first one that fits wins. A pet with none that fit wanders round the yard.'),
      rules.length ? h('ul', { class: 'rows outside-rules' }, rules.map((rule, k) => ruleRow(pet, index, rule, k))) : h('p', { class: 'muted fine' }, 'No automations yet.'),
      add);
  }

  function renderYard() {
    if (!catalog) return;
    const items = Array.isArray(shown.items) ? shown.items : [];
    const list = Array.isArray(shown.pets) ? shown.pets : [];
    yardTab.textContent = `In this yard (${items.length + list.length})`;
    const key = keyOf([items, list, catalog.limits]);
    if (key === yardKey) return;
    yardKey = key;
    const focused = yardEl.contains(document.activeElement) ? document.activeElement.name : null;
    yardEl.replaceChildren(
      h('section', { class: 'build-group', 'aria-label': 'Things' },
        h('h3', {}, 'Things'),
        items.length ? h('ul', { class: 'rows' }, items.map(itemRow)) : h('p', { class: 'muted' }, 'Nothing in the yard yet. Pick something from the catalog.')),
      h('section', { class: 'build-group', 'aria-label': 'Pets' },
        h('h3', {}, 'Pets'),
        list.length ? list.map(petCard) : h('p', { class: 'muted' }, 'No pets yet. They are at the end of the catalog.')));
    if (focused) yardEl.querySelector(`[name="${focused}"]`)?.focus({ preventScroll: true });
  }

  function render(force = false) {
    if (!active) return;
    const items = shown.items?.length || 0;
    const count = shown.pets?.length || 0;
    figures.textContent = catalog
      ? `${items} of ${limits().items} items, ${count} of ${limits().pets} pets.`
      : `${items} items, ${count} pets.`;
    canvas.setAttribute('aria-label', `The outside of your ${kind}: ${items} ${items === 1 ? 'thing' : 'things'} in the yard and ${count} ${count === 1 ? 'pet' : 'pets'}.`);
    if (catalog && !pickerOpen()) {
      const key = keyOf([shown, kind, catalog.limits]);
      if (force || key !== renderedKey) {
        renderedKey = key;
        renderCatalog();
      }
      renderYard();
    }
    renderDock();
  }

  // --- The dock, opening and closing ------------------------------------------------------------

  function renderDock() {
    const holding = Boolean(hand);
    dock.hidden = !active;
    frame.root.classList.toggle('has-dock', active);
    frame.root.classList.toggle('outside-open', active);
    nudges.hidden = !holding;
    turnButton.hidden = !holding || hand.type === 'building';
    for (const button of [putButton, cancelButton]) button.hidden = !holding;
    catalogButton.hidden = holding || panels.current === 'outside';
    seeButton.hidden = holding || panels.current !== 'outside' || beside.matches;
    undoButton.hidden = holding;
    undoButton.disabled = !undo.length && !timer;
    nightButton.hidden = holding;
    doneButton.hidden = holding;
    for (const [word, button] of tiles) button.setAttribute('aria-pressed', String(hand?.type === 'item' && word === hand.kind && (hand.index === undefined || hand.index === null)));
  }

  async function finish() {
    await flushed();
    const { x, y } = tile;
    close();
    world.focus(x, y, { zoom: 1.4 });
    world.canvas.focus({ preventScroll: true });
    say('Back on the map.');
  }

  function close() {
    if (!active) return;
    // A colour that is still settling is sent now, because leaving must not lose it.
    if (timer) {
      clearTimeout(timer);
      timer = null;
      save(shown);
    }
    active = false;
    hand = null;
    chosen = null;
    menu.close();
    clearTimeout(timer);
    timer = null;
    stage.hidden = true;
    toggle.hidden = true;
    if (panels.current === 'outside') panels.close(false);
    frame.root.classList.remove('outside-panel');
    renderDock();
    cancelAnimationFrame(raf);
    raf = 0;
    resizer?.disconnect();
    observer?.disconnect();
    observer = null;
    jobs.clear();
    queue.length = 0;
    layers = null;
    ghost = null;
    pets = [];
    petKey = '';
    document.removeEventListener('keydown', onKey, true);
    document.removeEventListener('visibilitychange', lookAgain);
  }

  // Keys while the stage is open. Says nothing to the page's own Escape unless it took the key.
  function onKey(event) {
    if (document.querySelector('dialog[open]')) return;
    if (menu.isOpen) {
      if (event.key === 'Escape') {
        menu.close(true);
        event.preventDefault();
        event.stopPropagation();
      }
      return;
    }
    const typing = event.target.closest?.('input, textarea, select');
    if (!hand) {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'z' && !typing && !event.shiftKey) {
        undoLast();
        event.preventDefault();
      }
      return;
    }
    if (event.key === 'Escape') {
      letGo();
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (typing) return;
    const step = NUDGE_KEYS[event.key];
    if (step) {
      nudge(...step);
      event.preventDefault();
      return;
    }
    if (event.key === 'Enter' && !event.target.closest?.('button, a')) {
      putDown();
      event.preventDefault();
    }
  }

  // Back on this tab after another had it: if the place was changed there and nothing here is waiting to
  // be saved, take that, because the next change here sends the whole document and would undo it.
  async function lookAgain() {
    if (document.hidden || !active || flying || timer || hand || busy) return;
    const era = session;
    try {
      const got = await api(urlOf());
      if (!active || era !== session || flying || timer || hand || busy) return;
      if (same(got.outside || {}, accepted)) return;
      accepted = clone(got.outside);
      shown = accepted;
      undo = [];
      changed(tile.x, tile.y, accepted);
      repaint();
      render();
      tell('This place was changed somewhere else, so this is how it is now.');
    } catch (error) {
      if (active && era === session && error.status === 404) gone();
    }
  }

  async function open(place) {
    if (active) close();
    const era = ++session;
    tile = { x: place.x, y: place.y };
    kind = place.kind || 'house';
    look = world.placeLook(place.x, place.y);
    accepted = clone(current(place.x, place.y));
    shown = accepted;
    undo = [];
    chain = Promise.resolve();
    flying = 0;
    hand = null;
    chosen = null;
    night = false;
    nightButton.textContent = 'See it at night';
    layers = null;
    petKey = '';
    renderedKey = null;
    yardKey = null;
    active = true;
    buildError.hidden = true;
    stage.hidden = false;
    toggle.hidden = false;
    showTab(catalogEl);
    document.addEventListener('keydown', onKey, true);
    document.addEventListener('visibilitychange', lookAgain);
    resizer?.observe(stage);
    renderDock();
    tell('Opening the outside…');
    panels.open('outside');
    fit();
    resync();
    kick();
    try {
      const got = await api(urlOf());
      if (!active || era !== session) return;
      if (got.yours === false) {
        close();
        say('Only the owner can change the outside.');
        return;
      }
      kind = got.kind || kind;
      // E36: the answer says which tile it's stored on. Everything from here on goes under that one.
      if (isWhole(got.x) && isWhole(got.y) && (got.x !== tile.x || got.y !== tile.y)) {
        tile = { x: got.x, y: got.y };
        look = world.placeLook(tile.x, tile.y);
        pets = [];
        petKey = '';
      }
      accepted = clone(got.outside);
      shown = accepted;
    } catch (error) {
      if (!active || era !== session) return;
      if (error.status === 403) {
        close();
        say(error.message);
        return;
      }
      if (error.status === 404) {
        gone();
        return;
      }
      fail(error);
    }
    repaint();
    try {
      if (!catalog) {
        catalog = await api('/api/outside');
        yardKinds = new Map(catalog.yard.map((entry) => [entry.kind, entry]));
      }
    } catch (error) {
      if (active && era === session) {
        catalogEl.replaceChildren(h('p', { class: 'error' }, error.message));
        fail(error);
      }
      return;
    }
    if (!active || era !== session) return;
    render(true);
    tell(idleHint());
  }

  return {
    open,
    close,
    get isOpen() {
      return active;
    },
    // Escape is for what's in hand or a menu first, and only then for the panel.
    takesEscape: () => active && (Boolean(hand) || menu.isOpen),
    // The tile being changed, with what's on the stage, so a read of the map's rows doesn't undo it.
    row: () => (active ? [tile.x, tile.y, shown] : null),
    destroy() {
      close();
      stage.remove();
      dock.remove();
    },
  };
}
