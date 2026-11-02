/**
 * KT-1492 — the QR encoder is checked by reading its own matrix back the way a
 * scanner does: format bits (twice), unmasking, zigzag read, de-interleaving,
 * every block's Reed–Solomon check, then the byte segment. The tables are
 * checked against the standard's byte capacities.
 */
import { describe, expect, it } from 'vitest';
import {
  blockLayout, dataPositions, divisor, encodeQr, formatBits, remainder, versionBits,
  type QrCode, type QrLevel,
} from '../qr';

const MASKS: Array<(x: number, y: number) => boolean> = [
  (x, y) => (x + y) % 2 === 0,
  (_x, y) => y % 2 === 0,
  x => x % 3 === 0,
  (x, y) => (x + y) % 3 === 0,
  (x, y) => (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0,
  (x, y) => ((x * y) % 2) + ((x * y) % 3) === 0,
  (x, y) => (((x * y) % 2) + ((x * y) % 3)) % 2 === 0,
  (x, y) => (((x + y) % 2) + ((x * y) % 3)) % 2 === 0,
];

const bit = (value: number, index: number) => (value >>> index) & 1;

/** What a scanner reads: the text, after checking everything it can check. */
function scan(qr: QrCode): string {
  const { modules, size, version, level } = qr;
  // Format information, first copy.
  let first = 0;
  const at = (x: number, y: number) => (modules[y][x] ? 1 : 0);
  for (let i = 0; i <= 5; i++) first |= at(8, i) << i;
  first |= at(8, 7) << 6;
  first |= at(8, 8) << 7;
  first |= at(7, 8) << 8;
  for (let i = 9; i < 15; i++) first |= at(14 - i, 8) << i;
  // Second copy.
  let second = 0;
  for (let i = 0; i < 8; i++) second |= at(size - 1 - i, 8) << i;
  for (let i = 8; i < 15; i++) second |= at(8, size - 15 + i) << i;
  expect(second).toBe(first);
  const data = (first ^ 0x5412) >>> 10;
  const mask = data & 7;
  expect(data >>> 3).toBe(level === 'L' ? 1 : 0);
  expect(formatBits(level, mask)).toBe(first);
  expect(mask).toBe(qr.mask);
  expect(at(8, size - 8)).toBe(1);

  // Unmask and read the data modules in the standard order.
  const bits = dataPositions(version, level).map(([x, y]) => (at(x, y) ^ (MASKS[mask](x, y) ? 1 : 0)));
  const { blocks, eccLength, raw } = blockLayout(version, level);
  const codewords: number[] = [];
  for (let i = 0; i < raw; i++) {
    codewords.push(bits.slice(i * 8, i * 8 + 8).reduce((acc, b) => (acc << 1) | b, 0));
  }
  // The remainder bits are zero.
  expect(bits.slice(raw * 8).every(b => b === 0)).toBe(true);

  // De-interleave: data column by column, then error correction column by column.
  const shortBlocks = blocks - (raw % blocks);
  const shortLength = Math.floor(raw / blocks);
  const dataLength = (j: number) => shortLength - eccLength + (j < shortBlocks ? 0 : 1);
  const dataBlocks: number[][] = Array.from({ length: blocks }, () => []);
  const eccBlocks: number[][] = Array.from({ length: blocks }, () => []);
  let cursor = 0;
  for (let i = 0; i < dataLength(blocks - 1); i++) {
    for (let j = 0; j < blocks; j++) if (i < dataLength(j)) dataBlocks[j].push(codewords[cursor++]);
  }
  for (let i = 0; i < eccLength; i++) {
    for (let j = 0; j < blocks; j++) eccBlocks[j].push(codewords[cursor++]);
  }
  expect(cursor).toBe(raw);
  const div = divisor(eccLength);
  dataBlocks.forEach((block, j) => expect(remainder(block, div)).toEqual(eccBlocks[j]));

  // The byte segment.
  const stream = dataBlocks.flat().flatMap(byte => Array.from({ length: 8 }, (_, i) => bit(byte, 7 - i)));
  const read = (from: number, length: number) =>
    stream.slice(from, from + length).reduce((acc, b) => (acc << 1) | b, 0);
  expect(read(0, 4)).toBe(0b0100);
  const countBits = version <= 9 ? 8 : 16;
  const count = read(4, countBits);
  const bytes = Array.from({ length: count }, (_, i) => read(4 + countBits + i * 8, 8));
  return new TextDecoder().decode(Uint8Array.from(bytes));
}

describe('the QR encoder reads back as a scanner would', () => {
  const texts = [
    'a',
    'otpauth://totp/Kronn:me?secret=JBSWY3DPEHPK3PXP',
    'otpauth://totp/Kronn:human?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Kronn&algorithm=SHA1&digits=6&period=30',
    'x'.repeat(150),
    'é€漢字🙂 — ' + 'z'.repeat(30),
    'x'.repeat(200),
  ];
  for (const level of ['L', 'M'] as QrLevel[]) {
    for (const text of texts) {
      it(`level ${level}, ${new TextEncoder().encode(text).length} bytes`, () => {
        const qr = encodeQr(text, level);
        expect(qr).not.toBeNull();
        expect(scan(qr as QrCode)).toBe(text);
      });
    }
  }

  it('every one of the eight masks is a valid code', () => {
    const text = 'otpauth://totp/Kronn:human?secret=GEZDGNBVGY3TQOJQ&issuer=Kronn';
    for (let mask = 0; mask < 8; mask++) {
      for (const level of ['L', 'M'] as QrLevel[]) {
        const qr = encodeQr(text, level, mask) as QrCode;
        expect(qr.mask).toBe(mask);
        expect(scan(qr)).toBe(text);
      }
    }
  });

  it('every version holds exactly the standard byte capacity', () => {
    const capacity: Record<QrLevel, number[]> = {
      L: [17, 32, 53, 78, 106, 134, 154, 192, 230, 271],
      M: [14, 26, 42, 62, 84, 106, 122, 152, 180, 213],
    };
    for (const level of ['L', 'M'] as QrLevel[]) {
      capacity[level].forEach((bytes, index) => {
        const version = index + 1;
        const fits = encodeQr('a'.repeat(bytes), level) as QrCode;
        expect(fits.version).toBe(version);
        expect(scan(fits)).toBe('a'.repeat(bytes));
        const next = encodeQr('a'.repeat(bytes + 1), level);
        expect(next === null || next.version === version + 1).toBe(true);
      });
    }
    expect(encodeQr('a'.repeat(272), 'L')).toBeNull();
  });

  it('draws the three finder patterns and the timing pattern', () => {
    const qr = encodeQr('hello', 'M') as QrCode;
    const { modules, size } = qr;
    for (const [ox, oy] of [[0, 0], [size - 7, 0], [0, size - 7]]) {
      for (let dy = 0; dy < 7; dy++) {
        for (let dx = 0; dx < 7; dx++) {
          const ring = Math.max(Math.abs(dx - 3), Math.abs(dy - 3));
          expect(modules[oy + dy][ox + dx]).toBe(ring !== 2);
        }
      }
    }
    for (let i = 8; i < size - 8; i++) expect(modules[6][i]).toBe(i % 2 === 0);
  });

  it('matches the standard format and version words', () => {
    // ISO/IEC 18004 table of format information: level M mask 0 and level L mask 0.
    expect(formatBits('M', 0)).toBe(0b101010000010010);
    expect(formatBits('L', 0)).toBe(0b111011111000100);
    // Version information of version 7.
    expect(versionBits(7)).toBe(0b000111110010010100);
  });
});
