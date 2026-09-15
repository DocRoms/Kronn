#!/usr/bin/env node
// Application icons must stay derivable from their own canonical SVG.
//
// Each family keeps a distinct source on purpose: the desktop icon carries a
// dark plate, because it lands on a dock or a wallpaper, while the web icon is
// transparent because the browser chrome is already a ground. Rendering one
// from the other silently drops or adds that plate, which is how the shipped
// desktop icons drifted from the mark in the first place.
//
// Shipped PNGs are compared byte for byte against a fresh render, so a
// hand-edit or a resample fails and not only a visibly wrong icon. Images
// embedded in the ICO and ICNS containers are written by the packaging tools,
// so byte identity does not hold there; they are compared on decoded pixels.
//
// Usage: node scripts/check-app-icons.mjs [--verbose]
// Requires the frontend dev dependencies (Playwright's bundled Chromium).
// Without them nothing is validated and the run exits 2.

import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import {
  ICNS_PNG_SIZES, ICO_REQUIRED_SIZES, diffRgba, missingEntries, parseIcns, parseIco,
} from './icon-containers.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const verbose = process.argv.includes('--verbose');
const sha = (b) => createHash('sha256').update(b).digest('hex');
const say = (m) => verbose && process.stdout.write(`${m}\n`);

const failures = [];
const fail = (m) => { failures.push(m); process.stdout.write(`FAIL ${m}\n`); };

