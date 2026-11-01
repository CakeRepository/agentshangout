// A place's building, from what its owner chose (docs/outside.md). It is painted in the same pieces
// the old placed buildings used, and with nothing chosen it comes out the same as they did (E4).
//
// This module imports from world.js, which imports it back, so nothing here calls an imported function
// while the module loads. Every table below holds literals and its own functions, and runs later.

import {
  BRICKS, GLASS_BOT, GLASS_TOP, P, TRIM, awning as mapAwning, box, brickChimney, clamp, dark, door as mapDoor, drawTree, ell,
  fp, fp3, gable as mapGable, grad, ih, layout, ledge, lerp, line, lite, mixHex, onL, onR, paintWall, pathOf, picket, rgba, rr, seeded,
  shadeFace, shadow, sign as mapSign, slope as mapSlope, topFace,
} from './world.js';
import { bodyOf, hexOf, isOn, numberOf, partOf, rectOf, styleOf } from './outside-spec.js';

const TAU = 6.2832;

// What each kind of place has when nothing is chosen: `h` is a storey's height, `win` a window's width,
// height and sill, and `door` a door's width and height. The rest is what a kind is known for (W90), and
// stays when something else about it changes: `up` the height of each storey above the first, `rows` the
// rows of windows on the first storey and on the others, `lit` how many of its windows shine at night out
// of a hundred, `frame` its window frames, `shade` how far a flat roof's shadow stands above its walls, `pk` how
// steep its pitched roof is, `over` how far it overhangs, `pshade` how far that roof's shadow stands above its
// walls, `curtain` the curtains in its windows, `shift` how far its door sits from the
// middle of its wall, and `wtop` the height its windows stop at.
const KINDS = {
  house: { h: 11, win: [4, 5, 4], door: [4.4, 8], roof: 'gable' },
  cafe: { h: 13, win: [5, 6, 4], door: [4.4, 8.5], roof: 'flat' },
  club: { h: 17, win: [4.4, 6, 5], door: [4.4, 8.5], roof: 'flat', doorCol: '#2a2436' },
  studio: { h: 15, win: [4.4, 7, 5], door: [4.4, 8.5], roof: 'flat' },
  lounge: { h: 11, win: [4.4, 5.5, 3.5], door: [4.4, 8], roof: 'flat', lit: 90, curtain: '#b45a6a' },
  bar: { h: 13, win: [3.6, 4.4, 4], door: [4.4, 8.5], roof: 'flat', lit: 100, doorCol: '#1e1a22', wtop: 9.6 },
  office: { h: 30, up: 18, win: [3.6, 5.5, 3.5], door: [4.4, 9], roof: 'flat', lit: 55, frame: '#5a6a80', doorCol: '#3a5a7a', shade: 4, shift: -0.4, rows: [3, 2], parapet: 1.5, coping: '#9aa0aa' },
  workshop: { h: 14, win: [4, 5, 6], door: [4.4, 8], roof: 'gable', lit: 70, frame: '#4a4a4a', roofCol: '#6a6f78', over: 1.2, pk: 42 / 76.5, pshade: 8, smoke: true },
  library: { h: 16, win: [2.8, 9, 4], door: [4.4, 10], roof: 'gable', lit: 75, frame: '#7a6a50', doorCol: '#5a3a24', roofCol: '#5f6f7f', over: 1.4, pk: 49 / 72, pshade: 8, shift: -0.2 },
};

// ---------------------------------------------------------------------------------------------
// Small tools

function glowAt(g, x, y, r, color, a = 0.5) {
  const gr = g.createRadialGradient(x, y, 0, x, y, r);
  gr.addColorStop(0, rgba(color, a));
  gr.addColorStop(1, rgba(color, 0));
  g.fillStyle = gr;
  g.fillRect(x - r, y - r, r * 2, r * 2);
}

function circle(g, fill, x, y, r) {
  g.beginPath();
  g.arc(x, y, r, 0, TAU);
  g.fillStyle = fill;
  g.fill();
}

function stroke(g, color, w) {
  g.strokeStyle = color;
  g.lineWidth = w;
  g.stroke();
}

// A polyline through screen points.
function trail(g, color, w, pts) {
  g.beginPath();
  g.moveTo(pts[0][0], pts[0][1]);
  for (let i = 1; i < pts.length; i++) g.lineTo(pts[i][0], pts[i][1]);
  stroke(g, color, w);
}

const P3 = (p) => P(p[0], p[1], p[2]);
const mid3 = (a, b, t = 0.5) => [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)];

// ---------------------------------------------------------------------------------------------
// Walls (E10). Each entry gives paintWall a material; the ones with `paint` draw their own pattern, where
// x runs 0 to len and the wall stands from y = -z1 to -z0.

// Colours a material has when its owner hasn't chosen one, for the ones that are only ever one colour.
const NATURAL = {
  stone: '#a39f94',
  logs: '#99683b',
  timber: '#c4975e',
  steel: '#9fb0c4',
  concrete: '#bcbab3',
  tudor: '#efe4c8',
  glass: '#6f9fc0',
};

function paintStucco(base) {
  return (g, len, z0, z1, rand) => {
    const top = -z1;
    const h = z1 - z0;
    g.fillStyle = grad(g, 0, top, 0, -z0, lite(base, 0.1), base, dark(base, 0.08));
    g.fillRect(0, top, len, h);
    for (let i = 0; i < len * h * 0.03; i++) ell(g, rand() < 0.5 ? 'rgba(255,255,245,.1)' : 'rgba(70,50,40,.07)', rand() * len, top + rand() * h, 1.5 + rand() * 3, 1 + rand() * 2.4);
    for (let i = 0; i < len * h * 0.05; i++) rr(g, rand() < 0.5 ? 'rgba(255,255,240,.28)' : 'rgba(60,40,30,.2)', rand() * len, top + rand() * h, 0.8, 0.8);
  };
}

function paintStone(base) {
  return (g, len, z0, z1, rand) => {
    const top = -z1;
    rr(g, dark(base, 0.4), 0, top, len, z1 - z0);
    let y = -z0;
    while (y > top) {
      const bh = 2.8 + rand() * 1.6;
      let x = -rand() * 4;
      while (x < len) {
        const bw = 4 + rand() * 4.5;
        rr(g, mixHex(base, rand() < 0.5 ? dark(base, 0.3) : lite(base, 0.35), 0.1 + rand() * 0.45), x + 0.35, y - bh + 0.35, bw - 0.7, bh - 0.7);
        rr(g, 'rgba(255,250,235,.2)', x + 0.35, y - bh + 0.35, bw - 0.7, 0.6);
        rr(g, 'rgba(20,15,30,.16)', x + 0.35, y - 1, bw - 0.7, 0.65);
        x += bw;
      }
      y -= bh;
    }
    g.fillStyle = grad(g, 0, top, 0, -z0, 'rgba(255,240,210,.14)', 'rgba(0,0,0,0)', 'rgba(20,10,30,.14)');
    g.fillRect(0, top, len, z1 - z0);
  };
}

function paintTimber(base) {
  return (g, len, z0, z1, rand) => {
    const top = -z1;
    const h = z1 - z0;
    rr(g, dark(base, 0.5), 0, top, len, h);
    for (let x = 0; x < len; x += 2.4) {
      const tone = mixHex(base, rand() < 0.5 ? dark(base, 0.3) : lite(base, 0.3), rand() * 0.5);
      rr(g, tone, x + 0.2, top, 2, h);
      rr(g, 'rgba(255,240,210,.22)', x + 0.2, top, 0.5, h);
      for (let k = 0; k < 2; k++) line(g, 'rgba(60,35,20,.22)', 0.4, x + 0.7 + rand(), top + rand() * h, x + 0.9 + rand(), top + rand() * h);
    }
    for (let y = -z0 - 6; y > top; y -= 7) rr(g, dark(base, 0.18), 0, y, len, 0.9);
    g.fillStyle = grad(g, 0, top, 0, -z0, 'rgba(255,240,210,.12)', 'rgba(0,0,0,0)', 'rgba(20,10,30,.12)');
    g.fillRect(0, top, len, h);
  };
}

function paintLogs(base) {
  return (g, len, z0, z1, rand) => {
    const top = -z1;
    rr(g, dark(base, 0.55), 0, top, len, z1 - z0);
    const lh = 3.2;
    for (let y = -z0 - lh; y > top - lh; y -= lh) {
      const tone = mixHex(base, rand() < 0.5 ? dark(base, 0.25) : lite(base, 0.2), rand() * 0.5);
      g.fillStyle = grad(g, 0, y, 0, y + lh, lite(tone, 0.25), tone, dark(tone, 0.3));
      g.fillRect(0, y + 0.2, len, lh - 0.4);
      for (let k = 0; k < len / 9; k++) {
        const kx = rand() * len;
        ell(g, dark(tone, 0.4), kx, y + lh * 0.5, 0.9, 0.6);
      }
    }
    // The cut ends of the logs at the corners.
    for (let y = -z0 - lh; y > top - lh; y -= lh) {
      ell(g, lite(base, 0.3), 0.6, y + lh / 2, 0.9, lh * 0.45);
      ell(g, lite(base, 0.3), len - 0.6, y + lh / 2, 0.9, lh * 0.45);
    }
  };
}

function paintShingleWall(base) {
  return (g, len, z0, z1, rand) => {
    const top = -z1;
    rr(g, dark(base, 0.4), 0, top, len, z1 - z0);
    const rh = 2.4;
    let row = 0;
    for (let y = -z0 + rh; y > top - rh; y -= rh * 0.7, row++) {
      for (let x = -(row % 2) * 1.6; x < len + 3; x += 3.2) {
        const tone = mixHex(base, rand() < 0.5 ? dark(base, 0.22) : lite(base, 0.25), rand() * 0.5);
        rr(g, tone, x, y - rh * 1.5, 3.1, rh * 1.5 - 1.1);
        g.beginPath();
        g.arc(x + 1.55, y - 1.1 - rh * 0.3, 1.55, 0, Math.PI);
        g.fillStyle = tone;
        g.fill();
        g.beginPath();
        g.arc(x + 1.55, y - 1.1 - rh * 0.3, 1.55, 0, Math.PI);
        stroke(g, 'rgba(25,15,35,.28)', 0.4);
      }
    }
    g.fillStyle = grad(g, 0, top, 0, -z0, 'rgba(255,240,210,.12)', 'rgba(0,0,0,0)', 'rgba(20,10,30,.12)');
    g.fillRect(0, top, len, z1 - z0);
  };
}

function paintTudor(base) {
  return (g, len, z0, z1, rand) => {
    const top = -z1;
    const h = z1 - z0;
    g.fillStyle = grad(g, 0, top, 0, -z0, lite(base, 0.08), base, dark(base, 0.06));
    g.fillRect(0, top, len, h);
    for (let i = 0; i < len * h * 0.04; i++) rr(g, rand() < 0.5 ? 'rgba(255,255,245,.4)' : 'rgba(80,60,40,.18)', rand() * len, top + rand() * h, 0.7, 0.7);
    const beam = '#4a3426';
    const stories = Math.max(1, Math.round(h / 10));
    const sh = h / stories;
    const bays = Math.max(1, Math.round(len / 7));
    const bw = len / bays;
    for (let s = 0; s < stories; s++) {
      const yb = -z0 - s * sh;
      const yt = yb - sh;
      for (let b = 0; b < bays; b++) {
        const xa = b * bw;
        // A brace in alternating panels.
        if ((b + s) % 2 === 0) line(g, beam, 0.9, xa, yb, xa + bw, yt);
        else line(g, beam, 0.9, xa + bw, yb, xa, yt);
      }
      rr(g, beam, 0, yt - 0.5, len, 1.1);
    }
    for (let b = 0; b <= bays; b++) rr(g, beam, b * bw - 0.5, top, 1.1, h);
    rr(g, beam, 0, -z0 - 1, len, 1.1);
    rr(g, 'rgba(255,240,210,.18)', 0, top, len, h * 0.3);
  };
}

function paintGlassWall(base) {
  return (g, len, z0, z1, rand) => {
    const top = -z1;
    const h = z1 - z0;
    g.fillStyle = grad(g, 0, top, 0, -z0, lite(base, 0.4), base, dark(base, 0.3));
    g.fillRect(0, top, len, h);
    // A reflection of the sky running across the panes.
    for (let k = -1; k < len / 12 + 1; k++) {
      const x = k * 12 + 2;
      fp(g, 'rgba(255,255,255,.2)', [[x, -z0], [x + 4, -z0], [x + 4 + h * 0.5, top], [x + h * 0.5, top]]);
      fp(g, 'rgba(255,255,255,.1)', [[x + 6, -z0], [x + 7.4, -z0], [x + 7.4 + h * 0.5, top], [x + 6 + h * 0.5, top]]);
    }
    for (let x = 0; x < len; x += 4.5) for (let y = -z0 - 5.5; y > top - 5.5; y -= 5.5) if (rand() < 0.2) rr(g, 'rgba(20,40,70,.18)', x + 0.4, y + 0.4, 3.7, 4.7);
    const mull = '#26364a';
    for (let x = 0; x <= len + 0.1; x += 4.5) rr(g, mull, Math.min(x, len - 0.4) - 0.2, top, 0.8, h);
    for (let y = -z0; y >= top - 0.1; y -= 5.5) rr(g, mull, 0, y - 0.4, len, 0.8);
  };
}

const WALL_ART = {
  siding: (c) => ({ k: 'siding', c }),
  brick: (c) => ({ k: 'brick', c }),
  stucco: (c) => ({ k: 'stucco', c, paint: paintStucco(c) }),
  stone: (c) => ({ k: 'stone', c, paint: paintStone(c) }),
  timber: (c) => ({ k: 'timber', c, paint: paintTimber(c) }),
  logs: (c) => ({ k: 'logs', c, paint: paintLogs(c) }),
  shingles: (c) => ({ k: 'shingles', c, paint: paintShingleWall(c) }),
  steel: (c) => ({ k: 'steel', c }),
  concrete: (c) => ({ k: 'concrete', c }),
  tudor: (c) => ({ k: 'tudor', c, paint: paintTudor(c) }),
  glass: (c) => ({ k: 'glass', c, paint: paintGlassWall(c) }),
};

// ---------------------------------------------------------------------------------------------
// Foundations (E10): a band at the foot of a wall, in the wall's own plane.

const FOUNDATION_ART = {
  none() {},
  stone(g, x0, x1, h, rand) {
    rr(g, '#6f6a62', x0, -h, x1 - x0, h);
    for (let y = -h; y < 0; y += 1.3) {
      for (let x = x0 - rand() * 2; x < x1; x += 2.6 + rand() * 1.6) {
        const xs = Math.max(x, x0);
        rr(g, mixHex('#9a958a', rand() < 0.5 ? '#6f6a62' : '#bdb8ad', rand() * 0.5), xs + 0.2, y + 0.15, Math.min(x + 2.4, x1) - xs - 0.4, 1);
      }
    }
    rr(g, 'rgba(255,250,235,.35)', x0, -h, x1 - x0, 0.6);
    rr(g, 'rgba(25,15,35,.2)', x0, -h + 0.6, x1 - x0, 0.6);
  },
  brick(g, x0, x1, h) {
    rr(g, '#c9bca4', x0, -h, x1 - x0, h);
    let row = 0;
    for (let y = -h; y < 0; y += 1.2, row++) {
      for (let x = x0 - (row % 2) * 1.6; x < x1; x += 3.2) {
        const xs = Math.max(x, x0);
        rr(g, ih(row, Math.round(x), 4) % 3 ? '#8c4b3a' : '#a05a44', xs + 0.2, y + 0.15, Math.min(x + 3, x1) - xs - 0.3, 0.9);
      }
    }
    rr(g, 'rgba(255,250,235,.3)', x0, -h, x1 - x0, 0.5);
  },
  // The raised plinth is a box under the whole building, so a wall needs no band of its own.
  raised() {},
};

// ---------------------------------------------------------------------------------------------
// Trim (E10): what the frames, sills and eaves are like. `fr` is how wide a frame is, `board` is the width
// of a corner board, `fascia` the height of the eave board, and `lintel` and `sill` say what goes round a window.

const TRIM_ART = {
  plain: { fr: 1, board: 0, fascia: 1.8, sill: 1, lintel: 1, bracket: false },
  bold: { fr: 1.7, board: 1.1, fascia: 2.8, sill: 1.7, lintel: 1.7, bracket: false },
  gingerbread: { fr: 1.4, board: 0.9, fascia: 2.4, sill: 1.3, lintel: 1.4, bracket: true },
  none: { fr: 0, board: 0, fascia: 1.4, sill: 0, lintel: 0, bracket: false },
};

// ---------------------------------------------------------------------------------------------
// Windows (E10). Each entry says how big it is next to the kind's own window, and how its panes run.

function shapePath(g, style, x, t, w, h, grow) {
  g.beginPath();
  if (style === 'arched' || style === 'stained') {
    const r = w / 2 + grow;
    const cx = x + w / 2;
    g.moveTo(cx - r, t + h + grow);
    g.lineTo(cx - r, t + r - grow);
    g.arc(cx, t + r - grow, r, Math.PI, 0);
    g.lineTo(cx + r, t + h + grow);
    g.closePath();
  } else if (style === 'round') {
    g.ellipse(x + w / 2, t + h / 2, w / 2 + grow, h / 2 + grow, 0, 0, TAU);
  } else {
    g.rect(x - grow, t - grow, w + grow * 2, h + grow * 2);
  }
}

function fillShape(g, style, fill, x, t, w, h, grow) {
  shapePath(g, style, x, t, w, h, grow);
  g.fillStyle = fill;
  g.fill();
}

const STAINED = ['#d9453f', '#f0b030', '#3f9a5a', '#3a78d0', '#8a52d0', '#f08a30'];

function stainedPanes(g, x, t, w, h, lit) {
  const cols = 3;
  const rows = 4;
  for (let r = 0; r < rows; r++) {
    for (let c = 0; c < cols; c++) {
      const col = STAINED[(r * 2 + c * 3 + r * c) % STAINED.length];
      rr(g, lit ? lite(col, 0.35) : col, x + (c * w) / cols, t + (r * h) / rows, w / cols + 0.1, h / rows + 0.1);
    }
  }
  for (let c = 1; c < cols; c++) line(g, 'rgba(30,25,40,.7)', 0.5, x + (c * w) / cols, t, x + (c * w) / cols, t + h);
  for (let r = 1; r < rows; r++) line(g, 'rgba(30,25,40,.7)', 0.5, x, t + (r * h) / rows, x + w, t + (r * h) / rows);
}

const WINDOW_ART = {
  square: {
    size: (w, h) => [w, h],
    bars(g, x, t, w, h, col, k = 1) {
      line(g, col, 0.9 * k, x + w / 2, t, x + w / 2, t + h);
      if (h > 7) line(g, col, 0.9 * k, x, t + h * 0.45, x + w, t + h * 0.45);
    },
  },
  tall: {
    size: (w, h) => [w * 0.72, h * 1.3],
    bars(g, x, t, w, h, col, k = 1) {
      line(g, col, 0.9 * k, x + w / 2, t, x + w / 2, t + h);
      line(g, col, 0.8 * k, x, t + h / 3, x + w, t + h / 3);
      line(g, col, 0.8 * k, x, t + (h * 2) / 3, x + w, t + (h * 2) / 3);
    },
  },
  wide: {
    size: (w, h) => [w * 1.6, h * 0.82],
    bars(g, x, t, w, h, col, k = 1) {
      line(g, col, 0.9 * k, x + w / 3, t, x + w / 3, t + h);
      line(g, col, 0.9 * k, x + (w * 2) / 3, t, x + (w * 2) / 3, t + h);
    },
  },
  arched: {
    size: (w, h) => [w, h * 1.15],
    bars(g, x, t, w, h, col, k = 1) {
      line(g, col, 0.9 * k, x + w / 2, t, x + w / 2, t + h);
      line(g, col, 0.9 * k, x, t + w / 2 + 0.4, x + w, t + w / 2 + 0.4);
    },
  },
  round: {
    size: (w, h) => {
      const d = Math.min(w + 0.8, h);
      return [d, d];
    },
    bars(g, x, t, w, h, col, k = 1) {
      line(g, col, 0.8 * k, x + w / 2, t, x + w / 2, t + h);
      line(g, col, 0.8 * k, x, t + h / 2, x + w, t + h / 2);
      g.beginPath();
      g.ellipse(x + w / 2, t + h / 2, w * 0.28, h * 0.28, 0, 0, TAU);
      stroke(g, col, 0.5 * k);
    },
  },
  paned: {
    size: (w, h) => [w, h * 1.05],
    bars(g, x, t, w, h, col, k = 1) {
      for (let k = 1; k < 3; k++) {
        line(g, col, 0.55 * k, x + (w * k) / 3, t, x + (w * k) / 3, t + h);
        line(g, col, 0.55 * k, x, t + (h * k) / 3, x + w, t + (h * k) / 3);
      }
    },
  },
  picture: {
    size: (w, h) => [w * 1.9, h * 1.1],
    bars(g, x, t, w, h) {
      fp(g, 'rgba(255,255,255,.2)', [[x + w * 0.55, t], [x + w * 0.78, t], [x + w * 0.35, t + h], [x + w * 0.12, t + h]]);
    },
  },
  diamond: {
    size: (w, h) => [w, h],
    bars(g, x, t, w, h, col, k = 1) {
      const s = Math.max(1.8, w / 2.5);
      for (let k = -Math.ceil(h / s); k < Math.ceil(w / s) + 1; k++) {
        line(g, col, 0.5 * k, x + k * s, t, x + k * s + h, t + h);
        line(g, col, 0.5 * k, x + k * s + h, t, x + k * s, t + h);
      }
    },
  },
  stained: {
    size: (w, h) => [w * 0.9, h * 1.3],
    bars() {},
  },
};

const SHUTTER_ART = {
  none() {},
  louvered(g, x, t, sw, h, col) {
    rr(g, col, x, t, sw, h);
    for (let k = 1; k < h / 1.7; k++) rr(g, 'rgba(0,0,0,.25)', x, t + k * 1.7, sw, 0.5);
    rr(g, 'rgba(255,255,255,.18)', x, t, 0.5, h);
  },
  board(g, x, t, sw, h, col) {
    rr(g, dark(col, 0.2), x, t, sw, h);
    for (let k = 0; k < sw; k += 0.9) rr(g, col, x + k + 0.1, t, 0.7, h);
    line(g, dark(col, 0.4), 0.5, x, t + h * 0.18, x + sw, t + h * 0.18);
    line(g, dark(col, 0.4), 0.5, x, t + h * 0.82, x + sw, t + h * 0.82);
    line(g, dark(col, 0.35), 0.5, x, t + h * 0.82, x + sw, t + h * 0.18);
  },
  panel(g, x, t, sw, h, col) {
    rr(g, col, x, t, sw, h);
    rr(g, dark(col, 0.28), x + 0.4, t + 0.5, sw - 0.8, h * 0.42);
    rr(g, lite(col, 0.18), x + 0.7, t + 0.8, sw - 1.4, h * 0.42 - 0.6);
    rr(g, dark(col, 0.28), x + 0.4, t + h * 0.52, sw - 0.8, h * 0.42);
    rr(g, lite(col, 0.18), x + 0.7, t + h * 0.52 + 0.3, sw - 1.4, h * 0.42 - 0.6);
  },
};

