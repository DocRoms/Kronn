import { Fragment, useState, useRef, useMemo, useEffect, useCallback } from 'react';
import { appendLiveBuffer } from '../lib/workflowUiUtils';
import { useIsMobile } from '../hooks/useMediaQuery';
import { useT } from '../lib/I18nContext';
import { workflows as workflowsApi, discussions as discussionsApi, quickPrompts as quickPromptsApi, quickApis as quickApisApi, quickExecs as quickExecsApi, mcps as mcpsApi, skills as skillsApi, profiles as profilesApi, directives as directivesApi } from '../lib/api';
import { userError } from '../lib/userError';
import { useApi } from '../hooks/useApi';
import type {
  Project, WorkflowSummary, Workflow, WorkflowRun,
  AgentType, AgentsConfig, ModelTier, StepResult, QuickPrompt, CreateQuickPromptRequest,
  QuickApi, CreateQuickApiRequest, QuickExec, CreateQuickExecRequest,
  JsonValue,
} from '../types/generated';
import type { ApiPluginOption } from '../components/workflows/ApiCallStepCard';
import {
  Plus, Trash2, Play, Loader2, ChevronLeft, ChevronRight, ChevronDown,
  Clock, GitBranch, Zap, Eye, Layers, X, Square,
  ToggleLeft, ToggleRight, Star,
  Upload, Download, AlertTriangle, Search, Workflow as WorkflowIcon,
  PlugZap, MessageSquareText, TerminalSquare,
} from 'lucide-react';
import { useWebSocket } from '../hooks/useWebSocket';
import { WorkflowDetail } from '../components/workflows/WorkflowDetail';
import { WorkflowWizard } from '../components/workflows/WorkflowWizard';
import { QuickPromptForm } from '../components/workflows/QuickPromptForm';
import { QuickApiForm } from '../components/workflows/QuickApiForm';
import { QuickExecForm } from '../components/workflows/QuickExecForm';
import QPHistoryDrawer from '../components/QPHistoryDrawer';
import QPCardMetricsChip from '../components/QPCardMetricsChip';
import { parseBatchQAItems } from '../components/workflows/parseBatchQAItems';
import { ImportDropzone } from '../components/workflows/ImportDropzone';
import { triggerDownload } from '../lib/downloadBlob';
import { mergeDeclaredAndDetected } from '../lib/workflowVariables';
import { detectAutomationImport, type AutomationImportKind } from '../lib/automationImport';
import { AGENT_LABELS, MODEL_TIER_ICONS, agentColor, modelForAgentTier } from '../lib/constants';
import { MatrixText } from '../components/MatrixText';
import { AgentSwitchPicker } from '../components/AgentSwitchPicker';
import { FavoriteToggle } from '../components/FavoriteToggle';
import { ListControls } from '../components/ListControls';
import { CopyIdPill } from '../components/CopyIdPill';
import { ContextHelp } from '../components/ContextHelp';
import {
  sortQuickApis,
  sortQuickPrompts,
  type QuickApiSort,
  type QuickPromptSort,
} from '../lib/automationSort';
import './DiscussionsPage.css';
import './WorkflowsPage.css';

type AutomationTab = 'workflows' | 'quickPrompts' | 'quickApis' | 'quickExecs';
type AutomationSection = AutomationTab | 'favorites';

const AUTOMATION_TABS: AutomationTab[] = ['workflows', 'quickPrompts', 'quickApis', 'quickExecs'];
const AUTOMATION_NAVIGATION_STORAGE_KEY = 'kronn:automationNavigation';
const AUTOMATION_COLLAPSED_STORAGE_KEY = 'kronn:automationCollapsedSections';

interface AutomationNavigationState {
  tab: AutomationTab;
  resourceId: string | null;
}

interface AutomationResourceRowProps {
  name: string;
  meta: string;
  icon: string;
  active: boolean;
  pinned: boolean;
  openLabel: string;
  pinLabel: string;
  unpinLabel: string;
  onOpen: () => void;
  onTogglePinned: () => void;
}

function AutomationResourceRow({
  name,
  meta,
  icon,
  active,
  pinned,
  openLabel,
  pinLabel,
  unpinLabel,
  onOpen,
  onTogglePinned,
}: AutomationResourceRowProps) {
  return (
    <div className="disc-swipe-wrap automation-resource-row">
      <div className="disc-item" data-active={active}>
        <button
          type="button"
          className="disc-item-open"
          onClick={onOpen}
          aria-label={openLabel}
        >
          <span className="automation-resource-icon" aria-hidden="true">{icon}</span>
          <span className="disc-item-content">
            <span className="disc-item-title">
              <span className="disc-item-title-text">{name}</span>
            </span>
            <span className="disc-item-meta">
              <span className="disc-item-meta-summary">{meta}</span>
            </span>
          </span>
        </button>
        <FavoriteToggle
          active={pinned}
          onToggle={onTogglePinned}
          activeLabel={unpinLabel}
          inactiveLabel={pinLabel}
          itemName={name}
          className="automation-resource-pin"
        />
      </div>
    </div>
  );
}

function isAutomationTab(value: unknown): value is AutomationTab {
  return typeof value === 'string' && AUTOMATION_TABS.includes(value as AutomationTab);
}

function readAutomationNavigation(): AutomationNavigationState {
  try {
    const raw = localStorage.getItem(AUTOMATION_NAVIGATION_STORAGE_KEY);
    if (!raw) return { tab: 'workflows', resourceId: null };
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    return {
      tab: isAutomationTab(parsed.tab) ? parsed.tab : 'workflows',
      resourceId: typeof parsed.resourceId === 'string' && parsed.resourceId.trim()
        ? parsed.resourceId
        : null,
    };
  } catch {
    return { tab: 'workflows', resourceId: null };
  }
}

function isAutomationSection(value: unknown): value is AutomationSection {
  return value === 'favorites' || isAutomationTab(value);
}

function readCollapsedAutomationSections(): Set<AutomationSection> {
  try {
    const parsed = JSON.parse(
      localStorage.getItem(AUTOMATION_COLLAPSED_STORAGE_KEY) ?? '[]',
    ) as unknown;
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter(isAutomationSection));
  } catch {
    return new Set();
  }
}

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
  onBatchLaunched?: (discIds: string[], batchRunId: string, mode?: 'batch' | 'compare') => void;
  /** When the user clicks a batch pastille in the discussion sidebar, the
   * Dashboard switches to this tab and sets this prop to the parent workflow
   * id. We auto-open its detail panel + switch to the 'workflows' sub-tab
   * so the user lands exactly on the run that spawned their batch. */
  initialSelectedWorkflowId?: string | null;
  /** Optional run to focus when cross-page navigation targets a specific
   * publication from the Page refresh timeline. */
  initialSelectedWorkflowRunId?: string | null;
  /** Ack callback — Dashboard clears the id after we've consumed it so the
   * same click doesn't re-open on every render. */
  onInitialSelectionConsumed?: () => void;
  /** Reverse direction: when "📋 N conversations" is clicked on a workflow run,
   * jump to the discussions tab and focus that batch group. */
  onNavigateToBatch?: (batchRunId: string) => void;
  toast?: (msg: string, type?: 'success' | 'error' | 'info') => void;
  /** 0.8.2 — Deep-link from the audit-validation CTA: opens the create
   * wizard with a preset pre-applied (e.g. `ticket-to-pr` for AutoPilot)
   * and the project pre-selected. Ack via `onPendingPresetConsumed`. */
  pendingPreset?: { presetId: string; projectId: string } | null;
  onPendingPresetConsumed?: () => void;
  onNavigatePage?: (pageId: string) => void;
}

const TRIGGER_LABELS: Record<string, string> = {
  cron: 'Cron',
  tracker: 'Tracker',
  manual: 'Manuel',
};

const RUN_FETCH_PAGE_SIZE = 10;
const RUN_FETCH_MAX_PAGE_SIZE = 500;

function readPostImprovedQuickPromptId(): string | null {
  try {
    return sessionStorage.getItem('kronn:postQpImproved');
  } catch {
    // sessionStorage may be unavailable in private/restricted browser modes.
    return null;
  }
}

