// Pets and their automations (E18 to E22). A pet lives in one yard, which is 32 by 32 units, and it
// follows the first of its rules that holds (E19). Nothing here posts, stores, sounds or wakes anyone
// (E21), and the only things it's told are what anyone looking at the map can see (E20).
//
// This module imports only web/outside-spec.js, so the rules can be run in Node. It never loads
// web/world.js, which would be a cycle.

import { CELL, CENTERPIECE, bodyOf, hexOf, itemsOf, partOf, rectOf, spanOf } from './outside-spec.js';

const TAU = 6.2832;
const INK = '#2a211d';

// Rules (E19, E20)

const HOURS = {
  morning: (h) => h >= 6 && h < 11,
  daytime: (h) => h >= 11 && h < 17,
  evening: (h) => h >= 17 && h < 21,
  night: (h) => h >= 21 || h < 6,
};

// E20: whether one `when` holds. A word this page doesn't know never holds (E4). `pulse` is seconds with
// the pet's own offset in them, and `now_and_then` holds for 20 of every 120.
export function holds(when, facts, pulse) {
  const f = facts || {};
  switch (when) {
    case 'always':
      return true;
    case 'morning':
    case 'daytime':
    case 'evening':
    case 'night': {
      const hour = Number(f.hour);
      return Number.isFinite(hour) && HOURS[when](((hour % 24) + 24) % 24);
    }
    case 'someone_near':
      return Boolean(f.person || f.bot || f.frog);
    case 'person_near':
      return Boolean(f.person);
    case 'bot_near':
      return Boolean(f.bot);
    case 'frog_near':
      return Boolean(f.frog);
    case 'street_empty':
      return !(f.person || f.bot || f.frog);
    case 'owner_watching':
      return Boolean(f.owner);
    case 'disaster':
      return Boolean(f.disaster);
    case 'now_and_then': {
      const n = Number(pulse);
      return Number.isFinite(n) && ((n % 120) + 120) % 120 < 20;
    }
    default:
      return false;
  }
}

// E19: the first rule whose `when` holds, or null, and then a pet wanders.
export function chooseRule(rules, facts, pulse) {
  if (!Array.isArray(rules)) return null;
  for (const rule of rules) {
    if (rule && typeof rule === 'object' && holds(rule.when, facts, pulse)) return rule;
  }
  return null;
}

// Colours

