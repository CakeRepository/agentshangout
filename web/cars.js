// CR10: the cars, painted. Each car in the catalog (the Rust catalog) has a look: a body, a colour, a trim
// colour and some flair. This paints that look in the map's own isometric pixel-art language, the
// same 2:1 projection, warm light and cool shadow as the cars in world.js, at the same scale (a car
// is about 13 tiles-sixteenths long, so it reads as one small thing on the road).
//
// A car is a handful of parts. Each part is a side profile (length along, height up) pushed across
// the car's width, so a windshield, a hood and a spoiler are all just slanted faces, painted back to
// front and lit by which way they face. Nothing here reads the page: world.js and the garage call
// in, and this file imports nothing, so a test can run it with a fake canvas.

const HEX = /^#[0-9a-fA-F]{6}$/;
const FLAIRS = ['stripes', 'spoiler', 'underglow', 'chrome', 'gold'];

// ---------------------------------------------------------------------------------------------
// Colour. The same warm-light, cool-shadow mixing the map uses.

const clamp = (n, a, b) => Math.max(a, Math.min(b, n));
const lerp = (a, b, t) => a + (b - a) * t;
const rgbOf = (hex) => {
  const n = parseInt(hex.slice(1), 16);
  return [n >> 16, (n >> 8) & 255, n & 255];
};
function mix(a, b, t) {
  const p = rgbOf(a);
  const q = rgbOf(b);
  const c = (i) => clamp(Math.round(lerp(p[i], q[i], t)), 0, 255);
  return `#${((1 << 24) | (c(0) << 16) | (c(1) << 8) | c(2)).toString(16).slice(1)}`;
}
const lite = (c, t) => mix(c, '#fff4d6', t);
const dark = (c, t) => mix(c, '#241f3c', t);
const rgba = (hex, a) => {
  const [r, g, b] = rgbOf(hex);
  return `rgba(${r},${g},${b},${a})`;
};
// How bright a face is, from the way it points: the top is lit, the left is as painted, the right is in shade.
const shade = (c, tone) => (tone >= 0.35 ? lite(c, (tone - 0.35) * 0.5) : dark(c, (0.35 - tone) * 0.55));

const GLASS = ['#b4dbf0', '#3b5774'];
const TYRE = '#14151c';
const RIM = '#7c8290';
const CHROME = '#eef3fa';
const GOLD = ['#fff0a0', '#e0b02a', '#a8761a'];

// ---------------------------------------------------------------------------------------------
// The bodies. A car lies along `s` (0 at the tail, L at the nose) and across `t` (0 to W). A part's
// `pts` are its side profile as [s, height], going round counter-clockwise from the tail's foot.
// `layer` 0 is the lower body, painted first; layer 1 sits on it. `win` lists the profile edges that
// are glass (a windshield), `side` is how its sides show glass. `lz` is the band the lights sit in.

