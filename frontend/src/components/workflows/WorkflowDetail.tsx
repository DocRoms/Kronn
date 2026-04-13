import { useState, useRef, useEffect } from 'react';
import { useT } from '../../lib/I18nContext';
import { workflows as workflowsApi, quickPrompts as quickPromptsApi } from '../../lib/api';
import { AGENT_COLORS, AGENT_LABELS, isAgentRestricted } from '../../lib/constants';
import type { Workflow, WorkflowRun, StepResult, AgentsConfig, WorkflowStep, QuickPrompt, BatchRunSummary, AgentType } from '../../types/generated';
import {
  Trash2, Play, Loader2, Check, X, ChevronRight,
  Settings, RefreshCw, AlertTriangle, FlaskConical,
  Layers, GitBranch, MessageSquare,
} from 'lucide-react';
import { RunDetail } from './RunDetail';
import '../../pages/WorkflowsPage.css';

const checkAgentRestricted = isAgentRestricted;

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
  result: import('../../types/generated').StepResult | null;
  error: string | null;
  batchPreview: import('../../lib/api').BatchPreview | null;
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
  let set = stepTestListeners.get(key);
  if (!set) { set = new Set(); stepTestListeners.set(key, set); }
  set.add(cb);
  return () => {
    set!.delete(cb);
    if (set!.size === 0) stepTestListeners.delete(key);
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
  /** Click on a "📋 N conversations" chip → jump to the discussions tab and
   *  expand+scroll to the matching batch group. */
  onNavigateToBatch?: (batchRunId: string) => void;
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

function StepCard({ step, index, agentAccess, projectId, t, quickPromptsById, workflowId, allSteps }: {
  step: WorkflowStep; index: number; agentAccess?: AgentsConfig | null;
  projectId?: string | null; t: (key: string, ...args: (string | number)[]) => string;
  quickPromptsById?: Map<string, QuickPrompt>;
  /** Workflow id is needed to key the dry-run test state cache (see module
   *  comment on `stepTestCache`) so the panel survives navigation. */
  workflowId: string;
  /** Full step list — lets StepCard look up the previous step's cached test
   *  result to auto-fill the mock input when the user opens the test panel. */
  allSteps: WorkflowStep[];
}) {
  const isBatch = step.step_type?.type === 'BatchQuickPrompt';
  const batchQp = isBatch && step.batch_quick_prompt_id
    ? quickPromptsById?.get(step.batch_quick_prompt_id)
    : undefined;

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
    if (!testRunning || !current?.startedAt) { setTestElapsed(0); return; }
    const tick = () => setTestElapsed(Math.floor((Date.now() - current.startedAt!) / 1000));
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
  //
  // Heuristic, in priority order:
  //   1. STEP_OUTPUT envelope (Structured mode) → take its contents
  //   2. Last non-empty line if it looks like a list (commas, semicolons,
  //      JSON-like brackets, or short enough to be data not prose)
  //   3. Fall back to the whole output (let the user clean it up)
  const extractLikelyOutput = (raw: string): { value: string; extracted: boolean } => {
    if (!raw) return { value: '', extracted: false };
    const envMatch = raw.match(/---STEP_OUTPUT---([\s\S]+?)---END_STEP_OUTPUT---/);
    if (envMatch) return { value: envMatch[1].trim(), extracted: true };
    const lines = raw.split('\n').map(l => l.trim()).filter(Boolean);
    if (lines.length === 0) return { value: '', extracted: false };
    if (lines.length === 1) return { value: lines[0], extracted: false };
    const last = lines[lines.length - 1];
    const looksLikeData = (
      last.length < 200 ||
      last.includes(',') ||
      last.includes(';') ||
      last.startsWith('[') ||
      last.startsWith('{')
    );
    if (looksLikeData) return { value: last, extracted: true };
    return { value: raw, extracted: false };
  };

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

  return (
    <div className="wf-step-card" data-step-type={isBatch ? 'batch-qp' : 'agent'}>
      <div className="flex-row gap-4">
        <span className="wf-step-number">{index + 1}</span>
        <span className="font-semibold text-md">{step.name}</span>
        {isBatch ? (
          <span className="wf-step-kind-badge" title={t('wiz.stepTypeBatchQPHint')}>
            <Layers size={10} /> {t('wiz.stepTypeBatchQP')}
          </span>
        ) : (
          <span className="text-xs font-semibold" style={{ color: AGENT_COLORS[step.agent] ?? '#888' }}>
            {AGENT_LABELS[step.agent] ?? step.agent}
          </span>
        )}
        {!isBatch && checkAgentRestricted(agentAccess ?? undefined, step.agent) && (
          <span className="flex-row gap-1 text-xs text-warning">
            <AlertTriangle size={10} />
            {t('config.restrictedStep')}
          </span>
        )}
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
        <div className="mt-2 text-xs text-warning">
          {step.on_result.map((r, j) => (
            <span key={j}>{t('wiz.ifContains')} "{r.contains}" &rarr; {r.action.type} </span>
          ))}
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

export function WorkflowDetail({ workflow, runs, liveRun, onTrigger, onRefresh, onEdit, onDeleteRun, onDeleteAllRuns, triggering, agentAccess, onNavigateToBatch }: WorkflowDetailProps) {
  const { t } = useT();
  const [showRuns, setShowRuns] = useState(true);

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
        <StepCard key={i} step={step} index={i} agentAccess={agentAccess} projectId={workflow.project_id} t={t} quickPromptsById={quickPromptsById} workflowId={workflow.id} allSteps={workflow.steps} />
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

      {showRuns && runs.map(run => {
        // If this linear run spawned a batch (BatchQuickPrompt step), show a
        // "📋 N conversations" chip pointing to the discussions tab.
        const childBatch = batchByParentRunId.get(run.id);
        return (
          <div key={run.id}>
            <RunDetail
              run={run}
              workflowSteps={workflow.steps}
              onDelete={() => onDeleteRun(run.id)}
              onCancel={async () => {
                try {
                  const res = await workflowsApi.cancelRun(workflow.id, run.id);
                  // No toast system here — use a silent refresh so the user
                  // sees the status flip to Cancelled without a popup.
                  console.info('Cancelled run:', res);
                  onRefresh();
                } catch (e) {
                  console.warn('Cancel failed:', e);
                  alert(t('wf.cancelRunError'));
                }
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
          </div>
        );
      })}
    </div>
  );
}
