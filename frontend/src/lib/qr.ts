// KT-1492 — a small QR Code encoder (ISO/IEC 18004, byte mode, error correction
// L or M, versions 1–10, the 8 masks), so the authenticator enrolment shows a
// scannable code without a dependency. It follows the construction of Project
// Nayuki's reference generator (MIT). Versions 1–10 hold up to 271 bytes at
// level L: an `otpauth://` URI is about 100.

export type QrLevel = 'L' | 'M';

/** Error-correction codewords per block, by version 1–10 (index 0 unused). */
const ECC_PER_BLOCK: Record<QrLevel, number[]> = {
  L: [-1, 7, 10, 15, 20, 26, 18, 20, 24, 30, 18],
  M: [-1, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26],
};
/** Number of error-correction blocks, by version 1–10 (index 0 unused). */
const BLOCKS: Record<QrLevel, number[]> = {
  L: [-1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4],
  M: [-1, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5],
};
/** The two format bits of each level. */
const FORMAT_BITS: Record<QrLevel, number> = { L: 1, M: 0 };
const MAX_VERSION = 10;

const bit = (value: number, index: number): boolean => ((value >>> index) & 1) !== 0;

const sizeOf = (version: number) => version * 4 + 17;

/** Data modules of a version once the function patterns are taken out. */
function rawDataModules(version: number): number {
  let result = (16 * version + 128) * version + 64;
  if (version >= 2) {
    const align = Math.floor(version / 7) + 2;
    result -= (25 * align - 10) * align - 55;
    if (version >= 7) result -= 36;
  }
  return result;
}

const dataCodewords = (version: number, level: QrLevel): number =>
  Math.floor(rawDataModules(version) / 8) - ECC_PER_BLOCK[level][version] * BLOCKS[level][version];

function alignmentPositions(version: number): number[] {
  if (version === 1) return [];
  const count = Math.floor(version / 7) + 2;
  const step = Math.ceil((version * 4 + 4) / (count * 2 - 2)) * 2;
  const positions = [6];
  for (let position = sizeOf(version) - 7; positions.length < count; position -= step) {
    positions.splice(1, 0, position);
  }
  return positions;
}

// ── Reed–Solomon over GF(2^8 / 0x11D) ───────────────────────────────────────

function multiply(x: number, y: number): number {
  let z = 0;
  for (let i = 7; i >= 0; i--) {
    z = (z << 1) ^ ((z >>> 7) * 0x11d);
    z ^= ((y >>> i) & 1) * x;
  }
  return z;
}

export function divisor(degree: number): number[] {
  const result: number[] = new Array(degree).fill(0);
  result[degree - 1] = 1;
  let root = 1;
  for (let i = 0; i < degree; i++) {
    for (let j = 0; j < degree; j++) {
      result[j] = multiply(result[j], root);
      if (j + 1 < degree) result[j] ^= result[j + 1];
    }
    root = multiply(root, 0x02);
  }
  return result;
}

/** The error-correction codewords of `data`. */
export function remainder(data: number[], div: number[]): number[] {
  const result: number[] = new Array(div.length).fill(0);
  for (const byte of data) {
    const factor = byte ^ (result.shift() as number);
    result.push(0);
    div.forEach((coefficient, i) => { result[i] ^= multiply(coefficient, factor); });
  }
  return result;
}

// ── Bit strings ─────────────────────────────────────────────────────────────

/** The 15 format bits (level, mask), BCH-protected and masked with 0x5412. */
export function formatBits(level: QrLevel, mask: number): number {
  const data = (FORMAT_BITS[level] << 3) | mask;
  let rem = data;
  for (let i = 0; i < 10; i++) rem = (rem << 1) ^ ((rem >>> 9) * 0x537);
  return ((data << 10) | rem) ^ 0x5412;
}

/** The 18 version bits of a version 7 or more, BCH-protected. */
export function versionBits(version: number): number {
  let rem = version;
  for (let i = 0; i < 12; i++) rem = (rem << 1) ^ ((rem >>> 11) * 0x1f25);
  return (version << 12) | rem;
}

function utf8(text: string): number[] {
  return Array.from(new TextEncoder().encode(text));
}

/** The data codewords: byte-mode segment, terminator, padding. */
function encodeData(bytes: number[], version: number, level: QrLevel): number[] {
  const bits: number[] = [];
  const push = (value: number, length: number) => {
    for (let i = length - 1; i >= 0; i--) bits.push((value >>> i) & 1);
  };
  push(0b0100, 4);
  push(bytes.length, version <= 9 ? 8 : 16);
  bytes.forEach(byte => push(byte, 8));
  const capacity = dataCodewords(version, level) * 8;
  push(0, Math.min(4, capacity - bits.length));
  push(0, (8 - (bits.length % 8)) % 8);
  for (let pad = 0xec; bits.length < capacity; pad ^= 0xec ^ 0x11) push(pad, 8);
  const codewords: number[] = [];
  for (let i = 0; i < bits.length; i += 8) {
    codewords.push(bits.slice(i, i + 8).reduce((acc, b) => (acc << 1) | b, 0));
  }
  return codewords;
}

