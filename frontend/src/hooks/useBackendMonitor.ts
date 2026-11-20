import { useEffect } from 'react';
import { fetchHealth } from '../lib/api';
import { onBackendSuspect, reportBackendRecovered, setBackendHealth } from '../lib/backendReachability';
import { confirmOutage, noteBackendAnswered } from '../lib/notifications';

/**
 * The one poller of `/api/health` (the BackendStatus pill until KT-1684: the
 * outage is now an entry of the bell's Notifications tab, so nothing is drawn
 * here).
 *
 * While the backend answers it polls every ~30 s. When it stops — usually a
 * restart — the outage is confirmed in the notification store at once: any API
 * call that fails the way a stopped backend fails triggers a check instead of
 * waiting for the healthy poll. While down, it polls every 2 s; on the way back
 * up it resolves the outage and announces the recovery so loads that failed
 * meanwhile retry (`useApi`), and it keeps the shared health state the
 * start-up screen reads.
 *
 * Start-up itself is covered by the single loading screen (`bootScreen.ts`);
 * this covers a backend lost mid-session, on every page.
 */
const HEALTHY_POLL_INTERVAL_MS = 30_000;
const UNHEALTHY_POLL_INTERVAL_MS = 2_000;
const POLL_JITTER_MS = 5_000; // randomise to avoid thundering herd on shared hosts

/** Starts a check now, outside the poll: the bell's Retry button. */
export const BACKEND_CHECK_NOW_EVENT = 'kronn:backend-check-now';

export function useBackendMonitor(): void {
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let checking = false;
    let wasDown = false;

    const check = async () => {
      if (checking || cancelled) return;
      checking = true;
      let healthy = false;
      try {
        await fetchHealth();
        healthy = true;
        setBackendHealth('up');
        noteBackendAnswered();
        if (wasDown) {
          wasDown = false;
          reportBackendRecovered();
        }
      } catch {
        // Any failure (network, 5xx, JSON parse) → the backend is down; the
        // next tick tries again and the first success resolves the outage.
        wasDown = true;
        setBackendHealth('down');
        confirmOutage();
      } finally {
        checking = false;
        if (!cancelled) {
          const delay = healthy
            ? HEALTHY_POLL_INTERVAL_MS + Math.random() * POLL_JITTER_MS
            : UNHEALTHY_POLL_INTERVAL_MS;
          timer = setTimeout(check, delay);
        }
      }
    };

    const checkNow = () => {
      if (timer) clearTimeout(timer);
      void check();
    };

    const checkWhenVisible = () => {
      if (document.visibilityState === 'visible') checkNow();
    };

    // First check fires immediately so a crash that happened just before the
    // user navigated shows up.
    void check();
    // A failed API call checks at once: a restart shorter than the healthy
    // poll interval would otherwise leave pages empty with no explanation.
    const stopSuspect = onBackendSuspect(checkNow);
    window.addEventListener(BACKEND_CHECK_NOW_EVENT, checkNow);
    window.addEventListener('online', checkNow);
    document.addEventListener('visibilitychange', checkWhenVisible);
    return () => {
      cancelled = true;
      stopSuspect();
      if (timer) clearTimeout(timer);
      window.removeEventListener(BACKEND_CHECK_NOW_EVENT, checkNow);
      window.removeEventListener('online', checkNow);
      document.removeEventListener('visibilitychange', checkWhenVisible);
    };
  }, []);
}
