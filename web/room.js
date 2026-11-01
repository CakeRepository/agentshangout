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
  [0, 0, 6, H],
  [W - 6, 0, 6, H],
  [0, H - 6, W, 6],
];

// What an owner can leave out of a floor and bots would otherwise walk round. The table stays,
// because a bot that's busy sits there.
const REMOVABLE = {
  bar: [370, 66, 110, 36],
  arcade: [428, 124, 50, 48],
  plants: [460, 224, 20, 28],
  couch: [12, 190, 96, 34],
  desk: [24, 98, 92, 38],
};

// Where the things that come with a room are drawn, nearest first, so build mode can tell what a
// click landed on. Plants are in two places.
const FURNITURE = [
  ['couch', 12, 188, 96, 36],
  ['beanbags', 120, 230, 88, 14],
  ['plants', 460, 224, 20, 28],
  ['arcade', 432, 124, 42, 48],
  ['table', 176, 124, 128, 48],
  ['desk', 24, 96, 92, 40],
  ['bar', 372, 49, 106, 51],
  ['plants', 6, 66, 16, 28],
  ['rug', 128, 104, 224, 100],
];

// A floor's look. These are listed words on the server, so nothing typed reaches the drawing.
const PAINTS = {
  dusk: { upper: '#364466', stripe: '#3a496d', lower: '#2a3450', panel: '#232c45' },
  sage: { upper: '#4f6b5c', stripe: '#567464', lower: '#3a5145', panel: '#304439' },
  plum: { upper: '#5b3f66', stripe: '#63466f', lower: '#46304f', panel: '#3a2742' },
  clay: { upper: '#8a5a44', stripe: '#94614a', lower: '#6b4333', panel: '#5a382a' },
  butter: { upper: '#b89a5e', stripe: '#c2a468', lower: '#8f7646', panel: '#7a643a' },
  ink: { upper: '#23293a', stripe: '#292f42', lower: '#1a1f2d', panel: '#141824' },
  sky: { upper: '#86bedb', stripe: '#90c6e1', lower: '#5f93ad', panel: '#527f97' },
  haunted: { upper: '#2c2042', stripe: '#33264c', lower: '#20172f', panel: '#2b1a2e', webs: true },
  harvest: { upper: '#9a4a24', stripe: '#a65528', lower: '#4a2e1e', panel: '#3c2418', turned: true },
};
const FLOORINGS = {
  oak: { a: '#a97b4e', b: '#a27347', line: '#8b6039', style: 'planks' },
  walnut: { a: '#6e4a30', b: '#67442b', line: '#523521', style: 'planks' },
  tile: { a: '#c9c4b5', b: '#8d8a80', line: '#77746b', style: 'tile' },
  carpet: { a: '#4d6a78', b: '#4a6673', line: '#415a66', style: 'carpet' },
  grass: { a: '#5f9a4f', b: '#6ba85a', line: '#4f8441', style: 'grass' },
  crypt: { a: '#57545f', b: '#4e4b56', line: '#34323c', style: 'flagstones' },
  leaves: { a: '#5a3c26', b: '#533621', line: '#3f2a1a', style: 'leaves' },
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

// I64: hide a waiting or thinking sign once its until has passed, without waiting for a reload.
function liveThinking(thought) {
  if (!thought || thought.until * 1000 <= Date.now()) return null;
  return thought;
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

function paintWindow(g, x, y, w, h, sky, turned) {
  r(g, '#20283e', x - 3, y - 3, w + 6, h + 7);
  const gradient = g.createLinearGradient(0, y, 0, y + h);
  gradient.addColorStop(0, sky.top);
  gradient.addColorStop(1, sky.low);
  g.fillStyle = gradient;
  g.fillRect(x, y, w, h);
  if (sky.stars) {
    for (let i = 0; i < 9; i++) r(g, '#fff', x + ((i * 37) % w), y + ((i * 23) % (h - 8)), 1, 1);
  }
  const autumn = turned && !sky.stars;
  g.fillStyle = sky.stars ? '#141a3a' : autumn ? '#8a6a30' : '#5f8f6d';
  for (let i = 0; i < w; i++) {
    const hill = Math.round(5 + 3 * Math.sin((i + x) / 9));
    g.fillRect(x + i, y + h - hill, 1, hill);
  }
  if (autumn) {
    // A treeline of small clumps along the ridge, gone red, orange and gold.
    const crowns = ['#b33a1f', '#e8802a', '#d9a520', '#8c2a1e', '#f0a03c', '#7f8c3a', '#d4502a'];
    for (let k = 0; k * 6 + 3 < w; k++) {
      const cx = k * 6 + 3;
      const base = y + h - Math.round(5 + 3 * Math.sin((cx + x) / 9));
      const crown = crowns[(k * 3) % crowns.length];
      r(g, crown, x + cx - 2, base - 3, 5, 3);
      r(g, crown, x + cx - 1, base - 4 - (k % 2), 3, 1 + (k % 2));
      r(g, shade(crown, 0.25), x + cx - 2, base - 3, 2, 1);
      r(g, '#3f2a1c', x + cx, base, 1, 1);
    }
  }
  r(g, '#20283e', x + w / 2 - 1, y, 2, h);
  r(g, '#20283e', x, y + h / 2 - 1, w, 2);
  r(g, '#d8c7a3', x - 5, y + h + 3, w + 10, 2);
}

// A garland of autumn leaves on twine, pinned over the windows and swagging in the gaps either side
// of the whiteboard.
function paintGarland(g) {
  const leaf = ['#b33a1f', '#e8802a', '#d9a520', '#d4502a', '#f0a03c', '#8c2a1e', '#eab34a', '#7f8c3a'];
  const stringOf = [
    [2, 8, 34, 6, 5], [34, 6, 72, 5, 3], [72, 5, 110, 6, 3], [110, 6, 144, 10, 22],
    [336, 10, 370, 6, 14], [370, 6, 408, 5, 3], [408, 5, 446, 6, 3], [446, 6, 478, 8, 5],
  ];
  let n = 0;
  for (const [x0, y0, x1, y1, sag] of stringOf) {
    const span = x1 - x0;
    const twineY = (x) => {
      const u = (x - x0) / span;
      return Math.round(y0 + (y1 - y0) * u + sag * 4 * u * (1 - u));
    };
    for (let x = x0; x <= x1; x++) r(g, '#c9a86a', x, twineY(x), 1, 1);
    for (let x = x0 + 1; x < x1; x += 2) r(g, '#8a5c37', x, twineY(x) + 1, 1, 1);
    // Leaves hang on the twine in clusters of two or three, and every so often an acorn or a berry.
    const step = sag > 10 ? 6 : 9;
    for (let x = x0 + 4; x < x1 - 2; x += step, n++) {
      const y = twineY(x);
      const pick = (k) => leaf[(n * 3 + k * 5) % leaf.length];
      if (n % 4 === 3) {
        r(g, '#6b4628', x, y + 1, 1, 2);
        r(g, '#8a5c37', x - 1, y + 3, 3, 1);
        r(g, '#c9a86a', x - 1, y + 4, 3, 2);
        r(g, '#a8843f', x + 1, y + 4, 1, 2);
        continue;
      }
      if (n % 4 === 2) {
        r(g, '#8c2a1e', x, y + 1, 2, 2);
        r(g, '#d4502a', x, y + 1, 1, 1);
        r(g, pick(1), x - 4, y, 3, 2);
        continue;
      }
      for (const [dx, dy, size] of [[-3, 1, 1], [0, 1, 2], [3, 1, 1]]) {
        const color = pick(dx + 4);
        const lx = x + dx;
        const ly = y + dy;
        r(g, color, lx - 1, ly, 3, 2 + size);
        r(g, color, lx - 2, ly + 1, 5, size);
        r(g, shade(color, 0.28), lx - 1, ly, 1, 1);
        r(g, shade(color, -0.3), lx + 1, ly + 1 + size, 1, 1);
      }
    }
    // A pin at each end.
    r(g, '#3f2a1c', x0, y0 - 1, 2, 3);
    r(g, '#3f2a1c', x1, y1 - 1, 2, 3);
  }
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

// Each is its size, then the rectangles of the leaf, then of its vein.
const FALLEN_LEAVES = [
  [[7, 6], [[3, 0, 1, 1], [1, 1, 5, 1], [0, 2, 7, 2], [1, 4, 5, 1], [3, 5, 1, 1]], [[3, 1, 1, 4]]],
  [[5, 3], [[0, 1, 5, 1], [1, 0, 2, 1], [2, 2, 2, 1]], [[1, 1, 3, 1]]],
  [[6, 5], [[2, 0, 2, 1], [1, 1, 4, 1], [0, 2, 6, 1], [1, 3, 4, 1], [2, 4, 2, 1]], [[2, 2, 3, 1]]],
];
const FALLEN_COLORS = ['#8c2a1e', '#a8401f', '#b86a1e', '#a8841f', '#5f6b2e', '#c8641e'];
const LEAF_HEAPS = [[58, 150], [425, 165], [250, 238]];

// Leaves drift against the walls and thin out where people walk. Two small mixers place them, so
// that no rows or columns show, and most of each drift lies close to its wall.
function paintFallenLeaves(g) {
  const near = (n, reach) => Math.floor((n * n * reach) / 1e6);
  for (let i = 0; i < 120; i++) {
    const one = Math.imul(i + 1, 2654435761) >>> 0;
    const two = Math.imul(i + 7, 2246822519) >>> 0;
    const across = (one >>> 8) % 1000;
    const down = (two >>> 8) % 1000;
    const drift = (one >>> 3) % 10;
    let x = Math.floor((across * (W - 10)) / 1000);
    let y = WALL + 3 + Math.floor((down * (H - WALL - 12)) / 1000);
    if (drift < 4) y = WALL + 3 + near(down, 32);
    else if (drift < 6) x = near(across, 60);
    else if (drift < 8) x = W - 10 - near(across, 60);
    else if (drift === 8) y = H - 8 - near(down, 32);
    else {
      const [heapX, heapY] = LEAF_HEAPS[(one >>> 12) % LEAF_HEAPS.length];
      x = heapX + (across % 25) - 12;
      y = heapY + (down % 15) - 7;
    }
    const [size, blade, vein] = FALLEN_LEAVES[(two >>> 4) % FALLEN_LEAVES.length];
    const color = FALLEN_COLORS[(one >>> 16) % FALLEN_COLORS.length];
    const turn = (two >>> 2) % 4;
    const fill = (tone, rects) => {
      for (const [dx, dy, w, h] of rects) {
        const [left, top, wide, tall] = turn & 1 ? [dy, dx, h, w] : [dx, dy, w, h];
        r(g, tone, x + (turn & 2 ? size[turn & 1] - left - wide : left), y + top, wide, tall);
      }
    };
    fill(color, blade);
    fill(shade(color, -0.35), vein);
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
  if (style === 'grass') {
    r(g, a, 0, WALL + 1, W, H - WALL - 1);
    // Tufts at places that don't line up, so it doesn't read as a pattern.
    for (let y = WALL + 4, row = 0; y < H; y += 6, row++) {
      for (let x = (row * 13) % 17; x < W; x += 17 + (row * 3) % 5) {
        r(g, b, x, y, 1, 3);
        r(g, b, x + 2, y + 1, 1, 2);
        r(g, line, x + 1, y + 3, 2, 1);
      }
    }
    return;
  }
  for (let y = WALL + 1, row = 0; y < H; y += 10, row++) {
    r(g, row % 2 ? a : b, 0, y, W, 10);
    r(g, line, 0, y + 9, W, 1);
    for (let x = (row * 37) % 60; x < W; x += 60) r(g, line, x, y, 1, 9);
  }
  if (style === 'leaves') paintFallenLeaves(g);
}

// Under the table.
function paintRug(g) {
  r(g, '#6a2a3c', 128, 104, 224, 100);
  r(g, '#c9a86a', 132, 108, 216, 1);
  r(g, '#c9a86a', 132, 199, 216, 1);
  r(g, '#c9a86a', 132, 108, 1, 92);
  r(g, '#c9a86a', 347, 108, 1, 92);
  r(g, '#7b3346', 138, 114, 204, 80);
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

  paintWindow(g, 36, 10, 72, 32, sky, paint.turned);
  paintWindow(g, 372, 10, 72, 32, sky, paint.turned);
  if (paint.webs) paintWebs(g);
  if (paint.turned) paintGarland(g);

  // Floor
  paintFloor(g, FLOORINGS[look.flooring] || FLOORINGS.oak);
  r(g, 'rgba(30,20,50,.20)', 0, WALL + 1, W, 3);

  if (!look.hidden.includes('rug')) paintRug(g);

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
  paintMachine(g, 432, 124, '#2fd6d6', t);
  paintMachine(g, 454, 124, '#ff7ad9', t);
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

// What people furnish a place with in games like RimWorld and The Sims (I47). Most of these are a block
// or two with a little detail on the front, so adding another is a few lines here and one in ITEMS.

// A block on the floor from the front, `tall` high, with a lighter top. Gives back its left edge.
function paintBlock(g, x, y, wide, tall, top, face) {
  const left = x - wide / 2;
  r(g, SHADOW, left, y - 2, wide, 2);
  r(g, face, left, y - tall + 3, wide, tall - 4);
  r(g, top, left, y - tall, wide, 3);
  r(g, shade(top, 0.16), left, y - tall, wide, 1);
  return left;
}

function paintDiningChair(g, x, y) {
  r(g, SHADOW, x - 6, y - 2, 12, 2);
  r(g, '#6b4628', x - 5, y - 10, 2, 9);
  r(g, '#6b4628', x + 3, y - 10, 2, 9);
  r(g, '#a0703f', x - 6, y - 13, 12, 3);
  r(g, '#6b4628', x - 5, y - 24, 2, 11);
  r(g, '#6b4628', x + 3, y - 24, 2, 11);
  r(g, '#8a5c37', x - 5, y - 24, 10, 3);
  r(g, '#8a5c37', x - 5, y - 19, 10, 2);
}

function paintBench(g, x, y) {
  const left = x - 23;
  r(g, SHADOW, left, y - 2, 46, 2);
  r(g, '#6b4628', left + 3, y - 20, 3, 19);
  r(g, '#6b4628', left + 40, y - 20, 3, 19);
  r(g, '#a0703f', left, y - 12, 46, 3);
  r(g, '#c08a52', left, y - 12, 46, 1);
  r(g, '#8a5c37', left, y - 22, 46, 3);
  r(g, '#a0703f', left, y - 17, 46, 2);
}

function paintThrone(g, x, y) {
  r(g, SHADOW, x - 12, y - 2, 24, 2);
  r(g, '#b88a2e', x - 10, y - 38, 20, 28);
  for (const tip of [-10, -2, 6]) r(g, '#ffd24d', x + tip, y - 41, 4, 3);
  r(g, '#8a2f4e', x - 7, y - 34, 14, 22);
  r(g, '#b88a2e', x - 12, y - 16, 24, 15);
  r(g, '#a83f62', x - 8, y - 16, 16, 6);
  r(g, '#cf5f84', x - 8, y - 16, 16, 1);
  r(g, '#ffd24d', x - 12, y - 17, 5, 2);
  r(g, '#ffd24d', x + 7, y - 17, 5, 2);
}

function paintEndTable(g, x, y) {
  r(g, SHADOW, x - 6, y - 2, 12, 2);
  r(g, '#6b4628', x - 5, y - 10, 2, 9);
  r(g, '#6b4628', x + 3, y - 10, 2, 9);
  r(g, '#7d5231', x - 5, y - 11, 10, 5);
  r(g, '#ffd24d', x - 1, y - 9, 2, 1);
  r(g, '#8a5c37', x - 6, y - 14, 12, 3);
  r(g, '#a0703f', x - 6, y - 14, 12, 1);
}

function paintDiningTable(g, x, y, t, wide, deep) {
  const top = paintTop(g, x, y, wide, deep, 10, '#a0703f', '#6f4728', '#6b4628');
  const apart = Math.floor(wide / 4);
  for (const at of [-apart, 0, apart]) {
    r(g, '#f4f1e8', x + at - 4, top + 8, 8, 5);
    r(g, '#d9d3c3', x + at - 2, top + 9, 4, 3);
  }
  r(g, '#3fb573', x - 2, top - 3, 4, 5);
  r(g, '#4f9dff', x - 1, top + 2, 2, 4);
}

function paintPicnicTable(g, x, y, t, wide, deep) {
  const top = paintTop(g, x, y, wide, deep, 9, '#b58a55', '#8a6b4a', '#8a6b4a');
  r(g, '#8a6b4a', x - wide / 2 + 3, y - 5, wide - 6, 2);
  for (let i = 0; i * 6 < wide - 14; i++) {
    r(g, i % 2 ? '#d6453f' : '#f4f1e8', x - wide / 2 + 7 + i * 6, top + 2, 6, deep - 10);
  }
}

function paintBedroll(g, x, y) {
  r(g, SHADOW, x - 11, y - 2, 22, 2);
  r(g, '#3f6a8c', x - 11, y - 7, 18, 6);
  r(g, '#5277ad', x - 11, y - 8, 18, 2);
  r(g, '#2f5069', x - 4, y - 8, 1, 7);
  r(g, '#f4f1e8', x + 6, y - 7, 5, 5);
}

const BLANKET = '#4a78b5';
const WIDE_BLANKET = '#a83f62';

// A bed from the side and a little above: a headboard at one end, a pillow, and the blanket.
function paintBed(g, x, y, wide, deep, blanket) {
  const left = x - wide / 2 + 1;
  const across = wide - 2;
  const back = y - 8 - deep + 6;
  r(g, SHADOW, left, y - 2, across, 2);
  r(g, '#6b4628', left + 1, y - 8, across - 2, 7);
  r(g, '#f4f1e8', left + 4, back, across - 6, deep - 6);
  r(g, blanket, left + 17, back, across - 19, deep - 6);
  r(g, shade(blanket, 0.16), left + 17, back, across - 19, 1);
  r(g, shade(blanket, -0.2), left + 17, y - 9, across - 19, 1);
  r(g, '#fff', left + 6, back + 2, 9, deep - 10);
  r(g, '#6b4628', left, back - 8, 4, y - back + 7);
  r(g, '#8a5c37', left, back - 9, 4, 2);
}

// The same bed from its foot, with the headboard along the back.
function paintBedEnd(g, x, y, wide, deep, blanket) {
  const left = x - wide / 2 + 1;
  const across = wide - 2;
  const back = y - 8 - deep + 6;
  r(g, SHADOW, left, y - 2, across, 2);
  r(g, '#6b4628', left, back - 8, across, 9);
  r(g, '#8a5c37', left, back - 9, across, 2);
  r(g, '#6b4628', left, y - 8, across, 7);
  r(g, '#f4f1e8', left + 2, back + 1, across - 4, deep - 7);
  r(g, '#fff', left + 4, back + 2, across - 8, 6);
  r(g, blanket, left + 2, back + 11, across - 4, deep - 17);
}

function paintBunkBed(g, x, y, t, wide) {
  const left = x - wide / 2 + 1;
  const across = wide - 2;
  r(g, SHADOW, left, y - 2, across, 2);
  r(g, '#6b4628', left, y - 42, 3, 41);
  r(g, '#6b4628', left + across - 3, y - 42, 3, 41);
  for (const level of [y - 6, y - 26]) {
    r(g, '#8a5c37', left + 3, level - 3, across - 6, 3);
    r(g, '#f4f1e8', left + 3, level - 7, across - 6, 4);
    r(g, BLANKET, left + 14, level - 8, across - 20, 5);
    r(g, '#fff', left + 5, level - 8, 8, 4);
  }
  r(g, '#a0703f', left + across - 10, y - 30, 2, 29);
  r(g, '#a0703f', left + across - 5, y - 30, 2, 29);
  for (let rung = 0; rung < 5; rung++) r(g, '#a0703f', left + across - 10, y - 26 + rung * 5, 7, 1);
}

function paintPetBed(g, x, y) {
  r(g, SHADOW, x - 12, y - 2, 24, 2);
  r(g, '#8a4636', x - 12, y - 12, 24, 11);
  r(g, '#c9775c', x - 12, y - 12, 24, 2);
  r(g, '#e8d7b8', x - 9, y - 10, 18, 6);
  r(g, '#b9a784', x - 3, y - 8, 2, 2);
  r(g, '#b9a784', x + 1, y - 8, 2, 2);
  r(g, '#b9a784', x - 1, y - 6, 2, 1);
}

function paintDresser(g, x, y) {
  const left = paintBlock(g, x, y, 34, 24, '#8a5c37', '#6b4628');
  for (let row = 0; row < 3; row++) {
    r(g, '#7d5231', left + 2, y - 20 + row * 6, 30, 5);
    r(g, '#ffd24d', x - 1, y - 18 + row * 6, 2, 1);
  }
}

function paintWardrobe(g, x, y) {
  const left = paintBlock(g, x, y, 34, 40, '#8a5c37', '#5a3a24');
  for (const door of [left + 2, left + 18]) {
    r(g, '#6f4728', door, y - 36, 14, 33);
    r(g, '#7d5231', door + 2, y - 34, 10, 29);
  }
  r(g, '#ffd24d', x - 3, y - 20, 2, 3);
  r(g, '#ffd24d', x + 1, y - 20, 2, 3);
}

function paintCrate(g, x, y) {
  const left = paintBlock(g, x, y, 20, 16, '#b58a55', '#9c7443');
  r(g, '#7a5a33', left, y - 9, 20, 1);
  r(g, '#7a5a33', left + 2, y - 13, 2, 12);
  r(g, '#7a5a33', left + 16, y - 13, 2, 12);
}

function paintChest(g, x, y) {
  const left = paintBlock(g, x, y, 22, 12, '#7d5231', '#6b4628');
  r(g, '#8a5c37', left, y - 17, 22, 6);
  r(g, '#a0703f', left + 1, y - 18, 20, 1);
  r(g, '#c9a86a', left + 3, y - 17, 2, 16);
  r(g, '#c9a86a', left + 17, y - 17, 2, 16);
  r(g, '#ffd24d', x - 2, y - 12, 4, 4);
}

function paintStorageShelf(g, x, y) {
  const left = x - 17;
  r(g, SHADOW, left, y - 2, 34, 2);
  r(g, '#5a6178', left, y - 32, 2, 31);
  r(g, '#5a6178', left + 32, y - 32, 2, 31);
  [0, 1, 2].forEach((row) => {
    const shelf = y - 4 - row * 12;
    r(g, '#7c8699', left, shelf, 34, 2);
    r(g, '#b58a55', left + 4 + row * 3, shelf - 7, 10, 7);
    r(g, '#7a5a33', left + 4 + row * 3, shelf - 7, 10, 1);
    r(g, BOOKS[row], left + 19, shelf - 5, 9, 5);
  });
}

function paintSafe(g, x, y) {
  const left = paintBlock(g, x, y, 22, 22, '#4a4f66', '#3b4158');
  r(g, '#2b3045', left + 3, y - 18, 16, 14);
  r(g, '#c9c4b5', x - 3, y - 14, 6, 6);
  r(g, '#77746b', x - 1, y - 12, 2, 2);
  r(g, '#c9c4b5', x + 5, y - 13, 1, 4);
}

function paintLantern(g, x, y, t) {
  const glow = `rgba(255,214,120,${Math.floor(t / 1900) % 2 ? 0.16 : 0.22})`;
  r(g, glow, x - 8, y - 20, 16, 18);
  r(g, SHADOW, x - 5, y - 2, 10, 2);
  r(g, '#fff0b8', x - 4, y - 14, 8, 10);
  r(g, '#ffde59', x - 1, y - 12, 2, 6);
  r(g, '#3b4158', x - 5, y - 4, 10, 3);
  r(g, '#3b4158', x - 5, y - 16, 10, 2);
  r(g, '#3b4158', x - 5, y - 14, 1, 10);
  r(g, '#3b4158', x + 4, y - 14, 1, 10);
  r(g, '#3b4158', x - 1, y - 20, 2, 4);
}

function paintTorch(g, x, y, t) {
  const flick = Math.floor(t / 150) % 2;
  r(g, 'rgba(255,160,70,.14)', x - 9, y - 36, 18, 18);
  r(g, SHADOW, x - 4, y - 2, 8, 2);
  r(g, '#4a3120', x - 1, y - 22, 3, 21);
  r(g, '#3b4158', x - 3, y - 24, 7, 3);
  r(g, '#ff8a3d', x - 2, y - 31 + flick, 5, 7);
  r(g, '#ffde59', x - 1, y - 28 + flick, 3, 4);
}

function paintCampfire(g, x, y, t) {
  const flick = Math.floor(t / 130);
  r(g, 'rgba(255,150,60,.14)', x - 16, y - 10, 32, 10);
  r(g, SHADOW, x - 12, y - 2, 24, 2);
  r(g, '#4a3120', x - 9, y - 8, 18, 3);
  r(g, '#6b4628', x - 6, y - 10, 12, 3);
  for (let i = 0; i < 4; i++) {
    const tall = 8 + ((flick + i * 3) % 5) * 2;
    r(g, '#ff8a3d', x - 7 + i * 4, y - 9 - tall, 3, tall);
    r(g, '#ffde59', x - 6 + i * 4, y - 5 - tall, 1, tall - 4);
  }
  for (let i = 0; i < 7; i++) r(g, i % 2 ? '#8d8a80' : '#77746b', x - 13 + i * 4, y - 5 + (i % 2), 3, 3);
}

function paintCandelabra(g, x, y, t) {
  const flick = Math.floor(t / 210);
  r(g, 'rgba(255,214,120,.16)', x - 10, y - 36, 20, 14);
  r(g, SHADOW, x - 6, y - 2, 12, 2);
  r(g, '#d9a520', x - 5, y - 4, 10, 3);
  r(g, '#d9a520', x - 1, y - 20, 2, 16);
  r(g, '#d9a520', x - 8, y - 20, 16, 2);
  [-8, -1, 6].forEach((at, i) => {
    r(g, '#f4f1e8', x + at, y - 28, 2, 8);
    r(g, (flick + i) % 2 ? '#ffb454' : '#ffde59', x + at, y - 31, 2, 3);
  });
}

function paintCounter(g, x, y) {
  const left = paintBlock(g, x, y, 46, 18, '#c9c4b5', '#8a5c37');
  for (let door = 0; door < 4; door++) r(g, '#7d5231', left + 2 + door * 11, y - 13, 9, 10);
  r(g, '#a0703f', left + 6, y - 21, 14, 3);
  r(g, '#ff5d7a', left + 26, y - 21, 3, 3);
  r(g, '#ffb454', left + 30, y - 21, 3, 3);
  r(g, '#3ddc97', left + 34, y - 21, 3, 3);
}

function paintStove(g, x, y) {
  const left = paintBlock(g, x, y, 22, 24, '#9aa3b8', '#c9c4b5');
  r(g, '#20283e', left + 3, y - 25, 6, 2);
  r(g, '#20283e', left + 13, y - 25, 6, 2);
  r(g, '#f4f1e8', left + 4, y - 19, 14, 1);
  r(g, '#20283e', left + 4, y - 17, 14, 11);
  r(g, '#ff8a3d', left + 6, y - 9, 10, 1);
  r(g, '#3b4158', left + 6, y - 15, 10, 4);
}

function paintFridge(g, x, y) {
  const left = paintBlock(g, x, y, 22, 38, '#e8ecf4', '#c9d3df');
  r(g, '#9aa3b8', left, y - 25, 22, 1);
  r(g, '#77746b', left + 17, y - 33, 2, 6);
  r(g, '#77746b', left + 17, y - 22, 2, 9);
  r(g, '#ffb454', left + 4, y - 32, 4, 3);
  r(g, '#ff5d7a', left + 9, y - 30, 3, 3);
}

function paintKitchenSink(g, x, y) {
  const left = paintBlock(g, x, y, 22, 18, '#c9c4b5', '#8a5c37');
  r(g, '#77746b', left + 5, y - 18, 12, 2);
  r(g, '#9aa3b8', x - 1, y - 26, 2, 8);
  r(g, '#9aa3b8', x - 1, y - 26, 6, 2);
  r(g, '#7d5231', left + 2, y - 13, 8, 10);
  r(g, '#7d5231', left + 12, y - 13, 8, 10);
}

function paintCoffeeMachine(g, x, y, t) {
  r(g, SHADOW, x - 7, y - 2, 14, 2);
  r(g, '#6b4628', x - 6, y - 10, 12, 9);
  r(g, '#8a5c37', x - 7, y - 11, 14, 2);
  r(g, '#b9c2d4', x - 6, y - 24, 12, 13);
  r(g, '#7c8699', x - 4, y - 19, 8, 5);
  r(g, '#ff5d7a', x + 3, y - 23, 2, 2);
  r(g, '#f4f1e8', x - 2, y - 14, 4, 3);
  r(g, '#fff', x - 1 + Math.floor(t / 900) % 2, y - 28, 1, 3);
}

function paintVendingMachine(g, x, y, t) {
  const left = paintBlock(g, x, y, 22, 40, '#c8342c', '#a82a24');
  r(g, '#0d1020', left + 3, y - 36, 12, 24);
  for (let row = 0; row < 4; row++) {
    for (let col = 0; col < 3; col++) r(g, BOOKS[(row * 2 + col) % BOOKS.length], left + 4 + col * 4, y - 35 + row * 6, 3, 4);
  }
  r(g, '#f4f1e8', left + 16, y - 34, 4, 7);
  r(g, Math.floor(t / 700) % 2 ? '#7fd4ff' : '#3f8fc4', left + 16, y - 25, 4, 2);
  r(g, '#0d1020', left + 3, y - 9, 16, 5);
}

function paintToilet(g, x, y) {
  r(g, SHADOW, x - 6, y - 2, 12, 2);
  r(g, '#e8ecf4', x - 6, y - 24, 12, 11);
  r(g, '#c9d3df', x - 6, y - 14, 12, 1);
  r(g, '#9aa3b8', x + 2, y - 21, 3, 1);
  r(g, '#f4f7fb', x - 7, y - 13, 14, 5);
  r(g, '#e8ecf4', x - 5, y - 8, 10, 7);
  r(g, '#c9d3df', x - 5, y - 2, 10, 1);
}

function paintShower(g, x, y, t) {
  r(g, SHADOW, x - 12, y - 2, 24, 2);
  r(g, '#c9d3df', x - 12, y - 8, 24, 7);
  r(g, '#9aa3b8', x + 9, y - 44, 2, 37);
  r(g, '#9aa3b8', x - 4, y - 44, 14, 2);
  r(g, '#b9c2d4', x - 7, y - 42, 7, 3);
  r(g, 'rgba(191,230,245,.25)', x - 12, y - 40, 24, 32);
  for (let i = 0; i < 3; i++) r(g, '#7fd4ff', x - 6 + i * 2, y - 38 + Math.floor(t / 90 + i * 7) % 28, 1, 2);
  r(g, '#e8ecf4', x - 12, y - 42, 1, 34);
}

function paintMirror(g, x, y) {
  r(g, SHADOW, x - 7, y - 2, 14, 2);
  r(g, '#6b4628', x - 6, y - 3, 12, 2);
  r(g, '#8a5c37', x - 7, y - 36, 14, 33);
  r(g, '#bfe6f5', x - 5, y - 34, 10, 29);
  r(g, '#e8f7fc', x - 3, y - 32, 2, 12);
  r(g, '#e8f7fc', x, y - 30, 1, 6);
}

function paintBathtub(g, x, y, t) {
  const left = x - 23;
  r(g, SHADOW, left, y - 2, 46, 2);
  r(g, '#e8ecf4', left, y - 18, 46, 15);
  r(g, '#f4f7fb', left, y - 18, 46, 1);
  r(g, '#7fd4ff', left + 3, y - 17, 40, 4);
  r(g, '#bfe6f5', left + 6 + (Math.floor(t / 600) % 3) * 4, y - 16, 8, 1);
  r(g, '#c9d3df', left, y - 5, 46, 2);
  r(g, '#9aa3b8', left + 3, y - 3, 3, 2);
  r(g, '#9aa3b8', left + 40, y - 3, 3, 2);
  r(g, '#9aa3b8', left + 40, y - 23, 2, 6);
  r(g, '#9aa3b8', left + 36, y - 23, 6, 2);
}

function paintRadio(g, x, y, t) {
  r(g, SHADOW, x - 7, y - 2, 14, 2);
  r(g, '#7a5233', x - 7, y - 13, 14, 12);
  r(g, '#a0703f', x - 7, y - 13, 14, 1);
  for (let row = 0; row < 4; row++) r(g, '#c9c4b5', x - 5, y - 11 + row * 2, 6, 1);
  r(g, '#ffd24d', x + 3, y - 11, 2, 2);
  r(g, '#20283e', x + 3, y - 7, 2, 2);
  r(g, '#9aa3b8', x + 5, y - 20, 1, 7);
  if (t) r(g, '#ffd24d', x - 3, y - 16 - Math.floor(t / 240) % 7, 2, 2);
}

function paintComputer(g, x, y, t) {
  r(g, SHADOW, x - 12, y - 2, 24, 2);
  r(g, '#c9c4b5', x - 11, y - 10, 22, 9);
  r(g, '#9aa3b8', x - 11, y - 3, 22, 2);
  r(g, '#20283e', x + 1, y - 8, 8, 2);
  r(g, '#3ddc97', x - 9, y - 8, 2, 2);
  r(g, '#c9c4b5', x - 9, y - 27, 18, 17);
  r(g, '#0d1020', x - 7, y - 25, 14, 12);
  r(g, '#3ddc97', x - 5, y - 23, 8, 1);
  r(g, '#3ddc97', x - 5, y - 21, 5, 1);
  r(g, Math.floor(t / 530) % 2 ? '#0d1020' : '#3ddc97', x - 5, y - 19, 3, 2);
}

function paintTelevision(g, x, y, t) {
  r(g, SHADOW, x - 17, y - 2, 34, 2);
  r(g, '#6b4628', x - 16, y - 8, 32, 7);
  r(g, '#8a5c37', x - 17, y - 9, 34, 2);
  r(g, '#20283e', x - 15, y - 31, 30, 22);
  r(g, '#3b4158', x - 15, y - 31, 30, 1);
  r(g, ['#2a6fdb', '#3a9d73', '#c9694a'][Math.floor(t / 2600) % 3], x - 13, y - 29, 26, 18);
  r(g, '#f4f1e8', x - 11, y - 27, 9, 2);
  r(g, '#f4f1e8', x - 11, y - 23, 16, 1);
  r(g, '#f4f1e8', x - 11, y - 20, 12, 1);
}

function paintTelescope(g, x, y) {
  r(g, SHADOW, x - 10, y - 2, 20, 2);
  r(g, '#6b4628', x - 9, y - 12, 2, 11);
  r(g, '#6b4628', x + 7, y - 12, 2, 11);
  r(g, '#6b4628', x - 1, y - 14, 2, 13);
  r(g, '#4a3120', x - 6, y - 15, 12, 3);
  for (let i = 0; i < 6; i++) r(g, i > 3 ? '#d9a520' : '#3b4158', x - 9 + i * 3, y - 20 - i * 3, 6, 5);
  r(g, '#7fd4ff', x + 9, y - 35, 3, 2);
}

function paintDartboard(g, x, y) {
  r(g, SHADOW, x - 6, y - 2, 12, 2);
  r(g, '#4a3120', x - 5, y - 3, 10, 2);
  r(g, '#4a3120', x - 1, y - 16, 2, 13);
  r(g, '#20283e', x - 8, y - 30, 16, 16);
  r(g, '#f4f1e8', x - 7, y - 29, 14, 14);
  r(g, '#c8342c', x - 6, y - 28, 12, 12);
  r(g, '#f4f1e8', x - 4, y - 26, 8, 8);
  r(g, '#3a9d73', x - 2, y - 24, 4, 4);
  r(g, '#c8342c', x - 1, y - 23, 2, 2);
  r(g, '#ffd24d', x + 2, y - 27, 4, 1);
}

function paintChessTable(g, x, y, t, wide, deep) {
  const top = paintTop(g, x, y, wide, deep, 8, '#6b4628', '#4a3120', '#4a3120');
  for (let row = 0; row < 4; row++) {
    for (let col = 0; col < 6; col++) r(g, (row + col) % 2 ? '#f4f1e8' : '#20283e', x - 9 + col * 3, top + 2 + row * 3, 3, 3);
  }
  r(g, '#ffd24d', x - 6, top - 1, 2, 4);
  r(g, '#c8342c', x + 4, top + 5, 2, 4);
}

// One arcade machine, standing with its top left at `left` and `top`.
function paintMachine(g, left, top, glow, t) {
  r(g, '#26233f', left, top, 20, 48);
  r(g, '#3a3560', left, top, 20, 3);
  r(g, '#0d1020', left + 3, top + 6, 14, 14);
  const blip = Math.floor(t / 300 + left) % 3;
  r(g, glow, left + 5 + blip * 3, top + 10 + blip, 3, 3);
  r(g, shade(glow, 0.4), left + 7, top + 16, 6, 1);
  r(g, '#3a3560', left + 2, top + 24, 16, 5);
  r(g, '#ffd24d', left + 5, top + 25, 2, 2);
  r(g, '#ff5d7a', left + 12, top + 25, 2, 2);
}

function paintArcadeCabinet(g, x, y, t) {
  r(g, SHADOW, x - 11, y - 2, 22, 2);
  paintMachine(g, x - 10, y - 48, '#ffb454', t);
}

function paintPinball(g, x, y, t) {
  const blip = Math.floor(t / 250) % 4;
  r(g, SHADOW, x - 11, y - 2, 22, 2);
  r(g, '#3a3560', x - 10, y - 12, 2, 11);
  r(g, '#3a3560', x + 8, y - 12, 2, 11);
  r(g, '#26233f', x - 11, y - 24, 22, 12);
  r(g, '#0d3a5c', x - 9, y - 23, 18, 8);
  r(g, '#ff7ad9', x - 6, y - 21, 3, 3);
  r(g, '#ffd24d', x + 3, y - 22, 3, 3);
  r(g, '#f4f1e8', x - 7 + blip * 4, y - 21 + (blip % 2) * 3, 2, 2);
  r(g, '#3a3560', x - 11, y - 14, 22, 3);
  r(g, '#26233f', x - 10, y - 36, 20, 12);
  r(g, '#ff7ad9', x - 8, y - 34, 16, 8);
  r(g, '#ffd24d', x - 6, y - 32, 12, 2);
}

function paintBilliards(g, x, y, t, wide, deep) {
  const top = paintTop(g, x, y, wide, deep, 11, '#6b4628', '#4a3120', '#4a3120');
  const left = x - wide / 2 + 4;
  const across = wide - 8;
  const down = deep - 12;
  r(g, '#2f7d5b', left, top + 3, across, down);
  r(g, '#3a9d73', left, top + 3, across, 1);
  for (const [px, py] of [[0, 0], [across - 3, 0], [0, down - 3], [across - 3, down - 3]]) r(g, '#141014', left + px, top + 3 + py, 3, 3);
  r(g, '#f4f1e8', x - 18, top + 11, 3, 3);
  [[8, 8], [11, 6], [11, 10], [14, 4], [14, 8], [14, 12]].forEach(([bx, by], i) => r(g, BOOKS[i], x + bx, top + 3 + by, 3, 3));
}

function paintGuitar(g, x, y) {
  r(g, SHADOW, x - 6, y - 2, 12, 2);
  r(g, '#20283e', x - 5, y - 3, 10, 2);
  r(g, '#6b4628', x - 1, y - 32, 2, 16);
  r(g, '#20283e', x - 2, y - 35, 4, 4);
  r(g, '#c9843a', x - 5, y - 19, 10, 8);
  r(g, '#c9843a', x - 6, y - 12, 12, 9);
  r(g, '#2c1d12', x - 2, y - 12, 4, 4);
  r(g, '#f4f1e8', x, y - 32, 1, 26);
}

function paintDrumKit(g, x, y, t) {
  const hit = Math.floor(t / 320) % 2;
  r(g, SHADOW, x - 17, y - 2, 34, 2);
  r(g, '#a83f62', x - 9, y - 17, 18, 16);
  r(g, '#f4f1e8', x - 7, y - 15, 14, 12);
  r(g, '#c9c4b5', x - 7, y - 10, 14, 1);
  for (const at of [-10, 1]) {
    r(g, '#a83f62', x + at, y - 24, 9, 7);
    r(g, '#f4f1e8', x + at, y - 25, 9, 2);
  }
  for (const [at, tilt] of [[-16, hit], [14, 1 - hit]]) {
    r(g, '#9aa3b8', x + at, y - 22, 1, 21);
    r(g, '#d9a520', x + at - 4, y - 24 + tilt, 9, 2);
  }
}

function paintHarp(g, x, y) {
  r(g, SHADOW, x - 11, y - 2, 22, 2);
  for (let i = 0; i < 6; i++) r(g, '#f4f1e8', x - 5 + i * 3, y - 38 + i * 2, 1, 33 - i * 2);
  r(g, '#d9a520', x - 10, y - 5, 20, 4);
  r(g, '#d9a520', x - 9, y - 40, 3, 36);
  r(g, '#ffd24d', x - 9, y - 40, 1, 36);
  for (let i = 0; i < 7; i++) r(g, '#d9a520', x - 7 + i * 3, y - 40 + i * 2, 4, 3);
  r(g, '#b88a2e', x + 8, y - 28, 3, 24);
}

function paintCactus(g, x, y) {
  r(g, '#b0623d', x - 5, y - 7, 10, 7);
  r(g, '#8f4c2d', x - 5, y - 2, 10, 2);
  r(g, '#2f8f5b', x - 3, y - 24, 6, 17);
  r(g, '#3fb573', x - 2, y - 24, 2, 17);
  r(g, '#2f8f5b', x - 7, y - 18, 4, 3);
  r(g, '#2f8f5b', x - 7, y - 21, 3, 4);
  r(g, '#2f8f5b', x + 3, y - 15, 4, 3);
  r(g, '#2f8f5b', x + 4, y - 20, 3, 6);
  r(g, '#ff7ad9', x - 1, y - 26, 3, 2);
}

function paintFlowerBox(g, x, y) {
  const left = paintBlock(g, x, y, 34, 10, '#8a5c37', '#6b4628');
  for (let i = 0; i < 8; i++) {
    r(g, '#2f8f5b', left + 2 + i * 4, y - 14, 2, 5);
    r(g, BOOKS[i % BOOKS.length], left + 1 + i * 4, y - 17 + (i % 2), 4, 4);
  }
}

function paintHedge(g, x, y) {
  const left = x - 17;
  r(g, SHADOW, left, y - 2, 34, 2);
  r(g, '#2f7d4b', left, y - 20, 34, 19);
  r(g, '#3fb573', left + 1, y - 21, 32, 1);
  for (let i = 0; i < 16; i++) r(g, i % 3 ? '#3fb573' : '#256b3f', left + (i * 13) % 31, y - 19 + (i * 7) % 16, 3, 2);
}

function paintBoulder(g, x, y) {
  r(g, SHADOW, x - 11, y - 2, 22, 2);
  r(g, '#77746b', x - 11, y - 10, 22, 9);
  r(g, '#8d8a80', x - 8, y - 14, 16, 5);
  r(g, '#a9a69b', x - 1, y - 14, 8, 1);
  r(g, '#4f7a35', x - 8, y - 14, 7, 3);
  r(g, '#61805a', x + 3, y - 6, 5, 2);
}

function paintPottedTree(g, x, y, t) {
  const sway = Math.floor(t / 1500) % 2;
  r(g, '#b0623d', x - 7, y - 10, 14, 10);
  r(g, '#8f4c2d', x - 7, y - 2, 14, 2);
  r(g, '#6b4628', x - 2, y - 30, 4, 20);
  r(g, '#2f8f5b', x - 11 + sway, y - 44, 22, 16);
  r(g, '#3fb573', x - 8 + sway, y - 52, 16, 12);
  r(g, '#2f8f5b', x - 4 + sway, y - 56, 8, 6);
}

function paintCoatRack(g, x, y) {
  r(g, SHADOW, x - 6, y - 2, 12, 2);
  r(g, '#4a3120', x - 5, y - 3, 10, 2);
  r(g, '#6b4628', x - 1, y - 34, 2, 31);
  r(g, '#6b4628', x - 5, y - 32, 10, 1);
  r(g, '#3a78d4', x - 7, y - 31, 5, 13);
  r(g, '#2c5aa8', x - 7, y - 19, 5, 2);
  r(g, '#b23a48', x + 2, y - 31, 4, 7);
  r(g, '#20283e', x - 3, y - 37, 7, 3);
}

function paintGlobe(g, x, y, t) {
  const land = x - 5 + (Math.floor(t / 600) % 4) * 2;
  r(g, SHADOW, x - 6, y - 2, 12, 2);
  r(g, '#6b4628', x - 5, y - 3, 10, 2);
  r(g, '#6b4628', x - 1, y - 8, 2, 5);
  r(g, '#3f8fc4', x - 6, y - 20, 12, 12);
  r(g, '#3f8fc4', x - 4, y - 22, 8, 16);
  r(g, '#3fb573', land, y - 18, 3, 5);
  r(g, '#3fb573', land + 1, y - 13, 2, 3);
  r(g, '#c9a86a', x - 8, y - 22, 1, 15);
  r(g, '#c9a86a', x - 8, y - 22, 4, 1);
  r(g, '#c9a86a', x - 8, y - 7, 4, 1);
}

function paintFramedPainting(g, x, y) {
  r(g, SHADOW, x - 10, y - 2, 20, 2);
  r(g, '#4a3120', x - 7, y - 8, 2, 7);
  r(g, '#4a3120', x + 5, y - 8, 2, 7);
  r(g, '#c9a86a', x - 11, y - 32, 22, 25);
  r(g, '#7fb6d9', x - 9, y - 30, 18, 12);
  r(g, '#ffd24d', x + 3, y - 28, 3, 3);
  r(g, '#3fb573', x - 9, y - 18, 18, 9);
  r(g, '#6b4628', x - 5, y - 21, 5, 5);
  r(g, '#c8342c', x - 6, y - 23, 7, 2);
}

function paintGrandfatherClock(g, x, y, t) {
  const swing = t ? Math.round(Math.sin(t / 500) * 2) : 0;
  r(g, SHADOW, x - 7, y - 2, 14, 2);
  r(g, '#5a3a24', x - 6, y - 40, 12, 39);
  r(g, '#7d5231', x - 7, y - 44, 14, 5);
  r(g, '#a0703f', x - 7, y - 44, 14, 1);
  r(g, '#f4f1e8', x - 4, y - 38, 8, 8);
  r(g, '#20283e', x - 1, y - 36, 1, 3);
  r(g, '#20283e', x, y - 34, 2, 1);
  r(g, '#2c1d12', x - 3, y - 27, 6, 20);
  r(g, '#d9a520', x + swing, y - 27, 1, 14);
  r(g, '#ffd24d', x - 1 + swing, y - 14, 3, 3);
}

function paintSculpture(g, x, y) {
  paintBlock(g, x, y, 20, 12, '#c9c4b5', '#8d8a80');
  r(g, '#e8e4d8', x - 6, y - 20, 12, 8);
  r(g, '#c9c4b5', x + 2, y - 20, 4, 8);
  r(g, '#e8e4d8', x - 2, y - 24, 4, 4);
  r(g, '#e8e4d8', x - 4, y - 33, 8, 9);
  r(g, '#c9c4b5', x + 1, y - 33, 3, 9);
  r(g, '#8d8a80', x - 2, y - 29, 1, 1);
  r(g, '#8d8a80', x + 1, y - 29, 1, 1);
}

function paintWorkbench(g, x, y, t, wide, deep) {
  const top = paintTop(g, x, y, wide, deep, 12, '#a0703f', '#6f4728', '#4a3120');
  r(g, '#3b4158', x - 15, top + 6, 12, 4);
  r(g, '#9aa3b8', x - 13, top + 3, 2, 3);
  r(g, '#c9c4b5', x + 1, top + 4, 9, 2);
  r(g, '#6b4628', x + 4, top + 6, 3, 6);
  r(g, '#f4f1e8', x + 13, top + 3, 6, 8);
  r(g, '#4f9dff', x + 14, top + 5, 4, 1);
}

function paintToolChest(g, x, y) {
  const left = paintBlock(g, x, y, 22, 20, '#c8342c', '#a82a24');
  for (let row = 0; row < 3; row++) {
    r(g, '#8e221d', left + 2, y - 16 + row * 5, 18, 4);
    r(g, '#c9c4b5', x - 3, y - 15 + row * 5, 6, 1);
  }
}

function paintAnvil(g, x, y) {
  r(g, SHADOW, x - 11, y - 2, 22, 2);
  r(g, '#6b4628', x - 7, y - 6, 14, 5);
  r(g, '#3b4158', x - 5, y - 11, 10, 5);
  r(g, '#4a4f66', x - 9, y - 15, 18, 4);
  r(g, '#4a4f66', x + 9, y - 14, 3, 2);
  r(g, '#6d7890', x - 9, y - 15, 18, 1);
}

function paintSewingTable(g, x, y, t, wide, deep) {
  const top = paintTop(g, x, y, wide, deep, 10, '#8a5c37', '#6f4728', '#6b4628');
  r(g, '#20283e', x - 9, top - 2, 13, 4);
  r(g, '#20283e', x + 1, top - 2, 3, 12);
  r(g, '#20283e', x - 11, top + 9, 15, 2);
  r(g, '#9aa3b8', x - 8, top + 2, 1, 7);
  r(g, BOOKS[0], x + 8, top + 3, 4, 7);
  r(g, BOOKS[1], x + 13, top + 3, 3, 7);
}

function paintResearchBench(g, x, y, t, wide, deep) {
  const top = paintTop(g, x, y, wide, deep, 14, '#c9c4b5', '#8d8a80', '#3b4158');
  const bubble = Math.floor(t / 300) % 3;
  r(g, '#3ddc97', x - 15, top + 5, 6, 6);
  r(g, '#bfe6f5', x - 14, top + 1, 4, 4);
  r(g, '#fff', x - 14 + bubble, top + 6, 1, 1);
  r(g, '#ff7ad9', x - 5, top + 6, 5, 5);
  r(g, '#bfe6f5', x - 4, top + 2, 3, 4);
  r(g, '#20283e', x + 5, top, 14, 10);
  r(g, '#7fd4ff', x + 6, top + 1, 12, 8);
  r(g, '#20283e', x + 8, top + 3, 6, 1);
}

// The Fall pack, for Thanksgiving and the leaves changing.

const FALL_WOOD = ['#3f2a1c', '#6b4628', '#8a5c37', '#a0703f'];
const FALL_STRAW = ['#a8843f', '#c9a86a', '#e3c987'];

// A rounded lump of stacked rows, `hw` out each side and `hh` up and down from its middle. `low` cuts
// it off below its middle.
function paintLump(g, color, cx, cy, hw, hh, low = hh) {
  for (let dy = -hh; dy <= low; dy++) {
    const w = Math.max(1, Math.round(hw * Math.sqrt(1 - (dy / (hh + 0.5)) ** 2)));
    r(g, color, cx - w, cy + dy, w * 2, 1);
  }
}

// The Fall pack's trees and leaves. Nothing here draws a curve: a round clump is a stack of rows, as
// with the pumpkin. Leaves that fall hold where they were for someone who asked for less motion.

const LEAVES_RED = ['#8c2a1e', '#b33a1f', '#d4502a'];
const LEAVES_ORANGE = ['#c8641e', '#e8802a', '#f0a03c'];
const LEAVES_GOLD = ['#d9a520', '#eab34a', '#f6d98a'];
const LEAVES_GREEN = ['#5f6b2e', '#7f8c3a', '#a3a84a'];

// One loose leaf, two pixels square with a darker underside.
function leavesLeaf(g, color, x, y) {
  r(g, color, x, y, 2, 2);
  r(g, shade(color, -0.22), x + 1, y + 1, 2, 1);
}

// A leaf let go: it drifts down swaying, lies a moment on the floor and goes. `phase` is where it is
// in its cycle.
function leavesFall(g, color, x, top, ground, phase, sway, t, seed) {
  if (phase >= 48) return;
  if (phase >= 34) {
    r(g, color, x, ground - 1, 3, 1);
    return;
  }
  const drop = Math.round(phase * ((ground - top) / 34));
  const over = Math.round(Math.sin(t / 260 + seed * 2) * sway);
  leavesLeaf(g, color, x + over, top + drop);
}

// A whole number scrambled from another, so leaves land the same way every time.
function leavesMix(n) {
  n = Math.imul(n ^ (n >>> 15), 0x2c1b3c6d);
  n = Math.imul(n ^ (n >>> 12), 0x297a2d39);
  return (n ^ (n >>> 15)) >>> 0;
}

// A clump of leaves: a few rows with ragged ends, so no edge is a smooth curve.
function leavesCloud(g, color, cx, cy, hw, hh, low, seed) {
  for (let dy = -hh; dy <= low; dy++) {
    const w = Math.round(hw * Math.sqrt(1 - (dy / (hh + 0.5)) ** 2));
    const m = leavesMix(cx * 31 + cy * 17 + dy * 7 + seed);
    const left = Math.max(1, w + (m % 3) - 1);
    const right = Math.max(1, w + ((m >>> 4) % 3) - 1);
    r(g, color, cx - left, cy + dy, left + right, 1);
  }
}

// A crown built in layers from clumps [dx, dy, hw, hh, lit] around (x, y): all dark, then the mid tone
// moved up and left, then light only on the lit ones, then dabs of single leaves, some of them past the
// edge. `tones(dx, dy)` gives [dark, mid, light, top] and `dabs` is { n, seed, color(dx, dy), wide, top }.
function paintCrown(g, x, y, clumps, tones, dabs) {
  for (const [dx, dy, hw, hh] of clumps) leavesCloud(g, tones(dx, dy)[0], x + dx, y + dy, hw, hh, hh, 1);
  for (const [dx, dy, hw, hh] of clumps) {
    leavesCloud(g, tones(dx, dy)[1], x + dx - 1, y + dy - 2, Math.max(1, hw - 1), hh - 1, hh - 3, 2);
  }
  for (const [dx, dy, hw, hh, lit] of clumps) {
    if (lit > 0) leavesCloud(g, tones(dx, dy)[2], x + dx - 2, y + dy - 3, Math.max(1, hw - 2), Math.max(2, hh - 3), 0, 3);
    if (lit > 1) leavesCloud(g, tones(dx, dy)[3], x + dx - 3, y + dy - 4, Math.max(1, hw - 3), Math.max(2, hh - 4), -1, 4);
  }
  for (let i = 0; i < dabs.n; i++) {
    const m = leavesMix(i * 7919 + dabs.seed);
    const [dx, dy, hw, hh] = clumps[m % clumps.length];
    const n = leavesMix(m);
    let ox = (n % (2 * hw + 3)) - hw - 1;
    let oy = ((n >>> 8) % (2 * hh + 3)) - hh - 1;
    if (i % 6 === 0) {
      if (Math.abs(dx) > 6) ox = Math.sign(dx) * (hw + 1);
      else oy = dy < 0 ? -hh - 1 : hh + 1;
    }
    const px = dx + ox;
    const py = dy + oy;
    if (Math.abs(px) > dabs.wide - 2 || py < dabs.top) continue;
    r(g, dabs.color(px, py), x + px, y + py, (n >>> 16) & 1 ? 2 : 1, (n >>> 16) & 1 ? 1 : 2);
  }
}

function paintMapleTree(g, x, y, t, wide) {
  const k = Math.min(1, Math.max(0.7, wide / 36));
  const s = (v) => Math.round(v * k);
  const [deep, red, bright] = LEAVES_RED;
  const [, orange, amber] = LEAVES_ORANGE;
  const cy = y - 40;
  r(g, SHADOW, x - 17, y - 2, 34, 2);
  // The trunk flares at its foot, with roots, and forks up into the crown.
  r(g, FALL_WOOD[1], x - 10, y - 2, 20, 2);
  r(g, FALL_WOOD[1], x - 8, y - 5, 16, 3);
  r(g, FALL_WOOD[1], x - 4, y - 36, 8, 32);
  r(g, FALL_WOOD[2], x - 10, y - 2, 3, 2);
  r(g, FALL_WOOD[2], x - 8, y - 5, 2, 3);
  r(g, FALL_WOOD[2], x - 4, y - 36, 2, 32);
  r(g, FALL_WOOD[0], x + 2, y - 36, 2, 32);
  r(g, FALL_WOOD[0], x + 3, y - 5, 3, 3);
  r(g, FALL_WOOD[0], x + 6, y - 2, 3, 2);
  r(g, FALL_WOOD[0], x - 1, y - 24, 1, 6);
  r(g, FALL_WOOD[0], x - 2, y - 14, 2, 1);
  // Lumps wider at the shoulders and lopsided, the last number saying how much light each takes.
  const lumps = [
    [-12, 6, 6, 5, 0], [-4, 10, 7, 5, 0], [5, 11, 6, 5, 0], [12, 6, 6, 5, 0],
    [-13, -3, 5, 6, 1], [-6, 0, 8, 7, 0], [4, 0, 8, 7, 0], [13, -3, 5, 6, 0],
    [-12, -9, 6, 6, 2], [11, -10, 6, 6, 1], [-7, -13, 7, 6, 2], [3, -15, 7, 6, 2],
    [-2, -19, 6, 5, 2], [-9, -18, 4, 4, 2], [12, -17, 4, 4, 1], [-15, -14, 3, 4, 1],
  ].map(([dx, dy, hw, hh, lit]) => [s(dx), dy, Math.max(2, s(hw)), hh, lit]);
  const tint = (ux, uy) => {
    const l = (-ux / 19 - uy / 26) / 2 + ((leavesMix(ux * 9 + uy * 5) % 5) - 2) / 14;
    if (l < -0.5) return deep;
    if (l < -0.15) return red;
    if (l < 0.1) return bright;
    if (l < 0.38) return orange;
    return l > 0.6 && ux < -3 ? LEAVES_GOLD[1] : amber;
  };
  paintCrown(g, x, cy, lumps, () => [deep, red, bright, orange], { n: 76, seed: 11, color: tint, wide: 19, top: -26 });
  // The gap where the branches show under the leaves.
  r(g, FALL_WOOD[1], x - 3, y - 34, 6, 10);
  r(g, FALL_WOOD[0], x + 1, y - 34, 2, 10);
  r(g, FALL_WOOD[2], x - 3, y - 34, 1, 10);
  r(g, FALL_WOOD[1], x - 5, y - 36, 2, 3);
  r(g, FALL_WOOD[1], x - 7, y - 38, 2, 3);
  r(g, FALL_WOOD[1], x + 3, y - 36, 2, 3);
  r(g, FALL_WOOD[1], x + 5, y - 38, 2, 3);
  r(g, FALL_WOOD[0], x + 6, y - 38, 1, 3);
  // Leaves on the ground, then three that let go.
  leavesLeaf(g, red, x - 15, y - 3);
  leavesLeaf(g, orange, x + 12, y - 3);
  leavesLeaf(g, LEAVES_GOLD[0], x - 11, y - 1);
  r(g, bright, x + 15, y - 1, 3, 1);
  const drift = [bright, amber, LEAVES_GOLD[1]];
  const from = [-5, 1, 6];
  for (let i = 0; i < 3; i++) {
    leavesFall(g, drift[i], x + s(from[i]), y - 28, y, (t / 40 + i * 19 + 8) % 56, 3, t, i);
  }
}

// Where along a branch (from x0, y0 going `dir`, `run` across and `rise` up) the i-th step is: it
// climbs, then droops at the tip.
function leavesBranchAt(x0, y0, dir, run, rise, i) {
  return [x0 + dir * i, y0 - Math.floor((i * rise) / run) + Math.max(0, i - (run - 3))];
}

function paintBirchTree(g, x, y, t, wide) {
  const k = Math.min(1, Math.max(0.7, wide / 36));
  const s = (v) => Math.round(v * k);
  const cy = y - 46;
  r(g, SHADOW, x - 11, y - 2, 22, 2);
  // The white trunk rises into the crown and forks into slim branches that droop at their tips.
  r(g, '#f4f1e8', x - 2, y - 46, 4, 46);
  r(g, '#d8c7a3', x + 1, y - 46, 2, 46);
  r(g, '#f4f1e8', x - 3, y - 3, 6, 3);
  r(g, '#d8c7a3', x + 2, y - 3, 1, 3);
  r(g, '#f4f1e8', x - 1, y - 58, 2, 12);
  r(g, '#d8c7a3', x + 1, y - 58, 1, 12);
  for (let i = 0; i < 7; i++) {
    r(g, FALL_WOOD[0], x - 2 + (i % 2) * 2, y - 6 - i * 5, 2 + (i % 3 === 0 ? 1 : 0), 1);
    if (i % 2 === 0) r(g, FALL_WOOD[1], x - 1, y - 8 - i * 5, 1, 2);
  }
  const branches = [[30, -1, 12, 9], [33, 1, 13, 11], [40, -1, 14, 14], [42, 1, 14, 15], [48, -1, 9, 10], [50, 1, 9, 11]];
  const lumps = [];
  branches.forEach(([up, dir, run, rise], b) => {
    const reach = Math.max(2, s(run));
    for (let i = 0; i <= reach; i++) {
      const [px, py] = leavesBranchAt(x, y - up, dir, reach, rise, i);
      r(g, '#d8c7a3', px, py, 1, 2);
    }
    [0.4, 0.7, 1].forEach((at, j) => {
      const [px, py] = leavesBranchAt(x, y - up, dir, reach, rise, Math.round(reach * at));
      const m = leavesMix(b * 5 + j * 11 + 3);
      lumps.push([px - x + (m % 3) - 1, py - cy + (j === 1 ? -2 : 2), 3, 2 + ((m >>> 6) % 2), 0]);
    });
  });
  [[0, -13], [-5, -10], [5, -9], [0, -6], [-3, -2], [4, -1]].forEach(([dx, dy]) => lumps.push([dx, dy, 3, 2, 0]));
  lumps.forEach((c) => { c[4] = c[0] + c[1] < -6 ? 1 : 0; });
  // Colour goes with distance from the middle: late green, olive, gold, then orange at the edge.
  const tone = (dx, dy) => {
    const d = Math.hypot(dx / 19, (dy * 0.8) / 20);
    return d < 0.2 ? LEAVES_GREEN[0] : d < 0.36 ? LEAVES_GREEN[1] : d < 0.5 ? LEAVES_GREEN[2]
      : d < 0.64 ? LEAVES_GOLD[0] : d < 0.7 ? LEAVES_GOLD[1] : LEAVES_ORANGE[1];
  };
  const shades = (dx, dy) => {
    const c = tone(dx, dy);
    return [shade(c, -0.28), c, shade(c, 0.18), shade(c, 0.3)];
  };
  const speck = (dx, dy) => shade(tone(dx, dy), ((leavesMix(dx * 3 + dy) % 3) - 1) * 0.18);
  paintCrown(g, x, cy, lumps, shades, { n: 34, seed: 5, color: speck, wide: 19, top: -20 });
  // Gold leaves let go, slower than the maple's.
  for (let i = 0; i < 2; i++) {
    leavesFall(g, i ? LEAVES_GOLD[2] : LEAVES_GOLD[0], x + s(i ? 6 : -6), y - 28, y, (t / 70 + i * 28) % 56, 2, t, i + 3);
  }
  leavesLeaf(g, LEAVES_GOLD[0], x - 12, y - 2);
  r(g, LEAVES_ORANGE[1], x + 10, y - 1, 3, 1);
}

function paintLeafPile(g, x, y, t, wide) {
  const k = Math.min(1, Math.max(0.5, wide / 24));
  const s = (v) => Math.sign(v) * Math.max(1, Math.round(Math.abs(v) * k));
  const [deep, red, bright] = LEAVES_RED;
  const [rust, orange, amber] = LEAVES_ORANGE;
  const [dim, gold, pale] = LEAVES_GOLD;
  r(g, SHADOW, x - 12, y - 2, 24, 2);
  // A raked mound in round clumps of one color each, the darkest at its foot.
  paintLump(g, deep, x, y - 1, s(11), 12, 0);
  paintLump(g, red, x - s(6), y - 3, s(5), 4, 2);
  paintLump(g, rust, x + s(7), y - 3, s(4), 4, 2);
  paintLump(g, bright, x + s(1), y - 4, s(5), 4, 2);
  paintLump(g, dim, x - s(9), y - 3, s(2), 2, 1);
  paintLump(g, red, x + s(2), y - 8, s(7), 5, 2);
  paintLump(g, orange, x - s(5), y - 8, s(5), 4);
  paintLump(g, gold, x + s(6), y - 8, s(4), 3);
  paintLump(g, amber, x - s(4), y - 10, s(4), 3);
  paintLump(g, orange, x + s(2), y - 11, s(4), 2);
  paintLump(g, gold, x - s(1), y - 12, s(3), 1);
  paintLump(g, pale, x - s(6), y - 11, s(2), 1);
  r(g, deep, x - s(2), y - 5, 3, 1);
  r(g, deep, x + s(5), y - 6, 2, 1);
  r(g, bright, x + s(5), y - 10, 2, 1);
  // Loose leaves round it, and one on top that flutters.
  leavesLeaf(g, orange, x - 12, y - 2);
  leavesLeaf(g, red, x + 10, y - 3);
  r(g, gold, x + 8, y - 1, 3, 1);
  r(g, amber, x - 9, y - 1, 2, 1);
  const flap = Math.floor(t / 300) % 2;
  r(g, bright, x + 1 - flap, y - 15 - flap, 3 - flap, 2 + flap);
  r(g, deep, x + 2, y - 14, 2, 1);
}

function paintRake(g, x, y) {
  const [wood, grain, pale] = [FALL_WOOD[1], FALL_WOOD[2], FALL_WOOD[3]];
  r(g, SHADOW, x - 6, y - 2, 12, 2);
  // A wooden handle standing over a fan of tines.
  r(g, wood, x - 1, y - 38, 3, 24);
  r(g, pale, x - 1, y - 38, 1, 24);
  r(g, FALL_WOOD[0], x + 1, y - 38, 1, 24);
  r(g, FALL_STRAW[1], x - 1, y - 38, 3, 2);
  r(g, grain, x, y - 26, 1, 1);
  r(g, FALL_WOOD[0], x - 1, y - 20, 3, 1);
  r(g, FALL_WOOD[0], x - 2, y - 15, 5, 2);
  r(g, FALL_STRAW[0], x - 3, y - 14, 7, 2);
  r(g, FALL_STRAW[1], x - 6, y - 12, 12, 2);
  r(g, FALL_STRAW[2], x - 6, y - 12, 12, 1);
  for (let i = 0; i < 6; i++) {
    r(g, FALL_STRAW[i % 2 ? 0 : 1], x - 6 + i * 2, y - 10, 1, 8);
  }
  // A small heap at its foot and a leaf caught in the tines.
  paintLump(g, LEAVES_RED[1], x + 2, y - 1, 6, 3, 0);
  paintLump(g, LEAVES_ORANGE[1], x - 3, y - 1, 4, 2, 0);
  r(g, LEAVES_GOLD[1], x + 3, y - 4, 3, 1);
  r(g, LEAVES_ORANGE[2], x + 7, y - 1, 3, 1);
  r(g, LEAVES_RED[2], x - 4, y - 9, 3, 2);
  r(g, LEAVES_GOLD[0], x + 1, y - 8, 3, 2);
}

// A bloom five pixels across with petal teeth along its top and bottom and a pale heart.
function leavesBloom(g, x, y, [dark, mid, light]) {
  r(g, mid, x + 1, y, 1, 1);
  r(g, mid, x + 3, y, 1, 1);
  r(g, mid, x, y + 1, 5, 3);
  r(g, dark, x + 4, y + 2, 1, 2);
  r(g, dark, x + 1, y + 4, 1, 1);
  r(g, dark, x + 3, y + 4, 1, 1);
  r(g, light, x, y + 1, 1, 1);
  r(g, light, x + 2, y + 2, 1, 1);
}

function paintMums(g, x, y) {
  const clay = LEAVES_ORANGE[0];
  r(g, SHADOW, x - 7, y - 2, 14, 2);
  // A terracotta pot, wider at the rim.
  r(g, shade(clay, -0.32), x - 5, y - 7, 10, 7);
  r(g, clay, x - 5, y - 7, 8, 7);
  r(g, shade(clay, 0.22), x - 5, y - 7, 2, 6);
  r(g, shade(clay, -0.15), x - 4, y - 1, 8, 1);
  r(g, shade(clay, -0.2), x - 6, y - 10, 12, 4);
  r(g, clay, x - 6, y - 10, 11, 3);
  r(g, shade(clay, 0.25), x - 6, y - 10, 11, 1);
  // Dark green behind the blooms, showing between them and at the base.
  r(g, LEAVES_GREEN[0], x - 9, y - 22, 18, 11);
  r(g, LEAVES_GREEN[0], x - 5, y - 25, 10, 3);
  r(g, LEAVES_GREEN[1], x - 11, y - 13, 3, 2);
  r(g, LEAVES_GREEN[1], x + 8, y - 12, 3, 2);
  r(g, LEAVES_GREEN[2], x - 3, y - 12, 2, 1);
  // A dome of twelve rosettes, mostly plum with gold ones mixed through.
  const plum = ['#5a2a4c', '#7a3a66', '#a3608f'];
  const gold = ['#a8843f', LEAVES_GOLD[0], LEAVES_GOLD[2]];
  const blooms = [
    [-2, -26, 0], [-8, -23, 0], [-4, -23, 1], [0, -23, 0], [4, -23, 0],
    [-10, -20, 0], [-6, -20, 0], [-2, -20, 0], [2, -20, 1], [6, -20, 0],
    [-11, -16, 1], [7, -16, 0],
  ];
  for (const [dx, dy, golden] of blooms) leavesBloom(g, x + dx, y + dy, golden ? gold : plum);
  r(g, LEAVES_GREEN[1], x - 3, y - 16, 2, 1);
  r(g, LEAVES_GREEN[1], x + 3, y - 15, 2, 1);
}

function paintCornStalks(g, x, y, t) {
  const sway = Math.floor(t / 600) % 2;
  r(g, SHADOW, x - 7, y - 2, 14, 2);
  const [dark, mid, light] = FALL_STRAW;
  // Stalks bundled upright, each its own column, and fanning out above the twine.
  for (let i = -3; i <= 3; i++) {
    const tone = i < 0 ? light : i > 1 ? dark : mid;
    r(g, tone, x + i * 2 - 1, y - 16, 2, 16);
    if (i < 3) r(g, FALL_WOOD[1], x + i * 2 + 1, y - 15, 1, 14);
    r(g, tone, x + i * 2 - 1, y - 1, 3, 1);
  }
  for (let i = -3; i <= 3; i++) {
    const tone = i < 0 ? light : i > 1 ? dark : mid;
    const lean = Math.round(i * 1.6);
    r(g, tone, x + lean - 1, y - 28, 2, 13);
    const out = Math.round(i * 3.3) + (i === 0 ? 0 : sway * Math.sign(i));
    r(g, tone, x + out - 1, y - 38, 2, 11);
    r(g, i % 2 ? dark : light, x + out + Math.sign(i) * 2 - 1, y - 44 + Math.abs(i), 1, 8 - Math.abs(i));
  }
  r(g, dark, x - 8, y - 12, 3, 1);
  r(g, mid, x - 9, y - 13, 2, 1);
  r(g, dark, x + 5, y - 9, 3, 1);
  r(g, mid, x + 7, y - 10, 2, 1);
  // Twine round the bundle.
  r(g, FALL_WOOD[2], x - 7, y - 18, 14, 2);
  r(g, FALL_WOOD[1], x - 7, y - 16, 14, 1);
  r(g, FALL_WOOD[2], x + 2, y - 16, 1, 4);
  r(g, FALL_WOOD[2], x - 1, y - 16, 1, 3);
  // Ears of corn peeking out of the bundle.
  r(g, LEAVES_GREEN[1], x - 5, y - 30, 5, 11);
  r(g, LEAVES_GREEN[2], x - 5, y - 30, 1, 11);
  r(g, LEAVES_GREEN[0], x - 1, y - 30, 1, 11);
  r(g, LEAVES_GOLD[1], x - 4, y - 34, 3, 5);
  r(g, LEAVES_GOLD[2], x - 4, y - 34, 1, 4);
  r(g, LEAVES_GOLD[0], x - 2, y - 33, 1, 4);
  r(g, LEAVES_GREEN[0], x + 2, y - 26, 5, 8);
  r(g, LEAVES_GREEN[1], x + 2, y - 26, 1, 8);
  r(g, LEAVES_GOLD[0], x + 3, y - 29, 3, 4);
  r(g, LEAVES_GOLD[1], x + 3, y - 29, 1, 3);
}

// The Thanksgiving feast. Steam and flames move with `t`, and for someone who asked for less motion
// everything sits in a complete, still pose.

const FEAST_CREAM = '#f4f1e8';
const FEAST_LINEN = '#d8c7a3';

// A lump lit from the upper left: a dark body, a lighter middle and a bright patch.
function feastBall(g, base, cx, cy, rx, ry) {
  paintLump(g, shade(base, -0.24), cx, cy, rx, ry);
  paintLump(g, base, cx - 1, cy - 1, rx - 1, ry - 1);
  paintLump(g, shade(base, 0.2), cx - 2, cy - 2, Math.max(1, rx - 3), Math.max(1, ry - 3));
}

// A glossy apple with a stem and one shine.
function feastApple(g, base, cx, cy) {
  feastBall(g, base, cx, cy, 3, 3);
  r(g, FALL_WOOD[0], cx, cy - 4, 1, 2);
  r(g, '#f6d98a', cx - 2, cy - 2, 1, 1);
}

// A wisp of steam that drifts up and thins out as it goes.
function feastSteam(g, cx, base, t, height) {
  for (let i = 0; i < 2; i++) {
    const rise = Math.floor(t / 70 + i * height / 2) % height;
    const alpha = (0.6 - (rise * 0.5) / height).toFixed(2);
    r(g, `rgba(255,255,255,${alpha})`, cx - 2 + i * 4 + (Math.floor(rise / 3) % 2), base - rise, 1, 2);
  }
}

// A taper candle in a brass holder, with a flame that flickers.
function feastCandle(g, cx, base, t, i) {
  const flick = Math.floor(t / 210 + i) % 2;
  r(g, '#d9a520', cx - 1, base - 1, 4, 2);
  r(g, '#a8843f', cx - 1, base + 1, 4, 1);
  r(g, FEAST_CREAM, cx, base - 7, 2, 7);
  r(g, FEAST_LINEN, cx + 1, base - 7, 1, 7);
  r(g, FLAME[1], cx, base - 10 + flick, 2, 3 - flick);
  r(g, FLAME[0], cx, base - 9 + flick, 1, 2);
  r(g, FLAME[2], cx, base - 8, 1, 1);
}

// The roast on its platter, with a leg on each side and a few sprigs of parsley and cranberry.
function feastTurkey(g, cx, cy, t, rx) {
  const gold = '#c8641e';
  const reach = rx - 3;
  paintLump(g, FEAST_LINEN, cx, cy + 1, rx, 5);
  paintLump(g, FEAST_CREAM, cx, cy, rx - 1, 4);
  for (const side of [-1, 1]) {
    paintLump(g, '#a04e17', cx + side * reach, cy + 1, 3, 2);
    paintLump(g, gold, cx + side * reach - 1, cy, 2, 1);
    r(g, FEAST_CREAM, side < 0 ? cx - reach - 5 : cx + reach + 3, cy + 1, 2, 2);
  }
  feastBall(g, gold, cx, cy, 7, 4);
  r(g, '#f0a03c', cx - 4, cy - 3, 4, 1);
  r(g, '#f0a03c', cx - 5, cy - 2, 2, 1);
  r(g, '#8c4a14', cx + 2, cy - 1, 1, 1);
  r(g, '#8c4a14', cx - 1, cy + 1, 2, 1);
  r(g, '#8c4a14', cx + 4, cy, 1, 1);
  r(g, '#7f8c3a', cx - rx + 1, cy + 4, 3, 1);
  r(g, '#5f6b2e', cx - rx + 2, cy + 3, 1, 1);
  r(g, '#7f8c3a', cx + rx - 4, cy + 4, 3, 1);
  r(g, '#b33a1f', cx - rx + 4, cy + 4, 1, 1);
  r(g, '#b33a1f', cx + rx - 5, cy + 4, 1, 1);
  feastSteam(g, cx - 1, cy - 5, t, 11);
}

// A pumpkin pie from above, small enough for a table: crust, filling, a slice gone and cream on top.
function feastTablePie(g, cx, cy) {
  paintLump(g, '#b8870f', cx, cy + 1, 5, 3);
  paintLump(g, '#eab34a', cx, cy, 5, 3);
  paintLump(g, '#c8641e', cx, cy, 4, 2);
  r(g, '#e8802a', cx - 2, cy - 1, 2, 1);
  r(g, FALL_STRAW[1], cx + 1, cy, 5, 2);
  r(g, '#a04e17', cx + 1, cy - 1, 2, 1);
  r(g, FEAST_CREAM, cx - 3, cy - 1, 3, 2);
  r(g, FEAST_CREAM, cx - 2, cy - 2, 1, 1);
}

// A bowl heaped with cranberries.
function feastBowl(g, cx, cy) {
  paintLump(g, '#a8843f', cx, cy + 1, 5, 3);
  paintLump(g, FEAST_LINEN, cx, cy, 5, 3);
  paintLump(g, '#8c2a1e', cx, cy - 1, 4, 2);
  r(g, '#b33a1f', cx - 3, cy - 2, 2, 1);
  r(g, '#d4502a', cx - 1, cy - 3, 2, 1);
  r(g, '#d4502a', cx + 1, cy - 1, 1, 1);
  r(g, '#b33a1f', cx + 2, cy - 2, 2, 1);
  r(g, '#f6d98a', cx - 2, cy - 3, 1, 1);
}

// A long table laid for everyone, from above and in front, with a runner down its length.
function paintHarvestTable(g, x, y, t, wide, deep) {
  const top = paintTop(g, x, y, wide, deep, 12, FALL_WOOD[3], '#6f4728', FALL_WOOD[1]);
  const left = x - wide / 2 + 1;
  const across = wide - 2;
  r(g, FALL_WOOD[2], left + 1, y - 10, across - 2, 2);
  r(g, shade(FALL_WOOD[3], -0.12), left, top + 16, across, 1);
  r(g, shade(FALL_WOOD[3], 0.14), left, top, 1, deep - 6);
  r(g, shade(FALL_WOOD[3], -0.2), left + across - 1, top, 1, deep - 6);
  r(g, FEAST_CREAM, left + 1, top + 3, across - 2, 12);
  r(g, '#d9a520', left + 1, top + 3, across - 2, 1);
  r(g, '#d9a520', left + 1, top + 14, across - 2, 1);
  r(g, FEAST_LINEN, left + 1, top + 13, across - 2, 1);
  for (const end of [left + 3, left + across - 7]) {
    r(g, '#b33a1f', end, top + 5, 4, 1);
    r(g, '#b33a1f', end, top + 12, 4, 1);
    r(g, '#d4502a', end + 1, top + 7, 2, 4);
  }
  r(g, FEAST_CREAM, x - 5, y - 12, 10, 8);
  r(g, FEAST_LINEN, x + 3, y - 12, 2, 8);
  r(g, '#d9a520', x - 5, y - 6, 10, 1);
  for (let i = 0; i < 5; i++) r(g, '#d9a520', x - 5 + i * 2, y - 4, 1, 1);
  feastCandle(g, x - 14, top + 7, t, 0);
  feastCandle(g, x + 13, top + 7, t, 1);
  feastTurkey(g, x, top + 9, t, 11);
  feastTablePie(g, x - 19, top + 10);
  feastBowl(g, x + 19, top + 10);
}

// The same table running back into the room, with the food in a line down the runner.
function paintHarvestTableEnd(g, x, y, t, wide) {
  const left = x - wide / 2 + 1;
  const across = wide - 2;
  const back = y - 36;
  r(g, SHADOW, left, y - 2, across, 2);
  r(g, FALL_WOOD[1], left + 1, y - 12, 3, 10);
  r(g, FALL_WOOD[1], left + across - 4, y - 12, 3, 10);
  r(g, FALL_WOOD[2], left + 1, y - 10, across - 2, 2);
  r(g, FALL_WOOD[3], left, back, across, 22);
  r(g, shade(FALL_WOOD[3], 0.14), left, back, across, 1);
  r(g, shade(FALL_WOOD[3], 0.14), left, back, 1, 22);
  r(g, shade(FALL_WOOD[3], -0.2), left + across - 1, back, 1, 22);
  r(g, '#6f4728', left, y - 14, across, 2);
  r(g, FEAST_CREAM, x - 6, back + 1, 12, 21);
  r(g, '#d9a520', x - 6, back + 1, 1, 21);
  r(g, '#d9a520', x + 5, back + 1, 1, 21);
  r(g, FEAST_LINEN, x + 3, back + 1, 2, 21);
  for (let i = 0; i < 3; i++) r(g, '#b33a1f', x - 2, back + 4 + i * 6, 4, 1);
  r(g, FEAST_CREAM, x - 5, y - 14, 10, 8);
  r(g, FEAST_LINEN, x + 3, y - 14, 2, 8);
  r(g, '#d9a520', x - 5, y - 8, 10, 1);
  for (let i = 0; i < 5; i++) r(g, '#d9a520', x - 5 + i * 2, y - 6, 1, 1);
  feastTablePie(g, x, back + 5);
  feastCandle(g, x - 10, y - 27, t, 0);
  feastCandle(g, x + 9, y - 27, t, 1);
  feastTurkey(g, x, y - 24, t, 8);
  feastBowl(g, x, y - 18);
}

// A whole pumpkin pie on a round wooden stand, with a slice gone and a dollop of cream.
function paintPumpkinPie(g, x, y, t) {
  r(g, SHADOW, x - 8, y - 2, 16, 2);
  r(g, FALL_WOOD[1], x - 5, y - 3, 10, 3);
  r(g, FALL_WOOD[2], x - 5, y - 3, 10, 1);
  r(g, FALL_WOOD[1], x - 1, y - 8, 3, 5);
  r(g, FALL_WOOD[2], x - 1, y - 8, 1, 5);
  paintLump(g, FALL_WOOD[1], x, y - 10, 9, 2);
  paintLump(g, FALL_WOOD[3], x, y - 11, 9, 2);
  r(g, shade(FALL_WOOD[3], 0.18), x - 4, y - 13, 6, 1);
  paintLump(g, '#b8870f', x, y - 11, 8, 4);
  paintLump(g, '#eab34a', x, y - 12, 8, 4);
  for (const nick of [-7, -3, 2, 6]) r(g, '#d9a520', x + nick, y - 9, 1, 1);
  paintLump(g, '#c8641e', x, y - 12, 6, 3);
  r(g, '#e8802a', x - 4, y - 14, 3, 1);
  r(g, '#e8802a', x + 1, y - 15, 2, 1);
  r(g, FALL_STRAW[1], x + 1, y - 12, 7, 3);
  r(g, FALL_STRAW[2], x + 2, y - 12, 5, 1);
  r(g, '#a04e17', x + 1, y - 14, 1, 2);
  r(g, '#d9a520', x + 7, y - 13, 1, 3);
  paintLump(g, FEAST_CREAM, x - 3, y - 14, 3, 2);
  r(g, FEAST_LINEN, x - 4, y - 13, 5, 1);
  r(g, FEAST_CREAM, x - 3, y - 17, 2, 1);
  const rise = Math.floor(t / 90) % 5;
  r(g, 'rgba(255,255,255,.4)', x - 1 + (Math.floor(rise / 2) % 2), y - 19 - rise, 1, 2);
  r(g, 'rgba(255,255,255,.28)', x + 4 - (Math.floor(rise / 2) % 2), y - 19 - ((rise + 2) % 5), 1, 2);
}

// A wicker horn lying on the floor, curled to a point on the left, with fruit tumbling out of its
// mouth along the floor to the right: tall at the mouth and low at the end.
function paintCornucopia(g, x, y) {
  const left = x - 19;
  const lift = (i) => (i < 12 ? Math.round(((12 - i) * (12 - i)) / 14) : 0);
  const thick = (i) => 3 + Math.floor(i * 0.5);
  r(g, SHADOW, left, y - 2, 38, 2);
  for (let i = 0; i < 24; i++) {
    const bottom = y - 3 - lift(i);
    const topRow = bottom - thick(i);
    r(g, FALL_STRAW[1], left + i, topRow, 1, thick(i));
    for (let k = 0; k < thick(i); k++) {
      if ((i + k) % 4 < 2) r(g, FALL_STRAW[0], left + i, topRow + k, 1, 1);
    }
    r(g, FALL_STRAW[2], left + i, topRow, 1, 1);
    r(g, shade(FALL_STRAW[0], -0.25), left + i, bottom - 1, 1, 1);
  }
  r(g, FALL_STRAW[0], left, y - 16, 1, 2);
  r(g, FALL_STRAW[2], left + 1, y - 16, 1, 1);
  for (const band of [8, 16]) {
    r(g, FALL_WOOD[1], left + band, y - 3 - lift(band) - thick(band), 1, thick(band));
  }
  paintLump(g, FALL_STRAW[0], x + 5, y - 11, 3, 7);
  paintLump(g, FALL_STRAW[2], x + 4, y - 11, 3, 7);
  paintLump(g, FALL_WOOD[0], x + 5, y - 11, 2, 5);
  // Corn lying behind the rest, husk to the right.
  r(g, '#d9a520', x + 10, y - 8, 6, 3);
  r(g, '#eab34a', x + 10, y - 8, 6, 1);
  r(g, '#7f8c3a', x + 16, y - 8, 3, 3);
  r(g, '#a3a84a', x + 17, y - 9, 1, 1);
  feastBall(g, '#c8641e', x + 4, y - 15, 3, 3);
  feastApple(g, '#b33a1f', x + 8, y - 12);
  for (const [gx, gy] of [[7, 5], [10, 6], [9, 9]]) feastBall(g, '#7a3a66', x + gx, y - gy, 2, 2);
  feastBall(g, '#e8802a', x + 12, y - 4, 3, 3);
  r(g, '#c8641e', x + 12, y - 6, 1, 4);
  feastApple(g, '#a3a84a', x + 16, y - 3);
  r(g, '#d9a520', x + 17, y - 3, 2, 2);
}

// The horn seen from its mouth: a ring of weave around a dark hollow, with fruit heaped over the top.
function paintCornucopiaEnd(g, x, y) {
  r(g, SHADOW, x - 9, y - 2, 18, 2);
  paintLump(g, FALL_STRAW[0], x, y - 10, 9, 8);
  paintLump(g, FALL_STRAW[1], x - 1, y - 11, 8, 7);
  paintLump(g, FALL_STRAW[2], x - 2, y - 12, 5, 4);
  for (let k = 0; k < 6; k++) r(g, FALL_STRAW[0], x - 7 + k * 3, y - 4 - (k % 2) * 8, 1, 2);
  paintLump(g, FALL_WOOD[0], x, y - 10, 6, 6);
  r(g, '#7f8c3a', x - 5, y - 18, 5, 3);
  r(g, '#d9a520', x - 1, y - 19, 4, 4);
  r(g, '#eab34a', x + 1, y - 21, 4, 3);
  feastBall(g, '#7a3a66', x - 5, y - 13, 2, 2);
  feastBall(g, '#7a3a66', x - 3, y - 10, 2, 2);
  feastBall(g, '#7a3a66', x - 6, y - 9, 2, 2);
  feastApple(g, '#b33a1f', x + 2, y - 11);
  feastApple(g, '#d4502a', x + 6, y - 8);
  feastBall(g, '#c8641e', x - 3, y - 5, 3, 2);
}

// A bushel basket heaped with apples, a hoop handle over it and one that rolled onto the floor.
function paintAppleBasket(g, x, y) {
  r(g, SHADOW, x - 9, y - 2, 20, 2);
  r(g, FALL_STRAW[0], x - 7, y - 17, 2, 7);
  r(g, FALL_STRAW[0], x + 5, y - 17, 2, 7);
  r(g, FALL_STRAW[0], x - 6, y - 19, 12, 2);
  r(g, FALL_STRAW[2], x - 6, y - 19, 11, 1);
  for (let k = 0; k < 10; k++) {
    const half = 8 - Math.floor(k / 4);
    const row = y - 12 + k;
    r(g, FALL_STRAW[1], x - half, row, half * 2, 1);
    r(g, FALL_STRAW[2], x - half, row, 2, 1);
    r(g, shade(FALL_STRAW[0], -0.1), x + half - 2, row, 2, 1);
    for (let i = 2; i < half * 2 - 2; i++) {
      if ((i + k) % 3 === 0) r(g, FALL_STRAW[0], x - half + i, row, 1, 1);
    }
  }
  r(g, FALL_STRAW[2], x - 8, y - 13, 16, 1);
  r(g, FALL_WOOD[2], x - 9, y - 12, 18, 2);
  r(g, FALL_WOOD[3], x - 9, y - 12, 18, 1);
  r(g, FALL_WOOD[1], x - 5, y - 2, 11, 1);
  feastApple(g, '#b33a1f', x - 4, y - 17);
  feastApple(g, '#d4502a', x + 3, y - 17);
  feastApple(g, '#b33a1f', x - 6, y - 14);
  feastBall(g, '#a3a84a', x, y - 14, 3, 3);
  r(g, '#d9a520', x + 1, y - 14, 2, 2);
  r(g, '#f6d98a', x - 2, y - 16, 1, 1);
  r(g, FALL_WOOD[0], x, y - 18, 1, 2);
  feastApple(g, '#d4502a', x + 6, y - 14);
  feastApple(g, '#8c2a1e', x + 10, y - 4);
  r(g, '#7f8c3a', x + 11, y - 8, 2, 1);
}

// An oak barrel on its side in a cradle, with a tap, a mug underneath and a drip that falls into it.
function paintCiderBarrel(g, x, y, t) {
  const left = x - 12;
  r(g, SHADOW, left - 1, y - 2, 26, 2);
  for (const cx of [x - 8, x + 7]) {
    r(g, FALL_WOOD[2], cx - 5, y - 3, 10, 3);
    r(g, FALL_WOOD[0], cx - 5, y - 1, 10, 1);
  }
  const centre = y - 16;
  const belly = (i) => 10 + Math.round(5 * (1 - ((i - 11.5) / 12) ** 2));
  for (let i = 0; i < 24; i++) {
    const high = belly(i);
    const top = centre - Math.ceil(high / 2);
    r(g, FALL_WOOD[3], left + i, top, 1, high);
    r(g, shade(FALL_WOOD[3], 0.2), left + i, top, 1, 2);
    r(g, shade(FALL_WOOD[3], 0.1), left + i, top + 2, 1, 2);
    r(g, FALL_WOOD[2], left + i, top + high - 4, 1, 2);
    r(g, FALL_WOOD[1], left + i, top + high - 2, 1, 2);
  }
  for (const seam of [-3, 0, 3]) r(g, shade(FALL_WOOD[2], -0.2), left + 1, centre + seam, 22, 1);
  r(g, shade(FALL_WOOD[3], 0.2), left, centre - 3, 1, 6);
  for (const hoop of [3, 7, 15, 19]) {
    const high = belly(hoop);
    const top = centre - Math.ceil(high / 2);
    r(g, FALL_WOOD[0], left + hoop, top, 2, high);
    r(g, FALL_WOOD[1], left + hoop, top + 1, 1, high - 2);
  }
  for (const cx of [x - 8, x + 7]) {
    r(g, FALL_WOOD[1], cx - 4, y - 9, 8, 3);
    r(g, FALL_WOOD[3], cx - 4, y - 9, 8, 1);
    r(g, FALL_WOOD[0], cx - 3, y - 6, 6, 4);
  }
  r(g, FALL_STRAW[1], x + 3, y - 24, 3, 1);
  r(g, '#eab34a', x - 2, y - 13, 3, 1);
  r(g, '#d9a520', x, y - 12, 1, 5);
  r(g, '#d9a520', x - 1, y - 8, 2, 1);
  const drip = Math.floor(t / 180) % 3;
  r(g, '#f0a03c', x, y - 6 + drip, 1, 1);
  r(g, '#c8641e', x - 2, y - 4, 4, 4);
  r(g, '#f0a03c', x - 2, y - 4, 4, 1);
  r(g, '#a04e17', x + 2, y - 3, 1, 2);
  feastApple(g, '#b33a1f', x - 6, y - 26);
  r(g, '#7f8c3a', x - 4, y - 28, 3, 1);
}

// The barrel from its round end, with two dark hoops around the head and a mug beside the cradle.
function paintCiderBarrelEnd(g, x, y) {
  r(g, SHADOW, x - 12, y - 2, 25, 2);
  r(g, FALL_WOOD[1], x - 9, y - 3, 18, 3);
  r(g, FALL_WOOD[0], x - 9, y - 1, 18, 1);
  r(g, FALL_WOOD[2], x - 9, y - 3, 18, 1);
  paintLump(g, FALL_WOOD[0], x, y - 16, 9, 9);
  paintLump(g, FALL_WOOD[3], x, y - 16, 8, 8);
  paintLump(g, shade(FALL_WOOD[3], 0.16), x - 2, y - 18, 4, 5);
  for (const at of [-4, 0, 4]) r(g, shade(FALL_WOOD[2], -0.08), x + at, y - 22, 1, 12);
  paintLump(g, FALL_WOOD[0], x, y - 16, 6, 6);
  paintLump(g, FALL_WOOD[3], x, y - 16, 5, 5);
  paintLump(g, shade(FALL_WOOD[3], 0.16), x - 1, y - 17, 3, 3);
  for (const at of [-3, 0, 3]) r(g, FALL_WOOD[2], x + at, y - 20, 1, 8);
  r(g, FALL_WOOD[0], x - 1, y - 17, 3, 3);
  r(g, FALL_STRAW[1], x, y - 16, 1, 1);
  r(g, '#c8641e', x + 5, y - 7, 4, 4);
  r(g, '#f0a03c', x + 5, y - 7, 4, 1);
  r(g, '#a04e17', x + 9, y - 6, 1, 2);
}

// A bit of straw with two orange pumpkins, a white one and a striped gourd with a crooked neck.
function paintGourds(g, x, y, t, wide) {
  const crowded = wide < 24;
  const nudge = crowded ? 4 : 0;
  r(g, SHADOW, x - 12 + nudge, y - 2, 24 - nudge * 2, 2);
  for (let i = 0; i < 12; i++) {
    const at = x - 11 + nudge + i * (crowded ? 1 : 2) + (i % 2);
    r(g, FALL_STRAW[i % 3], at, y - 3 - (i % 3), 1, 3 + (i % 3));
    r(g, FALL_STRAW[(i + 1) % 3], at + (i % 2 ? -1 : 1), y - 2, 1, 2);
  }
  const big = x - 6 + nudge;
  feastBall(g, '#e8802a', big, y - 9, crowded ? 5 : 6, crowded ? 5 : 6);
  for (const rib of [-3, 0, 3]) r(g, '#c8641e', big + rib, y - 14, 1, 10);
  r(g, FALL_WOOD[1], big - 1, y - 17, 3, 3);
  r(g, '#7f8c3a', big + 2, y - 16, 2, 1);
  const stripe = '#7f8c3a';
  const gourd = x + 8 - nudge;
  feastBall(g, '#d9a520', gourd, y - 4, 4, 3);
  for (const bar of [-2, 0, 2]) {
    const edge = Math.abs(bar) > 1 ? 1 : 0;
    r(g, stripe, gourd + bar, y - 7 + edge, 1, 7 - edge * 2);
  }
  r(g, '#d9a520', gourd - 1, y - 11, 2, 5);
  r(g, stripe, gourd, y - 11, 1, 5);
  r(g, '#d9a520', gourd, y - 12, 3, 2);
  r(g, stripe, gourd + 1, y - 12, 1, 1);
  r(g, '#d9a520', gourd + 2, y - 11, 1, 2);
  if (!crowded) {
    feastBall(g, '#e8802a', x + 3, y - 8, 4, 3);
    r(g, '#c8641e', x + 2, y - 10, 1, 5);
    r(g, '#c8641e', x + 5, y - 10, 1, 5);
    r(g, FALL_WOOD[1], x + 3, y - 12, 2, 2);
  }
  feastBall(g, FEAST_CREAM, x - 1 + nudge, y - 4, 4, 3);
  for (const rib of [-2, 1]) r(g, FEAST_LINEN, x - 1 + nudge + rib, y - 6, 1, 5);
  r(g, '#7f8c3a', x - 1 + nudge, y - 8, 2, 1);
}

// The farmyard. The crow hops and looks round, the turkey bobs and the fan breathes, and all of it
// stands still for someone who asked for less motion.

const FARM_INK = '#2b2430';

// A small round pumpkin or gourd, w by h, with its top left at x and y.
function paintFarmGourd(g, x, y, w, h, body, stem) {
  const mid = x + Math.floor(w / 2);
  r(g, shade(body, -0.22), x, y + 2, w, h - 2);
  r(g, body, x + 1, y + 1, w - 2, h - 1);
  r(g, shade(body, -0.22), x + 2, y, w - 4, 1);
  r(g, body, x + 2, y + 1, w - 4, 1);
  r(g, shade(body, -0.2), mid, y + 1, 1, h - 2);
  r(g, shade(body, 0.28), x + 1, y + 2, 1, Math.max(1, h - 4));
  r(g, stem, mid, y - 1, 1, 2);
}

function paintScarecrow(g, x, y, t) {
  const burlap = FALL_STRAW[1];
  const plaid = ['#b33a1f', '#8c2a1e', '#d4502a'];
  const step = Math.floor(t / 600) % 6;
  const hop = step === 4 ? 2 : 0;
  const turned = step === 2 || step === 3;
  r(g, SHADOW, x - 8, y - 2, 16, 2);
  // Straw legs, ragged at the bottom, on a post.
  r(g, FALL_WOOD[1], x - 1, y - 18, 2, 18);
  r(g, FALL_WOOD[2], x - 1, y - 18, 1, 18);
  for (const side of [-1, 1]) {
    const left = side < 0 ? x - 6 : x + 2;
    r(g, FALL_STRAW[0], left, y - 18, 4, 14);
    r(g, FALL_STRAW[1], left, y - 18, 3, 14);
    r(g, FALL_STRAW[2], left, y - 17, 1, 9);
    r(g, '#5a2a4c', left, y - 18, 4, 2);
    r(g, FALL_STRAW[2], left - 1, y - 5, 2, 4);
    r(g, FALL_STRAW[1], left + 1, y - 4, 2, 3);
    r(g, FALL_STRAW[0], left + 3, y - 4, 2, 3);
    r(g, FALL_STRAW[2], left + 2, y - 2, 1, 2);
  }
  // The crossbar, under a red flannel shirt with straw coming out of the cuffs.
  r(g, FALL_WOOD[1], x - 12, y - 28, 24, 2);
  r(g, plaid[0], x - 5, y - 31, 10, 15);
  r(g, plaid[0], x - 12, y - 30, 8, 4);
  r(g, plaid[0], x + 4, y - 30, 8, 4);
  r(g, plaid[2], x - 5, y - 31, 1, 15);
  r(g, plaid[2], x - 12, y - 30, 8, 1);
  r(g, plaid[2], x + 4, y - 30, 8, 1);
  for (let i = 0; i < 4; i++) r(g, plaid[1], x - 3 + i * 3, y - 30, 1, 14);
  for (let i = 0; i < 4; i++) r(g, plaid[1], x - 5, y - 28 + i * 4, 10, 1);
  r(g, plaid[1], x - 12, y - 28, 8, 1);
  r(g, plaid[1], x + 4, y - 28, 8, 1);
  r(g, '#f6d98a', x - 4, y - 25, 1, 1);
  r(g, '#f6d98a', x + 2, y - 21, 1, 1);
  r(g, FALL_WOOD[0], x - 1, y - 23, 2, 1);
  r(g, '#eab34a', x, y - 23, 1, 1);
  r(g, '#eab34a', x, y - 19, 1, 1);
  // Straw tufts at the cuffs.
  for (const side of [-1, 1]) {
    const edge = side < 0 ? x - 12 : x + 9;
    r(g, FALL_STRAW[1], edge, y - 31, 3, 2);
    r(g, FALL_STRAW[2], edge, y - 32, 1, 2);
    r(g, FALL_STRAW[2], edge + 2, y - 32, 1, 1);
    r(g, FALL_STRAW[0], edge + 1, y - 27, 2, 2);
  }
  // A burlap head with button eyes and a stitched smile.
  r(g, FALL_STRAW[0], x - 5, y - 40, 10, 10);
  r(g, burlap, x - 5, y - 40, 9, 9);
  r(g, FALL_STRAW[2], x - 4, y - 39, 2, 3);
  r(g, FALL_STRAW[1], x - 6, y - 38, 1, 6);
  r(g, FALL_STRAW[0], x + 5, y - 38, 1, 6);
  r(g, FALL_STRAW[2], x - 8, y - 36, 3, 1);
  r(g, FALL_STRAW[2], x + 5, y - 35, 3, 1);
  r(g, FALL_WOOD[0], x - 4, y - 37, 3, 3);
  r(g, FALL_WOOD[0], x + 1, y - 37, 3, 3);
  r(g, '#8a5c37', x - 3, y - 36, 1, 1);
  r(g, '#8a5c37', x + 2, y - 36, 1, 1);
  r(g, '#f4f1e8', x - 4, y - 37, 1, 1);
  r(g, '#f4f1e8', x + 1, y - 37, 1, 1);
  r(g, '#d4502a', x - 5, y - 34, 2, 1);
  r(g, '#d4502a', x + 3, y - 34, 2, 1);
  for (const [dx, dy] of [[-3, -33], [-2, -32], [-1, -32], [1, -32], [2, -32], [3, -33]]) {
    r(g, FALL_WOOD[0], x + dx, y + dy, 1, 1);
  }
  // A floppy straw hat with an autumn leaf in its band.
  r(g, FALL_STRAW[0], x - 10, y - 42, 20, 2);
  r(g, FALL_STRAW[1], x - 9, y - 43, 18, 2);
  r(g, FALL_STRAW[0], x - 12, y - 40, 4, 2);
  r(g, FALL_STRAW[0], x + 8, y - 40, 4, 2);
  r(g, FALL_STRAW[2], x - 9, y - 43, 6, 1);
  r(g, FALL_STRAW[1], x - 5, y - 47, 10, 5);
  r(g, FALL_STRAW[1], x - 4, y - 48, 8, 1);
  r(g, FALL_STRAW[2], x - 4, y - 47, 2, 3);
  r(g, FALL_STRAW[0], x + 3, y - 47, 2, 5);
  r(g, '#8c2a1e', x - 5, y - 44, 10, 2);
  r(g, '#e8802a', x + 2, y - 46, 3, 3);
  r(g, '#f0a03c', x + 2, y - 46, 1, 1);
  // The crow stands on the right arm, then hops and looks round.
  const top = y - 30 - hop;
  r(g, FARM_INK, x + 7, top - 5, 5, 5);
  r(g, FARM_INK, x + 8, top - 6, 3, 1);
  r(g, '#4a4052', x + 8, top - 5, 2, 3);
  r(g, FARM_INK, x + 11, top - 3, 1, 3);
  if (turned) {
    r(g, FARM_INK, x + 10, top - 8, 3, 3);
    r(g, '#d9a520', x + 13, top - 7, 2, 1);
    r(g, '#f4f1e8', x + 11, top - 7, 1, 1);
  } else {
    r(g, FARM_INK, x + 5, top - 8, 3, 3);
    r(g, '#d9a520', x + 3, top - 7, 2, 1);
    r(g, '#f4f1e8', x + 6, top - 7, 1, 1);
  }
  r(g, FARM_INK, x + 7, top - 6, 2, 2);
  if (!hop) {
    r(g, FALL_WOOD[0], x + 8, top, 1, 1);
    r(g, FALL_WOOD[0], x + 10, top, 1, 1);
  }
}

// A bale tied with two bands of twine, straw sticking out at its edges.
function paintHayBale(g, x, y) {
  const [dark, mid, light] = FALL_STRAW;
  r(g, SHADOW, x - 12, y - 2, 24, 2);
  r(g, dark, x - 11, y - 14, 22, 12);
  r(g, mid, x - 11, y - 14, 21, 11);
  r(g, light, x - 11, y - 17, 22, 3);
  r(g, mid, x - 10, y - 17, 17, 1);
  r(g, light, x - 10, y - 17, 9, 1);
  r(g, dark, x - 11, y - 14, 22, 1);
  r(g, dark, x - 11, y - 3, 22, 1);
  for (let i = 0; i < 18; i++) {
    const left = x - 10 + ((i * 7) % 18);
    const top = y - 13 + ((i * 5) % 9);
    r(g, i % 2 ? dark : light, left, top, 3 + (i % 3), 1);
  }
  for (let i = 0; i < 6; i++) r(g, i % 2 ? dark : light, x - 9 + i * 4, y - 17 + (i % 2), 2, 1);
  // Twine, with a small knot in the front of each band.
  for (const band of [-6, 4]) {
    r(g, FALL_WOOD[1], x + band, y - 17, 2, 15);
    r(g, FALL_WOOD[2], x + band, y - 17, 1, 15);
    r(g, FALL_WOOD[0], x + band + 1, y - 14, 1, 11);
  }
  r(g, FALL_WOOD[0], x - 7, y - 9, 4, 1);
  r(g, FALL_WOOD[2], x + 3, y - 9, 4, 1);
  // Stray stalks at the edges and top.
  r(g, light, x - 12, y - 12, 1, 3);
  r(g, mid, x - 12, y - 6, 1, 2);
  r(g, light, x + 11, y - 11, 1, 3);
  r(g, mid, x + 11, y - 5, 1, 2);
  r(g, light, x - 3, y - 18, 1, 1);
  r(g, light, x + 8, y - 18, 1, 1);
  r(g, mid, x + 1, y - 18, 1, 1);
}

// The cut end of a bale, with the twine going round it.
function paintHayEnd(g, x, y, t, wide) {
  const [dark, mid, light] = FALL_STRAW;
  const half = Math.min(6, wide / 2 - 1);
  r(g, SHADOW, x - half - 1, y - 2, half * 2 + 2, 2);
  r(g, dark, x - half, y - 14, half * 2, 12);
  r(g, mid, x - half + 1, y - 13, half * 2 - 2, 10);
  r(g, light, x - half, y - 17, half * 2, 3);
  r(g, mid, x - half + 1, y - 17, half * 2 - 2, 1);
  for (let i = 0; i < 4; i++) {
    r(g, i % 2 ? light : dark, x - half + 2 + i, y - 12 + i, half * 2 - 4 - i * 2, 1);
    r(g, i % 2 ? dark : light, x - half + 2 + i, y - 4 - i, half * 2 - 4 - i * 2, 1);
  }
  r(g, FALL_WOOD[1], x - half, y - 17, half * 2, 1);
  r(g, FALL_WOOD[1], x - half, y - 9, half * 2, 2);
  r(g, light, x - half - 1, y - 11, 1, 3);
  r(g, mid, x + half, y - 6, 1, 3);
  r(g, light, x - 2, y - 18, 1, 1);
  r(g, mid, x + 2, y - 18, 1, 1);
}

// A plump turkey seen from the front, nose in the air. The fan breathes and the head bobs.
function paintTurkey(g, x, y, t) {
  const breath = Math.floor(t / 700) % 2;
  const bob = Math.floor(t / 450) % 2;
  const a = 12 - breath;
  const b = 16 - breath;
  const fan = ['#f6d98a', '#d9a520', '#e8802a', '#c8641e', '#6b4628'];
  const edges = [1, 0.88, 0.74, 0.58, 0.4];
  r(g, SHADOW, x - 9, y - 2, 18, 2);
  // The fan: bands of colour, widest at the tips.
  const base = y - 13;
  for (let dy = 0; dy <= b; dy++) {
    edges.forEach((edge, k) => {
      const inside = edge * edge - (dy / b) * (dy / b);
      if (inside <= 0) return;
      const half = Math.round(a * Math.sqrt(inside));
      if (half > 0) r(g, fan[k], x - half, base - dy, half * 2, 1);
    });
  }
  for (let i = -3; i <= 3; i++) {
    for (let dy = 5; dy <= b - 1; dy++) {
      const reach = Math.round((i * (dy + 2)) / 3.2);
      if (reach * reach * b * b + dy * dy * a * a < 0.8 * a * a * b * b) r(g, i === 0 ? fan[4] : shade(fan[4], 0.08), x + reach, base - dy, 1, 1);
    }
  }
  // Plump body, chest out, with a beard.
  const brown = FALL_WOOD[1];
  r(g, brown, x - 4, y - 17, 8, 1);
  r(g, brown, x - 6, y - 16, 12, 1);
  r(g, brown, x - 7, y - 15, 14, 9);
  r(g, FALL_WOOD[0], x - 6, y - 6, 12, 1);
  r(g, FALL_WOOD[0], x - 4, y - 5, 8, 1);
  r(g, FALL_WOOD[2], x - 5, y - 15, 9, 9);
  r(g, FALL_WOOD[3], x - 4, y - 15, 5, 3);
  r(g, FALL_WOOD[3], x - 4, y - 12, 6, 5);
  for (let row = 0; row < 3; row++) {
    for (let col = 0; col < 3; col++) r(g, FALL_WOOD[2], x - 4 + col * 3 + (row % 2), y - 12 + row * 2, 2, 1);
  }
  r(g, '#f6d98a', x - 3, y - 15, 2, 1);
  r(g, FALL_WOOD[0], x, y - 14, 1, 4);
  r(g, FALL_WOOD[0], x - 1, y - 11, 3, 1);
  // Wings tucked in at the sides, tipped with russet.
  for (const side of [-1, 1]) {
    const left = side < 0 ? x - 9 : x + 6;
    r(g, FALL_WOOD[0], left, y - 15, 3, 9);
    r(g, FALL_WOOD[1], left + (side < 0 ? 1 : 0), y - 15, 2, 8);
    r(g, '#c8641e', left, y - 7, 3, 1);
    r(g, '#e8802a', left + (side < 0 ? 0 : 1), y - 8, 2, 1);
  }
  // Legs and big feet.
  for (const side of [-1, 1]) {
    const left = x + side * 4 - 1;
    r(g, '#a8843f', left, y - 5, 2, 4);
    r(g, '#d9a520', left, y - 5, 1, 3);
    r(g, '#d9a520', left - 2, y - 2, 6, 1);
    r(g, '#a8843f', left - 2, y - 1, 6, 1);
    r(g, '#d9a520', left - 3, y - 1, 1, 1);
    r(g, '#d9a520', left + 4, y - 1, 1, 1);
  }
  // The head, held up high, with a heavy-lidded look down its beak.
  const head = y - 27 + bob;
  const skin = '#d99a8c';
  r(g, skin, x - 1, head + 6, 3, 5 - bob);
  r(g, shade(skin, -0.16), x + 1, head + 6, 1, 5 - bob);
  r(g, skin, x - 3, head + 1, 6, 6);
  r(g, skin, x - 2, head, 4, 1);
  r(g, shade(skin, 0.18), x - 3, head + 1, 1, 4);
  r(g, shade(skin, -0.16), x + 2, head + 2, 1, 4);
  r(g, '#d4502a', x - 1, head - 2, 2, 3);
  r(g, '#8c2a1e', x, head - 2, 1, 2);
  r(g, '#d4502a', x - 1, head + 7, 3, 4);
  r(g, '#8c2a1e', x, head + 8, 2, 3);
  r(g, '#eab34a', x - 2, head + 5, 4, 2);
  r(g, '#d9a520', x - 1, head + 6, 3, 1);
  r(g, '#1b2030', x - 2, head + 3, 1, 1);
  r(g, '#1b2030', x + 1, head + 3, 1, 1);
  r(g, '#7a3a66', x - 3, head + 2, 3, 1);
  r(g, '#7a3a66', x + 1, head + 2, 2, 1);
}

// A wreath of turning leaves, hung on a slim stand the way the dartboard is.
const FARM_RING = ['#b33a1f', '#e8802a', '#d9a520', '#7f8c3a', '#d4502a', '#f0a03c', '#8c2a1e', '#c8641e'];

function paintWreath(g, x, y) {
  const cy = y - 26;
  const inRing = (dx, dy) => {
    const d2 = dx * dx + dy * dy;
    return d2 <= 110 && d2 >= 28;
  };
  r(g, SHADOW, x - 6, y - 2, 12, 2);
  r(g, FALL_WOOD[0], x - 5, y - 3, 10, 2);
  r(g, FALL_WOOD[1], x - 4, y - 3, 8, 1);
  r(g, FALL_WOOD[1], x - 1, y - 15, 2, 12);
  r(g, FALL_WOOD[2], x - 1, y - 15, 1, 12);
  for (let dy = -10; dy <= 10; dy++) {
    let runStart = 0;
    let runColor = null;
    for (let dx = -10; dx <= 11; dx++) {
      let color = null;
      if (dx <= 10 && inRing(dx, dy)) {
        const sector = Math.floor(((Math.atan2(dy, dx) + Math.PI) / (Math.PI * 2)) * 8) % 8;
        color = FARM_RING[sector];
        if (!inRing(dx - 1, dy - 1) && (dx < 0 || dy < 0)) color = shade(color, 0.2);
        else if (!inRing(dx + 1, dy + 1)) color = shade(color, -0.22);
        else if ((dx * 3 + dy * 5) % 7 === 0) color = shade(color, -0.12);
      }
      if (color !== runColor) {
        if (runColor) r(g, runColor, x + runStart, cy + dy, dx - runStart, 1);
        runStart = dx;
        runColor = color;
      }
    }
  }
  // Gourds, berries and a burlap bow.
  paintFarmGourd(g, x - 10, cy + 3, 5, 5, '#e8802a', '#5f6b2e');
  paintFarmGourd(g, x + 6, cy + 4, 4, 4, '#d9a520', '#5f6b2e');
  for (const [dx, dy] of [[5, -9], [7, -8], [6, -11], [-8, -8]]) {
    r(g, '#5a2a4c', x + dx, cy + dy, 2, 2);
    r(g, '#7a3a66', x + dx, cy + dy, 1, 1);
  }
  r(g, '#7f8c3a', x - 9, cy - 4, 2, 2);
  r(g, '#a3a84a', x - 9, cy - 4, 1, 1);
  r(g, FALL_STRAW[0], x - 4, y - 13, 2, 4);
  r(g, FALL_STRAW[0], x + 2, y - 13, 2, 4);
  r(g, FALL_STRAW[1], x - 6, y - 20, 5, 5);
  r(g, FALL_STRAW[1], x + 1, y - 20, 5, 5);
  r(g, FALL_STRAW[2], x - 6, y - 20, 5, 1);
  r(g, FALL_STRAW[2], x + 1, y - 20, 5, 1);
  r(g, FALL_STRAW[0], x - 6, y - 16, 5, 1);
  r(g, FALL_STRAW[0], x + 1, y - 16, 5, 1);
  r(g, FALL_STRAW[0], x - 1, y - 19, 2, 4);
  r(g, FALL_STRAW[2], x - 1, y - 19, 1, 1);
}

// Pumpkins and leaves heaped in a wooden barrow: wheel at the left, handles at the right.
function paintWheelbarrow(g, x, y) {
  r(g, SHADOW, x - 12, y - 2, 24, 2);
  paintFarmGourd(g, x - 9, y - 22, 7, 7, '#d4502a', '#5f6b2e');
  paintFarmGourd(g, x - 3, y - 23, 8, 8, '#e8802a', '#5f6b2e');
  paintFarmGourd(g, x + 4, y - 21, 6, 6, '#f0a03c', '#7f8c3a');
  r(g, '#b33a1f', x - 6, y - 19, 4, 3);
  r(g, '#d9a520', x + 2, y - 19, 3, 3);
  r(g, '#8c2a1e', x - 9, y - 17, 3, 2);
  r(g, '#e8802a', x + 8, y - 19, 3, 3);
  r(g, '#f6d98a', x + 8, y - 19, 1, 1);
  r(g, '#7f8c3a', x - 1, y - 17, 3, 2);
  // Handles to the right, one nearer than the other.
  r(g, FALL_WOOD[0], x + 8, y - 17, 4, 1);
  r(g, FALL_WOOD[2], x + 7, y - 16, 4, 1);
  r(g, FALL_WOOD[1], x + 6, y - 14, 6, 2);
  r(g, FALL_WOOD[3], x + 6, y - 14, 6, 1);
  r(g, FALL_STRAW[1], x + 10, y - 14, 2, 2);
  // The tray, its boards narrowing to the bottom.
  for (let i = 0; i < 9; i++) {
    const inset = Math.floor((i + 1) / 2);
    const left = x - 10 + inset;
    const wide = 20 - inset * 2;
    r(g, i % 3 === 2 ? FALL_WOOD[1] : FALL_WOOD[2], left, y - 16 + i, wide, 1);
    r(g, FALL_WOOD[3], left, y - 16 + i, 1, 1);
    r(g, FALL_WOOD[0], left + wide - 1, y - 16 + i, 1, 1);
  }
  r(g, FALL_WOOD[3], x - 10, y - 17, 20, 1);
  r(g, FALL_WOOD[0], x - 10, y - 16, 20, 1);
  r(g, FALL_WOOD[2], x - 10, y - 16, 19, 1);
  r(g, FALL_WOOD[1], x - 5, y - 7, 11, 1);
  // A leaf caught on the front.
  r(g, '#e8802a', x - 11, y - 14, 2, 2);
  r(g, '#b33a1f', x - 10, y - 12, 2, 1);
  // Rear leg, front fork and the wheel.
  r(g, FALL_WOOD[1], x + 4, y - 8, 2, 6);
  r(g, FALL_WOOD[0], x + 3, y - 3, 4, 1);
  r(g, FALL_WOOD[1], x - 6, y - 9, 2, 5);
  r(g, FALL_WOOD[0], x - 12, y - 9, 8, 8);
  r(g, FALL_WOOD[0], x - 11, y - 10, 6, 10);
  r(g, FALL_WOOD[2], x - 11, y - 8, 6, 6);
  r(g, FALL_WOOD[2], x - 10, y - 9, 4, 8);
  r(g, FALL_WOOD[3], x - 10, y - 8, 1, 3);
  r(g, '#c9a86a', x - 9, y - 6, 2, 2);
  r(g, FALL_WOOD[0], x - 9, y - 6, 1, 1);
}

// From the end: the wheel in front, the heap above it and the two handles coming at you.
function paintWheelbarrowEnd(g, x, y) {
  r(g, SHADOW, x - 7, y - 2, 14, 2);
  paintFarmGourd(g, x - 5, y - 23, 6, 7, '#e8802a', '#5f6b2e');
  paintFarmGourd(g, x + 0, y - 22, 5, 6, '#d4502a', '#5f6b2e');
  r(g, '#d9a520', x - 3, y - 18, 3, 3);
  r(g, '#7f8c3a', x + 1, y - 17, 3, 2);
  for (let i = 0; i < 7; i++) {
    r(g, i % 3 === 2 ? FALL_WOOD[1] : FALL_WOOD[2], x - 6 + Math.floor(i / 3), y - 16 + i, 12 - Math.floor(i / 3) * 2, 1);
  }
  r(g, FALL_WOOD[3], x - 6, y - 17, 12, 1);
  r(g, FALL_WOOD[0], x - 2, y - 9, 4, 9);
  r(g, FALL_WOOD[2], x - 1, y - 8, 2, 7);
  r(g, FALL_WOOD[3], x - 1, y - 7, 1, 3);
  r(g, FALL_WOOD[1], x - 7, y - 12, 2, 10);
  r(g, FALL_WOOD[1], x + 5, y - 12, 2, 10);
  r(g, FALL_WOOD[3], x - 7, y - 12, 1, 10);
  r(g, FALL_STRAW[1], x - 7, y - 3, 2, 2);
  r(g, FALL_STRAW[1], x + 5, y - 3, 2, 2);
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
  dining_chair: { paint: paintDiningChair, rise: 24 },
  bench: { paint: paintBench, side: endOf(20, '#a0703f', '#6b4628'), rise: 22 },
  throne: { paint: paintThrone, rise: 41 },
  end_table: { paint: paintEndTable, rise: 14 },
  dining_table: { paint: paintDiningTable, rise: 34 },
  picnic_table: { paint: paintPicnicTable, rise: 27 },
  bedroll: { paint: paintBedroll, side: endOf(6, '#5277ad', '#3f6a8c'), rise: 8 },
  bed: {
    paint: (g, x, y, t, wide, deep) => paintBed(g, x, y, wide, deep, BLANKET),
    side: (g, x, y, t, wide, deep) => paintBedEnd(g, x, y, wide, deep, BLANKET),
    rise: 35,
  },
  double_bed: {
    paint: (g, x, y, t, wide, deep) => paintBed(g, x, y, wide, deep, WIDE_BLANKET),
    side: (g, x, y, t, wide, deep) => paintBedEnd(g, x, y, wide, deep, WIDE_BLANKET),
    rise: 47,
  },
  bunk_bed: { paint: paintBunkBed, side: endOf(40, '#8a5c37', '#6b4628'), rise: 42 },
  pet_bed: { paint: paintPetBed, rise: 12 },
  dresser: { paint: paintDresser, side: endOf(22, '#8a5c37', '#6b4628'), rise: 24 },
  wardrobe: { paint: paintWardrobe, side: endOf(38, '#8a5c37', '#5a3a24'), rise: 40 },
  crate: { paint: paintCrate, rise: 16 },
  chest: { paint: paintChest, side: endOf(16, '#8a5c37', '#6b4628'), rise: 18 },
  storage_shelf: { paint: paintStorageShelf, side: endOf(30, '#7c8699', '#5a6178'), rise: 32 },
  safe: { paint: paintSafe, side: endOf(20, '#4a4f66', '#3b4158'), rise: 22 },
  lantern: { paint: paintLantern, rise: 20 },
  torch: { paint: paintTorch, rise: 31 },
  campfire: { paint: paintCampfire, rise: 27 },
  candelabra: { paint: paintCandelabra, rise: 31 },
  counter: { paint: paintCounter, side: endOf(16, '#c9c4b5', '#8a5c37'), rise: 21 },
  stove: { paint: paintStove, side: endOf(22, '#9aa3b8', '#c9c4b5'), rise: 25 },
  fridge: { paint: paintFridge, side: endOf(36, '#e8ecf4', '#c9d3df'), rise: 38 },
  kitchen_sink: { paint: paintKitchenSink, side: endOf(16, '#c9c4b5', '#8a5c37'), rise: 26 },
  coffee_machine: { paint: paintCoffeeMachine, rise: 28 },
  vending_machine: { paint: paintVendingMachine, side: endOf(38, '#c8342c', '#a82a24'), rise: 40 },
  toilet: { paint: paintToilet, rise: 24 },
  shower: { paint: paintShower, rise: 44 },
  mirror: { paint: paintMirror, rise: 36 },
  bathtub: { paint: paintBathtub, side: endOf(16, '#f4f7fb', '#e8ecf4'), rise: 23 },
  radio: { paint: paintRadio, rise: 20 },
  computer: { paint: paintComputer, side: endOf(25, '#c9c4b5', '#9aa3b8'), rise: 27 },
  television: { paint: paintTelevision, side: endOf(29, '#3b4158', '#20283e'), rise: 31 },
  telescope: { paint: paintTelescope, rise: 36 },
  dartboard: { paint: paintDartboard, rise: 30 },
  chess_table: { paint: paintChessTable, rise: 27 },
  arcade_cabinet: { paint: paintArcadeCabinet, side: endOf(46, '#3a3560', '#26233f'), rise: 48 },
  pinball: { paint: paintPinball, rise: 36 },
  billiards: { paint: paintBilliards, rise: 41 },
  guitar: { paint: paintGuitar, rise: 35 },
  drum_kit: { paint: paintDrumKit, rise: 25 },
  harp: { paint: paintHarp, side: edgeOf(38, '#ffd24d', '#d9a520'), rise: 40 },
  cactus: { paint: paintCactus, rise: 26 },
  flower_box: { paint: paintFlowerBox, side: endOf(10, '#3fb573', '#6b4628'), rise: 17 },
  hedge: { paint: paintHedge, side: endOf(18, '#3fb573', '#2f7d4b'), rise: 21 },
  boulder: { paint: paintBoulder, side: endOf(10, '#8d8a80', '#77746b'), rise: 14 },
  potted_tree: { paint: paintPottedTree, side: paintPottedTree, rise: 56 },
  coat_rack: { paint: paintCoatRack, rise: 37 },
  globe: { paint: paintGlobe, rise: 22 },
  framed_painting: { paint: paintFramedPainting, side: edgeOf(30, '#c9a86a', '#4a3120'), rise: 32 },
  grandfather_clock: { paint: paintGrandfatherClock, rise: 44 },
  sculpture: { paint: paintSculpture, rise: 33 },
  workbench: { paint: paintWorkbench, rise: 30 },
  tool_chest: { paint: paintToolChest, side: endOf(18, '#c8342c', '#a82a24'), rise: 20 },
  anvil: { paint: paintAnvil, side: endOf(12, '#6d7890', '#3b4158'), rise: 15 },
  sewing_table: { paint: paintSewingTable, rise: 28 },
  research_bench: { paint: paintResearchBench, rise: 32 },
  maple_tree: { paint: paintMapleTree, side: paintMapleTree, rise: 66 },
  birch_tree: { paint: paintBirchTree, side: paintBirchTree, rise: 66 },
  leaf_pile: { paint: paintLeafPile, side: paintLeafPile, rise: 16 },
  rake: { paint: paintRake, rise: 38 },
  mums: { paint: paintMums, rise: 26 },
  corn_stalks: { paint: paintCornStalks, rise: 44 },
  harvest_table: { paint: paintHarvestTable, side: paintHarvestTableEnd, rise: 36 },
  pumpkin_pie: { paint: paintPumpkinPie, rise: 22 },
  cornucopia: { paint: paintCornucopia, side: paintCornucopiaEnd, rise: 20 },
  apple_basket: { paint: paintAppleBasket, rise: 20 },
  cider_barrel: { paint: paintCiderBarrel, side: paintCiderBarrelEnd, rise: 30 },
  gourds: { paint: paintGourds, side: paintGourds, rise: 18 },
  scarecrow: { paint: paintScarecrow, rise: 48 },
  hay_bale: { paint: paintHayBale, side: paintHayEnd, rise: 18 },
  turkey: { paint: paintTurkey, rise: 30 },
  wreath: { paint: paintWreath, rise: 36 },
  wheelbarrow: { paint: paintWheelbarrow, side: paintWheelbarrowEnd, rise: 24 },
};

// Notes going up from whoever's making the music, drifting apart as they go.
function paintNotes(g, { x, y }, t) {
  for (const [i, color] of ['#ffd24d', '#7fd4ff', '#ff8fa8'].entries()) {
    const rise = (t / 34 + i * 16) % 48;
    const left = Math.round(x + (i - 1) * 7 + Math.sin(rise / 9 + i * 2) * 5);
    const top = Math.round(y - 26 - rise);
    g.globalAlpha = 1 - rise / 48;
    r(g, color, left, top + 5, 3, 2);
    r(g, color, left + 2, top, 1, 6);
    r(g, color, left + 2, top, 3, 1);
  }
  g.globalAlpha = 1;
}

// A bench, the keys going down in front of a bot at the piano, and the notes they make. A turned
// piano's keys are along its top, where this can't reach them, so it only gets the notes.
function paintPlaying(g, bot, t) {
  const x = Math.round(bot.x);
  const y = Math.round(bot.y);
  if (bot.facing === 'back') {
    r(g, '#3a3342', x - 10, y, 20, 3);
    r(g, '#1b1820', x - 9, y + 3, 2, 3);
    r(g, '#1b1820', x + 7, y + 3, 2, 3);
    const beat = Math.floor(t / 130);
    for (let i = 0; i < 4; i++) r(g, '#9aa3b8', x - 27 + ((beat + i) * (i * 2 + 5)) % 18 * 3, y - 21, 2, 6);
  }
  paintNotes(g, bot, t);
}

// What bots do with an item (I31 in docs/rooms.md). `spots` is `room_for` on the server, so every bot
// it lets use something has somewhere to be. A seat has them `on` it and anything else at its front.
// `pose` is how they are there, `hands` keeps theirs moving, and `busy` is painted over them.
const USES = {
  piano: { spots: 1, pose: 'sit', hands: true, busy: paintPlaying },
  jukebox: { spots: 1, pose: 'stand', busy: paintNotes },
  easel: { spots: 1, pose: 'stand', hands: true },
  game_table: { spots: 2, pose: 'sit', hands: true },
  aquarium: { spots: 2, pose: 'stand' },
  fireplace: { spots: 2, pose: 'stand' },
  stool: { spots: 1, pose: 'sit', on: true },
  armchair: { spots: 1, pose: 'sit', on: true },
  love_seat: { spots: 2, pose: 'sit', on: true },
  sofa: { spots: 3, pose: 'sit', on: true },
};

// Where an item stands, in the room's own pixels. `w` and `h` are cells as it's turned now, and
// `x` and `y` are its back left corner, as the server has it.
function standing({ id, item, x, y, w = 1, h = 1, facing = 0 }) {
  const wide = w * FLOOR_CELL;
  const deep = h * FLOOR_CELL;
  const left = x * FLOOR_CELL;
  const top = FLOOR_TOP + y * FLOOR_CELL;
  return { ...ITEMS[item], use: USES[item], id, facing, left, top, wide, deep, x: left + wide / 2, y: top + deep };
}

// Where bots go to use an item, one place for each that can. Its front is the side towards the room,
// or for one drawn from its end, the side it was turned to: right at facing 1, and left at 3.
function spotsOf({ use, side, facing, left, top, wide, deep, y }) {
  if (!use) return [];
  const { spots, pose, on, hands, busy } = use;
  const turned = Boolean(side) && facing % 2 === 1;
  const right = facing === 1;
  return Array.from({ length: spots }, (_, i) => {
    const along = (i + 0.5) / spots;
    const [ax, ay] = turned
      ? clearOf(right ? left + wide + 6 : left - 6, top + deep * along + 4)
      : clearOf(left + wide * along, y + 3);
    const drop = i % 2;
    if (!on) return { x: ax, y: ay, ax, ay, pose, drop, hands, busy, facing: turned ? (right ? 'left' : 'right') : 'back' };
    // Someone on a seat has their feet behind its front edge, and `over` keeps them painted on it.
    const seat = turned
      ? { x: left + wide / 2 + (right ? 3 : -3), y: top - 3 + (deep - 8) * along, facing: right ? 'right' : 'left' }
      : { x: left + 7 + (wide - 14) * along, y: y - 7, facing: 'front' };
    return { ...seat, ax, ay, pose, drop, over: y + 1 };
  });
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

// What stands against the back wall and in the corners, which nobody goes behind.
function paintBehind(g, hidden, t) {
  if (!hidden.includes('bar')) paintBar(g, t);
  if (!hidden.includes('plants')) {
    paintPlant(g, 470, 250);
    paintPlant(g, 14, 92);
  }
}

// What comes with a room and stands among the people in it, with where each is sorted by height.
function furnitureScene(g, hidden, t, lit, deskLit) {
  return [
    hidden.includes('desk') ? null : { y: 136, draw: () => paintDesk(g, t, deskLit) },
    { y: 150, draw: () => paintTable(g, lit) },
    hidden.includes('arcade') ? null : { y: 172, draw: () => paintArcade(g, t) },
    hidden.includes('couch') ? null : { y: 200, draw: () => paintCouch(g) },
    hidden.includes('beanbags') ? null : { y: 232, draw: () => paintBeanbags(g) },
  ].filter(Boolean);
}

// A whole room on a canvas of its own, drawn still, for choosing a template. `items` are as the
// server has them, and a word this page doesn't know is left out.
export function roomPicture(look, items = []) {
  const canvas = document.createElement('canvas');
  canvas.width = W;
  canvas.height = H;
  const g = canvas.getContext('2d');
  paintRoom(g, 12, look);
  paintBehind(g, look.hidden, 0);
  const placed = items.filter(({ item }) => Object.hasOwn(ITEMS, item)).map(standing);
  const scene = [
    ...furnitureScene(g, look.hidden, 0, [], []),
    ...placed.map((item) => ({ y: item.y, draw: () => paintItem(g, item, 0) })),
  ];
  scene.sort((a, b) => a.y - b.y);
  for (const each of scene) each.draw();
  return canvas;
}

const FURNITURE_ART = {
  couch: paintCouch,
  beanbags: paintBeanbags,
  plants: (g) => paintPlant(g, 470, 250),
  arcade: (g) => paintArcade(g, 0),
  table: (g) => paintTable(g, []),
  desk: (g) => paintDesk(g, 0, []),
  bar: (g) => paintBar(g, 0),
  rug: paintRug,
};

// What came with a room on a canvas of its own, drawn still, for the catalog.
export function furniturePicture(word) {
  const [, left, top, wide, deep] = FURNITURE.find(([each]) => each === word);
  const canvas = document.createElement('canvas');
  canvas.width = wide;
  canvas.height = deep;
  const g = canvas.getContext('2d');
  g.translate(-left, -top);
  FURNITURE_ART[word](g);
  return canvas;
}

// A piece of an empty room at midday, for choosing a paint or a flooring.
function surfacePicture(look, x, y, wide, deep) {
  const whole = document.createElement('canvas');
  whole.width = W;
  whole.height = H;
  paintRoom(whole.getContext('2d'), 12, { ...look, hidden: ['rug'] });
  const canvas = document.createElement('canvas');
  canvas.width = wide;
  canvas.height = deep;
  canvas.getContext('2d').drawImage(whole, x, y, wide, deep, 0, 0, wide, deep);
  return canvas;
}

export const wallPicture = (paint) => surfacePicture({ paint, flooring: 'oak' }, 108, 0, 40, 64);
export const floorPicture = (flooring) => surfacePicture({ paint: 'dusk', flooring }, 0, 72, 72, 48);

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
// typed reaches the drawing. The server says what everyone is wearing, here and on the maps, so a room
// and a map can't disagree. All this page decides is the art.
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
  sky: '#4f9dff',
  tangerine: '#ff8a3d',
  mint: '#3ddc97',
  coral: '#ff5d7a',
  lilac: '#b388ff',
  sun: '#ffd24d',
  aqua: '#2fd6d6',
  orchid: '#ff7ad9',
};
export const SKINS = {
  cream: '#f4d6b8',
  peach: '#f1c9a4',
  tan: '#d9a77c',
  caramel: '#b27a54',
  umber: '#8a5a3c',
  espresso: '#5e3a26',
};
export const HAIRS = {
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
export const LEGS = {
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

function botLook(worn = {}) {
  return {
    shell: pick(FABRICS, worn.shell, null),
    head: pick(HEADS, worn.head, HEADS.antenna),
    eyes: pick(EYES, worn.eyes, null),
    chest: pick(CHESTS, worn.chest, CHESTS.plain),
    hat: pick(HATS, worn.hat, HATS.none),
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
  // Nobody types on a sofa, so at an item its hands move only if that keeps them busy.
  const at = !walking && bot.spot;
  const typing = (at ? at.hands : sitting) && Math.floor(t / 170 + bot.phase * 5) % 2;
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

function personLook(worn = {}) {
  return {
    skin: pick(SKINS, worn.skin, SKINS.peach),
    hair: pick(HAIRS, worn.hair, HAIRS.brown),
    style: pick(STYLES, worn.style, STYLES.short),
    face: pick(FACES, worn.face, FACES.none),
    hat: pick(HATS, worn.hat, HATS.none),
    shirt: pick(FABRICS, worn.shirt, null),
    trousers: pick(LEGS, worn.trousers, LEGS.navy),
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

// Big enough for the tallest hat.
const FIGURE_WIDE = 26;
const FIGURE_TALL = 36;

// A figure on a canvas of its own, drawn still, for the wardrobe. `figure` is { kind, color, worn }.
export function figurePicture(figure, facing = 'front') {
  const canvas = document.createElement('canvas');
  canvas.width = FIGURE_WIDE;
  canvas.height = FIGURE_TALL;
  const still = { ...figure, x: FIGURE_WIDE / 2, y: FIGURE_TALL - 2, path: [], pose: 'stand', facing, phase: 0 };
  const g = canvas.getContext('2d');
  if (figure.kind === 'bot') drawBot(g, { ...still, wears: botLook(figure.worn) }, 0);
  else drawPerson(g, { ...still, look: personLook(figure.worn) }, 0);
  return canvas;
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
    const moved = hidden.join() !== this.look.hidden.join();
    this.look = { paint, flooring, hidden };
    fillGrid(hidden, this.items);
    this.paintedHour = -1;
    if (!moved) return;
    // A desk or a couch that came or went changes where bots belong.
    for (const bot of this.bots.values()) {
      if (bot.working || bot.state === 'offline') this.place(bot, bot.state, 'walk');
    }
    this.clearTheWay();
  }

  // Whoever's walking takes the new way round, and whoever something landed on steps off it.
  clearTheWay() {
    for (const who of [...this.bots.values(), ...this.people.values()]) {
      if (who.path.length) this.route(who);
      else if (who.pose === 'stand' && !clear(who.x, who.y)) this.send(who, standAt([[who.x, who.y]]), 'walk');
    }
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
    for (const item of this.items) item.spots = spotsOf(item);
    this.clearTheWay();
    // A bot using something goes with it, wherever it went (I38).
    for (const bot of this.bots.values()) if (bot.using) this.place(bot, bot.state, 'walk');
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

  // The word for what came with the room at a place on the page, if this floor shows it. An item
  // stands in front of all of it, so this is for a click that found no item.
  furnitureAt(clientX, clientY) {
    const box = this.canvas.getBoundingClientRect();
    if (!box.width || !box.height) return null;
    const x = ((clientX - box.left) / box.width) * W;
    const y = ((clientY - box.top) / box.height) * H;
    return FURNITURE.find(([word, left, top, wide, deep]) => !this.look.hidden.includes(word)
      && x >= left && x < left + wide && y >= top && y < top + deep)?.[0] ?? null;
  }

  // What someone is about to put down: { item, x, y, w, h, facing, ok, lifted }, in cells, or null.
  // `lifted` is the id of the placed item it is, if it's being moved. `kept` is the boxes of cells
  // that stay clear, to shade while something's in hand.
  setGhost(ghost, kept = []) {
    this.ghost = ghost && Object.hasOwn(ITEMS, ghost.item) ? { ...standing(ghost), ok: ghost.ok, lifted: ghost.lifted } : null;
    this.kept = kept;
  }

  // What build mode has selected: a placed item by id, or the word for what came with the room.
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
      person.look = personLook(data.look);
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
      } else if (bot.state !== data.state || bot.working !== Boolean(data.activity) || bot.using !== data.using) {
        bot.working = Boolean(data.activity);
        bot.using = data.using;
        this.place(bot, data.state, 'walk');
      }
      bot.state = data.state;
      bot.color = data.color;
      bot.name = data.name;
      bot.does = data.does;
      bot.wears = botLook(data.look);
      bot.chip.title = data.status ? `${data.name}: ${data.status}` : data.name;
      bot.thinking = liveThinking(data.thinking);
      this.syncThink(bot);
      // A bot on a sofa says what it's doing by being there, and side by side the words would collide.
      bot.doing.textContent = data.activity ? `· ${data.activity.via}`
        : data.does && !bot.spot?.over ? `· ${data.does}`
          : data.asking ? '· waiting' : '';
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
    for (const bot of this.bots.values()) if (!bot.spot) counts[bot.state] += 1;
    const people = this.people.size;
    const here = people ? ` ${people} ${people === 1 ? 'person' : 'people'} here.` : '';
    const quiet = this.items.some((item) => item.sign) ? ' A quiet sign is up, so bots can\'t post here.' : '';
    const using = [...this.bots.values()].filter((bot) => bot.spot).map((bot) => ` ${bot.name} is ${bot.does}.`).join('');
    this.canvas.setAttribute(
      'aria-label',
      `Hangout room: ${counts.active} bots at the table, ${counts.idle} wandering, ${counts.offline} napping.${here}${using}${quiet}`,
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
    const think = document.createElement('div');
    think.className = 'think-bubble';
    think.hidden = true;
    think.append(document.createElement('span'), document.createElement('span'), document.createElement('span'));
    tag.append(think, bubble, chip);
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
      spot: null,
      working: Boolean(data.activity),
      using: data.using,
      thinking: liveThinking(data.thinking),
      wait: 0,
      phase: (hash(data.id) % 1000) / 1000,
      wears: botLook(data.look),
      tag,
      chip,
      doing,
      bubble,
      bubbleTimer: 0,
      think,
      thinkTimer: 0,
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
    bot.spot = null;
    let goal = null;
    // The desk comes first, because that says what a bot is really doing (I36). A floor with no desk
    // has a bot that's controlling something sit at the table, and one with no couch has it nap by the
    // door. Where the beanbags were is floor too.
    const { hidden } = this.look;
    if (bot.working) {
      goal = hidden.includes('desk') ? null : this.claim(DESKS, 'desk');
      bot.desk = goal || null;
    }
    if (!goal && bot.using) {
      goal = this.claim(this.items.find((item) => item.id === bot.using)?.spots ?? [], 'spot');
      bot.spot = goal || null;
    }
    if (goal) {
      // Already placed at the desk or an item.
    } else if (state === 'active' || (bot.working && hidden.includes('desk'))) {
      goal = this.claim(SEATS, 'seat');
      bot.seat = goal || null;
      if (goal) this.vacate(goal);
    } else if (state === 'offline') {
      goal = this.claim(hidden.includes('couch') ? NAPS.slice(3) : NAPS, 'nap') || NAPS[NAPS.length - 1];
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
    this.syncThink(bot);
    clearTimeout(bot.bubbleTimer);
    const shown = Math.min(7000, Math.max(3000, text.length * 70));
    bot.bubbleTimer = setTimeout(() => {
      bot.bubble.hidden = true;
      this.syncThink(bot);
    }, shown);
  }

  // I64: a thinking bubble above the avatar, quieter while it's only waiting on a mention.
  syncThink(bot) {
    const live = liveThinking(bot.thinking);
    bot.thinking = live;
    clearTimeout(bot.thinkTimer);
    const speaking = !bot.bubble.hidden;
    bot.think.hidden = !live || speaking;
    bot.think.classList.toggle('waiting', Boolean(live && live.phase === 'waiting'));
    if (live) {
      bot.thinkTimer = setTimeout(() => this.syncThink(bot), Math.max(0, live.until * 1000 - Date.now()));
    }
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
    if (!bot.desk && !bot.spot && !bot.seat && (bot.state === 'idle' || bot.state === 'active')) {
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
    for (const [word, left, top, wide, deep] of FURNITURE) {
      if (word === this.chosen && !this.look.hidden.includes(word)) outline(g, { left, top, wide, deep }, '#ffde59');
    }
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
    paintBehind(g, hidden, time);

    const lit = SEATS.slice(0, 8).map((seat) =>
      [...this.bots.values()].some((bot) => bot.seat === seat && !bot.path.length),
    );
    const deskLit = DESKS.map((desk) =>
      [...this.bots.values()].some((bot) => bot.desk === desk && !bot.path.length),
    );
    const bots = [...this.bots.values()];
    const people = [...this.people.values()];
    for (const bot of bots) this.update(bot, dt);
    for (const person of people) this.updatePerson(person, dt);
    const still = this.calm ? 0 : time;
    const scene = [
      ...furnitureScene(g, hidden, time, lit, deskLit),
      ...this.items.filter((item) => !item.sign).map((item) => ({ y: item.y, draw: () => this.drawItem(g, item, still) })),
      ...bots.map((bot) => ({ y: (!bot.path.length && bot.spot?.over) || bot.y, draw: () => drawBot(g, bot, time) })),
      ...people.map((person) => ({ y: person.y, draw: () => drawPerson(g, person, still) })),
    ];
    scene.sort((a, b) => a.y - b.y);
    for (const item of scene) item.draw();
    for (const bot of bots) if (!bot.path.length) bot.spot?.busy?.(g, bot, still);
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
      const drop = (bot.seat || bot.spot) && !bot.path.length && (bot.seat || bot.spot).drop;
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
