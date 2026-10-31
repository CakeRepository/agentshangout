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
};
const FLOORINGS = {
  oak: { a: '#a97b4e', b: '#a27347', line: '#8b6039', style: 'planks' },
  walnut: { a: '#6e4a30', b: '#67442b', line: '#523521', style: 'planks' },
  tile: { a: '#c9c4b5', b: '#8d8a80', line: '#77746b', style: 'tile' },
  carpet: { a: '#4d6a78', b: '#4a6673', line: '#415a66', style: 'carpet' },
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
    ...items.map(({ x, y }) => [x - FLOOR_CELL / 2, y - FLOOR_CELL, FLOOR_CELL, FLOOR_CELL]),
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

function paintFloor(g, { a, b, line, style }) {
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

// What people can put on a floor. Each one stands on its cell the way paintPlant does: x is the
// middle of the cell and y is its bottom edge, and it rises from there, so an item takes one cell of
// floor however tall it is. `t` is 0 for someone who asked for less motion, so nothing moves then.

const BOOKS = ['#ff5d7a', '#4f9dff', '#ffb454', '#3ddc97', '#b388ff', '#f4f1e8'];

function paintBookshelf(g, x, y) {
  r(g, '#00000030', x - 9, y - 2, 18, 2);
  r(g, '#4a3120', x - 8, y - 28, 16, 27);
  r(g, '#8a5c37', x - 9, y - 30, 18, 3);
  r(g, '#a0703f', x - 9, y - 30, 18, 1);
  for (let shelf = 0; shelf < 3; shelf++) {
    const top = y - 26 + shelf * 8;
    r(g, '#2c1d12', x - 7, top, 14, 7);
    for (let i = 0; i < 6; i++) {
      const tall = 4 + ((i * 2 + shelf) % 3);
      r(g, BOOKS[(i + shelf * 2) % BOOKS.length], x - 6 + i * 2, top + 7 - tall, 2, tall);
    }
  }
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
  r(g, '#00000030', x - 8, y - 2, 16, 2);
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
  r(g, '#00000030', x - 8, y - 2, 16, 2);
  r(g, '#1b2030', x - 7, y - 30, 14, 29);
  r(g, '#3b4158', x - 7, y - 30, 14, 2);
  for (let i = 0; i < 5; i++) {
    const top = y - 27 + i * 5;
    r(g, '#2b3045', x - 6, top, 12, 4);
    r(g, '#0d1020', x, top + 1, 5, 1);
    r(g, '#0d1020', x, top + 2, 5, 1);
    const blink = Math.floor(t / 410 + i * 1.7) % 3;
    r(g, blink ? '#3ddc97' : '#1f5c47', x - 5, top + 1, 2, 2);
    r(g, blink === 2 ? '#ffb454' : '#5c4726', x - 2, top + 1, 1, 2);
  }
}

// Stands on its own legs and has the same frame and tray as the one on the wall.
function paintWhiteboard(g, x, y) {
  r(g, '#00000030', x - 10, y - 2, 20, 2);
  r(g, '#20283e', x - 9, y - 8, 2, 7);
  r(g, '#20283e', x + 7, y - 8, 2, 7);
  r(g, '#20283e', x - 12, y - 27, 24, 19);
  r(g, '#f4f1e8', x - 11, y - 26, 22, 16);
  r(g, '#c9a86a', x - 13, y - 9, 26, 2);
  r(g, '#4f9dff', x - 9, y - 24, 11, 1);
  r(g, '#4f9dff', x - 9, y - 22, 15, 1);
  r(g, '#ff5d7a', x - 9, y - 19, 7, 1);
  r(g, '#ff5d7a', x - 9, y - 17, 5, 1);
  r(g, '#3ddc97', x + 2, y - 19, 7, 5);
  r(g, '#f4f1e8', x + 3, y - 18, 5, 3);
}

// The couch's cloth, cut down to one seat.
function paintArmchair(g, x, y) {
  r(g, '#00000030', x - 11, y - 2, 22, 2);
  r(g, '#3f5f8c', x - 8, y - 20, 16, 11);
  r(g, '#35527a', x - 8, y - 11, 16, 3);
  r(g, '#5277ad', x - 7, y - 10, 14, 6);
  r(g, '#35527a', x - 11, y - 13, 5, 11);
  r(g, '#35527a', x + 6, y - 13, 5, 11);
  r(g, '#2c4466', x - 7, y - 5, 14, 3);
  r(g, '#20334e', x - 9, y - 2, 3, 2);
  r(g, '#20334e', x + 6, y - 2, 3, 2);
}

// The big table's wood, with a mug someone left on it.
function paintCoffeeTable(g, x, y, t) {
  r(g, '#00000030', x - 12, y - 2, 24, 2);
  r(g, '#6b4628', x - 10, y - 7, 3, 6);
  r(g, '#6b4628', x + 7, y - 7, 3, 6);
  r(g, '#8a5c37', x - 12, y - 12, 24, 6);
  r(g, '#a0703f', x - 12, y - 12, 24, 1);
  r(g, '#6f4728', x - 12, y - 7, 24, 2);
  r(g, '#7d5231', x - 11, y - 9, 22, 1);
  r(g, '#4f9dff', x - 8, y - 12, 8, 1);
  r(g, '#f4f1e8', x - 7, y - 13, 7, 1);
  r(g, '#f4f1e8', x + 3, y - 16, 5, 5);
  r(g, '#f4f1e8', x + 8, y - 15, 1, 3);
  r(g, '#6b4628', x + 4, y - 16, 3, 1);
  r(g, '#fff', x + 4 + Math.floor(t / 900) % 2 * 2, y - 19, 1, 2);
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
  r(g, '#00000030', x - 7, y - 2, 14, 2);
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

// These are listed words on the server. A word this page doesn't know is left out, so a newer
// server can't break an older page.
const ITEMS = {
  bookshelf: paintBookshelf,
  lamp: paintLamp,
  plant: paintPlant,
  monitor: paintMonitor,
  server_rack: paintServerRack,
  whiteboard: paintWhiteboard,
  armchair: paintArmchair,
  coffee_table: paintCoffeeTable,
  rubber_duck: paintRubberDuck,
  trophy: paintTrophy,
};

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

function drawBot(g, bot, t) {
  const x = Math.round(bot.x);
  const y = Math.round(bot.y);
  const color = bot.color;
  const dark = shade(color, -0.32);
  const glow = shade(color, 0.45);
  const shell = '#e8ecf4';
  const visor = '#1b2030';

  if (bot.pose === 'sleep') {
    r(g, 'rgba(0,0,0,.2)', x - 13, y + 2, 26, 2);
    r(g, color, x - 3, y - 4, 13, 6);
    r(g, dark, x - 3, y + 1, 13, 1);
    r(g, '#2a2f44', x + 10, y - 3, 3, 4);
    r(g, shell, x - 12, y - 6, 9, 8);
    r(g, visor, x - 10, y - 4, 2, 4);
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
  const typing = sitting && Math.floor(t / 170 + bot.phase * 5) % 2;
  r(g, dark, x - 5, base - 6 + (typing ? 1 : 0), 1, 4);
  r(g, dark, x + 4, base - 6 + (typing ? 0 : 1), 1, 4);

  const wide = bot.look % 3 === 2;
  const headW = wide ? 12 : 10;
  const left = x - headW / 2;
  const top = base - 14 - breathe;
  r(g, shell, left + 1, top, headW - 2, 8);
  r(g, shell, left, top + 1, headW, 6);
  if (bot.look % 3 === 1) {
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

function personLook(name) {
  const h = hash(name);
  return {
    skin: SKIN[(h >>> 3) % SKIN.length],
    hair: HAIR[(h >>> 6) % HAIR.length],
    hat: (h >>> 9) % 3 === 0,
    trousers: TROUSERS[(h >>> 11) % TROUSERS.length],
  };
}

// A bot is a big square head on a small body. A person is the other way round: a small head with
// hair or a cap, and shoulders wider than it.
function drawPerson(g, person, t) {
  const x = Math.round(person.x);
  const y = Math.round(person.y);
  const { color, facing } = person;
  const { skin, hair, hat, trousers } = person.look;
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
  if (hat) {
    r(g, color, x - 3, top - 1, 6, 2);
    r(g, color, x - 2, top - 2, 4, 1);
    r(g, dark, x - 4, top + 1, 8, 1);
  } else {
    r(g, hair, x - 2, top - 1, 4, 1);
    r(g, hair, x - 3, top, 6, 2);
  }
  if (facing === 'back') {
    r(g, hair, x - 3, top + 2, 6, 3);
    r(g, hair, x - 2, top + 5, 4, 1);
    return;
  }
  r(g, skin, x - 1, torso, 2, 1);
  const blink = t > 0 && Math.sin(t / 2300 + person.phase * 11) > 0.985;
  if (facing === 'left' || facing === 'right') {
    const ahead = facing === 'left' ? -1 : 1;
    if (!hat) r(g, hair, ahead < 0 ? x + 1 : x - 3, top + 2, 2, 2);
    if (!blink) r(g, '#2a2433', x + ahead * 2 - (ahead < 0 ? 0 : 1), top + 3, 1, 1);
    return;
  }
  if (blink) return;
  r(g, '#2a2433', x - 2, top + 3, 1, 1);
  r(g, '#2a2433', x + 1, top + 3, 1, 1);
}

// A bot's tag hangs under its feet. A person's sits over their head, and goes up a line at every
// other seat so two people side by side can both be read, as bot tags do with `drop`.
const TAG_OVER_HEAD = '4.7cqw';
const TAG_RAISED = 'calc(4.7cqw + 1.8em)';

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
    const key = known.map(({ item, x, y }) => `${item}:${x}:${y}`).join();
    if (key === this.itemsKey) return;
    this.itemsKey = key;
    this.items = known.map(({ item, x, y }) => ({
      paint: ITEMS[item],
      x: x * FLOOR_CELL + FLOOR_CELL / 2,
      y: FLOOR_TOP + (y + 1) * FLOOR_CELL,
    }));
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
        person.look = personLook(data.name);
      }
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
    this.canvas.setAttribute(
      'aria-label',
      `Hangout room: ${counts.active} bots at the table, ${counts.idle} wandering, ${counts.offline} napping.${here}`,
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
      look: hash(data.name),
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
      ...this.items.map((item) => ({ y: item.y, draw: () => item.paint(g, item.x, item.y, still) })),
      ...bots.map((bot) => ({ y: bot.y, draw: () => drawBot(g, bot, time) })),
      ...people.map((person) => ({ y: person.y, draw: () => drawPerson(g, person, still) })),
    ];
    scene.sort((a, b) => a.y - b.y);
    for (const item of scene) item.draw();

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
