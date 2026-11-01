// A place's ground, fence and plants, from what its owner chose (docs/outside.md).

import {
  P, bench, boulder, canopy, dark, disc, drawTree, ell, flowerBunch, fountain, fp, fp3, grad, grain, ih, lerp, lite, line, mixHex, onL, onR, paving,
  rgba, rr, seeded, shadow,
} from './world.js';
import { doorPoint } from './outside-building.js';
import { hexOf, itemsOf, partOf, rectOf, styleOf, numberOf } from './outside-spec.js';

// ---------------------------------------------------------------------------------------------
// Small tools. Nothing from world.js is called while this file loads, only inside functions.

const BLEED = 1.6;
const TAU = 6.2832;
const STONE = 'rgba(25,25,35,.28)';

// A polygon of screen points.
const quad = (g, fill, ...pts) => fp(g, fill, pts);

// Seeded randomness that depends on the sprite's seed and a salt.
const randOf = (S, salt = 0) => seeded(ih(S.seed, salt, 91) + salt);

const tint = (c, rand, k = 0.16) => mixHex(c, rand() < 0.5 ? '#ffffff' : '#2c2438', rand() * k);

function stroke(g, color, w, pts) {
  g.strokeStyle = color;
  g.lineWidth = w;
  g.beginPath();
  g.moveTo(pts[0][0], pts[0][1]);
  for (let i = 1; i < pts.length; i++) g.lineTo(pts[i][0], pts[i][1]);
  g.stroke();
}

// A soft shadow under something standing on the ground.
function pool(g, u, v, r, a = 0.3) {
  disc(g, `rgba(25,38,20,${a})`, u + r * 0.25, v + r * 0.25, r);
}

// A box: its two faces you can see and its top. `detailL` and `detailR` paint into each face's plane.
function solid(g, R, z0, z1, color, o = {}) {
  const [u0, v0, u1, v1] = R;
  fp3(g, o.left || color, [u0, v1, z0], [u1, v1, z0], [u1, v1, z1], [u0, v1, z1]);
  fp3(g, o.right || dark(color, 0.26), [u1, v1, z0], [u1, v0, z0], [u1, v0, z1], [u1, v1, z1]);
  fp3(g, o.top || lite(color, 0.16), [u0, v0, z1], [u1, v0, z1], [u1, v1, z1], [u0, v1, z1]);
  if (o.detailL) {
    onL(g, u0, v1, () => o.detailL(u1 - u0));
  }
  if (o.detailR) {
    onR(g, u1, v1, () => o.detailR(v1 - v0));
  }
}

// A round post or drum standing on the ground, with the light from the left.
function drum(g, u, v, r, z0, z1, color, topColor) {
  const [x, y] = P(u, v);
  const rx = r * 1.414;
  const ry = r * 0.707;
  g.fillStyle = grad(g, x - rx, 0, x + rx, 0, lite(color, 0.2), color, dark(color, 0.32));
  g.fillRect(x - rx, y - z1, rx * 2, z1 - z0);
  ell(g, dark(color, 0.32), x, y - z0, rx, ry);
  g.fillStyle = grad(g, x - rx, 0, x + rx, 0, lite(color, 0.2), color, dark(color, 0.32));
  g.fillRect(x - rx, y - z1, rx * 2, z1 - z0 + 0.1);
  ell(g, topColor || lite(color, 0.24), x, y - z1, rx, ry);
}

// A gable roof over a box, whose ridge runs along `axis`. It overhangs by `o`.
function gableRoof(g, R, z, rise, roof, wall, axis, o = 0.8) {
  const [u0, v0, u1, v1] = R;
  if (axis === 'u') {
    const vm = (v0 + v1) / 2;
    fp3(g, wall, [u1, v0, z], [u1, v1, z], [u1, vm, z + rise]);
    fp3(g, dark(roof, 0.12), [u0 - o, v0 - o, z], [u1 + o, v0 - o, z], [u1 + o, vm, z + rise], [u0 - o, vm, z + rise]);
    fp3(g, roof, [u0 - o, v1 + o, z], [u1 + o, v1 + o, z], [u1 + o, vm, z + rise], [u0 - o, vm, z + rise]);
    fp3(g, lite(roof, 0.18), [u0 - o, v1 + o, z], [u1 + o, v1 + o, z], [u1 + o, v1 + o, z - 0.7], [u0 - o, v1 + o, z - 0.7]);
    const a = P(u0 - o, vm, z + rise);
    const b = P(u1 + o, vm, z + rise);
    stroke(g, lite(roof, 0.35), 0.8, [a, b]);
    for (let k = 1; k < 4; k++) {
      const t = k / 4;
      stroke(g, rgba(dark(roof, 0.4), 0.35), 0.5, [P(u0 - o, lerp(vm, v1 + o, t), z + rise * (1 - t)), P(u1 + o, lerp(vm, v1 + o, t), z + rise * (1 - t))]);
    }
  } else {
    const um = (u0 + u1) / 2;
    fp3(g, wall, [u0, v1, z], [u1, v1, z], [um, v1, z + rise]);
    fp3(g, dark(roof, 0.12), [u0 - o, v0 - o, z], [um, v0 - o, z + rise], [um, v1 + o, z + rise], [u0 - o, v1 + o, z]);
    fp3(g, dark(roof, 0.26), [u1 + o, v0 - o, z], [u1 + o, v1 + o, z], [um, v1 + o, z + rise], [um, v0 - o, z + rise]);
    fp3(g, dark(roof, 0.1), [u1 + o, v1 + o, z], [u1 + o, v0 - o, z], [u1 + o, v0 - o, z - 0.7], [u1 + o, v1 + o, z - 0.7]);
    stroke(g, lite(roof, 0.3), 0.8, [P(um, v0 - o, z + rise), P(um, v1 + o, z + rise)]);
    for (let k = 1; k < 4; k++) {
      const t = k / 4;
      stroke(g, rgba(dark(roof, 0.55), 0.35), 0.5, [P(lerp(um, u1 + o, t), v0 - o, z + rise * (1 - t)), P(lerp(um, u1 + o, t), v1 + o, z + rise * (1 - t))]);
    }
  }
}

// Which way a thing is turned: `a` runs along its long side, `b` across it. Facing 2 and 3 are facing 0
// and 1 the other way round. Returns a mapping to the ground and the length and width.
function frame(item) {
  const du = item.u1 - item.u0;
  const dv = item.v1 - item.v0;
  const alongU = du >= dv;
  const L = Math.max(du, dv);
  const W = Math.min(du, dv);
  const flip = item.facing >= 2;
  const m = (a, b, z = 0) => {
    const aa = flip ? L - a : a;
    const bb = flip ? W - b : b;
    return alongU ? [item.u0 + aa, item.v0 + bb, z] : [item.u0 + bb, item.v0 + aa, z];
  };
  return { m, L, W, flip, alongU };
}
const pt = (f, a, b, z = 0) => {
  const [u, v, zz] = f.m(a, b, z);
  return P(u, v, zz);
};
const fq = (g, fill, f, ...pts) => fp(g, fill, pts.map(([a, b, z]) => pt(f, a, b, z || 0)));

// The color an item was given, or its usual one.
const colorIn = (item, fallback) => hexOf(item.color, fallback);

// ---------------------------------------------------------------------------------------------
// Ground (E10): painted into a sprite 72 by 40 with the top vertex at (36, 4).

const GROUND_COLORS = {
  grass: '#74a141', clover: '#6fa046', meadow: '#84a84a', dirt: '#8d6c46', gravel: '#b9b3a4', sand: '#dcc488',
  paving: '#d6ccb1', cobbles: '#a9a597', boards: '#a77a4a', moss: '#5f8a3c', snow: '#eef3f8', tiles: '#9aa3b5',
};

function wash(g, base, rand, blobs = 24, k = 0.3) {
  g.fillStyle = grad(g, -32, 0, 32, 32, lite(base, 0.03), dark(base, 0.05));
  g.fillRect(-40, -6, 80, 46);
  for (let i = 0; i < blobs; i++) {
    const r = 2 + rand() * 5;
    ell(g, rand() < 0.55 ? rgba(lite(base, k), 0.16) : rgba(dark(base, k), 0.16), (rand() - 0.5) * 60, 3 + rand() * 28, r * 1.8, r * 0.9);
  }
}

function blades(g, rand, base, n, len = 1.5) {
  g.lineWidth = 0.8;
  for (let i = 0; i < n; i++) {
    const [x, y] = P(rand() * 32, rand() * 32);
    g.strokeStyle = rand() < 0.5 ? rgba(dark(base, 0.4), 0.55) : rgba(lite(base, 0.45), 0.55);
    g.beginPath();
    g.moveTo(x, y);
    g.lineTo(x + (rand() - 0.5) * 1.6, y - len - rand() * len);
    g.stroke();
  }
}

function tufts(g, rand, base, n) {
  for (let i = 0; i < n; i++) {
    const [x, y] = P(1 + rand() * 30, 1 + rand() * 30);
    for (let k = -2; k <= 2; k++) line(g, k % 2 ? rgba(lite(base, 0.4), 0.8) : rgba(dark(base, 0.5), 0.8), 0.7, x, y, x + k * 0.6, y - 2.4 + Math.abs(k) * 0.4);
  }
}

const GROUNDS = {
  grass(g, base, rand) {
    wash(g, base, rand, 26, 0.34);
    for (let k = 0; k < 8; k++) {
      fp3(g, k % 2 ? rgba(dark(base, 0.4), 0.09) : rgba(lite(base, 0.45), 0.11), [k * 4, -2], [k * 4 + 4, -2], [k * 4 + 4, 34], [k * 4, 34]);
    }
    blades(g, rand, base, 90);
    tufts(g, rand, base, 16);
    grain(g, 0.06);
  },
  clover(g, base, rand) {
    wash(g, base, rand, 26, 0.3);
    blades(g, rand, base, 50);
    const leaf = mixHex(base, '#3f8f3f', 0.5);
    for (let i = 0; i < 70; i++) {
      const [x, y] = P(rand() * 32, rand() * 32);
      const c = rand() < 0.5 ? leaf : lite(leaf, 0.25);
      ell(g, c, x - 0.6, y, 0.6, 0.45);
      ell(g, c, x + 0.6, y, 0.6, 0.45);
      ell(g, c, x, y - 0.6, 0.6, 0.45);
    }
    for (let i = 0; i < 22; i++) {
      const [x, y] = P(1 + rand() * 30, 1 + rand() * 30);
      line(g, '#3d6f2a', 0.5, x, y, x, y - 1.8);
      ell(g, rand() < 0.65 ? '#fffdf0' : '#f6a4c6', x, y - 2.1, 1, 0.85);
      rr(g, '#f2c230', x - 0.25, y - 2.3, 0.5, 0.5);
    }
    grain(g, 0.05);
  },
  meadow(g, base, rand) {
    wash(g, mixHex(base, '#b6a64a', 0.15), rand, 30, 0.36);
    blades(g, rand, base, 150, 2.2);
    tufts(g, rand, base, 34);
    g.lineWidth = 0.7;
    for (let i = 0; i < 40; i++) {
      const [x, y] = P(rand() * 32, rand() * 32);
      g.strokeStyle = rand() < 0.5 ? '#d9c872' : '#a8b45a';
      g.beginPath();
      g.moveTo(x, y);
      g.quadraticCurveTo(x + (rand() - 0.5) * 2, y - 3, x + (rand() - 0.5) * 3, y - 4.4);
      g.stroke();
    }
    const rand2 = rand;
    for (let i = 0; i < 16; i++) {
      const [x, y] = P(2 + rand2() * 28, 2 + rand2() * 28);
      flowerBunch(g, rand2, x, y, 3);
    }
    grain(g, 0.05);
  },
  dirt(g, base, rand) {
    wash(g, base, rand, 34, 0.42);
    for (let i = 0; i < 12; i++) {
      const [x, y] = P(rand() * 32, rand() * 32);
      ell(g, rgba(dark(base, 0.5), 0.2), x, y, 3 + rand() * 4, 0.8 + rand());
    }
    for (let i = 0; i < 90; i++) {
      const [x, y] = P(rand() * 32, rand() * 32);
      ell(g, rand() < 0.5 ? rgba(lite(base, 0.45), 0.7) : rgba(dark(base, 0.5), 0.7), x, y, 0.4 + rand() * 0.9, 0.3 + rand() * 0.4);
    }
    g.lineWidth = 0.5;
    for (let i = 0; i < 8; i++) {
      const [x, y] = P(rand() * 30, rand() * 30);
      stroke(g, rgba(dark(base, 0.6), 0.45), 0.5, [[x, y], [x + 2 + rand() * 3, y + 0.8], [x + 4 + rand() * 3, y - 0.4]]);
    }
    grain(g, 0.09);
  },
  gravel(g, base, rand) {
    wash(g, base, rand, 22, 0.3);
    for (let i = 0; i < 380; i++) {
      const [x, y] = P(rand() * 32, rand() * 32);
      const tone = rand();
      const c = tone < 0.3 ? lite(base, 0.4) : tone < 0.65 ? dark(base, 0.3) : tone < 0.85 ? mixHex(base, '#a59070', 0.5) : mixHex(base, '#ffffff', 0.7);
      ell(g, c, x, y, 0.45 + rand() * 0.8, 0.32 + rand() * 0.4);
    }
    for (let i = 0; i < 14; i++) {
      const [x, y] = P(rand() * 32, rand() * 32);
      ell(g, rgba(dark(base, 0.5), 0.5), x + 0.3, y + 0.4, 1.5, 0.8);
      ell(g, lite(base, 0.3), x, y, 1.5, 0.8);
    }
    grain(g, 0.08);
  },
  sand(g, base, rand) {
    wash(g, base, rand, 26, 0.32);
    g.lineWidth = 0.7;
    for (let i = 0; i < 20; i++) {
      const [x, y] = P(rand() * 30, 1 + rand() * 30);
      const w = 5 + rand() * 6;
      g.strokeStyle = rgba(dark(base, 0.45), 0.35);
      g.beginPath();
      g.moveTo(x - w, y);
      g.quadraticCurveTo(x, y - 1.6, x + w, y + 0.6);
      g.stroke();
      g.strokeStyle = rgba(lite(base, 0.6), 0.6);
      g.beginPath();
      g.moveTo(x - w, y + 0.9);
      g.quadraticCurveTo(x, y - 0.7, x + w, y + 1.5);
      g.stroke();
    }
    for (let i = 0; i < 160; i++) {
      const [x, y] = P(rand() * 32, rand() * 32);
      rr(g, rand() < 0.5 ? rgba(lite(base, 0.5), 0.8) : rgba(dark(base, 0.4), 0.7), x, y, 0.7, 0.6);
    }
    grain(g, 0.06);
  },
  paving(g, base, rand) {
    g.fillStyle = dark(base, 0.3);
    g.fillRect(-40, -6, 80, 46);
    for (let i = 0; i < 4; i++) {
      for (let j = 0; j < 4; j++) {
        const c = tint(base, rand, 0.1);
        // The slabs at the edge run on past it, so no dark rim shows between two tiles.
        const u0 = i === 0 ? -2 : i * 8 + 0.3;
        const v0 = j === 0 ? -2 : j * 8 + 0.3;
        const u1 = i === 3 ? 34 : i * 8 + 7.7;
        const v1 = j === 3 ? 34 : j * 8 + 7.7;
        fp3(g, c, [u0, v0], [u1, v0], [u1, v1], [u0, v1]);
        fp3(g, rgba(lite(c, 0.5), 0.3), [u0, v0], [u1, v0], [u1, v0 + 0.8], [u0, v0 + 0.8]);
        fp3(g, rgba(lite(c, 0.5), 0.22), [u0, v0], [u0 + 0.8, v0], [u0 + 0.8, v1], [u0, v1]);
        for (let k = 0; k < 5; k++) {
          const [x, y] = P(u0 + 1 + rand() * 5.4, v0 + 1 + rand() * 5.4);
          ell(g, rand() < 0.5 ? 'rgba(255,255,240,.14)' : 'rgba(60,50,40,.1)', x, y, 0.8 + rand() * 1.4, 0.5 + rand() * 0.6);
        }
      }
    }
    grain(g, 0.07);
  },
  cobbles(g, base, rand) {
    g.fillStyle = dark(base, 0.42);
    g.fillRect(-40, -6, 80, 46);
    for (let j = -1; j < 12; j++) {
      for (let i = -1; i < 11; i++) {
        const u = i * 3.1 + (j % 2) * 1.55 + (rand() - 0.5) * 0.6;
        const v = j * 3 + 0.8 + (rand() - 0.5) * 0.5;
        const c = tint(base, rand, 0.26);
        const r = 1.35 + rand() * 0.25;
        disc(g, dark(c, 0.4), u + 0.1, v + 0.1, r + 0.1, 0);
        disc(g, c, u, v, r, 0.5);
        disc(g, rgba(lite(c, 0.6), 0.5), u - r * 0.25, v - r * 0.25, r * 0.5, 0.6);
      }
    }
    grain(g, 0.07);
  },
  boards(g, base, rand) {
    g.fillStyle = dark(base, 0.5);
    g.fillRect(-40, -6, 80, 46);
    for (let j = -1; j < 18; j++) {
      const v0 = j * 2;
      let u = -rand() * 6;
      while (u < 32) {
        const len = 7 + rand() * 9;
        const c = tint(base, rand, 0.2);
        const a = Math.max(0, u + 0.15);
        const b = Math.min(32, u + len - 0.15);
        if (b > a) {
          fp3(g, c, [a, v0 + 0.12], [b, v0 + 0.12], [b, v0 + 1.88], [a, v0 + 1.88]);
          fp3(g, rgba(lite(c, 0.5), 0.35), [a, v0 + 0.12], [b, v0 + 0.12], [b, v0 + 0.5], [a, v0 + 0.5]);
          const [gx, gy] = P(a, v0 + 1.0);
          const [hx, hy] = P(b, v0 + 1.0);
          stroke(g, rgba(dark(c, 0.5), 0.3), 0.4, [[gx, gy], [hx, hy]]);
          for (const uu of [a + 0.7, b - 0.7]) {
            if (uu > 0 && uu < 32) {
              const [nx, ny] = P(uu, v0 + 1);
              rr(g, rgba(dark(c, 0.7), 0.8), nx - 0.25, ny - 0.2, 0.5, 0.4);
            }
          }
        }
        u += len;
      }
    }
    grain(g, 0.07);
  },
  moss(g, base, rand) {
    wash(g, base, rand, 40, 0.34);
    for (let i = 0; i < 120; i++) {
      const [x, y] = P(rand() * 32, rand() * 32);
      ell(g, rand() < 0.5 ? rgba(lite(base, 0.35), 0.5) : rgba(dark(base, 0.4), 0.45), x, y, 0.8 + rand() * 1.8, 0.5 + rand() * 0.9);
    }
    blades(g, rand, base, 70, 1);
    for (let i = 0; i < 6; i++) {
      const [x, y] = P(3 + rand() * 26, 3 + rand() * 26);
      boulder(g, x, y, 1.3 + rand() * 1.1);
    }
    grain(g, 0.06);
  },
  snow(g, base, rand) {
    wash(g, base, rand, 30, 0.07);
    for (let i = 0; i < 16; i++) {
      const [x, y] = P(rand() * 32, rand() * 32);
      ell(g, 'rgba(120,150,210,.16)', x, y + 0.8, 3 + rand() * 5, 1 + rand() * 1.6);
    }
    for (let i = 0; i < 9; i++) {
      const [x, y] = P(1 + rand() * 30, 1 + rand() * 30);
      for (let k = -1; k <= 1; k++) line(g, '#6d8a4a', 0.6, x, y, x + k * 0.7, y - 1.5);
    }
    for (let i = 0; i < 90; i++) {
      const [x, y] = P(rand() * 32, rand() * 32);
      rr(g, 'rgba(255,255,255,.95)', x, y, 0.55, 0.5);
    }
    grain(g, 0.03);
  },
  tiles(g, base, rand) {
    g.fillStyle = dark(base, 0.5);
    g.fillRect(-40, -6, 80, 46);
    for (let i = 0; i < 8; i++) {
      for (let j = 0; j < 8; j++) {
        const c = (i + j) % 2 ? mixHex(base, '#ffffff', 0.12) : dark(base, 0.12);
        const u0 = i === 0 ? -2 : i * 4 + 0.18;
        const v0 = j === 0 ? -2 : j * 4 + 0.18;
        const u1 = i === 7 ? 34 : i * 4 + 3.82;
        const v1 = j === 7 ? 34 : j * 4 + 3.82;
        fp3(g, c, [u0, v0], [u1, v0], [u1, v1], [u0, v1]);
        fp3(g, 'rgba(255,255,255,.16)', [u0, v0], [u1, v0], [u1, v0 + 0.7], [u0, v0 + 0.7]);
        if (rand() < 0.3) fp3(g, 'rgba(255,255,255,.1)', [u0 + 0.6, v0 + 0.6], [u0 + 1.6, v0 + 0.6], [u0 + 0.6, v0 + 1.6]);
      }
    }
    grain(g, 0.05);
  },
};

