/**
 * KT-977 — the audit tab as a timeline: briefing, template, the audit's steps
 * grouped as the pipeline runs them, validation, validated. Presentational:
 * every action is a handler the ProjectCard already owns.
 */
import { useEffect, useMemo, useState } from 'react';
import {
  Check, ChevronDown, ChevronRight, FileText, Loader2, Play, RotateCcw,
  ShieldCheck, Sparkles, StopCircle, X, AlertTriangle,
} from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { projects as projectsApi, externalApi, type ExternalApiConnectionView } from '../lib/api';
import { AGENT_LABELS, MODEL_TIER_ICONS, isUsable } from '../lib/constants';
import { canRunAudit } from '../lib/agentCapabilities';
import { formatStepList } from '../lib/audit-resume';
import { BriefingForm } from './BriefingForm';
import type { AgentDetection, AgentType, AuditEntry, ModelTier, ModelTiersConfig } from '../types/generated';
import './AuditTimeline.css';

type StepStatus = 'done' | 'failed' | 'warned' | 'running' | 'pending' | 'todo';

interface StepRow {
  step_index: number;
  file_label: string;
  started_at: string;
  ended_at?: string | null;
  duration_ms?: number | null;
  cli_success: boolean;
  step_warning?: string | null;
  step_tokens?: number | null;
  input_tokens?: number | null;
  output_tokens?: number | null;
  cache_read_tokens?: number | null;
  carried_from_run_id?: string | null;
}

export interface AuditTimelineProps {
  projectId: string;
  auditStatus: string;
  techDebtCount: number;
  agents: AgentDetection[];
  selectedAgent: AgentType | null;
  selectedTier: ModelTier;
  modelTiers: ModelTiersConfig | null | undefined;
  /** Named external API connection of the selected HTTP agent, if any. */
  selectedConnectionId: string | null;
  onSelect: (agent: AgentType, tier: ModelTier, connectionId: string | null) => void;
  briefingDone: boolean;
  onBriefingSaved: () => void;
  auditActive: boolean;
  liveStep: number;
  liveTotal: number;
  liveFile: string;
  liveElapsed: string | null;
  liveTool: string | null;
  /** Wall-clock start of the live audit, to tell its steps from older runs'. */
  liveStartedAt: number | null;
  liveToolCalls: number | null;
  liveStepTokens: number | null;
  liveTotalTokens: number | null;
  /** Who runs the live audit, as the server reports it: shown and frozen in
   *  the agent panel, whichever client launched the audit. */
  liveAuditor?: { agent: AgentType; tier: ModelTier; connectionId: string | null } | null;
  /** An older conversational briefing still open: resume it instead. */
  onResumeBriefingDiscussion: (() => void) | null;
  onCancel: () => void;
  resumable: { id: string; steps_to_redo?: number[]; last_completed_step: number } | null;
  onLaunch: () => void;
  validationInProgress: boolean;
  onValidate: () => void;
  onViewTechDebts: () => void;
  refreshTrigger: number;
  toast: (msg: string, kind: 'success' | 'error' | 'info' | 'warning') => void;
}

const NEWS_KEY = 'kr.audit.timeline.news.0.14.2';
const LOCAL_AGENTS = new Set<AgentType>(['Ollama']);
const HTTP_AGENTS = new Set<AgentType>(['LiteLlm', 'Nvidia', 'Custom']);
const TIERS: ModelTier[] = ['reasoning', 'default', 'economy'];

const PRESET_AGENT: Record<string, AgentType> = {
  lite_llm: 'LiteLlm', nvidia: 'Nvidia', open_router: 'Custom', other: 'Custom',
};

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

function readNewsDismissed(): boolean {
  try { return localStorage.getItem(NEWS_KEY) === '1'; } catch { return false; }
}

