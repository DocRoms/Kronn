import { useState, useEffect, lazy, Suspense } from 'react';
import { setup as setupApi } from './lib/api';
import type { SetupStatus } from './types/generated';
import { ErrorBoundary } from './components/ErrorBoundary';
import './App.css';

const SetupWizard = lazy(() => import('./pages/SetupWizard').then(m => ({ default: m.SetupWizard })));
const Dashboard = lazy(() => import('./pages/Dashboard').then(m => ({ default: m.Dashboard })));

export function App() {
  const [setupStatus, setSetupStatus] = useState<SetupStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [apiError, setApiError] = useState(false);

  const fetchStatus = () => {
    setLoading(true);
    setApiError(false);
    setupApi.getStatus()
      .then((status) => { setSetupStatus(status); setApiError(false); })
      .catch(() => {
        setSetupStatus(null);
        setApiError(true);
      })
      .finally(() => setLoading(false));
  };

  useEffect(() => { fetchStatus(); }, []);

  if (loading) {
    return <LoadingScreen />;
  }

  // API unreachable — show error screen with retry, NOT the wizard
  if (apiError) {
    return <ApiErrorScreen onRetry={fetchStatus} />;
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
