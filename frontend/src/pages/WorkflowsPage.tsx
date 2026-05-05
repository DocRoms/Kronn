import { useState, useRef, useMemo, useEffect } from 'react';
import { useIsMobile } from '../hooks/useMediaQuery';
import { useT } from '../lib/I18nContext';
import { workflows as workflowsApi, discussions as discussionsApi, quickPrompts as quickPromptsApi, quickApis as quickApisApi, mcps as mcpsApi } from '../lib/api';
import { userError } from '../lib/userError';
import { useApi } from '../hooks/useApi';
import type {
  Project, WorkflowSummary, Workflow, WorkflowRun,
  AgentType, AgentsConfig, StepResult, QuickPrompt, CreateQuickPromptRequest,
  QuickApi, CreateQuickApiRequest,
} from '../types/generated';
import type { ApiPluginOption } from '../components/workflows/ApiCallStepCard';
import {
  Plus, Trash2, Play, Loader2, ChevronLeft, ChevronRight, ChevronDown,
  Clock, GitBranch, Zap, Eye, Layers, X, Square,
  ToggleLeft, ToggleRight,
  Upload, Download, AlertTriangle,
} from 'lucide-react';
import { WorkflowDetail } from '../components/workflows/WorkflowDetail';
import { WorkflowWizard } from '../components/workflows/WorkflowWizard';
import { QuickPromptForm } from '../components/workflows/QuickPromptForm';
import { QuickApiForm } from '../components/workflows/QuickApiForm';
import { parseBatchQAItems } from '../components/workflows/parseBatchQAItems';
import { ImportDropzone } from '../components/workflows/ImportDropzone';
import { triggerDownload } from '../lib/downloadBlob';
import { MatrixText } from '../components/MatrixText';
import './WorkflowsPage.css';

interface WorkflowsPageProps {
  projects: Project[];
  installedAgentTypes?: AgentType[];
  agentAccess?: AgentsConfig;
  configLanguage?: string;
  onNavigateDiscussion?: (discId: string) => void;
  /** Called after a batch launch — parent marks every returned disc id as
   * "sending" in the shared sendingMap so the sidebar spinner lights up for
   * all of them, not just the one we navigate to. Without this prop the
   * batch still works but only the navigated disc looks like it's running. */
  onBatchLaunched?: (discIds: string[], batchRunId: string) => void;
  /** When the user clicks a batch pastille in the discussion sidebar, the
   * Dashboard switches to this tab and sets this prop to the parent workflow
   * id. We auto-open its detail panel + switch to the 'workflows' sub-tab
   * so the user lands exactly on the run that spawned their batch. */
  initialSelectedWorkflowId?: string | null;
  /** Ack callback — Dashboard clears the id after we've consumed it so the
   * same click doesn't re-open on every render. */
  onInitialSelectionConsumed?: () => void;
  /** Reverse direction: when "📋 N conversations" is clicked on a workflow run,
   * jump to the discussions tab and focus that batch group. */
  onNavigateToBatch?: (batchRunId: string) => void;
  toast?: (msg: string, type?: 'success' | 'error' | 'info') => void;
}

const TRIGGER_LABELS: Record<string, string> = {
  cron: 'Cron',
  tracker: 'Tracker',
  manual: 'Manuel',
};

/** FIFO-cap a live text buffer at `max` characters — keep the trailing
 *  window, drop the oldest content. Used to bound the live-progress feed
 *  on long Agent steps so React doesn't re-render a multi-MB `<pre>`.
 *
 *  Exported so unit tests cover the boundary cases (exact-fit, just over,
 *  much over, multibyte safety) without instantiating the full page. */
export function appendLiveBuffer(prev: string, chunks: string, max: number): string {
  const merged = prev + chunks;
  if (merged.length <= max) return merged;
  return merged.slice(merged.length - max);
}

const STATUS_COLORS: Record<string, string> = {
  Pending: 'var(--kr-warning)',
  Running: 'var(--kr-cyan)',
  Success: 'var(--kr-success)',
  Failed: 'var(--kr-error)',
  Cancelled: 'var(--kr-cancelled)',
  WaitingApproval: 'var(--kr-accent-ink)',
  // 0.7.0 — `StoppedByGuard` is a self-protection stop, not a failure.
  // Antoine UX rationale: orange (warning) not red (error) so users
  // distinguish "I asked Kronn to stop me" from "something went wrong".
  StoppedByGuard: 'var(--kr-warning)',
};

