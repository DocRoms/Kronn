import { useState, useEffect, useRef, useCallback, useMemo, lazy, Suspense } from 'react';
import { Navigate, Outlet, useLocation } from 'react-router';
import { setup as setupApi, config as configApi, health as healthApi } from './lib/api';
import {
  RETRY_DELAY,
  STATUS_TIMEOUT_MS,
  cacheSetupStatus,
  clearCachedSetupStatus,
  readCachedSetupStatus,
  withTimeout,
} from './lib/appBoot';
import type { SetupStatus } from './types/generated';
import { ErrorBoundary } from './components/ErrorBoundary';
import { LoadingState } from './components/LoadingState';
import { AuthLockedScreen } from './components/AuthLockedScreen';
import { ApiRequestError } from './lib/apiRequestError';
import { armBootScreen } from './lib/bootScreen';
import { legacyHashToPath } from './lib/legacyRoutes';
import type { AppOutletContext } from './lib/appContext';
import './App.css';

const SetupWizard = lazy(() => import('./pages/SetupWizard').then(m => ({ default: m.SetupWizard })));

/**
 * The root route. It gates on setup, then hands over to its outlet: the
 * dashboard shell or a whole-window view, whichever the address names
 * (`routes/appRoutes.tsx`).
 */
export function App() {
  const { hash } = useLocation();
  const [setupStatus, setSetupStatus] = useState<SetupStatus | null>(readCachedSetupStatus);
  const setupStatusRef = useRef<SetupStatus | null>(setupStatus);
  const [loading, setLoading] = useState(setupStatus === null);
  // The backend has not answered for a while: still the loader, plus a calm
  // note and a retry. A restart with migrations can outlast the quick retries.
  const [slowStart, setSlowStart] = useState(false);
  // The API refuses everything but key recovery (stored auth token unreadable).
  const [authLocked, setAuthLocked] = useState(false);
  // A failed reset is said on screen (the backend names what was not done).
  const [resetError, setResetError] = useState<string | null>(null);
  // Under Docker, agent installs land in the container (not the host) → the
  // wizard disables Install and points to the host `kronn` CLI. Default false
  // (native/Tauri) until health resolves; a failed probe leaves it false.
  const [inDocker, setInDocker] = useState(false);
  const retries = useRef(0);
  // Retries never give up while the backend is down; they stop with the app.
  const mounted = useRef(true);

  const applySetupStatus = useCallback((status: SetupStatus) => {
    setupStatusRef.current = status;
    cacheSetupStatus(status);
    setSetupStatus(status);
    setSlowStart(false);
    setLoading(false);
  }, []);

  const fetchStatus = useCallback(function fetchSetupStatus(resetRetries = false) {
    if (resetRetries) retries.current = 0;
    if (!setupStatusRef.current) setLoading(true);
    // CRITICAL: time out the request. `getStatus` can HANG (not reject) when
    // the backend is slow — e.g. agent detection contends under concurrent-
    // agent load — and a hung promise fires neither .then nor .catch, so the
    // boot stays on "Almost ready…" forever (the retry logic only triggers on
    // rejection). A timeout converts a hang into a retry.
    withTimeout(setupApi.getStatus(), STATUS_TIMEOUT_MS)
      .then((status) => {
        retries.current = 0;
        applySetupStatus(status);
      })
      .catch((error: unknown) => {
        if (error instanceof ApiRequestError && error.code === 'auth_locked') {
          setAuthLocked(true);
          setLoading(false);
          return;
        }
        // Auto-retry up to 5 times with 2s delay (backend may still be starting)
        if (retries.current < 5) {
          retries.current += 1;
          setTimeout(() => { if (mounted.current) fetchSetupStatus(); }, RETRY_DELAY);
          return;
        }
        // A returning tab can keep rendering the last known setup state while
        // the backend recovers. The successful refresh will replace it later.
        if (setupStatusRef.current) {
          setLoading(false);
          return;
        }
        // Retries exhausted. Distinguish "backend slow" from "backend down":
        // if a fast endpoint answers, the backend IS up — setup/status is just
        // wedged — so proceed optimistically as a returning (non-first-run)
        // user instead of holding the whole app hostage to one slow probe.
        // An unreachable backend keeps the loader and keeps retrying.
        withTimeout(configApi.getLanguage(), 4000)
          .then(() => {
            console.warn('setup/status timed out but backend is reachable — proceeding optimistically.');
            const optimisticStatus: SetupStatus = {
              is_first_run: false,
              current_step: 'Complete',
              agents_detected: [],
              scan_paths_set: true,
              scan_paths_explored: [], config_set_aside: null,
              repos_detected: [],
              default_scan_path: null,
            };
            setupStatusRef.current = optimisticStatus;
            setSetupStatus(optimisticStatus);
            setSlowStart(false);
            setLoading(false);
          })
          .catch(() => {
            // Unreachable: keep the loader and keep trying until it answers.
            if (mounted.current) setSlowStart(true);
            setTimeout(() => { if (mounted.current) fetchSetupStatus(); }, RETRY_DELAY);
          });
      });
  }, [applySetupStatus]);

  // After the first commit, every loader that wants the start-up screen holds
  // it; if none does, the app is ready.
  useEffect(() => { armBootScreen(); }, []);

  useEffect(() => {
    mounted.current = true;
    fetchStatus();
    return () => { mounted.current = false; };
  }, [fetchStatus]);
  useEffect(() => { healthApi.get().then(h => setInDocker(h.in_docker)).catch(() => {}); }, []);

  // Intercept external link clicks in Tauri desktop only.
  // Tauri webview doesn't handle target="_blank" — we call /api/open-url
  // which uses the `open` crate to launch the system browser.
  // In normal browser mode, links work natively — no interception needed.
  useEffect(() => {
    // Detect Tauri: the backend sets a response header or we check the port pattern.
    // Simplest: Tauri loads from 127.0.0.1 with a random port, Docker/dev uses fixed ports.
    const isTauri = window.location.hostname === '127.0.0.1'
      && window.location.port !== '5173'   // not Vite dev
      && window.location.port !== '3456';  // not Docker gateway
    if (!isTauri) return;

    const handler = (e: MouseEvent) => {
      const anchor = (e.target as HTMLElement).closest('a');
      if (!anchor) return;
      const href = anchor.getAttribute('href');
      if (!href || href.startsWith('#') || href.startsWith('/')) return;
      if (href.startsWith('http://') || href.startsWith('https://')) {
        e.preventDefault();
        fetch('/api/open-url', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ url: href }),
        }).catch(e => console.warn('open-url failed:', e));
      }
    };
    document.addEventListener('click', handler);
    return () => document.removeEventListener('click', handler);
  }, []);

  const dismissResetError = useCallback(() => setResetError(null), []);
  const resetSetup = useCallback(() => {
    clearCachedSetupStatus();
    setupStatusRef.current = null;
    setupApi.reset().then(() => {
      setSetupStatus(null);
      setLoading(true);
      setupApi.getStatus().then(applySetupStatus).finally(() => setLoading(false));
    }).catch(e => {
      // A failed reset is said on screen (the backend names what was not done).
      setResetError(e instanceof Error ? e.message : String(e));
      fetchStatus(true);
    });
  }, [applySetupStatus, fetchStatus]);
  const outletContext = useMemo<AppOutletContext>(
    () => ({ resetSetup, resetError, dismissResetError }),
    [resetSetup, resetError, dismissResetError],
  );

  if (authLocked) {
    return <AuthLockedScreen onRestored={() => window.location.reload()} />;
  }

  // A deep link from before pages had addresses: send it to the address
  // that replaced it, whatever else is going on.
  const legacyPath = legacyHashToPath(hash);
  if (legacyPath) return <Navigate to={legacyPath} replace />;

  if (loading) {
    return <LoadingState fullscreen phase={slowStart ? 'slow' : 'connecting'} onRetry={slowStart ? () => fetchStatus(true) : undefined} />;
  }

  // First run or setup incomplete → show wizard
  if (!setupStatus || setupStatus.is_first_run || setupStatus.current_step !== 'Complete') {
    return (
      <ErrorBoundary>
        <Suspense fallback={<LoadingState fullscreen />}>
          <SetupWizard
            initialStatus={setupStatus}
            inDocker={inDocker}
            onComplete={() => {
              // Re-fetch status to get fresh state with is_first_run=false
              setupApi.getStatus().then(applySetupStatus).catch(e => console.warn('Setup status refresh failed:', e));
            }}
          />
        </Suspense>
      </ErrorBoundary>
    );
  }

  // Setup complete → whatever the address names, with the means to start over.
  return <Outlet context={outletContext} />;
}
