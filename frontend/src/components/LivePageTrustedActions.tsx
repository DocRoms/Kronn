import { useId, useState } from 'react';
import { ChevronDown, ShieldCheck, ShieldOff, ShieldAlert } from 'lucide-react';
import type { LivePageActionTrustState } from '../types/generated';
import { pages as pagesApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import { userError } from '../lib/userError';
import './LivePageTrustedActions.css';

export interface LivePageTrustedActionsProps {
  trusts: LivePageActionTrustState[];
  onChanged: () => Promise<void> | void;
}

export const TRUSTED_ACTIONS_EXPANDED_KEY = 'kronn:pageTrustedActionsExpanded';

function readExpanded(): boolean {
  try {
    return localStorage.getItem(TRUSTED_ACTIONS_EXPANDED_KEY) === 'true';
  } catch {
    return false;
  }
}

function writeExpanded(value: boolean): void {
  try {
    localStorage.setItem(TRUSTED_ACTIONS_EXPANDED_KEY, String(value));
  } catch {
    // Private mode or blocked storage: the panel simply starts collapsed next time.
  }
}

/** Withdrawn approvals first, then actions awaiting approval, then the approved ones. */
function eligibleRank(state: LivePageActionTrustState): number {
  if (state.trust?.invalidated_reason) return 0;
  return state.active ? 2 : 1;
}

/**
 * The human side of KT-1029: from the Page's details, never from its HTML, a
 * reader lets one action run on a click without its card, or withdraws that.
 */
export function LivePageTrustedActions({ trusts, onChanged }: LivePageTrustedActionsProps) {
  const { t } = useT();
  const detailsId = useId();
  const [expanded, setExpanded] = useState(readExpanded);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // The fingerprint shown here is what the server checks: a change since is refused.
  const approve = useAsyncGuard(async (state: LivePageActionTrustState) => {
    if (!state.fingerprint) return;
    try {
      await pagesApi.trustAction(state.action_id, state.fingerprint);
      setError(null);
      setConfirming(null);
    } catch (cause) {
      setError(userError(cause));
    }
    await onChanged();
  });

  const revoke = useAsyncGuard(async (state: LivePageActionTrustState) => {
    try {
      await pagesApi.revokeActionTrust(state.action_id);
      setError(null);
    } catch (cause) {
      setError(userError(cause));
    }
    await onChanged();
  });

  if (trusts.length === 0) return null;

  const eligible = trusts
    .filter(state => state.fingerprint || state.active)
    .sort((a, b) => eligibleRank(a) - eligibleRank(b));
  const groups = new Map<string, LivePageActionTrustState[]>();
  for (const state of trusts) {
    if (state.fingerprint || state.active) continue;
    const reason = state.refusal ?? 'unknown';
    groups.set(reason, [...(groups.get(reason) ?? []), state]);
  }
  const approved = eligible.filter(state => state.active).length;
  const pending = eligible.length - approved;
  const ineligible = trusts.length - eligible.length;
  const withdrawn = trusts.filter(state => state.trust?.invalidated_reason).length;

  const toggle = () => {
    setExpanded(value => {
      writeExpanded(!value);
      return !value;
    });
  };

  const renderRow = (state: LivePageActionTrustState) => {
    const invalidated = state.trust?.invalidated_reason ?? null;
    return (
      <li key={state.action_id} data-testid={`page-trust-${state.action_ref}`}>
        <span className="live-page-trusted-actions__name">
          {state.target_name} <code>{state.action_ref}</code>
        </span>
        {state.active ? (
          <>
            <span className="live-page-trusted-actions__badge">{t('pages.trust.active')}</span>
            <button type="button" onClick={() => void revoke(state)}>
              <ShieldOff size={12} aria-hidden /> {t('pages.trust.revoke')}
            </button>
          </>
        ) : confirming === state.action_id ? (
          <span className="live-page-trusted-actions__confirm">
            <span>{t('pages.trust.confirm', state.target_name)}</span>
            <button type="button" onClick={() => void approve(state)}>{t('pages.trust.confirmYes')}</button>
            <button type="button" onClick={() => setConfirming(null)}>{t('common.cancel')}</button>
          </span>
        ) : (
          <>
            {invalidated && (
              <span className="live-page-trusted-actions__notice" role="status">
                <ShieldAlert size={12} aria-hidden /> {t('pages.trust.invalidated', t(`pages.trust.reason.${invalidated}`))}
              </span>
            )}
            {state.fingerprint && (
              <button type="button" onClick={() => setConfirming(state.action_id)}>
                <ShieldCheck size={12} aria-hidden /> {invalidated ? t('pages.trust.reapprove') : t('pages.trust.approve')}
              </button>
            )}
            {invalidated && (
              <button type="button" onClick={() => void revoke(state)}>{t('pages.trust.dismiss')}</button>
            )}
          </>
        )}
      </li>
    );
  };

  const counts = eligible.length === 0
    ? [t('pages.trust.summaryNone', ineligible)]
    : [
      t('pages.trust.summaryPending', pending),
      t('pages.trust.summaryApproved', approved),
      ...(ineligible > 0 ? [t('pages.trust.summaryIneligible', ineligible)] : []),
    ];
  if (withdrawn > 0) counts.push(t('pages.trust.summaryWithdrawn', withdrawn));

  return (
    <section
      className="live-page-trusted-actions"
      aria-label={t('pages.trust.title')}
      data-testid="page-trusted-actions"
      data-expanded={expanded}
    >
      <div className="live-page-trusted-actions__summary">
        <ShieldCheck size={13} aria-hidden />
        <strong>{t('pages.trust.title')}</strong>
        <span className="live-page-trusted-actions__counts" data-testid="page-trusted-actions-counts">
          {counts.join(' · ')}
        </span>
        <button
          type="button"
          className="live-page-trusted-actions__toggle"
          onClick={toggle}
          aria-expanded={expanded}
          aria-controls={detailsId}
          data-testid="page-trusted-actions-toggle"
        >
          {expanded ? t('pages.trust.hideDetails') : t('pages.trust.details')}
          <ChevronDown size={12} aria-hidden />
        </button>
      </div>
      {error && <p className="live-page-trusted-actions__error" role="alert">{error}</p>}
      {expanded && (
        <div id={detailsId} className="live-page-trusted-actions__details">
          <small className="live-page-trusted-actions__hint">{t('pages.trust.hint')}</small>
          {eligible.length > 0 && (
            <ul data-testid="page-trusted-actions-eligible">{eligible.map(renderRow)}</ul>
          )}
          {[...groups.entries()].map(([reason, states]) => (
            <details
              key={reason}
              className="live-page-trusted-actions__group"
              data-testid={`page-trust-group-${reason}`}
              open={states.some(state => state.trust?.invalidated_reason) || undefined}
            >
              <summary>
                <ChevronDown size={12} aria-hidden className="live-page-trusted-actions__chevron" />
                <span>{t('pages.trust.ineligible', t(`pages.trust.reason.${reason}`))}</span>
                <span className="live-page-trusted-actions__count">{states.length}</span>
              </summary>
              <ul>{states.map(renderRow)}</ul>
            </details>
          ))}
        </div>
      )}
    </section>
  );
}