export function paintGroundCover(S, kind, outside) {
  const g = S.g;
  const style = styleOf(outside, 'ground', Object.keys(GROUNDS), null);
  if (!style) {
    // E4: with no ground chosen a place looks as it always did.
    if (kind === 'square') {
      paving(S, [1, 1, 31, 31], '#d6ccb1', '#b3a887');
      paving(S, [7, 7, 25, 25], '#c7bc9f', '#a79c7e');
    } else if (kind === 'cafe') {
      const r = rectOf(kind, outside);
      if (r) paving(S, [Math.max(1, r[0] - 2), Math.max(1, r[1] - 2), Math.min(31, r[2] + 2), Math.min(31, r[3] + 6)], '#d6ccb1', '#b3a887');
    }
    return;
  }
  const base = hexOf(partOf(outside, 'ground').color, GROUND_COLORS[style]);
  g.save();
  g.beginPath();
  g.moveTo(0, -BLEED);
  g.lineTo(32 + BLEED * 2, 16);
  g.lineTo(0, 32 + BLEED);
  g.lineTo(-32 - BLEED * 2, 16);
  g.closePath();
  g.clip();
  GROUNDS[style](g, base, randOf(S, 1));
  g.restore();
}

// ---------------------------------------------------------------------------------------------
// Paths (E10): from the front of the door to the edge of the tile. An arm is a strip: `a` runs along it
// from where it starts, `b` across it, and `m` turns that into a point on the ground.

const PATH_COLORS = { flagstone: '#b4b0a2', brick: '#a85a42', gravel: '#d3c6a0', stepping_stones: '#aaa79a', boards: '#9b6f43', cobbles: '#9b978b' };

// Where the door meets the ground, or the middle of its wall if there's no telling.
function doorOf(S, kind, outside, rect) {
  const fallback = S.door === 'R' ? [rect[2], (rect[1] + rect[3]) / 2] : [(rect[0] + rect[2]) / 2, rect[3]];
  return doorPoint(kind, outside, S.door) || fallback;
}

function armsOf(S, kind, outside) {
  const rect = rectOf(kind, outside);
  if (!rect) {
    const W = 5;
    const lo = 16 - W / 2;
    return [
      { L: 17, W, m: (a, b, z = 0) => [a - 1, lo + b, z] },
      { L: 17, W, m: (a, b, z = 0) => [33 - a, lo + b, z] },
      { L: 17, W, m: (a, b, z = 0) => [lo + b, a - 1, z] },
      { L: 17, W, m: (a, b, z = 0) => [lo + b, 33 - a, z] },
    ];
  }
  const [du, dv] = doorOf(S, kind, outside, rect);
  const W = 4.4;
  if (S.door === 'R') return [{ L: 34 - du, W, m: (a, b, z = 0) => [du + a - 0.4, dv - W / 2 + b, z] }];
  return [{ L: 34 - dv, W, m: (a, b, z = 0) => [du - W / 2 + b, dv + a - 0.4, z] }];
}

function band(g, arm, fill, a0, b0, a1, b1, z = 0) {
  fp3(g, fill, arm.m(a0, b0, z), arm.m(a1, b0, z), arm.m(a1, b1, z), arm.m(a0, b1, z));
}

// A slab with corners a little off true.
function slab(g, arm, a0, b0, a1, b1, fill, rand, j = 0.22) {
  const w = () => (rand() - 0.5) * j;
  const pts = [[a0 + w(), b0 + w()], [a1 + w(), b0 + w()], [a1 + w(), b1 + w()], [a0 + w(), b1 + w()]].map(([a, b]) => arm.m(a, b));
  fp3(g, fill, ...pts);
  stroke(g, rgba(lite(fill, 0.6), 0.45), 0.5, [P(pts[0][0], pts[0][1]), P(pts[1][0], pts[1][1])]);
}

const PATHS = {
  none() {},
  flagstone(g, arm, rand, c) {
    band(g, arm, rgba(dark(c, 0.6), 0.55), -0.5, -0.45, arm.L, arm.W + 0.45);
    let a = 0;
    while (a < arm.L) {
      const h = 2.2 + rand() * 1.6;
      const cuts = rand() < 0.6 ? [0, 0.35 + rand() * 0.3, 1] : [0, 1];
      for (let i = 0; i < cuts.length - 1; i++) {
        slab(g, arm, a + 0.2, cuts[i] * arm.W + 0.2, a + h - 0.2, cuts[i + 1] * arm.W - 0.2, tint(c, rand, 0.18), rand);
      }
      a += h;
    }
  },
  brick(g, arm, rand, c) {
    band(g, arm, '#cfc3a8', -0.5, -0.4, arm.L, arm.W + 0.4);
    const bl = 1.9;
    const bh = 0.95;
    for (let r = 0; r * bh < arm.L + 1; r++) {
      const off = (r % 2) * bl * 0.5;
      for (let b = -off; b < arm.W; b += bl) {
        const b0 = Math.max(0, b) + 0.06;
        const b1 = Math.min(arm.W, b + bl) - 0.06;
        if (b1 - b0 < 0.3) continue;
        const col = tint(c, rand, 0.2);
        band(g, arm, col, r * bh + 0.05, b0, r * bh + bh - 0.05, b1);
        band(g, arm, rgba(lite(col, 0.5), 0.3), r * bh + 0.05, b0, r * bh + 0.25, b1);
      }
    }
    band(g, arm, rgba(dark(c, 0.6), 0.5), -0.5, -0.5, arm.L, 0.12);
    band(g, arm, rgba(dark(c, 0.6), 0.5), -0.5, arm.W - 0.12, arm.L, arm.W + 0.5);
  },
  gravel(g, arm, rand, c) {
    for (let t = -0.5; t < arm.L; t += 1.2) {
      for (const b of [-0.1, arm.W + 0.1]) {
        const [u, v] = arm.m(t, b);
        disc(g, tint(mixHex(c, '#6c6254', 0.4), rand, 0.2), u, v, 0.75 + rand() * 0.3);
      }
    }
    band(g, arm, dark(c, 0.2), -0.5, 0, arm.L, arm.W);
    band(g, arm, c, -0.5, 0.35, arm.L, arm.W - 0.35);
    for (let i = 0; i < arm.L * arm.W * 5; i++) {
      const [u, v] = arm.m(-0.3 + rand() * (arm.L + 0.3), 0.3 + rand() * (arm.W - 0.6));
      const [x, y] = P(u, v);
      const t = rand();
      ell(g, t < 0.35 ? lite(c, 0.45) : t < 0.7 ? dark(c, 0.3) : mixHex(c, '#a58f6a', 0.4), x, y, 0.4 + rand() * 0.6, 0.3 + rand() * 0.3);
    }
  },
  stepping_stones(g, arm, rand, c) {
    let i = 0;
    for (let a = 1.2; a < arm.L; a += 3.5) {
      const b = arm.W / 2 + (i++ % 2 ? 0.5 : -0.5) + (rand() - 0.5) * 0.4;
      const [u, v] = arm.m(a, b);
      const r = 1.35 + rand() * 0.3;
      const col = tint(c, rand, 0.2);
      disc(g, 'rgba(25,30,20,.32)', u + 0.35, v + 0.35, r + 0.15);
      disc(g, dark(col, 0.35), u, v, r, 0);
      disc(g, col, u, v, r, 0.8);
      disc(g, rgba(lite(col, 0.6), 0.55), u - r * 0.25, v - r * 0.25, r * 0.55, 0.9);
    }
  },
  boards(g, arm, rand, c) {
    band(g, arm, 'rgba(25,20,30,.4)', -0.5, -0.2, arm.L, arm.W + 0.5);
    const planks = [];
    for (let a = -0.3; a < arm.L; a += 1.4) planks.push(a);
    for (const a of planks) {
      g.fillStyle = dark(c, 0.45);
      fp3(g, dark(c, 0.45), arm.m(a, arm.W, 0), arm.m(a + 1.4, arm.W, 0), arm.m(a + 1.4, arm.W, 0.9), arm.m(a, arm.W, 0.9));
    }
    for (const a of planks) {
      const col = tint(c, rand, 0.22);
      band(g, arm, col, a + 0.06, 0, a + 1.34, arm.W, 0.9);
      band(g, arm, rgba(lite(col, 0.6), 0.35), a + 0.06, 0, a + 0.4, arm.W, 0.9);
      for (const b of [0.5, arm.W - 0.5]) {
        const [x, y] = P(...arm.m(a + 0.7, b, 0.9).slice(0, 2), 0.9);
        rr(g, rgba(dark(col, 0.7), 0.8), x - 0.25, y - 0.2, 0.5, 0.4);
      }
    }
  },
  cobbles(g, arm, rand, c) {
    band(g, arm, dark(c, 0.55), -0.5, -0.3, arm.L, arm.W + 0.3);
    let r = 0;
    for (let a = -0.4; a < arm.L; a += 1.5) {
      for (let b = 0.6 + (r % 2) * 0.75; b < arm.W + 0.4; b += 1.5) {
        const [u, v] = arm.m(a + (rand() - 0.5) * 0.3, Math.min(arm.W - 0.45, Math.max(0.45, b + (rand() - 0.5) * 0.3)));
        const col = tint(c, rand, 0.28);
        const rad = 0.7 + rand() * 0.1;
        disc(g, dark(col, 0.4), u + 0.05, v + 0.05, rad + 0.08, 0);
        disc(g, col, u, v, rad, 0.4);
        disc(g, rgba(lite(col, 0.6), 0.45), u - 0.2, v - 0.2, rad * 0.45, 0.5);
      }
      r++;
    }
  },
};

export function paintPath(S, kind, outside) {
  const style = styleOf(outside, 'path', Object.keys(PATHS), 'none');
  if (style === 'none') return;
  const base = hexOf(partOf(outside, 'path').color, PATH_COLORS[style]);
  const g = S.g;
  const rand = randOf(S, 2);
  g.save();
  g.beginPath();
  g.moveTo(0, -BLEED);
  g.lineTo(32 + BLEED * 2, 16);
  g.lineTo(0, 32 + BLEED);
  g.lineTo(-32 - BLEED * 2, 16);
  g.closePath();
  g.clip();
  for (const arm of armsOf(S, kind, outside)) PATHS[style](g, arm, rand, base);
  g.restore();
}

// ---------------------------------------------------------------------------------------------
// Fences (E11): along the four edges of the tile, inset by one unit. A run goes along an edge: `t` is
// how far along, `off` how far out from the edge line towards the front, and `z` how high.

const SIDES = [
  { bit: 1, along: 'u', at: 1, front: false },
  { bit: 8, along: 'v', at: 1, front: false },
  { bit: 2, along: 'v', at: 31, front: true },
  { bit: 4, along: 'u', at: 31, front: true },
];
const GATE_HALF = 3;

function runOf(side) {
  const { along, at } = side;
  const lit = along === 'u';
  return { along, at, lit, pt: (t, off = 0, z = 0) => (lit ? P(t, at + off, z) : P(at + off, t, z)) };
}

const face = (g, fill, run, t0, t1, z0, z1, off = 0) => quad(g, fill, run.pt(t0, off, z0), run.pt(t1, off, z0), run.pt(t1, off, z1), run.pt(t0, off, z1));
const shaded = (run, c) => (run.lit ? c : dark(c, 0.26));

// A slab along the run: its front, its top and its end.
function wallBlock(g, run, t0, t1, d, z0, z1, color) {
  const h = d / 2;
  quad(g, shaded(run, color), run.pt(t0, h, z0), run.pt(t1, h, z0), run.pt(t1, h, z1), run.pt(t0, h, z1));
  quad(g, run.lit ? dark(color, 0.26) : color, run.pt(t1, h, z0), run.pt(t1, -h, z0), run.pt(t1, -h, z1), run.pt(t1, h, z1));
  quad(g, lite(color, 0.2), run.pt(t0, -h, z1), run.pt(t1, -h, z1), run.pt(t1, h, z1), run.pt(t0, h, z1));
}

const post = (g, run, t, w, h, color) => wallBlock(g, run, t - w / 2, t + w / 2, w, 0, h, color);

// Posts from `t0` to `t1` no further apart than `gap`.
function postsAt(t0, t1, gap) {
  const n = Math.max(1, Math.ceil((t1 - t0) / gap));
  return Array.from({ length: n + 1 }, (_, i) => lerp(t0, t1, i / n));
}

const runSeed = (run, t0) => Math.round(t0 * 13 + run.at * 7 + (run.lit ? 1 : 2));

const FENCES = {
  none() {},
  picket(g, run, t0, t1, c0) {
    const c = c0 || '#f6f1e4';
    const f = shaded(run, c);
    const h = 5.2;
    for (const z of [1.6, 3.7]) face(g, shaded(run, dark(c, 0.14)), run, t0, t1, z - 0.4, z + 0.4, -0.2);
    const n = Math.max(2, Math.round((t1 - t0) / 1.7));
    for (let i = 0; i <= n; i++) {
      const [x, y] = run.pt(lerp(t0, t1, i / n), 0.3, 0);
      rr(g, f, x - 0.7, y - h, 1.4, h);
      fp(g, f, [[x - 0.7, y - h], [x, y - h - 1.2], [x + 0.7, y - h]]);
      rr(g, 'rgba(25,15,35,.2)', x + 0.2, y - h, 0.5, h);
    }
    for (const t of [t0 + 0.3, t1 - 0.3]) post(g, run, t, 1.3, h + 1.3, lite(c, 0.06));
  },
  rail(g, run, t0, t1, c0) {
    const c = c0 || '#8a5d33';
    for (const t of postsAt(t0 + 0.3, t1 - 0.3, 8)) post(g, run, t, 1.3, 5.4, c);
    for (const z of [1.9, 3.9]) wallBlock(g, run, t0, t1, 0.6, z - 0.5, z + 0.5, lite(c, 0.12));
  },
  hedge(g, run, t0, t1, c0) {
    const c = c0 || '#3f7a3c';
    const d = 3.4;
    const H = 6.6;
    const rand = seeded(runSeed(run, t0));
    wallBlock(g, run, t0, t1, d, 0, H, c);
    face(g, 'rgba(15,35,20,.32)', run, t0, t1, 0, 2.2, d / 2);
    const n = Math.round((t1 - t0) * 3.4);
    for (let i = 0; i < n; i++) {
      const [x, y] = run.pt(lerp(t0, t1, rand()), d / 2, rand() * H);
      const r = 0.7 + rand() * 0.8;
      ell(g, rand() < 0.5 ? rgba(shaded(run, dark(c, 0.4)), 0.7) : rgba(shaded(run, lite(c, 0.35)), 0.6), x, y, r * 1.2, r);
    }
    for (let t = t0 + 0.5; t < t1; t += 1.3) {
      const [x, y] = run.pt(t + rand() * 0.5, d / 2, H);
      ell(g, shaded(run, lite(c, 0.1)), x, y, 1.35, 1);
    }
    for (let i = 0; i < n * 0.8; i++) {
      const [x, y] = run.pt(lerp(t0, t1, rand()), (rand() - 0.5) * d, H);
      ell(g, rand() < 0.5 ? lite(c, 0.32) : rgba(dark(c, 0.3), 0.7), x, y, 0.9, 0.6);
    }
  },
  stone(g, run, t0, t1, c0) {
    const c = c0 || '#a8a597';
    const d = 1.9;
    const rand = seeded(runSeed(run, t0));
    wallBlock(g, run, t0, t1, d, 0, 4.8, dark(c, 0.3));
    for (let r = 0; r < 3; r++) {
      const z0 = r * 1.6;
      let t = t0 - rand() * 1.5;
      while (t < t1) {
        const len = 1.8 + rand() * 2.2;
        const a = Math.max(t0, t) + 0.1;
        const b = Math.min(t1, t + len) - 0.1;
        if (b > a) {
          const col = shaded(run, tint(c, rand, 0.22));
          face(g, col, run, a, b, z0 + 0.1, z0 + 1.5, d / 2);
          face(g, 'rgba(255,255,255,.22)', run, a, b, z0 + 1.15, z0 + 1.5, d / 2);
        }
        t += len;
      }
    }
    face(g, lite(c, 0.12), run, t0, t1, 4.8 - 0.1, 4.8, d / 2);
    for (let t = t0 + 1; t < t1; t += 1.5 + rand() * 2) {
      const [x, y] = run.pt(t, 0, 4.8);
      stroke(g, 'rgba(40,35,30,.3)', 0.4, [[x, y], run.pt(t, d / 2, 4.8)]);
    }
  },
  brick(g, run, t0, t1, c0) {
    const c = c0 || '#a4553d';
    const d = 1.8;
    const rand = seeded(runSeed(run, t0));
    wallBlock(g, run, t0, t1, d, 0, 5.4, '#cdbfa5');
    for (let r = 0; r < 6; r++) {
      const z0 = r * 0.9;
      let t = t0 - (r % 2) * 0.95;
      while (t < t1) {
        const a = Math.max(t0, t) + 0.06;
        const b = Math.min(t1, t + 1.9) - 0.06;
        if (b > a + 0.2) face(g, shaded(run, tint(c, rand, 0.2)), run, a, b, z0 + 0.06, z0 + 0.84, d / 2);
        t += 1.9;
      }
    }
    wallBlock(g, run, t0, t1, d + 0.5, 5.4, 6.0, '#d9cdb5');
    const pillars = postsAt(t0 + 0.9, t1 - 0.9, 11);
    for (const t of pillars) {
      wallBlock(g, run, t - 1.1, t + 1.1, 2.4, 0, 6.9, mixHex(c, '#cdbfa5', 0.2));
      wallBlock(g, run, t - 1.5, t + 1.5, 3.0, 6.9, 7.6, '#d9cdb5');
    }
  },
  iron(g, run, t0, t1, c0) {
    const c = c0 || '#2b2d36';
    const f = run.lit ? lite(c, 0.1) : c;
    for (const z of [1.2, 4.7]) face(g, f, run, t0, t1, z - 0.3, z + 0.3);
    const n = Math.max(2, Math.round((t1 - t0) / 1.1));
    for (let i = 0; i <= n; i++) {
      const [x, y] = run.pt(lerp(t0, t1, i / n), 0, 0);
      rr(g, f, x - 0.3, y - 5.7, 0.6, 5.7);
      fp(g, f, [[x - 0.75, y - 5.7], [x, y - 7.4], [x + 0.75, y - 5.7]]);
      rr(g, 'rgba(255,255,255,.28)', x - 0.3, y - 5.4, 0.2, 5.4);
    }
    for (const t of postsAt(t0 + 0.3, t1 - 0.3, 8)) {
      const [x, y] = run.pt(t, 0, 0);
      rr(g, c, x - 0.7, y - 6.6, 1.4, 6.6);
      ell(g, c, x, y - 7.3, 1.05, 1.05);
      rr(g, 'rgba(255,255,255,.35)', x - 0.7, y - 6.4, 0.3, 6.2);
      ell(g, 'rgba(255,255,255,.4)', x - 0.3, y - 7.6, 0.35, 0.3);
    }
  },
  bamboo(g, run, t0, t1, c0) {
    const c = c0 || '#b2bd5c';
    const rand = seeded(runSeed(run, t0));
    const n = Math.max(2, Math.round((t1 - t0) / 1.25));
    for (let i = 0; i <= n; i++) {
      const h = 5.4 + rand() * 1.5;
      const [x, y] = run.pt(lerp(t0, t1, i / n), 0, 0);
      g.fillStyle = grad(g, x - 0.65, 0, x + 0.65, 0, lite(c, 0.28), c, dark(c, 0.32));
      g.fillRect(x - 0.65, y - h, 1.3, h);
      for (let z = 1.5 + rand() * 0.5; z < h; z += 1.8) rr(g, rgba(dark(c, 0.5), 0.7), x - 0.75, y - z, 1.5, 0.35);
      ell(g, dark(c, 0.45), x, y - h, 0.65, 0.3);
    }
    for (const z of [1.7, 4.1]) {
      const a = run.pt(t0, 0.7, z);
      const b = run.pt(t1, 0.7, z);
      stroke(g, shaded(run, dark(c, 0.12)), 1.1, [a, b]);
      stroke(g, 'rgba(255,255,255,.25)', 0.35, [[a[0], a[1] - 0.3], [b[0], b[1] - 0.3]]);
    }
    for (let i = 0; i <= n; i += 3) {
      const [x, y] = run.pt(lerp(t0, t1, i / n), 0.7, 0);
      for (const z of [1.7, 4.1]) rr(g, '#5e4426', x - 0.75, y - z - 0.3, 1.5, 0.6);
    }
  },
  rope(g, run, t0, t1, c0) {
    const c = c0 || '#d3b072';
    const wood = '#7a5530';
    const ts = postsAt(t0 + 0.4, t1 - 0.4, 7.5);
    for (const t of ts) {
      post(g, run, t, 1.3, 5.8, wood);
      const [x, y] = run.pt(t, 0, 6.3);
      ell(g, lite(wood, 0.18), x, y, 0.95, 0.95);
    }
    for (let k = 0; k < ts.length - 1; k++) {
      for (const [z, sag] of [[5.1, 1.5], [2.9, 1.3]]) {
        const a = run.pt(ts[k], 0.7, z);
        const b = run.pt(ts[k + 1], 0.7, z);
        for (const [w, col] of [[1.1, shaded(run, c)], [0.4, rgba(lite(c, 0.5), 0.8)]]) {
          g.strokeStyle = col;
          g.lineWidth = w;
          g.beginPath();
          g.moveTo(a[0], a[1]);
          g.quadraticCurveTo((a[0] + b[0]) / 2, (a[1] + b[1]) / 2 + sag * 2, b[0], b[1]);
          g.stroke();
        }
      }
    }
  },
  wattle(g, run, t0, t1, c0) {
    const c = c0 || '#9a7448';
    wallBlock(g, run, t0, t1, 1.2, 0, 4.8, dark(c, 0.3));
    for (let r = 0; r < 6; r++) {
      const pts = [];
      for (let t = t0; t <= t1 + 0.01; t += 0.4) pts.push(run.pt(t, 0.7, 0.55 + r * 0.78 + 0.28 * Math.sin(t * 2.4 + r * 2.1)));
      stroke(g, shaded(run, r % 2 ? lite(c, 0.12) : c), 0.95, pts);
    }
    for (let t = t0 + 0.5; t < t1; t += 1.6) {
      const [x, y] = run.pt(t, 0.7, 0);
      line(g, rgba(shaded(run, dark(c, 0.5)), 0.85), 0.8, x, y, x, y - 5.8);
    }
  },
  glass(g, run, t0, t1, c0) {
    const c = c0 || '#8fcbec';
    const frame = '#8a97a3';
    const H = 5.6;
    const ts = postsAt(t0 + 0.35, t1 - 0.35, 5.6);
    for (let k = 0; k < ts.length - 1; k++) {
      const a = ts[k];
      const b = ts[k + 1];
      const [x0, y0] = run.pt(a, 0, 0);
      const [x1, y1] = run.pt(b, 0, H);
      g.fillStyle = grad(g, x0, y1, x1, y0, rgba(lite(c, 0.4), 0.42), rgba(c, 0.26));
      face(g, g.fillStyle, run, a, b, 0.3, H);
      stroke(g, 'rgba(255,255,255,.6)', 0.5, [run.pt(lerp(a, b, 0.2), 0, 0.8), run.pt(lerp(a, b, 0.45), 0, H - 0.8)]);
      stroke(g, 'rgba(255,255,255,.35)', 0.4, [run.pt(lerp(a, b, 0.32), 0, 0.8), run.pt(lerp(a, b, 0.52), 0, H - 1.6)]);
      face(g, frame, run, a, b, H - 0.3, H + 0.2);
      face(g, frame, run, a, b, 0, 0.4);
    }
    for (const t of ts) post(g, run, t, 0.8, H + 0.4, frame);
  },
};

