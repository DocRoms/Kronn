import { useState, useRef } from 'react';
import { useT } from '../../lib/I18nContext';
import { workflows as workflowsApi } from '../../lib/api';
import { AGENT_COLORS, AGENT_LABELS, isAgentRestricted } from '../../lib/constants';
import type { Workflow, WorkflowRun, StepResult, AgentsConfig, WorkflowStep } from '../../types/generated';
import {
  Trash2, Play, Loader2, Check, X, ChevronRight,
  Settings, RefreshCw, AlertTriangle, FlaskConical,
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

function StepCard({ step, index, agentAccess, projectId, t }: {
  step: WorkflowStep; index: number; agentAccess?: AgentsConfig | null;
  projectId?: string | null; t: (key: string, ...args: (string | number)[]) => string;
}) {
  const [expanded, setExpanded] = useState(false);
  const [testOpen, setTestOpen] = useState(false);
  const [testMockInput, setTestMockInput] = useState('');
  const [testDryRun, setTestDryRun] = useState(true);
  const [testRunning, setTestRunning] = useState(false);
  const [testResult, setTestResult] = useState<StepResult | null>(null);
  const [testError, setTestError] = useState<string | null>(null);
  const [testLiveOutput, setTestLiveOutput] = useState('');
  const [testElapsed, setTestElapsed] = useState(0);
  const abortRef = useRef<AbortController | null>(null);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const liveOutputRef = useRef<HTMLPreElement | null>(null);
  const isLong = step.prompt_template.length > 200;

  const handleTest = async () => {
    setTestRunning(true);
    setTestResult(null);
    setTestError(null);
    setTestElapsed(0);
    setTestLiveOutput('');
    const controller = new AbortController();
    abortRef.current = controller;

    const start = Date.now();
    timerRef.current = setInterval(() => setTestElapsed(Math.floor((Date.now() - start) / 1000)), 1000);

    await workflowsApi.testStepStream(
      {
        step,
        project_id: projectId ?? null,
        mock_previous_output: testMockInput || null,
        dry_run: testDryRun,
      },
      () => {},
      (result) => {
        setTestResult(result);
      },
      () => {
        setTestRunning(false);
        if (timerRef.current) clearInterval(timerRef.current);
      },
      (err) => {
        setTestError(err);
        setTestRunning(false);
        if (timerRef.current) clearInterval(timerRef.current);
      },
      controller.signal,
      (text) => {
        setTestLiveOutput(prev => {
          const updated = prev + text;
          // Auto-scroll to bottom
          requestAnimationFrame(() => {
            if (liveOutputRef.current) {
              liveOutputRef.current.scrollTop = liveOutputRef.current.scrollHeight;
            }
          });
          return updated;
        });
      },
    );
    setTestRunning(false);
    if (timerRef.current) clearInterval(timerRef.current);
  };

  return (
    <div className="wf-step-card">
      <div className="flex-row gap-4">
        <span className="wf-step-number">{index + 1}</span>
        <span className="font-semibold text-md">{step.name}</span>
        <span className="text-xs font-semibold" style={{ color: AGENT_COLORS[step.agent] ?? '#888' }}>
          {AGENT_LABELS[step.agent] ?? step.agent}
        </span>
        {checkAgentRestricted(agentAccess ?? undefined, step.agent) && (
          <span className="flex-row gap-1 text-xs text-warning">
            <AlertTriangle size={10} />
            {t('config.restrictedStep')}
          </span>
        )}
        <button
          className="wf-test-btn"
          onClick={() => { if (!testRunning) setTestOpen(!testOpen); }}
          title={t('wiz.testStep')}
          data-active={testOpen || testRunning}
        >
          {testRunning ? <Loader2 size={11} className="spin" /> : <FlaskConical size={11} />}
          {testRunning ? `${t('wiz.testRunning')} ${testElapsed}s` : t('wiz.testStep')}
        </button>
      </div>
      <div
        className="wf-step-prompt-preview"
        style={expanded ? { maxHeight: 'none' } : undefined}
      >
        {expanded || !isLong ? step.prompt_template : step.prompt_template.slice(0, 200) + '...'}
      </div>
      {isLong && (
        <button
          className="wf-expand-btn"
          onClick={() => setExpanded(!expanded)}
          style={{ background: 'none', border: 'none', cursor: 'pointer', padding: '2px 0', fontSize: 12, color: 'var(--kr-text-muted)', display: 'flex', alignItems: 'center', gap: 4 }}
        >
          <ChevronRight size={10} style={{ transform: expanded ? 'rotate(90deg)' : 'none', transition: 'transform 0.15s' }} />
          {expanded ? t('wiz.showLess') : t('wiz.showMore')}
        </button>
      )}
      {step.on_result && step.on_result.length > 0 && (
        <div className="mt-2 text-xs text-warning">
          {step.on_result.map((r, j) => (
            <span key={j}>{t('wiz.ifContains')} "{r.contains}" &rarr; {r.action.type} </span>
          ))}
        </div>
      )}

      {/* Test panel */}
      {testOpen && (
        <div className="wf-test-panel">
          <p className="wf-test-info">{t('wiz.testInfo')}</p>
          {index > 0 && (
            <>
              <label className="text-xs text-muted mb-1">{t('wiz.testMockInput')}</label>
              <textarea
                className="wf-test-textarea"
                rows={4}
                value={testMockInput}
                onChange={e => setTestMockInput(e.target.value)}
                placeholder={t('wiz.testMockPlaceholder')}
              />
            </>
          )}
          <div className="wf-test-actions">
            <label className="wf-test-dryrun-toggle">
              <input type="checkbox" checked={testDryRun} onChange={e => setTestDryRun(e.target.checked)} />
              <span>{t('wiz.testDryRun')}</span>
            </label>
            <div className="wf-test-actions-right">
              {testRunning && (
                <button className="wf-test-cancel-btn" onClick={() => abortRef.current?.abort()}>
                  <X size={12} /> {t('wiz.testCancel')}
                </button>
              )}
              <button
                className="wf-test-run-btn"
                onClick={handleTest}
                disabled={testRunning}
              >
                {testRunning ? <Loader2 size={14} className="spin" /> : <Play size={14} />}
                {testRunning ? t('wiz.testRunning') : t('wiz.testRun')}
              </button>
            </div>
          </div>
          {/* Live streaming output */}
          {(testRunning || testLiveOutput) && (
            <div className="wf-test-live mt-2">
              {testRunning && (
                <div className="wf-test-live-header">
                  <Loader2 size={12} className="spin" />
                  <span>{t('wiz.testRunning')}</span>
                  <span className="wf-test-elapsed">{testElapsed}s</span>
                </div>
              )}
              <pre className="wf-test-live-output" ref={liveOutputRef}>
                {testLiveOutput || t('wiz.testLogConnecting')}
              </pre>
            </div>
          )}
          {testError && <div className="wf-test-error mt-2">{testError}</div>}
          {testResult && (
            <div className="wf-test-result mt-2">
              <div className="flex-row gap-3 mb-1">
                <span className={`wf-test-status wf-test-status-${testResult.status.toLowerCase()}`}>
                  {testResult.status}
                </span>
                <span className="text-xs text-muted">{testResult.tokens_used} tokens — {(testResult.duration_ms / 1000).toFixed(1)}s</span>
              </div>
              <pre className="wf-test-output">{testResult.output}</pre>
            </div>
          )}
        </div>
      )}
    </div>
  );
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
        <StepCard key={i} step={step} index={i} agentAccess={agentAccess} projectId={workflow.project_id} t={t} />
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
