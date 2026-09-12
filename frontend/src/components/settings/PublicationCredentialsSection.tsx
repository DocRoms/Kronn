// KT-619 — enrolling the credentials that may publish an important card.
//
// The authority is typed in and held in component state for the length of one
// administration session. It is never stored: not in localStorage, not in a
// URL, not in a query string. Reloading the page asks again, which is the
// point — a secret that survives a reload is a secret sitting somewhere.

import { useCallback, useRef, useState } from 'react';
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
  /// Where the bootstrap secret landed after a rotation. A path, never a secret.
  const [relocated, setRelocated] = useState<string | null>(null);
  // `busy` is React state, so it is not observable until the next render. Two
  // synchronous events — a double click, Enter on a focused button while the
  // click lands — both see `busy === false` and both fire. For a mutation that
  // MINTS A SECRET that is one secret too many, so the gate is a ref, which
  // changes now.
  const inFlight = useRef(false);

  /// Run `work` at most once at a time, whatever the event loop does.
  const once = useCallback(async (work: () => Promise<void>) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    try {
      await work();
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }, []);

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
        // And drop any secret still on screen. After a failed reload we no
        // longer know whether it is current — it may have been rotated away by
        // the very call that failed — and a stale secret displayed as usable is
        // worse than showing none.
        setMinted(null);
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
    await once(async () => {
      try {
        const created = await api.enrol(authority.trim(), role, label.trim());
        // The only moment this value exists outside the server.
        setMinted({ label: created.credential.label, secret: created.secret });
        setLabel('');
        await load(authority.trim());
      } catch {
        toast(t('settings.credentials.enrolFailed'), 'error');
      }
    });
  };

  const revoke = async (credential: HumanCredential) => {
    const reason = window.prompt(t('settings.credentials.revokeReason', credential.label));
    if (reason === null) return;
    await once(async () => {
      try {
        await api.revoke(authority.trim(), credential.id, reason || '—');
        await load(authority.trim());
        toast(t('settings.credentials.revoked', credential.label), 'success');
      } catch {
        toast(t('settings.credentials.revokeFailed'), 'error');
      }
    });
  };

  const rotate = async (credential: HumanCredential) => {
    await once(async () => {
      try {
        const secret = await api.rotate(authority.trim(), credential.id);
        setMinted({ label: credential.label, secret });
        await load(authority.trim());
      } catch {
        // The displayed secret may already be the one that was replaced, and a
        // stale secret shown as current is worse than none at all.
        setMinted(null);
        toast(t('settings.credentials.rotateFailed'), 'error');
      }
    });
  };

  /// Rotate the bootstrap secret itself — the way out when it may have leaked.
  ///
  /// The one who LOST it cannot come through here at all: proving you are the
  /// operator when you can no longer authenticate is a filesystem question, so
  /// that door is a `recover-admin-secret` file in the private directory,
  /// honoured at the next boot.
  const rotateAdmin = async () => {
    if (!window.confirm(t('settings.credentials.rotateAdminConfirm'))) return;
    await once(async () => {
      try {
        const { path } = await api.rotateAdmin(authority.trim());
        // What the operator just typed no longer authenticates anything, so the
        // screen goes back to the lock rather than leaving a dead authority in
        // a field that looks live.
        setRelocated(path);
        setCredentials(null);
        setAuthority('');
        setMinted(null);
      } catch {
        toast(t('settings.credentials.rotateAdminFailed'), 'error');
      }
    });
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

      {relocated && (
        <div className="settings-callout" role="status">
          <p>
            <strong>{t('settings.credentials.rotatedAdmin')}</strong>
          </p>
          <code>{relocated}</code>
          <p className="settings-hint">{t('settings.credentials.rotatedAdminHint')}</p>
        </div>
      )}

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
          {/* The operator who cannot get past this form is exactly the one no
              button here can help: the way back is a file in the private
              directory, so the screen says so instead of leaving them stuck. */}
          <p className="settings-hint">{t('settings.credentials.recoveryHint')}</p>
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

          <div className="settings-row">
            <button type="button" onClick={rotateAdmin} disabled={busy}>
              <RotateCw size={14} aria-hidden="true" />{' '}
              {t('settings.credentials.rotateAdmin')}
            </button>
            <p className="settings-hint">{t('settings.credentials.rotateAdminHint')}</p>
          </div>
        </>
      )}
    </section>
  );
}
