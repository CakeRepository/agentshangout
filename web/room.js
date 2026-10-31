// The pixel room. Where a bot stands comes from real presence data; the walking is just for fun.
// People with the room open are drawn in it too, but where a person stands doesn't mean anything.

const W = 480;
const H = 252;
const WALL = 66;
const CELL = 6;
const COLS = W / CELL;
const ROWS = Math.floor(H / CELL);
const SPEED = 38;

// The cells things are put down on: twice as wide as the ones bots walk on, and over the floor only.
const FLOOR_TOP = 84;
const FLOOR_CELL = 12;
const FLOOR_COLS = W / FLOOR_CELL;
const FLOOR_ROWS = (H - FLOOR_TOP) / FLOOR_CELL;

// How often someone who isn't sitting goes and sits down when they next move.
const SIT_CHANCE = 0.35;

const BLOCKED = [
  [0, 0, W, WALL + 8],
  [174, 124, 132, 46], // table
  [12, 190, 96, 34], // couch
  [24, 98, 92, 38], // control desk
  [0, 0, 6, H],
  [W - 6, 0, 6, H],
  [0, H - 6, W, 6],
];

// What an owner can leave out of a floor and bots would otherwise walk round. The table, couch and
// desk stay, because where a bot stands there says what it's doing.
const REMOVABLE = {
  bar: [370, 66, 110, 36],
  arcade: [428, 124, 50, 48],
  plants: [460, 224, 20, 28],
};

// A floor's look. These are listed words on the server, so nothing typed reaches the drawing.
const PAINTS = {
  dusk: { upper: '#364466', stripe: '#3a496d', lower: '#2a3450', panel: '#232c45' },
  sage: { upper: '#4f6b5c', stripe: '#567464', lower: '#3a5145', panel: '#304439' },
  plum: { upper: '#5b3f66', stripe: '#63466f', lower: '#46304f', panel: '#3a2742' },
  clay: { upper: '#8a5a44', stripe: '#94614a', lower: '#6b4333', panel: '#5a382a' },
  butter: { upper: '#b89a5e', stripe: '#c2a468', lower: '#8f7646', panel: '#7a643a' },
  ink: { upper: '#23293a', stripe: '#292f42', lower: '#1a1f2d', panel: '#141824' },
  haunted: { upper: '#2c2042', stripe: '#33264c', lower: '#20172f', panel: '#2b1a2e', webs: true },
};
const FLOORINGS = {
  oak: { a: '#a97b4e', b: '#a27347', line: '#8b6039', style: 'planks' },
  walnut: { a: '#6e4a30', b: '#67442b', line: '#523521', style: 'planks' },
  tile: { a: '#c9c4b5', b: '#8d8a80', line: '#77746b', style: 'tile' },
  carpet: { a: '#4d6a78', b: '#4a6673', line: '#415a66', style: 'carpet' },
  crypt: { a: '#57545f', b: '#4e4b56', line: '#34323c', style: 'flagstones' },
};
const PLAIN_LOOK = { paint: 'dusk', flooring: 'oak', hidden: [] };

const SEATS = [
  ...[192, 224, 256, 288].map((x, i) => ({ x, y: 127, ax: x, ay: 116, pose: 'sit', facing: 'front', drop: i % 2 })),
  ...[192, 224, 256, 288].map((x, i) => ({ x, y: 180, ax: x, ay: 188, pose: 'sit', facing: 'back', drop: i % 2 })),
  { x: 160, y: 152, ax: 150, ay: 152, pose: 'sit', facing: 'right', drop: 0 },
  { x: 320, y: 152, ax: 330, ay: 152, pose: 'sit', facing: 'left', drop: 0 },
];

// Where a bot sits while it's controlling something through another tool.
const DESKS = [44, 70, 96].map((x) => ({ x, y: 146, ax: x, ay: 158, pose: 'sit', facing: 'back', drop: 0 }));

const NAPS = [
  { x: 40, y: 206, ax: 40, ay: 232, pose: 'sleep' },
  { x: 68, y: 206, ax: 68, ay: 232, pose: 'sleep' },
  { x: 96, y: 206, ax: 96, ay: 232, pose: 'sleep' },
  { x: 132, y: 238, ax: 132, ay: 228, pose: 'sleep' },
  { x: 164, y: 238, ax: 164, ay: 228, pose: 'sleep' },
  { x: 196, y: 238, ax: 196, ay: 228, pose: 'sleep' },
];

const SPOTS = [
  [396, 110], [424, 110], [452, 110], // coffee
  [442, 182], [464, 182], // arcade
  [62, 84], [22, 172], [352, 112], // window and corners
  [134, 112], [134, 196], [346, 196],
  [240, 226], [300, 232], [400, 214], [120, 164],
];

const GRID = new Uint8Array(COLS * ROWS);

// Bots and people walk where the furniture isn't, so this follows what a floor has left out and
// what has been put on it.
function fillGrid(hidden, items = []) {
  const left = Object.entries(REMOVABLE).filter(([item]) => !hidden.includes(item));
  const blocks = [
    ...BLOCKED,
    ...left.map(([, block]) => block),
    ...items.map(({ left, top, wide, deep }) => [left, top, wide, deep]),
  ];
  for (let row = 0; row < ROWS; row++) {
    for (let col = 0; col < COLS; col++) {
      const x = col * CELL + CELL / 2;
      const y = row * CELL + CELL / 2;
      GRID[row * COLS + col] = blocks.some(([bx, by, bw, bh]) => x >= bx && x < bx + bw && y >= by && y < by + bh) ? 1 : 0;
    }
  }
}
fillGrid([]);

const free = (col, row) => col >= 0 && row >= 0 && col < COLS && row < ROWS && !GRID[row * COLS + col];
const clear = (x, y) => free(Math.floor(x / CELL), Math.floor(y / CELL));

function nearestFree(col, row) {
  for (let radius = 0; radius < 30; radius++) {
    for (let dy = -radius; dy <= radius; dy++) {
      for (let dx = -radius; dx <= radius; dx++) {
        if (free(col + dx, row + dy)) return [col + dx, row + dy];
      }
    }
  }
  return [col, row];
}

// Something can be put down on a spot, and then whoever was going there stands beside it.
function clearOf(x, y) {
  if (clear(x, y)) return [x, y];
  const [col, row] = nearestFree(Math.floor(x / CELL), Math.floor(y / CELL));
  return [col * CELL + CELL / 2, row * CELL + CELL / 2];
}

function standAt(spots) {
  const [x, y] = clearOf(...spots[Math.floor(Math.random() * spots.length)]);
  return { x, y, ax: x, ay: y, pose: 'stand', facing: 'front' };
}

const DIRS = [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]];

function findPath(fromX, fromY, toX, toY) {
  const [startCol, startRow] = nearestFree(Math.floor(fromX / CELL), Math.floor(fromY / CELL));
  const [endCol, endRow] = nearestFree(Math.floor(toX / CELL), Math.floor(toY / CELL));
  const start = startRow * COLS + startCol;
  const end = endRow * COLS + endCol;
  const cameFrom = new Int32Array(COLS * ROWS).fill(-1);
  cameFrom[start] = start;
  const queue = [start];
  for (let head = 0; head < queue.length && cameFrom[end] < 0; head++) {
    const cell = queue[head];
    const col = cell % COLS;
    const row = (cell - col) / COLS;
    for (const [dx, dy] of DIRS) {
      const nextCol = col + dx;
      const nextRow = row + dy;
      if (!free(nextCol, nextRow) || !free(col + dx, row) || !free(col, row + dy)) continue;
      const next = nextRow * COLS + nextCol;
      if (cameFrom[next] >= 0) continue;
      cameFrom[next] = cell;
      queue.push(next);
    }
  }
  if (cameFrom[end] < 0) return [{ x: toX, y: toY }];
  const path = [];
  for (let cell = end; cell !== start; cell = cameFrom[cell]) {
    const col = cell % COLS;
    path.unshift({ x: col * CELL + CELL / 2, y: (cell - col) / COLS * CELL + CELL / 2 });
  }
  path.push({ x: toX, y: toY });
  return path;
}

export function shade(hex, amount) {
  const n = parseInt(hex.slice(1), 16);
  const mix = (v) => Math.round(amount < 0 ? v * (1 + amount) : v + (255 - v) * amount);
  return `rgb(${mix(n >> 16)},${mix((n >> 8) & 255)},${mix(n & 255)})`;
}

export function hash(text) {
  let value = 0;
  for (const ch of text) value = (value * 31 + ch.charCodeAt(0)) >>> 0;
  return value;
}

const r = (g, color, x, y, w, h) => {
  g.fillStyle = color;
  g.fillRect(x, y, w, h);
};

export function skyFor(hour) {
  if (hour < 5 || hour >= 21) return { top: '#0c1330', low: '#1b2550', stars: true, dim: 0.26 };
  if (hour < 8) return { top: '#6d8fd0', low: '#f4b183', stars: false, dim: 0.08 };
  if (hour < 17) return { top: '#6fb9ec', low: '#bfe6f5', stars: false, dim: 0 };
  if (hour < 19) return { top: '#5a6fb5', low: '#f19a6b', stars: false, dim: 0.08 };
  return { top: '#2a2f6b', low: '#c2607a', stars: true, dim: 0.16 };
}

function paintWindow(g, x, y, w, h, sky) {
  r(g, '#20283e', x - 3, y - 3, w + 6, h + 7);
  const gradient = g.createLinearGradient(0, y, 0, y + h);
  gradient.addColorStop(0, sky.top);
  gradient.addColorStop(1, sky.low);
  g.fillStyle = gradient;
  g.fillRect(x, y, w, h);
  if (sky.stars) {
    for (let i = 0; i < 9; i++) r(g, '#fff', x + ((i * 37) % w), y + ((i * 23) % (h - 8)), 1, 1);
  }
  g.fillStyle = sky.stars ? '#141a3a' : '#5f8f6d';
  for (let i = 0; i < w; i++) {
    const hill = Math.round(5 + 3 * Math.sin((i + x) / 9));
    g.fillRect(x + i, y + h - hill, 1, hill);
  }
  r(g, '#20283e', x + w / 2 - 1, y, 2, h);
  r(g, '#20283e', x, y + h / 2 - 1, w, 2);
  r(g, '#d8c7a3', x - 5, y + h + 3, w + 10, 2);
}

// Cobwebs in the top corners, and bats asleep on the wall in the gaps either side of the whiteboard.
function paintWebs(g) {
  const silk = 'rgba(226,222,240,.34)';
  for (const [corner, away] of [[0, 1], [W - 1, -1]]) {
    const dot = (angle, reach) =>
      r(g, silk, corner + away * Math.round(Math.cos(angle) * reach), Math.round(Math.sin(angle) * reach), 1, 1);
    for (const turn of [0.14, 0.4, 0.68, 0.94]) {
      for (let reach = 0; reach < 38; reach++) dot((turn * Math.PI) / 2, reach);
    }
    for (const ring of [13, 24, 35]) {
      for (let i = 0; i <= ring * 2; i++) {
        const angle = ((i / (ring * 2)) * Math.PI) / 2;
        dot(angle, ring - Math.abs(Math.round(Math.sin(angle * 8) * 1.5)));
      }
    }
  }
  const bat = '#120c1e';
  for (const [x, y] of [[126, 16], [138, 30], [346, 12], [358, 26], [462, 34]]) {
    r(g, bat, x - 1, y - 2, 3, 4);
    r(g, bat, x - 1, y - 3, 1, 1);
    r(g, bat, x + 1, y - 3, 1, 1);
    r(g, bat, x - 5, y - 1, 4, 2);
    r(g, bat, x + 2, y - 1, 4, 2);
    r(g, bat, x - 6, y - 2, 2, 2);
    r(g, bat, x + 5, y - 2, 2, 2);
    r(g, bat, x - 4, y + 1, 1, 1);
    r(g, bat, x + 3, y + 1, 1, 1);
    r(g, '#ff8a3d', x - 1, y - 1, 1, 1);
    r(g, '#ff8a3d', x + 1, y - 1, 1, 1);
  }
}

