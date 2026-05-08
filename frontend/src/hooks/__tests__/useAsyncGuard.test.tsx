import { describe, it, expect, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useAsyncGuard } from '../useAsyncGuard';

describe('useAsyncGuard', () => {
  it('runs the inner fn the first time', async () => {
    const inner = vi.fn().mockResolvedValue('ok');
    const { result } = renderHook(() => useAsyncGuard(inner));
    await act(async () => {
      const out = await result.current('arg');
      expect(out).toBe('ok');
    });
    expect(inner).toHaveBeenCalledTimes(1);
    expect(inner).toHaveBeenCalledWith('arg');
  });

  it('blocks re-entry while the first call is still in flight', async () => {
    let resolveInner: ((v: string) => void) | undefined;
    const inner = vi.fn(() => new Promise<string>(resolve => { resolveInner = resolve; }));
    const { result } = renderHook(() => useAsyncGuard(inner));

    let firstResult: string | undefined;
    let secondResult: string | undefined;

    await act(async () => {
      const p1 = result.current().then(r => { firstResult = r; });
      const p2 = result.current().then(r => { secondResult = r; });
      // Second call returned `undefined` synchronously (ref already true).
      // The inner fn was only invoked once.
      expect(inner).toHaveBeenCalledTimes(1);
      resolveInner?.('ok');
      await Promise.all([p1, p2]);
    });

    expect(firstResult).toBe('ok');
    expect(secondResult).toBeUndefined();
  });

  it('allows the next call after the first one resolves', async () => {
    const inner = vi.fn().mockResolvedValue('ok');
    const { result } = renderHook(() => useAsyncGuard(inner));
    await act(async () => { await result.current(); });
    await act(async () => { await result.current(); });
    expect(inner).toHaveBeenCalledTimes(2);
  });

  it('releases the guard even if the inner fn rejects', async () => {
    const inner = vi.fn()
      .mockRejectedValueOnce(new Error('boom'))
      .mockResolvedValueOnce('ok');
    const { result } = renderHook(() => useAsyncGuard(inner));

    await act(async () => {
      await expect(result.current()).rejects.toThrow('boom');
    });
    // Guard released — the second call goes through.
    await act(async () => {
      const out = await result.current();
      expect(out).toBe('ok');
    });
    expect(inner).toHaveBeenCalledTimes(2);
  });
});
