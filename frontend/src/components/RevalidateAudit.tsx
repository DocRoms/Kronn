import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { CheckCircle2, ChevronDown, Loader2, RefreshCw, RotateCcw, ShieldCheck, Square, X } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { projects as projectsApi } from '../lib/api';
import type { RevalidationDoneInfo } from '../lib/api';
import type {
  AgentType, CitationStatus, DriftCheckResponse, ModelTier, Project, RevalidationPlan, RevalidationSection,
} from '../types/generated';
import { useDialogFocus } from '../hooks/useDialogFocus';
import { placeholderSteps } from '../lib/placeholderDocs';
import { formatUsd } from '../lib/audit-cost';
import { DocStateSummary } from './DocStateSummary';
import type { DocStateCounts } from './docState';
import './RevalidateAudit.css';

type Phase =
  | { kind: 'loading' }
  | { kind: 'ready' }
  | { kind: 'running'; step: number; total: number; file: string }
  | { kind: 'done'; info: RevalidationDoneInfo }
  | { kind: 'cancelled' }
  | { kind: 'error'; message: string; retry: 'plan' | 'run' };

export interface RevalidateAuditProps {
  gateLaunch?: (launch: () => void) => Promise<void>;
  project: Project;
  /** Another audit of this project is running: no launch meanwhile. */
  auditActive: boolean;
  agent: AgentType;
  tier: ModelTier;
  connectionId: string | null;
  toast: (msg: string, type: 'success' | 'error' | 'warning' | 'info') => void;
  onRefetch: () => void;
  onRefetchDrift: (projectId: string) => void;
  onOpenDiscussion: (discId: string) => void;
  onNavigate: (page: string) => void;
  /** ProjectCard owns the one shared dialog used by its header, timeline and doc viewer. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  hideTrigger?: boolean;
  drift?: DriftCheckResponse;
  reconfirmCount?: number | null;
  docStateUnknown?: boolean;
  onUpdateStale?: (steps: number[]) => void;
  onAcknowledgeStale?: () => void;
  driftAcking?: boolean;
  onFillPlaceholders?: (steps: number[]) => void;
  onReconfirm?: () => void;
  onFullAudit?: () => void;
}

/**
 * « Vérifier la doc »: the deterministic citation check first,
 * then a bounded agent pass on the sections it cannot settle. Every error,
 * stop or interruption offers « Reprendre », which relaunches the same action:
 * the backend recomputes what is left from the fingerprints.
 */
export function RevalidateAudit({
  project, auditActive, agent, tier, connectionId, toast, gateLaunch,
  onRefetch, onRefetchDrift, onOpenDiscussion, onNavigate,
  open: controlledOpen, onOpenChange, hideTrigger = false,
  drift, reconfirmCount = null, docStateUnknown = false,
  onUpdateStale, onAcknowledgeStale, driftAcking = false, onFillPlaceholders, onReconfirm, onFullAudit,
}: RevalidateAuditProps) {
  const { t, locale } = useT();
  const [internalOpen, setInternalOpen] = useState(false);
  const open = controlledOpen ?? internalOpen;
  const setOpen = useCallback((next: boolean) => {
    if (controlledOpen === undefined) setInternalOpen(next);
    onOpenChange?.(next);
  }, [controlledOpen, onOpenChange]);
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [plan, setPlan] = useState<RevalidationPlan | null>(null);
  const [stepTargets, setStepTargets] = useState<Map<number, string>>(new Map());
  const [estimates, setEstimates] = useState<{
    partialDurationMs: number | null;
    partialCostMicros: number | null;
    fullDurationMs: number | null;
    fullCostMicros: number | null;
  } | null>(null);
  const [acking, setAcking] = useState<string | null>(null);
  const runningRef = useRef(false);
  const deterministicOnlyRef = useRef(false);
  const abortRef = useRef<AbortController | null>(null);
  const eligible = project.audit_status === 'Audited' || project.audit_status === 'Validated';

  const loadPlan = useCallback(async (next: Phase = { kind: 'ready' }) => {
    try {
      const [nextPlan, auditSteps, timeline] = await Promise.all([
        projectsApi.revalidationPlan(project.id),
        projectsApi.auditSteps().catch(() => []),
        projectsApi.auditTimeline(project.id).catch(() => null),
      ]);
      setPlan(nextPlan);
      setStepTargets(new Map(auditSteps.map(step => [step.index, step.target_file])));

      if (timeline) {
        const wanted = new Set((drift?.stale_sections ?? []).map(section => section.audit_step));
        const runOrder = new Map(timeline.runs.map((run, index) => [run.id, index]));
        const latestByStep = [...timeline.steps]
          .sort((left, right) => (runOrder.get(left.audit_run_id) ?? Number.MAX_SAFE_INTEGER)
            - (runOrder.get(right.audit_run_id) ?? Number.MAX_SAFE_INTEGER))
          .reduce((byStep, row) => {
            if (!byStep.has(row.step_index)) byStep.set(row.step_index, row);
            return byStep;
          }, new Map<number, (typeof timeline.steps)[number]>());
        const partialRows = [...wanted].flatMap(step => {
          const row = latestByStep.get(step);
          return row ? [row] : [];
        });
        const latestFull = timeline.runs.find(run => run.kind === 'Full' && run.status === 'Completed') ?? null;
        const fullCost = latestFull
          ? await projectsApi.auditRunCost(latestFull.id).then(cost => cost.total.cost_usd_micros ?? null).catch(() => null)
          : null;
        setEstimates({
          partialDurationMs: partialRows.length > 0 && partialRows.every(row => row.duration_ms != null)
            ? partialRows.reduce((sum, row) => sum + (row.duration_ms ?? 0), 0)
            : null,
          partialCostMicros: partialRows.length > 0 && partialRows.every(row => row.cost_usd_micros != null || row.estimated_cost_usd_micros != null)
            ? partialRows.reduce((sum, row) => sum + (row.cost_usd_micros ?? row.estimated_cost_usd_micros ?? 0), 0)
            : null,
          fullDurationMs: latestFull?.duration_ms ?? null,
          fullCostMicros: fullCost,
        });
      } else {
        setEstimates(null);
      }
      setPhase(next);
    } catch (e) {
      setPhase({ kind: 'error', message: String(e instanceof Error ? e.message : e), retry: 'plan' });
    }
  }, [drift?.stale_sections, project.id]);

  const openPanel = useCallback(() => {
    setOpen(true);
  }, [setOpen]);

  const panelRef = useDialogFocus<HTMLDivElement>({ active: open, onEscape: () => setOpen(false) });

  useEffect(() => () => abortRef.current?.abort(), []);

  useEffect(() => {
    if (!open || runningRef.current) return;
    setPhase({ kind: 'loading' });
    void loadPlan();
  }, [open, loadPlan]);

  const settled = useCallback((next: Phase) => {
    runningRef.current = false;
    abortRef.current = null;
    onRefetch();
    onRefetchDrift(project.id);
    void loadPlan(next);
  }, [loadPlan, onRefetch, onRefetchDrift, project.id]);

  const runAudit = useCallback(async (deterministicOnly = false) => {
    if (runningRef.current) return;
    runningRef.current = true;
    deterministicOnlyRef.current = deterministicOnly;
    const controller = new AbortController();
    abortRef.current = controller;
    setPhase({ kind: 'running', step: 0, total: plan?.agent_steps.length ?? 0, file: '' });
    try {
      await projectsApi.revalidateAuditStream(project.id, {
        agent, tier, connection_id: connectionId ?? undefined,
        ...(deterministicOnly ? { deterministic_only: true } : {}),
      }, {
        onStepStart: (step, total, file) => setPhase({ kind: 'running', step, total, file }),
        onWarning: (message) => toast(message, 'warning'),
        onCancelled: () => settled({ kind: 'cancelled' }),
        onDone: (info) => {
          if (!info) { runningRef.current = false; return; }
          settled({ kind: 'done', info });
        },
        onError: (message) => settled({ kind: 'error', message, retry: 'run' }),
      }, controller.signal);
    } catch (e) {
      settled({ kind: 'error', message: String(e instanceof Error ? e.message : e), retry: 'run' });
    }
  }, [agent, tier, connectionId, plan, project.id, settled, toast]);

  const launch = useCallback(async (deterministicOnly = false) => {
    if (gateLaunch) await gateLaunch(() => void runAudit(deterministicOnly));
    else await runAudit(deterministicOnly);
  }, [gateLaunch, runAudit]);

  const stop = useCallback(async () => {
    try {
      await projectsApi.cancelAudit(project.id);
    } catch (e) {
      toast(t('audit.revalidate.error', String(e instanceof Error ? e.message : e)), 'error');
    }
  }, [project.id, t, toast]);

  const acknowledge = useCallback(async (doc: string) => {
    if (acking) return;
    setAcking(doc);
    try {
      await projectsApi.acknowledgeCitations(project.id, doc);
      toast(t('audit.revalidate.ackDone', doc), 'success');
      onRefetchDrift(project.id);
      await loadPlan(phase.kind === 'loading' ? { kind: 'ready' } : phase);
    } catch (e) {
      toast(t('audit.revalidate.ackFailed', String(e instanceof Error ? e.message : e)), 'error');
    } finally {
      setAcking(null);
    }
  }, [acking, loadPlan, onRefetchDrift, phase, project.id, t, toast]);

  if (!eligible) return null;
  const running = phase.kind === 'running';
  const resume = () => {
    if (phase.kind === 'error' && phase.retry === 'plan') {
      setPhase({ kind: 'loading' });
      void loadPlan();
    } else {
      void launch(deterministicOnlyRef.current);
    }
  };

  const stateLabel = (c: CitationStatus) => c.state === 'moved'
    ? t('audit.revalidate.state.moved', String(c.current_line ?? '?'))
    : t(`audit.revalidate.state.${c.state}`);

  const sectionRow = (s: RevalidationSection) => (
    <li key={s.doc} className="rv-section" data-testid="revalidate-section" data-doc={s.doc} data-action={s.action}>
      <div className="rv-section-head">
        <code className="rv-doc">{s.doc}</code>
        <span className="rv-chip" data-action={s.action}>{t(`audit.revalidate.action.${s.action}`)}</span>
        {s.action !== 'reanchor' && (
          <button
            type="button"
            className="rv-btn"
            data-testid="revalidate-ack"
            disabled={running || acking !== null || s.doc_missing}
            onClick={() => void acknowledge(s.doc)}
            title={t('audit.revalidate.ackHint')}
          >
            {acking === s.doc ? <Loader2 size={11} className="spin" /> : <CheckCircle2 size={11} />}
            {t('audit.revalidate.ack')}
          </button>
        )}
      </div>
      {s.action === 'manual' && <p className="rv-hint">{t(s.doc_missing ? 'audit.revalidate.docMissing' : 'audit.revalidate.manualHint')}</p>}
      {s.action === 'agent' && s.broken.length === 0 && s.reanchor.length === 0 && s.human.length === 0
        && (s.relevance ?? []).length === 0 && (
        <p className="rv-hint" data-testid="revalidate-prose-changed">{t('audit.revalidate.proseChanged')}</p>
      )}
      {(s.relevance ?? []).length > 0 && (
        <ul className="rv-relevance" data-testid="revalidate-relevance-sections">
          {(s.relevance ?? []).map(r => (
            <li key={`${r.doc}:${r.heading}`} data-testid="revalidate-relevance-section" data-verdict={r.verdict}>
              <strong>{r.heading || '—'}</strong>{' '}
              <span className="rv-chip" data-verdict={r.verdict}>{t(`audit.revalidate.relevance.${r.verdict}`)}</span>
              {r.untouched_since_code_changed && (
                <span className="rv-hint" data-testid="revalidate-untouched"> {t('audit.revalidate.relevance.untouched')}</span>
              )}
              <ul>
                {r.evidence.map(e => (
                  <li key={e.reference}><code>{e.reference}</code> — {t(`audit.revalidate.state.${e.state}`)}</li>
                ))}
                {r.corrections.map(c => (
                  <li key={`${c.kind}:${c.reference}`} className="rv-hint">
                    {t('audit.revalidate.relevance.proposed')}: {c.proposal}
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ul>
      )}
      <ul className="rv-cites">
        {[...s.broken, ...s.reanchor].map(c => (
          <li key={c.cite}><code>{c.cite}</code> — {stateLabel(c)}</li>
        ))}
        {s.human.map(h => (
          <li key={`${h.section}:${h.citation.cite}`} data-testid="revalidate-human-todo">
            {t('audit.revalidate.humanTodo', h.section)} <code>{h.citation.cite}</code> — {stateLabel(h.citation)}
          </li>
        ))}
      </ul>
    </li>
  );

  const outcome = () => {
    if (phase.kind === 'cancelled') return <p className="rv-outcome" data-tone="warning">{t('audit.revalidate.cancelled')}</p>;
    if (phase.kind === 'error') return <p className="rv-outcome" data-tone="error" role="alert">{t('audit.revalidate.error', phase.message)}</p>;
    if (phase.kind !== 'done') return null;
    const { info } = phase;
    if (info.kind === 'deterministic') {
      return info.status === 'clean'
        ? <p className="rv-outcome" data-tone="success">{t('audit.revalidate.done.clean', String(info.reanchored))}</p>
        : <p className="rv-outcome" data-tone="warning">{t('audit.revalidate.done.todo', String(info.reanchored))}</p>;
    }
    const partial = info.info;
    if (partial.status === 'complete' && partial.discussionId) {
      const id = partial.discussionId;
      return (
        <div className="rv-outcome" data-tone="success">
          <p>{t('audit.revalidate.done.complete')}</p>
          <button type="button" className="rv-btn" onClick={() => { onOpenDiscussion(id); onNavigate('discussions'); }}>
            {t('audit.revalidate.done.openValidation')}
          </button>
        </div>
      );
    }
    // KT-1207 — the done steps of a run that rewrote nothing, or of an
    // interrupted one, may carry their own validation discussion.
    const validationId = partial.discussionId;
    const openValidation = validationId && (
      <button type="button" className="rv-btn" onClick={() => { onOpenDiscussion(validationId); onNavigate('discussions'); }}>
        {t('audit.revalidate.done.openValidation')}
      </button>
    );
    if (partial.status === 'no_change') {
      return (
        <div className="rv-outcome" data-tone="warning">
          <p>{t('audit.revalidate.done.noChange')}</p>
          {openValidation}
        </div>
      );
    }
    return (
      <div className="rv-outcome" data-tone="error" role="alert">
        <p>{t('audit.revalidate.done.interrupted', String(partial.failedSteps.length))}</p>
        {openValidation}
      </div>
    );
  };

  const needsResume = phase.kind === 'error' || phase.kind === 'cancelled'
    || (phase.kind === 'done' && phase.info.kind === 'agent' && phase.info.info.status === 'interrupted');
  const hasTargetedAgentWork = (plan?.agent_steps.length ?? 0) > 0;
  const canLaunch = !needsResume && phase.kind !== 'loading' && !running && !auditActive
    && plan !== null && plan.sections.length > 0
    && (plan.sources_state !== 'unknown' || hasTargetedAgentWork);
  const docStateCounts: DocStateCounts | null = docStateUnknown || !drift || reconfirmCount === null
    || plan === null || plan.sources_state === 'unknown'
    ? null
    : {
      broken: drift.broken_citations ?? 0,
      reanchor: plan.reanchor_count,
      stale: (drift.stale_sections ?? []).length,
      reconfirm: reconfirmCount,
    };
  const staleSteps = [...new Set((drift?.stale_sections ?? []).map(section => section.audit_step))];
  const fillSteps = placeholderSteps(drift?.placeholder_docs ?? []);
  const formatEstimate = (durationMs: number | null | undefined, costMicros: number | null | undefined) => {
    const duration = durationMs == null
      ? t('docState.estimateUnknown')
      : t('docState.estimateDuration', Math.max(1, Math.round(durationMs / 60_000)));
    const cost = costMicros == null
      ? t('docState.estimateUnknown')
      : formatUsd(costMicros, locale);
    return t('docState.estimate', cost, duration);
  };

  return (
    <>
      {!hideTrigger && (
        <button
          type="button"
          className="dash-drift-update-btn"
          data-testid="project-revalidate"
          disabled={auditActive && !running}
          onClick={(e) => { e.stopPropagation(); openPanel(); }}
          title={t('audit.revalidate.buttonHint')}
        >
          {running ? <Loader2 size={9} className="spin" /> : <ShieldCheck size={9} />}
          {t('audit.revalidate.button')}
        </button>
      )}
      {open && createPortal(
        <div className="rv-overlay" role="presentation" onClick={(e) => { e.stopPropagation(); setOpen(false); }}>
          <div
            ref={panelRef}
            className="rv-panel"
            role="dialog"
            aria-modal="true"
            aria-label={t('audit.revalidate.title')}
            data-testid="revalidate-panel"
            data-phase={phase.kind}
            onClick={(e) => e.stopPropagation()}
          >
            <header className="rv-header">
              <h2>{t('audit.revalidate.title')}</h2>
              <button type="button" className="rv-icon-btn" aria-label={t('common.close')} onClick={() => setOpen(false)}>
                <X size={14} />
              </button>
            </header>
            <p className="rv-intro">{t('audit.revalidate.intro')}</p>
            <DocStateSummary
              counts={docStateCounts}
              variant="block"
              actions={{
                broken: {
                  label: needsResume ? t('audit.revalidate.resume') : t('docState.action.fixBroken'),
                  run: needsResume ? resume : () => void launch(),
                  disabled: auditActive || (!needsResume && !canLaunch),
                },
                reanchor: {
                  label: needsResume ? t('audit.revalidate.resume') : t('docState.action.reanchor'),
                  run: needsResume ? resume : () => void launch(true),
                  disabled: auditActive || (!needsResume && !canLaunch),
                },
                stale: {
                  label: t('docState.action.updateStale'),
                  run: () => onUpdateStale?.(staleSteps),
                  disabled: auditActive || !onUpdateStale,
                },
                reconfirm: {
                  label: t('docState.action.reconfirm'),
                  run: () => onReconfirm?.(),
                  disabled: auditActive || !onReconfirm,
                },
              }}
            />

            {(drift?.stale_sections.length ?? 0) > 0 && (
              <section className="rv-stale" data-testid="revalidate-stale-details">
                <h3>{t('docState.stale.title')}</h3>
                <p>{t('docState.stale.explanation')}</p>
                <ul>
                  {drift?.stale_sections.map(section => (
                    <li key={`${section.ai_file}:${section.audit_step}`}>
                      <strong><code>{section.ai_file}</code></strong>
                      <span>{t('docState.stale.step', section.audit_step, stepTargets.get(section.audit_step) ?? section.ai_file)}</span>
                      <span>{t('docState.stale.sources')}</span>
                      <ul>
                        {section.changed_sources.map(source => <li key={source}><code>{source}</code></li>)}
                      </ul>
                    </li>
                  ))}
                </ul>
                <p>{t('docState.stale.updateEffect')}</p>
                <p>{t('docState.stale.unchangedEffect')}</p>
                <p className="rv-hint">{formatEstimate(estimates?.partialDurationMs, estimates?.partialCostMicros)}</p>
                <div className="rv-actions">
                  {onUpdateStale && (
                    <button type="button" className="rv-btn rv-btn-primary" data-testid="revalidate-stale-update" disabled={auditActive} onClick={() => onUpdateStale(staleSteps)}>
                      <RefreshCw size={11} /> {t('docState.stale.update', staleSteps.length)}
                    </button>
                  )}
                  {onAcknowledgeStale && (
                    <button type="button" className="rv-btn" data-testid="revalidate-stale-ack" disabled={auditActive || driftAcking} onClick={onAcknowledgeStale}>
                      {driftAcking ? <Loader2 size={11} className="spin" /> : <CheckCircle2 size={11} />}
                      {t('audit.driftAck')}
                    </button>
                  )}
                </div>
              </section>
            )}

            {fillSteps.length > 0 && onFillPlaceholders && (
              <div className="rv-placeholder-action">
                <p>{t('docState.placeholders.explanation')}</p>
                <button type="button" className="rv-btn" data-testid="revalidate-placeholder-fill" disabled={auditActive} onClick={() => onFillPlaceholders(fillSteps)}>
                  <RefreshCw size={11} /> {t('audit.fillPlaceholders', fillSteps.length)}
                </button>
              </div>
            )}

            {(reconfirmCount ?? 0) > 0 && onReconfirm && (
              <div className="rv-placeholder-action" data-testid="revalidate-reconfirm-action">
                <p>{t('docState.reconfirm.explanation')}</p>
                <button type="button" className="rv-btn" disabled={auditActive} onClick={onReconfirm}>
                  <CheckCircle2 size={11} /> {t('docReconfirm.button', reconfirmCount ?? 0)}
                </button>
              </div>
            )}

            {phase.kind === 'loading' && <p className="rv-hint"><Loader2 size={11} className="spin" /> {t('audit.revalidate.loading')}</p>}
            {running && (
              <p className="rv-progress" data-testid="revalidate-progress">
                <Loader2 size={11} className="spin" />{' '}
                {phase.step > 0
                  ? t('audit.revalidate.runningStep', String(phase.step), String(phase.total), phase.file)
                  : t('audit.revalidate.running')}
              </p>
            )}
            {outcome()}

            {plan && phase.kind !== 'loading' && (
              <>
                {plan.sources_state === 'unknown' && (
                  <p className="rv-hint" data-testid="revalidate-unknown">
                    {t(hasTargetedAgentWork ? 'audit.revalidate.unknownWithAgentWork' : 'audit.revalidate.unknown')}
                  </p>
                )}
                {plan.relevance && plan.relevance.sections > 0 && (
                  <p className="rv-hint" data-testid="revalidate-relevance-counts">
                    {t('audit.revalidate.relevance',
                      String(plan.relevance.still_true),
                      String(plan.relevance.still_true + plan.relevance.outdated + plan.relevance.no_longer_relevant),
                      String(plan.relevance.outdated), String(plan.relevance.no_longer_relevant),
                      String(plan.relevance.unanchored), String(plan.relevance.untouched_stale))}
                  </p>
                )}
                {plan.sections.length === 0
                  ? plan.sources_state !== 'unknown' && (
                    <p className="rv-hint" data-testid="revalidate-clean">{t('audit.revalidate.clean')}</p>
                  )
                  : (
                    <>
                      <p className="rv-summary">
                        {t('audit.revalidate.summary',
                          String(plan.sections.length), String(plan.reanchor_count),
                          String(plan.agent_steps.length), String(plan.human_count))}
                      </p>
                      <ul className="rv-sections">{plan.sections.map(sectionRow)}</ul>
                    </>
                  )}
              </>
            )}

            <footer className="rv-footer">
              {running && (
                <button type="button" className="rv-btn" data-testid="revalidate-stop" onClick={() => void stop()}>
                  <Square size={11} /> {t('audit.revalidate.stop')}
                </button>
              )}
              {needsResume && (
                <button type="button" className="rv-btn rv-btn-primary" data-testid="revalidate-resume" disabled={auditActive} onClick={resume}>
                  <RotateCcw size={11} /> {t('audit.revalidate.resume')}
                </button>
              )}
              {canLaunch && (
                <button type="button" className="rv-btn rv-btn-primary" data-testid="revalidate-launch" onClick={() => void launch()}>
                  <RefreshCw size={11} /> {t('audit.revalidate.launch')}
                </button>
              )}
            </footer>

            {onFullAudit && (
              <details className="rv-full-audit">
                <summary><ChevronDown size={12} /> {t('docState.fullAudit.title')}</summary>
                <p>{t('docState.fullAudit.explanation')}</p>
                <p className="rv-hint">{formatEstimate(estimates?.fullDurationMs, estimates?.fullCostMicros)}</p>
                <button type="button" className="rv-btn" disabled={auditActive} onClick={onFullAudit}>
                  <RotateCcw size={11} /> {t('docState.fullAudit.action')}
                </button>
              </details>
            )}
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