function paintFloor(g, { a, b, line, style }) {
  if (style === 'flagstones') {
    r(g, a, 0, WALL + 1, W, H - WALL - 1);
    for (let y = WALL + 1, row = 0; y < H; y += 20, row++) {
      for (let x = (row % 2) * -18, col = 0; x < W; x += 36, col++) {
        const worn = (row * 5 + col * 3) % 4;
        if (worn === 0) r(g, b, x + 1, y, 35, 19);
        r(g, line, x, y, 1, 19);
        // A crack here and there, and moss where the stones meet.
        if (worn === 1) {
          r(g, line, x + 12, y + 4, 6, 1);
          r(g, line, x + 17, y + 5, 1, 5);
          r(g, line, x + 18, y + 9, 5, 1);
        }
        if (worn === 2) {
          r(g, '#4f6b48', x + 1, y + 15, 7, 4);
          r(g, '#61805a', x + 2, y + 17, 3, 2);
        }
      }
      r(g, line, 0, y + 19, W, 1);
    }
    return;
  }
  if (style === 'tile') {
    for (let y = WALL + 1, row = 0; y < H; y += 12, row++) {
      for (let x = 0, col = 0; x < W; x += 12, col++) r(g, (row + col) % 2 ? a : b, x, y, 12, 12);
    }
    return;
  }
  if (style === 'carpet') {
    r(g, a, 0, WALL + 1, W, H - WALL - 1);
    for (let y = WALL + 3; y < H; y += 4) {
      for (let x = (y % 8) * 2; x < W; x += 8) r(g, b, x, y, 1, 1);
    }
    return;
  }
  for (let y = WALL + 1, row = 0; y < H; y += 10, row++) {
    r(g, row % 2 ? a : b, 0, y, W, 10);
    r(g, line, 0, y + 9, W, 1);
    for (let x = (row * 37) % 60; x < W; x += 60) r(g, line, x, y, 1, 9);
  }
}

function paintRoom(g, hour, look) {
  const sky = skyFor(hour);
  const paint = PAINTS[look.paint] || PAINTS.dusk;
  // Wall
  r(g, paint.upper, 0, 0, W, WALL);
  for (let x = 0; x < W; x += 16) r(g, paint.stripe, x, 0, 8, 46);
  r(g, paint.lower, 0, 46, W, 18);
  for (let x = 6; x < W; x += 24) r(g, paint.panel, x, 48, 18, 14);
  r(g, '#c9a86a', 0, 45, W, 1);
  r(g, '#1b2236', 0, 64, W, 3);

  paintWindow(g, 36, 10, 72, 32, sky);
  paintWindow(g, 372, 10, 72, 32, sky);
  if (paint.webs) paintWebs(g);

  // Floor
  paintFloor(g, FLOORINGS[look.flooring] || FLOORINGS.oak);
  r(g, 'rgba(30,20,50,.20)', 0, WALL + 1, W, 3);

  // Rug under the table
  if (!look.hidden.includes('rug')) {
    r(g, '#6a2a3c', 128, 104, 224, 100);
    r(g, '#c9a86a', 132, 108, 216, 1);
    r(g, '#c9a86a', 132, 199, 216, 1);
    r(g, '#c9a86a', 132, 108, 1, 92);
    r(g, '#c9a86a', 347, 108, 1, 92);
    r(g, '#7b3346', 138, 114, 204, 80);
  }

  // Whiteboard frame; the words on it are an HTML overlay
  r(g, '#20283e', 148, 5, 184, 52);
  r(g, '#f4f1e8', 151, 8, 178, 46);
  r(g, '#c9a86a', 146, 56, 188, 2);

  if (sky.dim) {
    r(g, `rgba(18,20,60,${sky.dim})`, 0, 0, W, H);
  }
}

function paintBar(g, t) {
  r(g, '#5a3a24', 372, 72, 106, 28);
  r(g, '#7a5233', 372, 70, 106, 4);
  r(g, '#47301f', 372, 96, 106, 4);
  // Espresso machine and mugs
  r(g, '#b9c2d4', 388, 54, 26, 16);
  r(g, '#7c8699', 392, 62, 18, 5);
  r(g, '#ff5d7a', 408, 56, 3, 2);
  if (Math.floor(t / 900) % 2) r(g, '#fff', 400, 49, 1, 3);
  else r(g, '#fff', 402, 48, 1, 3);
  for (let i = 0; i < 4; i++) r(g, ['#ffb454', '#4f9dff', '#3ddc97', '#ff5d7a'][i], 426 + i * 10, 64, 6, 6);
}

function paintArcade(g, t) {
  for (const [x, glow] of [[432, '#2fd6d6'], [454, '#ff7ad9']]) {
    r(g, '#26233f', x, 124, 20, 48);
    r(g, '#3a3560', x, 124, 20, 3);
    r(g, '#0d1020', x + 3, 130, 14, 14);
    const blip = Math.floor(t / 300 + x) % 3;
    r(g, glow, x + 5 + blip * 3, 134 + blip, 3, 3);
    r(g, shade(glow, 0.4), x + 7, 140, 6, 1);
    r(g, '#3a3560', x + 2, 148, 16, 5);
    r(g, '#ffd24d', x + 5, 149, 2, 2);
    r(g, '#ff5d7a', x + 12, 149, 2, 2);
  }
}

function paintDesk(g, t, lit) {
  r(g, '#00000030', 26, 134, 90, 3);
  r(g, '#4a3120', 30, 126, 5, 9);
  r(g, '#4a3120', 106, 126, 5, 9);
  r(g, '#6b4628', 26, 118, 88, 8);
  r(g, '#8a5c37', 26, 116, 88, 3);
  DESKS.forEach((desk, i) => {
    const x = desk.x - 11;
    r(g, '#20283e', desk.x - 2, 112, 4, 5);
    r(g, '#20283e', desk.x - 6, 116, 12, 1);
    r(g, '#20283e', x, 96, 22, 16);
    if (!lit[i]) {
      r(g, '#2b3045', x + 1, 97, 20, 14);
      return;
    }
    // A remote desktop: a window, a taskbar and a cursor that's clearly someone else's.
    r(g, '#2a6fdb', x + 1, 97, 20, 14);
    r(g, '#e8ecf4', x + 3, 99, 11, 7);
    r(g, '#20283e', x + 3, 99, 11, 1);
    r(g, '#9aa3b8', x + 5, 102, 7, 1);
    r(g, '#9aa3b8', x + 5, 104, 5, 1);
    r(g, '#123a7a', x + 1, 109, 20, 2);
    const phase = t / 900 + i * 2;
    const cursorX = Math.round(x + 10 + 8 * Math.sin(phase));
    const cursorY = Math.round(103 + 4 * Math.cos(phase * 1.7));
    r(g, '#ffde59', cursorX, cursorY, 2, 2);
    r(g, '#ffde59', cursorX + 2, cursorY + 2, 1, 1);
  });
}

function paintCouch(g) {
  r(g, '#3f5f8c', 14, 188, 92, 14);
  r(g, '#35527a', 14, 200, 92, 6);
  r(g, '#5277ad', 18, 202, 84, 12);
  r(g, '#35527a', 12, 196, 8, 24);
  r(g, '#35527a', 100, 196, 8, 24);
  r(g, '#2c4466', 18, 214, 84, 6);
  r(g, '#20334e', 22, 220, 4, 4);
  r(g, '#20334e', 94, 220, 4, 4);
}

function paintBeanbags(g) {
  for (const [x, color] of [[132, '#8a5fb5'], [164, '#3a9d88'], [196, '#c9694a']]) {
    r(g, shade(color, -0.2), x - 12, 236, 24, 8);
    r(g, color, x - 11, 232, 22, 8);
    r(g, shade(color, 0.2), x - 8, 233, 12, 2);
  }
}

function paintPlant(g, x, y) {
  r(g, '#b0623d', x - 5, y - 8, 10, 8);
  r(g, '#8f4c2d', x - 5, y - 2, 10, 2);
  r(g, '#2f8f5b', x - 7, y - 16, 14, 8);
  r(g, '#3fb573', x - 4, y - 22, 8, 8);
  r(g, '#2f8f5b', x - 1, y - 26, 3, 6);
}

// What people can put on a floor. Each one stands on its cells the way paintPlant does: x is the
// middle of them and y is their bottom edge, and it rises from there, so what's tall leans over the
// floor behind it. `wide` and `deep` are the cells it takes, in pixels, as it's turned now. A table
// looks the same either way round, so it has one painting, and anything else has a `side` for when
// it's seen from its end. `t` is 0 for someone who asked for less motion, so nothing moves then.

const SHADOW = '#00000030';
const BOOKS = ['#ff5d7a', '#4f9dff', '#ffb454', '#3ddc97', '#b388ff', '#f4f1e8'];

// Seen from its end: its top runs back across the floor, above the end that faces the room.
function paintEnd(g, x, y, wide, deep, tall, top, face) {
  const left = x - wide / 2;
  r(g, SHADOW, left, y - 2, wide, 2);
  r(g, top, left, y - tall - deep, wide, deep);
  r(g, shade(top, 0.16), left, y - tall - deep, wide, 1);
  r(g, face, left, y - tall, wide, tall - 1);
  r(g, shade(top, 0.16), left, y - tall, wide, 1);
}

// A `side` that's only an end, with whatever `detail` adds along its top.
const endOf = (tall, top, face, detail) => (g, x, y, t, wide, deep) => {
  paintEnd(g, x, y, wide - 2, deep, tall, top, face);
  detail?.(g, x, y - tall - deep, deep, t);
};

// Something thin standing on its edge, like a board: feet at each end and a rail between them.
const edgeOf = (tall, top, face) => (g, x, y, t, wide, deep) => {
  paintEnd(g, x, y, 4, deep - 2, tall, top, face);
  r(g, face, x - 5, y - 2, 10, 2);
  r(g, face, x - 5, y - deep + 1, 10, 2);
};

// A table from above and in front, on two legs. Gives back the top's far edge, for what stands on it.
function paintTop(g, x, y, wide, deep, tall, wood, edge, leg) {
  const left = x - wide / 2 + 1;
  const across = wide - 2;
  const back = y - tall - deep + 6;
  r(g, SHADOW, left, y - 2, across, 2);
  r(g, leg, left + 2, y - tall, 3, tall - 1);
  r(g, leg, left + across - 5, y - tall, 3, tall - 1);
  r(g, wood, left, back, across, deep - 6);
  r(g, shade(wood, 0.14), left, back, across, 1);
  r(g, edge, left, y - tall, across, 2);
  return back;
}

const COUCH_CLOTH = ['#3f5f8c', '#35527a', '#5277ad'];
const SOFA_CLOTH = ['#a85a44', '#8a4636', '#c9775c'];
const LOVE_CLOTH = ['#a83f62', '#8a2f4e', '#cf5f84'];

// A chair, sofa or love seat from the front, with one cushion for each two cells of its width.
function paintSeat(g, x, y, wide, [cloth, dark, light]) {
  const left = x - wide / 2 + 1;
  const across = wide - 2;
  const seats = Math.floor(wide / 24);
  const each = Math.floor((across - 12) / seats);
  r(g, SHADOW, left, y - 2, across, 2);
  r(g, cloth, left + 3, y - 25, across - 6, 13);
  r(g, shade(cloth, 0.12), left + 3, y - 25, across - 6, 1);
  r(g, dark, left + 6, y - 13, across - 12, 3);
  for (let seat = 0; seat < seats; seat++) r(g, light, left + 6 + seat * each, y - 12, each - 1, 6);
  r(g, dark, left + 6, y - 6, across - 12, 4);
  r(g, dark, left, y - 16, 6, 14);
  r(g, dark, left + across - 6, y - 16, 6, 14);
  r(g, shade(dark, -0.3), left + 2, y - 2, 3, 2);
  r(g, shade(dark, -0.3), left + across - 5, y - 2, 3, 2);
}

// The same seat from its end: the back down one side, the cushions along the other.
function paintSeatEnd(g, x, y, wide, deep, [cloth, dark, light]) {
  const left = x - wide / 2 + 1;
  const across = wide - 2;
  const seats = Math.floor(deep / 24);
  const each = Math.floor((deep - 8) / seats);
  const back = y - 14 - deep + 4;
  r(g, SHADOW, left, y - 2, across, 2);
  r(g, dark, left + 6, back, across - 6, deep - 2);
  for (let seat = 0; seat < seats; seat++) r(g, light, left + 8, back + 4 + seat * each, across - 10, each - 1);
  r(g, cloth, left, back - 6, 7, deep + 2);
  r(g, shade(cloth, 0.12), left, back - 6, 7, 1);
  r(g, dark, left, y - 16, across, 14);
  r(g, shade(dark, 0.14), left, y - 16, across, 1);
  r(g, shade(dark, -0.3), left + 2, y - 2, 3, 2);
  r(g, shade(dark, -0.3), left + across - 5, y - 2, 3, 2);
}

const paintArmchair = (g, x, y, t, wide) => paintSeat(g, x, y, wide, COUCH_CLOTH);
const paintSofa = (g, x, y, t, wide) => paintSeat(g, x, y, wide, SOFA_CLOTH);

function paintHeart(g, x, y, color) {
  r(g, color, x, y, 2, 1);
  r(g, color, x + 3, y, 2, 1);
  r(g, color, x, y + 1, 5, 1);
  r(g, color, x + 1, y + 2, 3, 1);
  r(g, color, x + 2, y + 3, 1, 1);
}

function paintLoveSeat(g, x, y, t, wide) {
  paintSeat(g, x, y, wide, LOVE_CLOTH);
  paintHeart(g, x - 2, y - 21, '#ffc2d4');
}

function paintStool(g, x, y) {
  r(g, SHADOW, x - 5, y - 2, 10, 2);
  r(g, '#6b4628', x - 4, y - 8, 2, 7);
  r(g, '#6b4628', x + 2, y - 8, 2, 7);
  r(g, '#6b4628', x - 3, y - 5, 6, 1);
  r(g, '#8a5c37', x - 5, y - 11, 10, 3);
  r(g, '#a0703f', x - 5, y - 11, 10, 1);
}

