import { useEffect, useRef, useState } from 'react';
import { Check, Loader2, RotateCcw, X } from 'lucide-react';
import { planning } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import type {
  PlanningProposal,
  PlanningProposalItem,
  ProposalDecision,
} from '../types/generated';

interface Props {
  proposal: PlanningProposal;
  discussionId: string;
  compact?: boolean;
  onChanged?: (proposal: PlanningProposal) => void;
}

function freshIdempotencyKey() {
  return globalThis.crypto?.randomUUID?.()
    ?? `proposal-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function itemTitle(
  item: PlanningProposalItem,
  fallback: string,
  t: (key: string, ...args: (string | number)[]) => string,
) {
  if (item.action === 'status' && item.payload.task_id && item.payload.status) {
    return t(
      'planning.proposalAction.status',
      item.payload.task_id,
      t(`planning.status.${item.payload.status}`),
    );
  }
  if (item.action === 'complete' && item.payload.task_id) {
    return t('planning.proposalAction.complete', item.payload.task_id);
  }
  if (item.action === 'unblock' && item.payload.task_id) {
    return t('planning.proposalAction.unblock', item.payload.task_id);
  }
  return item.payload.title?.trim() || item.payload.task_id?.trim() || fallback;
}

export function PlanningProposalReview({
  proposal,
  discussionId,
  compact = false,
  onChanged,
}: Props) {
  const { t } = useT();
  const [current, setCurrent] = useState(proposal);
  const [savingItemId, setSavingItemId] = useState<string | null>(null);
  const [rejectingItemId, setRejectingItemId] = useState<string | null>(null);
  const [rejectReason, setRejectReason] = useState('');
  const [errors, setErrors] = useState<Record<string, string>>({});
  const retryKeys = useRef(new Map<string, string>());

  useEffect(() => setCurrent(proposal), [proposal]);

  const decide = async (
    item: PlanningProposalItem,
    decision: ProposalDecision,
    reason?: string,
  ) => {
    if (item.state !== 'pending' || savingItemId) return;
    const retrySlot = `${item.id}:${decision}`;
    const idempotencyKey = retryKeys.current.get(retrySlot) ?? freshIdempotencyKey();
    retryKeys.current.set(retrySlot, idempotencyKey);
    setSavingItemId(item.id);
    setErrors(previous => ({ ...previous, [item.id]: '' }));
    try {
      const result = await planning.decideProposalItem(current.id, item.id, {
        decision,
        reason: reason?.trim() || undefined,
        idempotency_key: idempotencyKey,
      });
      const next: PlanningProposal = {
        ...current,
        aggregate_state: result.aggregate_state,
        items: current.items.map(candidate => (
          candidate.id === item.id ? result.item : candidate
        )),
      };
      retryKeys.current.delete(retrySlot);
      setCurrent(next);
      setRejectingItemId(null);
      setRejectReason('');
      onChanged?.(next);
      window.dispatchEvent(new CustomEvent('kronn:plan-proposals-changed', {
        detail: { discussionId },
      }));
    } catch (cause) {
      setErrors(previous => ({ ...previous, [item.id]: userError(cause) }));
    } finally {
      setSavingItemId(null);
    }
  };

  return (
    <div
      className="planning-review"
      data-state={current.aggregate_state}
      data-compact={compact}
    >
      {current.items.map((item, index) => {
        const isSaving = savingItemId === item.id;
        const isRejecting = rejectingItemId === item.id;
        return (
          <div className="planning-review-item" data-state={item.state} key={item.id}>
            <div className="planning-review-item-main">
              <span className="planning-review-item-state" data-state={item.state}>
                {item.state === 'accepted' && <Check size={10} />}
                {item.state === 'rejected' && <X size={10} />}
                {t(`planning.proposalItem.${item.state}`)}
              </span>
              <strong>
                {itemTitle(item, t('planning.proposalItemFallback', index + 1), t)}
              </strong>
              {item.payload.description && <small>{item.payload.description}</small>}
              {item.rejected_reason && (
                <small>{t('planning.proposalRejectedReason', item.rejected_reason)}</small>
              )}
              {errors[item.id] && (
                <small className="planning-review-error">
                  {errors[item.id]}
                  <button
                    type="button"
                    onClick={() => void decide(
                      item,
                      isRejecting ? 'reject' : 'accept',
                      isRejecting ? rejectReason : undefined,
                    )}
                  >
                    <RotateCcw size={10} /> {t('planning.retry')}
                  </button>
                </small>
              )}
              {isRejecting && (
                <div className="planning-review-reject">
                  <input
                    autoFocus
                    value={rejectReason}
                    onChange={event => setRejectReason(event.target.value)}
                    onKeyDown={event => {
                      if (event.key === 'Enter') {
                        void decide(item, 'reject', rejectReason);
                      } else if (event.key === 'Escape') {
                        setRejectingItemId(null);
                        setRejectReason('');
                      }
                    }}
                    placeholder={t('planning.proposalRejectReason')}
                    maxLength={500}
                  />
                  <button
                    type="button"
                    onClick={() => void decide(item, 'reject', rejectReason)}
                    disabled={isSaving}
                  >
                    {isSaving ? <Loader2 size={11} className="spin" /> : <Check size={11} />}
                  </button>
                </div>
              )}
            </div>
            {item.state === 'pending' && !isRejecting && (
              <div className="planning-review-actions">
                <button
                  type="button"
                  data-action="reject"
                  onClick={() => {
                    setRejectingItemId(item.id);
                    setRejectReason('');
                  }}
                  disabled={Boolean(savingItemId)}
                >
                  <X size={11} /> {t('planning.rejectProposal')}
                </button>
                <button
                  type="button"
                  data-action="accept"
                  onClick={() => void decide(item, 'accept')}
                  disabled={Boolean(savingItemId)}
                >
                  {isSaving ? <Loader2 size={11} className="spin" /> : <Check size={11} />}
                  {t('planning.acceptProposal')}
                </button>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
