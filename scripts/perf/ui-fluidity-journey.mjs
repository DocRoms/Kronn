#!/usr/bin/env node
// Read-only browser journey over Automations, Plugins and a heavy discussion,
// measuring what a user feels: input-to-paint (Event Timing), long tasks, DOM
// size and the API traffic of each step. Used for the 0.14.1 fluidity audit.
//
// Every non-GET API call is answered locally with a 503, so the journey never
// writes to the instance it measures. Full discussion-detail fetches are traced
// with their JS stack, and WebSocket frames are counted by type, to attribute
// reloads to the event that triggered them.
//
// Usage: node scripts/perf/ui-fluidity-journey.mjs [--base URL] [--room ID] [--out FILE]
// Requires the frontend dev dependencies (Playwright's bundled Chromium).

import { execFileSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const arg = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at > 0 ? process.argv[at + 1] : fallback;
};
const BASE = arg('base', 'http://localhost:5173/');
const ROOM = arg('room', null);
const OUT = arg('out', null);

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
// A git worktree has no node_modules of its own: also look in the main checkout.
function mainCheckout() {
  try {
    const common = execFileSync('git', ['rev-parse', '--path-format=absolute', '--git-common-dir'],
      { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
    return common ? dirname(common) : null;
  } catch { return null; }
}
let chromium;
for (const base of [root, mainCheckout()].filter(Boolean)) {
  try { ({ chromium } = await import(`${base}/frontend/node_modules/@playwright/test/index.mjs`)); break; }
  catch { /* try the next checkout */ }
}
if (!chromium) {
  console.error('Playwright is not installed: run `pnpm install` in frontend/.');
  process.exit(2);
}

const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const requests = [];
page.on('requestfinished', async (request) => {
  const url = new URL(request.url());
  if (!url.pathname.startsWith('/api/')) return;
  let bytes = 0;
  try { bytes = (await request.sizes()).responseBodySize; } catch { /* aborted */ }
  requests.push({
    path: url.pathname.replace(/[0-9a-f-]{36}/g, ':id'),
    ms: Math.round(request.timing().responseEnd),
    kb: bytes / 1024,
  });
});

await page.addInitScript(() => {
  localStorage.setItem('kronn:tour-completed', 'true');
  window.__longTasks = [];
  window.__events = [];
  window.__ws = [];
  window.__fullDetail = [];
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) window.__longTasks.push(entry.duration);
  }).observe({ type: 'longtask', buffered: true });
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) window.__events.push({ name: entry.name, ms: entry.duration });
  }).observe({ type: 'event', durationThreshold: 16, buffered: true });
  const NativeWebSocket = window.WebSocket;
  window.WebSocket = class extends NativeWebSocket {
    constructor(...args) {
      super(...args);
      this.addEventListener('message', (event) => {
        try { window.__ws.push(JSON.parse(event.data).type); } catch { /* not JSON */ }
      });
    }
  };
  const nativeFetch = window.fetch.bind(window);
  window.fetch = (input, init) => {
    const method = (init?.method ?? (input instanceof Request ? input.method : 'GET')).toUpperCase();
    const url = typeof input === 'string' ? input : input instanceof Request ? input.url : String(input);
    if (url.includes('/api/') && !['GET', 'HEAD', 'OPTIONS'].includes(method)) {
      return Promise.resolve(new Response('{"success":false}', { status: 503 }));
    }
    if (/\/api\/discussions\/[0-9a-f-]{36}(\?|$)/.test(url)) {
      const frames = (new Error().stack ?? '').split('\n').slice(2)
        .map((line) => line.trim().replace(/^at /, '').replace(/https?:\/\/[^/]+\//, '').replace(/\?[^:)]*/, ''))
        .filter((line) => !line.includes('node_modules') && !line.includes('lib/api.ts'));
      window.__fullDetail.push(frames.slice(0, 3).join(' <- '));
    }
    return nativeFetch(input, init);
  };
});