function paintBookshelf(g, x, y) {
  const left = x - 23;
  r(g, SHADOW, left, y - 2, 46, 2);
  r(g, '#4a3120', left + 1, y - 28, 44, 27);
  r(g, '#8a5c37', left, y - 30, 46, 3);
  r(g, '#a0703f', left, y - 30, 46, 1);
  for (let shelf = 0; shelf < 3; shelf++) {
    const top = y - 26 + shelf * 8;
    r(g, '#2c1d12', left + 2, top, 42, 7);
    for (let i = 0; i < 21; i++) {
      if ((i * 7 + shelf * 5) % 11 === 0) continue;
      const tall = 4 + ((i * 2 + shelf) % 3);
      r(g, BOOKS[(i + shelf * 2) % BOOKS.length], left + 2 + i * 2, top + 7 - tall, 2, tall);
    }
  }
}

function paintCabinet(g, x, y) {
  r(g, SHADOW, x - 17, y - 2, 34, 2);
  r(g, '#4a3120', x - 16, y - 3, 3, 2);
  r(g, '#4a3120', x + 13, y - 3, 3, 2);
  r(g, '#6b4628', x - 16, y - 24, 32, 21);
  r(g, '#8a5c37', x - 17, y - 26, 34, 3);
  r(g, '#a0703f', x - 17, y - 26, 34, 1);
  for (const door of [x - 15, x + 1]) {
    r(g, '#7d5231', door, y - 22, 14, 17);
    r(g, '#6f4728', door + 2, y - 20, 10, 13);
    r(g, '#7d5231', door + 3, y - 19, 8, 11);
  }
  r(g, '#ffd24d', x - 3, y - 14, 2, 2);
  r(g, '#ffd24d', x + 1, y - 14, 2, 2);
}

function paintLamp(g, x, y, t) {
  const glow = `rgba(255,214,120,${Math.floor(t / 1700) % 2 ? 0.2 : 0.16})`;
  r(g, glow, x - 11, y - 3, 22, 3);
  r(g, glow, x - 11, y - 36, 22, 15);
  r(g, glow, x - 8, y - 39, 16, 21);
  r(g, '#20283e', x - 5, y - 2, 10, 2);
  r(g, '#3b4158', x - 1, y - 23, 2, 21);
  r(g, '#fff0b8', x - 3, y - 33, 6, 2);
  r(g, '#ffde59', x - 4, y - 31, 8, 3);
  r(g, '#ffb454', x - 5, y - 28, 10, 4);
  r(g, '#c9843a', x - 5, y - 25, 10, 1);
}

function paintMonitor(g, x, y, t) {
  r(g, SHADOW, x - 8, y - 2, 16, 2);
  r(g, '#20283e', x - 6, y - 2, 12, 2);
  r(g, '#20283e', x - 1, y - 9, 2, 7);
  r(g, '#20283e', x - 11, y - 25, 22, 16);
  r(g, '#3b4158', x - 11, y - 25, 22, 1);
  r(g, '#0d1020', x - 10, y - 24, 20, 14);
  r(g, '#3ddc97', x - 8, y - 22, 9, 1);
  r(g, '#3ddc97', x - 8, y - 20, 13, 1);
  r(g, '#7fd4ff', x - 8, y - 18, 6, 1);
  r(g, '#3ddc97', x - 8, y - 16, 11, 1);
  r(g, Math.floor(t / 530) % 2 ? '#0d1020' : '#3ddc97', x - 8, y - 14, 3, 2);
}

function paintServerRack(g, x, y, t) {
  r(g, SHADOW, x - 11, y - 2, 22, 2);
  r(g, '#1b2030', x - 10, y - 30, 20, 29);
  r(g, '#3b4158', x - 10, y - 30, 20, 2);
  for (let i = 0; i < 5; i++) {
    const top = y - 27 + i * 5;
    r(g, '#2b3045', x - 9, top, 18, 4);
    r(g, '#0d1020', x, top + 1, 8, 1);
    r(g, '#0d1020', x, top + 2, 8, 1);
    const blink = Math.floor(t / 410 + i * 1.7) % 3;
    r(g, blink ? '#3ddc97' : '#1f5c47', x - 8, top + 1, 2, 2);
    r(g, blink === 2 ? '#ffb454' : '#5c4726', x - 5, top + 1, 2, 2);
  }
}

// Stands on its own legs and has the same frame and tray as the one on the wall.
function paintWhiteboard(g, x, y) {
  r(g, SHADOW, x - 26, y - 2, 52, 2);
  r(g, '#20283e', x - 24, y - 8, 2, 7);
  r(g, '#20283e', x + 22, y - 8, 2, 7);
  r(g, '#20283e', x - 28, y - 32, 56, 24);
  r(g, '#f4f1e8', x - 27, y - 31, 54, 21);
  r(g, '#c9a86a', x - 29, y - 9, 58, 2);
  r(g, '#4f9dff', x - 24, y - 28, 20, 1);
  r(g, '#4f9dff', x - 24, y - 25, 27, 1);
  r(g, '#4f9dff', x - 24, y - 22, 14, 1);
  r(g, '#ff5d7a', x - 24, y - 18, 18, 1);
  r(g, '#ff5d7a', x - 24, y - 15, 10, 1);
  r(g, '#3ddc97', x + 9, y - 28, 14, 10);
  r(g, '#f4f1e8', x + 10, y - 27, 12, 8);
  r(g, '#ffb454', x + 12, y - 25, 5, 1);
  r(g, '#ffb454', x + 12, y - 22, 8, 1);
}

function paintEasel(g, x, y) {
  r(g, SHADOW, x - 9, y - 2, 18, 2);
  r(g, '#6b4628', x - 9, y - 13, 2, 11);
  r(g, '#6b4628', x + 7, y - 13, 2, 11);
  r(g, '#4a3120', x - 1, y - 37, 2, 34);
  r(g, '#c9a86a', x - 11, y - 34, 22, 21);
  r(g, '#f4f1e8', x - 10, y - 33, 20, 19);
  r(g, '#7fd4ff', x - 9, y - 32, 18, 9);
  r(g, '#ffd24d', x + 3, y - 31, 4, 4);
  r(g, '#3ddc97', x - 9, y - 23, 18, 5);
  r(g, '#2f8f5b', x - 6, y - 25, 7, 3);
  r(g, '#ff8a3d', x - 9, y - 18, 18, 3);
  r(g, '#6b4628', x - 12, y - 13, 24, 2);
}

// The big table's wood, with a mug someone left on it.
function paintCoffeeTable(g, x, y, t, wide, deep) {
  const top = paintTop(g, x, y, wide, deep, 7, '#8a5c37', '#6f4728', '#6b4628');
  r(g, '#7d5231', x - wide / 2 + 2, top + deep - 9, wide - 4, 1);
  r(g, '#4f9dff', x - 9, top + 3, 8, 6);
  r(g, '#f4f1e8', x - 8, top + 4, 6, 1);
  r(g, '#f4f1e8', x + 3, top + 2, 5, 5);
  r(g, '#f4f1e8', x + 8, top + 3, 1, 3);
  r(g, '#6b4628', x + 4, top + 2, 3, 1);
  r(g, '#fff', x + 4 + Math.floor(t / 900) % 2 * 2, top - 2, 1, 2);
}

function paintGameTable(g, x, y, t, wide, deep) {
  const top = paintTop(g, x, y, wide, deep, 9, '#6b4628', '#4a3120', '#4a3120');
  r(g, '#2f7d5b', x - wide / 2 + 4, top + 3, wide - 8, deep - 12);
  r(g, '#3a9d73', x - wide / 2 + 4, top + 3, wide - 8, 1);
  r(g, '#f4f1e8', x - 8, top + 6, 6, 8);
  r(g, '#ff5d7a', x - 7, top + 8, 4, 4);
  r(g, '#f4f1e8', x, top + 7, 6, 8);
  r(g, '#20283e', x + 1, top + 9, 4, 4);
  r(g, '#fff', x + 8, top + 11, 4, 4);
  r(g, '#20283e', x + 9, top + 12, 1, 1);
  r(g, '#20283e', x + 11, top + 14, 1, 1);
  r(g, '#ffb454', x - 14, top + 8, 3, 3);
  r(g, '#ff5d7a', x - 14, top + 11, 3, 3);
  r(g, '#4f9dff', x - 11, top + 8, 3, 3);
}

function paintStandingDesk(g, x, y, t, wide, deep) {
  const top = paintTop(g, x, y, wide, deep, 20, '#a0703f', '#6f4728', '#3b4158');
  r(g, '#3b4158', x - wide / 2 + 3, y - 3, wide - 6, 2);
  r(g, '#2b3045', x - 7, top + 3, 14, 9);
  r(g, Math.floor(t / 1100) % 2 ? '#7fd4ff' : '#9fe0ff', x - 6, top + 4, 12, 7);
  r(g, '#3b4158', x - 8, top + 12, 16, 2);
  r(g, '#f4f1e8', x + 11, top + 6, 4, 5);
}

function paintCandlelitTable(g, x, y, t, wide, deep) {
  const top = paintTop(g, x, y, wide, deep, 10, '#f4f1e8', '#d9d3c3', '#4a3120');
  r(g, '#e6e0d0', x - wide / 2 + 3, y - 8, wide - 6, 3);
  r(g, 'rgba(255,214,120,.24)', x - 13, top + 1, 26, 15);
  for (const [i, cx] of [-7, 5].entries()) {
    const flick = Math.floor(t / 210 + i) % 2;
    r(g, '#c9a86a', cx - 1, top + 12, 4, 2);
    r(g, '#fff4c2', cx, top + 6, 2, 6);
    r(g, '#ffb454', cx, top + 3 + flick, 2, 3 - flick);
    r(g, '#ffde59', cx, top + 5, 1, 1);
  }
  r(g, '#ff5d7a', x - 2, top + 16, 3, 3);
  r(g, '#2f8f5b', x + 1, top + 18, 3, 1);
}

function paintFountain(g, x, y, t) {
  const ripple = Math.floor(t / 420) % 4;
  const drop = Math.floor(t / 150) % 3;
  r(g, SHADOW, x - 17, y - 2, 34, 2);
  r(g, '#8d8a80', x - 17, y - 30, 34, 26);
  r(g, '#c9c4b5', x - 17, y - 30, 34, 2);
  r(g, '#77746b', x - 17, y - 8, 34, 7);
  r(g, '#8d8a80', x - 17, y - 8, 34, 1);
  r(g, '#3f8fc4', x - 14, y - 27, 28, 17);
  r(g, '#7fd4ff', x - 12 + ripple, y - 24, 6, 1);
  r(g, '#7fd4ff', x + 4 - ripple, y - 14, 7, 1);
  r(g, '#c9c4b5', x - 3, y - 30, 6, 17);
  r(g, '#8d8a80', x - 3, y - 15, 6, 2);
  r(g, '#c9c4b5', x - 8, y - 33, 16, 3);
  r(g, '#3f8fc4', x - 6, y - 33, 12, 1);
  r(g, '#bfe6f5', x - 1, y - 41, 2, 8);
  r(g, '#bfe6f5', x - 6, y - 37 + drop, 1, 2);
  r(g, '#bfe6f5', x + 5, y - 37 + (drop + 1) % 3, 1, 2);
}

function paintPiano(g, x, y, t) {
  const left = x - 29;
  r(g, SHADOW, left, y - 2, 58, 2);
  r(g, '#1b1820', left + 1, y - 37, 56, 28);
  r(g, '#3a3342', left, y - 38, 58, 3);
  r(g, '#2b2530', left + 3, y - 33, 52, 13);
  r(g, '#f4f1e8', x - 7, y - 32, 14, 9);
  r(g, '#9aa3b8', x - 5, y - 30, 10, 1);
  r(g, '#9aa3b8', x - 5, y - 28, 10, 1);
  r(g, '#9aa3b8', x - 5, y - 26, 7, 1);
  r(g, '#f4f1e8', left + 1, y - 18, 56, 6);
  for (let key = 0; key < 56; key += 3) {
    r(g, '#b9b3a3', left + 1 + key, y - 18, 1, 6);
    if (key % 21 !== 6 && key % 21 !== 18) r(g, '#1b1820', left + 2 + key, y - 18, 2, 3);
  }
  r(g, '#1b1820', left + 1, y - 12, 56, 3);
  r(g, '#1b1820', left + 3, y - 9, 3, 8);
  r(g, '#1b1820', left + 52, y - 9, 3, 8);
  const rise = Math.floor(t / 240) % 9;
  if (t) r(g, '#ffd24d', x + 12, y - 42 - rise, 2, 2);
}