function stepStatus(row: StepRow | undefined, index: number, redo: Set<number>, active: boolean): StepStatus {
  if (!row) return redo.has(index) ? 'todo' : 'pending';
  // Without a live audit, a step that never ended was interrupted.
  if (!row.ended_at) return active ? 'running' : 'failed';
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
      const latest = data.runs.find(run => run.kind === 'Full');
      return { id: ids[0] ?? null, rows: [...merged.values()], td: latest?.td_total ?? 0, recorded: data.recorded_audits };
    };
    load()
      .then(({ id, rows, td, recorded: entries }) => {
        if (alive) { setRunId(id); setSteps(rows); setTdTotal(td); setRecorded(entries); setStepsLoaded(true); }
      })
      .catch(() => { if (alive) { setSteps([]); setRecorded([]); setStepsLoaded(true); } });
    return () => { alive = false; };
  }, [projectId, resumable, refreshTrigger, auditActive, liveStep]);

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
      let status = stepStatus(row, index, redo, auditActive);
      if (auditActive && index === liveIndex) status = 'running';
      const file = row?.file_label ?? plan.get(index) ?? (auditActive && index === liveIndex ? props.liveFile : '');
      return { index, row, status, file, descriptionKey: stepDescriptionKey(file) };
    });
  }, [steps, total, redo, auditActive, liveIndex, props.liveFile, plan, resumable, props.liveStartedAt, partialRun]);

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

  const failed = rows.filter(r => r.status === 'failed' || r.status === 'todo').map(r => r.index);
  const done = rows.filter(r => r.status === 'done' || r.status === 'warned').length;
  const consolidationIndex = rows.find(r => stepGroup(r.file) === 'consolidation')?.index;
  const consolidationRedone = consolidationIndex !== undefined && redo.has(consolidationIndex)
    && [...redo].some(i => i !== consolidationIndex);

  const agentChoices = useMemo(() => {
    const kindOf = (agent: AgentType): AgentChoice['kind'] =>
      LOCAL_AGENTS.has(agent) ? 'local' : HTTP_AGENTS.has(agent) ? 'http' : 'cli';
    // A named connection replaces the bare provider slot of the same preset.
    const connected = new Set(connections.map(c => PRESET_AGENT[c.origin_preset] ?? 'Custom'));
    const detected: AgentChoice[] = props.agents
      .filter(a => canRunAudit(a) || (!isUsable(a) && isCliCandidate(a.agent_type)))
      .filter(a => !(HTTP_AGENTS.has(a.agent_type) && connected.has(a.agent_type)))
      .map(a => ({
        key: a.agent_type, agent: a.agent_type, label: AGENT_LABELS[a.agent_type] ?? a.name,
        usable: canRunAudit(a), kind: kindOf(a.agent_type), connection: null,
      }));
    const named: AgentChoice[] = connections.map(c => ({
      key: `conn:${c.id}`, agent: PRESET_AGENT[c.origin_preset] ?? 'Custom', label: c.display_name,
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

  const validated = auditStatus === 'Validated';
  const audited = auditStatus === 'Audited' || validated;
  const templateInstalled = auditStatus !== 'NoTemplate';
  const auditNodeStatus: StepStatus = auditActive ? 'running'
    : failed.length > 0 ? 'failed' : audited ? 'done' : 'pending';
  const launchLabel = auditActive ? t('auditTimeline.launch.running') : resumable
    ? (resumable.steps_to_redo && resumable.steps_to_redo.length > 0
      ? t('audit.resumeRedoSteps', formatStepList(resumable.steps_to_redo))
      : t('audit.resumeFromStep', resumable.last_completed_step + 1))
    : audited ? t('auditTimeline.relaunch') : t('audit.startFullAudit');

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

      <div className="audit-tl-layout">
        <ol className="audit-tl-phases" aria-label={t('auditTimeline.ariaPhases')}>
          <Phase
            status={props.briefingDone ? 'done' : 'pending'}
            eyebrow={t('auditTimeline.phase.optional', 0)}
            title={t('auditTimeline.briefing.title')}
            tag={props.briefingDone ? t('auditTimeline.status.saved') : t('auditTimeline.status.toFill')}
          >
            {props.briefingDone ? (
              <p className="audit-tl-muted">{t('auditTimeline.briefing.used')}</p>
            ) : props.onResumeBriefingDiscussion ? (
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
              />
            ) : (
              <div className="audit-tl-row">
                <p className="audit-tl-muted">{t('auditTimeline.briefing.intro')}</p>
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
                <div className="audit-tl-progress" role="progressbar" aria-valuemin={0} aria-valuemax={total} aria-valuenow={done} aria-label={t('auditTimeline.audit.title')}>
                  <div style={{ width: `${Math.round((done / total) * 100)}%` }} />
                </div>
              </>
            )}
            {auditActive && (
              <div className="audit-tl-live" data-testid="audit-timeline-live">
                <Loader2 size={13} className="spin" aria-hidden="true" />
                <span>{t('audit.step', liveIndex, partialRun ? total : props.liveTotal, props.liveFile)}</span>
                {props.liveElapsed && <span className="audit-tl-muted">{props.liveElapsed}</span>}
                {props.liveTool && (
                  <span className="audit-tl-muted">
                    {t('audit.currentTool', props.liveTool)}
                    {props.liveToolCalls != null && props.liveToolCalls > 0 && ` (${props.liveToolCalls})`}
                  </span>
                )}
                {props.liveTotalTokens != null && props.liveTotalTokens > 0 && (
                  <span className="audit-tl-muted">{t('audit.totalTokens', formatTokens(props.liveTotalTokens, locale))}</span>
                )}
                <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={props.onCancel}>
                  <StopCircle size={12} /> {t('audit.cancelAudit')}
                </button>
              </div>
            )}
            {consolidationRedone && (
              <p className="audit-tl-note">{t('auditTimeline.audit.consolidationNote')}</p>
            )}
            {groups.map(group => {
              const groupFailed = group.rows.some(r => r.status === 'failed' || r.status === 'todo');
              const open = openGroups[group.key] ?? (groupFailed || group.rows.some(r => r.status === 'running'));
              const groupDone = group.rows.filter(r => r.status === 'done' || r.status === 'warned').length;
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
                      <button type="button" className="audit-tl-btn audit-tl-btn-small audit-tl-btn-warn" onClick={props.onLaunch}>
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
                            {r.row?.step_warning ? (
                              <span className={`audit-tl-reason is-${r.status}`} title={r.row.step_warning}>
                                {stepReason(r.row.step_warning, t)}
                              </span>
                            ) : r.row && !r.row.ended_at && !auditActive && (
                              <span className="audit-tl-reason is-failed">{t('auditTimeline.reason.interrupted')}</span>
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
                              const part = (v?: number | null) => (v == null ? '—' : v.toLocaleString(locale));
                              return (
                                <span
                                  className="audit-tl-mono audit-tl-muted"
                                  data-testid={`audit-timeline-step-tokens-${r.index}`}
                                  title={r.status === 'running' ? undefined : [
                                    t('auditTimeline.tokens.detail',
                                      part(r.row?.input_tokens), part(r.row?.output_tokens), part(r.row?.cache_read_tokens)),
                                    r.row?.carried_from_run_id ? t('auditTimeline.tokens.carried', r.row.carried_from_run_id.slice(0, 8)) : '',
                                  ].filter(Boolean).join(' · ')}
                                >
                                  {t('auditTimeline.tokens.short', formatTokens(tokens, locale))}
                                </span>
                              );
                            })()}
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
            {runId === null && total === 0 && !auditActive && recorded.length === 0 && (
              <p className="audit-tl-muted">{t('auditTimeline.audit.notStarted')}</p>
            )}
          </Phase>

          <Phase
            status={validated ? 'done' : props.validationInProgress ? 'running' : audited ? 'pending' : 'blocked'}
            eyebrow={t('auditTimeline.phase.n', 3)}
            title={t('auditTimeline.validation.title')}
            tag={validated ? t('auditTimeline.status.done') : props.validationInProgress ? t('auditTimeline.status.running')
              : audited ? t('auditTimeline.status.todo') : t('auditTimeline.status.waitingAudit')}
          >
            <p className="audit-tl-muted">{t('auditTimeline.validation.desc')}</p>
            {audited && tdTotal > 0 && (
              <p className="audit-tl-muted" data-testid="audit-timeline-td-count">{t('auditTimeline.validation.tdCount', tdTotal)}</p>
            )}
            {audited && !validated && (
              <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={props.onValidate}>
                <ShieldCheck size={12} /> {props.validationInProgress ? t('audit.resumeValidation') : t('audit.validate')}
              </button>
            )}
          </Phase>

          <Phase
            status={validated ? 'done' : 'blocked'}
            eyebrow={t('auditTimeline.phase.n', 4)}
            title={t('auditTimeline.validated.title')}
            tag={validated ? t('auditTimeline.status.done') : t('auditTimeline.status.upcoming')}
            last
          >
            <p className="audit-tl-muted">{t('auditTimeline.validated.desc')}</p>
            {validated && props.techDebtCount > 0 && (
              <button type="button" className="audit-tl-btn audit-tl-btn-small" onClick={props.onViewTechDebts}>
                {t('audit.viewTechDebts', props.techDebtCount)}
              </button>
            )}
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
            <button
              type="button"
              className="audit-tl-btn audit-tl-btn-primary"
              onClick={props.onLaunch}
              disabled={auditActive || !selected || !agentChoices.some(g => g.items.some(i => i.usable))}
              data-testid="audit-timeline-launch"
            >
              {auditActive ? <Loader2 size={13} className="spin" aria-hidden="true" /> : <Play size={13} />} {launchLabel}
            </button>
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
  if (/interrupt|cancel/i.test(warning)) return t('auditTimeline.reason.interrupted');
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
