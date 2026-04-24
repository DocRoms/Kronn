import { useState, useEffect } from 'react';
import { useT } from '../../lib/I18nContext';
import type { WorkflowRun, WorkflowStep } from '../../types/generated';
import { Trash2, ChevronRight, Square, Loader2, Plug, Send, Layers } from 'lucide-react';
import { AGENT_LABELS, AGENT_COLORS } from '../../lib/constants';
import '../../pages/WorkflowsPage.css';

const STATUS_COLORS: Record<string, string> = {
  Pending: 'var(--kr-warning)',
  Running: 'var(--kr-cyan)',
  Success: 'var(--kr-success)',
  Failed: 'var(--kr-error)',
  Cancelled: 'var(--kr-cancelled)',
  WaitingApproval: 'var(--kr-accent-ink)',
};

export interface RunDetailProps {
  run: WorkflowRun;
  workflowSteps?: WorkflowStep[];
  onDelete: () => void;
  /** Cancel a Running workflow run. Button is only rendered for Running status. */
  onCancel?: () => void;
}

/** Live counter for the step that's currently running. The exact start
 *  timestamp isn't tracked in `step_results` (which only carries
 *  `duration_ms` post-completion), so we estimate: start = run.started_at
 *  + sum of completed steps' durations. Good enough to show the user
 *  "this step has been running for 23s" — way better than a static
 *  "running..." that screams "the page is frozen". Ticks every second
 *  via setInterval; effect cleaned up on unmount or status change. */