const SPECS = {
  hatch: {
    L: 10.5, W: 6, wr: 2.1, wheels: [2.5, 8.2], lz: [2.6, 3.9], deck: 4.9,
    parts: [
      { pts: [[0, 1.4], [10.5, 1.4], [10.5, 3.7], [9.5, 4.9], [0, 4.9]] },
      { layer: 1, t: [0.4, 5.6], pts: [[0.9, 4.9], [8.2, 4.9], [6.3, 8.7], [1.3, 8.7]], win: [1, 3], side: 'shrink' },
    ],
  },
  sedan: {
    L: 13, W: 6.4, wr: 2.2, wheels: [2.7, 10.3], lz: [2.8, 4.1], deck: 5.2,
    parts: [
      { pts: [[0, 1.4], [13, 1.4], [13, 4.0], [12.2, 5.2], [0.7, 5.2], [0, 4.6]] },
      { layer: 1, t: [0.55, 5.85], pts: [[3.0, 5.2], [9.2, 5.2], [7.5, 8.9], [4.6, 8.9]], win: [1, 3], side: 'shrink' },
    ],
  },
  wagon: {
    L: 13.5, W: 6.4, wr: 2.2, wheels: [2.8, 10.7], lz: [2.8, 4.1], deck: 5.2,
    parts: [
      { pts: [[0, 1.4], [13.5, 1.4], [13.5, 4.0], [12.6, 5.2], [0, 5.2]] },
      { layer: 1, t: [0.5, 5.9], pts: [[1.4, 5.2], [9.9, 5.2], [8.2, 8.9], [1.5, 8.9]], win: [1], side: 'shrink' },
      { layer: 1, mat: 'dark', t: [1.0, 1.6], pts: [[2.4, 8.9], [7.6, 8.9], [7.6, 9.6], [2.4, 9.6]] },
      { layer: 1, mat: 'dark', t: [4.8, 5.4], pts: [[2.4, 8.9], [7.6, 8.9], [7.6, 9.6], [2.4, 9.6]] },
    ],
  },
  van: {
    L: 12, W: 6.9, wr: 2.2, wheels: [2.6, 9.4], lz: [2.8, 4.2], deck: 10.4, tall: true,
    parts: [
      { pts: [[0, 1.4], [12, 1.4], [12, 4.4], [11, 5.6], [0, 5.6]] },
      { layer: 1, t: [0, 6.9], pts: [[0, 5.6], [10.7, 5.6], [9.9, 10.5], [0, 10.5]], win: [1], side: [[[6.4, 6.6], [10.0, 6.6], [9.5, 9.7], [6.4, 9.7]]] },
    ],
  },
  pickup: {
    L: 14, W: 6.7, wr: 2.7, wheels: [3.0, 11.0], lz: [3.2, 4.5], deck: 6.2, ride: 2.1,
    parts: [
      { pts: [[6.4, 2.1], [14, 2.1], [14, 4.8], [13, 5.7], [6.4, 5.7]] },
      { pts: [[0, 2.1], [6.4, 2.1], [6.4, 6.3], [0, 6.3]], well: { s0: 0.6, s1: 5.8, m: 0.6 } },
      { layer: 1, t: [0.4, 6.3], pts: [[7.0, 5.7], [11.1, 5.7], [9.9, 9.3], [7.2, 9.3]], win: [1, 3], side: 'shrink' },
    ],
  },
  jeep: {
    L: 11.5, W: 6.9, wr: 2.8, wheels: [2.7, 8.8], lz: [3.7, 5.1], deck: 6.0, ride: 2.5, round: true, spare: true,
    parts: [
      { pts: [[0, 2.5], [11.5, 2.5], [11.5, 5.5], [10.8, 6.1], [0, 6.1]] },
      { layer: 1, t: [0.3, 6.6], roof: 'trim', pts: [[0.9, 6.1], [8.8, 6.1], [8.4, 9.7], [1.1, 9.7]], win: [1], side: 'shrink' },
    ],
  },
  coupe: {
    L: 12.5, W: 6.4, wr: 2.1, wheels: [2.6, 9.8], lz: [2.4, 3.7], deck: 4.8,
    parts: [
      { pts: [[0, 1.2], [12.5, 1.2], [12.5, 3.5], [11.5, 4.8], [0.9, 4.8], [0, 4.3]] },
      { layer: 1, t: [0.5, 5.9], pts: [[3.0, 4.8], [8.5, 4.8], [6.6, 7.7], [5.1, 7.7]], win: [1, 3], side: 'shrink' },
    ],
  },
  muscle: {
    L: 14, W: 7.1, wr: 2.4, wheels: [2.9, 11.1], lz: [2.8, 4.1], deck: 5.5,
    parts: [
      { pts: [[0, 1.4], [14, 1.4], [14, 4.2], [13.1, 5.4], [2.0, 5.4], [0, 5.6]] },
      { layer: 1, t: [0.6, 6.5], pts: [[2.3, 5.5], [7.6, 5.4], [6.4, 8.3], [3.5, 8.3]], win: [1, 3], side: 'shrink' },
      { layer: 1, mat: 'dark', t: [2.4, 4.7], pts: [[9.0, 5.4], [11.9, 5.4], [11.4, 6.6], [9.5, 6.6]] },
    ],
  },
  sports: {
    L: 13.5, W: 7.1, wr: 2.1, wheels: [2.8, 10.6], lz: [1.7, 2.7], deck: 4.4,
    parts: [
      { pts: [[0, 1.0], [13.5, 1.0], [13.5, 1.9], [10, 3.2], [2.0, 4.4], [0, 4.4]] },
      { layer: 1, t: [0.9, 6.2], pts: [[4.6, 4.0], [9.2, 3.3], [7.5, 6.1], [5.4, 6.3]], win: [1, 3], side: 'shrink' },
    ],
  },
  roadster: {
    L: 12.5, W: 6.6, wr: 2.1, wheels: [2.6, 9.8], lz: [2.3, 3.5], deck: 4.5,
    parts: [
      { pts: [[0, 1.2], [12.5, 1.2], [12.5, 3.4], [11.4, 4.5], [0, 4.5]], well: { s0: 1.6, s1: 7.4, m: 0.7 } },
      { layer: 1, mat: 'trim', t: [0.9, 2.6], pts: [[2.0, 4.5], [3.0, 4.5], [3.0, 6.3], [2.0, 6.1]] },
      { layer: 1, mat: 'trim', t: [4.0, 5.7], pts: [[2.0, 4.5], [3.0, 4.5], [3.0, 6.3], [2.0, 6.1]] },
      { layer: 1, mat: 'glass', t: [0.2, 6.4], pts: [[7.5, 4.5], [8.7, 4.5], [7.9, 6.5], [7.4, 6.5]], win: [1] },
    ],
  },
  limo: {
    L: 19, W: 6.6, wr: 2.2, wheels: [2.8, 16.2], lz: [2.8, 4.1], deck: 5.2, pillars: [5.6, 8.9, 12.2],
    parts: [
      { pts: [[0, 1.4], [19, 1.4], [19, 4.0], [18.1, 5.2], [0.6, 5.2], [0, 4.7]] },
      { layer: 1, t: [0.5, 6.1], pts: [[1.8, 5.2], [14.7, 5.2], [13.1, 8.5], [3.2, 8.5]], win: [1, 3], side: 'shrink' },
    ],
  },
};

