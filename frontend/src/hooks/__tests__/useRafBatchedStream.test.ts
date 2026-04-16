import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useRafBatchedStream } from '../useRafBatchedStream';

describe('useRafBatchedStream', () => {
  let rafCallbacks: FrameRequestCallback[] = [];
  let originalRaf: typeof requestAnimationFrame;

  beforeEach(() => {
    rafCallbacks = [];
    originalRaf = globalThis.requestAnimationFrame;
    // Manual rAF — caller runs frames explicitly via flushFrame() below.
    globalThis.requestAnimationFrame = ((cb: FrameRequestCallback) => {
      rafCallbacks.push(cb);
      return rafCallbacks.length;
    }) as typeof requestAnimationFrame;
  });

  afterEach(() => {
    globalThis.requestAnimationFrame = originalRaf;
  });

  function flushFrame() {
    const cbs = rafCallbacks.slice();
    rafCallbacks = [];
    for (const cb of cbs) cb(performance.now());
  }

  it('batches multiple chunks within the same frame into one flush call', () => {
    const flush = vi.fn();
    const { result } = renderHook(() => useRafBatchedStream(flush));

    act(() => {
      result.current('d-1', 'Hel');
      result.current('d-1', 'lo');
      result.current('d-1', ' world');
    });

    // rAF not yet fired — nothing flushed.
    expect(flush).not.toHaveBeenCalled();

    act(() => flushFrame());

    expect(flush).toHaveBeenCalledTimes(1);
    expect(flush).toHaveBeenCalledWith({ 'd-1': 'Hello world' });
  });

  it('accumulates chunks per key independently', () => {
    const flush = vi.fn();
    const { result } = renderHook(() => useRafBatchedStream(flush));

    act(() => {
      result.current('d-1', 'A');
      result.current('d-2', 'B');
      result.current('d-1', 'C');
    });
    act(() => flushFrame());

    expect(flush).toHaveBeenCalledWith({ 'd-1': 'AC', 'd-2': 'B' });
  });

  it('schedules a new frame for chunks that arrive after a flush', () => {
    const flush = vi.fn();
    const { result } = renderHook(() => useRafBatchedStream(flush));

    act(() => result.current('d-1', 'first'));
    act(() => flushFrame());
    expect(flush).toHaveBeenLastCalledWith({ 'd-1': 'first' });

    act(() => result.current('d-1', 'second'));
    act(() => flushFrame());
    expect(flush).toHaveBeenLastCalledWith({ 'd-1': 'second' });
    expect(flush).toHaveBeenCalledTimes(2);
  });

  it('no-ops when the frame fires with an empty buffer', () => {
    // Defense against manual flush-after-flush or stale scheduled rAF.
    const flush = vi.fn();
    renderHook(() => useRafBatchedStream(flush));
    act(() => flushFrame());
    expect(flush).not.toHaveBeenCalled();
  });

  it('uses the latest flush callback even if the caller does not memoize', () => {
    const first = vi.fn();
    const second = vi.fn();
    const { result, rerender } = renderHook(
      ({ flush }: { flush: (s: Record<string, string>) => void }) =>
        useRafBatchedStream(flush),
      { initialProps: { flush: first } },
    );
    act(() => result.current('d-1', 'x'));
    // Caller re-renders with a fresh closure.
    rerender({ flush: second });
    act(() => flushFrame());
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledWith({ 'd-1': 'x' });
  });
});
