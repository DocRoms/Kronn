// KT-1007 — app-wide key notices while the API stays usable (auth off, or an
// operator session token): no key decrypts the stored secrets (restore here),
// the stored credentials failed to load, or config.toml was set aside.
import { useEffect, useState } from 'react';
import { useT } from '../lib/I18nContext';
import { config as configApi } from '../lib/api';
import type { ToastFn } from '../hooks/useToast';
import { RecoveryRestorePanel } from './RecoveryRestorePanel';
import { StartNewKeyPanel } from './StartNewKeyPanel';
import { AlertTriangle } from 'lucide-react';
import type { RecoveryStatus } from '../types/generated';

export function KeyLockedBanner() {
  const { t } = useT();
  const [status, setStatus] = useState<RecoveryStatus | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [setAsideDismissed, setSetAsideDismissed] = useState(false);
  const toast: ToastFn = text => setMessage(text);

  useEffect(() => {
    let live = true;
    Promise.resolve()
      .then(() => configApi.getRecoveryStatus())
      .then(s => { if (live) setStatus(s); })
      .catch(() => {});
    return () => { live = false; };
  }, []);

  if (!status) return null;
  return (
    <>
      {status.key_locked && (
        <div className="set-expose-warn" role="alert" data-testid="key-locked-banner">
          <AlertTriangle size={13} />
          <span>{t('keyLocked.banner')}</span>
          <RecoveryRestorePanel toast={toast} t={t} mode="restore" onRestored={() => window.location.reload()} />
          <StartNewKeyPanel t={t} onDone={() => window.location.reload()} />
          {message && <span className="set-hint-xs">{message}</span>}
        </div>
      )}
      {!status.key_locked && status.credentials_unavailable && (
        <div className="set-expose-warn" role="alert" data-testid="credentials-unavailable-banner">
          <AlertTriangle size={13} />
          <span>{t('keyLocked.credentialsUnavailable', status.credentials_unavailable)}</span>
        </div>
      )}
      {status.config_set_aside && !setAsideDismissed && (
        <div className="set-expose-warn" role="status" data-testid="config-set-aside-banner">
          <AlertTriangle size={13} />
          <span>{status.config_set_aside}</span>
          <button type="button" className="btn btn-ghost" aria-label={t('common.close')} onClick={() => setSetAsideDismissed(true)}>×</button>
        </div>
      )}
    </>
  );
}
