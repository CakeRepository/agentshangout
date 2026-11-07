// Run with node tests/my_bots_ui.cjs
// L58: My bots view in Bots panel, click-to-centre, and highlighting owner's bots on the map.
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');

const appSource = fs.readFileSync('web/app.js', 'utf8').replace(/\r\n/g, '\n');
const worldSource = fs.readFileSync('web/world.js', 'utf8').replace(/\r\n/g, '\n');

// 1. Verify drawPlayer and drawCar highlight logic in world.js
function extractFrom(source, startKeyword, endKeyword) {
  const start = source.indexOf(startKeyword);
  assert.ok(start >= 0, `missing ${startKeyword}`);
  const end = source.indexOf(endKeyword, start);
  assert.ok(end > start, `missing ${endKeyword}`);
  return source.slice(start, end + endKeyword.length);
}

// Minimal Canvas 2D mock to check strokes & shadows
function makeMockContext() {
  const operations = [];
  return {
    operations,
    save() { operations.push('save'); },
    restore() { operations.push('restore'); },
    beginPath() { operations.push('beginPath'); },
    ellipse(x, y, rx, ry, rot, start, end) { operations.push({ op: 'ellipse', x, y, rx, ry }); },
    stroke() { operations.push('stroke'); },
    fill() { operations.push('fill'); },
    drawImage(...args) { operations.push({ op: 'drawImage', args }); },
    fillRect() {},
    createRadialGradient() { return { addColorStop() {} }; },
    strokeStyle: '',
    fillStyle: '',
    shadowColor: '',
    shadowBlur: 0,
    lineWidth: 1,
  };
}

// 2. Test drawPlayer and drawCar with mock context
{
  const mockBgMine = makeMockContext();
  const mockBgNotMine = makeMockContext();

  const dummySprite = { c: {}, w: 20, h: 28, ax: 10, ay: 20 };
  const dummyPlayer = {
    dx: 100, dy: 200, born: 0, leaving: 0, emote: null, phase: 0, walking: false, face: 'fr',
    kind: 'bot', color: '#8866cc', look: {}, eyes: null, chest: null, hat: null,
  };
  const dummyCar = { dx: 150, dy: 250, heading: 0, color: 1 };

  const worldContext = {
    visitorSprite: () => dummySprite,
    botSprite: () => dummySprite,
    carSprite: () => dummySprite,
    drawEmote: () => {},
    ell: () => {},
    clamp: (x, a, b) => Math.max(a, Math.min(b, x)),
  };
  vm.createContext(worldContext);

  // Extract drawPlayer and drawCar
  const drawPlayerCode = extractFrom(worldSource, 'function drawPlayer(bg, p, t, clock, hovered, calm, mine) {', '\n}\n');
  const drawCarCode = extractFrom(worldSource, 'function drawCar(bg, car, night, mine) {', '\n}\n');
  vm.runInContext(`${drawPlayerCode}\n${drawCarCode}`, worldContext);

  // When mine is true for drawPlayer:
  worldContext.drawPlayer(mockBgMine, dummyPlayer, 0, 1, false, false, true);
  assert.equal(mockBgMine.strokeStyle, '#f2b632', 'drawPlayer should use gold stroke when mine');
  assert.equal(mockBgMine.shadowColor, '#f2b632', 'drawPlayer should use gold shadow blur when mine');
  assert.ok(mockBgMine.operations.includes('stroke'), 'drawPlayer should stroke ellipse when mine');

  // When mine is false for drawPlayer:
  worldContext.drawPlayer(mockBgNotMine, dummyPlayer, 0, 1, false, false, false);
  assert.notEqual(mockBgNotMine.strokeStyle, '#f2b632', 'drawPlayer must not set gold stroke when not mine');
  assert.notEqual(mockBgNotMine.shadowColor, '#f2b632', 'drawPlayer must not set gold shadow when not mine');

  // When mine is true for drawCar:
  const mockCarMine = makeMockContext();
  const mockCarNotMine = makeMockContext();
  worldContext.drawCar(mockCarMine, dummyCar, false, true);
  assert.equal(mockCarMine.strokeStyle, '#f2b632', 'drawCar should use gold stroke when mine');
  assert.equal(mockCarMine.shadowColor, '#f2b632', 'drawCar should use gold shadow when mine');

  worldContext.drawCar(mockCarNotMine, dummyCar, false, false);
  assert.notEqual(mockCarNotMine.strokeStyle, '#f2b632', 'drawCar must not set gold stroke when not mine');
  assert.notEqual(mockCarNotMine.shadowColor, '#f2b632', 'drawCar must not set gold shadow when not mine');
}

