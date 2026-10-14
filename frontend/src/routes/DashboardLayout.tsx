import { BackendStatus } from '../components/BackendStatus';
import { KeyLockedBanner } from '../components/KeyLockedBanner';
import { UpdateBanner } from '../components/UpdateBanner';
import { useAppContext } from '../lib/appContext';
import { Dashboard } from '../pages/Dashboard';

/** The dashboard shell and the banners that only make sense around it. */
export function DashboardLayout() {
  const { resetSetup, resetError, dismissResetError } = useAppContext();
  return (
    <>
      <UpdateBanner />
      <BackendStatus />
      <KeyLockedBanner />
      {resetError && (
        <div className="set-expose-warn" role="alert" data-testid="reset-error">
          <span>{resetError}</span>
          <button type="button" className="btn btn-ghost" onClick={dismissResetError}>×</button>
        </div>
      )}
      <Dashboard onReset={resetSetup} />
    </>
  );
}
