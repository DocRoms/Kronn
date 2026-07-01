// P2 (2026-07) — inline "restore the encryption key" flow, shown inside the
// plugins "not operational" banner. When secrets are unreadable because the
// encryption key changed (the 2026-06-30 incident), the user can restore the
// original key from their recovery passphrase (+ the saved recovery code when
// the local sidecar is gone too). Collapsed to a single CTA by default —
// re-entering each token by hand remains the other path.
import { useState } from 'react';
import { config as configApi } from '../lib/api';
import type { ToastFn } from '../hooks/useToast';
import { KeyRound } from 'lucide-react';

interface RecoveryRestorePanelProps {
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
  /** Called after a successful restore so the parent refetches plugin state. */
  onRestored: () => void;
}

export function RecoveryRestorePanel({ toast, t, onRestored }: RecoveryRestorePanelProps) {
  const [open, setOpen] = useState(false);
  const [passphrase, setPassphrase] = useState('');
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState(false);

  const restore = async () => {
    if (!passphrase || busy) return;
    setBusy(true);
    try {
      await configApi.restoreRecovery(passphrase, code.trim() || undefined);
      toast(t('mcp.recovery.restored'), 'success');
      setOpen(false);
      setPassphrase('');
      setCode('');
      onRestored();
    } catch (e) {
      // Backend messages are precise here (wrong passphrase / wrong instance /
      // no recovery data) — surface them verbatim.
      toast(e instanceof Error ? e.message : String(e), 'error');
    } finally {
      setBusy(false);
    }
  };

  if (!open) {
    return (
      <button
        type="button"
        className="mcp-warning-banner-item"
        data-testid="recovery-restore-cta"
        onClick={() => setOpen(true)}
      >
        <KeyRound size={13} /> <strong>{t('mcp.recovery.restoreCta')}</strong>
      </button>
    );
  }

  return (
    <div className="mcp-recovery-restore" data-testid="recovery-restore-panel">
      <p className="mcp-warning-banner-hint">{t('mcp.recovery.restoreHint')}</p>
      <input
        type="password"
        className="set-input"
        value={passphrase}
        autoComplete="current-password"
        placeholder={t('settings.recovery.passphrasePlaceholder')}
        onChange={e => setPassphrase(e.target.value)}
        data-testid="recovery-restore-passphrase"
      />
      <input
        type="text"
        className="set-input"
        value={code}
        placeholder={t('mcp.recovery.codePlaceholder')}
        onChange={e => setCode(e.target.value)}
        data-testid="recovery-restore-code"
      />
      <div className="flex-row gap-4">
        <button
          type="button"
          className="set-action-btn"
          disabled={!passphrase || busy}
          onClick={restore}
          data-testid="recovery-restore-submit"
        >
          {busy ? t('common.loading') : t('mcp.recovery.restoreBtn')}
        </button>
        <button type="button" className="btn-ghost" onClick={() => setOpen(false)}>
          {t('common.cancel')}
        </button>
      </div>
    </div>
  );
}
