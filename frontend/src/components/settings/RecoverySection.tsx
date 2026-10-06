// P2 (2026-07) — recovery passphrase for the MCP-secrets encryption key.
//
// The backend keeps the key in the OS keychain + a sidecar, but every copy
// lives ON the machine: an OS reinstall or a lost data dir takes them all
// (exactly the 2026-06-30 incident — 13 plugin tokens unrecoverable). This
// section lets the user wrap the key under a passphrase (Argon2id, backend
// /config/recovery/*) and hands back a RECOVERY CODE to store off-machine.
// Strongly-offered but non-blocking: a nudge when unconfigured, never a gate.
import { useState, useEffect } from 'react';
import { config as configApi } from '../../lib/api';
import { triggerDownload } from '../../lib/downloadBlob';
import type { ToastFn } from '../../hooks/useToast';
import { KeyRound, Copy, Download, ShieldCheck, AlertTriangle } from 'lucide-react';
import { RecoveryRestorePanel } from '../RecoveryRestorePanel';
import type { RecoveryStatus } from '../../types/generated';
import '../../pages/SettingsPage.css';

interface RecoverySectionProps {
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

// Mirrors the backend's MIN_RECOVERY_PASSPHRASE_LEN. Length over composition
// rules (NIST): 12+ chars — ideally a few words — is memorable AND puts offline
// brute-force on a leaked export out of practical reach under Argon2id.
const MIN_PASSPHRASE_LEN = 12;

export function RecoverySection({ toast, t }: RecoverySectionProps) {
  const [configured, setConfigured] = useState<boolean | null>(null);
  const [status, setStatus] = useState<RecoveryStatus | null>(null);
  const [passphrase, setPassphrase] = useState('');
  const [confirm, setConfirm] = useState('');
  // Replacing an existing passphrase must prove the current one (backend refuses otherwise).
  const [current, setCurrent] = useState('');
  // A recovery.key from before 0.14.3 may be replaced without its passphrase
  // once the user confirms (the backend keeps it; a restore still tries it).
  const [replaceUnverified, setReplaceUnverified] = useState(false);
  const [saving, setSaving] = useState(false);
  // The code is returned ONCE by /recovery/set — held only in component state,
  // never persisted UI-side.
  const [recoveryCode, setRecoveryCode] = useState<string | null>(null);

  useEffect(() => {
    configApi.getRecoveryStatus()
      .then(s => { setConfigured(s.configured); setStatus(s); })
      .catch(() => setConfigured(null));
  }, []);

  // A recovery.key for ANOTHER key, or a damaged one, is replaced without its
  // passphrase (the backend keeps it aside); a matching one needs it, and one
  // from before 0.14.3 needs it unless the user confirms.
  // Also a matching one whose passphrase is forgotten, once confirmed.
  const confirmable = !!status?.recovery_unverified || (!!configured && !!status?.matches_key);
  const confirmedUnverified = confirmable && replaceUnverified;
  const needsCurrent = !!configured && !status?.recovery_other_key && !status?.recovery_damaged
    && !confirmedUnverified;
  const canSave = passphrase.length >= MIN_PASSPHRASE_LEN && passphrase === confirm && !saving
    && (!needsCurrent || current.length > 0);

  const save = async () => {
    if (!canSave) return;
    setSaving(true);
    try {
      const res = needsCurrent
        ? await configApi.setRecovery(passphrase, current)
        : confirmedUnverified
          ? await configApi.setRecovery(passphrase, undefined, true)
          : await configApi.setRecovery(passphrase);
      setRecoveryCode(res.recovery_code);
      setConfigured(true);
      setStatus(s => (s ? { ...s, matches_key: true, recovery_other_key: false, recovery_unverified: false, recovery_damaged: false } : s));
      setReplaceUnverified(false);
      setPassphrase('');
      setConfirm('');
      setCurrent('');
      toast(t('settings.recovery.saved'), 'success');
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), 'error');
    } finally {
      setSaving(false);
    }
  };

  const copyCode = () => {
    if (!recoveryCode) return;
    navigator.clipboard.writeText(recoveryCode)
      .then(() => toast(t('settings.recovery.codeCopied'), 'success'))
      .catch(() => toast(t('settings.recovery.copyFailed'), 'error'));
  };

  const downloadCode = () => {
    if (!recoveryCode) return;
    triggerDownload('kronn-recovery-code.txt', new Blob([recoveryCode + '\n'], { type: 'text/plain' }));
  };

  return (
    <div id="settings-recovery" className="set-card">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <KeyRound size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('settings.recovery.title')}</span>
          {configured === true && status?.matches_key !== false && (
            <span className="set-hint-xs flex-row gap-3" data-testid="recovery-configured-badge">
              <ShieldCheck size={12} /> {t('settings.recovery.configured')}
            </span>
          )}
        </div>
        <p className="set-hint">{t('settings.recovery.hint')}</p>

        {/* KT-1007: a locked key is restored here; a blob for another key, or
            a key store holding another key, is said plainly. */}
        {status?.key_locked && (
          <div className="set-expose-warn" data-testid="recovery-locked">
            <AlertTriangle size={13} />
            <span>{t('settings.recovery.lockedHint')}</span>
          </div>
        )}
        {status?.key_locked && (
          <RecoveryRestorePanel toast={toast} t={t} mode="restore" initiallyOpen onRestored={() => window.location.reload()} />
        )}
        {status && !status.key_locked && status.configured && !status.matches_key && (
          <div className="set-expose-warn" data-testid="recovery-mismatch">
            <AlertTriangle size={13} />
            <span>{t(status.recovery_other_key ? 'settings.recovery.otherKeyWarning' : 'settings.recovery.mismatchWarning')}</span>
          </div>
        )}
        {status && (status.stale_sources ?? []).length > 0 && (
          <div className="set-expose-warn" data-testid="recovery-stale">
            <AlertTriangle size={13} />
            <span>{t('settings.recovery.staleWarning', (status.stale_sources ?? []).join(', '))}</span>
          </div>
        )}
        {status && (status.invalid_sources ?? []).length > 0 && (
          <div className="set-expose-warn" data-testid="recovery-invalid">
            <AlertTriangle size={13} />
            <span>{t('settings.recovery.invalidWarning', (status.invalid_sources ?? []).join(', '))}</span>
          </div>
        )}
        {status && !status.key_locked && !status.matches_key && status.copies < 2 && (
          <div className="set-expose-warn" data-testid="recovery-single-copy">
            <AlertTriangle size={13} />
            <span>{t('settings.recovery.singleCopyWarning', status.copies)}</span>
          </div>
        )}
        {status && (status.locked_credentials ?? 0) > 0 && (
          <div className="set-expose-warn" data-testid="recovery-locked-credentials">
            <AlertTriangle size={13} />
            <span>{t('settings.recovery.lockedCredentialsWarning', status.locked_credentials)}</span>
          </div>
        )}
        {status && !status.key_locked && status.credentials_unavailable && (
          <div className="set-expose-warn" data-testid="recovery-credentials-unavailable">
            <AlertTriangle size={13} />
            <span>{t('keyLocked.credentialsUnavailable', status.credentials_unavailable)}</span>
          </div>
        )}
        {status?.config_set_aside && (
          <div className="set-expose-warn" data-testid="recovery-config-set-aside">
            <AlertTriangle size={13} />
            <span>{status.config_set_aside}</span>
          </div>
        )}
        {status && (status.rows_moved_from_files ?? []).length > 0 && (
          <div className="set-hint" data-testid="recovery-rows-moved">
            {t('settings.recovery.rowsMoved', (status.rows_moved_from_files ?? []).join('; '))}
          </div>
        )}
        {status && (status.file_key_moves_pending ?? []).length > 0 && (
          <div className="set-hint" data-testid="recovery-moves-pending">
            {t('settings.recovery.movesPending', (status.file_key_moves_pending ?? []).join('; '))}
          </div>
        )}
        {/* Rows a passphrase or code can bring back: undecryptable ones the next
            start does not move by itself, or rows kept in a locked-secrets file. */}
        {status && !status.key_locked
          && ((status.undecryptable_rows ?? 0) > (status.file_key_rows_pending ?? 0)
            || (status.locked_file_rows ?? 0) > 0) && (
          <RecoveryRestorePanel
            toast={toast}
            t={t}
            onRestored={() => window.location.reload()}
            lockedFileRows={status.locked_file_rows ?? 0}
          />
        )}

        {configured === false && !recoveryCode && (
          <div className="set-expose-warn" data-testid="recovery-nudge">
            <AlertTriangle size={13} />
            <span>{t('settings.recovery.nudge')}</span>
          </div>
        )}

        {status?.key_locked ? null : recoveryCode ? (
          // One-time reveal of the code — the ONLY moment it's shown.
          <div data-testid="recovery-code-block">
            <div className="set-expose-warn">
              <AlertTriangle size={13} />
              <span>{t('settings.recovery.saveCodeWarning')}</span>
            </div>
            <code className="set-recovery-code" data-testid="recovery-code">{recoveryCode}</code>
            <div className="flex-row gap-8 mt-4">
              <button className="set-action-btn" onClick={copyCode}>
                <Copy size={13} /> {t('common.copy')}
              </button>
              <button className="set-action-btn" onClick={downloadCode}>
                <Download size={13} /> {t('settings.recovery.download')}
              </button>
              <button className="btn btn-ghost" onClick={() => setRecoveryCode(null)}>
                {t('settings.recovery.doneSaved')}
              </button>
            </div>
          </div>
        ) : (
          <div className="flex-col gap-2 mt-4">
            <label className="label">
              {configured ? t('settings.recovery.replaceLabel') : t('settings.recovery.setLabel')}
            </label>
            {/* Mnemonic suggestion only — deliberately NO what3words API call:
                sending the chosen place to a third party would leak the
                passphrase's source. The user visits the site themselves. The
                example link doubles as a format demo (dots + accents). */}
            <p className="set-hint-xs">
              {t('settings.recovery.mnemonicTip')}{' '}
              <a
                className="set-recovery-tip-link"
                href="https://what3words.com/%C3%A9chouer.ins%C3%A9rons.labeur"
                target="_blank"
                rel="noopener noreferrer"
              >
                what3words.com/échouer.insérons.labeur
              </a>
            </p>
            {configured && confirmable && (
              <label className="set-hint-xs flex-row gap-3">
                <input
                  type="checkbox"
                  checked={replaceUnverified}
                  onChange={e => setReplaceUnverified(e.target.checked)}
                  data-testid="recovery-replace-unverified"
                />
                {t(status?.recovery_unverified ? 'settings.recovery.replaceUnverified' : 'settings.recovery.forgotCurrent')}
              </label>
            )}
            {needsCurrent && (
              <input
                type="password"
                className="set-input"
                value={current}
                autoComplete="current-password"
                placeholder={t('settings.recovery.currentPlaceholder')}
                onChange={e => setCurrent(e.target.value)}
                data-testid="recovery-current"
              />
            )}
            <input
              type="password"
              className="set-input"
              value={passphrase}
              autoComplete="new-password"
              placeholder={t('settings.recovery.passphrasePlaceholder')}
              onChange={e => setPassphrase(e.target.value)}
              data-testid="recovery-passphrase"
            />
            <input
              type="password"
              className="set-input"
              value={confirm}
              autoComplete="new-password"
              placeholder={t('settings.recovery.confirmPlaceholder')}
              onChange={e => setConfirm(e.target.value)}
              data-testid="recovery-confirm"
            />
            {passphrase.length > 0 && passphrase.length < MIN_PASSPHRASE_LEN && (
              <p className="set-hint-xs">{t('settings.recovery.tooShort', MIN_PASSPHRASE_LEN)}</p>
            )}
            {confirm.length > 0 && confirm !== passphrase && (
              <p className="set-hint-xs">{t('settings.recovery.mismatch')}</p>
            )}
            <div className="flex-row gap-4">
              <button className="set-action-btn" disabled={!canSave} onClick={save} data-testid="recovery-save">
                {saving
                  ? t('common.loading')
                  : (configured ? t('settings.recovery.replaceBtn') : t('settings.recovery.setBtn'))}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