function rgbOf(hex) {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function mix(a, b, k) {
  const x = rgbOf(a);
  const y = rgbOf(b);
  const c = (i) => Math.round(x[i] + (y[i] - x[i]) * k);
  return `#${((1 << 24) | (c(0) << 16) | (c(1) << 8) | c(2)).toString(16).slice(1)}`;
}

// What a species looks like when nobody has chosen.
const USUAL = {
  dog: '#b9814a',
  cat: '#d9894a',
  rabbit: '#d8cfc0',
  chicken: '#f2ead8',
  duck: '#e8c84a',
  pig: '#f0a8b0',
  sheep: '#f1ede4',
  goat: '#e6dccb',
  fox: '#d9692a',
  hedgehog: '#7a5a3e',
  turtle: '#5e9e4a',
  parrot: '#3fae5a',
  frog: '#5aa84a',
  robot_dog: '#9aa3ad',
  turkey: '#8a5a30',
};
const USUAL_COLLAR = '#d64545';

// Drawing helpers, in art pixels with the ground at y = 0 and the pet facing right.

function ell(g, x, y, rx, ry, col, rot = 0) {
  g.beginPath();
  g.ellipse(x, y, rx, ry, rot, 0, TAU);
  g.fillStyle = col;
  g.fill();
}

function ball(g, x, y, rx, ry, col, rot = 0) {
  const gr = g.createLinearGradient(x, y - ry, x, y + ry);
  gr.addColorStop(0, mix(col, '#ffffff', 0.25));
  gr.addColorStop(0.55, col);
  gr.addColorStop(1, mix(col, '#000000', 0.22));
  ell(g, x, y, rx, ry, gr, rot);
}

function limb(g, x0, y0, x1, y1, w, col) {
  g.beginPath();
  g.moveTo(x0, y0);
  g.lineTo(x1, y1);
  g.lineWidth = w;
  g.lineCap = 'round';
  g.strokeStyle = col;
  g.stroke();
}

function curve(g, x0, y0, cx, cy, x1, y1, w, col) {
  g.beginPath();
  g.moveTo(x0, y0);
  g.quadraticCurveTo(cx, cy, x1, y1);
  g.lineWidth = w;
  g.lineCap = 'round';
  g.strokeStyle = col;
  g.stroke();
}

function poly(g, col, pts) {
  g.beginPath();
  g.moveTo(pts[0], pts[1]);
  for (let i = 2; i < pts.length; i += 2) g.lineTo(pts[i], pts[i + 1]);
  g.closePath();
  g.fillStyle = col;
  g.fill();
}

const dot = (g, x, y, r, col) => ell(g, x, y, r, r, col);

// An eye is shut when a pet lies down.
function eye(g, o, x, y, r = 0.45) {
  if (o.pose === 'lie') {
    limb(g, x - r, y, x + r, y, 0.3, INK);
    return;
  }
  dot(g, x, y, r, INK);
  dot(g, x + r * 0.3, y - r * 0.35, r * 0.3, '#ffffff');
}

// A collar is a band round the neck.
function band(g, o, x, y, ry, rot) {
  ell(g, x, y, 0.5, ry, o.collar, rot);
}

// Four-legged figures

function quad(g, o, P) {
  const { pose, f } = o;
  const { bl, bh, leg, lw } = P;
  const legc = P.legCol || mix(o.coat, '#000000', 0.18);
  const farc = mix(legc, '#000000', 0.28);
  let cx = 0;
  let cy = -(leg + bh);
  let rot = 0;
  if (pose === 'lie') cy = -bh * 0.95;
  else if (pose === 'sit') {
    rot = -0.62;
    cx = -bl * 0.3;
    cy = -(0.41 * bl + 0.65 * bh);
  } else if (pose === 'hop') cy = -(leg * 0.55 + bh);
  else if (pose === 'walk' && f & 1) cy -= 0.3;
  const hipY = cy + bh * 0.55;
  const stride = P.stride || 1.4;
  const leg4 = (x, ph, col) => {
    let fx = x;
    let fy = 0;
    if (pose === 'walk') {
      const a = (f / 4 + ph) * TAU;
      fx = x + Math.sin(a) * stride;
      fy = -Math.max(0, Math.cos(a)) * 0.9;
    } else if (pose === 'hop') {
      fx = x + (x < cx ? -0.9 : 0.9);
      fy = -leg * 0.5;
    }
    limb(g, x, hipY, fx, fy, lw, col);
    if (fy === 0) ell(g, fx + 0.3, -0.25, lw * 0.75, 0.35, col);
  };
  const hindX = cx - bl * 0.55;
  const foreX = cx + bl * 0.6;
  const upright = pose === 'stand' || pose === 'walk' || pose === 'hop';
  if (upright) {
    leg4(hindX, 0.5, farc);
    leg4(foreX, 0, farc);
  }
  g.save();
  g.translate(cx, cy);
  g.rotate(rot);
  P.tail?.(g, o, -bl * 0.85, -bh * 0.15);
  if (P.body) P.body(g, o);
  else {
    ball(g, 0, 0, bl, bh, o.coat);
    ell(g, 0.2, bh * 0.45, bl * 0.7, bh * 0.4, mix(o.coat, '#ffffff', 0.2));
  }
  const hx = bl * 0.8 + (P.hx || 0);
  const hy = pose === 'lie' ? bh - P.hr * 0.9 : -P.hy;
  P.head(g, o, hx, hy);
  if (P.collared !== false) band(g, o, hx - P.hr * 0.45, hy + P.hr * 0.8, P.hr * 0.75, 0.35);
  g.restore();
  if (upright) {
    leg4(hindX, 0, legc);
    leg4(foreX, 0.5, legc);
  } else if (pose === 'sit') {
    ball(g, cx - bl * 0.3, -bh * 0.8, bh * 1.05, bh * 0.85, mix(o.coat, '#000000', 0.08));
    ell(g, cx + bl * 0.1, -0.3, bh * 0.9, 0.4, legc);
    const fx = cx + bl * 0.7 * Math.cos(0.62) + bh * 0.2 * Math.sin(0.62);
    limb(g, fx, cy - bl * 0.7 * Math.sin(0.62) + bh * 0.35, fx, -0.2, lw, legc);
    ell(g, fx + 0.3, -0.25, lw * 0.75, 0.35, legc);
  } else {
    ball(g, cx - bl * 0.3, -bh * 0.7, bl * 0.55, bh * 0.7, mix(o.coat, '#000000', 0.1));
    ell(g, bl * 0.85, -0.5, 1.1, 0.5, legc);
  }
}

// A tail that wags when `f` changes, and lies flat when the pet does.
function wagTail(len, w, thick, col) {
  return (g, o, x, y) => {
    const k = o.f ? 0.7 : -0.3;
    if (o.pose === 'lie') curve(g, x, y + 0.4, x - len * 0.6, y + 1.4, x - len, y + 1.7, w, col(o));
    else curve(g, x, y, x - len * 0.6, y - len * thick, x - len * 0.9, y - len * 1.1 + k, w, col(o));
  };
}

const ARTS = {
  dog(g, o) {
    const ear = mix(o.coat, '#000000', 0.32);
    quad(g, o, {
      bl: 3.7, bh: 2.1, leg: 2.6, lw: 1.1, hx: 1.0, hy: 2.0, hr: 1.8,
      tail: wagTail(2.6, 1, 0.7, (q) => q.coat),
      head(g, o, x, y) {
        ball(g, x, y, 1.8, 1.65, o.coat);
        ell(g, x + 1.7, y + 0.55, 1.25, 0.85, mix(o.coat, '#ffffff', 0.35));
        dot(g, x + 2.8, y + 0.25, 0.5, INK);
        eye(g, o, x + 0.55, y - 0.35);
        ell(g, x - 0.7, y - 0.1, 0.85, 1.55, ear, 0.25);
      },
    });
  },
  cat(g, o) {
    const stripe = mix(o.coat, '#000000', 0.35);
    quad(g, o, {
      bl: 3.2, bh: 1.7, leg: 2.3, lw: 0.9, hx: 0.8, hy: 1.9, hr: 1.6, stride: 1.2,
      tail(g, o, x, y) {
        if (o.pose === 'lie') curve(g, x, y + 0.6, x - 2, y + 1.6, x - 3.4, y + 0.8, 1, o.coat);
        else curve(g, x, y, x - 3.2, y + 0.3, x - 2.6 + (o.f ? 0.8 : 0), y - 4.6, 1, o.coat);
      },
      body(g, o) {
        ball(g, 0, 0, 3.2, 1.7, o.coat);
        for (const sx of [-1.4, -0.2, 1]) limb(g, sx, -1.6, sx + 0.2, -0.3, 0.45, stripe);
        ell(g, 0.2, 0.8, 2.2, 0.7, mix(o.coat, '#ffffff', 0.3));
      },
      head(g, o, x, y) {
        poly(g, o.coat, [x - 1.3, y - 0.8, x - 0.9, y - 2.7, x - 0.1, y - 1.3]);
        poly(g, o.coat, [x + 0.1, y - 1.3, x + 0.9, y - 2.7, x + 1.2, y - 0.6]);
        poly(g, '#e89aa6', [x - 1, y - 1, x - 0.8, y - 2.1, x - 0.3, y - 1.2]);
        ball(g, x, y, 1.6, 1.4, o.coat);
        eye(g, o, x + 0.5, y - 0.3, 0.4);
        dot(g, x + 1.45, y + 0.25, 0.3, '#e0707e');
        limb(g, x + 1.2, y + 0.4, x + 2.6, y + 0.1, 0.18, '#ffffff');
        limb(g, x + 1.2, y + 0.55, x + 2.6, y + 0.9, 0.18, '#ffffff');
      },
    });
  },
  rabbit(g, o) {
    quad(g, o, {
      bl: 2.7, bh: 2.2, leg: 1.4, lw: 1.1, hx: 0.5, hy: 1.5, hr: 1.5, stride: 1,
      tail(g, o, x, y) {
        dot(g, x - 0.1, y + 0.8, 1, '#fbf7f0');
      },
      body(g, o) {
        ball(g, 0, 0, 2.7, 2.2, o.coat);
        ball(g, -1.1, 0.5, 1.5, 1.4, mix(o.coat, '#000000', 0.08));
        ell(g, 0.8, 1, 1.5, 0.9, mix(o.coat, '#ffffff', 0.4));
      },
      head(g, o, x, y) {
        const lop = o.pose === 'lie' ? 1.4 : 0;
        ell(g, x - 0.9, y - 3 + lop, 0.65, 2.5, o.coat, -0.2 - lop * 0.4);
        ell(g, x - 0.9, y - 3 + lop, 0.3, 1.9, '#f0b0b8', -0.2 - lop * 0.4);
        ell(g, x - 0.1, y - 3.2 + lop, 0.65, 2.5, mix(o.coat, '#000000', 0.1), 0.15 - lop * 0.4);
        ball(g, x, y, 1.5, 1.35, o.coat);
        eye(g, o, x + 0.6, y - 0.3, 0.4);
        dot(g, x + 1.5, y + 0.2, 0.3, '#d97a86');
      },
    });
  },
  pig(g, o) {
    const snout = mix(o.coat, '#c0507a', 0.35);
    quad(g, o, {
      bl: 3.6, bh: 2.7, leg: 1.4, lw: 1.2, hx: 0.3, hy: 0.4, hr: 2.0, stride: 1,
      legCol: mix(o.coat, '#000000', 0.12),
      tail(g, o, x, y) {
        g.beginPath();
        g.arc(x - 0.4, y + 0.3, 0.7, 0.6, 5.2);
        g.lineWidth = 0.55;
        g.lineCap = 'round';
        g.strokeStyle = mix(o.coat, '#000000', 0.15);
        g.stroke();
      },
      head(g, o, x, y) {
        poly(g, mix(o.coat, '#000000', 0.12), [x - 1.1, y - 1.2, x - 0.8, y - 2.5, x + 0.4, y - 1.3]);
        ball(g, x, y, 2, 1.9, o.coat);
        ell(g, x + 1.9, y + 0.5, 0.95, 1, snout);
        dot(g, x + 2.2, y + 0.3, 0.2, INK);
        dot(g, x + 2.2, y + 0.9, 0.2, INK);
        eye(g, o, x + 0.5, y - 0.5);
      },
    });
  },
  sheep(g, o) {
    const face = '#3d3a38';
    quad(g, o, {
      bl: 3.5, bh: 2.5, leg: 1.8, lw: 0.9, hx: 0.5, hy: 1.4, hr: 1.5, stride: 1.1,
      legCol: face,
      tail(g, o, x, y) {
        ell(g, x, y + 0.4, 0.9, 0.9, o.coat);
      },
      body(g, o) {
        const hi = mix(o.coat, '#ffffff', 0.35);
        const lo = mix(o.coat, '#000000', 0.18);
        for (const [x, y, r] of [[-2.4, 0.3, 1.5], [2.3, 0.4, 1.4], [-1.2, 1.2, 1.5], [1.1, 1.3, 1.4], [0, 1.6, 1.2]]) dot(g, x, y, r, lo);
        for (const [x, y, r] of [[-2.2, -0.5, 1.6], [-0.6, -1.2, 1.7], [1.2, -1, 1.7], [2.4, -0.2, 1.4], [0.3, 0.1, 1.9], [-1.4, 0.5, 1.6], [1.6, 0.8, 1.4]]) dot(g, x, y, r, o.coat);
        for (const [x, y, r] of [[-1.4, -1.8, 0.8], [0.4, -2, 0.9], [1.7, -1.6, 0.7]]) dot(g, x, y, r, hi);
      },
      head(g, o, x, y) {
        ell(g, x - 0.7, y - 0.2, 0.8, 0.5, face, 0.5);
        ball(g, x + 0.2, y + 0.2, 1.35, 1.5, face, 0.1);
        dot(g, x - 0.1, y - 1.2, 0.9, o.coat);
        ell(g, x + 1.3, y + 0.6, 0.45, 0.4, '#5a5552');
        o.pose === 'lie' ? limb(g, x + 0.2, y - 0.1, x + 0.9, y - 0.1, 0.3, '#ffffff') : dot(g, x + 0.6, y - 0.1, 0.32, '#ffffff');
      },
    });
  },
  goat(g, o) {
    const dk = mix(o.coat, '#000000', 0.35);
    quad(g, o, {
      bl: 3.4, bh: 1.9, leg: 3, lw: 0.9, hx: 0.8, hy: 2.3, hr: 1.5, stride: 1.4,
      legCol: mix(o.coat, '#000000', 0.2),
      tail(g, o, x, y) {
        poly(g, o.coat, [x, y - 0.2, x - 1.1, y - 1.5, x - 0.2, y + 0.5]);
      },
      head(g, o, x, y) {
        curve(g, x - 0.4, y - 1, x - 1.4, y - 2.6, x - 2.3, y - 1.8, 0.55, '#d8cdb8');
        curve(g, x + 0.1, y - 1.1, x - 0.5, y - 2.8, x - 1.6, y - 2.3, 0.45, '#cbbfa8');
        ell(g, x - 1, y - 0.1, 1, 0.45, dk, 0.4);
        ball(g, x, y, 1.45, 1.3, o.coat);
        ell(g, x + 1.3, y + 0.5, 1, 0.8, mix(o.coat, '#ffffff', 0.35), 0.25);
        poly(g, '#f3eee4', [x + 0.8, y + 1.1, x + 1.9, y + 1, x + 1.5, y + 2.7]);
        dot(g, x + 2.1, y + 0.4, 0.3, INK);
        eye(g, o, x + 0.4, y - 0.3, 0.4);
      },
    });
  },
  fox(g, o) {
    const black = '#2a1f1b';
    quad(g, o, {
      bl: 3.5, bh: 1.7, leg: 2.9, lw: 0.9, hx: 0.9, hy: 1.9, hr: 1.5, stride: 1.5,
      legCol: black,
      tail(g, o, x, y) {
        const up = o.pose === 'lie' ? 0.6 : -1;
        ball(g, x - 2.2, y + 0.4 + up * 0.8, 2.6, 1.2, o.coat, o.pose === 'lie' ? 0.1 : -0.5 + (o.f ? 0.15 : 0));
        ell(g, x - 4.1, y + 0.2 + up * 1.6, 1, 0.9, '#fbf7f0', o.pose === 'lie' ? 0.1 : -0.5);
      },
      body(g, o) {
        ball(g, 0, 0, 3.5, 1.7, o.coat);
        ell(g, 1.8, 0.9, 1.8, 0.8, '#fbf7f0');
      },
      head(g, o, x, y) {
        poly(g, black, [x - 1.3, y - 0.8, x - 1, y - 3, x - 0.1, y - 1.2]);
        poly(g, o.coat, [x - 1.1, y - 0.9, x - 0.9, y - 2.4, x - 0.2, y - 1.1]);
        poly(g, black, [x + 0.1, y - 1.2, x + 0.8, y - 3, x + 1.2, y - 0.6]);
        poly(g, o.coat, [x + 0.2, y - 1.1, x + 0.8, y - 2.4, x + 1.1, y - 0.7]);
        ball(g, x, y, 1.5, 1.3, o.coat);
        poly(g, '#fbf7f0', [x - 0.2, y + 0.8, x + 1.2, y - 0.2, x + 1.1, y + 1.2]);
        poly(g, o.coat, [x + 0.7, y - 0.5, x + 3.3, y + 0.6, x + 1, y + 0.9]);
        dot(g, x + 3.2, y + 0.5, 0.4, black);
        eye(g, o, x + 0.5, y - 0.3, 0.4);
      },
    });
  },
  robot_dog(g, o) {
    const metal = o.coat;
    const dkm = mix(metal, '#000000', 0.4);
    const light = o.f ? '#ff5252' : '#6b2222';
    quad(g, o, {
      bl: 3.7, bh: 2.0, leg: 2.6, lw: 1.2, hx: 0.9, hy: 2.1, hr: 1.7, stride: 1.2,
      legCol: dkm,
      collared: true,
      tail(g, o, x, y) {
        limb(g, x, y, x - 1.6, y - 1.6, 0.5, dkm);
        dot(g, x - 1.7, y - 1.7, 0.45, light);
      },
      body(g, o) {
        poly(g, dkm, [-3.7, -1.4, -3.2, 2, 3.4, 2, 3.8, -1.4]);
        poly(g, metal, [-3.7, -2, -3.4, 1.5, 3.4, 1.5, 3.8, -2, 3, -2.2, -3, -2.2]);
        poly(g, mix(metal, '#ffffff', 0.3), [-3.1, -2.2, 3.1, -2.2, 3.7, -1.8, -3.5, -1.8]);
        limb(g, -0.9, -1.9, -0.9, 1.3, 0.25, dkm);
        limb(g, 1.5, -1.9, 1.5, 1.3, 0.25, dkm);
        dot(g, 0.3, -0.1, 0.4, light);
        dot(g, -2.1, -0.4, 0.22, dkm);
        dot(g, -2.1, 0.5, 0.22, dkm);
      },
      head(g, o, x, y) {
        limb(g, x - 0.3, y - 1.4, x - 0.9, y - 2.8, 0.35, dkm);
        dot(g, x - 0.9, y - 2.9, 0.4, light);
        poly(g, dkm, [x - 1.8, y - 1.5, x + 2.8, y - 1.2, x + 2.9, y + 1.5, x - 1.8, y + 1.6]);
        poly(g, metal, [x - 1.7, y - 1.5, x + 2.7, y - 1.2, x + 2.7, y + 1.2, x - 1.7, y + 1.4]);
        poly(g, mix(metal, '#ffffff', 0.3), [x - 1.7, y - 1.5, x + 2.7, y - 1.2, x + 2.6, y - 0.8, x - 1.7, y - 1.1]);
        poly(g, o.pose === 'lie' ? dkm : '#7fe3ff', [x + 0.2, y - 0.6, x + 2.4, y - 0.5, x + 2.4, y + 0.3, x + 0.2, y + 0.2]);
        poly(g, dkm, [x + 1.2, y + 0.6, x + 2.8, y + 0.7, x + 2.8, y + 1.4, x + 1.2, y + 1.3]);
        poly(g, dkm, [x - 2, y - 0.7, x - 1.5, y - 0.8, x - 1.5, y + 0.7, x - 2, y + 0.7]);
      },
    });
  },

  // Everything below stands its own way.

  chicken(g, o) {
    const { pose, f } = o;
    const low = pose === 'sit' || pose === 'lie';
    const by = low ? -2.5 : -4.3;
    const beak = '#f2a830';
    if (!low) {
      for (const [x, ph] of [[-0.5, 0.5], [1, 0]]) {
        let fx = x;
        let fy = 0;
        if (pose === 'walk') {
          const a = (f / 4 + ph) * TAU;
          fx = x + Math.sin(a) * 1.2;
          fy = -Math.max(0, Math.cos(a)) * 0.8;
        }
        limb(g, x, by + 1.6, fx, fy, 0.5, '#e0a030');
        limb(g, fx - 0.4, fy, fx + 0.9, fy, 0.45, '#e0a030');
      }
    }
    const dk = mix(o.coat, '#000000', 0.25);
    ell(g, -3.1, by - 1.2, 1.7, 0.8, dk, -0.9);
    ell(g, -3.2, by - 0.6, 1.7, 0.7, mix(o.coat, '#000000', 0.12), -0.4);
    ball(g, 0, by, 3.1, 2.4, o.coat, low ? 0 : 0.12);
    const wy = pose === 'hop' ? -1.2 : 0;
    ell(g, -0.3, by + 0.2 + wy, 1.9, 1.2, mix(o.coat, '#000000', 0.14), pose === 'hop' ? -0.9 : 0.2);
    const hx = pose === 'lie' ? 2.1 : 2.5;
    const hy = pose === 'lie' ? by - 0.7 : by - 2.8;
    ball(g, hx, hy, 1.4, 1.4, o.coat);
    poly(g, beak, [hx + 1.1, hy - 0.2, hx + 2.4, hy + 0.2, hx + 1.1, hy + 0.7]);
    for (const [dx, r] of [[-0.6, 0.55], [0.1, 0.7], [0.8, 0.5]]) dot(g, hx + dx, hy - 1.35, r, '#d94040');
    ell(g, hx + 1, hy + 1.1, 0.4, 0.7, '#d94040');
    eye(g, o, hx + 0.4, hy - 0.2, 0.35);
    ell(g, hx - 0.4, hy + 1.2, 0.5, 0.35, o.collar, 0.2);
  },
  duck(g, o) {
    const { pose, f } = o;
    const low = pose === 'sit' || pose === 'lie';
    const by = low ? -1.9 : -2.9;
    const orange = '#f08a24';
    if (!low) {
      for (const [x, ph] of [[-0.6, 0.5], [0.9, 0]]) {
        let fx = x;
        let fy = 0;
        if (pose === 'walk') {
          const a = (f / 4 + ph) * TAU;
          fx = x + Math.sin(a) * 1;
          fy = -Math.max(0, Math.cos(a)) * 0.7;
        }
        limb(g, x, by + 1.5, fx, fy - 0.1, 0.5, orange);
        poly(g, orange, [fx - 0.5, fy, fx + 1.5, fy, fx + 0.3, fy - 0.7]);
      }
    }
    poly(g, mix(o.coat, '#000000', 0.12), [-2.8, by - 0.2, -4.8, by - 1.6, -3.6, by + 1.2]);
    ball(g, 0, by, 3.3, 2.1, o.coat);
    ell(g, -0.3, by + 0.1, 2, 1.1, mix(o.coat, '#000000', 0.16), 0.15);
    const hx = pose === 'lie' ? 1.2 : 2.5;
    const hy = pose === 'lie' ? by - 1.6 : by - 2.5;
    ball(g, hx, hy, 1.45, 1.45, mix(o.coat, '#ffffff', 0.12));
    ell(g, hx + 1.7, hy + 0.5, 1.3, 0.55, orange);
    eye(g, o, hx + 0.4, hy - 0.3, 0.38);
    ell(g, hx - 0.3, hy + 1.3, 0.55, 0.35, o.collar, 0.2);
  },
  parrot(g, o) {
    const { pose, f } = o;
    const low = pose === 'sit' || pose === 'lie';
    const by = low ? -3.1 : -4.8;
    const blue = '#2f6fd6';
    const red = '#e5483f';
    const yellow = '#f4c542';
    limb(g, -2.4, by + 1, -1.2, 0.3, 1.5, red);
    limb(g, -3.2, by + 1.4, -2.4, 0.3, 1.3, blue);
    if (!low) {
      for (const [x, ph] of [[-0.3, 0.5], [0.9, 0]]) {
        let fx = x;
        let fy = 0;
        if (pose === 'walk') {
          const a = (f / 4 + ph) * TAU;
          fx = x + Math.sin(a) * 0.9;
          fy = -Math.max(0, Math.cos(a)) * 0.7;
        }
        limb(g, x, by + 2.2, fx, fy, 0.5, '#8a7a6a');
        limb(g, fx - 0.4, fy, fx + 0.6, fy, 0.45, '#8a7a6a');
      }
    }
    ball(g, 0, by, 2, low ? 2.3 : 3, o.coat, 0.15);
    ell(g, 0.5, by + 0.8, 1, 1.6, mix(o.coat, '#ffffff', 0.35), 0.15);
    const wy = pose === 'hop' ? -1.4 : 0;
    ell(g, -0.9, by + 0.2 + wy, 1.3, 2.4, blue, pose === 'hop' ? -0.9 : 0.25);
    ell(g, -1.1, by + 1.6 + wy, 1, 1.2, red, 0.25);
    ell(g, -0.7, by - 0.6 + wy, 0.9, 1, yellow, 0.2);
    const hx = pose === 'lie' ? 1 : 1.1;
    const hy = pose === 'lie' ? by - 1.7 : by - 3.2;
    ball(g, hx, hy, 1.6, 1.6, mix(o.coat, '#ffffff', 0.1));
    poly(g, '#f0f0e8', [hx - 0.2, hy - 0.8, hx + 1.4, hy - 0.4, hx + 1.2, hy + 0.7, hx - 0.3, hy + 0.3]);
    g.beginPath();
    g.moveTo(hx + 0.9, hy - 0.8);
    g.quadraticCurveTo(hx + 3.2, hy - 0.8, hx + 2.3, hy + 1.5);
    g.quadraticCurveTo(hx + 2.1, hy + 0.5, hx + 0.9, hy + 0.5);
    g.closePath();
    g.fillStyle = '#2d2a28';
    g.fill();
    eye(g, o, hx + 0.5, hy - 0.4, 0.4);
    ell(g, hx - 0.5, hy + 1.5, 0.55, 0.35, o.collar, 0.2);
    if (f && pose === 'stand') dot(g, hx + 0.5, hy - 2.1, 0.5, red);
  },
  hedgehog(g, o) {
    const { pose, f } = o;
    const spike = o.coat;
    const tip = mix(o.coat, '#ffffff', 0.4);
    const face = '#e0c19a';
    const curled = pose === 'lie';
    const cy = curled ? -2.9 : pose === 'sit' ? -2.6 : -2.1;
    const rx = curled ? 2.9 : 3.4;
    const ry = curled ? 2.9 : 2.6;
    if (!curled) {
      for (const [x, ph] of [[-1.3, 0.5], [1.7, 0]]) {
        let fx = x;
        if (pose === 'walk') fx = x + Math.sin((f / 4 + ph) * TAU) * 0.9;
        limb(g, x, cy + 1.2, fx, -0.1, 0.8, '#8a6b52');
        ell(g, fx + 0.3, -0.2, 0.7, 0.35, '#8a6b52');
      }
    }
    ball(g, curled ? 0 : -0.5, cy, rx, ry, mix(spike, '#000000', 0.2));
    const n = curled ? 18 : 15;
    for (let i = 0; i < n; i++) {
      const a = curled ? (i / n) * TAU : 3.14 + 0.15 + (i / (n - 1)) * 3.0;
      const c = Math.cos(a);
      const s = Math.sin(a);
      const bx = (curled ? 0 : -0.5) + c * rx * 0.85;
      const by = cy + s * ry * 0.85;
      const len = 1.6 + (i % 3) * 0.3;
      limb(g, bx, by, bx + c * len, by + s * len, 0.75, i % 2 ? spike : mix(spike, '#000000', 0.15));
      dot(g, bx + c * len, by + s * len, 0.25, tip);
    }
    ball(g, curled ? 0 : -0.5, cy - 0.3, rx * 0.8, ry * 0.8, spike);
    if (!curled) {
      poly(g, face, [2.4, cy - 1.3, 5.9, cy + 1.2 + (pose === 'sit' ? -0.4 : 0), 2.4, cy + 1.9]);
      ell(g, 2.9, cy + 0.4, 1.4, 1.5, face);
      dot(g, 5.7, cy + 1.1, 0.5, INK);
      eye(g, o, 3.7, cy + 0.1, 0.4);
      ell(g, 2.3, cy - 0.6, 0.65, 0.8, '#c8a47c', 0.3);
    }
  },
  turtle(g, o) {
    const { pose, f } = o;
    const skin = '#9bb26a';
    const hidden = pose === 'lie';
    const cy = hidden ? -1.4 : -2.4;
    const skinD = mix(skin, '#000000', 0.2);
    if (!hidden) {
      for (const [x, ph] of [[-2.2, 0.5], [2, 0]]) {
        let fx = x;
        if (pose === 'walk') fx = x + Math.sin((f / 4 + ph) * TAU) * 0.8;
        limb(g, x, cy + 0.4, fx, -0.1, 1.3, skinD);
        ell(g, fx + 0.2, -0.2, 0.9, 0.4, skinD);
      }
      for (const [x, ph] of [[-1.2, 0], [2.7, 0.5]]) {
        let fx = x;
        if (pose === 'walk') fx = x + Math.sin((f / 4 + ph) * TAU) * 0.8;
        limb(g, x, cy + 0.4, fx, -0.1, 1.3, skin);
        ell(g, fx + 0.2, -0.2, 0.9, 0.4, skin);
      }
      poly(g, skin, [-3.6, cy + 0.6, -5, cy + 1.2, -3.6, cy + 1.2]);
    }
    const out = pose === 'sit' ? 0.4 : 1;
    if (!hidden) {
      limb(g, 3.2, cy, 3.4 + 1.2 * out, cy - 0.7 * out, 1.4, skin);
      ball(g, 3.7 + 1.6 * out, cy - 0.9 * out, 1.2, 1.1, skin);
      if (out > 0.5) eye(g, o, 4.3 + 1.6 * out, cy - 1.2 * out, 0.33);
    }
    const sh = hidden ? 1.7 : 2.7;
    g.beginPath();
    g.ellipse(0, cy + 0.3, 3.9, sh, 0, 3.1416, 6.2832);
    g.closePath();
    const gr = g.createLinearGradient(0, cy - sh, 0, cy + 0.3);
    gr.addColorStop(0, mix(o.coat, '#ffffff', 0.25));
    gr.addColorStop(1, mix(o.coat, '#000000', 0.25));
    g.fillStyle = gr;
    g.fill();
    ell(g, 0, cy + 0.5, 3.9, 0.6, '#d9c8a0');
    const line = mix(o.coat, '#000000', 0.38);
    for (const x of [-2.2, 0, 2.2]) limb(g, x, cy + 0.2, x * 0.8, cy - sh * 0.8, 0.3, line);
    limb(g, -3, cy - sh * 0.45, 3, cy - sh * 0.45, 0.3, line);
    if (!hidden) ell(g, 2.7, cy - 0.1, 0.45, 0.9, o.collar, 0.2);
  },
  frog(g, o) {
    const { pose, f } = o;
    const dk = mix(o.coat, '#000000', 0.25);
    const belly = mix(o.coat, '#f4f0b8', 0.6);
    const up = pose === 'sit';
    const leap = pose === 'hop';
    const cy = pose === 'lie' ? -1.5 : up ? -2.7 : leap ? -3 : -2.1;
    const rot = up ? -0.45 : leap ? 0.35 : 0;
    if (leap) {
      limb(g, -1.8, cy + 0.8, -5, cy + 2.2, 1.2, dk);
      ell(g, -5.5, cy + 2.4, 1.1, 0.4, dk);
      limb(g, 1.5, cy + 0.5, 3.8, cy - 0.6, 0.9, dk);
    } else {
      ell(g, -1.6, -1, 1.7, 1.3, dk);
      ell(g, -0.6, -0.3, 2, 0.4, dk);
      limb(g, 2.2, cy + 1, 2.6, -0.1, 0.9, dk);
      ell(g, 3.1, -0.2, 0.9, 0.35, dk);
    }
    g.save();
    g.translate(0, cy);
    g.rotate(rot);
    ball(g, 0, 0, 3, pose === 'lie' ? 1.4 : 2.1, o.coat);
    ell(g, 0.7, 0.9, 2, 0.9, belly);
    if (up) ell(g, 2.3, 0.7 + (f ? 0.2 : 0), 1.2 + (f ? 0.3 : 0), 1.2 + (f ? 0.3 : 0), belly);
    ell(g, -0.6, -1.2, 0.5, 0.35, dk);
    ell(g, 0.6, -1.4, 0.4, 0.3, dk);
    ball(g, 2.3, -0.9, 1.8, 1.4, o.coat);
    limb(g, 1.4, 0.3, 4, 0.1, 0.3, dk);
    for (const x of [1.5, 2.9]) {
      dot(g, x, -2.1, 0.85, o.coat);
      if (pose === 'lie') limb(g, x - 0.5, -2.1, x + 0.5, -2.1, 0.3, INK);
      else {
        dot(g, x + 0.1, -2.1, 0.62, '#fdfbe8');
        dot(g, x + 0.3, -2.1, 0.33, INK);
      }
    }
    ell(g, 1.4, 0.7, 0.55, 0.3, o.collar, 0.5);
    g.restore();
  },
  turkey(g, o) {
    const { pose, f } = o;
    const low = pose === 'sit' || pose === 'lie';
    const by = low ? -2.9 : -4.7;
    const leg = '#c4664e';
    const skin = '#8fb2cf';
    const red = '#d4402e';
    const tan = mix(o.coat, '#f2d9a0', 0.6);
    const dk = mix(o.coat, '#000000', 0.3);
    // The tail is a fan of feathers behind, tipped with tan. It spreads on a hop (fuss and dance) and
    // otherwise hangs folded.
    const feather = (a, len, wid, col) => {
      g.save();
      g.translate(-1.9, by - 0.4);
      g.rotate(a);
      ell(g, len * 0.55, 0, len * 0.55, wid, col);
      ell(g, len * 0.95, 0, len * 0.18, wid * 0.85, tan);
      g.restore();
    };
    if (pose === 'hop') {
      for (let i = 0; i < 9; i++) {
        const a = -3.05 + i * 0.3;
        feather(a, 6.2, 0.95, i % 2 ? mix(o.coat, '#ffffff', 0.1) : dk);
      }
    } else {
      for (const [a, len] of [[-2.75, 3.6], [3.05, 3.8], [2.7, 3.4]]) feather(a, low ? len - 0.6 : len, 0.85, dk);
    }
    if (!low) {
      for (const [x, ph] of [[-0.4, 0.5], [1, 0]]) {
        let fx = x;
        let fy = 0;
        if (pose === 'walk') {
          const a = (f / 4 + ph) * TAU;
          fx = x + Math.sin(a) * 1.1;
          fy = -Math.max(0, Math.cos(a)) * 0.8;
        }
        limb(g, x, by + 1.8, fx, fy - 0.1, 0.6, leg);
        limb(g, fx - 0.7, fy, fx + 1, fy, 0.5, leg);
        limb(g, fx - 0.2, fy, fx + 0.8, fy - 0.5, 0.35, leg);
      }
    }
    ball(g, 0, by, 3.2, 2.5, o.coat, 0.12);
    // Breast feathers in scallops, and a folded wing.
    ell(g, 1.2, by + 0.5, 1.6, 1.7, mix(o.coat, '#ffffff', 0.14));
    for (const [x, y] of [[0.8, 0.1], [1.7, 0.2], [1.2, 1], [2.1, 1.1]]) ell(g, x, by + y, 0.55, 0.3, tan);
    ell(g, -0.7, by + 0.1, 2, 1.35, mix(o.coat, '#000000', 0.2), 0.15);
    for (const y of [-0.5, 0.2, 0.9]) limb(g, -2.2, by + y, 0.7, by + y + 0.1, 0.3, tan);
    // The neck rises to a small bare head with a wattle.
    const hx = pose === 'lie' ? 2.6 : 3.3;
    const hy = pose === 'lie' ? by - 1.7 : low ? by - 3.4 : by - 4.1;
    limb(g, 2.1, by - 0.7, hx - 0.1, hy + 0.6, 1.2, mix(skin, '#000000', 0.12));
    limb(g, 2.6, by - 0.8, hx + 0.1, hy + 0.8, 0.5, o.coat);
    ell(g, 2.3, by - 0.6, 0.6, 0.4, o.collar, 0.5);
    ball(g, hx, hy, 1.15, 1.05, skin);
    poly(g, '#e8b040', [hx + 0.9, hy - 0.2, hx + 2.1, hy + 0.2, hx + 0.9, hy + 0.5]);
    ell(g, hx + 0.7, hy + 1.1, 0.4, 0.85, red);
    ell(g, hx + 1.1, hy - 0.5, 0.4, 0.3, red);
    eye(g, o, hx + 0.25, hy - 0.2, 0.3);
    // The beard.
    limb(g, 2.9, by + 0.5, 3, by + 1.5, 0.3, '#2a211d');
  },
};

// Every picture is drawn once at SCALE times its size, to a small canvas, and then stamped.

const SCALE = 6;
const W = 28;
const H = 26;
const OX = 14;
const OY = 21;
const FRAMES = { walk: 4, stand: 2, sit: 2, lie: 2, hop: 2 };
const cache = new Map();

export const POSES = Object.keys(FRAMES);

const isArt = (species) => typeof species === 'string' && Object.hasOwn(ARTS, species);

function colors(species, coat, collar) {
  return [hexOf(coat, Object.hasOwn(USUAL, species) ? USUAL[species] : '#b9814a'), hexOf(collar, USUAL_COLLAR)];
}

function sprite(species, coat, collar, pose, frame) {
  const key = `${species}|${coat}|${collar}|${pose}|${frame}`;
  let c = cache.get(key);
  if (c) return c;
  // Over the limit, the oldest quarter go, so a busy map re-draws a few pictures a frame and not all of
  // them at once.
  if (cache.size >= 900) {
    let drop = 225;
    for (const old of cache.keys()) {
      if (drop-- <= 0) break;
      cache.delete(old);
    }
  }
  c = document.createElement('canvas');
  c.width = W * SCALE;
  c.height = H * SCALE;
  const g = c.getContext('2d');
  g.scale(SCALE, SCALE);
  g.translate(OX, OY);
  try {
    ARTS[species](g, { pose, f: frame, coat, collar });
  } catch {
    // A figure that can't be drawn is left as it is.
  }
  cache.set(key, c);
  return c;
}

// A picture of a pet for catalog tiles: standing, in a square canvas of `size` pixels.
export function petPicture(species, coat, collar, size) {
  const n = Number.isFinite(size) && size >= 8 ? Math.min(512, Math.round(size)) : 48;
  const canvas = document.createElement('canvas');
  canvas.width = n;
  canvas.height = n;
  if (!isArt(species)) return canvas;
  const g = canvas.getContext('2d');
  const [c, k] = colors(species, coat, collar);
  const s = n / 24;
  g.save();
  g.translate(n / 2, n * 0.82);
  const gr = g.createRadialGradient(0, 0, 0, 0, 0, 5.5 * s);
  gr.addColorStop(0, 'rgba(0,0,0,.28)');
  gr.addColorStop(1, 'rgba(0,0,0,0)');
  g.save();
  g.scale(1, 0.4);
  g.fillStyle = gr;
  g.fillRect(-6 * s, -6 * s, 12 * s, 12 * s);
  g.restore();
  g.drawImage(sprite(species, c, k, 'stand', 0), -OX * s, -OY * s, W * s, H * s);
  g.restore();
  return canvas;
}

// The figures

function hashOf(text) {
  let h = 2166136261;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 16777619);
  return h >>> 0;
}