/** Split into blocks, append each block's error correction, interleave. */
function interleave(data: number[], version: number, level: QrLevel): number[] {
  const blocks = BLOCKS[level][version];
  const eccLength = ECC_PER_BLOCK[level][version];
  const raw = Math.floor(rawDataModules(version) / 8);
  const shortBlocks = blocks - (raw % blocks);
  const shortLength = Math.floor(raw / blocks);
  const div = divisor(eccLength);
  const all: number[][] = [];
  for (let i = 0, k = 0; i < blocks; i++) {
    const length = shortLength - eccLength + (i < shortBlocks ? 0 : 1);
    const chunk = data.slice(k, k + length);
    k += length;
    const ecc = remainder(chunk, div);
    if (i < shortBlocks) chunk.push(0);
    all.push(chunk.concat(ecc));
  }
  const result: number[] = [];
  for (let i = 0; i < all[0].length; i++) {
    all.forEach((block, j) => {
      if (i !== shortLength - eccLength || j >= shortBlocks) result.push(block[i]);
    });
  }
  return result;
}

// ── The matrix ──────────────────────────────────────────────────────────────

const MASKS: Array<(x: number, y: number) => boolean> = [
  (x, y) => (x + y) % 2 === 0,
  (_x, y) => y % 2 === 0,
  (x) => x % 3 === 0,
  (x, y) => (x + y) % 3 === 0,
  (x, y) => (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0,
  (x, y) => ((x * y) % 2) + ((x * y) % 3) === 0,
  (x, y) => (((x * y) % 2) + ((x * y) % 3)) % 2 === 0,
  (x, y) => (((x + y) % 2) + ((x * y) % 3)) % 2 === 0,
];

class Grid {
  readonly size: number;
  readonly modules: boolean[][];
  readonly isFunction: boolean[][];

  constructor(readonly version: number, readonly level: QrLevel) {
    this.size = sizeOf(version);
    this.modules = Array.from({ length: this.size }, () => new Array<boolean>(this.size).fill(false));
    this.isFunction = Array.from({ length: this.size }, () => new Array<boolean>(this.size).fill(false));
    this.drawFunctionPatterns();
  }

  private set(x: number, y: number, dark: boolean) {
    this.modules[y][x] = dark;
    this.isFunction[y][x] = true;
  }

  private finder(cx: number, cy: number) {
    for (let dy = -4; dy <= 4; dy++) {
      for (let dx = -4; dx <= 4; dx++) {
        const distance = Math.max(Math.abs(dx), Math.abs(dy));
        const x = cx + dx;
        const y = cy + dy;
        if (x >= 0 && x < this.size && y >= 0 && y < this.size) this.set(x, y, distance !== 2 && distance !== 4);
      }
    }
  }

  private alignment(cx: number, cy: number) {
    for (let dy = -2; dy <= 2; dy++) {
      for (let dx = -2; dx <= 2; dx++) this.set(cx + dx, cy + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
    }
  }

  drawFormat(mask: number) {
    const bits = formatBits(this.level, mask);
    for (let i = 0; i <= 5; i++) this.set(8, i, bit(bits, i));
    this.set(8, 7, bit(bits, 6));
    this.set(8, 8, bit(bits, 7));
    this.set(7, 8, bit(bits, 8));
    for (let i = 9; i < 15; i++) this.set(14 - i, 8, bit(bits, i));
    for (let i = 0; i < 8; i++) this.set(this.size - 1 - i, 8, bit(bits, i));
    for (let i = 8; i < 15; i++) this.set(8, this.size - 15 + i, bit(bits, i));
    this.set(8, this.size - 8, true);
  }

  private drawFunctionPatterns() {
    for (let i = 0; i < this.size; i++) {
      this.set(6, i, i % 2 === 0);
      this.set(i, 6, i % 2 === 0);
    }
    this.finder(3, 3);
    this.finder(this.size - 4, 3);
    this.finder(3, this.size - 4);
    const positions = alignmentPositions(this.version);
    const last = positions.length - 1;
    positions.forEach((x, i) => positions.forEach((y, j) => {
      const overlapsFinder = (i === 0 && j === 0) || (i === 0 && j === last) || (i === last && j === 0);
      if (!overlapsFinder) this.alignment(x, y);
    }));
    this.drawFormat(0);
    if (this.version >= 7) {
      const bits = versionBits(this.version);
      for (let i = 0; i < 18; i++) {
        const dark = bit(bits, i);
        const a = this.size - 11 + (i % 3);
        const b = Math.floor(i / 3);
        this.set(a, b, dark);
        this.set(b, a, dark);
      }
    }
  }

  /** The module positions data is written to, in reading order. */
  *dataPositions(): Generator<[number, number]> {
    for (let right = this.size - 1; right >= 1; right -= 2) {
      if (right === 6) right = 5;
      for (let vert = 0; vert < this.size; vert++) {
        for (let j = 0; j < 2; j++) {
          const x = right - j;
          const upward = ((right + 1) & 2) === 0;
          const y = upward ? this.size - 1 - vert : vert;
          if (!this.isFunction[y][x]) yield [x, y];
        }
      }
    }
  }

  drawCodewords(codewords: number[]) {
    let i = 0;
    for (const [x, y] of this.dataPositions()) {
      this.modules[y][x] = i < codewords.length * 8 ? bit(codewords[i >>> 3], 7 - (i & 7)) : false;
      i++;
    }
  }

  applyMask(mask: number) {
    for (let y = 0; y < this.size; y++) {
      for (let x = 0; x < this.size; x++) {
        if (!this.isFunction[y][x] && MASKS[mask](x, y)) this.modules[y][x] = !this.modules[y][x];
      }
    }
  }

  /** A readability score: long runs, 2×2 blocks and an unbalanced dark ratio cost points. */
  penalty(): number {
    let score = 0;
    const n = this.size;
    for (const rows of [this.modules, this.modules[0].map((_, x) => this.modules.map(row => row[x]))]) {
      for (const row of rows) {
        let run = 1;
        for (let i = 1; i < n; i++) {
          if (row[i] === row[i - 1]) {
            run++;
            if (run === 5) score += 3;
            else if (run > 5) score++;
          } else run = 1;
        }
      }
    }
    for (let y = 0; y < n - 1; y++) {
      for (let x = 0; x < n - 1; x++) {
        const color = this.modules[y][x];
        if (color === this.modules[y][x + 1] && color === this.modules[y + 1][x] && color === this.modules[y + 1][x + 1]) score += 3;
      }
    }
    const dark = this.modules.reduce((acc, row) => acc + row.filter(Boolean).length, 0);
    const total = n * n;
    score += (Math.ceil(Math.abs(dark * 20 - total * 10) / total) - 1) * 10;
    return score;
  }
}

export interface QrCode {
  version: number;
  level: QrLevel;
  mask: number;
  size: number;
  /** `modules[y][x]` is true for a dark module. */
  modules: boolean[][];
}

/** The module positions that are not data (finders, timing, format, version). */
export function functionModules(version: number, level: QrLevel): boolean[][] {
  return new Grid(version, level).isFunction;
}

/** Data positions in reading order, for a version. */
export function dataPositions(version: number, level: QrLevel): Array<[number, number]> {
  return Array.from(new Grid(version, level).dataPositions());
}

/** Blocks, error-correction codewords per block and total codewords of a version. */
export function blockLayout(version: number, level: QrLevel) {
  return {
    blocks: BLOCKS[level][version],
    eccLength: ECC_PER_BLOCK[level][version],
    raw: Math.floor(rawDataModules(version) / 8),
  };
}

/** Encode `text` (UTF-8, byte mode) at the lowest version that holds it. Returns
 *  null when it needs more than version 10. `forcedMask` pins the mask (tests). */
export function encodeQr(text: string, level: QrLevel = 'M', forcedMask?: number): QrCode | null {
  const bytes = utf8(text);
  for (let version = 1; version <= MAX_VERSION; version++) {
    const lengthBits = version <= 9 ? 8 : 16;
    if (4 + lengthBits + bytes.length * 8 > dataCodewords(version, level) * 8) continue;
    if (bytes.length >= 1 << lengthBits) return null;
    const codewords = interleave(encodeData(bytes, version, level), version, level);
    let best: { mask: number; grid: Grid; score: number } | null = null;
    for (let mask = forcedMask ?? 0; mask < (forcedMask === undefined ? 8 : forcedMask + 1); mask++) {
      const grid = new Grid(version, level);
      grid.drawCodewords(codewords);
      grid.applyMask(mask);
      grid.drawFormat(mask);
      const score = grid.penalty();
      if (!best || score < best.score) best = { mask, grid, score };
    }
    const { mask, grid } = best as { mask: number; grid: Grid };
    return { version, level, mask, size: grid.size, modules: grid.modules };
  }
  return null;
}
