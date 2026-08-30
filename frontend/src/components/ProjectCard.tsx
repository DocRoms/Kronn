import '../pages/Dashboard.css';
import { useState, useCallback, useEffect, useMemo, useRef } from 'react';
import { planning, projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { useIsMobile } from '../hooks/useMediaQuery';
import { isValidationDisc, isBriefingDisc, isBootstrapDisc, isTrackerMcp } from '../lib/constants';
import { canRunAudit, canRunBriefing } from '../lib/agentCapabilities';
import { AiDocViewer } from './AiDocViewer';
import { unseenBasis } from '../lib/discussionUiUtils';
import AuditRecapPanel from './AuditRecapPanel';
import type { AuditKind } from '../types/AuditKind';
import { ProjectSkills } from './ProjectSkills';
import { ProjectLinkedRepos } from './ProjectLinkedRepos';
import {
  saveAuditCheckpoint, loadAuditCheckpoint, clearAuditCheckpoint,
  type AuditCheckpointKind,
} from '../lib/audit-resume';
import type { Project, AgentDetection, AgentType, ModelTier, ModelTiersConfig, DriftCheckResponse, Discussion, Skill, McpConfigDisplay, WorkflowSummary, GitStatusResponse, DependencyUpdateSummary, AuditEvidenceResponse, ContextAuditResponse } from '../types/generated';
import {
  ChevronRight, ChevronDown, Cpu, Workflow,
  Plus, Trash2, Zap,
  Loader2,
  MessageSquare, AlertTriangle,
  Play, FileCode, ShieldCheck, StopCircle, BookOpen, Rocket, Check, RefreshCw, Puzzle,
  FolderInput, Plug, X, FileText, DownloadCloud,
  Code2, ExternalLink, GitBranch, GitPullRequest, Tag, Package, ListTodo,
  CircleHelp,
  Container,
  Copy,
} from 'lucide-react';
import { BriefingForm } from './BriefingForm';
import { CopyIdPill } from './CopyIdPill';
import { ProjectCodePanel } from './ProjectCodePanel';
import { ProjectGitPanel } from './ProjectGitPanel';
import { ProjectTasksPanel } from './ProjectTasksPanel';
import { ContextHelp } from './ContextHelp';
import { AgentSwitchPicker } from './AgentSwitchPicker';
import { ProjectDockerPanel } from './ProjectDockerPanel';

const STATUS_COLORS: Record<string, string> = {
  Pending: 'var(--kr-warning)', Running: 'var(--kr-cyan)', Success: 'var(--kr-success)',
  Failed: 'var(--kr-error)', Cancelled: 'var(--kr-cancelled)', WaitingApproval: 'var(--kr-accent-ink)',
};

type ProjectDetailView = 'overview' | 'discussions' | 'tasks' | 'audit' | 'docs' | 'code' | 'docker' | 'git' | 'resources';

const PROJECT_DETAIL_VIEWS: ProjectDetailView[] = [
  'overview', 'discussions', 'tasks', 'audit', 'docs', 'code', 'docker', 'git', 'resources',
];
const PROJECT_DETAIL_VIEW_STORAGE_KEY = 'kronn:projectDetailView';

function readProjectDetailView(): ProjectDetailView {
  try {
    const saved = localStorage.getItem(PROJECT_DETAIL_VIEW_STORAGE_KEY);
    return PROJECT_DETAIL_VIEWS.includes(saved as ProjectDetailView)
      ? saved as ProjectDetailView
      : 'overview';
  } catch {
    return 'overview';
  }
}

const LANGUAGE_COLORS: Record<string, string> = {
  TypeScript: '#3178c6',
  JavaScript: '#f1e05a',
  Rust: '#dea584',
  PHP: '#4f5d95',
  Python: '#3572a5',
  Go: '#00add8',
  Java: '#b07219',
  Kotlin: '#a97bff',
  Swift: '#f05138',
  C: '#555555',
  'C++': '#f34b7d',
  'C#': '#178600',
  Ruby: '#701516',
  Vue: '#41b883',
  Svelte: '#ff3e00',
  CSS: '#663399',
  HTML: '#e34c26',
  Shell: '#89e051',
};

/** Format a millisecond duration as `Xs` under 60s, `MmSSs` past 60s. */
function formatElapsedShort(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return s >= 60 ? `${Math.floor(s / 60)}m${String(s % 60).padStart(2, '0')}s` : `${s}s`;
}

export interface ProjectCardProps {
  project: Project;
  dockerRunning?: boolean;
  onDockerRunningChange?: (projectId: string, running: boolean) => void;
  detailMode?: boolean;
  isOpen: boolean;
  onToggleOpen: () => void;
  discussions: Discussion[];
  driftStatus: DriftCheckResponse | undefined;
  /** The Dashboard fleet poll sees a live audit for this project (possibly
   *  launched outside this card — MCP bridge, CLI). Triggers poll adoption. */
  externalAuditLive?: boolean;
  agents: AgentDetection[];
  allSkills: Skill[];
  mcpConfigs: McpConfigDisplay[];
  workflows: WorkflowSummary[];
  configLanguage: string | null;
  modelTiers?: ModelTiersConfig | null;
  toast: (msg: string, type: 'success' | 'error' | 'warning' | 'info') => void;
  onNavigate: (page: string) => void;
  onSetDiscPrefill: (prefill: { projectId: string; title: string; prompt: string; locked?: boolean }) => void;
  onAutoRunDiscussion: (discId: string) => void;
  onOpenDiscussion: (discId: string) => void;
  onRefetch: () => void;
  onRefetchDiscussions: () => void;
  onRefetchSkills: () => void;
  onRefetchDrift: (projectId: string) => void;
}

export function ProjectCard({
  externalAuditLive = false,
  detailMode = false,
  project: proj,
  dockerRunning = false,
  onDockerRunningChange,
  isOpen,
  onToggleOpen,
  discussions: projDiscussions,
  driftStatus,
  agents,
  allSkills,
  mcpConfigs,
  workflows,
  modelTiers,
  toast,
  onNavigate,
  onSetDiscPrefill,
  onAutoRunDiscussion,
  onOpenDiscussion,
  onRefetch,
  onRefetchDiscussions,
  onRefetchSkills,
  onRefetchDrift,
}: ProjectCardProps) {
  const { t, locale } = useT();
  const isMobile = useIsMobile();
  const [detailView, setDetailView] = useState<ProjectDetailView>(readProjectDetailView);
  const [codeInitialPath, setCodeInitialPath] = useState<string | null>(null);
  const [expandedTab, setExpandedTab] = useState<string | undefined>(undefined);
  const selectDetailView = useCallback((view: ProjectDetailView) => {
    setDetailView(view);
    try {
      localStorage.setItem(PROJECT_DETAIL_VIEW_STORAGE_KEY, view);
    } catch {
      // localStorage may be unavailable in private/restricted browser modes.
    }
  }, []);
  const reportDockerRunning = useCallback(
    (running: boolean) => onDockerRunningChange?.(proj.id, running),
    [onDockerRunningChange, proj.id],
  );
  const [gitRevision, setGitRevision] = useState(0);
  const [projectTaskCount, setProjectTaskCount] = useState<number | null>(null);
  const [visibleDiscussionCount, setVisibleDiscussionCount] = useState(10);
  const [discussionLoadAmount, setDiscussionLoadAmount] = useState<'10' | '50' | 'all'>('10');
  const [overviewGit, setOverviewGit] = useState<GitStatusResponse | null>(null);
  const [overviewGitLoading, setOverviewGitLoading] = useState(false);
  const [overviewGitError, setOverviewGitError] = useState(false);
  const gitLanguageRefreshRef = useRef(false);
  const [dependencyUpdates, setDependencyUpdates] = useState<DependencyUpdateSummary | null>(null);
  const [dependencyUpdatesLoading, setDependencyUpdatesLoading] = useState(false);
  const [dependencyUpdatesError, setDependencyUpdatesError] = useState(false);
  const [dependencyMonitoringSaving, setDependencyMonitoringSaving] = useState(false);
  const dependencyRefreshRef = useRef(false);
  const [auditEvidence, setAuditEvidence] = useState<AuditEvidenceResponse | null>(null);
  const [contextAudit, setContextAudit] = useState<ContextAuditResponse | null>(null);
  const [contextAuditLoading, setContextAuditLoading] = useState(false);
  const [contextAuditError, setContextAuditError] = useState(false);
  const [contextAuditDetailsOpen, setContextAuditDetailsOpen] = useState(false);
  const [contextAuditWhyOpen, setContextAuditWhyOpen] = useState(false);
  const [attestingDocumentation, setAttestingDocumentation] = useState(false);
  const [acceptingContextBaseline, setAcceptingContextBaseline] = useState(false);
  const attestationGuardRef = useRef(false);
  const recentProjectDiscussions = useMemo(
    () => [...projDiscussions].sort(
      (left, right) => Date.parse(right.updated_at) - Date.parse(left.updated_at),
    ),
    [projDiscussions],
  );

  useEffect(() => {
    if (!detailMode || !isOpen) return;
    let alive = true;
    planning.list({ projectId: proj.id, limit: 100 })
      .then(response => {
        if (alive) setProjectTaskCount(response.items.length);
      })
      .catch(() => {
        if (alive) setProjectTaskCount(null);
      });
    return () => { alive = false; };
  }, [detailMode, isOpen, proj.id]);

  useEffect(() => {
    if (!detailMode || !isOpen || detailView !== 'overview' || proj.path_exists === false) return;
    let alive = true;
    void Promise.resolve().then(async () => {
      if (!alive) return;
      setOverviewGitLoading(true);
      setOverviewGitError(false);
      try {
        const status = await projectsApi.gitStatus(proj.id);
        if (alive) setOverviewGit(status);
      } catch {
        if (alive) {
          setOverviewGit(null);
          setOverviewGitError(true);
        }
      } finally {
        if (alive) setOverviewGitLoading(false);
      }
    });
    return () => {
      alive = false;
    };
  }, [detailMode, detailView, isOpen, proj.id, proj.path_exists]);

  const refreshContextAudit = useCallback(async () => {
    if (contextAuditLoading || proj.path_exists === false) return;
    setContextAuditLoading(true);
    setContextAuditError(false);
    try {
      const [evidence, audit] = await Promise.all([
        projectsApi.auditEvidence(proj.id),
        projectsApi.contextAudit(proj.id),
      ]);
      setAuditEvidence(evidence);
      setContextAudit(audit);
    } catch {
      setContextAuditError(true);
    } finally {
      setContextAuditLoading(false);
    }
  }, [contextAuditLoading, proj.id, proj.path_exists]);

  useEffect(() => {
    if (!detailMode || !isOpen || detailView !== 'overview' || proj.path_exists === false) return;
    let alive = true;
    void Promise.all([
      projectsApi.auditEvidence(proj.id),
      projectsApi.contextAudit(proj.id),
    ]).then(([evidence, audit]) => {
      if (!alive) return;
      setAuditEvidence(evidence);
      setContextAudit(audit);
      setContextAuditError(false);
    }).catch(() => {
      if (alive) setContextAuditError(true);
    });
    return () => { alive = false; };
  }, [detailMode, detailView, isOpen, proj.id, proj.path_exists]);

  const attestExistingDocumentation = useCallback(async () => {
    if (attestationGuardRef.current) return;
    if (!window.confirm(t('projects.contextAudit.attestConfirm'))) return;
    attestationGuardRef.current = true;
    setAttestingDocumentation(true);
    try {
      setAuditEvidence(await projectsApi.attestDocumentation(proj.id));
      onRefetch();
      toast(t('projects.contextAudit.attested'), 'success');
    } catch {
      toast(t('projects.contextAudit.attestFailed'), 'error');
    } finally {
      attestationGuardRef.current = false;
      setAttestingDocumentation(false);
    }
  }, [onRefetch, proj.id, t, toast]);

  const acceptCurrentContextBaseline = useCallback(async () => {
    if (acceptingContextBaseline) return;
    setAcceptingContextBaseline(true);
    try {
      setContextAudit(await projectsApi.acceptContextBaseline(proj.id));
      toast(t('projects.contextAudit.baselineAccepted'), 'success');
    } catch {
      toast(t('projects.contextAudit.baselineFailed'), 'error');
    } finally {
      setAcceptingContextBaseline(false);
    }
  }, [acceptingContextBaseline, proj.id, t, toast]);

  const refreshGitLanguages = useCallback(async () => {
    if (gitLanguageRefreshRef.current) return;
    gitLanguageRefreshRef.current = true;
    setOverviewGitLoading(true);
    setOverviewGitError(false);
    try {
      setOverviewGit(await projectsApi.gitStatus(proj.id, true));
    } catch {
      setOverviewGitError(true);
    } finally {
      gitLanguageRefreshRef.current = false;
      setOverviewGitLoading(false);
    }
  }, [proj.id]);

  useEffect(() => {
    if (!detailMode || !isOpen || detailView !== 'overview' || proj.path_exists === false) return;
    let alive = true;
    void Promise.resolve().then(async () => {
      if (!alive) return;
      setDependencyUpdatesLoading(true);
      setDependencyUpdatesError(false);
      try {
        const summary = await projectsApi.dependencyUpdates(proj.id);
        if (alive) setDependencyUpdates(summary);
      } catch {
        if (alive) {
          setDependencyUpdates(null);
          setDependencyUpdatesError(true);
        }
      } finally {
        if (alive) setDependencyUpdatesLoading(false);
      }
    });
    return () => {
      alive = false;
    };
  }, [detailMode, detailView, isOpen, proj.id, proj.path_exists]);

  const refreshDependencyUpdates = useCallback(async () => {
    if (dependencyRefreshRef.current) return;
    dependencyRefreshRef.current = true;
    setDependencyUpdatesLoading(true);
    setDependencyUpdatesError(false);
    try {
      setDependencyUpdates(await projectsApi.dependencyUpdates(proj.id, true));
    } catch {
      setDependencyUpdatesError(true);
    } finally {
      dependencyRefreshRef.current = false;
      setDependencyUpdatesLoading(false);
    }
  }, [proj.id]);

  const updateDependencyMonitoring = useCallback(async (intervalDays: number | null) => {
    if (dependencyMonitoringSaving) return;
    setDependencyMonitoringSaving(true);
    try {
      await projectsApi.setDependencyMonitoring(proj.id, intervalDays);
      setDependencyUpdates(await projectsApi.dependencyUpdates(proj.id));
    } catch {
      setDependencyUpdatesError(true);
    } finally {
      setDependencyMonitoringSaving(false);
    }
  }, [dependencyMonitoringSaving, proj.id]);

  // ── Collapsible sections ──
  // 0.8.4 (#323 / F3) — on `Validated` and `Audited`, `aiContext` is
  // the action zone (relaunch the complete audit, validate). The
  // pre-fix default sent the user to `discussions` which forced 2
  // clicks to reach the launcher (and was discovered confusing in
  // the Marc persona Playwright pass). Bootstrapped keeps
  // `discussions` because the briefing disc is the natural focus
  // before audit; NoTemplate / TemplateInstalled also use `aiContext`
  // since that's where the briefing form CTA lives.
  const defaultSection = (auditStatus: string) =>
    auditStatus === 'Bootstrapped' ? 'discussions' : 'aiContext';
  const isSectionOpen = (section: string) => {
    if (expandedTab === undefined) return section === defaultSection(proj.audit_status);
    return expandedTab === section;
  };
  const toggleSection = (section: string) => {
    setExpandedTab(prev => (prev === section ? '' : section));
  };
  // When the user clicks the TD badge on the header, we deep-link the
  // docs viewer to expand the `docs/tech-debt/` folder and preselect the
  // first item inside it. The state is consumed by the `AiDocViewer`
  // `initialExpandFolder` prop; we clear it after one render via a
  // useEffect-less pattern (the prop only matters at mount time of the
  // viewer because of the dep on `projectId, initialExpandFolder` in
  // the load effect — see AiDocViewer L37).
  const [docDeepLink, setDocDeepLink] = useState<string | undefined>(undefined);

  // 0.8.7 — anti-hallu section status. Lazily fetched at mount + after every
  // explicit inject so the badge reflects current state. `null` = not yet
  // loaded (no badge displayed), `{present: false}` = legacy project that
  // needs migration → CTA shown, `{present: true}` = section is canonical.
  const [antiHalluStatus, setAntiHalluStatus] = useState<{
    present: boolean;
    audit_date?: string | null;
    file_exists: boolean;
  } | null>(null);
  const [antiHalluBusy, setAntiHalluBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    projectsApi
      .antiHalluStatus(proj.id)
      .then(s => {
        if (alive) setAntiHalluStatus(s);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [proj.id]);

  const handleInjectAntiHallu = useCallback(async () => {
    if (antiHalluBusy) return;
    setAntiHalluBusy(true);
    try {
      await projectsApi.injectAntiHallu(proj.id);
      // Refetch status to reflect the new state.
      const s = await projectsApi.antiHalluStatus(proj.id);
      setAntiHalluStatus(s);
    } finally {
      setAntiHalluBusy(false);
    }
  }, [proj.id, antiHalluBusy]);

  // 0.8.3 (#314) — post-validation deep-link consumer. MessageBubble
  // writes `kronn:postValidation:<projectId>` to sessionStorage when
  // the user clicks the "View Tech Debts" CTA in the validation
  // discussion. We read + clear it on every render where the card is
  // open AND the AI Context tab is exposable; the value is the
  // folder path to deep-link into (e.g. `docs/tech-debt`). One-shot:
  // we always remove the key so a manual reload doesn't re-trigger.
  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    let target: string | null = null;
    try {
      target = sessionStorage.getItem(`kronn:postValidation:${proj.id}`);
      if (target) sessionStorage.removeItem(`kronn:postValidation:${proj.id}`);
    } catch { /* private mode / quota — no deep-link */ }
    if (target) {
      queueMicrotask(() => {
        if (cancelled) return;
        selectDetailView('docs');
        setExpandedTab('docAi');
        setDocDeepLink(target);
      });
    }
    return () => { cancelled = true; };
  }, [isOpen, proj.id, selectDetailView]);

  // Generic one-shot project-tab deep-link. The validation discussion uses
  // this after "Mark audit as validated" so the user lands on the Audit tab
  // and sees the new validated state instead of staying on Discussions.
  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    let target: string | null = null;
    try {
      target = sessionStorage.getItem(`kronn:projectView:${proj.id}`);
      if (target) sessionStorage.removeItem(`kronn:projectView:${proj.id}`);
    } catch { /* private mode / quota — no deep-link */ }
    if (target && PROJECT_DETAIL_VIEWS.includes(target as ProjectDetailView)) {
      queueMicrotask(() => {
        if (!cancelled) selectDetailView(target as ProjectDetailView);
      });
    }
    return () => { cancelled = true; };
  }, [isOpen, proj.id, selectDetailView]);

  // ── Audit state ──
  const [auditActive, setAuditActive] = useState(false);
  // 0.8.4 (#298) — bump on every audit completion so the recap panel
  // refetches the latest run + per-step metrics without manual reload.
  const [auditCompletedTick, setAuditCompletedTick] = useState(0);
  const [auditStep, setAuditStep] = useState(0);
  const [auditTotalSteps, setAuditTotalSteps] = useState(0);
  const [auditCurrentFile, setAuditCurrentFile] = useState('');
  // 0.8.2 TD #233 — wall-clock elapsed for the live audit. The server-side
  // `AuditProgress` carries `started_at`; we just hold it in state and
  // drive a per-second re-render via `auditTick` so the displayed value
  // updates without re-polling the network. Cleared when the audit ends.
  const [auditStartedAt, setAuditStartedAt] = useState<number | null>(null);
  const [auditNow, setAuditNow] = useState(() => Date.now());
  // 0.8.3 TD #274 — per-step + cumulative token counters surfaced live.
  // Backend's enriched `step_done` SSE event carries:
  //   - tokens: max(input+output) for the step just finished (Claude
  //     reports cumulative per-call, so `.max()` over the stream is
  //     the correct aggregation, NOT a sum).
  //   - total_tokens: running sum across all completed steps.
  // Both are `null` until the first `step_done` lands; the UI hides
  // the chips until then so we don't show misleading "0 tk" on a
  // fresh audit that hasn't burned anything yet.
  const [auditLastStepTokens, setAuditLastStepTokens] = useState<number | null>(null);
  const [auditTotalTokens, setAuditTotalTokens] = useState<number | null>(null);
  // 0.8.3 (#281) — currently-active tool the agent is calling (Read,
  // Glob, mcp__..., …). Surfaced as a chip so the user knows what
  // the agent is busy with during the step. Cleared on step_done.
  const [auditCurrentTool, setAuditCurrentTool] = useState<string | null>(null);
  // 0.8.4 (#319 / B3) — running count of tool calls in the current
  // step. Surfaced after the tool name (`🔧 Write (14)`) so the user
  // sees forward motion even when the agent goes through a long
  // tool-only phase without `Usage` events to refresh tokens.
  const [auditToolCallCount, setAuditToolCallCount] = useState<number | null>(null);
  const [auditAbortController, setAuditAbortController] = useState<AbortController | null>(null);
  const [auditAgentChoice, setAuditAgentChoice] = useState<AgentType | undefined>(undefined);
  const [auditTierChoice, setAuditTierChoice] = useState<ModelTier>('reasoning');
  /// Briefing-start in flight — re-used by the post-form AI review
  /// trigger. Pre-fix it guarded the now-removed second "Briefing IA"
  /// button against double-clicks; with the form-only flow the inner
  /// submit button is already `disabled` while pending, but the state
  /// is still wired so the outer "Définir le briefing" CTA can grey
  /// out if a parallel briefing is mid-spawn.
  const [briefingStarting] = useState(false);
  // 0.8.4 (#285) — désagentified briefing form modal toggle.
  const [briefingFormOpen, setBriefingFormOpen] = useState(false);
  /// Companion ref for `auditActive` — keeps `handleFullAudit` and
  /// `startPartialAudit` race-free against a double-click that fires
  /// before React re-renders.
  const auditActiveRef = useRef(false);
  /// Handle to the polling interval so we can clear it on unmount / done.
  const auditPollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  /// `true` once the mount-time resume effect has run — avoids racing a
  /// fresh handleFullAudit() that also calls saveAuditCheckpoint.
  const resumeSettledRef = useRef(false);

  // ── Delete state ──
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);
  const [deleteConfirmInput, setDeleteConfirmInput] = useState('');

  // ── Docs migration (legacy ai/ → docs/) state ──
  // Defaults to symlink ON because most projects have CLI tooling, scripts,
  // CI configs that still reference `ai/`. The opt-out checkbox is one
  // click away for users who want a hard cut.
  const [migrating, setMigrating] = useState(false);
  const [migrationCreateSymlink, setMigrationCreateSymlink] = useState(true);
  const [migrationError, setMigrationError] = useState<string | null>(null);
  /** Set after a successful migration — keeps the banner visible briefly
   *  with a green confirmation row so the operator gets unmistakable
   *  feedback before the refetch removes the banner entirely. */
  const [migrationSuccess, setMigrationSuccess] = useState<{ filesMoved: number } | null>(null);

  // Remap state — shown when the project directory no longer resolves on disk
  // (typically after a cross-OS DB import where absolute paths don't translate).
  const [remapValue, setRemapValue] = useState('');
  const [remapping, setRemapping] = useState(false);
  const [remapError, setRemapError] = useState<string | null>(null);
  // useRef guard alongside the disabled state: React's state update is async, so
  // two fast clicks would otherwise both read `remapping === false` and fire
  // twice. See feedback_race_guards.
  const remapGuard = useRef(false);
  // Clone-and-remap state — alternative to manual remap when the project has a
  // known repo_url: re-clone it locally (via linked Git credentials) and
  // re-point the project at the clone in one click.
  const [cloning, setCloning] = useState(false);
  const cloneGuard = useRef(false);

  // 0.8.3 (#311) — resumable audit detection. Polled at mount + after
  // each audit completion/error so the "Lancer l'audit" button can
  // flip to the dynamic resume CTA when an Interrupted run is on file
  // for this project. `null` = no resumable run; otherwise the row.
  const [resumableAudit, setResumableAudit] = useState<{ id: string; kind: AuditKind; last_completed_step: number; started_at: string } | null>(null);
  useEffect(() => {
    let cancelled = false;
    projectsApi.auditResumable(proj.id).then(row => {
      if (cancelled) return;
      setResumableAudit(row);
    }).catch(() => { /* idle on error — button stays "Lancer" */ });
    return () => { cancelled = true; };
  }, [proj.id, proj.audit_status, auditActive]);

  // Keep the latest terminal result visible on an opened project card. The
  // live tracker owns the in-progress state; history owns the durable result
  // once the tracker clears (including Failed and Interrupted runs).
  const [latestAuditOutcome, setLatestAuditOutcome] = useState<{ id: string; status: string } | null>(null);
  const [selectedAuditRunId, setSelectedAuditRunId] = useState<string | null>(null);
  useEffect(() => {
    if (!detailMode && !isOpen) return;
    // Some narrow consumers (including older test doubles) expose only the
    // audit APIs they need. Treat an absent history reader as no recap rather
    // than throwing from this effect and taking the whole dashboard down.
    if (typeof projectsApi.auditHistory !== 'function') return;
    let cancelled = false;
    projectsApi.auditHistory(proj.id).then(rows => {
      if (cancelled) return;
      const latestTerminal = (rows ?? []).find(row => row.status !== 'Running');
      setLatestAuditOutcome(latestTerminal
        ? { id: latestTerminal.id, status: latestTerminal.status }
        : null);
    }).catch(() => {
      if (!cancelled) setLatestAuditOutcome(null);
    });
    return () => { cancelled = true; };
  }, [proj.id, isOpen, detailMode, auditCompletedTick]);

  // Briefing agent: an explicit audit pick stays valid (audit-capable ⊂
  // briefing-capable), otherwise fall back to any BRIEFING-capable agent —
  // never the audit list, which is empty when only Ollama is installed
  // while the briefing legitimately works with it.
  const briefingAgentPick = (auditAgentChoice && agents.some(a => a.agent_type === auditAgentChoice && canRunBriefing(a)))
    ? auditAgentChoice
    : (agents.filter(canRunBriefing)[0]?.agent_type ?? 'ClaudeCode');
  const auditAgents = useMemo(() => agents.filter(canRunAudit), [agents]);
  const selectedAuditAgent = auditAgentChoice && auditAgents.some(a => a.agent_type === auditAgentChoice)
    ? auditAgentChoice
    : (auditAgents[0]?.agent_type ?? 'ClaudeCode');

  // ── Computed ──
  const validationDisc = projDiscussions.find(d => isValidationDisc(d.title) && !d.archived);
  // 0.8.3 (#311 + #312) — a resumable audit run takes priority over
  // a leftover validation disc. Pre-fix, a rate-limit at step 5 still
  // marked the project Audited + left a validation disc, so the
  // ProjectCard read "Validation en cours" on an audit that hadn't
  // actually produced anything past step 5. The resumable check
  // catches that state and forces the "Reprendre" CTA instead.
  const validationInProgress = !!validationDisc && proj.audit_status === 'Audited' && !resumableAudit;
  const bootstrapDisc = projDiscussions.find(d => isBootstrapDisc(d.title) && !d.archived);
  const bootstrapInProgress = !!bootstrapDisc && proj.audit_status === 'TemplateInstalled';
  // Use the locale-aware detector — the backend's `start_briefing` emits
  // a localized title (`Project Briefing` in EN, `Briefing del proyecto`
  // in ES, `Briefing projet` in FR). Pre-fix a startsWith('Briefing')
  // here missed EN, leaving English users without the "Reprendre le
  // briefing" button after they navigated away mid-briefing.
  // 0.8.4 (#329 / F9) — only consider non-archived briefing discs.
  // Otherwise "Reprendre le briefing" stays visible forever once a
  // briefing has been auto-archived on `KRONN:BRIEFING_COMPLETE`,
  // blocking the new "Définir le briefing" CTA. Same for validation
  // + bootstrap below.
  const briefingDisc = projDiscussions.find(d => isBriefingDisc(d.title) && !d.archived);
  const briefingDone = proj.audit_status !== 'NoTemplate' && (
    !!proj.briefing_notes ||
    proj.audit_status === 'Audited' || proj.audit_status === 'Validated'
  );
  const projMcps = mcpConfigs.filter(c => c.is_global || c.project_ids.includes(proj.id));
  const projWorkflows = workflows.filter(w => w.project_id === proj.id);
  const repositoryUrl = overviewGit?.remote_url
    ?? (proj.repo_url?.startsWith('http') ? proj.repo_url.replace(/\.git\/?$/, '') : null);
  const pullRequestsUrl = overviewGit?.pull_requests_url ?? overviewGit?.pr_url ?? null;
  const languageStats = overviewGit?.languages ?? [];
  const languageTotalBytes = languageStats.reduce((total, item) => total + item.bytes, 0);
  const contextDrift = contextAudit?.drift;
  const contextGrowthCount = contextDrift?.paid_agent_growth.length ?? 0;
  const contextBrokenRouteCount = contextDrift?.newly_broken_routes.length ?? 0;
  const contextUnusedFileCount = contextDrift?.unused_files.length ?? 0;
  const hasContextDriftSignals = contextGrowthCount > 0
    || contextBrokenRouteCount > 0
    || contextUnusedFileCount > 0;
  const contextSignalSummaries = [
    contextGrowthCount > 0 ? t('projects.contextAudit.growthSummary', contextGrowthCount) : null,
    contextBrokenRouteCount > 0 ? t('projects.contextAudit.brokenSummary', contextBrokenRouteCount) : null,
    contextUnusedFileCount > 0 ? t('projects.contextAudit.orphanSummary', contextUnusedFileCount) : null,
  ].filter((summary): summary is string => summary !== null);
  const contextAuditDetailsId = `project-context-audit-details-${proj.id}`;
  const contextAuditWhyId = `project-context-audit-why-${proj.id}`;
  const projectReadOnly = proj.write_access?.status === 'ReadOnly';
  const primaryWritableRoot = proj.write_access?.writable_roots?.[0] ?? '';
  const projectParent = proj.path.replace(/[\\/][^\\/]+[\\/]?$/, '');
  const mcpSyncTone = proj.mcp_sync_report?.status === 'Written' || proj.mcp_sync_report?.status === 'Unchanged'
    ? 'success'
    : proj.mcp_sync_report?.status === 'ReadOnly' || proj.mcp_sync_report?.status === 'MissingSecrets'
      ? 'warning'
      : 'error';
  const languageCheckedTime = overviewGit?.languages_checked_at
    ? new Date(overviewGit.languages_checked_at).toLocaleTimeString(locale, {
      hour: '2-digit',
      minute: '2-digit',
    })
    : null;
  const incompleteDependencyChecks = dependencyUpdates?.managers.filter(
    manager => manager.status === 'Unsupported'
      || manager.status === 'Unavailable'
      || manager.status === 'Error'
      || manager.status === 'TimedOut',
  ).length ?? 0;
  const dependencyCheckedAt = dependencyUpdates?.checked_at
    ? new Date(dependencyUpdates.checked_at).toLocaleString(locale, {
      dateStyle: 'medium',
      timeStyle: 'short',
    })
    : null;
  const dependencyNextCheckAt = dependencyUpdates?.next_check_at
    ? new Date(dependencyUpdates.next_check_at).toLocaleString(locale, {
      dateStyle: 'medium',
      timeStyle: 'short',
    })
    : null;
  const dependencySummary = (() => {
    if (dependencyUpdatesLoading) {
      return { tone: 'loading', label: t('projects.master.overview.dependenciesChecking') };
    }
    if (dependencyUpdatesError || !dependencyUpdates) {
      return { tone: 'muted', label: t('projects.master.overview.dependenciesUnavailable') };
    }
    if (dependencyUpdates.managers.length === 0) {
      return { tone: 'muted', label: t('projects.master.overview.dependenciesNone') };
    }
    if (dependencyUpdates.total_outdated > 0) {
      return {
        tone: 'warning',
        label: t('projects.master.overview.dependencyOutdatedCount', dependencyUpdates.total_outdated),
      };
    }
    if (incompleteDependencyChecks > 0) {
      return {
        tone: 'muted',
        label: t('projects.master.overview.dependenciesPartial', incompleteDependencyChecks),
      };
    }
    return { tone: 'success', label: t('projects.master.overview.dependenciesUpToDate') };
  })();
  const gitSync = (() => {
    if (overviewGitLoading) {
      return { tone: 'loading', label: t('projects.master.overview.gitLoading') };
    }
    if (overviewGitError || !overviewGit) {
      return { tone: 'muted', label: t('projects.master.overview.gitUnavailable') };
    }
    if (!overviewGit.has_upstream) {
      return {
        tone: repositoryUrl ? 'warning' : 'muted',
        label: repositoryUrl
          ? t('projects.master.overview.noUpstream')
          : t('projects.master.overview.localOnly'),
      };
    }
    if (overviewGit.ahead > 0 && overviewGit.behind > 0) {
      return {
        tone: 'warning',
        label: t('projects.master.overview.diverged', overviewGit.ahead, overviewGit.behind),
      };
    }
    if (overviewGit.behind > 0) {
      return {
        tone: 'warning',
        label: t('projects.master.overview.behind', overviewGit.behind),
      };
    }
    if (overviewGit.ahead > 0) {
      return {
        tone: 'info',
        label: t('projects.master.overview.ahead', overviewGit.ahead),
      };
    }
    return { tone: 'success', label: t('projects.master.overview.upToDate') };
  })();
  // Pulse the "add plugins" hint when the project has zero MCPs AND hasn't
  // been audited yet — plugins dramatically improve briefing + audit quality
  // (tracker context, stack detection, MCP-aware questions) so the UI
  // actively suggests adding some before either flow is launched.
  const shouldPulseMcpHint = projMcps.length === 0
    && (proj.audit_status === 'NoTemplate' || proj.audit_status === 'TemplateInstalled' || proj.audit_status === 'Bootstrapped');

  // 0.8.2 — Tracker-MCP hint. The audit Phase 3 + AutoPilot workflow get
  // dramatically more useful when a GitHub/GitLab/Jira/Linear MCP is
  // wired (real ticket creation, real issue context, "fetch_issue" step
  // pointing at a real plugin). `isTrackerMcp` mirrors the backend's
  // detection (`detect_issue_tracker_mcp` in helpers.rs:325).
  const trackerMcps = projMcps.filter(c =>
    isTrackerMcp(c.server_name) || isTrackerMcp(c.server_id ?? ''),
  );
  // Dismissible per project so users who genuinely don't want a tracker
  // (perso projects with no issue queue) can hide the hint forever.
  const trackerHintDismissKey = `kronn:trackerHintDismissed:${proj.id}`;
  const [trackerHintDismissed, setTrackerHintDismissed] = useState<boolean>(() => {
    try { return localStorage.getItem(trackerHintDismissKey) === '1'; } catch { return false; }
  });
  const dismissTrackerHint = () => {
    try { localStorage.setItem(trackerHintDismissKey, '1'); } catch { /* swallow quota / private-mode */ }
    setTrackerHintDismissed(true);
  };
  // Show this preparation step before the briefing whenever no tracker is
  // wired. It remains optional and dismissible for projects without an issue
  // queue, but a fresh project must not hide it behind the template install.
  const shouldShowTrackerHint = !trackerHintDismissed
    && trackerMcps.length === 0;

  const handleDeleteProject = async (id: string, hard: boolean) => {
    await projectsApi.delete(id, hard);
    setDeleteConfirmId(null);
    setDeleteConfirmInput('');
    onRefetch();
  };

  // Trigger the ai/ → docs/ migration. Backend handles git mv + ref
  // rewriting + optional symlink. On success we hold the banner in
  // a "✓ Migré" state for ~1.6s, THEN refetch so the operator sees
  // confirmation before the banner disappears entirely (without the
  // dwell, the banner vanishes faster than a toast registers).
  const handleMigrateDocs = async () => {
    setMigrating(true);
    setMigrationError(null);
    setMigrationSuccess(null);
    try {
      const res = await projectsApi.migrateDocs(proj.id, { create_symlink: migrationCreateSymlink });
      if (res.status === 'Failed') {
        setMigrationError(res.reason ?? t('migration.failedGeneric'));
        toast(t('migration.failedToast', proj.name), 'error');
      } else if (res.status === 'Migrated') {
        const filesMoved = res.files_moved ?? 0;
        setMigrationSuccess({ filesMoved });
        toast(t('migration.successToast', String(filesMoved)), 'success');
        // Hold the green confirmation row, then refetch.
        window.setTimeout(() => onRefetch(), 1600);
      } else if (res.status === 'AlreadyMigrated') {
        toast(t('migration.alreadyToast'), 'info');
        onRefetch();
      } else {
        // NotApplicable — also refetch so the stale banner goes away.
        onRefetch();
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setMigrationError(msg);
      toast(t('migration.failedToast', proj.name), 'error');
    } finally {
      setMigrating(false);
    }
  };

  // Point the project at a new directory. The backend validates the path
  // exists (and rejects `..` traversal) before updating the row, so a bad
  // value comes back as a thrown error we surface inline. On success we
  // refetch — `enrich_audit_status` re-checks the path and the banner clears.
  const handleRemap = useCallback(async () => {
    const newPath = remapValue.trim();
    if (!newPath || remapGuard.current) return;
    remapGuard.current = true;
    setRemapping(true);
    setRemapError(null);
    try {
      await projectsApi.remapPath(proj.id, newPath);
      toast(t('projects.remap.successToast', proj.name), 'success');
      onRefetch();
    } catch (e) {
      setRemapError(e instanceof Error ? e.message : String(e));
    } finally {
      setRemapping(false);
      remapGuard.current = false;
    }
  }, [remapValue, proj.id, proj.name, onRefetch, toast, t]);

  // Clone the project's repo_url afresh and re-point the project at it. Uses
  // the optional remap input as the target parent directory when the user
  // typed one; otherwise the server picks an existing location. On success the
  // backend also re-syncs the project's plugins (MCP) + skills to the new path.
  const handleCloneRemap = useCallback(async () => {
    if (cloneGuard.current) return;
    cloneGuard.current = true;
    setCloning(true);
    setRemapError(null);
    try {
      const parentDir = remapValue.trim() || null;
      const res = await projectsApi.cloneAndRemap(proj.id, { parent_dir: parentDir });
      toast(t('projects.remap.cloneSuccessToast', proj.name, res.new_path), 'success');
      onRefetch();
    } catch (e) {
      setRemapError(e instanceof Error ? e.message : String(e));
    } finally {
      setCloning(false);
      cloneGuard.current = false;
    }
  }, [remapValue, proj.id, proj.name, onRefetch, toast, t]);

  // Stop polling the audit-status endpoint and drop the local checkpoint.
  // Called on done, error, cancel, and unmount — anywhere we know the
  // audit is no longer in-flight or we're leaving this card.
  const stopAuditPolling = useCallback(() => {
    if (auditPollRef.current) {
      clearInterval(auditPollRef.current);
      auditPollRef.current = null;
    }
  }, []);

  const handleCancelAudit = useCallback(async () => {
    auditAbortController?.abort();
    try {
      await projectsApi.cancelAudit(proj.id);
      toast(t('audit.cancelled'), 'success');
    } catch (e) {
      console.warn('Cancel audit failed:', e);
    }
    auditActiveRef.current = false;
    setAuditActive(false);
    setAuditAbortController(null);
    stopAuditPolling();
    clearAuditCheckpoint(proj.id);
    setAuditCompletedTick((tick) => tick + 1);
    onRefetch();
    onRefetchDiscussions();
  }, [auditAbortController, proj.id, toast, t, onRefetch, onRefetchDiscussions, stopAuditPolling]);

  const handleFullAudit = useCallback(async () => {
    // Guard against double-click — `setAuditActive(true)` flips the UI to
    // the progress panel synchronously, but a fast double-click can call
    // this handler twice before React re-renders, spawning two concurrent
    // SSE streams against the same project (template install races,
    // duplicate validation discs, …). The closure read of `auditActive`
    // is stale between two synchronous clicks, so a ref is the only
    // race-free guard — the second click reads the just-written ref and
    // bails out before the second SSE is dispatched.
    if (auditActiveRef.current) return;
    auditActiveRef.current = true;
    // Resume support. When a resumable interrupted run exists we resume it
    // by id — the server derives the kind AND the checkpoint from that row,
    // so client state can never graft onto the wrong pipeline (Codex #3).
    const resumeRunId = resumableAudit?.id ?? null;
    // Progress-bar total: the UI now exposes the complete chained audit only;
    // a resumed legacy targeted run still keeps the kind stored by the server.
    const effectiveKind = resumeRunId ? resumableAudit?.kind : 'Full';
    const controller = new AbortController();
    setAuditAbortController(controller);
    setAuditActive(true);
    setAuditStep(0);
    // Legacy targeted audits run a single step. Without this fallback the
    // progress bar would show 1/16 forever — visually
    // freezing as if the audit hung.
    const isSubAudit = effectiveKind !== undefined && effectiveKind !== 'Full';
    setAuditTotalSteps(isSubAudit ? 1 : 16);
    setAuditCurrentFile(t('audit.templateStep'));
    // 0.8.3 TD #274 — fallback wallclock seed so the elapsed chip
    // ticks during Phase 1 (template install + legacy migration)
    // BEFORE the `start` SSE event lands. Replaced by the backend's
    // authoritative value once `onAuditStart` fires.
    setAuditStartedAt(Date.now());
    setAuditLastStepTokens(null);
    setAuditTotalTokens(null);
    setAuditCurrentTool(null);
    setAuditToolCallCount(null);
    // Seed the resume checkpoint immediately so a tab-away during phase 1
    // (template install) still leaves a breadcrumb to poll against.
    const startedAt = new Date().toISOString();
    saveAuditCheckpoint({
      projectId: proj.id, kind: 'full_audit', startedAt,
      stepIndex: 0, totalSteps: 16, currentFile: null,
    });
    try {
      await projectsApi.fullAuditStream(proj.id, {
        agent: selectedAuditAgent,
        tier: auditTierChoice,
        // A fresh launch always runs the complete 16-step chain. Resume sends
        // only the run id so the server remains authoritative for legacy runs.
        kind: resumeRunId ? null : 'Full',
        resume_run_id: resumeRunId,
      }, {
        onTemplateInstalled: () => {},
        // 0.8.3 TD #274 — backend-authoritative wallclock for the
        // live elapsed counter. Overrides the local fallback so the
        // displayed value matches what the server uses internally
        // (avoids local-clock drift on long audits).
        onAuditStart: (_totalSteps, startedAtIso) => {
          const ms = Date.parse(startedAtIso);
          if (!Number.isNaN(ms)) setAuditStartedAt(ms);
        },
        onStepStart: (step, total, file) => {
          setAuditStep(step);
          setAuditTotalSteps(total);
          setAuditCurrentFile(file);
          // Mirror each step_start into localStorage so a remount can
          // pick up exactly where the server is.
          saveAuditCheckpoint({
            projectId: proj.id, kind: 'full_audit', startedAt,
            stepIndex: step, totalSteps: total, currentFile: file || null,
          });
        },
        onChunk: () => {},
        onStepDone: (_step, _success, tokens, _durationMs, totalTokens) => {
          // 0.8.3 TD #274 — surface per-step + cumulative tokens so
          // the operator sees `4,521 tk · 23,890 tk total` chips
          // live during the audit. `tokens` is the last step's
          // cost (max of input+output, NOT a sum); `totalTokens`
          // is the running sum maintained server-side. Both stay
          // null when the agent doesn't speak stream-json (Vibe,
          // Ollama direct) — the chips hide rather than show 0.
          if (typeof tokens === 'number') setAuditLastStepTokens(tokens);
          if (typeof totalTokens === 'number') setAuditTotalTokens(totalTokens);
          // 0.8.3 (#281) — clear current tool when step finishes so
          // the chip doesn't show stale "🔧 Read" on the next step.
          setAuditCurrentTool(null);
          // 0.8.4 (#319 / B3) — reset the per-step tool-call counter.
          setAuditToolCallCount(null);
        },
        // 0.8.3 (#281) — live token tick during a step. Backend
        // emits this every time it sees a `Usage` event in the
        // stream-json. Updates the same chip as `onStepDone` so
        // the counter ticks DURING the step instead of jumping at
        // the end.
        onStepProgress: (_step, stepTokens, totalTokensSoFar) => {
          setAuditLastStepTokens(stepTokens);
          if (totalTokensSoFar > 0) setAuditTotalTokens(totalTokensSoFar);
        },
        // 0.8.3 (#281) — name of the tool the agent just started
        // calling (Read, Glob, mcp__atlassian__..., …). Display
        // briefly as a chip — last-write-wins is fine; the user
        // mostly cares about "is something happening?".
        onToolCall: (_step, tool) => {
          setAuditCurrentTool(tool);
          // 0.8.4 (#319 / B3) — bump the per-step tool-call counter so
          // the chip reads `🔧 Tool (N)`. Increments on every tool
          // call regardless of name change — even Read → Read → Read
          // makes the count climb, which is what we want (user sees
          // forward motion in a tool-only phase).
          setAuditToolCallCount(prev => (prev ?? 0) + 1);
        },
        // 0.8.3 root-cause fix — the CLI exited 0, but validation
        // FAILED the step (target_file empty / truncated: agent crashed
        // mid-Write, or the sandbox blocked the write without the CLI
        // propagating it). The backend never rewrites the file
        // (re-running the step is the only repair path) and the run
        // will end non-green; this per-step toast tells the user which
        // step is the culprit.
        onStepWarning: (_step, file, reason, repaired) => {
          console.warn(`[audit] step warning on ${file}: ${reason} (repaired=${repaired})`);
          toast(
            t('audit.stepWarning', file, reason, repaired ? t('audit.stepWarningRepaired') : t('audit.stepWarningNotRepaired')),
            'error',
          );
        },
        // NON-terminal: the step closed with its own step_done and the run
        // continues — the done/interrupted terminal owns the cleanup UX.
        onStepError: (error) => {
          // The Full done has no interrupted toast (unlike the partial
          // flow), so this non-terminal toast is the user's only visible
          // signal for a failed step — no cleanup here.
          console.warn('Full audit step failed:', error);
          toast(t('audit.streamWarning', error), 'error');
        },
        // NON-terminal (e.g. drift baseline write failed): surface it, no
        // cleanup — a coherent done still follows.
        onWarning: (message) => { toast(t('audit.streamWarning', message), 'error'); },
        onValidationCreated: () => {},
        onDone: (discussionId) => {
          auditActiveRef.current = false;
          setAuditActive(false);
          setAuditAbortController(null);
          setAuditCompletedTick((t) => t + 1);
          clearAuditCheckpoint(proj.id);
          onRefetch();
          onRefetchDiscussions();
          if (discussionId) {
            toast(t('audit.fullAuditDone'), 'success');
            onAutoRunDiscussion(discussionId);
            onNavigate('discussions');
          }
        },
        onError: (error) => {
          // SSE `event: error` from the backend (agent crash, install
          // failure, etc.). Pre-fix this only logged + cleared the
          // checkpoint, leaving `auditActive=true` so the spinner
          // span forever and the cancel button stayed armed against
          // a run that wasn't going anywhere. Surface the failure +
          // refetch so the project card flips back to the right CTA.
          console.warn('Full audit error:', error);
          auditActiveRef.current = false;
          setAuditActive(false);
          setAuditAbortController(null);
          clearAuditCheckpoint(proj.id);
          toast(t('audit.streamError', error || 'unknown error'), 'error');
          setAuditCompletedTick((t) => t + 1);
          onRefetch();
        },
      }, controller.signal);
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') return;
      console.warn('Full audit failed:', e);
      auditActiveRef.current = false;
      setAuditActive(false);
      clearAuditCheckpoint(proj.id);
      setAuditCompletedTick((t) => t + 1);
    } finally {
      setAuditAbortController(null);
    }
  }, [selectedAuditAgent, auditTierChoice, proj.id, t, toast, onRefetch, onRefetchDiscussions, onAutoRunDiscussion, onNavigate, resumableAudit]);

  const startPartialAudit = useCallback(async (drift: DriftCheckResponse) => {
    if (auditActiveRef.current) return;
    auditActiveRef.current = true;
    const steps = drift.stale_sections.map(s => s.audit_step);
    const controller = new AbortController();
    setAuditAbortController(controller);
    setAuditActive(true);
    setAuditStep(0);
    setAuditTotalSteps(steps.length);
    setAuditCurrentFile('');
    const startedAt = new Date().toISOString();
    saveAuditCheckpoint({
      projectId: proj.id, kind: 'partial', startedAt,
      stepIndex: 0, totalSteps: steps.length, currentFile: null,
    });
    try {
      await projectsApi.partialAuditStream(proj.id, { agent: selectedAuditAgent, tier: auditTierChoice, steps }, {
        onStepStart: (step, total, file) => {
          setAuditStep(step);
          setAuditTotalSteps(total);
          setAuditCurrentFile(file);
          saveAuditCheckpoint({
            projectId: proj.id, kind: 'partial', startedAt,
            stepIndex: step, totalSteps: total, currentFile: file || null,
          });
        },
        onChunk: () => {},
        onStepDone: () => {},
        // NON-terminal: the step closes with its own step_done and the loop
        // continues — the `done interrupted` toast owns the terminal UX.
        onStepError: (error) => { console.warn('Partial audit step failed:', error); },
        // NON-terminal (post-commit baseline failure): surface it, no cleanup
        // — the `done complete` that follows stays the terminal signal.
        onWarning: (message) => { toast(t('audit.streamWarning', message), 'error'); },
        onDone: (info) => {
          // A user cancel aborts the fetch and the parser reports a clean
          // done with no payload — the cancel handler already owns that UX
          // (its own toast + cleanup via cancelAudit): never fall into the
          // interrupted branch on an abort.
          if (controller.signal.aborted) return;
          auditActiveRef.current = false;
          setAuditActive(false);
          setAuditAbortController(null);
          clearAuditCheckpoint(proj.id);
          setAuditCompletedTick((t) => t + 1);
          onRefetch();
          onRefetchDrift(proj.id);
          // A5 — a fully-successful partial created a SCOPED validation
          // discussion (the backend already spawned its agent — no auto-run
          // here or it would double-spawn): surface it like the Full flow.
          onRefetchDiscussions();
          // The runtime validator guarantees complete ⇒ discussionId — the
          // second condition only narrows the type, a payload without an id
          // never reaches here (refused as malformed, no legacy fallback).
          if (info?.status === 'complete' && info.discussionId) {
            toast(t('audit.partialValidationCreated', String(info.succeededSteps.length)), 'success');
            // Open AND navigate — same UX as the Full validation flow; no
            // auto-run (the backend already spawned the agent post-commit).
            onOpenDiscussion(info.discussionId);
            onNavigate('discussions');
          } else if (info?.status === 'no_change') {
            // Honest: nothing was rewritten, sections stay stale — and NO
            // "just relaunch" nudge (manual review/acceptance is a future
            // feature, not an available action).
            toast(t('audit.partialNoChange'), 'error');
          } else {
            // Interrupted: some sections stayed stale — never a green toast.
            toast(t('audit.partialInterrupted'), 'error');
          }
        },
        onError: (error) => {
          // Same fix as `handleFullAudit.onError` — without resetting
          // `auditActive`, the spinner stays on a dead run.
          console.warn('Partial audit error:', error);
          auditActiveRef.current = false;
          setAuditActive(false);
          setAuditAbortController(null);
          clearAuditCheckpoint(proj.id);
          toast(t('audit.partialStreamError', error || 'unknown error'), 'error');
          setAuditCompletedTick((t) => t + 1);
          onRefetch();
        },
      }, controller.signal);
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') return;
      console.warn('Partial audit failed:', e);
      auditActiveRef.current = false;
      setAuditActive(false);
      clearAuditCheckpoint(proj.id);
      setAuditCompletedTick((t) => t + 1);
    } finally {
      setAuditAbortController(null);
    }
  }, [selectedAuditAgent, auditTierChoice, proj.id, t, toast, onRefetch, onRefetchDrift, onRefetchDiscussions, onOpenDiscussion, onNavigate]);

  // ─── Audit resume on mount ───────────────────────────────────────────────
  // When a local checkpoint indicates an audit was in-flight (tab switch, page
  // navigation, browser reload), fetch the server-side status and paint the
  // progress bar without restarting the audit. Polls every 2 s until the
  // server reports `null` (done/cancelled/error) — then clear the checkpoint
  // and refetch the project so `audit_status` catches up.
  // Adopt an audit launched OUTSIDE this card (MCP bridge, CLI). The mount
  // poll below stops itself on idle cards, so an external launch that came
  // AFTER mount was invisible — the card even offered to start an audit
  // while one was running. The Dashboard fleet poll tells us one is live:
  // re-arm the same adoption poll and every existing badge/chip/CTA works.
  const [externalAdoptTick, setExternalAdoptTick] = useState(0);
  useEffect(() => {
    if (!externalAuditLive) return;
    if (auditActiveRef.current || auditPollRef.current) return; // already tracking
    resumeSettledRef.current = false;
    setExternalAdoptTick(t => t + 1);
  }, [externalAuditLive]);

  useEffect(() => {
    if (resumeSettledRef.current) return;
    resumeSettledRef.current = true;
    // 0.8.3 — fix: do NOT gate on `cp` presence. Pre-fix, a missing
    // localStorage checkpoint (storage cleared, cross-domain navigation,
    // dev-mode HMR wipe, page-refresh on a tab that never wrote one)
    // would skip the backend poll entirely → the user saw "Start audit"
    // even when an audit was actively running server-side. Now we ALWAYS
    // ask the backend once at mount; the checkpoint stays as a UX
    // optimization (seeds the panel before the network round-trip
    // completes) but is no longer a precondition.
    const cp = loadAuditCheckpoint(proj.id);

    let cancelled = false;

    const poll = async () => {
      try {
        // `api<T>()` unwraps ApiResponse and returns `T` directly (throws on
        // failure), so the data is an `AuditProgress | null`.
        const p = await projectsApi.auditStatus(proj.id);
        if (cancelled) return;
        if (p) {
          auditActiveRef.current = true;
          setAuditActive(true);
          setAuditStep(p.step_index);
          setAuditTotalSteps(p.total_steps);
          setAuditCurrentFile(p.current_file ?? '');
          // 0.8.2 TD #233 — surface elapsed for the live counter. Parse
          // once per poll; the per-second tick effect drives re-render.
          const startedMs = Date.parse(p.started_at);
          if (!Number.isNaN(startedMs)) {
            setAuditStartedAt(prev => (prev === startedMs ? prev : startedMs));
          }
          // Refresh the checkpoint so its age stays within the 1 h TTL.
          saveAuditCheckpoint({
            projectId: p.project_id,
            kind: (p.kind === 'partial' || p.kind === 'full' || p.kind === 'full_audit')
              ? (p.kind as AuditCheckpointKind)
              : 'full_audit',
            startedAt: p.started_at,
            stepIndex: p.step_index,
            totalSteps: p.total_steps,
            currentFile: p.current_file ?? null,
          });
          // 0.8.3 hotfix — re-seed the live chips from the poll so
          // they survive SSE buffer stalls AND page re-mounts.
          // Only overwrite when the backend has a value; otherwise
          // keep whatever the SSE stream last set (avoids flashing
          // null while the tracker resets between steps).
          if (typeof p.step_tokens === 'number') setAuditLastStepTokens(p.step_tokens);
          else if (p.step_tokens === null) setAuditLastStepTokens(null);
          if (typeof p.total_tokens_so_far === 'number') setAuditTotalTokens(p.total_tokens_so_far);
          if (typeof p.current_tool === 'string') setAuditCurrentTool(p.current_tool);
          else if (p.current_tool === null) setAuditCurrentTool(null);
          // 0.8.4 (#319 / B3) — re-seed the tool-call counter from
          // the poll snapshot. The SSE-driven counter survives buffer
          // stalls because the backend tracker holds the running
          // count; the frontend just mirrors it.
          if (typeof p.current_tool_call_count === 'number') setAuditToolCallCount(p.current_tool_call_count);
          else if (p.current_tool_call_count === null) setAuditToolCallCount(null);
        } else {
          // Server reports nothing → either the audit wrapped up while we
          // were away, the checkpoint is orphaned (server restart, etc.),
          // or this card is just idle (no audit, never had one).
          // Drop the checkpoint defensively, stop polling, and reset
          // local state IF it was active. Don't refetch on every idle
          // poll — that would spam the projects list endpoint when
          // every ProjectCard sits idle. Only refetch when we were
          // ACTIVELY showing an audit bar that just disappeared.
          clearAuditCheckpoint(proj.id);
          const wasActive = auditActiveRef.current;
          auditActiveRef.current = false;
          setAuditActive(false);
          setAuditStartedAt(null);
          // 0.8.3 (#274) — clear token chips when the audit wraps so
          // the next run starts from a clean slate instead of
          // briefly flashing the stale "23,890 tk total" from the
          // previous audit before the first step_done lands.
          setAuditLastStepTokens(null);
          setAuditTotalTokens(null);
          setAuditCurrentTool(null);
          setAuditToolCallCount(null);
          if (wasActive) setAuditCompletedTick((t) => t + 1);
          if (auditPollRef.current) {
            clearInterval(auditPollRef.current);
            auditPollRef.current = null;
          }
          if (wasActive) onRefetch();
        }
      } catch (err) {
        // Network hiccup — keep the checkpoint, keep polling. If the backend
        // is permanently gone the 1 h TTL will eventually retire the entry.
        console.warn('audit-status poll failed:', err);
      }
    };

    // 0.8.3 — when a checkpoint exists, seed the UI immediately so
    // the resume bar shows up without waiting for the first network
    // round-trip. When it doesn't, the first `poll()` below decides
    // whether to mount the bar based on what the backend actually
    // says — which is the authoritative source.
    if (cp) {
      auditActiveRef.current = true;
      queueMicrotask(() => {
        if (cancelled) return;
        setAuditActive(true);
        setAuditStep(cp.stepIndex);
        setAuditTotalSteps(cp.totalSteps);
        setAuditCurrentFile(cp.currentFile ?? '');
      });
    }

    // Fire one immediate poll. Whatever the local checkpoint says,
    // the BACKEND is the source of truth — if it has a live audit
    // for this project, the poll inside will flip `auditActive=true`
    // even without a local checkpoint. Then keep polling every 2 s
    // ONLY when an audit is actually live (the interval is stopped
    // by the `else` branch of `poll()` when the server reports
    // `null`, so idle cards don't burn network).
    poll();
    auditPollRef.current = setInterval(poll, 2000);

    return () => {
      cancelled = true;
      if (auditPollRef.current) {
        clearInterval(auditPollRef.current);
        auditPollRef.current = null;
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [proj.id, externalAdoptTick]);

  // Defensive cleanup: stop any lingering polling when the card unmounts.
  useEffect(() => {
    return () => {
      if (auditPollRef.current) {
        clearInterval(auditPollRef.current);
        auditPollRef.current = null;
      }
    };
  }, []);

  // 0.8.2 TD #233 — drive the elapsed counter. Only ticks while the audit
  // is active to avoid a useless 1s re-render loop on idle cards. The
  // interval triggers re-renders; the displayed value is computed inline
  // from `auditStartedAt` so the counter stays a real wall-clock value
  // (no skew if the tick misses by a few ms).
  useEffect(() => {
    if (!auditActive || auditStartedAt === null) return;
    const id = setInterval(() => setAuditNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [auditActive, auditStartedAt]);

  const auditAgentPicker = auditAgents.length > 0 ? (
    <div className="dash-audit-agent-picker" data-testid="project-audit-agent-picker">
      <span className="text-xs text-dim">{t('disc.agentAndMode')}</span>
      <AgentSwitchPicker
        currentAgent={selectedAuditAgent}
        currentTier={auditTierChoice}
        availableAgents={auditAgents.map(agent => agent.agent_type)}
        modelTiers={modelTiers}
        defaultModelLabel={t('disc.defaultAgentModel')}
        tierLabels={{
          economy: t('disc.tier.economy'),
          default: t('disc.tier.default'),
          reasoning: t('disc.tier.reasoning'),
        }}
        onSelectionChange={async (agent, tier) => {
          setAuditAgentChoice(agent);
          setAuditTierChoice(tier);
        }}
        title={t('disc.agentAndMode')}
        ariaLabel={t('disc.agentAndMode')}
      />
    </div>
  ) : (
    <span className="text-xs text-dim">{t('disc.noAgent')}</span>
  );

  const auditLaunchButton = (
    <button
      className="dash-icon-btn dash-btn-accent-border"
      onClick={() => void handleFullAudit()}
      disabled={auditAgents.length === 0}
    >
      <Play size={12} /> {resumableAudit
        ? t('audit.resumeFromStep', resumableAudit.last_completed_step + 1)
        : t('audit.startFullAudit')}
    </button>
  );

  const auditLaunchControls = (
    <div className="dash-audit-launch-controls" data-testid="project-audit-launch-controls">
      {auditAgentPicker}
      {auditLaunchButton}
    </div>
  );

  const auditOutcomeLabel = latestAuditOutcome?.status === 'Completed'
    ? t('projects.docAi.auditRecap.status.completed')
    : latestAuditOutcome?.status === 'Interrupted'
      ? t('projects.docAi.auditRecap.status.interrupted')
      : latestAuditOutcome?.status === 'Failed'
        ? t('projects.docAi.auditRecap.status.failed')
        : latestAuditOutcome?.status === 'Cancelled'
          ? t('projects.docAi.auditRecap.status.cancelled')
          : latestAuditOutcome?.status ?? '';
  const auditOutcomeFailed = latestAuditOutcome !== null
    && latestAuditOutcome.status !== 'Completed';
  const openAuditOutcome = () => {
    if (!latestAuditOutcome) return;
    setSelectedAuditRunId(latestAuditOutcome.id);
    if (detailMode) {
      selectDetailView('audit');
      return;
    }
    if (!isOpen) onToggleOpen();
    setExpandedTab('aiContext');
  };

  return (
    <div
      id={detailMode ? `project-detail-${proj.id}` : `project-${proj.id}`}
      className={`dash-card${detailMode ? ' project-detail-card' : ''}`}
      data-active={isOpen || auditActive}
    >
      {detailMode && (
        <>
          <header className="project-detail-header collection-detail-header">
            <div className="project-detail-heading">
              <div className="project-detail-icon"><FileCode size={18} /></div>
              <div className="project-detail-title-block">
                <div className="project-detail-title-row">
                  <h2>{proj.name}</h2>
                  <CopyIdPill id={proj.id} title={t('projects.master.copyId', proj.name)} />
                  <ContextHelp title={t('contextHelp.project.title')}>
                    <p>{t('contextHelp.project.intro')}</p>
                    <ul>
                      <li>{t('contextHelp.project.code')}</li>
                      <li>{t('contextHelp.project.work')}</li>
                      <li>{t('contextHelp.project.resources')}</li>
                    </ul>
                    <p className="kr-context-help-agent-note">{t('contextHelp.project.mcp')}</p>
                  </ContextHelp>
                  {(auditActive || validationInProgress) && (
                    <Loader2 size={13} className="spin text-accent" aria-label={t('audit.activityInProgress')} />
                  )}
                </div>
                <div className="project-detail-path" title={proj.path}>{proj.path}</div>
              </div>
            </div>
            <div className="project-detail-actions">
              {dockerRunning && (
                <span className="project-status-chip" data-tone="success" title={t('projects.master.dockerUp')}>
                  <Container size={10} /> {t('projects.master.dockerUp')}
                </span>
              )}
              {projectReadOnly && (
                <span className="project-status-chip" data-tone="warning">
                  <AlertTriangle size={10} /> {t('projects.writeAccess.readOnlyShort')}
                </span>
              )}
              {proj.mcp_sync_report && (
                <span
                  className="project-status-chip"
                  data-tone={mcpSyncTone}
                  title={proj.mcp_sync_report.detail ?? undefined}
                >
                  <Plug size={10} /> {t(`projects.mcpSync.${proj.mcp_sync_report.status}`)}
                </span>
              )}
              <span
                className="project-status-chip"
                data-testid={auditActive ? 'project-audit-progress' : undefined}
                data-tone={auditActive || validationInProgress
                  ? 'running'
                  : proj.audit_status === 'Validated'
                    ? 'success'
                    : proj.audit_status === 'Audited'
                      ? 'warning'
                      : 'muted'}
              >
                {auditActive
                  ? <><Loader2 size={10} className="spin" /> {t('projects.master.status.auditRunning')} {auditStep}/{auditTotalSteps}</>
                  : validationInProgress
                    ? <><Loader2 size={10} className="spin" /> {t('projects.status.validating')}</>
                    : proj.audit_status === 'Validated'
                      ? <><ShieldCheck size={10} /> {t('projects.master.status.validated')}</>
                      : proj.audit_status === 'Audited'
                        ? <><ShieldCheck size={10} /> {t('projects.master.status.toValidate')}</>
                      : <><FileCode size={10} /> {t('projects.master.status.toPrepare')}</>}
              </span>
              {!auditActive && latestAuditOutcome && (
                <button
                  type="button"
                  className="project-alert-chip"
                  data-tone={auditOutcomeFailed ? 'error' : 'success'}
                  data-testid="project-audit-outcome"
                  onClick={openAuditOutcome}
                >
                  {auditOutcomeFailed ? <AlertTriangle size={10} /> : <ShieldCheck size={10} />}
                  {auditOutcomeLabel}
                </button>
              )}
              {!auditActive && driftStatus && driftStatus.stale_sections.length > 0 && (
                <>
                  <span
                    className="dash-badge-drift"
                    data-testid="project-drift-status"
                    title={driftStatus.stale_sections.map(section => section.ai_file).join(', ')}
                  >
                    <AlertTriangle size={9} />
                    {t('audit.staleSections', String(driftStatus.stale_sections.length))}
                  </span>
                  <button
                    type="button"
                    className="dash-drift-update-btn"
                    data-testid="project-drift-update"
                    onClick={() => startPartialAudit(driftStatus)}
                    title={t('audit.updateStale', String(driftStatus.stale_sections.length))}
                  >
                    <RefreshCw size={9} />
                    {t('audit.updateStale', String(driftStatus.stale_sections.length))}
                  </button>
                </>
              )}
              <button
                type="button"
                className="dash-icon-btn"
                onClick={() => {
                  onSetDiscPrefill({ projectId: proj.id, title: '', prompt: '' });
                  onNavigate('discussions');
                }}
              >
                <Plus size={12} /> {t('disc.newTitle')}
              </button>
            </div>
          </header>
          <nav className="project-detail-tabs" aria-label={t('projects.master.detailNav')}>
            {([
              ['overview', t('projects.master.tab.overview'), FileCode, undefined],
              ['discussions', t('projects.master.tab.discussions'), MessageSquare, projDiscussions.length],
              ['tasks', t('projects.master.tab.tasks'), ListTodo, projectTaskCount],
              ['audit', t('projects.master.tab.audit'), Cpu, undefined],
              ['docs', t('projects.master.tab.docs'), BookOpen, undefined],
              ['code', t('projects.master.tab.code'), Code2, undefined],
              ['docker', t('projects.master.tab.docker'), Container, undefined],
              ['git', t('projects.master.tab.git'), GitBranch, undefined],
              ['resources', t('projects.master.tab.resources'), Puzzle, undefined],
            ] as const).map(([view, label, Icon, count]) => (
              <button
                key={view}
                type="button"
                data-active={detailView === view}
                onClick={() => selectDetailView(view)}
              >
                <Icon size={13} /> {label}
                {count !== undefined && (
                  <span className="project-detail-tab-count">{count ?? '…'}</span>
                )}
              </button>
            ))}
          </nav>
        </>
      )}
      {/* The header used to be a <button>, but it contains a nested <button>
          (the drift-update CTA), which is invalid HTML and produces a React
          warning in dev. We keep the same a11y semantics via role="button"
          + Enter/Space keyboard handling, and the inner button keeps its
          stopPropagation guard so a click on it doesn't toggle the card. */}
      {!detailMode && <div
        className="dash-card-header"
        role="button"
        tabIndex={0}
        onClick={onToggleOpen}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            onToggleOpen();
          }
        }}
        aria-expanded={isOpen}
      >
        <ChevronRight size={14} style={{ color: 'var(--kr-accent-ink)', transform: isOpen ? 'rotate(90deg)' : 'none', transition: 'transform 0.2s' }} />
        <div className="flex-1">
          <div className="flex-row gap-3 flex-wrap">
            <span className="dash-proj-name">
              {proj.name}
              {/* 0.8.3 UX — spinner sur le titre quand une activité longue
                  (audit ou validation) tourne. Visible d'un coup d'œil sans
                  déplier la card ni lire les badges, surtout utile quand
                  l'user a 10+ projets et veut savoir lesquels mouline. Le
                  title prop permet le hover-info ("Audit en cours" / "Validation
                  en cours") pour la a11y / clarification. */}
              {(auditActive || validationInProgress) && (
                <Loader2
                  size={12}
                  className="dash-proj-name-spinner"
                  style={{ animation: 'spin 1s linear infinite' }}
                  aria-label={t('audit.activityInProgress')}
                />
              )}
            </span>
            {/* Project docs badge */}
            {proj.audit_status === 'NoTemplate' ? (
              <span className="dash-badge-gray"><FileCode size={9} /> Project docs</span>
            ) : (
              <span className="dash-badge-green"><FileCode size={9} /> Project docs</span>
            )}
            {dockerRunning && (
              <span className="dash-badge-green" title={t('projects.master.dockerUp')}>
                <Container size={9} /> {t('projects.master.dockerUp')}
              </span>
            )}
            {projectReadOnly && (
              <span className="dash-badge-orange" title={t('projects.writeAccess.readOnlyDesc', proj.path)}>
                <AlertTriangle size={9} /> {t('projects.writeAccess.readOnlyShort')}
              </span>
            )}
            {/* AI audit badge */}
            {auditActive ? (
              <span className="dash-badge-orange">
                <Loader2 size={9} style={{ animation: 'spin 1s linear infinite' }} /> AI audit {auditStep}/{auditTotalSteps}
              </span>
            ) : (proj.audit_status === 'Bootstrapped' || proj.audit_status === 'Audited' || proj.audit_status === 'Validated') ? (
              <span className="dash-badge-green"><Cpu size={9} /> AI audit</span>
            ) : proj.audit_status === 'TemplateInstalled' ? (
              <span className="dash-badge-orange"><Cpu size={9} /> AI audit</span>
            ) : (
              <span className="dash-badge-gray"><Cpu size={9} /> AI audit</span>
            )}
            {/* Validated badge — hidden during an active audit (#326 / F6).
               When `auditActive=true`, the previous "Validated" / TD
               count / audit-date badges describe a stale state and
               confuse the user ("Validated but in progress?"). The
               "AI audit X/16" orange badge above is the only truth
               while a run is in flight. */}
            {!auditActive && proj.audit_status === 'Validated' ? (
              <span className="dash-badge-green"><ShieldCheck size={9} /> Validated</span>
            ) : !auditActive && validationInProgress ? (
              <span className="dash-badge-orange cursor-pointer" onClick={(e) => { e.stopPropagation(); if (validationDisc) onOpenDiscussion(validationDisc.id); onNavigate('discussions'); }}>
                <Loader2 size={9} style={{ animation: 'spin 1s linear infinite' }} /> Validation
              </span>
            ) : !auditActive && (proj.audit_status === 'Audited' || proj.audit_status === 'TemplateInstalled') ? (
              <span className="dash-badge-gray"><ShieldCheck size={9} /> Validated</span>
            ) : null}
            {!auditActive && latestAuditOutcome && (
              <button
                type="button"
                className={auditOutcomeFailed ? 'dash-badge-orange' : 'dash-badge-green'}
                data-testid="project-audit-outcome"
                onClick={(e) => { e.stopPropagation(); openAuditOutcome(); }}
              >
                {auditOutcomeFailed ? <AlertTriangle size={9} /> : <ShieldCheck size={9} />}
                {auditOutcomeLabel}
              </button>
            )}
            {/* Tech-debt count badge. 0.8.1: surfaced so users can spot
                projects with known issues at a glance. Counts both
                detail files under `docs/tech-debt/` and table rows
                in `docs/inconsistencies-tech-debt.md`. Click jumps to
                the docs viewer with the tech-debt section open. */}
            {/* 0.8.7 — Anti-hallu canonical section badge. Green check
                when present in docs/AGENTS.md, amber inject CTA when
                missing. Hidden during audit (the audit itself will
                refresh the section via STEP 0). i18n FR/EN/ES. */}
            {!auditActive && antiHalluStatus !== null && (
              antiHalluStatus.present ? (
                <span
                  className="dash-badge-green"
                  title={antiHalluStatus.audit_date
                    ? t('projects.antiHallu.refreshed', antiHalluStatus.audit_date)
                    : t('projects.antiHallu.present')}
                >
                  <ShieldCheck size={9} /> {t('projects.antiHallu.present')}
                </span>
              ) : antiHalluStatus.file_exists ? (
                <span
                  className="dash-badge-orange cursor-pointer"
                  onClick={(e) => { e.stopPropagation(); handleInjectAntiHallu(); }}
                  title={t('projects.antiHallu.injectTooltip')}
                >
                  {antiHalluBusy
                    ? <><Loader2 size={9} style={{ animation: 'spin 1s linear infinite' }} /> {t('projects.antiHallu.injecting')}</>
                    : <><AlertTriangle size={9} /> {t('projects.antiHallu.missing')}</>
                  }
                </span>
              ) : null
            )}
            {!auditActive && (proj.tech_debt_count ?? 0) > 0 && (
              <span
                className="dash-badge-tech-debt"
                title={t('projects.techDebtBadge', proj.tech_debt_count ?? 0)}
                onClick={(e) => {
                  e.stopPropagation();
                  // 0.8.1 UX fix: also expand the card itself if it's
                  // collapsed. Without this, the badge sets the docAi
                  // tab but the user sees nothing because the card body
                  // is gated on `isOpen` — looked like a broken button.
                  if (!isOpen) onToggleOpen();
                  // Open the docs section and deep-link the viewer to the
                  // tech-debt folder so the user lands one click away from
                  // the items. The viewer auto-selects the first file
                  // under `docs/tech-debt/` on mount.
                  selectDetailView('docs');
                  setExpandedTab('docAi');
                  setDocDeepLink('docs/tech-debt');
                }}
              >
                <AlertTriangle size={9} /> {proj.tech_debt_count} TD
              </span>
            )}
            {/* Drift badge — hidden during active audit (#326 / F6) */}
            {!auditActive && driftStatus && driftStatus.stale_sections.length > 0 && (
              <>
                <span
                  className="dash-badge-drift"
                  title={driftStatus.stale_sections.map(s => s.ai_file).join(', ')}
                >
                  <AlertTriangle size={9} />
                  {t('audit.staleSections', String(driftStatus.stale_sections.length))}
                </span>
                <button
                  className="dash-drift-update-btn"
                  onClick={(e) => {
                    e.stopPropagation();
                    startPartialAudit(driftStatus);
                  }}
                  disabled={auditActive}
                  title={t('audit.updateStale', String(driftStatus.stale_sections.length))}
                >
                  <RefreshCw size={9} />
                  {t('audit.updateStale', String(driftStatus.stale_sections.length))}
                </button>
              </>
            )}
            {/* Audit date — hidden during active audit (#326 / F6) */}
            {!auditActive && driftStatus?.audit_date && (
              <span className="dash-audit-date">
                {/* 0.8.4 (#324 / F4) — locale-aware format so the FR
                   user reads "15 mai 2026" and the EN user reads
                   "May 15, 2026" — pre-fix this used the browser's
                   raw `toLocaleDateString()` (no locale arg) which
                   produced US "5/15/2026" inside a FR app. */}
                {t('audit.auditDate', new Date(driftStatus.audit_date).toLocaleDateString(locale, { day: '2-digit', month: 'short', year: 'numeric' }))}
              </span>
            )}
          </div>
          <div className="dash-proj-path">{proj.path}</div>
        </div>
        <div className={`dash-proj-meta${isMobile ? ' flex-wrap' : ''}`}>
          <span className={`dash-meta-item ${projMcps.length <= 5 ? 'mcp-load-ok' : projMcps.length <= 10 ? 'mcp-load-warn' : 'mcp-load-danger'}`} title={projMcps.length <= 5 ? t('mcp.mcpLoadOk') : projMcps.length <= 10 ? t('mcp.mcpLoadWarn') : t('mcp.mcpLoadDanger')}><Puzzle size={12} /> {projMcps.length}</span>
          <span className="dash-meta-item"><MessageSquare size={12} /> {projDiscussions.length}</span>
        </div>
      </div>}

      {/* Remap banner — always visible (even on a collapsed card) when the
          project directory no longer resolves on disk. Lets the operator
          re-point the project after a cross-OS import without expanding it. */}
      {proj.path_exists === false && (
        <div
          className="dash-remap-banner"
          data-testid={`remap-banner-${proj.id}`}
          onClick={(e) => e.stopPropagation()}
        >
          <div className="dash-remap-icon" aria-hidden="true"><AlertTriangle size={16} /></div>
          <div className="dash-remap-content">
            <div className="dash-remap-title">{t('projects.remap.title')}</div>
            <div className="dash-remap-desc">{t('projects.remap.desc', proj.path)}</div>
            <div className="dash-remap-form">
              <input
                className="dash-remap-input"
                type="text"
                value={remapValue}
                placeholder={proj.repo_url ? t('projects.remap.placeholderWithClone') : t('projects.remap.placeholder')}
                onChange={(e) => { setRemapValue(e.target.value); if (remapError) setRemapError(null); }}
                onKeyDown={(e) => { if (e.key === 'Enter') handleRemap(); }}
                disabled={remapping || cloning}
                aria-label={t('projects.remap.title')}
              />
              <button
                type="button"
                className="dash-remap-btn"
                onClick={handleRemap}
                disabled={remapping || cloning || !remapValue.trim()}
              >
                {remapping
                  ? <><Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} /> {t('projects.remap.busy')}</>
                  : t('projects.remap.cta')}
              </button>
            </div>
            {/* Clone-and-remap — only when the project has a known repo URL.
                Re-clones the repo locally (using the linked Git credentials)
                and re-points the project at the clone, then re-syncs its
                plugins + skills. The remap input above doubles as an optional
                target parent directory. */}
            {proj.repo_url && (
              <div className="dash-remap-clone">
                <button
                  type="button"
                  className="dash-remap-clone-btn"
                  onClick={handleCloneRemap}
                  disabled={remapping || cloning}
                  title={t('projects.remap.cloneHint', proj.repo_url)}
                >
                  {cloning
                    ? <><Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} /> {t('projects.remap.cloneBusy')}</>
                    : <><DownloadCloud size={12} /> {t('projects.remap.cloneCta')}</>}
                </button>
                <span className="dash-remap-clone-from">{proj.repo_url}</span>
              </div>
            )}
            {remapError && (
              <div className="dash-remap-error" role="alert">
                <AlertTriangle size={11} /> {remapError}
              </div>
            )}
          </div>
        </div>
      )}

      {proj.path_exists !== false && projectReadOnly && (
        <div
          className="dash-write-access-banner"
          data-testid={`write-access-banner-${proj.id}`}
          onClick={(event) => event.stopPropagation()}
        >
          <div className="dash-remap-icon" aria-hidden="true"><AlertTriangle size={16} /></div>
          <div className="dash-remap-content">
            <div className="dash-remap-title">{t('projects.writeAccess.readOnlyTitle')}</div>
            <div className="dash-remap-desc">{t('projects.writeAccess.readOnlyDesc', proj.path)}</div>
            <div className="dash-write-access-actions">
              {primaryWritableRoot && (
                <button
                  type="button"
                  className="dash-remap-btn"
                  onClick={() => navigator.clipboard.writeText(`mv "${proj.path}" "${primaryWritableRoot}/"`).catch(() => {})}
                  title={t('projects.writeAccess.moveHint', primaryWritableRoot)}
                >
                  <FolderInput size={12} /> {t('projects.writeAccess.copyMove')}
                </button>
              )}
              <button
                type="button"
                className="dash-remap-clone-btn"
                onClick={() => navigator.clipboard.writeText(`KRONN_EXTRA_REPOS=${projectParent}`).catch(() => {})}
                title={t('projects.writeAccess.extraHint')}
              >
                <Copy size={12} /> {t('projects.writeAccess.copyExtra')}
              </button>
            </div>
            <div className="dash-write-access-restart">{t('projects.writeAccess.restart')}</div>
          </div>
        </div>
      )}

      {isOpen && (
        <div
          className="dash-card-body"
          data-detail-mode={detailMode}
          data-detail-view={detailView}
          onClick={(e) => e.stopPropagation()}
        >
          {detailMode && (
            <section className="project-detail-overview" data-testid="project-detail-overview">
              <div className="project-overview-context">
                <div>
                  <span>{t('projects.master.overview.updated')}</span>
                  <strong>{new Date(proj.updated_at).toLocaleDateString(locale, { day: '2-digit', month: 'short', year: 'numeric' })}</strong>
                </div>
                <div>
                  <span>{t('projects.master.overview.audit')}</span>
                  <strong>{driftStatus?.audit_date
                    ? new Date(driftStatus.audit_date).toLocaleDateString(locale, { day: '2-digit', month: 'short', year: 'numeric' })
                    : t('projects.master.overview.never')}</strong>
                </div>
                <div>
                  <span>{t('projects.master.overview.repository')}</span>
                  <strong>{proj.repo_url ? t('projects.master.overview.linked') : t('projects.master.overview.local')}</strong>
                </div>
              </div>
              <section className="project-context-audit" data-testid="project-context-audit">
                <header>
                  <div>
                    <span className="project-overview-repository-icon" aria-hidden="true">
                      <Cpu size={17} />
                    </span>
                    <div>
                      <strong>{t('projects.contextAudit.title')}</strong>
                      <small>{t('projects.contextAudit.subtitle')}</small>
                    </div>
                  </div>
                  <button
                    type="button"
                    className="project-context-audit-refresh"
                    onClick={() => void refreshContextAudit()}
                    disabled={contextAuditLoading}
                    aria-label={t('projects.contextAudit.refresh')}
                    title={t('projects.contextAudit.refresh')}
                  >
                    <RefreshCw size={14} className={contextAuditLoading ? 'spin' : undefined} />
                  </button>
                </header>
                {contextAuditError ? (
                  <p className="project-context-audit-error">
                    <AlertTriangle size={13} /> {t('projects.contextAudit.unavailable')}
                  </p>
                ) : auditEvidence && contextAudit ? (
                  <>
                    <div className="project-context-audit-evidence" data-kind={auditEvidence.kind}>
                      <div>
                        {auditEvidence.kind === 'kronn_audit' || auditEvidence.kind === 'legacy_evidence'
                          ? <ShieldCheck size={15} />
                          : auditEvidence.kind === 'human_attestation'
                            ? <Check size={15} />
                            : <AlertTriangle size={15} />}
                        <p>
                          <strong>{t(`projects.contextAudit.evidence.${auditEvidence.kind}`)}</strong>
                          <span>{t(`projects.contextAudit.evidence.${auditEvidence.kind}.help`)}</span>
                        </p>
                      </div>
                      {auditEvidence.kind === 'missing_evidence' && (
                        <button
                          type="button"
                          className="project-context-audit-attest"
                          onClick={() => void attestExistingDocumentation()}
                          disabled={attestingDocumentation}
                        >
                          {attestingDocumentation
                            ? <Loader2 size={12} className="spin" />
                            : <Check size={12} aria-hidden="true" />}
                          {t('projects.contextAudit.attest')}
                        </button>
                      )}
                    </div>
                    <div
                      className="project-context-audit-summary"
                      data-tone={hasContextDriftSignals ? 'warning' : 'success'}
                    >
                      {hasContextDriftSignals
                        ? <AlertTriangle size={15} />
                        : <Check size={15} />}
                      <p>
                        {contextAudit.drift === null ? (
                          <>
                            <strong>{t('projects.contextAudit.baselineCreatedTitle')}</strong>
                            <span>{t('projects.contextAudit.baselineCreated')}</span>
                          </>
                        ) : hasContextDriftSignals ? (
                          <>
                            <span className="project-context-audit-summary-title">
                              <strong>{contextSignalSummaries[0]}</strong>
                              {contextGrowthCount > 0 && (
                                <button
                                  type="button"
                                  className="project-context-audit-why-toggle"
                                  aria-label={t('projects.contextAudit.whyTitle')}
                                  title={t('projects.contextAudit.whyTitle')}
                                  aria-expanded={contextAuditWhyOpen}
                                  aria-controls={contextAuditWhyId}
                                  onClick={() => setContextAuditWhyOpen(open => !open)}
                                >
                                  <CircleHelp size={13} aria-hidden="true" />
                                </button>
                              )}
                            </span>
                            {contextGrowthCount > 0 && contextAuditWhyOpen && (
                              <span id={contextAuditWhyId} className="project-context-audit-why">
                                {t('projects.contextAudit.whyHelp')}
                              </span>
                            )}
                            {contextGrowthCount > 0 && (
                              <span>{t('projects.contextAudit.growthHelp')}</span>
                            )}
                            {contextSignalSummaries.slice(1).map(summary => (
                              <span key={summary}>{summary}</span>
                            ))}
                          </>
                        ) : (
                          <>
                            <strong>{t('projects.contextAudit.stableTitle')}</strong>
                            <span>{t('projects.contextAudit.stable')}</span>
                          </>
                        )}
                      </p>
                      <button
                        type="button"
                        className="project-context-audit-details-toggle"
                        aria-expanded={contextAuditDetailsOpen}
                        aria-controls={contextAuditDetailsId}
                        onClick={() => setContextAuditDetailsOpen(open => !open)}
                      >
                        {t(contextAuditDetailsOpen
                          ? 'projects.contextAudit.hideDetails'
                          : 'projects.contextAudit.showDetails')}
                        <ChevronDown size={13} aria-hidden="true" />
                      </button>
                    </div>
                    {contextAuditDetailsOpen && (
                      <div id={contextAuditDetailsId} className="project-context-audit-details">
                        <p className="project-context-audit-paths">
                          {t('projects.contextAudit.paths', auditEvidence.state_file, auditEvidence.runtime_workspace)}
                        </p>
                        {auditEvidence.audit_runs > 0 && (
                          <div className="project-context-audit-reliability">
                            <span>{t('projects.contextAudit.runs', auditEvidence.audit_runs)}</span>
                            <span data-warning={auditEvidence.interrupted_runs > 0}>
                              {t(
                                'projects.contextAudit.interruptions',
                                auditEvidence.interrupted_runs,
                                auditEvidence.interruption_rate_percent.toFixed(1),
                              )}
                            </span>
                            {auditEvidence.resumable_after_step !== null && (
                              <span data-tone="success">
                                {t('projects.contextAudit.resume', auditEvidence.resumable_after_step + 1)}
                              </span>
                            )}
                          </div>
                        )}
                        {contextAudit.drift && hasContextDriftSignals && (
                          <div className="project-context-audit-signals">
                            {contextAudit.drift.paid_agent_growth.map(growth => (
                              <div key={growth.agent} data-tone="warning">
                                <AlertTriangle size={13} />
                                <span>
                                  {t(
                                    'projects.contextAudit.paidGrowth',
                                    growth.agent,
                                    growth.delta_bytes,
                                    growth.current_bytes,
                                  )}
                                </span>
                              </div>
                            ))}
                            {contextAudit.drift.newly_broken_routes.map(route => (
                              <div key={`broken-${route}`} data-tone="error">
                                <AlertTriangle size={13} />
                                <span>{t('projects.contextAudit.brokenRoute', route)}</span>
                              </div>
                            ))}
                            {contextAudit.drift.unused_files.map(path => (
                              <div key={`unused-${path}`} data-tone="warning">
                                <FileText size={13} />
                                <span>{t('projects.contextAudit.orphanFile', path)}</span>
                              </div>
                            ))}
                            <button
                              type="button"
                              className="project-context-audit-accept"
                              onClick={() => void acceptCurrentContextBaseline()}
                              disabled={acceptingContextBaseline}
                              title={t('projects.contextAudit.acceptBaselineHelp')}
                            >
                              {acceptingContextBaseline
                                ? <Loader2 size={12} className="spin" />
                                : <Check size={12} aria-hidden="true" />}
                              {t('projects.contextAudit.acceptBaseline')}
                            </button>
                          </div>
                        )}
                      </div>
                    )}
                  </>
                ) : (
                  <p className="project-context-audit-loading">
                    <Loader2 size={13} className="spin" /> {t('projects.contextAudit.loading')}
                  </p>
                )}
              </section>
              <div className="project-overview-repository" data-testid="project-overview-repository">
                <div className="project-overview-repository-head">
                  <div className="project-overview-repository-title">
                    <span className="project-overview-repository-icon" aria-hidden="true">
                      <GitBranch size={17} />
                    </span>
                    <div>
                      <span>{t('projects.master.overview.repository')}</span>
                      {repositoryUrl ? (
                        <a href={repositoryUrl} target="_blank" rel="noreferrer">
                          {repositoryUrl.replace(/^https?:\/\//, '')}
                          <ExternalLink size={11} />
                        </a>
                      ) : (
                        <strong>{t('projects.master.overview.local')}</strong>
                      )}
                    </div>
                  </div>
                  <div className="project-overview-repository-actions">
                    {repositoryUrl && (
                      <a href={repositoryUrl} target="_blank" rel="noreferrer">
                        <ExternalLink size={13} />
                        {t('projects.master.overview.openRepository')}
                      </a>
                    )}
                    {pullRequestsUrl && (
                      <a href={pullRequestsUrl} target="_blank" rel="noreferrer">
                        <GitPullRequest size={13} />
                        {overviewGit?.provider === 'gitlab'
                          ? t('projects.master.overview.mergeRequests')
                          : t('projects.master.overview.pullRequests')}
                      </a>
                    )}
                  </div>
                </div>
                <div className="project-overview-repository-meta">
                  <span className="project-overview-git-chip">
                    <GitBranch size={12} />
                    {overviewGit?.branch || t('projects.master.overview.unknownBranch')}
                  </span>
                  <span className="project-overview-git-chip">
                    <Tag size={12} />
                    {overviewGit?.last_tag || t('projects.master.overview.noTag')}
                  </span>
                  <span className="project-overview-git-chip" data-tone={gitSync.tone}>
                    <i aria-hidden="true" />
                    {gitSync.label}
                  </span>
                  {!!overviewGit?.files.length && (
                    <span className="project-overview-git-chip" data-tone="warning">
                      {t('projects.master.overview.localChanges', overviewGit.files.length)}
                    </span>
                  )}
                  {languageCheckedTime && (
                    <button
                      type="button"
                      className="project-overview-language-refresh"
                      data-cached={overviewGit?.languages_cached}
                      onClick={() => void refreshGitLanguages()}
                      disabled={overviewGitLoading}
                      aria-label={t('projects.master.overview.languagesRefresh')}
                      title={overviewGit?.languages_cached
                        ? t('projects.master.overview.languagesCachedAt', languageCheckedTime)
                        : t('projects.master.overview.languagesCheckedAt', languageCheckedTime)}
                    >
                      <RefreshCw size={11} className={overviewGitLoading ? 'is-spinning' : undefined} />
                      {overviewGit?.languages_cached
                        ? t('projects.master.overview.languagesCachedShort', languageCheckedTime)
                        : languageCheckedTime}
                    </button>
                  )}
                </div>
                {/* KT-94 follow-up — the bar now arrives ~20 s AFTER the git
                    status (background computation). Rendering nothing until then
                    made the whole card jump when it landed; keep the slot at its
                    final height with a pending shimmer instead (CLS ≈ 0). */}
                {languageStats.length === 0 && (
                  <div className="project-overview-languages" aria-hidden="true">
                    <div className="project-overview-languages-title">
                      <strong>{t('projects.master.overview.languages')}</strong>
                      <span>{t('projects.master.overview.languagesPending')}</span>
                    </div>
                    <div className="project-overview-language-bar project-overview-language-bar--pending" />
                    <div className="project-overview-language-legend">
                      <span>
                        <i style={{ background: 'var(--kr-text-faint)' }} />
                        <strong>…</strong>
                      </span>
                    </div>
                  </div>
                )}
                {languageStats.length > 0 && languageTotalBytes > 0 && (
                  <div className="project-overview-languages">
                    <div className="project-overview-languages-title">
                      <strong>{t('projects.master.overview.languages')}</strong>
                      <span>{t('projects.master.overview.languagesHint')}</span>
                    </div>
                    <div
                      className="project-overview-language-bar"
                      role="img"
                      aria-label={t('projects.master.overview.languages')}
                    >
                      {languageStats.map(item => {
                        const percentage = (item.bytes / languageTotalBytes) * 100;
                        return (
                          <span
                            key={item.language}
                            style={{
                              width: `${percentage}%`,
                              background: LANGUAGE_COLORS[item.language] ?? 'var(--kr-text-faint)',
                            }}
                            title={`${item.language} · ${percentage.toFixed(1)} %`}
                          />
                        );
                      })}
                    </div>
                    <div className="project-overview-language-legend">
                      {languageStats.slice(0, 8).map(item => {
                        const percentage = (item.bytes / languageTotalBytes) * 100;
                        return (
                          <span key={item.language}>
                            <i style={{ background: LANGUAGE_COLORS[item.language] ?? 'var(--kr-text-faint)' }} />
                            <strong>{item.language}</strong>
                            {percentage.toFixed(1)} %
                          </span>
                        );
                      })}
                    </div>
                  </div>
                )}
              </div>
              <div className="project-overview-dependencies" data-testid="project-overview-dependencies">
                <div className="project-overview-dependencies-head">
                  <div className="project-overview-repository-title">
                    <span className="project-overview-repository-icon" aria-hidden="true">
                      <Package size={17} />
                    </span>
                    <div>
                      <span>{t('projects.master.overview.dependencies')}</span>
                      <strong>
                        {dependencySummary.label}
                        {!!dependencyUpdates?.total_outdated && dependencyUpdates.total_major > 0 && (
                          <span className="project-overview-dependency-major">
                            {' · '}
                            {t('projects.master.overview.dependencyMajorCount', dependencyUpdates.total_major)}
                          </span>
                        )}
                      </strong>
                    </div>
                  </div>
                  <button
                    type="button"
                    className="project-overview-dependencies-refresh"
                    onClick={() => void refreshDependencyUpdates()}
                    disabled={dependencyUpdatesLoading}
                    aria-label={t('projects.master.overview.dependenciesRefresh')}
                    title={t('projects.master.overview.dependenciesRefresh')}
                  >
                    <RefreshCw size={13} className={dependencyUpdatesLoading ? 'is-spinning' : undefined} />
                    {t('projects.master.overview.dependenciesRefresh')}
                  </button>
                </div>
                <div className="project-overview-dependencies-meta">
                  <span className="project-overview-git-chip" data-tone={dependencySummary.tone}>
                    <i aria-hidden="true" />
                    {dependencySummary.label}
                    {!!dependencyUpdates?.total_outdated && dependencyUpdates.total_major > 0 && (
                      <strong className="project-overview-dependency-major">
                        {' · '}
                        {t('projects.master.overview.dependencyMajorCount', dependencyUpdates.total_major)}
                      </strong>
                    )}
                  </span>
                  {dependencyUpdates?.cached && (
                    <span className="project-overview-git-chip">
                      {t('projects.master.overview.dependenciesCached')}
                    </span>
                  )}
                  {dependencyCheckedAt && (
                    <span className="project-overview-dependencies-date">
                      {t('projects.master.overview.dependenciesCheckedAt', dependencyCheckedAt)}
                    </span>
                  )}
                  {dependencyNextCheckAt && (
                    <span className="project-overview-dependencies-date">
                      {t('projects.master.overview.dependenciesNextCheckAt', dependencyNextCheckAt)}
                    </span>
                  )}
                  <label className="project-overview-dependencies-schedule">
                    <span>{t('projects.master.overview.dependenciesSchedule')}</span>
                    <select
                      value={dependencyUpdates?.monitoring_interval_days ?? 'manual'}
                      disabled={!dependencyUpdates || dependencyMonitoringSaving}
                      onChange={event => {
                        const value = event.currentTarget.value;
                        void updateDependencyMonitoring(
                          value === 'manual' ? null : Number(value),
                        );
                      }}
                    >
                      <option value="manual">
                        {t('projects.master.overview.dependenciesScheduleManual')}
                      </option>
                      <option value="7">
                        {t('projects.master.overview.dependenciesScheduleWeekly')}
                      </option>
                      <option value="14">
                        {t('projects.master.overview.dependenciesScheduleFortnightly')}
                      </option>
                      <option value="30">
                        {t('projects.master.overview.dependenciesScheduleMonthly')}
                      </option>
                    </select>
                  </label>
                </div>
                {!!dependencyUpdates?.managers.length && (
                  <div className="project-overview-dependency-list">
                    {dependencyUpdates.managers.map(manager => {
                      const packages = manager.packages ?? [];
                      const status = manager.status === 'UpdatesAvailable'
                        ? (
                          <>
                            {t('projects.master.overview.dependencyOutdatedCount', manager.outdated)}
                            {manager.major > 0 && (
                              <strong className="project-overview-dependency-major">
                                {' · '}
                                {t('projects.master.overview.dependencyMajorCount', manager.major)}
                              </strong>
                            )}
                          </>
                        )
                        : manager.status === 'UpToDate'
                          ? t('projects.master.overview.dependencyUpToDate')
                          : manager.status === 'Unsupported'
                            ? t('projects.master.overview.dependencyUnsupported')
                            : manager.status === 'Unavailable'
                              ? t('projects.master.overview.dependencyToolUnavailable')
                              : manager.status === 'TimedOut'
                                ? t('projects.master.overview.dependencyTimedOut')
                                : t('projects.master.overview.dependencyCheckFailed');
                      const tone = manager.status === 'UpdatesAvailable'
                        ? 'warning'
                        : manager.status === 'UpToDate'
                          ? 'success'
                          : 'muted';
                      return (
                        <div key={`${manager.manager}:${manager.manifest}`} className="project-overview-dependency-row">
                          <div>
                            <strong>{manager.manager}</strong>
                            <span>{manager.manifest}</span>
                            {!!packages.length && (
                              <small>
                                {packages.slice(0, 3).map((pkg, index) => (
                                  <span
                                    key={pkg.name}
                                    className={pkg.major ? 'project-overview-dependency-package-major' : undefined}
                                  >
                                    {index > 0 ? ' · ' : ''}
                                    {pkg.name} {pkg.current} → {pkg.latest}
                                  </span>
                                ))}
                                {manager.outdated > 3 && <span>{` · +${manager.outdated - 3}`}</span>}
                              </small>
                            )}
                          </div>
                          <span className="project-overview-git-chip" data-tone={tone}>
                            <i aria-hidden="true" />
                            {status}
                          </span>
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
              <div className="project-overview-grid">
                <button type="button" onClick={() => selectDetailView('discussions')}>
                  <MessageSquare size={16} />
                  <strong>{projDiscussions.length}</strong>
                  <span>{t('projects.master.tab.discussions')}</span>
                </button>
                <button type="button" onClick={() => selectDetailView('tasks')}>
                  <ListTodo size={16} />
                  <strong>{t('projects.master.overview.browse')}</strong>
                  <span>{t('projects.master.tab.tasks')}</span>
                </button>
                <button type="button" onClick={() => selectDetailView('audit')}>
                  <Cpu size={16} />
                  <strong>
                    {proj.audit_status === 'Validated'
                      ? t('projects.status.valid')
                      : proj.audit_status === 'Audited'
                        ? t('projects.status.auditOk')
                        : proj.audit_status === 'Bootstrapped'
                          ? t('projects.status.bootstrapped')
                          : proj.audit_status === 'TemplateInstalled'
                            ? t('projects.status.template')
                            : t('projects.status.none')}
                  </strong>
                  <span>{t('projects.master.tab.audit')}</span>
                </button>
                <button type="button" onClick={() => selectDetailView('docs')}>
                  <BookOpen size={16} />
                  <strong>{t('projects.master.overview.browse')}</strong>
                  <span>{t('projects.master.tab.docs')}</span>
                </button>
                <button type="button" onClick={() => selectDetailView('code')}>
                  <Code2 size={16} />
                  <strong>{t('projects.master.overview.browse')}</strong>
                  <span>{t('projects.master.tab.code')}</span>
                </button>
                <button type="button" onClick={() => {
                  selectDetailView('resources');
                  setExpandedTab('mcps');
                }}>
                  <Puzzle size={16} />
                  <strong>{projMcps.length}</strong>
                  <span>Plugins</span>
                </button>
                <button type="button" onClick={() => {
                  selectDetailView('resources');
                  setExpandedTab('workflows');
                }}>
                  <Workflow size={16} />
                  <strong>{projWorkflows.length}</strong>
                  <span>{t('projects.workflows')}</span>
                </button>
                <button type="button" onClick={() => {
                  selectDetailView('resources');
                  setExpandedTab('skills');
                }}>
                  <Zap size={16} />
                  <strong>{(proj.default_skill_ids ?? []).length}</strong>
                  <span>{t('projects.skills')}</span>
                </button>
                <button
                  type="button"
                  data-tone={(proj.tech_debt_count ?? 0) > 0 ? 'warning' : 'success'}
                  onClick={() => {
                    selectDetailView('docs');
                    setExpandedTab('docAi');
                    if ((proj.tech_debt_count ?? 0) > 0) setDocDeepLink('docs/tech-debt');
                  }}
                >
                  <AlertTriangle size={16} />
                  <strong>{proj.tech_debt_count ?? 0}</strong>
                  <span>{t('projects.master.sort.techDebt')}</span>
                </button>
                <button type="button" onClick={() => selectDetailView('audit')}>
                  <RefreshCw size={16} />
                  <strong>{driftStatus?.stale_sections.length ?? 0}</strong>
                  <span>{t('projects.master.overview.stale')}</span>
                </button>
                <button type="button" onClick={() => selectDetailView('resources')}>
                  <FolderInput size={16} />
                  <strong>{(proj.linked_repos ?? []).length}</strong>
                  <span>{t('linkedRepos.title')}</span>
                </button>
              </div>
            </section>
          )}
          {detailMode && detailView === 'code' && (
            <section className="project-detail-section project-detail-source" data-project-view="code">
              <ProjectCodePanel
                key={`${proj.id}:${gitRevision}`}
                projectId={proj.id}
                initialPath={codeInitialPath}
              />
            </section>
          )}
          {detailMode && detailView === 'docker' && (
            <section className="project-detail-section project-detail-docker" data-project-view="docker">
              <ProjectDockerPanel
                projectId={proj.id}
                toast={toast}
                onRunningChange={reportDockerRunning}
                onOpenConfig={(path) => {
                  setCodeInitialPath(path);
                  selectDetailView('code');
                }}
              />
            </section>
          )}
          {detailMode && detailView === 'git' && (
            <section className="project-detail-section project-detail-git" data-project-view="git">
              <ProjectGitPanel
                projectId={proj.id}
                onBranchChanged={() => {
                  setGitRevision(revision => revision + 1);
                  setOverviewGit(null);
                  onRefetch();
                }}
              />
            </section>
          )}
          {detailMode && detailView === 'tasks' && (
            <section className="project-detail-section" data-project-view="tasks">
              <ProjectTasksPanel
                projectId={proj.id}
                onOpenPlanning={(taskId) =>
                  onNavigate(taskId ? `planning:${taskId}` : 'planning')
                }
                onCountChange={setProjectTaskCount}
                toast={toast}
              />
            </section>
          )}
          {/* Docs migration banner — shown only on projects still using
              the legacy `ai/index.md` layout. Disappears the next time
              the project list is fetched after a successful migration. */}
          {proj.needs_docs_migration && (
            <div
              className="dash-migration-banner"
              data-testid={`migration-banner-${proj.id}`}
              data-state={migrationSuccess ? 'success' : migrating ? 'pending' : 'idle'}
            >
              <div className="dash-migration-icon" aria-hidden="true">
                {migrationSuccess
                  ? <Check size={16} />
                  : <FolderInput size={16} />}
              </div>
              <div className="dash-migration-content">
                <div className="dash-migration-title">{t('migration.title')}</div>
                <div className="dash-migration-desc">{t('migration.desc')}</div>
                <label className="dash-migration-checkbox">
                  <input
                    type="checkbox"
                    checked={migrationCreateSymlink}
                    onChange={e => setMigrationCreateSymlink(e.target.checked)}
                    disabled={migrating || !!migrationSuccess}
                  />
                  <span>{t('migration.symlink')}</span>
                </label>
                {migrationError && (
                  <div className="dash-migration-error" role="alert">
                    <AlertTriangle size={11} /> {migrationError}
                  </div>
                )}
                {migrating && (
                  <div className="dash-migration-progress" role="status">
                    <Loader2 size={11} style={{ animation: 'spin 1s linear infinite' }} />
                    {t('migration.inProgress')}
                  </div>
                )}
                {migrationSuccess && (
                  <div className="dash-migration-success" role="status">
                    <Check size={11} />
                    {t('migration.successInline', String(migrationSuccess.filesMoved))}
                  </div>
                )}
              </div>
              <button
                type="button"
                className="dash-migration-btn"
                onClick={handleMigrateDocs}
                disabled={migrating || !!migrationSuccess}
                data-testid={`migrate-docs-btn-${proj.id}`}
              >
                {migrating
                  ? <Loader2 size={11} style={{ animation: 'spin 1s linear infinite' }} />
                  : <Check size={11} />}
                {migrating
                  ? t('migration.ctaPending')
                  : migrationSuccess
                  ? t('migration.ctaDone')
                  : t('migration.cta')}
              </button>
            </div>
          )}

          {/* -- 1. Discussions -- */}
          <div className="dash-section project-detail-section" data-project-view="discussions">
            {recentProjectDiscussions.slice(0, visibleDiscussionCount).map(disc => (
              <div key={disc.id} className="dash-row">
                <div className="relative">
                  <div aria-hidden="true" className="dash-dot" data-on="true" />
                  <span className="dash-sr-only">
                    {t('config.enabled')}
                  </span>
                </div>
                <div className="flex-1 cursor-pointer" onClick={() => { onOpenDiscussion(disc.id); onNavigate('discussions'); }}>
                  <span className="dash-row-disc-title">
                    {isValidationDisc(disc.title) && <ShieldCheck size={10} className="text-accent" />}
                    {disc.title}
                  </span>
                  <span className="dash-row-disc-meta">
                    {unseenBasis(disc)} msg · {disc.agent}
                  </span>
                </div>
                <button className="dash-icon-btn" onClick={() => { onOpenDiscussion(disc.id); onNavigate('discussions'); }} aria-label="Open discussion">
                  <ChevronRight size={12} />
                </button>
              </div>
            ))}
            {recentProjectDiscussions.length === 0 && (
              <div className="dash-empty">{t('projects.master.discussions.empty')}</div>
            )}
            <div className="project-discussion-actions">
              {visibleDiscussionCount < recentProjectDiscussions.length && (
                <div
                  className="project-discussions-load-more"
                  onClick={event => {
                    if ((event.target as HTMLElement).closest('select')) return;
                    setVisibleDiscussionCount(count => (
                      discussionLoadAmount === 'all'
                        ? recentProjectDiscussions.length
                        : Math.min(
                          recentProjectDiscussions.length,
                          count + Number(discussionLoadAmount),
                        )
                    ));
                  }}
                >
                  <button type="button" className="project-discussions-load-action">
                    <ChevronDown size={12} />
                    {t('projects.master.discussions.loadPrefix')}
                  </button>
                  <select
                    className="project-discussions-load-select"
                    value={discussionLoadAmount}
                    onChange={event => {
                      setDiscussionLoadAmount(event.target.value as typeof discussionLoadAmount);
                    }}
                    aria-label={t('projects.master.discussions.loadAmountLabel')}
                  >
                    <option value="10">10</option>
                    <option value="50">50</option>
                    <option value="all">{t('projects.master.discussions.loadAll')}</option>
                  </select>
                  <span>{t('projects.master.discussions.loadSuffix')}</span>
                </div>
              )}
              <button
                type="button"
                className="dash-icon-btn"
                onClick={() => { onSetDiscPrefill({ projectId: proj.id, title: '', prompt: '' }); onNavigate('discussions'); }}
              >
                <Plus size={12} /> {t('disc.newTitle')}
              </button>
            </div>
          </div>

          {/* -- 2. Project documentation --
              0.8.1 UX: shown at every audit_status (not just Validated)
              so users can browse the partial doc, get oriented, and see
              the next step via the contextual banner below. Empty
              projects (NoTemplate) show just the banner with a CTA. */}
          {(() => {
            // Build the state-aware banner. Drives the user from the
            // current audit_status to the next action without leaving
            // the docs context.
            let banner: React.ReactNode = null;
            const status = proj.audit_status;
            // 0.8.3 UX — when an audit is RUNNING, override the static
            // CTA banners with a live "building in progress" notice so
            // the user understands why they're seeing template
            // placeholders instead of filled docs. Without this, the
            // banner would still say "Lance un audit IA pour…" which
            // is contradictory mid-audit. Takes priority over the
            // status-based branches below.
            if (auditActive) {
              banner = (
                <div className="dash-doc-banner dash-doc-banner-info">
                  <Loader2 size={11} style={{ animation: 'spin 1s linear infinite' }} />
                  <span>{t('projects.docAi.banner.auditRunning')}</span>
                </div>
              );
            } else if (status === 'NoTemplate' || status === 'TemplateInstalled') {
              banner = (
                <div className="dash-doc-banner dash-doc-banner-info">
                  <Cpu size={11} />
                  <span>{t('projects.docAi.banner.runAudit')}</span>
                </div>
              );
            } else if (status === 'Bootstrapped') {
              banner = (
                <div className="dash-doc-banner dash-doc-banner-info">
                  <Cpu size={11} />
                  <span>{t('projects.docAi.banner.bootstrapDone')}</span>
                </div>
              );
            } else if (status === 'Audited') {
              banner = (
                <div className="dash-doc-banner dash-doc-banner-warn">
                  <ShieldCheck size={11} />
                  <span>{t('projects.docAi.banner.validate')}</span>
                </div>
              );
            }
            return (
              <div className="dash-section project-detail-section" data-project-view="docs">
                {!detailMode && (
                  <button className="dash-collapsible-header" onClick={() => toggleSection('docAi')} aria-expanded={isSectionOpen('docAi')}>
                    {isSectionOpen('docAi') ? <ChevronDown size={12} className="flex-shrink-0" /> : <ChevronRight size={12} className="flex-shrink-0" />}
                    <BookOpen size={14} /> <span className="dash-section-title">{t('projects.docAi')}</span>
                  </button>
                )}
                {(detailMode ? detailView === 'docs' : isSectionOpen('docAi')) && (
                  <>
                  <AiDocViewer
                    projectId={proj.id}
                    initialExpandFolder={docDeepLink}
                    banner={banner}
                    onDiscussFile={(filePath) => {
                    // Tech-debt files get a resolution-oriented prompt
                    // (asks the agent to plan + execute the fix + update
                    // the TD entry) instead of the generic discuss
                    // template. Keeps both paths in one handler so the
                    // viewer doesn't need to know about the conditional.
                    const isTechDebt =
                      /\/tech-debt\//.test(filePath) &&
                      /\/TD-[^/]+\.md$/.test(filePath);
                    onSetDiscPrefill({
                      projectId: proj.id,
                      title: isTechDebt
                        ? `Fix: ${filePath.split('/').pop()?.replace('.md', '') ?? filePath}`
                        : `Doc: ${filePath.replace('docs/', '').replace('ai/', '')}`,
                      prompt: isTechDebt
                        ? t('projects.docAi.fixThisPrompt', filePath)
                        : t('projects.docAi.discussPrompt', filePath),
                    });
                    onNavigate('discussions');
                  }}
                />
                  </>
              )}
              </div>
            );
          })()}

          {/* -- 3. MCPs -- */}
          <div className="dash-section project-detail-section" data-project-view="resources">
            <button className="dash-collapsible-header" onClick={() => toggleSection('mcps')} aria-expanded={isSectionOpen('mcps')}>
              {isSectionOpen('mcps') ? <ChevronDown size={12} className="flex-shrink-0" /> : <ChevronRight size={12} className="flex-shrink-0" />}
              <Puzzle size={14} /> <span className="dash-section-title">Plugins</span>
              <span className="dash-count">{projMcps.length}</span>
            </button>
            {/* "Add plugins first" pulse hint — visible even when the section is
                collapsed so a user skimming the card doesn't miss it. Shown
                only when zero plugins AND no audit has run yet; once plugins
                exist or the audit is done, the hint disappears. */}
            {shouldPulseMcpHint && (
              <div className="dash-mcp-hint" role="note" aria-live="polite">
                <Zap size={14} className="dash-mcp-hint-icon" />
                <span className="dash-mcp-hint-text">{t('projects.mcpHint.beforeAudit')}</span>
                <button
                  type="button"
                  className="dash-mcp-hint-cta"
                  onClick={() => onNavigate('mcps')}
                >
                  {t('projects.mcpHint.cta')}
                </button>
              </div>
            )}
            {isSectionOpen('mcps') && (
              <>
                {projMcps.map(cfg => (
                  <div key={cfg.id} className="dash-row" style={{ cursor: 'pointer' }} onClick={() => onNavigate(`mcps:${cfg.id}`)}>
                    <div className="relative">
                      <div aria-hidden="true" className="dash-dot" data-on="true" />
                      <span className="dash-sr-only">
                        {t('config.enabled')}
                      </span>
                    </div>
                    <div className="flex-1">
                      <span className="dash-row-name">{cfg.server_name}</span>
                      <span className="dash-row-detail-sm">{cfg.label}</span>
                      {cfg.is_global && <span className="dash-row-global-tag">GLOBAL</span>}
                    </div>
                    <ChevronRight size={12} className="text-ghost" />
                  </div>
                ))}
                {projMcps.length === 0 && !shouldPulseMcpHint && (
                  <div className="dash-row-empty">
                    {t('projects.noMcp').split(' — ')[0]} — <button className="dash-icon-btn" style={{ fontSize: 11, color: 'var(--kr-accent-ink)', display: 'inline-flex' }} onClick={() => onNavigate('mcps')}>{t('projects.noMcp').split(' — ')[1]}</button>
                  </div>
                )}
              </>
            )}
          </div>

          {/* -- 4. Workflows -- */}
          <div className="dash-section project-detail-section" data-project-view="resources">
            <button className="dash-collapsible-header" onClick={() => toggleSection('workflows')} aria-expanded={isSectionOpen('workflows')}>
              {isSectionOpen('workflows') ? <ChevronDown size={12} className="flex-shrink-0" /> : <ChevronRight size={12} className="flex-shrink-0" />}
              <Workflow size={14} /> <span className="dash-section-title">{t('projects.workflows')}</span>
              <span className="dash-count">{projWorkflows.length}</span>
            </button>
            {isSectionOpen('workflows') && (
              <>
                {projWorkflows.map(wf => (
                  <div key={wf.id} className="dash-row">
                    <div className="relative">
                      <div aria-hidden="true" className="dash-dot" data-on={String(wf.enabled)} />
                      <span className="dash-sr-only">
                        {wf.enabled ? t('config.enabled') : t('config.disabled')}
                      </span>
                    </div>
                    <div className="flex-1">
                      <span className="dash-row-name">{wf.name}</span>
                      <span className="dash-row-detail-sm">
                        {wf.trigger_type} · {wf.step_count} step{wf.step_count > 1 ? 's' : ''}
                      </span>
                      {wf.last_run && (
                        <span className="dash-row-detail-sm" style={{ color: STATUS_COLORS[wf.last_run.status] ?? 'var(--kr-text-faint)' }}>
                          {wf.last_run.status}
                        </span>
                      )}
                    </div>
                    <button
                      className="dash-icon-btn"
                      onClick={() => onNavigate('workflows')}
                      title={t('projects.workflows')}
                      aria-label={t('projects.workflows')}
                    >
                      <ChevronRight size={12} />
                    </button>
                  </div>
                ))}
                {projWorkflows.length === 0 && (
                  <div className="dash-row-empty">
                    {t('projects.noWorkflows').split(' — ')[0]} — <button className="dash-icon-btn" style={{ fontSize: 11, color: 'var(--kr-accent-ink)', display: 'inline-flex' }} onClick={() => onNavigate('workflows')}>{t('projects.noWorkflows').split(' — ')[1]}</button>
                  </div>
                )}
              </>
            )}
          </div>

          {/* -- 5. Skills -- */}
          <div className="dash-section project-detail-section" data-project-view="resources">
            <button className="dash-collapsible-header" onClick={() => toggleSection('skills')} aria-expanded={isSectionOpen('skills')}>
              {isSectionOpen('skills') ? <ChevronDown size={12} className="flex-shrink-0" /> : <ChevronRight size={12} className="flex-shrink-0" />}
              <Zap size={14} /> <span className="dash-section-title">{t('projects.skills')}</span>
              <span className="dash-count">{(proj.default_skill_ids ?? []).length}</span>
            </button>
            {isSectionOpen('skills') && (
              <div style={{ paddingTop: 6 }}>
                <ProjectSkills
                  projectId={proj.id}
                  currentSkillIds={proj.default_skill_ids ?? []}
                  allSkills={allSkills}
                  onUpdate={() => { onRefetch(); onRefetchSkills(); }}
                />
              </div>
            )}
          </div>

          {/* -- 5b. Linked repos (0.8.3) — companion projects.
              Lives between Skills and AI Context because it's
              configuration that feeds INTO the agent's context
              (same conceptual layer as skills), and the audit
              pipeline picks it up at the same prompt-assembly
              point as briefing_notes. */}
          <div className="dash-section project-detail-section" data-project-view="resources">
            <button className="dash-collapsible-header" onClick={() => toggleSection('linkedRepos')} aria-expanded={isSectionOpen('linkedRepos')}>
              {isSectionOpen('linkedRepos') ? <ChevronDown size={12} className="flex-shrink-0" /> : <ChevronRight size={12} className="flex-shrink-0" />}
              <FolderInput size={14} /> <span className="dash-section-title">{t('linkedRepos.title')}</span>
              <span className="dash-count">{(proj.linked_repos ?? []).length}</span>
            </button>
            {isSectionOpen('linkedRepos') && (
              <ProjectLinkedRepos
                projectId={proj.id}
                currentRepos={proj.linked_repos ?? []}
                onUpdate={onRefetch}
              />
            )}
          </div>

          {/* -- 6. Audit -- */}
          <div className="dash-section project-detail-section" data-project-view="audit">
            {!detailMode && (
              <button className="dash-collapsible-header" onClick={() => toggleSection('aiContext')} aria-expanded={isSectionOpen('aiContext')}>
                {isSectionOpen('aiContext') ? <ChevronDown size={12} className="flex-shrink-0" /> : <ChevronRight size={12} className="flex-shrink-0" />}
                <FileCode size={14} /> <span className="dash-section-title">AI Context</span>
                <span className="dash-count">
                  {proj.audit_status === 'Validated' ? t('projects.status.valid') : validationInProgress ? t('projects.status.validating') : proj.audit_status === 'Audited' ? t('projects.status.auditOk') : proj.audit_status === 'Bootstrapped' ? t('projects.status.bootstrapped') : bootstrapInProgress ? t('projects.status.bootstrapping') : proj.audit_status === 'TemplateInstalled' ? t('projects.status.template') : t('projects.status.none')}
                </span>
              </button>
            )}
            {(detailMode ? detailView === 'audit' : isSectionOpen('aiContext')) && (
              <>
                {/* 0.8.4 — audit history panel (chips + per-step table).
                   Mounted here at the top of AI Context so it's directly
                   adjacent to the launcher row. Pre-fix it lived inside
                   "Documentation projet" which is a file browser, not an
                   audit surface — users had to expand the wrong section
                   to find their previous runs' timings. The panel
                   self-hides when history is empty (fresh project). */}
                <AuditRecapPanel
                  projectId={proj.id}
                  refreshTrigger={auditCompletedTick}
                  selectedRunId={selectedAuditRunId}
                />
                {(proj.audit_status === 'NoTemplate' || (proj.audit_status === 'TemplateInstalled' && !bootstrapInProgress)) && !auditActive && (
                  <div className="dash-audit-pad">
                    <p className="dash-audit-warning">
                      <AlertTriangle size={11} /> {proj.audit_status === 'NoTemplate' ? t('audit.noTemplate') : t('audit.description')}
                    </p>
                    {shouldShowTrackerHint && (
                      <div className="dash-tracker-hint" data-testid="audit-tracker-prerequisite">
                        <span className="dash-tracker-hint-text">
                          💡 {t('audit.trackerHint')}
                        </span>
                        <div className="dash-tracker-hint-actions">
                          <button className="dash-icon-btn" onClick={() => onNavigate('mcps')}>
                            <Plug size={12} /> {t('audit.trackerHintConfigure')}
                          </button>
                          <button
                            className="dash-icon-btn dash-tracker-hint-dismiss"
                            onClick={dismissTrackerHint}
                            title={t('audit.trackerHintDismiss')}
                          >
                            <X size={12} />
                          </button>
                        </div>
                      </div>
                    )}
                    {auditAgentPicker}
                    {briefingFormOpen && (
                      <BriefingForm
                        projectId={proj.id}
                        agent={briefingAgentPick}
                        tier={auditTierChoice}
                        onClose={() => setBriefingFormOpen(false)}
                        onSaved={(discId) => {
                          // 0.8.4 UX fix — single briefing flow. The form
                          // submits the answers AND spawns the AI review
                          // disc. Refetch the project list so the briefing
                          // notes pill updates, refetch discussions so the
                          // new disc shows up in the sidebar, then jump
                          // into the disc if it was created.
                          onRefetch();
                          onRefetchDiscussions();
                          if (discId) {
                            onAutoRunDiscussion(discId);
                            onNavigate('discussions');
                          }
                        }}
                        toast={toast}
                      />
                    )}
                    <div className="flex-row gap-4 mb-4">
                      {briefingDisc && !briefingDone ? (
                        <button
                          className="dash-icon-btn dash-btn-info"
                          onClick={() => { onOpenDiscussion(briefingDisc.id); onNavigate('discussions'); }}
                        >
                          <MessageSquare size={12} /> {t('audit.resumeBriefing')}
                        </button>
                      ) : !briefingDone ? (
                        // 0.8.4 UX fix — ONE entry point. The form is the
                        // canonical briefing flow now: fill it, save, AI
                        // reviews. Pre-fix we had 2 independent buttons
                        // ("Briefing formulaire" + "Briefing IA") that
                        // let users fork into inconsistent state. The
                        // form-only flow keeps the AI value (review +
                        // clarifications on ambiguous answers) while
                        // killing the "did I do both? did I do neither?"
                        // confusion.
                        <button
                          className="dash-icon-btn dash-btn-info"
                          onClick={() => setBriefingFormOpen(true)}
                          disabled={agents.filter(canRunBriefing).length === 0 || briefingStarting}
                          title={t('briefing.formBtnTooltip')}
                          data-testid="briefing-open-form-btn"
                        >
                          <FileText size={12} /> {t('briefing.formBtn')}
                        </button>
                      ) : (
                        <span className="dash-briefing-done">
                          <Check size={10} /> {t('audit.briefingDone')}
                        </span>
                      )}
                      {!briefingDone && (
                        <span className="dash-briefing-hint">
                          {t('audit.briefingDesc')}
                        </span>
                      )}
                    </div>
                    <p className="dash-audit-desc">
                      {t('audit.fullAuditDesc')}
                    </p>
                    <div className="flex-row gap-4" data-testid="project-audit-launch-controls">
                      {auditLaunchButton}
                    </div>
                  </div>
                )}

                {auditActive && (
                  <div className="dash-audit-pad">
                    <div className="flex-row gap-4 mb-4">
                      <Loader2 size={14} style={{ animation: 'spin 1s linear infinite' }} className="text-accent" />
                      <span className="dash-audit-step">
                        {t('audit.step', auditStep, auditTotalSteps, auditCurrentFile)}
                      </span>
                      {auditStartedAt !== null && (
                        <span className="text-2xs text-ghost" title={t('audit.elapsedTooltip')}>
                          {t('audit.elapsed', formatElapsedShort(Math.max(0, auditNow - auditStartedAt)))}
                        </span>
                      )}
                      {/* 0.8.3 (#274) — last step + cumulative token
                          chips. Both stay hidden until the first
                          step_done lands AND the agent reports usage
                          (Vibe/Ollama direct stream stay 0). Gives
                          the operator a live signal for "which step
                          should I optimize" without polling. */}
                      {auditLastStepTokens !== null && auditLastStepTokens > 0 && (
                        <span className="text-2xs text-ghost" title={t('audit.lastStepTokensTooltip')}>
                          {t('audit.lastStepTokens', auditLastStepTokens.toLocaleString())}
                        </span>
                      )}
                      {auditTotalTokens !== null && auditTotalTokens > 0 && (
                        <span className="text-2xs text-ghost" title={t('audit.totalTokensTooltip')}>
                          {t('audit.totalTokens', auditTotalTokens.toLocaleString())}
                        </span>
                      )}
                      {/* 0.8.3 (#281) — current tool the agent is
                          calling. Last-write-wins (the agent fires
                          tool_call as it goes); cleared on
                          step_done. Hidden when null so the chip
                          doesn't take space when the agent is
                          just thinking. */}
                      {auditCurrentTool && (
                        <span className="text-2xs text-ghost" title={t('audit.currentToolTooltip')}>
                          {t('audit.currentTool', auditCurrentTool)}
                          {/* 0.8.4 (#319 / B3) — show the running tool-call
                             count so the user sees forward motion during
                             long tool-only phases (e.g. Step 9 writes 25+
                             TD files without an intermediate `Usage` event). */}
                          {auditToolCallCount != null && auditToolCallCount > 0 && ` (${auditToolCallCount})`}
                        </span>
                      )}
                      <button
                        className="dash-icon-btn dash-btn-cancel"
                        onClick={handleCancelAudit}
                        title={t('audit.cancelAudit')}
                      >
                        <StopCircle size={12} /> {t('audit.cancelAudit')}
                      </button>
                    </div>
                    <div className="dash-progress-track">
                      <div className="dash-progress-fill" style={{
                        width: `${(auditStep / auditTotalSteps) * 100}%`,
                      }} />
                    </div>
                  </div>
                )}

                {bootstrapInProgress && bootstrapDisc && !auditActive && (
                  <div className="dash-audit-pad">
                    <p className="dash-audit-warning">
                      <Loader2 size={11} style={{ animation: 'spin 1s linear infinite' }} /> {t('audit.bootstrapInProgress')}
                    </p>
                    <button
                      className="dash-icon-btn dash-btn-accent-border"
                      onClick={() => { onOpenDiscussion(bootstrapDisc.id); onNavigate('discussions'); }}
                    >
                      <MessageSquare size={12} /> {t('audit.resumeBootstrap')}
                    </button>
                  </div>
                )}

                {proj.audit_status === 'Bootstrapped' && !auditActive && (
                  <div className="dash-audit-pad">
                    <p className="dash-audit-hint-accent">
                      <Rocket size={11} /> {t('audit.bootstrapDone')}
                    </p>
                    {auditLaunchControls}
                  </div>
                )}

                {proj.audit_status === 'Audited' && !auditActive && (
                  <div className="dash-audit-pad">
                    {validationInProgress && validationDisc ? (
                      <>
                        <p className="dash-audit-warning">
                          <Loader2 size={11} style={{ animation: 'spin 1s linear infinite' }} /> {t('audit.validationInProgress', unseenBasis(validationDisc))}
                        </p>
                        <p className="dash-audit-desc">
                          {t('audit.validationHint')}
                        </p>
                        <button
                          className="dash-icon-btn dash-btn-accent-border"
                          onClick={() => { onOpenDiscussion(validationDisc.id); onNavigate('discussions'); }}
                        >
                          <MessageSquare size={12} /> {t('audit.resumeValidation')}
                        </button>
                      </>
                    ) : (
                      <>
                        <p className="dash-audit-hint">
                          {t('audit.readyToValidate')}
                        </p>
                        <button
                          className="dash-icon-btn dash-btn-accent-border"
                          onClick={() => {
                            onSetDiscPrefill({
                              projectId: proj.id,
                              title: 'Validation audit AI',
                              prompt: t('audit.validationPrompt'),
                              locked: true,
                            });
                            onNavigate('discussions');
                          }}
                        >
                          <ShieldCheck size={12} /> {t('audit.validate')}
                        </button>
                      </>
                    )}
                    {/* Keep a complete re-audit available before validation. */}
                    {!validationInProgress && (
                      <div style={{ marginTop: 8 }}>
                        {auditLaunchControls}
                      </div>
                    )}
                  </div>
                )}

                {proj.audit_status === 'Validated' && !auditActive && (
                  <>
                  <div className="dash-audit-validated">
                    <ShieldCheck size={11} /> {t('audit.done')}
                    {/* 0.8.3 — quick access to the TD index post-validation. */}
                    {(proj.tech_debt_count ?? 0) > 0 && (
                      <button
                        type="button"
                        className="dash-audit-view-tds-btn"
                        onClick={(e) => {
                          e.stopPropagation();
                          if (!isOpen) onToggleOpen();
                          selectDetailView('docs');
                          setExpandedTab('docAi');
                          setDocDeepLink('docs/tech-debt');
                        }}
                      >
                        {t('audit.viewTechDebts', proj.tech_debt_count ?? 0)}
                      </button>
                    )}
                  </div>
                  <div className="dash-audit-pad">
                    <p className="dash-audit-desc">{t('audit.reAuditHint')}</p>
                    {auditLaunchControls}
                  </div>
                  </>
                )}
              </>
            )}
          </div>

          <div className="dash-delete-zone project-detail-section" data-project-view="resources">
            {deleteConfirmId === proj.id ? (
              <div>
                <div className="flex-row gap-4 mb-4">
                  <button
                    className="dash-soft-delete-btn"
                    onClick={() => handleDeleteProject(proj.id, false)}
                  >
                    {t('projects.deleteSoft')}
                  </button>
                </div>
                <div className="dash-delete-panel">
                  <div className="dash-delete-warn">
                    <AlertTriangle size={12} style={{ verticalAlign: 'middle', marginRight: 4 }} />
                    {t('projects.deleteHardWarn')}
                  </div>
                  <div className="dash-delete-label">{t('projects.deleteHardConfirmLabel')}</div>
                  <input
                    value={deleteConfirmInput}
                    onChange={e => setDeleteConfirmInput(e.target.value)}
                    placeholder={proj.name}
                    className="dash-delete-input"
                  />
                  <div className="flex-row gap-4">
                    <button
                      className="dash-danger-btn"
                      style={{ opacity: deleteConfirmInput === proj.name ? 1 : 0.4, pointerEvents: deleteConfirmInput === proj.name ? 'auto' : 'none' }}
                      onClick={() => handleDeleteProject(proj.id, true)}
                      disabled={deleteConfirmInput !== proj.name}
                    >
                      <Trash2 size={12} /> {t('projects.deleteHard')}
                    </button>
                    <button
                      className="dash-soft-delete-btn"
                      onClick={() => { setDeleteConfirmId(null); setDeleteConfirmInput(''); }}
                    >
                      {t('audit.cancelAudit')}
                    </button>
                  </div>
                </div>
              </div>
            ) : (
              <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
                <button className="dash-danger-btn" onClick={() => setDeleteConfirmId(proj.id)}>
                  <Trash2 size={12} /> {t('projects.delete')}
                </button>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