// E10: where the path meets the fence. A town square has one on every edge.
function gateCenters(S, kind, outside) {
  const rect = rectOf(kind, outside);
  const out = new Map();
  if (!rect) {
    for (const s of SIDES) out.set(s.bit, 16);
  } else if (S.door === 'R') {
    out.set(2, doorOf(S, kind, outside, rect)[1]);
  } else {
    out.set(4, doorOf(S, kind, outside, rect)[0]);
  }
  return out;
}

const GATES = {
  none() {},
  gap() {},
  wooden(g, run, t, c0) {
    const c = c0 || '#b88a52';
    for (const s of [-1, 1]) {
      const a = t + s * 0.15;
      const b = t + s * (GATE_HALF - 0.75);
      const lo = Math.min(a, b);
      const hi = Math.max(a, b);
      wallBlock(g, run, lo, hi, 0.7, 0.6, 5.6, c);
      for (let u = lo + 0.5; u < hi; u += 0.7) stroke(g, rgba(shaded(run, dark(c, 0.5)), 0.4), 0.35, [run.pt(u, 0.35, 0.8), run.pt(u, 0.35, 5.2)]);
      stroke(g, shaded(run, dark(c, 0.3)), 0.8, [run.pt(lo + 0.2, 0.4, 1.2), run.pt(hi - 0.2, 0.4, 5)]);
    }
    for (const s of [-1, 1]) post(g, run, t + s * GATE_HALF, 1.5, 6.6, dark(c, 0.12));
    const [x, y] = run.pt(t, 0.4, 3);
    rr(g, '#2b2d36', x - 0.5, y - 0.4, 1, 0.8);
  },
  iron(g, run, t, c0) {
    const c = c0 || '#2b2d36';
    const f = run.lit ? lite(c, 0.1) : c;
    for (const z of [1.2, 4.2]) face(g, f, run, t - GATE_HALF + 0.6, t + GATE_HALF - 0.6, z - 0.3, z + 0.3);
    for (let u = t - GATE_HALF + 1; u <= t + GATE_HALF - 0.9; u += 0.85) {
      const lift = 1.6 * (1 - Math.abs(u - t) / GATE_HALF);
      const [x, y] = run.pt(u, 0, 0);
      rr(g, f, x - 0.3, y - 5.4 - lift, 0.6, 5.4 + lift);
      fp(g, f, [[x - 0.75, y - 5.4 - lift], [x, y - 7.1 - lift], [x + 0.75, y - 5.4 - lift]]);
    }
    const a = run.pt(t - GATE_HALF + 0.6, 0, 4.4);
    const b = run.pt(t + GATE_HALF - 0.6, 0, 4.4);
    g.strokeStyle = f;
    g.lineWidth = 0.7;
    g.beginPath();
    g.moveTo(a[0], a[1]);
    g.quadraticCurveTo((a[0] + b[0]) / 2, (a[1] + b[1]) / 2 - 3.6, b[0], b[1]);
    g.stroke();
    for (const s of [-1, 1]) {
      const [x, y] = run.pt(t + s * GATE_HALF, 0, 0);
      rr(g, c, x - 0.8, y - 7, 1.6, 7);
      ell(g, c, x, y - 7.8, 1.15, 1.15);
      rr(g, 'rgba(255,255,255,.3)', x - 0.8, y - 6.8, 0.3, 6.6);
    }
  },
  arch(g, run, t, c0) {
    const c = c0 || '#efe9da';
    const rand = seeded(runSeed(run, t));
    for (const s of [-1, 1]) post(g, run, t + s * (GATE_HALF - 0.4), 1.2, 9, c);
    const a = run.pt(t - GATE_HALF + 0.4, 0, 9);
    const b = run.pt(t + GATE_HALF - 0.4, 0, 9);
    const mx = (a[0] + b[0]) / 2;
    const my = (a[1] + b[1]) / 2 - 7;
    for (const [w, col] of [[1.5, shaded(run, dark(c, 0.1))], [0.5, rgba(lite(c, 0.6), 0.8)]]) {
      g.strokeStyle = col;
      g.lineWidth = w;
      g.beginPath();
      g.moveTo(a[0], a[1]);
      g.quadraticCurveTo(mx, my, b[0], b[1]);
      g.stroke();
    }
    for (let i = 0; i < 26; i++) {
      const u = rand();
      const side = rand();
      let x;
      let y;
      if (side < 0.7) {
        x = (1 - u) * (1 - u) * a[0] + 2 * u * (1 - u) * mx + u * u * b[0];
        y = (1 - u) * (1 - u) * a[1] + 2 * u * (1 - u) * my + u * u * b[1];
      } else {
        [x, y] = run.pt(t + (rand() < 0.5 ? -1 : 1) * (GATE_HALF - 0.4), 0, rand() * 9);
      }
      ell(g, rand() < 0.5 ? '#3f8a3a' : '#69b04a', x + (rand() - 0.5), y + (rand() - 0.5), 0.8, 0.6);
      if (rand() < 0.22) ell(g, rand() < 0.5 ? '#f27a9a' : '#fff0c0', x, y - 0.4, 0.55, 0.5);
    }
  },
  pillars(g, run, t, c0) {
    const c = c0 || '#c9c4b3';
    for (const s of [-1, 1]) {
      const p = t + s * (GATE_HALF - 0.1);
      wallBlock(g, run, p - 1.1, p + 1.1, 2.2, 0, 8.4, c);
      wallBlock(g, run, p - 1.5, p + 1.5, 3.0, 8.4, 9.2, lite(c, 0.1));
      face(g, 'rgba(40,35,30,.22)', run, p - 1.1, p + 1.1, 2.9, 3.1, 1.1);
      face(g, 'rgba(40,35,30,.22)', run, p - 1.1, p + 1.1, 5.6, 5.8, 1.1);
      const [x, y] = run.pt(p, 0, 9.2);
      ell(g, shaded(run, lite(c, 0.15)), x, y - 1.1, 1.15, 1.15);
      ell(g, 'rgba(255,255,255,.4)', x - 0.35, y - 1.5, 0.4, 0.35);
    }
  },
};

export function paintFence(S, kind, outside, side) {
  if (!S.day) return;
  const style = styleOf(outside, 'fence', Object.keys(FENCES), 'none');
  if (style === 'none') return;
  const mask = numberOf(outside, 'fence', 15);
  const color = hexOf(partOf(outside, 'fence').color, null);
  const gateStyle = styleOf(outside, 'gate', Object.keys(GATES), 'gap');
  const gateColor = hexOf(partOf(outside, 'gate').color, null);
  const gates = gateCenters(S, kind, outside);
  const g = S.g;
  const part = (run, a, b) => {
    if (b - a > 0.7) FENCES[style](g, run, a, b, color);
  };
  for (const sd of SIDES) {
    if (!(mask & sd.bit) || sd.front !== (side === 'front')) continue;
    const run = runOf(sd);
    const at = gates.get(sd.bit);
    if (at === undefined || gateStyle === 'none') {
      part(run, 1, 31);
      continue;
    }
    part(run, 1, at - GATE_HALF);
    part(run, at + GATE_HALF, 31);
    GATES[gateStyle](g, run, at, gateColor);
  }
}

// ---------------------------------------------------------------------------------------------
// The town square's middle (E14): u and v from 12 to 20.

const PLAZA = [12, 12, 20, 20];
const FOUNTAIN_STONE = '#c9c6b8';

// The stone fountain `placedSquare` has always had, in another stone.
function plazaFountain(g, stone) {
  disc(g, 'rgba(25,25,35,.25)', 18, 18, 8.4);
  disc(g, mixHex(stone, '#7a766c', 0.5), 16, 16, 7.2, 0);
  disc(g, stone, 16, 16, 7.2, 3);
  disc(g, dark(stone, 0.28), 16, 16, 6, 3);
  disc(g, '#4fa6d8', 16, 16, 5.6, 2.2);
  disc(g, '#7bc4ec', 15.4, 15.4, 3.6, 2.3);
  const [x, y] = P(16, 16, 3);
  rr(g, lite(stone, 0.12), x - 1.6, y - 11, 3.2, 11);
  ell(g, stone, x, y - 11, 4, 1.7);
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
  glint(g, 12.4, 17.4, 3);
  glint(g, 17.6, 12.8, 2);
}

// Little bright strokes on water at a point on the ground.
function glint(g, u, v, z = 0, n = 1) {
  for (let i = 0; i < n; i++) {
    const [x, y] = P(u + i * 1.3, v - i * 0.9, z);
    stroke(g, 'rgba(255,255,255,.85)', 0.55, [[x - 1.1, y + 0.2], [x + 0.2, y - 0.2]]);
    ell(g, 'rgba(255,255,255,.9)', x + 0.9, y + 0.5, 0.45, 0.25);
  }
}

const FOUNTAIN_BENCHES = [[16, 4, 'u'], [16, 28, 'u'], [4, 16, 'v'], [28, 16, 'v']];