export function WorkflowsPage({ projects, installedAgentTypes, agentAccess, configLanguage, onNavigateDiscussion, onBatchLaunched, initialSelectedWorkflowId, initialSelectedWorkflowRunId, onInitialSelectionConsumed, onNavigateToBatch, toast: toastProp, pendingPreset, onPendingPresetConsumed, onNavigatePage }: WorkflowsPageProps) {
  const { t } = useT();
  // The 380px workflow list plus the detail panel needs substantially more
  // room than a phone-only breakpoint. Switch to the existing single-pane
  // navigation on tablets too, before the detail content becomes cramped.
  const isMobile = useIsMobile(1024);
  // Consume the one-shot navigation target during initial state creation.
  // Deriving the initial tab here avoids a second render solely to mirror
  // sessionStorage from a mount effect.
  const [postImprovedQpId, setPostImprovedQpId] = useState<string | null>(
    readPostImprovedQuickPromptId,
  );
  const [initialAutomationNavigation] = useState(readAutomationNavigation);
  const [tab, setTab] = useState<AutomationTab>(
    postImprovedQpId ? 'quickPrompts' : initialAutomationNavigation.tab,
  );
  const [automationQuery, setAutomationQuery] = useState('');
  const automationSearchRef = useRef<HTMLInputElement>(null);
  const [automationProjectFilter, setAutomationProjectFilter] = useState('all');
  const [collapsedAutomationSections, setCollapsedAutomationSections] = useState<Set<AutomationSection>>(
    readCollapsedAutomationSections,
  );
  useEffect(() => {
    const focusSearch = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (event.key !== '/' || target?.matches('input, textarea, select, [contenteditable="true"]')) return;
      event.preventDefault();
      automationSearchRef.current?.focus();
    };
    window.addEventListener('keydown', focusSearch);
    return () => window.removeEventListener('keydown', focusSearch);
  }, []);
  const [quickPromptSort, setQuickPromptSort] = useState<QuickPromptSort>('name');
  const [quickPromptSortReversed, setQuickPromptSortReversed] = useState(false);
  const [quickPromptAgentFilter, setQuickPromptAgentFilter] = useState<string>('all');
  const [quickApiSort, setQuickApiSort] = useState<QuickApiSort>('name');
  const [quickApiSortReversed, setQuickApiSortReversed] = useState(false);
  const [quickApiPluginFilter, setQuickApiPluginFilter] = useState('all');
  const [quickPromptUsage, setQuickPromptUsage] = useState<Record<string, number>>({});
  // 0.8.5 — post-deploy focus: when the user lands here right after
  // clicking "Deploy improved QP" in DiscussionsPage, this state holds
  // the target QP id and briefly highlights the matching card.
  const { data: workflowList, refetch } = useApi(() => workflowsApi.list(), []);
  const { data: quickPromptList, refetch: refetchQP } = useApi(() => quickPromptsApi.list(), []);
  const { data: quickApiList, refetch: refetchQA } = useApi(() => quickApisApi.list(), []);
  const { data: quickExecList, refetch: refetchQE } = useApi(() => quickExecsApi.list(), []);
  // 0.8.5 — catalogs for the QP form binding pickers (skills + profiles +
  // directives). Empty-array fallback keeps the form rendering during the
  // first paint before the API resolves.
  const { data: skillsCatalog } = useApi(() => skillsApi.list(), []);
  const { data: profilesCatalog } = useApi(() => profilesApi.list(), []);
  const { data: directivesCatalog } = useApi(() => directivesApi.list(), []);
  const recordQuickPromptUsage = useCallback((qpId: string, launches: number) => {
    setQuickPromptUsage(current =>
      current[qpId] === launches ? current : { ...current, [qpId]: launches }
    );
  }, []);
  const quickPromptUsageReady = (quickPromptList ?? [])
    .every(qp => Object.hasOwn(quickPromptUsage, qp.id));
  const [selectedId, setSelectedId] = useState<string | null>(
    initialAutomationNavigation.tab === 'workflows'
      ? initialAutomationNavigation.resourceId
      : null,
  );
  const [selectedQuickPromptId, setSelectedQuickPromptId] = useState<string | null>(
    postImprovedQpId
      ?? (initialAutomationNavigation.tab === 'quickPrompts'
        ? initialAutomationNavigation.resourceId
        : null),
  );
  const [selectedQuickApiId, setSelectedQuickApiId] = useState<string | null>(
    initialAutomationNavigation.tab === 'quickApis'
      ? initialAutomationNavigation.resourceId
      : null,
  );
  const [selectedQuickExecId, setSelectedQuickExecId] = useState<string | null>(
    initialAutomationNavigation.tab === 'quickExecs'
      ? initialAutomationNavigation.resourceId
      : null,
  );
  const invalidWorkflowSelection = Boolean(
    workflowList && selectedId && !workflowList.some(item => item.id === selectedId),
  );
  const invalidQuickPromptSelection = Boolean(
    quickPromptList && selectedQuickPromptId
      && !quickPromptList.some(item => item.id === selectedQuickPromptId),
  );
  const invalidQuickApiSelection = Boolean(
    quickApiList && selectedQuickApiId
      && !quickApiList.some(item => item.id === selectedQuickApiId),
  );
  const invalidQuickExecSelection = Boolean(
    quickExecList && selectedQuickExecId
      && !quickExecList.some(item => item.id === selectedQuickExecId),
  );
  useEffect(() => {
    const resourceId = tab === 'workflows'
      ? (invalidWorkflowSelection ? null : selectedId)
      : tab === 'quickPrompts'
        ? (invalidQuickPromptSelection ? null : selectedQuickPromptId)
        : tab === 'quickApis'
          ? (invalidQuickApiSelection ? null : selectedQuickApiId)
          : (invalidQuickExecSelection ? null : selectedQuickExecId);
    try {
      localStorage.setItem(
        AUTOMATION_NAVIGATION_STORAGE_KEY,
        JSON.stringify({ tab, resourceId }),
      );
    } catch {
      // localStorage may be unavailable in private/restricted browser modes.
    }
  }, [
    invalidQuickApiSelection,
    invalidQuickExecSelection,
    invalidQuickPromptSelection,
    invalidWorkflowSelection,
    selectedId,
    selectedQuickApiId,
    selectedQuickExecId,
    selectedQuickPromptId,
    tab,
  ]);
  useEffect(() => {
    try {
      localStorage.setItem(
        AUTOMATION_COLLAPSED_STORAGE_KEY,
        JSON.stringify([...collapsedAutomationSections]),
      );
    } catch {
      // localStorage may be unavailable in private/restricted browser modes.
    }
  }, [collapsedAutomationSections]);
  // #11 — when a parent run's sub-run link is clicked, remember the target
  // child run so the opened WorkflowDetail auto-expands + scrolls to it.
  const [focusRunId, setFocusRunId] = useState<string | null>(null);
  const [showCreateQP, setShowCreateQP] = useState(false);
  const [editingQP, setEditingQP] = useState<QuickPrompt | null>(null);
  const [launchingQP, setLaunchingQP] = useState<QuickPrompt | null>(null);
  const [launchVars, setLaunchVars] = useState<Record<string, string>>({});
  const [launching, setLaunching] = useState(false);
  // Race-free re-entry guard for handleLaunchQP. Two fast Enter presses on the
  // variable input (or two fast clicks on the inner Launch button) fire two
  // `discussions.create` calls before React re-renders the disabled state.
  // The ref reads/writes synchronously, so the second invocation bails out.
  const launchingRef = useRef(false);
  // Compare targets retain both axes that affect an answer. `null` means one
  // target per installed agent at the QP's tier. Keeping an explicit tier per
  // row also allows Codex/default vs Codex/reasoning comparisons.
  const [compareTargets, setCompareTargets] = useState<Array<{ agent: AgentType; tier: ModelTier }> | null>(null);
  // Batch launch state — when the user clicks "Batch" on a QP, we show a
  // modal that asks for one value of the first variable per line, then
  // fans out N discussions via POST /api/quick-prompts/:id/batch.
  const [batchingQP, setBatchingQP] = useState<QuickPrompt | null>(null);
  const [batchInputLines, setBatchInputLines] = useState('');
  const [batchLaunching, setBatchLaunching] = useState(false);
  const batchLaunchingRef = useRef(false); // Race-free guard, cf launchingRef.
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
  const launchingQARunRef = useRef(false); // Race-free guard, cf launchingRef.
  const [launchQAResult, setLaunchQAResult] = useState<{ ok: boolean; payload: unknown; error: string | null } | null>(null);
  // Batch state — same pattern as `batchingQP` but for QAs. Items is a
  // newline/comma/semicolon-separated string the user pastes; we parse
  // it into a JSON array (strings if one var on the QA, otherwise prompt
  // for object form) before posting to /api/quick-apis/:id/batch.
  const [batchingQA, setBatchingQA] = useState<QuickApi | null>(null);
  const [batchQAInput, setBatchQAInput] = useState('');
  const [batchQAConcurrentLimit, setBatchQAConcurrentLimit] = useState(5);
  const [batchQARunning, setBatchQARunning] = useState(false);
  const batchQARunningRef = useRef(false); // Race-free guard, cf launchingRef.
  const [batchQAResult, setBatchQAResult] = useState<{ status: string; items: Array<{ input: unknown; status: string; response?: unknown; error?: string; http_status?: number }>; total: number; succeeded: number; failed: number } | null>(null);
  const [showCreateQE, setShowCreateQE] = useState(false);
  const [editingQE, setEditingQE] = useState<QuickExec | null>(null);
  const [runningQE, setRunningQE] = useState<QuickExec | null>(null);
  const [runVarsQE, setRunVarsQE] = useState<Record<string, string>>({});
  const [runQEState, setRunQEState] = useState<{ busy: boolean; data: unknown | null; error: string | null }>({ busy: false, data: null, error: null });
  // Lignes du tableau de résultats batch dépliées (Set d'index). On garde
  // un Set plutôt qu'un single index pour permettre la comparaison de
  // plusieurs réponses côte-à-côte. Reset à chaque nouveau batch.
  const [expandedQARows, setExpandedQARows] = useState<Set<number>>(new Set());
  // Available API plugins (with `api_spec != null` and a configured
  // backend) — needed by `ApiCallStepCard` inside `QuickApiForm`. Loaded
  // lazily on tab open since not every user wires API plugins.
  const [availableApiPlugins, setAvailableApiPlugins] = useState<ApiPluginOption[]>([]);
  const quickPromptAgents = useMemo(
    () => Array.from(new Set((quickPromptList ?? []).map(qp => qp.agent)))
      .sort((a, b) => (AGENT_LABELS[a] ?? a).localeCompare(AGENT_LABELS[b] ?? b)),
    [quickPromptList],
  );
  const quickApiPlugins = useMemo(() => {
    const labels = new Map(
      availableApiPlugins.map(option => [option.server.id, option.server.name]),
    );
    return Array.from(new Set((quickApiList ?? []).map(qa => qa.api_plugin_slug)))
      .sort((a, b) => (labels.get(a) ?? a).localeCompare(labels.get(b) ?? b))
      .map(slug => ({ slug, label: labels.get(slug) ?? slug }));
  }, [availableApiPlugins, quickApiList]);
  const sortedQuickPrompts = useMemo(() => {
    const filtered = quickPromptAgentFilter === 'all'
      ? (quickPromptList ?? [])
      : (quickPromptList ?? []).filter(qp => qp.agent === quickPromptAgentFilter);
    return sortQuickPrompts(
      filtered,
      quickPromptSort,
      quickPromptUsage,
      quickPromptSortReversed,
    );
  }, [
    quickPromptAgentFilter,
    quickPromptList,
    quickPromptSort,
    quickPromptSortReversed,
    quickPromptUsage,
  ]);
  const sortedQuickApis = useMemo(() => {
    const filtered = quickApiPluginFilter === 'all'
      ? (quickApiList ?? [])
      : (quickApiList ?? []).filter(qa => qa.api_plugin_slug === quickApiPluginFilter);
    return sortQuickApis(filtered, quickApiSort, quickApiSortReversed);
  }, [quickApiList, quickApiPluginFilter, quickApiSort, quickApiSortReversed]);
  const [showCreate, setShowCreate] = useState(false);
  // 0.8.2 — Local snapshot of the pending preset, captured at the moment
  // `pendingPreset` arrives. We can't pass `pendingPreset` directly to
  // WorkflowWizard because Dashboard clears it on consume (causing the
  // wizard to remount without the preset). Snapshot survives the ack.
  const [pendingPresetLocal, setPendingPresetLocal] = useState<{ presetId: string; projectId: string } | null>(null);
  useEffect(() => {
    if (!pendingPreset) return;
    setPendingPresetLocal(pendingPreset);
    setShowCreate(true);
    onPendingPresetConsumed?.();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingPreset]);

  // A mount effect is appropriate for consuming the external navigation
  // receipt; unlike the previous implementation, it does not derive React
  // state or cause an additional render.
  useEffect(() => {
    if (!postImprovedQpId) return;
    try { sessionStorage.removeItem('kronn:postQpImproved'); } catch { /* private mode */ }
  }, [postImprovedQpId]);

  // Scroll + flash highlight the deep-linked QP card once the list is
  // available. Uses a 1.4s CSS animation (defined in WorkflowsPage.css)
  // and clears `postImprovedQpId` after to avoid re-flashing on every
  // re-render of the page.
  useEffect(() => {
    if (!postImprovedQpId) return;
    if (!quickPromptList || quickPromptList.length === 0) return;
    // Defer to the next paint so the QP cards have mounted under the
    // active tab (we just switched tabs on mount; the cards render on
    // the next React commit).
    const raf1 = requestAnimationFrame(() => {
      const raf2 = requestAnimationFrame(() => {
        const el = document.querySelector(`[data-qp-id="${postImprovedQpId}"]`) as HTMLElement | null;
        if (el) {
          el.scrollIntoView({ behavior: 'smooth', block: 'center' });
          el.classList.add('qp-card-flash');
          window.setTimeout(() => el.classList.remove('qp-card-flash'), 1500);
        }
        setPostImprovedQpId(null);
      });
      return () => cancelAnimationFrame(raf2);
    });
    return () => cancelAnimationFrame(raf1);
  }, [postImprovedQpId, quickPromptList]);
  const [editingWorkflow, setEditingWorkflow] = useState<Workflow | null>(null);
  // 0.7.0 UX pass — import drawer state. Set when the user clicks the
  // "Importer" button on either tab. Carries the parsed JSON content +
  // a small preview snapshot so the drawer can render "tu vas importer
  // X" before confirm.
  const [importing, setImporting] = useState<{
    kind: AutomationImportKind | null;
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
  const [detailRunPageCount, setDetailRunPageCount] = useState(0);
  const [detailRunTotal, setDetailRunTotal] = useState(0);
  const [hasMoreDetailRuns, setHasMoreDetailRuns] = useState(false);
  const [loadingMoreRuns, setLoadingMoreRuns] = useState(false);
  const loadingMoreRunsRef = useRef(false);
  // Throttle clock for mirroring WorkflowRunUpdated into the run list while a
  // local SSE run is streaming (see the useWebSocket handler below).
  const lastRunsRefetchRef = useRef(0);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [triggering, setTriggering] = useState<string | null>(null);
  // Race-free guard for handleTrigger / fireTrigger — `disabled={triggering===wf.id}`
  // is closure-stale between two synchronous clicks (the button re-render
  // hasn't happened yet), so a fast double-click on "Lancer" can call
  // workflowsApi.triggerStream twice in parallel and produce two runs of
  // the same workflow.
  const triggeringRef = useRef(false);
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
  // Abort the live-run SSE reader on unmount — it only stops WATCHING; the
  // backend run is unaffected. Without it the reader outlives tab switches.
  useEffect(() => () => { abortRef.current?.abort(); }, []);
  // Launch generation: abort() can't cancel an already-scheduled RAF or a
  // resolved read() microtask — callbacks must check they're still current.
  const launchGenRef = useRef(0);
  // openDetail response-ordering guard (see openDetail).
  const detailSeqRef = useRef(0);
  const detailScrollRef = useRef<HTMLDivElement>(null);

  const workflows = useMemo(() => workflowList ?? [], [workflowList]);
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
    // Group order mirrors the discussions sidebar: Favorites first
    // (cross-project, collapsible), then the "no project" bucket (shares
    // the `disc.general` wording — it used to be last + labelled "Sans
    // projet", which made the two panels feel like different products),
    // then one group per project. Pinned workflows also stay in their
    // project group (disc behaviour; React keys are scoped per group).
    const pinned = workflows.filter(wf => wf.pinned);
    if (pinned.length > 0) {
      groups.push({ key: '__favorites__', label: t('disc.favorites'), workflows: pinned });
    }
    if (noProject.length > 0) {
      groups.push({ key: '__global__', label: t('disc.general'), workflows: noProject });
    }
    for (const [pid, wfs] of byProject) {
      const label = wfs[0].project_name ?? pid;
      groups.push({ key: pid, label, workflows: wfs });
    }
    return groups;
  }, [workflows, t]);

  const matchesAutomationProject = useCallback((projectId: string | null | undefined) => (
    automationProjectFilter === 'all'
      || (automationProjectFilter === '__global__' ? !projectId : projectId === automationProjectFilter)
  ), [automationProjectFilter]);
  const automationNeedle = automationQuery.trim().toLocaleLowerCase();
  const matchesAutomationQuery = useCallback((...values: Array<string | null | undefined>) => (
    !automationNeedle
      || values.some(value => value?.toLocaleLowerCase().includes(automationNeedle))
  ), [automationNeedle]);
  const sidebarWorkflows = useMemo(
    () => workflows.filter(workflow => (
      matchesAutomationProject(workflow.project_id)
      && matchesAutomationQuery(workflow.name, workflow.project_name, workflow.trigger_type)
    )).sort((left, right) => Number(right.pinned) - Number(left.pinned)
      || left.name.localeCompare(right.name)),
    [matchesAutomationProject, matchesAutomationQuery, workflows],
  );
  const sidebarQuickApis = useMemo(
    () => (quickApiList ?? []).filter(quickApi => (
      matchesAutomationProject(quickApi.project_id)
      && matchesAutomationQuery(
        quickApi.name,
        quickApi.description,
        quickApi.api_plugin_slug,
        quickApi.api_endpoint_path,
      )
    )).sort((left, right) => Number(right.pinned) - Number(left.pinned)
      || left.name.localeCompare(right.name)),
    [matchesAutomationProject, matchesAutomationQuery, quickApiList],
  );
  const sidebarQuickPrompts = useMemo(
    () => (quickPromptList ?? []).filter(quickPrompt => (
      matchesAutomationProject(quickPrompt.project_id)
      && matchesAutomationQuery(
        quickPrompt.name,
        quickPrompt.description,
        AGENT_LABELS[quickPrompt.agent] ?? quickPrompt.agent,
      )
    )).sort((left, right) => Number(right.pinned) - Number(left.pinned)
      || left.name.localeCompare(right.name)),
    [matchesAutomationProject, matchesAutomationQuery, quickPromptList],
  );
  const sidebarQuickExecs = useMemo(
    () => (quickExecList ?? []).filter(quickExec => (
      matchesAutomationProject(quickExec.project_id)
      && matchesAutomationQuery(
        quickExec.name,
        quickExec.description,
        quickExec.command,
        quickExec.output_format,
      )
    )).sort((left, right) => Number(right.pinned) - Number(left.pinned)
      || left.name.localeCompare(right.name)),
    [matchesAutomationProject, matchesAutomationQuery, quickExecList],
  );
  const sidebarFavoriteCount = sidebarWorkflows.filter(item => item.pinned).length
    + sidebarQuickApis.filter(item => item.pinned).length
    + sidebarQuickPrompts.filter(item => item.pinned).length
    + sidebarQuickExecs.filter(item => item.pinned).length;
  const visibleQuickPrompts = selectedQuickPromptId
    ? (quickPromptList ?? []).filter(item => item.id === selectedQuickPromptId)
    : sortedQuickPrompts;
  const visibleQuickApis = selectedQuickApiId
    ? (quickApiList ?? []).filter(item => item.id === selectedQuickApiId)
    : sortedQuickApis;
  const visibleQuickExecs = selectedQuickExecId
    ? (quickExecList ?? []).filter(item => item.id === selectedQuickExecId)
    : (quickExecList ?? []);
  const selectedQuickPrompt = selectedQuickPromptId
    ? (quickPromptList ?? []).find(item => item.id === selectedQuickPromptId) ?? null
    : null;
  const selectedQuickApi = selectedQuickApiId
    ? (quickApiList ?? []).find(item => item.id === selectedQuickApiId) ?? null
    : null;
  const selectedQuickExec = selectedQuickExecId
    ? (quickExecList ?? []).find(item => item.id === selectedQuickExecId) ?? null
    : null;

  useEffect(() => {
    if (!invalidWorkflowSelection && !invalidQuickPromptSelection
      && !invalidQuickApiSelection && !invalidQuickExecSelection) return;
    const timeoutId = window.setTimeout(() => {
      if (invalidWorkflowSelection) {
        setSelectedId(null);
        setDetailWorkflow(null);
      }
      if (invalidQuickPromptSelection) setSelectedQuickPromptId(null);
      if (invalidQuickApiSelection) setSelectedQuickApiId(null);
      if (invalidQuickExecSelection) setSelectedQuickExecId(null);
    }, 0);
    return () => window.clearTimeout(timeoutId);
  }, [
    invalidQuickApiSelection,
    invalidQuickExecSelection,
    invalidQuickPromptSelection,
    invalidWorkflowSelection,
  ]);

  const toggleGroup = (key: string) => {
    setCollapsedGroups(prev => ({ ...prev, [key]: !prev[key] }));
  };

  const fetchRunPage = async (id: string, offset: number, pageSize: number) => {
    const [runs, total] = await Promise.all([
      workflowsApi.listRuns(id, pageSize, offset, true),
      workflowsApi.countRuns(id),
    ]);
    return {
      runs,
      total,
      hasMore: offset + runs.length < total,
    };
  };

  const openDetail = async (id: string, runId?: string, runLimit = RUN_FETCH_PAGE_SIZE) => {
    // The workflow list and detail are independently scrollable on desktop.
    // Keep the user's position in a long list, but always reveal the top of
    // the newly selected workflow instead of leaving its detail scrolled down.
    if (detailScrollRef.current) detailScrollRef.current.scrollTop = 0;
    setSelectedId(id);
    setFocusRunId(runId ?? null);
    setLoadingDetail(true);
    setHasMoreDetailRuns(false);
    // Out-of-order guard: click A (slow fetch) then B (fast) — A's response
    // landing last must not repaint the panel with A while selectedId is B.
    const seq = ++detailSeqRef.current;
    try {
      const [wf, page, focusedRun] = await Promise.all([
        workflowsApi.get(id),
        fetchRunPage(id, 0, runLimit),
        runId ? workflowsApi.getRun(id, runId).catch(() => null) : Promise.resolve(null),
      ]);
      if (detailSeqRef.current !== seq) return;
      setDetailWorkflow(wf);
      const runs = focusedRun && !page.runs.some(run => run.id === focusedRun.id)
        ? [...page.runs, focusedRun]
        : page.runs;
      setDetailRuns(runs);
      setDetailRunPageCount(page.runs.length);
      setDetailRunTotal(page.total);
      setHasMoreDetailRuns(page.hasMore);
    } catch (e) {
      console.warn('Workflow action failed:', e);
    } finally {
      if (detailSeqRef.current === seq) setLoadingDetail(false);
    }
  };

  const restoredWorkflowSelection = useRef(false);
  useEffect(() => {
    if (restoredWorkflowSelection.current || !workflowList) return;
    restoredWorkflowSelection.current = true;
    if (postImprovedQpId || initialSelectedWorkflowId) return;
    if (initialAutomationNavigation.tab !== 'workflows'
      || !initialAutomationNavigation.resourceId
      || !workflowList.some(item => item.id === initialAutomationNavigation.resourceId)) return;
    const resourceId = initialAutomationNavigation.resourceId;
    const timeoutId = window.setTimeout(() => { void openDetail(resourceId); }, 0);
    // The restore target is an immutable mount snapshot; subsequent selections
    // already call openDetail directly.
    return () => window.clearTimeout(timeoutId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workflowList]);

  const changeStepAgent = async (stepIndex: number, agent: AgentType, tier: ModelTier) => {
    const workflow = detailWorkflow;
    const step = workflow?.steps[stepIndex];
    if (!workflow || !step || (step.step_type && step.step_type.type !== 'Agent')) return;
    const currentTier = step.agent_settings?.tier ?? 'default';
    if (step.agent === agent && currentTier === tier) return;

    // Selecting an abstract tier explicitly clears any concrete model. A
    // concrete model wins over the tier at runtime, so preserving it here
    // would make the newly selected mode look active without taking effect.
    const steps = workflow.steps.map((current, index) =>
      index === stepIndex
        ? {
            ...current,
            agent,
            agent_settings: { ...current.agent_settings, model: null, tier },
          }
        : current
    );
    try {
      const updated = await workflowsApi.update(workflow.id, { steps });
      setDetailWorkflow(current => current?.id === workflow.id ? updated : current);
      void refetch();
      toastProp?.(
        t('wf.stepAgentChanged', step.name, AGENT_LABELS[agent] ?? agent),
        'success',
      );
    } catch (error) {
      toastProp?.(userError(error), 'error');
      throw error;
    }
  };

  // Cross-page navigation: when the sidebar's batch pastille is clicked, the
  // Dashboard passes the parent workflow id here. We auto-switch to the
  // "workflows" sub-tab, open its detail panel, then ack so the same click
  // doesn't re-fire on every render.
  useEffect(() => {
    if (!initialSelectedWorkflowId) return;
    const timeoutId = window.setTimeout(() => {
      setTab('workflows');
      void openDetail(initialSelectedWorkflowId, initialSelectedWorkflowRunId ?? undefined);
      onInitialSelectionConsumed?.();
    }, 0);
    return () => {
      window.clearTimeout(timeoutId);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialSelectedWorkflowId, initialSelectedWorkflowRunId]);

  // 0.8.2 — Live workflow-run updates. The SSE stream is tab-local: if the
  // user opens the workflow detail in a *different* tab while a run is in
  // flight (or after a hard refresh on a paused Gate), liveRun is null and
  // the page never updates. WorkflowRunUpdated is broadcast on every
  // status flip + step transition; we mirror it into detailRuns so the
  // Gate appears live without the user having to F5.
  useWebSocket((msg) => {
    if (msg.type !== 'workflow_run_updated') return;
    if (!detailWorkflow || msg.workflow_id !== detailWorkflow.id) return;
    // During our own SSE-driven run, the blue live view already streams the
    // in-flight step. We STILL mirror WorkflowRunUpdated into the run list so
    // per-step statuses and the in-progress history card refresh without an
    // F5 — but THROTTLED (≤ 1×/3.5s) so the streaming <pre> never flickers.
    const activeLocalRun = liveRun && liveRun.workflowId === detailWorkflow.id && !liveRun.finished;
    if (activeLocalRun) {
      const nowTs = Date.now();
      if (nowTs - lastRunsRefetchRef.current < 3500) return;
      lastRunsRefetchRef.current = nowTs;
    }
    fetchRunPage(
      detailWorkflow.id,
      0,
      Math.max(RUN_FETCH_PAGE_SIZE, detailRunPageCount),
    ).then(page => {
      setDetailRuns(page.runs);
      setDetailRunPageCount(page.runs.length);
      setDetailRunTotal(page.total);
      setHasMoreDetailRuns(page.hasMore);
    }).catch(() => {});
  });

  const loadMoreDetailRuns = async (amount: number | 'all') => {
    if (!detailWorkflow || loadingMoreRunsRef.current) return;
    loadingMoreRunsRef.current = true;
    setLoadingMoreRuns(true);
    try {
      let nextRuns: WorkflowRun[] = [];
      let nextOffset = detailRunPageCount;
      let total = detailRunTotal;
      if (amount === 'all') {
        total = await workflowsApi.countRuns(detailWorkflow.id);
        while (nextOffset < total) {
          const page = await workflowsApi.listRuns(
            detailWorkflow.id,
            Math.min(RUN_FETCH_MAX_PAGE_SIZE, total - nextOffset),
            nextOffset,
            true,
          );
          if (page.length === 0) break;
          nextRuns = [...nextRuns, ...page];
          nextOffset += page.length;
        }
      } else {
        const page = await fetchRunPage(detailWorkflow.id, nextOffset, amount);
        nextRuns = page.runs;
        nextOffset += page.runs.length;
        total = page.total;
      }
      setDetailRuns(current => {
        const seen = new Set(current.map(run => run.id));
        return [...current, ...nextRuns.filter(run => !seen.has(run.id))];
      });
      setDetailRunPageCount(nextOffset);
      setDetailRunTotal(total);
      setHasMoreDetailRuns(nextOffset < total);
    } catch (e) {
      console.warn('Workflow run history load failed:', e);
    } finally {
      loadingMoreRunsRef.current = false;
      setLoadingMoreRuns(false);
    }
  };

  /** 0.6.0 UX pass — actually fire the trigger (with optional variables).
   *  Split out so handleTrigger can intercept and show the launch modal
   *  first when a workflow declares manual variables. */
  const fireTrigger = async (id: string, variables?: Record<string, string>) => {
    setTriggering(id);
    // Re-acquire the one-launch-at-a-time guard — the modal path released it
    // so the user could cancel, and its submit calls fireTrigger directly.
    triggeringRef.current = true;

    // Stop watching any previous launch's stream before resetting liveRun.
    abortRef.current?.abort();
    const gen = ++launchGenRef.current;
    const isCurrent = () => launchGenRef.current === gen;

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
      if (!chunks || !isCurrent()) return;
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
        if (!isCurrent()) return;
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
        if (!isCurrent()) return;
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
        if (!isCurrent()) return;
        if (rafId !== null) { cancelAnimationFrame(rafId); rafId = null; }
        pendingChunks = '';
        setLiveRun(prev => prev ? { ...prev, finished: true, status: data.status, currentStep: null } : prev);
        triggeringRef.current = false;
        setTriggering(null);
        refetch();
        if (selectedId === id) openDetail(id);
      },
      (error) => {
        if (!isCurrent()) return;
        if (rafId !== null) { cancelAnimationFrame(rafId); rafId = null; }
        console.warn('Workflow trigger error:', error);
        // Toast, because the card path renders nothing for a finished-failed
        // liveRun — the failure would otherwise be invisible.
        if (toastProp) toastProp(t('wf.triggerFailed', error), 'error');
        setLiveRun(prev => prev ? { ...prev, finished: true, status: 'Failed', currentStep: null } : prev);
        triggeringRef.current = false;
        setTriggering(null);
        // A run row may still have been created before the error — show it.
        refetch();
        if (selectedId === id) openDetail(id);
      },
      abort.signal,
      variables,
      // Live progress — accumulate into a coalescing buffer; flush on
      // the next animation frame. The workflow live view reads
      // `currentStepText` to render the streaming agent output for the
      // in-flight step.
      (text) => {
        if (!isCurrent()) return;
        pendingChunks += text;
        if (rafId === null) {
          rafId = requestAnimationFrame(flushChunks);
        }
      },
      // run_start carries the backend-minted run_id — needed for the
      // Stop button to call cancelRun(workflowId, runId). Without it,
      // the live view can't address the run it's watching.
      (runId) => {
        if (!isCurrent()) return;
        setLiveRun(prev => prev ? { ...prev, runId } : prev);
      },
    );
  };

  /** 0.6.0 UX pass — entry point for "Lancer". 0.8.1: also auto-detects
   *  unbound `{{var}}` references in step templates (`mergeDeclaredAndDetected`)
   *  so a workflow with `{{issue}}` in its prompt but no `variables[]`
   *  declared still gets the launch modal — closes the "autoBot
   *  {{issue}} unset" bug. Declared variables (with descriptions,
   *  defaults, kinds) win over auto-detected synthetics. */
  const handleTrigger = async (id: string) => {
    if (triggeringRef.current) return;
    triggeringRef.current = true;
    let wf: Workflow;
    try {
      wf = await workflowsApi.get(id);
    } catch (e) {
      console.warn('Failed to fetch workflow before trigger:', e);
      // Fallback: launch without variables — backend rejects loudly if required vars missing.
      await fireTrigger(id);
      // `fireTrigger` clears the ref in its own finally branch.
      return;
    }
    const vars = mergeDeclaredAndDetected(wf);
    if (vars.length === 0) {
      await fireTrigger(id);
      return;
    }
    // Variables modal opens — release the guard so the user can cancel
    // and try again. The launch modal has its own `submitting` flag.
    triggeringRef.current = false;
    // Open the launch modal with empty values; user fills + submits.
    // The synthetic workflow object carries the merged variables list
    // (declared + auto-detected) so the existing modal JSX picks them
    // up without any per-variable special-casing.
    setLaunchingWorkflow({
      workflow: { ...wf, variables: vars },
      values: Object.fromEntries(vars.map(v => [v.name, ''])),
      submitting: false,
      error: null,
    });
  };

  // Race-free guard, cf launchingRef — a double-click inside the in-flight
  // window fires duplicate updates computed from the same stale summary.
  // Shared by both card toggles (enabled + pinned), keyed per workflow.
  const toggleInFlightRef = useRef<Set<string>>(new Set());
  const toggleWorkflowField = async (wf: WorkflowSummary, patch: { enabled?: boolean; pinned?: boolean }) => {
    if (toggleInFlightRef.current.has(wf.id)) return;
    toggleInFlightRef.current.add(wf.id);
    try {
      await workflowsApi.update(wf.id, patch);
      refetch();
    } catch (e) {
      console.warn('Workflow action failed:', e);
    } finally {
      toggleInFlightRef.current.delete(wf.id);
    }
  };
  const handleToggle = (wf: WorkflowSummary) => toggleWorkflowField(wf, { enabled: !wf.enabled });
  const handleTogglePin = (wf: WorkflowSummary) => toggleWorkflowField(wf, { pinned: !wf.pinned });

  // Favorite mutations are metadata-only PATCHes. Keeping them out of the
  // normal editors avoids rewriting a resource and, for QPs, avoids creating
  // a fake prompt revision merely because the user clicked a star.
  const quickFavoriteInFlightRef = useRef<Set<string>>(new Set());
  const toggleQuickFavorite = async (
    kind: Exclude<AutomationTab, 'workflows'>,
    id: string,
    pinned: boolean,
    mutate: (resourceId: string, nextPinned: boolean) => Promise<unknown>,
    refresh: () => void,
  ) => {
    const key = `${kind}:${id}`;
    if (quickFavoriteInFlightRef.current.has(key)) return;
    quickFavoriteInFlightRef.current.add(key);
    try {
      await mutate(id, !pinned);
      refresh();
    } catch (error) {
      toastProp?.(userError(error), 'error');
    } finally {
      quickFavoriteInFlightRef.current.delete(key);
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
    // Pre-fix the workflow card's red trash button fired delete with no
    // confirmation — one mis-click destroyed the workflow + every run +
    // every child discussion. Now an explicit confirm is required.
    if (!confirm(t('wf.deleteWorkflowConfirm'))) return;
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
    const targetId = editingQP?.id ?? (!showCreateQP ? selectedQuickPromptId : null);
    const saved = targetId
      ? await quickPromptsApi.update(targetId, req)
      : await quickPromptsApi.create(req);
    setShowCreateQP(false);
    setEditingQP(null);
    if (saved?.id) setSelectedQuickPromptId(saved.id);
    refetchQP();
  };

  const changeQuickPromptAgent = async (qp: QuickPrompt, agent: AgentType, tier: ModelTier) => {
    if (qp.agent === agent && qp.tier === tier) return;
    try {
      await quickPromptsApi.update(qp.id, {
        name: qp.name,
        icon: qp.icon,
        prompt_template: qp.prompt_template,
        variables: qp.variables,
        agent,
        project_id: qp.project_id,
        skill_ids: qp.skill_ids,
        profile_ids: qp.profile_ids,
        directive_ids: qp.directive_ids,
        tier,
        // The explicit matrix choice must win over an expert model override.
        // Clearing it also prevents carrying a provider-specific model to a
        // different agent (for example Claude's "opus" to Codex).
        agent_settings: qp.agent_settings
          ? { ...qp.agent_settings, model: null, tier }
          : { model: null, tier },
        description: qp.description,
      });
      refetchQP();
      toastProp?.(
        t('qp.agentChanged', qp.name, AGENT_LABELS[agent] ?? agent),
        'success',
      );
    } catch (error) {
      toastProp?.(userError(error), 'error');
      throw error;
    }
  };

  const handleSaveQA = async (req: CreateQuickApiRequest) => {
    const targetId = editingQA?.id ?? (!showCreateQA ? selectedQuickApiId : null);
    const saved = targetId
      ? await quickApisApi.update(targetId, req)
      : await quickApisApi.create(req);
    setShowCreateQA(false);
    setEditingQA(null);
    if (saved?.id) setSelectedQuickApiId(saved.id);
    refetchQA();
  };

  const handleSaveQE = async (request: CreateQuickExecRequest) => {
    const targetId = editingQE?.id ?? (!showCreateQE ? selectedQuickExecId : null);
    const saved = targetId
      ? await quickExecsApi.update(targetId, request)
      : await quickExecsApi.create(request);
    setShowCreateQE(false);
    setEditingQE(null);
    if (saved?.id) setSelectedQuickExecId(saved.id);
    refetchQE();
  };

  const handleRunQE = async (quickExec: QuickExec) => {
    if (runQEState.busy) return;
    setRunQEState({ busy: true, data: null, error: null });
    try {
      const response = await quickExecsApi.run(quickExec.id, { variables: runVarsQE });
      setRunQEState({
        busy: false,
        data: response.data ?? null,
        error: response.success ? null : response.error ?? t('qe.runFailed'),
      });
    } catch (error) {
      setRunQEState({ busy: false, data: null, error: userError(error) });
    }
  };

  // Direct-run a saved Quick API. Variables are user-supplied via the
  // launch modal; the backend builds an ephemeral ApiCall step from the
  // QA config + variables and calls the same executor a workflow uses.
  // Result panel shows envelope.data on success, the error string on fail.
  const handleLaunchQA = async (qa: QuickApi) => {
    if (launchingQARunRef.current) return;
    launchingQARunRef.current = true;
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
      launchingQARunRef.current = false;
      setLaunchingQARun(false);
    }
  };

  const handleBatchLaunchQA = async (qa: QuickApi) => {
    if (batchQARunningRef.current) return;
    const parsed = parseBatchQAItems({ varCount: qa.variables.length }, batchQAInput);
    if ('errorKey' in parsed) {
      const errorMsg = parsed.errorArg
        ? t(parsed.errorKey).replace('{0}', parsed.errorArg)
        : t(parsed.errorKey);
      setBatchQAResult({ status: 'ERROR', items: [{ input: '', status: 'ERROR', error: errorMsg }], total: 0, succeeded: 0, failed: 1 });
      return;
    }
    batchQARunningRef.current = true;
    setBatchQARunning(true);
    setBatchQAResult(null);
    setExpandedQARows(new Set());
    try {
      const res = await quickApisApi.batchRunQa(qa.id, {
        items: parsed.items as JsonValue[],
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
      batchQARunningRef.current = false;
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

  /**
   * 0.8.5 follow-up — validate that every variable flagged as required
   * has a non-empty value before firing a launch / compare-agents.
   * Pre-fix the buttons fired regardless, sending the agent a prompt
   * full of empty placeholders. Returns the list of missing variable
   * labels (or names if no label is set) so the caller can render a
   * single localized toast. Empty list = OK to proceed.
   */
  const collectMissingRequiredVars = (qp: QuickPrompt, vars: Record<string, string>): string[] => {
    return qp.variables
      // `required` defaults to true (legacy QPs); only explicitly false skips validation.
      .filter(v => v.required !== false && !(vars[v.name] ?? '').trim())
      .map(v => v.label || v.name);
  };

  const handleLaunchQP = async (qp: QuickPrompt) => {
    if (launchingRef.current) return;
    // 0.8.5 — block launches that leave required variables empty. The
    // QP form's `required` checkbox is the source of truth; this is
    // the launcher mirror so the agent never receives a half-rendered
    // template.
    const missing = collectMissingRequiredVars(qp, launchVars);
    if (missing.length > 0) {
      if (toastProp) toastProp(t('qp.launch.missingRequired', missing.join(', ')), 'error');
      return;
    }
    launchingRef.current = true;
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
        // 0.8.5 — propagate QP-level persona + directive bindings to the
        // freshly spawned discussion so the agent is configured the same
        // way every time this QP is launched.
        profile_ids: qp.profile_ids?.length ? qp.profile_ids : undefined,
        directive_ids: qp.directive_ids?.length ? qp.directive_ids : undefined,
        // 0.8.5 — lineage: the QP-metrics aggregator GROUPs BY this column
        // to compute avg tokens / duration / cost per version_index.
        // Backend resolves the version itself (server-side source of truth).
        originating_qp_id: qp.id,
        tier: qp.tier !== 'default' ? qp.tier : undefined,
      });
      setLaunchingQP(null);
      setLaunchVars({});
      onNavigateDiscussion?.(disc.id);
    } catch (e) {
      console.warn('Launch failed:', e);
    } finally {
      launchingRef.current = false;
      setLaunching(false);
    }
  };

  /**
   * 0.8.5 — Spawn an AI-improver discussion for the given QP. The
   * agent receives the canonical QP body (id + name + template + vars
   * + bindings) in the initial prompt and the `qp-improver` skill in
   * its context. It audits the prompt and emits a refactored version
   * tagged with `KRONN:QP_IMPROVED` — the DiscussionsPage banner
   * parses the JSON and offers a one-click deploy back to PUT
   * `/api/quick-prompts/:id`.
   *
   * The discussion title carries the QP id under the bracketed prefix
   * `[Improve QP <id>]` so the banner can recover the target id from
   * the discussion alone (the QP id is not re-emitted by the agent).
   */
  const handleImproveQP = async (qp: QuickPrompt) => {
    try {
      const payload = {
        id: qp.id,
        name: qp.name,
        icon: qp.icon,
        prompt_template: qp.prompt_template,
        variables: qp.variables,
        agent: qp.agent,
        project_id: qp.project_id ?? null,
        skill_ids: qp.skill_ids ?? [],
        profile_ids: qp.profile_ids ?? [],
        directive_ids: qp.directive_ids ?? [],
        tier: qp.tier ?? 'default',
        description: qp.description ?? '',
      };
      // 0.8.5 follow-up — feed the agent the actual catalog of
      // skills / profiles / directives so it can RECOMMEND additions
      // (not just preserve what's already on the QP). Without this
      // section the skill's anti-hallucination rule forces empty
      // arrays, which surprised the user on the first run. The
      // catalog is rendered compact (id + short description) — ~30
      // lines total for a typical Kronn install, negligible token cost.
      const renderCatalog = <T extends { id: string; description?: string; name?: string }>(
        items: T[] | undefined | null,
        label: string,
        emptyHint: string,
      ): string => {
        if (!items || items.length === 0) return `### ${label}\n${emptyHint}\n`;
        const lines = items
          .map(it => `- \`${it.id}\` — ${(it.description || it.name || '').replace(/\s+/g, ' ').slice(0, 120)}`)
          .join('\n');
        return `### ${label} (${items.length})\n${lines}\n`;
      };
      const profileLine = (p: { id: string; persona_name?: string; role?: string }): string => {
        const desc = [p.persona_name, p.role].filter(Boolean).join(' · ').slice(0, 120);
        return `- \`${p.id}\` — ${desc || p.id}`;
      };
      const profilesBlock = !profilesCatalog || profilesCatalog.length === 0
        ? `### ${t('qp.improveCatalogProfiles')}\n${t('qp.improveCatalogEmpty')}\n`
        : `### ${t('qp.improveCatalogProfiles')} (${profilesCatalog.length})\n${profilesCatalog.map(profileLine).join('\n')}\n`;
      const catalog = [
        renderCatalog(skillsCatalog, t('qp.improveCatalogSkills'), t('qp.improveCatalogEmpty')),
        profilesBlock,
        renderCatalog(directivesCatalog, t('qp.improveCatalogDirectives'), t('qp.improveCatalogEmpty')),
      ].join('\n');
      // 0.8.5 follow-up — wrap the heavy technical seed (QP JSON +
      // catalog + audit protocol reminder) in the KRONN_SEED markers
      // so MessageBubble can collapse it behind a toggle. The user
      // sees a short status line (`✨ Audit du QP en cours…`); the
      // agent still reads the entire message verbatim.
      const visibleIntro = t('qp.improveVisibleIntro', qp.name);
      const seedBody =
        `${t('qp.improveInitialPrompt', qp.name)}\n\n` +
        '```json\n' + JSON.stringify(payload, null, 2) + '\n```\n\n' +
        `## ${t('qp.improveCatalogHeader')}\n` +
        `${t('qp.improveCatalogIntro')}\n\n` +
        catalog;
      const initialPrompt = `${visibleIntro}\n<!--KRONN_SEED_START-->\n${seedBody}\n<!--KRONN_SEED_END-->`;
      const disc = await discussionsApi.create({
        project_id: qp.project_id ?? null,
        title: `[Improve QP ${qp.id}] ${qp.name}`,
        agent: qp.agent,
        language: configLanguage || 'fr',
        initial_prompt: initialPrompt,
        // Pin the qp-improver skill so the agent reads the audit
        // protocol every time, even if no other context is wired in.
        skill_ids: ['qp-improver'],
      });
      onNavigateDiscussion?.(disc.id);
    } catch (e) {
      console.warn('QP improve failed:', e);
      if (toastProp) toastProp(userError(e), 'error');
    }
  };

  /**
   * Compare launch — fans the same rendered prompt across explicit
   * agent+model-tier targets in parallel. Reuses
   * `quickPromptsApi.compareAgents` which delegates to the
   * backend's `create_batch_run` with per-item `agent_override`.
   *
   * Side effect: Dashboard opens the dedicated comparison workspace while
   * the durable child discussions stream independently behind each column.
   */
  const handleCompareAgents = async (qp: QuickPrompt) => {
    if (launchingRef.current) return;
    // 0.8.5 — same required-vars guard as `handleLaunchQP`. Compare
    // is a worse offender than a single launch because the empty
    // template gets fanned-out across N agents (N × wasted runs).
    const missing = collectMissingRequiredVars(qp, launchVars);
    if (missing.length > 0) {
      if (toastProp) toastProp(t('qp.launch.missingRequired', missing.join(', ')), 'error');
      return;
    }
    // A fresh form compares every installed agent at the QP's configured
    // tier. Once edited, every row carries its own agent+tier selection.
    const targets = compareTargets ?? (installedAgentTypes ?? []).map(agent => ({
      agent,
      tier: qp.tier ?? 'default',
    }));
    if (targets.length === 0) {
      console.warn('Compare needs at least 1 selected agent/model target');
      return;
    }
    launchingRef.current = true;
    setLaunching(true);
    try {
      const rendered = renderTemplate(qp.prompt_template, launchVars);
      const firstVal = qp.variables.map(v => launchVars[v.name]).find(v => v?.trim());
      const stamp = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
      const batchName = firstVal
        ? `🤝 ${qp.name} — ${firstVal} — ${stamp}`
        : `🤝 ${qp.name} — ${stamp}`;
      const result = await quickPromptsApi.compareAgents(qp.id, {
        prompt: rendered,
        batch_name: batchName,
        targets,
        project_id: qp.project_id ?? undefined,
      });
      setLaunchingQP(null);
      setLaunchVars({});
      setCompareTargets(null);
      // Kick off each child discussion's agent run in parallel — same
      // pattern as `handleBatchLaunch` (cf. comments there for the
      // connection-pool + tokio detached-task gotchas). Without this
      // fan-out, only the navigated disc auto-runs and the siblings
      // sit dormant forever — the user-reported bug "only the first
      // agent works" surfaces here.
      result.discussion_ids.forEach(discId => {
        const controller = new AbortController();
        fetch(`/api/discussions/${discId}/run`, {
          method: 'POST',
          signal: controller.signal,
          keepalive: true,
        }).catch(() => { /* aborted or net blip — backend run is detached */ });
        setTimeout(() => controller.abort(), 500);
      });
      // Use `onBatchLaunched` (NOT `onNavigateDiscussion`) so the
      // sibling spinners light up in the sidebar AND we don't re-fire
      // a second `/run` on top of the fan-out we just dispatched.
      onBatchLaunched?.(result.discussion_ids, result.run_id, 'compare');
    } catch (e) {
      console.warn('Compare-agents launch failed:', e);
    } finally {
      launchingRef.current = false;
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
    if (batchLaunchingRef.current) return;
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
    // Fallback when `toast` prop isn't supplied (e.g. tests rendering this
     // page in isolation). `alert()` blocks the main thread, breaks E2E
     // runs, and surfaces a native modal that isn't styled — `console.warn`
     // is enough for the rare path where the parent forgot to wire the toast.
    const notify = toastProp ?? ((msg: string) => { console.warn('[Workflows]', msg); });
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

    batchLaunchingRef.current = true;
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
      batchLaunchingRef.current = false;
      setBatchLaunching(false);
    }
  };

  const aiCreation = tab === 'workflows'
    ? {
        title: t('wf.aiArchitectTitle'),
        prompt: t('wf.aiArchitectPrompt'),
        hint: t('wf.createWithAIHint'),
        skillIds: ['workflow-architect'],
      }
    : tab === 'quickPrompts'
      ? {
          title: t('qp.aiArchitectTitle'),
          prompt: t('qp.aiArchitectPrompt'),
          hint: t('qp.createWithAIHint'),
          skillIds: ['qp-improver'],
        }
      : tab === 'quickApis' ? {
          title: t('qa.aiArchitectTitle'),
          prompt: t('qa.aiArchitectPrompt'),
          hint: t('qa.createWithAIHint'),
          skillIds: [],
        }
      : {
          title: t('qe.aiArchitectTitle'),
          prompt: t('qe.aiArchitectPrompt'),
          hint: t('qe.createWithAIHint'),
          skillIds: ['workflow-architect'],
        };

  const handleCreateWithAI = async () => {
    if (!onNavigateDiscussion) return;
    try {
      // Compact project lookup: enough context to attach the draft without
      // spending a tool round-trip or filling the agent context needlessly.
      const shown = projects.slice(0, 20);
      const projectContext = shown.length > 0
        ? '\n\n---\nProjets[name=id]: ' + shown.map(p => `${p.name}=${p.id}`).join(' | ')
          + (projects.length > 20 ? ` (+${projects.length - 20})` : '')
        : '';
      const disc = await discussionsApi.create({
        project_id: null,
        title: aiCreation.title,
        agent: 'ClaudeCode',
        language: configLanguage || 'fr',
        initial_prompt: aiCreation.prompt + projectContext,
        skill_ids: aiCreation.skillIds,
        profile_ids: [],
        directive_ids: [],
        tier: 'reasoning',
      });
      onNavigateDiscussion(disc.id);
    } catch (e) {
      console.warn('Failed to create AI discussion:', e);
    }
  };

  const clearAutomationEditors = () => {
    setShowCreate(false);
    setEditingWorkflow(null);
    setShowCreateQP(false);
    setEditingQP(null);
    setShowCreateQA(false);
    setEditingQA(null);
    setShowCreateQE(false);
    setEditingQE(null);
  };

  const selectAutomationKind = (kind: AutomationTab) => {
    clearAutomationEditors();
    setTab(kind);
    setCollapsedAutomationSections(current => {
      if (automationQuery.trim()) return current;
      const next = new Set(current);
      if (tab === kind) {
        if (next.has(kind)) next.delete(kind);
        else next.add(kind);
      } else {
        next.delete(kind);
      }
      return next;
    });
    if (kind === 'workflows') {
      setSelectedId(null);
      setDetailWorkflow(null);
    } else if (kind === 'quickApis') {
      setSelectedQuickApiId(null);
    } else if (kind === 'quickPrompts') {
      setSelectedQuickPromptId(null);
    } else {
      setSelectedQuickExecId(null);
    }
  };

  const isAutomationSectionCollapsed = (kind: AutomationSection) => (
    !automationQuery.trim() && collapsedAutomationSections.has(kind)
  );

  const toggleFavoritesSection = () => {
    if (automationQuery.trim()) return;
    setCollapsedAutomationSections(current => {
      const next = new Set(current);
      if (next.has('favorites')) next.delete('favorites');
      else next.add('favorites');
      return next;
    });
  };

  const openQuickPrompt = (quickPrompt: QuickPrompt) => {
    clearAutomationEditors();
    setTab('quickPrompts');
    setSelectedQuickPromptId(quickPrompt.id);
  };
  const openQuickApi = (quickApi: QuickApi) => {
    clearAutomationEditors();
    setTab('quickApis');
    setSelectedQuickApiId(quickApi.id);
  };
  const openQuickExec = (quickExec: QuickExec) => {
    clearAutomationEditors();
    setTab('quickExecs');
    setSelectedQuickExecId(quickExec.id);
  };

  const totalAutomationResources = workflows.length
    + (quickApiList?.length ?? 0)
    + (quickPromptList?.length ?? 0)
    + (quickExecList?.length ?? 0);
  const automationHasSelection = Boolean(
    selectedId || selectedQuickApiId || selectedQuickPromptId || selectedQuickExecId
      || showCreate || editingWorkflow || showCreateQA || editingQA
      || showCreateQP || editingQP || showCreateQE || editingQE,
  );
  const currentResourceSelected = Boolean(
    selectedId || selectedQuickApi || selectedQuickPrompt || selectedQuickExec,
  );
  const automationContentTitle = detailWorkflow?.name
    ?? selectedQuickApi?.name
    ?? selectedQuickPrompt?.name
    ?? selectedQuickExec?.name
    ?? (tab === 'workflows' ? t('wf.tabWorkflows')
      : tab === 'quickApis' ? t('wf.tabQuickApis')
        : tab === 'quickPrompts' ? t('wf.tabQuickPrompts')
          : t('wf.tabQuickExecs'));

  return (
    <div className="automation-page" data-has-selection={automationHasSelection}>
      <aside
        className="disc-sidebar automation-sidebar"
        aria-label={t('wf.title')}
        data-tour-id="automation-library"
      >
        <div className="disc-sidebar-header">
          <span className="disc-sidebar-header-title">
            {t('wf.title')}
            <span className="disc-sidebar-header-count">{' · '}{totalAutomationResources}</span>
          </span>
          <ContextHelp title={t('contextHelp.automation.title')}>
            <p>{t('contextHelp.automation.intro')}</p>
            <ul>
              <li>{t('contextHelp.automation.wf')}</li>
              <li>{t('contextHelp.automation.qa')}</li>
              <li>{t('contextHelp.automation.qp')}</li>
              <li>{t('contextHelp.automation.qe')}</li>
            </ul>
            <p className="kr-context-help-agent-note">{t('contextHelp.automation.mcp')}</p>
          </ContextHelp>
        </div>

        <div className="disc-search-wrap automation-sidebar-search">
          <div className="disc-search-controls">
            <label className="disc-search-box">
              <Search size={14} className="disc-search-icon" />
              <input
                ref={automationSearchRef}
                className="disc-search-input"
                value={automationQuery}
                onChange={event => setAutomationQuery(event.target.value)}
                placeholder={t('automation.search')}
                aria-label={t('automation.search')}
              />
              {automationQuery && (
                <button
                  type="button"
                  className="disc-search-clear"
                  onClick={() => setAutomationQuery('')}
                  aria-label={t('automation.clearSearch')}
                >
                  <X size={12} />
                </button>
              )}
            </label>
          </div>
          <select
            className="automation-project-filter"
            value={automationProjectFilter}
            onChange={event => setAutomationProjectFilter(event.target.value)}
            aria-label={t('automation.projectFilter')}
          >
            <option value="all">{t('automation.allProjects')}</option>
            <option value="__global__">{t('disc.general')}</option>
            {projects.map(project => <option key={project.id} value={project.id}>{project.name}</option>)}
          </select>
        </div>

        <div className="disc-sidebar-list automation-sidebar-items" data-tour-id="automation-kinds">
          {sidebarFavoriteCount > 0 && (
            <div
              className="disc-sidebar-section automation-sidebar-section automation-favorites-section"
              data-expanded={!isAutomationSectionCollapsed('favorites')}
            >
              <button
                type="button"
                className="disc-group-btn automation-kind-button"
                onClick={toggleFavoritesSection}
                aria-expanded={!isAutomationSectionCollapsed('favorites')}
              >
                <ChevronRight size={10} className="disc-chevron" data-expanded={!isAutomationSectionCollapsed('favorites')} />
                <Star size={11} fill="currentColor" />
                <span>{t('disc.favorites')}</span>
                <span className="disc-group-count">({sidebarFavoriteCount})</span>
              </button>
              {!isAutomationSectionCollapsed('favorites') && sidebarWorkflows.filter(item => item.pinned).map(workflow => (
                <AutomationResourceRow
                  key={`favorite-workflow-${workflow.id}`}
                  name={workflow.name}
                  meta={`${t('wf.tabWorkflows')} · ${TRIGGER_LABELS[workflow.trigger_type] ?? workflow.trigger_type}`}
                  icon={workflow.enabled ? '⚡' : '○'}
                  active={tab === 'workflows' && selectedId === workflow.id}
                  pinned={workflow.pinned}
                  openLabel={t('automation.openResource', workflow.name)}
                  pinLabel={t('wf.pin')}
                  unpinLabel={t('wf.unpin')}
                  onOpen={() => { clearAutomationEditors(); setTab('workflows'); void openDetail(workflow.id); }}
                  onTogglePinned={() => { void handleTogglePin(workflow); }}
                />
              ))}
              {!isAutomationSectionCollapsed('favorites') && sidebarQuickApis.filter(item => item.pinned).map(quickApi => (
                <AutomationResourceRow
                  key={`favorite-quick-api-${quickApi.id}`}
                  name={quickApi.name}
                  meta={`${t('wf.tabQuickApis')} · ${quickApi.api_method ?? 'GET'}`}
                  icon={quickApi.icon}
                  active={tab === 'quickApis' && selectedQuickApiId === quickApi.id}
                  pinned={quickApi.pinned}
                  openLabel={t('automation.openResource', quickApi.name)}
                  pinLabel={t('wf.pin')}
                  unpinLabel={t('wf.unpin')}
                  onOpen={() => openQuickApi(quickApi)}
                  onTogglePinned={() => { void toggleQuickFavorite('quickApis', quickApi.id, quickApi.pinned, quickApisApi.setPinned, refetchQA); }}
                />
              ))}
              {!isAutomationSectionCollapsed('favorites') && sidebarQuickPrompts.filter(item => item.pinned).map(quickPrompt => (
                <AutomationResourceRow
                  key={`favorite-quick-prompt-${quickPrompt.id}`}
                  name={quickPrompt.name}
                  meta={`${t('wf.tabQuickPrompts')} · ${AGENT_LABELS[quickPrompt.agent] ?? quickPrompt.agent}`}
                  icon={quickPrompt.icon}
                  active={tab === 'quickPrompts' && selectedQuickPromptId === quickPrompt.id}
                  pinned={quickPrompt.pinned}
                  openLabel={t('automation.openResource', quickPrompt.name)}
                  pinLabel={t('wf.pin')}
                  unpinLabel={t('wf.unpin')}
                  onOpen={() => openQuickPrompt(quickPrompt)}
                  onTogglePinned={() => { void toggleQuickFavorite('quickPrompts', quickPrompt.id, quickPrompt.pinned, quickPromptsApi.setPinned, refetchQP); }}
                />
              ))}
              {!isAutomationSectionCollapsed('favorites') && sidebarQuickExecs.filter(item => item.pinned).map(quickExec => (
                <AutomationResourceRow
                  key={`favorite-quick-exec-${quickExec.id}`}
                  name={quickExec.name}
                  meta={`${t('wf.tabQuickExecs')} · ${quickExec.output_format.toUpperCase()}`}
                  icon={quickExec.icon}
                  active={tab === 'quickExecs' && selectedQuickExecId === quickExec.id}
                  pinned={quickExec.pinned}
                  openLabel={t('automation.openResource', quickExec.name)}
                  pinLabel={t('wf.pin')}
                  unpinLabel={t('wf.unpin')}
                  onOpen={() => openQuickExec(quickExec)}
                  onTogglePinned={() => { void toggleQuickFavorite('quickExecs', quickExec.id, quickExec.pinned, quickExecsApi.setPinned, refetchQE); }}
                />
              ))}
            </div>
          )}

          <div className="disc-sidebar-section automation-sidebar-section" data-expanded={!isAutomationSectionCollapsed('workflows')}>
            <button
              type="button"
              className="disc-group-btn automation-kind-button"
              data-active={tab === 'workflows'}
              data-tour-id="automation-kind-workflow"
              onClick={() => selectAutomationKind('workflows')}
              aria-expanded={!isAutomationSectionCollapsed('workflows')}
            >
              <ChevronRight size={10} className="disc-chevron" data-expanded={!isAutomationSectionCollapsed('workflows')} />
              <WorkflowIcon size={11} />
              <span>{t('wf.tabWorkflows')}</span>
              <span className="disc-group-count">({sidebarWorkflows.length})</span>
            </button>
            {!isAutomationSectionCollapsed('workflows') && sidebarWorkflows.map(workflow => (
              <AutomationResourceRow
                key={workflow.id}
                name={workflow.name}
                meta={`${TRIGGER_LABELS[workflow.trigger_type] ?? workflow.trigger_type} · ${workflow.step_count} step${workflow.step_count > 1 ? 's' : ''}`}
                icon={workflow.enabled ? '⚡' : '○'}
                active={tab === 'workflows' && selectedId === workflow.id}
                pinned={workflow.pinned}
                openLabel={t('automation.openResource', workflow.name)}
                pinLabel={t('wf.pin')}
                unpinLabel={t('wf.unpin')}
                onOpen={() => { clearAutomationEditors(); setTab('workflows'); void openDetail(workflow.id); }}
                onTogglePinned={() => { void handleTogglePin(workflow); }}
              />
            ))}
          </div>

          <div className="disc-sidebar-section automation-sidebar-section" data-expanded={!isAutomationSectionCollapsed('quickApis')}>
            <button
              type="button"
              className="disc-group-btn automation-kind-button"
              data-active={tab === 'quickApis'}
              data-tour-id="automation-kind-quick-api"
              onClick={() => selectAutomationKind('quickApis')}
              aria-expanded={!isAutomationSectionCollapsed('quickApis')}
            >
              <ChevronRight size={10} className="disc-chevron" data-expanded={!isAutomationSectionCollapsed('quickApis')} />
              <PlugZap size={11} />
              <span>{t('wf.tabQuickApis')}</span>
              <span className="disc-group-count">({sidebarQuickApis.length})</span>
            </button>
            {!isAutomationSectionCollapsed('quickApis') && sidebarQuickApis.map(quickApi => (
              <AutomationResourceRow
                key={quickApi.id}
                name={quickApi.name}
                meta={`${quickApi.api_method ?? 'GET'} · ${quickApi.api_endpoint_path}`}
                icon={quickApi.icon}
                active={tab === 'quickApis' && selectedQuickApiId === quickApi.id}
                pinned={quickApi.pinned}
                openLabel={t('automation.openResource', quickApi.name)}
                pinLabel={t('wf.pin')}
                unpinLabel={t('wf.unpin')}
                onOpen={() => openQuickApi(quickApi)}
                onTogglePinned={() => { void toggleQuickFavorite('quickApis', quickApi.id, quickApi.pinned, quickApisApi.setPinned, refetchQA); }}
              />
            ))}
          </div>

          <div className="disc-sidebar-section automation-sidebar-section" data-expanded={!isAutomationSectionCollapsed('quickPrompts')}>
            <button
              type="button"
              className="disc-group-btn automation-kind-button"
              data-active={tab === 'quickPrompts'}
              data-tour-id="automation-kind-quick-prompt"
              onClick={() => selectAutomationKind('quickPrompts')}
              aria-expanded={!isAutomationSectionCollapsed('quickPrompts')}
            >
              <ChevronRight size={10} className="disc-chevron" data-expanded={!isAutomationSectionCollapsed('quickPrompts')} />
              <MessageSquareText size={11} />
              <span>{t('wf.tabQuickPrompts')}</span>
              <span className="disc-group-count">({sidebarQuickPrompts.length})</span>
            </button>
            {!isAutomationSectionCollapsed('quickPrompts') && sidebarQuickPrompts.map(quickPrompt => (
              <AutomationResourceRow
                key={quickPrompt.id}
                name={quickPrompt.name}
                meta={AGENT_LABELS[quickPrompt.agent] ?? quickPrompt.agent}
                icon={quickPrompt.icon}
                active={tab === 'quickPrompts' && selectedQuickPromptId === quickPrompt.id}
                pinned={quickPrompt.pinned}
                openLabel={t('automation.openResource', quickPrompt.name)}
                pinLabel={t('wf.pin')}
                unpinLabel={t('wf.unpin')}
                onOpen={() => openQuickPrompt(quickPrompt)}
                onTogglePinned={() => { void toggleQuickFavorite('quickPrompts', quickPrompt.id, quickPrompt.pinned, quickPromptsApi.setPinned, refetchQP); }}
              />
            ))}
          </div>

          <div className="disc-sidebar-section automation-sidebar-section" data-expanded={!isAutomationSectionCollapsed('quickExecs')}>
            <button
              type="button"
              className="disc-group-btn automation-kind-button"
              data-active={tab === 'quickExecs'}
              data-tour-id="automation-kind-quick-exec"
              onClick={() => selectAutomationKind('quickExecs')}
              aria-expanded={!isAutomationSectionCollapsed('quickExecs')}
            >
              <ChevronRight size={10} className="disc-chevron" data-expanded={!isAutomationSectionCollapsed('quickExecs')} />
              <TerminalSquare size={11} />
              <span>{t('wf.tabQuickExecs')}</span>
              <span className="disc-group-count">({sidebarQuickExecs.length})</span>
            </button>
            {!isAutomationSectionCollapsed('quickExecs') && sidebarQuickExecs.map(quickExec => (
              <AutomationResourceRow
                key={quickExec.id}
                name={quickExec.name}
                meta={`${quickExec.command} · ${quickExec.output_format.toUpperCase()}`}
                icon={quickExec.icon}
                active={tab === 'quickExecs' && selectedQuickExecId === quickExec.id}
                pinned={quickExec.pinned}
                openLabel={t('automation.openResource', quickExec.name)}
                pinLabel={t('wf.pin')}
                unpinLabel={t('wf.unpin')}
                onOpen={() => openQuickExec(quickExec)}
                onTogglePinned={() => { void toggleQuickFavorite('quickExecs', quickExec.id, quickExec.pinned, quickExecsApi.setPinned, refetchQE); }}
              />
            ))}
          </div>

          {sidebarWorkflows.length + sidebarQuickApis.length + sidebarQuickPrompts.length + sidebarQuickExecs.length === 0 && (
            <div className="disc-empty">{t('automation.noSearchResults')}</div>
          )}
        </div>
        <div className="disc-sidebar-footer">
          <span>{t('automation.sidebarHint')}</span>
          <span><kbd>/</kbd> {t('automation.sidebarSearchHint')}</span>
        </div>
      </aside>

      <section className="automation-viewer">
      <div className="flex-between mb-4 automation-page-header">
        <div className="kr-context-help-title-row">
          <h1 className="wf-h1"><MatrixText text={automationContentTitle} /></h1>
        </div>
        <div className="flex-row gap-3 automation-header-actions" data-tour-id="automation-actions">
          {!currentResourceSelected && onNavigateDiscussion && (
            <button className="wf-create-ai-btn" data-tour-id="automation-ai-btn" title={aiCreation.hint} onClick={handleCreateWithAI}>
              <Zap size={14} /> {t('wf.createWithAI')}
            </button>
          )}
          <button
            className="wf-create-btn wf-create-btn-secondary"
            title={t('imp.globalHint')}
            onClick={() => setImporting({
              kind: null,
              content: '',
              preview: { name: '' },
              targetProjectId: '',
            })}
          >
            <Upload size={14} /> {t('wf.import')}
          </button>
          {!currentResourceSelected && tab === 'workflows' && (
            <button className="wf-create-btn" title={t('wf.newHint')} onClick={() => setShowCreate(true)}>
            <Plus size={14} /> {t('wf.new')}
            </button>
          )}
          {!currentResourceSelected && tab === 'quickPrompts' && (
            <button className="wf-create-btn" onClick={() => { setShowCreateQP(true); setEditingQP(null); }}>
              <Plus size={14} /> {t('qp.new')}
            </button>
          )}
          {!currentResourceSelected && tab === 'quickApis' && availableApiPlugins.length > 0 && (
            <button className="wf-create-btn" onClick={() => setShowCreateQA(true)}>
              <Plus size={14} /> {t('qa.new')}
            </button>
          )}
          {!currentResourceSelected && tab === 'quickExecs' && (
            <button className="wf-create-btn" onClick={() => { setShowCreateQE(true); setEditingQE(null); }}>
              <Plus size={14} /> {t('qe.new')}
            </button>
          )}
        </div>
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
          initialPresetId={pendingPresetLocal?.presetId}
          initialProjectId={pendingPresetLocal?.projectId}
          onNavigatePage={onNavigatePage}
          onDone={() => { setShowCreate(false); setPendingPresetLocal(null); refetch(); }}
          onCancel={() => { setShowCreate(false); setPendingPresetLocal(null); }}
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
          onNavigatePage={onNavigatePage}
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
        <div className="wf-workspace" data-mobile={isMobile} data-selected={Boolean(selectedId)}>
          {/* List — grouped by project */}
          {!selectedId && (
          <div className="wf-workflow-list-pane" data-testid="workflow-list-pane">
            {groupedWorkflows.map(group => (
              <div key={group.key} className="mb-6">
                {/* Group header */}
                <button
                  className="wf-group-header"
                  onClick={() => toggleGroup(group.key)}
                  aria-expanded={!collapsedGroups[group.key]}
                >
                  {collapsedGroups[group.key] ? <ChevronRight size={12} /> : <ChevronDown size={12} />}
                  {group.key === '__favorites__' && (
                    <Star size={11} style={{ color: 'var(--kr-warning)', flexShrink: 0 }} />
                  )}
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
                  const storedRunIsActive = lastStatus === 'Running' || lastStatus === 'Pending';
                  const runIdToCancel = live?.runId
                    ?? (storedRunIsActive ? wf.last_run?.id : undefined);
                  const cancelling = runIdToCancel
                    ? cancellingRunIds.has(runIdToCancel)
                    : false;
                  return (
                  <div
                    key={wf.id}
                    className="wf-card"
                    data-active={selectedId === wf.id}
                    data-running={isRunning}
                    onClick={() => openDetail(wf.id)}
                  >
                    <div className="wf-card-head">
                      <div className="wf-card-identity">
                        {isRunning && (
                          <Loader2
                            size={14}
                            className="wf-spin"
                            aria-label={t('wf.running')}
                          />
                        )}
                        <span className="wf-card-title">{wf.name}</span>
                      </div>
                      <div className="wf-card-controls">
                        <button
                          className="wf-icon-btn"
                          data-active={wf.pinned}
                          onClick={(e) => { e.stopPropagation(); handleTogglePin(wf); }}
                          title={wf.pinned ? t('wf.unpin') : t('wf.pin')}
                          aria-pressed={wf.pinned}
                          aria-label={wf.pinned ? t('wf.unpin') : t('wf.pin')}
                        >
                          <Star size={14} fill={wf.pinned ? 'currentColor' : 'none'} />
                        </button>
                        <button
                          className="wf-icon-btn wf-card-enabled-btn"
                          data-enabled={wf.enabled}
                          onClick={(e) => { e.stopPropagation(); handleToggle(wf); }}
                          title={wf.enabled ? t('wf.active') : t('wf.inactive')}
                          aria-pressed={wf.enabled}
                          aria-label={wf.enabled ? t('wf.active') : t('wf.inactive')}
                        >
                          {wf.enabled ? <ToggleRight size={18} /> : <ToggleLeft size={18} />}
                        </button>
                      </div>
                    </div>

                    <div className="wf-card-meta">
                      <span className="wf-trigger-badge" data-type={wf.trigger_type}>
                        {wf.trigger_type === 'cron' && <Clock size={10} />}
                        {wf.trigger_type === 'tracker' && <GitBranch size={10} />}
                        {wf.trigger_type === 'manual' && <Zap size={10} />}
                        {TRIGGER_LABELS[wf.trigger_type] ?? wf.trigger_type}
                      </span>
                      <span className="wf-card-step-count">
                        <Layers size={11} />
                        {wf.step_count} step{wf.step_count > 1 ? 's' : ''}
                      </span>
                      {wf.misconfigured_step_count > 0 && (
                        <span
                          className="wf-needs-config-badge"
                          title={t('wf.needsConfigTip')}
                        >
                          <AlertTriangle size={10} />
                          {t('wf.needsConfig').replace('{0}', String(wf.misconfigured_step_count))}
                        </span>
                      )}
                    </div>

                    <div className="wf-card-footer">
                      <div className="wf-card-run-summary">
                        {wf.last_run && (
                          <>
                            <span
                              className="wf-run-status-chip"
                              data-status={wf.last_run.status}
                              data-current="true"
                            >
                              {wf.last_run.status}
                            </span>
                            <span className="wf-card-run-date">
                              {new Date(wf.last_run.started_at).toLocaleString()}
                            </span>
                            {wf.last_run.tokens_used > 0 && (
                              <span className="wf-card-run-tokens">
                                {wf.last_run.tokens_used} tokens
                              </span>
                            )}
                          </>
                        )}
                      </div>
                      <div className="wf-card-actions">
                        {runIdToCancel && (
                          <button
                            className="wf-card-stop-btn"
                            onClick={(e) => {
                              e.stopPropagation();
                              void handleCancelRun(wf.id, runIdToCancel);
                            }}
                            disabled={cancelling}
                            title={t('wf.cancelRun')}
                            aria-label={t('wf.cancelRun')}
                          >
                            <Square size={9} fill="currentColor" />
                            {cancelling ? t('wf.cancelling') : t('wf.cancelRun')}
                          </button>
                        )}
                        <button
                          className="wf-card-run-btn"
                          onClick={(e) => { e.stopPropagation(); handleTrigger(wf.id); }}
                          disabled={!wf.enabled || triggering === wf.id}
                          title={!wf.enabled ? t('wf.launchDisabledHint') : undefined}
                        >
                          {triggering === wf.id ? <Loader2 size={11} className="spin" /> : <Play size={11} fill="currentColor" />}
                          {t('wf.trigger')}
                        </button>
                        <button
                          className="wf-card-delete-btn"
                          onClick={(e) => { e.stopPropagation(); handleDelete(wf.id); }}
                          title={t('wf.delete')}
                          aria-label={`${t('wf.delete')} ${wf.name}`}
                        >
                          <Trash2 size={12} />
                        </button>
                      </div>
                    </div>
                  </div>
                  );
                })}
              </div>
            ))}
          </div>
          )}

          {/* Detail panel — the shared Automation sidebar owns navigation. */}
          {selectedId && (
          <div
            ref={detailScrollRef}
            className="wf-workflow-detail-pane"
            data-testid="workflow-detail-pane"
          >
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
                key={detailWorkflow.id}
                workflow={detailWorkflow}
                runs={detailRuns}
                availableAgentTypes={installedAgentTypes}
                onChangeStepAgent={changeStepAgent}
                totalRuns={detailRunTotal}
                hasMoreRuns={hasMoreDetailRuns}
                loadingMoreRuns={loadingMoreRuns}
                onLoadMoreRuns={loadMoreDetailRuns}
                liveRun={liveRun?.workflowId === detailWorkflow.id ? liveRun : null}
                onTrigger={() => handleTrigger(detailWorkflow.id)}
                onRefresh={() => openDetail(
                  detailWorkflow.id,
                  undefined,
                  Math.max(RUN_FETCH_PAGE_SIZE, detailRunPageCount),
                )}
                onEdit={() => setEditingWorkflow(detailWorkflow)}
                projects={projects}
                configLanguage={configLanguage}
                onNavigatePage={onNavigatePage}
                onDeleteRun={async (runId) => {
                  if (!confirm(t('wf.deleteRunConfirm'))) return;
                  await workflowsApi.deleteRun(detailWorkflow.id, runId);
                  openDetail(detailWorkflow.id);
                  refetch();
                }}
                onDeleteAllRuns={async () => {
                  if (!confirm(t('wf.deleteAllRunsConfirm', detailRunTotal))) return;
                  await workflowsApi.deleteAllRuns(detailWorkflow.id);
                  openDetail(detailWorkflow.id);
                  refetch();
                }}
                triggering={triggering === detailWorkflow.id}
                agentAccess={agentAccess}
                onNavigateToBatch={onNavigateToBatch}
                onNavigateToWorkflow={(wfId) => openDetail(wfId)}
                onNavigateToRun={(wfId, runId) => openDetail(wfId, runId)}
                focusRunId={focusRunId}
                toast={toastProp}
                onToggleEnabled={async (enabled) => {
                  // 0.8.11 UX — one-click enable from the detail (a disabled
                  // workflow's launch button is inert; see WorkflowDetail).
                  try {
                    await workflowsApi.update(detailWorkflow.id, { enabled });
                    if (toastProp) toastProp(t(enabled ? 'wf.enabledToast' : 'wf.disabledToast'), 'success');
                    openDetail(detailWorkflow.id);
                    refetch();
                  } catch (e) {
                    if (toastProp) toastProp(String(e), 'error');
                  }
                }}
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

          </div>
          )}
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
              skills={skillsCatalog ?? []}
              profiles={profilesCatalog ?? []}
              directives={directivesCatalog ?? []}
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
                <>
                  {!selectedQuickPromptId && <div className="automation-list-toolbar">
                    <ListControls
                      filterLabel={t('automation.filter.label')}
                      filterAriaLabel={t('automation.filter.qpAgentLabel')}
                      filterValue={quickPromptAgentFilter}
                      filterOptions={[
                        { value: 'all', label: t('automation.filter.allAgents') },
                        ...quickPromptAgents.map(agent => ({
                          value: agent,
                          label: AGENT_LABELS[agent] ?? agent,
                        })),
                      ]}
                      onFilterChange={setQuickPromptAgentFilter}
                      sortLabel={t('automation.sort.label')}
                      sortAriaLabel={t('automation.sort.qpLabel')}
                      sortValue={quickPromptSort}
                      sortOptions={[
                        { value: 'name', label: t('automation.sort.name') },
                        { value: 'updated', label: t('automation.sort.updated') },
                        {
                          value: 'usage',
                          label: t(quickPromptUsageReady
                            ? 'automation.sort.usage'
                            : 'automation.sort.usageLoading'),
                          disabled: !quickPromptUsageReady,
                        },
                      ]}
                      onSortChange={setQuickPromptSort}
                      reversed={quickPromptSortReversed}
                      onToggleDirection={() => setQuickPromptSortReversed(value => !value)}
                      directionLabel={t(quickPromptSortReversed
                        ? 'automation.sort.restoreDirection'
                        : 'automation.sort.reverseDirection')}
                    />
                  </div>}
                  <div className="qp-list">
                  {visibleQuickPrompts.length === 0 && (
                    <div className="automation-filter-empty">{t('automation.filter.empty')}</div>
                  )}
                  {visibleQuickPrompts.map(qp => (
                    <Fragment key={qp.id}>
                    <div
                      className="qp-card"
                      data-kind="prompt"
                      data-detail={selectedQuickPromptId === qp.id}
                      data-qp-id={qp.id}
                    >
                      <div className="qp-card-header">
                        <div className="qp-card-identity">
                          <span className="qp-card-icon">{qp.icon}</span>
                          <span className="qp-card-name">{qp.name}</span>
                        </div>
                        <div className="qp-card-head-controls">
                          <button
                            className="wf-icon-btn qp-card-tool-btn"
                            data-active={qp.pinned}
                            onClick={() => { void toggleQuickFavorite('quickPrompts', qp.id, qp.pinned, quickPromptsApi.setPinned, refetchQP); }}
                            title={qp.pinned ? t('wf.unpin') : t('wf.pin')}
                            aria-pressed={qp.pinned}
                            aria-label={`${qp.pinned ? t('wf.unpin') : t('wf.pin')} · ${qp.name}`}
                          >
                            <Star size={14} fill={qp.pinned ? 'currentColor' : 'none'} />
                          </button>
                          <CopyIdPill id={qp.id} title={t('automation.copyId', qp.name)} />
                          <AgentSwitchPicker
                            currentAgent={qp.agent}
                            availableAgents={installedAgentTypes ?? []}
                            currentTier={qp.tier ?? 'default'}
                            onSelectionChange={(agent, tier) => changeQuickPromptAgent(qp, agent, tier)}
                            tierLabels={{
                              economy: t('disc.tier.economy'),
                              default: t('disc.tier.default'),
                              reasoning: t('disc.tier.reasoning'),
                            }}
                            modelTiers={agentAccess?.model_tiers}
                            defaultModelLabel={t('config.defaultModel')}
                            title={t('disc.switchAgentAndTier')}
                            ariaLabel={t('qp.agentSwitchLabel', qp.name, AGENT_LABELS[qp.agent] ?? qp.agent)}
                            staticClassName="qp-card-agent-static"
                          />
                        </div>
                      </div>

                      {qp.description && (
                        <p className="qp-card-desc">{qp.description}</p>
                      )}

                      <div className="qp-card-meta">
                        {qp.variables.length > 0 && (
                          <span className="qp-card-vars">{t('qp.vars', qp.variables.length)}</span>
                        )}
                        {/* 0.8.5 — fitness chip: current-version avg
                            tokens / duration + launch count. Hidden
                            until the QP has ≥ 1 launch. */}
                        <QPCardMetricsChip
                          qpId={qp.id}
                          onUsageLoaded={recordQuickPromptUsage}
                        />
                      </div>

                      <div className="qp-card-footer">
                        <div className="qp-card-tools">
                          {selectedQuickPromptId !== qp.id && <button
                            className="wf-icon-btn qp-card-tool-btn"
                            onClick={() => setEditingQP(qp)}
                            title={t('qp.edit')}
                            aria-label={`${t('qp.edit')} ${qp.name}`}
                          >
                            <Eye size={12} />
                          </button>}
                          {/* 0.8.5 — "Improve with AI" — opens a discussion
                              seeded with the QP body + the qp-improver
                              skill. The agent audits the prompt, emits a
                              refactored version, and a DiscussionsPage CTA
                              deploys it in one click via PUT
                              /api/quick-prompts/:id. */}
                          <button
                            className="wf-icon-btn qp-card-tool-btn"
                            data-testid="qp-improve-btn"
                            disabled={!installedAgentTypes || installedAgentTypes.length === 0}
                            onClick={() => handleImproveQP(qp)}
                            title={t('qp.improveWithAi')}
                            aria-label={`${t('qp.improveWithAi')} ${qp.name}`}
                          >
                            ✨
                          </button>
                          {/* 0.8.5 — version history + metrics drawer.
                              Reads /api/quick-prompts/:id/{history,metrics}.
                              Hides itself when the QP has no snapshot
                              (legacy pre-0.8.5 QPs). */}
                          <QPHistoryDrawer qpId={qp.id} qpName={qp.name} />

                          {/* 0.7.0 UX pass — export QP as JSON file. */}
                          <button
                            className="wf-icon-btn qp-card-tool-btn"
                            onClick={async () => {
                              try {
                                const { filename, blob } = await quickPromptsApi.exportQp(qp.id);
                                triggerDownload(filename, blob);
                                if (toastProp) toastProp(t('qp.exportDone').replace('{name}', qp.name), 'success');
                              } catch (e) {
                                if (toastProp) toastProp(userError(e), 'error');
                              }
                            }}
                            title={t('qp.export')}
                            aria-label={`${t('qp.export')} ${qp.name}`}
                          >
                            <Download size={12} />
                          </button>
                          <button
                            className="wf-icon-btn qp-card-tool-btn"
                            data-danger="true"
                            onClick={async () => {
                              if (!confirm(t('qp.deleteConfirm'))) return;
                              await quickPromptsApi.delete(qp.id);
                              if (selectedQuickPromptId === qp.id) setSelectedQuickPromptId(null);
                              refetchQP();
                            }}
                            title={t('qp.delete')}
                            aria-label={`${t('qp.delete')} ${qp.name}`}
                          >
                            <Trash2 size={12} />
                          </button>
                        </div>

                        <div className="qp-card-primary-actions">
                          {/* Batch button — fans out N discussions from a list
                              of values for the first QP variable. Only meaningful
                              for QPs with at least one variable. */}
                          {qp.variables.length > 0 && (
                            <button
                              className="qp-card-secondary-btn"
                              onClick={() => {
                                setBatchingQP(batchingQP?.id === qp.id ? null : qp);
                                setBatchInputLines('');
                              }}
                              title={t('qp.batch.launch')}
                              aria-label={`${t('qp.batch.launch')} ${qp.name}`}
                            >
                              <Layers size={12} /> {t('qp.batch.short')}
                            </button>
                          )}
                          {/* Compare-agents button — opens the launch form
                              regardless of whether the QP has variables, so
                              the user can pick which subset of agents to
                              compare against (chip selector inside the form).
                              `installedAgentTypes` may be empty during boot
                              detection — disable in that case. */}
                          <button
                            className="qp-card-secondary-btn"
                            data-testid="qp-compare-agents-btn"
                            disabled={!installedAgentTypes || installedAgentTypes.length === 0}
                            onClick={() => {
                              setLaunchingQP(launchingQP?.id === qp.id ? null : qp);
                              setLaunchVars({});
                              setCompareTargets(null);
                            }}
                            title={t('qp.compareAgents.button', installedAgentTypes?.length ?? 0)}
                            aria-label={`${t('qp.compareAgents.button', installedAgentTypes?.length ?? 0)} — ${qp.name}`}
                          >
                            🤝 {t('qp.compareAgents.short')}
                          </button>
                          <button
                            className="qp-launch-btn"
                            onClick={() => {
                              // Guard against double / triple click on
                              // no-variable QPs — `handleLaunchQP` creates
                              // a discussion via `discussions.create`,
                              // which is not idempotent. Pre-fix a fast
                              // operator click ended up with 2-3 discussions
                              // launched in parallel for the same prompt.
                              // For QPs with variables this branch only
                              // toggles the launch form open/close so the
                              // disabled state isn't needed.
                              if (qp.variables.length === 0) {
                                if (launching) return;
                                setLaunchingQP(qp);
                                setLaunchVars({});
                                handleLaunchQP(qp);
                              } else {
                                setLaunchingQP(launchingQP?.id === qp.id ? null : qp);
                                setLaunchVars({});
                              }
                            }}
                            disabled={launching && qp.variables.length === 0}
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

                      {/* Launch form — variables (if any) + agent chip
                          selector + the two action buttons. Opens for both
                          variable-bearing QPs (regular Launch flow) and
                          no-var QPs (so the user can pick a compare-agents
                          subset before firing). */}
                      {launchingQP?.id === qp.id && (
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
                          {/* Compare targets use the shared agent+tier picker,
                              so this launch surface speaks the same model
                              language as discussions, QPs and workflow steps. */}
                          {installedAgentTypes && installedAgentTypes.length > 0 && (() => {
                            const targets = compareTargets ?? installedAgentTypes.map(agent => ({
                              agent,
                              tier: qp.tier ?? 'default' as ModelTier,
                            }));
                            const representedAgents = new Set(targets.map(target => target.agent));
                            const missingAgents = installedAgentTypes.filter(agent => !representedAgents.has(agent));
                            const hasOneOfEveryAgent = targets.length === installedAgentTypes.length
                              && missingAgents.length === 0;
                            return (
                              <div className="qp-compare-selector">
                                <span className="qp-compare-selector-label">
                                  🤝 {t('qp.compareAgents.selectorLabel')}
                                </span>
                                {targets.map((target, targetIndex) => {
                                  const duplicateIndex = targets
                                    .slice(0, targetIndex)
                                    .filter(row => row.agent === target.agent).length;
                                  return (
                                    <span
                                      key={`${targetIndex}-${target.agent}-${target.tier}`}
                                      className="qp-compare-target qp-compare-target--active"
                                      style={{ borderColor: agentColor(target.agent) }}
                                    >
                                      <button
                                        type="button"
                                        data-testid={`qp-compare-chip-${target.agent}${duplicateIndex > 0 ? `-${duplicateIndex + 1}` : ''}`}
                                        className="qp-compare-target-toggle"
                                        onClick={() => setCompareTargets(targets.filter((_, index) => index !== targetIndex))}
                                        aria-pressed="true"
                                        aria-label={`${t('qp.compareAgents.removeTarget')} ${AGENT_LABELS[target.agent] ?? target.agent}`}
                                      >
                                        ✓
                                      </button>
                                      <AgentSwitchPicker
                                        currentAgent={target.agent}
                                        availableAgents={installedAgentTypes}
                                        currentTier={target.tier}
                                        onSelectionChange={async (agent, tier) => {
                                          setCompareTargets(current => {
                                            const rows = current ?? installedAgentTypes.map(installed => ({
                                              agent: installed,
                                              tier: qp.tier ?? 'default' as ModelTier,
                                            }));
                                            return rows.map((row, index) => index === targetIndex ? { agent, tier } : row);
                                          });
                                        }}
                                        tierLabels={{
                                          economy: t('disc.tier.economy'),
                                          default: t('disc.tier.default'),
                                          reasoning: t('disc.tier.reasoning'),
                                        }}
                                        modelTiers={agentAccess?.model_tiers}
                                        defaultModelLabel={t('config.defaultModel')}
                                        title={t('disc.switchAgentAndTier')}
                                        ariaLabel={t('qp.compareAgents.targetLabel', AGENT_LABELS[target.agent] ?? target.agent)}
                                        suffix={modelForAgentTier(target.agent, target.tier, agentAccess?.model_tiers, t('config.defaultModel'))}
                                        compact
                                      />
                                    </span>
                                  );
                                })}
                                {missingAgents.map(agent => (
                                  <span key={`missing-${agent}`} className="qp-compare-target qp-compare-target--inactive">
                                    <button
                                      type="button"
                                      data-testid={`qp-compare-chip-${agent}`}
                                      className="qp-compare-target-toggle"
                                      onClick={() => setCompareTargets([...targets, { agent, tier: qp.tier ?? 'default' }])}
                                      aria-pressed="false"
                                      aria-label={`${t('qp.compareAgents.addTarget')} ${AGENT_LABELS[agent] ?? agent}`}
                                    >
                                      +
                                    </button>
                                    <span className="qp-compare-target-empty">
                                      {AGENT_LABELS[agent] ?? agent}
                                      <span aria-hidden="true">{MODEL_TIER_ICONS.default}</span>
                                    </span>
                                  </span>
                                ))}
                                <button
                                  type="button"
                                  className="qp-compare-add-model"
                                  onClick={() => setCompareTargets([
                                    ...targets,
                                    { agent: qp.agent, tier: qp.tier ?? 'default' },
                                  ])}
                                >
                                  <Plus size={11} /> {t('qp.compareAgents.addModel')}
                                </button>
                                <button
                                  type="button"
                                  className="qp-compare-toggle-all"
                                  onClick={() =>
                                    setCompareTargets(hasOneOfEveryAgent ? [] : null)
                                  }
                                >
                                  {hasOneOfEveryAgent
                                    ? t('qp.compareAgents.selectNone')
                                    : t('qp.compareAgents.selectAll')}
                                </button>
                              </div>
                            );
                          })()}
                          <div className="flex-row gap-3">
                            {qp.variables.length > 0 && (
                              <button
                                className="qp-launch-go-btn"
                                onClick={() => handleLaunchQP(qp)}
                                disabled={launching}
                              >
                                {launching ? <Loader2 size={14} className="spin" /> : <Play size={14} />}
                                {launching ? '...' : t('qp.launch')}
                              </button>
                            )}
                            {/* Compare-agents CTA — fires the rendered prompt
                             *  across the selected agents only. Disabled when
                             *  the chip selector reaches 0 selected (the
                             *  user actively chose nothing — fail loud
                             *  rather than silently fan-out across all). */}
                            {(() => {
                              const selectedCount = (compareTargets ?? installedAgentTypes ?? []).length;
                              return (
                                <button
                                  className="qp-launch-compare-btn"
                                  data-testid="qp-compare-agents-launch"
                                  onClick={() => handleCompareAgents(qp)}
                                  disabled={launching || selectedCount === 0}
                                  title={t('qp.compareAgents.tooltip', selectedCount)}
                                >
                                  🤝 {t('qp.compareAgents.cta', selectedCount)}
                                </button>
                              );
                            })()}
                          </div>
                        </div>
                      )}
                    </div>
                    {selectedQuickPromptId === qp.id && (
                      <div className="automation-direct-editor">
                        <QuickPromptForm
                          editPrompt={qp}
                          projects={projects}
                          skills={skillsCatalog ?? []}
                          profiles={profilesCatalog ?? []}
                          directives={directivesCatalog ?? []}
                          onSave={handleSaveQP}
                          onCancel={() => setSelectedQuickPromptId(null)}
                        />
                      </div>
                    )}
                    </Fragment>
                  ))}
                  </div>
                </>
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
                <>
                  {!selectedQuickApiId && <div className="automation-list-toolbar">
                    <ListControls
                      filterLabel={t('automation.filter.label')}
                      filterAriaLabel={t('automation.filter.qaApiLabel')}
                      filterValue={quickApiPluginFilter}
                      filterOptions={[
                        { value: 'all', label: t('automation.filter.allApis') },
                        ...quickApiPlugins.map(plugin => ({
                          value: plugin.slug,
                          label: plugin.label,
                        })),
                      ]}
                      onFilterChange={setQuickApiPluginFilter}
                      sortLabel={t('automation.sort.label')}
                      sortAriaLabel={t('automation.sort.qaLabel')}
                      sortValue={quickApiSort}
                      sortOptions={[
                        { value: 'name', label: t('automation.sort.name') },
                        { value: 'updated', label: t('automation.sort.updated') },
                        { value: 'endpoint', label: t('automation.sort.endpoint') },
                      ]}
                      onSortChange={setQuickApiSort}
                      reversed={quickApiSortReversed}
                      onToggleDirection={() => setQuickApiSortReversed(value => !value)}
                      directionLabel={t(quickApiSortReversed
                        ? 'automation.sort.restoreDirection'
                        : 'automation.sort.reverseDirection')}
                    />
                  </div>}
                  <div className="qp-list">
                  {visibleQuickApis.length === 0 && (
                    <div className="automation-filter-empty">{t('automation.filter.empty')}</div>
                  )}
                  {visibleQuickApis.map((qa, index) => {
                    const startsApiGroup = quickApiSort === 'endpoint'
                      && visibleQuickApis[index - 1]?.api_plugin_slug !== qa.api_plugin_slug;
                    const apiLabel = availableApiPlugins.find(
                      option => option.server.id === qa.api_plugin_slug,
                    )?.server.name ?? qa.api_plugin_slug;
                    return (
                    <Fragment key={qa.id}>
                    {startsApiGroup && (
                      <div className="quick-api-group-heading">
                        <span>{apiLabel}</span>
                        <span>{t('automation.sort.apiGroup')}</span>
                      </div>
                    )}
                    <div
                      className="qp-card"
                      data-kind="api"
                      data-detail={selectedQuickApiId === qa.id}
                      data-qa-id={qa.id}
                    >
                      <div className="qp-card-header">
                        <div className="qp-card-identity">
                          <span className="qp-card-icon">{qa.icon}</span>
                          <span className="qp-card-name">{qa.name}</span>
                        </div>
                        <div className="qp-card-head-controls">
                          <button
                            className="wf-icon-btn qp-card-tool-btn"
                            data-active={qa.pinned}
                            onClick={() => { void toggleQuickFavorite('quickApis', qa.id, qa.pinned, quickApisApi.setPinned, refetchQA); }}
                            title={qa.pinned ? t('wf.unpin') : t('wf.pin')}
                            aria-pressed={qa.pinned}
                            aria-label={`${qa.pinned ? t('wf.unpin') : t('wf.pin')} · ${qa.name}`}
                          >
                            <Star size={14} fill={qa.pinned ? 'currentColor' : 'none'} />
                          </button>
                          <CopyIdPill id={qa.id} title={t('automation.copyId', qa.name)} />
                        </div>
                      </div>

                      {qa.description && (
                        <p className="qp-card-desc">{qa.description}</p>
                      )}

                      <div className="qp-card-meta">
                        <span className="qp-card-api-plugin">{apiLabel}</span>
                        <span className="qp-card-endpoint">
                          <span className="qp-card-method" data-method={qa.api_method ?? 'GET'}>
                            {qa.api_method ?? 'GET'}
                          </span>
                          <code>{qa.api_endpoint_path}</code>
                        </span>
                        {qa.variables.length > 0 && (
                          <span className="qp-card-vars">{t('qa.vars', qa.variables.length)}</span>
                        )}
                      </div>

                      <div className="qp-card-footer">
                        <div className="qp-card-tools">
                          {selectedQuickApiId !== qa.id && <button
                            className="wf-icon-btn qp-card-tool-btn"
                            onClick={() => setEditingQA(qa)}
                            title={t('qa.edit')}
                            aria-label={`${t('qa.edit')} ${qa.name}`}
                          >
                            <Eye size={12} />
                          </button>}
                          <button
                            className="wf-icon-btn qp-card-tool-btn"
                            onClick={async () => {
                              try {
                                const { filename, blob } = await quickApisApi.exportQa(qa.id);
                                triggerDownload(filename, blob);
                                if (toastProp) toastProp(t('qa.exportDone').replace('{name}', qa.name), 'success');
                              } catch (e) {
                                if (toastProp) toastProp(userError(e), 'error');
                              }
                            }}
                            title={t('qa.export')}
                            aria-label={`${t('qa.export')} ${qa.name}`}
                          >
                            <Download size={12} />
                          </button>
                          <button
                            className="wf-icon-btn qp-card-tool-btn"
                            data-danger="true"
                            onClick={async () => {
                              if (!confirm(t('qa.deleteConfirm').replace('{name}', qa.name))) return;
                              await quickApisApi.delete(qa.id);
                              if (selectedQuickApiId === qa.id) setSelectedQuickApiId(null);
                              refetchQA();
                            }}
                            title={t('qa.delete')}
                            aria-label={`${t('qa.delete')} ${qa.name}`}
                          >
                            <Trash2 size={12} />
                          </button>
                        </div>

                        <div className="qp-card-primary-actions">
                          {/* Batch button — fans out N HTTP calls from a list
                              of values for the QA's variables. Mirror of the
                              QP Batch button. Only meaningful when the QA
                              declares at least one variable. */}
                          {qa.variables.length > 0 && (
                            <button
                              className="qp-card-secondary-btn"
                              onClick={() => {
                                setBatchingQA(batchingQA?.id === qa.id ? null : qa);
                                setBatchQAInput('');
                                setBatchQAResult(null);
                              }}
                              title={t('qa.batch.launch')}
                              aria-label={`${t('qa.batch.launch')} ${qa.name}`}
                            >
                              <Layers size={12} /> {t('qa.batch.short')}
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
                    {selectedQuickApiId === qa.id && (
                      <div className="automation-direct-editor">
                        <QuickApiForm
                          editApi={qa}
                          projects={projects}
                          availableApiPlugins={availableApiPlugins}
                          installedAgents={installedAgentTypes ?? []}
                          configLanguage={configLanguage}
                          onSave={handleSaveQA}
                          onCancel={() => setSelectedQuickApiId(null)}
                        />
                      </div>
                    )}
                    </Fragment>
                    );
                  })}
                  </div>
                </>
              )}
            </>
          )}
        </div>
      )}

      {/* ═══ QUICK EXECS TAB ═══ */}
      {tab === 'quickExecs' && (
        <div>
          {(showCreateQE || editingQE) && (
            <QuickExecForm
              editExec={editingQE ?? undefined}
              projects={projects}
              onSave={handleSaveQE}
              onCancel={() => { setShowCreateQE(false); setEditingQE(null); }}
            />
          )}
          {!showCreateQE && !editingQE && (
            !quickExecList || quickExecList.length === 0 ? (
              <div className="wf-empty">
                <p className="wf-empty-title">{t('qe.empty')}</p>
                <p className="wf-empty-hint">{t('qe.emptyHint')}</p>
              </div>
            ) : (
              <div className="qp-list">
                {visibleQuickExecs.map(quickExec => {
                  const project = projects.find(item => item.id === quickExec.project_id);
                  const isOpen = runningQE?.id === quickExec.id;
                  const commandPreview = [quickExec.command, ...quickExec.args].join(' ');
                  return (
                    <Fragment key={quickExec.id}>
                    <div
                      className="qp-card qe-card"
                      data-kind="exec"
                      data-detail={selectedQuickExecId === quickExec.id}
                    >
                      <div className="qp-card-header">
                        <div className="qp-card-identity">
                          <span className="qp-card-icon">{quickExec.icon}</span>
                          <span className="qp-card-name">{quickExec.name}</span>
                        </div>
                        <div className="qp-card-head-controls">
                          <button
                            className="wf-icon-btn qp-card-tool-btn"
                            data-active={quickExec.pinned}
                            onClick={() => { void toggleQuickFavorite('quickExecs', quickExec.id, quickExec.pinned, quickExecsApi.setPinned, refetchQE); }}
                            title={quickExec.pinned ? t('wf.unpin') : t('wf.pin')}
                            aria-pressed={quickExec.pinned}
                            aria-label={`${quickExec.pinned ? t('wf.unpin') : t('wf.pin')} · ${quickExec.name}`}
                          >
                            <Star size={14} fill={quickExec.pinned ? 'currentColor' : 'none'} />
                          </button>
                          <CopyIdPill id={quickExec.id} title={t('automation.copyId', quickExec.name)} />
                        </div>
                      </div>
                      {quickExec.description && <p className="qp-card-desc">{quickExec.description}</p>}
                      <div className="qe-command-preview">
                        {selectedQuickExecId === quickExec.id ? (
                          <code className="qe-command-line" title={commandPreview}>{commandPreview}</code>
                        ) : (
                          <>
                            <code>{quickExec.command}</code>
                            {quickExec.args.map((argument, index) => <code key={index}>{argument}</code>)}
                          </>
                        )}
                      </div>
                      <div className="qp-card-meta">
                        <span>{quickExec.output_format.toUpperCase()}</span>
                        <span>{quickExec.timeout_secs}s</span>
                        {project && <span>{project.name}</span>}
                        {quickExec.variables.length > 0 && <span>{t('qp.vars', quickExec.variables.length)}</span>}
                      </div>
                      <div className="qp-card-footer">
                        <div className="qp-card-tools">
                          {selectedQuickExecId !== quickExec.id && <button className="wf-icon-btn qp-card-tool-btn" onClick={() => setEditingQE(quickExec)} title={t('qe.edit')}>
                            <Eye size={12} />
                          </button>}
                          <button
                            className="wf-icon-btn qp-card-tool-btn"
                            title={t('qe.export')}
                            onClick={async () => {
                              const result = await quickExecsApi.export(quickExec.id);
                              triggerDownload(result.filename, result.blob);
                            }}
                          >
                            <Download size={12} />
                          </button>
                          <button
                            className="wf-icon-btn qp-card-tool-btn"
                            title={t('qe.delete')}
                            onClick={async () => {
                              if (!confirm(t('qe.deleteConfirm', quickExec.name))) return;
                              await quickExecsApi.delete(quickExec.id);
                              if (runningQE?.id === quickExec.id) setRunningQE(null);
                              if (selectedQuickExecId === quickExec.id) setSelectedQuickExecId(null);
                              refetchQE();
                            }}
                          >
                            <Trash2 size={12} />
                          </button>
                        </div>
                        <button
                          className="qp-launch-btn"
                          onClick={() => {
                            if (isOpen) {
                              setRunningQE(null);
                              setRunQEState({ busy: false, data: null, error: null });
                            } else {
                              setRunningQE(quickExec);
                              setRunVarsQE({});
                              setRunQEState({ busy: false, data: null, error: null });
                              if (quickExec.variables.length === 0) void handleRunQE(quickExec);
                            }
                          }}
                        >
                          <Play size={12} /> {t('qe.run')}
                        </button>
                      </div>
                      {isOpen && (
                        <div className="qp-launch-form qe-run-form">
                          {quickExec.variables.map(variable => (
                            <div className="qp-launch-field" key={variable.name}>
                              <label className="qp-launch-label">
                                {variable.label || variable.name}{variable.required && ' *'}
                              </label>
                              <input
                                className="wf-input flex-1"
                                value={runVarsQE[variable.name] ?? ''}
                                placeholder={variable.placeholder}
                                onChange={event => setRunVarsQE(current => ({ ...current, [variable.name]: event.target.value }))}
                              />
                            </div>
                          ))}
                          {quickExec.variables.length > 0 && (
                            <button
                              className="qp-launch-go-btn"
                              disabled={runQEState.busy || quickExec.variables.some(variable => variable.required && !(runVarsQE[variable.name] ?? '').trim())}
                              onClick={() => void handleRunQE(quickExec)}
                            >
                              {runQEState.busy ? <Loader2 size={14} className="spin" /> : <Play size={14} />}
                              {t('qe.run')}
                            </button>
                          )}
                          {runQEState.error && <div className="wf-apicall-error">{runQEState.error}</div>}
                          {runQEState.data !== null && (
                            <div className="wf-apicall-success qe-run-result">
                              <strong>{t('qe.result')}</strong>
                              <pre>{JSON.stringify(runQEState.data, null, 2)}</pre>
                            </div>
                          )}
                        </div>
                      )}
                    </div>
                    {selectedQuickExecId === quickExec.id && (
                      <div className="automation-direct-editor">
                        <QuickExecForm
                          editExec={quickExec}
                          projects={projects}
                          onSave={handleSaveQE}
                          onCancel={() => setSelectedQuickExecId(null)}
                        />
                      </div>
                    )}
                    </Fragment>
                  );
                })}
              </div>
            )
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
                  // Validate declared patterns HERE — the backend enforces
                  // them at trigger time, but by then the modal has closed
                  // (0.8.5 immediate-close) and its rejection was invisible.
                  // Bounded: a pathological pattern (catastrophic backtracking)
                  // or a huge value could freeze the UI — past these sizes we
                  // skip the local check and let the backend be the authority.
                  const badPattern = vars.find(v => {
                    const val = (launchingWorkflow.values[v.name] ?? '').trim();
                    if (!v.pattern || !val) return false;
                    if (v.pattern.length > 200 || val.length > 512) return false;
                    try { return !new RegExp(`^(?:${v.pattern})$`).test(val); }
                    catch { return false; } // invalid regex → let the backend decide
                  });
                  if (badPattern) {
                    setLaunchingWorkflow(prev => prev ? {
                      ...prev,
                      error: t('wf.launchModalPattern')
                        .replace('{name}', badPattern.label || badPattern.name)
                        .replace('{pattern}', badPattern.pattern ?? ''),
                    } : prev);
                    return;
                  }
                  // 0.8.5 — close the modal IMMEDIATELY after validation
                  // passes. Pre-fix the modal awaited `fireTrigger(...)`
                  // which only resolves when the SSE stream completes
                  // (i.e. the whole run is done) — so the launch box
                  // stayed open for the entire workflow duration, often
                  // tens of minutes. The live progress view (`liveRun`)
                  // takes over rendering once `fireTrigger` fires.
                  const wfId = launchingWorkflow.workflow.id;
                  const vals = launchingWorkflow.values;
                  setLaunchingWorkflow(null);
                  fireTrigger(wfId, vals).catch(e => {
                    // Failure path: the live-run pane surfaces the error;
                    // we just log here so the warning still hits devtools.
                    console.warn('Launch failed:', e);
                  });
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

      {/* Shared Import drawer (workflow, QP, QA, or QE).
          2-step flow : (1) drop / pick file → preview rendered + project
          dropdown ; (2) confirm → POST → refetch + close.
          Modal-style overlay so the user is focused on the operation. */}
      {importing && (
        <div className="wf-import-modal-backdrop" onClick={() => !importingSubmit && setImporting(null)}>
          <div className="wf-import-modal" onClick={(e) => e.stopPropagation()}>
            <div className="flex-row gap-3 mb-4">
              <Upload size={16} />
              <h3 className="text-lg font-semibold flex-1" style={{ margin: 0 }}>
                {importing.kind === null
                  ? t('imp.globalTitle')
                  : importing.kind === 'workflow'
                  ? t('imp.workflowTitle')
                  : importing.kind === 'qp'
                    ? t('imp.qpTitle')
                    : importing.kind === 'qa'
                      ? t('imp.qaTitle')
                      : t('imp.qeTitle')}
              </h3>
              <button className="wf-icon-btn" onClick={() => setImporting(null)} disabled={importingSubmit}>
                <X size={12} />
              </button>
            </div>

            {!importing.content ? (
              <ImportDropzone
                validateParsed={(parsed) => {
                  const detection = detectAutomationImport(parsed);
                  if (detection.ok) return null;
                  return t(detection.reason === 'ambiguous'
                    ? 'imp.errorAmbiguousExport'
                    : 'imp.errorInvalidExport');
                }}
                embedded
                onFile={(content, parsed) => {
                  const detection = detectAutomationImport(parsed);
                  if (!detection.ok) return;
                  if (detection.kind === 'workflow') {
                    const env = parsed as { workflow?: { name?: string; steps?: unknown[] }; referenced_quick_prompts?: unknown[] };
                    setImporting({
                      ...importing,
                      kind: detection.kind,
                      content,
                      preview: {
                        name: env.workflow?.name ?? '?',
                        stepCount: env.workflow?.steps?.length ?? 0,
                        qpVarsCount: env.referenced_quick_prompts?.length ?? 0,
                      },
                    });
                  } else if (detection.kind === 'qp') {
                    const env = parsed as { quick_prompt?: { name?: string; variables?: unknown[] } };
                    setImporting({
                      ...importing,
                      kind: detection.kind,
                      content,
                      preview: {
                        name: env.quick_prompt?.name ?? '?',
                        qpVarsCount: env.quick_prompt?.variables?.length ?? 0,
                      },
                    });
                  } else if (detection.kind === 'qa') {
                    const env = parsed as { quick_api?: { name?: string; variables?: unknown[] } };
                    setImporting({
                      ...importing,
                      kind: detection.kind,
                      content,
                      preview: {
                        name: env.quick_api?.name ?? '?',
                        qpVarsCount: env.quick_api?.variables?.length ?? 0,
                      },
                    });
                  } else {
                    const env = parsed as { quick_exec?: { name?: string; variables?: unknown[] } };
                    setImporting({
                      ...importing,
                      kind: detection.kind,
                      content,
                      preview: {
                        name: env.quick_exec?.name ?? '?',
                        qpVarsCount: env.quick_exec?.variables?.length ?? 0,
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
                  {(importing.kind === 'qp' || importing.kind === 'qa' || importing.kind === 'qe')
                    && (importing.preview.qpVarsCount ?? 0) > 0 && (
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
                    onClick={() => setImporting({ ...importing, kind: null, content: '', preview: { name: '' } })}
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
                      if (!importing.kind) return;
                      setImportingSubmit(true);
                      try {
                        const projectId = importing.targetProjectId || null;
                        if (importing.kind === 'workflow') {
                          const imported = await workflowsApi.importWorkflow({ content: importing.content, project_id: projectId });
                          if (toastProp) toastProp(t('imp.workflowDone').replace('{name}', importing.preview.name), 'success');
                          refetch();
                          setTab('workflows');
                          window.dispatchEvent(new Event('kronn:pages-activated'));
                          void openDetail(imported.id);
                        } else if (importing.kind === 'qp') {
                          const imported = await quickPromptsApi.importQp({ content: importing.content, project_id: projectId });
                          if (toastProp) toastProp(t('imp.qpDone').replace('{name}', importing.preview.name), 'success');
                          refetchQP();
                          setTab('quickPrompts');
                          setSelectedQuickPromptId(imported.id);
                        } else if (importing.kind === 'qa') {
                          const imported = await quickApisApi.importQa({ content: importing.content, project_id: projectId });
                          if (toastProp) toastProp(t('imp.qaDone').replace('{name}', importing.preview.name), 'success');
                          refetchQA();
                          setTab('quickApis');
                          setSelectedQuickApiId(imported.id);
                        } else {
                          const imported = await quickExecsApi.import({ content: importing.content, project_id: projectId });
                          if (toastProp) toastProp(t('imp.qeDone').replace('{name}', importing.preview.name), 'success');
                          refetchQE();
                          setTab('quickExecs');
                          setSelectedQuickExecId(imported.id);
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
      </section>
    </div>
  );
}
