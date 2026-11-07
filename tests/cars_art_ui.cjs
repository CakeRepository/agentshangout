// Run with node tests/cars_art_ui.cjs. Runs the shipped web/cars.js (CR10) against a fake canvas:
// every body and every flair paints at every heading without throwing, a bad look still paints,
// and the preview comes back as a canvas.
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const rust = fs.readFileSync('src/cars.rs', 'utf8');
const source = fs.readFileSync('web/cars.js', 'utf8').replace(/\r\n/g, '\n');

const list = (name) => {
  const at = rust.indexOf(`${name}: &[&str] = &[`);
  assert.ok(at >= 0, `no ${name} in src/cars.rs`);
  const body = rust.slice(rust.indexOf('= &[', at) + 4, rust.indexOf('];', at));
  return [...body.matchAll(/"(\w+)"/g)].map((m) => m[1]);
};
const bodies = list('BODIES');
const flairs = list('FLAIRS');
assert.ok(bodies.length >= 11 && flairs.length >= 5);

let calls = 0;
const ctx = new Proxy({}, {
  get: (_, k) => (k === 'createLinearGradient' || k === 'createRadialGradient'
    ? () => ({ addColorStop() {} })
    : () => { calls++; }),
  set: () => true,
});
const makeCanvas = () => ({ width: 0, height: 0, getContext: () => ctx });
const c = { document: { createElement: makeCanvas }, window: {} };
vm.createContext(c);
vm.runInContext(`${source.replace(/^export /gm, '')}\nthis.api = { paintCar, paintCarGlow, paintCarSparkle, paintCarPreview, carKey, hasLook, CAR_BODIES };`, c);
const api = c.api;

for (const body of bodies) assert.ok(api.CAR_BODIES.includes(body), `cars.js has no ${body} body`);
for (const body of bodies) {
  for (let h = 0; h < 4; h++) {
    api.paintCar(ctx, { body, color: '#d4af37', trim: '#fff3b0', flair: flairs }, h);
    for (const flair of flairs) api.paintCar(ctx, { body, color: '#336699', trim: '#eeeeee', flair: [flair] }, h);
    api.paintCar(ctx, { body, color: '#336699', trim: '#eeeeee', flair: [] }, h);
  }
}
assert.ok(calls > 1000, 'the cars painted something');

// A look that isn't one still paints.
for (const bad of [null, {}, { body: 'tank', color: 'red', trim: 5, flair: ['x', 3] }, { flair: 'gold' }]) api.paintCar(ctx, bad, 1);
api.paintCarGlow(ctx, 10, 10, { body: 'limo', color: '#111111', trim: '#c7ecee', flair: ['underglow', 'gold'] }, 1, 2);
api.paintCarSparkle(ctx, 10, 10, { body: 'roadster', color: '#d4af37', trim: '#fff3b0', flair: ['gold'] }, 2);

assert.equal(api.carKey({ body: 'van', color: '#aabbcc', trim: '#112233', flair: ['chrome'] }), 'van|#aabbcc|#112233|chrome');
assert.equal(api.hasLook({ body: 'van' }), true);
assert.equal(api.hasLook(null), false);
const preview = api.paintCarPreview({ body: 'pickup', color: '#7a9e5a', trim: '#3d4a30', flair: [] }, 80);
assert.equal(preview.width, 80);
assert.equal(typeof c.window.paintCarPreview, 'function');
console.log('cars art ok');
