import { describe, it, expect } from 'vitest';
import { freshnessOf, presenceFromActivity } from '../discPresence';

const AWAY = 600_000; // 480s cap + 2min margin

describe('freshnessOf (heartbeat only)', () => {
  it('no last_seen → away', () => {
    expect(freshnessOf(null, AWAY)).toBe('away');
  });
  it('recent → fresh, mid → idle, old → away', () => {
    const now = Date.now();
    expect(freshnessOf(new Date(now - 30_000).toISOString(), AWAY)).toBe('fresh');
    expect(freshnessOf(new Date(now - 5 * 60_000).toISOString(), AWAY)).toBe('idle');
    expect(freshnessOf(new Date(now - 20 * 60_000).toISOString(), AWAY)).toBe('away');
  });
});

describe('presenceFromActivity (presence-gap fix)', () => {
  // The core of the fix: an unexpired activity means present RIGHT NOW,
  // whatever last_seen's age — so a dormant agent in a long cold-regime
  // pause no longer flips to "away" and the human stops relaunching it.
  const stale = new Date(Date.now() - 20 * 60_000).toISOString(); // would be 'away'

  it('waiting outranks a stale heartbeat → fresh, not away', () => {
    expect(presenceFromActivity('waiting', stale, AWAY)).toBe('fresh');
  });
  it('listening and reading are also live', () => {
    expect(presenceFromActivity('listening', stale, AWAY)).toBe('fresh');
    expect(presenceFromActivity('reading', stale, AWAY)).toBe('fresh');
  });
  it('no activity → falls back to heartbeat freshness', () => {
    expect(presenceFromActivity(null, stale, AWAY)).toBe('away');
    expect(presenceFromActivity(undefined, new Date(Date.now() - 30_000).toISOString(), AWAY)).toBe('fresh');
  });
  it('an unknown/future activity token does NOT fake presence', () => {
    // Forward-compat: only the known live set outranks the heartbeat.
    expect(presenceFromActivity('pondering', stale, AWAY)).toBe('away');
  });
});