// Curtains are painted inside the glass. The ones that cover it are drawn by day only.
const CURTAIN_ART = {
  none() {},
  open(g, x, t, w, h, col) {
    fp(g, rgba(col, 0.9), [[x, t], [x + w * 0.38, t], [x + w * 0.1, t + h * 0.62], [x, t + h * 0.62]]);
    fp(g, rgba(col, 0.9), [[x + w, t], [x + w * 0.62, t], [x + w * 0.9, t + h * 0.62], [x + w, t + h * 0.62]]);
  },
  closed(g, x, t, w, h, col) {
    rr(g, rgba(col, 0.96), x, t, w, h);
    for (let k = 1; k < 5; k++) rr(g, 'rgba(25,15,35,.14)', x + (w * k) / 5 - 0.2, t, 0.5, h);
    rr(g, 'rgba(25,15,35,.25)', x, t, w, 0.8);
  },
  blinds(g, x, t, w, h, col) {
    for (let y = t; y < t + h * 0.62; y += 1.2) {
      rr(g, col, x, y, w, 0.85);
      rr(g, 'rgba(25,15,35,.22)', x, y + 0.85, w, 0.35);
    }
    rr(g, dark(col, 0.2), x, t + h * 0.62, w, 0.6);
  },
  lace(g, x, t, w, h, col) {
    rr(g, rgba(col, 0.5), x, t, w, h);
    for (let yy = t + 1; yy < t + h; yy += 1.6) {
      for (let xx = x + 0.8 + ((yy / 1.6) % 2) * 0.8; xx < x + w; xx += 1.6) circle(g, rgba(col, 0.9), xx, yy, 0.38);
    }
    for (let xx = x + 0.7; xx < x + w; xx += 1.4) ell(g, rgba(col, 0.95), xx, t + h - 0.2, 0.8, 0.9);
  },
};

const BOX_ART = {
  none() {},
  flowers(g, x, z, w) {
    for (let k = 0; k < w + 1; k += 1.6) {
      ell(g, ['#ff7fa6', '#ffd75e', '#ffffff', '#ff9a5a'][Math.floor(k) % 4], x - 0.4 + k, -z + 0.4, 1, 0.9);
      rr(g, '#3f8a3c', x - 0.5 + k, -z + 0.8, 0.8, 1.2);
    }
  },
  herbs(g, x, z, w) {
    for (let k = 0; k < w + 1; k += 1.2) {
      const tall = ih(Math.round(k * 5), Math.round(x), 3) % 3;
      fp(g, k % 2 < 1 ? '#4f9a46' : '#6dba5c', [[x - 0.4 + k, -z + 1.4], [x + 0.3 + k, -z - 0.6 - tall * 0.5], [x + 1 + k, -z + 1.4]]);
      if (tall === 0) circle(g, '#f6f6e8', x + 0.3 + k, -z - 0.8, 0.45);
    }
  },
  mums(g, x, z, w) {
    // The room's mums (web/room.js): mostly plum with gold ones mixed through, on dark green.
    const cols = ['#7a3a66', '#d9a520', '#a3608f', '#5a2a4c'];
    // Dark green leaves behind, then the round blooms along the box.
    for (let k = 0.3; k < w + 1; k += 1.1) ell(g, k % 2.2 < 1.1 ? '#5f6b2e' : '#7f8c3a', x - 0.2 + k, -z + 1, 0.9, 0.7);
    let n = 0;
    for (let k = 0; k < w + 1; k += 1.6) {
      const c = cols[n++ % 4];
      const bx = x - 0.2 + k;
      const by = -z + 0.1 - (ih(Math.round(k * 5), Math.round(x), 4) % 2) * 0.5;
      ell(g, dark(c, 0.35), bx + 0.15, by + 0.3, 1.25, 1.2);
      ell(g, c, bx, by, 1.2, 1.1);
      ell(g, rgba(lite(c, 0.45), 0.75), bx - 0.4, by - 0.35, 0.5, 0.35);
      ell(g, '#e8b431', bx, by, 0.28, 0.26);
    }
  },
  ivy(g, x, z, w) {
    for (let k = 0; k < w + 1; k += 0.9) {
      ell(g, k % 1.8 < 0.9 ? '#3f8a3c' : '#58a64a', x - 0.2 + k, -z + 0.5, 0.95, 0.9);
      if (ih(Math.round(k * 7), Math.round(x), 8) % 2) {
        const hang = 1.6 + (ih(Math.round(k * 3), Math.round(x), 2) % 3);
        line(g, '#3f7a38', 0.45, x - 0.2 + k, -z + 1.2, x - 0.2 + k, -z + 1.2 + hang + 2.6);
        ell(g, '#58a64a', x - 0.2 + k, -z + 3.9 + hang, 0.7, 0.9);
      }
    }
  },
};

// The fall palette for leaves on a garland, a window box and a wreath: russet, gold and deep red.
const FALL_LEAVES = ['#b5471f', '#d98a22', '#a42a1f', '#e0b030', '#8f3a1c', '#c9621f', '#e8a23a', '#7a2a18'];

// One pointed leaf from its stalk at (x, y), `s` long and turned to `a`.
function leafAt(g, fill, x, y, a, s) {
  const ca = Math.cos(a);
  const sa = Math.sin(a);
  const mx = x + (ca * s) / 2;
  const my = y + (sa * s) / 2;
  g.beginPath();
  g.moveTo(x, y);
  g.quadraticCurveTo(mx - sa * s * 0.4, my + ca * s * 0.4, x + ca * s, y + sa * s);
  g.quadraticCurveTo(mx + sa * s * 0.4, my - ca * s * 0.4, x, y);
  g.fillStyle = fill;
  g.fill();
}

// One window, in the plane of the wall it is on. `o` says what it is made of: its style, its glass, the frame
// round it, and what hangs at it. By day it's painted whole. At night only the lit ones glow, and which
// ones are lit comes from the building's seed and the window's number, so the same windows stay lit (E10).
function windowArt(B, x, z, w, h, o) {
  const { g, S } = B;
  const art = WINDOW_ART[o.style] || WINDOW_ART.square;
  const [bw, bh] = o.fixedSize ? [w, h] : art.size(w, h);
  // A window of another size stands where the plain one did, with its sill in the same place.
  x += (w - bw) / 2;
  w = bw;
  h = bh;
  const t = -(z + h);
  if (!S.day) {
    const n = S.n++;
    if (ih(S.seed, n, 5) % 100 >= (o.lit ?? B.glowPct)) return;
    const gc = B.glowColor;
    shapePath(g, o.style, x, t, w, h, 0);
    g.save();
    g.clip();
    if (o.style === 'stained') {
      stainedPanes(g, x, t, w, h, true);
    } else {
      rr(g, grad(g, 0, t, 0, t + h, gc ? lite(gc, 0.55) : '#fff0b8', gc || '#ffc260'), x, t, w, h);
      art.bars(g, x, t, w, h, 'rgba(120,70,20,.55)', 0.67);
      if (o.curtain && o.curtain.style === 'open') rr(g, 'rgba(255,255,255,.35)', x, t, w * 0.32, h * 0.7);
      if (o.curtain && o.curtain.style === 'closed') rr(g, 'rgba(60,30,10,.35)', x, t, w, h);
      if (o.curtain && o.curtain.style === 'blinds') CURTAIN_ART.blinds(g, x, t, w, h, 'rgba(120,70,20,.5)');
    }
    g.restore();
    return;
  }
  const trim = B.trim;
  const fr = o.frame;
  const sw = o.shutters;
  // Shutters sit behind the frame.
  if (sw && SHUTTER_ART[sw.style]) {
    const sh = Math.max(1.8, w * 0.42);
    SHUTTER_ART[sw.style](g, x - 1 - sh, t - 0.6, sh, h + 1.2, sw.color);
    SHUTTER_ART[sw.style](g, x + w + 1, t - 0.6, sh, h + 1.2, sw.color);
  }
  if (o.style === 'square' || o.style === 'tall' || o.style === 'wide' || o.style === 'paned' || o.style === 'picture' || o.style === 'diamond') rr(g, 'rgba(25,15,35,.3)', x - 1.2, t - 1, w + 2.4, h + 2.4);
  else fillShape(g, o.style, 'rgba(25,15,35,.3)', x, t, w, h, 1.2);
  if (fr) fillShape(g, o.style, fr, x, t, w, h, trim.fr);
  const glass = o.glass;
  g.save();
  shapePath(g, o.style, x, t, w, h, 0);
  g.clip();
  if (o.style === 'stained') {
    rr(g, '#2a2638', x, t, w, h);
    stainedPanes(g, x, t, w, h, false);
    fp(g, 'rgba(255,255,255,.22)', [[x + w * 0.1, t], [x + w * 0.5, t], [x + w * 0.1, t + h * 0.7]]);
  } else {
    rr(g, grad(g, 0, t, 0, t + h, glass ? lite(glass, 0.4) : GLASS_TOP, glass || '#8ab8d6', glass ? dark(glass, 0.25) : GLASS_BOT), x, t, w, h);
    if (o.curtain && CURTAIN_ART[o.curtain.style]) CURTAIN_ART[o.curtain.style](g, x, t, w, h, o.curtain.color);
    fp(g, 'rgba(255,255,255,.3)', [[x + w * 0.1, t], [x + w * 0.5, t], [x + w * 0.1, t + h * 0.7]]);
    art.bars(g, x, t, w, h, fr || '#3b4256');
  }
  g.restore();
  if (o.style !== 'round' && trim.sill > 0) {
    const sillW = w + 3.6 + (trim.sill - 1) * 1.4;
    rr(g, lite(fr || TRIM, 0.1), x - sillW / 2 + w / 2, -z, sillW, 0.5 + trim.sill);
    rr(g, 'rgba(25,15,35,.28)', x - sillW / 2 + w / 2 + 0.4, -z + 0.5 + trim.sill, sillW - 0.8, 1);
  }
  if (fr && trim.lintel > 0 && o.style !== 'round' && o.style !== 'arched' && o.style !== 'stained') {
    rr(g, dark(fr, 0.2), x - 1.4 - (trim.lintel - 1) * 0.7, t - 1.8 - (trim.lintel - 1) * 0.6, w + 2.8 + (trim.lintel - 1) * 1.4, 0.9 + (trim.lintel - 1) * 0.9);
    if (B.trimStyle === 'gingerbread') fp(g, fr, [[x - 1, t - 3], [x + w / 2, t - 5.4], [x + w + 1, t - 3]]);
  }
  if (o.box && BOX_ART[o.box]) {
    rr(g, '#7a4b2a', x - 1.4, -z + 1.5, w + 2.8, 2.6);
    rr(g, '#9a6a40', x - 1.4, -z + 1.5, w + 2.8, 0.8);
    BOX_ART[o.box](g, x, z, w);
  }
}

// ---------------------------------------------------------------------------------------------
// Doors (E10), painted into the plane of the wall. `col` is the door's colour and `fr` the colour of the
// frame round it, or null with no trim.

function doorStep(g, x, w) {
  rr(g, '#d5cebd', x - 2.4, -1.5, w + 4.8, 1.5);
  rr(g, '#ebe5d4', x - 2.4, -2, w + 4.8, 0.6);
}

function knob(g, x, y) {
  circle(g, '#e8c860', x, y, 0.6);
}

const DOOR_ART = {
  panel(B, x, w, h, col, fr) {
    const { g } = B;
    doorStep(g, x, w);
    if (fr) rr(g, fr, x - 1.4, -h - 1.4, w + 2.8, h + 1.4);
    rr(g, grad(g, x, 0, x + w, 0, lite(col, 0.12), dark(col, 0.12)), x, -h, w, h - 1.5);
    const pw = w / 2 - 1.2;
    rr(g, dark(col, 0.22), x + 0.8, -h + 1, pw, h * 0.34);
    rr(g, dark(col, 0.22), x + w / 2 + 0.4, -h + 1, pw, h * 0.34);
    rr(g, dark(col, 0.2), x + 0.8, -h * 0.55, pw, h * 0.35);
    rr(g, dark(col, 0.2), x + w / 2 + 0.4, -h * 0.55, pw, h * 0.35);
    rr(g, grad(g, 0, -h + 1, 0, -h + 4, GLASS_TOP, GLASS_BOT), x + 1, -h + 1.4, w - 2, 2.6);
    knob(g, x + w - 1.4, -h * 0.45);
    rr(g, 'rgba(25,15,35,.3)', x - 0.2, -h - 0.4, w + 0.4, 0.7);
  },
  glass(B, x, w, h, col, fr) {
    const { g } = B;
    doorStep(g, x, w);
    if (fr) rr(g, fr, x - 1.4, -h - 1.4, w + 2.8, h + 1.4);
    rr(g, col, x, -h, w, h - 1.5);
    rr(g, grad(g, 0, -h, 0, -2, GLASS_TOP, '#8ab8d6', GLASS_BOT), x + 0.8, -h + 0.8, w - 1.6, h - 3.8);
    fp(g, 'rgba(255,255,255,.3)', [[x + 1, -h + 0.8], [x + w * 0.55, -h + 0.8], [x + 1, -h * 0.45]]);
    rr(g, col, x + 0.8, -h * 0.5, w - 1.6, 0.7);
    rr(g, lite(col, 0.5), x + w - 1.7, -h * 0.62, 0.6, 3.2);
  },
  arched(B, x, w, h, col, fr) {
    const { g } = B;
    doorStep(g, x, w);
    const r = w / 2;
    const cx = x + r;
    const path = (grow) => {
      g.beginPath();
      g.moveTo(x - grow, -1.5);
      g.lineTo(x - grow, -h + r);
      g.arc(cx, -h + r, r + grow, Math.PI, 0);
      g.lineTo(x + w + grow, -1.5);
      g.closePath();
    };
    if (fr) {
      path(1.3);
      g.fillStyle = fr;
      g.fill();
    }
    path(0);
    g.fillStyle = grad(g, x, 0, x + w, 0, lite(col, 0.12), dark(col, 0.14));
    g.fill();
    g.save();
    path(0);
    g.clip();
    rr(g, grad(g, 0, -h, 0, -h + r + 1, GLASS_TOP, GLASS_BOT), x + 0.8, -h + 0.8, w - 1.6, r + 0.4);
    line(g, col, 0.6, cx, -h + 0.8, cx, -h + r + 1);
    rr(g, dark(col, 0.22), x + 0.8, -h * 0.55, w - 1.6, h * 0.5 - 1.8);
    rr(g, lite(col, 0.12), x + 1.2, -h * 0.55 + 0.4, w - 2.4, h * 0.5 - 2.6);
    g.restore();
    knob(g, x + w - 1.2, -h * 0.4);
  },
  double(B, x, w, h, col, fr) {
    const { g } = B;
    doorStep(g, x, w);
    if (fr) rr(g, fr, x - 1.4, -h - 1.4, w + 2.8, h + 1.4);
    for (const k of [0, 1]) {
      const lx = x + k * (w / 2);
      rr(g, grad(g, lx, 0, lx + w / 2, 0, lite(col, 0.12), dark(col, 0.12)), lx, -h, w / 2 - (k ? 0 : 0.1), h - 1.5);
      rr(g, dark(col, 0.22), lx + 0.9, -h + 1, w / 2 - 1.9, h * 0.3);
      rr(g, dark(col, 0.2), lx + 0.9, -h * 0.55, w / 2 - 1.9, h * 0.35);
      rr(g, grad(g, 0, -h + 1, 0, -h + 4, GLASS_TOP, GLASS_BOT), lx + 1.2, -h + 1.4, w / 2 - 2.4, 2.4);
    }
    rr(g, 'rgba(25,15,35,.55)', x + w / 2 - 0.2, -h, 0.4, h - 1.5);
    knob(g, x + w / 2 - 1.1, -h * 0.45);
    knob(g, x + w / 2 + 1.1, -h * 0.45);
  },
  barn(B, x, w, h, col, fr) {
    const { g } = B;
    // A sliding door hangs from a rail.
    rr(g, '#4a5160', x - 1.5, -h - 1.6, w + 3, 0.9);
    for (const rx of [x + 1, x + w - 1.6]) rr(g, '#2a2f3e', rx, -h - 1.9, 0.9, 1.4);
    if (fr) rr(g, fr, x - 0.4, -h - 0.6, w + 0.8, h - 0.9);
    rr(g, dark(col, 0.35), x, -h, w, h - 0.5);
    for (let bx = x + 0.3; bx < x + w - 0.3; bx += 1.2) {
      rr(g, mixHex(col, ih(Math.round(bx * 4), 3, 2) % 2 ? '#ffffff' : '#000000', 0.08), bx, -h + 0.3, 1, h - 1.1);
    }
    const brace = fr || lite(col, 0.55);
    rr(g, brace, x, -h, w, 0.8);
    rr(g, brace, x, -1.3, w, 0.8);
    rr(g, brace, x, -h, 0.8, h - 0.5);
    rr(g, brace, x + w - 0.8, -h, 0.8, h - 0.5);
    line(g, brace, 0.8, x + 0.4, -h + 0.4, x + w - 0.4, -1);
    line(g, brace, 0.8, x + w - 0.4, -h + 0.4, x + 0.4, -1);
    rr(g, '#23262e', x + w - 2, -h * 0.5, 0.9, 2.6);
  },
  round(B, x, w, h, col) {
    const { g } = B;
    const cx = x + w / 2;
    const cy = -h / 2;
    ell(g, '#8f8a7e', cx, cy, w / 2 + 1.5, h / 2 + 1.5);
    ell(g, '#b9b4a6', cx, cy - 0.3, w / 2 + 1.2, h / 2 + 1.2);
    g.save();
    g.beginPath();
    g.ellipse(cx, cy, w / 2, h / 2, 0, 0, TAU);
    g.clip();
    rr(g, grad(g, x, 0, x + w, 0, lite(col, 0.15), col, dark(col, 0.25)), x, cy - h / 2, w, h);
    for (let k = 1; k < 6; k++) line(g, 'rgba(25,15,35,.28)', 0.4, x + (w * k) / 6, cy - h / 2, x + (w * k) / 6, cy + h / 2);
    rr(g, '#2a2f3e', x, cy - h * 0.2, w, 0.8);
    rr(g, '#2a2f3e', x, cy + h * 0.2, w, 0.8);
    g.restore();
    circle(g, '#e8c860', cx + w * 0.2, cy + 0.3, 0.8);
    circle(g, '#fff0a0', cx + w * 0.2 - 0.2, cy, 0.3);
  },
  dutch(B, x, w, h, col, fr) {
    const { g } = B;
    doorStep(g, x, w);
    if (fr) rr(g, fr, x - 1.4, -h - 1.4, w + 2.8, h + 1.4);
    const split = -h * 0.5;
    // The top half stands open on a dark room.
    rr(g, '#2a2236', x, -h, w, h * 0.5);
    rr(g, grad(g, 0, -h, 0, split, 'rgba(255,214,150,.4)', 'rgba(255,214,150,0)'), x, -h, w, h * 0.5);
    rr(g, dark(col, 0.2), x, -h, 0.9, h * 0.5);
    rr(g, dark(col, 0.1), x + w - 0.9, -h, 0.9, h * 0.5);
    rr(g, grad(g, x, 0, x + w, 0, lite(col, 0.12), dark(col, 0.12)), x, split, w, h * 0.5 - 1.5);
    rr(g, dark(col, 0.22), x + 0.9, split + 1, w - 1.8, h * 0.5 - 3.5);
    rr(g, lite(col, 0.4), x - 0.4, split - 0.5, w + 0.8, 0.9);
    knob(g, x + w - 1.2, split + 1.4);
  },
  sliding(B, x, w, h, col) {
    const { g } = B;
    rr(g, '#8a90a0', x - 1, -1.3, w + 2, 1.3);
    rr(g, col, x - 0.6, -h - 0.6, w + 1.2, h - 0.7);
    for (const k of [0, 1]) {
      const px = x + k * (w / 2 - 0.2);
      rr(g, grad(g, 0, -h, 0, -1.5, GLASS_TOP, '#8ab8d6', GLASS_BOT), px + 0.3, -h + 0.2, w / 2 + 0.1, h - 2.1);
      rr(g, k ? dark(col, 0.1) : col, px, -h, 0.5, h - 1.6);
      rr(g, k ? dark(col, 0.1) : col, px + w / 2 - 0.1, -h, 0.5, h - 1.6);
    }
    fp(g, 'rgba(255,255,255,.28)', [[x + 1, -h + 0.4], [x + w * 0.4, -h + 0.4], [x + 1, -h * 0.4]]);
    rr(g, lite(col, 0.5), x + w / 2 - 1.1, -h * 0.55, 0.5, 2.6);
    rr(g, lite(col, 0.5), x + w / 2 + 0.6, -h * 0.55, 0.5, 2.6);
  },
};

// A door's size next to the kind's own door.
const DOOR_SIZE = {
  panel: (w, h) => [w, h],
  glass: (w, h) => [w, h],
  arched: (w, h) => [w, h * 1.08],
  double: (w, h) => [w * 1.9, h],
  barn: (w, h) => [w * 1.5, h],
  round: (w, h) => [Math.max(6, w * 1.45), Math.max(6, h * 0.78)],
  dutch: (w, h) => [w, h],
  sliding: (w, h) => [w * 1.8, h],
};

// ---------------------------------------------------------------------------------------------
// Roofing (E10): what covers a sloped roof, painted into one plane of it. `top` and `edge` are each a pair of
// points, the two ends of the ridge side and the two ends of the eave side, so a plane can be a quad or
// a triangle with the ridge side squeezed to a point.

function openPlane(g, base, top, edge, bright, fill = true) {
  const [t0, t1] = top;
  const [e0, e1] = edge;
  g.save();
  pathOf(g, [P3(t0), P3(t1), P3(e1), P3(e0)]);
  g.clip();
  if (fill) {
    const a = P3(t0);
    const b = P3(e0);
    g.fillStyle = grad(g, a[0], a[1], b[0], b[1], lite(base, 0.16 + bright), base, dark(base, 0.14));
    g.fillRect(-120, -120, 320, 320);
  }
  return {
    pt: (s, t) => P3([0, 1, 2].map((k) => lerp(lerp(t0[k], t1[k], s), lerp(e0[k], e1[k], s), t))),
    along: Math.max(Math.hypot(t1[0] - t0[0], t1[1] - t0[1]), Math.hypot(e1[0] - e0[0], e1[1] - e0[1])),
  };
}