const CENTERS = {
  none() {},
  fountain(S, c, outside) {
    if (!S.day) return;
    if (c) plazaFountain(S.g, c);
    else fountain(S.g);
    // The benches a square has always had make way for anything put where they stand.
    const items = itemsOf(outside);
    for (const [u, v, side] of FOUNTAIN_BENCHES) {
      const [du, dv] = side === 'u' ? [5, 2] : [2, 5];
      if (!items.some((item) => item.u0 < u + du && item.u1 > u - du && item.v0 < v + dv && item.v1 > v - dv)) bench(S.g, u, v, side);
    }
  },
  statue(S, c) {
    if (!S.day) return;
    const g = S.g;
    const bronze = c || '#78a08c';
    shadow(S, [14, 14, 18, 18], 28);
    disc(g, 'rgba(25,25,35,.25)', 17.5, 17.5, 6.6);
    const granite = '#aaa79b';
    solid(g, [12.4, 12.4, 19.6, 19.6], 0, 1.6, lite(granite, 0.1));
    solid(g, [13.5, 13.5, 18.5, 18.5], 1.6, 3.2, granite);
    solid(g, [14.5, 14.5, 17.5, 17.5], 3.2, 11.5, lite(granite, 0.06), {
      detailL: (w) => {
        rr(g, rgba(dark(granite, 0.4), 0.7), 0.6, -9.2, w - 1.2, 4.6);
        rr(g, lite(granite, 0.3), 0.8, -9, w - 1.6, 4.2);
        rr(g, 'rgba(60,55,50,.5)', 1.4, -7.6, w - 2.8, 0.4);
        rr(g, 'rgba(60,55,50,.5)', 1.4, -6.5, w - 2.8, 0.4);
      },
    });
    solid(g, [14.1, 14.1, 17.9, 17.9], 11.5, 12.5, lite(granite, 0.14));
    const [x, y] = P(16, 16, 12.5);
    // The cloak behind, then the robe, an arm down and an arm up holding a torch.
    quad(g, dark(bronze, 0.4), [x - 3.6, y], [x - 2.4, y - 11], [x - 1, y - 14], [x + 0.6, y - 14], [x + 0.6, y]);
    quad(g, grad(g, x - 3, 0, x + 3, 0, lite(bronze, 0.3), bronze, dark(bronze, 0.35)), [x - 2.9, y], [x - 2, y - 10], [x - 1.3, y - 13.4], [x + 1.3, y - 13.4], [x + 2.1, y - 10], [x + 3, y]);
    stroke(g, dark(bronze, 0.3), 0.5, [[x - 1, y - 1], [x - 0.8, y - 9]]);
    stroke(g, dark(bronze, 0.3), 0.5, [[x + 0.8, y - 1], [x + 0.9, y - 9]]);
    stroke(g, bronze, 1.4, [[x - 1.9, y - 12.4], [x - 2.7, y - 6.6]]);
    stroke(g, lite(bronze, 0.1), 1.4, [[x + 1.8, y - 12.4], [x + 3.4, y - 18]]);
    rr(g, dark(bronze, 0.3), x + 3.1, y - 21, 0.7, 3.2);
    ell(g, '#e8b64a', x + 3.45, y - 21.6, 1.3, 1.6);
    ell(g, lite(bronze, 0.1), x, y - 15.4, 1.55, 1.7);
    ell(g, 'rgba(255,255,255,.35)', x - 0.6, y - 16, 0.5, 0.6);
    ell(g, dark(bronze, 0.3), x - 0.2, y - 16.9, 1.6, 0.8);
  },
  bandstand(S, c) {
    const g = S.g;
    const roof = c || '#c0453f';
    const lanterns = [[12.9, 19.1], [19.1, 12.9], [19.1, 19.1]];
    if (!S.day) {
      for (const [u, v] of lanterns) {
        const [x, y] = P(u, v, 12.2);
        const gr = g.createRadialGradient(x, y, 0.5, x, y, 7);
        gr.addColorStop(0, 'rgba(255,225,150,.8)');
        gr.addColorStop(1, 'rgba(255,200,110,0)');
        g.fillStyle = gr;
        g.fillRect(x - 7, y - 7, 14, 14);
      }
      return;
    }
    shadow(S, PLAZA, 20);
    disc(g, 'rgba(25,25,35,.22)', 17, 17, 6.4);
    solid(g, [12.2, 12.2, 19.8, 19.8], 0, 2.6, '#cdbf9c', {
      detailL: (w) => {
        for (let x = 0.5; x < w; x += 1.6) rr(g, 'rgba(60,50,40,.22)', x, -2.4, 0.4, 2.2);
      },
    });
    solid(g, [14.4, 19.8, 17.6, 21.3], 0, 1.3, '#bfb190');
    solid(g, [19.8, 14.4, 21.3, 17.6], 0, 1.3, '#bfb190');
    const white = '#f4efe2';
    const posts = [[12.9, 12.9], [19.1, 12.9], [12.9, 19.1], [19.1, 19.1]];
    const pillar = (u, v) => {
      const [x, y] = P(u, v, 2.6);
      g.fillStyle = grad(g, x - 0.6, 0, x + 0.6, 0, white, dark(white, 0.3));
      g.fillRect(x - 0.6, y - 9.6, 1.2, 9.6);
    };
    pillar(...posts[0]);
    pillar(...posts[1]);
    // Railings between the front posts.
    for (const [a, b] of [[posts[2], posts[3]], [posts[1], posts[3]]]) {
      for (const z of [3.4, 5]) stroke(g, white, 0.7, [P(a[0], a[1], z), P(b[0], b[1], z)]);
    }
    pillar(...posts[2]);
    pillar(...posts[3]);
    const [u0, v0, u1, v1, zE, rise, o] = [12, 12, 20, 20, 13.2, 7.5, 1.2];
    // The roof is a pyramid with red and cream stripes.
    const apex = [16, 16, zE + rise];
    const stripes = 6;
    const faceStripes = (a, b, shade) => {
      for (let k = 0; k < stripes; k++) {
        const t0 = k / stripes;
        const t1 = (k + 1) / stripes;
        const col = k % 2 ? '#f4efe2' : roof;
        const p0 = [lerp(a[0], b[0], t0), lerp(a[1], b[1], t0), zE];
        const p1 = [lerp(a[0], b[0], t1), lerp(a[1], b[1], t1), zE];
        fp3(g, shade ? dark(col, 0.26) : col, p0, p1, apex);
      }
    };
    faceStripes([u0 - o, v1 + o], [u1 + o, v1 + o], false);
    faceStripes([u1 + o, v1 + o], [u1 + o, v0 - o], true);
    // A scalloped edge along the front eaves.
    for (let k = 0; k < 8; k++) {
      const a = lerp(u0 - o, u1 + o, k / 8);
      const b = lerp(u0 - o, u1 + o, (k + 1) / 8);
      fp3(g, k % 2 ? '#f4efe2' : roof, [a, v1 + o, zE], [b, v1 + o, zE], [(a + b) / 2, v1 + o, zE - 1.2]);
      const a2 = lerp(v1 + o, v0 - o, k / 8);
      const b2 = lerp(v1 + o, v0 - o, (k + 1) / 8);
      fp3(g, dark(k % 2 ? '#f4efe2' : roof, 0.26), [u1 + o, a2, zE], [u1 + o, b2, zE], [u1 + o, (a2 + b2) / 2, zE - 1.2]);
    }
    const [ax, ay] = P(...apex);
    line(g, '#7a5530', 0.7, ax, ay, ax, ay - 3.4);
    fp(g, '#f2c230', [[ax, ay - 3.4], [ax + 3, ay - 2.5], [ax, ay - 1.6]]);
    for (const [u, v] of lanterns) {
      const [x, y] = P(u, v, 12.2);
      rr(g, '#2b2d36', x - 0.7, y - 0.4, 1.4, 0.5);
      rr(g, '#f3c978', x - 0.6, y, 1.2, 1.4);
    }
  },
  clock(S, c) {
    const g = S.g;
    const body = c || '#c9b48e';
    const R = [14.3, 14.3, 17.7, 17.7];
    const faceAt = 30.5;
    if (!S.day) {
      const glow = (x, y) => {
        const gr = g.createRadialGradient(x, y, 0.5, x, y, 6);
        gr.addColorStop(0, 'rgba(255,230,160,.85)');
        gr.addColorStop(1, 'rgba(255,200,110,0)');
        g.fillStyle = gr;
        g.fillRect(x - 6, y - 6, 12, 12);
      };
      glow(...P(16, 17.8, faceAt));
      glow(...P(17.8, 16, faceAt));
      // The lit faces themselves.
      const lit = (w) => {
        g.fillStyle = 'rgba(255,240,180,.85)';
        g.beginPath();
        g.arc(w / 2, -faceAt, 2.7, 0, TAU);
        g.fill();
        line(g, 'rgba(120,70,20,.7)', 0.5, w / 2, -faceAt, w / 2, -faceAt - 1.9);
        line(g, 'rgba(120,70,20,.7)', 0.4, w / 2, -faceAt, w / 2 + 1.4, -faceAt + 0.5);
      };
      onL(g, R[0] - 0.4, R[3] + 0.4, () => lit(R[2] - R[0] + 0.8));
      onR(g, R[2] + 0.4, R[3] + 0.4, () => lit(R[3] - R[1] + 0.8));
      return;
    }
    shadow(S, R, 40);
    disc(g, 'rgba(25,25,35,.25)', 17.5, 17.5, 4.6);
    solid(g, [13.4, 13.4, 18.6, 18.6], 0, 4.5, lite(body, 0.05));
    solid(g, [13.9, 13.9, 18.1, 18.1], 4.5, 5.3, lite(body, 0.18));
    solid(g, R, 5.3, 26.5, body, {
      detailL: (w) => {
        for (let z = 8; z < 26; z += 2.2) rr(g, 'rgba(60,45,30,.16)', 0, -z, w, 0.35);
        rr(g, '#3a3042', w / 2 - 0.4, -22, 0.8, 3.4);
      },
      detailR: (w) => {
        for (let z = 8; z < 26; z += 2.2) rr(g, 'rgba(60,45,30,.16)', 0, -z, w, 0.35);
        rr(g, '#3a3042', w / 2 - 0.4, -22, 0.8, 3.4);
      },
    });
    solid(g, [13.8, 13.8, 18.2, 18.2], 26.5, 27.2, lite(body, 0.2));
    const clockFace = (w) => {
      const cx = w / 2;
      g.fillStyle = '#f7f1dc';
      g.beginPath();
      g.arc(cx, -faceAt, 2.7, 0, TAU);
      g.fill();
      g.strokeStyle = '#3a3042';
      g.lineWidth = 0.7;
      g.beginPath();
      g.arc(cx, -faceAt, 2.7, 0, TAU);
      g.stroke();
      for (let k = 0; k < 12; k++) {
        const a = (k / 12) * TAU;
        rr(g, '#3a3042', cx + Math.cos(a) * 2.2 - 0.15, -faceAt + Math.sin(a) * 2.2 - 0.15, 0.3, 0.3);
      }
      line(g, '#3a3042', 0.6, cx, -faceAt, cx, -faceAt - 1.9);
      line(g, '#3a3042', 0.5, cx, -faceAt, cx + 1.4, -faceAt + 0.5);
    };
    solid(g, [13.9, 13.9, 18.1, 18.1], 27.2, 34, lite(body, 0.1), { detailL: clockFace, detailR: clockFace });
    solid(g, [13.5, 13.5, 18.5, 18.5], 34, 34.8, lite(body, 0.22));
    const roof = dark(body, 0.5);
    const apex = [16, 16, 44];
    fp3(g, roof, [13.7, 18.3, 34.8], [18.3, 18.3, 34.8], apex);
    fp3(g, dark(roof, 0.28), [18.3, 18.3, 34.8], [18.3, 13.7, 34.8], apex);
    stroke(g, lite(roof, 0.3), 0.5, [P(16, 18.3, 34.8), P(...apex)]);
    const [x, y] = P(...apex);
    line(g, '#d9b44a', 0.8, x, y, x, y - 5);
    ell(g, '#e8c860', x, y - 5.4, 0.9, 0.9);
  },
  tree(S, c) {
    if (!S.day) return;
    const g = S.g;
    const cols = c ? [dark(c, 0.45), c, lite(c, 0.3), lite(c, 0.55)] : ['#2d5f26', '#4a8a36', '#7db84a', '#b4dd70'];
    disc(g, 'rgba(25,25,35,.25)', 18.5, 18.5, 6.6);
    drum(g, 16, 16, 5.8, 0, 2.6, '#a8a597', '#7a5a3a');
    disc(g, '#5a4430', 16, 16, 4.8, 2.6);
    const [x, y] = P(16, 16, 2.6);
    shadow(S, [13, 13, 19, 19], 30);
    bigTree(g, x, y, 1.15, cols, seeded(7), { trunk: ['#8a6238', '#4a321c'], limbs: 3 });
  },
  obelisk(S, c) {
    if (!S.day) return;
    const g = S.g;
    const stone = c || '#cfc8b4';
    shadow(S, [14.4, 14.4, 17.6, 17.6], 38);
    disc(g, 'rgba(25,25,35,.25)', 17.5, 17.5, 5.4);
    solid(g, [13, 13, 19, 19], 0, 1.4, lite(stone, 0.06));
    solid(g, [13.9, 13.9, 18.1, 18.1], 1.4, 2.8, stone);
    solid(g, [14.6, 14.6, 17.4, 17.4], 2.8, 4.2, lite(stone, 0.1));
    const w0 = 1.55;
    const w1 = 0.85;
    const z0 = 4.2;
    const z1 = 36;
    fp3(g, grad(g, ...P(14, 18), ...P(18, 18), lite(stone, 0.12), stone), [16 - w0, 16 + w0, z0], [16 + w0, 16 + w0, z0], [16 + w1, 16 + w1, z1], [16 - w1, 16 + w1, z1]);
    fp3(g, dark(stone, 0.28), [16 + w0, 16 + w0, z0], [16 + w0, 16 - w0, z0], [16 + w1, 16 - w1, z1], [16 + w1, 16 + w1, z1]);
    for (let k = 1; k < 6; k++) {
      const z = lerp(z0, z1, k / 6);
      const w = lerp(w0, w1, k / 6);
      stroke(g, 'rgba(60,50,40,.18)', 0.4, [P(16 - w, 16 + w, z), P(16 + w, 16 + w, z), P(16 + w, 16 - w, z)]);
    }
    stroke(g, 'rgba(255,255,255,.35)', 0.5, [P(16 - w0 * 0.8, 16 + w0 * 0.8, z0 + 1), P(16 - w1 * 0.8, 16 + w1 * 0.8, z1 - 1)]);
    const apex = [16, 16, z1 + 4.2];
    fp3(g, '#e9c85e', [16 - w1, 16 + w1, z1], [16 + w1, 16 + w1, z1], apex);
    fp3(g, '#b8963a', [16 + w1, 16 + w1, z1], [16 + w1, 16 - w1, z1], apex);
  },
};

export function paintCenterpiece(S, outside) {
  const style = styleOf(outside, 'centerpiece', Object.keys(CENTERS), 'fountain');
  CENTERS[style](S, hexOf(partOf(outside, 'centerpiece').color, null), outside);
}

// ---------------------------------------------------------------------------------------------
// String lights (E10): round the town square, from post to post. Each is an unlit fitting by day and
// glows at night.

const LIGHT_COLORS = { eaves: '#ffe3a0', fairy: '#ffe3a0', festoon: '#ffe3a0', icicle: '#dff4ff', neon: '#ff6fb0' };
const STRING_LOOP = [[3, 3], [16, 3], [29, 3], [29, 16], [29, 29], [16, 29], [3, 29], [3, 16]];
const POST_TOP = 15;

function glow(g, x, y, r, color, a) {
  const gr = g.createRadialGradient(x, y, 0, x, y, r);
  gr.addColorStop(0, rgba(color, a));
  gr.addColorStop(1, rgba(color, 0));
  g.fillStyle = gr;
  g.fillRect(x - r, y - r, r * 2, r * 2);
}

// A point along a sagging line from `a` to `b`.
const along = (a, b, sag, t) => {
  const cx = (a[0] + b[0]) / 2;
  const cy = (a[1] + b[1]) / 2 + sag * 2;
  return [(1 - t) * (1 - t) * a[0] + 2 * t * (1 - t) * cx + t * t * b[0], (1 - t) * (1 - t) * a[1] + 2 * t * (1 - t) * cy + t * t * b[1]];
};

function wire(g, color, w, a, b, sag) {
  g.strokeStyle = color;
  g.lineWidth = w;
  g.beginPath();
  g.moveTo(a[0], a[1]);
  g.quadraticCurveTo((a[0] + b[0]) / 2, (a[1] + b[1]) / 2 + sag * 2, b[0], b[1]);
  g.stroke();
}

const STRINGS = {
  none() {},
  eaves(S, a, b, c, rand) {
    const g = S.g;
    const n = Math.max(2, Math.round(Math.hypot(b[0] - a[0], b[1] - a[1]) / 3));
    if (S.day) wire(g, '#3a3042', 0.5, a, b, 0.5);
    for (let i = 1; i < n; i++) {
      const [x, y] = along(a, b, 0.5, i / n);
      if (S.day) {
        rr(g, '#3a3042', x - 0.3, y, 0.6, 0.7);
        ell(g, mixHex(c, '#8a8070', 0.35), x, y + 1.3, 0.75, 0.9);
        ell(g, 'rgba(255,255,255,.5)', x - 0.25, y + 1, 0.25, 0.3);
      } else {
        glow(g, x, y + 1.3, 4.4, c, 0.5);
        ell(g, lite(c, 0.5), x, y + 1.3, 0.8, 0.95);
      }
    }
    void rand;
  },
  fairy(S, a, b, c, rand) {
    const g = S.g;
    const n = Math.max(3, Math.round(Math.hypot(b[0] - a[0], b[1] - a[1]) / 1.7));
    if (S.day) wire(g, '#3d5a35', 0.4, a, b, 2);
    for (let i = 1; i < n; i++) {
      const [x, y] = along(a, b, 2, i / n);
      const yy = y + 0.5 + rand() * 0.8;
      if (S.day) ell(g, mixHex(c, '#8a8070', 0.35), x, yy, 0.5, 0.5);
      else {
        glow(g, x, yy, 2.6, c, 0.4);
        ell(g, lite(c, 0.6), x, yy, 0.55, 0.55);
      }
    }
  },
  festoon(S, a, b, c, rand) {
    const g = S.g;
    const mid = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2 + 2.5];
    for (const [p, q] of [[a, mid], [mid, b]]) {
      const n = Math.max(2, Math.round(Math.hypot(q[0] - p[0], q[1] - p[1]) / 3));
      if (S.day) wire(g, '#3a3042', 0.55, p, q, 3.2);
      for (let i = 1; i < n; i++) {
        const [x, y] = along(p, q, 3.2, i / n);
        if (S.day) {
          rr(g, '#3a3042', x - 0.4, y, 0.8, 1);
          ell(g, mixHex(c, '#8a8070', 0.3), x, y + 2.2, 1.1, 1.35);
          ell(g, 'rgba(255,255,255,.5)', x - 0.35, y + 1.7, 0.35, 0.5);
        } else {
          glow(g, x, y + 2.2, 6, c, 0.55);
          ell(g, lite(c, 0.5), x, y + 2.2, 1.1, 1.35);
        }
      }
    }
    void rand;
  },
  icicle(S, a, b, c, rand) {
    const g = S.g;
    const n = Math.max(3, Math.round(Math.hypot(b[0] - a[0], b[1] - a[1]) / 1.3));
    if (S.day) wire(g, '#6c7480', 0.45, a, b, 0.6);
    for (let i = 1; i < n; i++) {
      const [x, y] = along(a, b, 0.6, i / n);
      const len = 1.5 + rand() * 3.4;
      if (S.day) {
        line(g, mixHex(c, '#7a8a9a', 0.35), 0.6, x, y, x, y + len);
        ell(g, mixHex(c, '#9aa8b4', 0.3), x, y + len, 0.4, 0.4);
      } else {
        glow(g, x, y + len, 3, c, 0.45);
        line(g, lite(c, 0.4), 0.55, x, y, x, y + len);
        ell(g, '#ffffff', x, y + len, 0.45, 0.45);
      }
    }
  },
  neon(S, a, b, c) {
    const g = S.g;
    const sag = 0.4;
    if (S.day) {
      wire(g, mixHex(c, '#d8d4d0', 0.6), 1.2, a, b, sag);
      wire(g, 'rgba(255,255,255,.5)', 0.4, [a[0], a[1] - 0.3], [b[0], b[1] - 0.3], sag);
      for (let i = 1; i < 4; i++) {
        const [x, y] = along(a, b, sag, i / 4);
        rr(g, '#3a3042', x - 0.3, y - 0.2, 0.6, 1.3);
      }
    } else {
      wire(g, rgba(c, 0.12), 6, a, b, sag);
      wire(g, rgba(c, 0.26), 3.4, a, b, sag);
      wire(g, rgba(c, 0.9), 1.5, a, b, sag);
      wire(g, rgba(lite(c, 0.7), 0.95), 0.6, a, b, sag);
    }
  },
};

export function paintStringLights(S, outside) {
  const style = styleOf(outside, 'string_lights', Object.keys(STRINGS), 'none');
  if (style === 'none') return;
  const g = S.g;
  const c = hexOf(partOf(outside, 'string_lights').color, LIGHT_COLORS[style]);
  const tops = STRING_LOOP.map(([u, v]) => P(u, v, POST_TOP));
  const rand = seeded(5);
  if (S.day) {
    for (const [u, v] of STRING_LOOP) {
      const [x, y] = P(u, v);
      ell(g, STONE, x + 1, y + 0.4, 2.4, 0.9);
      g.fillStyle = grad(g, x - 0.7, 0, x + 0.7, 0, '#7a5a3a', '#3e2c1c');
      g.fillRect(x - 0.7, y - POST_TOP, 1.4, POST_TOP);
      ell(g, '#5a4430', x, y - POST_TOP, 1, 0.55);
    }
  }
  for (let i = 0; i < tops.length; i++) STRINGS[style](S, tops[i], tops[(i + 1) % tops.length], c, rand);
  void g;
}

// ---------------------------------------------------------------------------------------------
// Things in the yard (E15). Each is painted standing on its cells, in the day pass only, unless it gives
// light. A cell is 2 units, which is about 4 pixels across, so a thing needs a clear outline first.

// The points of a ring on the ground, at height z.
function ring(cu, cv, r, n, z = 0, rot = 0) {
  return Array.from({ length: n }, (_, i) => {
    const a = rot + (i / n) * TAU;
    return [cu + Math.cos(a) * r, cv + Math.sin(a) * r, z];
  });
}
const rectRing = (u0, v0, u1, v1, z = 0) => [[u0, v0, z], [u1, v0, z], [u1, v1, z], [u0, v1, z]];

// A solid with any outline: the sides you can see, darkest where they face right, then the top.
function prism(g, top, bot, color, topColor) {
  let area = 0;
  for (let i = 0; i < top.length; i++) {
    const j = (i + 1) % top.length;
    area += top[i][0] * top[j][1] - top[j][0] * top[i][1];
  }
  const s = area >= 0 ? 1 : -1;
  const faces = [];
  for (let i = 0; i < top.length; i++) {
    const j = (i + 1) % top.length;
    const nu = (top[j][1] - top[i][1]) * s;
    const nv = -(top[j][0] - top[i][0]) * s;
    if (nu + nv <= 0.0001) continue;
    const k = Math.min(1, Math.max(0, ((nu - nv) / Math.hypot(nu, nv) + 1) / 2));
    faces.push({ i, j, k, y: P(top[i][0], top[i][1], top[i][2])[1] + P(top[j][0], top[j][1], top[j][2])[1] });
  }
  faces.sort((a, b) => a.y - b.y);
  for (const f of faces) fp3(g, mixHex(lite(color, 0.1), dark(color, 0.36), f.k), top[f.i], top[f.j], bot[f.j], bot[f.i]);
  if (topColor !== null) fp3(g, topColor || lite(color, 0.18), ...top);
}

// A round blob of leaves.
function puff(g, x, y, r, cols) {
  ell(g, cols[0], x + 0.3, y + r * 0.25, r * 1.1, r);
  ell(g, cols[1], x - 0.2, y - r * 0.1, r, r * 0.92);
  ell(g, rgba(cols[2], 0.9), x - r * 0.3, y - r * 0.38, r * 0.55, r * 0.45);
}
const leafCols = (c) => [dark(c, 0.45), c, lite(c, 0.32)];

// Where an item stands, and randomness of its own.
function where(S, item, salt = 0) {
  const [x, y] = P(item.u, item.v);
  return { g: S.g, x, y, rand: seeded(ih(item.x, item.y, 7 + salt)) };
}

// A circle drawn flat in a vertical plane that runs along u (or along v), such as a wheel.
function upright(g, alongU, u, v, fn) {
  if (alongU) onL(g, u, v, fn);
  else onR(g, u, v, fn);
}