// A small seeded random, so a pet's moods are its own. It's on the figure and moves it on.
function rand(p) {
  p.rs = (p.rs + 0x6d2b79f5) | 0;
  let t = Math.imul(p.rs ^ (p.rs >>> 15), 1 | p.rs);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}

// How fast a species goes, against a walking dog.
const PACE = { turtle: 0.4, hedgehog: 0.7, sheep: 0.8, pig: 0.9, duck: 0.9, parrot: 0.9, turkey: 0.9, rabbit: 1.3, fox: 1.2, frog: 0.9 };
const HOPPERS = { rabbit: true, frog: true };

// Only a species this table lists has a pace of its own. A word like `constructor` is on every object,
// and reading it here would make a pet's speed not a number and then its place too (E4).
const paceOf = (species) => (Object.hasOwn(PACE, species) ? PACE[species] : 1);

const MARGIN = 1.8; // the yard's edge, where the fence stands
const KEEP = 1.2; // how far it keeps from a building
const KEEP_ITEM = 0.8;

// E14: whether something stands in the middle. A square has a fountain unless its style is `none`, and a
// park has nothing unless one is chosen.
function centred(kind, outside) {
  const style = partOf(outside, 'centerpiece').style;
  return (typeof style === 'string' ? style : kind === 'green' ? 'none' : 'fountain') !== 'none';
}

