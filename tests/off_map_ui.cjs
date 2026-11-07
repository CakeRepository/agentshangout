// Run with node tests/off_map_ui.cjs. Runs the shipped My bots row (S86, S89, S90): a bot in a room with
// no spot on the map says so, and a bot the map can center on keeps its Center button.
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const app = fs.readFileSync('web/app.js', 'utf8').replace(/\r\n/g, '\n');

const from = app.indexOf('const offMapText = (bot) =>');
assert.ok(from >= 0, 'missing offMapText');
const to = app.indexOf('\n}\n', app.indexOf('function yourBotRow(', from)) + 3;
const code = app.slice(from, to);

// A tiny h() that keeps tags, classes and text, so the row can be searched.
const h = (tag, attrs, ...kids) => ({ tag, attrs: attrs || {}, kids: kids.flat().filter((k) => k !== null && k !== undefined && k !== false) });
const text = (node) => (typeof node === 'string' ? node : node.kids.map(text).join(' '));
const find = (node, pred) => {
  if (typeof node === 'string') return [];
  return [...(pred(node) ? [node] : []), ...node.kids.flatMap((kid) => find(kid, pred))];
};
const c = {
  h,
  zoomed: () => h('canvas', {}),
  figurePicture: () => ({}),
  LOOK_ZOOM_SMALL: 1,
  statusText: () => 'Idle',
  where: (bot) => (bot.room ? `In ${bot.room.name}` : 'Not in any room right now'),
  DOING: {},
  agoText: () => 'now',
  untilText: () => 'soon',
  NEXT: {},
  link: (href, label) => h('a', { href }, label),
  routineDialog: async () => {},
  deleteBotDialog: () => {},
  alert: () => {},
};
vm.createContext(c);
vm.runInContext(`${code}\nthis.yourBotRow = yourBotRow; this.offMapText = offMapText;`, c);

const lab = { name: 'axiom', room: { id: 'r1', name: 'Innovation Lab' }, status: { state: 'idle' }, doing: 'chat' };
const row = c.yourBotRow(lab, { offMap: true, reload: () => {} });
const off = find(row, (n) => String(n.attrs.class || '').includes('off-map'));
assert.equal(off.length, 1, 'the row says it is off the map');
assert.equal(text(off[0]), "Innovation Lab isn't on the map, so there's nowhere to center on it");
assert.equal(find(row, (n) => n.tag === 'button' && n.kids.includes('Center')).length, 0, 'no Center');

// S90: the person who runs that space also gets a link to its settings; nobody else does.
const mine = c.yourBotRow(lab, { offMap: true, settingsPath: '/s/sp 1', reload: () => {} });
const links = find(mine, (n) => n.tag === 'a' && text(n) === 'Space settings');
assert.equal(links.length, 1);
assert.equal(links[0].attrs.href, '/s/sp 1');
assert.equal(text(links[0]), 'Space settings');
assert.equal(find(row, (n) => n.tag === 'a' && text(n) === 'Space settings').length, 0, 'no link without settingsPath');
const mineOnMap = c.yourBotRow(lab, { focusOn: () => {}, settingsPath: '/s/x', reload: () => {} });
assert.equal(find(mineOnMap, (n) => n.tag === 'a' && text(n) === 'Space settings').length, 0, 'a link only rides on the off-map line');

// A bot the map can center on keeps Center and says nothing about the map.
const onMap = c.yourBotRow({ ...lab, room: { id: 'r2', name: 'Fairground' } }, { focusOn: () => {}, reload: () => {} });
assert.equal(find(onMap, (n) => String(n.attrs.class || '').includes('off-map')).length, 0);
assert.equal(find(onMap, (n) => n.tag === 'button' && n.kids.includes('Center')).length, 1);
// Outside the map (a room's My bots), there's no such line either.
const inRoom = c.yourBotRow(lab, { reload: () => {} });
assert.equal(find(inRoom, (n) => String(n.attrs.class || '').includes('off-map')).length, 0);
assert.equal(c.offMapText({}), "Its room isn't on the map, so there's nowhere to center on it");

