// KT-1492 — Settings › Trust levels: the strong confirmation (an authenticator) that
// raising a trust level needs. A code from any TOTP app (Proton, Microsoft,
// Google, KeePassXC) or one of ten single-use recovery codes. Changing or
// disabling it needs a proof of the current one.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { KeyRound } from 'lucide-react';
import { trust as trustApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import { encodeQr } from '../../lib/qr';
import { userError } from '../../lib/userError';
import { useLockExpiry } from '../../hooks/useLockExpiry';
import type { ToastFn } from '../../hooks/useToast';
import type { TrustEnrolment, TrustProofView } from '../../types/generated';

const CHANGED = 'kronn:trust-changed';
const QUIET_ZONE = 4;

/** The seed as an authenticator draws it: scannable, with the manual key beside it. */
export function QrImage({ text, label }: { text: string; label: string }) {
  const qr = useMemo(() => encodeQr(text, 'M'), [text]);
  if (!qr) return null;
  const size = qr.size + QUIET_ZONE * 2;
  const path = qr.modules
    .flatMap((row, y) => row.map((dark, x) => (dark ? `M${x + QUIET_ZONE} ${y + QUIET_ZONE}h1v1h-1z` : '')))
    .join('');
  return (
    <svg
      className="trust-qr"
      role="img"
      aria-label={label}
      viewBox={`0 0 ${size} ${size}`}
      shapeRendering="crispEdges"
      data-testid="trust-proof-qr"
    >
      <rect width={size} height={size} fill="#fff" />
      <path d={path} fill="#000" />
    </svg>
  );
}

/** The manual key in groups of four, as it is easier to copy by eye. */
const grouped = (secret: string) => secret.replace(/(.{4})/g, '$1 ').trim();

export function TrustProofSection({ toast }: { toast?: ToastFn }) {
  const { t } = useT();
  const [status, setStatus] = useState<TrustProofView | null>(null);
  const [enrolment, setEnrolment] = useState<TrustEnrolment | null>(null);
  const [code, setCode] = useState('');
  const [proof, setProof] = useState('');
  const [adminSecret, setAdminSecret] = useState('');
  const [recovery, setRecovery] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [replacing, setReplacing] = useState(false);

  const load = useCallback(() => {
    Promise.resolve()
      .then(() => trustApi.proof())
      .then(view => setStatus(view ?? null))
      .catch(() => setStatus(null));
  }, []);
  useEffect(() => {
    load();
    window.addEventListener(CHANGED, load);
    return () => window.removeEventListener(CHANGED, load);
  }, [load]);

  useLockExpiry(status?.locked_for_secs ?? 0, load);

  const run = async (work: () => Promise<void>) => {
    setBusy(true);
    try {
      await work();
    } catch (error) {
      toast?.(userError(error), 'error');
    } finally {
      setBusy(false);
      load();
    }
  };

  const begin = () => run(async () => {
    setEnrolment(await trustApi.enrolProof(status?.enrolled ? proof.trim() : undefined, adminSecret.trim()));
    setProof('');
    setCode('');
    setReplacing(false);
  });
  const confirm = () => run(async () => {
    const issued = await trustApi.confirmProof(code.trim(), adminSecret.trim());
    setRecovery(issued.codes);
    setEnrolment(null);
    setCode('');
    setAdminSecret('');
    window.dispatchEvent(new CustomEvent(CHANGED));
  });
  const renew = () => run(async () => {
    const issued = await trustApi.renewRecoveryCodes(proof.trim());
    setRecovery(issued.codes);
    setProof('');
  });
  const cancelPending = () => run(async () => {
    await trustApi.disableProof(undefined, adminSecret.trim());
    setAdminSecret('');
    window.dispatchEvent(new CustomEvent(CHANGED));
  });
  const disable = () => run(async () => {
    await trustApi.disableProof(proof.trim());
    setProof('');
    setRecovery(null);
    window.dispatchEvent(new CustomEvent(CHANGED));
  });

  if (!status) return null;
  const locked = status.locked_for_secs > 0;
  // The first enrolment (and a replacement, which a single code handed to an
  // agent must not allow) is authorised by the admin secret: until a code is
  // enrolled, nothing else proves who is asking. Once enrolled, the field only
  // shows when the owner starts a replacement.
  const adminField = (
    <label className="trust-proof">
      <span>{t('trust.proof.adminSecret')}</span>
      <input
        type="password"
        autoComplete="off"
        value={adminSecret}
        onChange={event => setAdminSecret(event.target.value)}
        data-testid="trust-proof-admin-secret"
      />
      <span className="set-meta">
        {t(status.enrolled ? 'trust.proof.adminSecretHintReplace' : 'trust.proof.adminSecretHint')}
      </span>
    </label>
  );
  return (
    <div className="trust-proof-section" data-testid="trust-proof-section">
      <h4 className="mt-8 flex-row gap-4">
        <KeyRound size={14} aria-hidden="true" />
        {t('trust.proof.title')}
      </h4>
      <p className="set-meta">{t('trust.proof.hint')}</p>

      {recovery && (
        <div className="trust-recovery" role="alert" data-testid="trust-proof-recovery">
          <strong>{t('trust.proof.recoveryTitle')}</strong>
          <p className="set-meta">{t('trust.proof.recoveryHint')}</p>
          <ul>
            {recovery.map(item => <li key={item}><code>{item}</code></li>)}
          </ul>
          <div className="trust-actions">
            <button
              type="button"
              className="trust-btn"
              onClick={() => void navigator.clipboard?.writeText(recovery.join('\n'))}
              data-testid="trust-proof-copy"
            >
              {t('trust.proof.copy')}
            </button>
            <button
              type="button"
              className="trust-btn"
              onClick={() => setRecovery(null)}
              data-testid="trust-proof-saved"
            >
              {t('trust.proof.saved')}
            </button>
          </div>
        </div>
      )}

      {enrolment && (
        <div className="trust-enrol" data-testid="trust-proof-enrol">
          <p>{t('trust.proof.scan')}</p>
          <QrImage text={enrolment.uri} label={t('trust.proof.qrLabel')} />
          <p className="set-meta">{t('trust.proof.manualKey')}</p>
          <code className="trust-secret" data-testid="trust-proof-secret">{grouped(enrolment.secret)}</code>
          <label className="trust-proof">
            <span>{t('trust.proof.firstCode')}</span>
            <input
              type="text"
              inputMode="numeric"
              autoComplete="one-time-code"
              value={code}
              onChange={event => setCode(event.target.value)}
              placeholder="123456"
              data-testid="trust-proof-code"
            />
          </label>
          <button
            type="button"
            className="trust-btn"
            disabled={busy || code.trim() === '' || adminSecret.trim() === ''}
            onClick={() => void confirm()}
            data-testid="trust-proof-confirm"
          >
            {t('trust.proof.confirm')}
          </button>
        </div>
      )}

      {!enrolment && !status.enrolled && (
        <>
          <p className="set-meta" data-testid="trust-proof-status">
            {status.pending ? t('trust.proof.statusPending') : t('trust.proof.statusNone')}
          </p>
          {adminField}
          <div className="trust-actions">
            <button
              type="button"
              className="trust-btn"
              disabled={busy || adminSecret.trim() === ''}
              onClick={() => void begin()}
              data-testid="trust-proof-begin"
            >
              {t('trust.proof.begin')}
            </button>
            {status.pending && (
              <button
                type="button"
                className="trust-btn"
                disabled={busy || adminSecret.trim() === ''}
                onClick={() => void cancelPending()}
                data-testid="trust-proof-cancel-pending"
              >
                {t('trust.proof.cancelPending')}
              </button>
            )}
          </div>
        </>
      )}

      {!enrolment && status.enrolled && (
        <>
          <p className="set-meta" data-testid="trust-proof-status">
            {t('trust.proof.statusEnrolled', String(status.recovery_codes_left))}
          </p>
          {locked && (
            <p className="trust-blocked" role="alert" data-testid="trust-proof-locked">
              {t('trust.panel.proofLocked', String(status.locked_for_secs))}
            </p>
          )}
          <label className="trust-proof">
            <span>{t('trust.proof.actionCode')}</span>
            <input
              type="text"
              autoComplete="one-time-code"
              value={proof}
              onChange={event => setProof(event.target.value)}
              placeholder="123456"
              data-testid="trust-proof-action-code"
            />
          </label>
          {replacing && adminField}
          <div className="trust-actions">
            {replacing ? (
              <>
                <button
                  type="button"
                  className="trust-btn"
                  disabled={busy || proof.trim() === '' || adminSecret.trim() === ''}
                  onClick={() => void begin()}
                  data-testid="trust-proof-replace"
                >
                  {t('trust.proof.replaceStart')}
                </button>
                <button
                  type="button"
                  className="trust-btn"
                  disabled={busy}
                  onClick={() => { setReplacing(false); setAdminSecret(''); }}
                  data-testid="trust-proof-replace-cancel"
                >
                  {t('trust.proof.replaceCancel')}
                </button>
              </>
            ) : (
              <button
                type="button"
                className="trust-btn"
                disabled={busy}
                onClick={() => setReplacing(true)}
                data-testid="trust-proof-replace-open"
              >
                {t('trust.proof.replace')}
              </button>
            )}
            <button
              type="button"
              className="trust-btn"
              disabled={busy || proof.trim() === ''}
              onClick={() => void renew()}
              data-testid="trust-proof-renew"
            >
              {t('trust.proof.renew')}
            </button>
            <button
              type="button"
              className="trust-btn trust-btn-danger"
              disabled={busy || proof.trim() === ''}
              onClick={() => void disable()}
              data-testid="trust-proof-disable"
            >
              {t('trust.proof.disable')}
            </button>
          </div>
        </>
      )}
      <p className="set-meta">{t('trust.proof.reset')}</p>
    </div>
  );
}
