// The pixel room. Where a bot stands comes from real presence data; the walking is just for fun.

const W = 480;
const H = 252;
const WALL = 66;
const CELL = 6;
const COLS = W / CELL;
const ROWS = Math.floor(H / CELL);
const SPEED = 38;

const BLOCKED = [
  [0, 0, W, WALL + 8],
  [174, 124, 132, 46], // table
  [12, 190, 96, 34], // couch
  [24, 98, 92, 38], // control desk
  [370, 66, 110, 36], // coffee bar
  [428, 124, 50, 48], // arcade
  [460, 224, 20, 28], // plant
  [0, 0, 6, H],
  [W - 6, 0, 6, H],
  [0, H - 6, W, 6],
];

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

function isBlocked(col, row) {
  const x = col * CELL + CELL / 2;
  const y = row * CELL + CELL / 2;
  return BLOCKED.some(([bx, by, bw, bh]) => x >= bx && x < bx + bw && y >= by && y < by + bh);
}

const GRID = new Uint8Array(COLS * ROWS);
for (let row = 0; row < ROWS; row++) {
  for (let col = 0; col < COLS; col++) GRID[row * COLS + col] = isBlocked(col, row) ? 1 : 0;
}

const free = (col, row) => col >= 0 && row >= 0 && col < COLS && row < ROWS && !GRID[row * COLS + col];

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

function paintRoom(g, hour) {
  const sky = skyFor(hour);
  // Wall
  r(g, '#364466', 0, 0, W, WALL);
  for (let x = 0; x < W; x += 16) r(g, '#3a496d', x, 0, 8, 46);
  r(g, '#2a3450', 0, 46, W, 18);
  for (let x = 6; x < W; x += 24) r(g, '#232c45', x, 48, 18, 14);
  r(g, '#c9a86a', 0, 45, W, 1);
  r(g, '#1b2236', 0, 64, W, 3);

  paintWindow(g, 36, 10, 72, 32, sky);
  paintWindow(g, 372, 10, 72, 32, sky);

  // Floor
  for (let y = WALL + 1, row = 0; y < H; y += 10, row++) {
    r(g, row % 2 ? '#a97b4e' : '#a27347', 0, y, W, 10);
    r(g, '#8b6039', 0, y + 9, W, 1);
    for (let x = (row * 37) % 60; x < W; x += 60) r(g, '#8b6039', x, y, 1, 9);
  }
  r(g, 'rgba(30,20,50,.20)', 0, WALL + 1, W, 3);

  // Rug under the table
  r(g, '#6a2a3c', 128, 104, 224, 100);
  r(g, '#c9a86a', 132, 108, 216, 1);
  r(g, '#c9a86a', 132, 199, 216, 1);
  r(g, '#c9a86a', 132, 108, 1, 92);
  r(g, '#c9a86a', 347, 108, 1, 92);
  r(g, '#7b3346', 138, 114, 204, 80);

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
    this.bots = new Map();
    this.confetti = [];
    this.last = performance.now();
    this.calm = matchMedia('(prefers-reduced-motion: reduce)').matches;
    this.running = true;
    requestAnimationFrame((t) => this.frame(t));
  }

  destroy() {
    this.running = false;
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
    const counts = { active: 0, idle: 0, offline: 0 };
    for (const data of list) counts[data.state] += 1;
    this.canvas.setAttribute(
      'aria-label',
      `Hangout room: ${counts.active} bots at the table, ${counts.idle} wandering, ${counts.offline} napping.`,
    );
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

  claim(list, key) {
    const taken = new Set([...this.bots.values()].map((bot) => bot[key]).filter(Boolean));
    return list.find((spot) => !taken.has(spot));
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
    } else if (state === 'offline') {
      goal = this.claim(NAPS, 'nap') || NAPS[NAPS.length - 1];
      bot.nap = goal;
    }
    if (!goal) {
      const [x, y] = SPOTS[Math.floor(Math.random() * SPOTS.length)];
      goal = { x, y, ax: x, ay: y, pose: 'stand', facing: 'front' };
    }
    bot.goal = goal;
    bot.pose = 'stand';
    if (how === 'instant' || this.calm) {
      bot.x = goal.x;
      bot.y = goal.y;
      bot.path = [];
      this.arrive(bot);
      return;
    }
    if (how === 'door') {
      bot.x = 240;
      bot.y = H - 12;
    }
    bot.path = [...findPath(bot.x, bot.y, goal.ax, goal.ay), { x: goal.x, y: goal.y }];
  }

  arrive(bot) {
    const goal = bot.goal;
    bot.pose = goal.pose;
    bot.facing = goal.facing || 'front';
    bot.wait = 3 + Math.random() * 7;
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

  update(bot, dt) {
    if (bot.path.length) {
      let budget = SPEED * dt;
      while (budget > 0 && bot.path.length) {
        const next = bot.path[0];
        const dx = next.x - bot.x;
        const dy = next.y - bot.y;
        const distance = Math.hypot(dx, dy);
        if (Math.abs(dx) > 0.5) bot.facing = dx < 0 ? 'left' : 'right';
        if (distance <= budget) {
          bot.x = next.x;
          bot.y = next.y;
          bot.path.shift();
          budget -= distance;
        } else {
          bot.x += (dx / distance) * budget;
          bot.y += (dy / distance) * budget;
          budget = 0;
        }
      }
      if (!bot.path.length) this.arrive(bot);
      return;
    }
    // A busy bot that didn't get a seat wanders too, and takes one when it frees up.
    if (!bot.desk && (bot.state === 'idle' || (bot.state === 'active' && !bot.seat))) {
      bot.wait -= dt;
      if (bot.wait <= 0) this.place(bot, bot.state, 'walk');
    }
  }

  frame(time) {
    if (!this.running) return;
    const dt = Math.min(0.1, (time - this.last) / 1000);
    this.last = time;
    const hour = new Date().getHours();
    if (hour !== this.paintedHour) {
      this.paintedHour = hour;
      paintRoom(this.background.getContext('2d'), hour);
    }

    const g = this.g;
    g.drawImage(this.background, 0, 0);
    paintBar(g, time);
    paintPlant(g, 470, 250);
    paintPlant(g, 14, 92);

    const lit = SEATS.slice(0, 8).map((seat) =>
      [...this.bots.values()].some((bot) => bot.seat === seat && !bot.path.length),
    );
    const deskLit = DESKS.map((desk) =>
      [...this.bots.values()].some((bot) => bot.desk === desk && !bot.path.length),
    );
    const props = [
      { y: 136, draw: () => paintDesk(g, time, deskLit) },
      { y: 150, draw: () => paintTable(g, lit) },
      { y: 172, draw: () => paintArcade(g, time) },
      { y: 200, draw: () => paintCouch(g) },
      { y: 232, draw: () => paintBeanbags(g) },
    ];
    const bots = [...this.bots.values()];
    for (const bot of bots) this.update(bot, dt);
    const scene = [...props, ...bots.map((bot) => ({ y: bot.y, draw: () => drawBot(g, bot, time) }))];
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
    requestAnimationFrame((t) => this.frame(t));
  }
}
