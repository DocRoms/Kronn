import { useCallback, useEffect, useRef, useState } from 'react';
import { activity as activityApi } from '../lib/api';
import {
  ACTIVITY_POLL_BUSY_MS,
  ACTIVITY_POLL_IDLE_MS,
  isAnythingRunning,
  sameActivity,
} from '../lib/activity';
import type { ActivityOverview } from '../types/generated';

/**
 * What is running in Kronn, read from the aggregation endpoint (KT-1658). It
 * polls quickly while something moves and slowly when idle; a failed poll keeps
 * the last good view and flags `failed` so the panel can say it may be stale.
 */
export function useActivityOverview() {
  const [overview, setOverview] = useState<ActivityOverview | null>(null);
  const [failed, setFailed] = useState(false);
  const alive = useRef(true);

  const refetch = useCallback(async () => {
    try {
      const next = await activityApi.overview();
      if (!alive.current) return;
      // Same rows: keep the object, so a quiet poll renders nothing.
      setOverview(previous => (sameActivity(previous, next) ? previous : next));
      setFailed(false);
    } catch {
      if (alive.current) setFailed(true);
    }
  }, []);

  const busy = isAnythingRunning(overview);

  // One read on mount, and again whenever the tab comes back to the front.
  useEffect(() => {
    alive.current = true;
    void refetch();
    const onVisibilityChange = () => {
      if (document.visibilityState === 'visible') void refetch();
    };
    document.addEventListener('visibilitychange', onVisibilityChange);
    return () => {
      alive.current = false;
      document.removeEventListener('visibilitychange', onVisibilityChange);
    };
  }, [refetch]);

  // The pace changes with `busy`; re-arming the timer must not read again.
  useEffect(() => {
    const interval = setInterval(() => { void refetch(); }, busy ? ACTIVITY_POLL_BUSY_MS : ACTIVITY_POLL_IDLE_MS);
    return () => clearInterval(interval);
  }, [refetch, busy]);

  return { overview, failed, refetch };
}