function LiveStepStatus({
  run,
  step,
  stepIndex,
  t,
}: {
  run: WorkflowRun;
  step: WorkflowStep;
  stepIndex: number;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  const runStart = new Date(run.started_at).getTime();
  const completedDurationMs = run.step_results
    .slice(0, stepIndex)
    .reduce((acc, sr) => acc + (sr.duration_ms || 0), 0);
  const stepStart = runStart + completedDurationMs;
  const elapsedSec = Math.max(0, Math.floor((now - stepStart) / 1000));

  // Step-type-aware activity hint. Generic "running..." was a black
  // box — telling the user *what kind* of activity is in flight (HTTP
  // call, agent reasoning, webhook, fan-out) keeps them oriented.
  const kind = step.step_type?.type ?? 'Agent';
  const activity = kind === 'ApiCall' ? t('wf.liveStep.api')
    : kind === 'Notify' ? t('wf.liveStep.notify')
    : kind === 'BatchQuickPrompt' ? t('wf.liveStep.batch')
    : t('wf.liveStep.agent');

  return (
    <span className="wf-live-step-status">
      <Loader2 size={10} className="spin" style={{ color: 'var(--kr-warning)' }} />
      <span className="wf-live-step-activity">{activity}</span>
      <span className="wf-live-step-elapsed">{elapsedSec}s</span>
    </span>
  );
}

export function RunDetail({ run, workflowSteps, onDelete, onCancel }: RunDetailProps) {
  const { t } = useT();
  const [expandedStep, setExpandedStep] = useState<number | null>(null);

  const CONDITION_LABELS: Record<string, string> = {
    Stop: 'Stop',
    Skip: t('wf.skipNext'),
  };

  const conditionLabel = (cr: string): string => {
    if (cr.startsWith('Goto:')) return `Goto \u2192 ${cr.slice(5)}`;
    return CONDITION_LABELS[cr] ?? cr;
  };

  return (
    <div className="wf-run-card">
      <div className="flex-row gap-4">
        <span
          className="wf-step-dot"
          style={{ width: 8, height: 8, background: STATUS_COLORS[run.status] ?? 'var(--kr-text-faint)' }}
        />
        <span className="font-semibold text-base" style={{ color: STATUS_COLORS[run.status] ?? 'var(--kr-text-faint)' }}>
          {run.status}
        </span>
        <span className="text-xs text-muted flex-1">
          {new Date(run.started_at).toLocaleString()}
          {run.finished_at && ` \u2014 ${new Date(run.finished_at).toLocaleString()}`}
        </span>
        {run.tokens_used > 0 && (
          <span className="text-xs text-muted">{run.tokens_used} tokens</span>
        )}
        {run.status === 'Running' && onCancel && (
          <button
            className="wf-run-cancel-btn"
            onClick={(e) => {
              e.stopPropagation();
              if (confirm(t('wf.cancelRunConfirm'))) onCancel();
            }}
            title={t('wf.cancelRun')}
          >
            <Square size={10} style={{ fill: 'currentColor' }} />
            {t('wf.cancelRun')}
          </button>
        )}
        <button
          className="wf-run-delete-btn"
          onClick={(e) => { e.stopPropagation(); onDelete(); }}
          title={t('wf.deleteRun')}
        >
          <Trash2 size={10} />
        </button>
      </div>

      {/* Step progress — show workflow steps with completion status when running */}
      {run.status === 'Running' && workflowSteps && workflowSteps.length > 0 && (
        <div className="mt-4 py-2">
          {workflowSteps.map((ws_step, i) => {
            const completed = run.step_results.find(sr => sr.step_name === ws_step.name);
            const isNext = !completed && run.step_results.length === i;
            return (
              <div key={i} className="flex-row gap-3 text-sm" style={{ padding: '3px 0' }}>
                <span
                  className="wf-step-dot"
                  style={{
                    background: completed ? (STATUS_COLORS[completed.status] ?? 'var(--kr-text-faint)') : isNext ? 'var(--kr-warning)' : 'var(--kr-border-medium)',
                    animation: isNext ? 'pulse 1.5s ease-in-out infinite' : undefined,
                  }}
                />
                <span style={{
                  fontWeight: isNext ? 600 : 400,
                  color: completed ? 'var(--kr-text-secondary)' : isNext ? 'var(--kr-warning)' : 'var(--kr-text-ghost)',
                }}>
                  {ws_step.name}
                </span>
                {ws_step.step_type && (
                  <span className="wf-step-type-badge" data-type={
                    ws_step.step_type.type === 'ApiCall' ? 'api'
                      : ws_step.step_type.type === 'Notify' ? 'notify'
                      : ws_step.step_type.type === 'BatchQuickPrompt' ? 'batch'
                      : 'agent'
                  }>
                    {ws_step.step_type.type === 'ApiCall' ? 'API'
                      : ws_step.step_type.type === 'Notify' ? 'NOTIFY'
                      : ws_step.step_type.type === 'BatchQuickPrompt' ? 'BATCH'
                      : 'AGENT'}
                  </span>
                )}
                {ws_step.description && (
                  <span className="text-ghost text-xs" style={{ fontStyle: 'italic' }}>{ws_step.description}</span>
                )}
                {completed && completed.duration_ms > 0 && (
                  <span className="text-ghost text-xs">{(completed.duration_ms / 1000).toFixed(1)}s</span>
                )}
                {isNext && (
                  <LiveStepStatus run={run} step={ws_step} stepIndex={i} t={t} />
                )}
              </div>
            );
          })}
        </div>
      )}

      {/* Step results (completed runs) */}
      {run.step_results.length > 0 && run.status !== 'Running' && (
        <div className="mt-4">
          {run.step_results.map((sr, i) => {
            const isExpanded = expandedStep === i;
            return (
              <div key={i} className="wf-step-result" data-expanded={isExpanded}>
                <button
                  className="wf-step-result-btn"
                  data-expanded={isExpanded}
                  onClick={() => setExpandedStep(isExpanded ? null : i)}
                  aria-expanded={isExpanded}
                >
                  <span
                    className="wf-step-result-dot"
                    style={{ background: STATUS_COLORS[sr.status] ?? 'var(--kr-text-faint)' }}
                  />
                  <span className="font-semibold">{sr.step_name}</span>
                  {/* Snapshot of what was actually used for this step at
                      run time — frozen on the row so editing the workflow
                      afterwards (swapping agent, retargeting plugin) keeps
                      the run history honest. Null = legacy row written
                      before the field shipped — fall back to nothing. */}
                  {sr.step_kind === 'ApiCall' && (
                    <span className="wf-step-kind-badge" data-kind="api">
                      <Plug size={9} /> API
                      {(sr.step_api_plugin_slug || sr.step_api_endpoint_path) && (
                        <span className="text-xs text-ghost" style={{ fontWeight: 400, marginLeft: 4 }}>
                          {sr.step_api_plugin_slug ?? '?'}
                          {sr.step_api_endpoint_path ? ` · ${sr.step_api_endpoint_path}` : ''}
                        </span>
                      )}
                    </span>
                  )}
                  {sr.step_kind === 'Notify' && (
                    <span className="wf-step-kind-badge" data-kind="notify">
                      <Send size={9} /> NOTIFY
                    </span>
                  )}
                  {sr.step_kind === 'BatchQuickPrompt' && (
                    <span className="wf-step-kind-badge">
                      <Layers size={9} /> BATCH
                    </span>
                  )}
                  {sr.step_kind === 'Agent' && sr.step_agent && (
                    <span className="text-xs font-semibold" style={{ color: AGENT_COLORS[sr.step_agent] ?? 'var(--kr-text-faint)' }}>
                      {AGENT_LABELS[sr.step_agent] ?? sr.step_agent}
                    </span>
                  )}
                  <span className="text-ghost">
                    {sr.duration_ms > 0 ? `${(sr.duration_ms / 1000).toFixed(1)}s` : ''}
                  </span>
                  {sr.condition_result && (
                    <span className="wf-condition-badge" data-action={sr.condition_result === 'Stop' ? 'stop' : 'other'}>
                      {conditionLabel(sr.condition_result)}
                    </span>
                  )}
                  {!sr.condition_result && sr.status === 'Success' && i < run.step_results.length - 1 && (
                    <span className="text-2xs" style={{ color: 'rgba(var(--kr-success-rgb), 0.5)' }}>&rarr; suite</span>
                  )}
                  <span className="flex-1" />
                  {!isExpanded && sr.output && (
                    <span className="wf-step-output-preview">
                      {sr.output.slice(0, 80)}
                    </span>
                  )}
                  <ChevronRight size={10} className={isExpanded ? 'wf-chevron-rotated text-ghost flex-shrink-0' : 'wf-chevron text-ghost flex-shrink-0'} />
                </button>

                {isExpanded && (
                  <div className="wf-step-output-full">
                    <div className="wf-step-output-code">
                      {sr.output || t('wf.noOutput')}
                    </div>
                    <div className="flex-row gap-6 mt-3 text-xs text-faint">
                      <span>{t('wf.status')}: <span style={{ color: STATUS_COLORS[sr.status] ?? 'var(--kr-text-faint)' }}>{sr.status}</span></span>
                      {sr.duration_ms > 0 && <span>{t('wf.duration')}: {(sr.duration_ms / 1000).toFixed(1)}s</span>}
                      {sr.tokens_used > 0 && <span>Tokens: {sr.tokens_used}</span>}
                      {sr.condition_result && <span>Condition: <span className="text-warning">{conditionLabel(sr.condition_result)}</span></span>}
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
