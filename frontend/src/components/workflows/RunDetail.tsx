import { useState, useEffect } from 'react';
import { useT } from '../../lib/I18nContext';
import { workflows as workflowsApi } from '../../lib/api';
import type { WorkflowRun, WorkflowStep, DecideRunRequest, ProducedBranch } from '../../types/generated';
import { Trash2, ChevronRight, Square, Loader2, Plug, Send, Layers, Shield, Hand, Check, X, RotateCcw, Terminal, GitBranch, Copy, FlaskConical, AlertTriangle, CornerDownRight } from 'lucide-react';
import { AGENT_LABELS, AGENT_COLORS } from '../../lib/constants';
import { parseForeachEnvelope, isZeroTokenItem } from '../../lib/foreach-envelope';
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
  // 0.8.11 — backend died mid-run (crash/restart). Neutral grey, not red:
  // the workflow didn't fail, the host went away.
  Interrupted: 'var(--kr-text-ghost)',
};

export interface RunDetailProps {
  run: WorkflowRun;
  workflowSteps?: WorkflowStep[];
  onDelete: () => void;
  /** Cancel a Running workflow run. Button is only rendered for Running status. */
  onCancel?: () => void;
  /** 0.7.0 Phase 4 — submit a gate decision (approve / request_changes / reject). */
  onDecide?: (payload: DecideRunRequest) => Promise<void> | void;
  /** 2026-06-13 — jump to a (sub-)workflow's run list, e.g. from a fan-out
   *  per-task row to the child sub-workflow that ran that task. */
  onNavigateToWorkflow?: (workflowId: string) => void;
  /** #11 — jump to a SPECIFIC child run (workflow + run id). Falls back to
   *  onNavigateToWorkflow when not provided. */
  onNavigateToRun?: (workflowId: string, runId: string) => void;
}

/** Live counter for the step that's currently running. The exact start
 *  timestamp isn't tracked in `step_results` (which only carries
 *  `duration_ms` post-completion), so we estimate: start = run.started_at
 *  + sum of completed steps' durations. Good enough to show the user
 *  "this step has been running for 23s" — way better than a static
 *  "running..." that screams "the page is frozen". Ticks every second
 *  via setInterval; effect cleaned up on unmount or status change. */

/** 2026-06-13 — live fan-out progress for a Running SubWorkflow (foreach) step.
 *  A SubWorkflow step doesn't stream agent chunks, so the parent run view used
 *  to sit on "agent starting… (no chunk received)" for the whole 2-3h fan-out —
 *  no way to tell it's alive or where it's at. This polls the child workflow's
 *  runs and shows "N items · M ✓ · running <id>". */
