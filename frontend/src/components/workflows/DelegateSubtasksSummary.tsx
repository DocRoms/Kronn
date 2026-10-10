import { Network, Info } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import type { DelegateWorker, WorkflowStep } from '../../types/generated';

const workerLabel = (worker: DelegateWorker) =>
  [worker.agent, worker.tier, worker.model].filter(Boolean).join(' · ');

/** KT-909 — read-only view of a DelegateSubtasks step; it is authored as JSON or by the assistant. */
export function DelegateSubtasksSummary({ step }: { step: WorkflowStep }) {
  const { t } = useT();
  const config = step.delegate_subtasks;
  const workers = Object.entries(config?.worker_map ?? {});
  const rows: Array<[string, string]> = [
    [t('wiz.delegateParentTask'), config?.parent_task?.trim() || '—'],
    [t('wiz.delegateReviewer'), [step.agent, step.agent_settings?.tier, step.agent_settings?.model].filter(Boolean).join(' · ')],
    [t('wiz.delegateWorkers'), workers.length
      ? workers.map(([key, worker]) => `worker:${key} → ${workerLabel(worker)}`).join(', ')
      : '—'],
    [t('wiz.delegateDefaultWorker'), config?.default_worker ? workerLabel(config.default_worker) : '—'],
    [t('wiz.delegateLimits'), `${config?.concurrency ?? 1} · ${config?.max_review_rounds ?? 3}`],
    [t('wiz.delegateTargetBranch'), config?.target_branch?.trim() || t('wiz.delegateTargetBranchDefault')],
    [t('wiz.delegateValidations'), (config?.validations ?? []).map(spec => spec.command).join(', ') || '—'],
  ];
  return (
    <div className="wf-json-data-form" data-testid="delegate-subtasks-summary">
      <div className="wf-batch-intro">
        <Network size={14} />
        <div>
          <strong>{t('wiz.stepTypeDelegateSubtasks')}</strong>
          <p className="text-xs text-muted">{t('wiz.stepTypeDelegateSubtasksHint')}</p>
        </div>
      </div>
      {rows.map(([label, value]) => (
        <div key={label} className="wf-summary-row">
          <span className="wf-summary-label">{label}</span> <code>{value}</code>
        </div>
      ))}
      {step.prompt_template?.trim() && (
        <div className="wf-summary-row">
          <span className="wf-summary-label">{t('wiz.delegateGuidance')}</span> {step.prompt_template}
        </div>
      )}
      <div className="wf-batch-info mt-3">
        <Info size={12} />
        <span>{t('wiz.delegateReadOnly')}</span>
      </div>
    </div>
  );
}
