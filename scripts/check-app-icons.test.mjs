// Offline structural tests for the icon container parsers.
//
// They rebuild containers in memory from payloads parsed out of the shipped
// assets, so a malformed case never touches a file on disk. No browser and no
// network: this runs anywhere `node` does.
//
// Limit: these cover structure only. Whether an entry holds the *right*
// drawing is decided by the decoded-pixel comparison in check-app-icons.mjs,
// which needs a renderer; diffRgba is exercised here on synthetic samples.
//
// Usage: node scripts/check-app-icons.test.mjs

import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import {
  ICNS_PNG_SIZES, ICO_REQUIRED_SIZES, diffRgba, missingEntries, parseIcns, parseIco,
} from './icon-containers.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const icoBytes = await readFile(`${root}/desktop/src-tauri/icons/icon.ico`);
const icnsBytes = await readFile(`${root}/desktop/src-tauri/icons/icon.icns`);

// Real payloads, parsed out of the shipped assets and never written back.
const realIco = parseIco(icoBytes);
const realIcns = parseIcns(icnsBytes);
const png = (size) => {
  const entry = realIco.entries.find((e) => e.width === size);
  if (!entry) throw new Error(`fixture setup: no ${size}px payload in the shipped ICO`);
  return Buffer.from(entry.bytes);
};

const buildIco = (items) => {
  const head = Buffer.alloc(6 + items.length * 16);
  head.writeUInt16LE(0, 0);
  head.writeUInt16LE(1, 2);
  head.writeUInt16LE(items.length, 4);
  let offset = head.length;
  items.forEach((item, i) => {
    const o = 6 + i * 16;
    head.writeUInt8(item.width === 256 ? 0 : item.width, o);
    head.writeUInt8(item.height === 256 ? 0 : item.height, o + 1);
    head.writeUInt16LE(1, o + 4);
    head.writeUInt16LE(32, o + 6);
    head.writeUInt32LE(item.length ?? item.bytes.length, o + 8);
    head.writeUInt32LE(item.offset ?? offset, o + 12);
    offset += item.bytes.length;
  });
  return Buffer.concat([head, ...items.map((i) => i.bytes)]);
};

const buildIcns = (blocks) => {
  const parts = blocks.map((b) => {
    const header = Buffer.alloc(8);
    header.write(b.type, 0, 4, 'latin1');
    header.writeUInt32BE(b.length ?? b.body.length + 8, 4);
    return Buffer.concat([header, b.body]);
  });
  const body = Buffer.concat(parts);
  const head = Buffer.alloc(8);
  head.write('icns', 0, 4, 'latin1');
  head.writeUInt32BE(8 + body.length, 4);
  return Buffer.concat([head, body]);
};

const rejects = (fn, fragment, what) => {
  assert.throws(fn, (err) => {
    assert.match(err.message, fragment, `${what}: wrong rejection message`);
    return true;
  }, `${what}: was accepted but must be rejected`);
};

// ── The shipped assets themselves parse, and hold what is required ──
assert.equal(realIco.count, realIco.entries.length);
assert.deepEqual(missingEntries(ICO_REQUIRED_SIZES, realIco.entries.map((e) => `${e.width}x${e.height}`)), []);
assert.deepEqual(missingEntries(Object.keys(ICNS_PNG_SIZES), realIcns.blocks.map((b) => b.type)), []);
for (const [type, size] of Object.entries(ICNS_PNG_SIZES)) {
  const block = realIcns.blocks.find((b) => b.type === type);
  assert.equal(block.png?.width, size, `shipped ICNS ${type} should hold ${size}px`);
  assert.equal(block.png?.height, size, `shipped ICNS ${type} should be square`);
}

// ── ICO: a valid rebuild is accepted, so the negatives below isolate one fault ──
const goodIco = buildIco([{ width: 16, height: 16, bytes: png(16) }, { width: 32, height: 32, bytes: png(32) }]);
assert.equal(parseIco(goodIco).entries.length, 2);

// Malformed headers.
const reserved = Buffer.from(goodIco); reserved.writeUInt16LE(1, 0);
rejects(() => parseIco(reserved), /reserved field is not zero/, 'ICO with a non-zero reserved field');
const wrongType = Buffer.from(goodIco); wrongType.writeUInt16LE(2, 2);
rejects(() => parseIco(wrongType), /type field is not 1/, 'ICO declaring a cursor type');
const noEntries = Buffer.from(goodIco); noEntries.writeUInt16LE(0, 4);
rejects(() => parseIco(noEntries), /declares no entries/, 'ICO declaring zero entries');
const overCount = Buffer.from(goodIco); overCount.writeUInt16LE(400, 4);
rejects(() => parseIco(overCount), /runs past the file/, 'ICO whose directory overruns the file');

