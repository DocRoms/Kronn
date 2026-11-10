import { useEffect, useMemo } from 'react';
import { workflows as workflowsApi } from '../lib/api';
import { quotaPausedRuns } from '../lib/quotaPause';
import { useApi } from './useApi';

/** The engine re-checks a paused provider every ~20 min; a minute is a fresh enough view. */
const POLL_MS = 30_000;

/**
 * The runs paused on a provider quota, with their wake time (KT-1657). Read
 * from the active-run board, so a paused run stays visible while it holds a
 * workflow's concurrency slot.
 */
export function useQuotaPausedRuns(pollMs: number = POLL_MS) {
  const { data, refetch } = useApi(() => workflowsApi.activeRuns(), []);

  useEffect(() => {
    const interval = setInterval(refetch, pollMs);
    const onVisibilityChange = () => {
      if (document.visibilityState === 'visible') refetch();
    };
    document.addEventListener('visibilitychange', onVisibilityChange);
    return () => {
      clearInterval(interval);
      document.removeEventListener('visibilitychange', onVisibilityChange);
    };
  }, [refetch, pollMs]);

  const runs = useMemo(() => quotaPausedRuns(data), [data]);
  return { runs, refetch };
}
