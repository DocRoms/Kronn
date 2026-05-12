import '../pages/Dashboard.css';
import { useState, useCallback, useEffect, useRef } from 'react';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { useIsMobile } from '../hooks/useMediaQuery';
import { isValidationDisc, isBriefingDisc, isBootstrapDisc, isUsable, isTrackerMcp } from '../lib/constants';
import { AiDocViewer } from './AiDocViewer';
import { ProjectSkills } from './ProjectSkills';
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
  FolderInput, Plug, X,
} from 'lucide-react';

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
  const { t } = useT();
  const isMobile = useIsMobile();

  // ── Collapsible sections ──
  const defaultSection = (auditStatus: string) =>
    (auditStatus === 'Bootstrapped' || auditStatus === 'Audited' || auditStatus === 'Validated') ? 'discussions' : 'aiContext';
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

  // ── Audit state ──
  const [auditActive, setAuditActive] = useState(false);
  const [auditStep, setAuditStep] = useState(0);
  const [auditTotalSteps, setAuditTotalSteps] = useState(0);
  const [auditCurrentFile, setAuditCurrentFile] = useState('');
  // 0.8.2 TD #233 — wall-clock elapsed for the live audit. The server-side
  // `AuditProgress` carries `started_at`; we just hold it in state and
  // drive a per-second re-render via `auditTick` so the displayed value
  // updates without re-polling the network. Cleared when the audit ends.
  const [auditStartedAt, setAuditStartedAt] = useState<number | null>(null);
  const [, setAuditTick] = useState(0);
  const [auditAbortController, setAuditAbortController] = useState<AbortController | null>(null);
  const [auditAgentChoice, setAuditAgentChoice] = useState<AgentType | undefined>(undefined);
  /// Briefing-start in flight — pre-fix the button was only disabled when
  /// `agents.length === 0`, so a double-click on a slow connection created
  /// two briefing discussions on the same project. The companion ref makes
  /// the guard race-free against two synchronous click events that fire
  /// before React re-renders the disabled state.
  const [briefingStarting, setBriefingStarting] = useState(false);
  const briefingStartingRef = useRef(false);
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

  // ── Computed ──
  const validationDisc = projDiscussions.find(d => isValidationDisc(d.title));
  const validationInProgress = !!validationDisc && proj.audit_status === 'Audited';
  const bootstrapDisc = projDiscussions.find(d => isBootstrapDisc(d.title));
  const bootstrapInProgress = !!bootstrapDisc && proj.audit_status === 'TemplateInstalled';
  // Use the locale-aware detector — the backend's `start_briefing` emits
  // a localized title (`Project Briefing` in EN, `Briefing del proyecto`
  // in ES, `Briefing projet` in FR). Pre-fix a startsWith('Briefing')
  // here missed EN, leaving English users without the "Reprendre le
  // briefing" button after they navigated away mid-briefing.
  const briefingDisc = projDiscussions.find(d => isBriefingDisc(d.title));
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
    const controller = new AbortController();
    setAuditAbortController(controller);
    setAuditActive(true);
    setAuditStep(0);
    setAuditTotalSteps(10);
    setAuditCurrentFile(t('audit.templateStep'));
    // Seed the resume checkpoint immediately so a tab-away during phase 1
    // (template install) still leaves a breadcrumb to poll against.
    const startedAt = new Date().toISOString();
    saveAuditCheckpoint({
      projectId: proj.id, kind: 'full_audit', startedAt,
      stepIndex: 0, totalSteps: 10, currentFile: null,
    });
    try {
      const auditAgent = auditAgentChoice ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode';
      await projectsApi.fullAuditStream(proj.id, { agent: auditAgent }, {
        onTemplateInstalled: () => {},
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
        onStepDone: () => {},
        onValidationCreated: () => {},
        onDone: (discussionId) => {
          auditActiveRef.current = false;
          setAuditActive(false);
          setAuditAbortController(null);
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
  }, [auditAgentChoice, agents, proj.id, t, toast, onRefetch, onRefetchDiscussions, onAutoRunDiscussion, onNavigate]);

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
    const cp = loadAuditCheckpoint(proj.id);
    if (!cp) return;

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
        } else {
          // Server reports nothing → either the audit wrapped up while we
          // were away or the checkpoint is orphaned (server restart, etc.).
          // Either way: drop the checkpoint, stop the UI state, refetch.
          clearAuditCheckpoint(proj.id);
          auditActiveRef.current = false;
          setAuditActive(false);
          setAuditStartedAt(null);
          if (auditPollRef.current) {
            clearInterval(auditPollRef.current);
            auditPollRef.current = null;
          }
          onRefetch();
        }
      } catch (err) {
        // Network hiccup — keep the checkpoint, keep polling. If the backend
        // is permanently gone the 1 h TTL will eventually retire the entry.
        console.warn('audit-status poll failed:', err);
      }
    };

    // Seed the UI immediately from the checkpoint so the resume bar shows
    // up without waiting for the first network round-trip.
    auditActiveRef.current = true;
    setAuditActive(true);
    setAuditStep(cp.stepIndex);
    setAuditTotalSteps(cp.totalSteps);
    setAuditCurrentFile(cp.currentFile ?? '');

    // Fire one immediate poll then every 2 s.
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
            <span className="dash-proj-name">{proj.name}</span>
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
            {/* Validated badge */}
            {proj.audit_status === 'Validated' ? (
              <span className="dash-badge-green"><ShieldCheck size={9} /> Validated</span>
            ) : validationInProgress ? (
              <span className="dash-badge-orange cursor-pointer" onClick={(e) => { e.stopPropagation(); if (validationDisc) onOpenDiscussion(validationDisc.id); onNavigate('discussions'); }}>
                <Loader2 size={9} style={{ animation: 'spin 1s linear infinite' }} /> Validation
              </span>
            ) : (proj.audit_status === 'Audited' || proj.audit_status === 'TemplateInstalled') ? (
              <span className="dash-badge-gray"><ShieldCheck size={9} /> Validated</span>
            ) : null}
            {/* Tech-debt count badge. 0.8.1: surfaced so users can spot
                projects with known issues at a glance. Counts both
                detail files under `docs/tech-debt/` and table rows
                in `docs/inconsistencies-tech-debt.md`. Click jumps to
                the docs viewer with the tech-debt section open. */}
            {(proj.tech_debt_count ?? 0) > 0 && (
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
            {/* Drift badge */}
            {driftStatus && driftStatus.stale_sections.length > 0 && (
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
            {/* Audit date */}
            {driftStatus?.audit_date && (
              <span className="dash-audit-date">
                {t('audit.auditDate', new Date(driftStatus.audit_date).toLocaleDateString())}
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
                        {disc.message_count ?? disc.messages.length} msg · {disc.agent}
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
            if (status === 'NoTemplate' || status === 'TemplateInstalled') {
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
                {(proj.audit_status === 'NoTemplate' || (proj.audit_status === 'TemplateInstalled' && !bootstrapInProgress)) && !auditActive && (
                  <div className="dash-audit-pad">
                    <p className="dash-audit-warning">
                      <AlertTriangle size={11} /> {proj.audit_status === 'NoTemplate' ? t('audit.noTemplate') : t('audit.description')}
                    </p>
                    <div className="flex-row gap-4 mb-4">
                      {briefingDisc && !briefingDone ? (
                        <button
                          className="dash-icon-btn dash-btn-info"
                          onClick={() => { onOpenDiscussion(briefingDisc.id); onNavigate('discussions'); }}
                        >
                          <MessageSquare size={12} /> {t('audit.resumeBriefing')}
                        </button>
                      ) : !briefingDone ? (
                        <button
                          className="dash-icon-btn dash-btn-info"
                          onClick={async () => {
                            // Guard against double / triple click — the
                            // backend's `start_briefing` is not idempotent
                            // (each call creates a new discussion), so a
                            // distracted operator could end up with two
                            // briefing discs on the same project. The state
                            // alone is not enough: two synchronous clicks
                            // both read `briefingStarting === false` from
                            // the closure before React re-renders the
                            // disabled prop. The ref reads/writes
                            // synchronously so the second click bails out.
                            if (briefingStartingRef.current) return;
                            briefingStartingRef.current = true;
                            setBriefingStarting(true);
                            const agent = auditAgentChoice ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode';
                            try {
                              const { discussion_id } = await projectsApi.startBriefing(proj.id, agent);
                              onRefetchDiscussions();
                              onAutoRunDiscussion(discussion_id);
                              onNavigate('discussions');
                            } catch (err) {
                              toast(String(err), 'error');
                            } finally {
                              briefingStartingRef.current = false;
                              setBriefingStarting(false);
                            }
                          }}
                          disabled={briefingStarting || agents.filter(canAudit).length === 0}
                        >
                          <MessageSquare size={12} /> {t('audit.startBriefing')}
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
                      <button
                        className="dash-icon-btn dash-btn-accent-border"
                        onClick={handleFullAudit}
                        disabled={agents.filter(canAudit).length === 0}
                      >
                        <Play size={12} /> {t('audit.startFullAudit')}
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
                      <button
                        className="dash-icon-btn dash-btn-accent-border"
                        onClick={handleFullAudit}
                        disabled={agents.filter(canAudit).length === 0}
                      >
                        <Play size={12} /> {t('audit.startFullAudit')}
                      </button>
                    </div>
                  </div>
                )}

                {proj.audit_status === 'Audited' && !auditActive && (
                  <div className="dash-audit-pad">
                    {validationInProgress && validationDisc ? (
                      <>
                        <p className="dash-audit-warning">
                          <Loader2 size={11} style={{ animation: 'spin 1s linear infinite' }} /> {t('audit.validationInProgress', validationDisc.message_count ?? validationDisc.messages.length)}
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
                  </div>
                )}

                {proj.audit_status === 'Validated' && !auditActive && (
                  <div className="dash-audit-validated">
                    <ShieldCheck size={11} /> {t('audit.done')}
                  </div>
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
