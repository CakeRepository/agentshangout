// The private city map. It is drawn by the same renderer as the public worlds (world.js), so a
// city looks like one: an adapter turns a CityView into the WorldView that renderer wants. The land
// is grass, the Town Square is a block of park in the middle, each house is a `homes` tile on a
// fixed plot, roads join the houses bots have visited, and bots are players. Where a bot stands
// comes from real data: the house it lives in, or the house it is visiting.

import { World } from './world.js';
import { hash } from './room.js';

const N = 28;

// The land is a grid of plots, five rows of eight, with the Town Square across the middle of the
// third row. A plot is one house tile with its street along the tile below it. Houses take plots in
// the order they were built, so a house never moves.
const COLS = 8;
const ROWS = 5;
const X0 = 7;
const Y0 = 5;
const DX = 2;
const DY = 4;

const SQUARE = { x0: X0 + DX * 2, y0: Y0 + DY * 2 - 1, x1: X0 + DX * 5, y1: Y0 + DY * 2 + 1 };
const inSquare = (x, y) => x >= SQUARE.x0 && x <= SQUARE.x1 && y >= SQUARE.y0 && y <= SQUARE.y1;

// The first few go round the square, then the rest work outward.
const FIRST_PLOTS = [[3, 1], [4, 3], [4, 1], [3, 3], [2, 1], [5, 3], [5, 1], [2, 3], [1, 2], [6, 2]];

function makePlots() {
  const all = [];
  for (let row = 0; row < ROWS; row++) {
    for (let col = 0; col < COLS; col++) {
      if (row === 2 && col >= 2 && col <= 5) continue;
      all.push({ col, row, x: X0 + DX * col, y: Y0 + DY * row });
    }
  }
  const first = FIRST_PLOTS.map(([col, row]) => all.find((plot) => plot.col === col && plot.row === row));
  const cx = (SQUARE.x0 + SQUARE.x1) / 2;
  const cy = (SQUARE.y0 + SQUARE.y1) / 2;
  const distance = (plot) => Math.hypot(plot.x - cx, (plot.y - cy) * 1.6);
  const rest = all
    .filter((plot) => !first.includes(plot))
    .sort((a, b) => distance(a) - distance(b) || a.col - b.col || a.row - b.row);
  return [...first, ...rest].map((plot, index) => ({ ...plot, index }));
}

export const PLOTS = makePlots();

// The square is a block of park that fills out from the middle as the city grows: a patch to start
// with, and one more tile for every finished Town Square plan. Parks bring the trees.
const SQUARE_ORDER = (() => {
  const cx = (SQUARE.x0 + SQUARE.x1) / 2;
  const cy = (SQUARE.y0 + SQUARE.y1) / 2;
  const tiles = [];
  for (let y = SQUARE.y0; y <= SQUARE.y1; y++) {
    for (let x = SQUARE.x0; x <= SQUARE.x1; x++) tiles.push([x, y]);
  }
  const far = ([x, y]) => ((x - cx) / 3.5) ** 2 + ((y - cy) / 1.4) ** 2;
  return tiles.sort((a, b) => far(a) - far(b) || a[0] - b[0] || a[1] - b[1]);
})();

const STAGES = ['hamlet', 'village', 'town', 'city'];
const STAGE_NAME = { hamlet: 'Hamlet', village: 'Village', town: 'Town', city: 'City' };
const stageIndex = (stage) => Math.max(0, STAGES.indexOf(stage));
const parkCount = (gatherings) => Math.min(SQUARE_ORDER.length, 9 + gatherings);

// Where bots stand, relative to their house tile. Residents stay beside it and visitors wait out
// front. More bots than spots share a spot, which the renderer nudges apart.
const YARD = [[-1, 1], [1, 1], [-1, 0], [1, 0], [0, 2], [-1, 2], [1, 2]];
const PORCH = [[0, 1], [0, 2], [1, 2], [-1, 2]];

// Roads
//
// Streets run along the tile below each row of houses and in the gap between two houses, and never
// through the square. A road between two houses is the shortest way along them.

const STREET_ROWS = Array.from({ length: ROWS }, (_, row) => Y0 + DY * row + 1);
const GAP_COLS = Array.from({ length: COLS - 1 }, (_, col) => X0 + DX * col + 1);

function isStreet(x, y) {
  if (inSquare(x, y)) return false;
  if (STREET_ROWS.includes(y) && x >= X0 - 1 && x <= X0 + DX * (COLS - 1) + 1) return true;
  return GAP_COLS.includes(x) && y >= STREET_ROWS[0] && y <= STREET_ROWS[ROWS - 1];
}

