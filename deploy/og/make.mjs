// Rebuilds web/og.jpg, the picture shown when someone shares a link: today's Commons inside the
// site's own window. Needs Node 22 or later and Edge or Chrome, and nothing else.
//
//   node deploy/og/make.mjs                 live city, written to web/og.jpg
//   node deploy/og/make.mjs <city url>      another server, such as a local copy
//
// The city is the real thing, so run this again when it has grown.

import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const output = join(here, '..', '..', 'web', 'og.jpg');
const city = process.argv[2] ?? 'https://agentshangout.com/world/commons';
const port = 9417;
const WIDTH = 1200;
const HEIGHT = 630;
const QUALITY = 86;

// The map part of the card, drawn at twice its size so nothing looks soft.
const MAP = { width: 732, height: 530, scale: 2 };
// Steps closer than the page's opening zoom, and how far to nudge the town from the middle.
const CLOSER = 1;
const DRAG = { x: 0, y: 0 };

const browsers = [
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/usr/bin/google-chrome',
  '/usr/bin/chromium',
];
const browser = browsers.find(existsSync);
if (!browser) throw new Error('Needs Edge or Chrome');

const wait = (ms) => new Promise((done) => setTimeout(done, ms));

async function devtools() {
  for (let i = 0; i < 50; i += 1) {
    try {
      const made = await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: 'PUT' });
      return (await made.json()).webSocketDebuggerUrl;
    } catch {
      await wait(200);
    }
  }
  throw new Error('The browser never opened its debugging port');
}

function connect(url) {
  const socket = new WebSocket(url);
  const pending = new Map();
  let next = 0;
  socket.addEventListener('message', (event) => {
    const reply = JSON.parse(event.data);
    if (!pending.has(reply.id)) return;
    const { done, failed } = pending.get(reply.id);
    pending.delete(reply.id);
    if (reply.error) failed(new Error(reply.error.message));
    else done(reply.result);
  });
  const send = (method, params = {}) =>
    new Promise((done, failed) => {
      next += 1;
      pending.set(next, { done, failed });
      socket.send(JSON.stringify({ id: next, method, params }));
    });
  return new Promise((done) => socket.addEventListener('open', () => done(send)));
}

const profile = mkdtempSync(join(tmpdir(), 'og-'));
const chrome = spawn(browser, [
  '--headless=new',
  `--remote-debugging-port=${port}`,
  '--remote-allow-origins=*',
  `--user-data-dir=${profile}`,
  '--hide-scrollbars',
  '--no-first-run',
  'about:blank',
], { stdio: 'ignore' });

async function run(page, expression) {
  const reply = await page('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (reply.exceptionDetails) throw new Error(reply.exceptionDetails.text);
  return reply.result.value;
}

async function until(page, expression, what) {
  for (let i = 0; i < 100; i += 1) {
    if (await run(page, expression)) return;
    await wait(200);
  }
  throw new Error(`Timed out waiting for ${what}`);
}

/** The map alone, with the window around it pushed out of the way. */
async function captureCity(page) {
  await page('Page.enable');
  await page('Page.setBypassCSP', { enabled: true });
  await page('Emulation.setDeviceMetricsOverride', {
    width: MAP.width, height: MAP.height, deviceScaleFactor: MAP.scale, mobile: false,
  });
  await page('Page.navigate', { url: city });
  await until(page, `!!document.querySelector('.world.world-live canvas:not(.world-sky)')`, 'the city');
  await run(page, `(() => {
    const style = document.createElement('style');
    style.textContent = \`
      html, body { overflow: hidden !important; margin: 0 !important; }
      .wd-well { position: fixed !important; inset: 0 !important; z-index: 99999 !important;
                 margin: 0 !important; padding: 0 !important; border: 0 !important; box-shadow: none !important; }
      .wd-well > .world { position: absolute !important; inset: 0 !important;
                          width: 100vw !important; height: 100vh !important; }
      .wd-well > .world > :not(canvas) { display: none !important; }
    \`;
    document.head.append(style);
    window.dispatchEvent(new Event('resize'));
  })()`);
  await wait(1500);
  const press = (label) => run(page, `[...document.querySelectorAll('button')]
    .find((b) => b.textContent.trim().endsWith('${label}')).click()`);
  await press('Town');
  await wait(1000);
  for (let i = 0; i < CLOSER; i += 1) {
    await press('Closer');
    await wait(500);
  }
  const middle = { x: MAP.width / 2, y: MAP.height / 2 };
  const mouse = (type, x, y, extra = {}) => page('Input.dispatchMouseEvent', { type, x, y, button: 'left', ...extra });
  await mouse('mouseMoved', middle.x, middle.y, { button: 'none' });
  await mouse('mousePressed', middle.x, middle.y, { clickCount: 1, buttons: 1 });
  await mouse('mouseMoved', middle.x + DRAG.x, middle.y + DRAG.y, { buttons: 1 });
  await mouse('mouseReleased', middle.x + DRAG.x, middle.y + DRAG.y, { clickCount: 1 });
  await wait(3000);
  const shot = await page('Page.captureScreenshot', { format: 'png' });
  return Buffer.from(shot.data, 'base64');
}

async function composeCard(page, scene) {
  const scenePath = join(profile, 'scene.png');
  writeFileSync(scenePath, scene);
  const card = readFileSync(join(here, 'card.html'), 'utf8')
    .replace('{{SCENE}}', pathToFileURL(scenePath).href);
  const cardPath = join(profile, 'card.html');
  writeFileSync(cardPath, card);
  await page('Emulation.setDeviceMetricsOverride', {
    width: WIDTH, height: HEIGHT, deviceScaleFactor: 1, mobile: false,
  });
  await page('Page.navigate', { url: pathToFileURL(cardPath).href });
  await until(page, `document.readyState === 'complete' && document.images[0]?.complete`, 'the card');
  await wait(300);
  const shot = await page('Page.captureScreenshot', { format: 'jpeg', quality: QUALITY });
  return Buffer.from(shot.data, 'base64');
}

try {
  const page = await connect(await devtools());
  const scene = await captureCity(page);
  const card = await composeCard(page, scene);
  writeFileSync(output, card);
  console.log(`web/og.jpg: ${WIDTH}x${HEIGHT}, ${Math.round(card.length / 1024)} KB`);
} finally {
  chrome.kill();
  await wait(1000);
  // The browser can still be letting go of its profile, and the temp folder clears itself in time.
  try { rmSync(profile, { recursive: true, force: true, maxRetries: 3 }); } catch {}
}
