// KT-1007 — app-wide notice when no key decrypts the stored secrets while the
// API stays usable (auth off, or an operator session token). Restoring is
// offered right here; the Settings section offers the same.
import { useEffect, useState } from 'react';
import { useT } from '../lib/I18nContext';
import { config as configApi } from '../lib/api';
import type { ToastFn } from '../hooks/useToast';
import { RecoveryRestorePanel } from './RecoveryRestorePanel';
import { AlertTriangle } from 'lucide-react';

export function KeyLockedBanner() {
  const { t } = useT();
  const [locked, setLocked] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const toast: ToastFn = text => setMessage(text);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => configApi.getRecoveryStatus())
      .then(s => { if (live) setLocked(s.key_locked); })
      .catch(() => {});
    return () => { live = false; };
  }, []);

  if (!locked) return null;
  return (
    <div className="set-expose-warn" role="alert" data-testid="key-locked-banner">
      <AlertTriangle size={13} />
      <span>{t('keyLocked.banner')}</span>
      <RecoveryRestorePanel toast={toast} t={t} mode="restore" onRestored={() => window.location.reload()} />
      {message && <span className="set-hint-xs">{message}</span>}
    </div>
  );
}