function seg(g, color, w, a, b) {
  line(g, color, w, a[0], a[1], b[0], b[1]);
}

const ROOFING_ART = {
  shingles(B, base, top, edge, rows, bright) {
    mapSlope(B.g, base, top, edge, rows, 3.2, bright);
  },
  tiles(B, base, top, edge, rows, bright) {
    const { g } = B;
    const q = openPlane(g, base, top, edge, bright);
    const cols = Math.max(3, Math.round(q.along / 2.8));
    const rand = seeded(B.S.seed + 7);
    for (let r = rows - 1; r >= 0; r--) {
      const ta = r / rows;
      const tb = (r + 1) / rows;
      const off = r % 2 ? 0.5 : 0;
      for (let c = -1; c < cols; c++) {
        const sa = (c + off) / cols;
        const sb = (c + 1 + off) / cols;
        const a = q.pt(sa, ta);
        const b = q.pt(sb, ta);
        const d = q.pt(sa, tb);
        const e = q.pt(sb, tb);
        const m = q.pt((sa + sb) / 2, tb + 0.42 / rows);
        g.beginPath();
        g.moveTo(a[0], a[1]);
        g.lineTo(b[0], b[1]);
        g.lineTo(e[0], e[1]);
        g.quadraticCurveTo(m[0], m[1], d[0], d[1]);
        g.closePath();
        g.fillStyle = mixHex(base, rand() < 0.5 ? lite(base, 0.3) : dark(base, 0.22), 0.1 + rand() * 0.45);
        g.fill();
        stroke(g, 'rgba(40,12,8,.4)', 0.5);
      }
    }
    g.restore();
  },
  slate(B, base, top, edge, rows, bright) {
    const { g } = B;
    const q = openPlane(g, base, top, edge, bright);
    const rand = seeded(B.S.seed + 11);
    for (let r = 0; r < rows; r++) {
      const ta = r / rows;
      const tb = (r + 1) / rows;
      let s = -rand() * 0.04;
      while (s < 1) {
        const ws = (1.9 + rand() * 1.2) / q.along;
        const a = q.pt(s, ta);
        const b = q.pt(s + ws, ta);
        const d = q.pt(s, tb);
        const e = q.pt(s + ws, tb);
        fp(g, mixHex(base, rand() < 0.5 ? lite(base, 0.25) : dark(base, 0.28), 0.1 + rand() * 0.5), [a, b, e, d]);
        seg(g, 'rgba(15,18,28,.45)', 0.45, d, e);
        seg(g, 'rgba(15,18,28,.3)', 0.4, a, d);
        seg(g, 'rgba(255,255,255,.18)', 0.4, a, b);
        s += ws;
      }
    }
    g.restore();
  },
  metal(B, base, top, edge, rows, bright) {
    const { g } = B;
    const q = openPlane(g, base, top, edge, bright + 0.04);
    const n = Math.max(3, Math.round(q.along / 3));
    for (let k = 0; k <= n; k++) {
      const s = k / n;
      seg(g, 'rgba(15,20,30,.45)', 0.8, q.pt(s, 0), q.pt(s, 1));
      seg(g, 'rgba(255,255,255,.4)', 0.5, q.pt(Math.min(1, s + 0.012), 0), q.pt(Math.min(1, s + 0.012), 1));
    }
    fp(g, 'rgba(255,255,255,.1)', [q.pt(0, 0), q.pt(0.5, 0), q.pt(0.2, 1), q.pt(0, 1)]);
    g.restore();
  },
  thatch(B, base, top, edge, rows, bright) {
    const { g } = B;
    const q = openPlane(g, base, top, edge, bright);
    const rand = seeded(B.S.seed + 13);
    const bands = Math.round(rows * 1.4);
    for (let r = 0; r < bands; r++) {
      const t = (r + 0.5) / bands;
      seg(g, mixHex(base, dark(base, 0.4), 0.2 + rand() * 0.3), 2.2, q.pt(0, t), q.pt(1, t));
    }
    for (let i = 0; i < q.along * rows * 1.3; i++) {
      const s = rand();
      const t = rand();
      seg(g, rand() < 0.5 ? lite(base, 0.3 + rand() * 0.2) : dark(base, 0.2 + rand() * 0.2), 0.5, q.pt(s, t), q.pt(Math.min(1, s + (rand() - 0.5) * 0.04), Math.min(1, t + 0.07)));
    }
    g.restore();
  },
  shakes(B, base, top, edge, rows, bright) {
    const { g } = B;
    const q = openPlane(g, base, top, edge, bright);
    const rand = seeded(B.S.seed + 17);
    for (let r = rows - 1; r >= 0; r--) {
      const ta = r / rows;
      const tb = (r + 1) / rows;
      let s = -rand() * 0.05;
      while (s < 1) {
        const ws = (1 + rand() * 1.5) / q.along;
        const drop = 0.1 + rand() * 0.18;
        const a = q.pt(s, ta);
        const b = q.pt(s + ws, ta);
        const d = q.pt(s, Math.min(1, tb + drop / rows));
        const e = q.pt(s + ws, Math.min(1, tb + drop / rows));
        fp(g, mixHex(base, rand() < 0.5 ? lite(base, 0.3) : dark(base, 0.3), 0.1 + rand() * 0.55), [a, b, e, d]);
        seg(g, 'rgba(20,10,5,.5)', 0.45, a, d);
        seg(g, 'rgba(20,10,5,.4)', 0.5, d, e);
        s += ws;
      }
    }
    g.restore();
  },
  moss(B, base, top, edge, rows, bright) {
    mapSlope(B.g, base, top, edge, rows, 3.2, bright);
    const { g } = B;
    const q = openPlane(g, base, top, edge, bright, false);
    const rand = seeded(B.S.seed + 19);
    for (let i = 0; i < 16; i++) {
      const [x, y] = q.pt(rand(), Math.pow(rand(), 0.7));
      const r = 1.2 + rand() * 2.8;
      ell(g, `rgba(${70 + Math.floor(rand() * 30)},${130 + Math.floor(rand() * 40)},${50 + Math.floor(rand() * 20)},.62)`, x, y, r * 1.5, r * 0.8);
      ell(g, 'rgba(160,210,100,.35)', x - r * 0.3, y - r * 0.2, r * 0.8, r * 0.4);
    }
    g.restore();
  },
  solar(B, base, top, edge, rows, bright) {
    const { g } = B;
    const q = openPlane(g, base, top, edge, bright);
    const n = Math.max(4, Math.round(q.along / 2.6));
    const m = Math.max(4, rows);
    for (let k = 0; k <= n; k++) seg(g, 'rgba(150,175,215,.6)', 0.5, q.pt(k / n, 0), q.pt(k / n, 1));
    for (let k = 0; k <= m; k++) seg(g, 'rgba(150,175,215,.6)', 0.5, q.pt(0, k / m), q.pt(1, k / m));
    for (let k = 0; k <= n; k += 3) seg(g, 'rgba(225,235,255,.7)', 0.9, q.pt(k / n, 0), q.pt(k / n, 1));
    fp(g, 'rgba(255,255,255,.16)', [q.pt(0.1, 0), q.pt(0.45, 0), q.pt(0.15, 1), q.pt(0, 1)]);
    g.restore();
  },
};

// What a roofing is when its owner hasn't chosen the roof's colour. `null` leaves the map's own roof colour.
const ROOFING_COLOR = { shingles: null, tiles: '#b5532f', slate: '#5f6978', metal: null, thatch: '#c9a95c', shakes: '#8b6a47', moss: null, solar: '#1f2f55' };

// The eave board along an edge. Thatch has no board, but a thick soft roll of straw.
function eaveBoard(B, a, b, tone = 0) {
  const { g } = B;
  const h = B.trim.fascia;
  if (B.roofing === 'thatch') {
    const pa = P3(a);
    const pb = P3(b);
    const n = Math.max(3, Math.round(Math.hypot(pb[0] - pa[0], pb[1] - pa[1]) / 1.5));
    const rand = seeded(B.S.seed + Math.round(a[0] * 3 + a[1]));
    for (let i = 0; i <= n; i++) {
      const x = lerp(pa[0], pb[0], i / n);
      const y = lerp(pa[1], pb[1], i / n);
      ell(g, mixHex(B.roofBase, dark(B.roofBase, 0.4), tone + rand() * 0.4), x, y + 0.5 + rand() * 0.8, 1.5, 2 + rand() * 0.7);
      ell(g, lite(B.roofBase, 0.1 + rand() * 0.15 - tone), x - 0.2, y - 0.1 + rand() * 0.5, 1.1, 1.3);
    }
    return;
  }
  const col = B.trimStyle === 'none' ? dark(B.roofBase, 0.32) : mixHex(B.fascia, '#241f3c', tone);
  fp3(g, col, a, b, [b[0], b[1], b[2] - h], [a[0], a[1], a[2] - h]);
  if (B.trimStyle !== 'none') fp3(g, 'rgba(30,20,50,.26)', [a[0], a[1], a[2] - h], [b[0], b[1], b[2] - h], [b[0], b[1], b[2] - h - 1], [a[0], a[1], a[2] - h - 1]);
  if (B.trim.bracket) {
    const pa = P3(a);
    const pb = P3(b);
    const n = Math.max(2, Math.round(Math.hypot(pb[0] - pa[0], pb[1] - pa[1]) / 1.8));
    for (let i = 0; i < n; i++) {
      const x = lerp(pa[0], pb[0], (i + 0.5) / n);
      const y = lerp(pa[1], pb[1], (i + 0.5) / n) + h + 0.4;
      ell(g, col, x, y, 0.85, 1.1);
      circle(g, lite(col, 0.3), x, y + 0.7, 0.35);
    }
  }
}

// A pitched roof's side edge, as the dark board along a rake.
function rakeBoard(B, a, b) {
  const h = B.roofing === 'thatch' ? B.trim.fascia + 1.2 : B.trim.fascia;
  fp3(B.g, dark(B.roofBase, B.roofing === 'thatch' ? 0.2 : 0.38), a, b, [b[0], b[1], b[2] - h], [a[0], a[1], a[2] - h]);
}

// The wall's own material filling a gable end, whatever shape the end is. `poly` is in the end's plane.
function endWall(B, onPlane, plane, poly, z0, z1, seed, shade) {
  const { g } = B;
  onPlane(g, plane[0], plane[1], () => {
    g.save();
    g.beginPath();
    g.moveTo(poly[0][0], poly[0][1]);
    for (let i = 1; i < poly.length; i++) g.lineTo(poly[i][0], poly[i][1]);
    g.closePath();
    g.clip();
    paintWall(g, plane[2], z0, z1, B.mat, seed);
    if (shade) shadeFace(g, plane[2], z0, z1);
    g.restore();
  });
}

function surfRoof(B, base, top, edge, rows, bright = 0) {
  (ROOFING_ART[B.roofing] || ROOFING_ART.shingles)(B, base, top, edge, rows, bright);
}

const riseFor = (B, half) => Math.round(clamp(half * (9 / 7) * (B.k.pk ?? 1) * ((B.pitch ?? 2) / 2), 3, 24) * 100) / 100;

// A roof's own report of itself, so what stands on it, hangs from it or goes through it knows where to be.
// `eave` is the polyline of the roof's visible edge (a string of lights hangs from it).

// --- Roof shapes (E10). Each takes a rectangle, the height of its walls and how far it overhangs. ---

