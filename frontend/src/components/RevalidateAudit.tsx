import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { CheckCircle2, Loader2, RefreshCw, RotateCcw, ShieldCheck, Square, X } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { projects as projectsApi } from '../lib/api';
import type { RevalidationDoneInfo } from '../lib/api';
import type {
  AgentType, CitationStatus, ModelTier, Project, RevalidationPlan, RevalidationSection,
} from '../types/generated';
import './RevalidateAudit.css';

type Phase =
  | { kind: 'loading' }
  | { kind: 'ready' }
  | { kind: 'running'; step: number; total: number; file: string }
  | { kind: 'done'; info: RevalidationDoneInfo }
  | { kind: 'cancelled' }
  | { kind: 'error'; message: string; retry: 'plan' | 'run' };

export interface RevalidateAuditProps {
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
}

/**
 * « Revalider l'audit » (KT-1168): the deterministic citation check first,
 * then a bounded agent pass on the sections it cannot settle. Every error,
 * stop or interruption offers « Reprendre », which relaunches the same action:
 * the backend recomputes what is left from the fingerprints.
 */
export function RevalidateAudit({
  project, auditActive, agent, tier, connectionId, toast,
  onRefetch, onRefetchDrift, onOpenDiscussion, onNavigate,
}: RevalidateAuditProps) {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [plan, setPlan] = useState<RevalidationPlan | null>(null);
  const [acking, setAcking] = useState<string | null>(null);
  const runningRef = useRef(false);
  const abortRef = useRef<AbortController | null>(null);
  const eligible = project.audit_status === 'Audited' || project.audit_status === 'Validated';

  const loadPlan = useCallback(async (next: Phase = { kind: 'ready' }) => {
    try {
      setPlan(await projectsApi.revalidationPlan(project.id));
      setPhase(next);
    } catch (e) {
      setPhase({ kind: 'error', message: String(e instanceof Error ? e.message : e), retry: 'plan' });
    }
  }, [project.id]);

  const openPanel = useCallback(() => {
    setOpen(true);
    if (!runningRef.current) {
      setPhase({ kind: 'loading' });
      void loadPlan();
    }
  }, [loadPlan]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(false); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  useEffect(() => () => abortRef.current?.abort(), []);

  const settled = useCallback((next: Phase) => {
    runningRef.current = false;
    abortRef.current = null;
    onRefetch();
    onRefetchDrift(project.id);
    void loadPlan(next);
  }, [loadPlan, onRefetch, onRefetchDrift, project.id]);

  const launch = useCallback(async () => {
    if (runningRef.current) return;
    runningRef.current = true;
    const controller = new AbortController();
    abortRef.current = controller;
    setPhase({ kind: 'running', step: 0, total: plan?.agent_steps.length ?? 0, file: '' });
    try {
      await projectsApi.revalidateAuditStream(project.id, {
        agent, tier, connection_id: connectionId ?? undefined,
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
      void launch();
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
    if (partial.status === 'no_change') return <p className="rv-outcome" data-tone="warning">{t('audit.revalidate.done.noChange')}</p>;
    return <p className="rv-outcome" data-tone="error" role="alert">{t('audit.revalidate.done.interrupted', String(partial.failedSteps.length))}</p>;
  };

  const needsResume = phase.kind === 'error' || phase.kind === 'cancelled'
    || (phase.kind === 'done' && phase.info.kind === 'agent' && phase.info.info.status === 'interrupted');
  const canLaunch = !needsResume && phase.kind !== 'loading' && !running && !auditActive
    && plan !== null && plan.sources_state !== 'unknown' && plan.sections.length > 0;

  return (
    <>
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
      {open && createPortal(
        <div className="rv-overlay" role="presentation" onClick={(e) => { e.stopPropagation(); setOpen(false); }}>
          <div
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
              plan.sources_state === 'unknown'
                ? <p className="rv-hint" data-testid="revalidate-unknown">{t('audit.revalidate.unknown')}</p>
                : plan.sections.length === 0
                  ? <p className="rv-hint" data-testid="revalidate-clean">{t('audit.revalidate.clean')}</p>
                  : (
                    <>
                      <p className="rv-summary">
                        {t('audit.revalidate.summary',
                          String(plan.sections.length), String(plan.reanchor_count),
                          String(plan.agent_steps.length), String(plan.human_count))}
                      </p>
                      <ul className="rv-sections">{plan.sections.map(sectionRow)}</ul>
                    </>
                  )
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
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