// A tree: a flared trunk and a cauliflower canopy, drawn at `k` times the map's size.
function bigTree(g, x, y, k, cols, rand, o = {}) {
  g.save();
  g.translate(x, y);
  g.scale(k, k);
  const trunk = o.trunk || ['#8a6238', '#4a321c'];
  fp(g, grad(g, -4, 0, 4, 0, trunk[0], trunk[1]), [[-4.2, 0], [-2.6, -6], [-1.9, -16], [1.9, -16], [2.6, -6], [4.2, 0]]);
  if (o.mark) o.mark(g);
  line(g, trunk[1], 1.3, 0, -15, -6, -22);
  line(g, trunk[1], 1.3, 0, -14, 6, -21);
  canopy(g, 0, o.cy ?? -27, o.rx ?? 17, o.ry ?? 14, cols, rand, o.n ?? 17, 1);
  if (o.dress) o.dress(g, rand);
  g.restore();
}

// A tree standing on its cells, with its shadow.
function tree(S, item, k, cols, o = {}) {
  const { g, x, y, rand } = where(S, item);
  shadow(S, [item.u0, item.v0, item.u1, item.v1], o.shadow ?? 22);
  pool(g, item.u, item.v, Math.max(item.w, item.h) * 1.15, 0.28);
  if (o.under) o.under(g, x, y, rand);
  bigTree(g, x, y, k, cols, rand, o);
}

const OAK = ['#2d5f26', '#4a8a36', '#7db84a', '#b4dd70'];
const OAK2 = ['#365f20', '#58862d', '#8eb83e', '#c2dc68'];
const BIRCH = ['#5f8a3a', '#8db84e', '#b9d96e', '#e1f19a'];
const MAPLE = ['#8a2a1c', '#c4472a', '#e8803a', '#f7b865'];
const BLOSSOM = ['#b7507a', '#e07fa6', '#f7b6cf', '#ffe0ec'];

// A scatter of marks on the ground round a tree's foot, such as fallen petals.
function litter(g, x, y, rand, colors, n, spread) {
  for (let i = 0; i < n; i++) {
    const a = rand() * TAU;
    const d = 2 + rand() * spread;
    ell(g, colors[Math.floor(rand() * colors.length)], x + Math.cos(a) * d * 1.4, y + Math.sin(a) * d * 0.6 + 1, 0.8, 0.5);
  }
}

// Branches that fork, for a tree with no leaves.
function branch(g, x, y, ang, len, w, rand, depth, color) {
  const x1 = x + Math.cos(ang) * len;
  const y1 = y + Math.sin(ang) * len;
  line(g, color, w, x, y, x1, y1);
  if (depth <= 0) return;
  branch(g, x1, y1, ang - 0.5 - rand() * 0.4, len * 0.72, w * 0.7, rand, depth - 1, color);
  branch(g, x1, y1, ang + 0.5 + rand() * 0.4, len * 0.72, w * 0.7, rand, depth - 1, color);
  if (rand() < 0.5) branch(g, x1, y1, ang + (rand() - 0.5) * 0.4, len * 0.6, w * 0.6, rand, depth - 2, color);
}

const TREES = {
  oak(S, item) {
    if (!S.day) return;
    tree(S, item, 0.62, item.x % 2 ? OAK : OAK2);
  },
  pine(S, item) {
    if (!S.day) return;
    const { g, x, y } = where(S, item);
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 26);
    pool(g, item.u, item.v, 2.6, 0.28);
    g.save();
    g.translate(x, y);
    g.scale(0.66, 0.66);
    drawTree(g, 0, 0, 'pine', item.x + item.y);
    g.restore();
  },
  birch(S, item) {
    if (!S.day) return;
    tree(S, item, 0.62, BIRCH, {
      trunk: ['#f1ede2', '#bdb6a6'],
      cy: -29,
      rx: 14,
      ry: 14,
      n: 15,
      mark: (g) => {
        for (const [yy, w] of [[-3, 3], [-7, 2], [-10.5, 3.2], [-14, 2.2]]) rr(g, '#2e2a2a', -w / 2 - 0.4, yy, w, 1.1);
      },
    });
  },
  maple(S, item) {
    if (!S.day) return;
    tree(S, item, 0.62, MAPLE, {
      under: (g, x, y, rand) => litter(g, x, y, rand, ['#c4472a', '#e8803a', '#f7b865'], 14, 6),
    });
  },
  cherry_blossom(S, item) {
    if (!S.day) return;
    tree(S, item, 0.6, BLOSSOM, {
      trunk: ['#6f4a3a', '#3b2820'],
      under: (g, x, y, rand) => litter(g, x, y, rand, ['#f7b6cf', '#ffe0ec', '#e07fa6'], 20, 7),
      dress: (g, rand) => {
        for (let i = 0; i < 10; i++) {
          const a = rand() * TAU;
          ell(g, '#fff1f6', Math.cos(a) * 12 * rand(), -27 + Math.sin(a) * 9 * rand(), 0.9, 0.9);
        }
      },
    });
  },
  apple_tree(S, item) {
    if (!S.day) return;
    tree(S, item, 0.58, OAK, {
      cy: -24,
      rx: 16,
      ry: 12,
      n: 15,
      dress: (g, rand) => {
        for (let i = 0; i < 9; i++) {
          const a = rand() * TAU;
          const d = Math.sqrt(rand());
          const px = Math.cos(a) * d * 11;
          const py = -24 + Math.sin(a) * d * 8;
          ell(g, '#d93a3a', px, py, 1.5, 1.5);
          ell(g, 'rgba(255,255,255,.5)', px - 0.4, py - 0.5, 0.45, 0.4);
        }
      },
      under: (g, x, y) => {
        ell(g, '#d93a3a', x + 3.5, y + 1.8, 0.8, 0.7);
        ell(g, '#d93a3a', x - 4, y + 2.4, 0.8, 0.7);
      },
    });
  },
  palm(S, item) {
    if (!S.day) return;
    const { g, x, y, rand } = where(S, item);
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 24);
    pool(g, item.u, item.v, 2.6, 0.28);
    g.save();
    g.translate(x, y);
    g.scale(0.7, 0.7);
    // The trunk leans and is ringed.
    const top = [7, -36];
    for (const [w, col] of [[4.2, '#5a3d22'], [3.2, '#8a6a42'], [1.1, '#a98456']]) {
      g.strokeStyle = col;
      g.lineWidth = w;
      g.beginPath();
      g.moveTo(0, 0);
      g.quadraticCurveTo(-4, -18, top[0], top[1]);
      g.stroke();
    }
    for (let t = 0.1; t < 1; t += 0.09) {
      const px = 2 * t * (1 - t) * -4 + t * t * top[0];
      const py = 2 * t * (1 - t) * -18 + t * t * top[1];
      rr(g, 'rgba(40,25,10,.4)', px - 2, py, 4, 0.7);
    }
    for (let k = 0; k < 9; k++) {
      const a = (k / 9) * TAU + 0.3;
      const len = 15 + rand() * 4;
      const tx = top[0] + Math.cos(a) * len;
      const ty = top[1] + Math.sin(a) * len * 0.45 + len * 0.3;
      for (const [w, col] of [[3, '#1f5a2c'], [2, '#3f8f3c'], [0.7, '#8ccf5c']]) {
        g.strokeStyle = col;
        g.lineWidth = w;
        g.beginPath();
        g.moveTo(top[0], top[1]);
        g.quadraticCurveTo(top[0] + Math.cos(a) * len * 0.6, top[1] - 6 + Math.sin(a) * len * 0.2, tx, ty);
        g.stroke();
      }
    }
    for (const dx of [-1.6, 0, 1.6]) ell(g, '#5a3a1c', top[0] + dx, top[1] + 2.4 + Math.abs(dx) * 0.3, 1.5, 1.5);
    g.restore();
  },
  willow(S, item) {
    if (!S.day) return;
    const { g, x, y, rand } = where(S, item);
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 26);
    pool(g, item.u, item.v, 4.4, 0.28);
    g.save();
    g.translate(x, y);
    g.scale(0.72, 0.72);
    fp(g, grad(g, -5, 0, 5, 0, '#7a5a38', '#43301c'), [[-6, 0], [-3.4, -7], [-2.6, -17], [2.6, -17], [3.4, -7], [6, 0]]);
    line(g, '#43301c', 1.6, 0, -16, -8, -24);
    line(g, '#43301c', 1.6, 0, -15, 8, -23);
    const cols = ['#40702a', '#6a9a3c', '#9cc654', '#cde383'];
    canopy(g, 0, -29, 20, 9, cols, rand, 11, 0.95);
    // The long strands, hanging and swaying.
    for (let i = 0; i < 40; i++) {
      const a = (i / 40) * TAU;
      const sx = Math.cos(a) * (14 + rand() * 6);
      const sy = -26 + Math.sin(a) * 4;
      const len = 9 + rand() * 11;
      g.strokeStyle = rand() < 0.5 ? rgba(cols[1], 0.9) : rgba(cols[2], 0.85);
      g.lineWidth = 0.9;
      g.beginPath();
      g.moveTo(sx, sy);
      g.quadraticCurveTo(sx + (rand() - 0.5) * 3, sy + len * 0.5, sx + (rand() - 0.5) * 2.4, sy + len);
      g.stroke();
    }
    g.restore();
  },
  cypress(S, item) {
    if (!S.day) return;
    const { g, x, y, rand } = where(S, item);
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 22);
    pool(g, item.u, item.v, 1.6, 0.28);
    rr(g, '#4a321c', x - 0.7, y - 4, 1.4, 4);
    for (let i = 0; i < 10; i++) {
      const w = 3.9 * (1 - (i / 11) ** 1.6);
      const yy = y - 6 - i * 2.9;
      ell(g, '#1c4a2a', x + 0.3, yy + 0.8, w, 3.1);
      ell(g, '#2d6a3a', x - 0.2, yy, w * 0.92, 2.9);
      ell(g, rgba('#6aa856', 0.7), x - w * 0.35, yy - 0.8, w * 0.4, 1.2);
    }
    ell(g, '#2d6a3a', x, y - 36, 1.1, 2.4);
    void rand;
  },
  dead_tree(S, item) {
    if (!S.day) return;
    const { g, x, y, rand } = where(S, item);
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 18);
    pool(g, item.u, item.v, 2.4, 0.26);
    g.save();
    g.translate(x, y);
    g.scale(0.66, 0.66);
    const bark = '#5d5046';
    fp(g, grad(g, -4, 0, 4, 0, '#7a6a5a', '#3d332b'), [[-4.4, 0], [-2.6, -8], [-2, -18], [-1, -24], [1.6, -24], [2.2, -18], [2.8, -8], [4.6, 0]]);
    rr(g, 'rgba(20,14,10,.5)', -1, -14, 1.2, 3);
    branch(g, 0, -22, -1.9, 12, 2, rand, 3, bark);
    branch(g, 0, -20, -1.2, 14, 2.2, rand, 3, bark);
    branch(g, 0, -17, -0.4, 9, 1.4, rand, 2, bark);
    branch(g, 0, -17, -2.7, 9, 1.4, rand, 2, bark);
    for (let i = 0; i < 4; i++) ell(g, i % 2 ? '#8a6a3a' : '#a07a42', (rand() - 0.5) * 22, -24 - rand() * 14, 1, 0.6);
    g.restore();
  },
};

// ---------------------------------------------------------------------------------------------
// Plants

const PLANTS = {
  bush(S, item) {
    if (!S.day) return;
    const { g, x, y } = where(S, item);
    const cols = leafCols(colorIn(item, '#4c9440'));
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 5);
    pool(g, item.u, item.v, 1.7, 0.28);
    for (const [dx, dy, r] of [[-1.9, -2, 2.3], [2, -2, 2.3], [-0.2, -1, 2.5], [0.1, -4, 2.4], [-1.5, -0.4, 1.9], [1.7, -0.3, 1.9]]) puff(g, x + dx, y + dy, r, cols);
  },
  topiary(S, item) {
    if (!S.day) return;
    const { g, x, y } = where(S, item);
    const c = colorIn(item, '#3f8a3a');
    const cols = leafCols(c);
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 10);
    pool(g, item.u, item.v, 1.5, 0.28);
    quad(g, grad(g, x - 2, 0, x + 2, 0, '#d9825a', '#9a4e30'), [x - 1.9, y - 3.2], [x + 1.9, y - 3.2], [x + 1.4, y], [x - 1.4, y]);
    ell(g, '#e69468', x, y - 3.2, 1.9, 0.7);
    rr(g, '#5a3d22', x - 0.35, y - 6, 0.7, 3);
    const style = (item.x + item.y) % 3;
    if (style === 0) {
      puff(g, x, y - 8.2, 3, cols);
      puff(g, x, y - 12.2, 1.9, cols);
    } else if (style === 1) {
      quad(g, cols[1], [x - 3, y - 6], [x + 3, y - 6], [x + 0.2, y - 14], [x - 0.2, y - 14]);
      quad(g, cols[0], [x + 3, y - 6], [x + 0.2, y - 14], [x + 0.9, y - 12]);
      quad(g, rgba(cols[2], 0.8), [x - 3, y - 6], [x - 0.3, y - 14], [x - 1.2, y - 9]);
      ell(g, cols[0], x, y - 6, 3, 0.9);
    } else {
      for (const [yy, r] of [[-7, 3], [-10, 2.5], [-12.6, 1.8]]) puff(g, x, y + yy, r, cols);
    }
  },
  rose_bush(S, item) {
    if (!S.day) return;
    const { g, x, y, rand } = where(S, item);
    const rose = colorIn(item, '#d8364a');
    const cols = leafCols('#3f7a34');
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 6);
    pool(g, item.u, item.v, 1.7, 0.28);
    for (const [dx, dy, r] of [[-1.9, -2, 2.2], [2, -2, 2.2], [0, -3.4, 2.4], [-0.6, -0.8, 2], [1.6, -0.6, 1.9]]) puff(g, x + dx, y + dy, r, cols);
    for (let i = 0; i < 7; i++) {
      const px = x + (rand() - 0.5) * 6.4;
      const py = y - 1 - rand() * 4.6;
      ell(g, dark(rose, 0.25), px, py + 0.2, 1.05, 0.95);
      ell(g, rose, px, py, 0.95, 0.85);
      ell(g, lite(rose, 0.4), px - 0.2, py - 0.2, 0.45, 0.4);
    }
  },
  hydrangea(S, item) {
    if (!S.day) return;
    const { g, x, y, rand } = where(S, item);
    const c = colorIn(item, '#7a9be0');
    const cols = leafCols('#3f7a34');
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 7);
    pool(g, item.u, item.v, 1.7, 0.28);
    for (const [dx, dy, r] of [[-1.8, -1.8, 2.2], [2, -1.8, 2.2], [0, -1, 2.4]]) puff(g, x + dx, y + dy, r, cols);
    for (const [dx, dy, r] of [[-2, -4.2, 1.9], [1.7, -4.8, 2], [-0.2, -6.4, 1.8], [0.2, -3.2, 1.6]]) {
      ell(g, dark(c, 0.3), x + dx + 0.2, y + dy + 0.3, r * 1.05, r);
      ell(g, c, x + dx, y + dy, r, r * 0.92);
      for (let i = 0; i < 9; i++) ell(g, rand() < 0.5 ? lite(c, 0.5) : lite(c, 0.25), x + dx + (rand() - 0.5) * r * 1.6, y + dy + (rand() - 0.5) * r * 1.4, 0.45, 0.4);
    }
  },
  fern(S, item) {
    if (!S.day) return;
    const { g, x, y, rand } = where(S, item);
    pool(g, item.u, item.v, 1.5, 0.26);
    const fronds = [-2.2, -1.6, -1.1, -0.55, -0.1, 0.45, 1.0];
    for (const [i, a] of fronds.entries()) {
      const ang = -1.5 + a * 0.62;
      const len = 7 + rand() * 2;
      const tx = x + Math.sin(ang) * len;
      const ty = y - Math.cos(ang) * len * 0.8;
      const col = i % 2 ? '#3f8a3a' : '#5ca545';
      g.strokeStyle = col;
      g.lineWidth = 0.7;
      g.beginPath();
      g.moveTo(x, y);
      g.quadraticCurveTo(x + Math.sin(ang) * len * 0.45, y - len * 0.9, tx, ty + 1.6);
      g.stroke();
      for (let t = 0.2; t < 1; t += 0.14) {
        const px = lerp(x, tx, t);
        const py = lerp(y, ty + 1.6, t) - Math.sin(t * 3.14) * len * 0.35;
        stroke(g, col, 0.5, [[px - 1.1, py + 0.5], [px + 1.1, py + 0.5]]);
      }
    }
  },
  tall_grass(S, item) {
    if (!S.day) return;
    const { g, x, y, rand } = where(S, item);
    pool(g, item.u, item.v, 1.4, 0.22);
    for (let i = 0; i < 16; i++) {
      const bx = x + (rand() - 0.5) * 4;
      const h = 5 + rand() * 4;
      const lean = (rand() - 0.5) * 4;
      g.strokeStyle = i % 3 === 0 ? '#9db858' : i % 3 === 1 ? '#5f9a40' : '#3f7a34';
      g.lineWidth = 0.8;
      g.beginPath();
      g.moveTo(bx, y + (rand() - 0.5));
      g.quadraticCurveTo(bx + lean * 0.3, y - h * 0.6, bx + lean, y - h);
      g.stroke();
      if (i % 4 === 0) ell(g, '#d6c070', bx + lean, y - h - 0.6, 0.5, 1.1);
    }
  },
  cactus(S, item) {
    if (!S.day) return;
    const { g, x, y } = where(S, item);
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 11);
    disc(g, '#d9c288', item.u, item.v, 1.9);
    disc(g, 'rgba(25,25,35,.22)', item.u + 0.5, item.v + 0.5, 1.2);
    const body = '#5f9a55';
    const part = (cx, w, y0, y1, flip) => {
      g.fillStyle = grad(g, cx - w, 0, cx + w, 0, lite(body, 0.3), body, dark(body, 0.35));
      g.fillRect(cx - w, y1, w * 2, y0 - y1);
      ell(g, lite(body, 0.18), cx, y1, w, w * 0.6);
      void flip;
    };
    part(x, 1.5, y, y - 10);
    // Two arms, each going out and then up.
    for (const s of [-1, 1]) {
      const up = s < 0 ? 5 : 6.5;
      rr(g, body, s < 0 ? x - 3.6 : x + 1.4, y - 5.2 - (s < 0 ? 0 : 1.5), 2.2, 1.7);
      part(x + s * 3.5, 0.95, y - 4 - (s < 0 ? 0 : 1.5), y - up - 4);
    }
    for (let k = -1; k <= 1; k++) rr(g, 'rgba(30,60,30,.35)', x + k * 0.6, y - 9.5, 0.2, 9);
    for (let i = 0; i < 12; i++) rr(g, '#f3eed4', x - 1.2 + (i % 3) * 1.1, y - 1.5 - i * 0.7, 0.35, 0.35);
    ell(g, '#f4768a', x, y - 11, 1, 0.8);
    ell(g, '#ffd54a', x, y - 11, 0.4, 0.3);
  },
  bamboo_clump(S, item) {
    if (!S.day) return;
    const { g, x, y, rand } = where(S, item);
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 16);
    pool(g, item.u, item.v, 1.5, 0.26);
    const poles = [[-2, 17], [-0.8, 21], [0.6, 19], [1.9, 15], [0, 13]].map(([dx, h]) => [dx, h + rand() * 2]);
    poles.sort((a, b) => a[1] - b[1]);
    for (const [dx, h] of poles) {
      const bx = x + dx;
      g.fillStyle = grad(g, bx - 0.7, 0, bx + 0.7, 0, '#c8d37a', '#8c9c3e');
      g.fillRect(bx - 0.65, y - h, 1.3, h);
      for (let z = 2.5; z < h; z += 3) rr(g, 'rgba(60,70,20,.6)', bx - 0.75, y - z, 1.5, 0.4);
      for (let k = 0; k < 4; k++) {
        const a = -0.9 + k * 0.6;
        const lx = bx + Math.sin(a) * 4.6;
        const ly = y - h + 1.5 - Math.cos(a) * 1.2 + k * 0.3;
        stroke(g, k % 2 ? '#4f8f3c' : '#6fae4c', 1.1, [[bx, y - h + 1], [lx, ly + 2]]);
      }
    }
  },
  mushroom_ring(S, item) {
    if (!S.day) return;
    const { g, rand } = where(S, item);
    disc(g, rgba('#3f7a34', 0.4), item.u, item.v, 2.6);
    const caps = [];
    for (let i = 0; i < 7; i++) {
      const a = (i / 7) * TAU + 0.3;
      caps.push([item.u + Math.cos(a) * 1.75, item.v + Math.sin(a) * 1.75, 0.7 + rand() * 0.4]);
    }
    caps.sort((a, b) => a[0] + a[1] - b[0] - b[1]);
    for (const [u, v, s] of caps) {
      const [x, y] = P(u, v);
      ell(g, 'rgba(25,25,35,.25)', x + 0.5, y + 0.3, 1.5 * s, 0.6 * s);
      rr(g, '#f1e8d2', x - 0.35 * s, y - 2.2 * s, 0.7 * s, 2.2 * s);
      ell(g, '#c93a36', x, y - 2.4 * s, 1.7 * s, 1.1 * s);
      ell(g, lite('#c93a36', 0.25), x - 0.4 * s, y - 2.8 * s, 0.9 * s, 0.4 * s);
      for (const [dx, dy] of [[-0.6, -2.4], [0.5, -2.7], [0.8, -2.2]]) ell(g, '#fffaf0', x + dx * s, y + dy * s, 0.28 * s, 0.22 * s);
    }
  },
};