const doorOf = (plot) => [plot.x, plot.y + 1];

function streetRoute(from, to) {
  const key = ([x, y]) => y * N + x;
  const prev = new Map([[key(from), null]]);
  const queue = [from];
  for (let head = 0; head < queue.length; head++) {
    const here = queue[head];
    if (here[0] === to[0] && here[1] === to[1]) break;
    for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
      const next = [here[0] + dx, here[1] + dy];
      if (!isStreet(...next) || prev.has(key(next))) continue;
      prev.set(key(next), here);
      queue.push(next);
    }
  }
  if (!prev.has(key(to))) return [];
  const route = [];
  for (let at = to; at; at = prev.get(key(at))) route.push(at);
  return route;
}

// Turns the city's roads (house to house, with a count of trips) into road tiles. A road needs a
// visit, and every house it passes shares the street.
function roadTiles(roads, plotOf) {
  const tiles = new Map();
  for (const road of roads || []) {
    const a = plotOf(road.a);
    const b = plotOf(road.b);
    if (!a || !b || a === b || !(road.trips >= 1)) continue;
    for (const [x, y] of streetRoute(doorOf(a), doorOf(b))) tiles.set(y * N + x, [x, y]);
  }
  return [...tiles.values()];
}

// Land: a clearing in a wood. Meadow in the middle where the houses go, a ring of forest round the
// edge that is ragged, so it reads as a place and not a square, and a pond with a sandy shore in one
// corner, clear of the plots. Nothing about the city changes it, so every city looks the same here.
function makeLand() {
  const noise = (x, y) => (Math.imul(x * 73856093 ^ y * 19349663, 2654435761) >>> 8 & 255) / 255;
  const rows = [];
  for (let y = 0; y < N; y++) {
    let row = '';
    let run = 0;
    let last = '';
    for (let x = 0; x < N; x++) {
      const edge = Math.min(x, y, N - 1 - x, N - 1 - y);
      const pond = Math.hypot((x - 3.5) / 1.35, y - 22.5);
      let letter = 'g';
      if (edge + noise(x, y) * 2.2 < 2.9) letter = 'f';
      if (pond < 2.2) letter = 'w';
      else if (pond < 3.2 && letter !== 'f') letter = 's';
      if (letter === last) run++;
      else {
        if (last) row += `${run}${last}`;
        last = letter;
        run = 1;
      }
    }
    rows.push(`${row}${run}${last}`);
  }
  return rows;
}
const LAND = makeLand();

// The window
//
// The bevelled frame the worlds live in. app.js builds the public worlds' windows with this too.

const el = (tag, className, text) => {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
};

// `stats` is a list of [key, label, amber], `live` adds the lamp that says the map is the city as it is
// now, and `heading` is the tag the title is made with, for a page that has no heading of its own. Returns the pieces to fill in: `title` text, `actions`
// at the right of the title bar, the LCD `outputs` by key, `tools` (a toolbar, when asked for), the
// `host` the map is drawn in, and the status bar's `message` and `where`. The message isn't a live
// region until a page makes it one, so hovering doesn't talk over a screen reader.
export function windowFrame({ label, stats, tools = false, heading = 'span', live = false }) {
  const root = el('section', 'wd');
  root.setAttribute('aria-label', label);
  const title = el(heading, 'wd-title-text');
  const actions = el('span', 'wd-title-actions');
  const header = el('header', 'wd-title');
  header.append(title, actions);
  root.append(header);

  const outputs = {};
  const bar = el('div', 'wd-bar wd-stats');
  bar.setAttribute('role', 'group');
  bar.setAttribute('aria-label', 'Town facts');
  for (const [key, text, amber] of stats) {
    const box = el('div', 'wd-stat');
    const out = el('output', amber ? 'wd-lcd amber' : 'wd-lcd', '-');
    // The text summary under the map is what speaks up when things change, not four numbers.
    out.setAttribute('aria-live', 'off');
    box.append(el('span', 'wd-stat-label', text), out);
    bar.append(box);
    outputs[key] = out;
  }
  if (live) {
    const lamp = el('span', 'wd-live');
    const dot = el('span', 'wd-lamp');
    dot.setAttribute('aria-hidden', 'true');
    lamp.append(dot, 'Live');
    header.insertBefore(lamp, actions);
  }
  root.append(bar);

  let toolbar = null;
  if (tools) {
    toolbar = el('div', 'wd-bar wd-tools');
    toolbar.setAttribute('role', 'toolbar');
    toolbar.setAttribute('aria-label', 'Build');
    root.append(toolbar);
  }

  const well = el('div', 'wd-well');
  const host = el('div');
  well.append(host);
  const status = el('footer', 'wd-status');
  const message = el('span', 'wd-seg');
  const where = el('span', 'wd-seg fixed');
  where.setAttribute('aria-hidden', 'true');
  status.append(message, where);
  root.append(well, status);
  return { root, title, actions, outputs, tools: toolbar, host, status, message, where };
}

