import { describe, it, expect } from 'vitest';
import {
  freshnessForPresence,
  freshnessOf,
  honestPresenceState,
  presenceFromActivity,
  secondsUntil,
} from '../discPresence';

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

describe('honestPresenceState (0.9.2 server contract + legacy fallback)', () => {
  const recent = new Date(Date.now() - 30_000).toISOString();
  const stale = new Date(Date.now() - 20 * 60_000).toISOString();

  it('trusts a known server state but always projects paused to offline', () => {
    expect(honestPresenceState('listening', 'active', null, stale, AWAY)).toBe('listening');
    expect(honestPresenceState('dormant', 'active', 'listening', recent, AWAY)).toBe('dormant');
    expect(honestPresenceState('resume_expected', 'active', null, stale, AWAY)).toBe('resume_expected');
    expect(honestPresenceState('stalled', 'active', null, recent, AWAY)).toBe('stalled');
    expect(honestPresenceState('quota_exhausted', 'active', null, recent, AWAY)).toBe('quota_exhausted');
    expect(honestPresenceState('listening', 'paused', 'listening', recent, AWAY)).toBe('offline');
  });

  it('never claims a legacy heartbeat alone means listening', () => {
    expect(honestPresenceState(undefined, 'active', null, recent, AWAY)).toBe('dormant');
    expect(honestPresenceState(undefined, 'active', null, stale, AWAY)).toBe('offline');
    expect(honestPresenceState(undefined, 'active', 'waiting', stale, AWAY)).toBe('dormant');
    expect(honestPresenceState(undefined, 'active', 'reading', stale, AWAY)).toBe('listening');
  });

  it('maps honest states to the legacy color freshness scale', () => {
    expect(freshnessForPresence('running')).toBe('fresh');
    expect(freshnessForPresence('listening')).toBe('fresh');
    expect(freshnessForPresence('dormant')).toBe('idle');
    expect(freshnessForPresence('resume_expected')).toBe('idle');
    expect(freshnessForPresence('stalled')).toBe('away');
    expect(freshnessForPresence('quota_exhausted')).toBe('away');
    expect(freshnessForPresence('offline')).toBe('away');
  });

  it('computes a safe non-negative next-poll delay', () => {
    expect(secondsUntil(null)).toBeNull();
    expect(secondsUntil('not-a-date')).toBeNull();
    expect(secondsUntil(new Date(Date.now() - 1_000).toISOString())).toBe(0);
    expect(secondsUntil(new Date(Date.now() + 30_000).toISOString())).toBeGreaterThan(0);
  });
});
