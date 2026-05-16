// 0.8.4 follow-up — pin the localStorage-backed "deployed at v<N>"
// marker the banner uses to render the disabled state.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  deployedVersionKey,
  getDeployedVersion,
  setDeployedVersion,
  clearDeployedVersion,
} from '../qp-improver-banner';

describe('QP improver deployed marker (localStorage)', () => {
  beforeEach(() => {
    localStorage.clear();
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('builds the canonical storage key', () => {
    expect(deployedVersionKey('disc-abc')).toBe('kronn:qpDisc:disc-abc:deployedVersion');
  });

  it('returns null when no marker is set', () => {
    expect(getDeployedVersion('disc-1')).toBeNull();
  });

  it('round-trips a positive integer version', () => {
    setDeployedVersion('disc-1', 3);
    expect(getDeployedVersion('disc-1')).toBe(3);
  });

  it('isolates markers between discussions', () => {
    setDeployedVersion('disc-A', 2);
    setDeployedVersion('disc-B', 7);
    expect(getDeployedVersion('disc-A')).toBe(2);
    expect(getDeployedVersion('disc-B')).toBe(7);
  });

  it('clearDeployedVersion removes the marker', () => {
    setDeployedVersion('disc-1', 4);
    clearDeployedVersion('disc-1');
    expect(getDeployedVersion('disc-1')).toBeNull();
  });

  it('returns null on a non-numeric stored value (defensive)', () => {
    localStorage.setItem(deployedVersionKey('disc-1'), 'not-a-number');
    expect(getDeployedVersion('disc-1')).toBeNull();
  });

  it('returns null on a zero or negative stored value (defensive)', () => {
    localStorage.setItem(deployedVersionKey('disc-1'), '0');
    expect(getDeployedVersion('disc-1')).toBeNull();
    localStorage.setItem(deployedVersionKey('disc-1'), '-5');
    expect(getDeployedVersion('disc-1')).toBeNull();
  });

  it('swallows localStorage exceptions on read (Safari private mode)', () => {
    const spy = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('storage disabled');
    });
    expect(getDeployedVersion('disc-1')).toBeNull();
    spy.mockRestore();
  });

  it('swallows localStorage exceptions on write (quota exceeded)', () => {
    const spy = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('quota exceeded');
    });
    // Must NOT throw — the caller already navigated; losing the marker
    // is acceptable.
    expect(() => setDeployedVersion('disc-1', 4)).not.toThrow();
    spy.mockRestore();
  });
});