export function WorkflowsPage({ projects, installedAgentTypes, agentAccess, configLanguage, onNavigateDiscussion, onBatchLaunched, initialSelectedWorkflowId, onInitialSelectionConsumed, onNavigateToBatch, toast: toastProp }: WorkflowsPageProps) {
  const { t } = useT();
  const isMobile = useIsMobile();
  const [tab, setTab] = useState<'workflows' | 'quickPrompts' | 'quickApis'>('workflows');
  const { data: workflowList, refetch } = useApi(() => workflowsApi.list(), []);
  const { data: quickPromptList, refetch: refetchQP } = useApi(() => quickPromptsApi.list(), []);
  const { data: quickApiList, refetch: refetchQA } = useApi(() => quickApisApi.list(), []);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [showCreateQP, setShowCreateQP] = useState(false);
  const [editingQP, setEditingQP] = useState<QuickPrompt | null>(null);
  const [launchingQP, setLaunchingQP] = useState<QuickPrompt | null>(null);
  const [launchVars, setLaunchVars] = useState<Record<string, string>>({});
  const [launching, setLaunching] = useState(false);
  // Batch launch state — when the user clicks "Batch" on a QP, we show a
  // modal that asks for one value of the first variable per line, then
  // fans out N discussions via POST /api/quick-prompts/:id/batch.
  const [batchingQP, setBatchingQP] = useState<QuickPrompt | null>(null);
  const [batchInputLines, setBatchInputLines] = useState('');
  const [batchLaunching, setBatchLaunching] = useState(false);
  const [batchIsolated, setBatchIsolated] = useState(false);
  // Quick APIs state (mirrors QP). Run drawer is deferred to v3 — for v2,
  // editing + saving + ApiCallStepCard's built-in "Test the call" cover
  // the validation loop.
  const [showCreateQA, setShowCreateQA] = useState(false);
  const [editingQA, setEditingQA] = useState<QuickApi | null>(null);
  // Direct-run drawer state. Mirror of `launchingQP` for QAs: opens a
  // modal asking for values for each declared variable, fires the
  // `/api/quick-apis/:id/run` endpoint, displays the response inline.
  const [launchingQA, setLaunchingQA] = useState<QuickApi | null>(null);
  const [launchVarsQA, setLaunchVarsQA] = useState<Record<string, string>>({});
  const [launchingQARun, setLaunchingQARun] = useState(false);
  const [launchQAResult, setLaunchQAResult] = useState<{ ok: boolean; payload: unknown; error: string | null } | null>(null);
  // Batch state — same pattern as `batchingQP` but for QAs. Items is a
  // newline/comma/semicolon-separated string the user pastes; we parse
  // it into a JSON array (strings if one var on the QA, otherwise prompt
  // for object form) before posting to /api/quick-apis/:id/batch.
  const [batchingQA, setBatchingQA] = useState<QuickApi | null>(null);
  const [batchQAInput, setBatchQAInput] = useState('');
  const [batchQAConcurrentLimit, setBatchQAConcurrentLimit] = useState(5);
  const [batchQARunning, setBatchQARunning] = useState(false);
  const [batchQAResult, setBatchQAResult] = useState<{ status: string; items: Array<{ input: unknown; status: string; response?: unknown; error?: string; http_status?: number }>; total: number; succeeded: number; failed: number } | null>(null);
  // Lignes du tableau de résultats batch dépliées (Set d'index). On garde
  // un Set plutôt qu'un single index pour permettre la comparaison de
  // plusieurs réponses côte-à-côte. Reset à chaque nouveau batch.
  const [expandedQARows, setExpandedQARows] = useState<Set<number>>(new Set());
  // Available API plugins (with `api_spec != null` and a configured
  // backend) — needed by `ApiCallStepCard` inside `QuickApiForm`. Loaded
  // lazily on tab open since not every user wires API plugins.
  const [availableApiPlugins, setAvailableApiPlugins] = useState<ApiPluginOption[]>([]);
  const [showCreate, setShowCreate] = useState(false);
  const [editingWorkflow, setEditingWorkflow] = useState<Workflow | null>(null);
  // 0.7.0 UX pass — import drawer state. Set when the user clicks the
  // "Importer" button on either tab. Carries the parsed JSON content +
  // a small preview snapshot so the drawer can render "tu vas importer
  // X" before confirm.
  const [importing, setImporting] = useState<{
    kind: 'workflow' | 'qp';
    content: string;
    preview: { name: string; stepCount?: number; qpVarsCount?: number };
    targetProjectId: string;
  } | null>(null);
  const [importingSubmit, setImportingSubmit] = useState(false);
  // 0.6.0 UX pass — Launch modal state for workflows declaring variables.
  // null = no modal open. submitting=true = trigger in flight, fields disabled.
  const [launchingWorkflow, setLaunchingWorkflow] = useState<{
    workflow: Workflow;
    values: Record<string, string>;
    submitting: boolean;
    error: string | null;
  } | null>(null);
  const [detailWorkflow, setDetailWorkflow] = useState<Workflow | null>(null);
  const [detailRuns, setDetailRuns] = useState<WorkflowRun[]>([]);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [triggering, setTriggering] = useState<string | null>(null);
  const [cancellingRunIds, setCancellingRunIds] = useState<Set<string>>(new Set());

  // Live run state for SSE streaming
  const [liveRun, setLiveRun] = useState<{
    workflowId: string;
    runId: string | null;
    currentStep: string | null;
    stepIndex: number;
    totalSteps: number;
    completedSteps: StepResult[];
    /** Live agent output for the step currently in flight. Reset on each
     *  StepStart, accumulated on every StepProgress chunk, never moves to
     *  a completed slot (that's covered by `completedSteps[].output`). */
    currentStepText: string;
    startedAt: number;
    currentStepStartedAt: number | null;
    finished: boolean;
    status: string | null;
  } | null>(null);
  const abortRef = useRef<AbortController | null>(null);

  const workflows = workflowList ?? [];
  // Persist collapse state across reloads — same convention as the
  // discussions sidebar (`kronn:discCollapsedGroups`). Without this the
  // user re-collapses every project group on every nav back to this
  // page (UX feedback 2026-04-29). Different storage key from
  // discussions so the two lists track independently.
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>(() => {
    try {
      const saved = localStorage.getItem('kronn:wfCollapsedGroups');
      return saved ? JSON.parse(saved) as Record<string, boolean> : {};
    } catch { return {}; }
  });
  useEffect(() => {
    localStorage.setItem('kronn:wfCollapsedGroups', JSON.stringify(collapsedGroups));
  }, [collapsedGroups]);

  const groupedWorkflows = useMemo(() => {
    const groups: { key: string; label: string; workflows: WorkflowSummary[] }[] = [];
    const byProject = new Map<string, WorkflowSummary[]>();
    const noProject: WorkflowSummary[] = [];
    for (const wf of workflows) {
      if (wf.project_id) {
        const arr = byProject.get(wf.project_id) ?? [];
        arr.push(wf);
        byProject.set(wf.project_id, arr);
      } else {
        noProject.push(wf);
      }
    }
    // The "no project" bucket renders FIRST and shares the discussions
    // sidebar wording (`disc.general` → "Général" / "General"). Before
    // this it was last + labelled "Sans projet", which made the two
    // panels feel like different products to the user.
    if (noProject.length > 0) {
      groups.push({ key: '__global__', label: t('disc.general'), workflows: noProject });
    }
    for (const [pid, wfs] of byProject) {
      const label = wfs[0].project_name ?? pid;
      groups.push({ key: pid, label, workflows: wfs });
    }
    return groups;
  }, [workflows, t]);

  const toggleGroup = (key: string) => {
    setCollapsedGroups(prev => ({ ...prev, [key]: !prev[key] }));
  };

  const openDetail = async (id: string) => {
    setSelectedId(id);
    setLoadingDetail(true);
    try {
      const [wf, runs] = await Promise.all([
        workflowsApi.get(id),
        workflowsApi.listRuns(id),
      ]);
      setDetailWorkflow(wf);
      setDetailRuns(runs);
    } catch (e) {
      console.warn('Workflow action failed:', e);
    } finally {
      setLoadingDetail(false);
    }
  };

  // Cross-page navigation: when the sidebar's batch pastille is clicked, the
  // Dashboard passes the parent workflow id here. We auto-switch to the
  // "workflows" sub-tab, open its detail panel, then ack so the same click
  // doesn't re-fire on every render.
  useEffect(() => {
    if (!initialSelectedWorkflowId) return;
    setTab('workflows');
    openDetail(initialSelectedWorkflowId);
    onInitialSelectionConsumed?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialSelectedWorkflowId]);

  /** 0.6.0 UX pass — actually fire the trigger (with optional variables).
   *  Split out so handleTrigger can intercept and show the launch modal
   *  first when a workflow declares manual variables. */
  const fireTrigger = async (id: string, variables?: Record<string, string>) => {
    setTriggering(id);

    // Reset live run
    setLiveRun({
      workflowId: id,
      runId: null,
      currentStep: null,
      stepIndex: 0,
      totalSteps: 0,
      completedSteps: [],
      currentStepText: '',
      startedAt: Date.now(),
      currentStepStartedAt: null,
      finished: false,
      status: null,
    });

    const abort = new AbortController();
    abortRef.current = abort;

    // 0.7.0 — bound + coalesce live-progress chunks.
    //
    // Without this fix, the workflow page lagged hard whenever an Agent
    // step ran longer than ~5 min: tool-call streaming emits chunks at
    // 10-20/s, each one triggered a setLiveRun → React re-render of the
    // full <pre>currentStepText</pre>. The buffer also grew unbounded
    // (1MB+ on heavy implements). Two compounding problems:
    //   1. setState rate (every chunk = re-render storm)
    //   2. buffer size (re-rendering 1MB of text on every chunk)
    //
    // The fix:
    //   1. Coalesce chunks within a single animation frame (~60fps cap).
    //      All chunks that arrive between two RAFs land in one setState.
    //   2. FIFO-drop oldest content past `LIVE_BUFFER_MAX` chars. Live
    //      view shows the most recent activity — older chunks are
    //      already off-screen anyway.
    const LIVE_BUFFER_MAX = 50_000;
    let pendingChunks = '';
    let rafId: number | null = null;
    const flushChunks = () => {
      rafId = null;
      const chunks = pendingChunks;
      if (!chunks) return;
      pendingChunks = '';
      setLiveRun(prev => {
        if (!prev) return prev;
        return {
          ...prev,
          currentStepText: appendLiveBuffer(prev.currentStepText, chunks, LIVE_BUFFER_MAX),
        };
      });
    };

    await workflowsApi.triggerStream(
      id,
      (data) => {
        // New step kicks off — wipe the live text buffer so chunks from
        // the previous step don't bleed into the new one. Stamp the
        // step's start time so the per-step elapsed badge ticks from 0
        // (and not from the workflow start).
        pendingChunks = ''; // discard any in-flight chunks from the prior step
        setLiveRun(prev => prev ? {
          ...prev,
          currentStep: data.step_name,
          stepIndex: data.step_index,
          totalSteps: data.total_steps,
          currentStepText: '',
          currentStepStartedAt: Date.now(),
        } : prev);
      },
      (stepResult) => {
        pendingChunks = '';
        setLiveRun(prev => prev ? {
          ...prev,
          currentStep: null,
          completedSteps: [...prev.completedSteps, stepResult],
          currentStepText: '',
          currentStepStartedAt: null,
        } : prev);
      },
      (data) => {
        if (rafId !== null) { cancelAnimationFrame(rafId); rafId = null; }
        pendingChunks = '';
        setLiveRun(prev => prev ? { ...prev, finished: true, status: data.status, currentStep: null } : prev);
        setTriggering(null);
        refetch();
        if (selectedId === id) openDetail(id);
      },
      (error) => {
        if (rafId !== null) { cancelAnimationFrame(rafId); rafId = null; }
        console.warn('Workflow trigger error:', error);
        setLiveRun(prev => prev ? { ...prev, finished: true, status: 'Failed', currentStep: null } : prev);
        setTriggering(null);
      },
      abort.signal,
      variables,
      // Live progress — accumulate into a coalescing buffer; flush on
      // the next animation frame. The workflow live view reads
      // `currentStepText` to render the streaming agent output for the
      // in-flight step.
      (text) => {
        pendingChunks += text;
        if (rafId === null) {
          rafId = requestAnimationFrame(flushChunks);
        }
      },
      // run_start carries the backend-minted run_id — needed for the
      // Stop button to call cancelRun(workflowId, runId). Without it,
      // the live view can't address the run it's watching.
      (runId) => {
        setLiveRun(prev => prev ? { ...prev, runId } : prev);
      },
    );
  };

  /** 0.6.0 UX pass — entry point for "Lancer". Inspects the workflow's
   *  declared variables ; if non-empty, shows the launch form first.
   *  Otherwise delegates straight to fireTrigger (legacy behavior). */
  const handleTrigger = async (id: string) => {
    let wf: Workflow;
    try {
      wf = await workflowsApi.get(id);
    } catch (e) {
      console.warn('Failed to fetch workflow before trigger:', e);
      // Fallback: launch without variables — backend rejects loudly if required vars missing.
      await fireTrigger(id);
      return;
    }
    const vars = wf.variables ?? [];
    if (vars.length === 0) {
      await fireTrigger(id);
      return;
    }
    // Open the launch modal with empty values; user fills + submits.
    setLaunchingWorkflow({
      workflow: wf,
      values: Object.fromEntries(vars.map(v => [v.name, ''])),
      submitting: false,
      error: null,
    });
  };

  const handleToggle = async (wf: WorkflowSummary) => {
    try {
      await workflowsApi.update(wf.id, { enabled: !wf.enabled });
      refetch();
    } catch (e) {
      console.warn('Workflow action failed:', e);
    }
  };

  // Inline Stop on a workflow card. Silent refresh — the card's last_run
  // status will flip to Cancelled at the next refetch tick.
  const handleCancelRun = async (workflowId: string, runId: string) => {
    setCancellingRunIds(prev => {
      const next = new Set(prev);
      next.add(runId);
      return next;
    });
    try {
      await workflowsApi.cancelRun(workflowId, runId);
    } catch {
      // Refetch reconciles the UI; errors here mean the run was already
      // finished or the registry lost the token — nothing actionable.
    }
    refetch();
  };

  const handleDelete = async (id: string) => {
    try {
      await workflowsApi.delete(id);
      if (selectedId === id) {
        setSelectedId(null);
        setDetailWorkflow(null);
      }
      refetch();
    } catch (e) {
      console.warn('Workflow action failed:', e);
    }
  };

  const handleSaveQP = async (req: CreateQuickPromptRequest) => {
    if (editingQP) {
      await quickPromptsApi.update(editingQP.id, req);
    } else {
      await quickPromptsApi.create(req);
    }
    setShowCreateQP(false);
    setEditingQP(null);
    refetchQP();
  };

  const handleSaveQA = async (req: CreateQuickApiRequest) => {
    if (editingQA) {
      await quickApisApi.update(editingQA.id, req);
    } else {
      await quickApisApi.create(req);
    }
    setShowCreateQA(false);
    setEditingQA(null);
    refetchQA();
  };

  // Direct-run a saved Quick API. Variables are user-supplied via the
  // launch modal; the backend builds an ephemeral ApiCall step from the
  // QA config + variables and calls the same executor a workflow uses.
  // Result panel shows envelope.data on success, the error string on fail.
  const handleLaunchQA = async (qa: QuickApi) => {
    setLaunchingQARun(true);
    setLaunchQAResult(null);
    try {
      const res = await quickApisApi.runQa(qa.id, { variables: launchVarsQA });
      const env = res.envelope as { data?: unknown } | null;
      setLaunchQAResult({
        ok: res.success,
        payload: env?.data ?? res.envelope ?? null,
        error: res.error ?? null,
      });
    } catch (e) {
      setLaunchQAResult({ ok: false, payload: null, error: String(e) });
    } finally {
      setLaunchingQARun(false);
    }
  };

  const handleBatchLaunchQA = async (qa: QuickApi) => {
    const parsed = parseBatchQAItems({ varCount: qa.variables.length }, batchQAInput);
    if ('errorKey' in parsed) {
      const errorMsg = parsed.errorArg
        ? t(parsed.errorKey).replace('{0}', parsed.errorArg)
        : t(parsed.errorKey);
      setBatchQAResult({ status: 'ERROR', items: [{ input: '', status: 'ERROR', error: errorMsg }], total: 0, succeeded: 0, failed: 1 });
      return;
    }
    setBatchQARunning(true);
    setBatchQAResult(null);
    setExpandedQARows(new Set());
    try {
      const res = await quickApisApi.batchRunQa(qa.id, {
        items: parsed.items,
        concurrent_limit: batchQAConcurrentLimit,
      });
      const env = res.envelope as { data?: { items?: Array<{ input: unknown; status: string; response?: unknown; error?: string; http_status?: number }>; total?: number; succeeded?: number; failed?: number } } | null;
      const data = env?.data;
      setBatchQAResult({
        status: res.status,
        items: data?.items ?? [],
        total: data?.total ?? parsed.items.length,
        succeeded: data?.succeeded ?? 0,
        failed: data?.failed ?? 0,
      });
    } catch (e) {
      setBatchQAResult({ status: 'ERROR', items: [], total: parsed.items.length, succeeded: 0, failed: parsed.items.length });
      console.error('[QA batch] launch failed:', e);
    } finally {
      setBatchQARunning(false);
    }
  };

  // Lazy-load API plugins on first switch to the QuickApis tab. Same
  // pattern as the workflow wizard does on mount, but here only when
  // the user actually opens the tab — keeps the page light.
  useEffect(() => {
    if (tab !== 'quickApis' || availableApiPlugins.length > 0) return;
    mcpsApi.overview()
      .then(overview => {
        const options: ApiPluginOption[] = [];
        for (const config of overview.configs) {
          const server = overview.servers.find(s => s.id === config.server_id);
          if (!server || !server.api_spec) continue;
          options.push({ server, config });
        }
        setAvailableApiPlugins(options);
      })
      .catch(e => console.warn('Failed to load API plugins:', e));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab]);

  const renderTemplate = (template: string, vars: Record<string, string>): string => {
    let rendered = template;
    // 1. Process conditional sections: {{#var}}content{{/var}} — removed if var is empty
    rendered = rendered.replace(/\{\{#(\w+)\}\}([\s\S]*?)\{\{\/\1\}\}/g, (_, name, content) => {
      return vars[name]?.trim() ? content : '';
    });
    // 2. Replace remaining {{var}} placeholders
    rendered = rendered.replace(/\{\{(\w+)\}\}/g, (_, name) => vars[name] ?? '');
    // 3. Clean up double spaces/commas from removed sections
    rendered = rendered.replace(/  +/g, ' ').replace(/, ,/g, ',').trim();
    return rendered;
  };

  const handleLaunchQP = async (qp: QuickPrompt) => {
    setLaunching(true);
    try {
      const rendered = renderTemplate(qp.prompt_template, launchVars);
      // Build dynamic title with first non-empty variable value
      const firstVal = qp.variables.map(v => launchVars[v.name]).find(v => v?.trim());
      const title = firstVal ? `${qp.name} — ${firstVal}` : qp.name;
      const disc = await discussionsApi.create({
        project_id: qp.project_id ?? null,
        title,
        agent: qp.agent,
        language: configLanguage || 'fr',
        initial_prompt: rendered,
        skill_ids: qp.skill_ids?.length ? qp.skill_ids : undefined,
        tier: qp.tier !== 'default' ? qp.tier : undefined,
      });
      setLaunchingQP(null);
      setLaunchVars({});
      onNavigateDiscussion?.(disc.id);
    } catch (e) {
      console.warn('Launch failed:', e);
    } finally {
      setLaunching(false);
    }
  };

  /**
   * Fan-out launch: parse the user's list (one value per line), render the
   * QP template for each, call POST /quick-prompts/:id/batch which creates
   * N child discussions linked to a batch WorkflowRun, then kick off the
   * agent runs in parallel (backend semaphore limits concurrency to 5).
   */
  const handleBatchLaunch = async (qp: QuickPrompt) => {
    // Accept newline, comma AND semicolon as separators — users often paste
    // a comma-separated list from Jira/GitHub search results on a single line
    // (real-world bug report from the first batch run that shipped a single
    // disc with title "EW-7223,EW-7182,EW-6071,EW-7141").
    // Also deduplicate so accidental repeats don't create duplicate discussions.
    const rawItems = batchInputLines
      .split(/[\n,;]/)
      .map(s => s.trim())
      .filter(Boolean);
    const seen = new Set<string>();
    const lines: string[] = [];
    for (const item of rawItems) {
      if (!seen.has(item)) {
        seen.add(item);
        lines.push(item);
      }
    }
    const notify = toastProp ?? ((msg: string) => alert(msg));
    if (lines.length === 0) {
      notify(t('qp.batch.emptyInput'), 'error');
      return;
    }
    if (lines.length > 50) {
      notify(t('qp.batch.tooManyItems', lines.length), 'error');
      return;
    }
    if (qp.variables.length === 0) {
      notify(t('qp.batch.needsVariable'), 'error');
      return;
    }
    // Use the FIRST variable as the batch key. Each line becomes the value
    // of that variable for one child discussion. For now we only support
    // 1 variable per batch — multi-var comes in Phase 2 with auto-fetch.
    const keyVar = qp.variables[0];
    // Confirm before firing — N agents × ~X tokens is not cheap.
    const estimateMsg = t('qp.batch.confirm', lines.length, qp.name);
    if (!confirm(estimateMsg)) return;

    setBatchLaunching(true);
    try {
      const items = lines.map(line => {
        const vars: Record<string, string> = { [keyVar.name]: line };
        const prompt = renderTemplate(qp.prompt_template, vars);
        return { title: `${qp.name} — ${line}`, prompt };
      });
      const now = new Date();
      const batchName = `${qp.name} — ${now.toLocaleString(configLanguage || 'fr', {
        day: '2-digit', month: 'short', hour: '2-digit', minute: '2-digit',
      })}`;
      const res = await quickPromptsApi.batchRun(qp.id, {
        items,
        batch_name: batchName,
        project_id: qp.project_id ?? null,
        workspace_mode: batchIsolated ? 'Isolated' : 'Direct',
      });
      // Kick off each child discussion's agent run in parallel.
      //
      // Two gotchas:
      // 1. The backend spawns the agent in a detached tokio task that
      //    outlives the HTTP request, so we don't need the SSE stream
      //    at all. We abort the fetch right after dispatch to release
      //    the browser's per-host HTTP/1.1 connection slot (~6 max).
      //    Without this, launching >5 discs saturates the connection
      //    pool and later POSTs queue on the client side, making the
      //    batch look sequential.
      // 2. We mark every disc as "sending" in the shared sendingMap so
      //    the sidebar spinner lights up for all of them — otherwise
      //    only the navigated disc looks like it's running.
      res.discussion_ids.forEach(discId => {
        const controller = new AbortController();
        fetch(`/api/discussions/${discId}/run`, {
          method: 'POST',
          signal: controller.signal,
          // keepalive lets the browser finish dispatching even if we
          // abort the read side almost immediately.
          keepalive: true,
        }).catch(() => {
          // Aborted or network blip — backend run is independent, so
          // we just log and move on.
        });
        // Release the connection slot after the request is definitely
        // on the wire. 500ms is enough for the browser to flush the
        // headers + body; the backend `tokio::spawn` inside
        // make_agent_stream has already captured the work.
        setTimeout(() => controller.abort(), 500);
      });
      // Tell the parent (Dashboard) to mark all these disc ids as
      // sending AND to open the first one — this is what lights up
      // the spinner in the sidebar and routes the user to the live
      // batch view. We deliberately do NOT call onNavigateDiscussion
      // here: that path triggers an auto-run on the navigated disc,
      // which would create a second POST /run on top of the one we
      // already fired in the fan-out loop, doubling its response.
      // `onBatchLaunched` in Dashboard uses `setOpenDiscussionId`
      // which opens without auto-running.
      onBatchLaunched?.(res.discussion_ids, res.run_id);
      setBatchingQP(null);
      setBatchInputLines('');
    } catch (e) {
      console.warn('Batch launch failed:', e);
      notify(t('qp.batch.failed', userError(e)), 'error');
    } finally {
      setBatchLaunching(false);
    }
  };

  return (
    <div>
      <div className="flex-between mb-4">
        <div>
          <h1 className="wf-h1"><MatrixText text={t('wf.title')} /></h1>
        </div>
        {tab === 'workflows' ? (
        <div className="flex-row gap-3">
          {onNavigateDiscussion && (
            <button className="wf-create-ai-btn" title={t('wf.createWithAIHint')} onClick={async () => {
              try {
                // Inject project list as compact lookup table (max 20, saves tokens)
                const shown = projects.slice(0, 20);
                const projectContext = shown.length > 0
                  ? '\n\n---\nProjets[name=id]: ' + shown.map(p => `${p.name}=${p.id}`).join(' | ')
                    + (projects.length > 20 ? ` (+${projects.length - 20})` : '')
                  : '';
                const disc = await discussionsApi.create({
                  project_id: null,
                  title: 'Workflow Architect',
                  agent: 'ClaudeCode',
                  language: configLanguage || 'fr',
                  initial_prompt: t('wf.aiArchitectPrompt') + projectContext,
                  skill_ids: ['workflow-architect'],
                  profile_ids: [],
                  directive_ids: [],
                  tier: 'reasoning',
                });
                onNavigateDiscussion(disc.id);
              } catch (e) {
                console.warn('Failed to create AI discussion:', e);
              }
            }}>
              <Zap size={14} /> {t('wf.createWithAI')}
            </button>
          )}
          {/* 0.7.0 UX pass — Import workflow JSON. Drag-and-drop or
              file picker. Sister button to "Nouveau workflow". */}
          <button
            className="wf-create-btn wf-create-btn-secondary"
            title={t('wf.importHint')}
            onClick={() => setImporting({
              kind: 'workflow',
              content: '',
              preview: { name: '' },
              targetProjectId: '',
            })}
          >
            <Upload size={14} /> {t('wf.import')}
          </button>
          <button className="wf-create-btn" title={t('wf.newHint')} onClick={() => setShowCreate(true)}>
            <Plus size={14} /> {t('wf.new')}
          </button>
        </div>
        ) : tab === 'quickPrompts' ? (
        <div className="flex-row gap-3">
          <button
            className="wf-create-btn wf-create-btn-secondary"
            title={t('qp.importHint')}
            onClick={() => setImporting({
              kind: 'qp',
              content: '',
              preview: { name: '' },
              targetProjectId: '',
            })}
          >
            <Upload size={14} /> {t('qp.import')}
          </button>
          <button className="wf-create-btn" onClick={() => { setShowCreateQP(true); setEditingQP(null); }}>
            <Plus size={14} /> {t('qp.new')}
          </button>
        </div>
        ) : (
        // quickApis — header aligné sur les deux autres onglets. Pas
        // d'import pour l'instant (pas de drawer QA). Le bouton est
        // masqué tant qu'aucun plugin API n'est wired ; la warning
        // détaillée + CTA reste affichée dans le contenu de l'onglet.
        <div className="flex-row gap-3">
          {availableApiPlugins.length > 0 && (
            <button className="wf-create-btn" onClick={() => setShowCreateQA(true)}>
              <Plus size={14} /> {t('qa.new')}
            </button>
          )}
        </div>
        )}
      </div>

      {/* Tab bar */}
      <div className="dash-tab-bar mb-6">
        <button className="dash-tab" data-active={tab === 'workflows'} onClick={() => setTab('workflows')}>
          {t('wf.tabWorkflows')} {workflowList ? `(${workflowList.length})` : ''}
        </button>
        <button className="dash-tab" data-active={tab === 'quickPrompts'} onClick={() => setTab('quickPrompts')}>
          {t('wf.tabQuickPrompts')} {quickPromptList ? `(${quickPromptList.length})` : ''}
        </button>
        <button className="dash-tab" data-active={tab === 'quickApis'} onClick={() => setTab('quickApis')}>
          {t('wf.tabQuickApis')} {quickApiList ? `(${quickApiList.length})` : ''}
        </button>
      </div>

      {/* ═══ WORKFLOWS TAB ═══ */}
      {tab === 'workflows' && (<>

      {/* Create wizard */}
      {showCreate && (
        <WorkflowWizard
          projects={projects}
          installedAgentTypes={installedAgentTypes}
          agentAccess={agentAccess}
          configLanguage={configLanguage}
          onDone={() => { setShowCreate(false); refetch(); }}
          onCancel={() => setShowCreate(false)}
        />
      )}

      {/* Edit wizard */}
      {editingWorkflow && (
        <WorkflowWizard
          key={editingWorkflow.id}
          projects={projects}
          installedAgentTypes={installedAgentTypes}
          agentAccess={agentAccess}
          configLanguage={configLanguage}
          editWorkflow={editingWorkflow}
          onDone={() => { setEditingWorkflow(null); refetch(); if (editingWorkflow) openDetail(editingWorkflow.id); }}
          onCancel={() => setEditingWorkflow(null)}
        />
      )}

      {/* Workflow list */}
      {!showCreate && !editingWorkflow && workflows.length === 0 && (
        <div className="wf-empty">
          <Zap size={32} className="text-ghost mb-4" />
          <p>{t('wf.empty')}</p>
          <p className="text-sm text-muted mt-2">
            {t('wf.emptyHint')}
          </p>
        </div>
      )}

      {!showCreate && !editingWorkflow && workflows.length > 0 && (
        <div className="flex-col gap-8" style={isMobile ? undefined : { flexDirection: 'row' }}>
          {/* List — grouped by project */}
          {!(isMobile && selectedId) && (
          <div style={{ flex: isMobile ? '1 1 auto' : '0 0 380px' }}>
            {groupedWorkflows.map(group => (
              <div key={group.key} className="mb-6">
                {/* Group header */}
                <button
                  className="wf-group-header"
                  onClick={() => toggleGroup(group.key)}
                  aria-expanded={!collapsedGroups[group.key]}
                >
                  {collapsedGroups[group.key] ? <ChevronRight size={12} /> : <ChevronDown size={12} />}
                  <span className="flex-1">{group.label}</span>
                  <span className="text-xs text-secondary">{group.workflows.length}</span>
                </button>

                {/* Workflow cards */}
                {!collapsedGroups[group.key] && group.workflows.map(wf => {
                  // A workflow is "in flight" if EITHER an SSE-tracked
                  // liveRun is attached to it, OR its persisted last_run
                  // is still Running/Pending. We compute it once and use
                  // it to drive the spinner next to the name + the Stop
                  // affordance below. Showing only the Stop button (UX
                  // feedback 2026-04-29) buried the running state — the
                  // user wanted a spinner to spot active runs at a
                  // glance in long workflow lists.
                  const live = liveRun && liveRun.workflowId === wf.id && !liveRun.finished ? liveRun : null;
                  const lastStatus = wf.last_run?.status;
                  const isRunning = !!live || lastStatus === 'Running' || lastStatus === 'Pending';
                  return (
                  <div
                    key={wf.id}
                    className="wf-card"
                    data-active={selectedId === wf.id}
                    data-running={isRunning}
                    onClick={() => openDetail(wf.id)}
                  >
                    <div className="flex-row gap-4 mb-3">
                      {isRunning && (
                        <Loader2
                          size={14}
                          className="wf-spin"
                          style={{ color: 'var(--kr-cyan)', flexShrink: 0 }}
                          aria-label={t('wf.running')}
                        />
                      )}
                      <span className="font-semibold text-md flex-1">{wf.name}</span>
                      <button
                        className="wf-icon-btn"
                        style={{ color: wf.enabled ? 'var(--kr-success)' : 'var(--kr-text-dim)' }}
                        onClick={(e) => { e.stopPropagation(); handleToggle(wf); }}
                        title={wf.enabled ? t('wf.active') : t('wf.inactive')}
                        aria-pressed={wf.enabled}
                        aria-label={wf.enabled ? t('wf.active') : t('wf.inactive')}
                      >
                        {wf.enabled ? <ToggleRight size={16} /> : <ToggleLeft size={16} />}
                      </button>
                    </div>

                    <div className="flex-row gap-4 text-sm text-muted">
                      <span className="wf-trigger-badge" data-type={wf.trigger_type}>
                        {wf.trigger_type === 'cron' && <Clock size={10} />}
                        {wf.trigger_type === 'tracker' && <GitBranch size={10} />}
                        {wf.trigger_type === 'manual' && <Zap size={10} />}
                        {TRIGGER_LABELS[wf.trigger_type] ?? wf.trigger_type}
                      </span>
                      <span>{wf.step_count} step{wf.step_count > 1 ? 's' : ''}</span>
                    </div>

                    {wf.last_run && (
                      <div className="flex-row gap-3 mt-3 text-xs">
                        <span style={{ color: STATUS_COLORS[wf.last_run.status] ?? 'var(--kr-text-faint)' }}>
                          {wf.last_run.status}
                        </span>
                        <span className="text-ghost">
                          {new Date(wf.last_run.started_at).toLocaleString()}
                        </span>
                        {wf.last_run.tokens_used > 0 && (
                          <span className="text-ghost">
                            · {wf.last_run.tokens_used} tokens
                          </span>
                        )}
                        {/* Stop button — visible whenever the row's workflow has
                            an in-flight run, sourced from EITHER the persisted
                            `last_run` (post-refetch) OR the in-memory `liveRun`
                            (during SSE between trigger and first refetch).
                            Without the liveRun branch, the user sees a Running
                            spinner with no Stop affordance for the first ~5-10s
                            after they click "Lancer". */}
                        {(() => {
                          const live = liveRun && liveRun.workflowId === wf.id && !liveRun.finished ? liveRun : null;
                          const runStatus = wf.last_run.status;
                          const isLive = !!live;
                          const isStored = runStatus === 'Running' || runStatus === 'Pending';
                          if (!isLive && !isStored) return null;
                          const runIdToCancel = live?.runId ?? wf.last_run!.id;
                          const cancelling = cancellingRunIds.has(runIdToCancel);
                          return (
                            <button
                              className="wf-card-stop-btn"
                              onClick={(e) => {
                                e.stopPropagation();
                                if (!runIdToCancel) return;
                                void handleCancelRun(wf.id, runIdToCancel);
                              }}
                              disabled={!runIdToCancel || cancelling}
                              title={t('wf.cancelRun')}
                              aria-label={t('wf.cancelRun')}
                            >
                              <Square size={9} style={{ fill: 'currentColor' }} />
                              {cancelling ? t('wf.cancelling') : t('wf.cancelRun')}
                            </button>
                          );
                        })()}
                      </div>
                    )}

                    <div className="flex-row gap-2 mt-4">
                      <button
                        className="wf-small-btn"
                        onClick={(e) => { e.stopPropagation(); handleTrigger(wf.id); }}
                        disabled={!wf.enabled || triggering === wf.id}
                      >
                        {triggering === wf.id ? <Loader2 size={10} className="spin" /> : <Play size={10} />}
                        {t('wf.trigger')}
                      </button>
                      <button
                        className="wf-small-btn wf-small-btn-danger"
                        onClick={(e) => { e.stopPropagation(); handleDelete(wf.id); }}
                      >
                        <Trash2 size={10} /> {t('wf.delete')}
                      </button>
                    </div>
                  </div>
                  );
                })}
              </div>
            ))}
          </div>
          )}

          {/* Detail panel */}
          <div className="flex-1 min-w-0">
            {isMobile && selectedId && (
              <button
                className="wf-back-btn"
                onClick={() => setSelectedId(null)}
              >
                <ChevronLeft size={14} /> {t('wf.back') ?? 'Back'}
              </button>
            )}
            {selectedId && loadingDetail && (
              <div className="wf-empty" style={{ padding: 40 }}>
                <Loader2 size={24} className="text-accent wf-spin" />
              </div>
            )}

            {selectedId && !loadingDetail && detailWorkflow && (
              <WorkflowDetail
                workflow={detailWorkflow}
                runs={detailRuns}
                liveRun={liveRun?.workflowId === detailWorkflow.id ? liveRun : null}
                onTrigger={() => handleTrigger(detailWorkflow.id)}
                onRefresh={() => openDetail(detailWorkflow.id)}
                onEdit={() => setEditingWorkflow(detailWorkflow)}
                onDeleteRun={async (runId) => {
                  await workflowsApi.deleteRun(detailWorkflow.id, runId);
                  openDetail(detailWorkflow.id);
                  refetch();
                }}
                onDeleteAllRuns={async () => {
                  await workflowsApi.deleteAllRuns(detailWorkflow.id);
                  openDetail(detailWorkflow.id);
                  refetch();
                }}
                triggering={triggering === detailWorkflow.id}
                agentAccess={agentAccess}
                onNavigateToBatch={onNavigateToBatch}
                onGateDecided={() => setLiveRun(null)}
                onExport={async () => {
                  try {
                    const { filename, blob } = await workflowsApi.exportWorkflow(detailWorkflow.id);
                    triggerDownload(filename, blob);
                    if (toastProp) toastProp(t('wf.exportDone').replace('{name}', detailWorkflow.name), 'success');
                  } catch (e) {
                    if (toastProp) toastProp(userError(e), 'error');
                  }
                }}
              />
            )}

            {!selectedId && (
              <div className="wf-empty">
                <Eye size={24} className="text-ghost mb-4" />
                <p>{t('wf.selectOne')}</p>
              </div>
            )}
          </div>
        </div>
      )}
      </>)}

      {/* ═══ QUICK PROMPTS TAB ═══ */}
      {tab === 'quickPrompts' && (
        <div>
          {/* Create/Edit form */}
          {(showCreateQP || editingQP) && (
            <QuickPromptForm
              editPrompt={editingQP ?? undefined}
              projects={projects}
              onSave={handleSaveQP}
              onCancel={() => { setShowCreateQP(false); setEditingQP(null); }}
            />
          )}

          {/* Quick prompt list */}
          {!showCreateQP && !editingQP && (
            <>
              {(!quickPromptList || quickPromptList.length === 0) ? (
                <div className="wf-empty">
                  <p className="wf-empty-title">{t('qp.empty')}</p>
                  <p className="wf-empty-hint">{t('qp.emptyHint')}</p>
                </div>
              ) : (
                <div className="qp-list">
                  {quickPromptList.map(qp => (
                    <div key={qp.id} className="qp-card">
                      <div className="qp-card-header">
                        <span className="qp-card-icon">{qp.icon}</span>
                        <span className="qp-card-name">{qp.name}</span>
                        {qp.variables.length > 0 && (
                          <span className="qp-card-vars">{t('qp.vars', qp.variables.length)}</span>
                        )}
                        <div className="qp-card-actions">
                          <button className="wf-icon-btn" onClick={() => setEditingQP(qp)} title="Edit">
                            <Eye size={12} />
                          </button>
                          {/* 0.7.0 UX pass — export QP as JSON file. */}
                          <button className="wf-icon-btn" onClick={async () => {
                            try {
                              const { filename, blob } = await quickPromptsApi.exportQp(qp.id);
                              triggerDownload(filename, blob);
                              if (toastProp) toastProp(t('qp.exportDone').replace('{name}', qp.name), 'success');
                            } catch (e) {
                              if (toastProp) toastProp(userError(e), 'error');
                            }
                          }} title={t('qp.export')}>
                            <Download size={12} />
                          </button>
                          <button className="wf-icon-btn" onClick={async () => {
                            if (!confirm(t('qp.deleteConfirm'))) return;
                            await quickPromptsApi.delete(qp.id);
                            refetchQP();
                          }} title={t('qp.delete')}>
                            <Trash2 size={12} />
                          </button>
                          {/* Batch button — fans out N discussions from a list
                              of values for the first QP variable. Only meaningful
                              for QPs with at least one variable. */}
                          {qp.variables.length > 0 && (
                            <button
                              className="wf-icon-btn"
                              onClick={() => {
                                setBatchingQP(batchingQP?.id === qp.id ? null : qp);
                                setBatchInputLines('');
                              }}
                              title={t('qp.batch.launch')}
                            >
                              <Layers size={12} />
                            </button>
                          )}
                          <button
                            className="qp-launch-btn"
                            onClick={() => {
                              if (qp.variables.length === 0) {
                                // No variables — launch directly
                                setLaunchingQP(qp);
                                setLaunchVars({});
                                // Immediate launch
                                handleLaunchQP(qp);
                              } else {
                                setLaunchingQP(launchingQP?.id === qp.id ? null : qp);
                                setLaunchVars({});
                              }
                            }}
                          >
                            <Play size={12} /> {t('qp.launch')}
                          </button>
                        </div>
                      </div>

                      {/* Batch launch form — list of values, one per line,
                          each becomes the first variable for one child disc. */}
                      {batchingQP?.id === qp.id && (
                        <div className="qp-launch-form">
                          {qp.description && (
                            <p className="qp-batch-hint">{qp.description}</p>
                          )}
                          <label className="qp-launch-label">
                            {t('qp.batch.inputLabel', qp.variables[0]?.label || qp.variables[0]?.name || 'value')}
                            {qp.variables[0]?.description && (
                              <span className="qp-batch-var-desc"> — {qp.variables[0].description}</span>
                            )}
                          </label>
                          <textarea
                            className="wf-textarea"
                            rows={6}
                            value={batchInputLines}
                            onChange={e => setBatchInputLines(e.target.value)}
                            placeholder={t('qp.batch.inputPlaceholder')}
                            autoFocus
                          />
                          <p className="qp-syntax-hint">
                            {/* Same multi-separator split as handleBatchLaunch
                                so the live count matches what will actually run */}
                            {t(
                              'qp.batch.countHint',
                              new Set(
                                batchInputLines.split(/[\n,;]/).map(s => s.trim()).filter(Boolean)
                              ).size
                            )}
                          </p>
                          {/* Isolated-worktree toggle — only meaningful if the QP is project-linked */}
                          <label
                            className="flex-row gap-2 text-xs mt-2 mb-2"
                            style={{ alignItems: 'center', opacity: qp.project_id ? 1 : 0.5 }}
                            title={qp.project_id ? t('qp.batch.worktreeHint') : t('qp.batch.worktreeNoProject')}
                          >
                            <input
                              type="checkbox"
                              checked={batchIsolated}
                              disabled={!qp.project_id}
                              onChange={e => setBatchIsolated(e.target.checked)}
                            />
                            {t('qp.batch.worktree')}
                          </label>
                          <div className="flex-row gap-4">
                            <button
                              className="qp-launch-go-btn"
                              onClick={() => handleBatchLaunch(qp)}
                              disabled={batchLaunching || !batchInputLines.trim()}
                            >
                              {batchLaunching ? <Loader2 size={14} className="spin" /> : <Layers size={14} />}
                              {batchLaunching ? '...' : t('qp.batch.launch')}
                            </button>
                            <button
                              className="wf-icon-btn"
                              onClick={() => { setBatchingQP(null); setBatchInputLines(''); }}
                              title={t('qp.batch.cancel')}
                            >
                              <X size={14} />
                            </button>
                          </div>
                        </div>
                      )}

                      {/* Launch form with variable inputs */}
                      {launchingQP?.id === qp.id && qp.variables.length > 0 && (
                        <div className="qp-launch-form">
                          {qp.variables.map(v => (
                            <div key={v.name} className="qp-launch-field">
                              <label className="qp-launch-label">{v.label || v.name}</label>
                              <input
                                className="wf-input flex-1"
                                value={launchVars[v.name] ?? ''}
                                onChange={e => setLaunchVars(prev => ({ ...prev, [v.name]: e.target.value }))}
                                placeholder={v.placeholder}
                                autoFocus={qp.variables.indexOf(v) === 0}
                                onKeyDown={e => { if (e.key === 'Enter') handleLaunchQP(qp); }}
                              />
                            </div>
                          ))}
                          <button
                            className="qp-launch-go-btn"
                            onClick={() => handleLaunchQP(qp)}
                            disabled={launching}
                          >
                            {launching ? <Loader2 size={14} className="spin" /> : <Play size={14} />}
                            {launching ? '...' : t('qp.launch')}
                          </button>
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
      )}

      {/* ═══ QUICK APIS TAB ═══ */}
      {/* Mirror of the QuickPrompts tab but for HTTP call templates.
          The form embeds ApiCallStepCard which carries the AI helper —
          identical UX to editing an ApiCall step in a workflow. The
          v3 deferred bits (run drawer with variables, batch launch from
          page) live as TODO; for v2 the user prototypes via the form's
          built-in "Test the call" button. */}
      {tab === 'quickApis' && (
        <div>
          {(showCreateQA || editingQA) && (
            <QuickApiForm
              editApi={editingQA ?? undefined}
              projects={projects}
              availableApiPlugins={availableApiPlugins}
              installedAgents={installedAgentTypes ?? []}
              configLanguage={configLanguage}
              onSave={handleSaveQA}
              onCancel={() => { setShowCreateQA(false); setEditingQA(null); }}
            />
          )}

          {!showCreateQA && !editingQA && (
            <>
              {/* Le bouton "Nouveau Quick API" vit dans le header (aligné
                  sur Workflows + Quick Prompts). On garde ici uniquement
                  la warning prerequisite quand aucun plugin API n'est
                  câblé — le bouton du header est masqué dans ce cas. */}
              {availableApiPlugins.length === 0 && (
                <div className="wf-restricted-warning mb-4">
                  <AlertTriangle size={12} />
                  <span className="flex-1">{t('qa.noPluginsWarning')}</span>
                  <button
                    type="button"
                    className="wf-allowlist-cta"
                    onClick={() => { window.location.hash = '#mcps'; }}
                  >
                    {t('qa.noPluginsCta')}
                  </button>
                </div>
              )}

              {(!quickApiList || quickApiList.length === 0) ? (
                <div className="wf-empty">
                  <p className="wf-empty-title">{t('qa.empty')}</p>
                  <p className="wf-empty-hint">{t('qa.emptyHint')}</p>
                </div>
              ) : (
                <div className="qp-list">
                  {quickApiList.map(qa => (
                    <div key={qa.id} className="qp-card">
                      <div className="qp-card-header">
                        <span className="qp-card-icon">{qa.icon}</span>
                        <span className="qp-card-name">{qa.name}</span>
                        {qa.variables.length > 0 && (
                          <span className="qp-card-vars">{t('qa.vars', qa.variables.length)}</span>
                        )}
                        <span className="text-2xs text-ghost" style={{ marginLeft: 8 }}>
                          {qa.api_method ?? 'GET'} {qa.api_endpoint_path}
                        </span>
                        <div className="qp-card-actions">
                          <button className="wf-icon-btn" onClick={() => setEditingQA(qa)} title={t('qa.edit')}>
                            <Eye size={12} />
                          </button>
                          <button className="wf-icon-btn" onClick={async () => {
                            try {
                              const { filename, blob } = await quickApisApi.exportQa(qa.id);
                              triggerDownload(filename, blob);
                              if (toastProp) toastProp(t('qa.exportDone').replace('{name}', qa.name), 'success');
                            } catch (e) {
                              if (toastProp) toastProp(userError(e), 'error');
                            }
                          }} title={t('qa.export')}>
                            <Download size={12} />
                          </button>
                          <button className="wf-icon-btn" onClick={async () => {
                            if (!confirm(t('qa.deleteConfirm').replace('{name}', qa.name))) return;
                            await quickApisApi.delete(qa.id);
                            refetchQA();
                          }} title={t('qa.delete')}>
                            <Trash2 size={12} />
                          </button>
                          {/* Batch button — fans out N HTTP calls from a list
                              of values for the QA's variables. Mirror of the
                              QP Batch button. Only meaningful when the QA
                              declares at least one variable. */}
                          {qa.variables.length > 0 && (
                            <button
                              className="wf-icon-btn"
                              onClick={() => {
                                setBatchingQA(batchingQA?.id === qa.id ? null : qa);
                                setBatchQAInput('');
                                setBatchQAResult(null);
                              }}
                              title={t('qa.batch.launch')}
                            >
                              <Layers size={12} />
                            </button>
                          )}
                          {/* Launch button — same shape as QP for visual
                              parity. With variables, expands an inline
                              form below; without, fires immediately. */}
                          <button
                            className="qp-launch-btn"
                            onClick={() => {
                              if (qa.variables.length === 0) {
                                setLaunchingQA(qa);
                                setLaunchVarsQA({});
                                setLaunchQAResult(null);
                                handleLaunchQA(qa);
                              } else {
                                const same = launchingQA?.id === qa.id;
                                setLaunchingQA(same ? null : qa);
                                setLaunchVarsQA({});
                                setLaunchQAResult(null);
                              }
                            }}
                          >
                            <Play size={12} /> {t('qa.launch')}
                          </button>
                        </div>
                      </div>
                      {qa.description && (
                        <p className="qp-card-desc">{qa.description}</p>
                      )}
                      {/* Batch form — paste list of values, fan-out parallel HTTP.
                          For 1-variable QAs: one value per line.
                          For N-variable QAs: JSON array of objects.
                          Result table appears below on submit. */}
                      {batchingQA?.id === qa.id && (
                        <div className="qp-launch-form">
                          {qa.variables.length > 1 ? (
                            <label className="qp-launch-label">
                              {t('qa.batch.jsonInputLabel')}
                              <p className="qp-batch-var-desc text-2xs text-ghost mt-1">
                                {t('qa.batch.jsonInputHint', qa.variables.map(v => v.name).join(', '))}
                              </p>
                            </label>
                          ) : (
                            <label className="qp-launch-label">
                              {t('qa.batch.inputLabel', qa.variables[0]?.label || qa.variables[0]?.name || 'value')}
                              {qa.variables[0]?.description && (
                                <span className="qp-batch-var-desc"> — {qa.variables[0].description}</span>
                              )}
                            </label>
                          )}
                          <textarea
                            className="wf-textarea"
                            rows={qa.variables.length > 1 ? 8 : 6}
                            value={batchQAInput}
                            onChange={e => setBatchQAInput(e.target.value)}
                            placeholder={qa.variables.length > 1
                              ? t('qa.batch.jsonInputPlaceholder')
                              : t('qa.batch.inputPlaceholder')}
                            autoFocus
                            spellCheck={false}
                            style={qa.variables.length > 1 ? { fontFamily: 'var(--kr-font-mono)', fontSize: 12 } : undefined}
                          />
                          <p className="qp-syntax-hint">
                            {qa.variables.length > 1
                              ? t('qa.batch.jsonCountHint',
                                  (() => {
                                    try {
                                      const arr = JSON.parse(batchQAInput);
                                      return Array.isArray(arr) ? arr.length : 0;
                                    } catch { return 0; }
                                  })()
                                )
                              : t('qa.batch.countHint',
                                  new Set(batchQAInput.split(/[\n,;]/).map(s => s.trim()).filter(Boolean)).size
                                )
                            }
                          </p>
                          <div className="flex-row gap-4 mb-2" style={{ alignItems: 'center' }}>
                            <label className="text-xs text-muted">{t('wiz.batchApiConcurrentLimit')}</label>
                            <input
                              type="number" min={1} max={20}
                              className="wf-input text-sm"
                              style={{ width: 70 }}
                              value={batchQAConcurrentLimit}
                              onChange={e => setBatchQAConcurrentLimit(Math.max(1, Math.min(20, parseInt(e.target.value || '5', 10))))}
                            />
                            <span className="text-2xs text-ghost">{t('wiz.batchApiConcurrentLimitHint')}</span>
                          </div>
                          <div className="flex-row gap-4">
                            <button
                              className="qp-launch-go-btn"
                              onClick={() => handleBatchLaunchQA(qa)}
                              disabled={batchQARunning || !batchQAInput.trim()}
                            >
                              {batchQARunning ? <Loader2 size={14} className="spin" /> : <Layers size={14} />}
                              {batchQARunning ? '...' : t('qa.batch.go')}
                            </button>
                            <button
                              className="wf-icon-btn"
                              onClick={() => { setBatchingQA(null); setBatchQAInput(''); setBatchQAResult(null); }}
                              title={t('qa.batch.cancel')}
                            >
                              <X size={14} />
                            </button>
                          </div>
                          {/* Per-item result table. Compact — shows input,
                              status pill, and either a 1-line response excerpt
                              or the error string. */}
                          {batchQAResult && (
                            <div style={{ marginTop: 12 }}>
                              <p className="text-xs text-muted mb-2">
                                {t('qa.batch.resultSummary',
                                  batchQAResult.succeeded,
                                  batchQAResult.total,
                                  batchQAResult.failed
                                )}
                                {' '}
                                <span style={{
                                  fontWeight: 600,
                                  color: batchQAResult.status === 'OK' ? 'var(--kr-success)'
                                    : batchQAResult.status === 'PARTIAL' ? 'var(--kr-warning)'
                                    : 'var(--kr-error)',
                                }}>
                                  {batchQAResult.status}
                                </span>
                              </p>
                              <div style={{ maxHeight: 360, overflow: 'auto', border: '1px solid var(--kr-border-faint)', borderRadius: 6 }}>
                                <table style={{ width: '100%', fontSize: 11, borderCollapse: 'collapse' }}>
                                  <thead>
                                    <tr style={{ background: 'rgba(var(--kr-text-faint-rgb, 128, 128, 128), 0.05)', textAlign: 'left' }}>
                                      <th style={{ padding: '4px 8px', fontWeight: 600 }}>#</th>
                                      <th style={{ padding: '4px 8px', fontWeight: 600 }}>{t('qa.batch.colInput')}</th>
                                      <th style={{ padding: '4px 8px', fontWeight: 600 }}>{t('qa.batch.colStatus')}</th>
                                      <th style={{ padding: '4px 8px', fontWeight: 600 }}>{t('qa.batch.colResult')}</th>
                                    </tr>
                                  </thead>
                                  <tbody>
                                    {batchQAResult.items.map((item, idx) => {
                                      // Texte complet de la cellule Result. JSON pretty-printé à 2 espaces
                                      // pour la lecture en mode déplié — évite un mur de braces collées.
                                      const fullResult = item.status === 'OK'
                                        ? (typeof item.response === 'string'
                                            ? item.response
                                            : JSON.stringify(item.response, null, 2))
                                        : (item.error ?? '');
                                      const truncated = fullResult.length > 80;
                                      const isExpanded = expandedQARows.has(idx);
                                      const preview = truncated && !isExpanded
                                        ? fullResult.slice(0, 80) + '…'
                                        : fullResult;
                                      const toggle = () => {
                                        if (!truncated) return;
                                        setExpandedQARows(prev => {
                                          const next = new Set(prev);
                                          if (next.has(idx)) next.delete(idx); else next.add(idx);
                                          return next;
                                        });
                                      };
                                      return (
                                        <tr
                                          key={idx}
                                          onClick={toggle}
                                          style={{
                                            borderTop: '1px solid var(--kr-border-faint)',
                                            cursor: truncated ? 'pointer' : 'default',
                                          }}
                                          title={truncated ? (isExpanded ? t('qa.batch.collapseRow') : t('qa.batch.expandRow')) : undefined}
                                        >
                                          <td style={{ padding: '4px 8px', color: 'var(--kr-text-ghost)', verticalAlign: 'top' }}>{idx + 1}</td>
                                          <td style={{ padding: '4px 8px', fontFamily: 'var(--kr-font-mono)', fontSize: 10, verticalAlign: 'top' }}>
                                            {typeof item.input === 'string'
                                              ? item.input
                                              : JSON.stringify(item.input)}
                                          </td>
                                          <td style={{ padding: '4px 8px', verticalAlign: 'top' }}>
                                            <span style={{
                                              fontWeight: 600,
                                              color: item.status === 'OK' ? 'var(--kr-success)' : 'var(--kr-error)',
                                            }}>
                                              {item.status}
                                              {item.http_status ? ` ${item.http_status}` : ''}
                                            </span>
                                          </td>
                                          <td style={{
                                            padding: '4px 8px',
                                            fontFamily: 'var(--kr-font-mono)',
                                            fontSize: 10,
                                            color: item.status === 'OK' ? 'var(--kr-text-secondary)' : 'var(--kr-error)',
                                            whiteSpace: isExpanded ? 'pre-wrap' : 'nowrap',
                                            wordBreak: isExpanded ? 'break-word' : 'normal',
                                            verticalAlign: 'top',
                                          }}>
                                            {preview}
                                            {truncated && (
                                              <span style={{
                                                color: 'var(--kr-text-ghost)',
                                                fontFamily: 'inherit',
                                                fontSize: 10,
                                                marginLeft: 6,
                                                userSelect: 'none',
                                              }}>
                                                [{isExpanded ? t('qa.batch.collapseRow') : t('qa.batch.expandRow')}]
                                              </span>
                                            )}
                                          </td>
                                        </tr>
                                      );
                                    })}
                                  </tbody>
                                </table>
                              </div>
                            </div>
                          )}
                        </div>
                      )}
                      {/* Inline launch form — variables + result panel. Mirror of
                          the QP launch form. Stays expanded until the user
                          clicks Launch again or another QA's button. */}
                      {launchingQA?.id === qa.id && qa.variables.length > 0 && (
                        <div className="qp-launch-form">
                          {qa.variables.map(v => (
                            <div key={v.name} className="qp-launch-field">
                              <label className="qp-launch-label">
                                {v.label || v.name}
                                {(v.required ?? true) && <span className="wf-required"> *</span>}
                              </label>
                              <input
                                className="wf-input flex-1"
                                value={launchVarsQA[v.name] ?? ''}
                                onChange={e => setLaunchVarsQA(prev => ({ ...prev, [v.name]: e.target.value }))}
                                placeholder={v.placeholder}
                                autoFocus={qa.variables.indexOf(v) === 0}
                                onKeyDown={e => { if (e.key === 'Enter') handleLaunchQA(qa); }}
                              />
                            </div>
                          ))}
                          <button
                            className="qp-launch-go-btn"
                            onClick={() => handleLaunchQA(qa)}
                            disabled={launchingQARun || qa.variables.some(v => (v.required ?? true) && !(launchVarsQA[v.name] ?? '').trim())}
                          >
                            {launchingQARun ? <Loader2 size={14} className="spin" /> : <Play size={14} />}
                            {launchingQARun ? '...' : t('qa.runGo')}
                          </button>
                        </div>
                      )}
                      {/* Result panel — appears under the form once Run fires */}
                      {launchingQA?.id === qa.id && launchQAResult && (
                        <div
                          className={launchQAResult.ok ? 'wf-apicall-success' : 'wf-apicall-error'}
                          style={{ marginTop: 8, padding: 10, borderRadius: 6, maxHeight: 320, overflow: 'auto' }}
                        >
                          {launchQAResult.ok ? (
                            <pre style={{ margin: 0, fontSize: 11, whiteSpace: 'pre-wrap' }}>
                              {JSON.stringify(launchQAResult.payload, null, 2)}
                            </pre>
                          ) : (
                            <span>{launchQAResult.error ?? t('qa.runFailed')}</span>
                          )}
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
      )}

      {/* 0.6.0 UX pass — Workflow launch modal.
          Affiché quand l'user clique "Lancer" sur un workflow déclarant
          des variables manuelles. Mêmes patterns visuels que l'import
          modal pour l'unité d'expérience. Required-but-empty bloque
          submit (gardé en frontend pour feedback instant + le backend
          revalide). Supporte Enter pour valider quand 1 seule var. */}
      {launchingWorkflow && (
        <div
          className="wf-import-modal-backdrop"
          onClick={() => !launchingWorkflow.submitting && setLaunchingWorkflow(null)}
        >
          <div className="wf-import-modal" onClick={(e) => e.stopPropagation()}>
            <div className="flex-row gap-3 mb-4">
              <Play size={16} />
              <h3 className="text-lg font-semibold flex-1" style={{ margin: 0 }}>
                {t('wf.launchModalTitle').replace('{name}', launchingWorkflow.workflow.name)}
              </h3>
              <button
                className="wf-icon-btn"
                onClick={() => setLaunchingWorkflow(null)}
                disabled={launchingWorkflow.submitting}
              ><X size={12} /></button>
            </div>
            <p className="text-xs text-muted mb-4">{t('wf.launchModalHint')}</p>
            {(launchingWorkflow.workflow.variables ?? []).map((v, idx) => {
              const required = v.required ?? true;
              return (
                <div key={v.name} className="qp-launch-field mb-3">
                  <label className="qp-launch-label">
                    {v.label || v.name}
                    {required && <span className="text-error" style={{ marginLeft: 4 }}>*</span>}
                  </label>
                  <input
                    className="wf-input flex-1"
                    value={launchingWorkflow.values[v.name] ?? ''}
                    onChange={e => setLaunchingWorkflow(prev => prev ? {
                      ...prev,
                      values: { ...prev.values, [v.name]: e.target.value },
                      error: null,
                    } : prev)}
                    placeholder={v.placeholder}
                    autoFocus={idx === 0}
                    disabled={launchingWorkflow.submitting}
                    onKeyDown={e => {
                      if (e.key === 'Enter' && !e.shiftKey && !launchingWorkflow.submitting) {
                        e.preventDefault();
                        const submitBtn = (e.currentTarget.closest('.wf-import-modal') as HTMLElement | null)?.querySelector('.wf-launch-submit-btn') as HTMLButtonElement | null;
                        submitBtn?.click();
                      }
                    }}
                  />
                  {v.description && (
                    <p className="text-2xs text-ghost mt-1" style={{ margin: '2px 0 0' }}>{v.description}</p>
                  )}
                </div>
              );
            })}
            {launchingWorkflow.error && (
              <div className="text-xs text-error mb-3">{launchingWorkflow.error}</div>
            )}
            <div className="flex-row gap-3 mt-4">
              <span className="flex-1" />
              <button
                className="wf-small-btn"
                onClick={() => setLaunchingWorkflow(null)}
                disabled={launchingWorkflow.submitting}
              >{t('imp.cancel')}</button>
              <button
                className="wf-small-btn wf-small-btn-accent wf-launch-submit-btn"
                disabled={launchingWorkflow.submitting}
                onClick={async () => {
                  // Validate required vars are filled.
                  const vars = launchingWorkflow.workflow.variables ?? [];
                  const missing = vars
                    .filter(v => (v.required ?? true) && !(launchingWorkflow.values[v.name] ?? '').trim())
                    .map(v => v.label || v.name);
                  if (missing.length > 0) {
                    setLaunchingWorkflow(prev => prev ? {
                      ...prev,
                      error: t('wf.launchModalRequired').replace('{names}', missing.join(', ')),
                    } : prev);
                    return;
                  }
                  setLaunchingWorkflow(prev => prev ? { ...prev, submitting: true, error: null } : prev);
                  try {
                    await fireTrigger(launchingWorkflow.workflow.id, launchingWorkflow.values);
                    setLaunchingWorkflow(null);
                  } catch (e) {
                    console.warn('Launch failed:', e);
                    setLaunchingWorkflow(prev => prev ? {
                      ...prev,
                      submitting: false,
                      error: String(e),
                    } : prev);
                  }
                }}
              >
                {launchingWorkflow.submitting
                  ? <Loader2 size={10} className="spin" />
                  : <Play size={10} />}
                {t('wf.launchModalGo')}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 0.7.0 UX pass — Import drawer (workflow OR qp).
          2-step flow : (1) drop / pick file → preview rendered + project
          dropdown ; (2) confirm → POST → refetch + close.
          Modal-style overlay so the user is focused on the operation. */}
      {importing && (
        <div className="wf-import-modal-backdrop" onClick={() => !importingSubmit && setImporting(null)}>
          <div className="wf-import-modal" onClick={(e) => e.stopPropagation()}>
            <div className="flex-row gap-3 mb-4">
              <Upload size={16} />
              <h3 className="text-lg font-semibold flex-1" style={{ margin: 0 }}>
                {importing.kind === 'workflow' ? t('imp.workflowTitle') : t('imp.qpTitle')}
              </h3>
              <button className="wf-icon-btn" onClick={() => setImporting(null)} disabled={importingSubmit}>
                <X size={12} />
              </button>
            </div>

            {!importing.content ? (
              <ImportDropzone
                expectedKind={importing.kind === 'workflow' ? 'kronn.workflow' : 'kronn.quick_prompt'}
                embedded
                onFile={(content, parsed) => {
                  if (importing.kind === 'workflow') {
                    const env = parsed as { workflow?: { name?: string; steps?: unknown[] }; referenced_quick_prompts?: unknown[] };
                    setImporting({
                      ...importing,
                      content,
                      preview: {
                        name: env.workflow?.name ?? '?',
                        stepCount: env.workflow?.steps?.length ?? 0,
                        qpVarsCount: env.referenced_quick_prompts?.length ?? 0,
                      },
                    });
                  } else {
                    const env = parsed as { quick_prompt?: { name?: string; variables?: unknown[] } };
                    setImporting({
                      ...importing,
                      content,
                      preview: {
                        name: env.quick_prompt?.name ?? '?',
                        qpVarsCount: env.quick_prompt?.variables?.length ?? 0,
                      },
                    });
                  }
                }}
              />
            ) : (
              <>
                <div className="wf-import-preview">
                  <div className="text-sm">
                    <span className="text-muted">{t('imp.previewName')}:</span>{' '}
                    <strong>{importing.preview.name}</strong>
                  </div>
                  {importing.kind === 'workflow' && (
                    <>
                      <div className="text-sm">
                        <span className="text-muted">{t('imp.previewSteps')}:</span>{' '}
                        {importing.preview.stepCount}
                      </div>
                      {(importing.preview.qpVarsCount ?? 0) > 0 && (
                        <div className="text-sm">
                          <span className="text-muted">{t('imp.previewBundledQps')}:</span>{' '}
                          {importing.preview.qpVarsCount}
                        </div>
                      )}
                    </>
                  )}
                  {importing.kind === 'qp' && (importing.preview.qpVarsCount ?? 0) > 0 && (
                    <div className="text-sm">
                      <span className="text-muted">{t('imp.previewVars')}:</span>{' '}
                      {importing.preview.qpVarsCount}
                    </div>
                  )}
                </div>

                <label className="wf-label mt-4">{t('imp.targetProject')}</label>
                <select
                  className="wf-select text-sm"
                  value={importing.targetProjectId}
                  onChange={(e) => setImporting({ ...importing, targetProjectId: e.target.value })}
                  disabled={importingSubmit}
                >
                  <option value="">— {t('imp.noProject')} —</option>
                  {projects.map((p) => (
                    <option key={p.id} value={p.id}>{p.name}</option>
                  ))}
                </select>

                <div className="flex-row gap-3 mt-6">
                  <button
                    className="wf-small-btn"
                    onClick={() => setImporting({ ...importing, content: '', preview: { name: '' } })}
                    disabled={importingSubmit}
                  >
                    {t('imp.pickAnother')}
                  </button>
                  <span className="flex-1" />
                  <button
                    className="wf-small-btn"
                    onClick={() => setImporting(null)}
                    disabled={importingSubmit}
                  >
                    {t('imp.cancel')}
                  </button>
                  <button
                    className="wf-small-btn wf-small-btn-accent"
                    disabled={importingSubmit}
                    onClick={async () => {
                      setImportingSubmit(true);
                      try {
                        const projectId = importing.targetProjectId || null;
                        if (importing.kind === 'workflow') {
                          await workflowsApi.importWorkflow({ content: importing.content, project_id: projectId });
                          if (toastProp) toastProp(t('imp.workflowDone').replace('{name}', importing.preview.name), 'success');
                          refetch();
                        } else {
                          await quickPromptsApi.importQp({ content: importing.content, project_id: projectId });
                          if (toastProp) toastProp(t('imp.qpDone').replace('{name}', importing.preview.name), 'success');
                          refetchQP();
                        }
                        setImporting(null);
                      } catch (e) {
                        if (toastProp) toastProp(userError(e), 'error');
                      } finally {
                        setImportingSubmit(false);
                      }
                    }}
                  >
                    {importingSubmit ? <Loader2 size={10} className="spin" /> : <Upload size={10} />}
                    {t('imp.confirm')}
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
