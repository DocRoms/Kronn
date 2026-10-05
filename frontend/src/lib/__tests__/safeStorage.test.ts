import { afterEach, describe, expect, it, vi } from 'vitest';
import { safeGetItem, safeRemoveItem, safeSetItem, setStorageWriteListener } from '../safeStorage';

describe('safeStorage', () => {
  afterEach(() => {
    setStorageWriteListener(null);
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

  it('tells the write listener about successful writes and removals only', () => {
    const heard: string[] = [];
    setStorageWriteListener(key => heard.push(key));
    safeSetItem('kronn:theme', 'dark');
    safeRemoveItem('kronn:theme');
    expect(safeGetItem('kronn:theme')).toBeNull();
    vi.spyOn(localStorage, 'setItem').mockImplementation(() => { throw new Error('quota'); });
    safeSetItem('kronn:theme', 'light');
    expect(heard).toEqual(['kronn:theme', 'kronn:theme']);
  });

  it('never lets a failing listener break the write', () => {
    setStorageWriteListener(() => { throw new Error('mirror down'); });
    expect(() => safeSetItem('k', 'v')).not.toThrow();
    expect(safeGetItem('k')).toBe('v');
  });
});
