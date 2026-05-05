import { useState, useEffect } from 'react';
import { useT } from '../../lib/I18nContext';
import { workflows as workflowsApi } from '../../lib/api';
import type { WorkflowRun, WorkflowStep, DecideRunRequest, ProducedBranch } from '../../types/generated';
import { Trash2, ChevronRight, Square, Loader2, Plug, Send, Layers, Shield, Hand, Check, X, RotateCcw, Terminal, GitBranch, Copy, FlaskConical, AlertTriangle } from 'lucide-react';
import { AGENT_LABELS, AGENT_COLORS } from '../../lib/constants';
import '../../pages/WorkflowsPage.css';

export type GateDecisionKind = 'approve' | 'request_changes' | 'reject';

const STATUS_COLORS: Record<string, string> = {
  Pending: 'var(--kr-warning)',
  Running: 'var(--kr-cyan)',
  Success: 'var(--kr-success)',
  Failed: 'var(--kr-error)',
  Cancelled: 'var(--kr-cancelled)',
  WaitingApproval: 'var(--kr-accent-ink)',
  // 0.7.0 — guard-stopped runs are amber (self-protection, not failure).
  StoppedByGuard: 'var(--kr-warning)',
};

export interface RunDetailProps {
  run: WorkflowRun;
  workflowSteps?: WorkflowStep[];
  onDelete: () => void;
  /** Cancel a Running workflow run. Button is only rendered for Running status. */
  onCancel?: () => void;
  /** 0.7.0 Phase 4 — submit a gate decision (approve / request_changes / reject). */
  onDecide?: (payload: DecideRunRequest) => Promise<void> | void;
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
    : kind === 'Gate' ? t('wf.liveStep.gate')
    : kind === 'Exec' ? t('wf.liveStep.exec')
    : t('wf.liveStep.agent');

  return (
    <span className="wf-live-step-status">
      <Loader2 size={10} className="spin" style={{ color: 'var(--kr-warning)' }} />
      <span className="wf-live-step-activity">{activity}</span>
      <span className="wf-live-step-elapsed">{elapsedSec}s</span>
    </span>
  );
}

/** B5 (0.7.0 UX pass) — counter showing how long a run has been paused
 *  on a Gate. The pause start = run.started_at + sum of completed step
 *  durations (same arithmetic as LiveStepStatus). Ticks every minute
 *  for &gt;1min, every second below. Returns a human "il y a 2h17". */
function PausedSince({
  run,
  t,
}: {
  run: WorkflowRun;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    // 30s tick is enough — past the first minute, sub-minute precision
    // doesn't matter to the operator.
    const id = window.setInterval(() => setNow(Date.now()), 30000);
    return () => window.clearInterval(id);
  }, []);

  const runStart = new Date(run.started_at).getTime();
  // Sum of all step durations excluding the trailing WaitingApproval one
  // (its duration_ms is the time the gate executor took to render the
  // message, ~0ms — it's NOT the pause duration).
  const completedDurationMs = run.step_results
    .slice(0, -1)
    .reduce((acc, sr) => acc + (sr.duration_ms || 0), 0);
  const pauseStart = runStart + completedDurationMs;
  const elapsedMs = Math.max(0, now - pauseStart);
  const elapsedMin = Math.floor(elapsedMs / 60000);

  let label: string;
  if (elapsedMin < 1) label = t('wf.pausedJustNow');
  else if (elapsedMin < 60) label = t('wf.pausedMinutes', elapsedMin);
  else if (elapsedMin < 60 * 24) {
    const h = Math.floor(elapsedMin / 60);
    const m = elapsedMin % 60;
    label = m > 0 ? t('wf.pausedHoursMinutes', h, m) : t('wf.pausedHours', h);
  } else {
    const d = Math.floor(elapsedMin / (60 * 24));
    label = t('wf.pausedDays', d);
  }
  return <span className="wf-paused-since">{label}</span>;
}

/** 0.7.0 Phase 4 — interactive panel rendered when the run is paused on a
 *  Gate step. Shows the operator the rendered message + 3 decision buttons.
 *  The "Request changes" path requires a non-empty comment (the agent
 *  needs feedback to act on); the other two make the comment optional. */
