// Stream-flush helper — TD-20260504 unit tests.
//
// Covers the contract that the rAF flush callback in DiscussionsPage
// implements: chunks merge into the running text per discussion AND
// the stale-stream watchdog gets a tick for every discussion the
// snapshot touched.

import { describe, it, expect, vi } from 'vitest';
import type { Dispatch, SetStateAction } from 'react';
import { buildStreamingFlush } from '../stream-flush';

/** Build a `setState`-shaped mock that mirrors React's behaviour:
 *  accepts either a value OR an updater function. Production code in
 *  DiscussionsPage always uses the updater form, but the type signature
 *  demands both. */
function makeSetStateMock(initial: Record<string, string> = {}) {
  let state = initial;
  const fn = vi.fn((arg: SetStateAction<Record<string, string>>) => {
    state = typeof arg === 'function'
      ? (arg as (prev: Record<string, string>) => Record<string, string>)(state)
      : arg;
  }) as unknown as Dispatch<SetStateAction<Record<string, string>>>;
  return { fn, get: () => state };
}

describe('buildStreamingFlush', () => {
  it('appends new chunks to the existing text per discussion', () => {
    const sm = makeSetStateMock({ d1: 'hello ' });
    const noteStreamTick = vi.fn();
    const flush = buildStreamingFlush(sm.fn, noteStreamTick);

    flush({ d1: 'world' });
    expect(sm.get()).toEqual({ d1: 'hello world' });
  });

  it('initialises previously-empty discussions', () => {
    const sm = makeSetStateMock();
    const noteStreamTick = vi.fn();
    const flush = buildStreamingFlush(sm.fn, noteStreamTick);

    flush({ d1: 'first chunk' });
    expect(sm.get()).toEqual({ d1: 'first chunk' });
  });

  it('preserves untouched discussions on a partial snapshot', () => {
    // The stream may flush only one of several active discussions per
    // frame — the others must keep their accumulated text untouched.
    const sm = makeSetStateMock({ d1: 'kept', d2: 'old' });
    const noteStreamTick = vi.fn();
    const flush = buildStreamingFlush(sm.fn, noteStreamTick);

    flush({ d2: ' new' });
    expect(sm.get()).toEqual({ d1: 'kept', d2: 'old new' });
  });

  it('fires noteStreamTick for every discussion in the snapshot', () => {
    // The watchdog needs to know which discussions are alive each
    // frame. Without this tick, the Dashboard's setInterval would
    // wrongly mark a streaming discussion as stale after 5 min.
    const sm = makeSetStateMock();
    const noteStreamTick = vi.fn();
    const flush = buildStreamingFlush(sm.fn, noteStreamTick);

    flush({ d1: 'a', d2: 'b', d3: 'c' });
    expect(noteStreamTick).toHaveBeenCalledTimes(3);
    expect(noteStreamTick).toHaveBeenCalledWith('d1');
    expect(noteStreamTick).toHaveBeenCalledWith('d2');
    expect(noteStreamTick).toHaveBeenCalledWith('d3');
  });

  it('does not tick the watchdog for an empty snapshot', () => {
    const sm = makeSetStateMock();
    const noteStreamTick = vi.fn();
    const flush = buildStreamingFlush(sm.fn, noteStreamTick);

    flush({});
    expect(noteStreamTick).not.toHaveBeenCalled();
  });
});
