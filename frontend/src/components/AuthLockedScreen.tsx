// KT-1007 — shown when the API answers `auth_locked`: a stored auth token
// exists but the encryption key that opens it is missing. Every route but the
// recovery ones is refused, so this screen is the only way back: restore the
// key with the recovery passphrase, then reload.
import { useState } from 'react';
import { useT } from '../lib/I18nContext';
import type { ToastFn } from '../hooks/useToast';
import { RecoveryRestorePanel } from './RecoveryRestorePanel';
import { KeyRound } from 'lucide-react';

interface AuthLockedScreenProps {
  /** Called after a successful restore (the app reloads). */
  onRestored: () => void;
}

export function AuthLockedScreen({ onRestored }: AuthLockedScreenProps) {
  const { t } = useT();
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);
  const toast: ToastFn = (text, type = 'info') => setMessage({ text, error: type === 'error' });

  return (
    <main className="bootstrap-error" role="alert" data-testid="auth-locked-screen">
      <h1>
        <KeyRound size={18} /> {t('authLocked.title')}
      </h1>
      <p>{t('authLocked.hint')}</p>
      <RecoveryRestorePanel toast={toast} t={t} onRestored={onRestored} mode="restore" initiallyOpen />
      {message && (
        <p className={message.error ? 'text-error' : 'set-hint'} data-testid="auth-locked-message">
          {message.text}
        </p>
      )}
    </main>
  );
}
