import { describe, expect, it } from 'vitest';

// The two specs below are order-dependent on purpose (shuffle is pinned off for
// this block): the first leaves state behind, the second proves setup.ts's
// afterEach wiped it.
describe('Vitest storage isolation', { shuffle: false }, () => {
  it('lets a spec write to both stores', () => {
    localStorage.setItem('kronn:isolation-probe', 'local');
    sessionStorage.setItem('kronn:isolation-probe', 'session');
    expect(localStorage.getItem('kronn:isolation-probe')).toBe('local');
    expect(sessionStorage.getItem('kronn:isolation-probe')).toBe('session');
  });

  it('starts the next spec with both stores empty', () => {
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);
  });
});