export const CAR_BODIES = Object.keys(SPECS);

const DEFAULT_LOOK = { body: 'sedan', color: '#6fa8dc', trim: '#3c4a5a', flair: [] };

// A look from the server, held to what this file can paint: an unknown body is a sedan, a colour
// that isn't #rrggbb is the default, and flair it doesn't know is dropped.
export function normalizeLook(look) {
  const l = look && typeof look === 'object' ? look : {};
  const flair = Array.isArray(l.flair) ? l.flair.filter((f) => FLAIRS.includes(f)) : [];
  return {
    body: typeof l.body === 'string' && Object.hasOwn(SPECS, l.body) ? l.body : DEFAULT_LOOK.body,
    color: typeof l.color === 'string' && HEX.test(l.color) ? l.color : DEFAULT_LOOK.color,
    trim: typeof l.trim === 'string' && HEX.test(l.trim) ? l.trim : DEFAULT_LOOK.trim,
    flair: [...new Set(flair)],
  };
}

// Whether a ride carries something this file can paint.
export const hasLook = (look) => Boolean(look) && typeof look === 'object' && typeof look.body === 'string';

// What a sprite is cached under: the same look is painted once per heading, whichever car it is.
export function carKey(look) {
  const l = normalizeLook(look);
  return `${l.body}|${l.color}|${l.trim}|${l.flair.join(',')}`;
}

// ---------------------------------------------------------------------------------------------
// Painting

// Iso projection, as in world.js: u runs down-right, v down-left, z up.
const P = (u, v, z = 0) => [u - v, (u + v) / 2 - z];

function path(g, pts) {
  g.beginPath();
  g.moveTo(pts[0][0], pts[0][1]);
  for (let i = 1; i < pts.length; i++) g.lineTo(pts[i][0], pts[i][1]);
  g.closePath();
}
function fillPoly(g, fill, pts) {
  path(g, pts);
  g.fillStyle = fill;
  g.fill();
}
const centre = (pts) => [pts.reduce((a, p) => a + p[0], 0) / pts.length, pts.reduce((a, p) => a + p[1], 0) / pts.length];
const shrink = (pts, k) => {
  const [cx, cy] = centre(pts);
  return pts.map((p) => [cx + (p[0] - cx) * k, cy + (p[1] - cy) * k]);
};
const spanY = (pts) => [Math.min(...pts.map((p) => p[1])), Math.max(...pts.map((p) => p[1]))];
function vgrad(g, pts, a, b) {
  const [y0, y1] = spanY(pts);
  const gr = g.createLinearGradient(0, y0, 0, y1 + 0.01);
  gr.addColorStop(0, a);
  gr.addColorStop(1, b);
  return gr;
}