const ROOF_ART = {
  gable(B, R, zE, o, axis = 'u') {
    const { g } = B;
    const [u0, v0, u1, v1] = R;
    const base = B.roofBase;
    const along = axis === 'u';
    const half = (along ? v1 - v0 : u1 - u0) / 2;
    const rise = riseFor(B, half);
    const zR = zE + rise;
    const rows = Math.max(5, Math.round(rise * 0.9));
    const gablePoly = (len) => [[-0.5, -zE + 0.5], [len + 0.5, -zE + 0.5], [len / 2, -zR - 0.6]];
    const vent = (len, lit) => {
      if (rise < 5) return;
      rr(g, 'rgba(30,20,50,.4)', len / 2 - 2, -zE - rise * 0.45, 4, 5);
      rr(g, lit, len / 2 - 1.4, -zE - rise * 0.45 + 0.6, 2.8, 3.8);
    };
    let info;
    if (along) {
      const vm = (v0 + v1) / 2;
      const ve = v1 + o;
      const zA = (v) => zE + (v1 - v) * (rise / (v1 - vm));
      endWall(B, onR, [u1, v1, v1 - v0], gablePoly(v1 - v0), zE - 1, zR + 1, 41, true);
      onR(g, u1, v1, () => vent(v1 - v0, 'rgba(255,255,255,.25)'));
      surfRoof(B, base, [[u0 - o, vm, zR], [u1 + o, vm, zR]], [[u0 - o, ve, zA(ve)], [u1 + o, ve, zA(ve)]], rows);
      rakeBoard(B, [u1 + o, vm, zR], [u1 + o, ve, zA(ve)]);
      eaveBoard(B, [u0 - o, ve, zA(ve)], [u1 + o, ve, zA(ve)]);
      line(g, lite(base, 0.45), 1.2, ...P(u0 - o, vm, zR), ...P(u1 + o, vm, zR));
      info = {
        zTop: zR,
        ridge: [[u0 - o, vm, zR], [u1 + o, vm, zR]],
        slope: { top: [[u0 - o, vm, zR], [u1 + o, vm, zR]], edge: [[u0 - o, ve, zA(ve)], [u1 + o, ve, zA(ve)]], face: 'L' },
        eave: [[u0 - o, ve, zA(ve)], [u1 + o, ve, zA(ve)], [u1 + o, vm, zR]],
        chim: [u0 + Math.round(((u1 - u0) * 1100) / 18) / 100, vm - 4, zR - 3, zR + 4],
        lowZ: zA(ve),
      };
    } else {
      const um = (u0 + u1) / 2;
      const ue = u1 + o;
      const zA = (u) => zE + (u1 - u) * (rise / (u1 - um));
      endWall(B, onL, [u0, v1, u1 - u0], gablePoly(u1 - u0), zE - 1, zR + 1, 43, false);
      onL(g, u0, v1, () => vent(u1 - u0, 'rgba(255,255,255,.3)'));
      surfRoof(B, dark(base, 0.14), [[um, v0 - o, zR], [um, v1 + o, zR]], [[ue, v0 - o, zA(ue)], [ue, v1 + o, zA(ue)]], rows);
      rakeBoard(B, [um, v1 + o, zR], [ue, v1 + o, zA(ue)]);
      eaveBoard(B, [ue, v0 - o, zA(ue)], [ue, v1 + o, zA(ue)], 0.12);
      line(g, lite(base, 0.3), 1.2, ...P(um, v0 - o, zR), ...P(um, v1 + o, zR));
      info = {
        zTop: zR,
        ridge: [[um, v0 - o, zR], [um, v1 + o, zR]],
        slope: { top: [[um, v0 - o, zR], [um, v1 + o, zR]], edge: [[ue, v0 - o, zA(ue)], [ue, v1 + o, zA(ue)]], face: 'R' },
        eave: [[ue, v0 - o, zA(ue)], [ue, v1 + o, zA(ue)], [um, v1 + o, zR]],
        chim: [um - 4 - 3.4, v0 + (v1 - v0) * 0.2, zR - 3, zR + 4],
        lowZ: zA(ue),
      };
    }
    return info;
  },

  cross_gable(B, R, zE, o) {
    return ROOF_ART.gable(B, R, zE, o, 'v');
  },

  hip(B, R, zE, o) {
    const { g } = B;
    const [u0, v0, u1, v1] = R;
    const base = B.roofBase;
    const wu = u1 - u0;
    const wv = v1 - v0;
    const rise = riseFor(B, Math.min(wu, wv) / 2);
    const zR = zE + rise;
    const rows = Math.max(5, Math.round(rise * 0.9));
    const dz = zE - o * (rise / (Math.min(wu, wv) / 2));
    let ra;
    let rb;
    if (wu >= wv) {
      ra = [u0 + wv / 2, (v0 + v1) / 2];
      rb = [u1 - wv / 2, (v0 + v1) / 2];
    } else {
      ra = [(u0 + u1) / 2, v0 + wu / 2];
      rb = [(u0 + u1) / 2, v1 - wu / 2];
    }
    const eFL = [u0 - o, v1 + o, dz];
    const eFR = [u1 + o, v1 + o, dz];
    const eBR = [u1 + o, v0 - o, dz];
    const a3 = [ra[0], ra[1], zR];
    const b3 = [rb[0], rb[1], zR];
    if (wu >= wv) {
      surfRoof(B, base, [a3, b3], [eFL, eFR], rows);
      surfRoof(B, dark(base, 0.16), [b3, b3], [eFR, eBR], rows);
    } else {
      surfRoof(B, base, [b3, b3], [eFL, eFR], rows);
      surfRoof(B, dark(base, 0.16), [b3, a3], [eFR, eBR], rows);
    }
    eaveBoard(B, eFL, eFR);
    eaveBoard(B, eFR, eBR, 0.2);
    line(g, lite(base, 0.4), 1.1, ...P(...a3), ...P(...b3));
    const top = wu >= wv ? [a3, b3] : [b3, b3];
    return {
      zTop: zR,
      ridge: [a3, b3],
      slope: { top, edge: [eFL, eFR], face: 'L' },
      eave: [eFL, eFR, eBR],
      chim: [(ra[0] + rb[0]) / 2 + 1.5, (ra[1] + rb[1]) / 2 - 4, zR - 3, zR + 4],
      lowZ: dz,
    };
  },

  pyramid(B, R, zE, o) {
    const { g } = B;
    const [u0, v0, u1, v1] = R;
    const base = B.roofBase;
    const rise = riseFor(B, Math.min(u1 - u0, v1 - v0) / 2) * 1.05;
    const zR = zE + rise;
    const rows = Math.max(5, Math.round(rise * 0.9));
    const dz = zE - o * (rise / (Math.min(u1 - u0, v1 - v0) / 2));
    const apex = [(u0 + u1) / 2, (v0 + v1) / 2, zR];
    const eFL = [u0 - o, v1 + o, dz];
    const eFR = [u1 + o, v1 + o, dz];
    const eBR = [u1 + o, v0 - o, dz];
    surfRoof(B, base, [apex, apex], [eFL, eFR], rows);
    surfRoof(B, dark(base, 0.16), [apex, apex], [eFR, eBR], rows);
    eaveBoard(B, eFL, eFR);
    eaveBoard(B, eFR, eBR, 0.2);
    line(g, lite(base, 0.35), 0.8, ...P(...apex), ...P(...eFL));
    line(g, lite(base, 0.2), 0.8, ...P(...apex), ...P(...eFR));
    return {
      zTop: zR,
      ridge: null,
      apex,
      slope: { top: [apex, apex], edge: [eFL, eFR], face: 'L' },
      eave: [eFL, eFR, eBR],
      chim: [apex[0] + 2, apex[1] - 5, zR - 4, zR + 3],
      lowZ: dz,
    };
  },

  flat(B, R, zE, o, extra = {}, spec = {}) {
    const { g } = B;
    const [u0, v0, u1, v1] = R;
    const t = extra.parapet ?? B.k.parapet ?? (B.trimStyle === 'bold' ? 3.2 : B.trimStyle === 'none' ? 1.8 : 2.5);
    const c = B.roofHex;
    const cope = c ? lite(c, 0.32) : B.k.coping || null;
    onL(g, u0, v1, () => {
      rr(g, grad(g, 0, -zE - t, 0, -zE, lite(cope || '#e8e2d2', 0.1), cope || '#d8d1c0'), -0.3, -zE - t, u1 - u0 + 0.6, t);
      rr(g, 'rgba(25,15,35,.24)', 0, -zE - 0.8, u1 - u0, 0.8);
    });
    // Where another block of the building stands against this wall, there is no parapet.
    const skip = spec.cut ? v1 - spec.cut.R[0] : 0;
    onR(g, u1, v1, () => {
      rr(g, grad(g, 0, -zE - t, 0, -zE, c ? dark(cope, 0.1) : '#c8c1af', c ? dark(cope, 0.24) : '#aaa391'), skip - 0.3, -zE - t, v1 - v0 - skip + 0.6, t);
      rr(g, 'rgba(36,26,70,.34)', skip - 0.3, -zE - t, v1 - v0 - skip + 0.6, t);
    });
    const zt = zE + t;
    fp3(g, c ? lite(c, 0.45) : '#e5dfce', [u0, v0, zt], [u1, v0, zt], [u1, v1, zt], [u0, v1, zt]);
    fp3(g, grad(g, ...P(u0, v0, zt), ...P(u1, v1, zt), c ? dark(c, 0.05) : '#747b8c', c ? dark(c, 0.22) : '#5d6474'), [u0 + 1, v0 + 1, zt], [u1 - 1, v0 + 1, zt], [u1 - 1, v1 - 1, zt], [u0 + 1, v1 - 1, zt]);
    fp3(g, 'rgba(25,20,40,.3)', [u0 + 1, v0 + 1, zt], [u1 - 1, v0 + 1, zt], [u1 - 1, v0 + 2.6, zt], [u0 + 1, v0 + 2.6, zt]);
    fp3(g, 'rgba(25,20,40,.3)', [u0 + 1, v0 + 1, zt], [u0 + 2.6, v0 + 1, zt], [u0 + 2.6, v1 - 1, zt], [u0 + 1, v1 - 1, zt]);
    return {
      zTop: zt,
      deck: { R: [u0 + 1, v0 + 1, u1 - 1, v1 - 1], z: zt },
      sh: zE + (B.k.shade ?? (B.kind === 'studio' ? 6 : 2)),
      eave: [[u0, v1, zt], [u1, v1, zt], [u1, v0, zt]],
      chim: [u1 - 7, v0 + 3, zt - 0.5, zt + 9],
      lowZ: zt,
    };
  },

  shed(B, R, zE, o, axis, spec = {}) {
    const { g } = B;
    const [u0, v0, u1, v1] = R;
    const base = B.roofBase;
    // On an L every block shares the one slope.
    const ref = spec.ref || R;
    const rise = riseFor(B, (ref[3] - ref[1]) / 2) * 0.9;
    const k = rise / (ref[3] - ref[1]);
    const zR = zE + (v1 - v0) * k;
    const ve = v1 + o;
    const zA = (v) => zE + (v1 - v) * k;
    const rows = Math.max(5, Math.round(rise * 0.9));
    endWall(B, onR, [u1, v1, v1 - v0], [[-0.5, -zE + 0.5], [v1 - v0 + 0.5, -zE + 0.5], [v1 - v0 + 0.5, -zR - 0.6]], zE - 1, zR + 1, 45, true);
    surfRoof(B, base, [[u0 - o, v0 - o, zA(v0 - o)], [u1 + o, v0 - o, zA(v0 - o)]], [[u0 - o, ve, zA(ve)], [u1 + o, ve, zA(ve)]], rows);
    rakeBoard(B, [u1 + o, v0 - o, zA(v0 - o)], [u1 + o, ve, zA(ve)]);
    eaveBoard(B, [u0 - o, ve, zA(ve)], [u1 + o, ve, zA(ve)]);
    line(g, lite(base, 0.45), 1.1, ...P(u0 - o, v0 - o, zA(v0 - o)), ...P(u1 + o, v0 - o, zA(v0 - o)));
    return {
      zTop: zA(v0 - o),
      ridge: [[u0 - o, v0 - o, zA(v0 - o)], [u1 + o, v0 - o, zA(v0 - o)]],
      slope: { top: [[u0 - o, v0 - o, zA(v0 - o)], [u1 + o, v0 - o, zA(v0 - o)]], edge: [[u0 - o, ve, zA(ve)], [u1 + o, ve, zA(ve)]], face: 'L' },
      eave: [[u0 - o, ve, zA(ve)], [u1 + o, ve, zA(ve)], [u1 + o, v0 - o, zA(v0 - o)]],
      chim: [u0 + (u1 - u0) * 0.62, v0 + 1.5, zR - 2.5, zR + 4],
      lowZ: zA(ve),
    };
  },

  mansard(B, R, zE, o) {
    const { g } = B;
    const [u0, v0, u1, v1] = R;
    const base = B.roofBase;
    const p = B.pitch ?? 2;
    const small = Math.min(u1 - u0, v1 - v0);
    const hb = Math.min(5.5 + p * 1.6, small * 0.7);
    const m = Math.min(hb * 0.42, small / 2 - 1.5);
    const zb = zE + hb;
    const dz = zE - o * (hb / m);
    const rows = Math.max(4, Math.round(hb * 0.8));
    const I = [u0 + m, v0 + m, u1 - m, v1 - m];
    const eFL = [u0 - o, v1 + o, dz];
    const eFR = [u1 + o, v1 + o, dz];
    const eBR = [u1 + o, v0 - o, dz];
    const iFL = [I[0], I[3], zb];
    const iFR = [I[2], I[3], zb];
    const iBR = [I[2], I[1], zb];
    surfRoof(B, base, [iFL, iFR], [eFL, eFR], rows);
    surfRoof(B, dark(base, 0.16), [iFR, iBR], [eFR, eBR], rows);
    eaveBoard(B, eFL, eFR);
    eaveBoard(B, eFR, eBR, 0.2);
    // The shallow top, a low hip above the steep band.
    const cap = Math.min(2 + p * 0.5, (Math.min(I[2] - I[0], I[3] - I[1]) / 2) * 0.9);
    const cx = (I[0] + I[2]) / 2;
    const cy = (I[1] + I[3]) / 2;
    const apex = [cx, cy, zb + cap];
    const hw = (I[2] - I[0]) / 2;
    const hd = (I[3] - I[1]) / 2;
    if (hw >= hd) {
      const ra = [I[0] + hd, cy, zb + cap];
      const rb = [I[2] - hd, cy, zb + cap];
      surfRoof(B, lite(base, 0.04), [ra, rb], [iFL, iFR], 3, 0.04);
      surfRoof(B, dark(base, 0.12), [rb, rb], [iFR, iBR], 3);
      line(g, lite(base, 0.4), 0.9, ...P(...ra), ...P(...rb));
    } else {
      const ra = [cx, I[1] + hw, zb + cap];
      const rb = [cx, I[3] - hw, zb + cap];
      surfRoof(B, lite(base, 0.04), [rb, rb], [iFL, iFR], 3, 0.04);
      surfRoof(B, dark(base, 0.12), [rb, ra], [iFR, iBR], 3);
      line(g, lite(base, 0.4), 0.9, ...P(...ra), ...P(...rb));
    }
    // Dormer windows stand out of the steep band.
    const zD = zE + hb * 0.42;
    const f = (zD - dz) / (zb - dz);
    const vD = lerp(v1 + o, I[3], f);
    const uD = lerp(u1 + o, I[2], f);
    const wuD = uD - lerp(u0 - o, I[0], f);
    const nF = wuD > 22 ? 3 : wuD > 11 ? 2 : 1;
    for (let k = 0; k < nF; k++) dormerOn(B, lerp(u0 + 1, u1 - 1, (k + 0.5) / nF) , vD, zD, 'L');
    const wvD = (vD - lerp(v0 - o, I[1], f));
    const nR = wvD > 22 ? 3 : wvD > 11 ? 2 : 1;
    for (let k = 0; k < nR; k++) dormerOn(B, uD, lerp(v1 - 1, v0 + 1, (k + 0.5) / nR), zD, 'R');
    return {
      zTop: zb + cap,
      apex,
      deck: null,
      slope: { top: [iFL, iFR], edge: [eFL, eFR], face: 'L' },
      eave: [eFL, eFR, eBR],
      chim: [cx + 1, cy - 2, zb, zb + cap + 6],
      lowZ: dz,
      band: true,
    };
  },

  gambrel(B, R, zE, o) {
    const { g } = B;
    const [u0, v0, u1, v1] = R;
    const base = B.roofBase;
    const half = (v1 - v0) / 2;
    const rise = riseFor(B, half) * 0.95;
    const vm = (v0 + v1) / 2;
    const r1 = rise * 0.62;
    const kd = half * 0.34;
    const zR = zE + rise;
    const ve = v1 + o;
    const zl = zE - o * (r1 / kd);
    const rows = Math.max(4, Math.round(rise * 0.55));
    const len = v1 - v0;
    endWall(B, onR, [u1, v1, len], [[-0.5, -zE + 0.5], [kd, -zE - r1], [half, -zR - 0.6], [len - kd, -zE - r1], [len + 0.5, -zE + 0.5]], zE - 1, zR + 1, 47, true);
    surfRoof(B, base, [[u0 - o, v1 - kd, zE + r1], [u1 + o, v1 - kd, zE + r1]], [[u0 - o, ve, zl], [u1 + o, ve, zl]], rows, -0.02);
    surfRoof(B, lite(base, 0.03), [[u0 - o, vm, zR], [u1 + o, vm, zR]], [[u0 - o, v1 - kd, zE + r1], [u1 + o, v1 - kd, zE + r1]], Math.max(3, rows - 1), 0.06);
    rakeBoard(B, [u1 + o, ve, zl], [u1 + o, v1 - kd, zE + r1]);
    rakeBoard(B, [u1 + o, v1 - kd, zE + r1], [u1 + o, vm, zR]);
    eaveBoard(B, [u0 - o, ve, zl], [u1 + o, ve, zl]);
    line(g, lite(base, 0.4), 0.8, ...P(u0 - o, v1 - kd, zE + r1), ...P(u1 + o, v1 - kd, zE + r1));
    line(g, lite(base, 0.45), 1.2, ...P(u0 - o, vm, zR), ...P(u1 + o, vm, zR));
    return {
      zTop: zR,
      ridge: [[u0 - o, vm, zR], [u1 + o, vm, zR]],
      slope: { top: [[u0 - o, vm, zR], [u1 + o, vm, zR]], edge: [[u0 - o, v1 - kd, zE + r1], [u1 + o, v1 - kd, zE + r1]], face: 'L' },
      eave: [[u0 - o, ve, zl], [u1 + o, ve, zl], [u1 + o, v1 - kd, zE + r1], [u1 + o, vm, zR]],
      chim: [u0 + (u1 - u0) * 0.611, vm - 3, zR - 3, zR + 4],
      lowZ: zl,
    };
  },

  dome(B, R, zE, o, extra = {}) {
    const { g } = B;
    const [u0, v0, u1, v1] = R;
    const base = B.roofBase;
    const deck = ROOF_ART.flat(B, R, zE, o, { parapet: extra.parapet ?? 1.8 });
    const zt = deck.zTop;
    const r = Math.min(u1 - u0, v1 - v0) / 2 - 1.4;
    const [cx, cy] = P((u0 + u1) / 2, (v0 + v1) / 2, zt);
    const rx = r * 1.414;
    const ry = r * 0.707;
    const hd = r * (0.62 + 0.14 * (B.pitch ?? 2));
    const dome = () => {
      g.beginPath();
      g.moveTo(cx + rx, cy);
      g.ellipse(cx, cy, rx, ry, 0, 0, Math.PI);
      g.ellipse(cx, cy, rx, hd, 0, Math.PI, TAU);
      g.closePath();
    };
    // A low drum for the dome to sit on.
    dome();
    const lg = g.createRadialGradient(cx - rx * 0.4, cy - hd * 0.75, 1, cx - rx * 0.1, cy - hd * 0.3, rx * 1.35);
    lg.addColorStop(0, lite(base, 0.5));
    lg.addColorStop(0.45, base);
    lg.addColorStop(1, dark(base, 0.38));
    g.fillStyle = lg;
    g.fill();
    g.save();
    dome();
    g.clip();
    const rand = seeded(B.S.seed + 23);
    const style = B.roofing;
    const rings = style === 'thatch' ? 7 : 10;
    for (let k = 1; k < rings; k++) {
      const f = k / rings;
      const sq = Math.sqrt(1 - f * f);
      g.beginPath();
      g.ellipse(cx, cy - hd * f, rx * sq, ry * sq, 0, 0, Math.PI);
      if (style === 'thatch') stroke(g, rgba(dark(base, 0.45), 0.5), 1.8);
      else if (style === 'tiles') stroke(g, 'rgba(40,12,8,.45)', 0.9);
      else if (style === 'metal' || style === 'solar') stroke(g, 'rgba(15,20,30,.14)', 0.4);
      else stroke(g, 'rgba(30,14,20,.3)', 0.6);
    }
    const ribs = style === 'metal' || style === 'solar' ? 14 : 8;
    for (let k = 0; k < ribs; k++) {
      const a = (k / ribs) * TAU;
      const ex = cx + rx * Math.cos(a);
      const ey = cy + ry * Math.sin(a);
      g.beginPath();
      g.moveTo(cx, cy - hd);
      g.quadraticCurveTo(cx + (ex - cx) * 0.92, cy - hd * 0.5 + (ey - cy) * 0.3, ex, ey);
      stroke(g, style === 'solar' ? 'rgba(150,175,215,.55)' : style === 'metal' ? 'rgba(255,255,255,.28)' : 'rgba(30,14,20,.2)', 0.5);
    }
    if (style === 'moss') for (let k = 0; k < 9; k++) ell(g, 'rgba(90,150,60,.55)', cx + (rand() - 0.5) * rx * 1.5, cy - rand() * hd * 0.9, 2 + rand() * 3, 1 + rand() * 1.4);
    ell(g, 'rgba(255,255,255,.16)', cx - rx * 0.35, cy - hd * 0.6, rx * 0.28, hd * 0.2);
    g.restore();
    // A finial on top.
    rr(g, lite(base, 0.3), cx - 0.4, cy - hd - 3, 0.8, 3);
    circle(g, '#e8c860', cx, cy - hd - 3.2, 0.9);
    return {
      zTop: zt + hd + 3,
      deck: { R: [u0 + 1, v0 + 1, u1 - 1, v1 - 1], z: zt, dome: { x: cx, y: cy, rx, ry } },
      apexPt: [cx, cy - hd - 3],
      eave: deck.eave,
      chim: [u1 - 5, v0 + 3, zt - 0.5, zt + 9],
      lowZ: zt,
    };
  },

  sawtooth(B, R, zE) {
    const { g } = B;
    const [u0, v0, u1, v1] = R;
    const base = B.roofBase;
    const wu = u1 - u0;
    const n = clamp(Math.round(wu / 6), 2, 5);
    const tw = wu / n;
    const rise = clamp(4.5 + (B.pitch ?? 2) * 2, 6, 11);
    const zR = zE + rise;
    const rows = Math.max(4, Math.round(rise * 0.8));
    // The wall under the teeth, so the profile shows along the front.
    onL(g, u0, v1, () => {
      g.save();
      for (let k = 0; k < n; k++) {
        const a = k * tw;
        g.beginPath();
        g.moveTo(a, -zE + 0.4);
        g.lineTo(a + tw, -zE + 0.4);
        g.lineTo(a + tw, -zR);
        g.closePath();
        g.save();
        g.clip();
        paintWall(g, wu, zE - 1, zR + 1, B.mat, 49);
        g.restore();
      }
      g.restore();
    });
    for (let k = 0; k < n; k++) {
      const a = u0 + k * tw;
      const b = a + tw;
      surfRoof(B, k % 2 ? dark(base, 0.06) : base, [[b, v0, zR], [b, v1, zR]], [[a, v0, zE], [a, v1, zE]], rows);
      // The glazed face looks to the right, and the teeth behind it throw their shade on it.
      const gl = (t) => [[b, v0, zE + (zR - zE) * t], [b, v1, zE + (zR - zE) * t]];
      fp3(g, grad(g, ...P(b, v0, zR), ...P(b, v0, zE), '#cfeaf7', '#8fc0de', '#5c8fb6'), ...gl(0), ...gl(1).reverse());
      for (let s = 0; s <= (v1 - v0) / 3; s++) {
        const v = v0 + s * 3;
        if (v > v1) break;
        line(g, 'rgba(40,60,85,.75)', 0.6, ...P(b, v, zE), ...P(b, v, zR));
      }
      line(g, 'rgba(40,60,85,.75)', 0.7, ...P(b, v0, zE + rise * 0.5), ...P(b, v1, zE + rise * 0.5));
      fp3(g, 'rgba(255,255,255,.22)', [b, v0 + 1, zR - 1], [b, v0 + 5, zR - 1], [b, v0 + 2.4, zE + 1], [b, v0 + 1, zE + 1]);
      fp3(g, 'rgba(36,26,70,.34)', ...gl(0), ...gl(1).reverse());
      line(g, lite(base, 0.4), 1, ...P(b, v0, zR), ...P(b, v1, zR));
    }
    const mid = Math.floor(n / 2);
    return {
      zTop: zR,
      deck: null,
      ridge: null,
      slope: { top: [[u0 + (mid + 1) * tw, v0, zR], [u0 + (mid + 1) * tw, v1, zR]], edge: [[u0 + mid * tw, v0, zE], [u0 + mid * tw, v1, zE]], face: 'R' },
      eave: [[u0, v1, zE], [u1, v1, zE], [u1, v1, zR], [u1, v0, zR]],
      chim: [u1 - 5, v0 + 2, zR - 1, zR + 9],
      lowZ: zE,
      teeth: true,
    };
  },
};

// A dormer on the steep band of a mansard roof, or on any slope: a small box with its own little roof.
function dormerOn(B, u, v, z, facing, mini = false) {
  const { S } = B;
  const w = mini ? 3 : 3.6;
  const d = 2.8;
  const h = mini ? 3.2 : 3.8;
  const wall = { k: 'siding', c: B.trimHex ? lite(B.trimHex, 0.1) : '#efe8d6' };
  if (facing === 'L') {
    const Rd = [u - w / 2, v - d, u + w / 2, v];
    box(S, Rd, z - 1.2, z + h, wall, (side, len) => {
      if (side === 'L') windowArt(B, len / 2 - 1.1, 1.2 + 1.2, 2.2, 2.4, { style: 'square', frame: B.trimHex || TRIM, glass: B.glassColor, curtain: null, shutters: null, box: null });
    });
    mapGable(S, Rd, z + h, 2.2, B.roofBase, 0.5, 'v', wall);
  } else {
    const Rd = [u, v - w / 2, u + d, v + w / 2];
    box(S, Rd, z - 1.2, z + h, wall, (side, len) => {
      if (side === 'R') windowArt(B, len / 2 - 1.1, 1.2 + 1.2, 2.2, 2.4, { style: 'square', frame: B.trimHex || TRIM, glass: B.glassColor, curtain: null, shutters: null, box: null });
    });
    mapGable(S, Rd, z + h, 2.2, B.roofBase, 0.5, 'u', wall);
  }
}

// ---------------------------------------------------------------------------------------------
// Chimneys (E10). `at` is where the roof says one can stand: [u, v, z0, z1].

function smokeAt(S, x, y) {
  if (S.info) S.info.smoke.push([Math.round(x), Math.round(y)]);
}

function cylinder(g, x, y0, y1, rx, ry, col) {
  g.beginPath();
  g.moveTo(x - rx, y0);
  g.lineTo(x - rx, y1);
  g.ellipse(x, y1, rx, ry, 0, Math.PI, 0, true);
  g.lineTo(x + rx, y0);
  g.closePath();
  g.fillStyle = grad(g, x - rx, 0, x + rx, 0, lite(col, 0.3), col, dark(col, 0.35));
  g.fill();
}

const CHIMNEY_ART = {
  none() {},
  brick(B, at, col, smoke) {
    const [u, v, z0, z1, wide] = at;
    const w = wide || 3.4;
    if (!col) {
      brickChimney(B.S, u, v, z0, z1, w, smoke);
      return;
    }
    chimneyBox(B, u, v, z0, z1, w, { k: 'brick', c: col }, smoke);
  },
  stone(B, at, col, smoke) {
    const [u, v, z0, z1] = at;
    const c = col || '#9d988c';
    chimneyBox(B, u, v, z0, z1, 3.8, { k: 'stone', c, paint: paintStone(c) }, smoke);
  },
  metal(B, at, col, smoke) {
    if (!B.S.day) return;
    const { g, S } = B;
    const [u, v, z0, z1] = at;
    const c = col || '#aab3c2';
    const [x, y] = P(u + 1.4, v + 1.4, z0);
    const top = y - (z1 - z0) - 2;
    ell(g, 'rgba(25,25,35,.25)', x + 1.6, y + 0.4, 3, 1.2);
    cylinder(g, x, top, y, 1.9, 0.95, c);
    for (const f of [0.3, 0.7]) rr(g, 'rgba(25,25,35,.3)', x - 1.9, lerp(top, y, f), 3.8, 0.6);
    // A rain hat over the flue.
    fp(g, dark(c, 0.15), [[x - 3, top - 1.4], [x + 3, top - 1.4], [x, top - 4.4]]);
    fp(g, lite(c, 0.25), [[x - 3, top - 1.4], [x, top - 1.4], [x, top - 4.4]]);
    rr(g, dark(c, 0.4), x - 0.3, top - 1.4, 0.6, 1.6);
    if (smoke) smokeAt(S, x, top - 5);
    shadow(S, [u + 0.6, v + 0.6, u + 2.2, v + 2.2], z1 - z0);
  },
  pipe(B, at, col, smoke) {
    if (!B.S.day) return;
    const { g, S } = B;
    const [u, v, z0, z1] = at;
    const c = col || '#2d3039';
    const [x, y] = P(u + 1, v + 1, z0);
    const top = y - (z1 - z0) + 1;
    cylinder(g, x, top, y, 1, 0.5, c);
    rr(g, lite(c, 0.35), x - 0.5, top, 0.4, y - top);
    for (const f of [0.35, 0.65]) rr(g, dark(c, 0.3), x - 1.3, lerp(top, y, f), 2.6, 0.7);
    ell(g, dark(c, 0.15), x, y - 0.2, 1.8, 0.8);
    fp(g, c, [[x - 2, top - 0.6], [x + 2, top - 0.6], [x, top - 2.6]]);
    rr(g, lite(c, 0.3), x - 0.2, top - 0.4, 0.4, 1.2);
    if (smoke) smokeAt(S, x, top - 3);
    shadow(S, [u + 0.4, v + 0.4, u + 1.6, v + 1.6], z1 - z0);
  },
};

function chimneyBox(B, u, v, z0, z1, w, mat, smoke) {
  if (!B.S.day) return;
  const { g, S } = B;
  const R = [u, v, u + w, v + w];
  box(S, R, z0, z1, mat);
  fp3(g, '#7a6a62', [u - 0.8, v - 0.8, z1], [u + w + 0.8, v - 0.8, z1], [u + w + 0.8, v + w + 0.8, z1], [u - 0.8, v + w + 0.8, z1]);
  fp3(g, '#2b2528', [u + 0.6, v + 0.6, z1], [u + w - 0.6, v + 0.6, z1], [u + w - 0.6, v + w - 0.6, z1], [u + 0.6, v + w - 0.6, z1]);
  fp3(g, '#9a8d86', [u - 0.8, v + w + 0.8, z1 - 1.2], [u + w + 0.8, v + w + 0.8, z1 - 1.2], [u + w + 0.8, v + w + 0.8, z1], [u - 0.8, v + w + 0.8, z1]);
  if (smoke) {
    const [x, y] = P(u + w / 2, v + w / 2, z1);
    smokeAt(S, x, y);
  }
  shadow(S, R, z1 - z0);
}

// ---------------------------------------------------------------------------------------------
// On the roof (E10). Each entry paints one thing where the roof says it can stand. `info` is what the
// roof reported of itself: a ridge or an apex, a front slope, or a flat deck. `col` is the part's colour.

// Where a thing stands on a flat deck, as a fraction across it. Each has a place of its own, so any number of
// them can be on at once without sitting on each other.
const DECK_SPOT = {
  antenna: [0.1, 0.16],
  weathervane: [0.36, 0.16],
  water_tank: [0.62, 0.18],
  flag: [0.12, 0.5],
  skylights: [0.5, 0.52],
  dish: [0.88, 0.5],
  ac_unit: [0.16, 0.88],
  solar_panels: [0.55, 0.86],
  dormers: [0.9, 0.88],
};
// And on a ridge, as a fraction along it. Anything on a roof with no ridge stands near the top of the slope.
const RIDGE_SPOT = { weathervane: 0.36, antenna: 0.2, flag: 0.08, water_tank: 0.5, dish: 0.94 };
// A slope is a rectangle, along the ridge and down it. A thing that lies on it takes a patch of that.
const SLOPE_PATCH = {
  solar_panels: [0.06, 0.4, 0.3, 0.84],
  skylights: [0.72, 0.9, 0.3, 0.58],
  dormers: [0.46, 0.64, 0.3, 0.7],
};
const SLOPE_SPOT = { ac_unit: [0.8, 0.84], dish: [0.94, 0.7], roof_garden: [0.5, 0.94] };

function slopePt(info, s, t) {
  const { top, edge } = info.slope;
  return mid3(mid3(top[0], top[1], s), mid3(edge[0], edge[1], s), t);
}

function ridgeLength(info) {
  if (!info.ridge) return 0;
  const [a, b] = info.ridge;
  return Math.hypot(a[0] - b[0], a[1] - b[1]);
}