function paintFireplace(g, x, y, t) {
  const left = x - 35;
  r(g, SHADOW, left, y - 2, 70, 2);
  r(g, '#8a4a3a', left + 2, y - 31, 66, 30);
  for (let row = 0; row < 7; row++) {
    r(g, '#6f3a2d', left + 2, y - 28 + row * 4, 66, 1);
    for (let col = (row % 2) * 5; col < 66; col += 10) r(g, '#6f3a2d', left + 2 + col, y - 31 + row * 4, 1, 3);
  }
  r(g, '#6b4628', left, y - 35, 70, 4);
  r(g, '#a0703f', left, y - 35, 70, 1);
  r(g, '#141014', x - 17, y - 26, 34, 25);
  r(g, '#c9a86a', x - 18, y - 27, 36, 1);
  r(g, '#4a3120', x - 12, y - 5, 24, 3);
  r(g, '#2c1d12', x - 9, y - 3, 18, 2);
  const flick = Math.floor(t / 140);
  for (let i = 0; i < 6; i++) {
    const tall = 8 + ((flick + i * 3) % 5) * 2;
    const flame = x - 13 + i * 5;
    r(g, '#ff8a3d', flame, y - 5 - tall, 4, tall);
    r(g, '#ffde59', flame + 1, y - 5 - tall + 4, 2, tall - 4);
  }
  r(g, 'rgba(255,160,70,.13)', x - 30, y - 1, 60, 3);
  r(g, '#c9a86a', x - 26, y - 41, 4, 6);
  r(g, '#ffb454', x + 21, y - 39, 6, 4);
}

function paintAquarium(g, x, y, t) {
  const left = x - 35;
  r(g, SHADOW, left, y - 2, 70, 2);
  r(g, '#4a3120', left + 1, y - 13, 68, 12);
  r(g, '#6b4628', left + 1, y - 13, 68, 2);
  r(g, '#20283e', left, y - 42, 70, 29);
  r(g, '#2a86b8', left + 1, y - 41, 68, 27);
  r(g, '#55b4de', left + 1, y - 41, 68, 3);
  r(g, '#d8c7a3', left + 1, y - 17, 68, 3);
  for (const [dx, tall] of [[6, 10], [10, 15], [13, 8], [50, 13], [54, 9], [61, 14]]) r(g, '#2f8f5b', left + dx, y - 17 - tall, 2, tall);
  for (const [i, color] of ['#ffb454', '#ff5d7a', '#ffd24d', '#f4f1e8'].entries()) {
    const swim = (t / (70 + i * 34) + i * 17) % 58;
    const fish = left + 6 + Math.floor(swim);
    const level = y - 36 + i * 5 + (Math.floor(swim / 9) % 2);
    r(g, color, fish, level, 5, 3);
    r(g, color, fish - 2, level - 1, 2, 5);
    r(g, '#1b2030', fish + 3, level, 1, 1);
  }
  const bubble = Math.floor(t / 230) % 20;
  r(g, '#bfe6f5', left + 34, y - 18 - bubble, 1, 1);
  r(g, '#bfe6f5', left + 44, y - 18 - (bubble + 9) % 20, 2, 2);
}

function paintJukebox(g, x, y, t) {
  const beat = Math.floor(t / 320);
  r(g, SHADOW, x - 11, y - 2, 22, 2);
  r(g, '#7a2b3a', x - 10, y - 27, 20, 26);
  r(g, '#7a2b3a', x - 8, y - 31, 16, 4);
  r(g, '#a23a4e', x - 5, y - 33, 10, 2);
  r(g, '#ffd24d', x - 10, y - 27, 1, 26);
  r(g, '#ffd24d', x + 9, y - 27, 1, 26);
  r(g, '#ffd24d', x - 8, y - 31, 16, 1);
  r(g, '#0d1020', x - 6, y - 29, 12, 9);
  r(g, '#f4f1e8', x - 4, y - 26, 8, 1);
  r(g, '#f4f1e8', x - 4, y - 23, 8, 1);
  for (let i = 0; i < 4; i++) r(g, BOOKS[(i + beat) % BOOKS.length], x - 7 + i * 4, y - 18, 3, 5);
  r(g, '#20283e', x - 7, y - 11, 14, 7);
  for (let i = 0; i < 3; i++) r(g, '#4a4f66', x - 6, y - 10 + i * 2, 12, 1);
}

function paintRoseArch(g, x, y, t) {
  const left = x - 34;
  const right = x + 30;
  r(g, SHADOW, left, y - 2, 68, 2);
  r(g, '#8a6b4a', left, y - 44, 4, 43);
  r(g, '#8a6b4a', right, y - 44, 4, 43);
  r(g, '#8a6b4a', left + 3, y - 49, 62, 5);
  r(g, '#8a6b4a', left + 11, y - 54, 46, 5);
  r(g, '#a0845f', left + 11, y - 54, 46, 1);
  for (let i = 0; i < 9; i++) {
    r(g, '#2f8f5b', left - 1, y - 8 - i * 5, 6, 3);
    r(g, '#2f8f5b', right - 1, y - 6 - i * 5, 6, 3);
    r(g, '#3fb573', left + 8 + i * 7, y - 53 + (i % 2) * 7, 6, 3);
  }
  const sway = Math.floor(t / 1300) % 2;
  for (const post of [left, right]) {
    for (const up of [12, 26, 38]) {
      r(g, '#ff5d7a', post + sway, y - up, 4, 4);
      r(g, '#c9344e', post + sway + 1, y - up + 1, 2, 2);
    }
  }
  for (let i = 0; i < 8; i++) {
    const rose = left + 9 + i * 7;
    r(g, i % 2 ? '#ff8fa8' : '#ff5d7a', rose, y - 51 + (i % 2) * 5, 4, 4);
    r(g, '#c9344e', rose + 1, y - 50 + (i % 2) * 5, 2, 2);
  }
}

function paintRubberDuck(g, x, y, t) {
  const nod = Math.floor(t / 1300) % 4 === 3 ? 1 : 0;
  r(g, 'rgba(0,0,0,.2)', x - 8, y - 1, 16, 2);
  r(g, '#ffd24d', x - 7, y - 8, 14, 7);
  r(g, '#ffd24d', x - 6, y - 9, 11, 9);
  r(g, '#ffd24d', x - 9, y - 10, 3, 4);
  r(g, '#e8b52e', x - 6, y - 2, 12, 2);
  r(g, '#e8b52e', x - 4, y - 6, 6, 2);
  r(g, '#ffd24d', x - 1, y - 15 + nod, 8, 7);
  r(g, '#ffde59', x, y - 15 + nod, 5, 1);
  r(g, '#ff8a3d', x + 7, y - 12 + nod, 4, 2);
  r(g, '#1b2030', x + 3, y - 13 + nod, 2, 2);
}

function paintTrophy(g, x, y, t) {
  r(g, SHADOW, x - 7, y - 2, 14, 2);
  r(g, '#4a3120', x - 6, y - 7, 12, 6);
  r(g, '#6b4628', x - 6, y - 7, 12, 1);
  r(g, '#c9a86a', x - 2, y - 5, 4, 2);
  r(g, '#d9a520', x - 3, y - 9, 6, 2);
  r(g, '#d9a520', x - 1, y - 13, 2, 4);
  r(g, '#ffd24d', x - 5, y - 24, 10, 8);
  r(g, '#ffd24d', x - 4, y - 16, 8, 2);
  r(g, '#ffd24d', x - 2, y - 14, 4, 1);
  r(g, '#d9a520', x - 5, y - 24, 10, 1);
  r(g, '#d9a520', x + 3, y - 23, 2, 7);
  r(g, '#d9a520', x - 8, y - 23, 3, 1);
  r(g, '#d9a520', x - 8, y - 23, 1, 4);
  r(g, '#d9a520', x - 8, y - 20, 3, 1);
  r(g, '#d9a520', x + 5, y - 23, 3, 1);
  r(g, '#d9a520', x + 7, y - 23, 1, 4);
  r(g, '#d9a520', x + 5, y - 20, 3, 1);
  r(g, '#fff4c2', x - 3 + Math.floor(t / 700) % 6, y - 22, 1, 4);
}

// The word on the quiet sign, in dots twice the size of the room's own, because a phone draws the
// room smaller than its pixels and a thinner letter would drop out. A sign stops bots, so it has to
// be read by anyone who can see the room.
const QUIET_WORD = [
  '### # # ### ### ###',
  '# # # #  #  #    # ',
  '# # # #  #  ##   # ',
  '### # #  #  #    # ',
  '  # ### ### ###  # ',
].flatMap((line, row) => [...line].flatMap((dot, col) => (dot === '#' ? [[col * 2, row * 2]] : [])));
const SIGN_WIDE = 46;

// Its board is wider than the cell it stands on, so at the edge of the room the board stays in view.
function paintQuietSign(g, x, y) {
  const board = Math.min(W - SIGN_WIDE / 2, Math.max(SIGN_WIDE / 2, x));
  r(g, SHADOW, x - 6, y - 2, 12, 2);
  r(g, '#20283e', x - 5, y - 3, 10, 2);
  r(g, '#3b4158', x - 1, y - 14, 3, 12);
  r(g, '#20283e', board - 23, y - 32, SIGN_WIDE, 19);
  r(g, '#fff', board - 22, y - 31, SIGN_WIDE - 2, 17);
  r(g, '#c8342c', board - 21, y - 30, SIGN_WIDE - 4, 15);
  for (const [dx, dy] of QUIET_WORD) r(g, '#fff', board - 19 + dx, y - 28 + dy, 2, 2);
}

// The Halloween pack. A flame holds still and a ghost stops bobbing for someone who asked for less
// motion, the same as everything else here.

const FLAME = ['#ffde59', '#ffb454', '#fff0b8'];

function paintPumpkin(g, x, y, t) {
  const flicker = Math.floor(t / 170) % 3;
  const lit = FLAME[flicker];
  const glow = `rgba(255,150,50,${flicker === 1 ? 0.07 : 0.11})`;
  r(g, glow, x - 12, y - 11, 24, 8);
  r(g, glow, x - 9, y - 15, 18, 15);
  r(g, SHADOW, x - 9, y - 1, 18, 2);
  r(g, '#b4501a', x - 9, y - 11, 18, 9);
  r(g, '#b4501a', x - 7, y - 13, 14, 13);
  r(g, '#e8802a', x - 8, y - 11, 16, 9);
  r(g, '#e8802a', x - 6, y - 13, 12, 12);
  r(g, '#d0691f', x - 4, y - 13, 1, 12);
  r(g, '#d0691f', x + 3, y - 13, 1, 12);
  r(g, '#ffae5c', x - 7, y - 11, 1, 4);
  r(g, '#4f7a35', x - 1, y - 16, 3, 3);
  r(g, '#3a5c28', x + 1, y - 17, 2, 2);
  r(g, '#5a2a10', x - 6, y - 10, 4, 3);
  r(g, '#5a2a10', x + 2, y - 10, 4, 3);
  r(g, '#5a2a10', x - 6, y - 6, 12, 4);
  r(g, lit, x - 5, y - 9, 3, 2);
  r(g, lit, x - 4, y - 10, 1, 1);
  r(g, lit, x + 2, y - 9, 3, 2);
  r(g, lit, x + 3, y - 10, 1, 1);
  r(g, lit, x - 5, y - 5, 10, 2);
  r(g, '#5a2a10', x - 3, y - 5, 2, 1);
  r(g, '#5a2a10', x + 1, y - 4, 2, 1);
}

function paintGhost(g, x, y, t) {
  const bob = Math.round(Math.sin(t / 640) * 2);
  const sway = Math.floor(t / 320) % 2;
  const sheet = '#ece9f6';
  const fold = '#c4c1da';
  const top = y - 31 + bob;
  r(g, SHADOW, x - 6, y - 1, 12, 2);
  r(g, sheet, x - 4, top, 8, 1);
  r(g, sheet, x - 6, top + 1, 12, 2);
  r(g, sheet, x - 7, top + 3, 14, 18);
  r(g, fold, x + 5, top + 5, 2, 16);
  r(g, sheet, x - 9, top + 10 - sway, 2, 5);
  r(g, sheet, x + 7, top + 10 - sway, 2, 5);
  for (let i = 0; i < 3; i++) {
    const tail = 2 + ((i + sway) % 2) * 2;
    r(g, i === 2 ? fold : sheet, x - 7 + i * 5, top + 21, 4, tail);
  }
  r(g, '#1b2030', x - 4, top + 7, 2, 3);
  r(g, '#1b2030', x + 2, top + 7, 2, 3);
  r(g, '#1b2030', x - 1, top + 12, 2, 3);
  r(g, '#f4b6c8', x - 6, top + 11, 2, 1);
  r(g, '#f4b6c8', x + 4, top + 11, 2, 1);
}