// Playwright lives in the frontend dev dependencies. A git worktree has no
// node_modules of its own, so also look in the main checkout — located through
// Git metadata rather than a hard-coded path, so this runs on any machine.
function mainCheckout() {
  try {
    const common = execFileSync('git', ['rev-parse', '--path-format=absolute', '--git-common-dir'],
      { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
    return common ? dirname(common) : null;
  } catch { return null; }
}

let chromium;
const searched = [root, mainCheckout()].filter(Boolean);
for (const base of searched) {
  try { ({ chromium } = await import(`${base}/frontend/node_modules/@playwright/test/index.mjs`)); break; }
  catch { /* try the next checkout */ }
}
if (!chromium) {
  process.stdout.write(
    'NOT VALIDATED: Playwright is not installed, so no icon was checked.\n'
    + `Searched: ${searched.map((b) => `${b}/frontend/node_modules`).join(', ')}\n`
    + 'Run `pnpm install` in frontend/ and retry.\n');
  process.exit(2);
}

const DESKTOP_SVG = 'desktop/src-tauri/icons/icon.svg';
const WEB_SVG = 'frontend/public/favicon.svg';

// name -> [source family, pixel size]
const PNGS = {
  'desktop/src-tauri/icons/32x32.png': ['desktop', 32],
  'desktop/src-tauri/icons/64x64.png': ['desktop', 64],
  'desktop/src-tauri/icons/128x128.png': ['desktop', 128],
  'desktop/src-tauri/icons/128x128@2x.png': ['desktop', 256],
  'desktop/src-tauri/icons/icon.png': ['desktop', 512],
  'desktop/src-tauri/icons/Square30x30Logo.png': ['desktop', 30],
  'desktop/src-tauri/icons/Square44x44Logo.png': ['desktop', 44],
  'desktop/src-tauri/icons/StoreLogo.png': ['desktop', 50],
  'desktop/src-tauri/icons/Square71x71Logo.png': ['desktop', 71],
  'desktop/src-tauri/icons/Square89x89Logo.png': ['desktop', 89],
  'desktop/src-tauri/icons/Square107x107Logo.png': ['desktop', 107],
  'desktop/src-tauri/icons/Square142x142Logo.png': ['desktop', 142],
  'desktop/src-tauri/icons/Square150x150Logo.png': ['desktop', 150],
  'desktop/src-tauri/icons/Square284x284Logo.png': ['desktop', 284],
  'desktop/src-tauri/icons/Square310x310Logo.png': ['desktop', 310],
  'frontend/public/apple-touch-icon.png': ['web', 180],
  'site/apple-touch-icon.png': ['web', 180],
};

const desktopSvg = await readFile(`${root}/${DESKTOP_SVG}`);
const webSvg = await readFile(`${root}/${WEB_SVG}`);

// The web sources must stay one drawing in two places.
if (sha(webSvg) !== sha(await readFile(`${root}/site/favicon.svg`))) {
  fail('frontend/public/favicon.svg and site/favicon.svg have diverged');
}

const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 64, height: 64 }, deviceScaleFactor: 1 });
  const external = [];
  await page.route('**/*', (r) => { external.push(r.request().url()); return r.abort(); });

  const renders = new Map();
  const render = async (family, size) => {
    const key = `${family}:${size}`;
    if (!renders.has(key)) {
      const svg = family === 'desktop' ? desktopSvg : webSvg;
      await page.setViewportSize({ width: size, height: size });
      await page.setContent(
        `<style>html,body{margin:0;width:${size}px;height:${size}px;background:transparent}`
        + `img{display:block;width:${size}px;height:${size}px}</style><img alt="Kronn">`);
      await page.locator('img').evaluate((img, uri) => { img.src = uri; return img.decode(); },
        `data:image/svg+xml;base64,${svg.toString('base64')}`);
      // Captured twice: a renderer that is not deterministic at this size would
      // make every byte comparison below meaningless.
      const first = await page.screenshot({ omitBackground: true });
      const second = await page.screenshot({ omitBackground: true });
      if (sha(first) !== sha(second)) fail(`${family} render at ${size}px is not reproducible`);
      renders.set(key, first);
    }
    return renders.get(key);
  };

  // Decoding happens in the same engine that produced the render, so both sides
  // of a pixel comparison go through one decoder.
  const rgba = (buf, size) => page.evaluate(async ([b64, s]) => {
    const img = new Image();
    img.src = `data:image/png;base64,${b64}`;
    await img.decode();
    if (img.naturalWidth !== s || img.naturalHeight !== s) return null;
    const canvas = document.createElement('canvas');
    canvas.width = s; canvas.height = s;
    const ctx = canvas.getContext('2d', { willReadFrequently: true });
    ctx.clearRect(0, 0, s, s);
    ctx.drawImage(img, 0, 0);
    return Array.from(ctx.getImageData(0, 0, s, s).data);
  }, [Buffer.from(buf).toString('base64'), size]);

  const sameArtwork = async (buf, family, size, what) => {
    const embedded = await rgba(buf, size);
    if (embedded === null) { fail(`${what}: does not decode as ${size}x${size}`); return; }
    const fresh = await rgba(await render(family, size), size);
    const differing = diffRgba(fresh, embedded);
    if (differing) fail(`${what}: ${differing} pixel(s) differ from a fresh ${size}px render`);
    else say(`ok   ${what} (${size}px artwork matches)`);
  };

  // ── Each family's ground, measured rather than taken from its comment ──
  // The desktop source opens with a full-bleed <rect> so its render is wholly
  // opaque; the web source has no ground at all, so most of its frame is clear.
  const alpha = async (family, size) => {
    const pixels = await rgba(await render(family, size), size);
    let opaque = 0; let clear = 0;
    for (let i = 3; i < pixels.length; i += 4) {
      if (pixels[i] === 255) opaque += 1; else if (pixels[i] === 0) clear += 1;
    }
    return { opaque, clear, total: pixels.length / 4 };
  };
  const desktopAlpha = await alpha('desktop', 512);
  if (desktopAlpha.opaque !== desktopAlpha.total) {
    fail(`${DESKTOP_SVG}: expected a fully opaque plate, ${desktopAlpha.total - desktopAlpha.opaque} pixel(s) are not`);
  } else { say(`ok   ${DESKTOP_SVG} renders a fully opaque plate (${desktopAlpha.total} px)`); }
  const webAlpha = await alpha('web', 180);
  if (webAlpha.clear === 0) fail(`${WEB_SVG}: expected a transparent ground, every pixel is painted`);
  else say(`ok   ${WEB_SVG} keeps a transparent ground (${webAlpha.clear} clear px)`);

  // ── Shipped PNGs: byte identity against a fresh render ──
  for (const [rel, [family, size]] of Object.entries(PNGS)) {
    const rendered = await render(family, size);
    const shipped = await readFile(`${root}/${rel}`);
    if (shipped.readUInt32BE(16) !== size || shipped.readUInt32BE(20) !== size) {
      fail(`${rel}: shipped file is ${shipped.readUInt32BE(16)}x${shipped.readUInt32BE(20)}, expected ${size}x${size}`);
    } else if (sha(rendered) !== sha(shipped)) {
      fail(`${rel}: shipped bytes do not match a fresh render of ${family === 'desktop' ? DESKTOP_SVG : WEB_SVG}`);
    } else {
      say(`ok   ${rel} (${size}x${size})`);
    }
  }

  // ── ICO ──
  try {
    const ico = parseIco(await readFile(`${root}/desktop/src-tauri/icons/icon.ico`));
    const sizes = ico.entries.map((e) => `${e.width}x${e.height}`);
    for (const absent of missingEntries(ICO_REQUIRED_SIZES, sizes)) {
      fail(`icon.ico is missing its ${absent} entry`);
    }
    for (const entry of ico.entries) {
      await sameArtwork(entry.bytes, 'desktop', entry.width, `icon.ico ${entry.width}x${entry.height}`);
    }
    say(`ok   icon.ico (${sizes.join(', ')})`);
  } catch (err) { fail(`icon.ico is malformed: ${err.message}`); }

  // ── ICNS ──
  try {
    const icns = parseIcns(await readFile(`${root}/desktop/src-tauri/icons/icon.icns`));
    for (const absent of missingEntries(Object.keys(ICNS_PNG_SIZES), icns.blocks.map((b) => b.type))) {
      fail(`icon.icns is missing its ${absent} entry`);
    }
    for (const block of icns.blocks) {
      const expected = ICNS_PNG_SIZES[block.type];
      if (expected === undefined) { say(`ok   icon.icns '${block.type}' (not asserted)`); continue; }
      if (!block.png) { fail(`icon.icns '${block.type}' is not a PNG entry`); continue; }
      if (block.png.width !== expected || block.png.height !== expected) {
        fail(`icon.icns '${block.type}' holds ${block.png.width}x${block.png.height}, expected ${expected}x${expected}`);
        continue;
      }
      await sameArtwork(block.body, 'desktop', expected, `icon.icns '${block.type}'`);
    }
    say(`ok   icon.icns (${icns.blocks.map((b) => b.type).join(', ')})`);
  } catch (err) { fail(`icon.icns is malformed: ${err.message}`); }

  if (external.length) fail(`capture reached the network: ${external.length} request(s)`);

  // The run must not have modified what it read.
  if (sha(await readFile(`${root}/${DESKTOP_SVG}`)) !== sha(desktopSvg)
    || sha(await readFile(`${root}/${WEB_SVG}`)) !== sha(webSvg)) {
    fail('a source SVG changed while the check was running');
  }
} finally { await browser.close(); }