// ---------------------------------------------------------------------------------------------
// Flowers

const FLOWER_MIX = ['#ff7fa6', '#ffd75e', '#fffbe8', '#c9a9ff', '#ff9a5a'];

// A little flower on a stem.
function bloom(g, x, y, h, color, r = 1) {
  line(g, '#3d6f2a', 0.6, x, y, x, y - h);
  ell(g, dark(color, 0.2), x, y - h + 0.15, r * 1.1, r * 0.95);
  ell(g, color, x - 0.1, y - h, r, r * 0.9);
  rr(g, '#f2c230', x - 0.25, y - h - 0.25, 0.5, 0.5);
}

const FLOWERS = {
  flower_bed(S, item) {
    if (!S.day) return;
    const { g, rand } = where(S, item);
    const c = hexOf(item.color, null);
    const R = [item.u0 + 0.1, item.v0 + 0.1, item.u1 - 0.1, item.v1 - 0.1];
    solid(g, R, 0, 1.4, '#a9926a');
    fp3(g, '#4f3a26', [R[0] + 0.45, R[1] + 0.45, 1.4], [R[2] - 0.45, R[1] + 0.45, 1.4], [R[2] - 0.45, R[3] - 0.45, 1.4], [R[0] + 0.45, R[3] - 0.45, 1.4]);
    const spots = [];
    for (let i = 0; i < 18; i++) spots.push([R[0] + 0.8 + rand() * (R[2] - R[0] - 1.6), R[1] + 0.7 + rand() * (R[3] - R[1] - 1.4), rand()]);
    spots.sort((a, b) => a[0] + a[1] - b[0] - b[1]);
    for (const [u, v, t] of spots) {
      const [x, y] = P(u, v, 1.4);
      ell(g, '#3f7a34', x, y - 0.3, 0.9, 0.6);
      bloom(g, x, y, 2 + t * 1.8, c ? (t < 0.65 ? c : lite(c, 0.45)) : FLOWER_MIX[Math.floor(t * 5) % 5], 1.05);
    }
  },
  tulips(S, item) {
    if (!S.day) return;
    const { g, x, y, rand } = where(S, item);
    const c = colorIn(item, '#e8484a');
    disc(g, '#5a4430', item.u, item.v, 1.9);
    disc(g, '#4f3a26', item.u, item.v, 1.5);
    for (let i = 0; i < 6; i++) {
      const px = x + (i - 2.5) * 0.85 + (rand() - 0.5) * 0.6;
      const py = y - 0.3 + (rand() - 0.5) * 1.1;
      const h = 4 + rand() * 2;
      const col = i % 3 === 2 ? lite(c, 0.35) : c;
      stroke(g, '#3f7a34', 0.7, [[px, py], [px, py - h]]);
      ell(g, '#4f9040', px + 0.7, py - h * 0.45, 1.2, 0.5);
      fp(g, col, [[px - 0.95, py - h], [px - 0.8, py - h - 1.7], [px - 0.3, py - h - 1], [px, py - h - 1.9], [px + 0.3, py - h - 1], [px + 0.8, py - h - 1.7], [px + 0.95, py - h]]);
      ell(g, col, px, py - h, 0.95, 0.5);
      ell(g, 'rgba(255,255,255,.35)', px - 0.35, py - h - 0.7, 0.25, 0.6);
    }
  },
  sunflowers(S, item) {
    if (!S.day) return;
    const { g, x, y, rand } = where(S, item);
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 12);
    pool(g, item.u, item.v, 1.4, 0.24);
    const stems = [[-1.6, 10], [1.5, 11.5], [0, 8.5]].map(([dx, h]) => [dx, h + rand()]);
    stems.sort((a, b) => a[1] - b[1]);
    for (const [dx, h] of stems) {
      const bx = x + dx;
      stroke(g, '#3f7a34', 0.9, [[bx, y], [bx, y - h]]);
      ell(g, '#4f9040', bx - 1.3, y - h * 0.45, 1.5, 0.6);
      ell(g, '#4f9040', bx + 1.3, y - h * 0.3, 1.5, 0.6);
      for (let k = 0; k < 12; k++) {
        const a = (k / 12) * TAU;
        ell(g, k % 2 ? '#f6c32b' : '#ffd84a', bx + Math.cos(a) * 1.9, y - h + Math.sin(a) * 1.9, 0.95, 0.65);
      }
      ell(g, '#5a3a1c', bx, y - h, 1.35, 1.35);
      ell(g, '#8a5a2a', bx - 0.3, y - h - 0.3, 0.6, 0.6);
    }
  },
  lavender(S, item) {
    if (!S.day) return;
    const { g, x, y, rand } = where(S, item);
    const c = colorIn(item, '#8b6fc4');
    pool(g, item.u, item.v, 1.5, 0.24);
    ell(g, '#6f8f66', x, y - 0.8, 2.6, 1.2);
    for (let i = 0; i < 12; i++) {
      const bx = x + (i - 5.5) * 0.46;
      const by = y - 1 + (rand() - 0.5);
      const h = 4.5 + rand() * 2.4;
      stroke(g, '#6f8f66', 0.4, [[bx, by], [bx + (rand() - 0.5), by - h + 2]]);
      stroke(g, i % 2 ? c : lite(c, 0.25), 1, [[bx + (rand() - 0.5) * 0.4, by - h + 2], [bx, by - h]]);
      rr(g, lite(c, 0.5), bx - 0.15, by - h, 0.3, 0.3);
    }
  },
  wildflowers(S, item) {
    if (!S.day) return;
    const { g, rand } = where(S, item);
    for (let i = 0; i < 3; i++) disc(g, rgba('#4f8a3a', 0.45), item.u + (rand() - 0.5) * 1.6, item.v + (rand() - 0.5) * 1.6, 1.2 + rand() * 0.8);
    const spots = [];
    for (let i = 0; i < 22; i++) spots.push([item.u0 + 0.5 + rand() * (item.u1 - item.u0 - 1), item.v0 + 0.5 + rand() * (item.v1 - item.v0 - 1), rand()]);
    spots.sort((a, b) => a[0] + a[1] - b[0] - b[1]);
    for (const [u, v, t] of spots) {
      const [x, y] = P(u, v);
      for (let k = -1; k <= 1; k++) line(g, '#4f8a3a', 0.5, x, y, x + k * 0.7, y - 2.4);
      bloom(g, x, y, 3 + t * 2.2, FLOWER_MIX[Math.floor(t * 50) % 5], 0.9);
    }
  },
};

// ---------------------------------------------------------------------------------------------
// The garden

// Things that make up one item, painted from the back to the front.
function inOrder(ops) {
  ops.sort((a, b) => a[0] - b[0]);
  for (const [, fn] of ops) fn();
}

// A crop on a bed, standing at a screen point.
const CROPS = [
  function cabbage(g, x, y) {
    ell(g, '#3f7a5a', x, y - 0.8, 1.5, 1.1);
    ell(g, '#86bd7a', x, y - 1.2, 1.25, 0.95);
    ell(g, '#b9dca0', x - 0.2, y - 1.5, 0.7, 0.5);
    stroke(g, '#4f8a5a', 0.4, [[x - 0.8, y - 1], [x, y - 1.8], [x + 0.8, y - 1]]);
  },
  function carrot(g, x, y) {
    ell(g, '#e8872a', x, y - 0.1, 0.7, 0.4);
    for (let k = -2; k <= 2; k++) stroke(g, k % 2 ? '#4f9a3c' : '#69b04a', 0.5, [[x, y - 0.2], [x + k * 0.55, y - 2.8 + Math.abs(k) * 0.35]]);
  },
  function tomato(g, x, y) {
    stroke(g, '#8a5d33', 0.5, [[x, y], [x, y - 5.4]]);
    puff(g, x, y - 2.8, 1.6, ['#2f6a2c', '#4f9a3c', '#8ccf5c']);
    puff(g, x, y - 4.4, 1.2, ['#2f6a2c', '#4f9a3c', '#8ccf5c']);
    for (const [dx, dy] of [[-1, -2], [0.9, -3.4], [0.1, -1.4]]) ell(g, '#d93a3a', x + dx, y + dy, 0.6, 0.6);
  },
  function lettuce(g, x, y) {
    for (let k = 0; k < 5; k++) {
      const a = (k / 5) * TAU;
      ell(g, k % 2 ? '#88c35a' : '#a9d46a', x + Math.cos(a) * 0.9, y - 0.9 + Math.sin(a) * 0.5, 0.8, 0.65);
    }
    ell(g, '#c8e48c', x, y - 1, 0.7, 0.5);
  },
];