function paintCauldron(g, x, y, t) {
  const flicker = Math.floor(t / 150) % 3;
  const glow = `rgba(120,240,140,${flicker === 1 ? 0.06 : 0.09})`;
  r(g, glow, x - 11, y - 27, 22, 8);
  r(g, glow, x - 7, y - 32, 14, 13);
  r(g, SHADOW, x - 11, y - 1, 22, 2);
  r(g, '#14121c', x - 8, y - 5, 2, 5);
  r(g, '#14121c', x + 6, y - 5, 2, 5);
  r(g, '#ff8a3d', x - 5, y - 3, 10, 3);
  r(g, FLAME[flicker], x - 3 + flicker, y - 3, 3, 2);
  r(g, '#2a2733', x - 10, y - 16, 20, 11);
  r(g, '#2a2733', x - 8, y - 6, 16, 2);
  r(g, '#191722', x + 6, y - 16, 4, 11);
  r(g, '#454252', x - 8, y - 15, 2, 7);
  r(g, '#3f3c4c', x - 11, y - 18, 22, 3);
  r(g, '#5a5768', x - 11, y - 18, 22, 1);
  r(g, '#5fe07a', x - 9, y - 19, 18, 2);
  r(g, '#a6f5a0', x - 6 + flicker * 3, y - 19, 3, 1);
  for (let i = 0; i < 3; i++) {
    const rise = (t / 45 + i * 9) % 13;
    r(g, '#8df09a', x - 5 + i * 4 + (Math.floor(rise / 4) % 2), y - 20 - rise, 2, 2);
  }
}

// R, I and P, three pixels wide and five tall.
const RIP = [[0, 7, 5, 6, 5, 5], [4, 7, 2, 2, 2, 7], [8, 7, 5, 7, 4, 4]];

function paintGravestone(g, x, y) {
  r(g, SHADOW, x - 9, y - 2, 18, 2);
  r(g, '#5a4634', x - 9, y - 3, 18, 3);
  r(g, '#8b8d99', x - 7, y - 20, 14, 18);
  r(g, '#8b8d99', x - 5, y - 22, 10, 2);
  r(g, '#a9abb8', x - 7, y - 20, 2, 17);
  r(g, '#a9abb8', x - 5, y - 22, 10, 1);
  r(g, '#6b6d79', x + 5, y - 20, 2, 17);
  for (const [left, ...rows] of RIP) {
    rows.forEach((bits, row) => {
      for (let bit = 0; bit < 3; bit++) {
        if (bits & (4 >> bit)) r(g, '#4c4e5a', x - 6 + left + bit, y - 18 + row, 1, 1);
      }
    });
  }
  r(g, '#4f7a35', x - 9, y - 5, 2, 3);
  r(g, '#3f6a30', x - 4, y - 4, 2, 2);
  r(g, '#4f7a35', x + 3, y - 4, 2, 2);
  r(g, '#3f6a30', x + 7, y - 5, 2, 3);
}

function paintSkeleton(g, x, y, t) {
  const bone = '#ece7d8';
  const shadow = '#b9b4a2';
  const hollow = '#1b2030';
  const wave = Math.floor(t / 420) % 2;
  r(g, SHADOW, x - 7, y - 1, 14, 2);
  r(g, bone, x - 3, y - 11, 2, 10);
  r(g, bone, x + 1, y - 11, 2, 10);
  r(g, bone, x - 4, y - 2, 3, 2);
  r(g, bone, x + 1, y - 2, 3, 2);
  r(g, shadow, x - 4, y - 13, 8, 2);
  r(g, shadow, x - 1, y - 22, 2, 9);
  for (let i = 0; i < 3; i++) r(g, bone, x - 4, y - 22 + i * 3, 8, 2);
  r(g, bone, x - 6, y - 22, 2, 1);
  r(g, bone, x - 6, y - 22, 1, 10);
  r(g, bone, x + 4, y - 22, 3, 1);
  r(g, bone, x + 6, y - 28 + wave, 1, 7 - wave);
  r(g, bone, x + 5 + wave * 2, y - 30 + wave, 2, 2);
  r(g, bone, x - 4, y - 31, 8, 6);
  r(g, bone, x - 3, y - 32, 6, 1);
  r(g, shadow, x - 3, y - 25, 6, 2);
  r(g, hollow, x - 3, y - 29, 2, 2);
  r(g, hollow, x + 1, y - 29, 2, 2);
  r(g, hollow, x, y - 27, 1, 1);
  r(g, hollow, x - 2, y - 24, 1, 1);
  r(g, hollow, x, y - 24, 1, 1);
  r(g, hollow, x + 2, y - 24, 1, 1);
}

// These are listed words on the server. A word this page doesn't know is left out, so a newer
// server can't break an older page. `rise` is how far up from its front edge it's painted, and a
// `sign` changes what bots can do, so it's painted over everything else.
const ITEMS = {
  plant: { paint: paintPlant, rise: 26 },
  lamp: { paint: paintLamp, rise: 39 },
  rubber_duck: { paint: paintRubberDuck, rise: 15 },
  stool: { paint: paintStool, rise: 11 },
  coffee_table: { paint: paintCoffeeTable, rise: 25 },
  bookshelf: {
    paint: paintBookshelf,
    side: endOf(30, '#8a5c37', '#4a3120', (g, x, top, deep) => {
      for (let i = 0; i * 4 < deep - 3; i++) r(g, BOOKS[i % BOOKS.length], x + 2, top + 2 + i * 4, 2, 3);
    }),
    rise: 30,
  },
  cabinet: {
    paint: paintCabinet,
    side: endOf(26, '#8a5c37', '#6b4628', (g, x, top, deep) => {
      r(g, '#a0703f', x + 3, top, 1, deep);
      r(g, '#ffd24d', x + 3, top + deep + 13, 2, 2);
    }),
    rise: 26,
  },
  whiteboard: { paint: paintWhiteboard, side: edgeOf(30, '#f4f1e8', '#20283e'), rise: 32 },
  quiet: { paint: paintQuietSign, rise: 32, sign: true },
  monitor: { paint: paintMonitor, side: edgeOf(22, '#3b4158', '#20283e'), rise: 25 },
  armchair: {
    paint: paintArmchair,
    side: (g, x, y, t, wide, deep) => paintSeatEnd(g, x, y, wide, deep, COUCH_CLOTH),
    rise: 25,
  },
  easel: { paint: paintEasel, side: edgeOf(30, '#f4f1e8', '#6b4628'), rise: 37 },
  game_table: { paint: paintGameTable, rise: 34 },
  standing_desk: { paint: paintStandingDesk, rise: 38 },
  sofa: {
    paint: paintSofa,
    side: (g, x, y, t, wide, deep) => paintSeatEnd(g, x, y, wide, deep, SOFA_CLOTH),
    rise: 25,
  },
  fountain: { paint: paintFountain, rise: 41 },
  piano: {
    paint: paintPiano,
    side: endOf(34, '#3a3342', '#1b1820', (g, x, top, deep) => {
      r(g, '#f4f1e8', x + 5, top + 4, 5, deep - 8);
      for (let key = 0; key < deep - 8; key += 3) r(g, '#b9b3a3', x + 5, top + 4 + key, 5, 1);
    }),
    rise: 38,
  },
  candlelit_table: { paint: paintCandlelitTable, rise: 34 },
  server_rack: {
    paint: paintServerRack,
    side: endOf(30, '#3b4158', '#1b2030', (g, x, top, deep) => {
      for (let i = 0; i < 3; i++) r(g, '#0d1020', x - 3, top + 5 + i * 5, 6, 2);
    }),
    rise: 30,
  },
  fireplace: {
    paint: paintFireplace,
    side: endOf(34, '#6b4628', '#8a4a3a', (g, x, top, deep, t) => {
      r(g, '#a0703f', x - 5, top, 10, 1);
      for (let row = 0; row < 7; row++) r(g, '#6f3a2d', x - 5, top + deep + 3 + row * 4, 10, 1);
      r(g, Math.floor(t / 140) % 2 ? '#ff8a3d' : '#ffb454', x + 4, top + 8, 1, deep - 16);
    }),
    rise: 41,
  },
  aquarium: {
    paint: paintAquarium,
    side: endOf(40, '#55b4de', '#2a86b8', (g, x, top, deep) => {
      r(g, '#20283e', x - 11, top, 22, 1);
      r(g, '#4a3120', x - 11, top + deep + 27, 22, 12);
      r(g, '#6b4628', x - 11, top + deep + 27, 22, 2);
      r(g, '#d8c7a3', x - 11, top + deep + 23, 22, 3);
    }),
    rise: 42,
  },
  trophy: { paint: paintTrophy, rise: 24 },
  jukebox: {
    paint: paintJukebox,
    side: endOf(30, '#a23a4e', '#7a2b3a', (g, x, top, deep) => {
      r(g, '#ffd24d', x - 5, top + deep, 1, 29);
      r(g, '#ffd24d', x + 4, top + deep, 1, 29);
    }),
    rise: 33,
  },
  love_seat: {
    paint: paintLoveSeat,
    side: (g, x, y, t, wide, deep) => paintSeatEnd(g, x, y, wide, deep, LOVE_CLOTH),
    rise: 25,
  },
  rose_arch: {
    paint: paintRoseArch,
    side: (g, x, y, t, wide, deep) => {
      paintEnd(g, x, y, 4, deep - 2, 48, '#8a6b4a', '#8a6b4a');
      r(g, '#8a6b4a', x - 5, y - 2, 10, 2);
      r(g, '#8a6b4a', x - 5, y - deep + 1, 10, 2);
      for (let i = 0; i * 9 < deep - 6; i++) {
        r(g, '#2f8f5b', x - 4, y - 54 - i * 9, 8, 5);
        r(g, i % 2 ? '#ff8fa8' : '#ff5d7a', x - 2, y - 53 - i * 9, 4, 4);
      }
    },
    rise: 54,
  },
  pumpkin: { paint: paintPumpkin, rise: 17 },
  ghost: { paint: paintGhost, rise: 33 },
  cauldron: { paint: paintCauldron, rise: 33 },
  gravestone: { paint: paintGravestone, rise: 22, words: true },
  skeleton: { paint: paintSkeleton, rise: 32 },
};

// Where an item stands, in the room's own pixels. `w` and `h` are cells as it's turned now, and
// `x` and `y` are its back left corner, as the server has it.
function standing({ id, item, x, y, w = 1, h = 1, facing = 0 }) {
  const wide = w * FLOOR_CELL;
  const deep = h * FLOOR_CELL;
  const left = x * FLOOR_CELL;
  const top = FLOOR_TOP + y * FLOOR_CELL;
  return { ...ITEMS[item], id, facing, left, top, wide, deep, x: left + wide / 2, y: top + deep };
}

// Facing 2 and 3 are the other way round. Words stay readable, so a sign or a kind with `words` never is.
function paintItem(g, { paint, side, sign, words, x, y, wide, deep, facing }, t) {
  const flip = facing >= 2 && !sign && !words;
  if (flip) {
    g.save();
    g.translate(x * 2, 0);
    g.scale(-1, 1);
  }
  (facing % 2 && side ? side : paint)(g, x, y, t, wide, deep);
  if (flip) g.restore();
}

// A picture of a kind for the catalog, on a canvas of its own, drawn still. Null for a word this page
// doesn't know.
export function itemPicture(item, w, h) {
  if (!Object.hasOwn(ITEMS, item)) return null;
  const { paint, rise, sign } = ITEMS[item];
  const wide = w * FLOOR_CELL;
  const deep = h * FLOOR_CELL;
  const canvas = document.createElement('canvas');
  canvas.width = (sign ? SIGN_WIDE : Math.max(wide, 24)) + 2;
  canvas.height = Math.max(rise, deep) + 2;
  paint(canvas.getContext('2d'), canvas.width / 2, canvas.height - 1, 0, wide, deep);
  return canvas;
}

function paintTable(g, lit) {
  r(g, '#00000030', 176, 168, 132, 4);
  r(g, '#6b4628', 180, 160, 6, 10);
  r(g, '#6b4628', 294, 160, 6, 10);
  r(g, '#8a5c37', 176, 124, 128, 36);
  r(g, '#a0703f', 176, 124, 128, 2);
  r(g, '#6f4728', 176, 158, 128, 3);
  for (let y = 134; y < 156; y += 10) r(g, '#7d5231', 178, y, 124, 1);
  // Laptops light up where someone's sitting
  SEATS.slice(0, 8).forEach((seat, i) => {
    const top = i < 4;
    const y = top ? 134 : 148;
    r(g, '#2b3045', seat.x - 6, y, 12, 7);
    if (!top) r(g, lit[i] ? '#7fd4ff' : '#3b4158', seat.x - 5, y + 1, 10, 5);
    else r(g, lit[i] ? '#55607f' : '#3b4158', seat.x - 5, y + 1, 10, 5);
  });
  r(g, '#f4f1e8', 236, 138, 8, 8);
  r(g, '#ffb454', 238, 140, 4, 4);
}

function drawZ(g, x, y, alpha) {
  g.globalAlpha = alpha;
  r(g, '#fff', x, y, 3, 1);
  r(g, '#fff', x + 1, y + 1, 1, 1);
  r(g, '#fff', x, y + 2, 3, 1);
  g.globalAlpha = 1;
}