// The obstacles in a yard, in units: [u0, v0, u1, v1, keep].
function obstaclesOf(kind, outside) {
  const list = [];
  const rect = rectOf(kind, outside);
  if (rect) list.push([rect[0], rect[1], rect[2], rect[3], KEEP]);
  else if (!bodyOf(kind, outside) && centred(kind, outside)) list.push([CENTERPIECE.x * CELL, CENTERPIECE.y * CELL, (CENTERPIECE.x + CENTERPIECE.w) * CELL, (CENTERPIECE.y + CENTERPIECE.h) * CELL, KEEP]);
  for (const item of itemsOf(outside)) list.push([item.u0, item.v0, item.u1, item.v1, KEEP_ITEM]);
  return list;
}

function blocked(p, u, v) {
  if (u < MARGIN || v < MARGIN || u > 32 - MARGIN || v > 32 - MARGIN) return true;
  for (const o of p.obs) if (u > o[0] - o[4] && u < o[2] + o[4] && v > o[1] - o[4] && v < o[3] + o[4]) return true;
  return false;
}

// Whether a straight walk from one place to another stays in the clear.
function clear(p, u0, v0, u1, v1) {
  const n = Math.max(1, Math.ceil(Math.hypot(u1 - u0, v1 - v0) / 1.5));
  for (let i = 1; i <= n; i++) if (blocked(p, u0 + ((u1 - u0) * i) / n, v0 + ((v1 - v0) * i) / n)) return false;
  return true;
}