function GatePanel({
  message,
  onDecide,
  t,
}: {
  message: string;
  onDecide: (payload: DecideRunRequest) => Promise<void> | void;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  const [comment, setComment] = useState('');
  const [submitting, setSubmitting] = useState<GateDecisionKind | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handle = async (decision: GateDecisionKind) => {
    if (decision === 'request_changes' && comment.trim() === '') {
      setError(t('wf.gate.commentRequired'));
      return;
    }
    setError(null);
    setSubmitting(decision);
    try {
      await onDecide({ decision, comment: comment.trim() || null });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setSubmitting(null);
    }
  };

  return (
    <div className="wf-gate-panel">
      <div className="wf-gate-header">
        <Hand size={14} style={{ color: 'var(--kr-accent-ink)' }} />
        <span className="font-semibold">{t('wf.gate.title')}</span>
      </div>
      <div className="wf-gate-message">
        {message || t('wf.gate.defaultMessage')}
      </div>
      <textarea
        className="wf-gate-comment"
        placeholder={t('wf.gate.commentPlaceholder')}
        value={comment}
        onChange={(e) => setComment(e.target.value)}
        rows={3}
        disabled={submitting !== null}
      />
      {error && <div className="wf-gate-error">{error}</div>}
      <div className="wf-gate-actions">
        <button
          className="wf-gate-btn wf-gate-btn--approve"
          onClick={() => handle('approve')}
          disabled={submitting !== null}
        >
          {submitting === 'approve'
            ? <Loader2 size={12} className="spin" />
            : <Check size={12} />}
          {t('wf.gate.approve')}
        </button>
        <button
          className="wf-gate-btn wf-gate-btn--changes"
          onClick={() => handle('request_changes')}
          disabled={submitting !== null}
        >
          {submitting === 'request_changes'
            ? <Loader2 size={12} className="spin" />
            : <RotateCcw size={12} />}
          {t('wf.gate.requestChanges')}
        </button>
        <button
          className="wf-gate-btn wf-gate-btn--reject"
          onClick={() => handle('reject')}
          disabled={submitting !== null}
        >
          {submitting === 'reject'
            ? <Loader2 size={12} className="spin" />
            : <X size={12} />}
          {t('wf.gate.reject')}
        </button>
      </div>
    </div>
  );
}

/** 0.7.0 — surfaces commits the agent produced in its worktree but that
 *  outlived the cleanup (typically because the push step failed). Without
 *  this, the operator's only recourse is `git reflog` + dangling commit
 *  archaeology — visible to none but the very-comfortable git users.
 *
 *  Each row exposes:
 *    - Branch name + short SHA + commit-count-ahead vs base
 *    - "Push status" pill (✓ pushed | ⚠ local-only)
 *    - Copy git-checkout / git-diff command (no shell required)
 *    - "Tester en worktree" button — POSTs to /test-worktree, returns a
 *      path the operator can `cd` into. Stays as a separate worktree to
 *      avoid clobbering main.
 */
function ProducedBranchesPanel({
  run,
  branches,
  t,
}: {
  run: WorkflowRun;
  branches: ProducedBranch[];
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  type State = { kind: 'idle' } | { kind: 'creating' } | { kind: 'ready', path: string } | { kind: 'error', msg: string };
  const [state, setState] = useState<Record<number, State>>({});

  const setStateFor = (i: number, s: State) =>
    setState(prev => ({ ...prev, [i]: s }));

  const onCreate = async (i: number) => {
    setStateFor(i, { kind: 'creating' });
    try {
      // `api()` unwraps the envelope and throws on error — the resolved
      // value is `{worktree_path, branch_name, head_sha}` directly.
      const res = await workflowsApi.createTestWorktree(run.workflow_id, run.id, i);
      setStateFor(i, { kind: 'ready', path: res.worktree_path });
    } catch (e) {
      setStateFor(i, { kind: 'error', msg: e instanceof Error ? e.message : String(e) });
    }
  };

  const onCleanup = async (i: number) => {
    try {
      await workflowsApi.deleteTestWorktree(run.workflow_id, run.id);
      setStateFor(i, { kind: 'idle' });
    } catch { /* swallow */ }
  };

  const copy = (s: string) => navigator.clipboard?.writeText(s).catch(() => {});

  return (
    <div className="wf-produced-branches mt-3">
      <div className="wf-produced-branches-title">
        <GitBranch size={11} />
        <span>{t('wf.produced.title', branches.length)}</span>
      </div>
      {branches.map((b, i) => {
        const st = state[i] ?? { kind: 'idle' };
        const checkoutCmd = `git checkout ${b.branch_name}`;
        const diffCmd = `git log -p ${b.head_sha}~..${b.head_sha}`;
        return (
          <div key={i} className="wf-produced-branch-row">
            <div className="wf-produced-branch-head">
              <code title={b.branch_name}>{b.branch_name}</code>
              <span className="text-xs text-faint">·</span>
              <code className="text-xs" title={b.head_sha}>{b.head_sha.slice(0, 8)}</code>
              <span className="text-xs text-faint">·</span>
              <span className="text-xs text-muted">{t('wf.produced.aheadCount', b.ahead)}</span>
              {b.pushed_upstream ? (
                <span className="wf-produced-pill wf-produced-pill--pushed">
                  <Check size={9} /> {t('wf.produced.pushed')}
                </span>
              ) : (
                <span className="wf-produced-pill wf-produced-pill--local">
                  <AlertTriangle size={9} /> {t('wf.produced.localOnly')}
                </span>
              )}
            </div>
            <div className="wf-produced-branch-actions">
              <button
                type="button"
                className="wf-small-btn"
                onClick={() => copy(checkoutCmd)}
                title={t('wf.produced.copyCheckout')}
              >
                <Copy size={9} /> {t('wf.produced.copyCheckout')}
              </button>
              <button
                type="button"
                className="wf-small-btn"
                onClick={() => copy(diffCmd)}
                title={t('wf.produced.copyDiff')}
              >
                <Copy size={9} /> {t('wf.produced.copyDiff')}
              </button>
              {st.kind === 'idle' && (
                <button
                  type="button"
                  className="wf-small-btn wf-produced-test-btn"
                  onClick={() => onCreate(i)}
                  title={t('wf.produced.testHint')}
                >
                  <FlaskConical size={9} /> {t('wf.produced.test')}
                </button>
              )}
              {st.kind === 'creating' && (
                <span className="text-xs text-muted"><Loader2 size={9} className="spin" /> {t('wf.produced.creating')}</span>
              )}
              {st.kind === 'ready' && (
                <>
                  <button
                    type="button"
                    className="wf-small-btn"
                    onClick={() => copy(`cd "${st.path}"`)}
                    title={t('wf.produced.copyCd')}
                  >
                    <Copy size={9} /> cd
                  </button>
                  <code className="wf-produced-test-path" title={st.path}>{st.path}</code>
                  <button
                    type="button"
                    className="wf-small-btn wf-small-btn-delete-all"
                    onClick={() => onCleanup(i)}
                    title={t('wf.produced.cleanup')}
                  >
                    <Trash2 size={9} /> {t('wf.produced.cleanup')}
                  </button>
                </>
              )}
              {st.kind === 'error' && (
                <span className="wf-produced-error" title={st.msg}>
                  <AlertTriangle size={9} /> {st.msg.slice(0, 80)}
                </span>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function RunDetail({ run, workflowSteps, onDelete, onCancel, onDecide }: RunDetailProps) {
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
        {/* B5 (0.7.0 UX pass) \u2014 WaitingApproval gets a hand icon + a
            distinct "\u00c0 VALIDER" badge instead of the generic status dot.
            Marie + Antony + Cyndie : trois personas pour qui les runs
            en pause se noient dans la liste. Pulse animation discr\u00e8te
            pour attirer l'\u0153il sans surcharger. */}
        {run.status === 'WaitingApproval' ? (
          <Hand size={14} style={{ color: 'var(--kr-warning)' }} />
        ) : run.status === 'StoppedByGuard' ? (
          <Shield size={12} style={{ color: 'var(--kr-warning)' }} />
        ) : (
          <span
            className="wf-step-dot"
            style={{ width: 8, height: 8, background: STATUS_COLORS[run.status] ?? 'var(--kr-text-faint)' }}
          />
        )}
        {run.status === 'WaitingApproval' ? (
          <span className="wf-run-status-badge wf-run-status-badge--gate">
            {t('wf.runStatusToReview')}
          </span>
        ) : (
          <span className="font-semibold text-base" style={{ color: STATUS_COLORS[run.status] ?? 'var(--kr-text-faint)' }}>
            {run.status === 'StoppedByGuard' ? t('wf.guards.stoppedBy.title') : run.status}
          </span>
        )}
        <span className="text-xs text-muted flex-1">
          {new Date(run.started_at).toLocaleString()}
          {run.finished_at && ` \u2014 ${new Date(run.finished_at).toLocaleString()}`}
          {run.status === 'WaitingApproval' && (
            <>
              {' \u2014 '}
              <PausedSince run={run} t={t} />
              {' \u00b7 '}
              <span className="wf-paused-zero-tokens" title={t('wf.pausedZeroTokensHint')}>
                {t('wf.pausedZeroTokens')}
              </span>
            </>
          )}
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

      {/* Worktree path — surfaces the actual filesystem location of the run's
          isolated git worktree. The wizard chips promise isolation; this row
          is where the promise becomes tangible: the user can cd into it,
          inspect artefacts, or clone elsewhere. Hidden when no project bound
          (workspace_path is null) — ApiCall-only / Notify-only workflows
          legitimately have no worktree. */}
      {run.workspace_path && (
        <div className="wf-run-worktree-row mt-3">
          <span title={t('wiz.worktreeIconTooltip')} style={{ cursor: 'help', display: 'inline-flex' }}>
            <GitBranch size={11} />
          </span>
          <span className="text-xs text-muted">{t('wf.runWorktreePath')}</span>
          <code className="wf-run-worktree-path" title={run.workspace_path}>
            {run.workspace_path}
          </code>
          <button
            type="button"
            className="wf-run-worktree-copy"
            onClick={(e) => {
              e.stopPropagation();
              navigator.clipboard?.writeText(run.workspace_path ?? '').catch(() => {});
            }}
            title={t('wf.runWorktreeCopy')}
            aria-label={t('wf.runWorktreeCopy')}
          >
            <Copy size={10} />
          </button>
        </div>
      )}
      {/* 0.7.0 — Produced branches panel. Surfaces commits the agent
          made in its worktree but that were preserved on cleanup
          (typically because push failed). The agent's work is otherwise
          invisible to non-git-experts. */}
      {run.produced_branches && run.produced_branches.length > 0 && (
        <ProducedBranchesPanel run={run} branches={run.produced_branches} t={t} />
      )}

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
                      : ws_step.step_type.type === 'Gate' ? 'gate'
                      : ws_step.step_type.type === 'Exec' ? 'exec'
                      : 'agent'
                  }>
                    {ws_step.step_type.type === 'ApiCall' ? 'API'
                      : ws_step.step_type.type === 'Notify' ? 'NOTIFY'
                      : ws_step.step_type.type === 'BatchQuickPrompt' ? 'BATCH'
                      : ws_step.step_type.type === 'Gate' ? 'GATE'
                      : ws_step.step_type.type === 'Exec' ? 'EXEC'
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

      {/* 0.7.0 Phase 4 — Gate decision panel when paused awaiting approval.
          Rendered ABOVE the step list so the operator's eyes land on the
          decision UI first; the step history is collapsible context behind
          it. The panel is only shown when:
            - run is Waiting (server-side state)
            - the trailing step is a Gate (so we have a message to show)
            - onDecide handler is wired (caller opted in to gating) */}
      {run.status === 'WaitingApproval' && onDecide && (() => {
        const last = run.step_results[run.step_results.length - 1];
        if (!last || last.step_kind !== 'Gate') return null;
        return (
          <GatePanel
            message={last.output}
            onDecide={onDecide}
            t={t}
          />
        );
      })()}

      {/* Step results — clickable, expandable details for every completed
          step. Only rendered for finished runs; during a Running run the
          parent (`WorkflowDetail`) shows a unified live view with the
          same expand-per-step affordance, fed by SSE events instead of
          this static `run.step_results` snapshot which would only update
          on page reload. Splitting the responsibility avoids two parallel
          step lists fighting for the user's attention. */}
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
                  {sr.step_kind === 'Gate' && (
                    <span className="wf-step-kind-badge" data-kind="gate">
                      <Hand size={9} /> GATE
                    </span>
                  )}
                  {sr.step_kind === 'Exec' && (
                    <span className="wf-step-kind-badge" data-kind="exec">
                      <Terminal size={9} /> EXEC
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
