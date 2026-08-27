import { useT } from '../lib/I18nContext';
import type { TaskExecutionDetail, TaskExecutionUsage } from '../types/generated';
import { allowedActions } from '../lib/orch-actions';

/** Tokens the CLI never reported are unknown, not zero. Rendering a total over
 *  partly-unmeasured sessions would read as "this is what it cost". */
function cliTokens(usage: TaskExecutionUsage): { value: number | null; complete: boolean } {
  const complete = usage.cli_sessions_unmeasured === 0;
  return { value: usage.cli_billable_tokens, complete };
}

interface Props {
  detail: TaskExecutionDetail | null;
  onOpen?: (subDiscussionId: string) => void;
  onStop?: () => void;
  onReassign?: () => void;
  onApprove?: () => void;
  onRequestChanges?: () => void;
}

export function TaskExecutionCard({
  detail,
  onOpen,
  onStop,
  onReassign,
  onApprove,
  onRequestChanges,
}: Props) {
  const { t } = useT();
  if (!detail) return null;

  const { lineage, target_branch, definition_of_done, attempts, validation_runs, usage, recovery } = detail;
  const exec = lineage.execution;
  const subDiscussionId = lineage.sub_discussion_id;
  const actions = allowedActions(exec.status, exec.interrupted_from_status);
  const cli = cliTokens(usage);
  const failing = validation_runs.filter(run => run.exit_code !== 0);

  return (
    <section className="orch-exec" data-testid="orch-exec-card">
      <header className="orch-exec-head">
        <span className="orch-exec-status" data-testid="orch-exec-status">{exec.status}</span>
        <span className="orch-exec-task">{lineage.task_reference} — {lineage.task_title}</span>
      </header>

      <dl className="orch-exec-facts">
        <dt>{t('orch.exec.worker')}</dt>
        <dd data-testid="orch-exec-worker">{exec.worker_agent_type ?? '—'}</dd>

        <dt>{t('orch.exec.model')}</dt>
        <dd data-testid="orch-exec-model">{exec.worker_model ?? '—'}</dd>

        <dt>{t('orch.exec.branch')}</dt>
        <dd data-testid="orch-exec-branch">{exec.child_branch ?? '—'}</dd>

        <dt>{t('orch.exec.target')}</dt>
        <dd data-testid="orch-exec-target">{target_branch ?? '—'}</dd>

        <dt>{t('orch.exec.integrated')}</dt>
        <dd data-testid="orch-exec-integrated">{exec.integrated_sha ?? '—'}</dd>

        <dt>{t('orch.exec.candidate')}</dt>
        <dd data-testid="orch-exec-candidate">{exec.candidate_merge_sha ?? exec.candidate_target_sha ?? '—'}</dd>

        <dt>{t('orch.exec.worktree')}</dt>
        <dd data-testid="orch-exec-worktree">{lineage.workspace_canonical_path ?? '—'}</dd>

        <dt>{t('orch.exec.duration')}</dt>
        <dd data-testid="orch-exec-duration">{Math.round(usage.duration_ms / 1000)}s</dd>

        <dt>{t('orch.exec.tokens')}</dt>
        <dd data-testid="orch-exec-tokens">
          {cli.value === null ? '—' : cli.value.toLocaleString()}
          {!cli.complete && (
            // Say it rather than average it away: a total drawn from sessions
            // that never reported is not the cost, it is a lower bound.
            <span className="orch-exec-partial" data-testid="orch-exec-tokens-partial">
              {t('orch.exec.partial', usage.cli_sessions_unmeasured, usage.cli_sessions)}
            </span>
          )}
        </dd>

        <dt>{t('orch.exec.cost')}</dt>
        <dd data-testid="orch-exec-cost">
          {usage.in_app_cost_usd === null ? '—' : `$${usage.in_app_cost_usd.toFixed(4)}`}
          {usage.in_app_cost_is_partial && (
            <span className="orch-exec-partial">{t('orch.exec.costPartial')}</span>
          )}
        </dd>

        <dt>{t('orch.exec.rounds')}</dt>
        <dd data-testid="orch-exec-rounds">{attempts.length}</dd>
      </dl>

      {failing.length > 0 && (
        <ul className="orch-exec-failing" data-testid="orch-exec-failing">
          {failing.map(run => (
            <li key={run.id}>{run.command}</li>
          ))}
        </ul>
      )}

      {recovery?.pending && (
        <p className="orch-exec-recovery" data-testid="orch-exec-recovery">
          {recovery.recovery_reason}
        </p>
      )}

      <ul className="orch-exec-dod" data-testid="orch-exec-dod">
        {definition_of_done.map(item => (
          <li key={item.id} data-met={item.completed}>
            {item.completed ? '✓' : '○'} {item.sentence}
          </li>
        ))}
      </ul>

      <div className="orch-exec-actions">
        {subDiscussionId && onOpen && (
          <button type="button" data-testid="orch-exec-open" onClick={() => onOpen(subDiscussionId)}>
            {t('orch.exec.open')}
          </button>
        )}
        {actions.approve && onApprove && (
          <button type="button" data-testid="orch-exec-approve" onClick={onApprove}>
            {t('orch.exec.approve')}
          </button>
        )}
        {actions.requestChanges && onRequestChanges && (
          <button type="button" data-testid="orch-exec-request-changes" onClick={onRequestChanges}>
            {t('orch.exec.requestChanges')}
          </button>
        )}
        {actions.stop && onStop && (
          <button type="button" data-testid="orch-exec-stop" onClick={onStop}>
            {t('orch.exec.stop')}
          </button>
        )}
        {actions.reassign && onReassign && (
          <button type="button" data-testid="orch-exec-reassign" onClick={onReassign}>
            {t('orch.exec.reassign')}
          </button>
        )}
      </div>
    </section>
  );
}