// The nearest free spot to a wish, looking outwards in rings.
function nearFree(p, u, v) {
  if (!blocked(p, u, v)) return [u, v];
  for (let r = 1.5; r < 20; r += 1.5) {
    for (let a = 0; a < 8; a++) {
      const x = u + Math.cos(a * 0.7854) * r;
      const y = v + Math.sin(a * 0.7854) * r;
      if (!blocked(p, x, y)) return [x, y];
    }
  }
  return [p.u, p.v];
}

// A free spot to walk to from here, or somewhere close to it.
function pickSpot(p) {
  for (let i = 0; i < 14; i++) {
    const u = MARGIN + rand(p) * (32 - MARGIN * 2);
    const v = MARGIN + rand(p) * (32 - MARGIN * 2);
    if (!blocked(p, u, v) && clear(p, p.u, p.v, u, v)) return [u, v];
  }
  return nearFree(p, p.u, p.v);
}

function target(p, spot) {
  p.tu = spot[0];
  p.tv = spot[1];
}

// Where it stands to guard the door, facing out, and the way it faces.
function doorSpot(p) {
  const rect = rectOf(p.kind, p.outside);
  if (!rect) return [nearFree(p, 23, 23), 1];
  const [u, v] = p.door || [rect[2], rect[3]];
  return p.doorSide === 'R' ? [nearFree(p, u + 2.8, v), 1] : [nearFree(p, u, v + 2.8), -1];
}

