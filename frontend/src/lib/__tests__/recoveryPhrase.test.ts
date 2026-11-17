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
    // 248..255 are past the last full multiple of 31: they must be skipped.
    const bytes = [255, 248, 0, 31, 62];
    let i = 0;
    vi.spyOn(crypto, 'getRandomValues').mockImplementation(array => {
      (array as Uint8Array)[0] = bytes[i++ % bytes.length];
      return array;
    });
    expect(generateRecoveryPhrase().replace(/-/g, '')).toBe('a'.repeat(25));
  });
});
