// What people put in a yard besides plants (docs/outside.md). Every kind is drawn in its own little
// frame, facing 0: `a` runs along the footprint's width in units, `b` along its depth, `z` up. The kit
// below turns that to whichever way the item faces (E15), so nothing here cares about the turn.
//
// This module imports from world.js, which imports it back, so nothing here may call an imported
// function while the module loads. Everything runs inside the entries.

import { CELL, YARD_SIZES, hexOf } from './outside-spec.js';
import { P, dark, ell, fp, grad, lite, line, mixHex, rgba, rr, shadow } from './world.js';

const TAU = 6.2832;
const INK = 'rgba(28,22,48,.22)';

// A colour lit (positive) or shaded (negative).
const tint = (col, n) => (n >= 0 ? lite(col, n) : dark(col, -n));

// Light comes from the front left: tops are lit most, the left face is the base, the right face is shaded.
function toneOf(col, nu, nv, nz) {
  return tint(col, nz * 0.26 + nv * 0.03 - nu * 0.24);
}

// A ball with its light up and to the left.
function ball(g, x, y, r, col, rx = r, ry = r) {
  const gr = g.createRadialGradient(x - rx * 0.35, y - ry * 0.4, Math.min(rx, ry) * 0.1, x, y, Math.max(rx, ry));
  gr.addColorStop(0, lite(col, 0.45));
  gr.addColorStop(0.55, col);
  gr.addColorStop(1, dark(col, 0.35));
  g.beginPath();
  g.ellipse(x, y, rx, ry, 0, 0, TAU);
  g.fillStyle = gr;
  g.fill();
}

// Light spilling into the air round a lamp (E29), for the night pass.
function glow(g, x, y, r, color, a = 0.55) {
  const gr = g.createRadialGradient(x, y, 0, x, y, r);
  gr.addColorStop(0, rgba(color, a));
  gr.addColorStop(0.35, rgba(color, a * 0.4));
  gr.addColorStop(1, rgba(color, 0));
  g.fillStyle = gr;
  g.beginPath();
  g.arc(x, y, r, 0, TAU);
  g.fill();
}

// And the same light where it lands on the ground, which is squashed like the ground is.
function pool(g, x, y, r, color, a = 0.4) {
  g.save();
  g.translate(x, y);
  g.scale(1, 0.5);
  glow(g, 0, 0, r, color, a);
  g.restore();
}

function flame(g, x, y, h, w) {
  g.beginPath();
  g.moveTo(x - w, y);
  g.bezierCurveTo(x - w * 1.1, y - h * 0.45, x - w * 0.3, y - h * 0.55, x + w * 0.1, y - h);
  g.bezierCurveTo(x + w * 0.5, y - h * 0.55, x + w * 1.2, y - h * 0.4, x + w, y);
  g.closePath();
  g.fillStyle = '#ff8a2a';
  g.fill();
  g.beginPath();
  g.moveTo(x - w * 0.55, y);
  g.bezierCurveTo(x - w * 0.6, y - h * 0.3, x - w * 0.1, y - h * 0.4, x + w * 0.05, y - h * 0.72);
  g.bezierCurveTo(x + w * 0.3, y - h * 0.4, x + w * 0.65, y - h * 0.3, x + w * 0.55, y);
  g.closePath();
  g.fillStyle = '#ffd75a';
  g.fill();
}

function strokeArc(g, col, w, x, y, rx, ry, from, to) {
  g.strokeStyle = col;
  g.lineWidth = w;
  g.beginPath();
  g.ellipse(x, y, rx, ry, 0, from, to);
  g.stroke();
}

// The frame an item draws in. `at` gives a screen point, `box` a lit block, `q` a flat face that is
// only painted when it faces the viewer, and `parts` paints several things back to front.
function kit(S, item) {
  const g = S.g;
  const [cw, ch] = YARD_SIZES[item.kind] || [1, 1];
  const A = cw * CELL;
  const B = ch * CELL;
  const f = item.facing & 3;
  const { u0, v0 } = item;
  const map = (a, b) => {
    if (f === 0) return [u0 + a, v0 + b];
    if (f === 1) return [u0 + b, v0 + A - a];
    if (f === 2) return [u0 + A - a, v0 + B - b];
    return [u0 + B - b, v0 + a];
  };
  const dir = (da, db) => {
    const o = map(0, 0);
    const e = map(da, db);
    return [e[0] - o[0], e[1] - o[1]];
  };
  const at = (a, b, z = 0) => {
    const [u, v] = map(a, b);
    return P(u, v, z);
  };
  const K = { S, g, A, B, at, dir, mirror: f >= 2 };

  K.rect = (pad = 0) => [item.u0 + pad, item.v0 + pad, item.u1 - pad, item.v1 - pad];
  K.depth = (a, b) => {
    const [u, v] = map(a, b);
    return u + v;
  };

  // A soft patch of shade under a thing, so it sits on the grass.
  K.contact = (k = 0.7) => {
    const [x, y] = at(A / 2, B / 2, 0);
    ell(g, 'rgba(30,24,50,.2)', x + 0.6, y + 0.4, (A + B) * 0.5 * k * 0.9, (A + B) * 0.25 * k * 0.9);
  };

  // A flat face given as local points [a, b, z], painted only when it turns towards the viewer.
  K.q = (col, n, ...pts) => {
    const [nu, nv] = dir(n[0], n[1]);
    const nz = n[2] || 0;
    if (nu + nv + nz <= 0.001) return;
    fp(g, toneOf(col, nu, nv, nz), pts.map((p) => at(p[0], p[1], p[2] || 0)));
    g.strokeStyle = INK;
    g.lineWidth = 0.18;
    g.stroke();
  };
  K.flat = (col, ...pts) => fp(g, col, pts.map((p) => at(p[0], p[1], p[2] || 0)));

  K.box = (a0, b0, a1, b1, z0, z1, col) => {
    K.q(col, [0, 1, 0], [a0, b1, z0], [a1, b1, z0], [a1, b1, z1], [a0, b1, z1]);
    K.q(col, [0, -1, 0], [a0, b0, z0], [a1, b0, z0], [a1, b0, z1], [a0, b0, z1]);
    K.q(col, [1, 0, 0], [a1, b0, z0], [a1, b1, z0], [a1, b1, z1], [a1, b0, z1]);
    K.q(col, [-1, 0, 0], [a0, b0, z0], [a0, b1, z0], [a0, b1, z1], [a0, b0, z1]);
    K.q(col, [0, 0, 1], [a0, b0, z1], [a1, b0, z1], [a1, b1, z1], [a0, b1, z1]);
  };

  // An upright cylinder, narrowing from r0 at the bottom to r1 at the top.
  K.cyl = (a, b, r0, z0, z1, col, r1 = r0) => {
    const [x, y] = at(a, b, 0);
    const rx0 = r0 * 1.414;
    const ry0 = r0 * 0.707;
    const rx1 = r1 * 1.414;
    const ry1 = r1 * 0.707;
    g.beginPath();
    g.moveTo(x + rx1, y - z1);
    g.lineTo(x + rx0, y - z0);
    g.ellipse(x, y - z0, rx0, ry0, 0, 0, Math.PI);
    g.lineTo(x - rx1, y - z1);
    g.closePath();
    g.fillStyle = grad(g, x - rx0, 0, x + rx0, 0, lite(col, 0.3), col, dark(col, 0.32));
    g.fill();
    ell(g, lite(col, 0.22), x, y - z1, rx1, ry1);
  };

  // A thin upright post.
  K.rod = (a, b, z0, z1, w, col) => {
    const [x, y] = at(a, b, 0);
    g.fillStyle = grad(g, x - w / 2, 0, x + w / 2, 0, lite(col, 0.25), col, dark(col, 0.3));
    g.fillRect(x - w / 2, y - z1, w, z1 - z0);
  };

  K.ball = (a, b, z, r, col) => {
    const [x, y] = at(a, b, z);
    ball(g, x, y, r, col);
  };

  K.disc = (a, b, r, col, z = 0) => {
    const [x, y] = at(a, b, z);
    ell(g, col, x, y, r * 1.414, r * 0.707);
  };

  K.seg = (col, w, p, q) => {
    const [x0, y0] = at(p[0], p[1], p[2] || 0);
    const [x1, y1] = at(q[0], q[1], q[2] || 0);
    line(g, col, w, x0, y0, x1, y1);
  };

  // A circle standing in the plane of the local axis `axis` ('a' or 'b') and z. `nrm` is which way it
  // faces along the other axis. With a width `w` it is a ring and is seen from both sides.
  K.circ = (col, a, b, z, r, axis, nrm, w = 0) => {
    if (!w) {
      const [nu, nv] = axis === 'a' ? dir(0, nrm) : dir(nrm, 0);
      if (nu + nv <= 0.001) return;
    }
    const pts = [];
    for (let i = 0; i < 16; i++) {
      const t = (i / 16) * TAU;
      const h = Math.cos(t) * r;
      pts.push(at(axis === 'a' ? a + h : a, axis === 'b' ? b + h : b, z + Math.sin(t) * r));
    }
    if (w) {
      g.strokeStyle = col;
      g.lineWidth = w;
      g.beginPath();
      g.moveTo(pts[0][0], pts[0][1]);
      for (let i = 1; i < pts.length; i++) g.lineTo(pts[i][0], pts[i][1]);
      g.closePath();
      g.stroke();
    } else fp(g, col, pts);
  };

  // A lying log or bale: a thick line with its near end showing as a ring.
  K.log = (a0, b0, a1, b1, z, r, col, rings = true) => {
    const p = at(a0, b0, z + r);
    const q = at(a1, b1, z + r);
    line(g, dark(col, 0.22), r * 2.1, p[0], p[1], q[0], q[1]);
    line(g, col, r * 1.7, p[0], p[1] - r * 0.15, q[0], q[1] - r * 0.15);
    line(g, lite(col, 0.28), r * 0.6, p[0], p[1] - r * 0.55, q[0], q[1] - r * 0.55);
    const [du, dv] = dir(a1 - a0, b1 - b0);
    const near = du + dv > 0 ? q : p;
    g.save();
    g.transform(1, Math.abs(du) > Math.abs(dv) ? -0.5 : 0.5, 0, 1, near[0], near[1]);
    ell(g, dark(col, 0.2), 0, 0, r * 1.02, r * 1.02);
    ell(g, lite(col, 0.35), 0, 0, r * 0.86, r * 0.86);
    if (rings) {
      strokeArc(g, dark(col, 0.25), 0.25, 0, 0, r * 0.55, r * 0.55, 0, TAU);
      strokeArc(g, dark(col, 0.25), 0.25, 0, 0, r * 0.25, r * 0.25, 0, TAU);
    }
    g.restore();
  };

  // Several things in a frame, back to front. A higher layer always goes over a lower one.
  K.parts = (list) => {
    const sorted = list.map((entry, i) => ({ a: entry[0], b: entry[1], fn: entry[2], layer: entry[3] || 0, i }));
    sorted.sort((p, q) => p.layer - q.layer || K.depth(p.a, p.b) - K.depth(q.a, q.b) || p.i - q.i);
    for (const part of sorted) part.fn();
  };

  // A standing figure drawn in screen units from the point on the ground at (a, b). Facings 2 and 3 turn it round.
  K.figure = (a, b, fn, k = 1) => {
    const [x, y] = at(a, b, 0);
    g.save();
    g.translate(x, y);
    g.scale(K.mirror ? -k : k, k);
    fn();
    g.restore();
  };
  return K;
}

// A rough lump of stone with a lit top left and a dark lower right.
function stone(g, x, y, s, col, seed) {
  ell(g, 'rgba(30,24,50,.25)', x + s * 0.4, y + s * 0.1, s * 1.5, s * 0.6);
  const j = (k) => 0.88 + (((seed * 37 + k * 53) % 25) / 25) * 0.24;
  const pts = [
    [x - s * 1.25 * j(1), y],
    [x - s * 1.05 * j(2), y - s * 0.75 * j(3)],
    [x - s * 0.35 * j(4), y - s * 1.3 * j(5)],
    [x + s * 0.45 * j(6), y - s * 1.2 * j(7)],
    [x + s * 1.1 * j(8), y - s * 0.6 * j(9)],
    [x + s * 1.3 * j(10), y],
    [x + s * 0.3, y + s * 0.3],
    [x - s * 0.7, y + s * 0.25],
  ];
  fp(g, grad(g, x - s, y - s, x + s, y + s * 0.4, lite(col, 0.18), col, dark(col, 0.28)), pts);
  fp(g, rgba(lite(col, 0.5), 0.5), [pts[1], pts[2], pts[3], [x + s * 0.1, y - s * 0.7], [x - s * 0.6, y - s * 0.45]]);
  fp(g, rgba(dark(col, 0.5), 0.35), [[x + s * 0.2, y - s * 0.4], pts[4], pts[5], pts[6], [x + s * 0.1, y]]);
  g.strokeStyle = INK;
  g.lineWidth = 0.3;
  g.beginPath();
  g.moveTo(pts[0][0], pts[0][1]);
  for (let i = 1; i < pts.length; i++) g.lineTo(pts[i][0], pts[i][1]);
  g.closePath();
  g.stroke();
}

// A wrapped present: a box with a ribbon over its top and two faces, and a bow.
function gift(K, a0, b0, a1, b1, z0, z1, col, ribbon) {
  K.box(a0, b0, a1, b1, z0, z1, col);
  const am = (a0 + a1) / 2;
  const bm = (b0 + b1) / 2;
  const w = (a1 - a0) * 0.16;
  K.q(ribbon, [0, 1, 0], [am - w, b1, z0], [am + w, b1, z0], [am + w, b1, z1], [am - w, b1, z1]);
  K.q(ribbon, [1, 0, 0], [a1, bm - w, z0], [a1, bm + w, z0], [a1, bm + w, z1], [a1, bm - w, z1]);
  K.q(ribbon, [0, 0, 1], [am - w, b0, z1], [am + w, b0, z1], [am + w, b1, z1], [am - w, b1, z1]);
  K.q(ribbon, [0, 0, 1], [a0, bm - w, z1], [a1, bm - w, z1], [a1, bm + w, z1], [a0, bm + w, z1]);
  const [x, y] = K.at(am, bm, z1);
  ell(K.g, lite(ribbon, 0.2), x - 0.5, y - 0.4, 0.55, 0.35);
  ell(K.g, lite(ribbon, 0.2), x + 0.5, y - 0.4, 0.55, 0.35);
  ell(K.g, dark(ribbon, 0.15), x, y - 0.3, 0.28, 0.25);
}

// The Fall pack's colours, the same toasted russets and walnut as the room's.
const FALL_WOOD = ['#3f2a1c', '#6b4628', '#8a5c37', '#a0703f'];
const FALL_STRAW = ['#a8843f', '#c9a86a', '#e3c987'];
const FALL_LEAVES = ['#b33a1f', '#e8802a', '#d9a520', '#7f8c3a', '#d4502a', '#f0a03c', '#8c2a1e', '#c8641e'];
const FALL_CREAM = '#f4f1e8';

// One leaf lying or hung at a slant, with a rib down it.
function leaf(g, x, y, rx, ry, col, rot) {
  g.save();
  g.translate(x, y);
  g.rotate(rot);
  g.beginPath();
  g.ellipse(0, 0, rx, ry, 0, 0, TAU);
  g.fillStyle = col;
  g.fill();
  line(g, dark(col, 0.3), 0.15, -rx * 0.8, 0, rx * 0.8, 0);
  g.restore();
}