// A spot to stand on, in 3D, for a thing that stands up.
function spotFor(info, word) {
  if (info.deck) {
    const { R, z } = info.deck;
    const m = 1.5;
    const [fu, fv] = DECK_SPOT[word];
    return [lerp(R[0] + m, R[2] - m, fu), lerp(R[1] + m, R[3] - m, fv), z];
  }
  const rs = RIDGE_SPOT[word];
  if (rs !== undefined && ridgeLength(info) > 4) return mid3(info.ridge[0], info.ridge[1], rs);
  if (!info.slope) return null;
  if (SLOPE_SPOT[word]) return slopePt(info, SLOPE_SPOT[word][0], SLOPE_SPOT[word][1]);
  // No ridge to stand on: near the top of the front slope, spread along it.
  const s = rs === undefined ? 0.5 : clamp(rs, 0.12, 0.88);
  return slopePt(info, s, 0.22);
}

const scaleOf = (info) => (info.deck ? clamp(Math.min(info.deck.R[2] - info.deck.R[0], info.deck.R[3] - info.deck.R[1]) / 13, 0.55, 1.1) : 1);

// Four corners of a patch of the front slope, in 3D.
function patchOf(info, word, shrink = 0) {
  const [s0, s1, t0, t1] = SLOPE_PATCH[word];
  return [slopePt(info, s0 + shrink, t0), slopePt(info, s1 - shrink, t0), slopePt(info, s1 - shrink, t1), slopePt(info, s0 + shrink, t1)];
}

// A box on the deck for the things that are small blocks of metal or wood.
function deckBox(B, p, w, d, h, mat) {
  box(B.S, [p[0] - w / 2, p[1] - d / 2, p[0] + w / 2, p[1] + d / 2], p[2], p[2] + h, mat);
  topFace(B.S, [p[0] - w / 2, p[1] - d / 2, p[0] + w / 2, p[1] + d / 2], p[2] + h, lite(mat.c, 0.2));
}

const ROOFTOP_ART = {
  weathervane(B, info, col) {
    const { g } = B;
    const c = col || '#3a3f4a';
    if (info.apexPt) return weatherAt(g, info.apexPt[0], info.apexPt[1] - 1, c);
    const p = spotFor(info, 'weathervane');
    if (!p) return;
    const [x, y] = P3(p);
    weatherAt(g, x, y, c);
  },
  antenna(B, info, col) {
    const { g } = B;
    const p = spotFor(info, 'antenna');
    if (!p) return;
    const [x, y] = P3(p);
    const c = col || '#8f96a8';
    const k = scaleOf(info);
    line(g, c, 0.9, x, y, x, y - 10 * k);
    for (const [f, hw] of [[0.45, 4], [0.65, 3], [0.82, 2]]) line(g, c, 0.7, x - hw * k, y - 10 * k * f, x + hw * k, y - 10 * k * f);
    line(g, dark(c, 0.3), 0.5, x, y, x - 3 * k, y + 1.2);
    circle(g, '#d9433f', x, y - 10 * k - 0.4, 0.5);
  },
  dish(B, info, col) {
    const { g } = B;
    const p = spotFor(info, 'dish');
    if (!p) return;
    const [x, y] = P3(p);
    const c = col || '#e4e8f0';
    const k = scaleOf(info);
    line(g, '#6c7384', 0.9, x, y, x, y - 3 * k);
    ell(g, 'rgba(25,25,35,.25)', x + 1, y + 0.4, 2.4 * k, 0.9);
    ell(g, dark(c, 0.3), x, y - 4.4 * k, 2.8 * k, 2.4 * k);
    ell(g, c, x - 0.2, y - 4.6 * k, 2.5 * k, 2.1 * k);
    ell(g, dark(c, 0.15), x - 0.1, y - 4.5 * k, 1.2 * k, 1 * k);
    line(g, '#6c7384', 0.6, x - 0.1, y - 4.5 * k, x + 2.2 * k, y - 7 * k);
    circle(g, '#3a3f4a', x + 2.3 * k, y - 7.1 * k, 0.55);
  },
  solar_panels(B, info, col) {
    const { g } = B;
    if (info.deck) {
      const p = spotFor(info, 'solar_panels');
      const k = scaleOf(info);
      const w = 8 * k;
      const d = 2.6 * k;
      for (const row of [0, 1]) {
        const v = p[1] - row * (d + 0.8);
        const a = [p[0] - w / 2, v + d / 2, p[2] + 0.5];
        const b = [p[0] + w / 2, v + d / 2, p[2] + 0.5];
        const c3 = [p[0] + w / 2, v - d / 2, p[2] + 2.3];
        const d3 = [p[0] - w / 2, v - d / 2, p[2] + 2.3];
        fp3(g, '#c9ced8', a, b, c3, d3);
        const inset = (t) => [mid3(a, d3, t), mid3(b, c3, t)];
        fp3(g, grad(g, ...P3(d3), ...P3(a), '#355a9a', '#18305e'), mid3(a, d3, 0.1), mid3(b, c3, 0.1), mid3(b, c3, 0.9), mid3(a, d3, 0.9));
        for (let q = 1; q < 4; q++) line(g, 'rgba(180,200,235,.55)', 0.4, ...P3(mid3(a, b, q / 4)), ...P3(mid3(d3, c3, q / 4)));
        line(g, 'rgba(180,200,235,.55)', 0.4, ...P3(inset(0.5)[0]), ...P3(inset(0.5)[1]));
        line(g, '#6c7384', 0.7, ...P3(a), ...P3([a[0], a[1], p[2]]));
        line(g, '#6c7384', 0.7, ...P3(b), ...P3([b[0], b[1], p[2]]));
      }
      return;
    }
    if (!info.slope) return;
    const q = patchOf(info, 'solar_panels');
    fp3(g, '#c9ced8', ...q);
    const at = (s, t) => mid3(mid3(q[0], q[1], s), mid3(q[3], q[2], s), t);
    for (let r = 0; r < 2; r++) {
      for (let c = 0; c < 2; c++) {
        fp3(g, '#1f3a6e', at(c / 2 + 0.03, r / 2 + 0.04), at((c + 1) / 2 - 0.03, r / 2 + 0.04), at((c + 1) / 2 - 0.03, (r + 1) / 2 - 0.04), at(c / 2 + 0.03, (r + 1) / 2 - 0.04));
        for (let k = 1; k < 3; k++) line(g, 'rgba(180,200,235,.55)', 0.35, ...P3(at(c / 2 + (0.5 * k) / 3, r / 2 + 0.04)), ...P3(at(c / 2 + (0.5 * k) / 3, (r + 1) / 2 - 0.04)));
      }
    }
    fp3(g, 'rgba(255,255,255,.2)', at(0.05, 0.05), at(0.4, 0.05), at(0.2, 0.5), at(0.05, 0.5));
  },
  skylights(B, info, col) {
    const { g } = B;
    const glass = (a, b, c3, d3) => {
      fp3(g, '#e4e8f0', a, b, c3, d3);
      const m = 0.14;
      const ia = mid3(mid3(a, b, m), mid3(d3, c3, m), m);
      const ib = mid3(mid3(b, a, m), mid3(c3, d3, m), m);
      const ic = mid3(mid3(c3, d3, m), mid3(b, a, m), m);
      const id = mid3(mid3(d3, c3, m), mid3(a, b, m), m);
      fp3(g, grad(g, ...P3(ia), ...P3(ic), '#cfeaf7', '#8fc0de', '#5c8fb6'), ia, ib, ic, id);
      fp3(g, 'rgba(255,255,255,.3)', ia, mid3(ia, ib, 0.5), mid3(id, ic, 0.2), id);
    };
    if (info.deck) {
      const p = spotFor(info, 'skylights');
      const k = scaleOf(info);
      const w = 5.4 * k;
      const d = 4 * k;
      // A pitched pane of glass on a low curb.
      const a = [p[0] - w / 2, p[1] + d / 2, p[2]];
      const b = [p[0] + w / 2, p[1] + d / 2, p[2]];
      fp3(g, '#c9ced8', a, b, [b[0], b[1], p[2] + 1], [a[0], a[1], p[2] + 1]);
      glass([a[0], a[1], p[2] + 1], [b[0], b[1], p[2] + 1], [b[0], b[1] - d, p[2] + 2.6 * k + 1], [a[0], a[1] - d, p[2] + 2.6 * k + 1]);
      return;
    }
    if (!info.slope) return;
    for (const sh of [0, 0.5]) {
      const [s0, s1, t0, t1] = SLOPE_PATCH.skylights;
      const w = s1 - s0;
      const q = [slopePt(info, s0 - sh * w * 1.15, t0), slopePt(info, s0 - sh * w * 1.15 + w * 0.46, t0), slopePt(info, s0 - sh * w * 1.15 + w * 0.46, t1), slopePt(info, s0 - sh * w * 1.15, t1)];
      glass(...q);
    }
  },
  dormers(B, info) {
    const { g } = B;
    if (info.deck) {
      const p = spotFor(info, 'dormers');
      const k = scaleOf(info);
      const w = 4.4 * k;
      const d = 3.6 * k;
      const mat = { k: 'concrete', c: '#d6d1c4' };
      deckBox(B, p, w, d, 4.2 * k, mat);
      onL(g, p[0] - w / 2, p[1] + d / 2, () => {
        rr(g, '#4a4f5e', w * 0.28, -p[2] - 3.4 * k, w * 0.44, 3.4 * k);
        rr(g, 'rgba(255,255,255,.18)', w * 0.28, -p[2] - 3.4 * k, w * 0.1, 3.4 * k);
      });
      return;
    }
    if (!info.slope) return;
    const [s0, s1, t0, t1] = SLOPE_PATCH.dormers;
    const p = slopePt(info, (s0 + s1) / 2, (t0 + t1) / 2);
    dormerOn(B, p[0], p[1], p[2], info.slope.face === 'L' ? 'L' : 'R', true);
  },
  flag(B, info, col) {
    const { g, S } = B;
    const p = spotFor(info, 'flag');
    if (!p) return;
    const [x, y] = P3(p);
    const c = col || '#d9433f';
    const k = scaleOf(info);
    line(g, '#8f96a8', 0.9, x, y, x, y - 12 * k);
    circle(g, '#e8c860', x, y - 12.4 * k, 0.6);
    g.beginPath();
    g.moveTo(x + 0.4, y - 11.6 * k);
    g.quadraticCurveTo(x + 3.6 * k, y - 12.8 * k, x + 7 * k, y - 11 * k);
    g.quadraticCurveTo(x + 4 * k, y - 9.8 * k, x + 7 * k, y - 8.2 * k);
    g.quadraticCurveTo(x + 3.6 * k, y - 9.6 * k, x + 0.4, y - 8.2 * k);
    g.closePath();
    g.fillStyle = c;
    g.fill();
    fp(g, 'rgba(255,255,255,.25)', [[x + 0.4, y - 11.6 * k], [x + 3.6 * k, y - 12.2 * k], [x + 3.2 * k, y - 10.6 * k], [x + 0.4, y - 10.4 * k]]);
  },
  ac_unit(B, info, col) {
    const p = spotFor(info, 'ac_unit');
    if (!p) return;
    const k = scaleOf(info);
    const c = col || '#c9ced8';
    const w = 5 * k;
    const d = 4 * k;
    const R = [p[0] - w / 2, p[1] - d / 2, p[0] + w / 2, p[1] + d / 2];
    box(B.S, R, p[2], p[2] + 3.6 * k, { k: 'steel', c });
    topFace(B.S, R, p[2] + 3.6 * k, lite(c, 0.3));
    onL(B.g, R[0], R[3], () => {
      ell(B.g, '#3a3f50', w / 2, -p[2] - 1.8 * k, 1.7 * k, 1.5 * k);
      for (let q = 0; q < 4; q++) line(B.g, '#8a90a2', 0.5, w / 2 - 1.6 * k, -p[2] - 1.8 * k + q * 0.5 - 0.8, w / 2 + 1.6 * k, -p[2] - 1.8 * k + q * 0.5 - 0.8);
    });
  },
  roof_garden(B, info) {
    const { g } = B;
    if (info.deck) {
      const { R, z } = info.deck;
      // Planters along the front edges, inside the parapet.
      const lush = (u, v, w, d) => {
        const mat = { k: 'siding', c: '#8a6038' };
        box(B.S, [u, v, u + w, v + d], z, z + 1.5, mat);
        const rand = seeded(Math.round(u * 7 + v * 3));
        const n = Math.round((w + d) * 1.1);
        for (let q = 0; q < n; q++) {
          const [x, y] = P(u + rand() * w, v + rand() * d, z + 1.5);
          ell(g, ['#4f9a46', '#6dba5c', '#3d8a3a'][q % 3], x, y - 0.6, 1.1, 1);
          if (q % 4 === 0) circle(g, ['#ff7fa6', '#ffd75e', '#ffffff'][q % 3], x + 0.3, y - 1.4, 0.45);
        }
      };
      const span = R[2] - R[0];
      const inset = 0.4;
      lush(R[0] + 3, R[3] - 1.6 - inset, span * 0.4, 1.6);
      lush(R[2] - 1.6 - inset, R[1] + 3, 1.6, (R[3] - R[1]) * 0.4);
      return;
    }
    if (!info.slope) return;
    // Pots along the eave.
    for (const s of [0.18, 0.4, 0.62, 0.84]) {
      const p = slopePt(info, s, 0.93);
      const [x, y] = P3(p);
      ell(g, 'rgba(25,25,35,.2)', x + 0.8, y + 0.4, 1.8, 0.7);
      rr(g, '#a56a44', x - 1.3, y - 2.4, 2.6, 2.4);
      ell(g, '#b97a52', x, y - 2.4, 1.3, 0.55);
      ell(g, ['#4f9a46', '#6dba5c', '#3d8a3a'][Math.round(s * 5) % 3], x, y - 3.3, 1.7, 1.3);
      circle(g, ['#ff7fa6', '#ffd75e', '#ffffff'][Math.round(s * 7) % 3], x + 0.5, y - 4, 0.5);
    }
  },
  water_tank(B, info, col) {
    const { g } = B;
    const p = spotFor(info, 'water_tank');
    if (!p) return;
    const [x, y] = P3(p);
    const k = scaleOf(info);
    const c = col || '#a9744a';
    const rx = 2.6 * k;
    const ry = 1.3 * k;
    const legH = 3.2 * k;
    const tankH = 5 * k;
    ell(g, 'rgba(25,25,35,.25)', x + 2, y + 0.6, rx * 1.3, ry * 1.1);
    for (const dx of [-rx * 0.8, rx * 0.8]) line(g, '#5a4a3a', 0.9, x + dx, y, x + dx, y - legH);
    line(g, '#5a4a3a', 0.6, x - rx * 0.8, y - legH * 0.5, x + rx * 0.8, y - legH * 0.9);
    line(g, '#5a4a3a', 0.6, x + rx * 0.8, y - legH * 0.5, x - rx * 0.8, y - legH * 0.9);
    cylinder(g, x, y - legH - tankH, y - legH, rx, ry, c);
    for (const f of [0.25, 0.7]) {
      g.beginPath();
      g.ellipse(x, y - legH - tankH * f, rx, ry, 0, 0, Math.PI);
      stroke(g, 'rgba(30,20,10,.5)', 0.7);
    }
    fp(g, dark(c, 0.15), [[x - rx - 0.3, y - legH - tankH], [x + rx + 0.3, y - legH - tankH], [x, y - legH - tankH - 3 * k]]);
    fp(g, lite(c, 0.2), [[x - rx - 0.3, y - legH - tankH], [x, y - legH - tankH], [x, y - legH - tankH - 3 * k]]);
  },
};

function weatherAt(g, x, y, c) {
  line(g, c, 0.9, x, y, x, y - 9);
  line(g, c, 0.6, x - 2.4, y - 5.5, x + 2.4, y - 5.5);
  line(g, c, 0.6, x - 1.4, y - 5.5 - 1.4, x + 1.4, y - 5.5 + 1.4);
  for (const dx of [-2.6, 2.6]) circle(g, c, x + dx, y - 5.5, 0.45);
  fp(g, c, [[x - 3.6, y - 9.8], [x + 1, y - 9.8], [x + 1, y - 11.2], [x + 4.4, y - 9.1], [x + 1, y - 7], [x + 1, y - 8.4], [x - 3.6, y - 8.4]]);
  fp(g, lite(c, 0.4), [[x - 3.6, y - 9.8], [x + 1, y - 9.8], [x + 1, y - 9.2], [x - 3.6, y - 9.2]]);
  circle(g, '#e8c860', x, y - 9.1, 0.55);
}

// ---------------------------------------------------------------------------------------------
// Faces. A porch, an awning and a rail all run along a wall, so they're drawn in terms of how far along it
// (a) and how far out from it (d), and this turns that into the map's u and v for whichever wall it is.

function faceFrame(B, side, R) {
  const [u0, v0, u1, v1] = R;
  const map = side === 'L' ? (a, d, z) => [u0 + a, v1 + d, z] : (a, d, z) => [u1 + d, v1 - a, z];
  const rect = side === 'L' ? (a0, a1, d0, d1) => [u0 + a0, v1 + d0, u0 + a1, v1 + d1] : (a0, a1, d0, d1) => [u1 + d0, v1 - a1, u1 + d1, v1 - a0];
  return {
    side,
    map,
    pt: (a, d, z) => P3(map(a, d, z)),
    flat: (fill, a0, a1, d0, d1, z) => fp3(B.g, fill, map(a0, d0, z), map(a1, d0, z), map(a1, d1, z), map(a0, d1, z)),
    wall: (fill, a0, a1, d, z0, z1) => fp3(B.g, fill, map(a0, d, z0), map(a1, d, z0), map(a1, d, z1), map(a0, d, z1)),
    end: (fill, a, d0, d1, z0, z1) => fp3(B.g, fill, map(a, d0, z0), map(a, d1, z0), map(a, d1, z1), map(a, d0, z1)),
    box: (a0, a1, d0, d1, z0, z1, mat) => box(B.S, rect(a0, a1, d0, d1), z0, z1, mat),
    // The end that faces the viewer, as the a it's at.
    near: (a0, a1) => (side === 'L' ? a1 : a0),
  };
}

function post(B, F, a, d, z0, z1, col, w = 1.6) {
  const { g } = B;
  const [x, y] = F.pt(a, d, z0);
  rr(g, 'rgba(25,15,35,.18)', x + 0.3, y - 0.4, w, 0.9);
  rr(g, grad(g, x - w / 2, 0, x + w / 2, 0, lite(col, 0.25), col, dark(col, 0.25)), x - w / 2, y - (z1 - z0), w, z1 - z0);
  rr(g, dark(col, 0.2), x - w / 2 - 0.2, y - (z1 - z0), w + 0.4, 0.6);
  rr(g, dark(col, 0.2), x - w / 2 - 0.2, y - 0.8, w + 0.4, 0.8);
}

// Steps down from a platform, from depth d0 outward, as many as its height needs.
function stairs(B, F, a0, a1, d0, zTop, zBase, mat) {
  const n = Math.max(1, Math.round((zTop - zBase) / 0.85));
  for (let i = 1; i < n; i++) F.box(a0, a1, d0 + (i - 1) * 1.25, d0 + i * 1.25, zBase, zBase + (zTop - zBase) * (1 - i / n), mat);
}

const STONE_STEP = { k: 'concrete', c: '#cfc8b8' };

// --- Porches (E10). `door` is where the door is along the wall: { a0, a1 }. `z` is the wall's foot. ---

