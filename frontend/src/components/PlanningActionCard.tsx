import { useEffect, useState } from 'react';
import { ListPlus, Loader2, Target } from 'lucide-react';
import { planning } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import type { PlanningProposal as ParsedPlanningProposal } from '../lib/planningProposal';
import type { PlanningProposal } from '../types/generated';
import { PlanningProposalReview } from './PlanningProposalReview';

interface Props {
  proposal: ParsedPlanningProposal;
  discussionId: string;
  sourceMessageId?: string;
  fenceIndex?: number;
}

export function PlanningActionCard({
  proposal,
  discussionId,
  sourceMessageId,
  fenceIndex,
}: Props) {
  const { t } = useT();
  const proposalId = proposal.action !== 'open' && sourceMessageId && fenceIndex !== undefined
    ? `proposal:${sourceMessageId}:${fenceIndex}`
    : null;
  const [result, setResult] = useState<{
    proposalId: string | null;
    durable: PlanningProposal | null;
    error: string;
  }>({ proposalId: null, durable: null, error: '' });
  const durable = result.proposalId === proposalId ? result.durable : null;
  const error = result.proposalId === proposalId ? result.error : '';
  const loading = Boolean(proposalId && result.proposalId !== proposalId);

  const title = proposal.action === 'create'
    ? proposal.title
    : proposal.action === 'create_many'
      ? t('planning.proposalCreateMany', proposal.tasks.length)
      : proposal.action === 'open'
        ? t('planning.proposalOpen')
        : t('planning.proposalUpdate', proposal.task_id);

  useEffect(() => {
    if (!proposalId) return;
    let cancelled = false;
    const load = () => {
      planning.proposal(proposalId)
        .then(next => {
          if (!cancelled) {
            setResult({ proposalId, durable: next, error: '' });
          }
        })
        .catch(cause => {
          if (!cancelled) {
            setResult({ proposalId, durable: null, error: userError(cause) });
          }
        });
    };
    const refresh = (event: Event) => {
      const changedDiscussionId = (
        event as CustomEvent<{ discussionId?: string }>
      ).detail?.discussionId;
      if (changedDiscussionId === discussionId) load();
    };
    load();
    window.addEventListener('kronn:plan-proposals-changed', refresh);
    return () => {
      cancelled = true;
      window.removeEventListener('kronn:plan-proposals-changed', refresh);
    };
  }, [discussionId, proposalId]);

  const open = () => {
    window.dispatchEvent(new CustomEvent('kronn:open-discussion-plan', {
      detail: { discussionId },
    }));
  };

  return (
    <div
      className="planning-proposal"
      data-state={durable?.aggregate_state ?? (error ? 'error' : 'pending')}
    >
      <div className="planning-proposal-icon">
        {proposal.action === 'open' ? <Target size={16} /> : <ListPlus size={16} />}
      </div>
      <div className="planning-proposal-main">
        <span>{t('planning.agentProposal')}</span>
        <strong>{title}</strong>
        {proposal.action === 'open' ? (
          <button type="button" className="planning-proposal-open" onClick={open}>
            {t('planning.proposalOpen')}
          </button>
        ) : loading ? (
          <span className="planning-proposal-loading">
            <Loader2 size={12} className="spin" /> {t('common.loading')}
          </span>
        ) : durable ? (
          <PlanningProposalReview
            proposal={durable}
            discussionId={discussionId}
            compact
            onChanged={next => setResult({ proposalId: next.id, durable: next, error: '' })}
          />
        ) : (
          <small>{error || t('planning.proposalUnavailable')}</small>
        )}
      </div>
    </div>
  );
}
