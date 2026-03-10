import { describe, it, expect, vi } from 'vitest';
import { renderHook, waitFor, act } from '@testing-library/react';
import { useApi } from '../useApi';

describe('useApi', () => {
  it('fetches data on mount', async () => {
    const fetcher = vi.fn().mockResolvedValue({ id: '1', name: 'test' });
    const { result } = renderHook(() => useApi(fetcher, []));

    // Initially loading
    expect(result.current.loading).toBe(true);
    expect(result.current.data).toBeNull();

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(result.current.data).toEqual({ id: '1', name: 'test' });
    expect(result.current.error).toBeNull();
    expect(fetcher).toHaveBeenCalledOnce();
  });

  it('handles fetch errors', async () => {
    const fetcher = vi.fn().mockRejectedValue(new Error('Network error'));
    const { result } = renderHook(() => useApi(fetcher, []));

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(result.current.data).toBeNull();
    expect(result.current.error).toBe('Network error');
  });

  it('handles non-Error thrown values', async () => {
    const fetcher = vi.fn().mockRejectedValue('string error');
    const { result } = renderHook(() => useApi(fetcher, []));

    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(result.current.error).toBe('Unknown error');
  });

  it('refetch re-fetches data', async () => {
    let callCount = 0;
    const fetcher = vi.fn().mockImplementation(() => {
      callCount++;
      return Promise.resolve(`result-${callCount}`);
    });

    const { result } = renderHook(() => useApi(fetcher, []));
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.data).toBe('result-1');

    // Trigger refetch inside act() to flush state updates
    act(() => { result.current.refetch(); });
    await waitFor(() => expect(result.current.data).toBe('result-2'));
  });

  it('only applies latest result (race condition protection)', async () => {
    const resolvers: Array<(value: string) => void> = [];
    const fetcher = vi.fn().mockImplementation(() => {
      return new Promise<string>(resolve => {
        resolvers.push(resolve);
      });
    });

    const { result } = renderHook(() => useApi(fetcher, []));

    // Wait for initial call
    await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());

    // Trigger a refetch before first one resolves
    act(() => { result.current.refetch(); });
    await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));

    // Resolve in reverse order: second one first, then first
    act(() => { resolvers[1]('second'); });
    act(() => { resolvers[0]('first'); });

    await waitFor(() => expect(result.current.loading).toBe(false));

    // Should only have the second (latest) result
    expect(result.current.data).toBe('second');
  });
});