const PORCH_ART = {
  none() {},
  stoop(B, F, door, col, z) {
    const top = z + 1.2;
    const c = col || '#cfc8b8';
    F.box(door.a0 - 1.2, door.a1 + 1.2, 0, 3.4, 0, top, { k: 'concrete', c });
    stairs(B, F, door.a0 - 0.6, door.a1 + 0.6, 3.4, top, 0, { k: 'concrete', c: lite(c, 0.05) });
    // A low iron rail on either side.
    if (B.S.day) {
      for (const a of [door.a0 - 1, door.a1 + 1]) {
        const [x, y] = F.pt(a, 3, top);
        rr(B.g, '#2d3039', x - 0.3, y - 3.2, 0.6, 3.2);
        const [x2, y2] = F.pt(a, 0.6, top);
        line(B.g, '#2d3039', 0.6, x, y - 3, x2, y2 - 3);
      }
    }
  },
  covered(B, F, door, col, z) {
    const { g } = B;
    const c = col || '#fffaf0';
    const a0 = door.a0 - 1.2;
    const a1 = door.a1 + 1.2;
    const D = 5.2;
    const top = z + 1.4;
    F.box(a0, a1, 0, D, 0, top, { k: 'siding', c: '#b6a889' });
    stairs(B, F, door.a0, door.a1, D, top, 0, STONE_STEP);
    const zp = z + door.h + 1.6;
    const roof = dark(B.roofBase, 0.1);
    for (const a of [a0 + 0.3, a1 - 0.3]) post(B, F, a, D - 0.4, top, zp, c);
    // A lean-to roof sloping down from the wall.
    F.flat(roof, a0 - 1, a1 + 1, -0.4, D + 0.8, zp + 2.2);
    fp3(g, roof, F.map(a0 - 1, -0.4, zp + 2.2), F.map(a1 + 1, -0.4, zp + 2.2), F.map(a1 + 1, D + 0.8, zp), F.map(a0 - 1, D + 0.8, zp));
    F.wall(dark(roof, 0.3), a0 - 1, a1 + 1, D + 0.8, zp - 1.1, zp);
    F.end(dark(roof, 0.2), F.near(a0 - 1, a1 + 1), -0.4, D + 0.8, zp, zp + 2.2);
    if (B.trim.bracket) bracketsOn(B, F, [a0 + 0.3, a1 - 0.3], D - 0.4, zp, c);
  },
  columns(B, F, door, col, z) {
    const { g } = B;
    const c = col || B.trimHex || '#f7f1e3';
    const a0 = door.a0 - 2.4;
    const a1 = door.a1 + 2.4;
    const D = 5.6;
    const top = z + 1.4;
    F.box(a0, a1, 0, D, 0, top, { k: 'concrete', c: '#d8d2c2' });
    stairs(B, F, door.a0 - 0.6, door.a1 + 0.6, D, top, 0, STONE_STEP);
    const zp = z + door.h + 2.4;
    for (const a of [a0 + 1, a1 - 1]) {
      const [x, y] = F.pt(a, D - 0.8, top);
      cylinder(g, x, y - (zp - top), y, 1.15, 0.58, c);
      rr(g, dark(c, 0.15), x - 1.5, y - 0.9, 3, 0.9);
      rr(g, lite(c, 0.25), x - 1.5, y - (zp - top) - 0.6, 3, 0.9);
      for (const f of [-0.45, 0, 0.45]) line(g, 'rgba(25,15,35,.18)', 0.35, x + f, y - 1, x + f, y - (zp - top) + 0.4);
    }
    // The beam and the triangle above it.
    F.box(a0 - 0.4, a1 + 0.4, -0.2, D + 0.4, zp, zp + 1.6, { k: 'concrete', c: lite(c, 0.05) });
    const dFront = D + 0.4;
    const peak = zp + 1.6 + 3.4;
    F.wall(c, a0 - 0.4, a1 + 0.4, dFront, zp + 1.6, zp + 1.7);
    const mA = (a0 + a1) / 2;
    fp3(g, c, F.map(a0 - 0.4, dFront, zp + 1.6), F.map(a1 + 0.4, dFront, zp + 1.6), F.map(mA, dFront, peak));
    fp3(g, dark(c, 0.14), F.map(a0 + 1, dFront, zp + 2.1), F.map(a1 - 1, dFront, zp + 2.1), F.map(mA, dFront, peak - 1));
    line(g, lite(c, 0.4), 0.7, ...F.pt(a0 - 0.4, dFront, zp + 1.6), ...F.pt(mA, dFront, peak));
    line(g, dark(c, 0.3), 0.7, ...F.pt(a1 + 0.4, dFront, zp + 1.6), ...F.pt(mA, dFront, peak));
  },
  deck(B, F, door, col, z) {
    const { g } = B;
    const c = col || '#a9784a';
    const a0 = door.a0 - 4;
    const a1 = door.a1 + 4;
    const D = 6.6;
    const top = z + 1.6;
    F.box(a0, a1, 0, D, 0, top, { k: 'timber', c: dark(c, 0.15), paint: paintTimber(c) });
    F.flat(c, a0, a1, 0, D, top);
    for (let d = 0.9; d < D; d += 0.9) {
      const q = F.pt(a0, d, top);
      const r = F.pt(a1, d, top);
      line(g, 'rgba(40,22,10,.35)', 0.4, q[0], q[1], r[0], r[1]);
    }
    stairs(B, F, door.a0, door.a1, D, top, 0, { k: 'timber', c: dark(c, 0.1), paint: paintTimber(c) });
    // A rail round the edge, open in front of the steps.
    const rail = lite(c, 0.18);
    const baluster = (a, d) => {
      const [x, y] = F.pt(a, d, top);
      rr(g, rail, x - 0.25, y - 3.4, 0.5, 3.4);
    };
    const run = (fromA, fromD, toA, toD) => {
      const n = Math.max(2, Math.round(Math.hypot(toA - fromA, toD - fromD) / 1.3));
      for (let i = 0; i <= n; i++) baluster(lerp(fromA, toA, i / n), lerp(fromD, toD, i / n));
      line(g, rail, 0.9, ...F.pt(fromA, fromD, top + 3.4), ...F.pt(toA, toD, top + 3.4));
    };
    const farEnd = F.near(a0, a1) === a1 ? a0 : a1;
    run(farEnd, 0.2, farEnd, D - 0.2);
    run(a0 + 0.2, D - 0.2, door.a0 - 0.6, D - 0.2);
    run(door.a1 + 0.6, D - 0.2, a1 - 0.2, D - 0.2);
    run(F.near(a0, a1), D - 0.2, F.near(a0, a1), 0.2);
  },
  veranda(B, F, door, col, z, run) {
    const { g } = B;
    const c = col || '#f2eee2';
    const a0 = run[0];
    const a1 = run[1];
    const D = 5.4;
    const top = z + 1.4;
    F.box(a0, a1, 0, D, 0, top, { k: 'siding', c: '#b6a889' });
    stairs(B, F, door.a0, door.a1, D, top, 0, STONE_STEP);
    const zp = z + door.h + 1.8;
    const n = Math.max(1, Math.round((a1 - a0) / 5.5));
    const posts = [];
    for (let i = 0; i <= n; i++) posts.push(lerp(a0 + 0.5, a1 - 0.5, i / n));
    // Railing between the posts, with a gap where the steps come up.
    const rail = lite(c, 0.1);
    for (let i = 0; i < n; i++) {
      const pa = posts[i] + 0.8;
      const pb = posts[i + 1] - 0.8;
      for (let q = 0; q <= 5; q++) {
        const a = lerp(pa, pb, q / 5);
        if (a > door.a0 - 0.4 && a < door.a1 + 0.4) continue;
        const [x, y] = F.pt(a, D - 0.4, top);
        rr(g, rail, x - 0.25, y - 3, 0.5, 3);
      }
      line(g, rail, 0.9, ...F.pt(pa, D - 0.4, top + 3), ...F.pt(pb, D - 0.4, top + 3));
    }
    for (const a of posts) post(B, F, a, D - 0.4, top, zp, c);
    const roof = dark(B.roofBase, 0.1);
    fp3(g, roof, F.map(a0 - 0.8, -0.4, zp + 2.4), F.map(a1 + 0.8, -0.4, zp + 2.4), F.map(a1 + 0.8, D + 0.8, zp), F.map(a0 - 0.8, D + 0.8, zp));
    F.wall(dark(roof, 0.3), a0 - 0.8, a1 + 0.8, D + 0.8, zp - 1.1, zp);
    F.end(dark(roof, 0.2), F.near(a0 - 0.8, a1 + 0.8), -0.4, D + 0.8, zp, zp + 2.4);
    for (let i = 1; i < n * 2; i++) line(g, 'rgba(30,14,20,.2)', 0.5, ...F.pt(lerp(a0, a1, i / (n * 2)), -0.4, zp + 2.4), ...F.pt(lerp(a0, a1, i / (n * 2)), D + 0.8, zp));
    if (B.trim.bracket) bracketsOn(B, F, posts, D - 0.4, zp, c);
  },
};

// Scrolled brackets in the corners between each post and the beam.
function bracketsOn(B, F, posts, d, zp, c) {
  const { g } = B;
  for (const a of posts) {
    for (const s of [-1, 1]) {
      const [x, y] = F.pt(a, d, zp);
      g.beginPath();
      g.moveTo(x, y + 2.6);
      g.quadraticCurveTo(x + s * 0.4, y + 0.6, x + s * 2.6, y);
      g.lineTo(x + s * 2.6, y + 0.1);
      g.quadraticCurveTo(x + s * 0.9, y + 1.2, x + s * 0.7, y + 2.6);
      g.closePath();
      g.fillStyle = c;
      g.fill();
      circle(g, c, x + s * 1.2, y + 1.3, 0.55);
    }
  }
}

// --- Awnings (E10). Over a stretch of wall, sloping down and out. ---

function awningSlope(B, F, a0, a1, zTop, depth, fillAt) {
  const drop = 4;
  const pieces = Math.max(1, Math.round((a1 - a0) / 2.6));
  const sw = (a1 - a0) / pieces;
  for (let k = 0; k < pieces; k++) {
    const a = a0 + k * sw;
    const b = a + sw;
    fillAt(k, a, b, [F.map(a, 0, zTop), F.map(b, 0, zTop), F.map(b, depth, zTop - drop), F.map(a, depth, zTop - drop)]);
  }
}

const AWNING_ART = {
  none() {},
  plain(B, F, a0, a1, zTop, depth, [c1]) {
    const { g } = B;
    awningSlope(B, F, a0, a1, zTop, depth, (k, a, b, q) => {
      fp3(g, grad(g, ...P3(q[0]), ...P3(q[3]), lite(c1, 0.12), dark(c1, 0.12)), ...q);
    });
    F.wall(dark(c1, 0.25), a0, a1, depth, zTop - 5.1, zTop - 4);
    F.wall('rgba(25,15,35,.22)', a0, a1, 0, zTop - 5, zTop - 4);
  },
  striped(B, F, a0, a1, zTop, depth, [c1, c2]) {
    const { g } = B;
    awningSlope(B, F, a0, a1, zTop, depth, (k, a, b, q) => {
      const col = [c1, c2][k % 2];
      fp3(g, grad(g, ...P3(q[0]), ...P3(q[3]), lite(col, 0.12), dark(col, 0.1)), ...q);
    });
    F.wall(dark(c1, 0.25), a0, a1, depth, zTop - 4.9, zTop - 4);
    F.wall('rgba(25,15,35,.22)', a0, a1, 0, zTop - 5, zTop - 4);
  },
  scalloped(B, F, a0, a1, zTop, depth, [c1, c2]) {
    const { g } = B;
    awningSlope(B, F, a0, a1, zTop, depth, (k, a, b, q) => {
      fp3(g, grad(g, ...P3(q[0]), ...P3(q[3]), lite(c1, 0.12), dark(c1, 0.1)), ...q);
      const [x, y] = F.pt((a + b) / 2, depth, zTop - 4);
      ell(g, c1, x, y + 0.8, (Math.abs(b - a) * 0.72), 1.7);
      ell(g, rgba('#000', 0.14), x, y + 1.2, Math.abs(b - a) * 0.72, 1.3);
      ell(g, c2, x, y + 1.7, Math.abs(b - a) * 0.3, 0.5);
    });
    F.wall('rgba(25,15,35,.22)', a0, a1, 0, zTop - 5, zTop - 4);
  },
  canopy(B, F, a0, a1, zTop, depth, [c1, c2]) {
    const { g } = B;
    // A flat metal and fabric canopy held out from the wall on rods.
    const zf = zTop - 1.6;
    for (const a of [a0 + 0.8, a1 - 0.8]) line(g, '#5a6070', 0.6, ...F.pt(a, 0, zTop + 1.8), ...F.pt(a, depth, zf + 0.4));
    F.flat(grad(g, ...F.pt(a0, 0, zf), ...F.pt(a0, depth, zf), lite(c1, 0.14), c1), a0, a1, 0, depth, zf);
    F.wall(c1, a0, a1, depth, zf - 1.8, zf);
    F.wall(dark(c1, 0.22), a0, a1, depth, zf - 2.3, zf - 1.8);
    F.end(dark(c1, 0.12), F.near(a0, a1), 0, depth, zf - 1.8, zf);
    F.wall(rgba(c2, 0.5), a0 + 0.4, a1 - 0.4, depth, zf - 1.2, zf - 1);
    F.wall('rgba(25,15,35,.22)', a0, a1, 0, zf - 1, zf);
  },
};

// ---------------------------------------------------------------------------------------------
// Signs (E10): a picture on a plate hung from a bracket. These draw their picture round the middle of the
// plate, a little over three units across, in `ink`, with `plate` for anything cut out of it.

const SIGN_ART = {
  none() {},
  heart(g, ink) {
    g.beginPath();
    g.moveTo(0, 1.9);
    g.bezierCurveTo(-3, -0.2, -1.8, -2.5, 0, -1);
    g.bezierCurveTo(1.8, -2.5, 3, -0.2, 0, 1.9);
    g.fillStyle = ink;
    g.fill();
  },
  star(g, ink) {
    g.beginPath();
    for (let i = 0; i < 10; i++) {
      const r = i % 2 ? 1 : 2.4;
      const a = -1.5708 + (i * Math.PI) / 5;
      if (i) g.lineTo(Math.cos(a) * r, Math.sin(a) * r + 0.1);
      else g.moveTo(Math.cos(a) * r, Math.sin(a) * r + 0.1);
    }
    g.closePath();
    g.fillStyle = ink;
    g.fill();
  },
  note(g, ink) {
    ell(g, ink, -0.9, 1.5, 1.2, 0.85);
    line(g, ink, 0.8, 0.2, 1.3, 0.2, -2);
    g.beginPath();
    g.moveTo(0.2, -2);
    g.bezierCurveTo(1.6, -1.6, 2.4, -0.6, 1.8, 0.8);
    g.bezierCurveTo(1.6, -0.4, 1, -0.9, 0.2, -0.8);
    g.closePath();
    g.fillStyle = ink;
    g.fill();
  },
  cup(g, ink) {
    g.beginPath();
    g.moveTo(-1.8, -0.4);
    g.lineTo(1.4, -0.4);
    g.lineTo(1.1, 1.6);
    g.quadraticCurveTo(0, 2.3, -1.5, 1.6);
    g.closePath();
    g.fillStyle = ink;
    g.fill();
    g.beginPath();
    g.arc(1.5, 0.6, 0.9, -1.4, 1.4);
    stroke(g, ink, 0.6);
    for (const x of [-0.8, 0.3]) {
      g.beginPath();
      g.moveTo(x, -0.9);
      g.quadraticCurveTo(x - 0.7, -1.6, x, -2.2);
      g.quadraticCurveTo(x + 0.6, -2.7, x, -3);
      stroke(g, ink, 0.4);
    }
  },
  moon(g, ink, plate) {
    circle(g, ink, 0, 0, 2.2);
    circle(g, plate, 1, -0.6, 1.8);
  },
  paw(g, ink) {
    ell(g, ink, 0, 1, 1.5, 1.2);
    for (const [x, y] of [[-2, -0.4], [-0.75, -1.7], [0.75, -1.7], [2, -0.4]]) ell(g, ink, x, y, 0.6, 0.85);
  },
  leaf(g, ink, plate) {
    g.beginPath();
    g.moveTo(-0.5, 2.3);
    g.bezierCurveTo(-2.8, 0.8, -1.8, -1.8, 1.2, -2.3);
    g.bezierCurveTo(2.6, 0.6, 1.4, 2, -0.5, 2.3);
    g.closePath();
    g.fillStyle = ink;
    g.fill();
    line(g, plate, 0.4, -0.7, 2.4, 0.8, -1.4);
  },
  wreath(g, ink, plate) {
    // At night the plate hands over a see-through colour, and the wreath shines warm in it.
    const night = ink.startsWith('rgba');
    // A dim warm ring in a soft glow, not a pale disc, so it reads as leaves and not as a speech bubble.
    if (night) glowAt(g, 0, 0, 4.6, '#ff9a3a', 0.2);
    const cols = night ? ['rgba(255,120,30,.36)', 'rgba(255,170,50,.34)', 'rgba(230,80,30,.38)'] : FALL_LEAVES.slice(0, 6);
    if (!night) {
      g.beginPath();
      g.arc(0, -0.2, 1.75, 0, TAU);
      stroke(g, '#5a3a1e', 1.1);
    }
    for (let i = 0; i < 16; i++) {
      const a = (i / 16) * TAU;
      const c = cols[i % cols.length];
      leafAt(g, night ? c : i % 2 ? c : dark(c, 0.16), Math.cos(a) * 1.8, Math.sin(a) * 1.8 - 0.2, a + 1.6, 1.3);
    }
    for (const [dx, dy] of night ? [] : [[-1.7, -0.9], [1.5, 0.6], [0.2, -1.9]]) circle(g, '#d93a3a', dx, dy - 0.2, 0.32);
    if (night) {
      // A small knot with two short tails, dim so it doesn't outshine the leaves.
      circle(g, 'rgba(220,100,60,.5)', 0, 1.9, 0.45);
      for (const s of [-1, 1]) line(g, 'rgba(220,100,60,.45)', 0.45, 0, 2, s * 0.9, 3.1);
      return;
    }
    // A red ribbon, so it shows on a plate of any colour.
    const bow = '#c0392b';
    const edge = '#6e1414';
    for (const s of [-1, 1]) {
      fp(g, edge, [[0, 1.9], [s * 1.5, 1.1], [s * 1.7, 2.7]]);
      fp(g, bow, [[0, 1.9], [s * 1.3, 1.3], [s * 1.4, 2.5]]);
    }
    circle(g, bow, 0, 1.9, 0.4);
  },
  bolt(g, ink) {
    fp(g, ink, [[0.9, -2.5], [-1.7, 0.3], [-0.1, 0.3], [-0.9, 2.5], [1.8, -0.5], [0.2, -0.5]]);
  },
  flower(g, ink, plate) {
    for (let i = 0; i < 5; i++) {
      const a = (i / 5) * TAU - 1.5708;
      circle(g, ink, Math.cos(a) * 1.3, Math.sin(a) * 1.3, 0.95);
    }
    circle(g, plate, 0, 0, 0.7);
    circle(g, '#f3c53a', 0, 0, 0.5);
  },
};

// A plate on chains from a bracket, at x with its top at -z in the wall's plane. It glows a little at night.
function signPlate(B, x, z, col) {
  const { g, S } = B;
  const wpl = 5.6;
  const hpl = 5.6;
  const word = B.signStyle;
  const art = SIGN_ART[word];
  if (!art || word === 'none') return;
  const top = -z;
  const cx = x + wpl / 2;
  const cy = top + 1.6 + hpl / 2;
  if (!S.day) {
    glowAt(g, cx, cy, 8, lite(col, 0.2), 0.34);
    rr(g, rgba(lite(col, 0.5), word === 'wreath' ? 0.16 : 0.55), x, top + 1.6, wpl, hpl);
    g.save();
    g.translate(cx, cy);
    art(g, 'rgba(255,255,255,.9)', rgba(lite(col, 0.5), word === 'wreath' ? 0.16 : 0.55));
    g.restore();
    return;
  }
  // The arm, from the wall.
  rr(g, '#2d3039', x - 0.6, top - 0.3, wpl + 1.2, 0.9);
  rr(g, '#2d3039', x - 1, top - 1.4, 0.8, 2.6);
  for (const cxx of [x + 0.7, x + wpl - 0.7]) line(g, '#2d3039', 0.5, cxx, top + 0.5, cxx, top + 1.7);
  rr(g, 'rgba(25,15,35,.3)', x + 0.6, top + 2.2, wpl, hpl);
  rr(g, dark(col, 0.45), x - 0.5, top + 1.1, wpl + 1, hpl + 1);
  rr(g, grad(g, 0, top + 1.6, 0, top + 1.6 + hpl, lite(col, 0.18), col, dark(col, 0.1)), x, top + 1.6, wpl, hpl);
  rr(g, 'rgba(255,255,255,.22)', x, top + 1.6, wpl, 0.6);
  g.save();
  g.translate(cx, cy);
  art(g, '#fff8ea', col);
  g.restore();
}

// ---------------------------------------------------------------------------------------------
// Lights along the roof's edge (E10), in both passes: a fitting by day and a glow by night. `pts` are the
// edge's corners in 3D and `col` is the part's colour or null.

function edgeWalk(pts, step, fn) {
  let carried = 0;
  for (let i = 0; i + 1 < pts.length; i++) {
    const a = P3(pts[i]);
    const b = P3(pts[i + 1]);
    const len = Math.hypot(b[0] - a[0], b[1] - a[1]);
    let d = carried === 0 ? step * 0.5 : carried;
    while (d < len) {
      fn(lerp(a[0], b[0], d / len), lerp(a[1], b[1], d / len), a, b);
      d += step;
    }
    carried = d - len;
  }
}

function bulb(g, day, x, y, r, col, halo) {
  if (day) {
    circle(g, mixHex(col, '#9a948a', 0.5), x, y, r);
    circle(g, 'rgba(255,255,255,.35)', x - r * 0.3, y - r * 0.3, r * 0.35);
  } else {
    glowAt(g, x, y, r * halo, col, 0.5);
    circle(g, lite(col, 0.55), x, y, r * 0.9);
  }
}

function wire(g, day, pts, sag) {
  if (!day) return;
  for (let i = 0; i + 1 < pts.length; i++) {
    const a = P3(pts[i]);
    const b = P3(pts[i + 1]);
    g.beginPath();
    g.moveTo(a[0], a[1]);
    g.quadraticCurveTo((a[0] + b[0]) / 2, (a[1] + b[1]) / 2 + sag, b[0], b[1]);
    stroke(g, 'rgba(25,22,35,.7)', 0.5);
  }
}

const STRING_ART = {
  none() {},
  eaves(B, pts, col) {
    const { g, S } = B;
    const c = col || '#ffe9a0';
    wire(g, S.day, pts, 0.6);
    edgeWalk(pts, 3.4, (x, y) => bulb(g, S.day, x, y + 0.6, 0.95, c, 5));
  },
  fairy(B, pts, col) {
    const { g, S } = B;
    const c = col || '#ffeab8';
    wire(g, S.day, pts, 0.4);
    let n = 0;
    edgeWalk(pts, 1.15, (x, y) => {
      n++;
      bulb(g, S.day, x, y + 0.5 + (n % 2) * 0.5, 0.45, n % 3 ? c : lite(c, 0.4), 4);
    });
  },
  festoon(B, pts, col) {
    const { g, S } = B;
    const cs = col ? [col, lite(col, 0.25)] : ['#ffe9a0', '#ffc880'];
    // Drooping wire between hooks, with a heavy bulb hanging from it.
    edgeWalk(pts, 5.2, (x, y, a, b) => {
      const dx = (b[0] - a[0]) / Math.hypot(b[0] - a[0], b[1] - a[1]);
      const dy = (b[1] - a[1]) / Math.hypot(b[0] - a[0], b[1] - a[1]);
      const hx = x + dx * 2.6;
      const hy = y + dy * 2.6;
      if (S.day) {
        g.beginPath();
        g.moveTo(x, y);
        g.quadraticCurveTo((x + hx) / 2, (y + hy) / 2 + 4.4, hx, hy);
        stroke(g, 'rgba(25,22,35,.75)', 0.5);
      }
      const mx = (x + hx) / 2;
      const my = (y + hy) / 2 + 2.2;
      if (S.day) rr(g, '#3a3f4a', mx - 0.4, my, 0.8, 0.9);
      bulb(g, S.day, mx, my + 1.9, 1.3, cs[Math.abs(Math.round(x)) % 2], 4.2);
    });
  },
  garland(B, pts, col) {
    const { g, S } = B;
    const c = col || '#ffd54f';
    if (S.day) {
      // A rope of leaves in fall colours: two or three at each step, turned every way.
      trail(g, '#5a3a22', 0.8, pts.map(P3));
      edgeWalk(pts, 0.7, (x, y) => {
        const h = ih(Math.round(x * 4), Math.round(y * 4), 6);
        for (let k = 0; k < 3; k++) {
          const col2 = FALL_LEAVES[(h + k * 3) % FALL_LEAVES.length];
          leafAt(g, k === 1 ? col2 : dark(col2, 0.2), x, y + 0.2 + k * 0.4, ((h >> (k * 3)) % 16) * 0.4, 1.9 + (h % 4) * 0.3);
        }
      });
    }
    // The warm bulbs hang under it, a few units apart.
    edgeWalk(pts, 4.2, (x, y) => {
      if (S.day) {
        line(g, '#5a3a22', 0.4, x, y + 0.4, x, y + 1.2);
        circle(g, mixHex(c, '#8a7050', 0.35), x, y + 2, 0.85);
        circle(g, 'rgba(255,255,255,.4)', x - 0.25, y + 1.7, 0.28);
      } else {
        glowAt(g, x, y + 2, 4.6, c, 0.5);
        circle(g, lite(c, 0.55), x, y + 2, 0.8);
      }
    });
  },
  icicle(B, pts, col) {
    const { g, S } = B;
    const c = col || '#d8f0ff';
    wire(g, S.day, pts, 0.3);
    let n = 0;
    edgeWalk(pts, 1.5, (x, y) => {
      n++;
      const len = 2 + (n % 4) * 1.3;
      if (S.day) line(g, 'rgba(25,22,35,.6)', 0.35, x, y, x, y + len);
      for (let k = 1.6; k <= len; k += 1.5) bulb(g, S.day, x, y + k, 0.5, c, 3.4);
    });
  },
  neon(B, pts, col) {
    const { g, S } = B;
    const c = col || '#ff5fb0';
    const line2 = pts.map(P3);
    g.lineCap = 'round';
    g.lineJoin = 'round';
    if (S.day) {
      trail(g, 'rgba(25,22,35,.55)', 2.6, line2.map(([x, y]) => [x + 0.3, y + 0.8]));
      trail(g, mixHex(c, '#d8d4ce', 0.55), 1.7, line2);
      trail(g, 'rgba(255,255,255,.45)', 0.5, line2.map(([x, y]) => [x, y - 0.4]));
    } else {
      // A halo that fades: wide and faint layers under narrow and bright ones, so no layer shows an edge.
      for (const [w, a] of [[11, 0.03], [9, 0.04], [7.2, 0.05], [5.6, 0.07], [4.2, 0.1]]) trail(g, rgba(c, a), w, line2);
      trail(g, lite(c, 0.45), 1.8, line2);
      trail(g, 'rgba(255,255,255,.85)', 0.7, line2);
    }
    g.lineCap = 'butt';
  },
};