// A nice place to stay, that isn't the door and isn't in the way.
function restSpot(p, back) {
  const rect = rectOf(p.kind, p.outside) || [12, 12, 20, 20];
  if (back) return nearFree(p, Math.max(MARGIN + 1, rect[0] - 3), Math.max(MARGIN + 1, rect[1] + 2));
  return nearFree(p, rect[2] + 2.5 + (p.seed % 3), rect[1] + 3 + ((p.seed >> 3) % 4));
}

// The street is at the front, past u = 32 and v = 32.
function frontSpot(p) {
  for (let i = 0; i < 12; i++) {
    const e = rand(p);
    const spot = nearFree(p, e < 0.5 ? 30 : 6 + rand(p) * 22, e < 0.5 ? 6 + rand(p) * 22 : 30);
    if (clear(p, p.u, p.v, spot[0], spot[1])) return spot;
  }
  return pickSpot(p);
}

// Moves toward the target. 1 when it gets there, -1 when something's in the way, else 0.
function go(p, dt, rate) {
  const du = p.tu - p.u;
  const dv = p.tv - p.v;
  const d = Math.hypot(du, dv);
  const speed = 5 * rate * paceOf(p.species);
  const stride = speed * dt;
  if (d <= stride + 0.05) {
    p.u = p.tu;
    p.v = p.tv;
    p.moving = false;
    return 1;
  }
  const nu = p.u + (du / d) * stride;
  const nv = p.v + (dv / d) * stride;
  if (blocked(p, nu, nv)) {
    p.moving = false;
    return -1;
  }
  p.u = nu;
  p.v = nv;
  p.moving = true;
  p.pose = 'walk';
  p.rate = rate;
  if (Math.abs(du - dv) > 0.04) p.face = du - dv > 0 ? 1 : -1;
  return 0;
}

function settle(p, pose) {
  p.moving = false;
  p.pose = pose;
}