const GARDEN = {
  vegetable_patch(S, item) {
    if (!S.day) return;
    const { g } = where(S, item);
    const f = frame(item);
    const R = [item.u0 + 0.1, item.v0 + 0.1, item.u1 - 0.1, item.v1 - 0.1];
    solid(g, R, 0, 1.1, '#a9926a');
    fq(g, '#4a3626', f, [0.45, 0.45, 1.1], [f.L - 0.45, 0.45, 1.1], [f.L - 0.45, f.W - 0.45, 1.1], [0.45, f.W - 0.45, 1.1]);
    const rows = 3;
    const plants = [];
    for (let r = 0; r < rows; r++) {
      const b = 0.85 + (r * (f.W - 1.7)) / (rows - 1);
      fq(g, '#5a4430', f, [0.6, b - 0.35, 1.1], [f.L - 0.6, b - 0.35, 1.1], [f.L - 0.6, b + 0.35, 1.1], [0.6, b + 0.35, 1.1]);
      for (let a = 1; a < f.L - 0.6; a += 1.05) {
        const [u, v, z] = f.m(a, b, 1.1);
        plants.push([u + v, CROPS[(r + (item.x & 1)) % 4], P(u, v, z)]);
      }
    }
    plants.sort((p, q) => p[0] - q[0]);
    for (const [, fn, [x, y]] of plants) fn(g, x, y);
  },
  planter(S, item) {
    if (!S.day) return;
    const { g, rand } = where(S, item);
    const c = colorIn(item, '#a5663f');
    const R = [item.u0 + 0.15, item.v0 + 0.15, item.u1 - 0.15, item.v1 - 0.15];
    shadow(S, R, 6);
    solid(g, R, 0, 3, c, {
      detailL: (w) => {
        for (let z = 1; z < 3; z += 1) rr(g, 'rgba(30,15,10,.3)', 0, -z, w, 0.3);
        rr(g, rgba(lite(c, 0.5), 0.4), 0, -3, w, 0.4);
      },
      detailR: (w) => {
        for (let z = 1; z < 3; z += 1) rr(g, 'rgba(30,15,10,.3)', 0, -z, w, 0.3);
      },
    });
    fp3(g, '#4a3626', [R[0] + 0.3, R[1] + 0.3, 3], [R[2] - 0.3, R[1] + 0.3, 3], [R[2] - 0.3, R[3] - 0.3, 3], [R[0] + 0.3, R[3] - 0.3, 3]);
    const spots = [];
    for (let i = 0; i < 9; i++) spots.push([R[0] + 0.6 + rand() * (R[2] - R[0] - 1.2), R[1] + 0.6 + rand() * (R[3] - R[1] - 1.2), rand()]);
    spots.sort((a, b) => a[0] + a[1] - b[0] - b[1]);
    for (const [u, v, t] of spots) {
      const [x, y] = P(u, v, 3);
      puff(g, x, y - 0.8, 1.4, ['#2f6a2c', '#4f9a3c', '#8ccf5c']);
      bloom(g, x, y - 0.6, 1.8 + t * 1.6, FLOWER_MIX[Math.floor(t * 5) % 5], 1);
    }
    const [x, y] = P(R[0] + (R[2] - R[0]) / 2, R[3], 3);
    for (const dx of [-1.6, 0.4, 1.8]) stroke(g, '#4f9a3c', 0.8, [[x + dx, y - 0.2], [x + dx + 0.3, y + 1.6]]);
  },
  compost_bin(S, item) {
    if (!S.day) return;
    const { g, rand } = where(S, item);
    const wood = '#7a5a38';
    const R = [item.u0 + 0.15, item.v0 + 0.15, item.u1 - 0.15, item.v1 - 0.15];
    shadow(S, R, 5);
    solid(g, R, 0, 4, wood, {
      detailL: (w) => {
        for (let z = 0.8; z < 4; z += 1.0) rr(g, 'rgba(20,12,6,.55)', 0, -z, w, 0.28);
        rr(g, 'rgba(255,240,200,.18)', 0, -4, w, 0.4);
      },
      detailR: (w) => {
        for (let z = 0.8; z < 4; z += 1.0) rr(g, 'rgba(20,12,6,.55)', 0, -z, w, 0.28);
      },
    });
    fp3(g, '#3a2a1c', [R[0] + 0.3, R[1] + 0.3, 4], [R[2] - 0.3, R[1] + 0.3, 4], [R[2] - 0.3, R[3] - 0.3, 4], [R[0] + 0.3, R[3] - 0.3, 4]);
    const [x, y] = P(item.u, item.v, 4);
    ell(g, '#4a3626', x, y - 0.3, 1.6, 0.9);
    ell(g, '#5f7a34', x - 0.6, y - 0.9, 0.9, 0.55);
    ell(g, '#e8872a', x + 0.8, y - 0.8, 0.45, 0.3);
    ell(g, '#d9c070', x + 0.1, y - 1.2, 0.5, 0.3);
    void rand;
  },
  wheelbarrow(S, item) {
    if (!S.day) return;
    const { g } = where(S, item);
    const f = frame(item);
    const c = colorIn(item, '#d8504a');
    const wood = '#8a5d33';
    const key = (a, b) => {
      const [u, v] = f.m(a, b);
      return u + v;
    };
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 5);
    pool(g, item.u, item.v, 2.1, 0.22);
    const tubTop = [[0.9, 0.1], [3.1, 0.1], [3.1, 1.9], [0.9, 1.9]].map(([a, b]) => f.m(a, b, 3.9));
    const tubBot = [[1.4, 0.55], [2.8, 0.55], [2.8, 1.45], [1.4, 1.45]].map(([a, b]) => f.m(a, b, 1.9));
    inOrder([
      [
        key(3.5, 1),
        () => {
          const [u, v] = f.m(3.55, 1);
          upright(g, f.alongU, u, v, () => {
            g.fillStyle = '#2b2d36';
            g.beginPath();
            g.arc(0, -1.3, 1.3, 0, TAU);
            g.fill();
            g.fillStyle = '#9aa0aa';
            g.beginPath();
            g.arc(0, -1.3, 0.55, 0, TAU);
            g.fill();
            g.strokeStyle = '#d3d6dc';
            g.lineWidth = 0.25;
            g.beginPath();
            g.moveTo(-1, -1.3);
            g.lineTo(1, -1.3);
            g.moveTo(0, -2.3);
            g.lineTo(0, -0.3);
            g.stroke();
          });
        },
      ],
      [
        key(2, 1),
        () => {
          for (const b of [0.55, 1.45]) {
            stroke(g, wood, 0.9, [pt(f, 3.3, b, 1.9), pt(f, 3.6, b, 1.5)]);
            stroke(g, wood, 0.9, [pt(f, 0, b, 3), pt(f, 1.4, b, 2.5)]);
            stroke(g, dark(wood, 0.3), 0.7, [pt(f, 0.6, b, 2.9), pt(f, 0.35, b, 0)]);
          }
          prism(g, tubTop, tubBot, c, dark(c, 0.45));
          for (const [a, b, r] of [[1.7, 0.8, 0.5], [2.3, 1.3, 0.6], [2.7, 0.7, 0.45], [1.5, 1.4, 0.45]]) {
            const [x, y] = pt(f, a, b, 3.8);
            ell(g, '#4a3626', x, y - 0.4, r * 1.6, r * 0.9);
            ell(g, '#5f7a34', x + 0.1, y - 0.8, r * 1.1, r * 0.6);
          }
        },
      ],
    ]);
  },
  greenhouse(S, item) {
    const g = S.g;
    const R = [item.u0 + 0.1, item.v0 + 0.1, item.u1 - 0.1, item.v1 - 0.1];
    const [u0, v0, u1, v1] = R;
    const frameC = colorIn(item, '#eef2ee');
    const zB = 1.6;
    const zE = 7.4;
    const rise = 3.6;
    const axis = item.u1 - item.u0 >= item.v1 - item.v0 ? 'u' : 'v';
    const glass = (a, b) => {
      const [, ay] = P(...a);
      const [, by] = P(...b);
      return grad(g, 0, Math.min(ay, by), 0, Math.max(ay, by), 'rgba(205,240,230,.34)', 'rgba(150,200,205,.3)');
    };
    if (!S.day) {
      // A soft glow from the plants' lamps.
      const [x, y] = P(item.u, item.v, 4);
      const gr = g.createRadialGradient(x, y, 1, x, y, 15);
      gr.addColorStop(0, 'rgba(255,225,150,.5)');
      gr.addColorStop(1, 'rgba(255,200,110,0)');
      g.fillStyle = gr;
      g.fillRect(x - 15, y - 15, 30, 30);
      fp3(g, 'rgba(255,222,140,.22)', [u0, v1, zB], [u1, v1, zB], [u1, v1, zE], [u0, v1, zE]);
      fp3(g, 'rgba(255,222,140,.16)', [u1, v1, zB], [u1, v0, zB], [u1, v0, zE], [u1, v1, zE]);
      return;
    }
    shadow(S, R, 12);
    pool(g, item.u, item.v, 3.6, 0.2);
    const rand = seeded(ih(item.x, item.y, 31));
    // The far glass first, then what grows inside, then the near glass.
    fp3(g, 'rgba(190,225,215,.22)', [u0, v0, zB], [u1, v0, zB], [u1, v0, zE], [u0, v0, zE]);
    fp3(g, 'rgba(190,225,215,.22)', [u0, v0, zB], [u0, v1, zB], [u0, v1, zE], [u0, v0, zE]);
    fp3(g, '#6e5a40', [u0, v0, 0.4], [u1, v0, 0.4], [u1, v1, 0.4], [u0, v1, 0.4]);
    const beds = [];
    for (let i = 0; i < 14; i++) beds.push([u0 + 0.8 + rand() * (u1 - u0 - 1.6), v0 + 0.8 + rand() * (v1 - v0 - 1.6), rand()]);
    beds.sort((a, b) => a[0] + a[1] - b[0] - b[1]);
    for (const [u, v, t] of beds) {
      const [x, y] = P(u, v, 0.4);
      rr(g, '#8a5d33', x - 1.4, y - 1.6, 2.8, 1.6);
      puff(g, x, y - 3.2 - t * 1.6, 1.5 + t * 0.4, leafCols(t < 0.5 ? '#3f8a3a' : '#5f9a40'));
      if (t > 0.6) ell(g, '#ff7a8a', x + 0.4, y - 4, 0.55, 0.5);
    }
    fp3(g, glass(P(u0, v1, zB), P(u0, v1, zE)), [u0, v1, zB], [u1, v1, zB], [u1, v1, zE], [u0, v1, zE]);
    fp3(g, glass(P(u1, v1, zB), P(u1, v0, zE)), [u1, v1, zB], [u1, v0, zB], [u1, v0, zE], [u1, v1, zE]);
    // The low wall all round, in the frame's colour.
    fp3(g, frameC, [u0, v1, 0], [u1, v1, 0], [u1, v1, zB], [u0, v1, zB]);
    fp3(g, dark(frameC, 0.28), [u1, v1, 0], [u1, v0, 0], [u1, v0, zB], [u1, v1, zB]);
    const fl = (u, v, z) => P(u, v, z);
    g.lineWidth = 0.45;
    for (let u = u0; u <= u1 + 0.01; u += (u1 - u0) / Math.round((u1 - u0) / 1.9)) stroke(g, frameC, 0.45, [fl(u, v1, zB), fl(u, v1, zE)]);
    for (let v = v0; v <= v1 + 0.01; v += (v1 - v0) / Math.round((v1 - v0) / 1.9)) stroke(g, dark(frameC, 0.28), 0.45, [fl(u1, v, zB), fl(u1, v, zE)]);
    stroke(g, frameC, 0.7, [fl(u0, v1, zE), fl(u1, v1, zE), fl(u1, v0, zE)]);
    // The door, on the front face it's turned to.
    const doorL = item.facing % 2 === 0;
    if (doorL) {
      const um = (u0 + u1) / 2;
      fp3(g, 'rgba(30,50,40,.5)', [um - 1.1, v1, zB], [um + 1.1, v1, zB], [um + 1.1, v1, zB + 4.8], [um - 1.1, v1, zB + 4.8]);
      stroke(g, frameC, 0.7, [fl(um - 1.1, v1, zB), fl(um - 1.1, v1, zB + 4.8), fl(um + 1.1, v1, zB + 4.8), fl(um + 1.1, v1, zB)]);
    } else {
      const vm = (v0 + v1) / 2;
      fp3(g, 'rgba(20,40,30,.5)', [u1, vm - 1.1, zB], [u1, vm + 1.1, zB], [u1, vm + 1.1, zB + 4.8], [u1, vm - 1.1, zB + 4.8]);
      stroke(g, frameC, 0.7, [fl(u1, vm - 1.1, zB), fl(u1, vm - 1.1, zB + 4.8), fl(u1, vm + 1.1, zB + 4.8), fl(u1, vm + 1.1, zB)]);
    }
    // The roof: two glass slopes with rafters, and the gable end.
    const o = 0.3;
    if (axis === 'u') {
      const vm = (v0 + v1) / 2;
      fp3(g, 'rgba(205,240,230,.3)', [u1, v0, zE], [u1, v1, zE], [u1, vm, zE + rise]);
      fp3(g, glass(P(u0, vm, zE + rise), P(u0, v1, zE)), [u0 - o, v1 + o, zE], [u1 + o, v1 + o, zE], [u1 + o, vm, zE + rise], [u0 - o, vm, zE + rise]);
      for (let u = u0; u <= u1 + 0.01; u += (u1 - u0) / Math.round((u1 - u0) / 1.9)) stroke(g, frameC, 0.45, [fl(u, v1 + o, zE), fl(u, vm, zE + rise)]);
      stroke(g, frameC, 0.9, [fl(u0 - o, vm, zE + rise), fl(u1 + o, vm, zE + rise)]);
      stroke(g, frameC, 0.7, [fl(u0 - o, v1 + o, zE), fl(u1 + o, v1 + o, zE), fl(u1 + o, vm, zE + rise)]);
    } else {
      const um = (u0 + u1) / 2;
      fp3(g, 'rgba(205,240,230,.3)', [u0, v1, zE], [u1, v1, zE], [um, v1, zE + rise]);
      fp3(g, glass(P(um, v0, zE + rise), P(u1, v0, zE)), [u1 + o, v0 - o, zE], [u1 + o, v1 + o, zE], [um, v1 + o, zE + rise], [um, v0 - o, zE + rise]);
      for (let v = v0; v <= v1 + 0.01; v += (v1 - v0) / Math.round((v1 - v0) / 1.9)) stroke(g, dark(frameC, 0.28), 0.45, [fl(u1 + o, v, zE), fl(um, v, zE + rise)]);
      stroke(g, frameC, 0.9, [fl(um, v0 - o, zE + rise), fl(um, v1 + o, zE + rise)]);
      stroke(g, frameC, 0.7, [fl(u0, v1, zE), fl(um, v1, zE + rise), fl(u1 + o, v1 + o, zE), fl(u1 + o, v0 - o, zE)]);
    }
    stroke(g, 'rgba(255,255,255,.6)', 0.5, [fl(lerp(u0, u1, 0.15), v1, zE - 0.8), fl(lerp(u0, u1, 0.3), v1, zB + 1)]);
  },
  garden_shed(S, item) {
    if (!S.day) return;
    const { g } = where(S, item);
    const wall = colorIn(item, '#b8845a');
    const R = [item.u0 + 0.2, item.v0 + 0.2, item.u1 - 0.2, item.v1 - 0.2];
    const H = 6.6;
    const axis = item.facing % 2 === 0 ? 'u' : 'v';
    shadow(S, R, H + 3);
    solid(g, [R[0] - 0.15, R[1] - 0.15, R[2] + 0.15, R[3] + 0.15], 0, 0.8, '#9a978b');
    const door = (w) => {
      const x = w / 2 - 1.1;
      rr(g, dark(wall, 0.5), x - 0.3, -5.2, 2.6, 5.2);
      rr(g, dark(wall, 0.15), x, -4.9, 2.2, 4.9);
      rr(g, 'rgba(0,0,0,.22)', x + 1.1, -4.9, 0.25, 4.9);
      rr(g, '#e8c860', x + 1.5, -2.4, 0.5, 0.5);
    };
    const win = (w, x0 = 0.35) => {
      const x = w * x0;
      rr(g, '#fff4dc', x - 0.35, -5.2, 2.9, 2.9);
      rr(g, 'rgba(120,180,210,.85)', x, -4.85, 2.2, 2.2);
      rr(g, '#fff4dc', x + 1.05, -4.85, 0.25, 2.2);
      rr(g, 'rgba(255,255,255,.4)', x + 0.2, -4.7, 0.5, 1.3);
    };
    const planks = (w) => {
      for (let x = 0.9; x < w; x += 0.9) rr(g, 'rgba(40,22,12,.28)', x, -H, 0.22, H);
    };
    const f = item.facing;
    solid(g, R, 0.8, H, wall, {
      detailL: (w) => {
        planks(w);
        if (f === 0) {
          door(w);
        } else {
          win(w, 0.3);
          win(w, 0.62);
        }
      },
      detailR: (w) => {
        planks(w);
        if (f === 1) door(w);
        else win(w, 0.34);
      },
    });
    gableRoof(g, R, H, 3.4, '#6b4a3a', dark(wall, 0.1), axis, 0.9);
  },
  beehive(S, item) {
    if (!S.day) return;
    const { g, rand } = where(S, item);
    const u = item.u;
    const v = item.v;
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 8);
    pool(g, u, v, 1.6, 0.26);
    const legs = [[-1.1, -1.1], [1.1, -1.1], [-1.1, 1.1], [1.1, 1.1]];
    for (const [du, dv] of legs) {
      const [x, y] = P(u + du, v + dv);
      rr(g, '#7a5a38', x - 0.35, y - 1.7, 0.7, 1.7);
    }
    const colors = ['#f4efe2', '#f2c230', '#f4efe2'];
    colors.forEach((c, i) => {
      const z0 = 1.7 + i * 1.7;
      solid(g, [u - 1.3, v - 1.3, u + 1.3, v + 1.3], z0, z0 + 1.6, c, {
        detailL: (w) => rr(g, 'rgba(60,40,10,.25)', 0, -z0 - 1.6, w, 0.25),
      });
    });
    onL(g, u - 1.3, v + 1.3, () => rr(g, '#2a2018', 0.5, -2.1, 1.6, 0.35));
    solid(g, [u - 1.6, v - 1.6, u + 1.6, v + 1.6], 6.8, 7.4, '#9aa0aa');
    gableRoof(g, [u - 1.5, v - 1.5, u + 1.5, v + 1.5], 7.4, 1.4, '#8a909a', '#d8d4c8', 'u', 0.1);
    for (let i = 0; i < 4; i++) {
      const [x, y] = P(u + (rand() - 0.5) * 3, v + 1.4);
      ell(g, '#2a2018', x, y - 1 - rand() * 4, 0.35, 0.25);
      ell(g, '#f2c230', x + 0.2, y - 1 - rand() * 4, 0.3, 0.25);
    }
  },
  chicken_coop(S, item) {
    if (!S.day) return;
    const { g } = where(S, item);
    const f = frame(item);
    const wall = colorIn(item, '#c1473d');
    const box = (a0, b0, a1, b1) => {
      const [p, q] = [f.m(a0, b0), f.m(a1, b1)];
      return [Math.min(p[0], q[0]), Math.min(p[1], q[1]), Math.max(p[0], q[0]), Math.max(p[1], q[1])];
    };
    const cab = box(0.25, 0.35, 3.0, f.W - 0.35);
    const run = box(3.1, 0.2, f.L - 0.15, f.W - 0.2);
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 9);
    pool(g, item.u, item.v, 3.2, 0.16);
    const axis = f.alongU ? 'u' : 'v';
    inOrder([
      [
        run[0] + run[1] + run[2] + run[3],
        () => {
          disc(g, '#d9c070', (run[0] + run[2]) / 2, (run[1] + run[3]) / 2, 1.4);
          const corners = [[run[0], run[1]], [run[2], run[1]], [run[2], run[3]], [run[0], run[3]]];
          const edge = (a, b) => {
            fp3(g, 'rgba(210,220,230,.16)', [a[0], a[1], 0], [b[0], b[1], 0], [b[0], b[1], 3.6], [a[0], a[1], 3.6]);
            for (let k = 0; k <= 4; k++) {
              const u = lerp(a[0], b[0], k / 4);
              const v = lerp(a[1], b[1], k / 4);
              stroke(g, 'rgba(120,125,135,.5)', 0.3, [P(u, v, 0), P(u, v, 3.6)]);
            }
            stroke(g, '#8a8f99', 0.5, [P(a[0], a[1], 3.6), P(b[0], b[1], 3.6)]);
            stroke(g, '#8a8f99', 0.35, [P(a[0], a[1], 1.8), P(b[0], b[1], 1.8)]);
          };
          [[corners[0], corners[1]], [corners[3], corners[0]], [corners[1], corners[2]], [corners[2], corners[3]]].forEach(([a, b]) => edge(a, b));
          for (const [u, v] of corners) {
            const [x, y] = P(u, v);
            rr(g, '#7a5a38', x - 0.5, y - 3.9, 1, 3.9);
          }
          // A feeder on the ground.
          const [x, y] = P((run[0] + run[2]) / 2 + 0.6, (run[1] + run[3]) / 2 + 0.4);
          ell(g, '#b8b2a4', x, y - 0.4, 0.9, 0.5);
          ell(g, '#d9c070', x, y - 0.7, 0.6, 0.3);
        },
      ],
      [
        cab[0] + cab[1] + cab[2] + cab[3],
        () => {
          for (const [u, v] of [[cab[0] + 0.3, cab[1] + 0.3], [cab[2] - 0.3, cab[1] + 0.3], [cab[0] + 0.3, cab[3] - 0.3], [cab[2] - 0.3, cab[3] - 0.3]]) {
            const [x, y] = P(u, v);
            rr(g, '#6a4a2c', x - 0.4, y - 1.5, 0.8, 1.5);
          }
          solid(g, cab, 1.5, 6, wall, {
            detailL: (w) => {
              for (let x = 0.8; x < w; x += 0.8) rr(g, 'rgba(40,10,10,.25)', x, -6, 0.2, 4.5);
              rr(g, '#2a1c16', w / 2 - 0.6, -3.6, 1.2, 1.6);
              rr(g, '#f4efe2', w * 0.15, -5, 1.2, 1.1);
            },
          });
          // A ramp down from the pop hole.
          const um = (cab[0] + cab[2]) / 2;
          fp3(g, '#9a7448', [um - 0.5, cab[3], 2.1], [um + 0.5, cab[3], 2.1], [um + 0.5, cab[3] + 1.8, 0], [um - 0.5, cab[3] + 1.8, 0]);
          for (let k = 1; k < 4; k++) stroke(g, '#5e4426', 0.35, [P(um - 0.5, cab[3] + 0.45 * k, 2.1 - 0.5 * k), P(um + 0.5, cab[3] + 0.45 * k, 2.1 - 0.5 * k)]);
          gableRoof(g, cab, 6, 1.9, '#5a4636', dark(wall, 0.1), axis, 0.6);
          // A nest box on the side.
          solid(g, [cab[2], (cab[1] + cab[3]) / 2 - 0.9, cab[2] + 0.9, (cab[1] + cab[3]) / 2 + 0.9], 2.6, 4.6, lite(wall, 0.08));
        },
      ],
    ]);
  },
  scarecrow(S, item) {
    if (!S.day) return;
    const { g, x, y } = where(S, item);
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 14);
    pool(g, item.u, item.v, 1.4, 0.24);
    rr(g, '#7a5a38', x - 0.5, y - 14, 1, 14);
    // The arms along a pole, in a plaid shirt.
    stroke(g, '#7a5a38', 0.9, [[x - 5, y - 10.6], [x + 5, y - 10.6]]);
    quad(g, '#3f6fb0', [x - 2.2, y - 11.8], [x + 2.2, y - 11.8], [x + 2.6, y - 6.2], [x - 2.6, y - 6.2]);
    quad(g, '#3f6fb0', [x - 5, y - 11.4], [x - 2.2, y - 11.8], [x - 2.2, y - 9.6], [x - 5, y - 9.8]);
    quad(g, '#3f6fb0', [x + 5, y - 11.4], [x + 2.2, y - 11.8], [x + 2.2, y - 9.6], [x + 5, y - 9.8]);
    for (const dy of [-10.4, -8.4, -6.8]) rr(g, 'rgba(230,200,90,.7)', x - 2.5, y + dy, 5, 0.5);
    for (const dx of [-1, 0.6]) rr(g, 'rgba(230,200,90,.7)', x + dx, y - 11.6, 0.5, 5.4);
    rr(g, '#c9a64a', x - 2.4, y - 6.4, 4.8, 0.9);
    // Straw out of the sleeves and the hem.
    for (const s of [-1, 1]) for (let k = 0; k < 4; k++) stroke(g, '#e6c75a', 0.4, [[x + s * 5, y - 10.6 + k * 0.3], [x + s * (6.2 + k * 0.3), y - 11.2 + k * 0.9]]);
    for (let k = -2; k <= 2; k++) stroke(g, '#e6c75a', 0.4, [[x + k * 0.9, y - 6.2], [x + k * 1.0, y - 4.9]]);
    ell(g, '#d9c08a', x, y - 14.2, 1.9, 2.1);
    rr(g, '#3a2a1c', x - 1, y - 14.8, 0.5, 0.5);
    rr(g, '#3a2a1c', x + 0.5, y - 14.8, 0.5, 0.5);
    stroke(g, '#3a2a1c', 0.3, [[x - 0.9, y - 13.3], [x, y - 13], [x + 0.9, y - 13.3]]);
    ell(g, '#c9a64a', x, y - 16, 3.6, 0.9);
    quad(g, '#b88a3a', [x - 1.9, y - 16], [x + 1.9, y - 16], [x + 1.3, y - 18.6], [x - 1.3, y - 18.6]);
    rr(g, '#7a3a2c', x - 1.9, y - 16.6, 3.8, 0.6);
  },
  windmill(S, item) {
    if (!S.day) return;
    const { g } = where(S, item);
    const body = colorIn(item, '#efe6d2');
    const cu = item.u;
    const cv = item.v;
    shadow(S, [item.u0 + 0.6, item.v0 + 0.6, item.u1 - 0.6, item.v1 - 0.6], 22);
    pool(g, cu, cv, 2.4, 0.28);
    const rb = 1.95;
    const rt = 1.25;
    const hTop = 14;
    prism(g, ring(cu, cv, rt, 10, hTop, 0.3), ring(cu, cv, rb, 10, 1.4, 0.3), body, null);
    prism(g, ring(cu, cv, rb + 0.1, 10, 1.4, 0.3), ring(cu, cv, rb + 0.1, 10, 0, 0.3), '#8f8c80', null);
    // The roof: a cone with a ridge, the colour of the sails' cloth frame.
    const roof = '#b5503f';
    prism(g, ring(cu, cv, 0.2, 10, hTop + 4, 0.3), ring(cu, cv, rt + 0.55, 10, hTop, 0.3), roof, null);
    // A door and a window on the faces you see.
    const [dx, dy] = P(cu - 0.2, cv + rb - 0.35, 0);
    quad(g, '#5a3a24', [dx - 0.9, dy], [dx + 0.9, dy], [dx + 0.9, dy - 3.2], [dx, dy - 3.8], [dx - 0.9, dy - 3.2]);
    const [wx, wy] = P(cu + rb * 0.55, cv + rb * 0.55, 8);
    rr(g, '#fff4dc', wx - 0.9, wy - 1.7, 1.8, 2.4);
    rr(g, 'rgba(120,180,210,.9)', wx - 0.6, wy - 1.4, 1.2, 1.8);
    // The sails, turned a little, on a hub at the front of the cap.
    const [hx, hy] = P(cu + 0.9, cv + 0.9, hTop + 1.4);
    const rot = 0.5;
    for (let k = 0; k < 4; k++) {
      const a = rot + (k * TAU) / 4;
      g.save();
      g.translate(hx, hy);
      g.rotate(a);
      rr(g, '#6a4a2c', 0.4, -0.35, 9, 0.7);
      quad(g, '#f4efe2', [1.6, -0.35], [8.8, -0.35], [8.8, -2.9], [1.6, -2.9]);
      stroke(g, '#c9bfa5', 0.3, [[3.4, -0.35], [3.4, -2.9]]);
      stroke(g, '#c9bfa5', 0.3, [[5.2, -0.35], [5.2, -2.9]]);
      stroke(g, '#c9bfa5', 0.3, [[7, -0.35], [7, -2.9]]);
      g.restore();
    }
    ell(g, '#3a2a1c', hx, hy, 1.1, 1.1);
    ell(g, '#8a8f99', hx - 0.2, hy - 0.2, 0.5, 0.5);
  },
};

// ---------------------------------------------------------------------------------------------
// Water

