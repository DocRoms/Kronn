import { useState, useEffect, useCallback, useRef } from 'react';

interface UseApiState<T> {
  data: T | null;
  loading: boolean;
  error: string | null;
  refetch: () => void;
}

/**
 * Generic hook for API calls with loading/error states.
 * Uses a request counter so only the latest fetch wins (prevents race conditions).
 */
export function useApi<T>(
  fetcher: () => Promise<T>,
  deps: unknown[] = [],
): UseApiState<T> {
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);

  const fetch = useCallback(async () => {
    const thisRequest = ++requestId.current;
    setLoading(true);
    setError(null);
    try {
      const result = await fetcher();
      // Only apply result if this is still the latest request
      if (thisRequest === requestId.current) {
        setData(result);
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

  return { data, loading, error, refetch: fetch };
}
