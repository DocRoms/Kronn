import { useState } from 'react';
import { useT } from '../../lib/I18nContext';
import { AGENT_COLORS, AGENT_LABELS, isAgentRestricted } from '../../lib/constants';
import type { Workflow, WorkflowRun, StepResult, AgentsConfig } from '../../types/generated';
import {
  Trash2, Play, Loader2, Check, X, ChevronRight,
  Settings, RefreshCw, AlertTriangle,
} from 'lucide-react';
import { RunDetail } from './RunDetail';
import '../../pages/WorkflowsPage.css';

const checkAgentRestricted = isAgentRestricted;

export interface LiveRunState {
  workflowId: string;
  currentStep: string | null;
  stepIndex: number;
  totalSteps: number;
  completedSteps: StepResult[];
  finished: boolean;
  status: string | null;
}

export interface WorkflowDetailProps {
  workflow: Workflow;
  runs: WorkflowRun[];
  liveRun: LiveRunState | null;
  onTrigger: () => void;
  onRefresh: () => void;
  onEdit: () => void;
  onDeleteRun: (runId: string) => void;
  onDeleteAllRuns: () => void;
  triggering: boolean;
  agentAccess?: AgentsConfig;
}

export function WorkflowDetail({ workflow, runs, liveRun, onTrigger, onRefresh, onEdit, onDeleteRun, onDeleteAllRuns, triggering, agentAccess }: WorkflowDetailProps) {
  const { t } = useT();
  const [showRuns, setShowRuns] = useState(true);

  const triggerLabel = (() => {
    switch (workflow.trigger.type) {
      case 'Cron': return `Cron: ${workflow.trigger.schedule}`;
      case 'Tracker': {
        const src = workflow.trigger.source;
        return `Tracker: ${src.owner}/${src.repo}`;
      }
      case 'Manual': return t('wf.manual');
      default: return t('wf.unknown');
    }
  })();

  return (
    <div className="wf-detail-panel">
      <div className="flex-row gap-6 mb-8">
        <h2 className="text-lg font-bold flex-1" style={{ margin: 0 }}>{workflow.name}</h2>
        <button className="wf-small-btn" onClick={onEdit}>
          <Settings size={10} /> {t('wf.edit')}
        </button>
        <button className="wf-small-btn" onClick={onRefresh}>
          <RefreshCw size={10} /> {t('wf.refresh')}
        </button>
        <button
          className="wf-small-btn wf-small-btn-accent"
          onClick={onTrigger}
          disabled={!workflow.enabled || triggering}
        >
          {triggering ? <Loader2 size={10} /> : <Play size={10} />}
          {t('wf.launch')}
        </button>
      </div>

      {/* Trigger info */}
      <div className="wf-info-row">
        <span className="wf-info-label">Trigger</span>
        <span>{triggerLabel}</span>
      </div>

      {workflow.concurrency_limit && (
        <div className="wf-info-row">
          <span className="wf-info-label">Concurrence max</span>
          <span>{workflow.concurrency_limit}</span>
        </div>
      )}

      {/* Steps */}
      <h3 className="wf-section-title">Steps ({workflow.steps.length})</h3>
      {workflow.steps.map((step, i) => (
        <div key={i} className="wf-step-card">
          <div className="flex-row gap-4">
            <span className="wf-step-number">{i + 1}</span>
            <span className="font-semibold text-md">{step.name}</span>
            <span className="text-xs font-semibold" style={{ color: AGENT_COLORS[step.agent] ?? '#888' }}>
              {AGENT_LABELS[step.agent] ?? step.agent}
            </span>
            {checkAgentRestricted(agentAccess, step.agent) && (
              <span className="flex-row gap-1 text-xs text-warning">
                <AlertTriangle size={10} />
                {t('config.restrictedStep')}
              </span>
            )}
          </div>
          <div className="wf-step-prompt-preview">
            {step.prompt_template.slice(0, 200)}{step.prompt_template.length > 200 ? '...' : ''}
          </div>
          {step.on_result && step.on_result.length > 0 && (
            <div className="mt-2 text-xs text-warning">
              {step.on_result.map((r, j) => (
                <span key={j}>{t('wiz.ifContains')} "{r.contains}" &rarr; {r.action.type} </span>
              ))}
            </div>
          )}
        </div>
      ))}

      {/* Live run progress */}
      {liveRun && liveRun.workflowId === workflow.id && !liveRun.finished && (
        <div className="wf-live-run">
          <div className="flex-row gap-4 mb-5">
            <Loader2 size={12} className="wf-spin" style={{ color: '#00d4ff' }} />
            <span className="text-base font-bold" style={{ color: '#00d4ff' }}>
              {t('wf.running')}
            </span>
            {liveRun.totalSteps > 0 && (
              <span className="text-xs text-muted">
                ({liveRun.completedSteps.length}/{liveRun.totalSteps} steps)
              </span>
            )}
          </div>

          {/* Progress bar */}
          {liveRun.totalSteps > 0 && (
            <div className="wf-progress-bar-track">
              <div
                className="wf-progress-bar-fill"
                style={{ width: `${(liveRun.completedSteps.length / liveRun.totalSteps) * 100}%` }}
              />
            </div>
          )}

          {/* Step indicators */}
          {workflow.steps.map((step, i) => {
            const completed = liveRun.completedSteps.find(s => s.step_name === step.name);
            const isCurrent = liveRun.currentStep === step.name;
            const isPending = !completed && !isCurrent;

            const stepState = completed ? 'completed' : isCurrent ? 'current' : 'pending';
            const iconState = completed
              ? (completed.status === 'Success' ? 'success' : 'failed')
              : isCurrent ? 'current' : 'pending';

            return (
              <div key={i} className="wf-live-step" data-state={isCurrent ? 'current' : 'other'}>
                {/* Status icon */}
                <span className="wf-live-step-icon" data-state={iconState}>
                  {completed ? (
                    completed.status === 'Success'
                      ? <Check size={9} className="text-success" />
                      : <X size={9} className="text-error" />
                  ) : isCurrent ? (
                    <span className="wf-live-step-pulse" />
                  ) : (
                    <span className="wf-live-step-dot-pending" />
                  )}
                </span>

                {/* Step name */}
                <span className="wf-live-step-name" data-state={stepState}>
                  {step.name}
                </span>

                {/* Duration for completed */}
                {completed && completed.duration_ms > 0 && (
                  <span className="text-2xs text-ghost">
                    {(completed.duration_ms / 1000).toFixed(1)}s
                  </span>
                )}

                {/* Current step indicator */}
                {isCurrent && (
                  <span className="wf-live-step-in-progress">
                    {t('wf.inProgress')}
                  </span>
                )}

                {isPending && (
                  <span className="text-2xs" style={{ color: 'rgba(255,255,255,0.1)' }}>
                    {t('wf.pending')}
                  </span>
                )}
              </div>
            );
          })}
        </div>
      )}

      {/* Live run finished banner */}
      {liveRun && liveRun.workflowId === workflow.id && liveRun.finished && (
        <div className="wf-live-finished" data-status={liveRun.status === 'Success' ? 'success' : 'failed'}>
          {liveRun.status === 'Success'
            ? <Check size={12} className="text-success" />
            : <X size={12} className="text-error" />
          }
          <span className="text-base font-semibold" style={{ color: liveRun.status === 'Success' ? '#34d399' : '#ff4d6a' }}>
            {t('wf.runDone', liveRun.status ?? '')}
          </span>
          <span className="text-xs text-dim">
            {t('wf.stepsExecuted', liveRun.completedSteps.length)}
          </span>
        </div>
      )}

      {/* Runs */}
      <div className="flex-row gap-4 mt-8">
        <h3 className="wf-section-title flex-1" style={{ margin: 0 }}>
          Runs ({runs.length})
        </h3>
        {runs.length > 0 && (
          <button
            className="wf-small-btn wf-small-btn-delete-all"
            onClick={onDeleteAllRuns}
          >
            <Trash2 size={9} /> {t('wf.deleteAll')}
          </button>
        )}
        <button className="wf-icon-btn" onClick={() => setShowRuns(!showRuns)} aria-label={showRuns ? 'Collapse runs' : 'Expand runs'}>
          <ChevronRight size={12} className={showRuns ? 'wf-chevron-rotated' : 'wf-chevron'} />
        </button>
      </div>

      {showRuns && runs.length === 0 && (
        <p className="text-sm text-faint mt-4">{t('wf.noRuns')}</p>
      )}

      {showRuns && runs.map(run => (
        <RunDetail key={run.id} run={run} workflowSteps={workflow.steps} onDelete={() => onDeleteRun(run.id)} />
      ))}
    </div>
  );
}