function blob(cu, cv, ru, rv, n, rand, z = 0) {
  const ph = rand() * TAU;
  const ph2 = rand() * TAU;
  return Array.from({ length: n }, (_, i) => {
    const a = (i / n) * TAU;
    // A squircle, so the pond fills the corners of its cells.
    const sq = (Math.abs(Math.cos(a)) ** 3 + Math.abs(Math.sin(a)) ** 3) ** (-1 / 3);
    const k = sq * (1 + 0.06 * Math.sin(a * 3 + ph) + 0.04 * Math.sin(a * 5 + ph2));
    return [cu + Math.cos(a) * ru * k, cv + Math.sin(a) * rv * k, z];
  });
}

function reeds(g, x, y, rand, n) {
  for (let i = 0; i < n; i++) {
    const bx = x + (rand() - 0.5) * 3;
    const h = 2.8 + rand() * 2.4;
    stroke(g, i % 2 ? '#4f8a3a' : '#6aa04a', 0.6, [[bx, y], [bx + (rand() - 0.5) * 1.6, y - h]]);
    if (i % 3 === 0) {
      const tx = bx + 0.2;
      rr(g, '#6a4226', tx - 0.4, y - h - 1.2, 0.8, 1.6);
    }
  }
}

function lilyPad(g, u, v, r, flower) {
  const [x, y] = P(u, v, 0.1);
  ell(g, '#2f7a3c', x, y + 0.1, r * 1.6, r * 0.85);
  ell(g, '#4fa04a', x, y, r * 1.5, r * 0.75);
  fp(g, '#2a6a8a', [[x, y], [x + r * 1.5, y - r * 0.5], [x + r * 1.5, y + r * 0.1]]);
  if (flower) {
    ell(g, '#f7b6cf', x - 0.2, y - 0.8, 0.9, 0.7);
    ell(g, '#fff1f6', x - 0.2, y - 1.1, 0.5, 0.45);
  }
}

function pondArt(S, item, o) {
  const g = S.g;
  const rand = seeded(ih(item.x, item.y, 53));
  const hu = (item.u1 - item.u0) / 2;
  const hv = (item.v1 - item.v0) / 2;
  const k = 0.95;
  const bank = blob(item.u, item.v, hu * k, hv * k, 26, seeded(ih(item.x, item.y, 54)));
  const water = bank.map(([u, v]) => [item.u + (u - item.u) * 0.88, item.v + (v - item.v) * 0.88, 0.05]);
  const deep = bank.map(([u, v]) => [item.u + (u - item.u) * 0.5, item.v + (v - item.v) * 0.5, 0.06]);
  fp3(g, '#6f5c3c', ...bank.map(([u, v]) => [item.u + (u - item.u) * 1.08, item.v + (v - item.v) * 1.08, 0]));
  fp3(g, '#8d7a52', ...bank);
  const [, yTop] = P(item.u0, item.v0);
  const [, yBot] = P(item.u1, item.v1);
  fp3(g, grad(g, 0, yTop, 0, yBot, o.far, o.near), ...water);
  fp3(g, rgba(o.deep, 0.55), ...deep);
  // The near rim shows its wet edge.
  g.strokeStyle = 'rgba(255,255,255,.35)';
  g.lineWidth = 0.5;
  g.beginPath();
  water.forEach(([u, v, z], i) => {
    const [x, y] = P(u, v, z);
    if (i === 0) g.moveTo(x, y);
    else g.lineTo(x, y);
  });
  g.closePath();
  g.stroke();
  for (let i = 0; i < 5; i++) {
    const a = rand() * TAU;
    const d = 0.3 + rand() * 0.45;
    lilyPad(g, item.u + Math.cos(a) * hu * d, item.v + Math.sin(a) * hv * d, 0.65 + rand() * 0.3, i === 1);
  }
  if (o.extra) o.extra(g, item, rand);
  for (let i = 0; i < 4; i++) glint(g, item.u - hu * 0.4 + rand() * hu * 0.8, item.v - hv * 0.4 + rand() * hv * 0.8, 0.1);
  // Stones round the rim, and reeds at the back.
  const rim = [];
  for (let i = 0; i < 9; i++) {
    const a = (i / 9) * TAU + 0.2;
    const r = 0.98 + rand() * 0.04;
    rim.push([item.u + Math.cos(a) * hu * k * r, item.v + Math.sin(a) * hv * k * r, rand()]);
  }
  rim.sort((a, b) => a[0] + a[1] - b[0] - b[1]);
  for (const [u, v, t] of rim) {
    const [x, y] = P(u, v);
    boulder(g, x, y, 0.45 + t * 0.4);
  }
  const [bx, by] = P(item.u0 + 1.2, item.v + 0.5);
  reeds(g, bx, by, rand, 4);
  const [cx, cy] = P(item.u + 0.5, item.v0 + 1.2);
  reeds(g, cx, cy, rand, 3);
}

const WATER = {
  pond(S, item) {
    if (!S.day) return;
    pondArt(S, item, { far: '#4a97b8', near: '#7ccbe3', deep: '#2a6a90' });
  },
  koi_pond(S, item) {
    if (!S.day) return;
    pondArt(S, item, {
      far: '#3f86a8',
      near: '#74c0d8',
      deep: '#1f5a80',
      extra: (g, it, rand) => {
        const fish = [['#ee6a2a', 0.4, -0.3], ['#f6f1e4', 1.9, 0.4], ['#2b2d36', 3.6, -0.1], ['#ee8a3a', 4.8, 0.3]];
        fish.forEach(([c, a, d], i) => {
          const u = it.u + Math.cos(a) * 3.2 * (0.6 + d * 0.3 + 0.3);
          const v = it.v + Math.sin(a) * 2.2;
          const [x, y] = P(u, v, 0.1);
          g.save();
          g.translate(x, y);
          g.rotate(a + 0.6);
          ell(g, rgba(c, 0.95), 0, 0, 2, 0.8);
          fp(g, rgba(c, 0.8), [[-1.6, 0], [-3.1, -0.8], [-3.1, 0.8]]);
          if (i % 2 === 0) ell(g, '#f6f1e4', 0.4, -0.1, 0.7, 0.4);
          g.restore();
        });
        // Lotus flowers.
        for (const [du, dv] of [[1.8, 1.6], [-2.4, 0.4]]) {
          const [x, y] = P(it.u + du, it.v + dv, 0.1);
          ell(g, '#2f7a3c', x, y, 1.8, 0.9);
          for (const dx of [-0.8, 0, 0.8]) ell(g, dx ? '#f7b6cf' : '#fff1f6', x + dx, y - 0.8, 0.6, 1);
          ell(g, '#f2c230', x, y - 0.7, 0.35, 0.3);
        }
        void rand;
      },
    });
  },
  fountain(S, item) {
    if (!S.day) return;
    const { g } = where(S, item);
    const stone = colorIn(item, '#c9c6b8');
    const cu = item.u;
    const cv = item.v;
    shadow(S, [item.u0 + 0.5, item.v0 + 0.5, item.u1 - 0.5, item.v1 - 0.5], 10);
    disc(g, 'rgba(25,25,35,.25)', cu + 0.5, cv + 0.5, 2.2);
    prism(g, ring(cu, cv, 1.9, 12, 2.4), ring(cu, cv, 1.9, 12, 0), stone, null);
    fp3(g, lite(stone, 0.2), ...ring(cu, cv, 1.9, 12, 2.4));
    fp3(g, '#4fa6d8', ...ring(cu, cv, 1.5, 12, 2.1));
    fp3(g, '#7bc4ec', ...ring(cu - 0.3, cv - 0.3, 0.9, 10, 2.15));
    prism(g, ring(cu, cv, 0.45, 8, 6), ring(cu, cv, 0.5, 8, 2.1), lite(stone, 0.05), null);
    prism(g, ring(cu, cv, 1.1, 10, 6.8), ring(cu, cv, 0.5, 10, 5.6), stone, null);
    fp3(g, lite(stone, 0.2), ...ring(cu, cv, 1.1, 10, 6.8));
    fp3(g, '#8ccaf0', ...ring(cu, cv, 0.85, 10, 6.65));
    const [x, y] = P(cu, cv, 6.8);
    for (let k = 0; k < 5; k++) {
      const a = -1.1 + k * 0.55;
      g.strokeStyle = 'rgba(215,242,255,.9)';
      g.lineWidth = 0.55;
      g.beginPath();
      g.moveTo(x, y - 2.8);
      g.quadraticCurveTo(x + Math.sin(a) * 2.6, y - 6.4, x + Math.sin(a) * 4.4, y - 0.6);
      g.stroke();
    }
    stroke(g, 'rgba(215,242,255,.9)', 0.6, [[x, y], [x, y - 3.4]]);
    ell(g, 'rgba(255,255,255,.9)', x, y - 3.6, 0.5, 0.45);
    glint(g, cu - 0.6, cv + 0.7, 2.2);
  },
  bird_bath(S, item) {
    if (!S.day) return;
    const { g, x, y } = where(S, item);
    const stone = colorIn(item, '#c9c6b8');
    const cu = item.u;
    const cv = item.v;
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 6);
    pool(g, cu, cv, 1.4, 0.26);
    prism(g, ring(cu, cv, 0.85, 8, 1), ring(cu, cv, 0.9, 8, 0), dark(stone, 0.1), null);
    fp3(g, lite(stone, 0.1), ...ring(cu, cv, 0.85, 8, 1));
    prism(g, ring(cu, cv, 0.35, 8, 4.2), ring(cu, cv, 0.45, 8, 1), stone, null);
    prism(g, ring(cu, cv, 1.5, 12, 5.4), ring(cu, cv, 0.6, 12, 4), stone, null);
    fp3(g, lite(stone, 0.2), ...ring(cu, cv, 1.5, 12, 5.4));
    fp3(g, '#7bc4ec', ...ring(cu, cv, 1.2, 12, 5.25));
    glint(g, cu - 0.4, cv + 0.2, 5.3);
    // A small blue bird on the rim.
    const bx = x + 1.8;
    const by = y - 6.8;
    ell(g, '#3f6fb0', bx, by, 1.1, 0.8);
    ell(g, '#f1e8d2', bx + 0.1, by + 0.3, 0.7, 0.45);
    ell(g, '#3f6fb0', bx + 1, by - 0.7, 0.6, 0.6);
    fp(g, '#f0a030', [[bx + 1.5, by - 0.8], [bx + 2.2, by - 0.6], [bx + 1.5, by - 0.5]]);
    stroke(g, '#2b4f8c', 0.5, [[bx - 0.9, by], [bx - 2, by + 0.6]]);
  },
  well(S, item) {
    if (!S.day) return;
    const { g } = where(S, item);
    const cu = item.u;
    const cv = item.v;
    const stone = '#a8a597';
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 14);
    disc(g, 'rgba(25,25,35,.25)', cu + 0.6, cv + 0.6, 2.2);
    prism(g, ring(cu, cv, 1.8, 14, 4.2), ring(cu, cv, 1.8, 14, 0), stone, null);
    // Courses of stones round the front half.
    for (const [z, off] of [[1.4, 0], [2.8, 0.18]]) {
      const pts = [];
      for (let a = -0.8; a <= 2.4; a += 0.1) pts.push(P(cu + Math.cos(a) * 1.8, cv + Math.sin(a) * 1.8, z));
      stroke(g, 'rgba(40,35,30,.45)', 0.4, pts);
      for (let a = -0.7 + off; a < 2.3; a += 0.36) {
        const lo = z === 1.4 ? 0 : 1.4;
        stroke(g, 'rgba(40,35,30,.4)', 0.35, [P(cu + Math.cos(a) * 1.8, cv + Math.sin(a) * 1.8, lo), P(cu + Math.cos(a) * 1.8, cv + Math.sin(a) * 1.8, z)]);
      }
    }
    fp3(g, lite(stone, 0.22), ...ring(cu, cv, 1.9, 14, 4.4));
    fp3(g, '#2a2d38', ...ring(cu, cv, 1.35, 14, 4.4));
    fp3(g, '#2f6a90', ...ring(cu, cv, 1.1, 14, 4.2));
    const axis = item.facing % 2 === 0 ? 'u' : 'v';
    const post = (u, v) => {
      const [x, y] = P(u, v, 4.4);
      g.fillStyle = grad(g, x - 0.5, 0, x + 0.5, 0, '#a98456', '#5e4426');
      g.fillRect(x - 0.5, y - 7, 1, 7);
    };
    const roofR = axis === 'u' ? [cu - 2, cv - 1.4, cu + 2, cv + 1.4] : [cu - 1.4, cv - 2, cu + 1.4, cv + 2];
    const ps = axis === 'u' ? [[cu - 1.8, cv], [cu + 1.8, cv]] : [[cu, cv - 1.8], [cu, cv + 1.8]];
    post(...ps[0]);
    const [bx, by] = P(cu, cv, 11.4);
    stroke(g, '#6a4a2c', 0.9, [P(ps[0][0], ps[0][1], 10.4), P(ps[1][0], ps[1][1], 10.4)]);
    stroke(g, '#bfae8a', 0.35, [[bx, by + 0.3], [bx, by + 3.6]]);
    quad(g, '#8a5d33', [bx - 0.9, by + 3.6], [bx + 0.9, by + 3.6], [bx + 0.7, by + 5.6], [bx - 0.7, by + 5.6]);
    rr(g, '#4a321c', bx - 0.9, by + 3.8, 1.8, 0.3);
    post(...ps[1]);
    gableRoof(g, roofR, 10.8, 3, '#7a4a34', '#9a6a4a', axis, 0.5);
  },
  pool(S, item) {
    if (!S.day) return;
    const { g } = where(S, item);
    const water = colorIn(item, '#46b4e0');
    const R = [item.u0, item.v0, item.u1, item.v1];
    const rand = seeded(ih(item.x, item.y, 61));
    const zc = 0.7;
    const coping = '#e6dfcf';
    // The deck round it, then the basin cut into it.
    fp3(g, dark(coping, 0.08), [R[0] - 0.2, R[1] - 0.2, 0], [R[2] + 0.2, R[1] - 0.2, 0], [R[2] + 0.2, R[3] + 0.2, 0], [R[0] - 0.2, R[3] + 0.2, 0]);
    solid(g, R, 0, zc, coping);
    const w = [R[0] + 1, R[1] + 1, R[2] - 1, R[3] - 1];
    const [, yTop] = P(w[0], w[1]);
    const [, yBot] = P(w[2], w[3]);
    fp3(g, grad(g, 0, yTop, 0, yBot, dark(water, 0.3), lite(water, 0.25)), [w[0], w[1], 0], [w[2], w[1], 0], [w[2], w[3], 0], [w[0], w[3], 0]);
    // The wall at the back, with its tiles showing above the water.
    fp3(g, dark(water, 0.4), [w[0], w[1], 0], [w[2], w[1], 0], [w[2], w[1], zc], [w[0], w[1], zc]);
    fp3(g, dark(water, 0.5), [w[0], w[1], 0], [w[0], w[3], 0], [w[0], w[3], zc], [w[0], w[1], zc]);
    // Lines on the floor under the water, and light playing on it.
    for (const t of [0.33, 0.66]) stroke(g, 'rgba(15,60,110,.3)', 0.6, [P(w[0] + 0.5, lerp(w[1], w[3], t)), P(w[2] - 0.5, lerp(w[1], w[3], t))]);
    for (let i = 0; i < 9; i++) {
      const [x, y] = P(lerp(w[0] + 1, w[2] - 1, rand()), lerp(w[1] + 0.8, w[3] - 0.8, rand()));
      const pts = [];
      for (let k = 0; k <= 6; k++) pts.push([x + k * 0.7, y + Math.sin(k * 1.1 + i) * 0.35]);
      stroke(g, 'rgba(255,255,255,.28)', 0.5, pts);
    }
    for (let i = 0; i < 5; i++) glint(g, lerp(w[0] + 1, w[2] - 1.5, rand()), lerp(w[1] + 0.8, w[3] - 0.8, rand()), 0.1, 1);
    // A float on the water.
    const [fx, fy] = P(lerp(w[0], w[2], 0.62), lerp(w[1], w[3], 0.45), 0.1);
    ell(g, '#f4efe2', fx, fy, 1.7, 0.85);
    ell(g, dark(water, 0.1), fx, fy, 0.8, 0.4);
    stroke(g, '#e8484a', 1.2, [[fx - 1.2, fy + 0.5], [fx - 0.2, fy + 0.8]]);
    stroke(g, '#e8484a', 1.2, [[fx + 0.8, fy - 0.6], [fx + 1.4, fy - 0.2]]);
    // The ladder on the near side.
    for (const dv of [-0.5, 0.5]) {
      const v = (w[1] + w[3]) / 2 + dv + 1.5;
      const a = P(R[2] - 0.6, v, zc);
      const b = P(R[2] - 0.6, v, zc + 3);
      const c2 = P(R[2] - 1.8, v, zc + 2.6);
      const d = P(R[2] - 1.8, v, 0);
      g.strokeStyle = '#d3d6dc';
      g.lineWidth = 0.55;
      g.beginPath();
      g.moveTo(a[0], a[1]);
      g.lineTo(b[0], b[1]);
      g.quadraticCurveTo(lerp(b[0], c2[0], 0.5), Math.min(b[1], c2[1]) - 1, c2[0], c2[1]);
      g.lineTo(d[0], d[1]);
      g.stroke();
    }
    for (const dz of [0.9, 1.8]) stroke(g, '#d3d6dc', 0.4, [P(R[2] - 1.8, (w[1] + w[3]) / 2 + 1, dz), P(R[2] - 1.8, (w[1] + w[3]) / 2 + 2, dz)]);
  },
  hot_tub(S, item) {
    if (!S.day) return;
    const { g } = where(S, item);
    const cab = colorIn(item, '#7a4e2c');
    const cu = item.u - 0.2;
    const cv = item.v - 0.2;
    shadow(S, [item.u0, item.v0, item.u1, item.v1], 6);
    disc(g, 'rgba(25,25,35,.25)', cu + 0.6, cv + 0.6, 3);
    solid(g, [item.u1 - 0.9, item.v - 0.8, item.u1 - 0.1, item.v + 0.8], 0, 1.8, lite(cab, 0.08));
    prism(g, ring(cu, cv, 2.55, 16, 3.7), ring(cu, cv, 2.55, 16, 0), cab, null);
    prism(g, ring(cu, cv, 2.6, 16, 4.4), ring(cu, cv, 2.6, 16, 3.7), '#e9e9e2', '#f6f6f0');
    fp3(g, '#4cc0dc', ...ring(cu, cv, 2.05, 16, 4.2));
    fp3(g, '#7fdcee', ...ring(cu - 0.4, cv - 0.4, 1.2, 12, 4.25));
    const rand = seeded(ih(item.x, item.y, 71));
    for (let i = 0; i < 9; i++) {
      const a = rand() * TAU;
      const d = rand() * 1.7;
      const [x, y] = P(cu + Math.cos(a) * d, cv + Math.sin(a) * d, 4.3);
      ell(g, 'rgba(255,255,255,.75)', x, y, 0.4, 0.28);
    }
    glint(g, cu - 1, cv + 0.3, 4.3, 2);
    // Steam drifting up.
    const [sx, sy] = P(cu, cv, 4.4);
    for (const [dx, dy, r, a] of [[-0.8, -3.4, 1.5, 0.24], [0.9, -6.2, 1.9, 0.18], [-0.4, -9.2, 2.2, 0.12]]) ell(g, `rgba(255,255,255,${a})`, sx + dx, sy + dy, r * 1.3, r);
  },
};

// E15: every kind in `YARD` that this file draws, by its catalog word.
export const YARD_ART = {
  ...TREES,
  ...PLANTS,
  ...FLOWERS,
  ...GARDEN,
  ...WATER,
};