// Which way the car lies on the map. Heading 0 runs along +u (down-right), 1 along +v, 2 along -u,
// 3 along -v, as in world.js. Returns the projection of a point in the car's own s, t and height,
// and the way s and t run on the ground.
function frame(spec, heading) {
  const { L, W } = spec;
  const m = [
    (s, t) => [s - L / 2, t - W / 2],
    (s, t) => [t - W / 2, s - L / 2],
    (s, t) => [L / 2 - s, W / 2 - t],
    (s, t) => [W / 2 - t, L / 2 - s],
  ][heading];
  const o = m(0, 0);
  const ds = m(1, 0).map((v, i) => v - o[i]);
  const dt = m(0, 1).map((v, i) => v - o[i]);
  return {
    m,
    ds,
    dt,
    at: (s, t, z) => P(...m(s, t), z),
    sideT: dt[0] + dt[1] > 0 ? W : 0, // which side of the car we see
    endS: ds[0] + ds[1] > 0 ? L : 0, // which end we see
    sideSign: dt[0] + dt[1] > 0 ? 1 : -1,
  };
}

// The plane of a face drawn flat: x runs along the car's side or end, y runs down. Returns the
// transform's origin and the way x runs on screen, for a point at ground position (u, v).
function planeOf(fr, kind, fixed) {
  // A side (t fixed) runs along s; an end (s fixed) runs along t.
  const along = kind === 'side' ? fr.ds : fr.dt;
  const u = along[0] !== 0; // runs along u (else v)
  const [fu, fv] = kind === 'side' ? fr.m(0, fixed) : fr.m(fixed, 0);
  return u
    ? { dx: 1, dy: 0.5, ox: -fv, oy: fv / 2, coord: (s, t) => fr.m(s, t)[0] }
    : { dx: -1, dy: 0.5, ox: fu, oy: fu / 2, coord: (s, t) => fr.m(s, t)[1] };
}

function onPlane(g, pl, fn) {
  g.save();
  g.transform(pl.dx, pl.dy, 0, 1, pl.ox, pl.oy);
  fn();
  g.restore();
}

function matColor(mat, look) {
  switch (mat) {
    case 'trim': return look.trim;
    case 'dark': return '#2a2830';
    case 'glass': return '#4a6a8a';
    case 'chrome': return '#c9d3e0';
    case 'gold': return GOLD[1];
    default: return look.color;
  }
}

function glassFill(g, pts, tone) {
  fillPoly(g, vgrad(g, pts, shade(GLASS[0], tone * 0.6), shade(GLASS[1], tone * 0.6)), pts);
}

