import { afterEach, describe, expect, it, vi } from 'vitest';
import { safeGetItem, safeSetItem } from '../safeStorage';

describe('safeStorage', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    localStorage.clear();
  });

  it('round-trips a value, unicode included', () => {
    safeSetItem('k', 'été ✓');
    expect(safeGetItem('k')).toBe('été ✓');
  });

  it('returns null for a missing key', () => {
    expect(safeGetItem('absent')).toBeNull();
  });

  it('does not throw when storage is unavailable or full', () => {
    vi.spyOn(localStorage, 'getItem').mockImplementation(() => { throw new Error('denied'); });
    vi.spyOn(localStorage, 'setItem').mockImplementation(() => { throw new Error('quota'); });
    expect(() => safeSetItem('k', 'v')).not.toThrow();
    expect(safeGetItem('k')).toBeNull();
  });
});
