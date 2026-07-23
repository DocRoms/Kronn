import { useState, useRef, useEffect, useMemo, Fragment } from 'react';
import { useT } from '../../lib/I18nContext';
import { workflows as workflowsApi, quickPrompts as quickPromptsApi } from '../../lib/api';
import type { BatchPreview } from '../../lib/api';
import { AGENT_COLORS, AGENT_LABELS, isAgentRestricted } from '../../lib/constants';
import { extractLikelyOutput } from '../../lib/extractLikelyOutput';
import type { Workflow, WorkflowRun, StepResult, AgentsConfig, WorkflowStep, QuickPrompt, BatchRunSummary, AgentType } from '../../types/generated';
import {
  Trash2, Play, Loader2, Check, X, ChevronRight, ChevronDown,
  Settings, RefreshCw, AlertTriangle, FlaskConical,
  Layers, GitBranch, MessageSquare, Plug, Send,
  Download, Square, Hand, Terminal, Braces, Sparkles, Zap, Search,
} from 'lucide-react';
import { filterRuns, groupRunsByParent, RUN_PAGE_SIZE, type RunStatusFilter } from '../../lib/runFilters';
import { formatDurationCompact } from '../../lib/kronnToolParser';
import { hasBranches } from '../../lib/stepGraph';
import { StepBranchMap } from './StepBranchMap';
import { RunDetail } from './RunDetail';
import '../../pages/WorkflowsPage.css';

const checkAgentRestricted = isAgentRestricted;

/** 0.8.8 — which "step in progress" placeholder to show in the live-run view.
 *  Only `Agent` steps stream chunks into `currentStepText`; every other type
 *  (ApiCall, Exec, Gate, Notify, JsonData, Batch*, SubWorkflow) produces its
 *  output at the END of the step, so the "L'agent démarre…" copy was wrong
 *  there — a user with a `fetch` ApiCall step (no agent at all) saw it. */
export function liveStepWaitingKey(step: Pick<WorkflowStep, 'step_type'>): string {
  const isAgentLike = !step.step_type || step.step_type.type === 'Agent';
  return isAgentLike ? 'wf.live.stepStreamingWaiting' : 'wf.live.stepRunningNoStream';
}

/** Module-level tracker for in-flight and completed step tests.
 *
 *  A StepCard that starts a test doesn't own the SSE stream — it delegates
 *  to `startStepTest` / `startBatchStepTest` here, which own the actual
 *  network handles and write into the Map. The component subscribes to
 *  updates for its cache key and re-renders when the tracker notifies.
 *
 *  Why module-level and not React state: the user's scenario is "I click
 *  Test, change tab, come back". When they change tab the StepCard
 *  unmounts — any React state inside it dies, and a callback fired after
 *  unmount (SSE result arriving late) has nowhere to write. Moving the
 *  state outside React means the SSE task keeps updating the shared Map
 *  even with no observer, and the next mount picks up exactly where we
 *  left off. Same trick as React-Query's cache or Zustand stores.
 *
 *  Persistence: in-memory only (dies at tab close). sessionStorage would
 *  add durability across hard refresh, but we can't serialize a live
 *  AbortController / fetch-in-progress, so the tracker would end up
 *  inconsistent on refresh anyway. Accept "closed tab = lost test" as
 *  normal browser behaviour. */
interface ActiveStepTest {
  /** Identity echoed back for convenience when iterating. */
  cacheKey: string;
  running: boolean;
  /** User-supplied mock previous output — preserved so the user sees their
   *  typed input when they come back, even if the test is still running. */
  mockInput: string;
  dryRun: boolean;
  liveOutput: string;
  result: StepResult | null;
  error: string | null;
  batchPreview: BatchPreview | null;
  startedAt: number | null;
  /** Kept alive as long as the test is running. Not serialized, never
   *  aborted automatically on unmount — lets the test survive tab switches. */
  abortController: AbortController | null;
}

const SS_KEY_PREFIX = 'kronn:stepTest:';
const stepTestKey = (workflowId: string, stepName: string, index: number) =>
  `${SS_KEY_PREFIX}${workflowId}::${stepName}::${index}`;

const activeStepTests = new Map<string, ActiveStepTest>();
const stepTestListeners = new Map<string, Set<() => void>>();

function subscribeStepTest(key: string, cb: () => void): () => void {
  let listeners = stepTestListeners.get(key);
  if (!listeners) {
    listeners = new Set();
    stepTestListeners.set(key, listeners);
  }
  // Hoist into a const so the unsubscribe closure below captures a
  // non-nullable reference (avoids `listeners!` and the React-19
  // strict-rule warning for non-null assertions on a let binding).
  const captured = listeners;
  captured.add(cb);
  return () => {
    captured.delete(cb);
    if (captured.size === 0) stepTestListeners.delete(key);
  };
}

function notifyStepTest(key: string): void {
  stepTestListeners.get(key)?.forEach(cb => cb());
}

/** Atomic patch + notify so every observer sees a consistent state. */
function patchStepTest(key: string, patch: Partial<ActiveStepTest>): void {
  const current = activeStepTests.get(key) ?? {
    cacheKey: key,
    running: false,
    mockInput: '',
    dryRun: true,
    liveOutput: '',
    result: null,
    error: null,
    batchPreview: null,
    startedAt: null,
    abortController: null,
  };
  activeStepTests.set(key, { ...current, ...patch });
  notifyStepTest(key);
}

function getStepTest(key: string): ActiveStepTest | undefined {
  return activeStepTests.get(key);
}

export interface LiveRunState {
  workflowId: string;
  /** Run id, set after the backend's `run_start` SSE event. Required to
   *  call `cancelRun(workflow_id, run_id)` from the Stop button. Null
   *  for the brief window between trigger() POST and the first SSE event. */
  runId: string | null;
  currentStep: string | null;
  stepIndex: number;
  totalSteps: number;
  completedSteps: StepResult[];
  /** Live agent stdout for the step currently in flight. Reset on every
   *  StepStart, accumulated on every StepProgress chunk. Empty string
   *  when no Agent step is running. */
  currentStepText: string;
  /** Wall-clock millis at trigger time. Drives the "total elapsed" badge
   *  on the live view header. Set once when the live run starts. */
  startedAt: number;
  /** Wall-clock millis at the current step's StepStart. Set on every
   *  step transition, null between steps. Drives the per-step elapsed
   *  badge on the in-flight row. */
  currentStepStartedAt: number | null;
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
  /** Click on a "📋 N conversations" chip → jump to the discussions tab and
   *  expand+scroll to the matching batch group. */
  onNavigateToBatch?: (batchRunId: string) => void;
  /** 2026-06-13 — open another workflow's detail (e.g. jump from a fan-out
   *  per-task row to the child sub-workflow that ran it). */
  onNavigateToWorkflow?: (workflowId: string) => void;
  /** #11 — jump to a SPECIFIC child run (workflow + run id): opens that
   *  workflow's detail and focuses the exact run. */
  onNavigateToRun?: (workflowId: string, runId: string) => void;
  /** 0.8.11 UX — one-click enable/disable from the detail header (a disabled
   *  workflow's launch button is inert; this is the visible way out). */
  onToggleEnabled?: (enabled: boolean) => void;
  /** #11 — a run id to auto-expand + scroll into view once loaded. */
  focusRunId?: string | null;
  /** Feedback for the copy-id pill; the copy still works without it. */
  toast?: (msg: string, type?: 'success' | 'error' | 'info') => void;
  /** 0.7.0 UX pass — export the workflow as a JSON file. The handler
   *  is wired in the parent page (it has the api binding + toast). */
  onExport?: () => void;
  /** Called when the operator approves/rejects/requests-changes on a Gate.
   *  The SSE stream closed when the run paused, so the parent's `liveRun`
   *  is now stale ("Awaiting your decision" stays on screen forever).
   *  The page wipes its liveRun state in response — the runs list (which
   *  refreshes from DB on the same callback) takes over showing the
   *  post-decision progress. */
  onGateDecided?: () => void;
}

/** Renders the per-item preview of a BatchQuickPrompt dry-run.
 *
 *  For each sample item, shows:
 *    1. The item id (what fills the QP variable)
 *    2. The rendered prompt (collapsible — usually long)
 *    3. A "🧪 Lancer dry-run" button that fires test-step against an
 *       ephemeral Agent step with the rendered prompt + dry_run=true.
 *       Result is shown inline so Marie can see what each agent would do
 *       without leaving the test panel.
 *
 *  The button hits the existing /api/workflows/test-step SSE endpoint —
 *  no new backend route. Agent type is taken from the QP (we know it from
 *  the BatchPreview). Each item runs in its own AbortController so the
 *  user can stop one without killing the others. */
