import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { usePersistentFold } from '../usePersistentFold';

const KEY = 'kronn:test:persistent-fold';

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe('usePersistentFold', () => {
  it('follows the default until the user decides, without writing it', () => {
    const { result, rerender } = renderHook(
      ({ defaultOpen }) => usePersistentFold(KEY, defaultOpen),
      { initialProps: { defaultOpen: true } },
    );
    expect(result.current[0]).toBe(true);
    // The default moves (a model got installed): the block follows it.
    rerender({ defaultOpen: false });
    expect(result.current[0]).toBe(false);
    expect(localStorage.getItem(KEY)).toBeNull();
  });

  it('an explicit toggle wins over the default from then on, and is remembered', () => {
    const first = renderHook(({ defaultOpen }) => usePersistentFold(KEY, defaultOpen), {
      initialProps: { defaultOpen: false },
    });
    act(() => first.result.current[1]());
    expect(first.result.current[0]).toBe(true);
    expect(localStorage.getItem(KEY)).toBe('1');

    first.rerender({ defaultOpen: false });
    expect(first.result.current[0]).toBe(true);

    // A later session reads it back, whatever its own default says.
    const second = renderHook(() => usePersistentFold(KEY, false));
    expect(second.result.current[0]).toBe(true);

    act(() => second.result.current[1]());
    expect(second.result.current[0]).toBe(false);
    expect(localStorage.getItem(KEY)).toBe('0');
    expect(renderHook(() => usePersistentFold(KEY, true)).result.current[0]).toBe(false);
  });

  it('treats an unreadable stored value as no choice', () => {
    localStorage.setItem(KEY, 'banana');
    expect(renderHook(() => usePersistentFold(KEY, true)).result.current[0]).toBe(true);
    expect(renderHook(() => usePersistentFold(KEY, false)).result.current[0]).toBe(false);
  });

  it('stays usable in memory when storage is unavailable', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('denied'); });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('denied'); });
    const { result } = renderHook(() => usePersistentFold(KEY, false));
    expect(result.current[0]).toBe(false);
    act(() => result.current[1]());
    expect(result.current[0]).toBe(true);
  });
});
