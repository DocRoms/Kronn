import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { clearCachedResources, invalidateCachedResource, useCachedResource } from '../useCachedResource';

afterEach(clearCachedResources);

const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
};

describe('useCachedResource', () => {
  it('shows the cached value at once on a later mount and refreshes behind it', async () => {
    const load = vi.fn().mockResolvedValueOnce('first');
    const first = renderHook(() => useCachedResource<string>({ key: 'k', load }));
    await waitFor(() => expect(first.result.current.data).toBe('first'));
    first.unmount();

    const next = deferred<string>();
    load.mockReturnValueOnce(next.promise);
    const second = renderHook(() => useCachedResource<string>({ key: 'k', load }));
    expect(second.result.current.data).toBe('first');
    expect(second.result.current.fetchedAt).not.toBeNull();
    expect(second.result.current.refreshing).toBe(true);
    await act(async () => { next.resolve('second'); });
    expect(second.result.current.data).toBe('second');
    expect(second.result.current.refreshing).toBe(false);
  });

  it('keeps the last value when a refresh fails and flags the error', async () => {
    const load = vi.fn().mockResolvedValueOnce('kept').mockRejectedValueOnce(new Error('down'));
    const { result } = renderHook(() => useCachedResource<string>({ key: 'k', load }));
    await waitFor(() => expect(result.current.data).toBe('kept'));
    await act(async () => { await result.current.refresh(); });
    expect(result.current.data).toBe('kept');
    expect(result.current.error).toBe(true);
    expect(result.current.refreshing).toBe(false);
  });

  it('publishes a partial result before the full one lands', async () => {
    const full = deferred<string>();
    const load = vi.fn(async (_force: boolean, publish: (value: string) => void) => {
      publish('partial');
      return full.promise;
    });
    const { result } = renderHook(() => useCachedResource<string>({ key: 'k', load }));
    await waitFor(() => expect(result.current.data).toBe('partial'));
    expect(result.current.refreshing).toBe(true);
    await act(async () => { full.resolve('full'); });
    expect(result.current.data).toBe('full');
  });

  it('passes force only on an explicit refresh and ignores a second one in flight', async () => {
    const pending = deferred<string>();
    const load = vi.fn().mockResolvedValueOnce('a').mockReturnValueOnce(pending.promise);
    const { result } = renderHook(() => useCachedResource<string>({ key: 'k', load }));
    await waitFor(() => expect(result.current.data).toBe('a'));
    expect(load).toHaveBeenLastCalledWith(false, expect.any(Function));
    act(() => { void result.current.refresh(); void result.current.refresh(); });
    expect(load).toHaveBeenCalledTimes(2);
    expect(load).toHaveBeenLastCalledWith(true, expect.any(Function));
    await act(async () => { pending.resolve('b'); });
  });

  it('keeps one project result away from another and does nothing when disabled', async () => {
    const load = vi.fn(async () => 'x');
    const { result, rerender } = renderHook(
      ({ id }: { id: string | null }) => useCachedResource<string>({ key: id, load }),
      { initialProps: { id: 'a' as string | null } },
    );
    await waitFor(() => expect(result.current.data).toBe('x'));
    rerender({ id: 'b' });
    expect(result.current.data).toBeNull();
    rerender({ id: null });
    expect(result.current.data).toBeNull();
    expect(load).toHaveBeenCalledTimes(2);
  });

  it('forgets an invalidated value', async () => {
    const load = vi.fn().mockResolvedValue('old');
    const first = renderHook(() => useCachedResource<string>({ key: 'k', load }));
    await waitFor(() => expect(first.result.current.data).toBe('old'));
    first.unmount();
    invalidateCachedResource('k');
    load.mockReturnValueOnce(new Promise(() => {}));
    const second = renderHook(() => useCachedResource<string>({ key: 'k', load }));
    expect(second.result.current.data).toBeNull();
  });

  it('keeps the new result when an older response arrives late after a remount', async () => {
    const old = deferred<string>();
    const fresh = deferred<string>();
    const load = vi.fn().mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
    const first = renderHook(() => useCachedResource<string>({ key: 'k', load }));
    first.unmount();
    const second = renderHook(() => useCachedResource<string>({ key: 'k', load }));
    await act(async () => { fresh.resolve('new result'); });
    expect(second.result.current.data).toBe('new result');
    await act(async () => { old.resolve('old result'); });
    // Read the shared cache again: the last render alone would hide a stale write.
    second.rerender();
    expect(second.result.current.data).toBe('new result');
  });

  it('ignores a response that lands after the resource was invalidated', async () => {
    const pending = deferred<string>();
    const load = vi.fn().mockReturnValueOnce(pending.promise);
    const { result, rerender } = renderHook(() => useCachedResource<string>({ key: 'k', load }));
    invalidateCachedResource('k');
    await act(async () => { pending.resolve('stale'); });
    rerender();
    expect(result.current.data).toBeNull();
  });

  it('drops a late partial from an older request but takes the newer one in order', async () => {
    let oldPublish!: (value: string) => void;
    const oldFull = deferred<string>();
    const newFull = deferred<string>();
    const load = vi.fn()
      .mockImplementationOnce(async (_f: boolean, publish: (value: string) => void) => {
        oldPublish = publish;
        return oldFull.promise;
      })
      .mockImplementationOnce(async (_f: boolean, publish: (value: string) => void) => {
        publish('new partial');
        return newFull.promise;
      });
    const first = renderHook(() => useCachedResource<string>({ key: 'k', load }));
    first.unmount();
    const second = renderHook(() => useCachedResource<string>({ key: 'k', load }));
    await waitFor(() => expect(second.result.current.data).toBe('new partial'));
    await act(async () => { oldPublish('old partial'); });
    second.rerender();
    expect(second.result.current.data).toBe('new partial');
    await act(async () => { newFull.resolve('new full'); });
    expect(second.result.current.data).toBe('new full');
    await act(async () => { oldFull.resolve('old full'); });
    second.rerender();
    expect(second.result.current.data).toBe('new full');
  });
});
