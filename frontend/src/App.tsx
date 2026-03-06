import { useState, useEffect } from 'react';
import { setup as setupApi } from './lib/api';
import type { SetupStatus } from './types/generated';
import { SetupWizard } from './pages/SetupWizard';
import { Dashboard } from './pages/Dashboard';

export function App() {
  const [setupStatus, setSetupStatus] = useState<SetupStatus | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setupApi.getStatus()
      .then(setSetupStatus)
      .catch(() => {
        // If API is down, show setup anyway
        setSetupStatus(null);
      })
      .finally(() => setLoading(false));
  }, []);

  if (loading) {
    return <LoadingScreen />;
  }

  // First run or setup incomplete → show wizard
  if (!setupStatus || setupStatus.is_first_run || setupStatus.current_step !== 'Complete') {
    return (
      <SetupWizard
        initialStatus={setupStatus}
        onComplete={() => {
          // Re-fetch status to get fresh state with is_first_run=false
          setupApi.getStatus().then(setSetupStatus).catch(() => {});
        }}
      />
    );
  }

  // Setup complete → show dashboard
  return <Dashboard onReset={() => {
    setupApi.reset().then(() => {
      setSetupStatus(null);
      setLoading(true);
      setupApi.getStatus().then(setSetupStatus).finally(() => setLoading(false));
    }).catch(() => {});
  }} />;
}

function LoadingScreen() {
  return (
    <div style={{
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'center',
      height: '100vh',
      flexDirection: 'column',
      gap: 16,
    }}>
      <div style={{
        width: 32, height: 32,
        border: '3px solid rgba(200,255,0,0.2)',
        borderTopColor: '#c8ff00',
        borderRadius: '50%',
        animation: 'spin 0.8s linear infinite',
      }} />
      <span style={{ color: 'rgba(255,255,255,0.4)', fontSize: 13, fontFamily: 'JetBrains Mono, monospace' }}>
        Entering the grid...
      </span>
      <style>{`@keyframes spin { to { transform: rotate(360deg) } }`}</style>
    </div>
  );
}
