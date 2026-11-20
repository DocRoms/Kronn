import { useBackendMonitor } from '../hooks/useBackendMonitor';
import { useSystemNotices } from '../hooks/useSystemNotices';
import { useAppContext } from '../lib/appContext';
import { Dashboard } from '../pages/Dashboard';

/** The dashboard shell. Nothing sits above it: what the install has to say
 *  goes to the bell (KT-1686). */
export function DashboardLayout() {
  const { resetSetup, resetError, dismissResetError } = useAppContext();
  useBackendMonitor();
  useSystemNotices(resetError, dismissResetError);
  return <Dashboard onReset={resetSetup} />;
}