export function BatchItemsList({
  items, renderedPrompts, quickPromptAgent, projectId, t,
}: {
  items: string[];
  renderedPrompts: string[];
  quickPromptAgent: string | null;
  projectId: string | null;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  type ItemDryRun = {
    running: boolean;
    output: string;
    error: string | null;
    abort: AbortController | null;
  };
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  const [dryRuns, setDryRuns] = useState<Record<number, ItemDryRun>>({});

  const toggleExpand = (i: number) => {
    setExpanded(prev => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i); else next.add(i);
      return next;
    });
  };

  const launchDryRun = async (i: number) => {
    if (!quickPromptAgent) return;
    const prompt = renderedPrompts[i];
    if (!prompt) return;
    // Cancel any previous run for the same index.
    dryRuns[i]?.abort?.abort();
    const controller = new AbortController();
    setDryRuns(prev => ({ ...prev, [i]: { running: true, output: '', error: null, abort: controller } }));

    // Build an ephemeral Agent step carrying the already-rendered prompt.
    // The backend test-step handler doesn't care that this step never lived
    // in the workflow — it just runs an agent in dry-run mode with this
    // prompt and streams back the result.
    const ephemeralStep: WorkflowStep = {
      name: `__batch_item_${i}__`,
      step_type: { type: 'Agent' },
      agent: quickPromptAgent as AgentType,
      output_format: { type: 'FreeText' },
      prompt_template: prompt,
      mode: { type: 'Normal' },
    };

    let liveOutput = '';
    await workflowsApi.testStepStream(
      { step: ephemeralStep, project_id: projectId, mock_previous_output: null, dry_run: true },
      () => {},
      (result) => { liveOutput = result.output; },
      () => {
        setDryRuns(prev => ({ ...prev, [i]: { running: false, output: liveOutput, error: null, abort: null } }));
      },
      (err) => {
        setDryRuns(prev => ({ ...prev, [i]: { running: false, output: liveOutput, error: err, abort: null } }));
      },
      controller.signal,
      (chunk) => {
        liveOutput += chunk;
        setDryRuns(prev => ({ ...prev, [i]: { ...(prev[i] ?? { abort: controller }), running: true, output: liveOutput, error: null, abort: controller } }));
      },
    );
  };

  return (
    <div className="wf-batch-items-list">
      {items.map((item, i) => {
        const isExpanded = expanded.has(i);
        const dr = dryRuns[i];
        return (
          <div key={i} className="wf-batch-item">
            <div className="wf-batch-item-header">
              <button
                type="button"
                className="wf-batch-item-toggle"
                onClick={() => toggleExpand(i)}
                aria-expanded={isExpanded}
                title={t('wiz.testBatchItemToggle')}
              >
                <ChevronRight size={10} style={{ transform: isExpanded ? 'rotate(90deg)' : 'none', transition: 'transform 0.15s' }} />
                <code>{item}</code>
              </button>
              {quickPromptAgent && (
                <button
                  type="button"
                  className="wf-batch-item-dryrun-btn"
                  onClick={() => launchDryRun(i)}
                  disabled={dr?.running}
                  title={t('wiz.testBatchItemDryRunHint')}
                >
                  {dr?.running ? <Loader2 size={10} className="spin" /> : '🧪'}
                  {dr?.running ? t('wiz.testBatchItemDryRunLoading') : t('wiz.testBatchItemDryRunBtn')}
                </button>
              )}
            </div>
            {isExpanded && renderedPrompts[i] && (
              <div className="wf-batch-item-body">
                <div className="text-xs text-muted mb-1">{t('wiz.testBatchItemPrompt')}</div>
                <pre className="wf-test-output">{renderedPrompts[i]}</pre>
              </div>
            )}
            {dr && (dr.output || dr.error) && (
              <div className="wf-batch-item-result">
                {dr.error && <div className="wf-test-error">{dr.error}</div>}
                {dr.output && (
                  <>
                    <div className="text-xs text-muted mb-1">
                      {t('wiz.testBatchItemDryRunResult')}
                    </div>
                    <pre className="wf-test-output">{dr.output}</pre>
                  </>
                )}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function StepCard({ step, index, agentAccess, projectId, t, quickPromptsById, workflowId, allSteps, nested = false }: {
  step: WorkflowStep; index: number; agentAccess?: AgentsConfig | null;
  projectId?: string | null; t: (key: string, ...args: (string | number)[]) => string;
  quickPromptsById?: Map<string, QuickPrompt>;
  /** Workflow id is needed to key the dry-run test state cache (see module
   *  comment on `stepTestCache`) so the panel survives navigation. */
  workflowId: string;
  /** Full step list — lets StepCard look up the previous step's cached test
   *  result to auto-fill the mock input when the user opens the test panel. */
  allSteps: WorkflowStep[];
  /** 2026-06-11 — true when this card is rendered INSIDE a SubWorkflow step's
   *  expansion (a child step). Suppresses the dry-run test panel and stops
   *  the SubWorkflow body from recursing further (a nested sub-WF shows a
   *  compact list instead of another level of cards). Bounds recursion. */
  nested?: boolean;
}) {
  const isBatch = step.step_type?.type === 'BatchQuickPrompt';
  const isApi = step.step_type?.type === 'ApiCall';
  const isNotify = step.step_type?.type === 'Notify';
  const isGate = step.step_type?.type === 'Gate';
  const isExec = step.step_type?.type === 'Exec';
  const isBatchApi = step.step_type?.type === 'BatchApiCall';
  const isJsonData = step.step_type?.type === 'JsonData';
  const isSubWorkflow = step.step_type?.type === 'SubWorkflow';
  // Only the Agent step type actually consumes the `agent` field; every
  // other type delegates: Batch → QP, ApiCall / BatchApiCall → HTTP, Notify
  // → webhook, Gate → human pause, Exec → shell binary, JsonData → static
  // payload. Showing the agent badge on those types confused users into
  // thinking "Claude Code" was running inside their `cargo test` step (or
  // on a JsonData fixture). Whitelist Agent explicitly to avoid the
  // recurring "miss a step type in the negation" bug.
  const isAgentLike = !step.step_type || step.step_type.type === 'Agent';
  const batchQp = isBatch && step.batch_quick_prompt_id
    ? quickPromptsById?.get(step.batch_quick_prompt_id)
    : undefined;
  // For Notify steps the only useful summary is "where does the ping
  // land" — we extract just the host of the URL (full URL might leak
  // a webhook secret in a screenshot, host alone tells the user it's
  // their Slack/Teams/etc.).
  const notifyHost = isNotify && step.notify_config?.url
    ? (() => { try { return new URL(step.notify_config.url).host; } catch { return step.notify_config.url; } })()
    : null;

  // Observe the module-level tracker for this (workflow, step) pair.
  // The tracker owns the SSE stream and survives component unmount — when
  // the user switches tab and comes back, `current` reflects whatever state
  // the still-running / finished test arrived at, instead of the local
  // default that a fresh mount would show.
  const cacheKey = stepTestKey(workflowId, step.name, index);
  const [, forceRender] = useState(0);
  useEffect(() => {
    return subscribeStepTest(cacheKey, () => forceRender(n => n + 1));
  }, [cacheKey]);

  // 2026-06-11 — for a SubWorkflow step, load the referenced child workflow
  // so we can show its steps decomposed (like a normal WF) instead of an
  // opaque box. `@bundle:` sentinels (un-saved decomposed presets) have no
  // real id yet → we skip the fetch and show a "created on save" note.
  const [childWf, setChildWf] = useState<Workflow | null>(null);
  const subRefIsBundle = !!step.sub_workflow_id?.startsWith('@bundle:');
  useEffect(() => {
    if (!isSubWorkflow || !step.sub_workflow_id || subRefIsBundle) { setChildWf(null); return; }
    let alive = true;
    workflowsApi.get(step.sub_workflow_id).then(w => { if (alive) setChildWf(w); }).catch(() => {});
    return () => { alive = false; };
  }, [isSubWorkflow, step.sub_workflow_id, subRefIsBundle]);

  const current = getStepTest(cacheKey);
  const testMockInput = current?.mockInput ?? '';
  const testDryRun = current?.dryRun ?? true;
  const testRunning = current?.running ?? false;
  const testResult = current?.result ?? null;
  const testError = current?.error ?? null;
  const testLiveOutput = current?.liveOutput ?? '';
  const batchPreview = current?.batchPreview ?? null;

  // `testOpen` is a pure local UI toggle — NOT derived from the tracker.
  // Initially opens automatically if the tracker already holds meaningful
  // content for this key (user came back and there's a running test or a
  // result to see). After that, the user controls it via the Test button.
  const hasMeaningfulContent = Boolean(
    current && (current.running || current.result || current.error || current.batchPreview || current.liveOutput)
  );
  const [testOpen, setTestOpenLocal] = useState(hasMeaningfulContent);
  // If the tracker transitions from empty → populated while mounted (e.g. a
  // WS-driven update), auto-open the panel so the user sees the new content.
  const prevHasContent = useRef(hasMeaningfulContent);
  useEffect(() => {
    if (hasMeaningfulContent && !prevHasContent.current) {
      setTestOpenLocal(true);
    }
    prevHasContent.current = hasMeaningfulContent;
  }, [hasMeaningfulContent]);

  // Close/clear the panel for this step. Also aborts the in-flight stream
  // if one is running — exposed so the "close test" X button can wipe state.
  const closeTestPanel = () => {
    current?.abortController?.abort();
    activeStepTests.delete(cacheKey);
    notifyStepTest(cacheKey);
    setTestOpenLocal(false);
  };

  // Elapsed ticker — purely a visual counter derived from startedAt, kept
  // local to this component (not worth storing in the tracker since any
  // render recomputes it from the timestamp). Reset when testRunning drops.
  const [testElapsed, setTestElapsed] = useState(0);
  useEffect(() => {
    const startedAt = current?.startedAt;
    if (!testRunning || !startedAt) { setTestElapsed(0); return; }
    const tick = () => setTestElapsed(Math.floor((Date.now() - startedAt) / 1000));
    tick();
    const handle = setInterval(tick, 1000);
    return () => clearInterval(handle);
  }, [testRunning, current?.startedAt]);

  const [expanded, setExpanded] = useState(false);
  const liveOutputRef = useRef<HTMLPreElement | null>(null);
  const isLong = step.prompt_template.length > 200;

  // Local setters that project into the tracker so the user's typing on
  // the mock-input textarea survives unmount too.
  const setTestMockInput = (v: string) => patchStepTest(cacheKey, { mockInput: v });
  const setTestDryRun = (v: boolean) => patchStepTest(cacheKey, { dryRun: v });

  // Look up the previous step's cached test output, if any. Used to auto-fill
  // the mock input so the user doesn't have to paste it manually every time.
  const prevStepResult = (() => {
    if (index === 0) return null;
    const prevStep = allSteps[index - 1];
    if (!prevStep) return null;
    const prevKey = stepTestKey(workflowId, prevStep.name, index - 1);
    const prev = getStepTest(prevKey);
    return prev?.result?.output ?? null;
  })();

  // Smart-extract the "useful" part of an agent's reply for auto-fill.
  //
  // Agents typically wrap their actual output in conversational filler:
  //   "Voici les 3 plus vieux tickets ... :\n\nEW-2687,EW-3055"
  // If we paste the whole thing into Mock input, parse_items splits the
  // blabla on newlines and treats each chunk as a separate item — the
  // batch step then thinks it has 5 tickets including the explanation.
  // Heuristic extracted to lib/extractLikelyOutput.ts for testability.

  const [prefilledFromPrev, setPrefilledFromPrev] = useState(false);
  const [showFullPrev, setShowFullPrev] = useState(false);

  const applyPrevStepResult = (full: boolean) => {
    if (!prevStepResult) return;
    const { value, extracted } = full
      ? { value: prevStepResult, extracted: false }
      : extractLikelyOutput(prevStepResult);
    patchStepTest(cacheKey, { mockInput: value });
    setPrefilledFromPrev(true);
    setShowFullPrev(full || !extracted);
  };

  const setTestOpen = (v: boolean) => {
    if (!v) {
      closeTestPanel();
      setPrefilledFromPrev(false);
      setShowFullPrev(false);
      return;
    }
    setTestOpenLocal(true);
    // Opening the test panel for the first time (mock input is empty)?
    // If the previous step was tested and produced an output, pre-fill
    // from it — extracted by default, but the user can switch to "voir
    // tout" if the heuristic guessed wrong.
    const currentMock = activeStepTests.get(cacheKey)?.mockInput ?? '';
    if (!currentMock && prevStepResult) {
      applyPrevStepResult(false);
    }
  };

  const handleBatchTest = async () => {
    patchStepTest(cacheKey, {
      running: true,
      batchPreview: null,
      error: null,
      startedAt: Date.now(),
    });
    try {
      // Pass the REAL name of the step before us so `{{steps.<name>.output}}`
      // / `{{steps.<name>.data}}` resolve in the dry-run. Hard-coding 'previous'
      // here would only populate `{{previous_step.X}}` shorthand and leave
      // any `steps.<actualName>.X` template unresolved.
      const previousStepName = index > 0 ? allSteps[index - 1]?.name ?? 'previous' : 'previous';
      const preview = await workflowsApi.testBatchStep({
        step,
        mock_previous_output: testMockInput || null,
        previous_step_name: previousStepName,
      });
      patchStepTest(cacheKey, { batchPreview: preview, running: false });
    } catch (e) {
      patchStepTest(cacheKey, { error: String(e), running: false });
    }
  };

  const handleTest = async () => {
    const controller = new AbortController();
    patchStepTest(cacheKey, {
      running: true,
      result: null,
      error: null,
      liveOutput: '',
      startedAt: Date.now(),
      abortController: controller,
    });

    await workflowsApi.testStepStream(
      {
        step,
        project_id: projectId ?? null,
        mock_previous_output: testMockInput || null,
        dry_run: testDryRun,
      },
      () => {},
      (result) => {
        patchStepTest(cacheKey, { result });
      },
      () => {
        patchStepTest(cacheKey, { running: false, abortController: null });
      },
      (err) => {
        patchStepTest(cacheKey, { running: false, error: err, abortController: null });
      },
      controller.signal,
      (text) => {
        const prev = activeStepTests.get(cacheKey)?.liveOutput ?? '';
        patchStepTest(cacheKey, { liveOutput: prev + text });
        requestAnimationFrame(() => {
          if (liveOutputRef.current) {
            liveOutputRef.current.scrollTop = liveOutputRef.current.scrollHeight;
          }
        });
      },
    );
    // Safety net — most paths go through onDone/onError above.
    const latest = activeStepTests.get(cacheKey);
    if (latest?.running) {
      patchStepTest(cacheKey, { running: false, abortController: null });
    }
  };

  const cardKind = isBatch ? 'batch-qp' : isApi ? 'api' : isNotify ? 'notify' : isGate ? 'gate' : isExec ? 'exec' : 'agent';
  return (
    <div className="wf-step-card" data-step-type={cardKind}>
      <div className="flex-row gap-4">
        <span className="wf-step-number">{index + 1}</span>
        <span className="font-semibold text-md">{step.name}</span>
        {/* Per step_type tag — the previous version always rendered the
            agent label, which made an ApiCall step look like an Agent
            step ("main · Claude Code"). Now: distinct badge for Batch,
            API, Notify; agent label only for genuine Agent steps. */}
        {isBatch && (
          <span className="wf-step-kind-badge" title={t('wiz.stepTypeBatchQPHint')}>
            <Layers size={10} /> {t('wiz.stepTypeBatchQP')}
          </span>
        )}
        {isApi && (
          <span className="wf-step-kind-badge" data-kind="api" title={t('wiz.stepTypeApiCallHint')}>
            <Plug size={10} /> API
            {step.api_endpoint_path && (
              <span className="text-xs text-ghost" style={{ fontWeight: 400, marginLeft: 6 }}>
                {step.api_plugin_slug ?? '?'} · {step.api_endpoint_path}
              </span>
            )}
          </span>
        )}
        {isNotify && (
          <span className="wf-step-kind-badge" data-kind="notify" title={t('wiz.notifyHint')}>
            <Send size={10} /> {t('wiz.stepTypeNotify')}
            {notifyHost && (
              <span className="text-xs text-ghost" style={{ fontWeight: 400, marginLeft: 6 }}>
                {notifyHost}
              </span>
            )}
          </span>
        )}
        {isBatchApi && (
          <span className="wf-step-kind-badge" data-kind="batch-api" title={t('wiz.stepTypeBatchApiHint')}>
            <Layers size={10} /> {t('wiz.stepTypeBatchApi')}
            {step.api_endpoint_path && (
              <span className="text-xs text-ghost" style={{ fontWeight: 400, marginLeft: 6 }}>
                {step.api_plugin_slug ?? '?'} · {step.api_endpoint_path}
              </span>
            )}
          </span>
        )}
        {isGate && (
          <span className="wf-step-kind-badge" data-kind="gate" title={t('wiz.stepTypeGateHint')}>
            <Hand size={10} /> {t('wiz.stepTypeGate')}
          </span>
        )}
        {isExec && (
          <span className="wf-step-kind-badge" data-kind="exec" title={t('wiz.stepTypeExecHint')}>
            <Terminal size={10} /> {t('wiz.stepTypeExec')}
            {step.exec_command && (
              <span className="text-xs text-ghost" style={{ fontWeight: 400, marginLeft: 6 }}>
                {step.exec_command}{(step.exec_args ?? []).length > 0 ? ' ' + (step.exec_args ?? []).join(' ') : ''}
              </span>
            )}
          </span>
        )}
        {isJsonData && (
          <span className="wf-step-kind-badge" data-kind="json-data" title={t('wiz.stepTypeJsonDataHint')}>
            <Braces size={10} /> {t('wiz.stepTypeJsonData')}
            {(() => {
              const p = step.json_data_payload;
              if (p === null || p === undefined) return null;
              const summary = Array.isArray(p)
                ? t('wiz.jsonDataSummaryArray', p.length)
                : (typeof p === 'object' ? t('wiz.jsonDataSummaryObject', Object.keys(p as object).length) : t('wiz.jsonDataSummaryScalar'));
              return (
                <span className="text-xs text-ghost" style={{ fontWeight: 400, marginLeft: 6 }}>
                  {summary}
                </span>
              );
            })()}
          </span>
        )}
        {isSubWorkflow && (
          <span className="wf-step-kind-badge" data-kind="subworkflow" title={t('wiz.subWorkflowHint')}>
            <GitBranch size={10} /> {t('wiz.stepTypeSubWorkflow')}
            <span className="text-xs text-ghost" style={{ fontWeight: 400, marginLeft: 6 }}>
              {subRefIsBundle ? (step.sub_workflow_id ?? '').replace('@bundle:', '↳ ') : (childWf?.name ?? '…')}
            </span>
          </span>
        )}
        {isAgentLike && (
          <span className="text-xs font-semibold" style={{ color: AGENT_COLORS[step.agent] ?? 'var(--kr-text-faint)' }}>
            {AGENT_LABELS[step.agent] ?? step.agent}
          </span>
        )}
        {isAgentLike && <TierBadge step={step} t={t} />}
        {isAgentLike && checkAgentRestricted(agentAccess ?? undefined, step.agent) && (
          <span className="flex-row gap-1 text-xs text-warning">
            <AlertTriangle size={10} />
            {t('config.restrictedStep')}
          </span>
        )}
        {/* The Test button is a dry-run mock for Agent / Batch steps —
            it doesn't apply to:
              - ApiCall (real-call test lives in the wizard's `Test the
                call` button),
              - Notify (no agent run),
              - Exec (mechanical binary execution; the test endpoint
                routes through the Agent runner today, so the panel
                would show useless mock/dry-run controls AND not
                actually exec the binary — UX feedback 2026-04-29),
              - Gate (a human-pause step has nothing to test).
            Hiding it on those types keeps the row clean. */}
        {!isApi && !isNotify && !isExec && !isGate && !isBatchApi && !isJsonData && !isSubWorkflow && !nested && (
          <button
            className="wf-test-btn"
            onClick={() => { if (!testRunning) setTestOpen(!testOpen); }}
            title={isBatch ? t('wiz.testBatchStep') : t('wiz.testStep')}
            data-active={testOpen || testRunning}
          >
            {testRunning ? <Loader2 size={11} className="spin" /> : <FlaskConical size={11} />}
            {testRunning
              ? `${t('wiz.testRunning')} ${testElapsed}s`
              : (isBatch ? t('wiz.testBatchStep') : t('wiz.testStep'))
            }
          </button>
        )}
      </div>

      {isBatch ? (
        <div className="wf-batch-step-summary">
          {/* Referenced QP line (resolved to name + icon + agent, or raw id fallback) */}
          {batchQp ? (
            <div className="wf-batch-step-qp">
              <span className="wf-batch-step-qp-icon">{batchQp.icon}</span>
              <div className="flex-1" style={{ minWidth: 0 }}>
                <div className="flex-row gap-3" style={{ alignItems: 'baseline' }}>
                  <span className="font-semibold text-sm">{batchQp.name}</span>
                  <span className="text-xs text-dim">
                    → {AGENT_LABELS[batchQp.agent] ?? batchQp.agent}
                  </span>
                </div>
                {batchQp.description && (
                  <p className="text-xs text-muted" style={{ margin: '2px 0 0 0' }}>{batchQp.description}</p>
                )}
                {batchQp.variables.length > 0 && (
                  <p className="text-xs text-ghost" style={{ margin: '2px 0 0 0' }}>
                    {t('wiz.batchQPVarSubst', batchQp.variables[0].name)}
                  </p>
                )}
              </div>
            </div>
          ) : step.batch_quick_prompt_id ? (
            <div className="wf-batch-step-qp">
              <span className="wf-batch-step-qp-icon">?</span>
              <div>
                <span className="text-sm text-muted">{t('wf.batchStepQpMissing', step.batch_quick_prompt_id)}</span>
              </div>
            </div>
          ) : (
            <div className="wf-field-error">{t('wiz.batchQPRequired')}</div>
          )}

          {/* Items source */}
          <div className="wf-batch-step-row">
            <span className="text-xs text-muted">{t('wf.batchStepItemsLabel')}</span>
            <code className="wf-batch-step-items">
              {step.batch_items_from || t('wiz.batchItemsFromRequired')}
            </code>
          </div>

          {/* Flags row: wait / max / worktree */}
          <div className="wf-batch-step-flags">
            <span className="wf-batch-step-flag">
              {(step.batch_wait_for_completion ?? true)
                ? t('wf.batchStepWaits')
                : t('wf.batchStepFireForget')}
            </span>
            <span className="wf-batch-step-flag">
              {t('wf.batchStepMaxItems', step.batch_max_items ?? 50)}
            </span>
            {step.batch_workspace_mode === 'Isolated' && (
              <span className="wf-batch-step-flag wf-batch-step-flag-worktree">
                <GitBranch size={9} /> {t('wf.batchStepWorktree')}
              </span>
            )}
          </div>

          {/* QP Chain (Phase 2) — show the sequenced follow-up QPs if any */}
          {step.batch_chain_prompt_ids && step.batch_chain_prompt_ids.length > 0 && (
            <div className="wf-batch-step-row">
              <span className="text-xs text-muted">{t('wf.batchStepChainLabel')}</span>
              <div className="flex-row flex-wrap gap-2">
                {step.batch_chain_prompt_ids.map((qpId, chainIdx) => {
                  const qp = quickPromptsById?.get(qpId);
                  const label = qp ? `${qp.icon} ${qp.name}` : `⚠️ ${qpId}`;
                  return (
                    <span key={`${qpId}-${chainIdx}`} className="wf-chain-pill">
                      <span className="wf-chain-pos">{chainIdx + 1}.</span>
                      <span>{label}</span>
                    </span>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      ) : isSubWorkflow ? (
        <div className="wf-subworkflow-steps" style={{ marginTop: 6 }}>
          {subRefIsBundle ? (
            <p className="text-xs text-ghost" style={{ margin: 0 }}>
              {t('wiz.subWorkflowBundleNew')}
            </p>
          ) : childWf ? (
            <>
              <div className="text-2xs text-ghost" style={{ marginBottom: 6 }}>
                {t('wf.subWorkflowChildSteps', childWf.name, childWf.steps.length)}
              </div>
              {nested ? (
                // Already inside a sub-workflow expansion — don't recurse a
                // 3rd level of cards; show a compact list to bound depth.
                childWf.steps.map((cs, ci) => (
                  <div key={ci} className="flex-row gap-2" style={{ alignItems: 'center', padding: '2px 0' }}>
                    <span className="text-2xs text-ghost" style={{ minWidth: 14 }}>{ci + 1}.</span>
                    <span className="wf-step-kind-badge" data-kind={(cs.step_type?.type ?? 'Agent').toLowerCase()} style={{ fontSize: 10 }}>
                      {cs.step_type?.type ?? 'Agent'}
                    </span>
                    <span className="text-xs">{cs.name}</span>
                  </div>
                ))
              ) : (
                // Top level — render each child step as a FULL StepCard, like
                // a normal workflow, indented to show it lives in the child.
                <div
                  className="wf-subworkflow-nested"
                  style={{ borderLeft: '2px solid var(--kr-border-medium)', paddingLeft: 12, marginLeft: 2, display: 'flex', flexDirection: 'column', gap: 8 }}
                >
                  {childWf.steps.map((cs, ci) => (
                    <StepCard
                      key={ci}
                      step={cs}
                      index={ci}
                      nested
                      agentAccess={agentAccess}
                      projectId={childWf.project_id}
                      t={t}
                      quickPromptsById={quickPromptsById}
                      workflowId={childWf.id}
                      allSteps={childWf.steps}
                    />
                  ))}
                </div>
              )}
            </>
          ) : (
            <p className="text-xs text-ghost" style={{ margin: 0 }}>…</p>
          )}
        </div>
      ) : (
        <>
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
        </>
      )}
      {step.on_result && step.on_result.length > 0 && (
        <div className="mt-2 text-xs text-warning flex-row" style={{ gap: 6, flexWrap: 'wrap', alignItems: 'center' }}>
          {step.on_result.map((r, j) => {
            // For Goto: resolve target name → 1-based index so we can
            // render a step-number chip (same circular badge as the
            // step row) instead of a bare "Goto". Without this, the
            // recap showed "→ Goto" twice in a row with no hint at
            // which step the run jumps to (UX feedback 2026-04-29).
            const targetName = r.action.type === 'Goto' ? r.action.step_name : null;
            const targetIdx = targetName !== null
              ? allSteps.findIndex(s => s.name === targetName)
              : -1;
            return (
              <span key={j} className="flex-row" style={{ gap: 4, alignItems: 'center' }}>
                {t('wiz.ifContains')} "{r.contains}" &rarr; {r.action.type}
                {targetName !== null && targetIdx >= 0 && (
                  <span
                    className="wf-step-number wf-step-number-chip"
                    title={targetName}
                  >
                    {targetIdx + 1}
                  </span>
                )}
                {targetName !== null && targetIdx < 0 && (
                  /* Dangling reference — the target step was renamed or
                     deleted. Surface the broken edge instead of pretending
                     it's fine; user can fix in the wizard. */
                  <span className="text-error" title={`Step '${targetName}' introuvable`}>
                    ?{targetName}
                  </span>
                )}
              </span>
            );
          })}
        </div>
      )}

      {/* Test panel */}
      {testOpen && (
        <div className="wf-test-panel">
          <p className="wf-test-info">{isBatch ? t('wiz.testBatchInfo') : t('wiz.testInfo')}</p>
          {index > 0 && (
            <>
              <div className="flex-row gap-3" style={{ alignItems: 'baseline', marginBottom: 4, flexWrap: 'wrap' }}>
                <label className="text-xs text-muted">{t('wiz.testMockInput')}</label>
                {prefilledFromPrev && (
                  <span className="text-xs text-accent" style={{ fontStyle: 'italic' }}>
                    · {showFullPrev
                        ? t('wiz.testMockPrefilledFull', allSteps[index - 1]?.name ?? '')
                        : t('wiz.testMockPrefilledExtract', allSteps[index - 1]?.name ?? '')
                      }
                  </span>
                )}
                {/* Toggle between extracted last-line and full agent reply.
                    Only meaningful when the heuristic actually trimmed something
                    (i.e. the prev result has multiple lines or an envelope). */}
                {prefilledFromPrev && prevStepResult && extractLikelyOutput(prevStepResult).extracted && (
                  <button
                    type="button"
                    onClick={() => applyPrevStepResult(!showFullPrev)}
                    style={{
                      background: 'none',
                      border: 'none',
                      color: 'var(--kr-accent)',
                      fontSize: 11,
                      cursor: 'pointer',
                      textDecoration: 'underline',
                      padding: 0,
                    }}
                  >
                    {showFullPrev ? t('wiz.testMockShowExtract') : t('wiz.testMockShowFull')}
                  </button>
                )}
                {/* Manual re-fill button when the user has cleared/edited the
                    mock and wants to start over from the previous step output. */}
                {prevStepResult && !prefilledFromPrev && (
                  <button
                    type="button"
                    onClick={() => applyPrevStepResult(false)}
                    style={{
                      background: 'none',
                      border: 'none',
                      color: 'var(--kr-accent)',
                      fontSize: 11,
                      cursor: 'pointer',
                      textDecoration: 'underline',
                      padding: 0,
                    }}
                    title={t('wiz.testMockUsePrev', allSteps[index - 1]?.name ?? '')}
                  >
                    ↓ {t('wiz.testMockUsePrev', allSteps[index - 1]?.name ?? '')}
                  </button>
                )}
              </div>
              <textarea
                className="wf-test-textarea"
                rows={4}
                value={testMockInput}
                onChange={e => {
                  setTestMockInput(e.target.value);
                  // User typed something → no longer strictly pre-filled from prev.
                  setPrefilledFromPrev(false);
                }}
                placeholder={isBatch
                  ? '{"data":{"tickets":["EW-100","EW-101","EW-102"]},"status":"OK","summary":"3"}'
                  : t('wiz.testMockPlaceholder')
                }
              />
            </>
          )}
          <div className="wf-test-actions">
            {!isBatch && (
              <label className="wf-test-dryrun-toggle">
                <input type="checkbox" checked={testDryRun} onChange={e => setTestDryRun(e.target.checked)} />
                <span>{t('wiz.testDryRun')}</span>
              </label>
            )}
            {isBatch && (
              <span className="text-xs text-muted">{t('wiz.testBatchAlwaysDryRun')}</span>
            )}
            <div className="wf-test-actions-right">
              {testRunning && !isBatch && (
                <button className="wf-test-cancel-btn" onClick={() => current?.abortController?.abort()}>
                  <X size={12} /> {t('wiz.testCancel')}
                </button>
              )}
              <button
                className="wf-test-run-btn"
                onClick={isBatch ? handleBatchTest : handleTest}
                disabled={testRunning}
              >
                {testRunning ? <Loader2 size={14} className="spin" /> : <Play size={14} />}
                {testRunning ? t('wiz.testRunning') : t('wiz.testRun')}
              </button>
            </div>
          </div>
          {/* Agent step: live streaming output */}
          {!isBatch && (testRunning || testLiveOutput) && (
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
          {!isBatch && testResult && (
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

          {/* Batch step: dry-run preview result */}
          {isBatch && batchPreview && (
            <div className="wf-batch-preview mt-2">
              {batchPreview.errors.length > 0 ? (
                <div className="wf-test-error">
                  <strong>{t('wiz.testBatchErrors', batchPreview.errors.length)}</strong>
                  <ul style={{ margin: '4px 0 0 16px', padding: 0 }}>
                    {batchPreview.errors.map((err, i) => <li key={i}>{err}</li>)}
                  </ul>
                </div>
              ) : (
                <div className="wf-batch-preview-ok">
                  <strong>✓ {t('wiz.testBatchOk')}</strong>
                </div>
              )}
              {/* Non-blocking warnings (orange) — preview ran fine but the
                  config has a smell that would fail in production. */}
              {batchPreview.warnings.length > 0 && (
                <div className="wf-test-warning mt-2">
                  <strong>⚠️ {t('wiz.testBatchWarnings', batchPreview.warnings.length)}</strong>
                  <ul style={{ margin: '4px 0 0 16px', padding: 0 }}>
                    {batchPreview.warnings.map((w, i) => <li key={i}>{w}</li>)}
                  </ul>
                </div>
              )}
              <div className="wf-batch-preview-summary mt-2">
                <div className="text-sm">
                  <strong>{batchPreview.total_items}</strong> {t('wiz.testBatchItemsCount')}
                  {batchPreview.total_items > batchPreview.capped_at && (
                    <span className="text-xs text-muted">
                      {' '}({t('wiz.testBatchCappedAt', batchPreview.capped_at)})
                    </span>
                  )}
                </div>
                {batchPreview.quick_prompt_name && (
                  <div className="text-xs text-muted">
                    {batchPreview.quick_prompt_icon} {batchPreview.quick_prompt_name}
                    {' → '}{batchPreview.quick_prompt_agent}
                    {batchPreview.first_variable_name && (
                      <> · {t('wiz.testBatchFillsVar', batchPreview.first_variable_name)}</>
                    )}
                  </div>
                )}
                <div className="text-xs text-muted">
                  {t('wiz.testBatchWorkspace')}: <code>{batchPreview.workspace_mode}</code>
                  {' · '}
                  {batchPreview.wait_for_completion ? t('wiz.testBatchWaits') : t('wiz.testBatchFireForget')}
                </div>
              </div>
              {batchPreview.sample_items.length > 0 && (
                <div className="mt-3">
                  <div className="text-xs text-muted mb-2">
                    {t('wiz.testBatchItemsSample')}
                    {batchPreview.total_items > batchPreview.capped_at && (
                      <> · <em>{t('wiz.testBatchCappedAt', batchPreview.capped_at)}</em></>
                    )}
                  </div>
                  <BatchItemsList
                    items={batchPreview.sample_items}
                    renderedPrompts={batchPreview.sample_rendered_prompts}
                    quickPromptAgent={batchPreview.quick_prompt_agent}
                    projectId={projectId ?? null}
                    t={t}
                  />
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** Banner shown beneath the live run progress once the SSE stream emits
 *  its terminal `run_done` event. Three colour states:
 *    - **Success** : green check + `wf.runDone — Success`
 *    - **WaitingApproval** : amber Hand + `wf.runWaiting`. Distinct from
 *      the failed state so the operator instantly sees "your turn" vs
 *      "broken". Earlier 0.6.x versions painted this red, which read as
 *      an error even though the run was healthily paused on a Gate.
 *    - **anything else** (Failed / Cancelled / StoppedByGuard) : red X.
 *
 *  Exported so the unit tests in `__tests__/WorkflowDetail.test.tsx`
 *  can mount it in isolation without spinning up the whole detail tree.
 */
export function LiveFinishedBanner({
  status,
  stepsExecuted,
  t,
}: {
  status: string | null;
  stepsExecuted: number;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  const isSuccess = status === 'Success';
  const isWaiting = status === 'WaitingApproval';
  const dataStatus = isSuccess ? 'success' : isWaiting ? 'waiting' : 'failed';
  const color = isSuccess
    ? 'var(--kr-success)'
    : isWaiting
      ? 'var(--kr-warning)'
      : 'var(--kr-error)';
  const label = isWaiting ? t('wf.runWaiting') : t('wf.runDone', status ?? '');
  return (
    <div className="wf-live-finished" data-status={dataStatus}>
      {isSuccess
        ? <Check size={12} className="text-success" />
        : isWaiting
          ? <Hand size={12} style={{ color }} />
          : <X size={12} className="text-error" />}
      <span className="text-base font-semibold" style={{ color }}>{label}</span>
      <span className="text-xs text-dim">
        {t('wf.stepsExecuted', stepsExecuted)}
      </span>
    </div>
  );
}

/** Compact per-step descriptor for the collapsed pipeline view. The `kind`
 *  drives the chip color (mirrors the full StepCard badge vocabulary) and
 *  `usesTokens` splits steps into "agent" (LLM, costs tokens) vs the
 *  mechanical/deterministic ones (0 token) — the headline distinction Kronn
 *  sells. Agent + BatchQuickPrompt both run an LLM. */
/** 0.8.11 — per-step model tier for display. Surface only a non-default choice
 *  (economy/reasoning) — `default` is the norm, showing it would be noise. The
 *  emote mirrors the wizard selector so a step's model class is scannable. */
const TIER_EMOTE: Record<string, string> = { economy: '⚡', default: '🎯', reasoning: '🧠' };
function stepTier(step: WorkflowStep): 'economy' | 'reasoning' | null {
  const tr = step.agent_settings?.tier;
  return tr === 'economy' || tr === 'reasoning' ? tr : null;
}

/** Non-default model-tier badge for an Agent step. `chip` = compact (emote only)
 *  for the pipeline; otherwise emote + label for the detail card. Renders
 *  nothing for a default/unset tier. */
function TierBadge({ step, t, chip }: { step: WorkflowStep; t: (k: string) => string; chip?: boolean }) {
  const tr = stepTier(step);
  if (!tr) return null;
  const label = t(`disc.tier.${tr}`);
  return chip
    ? <span className="wf-pipe-chip-tier" title={label}>{TIER_EMOTE[tr]}</span>
    : <span className="wf-step-tier" title={label}>{TIER_EMOTE[tr]} {label}</span>;
}

function compactStepMeta(step: WorkflowStep): { kind: string; Icon: typeof Plug; usesTokens: boolean } {
  switch (step.step_type?.type) {
    case 'ApiCall': return { kind: 'api', Icon: Plug, usesTokens: false };
    case 'BatchApiCall': return { kind: 'batch-api', Icon: Layers, usesTokens: false };
    case 'Notify': return { kind: 'notify', Icon: Send, usesTokens: false };
    case 'Gate': return { kind: 'gate', Icon: Hand, usesTokens: false };
    case 'Exec': return { kind: 'exec', Icon: Terminal, usesTokens: false };
    case 'JsonData': return { kind: 'json-data', Icon: Braces, usesTokens: false };
    case 'BatchQuickPrompt': return { kind: 'batch-qp', Icon: Layers, usesTokens: true };
    default: return { kind: 'agent', Icon: Sparkles, usesTokens: true }; // Agent (or legacy undefined)
  }
}

export function WorkflowDetail({ workflow, runs, liveRun, onTrigger, onRefresh, onEdit, onDeleteRun, onDeleteAllRuns, triggering, agentAccess, onNavigateToBatch, onNavigateToWorkflow, onNavigateToRun, focusRunId, onExport, onGateDecided, onToggleEnabled, toast }: WorkflowDetailProps) {
  const { t } = useT();
  const [showRuns, setShowRuns] = useState(true);
  const [isWorkflowIdCopied, setIsWorkflowIdCopied] = useState(false);
  const workflowIdResetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (workflowIdResetTimer.current) clearTimeout(workflowIdResetTimer.current);
  }, []);
  const copyWorkflowId = async () => {
    try {
      await navigator.clipboard.writeText(workflow.id);
      setIsWorkflowIdCopied(true);
      if (workflowIdResetTimer.current) clearTimeout(workflowIdResetTimer.current);
      workflowIdResetTimer.current = setTimeout(() => setIsWorkflowIdCopied(false), 1500);
      toast?.(t('disc.idCopied'), 'success');
    } catch {
      setIsWorkflowIdCopied(false);
      toast?.(t('disc.idCopyFailed'), 'error');
    }
  };
  // Run-list control bar (#2): status filter + free-text search + fold past N.
  // Pure in-memory over `runs` already fetched — no backend round-trip.
  const [runFilter, setRunFilter] = useState<RunStatusFilter>('all');
  const [runSearch, setRunSearch] = useState('');
  const [showAllRuns, setShowAllRuns] = useState(false);
  // #6 — runs render as dense compact rows, expandable to the full RunDetail.
  // Terminal runs collapse (the 151-sub-run case); non-terminal ones stay open
  // by default because they need attention.
  const [expandedRunIds, setExpandedRunIds] = useState<Set<string>>(new Set());
  const isRunExpanded = (run: WorkflowRun): boolean =>
    expandedRunIds.has(run.id) ||
    run.status === 'Running' || run.status === 'Pending' || run.status === 'WaitingApproval';
  const toggleRunExpanded = (id: string) => setExpandedRunIds(prev => {
    const next = new Set(prev);
    if (next.has(id)) next.delete(id); else next.add(id);
    return next;
  });
  // #14 — sub-runs of one parent tick collapse under a group accordion.
  // Default open when the group holds a run still needing attention; a user
  // click records an explicit override that wins over the default.
  const [groupOverride, setGroupOverride] = useState<Record<string, boolean>>({});
  const isGroupExpanded = (key: string, groupRuns: WorkflowRun[]): boolean => {
    if (key in groupOverride) return groupOverride[key];
    return groupRuns.some(r => r.status === 'Running' || r.status === 'Pending' || r.status === 'WaitingApproval');
  };
  const toggleGroup = (key: string, groupRuns: WorkflowRun[]) =>
    setGroupOverride(prev => ({ ...prev, [key]: !isGroupExpanded(key, groupRuns) }));

  // #11 — when arriving with a focus run id (drill from a parent's sub-run
  // link), make sure it's visible: clear filters, unfold the list, expand the
  // run + its group, then scroll it into view.
  useEffect(() => {
    if (!focusRunId) return;
    const target = runs.find(r => r.id === focusRunId);
    if (!target) return;
    setRunFilter('all');
    setRunSearch('');
    setShowAllRuns(true);
    setExpandedRunIds(prev => new Set(prev).add(focusRunId));
    setGroupOverride(prev => ({ ...prev, [target.parent_run_id ?? focusRunId]: true }));
    const raf = requestAnimationFrame(() => {
      const el = document.querySelector(`[data-run-id="${focusRunId}"]`);
      // jsdom doesn't implement scrollIntoView; guard so tests don't throw.
      if (el && typeof (el as HTMLElement).scrollIntoView === 'function') {
        try { (el as HTMLElement).scrollIntoView({ behavior: 'smooth', block: 'center' }); } catch { /* no-op */ }
      }
    });
    return () => cancelAnimationFrame(raf);
  }, [focusRunId, runs]);
  // Steps panel collapses to a compact pipeline by default — the full
  // per-step cards (with prompts + Test buttons) are heavy and rarely what
  // you want at a glance, especially while a run is in flight. "Voir en
  // détails" expands the legacy card list.
  const [stepsExpanded, setStepsExpanded] = useState(false);
  // Per-step expand state for the live run view. Keyed by step name (not
  // index — order can shift if Goto loops re-fire a step). The user
  // clicks a completed step to inspect its output without leaving the
  // run-in-progress page; the SSE-fed `liveRun.completedSteps` keeps
  // the expanded body fresh as new chunks arrive on subsequent steps.
  const [expandedLiveSteps, setExpandedLiveSteps] = useState<Set<string>>(new Set());
  const toggleLiveStep = (stepName: string) => {
    setExpandedLiveSteps(prev => {
      const next = new Set(prev);
      if (next.has(stepName)) next.delete(stepName); else next.add(stepName);
      return next;
    });
  };

  // 1Hz tick to refresh the live elapsed-time badges (workflow total +
  // current step) without storing a recomputed string in state. Cheap
  // re-render — only the two duration spans depend on it. Stops as soon
  // as the live run finishes (cleared by the !active branch in the
  // effect's deps).
  const [, tickElapsed] = useState(0);
  // 1Hz tick to refresh elapsed badges. Runs whenever this detail page is
  // mounted — cost is one re-render per second of a single component,
  // cheap. The render itself short-circuits to nothing when no live view
  // is active (the live block is gated on `effectiveLiveRun`).
  useEffect(() => {
    const id = setInterval(() => tickElapsed(t => t + 1), 1000);
    return () => clearInterval(id);
  }, []);

  /** Format a millisecond duration as `Xs` for short, `MmSSs` past 60s. */
  const fmtDuration = (ms: number): string => {
    const s = Math.max(0, Math.floor(ms / 1000));
    return s >= 60 ? `${Math.floor(s / 60)}m${String(s % 60).padStart(2, '0')}s` : `${s}s`;
  };

  // 0.8.2 — When the user navigates away then back to a workflow that's
  // still running (or paused on a Gate), the SSE-fed `liveRun` is null
  // because the stream was tied to the original tab. Pre-fix, the page
  // collapsed to the static step-config view — no progress bar, no live
  // counter — and the user thought it was "stuck/blocked". Derive a
  // pseudo-live view from the latest non-finished run in `runs[]` so the
  // progress UI persists across page navigations. Streaming text
  // (`currentStepText`) stays empty — that's still SSE-only — but the
  // step list, durations, and current-step indicator come back.
  const effectiveLiveRun: LiveRunState | null = useMemo(() => {
    if (liveRun && liveRun.workflowId === workflow.id && !liveRun.finished) return liveRun;
    const activeRun = runs.find(r => r.status === 'Running' || r.status === 'WaitingApproval');
    if (!activeRun) return null;
    const completed = activeRun.step_results;
    // Current step = the next step in the workflow that hasn't been
    // recorded yet. If the trailing step_result is WaitingApproval, that
    // gate IS the "current" step. Otherwise, jump to the next.
    const last = completed[completed.length - 1];
    let currentStepName: string | null = null;
    if (last && last.status === 'WaitingApproval') {
      currentStepName = last.step_name;
    } else {
      const nextIdx = completed.length; // # of finished step_results
      currentStepName = workflow.steps[nextIdx]?.name ?? null;
    }
    return {
      workflowId: workflow.id,
      runId: activeRun.id,
      currentStep: currentStepName,
      stepIndex: completed.length,
      totalSteps: workflow.steps.length,
      // Only count fully-finished steps (Success/Failed) into completedSteps
      // so the trailing WaitingApproval row renders as "current" not "done".
      completedSteps: completed.filter(s => s.status === 'Success' || s.status === 'Failed'),
      currentStepText: '',
      startedAt: new Date(activeRun.started_at).getTime(),
      // currentStepStartedAt: prefer the running step's authoritative
      // started_at (stamped by the runner). If the step has no result yet
      // (very brief window between dispatch and the first persist), fall
      // back to the legacy estimate `runStart + sum of completed durations`.
      // Reading `last.started_at` (the gate's start) was wrong — it
      // produced a counter that included the gate pause in the next
      // step's elapsed.
      currentStepStartedAt: (() => {
        // If trailing step_result is WaitingApproval, `last` IS the
        // current step (the gate) — use its started_at directly.
        if (last && last.status === 'WaitingApproval' && last.started_at) {
          return new Date(last.started_at).getTime();
        }
        // Otherwise, look up the currently-running step's result.
        const runningResult = completed.find(s => s.step_name === currentStepName && s.status === 'Running');
        if (runningResult?.started_at) return new Date(runningResult.started_at).getTime();
        // Legacy fallback: estimate from completed durations. Correct as
        // long as the runner doesn't lag between steps (it usually doesn't).
        const sumCompleted = completed
          .filter(s => s.status === 'Success' || s.status === 'Failed')
          .reduce((acc, s) => acc + (s.duration_ms || 0), 0);
        return new Date(activeRun.started_at).getTime() + sumCompleted;
      })(),
      finished: false,
      status: activeRun.status,
    };
  }, [liveRun, runs, workflow.id, workflow.steps]);

  // Resolve Quick Prompts referenced by BatchQuickPrompt steps so the step card
  // can show the QP name/icon/description instead of just an opaque id. One
  // fetch per workflow view is fine; the list is small (<100 QPs typically).
  const [quickPromptsById, setQuickPromptsById] = useState<Map<string, QuickPrompt>>(new Map());
  const hasBatchStep = workflow.steps.some(s => s.step_type?.type === 'BatchQuickPrompt');
  useEffect(() => {
    if (!hasBatchStep) return;
    quickPromptsApi.list()
      .then(list => setQuickPromptsById(new Map(list.map(qp => [qp.id, qp]))))
      .catch(() => {});
  }, [hasBatchStep]);

  // Fetch batch summaries to map "linear run → its child batch" so the runs
  // list can show a "📋 N conversations" chip on each linear run that spawned
  // discussions. Only worth fetching when this workflow could have batch
  // children (= it has at least one BatchQuickPrompt step).
  const [batchByParentRunId, setBatchByParentRunId] = useState<Map<string, BatchRunSummary>>(new Map());
  useEffect(() => {
    if (!hasBatchStep) return;
    workflowsApi.listBatchRunSummaries()
      .then(list => {
        const m = new Map<string, BatchRunSummary>();
        for (const s of list) {
          if (s.parent_run_id) m.set(s.parent_run_id, s);
        }
        setBatchByParentRunId(m);
      })
      .catch(() => {});
  }, [hasBatchStep, runs.length]); // refetch when a new run lands

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
        <h2 className="text-lg font-bold" style={{ margin: 0 }}>{workflow.name}</h2>
        {/* Short workflow-id pill — same affordance as the disc-id pill:
            click copies the full id for pasting into a disc, a CLI or a
            linked issue (the title alone is ambiguous across variants). */}
        <button
          type="button"
          className="wf-id-pill"
          data-copied={isWorkflowIdCopied}
          onClick={() => void copyWorkflowId()}
          title={t('disc.idPillTooltip', workflow.id)}
          aria-label={t('disc.idPillTooltip', workflow.id)}
        >
          {isWorkflowIdCopied ? <Check size={8} /> : null}
          #{workflow.id.slice(0, 8)}
        </button>
        <span className="flex-1" />
        <button className="wf-small-btn" onClick={onEdit}>
          <Settings size={10} /> {t('wf.edit')}
        </button>
        {/* 0.7.0 UX pass — export button bundles the workflow + any
            referenced QPs into a single JSON file for sharing across
            instances. The download is triggered by the parent page. */}
        {onExport && (
          <button className="wf-small-btn" onClick={onExport} title={t('wf.exportHint')}>
            <Download size={10} /> {t('wf.export')}
          </button>
        )}
        <button className="wf-small-btn" onClick={onRefresh}>
          <RefreshCw size={10} /> {t('wf.refresh')}
        </button>
        <button
          className="wf-small-btn wf-small-btn-accent"
          onClick={onTrigger}
          disabled={!workflow.enabled || triggering}
          title={!workflow.enabled ? t('wf.launchDisabledHint') : undefined}
        >
          {triggering ? <Loader2 size={10} /> : <Play size={10} />}
          {t('wf.launch')}
        </button>
        {/* 0.8.11 UX — a disabled workflow used to leave "Lancer" silently
            inert (clone lands disabled by design → user clicks, nothing
            happens, no clue why). Make the state VISIBLE and actionable:
            an explanatory chip + a one-click enable. */}
        {!workflow.enabled && (
          <span className="wf-disabled-chip" title={t('wf.launchDisabledHint')}>
            ⏸ {t('wf.disabledChip')}
          </span>
        )}
        {!workflow.enabled && onToggleEnabled && (
          <button
            className="wf-small-btn wf-enable-btn"
            onClick={() => onToggleEnabled(true)}
            title={t('wf.launchDisabledHint')}
          >
            {t('wf.enableNow')}
          </button>
        )}
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

      {/* Steps — collapsed compact pipeline by default; "Voir en détails"
          reveals the full per-step cards. */}
      {(() => {
        const agentCount = workflow.steps.filter(s => compactStepMeta(s).usesTokens).length;
        const determCount = workflow.steps.length - agentCount;
        return (
          <div className="wf-steps-section" data-testid="wf-steps-section">
            <div className="wf-steps-head">
              <h3 className="wf-section-title" style={{ margin: 0 }}>
                {t('wf.stepsTitle', workflow.steps.length)}
              </h3>
              <span className="wf-steps-count" data-kind="agent" title={t('wf.stepsAgentHint')}>
                <Sparkles size={11} /> {t('wf.stepsAgentCount', agentCount)}
              </span>
              <span className="wf-steps-count" data-kind="determ" title={t('wf.stepsDetermHint')}>
                <Zap size={11} /> {t('wf.stepsDetermCount', determCount)}
              </span>
              <button
                type="button"
                className="wf-steps-toggle"
                data-testid="wf-steps-toggle"
                aria-expanded={stepsExpanded}
                onClick={() => setStepsExpanded(v => !v)}
              >
                <ChevronDown
                  size={13}
                  style={{ transform: stepsExpanded ? 'rotate(180deg)' : 'none', transition: 'transform 0.15s' }}
                />
                {stepsExpanded ? t('wf.stepsHideDetails') : t('wf.stepsShowDetails')}
              </button>
            </div>

            {/* Compact horizontal pipeline — number + kind icon + name. Click
                any chip to open the full detail. */}
            <div className="wf-steps-pipeline" role="list">
              {workflow.steps.map((step, i) => {
                const meta = compactStepMeta(step);
                const Icon = meta.Icon;
                // Show the agent identity (name + brand color) only on genuine
                // Agent steps — same whitelist as the detail card's `isAgentLike`
                // so both views read the same label. Batch/Exec/etc. delegate
                // and don't run `step.agent` directly.
                const isAgentStep = !step.step_type || step.step_type.type === 'Agent';
                return (
                  <Fragment key={i}>
                    {i > 0 && <ChevronRight size={13} className="wf-pipe-arrow" aria-hidden />}
                    <button
                      type="button"
                      className="wf-pipe-chip"
                      data-kind={meta.kind}
                      data-class={meta.usesTokens ? 'agent' : 'determ'}
                      role="listitem"
                      title={`${i + 1}. ${step.name}`}
                      onClick={() => setStepsExpanded(true)}
                    >
                      <span className="wf-pipe-chip-row">
                        <span className="wf-pipe-chip-num">{i + 1}</span>
                        <Icon size={11} />
                        <span className="wf-pipe-chip-name">{step.name}</span>
                      </span>
                      {isAgentStep && (
                        <span
                          className="wf-pipe-chip-agent"
                          style={{ color: AGENT_COLORS[step.agent] ?? 'var(--kr-text-faint)' }}
                        >
                          {AGENT_LABELS[step.agent] ?? step.agent}
                        </span>
                      )}
                      {isAgentStep && <TierBadge step={step} t={t} chip />}
                    </button>
                  </Fragment>
                );
              })}
            </div>

            {stepsExpanded && (
              <div className="wf-steps-detail" data-testid="wf-steps-detail">
                {/* #15 — branch map, only for workflows that actually branch. */}
                {hasBranches(workflow.steps) && <StepBranchMap steps={workflow.steps} t={t} />}
                {workflow.steps.map((step, i) => (
                  <StepCard key={i} step={step} index={i} agentAccess={agentAccess} projectId={workflow.project_id} t={t} quickPromptsById={quickPromptsById} workflowId={workflow.id} allSteps={workflow.steps} />
                ))}
              </div>
            )}
          </div>
        );
      })()}

      {/* Live run progress — driven by SSE when active in the current tab,
          or synthesized from `runs[]` when the user navigated away and
          back (cf. `effectiveLiveRun` useMemo above). The IIFE aliases
          `effectiveLiveRun` back to `liveRun` so the existing JSX (≈ 14
          references) doesn't need a sweep. */}
      {effectiveLiveRun && (() => { const liveRun = effectiveLiveRun; return (
        <div className="wf-live-run">{/* live block — */}
          <div className="flex-row gap-4 mb-5" style={{ alignItems: 'center' }}>
            <Loader2 size={12} className="wf-spin" style={{ color: 'var(--kr-cyan)' }} />
            <span className="text-base font-bold" style={{ color: 'var(--kr-cyan)' }}>
              {t('wf.running')}
            </span>
            {effectiveLiveRun.totalSteps > 0 && (
              <span className="text-xs text-muted">
                ({effectiveLiveRun.completedSteps.length}/{effectiveLiveRun.totalSteps} steps)
              </span>
            )}
            {/* Total = sum of completed step durations + elapsed on the
                current step. By construction the badge equals the sum of
                the per-step badges, so users don't see "total 1m20" while
                the steps below add up to 50s. Wall-clock from click would
                include scheduler/SSE latency that's noise to the user. */}
            <span className="text-xs text-ghost" title={t('wf.live.totalElapsedHint')}>
              ⏱ {fmtDuration(
                liveRun.completedSteps.reduce((acc, s) => acc + (s.duration_ms || 0), 0)
                + (liveRun.currentStepStartedAt ? Date.now() - liveRun.currentStepStartedAt : 0)
              )}
            </span>
            {/* Stop the in-flight run. Disabled until run_start lands
                (we need the run_id to call cancelRun). Cancelling
                cascades to in-flight agents + child batches; the runner
                short-circuits at the next checkpoint. */}
            {liveRun.runId && (
              <button
                className="wf-run-cancel-btn"
                style={{ marginLeft: 'auto' }}
                onClick={async () => {
                  if (!liveRun.runId) return;
                  if (!confirm(t('wf.cancelRunConfirm'))) return;
                  try {
                    await workflowsApi.cancelRun(workflow.id, liveRun.runId);
                  } catch (e) {
                    console.warn('cancelRun failed:', e);
                  }
                }}
                title={t('wf.cancelRun')}
              >
                <Square size={10} style={{ fill: 'currentColor' }} />
                {t('wf.cancelRun')}
              </button>
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

          {/* Step indicators — completed rows are clickable to inspect
              the output without reloading. The expanded body uses the
              same `liveRun.completedSteps` that the SSE stream feeds, so
              it stays fresh as the run progresses. */}
          {workflow.steps.map((step, i) => {
            const completed = liveRun.completedSteps.find(s => s.step_name === step.name);
            const isCurrent = liveRun.currentStep === step.name;
            const isPending = !completed && !isCurrent;
            const isExpanded = expandedLiveSteps.has(step.name);
            // Both completed AND in-flight rows are expandable. The
            // in-flight one streams the agent's stdout chunks as they
            // arrive (`liveRun.currentStepText`), the completed ones
            // show their full saved output. Pending rows stay inert.
            const isClickable = !!completed || isCurrent;

            const stepState = completed ? 'completed' : isCurrent ? 'current' : 'pending';
            const iconState = completed
              ? (completed.status === 'Success' ? 'success' : 'failed')
              : isCurrent ? 'current' : 'pending';

            const RowTag = isClickable ? 'button' : 'div';
            return (
              <div key={i}>
                <RowTag
                  className="wf-live-step"
                  data-state={isCurrent ? 'current' : 'other'}
                  data-expanded={isExpanded}
                  // The whole row is the click target on completed/current
                  // steps — bigger hit area, no extra chevron column.
                  // Pending rows stay as <div> so they don't suggest
                  // interactivity that doesn't exist yet.
                  {...(isClickable ? {
                    onClick: () => toggleLiveStep(step.name),
                    type: 'button' as const,
                    'aria-expanded': isExpanded,
                  } : {})}
                  style={isClickable ? { cursor: 'pointer', width: '100%', textAlign: 'left', background: 'transparent', border: 'none', padding: 0 } : undefined}
                >
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

                  {/* 0.8.2 — Per-step token badge. Mirrors RunDetail. Helps
                      spot which steps burn the most tokens (analyze /
                      implement / review are usually the heavies; the
                      "désagentification" pattern moves the cheap mechanical
                      ones to ApiCall / Exec / Notify). */}
                  {completed && completed.tokens_used > 0 && (
                    <span
                      className="text-2xs text-ghost"
                      title={t('wf.stepTokensHint')}
                      style={{ color: 'var(--kr-accent-ink)' }}
                    >
                      {completed.tokens_used.toLocaleString()} {t('wf.stepTokensSuffix')}
                    </span>
                  )}

                  {/* Current step indicator + live elapsed */}
                  {isCurrent && (
                    <>
                      <span className="wf-live-step-in-progress">
                        {t('wf.inProgress')}
                      </span>
                      {liveRun.currentStepStartedAt && (
                        <span className="text-2xs text-ghost">
                          {fmtDuration(Date.now() - liveRun.currentStepStartedAt)}
                        </span>
                      )}
                    </>
                  )}

                  {isPending && (
                    <span className="text-2xs" style={{ color: 'var(--kr-border-medium)' }}>
                      {t('wf.pending')}
                    </span>
                  )}

                  {/* Chevron — collapsed/expanded affordance for any
                      clickable row (completed OR current-streaming).
                      Hidden on pending so the row doesn't look
                      interactive when it isn't. */}
                  {isClickable && (
                    <ChevronRight
                      size={10}
                      className="wf-chevron"
                      data-expanded={isExpanded}
                      style={{ marginLeft: 'auto' }}
                    />
                  )}
                </RowTag>

                {/* Inline expanded body — content depends on the row state:
                      - completed: the step's saved `output` (markdown / JSON
                        envelope / exec stdout, depending on step type)
                      - current: the streaming `currentStepText` accumulated
                        from SSE `step_progress` chunks (live!), with a
                        "still streaming" cursor while it grows */}
                {isExpanded && (completed || isCurrent) && (
                  <div
                    className="wf-live-step-body"
                    style={{
                      margin: '4px 0 8px 28px',
                      padding: '8px 12px',
                      background: 'rgba(var(--kr-text-faint-rgb, 128, 128, 128), 0.04)',
                      borderLeft: `2px solid ${isCurrent ? 'var(--kr-cyan)' : 'var(--kr-border-faint)'}`,
                      borderRadius: 4,
                      fontSize: 11,
                      maxHeight: 360,
                      overflow: 'auto',
                    }}
                    aria-live={isCurrent ? 'polite' : undefined}
                  >
                    <pre style={{ margin: 0, whiteSpace: 'pre-wrap', fontFamily: 'var(--kr-font-mono)', fontSize: 11 }}>
                      {completed
                        ? (completed.output || t('wf.live.stepNoOutput'))
                        : (liveRun.currentStepText
                            ? <>{liveRun.currentStepText}<span className="wf-streaming-cursor" style={{ opacity: 0.6 }}>▊</span></>
                            : t(liveStepWaitingKey(step)))}
                    </pre>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      ); })()}

      {/* Live run finished banner. Extracted to <LiveFinishedBanner> so its
          three-state colour mapping (success/waiting/failed) can be unit
          tested without booting the whole WorkflowDetail tree. */}
      {liveRun && liveRun.workflowId === workflow.id && liveRun.finished && (
        <LiveFinishedBanner
          status={liveRun.status}
          stepsExecuted={liveRun.completedSteps.length}
          t={t}
        />
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

      {/* #2 — control bar: only when the list is long enough to warrant it. */}
      {showRuns && runs.length > RUN_PAGE_SIZE && (
        <div className="wf-runs-controls">
          <div className="wf-runs-filter" role="group" aria-label={t('wf.runs.filterLabel')}>
            {(['all', 'failed', 'waiting', 'stopped'] as const).map(f => (
              <button
                key={f}
                type="button"
                className="wf-runs-filter-btn"
                data-active={runFilter === f}
                onClick={() => { setRunFilter(f); setShowAllRuns(false); }}
              >{t(`wf.runs.filter.${f}`)}</button>
            ))}
          </div>
          <div className="wf-runs-search">
            <Search size={12} />
            <input
              type="search"
              className="wf-runs-search-input"
              placeholder={t('wf.runs.searchPlaceholder')}
              value={runSearch}
              onChange={e => { setRunSearch(e.target.value); setShowAllRuns(false); }}
              aria-label={t('wf.runs.searchPlaceholder')}
            />
          </div>
        </div>
      )}

      {(() => {
        if (!showRuns) return null;
        const visible = filterRuns(runs, runFilter, runSearch);
        const shown = showAllRuns ? visible : visible.slice(0, RUN_PAGE_SIZE);
        const hidden = visible.length - shown.length;
        const groups = groupRunsByParent(shown);
        const renderRunItem = (run: WorkflowRun) => {
        // If this linear run spawned a batch (BatchQuickPrompt step), show a
        // "📋 N conversations" chip pointing to the discussions tab.
        const childBatch = batchByParentRunId.get(run.id);
        const expanded = isRunExpanded(run);
        const durMs = run.finished_at
          ? new Date(run.finished_at).getTime() - new Date(run.started_at).getTime()
          : null;
        return (
          <div key={run.id} className="wf-run-item" data-testid="wf-run-item" data-run-id={run.id}>
            {/* #6 — compact row = the always-visible accordion header. */}
            <button
              type="button"
              className="wf-run-compact"
              data-status={run.status}
              aria-expanded={expanded}
              onClick={() => toggleRunExpanded(run.id)}
            >
              <span className="wf-step-dot wf-run-compact-dot" style={{ width: 8, height: 8 }} />
              <span className="wf-run-compact-status">{run.status}</span>
              {run.parent_workflow_name && (
                <span className="wf-run-compact-parent" title={run.parent_workflow_name}>↳ {run.parent_workflow_name}</span>
              )}
              <span className="wf-run-compact-time">{new Date(run.started_at).toLocaleString()}</span>
              <span className="wf-run-compact-meta">
                {durMs != null && formatDurationCompact(durMs)}
                {run.tokens_used > 0 && ` · ${run.tokens_used} tk`}
              </span>
              <ChevronRight size={12} className={expanded ? 'wf-chevron-rotated' : 'wf-chevron'} />
            </button>
            {expanded && (<>
            <RunDetail
              run={run}
              workflowSteps={workflow.steps}
              onNavigateToWorkflow={onNavigateToWorkflow}
              onNavigateToRun={onNavigateToRun}
              onDelete={() => onDeleteRun(run.id)}
              onCancel={async () => {
                try {
                  const res = await workflowsApi.cancelRun(workflow.id, run.id);
                  // No toast system here — use a silent refresh so the user
                  // sees the status flip to Cancelled without a popup.
                  // (Removed console.info — eslint no-console; the WS event
                  // path provides confirmation.)
                  void res;
                  onRefresh();
                } catch {
                  // Cancel errors are rare (run already finished, registry
                  // poisoned) — the UI refreshes automatically via onRefresh
                  // so we just swallow silently.
                }
              }}
              onResume={async () => {
                try {
                  await workflowsApi.resumeRun(run.id);
                  onRefresh();
                } catch (e) {
                  // Claim lost (double-click, benign: the other click won) or
                  // worktree gone — same silent-refresh convention as onCancel
                  // above; the precise backend message lands in the console.
                  console.warn('resumeRun failed:', e);
                  onRefresh();
                }
              }}
              onDecide={async (payload) => {
                await workflowsApi.decideRun(workflow.id, run.id, payload);
                onRefresh();
                // Wipe the now-stale "Awaiting your decision" live banner.
                // The post-decision progress is visible in the runs list,
                // which onRefresh just reloaded.
                onGateDecided?.();
              }}
            />
            {childBatch && onNavigateToBatch && (
              <button
                type="button"
                className="wf-run-batch-chip"
                onClick={() => onNavigateToBatch(childBatch.run_id)}
                title={t('wf.runBatchChipHint')}
              >
                <MessageSquare size={11} />
                {t('wf.runBatchChip', childBatch.batch_total)}
                {childBatch.status === 'Running' && (
                  <span className="wf-run-batch-chip-status">⏳</span>
                )}
                <ChevronRight size={10} />
              </button>
            )}
            </>)}
          </div>
        );
        };
        return (
          <>
            {runs.length > RUN_PAGE_SIZE && visible.length === 0 && (
              <p className="text-sm text-faint mt-4">{t('wf.runs.noMatch')}</p>
            )}
            {groups.map(g => {
              // A real multi-child parent tick → group accordion. Standalone or
              // single runs render as plain compact rows (no group header).
              if (g.parentRunId && g.runs.length > 1) {
                const gExpanded = isGroupExpanded(g.key, g.runs);
                const failed = g.runs.filter(r => r.status === 'Failed').length;
                return (
                  <div key={g.key} className="wf-run-group">
                    <button
                      type="button"
                      className="wf-run-group-header"
                      aria-expanded={gExpanded}
                      onClick={() => toggleGroup(g.key, g.runs)}
                    >
                      <ChevronRight size={12} className={gExpanded ? 'wf-chevron-rotated' : 'wf-chevron'} />
                      <span className="wf-run-group-title">
                        {g.tickAt ? new Date(g.tickAt).toLocaleString() : (g.parentName ?? t('wf.runs.groupFallback'))}
                      </span>
                      <span className="wf-run-group-meta">
                        {t('wf.runs.groupSummary', g.runs.length)}
                        {failed > 0 && <span className="wf-run-group-failed"> · {t('wf.runs.groupFailed', failed)}</span>}
                      </span>
                    </button>
                    {gExpanded && <div className="wf-run-group-body">{g.runs.map(renderRunItem)}</div>}
                  </div>
                );
              }
              return <Fragment key={g.key}>{g.runs.map(renderRunItem)}</Fragment>;
            })}
            {hidden > 0 && (
              <button
                type="button"
                className="wf-runs-show-more"
                onClick={() => setShowAllRuns(true)}
              >{t('wf.runs.showMore', hidden)}</button>
            )}
          </>
        );
      })()}
    </div>
  );
}
