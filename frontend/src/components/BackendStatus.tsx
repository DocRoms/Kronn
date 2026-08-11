import { useEffect, useState } from 'react';
import { fetchHealth } from '../lib/api';
import { useT } from '../lib/I18nContext';

/**
 * Persistent backend-health indicator anchored next to the UpdateBanner.
 *
 * Polls `/api/health` every 30s. While the backend answers, the pill
 * stays hidden (no chrome noise). When it stops answering, a red
 * "backend offline" pill surfaces so the user knows the next API
 * call won't reach the server — without waiting for an action to
 * fail. While offline, polling accelerates so a restarted backend is
 * detected quickly.
 *
 * # Why not just rely on `<ApiErrorScreen />`
 *
 * `ApiErrorScreen` covers the **boot** path — when `setupApi.getStatus()`
 * fails on first mount. After the dashboard mounts, a backend crash
 * mid-session goes unnoticed until the user clicks something. This
 * pill closes that gap with minimal noise (hidden when healthy).
 *
 * # Why not surface inside the existing reconnect banner
 *
 * The WebSocket reconnect already shows progress, but only in
 * pages that actively subscribe to WS. Settings, Workflows etc.
 * don't always have a live WS — a pure HTTP health check is the
 * superset.
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

    const check = async () => {
      if (checking || cancelled) return;
      checking = true;
      let nextHealthy = false;
      try {
        await fetchHealth();
        nextHealthy = true;
        if (!cancelled) setHealthy(true);
      } catch {
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
    window.addEventListener('online', checkNow);
    document.addEventListener('visibilitychange', checkWhenVisible);
    return () => {
      cancelled = true;
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
      <span className="kronn-backend-status-dot" aria-hidden="true" />
      <span className="kronn-backend-status-text">
        {t('app.backendOffline')}
      </span>
    </div>
  );
}
