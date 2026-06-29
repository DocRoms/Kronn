import '../pages/Dashboard.css';
import { useState, useCallback, useEffect, useRef } from 'react';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { useIsMobile } from '../hooks/useMediaQuery';
import { isValidationDisc, isBriefingDisc, isBootstrapDisc, isUsable, isTrackerMcp } from '../lib/constants';
import { AiDocViewer } from './AiDocViewer';
import { unseenBasis } from './SwipeableDiscItem';
import AuditRecapPanel from './AuditRecapPanel';
import type { AuditKind } from '../types/AuditKind';
import { ProjectSkills } from './ProjectSkills';
import { ProjectLinkedRepos } from './ProjectLinkedRepos';
import {
  saveAuditCheckpoint, loadAuditCheckpoint, clearAuditCheckpoint,
  type AuditCheckpointKind,
} from '../lib/audit-resume';
import type { Project, AgentDetection, AgentType, DriftCheckResponse, Discussion, Skill, McpConfigDisplay, WorkflowSummary } from '../types/generated';
import {
  ChevronRight, ChevronDown, Cpu, Workflow,
  Plus, Trash2, Zap,
  Loader2,
  MessageSquare, AlertTriangle,
  Play, FileCode, ShieldCheck, StopCircle, BookOpen, Rocket, Check, RefreshCw, Puzzle,
  FolderInput, Plug, X, FileText, DownloadCloud,
} from 'lucide-react';
import { BriefingForm } from './BriefingForm';

const STATUS_COLORS: Record<string, string> = {
  Pending: 'var(--kr-warning)', Running: 'var(--kr-cyan)', Success: 'var(--kr-success)',
  Failed: 'var(--kr-error)', Cancelled: 'var(--kr-cancelled)', WaitingApproval: 'var(--kr-accent-ink)',
};

/** Format a millisecond duration as `Xs` under 60s, `MmSSs` past 60s. */
function formatElapsedShort(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return s >= 60 ? `${Math.floor(s / 60)}m${String(s % 60).padStart(2, '0')}s` : `${s}s`;
}

/** Agents that can run audits/briefings (need filesystem access + CLI mode). Excludes Vibe (API-only). */
const canAudit = (a: AgentDetection) => isUsable(a) && a.agent_type !== 'Vibe';

export interface ProjectCardProps {
  project: Project;
  isOpen: boolean;
  onToggleOpen: () => void;
  discussions: Discussion[];
  driftStatus: DriftCheckResponse | undefined;
  agents: AgentDetection[];
  allSkills: Skill[];
  mcpConfigs: McpConfigDisplay[];
  workflows: WorkflowSummary[];
  configLanguage: string | null;
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
  project: proj,
  isOpen,
  onToggleOpen,
  discussions: projDiscussions,
  driftStatus,
  agents,
  allSkills,
  mcpConfigs,
  workflows,
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