// What the words in a look are drawn as (docs/looks.md). They're listed words on the server, so nothing
// typed reaches the drawing, and a word this page doesn't know counts as one nobody chose.
export const FABRICS = {
  red: '#d6453f',
  orange: '#ec8a2c',
  yellow: '#e8c22e',
  green: '#3fa35a',
  teal: '#25a4a0',
  blue: '#3a78d4',
  indigo: '#4a52b8',
  purple: '#8e56c9',
  pink: '#e96fa4',
  white: '#e8ecf4',
  grey: '#8a909e',
  black: '#2f3342',
};
const SKINS = {
  cream: '#f4d6b8',
  peach: '#f1c9a4',
  tan: '#d9a77c',
  caramel: '#b27a54',
  umber: '#8a5a3c',
  espresso: '#5e3a26',
};
const HAIRS = {
  black: '#2b2118',
  brown: '#6a4326',
  honey: '#b9822f',
  blond: '#d8c08a',
  chestnut: '#4a2a22',
  grey: '#8a8f9c',
  red: '#b5472a',
  pink: '#e96fa4',
  teal: '#25a4a0',
  violet: '#7b54d6',
};
const LEGS = {
  navy: '#394666',
  charcoal: '#4a4a52',
  khaki: '#6a5a46',
  pine: '#2f4a48',
  denim: '#4f6f9c',
  stone: '#b8ae98',
};
export const EYES = {
  cyan: '#8be0ff',
  amber: '#ffb84d',
  pink: '#ff8ac0',
  green: '#8cf08a',
  white: '#ffffff',
};
// The parts that are only a colour, so the wardrobe can show a swatch.
export const LOOK_COLOURS = { skin: SKINS, hair: HAIRS, shirt: FABRICS, trousers: LEGS, shell: FABRICS, eyes: EYES };

// Hats fit a person or a bot: `half` is half the width of the top of the head, and `top` is its top
// row. A cap and a beanie come in whatever the figure is wearing.
const HATS = {
  none: () => {},
  cap: (g, x, top, half, color, dark) => {
    r(g, color, x - half, top - 1, half * 2, 2);
    r(g, color, x - half + 1, top - 2, half * 2 - 2, 1);
    r(g, dark, x - half - 1, top + 1, half * 2 + 2, 1);
  },
  beanie: (g, x, top, half, color, dark) => {
    r(g, '#f4f1e8', x - 1, top - 4, 2, 1);
    r(g, color, x - half + 1, top - 3, half * 2 - 2, 1);
    r(g, color, x - half, top - 2, half * 2, 2);
    r(g, dark, x - half, top, half * 2, 2);
  },
  top_hat: (g, x, top, half) => {
    r(g, '#22222e', x - half, top - 5, half * 2, 5);
    r(g, '#b23a48', x - half, top - 2, half * 2, 1);
    r(g, '#22222e', x - half - 1, top, half * 2 + 2, 1);
  },
  witch: (g, x, top, half) => {
    for (let i = 0; i < half; i++) r(g, '#2b2140', x - half + i, top - 2 - i * 2, (half - i) * 2, 2);
    r(g, '#2b2140', x, top - 1 - half * 2, 2, 1);
    r(g, '#e8a02e', x - half, top - 1, half * 2, 1);
    r(g, '#2b2140', x - half - 2, top, half * 2 + 4, 1);
  },
  crown: (g, x, top, half) => {
    r(g, '#f0c23a', x - half, top - 1, half * 2, 2);
    r(g, '#f0c23a', x - half, top - 2, 1, 1);
    r(g, '#f0c23a', x - 1, top - 2, 2, 1);
    r(g, '#f0c23a', x + half - 1, top - 2, 1, 1);
    r(g, '#d6453f', x - 1, top, 2, 1);
  },
  party: (g, x, top, half) => {
    for (let i = 0; i < half; i++) r(g, i % 2 ? '#ffd24d' : '#e96fa4', x - half + i, top - 2 - i * 2, (half - i) * 2, 2);
    r(g, '#f4f1e8', x - 1, top - 2 - half * 2, 2, 1);
  },
};

// A person's hair. `crown` is the top of it, which a hat covers, `front` is what hangs beside the face,
// `side` is what shows behind the face from the side, starting at `behind`, and `back` is the rest.
const SHORT_CROWN = (g, x, top, hair) => {
  r(g, hair, x - 2, top - 1, 4, 1);
  r(g, hair, x - 3, top, 6, 2);
};
const STYLES = {
  short: {
    crown: SHORT_CROWN,
    front: () => {},
    side: (g, behind, top, hair) => r(g, hair, behind, top + 2, 2, 2),
    back: (g, x, top, hair) => {
      r(g, hair, x - 3, top + 2, 6, 3);
      r(g, hair, x - 2, top + 5, 4, 1);
    },
  },
  long: {
    crown: SHORT_CROWN,
    front: (g, x, top, hair) => {
      r(g, hair, x - 4, top + 1, 1, 7);
      r(g, hair, x + 3, top + 1, 1, 7);
    },
    side: (g, behind, top, hair) => r(g, hair, behind, top + 2, 2, 6),
    back: (g, x, top, hair) => {
      r(g, hair, x - 4, top + 1, 8, 7);
      r(g, hair, x - 3, top + 8, 6, 1);
    },
  },
  curly: {
    crown: (g, x, top, hair) => {
      r(g, hair, x - 3, top - 2, 6, 1);
      r(g, hair, x - 4, top - 1, 8, 3);
    },
    front: (g, x, top, hair) => {
      r(g, hair, x - 4, top + 2, 1, 2);
      r(g, hair, x + 3, top + 2, 1, 2);
    },
    side: (g, behind, top, hair) => r(g, hair, behind, top + 2, 2, 3),
    back: (g, x, top, hair) => {
      r(g, hair, x - 4, top + 2, 8, 3);
      r(g, hair, x - 3, top + 5, 6, 1);
    },
  },
  bun: {
    crown: (g, x, top, hair) => {
      SHORT_CROWN(g, x, top, hair);
      r(g, hair, x - 1, top - 3, 2, 2);
    },
    front: () => {},
    side: (g, behind, top, hair) => r(g, hair, behind, top + 2, 2, 1),
    back: (g, x, top, hair) => r(g, hair, x - 3, top + 2, 6, 2),
  },
  mohawk: {
    crown: (g, x, top, hair) => r(g, hair, x - 1, top - 3, 2, 4),
    front: () => {},
    side: () => {},
    back: (g, x, top, hair) => r(g, hair, x - 1, top + 1, 2, 4),
  },
  bald: {
    crown: () => {},
    front: () => {},
    side: () => {},
    back: () => {},
  },
};

// What's on a person's face. `ahead` is -1 or 1, the way they face from the side.
const FACES = {
  none: {
    front: () => {},
    side: () => {},
  },
  glasses: {
    front: (g, x, top) => {
      r(g, '#20242f', x - 3, top + 2, 6, 1);
      r(g, '#20242f', x - 3, top + 3, 1, 1);
      r(g, '#20242f', x + 2, top + 3, 1, 1);
    },
    side: (g, x, top, ahead) => r(g, '#20242f', ahead < 0 ? x - 3 : x, top + 2, 3, 1),
  },
  shades: {
    front: (g, x, top) => {
      r(g, '#15161c', x - 3, top + 2, 6, 2);
      r(g, '#6d7890', x - 2, top + 2, 1, 1);
      r(g, '#6d7890', x + 1, top + 2, 1, 1);
    },
    side: (g, x, top, ahead) => r(g, '#15161c', ahead < 0 ? x - 3 : x, top + 2, 3, 2),
  },
  moustache: {
    front: (g, x, top, ahead, hair) => r(g, hair, x - 2, top + 4, 4, 1),
    side: (g, x, top, ahead, hair) => r(g, hair, ahead < 0 ? x - 3 : x + 1, top + 4, 2, 1),
  },
  beard: {
    front: (g, x, top, ahead, hair) => {
      r(g, hair, x - 3, top + 4, 6, 1);
      r(g, hair, x - 2, top + 5, 4, 1);
      r(g, hair, x - 1, top + 6, 2, 1);
    },
    side: (g, x, top, ahead, hair) => r(g, hair, ahead < 0 ? x - 3 : x, top + 4, 3, 2),
  },
};

// A bot's head: how wide it is, and whether it has an antenna each side or one in the middle.
const HEADS = {
  antenna: { wide: 10, twin: false },
  twin: { wide: 10, twin: true },
  wide: { wide: 12, twin: false },
};

// A picture on a bot's chest, on a dark plate so it can be seen on any shell. Lights are the bare chest.
const plate = (g, x, y, color, rows) => {
  r(g, '#1b2030', x - 3, y, 6, rows.length);
  rows.forEach((row, i) => [...row].forEach((cell, j) => {
    if (cell === '#') r(g, color, x - 2 + j, y + i, 1, 1);
  }));
};
const CHESTS = {
  plain: () => {},
  heart: (g, x, y) => plate(g, x, y, '#ff5d7a', ['#..#', '####', '.##.']),
  star: (g, x, y) => plate(g, x, y, '#ffd24d', ['.##.', '####', '.##.']),
  bolt: (g, x, y) => plate(g, x, y, '#8be0ff', ['..##', '.##.', '##..']),
};

const pick = (table, word, otherwise) => (Object.hasOwn(table, word) ? table[word] : otherwise);

function botLook(name, chosen = {}) {
  return {
    shell: pick(FABRICS, chosen.shell, null),
    head: pick(HEADS, chosen.head, Object.values(HEADS)[hash(name) % 3]),
    eyes: pick(EYES, chosen.eyes, null),
    chest: pick(CHESTS, chosen.chest, CHESTS.plain),
    hat: pick(HATS, chosen.hat, HATS.none),
  };
}

function drawBot(g, bot, t) {
  const x = Math.round(bot.x);
  const y = Math.round(bot.y);
  const { shell: coat, head, eyes, chest, hat } = bot.wears;
  const color = coat || bot.color;
  const dark = shade(color, -0.32);
  const glow = eyes || shade(color, 0.45);
  const shell = '#e8ecf4';
  const visor = '#1b2030';

  if (bot.pose === 'sleep') {
    r(g, 'rgba(0,0,0,.2)', x - 13, y + 2, 26, 2);
    r(g, color, x - 3, y - 4, 13, 6);
    r(g, dark, x - 3, y + 1, 13, 1);
    r(g, '#2a2f44', x + 10, y - 3, 3, 4);
    r(g, shell, x - 12, y - 6, 9, 8);
    r(g, visor, x - 10, y - 4, 2, 4);
    // Lying down, so the hat is on its side: up, for a hat, is to the left here.
    g.save();
    g.translate(x - 12, y - 2);
    g.rotate(-Math.PI / 2);
    hat(g, 0, 0, 4, color, dark);
    g.restore();
    for (let i = 0; i < 3; i++) {
      const rise = (t / 70 + i * 13 + bot.phase * 40) % 36;
      drawZ(g, x - 10 + i * 3 + rise / 6, y - 10 - rise, 1 - rise / 36);
    }
    return;
  }

  const walking = bot.path.length > 0;
  const sitting = bot.pose === 'sit' && !walking;
  const step = walking ? Math.floor(t / 130 + bot.phase * 9) % 2 : 0;
  const breathe = !walking && Math.sin(t / 800 + bot.phase * 7) > 0.35 ? 1 : 0;
  const base = sitting ? y - 1 : y - 3;

  r(g, 'rgba(0,0,0,.25)', x - 6, y - 1, 12, 2);
  if (!sitting) {
    r(g, '#2a2f44', x - 3, y - 3 - step, 2, 3 + step);
    r(g, '#2a2f44', x + 1, y - 3 - (step ? 0 : walking ? 1 : 0), 2, 3);
  }

  r(g, color, x - 4, base - 6, 8, 6);
  r(g, dark, x - 4, base - 1, 8, 1);
  if (bot.facing !== 'back') chest(g, x, base - 5);
  const typing = sitting && Math.floor(t / 170 + bot.phase * 5) % 2;
  r(g, dark, x - 5, base - 6 + (typing ? 1 : 0), 1, 4);
  r(g, dark, x + 4, base - 6 + (typing ? 0 : 1), 1, 4);

  const headW = head.wide;
  const left = x - headW / 2;
  const top = base - 14 - breathe;
  r(g, shell, left + 1, top, headW - 2, 8);
  r(g, shell, left, top + 1, headW, 6);
  // A hat goes on where the antennas would be.
  if (hat !== HATS.none) {
    hat(g, x, top, headW / 2 - 1, color, dark);
  } else if (head.twin) {
    r(g, '#9aa3b8', left + 2, top - 3, 1, 3);
    r(g, '#9aa3b8', left + headW - 3, top - 3, 1, 3);
    r(g, color, left + 1, top - 4, 3, 1);
    r(g, color, left + headW - 4, top - 4, 3, 1);
  } else {
    r(g, '#9aa3b8', x, top - 3, 1, 3);
    const lit = sitting && Math.floor(t / 400) % 2;
    r(g, lit ? '#ffde59' : color, x - 1, top - 4, 3, 1);
  }

  if (bot.facing === 'back') {
    r(g, '#cdd3df', left + 1, top + 5, headW - 2, 1);
    r(g, '#cdd3df', left + 2, top + 2, headW - 4, 1);
    return;
  }
  r(g, visor, left + 1, top + 2, headW - 2, 4);
  const gaze = bot.facing === 'left' ? -1 : bot.facing === 'right' ? 1 : 0;
  const blink = Math.sin(t / 1900 + bot.phase * 11) > 0.985;
  r(g, glow, x - 3 + gaze, top + 3 + (blink ? 1 : 0), 2, blink ? 1 : 2);
  r(g, glow, x + 1 + gaze, top + 3 + (blink ? 1 : 0), 2, blink ? 1 : 2);
}