// A pumpkin or gourd sitting on the ground at (x, y), with ribs and a stem.
function squash(g, x, y, rx, ry, col, stem = '#4f6a2a') {
  ell(g, 'rgba(30,24,50,.22)', x + rx * 0.2, y, rx * 1.05, ry * 0.4);
  ball(g, x, y - ry * 0.9, rx, col, rx, ry * 0.9);
  for (const k of [-0.5, 0.5]) {
    g.strokeStyle = rgba(dark(col, 0.45), 0.55);
    g.lineWidth = 0.18;
    g.beginPath();
    g.moveTo(x + rx * k * 0.3, y - ry * 1.75);
    g.quadraticCurveTo(x + rx * k * 1.15, y - ry * 0.9, x + rx * k * 0.3, y - ry * 0.05);
    g.stroke();
  }
  line(g, stem, Math.max(0.35, rx * 0.2), x, y - ry * 1.75, x + rx * 0.15, y - ry * 2.15);
}

// An apple, with its stalk.
function apple(g, x, y, r, col) {
  ball(g, x, y, r, col);
  line(g, '#4a3220', 0.2, x, y - r * 0.8, x + 0.1, y - r * 1.3);
}

export const DECOR_ART = {
  // --- Seating and shade ---------------------------------------------------------------------

  bench(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const wood = hexOf(item.color, '#a9743f');
    const iron = '#3a3d4a';
    shadow(S, K.rect(), 6);
    K.contact(0.6);
    K.parts([
      [0.5, 0.55, () => K.box(0.3, 0.4, 0.65, 0.75, 0, 5.4, iron)],
      [3.5, 0.55, () => K.box(3.35, 0.4, 3.7, 0.75, 0, 5.4, iron)],
      [2, 0.55, () => {
        K.box(0.1, 0.35, 3.9, 0.6, 4.9, 5.6, wood);
        K.box(0.1, 0.35, 3.9, 0.6, 3.7, 4.4, wood);
      }],
      [2, 1.2, () => K.box(0.1, 0.55, 3.9, 1.85, 2.6, 3.2, wood)],
      [0.5, 1.6, () => K.box(0.3, 1.45, 0.65, 1.75, 0, 2.6, iron)],
      [3.5, 1.6, () => K.box(3.35, 1.45, 3.7, 1.75, 0, 2.6, iron)],
    ]);
  },

  picnic_table(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const wood = hexOf(item.color, '#a97440');
    shadow(S, K.rect(), 5);
    K.contact(0.7);
    const trestle = (a) => () => {
      for (const b of [0.55, 3.45]) K.seg(dark(wood, 0.3), 0.55, [a, b, 2.3], [a, 2, 3.7]);
      K.seg(dark(wood, 0.2), 0.5, [a, 0.55, 0], [a, 0.55, 2.3]);
      K.seg(dark(wood, 0.2), 0.5, [a, 3.45, 0], [a, 3.45, 2.3]);
    };
    K.parts([
      [3, 0.5, () => K.box(0.6, 0.1, 5.4, 0.9, 2.2, 2.7, wood)],
      [1.4, 1.9, trestle(1.4)],
      [4.6, 1.9, trestle(4.6)],
      [3, 3.5, () => K.box(0.6, 3.1, 5.4, 3.9, 2.2, 2.7, wood)],
      [3, 2, () => {
        K.box(0.4, 1.0, 5.6, 3.0, 3.6, 4.2, wood);
        for (const b of [1.5, 2.0, 2.5]) K.seg(dark(wood, 0.2), 0.2, [0.4, b, 4.2], [5.6, b, 4.2]);
      }, 1],
    ]);
  },

  patio_set(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const frame = hexOf(item.color, '#e6dfcc');
    shadow(S, K.rect(), 5);
    K.contact(0.7);
    // A chair has its back on the outside, so (na, nb) says which way that is.
    const chair = (a, b, na, nb) => [a, b, () => {
      for (const [da, db] of [[-0.42, -0.42], [0.42, -0.42], [-0.42, 0.42], [0.42, 0.42]]) K.rod(a + da, b + db, 0, 1.7, 0.3, dark(frame, 0.25));
      K.box(a - 0.55, b - 0.55, a + 0.55, b + 0.55, 1.7, 2.1, frame);
      K.box(a - 0.45, b - 0.45, a + 0.45, b + 0.45, 2.1, 2.4, '#58a99a');
      if (na) K.box(a + na * 0.5 - 0.1, b - 0.55, a + na * 0.5 + 0.1, b + 0.55, 2.1, 4.3, frame);
      else K.box(a - 0.55, b + nb * 0.5 - 0.1, a + 0.55, b + nb * 0.5 + 0.1, 2.1, 4.3, frame);
    }];
    K.parts([
      chair(2, 0.6, 0, -1),
      chair(0.6, 2, -1, 0),
      [2, 2, () => {
        K.cyl(2, 2, 0.25, 0, 2.9, dark(frame, 0.2));
        K.cyl(2, 2, 1.15, 2.9, 3.3, '#9ccbe0');
        strokeArc(K.g, frame, 0.35, ...K.at(2, 2, 3.3), 1.15 * 1.414, 1.15 * 0.707, 0, TAU);
      }],
      chair(3.4, 2, 1, 0),
      chair(2, 3.4, 0, 1),
    ]);
  },

  deck_chair(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const cloth = hexOf(item.color, '#3b8fd0');
    const wood = '#b98a58';
    shadow(S, K.rect(), 4);
    K.contact(0.5);
    // Side rails: the back leans up at the head end, the seat dips, the legs prop it.
    const rail = (a) => () => {
      K.seg(wood, 0.4, [a, 0.5, 4.3], [a, 2.2, 1.8]);
      K.seg(wood, 0.4, [a, 2.2, 1.8], [a, 3.7, 2.6]);
      K.seg(dark(wood, 0.2), 0.4, [a, 2.2, 1.8], [a, 3.4, 0]);
      K.seg(dark(wood, 0.2), 0.4, [a, 0.9, 3.4], [a, 0.7, 0]);
    };
    K.parts([
      [0.2, 2, rail(0.2)],
      [1, 2, () => {
        const sling = [[0.35, 0.5, 4.3], [1.65, 0.5, 4.3], [1.65, 2.2, 1.9], [0.35, 2.2, 1.9]];
        K.flat(cloth, ...sling);
        K.flat(cloth, [0.35, 2.2, 1.9], [1.65, 2.2, 1.9], [1.65, 3.7, 2.6], [0.35, 3.7, 2.6]);
        for (const t of [0.2, 0.4, 0.6, 0.8]) {
          K.seg(rgba('#ffffff', 0.55), 0.35, [0.35, 0.5 + t * 1.7, 4.3 - t * 2.4], [1.65, 0.5 + t * 1.7, 4.3 - t * 2.4]);
        }
        K.seg(dark(cloth, 0.25), 0.3, [0.35, 2.2, 1.9], [1.65, 2.2, 1.9]);
      }, 1],
      [1.8, 2, rail(1.8)],
    ]);
  },

  hammock(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const cloth = hexOf(item.color, '#e8825a');
    shadow(S, K.rect(), 6);
    K.contact(0.6);
    const sag = (t) => 6.2 - Math.sin(t * Math.PI) * 3.4;
    const N = 12;
    K.parts([
      [0.3, 1, () => K.rod(0.3, 1, 0, 7.4, 0.7, '#7d522c')],
      [3, 1, () => {
        const near = [];
        const far = [];
        for (let i = 0; i <= N; i++) {
          const t = i / N;
          const a = 0.6 + t * 4.8;
          // The cloth bellies out sideways as it dips.
          const wide = 0.1 + Math.pow(Math.sin(t * Math.PI), 0.7) * 0.82;
          far.push([a, 1 - wide, sag(t)]);
          near.push([a, 1 + wide, sag(t)]);
        }
        K.flat(dark(cloth, 0.12), ...far, ...near.reverse());
        for (let i = 1; i < N; i += 2) {
          const t = i / N;
          const wide = 0.1 + Math.pow(Math.sin(t * Math.PI), 0.7) * 0.82;
          K.flat(lite(cloth, 0.35), [0.6 + t * 4.8, 1 - wide, sag(t)], [0.6 + (t + 0.04) * 4.8, 1 - wide, sag(t + 0.04)], [0.6 + (t + 0.04) * 4.8, 1 + wide, sag(t + 0.04)], [0.6 + t * 4.8, 1 + wide, sag(t)]);
        }
        K.seg('#f6f0e4', 1.5, [1.4, 1.15, sag(0.18) + 0.2], [2.1, 1.15, sag(0.32) + 0.2]);
        K.seg('#6b5a44', 0.2, [0.3, 1, 7.2], [0.6, 1, sag(0)]);
        K.seg('#6b5a44', 0.2, [5.7, 1, 7.2], [5.4, 1, sag(1)]);
      }, 1],
      [5.7, 1, () => K.rod(5.7, 1, 0, 7.4, 0.7, '#7d522c'), 2],
    ]);
  },

  porch_swing(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const seat = hexOf(item.color, '#e9e4d6');
    const wood = '#7d522c';
    shadow(S, K.rect(), 8);
    K.contact(0.5);
    const frame = (a) => () => {
      K.seg(wood, 0.55, [a, 0.15, 0], [a, 1, 7.4]);
      K.seg(wood, 0.55, [a, 1.85, 0], [a, 1, 7.4]);
    };
    K.parts([
      [0.2, 1, frame(0.2)],
      [2, 1, () => {
        K.seg(wood, 0.7, [0.1, 1, 7.4], [3.9, 1, 7.4]);
        K.seg('#4a3a2a', 0.18, [0.8, 0.6, 7.3], [0.8, 0.6, 3.1]);
        K.seg('#4a3a2a', 0.18, [3.2, 0.6, 7.3], [3.2, 0.6, 3.1]);
        K.seg('#4a3a2a', 0.18, [0.8, 1.4, 7.3], [0.8, 1.4, 3.1]);
        K.seg('#4a3a2a', 0.18, [3.2, 1.4, 7.3], [3.2, 1.4, 3.1]);
        K.box(0.6, 0.5, 3.4, 1.5, 2.6, 3.2, seat);
        K.box(0.6, 0.5, 3.4, 0.75, 3.2, 5.4, seat);
        K.box(0.7, 0.55, 3.3, 1.4, 3.2, 3.55, '#d8785a');
      }, 1],
      [3.8, 1, frame(3.8), 2],
    ]);
  },

  fire_pit(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const [fx, fy] = at(2, 2, 1.2);
    if (!S.day) {
      pool(g, ...at(2, 2, 0), 17, '#ff9a3c', 0.5);
      glow(g, fx, fy - 1.5, 9, '#ffb04a', 0.6);
      flame(g, fx, fy, 4.6, 1.5);
      return;
    }
    shadow(S, K.rect(0.3), 3);
    K.contact(0.8);
    const stones = [];
    for (let i = 0; i < 12; i++) {
      const t = (i / 12) * TAU;
      const a = 2 + Math.cos(t) * 1.65;
      const b = 2 + Math.sin(t) * 1.65;
      stones.push([a, b, () => K.cyl(a, b, 0.5, 0, 1.4, i % 3 === 0 ? '#9d9a92' : i % 3 === 1 ? '#8a8780' : '#a7a49a')]);
    }
    K.parts([
      ...stones,
      [2, 2, () => {
        K.disc(2, 2, 1.45, '#2a2527', 0.9);
        K.seg('#5a3d26', 1.1, [1.3, 1.7, 1.1], [2.7, 2.3, 1.4]);
        K.seg('#6b4a2e', 1.1, [1.3, 2.4, 1.1], [2.7, 1.6, 1.4]);
        ell(g, '#ff7a2a', fx - 1, fy + 0.3, 0.6, 0.3);
        flame(g, fx, fy, 4.6, 1.5);
      }],
    ]);
  },

  bbq_grill(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    shadow(S, K.rect(0.3), 5);
    K.contact(0.5);
    const [x, y] = at(1, 1, 0);
    for (const [a, b] of [[0.5, 0.6], [1.5, 0.6], [1, 1.6]]) K.seg('#2b2d34', 0.35, [1, 1, 2.6], [a, b, 0]);
    ball(g, x, y - 3.9, 1.5, '#c8412f', 1.8, 1.25);
    // The lid sits ajar, with a handle and a vent.
    g.beginPath();
    g.ellipse(x, y - 4.4, 1.85, 1.15, 0, Math.PI, TAU);
    g.closePath();
    g.fillStyle = grad(g, x - 1.8, 0, x + 1.8, 0, lite('#c8412f', 0.3), '#c8412f', dark('#c8412f', 0.3));
    g.fill();
    ell(g, '#1e1d24', x, y - 4.4, 1.85, 0.7);
    ell(g, '#c9c5bc', x, y - 5.4, 0.4, 0.2);
    line(g, '#2b2d34', 0.4, x + 1.6, y - 4.5, x + 2.2, y - 3.4);
    K.box(1.55, 0.6, 1.95, 1.5, 2.3, 2.5, '#6b4a2e');
  },

  parasol(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    const cloth = hexOf(item.color, '#e5484d');
    shadow(S, K.rect(0.2), 8);
    K.contact(0.6);
    K.cyl(2, 2, 0.9, 0, 0.7, '#6b6f7c');
    K.rod(2, 2, 0, 9.4, 0.4, '#d8d2c2');
    const [tx, ty] = at(2, 2, 11);
    const rim = [];
    for (let i = 0; i < 12; i++) {
      const t = (i / 12) * TAU;
      rim.push([Math.cos(t), Math.sin(t)]);
    }
    const wedges = rim.map((p, i) => ({ i, p, q: rim[(i + 1) % 12] }));
    const order = wedges.sort((m, n) => (m.p[0] + m.p[1] + m.q[0] + m.q[1]) - (n.p[0] + n.p[1] + n.q[0] + n.q[1]));
    for (const { i, p, q } of order) {
      const r = 3.7;
      const pa = at(2 + p[0] * r, 2 + p[1] * r, 8.1);
      const qa = at(2 + q[0] * r, 2 + q[1] * r, 8.1);
      const mid = (p[0] + q[0] + p[1] + q[1]) * 0.25;
      const base = i % 2 ? '#fff6e6' : cloth;
      fp(g, tint(base, -mid * 0.18 + 0.05), [[tx, ty], pa, qa]);
      // A scalloped hem.
      const hx = (pa[0] + qa[0]) / 2;
      const hy = (pa[1] + qa[1]) / 2;
      ell(g, tint(base, -mid * 0.18 + 0.05), hx, hy + 0.1, Math.hypot(qa[0] - pa[0], qa[1] - pa[1]) / 2, 0.35);
    }
    ball(g, tx, ty - 0.3, 0.45, '#d8d2c2');
  },

  gazebo(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const roof = hexOf(item.color, '#b5533c');
    const white = '#f4f0e6';
    shadow(S, K.rect(0.3), 15);
    K.contact(0.8);
    // Floor and steps.
    K.box(0.2, 0.2, 7.8, 7.8, 0, 1, '#a9906a');
    const post = (a, b) => [a, b, () => K.box(a - 0.3, b - 0.3, a + 0.3, b + 0.3, 1, 9.2, white)];
    const rail = (a0, b0, a1, b1) => () => {
      const bars = 6;
      K.seg(white, 0.45, [a0, b0, 3.8], [a1, b1, 3.8]);
      K.seg(white, 0.35, [a0, b0, 1.5], [a1, b1, 1.5]);
      for (let i = 1; i < bars; i++) {
        const t = i / bars;
        K.seg(white, 0.25, [a0 + (a1 - a0) * t, b0 + (b1 - b0) * t, 1.5], [a0 + (a1 - a0) * t, b0 + (b1 - b0) * t, 3.8]);
      }
    };
    K.parts([
      // A bench round the inside, then the posts and rails.
      [4, 1.2, () => K.box(1.2, 0.8, 6.8, 1.5, 1, 2.2, '#8f6a42')],
      [1.2, 4, () => K.box(0.8, 1.5, 1.5, 6.8, 1, 2.2, '#8f6a42')],
      post(0.8, 0.8),
      post(7.2, 0.8),
      post(0.8, 7.2),
      post(7.2, 7.2),
      [4, 0.8, rail(0.8, 0.8, 7.2, 0.8)],
      [0.8, 4, rail(0.8, 0.8, 0.8, 7.2)],
      [4, 7.2, rail(0.8, 7.2, 3.2, 7.2)],
      [4, 7.2, rail(4.8, 7.2, 7.2, 7.2)],
      [7.2, 4, rail(7.2, 0.8, 7.2, 3.2)],
      [7.2, 4, rail(7.2, 4.8, 7.2, 7.2)],
    ]);
    // The roof: a pyramid with a little cap, over the lot.
    const ap = [4, 4, 14.2];
    const e = [-0.5, 8.5];
    K.q(roof, [0, 1, 0.7], [e[0], e[1], 9], [e[1], e[1], 9], ap);
    K.q(roof, [1, 0, 0.7], [e[1], e[0], 9], [e[1], e[1], 9], ap);
    K.q(roof, [0, -1, 0.7], [e[0], e[0], 9], [e[1], e[0], 9], ap);
    K.q(roof, [-1, 0, 0.7], [e[0], e[0], 9], [e[0], e[1], 9], ap);
    K.q(dark(roof, 0.35), [0, 1, 0], [e[0], e[1], 9], [e[1], e[1], 9], [e[1], e[1], 8.5], [e[0], e[1], 8.5]);
    K.q(dark(roof, 0.35), [1, 0, 0], [e[1], e[0], 9], [e[1], e[1], 9], [e[1], e[1], 8.5], [e[1], e[0], 8.5]);
    K.rod(4, 4, 14, 16.4, 0.35, '#d9b44a');
    K.ball(4, 4, 16.6, 0.6, '#d9b44a');
  },

  // --- Play ----------------------------------------------------------------------------------

  swing_set(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const frame = hexOf(item.color, '#e0533d');
    shadow(S, K.rect(0.2), 9);
    K.contact(0.6);
    const leg = (a) => () => {
      K.seg(frame, 0.6, [a, 0.4, 0], [a, 2, 9]);
      K.seg(frame, 0.6, [a, 3.6, 0], [a, 2, 9]);
    };
    const swing = (a) => () => {
      K.seg('#8a8f9c', 0.18, [a - 0.5, 2, 9], [a - 0.5, 2, 2.6]);
      K.seg('#8a8f9c', 0.18, [a + 0.5, 2, 9], [a + 0.5, 2, 2.6]);
      K.box(a - 0.7, 1.7, a + 0.7, 2.3, 2.2, 2.7, '#2f4a7a');
    };
    K.parts([
      [0.4, 2, leg(0.4)],
      [2, 2, swing(2), 1],
      [4, 2, swing(4), 1],
      [3, 2, () => K.seg(dark(frame, 0.1), 0.8, [0.2, 2, 9], [5.8, 2, 9]), 1],
      [5.6, 2, leg(5.6), 2],
    ]);
  },

  slide(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const chute = hexOf(item.color, '#f2c230');
    const wood = '#b07a46';
    shadow(S, K.rect(0.2), 7);
    K.contact(0.5);
    const post = (a, b) => [a, b, () => K.rod(a, b, 0, 6.2, 0.55, wood)];
    K.parts([
      post(0.2, 0.2),
      post(1.6, 0.2),
      // The slope runs from the platform down to the grass.
      [3.8, 1, () => {
        const top = [1.6, 6.1];
        const foot = [5.8, 0.6];
        for (const b of [0.2, 1.8]) {
          K.flat(dark(chute, 0.15), [top[0], b, top[1]], [foot[0], b, foot[1]], [foot[0], b, foot[1] + 0.7], [top[0], b, top[1] + 0.7]);
        }
        K.flat(chute, [top[0], 0.2, top[1]], [foot[0], 0.2, foot[1]], [foot[0], 1.8, foot[1]], [top[0], 1.8, top[1]]);
        K.seg(lite(chute, 0.4), 0.3, [top[0], 0.8, top[1]], [foot[0], 0.8, foot[1]]);
      }, 1],
      // The platform with its rail and a little roof.
      [0.8, 1, () => {
        K.box(0, 0, 1.8, 2, 5.6, 6.2, wood);
        K.seg(wood, 0.3, [0.05, 0.1, 6.2], [0.05, 0.1, 7.8]);
        K.seg(wood, 0.3, [0.05, 1.9, 6.2], [0.05, 1.9, 7.8]);
        K.seg(dark(wood, 0.2), 0.5, [0.05, 0.1, 7.8], [0.05, 1.9, 7.8]);
        K.seg(wood, 0.3, [1.7, 0.1, 6.2], [1.7, 0.1, 7.8]);
        K.seg(dark(wood, 0.2), 0.4, [0.05, 0.1, 7.8], [1.7, 0.1, 7.8]);
      }, 2],
      post(0.2, 1.8),
      post(1.6, 1.8),
    ]);
    // The ladder up the front.
    K.seg(wood, 0.3, [0.3, 1.95, 0], [0.3, 1.95, 5.8]);
    K.seg(wood, 0.3, [1.5, 1.95, 0], [1.5, 1.95, 5.8]);
    for (let z = 1; z < 5.6; z += 1.1) K.seg(lite(wood, 0.2), 0.25, [0.3, 1.97, z], [1.5, 1.97, z]);
  },

  sandbox(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const wood = '#9c6b3e';
    shadow(S, K.rect(), 2);
    K.contact(0.6);
    const g = K.g;
    // Back boards, the sand, the toys, then the near boards.
    K.box(0.1, 0.1, 3.9, 0.5, 0, 1.4, wood);
    K.box(0.1, 0.5, 0.5, 3.9, 0, 1.4, wood);
    K.flat('#e8d29a', [0.5, 0.5, 1.1], [3.5, 0.5, 1.1], [3.5, 3.5, 1.1], [0.5, 3.5, 1.1]);
    K.flat('#d8bf84', [0.8, 0.9, 1.12], [2.4, 0.9, 1.12], [2.6, 1.6, 1.12], [1.0, 1.7, 1.12]);
    K.cyl(1.6, 1.4, 0.9, 1.1, 1.9, '#e9d9a8', 0.5);
    K.cyl(1.6, 1.4, 0.35, 1.9, 2.6, '#e0cb92', 0.3);
    ell(g, '#d94040', ...K.at(2.7, 2.9, 1.7), 0.9, 0.4);
    K.cyl(2.7, 2.9, 0.5, 1.1, 2.1, '#d94040', 0.65);
    K.disc(2.7, 2.9, 0.4, '#8a5a3a', 2.1);
    K.seg('#2f78d0', 0.5, [1.4, 2.7, 1.4], [2.2, 3.1, 1.9]);
    K.box(3.5, 0.5, 3.9, 3.9, 0, 1.4, wood);
    K.box(0.1, 3.5, 3.9, 3.9, 0, 1.4, wood);
    K.seg(rgba('#ffffff', 0.25), 0.3, [0.1, 3.9, 1.3], [3.9, 3.9, 1.3]);
  },

  trampoline(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    const pad = hexOf(item.color, '#2f5d9a');
    shadow(S, K.rect(0.3), 8);
    K.contact(0.9);
    const R = 2.6;
    const post = (i) => {
      const t = (i / 10) * TAU;
      return [3 + Math.cos(t) * R, 3 + Math.sin(t) * R];
    };
    const net = (i) => () => {
      const [a, b] = post(i);
      const [c, d] = post(i + 1);
      fp(g, 'rgba(235,240,250,.2)', [at(a, b, 3.2), at(c, d, 3.2), at(c, d, 8), at(a, b, 8)]);
      K.seg('#d8dce6', 0.35, [a, b, 3.2], [a, b, 8]);
    };
    const halves = [];
    for (let i = 0; i < 10; i++) {
      const [a, b] = post(i);
      halves.push([a, b, net(i), 1]);
    }
    K.parts([
      [3, 3, () => {
        for (let i = 0; i < 6; i++) {
          const t = (i / 6) * TAU + 0.3;
          K.seg('#4a4d57', 0.5, [3 + Math.cos(t) * R, 3 + Math.sin(t) * R, 0], [3 + Math.cos(t) * R, 3 + Math.sin(t) * R, 3.1]);
        }
        K.disc(3, 3, R + 0.25, pad, 3.1);
        K.disc(3, 3, R - 0.45, '#25262e', 3.14);
        K.disc(3, 3, R - 1.2, '#2d2f38', 3.18);
      }],
      ...halves,
    ]);
    strokeArc(g, '#d8dce6', 0.45, ...at(3, 3, 8), (R + 0.1) * 1.414, (R + 0.1) * 0.707, 0, TAU);
  },

  seesaw(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const plank = hexOf(item.color, '#e5b53a');
    shadow(S, K.rect(0.2), 5);
    K.contact(0.3);
    const lo = [0.4, 1.0];
    const hi = [5.6, 4.9];
    const z = (a) => lo[1] + ((a - lo[0]) / (hi[0] - lo[0])) * (hi[1] - lo[1]);
    const iron = '#4a4d57';
    K.parts([
      [3, 1, () => {
        // The pivot is a little A-frame under the middle of the plank.
        for (const b of [0.55, 1.45]) {
          K.seg(iron, 0.55, [2.5, b, 0], [3, b, z(3) - 0.2]);
          K.seg(iron, 0.55, [3.5, b, 0], [3, b, z(3) - 0.2]);
        }
        K.seg(iron, 0.4, [3, 0.55, z(3) - 0.2], [3, 1.45, z(3) - 0.2]);
      }],
      [3, 1, () => {
        K.flat(dark(plank, 0.15), [0.3, 0.45, z(0.3)], [5.7, 0.45, z(5.7)], [5.7, 0.45, z(5.7) - 0.5], [0.3, 0.45, z(0.3) - 0.5]);
        K.flat(plank, [0.3, 0.45, z(0.3)], [5.7, 0.45, z(5.7)], [5.7, 1.55, z(5.7)], [0.3, 1.55, z(0.3)]);
        K.seg(lite(plank, 0.3), 0.25, [0.3, 0.85, z(0.3)], [5.7, 0.85, z(5.7)]);
        // A handle at each end.
        for (const a of [1.0, 5.0]) {
          K.seg(iron, 0.3, [a, 0.6, z(a)], [a, 0.6, z(a) + 1.5]);
          K.seg(iron, 0.3, [a, 1.4, z(a)], [a, 1.4, z(a) + 1.5]);
          K.seg(iron, 0.4, [a, 0.6, z(a) + 1.5], [a, 1.4, z(a) + 1.5]);
        }
      }, 1],
    ]);
  },

  tire_swing(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    shadow(S, K.rect(0.3), 9);
    K.contact(0.4);
    K.rod(0.3, 1, 0, 9.4, 0.8, '#7d522c');
    K.seg('#7d522c', 0.7, [0.3, 1, 9.2], [1.7, 1, 9.2]);
    K.seg('#c9b88a', 0.2, [1.5, 1, 9.2], [1.5, 1, 4]);
    const [x, y] = at(1.5, 1, 3);
    // The tyre, with its hole showing and a lit edge.
    strokeArc(g, '#2a2a30', 1.2, x, y, 1.0, 1.35, 0, TAU);
    strokeArc(g, '#4a4a54', 0.35, x - 0.1, y - 0.1, 1.05, 1.4, 3.4, 5.3);
    ell(g, 'rgba(60,110,50,.5)', x, y + 1.9, 0.7, 0.3);
  },

  basketball_hoop(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    shadow(S, K.rect(0.3), 11);
    K.contact(0.5);
    K.box(0.15, 0.55, 0.85, 1.45, 0, 0.9, '#5d6170');
    K.rod(0.5, 1, 0.9, 10.4, 0.6, '#6b6f7c');
    K.seg('#6b6f7c', 0.5, [0.5, 1, 9.6], [1.1, 1, 9.6]);
    // The board is in the plane across the court, with a red square.
    K.q('#f4f6fa', [1, 0, 0], [1.1, 0.1, 7.2], [1.1, 1.9, 7.2], [1.1, 1.9, 10.8], [1.1, 0.1, 10.8]);
    K.q('#d94040', [1, 0, 0], [1.12, 0.6, 7.6], [1.12, 1.4, 7.6], [1.12, 1.4, 8.7], [1.12, 0.6, 8.7]);
    K.q('#f4f6fa', [1, 0, 0], [1.14, 0.7, 7.7], [1.14, 1.3, 7.7], [1.14, 1.3, 8.6], [1.14, 0.7, 8.6]);
    // The net hangs from the rim.
    const rimC = [1.75, 1, 7.4];
    const n = 8;
    for (let i = 0; i < n; i++) {
      const t = (i / n) * TAU;
      const p = [rimC[0] + Math.cos(t) * 0.6, rimC[1] + Math.sin(t) * 0.6, rimC[2]];
      const q = [rimC[0] + Math.cos(t + 0.4) * 0.38, rimC[1] + Math.sin(t + 0.4) * 0.38, rimC[2] - 1.4];
      K.seg('rgba(250,250,250,.85)', 0.18, p, q);
    }
    K.seg('rgba(250,250,250,.7)', 0.18, [rimC[0] - 0.45, rimC[1], 6.4], [rimC[0] + 0.45, rimC[1], 6.4]);
    const [rx, ry] = at(rimC[0], rimC[1], rimC[2]);
    strokeArc(g, '#e8672b', 0.5, rx, ry, 0.6 * 1.414, 0.6 * 0.707, 0, TAU);
  },

  tent(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const cloth = hexOf(item.color, '#e8823c');
    const inside = '#2b2230';
    shadow(S, K.rect(0.3), 5);
    K.contact(0.8);
    const R = [0.4, 5.6];
    const H = 4.8;
    K.q(cloth, [0, -0.94, 0.35], [R[0], 0.2, 0], [R[1], 0.2, 0], [R[1], 2, H], [R[0], 2, H]);
    K.q(cloth, [-1, 0, 0], [R[0], 0.2, 0], [R[0], 3.8, 0], [R[0], 2, H]);
    K.q(cloth, [0, 0.94, 0.35], [R[0], 3.8, 0], [R[1], 3.8, 0], [R[1], 2, H], [R[0], 2, H]);
    K.q(cloth, [1, 0, 0], [R[1], 0.2, 0], [R[1], 3.8, 0], [R[1], 2, H]);
    // The door: an opening with the flap rolled to one side.
    K.q(inside, [1, 0, 0], [R[1] + 0.02, 1.0, 0], [R[1] + 0.02, 3.0, 0], [R[1] + 0.02, 2, 3.4]);
    K.q(dark(cloth, 0.2), [1, 0, 0], [R[1] + 0.06, 2, 0], [R[1] + 0.06, 3.0, 0], [R[1] + 0.06, 2, 3.4]);
    K.seg(lite(cloth, 0.4), 0.3, [R[0], 2, H], [R[1], 2, H]);
    // Stitched seams down the slopes, a pole tip and guy lines to pegs.
    for (const a of [1.7, 3.0, 4.3]) {
      K.seg(rgba('#000000', 0.12), 0.25, [a, 3.8, 0], [a, 2, H]);
      K.seg(rgba('#000000', 0.12), 0.25, [a, 0.2, 0], [a, 2, H]);
    }
    K.rod(R[1] + 0.6, 2, 0, 5.6, 0.3, '#c9b88a');
    K.seg('#c9b88a', 0.5, [R[1], 2, H], [R[1] + 0.6, 2, 5.4]);
    K.seg('#e8e2cf', 0.15, [R[1] + 0.6, 2, 5.4], [R[1] + 0.9, 0.2, 0]);
    K.seg('#e8e2cf', 0.15, [R[1] + 0.6, 2, 5.4], [R[1] + 0.9, 3.8, 0]);
  },

  // --- Lights --------------------------------------------------------------------------------

  lamp_post(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const lamp = hexOf(item.color, '#ffe3a0');
    const [hx, hy] = at(1, 1, 14.4);
    if (!S.day) {
      pool(g, ...at(1, 1, 0), 15, lamp, 0.4);
      glow(g, hx, hy, 13, lamp, 0.6);
      ell(g, lite(lamp, 0.6), hx, hy, 0.8, 1.4);
      return;
    }
    const iron = '#33374a';
    shadow(S, K.rect(0.5), 15);
    K.contact(0.35);
    K.box(0.5, 0.5, 1.5, 1.5, 0, 1.6, iron);
    K.cyl(1, 1, 0.6, 1.6, 2.4, dark(iron, 0.1), 0.35);
    K.rod(1, 1, 2.2, 13, 0.7, '#41465a');
    K.cyl(1, 1, 0.55, 12.6, 13.1, iron);
    // The lamp: a lit glass lantern under a little roof.
    K.box(0.55, 0.55, 1.45, 1.45, 13.1, 15.4, mixHex(lamp, '#8a8272', 0.55));
    K.seg(iron, 0.2, [0.55, 1.46, 13.1], [0.55, 1.46, 15.4]);
    K.seg(iron, 0.2, [1.45, 1.46, 13.1], [1.45, 1.46, 15.4]);
    K.q(iron, [0, 1, 0.7], [0.3, 1.7, 15.4], [1.7, 1.7, 15.4], [1, 1, 17]);
    K.q(iron, [1, 0, 0.7], [1.7, 0.3, 15.4], [1.7, 1.7, 15.4], [1, 1, 17]);
    K.ball(1, 1, 17.2, 0.5, iron);
  },

  lantern(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const lamp = hexOf(item.color, '#ffc870');
    const [hx, hy] = at(1.7, 1, 5.4);
    if (!S.day) {
      pool(g, ...at(1.7, 1, 0), 10, lamp, 0.4);
      glow(g, hx, hy, 9, lamp, 0.65);
      ell(g, lite(lamp, 0.6), hx, hy, 0.7, 1);
      return;
    }
    const iron = '#3a3326';
    shadow(S, K.rect(0.4), 8);
    K.contact(0.3);
    K.rod(0.3, 1, 0, 8, 0.6, '#6b4a2e');
    K.seg(iron, 0.4, [0.3, 1, 8], [1.7, 1, 8]);
    K.seg(iron, 0.2, [1.7, 1, 8], [1.7, 1, 6.6]);
    K.box(1.35, 0.65, 2.05, 1.35, 4.2, 6.2, mixHex(lamp, '#8a8272', 0.5));
    K.box(1.28, 0.58, 2.12, 1.42, 6.2, 6.6, iron);
    K.box(1.28, 0.58, 2.12, 1.42, 3.9, 4.2, iron);
    K.seg(iron, 0.2, [1.35, 1.36, 4.2], [1.35, 1.36, 6.2]);
    K.seg(iron, 0.2, [2.05, 1.36, 4.2], [2.05, 1.36, 6.2]);
  },

  garden_light(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const lamp = hexOf(item.color, '#fff1b8');
    const [hx, hy] = at(1, 1, 3.3);
    if (!S.day) {
      pool(g, ...at(1, 1, 0), 8, lamp, 0.45);
      glow(g, hx, hy, 6, lamp, 0.6);
      ell(g, lite(lamp, 0.5), hx, hy, 0.9, 0.9);
      return;
    }
    shadow(S, K.rect(0.5), 4);
    K.rod(1, 1, 0, 2.6, 0.35, '#4a4d57');
    K.cyl(1, 1, 0.5, 0, 0.5, '#4a4d57');
    // A glass dome under a cap, which looks like a small mushroom.
    ball(g, hx, hy, 1, mixHex(lamp, '#a8a090', 0.5), 1.05, 0.95);
    g.beginPath();
    g.ellipse(hx, hy - 0.3, 1.4, 0.75, 0, Math.PI, TAU);
    g.closePath();
    g.fillStyle = grad(g, hx - 1.4, 0, hx + 1.4, 0, '#6b6f7c', '#3a3d4a');
    g.fill();
  },

  torch(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const [fx, fy] = at(1, 1, 10);
    if (!S.day) {
      pool(g, ...at(1, 1, 0), 12, '#ff9a3c', 0.4);
      glow(g, fx, fy - 1.5, 10, '#ffb04a', 0.65);
      flame(g, fx, fy, 3.8, 1.2);
      return;
    }
    shadow(S, K.rect(0.5), 10);
    K.contact(0.3);
    K.rod(1, 1, 0, 9, 0.6, '#b9955a');
    for (const z of [1.8, 4, 6.2]) K.disc(1, 1, 0.42, '#7d6234', z);
    K.cyl(1, 1, 0.7, 8.6, 9.8, '#4a3a2a', 0.9);
    K.disc(1, 1, 0.7, '#e0c070', 9.8);
    flame(g, fx, fy, 3.8, 1.2);
  },

  string_post(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const bulb = hexOf(item.color, '#ffd98a');
    // The strings run from the top of the post down to little stakes round the foot.
    const ends = [[0.1, 0.1], [1.9, 0.1], [0.1, 1.9], [1.9, 1.9]];
    const top = [1, 1, 11.4];
    const lights = [];
    for (const [a, b] of ends) {
      for (let i = 1; i <= 3; i++) {
        const t = i / 3.4;
        const sagz = Math.sin(t * Math.PI) * 0.6;
        lights.push(at(top[0] + (a - top[0]) * t, top[1] + (b - top[1]) * t, top[2] * (1 - t) + 0.4 * t - sagz));
      }
    }
    if (!S.day) {
      pool(g, ...at(1, 1, 0), 9, bulb, 0.3);
      for (const [x, y] of lights) {
        glow(g, x, y, 3.2, bulb, 0.32);
        ell(g, lite(bulb, 0.6), x, y, 0.45, 0.45);
      }
      glow(g, ...at(1, 1, 11.4), 4, bulb, 0.4);
      return;
    }
    shadow(S, K.rect(0.5), 12);
    K.contact(0.3);
    for (const [a, b] of ends) {
      K.seg('rgba(40,36,46,.8)', 0.18, top, [a, b, 0.4]);
      K.rod(a, b, 0, 0.8, 0.3, '#5a4a3a');
    }
    K.rod(1, 1, 0, 11.4, 0.6, '#6b4a2e');
    for (const [x, y] of lights) ell(g, mixHex(bulb, '#e8e2cf', 0.5), x, y, 0.4, 0.4);
    K.ball(1, 1, 11.5, 0.5, '#4a4d57');
  },

  spotlight(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const lamp = hexOf(item.color, '#fff3c0');
    const [lx, ly] = at(1, 1, 3.4);
    if (!S.day) {
      pool(g, ...at(1, 1, 0), 9, lamp, 0.35);
      // The beam leans up and out from the lens.
      const beam = g.createLinearGradient(lx, ly, lx + 2, ly - 13);
      beam.addColorStop(0, rgba(lamp, 0.4));
      beam.addColorStop(1, rgba(lamp, 0));
      fp(g, beam, [[lx - 0.8, ly], [lx + 0.9, ly - 0.4], [lx + 6.5, ly - 13], [lx - 3.5, ly - 12]]);
      glow(g, lx, ly, 4.5, lamp, 0.7);
      ell(g, lite(lamp, 0.6), lx + 0.1, ly - 0.1, 0.8, 0.5);
      return;
    }
    shadow(S, K.rect(0.5), 5);
    K.contact(0.35);
    K.seg('#3a3d4a', 0.5, [1, 1, 0], [1, 1, 2.2]);
    K.disc(1, 1, 0.6, '#3a3d4a', 0);
    // The lamp head tilts up, with its lens facing out.
    g.save();
    g.translate(lx, ly);
    g.rotate(-0.5);
    rr(g, grad(g, 0, -1.2, 0, 1.2, '#6b6f7c', '#2f3342'), -1.5, -1.2, 3, 2.4);
    ell(g, mixHex(lamp, '#a8a090', 0.5), 1.5, 0, 0.5, 1.1);
    g.restore();
  },

  campfire(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const [fx, fy] = at(1, 1, 1);
    if (!S.day) {
      pool(g, ...at(1, 1, 0), 14, '#ff9a3c', 0.5);
      glow(g, fx, fy - 1.5, 10, '#ffb04a', 0.65);
      flame(g, fx, fy, 4.4, 1.4);
      return;
    }
    shadow(S, K.rect(0.4), 3);
    K.contact(0.35);
    for (let i = 0; i < 8; i++) {
      const t = (i / 8) * TAU;
      K.parts([[1 + Math.cos(t) * 0.9, 1 + Math.sin(t) * 0.9, () => K.cyl(1 + Math.cos(t) * 0.9, 1 + Math.sin(t) * 0.9, 0.22, 0, 0.5, i % 2 ? '#8a8780' : '#a7a49a')]]);
    }
    // Logs leaning in on each other.
    K.seg('#4a3322', 0.8, [0.35, 1, 0.2], [1, 1, 2.8]);
    K.seg('#5a3d26', 0.8, [1, 0.35, 0.2], [1, 1, 2.8]);
    K.seg('#6b4a2e', 0.8, [1.65, 1, 0.2], [1, 1, 2.8]);
    K.seg('#5a3d26', 0.8, [1, 1.65, 0.2], [1, 1, 2.6]);
    flame(g, fx, fy, 4.4, 1.4);
  },

  // --- Decor ---------------------------------------------------------------------------------

  mailbox(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const body = hexOf(item.color, '#2f5d9a');
    shadow(S, K.rect(0.5), 7);
    K.contact(0.3);
    K.rod(1, 1, 0, 4.6, 0.8, '#6b4a2e');
    K.box(0.5, 0.5, 1.5, 1.5, 4.4, 5.2, '#5a4a3a');
    // The box has a rounded top and an open end that faces along the footprint.
    K.box(0.15, 0.55, 1.85, 1.45, 4.9, 6.3, body);
    K.box(0.15, 0.55, 1.85, 1.45, 6.3, 6.6, lite(body, 0.15));
    K.q(dark(body, 0.5), [1, 0, 0], [1.86, 0.65, 5.0], [1.86, 1.35, 5.0], [1.86, 1.35, 6.4], [1.86, 0.65, 6.4]);
    K.q(lite(body, 0.25), [0, 1, 0], [0.5, 1.46, 5.4], [1.5, 1.46, 5.4], [1.5, 1.46, 5.55], [0.5, 1.46, 5.55]);
    // The flag is up.
    K.seg('#8a8f9c', 0.3, [1.4, 1.5, 5.6], [1.4, 1.5, 7.5]);
    K.q('#e5484d', [0, 1, 0], [1.4, 1.52, 7.5], [1.4, 1.52, 6.6], [0.8, 1.52, 6.6], [0.8, 1.52, 7.5]);
  },

  bird_house(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const body = hexOf(item.color, '#e7c27a');
    const roof = '#8a3b2c';
    shadow(S, K.rect(0.5), 9);
    K.contact(0.3);
    K.rod(1, 1, 0, 6.4, 0.7, '#6b4a2e');
    K.box(0.5, 0.5, 1.5, 1.5, 6.2, 8.2, body);
    K.q(body, [0, 1, 0], [0.5, 1.5, 8.2], [1.5, 1.5, 8.2], [1, 1.5, 9.1]);
    K.q(body, [0, -1, 0], [0.5, 0.5, 8.2], [1.5, 0.5, 8.2], [1, 0.5, 9.1]);
    K.q(roof, [-0.7, 0, 0.7], [0.2, 0.2, 8.1], [0.2, 1.8, 8.1], [1, 1.8, 9.5], [1, 0.2, 9.5]);
    K.q(roof, [0.7, 0, 0.7], [1.8, 0.2, 8.1], [1.8, 1.8, 8.1], [1, 1.8, 9.5], [1, 0.2, 9.5]);
    K.circ('#2a2018', 1, 1.52, 7.3, 0.33, 'a', 1);
    K.seg('#6b4a2e', 0.25, [1, 1.52, 6.7], [1, 1.9, 6.7]);
  },

  gnome(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const hat = hexOf(item.color, '#d94040');
    shadow(S, K.rect(0.5), 7);
    K.contact(0.3);
    K.figure(1, 1, () => {
      const g = K.g;
      rr(g, '#3a2a1e', -1.3, -0.8, 1.2, 0.8);
      rr(g, '#3a2a1e', 0.1, -0.8, 1.2, 0.8);
      // Blue coat, a belt, then the beard over the top of it.
      fp(g, grad(g, -1.6, 0, 1.6, 0, '#5b86d6', '#2f4f96'), [[-1.5, -0.7], [1.5, -0.7], [1.2, -3.4], [-1.2, -3.4]]);
      rr(g, '#2a2a30', -1.4, -1.7, 2.8, 0.45);
      rr(g, '#e8c040', -0.3, -1.75, 0.6, 0.55);
      fp(g, grad(g, -1.5, 0, 1.5, 0, '#ffffff', '#cfd4de'), [[-1.4, -4.6], [1.4, -4.6], [0.5, -1.6], [-0.5, -1.6]]);
      ell(g, '#f2b8a0', 0, -4.6, 0.55, 0.5);
      ell(g, '#f2b8a0', 1.5, -2.6, 0.45, 0.45);
      ell(g, '#f2b8a0', -1.5, -2.6, 0.45, 0.45);
      // The hat, with a bent tip.
      g.beginPath();
      g.moveTo(-1.8, -5.1);
      g.quadraticCurveTo(-1, -7.2, 1.4, -9.4);
      g.quadraticCurveTo(1.9, -7, 1.8, -5.1);
      g.closePath();
      g.fillStyle = grad(g, -1.8, 0, 1.8, 0, lite(hat, 0.25), hat, dark(hat, 0.3));
      g.fill();
      ell(g, dark(hat, 0.2), 0, -5.1, 1.85, 0.5);
      ell(g, '#2a2a30', -0.3, -5.1, 0.25, 0.18);
    }, 0.75);
  },

  flamingo(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const body = hexOf(item.color, '#f48fb1');
    shadow(S, K.rect(0.5), 8);
    K.contact(0.3);
    K.figure(1, 1, () => {
      const g = K.g;
      // One leg down, the other tucked up.
      line(g, '#e0708f', 0.35, 0.1, -4.2, 0.1, 0);
      line(g, '#e0708f', 0.25, 0.4, -4, 1.2, -3);
      line(g, '#e0708f', 0.25, 1.2, -3, 0.5, -2.4);
      ell(g, '#d9677f', 0.1, 0, 0.7, 0.3);
      // The body, the tail feathers and the folded wing.
      g.beginPath();
      g.moveTo(-1.9, -5.7);
      g.quadraticCurveTo(-1, -3.6, 1.1, -4.1);
      g.quadraticCurveTo(1.7, -5, 1.3, -6);
      g.quadraticCurveTo(0, -6.4, -1.9, -5.7);
      g.closePath();
      g.fillStyle = grad(g, -1.8, -6, 1.4, -4, lite(body, 0.3), body, dark(body, 0.25));
      g.fill();
      g.beginPath();
      g.moveTo(-0.9, -5.5);
      g.quadraticCurveTo(0, -4.3, 0.9, -5);
      g.quadraticCurveTo(0.1, -5.3, -0.9, -5.5);
      g.fillStyle = dark(body, 0.22);
      g.fill();
      // An S of a neck, then the head with its bent beak.
      g.strokeStyle = body;
      g.lineWidth = 0.6;
      g.beginPath();
      g.moveTo(1.1, -5.6);
      g.bezierCurveTo(2.6, -6.6, -0.2, -7.6, 1.5, -9.2);
      g.stroke();
      ell(g, lite(body, 0.1), 1.6, -9.4, 0.55, 0.5);
      g.beginPath();
      g.moveTo(1.9, -9.5);
      g.lineTo(3, -9.2);
      g.lineTo(2.4, -8.7);
      g.closePath();
      g.fillStyle = '#f2e6d8';
      g.fill();
      g.beginPath();
      g.moveTo(2.5, -9.3);
      g.lineTo(3, -9.2);
      g.lineTo(2.6, -8.9);
      g.closePath();
      g.fillStyle = '#2a2a30';
      g.fill();
      ell(g, '#2a2a30', 1.6, -9.5, 0.15, 0.15);
    }, 0.85);
  },

  statue(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const stone = hexOf(item.color, '#cfcbc0');
    shadow(S, K.rect(0.4), 9);
    K.contact(0.4);
    K.box(0.3, 0.3, 1.7, 1.7, 0, 0.8, dark(stone, 0.1));
    K.box(0.5, 0.5, 1.5, 1.5, 0.8, 2.4, stone);
    K.box(0.4, 0.4, 1.6, 1.6, 2.4, 2.8, lite(stone, 0.1));
    K.figure(1, 1, () => {
      const g = K.g;
      const top = -2.8;
      // A robed figure holding one arm up.
      fp(g, grad(g, -1, 0, 1, 0, lite(stone, 0.2), stone, dark(stone, 0.25)), [[-1.1, top], [1.1, top], [0.8, top - 3.2], [0.4, top - 4.1], [-0.4, top - 4.1], [-0.8, top - 3.2]]);
      line(g, dark(stone, 0.2), 0.3, -0.3, top - 0.3, -0.4, top - 3.5);
      line(g, dark(stone, 0.2), 0.3, 0.3, top - 0.3, 0.4, top - 3.5);
      line(g, stone, 0.8, 0.8, top - 3.6, 1.6, top - 5.4);
      ell(g, lite(stone, 0.2), 1.6, top - 5.6, 0.45, 0.4);
      ball(g, 0, top - 4.9, 0.75, stone);
      line(g, dark(stone, 0.35), 0.3, -0.6, top - 4.9, 0.6, top - 5.0);
    });
  },

  sundial(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const stone = '#bdb7a8';
    shadow(S, K.rect(0.4), 4);
    K.contact(0.4);
    K.cyl(1, 1, 0.9, 0, 0.6, dark(stone, 0.1));
    K.cyl(1, 1, 0.55, 0.6, 2.4, stone, 0.45);
    K.cyl(1, 1, 0.95, 2.4, 2.9, lite(stone, 0.1));
    K.disc(1, 1, 0.8, '#c9a24a', 2.9);
    // Hour marks round the plate, and the blade that throws the shadow.
    for (let i = 0; i < 8; i++) {
      const t = (i / 8) * TAU;
      K.seg('#6b5320', 0.12, [1 + Math.cos(t) * 0.55, 1 + Math.sin(t) * 0.55, 2.92], [1 + Math.cos(t) * 0.78, 1 + Math.sin(t) * 0.78, 2.92]);
    }
    K.q('#a88334', [0, 1, 0], [0.5, 1, 2.9], [1.4, 1, 2.9], [1.4, 1, 4.3]);
    K.seg('rgba(60,50,40,.35)', 0.25, [1.4, 1, 2.92], [1.9, 1.6, 2.92]);
  },

  rock(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    shadow(S, K.rect(0.4), 3);
    const [x, y] = K.at(1, 1, 0);
    stone(K.g, x, y, 1.5, '#9a998f', item.x * 3 + item.y);
  },

  boulder(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g } = K;
    shadow(S, K.rect(0.4), 6);
    const [x, y] = K.at(2, 2, 0);
    const [x2, y2] = K.at(3.1, 3.1, 0);
    const [x3, y3] = K.at(0.8, 3.2, 0);
    // The big one in the middle, with a smaller one by its feet, and moss on top.
    stone(g, x, y - 0.6, 3.1, '#8f8e84', item.x * 5 + item.y);
    stone(g, x2, y2, 1.2, '#a3a298', item.x + item.y * 7);
    stone(g, x3, y3, 0.9, '#8a897f', item.x * 2 + item.y * 3);
    for (const [dx, dy, s] of [[-1.2, -3.4, 1], [0.6, -3.9, 1.3], [1.5, -3.1, 0.8]]) {
      ell(g, 'rgba(98,140,60,.85)', x + dx, y + dy, s * 1.2, s * 0.5);
    }
    ell(g, 'rgba(120,160,70,.7)', x - 0.3, y - 3.5, 1.2, 0.4);
  },

  log_pile(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    shadow(S, K.rect(0.3), 3);
    K.contact(0.6);
    K.parts([
      [2, 0.55, () => K.log(0.3, 0.55, 3.7, 0.55, 0, 0.52, '#7d5a38')],
      [2, 1.45, () => K.log(0.3, 1.45, 3.7, 1.45, 0, 0.52, '#8a6240')],
      [2, 1, () => K.log(0.4, 1, 3.6, 1, 0.9, 0.5, '#6f4e30'), 1],
    ]);
  },

  barrel(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    const wood = hexOf(item.color, '#9a6a3c');
    shadow(S, K.rect(0.5), 4);
    K.contact(0.4);
    K.cyl(1, 1, 0.82, 0, 1.8, wood, 0.98);
    K.cyl(1, 1, 0.98, 1.8, 3.6, wood, 0.82);
    const [x, y] = at(1, 1, 0);
    for (const [z, r] of [[0.7, 0.9], [2.9, 0.9], [1.8, 1.0]]) {
      strokeArc(g, '#3a3d4a', 0.35, x, y - z, r * 1.414, r * 0.707, 0, Math.PI);
    }
    // Staves.
    for (const dx of [-0.55, 0.05, 0.65]) line(g, rgba('#000000', 0.12), 0.18, x + dx, y - 3.5, x + dx * 1.05, y - 0.3);
    ell(g, dark(wood, 0.3), x, y - 3.6, 0.95 * 1.414 * 0.84, 0.95 * 0.707 * 0.84);
  },

  flag_pole(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const cloth = hexOf(item.color, '#e5484d');
    shadow(S, K.rect(0.5), 17);
    K.contact(0.3);
    K.cyl(0.4, 1, 0.5, 0, 0.7, '#6b6f7c');
    K.rod(0.4, 1, 0.5, 17.4, 0.5, '#d8dce6');
    K.ball(0.4, 1, 17.6, 0.55, '#d9b44a');
    // The flag ripples away from the pole in three folds.
    const waves = 6;
    for (let i = 0; i < waves; i++) {
      const a0 = 0.6 + (i / waves) * 2.2;
      const a1 = 0.6 + ((i + 1) / waves) * 2.2;
      const w0 = Math.sin((i / waves) * 5) * 0.6;
      const w1 = Math.sin(((i + 1) / waves) * 5) * 0.6;
      const bar = i % 2 ? 0.1 : -0.06;
      K.flat(tint(cloth, bar), [a0, 1, 17 + w0], [a1, 1, 17 + w1], [a1, 1, 13 + w1], [a0, 1, 13 + w0]);
      K.flat('#fff6e6', [a0, 1.01, 15.4 + w0], [a1, 1.01, 15.4 + w1], [a1, 1.01, 14.6 + w1], [a0, 1.01, 14.6 + w0]);
    }
  },

  bike(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const frame = hexOf(item.color, '#d94040');
    shadow(S, K.rect(0.3), 4);
    K.contact(0.4);
    const rear = [0.9, 1, 1.35];
    const front = [3.1, 1, 1.35];
    const bb = [1.7, 1, 1.15];
    const seat = [1.4, 1, 3.1];
    const head = [2.6, 1, 3.0];
    const tyre = '#2b2b33';
    K.circ(tyre, rear[0], rear[1], rear[2], 1.3, 'a', 1, 0.55);
    K.circ(tyre, front[0], front[1], front[2], 1.3, 'a', 1, 0.55);
    for (const c of [rear, front]) {
      K.circ('#aeb3bf', c[0], c[1], c[2], 1.0, 'a', 1, 0.12);
      for (let i = 0; i < 5; i++) {
        const t = (i / 5) * Math.PI;
        K.seg('#aeb3bf', 0.1, [c[0] - Math.cos(t), 1, c[2] - Math.sin(t)], [c[0] + Math.cos(t), 1, c[2] + Math.sin(t)]);
      }
    }
    K.seg(frame, 0.4, rear, bb);
    K.seg(frame, 0.4, rear, seat);
    K.seg(frame, 0.4, bb, seat);
    K.seg(frame, 0.45, seat, head);
    K.seg(frame, 0.45, bb, head);
    K.seg('#6b6f7c', 0.35, head, front);
    K.seg('#6b6f7c', 0.3, head, [2.45, 1, 3.7]);
    K.seg('#2a2a30', 0.35, [2.45, 0.35, 3.8], [2.45, 1.65, 3.8]);
    K.seg('#2a2a30', 0.5, [1.15, 1, 3.4], [1.65, 1, 3.4]);
    K.seg('#6b6f7c', 0.3, seat, [1.4, 1, 3.4]);
    K.seg('#2a2a30', 0.3, bb, [1.55, 1.25, 0.8]);
    K.seg('#2a2a30', 0.3, bb, [1.85, 0.75, 1.5]);
  },

  car(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const paint = hexOf(item.color, '#d9433f');
    const glass = '#8fc0dc';
    shadow(S, K.rect(0.3), 5);
    K.contact(0.9);
    K.box(0.3, 0.5, 7.7, 3.5, 0.9, 3.1, paint);
    // Bumpers, headlights at the +a end and tail lights at the other.
    K.q('#2b2d34', [1, 0, 0], [7.72, 0.5, 0.9], [7.72, 3.5, 0.9], [7.72, 3.5, 1.5], [7.72, 0.5, 1.5]);
    K.q('#2b2d34', [-1, 0, 0], [0.28, 0.5, 0.9], [0.28, 3.5, 0.9], [0.28, 3.5, 1.5], [0.28, 0.5, 1.5]);
    for (const b of [0.9, 2.6]) {
      K.q('#fff4c0', [1, 0, 0], [7.72, b, 1.9], [7.72, b + 0.6, 1.9], [7.72, b + 0.6, 2.6], [7.72, b, 2.6]);
      K.q('#d93a3a', [-1, 0, 0], [0.28, b, 1.9], [0.28, b + 0.6, 1.9], [0.28, b + 0.6, 2.6], [0.28, b, 2.6]);
    }
    // The cabin narrows to a roof; its glass is set into each side.
    const bot = { a0: 2.0, a1: 6.0, b0: 0.9, b1: 3.1, z: 3.1 };
    const top = { a0: 2.9, a1: 5.1, b0: 1.1, b1: 2.9, z: 4.9 };
    K.q(paint, [0, 1, 0.2], [bot.a0, bot.b1, bot.z], [bot.a1, bot.b1, bot.z], [top.a1, top.b1, top.z], [top.a0, top.b1, top.z]);
    K.q(paint, [0, -1, 0.2], [bot.a0, bot.b0, bot.z], [bot.a1, bot.b0, bot.z], [top.a1, top.b0, top.z], [top.a0, top.b0, top.z]);
    K.q(glass, [1, 0, 0.5], [bot.a1, bot.b0 + 0.15, bot.z], [bot.a1, bot.b1 - 0.15, bot.z], [top.a1, top.b1 - 0.1, top.z], [top.a1, top.b0 + 0.1, top.z]);
    K.q(glass, [-1, 0, 0.5], [bot.a0, bot.b0 + 0.15, bot.z], [bot.a0, bot.b1 - 0.15, bot.z], [top.a0, top.b1 - 0.1, top.z], [top.a0, top.b0 + 0.1, top.z]);
    K.q(glass, [0, 1, 0.2], [2.5, 3.0, 3.35], [3.75, 3.0, 3.35], [3.75, 2.92, 4.6], [3.1, 2.92, 4.6]);
    K.q(glass, [0, 1, 0.2], [4.05, 3.0, 3.35], [5.45, 3.0, 3.35], [4.95, 2.92, 4.6], [4.05, 2.92, 4.6]);
    K.q(glass, [0, -1, 0.2], [2.5, 1.0, 3.35], [3.75, 1.0, 3.35], [3.75, 1.08, 4.6], [3.1, 1.08, 4.6]);
    K.q(glass, [0, -1, 0.2], [4.05, 1.0, 3.35], [5.45, 1.0, 3.35], [4.95, 1.08, 4.6], [4.05, 1.08, 4.6]);
    K.q(paint, [0, 0, 1], [top.a0, top.b0, top.z], [top.a1, top.b0, top.z], [top.a1, top.b1, top.z], [top.a0, top.b1, top.z]);
    // Wheels on the sides that show.
    for (const a of [1.8, 6.2]) {
      for (const [b, n] of [[3.52, 1], [0.48, -1]]) {
        K.circ('#17181f', a, b, 1.0, 1.05, 'a', n);
        K.circ('#aeb3bf', a, b + 0.03 * n, 1.0, 0.5, 'a', n);
        K.circ('#6b6f7c', a, b + 0.05 * n, 1.0, 0.2, 'a', n);
      }
    }
    K.seg(lite(paint, 0.4), 0.2, [0.4, 3.52, 2.9], [7.6, 3.52, 2.9]);
  },

  boat(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const hull = hexOf(item.color, '#2f78d0');
    shadow(S, K.rect(0.3), 11);
    K.contact(0.9);
    const rim = [[0.4, 0.8], [4, 0.3], [7.8, 2], [4, 3.7], [0.4, 3.2]];
    const keel = [[1.3, 1.4], [4, 1.05], [7, 2], [4, 2.95], [1.3, 2.6]];
    const [ca, cb] = [4, 2];
    // The sides, with a white strip along the top, then the inside.
    for (let i = 0; i < rim.length; i++) {
      const j = (i + 1) % rim.length;
      const ea = rim[j][0] - rim[i][0];
      const eb = rim[j][1] - rim[i][1];
      const len = Math.hypot(ea, eb);
      let na = eb / len;
      let nb = -ea / len;
      if (na * ((rim[i][0] + rim[j][0]) / 2 - ca) + nb * ((rim[i][1] + rim[j][1]) / 2 - cb) < 0) {
        na = -na;
        nb = -nb;
      }
      K.q(hull, [na, nb, -0.2], [rim[i][0], rim[i][1], 2.7], [rim[j][0], rim[j][1], 2.7], [keel[j][0], keel[j][1], 0.5], [keel[i][0], keel[i][1], 0.5]);
      K.q('#f4f6fa', [na, nb, -0.2], [rim[i][0], rim[i][1], 2.7], [rim[j][0], rim[j][1], 2.7], [rim[j][0], rim[j][1], 2.2], [rim[i][0], rim[i][1], 2.2]);
    }
    K.flat('#8a5d33', ...rim.map(([a, b]) => [a, b, 2.7]));
    K.flat('#a97440', ...rim.map(([a, b]) => [a * 0.9 + 0.4, (b - 2) * 0.82 + 2, 2.7]));
    for (const a of [2.3, 4.6]) K.box(a - 0.25, 0.75, a + 0.25, 3.25, 1.7, 2.1, '#c9a068');
    // A short mast and a three-cornered sail.
    K.rod(4, 2, 2, 9.6, 0.4, '#c9b88a');
    K.flat('#fff6e6', [4.15, 2, 9.3], [4.15, 2, 3.8], [6.8, 2, 3.8]);
    K.flat('#e8dcc0', [3.85, 2, 8.6], [3.85, 2, 3.8], [1.9, 2, 3.8]);
    K.seg('#c9b88a', 0.3, [1.4, 2, 3.8], [7.4, 2, 3.8]);
  },

  doghouse(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const wall = hexOf(item.color, '#c9894a');
    const roof = mixHex(wall, '#7a2f2a', 0.55);
    shadow(S, K.rect(0.3), 8);
    K.contact(0.8);
    K.box(0.5, 0.6, 3.5, 3.4, 0, 3.2, wall);
    // Planks on the front, the gable above them, and the roof over the lot.
    for (const a of [1.0, 1.5, 2.5, 3.0]) K.seg(rgba('#000000', 0.12), 0.18, [a, 3.41, 0], [a, 3.41, 3.2]);
    K.q(wall, [0, 1, 0], [0.5, 3.4, 3.2], [3.5, 3.4, 3.2], [2, 3.4, 5.0]);
    K.q(wall, [0, -1, 0], [0.5, 0.6, 3.2], [3.5, 0.6, 3.2], [2, 0.6, 5.0]);
    K.circ('#241a14', 2, 3.42, 1.3, 1.0, 'a', 1);
    K.box(0.9, 3.38, 3.1, 3.46, 0, 0.3, dark(wall, 0.2));
    K.q(roof, [-0.7, 0, 0.7], [0.1, 0.3, 3.0], [0.1, 3.7, 3.0], [2, 3.7, 5.2], [2, 0.3, 5.2]);
    K.q(roof, [0.7, 0, 0.7], [3.9, 0.3, 3.0], [3.9, 3.7, 3.0], [2, 3.7, 5.2], [2, 0.3, 5.2]);
    K.q(dark(roof, 0.3), [0, 1, 0], [0.1, 3.7, 3.0], [2, 3.7, 5.2], [2, 3.7, 4.8], [0.1, 3.7, 2.7]);
    K.q(dark(roof, 0.3), [0, 1, 0], [3.9, 3.7, 3.0], [2, 3.7, 5.2], [2, 3.7, 4.8], [3.9, 3.7, 2.7]);
    // A bone-shaped plaque over the door, and a bowl on the grass.
    K.seg('#f4f6fa', 0.45, [1.5, 3.42, 3.9], [2.5, 3.42, 3.9]);
    K.ball(1.45, 3.42, 3.9, 0.3, '#f4f6fa');
    K.ball(2.55, 3.42, 3.9, 0.3, '#f4f6fa');
    K.cyl(3.4, 3.7, 0.28, 0, 0.35, '#d94040', 0.32);
  },

  // --- Seasonal ------------------------------------------------------------------------------

  pumpkin(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const [x, y] = at(1, 1, 0);
    const face = (col) => {
      // Two three-cornered eyes, a nose and a jagged grin.
      fp(g, col, [[x - 1.2, y - 2.3], [x - 0.4, y - 2.3], [x - 0.8, y - 3.1]]);
      fp(g, col, [[x + 0.4, y - 2.3], [x + 1.2, y - 2.3], [x + 0.8, y - 3.1]]);
      fp(g, col, [[x - 0.25, y - 1.9], [x + 0.25, y - 1.9], [x, y - 2.3]]);
      fp(g, col, [[x - 1.3, y - 1.5], [x - 0.8, y - 1.0], [x - 0.4, y - 1.4], [x, y - 0.9], [x + 0.4, y - 1.4], [x + 0.8, y - 1.0], [x + 1.3, y - 1.5], [x + 0.9, y - 0.5], [x - 0.9, y - 0.5]]);
    };
    if (!S.day) {
      pool(g, x, y, 6, '#ffb030', 0.35);
      glow(g, x, y - 1.6, 5.5, '#ffb030', 0.5);
      face('#ffe27a');
      return;
    }
    shadow(S, K.rect(0.5), 3);
    K.contact(0.3);
    ball(g, x - 1.1, y - 1.5, 1, '#d9701c', 1, 1.25);
    ball(g, x + 1.1, y - 1.5, 1, '#c9601a', 1, 1.25);
    ball(g, x, y - 1.7, 1.15, '#f08a28', 1.15, 1.45);
    line(g, 'rgba(120,50,10,.3)', 0.25, x - 0.55, y - 0.4, x - 0.5, y - 2.9);
    line(g, 'rgba(120,50,10,.3)', 0.25, x + 0.6, y - 0.4, x + 0.55, y - 2.9);
    g.beginPath();
    g.moveTo(x - 0.3, y - 3.1);
    g.lineTo(x - 0.1, y - 4.1);
    g.lineTo(x + 0.4, y - 4.1);
    g.lineTo(x + 0.3, y - 3.1);
    g.closePath();
    g.fillStyle = '#4f7a2a';
    g.fill();
    face('#3a1c0c');
  },

  gravestone(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    const stone = '#a9aab0';
    const R = 0.75;
    shadow(S, K.rect(0.4), 4);
    K.contact(0.4);
    const [mx, my] = at(1, 1.5, 0);
    ell(g, '#6b5a40', mx, my, 1.9, 0.75);
    // A slab with a rounded top: the front, then a strip for each bit of the curve.
    K.box(0.25, 0.7, 1.75, 1.3, 0, 3, stone);
    const arch = (t) => [1 + Math.cos(t) * R, 3 + Math.sin(t) * R];
    const steps = 8;
    for (let i = 0; i < steps; i++) {
      const t0 = (i / steps) * Math.PI;
      const t1 = ((i + 1) / steps) * Math.PI;
      const [a0, z0] = arch(t0);
      const [a1, z1] = arch(t1);
      const tm = (t0 + t1) / 2;
      K.q(stone, [Math.cos(tm), 0, Math.sin(tm)], [a0, 0.7, z0], [a1, 0.7, z1], [a1, 1.3, z1], [a0, 1.3, z0]);
    }
    const front = [[0.25, 1.3, 3]];
    for (let i = 0; i <= steps; i++) front.push([...arch((i / steps) * Math.PI).slice(0, 1), 1.3, arch((i / steps) * Math.PI)[1]]);
    front.push([1.75, 1.3, 3]);
    K.q(lite(stone, 0.06), [0, 1, 0], ...front);
    // A cross cut into it, and tufts at its foot.
    K.q(dark(stone, 0.45), [0, 1, 0], [0.9, 1.32, 1.1], [1.1, 1.32, 1.1], [1.1, 1.32, 3.1], [0.9, 1.32, 3.1]);
    K.q(dark(stone, 0.45), [0, 1, 0], [0.6, 1.32, 2.4], [1.4, 1.32, 2.4], [1.4, 1.32, 2.7], [0.6, 1.32, 2.7]);
    ell(g, 'rgba(98,140,60,.85)', ...at(0.5, 1.3, 0.3), 0.8, 0.35);
    ell(g, 'rgba(98,140,60,.85)', ...at(1.55, 1.35, 0.2), 0.55, 0.28);
  },

  skeleton(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    shadow(S, K.rect(0.5), 7);
    K.contact(0.3);
    K.figure(1, 1, () => {
      const g = K.g;
      const bone = '#efe8d4';
      const edge = '#5a5448';
      const bar = (x0, y0, x1, y1, w = 0.55) => {
        line(g, edge, w + 0.35, x0, y0, x1, y1);
        line(g, bone, w, x0, y0, x1, y1);
      };
      // Legs and feet, pelvis, spine, ribs, arms (one waving) and the skull.
      bar(-0.5, -3, -0.7, -0.3);
      bar(0.5, -3, 0.7, -0.3);
      rr(g, bone, -1.2, -0.4, 1, 0.4);
      rr(g, bone, 0.2, -0.4, 1, 0.4);
      ell(g, bone, 0, -3.2, 1, 0.55);
      bar(0, -3.2, 0, -5.8, 0.45);
      for (const y of [-4.1, -4.8, -5.4]) {
        const w = y === -4.8 ? 1.25 : 1.05;
        line(g, edge, 0.75, -w, y, w, y);
        line(g, bone, 0.4, -w, y, w, y);
      }
      bar(-1.1, -5.5, -1.7, -3.5, 0.4);
      bar(1.1, -5.5, 1.9, -7.3, 0.4);
      ball(g, 0, -7.0, 0.95, bone);
      rr(g, '#2a2a30', -0.6, -7.3, 0.45, 0.5);
      rr(g, '#2a2a30', 0.15, -7.3, 0.45, 0.5);
      rr(g, '#2a2a30', -0.15, -6.7, 0.3, 0.3);
      rr(g, bone, -0.5, -6.1, 1, 0.45);
      line(g, edge, 0.15, -0.25, -6.1, -0.25, -5.7);
      line(g, edge, 0.15, 0.25, -6.1, 0.25, -5.7);
    });
  },

  ghost(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const [x, y] = at(1, 1, 0);
    const shape = () => {
      g.beginPath();
      g.moveTo(x - 1.6, y - 1.5);
      g.bezierCurveTo(x - 1.9, y - 5.2, x - 1.2, y - 6.4, x, y - 6.4);
      g.bezierCurveTo(x + 1.2, y - 6.4, x + 1.9, y - 5.2, x + 1.6, y - 1.5);
      g.quadraticCurveTo(x + 1.2, y - 0.4, x + 0.8, y - 1.4);
      g.quadraticCurveTo(x + 0.4, y - 0.4, x, y - 1.4);
      g.quadraticCurveTo(x - 0.4, y - 0.4, x - 0.8, y - 1.4);
      g.quadraticCurveTo(x - 1.2, y - 0.4, x - 1.6, y - 1.5);
      g.closePath();
    };
    if (!S.day) {
      pool(g, x, y, 7, '#bfe0ff', 0.28);
      glow(g, x, y - 3.5, 6.5, '#bfe0ff', 0.5);
      shape();
      g.fillStyle = 'rgba(200,225,255,.45)';
      g.fill();
      return;
    }
    shadow(S, K.rect(0.5), 6);
    ell(g, 'rgba(30,24,50,.2)', x + 0.4, y + 0.2, 1.7, 0.6);
    shape();
    g.fillStyle = grad(g, x - 1.9, y - 6, x + 1.9, y - 1, '#ffffff', '#f2f5ff', '#c4cce6');
    g.fill();
    g.strokeStyle = 'rgba(70,80,130,.35)';
    g.lineWidth = 0.3;
    g.stroke();
    ell(g, '#2a2a40', x - 0.65, y - 4.4, 0.35, 0.55);
    ell(g, '#2a2a40', x + 0.65, y - 4.4, 0.35, 0.55);
    ell(g, '#2a2a40', x, y - 3.2, 0.3, 0.45);
    ell(g, 'rgba(255,170,190,.5)', x - 1.1, y - 3.5, 0.35, 0.22);
    ell(g, 'rgba(255,170,190,.5)', x + 1.1, y - 3.5, 0.35, 0.22);
    line(g, '#e8edfb', 0.9, x - 1.6, y - 3.7, x - 2.4, y - 3.1);
    line(g, '#e8edfb', 0.9, x + 1.6, y - 3.7, x + 2.4, y - 4.3);
  },

  cauldron(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const [x, y] = at(1, 1, 0);
    if (!S.day) {
      pool(g, x, y, 8, '#7bff5a', 0.3);
      glow(g, x, y - 3.4, 6.5, '#7bff5a', 0.6);
      ell(g, '#aaff8a', x, y - 3.4, 1.4, 0.6);
      for (const [dx, dy] of [[-0.6, -4.4], [0.7, -5.0], [0.1, -5.9]]) glow(g, x + dx, y + dy, 1.3, '#c9ffb0', 0.7);
      return;
    }
    shadow(S, K.rect(0.5), 4);
    K.contact(0.4);
    for (const dx of [-1.3, 0, 1.3]) rr(g, '#1c1b22', x + dx - 0.35, y - 1, 0.7, 1);
    // A fat black pot with a rim, a handle and a bubbling green brew in it.
    ball(g, x, y - 2.1, 1.9, '#2c2b33', 2.1, 1.6);
    g.strokeStyle = '#2c2b33';
    g.lineWidth = 0.45;
    g.beginPath();
    g.arc(x, y - 3.7, 2.15, 3.3, 6.1);
    g.stroke();
    ell(g, '#47454f', x, y - 3.5, 2.1, 0.95);
    ell(g, '#2a7a2a', x, y - 3.4, 1.7, 0.7);
    ell(g, '#7be05a', x - 0.1, y - 3.5, 1.45, 0.55);
    for (const [dx, dy, r] of [[-0.7, -4, 0.35], [0.6, -4.6, 0.45], [0.1, -5.5, 0.3]]) {
      ell(g, 'rgba(170,255,130,.9)', x + dx, y + dy, r, r);
      ell(g, 'rgba(255,255,255,.7)', x + dx - r * 0.3, y + dy - r * 0.3, r * 0.3, r * 0.3);
    }
  },

  snowman(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    shadow(S, K.rect(0.5), 8);
    K.contact(0.35);
    K.figure(1, 1, () => {
      const g = K.g;
      const snow = '#f2f6fc';
      // Arms of sticks, then three balls, with the scarf, hat and a carrot nose.
      line(g, '#6b4a2e', 0.4, -1, -3.7, -2.7, -4.8);
      line(g, '#6b4a2e', 0.4, 1, -3.7, 2.7, -5);
      line(g, '#6b4a2e', 0.25, 2.2, -4.6, 2.7, -4.2);
      ball(g, 0, -1.5, 1.6, snow);
      ball(g, 0, -3.7, 1.2, snow);
      ball(g, 0, -5.6, 0.95, snow);
      for (const y of [-3.9, -3.1]) ell(g, '#2a2a30', 0, y, 0.2, 0.2);
      ell(g, '#2a2a30', 0, -2.1, 0.2, 0.2);
      ell(g, '#2a2a30', -0.35, -5.8, 0.14, 0.14);
      ell(g, '#2a2a30', 0.35, -5.8, 0.14, 0.14);
      fp(g, '#f08a28', [[0, -5.5], [1.7, -5.3], [0, -5.2]]);
      rr(g, '#d94040', -1.1, -4.8, 2.2, 0.6);
      rr(g, '#d94040', 0.5, -4.8, 0.6, 1.6);
      ell(g, '#1c1b22', 0, -6.5, 1.3, 0.4);
      rr(g, '#1c1b22', -0.8, -8.2, 1.6, 1.7);
      ell(g, '#2d2c34', 0, -8.2, 0.8, 0.3);
      rr(g, '#d94040', -0.8, -7.0, 1.6, 0.3);
    });
  },

  christmas_tree(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const [x, y] = at(2, 2, 0);
    // The bulbs hang at fixed places on the three tiers, in four colours.
    const colours = ['#ff4a4a', '#ffd23a', '#4aa3f5', '#5bd16d'];
    const bulbs = [
      [-0.2, -4.6], [1.5, -3.4], [-2.2, -2.4], [2.9, -2.1], [-0.8, -6.7], [0.9, -8.2], [-1.4, -9.3], [0.2, -10.7],
      [-3.6, -1.6], [0.6, -5.7], [-1.6, -4.1], [2.2, -5.4],
    ];
    if (!S.day) {
      pool(g, x, y, 10, '#ffd23a', 0.2);
      bulbs.forEach(([dx, dy], i) => {
        glow(g, x + dx, y + dy, 3.2, colours[i % 4], 0.6);
        ell(g, lite(colours[i % 4], 0.5), x + dx, y + dy, 0.4, 0.4);
      });
      glow(g, x, y - 15.6, 4.5, '#fff0a0', 0.7);
      return;
    }
    shadow(S, K.rect(0.4), 15);
    K.contact(0.6);
    rr(g, '#5a3d26', x - 0.7, y - 2, 1.4, 2);
    const tier = (yb, w, h, col) => {
      g.beginPath();
      g.moveTo(x - w, yb);
      g.lineTo(x, yb - h);
      g.lineTo(x + w, yb);
      g.quadraticCurveTo(x + w * 0.5, yb + 0.9, x, yb + 0.3);
      g.quadraticCurveTo(x - w * 0.5, yb + 0.9, x - w, yb);
      g.closePath();
      g.fillStyle = grad(g, x - w, 0, x + w, 0, lite(col, 0.18), col, dark(col, 0.32));
      g.fill();
    };
    tier(y - 1.4, 4.1, 5.2, '#1f6b3a');
    tier(y - 4.6, 3.2, 4.7, '#25783f');
    tier(y - 7.8, 2.3, 4.4, '#2b8446');
    // Gold tinsel in two swags.
    strokeArc(g, '#f2d04a', 0.4, x, y - 4.4, 3.2, 1.1, 0.2, 2.95);
    strokeArc(g, '#f2d04a', 0.4, x, y - 7.4, 2.2, 0.8, 0.2, 2.95);
    bulbs.forEach(([dx, dy], i) => {
      ell(g, colours[i % 4], x + dx, y + dy, 0.45, 0.45);
      ell(g, 'rgba(255,255,255,.7)', x + dx - 0.15, y + dy - 0.15, 0.14, 0.14);
    });
    // The star.
    const sx = x;
    const sy = y - 13.1;
    const star = [];
    for (let i = 0; i < 10; i++) {
      const t = -1.5708 + (i / 10) * TAU;
      const r = i % 2 ? 0.7 : 1.5;
      star.push([sx + Math.cos(t) * r, sy + Math.sin(t) * r]);
    }
    fp(g, '#ffd84a', star);
  },

  gift_pile(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const main = hexOf(item.color, '#d94040');
    shadow(S, K.rect(0.2), 4);
    K.contact(0.6);
    K.parts([
      [1.5, 0.7, () => gift(K, 1.1, 0.1, 1.95, 1.0, 0, 1.0, '#2f78d0', '#f4f6fa')],
      [0.7, 1.4, () => gift(K, 0.05, 0.8, 1.25, 1.95, 0, 1.5, main, '#f2d04a')],
      [0.7, 1.4, () => gift(K, 0.3, 1.1, 0.95, 1.75, 1.5, 2.2, '#2a9560', '#d94040'), 1],
      [1.6, 1.6, () => gift(K, 1.35, 1.3, 1.95, 1.95, 0, 0.7, '#f2c230', '#d94040')],
    ]);
  },

  hay_bale(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const straw = '#d8b050';
    shadow(S, K.rect(0.3), 3);
    K.contact(0.8);
    // The room's bale (web/room.js): square bales tied with twine, two side by side and one on top, with
    // straw sticking out at the edges. A round roll read as a pair of logs at this size.
    const twine = '#8a5d33';
    const bale = (a0, a1, z0, z1) => {
      K.box(a0, 0.15, a1, 1.85, z0, z1, straw);
      for (const t of [0.27, 0.73]) {
        const x = a0 + (a1 - a0) * t;
        K.seg(twine, 0.2, [x, 0.15, z1], [x, 1.85, z1]);
        K.seg(twine, 0.2, [x, 1.85, z1], [x, 1.85, z0]);
        K.seg(twine, 0.2, [x, 0.15, z1], [x, 0.15, z0]);
      }
      for (const [b, d] of [[0.5, 0.2], [0.95, -0.15], [1.4, 0.1]]) K.seg(dark(straw, 0.2), 0.14, [a0 + 0.2, b, z1], [a1 - 0.2, b + d * 0.3, z1]);
      for (const [b, z] of [[1.85, 0.3], [1.85, 0.8]]) K.seg(lite(straw, 0.4), 0.14, [a0 + 0.15, b, z0 + z], [a1 - 0.15, b, z0 + z + 0.05]);
      K.seg(lite(straw, 0.45), 0.16, [a1, 0.3, z1], [a1 + 0.2, 0.2, z1 + 0.35]);
      K.seg(lite(straw, 0.45), 0.16, [a1, 1.5, z1 - 0.2], [a1 + 0.2, 1.65, z1 + 0.1]);
    };
    K.parts([
      [1, 1, () => bale(0.1, 1.95, 0, 1.4)],
      [3, 1, () => bale(2.05, 3.9, 0, 1.4)],
      [2, 1, () => bale(1.1, 2.95, 1.4, 2.8), 1],
    ]);
    // A few loose stalks at the front.
    for (const [a, b, z] of [[0.6, 1.95, 0], [2.4, 1.97, 0], [3.4, 1.95, 0]]) K.seg(lite(straw, 0.3), 0.2, [a - 0.2, b, z], [a + 0.3, b + 0.25, z + 0.1]);
  },

  rake(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    shadow(S, K.rect(0.2), 3);
    K.contact(0.5);
    const iron = '#6d6a62';
    // The handle leans back from the head, which rests in the leaves.
    K.seg(dark(FALL_WOOD[3], 0.1), 0.55, [0.9, 1.5, 0.5], [1.5, 0.5, 7.4]);
    K.seg(lite(FALL_WOOD[3], 0.2), 0.2, [0.9, 1.5, 0.6], [1.5, 0.5, 7.4]);
    K.seg(iron, 0.5, [0.2, 1.7, 0.35], [1.3, 1.3, 0.35]);
    for (const t of [0.2, 0.45, 0.7, 0.95]) K.seg(iron, 0.3, [0.2 + t, 1.7 - t * 0.35, 0.35], [0.1 + t, 1.9 - t * 0.35, 0]);
    // The heap it has gathered.
    const [x, y] = at(1.2, 1.2, 0);
    ell(g, 'rgba(30,24,50,.18)', x + 0.3, y + 0.3, 3, 1.4);
    const cols = FALL_LEAVES;
    ell(g, '#b8651c', x, y - 0.6, 2.6, 1.1);
    ell(g, '#d9902a', x - 0.3, y - 1.0, 1.8, 0.8);
    for (let i = 0; i < 16; i++) {
      const t = i / 16;
      const lx = x + (((i * 37) % 11) - 5) * 0.42;
      const ly = y - 0.4 - (1 - Math.abs(lx - x) / 2.4) * 1.3 - (i % 3) * 0.3;
      leaf(g, lx, ly, 0.9, 0.5, cols[(i * 3) % 8], t * 5);
    }
    for (const [dx, dy, c] of [[2.6, 0.7, 1], [-2.7, 0.9, 4], [1.3, 1.7, 2], [3.2, 0.1, 0], [-1.4, 1.5, 5]]) leaf(g, x + dx, y + dy, 0.7, 0.4, cols[c], dx);
  },

  harvest_table(S, item) {
    const K = kit(S, item);
    const { g, at } = K;
    const top = 5;
    const wood = FALL_WOOD[2];
    const flames = [[2, 2], [6, 2]];
    if (!S.day) {
      for (const [a, b] of flames) {
        const [x, y] = at(a, b, top + 1.8);
        pool(g, x, y + 1.5, 7, '#ffb347', 0.4);
        glow(g, x, y - 0.4, 4.2, '#ffc766', 0.7);
        flame(g, x, y + 0.9, 1.9, 0.5);
      }
      return;
    }
    shadow(S, K.rect(0.2), 5);
    K.contact(0.5);
    const bench = (b0) => () => {
      for (const a of [0.8, 7.2]) K.box(a - 0.25, b0, a + 0.25, b0 + 0.6, 0, 2.3, FALL_WOOD[1]);
      K.box(0.3, b0 - 0.05, 7.7, b0 + 0.65, 2.3, 2.85, FALL_WOOD[3]);
    };
    const leg = (a, b) => () => K.box(a - 0.3, b - 0.3, a + 0.3, b + 0.3, 0, top - 0.5, FALL_WOOD[1]);
    K.parts([
      [4, 0.4, bench(0.15)],
      [0.5, 1.2, leg(0.6, 1.25)],
      [7.5, 1.2, leg(7.4, 1.25)],
      [0.5, 2.8, leg(0.6, 2.75)],
      [7.5, 2.8, leg(7.4, 2.75)],
      [4, 3.6, bench(3.25)],
      [4, 2, () => {
        K.box(0.1, 0.95, 7.9, 3.05, top - 0.6, top, wood);
        for (const b of [1.5, 2, 2.5]) K.seg(dark(wood, 0.25), 0.15, [0.1, b, top], [7.9, b, top]);
        // The runner: cream cloth with a gold edge and russet stripes at the ends, hanging a little over.
        K.flat(FALL_CREAM, [0.4, 1.35, top + 0.02], [7.6, 1.35, top + 0.02], [7.6, 2.65, top + 0.02], [0.4, 2.65, top + 0.02]);
        K.flat('#d9a520', [0.4, 1.35, top + 0.03], [7.6, 1.35, top + 0.03], [7.6, 1.5, top + 0.03], [0.4, 1.5, top + 0.03]);
        K.flat('#d9a520', [0.4, 2.5, top + 0.03], [7.6, 2.5, top + 0.03], [7.6, 2.65, top + 0.03], [0.4, 2.65, top + 0.03]);
        for (const [a0, a1] of [[0.55, 1.1], [6.9, 7.45]]) K.flat('#b33a1f', [a0, 1.55, top + 0.04], [a1, 1.55, top + 0.04], [a1, 2.45, top + 0.04], [a0, 2.45, top + 0.04]);
        K.q(FALL_CREAM, [0, 1, 0], [0.4, 2.65, top], [7.6, 2.65, top], [7.6, 2.65, top - 0.9], [0.4, 2.65, top - 0.9]);
      }, 1],
      // Plates and napkins along the near and far sides.
      ...[1.7, 4, 6.3].flatMap((a) => [1.1, 2.9].map((b) => [a, b, () => {
        K.disc(a, b, 0.55, '#f1ead8', top + 0.04);
        K.disc(a, b, 0.32, '#d8c7a3', top + 0.06);
        K.disc(a - 0.6, b, 0.2, '#b33a1f', top + 0.06);
      }, 2])),
      [2.9, 2.1, () => {
        // A bowl of apples.
        const [x, y] = at(2.9, 2.1, top);
        ell(g, '#8a5c37', x, y, 1.5, 0.75);
        apple(g, x - 0.7, y - 0.6, 0.65, '#b33a1f');
        apple(g, x + 0.6, y - 0.5, 0.65, '#d4502a');
        apple(g, x, y - 1.1, 0.65, '#a3a84a');
      }, 3],
      [5.2, 2, () => {
        const [x, y] = at(5.2, 2, top);
        squash(g, x - 0.4, y, 0.7, 0.55, '#7a3a66', '#4a3220');
        squash(g, x + 0.5, y - 0.1, 0.55, 0.45, '#d9a520');
      }, 3],
      [4, 2, () => {
        // The pumpkin in the middle, with leaves round it.
        const [x, y] = at(4, 2, top);
        for (let i = 0; i < 6; i++) leaf(g, x + Math.cos(i * 1.05) * 1.9, y + 0.1 + Math.sin(i * 1.05) * 0.8, 0.7, 0.35, FALL_LEAVES[(i * 3 + 1) % 8], i * 1.3);
        squash(g, x, y, 1.45, 1.2, '#e8802a');
      }, 4],
      ...flames.map(([a, b]) => [a, b, () => {
        // A candle in a little brass holder; its flame is for the night.
        const [x, y] = at(a, b, top);
        ell(g, '#c9a24a', x, y, 0.75, 0.38);
        K.rod(a, b, top, top + 1.6, 0.8, FALL_CREAM);
        const [fx, fy] = at(a, b, top + 1.6);
        ell(g, '#d8c7a3', fx, fy, 0.4, 0.2);
        line(g, '#2a211d', 0.15, fx, fy, fx, fy - 0.5);
      }, 5]),
    ]);
  },

  pumpkin_pie(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    shadow(S, K.rect(0.3), 3);
    K.contact(0.4);
    // A round stump to stand it on, with its cut top and rings.
    K.cyl(1, 1, 0.9, 0, 1.2, FALL_WOOD[2], 0.85);
    const [x, y] = at(1, 1, 1.2);
    ell(g, FALL_WOOD[3], x, y, 0.85 * 1.414, 0.85 * 0.707);
    strokeArc(g, FALL_WOOD[1], 0.15, x, y, 0.5 * 1.414, 0.5 * 0.707, 0, TAU);
    strokeArc(g, FALL_WOOD[1], 0.15, x, y, 0.22 * 1.414, 0.22 * 0.707, 0, TAU);
    // The pie: a crust, the filling, and a slice taken from the front.
    K.cyl(1, 1, 0.98, 1.2, 2.0, '#d9a24a');
    const z = 2.0;
    K.flat('#e08a2a', ...Array.from({ length: 14 }, (_, i) => [1 + Math.cos((i / 14) * TAU) * 0.82, 1 + Math.sin((i / 14) * TAU) * 0.82, z + 0.02]));
    K.flat('#f0a03c', ...Array.from({ length: 8 }, (_, i) => [0.8 + Math.cos((i / 8) * TAU) * 0.4, 0.8 + Math.sin((i / 8) * TAU) * 0.4, z + 0.03]));
    // The gap, showing the dark tin and the cut edge of the filling.
    K.flat('#6b4628', [1, 1, z + 0.04], [1.95, 0.6, z + 0.04], [1.9, 1.7, z + 0.04]);
    K.flat('#e08a2a', [1, 1, z + 0.05], [1.6, 0.75, z + 0.05], [1.58, 1.4, z + 0.05]);
    K.seg('#c8641e', 0.2, [1, 1, z], [1.9, 0.62, z]);
    K.seg('#c8641e', 0.2, [1, 1, z], [1.85, 1.68, z]);
    for (const t of [0.15, 0.35, 0.55, 0.75, 0.95]) {
      const [px, py] = at(1 + Math.cos(t * 6.2) * 0.9, 1 + Math.sin(t * 6.2) * 0.9, z - 0.1);
      ell(g, '#b8870f', px, py, 0.2, 0.2);
    }
    // A dollop of cream.
    const [cx, cy] = at(0.7, 0.9, z + 0.1);
    ball(g, cx, cy - 0.3, 0.5, FALL_CREAM, 0.6, 0.45);
  },

  cornucopia(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    shadow(S, K.rect(0.2), 3);
    K.contact(0.7);
    const [mu, mv] = K.dir(1, 0);
    const mouthSeen = mu + mv > 0;
    K.parts([
      [1.2, 1, () => {
        // A woven horn lying with its tip curled up at the back and its mouth to the right.
        const N = 9;
        for (let i = 0; i < N; i++) {
          const t = i / (N - 1);
          const w = 0.9 + t * 2.3;
          const a = 0.3 + t * 1.9;
          const b = 0.5 + t * 0.5 + Math.sin(t * 3) * 0.2;
          const z = 0.3 + w * 0.3 + (1 - t) * (1 - t) * 2.6;
          const [x, y] = at(a, b, z);
          ell(g, FALL_STRAW[i % 2 ? 0 : 1], x, y, w * 0.75, w * 0.55);
          ell(g, FALL_STRAW[2], x - w * 0.12, y - w * 0.16, w * 0.4, w * 0.2);
          line(g, FALL_STRAW[0], 0.2, x - w * 0.4, y - w * 0.35, x + w * 0.1, y + w * 0.5);
        }
        const [x, y] = at(2.2, 1, 0.3 + 3.2 * 0.3);
        if (mouthSeen) ell(g, FALL_WOOD[0], x + 0.2, y, 1.3, 1.1);
        else {
          // Turned away, the mouth still opens upward: a woven rim with the dark inside seen from above.
          ell(g, FALL_STRAW[0], x, y, 1.4, 1.2);
          ell(g, FALL_STRAW[2], x, y - 0.1, 1.25, 1.05);
          ell(g, FALL_WOOD[0], x, y - 0.35, 1.0, 0.7);
          ell(g, FALL_WOOD[1], x, y - 0.1, 0.8, 0.4);
        }
      }],
      // What spills from it.
      [2.5, 0.7, () => { const [x, y] = at(2.6, 0.7, 0); squash(g, x, y, 0.95, 0.8, '#e8802a'); }],
      [3, 1.3, () => { const [x, y] = at(3.1, 1.3, 0.8); apple(g, x, y, 0.7, '#b33a1f'); }],
      [3.5, 0.9, () => { const [x, y] = at(3.5, 0.8, 0); squash(g, x, y, 0.7, 0.6, '#d9a520'); }],
      [3.3, 1.6, () => { const [x, y] = at(3.4, 1.65, 0); squash(g, x, y, 0.65, 0.5, '#7f8c3a', '#4a3220'); }],
      [2.8, 1.7, () => { const [x, y] = at(2.8, 1.7, 0.4); for (const [dx, dy] of [[-0.4, 0], [0.3, -0.1], [0, -0.6]]) ell(g, '#7a3a66', x + dx, y + dy, 0.4, 0.4); }, 1],
      [3.6, 1.8, () => { const [x, y] = at(3.65, 1.8, 0.7); apple(g, x, y, 0.6, '#d4502a'); }, 1],
    ]);
  },

  apple_basket(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    shadow(S, K.rect(0.3), 3);
    K.contact(0.4);
    const wick = FALL_STRAW[1];
    // The handle, over the top.
    K.seg(FALL_STRAW[0], 0.45, [0.25, 1, 1.7], [0.45, 1, 3.4]);
    K.seg(FALL_STRAW[0], 0.45, [0.45, 1, 3.4], [1.55, 1, 3.4]);
    K.seg(FALL_STRAW[0], 0.45, [1.55, 1, 3.4], [1.75, 1, 1.7]);
    K.cyl(1, 1, 0.7, 0, 1.9, wick, 0.88);
    const [x, y] = at(1, 1, 0);
    for (const z of [0.5, 1.0, 1.5]) strokeArc(g, rgba(FALL_STRAW[0], 0.85), 0.25, x, y - z, (0.7 + z * 0.1) * 1.414, (0.7 + z * 0.1) * 0.707, 0, Math.PI);
    for (const dx of [-0.6, 0, 0.6]) line(g, rgba(FALL_STRAW[0], 0.6), 0.2, x + dx, y - 1.8, x + dx * 0.9, y - 0.1);
    ell(g, FALL_STRAW[0], x, y - 1.9, 0.88 * 1.414, 0.88 * 0.707);
    ell(g, '#7a4a2a', x, y - 1.95, 0.8 * 1.414, 0.8 * 0.707);
    for (const [dx, dy, r, c] of [[-0.8, -2.3, 0.7, '#b33a1f'], [0.7, -2.3, 0.7, '#d4502a'], [0, -2.1, 0.7, '#8c2a1e'], [-0.3, -3.0, 0.7, '#d4502a'], [0.5, -3.0, 0.65, '#a3a84a']]) {
      apple(g, x + dx, y + dy, r, c);
    }
    leaf(g, x + 0.2, y - 3.7, 0.55, 0.3, '#5f8a2e', 0.6);
    const [ax, ay] = at(1.9, 1.9, 0.6);
    apple(g, ax, ay, 0.6, '#b33a1f');
  },

  cider_barrel(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    const [du, dv] = K.dir(1, 0);
    const nearEnd = du + dv > 0 ? 3.4 : 0.6;
    shadow(S, K.rect(0.2), 4);
    K.contact(0.7);
    const r = 1.0;
    // A cradle under it, then the barrel on its side along the length.
    K.parts([
      [1.2, 1, () => K.box(0.9, 0.3, 1.5, 1.7, 0, 1.0, FALL_WOOD[1])],
      [2.8, 1, () => K.box(2.5, 0.3, 3.1, 1.7, 0, 1.0, FALL_WOOD[1])],
      [2, 1, () => {
        K.log(0.6, 1, 3.4, 1, 0.55, r, FALL_WOOD[3], false);
        // Hoops round it, and staves along it.
        for (const a of [0.95, 1.9, 2.15, 3.05]) {
          const [x, y] = at(a, 1, r);
          line(g, '#3a3d4a', 0.3, x + r * 0.45, y - r * 0.9, x - r * 0.45, y + r * 0.9);
        }
        for (const z of [0.9, 1.4]) K.seg(rgba('#000000', 0.12), 0.15, [0.7, 1, z + 0.55], [3.3, 1, z + 0.55]);
      }, 1],
      [nearEnd, 1, () => {
        // The tap on the end that faces you, and a drip in a mug.
        const sign = nearEnd > 2 ? 1 : -1;
        const [x, y] = at(nearEnd + sign * 0.1, 1, 0.9);
        line(g, '#d9a520', 0.55, x, y, x + sign * 0.2, y + 0.4);
        line(g, '#b8870f', 0.4, x, y + 0.1, x + sign * 0.8, y + 0.1);
        ell(g, '#f0a03c', x + sign * 0.8, y + 0.7, 0.18, 0.28);
        const [mx, my] = at(nearEnd + sign * 0.5, 1.6, 0);
        ell(g, '#8a5c37', mx, my - 0.5, 0.55, 0.5);
        ell(g, '#f0a03c', mx, my - 0.9, 0.4, 0.2);
      }, 2],
    ]);
    // A red apple on the top.
    const [px, py] = at(2, 1, r * 2 + 0.3);
    apple(g, px, py, 0.55, '#b33a1f');
  },

  gourds(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    shadow(S, K.rect(0.2), 3);
    K.contact(0.8);
    // A little straw underneath, then the heap from the back.
    K.parts([
      [2, 1, () => {
        const [x, y] = at(2, 1, 0);
        for (let i = 0; i < 12; i++) line(g, FALL_STRAW[i % 3], 0.25, x - 3.4 + i * 0.6, y + 0.5, x - 3.5 + i * 0.6, y - 0.1 - (i % 3) * 0.2);
      }, -1],
      [2.4, 0.5, () => { const [x, y] = at(2.4, 0.5, 0); squash(g, x, y, 1.45, 1.2, '#e8802a'); }],
      [1, 0.8, () => { const [x, y] = at(0.9, 0.8, 0); squash(g, x, y, 1, 0.85, '#d9701c'); }],
      [3.3, 0.7, () => { const [x, y] = at(3.3, 0.7, 0); squash(g, x, y, 0.85, 1.0, '#7f8c3a', '#4a3220'); }],
      [1.9, 1.3, () => { const [x, y] = at(1.9, 1.4, 0); squash(g, x, y, 0.95, 0.75, FALL_CREAM, '#7f8c3a'); }],
      [0.8, 1.5, () => { const [x, y] = at(0.7, 1.5, 0); squash(g, x, y, 0.75, 0.6, '#d9a520'); }],
      [3.1, 1.5, () => {
        // A striped gourd with a crooked neck.
        const [x, y] = at(3.1, 1.5, 0);
        ball(g, x, y - 0.7, 0.9, '#d9a520', 0.9, 0.7);
        for (const dx of [-0.4, 0, 0.4]) line(g, '#7f8c3a', 0.2, x + dx, y - 1.3, x + dx * 1.1, y - 0.2);
        ell(g, '#d9a520', x + 0.3, y - 1.8, 0.4, 0.6);
        line(g, '#7f8c3a', 0.3, x + 0.4, y - 2.3, x + 0.6, y - 2.6);
      }],
      [2.5, 1.7, () => { const [x, y] = at(2.5, 1.75, 0); squash(g, x, y, 0.55, 0.45, '#e8802a'); }],
    ]);
  },

  turkey(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g } = K;
    shadow(S, K.rect(0.3), 4);
    K.contact(0.5);
    const bronze = '#7a4a24';
    const tan = '#e3c987';
    K.figure(1, 1, () => {
      // The tail fanned out behind, in bands, with a tan tip on every feather.
      for (let i = 0; i < 11; i++) {
        const a = -3.14 + (i + 0.5) * 0.28;
        const col = i % 2 ? '#8c4a20' : '#5b3a20';
        g.save();
        g.translate(0, -3.6);
        g.rotate(a);
        ell(g, col, 3.4, 0, 3.2, 0.78);
        ell(g, '#c8641e', 4.1, 0, 1.6, 0.6);
        ell(g, tan, 5.5, 0, 1.3, 0.7);
        g.restore();
      }
      for (const s of [-1, 1]) {
        line(g, '#c46a3a', 0.45, s * 0.9, -1.5, s * 1, -0.1);
        line(g, '#c46a3a', 0.35, s * 1, -0.1, s * 1.9, 0.1);
        line(g, '#c46a3a', 0.35, s * 1, -0.1, s * 0.2, 0.15);
      }
      // A plump body, a lighter breast, wings folded at the sides.
      ball(g, 0, -3.1, 2.4, bronze, 2.4, 2.5);
      ell(g, '#9a6234', 0, -2.6, 1.5, 1.7);
      for (const [x, y] of [[-0.7, -3], [0.6, -3.1], [0, -2.1], [-0.8, -2], [0.8, -2.1]]) ell(g, tan, x, y, 0.5, 0.28);
      for (const s of [-1, 1]) {
        ell(g, '#5b3a20', s * 2, -3, 0.75, 1.7);
        line(g, tan, 0.2, s * 2.2, -3.6, s * 2.1, -1.9);
      }
      // The head, bare and blue, with a red wattle and snood.
      line(g, '#6f98bd', 1, 0, -5, 0, -6.3);
      ball(g, 0, -7, 0.85, '#8fb2cf', 0.85, 0.9);
      ell(g, '#d4402e', 0.1, -5.8, 0.4, 0.8);
      ell(g, '#d4402e', 0.1, -8, 0.3, 0.5);
      fp(g, '#e8b040', [[-0.2, -6.8], [0.2, -6.8], [0, -6.2]]);
      ell(g, '#1b1b1b', -0.4, -7.2, 0.15, 0.15);
      ell(g, '#1b1b1b', 0.4, -7.2, 0.15, 0.15);
    }, 0.55);
  },

  wreath(S, item) {
    if (!S.day) return;
    const K = kit(S, item);
    const { g, at } = K;
    shadow(S, K.rect(0.4), 3);
    K.contact(0.4);
    // A little easel of wood to stand it on.
    const wood = FALL_WOOD[2];
    K.seg(dark(wood, 0.2), 0.55, [1, 1.5, 0], [1, 1.1, 5.6]);
    K.seg(wood, 0.6, [0.35, 0.8, 0], [1, 1.1, 5.6]);
    K.seg(wood, 0.6, [1.65, 0.8, 0], [1, 1.1, 5.6]);
    K.seg(dark(wood, 0.1), 0.45, [0.55, 0.9, 1], [1.45, 0.9, 1]);
    // The ring of turning leaves, seen from either side.
    const cz = 3.9;
    const R = 1.75;
    for (let i = 0; i < 18; i++) {
      const t = (i / 18) * TAU;
      const [x, y] = at(1 + Math.cos(t) * R, 1.15, cz + Math.sin(t) * R);
      leaf(g, x, y, 0.8, 0.42, FALL_LEAVES[(i * 3) % 8], t + 1.57);
    }
    for (let i = 0; i < 12; i++) {
      const t = (i / 12) * TAU + 0.26;
      const [x, y] = at(1 + Math.cos(t) * R * 0.98, 1.2, cz + Math.sin(t) * R * 0.98);
      leaf(g, x, y, 0.6, 0.32, FALL_LEAVES[(i * 5 + 2) % 8], t + 1);
    }
    // Berries, a tiny gourd and a bow.
    for (const [t, c] of [[0.6, '#7a3a66'], [1.0, '#b33a1f'], [4.1, '#7a3a66']]) {
      const [x, y] = at(1 + Math.cos(t) * R, 1.25, cz + Math.sin(t) * R);
      ell(g, c, x, y, 0.38, 0.38);
    }
    const [gx, gy] = at(1 + Math.cos(3.7) * R, 1.25, cz + Math.sin(3.7) * R);
    ball(g, gx, gy, 0.6, '#e8802a', 0.6, 0.5);
    const [bx, by] = at(1, 1.3, cz - R);
    for (const s of [-1, 1]) leaf(g, bx + s * 0.8, by, 0.8, 0.4, '#b33a1f', s * -0.4);
    ell(g, '#8c2a1e', bx, by, 0.35, 0.35);
    line(g, '#b33a1f', 0.3, bx, by, bx - 0.4, by + 1.1);
    line(g, '#b33a1f', 0.3, bx, by, bx + 0.4, by + 1.1);
  },
};