  // ── Collapsible sections ──
  // 0.8.4 (#323 / F3) — on `Validated` and `Audited`, `aiContext` is
  // the action zone (relaunch audit, launch sub-audit, validate). The
  // pre-fix default sent the user to `discussions` which forced 2
  // clicks to reach the launcher (and was discovered confusing in
  // the Marc persona Playwright pass). Bootstrapped keeps
  // `discussions` because the briefing disc is the natural focus
  // before audit; NoTemplate / TemplateInstalled also use `aiContext`
  // since that's where the briefing form CTA lives.
  const defaultSection = (auditStatus: string) =>
    auditStatus === 'Bootstrapped' ? 'discussions' : 'aiContext';
  const [expandedTab, setExpandedTab] = useState<string | undefined>(undefined);
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
      const r = await projectsApi.injectAntiHallu(proj.id);
      // Refetch status to reflect the new state.
      const s = await projectsApi.antiHalluStatus(proj.id);
      setAntiHalluStatus(s);
      // Small toast via document title flash — minimal UX without
      // bringing in a full toast system here. The badge state change is
      // the primary signal.
      if (r?.status === 'ok') {
        // Browser console as last-resort feedback for debugging.
        console.info(`[anti-hallu] ${r.result} on ${proj.id}`);
      }
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
    let target: string | null = null;
    try {
      target = sessionStorage.getItem(`kronn:postValidation:${proj.id}`);
      if (target) sessionStorage.removeItem(`kronn:postValidation:${proj.id}`);
    } catch { /* private mode / quota — no deep-link */ }
    if (target) {
      setExpandedTab('docAi');
      setDocDeepLink(target);
    }
  }, [isOpen, proj.id]);

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
  const [, setAuditTick] = useState(0);
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
  // 0.8.4 (#287) — selected audit kind for the next launch. Defaults
  // to Full. Sub-audit options in the dropdown are disabled until the
  // project reaches Audited/Validated (running a Security/RGAA audit
  // before the baseline doc audit makes no sense — the agent has no
  // `inconsistencies-*.md` to refine against).
  const [auditKindChoice, setAuditKindChoice] = useState<AuditKind>('Full');
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
  // flip to "Reprendre Step N/10" when an Interrupted run is on file
  // for this project. `null` = no resumable run; otherwise the row.
  const [resumableAudit, setResumableAudit] = useState<{ id: string; last_completed_step: number; started_at: string } | null>(null);
  useEffect(() => {
    let cancelled = false;
    projectsApi.auditResumable(proj.id).then(row => {
      if (cancelled) return;
      setResumableAudit(row);
    }).catch(() => { /* idle on error — button stays "Lancer" */ });
    return () => { cancelled = true; };
  }, [proj.id, proj.audit_status, auditActive]);

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
  // Show the hint when:
  //   - audit hasn't run yet (NoTemplate / TemplateInstalled / Bootstrapped), OR
  //   - audit already ran (Audited / Validated) — pre-AutoPilot suggestion
  //   AND no tracker MCP is wired AND user hasn't dismissed.
  const shouldShowTrackerHint = !trackerHintDismissed
    && trackerMcps.length === 0
    && proj.audit_status !== 'NoTemplate'; // hide on truly fresh project — the empty-MCP pulse already nudges

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
    onRefetch();
    onRefetchDiscussions();
  }, [auditAbortController, proj.id, toast, t, onRefetch, onRefetchDiscussions, stopAuditPolling]);

  const handleFullAudit = useCallback(async (resumeFromOverride?: number, kindOverride?: AuditKind) => {
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
    // 0.8.3 (#311) — resume support. If the caller didn't pass
    // an explicit `resumeFromOverride` AND there is a resumable
    // interrupted run on file, transparently resume from there.
    // The button copy reflects this via `resumableAudit` in the
    // render block.
    const resumeFrom = resumeFromOverride ?? resumableAudit?.last_completed_step ?? 0;
    const controller = new AbortController();
    setAuditAbortController(controller);
    setAuditActive(true);
    setAuditStep(0);
    // 0.8.4 (#287) — sub-audits run a single targeted step (not 10).
    // Without this the progress bar shows 1/10 forever — visually
    // freezing as if the audit hung.
    const isSubAudit = kindOverride !== undefined && kindOverride !== 'Full';
    setAuditTotalSteps(isSubAudit ? 1 : 10);
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
      stepIndex: 0, totalSteps: 10, currentFile: null,
    });
    try {
      const auditAgent = auditAgentChoice ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode';
      await projectsApi.fullAuditStream(proj.id, {
        agent: auditAgent,
        // 0.8.4 (#287) — pass kind through so the backend routes to
        // SECURITY_STEPS / RGAA_STEPS / DATABASE_STEPS / etc. instead
        // of the default ANALYSIS_STEPS.
        kind: kindOverride ?? null,
        resume_from: resumeFrom > 0 ? resumeFrom : null,
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
        // 0.8.3 root-cause fix — the CLI exited 0 but the step's
        // target_file is empty / truncated (e.g. agent crashed
        // mid-Write or the sandbox blocked the file write but the
        // CLI didn't propagate). Backend already auto-repaired from
        // the template; we surface a per-step warning toast so the
        // user immediately knows the audit "succeeded" but this
        // particular step didn't produce useful output.
        onStepWarning: (_step, file, reason, repaired) => {
          console.warn(`[audit] step warning on ${file}: ${reason} (repaired=${repaired})`);
          toast(
            t('audit.stepWarning', file, reason, repaired ? t('audit.stepWarningRepaired') : t('audit.stepWarningNotRepaired')),
            'error',
          );
        },
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
          onRefetch();
        },
      }, controller.signal);
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') return;
      console.warn('Full audit failed:', e);
      auditActiveRef.current = false;
      setAuditActive(false);
      clearAuditCheckpoint(proj.id);
    } finally {
      setAuditAbortController(null);
    }
  }, [auditAgentChoice, agents, proj.id, t, toast, onRefetch, onRefetchDiscussions, onAutoRunDiscussion, onNavigate, resumableAudit]);

  const startPartialAudit = useCallback(async (drift: DriftCheckResponse) => {
    if (auditActiveRef.current) return;
    auditActiveRef.current = true;
    const steps = drift.stale_sections.map(s => s.audit_step);
    const controller = new AbortController();
    setAuditAbortController(controller);
    const auditAgent = auditAgentChoice ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode';
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
      await projectsApi.partialAuditStream(proj.id, { agent: auditAgent, steps }, {
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
        onDone: () => {
          auditActiveRef.current = false;
          setAuditActive(false);
          setAuditAbortController(null);
          clearAuditCheckpoint(proj.id);
          onRefetch();
          onRefetchDrift(proj.id);
          toast(t('audit.updateStale', String(steps.length)), 'success');
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
          onRefetch();
        },
      }, controller.signal);
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') return;
      console.warn('Partial audit failed:', e);
      auditActiveRef.current = false;
      setAuditActive(false);
      clearAuditCheckpoint(proj.id);
    } finally {
      setAuditAbortController(null);
    }
  }, [auditAgentChoice, agents, proj.id, t, toast, onRefetch, onRefetchDrift]);

  // ─── Audit resume on mount ───────────────────────────────────────────────
  // When a local checkpoint indicates an audit was in-flight (tab switch, page
  // navigation, browser reload), fetch the server-side status and paint the
  // progress bar without restarting the audit. Polls every 2 s until the
  // server reports `null` (done/cancelled/error) — then clear the checkpoint
  // and refetch the project so `audit_status` catches up.
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
      setAuditActive(true);
      setAuditStep(cp.stepIndex);
      setAuditTotalSteps(cp.totalSteps);
      setAuditCurrentFile(cp.currentFile ?? '');
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
  }, [proj.id]);

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
    const id = setInterval(() => setAuditTick(t => t + 1), 1000);
    return () => clearInterval(id);
  }, [auditActive, auditStartedAt]);

  return (
    <div id={`project-${proj.id}`} className="dash-card" data-active={isOpen || auditActive}>
      {/* The header used to be a <button>, but it contains a nested <button>
          (the drift-update CTA), which is invalid HTML and produces a React
          warning in dev. We keep the same a11y semantics via role="button"
          + Enter/Space keyboard handling, and the inner button keeps its
          stopPropagation guard so a click on it doesn't toggle the card. */}
      <div
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
               "AI audit X/10" orange badge above is the only truth
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
                title={t('projects.techDebtBadge', proj.tech_debt_count!)}
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
      </div>

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

      {isOpen && (
        <div className="dash-card-body" onClick={(e) => e.stopPropagation()}>
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
          <div className="dash-section">
            <button className="dash-collapsible-header" onClick={() => toggleSection('discussions')} aria-expanded={isSectionOpen('discussions')}>
              {isSectionOpen('discussions') ? <ChevronDown size={12} className="flex-shrink-0" /> : <ChevronRight size={12} className="flex-shrink-0" />}
              <MessageSquare size={14} /> <span className="dash-section-title">Discussions</span>
              <span className="dash-count">{projDiscussions.length}</span>
            </button>
            {isSectionOpen('discussions') && (
              <>
                {projDiscussions.slice(0, 3).map(disc => (
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
                <button
                  className="dash-icon-btn mt-4"
                  style={{ fontSize: 11, gap: 4 }}
                  onClick={() => { onSetDiscPrefill({ projectId: proj.id, title: '', prompt: '' }); onNavigate('discussions'); }}
                >
                  <Plus size={12} /> {t('disc.newTitle')}
                </button>
              </>
            )}
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
              <div className="dash-section">
                <button className="dash-collapsible-header" onClick={() => toggleSection('docAi')} aria-expanded={isSectionOpen('docAi')}>
                  {isSectionOpen('docAi') ? <ChevronDown size={12} className="flex-shrink-0" /> : <ChevronRight size={12} className="flex-shrink-0" />}
                  <BookOpen size={14} /> <span className="dash-section-title">{t('projects.docAi')}</span>
                </button>
                {isSectionOpen('docAi') && (
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
          <div className="dash-section">
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
          <div className="dash-section">
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
          <div className="dash-section">
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
          <div className="dash-section">
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

          {/* -- 6. AI Context / Audit -- */}
          <div className="dash-section">
            <button className="dash-collapsible-header" onClick={() => toggleSection('aiContext')} aria-expanded={isSectionOpen('aiContext')}>
              {isSectionOpen('aiContext') ? <ChevronDown size={12} className="flex-shrink-0" /> : <ChevronRight size={12} className="flex-shrink-0" />}
              <FileCode size={14} /> <span className="dash-section-title">AI Context</span>
              <span className="dash-count">
                {proj.audit_status === 'Validated' ? t('projects.status.valid') : validationInProgress ? t('projects.status.validating') : proj.audit_status === 'Audited' ? t('projects.status.auditOk') : proj.audit_status === 'Bootstrapped' ? t('projects.status.bootstrapped') : bootstrapInProgress ? t('projects.status.bootstrapping') : proj.audit_status === 'TemplateInstalled' ? t('projects.status.template') : t('projects.status.none')}
              </span>
            </button>
            {isSectionOpen('aiContext') && (
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
                />
                {(proj.audit_status === 'NoTemplate' || (proj.audit_status === 'TemplateInstalled' && !bootstrapInProgress)) && !auditActive && (
                  <div className="dash-audit-pad">
                    <p className="dash-audit-warning">
                      <AlertTriangle size={11} /> {proj.audit_status === 'NoTemplate' ? t('audit.noTemplate') : t('audit.description')}
                    </p>
                    {briefingFormOpen && (
                      <BriefingForm
                        projectId={proj.id}
                        agent={auditAgentChoice ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode'}
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
                          disabled={agents.filter(canAudit).length === 0 || briefingStarting}
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
                    {shouldShowTrackerHint && (
                      <div className="dash-tracker-hint">
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
                    <div className="flex-row gap-4">
                      <select
                        className="dash-audit-select"
                        value={auditAgentChoice ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode'}
                        onChange={e => setAuditAgentChoice(e.target.value as AgentType)}
                      >
                        {agents.filter(canAudit).map(a => (
                          <option key={a.agent_type} value={a.agent_type}>{a.name}</option>
                        ))}
                        {agents.filter(canAudit).length === 0 && (
                          <option value="" disabled>{t('disc.noAgent')}</option>
                        )}
                      </select>
                      {/* 0.8.4 (#287) — audit kind dropdown. Sub-audits stay
                         disabled until the project has a Full audit baseline
                         (Audited or Validated). Otherwise the targeted
                         agent would have no `inconsistencies-*.md` to
                         refine against, producing a half-baked re-discovery
                         instead of a focused re-scan. */}
                      <select
                        className="dash-audit-select"
                        data-testid="audit-kind-select"
                        value={auditKindChoice}
                        onChange={e => setAuditKindChoice(e.target.value as AuditKind)}
                        title={t('audit.kindSelector.tooltip')}
                      >
                        <option value="Full">{t('audit.kind.Full')}</option>
                        <option value="Security" disabled>{t('audit.kind.Security')} {t('audit.kind.afterFull')}</option>
                        <option value="Docker" disabled>{t('audit.kind.Docker')} {t('audit.kind.afterFull')}</option>
                        <option value="Performance" disabled>{t('audit.kind.Performance')} {t('audit.kind.afterFull')}</option>
                        <option value="Accessibility" disabled>{t('audit.kind.Accessibility')} {t('audit.kind.afterFull')}</option>
                        <option value="Rgaa" disabled>{t('audit.kind.Rgaa')} {t('audit.kind.afterFull')}</option>
                        <option value="Database" disabled>{t('audit.kind.Database')} {t('audit.kind.afterFull')}</option>
                        <option value="ApiDesign" disabled>{t('audit.kind.ApiDesign')} {t('audit.kind.afterFull')}</option>
                      </select>
                      <button
                        className="dash-icon-btn dash-btn-accent-border"
                        onClick={() => handleFullAudit(undefined, auditKindChoice)}
                        disabled={agents.filter(canAudit).length === 0}
                      >
                        <Play size={12} /> {resumableAudit
                          ? t('audit.resumeFromStep', resumableAudit.last_completed_step + 1)
                          : t('audit.kindSelector.launchLabel', t(`audit.kind.${auditKindChoice}`))}
                      </button>
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
                          {t('audit.elapsed', formatElapsedShort(Date.now() - auditStartedAt))}
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
                    <div className="flex-row gap-4">
                      <select
                        className="dash-audit-select"
                        value={auditAgentChoice ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode'}
                        onChange={e => setAuditAgentChoice(e.target.value as AgentType)}
                      >
                        {agents.filter(canAudit).map(a => (
                          <option key={a.agent_type} value={a.agent_type}>{a.name}</option>
                        ))}
                        {agents.filter(canAudit).length === 0 && (
                          <option value="" disabled>{t('disc.noAgent')}</option>
                        )}
                      </select>
                      <select
                        className="dash-audit-select"
                        data-testid="audit-kind-select-bootstrapped"
                        value={auditKindChoice}
                        onChange={e => setAuditKindChoice(e.target.value as AuditKind)}
                        title={t('audit.kindSelector.tooltip')}
                      >
                        <option value="Full">{t('audit.kind.Full')}</option>
                        <option value="Security" disabled>{t('audit.kind.Security')} {t('audit.kind.afterFull')}</option>
                        <option value="Docker" disabled>{t('audit.kind.Docker')} {t('audit.kind.afterFull')}</option>
                        <option value="Performance" disabled>{t('audit.kind.Performance')} {t('audit.kind.afterFull')}</option>
                        <option value="Accessibility" disabled>{t('audit.kind.Accessibility')} {t('audit.kind.afterFull')}</option>
                        <option value="Rgaa" disabled>{t('audit.kind.Rgaa')} {t('audit.kind.afterFull')}</option>
                        <option value="Database" disabled>{t('audit.kind.Database')} {t('audit.kind.afterFull')}</option>
                        <option value="ApiDesign" disabled>{t('audit.kind.ApiDesign')} {t('audit.kind.afterFull')}</option>
                      </select>
                      <button
                        className="dash-icon-btn dash-btn-accent-border"
                        onClick={() => handleFullAudit(undefined, auditKindChoice)}
                        disabled={agents.filter(canAudit).length === 0}
                      >
                        <Play size={12} /> {resumableAudit
                          ? t('audit.resumeFromStep', resumableAudit.last_completed_step + 1)
                          : t('audit.kindSelector.launchLabel', t(`audit.kind.${auditKindChoice}`))}
                      </button>
                    </div>
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
                    {/* 0.8.4 (#318 / B2) — post-Full launcher BEFORE validation.
                       Lets the user chain Full → Security/RGAA without
                       having to finish the validation discussion first.
                       The Full baseline (docs/ + index files) exists at
                       this point so sub-audits have something to refine. */}
                    {!validationInProgress && (
                      <div className="flex-row gap-4" style={{ marginTop: 8 }}>
                        <select
                          className="dash-audit-select"
                          value={auditAgentChoice ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode'}
                          onChange={e => setAuditAgentChoice(e.target.value as AgentType)}
                        >
                          {agents.filter(canAudit).map(a => (
                            <option key={a.agent_type} value={a.agent_type}>{a.name}</option>
                          ))}
                          {agents.filter(canAudit).length === 0 && (
                            <option value="" disabled>{t('disc.noAgent')}</option>
                          )}
                        </select>
                        <select
                          className="dash-audit-select"
                          data-testid="audit-kind-select-audited"
                          value={auditKindChoice}
                          onChange={e => setAuditKindChoice(e.target.value as AuditKind)}
                          title={t('audit.kindSelector.tooltip')}
                        >
                          <option value="Full">{t('audit.kind.Full')}</option>
                          <option value="Security">{t('audit.kind.Security')}</option>
                          <option value="Docker">{t('audit.kind.Docker')}</option>
                          <option value="Performance">{t('audit.kind.Performance')}</option>
                          <option value="Accessibility">{t('audit.kind.Accessibility')}</option>
                          <option value="Rgaa">{t('audit.kind.Rgaa')}</option>
                          <option value="Database">{t('audit.kind.Database')}</option>
                          <option value="ApiDesign">{t('audit.kind.ApiDesign')}</option>
                        </select>
                        <button
                          className="dash-icon-btn dash-btn-accent-border"
                          onClick={() => handleFullAudit(undefined, auditKindChoice)}
                          disabled={agents.filter(canAudit).length === 0}
                        >
                          <Play size={12} /> {t('audit.kindSelector.launchLabel', t(`audit.kind.${auditKindChoice}`))}
                        </button>
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
                          setExpandedTab('docAi');
                          setDocDeepLink('docs/tech-debt');
                        }}
                      >
                        {t('audit.viewTechDebts', proj.tech_debt_count!)}
                      </button>
                    )}
                  </div>
                  {/* 0.8.4 (#287) — post-baseline re-audit row. SAME shape as
                     the TemplateInstalled/Bootstrapped launch row (agent +
                     kind + button). Sub-audits are ENABLED here because the
                     Full audit baseline exists. */}
                  <div className="dash-audit-pad">
                    <p className="dash-audit-desc">{t('audit.kindSelector.reAuditHint')}</p>
                    <div className="flex-row gap-4">
                      <select
                        className="dash-audit-select"
                        value={auditAgentChoice ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode'}
                        onChange={e => setAuditAgentChoice(e.target.value as AgentType)}
                      >
                        {agents.filter(canAudit).map(a => (
                          <option key={a.agent_type} value={a.agent_type}>{a.name}</option>
                        ))}
                        {agents.filter(canAudit).length === 0 && (
                          <option value="" disabled>{t('disc.noAgent')}</option>
                        )}
                      </select>
                      <select
                        className="dash-audit-select"
                        data-testid="audit-kind-select-validated"
                        value={auditKindChoice}
                        onChange={e => setAuditKindChoice(e.target.value as AuditKind)}
                        title={t('audit.kindSelector.tooltip')}
                      >
                        <option value="Full">{t('audit.kind.Full')}</option>
                        <option value="Security">{t('audit.kind.Security')}</option>
                        <option value="Docker">{t('audit.kind.Docker')}</option>
                        <option value="Performance">{t('audit.kind.Performance')}</option>
                        <option value="Accessibility">{t('audit.kind.Accessibility')}</option>
                        <option value="Rgaa">{t('audit.kind.Rgaa')}</option>
                        <option value="Database">{t('audit.kind.Database')}</option>
                        <option value="ApiDesign">{t('audit.kind.ApiDesign')}</option>
                      </select>
                      <button
                        className="dash-icon-btn dash-btn-accent-border"
                        onClick={() => handleFullAudit(undefined, auditKindChoice)}
                        disabled={agents.filter(canAudit).length === 0}
                      >
                        <Play size={12} /> {t('audit.kindSelector.launchLabel', auditKindChoice === 'Full' ? t('audit.kind.Full') : t(`audit.kind.${auditKindChoice}`))}
                      </button>
                    </div>
                  </div>
                  </>
                )}
              </>
            )}
          </div>

          <div className="dash-delete-zone">
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