// The skin, hair, cap and trousers a person has on the street, so they're the same someone here.
const SKIN = ['#f1c9a4', '#d9a77c', '#b27a54', '#8a5a3c', '#f4d6b8'];
const HAIR = ['#2b2118', '#6a4326', '#b9822f', '#d8c08a', '#4a2a22', '#8a8f9c'];
const TROUSERS = ['#394666', '#4a4a52', '#6a5a46', '#2f4a48'];

// What they chose comes first (C4). A shirt nobody chose stays the person's own colour.
function personLook(name, chosen = {}) {
  const h = hash(name);
  return {
    skin: pick(SKINS, chosen.skin, SKIN[(h >>> 3) % SKIN.length]),
    hair: pick(HAIRS, chosen.hair, HAIR[(h >>> 6) % HAIR.length]),
    style: pick(STYLES, chosen.style, STYLES.short),
    face: pick(FACES, chosen.face, FACES.none),
    hat: pick(HATS, chosen.hat, (h >>> 9) % 3 === 0 ? HATS.cap : HATS.none),
    shirt: pick(FABRICS, chosen.shirt, null),
    trousers: pick(LEGS, chosen.trousers, TROUSERS[(h >>> 11) % TROUSERS.length]),
  };
}

// A bot is a big square head on a small body. A person is the other way round: a small head with
// hair or a cap, and shoulders wider than it.
function drawPerson(g, person, t) {
  const x = Math.round(person.x);
  const y = Math.round(person.y);
  const { facing } = person;
  const { skin, hair, style, face, hat, shirt, trousers } = person.look;
  const color = shirt || person.color;
  const dark = shade(color, -0.32);
  const shoe = '#2b2420';

  const walking = person.path.length > 0;
  const sitting = person.pose === 'sit' && !walking;
  const step = walking ? Math.floor(t / 130 + person.phase * 9) % 2 : 0;
  const breathe = !walking && Math.sin(t / 800 + person.phase * 7) > 0.35 ? 1 : 0;
  const base = sitting ? y - 2 : y - 6;

  r(g, 'rgba(0,0,0,.25)', x - 5, y - 1, 10, 2);
  if (sitting) {
    r(g, trousers, x - 4, y - 2, 8, 2);
    r(g, shoe, x - 4, y - 1, 3, 1);
    r(g, shoe, x + 1, y - 1, 3, 1);
  } else {
    r(g, trousers, x - 3, base - 1, 2, 6 - step);
    r(g, trousers, x + 1, base - 1, 2, 6 - (walking && !step ? 1 : 0));
    r(g, shoe, x - 3, y - 1 - step, 3, 1);
    r(g, shoe, x + 1, y - 1 - (walking && !step ? 1 : 0), 3, 1);
  }

  const torso = base - 7;
  r(g, color, x - 4, torso, 8, 7);
  r(g, dark, x - 4, base - 1, 8, 1);
  const swing = walking ? (step ? 1 : -1) : 0;
  r(g, dark, x - 5, torso + swing + 1, 1, 4);
  r(g, skin, x - 5, torso + swing + 5, 1, 1);
  r(g, dark, x + 4, torso - swing + 1, 1, 4);
  r(g, skin, x + 4, torso - swing + 5, 1, 1);

  const top = torso - 6 - breathe;
  r(g, skin, x - 2, top, 4, 6);
  r(g, skin, x - 3, top + 1, 6, 4);
  const bare = hat === HATS.none;
  if (bare) style.crown(g, x, top, hair);
  if (facing === 'back') {
    style.back(g, x, top, hair);
    hat(g, x, top, 3, color, dark);
    return;
  }
  r(g, skin, x - 1, torso, 2, 1);
  const blink = t > 0 && Math.sin(t / 2300 + person.phase * 11) > 0.985;
  if (facing === 'left' || facing === 'right') {
    const ahead = facing === 'left' ? -1 : 1;
    if (bare) style.side(g, ahead < 0 ? x + 1 : x - 3, top, hair);
    hat(g, x, top, 3, color, dark);
    if (!blink) r(g, '#2a2433', x + ahead * 2 - (ahead < 0 ? 0 : 1), top + 3, 1, 1);
    face.side(g, x, top, ahead, hair);
    return;
  }
  style.front(g, x, top, hair);
  hat(g, x, top, 3, color, dark);
  if (!blink) {
    r(g, '#2a2433', x - 2, top + 3, 1, 1);
    r(g, '#2a2433', x + 1, top + 3, 1, 1);
  }
  face.front(g, x, top, 0, hair);
}

// A figure is { kind, name, color, chosen }, and big enough for its tallest hat.
const FIGURE_WIDE = 26;
const FIGURE_TALL = 36;
const lookOf = ({ kind, name, chosen }) => (kind === 'bot' ? botLook(name, chosen) : personLook(name, chosen));

// A figure on a canvas of its own, drawn still, for the wardrobe.
export function figurePicture(figure, facing = 'front') {
  const canvas = document.createElement('canvas');
  canvas.width = FIGURE_WIDE;
  canvas.height = FIGURE_TALL;
  const still = { ...figure, x: FIGURE_WIDE / 2, y: FIGURE_TALL - 2, path: [], pose: 'stand', facing, phase: 0 };
  const g = canvas.getContext('2d');
  if (figure.kind === 'bot') drawBot(g, { ...still, wears: botLook(figure.name, figure.chosen) }, 0);
  else drawPerson(g, { ...still, look: personLook(figure.name, figure.chosen) }, 0);
  return canvas;
}

// Whether a figure has this word on now, whether it was chosen or worked out from the name.
export function isWearing(figure, part, word) {
  return lookOf(figure)[part] === lookOf({ ...figure, chosen: { ...figure.chosen, [part]: word } })[part];
}

// A bot's tag hangs under its feet. A person's sits over their head, and goes up a line at every
// other seat so two people side by side can both be read, as bot tags do with `drop`.
const TAG_OVER_HEAD = '4.7cqw';
const TAG_RAISED = 'calc(4.7cqw + 1.8em)';

// A box drawn round the cells something takes.
function outline(g, { left, top, wide, deep }, color) {
  r(g, color, left, top, wide, 1);
  r(g, color, left, top + deep - 1, wide, 1);
  r(g, color, left, top, 1, deep);
  r(g, color, left + wide - 1, top, 1, deep);
}

export class Room {
  constructor(stage) {
    this.stage = stage;
    this.canvas = document.createElement('canvas');
    this.canvas.width = W;
    this.canvas.height = H;
    this.canvas.setAttribute('role', 'img');
    this.overlay = document.createElement('div');
    this.overlay.className = 'stage-overlay';
    stage.append(this.canvas, this.overlay);
    this.g = this.canvas.getContext('2d');
    this.background = document.createElement('canvas');
    this.background.width = W;
    this.background.height = H;
    this.paintedHour = -1;
    this.look = PLAIN_LOOK;
    this.bots = new Map();
    this.people = new Map();
    this.peopleSeen = false;
    this.items = [];
    this.itemsKey = '';
    this.ghost = null;
    this.kept = [];
    this.chosen = null;
    this.confetti = [];
    this.last = performance.now();
    this.calm = matchMedia('(prefers-reduced-motion: reduce)').matches;
    this.running = true;
    requestAnimationFrame((t) => this.frame(t));
  }

  destroy() {
    this.running = false;
    fillGrid([]);
  }

  // A room that isn't a floor keeps the plain look.
  setLook({ paint, flooring, hidden } = PLAIN_LOOK) {
    if (paint === this.look.paint && flooring === this.look.flooring
      && hidden.join() === this.look.hidden.join()) return;
    this.look = { paint, flooring, hidden };
    fillGrid(hidden, this.items);
    this.paintedHour = -1;
  }

  // Called on every render, so it does nothing unless what's on the floor changed.
  setItems(items = []) {
    const known = items.filter(({ item, x, y }) => Object.hasOwn(ITEMS, item)
      && Number.isInteger(x) && Number.isInteger(y)
      && x >= 0 && x < FLOOR_COLS && y >= 0 && y < FLOOR_ROWS);
    const key = known.map(({ id, item, x, y, facing }) => `${id}:${item}:${x}:${y}:${facing}`).join();
    if (key === this.itemsKey) return;
    this.itemsKey = key;
    this.items = known.map(standing);
    this.describe();
    fillGrid(this.look.hidden, this.items);
    // Whoever's walking takes the new way round, and whoever something landed on steps off it.
    for (const who of [...this.bots.values(), ...this.people.values()]) {
      if (who.path.length) this.route(who);
      else if (who.pose === 'stand' && !clear(who.x, who.y)) this.send(who, standAt([[who.x, who.y]]), 'walk');
    }
  }

  // The floor cell under a place on the page, at whatever size the room is drawn.
  cellAt(clientX, clientY) {
    const box = this.canvas.getBoundingClientRect();
    if (!box.width || !box.height) return null;
    const x = Math.floor(((clientX - box.left) / box.width) * W / FLOOR_CELL);
    const y = Math.floor((((clientY - box.top) / box.height) * H - FLOOR_TOP) / FLOOR_CELL);
    return x >= 0 && x < FLOOR_COLS && y >= 0 && y < FLOOR_ROWS ? { x, y } : null;
  }

  // The id of what's drawn at a place on the page, with the nearest thing first. A small item is
  // tried a little wider than its cell, or nobody could put a finger on it.
  itemAt(clientX, clientY) {
    const box = this.canvas.getBoundingClientRect();
    if (!box.width || !box.height) return null;
    const x = ((clientX - box.left) / box.width) * W;
    const y = ((clientY - box.top) / box.height) * H;
    const under = this.items.filter((item) => {
      const spare = Math.max(0, (24 - item.wide) / 2);
      const reach = item.facing % 2 ? item.deep + item.rise - item.wide : Math.max(item.rise, item.deep);
      return x >= item.left - spare && x < item.left + item.wide + spare && y >= item.y - reach && y < item.y;
    });
    return under.sort((a, b) => Number(Boolean(b.sign)) - Number(Boolean(a.sign)) || b.y - a.y)[0]?.id ?? null;
  }

  // What someone is about to put down: { item, x, y, w, h, facing, ok, lifted }, in cells, or null.
  // `lifted` is the id of the placed item it is, if it's being moved. `kept` is the boxes of cells
  // that stay clear, to shade while something's in hand.
  setGhost(ghost, kept = []) {
    this.ghost = ghost && Object.hasOwn(ITEMS, ghost.item) ? { ...standing(ghost), ok: ghost.ok, lifted: ghost.lifted } : null;
    this.kept = kept;
  }

  // The placed item that build mode has selected, by id.
  setChosen(id) {
    this.chosen = id;
  }

  // Called on every render, so it only moves someone who has just come in.
  setPeople(list = []) {
    const seen = new Set();
    const first = !this.peopleSeen;
    this.peopleSeen = true;
    for (const data of list) {
      seen.add(data.id);
      let person = this.people.get(data.id);
      if (!person) {
        person = this.addPerson(data);
        this.placePerson(person, first ? 'instant' : 'door');
      }
      if (person.name !== data.name) {
        person.name = data.name;
        person.label.textContent = data.name;
        person.chip.title = data.name;
      }
      person.look = personLook(data.name, data.look);
      if (person.color !== data.color) {
        person.color = data.color;
        person.dot.style.background = data.color;
      }
    }
    for (const [id, person] of this.people) {
      if (seen.has(id)) continue;
      person.tag.remove();
      this.people.delete(id);
    }
    this.describe();
  }

  setBots(list) {
    const seen = new Set();
    const first = this.bots.size === 0;
    for (const data of list) {
      seen.add(data.id);
      let bot = this.bots.get(data.id);
      if (!bot) {
        bot = this.addBot(data);
        this.place(bot, data.state, first ? 'instant' : 'door');
      } else if (bot.state !== data.state || bot.working !== Boolean(data.activity)) {
        bot.working = Boolean(data.activity);
        this.place(bot, data.state, 'walk');
      }
      bot.state = data.state;
      bot.color = data.color;
      bot.wears = botLook(data.name, data.look);
      bot.chip.title = data.status ? `${data.name}: ${data.status}` : data.name;
      bot.doing.textContent = data.activity ? `· ${data.activity.via}` : data.asking ? '· waiting' : '';
    }
    for (const [id, bot] of this.bots) {
      if (seen.has(id)) continue;
      bot.tag.remove();
      this.bots.delete(id);
    }
    this.describe();
  }

