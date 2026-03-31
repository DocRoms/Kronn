import { useState, useEffect, useRef, lazy, Suspense } from 'react';
import { setup as setupApi } from './lib/api';
import type { SetupStatus } from './types/generated';
import { ErrorBoundary } from './components/ErrorBoundary';
import './App.css';

const SetupWizard = lazy(() => import('./pages/SetupWizard').then(m => ({ default: m.SetupWizard })));
const Dashboard = lazy(() => import('./pages/Dashboard').then(m => ({ default: m.Dashboard })));

/** Auto-retry delay (ms) for backend connection. Exported for test override. */
export let RETRY_DELAY = 2000;
export function setRetryDelay(ms: number) { RETRY_DELAY = ms; }

export function App() {
  const [setupStatus, setSetupStatus] = useState<SetupStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [apiError, setApiError] = useState(false);
  const retries = useRef(0);

  const fetchStatus = (resetRetries = false) => {
    if (resetRetries) retries.current = 0;
    setLoading(true);
    setApiError(false);
    setupApi.getStatus()
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
        } else {
          setSetupStatus(null);
          setApiError(true);
          setLoading(false);
        }
      });
  };

  useEffect(() => { fetchStatus(); }, []);

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
        }).catch(() => {});
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
            onComplete={() => {
              // Re-fetch status to get fresh state with is_first_run=false
              setupApi.getStatus().then(setSetupStatus).catch(() => {});
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
        <Dashboard onReset={() => {
        setupApi.reset().then(() => {
          setSetupStatus(null);
          setLoading(true);
          setupApi.getStatus().then(setSetupStatus).finally(() => setLoading(false));
        }).catch(() => {});
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
  return (
    <div className="app-fullscreen">
      <div className="app-spinner" />
      <span className="app-loading-text">
        Entering the grid...
      </span>
      {/* Keyframes (spin, pulse, reduced-motion) defined in index.html */}
    </div>
  );
}
