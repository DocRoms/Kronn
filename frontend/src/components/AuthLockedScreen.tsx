// KT-1007 — shown when the API answers `auth_locked`: a stored auth token
// exists but cannot be decrypted. Every route but the recovery ones is refused
// (and those only to a caller on this machine), so this screen is the way back:
// restore the key, or — when the key is fine and only the token row is
// unreadable — set a new token.
import { useEffect, useState } from 'react';
import { useT } from '../lib/I18nContext';
import { config as configApi } from '../lib/api';
import type { ToastFn } from '../hooks/useToast';
import { RecoveryRestorePanel } from './RecoveryRestorePanel';
import { KeyRound } from 'lucide-react';

interface AuthLockedScreenProps {
  /** Called after a successful restore or a new token (the app reloads). */
  onRestored: () => void;
}

type LockState = 'checking' | 'remote' | 'keyLost' | 'tokenUnreadable';

export function AuthLockedScreen({ onRestored }: AuthLockedScreenProps) {
  const { t } = useT();
  const [state, setState] = useState<LockState>('checking');
  const [newToken, setNewToken] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const toast: ToastFn = (text, type = 'info') => setMessage({ text, error: type === 'error' });

  useEffect(() => {
    let live = true;
    // Only a caller on this machine may read the status: a refusal means remote.
    Promise.resolve()
      .then(() => configApi.getRecoveryStatus())
      .then(s => { if (live) setState(s.key_locked ? 'keyLost' : 'tokenUnreadable'); })
      .catch(() => { if (live) setState('remote'); });
    return () => { live = false; };
  }, []);

  const replaceToken = async () => {
    if (busy) return;
    setBusy(true);
    try {
      setNewToken(await configApi.regenerateAuthToken());
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), 'error');
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="bootstrap-error" role="alert" data-testid="auth-locked-screen">
      <h1>
        <KeyRound size={18} /> {t('authLocked.title')}
      </h1>
      {state === 'remote' && <p data-testid="auth-locked-remote">{t('authLocked.remoteHint')}</p>}
      {state === 'keyLost' && (
        <>
          <p>{t('authLocked.hint')}</p>
          <RecoveryRestorePanel toast={toast} t={t} onRestored={onRestored} mode="restore" initiallyOpen />
        </>
      )}
      {state === 'tokenUnreadable' && (
        <div data-testid="auth-locked-token">
          <p>{t('authLocked.tokenHint')}</p>
          {newToken ? (
            <>
              <p>{t('authLocked.newTokenDone')}</p>
              <code className="set-recovery-code" data-testid="auth-locked-new-token">{newToken}</code>
              <button type="button" className="btn btn-primary" onClick={onRestored}>
                {t('authLocked.continue')}
              </button>
            </>
          ) : (
            <button
              type="button"
              className="btn btn-primary"
              disabled={busy}
              onClick={replaceToken}
              data-testid="auth-locked-new-token-btn"
            >
              {busy ? t('common.loading') : t('authLocked.newToken')}
            </button>
          )}
        </div>
      )}
      {message && (
        <p className={message.error ? 'text-error' : 'set-hint'} data-testid="auth-locked-message">
          {message.text}
        </p>
      )}
    </main>
  );
}
