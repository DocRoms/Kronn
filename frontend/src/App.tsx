import { useState, useEffect, useRef, lazy, Suspense } from 'react';
import { setup as setupApi, config as configApi, health as healthApi } from './lib/api';
import type { SetupStatus } from './types/generated';
import { ErrorBoundary } from './components/ErrorBoundary';
import { UpdateBanner } from './components/UpdateBanner';
import { BackendStatus } from './components/BackendStatus';
import './App.css';

const SetupWizard = lazy(() => import('./pages/SetupWizard').then(m => ({ default: m.SetupWizard })));
const Dashboard = lazy(() => import('./pages/Dashboard').then(m => ({ default: m.Dashboard })));

/** Auto-retry delay (ms) for backend connection. Exported for test override. */
export let RETRY_DELAY = 2000;
export function setRetryDelay(ms: number) { RETRY_DELAY = ms; }

// Per-request boot timeout. Kept short so a slow setup/status can't wedge the
// boot — exported so tests can shrink it.
export let STATUS_TIMEOUT_MS = 8000;
export function setStatusTimeout(ms: number) { STATUS_TIMEOUT_MS = ms; }

/** Reject if `p` doesn't settle within `ms`. Lets the boot treat a HANGING
 *  request (which never rejects on its own) like a failure it can retry. */
export function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  return Promise.race([
    p,
    new Promise<T>((_, reject) => setTimeout(() => reject(new Error('timeout')), ms)),
  ]);
}

export function App() {
  const [setupStatus, setSetupStatus] = useState<SetupStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [apiError, setApiError] = useState(false);
  // Under Docker, agent installs land in the container (not the host) → the
  // wizard disables Install and points to the host `kronn` CLI. Default false
  // (native/Tauri) until health resolves; a failed probe leaves it false.
  const [inDocker, setInDocker] = useState(false);
  const retries = useRef(0);

  const fetchStatus = (resetRetries = false) => {
    if (resetRetries) retries.current = 0;
    setLoading(true);
    setApiError(false);
    // CRITICAL: time out the request. `getStatus` can HANG (not reject) when
    // the backend is slow — e.g. agent detection contends under concurrent-
    // agent load — and a hung promise fires neither .then nor .catch, so the
    // boot stays on "Almost ready…" forever (the retry logic only triggers on
    // rejection). A timeout converts a hang into a retry.
    withTimeout(setupApi.getStatus(), STATUS_TIMEOUT_MS)
      .then((status) => {
        retries.current = 0;
        setSetupStatus(status);
        setLoading(false);
      })
      .catch(() => {
        // Auto-retry up to 5 times with 2s delay (backend may still be starting)
        if (retries.current < 5) {
          retries.current += 1;
          setTimeout(fetchStatus, RETRY_DELAY);
          return;
        }
        // Retries exhausted. Distinguish "backend slow" from "backend down":
        // if a fast endpoint answers, the backend IS up — setup/status is just
        // wedged — so proceed optimistically as a returning (non-first-run)
        // user instead of holding the whole app hostage to one slow probe.
        // Only a genuinely unreachable backend shows the error screen.
        withTimeout(configApi.getLanguage(), 4000)
          .then(() => {
            console.warn('setup/status timed out but backend is reachable — proceeding optimistically.');
            setSetupStatus({
              is_first_run: false,
              current_step: 'Complete',
              agents_detected: [],
              scan_paths_set: true,
              repos_detected: [],
              default_scan_path: null,
            });
            setApiError(false);
            setLoading(false);
          })
          .catch(() => {
            setSetupStatus(null);
            setApiError(true);
            setLoading(false);
          });
      });
  };

  useEffect(() => { fetchStatus(); }, []);
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

  if (loading) {
    return <LoadingScreen />;
  }

  // API unreachable — show error screen with retry, NOT the wizard
  if (apiError) {
    return <ApiErrorScreen onRetry={() => fetchStatus(true)} />;
  }

  // First run or setup incomplete → show wizard
  if (!setupStatus || setupStatus.is_first_run || setupStatus.current_step !== 'Complete') {
    return (
      <ErrorBoundary>
        <Suspense fallback={<LoadingScreen />}>
          <SetupWizard
            initialStatus={setupStatus}
            inDocker={inDocker}
            onComplete={() => {
              // Re-fetch status to get fresh state with is_first_run=false
              setupApi.getStatus().then(setSetupStatus).catch(e => console.warn('Setup status refresh failed:', e));
            }}
          />
        </Suspense>
      </ErrorBoundary>
    );
  }

  // Setup complete → show dashboard
  return (
    <ErrorBoundary>
      <Suspense fallback={<LoadingScreen />}>
        <UpdateBanner />
        <BackendStatus />
        <Dashboard onReset={() => {
        setupApi.reset().then(() => {
          setSetupStatus(null);
          setLoading(true);
          setupApi.getStatus().then(setSetupStatus).finally(() => setLoading(false));
        }).catch(e => console.warn('Setup reset failed:', e));
      }} />
      </Suspense>
    </ErrorBoundary>
  );
}

function ApiErrorScreen({ onRetry }: { onRetry: () => void }) {
  return (
    <div className="app-fullscreen">
      <div className="app-error-icon">!</div>
      <span className="app-error-title">
        Cannot connect to backend
      </span>
      <span className="app-error-desc">
        The API server is unreachable. Check that the backend is running and try again.
      </span>
      <button onClick={onRetry} className="app-retry-btn">
        Retry
      </button>
      <style>{`@keyframes pulse { 0%, 100% { opacity: 1 } 50% { opacity: 0.3 } }
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
  }
}`}</style>
    </div>
  );
}

function LoadingScreen() {
  // Cycle through progress hints every 1.5 s so the user knows the boot
  // is alive even when first-load takes 4-5 s (Vite cold compile + lazy
  // chunks + setup-status round trip). Pre-fix the user just saw
  // "Entering the grid…" frozen for 5 s and assumed the app had hung
  // — Alicia's audit on 2026-05-09 specifically called that out.
  // We don't translate this string set: the boot screen renders BEFORE
  // I18nProvider mounts (it lives outside the Suspense for that very
  // provider), so calling `useT()` here would crash with "useT must be
  // used within I18nProvider". The hints below are written so they're
  // self-explanatory regardless of locale.
  const hints = [
    'Entering the grid…',
    'Loading config…',
    'Detecting agents…',
    'Almost ready…',
  ];
  const [hintIdx, setHintIdx] = useState(0);
  useEffect(() => {
    const id = setInterval(() => {
      setHintIdx(prev => Math.min(prev + 1, hints.length - 1));
    }, 1500);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <div className="app-fullscreen">
      <div className="app-spinner" />
      <span className="app-loading-text">
        {hints[hintIdx]}
      </span>
      {/* Keyframes (spin, pulse, reduced-motion) defined in index.html */}
    </div>
  );
}
