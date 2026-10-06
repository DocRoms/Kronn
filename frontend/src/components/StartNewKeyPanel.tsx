// KT-1007 — the way forward when the key is lost for good (no passphrase, no
// code): the locked secrets are kept in a file and removed, a new key starts,
// and everything not encrypted (discussions, projects, workflows) stays.
import { useState } from 'react';
import { config as configApi } from '../lib/api';
import type { StartNewKeyResponse } from '../types/generated';

interface StartNewKeyPanelProps {
  t: (key: string, ...args: (string | number)[]) => string;
  /** Called once the user has seen the result (the app reloads). */
  onDone: () => void;
}

export function StartNewKeyPanel({ t, onDone }: StartNewKeyPanelProps) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<StartNewKeyResponse | null>(null);

  const start = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      setResult(await configApi.startNewKey());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  if (result) {
    return (
      <div data-testid="start-new-key-done">
        <p>{t('startNewKey.done', result.rows, result.kept_file)}</p>
        {result.auth_token && (
          <>
            <p>{t('authLocked.newTokenDone')}</p>
            <code className="set-recovery-code" data-testid="start-new-key-token">{result.auth_token}</code>
          </>
        )}
        <button type="button" className="btn btn-primary" onClick={onDone}>{t('authLocked.continue')}</button>
      </div>
    );
  }
  if (!confirming) {
    return (
      <button type="button" className="btn btn-ghost" onClick={() => setConfirming(true)} data-testid="start-new-key-btn">
        {t('startNewKey.action')}
      </button>
    );
  }
  return (
    <div className="set-expose-warn" data-testid="start-new-key-confirm">
      <p>{t('startNewKey.confirm')}</p>
      <div className="flex-row gap-4">
        <button type="button" className="btn btn-primary" disabled={busy} onClick={start} data-testid="start-new-key-yes">
          {busy ? t('common.loading') : t('startNewKey.yes')}
        </button>
        <button type="button" className="btn btn-ghost" disabled={busy} onClick={() => setConfirming(false)}>
          {t('common.cancel')}
        </button>
      </div>
      {error && <p className="text-error" data-testid="start-new-key-error">{error}</p>}
    </div>
  );
}