const snapshot = () => page.evaluate(() => {
  const tasks = window.__longTasks;
  const worst = window.__events.sort((a, b) => b.ms - a.ms)[0];
  window.__longTasks = [];
  window.__events = [];
  return {
    longTasks: tasks.length,
    longTaskMs: Math.round(tasks.reduce((sum, ms) => sum + ms, 0)),
    worstTaskMs: Math.round(Math.max(0, ...tasks)),
    worstInput: worst ? `${worst.name} ${Math.round(worst.ms)}ms` : '-',
    dom: document.getElementsByTagName('*').length,
  };
});
const settle = async () => {
  await page.waitForLoadState('networkidle', { timeout: 30000 }).catch(() => {});
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
};

const steps = [];
async function step(label, action) {
  const first = requests.length;
  const started = Date.now();
  try {
    await action();
  } catch (error) {
    steps.push({ label, error: String(error).slice(0, 160) });
    return;
  }
  await settle();
  const ms = Date.now() - started;
  const traffic = requests.slice(first);
  const byPath = {};
  for (const request of traffic) {
    const entry = (byPath[request.path] ??= { count: 0, kb: 0 });
    entry.count += 1;
    entry.kb += request.kb;
  }
  const heaviest = Object.entries(byPath).sort((a, b) => b[1].kb - a[1].kb).slice(0, 3)
    .map(([path, entry]) => `${entry.count}× ${path} ${Math.round(entry.kb)}KB`);
  steps.push({ label, ms, ...(await snapshot()), requests: traffic.length,
    kb: Math.round(traffic.reduce((sum, request) => sum + request.kb, 0)), heaviest });
}

const nav = (label) => page.locator('.dash-nav-btn', { hasText: label }).first().click();
const scroll = async () => {
  await page.mouse.move(700, 500);
  await page.mouse.wheel(0, 3000);
  await page.waitForTimeout(600);
};

await step('boot', () => page.goto(BASE));
for (const pass of ['cold', 'warm']) {
  await step(`${pass} · Automations`, () => nav('Automatisation'));
  await step(`${pass} · Automations scroll`, scroll);
  await step(`${pass} · Automations idle 10 s`, () => page.waitForTimeout(10000));
  await step(`${pass} · Plugins`, () => nav('Plugins'));
  await step(`${pass} · Plugins scroll`, scroll);
  await step(`${pass} · Plugins search`, async () => {
    await page.locator('input[type="search"], input[placeholder*="echerch"], input[placeholder*="earch"]').first()
      .click({ timeout: 5000 });
    await page.keyboard.type('git', { delay: 60 });
  });
  await step(`${pass} · Plugins idle 10 s`, () => page.waitForTimeout(10000));
  await step(`${pass} · Discussions`, () => nav('Discussions'));
}
if (ROOM) {
  await step('room · open', async () => {
    await page.evaluate((id) => { location.hash = `#discussion-${id}`; }, ROOM);
    await page.reload();
    await page.locator('.disc-msg-bubble').last().waitFor({ timeout: 90000 });
  });
  await step('room · idle 20 s', () => page.waitForTimeout(20000));
}

const fullDetail = await page.evaluate(() => window.__fullDetail);
const wsTypes = await page.evaluate(() => window.__ws);
await browser.close();

const count = (items) => items.reduce((acc, item) => ({ ...acc, [item]: (acc[item] ?? 0) + 1 }), {});
const result = { base: BASE, room: ROOM, steps, fullDetailFetches: count(fullDetail), wsFrames: count(wsTypes) };
if (OUT) writeFileSync(OUT, JSON.stringify(result, null, 1));
for (const s of steps) {
  console.log(s.error
    ? `${s.label}: ERROR ${s.error}`
    : `${s.label.padEnd(28)} ${String(s.ms).padStart(6)} ms | long tasks ${s.longTasks} (${s.longTaskMs} ms, max ${s.worstTaskMs}) | worst input ${s.worstInput} | ${s.requests} req ${s.kb} KB | DOM ${s.dom} | ${s.heaviest.join(', ')}`);
}
console.log('full discussion-detail fetches by caller:', JSON.stringify(result.fullDetailFetches));
console.log('WebSocket frames by type:', JSON.stringify(result.wsFrames));