// One part: its visible faces, each lit by the way it points.
function paintPart(g, cx, part) {
  const { fr, look, F } = cx;
  const tA = part.t ? part.t[0] : 0;
  const tB = part.t ? part.t[1] : cx.spec.W;
  const mat = part.mat || 'body';
  const base = mat === 'body' && F.gold ? mix(look.color, '#b8860b', 0.22) : matColor(mat, look);
  const faces = [];
  const sideT = fr.sideT === cx.spec.W ? tB : tA;
  faces.push({ side: true, poly: part.pts.map((p) => fr.at(p[0], sideT, p[1])), n: [0, fr.sideSign, 0], t: sideT });
  const pts = part.pts;
  for (let i = 0; i < pts.length; i++) {
    const p = pts[i];
    const q = pts[(i + 1) % pts.length];
    const len = Math.hypot(q[0] - p[0], q[1] - p[1]);
    if (!len) continue;
    const ns = (q[1] - p[1]) / len;
    const nz = -(q[0] - p[0]) / len;
    if (nz < -0.5) continue;
    faces.push({
      edge: i,
      p,
      q,
      poly: [fr.at(p[0], tA, p[1]), fr.at(q[0], tA, q[1]), fr.at(q[0], tB, q[1]), fr.at(p[0], tB, p[1])],
      n: [ns, 0, nz],
    });
  }
  for (const f of faces) {
    // The face's normal on the map: which way it looks, against the way the camera looks.
    const nU = f.n[0] * fr.ds[0] + f.n[1] * fr.dt[0];
    const nV = f.n[0] * fr.ds[1] + f.n[1] * fr.dt[1];
    const nZ = f.n[2];
    if (nU + nV + nZ <= 0.02) continue;
    const tone = nZ + nV * 0.35 - nU * 0.25;
    const top = nZ > 0.5;
    let col = top && part.roof ? matColor(part.roof, look) : base;
    if (mat === 'chrome') col = CHROME;
    const lit = shade(col, tone);
    fillPoly(g, vgrad(g, f.poly, top ? lit : lite(lit, 0.1), top ? lit : dark(lit, 0.14)), f.poly);
    if (F.gold && mat === 'body') {
      // A band of light running across the paint.
      g.save();
      path(g, f.poly);
      g.clip();
      const [x0, x1] = [Math.min(...f.poly.map((p) => p[0])), Math.max(...f.poly.map((p) => p[0]))];
      const [y0, y1] = spanY(f.poly);
      const gr = g.createLinearGradient(x0, y1, x1, y0);
      gr.addColorStop(0, 'rgba(255,250,210,0)');
      gr.addColorStop(0.5, `rgba(255,250,210,${top ? 0.26 : 0.2})`);
      gr.addColorStop(1, 'rgba(255,250,210,0)');
      g.fillStyle = gr;
      g.fillRect(x0 - 1, y0 - 1, x1 - x0 + 2, y1 - y0 + 2);
      g.restore();
    }
    // Stripes run down the middle of everything that faces up.
    if (F.stripes && mat === 'body' && nZ > 0.35 && f.edge !== undefined) {
      const mid = cx.spec.W / 2;
      for (const [a, b] of [[-1.15, -0.45], [0.45, 1.15]]) {
        const ta = clamp(mid + a, tA, tB);
        const tb = clamp(mid + b, tA, tB);
        if (tb - ta < 0.2) continue;
        fillPoly(g, shade(look.trim, tone), [
          fr.at(f.p[0], ta, f.p[1]), fr.at(f.q[0], ta, f.q[1]), fr.at(f.q[0], tb, f.q[1]), fr.at(f.p[0], tb, f.p[1]),
        ]);
      }
    }
    // A bed or a cockpit: the top is a hole, dark inside.
    if (part.well && top && f.edge !== undefined) {
      const w = part.well;
      const s0 = Math.max(w.s0, Math.min(f.p[0], f.q[0]));
      const s1 = Math.min(w.s1, Math.max(f.p[0], f.q[0]));
      if (s1 > s0) {
        const z = f.p[1];
        const hole = [
          fr.at(s0, tA + w.m, z), fr.at(s1, tA + w.m, z), fr.at(s1, tB - w.m, z), fr.at(s0, tB - w.m, z),
        ];
        fillPoly(g, vgrad(g, hole, dark(look.trim, 0.55), dark(look.trim, 0.8)), hole);
      }
    }
    // Glass.
    if (f.side && part.side) {
      const wins = part.side === 'shrink'
        ? [shrink(f.poly, 0.7).map(([x, y], i) => [x, y + (i < 2 ? 0 : -0.1)])]
        : part.side.map((w) => w.map((p) => fr.at(p[0], f.t, p[1])));
      for (const w of wins) glassFill(g, w, tone);
      if (cx.spec.pillars) {
        for (const s of cx.spec.pillars) {
          const a = fr.at(s, f.t, 5.5);
          const b = fr.at(s, f.t, 8.3);
          g.strokeStyle = shade(look.color, tone);
          g.lineWidth = 0.8;
          g.beginPath();
          g.moveTo(a[0], a[1]);
          g.lineTo(b[0], b[1]);
          g.stroke();
        }
      }
    }
    if (f.edge !== undefined && part.win && part.win.includes(f.edge)) {
      glassFill(g, shrink(f.poly, 0.8), tone);
      if (mat === 'body') {
        // A shine across the glass.
        const w = shrink(f.poly, 0.8);
        g.strokeStyle = 'rgba(255,255,255,.35)';
        g.lineWidth = 0.6;
        g.beginPath();
        g.moveTo(w[0][0] + (w[1][0] - w[0][0]) * 0.55, w[0][1] + (w[1][1] - w[0][1]) * 0.55);
        g.lineTo(w[3][0] + (w[2][0] - w[3][0]) * 0.55, w[3][1] + (w[2][1] - w[3][1]) * 0.55);
        g.stroke();
      }
    }
  }
}

