import { useState } from 'react';
import { Check, ListPlus, Loader2, Target } from 'lucide-react';
import { planning } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import type { PlanningProposal } from '../lib/planningProposal';

interface Props {
  proposal: PlanningProposal;
  discussionId: string;
}

export function PlanningActionCard({ proposal, discussionId }: Props) {
  const { t } = useT();
  const [state, setState] = useState<'idle' | 'running' | 'done' | 'error'>('idle');
  const [error, setError] = useState('');

  const title = proposal.action === 'create'
    ? proposal.title
    : proposal.action === 'create_many'
      ? t('planning.proposalCreateMany', proposal.tasks.length)
      : proposal.action === 'open'
        ? t('planning.proposalOpen')
        : t('planning.proposalUpdate', proposal.task_id);

  const apply = async () => {
    if (state !== 'idle') return;
    setState('running');
    try {
      if (proposal.action === 'open') {
        window.dispatchEvent(new CustomEvent('kronn:open-discussion-plan', {
          detail: { discussionId },
        }));
      } else if (proposal.action === 'create' || proposal.action === 'create_many') {
        const tasks = proposal.action === 'create_many' ? proposal.tasks : [proposal];
        for (const task of tasks) {
          const created = await planning.create({
            title: task.title.trim(),
            description: task.description ?? '',
            priority: task.priority ?? 'normal',
            status: 'todo',
          });
          await planning.linkDiscussion(created.id, {
            discussion_id: discussionId,
            placement: task.placement ?? 'active',
            is_primary: proposal.action === 'create' && Boolean(proposal.is_primary),
          });
        }
      } else if (proposal.action === 'complete') {
        await planning.update(proposal.task_id, { status: 'done' });
      } else if (proposal.action === 'unblock') {
        await planning.update(proposal.task_id, {
          status: 'todo',
          blocked_reason: null,
        });
      } else {
        await planning.update(proposal.task_id, { status: proposal.status });
      }
      setState('done');
      window.dispatchEvent(new CustomEvent('kronn:plan-changed', {
        detail: { discussionId },
      }));
    } catch (cause) {
      setError(userError(cause));
      setState('error');
    }
  };

  return (
    <div className="planning-proposal" data-state={state}>
      <div className="planning-proposal-icon">
        {proposal.action === 'open' ? <Target size={16} /> : <ListPlus size={16} />}
      </div>
      <div className="planning-proposal-main">
        <span>{t('planning.agentProposal')}</span>
        <strong>{title}</strong>
        {proposal.action === 'create_many' && (
          <ol className="planning-proposal-task-list">
            {proposal.tasks.map((task, index) => (
              <li key={`${task.title}-${index}`} title={task.title.trim()}>
                {task.title.trim()}
              </li>
            ))}
          </ol>
        )}
        {error && <small>{error}</small>}
      </div>
      <button type="button" onClick={() => void apply()} disabled={state !== 'idle'}>
        {state === 'running' && <Loader2 size={13} className="spin" />}
        {state === 'done' && <Check size={13} />}
        {state === 'idle' && t('planning.applyProposal')}
        {state === 'done' && t('planning.applied')}
        {state === 'error' && t('planning.failed')}
      </button>
    </div>
  );
}
