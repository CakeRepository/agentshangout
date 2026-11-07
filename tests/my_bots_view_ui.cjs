// node tests/my_bots_view_ui.cjs: S86, one My bots button and large panel for Trust, Bots and Crew.
const fs = require('node:fs'), vm = require('node:vm'), assert = require('node:assert/strict');
const app = fs.readFileSync('web/app.js', 'utf8').replace(/\r\n/g, '\n');
const css = fs.readFileSync('web/trust.css', 'utf8').replace(/\r\n/g, '\n');

// The map: one bolder My bots button, a My bots panel that holds the list and Trust, no Trust launcher.
assert.ok(app.includes("panels.toggle('bots', 'My bots', { short: 'Bots', className: 'my-bots-button' })"));
assert.ok(app.includes("panels.add('bots', 'My bots', { onOpen: () => { loadBots(true); trust?.load(); } }, botsBody, trustHost)"));
assert.ok(app.includes("host: trustHost, hostPanel: 'bots'"));
assert.ok(!app.includes('trust-launcher') && !css.includes('trust-launcher'), 'no separate Trust launcher');
assert.ok(!app.includes("moreRow('bots'"), 'My bots never folds into More, so More has no row for it');
// The phone tab bar: one My bots tab in place of Trust and Crew.
assert.ok(app.includes("{ id: 'bots', label: 'My bots', icon: '👥'"));
assert.ok(!app.includes("{ id: 'trust', label: 'Trust'") && !app.includes("{ id: 'crew', label: 'Crew'"));
// A room: the same button, opening on who's in this room, then your own bots (not for a guest).
assert.ok(app.includes("panels.toggle('crew', 'My bots', { short: 'Bots', className: 'my-bots-button' })"));
assert.ok(app.includes("panels.add('crew', 'My bots', { onOpen: () => { loadRoomBots(); roomTrust?.load(); } },\n    h('h3', { class: 'my-bots-here' }, 'In this room'),"));
// The same list of your bots as the map, with pictures, in a room too (not for a guest).
assert.ok(app.includes("const roomBotsList = roomTrustHost ? h('div', { class: 'my-bots-list' }) : null;"));
assert.ok(app.includes('...yourBotsSummary(bots, changed)'));
// A change (a Delete) reads each bot's tabs again too, on the map and in a room.
assert.ok(app.includes('const changed = async () => { await loadRoomBots(); roomTrust?.load(); };'));
assert.ok(app.includes('return yourBotRow(bot, { focusOn, offMap, settingsPath: spaceId ? `/s/${encodeURIComponent(spaceId)}` : null, garage: carPicker, reload: async () => { await loadBots(true); trust?.load(); } });'), 'the map draws its rows with the same function');
// A list that couldn't be read says so, rather than looking like you have no bots.
assert.ok(app.includes("h('p', { class: 'error' }, `Couldn't load your bots: ${error.message}`)"));
assert.ok(app.includes("zoomed(figurePicture({ kind: 'bot', color: bot.color || '#8866cc', worn: bot.look || {} }), LOOK_ZOOM_SMALL)"), "each row has the bot's picture");
// Run it: a guest, or a bot, gets no section about their own bots; a person in their room does.
const guardStart = app.indexOf('function yourBotsSection(');
const guard = app.slice(guardStart, app.indexOf('\n}\n', guardStart) + 2);
const guardContext = { h: (tag, props) => ({ tag, props }) };
vm.createContext(guardContext);
vm.runInContext(guard + '\nthis.yourBotsSection = yourBotsSection;', guardContext);
const person = { kind: 'person' };
assert.equal(guardContext.yourBotsSection(person, { guest: { until: 1 } }), null, 'a guest sees only who is here');
assert.equal(guardContext.yourBotsSection({ kind: 'bot' }, {}), null, 'a bot gets no section');
assert.equal(guardContext.yourBotsSection(null, {}), null, 'nor someone signed out');
assert.equal(guardContext.yourBotsSection(person, { guest: null }).tag, 'section');
assert.ok(app.includes('const roomTrustHost = yourBotsSection(me, snapshot);'));
// The phone tabs: Who's around stays a tab for everyone, and a room's tab is named for what it opens.
assert.ok(app.includes("{ id: 'around', label: 'Around', icon: '👀'"));
assert.ok(app.includes("{ id: 'guests', label: 'My bots', icon: '👥', action: () => panels.open('crew') }"));
// Full screen, on both.
assert.ok(css.includes('.wd-game > .game-panel-bots, .wd-game > .game-panel-crew { width: min(880px, calc(100% - 24px)); max-width: none; }'), 'a large panel, as tall as its contents');
assert.ok(!css.includes('height: calc(100% - 24px); max-height: none;'), 'not stretched to the full height');

// Trust inside a host: draws into it, has no panel or button of its own, and opens the host panel.
const h = (tag, props = {}, ...children) => ({ tag, props, children, classList: { add() {} }, append() {}, replaceChildren() {} });
const added = [], toggled = [], opened = [];
const panels = { add: (name) => { added.push(name); return h('div'); }, toggle: (name) => { toggled.push(name); return h('button'); }, open: (name) => opened.push(name) };
const context = { h, api: async () => ({}), panels, document: { createElement: () => ({}) }, console };
vm.createContext(context);
vm.runInContext(fs.readFileSync('web/trust.js', 'utf8').replace('export function trustPanel', 'function trustPanel') + '\nthis.trustPanel = trustPanel;', context);
const host = h('section');
const hosted = context.trustPanel({ h, api: context.api, panels, me: { kind: 'person' }, host, hostPanel: 'bots' });
assert.deepEqual([added, toggled], [[], []], 'no panel and no top-bar button of its own');
assert.equal(hosted.toggle, null);
hosted.open();
assert.deepEqual(opened, ['bots'], 'opening it opens the My bots view');
assert.equal(typeof hosted.load, 'function');
// Without a host it is the panel it always was.
const alone = context.trustPanel({ h, api: context.api, panels, me: { kind: 'person' } });
assert.deepEqual([added, toggled], [['trust'], ['trust']]);
alone.open();
assert.deepEqual(opened, ['bots', 'trust']);
console.log('My bots view: one button and view on the map and in a room, Trust hosted inside it, as a large panel.');
