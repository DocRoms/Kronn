import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useLockExpiry } from '../useLockExpiry';

/** Time passes in small steps, each one flushed: the hook re-arms itself from
 *  a re-render, which `act` only runs when it returns. */
function elapse(ms: number) {
  for (let spent = 0; spent < ms; spent += 250) {
    act(() => {
      vi.advanceTimersByTime(250);
    });
  }
}

describe('useLockExpiry', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('does nothing while nothing is locked', () => {
    const reload = vi.fn();
    renderHook(() => useLockExpiry(0, reload));
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    expect(reload).not.toHaveBeenCalled();
  });

  it('re-reads the lock once it has ended', () => {
    const reload = vi.fn();
    renderHook(() => useLockExpiry(2, reload));
    act(() => {
      vi.advanceTimersByTime(2_200);
    });
    expect(reload).not.toHaveBeenCalled();
    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('re-checks again when the re-read still reports the same remaining seconds', () => {
    const reload = vi.fn();
    // The backend keeps answering "1 s left": the value never changes.
    renderHook(() => useLockExpiry(1, reload));
    act(() => {
      vi.advanceTimersByTime(1_250);
    });
    expect(reload).toHaveBeenCalledTimes(1);
    act(() => {
      vi.advanceTimersByTime(250);
    });
    expect(reload).toHaveBeenCalledTimes(2);
    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(reload).toHaveBeenCalledTimes(3);
  });

  it('backs off and gives up on a value that never moves', () => {
    const reload = vi.fn();
    renderHook(() => useLockExpiry(1, reload));
    elapse(120_000);
    // The first read and five delayed re-checks, then it waits.
    expect(reload).toHaveBeenCalledTimes(6);
  });

  it('starts over when the lock changes, and stops when it is lifted', () => {
    const reload = vi.fn();
    const { rerender } = renderHook(({ secs }) => useLockExpiry(secs, reload), {
      initialProps: { secs: 1 },
    });
    elapse(120_000);
    expect(reload).toHaveBeenCalledTimes(6);
    rerender({ secs: 3 });
    elapse(3_250);
    expect(reload).toHaveBeenCalledTimes(7);
    rerender({ secs: 0 });
    elapse(120_000);
    expect(reload).toHaveBeenCalledTimes(7);
  });

  it('keeps its timer and its back-off when the caller passes a new callback on every render', () => {
    const calls: number[] = [];
    const { rerender } = renderHook(
      ({ tick }) => useLockExpiry(2, () => calls.push(tick)),
      { initialProps: { tick: 0 } },
    );
    // Re-render often, with a fresh function each time, before the lock ends.
    for (let tick = 1; tick <= 7; tick += 1) {
      act(() => {
        vi.advanceTimersByTime(250);
      });
      rerender({ tick });
    }
    // 1.75 s spent: the first re-read is not early, and not pushed back.
    expect(calls).toEqual([]);
    elapse(500);
    // It fired on the original schedule (2.25 s), with the latest callback.
    expect(calls).toEqual([7]);
    elapse(120_000);
    // The back-off was not restarted by the re-renders: first read + 5 re-checks.
    expect(calls).toHaveLength(6);
  });
});