// Each action is called once a frame and keeps its place in `p.s`. A pet starts an action from the top.
const ACTS = {
  wander(p, dt) {
    if (p.s === 0) {
      target(p, pickSpot(p));
      p.s = 1;
    } else if (p.s === 1) {
      const r = go(p, dt, 1);
      if (r === 1) {
        p.s = 2;
        p.wait = 1 + rand(p) * 2.5;
        settle(p, rand(p) < 0.3 ? 'sit' : 'stand');
      } else if (r < 0) p.s = 0;
    } else if ((p.wait -= dt) <= 0) p.s = 0;
  },
  patrol(p, dt) {
    // Round the inside of the fence, in one direction, a few units at a time.
    if (p.s === 0) {
      const a = 2.5;
      const b = 29.5;
      const len = b - a;
      p.ring = ((p.ring ?? 0) + 5) % (len * 4);
      const s = p.ring;
      const side = Math.floor(s / len);
      const k = s - side * len;
      const pts = [[a + k, a], [b, a + k], [b - k, b], [a, b - k]];
      const spot = nearFree(p, pts[side][0], pts[side][1]);
      target(p, p.dir < 0 ? [spot[1], spot[0]] : spot);
      p.s = 1;
    } else {
      const r = go(p, dt, 1);
      if (r === 1) p.s = 0;
      else if (r < 0) {
        p.ring = (p.ring ?? 0) + 6;
        p.s = 0;
      }
    }
  },
  guard(p, dt) {
    if (p.s === 0) {
      const [spot, face] = doorSpot(p);
      target(p, spot);
      p.faceTo = face;
      p.s = 1;
    } else if (p.s === 1) {
      const r = go(p, dt, 1);
      if (r !== 0) {
        p.s = 2;
        if (r < 0) p.s = 0;
        p.face = p.faceTo;
        settle(p, 'stand');
      }
    } else {
      p.face = p.faceTo;
      settle(p, 'stand');
    }
  },
  sit(p, dt) {
    ACTS.stay(p, dt, 'sit', false);
  },
  sleep(p, dt) {
    ACTS.stay(p, dt, 'lie', true);
    p.fx = p.s === 2 ? 'z' : '';
  },
  stay(p, dt, pose, back) {
    if (p.s === 0) {
      target(p, restSpot(p, back));
      p.s = 1;
    } else if (p.s === 1) {
      const r = go(p, dt, 1);
      if (r !== 0) {
        p.s = r < 0 ? 0 : 2;
        if (r < 0) {
          target(p, pickSpot(p));
          p.s = 1;
        }
      }
    } else settle(p, pose);
  },
  play(p, dt) {
    if (p.s === 0) {
      // A middle where a ring of four units is free all the way round.
      for (let i = 0; i < 16; i++) {
        const cu = 6 + rand(p) * 20;
        const cv = 6 + rand(p) * 20;
        let ok = true;
        for (let a = 0; a < 8 && ok; a++) ok = !blocked(p, cu + Math.cos(a * 0.7854) * 4.2, cv + Math.sin(a * 0.7854) * 4.2);
        if (ok) {
          p.cu = cu;
          p.cv = cv;
          p.ang = 0;
          p.wait = 8 + rand(p) * 6;
          target(p, [cu + 4, cv]);
          p.s = 1;
          return;
        }
      }
      target(p, pickSpot(p));
      p.s = 3;
    } else if (p.s === 1) {
      const r = go(p, dt, 1.5);
      if (r === 1) p.s = 2;
      else if (r < 0) p.s = 0;
    } else if (p.s === 2) {
      p.ang += (dt * 7 * paceOf(p.species)) / 4;
      const nu = p.cu + Math.cos(p.ang) * 4;
      const nv = p.cv + Math.sin(p.ang) * 4;
      if ((p.wait -= dt) <= 0 || blocked(p, nu, nv)) p.s = 0;
      else {
        const dx = nu - p.u - (nv - p.v);
        if (Math.abs(dx) > 0.01) p.face = dx > 0 ? 1 : -1;
        p.u = nu;
        p.v = nv;
        p.moving = true;
        p.pose = 'hop';
        p.rate = 1.8;
        p.z = Math.abs(Math.sin(p.t * 8)) * 2.2;
      }
    } else if (go(p, dt, 1.5) !== 0) p.s = 0;
  },
  dig(p, dt) {
    if (p.s === 0) {
      target(p, pickSpot(p));
      p.s = 1;
    } else if (p.s === 1) {
      const r = go(p, dt, 1);
      if (r === 1) {
        p.s = 2;
        p.wait = 3 + rand(p) * 3;
      } else if (r < 0) p.s = 0;
    } else {
      settle(p, 'stand');
      p.fx = 'dig';
      p.tilt = 0.32 + Math.sin(p.t * 14) * 0.05;
      if ((p.wait -= dt) <= 0) p.s = 0;
    }
  },
  chase(p, dt) {
    if (p.s === 0) {
      p.hu = p.u;
      p.hv = p.v;
      target(p, frontSpot(p));
      p.s = 1;
    } else if (p.s === 1) {
      const r = go(p, dt, 2.2);
      if (r < 0) p.s = 0;
      else if (r !== 0) {
        p.s = 2;
        p.wait = 0.4;
        settle(p, 'stand');
      }
    } else if (p.s === 2) {
      settle(p, 'stand');
      if ((p.wait -= dt) <= 0) {
        target(p, nearFree(p, p.hu, p.hv));
        p.s = 3;
      }
    } else if (p.s === 3) {
      const r = go(p, dt, 2.2);
      if (r !== 0) {
        if (r < 0) target(p, pickSpot(p));
        p.s = 4;
        p.wait = 0.8;
      }
    } else {
      settle(p, 'stand');
      if ((p.wait -= dt) <= 0) p.s = 0;
    }
  },
  greet(p, dt) {
    if (p.s === 0) {
      target(p, nearFree(p, 28, 28));
      p.s = 1;
    } else if (p.s === 1) {
      if (go(p, dt, 1.3) !== 0) {
        p.s = 2;
        p.wait = 4;
      }
    } else if (p.s === 2) {
      p.face = 1;
      p.pose = 'hop';
      p.moving = false;
      p.z = Math.abs(Math.sin(p.t * 6)) * 2.6;
      p.fx = 'heart';
      if ((p.wait -= dt) <= 0) {
        p.s = 3;
        p.wait = 1.2;
      }
    } else {
      settle(p, 'stand');
      if ((p.wait -= dt) <= 0) {
        p.s = 2;
        p.wait = 4;
      }
    }
  },
  fuss(p, dt) {
    settle(p, 'hop');
    p.z = Math.abs(Math.sin(p.t * 9)) * 2.6;
    p.fx = 'bang';
    if (Math.sin(p.t * 3.1) > 0.6) p.face = -p.face;
  },
  dance(p, dt) {
    settle(p, Math.floor(p.t * 4) & 1 ? 'hop' : 'stand');
    p.z = Math.abs(Math.sin(p.t * 8)) * 1.6;
    p.face = Math.floor(p.t / 0.22) & 1 ? 1 : -1;
    p.tilt = Math.sin(p.t * 8) * 0.22;
  },
  hide(p, dt) {
    if (p.s === 0) {
      // Behind the building: the back of it, where its own wall hides the pet. The way round is by one
      // of the two far corners, and if neither is clear it stays where it can.
      const rect = rectOf(p.kind, p.outside) || [12, 12, 20, 20];
      let spot = null;
      for (let i = 0; i < 20 && !spot; i++) {
        const u = MARGIN + rand(p) * Math.max(0.1, rect[0] - KEEP - MARGIN);
        const v = MARGIN + rand(p) * Math.max(0.1, rect[1] + (rect[3] - rect[1]) * 0.7 - MARGIN);
        if (!blocked(p, u, v)) spot = [u, v];
      }
      spot = spot || nearFree(p, MARGIN + 0.5, MARGIN + 0.5);
      const ways = [[[2.5, 29.5]], [[29.5, 2.5]], [[2.5, 29.5], [2.5, 2.5]], [[29.5, 29.5], [2.5, 29.5]], [[29.5, 29.5], [29.5, 2.5]], [[29.5, 29.5], [2.5, 29.5], [2.5, 2.5]], []];
      p.route = [];
      for (const way of ways) {
        const stops = way.map((w) => nearFree(p, w[0], w[1])).concat([spot]);
        let from = [p.u, p.v];
        let ok = true;
        for (const stop of stops) {
          ok = ok && clear(p, from[0], from[1], stop[0], stop[1]);
          from = stop;
        }
        if (ok) {
          p.route = stops;
          break;
        }
      }
      if (!p.route.length) p.route = [spot];
      p.s = 1;
    } else if (p.s === 1) {
      target(p, p.route[0]);
      const r = go(p, dt, 1);
      if (r !== 0) {
        if (r > 0 && p.route.length > 1) p.route.shift();
        else p.s = r > 0 ? 2 : 3;
      }
    } else if (p.s === 3) {
      // Stuck: it stays put, and tries again in a while.
      settle(p, 'sit');
      p.wait = (p.wait || 0) + dt;
      if (p.wait > 4) {
        p.wait = 0;
        p.s = 0;
      }
    } else settle(p, 'sit');
  },
};

const ACTION = { wander: 1, patrol: 1, guard: 1, sit: 1, sleep: 1, play: 1, dig: 1, chase: 1, greet: 1, fuss: 1, dance: 1, hide: 1 };

