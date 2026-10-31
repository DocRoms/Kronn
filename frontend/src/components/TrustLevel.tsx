// KT-1492 — trust levels: the control that sets one, the risk panel that gates
// any level above Standard, and the badge that shows an active level with a
// one-click revoke.
import { useCallback, useEffect, useId, useState } from 'react';
import { AlertTriangle, ShieldAlert, ShieldCheck } from 'lucide-react';
import { trust as trustApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { settingsSectionPath } from '../lib/routes';
import { userError } from '../lib/userError';
import { useLockExpiry } from '../hooks/useLockExpiry';
import type { ToastFn } from '../hooks/useToast';
import type {
  NeverCovered, TrustEffect, TrustLevel, TrustOverview, TrustScopeView,
} from '../types/generated';
import './TrustLevel.css';

/** The Settings section that holds the global level (Settings › Trust levels). */
export const TRUST_SETTINGS_SECTION = 'settings-trust';

export type TrustTarget =
  | { scope: 'instance' }
  | { scope: 'room'; id: string }
  | { scope: 'workflow'; id: string };

const CHANGED = 'kronn:trust-changed';
const LEVELS: TrustLevel[] = ['standard', 'trusted', 'full'];
const RANK: Record<TrustLevel, number> = { standard: 0, trusted: 1, full: 2 };

const announce = () => window.dispatchEvent(new CustomEvent(CHANGED));

function read(target: TrustTarget): Promise<TrustScopeView> {
  return target.scope === 'instance' ? trustApi.instance() : trustApi.get(target.scope, target.id);
}

function write(
  target: TrustTarget, level: TrustLevel, acknowledged: boolean, proof?: string, vaultAcknowledged = false,
): Promise<TrustScopeView> {
  const request = {
    level,
    acknowledged_risks: acknowledged,
    acknowledged_vault_unprotected: vaultAcknowledged,
    proof,
  };
  return target.scope === 'instance'
    ? trustApi.setInstance(request)
    : trustApi.set(target.scope, target.id, request);
}

/** The scope's own setting and the level in force there, refreshed whenever
 *  a level changes anywhere on the page. A failed read shows nothing. */
function useTrust(target: TrustTarget) {
  const key = 'id' in target ? `${target.scope}:${target.id}` : target.scope;
  const [loaded, setLoaded] = useState<{ key: string; view: TrustScopeView | null } | null>(null);
  const refresh = useCallback(() => {
    Promise.resolve()
      .then(() => read(target))
      .then(view => setLoaded({ key, view: view ?? null }))
      .catch(() => setLoaded({ key, view: null }));
    // `target` is rebuilt every render; `key` is its identity.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  useEffect(() => {
    refresh();
    window.addEventListener(CHANGED, refresh);
    return () => window.removeEventListener(CHANGED, refresh);
  }, [refresh]);
  return { view: loaded?.key === key ? loaded.view : null, refresh };
}

const levelLabel = (t: (key: string) => string, level: TrustLevel) => t(`trust.level.${level}`);

/** What stays reachable by what a level approves (it is the owner's machine). */
const REACHABLE = [
  'projectDatabases', 'network', 'kronnApi', 'docker', 'otherRepositories', 'cliCredentials',
] as const;
/** What the OS sandbox closes to every command a level approves. */
const CLOSED = ['kronnData'] as const;

/** The sentence naming the missing vault protection, or null when it holds. */
const vaultGap = (t: (key: string, ...args: string[]) => string, sandbox: TrustOverview['sandbox']) => {
  if (sandbox.protected) return null;
  return sandbox.protect_vault
    ? t('trust.panel.vaultUnavailable', sandbox.reason ?? '')
    : t('trust.panel.vaultOff');
};

/** What choosing a level changes, concretely, and the confirmations it needs:
 *  the acknowledgement (and, when the Kronn vault is not protected on this
 *  machine, an acknowledgement that names it), and a code from the
 *  authenticator enrolled in Settings. */
export function TrustRiskPanel({
  level, overview, scope, busy, onConfirm, onCancel,
}: {
  level: TrustLevel;
  overview: TrustOverview;
  scope: TrustTarget['scope'];
  busy?: boolean;
  onConfirm: (proof: string, vaultAcknowledged: boolean) => void;
  onCancel: () => void;
}) {
  const { t } = useT();
  const titleId = useId();
  const [understood, setUnderstood] = useState(false);
  const [vaultUnderstood, setVaultUnderstood] = useState(false);
  const [proof, setProof] = useState('');
  const { sandbox, proof: enrolment } = overview;
  const locked = enrolment.locked_for_secs > 0;
  const gap = vaultGap(t, sandbox);
  // A lockout refuses an authenticator code, never a recovery code: the field
  // stays, and the server decides which code it accepts.
  const ready = enrolment.enrolled;
  const effects: TrustEffect[] = overview.effects.filter(effect => RANK[effect.from_level] <= RANK[level]);
  const never: NeverCovered[] = overview.never_covered;
  return (
    <div className="trust-backdrop" role="presentation" onClick={onCancel}>
      <div
        className="trust-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        data-testid="trust-risk-panel"
        data-level={level}
        onClick={event => event.stopPropagation()}
        onKeyDown={event => { if (event.key === 'Escape') onCancel(); }}
      >
        <h3 id={titleId} className="trust-dialog-title">
          <AlertTriangle size={16} aria-hidden="true" />
          {t('trust.panel.title', levelLabel(t, level))}
        </h3>
        <p className="trust-dialog-intro">{t(`trust.panel.intro.${level}`)}</p>
        <ul className="trust-effects" data-testid="trust-effects">
          {effects.map(effect => (
            <li key={effect.approval} data-approval={effect.approval}>
              <strong>{t(`trust.effect.${effect.approval}`)}</strong>
              {!effect.enforced && <span className="trust-pending-tag">{t('trust.notEnforced')}</span>}
              <span className="trust-risk">{t(`trust.risk.${effect.approval}`)}</span>
            </li>
          ))}
        </ul>
        <p className="trust-journal">{t('trust.panel.journal')}</p>
        <div className="trust-reach" data-testid="trust-reachable">
          <strong>{t('trust.panel.reachableTitle')}</strong>
          <ul>
            {REACHABLE.map(key => <li key={key}>{t(`trust.panel.reachable.${key}`)}</li>)}
          </ul>
        </div>
        <div className="trust-reach trust-closed" data-testid="trust-closed">
          <strong>{t('trust.panel.closedTitle')}</strong>
          <ul>
            {CLOSED.map(key => <li key={key}>{t(`trust.panel.closed.${key}`)}</li>)}
          </ul>
        </div>
        <p className="trust-limit" data-testid="trust-limit">{t('trust.panel.limit')}</p>
        <div className="trust-never" data-testid="trust-never">
          <strong>{t('trust.panel.neverTitle')}</strong>
          <ul>
            {never.map(kind => <li key={kind}>{t(`trust.never.${kind}`)}</li>)}
          </ul>
        </div>
        {scope !== 'instance' && (
          <p className="trust-global-link">
            <a href={settingsSectionPath(TRUST_SETTINGS_SECTION)} data-testid="trust-panel-settings-link">
              {t('trust.globalLink')}
            </a>
          </p>
        )}
        {gap && (
          <p className="trust-blocked" role="alert" data-testid="trust-vault-unprotected">
            {gap}
          </p>
        )}
        {!enrolment.enrolled && (
          <p className="trust-blocked" role="alert" data-testid="trust-proof-missing">
            {t('trust.panel.proofMissing')}{' '}
            <a href={settingsSectionPath(TRUST_SETTINGS_SECTION)} data-testid="trust-proof-enrol-link">
              {t('trust.panel.proofEnrolLink')}
            </a>
          </p>
        )}
        {ready && (
          <label className="trust-proof">
            <span>{t('trust.panel.proofLabel')}</span>
            <input
              type="text"
              inputMode="text"
              autoComplete="one-time-code"
              spellCheck={false}
              value={proof}
              onChange={event => setProof(event.target.value)}
              placeholder="123456"
              data-testid="trust-proof-input"
            />
          </label>
        )}
        {locked && (
          <p className="trust-blocked" role="alert" data-testid="trust-proof-locked">
            {t('trust.panel.proofLocked', String(enrolment.locked_for_secs))}
          </p>
        )}
        <label className="trust-ack">
          <input
            type="checkbox"
            checked={understood}
            onChange={event => setUnderstood(event.target.checked)}
            data-testid="trust-ack"
          />
          {t('trust.panel.ack')}
        </label>
        {gap && (
          <label className="trust-ack">
            <input
              type="checkbox"
              checked={vaultUnderstood}
              onChange={event => setVaultUnderstood(event.target.checked)}
              data-testid="trust-vault-ack"
            />
            {t('trust.panel.vaultAck')}
          </label>
        )}
        <div className="trust-dialog-actions">
          <button type="button" className="trust-btn" autoFocus onClick={onCancel} data-testid="trust-cancel">
            {t('common.cancel')}
          </button>
          <button
            type="button"
            className="trust-btn trust-btn-danger"
            disabled={!understood || (gap !== null && !vaultUnderstood) || !ready || proof.trim() === '' || busy}
            onClick={() => onConfirm(proof.trim(), gap !== null)}
            data-testid="trust-confirm"
          >
            {t('trust.panel.confirm', levelLabel(t, level))}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Choose Standard, Trusted or Full access for one scope. Anything above
 *  Standard goes through the risk panel; lowering never asks. */
export function TrustLevelControl({ target, toast }: { target: TrustTarget; toast?: ToastFn }) {
  const { t } = useT();
  const { view, refresh } = useTrust(target);
  const [pending, setPending] = useState<TrustLevel | null>(null);
  const [overview, setOverview] = useState<TrustOverview | null>(null);
  const [sandbox, setSandbox] = useState<TrustOverview['sandbox'] | null>(null);
  const [busy, setBusy] = useState(false);
  // The panel's own copy of the lock state, re-read after a refusal and when a lockout ends.
  const reloadOverview = useCallback(() => {
    Promise.resolve()
      .then(() => trustApi.overview())
      .then(loaded => { if (loaded) setOverview(loaded); })
      .catch(() => undefined);
  }, []);
  useLockExpiry(pending ? overview?.proof.locked_for_secs ?? 0 : 0, reloadOverview);

  // The vault protection is stated on the control itself, not only in the panel.
  useEffect(() => {
    Promise.resolve()
      .then(() => trustApi.overview())
      .then(loaded => setSandbox(loaded?.sandbox ?? null))
      .catch(() => setSandbox(null));
  }, []);

  const apply = async (level: TrustLevel, acknowledged: boolean, proof?: string, vaultAcknowledged = false) => {
    setBusy(true);
    try {
      await write(target, level, acknowledged, proof, vaultAcknowledged);
      setPending(null);
      announce();
      refresh();
    } catch (error) {
      toast?.(userError(error), 'error');
      // A refused code may have started a lockout: show it as it is now.
      if (pending) reloadOverview();
    } finally {
      setBusy(false);
    }
  };

  const choose = async (level: TrustLevel) => {
    // Lowering (or revoking) never asks: no panel, no code.
    if (level === 'standard' || (view && RANK[level] <= RANK[view.effective.level])) {
      return apply(level, false);
    }
    try {
      setOverview(await trustApi.overview());
      setPending(level);
    } catch (error) {
      toast?.(userError(error), 'error');
    }
  };

  const clear = async () => {
    if (target.scope === 'instance') return;
    try {
      await trustApi.clear(target.scope, target.id);
      announce();
      refresh();
    } catch (error) {
      toast?.(userError(error), 'error');
    }
  };

  if (!view) return null;
  const { effective, setting } = view;
  const own = setting.level ?? null;
  return (
    <div className="trust-control" data-testid={`trust-control-${target.scope}`}>
      <div className="trust-levels" role="radiogroup" aria-label={t('trust.title')}>
        {LEVELS.map(level => (
          <button
            key={level}
            type="button"
            role="radio"
            aria-checked={own === level}
            className="trust-level"
            data-level={level}
            data-active={own === level}
            disabled={busy}
            onClick={() => void choose(level)}
            data-testid={`trust-level-${level}`}
          >
            {levelLabel(t, level)}
          </button>
        ))}
        {target.scope !== 'instance' && own !== null && (
          <button type="button" className="trust-level trust-inherit" onClick={() => void clear()} data-testid="trust-inherit">
            {t('trust.inherit')}
          </button>
        )}
      </div>
      <p className="trust-effective" data-testid="trust-effective">
        {t('trust.effective', levelLabel(t, effective.level), t(`trust.source.${effective.source}`))}
      </p>
      {sandbox && !sandbox.protected && (
        <p className="trust-vault-note" role="status" data-testid="trust-vault-note">
          {sandbox.protect_vault ? t('trust.control.vaultUnavailable') : t('trust.control.vaultOff')}
        </p>
      )}
      {pending && overview && (
        <TrustRiskPanel
          level={pending}
          overview={overview}
          scope={target.scope}
          busy={busy}
          onConfirm={(proof, vaultAcknowledged) => void apply(pending, true, proof, vaultAcknowledged)}
          onCancel={() => setPending(null)}
        />
      )}
    </div>
  );
}

/** Visible while a level above Standard is in force for the scope, with where
 *  it comes from and a one-click revoke (an explicit Standard here). */
export function TrustBadge({ target, toast }: { target: TrustTarget; toast?: ToastFn }) {
  const { t } = useT();
  const { view, refresh } = useTrust(target);
  const [busy, setBusy] = useState(false);
  if (!view || view.effective.level === 'standard') return null;
  const { level, source } = view.effective;
  const revoke = async () => {
    setBusy(true);
    try {
      await write(target, 'standard', false);
      announce();
      refresh();
    } catch (error) {
      toast?.(userError(error), 'error');
    } finally {
      setBusy(false);
    }
  };
  const Icon = level === 'full' ? ShieldAlert : ShieldCheck;
  return (
    <span
      className="trust-badge"
      data-level={level}
      data-testid={`trust-badge-${target.scope}`}
      title={t('trust.badge.title', levelLabel(t, level), t(`trust.source.${source}`))}
    >
      <Icon size={10} aria-hidden="true" />
      <span>{t('trust.badge.label', levelLabel(t, level), t(`trust.source.${source}`))}</span>
      <button
        type="button"
        className="trust-badge-revoke"
        disabled={busy}
        onClick={() => void revoke()}
        aria-label={t('trust.revoke')}
        title={t('trust.revoke')}
        data-testid="trust-revoke"
      >
        ×
      </button>
    </span>
  );
}
