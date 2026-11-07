// Run with node tests/garage_ui.cjs. Runs the shipped garage code (CR3, CR4, CR11): a bot's row shows
// its car with a select over the cars you own, and the Garage lists every car with Owned or Buy.
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const app = fs.readFileSync('web/app.js', 'utf8').replace(/\r\n/g, '\n');

const from = app.indexOf('function carSwatch(car)');
const to = app.indexOf('// S86: one of your bots, as a row', from);
assert.ok(from >= 0 && to > from, 'missing the garage functions');
const code = app.slice(from, to);

const h = (tag, attrs, ...kids) => ({ tag, attrs: attrs || {}, kids: kids.flat().filter((k) => k !== null && k !== undefined && k !== false) });
const text = (node) => (typeof node === 'string' ? node : node.kids.map(text).join(''));
const find = (node, pred) => (typeof node === 'string' ? [] : [...(pred(node) ? [node] : []), ...node.kids.flatMap((kid) => find(kid, pred))]);
const alerts = [];
const c = { h, alert: (message) => alerts.push(message), Promise };
vm.createContext(c);
vm.runInContext(`${code}\nthis.botCarLine = botCarLine; this.garageList = garageList; this.carSwatch = carSwatch;`, c);

const pip = { id: 'pip', name: 'Pip', price: 0, starter: true, speed: 3, look: { color: '#e8c547', trim: '#4a4a4a' } };
const dinky = { id: 'dinky', name: 'Dinky', price: 50, starter: false, speed: 3.2, look: { color: '#d9534f', trim: '#3a2a2a' } };
const goose = { id: 'goose', name: 'Golden Goose', price: 1000000, starter: false, speed: 12, look: { color: '#d4af37', trim: '#fff3b0' } };

(async () => {
  // A swatch in the car's colour, until the art's paintCarPreview exists.
  assert.match(c.carSwatch(dinky).attrs.style, /background:#d9534f/);
  c.paintCarPreview = (car) => h('canvas', { 'data-car': car.id });
  assert.equal(c.carSwatch(dinky).tag, 'canvas');
  c.paintCarPreview = () => { throw new Error('art broke'); };
  assert.equal(c.carSwatch(dinky).tag, 'span', 'a failing preview falls back to the swatch');
  delete c.paintCarPreview;

  // Only the starters owned: no select, just the name.
  const bot = { id: 'b1', name: 'axiom', car: pip };
  const plain = c.botCarLine(bot, { owned: [pip], choose: async () => {} });
  assert.equal(find(plain, (n) => n.tag === 'select').length, 0);
  assert.equal(text(plain).trim(), 'Pip');
  assert.equal(c.botCarLine({ name: 'x' }, null), null, 'no car, no line');

  // Several owned: a select with each one, the bot's own selected, and a change calls choose.
  const chosen = [];
  const line = c.botCarLine(bot, { owned: [pip, dinky], choose: async (b, id) => { chosen.push([b.id, id]); } });
  const [select] = find(line, (n) => n.tag === 'select');
  const options = find(select, (n) => n.tag === 'option');
  assert.deepEqual(options.map((o) => o.attrs.value), ['pip', 'dinky']);
  assert.deepEqual(options.map((o) => Boolean(o.attrs.selected)), [true, false]);
  select.attrs.onchange({ target: { value: 'dinky' } });
  assert.deepEqual(chosen, [['b1', 'dinky']]);
  // A refused choice is shown, not swallowed.
  const refused = c.botCarLine(bot, { owned: [pip, dinky], choose: async () => { throw new Error("you don't own that car"); } });
  find(refused, (n) => n.tag === 'select')[0].attrs.onchange({ target: { value: 'dinky' } });
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(alerts, ["you don't own that car"]);

  // The Garage: the balance, then each car as Owned, Buy, or a Buy it can't afford.
  const bought = [];
  const rows = c.garageList({ cars: [pip, dinky, goose], owned: ['pip'], balance: 60 }, (car) => bought.push(car.id));
  assert.match(text(rows[0]), /Your balance: 60 credits/);
  const cars = rows.slice(1);
  assert.equal(cars.length, 3);
  assert.match(text(cars[0]), /Pip.*Starter · speed 3/);
  assert.equal(find(cars[0], (n) => n.attrs.class === 'badge').map(text)[0], 'Owned');
  assert.match(text(cars[1]), /Dinky.*50 credits · speed 3.2/);
  const [buyDinky] = find(cars[1], (n) => n.tag === 'button');
  assert.ok(!buyDinky.attrs.disabled, 'a car you can afford can be bought');
  buyDinky.attrs.onclick();
  assert.deepEqual(bought, ['dinky']);
  assert.match(text(cars[2]), /1,000,000 credits/);
  assert.ok(find(cars[2], (n) => n.tag === 'button')[0].attrs.disabled, 'a car you cannot afford is disabled');
  console.log('Garage: bots show and change their car, and the Garage sells what you can afford.');
})().catch((failure) => { console.error(failure); process.exit(1); });
