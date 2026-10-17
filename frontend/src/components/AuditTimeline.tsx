/**
 * KT-977 — the audit tab as a timeline: briefing, template, the audit's steps
 * grouped as the pipeline runs them, validation, validated. Presentational:
 * every action is a handler the ProjectCard already owns.
 */
import { useEffect, useMemo, useState, type ReactNode } from 'react';
import {
  Check, ChevronDown, ChevronRight, FileText, Loader2, Play, RotateCcw,
  ShieldCheck, Sparkles, StopCircle, X, AlertTriangle,
} from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { projects as projectsApi, externalApi, type ExternalApiConnectionView } from '../lib/api';
import { connectionAgent } from '../lib/auditLaunchChoice';
import { AGENT_LABELS, MODEL_TIER_ICONS, isUsable } from '../lib/constants';
import { stepRanges } from '../lib/stepRanges';
import { canRunAudit } from '../lib/agentCapabilities';
import { formatStepList } from '../lib/audit-resume';
import { costReasonKey, formatUsd, summarizeAuditCost, tokenBreakdownTitle } from '../lib/audit-cost';
import { BriefingForm } from './BriefingForm';
import { AuditStepActivity } from './AuditStepActivity';
import { activityCategoryLabel } from '../lib/activity-category';
import { DocStateSummary } from './DocStateSummary';
import type { DocStateCounts } from './docState';
import type {
  AgentDetection, AgentType, AuditEntry, AuditRecentActivity, AuditTimelineValidation, AuditTokenBreakdown, RepoAuditRun, ValidationBlock,
  LinkedRepo, ModelTier, ModelTiersConfig,
} from '../types/generated';
import './AuditTimeline.css';

type StepStatus = 'done' | 'unchanged' | 'accepted' | 'failed' | 'warned' | 'running' | 'pending' | 'todo';

/** KT-1211 — what the timeline needs to say of the latest run. */
interface LatestRunFacts {
  id: string;
  status: string;
  endedAt: string | null;
  /** The step in flight when the user cancelled (0 = before step 1). */
  cancelledAtStep: number | null;
  /** What moved in the sources during a resume. */
  resumeDrift: string | null;
}

interface StepRow {
  step_index: number;
  file_label: string;
  started_at: string;
  ended_at?: string | null;
  duration_ms?: number | null;
  cli_success: boolean;
  step_warning?: string | null;
  /** The target was already up to date: checked, nothing to fix (KT-1130). */
  unchanged?: boolean;
  /** A human accepted the failed step as it stands (KT-1131). */
  accepted_at?: string | null;
  accepted_by?: string | null;
  accepted_reason?: string | null;
  /** KT-1211 — what became of the target of a step that did not succeed. */
  target_state?: string | null;
  step_tokens?: number | null;
  input_tokens?: number | null;
  output_tokens?: number | null;
  cache_read_tokens?: number | null;
  /** What the step's agent reported it cost; absent = unknown, never 0. */
  cost_usd_micros?: number | null;
  /** Kronn's estimate when the agent reported none; why there is neither. */
  estimated_cost_usd_micros?: number | null;
  cost_unknown_reason?: string | null;
  breakdown?: AuditTokenBreakdown | null;
  carried_from_run_id?: string | null;
}

export interface AuditTimelineProps {
  projectId: string;
  auditStatus: string;
  agents: AgentDetection[];
  selectedAgent: AgentType | null;
  selectedTier: ModelTier;
  /** The shown choice is the one of the run being retried or resumed (KT-1320). */
  selectionFromRun?: boolean;
  /** The run's own named connection was deleted: the launch uses the agent's default one. */
  selectionConnectionGone?: boolean;
  modelTiers: ModelTiersConfig | null | undefined;
  /** Named external API connection of the selected HTTP agent, if any. */
  selectedConnectionId: string | null;
  onSelect: (agent: AgentType, tier: ModelTier, connectionId: string | null) => void;
  briefingDone: boolean;
  onBriefingSaved: () => void;
  /** KT-999 — linked repos edited from the briefing form. */
  linkedRepos?: LinkedRepo[];
  onLinkedReposChange?: () => void;
  auditActive: boolean;
  liveStep: number;
  liveTotal: number;
  liveFile: string;
  liveElapsed: string | null;
  liveTool: string | null;
  /** Wall-clock start of the live audit, to tell its steps from older runs'. */
  liveStartedAt: number | null;
  liveToolCalls: number | null;
  /** The running step's latest actions, for its details panel. */
  liveActivity?: AuditRecentActivity | null;
  liveStepTokens: number | null;
  /** The running step's headline parts, for its tooltip. */
  liveStepBreakdown?: AuditTokenBreakdown | null;
  liveTotalTokens: number | null;
  /** Who runs the live audit, as the server reports it: shown and frozen in
   *  the agent panel, whichever client launched the audit. */
  liveAuditor?: { agent: AgentType; tier: ModelTier; connectionId: string | null } | null;
  /** An older conversational briefing still open: resume it instead. */
  onResumeBriefingDiscussion: (() => void) | null;
  onCancel: () => void;
  /** The backend has not acknowledged the cancel yet: the audit may still run. */
  cancelPending?: boolean;
  resumable: { id: string; steps_to_redo?: number[]; last_completed_step: number; from_repository?: boolean } | null;
  /** Whether a run can be resumed is not known: launching waits for it (KT-1169). */
  resumableUnknown?: boolean;
  onReloadResumable?: () => void;
  /** The last launch stopped on this error; `retry` replays it (KT-1169). */
  launchError?: { message: string; retry: () => void } | null;
  onDismissLaunchError?: () => void;
  onLaunch: () => void;
  /** Retry one failed step of the resumable run, alone (KT-1131). */
  onRetryStep?: (step: number) => void;
  /** Accept one failed step of the resumable run as it stands (KT-1131). */
  onAcceptStep?: (step: number, reason: string | null, overrideUnsound?: boolean) => Promise<void>;
  /** KT-1211 — redo steps the resumed run inherited, because the sources moved. */
  onRedoSteps?: (steps: number[]) => void;
  /** A fresh full audit, even though a run can be resumed. */
  onRelaunchFull?: () => void;
  /** Settle what no worker runs any more and release an orphaned lease (KT-1134). */
  onCleanupLease?: () => Promise<void>;
  validationInProgress: boolean;
  /** Records the validation once the linked discussion has finished. */
  onMarkValid: () => Promise<void>;
  /** Opens a discussion, archived ones included. */
  onOpenValidation: (discussionId: string) => void;
  /** Step 4's tech-debt control: the same review entry as the project overview, plus the raw-file link. */
  techDebtEntry: ReactNode;
  /** The same verifier opened from the compact project header and stale docs. */
  onVerifyDocs?: () => void;
  docStateCounts?: DocStateCounts | null;
  refreshTrigger: number;
  toast: (msg: string, kind: 'success' | 'error' | 'info' | 'warning') => void;
}

/** Prefix of the backend's refusal that asks for an explicit override (KT-1211). */
export const OVERRIDE_REQUIRED_PREFIX = 'override_required:';

const NEWS_KEY = 'kr.audit.timeline.news.0.14.2';
const LOCAL_AGENTS = new Set<AgentType>(['Ollama']);
const HTTP_AGENTS = new Set<AgentType>(['LiteLlm', 'Nvidia', 'Custom']);
const TIERS: ModelTier[] = ['reasoning', 'default', 'economy'];

interface AgentChoice {
  key: string;
  agent: AgentType;
  label: string;
  usable: boolean;
  kind: 'cli' | 'http' | 'local';
  connection: ExternalApiConnectionView | null;
}

const TIER_KEY: Partial<Record<AgentType, keyof ModelTiersConfig>> = {
  ClaudeCode: 'claude_code', Codex: 'codex', OpenCode: 'open_code', GeminiCli: 'gemini_cli',
  Kiro: 'kiro', Vibe: 'vibe', CopilotCli: 'copilot_cli', Ollama: 'ollama', LiteLlm: 'lite_llm',
  Nvidia: 'nvidia',
};

/** A step that needs nothing more: written, checked unchanged, accepted, or warned. */
function isDoneStatus(status: StepStatus): boolean {
  return status === 'done' || status === 'unchanged' || status === 'accepted' || status === 'warned';
}

function readNewsDismissed(): boolean {
  try { return localStorage.getItem(NEWS_KEY) === '1'; } catch { return false; }
}