// The map

const HINT = 'Click a house or the Town Square. Drag to look around.';
const countOf = (n, one, many) => `${n} ${n === 1 ? one : many}`;
const nameList = new Intl.ListFormat(undefined, { style: 'long', type: 'conjunction' });

// Whose home a house is, in a few words: the people who live there are its household.
export function homeOf(view, house) {
  if (house.id === view.you.house_id) return 'Your house';
  const names = house.residents.map((id) => view.people.find((person) => person.id === id)?.name).filter(Boolean);
  return names.length ? `Home of ${nameList.format(names)}` : 'Nobody lives here yet';
}

// H21: what each of your own bots may do at a house, from its passes and keys. A line per bot.
export function houseAccess(view, house) {
  const yours = view.bots.filter((bot) => bot.owner_id === view.you.person_id && !bot.is_house);
  return yours.map((bot) => {
    const pass = (view.your_passes || []).find((item) => item.agent_id === bot.id && item.house_id === house.id);
    const keys = pass?.keys || [];
    const keyText = keys.length
      ? `, with the ${keys.map((key) => (key === 'living_room' ? 'Living Room' : 'scenes')).join(' and ')} key${keys.length > 1 ? 's' : ''}`
      : '';
    let text;
    if (bot.home_house_id === house.id) text = 'lives here';
    else if (bot.away_at_house_id === house.id) text = `is visiting now${pass?.accepted_by ? keyText : ''}`;
    else if (pass?.accepted_by) text = `can come in: trips up to ${pass.max_minutes} minutes start at once${keyText}`;
    else if (pass) text = 'has been offered a pass here: accept it under the map';
    else if (bot.stays_home) text = 'stays home, so it can\'t visit';
    else text = 'needs a yes from someone here each time';
    return { name: bot.name, text, allowed: Boolean(pass?.accepted_by) || bot.home_house_id === house.id };
  });
}

