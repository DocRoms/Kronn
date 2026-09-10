// KT-619 — enrolling the credentials that may publish an important card.
//
// The authority is typed in and held in component state for the length of one
// administration session. It is never stored: not in localStorage, not in a
// URL, not in a query string. Reloading the page asks again, which is the
// point — a secret that survives a reload is a secret sitting somewhere.

import { useCallback, useState } from 'react';
import { KeyRound, RotateCw, ShieldOff, Copy, Check } from 'lucide-react';
import { publicationCredentials as api } from '../../lib/api';
import type { GrantRole, HumanCredential } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';
import '../../pages/SettingsPage.css';

interface Props {
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

/// Shown once, then gone. Kept out of the credential list on purpose.
interface Minted {
  label: string;
  secret: string;
}

export function PublicationCredentialsSection({ toast, t }: Props) {
  const [authority, setAuthority] = useState('');
  const [credentials, setCredentials] = useState<HumanCredential[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [label, setLabel] = useState('');
  const [role, setRole] = useState<GrantRole>('human');
  const [minted, setMinted] = useState<Minted | null>(null);
  const [copied, setCopied] = useState(false);

  const load = useCallback(
    async (secret: string) => {
      setBusy(true);
      try {
        setCredentials(await api.list(secret));
      } catch {
        // The server refuses every wrong authority the same way, so the UI
        // says the same thing: telling the user which guard they tripped would
        // tell an attacker what to try next.
        setCredentials(null);
        toast(t('settings.credentials.refused'), 'error');
      } finally {
        setBusy(false);
      }
    },
    [toast, t],
  );

  const unlock = async (event: React.FormEvent) => {
    event.preventDefault();
    if (authority.trim()) await load(authority.trim());
  };

  const enrol = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!label.trim()) return;
    setBusy(true);
    try {
      const created = await api.enrol(authority.trim(), role, label.trim());
      // The only moment this value exists outside the server.
      setMinted({ label: created.credential.label, secret: created.secret });
      setLabel('');
      await load(authority.trim());
    } catch {
      toast(t('settings.credentials.enrolFailed'), 'error');
    } finally {
      setBusy(false);
    }
  };

  const revoke = async (credential: HumanCredential) => {
    const reason = window.prompt(t('settings.credentials.revokeReason', credential.label));
    if (reason === null) return;
    setBusy(true);
    try {
      await api.revoke(authority.trim(), credential.id, reason || '—');
      await load(authority.trim());
      toast(t('settings.credentials.revoked', credential.label), 'success');
    } catch {
      toast(t('settings.credentials.revokeFailed'), 'error');
    } finally {
      setBusy(false);
    }
  };

  const rotate = async (credential: HumanCredential) => {
    setBusy(true);
    try {
      const secret = await api.rotate(authority.trim(), credential.id);
      setMinted({ label: credential.label, secret });
      await load(authority.trim());
    } catch {
      toast(t('settings.credentials.rotateFailed'), 'error');
    } finally {
      setBusy(false);
    }
  };

  const copySecret = async () => {
    if (!minted) return;
    try {
      await navigator.clipboard.writeText(minted.secret);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      // Clipboard access is refused often enough that failing silently would
      // look like the button doing nothing.
      toast(t('settings.credentials.copyFailed'), 'error');
    }
  };

  return (
    <section className="settings-section" aria-labelledby="credentials-heading">
      <h2 id="credentials-heading">
        <KeyRound size={18} aria-hidden="true" /> {t('settings.credentials.title')}
      </h2>
      <p className="settings-hint">{t('settings.credentials.intro')}</p>

      {credentials === null ? (
        <form onSubmit={unlock} className="settings-row">
          <label htmlFor="credentials-authority">
            {t('settings.credentials.authorityLabel')}
          </label>
          <input
            id="credentials-authority"
            type="password"
            autoComplete="off"
            value={authority}
            onChange={(event) => setAuthority(event.target.value)}
            placeholder={t('settings.credentials.authorityPlaceholder')}
          />
          <button type="submit" disabled={busy || !authority.trim()}>
            {t('settings.credentials.unlock')}
          </button>
          <p className="settings-hint">{t('settings.credentials.authorityHint')}</p>
        </form>
      ) : (
        <>
          <form onSubmit={enrol} className="settings-row">
            <label htmlFor="credentials-label">{t('settings.credentials.newLabel')}</label>
            <input
              id="credentials-label"
              value={label}
              onChange={(event) => setLabel(event.target.value)}
              maxLength={100}
            />
            <label htmlFor="credentials-role">{t('settings.credentials.newRole')}</label>
            <select
              id="credentials-role"
              value={role}
              onChange={(event) => setRole(event.target.value as GrantRole)}
            >
              <option value="human">{t('settings.credentials.role.human')}</option>
              <option value="orchestrator">
                {t('settings.credentials.role.orchestrator')}
              </option>
            </select>
            <button type="submit" disabled={busy || !label.trim()}>
              {t('settings.credentials.enrol')}
            </button>
          </form>

          {minted && (
            <div className="settings-callout" role="status">
              <p>
                <strong>{t('settings.credentials.mintedOnce', minted.label)}</strong>
              </p>
              <code>{minted.secret}</code>
              <button
                type="button"
                onClick={copySecret}
                aria-label={t('settings.credentials.copy')}
              >
                {copied ? <Check size={14} /> : <Copy size={14} />}
              </button>
              <p className="settings-hint">{t('settings.credentials.mintedHint')}</p>
            </div>
          )}

          <table className="settings-table">
            <caption className="sr-only">{t('settings.credentials.tableCaption')}</caption>
            <thead>
              <tr>
                <th scope="col">{t('settings.credentials.colLabel')}</th>
                <th scope="col">{t('settings.credentials.colRole')}</th>
                <th scope="col">{t('settings.credentials.colState')}</th>
                <th scope="col">{t('settings.credentials.colActions')}</th>
              </tr>
            </thead>
            <tbody>
              {credentials.length === 0 && (
                <tr>
                  <td colSpan={4}>{t('settings.credentials.empty')}</td>
                </tr>
              )}
              {credentials.map((credential) => (
                <tr key={credential.id} data-revoked={credential.revoked_at ? 'true' : 'false'}>
                  <td>{credential.label}</td>
                  <td>{t(`settings.credentials.role.${credential.role}`)}</td>
                  <td>
                    {credential.revoked_at
                      ? t('settings.credentials.stateRevoked', credential.revoked_reason || '—')
                      : t('settings.credentials.stateLive')}
                  </td>
                  <td>
                    <button
                      type="button"
                      onClick={() => rotate(credential)}
                      disabled={busy || Boolean(credential.revoked_at)}
                      aria-label={t('settings.credentials.rotate', credential.label)}
                    >
                      <RotateCw size={14} aria-hidden="true" />
                    </button>
                    <button
                      type="button"
                      onClick={() => revoke(credential)}
                      disabled={busy || Boolean(credential.revoked_at)}
                      aria-label={t('settings.credentials.revoke', credential.label)}
                    >
                      <ShieldOff size={14} aria-hidden="true" />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
    </section>
  );
}
