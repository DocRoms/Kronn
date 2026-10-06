// P2 (2026-07) — inline "restore the encryption key" flow, shown inside the
// plugins "not operational" banner. When secrets are unreadable because the
// encryption key changed (the 2026-06-30 incident), the user can restore the
// original key from their recovery passphrase (+ the saved recovery code when
// the local sidecar is gone too). Collapsed to a single CTA by default —
// re-entering each token by hand remains the other path.
import { useEffect, useState } from 'react';
import { config as configApi } from '../lib/api';
import type { ToastFn } from '../hooks/useToast';
import { KeyRound } from 'lucide-react';

interface RecoveryRestorePanelProps {
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
  /** Called after a successful restore so the parent refetches plugin state. */
  onRestored: () => void;
  /** `restore` forces the key-restore flow (locked instance); `auto` asks the
   *  backend: a running instance re-encrypts imported secrets instead (KT-1007). */
  mode?: 'auto' | 'restore';
  /** Start expanded (the auth-locked screen has nothing else to show). */
  initiallyOpen?: boolean;
  /** Rows waiting in locked-secrets files (a key given up), for the hint. */
  lockedFileRows?: number;
}

export function RecoveryRestorePanel({ toast, t, onRestored, mode = 'auto', initiallyOpen = false, lockedFileRows = 0 }: RecoveryRestorePanelProps) {
  const [open, setOpen] = useState(initiallyOpen);
  // Restoring another key on a running instance would split the data between
  // two keys; there the imported secrets are re-encrypted under the live key.
  const [reencrypt, setReencrypt] = useState(false);
  // In auto mode nothing is submitted before the backend says which flow applies.
  const [modeKnown, setModeKnown] = useState(mode !== 'auto');
  useEffect(() => {
    if (mode !== 'auto') return;
    let live = true;
    Promise.resolve()
      .then(() => configApi.getRecoveryStatus())
      .then(status => { if (live) setReencrypt(!status.key_locked); })
      .catch(() => { if (live) setReencrypt(false); })
      .finally(() => { if (live) setModeKnown(true); });
    return () => { live = false; };
  }, [mode]);
  const [passphrase, setPassphrase] = useState('');
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState(false);

  const restore = async () => {
    if (!passphrase || busy || !modeKnown) return;
    setBusy(true);
    try {
      if (reencrypt) {
        const res = await configApi.reencryptImported(passphrase, code.trim() || undefined);
        // Rows put back from locked-secrets files count too.
        toast(t('mcp.recovery.reencrypted', res.rewritten + (res.restored_from_files ?? 0)), 'success');
        const waiting = lockedFileRows - (res.restored_from_files ?? 0);
        if (lockedFileRows > 0 && waiting > 0) toast(t('mcp.recovery.stillWaiting', waiting), 'error');
      } else {
        await configApi.restoreRecovery(passphrase, code.trim() || undefined);
        toast(t('mcp.recovery.restored'), 'success');
      }
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
        <KeyRound size={13} /> <strong>{t(reencrypt ? 'mcp.recovery.reencryptCta' : 'mcp.recovery.restoreCta')}</strong>
      </button>
    );
  }

  return (
    <div className="mcp-recovery-restore" data-testid="recovery-restore-panel">
      <p className="mcp-warning-banner-hint">{t(reencrypt ? 'mcp.recovery.reencryptHint' : 'mcp.recovery.restoreHint')}</p>
      {reencrypt && lockedFileRows > 0 && (
        <p className="mcp-warning-banner-hint" data-testid="recovery-locked-files-hint">
          {t('mcp.recovery.lockedFilesHint', lockedFileRows)}
        </p>
      )}
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
          disabled={!passphrase || busy || !modeKnown}
          onClick={restore}
          data-testid="recovery-restore-submit"
        >
          {busy ? t('common.loading') : t(reencrypt ? 'mcp.recovery.reencryptBtn' : 'mcp.recovery.restoreBtn')}
        </button>
        <button type="button" className="btn btn-ghost" onClick={() => setOpen(false)}>
          {t('common.cancel')}
        </button>
      </div>
    </div>
  );
}