export class City {
  constructor(stage, handlers = {}) {
    this.handlers = handlers;
    this.frame = windowFrame({
      label: 'The city map',
      stats: [['houses', 'Houses'], ['bots', 'Bots'], ['gatherings', 'Gatherings'], ['stage', 'Stage', true]],
    });
    this.frame.root.classList.add('wd-city');
    this.frame.message.textContent = HINT;
    stage.append(this.frame.root);
    // The card that follows the pointer over a house: whose it is and what your bots may do there.
    this.card = el('div', 'city-card');
    this.card.hidden = true;
    this.card.setAttribute('aria-hidden', 'true');
    this.frame.host.parentElement.append(this.card);
    this.pointer = null;

    this.targets = new Map();
    this.houses = [];
    this.lot = null;
    this.view = null;
    this.fitted = false;
    this.world = new World(this.frame.host, {
      interactive: true,
      onHover: (hover) => this.hovered(hover),
      framing: { margin: [300, 300], maxZoom: 1.7 },
    });

    // A click on a tile is not a tool use, so it can't go through the world's onTile, which would
    // need a tool picked and would stop the map being dragged. The world's own picking finds the
    // tile instead, once the pointer has stayed put.
    const canvas = this.world.canvas;
    this.down = null;
    this.onDown = (event) => {
      this.down = { x: event.clientX, y: event.clientY };
    };
    this.onMove = (event) => {
      this.pointer = { x: event.clientX, y: event.clientY };
      if (!this.card.hidden) this.placeCard();
    };
    this.onLeave = () => {
      this.pointer = null;
      this.card.hidden = true;
    };
    canvas.addEventListener('pointermove', this.onMove);
    canvas.addEventListener('pointerleave', this.onLeave);
    this.onClick = (event) => {
      const down = this.down;
      this.down = null;
      if (down && Math.hypot(event.clientX - down.x, event.clientY - down.y) > 6) return;
      const rect = canvas.getBoundingClientRect();
      const tile = this.world.tileAtArt(...this.world.toArt(event.clientX - rect.left, event.clientY - rect.top));
      const target = tile && this.targetAt(tile.x, tile.y);
      if (target) this.open(target);
    };
    canvas.addEventListener('pointerdown', this.onDown);
    canvas.addEventListener('click', this.onClick);

    // The map starts at the zoom that fits the whole town on a narrow screen too, once it has a size.
    this.resizer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => this.fit());
    if (this.resizer) this.resizer.observe(this.frame.host);
  }

  destroy() {
    if (this.resizer) this.resizer.disconnect();
    this.world.canvas.removeEventListener('pointerdown', this.onDown);
    this.world.canvas.removeEventListener('pointermove', this.onMove);
    this.world.canvas.removeEventListener('pointerleave', this.onLeave);
    this.world.canvas.removeEventListener('click', this.onClick);
    this.world.destroy();
    this.frame.root.remove();
  }

  fit() {
    const width = this.frame.host.clientWidth;
    if (this.fitted || !width || !this.world.ready) return;
    this.fitted = true;
    // Close enough to see the houses and who is at their doors, with the whole village in view.
    this.world.frameTown();
  }

  setCity(view) {
    this.view = view;
    const people = new Map(view.people.map((person) => [person.id, person]));
    const bots = view.bots.filter((bot) => !bot.is_house);
    const stage = view.growth.stage;
    // A bigger town makes for bigger houses, and so does a fuller household.
    const level = [2, 2, 2, 3][stageIndex(stage)];

    this.houses = view.houses.slice(0, PLOTS.length).map((house, index) => ({
      id: house.id,
      name: house.name,
      residents: house.residents.map((id) => people.get(id)?.name).filter(Boolean),
      plot: PLOTS[index],
    }));
    const byId = new Map(this.houses.map((house) => [house.id, house]));
    const next = view.houses.length < PLOTS.length && !view.you.house_id ? PLOTS[view.houses.length] : null;
    this.lot = next ? { plot: next } : null;

    const tiles = [];
    for (const [x, y] of SQUARE_ORDER.slice(0, parkCount(view.growth.gatherings))) tiles.push([x, y, 'park', 0]);
    for (const [x, y] of roadTiles(view.growth.roads, (id) => byId.get(id)?.plot)) tiles.push([x, y, 'road', 0]);
    for (const house of this.houses) {
      const here = bots.filter((bot) => (bot.away_at_house_id || bot.home_house_id) === house.id).length;
      tiles.push([house.plot.x, house.plot.y, 'homes', Math.min(3, level + (here >= 3 && stageIndex(stage) >= 2 ? 1 : 0))]);
    }
    if (this.lot) tiles.push([next.x, next.y, 'homes', 0]);

    this.targets = new Map();
    for (let y = SQUARE.y0; y <= SQUARE.y1; y++) {
      for (let x = SQUARE.x0; x <= SQUARE.x1; x++) this.targets.set(`${x},${y}`, { type: 'square' });
    }
    for (const house of this.houses) this.targets.set(`${house.plot.x},${house.plot.y}`, { type: 'house', house });
    if (this.lot) this.targets.set(`${next.x},${next.y}`, { type: 'lot' });

    const population = view.people.length;
    this.world.setView({
      id: view.city.id,
      name: view.city.name,
      kind: 'city',
      size: N,
      seed: hash(view.city.id),
      land: LAND,
      tiles,
      players: this.botsOf(bots, byId),
      population,
      stage,
    });

    const away = bots.filter((bot) => bot.away_at_house_id).length;
    const napping = bots.filter((bot) => bot.state === 'offline').length;
    const label = `${view.city.name}, a ${stage}: ${countOf(view.houses.length, 'house', 'houses')}, `
      + `${countOf(bots.length, 'bot', 'bots')} (${away} visiting, ${napping} napping) `
      + `and ${countOf(view.growth.gatherings, 'gathering', 'gatherings')} in the Town Square.`;
    this.world.setLabels(this.houses.map((house) => ({ x: house.plot.x, y: house.plot.y, text: house.name, rise: 44 + level * 9 })));
    this.world.canvas.setAttribute('aria-label', label);
    this.frame.title.textContent = `${view.city.name} - ${STAGE_NAME[stage] || stage}`;
    const { outputs } = this.frame;
    outputs.houses.textContent = String(view.houses.length);
    outputs.bots.textContent = String(bots.length);
    outputs.gatherings.textContent = String(view.growth.gatherings);
    outputs.stage.textContent = STAGE_NAME[stage] || stage;
    this.fit();
  }

  // Works out where each bot should be from its data: at its house, on the porch of the house it is
  // visiting, or around the square if it has none. A napping bot sleeps where it stands.
  botsOf(bots, byId) {
    const homeless = [];
    const seats = new Map();
    const players = [];
    const spot = (house, slots, id) => {
      const key = `${slots === PORCH ? 'p' : 'y'}${house.id}`;
      const list = seats.get(key) || [];
      if (!list.includes(id)) list.push(id);
      seats.set(key, list);
      const [dx, dy] = slots[(list.indexOf(id)) % slots.length];
      return [house.plot.x + dx, house.plot.y + dy];
    };
    for (const bot of [...bots].sort((a, b) => (a.id < b.id ? -1 : 1))) {
      const visiting = bot.away_at_house_id && byId.get(bot.away_at_house_id);
      const home = bot.home_house_id && byId.get(bot.home_house_id);
      let at;
      if (visiting) at = spot(visiting, PORCH, bot.id);
      else if (home) at = spot(home, YARD, bot.id);
      else {
        homeless.push(bot.id);
        const [x, y] = SQUARE_ORDER[(homeless.length - 1) % SQUARE_ORDER.length];
        at = [x, y];
      }
      players.push([bot.id, bot.name, at[0], at[1], bot.state === 'offline' ? 'sleep' : null, null, this.view.bot_looks[bot.id]]);
    }
    return players;
  }

  // What is at a tile. The buildings stand taller than their tile, so a click on a roof lands on a
  // tile behind the house: look there too, nearest first.
  targetAt(x, y) {
    for (const [dx, dy] of [[0, 0], [1, 1], [1, 0], [0, 1]]) {
      const target = this.targets.get(`${x + dx},${y + dy}`);
      if (target && target.type !== 'square') return target;
    }
    return this.targets.get(`${x},${y}`) || null;
  }

  hovered(hover) {
    const { message, where } = this.frame;
    let target = null;
    if (hover && hover.player) {
      where.textContent = hover.player.name;
    } else if (hover && hover.x !== null) {
      target = this.targetAt(hover.x, hover.y);
      where.textContent = target ? this.nameOf(target) : '';
    } else {
      where.textContent = '';
    }
    this.frame.host.classList.toggle('city-over', Boolean(target));
    message.textContent = target ? this.hintFor(target) : HINT;
    this.showCard(target);
  }

  // Fills the card for a house, or hides it for anything else.
  showCard(target) {
    if (!target || target.type !== 'house' || !this.view || !this.pointer) {
      this.card.hidden = true;
      return;
    }
    const house = this.view.houses.find((item) => item.id === target.house.id);
    if (!house) {
      this.card.hidden = true;
      return;
    }
    const mine = house.id === this.view.you.house_id;
    const lines = [el('strong', 'city-card-name', house.name), el('span', 'city-card-home', homeOf(this.view, house))];
    for (const bot of houseAccess(this.view, house)) {
      const row = el('span', bot.allowed ? 'city-card-bot allowed' : 'city-card-bot');
      row.append(el('b', '', bot.name), ` ${bot.text}`);
      lines.push(row);
    }
    lines.push(el('small', 'city-card-hint', mine ? 'Click to open your house.' : 'Click to ask for a visit for one of your bots.'));
    this.card.replaceChildren(...lines);
    this.card.hidden = false;
    this.placeCard();
  }

  // Beside the pointer, turned back inside the map near its right and bottom edges.
  placeCard() {
    const box = this.card.parentElement.getBoundingClientRect();
    const x = this.pointer.x - box.left;
    const y = this.pointer.y - box.top;
    const w = this.card.offsetWidth;
    const h = this.card.offsetHeight;
    this.card.style.left = `${Math.max(4, x + 16 + w > box.width ? x - w - 16 : x + 16)}px`;
    this.card.style.top = `${Math.max(4, y + 16 + h > box.height ? y - h - 16 : y + 16)}px`;
  }

  nameOf(target) {
    if (target.type === 'house') return target.house.name;
    return target.type === 'square' ? 'Town Square' : 'A free plot';
  }

  hintFor(target) {
    if (target.type === 'square') return 'Town Square: chat and make plans with everyone in the city.';
    if (target.type === 'lot') return 'A free plot: build your house here.';
    const { house } = target;
    const who = house.residents.length ? `${nameList.format(house.residents)} live${house.residents.length === 1 ? 's' : ''} here. ` : '';
    return `${house.name}. ${who}${house.id === this.view?.you.house_id ? 'Open your house.' : 'Click to send a bot.'}`;
  }

  open(target) {
    if (target.type === 'house') this.handlers.house?.(target.house.id);
    else if (target.type === 'square') this.handlers.square?.();
    else this.handlers.lot?.();
  }
}
