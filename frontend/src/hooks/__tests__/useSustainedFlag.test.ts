// #220 — the realtime banner showed on every socket close, even one that
// reconnected within a second.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { useSustainedFlag } from '../useSustainedFlag';

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('useSustainedFlag', () => {
  it('stays false for a state that clears before the delay', () => {
    const { result, rerender } = renderHook(({ flag }) => useSustainedFlag(flag, 3000), { initialProps: { flag: true } });
    act(() => { vi.advanceTimersByTime(2999); });
    expect(result.current).toBe(false);
    rerender({ flag: false });
    act(() => { vi.advanceTimersByTime(5000); });
    expect(result.current).toBe(false);
  });

  it('turns true once the state has lasted the delay, and false as soon as it clears', () => {
    const { result, rerender } = renderHook(({ flag }) => useSustainedFlag(flag, 3000), { initialProps: { flag: true } });
    act(() => { vi.advanceTimersByTime(3000); });
    expect(result.current).toBe(true);
    rerender({ flag: false });
    expect(result.current).toBe(false);
  });

  it('starts the delay over when the state comes back', () => {
    const { result, rerender } = renderHook(({ flag }) => useSustainedFlag(flag, 3000), { initialProps: { flag: true } });
    act(() => { vi.advanceTimersByTime(2000); });
    rerender({ flag: false });
    rerender({ flag: true });
    act(() => { vi.advanceTimersByTime(2000); });
    expect(result.current).toBe(false);
    act(() => { vi.advanceTimersByTime(1000); });
    expect(result.current).toBe(true);
  });
});