function wheelsOn(g, cx) {
  const { fr, look, F, spec } = cx;
  const pl = planeOf(fr, 'side', fr.sideT);
  const zc = spec.wr - 0.3;
  const rim = F.gold ? GOLD[0] : F.chrome ? CHROME : RIM;
  onPlane(g, pl, () => {
    for (const s of spec.wheels) {
      const a = pl.coord(s, fr.sideT);
      g.fillStyle = 'rgba(20,18,36,.26)';
      g.beginPath();
      g.ellipse(a, -zc, spec.wr + 0.3, spec.wr + 0.3, 0, 0, 6.2832);
      g.fill();
      g.fillStyle = TYRE;
      g.beginPath();
      g.ellipse(a, -zc, spec.wr, spec.wr, 0, 0, 6.2832);
      g.fill();
      g.fillStyle = rim;
      g.beginPath();
      g.ellipse(a, -zc, spec.wr * 0.46, spec.wr * 0.46, 0, 0, 6.2832);
      g.fill();
      g.fillStyle = F.gold ? GOLD[2] : dark(rim, 0.4);
      g.beginPath();
      g.ellipse(a, -zc, spec.wr * 0.2, spec.wr * 0.2, 0, 0, 6.2832);
      g.fill();
      if (F.chrome || F.gold) {
        g.fillStyle = 'rgba(255,255,255,.85)';
        g.fillRect(a - spec.wr * 0.34, -zc - spec.wr * 0.34, spec.wr * 0.25, spec.wr * 0.14);
      }
    }
  });
  void look;
}

// The end we can see: headlights if it's the nose, tail lights if it's the tail.
function endLights(g, cx) {
  const { fr, spec, F } = cx;
  const nose = fr.endS === spec.L;
  const pl = planeOf(fr, 'end', fr.endS);
  const [z0, z1] = spec.lz;
  const h = z1 - z0;
  const at = (t) => pl.coord(fr.endS, t);
  onPlane(g, pl, () => {
    if (nose) {
      // A dark grille between the lamps.
      const a = at(spec.W * 0.34);
      const b = at(spec.W * 0.66);
      g.fillStyle = 'rgba(30,28,40,.7)';
      g.fillRect(Math.min(a, b), -z1 + 0.3, Math.abs(b - a), h * 0.7);
    }
    for (const t of [0.7, spec.W - 2.1]) {
      const a = at(t);
      const b = at(t + 1.4);
      const x = Math.min(a, b);
      const w = Math.abs(b - a);
      if (nose) {
        g.fillStyle = 'rgba(255,240,170,.5)';
        g.beginPath();
        g.ellipse(x + w / 2, -(z0 + z1) / 2, w * 0.9, h * 0.9, 0, 0, 6.2832);
        g.fill();
      }
      g.fillStyle = nose ? '#fff6c4' : '#ff4a4a';
      if (spec.round && nose) {
        g.beginPath();
        g.ellipse(x + w / 2, -(z0 + z1) / 2, w * 0.5, h * 0.5, 0, 0, 6.2832);
        g.fill();
      } else {
        g.fillRect(x, -z1, w, h);
      }
    }
    // A spare wheel on the tail of a jeep.
    if (spec.spare && !nose) {
      const a = at(spec.W / 2);
      g.fillStyle = TYRE;
      g.beginPath();
      g.ellipse(a, -4.2, 2.4, 2.4, 0, 0, 6.2832);
      g.fill();
      g.fillStyle = RIM;
      g.beginPath();
      g.ellipse(a, -4.2, 1.1, 1.1, 0, 0, 6.2832);
      g.fill();
    }
    void F;
  });
}