// E19: a figure for one pet. `x`, `y` is its tile, and `kind` the place's kind.
// `door` is `{ side, at }`, where the place's door meets the ground, for a pet on guard.
export function makePet(id, x, y, pet, outside, key, kind, door = null) {
  const doc = pet && typeof pet === 'object' ? pet : {};
  const seed = hashOf(String(id));
  const rules = (Array.isArray(doc.rules) ? doc.rules : []).filter((r) => r && typeof r === 'object').slice(0, 6);
  const p = {
    type: 'pet',
    id,
    key,
    tx: x,
    ty: y,
    px: x + 0.5,
    py: y + 0.5,
    name: typeof doc.name === 'string' ? doc.name : '',
    door: Array.isArray(door?.at) ? door.at : null,
    doorSide: door?.side || 'L',
    species: doc.species,
    coat: doc.coat,
    collar: doc.collar,
    rules,
    kind,
    // E36: how many tiles the yard is spread over, each way.
    span: spanOf(kind),
    outside,
    obs: obstaclesOf(kind, outside),
    seed,
    rs: seed,
    u: 16,
    v: 16,
    tu: 16,
    tv: 16,
    face: seed & 1 ? 1 : -1,
    dir: seed & 2 ? 1 : -1,
    pose: 'stand',
    moving: false,
    rate: 1,
    t: 0,
    clock: 0,
    off: (seed % 12000) / 100,
    pulse: 0,
    check: 0,
    act: '',
    cand: '',
    candN: 0,
    s: 0,
    wait: 0,
    z: 0,
    tilt: 0,
    fx: '',
    behind: false,
    pause: 0,
  };
  const start = pickSpot(p);
  p.u = start[0];
  p.v = start[1];
  place(p);
  return p;
}

function place(p) {
  p.px = p.tx + (p.u * p.span) / 32;
  p.py = p.ty + (p.v * p.span) / 32;
  const rect = rectOf(p.kind, p.outside);
  p.behind = Boolean(rect) && (p.u < rect[0] || p.v < rect[1]);
}

// E19, E20, E21: once a frame while it's on screen. `facts` is only what anyone can see on this map. It can
// be a function that gives them, which is only called when the pet is about to look at its rules, a few
// times a second, so that finding who's near doesn't cost every pet every frame.
export function stepPet(p, dt, facts) {
  if (!(dt > 0)) return;
  dt = Math.min(dt, 0.1);
  p.t += dt;
  p.pulse = p.t + p.off;
  p.check -= dt;
  if (p.check <= 0) {
    // A few times a second, and a change has to hold a moment, so someone on the edge of the range
    // doesn't make it flicker.
    p.check = 0.25;
    const rule = chooseRule(p.rules, typeof facts === 'function' ? (p.rules.length ? facts() : null) : facts, p.pulse);
    const act = rule && typeof rule.do === 'string' && Object.hasOwn(ACTION, rule.do) && Object.hasOwn(ACTS, rule.do) ? rule.do : 'wander';
    if (!p.act) begin(p, act, 0);
    else if (act === p.act) p.candN = 0;
    else if (act !== p.cand) {
      p.cand = act;
      p.candN = 1;
    } else if (++p.candN >= 3) begin(p, act, 0.4);
  }
  if (p.pause > 0) {
    p.pause -= dt;
    settle(p, 'stand');
  } else {
    p.fx = '';
    p.z = 0;
    p.tilt = 0;
    ACTS[p.act](p, dt);
  }
  if (p.moving) p.clock += dt * 6 * p.rate * paceOf(p.species);
  place(p);
}

function begin(p, act, pause) {
  p.act = act;
  p.cand = '';
  p.candN = 0;
  p.s = 0;
  p.pause = pause;
  p.fx = '';
  p.moving = false;
}

// Drawing

// E21: with `calm` it sits, still.
export function drawPet(bg, p, t, calm) {
  if (!isArt(p.species)) return null;
  const [coat, collar] = colors(p.species, p.coat, p.collar);
  const X = p.dx;
  const Y = p.dy;
  let pose = calm ? 'sit' : p.pose;
  let z = calm ? 0 : p.z || 0;
  const tilt = calm ? 0 : p.tilt || 0;
  let frame = 0;
  if (!calm) {
    if (pose === 'walk') {
      if (Object.hasOwn(HOPPERS, p.species)) {
        // A hopper goes in hops, and crouches between.
        const ph = (p.clock / 6) % 1;
        pose = ph < 0.55 ? 'hop' : 'stand';
        z = pose === 'hop' ? Math.sin((ph / 0.55) * 3.1416) * 3 : 0;
      } else frame = Math.floor(p.clock) & 3;
    } else frame = Math.floor(t * 2.6 + p.seed) & 1;
  }
  const face = p.face < 0 ? -1 : 1;
  // A soft small shadow, smaller when it's in the air.
  const sw = 4.2 - z * 0.25;
  const gr = bg.createRadialGradient(X, Y, 0, X, Y, sw);
  gr.addColorStop(0, 'rgba(20,30,10,.34)');
  gr.addColorStop(1, 'rgba(20,30,10,0)');
  bg.save();
  bg.translate(X, Y);
  bg.scale(1, 0.4);
  bg.translate(-X, -Y);
  bg.fillStyle = gr;
  bg.fillRect(X - sw, Y - sw, sw * 2, sw * 2);
  bg.restore();
  bg.save();
  bg.translate(X, Y - z);
  if (tilt) bg.rotate(tilt * face);
  bg.scale(face, 1);
  bg.drawImage(sprite(p.species, coat, collar, pose, frame), -OX, -OY, W, H);
  bg.restore();
  if (!calm && p.fx) effect(bg, p.fx, X, Y - z, t, face);
  return [X - 14, Y - z - 30, X + 14, Y + 4];
}

function effect(g, fx, X, Y, t, face) {
  if (fx === 'z') {
    // Three small z shapes, rising and fading in turn.
    for (let i = 0; i < 3; i++) {
      const k = (t * 0.5 + i / 3) % 1;
      const s = 1 + k * 1.4;
      const x = X + face * (3 + k * 2.5);
      const y = Y - 7 - k * 8;
      g.globalAlpha = Math.min(1, (1 - k) * 1.6);
      g.beginPath();
      g.moveTo(x - s, y - s);
      g.lineTo(x + s, y - s);
      g.lineTo(x - s, y + s);
      g.lineTo(x + s, y + s);
      g.lineWidth = 0.7;
      g.lineCap = 'round';
      g.lineJoin = 'round';
      g.strokeStyle = '#ffffff';
      g.stroke();
    }
    g.globalAlpha = 1;
  } else if (fx === 'heart') {
    const k = (t * 1.2) % 1;
    const x = X;
    const y = Y - 12 - k * 5;
    g.globalAlpha = Math.min(1, (1 - k) * 2);
    g.fillStyle = '#ee4466';
    g.beginPath();
    g.arc(x - 0.9, y - 0.5, 1, 3.1416, 6.2832);
    g.arc(x + 0.9, y - 0.5, 1, 3.1416, 6.2832);
    g.lineTo(x, y + 2);
    g.closePath();
    g.fill();
    g.globalAlpha = 1;
  } else if (fx === 'bang') {
    const wob = Math.sin(t * 18) * 0.5;
    g.fillStyle = '#ffcf33';
    for (const dx of [-3, 3]) {
      const x = X + dx;
      const y = Y - 15 + wob * (dx < 0 ? 1 : -1);
      g.beginPath();
      g.moveTo(x - 0.8, y - 4);
      g.lineTo(x + 0.8, y - 4);
      g.lineTo(x + 0.4, y);
      g.lineTo(x - 0.4, y);
      g.closePath();
      g.fill();
      g.beginPath();
      g.arc(x, y + 1.6, 0.7, 0, 6.2832);
      g.fill();
    }
  } else if (fx === 'dig') {
    // Flecks of earth flying up from the front paws.
    for (let i = 0; i < 6; i++) {
      const k = (t * 3 + i * 0.17) % 1;
      const x = X + face * (4.5 - k * 3 + (i % 3) * 0.7);
      const y = Y - 1 - Math.sin(k * 3.1416) * (3 + (i % 2));
      g.globalAlpha = 1 - k * 0.5;
      g.fillStyle = i % 2 ? '#7a5a3a' : '#a07a52';
      g.beginPath();
      g.arc(x, y, 0.55, 0, 6.2832);
      g.fill();
    }
    g.globalAlpha = 1;
  }
}
