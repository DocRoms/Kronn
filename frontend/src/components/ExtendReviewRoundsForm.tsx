// KT-1345 — « extend by N » for an execution that is at (or near) its review-round
// limit: the human's counterpart of the principal's `task_exec_reassign` with
// `extend_review_rounds`. Raising the limit needs no escalation and no reassignment;
// an execution the limit had escalated is re-offered to its worker by the backend.
import { useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import { MAX_REVIEW_ROUNDS, MIN_REVIEW_ROUNDS, clampRounds, roundsLimitLabel } from '../lib/reviewRounds';

interface Props {
  t: (key: string, ...args: (string | number)[]) => string;
  reviewRounds: number;
  maxReviewRounds: number;
  busy?: boolean;
  onConfirm: (extend: number | 'unlimited') => void;
  onCancel: () => void;
}

export function ExtendReviewRoundsForm({ t, reviewRounds, maxReviewRounds, busy, onConfirm, onCancel }: Props) {
  const [by, setBy] = useState('3');
  const [unlimited, setUnlimited] = useState(false);
  const count = Number(by);
  const valid = unlimited || (Number.isInteger(count) && count >= MIN_REVIEW_ROUNDS && count <= MAX_REVIEW_ROUNDS);

  return (
    <div className="orch-inline-action" role="dialog" aria-label={t('orch.exec.extendRounds')} data-testid="orch-extend-form">
      <p data-testid="orch-extend-current">
        {t('orch.extend.current', reviewRounds, roundsLimitLabel(maxReviewRounds))}
      </p>
      <label>
        <span>{t('orch.extend.by')}</span>
        <input
          type="number"
          min={MIN_REVIEW_ROUNDS}
          max={MAX_REVIEW_ROUNDS}
          value={by}
          disabled={unlimited || busy}
          onChange={event => setBy(event.target.value)}
          data-testid="orch-extend-by"
        />
      </label>
      <label className="orch-approval-confirm">
        <input
          type="checkbox"
          checked={unlimited}
          disabled={busy}
          onChange={event => setUnlimited(event.target.checked)}
          data-testid="orch-extend-unlimited"
        />
        <span>{t('orch.extend.unlimited')}</span>
      </label>
      {unlimited && (
        <div className="set-expose-warn" role="alert" data-testid="orch-extend-warning">
          <AlertTriangle size={13} />
          <span>{t('config.workers.unlimitedWarning')}</span>
        </div>
      )}
      <p className="text-sm text-faint">{t('orch.extend.hint')}</p>
      <div>
        <button type="button" onClick={onCancel} disabled={busy}>{t('common.cancel')}</button>
        <button
          type="button"
          className="btn btn-primary"
          disabled={busy || !valid}
          data-testid="orch-extend-confirm"
          onClick={() => onConfirm(unlimited ? 'unlimited' : clampRounds(count))}
        >
          {t('orch.exec.extendRounds')}
        </button>
      </div>
    </div>
  );
}
