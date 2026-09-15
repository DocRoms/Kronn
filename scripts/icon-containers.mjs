// Structural parsers for the icon containers we ship.
//
// They are pure and throw on anything malformed, so the validator and its
// negative tests share one definition of "valid". A parser that recovers from a
// bad length by stopping early would report success on a truncated container,
// which is the failure these were written to prevent.

const PNG_MAGIC = '89504e470d0a1a0a';

// A PNG header is fixed-layout: magic, then the IHDR chunk whose width and
// height are the only dimensions a consumer can trust.
export function readPngSize(buf, what) {
  if (buf.length < 24) throw new Error(`${what}: too short to hold a PNG header`);
  if (buf.subarray(0, 8).toString('hex') !== PNG_MAGIC) throw new Error(`${what}: not a PNG payload`);
  if (buf.subarray(12, 16).toString('latin1') !== 'IHDR') throw new Error(`${what}: first chunk is not IHDR`);
  const width = buf.readUInt32BE(16);
  const height = buf.readUInt32BE(20);
  if (width === 0 || height === 0) throw new Error(`${what}: declares a zero dimension`);
  return { width, height };
}

export function parseIco(buf) {
  if (buf.length < 6) throw new Error('ico: shorter than its directory header');
  if (buf.readUInt16LE(0) !== 0) throw new Error('ico: reserved field is not zero');
  if (buf.readUInt16LE(2) !== 1) throw new Error('ico: type field is not 1 (icon)');
  const count = buf.readUInt16LE(4);
  if (count === 0) throw new Error('ico: declares no entries');
  const directoryEnd = 6 + count * 16;
  if (directoryEnd > buf.length) throw new Error(`ico: directory of ${count} entries runs past the file`);

  const entries = [];
  const seen = new Set();
  for (let i = 0; i < count; i += 1) {
    const o = 6 + i * 16;
    // 0 in the directory means 256: the field is a single byte.
    const width = buf.readUInt8(o) || 256;
    const height = buf.readUInt8(o + 1) || 256;
    const length = buf.readUInt32LE(o + 8);
    const offset = buf.readUInt32LE(o + 12);
    if (length === 0) throw new Error(`ico: entry ${i} (${width}x${height}) declares a zero length`);
    if (offset < directoryEnd) throw new Error(`ico: entry ${i} (${width}x${height}) points inside the directory`);
    if (offset + length > buf.length) throw new Error(`ico: entry ${i} (${width}x${height}) runs past the file`);

    const key = `${width}x${height}`;
    if (seen.has(key)) throw new Error(`ico: ${key} is declared more than once`);
    seen.add(key);

    const bytes = buf.subarray(offset, offset + length);
    const png = readPngSize(bytes, `ico entry ${i} (${key})`);
    if (png.width !== width || png.height !== height) {
      throw new Error(`ico: entry ${i} declares ${key} but embeds ${png.width}x${png.height}`);
    }
    entries.push({ index: i, width, height, offset, length, bytes });
  }
  return { count, entries };
}

export function parseIcns(buf) {
  if (buf.length < 8) throw new Error('icns: shorter than its header');
  if (buf.subarray(0, 4).toString('latin1') !== 'icns') throw new Error('icns: missing the icns magic');
  const declaredLength = buf.readUInt32BE(4);
  if (declaredLength !== buf.length) {
    throw new Error(`icns: header declares ${declaredLength} bytes, file holds ${buf.length}`);
  }

  const blocks = [];
  const seen = new Set();
  let offset = 8;
  while (offset < buf.length) {
    if (offset + 8 > buf.length) throw new Error(`icns: block header at ${offset} runs past the file`);
    const type = buf.subarray(offset, offset + 4).toString('latin1');
    const length = buf.readUInt32BE(offset + 4);
    if (length < 8) throw new Error(`icns: block '${type}' declares an impossible length of ${length}`);
    if (offset + length > buf.length) throw new Error(`icns: block '${type}' runs past the file`);
    if (seen.has(type)) throw new Error(`icns: block '${type}' appears more than once`);
    seen.add(type);

    const body = buf.subarray(offset + 8, offset + length);
    const isPng = body.length >= 8 && body.subarray(0, 8).toString('hex') === PNG_MAGIC;
    blocks.push({ type, length, offset, body, png: isPng ? readPngSize(body, `icns block '${type}'`) : null });
    offset += length;
  }
  // Exact traversal: the blocks must tile the file with nothing left over.
  if (offset !== buf.length) throw new Error(`icns: block traversal ended at ${offset}, expected ${buf.length}`);
  return { declaredLength, blocks };
}

// Which of the required keys are absent from what a container actually holds.
export function missingEntries(required, actual) {
  const have = new Set(actual);
  return required.filter((key) => !have.has(key));
}

// Structural parsing cannot tell a correct icon from a stale one of the same
// size, so artwork is compared on decoded pixels. Returns how many differ.
export function diffRgba(a, b) {
  if (a.length !== b.length) throw new Error(`rgba: ${a.length} samples vs ${b.length}`);
  let differing = 0;
  for (let i = 0; i < a.length; i += 4) {
    if (a[i] !== b[i] || a[i + 1] !== b[i + 1] || a[i + 2] !== b[i + 2] || a[i + 3] !== b[i + 3]) {
      differing += 1;
    }
  }
  return differing;
}

// What each container must hold for the packagers that read it.
// ICO: the sizes Windows picks between in Explorer, the taskbar and the shell.
export const ICO_REQUIRED_SIZES = ['16x16', '24x24', '32x32', '48x48', '64x64', '256x256'];
// ICNS: the modern PNG-encoded types, mapped to the pixel size each must hold.
// The container also keeps non-PNG entries (ic04, ic05) and an `info` block;
// those are allowed through unchecked rather than asserted.
export const ICNS_PNG_SIZES = {
  ic11: 32, ic12: 64, ic07: 128, ic13: 256, ic08: 256, ic14: 512, ic09: 512, ic10: 1024,
};
