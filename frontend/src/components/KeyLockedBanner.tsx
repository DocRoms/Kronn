// KT-1007 — app-wide key notices while the API stays usable (auth off, or an
// operator session token): no key decrypts the stored secrets (restore here),
// the stored credentials failed to load, or config.toml was set aside. A key
// kept beside its data is no banner: it goes to the bell's notifications.
import { useEffect, useState } from 'react';
import { useT } from '../lib/I18nContext';
import { config as configApi } from '../lib/api';
import type { ToastFn } from '../hooks/useToast';
import { RecoveryRestorePanel } from './RecoveryRestorePanel';
import { StartNewKeyPanel } from './StartNewKeyPanel';
import { AlertTriangle } from 'lucide-react';
import type { RecoveryStatus } from '../types/generated';
import { settingsSectionPath } from '../lib/routes';
import { clearCredentialLabel } from '../lib/clearCredentials';
import { setNotices } from '../lib/notifications';

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
      .then(s => {
        if (!live) return;
        setStatus(s);
        setNotices('key', !s.key_locked && s.config_holds_key
          ? [{ id: 'beside-data', kind: 'key_beside_data', link: settingsSectionPath('settings-recovery'), category: 'kronn' }]
          : []);
      })
      .catch(() => {});
    return () => { live = false; };
  }, []);

  if (!status) return null;
  const clear = status.clear_credentials ?? [];
  return (
    <>
      {status.key_locked && (
        <div className="set-expose-warn" role="alert" data-testid="key-locked-banner">
          <AlertTriangle size={13} />
          <span>{t('keyLocked.banner')}</span>
          {clear.length > 0 && (
            <span data-testid="key-locked-clear-credentials">
              {t('keyLocked.clearCredentials', clear.map(c => clearCredentialLabel(c, t)).join(', '))}
            </span>
          )}
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