// Truncated and out-of-bounds payloads — the case a lenient parser calls success.
rejects(() => parseIco(goodIco.subarray(0, goodIco.length - 40)), /runs past the file/, 'truncated ICO');
rejects(() => parseIco(buildIco([{ width: 16, height: 16, bytes: png(16), length: 99999 }])),
  /runs past the file/, 'ICO entry with an inflated length');
rejects(() => parseIco(buildIco([{ width: 16, height: 16, bytes: png(16), offset: 4 }])),
  /points inside the directory/, 'ICO entry pointing into its own directory');
rejects(() => parseIco(buildIco([{ width: 16, height: 16, bytes: png(16), length: 0 }])),
  /zero length/, 'ICO entry declaring a zero length');

// Duplicate and substituted entries.
rejects(() => parseIco(buildIco([
  { width: 32, height: 32, bytes: png(32) }, { width: 32, height: 32, bytes: png(32) },
])), /declared more than once/, 'ICO declaring 32x32 twice');
rejects(() => parseIco(buildIco([{ width: 32, height: 32, bytes: png(64) }])),
  /declares 32x32 but embeds 64x64/, 'ICO entry holding a substituted image');
rejects(() => parseIco(buildIco([{ width: 16, height: 16, bytes: Buffer.alloc(64) }])),
  /not a PNG payload/, 'ICO entry holding a non-PNG payload');

// A required size simply absent.
assert.deepEqual(missingEntries(ICO_REQUIRED_SIZES, ['16x16', '32x32']), ['24x24', '48x48', '64x64', '256x256']);

// ── ICNS ──
const goodIcns = buildIcns([{ type: 'ic11', body: png(32) }, { type: 'ic12', body: png(64) }]);
assert.equal(parseIcns(goodIcns).blocks.length, 2);

const badMagic = Buffer.from(goodIcns); badMagic.write('icnt', 0, 4, 'latin1');
rejects(() => parseIcns(badMagic), /missing the icns magic/, 'ICNS without its magic');

// A declared length that disagrees with the file: the truncation case.
rejects(() => parseIcns(goodIcns.subarray(0, goodIcns.length - 20)),
  /header declares \d+ bytes, file holds \d+/, 'truncated ICNS');
const inflated = Buffer.from(goodIcns); inflated.writeUInt32BE(goodIcns.length + 10, 4);
rejects(() => parseIcns(inflated), /header declares/, 'ICNS whose header overstates its length');

// Block lengths that a break-on-error traversal would swallow.
rejects(() => parseIcns(buildIcns([{ type: 'ic11', body: png(32), length: 4 }])),
  /impossible length/, 'ICNS block shorter than its own header');
rejects(() => parseIcns(buildIcns([{ type: 'ic11', body: png(32), length: 999999 }])),
  /runs past the file/, 'ICNS block overrunning the file');
const trailing = Buffer.concat([goodIcns, Buffer.alloc(3)]);
trailing.writeUInt32BE(trailing.length, 4);
rejects(() => parseIcns(trailing), /block header at \d+ runs past the file/, 'ICNS with a trailing fragment');

rejects(() => parseIcns(buildIcns([{ type: 'ic11', body: png(32) }, { type: 'ic11', body: png(32) }])),
  /appears more than once/, 'ICNS declaring ic11 twice');

// A substituted image: structurally sound, wrong size for its type.
const swapped = parseIcns(buildIcns([{ type: 'ic07', body: png(256) }]));
assert.equal(swapped.blocks[0].png.width, 256);
assert.notEqual(swapped.blocks[0].png.width, ICNS_PNG_SIZES.ic07,
  'ic07 holding a 256px image must not match its expected size');

assert.deepEqual(missingEntries(Object.keys(ICNS_PNG_SIZES), ['ic07', 'ic08']),
  ['ic11', 'ic12', 'ic13', 'ic14', 'ic09', 'ic10']);

// ── Pixel comparison ──
const base = new Uint8ClampedArray([0, 0, 0, 255, 10, 20, 30, 255]);
assert.equal(diffRgba(base, base.slice()), 0);
const oneOff = base.slice(); oneOff[5] = 21;
assert.equal(diffRgba(base, oneOff), 1);
const alphaOnly = base.slice(); alphaOnly[3] = 254;
assert.equal(diffRgba(base, alphaOnly), 1, 'a difference in alpha alone must count');
rejects(() => diffRgba(base, base.slice(0, 4)), /8 samples vs 4/, 'comparing different sample counts');

process.stdout.write('icon container parsers: all structural negative cases rejected\n');
