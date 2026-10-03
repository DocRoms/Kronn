import { useEffect, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { fetchHealth } from '../lib/api';
import { onBackendSuspect, reportBackendRecovered, setBackendHealth } from '../lib/backendReachability';
import { useT } from '../lib/I18nContext';

/**
 * Backend-health pill anchored next to the UpdateBanner, the only poller of
 * `/api/health`.
 *
 * Hidden while the backend answers. When it stops — usually a restart — a
 * "reconnecting" pill surfaces at once: any API call that fails the way a
 * stopped backend fails triggers a check instead of waiting for the healthy
 * 30 s poll. While down, polling every 2 s; on the way back up it announces
 * the recovery so loads that failed meanwhile retry (`useApi`), and it keeps
 * the shared health state the start-up screen reads.
 *
 * Start-up itself is covered by the single loading screen (`bootScreen.ts`);
 * this pill covers a backend lost mid-session, on every page, WebSocket or not.
 */
const HEALTHY_POLL_INTERVAL_MS = 30_000;
const UNHEALTHY_POLL_INTERVAL_MS = 2_000;
const POLL_JITTER_MS = 5_000; // randomise to avoid thundering herd on shared hosts

export function BackendStatus() {
  const { t } = useT();
  // `null` = haven't checked yet (first paint hides the pill)
  // `true` = healthy
  // `false` = unreachable
  const [healthy, setHealthy] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let checking = false;
    let wasDown = false;

    const check = async () => {
      if (checking || cancelled) return;
      checking = true;
      let nextHealthy = false;
      try {
        await fetchHealth();
        nextHealthy = true;
        if (!cancelled) setHealthy(true);
        setBackendHealth('up');
        if (wasDown) {
          wasDown = false;
          reportBackendRecovered();
        }
      } catch {
        wasDown = true;
        setBackendHealth('down');
        // Any failure (network, 5xx, JSON parse) → mark unhealthy.
        // The pill renders, the user notices, and on the next tick we
        // try again — when it succeeds, the pill auto-hides.
        if (!cancelled) setHealthy(false);
      } finally {
        checking = false;
        if (!cancelled) {
          const delay = nextHealthy
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

    // First check fires immediately so the pill surfaces a backend
    // crash that happened just before the user navigated.
    void check();
    // A failed API call checks at once: a restart shorter than the healthy
    // poll interval would otherwise leave pages empty with no explanation.
    const stopSuspect = onBackendSuspect(checkNow);
    window.addEventListener('online', checkNow);
    document.addEventListener('visibilitychange', checkWhenVisible);
    return () => {
      cancelled = true;
      stopSuspect();
      if (timer) clearTimeout(timer);
      window.removeEventListener('online', checkNow);
      document.removeEventListener('visibilitychange', checkWhenVisible);
    };
  }, []);

  // Hide while healthy (or while we haven't checked yet) — zero
  // chrome noise on the happy path.
  if (healthy !== false) return null;

  return (
    <div
      className="kronn-backend-status"
      role="status"
      aria-live="polite"
      title={t('app.backendOfflineTitle')}
    >
      <Loader2 size={12} className="kronn-backend-status-spinner" aria-hidden="true" />
      <span className="kronn-backend-status-text">
        {t('app.backendOffline')}
      </span>
    </div>
  );
}