// ---------------------------------------------------------------------------------------------
// Lights on the walls (E10). They run for each face as its details are painted, in the wall's own plane.
// `f` describes the face: its length, where the door is, where the ground floor's windows are, and how tall it
// stands (`top`, up from the wall's foot).

function lampAt(B, x, y, col) {
  const { g, S } = B;
  if (S.day) {
    rr(g, '#2c2f3c', x - 1, y - 1.4, 2, 3);
    rr(g, col ? lite(col, 0.3) : '#ffe9a0', x - 0.7, y - 1, 1.4, 2.2);
  } else {
    glowAt(g, x, y, 4.6, col || '#ffdc82', 0.42);
    rr(g, col ? lite(col, 0.55) : '#fff4b8', x - 0.9, y - 1, 1.8, 2.6);
  }
}

const WALL_LIGHT_ART = {
  porch_lamp(B, f, col) {
    if (f.door) lampAt(B, f.door.x + f.door.w + 2.5, -f.door.h, col);
  },
  none() {},
  sconces(B, f, col) {
    const { g, S } = B;
    const sconce = (x, y) => {
      if (S.day) {
        rr(g, '#2c2f3c', x - 0.4, y - 0.4, 0.8, 2.4);
        rr(g, '#2c2f3c', x - 1.1, y - 2.2, 2.2, 0.7);
        rr(g, col ? lite(col, 0.3) : '#ffe9a0', x - 0.8, y - 1.6, 1.6, 2);
      } else {
        glowAt(g, x, y - 0.6, 6.5, col || '#ffd890', 0.5);
        rr(g, col ? lite(col, 0.55) : '#fff4b8', x - 0.8, y - 1.6, 1.6, 2);
      }
    };
    if (f.door) {
      sconce(f.door.x - 2.6, -f.door.h * 0.62);
      sconce(f.door.x + f.door.w + 2.6, -f.door.h * 0.62);
    }
    for (const win of f.wins) if (win.f === 0) sconce(win.x - 3.2, -(win.z + win.h * 0.5));
  },
  uplights(B, f, col) {
    const { g, S } = B;
    const c = col || '#fff0c0';
    const step = 6.4;
    for (let x = f.lo + 2; x < f.hi - 0.5; x += step) {
      if (f.door && x > f.door.x - 2 && x < f.door.x + f.door.w + 2) continue;
      if (S.day) {
        rr(g, '#2c2f3c', x - 1, -f.zBase - 1.1, 2, 1.1);
        rr(g, lite(c, 0.2), x - 0.6, -f.zBase - 1.3, 1.2, 0.4);
      } else {
        // The beam spreads as it climbs the wall and fades out.
        const h = Math.min(f.top - f.zBase, 13);
        const gr = g.createLinearGradient(0, -f.zBase, 0, -f.zBase - h);
        gr.addColorStop(0, rgba(c, 0.2));
        gr.addColorStop(1, rgba(c, 0));
        g.fillStyle = gr;
        // Three layers, wide to narrow, so the sides of the beam fade instead of ending in an edge.
        for (const k of [1.9, 1.35, 1]) {
          g.beginPath();
          g.moveTo(x - 0.8 * k, -f.zBase - 1);
          g.lineTo(x - 3.6 * k, -f.zBase - h);
          g.lineTo(x + 3.6 * k, -f.zBase - h);
          g.lineTo(x + 0.8 * k, -f.zBase - 1);
          g.closePath();
          g.fill();
        }
        glowAt(g, x, -f.zBase - 0.6, 3, c, 0.6);
      }
    }
  },
  spotlights(B, f, col) {
    const { g, S } = B;
    const c = col || '#f4f8ff';
    const y = -f.top + 1.4;
    for (const [x, dir] of [[f.lo + 1.4, 1], [f.hi - 1.4, -1]]) {
      if (S.day) {
        rr(g, '#2c2f3c', x - 0.4, y - 1.4, 0.8, 1.8);
        rr(g, '#2c2f3c', x - 1.6, y, 3.2, 1.8);
        rr(g, lite(c, 0.1), x - 1.1, y + 1.6, 2.2, 0.5);
      } else {
        const len = Math.min(12, f.top - f.zBase);
        const gr = g.createLinearGradient(x, y + 1.8, x + dir * 3, y + 1.8 + len);
        gr.addColorStop(0, rgba(c, 0.2));
        gr.addColorStop(1, rgba(c, 0));
        g.fillStyle = gr;
        // Three layers again, so the beam's sides fade out.
        for (const k of [1.8, 1.3, 1]) {
          g.beginPath();
          g.moveTo(x - 1 * k, y + 1.8);
          g.lineTo(x + 1 * k, y + 1.8);
          g.lineTo(x + dir * 7 + 4 * k - 1, y + 1.8 + len);
          g.lineTo(x + dir * 7 - 4 * k - 1, y + 1.8 + len);
          g.closePath();
          g.fill();
        }
        glowAt(g, x, y + 2, 4, c, 0.65);
      }
    }
  },
};

// ---------------------------------------------------------------------------------------------
// Glow (E10): how many windows are lit at night, out of a hundred.
const GLOW_ART = { evening: 64, all: 101, few: 25, off: 0 };

const CURTAIN_COLOR = { open: '#d8e4f4', closed: '#e9d9b8', blinds: '#e6dcc4', lace: '#fffdf6' };

// ---------------------------------------------------------------------------------------------
// Shapes (E10). All of them stay inside the body rectangle: a shape says which blocks make the building
// and where its roofs go. `hide` is the stretch of a wall that another block covers, and `off` where
// a block's wall starts along the whole wall.

function blockOf(B, R, z0, z1, f0, f1, hide = {}) {
  const [u0, v0, u1, v1] = B.R;
  return { R, z0, z1, f0, f1, hide, off: { L: R[0] - u0, R: v1 - R[3] }, plan: {}, ground: f0 === 0 };
}

const SHAPE_ART = {
  plain(B) {
    const blk = blockOf(B, B.R, B.zf[0], B.zf[B.floors], 0, B.floors);
    return { blocks: [blk], roofs: [{ R: B.R, zE: B.zf[B.floors], after: 0, main: true, o: B.overhang }] };
  },
  ell(B) {
    const [u0, v0, u1, v1] = B.R;
    // The back right corner is left out, so the plan is an L. Its inside corner shows from the front.
    const nu = Math.max(4, Math.round((u1 - u0) * 0.45));
    const nv = Math.max(4, Math.round((v1 - v0) * 0.45));
    const um = u1 - nu;
    const vm = v0 + nv;
    const zt = B.zf[B.floors];
    const b1 = blockOf(B, [u0, v0, um, v1], B.zf[0], zt, 0, B.floors, { R: [0, v1 - vm] });
    const b2 = blockOf(B, [um, vm, u1, v1], B.zf[0], zt, 0, B.floors);
    b1.spanAll = { L: [0, u1 - u0] };
    b2.spanAll = { L: [0, u1 - u0] };
    return {
      blocks: [b1, b2],
      roofs: [
        { R: b1.R, zE: zt, after: 0, main: true, o: B.overhang, axis: 'v', cut: { R: [vm, v1] }, ref: B.R },
        { R: b2.R, zE: zt, after: 1, o: B.overhang, axis: 'u', seam: true, ref: B.R },
      ],
    };
  },
  tower(B) {
    const [u0, v0, u1, v1] = B.R;
    const tw = clamp(Math.round(Math.min(u1 - u0, v1 - v0) * 0.42), 4, 8);
    const zt = B.zf[B.floors];
    const th = Math.max(6, B.Hu * 0.85);
    const main = blockOf(B, B.R, B.zf[0], zt, 0, B.floors, { L: [u1 - u0 - tw, u1 - u0], R: [0, tw] });
    const T = [u1 - tw, v1 - tw, u1, v1];
    const tower = blockOf(B, T, B.zf[0], zt + th, 0, B.floors + 1);
    tower.tower = true;
    return {
      blocks: [main, tower],
      roofs: [
        { R: B.R, zE: zt, after: 0, main: true, o: 0 },
        { R: T, zE: zt + th, after: 1, o: 0.8, tower: true },
      ],
    };
  },
  stepped(B) {
    const [u0, v0, u1, v1] = B.R;
    const n = B.floors;
    const step = n > 1 ? clamp((Math.min(u1 - u0, v1 - v0) - 6) / (2 * (n - 1)), 0.8, 2.2) : 0;
    const blocks = [];
    for (let f = 0; f < n; f++) {
      const i = f * step;
      blocks.push(blockOf(B, [u0 + i, v0 + i, u1 - i, v1 - i], B.zf[f], B.zf[f + 1], f, f + 1));
      blocks[f].terrace = f < n - 1;
    }
    const top = blocks[n - 1];
    return { blocks, roofs: [{ R: top.R, zE: B.zf[n], after: n - 1, main: true, o: n > 1 ? Math.min(B.overhang, 1.2) : B.overhang }] };
  },
};

// ---------------------------------------------------------------------------------------------
// The building as its owner chose it. Anything not chosen is what that kind of place has always had.

const keysOf = (table) => Object.keys(table);

function kindWall(S, kind, c) {
  if (kind === 'house') return S.v === 1 ? { k: 'brick', c: BRICKS[S.pal % 4] } : { k: 'siding', c: c.wall };
  if (kind === 'cafe') return { k: 'siding', c: '#e8d9b8' };
  if (kind === 'club') return { k: 'brick', c: '#5a4668' };
  if (kind === 'lounge') return { k: 'siding', c: '#e6bcae' };
  if (kind === 'bar') return { k: 'brick', c: '#4a3a3a' };
  if (kind === 'office') return { k: 'concrete', c: '#c9ccd2' };
  if (kind === 'workshop') return { k: 'brick', c: BRICKS[(S.pal + 1) % 4] };
  if (kind === 'library') return { k: 'siding', c: '#ece4cf' };
  return { k: 'brick', c: BRICKS[S.pal % 4] };
}

function wallMat(S, kind, c, style, color) {
  const kd = kindWall(S, kind, c);
  if (!style) return color ? WALL_ART[kd.k](color) : kd;
  let base = color || NATURAL[style];
  if (!base) base = style === kd.k ? kd.c : style === 'brick' ? BRICKS[S.pal % 4] : kd.k === 'brick' ? c.wall : kd.c;
  return WALL_ART[style](base);
}

function look(S, kind, outside, c) {
  const k = Object.hasOwn(KINDS, kind) ? KINDS[kind] : undefined;
  let R = rectOf(kind, outside);
  let body = bodyOf(kind, outside);
  if (!k || !R) return null;
  // A body that doesn't fit on the tile is no body at all (E4).
  if (R[2] - R[0] < 4 || R[3] - R[1] < 4 || R[0] < 0 || R[1] < 0 || R[2] > 32 || R[3] > 32) {
    R = rectOf(kind, null);
    body = null;
  }
  const word = (part, table, fallback) => styleOf(outside, part, keysOf(table), fallback);
  const hex = (part) => hexOf(partOf(outside, part).color, null);
  const floors = clamp(body ? body.floors : 1, 1, 3);
  const foundation = word('foundation', FOUNDATION_ART, 'none');
  const Z0 = foundation === 'raised' ? 3.2 : 0;
  const Hu = k.up ?? k.h * 0.9;
  const zf = [Z0, Z0 + k.h];
  for (let f = 1; f < floors; f++) zf.push(zf[f] + Hu);
  const trimStyle = word('trim', TRIM_ART, 'plain');
  const trimHex = hex('trim');
  const roofing = word('roofing', ROOFING_ART, 'shingles');
  const roofHex = hex('roof');
  const pitchRaw = numberOf(outside, 'roof', null);
  const roofStyle = word('roof', ROOF_ART, k.roof);
  const wallStyle = word('walls', WALL_ART, null);
  const countRaw = numberOf(outside, 'windows', null);
  const chimney = word('chimney', CHIMNEY_ART, kind === 'house' || k.smoke ? 'brick' : 'none');
  const B = {
    S,
    g: S.g,
    kind,
    k,
    c,
    outside,
    R,
    floors,
    H: k.h,
    Hu,
    Z0,
    zf,
    foundation,
    shape: word('shape', SHAPE_ART, 'plain'),
    mat: wallMat(S, kind, c, wallStyle, hex('walls')),
    trimStyle,
    trim: TRIM_ART[trimStyle],
    trimHex,
    frameCol: trimStyle === 'none' ? null : trimHex || TRIM,
    winFrame: trimStyle === 'none' ? null : trimHex || k.frame || TRIM,
    fascia: trimHex || '#efe8d6',
    roofStyle,
    roofHex,
    pitch: Number.isInteger(pitchRaw) ? clamp(pitchRaw, 1, 3) : null,
    roofing,
    roofBase: roofing === 'solar' ? ROOFING_COLOR.solar : roofHex || ROOFING_COLOR[roofing] || k.roofCol || c.roof,
    winStyle: word('windows', WINDOW_ART, null),
    winCount: Number.isInteger(countRaw) ? clamp(countRaw, 0, 6) : null,
    glassColor: hex('windows'),
    shutter: word('shutters', SHUTTER_ART, 'none'),
    shutterCol: hex('shutters') || c.door,
    curtain: word('curtains', CURTAIN_ART, null),
    curtainCol: hex('curtains'),
    boxes: word('window_boxes', BOX_ART, 'none'),
    doorStyle: word('door', DOOR_ART, null),
    doorCol: hex('door') || k.doorCol || c.door,
    porch: word('porch', PORCH_ART, 'none'),
    porchCol: hex('porch'),
    awningSet: !!word('awning', AWNING_ART, null),
    awning: word('awning', AWNING_ART, kind === 'cafe' ? 'striped' : 'none'),
    awningCol: hex('awning'),
    signStyle: word('sign', SIGN_ART, 'none'),
    signCol: hex('sign'),
    glowSet: !!word('glow', GLOW_ART, null),
    glowPct: GLOW_ART[word('glow', GLOW_ART, 'evening')],
    glowColor: hex('glow'),
    wallLight: word('wall_lights', WALL_LIGHT_ART, 'porch_lamp'),
    wallLightCol: hex('wall_lights'),
    stringStyle: word('string_lights', STRING_ART, 'none'),
    stringCol: hex('string_lights'),
    chimney,
    chimneyCol: hex('chimney'),
    chimneySet: !!word('chimney', CHIMNEY_ART, null),
    rooftop: keysOf(ROOFTOP_ART).filter((w) => isOn(outside, 'rooftop', w)),
    rooftopCol: hex('rooftop'),
    overhang: (k.over ?? 1.8) + (trimStyle === 'bold' ? 0.5 : 0),
    yardFence: !!outside && !!partOf(outside, 'fence').style,
  };
  return B;
}

// ---------------------------------------------------------------------------------------------
// Walls and what's on them.

// The stretch of a wall that shows, in the block's own x.
function visible(blk, side, len) {
  const hide = blk.hide[side];
  if (!hide) return [0, len];
  return hide[0] <= 0.01 ? [hide[1], len] : [0, hide[0]];
}

function wallLen(blk, side) {
  return side === 'L' ? blk.R[2] - blk.R[0] : blk.R[3] - blk.R[1];
}

// What the old placed buildings had on the ground floor when nothing is chosen: windows, a door and the
// order they were painted in, which is the order their lights were counted in.
const OLD_PLAN = {
  house(len, doorFace) {
    const wins = [{ x: 3, w: 4, h: 5, z: 4, curtain: '#f1e2c0' }];
    if (doorFace) return { door: len - 8, wins };
    wins.push({ x: len - 7, w: 4, h: 5, z: 4 });
    return { door: null, wins };
  },
  cafe(len, doorFace) {
    if (doorFace) return { door: len / 2 - 2.2, wins: [{ x: 2.5, w: 5, h: 6, z: 4 }, { x: len - 7.5, w: 5, h: 6, z: 4 }] };
    return { door: null, wins: [{ x: 4, w: len - 8, h: 6, z: 4, curtain: '#f1e2c0', fixed: true }] };
  },
  club(len, doorFace) {
    return { door: doorFace ? len / 2 - 2.2 : null, wins: [] };
  },
  studio(len, doorFace) {
    const lay = layout(len, 4.4, 3.4, 2.2);
    const wins = [];
    for (let i = doorFace ? 1 : 0; i < lay.count; i++) wins.push({ x: lay.start + i * lay.step, w: 4.4, h: 7, z: 5 });
    return { door: doorFace ? 3 : null, wins };
  },
  lounge(len, doorFace) {
    const curtain = KINDS.lounge.curtain;
    if (doorFace) {
      const w = len / 2 - 6;
      return { wins: [{ x: 2.5, w, h: 5.5, curtain, fixed: true }, { x: len / 2 + 3, w, h: 5.5, curtain, fixed: true }] };
    }
    return { wins: [{ x: 3, w: len - 6, h: 5.5, curtain, fixed: true }] };
  },
  bar(len) {
    return { wins: [{ x: 2.5, w: 3.6, h: 4.4 }, { x: len - 6.1, w: 3.6, h: 4.4 }] };
  },
  office(len, doorFace) {
    // Three rows of windows, and the door takes the place of the first row's middle ones.
    const lay = layout(len, 3.6, 2.2, 1.8);
    const wins = [];
    for (let row = doorFace ? 1 : 0; row < 3; row++) {
      for (let i = 0; i < lay.count; i++) wins.push({ x: lay.start + i * lay.step, w: 3.6, h: 5.5, row });
    }
    return { wins };
  },
  workshop(len, doorFace) {
    const wins = [];
    if (!doorFace) for (let i = 0; i < 3; i++) wins.push({ x: 3 + i * 6, w: 4, h: 5 });
    return { wins };
  },
  library(len, doorFace) {
    const lay = layout(len, 2.8, 2.6, 2);
    const wins = [];
    for (let i = 0; i < lay.count; i++) {
      const x = lay.start + i * lay.step;
      if (!(doorFace && Math.abs(x - len / 2) < 4)) wins.push({ x, w: 2.8, h: 9 });
    }
    return { wins };
  },
};

// How far apart a kind's rows of windows are.
const ROW = 8.5;

function planFace(B, blk, side, len) {
  if (blk.plan[side]) return blk.plan[side];
  const { S, k } = B;
  const [lo, hi] = visible(blk, side, len);
  const doorFace = B.doorBlk === blk && side === S.door;
  const art = WINDOW_ART[B.winStyle || 'square'];
  const dsize = DOOR_SIZE[B.doorStyle || 'panel'](k.door[0], k.door[1]);
  const dw = dsize[0];
  const dh = Math.min(dsize[1], B.H - 1.8);
  const plan = { lo, hi, len, door: null, wins: [], sign: null, zBase: blk.z0 - B.Z0, top: blk.z1 - B.Z0 };
  const custom = B.winCount !== null;
  // The ground floor of a block that has one.
  const ground = blk.f0 === 0 ? (blk.tower ? { wins: [] } : OLD_PLAN[B.kind](len, doorFace)) : null;
  if (doorFace) {
    // A door goes where the kind put it, narrowed to the wall that shows.
    let x = B.kind === 'house' ? len - dw - 3.6 : B.kind === 'studio' ? 3 : B.kind === 'workshop' ? len - dw - 2.6 : len / 2 - dw / 2 + (k.shift || 0);
    if (hi - lo < dw + 2.8) x = lo + (hi - lo - dw) / 2;
    else x = clamp(x, lo + 1.4, hi - dw - 1.4);
    plan.door = { x, w: dw, h: dh };
  }
  const obstacles = [];
  // What a window above the first row has to keep clear of.
  const overSign = [];
  if (plan.door) {
    const lamp = B.wallLight === 'porch_lamp' ? 4.4 : 1.6;
    obstacles.push([plan.door.x - 1.6, plan.door.x + dw + lamp]);
  }
  // A sign hangs beside the door, on whichever side leaves the windows alone.
  if (plan.door && B.signStyle !== 'none') {
    const pw = 5.6;
    const left = plan.door.x - 1.8 - pw;
    const right = plan.door.x + dw + 4.6;
    const fits = (x) => x >= lo + 0.5 && x + pw <= hi - 0.5;
    plan.sign = fits(left) ? left : fits(right) ? right : null;
    if (plan.sign !== null) {
      obstacles.push([plan.sign - 0.8, plan.sign + pw + 0.8]);
      overSign.push(obstacles[obstacles.length - 1]);
    }
  }
  for (let f = blk.f0; f < blk.f1; f++) {
    const fh = f === 0 ? B.H : B.Hu;
    const sill = f === 0 ? k.win[2] : (k.win[2] * B.Hu) / B.H;
    const z = B.zf[f] + sill;
    const base = k.win;
    const items = [];
    const isOld = f === 0 && !custom && !blk.tower;
    // A kind with rows of windows has them on every storey (an office's first has three).
    const rows = k.rows ? k.rows[f === 0 ? 0 : 1] : 1;
    const [bw, bh] = art.size(base[0], base[1]);
    const place = (x, ww, hh, extra = {}) => {
      const [nw, nh] = extra.fixed ? [ww, hh] : art.size(ww, hh);
      const row = extra.row || 0;
      const room = Math.min(fh - sill - row * ROW - 1.6, k.wtop ? k.wtop - sill : Infinity);
      items.push({ ...extra, x: x + (ww - nw) / 2, w: nw, h: Math.min(nh, room), z: z + row * ROW, row, f, old: isOld });
    };
    if (f === 0 && !custom && !blk.tower) {
      for (const w of ground.wins) place(w.x, w.w, w.h, { curtain: w.curtain, fixed: w.fixed, row: w.row });
    } else {
      const span = blk.spanAll?.[side] || [blk.off[side], blk.off[side] + len];
      const gap = 1.6;
      const room = span[1] - span[0];
      let n;
      if (custom) n = Math.min(B.winCount, Math.floor((room + gap) / (bw + gap)));
      else n = layout(hi - lo, bw, 3.4, 2).count;
      for (let row = 0; row < rows; row++) {
        if (custom) {
          for (let q = 0; q < n; q++) {
            const cx = span[0] + (room * (q + 0.5)) / n - blk.off[side];
            place(cx - base[0] / 2, base[0], base[1], { row, curtain: f === 0 ? k.curtain : undefined });
          }
        } else {
          const lay = layout(hi - lo, bw, 3.4, 2);
          for (let q = 0; q < lay.count; q++) place(lo + lay.start + q * lay.step + (bw - base[0]) / 2, base[0], base[1], { row });
        }
      }
    }
    // Windows that don't fit on the wall that shows, or that sit on the door or the sign, aren't painted.
    const kept = [];
    for (const it of items) {
      // A short wall can leave a window with no width (a cafe's long one), and a round or arched window can't have a negative radius.
      if (!(it.w > 0.5 && it.h > 0.5)) continue;
      if (it.x < lo + 0.7 || it.x + it.w > hi - 0.7) continue;
      if (f === 0 && !it.old && (it.row ? overSign : obstacles).some(([a, b]) => it.x - 1 < b && it.x + it.w + 1 > a)) continue;
      // The old house's window tucks a unit under its door, and the door is painted over it (E4), so only more than that is a clash.
      if (it.old && !it.row && plan.door && Math.min(it.x + it.w, plan.door.x + dw) - Math.max(it.x, plan.door.x) > 1.2) continue;
      if (it.old && plan.sign !== null && it.x < plan.sign + 6.4 && it.x + it.w > plan.sign - 0.8) continue;
      const prev = kept[kept.length - 1];
      if (prev && prev.row === it.row && it.x - (it.old ? 0 : 1.2) < prev.x + prev.w) continue;
      kept.push(it);
    }
    plan.wins.push(...kept);
  }
  blk.plan[side] = plan;
  return plan;
}

