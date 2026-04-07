import { useState, useEffect, useCallback, useRef } from 'react';

interface UseApiState<T> {
  data: T | null;
  loading: boolean;
  /** True only on the very first fetch (data is null). False during refetches. */
  initialLoading: boolean;
  error: string | null;
  refetch: () => void;
}

/**
 * Generic hook for API calls with loading/error states.
 * Uses a request counter so only the latest fetch wins (prevents race conditions).
 *
 * Stale-while-revalidate: during a refetch, the previous data stays visible
 * and `loading` is true but `initialLoading` is false. This prevents the
 * "flash of empty state" when navigating back to a page.
 */
export function useApi<T>(
  fetcher: () => Promise<T>,
  deps: unknown[] = [],
): UseApiState<T> {
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);
  const hasLoaded = useRef(false);

  const fetch = useCallback(async () => {
    const thisRequest = ++requestId.current;
    setLoading(true);
    setError(null);
    // Don't reset data to null — keep stale data visible during refetch
    try {
      const result = await fetcher();
      // Only apply result if this is still the latest request
      if (thisRequest === requestId.current) {
        setData(result);
        hasLoaded.current = true;
      }
    } catch (e) {
      if (thisRequest === requestId.current) {
        setError(e instanceof Error ? e.message : 'Unknown error');
        // Keep previous data on error — don't setData(null).
        // This prevents the UI from flashing empty when a transient error occurs
        // (e.g. SQLITE_BUSY during concurrent writes).
      }
    } finally {
      if (thisRequest === requestId.current) {
        setLoading(false);
      }
    }
  }, deps);

  useEffect(() => {
    fetch();
  }, [fetch]);

  return {
    data,
    loading,
    initialLoading: loading && !hasLoaded.current,
    error,
    refetch: fetch,
  };
}