// The map's list marks a bot off the map only when it's in a room and has no tile.
assert.ok(app.includes('const offMap = !tile && Boolean(bot.room);'));
assert.ok(app.includes('return yourBotRow(bot, { focusOn, offMap, settingsPath:'));
// loadBots (S90): runs the shipped code with a stubbed api, asking /api/spaces only when a bot is off the map.
const slice = (start, end) => app.slice(app.indexOf(start), app.indexOf(end, app.indexOf(start)));
const loader = [
  'let myBots = [];\nlet offMapFloors = new Map();',
  slice('  // Where the map can center on a bot', '  function renderBots()'),
  slice('  let botsAt = 0;', '\n  const aboutBody'),
].join('\n');
async function run({ bots, spaces, latest = { bots_out: [] } }) {
  const calls = [];
  const seen = {};
  const ctx = {
    me: { kind: 'person', name: 'justin' },
    latest,
    world: {},
    renderBots: () => { seen.bots = JSON.parse(ctx.snapshot()); },
    api: async (path) => {
      calls.push(path);
      if (path === '/api/me/bots') return { bots };
      if (path === '/api/cars') return { cars: [], owned: [] };
      if (path === '/api/spaces') return spaces();
      throw new Error(`unexpected ${path}`);
    },
  };
  vm.createContext(ctx);
  vm.runInContext(`${loader}\nthis.snapshot = () => JSON.stringify({ myBots, floors: [...offMapFloors] }); this.loadBots = loadBots;`, ctx);
  await ctx.loadBots(true);
  return { calls, seen: seen.bots, world: ctx.world };
}
const spaceList = () => ({
  house: { on_map: false, space: { id: 'sp1' }, floors: [{ id: 'r1' }] },
  third: [{ on_map: true, space: { id: 'sp2' }, floors: [{ id: 'r2' }] }],
});
(async () => {
  // No bot is in a room off the map (one has a tile, one has no room): no /api/spaces request.
  const quiet = await run({ bots: [{ name: 'a', room: { id: 'r2' }, place: { x: 1, y: 2 } }, { name: 'b', room: null }], spaces: () => { throw new Error('asked'); } });
  assert.deepEqual(quiet.calls, ['/api/me/bots', '/api/cars']);
  assert.equal(quiet.seen.myBots.length, 2);
  assert.deepEqual(quiet.seen.floors, []);
  // A bot out on the map by the live feed counts as on the map too.
  const live = await run({ bots: [{ name: 'a', room: { id: 'r2' } }], latest: { bots_out: [['a', 3, 4]] }, spaces: () => { throw new Error('asked'); } });
  assert.deepEqual(live.calls, ['/api/me/bots', '/api/cars']);
  // An off-map bot gets its space from /api/spaces, so the row can link to the settings.
  const off = await run({ bots: [{ name: 'a', room: { id: 'r1' } }], spaces: spaceList });
  assert.deepEqual(off.calls, ['/api/me/bots', '/api/cars', '/api/spaces']);
  assert.deepEqual(off.seen.floors, [['r1', 'sp1']]);
  assert.ok(off.world.myBots.has('a'));
  // /api/spaces failing still draws the rows, without the link.
  const failed = await run({ bots: [{ name: 'a', room: { id: 'r1' } }], spaces: () => { throw new Error('down'); } });
  assert.deepEqual(failed.calls, ['/api/me/bots', '/api/cars', '/api/spaces']);
  assert.equal(failed.seen.myBots.length, 1);
  assert.deepEqual(failed.seen.floors, []);
  console.log('My bots: a bot in a room off the map says so; one on the map keeps Center.');
})().catch((failure) => { console.error(failure); process.exit(1); });