function windowOptions(B, it, index) {
  const curtainStyle = B.curtain === null ? (it.curtain || it.f > 0 ? 'open' : null) : B.curtain === 'none' ? null : B.curtain;
  let curtainColor = B.curtainCol || CURTAIN_COLOR[curtainStyle];
  if (B.curtain === null && curtainStyle) curtainColor = it.curtain || (index % 2 ? '#e8c8d8' : '#d8e4f4');
  return {
    style: B.winStyle || 'square',
    frame: B.winFrame,
    lit: B.glowSet ? null : B.k.lit ?? null,
    glass: B.glassColor,
    curtain: curtainStyle ? { style: curtainStyle, color: curtainColor } : null,
    shutters: B.shutter !== 'none' ? { style: B.shutter, color: B.shutterCol } : null,
    box: B.boxes !== 'none' ? B.boxes : null,
  };
}

// A foundation band, the cornice, corner boards and the ledges between floors for one face.
function wallDressing(B, blk, side, plan) {
  const { g } = B;
  const { lo, hi } = plan;
  if (blk.ground && B.foundation !== 'none' && B.foundation !== 'raised') {
    FOUNDATION_ART[B.foundation](g, lo, hi, 2.4, seeded(B.S.seed + 31));
  }
  const tr = B.trim;
  const tc = B.trimHex || TRIM;
  if (tr.board > 0) {
    for (const x of [0, plan.len - tr.board]) {
      if (x < lo - 0.01 || x + tr.board > hi + 0.01) continue;
      rr(g, tc, x, -blk.z1, tr.board, blk.z1 - blk.z0);
      rr(g, 'rgba(25,15,35,.2)', x + tr.board - 0.3, -blk.z1, 0.3, blk.z1 - blk.z0);
    }
    rr(g, tc, lo, -blk.z1, hi - lo, 1.2);
    rr(g, 'rgba(25,15,35,.22)', lo, -blk.z1 + 1.2, hi - lo, 0.7);
  }
  // A ledge where each floor meets the next.
  for (let f = Math.max(blk.f0, 1); f < blk.f1; f++) {
    const z = B.zf[f] + 0.5;
    rr(g, B.trimStyle !== 'none' ? tc : '#e8e1cd', lo, -z, hi - lo, tr.board ? 1.7 : 1.4);
    rr(g, 'rgba(25,15,35,.28)', lo, -z + (tr.board ? 1.7 : 1.4), hi - lo, 1.2);
  }
}

function paintDoor(B, plan) {
  const { g, S } = B;
  const d = plan.door;
  if (!d) return;
  g.save();
  g.translate(0, -B.Z0);
  if (S.day) {
    const exact = !B.doorStyle && B.trimStyle === 'plain' && !B.trimHex;
    if (exact) mapDoor(S, d.x, d.w, d.h, B.doorCol, { lamp: false });
    else DOOR_ART[B.doorStyle || 'panel'](B, d.x, d.w, d.h, B.doorCol, B.frameCol);
  }
  g.restore();
}

function faceLight(B, blk, side, plan) {
  const { g } = B;
  g.save();
  g.translate(0, -B.Z0);
  const f = { lo: plan.lo, hi: plan.hi, len: plan.len, door: plan.door, wins: plan.wins.map((w) => ({ ...w, z: w.z - B.Z0 })), top: plan.top, zBase: plan.zBase };
  WALL_LIGHT_ART[B.wallLight](B, f, B.wallLightCol);
  g.restore();
}

// The band of colour over a club's or a bar's door: where it is up the wall and how tall.
const BANDS = {
  club: { z: 11.4, h: 3, all: true },
  bar: { z: 10, h: 2.4, all: false },
};

// A workshop's big door, a wide one of boards beside its own door.
function bigDoor(B, plan) {
  const { g } = B;
  const x0 = Math.max(3, plan.lo + 1.2);
  const x1 = Math.min(3 + plan.len / 2, plan.door.x + 1.5, plan.hi - 1);
  if (x1 - x0 < 3) return;
  g.save();
  g.translate(0, -B.Z0);
  rr(g, '#5b4a3a', x0, -10, x1 - x0, 10);
  for (let y = -9; y < 0; y += 2) line(g, 'rgba(30,20,10,.35)', 0.6, x0, y, x1, y);
  g.restore();
}

function faceDetail(B, blk) {
  const { g, S } = B;
  return (side, len) => {
    const plan = planFace(B, blk, side, len);
    if (S.day) wallDressing(B, blk, side, plan);
    // The windows are painted in the order the old buildings painted them in, ground floor first. A house's
    // door goes over its window, and the others' windows go over their door, as the old ones did.
    const door = () => {
      paintDoor(B, plan);
      faceLight(B, blk, side, plan);
    };
    if (S.day && B.kind === 'workshop' && blk.ground && plan.door) bigDoor(B, plan);
    if (B.kind !== 'house') door();
    // A club's sign band goes right round, a bar's is over its door.
    const band = Object.hasOwn(BANDS, B.kind) ? BANDS[B.kind] : null;
    if (band && blk.ground && (band.all || (blk === B.doorBlk && side === S.door))) {
      g.save();
      g.translate(0, -B.Z0);
      mapSign(S, 3, band.z, len - 6, band.h, B.c.awn[0]);
      g.restore();
    }
    let i = 0;
    for (const it of plan.wins) windowArt(B, it.x, it.z, it.w, it.h, { ...windowOptions(B, it, i++), fixedSize: true });
    if (B.kind === 'house') door();
    if (S.day && B.kind === 'office' && blk.ground) {
      // An office's ledges, between its rows of windows.
      g.save();
      g.translate(0, -B.Z0);
      for (const z of [11.5, 20]) ledge(S, plan.lo, z, plan.hi - plan.lo, B.trimHex || '#a9adb5');
      g.restore();
    }
    if (plan.sign !== null) {
      g.save();
      g.translate(0, -B.Z0);
      signPlate(B, plan.sign, plan.door.h + 0.6, B.signCol || B.c.awn[0]);
      g.restore();
    }
  };
}

// ---------------------------------------------------------------------------------------------
// The whole building.

function pickDoorBlock(B, blocks) {
  let best = null;
  let bestLen = -1;
  for (const blk of blocks) {
    if (!blk.ground || blk.tower) continue;
    const side = B.S.door;
    const [lo, hi] = visible(blk, side, wallLen(blk, side));
    if (hi - lo > bestLen + 0.01) {
      best = blk;
      bestLen = hi - lo;
    }
  }
  return best || blocks[0];
}

function frameOfDoor(B) {
  const blk = B.doorBlk;
  const side = B.S.door;
  const plan = planFace(B, blk, side, wallLen(blk, side));
  if (!plan.door) return null;
  return { blk, side, plan, door: { a0: plan.door.x, a1: plan.door.x + plan.door.w, w: plan.door.w, h: plan.door.h } };
}

function paintRoof(B, spec) {
  const { S } = B;
  let style = B.roofStyle;
  if (spec.tower) style = style === 'flat' || style === 'dome' ? style : 'pyramid';
  let axis = spec.axis;
  if (style === 'gable' || style === 'cross_gable') {
    const base = spec.axis || 'u';
    axis = style === 'cross_gable' ? (base === 'u' ? 'v' : 'u') : base;
    style = 'gable';
  }
  const art = ROOF_ART[style];
  let o = spec.o;
  if (style === 'mansard') o = Math.min(o, 0.6);
  if (style === 'sawtooth') o = 0;
  const info = art(B, spec.R, spec.zE, o, axis, spec);
  if (spec.tower) B.towerTop = info.zTop;
  return info;
}

function paintFurniture(B, info, spec) {
  const { g } = B;
  // The chimney, then what's on the roof. A workshop's stands taller and smokes, whatever else is chosen.
  let chim = info.chim;
  if (B.k.smoke && B.roofStyle === 'gable' && info.slope?.face === 'L') {
    const [u0, v0, u1, v1] = spec.R;
    chim = [u0 + (u1 - u0) * (17 / 22), (v0 + v1) / 2 - Math.min(5.5, (v1 - v0) / 2 - 1.5), info.zTop - 4, info.zTop + 10, 3];
  }
  const chimneyOn = B.chimney !== 'none' && chim;
  if (chimneyOn) CHIMNEY_ART[B.chimney](B, chim, B.chimneyCol, B.chimneySet || !!B.k.smoke);
  if (B.kind === 'lounge' && info.deck && B.roofStyle === 'flat') {
    // A lounge's plants, on its roof.
    const [u0, v0, u1, v1] = spec.R;
    for (const [fu, fv] of [[4 / 22, 4 / 16], [17 / 22, 4 / 16], [10 / 22, 12 / 16]]) {
      const u = u0 + fu * (u1 - u0);
      const v = v0 + fv * (v1 - v0);
      const [x, y] = P(u, v, spec.zE);
      drawTree(g, x, y, 'bush', Math.round(u + v));
    }
  }
  if (B.kind === 'studio' && info.deck && B.roofStyle === 'flat' && (!B.rooftop.length || B.rooftop.includes('skylights'))) {
    // A studio's roof lights, for the people working underneath.
    const [u0, v0, u1, v1] = info.deck.R;
    const wu = u1 - u0 + 2;
    const k = wu / 20;
    for (let q = 0; q < 3; q++) {
      const ua = u0 - 1 + k + q * 6.3 * k;
      mapSlope(g, '#8fb7d0', [[ua, v0, info.deck.z - 17.5 + 21], [ua + 5.6 * k, v0, info.deck.z - 17.5 + 21]], [[ua, v1, info.deck.z - 17.5 + 15], [ua + 5.6 * k, v1, info.deck.z - 17.5 + 15]], 6, 2.4);
    }
  }
  for (const word of B.rooftop) {
    if (word === 'skylights' && B.kind === 'studio' && info.deck && B.roofStyle === 'flat') continue;
    ROOFTOP_ART[word](B, info, B.rooftopCol);
  }
}

function paintPorchAndAwning(B) {
  const fd = frameOfDoor(B);
  if (!fd) return;
  const { blk, side, plan, door } = fd;
  const F = faceFrame(B, side, blk.R);
  let roofed = false;
  if (B.porch !== 'none') {
    PORCH_ART[B.porch](B, F, door, B.porchCol, B.Z0, [plan.lo, plan.hi]);
    roofed = B.porch === 'covered' || B.porch === 'columns' || B.porch === 'veranda';
  }
  if (B.awning !== 'none') {
    const cols = B.awningCol ? [B.awningCol, '#fff4e4'] : B.c.awn;
    if (!B.awningSet) {
      // A café keeps the awning it has always had, across its left wall.
      mapAwning(B.S, B.R, B.zf[0] + B.H - 3, 5, B.c.awn);
    } else if (!roofed) {
      const wide = B.kind === 'cafe';
      const a0 = wide ? plan.lo + 0.5 : Math.max(plan.lo + 0.3, door.a0 - 2.6);
      const a1 = wide ? plan.hi - 0.5 : Math.min(plan.hi - 0.3, door.a1 + 2.6);
      AWNING_ART[B.awning](B, F, a0, a1, B.Z0 + door.h + 1.6, wide ? 5 : 4.2, cols);
    } else {
      // Under a porch roof the awning goes over the windows of the other wall.
      const other = side === 'L' ? 'R' : 'L';
      const ob = B.blocks.find((b) => b.ground && !b.tower) || blk;
      const olen = wallLen(ob, other);
      const [lo, hi] = visible(ob, other, olen);
      const F2 = faceFrame(B, other, ob.R);
      AWNING_ART[B.awning](B, F2, lo + 0.5, hi - 0.5, B.zf[0] + 10.2, 4.2, cols);
    }
  }
}

// The raised plinth under the whole building, and its steps are in the porch or at the door.
function plinth(B) {
  if (B.foundation !== 'raised') return;
  const [u0, v0, u1, v1] = B.R;
  const R = [u0 - 0.5, v0 - 0.5, u1 + 0.5, v1 + 0.5];
  box(B.S, R, 0, B.Z0, { k: 'stone', c: '#9a958a', paint: paintStone('#9a958a') });
  topFace(B.S, R, B.Z0, '#bdb8ad', 'rgba(25,15,35,.25)');
}

function doorSteps(B) {
  // A raised building with no porch still needs a way up to its door.
  if (B.foundation !== 'raised' || B.porch !== 'none') return;
  const fd = frameOfDoor(B);
  if (!fd) return;
  const F = faceFrame(B, fd.side, fd.blk.R);
  const { door } = fd;
  const top = B.Z0;
  F.box(door.a0 - 1, door.a1 + 1, 0, 1.4, 0, top - 0.5, STONE_STEP);
  stairs(B, F, door.a0 - 1, door.a1 + 1, 1.4, top - 0.5, 0, STONE_STEP);
}

// Where a building's door meets the ground, as `[u, v]` in units, on the side `S.door` would put it. The
// path, the gate and a pet on guard all go by this, so they meet the door wherever a kind puts it.
export function doorPoint(kind, outside, side) {
  const S = { g: null, day: true, v: 0, pal: 0, door: side, seed: 0, n: 0, info: null };
  const B = look(S, kind, outside, { wall: '#ffffff', roof: '#ffffff', door: '#ffffff', awn: ['#ffffff', '#ffffff'], hue: 0 });
  if (!B) return null;
  B.blocks = SHAPE_ART[B.shape](B).blocks;
  B.doorBlk = pickDoorBlock(B, B.blocks);
  const fd = frameOfDoor(B);
  if (!fd) return null;
  const [u0, , u1, v1] = fd.blk.R;
  const middle = fd.plan.door.x + fd.plan.door.w / 2;
  return side === 'R' ? [u1, v1 - middle] : [u0 + middle, v1];
}

// A warm pool on the ground in front of a porch lamp, where the map has always put one by a door. It lies
// flat in iso: a circle in u and v, which comes out twice as wide as it is tall, fading to nothing before
// its edge, and kept on the tile so it never spills over a neighbour.
function lampPool(B) {
  if (B.wallLight !== 'porch_lamp') return;
  const fd = frameOfDoor(B);
  if (!fd) return;
  const [u0, , u1, v1] = fd.blk.R;
  const middle = fd.plan.door.x + fd.plan.door.w / 2;
  const [cu, cv] = fd.side === 'R' ? [u1 + 2, v1 - middle] : [u0 + middle, v1 + 2];
  const u = clamp(cu, 1.5, 30.5);
  const v = clamp(cv, 1.5, 30.5);
  const r = Math.min(11, u, v, 32 - u, 32 - v) - 0.5;
  if (!(r > 2.5)) return;
  const { g } = B;
  const col = B.wallLightCol || '#ffc46e';
  g.save();
  g.transform(1, 0.5, -1, 0.5, 0, 0);
  const pool = g.createRadialGradient(u, v, 0, u, v, r);
  pool.addColorStop(0, rgba(col, 0.3));
  pool.addColorStop(0.45, rgba(col, 0.16));
  pool.addColorStop(1, rgba(col, 0));
  g.fillStyle = pool;
  g.fillRect(u - r, v - r, r * 2, r * 2);
  g.restore();
}

export function paintBuilding(S, kind, outside, c) {
  const B = look(S, kind, outside, c);
  if (!B) return;
  const shaped = SHAPE_ART[B.shape](B);
  B.blocks = shaped.blocks;
  B.doorBlk = pickDoorBlock(B, shaped.blocks);
  const { g } = B;
  if (!S.day) {
    // Only what shines: the windows and lamps of each block, then the lights along the roof.
    lampPool(B);
    for (const blk of shaped.blocks) box(S, blk.R, blk.z0, blk.z1, B.mat, faceDetail(B, blk));
    nightRoof(B, shaped);
    return;
  }
  const tops = [];
  const casts = shaped.blocks.map((blk) => {
    shadow(S, blk.R, blk.z1);
    return S.info ? S.info.shadows[S.info.shadows.length - 1] : null;
  });
  plinth(B);
  let main = null;
  for (let i = 0; i < shaped.blocks.length; i++) {
    const blk = shaped.blocks[i];
    box(S, blk.R, blk.z0, blk.z1, B.mat, faceDetail(B, blk));
    if (blk.terrace) topFace(S, blk.R, blk.z1, '#bdb4a0', 'rgba(25,15,35,.25)');
    for (const spec of shaped.roofs) {
      if (spec.after !== i) continue;
      B.curCut = spec.cut;
      const info = paintRoof(B, spec);
      tops.push([spec.R, spec.main && B.k.pshade && B.roofStyle === B.k.roof ? spec.zE + B.k.pshade : info.sh ?? info.zTop, spec, i]);
      if (spec.seam) seamOf(B, shaped, info);
      if (spec.main) {
        main = info;
        paintFurniture(B, info, spec);
      }
    }
  }
  main = main || {};
  doorSteps(B);
  paintPorchAndAwning(B);
  if (B.stringStyle !== 'none' && main.eave) lightsAlong(B, main);
  // The map casts one shadow for all of it.
  for (const [, top, , i] of tops) if (casts[i]) casts[i][1] = top;
  if (B.kind === 'house' && !B.yardFence && !outside?.body) {
    picket(S, 4, 29, 11, 29);
  }
  B.main = main;
}

// Where two roofs of an L meet, the seam between their decks is painted over.
function seamOf(B, shaped, info) {
  if (!info.deck) return;
  const [a, b] = shaped.roofs.map((r) => r.R);
  const z = info.deck.z;
  const c = B.roofHex ? dark(B.roofHex, 0.14) : '#686f7e';
  fp3(B.g, c, [a[2] - 1, b[1] + 1, z], [a[2] + 1, b[1] + 1, z], [a[2] + 1, b[3] - 1, z], [a[2] - 1, b[3] - 1, z]);
}

function lightsAlong(B, info) {
  const pts = info.eave.map((p) => [p[0], p[1], p[2] - (B.stringStyle === 'neon' ? 0.3 : B.trim.fascia + 0.6)]);
  STRING_ART[B.stringStyle](B, pts, B.stringCol);
}

function nightRoof(B, shaped) {
  // The roofs aren't painted at night, so the edge the lights hang from has to be worked out again.
  const spec = shaped.roofs.find((r) => r.main) || shaped.roofs[0];
  const probe = probeRoof(B, spec);
  if (B.stringStyle !== 'none' && probe.eave) lightsAlong(B, probe);
  if (probe.deck && B.rooftop.includes('skylights')) glowSkylights(B, probe);
}

// A roof's outline without painting it: the same call, onto a context that draws nothing.
function probeRoof(B, spec) {
  const real = B.g;
  const dummy = new Proxy({}, { get: () => () => ({ addColorStop() {} }), set: () => true });
  B.g = dummy;
  B.S = { ...B.S, g: dummy, info: null, day: true };
  let info = {};
  try {
    info = paintRoof(B, spec);
  } catch {
    info = {};
  }
  B.g = real;
  B.S = { ...B.S, g: real, day: false };
  return info;
}

function glowSkylights(B, info) {
  const { g } = B;
  const p = spotFor(info, 'skylights');
  if (!p) return;
  const [x, y] = P3(p);
  if (ih(B.S.seed, 91, 5) % 100 < B.glowPct) glowAt(g, x, y - 2, 6, '#ffd890', 0.4);
}

// ---------------------------------------------------------------------------------------------
// What a place with no outside has, so the builder can show which tile is pressed (E4).

export function defaultStyles(kind, variant = 0) {
  const k = Object.hasOwn(KINDS, kind) ? KINDS[kind] : undefined;
  if (!k) return {};
  const base = {
    shape: 'plain',
    door_side: 'auto',
    walls: kind === 'house' ? (variant === 1 ? 'brick' : 'siding') : kindWall({ v: variant, pal: 0 }, kind, { wall: '#ffffff' }).k,
    foundation: 'none',
    trim: 'plain',
    roof: k.roof,
    roofing: 'shingles',
    chimney: kind === 'house' || k.smoke ? 'brick' : 'none',
    windows: 'square',
    shutters: 'none',
    curtains: kind === 'house' || kind === 'cafe' || k.curtain ? 'open' : 'none',
    window_boxes: 'none',
    door: 'panel',
    porch: 'none',
    awning: kind === 'cafe' ? 'striped' : 'none',
    sign: 'none',
    glow: 'evening',
    wall_lights: 'porch_lamp',
    string_lights: 'none',
  };
  return base;
}

// The same strings of lights for a place with no building, which is what a town square has (E10). `pts` are
// the corners of the edge in u, v and z, and `color` is a number's hex or null.
export function paintLightString(S, style, pts, color) {
  const art = STRING_ART[style];
  if (art && pts && pts.length > 1) art({ g: S.g, S }, pts, color || null);
}
