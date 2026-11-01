// A tile's outside, in the order it's painted (E29). Everything here is drawn at the sprite's own
// resolution by web/outside-building.js, web/outside-yard.js and web/outside-decor.js. The ground goes
// with the land, under every shadow, and everything else stands up, back to front.

import { itemsOf, rectOf } from './outside-spec.js';
import { paintBuilding } from './outside-building.js';
import { DECOR_ART } from './outside-decor.js';
import { YARD_ART, paintCenterpiece, paintFence, paintGroundCover, paintPath, paintStringLights } from './outside-yard.js';

// A town square has no building, so its centrepiece stands in for one when things are sorted.
const MIDDLE = [12, 12, 20, 20];

export function paintOutsideGround(S, kind, outside) {
  paintGroundCover(S, kind, outside);
  paintPath(S, kind, outside);
}

// An item of a kind that has no art yet shows nothing, as any word this page doesn't know (E4).
function paintItem(S, item) {
  (YARD_ART[item.kind] || DECOR_ART[item.kind])?.(S, item);
}

// Whether `a` ends before `b` begins, along u or along v, so `b` stands in front of it.
const before = (a, b) => a.u1 <= b.u0 || a.v1 <= b.v0;

// Back to front. Sorting by corner alone puts a long thing in front of a tall one that stands behind its
// far end, so each thing goes after everything that is wholly behind it, and a tie goes to the nearer middle.
function backToFront(things) {
  const n = things.length;
  const after = things.map(() => []);
  const waiting = things.map(() => 0);
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < n; j++) {
      if (i === j || !before(things[i], things[j]) || before(things[j], things[i])) continue;
      after[i].push(j);
      waiting[j]++;
    }
  }
  const depth = (t) => t.u0 + t.u1 + t.v0 + t.v1;
  const done = new Array(n).fill(false);
  const out = [];
  while (out.length < n) {
    let best = -1;
    for (let pass = 0; pass < 2 && best < 0; pass++) {
      for (let i = 0; i < n; i++) {
        if (done[i] || (pass === 0 && waiting[i] > 0)) continue;
        if (best < 0 || depth(things[i]) < depth(things[best])) best = i;
      }
    }
    done[best] = true;
    out.push(things[best]);
    for (const j of after[best]) waiting[j]--;
  }
  return out;
}

export function paintOutsideStanding(S, kind, outside, colors) {
  const rect = rectOf(kind, outside);
  const [u0, v0, u1, v1] = rect || MIDDLE;
  const items = itemsOf(outside);
  // The building, or a square's middle, stands among the items in the same order.
  const middle = { u0, v0, u1, v1, middle: true };
  paintFence(S, kind, outside, 'back');
  for (const thing of backToFront([...items, middle])) {
    if (!thing.middle) paintItem(S, thing);
    else if (!rect) paintCenterpiece(S, outside, kind);
    else paintBuilding(S, kind, outside, colors);
  }
  paintFence(S, kind, outside, 'front');
  if (!rect) paintStringLights(S, outside);
}
