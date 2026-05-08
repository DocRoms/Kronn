// Stream watchdog — TD-20260504 unit tests.
// Covers the decision logic for detecting dead streams so the
// Dashboard's setInterval doesn't have to be tested as a unit (its
// only concern is wiring inputs/outputs of this helper).

import { describe, it, expect } from 'vitest';
import { detectStaleStreams, DEFAULT_STREAM_STALE_MS } from '../stream-watchdog';

const NOW = 10_000_000;
const FIVE_MIN = DEFAULT_STREAM_STALE_MS;

describe('detectStaleStreams', () => {
  it('returns nothing when no spinners are active', () => {
    expect(detectStaleStreams({
      sendingMap: { d1: false, d2: false },
      lastTickMap: {},
      sendingStartMap: {},
      now: NOW,
    })).toEqual([]);
  });

  it('returns nothing when chunks arrived recently', () => {
    expect(detectStaleStreams({
      sendingMap: { d1: true },
      lastTickMap: { d1: NOW - 30_000 },
      sendingStartMap: { d1: NOW - 60_000 },
      now: NOW,
    })).toEqual([]);
  });

  it('flags a discussion whose last chunk is older than the threshold', () => {
    expect(detectStaleStreams({
      sendingMap: { d1: true },
      lastTickMap: { d1: NOW - FIVE_MIN - 1000 },
      sendingStartMap: { d1: NOW - 10 * 60_000 },
      now: NOW,
    })).toEqual(['d1']);
  });

  it('flags a discussion that never produced a chunk if start is stale', () => {
    // Mirrors the worst case: laptop suspended *between* "Send" and
    // the first SSE delta — sendingStart fires but no chunk ever lands.
    expect(detectStaleStreams({
      sendingMap: { d1: true },
      lastTickMap: {},
      sendingStartMap: { d1: NOW - FIVE_MIN - 1 },
      now: NOW,
    })).toEqual(['d1']);
  });

  it('does NOT flag a discussion missing both lastTick and sendingStart', () => {
    // Defensive: don't throw / clear a stuck-looking spinner when we
    // have no idea when it started — that's a separate bug, not staleness.
    expect(detectStaleStreams({
      sendingMap: { d1: true },
      lastTickMap: {},
      sendingStartMap: {},
      now: NOW,
    })).toEqual([]);
  });

  it('respects a custom threshold', () => {
    // Allows tests / future "aggressive recovery mode" without changing
    // the canonical 5-min default.
    expect(detectStaleStreams({
      sendingMap: { d1: true },
      lastTickMap: { d1: NOW - 90_000 },
      sendingStartMap: { d1: NOW - 90_000 },
      now: NOW,
      thresholdMs: 60_000,
    })).toEqual(['d1']);
  });

  it('isolates the decision per discussion', () => {
    expect(detectStaleStreams({
      sendingMap: { d1: true, d2: true, d3: true },
      lastTickMap: {
        d1: NOW - FIVE_MIN - 5_000, // stale
        d2: NOW - 30_000,           // fresh
        // d3: no tick
      },
      sendingStartMap: {
        d3: NOW - FIVE_MIN - 5_000, // stale via start
      },
      now: NOW,
    }).sort()).toEqual(['d1', 'd3']);
  });

  it('treats lastTick as the source of truth when both are present', () => {
    // Even with an ancient sendingStart, a recent chunk means the
    // stream is alive — don't kill it.
    expect(detectStaleStreams({
      sendingMap: { d1: true },
      lastTickMap: { d1: NOW - 60_000 },
      sendingStartMap: { d1: NOW - 30 * 60_000 },
      now: NOW,
    })).toEqual([]);
  });
});