function FanOutProgress({
  childWorkflowId,
  t,
}: {
  childWorkflowId: string;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  const [runs, setRuns] = useState<WorkflowRun[]>([]);
  useEffect(() => {
    let alive = true;
    const fetchRuns = async () => {
      try {
        const r = await workflowsApi.listRuns(childWorkflowId);
        if (alive) setRuns(r);
      } catch { /* transient — keep last */ }
    };
    void fetchRuns();
    const id = window.setInterval(() => void fetchRuns(), 8000);
    return () => { alive = false; window.clearInterval(id); };
  }, [childWorkflowId]);

  if (runs.length === 0) return null;
  const done = runs.filter(r => r.status !== 'Running').length;
  const ok = runs.filter(r => r.status === 'Success').length;
  const running = runs.find(r => r.status === 'Running');
  return (
    <span className="wf-fanout-live" data-testid="wf-fanout-live" style={{ color: 'var(--kr-warning)' }}>
      {t('wf.fanoutProgress', done, ok)}
      {running ? ` · ${running.id.slice(0, 8)}…` : ''}
    </span>
  );
}

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

  // 0.8.2 — Prefer the step's authoritative `started_at` (stamped by the
  // runner before each executor fires) over the legacy `runStart + sum of
  // prior durations` estimate. The estimate drifts after Goto loops or any
  // scheduling gap between steps, which produced visible disagreement
  // between this view and WorkflowDetail's live-mini-dashboard. Old runs
  // pre-dating the field stay readable via the estimate fallback.
  const currentStepResult = run.step_results[stepIndex];
  const runStart = new Date(run.started_at).getTime();
  const completedDurationMs = run.step_results
    .slice(0, stepIndex)
    .reduce((acc, sr) => acc + (sr.duration_ms || 0), 0);
  const stepStart = currentStepResult?.started_at
    ? new Date(currentStepResult.started_at).getTime()
    : runStart + completedDurationMs;
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

/** 0.8.3 — shape of a Feasibility-Gated triage manifest as emitted by
 *  the triage step and substituted into the review_triage Gate message.
 *  Mirror of `backend/src/workflows/triage.rs::triage_manifest_schema`.
 *  Fields beyond the four categories (e.g. `files_touched`) are tolerated
 *  but rendered in a collapsible summary so they don't dominate the panel.
 */
type TriageClear = { id: string; what: string; where?: string };
type TriageDecided = {
  id: string;
  what: string;
  chosen: string;
  why: string;
  options_considered?: string[];
};
type TriageMocked = {
  id: string;
  what: string;
  placeholder: string;
  strategy?: string;
  revisit_when?: string;
};
type TriageBlocked = {
  id: string;
  what: string;
  why: string;
  needed_from: string;
  workaround?: string;
};
type TriageManifest = {
  clear: TriageClear[];
  decided: TriageDecided[];
  mocked: TriageMocked[];
  blocked: TriageBlocked[];
  files_touched?: string[];
};

/** Try to extract a triage manifest from a Gate message. The runner
 *  substitutes `{{steps.triage.data}}` with the JSON value, so the
 *  message has prose + one embedded JSON object. We scan for the first
 *  `{`, brace-count to the matching `}`, and parse. Returns `null` if
 *  the JSON is missing, malformed, or doesn't match the manifest shape.
 *
 *  Defensive: any non-triage Gate (e.g. a plain "approve to deploy?"
 *  Gate) won't match the shape and we fall back to the raw message.
 *
 *  Exported for tests. The brace-counter has to handle `{` / `}` inside
 *  strings without mis-counting; the test suite exercises escaped quotes,
 *  nested objects, missing categories, and non-array category values. */
export function tryParseTriageManifest(msg: string): TriageManifest | null {
  if (!msg) return null;
  const openIdx = msg.indexOf('{');
  if (openIdx < 0) return null;
  let depth = 0;
  let endIdx = -1;
  let inStr = false;
  let esc = false;
  for (let i = openIdx; i < msg.length; i++) {
    const ch = msg[i];
    if (inStr) {
      if (esc) {
        esc = false;
      } else if (ch === '\\') {
        esc = true;
      } else if (ch === '"') {
        inStr = false;
      }
      continue;
    }
    if (ch === '"') {
      inStr = true;
    } else if (ch === '{') {
      depth++;
    } else if (ch === '}') {
      depth--;
      if (depth === 0) {
        endIdx = i + 1;
        break;
      }
    }
  }
  if (endIdx < 0) return null;
  try {
    const obj = JSON.parse(msg.slice(openIdx, endIdx));
    if (!obj || typeof obj !== 'object') return null;
    if (
      !Array.isArray(obj.clear) ||
      !Array.isArray(obj.decided) ||
      !Array.isArray(obj.mocked) ||
      !Array.isArray(obj.blocked)
    ) {
      return null;
    }
    return obj as TriageManifest;
  } catch {
    return null;
  }
}

/** 0.8.3 — structured rendering of a Feasibility-Gated triage manifest.
 *  Replaces the raw-JSON dump that the user used to see in the Gate
 *  panel. Each of the four categories is a collapsible section with
 *  per-entry cards; `files_touched` lives in a separate collapsible
 *  footer so it doesn't dominate the view. */
function TriageManifestPanel({
  manifest,
  t,
}: {
  manifest: TriageManifest;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  const [openCat, setOpenCat] = useState<Record<string, boolean>>({
    clear: false,
    decided: true,
    mocked: true,
    blocked: true,
  });
  const [openFiles, setOpenFiles] = useState(false);

  const counts = {
    clear: manifest.clear.length,
    decided: manifest.decided.length,
    mocked: manifest.mocked.length,
    blocked: manifest.blocked.length,
  };

  const toggle = (cat: string) => setOpenCat(s => ({ ...s, [cat]: !s[cat] }));

  return (
    <div className="wf-triage-manifest">
      {/* CLEAR — single-way items, no judgment call */}
      <details className="wf-triage-section" data-cat="clear" open={openCat.clear}>
        <summary onClick={(e) => { e.preventDefault(); toggle('clear'); }}>
          <span className="wf-triage-cat-label">{t('wf.gate.triage.clear')}</span>
          <span className="wf-triage-cat-count">{counts.clear}</span>
        </summary>
        <div className="wf-triage-list">
          {manifest.clear.map((it) => (
            <div key={it.id} className="wf-triage-entry" data-cat="clear">
              <div className="wf-triage-entry-id">{it.id}</div>
              <div className="wf-triage-entry-what">{it.what}</div>
              {it.where && <div className="wf-triage-entry-where">→ <code>{it.where}</code></div>}
            </div>
          ))}
        </div>
      </details>

      {/* DECIDED — multiple options, agent picked one. Most useful for review. */}
      <details className="wf-triage-section" data-cat="decided" open={openCat.decided}>
        <summary onClick={(e) => { e.preventDefault(); toggle('decided'); }}>
          <span className="wf-triage-cat-label">{t('wf.gate.triage.decided')}</span>
          <span className="wf-triage-cat-count">{counts.decided}</span>
        </summary>
        <div className="wf-triage-list">
          {manifest.decided.map((it) => (
            <div key={it.id} className="wf-triage-entry" data-cat="decided">
              <div className="wf-triage-entry-id">{it.id}</div>
              <div className="wf-triage-entry-what">{it.what}</div>
              <div className="wf-triage-entry-chosen">
                <span className="wf-triage-entry-label">{t('wf.gate.triage.chosen')}</span>
                <span>{it.chosen}</span>
              </div>
              <div className="wf-triage-entry-why">
                <span className="wf-triage-entry-label">{t('wf.gate.triage.why')}</span>
                <span>{it.why}</span>
              </div>
              {it.options_considered && it.options_considered.length > 0 && (
                <details className="wf-triage-entry-considered">
                  <summary>{t('wf.gate.triage.optionsConsidered', it.options_considered.length)}</summary>
                  <ul>
                    {it.options_considered.map((o, i) => <li key={i}>{o}</li>)}
                  </ul>
                </details>
              )}
            </div>
          ))}
        </div>
      </details>

      {/* MOCKED — real value missing, safe placeholder lets the rest ship */}
      <details className="wf-triage-section" data-cat="mocked" open={openCat.mocked}>
        <summary onClick={(e) => { e.preventDefault(); toggle('mocked'); }}>
          <span className="wf-triage-cat-label">{t('wf.gate.triage.mocked')}</span>
          <span className="wf-triage-cat-count">{counts.mocked}</span>
        </summary>
        <div className="wf-triage-list">
          {manifest.mocked.map((it) => (
            <div key={it.id} className="wf-triage-entry" data-cat="mocked">
              <div className="wf-triage-entry-id">{it.id}</div>
              <div className="wf-triage-entry-what">{it.what}</div>
              <div className="wf-triage-entry-placeholder">
                <span className="wf-triage-entry-label">{t('wf.gate.triage.placeholder')}</span>
                <span>{it.placeholder}</span>
              </div>
              {it.strategy && (
                <div className="wf-triage-entry-strategy">
                  <span className="wf-triage-entry-label">{t('wf.gate.triage.strategy')}</span>
                  <span>{it.strategy}</span>
                </div>
              )}
              {it.revisit_when && (
                <div className="wf-triage-entry-revisit">
                  <span className="wf-triage-entry-label">{t('wf.gate.triage.revisitWhen')}</span>
                  <span>{it.revisit_when}</span>
                </div>
              )}
            </div>
          ))}
        </div>
      </details>

      {/* BLOCKED — cannot proceed, needs external input */}
      <details className="wf-triage-section" data-cat="blocked" open={openCat.blocked}>
        <summary onClick={(e) => { e.preventDefault(); toggle('blocked'); }}>
          <span className="wf-triage-cat-label">{t('wf.gate.triage.blocked')}</span>
          <span className="wf-triage-cat-count">{counts.blocked}</span>
        </summary>
        <div className="wf-triage-list">
          {manifest.blocked.map((it) => (
            <div key={it.id} className="wf-triage-entry" data-cat="blocked">
              <div className="wf-triage-entry-id">{it.id}</div>
              <div className="wf-triage-entry-what">{it.what}</div>
              <div className="wf-triage-entry-needed">
                <span className="wf-triage-entry-label">{t('wf.gate.triage.neededFrom')}</span>
                <span>{it.needed_from}</span>
              </div>
              <div className="wf-triage-entry-why">
                <span className="wf-triage-entry-label">{t('wf.gate.triage.why')}</span>
                <span>{it.why}</span>
              </div>
              {it.workaround && (
                <div className="wf-triage-entry-workaround">
                  <span className="wf-triage-entry-label">{t('wf.gate.triage.workaround')}</span>
                  <span>{it.workaround}</span>
                </div>
              )}
            </div>
          ))}
        </div>
      </details>

      {/* files_touched is a flat list — collapsible footer so it
          doesn't compete with the four decision categories for attention. */}
      {manifest.files_touched && manifest.files_touched.length > 0 && (
        <details
          className="wf-triage-files"
          open={openFiles}
          onToggle={(e) => setOpenFiles((e.target as HTMLDetailsElement).open)}
        >
          <summary>
            {t('wf.gate.triage.filesTouched', manifest.files_touched.length)}
          </summary>
          <ul className="wf-triage-files-list">
            {manifest.files_touched.map((f, i) => <li key={i}><code>{f}</code></li>)}
          </ul>
        </details>
      )}
    </div>
  );
}

/** 0.7.0 Phase 4 — interactive panel rendered when the run is paused on a
 *  Gate step. Shows the operator the rendered message + 3 decision buttons.
 *  The "Request changes" path requires a non-empty comment (the agent
 *  needs feedback to act on); the other two make the comment optional.
 *
 *  0.8.3 — when the message wraps a Feasibility-Gated triage manifest
 *  (detected via `tryParseTriageManifest`), the JSON dump is replaced
 *  with a structured visualization (`TriageManifestPanel`). Non-triage
 *  Gates render the raw message verbatim as before. */
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
  const triageManifest = tryParseTriageManifest(message);

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
        {triageManifest
          ? <TriageManifestPanel manifest={triageManifest} t={t} />
          : (message || t('wf.gate.defaultMessage'))}
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

export function RunDetail({ run, workflowSteps, onDelete, onCancel, onDecide, onNavigateToWorkflow, onNavigateToRun }: RunDetailProps) {
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
        {/* Provenance \u2014 a sub-workflow run shows which parent run/WF spawned it.
            Resolved server-side (parent_workflow_name); clickable \u2192 parent WF. */}
        {run.parent_workflow_name && (
          <button
            type="button"
            className="wf-run-provenance"
            disabled={!run.parent_workflow_id || !onNavigateToWorkflow}
            onClick={() => run.parent_workflow_id && onNavigateToWorkflow?.(run.parent_workflow_id)}
            title={t('wf.run.provenanceHint', run.parent_workflow_name)}
          >
            <CornerDownRight size={11} />
            <span className="wf-run-provenance-name">{run.parent_workflow_name}</span>
            {run.parent_run_started_at && (
              <span className="wf-run-provenance-date">
                {' \u00b7 '}{new Date(run.parent_run_started_at).toLocaleString()}
              </span>
            )}
          </button>
        )}
        {run.tokens_used > 0 && (
          // 0.8.2 — Explicit "total" scope. Pre-fix this badge sat next to
          // the gate's `0 tokens consumed` and read as a contradiction
          // ("you said 0, then 3579?"). The two numbers are legitimate:
          // gate-step = 0 (correct, gates burn no LLM), run-total = sum
          // across all completed steps. Adding `(total)` + a tooltip
          // disambiguates without dropping the run-cost-to-date info.
          <span
            className="text-xs text-muted"
            title={t('wf.tokensTotalHint')}
          >
            {run.tokens_used} {t('wf.tokensTotal')}
          </span>
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
                      : ws_step.step_type.type === 'BatchApiCall' ? 'api'
                      : ws_step.step_type.type === 'Notify' ? 'notify'
                      : ws_step.step_type.type === 'BatchQuickPrompt' ? 'batch'
                      : ws_step.step_type.type === 'Gate' ? 'gate'
                      : ws_step.step_type.type === 'Exec' ? 'exec'
                      : ws_step.step_type.type === 'JsonData' ? 'data'
                      : ws_step.step_type.type === 'SubWorkflow' ? 'subwf'
                      : 'agent'
                  }>
                    {ws_step.step_type.type === 'ApiCall' ? 'API'
                      : ws_step.step_type.type === 'BatchApiCall' ? 'API'
                      : ws_step.step_type.type === 'Notify' ? 'NOTIFY'
                      : ws_step.step_type.type === 'BatchQuickPrompt' ? 'BATCH'
                      : ws_step.step_type.type === 'Gate' ? 'GATE'
                      : ws_step.step_type.type === 'Exec' ? 'EXEC'
                      : ws_step.step_type.type === 'JsonData' ? 'DATA'
                      : ws_step.step_type.type === 'SubWorkflow' ? 'SUB-WF'
                      : 'AGENT'}
                  </span>
                )}
                {ws_step.description && (
                  <span className="text-ghost text-xs" style={{ fontStyle: 'italic' }}>{ws_step.description}</span>
                )}
                {completed && completed.duration_ms > 0 && (
                  <span className="text-ghost text-xs">{(completed.duration_ms / 1000).toFixed(1)}s</span>
                )}
                {/* 0.8.2 — Per-step token badge. Surfaces "which step burns
                    the most tokens" at a glance so the operator can spot
                    candidates for desagentification (swap Agent → Exec /
                    ApiCall on the hot steps). Zero-token steps (Gate, Exec,
                    Notify, ApiCall, JsonData) stay clean — only steps that
                    actually consumed LLM tokens show the badge. */}
                {completed && completed.tokens_used > 0 && (
                  <span
                    className="text-ghost text-xs"
                    title={t('wf.stepTokensHint')}
                    style={{ color: 'var(--kr-accent-ink)' }}
                  >
                    {completed.tokens_used.toLocaleString()} {t('wf.stepTokensSuffix')}
                  </span>
                )}
                {isNext && ws_step.step_type?.type === 'SubWorkflow' && ws_step.sub_workflow_id ? (
                  <FanOutProgress childWorkflowId={ws_step.sub_workflow_id} t={t} />
                ) : isNext ? (
                  <LiveStepStatus run={run} step={ws_step} stepIndex={i} t={t} />
                ) : null}
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
            // 2026-06-10 — on_failure compensation steps are visually fenced
            // off: pre-fix a green rollback right after the failed step read
            // as "the run continued past the failure" (live confusion on the
            // Ticket→PR run). The banner marks where compensation starts.
            const isFirstRollback = !!sr.is_rollback && !(run.step_results[i - 1]?.is_rollback);
            // 2026-06-12 — fan-out aggregate (sub_workflow_foreach_file):
            // per-task table + chip. PARTIAL is Success engine-side (so the
            // run continues to pr_draft) — the warning chip is how a human
            // notices the partial without reading the raw envelope JSON.
            const foreach = sr.step_kind === 'SubWorkflow' ? parseForeachEnvelope(sr.output) : null;
            // The child workflow that ran the fan-out items — its id lives on
            // the SubWorkflow step definition. Lets each per-task row link to
            // the child sub-workflow's run list (2026-06-13 user request).
            const childWorkflowId = foreach
              ? workflowSteps?.find(ws => ws.name === sr.step_name)?.sub_workflow_id ?? null
              : null;
            return (
              <div key={i} className="wf-step-result" data-expanded={isExpanded} data-rollback={sr.is_rollback ? 'true' : undefined}>
                {isFirstRollback && (
                  <div className="wf-rollback-separator" data-testid="wf-rollback-separator">
                    ⚠ {t('wf.rollbackSection')}
                  </div>
                )}
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
                  {sr.is_rollback && (
                    <span className="wf-step-kind-badge" data-kind="rollback" style={{ color: 'var(--kr-warning)', borderColor: 'rgba(var(--kr-warning-rgb), 0.4)' }}>
                      ↩ ROLLBACK
                    </span>
                  )}
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
                  {sr.step_kind === 'SubWorkflow' && (
                    <span className="wf-step-kind-badge" data-kind="subworkflow">
                      <GitBranch size={9} /> SOUS-WF
                    </span>
                  )}
                  {foreach && (
                    <span
                      className="wf-step-kind-badge"
                      data-testid="wf-foreach-chip"
                      style={foreach.failed > 0
                        ? { color: 'var(--kr-warning)', borderColor: 'rgba(var(--kr-warning-rgb), 0.4)' }
                        : { color: 'var(--kr-success)', borderColor: 'rgba(var(--kr-success-rgb), 0.4)' }}
                    >
                      {foreach.failed > 0
                        ? `⚠ ${foreach.succeeded}/${foreach.total} · ${foreach.failed} ${t('wf.foreachFailedChip')}`
                        : `${foreach.succeeded}/${foreach.total}`}
                    </span>
                  )}
                  {/* 2026-06-11 Phase 1c — drill indicator into the nested
                      child run. Full recursive tree view is a follow-up; this
                      surfaces that a child run exists + its id. */}
                  {sr.child_run_id && (
                    <span className="text-2xs text-ghost" title={sr.child_run_id} style={{ fontFamily: 'var(--kr-font-mono)' }}>
                      ↳ {t('wf.subRun')} {sr.child_run_id.slice(0, 8)}…
                    </span>
                  )}
                  {sr.step_kind === 'Agent' && sr.step_agent && (
                    <span className="text-xs font-semibold" style={{ color: AGENT_COLORS[sr.step_agent] ?? 'var(--kr-text-faint)' }}>
                      {AGENT_LABELS[sr.step_agent] ?? sr.step_agent}
                    </span>
                  )}
                  {/* 2026-06-13 — the model/tier actually resolved for this step
                      (backend-stamped), shown on EVERY agent step incl. per-item
                      fan-out routing. Falls back to the step-def tier for runs
                      recorded before step_model shipped. */}
                  {sr.step_kind === 'Agent' && (() => {
                    const fallback = workflowSteps?.find(ws => ws.name === sr.step_name)?.agent_settings;
                    const label = sr.step_model || fallback?.model || fallback?.tier;
                    return label ? (
                      <span className="wf-tier-badge" title={t('wf.modelTierHint')}>{label}</span>
                    ) : null;
                  })()}
                  <span className="text-ghost">
                    {sr.duration_ms > 0 ? `${(sr.duration_ms / 1000).toFixed(1)}s` : ''}
                  </span>
                  {sr.condition_result && (
                    <span className="wf-condition-badge" data-action={sr.condition_result === 'Stop' ? 'stop' : 'other'}>
                      {conditionLabel(sr.condition_result)}
                    </span>
                  )}
                  {/* "→ next" only on NOMINAL continuation: never on a
                      rollback step, never when the next row is rollback
                      (the run did NOT "continue", it's compensating). */}
                  {!sr.condition_result && sr.status === 'Success' && i < run.step_results.length - 1
                    && !sr.is_rollback && !run.step_results[i + 1]?.is_rollback && (
                    <span className="text-2xs" style={{ color: 'rgba(var(--kr-success-rgb), 0.5)' }}>&rarr; {t('wf.nextStepArrow')}</span>
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
                    {foreach && (
                      <table className="wf-foreach-table" data-testid="wf-foreach-table">
                        <thead>
                          <tr>
                            <th>#</th>
                            <th>{t('wf.foreachTaskCol')}</th>
                            <th>{t('wf.status')}</th>
                            <th>{t('wf.subRun')}</th>
                          </tr>
                        </thead>
                        <tbody>
                          {foreach.items.map(it => (
                            <tr key={`${it.item}-${it.id}`}>
                              <td className="text-ghost">{it.item}</td>
                              <td style={{ fontFamily: 'var(--kr-font-mono)' }}>{it.id}</td>
                              {/* binary headline color (ok/failed); the 0 tk
                                  hint carries the per-type detail */}
                              <td style={{ color: it.status === 'Failed' ? (STATUS_COLORS['Failed'] ?? 'var(--kr-warning)') : (STATUS_COLORS['Success'] ?? 'var(--kr-success)') }}>
                                {it.status}{isZeroTokenItem(it.status) ? ' · 0 tk' : ''}
                              </td>
                              <td className="text-ghost" style={{ fontFamily: 'var(--kr-font-mono)' }} title={it.child_run_id ?? undefined}>
                                {it.child_run_id
                                  ? (childWorkflowId && (onNavigateToRun || onNavigateToWorkflow)
                                      ? <button
                                          type="button"
                                          className="wf-subrun-link"
                                          onClick={() => onNavigateToRun
                                            ? onNavigateToRun(childWorkflowId, it.child_run_id!)
                                            : onNavigateToWorkflow!(childWorkflowId)}
                                          title={t('wf.openSubRun')}
                                        >{it.child_run_id.slice(0, 8)}… ↗</button>
                                      : `${it.child_run_id.slice(0, 8)}…`)
                                  : '—'}
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    )}
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