// Bumpers, the belt line and the spoiler: parts too.
function extras(spec, look, F) {
  const parts = [];
  const z0 = spec.ride || 1.4;
  if (F.chrome || F.gold) {
    const mat = F.gold ? 'gold' : 'chrome';
    parts.push({ mat, pts: [[spec.L, z0 + 0.15], [spec.L + 0.6, z0 + 0.15], [spec.L + 0.6, z0 + 1.1], [spec.L, z0 + 1.1]], t: [0.3, spec.W - 0.3] });
    parts.push({ mat, pts: [[-0.6, z0 + 0.15], [0, z0 + 0.15], [0, z0 + 1.1], [-0.6, z0 + 1.1]], t: [0.3, spec.W - 0.3] });
  }
  if (F.spoiler && !spec.tall) {
    const z = spec.deck;
    parts.push({ layer: 1, mat: 'dark', pts: [[0.4, z], [1.0, z], [1.0, z + 1.1], [0.4, z + 1.1]], t: [1.0, 1.5] });
    parts.push({ layer: 1, mat: 'dark', pts: [[0.4, z], [1.0, z], [1.0, z + 1.1], [0.4, z + 1.1]], t: [spec.W - 1.5, spec.W - 1.0] });
    parts.push({ layer: 1, mat: 'trim', pts: [[-0.5, z + 1.0], [1.7, z + 1.0], [1.5, z + 1.6], [-0.3, z + 1.6]], t: [0.1, spec.W - 0.1] });
  }
  void look;
  return parts;
}

// The soft glow under a car with underglow, baked in so it shows by day too.
function underglow(g, spec, look, alpha) {
  const rx = (spec.L + spec.W) * 0.62;
  const ry = rx * 0.4;
  g.save();
  g.translate(0, 1.2);
  g.scale(1, ry / rx);
  const gr = g.createRadialGradient(0, 0, rx * 0.15, 0, 0, rx);
  gr.addColorStop(0, rgba(look.trim, alpha));
  gr.addColorStop(1, rgba(look.trim, 0));
  g.fillStyle = gr;
  g.beginPath();
  g.arc(0, 0, rx, 0, 6.2832);
  g.fill();
  g.restore();
}

// Paint a car with its ground centre at the origin. `heading` is 0 to 3, as world.js's cars have.
export function paintCar(g, look0, heading = 0) {
  const look = normalizeLook(look0);
  const spec = SPECS[look.body];
  const h = ((Math.trunc(heading) % 4) + 4) % 4;
  const F = Object.fromEntries(look.flair.map((f) => [f, true]));
  const fr = frame(spec, h);
  const cx = { spec, look, F, fr };
  const rx = (spec.L + spec.W) * 0.62;
  g.save();
  g.translate(2.2, 1.4);
  g.scale(1, (3 + (spec.L + spec.W) * 0.06) / rx);
  g.fillStyle = 'rgba(20,18,36,.32)';
  g.beginPath();
  g.arc(0, 0, rx, 0, 6.2832);
  g.fill();
  g.restore();
  if (F.underglow) underglow(g, spec, look, 0.4);
  const depth = (p) => {
    const s = p.pts.reduce((a, q) => a + q[0], 0) / p.pts.length;
    const t = p.t ? (p.t[0] + p.t[1]) / 2 : spec.W / 2;
    const [u, v] = fr.m(s, t);
    return u + v;
  };
  const parts = [...spec.parts, ...extras(spec, look, F)];
  const low = parts.filter((p) => !p.layer).sort((a, b) => depth(a) - depth(b));
  const high = parts.filter((p) => p.layer).sort((a, b) => depth(a) - depth(b));
  for (const p of low) paintPart(g, cx, p);
  wheelsOn(g, cx);
  endLights(g, cx);
  for (const p of high) paintPart(g, cx, p);
  if (F.chrome || F.gold) {
    // A bright line along the side, at the belt.
    const a = fr.at(0.6, fr.sideT, (spec.ride || 1.4) + 1.9);
    const b = fr.at(spec.L - 0.6, fr.sideT, (spec.ride || 1.4) + 1.9);
    g.strokeStyle = F.gold ? GOLD[0] : CHROME;
    g.lineWidth = 0.5;
    g.beginPath();
    g.moveTo(a[0], a[1]);
    g.lineTo(b[0], b[1]);
    g.stroke();
  }
}