// 3. Test owner matching in world.js: another owner's bot with the same name is never marked,
// and a cleared ownerName (e.g. sign-out or account switch) marks nothing.
{
  assert.ok(
    worldSource.includes('Boolean(this.ownerName && e.owner === this.ownerName && this.myBots?.has(e.name))'),
    'world.js must require explicit owner match when highlighting entities'
  );
  assert.ok(
    worldSource.includes('if (this.ownerName && p.owner === this.ownerName && this.myBots?.has(p.name)) there.mine = true;'),
    'world.js must require explicit owner match when marking building roof indicators'
  );

  const worldMock = {
    myBots: new Set(['Scout']),
    ownerName: 'Justin',
  };
  const isMineBot = (e, ownerName = worldMock.ownerName) => Boolean(ownerName && e.owner === ownerName && worldMock.myBots?.has(e.name));

  assert.equal(isMineBot({ name: 'Scout', owner: 'Justin' }), true, "viewer's Scout is marked");
  assert.equal(isMineBot({ name: 'Scout', owner: 'SomeoneElse' }), false, "stranger's Scout with identical name is never marked");
  assert.equal(isMineBot({ name: 'Scout', owner: '' }), false, "ownerless Scout is never marked");
  assert.equal(isMineBot({ name: 'Stranger', owner: 'Justin' }), false, "unowned bot name is never marked");

  // When ownerName is cleared (signed out / account switch), nothing is marked:
  assert.equal(isMineBot({ name: 'Scout', owner: 'Justin' }, null), false, "cleared ownerName marks nothing");
  assert.equal(isMineBot({ name: 'Scout', owner: 'Justin' }, ''), false, "empty ownerName marks nothing");
}