function stepStatus(
  row: StepRow | undefined, index: number, redo: Set<number>, active: boolean, cancelled = false,
): StepStatus {
  if (!row) return redo.has(index) ? 'todo' : 'pending';
  // Accepted by a human: done, whatever its failure was.
  if (row.accepted_at) return 'accepted';
  // The user cancelled the run in this step: it is not "interrupted", it is
  // simply to run again (KT-1211) — the run's banner says what happened.
  if (!row.ended_at && cancelled && !active) return 'pending';
  // Without a live audit, a step that never ended was interrupted.
  if (!row.ended_at) return active ? 'running' : 'failed';
  // Neither written nor failed: a neutral outcome, whatever the run made of it.
  if (row.unchanged) return 'unchanged';
  if (!row.cli_success) return 'failed';
  return row.step_warning ? 'warned' : 'done';
}

/** When the step last ran: steps of one audit can come from different runs. */
function formatStepDate(iso: string, locale: string, full: boolean): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '';
  return date.toLocaleString(locale, full
    ? { dateStyle: 'full', timeStyle: 'short' }
    : { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' });
}

/** `48.2 k`, `1.31 M`: a step's tokens at a glance. */
function formatTokens(n: number, locale: string): string {
  const fmt = (v: number, digits: number) => v.toLocaleString(locale, { maximumFractionDigits: digits });
  if (n >= 1_000_000) return `${fmt(n / 1_000_000, 2)} M`;
  if (n >= 1_000) return `${fmt(n / 1_000, 1)} k`;
  return fmt(n, 0);
}

/** A pricing reason in the UI's language; an unknown one keeps the server's text. */
function localizedCostReason(reason: string, t: (key: string, ...args: (string | number)[]) => string): string {
  const key = costReasonKey(reason);
  return key ? t(key) : reason;
}

function formatDuration(ms?: number | null): string {
  if (ms == null) return '';
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s} s` : `${Math.floor(s / 60)} min ${String(s % 60).padStart(2, '0')}`;
}

export function AuditTimeline(props: AuditTimelineProps) {
  const { t, locale } = useT();
  const {
    projectId, auditStatus, auditActive, resumable, refreshTrigger, liveStep,
  } = props;
  const [newsDismissed, setNewsDismissed] = useState(readNewsDismissed);
  const [briefingOpen, setBriefingOpen] = useState(false);
  const [runId, setRunId] = useState<string | null>(null);
  const [tdTotal, setTdTotal] = useState(0);
  const [steps, setSteps] = useState<StepRow[]>([]);
  // No step is shown before its recorded result is known: a done step must
  // never flash as "to do" while the runs load.
  const [stepsLoaded, setStepsLoaded] = useState(false);
  const [openGroups, setOpenGroups] = useState<Record<string, boolean>>({});
  const [connections, setConnections] = useState<ExternalApiConnectionView[]>([]);
  const [plan, setPlan] = useState<Map<number, string>>(new Map());
  // Audits the branch's `.kronn.json` records: another instance, an attestation.
  const [recorded, setRecorded] = useState<AuditEntry[]>([]);
  const [linkedValidation, setLinkedValidation] = useState<AuditTimelineValidation | null>(null);
  // KT-1122 — why the server would refuse a validation now (null = it accepts),
  // and who opened / validated it, as `.kronn.json` records them.
  const [validationBlock, setValidationBlock] = useState<ValidationBlock | null>(null);
  const [validatedBy, setValidatedBy] = useState<string | null>(null);
  const [recordedStartedBy, setRecordedStartedBy] = useState<string | null>(null);
  const [recordedValidatedAt, setRecordedValidatedAt] = useState<string | null>(null);
  // KT-1199 — the audit run the repository describes (a colleague's, after a
  // fetch or a rebase) and whether the server would validate from it.
  const [repoRun, setRepoRun] = useState<RepoAuditRun | null>(null);
  const [repoCanValidate, setRepoCanValidate] = useState(false);
  const [marking, setMarking] = useState(false);
  const [leaseOrphaned, setLeaseOrphaned] = useState(false);
  const [releasing, setReleasing] = useState(false);
  const [reloadTick, setReloadTick] = useState(0);
  // The failed step whose acceptance form is open, and its reason.
  const [accepting, setAccepting] = useState<number | null>(null);
  const [acceptReason, setAcceptReason] = useState('');
  const [acceptBusy, setAcceptBusy] = useState(false);
  const [confirmRelaunch, setConfirmRelaunch] = useState(false);
  const [acceptError, setAcceptError] = useState<string | null>(null);
  // The recorded steps could not be read: never shown as "not started".
  const [loadFailed, setLoadFailed] = useState(false);
  const [latestRunStatus, setLatestRunStatus] = useState<string | null>(null);
  // KT-1211 — the latest run's cancel and source-drift facts.
  const [latestRun, setLatestRun] = useState<LatestRunFacts | null>(null);
  // KT-1211 — the refusal that asks for an explicit override, with its diff.
  const [overrideDiff, setOverrideDiff] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    projectsApi.auditSteps()
      .then(list => { if (alive) setPlan(new Map((list ?? []).map(s => [s.index, s.target_file]))); })
      .catch(() => { /* the timeline still shows recorded steps */ });
    return () => { alive = false; };
  }, []);

  useEffect(() => {
    let alive = true;
    externalApi.list()
      .then(list => { if (alive) setConnections(list ?? []); })
      .catch(() => { if (alive) setConnections([]); });
    return () => { alive = false; };
  }, []);

  // A resume or a partial run records only the steps it ran: the other steps
  // live in earlier runs. Merge the recent Full and Partial runs step by step,
  // newest first (both record the plan's own step numbers).
  useEffect(() => {
    let alive = true;
    const load = async () => {
      // One request: the runs and their steps come together.
      const data = await projectsApi.auditTimeline(projectId);
      const runs = data.runs.filter(run => run.kind === 'Full' || run.kind === 'Partial').map(run => run.id);
      const ids = [...new Set([...(resumable ? [resumable.id] : []), ...runs])];
      const byRun = new Map<string, StepRow[]>();
      for (const row of data.steps) byRun.set(row.audit_run_id, [...(byRun.get(row.audit_run_id) ?? []), row]);
      const merged = new Map<number, StepRow>();
      for (const id of ids) {
        for (const row of byRun.get(id) ?? []) {
          if (!merged.has(row.step_index)) merged.set(row.step_index, row);
        }
      }
      const latestFull = data.runs.find(run => run.kind === 'Full');
      const latestRun = data.runs[0];
      return {
        id: ids[0] ?? null, rows: [...merged.values()], td: latestFull?.td_total ?? 0,
        recorded: data.recorded_audits, validation: data.latest_validation ?? null,
        block: data.validation_block ?? null,
        validatedBy: data.recorded_validated_by ?? null,
        startedBy: data.recorded_validation_started_by ?? null,
        validatedAt: data.recorded_validated_at ?? null,
        orphaned: data.lease_orphaned === true,
        latestStatus: latestRun?.status ?? null,
        latestFacts: latestRun ? {
          id: latestRun.id, status: latestRun.status, endedAt: latestRun.ended_at ?? null,
          cancelledAtStep: latestRun.cancelled_at_step ?? null,
          resumeDrift: latestRun.resume_drift ?? null,
        } : null,
        repo: data.repo ?? null,
        repoCanValidate: data.repo_can_validate === true,
      };
    };
    load()
      .then(({ id, rows, td, recorded: entries, validation, block, validatedBy: by, startedBy, validatedAt, orphaned, latestStatus, latestFacts, repo, repoCanValidate: canValidate }) => {
        if (alive) {
          setLatestRun(latestFacts);
          setRunId(id); setSteps(rows); setTdTotal(td); setRecorded(entries);
          setLinkedValidation(validation); setLeaseOrphaned(orphaned); setStepsLoaded(true); setLatestRunStatus(latestStatus);
          setValidationBlock(block); setValidatedBy(by); setRecordedStartedBy(startedBy); setRecordedValidatedAt(validatedAt);
          setRepoRun(repo); setRepoCanValidate(canValidate);
          setLoadFailed(false);
        }
      })
      .catch(() => { if (alive) { setSteps([]); setRecorded([]); setLinkedValidation(null); setValidationBlock(null); setLatestRunStatus(null); setLatestRun(null); setRepoRun(null); setRepoCanValidate(false); setStepsLoaded(true); setLoadFailed(true); } });
    return () => { alive = false; };
  }, [projectId, resumable, refreshTrigger, auditActive, liveStep, reloadTick]);

  const redo = useMemo(() => new Set(resumable?.steps_to_redo ?? []), [resumable]);
  // Steps that never ran have no row: the run's length also comes from the
  // steps a resume will redo and from the live total.
  const total = !stepsLoaded ? 0 : Math.max(
    steps.length, ...steps.map(s => s.step_index), ...redo, auditActive ? props.liveTotal : 0,
    plan.size,
  );

  // The live progress counts the steps of THIS run: a partial audit of step 3
  // is at "1 of 1". The step it runs is found by its file in the plan.
  const liveIndex = useMemo(() => {
    if (!auditActive) return 0;
    for (const [index, file] of plan) if (file === props.liveFile) return index;
    return liveStep;
  }, [auditActive, plan, props.liveFile, liveStep]);
  const partialRun = auditActive && plan.size > 0 && props.liveTotal > 0 && props.liveTotal < plan.size;

  // KT-1211 — the latest run was cancelled by the user. Settled `Cancelled` (no
  // step completed) the only next action is a fresh launch; settled
  // `Interrupted` it is resumable and still tells it was cancelled.
  const cancelledRun = !auditActive && latestRun !== null && latestRun.cancelledAtStep !== null
    && (latestRun.status === 'Cancelled' || latestRun.status === 'Interrupted');

  const rows = useMemo(() => {
    // A fresh full audit (not a resume) redoes every step: until it reaches
    // one, that step's older result is not this audit's. A partial run redoes
    // only its own steps, so every other result still stands.
    const fresh = auditActive && !resumable && !partialRun && props.liveStartedAt !== null;
    const current = fresh
      ? steps.filter(s => Date.parse(s.started_at) >= (props.liveStartedAt ?? 0) - 5000)
      : steps;
    const byIndex = new Map(current.map(s => [s.step_index, s]));
    return Array.from({ length: total }, (_, i) => {
      const index = i + 1;
      const row = byIndex.get(index);
      let status = stepStatus(row, index, redo, auditActive, cancelledRun);
      if (auditActive && index === liveIndex) status = 'running';
      const file = row?.file_label ?? plan.get(index) ?? (auditActive && index === liveIndex ? props.liveFile : '');
      return { index, row, status, file, descriptionKey: stepDescriptionKey(file) };
    });
  }, [steps, total, redo, auditActive, liveIndex, props.liveFile, plan, resumable, props.liveStartedAt, partialRun, cancelledRun]);

  // Grouped by what each step produces, not by position: the consolidation's
  // place in the chain differs between versions.
  const groups = useMemo(() => {
    const kinds = rows.map(r => stepGroup(r.file));
    // A step whose file is unknown cannot be placed honestly: keep one group.
    if (kinds.some(k => k === null)) return [{ key: 'all', title: t('auditTimeline.group.all'), rows }];
    // Groups follow the order the pipeline runs them in.
    return (['core', 'specialists', 'consolidation'] as const)
      .map(key => ({ key, title: t(`auditTimeline.group.${key}`), rows: rows.filter((_, i) => (kinds[i] ?? 'core') === key) }))
      .filter(g => g.rows.length > 0)
      .sort((a, b) => a.rows[0].index - b.rows[0].index);
  }, [rows, t]);

  const cost = useMemo(() => summarizeAuditCost(rows.flatMap(r => (r.row ? [r.row] : []))), [rows]);
  const failed = rows.filter(r => r.status === 'failed' || r.status === 'todo').map(r => r.index);
  const done = rows.filter(r => isDoneStatus(r.status)).length;
  // KT-1211 (M6) — the done steps this run inherited from an earlier run: the
  // ones a source drift makes worth redoing.
  const inheritedDone = rows
    .filter(r => isDoneStatus(r.status) && !!r.row?.carried_from_run_id)
    .map(r => r.index);
  const consolidationIndex = rows.find(r => stepGroup(r.file) === 'consolidation')?.index;
  const consolidationRedone = consolidationIndex !== undefined && redo.has(consolidationIndex)
    && [...redo].some(i => i !== consolidationIndex);

  const agentChoices = useMemo(() => {
    const kindOf = (agent: AgentType): AgentChoice['kind'] =>
      LOCAL_AGENTS.has(agent) ? 'local' : HTTP_AGENTS.has(agent) ? 'http' : 'cli';
    // A named connection replaces the bare provider slot of the same preset.
    const connected = new Set(connections.map(c => connectionAgent(c)));
    const detected: AgentChoice[] = props.agents
      .filter(a => canRunAudit(a) || (!isUsable(a) && isCliCandidate(a.agent_type)))
      .filter(a => !(HTTP_AGENTS.has(a.agent_type) && connected.has(a.agent_type)))
      .map(a => ({
        key: a.agent_type, agent: a.agent_type, label: AGENT_LABELS[a.agent_type] ?? a.name,
        usable: canRunAudit(a), kind: kindOf(a.agent_type), connection: null,
      }));
    const named: AgentChoice[] = connections.map(c => ({
      key: `conn:${c.id}`, agent: connectionAgent(c), label: c.display_name,
      usable: !!c.endpoint, kind: 'http', connection: c,
    }));
    const all = [...detected, ...named];
    return (['cli', 'http', 'local'] as const)
      .map(kind => ({ kind, items: all.filter(e => e.kind === kind) }))
      .filter(g => g.items.length > 0);
  }, [props.agents, connections]);

  // While an audit runs, the panel shows the agent that runs it and cannot
  // change: launching again would start a second audit on the same docs.
  // Without the server's word on who audits, highlight nothing rather than
  // present the panel's own choice as the running agent.
  const locked = auditActive;
  const live = locked ? props.liveAuditor ?? null : null;
  const selected = locked ? live?.agent ?? null : props.selectedAgent;
  const selectedTier: ModelTier | null = locked ? live?.tier ?? null : props.selectedTier;
  const selectedConnectionId = locked ? live?.connectionId ?? null : props.selectedConnectionId;
  const selectedKind = selected && LOCAL_AGENTS.has(selected) ? 'local'
    : selected && HTTP_AGENTS.has(selected) ? 'http' : 'cli';
  const selectedConnection = connections.find(c => c.id === selectedConnectionId) ?? null;
  const tierModel = (tier: ModelTier) => {
    if (selectedConnection) return selectedConnection[`${tier}_model`] ?? selectedConnection.default_model ?? '';
    const key = selected ? TIER_KEY[selected] : undefined;
    const config = key ? props.modelTiers?.[key] : undefined;
    return (config?.[tier] as string | null | undefined) ?? '';
  };

  const dismissNews = () => {
    setNewsDismissed(true);
    try { localStorage.setItem(NEWS_KEY, '1'); } catch { /* per-viewer convenience only */ }
  };

  const completedLocalRun = stepsLoaded && !loadFailed && latestRunStatus === 'Completed';
  const validated = auditStatus === 'Validated' && completedLocalRun;
  const audited = auditStatus === 'Audited' || auditStatus === 'Validated';
  const templateInstalled = auditStatus !== 'NoTemplate';
  const auditNodeStatus: StepStatus = auditActive ? 'running'
    : failed.length > 0 ? 'failed' : audited ? 'done' : 'pending';
  const launchLabel = auditActive ? t('auditTimeline.launch.running') : resumable
    ? (resumable.steps_to_redo && resumable.steps_to_redo.length > 0
      ? t('audit.resumeRedoSteps', formatStepList(resumable.steps_to_redo))
      // Every failed step was accepted: the resume only finalizes the run.
      : resumable.steps_to_redo ? t('auditTimeline.audit.finalize')
        : t('audit.resumeFromStep', resumable.last_completed_step + 1))
    : cancelledRun && latestRun?.status === 'Cancelled' ? t('auditTimeline.cancelled.relaunch')
      : audited ? t('audit.revalidate.button') : t('audit.startFullAudit');
  // Per-step actions exist only on the run a resume continues.
  // A run still in the repository has no local row to retry or accept a step
  // on: the resume adopts it first.
  const stepActions = !auditActive && !!resumable && !resumable.from_repository && !leaseOrphaned;
  // No launch while the state it must start from is unknown or locked.
  // KT-1192 — no audit starts without a briefing; the backend refuses it too.
  const launchBlocked = leaseOrphaned || !!props.resumableUnknown || !props.briefingDone;
  // The latest run is interrupted: validating waits for its failed steps.
  const runPending = !!resumable && !auditActive;
  const validationRunning = completedLocalRun && linkedValidation !== null && !linkedValidation.finished && !auditActive;
  const verifyEntry = audited && !resumable && props.onVerifyDocs !== undefined;

  const submitAccept = async (step: number, overrideUnsound = false) => {
    if (!props.onAcceptStep || acceptBusy) return;
    setAcceptBusy(true);
    setAcceptError(null);
    try {
      if (overrideUnsound) await props.onAcceptStep(step, acceptReason.trim() || null, true);
      else await props.onAcceptStep(step, acceptReason.trim() || null);
      setAccepting(null);
      setAcceptReason('');
      setOverrideDiff(null);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      // KT-1211 — a step known to leave an unsound document: the refusal
      // carries what the step changed, and an explicit override is offered.
      if (message.includes(OVERRIDE_REQUIRED_PREFIX)) {
        setOverrideDiff(message.slice(message.indexOf(OVERRIDE_REQUIRED_PREFIX) + OVERRIDE_REQUIRED_PREFIX.length).trim());
      } else {
        setAcceptError(message);
      }
    } finally {
      setAcceptBusy(false);
    }
  };

  const releaseLease = async () => {
    if (!props.onCleanupLease || releasing) return;
    setReleasing(true);
    try {
      await props.onCleanupLease();
      setReloadTick(n => n + 1);
    } finally {
      setReleasing(false);
    }
  };

  return (
    <div className="audit-tl" data-testid="audit-timeline">
      {!newsDismissed && (
        <section className="audit-tl-news" aria-label={t('auditTimeline.news.title')}>
          <Sparkles size={16} aria-hidden="true" className="audit-tl-news-icon" />
          <div className="audit-tl-news-body">
            <strong>{t('auditTimeline.news.title')}</strong>
            <ul>
              <li><b>{t('auditTimeline.news.resumeTitle')}</b> {t('auditTimeline.news.resume')}</li>
              <li><b>{t('auditTimeline.news.consolidationTitle')}</b> {t('auditTimeline.news.consolidation')}</li>
              <li><b>{t('auditTimeline.news.validationTitle')}</b> {t('auditTimeline.news.validation')}</li>
            </ul>
          </div>
          <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={dismissNews}>
            {t('auditTimeline.news.dismiss')}
          </button>
        </section>
      )}

      {audited && (
        <section className="audit-tl-doc-state" data-testid="audit-timeline-doc-state">
          <DocStateSummary counts={props.docStateCounts ?? null} variant="block" />
          {props.onVerifyDocs && (
            <button type="button" className="audit-tl-btn audit-tl-btn-primary" onClick={props.onVerifyDocs}>
              <ShieldCheck size={12} /> {t('audit.revalidate.button')}
            </button>
          )}
        </section>
      )}

      <div className="audit-tl-layout">
        <ol className="audit-tl-phases" aria-label={t('auditTimeline.ariaPhases')}>
          <Phase
            status={props.briefingDone ? 'done' : 'pending'}
            eyebrow={t('auditTimeline.phase.n', 0)}
            title={t('auditTimeline.briefing.title')}
            tag={props.briefingDone ? t('auditTimeline.status.saved') : t('auditTimeline.status.toFill')}
          >
            {props.briefingDone && !briefingOpen ? (
              <div className="audit-tl-row">
                <p className="audit-tl-muted">{t('auditTimeline.briefing.used')}</p>
                <button
                  type="button"
                  className="audit-tl-btn audit-tl-btn-small"
                  onClick={() => setBriefingOpen(true)}
                  disabled={auditActive}
                  title={auditActive ? t('auditTimeline.briefing.lockedDuringAudit') : undefined}
                  data-testid="audit-timeline-briefing-edit"
                >
                  <FileText size={12} /> {t('auditTimeline.briefing.edit')}
                </button>
              </div>
            ) : !props.briefingDone && props.onResumeBriefingDiscussion ? (
              <div className="audit-tl-row">
                <p className="audit-tl-muted">{t('auditTimeline.briefing.inDiscussion')}</p>
                <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={props.onResumeBriefingDiscussion}>
                  {t('audit.resumeBriefing')}
                </button>
              </div>
            ) : briefingOpen ? (
              <BriefingForm
                projectId={projectId}
                agent={selected ?? 'ClaudeCode'}
                tier={props.selectedTier}
                review={false}
                onClose={() => setBriefingOpen(false)}
                onSaved={() => { setBriefingOpen(false); props.onBriefingSaved(); }}
                toast={props.toast}
                linkedRepos={props.linkedRepos}
                onLinkedReposChange={props.onLinkedReposChange}
              />
            ) : (
              <div className="audit-tl-row">
                <p className="audit-tl-muted" data-testid="audit-timeline-briefing-required">{t('auditTimeline.briefing.required')}</p>
                <button
                  type="button"
                  className="audit-tl-btn audit-tl-btn-small"
                  onClick={() => setBriefingOpen(true)}
                  disabled={auditActive}
                  title={auditActive ? t('auditTimeline.briefing.lockedDuringAudit') : undefined}
                  data-testid="audit-timeline-briefing-open"
                >
                  <FileText size={12} /> {t('auditTimeline.briefing.fill')}
                </button>
              </div>
            )}
          </Phase>

          <Phase
            status={templateInstalled ? 'done' : 'pending'}
            eyebrow={t('auditTimeline.phase.n', 1)}
            title={t('auditTimeline.template.title')}
            tag={templateInstalled ? t('auditTimeline.status.installed') : t('auditTimeline.status.atLaunch')}
          >
            <p className="audit-tl-muted">{t('auditTimeline.template.desc')}</p>
          </Phase>

          <Phase
            status={auditNodeStatus}
            eyebrow={t('auditTimeline.phase.n', 2)}
            title={t('auditTimeline.audit.title')}
            tag={auditActive ? t('auditTimeline.status.running')
              : failed.length > 0 ? t(resumable ? 'auditTimeline.audit.toResume' : 'auditTimeline.audit.failedCount', failed.length)
                : audited ? t('auditTimeline.status.done') : t('auditTimeline.status.todo')}
          >
            {total > 0 && (
              <>
                <p className="audit-tl-muted">{t('auditTimeline.audit.summary', done, total)}</p>
                {cost.kind !== 'none' && (
                  <p className="audit-tl-muted audit-tl-mono" data-testid="audit-timeline-cost-total"
                    title={(cost.kind === 'exact' || cost.kind === 'floor') && cost.estimated
                      ? `${t('auditTimeline.cost.totalTitle')} ${t('auditTimeline.cost.estimatedTitle')}`
                      : t('auditTimeline.cost.totalTitle')}>
                    {cost.kind === 'exact'
                      ? t(cost.estimated ? 'auditTimeline.cost.totalEstimated' : 'auditTimeline.cost.total', formatUsd(cost.usdMicros, locale))
                      : cost.kind === 'floor'
                        ? t(cost.estimated ? 'auditTimeline.cost.totalEstimatedFloor' : 'auditTimeline.cost.totalFloor', formatUsd(cost.usdMicros, locale), cost.unknownSteps)
                        : t('auditTimeline.cost.totalUnknown')}
                  </p>
                )}
                <div className="audit-tl-progress" role="progressbar" aria-valuemin={0} aria-valuemax={total} aria-valuenow={done} aria-label={t('auditTimeline.audit.title')}>
                  <div style={{ width: `${Math.round((done / total) * 100)}%` }} />
                </div>
              </>
            )}
            {/* Mounted before the run starts so screen readers pick up the first step; only the step text changes. */}
            <p className="audit-tl-sr-only" role="status" aria-live="polite" aria-atomic="true" data-testid="audit-timeline-step-announce">
              {auditActive ? t('audit.step', liveIndex, partialRun ? total : props.liveTotal, props.liveFile) : ''}
            </p>
            {auditActive && (
              <div className="audit-tl-live" data-testid="audit-timeline-live">
                <Loader2 size={13} className="spin" aria-hidden="true" />
                <span>{t('audit.step', liveIndex, partialRun ? total : props.liveTotal, props.liveFile)}</span>
                {props.liveElapsed && <span className="audit-tl-muted">{props.liveElapsed}</span>}
                {props.liveTool && (
                  <span className="audit-tl-muted">
                    {t('audit.currentTool', activityCategoryLabel(t, props.liveTool))}
                    {props.liveToolCalls != null && props.liveToolCalls > 0 && ` (${props.liveToolCalls})`}
                  </span>
                )}
                {props.liveTotalTokens != null && props.liveTotalTokens > 0 && (
                  <span className="audit-tl-muted" title={t('auditTimeline.tokens.freshTitle')}>{t('audit.totalTokens', formatTokens(props.liveTotalTokens, locale))}</span>
                )}
                <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={props.onCancel} data-testid="audit-timeline-cancel">
                  <StopCircle size={12} /> {props.cancelPending ? t('auditTimeline.cancelRetry') : t('audit.cancelAudit')}
                </button>
              </div>
            )}
            {auditActive && props.cancelPending && (
              <p className="audit-tl-note" role="status" data-testid="audit-timeline-cancel-pending">{t('auditTimeline.cancelPending')}</p>
            )}
            {cancelledRun && latestRun && (
              <div className="audit-tl-note" role="status" data-testid="audit-timeline-cancelled">
                <p>
                  <strong>
                    {latestRun.cancelledAtStep && latestRun.cancelledAtStep > 0
                      ? t('auditTimeline.cancelled.during', formatStepDate(latestRun.endedAt ?? '', locale, true), latestRun.cancelledAtStep)
                      : t('auditTimeline.cancelled.before', formatStepDate(latestRun.endedAt ?? '', locale, true))}
                  </strong>
                </p>
                <p className="audit-tl-muted" data-testid="audit-timeline-cancelled-next">
                  {resumable
                    ? t('auditTimeline.cancelled.resumable', resumable.last_completed_step + 1)
                    : props.briefingDone
                      ? t('auditTimeline.cancelled.nextRelaunch')
                      : t('auditTimeline.cancelled.nextBriefing')}
                </p>
              </div>
            )}
            {latestRun?.resumeDrift && !auditActive && (
              <div className="audit-tl-warn" role="status" data-testid="audit-timeline-resume-drift">
                <AlertTriangle size={13} aria-hidden="true" />
                <div>
                  <p>{latestRun.resumeDrift}</p>
                  {props.onRedoSteps && resumable && stepActions && inheritedDone.length > 0 && (
                    <button
                      type="button"
                      className="audit-tl-btn audit-tl-btn-small audit-tl-btn-warn"
                      onClick={() => props.onRedoSteps?.(inheritedDone)}
                      disabled={launchBlocked || !selected}
                      data-testid="audit-timeline-redo-drifted"
                    >
                      <RotateCcw size={11} /> {t('auditTimeline.drift.redo', inheritedDone.length)}
                    </button>
                  )}
                </div>
              </div>
            )}
            {leaseOrphaned && !auditActive && (
              <div className="audit-tl-warn" role="alert" data-testid="audit-timeline-lease-orphaned">
                <AlertTriangle size={13} aria-hidden="true" />
                <div>
                  <p>{t('auditTimeline.lease.stuck')}</p>
                  {props.onCleanupLease && (
                    <button type="button" className="audit-tl-btn audit-tl-btn-small" disabled={releasing} onClick={() => void releaseLease()} data-testid="audit-timeline-lease-release">
                      {releasing ? <Loader2 size={12} className="spin" /> : <RotateCcw size={12} />} {t('auditTimeline.lease.release')}
                    </button>
                  )}
                </div>
              </div>
            )}
            {props.launchError && !auditActive && (
              <div className="audit-tl-warn" role="alert" data-testid="audit-timeline-launch-error">
                <AlertTriangle size={13} aria-hidden="true" />
                <div>
                  <p><strong>{t('auditTimeline.error.title')}</strong></p>
                  <p className="audit-tl-error-message">{props.launchError.message}</p>
                  <div className="audit-tl-validation-actions">
                    <button type="button" className="audit-tl-btn audit-tl-btn-small audit-tl-btn-warn" onClick={props.launchError.retry} data-testid="audit-timeline-launch-error-retry">
                      <RotateCcw size={12} /> {t('auditTimeline.error.resume')}
                    </button>
                    {props.onDismissLaunchError && (
                      <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={props.onDismissLaunchError}>
                        {t('auditTimeline.error.dismiss')}
                      </button>
                    )}
                  </div>
                </div>
              </div>
            )}
            {props.resumableUnknown && !auditActive && (
              <div className="audit-tl-warn" role="alert" data-testid="audit-timeline-resumable-unknown">
                <AlertTriangle size={13} aria-hidden="true" />
                <div>
                  <p>{t('auditTimeline.error.resumableUnknown')}</p>
                  {props.onReloadResumable && (
                    <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={props.onReloadResumable} data-testid="audit-timeline-resumable-reload">
                      <RotateCcw size={12} /> {t('auditTimeline.error.retry')}
                    </button>
                  )}
                </div>
              </div>
            )}
            {loadFailed && (
              <div className="audit-tl-warn" role="alert" data-testid="audit-timeline-load-error">
                <AlertTriangle size={13} aria-hidden="true" />
                <div>
                  <p>{t('auditTimeline.error.timeline')}</p>
                  <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={() => setReloadTick(n => n + 1)} data-testid="audit-timeline-load-retry">
                    <RotateCcw size={12} /> {t('auditTimeline.error.retry')}
                  </button>
                </div>
              </div>
            )}
            {consolidationRedone && (
              <p className="audit-tl-note">{t('auditTimeline.audit.consolidationNote')}</p>
            )}
            {groups.map(group => {
              const groupFailed = group.rows.some(r => r.status === 'failed' || r.status === 'todo');
              const open = openGroups[group.key] ?? (groupFailed || group.rows.some(r => r.status === 'running'));
              const groupDone = group.rows.filter(r => isDoneStatus(r.status)).length;
              return (
                <div key={group.key} className="audit-tl-group">
                  <div className="audit-tl-group-row">
                    <button
                      type="button"
                      className="audit-tl-group-head"
                      aria-expanded={open}
                      onClick={() => setOpenGroups(g => ({ ...g, [group.key]: !open }))}
                    >
                      {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                      <span>{group.title}</span>
                      <span className={`audit-tl-tag ${groupFailed ? 'is-failed' : 'is-done'}`}>
                        {groupFailed ? t(resumable ? 'auditTimeline.group.failed' : 'auditTimeline.status.failed') : `${groupDone}/${group.rows.length}`}
                      </span>
                    </button>
                    {groupFailed && resumable && !auditActive && (
                      <button type="button" className="audit-tl-btn audit-tl-btn-small audit-tl-btn-warn" onClick={props.onLaunch} disabled={launchBlocked || !selected}>
                        <RotateCcw size={11} /> {t('auditTimeline.audit.resume')}
                      </button>
                    )}
                  </div>
                  {open && (
                    <ul className="audit-tl-steps">
                      {group.rows.map(r => (
                        <li key={r.index} className={`audit-tl-step is-${r.status}`} data-testid={`audit-timeline-step-${r.index}`}>
                          <span className={`audit-tl-dot is-${r.status}`} aria-hidden="true" />
                          <span className="audit-tl-step-main">
                            <span className="audit-tl-step-head">
                              <span className="audit-tl-mono audit-tl-muted">{String(r.index).padStart(2, '0')}</span>
                              <span className="audit-tl-mono">{r.file || t('auditTimeline.audit.notRun')}</span>
                            </span>
                            {r.descriptionKey && (
                              <span className="audit-tl-step-desc">{t(r.descriptionKey)}</span>
                            )}
                            {r.status === 'running' && (
                              <AuditStepActivity projectId={projectId} recent={props.liveActivity ?? null} />
                            )}
                            {r.status === 'accepted' && r.row ? (
                              <span className="audit-tl-reason is-accepted" data-testid={`audit-timeline-step-accepted-${r.index}`} title={r.row.step_warning ?? undefined}>
                                {r.row.accepted_reason
                                  ? t('auditTimeline.reason.acceptedWhy', r.row.accepted_by ?? '', formatStepDate(r.row.accepted_at ?? '', locale, false), r.row.accepted_reason)
                                  : t('auditTimeline.reason.accepted', r.row.accepted_by ?? '', formatStepDate(r.row.accepted_at ?? '', locale, false))}
                              </span>
                            ) : r.status === 'unchanged' ? (
                              <span className="audit-tl-reason is-unchanged">{t('auditTimeline.reason.verified')}</span>
                            ) : r.row?.step_warning ? (
                              <span className={`audit-tl-reason is-${r.status}`} title={r.row.step_warning}>
                                {stepReason(r.row.step_warning, t)}
                              </span>
                            ) : r.row && !r.row.ended_at && !auditActive && (
                              cancelledRun
                                ? <span className="audit-tl-reason" data-testid={`audit-timeline-step-cancelled-${r.index}`}>{t('auditTimeline.reason.cancelledHere')}</span>
                                : <span className="audit-tl-reason is-failed">{t('auditTimeline.reason.interrupted')}</span>
                            )}
                            {r.row?.target_state && (
                              <span className="audit-tl-reason is-failed" data-testid={`audit-timeline-step-target-${r.index}`}>
                                {t(r.row.target_state === 'restored' ? 'auditTimeline.target.restored' : 'auditTimeline.target.partial')}
                              </span>
                            )}
                            {stepActions && (r.status === 'failed' || r.status === 'todo') && (
                              <span className="audit-tl-step-actions" data-testid={`audit-timeline-step-actions-${r.index}`}>
                                {props.onRetryStep && (
                                  <button type="button" className="audit-tl-btn audit-tl-btn-small audit-tl-btn-warn" onClick={() => props.onRetryStep?.(r.index)} disabled={launchBlocked || !selected} data-testid={`audit-timeline-step-retry-${r.index}`}>
                                    <RotateCcw size={11} /> {t('auditTimeline.step.retry')}
                                  </button>
                                )}
                                {props.onAcceptStep && accepting !== r.index && (
                                  <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={() => { setAccepting(r.index); setAcceptReason(''); setAcceptError(null); setOverrideDiff(null); }} data-testid={`audit-timeline-step-accept-${r.index}`}>
                                    <Check size={11} /> {t('auditTimeline.step.accept')}
                                  </button>
                                )}
                              </span>
                            )}
                            {stepActions && accepting === r.index && (
                              <span className="audit-tl-accept-form" data-testid={`audit-timeline-accept-form-${r.index}`}>
                                <span className="audit-tl-muted">{t('auditTimeline.step.acceptHint')}</span>
                                <textarea
                                  className="audit-tl-accept-reason"
                                  aria-label={t('auditTimeline.step.acceptReason')}
                                  placeholder={t('auditTimeline.step.acceptReason')}
                                  value={acceptReason}
                                  maxLength={500}
                                  rows={2}
                                  onChange={e => setAcceptReason(e.target.value)}
                                />
                                {acceptError && (
                                  <span className="audit-tl-reason is-failed" role="alert" data-testid={`audit-timeline-accept-error-${r.index}`}>
                                    {t('auditTimeline.step.acceptFailed', acceptError)}
                                  </span>
                                )}
                                {overrideDiff && (
                                  <span className="audit-tl-warn" role="alert" data-testid={`audit-timeline-accept-override-${r.index}`}>
                                    <AlertTriangle size={13} aria-hidden="true" />
                                    <span>
                                      <strong>{t('auditTimeline.step.overrideTitle')}</strong>
                                      <pre className="audit-tl-accept-diff">{overrideDiff}</pre>
                                    </span>
                                  </span>
                                )}
                                <span className="audit-tl-row">
                                  <button
                                    type="button"
                                    className={`audit-tl-btn audit-tl-btn-small${overrideDiff ? ' audit-tl-btn-warn' : ''}`}
                                    disabled={acceptBusy}
                                    onClick={() => void submitAccept(r.index, overrideDiff !== null)}
                                    data-testid={`audit-timeline-accept-confirm-${r.index}`}
                                  >
                                    {acceptBusy ? <Loader2 size={11} className="spin" /> : <Check size={11} />} {overrideDiff ? t('auditTimeline.step.overrideConfirm') : t('auditTimeline.step.acceptConfirm')}
                                  </button>
                                  <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={() => { setAccepting(null); setAcceptError(null); setOverrideDiff(null); }}>
                                    {t('auditTimeline.step.acceptCancel')}
                                  </button>
                                </span>
                              </span>
                            )}
                          </span>
                          <span className="audit-tl-step-side">
                            {r.row && (
                              <time
                                className="audit-tl-muted audit-tl-step-date"
                                dateTime={r.row.ended_at ?? r.row.started_at}
                                title={formatStepDate(r.row.ended_at ?? r.row.started_at, locale, true)}
                              >
                                {formatStepDate(r.row.ended_at ?? r.row.started_at, locale, false)}
                              </time>
                            )}
                            <span className="audit-tl-mono audit-tl-muted">{formatDuration(r.row?.duration_ms)}</span>
                            {(() => {
                              // The running step reads its live count; a finished one its record.
                              const tokens = r.status === 'running' ? props.liveStepTokens : r.row?.step_tokens;
                              // A finished step whose agent reported nothing is unknown, not free.
                              if (tokens == null && r.row?.ended_at && r.status !== 'running') {
                                return (
                                  <span
                                    className="audit-tl-mono audit-tl-muted"
                                    data-testid={`audit-timeline-step-tokens-${r.index}`}
                                    title={t('auditTimeline.tokens.unknownTitle')}
                                  >
                                    {t('auditTimeline.tokens.unknown')}
                                  </span>
                                );
                              }
                              if (tokens == null || (r.status === 'running' && tokens <= 0)) return null;
                              const breakdown = r.status === 'running' ? props.liveStepBreakdown : r.row?.breakdown;
                              return (
                                <span
                                  className="audit-tl-mono audit-tl-muted"
                                  data-testid={`audit-timeline-step-tokens-${r.index}`}
                                  title={[
                                    tokenBreakdownTitle(breakdown, t, locale),
                                    r.status !== 'running' && r.row?.carried_from_run_id ? t('auditTimeline.tokens.carried', r.row.carried_from_run_id.slice(0, 8)) : '',
                                  ].filter(Boolean).join(' · ') || undefined}
                                >
                                  {t('auditTimeline.tokens.short', formatTokens(tokens, locale))}
                                </span>
                              );
                            })()}
                            {r.row?.ended_at && r.status !== 'running' && (
                              // A finished step whose agent reported no cost is unknown, never free.
                              typeof r.row.cost_usd_micros === 'number' ? (
                                <span className="audit-tl-mono audit-tl-muted" data-testid={`audit-timeline-step-cost-${r.index}`}>
                                  {t('auditTimeline.cost.short', formatUsd(r.row.cost_usd_micros, locale))}
                                </span>
                              ) : typeof r.row.estimated_cost_usd_micros === 'number' ? (
                                <span
                                  className="audit-tl-mono audit-tl-muted"
                                  data-testid={`audit-timeline-step-cost-${r.index}`}
                                  title={t('auditTimeline.cost.estimatedTitle')}
                                >
                                  {t('auditTimeline.cost.estimated', formatUsd(r.row.estimated_cost_usd_micros, locale))}
                                </span>
                              ) : (
                                <span
                                  className="audit-tl-mono audit-tl-muted"
                                  data-testid={`audit-timeline-step-cost-${r.index}`}
                                  title={[t('auditTimeline.cost.unknownTitle'),
                                    r.row.cost_unknown_reason ? t('auditTimeline.cost.unknownReason', localizedCostReason(r.row.cost_unknown_reason, t)) : '',
                                  ].filter(Boolean).join(' ')}
                                >
                                  {t('auditTimeline.cost.unknown')}
                                </span>
                              )
                            )}
                            <span className={`audit-tl-tag is-${r.status}`}>{t(`auditTimeline.status.${r.status}`)}</span>
                          </span>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
              );
            })}
            {runId === null && !auditActive && recorded.length > 0 && (
              <div className="audit-tl-recorded" data-testid="audit-timeline-recorded">
                <p className="audit-tl-muted">{t('auditTimeline.recorded.intro')}</p>
                <ul>
                  {[...recorded].reverse().map((entry, i) => (
                    <li key={`${entry.date}-${i}`} data-testid="audit-timeline-recorded-entry">
                      <time className="audit-tl-mono" dateTime={entry.date}>{entry.date}</time>
                      <span>{t(`auditTimeline.recorded.provenance.${entry.provenance}`)}</span>
                      <span className="audit-tl-muted">
                        {entry.kronn_version === 'legacy'
                          ? t('auditTimeline.recorded.legacyVersion')
                          : t('auditTimeline.recorded.version', entry.kronn_version)}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            )}
            {runId === null && total === 0 && !auditActive && recorded.length === 0 && !loadFailed && (
              <p className="audit-tl-muted">{t('auditTimeline.audit.notStarted')}</p>
            )}
          </Phase>

          <Phase
            status={validated ? 'done' : validationRunning ? 'running' : completedLocalRun && !runPending && !auditActive ? 'pending' : 'blocked'}
            eyebrow={t('auditTimeline.phase.n', 3)}
            title={t('auditTimeline.validation.title')}
            tag={validated ? t('auditTimeline.status.done') : validationRunning ? t('auditTimeline.status.running')
              : completedLocalRun && !runPending && !auditActive ? t('auditTimeline.status.todo') : t('auditTimeline.status.waitingAudit')}
          >
            <p className="audit-tl-muted">{t('auditTimeline.validation.desc')}</p>
            {audited && tdTotal > 0 && (
              <p className="audit-tl-muted" data-testid="audit-timeline-td-count">{t('auditTimeline.validation.tdCount', tdTotal)}</p>
            )}
            {repoRun?.newer_than_local && !auditActive && (
              <div className="audit-tl-validation-actions" data-testid="audit-timeline-repo-run">
                <p className="audit-tl-muted" data-testid="audit-timeline-repo-run-summary">
                  {t(
                    'auditTimeline.repo.summary',
                    repoRun.started_by,
                    new Date(repoRun.started_at).toLocaleDateString(),
                    stepRanges(repoRun.done_steps) || t('auditTimeline.repo.none'),
                    stepRanges(repoRun.resume_steps) || t('auditTimeline.repo.none'),
                    stepRanges(repoRun.recheck_steps) || t('auditTimeline.repo.none'),
                  )}
                </p>
                <p className="audit-tl-muted" data-testid="audit-timeline-repo-run-td">
                  {t('auditTimeline.repo.td', repoRun.td_total, repoRun.td_settled)}
                </p>
                {!repoRun.committed && (
                  <p className="audit-tl-muted" data-testid="audit-timeline-repo-run-uncommitted">{t('auditTimeline.repo.uncommitted')}</p>
                )}
                {repoRun.resumable && resumable?.id === repoRun.run_id && (
                  <p className="audit-tl-muted" data-testid="audit-timeline-repo-run-resumable">{t('auditTimeline.repo.resumable')}</p>
                )}
                {repoCanValidate && !repoRun.validated_at ? (
                  <button
                    type="button"
                    className="audit-tl-btn audit-tl-btn-small"
                    disabled={marking}
                    data-testid="audit-timeline-repo-run-validate"
                    onClick={() => {
                      setMarking(true);
                      void props.onMarkValid().finally(() => setMarking(false));
                    }}
                  >
                    {marking ? <Loader2 size={12} className="spin" /> : <ShieldCheck size={12} />} {t('auditTimeline.repo.validate')}
                  </button>
                ) : validationBlock && repoRun.status === 'Completed' && !repoRun.validated_at ? (
                  <p className="audit-tl-muted" data-testid="audit-timeline-repo-run-block">
                    {t(`auditTimeline.validation.block.${validationBlock}`)}
                  </p>
                ) : null}
              </div>
            )}
            {!validated && !auditActive && (runPending || (!audited && runId !== null)) ? (
              // The run is not finished: the action here is the audit's own.
              <div className="audit-tl-validation-actions" data-testid="audit-timeline-validation-blocked">
                {runPending && <p className="audit-tl-muted">{t('auditTimeline.validation.afterRun')}</p>}
                <button
                  type="button"
                  className="audit-tl-btn audit-tl-btn-small"
                  onClick={props.onLaunch}
                  disabled={launchBlocked || !selected}
                  data-testid="audit-timeline-validation-launch"
                >
                  {runPending ? <RotateCcw size={12} /> : <Play size={12} />} {launchLabel}
                </button>
              </div>
            ) : auditStatus === 'Audited' && completedLocalRun && linkedValidation?.finished ? (
              // The finished discussion is auto-archived: validate from here, never start a new one.
              <div className="audit-tl-validation-actions" data-testid="audit-timeline-validation-finished">
                <p className="audit-tl-muted">{t('auditTimeline.validation.finished')}</p>
                {linkedValidation.started_by && (
                  <p className="audit-tl-muted" data-testid="audit-timeline-validation-started-by">
                    {t('auditTimeline.validation.startedBy', linkedValidation.started_by)}
                  </p>
                )}
                {validationBlock ? (
                  // The server would refuse: the reason, not a button that fails.
                  <p className="audit-tl-muted" data-testid="audit-timeline-validation-block">
                    {t(`auditTimeline.validation.block.${validationBlock}`)}
                  </p>
                ) : (
                  <button
                    type="button"
                    className="audit-tl-btn audit-tl-btn-small"
                    disabled={marking}
                    onClick={() => {
                      setMarking(true);
                      void props.onMarkValid().finally(() => setMarking(false));
                    }}
                  >
                    {marking ? <Loader2 size={12} className="spin" /> : <ShieldCheck size={12} />} {t('audit.validate')}
                  </button>
                )}
                <button
                  type="button"
                  className="audit-tl-btn audit-tl-btn-small"
                  onClick={() => props.onOpenValidation(linkedValidation.discussion_id)}
                >
                  <FileText size={12} /> {t('auditTimeline.validation.viewDiscussion')}
                </button>
              </div>
            ) : auditStatus === 'Audited' && completedLocalRun && linkedValidation && !linkedValidation.finished && !auditActive ? (
              <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={() => props.onOpenValidation(linkedValidation.discussion_id)}>
                <FileText size={12} /> {t('audit.resumeValidation')}
              </button>
            ) : auditStatus === 'Audited' && stepsLoaded && !loadFailed && !completedLocalRun && !runPending ? (
              <p className="audit-tl-muted" data-testid="audit-timeline-validation-unavailable">
                {/* A peer or clone that never ran the audit is read-only: say it. */}
                {validationBlock === 'no_local_run'
                  ? t('auditTimeline.validation.block.no_local_run')
                  : t('auditTimeline.validation.noCompletedRun')}
              </p>
            ) : auditStatus === 'Audited' && completedLocalRun && linkedValidation === null ? (
              <p className="audit-tl-muted" data-testid="audit-timeline-validation-unlinked">{t('auditTimeline.validation.unlinked')}</p>
            ) : null}
          </Phase>

          <Phase
            status={validated ? 'done' : 'blocked'}
            eyebrow={t('auditTimeline.phase.n', 4)}
            title={t('auditTimeline.validated.title')}
            tag={validated ? t('auditTimeline.status.done') : t('auditTimeline.status.upcoming')}
            last
          >
            <p className="audit-tl-muted">{t('auditTimeline.validated.desc')}</p>
            {validated && (validatedBy || recordedStartedBy) && (
              <p className="audit-tl-muted" data-testid="audit-timeline-validated-by">
                {[
                  recordedStartedBy ? t('auditTimeline.validation.startedBy', recordedStartedBy) : null,
                  validatedBy
                    ? t('auditTimeline.validation.validatedBy', validatedBy, recordedValidatedAt ?? '')
                    : null,
                ].filter(Boolean).join(' · ')}
              </p>
            )}
            {validated && props.techDebtEntry}
          </Phase>
        </ol>

        <aside className="audit-tl-side">
          <section className={`audit-tl-card${locked ? ' is-locked' : ''}`} aria-labelledby="audit-tl-agent-h">
            <h3 id="audit-tl-agent-h">{t('auditTimeline.agent.title')}</h3>
            <p className="audit-tl-muted">{t('auditTimeline.agent.why')}</p>
            {locked && selected && selectedTier && (
              <p className="audit-tl-note" data-testid="audit-timeline-auditor">
                {t('auditTimeline.agent.running', [
                  selectedConnection?.display_name ?? AGENT_LABELS[selected] ?? selected,
                  `${MODEL_TIER_ICONS[selectedTier]} ${t(`disc.tier.${selectedTier}`)}`,
                  tierModel(selectedTier),
                ].filter(Boolean).join(' · '))}
              </p>
            )}
            {!locked && props.selectionFromRun && selected && selectedTier && (
              <p className="audit-tl-note" data-testid="audit-timeline-selection-from-run">
                {t('auditTimeline.agent.fromRun', `${MODEL_TIER_ICONS[selectedTier]} ${t(`disc.tier.${selectedTier}`)}`)}
              </p>
            )}
            {!locked && props.selectionFromRun && props.selectionConnectionGone && selected && (
              <p className="audit-tl-note" role="status" data-testid="audit-timeline-connection-gone">
                {t('auditTimeline.agent.connectionGone', AGENT_LABELS[selected] ?? selected)}
              </p>
            )}
            {agentChoices.length === 0 && <p className="audit-tl-muted">{t('disc.noAgent')}</p>}
            {agentChoices.map(group => (
              <div key={group.kind} className="audit-tl-agent-group">
                <div className="audit-tl-eyebrow">{t(`auditTimeline.agent.group.${group.kind}`)}</div>
                <div className="audit-tl-agent-grid">
                  {group.items.map(choice => {
                    const on = selected === choice.agent
                      && (choice.connection?.id ?? null) === (selectedConnectionId ?? null);
                    return (
                      <button
                        key={choice.key}
                        type="button"
                        className={`audit-tl-choice${on ? ' is-on' : ''}`}
                        aria-pressed={on}
                        disabled={!choice.usable || locked}
                        onClick={() => props.onSelect(choice.agent, selectedTier ?? props.selectedTier, choice.connection?.id ?? null)}
                      >
                        <span className="audit-tl-choice-head">
                          <span>{choice.label}</span>
                          {choice.kind === 'cli' && choice.usable && <span className="audit-tl-tag is-reco">{t('auditTimeline.agent.reco')}</span>}
                        </span>
                        <span className="audit-tl-choice-hint">
                          {!choice.usable ? t(choice.kind === 'cli' ? 'auditTimeline.agent.notInstalled' : 'auditTimeline.agent.notConfigured')
                            : choice.connection ? (AGENT_LABELS[choice.agent] ?? t('auditTimeline.agent.http'))
                              : choice.kind === 'cli' ? t('auditTimeline.agent.installed')
                                : choice.kind === 'http' ? t('auditTimeline.agent.http') : t('auditTimeline.agent.local')}
                        </span>
                      </button>
                    );
                  })}
                </div>
              </div>
            ))}
            <div className="audit-tl-eyebrow">{t('auditTimeline.agent.level')}</div>
            <div className="audit-tl-tiers">
              {TIERS.map(tier => (
                <button
                  key={tier}
                  type="button"
                  className={`audit-tl-choice audit-tl-tier${selectedTier === tier ? ' is-on' : ''}`}
                  aria-pressed={selectedTier === tier}
                  disabled={!selected || locked}
                  onClick={() => selected && props.onSelect(selected, tier, selectedConnectionId)}
                >
                  <span className="audit-tl-choice-head">
                    <span><span aria-hidden="true">{MODEL_TIER_ICONS[tier]}</span> {t(`disc.tier.${tier}`)}</span>
                    {tier === 'reasoning' && <span className="audit-tl-tag is-reco">{t('auditTimeline.agent.recommended')}</span>}
                  </span>
                  {tierModel(tier) && <span className="audit-tl-choice-hint audit-tl-mono">{tierModel(tier)}</span>}
                </button>
              ))}
            </div>
            {selected && (selectedKind !== 'cli' || selectedTier !== 'reasoning') && (
              <div className="audit-tl-warn" data-testid="audit-timeline-agent-warning">
                <AlertTriangle size={13} aria-hidden="true" />
                <div>
                  {selectedKind === 'http' && <p>{t('auditTimeline.agent.warnHttp')}</p>}
                  {selectedKind === 'local' && <p>{t('auditTimeline.agent.warnLocal')}</p>}
                  {selectedTier !== 'reasoning' && <p>{t('auditTimeline.agent.warnTier')}</p>}
                  {selectedTier !== 'reasoning' && !locked && (
                    <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={() => props.onSelect(selected, 'reasoning', selectedConnectionId)}>
                      {t('auditTimeline.agent.backToReco')}
                    </button>
                  )}
                </div>
              </div>
            )}
            {audited && !resumable && !props.resumableUnknown && !auditActive && (
              <p className="audit-tl-muted" data-testid="audit-timeline-reverify-hint">{t('auditTimeline.reverifyHint')}</p>
            )}
            <button
              type="button"
              className="audit-tl-btn audit-tl-btn-primary"
              onClick={verifyEntry ? (props.onVerifyDocs ?? props.onLaunch) : props.onLaunch}
              disabled={verifyEntry
                ? auditActive || !!props.resumableUnknown
                : auditActive || launchBlocked || !selected || !agentChoices.some(g => g.items.some(i => i.usable))}
              data-testid="audit-timeline-launch"
            >
              {auditActive ? <Loader2 size={13} className="spin" aria-hidden="true" /> : <Play size={13} />} {launchLabel}
            </button>
            {runPending && props.onRelaunchFull && !confirmRelaunch && (
              <button
                type="button"
                className="audit-tl-btn audit-tl-btn-small"
                onClick={() => setConfirmRelaunch(true)}
                disabled={launchBlocked || !selected}
                data-testid="audit-timeline-relaunch-full"
              >
                {t('auditTimeline.relaunchFull')}
              </button>
            )}
            {runPending && confirmRelaunch && (
              <div className="audit-tl-warn" role="dialog" aria-label={t('auditTimeline.confirmRelaunch.title')} data-testid="audit-timeline-relaunch-confirm">
                <AlertTriangle size={13} aria-hidden="true" />
                <div>
                  <p><strong>{t('auditTimeline.confirmRelaunch.title')}</strong></p>
                  <p>{t('auditTimeline.confirmRelaunch.body', failed.length)}</p>
                  <div className="audit-tl-validation-actions">
                    <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={() => { setConfirmRelaunch(false); props.onLaunch(); }} data-testid="audit-timeline-relaunch-confirm-resume">
                      <RotateCcw size={12} /> {launchLabel}
                    </button>
                    <button
                      type="button"
                      className="audit-tl-btn audit-tl-btn-small"
                      onClick={() => {
                        setConfirmRelaunch(false);
                        setOpenGroups(Object.fromEntries(groups.map(g => [g.key, g.rows.some(r => r.status === 'failed' || r.status === 'todo')])));
                      }}
                      data-testid="audit-timeline-relaunch-confirm-review"
                    >
                      {t('auditTimeline.confirmRelaunch.review')}
                    </button>
                    <button type="button" className="audit-tl-btn audit-tl-btn-small audit-tl-btn-warn" onClick={() => { setConfirmRelaunch(false); props.onRelaunchFull?.(); }} data-testid="audit-timeline-relaunch-confirm-full">
                      {t('auditTimeline.confirmRelaunch.full')}
                    </button>
                    <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={() => setConfirmRelaunch(false)}>
                      {t('auditTimeline.step.acceptCancel')}
                    </button>
                  </div>
                </div>
              </div>
            )}
          </section>
        </aside>
      </div>
    </div>
  );
}

const STEP_DESCRIPTIONS = new Set([
  'AGENTS', 'glossary', 'repo-map', 'coding-rules', 'testing-quality', 'overview', 'debug-operations',
  'inconsistencies-tech-debt', 'decisions', 'inconsistencies-security', 'inconsistencies-docker',
  'inconsistencies-performance', 'inconsistencies-accessibility', 'inconsistencies-database',
  'inconsistencies-api', 'inconsistencies-code-quality',
]);

function stepSlug(file: string): string {
  return file.split('/').pop()?.replace(/\.md$/, '') ?? '';
}

/** Translation key for what a step produces, when Kronn knows that step. */
function stepDescriptionKey(file: string): string | null {
  const slug = stepSlug(file);
  return STEP_DESCRIPTIONS.has(slug) ? `auditTimeline.stepDesc.${slug}` : null;
}

function stepGroup(file: string): 'core' | 'specialists' | 'consolidation' | null {
  const slug = stepSlug(file);
  if (!slug) return null;
  if (slug === 'decisions') return 'consolidation';
  if (slug.startsWith('inconsistencies-') && slug !== 'inconsistencies-tech-debt') return 'specialists';
  return 'core';
}

function isCliCandidate(agent: AgentType): boolean {
  return !LOCAL_AGENTS.has(agent) && !HTTP_AGENTS.has(agent) && agent !== 'Vibe';
}

/** The backend warning is technical English; show its gist, keep it as title. */
function stepReason(warning: string, t: (key: string, ...args: (string | number)[]) => string): string {
  if (/placeholders remain/i.test(warning)) return t('auditTimeline.reason.unfilled');
  if (/tool rounds|round ceiling|budget reached/i.test(warning)) return t('auditTimeline.reason.rounds');
  if (/rate.?limit|quota|429/i.test(warning)) return t('auditTimeline.reason.quota');
  if (/stopped during step/i.test(warning)) return t('auditTimeline.reason.stopped');
  if (/interrupt|cancel/i.test(warning)) return t('auditTimeline.reason.interrupted');
  if (/looks truncated/i.test(warning)) return t('auditTimeline.reason.truncated');
  if (/rewrote nothing|unchanged from before/i.test(warning)) return t('auditTimeline.reason.unchanged');
  if (/no output|missing or empty/i.test(warning)) return t('auditTimeline.reason.noOutput');
  if (/broken citation|citation path|dimension coverage/i.test(warning)) return t('auditTimeline.reason.checks');
  return warning.length > 140 ? `${warning.slice(0, 140)}…` : warning;
}

function Phase(props: {
  status: StepStatus | 'blocked';
  eyebrow: string;
  title: string;
  tag: string;
  last?: boolean;
  children?: React.ReactNode;
}) {
  return (
    <li className={`audit-tl-phase${props.last ? ' is-last' : ''}`}>
      <span className="audit-tl-rail" aria-hidden="true">
        <span className={`audit-tl-dot audit-tl-dot-lg is-${props.status}`}>
          {props.status === 'done' && <Check size={10} />}
          {props.status === 'failed' && <X size={10} />}
        </span>
      </span>
      <div className="audit-tl-phase-body">
        <div className="audit-tl-phase-head">
          <div>
            <div className="audit-tl-eyebrow">{props.eyebrow}</div>
            <div className="audit-tl-phase-title">{props.title}</div>
          </div>
          <span className={`audit-tl-tag is-${props.status}`}>{props.tag}</span>
        </div>
        {props.children}
      </div>
    </li>
  );
}