  // People aren't bots, so they get their own sentence.
  describe() {
    const counts = { active: 0, idle: 0, offline: 0 };
    for (const bot of this.bots.values()) counts[bot.state] += 1;
    const people = this.people.size;
    const here = people ? ` ${people} ${people === 1 ? 'person' : 'people'} here.` : '';
    const quiet = this.items.some((item) => item.sign) ? ' A quiet sign is up, so bots can\'t post here.' : '';
    this.canvas.setAttribute(
      'aria-label',
      `Hangout room: ${counts.active} bots at the table, ${counts.idle} wandering, ${counts.offline} napping.${here}${quiet}`,
    );
  }

  addPerson(data) {
    const tag = document.createElement('div');
    tag.className = 'bot-tag';
    const chip = document.createElement('span');
    chip.className = 'bot-chip';
    chip.style.top = 'auto';
    const dot = document.createElement('i');
    const label = document.createElement('span');
    chip.append(dot, label);
    tag.append(chip);
    this.overlay.append(tag);
    const person = {
      id: data.id,
      name: null,
      color: null,
      look: null,
      x: 240,
      y: H - 12,
      path: [],
      pose: 'stand',
      facing: 'front',
      goal: null,
      seat: null,
      wait: 0,
      phase: (hash(data.id) % 1000) / 1000,
      tag,
      chip,
      dot,
      label,
    };
    this.people.set(data.id, person);
    return person;
  }

  addBot(data) {
    const tag = document.createElement('div');
    tag.className = 'bot-tag';
    const chip = document.createElement('span');
    chip.className = 'bot-chip';
    const dot = document.createElement('i');
    dot.style.background = data.color;
    const name = document.createElement('span');
    name.textContent = data.name;
    const doing = document.createElement('span');
    doing.className = 'bot-doing';
    chip.append(dot, name, doing);
    const bubble = document.createElement('div');
    bubble.className = 'bubble';
    bubble.hidden = true;
    tag.append(bubble, chip);
    this.overlay.append(tag);
    const bot = {
      id: data.id,
      color: data.color,
      state: data.state,
      x: 240,
      y: H - 12,
      path: [],
      pose: 'stand',
      facing: 'front',
      goal: null,
      seat: null,
      nap: null,
      desk: null,
      working: Boolean(data.activity),
      wait: 0,
      phase: (hash(data.id) % 1000) / 1000,
      wears: botLook(data.name, data.look),
      tag,
      chip,
      doing,
      bubble,
      bubbleTimer: 0,
    };
    this.bots.set(data.id, bot);
    return bot;
  }

  // A bot goes round a person if it can, but it can have any seat no other bot is in.
  claim(list, key) {
    const taken = new Set([...this.bots.values()].map((bot) => bot[key]).filter(Boolean));
    const sat = new Set([...this.people.values()].map((person) => person.seat));
    const open = list.filter((spot) => !taken.has(spot));
    return open.find((spot) => !sat.has(spot)) || open[0];
  }

  place(bot, state, how) {
    bot.seat = null;
    bot.nap = null;
    bot.desk = null;
    let goal = null;
    if (bot.working) {
      goal = this.claim(DESKS, 'desk');
      bot.desk = goal || null;
    }
    if (goal) {
      // Already placed at the desk.
    } else if (state === 'active') {
      goal = this.claim(SEATS, 'seat');
      bot.seat = goal || null;
      if (goal) this.vacate(goal);
    } else if (state === 'offline') {
      goal = this.claim(NAPS, 'nap') || NAPS[NAPS.length - 1];
      bot.nap = goal;
    }
    this.send(bot, goal || standAt(SPOTS), how);
  }

  // Where a bot sits says what it's doing, so a person in that seat gets up. Someone who has just
  // got up goes to stand somewhere, so they can't sit straight back down in it.
  vacate(seat) {
    for (const person of this.people.values()) {
      if (person.seat === seat) this.placePerson(person, 'walk');
    }
  }

  // A person sits only in a seat no bot has, and never at a desk: a lit desk says a bot is working.
  freeSeat() {
    const taken = new Set([...this.bots.values(), ...this.people.values()].map((who) => who.seat));
    const open = SEATS.filter((seat) => !taken.has(seat));
    return open[Math.floor(Math.random() * open.length)];
  }

  // Somewhere nobody else is standing or going, if there is anywhere.
  freeSpots() {
    const goals = [...this.bots.values(), ...this.people.values()].map((who) => who.goal).filter(Boolean);
    const open = SPOTS.filter(([x, y]) => !goals.some((goal) => goal.ax === x && goal.ay === y));
    return open.length ? open : SPOTS;
  }

  placePerson(person, how) {
    const wasSitting = person.seat;
    person.seat = null;
    const seat = !wasSitting && Math.random() < SIT_CHANCE ? this.freeSeat() : null;
    person.seat = seat || null;
    this.send(person, seat || standAt(this.freeSpots()), how);
  }

  // Walks a bot or a person to a goal, or puts them straight there.
  send(who, goal, how) {
    who.goal = goal;
    who.pose = 'stand';
    if (how === 'instant' || this.calm) {
      who.x = goal.x;
      who.y = goal.y;
      who.path = [];
      this.arrive(who);
      return;
    }
    if (how === 'door') {
      who.x = 240;
      who.y = H - 12;
    }
    this.route(who);
  }

  // A seat or a nap is inside the furniture, so the last step goes in from beside it.
  route(who) {
    const { goal } = who;
    who.path = findPath(who.x, who.y, ...clearOf(goal.ax, goal.ay));
    if (goal.pose !== 'stand') who.path.push({ x: goal.x, y: goal.y });
  }

  // A bot in a seat doesn't count this down. A person does, and stays longer than they stand.
  arrive(who) {
    const goal = who.goal;
    who.pose = goal.pose;
    who.facing = goal.facing || 'front';
    who.wait = who.seat ? 20 + Math.random() * 30 : 3 + Math.random() * 7;
  }

  say(agentId, text) {
    const bot = this.bots.get(agentId);
    if (!bot) return;
    bot.bubble.textContent = text.length > 90 ? `${text.slice(0, 87)}…` : text;
    bot.bubble.hidden = false;
    clearTimeout(bot.bubbleTimer);
    const shown = Math.min(7000, Math.max(3000, text.length * 70));
    bot.bubbleTimer = setTimeout(() => {
      bot.bubble.hidden = true;
    }, shown);
  }

  celebrate() {
    if (this.calm) return;
    const colors = ['#ffb454', '#4f9dff', '#3ddc97', '#ff5d7a', '#ffd24d', '#b388ff'];
    for (let i = 0; i < 140; i++) {
      this.confetti.push({
        x: 240 + (Math.random() - 0.5) * 170,
        y: 30,
        vx: (Math.random() - 0.5) * 70,
        vy: -30 - Math.random() * 50,
        color: colors[i % colors.length],
        life: 2 + Math.random() * 1.5,
      });
    }
  }

  // Moves a bot or a person along their path, and says whether they had one.
  walk(who, dt) {
    if (!who.path.length) return false;
    let budget = SPEED * dt;
    while (budget > 0 && who.path.length) {
      const next = who.path[0];
      const dx = next.x - who.x;
      const dy = next.y - who.y;
      const distance = Math.hypot(dx, dy);
      if (Math.abs(dx) > 0.5) who.facing = dx < 0 ? 'left' : 'right';
      if (distance <= budget) {
        who.x = next.x;
        who.y = next.y;
        who.path.shift();
        budget -= distance;
      } else {
        who.x += (dx / distance) * budget;
        who.y += (dy / distance) * budget;
        budget = 0;
      }
    }
    if (!who.path.length) this.arrive(who);
    return true;
  }

  update(bot, dt) {
    if (this.walk(bot, dt)) return;
    // A busy bot that didn't get a seat wanders too, and takes one when it frees up.
    if (!bot.desk && (bot.state === 'idle' || (bot.state === 'active' && !bot.seat))) {
      bot.wait -= dt;
      if (bot.wait <= 0) this.place(bot, bot.state, 'walk');
    }
  }

  // With less motion asked for, a person stays where they first stood.
  updatePerson(person, dt) {
    if (this.walk(person, dt) || this.calm) return;
    person.wait -= dt;
    if (person.wait <= 0) this.placePerson(person, 'walk');
  }

  // Something being moved stays faintly where it was until it lands.
  drawItem(g, item, t) {
    g.globalAlpha = item.id === this.ghost?.lifted ? 0.35 : 1;
    paintItem(g, item, t);
    g.globalAlpha = 1;
  }

  // What build mode adds over the room: the selected item, the cells that stay clear, and what's in
  // hand, with its cells green where it fits and red where it doesn't.
  drawHand(g, t) {
    const chosen = this.items.find((item) => item.id === this.chosen);
    if (chosen) outline(g, chosen, '#ffde59');
    if (!this.ghost) return;
    for (const { x, y, w, h } of this.kept) {
      r(g, 'rgba(255,93,122,.14)', x * FLOOR_CELL, FLOOR_TOP + y * FLOOR_CELL, w * FLOOR_CELL, h * FLOOR_CELL);
    }
    const { ok, left, top, wide, deep } = this.ghost;
    r(g, ok ? 'rgba(61,220,151,.4)' : 'rgba(255,60,90,.5)', left, top, wide, deep);
    g.globalAlpha = 0.85;
    paintItem(g, this.ghost, t);
    g.globalAlpha = 1;
    outline(g, this.ghost, ok ? '#3ddc97' : '#ff3c5a');
  }

  frame(time) {
    if (!this.running) return;
    const dt = Math.min(0.1, (time - this.last) / 1000);
    this.last = time;
    const hour = new Date().getHours();
    if (hour !== this.paintedHour) {
      this.paintedHour = hour;
      paintRoom(this.background.getContext('2d'), hour, this.look);
    }

    const g = this.g;
    const { hidden } = this.look;
    g.drawImage(this.background, 0, 0);
    if (!hidden.includes('bar')) paintBar(g, time);
    if (!hidden.includes('plants')) {
      paintPlant(g, 470, 250);
      paintPlant(g, 14, 92);
    }

    const lit = SEATS.slice(0, 8).map((seat) =>
      [...this.bots.values()].some((bot) => bot.seat === seat && !bot.path.length),
    );
    const deskLit = DESKS.map((desk) =>
      [...this.bots.values()].some((bot) => bot.desk === desk && !bot.path.length),
    );
    const props = [
      { y: 136, draw: () => paintDesk(g, time, deskLit) },
      { y: 150, draw: () => paintTable(g, lit) },
      hidden.includes('arcade') ? null : { y: 172, draw: () => paintArcade(g, time) },
      { y: 200, draw: () => paintCouch(g) },
      hidden.includes('beanbags') ? null : { y: 232, draw: () => paintBeanbags(g) },
    ].filter(Boolean);
    const bots = [...this.bots.values()];
    const people = [...this.people.values()];
    for (const bot of bots) this.update(bot, dt);
    for (const person of people) this.updatePerson(person, dt);
    const still = this.calm ? 0 : time;
    const scene = [
      ...props,
      ...this.items.filter((item) => !item.sign).map((item) => ({ y: item.y, draw: () => this.drawItem(g, item, still) })),
      ...bots.map((bot) => ({ y: bot.y, draw: () => drawBot(g, bot, time) })),
      ...people.map((person) => ({ y: person.y, draw: () => drawPerson(g, person, still) })),
    ];
    scene.sort((a, b) => a.y - b.y);
    for (const item of scene) item.draw();
    // A sign changes what bots can do here, so nothing and nobody ever stands in front of it.
    for (const item of this.items) if (item.sign) this.drawItem(g, item, still);
    this.drawHand(g, still);

    this.confetti = this.confetti.filter((bit) => bit.life > 0);
    for (const bit of this.confetti) {
      bit.life -= dt;
      bit.vy += 90 * dt;
      bit.x += bit.vx * dt;
      bit.y += bit.vy * dt;
      r(g, bit.color, Math.round(bit.x), Math.round(bit.y), 2, 2);
    }

    for (const bot of bots) {
      bot.tag.style.left = `${(bot.x / W) * 100}%`;
      bot.tag.style.top = `${(bot.y / H) * 100}%`;
      const drop = bot.seat && !bot.path.length && bot.seat.drop;
      bot.tag.classList.toggle('drop', Boolean(drop));
    }
    for (const person of people) {
      person.tag.style.left = `${(person.x / W) * 100}%`;
      person.tag.style.top = `${(person.y / H) * 100}%`;
      const raised = person.seat && !person.path.length && person.seat.drop;
      person.chip.style.bottom = raised ? TAG_RAISED : TAG_OVER_HEAD;
    }
    requestAnimationFrame((t) => this.frame(t));
  }
}