// Live: the glow of underglow on the road at night, drawn before the car so it lies under it. `night`
// is 0 by day to 1 at night. Gold cars glow warmly all the time.
export function paintCarGlow(bg, X, Y, look0, night, t = 0) {
  const look = normalizeLook(look0);
  const gold = look.flair.includes('gold');
  const under = look.flair.includes('underglow');
  if (!under && !gold) return;
  const spec = SPECS[look.body];
  const pulse = 0.9 + 0.1 * Math.sin(t * 3);
  const rx = (spec.L + spec.W) * 0.7;
  const ry = rx * 0.42;
  bg.save();
  bg.globalCompositeOperation = 'lighter';
  bg.translate(X, Y + 2);
  bg.scale(1, ry / rx);
  if (under) {
    const gr = bg.createRadialGradient(0, 0, rx * 0.1, 0, 0, rx);
    gr.addColorStop(0, rgba(look.trim, (0.2 + 0.5 * night) * pulse));
    gr.addColorStop(1, rgba(look.trim, 0));
    bg.fillStyle = gr;
    bg.beginPath();
    bg.arc(0, 0, rx, 0, 6.2832);
    bg.fill();
  }
  if (gold) {
    const gr = bg.createRadialGradient(0, -4, rx * 0.1, 0, -4, rx * 0.95);
    gr.addColorStop(0, `rgba(255,214,90,${(0.16 + 0.22 * night) * pulse})`);
    gr.addColorStop(1, 'rgba(255,214,90,0)');
    bg.fillStyle = gr;
    bg.beginPath();
    bg.arc(0, -4, rx, 0, 6.2832);
    bg.fill();
  }
  bg.restore();
}

// Live: the twinkle of gold, drawn over the car. Each glint comes and goes on its own beat.
export function paintCarSparkle(bg, X, Y, look0, t = 0) {
  const look = normalizeLook(look0);
  if (!look.flair.includes('gold')) return;
  const spots = [[-9, -9], [4, -13], [10, -5], [-3, -4], [-12, -2], [7, -9]];
  bg.save();
  spots.forEach(([dx, dy], i) => {
    const a = Math.sin(t * 2.4 + i * 1.9);
    if (a < 0.35) return;
    const k = (a - 0.35) / 0.65;
    const r = 1 + 2.6 * k;
    bg.globalAlpha = k;
    bg.strokeStyle = '#fffbe0';
    bg.lineWidth = 1.1;
    bg.beginPath();
    bg.moveTo(X + dx - r, Y + dy);
    bg.lineTo(X + dx + r, Y + dy);
    bg.moveTo(X + dx, Y + dy - r);
    bg.lineTo(X + dx, Y + dy + r);
    bg.stroke();
    bg.fillStyle = '#ffffff';
    bg.fillRect(X + dx - 0.6, Y + dy - 0.6, 1.2, 1.2);
  });
  bg.restore();
}

// A small picture of a car for the garage: a square canvas `size` pixels across, the car facing
// down-right and filling it.
export function paintCarPreview(look0, size = 96) {
  const look = normalizeLook(look0);
  const spec = SPECS[look.body];
  const c = document.createElement('canvas');
  c.width = size;
  c.height = size;
  const g = c.getContext('2d');
  const fr = frame(spec, 0);
  let x0 = Infinity;
  let x1 = -Infinity;
  let y0 = Infinity;
  let y1 = -Infinity;
  for (const s of [0, spec.L]) {
    for (const t of [0, spec.W]) {
      for (const z of [0, 10.6]) {
        const [x, y] = fr.at(s, t, z);
        x0 = Math.min(x0, x);
        x1 = Math.max(x1, x);
        y0 = Math.min(y0, y);
        y1 = Math.max(y1, y);
      }
    }
  }
  const k = Math.min((size * 0.92) / (x1 - x0), (size * 0.8) / (y1 - y0));
  g.translate(size / 2 - ((x0 + x1) / 2) * k, size / 2 - ((y0 + y1) / 2) * k + size * 0.04);
  g.scale(k, k);
  paintCar(g, look, 0);
  return c;
}

if (typeof window !== 'undefined') window.paintCarPreview = paintCarPreview;