// 4. Test Bots panel renderBots in app.js
{
  function node(tag, props = {}, ...children) {
    return {
      tag,
      ...props,
      children: children.flat().filter(Boolean),
      replaceChildren(...next) { this.children = next.flat().filter(Boolean); },
    };
  }

  function allNodes(tree, predicate, out = []) {
    if (!tree || typeof tree !== 'object') return out;
    if (predicate(tree)) out.push(tree);
    (tree.children || []).forEach((c) => allNodes(c, predicate, out));
    return out;
  }

  const focused = [];
  const panelsClosed = [];
  const appContext = {
    me: { name: 'Justin', kind: 'person' },
    h: node,
    link: (to, label, props) => node('a', { href: to, ...props }, label),
    zoomed: (el) => el,
    figurePicture: () => node('canvas'),
    LOOK_ZOOM_SMALL: 1,
    STATUS_ORDER: { working: 0, idle: 1, asleep: 2, offline: 3 },
    DOING: {},
    NEXT: {},
    KIND_WORDS: {},
    agoText: () => 'recently',
    untilText: () => 'soon',
    statusText: (bot) => bot.status?.state || 'idle',
    nameAt: () => 'The Park',
    routineDialog: async () => {},
    deleteBotDialog: async () => {},
    makeBot: () => {},
    where: (bot) => bot.room?.name || 'In room',
    world: {
      myBots: new Set(['Rover', 'Scout']),
      ownerName: 'Justin',
      focus: (x, y, opts) => { focused.push({ x, y, opts }); },
    },
    panels: {
      close: (val) => { panelsClosed.push(val); },
    },
  };

  // Find the exact renderBots inside worldView
  const marker = "  let botsTab = 'yours';";
  const mIdx = appSource.indexOf(marker);
  assert.ok(mIdx >= 0, 'marker not found in app.js');
  const start = appSource.indexOf('  // Where the map can center on a bot', mIdx);
  assert.ok(start >= 0 && start < appSource.indexOf('  function renderBots() {', mIdx), 'botTile not found');
  const endStr = '    botsBody.replaceChildren(tabs, ...rows);\n  }';
  const end = appSource.indexOf(endStr, start) + endStr.length;
  const renderBotsCode = appSource.slice(start, end);
  // S86: each row is drawn by the shared yourBotRow, which a room's My bots uses too.
  const rowStart = appSource.indexOf('function yourBotRow(');
  assert.ok(rowStart >= 0, 'yourBotRow not found in app.js');
  const yourBotRowCode = appSource.slice(rowStart, appSource.indexOf('\n}\n', rowStart) + 2);

  vm.createContext(appContext);
  vm.runInContext(`
    let botsTab = 'yours';
    let garage = null;
    let myBots = [
      {
        name: 'Rover',
        status: { state: 'working' },
        place: { x: 42, y: 88, name: 'Main Lab' },
        room: { id: 'room-1', name: 'Main Lab' },
      },
      {
        name: 'Scout',
        status: { state: 'idle' },
        place: null,
        room: { id: 'room-2', name: 'Commons Fairground' },
      }
    ];
    let latest = {
      bots_out: [
        ['Scout', 12, 14, {}, null, null, true, 'Justin', [12, 14]],
        ['Scout', 50, 60, {}, null, null, true, 'SomeoneElse', [50, 60]],
        ['Stranger', 30, 40, {}, null, null, true, 'SomeoneElse', [30, 40]],
      ]
    };
    const botsBody = { children: [], replaceChildren(...next) { this.children = next.flat().filter(Boolean); } };
    ${yourBotRowCode}
    ${renderBotsCode}
    this.renderBots = renderBots;
    this.setBotsTab = (t) => { botsTab = t; };
    this.getBotsBody = () => botsBody;
  `, appContext);

  // Test 4.1: Tablist contains 'My bots' and 'Everyone out'
  appContext.renderBots();
  const body = appContext.getBotsBody();
  const tabs = allNodes(body, (n) => n.role === 'tab');
  assert.equal(tabs.length, 3, 'should have 3 tabs');
  assert.equal(tabs[0].children[0], 'My bots', 'first tab should be labeled My bots');
  assert.equal(tabs[1].children[0], 'Garage', 'second tab should be labeled Garage');
  assert.equal(tabs[2].children[0], 'Everyone out', 'third tab should be labeled Everyone out');

  // Test 4.2: In 'My bots', Rover has a Center button focusing (42, 88)
  const centerButtons = allNodes(body, (n) => n.tag === 'button' && n.children[0] === 'Center');
  assert.equal(centerButtons.length, 2, 'both bots with known location have Center buttons');

  // Click Center for Rover
  centerButtons[0].onclick();
  const lastRover = focused.at(-1);
  assert.equal(lastRover.x, 42);
  assert.equal(lastRover.y, 88);
  assert.equal(lastRover.opts.zoom, 1.5);

  // Click Center for Scout (coordinates derived from latest.bots_out [12, 14])
  centerButtons[1].onclick();
  const lastScout = focused.at(-1);
  assert.equal(lastScout.x, 12);
  assert.equal(lastScout.y, 14);
  assert.equal(lastScout.opts.zoom, 1.5);

  // Test 4.3: In 'Everyone out' tab, viewer's bot (Justin's Scout) has a Yours badge and mine class
  // Stranger's bot with the same name ('SomeoneElse's Scout) does NOT have Yours badge or mine class!
  appContext.setBotsTab('everyone');
  appContext.renderBots();
  const everyoneBody = appContext.getBotsBody();

  const allRows = everyoneBody.children.slice(1);
  assert.equal(allRows.length, 3, 'should render 3 bot rows in Everyone out');

  // Justin's Scout:
  const justinScoutRow = allRows[0];
  assert.ok(justinScoutRow.class.includes('mine'), "Justin's Scout row should have mine class");
  const justinBadge = allNodes(justinScoutRow, (n) => n.children?.includes('Yours'));
  assert.equal(justinBadge.length, 1, "Justin's Scout must have Yours badge");

  // SomeoneElse's Scout (same name!):
  const strangerScoutRow = allRows[1];
  assert.ok(!strangerScoutRow.class.includes('mine'), "SomeoneElse's Scout must not have mine class");
  const strangerScoutBadge = allNodes(strangerScoutRow, (n) => n.children?.includes('Yours'));
  assert.equal(strangerScoutBadge.length, 0, "SomeoneElse's Scout must never have Yours badge");

  // Stranger:
  const strangerRow = allRows[2];
  assert.ok(!strangerRow.class.includes('mine'), 'Stranger row must not have mine class');
  const strangerBadge = allNodes(strangerRow, (n) => n.children?.includes('Yours'));
  assert.equal(strangerBadge.length, 0, 'Stranger row must not have Yours badge');

  // Test 4.4: When signed out (me = null), no row has Yours badge or mine class
  appContext.me = null;
  appContext.renderBots();
  const signedOutBody = appContext.getBotsBody();
  const signedOutRows = signedOutBody.children.slice(1);
  assert.equal(signedOutRows.length, 3);
  for (const r of signedOutRows) {
    assert.ok(!r.class?.includes('mine'), 'signed out user must not see mine class');
    const b = allNodes(r, (n) => n.children?.includes('Yours'));
    assert.equal(b.length, 0, 'signed out user must not see Yours badge');
  }
}

console.log('tests/my_bots_ui.cjs: all tests passed!');
