/** KT-1686 — the recovery phrase Kronn writes for the owner. */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { generateRecoveryPhrase } from '../recoveryPhrase';

afterEach(() => vi.restoreAllMocks());

describe('generateRecoveryPhrase', () => {
  it('is five groups of five characters, long enough for the backend, with no look-alike', () => {
    const phrase = generateRecoveryPhrase();
    expect(phrase).toMatch(/^([a-hjkmnp-z2-9]{5}-){4}[a-hjkmnp-z2-9]{5}$/);
    expect(phrase.length).toBeGreaterThanOrEqual(12);
  });

  it('is drawn from the system random source, and differs each time', () => {
    const spy = vi.spyOn(crypto, 'getRandomValues');
    const seen = new Set(Array.from({ length: 50 }, () => generateRecoveryPhrase()));
    expect(seen.size).toBe(50);
    expect(spy).toHaveBeenCalled();
  });

  it('draws again a byte that would favour the first symbols', () => {
    // 248..255 are past the last full multiple of 31: they must be skipped
    // (249 would otherwise read as 'b').
    const bytes = [255, 249, 248, 0, 31, 62];
    let i = 0;
    vi.spyOn(crypto, 'getRandomValues').mockImplementation(array => {
      (array as Uint8Array)[0] = bytes[i++ % bytes.length];
      return array;
    });
    expect(generateRecoveryPhrase().replace(/-/g, '')).toBe('a'.repeat(25));
  });

  it('draws again the first byte past the last full multiple', () => {
    // 248 would read as 'a': skipped, only the 1 ('b') is kept.
    const bytes = [248, 1];
    let i = 0;
    vi.spyOn(crypto, 'getRandomValues').mockImplementation(array => {
      (array as Uint8Array)[0] = bytes[i++ % bytes.length];
      return array;
    });
    expect(generateRecoveryPhrase().replace(/-/g, '')).toBe('b'.repeat(25));
  });

  it('keeps the last byte of the last full multiple', () => {
    vi.spyOn(crypto, 'getRandomValues').mockImplementation(array => {
      (array as Uint8Array)[0] = 247;
      return array;
    });
    expect(generateRecoveryPhrase().replace(/-/g, '')).toBe('9'.repeat(25));
  });
});