// ── References must keep pointing at these files ──
const tauri = JSON.parse(await readFile(`${root}/desktop/src-tauri/tauri.conf.json`, 'utf8'));
const declared = [...tauri.bundle.icon];
// The tray reads its own path, outside the bundle list.
const trayPath = tauri.app?.trayIcon?.iconPath;
if (!trayPath) fail('tauri.conf.json declares no tray icon path');
else declared.push(trayPath);
for (const rel of declared) {
  try { await readFile(`${root}/desktop/src-tauri/${rel}`); say(`ok   tauri.conf.json -> ${rel}`); }
  catch { fail(`tauri.conf.json declares ${rel}, which does not exist`); }
}
if (trayPath && !Object.keys(PNGS).includes(`desktop/src-tauri/${trayPath}`)) {
  fail(`the tray icon ${trayPath} is not among the icons this check regenerates`);
}

for (const [page, pattern] of [
  ['frontend/index.html', /rel="apple-touch-icon"\s+href="\/apple-touch-icon\.png"/],
  ['site/index.html', /rel="apple-touch-icon"\s+href="apple-touch-icon\.png"/],
  ['site/en.html', /rel="apple-touch-icon"\s+href="apple-touch-icon\.png"/],
  ['site/es.html', /rel="apple-touch-icon"\s+href="apple-touch-icon\.png"/],
]) {
  const html = await readFile(`${root}/${page}`, 'utf8');
  if (pattern.test(html)) say(`ok   ${page} references its icon`);
  else fail(`${page} no longer references its apple-touch icon as expected`);
}

if (failures.length) {
  process.stdout.write(`\n${failures.length} check(s) failed\n`);
  process.exit(1);
}
process.stdout.write('17 PNG files and both containers match a fresh render of their canonical SVG\n');
