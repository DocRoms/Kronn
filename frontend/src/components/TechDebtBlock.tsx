import { lazy, Suspense, useCallback, useEffect, useState } from 'react';
import { AlertTriangle, ListChecks, MessageSquare, ShieldCheck } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { projects as projectsApi } from '../lib/api';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import type { AgentDetection, ModelTiersConfig, Project, TdReviewList } from '../types/generated';
import { TdProgressBar } from './TdProgressBar';
import './TechDebt.css';

const TechDebtReview = lazy(() => import('./TechDebtReview').then(m => ({ default: m.TechDebtReview })));

export interface TechDebtBlockProps {
  project: Project;
  auditActive: boolean;
  agents: AgentDetection[];
  modelTiers?: ModelTiersConfig | null;
  toast: (msg: string, type: 'success' | 'error' | 'warning' | 'info') => void;
  onOpenDiscussion: (discId: string) => void;
  onNavigate: (page: string) => void;
  onRefetch: () => void;
}

/** "Dette technique" block of the project card: progress, the Validated rule, and the review entry. */
export function TechDebtBlock({
  project, auditActive, agents, modelTiers, toast, onOpenDiscussion, onNavigate, onRefetch,
}: TechDebtBlockProps) {
  const { t } = useT();
  const [list, setList] = useState<TdReviewList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const hasDebt = (project.tech_debt_count ?? 0) > 0;

  const reload = useCallback(async () => {
    try {
      setList(await projectsApi.auditTds(project.id));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [project.id]);

  useEffect(() => {
    if (!hasDebt || auditActive) return;
    let cancelled = false;
    projectsApi.auditTds(project.id)
      .then(data => { if (!cancelled) { setList(data); setError(null); } })
      .catch(e => { if (!cancelled) setError(e instanceof Error ? e.message : String(e)); });
    return () => { cancelled = true; };
  }, [project.id, hasDebt, auditActive]);

  const validate = useAsyncGuard(async () => {
    try {
      await projectsApi.validateAudit(project.id);
      toast(t('techDebt.validateDone'), 'success');
      onRefetch();
      await reload();
    } catch (e) {
      toast(t('techDebt.validateError', e instanceof Error ? e.message : String(e)), 'error');
    }
  });

  if (auditActive || !hasDebt) return null;
  if (error && !list) {
    return (
      <section className="td-block" data-testid="td-block">
        <div className="td-block-error">{t('techDebt.loadError', error)}</div>
      </section>
    );
  }
  if (!list || list.items.length === 0) return null;

  const total = list.items.length;
  const remaining = list.counts.pending + list.counts.deferred;
  const linkedTasks = list.items.filter(item => item.task).length;
  const { gate } = list;
  const validationId = list.validation_discussion_id;

  return (
    <section className="td-block" data-testid="td-block">
      <header className="td-block-head">
        <AlertTriangle size={15} />
        <strong>{t('techDebt.title')}</strong>
        <span className="td-block-remaining">{t('techDebt.remaining', remaining)}</span>
      </header>
      <TdProgressBar counts={list.counts} total={total} />
      <div className="td-block-severities">
        {list.by_severity.filter(row => row.total > 0).map(row => (
          <span key={row.severity} className="td-sev-count" data-severity={row.severity}>
            <span className="td-sev-dot" data-severity={row.severity} aria-hidden="true" />
            {row.severity} <strong>{row.remaining} / {row.total}</strong>
          </span>
        ))}
      </div>
      <p className="td-rule" data-met={gate.met ? 'true' : 'false'} data-testid="td-rule">
        <ShieldCheck size={13} />
        {list.validated
          ? t('techDebt.validated')
          : gate.met
            ? t('techDebt.ruleMet', gate.blocking_total)
            : t('techDebt.rule', gate.blocking_total, gate.blocking_settled)}
      </p>
      <div className="td-block-actions">
        <button type="button" className="td-primary-btn" data-testid="td-open-review" onClick={() => setOpen(true)}>
          {remaining > 0 ? t('techDebt.open', remaining) : t('techDebt.openAll')}
        </button>
        {gate.met && !list.validated && (
          <button type="button" className="td-secondary-btn" data-testid="td-validate" onClick={() => { void validate(); }}>
            <ShieldCheck size={13} /> {t('techDebt.validate')}
          </button>
        )}
        {validationId && (
          <button
            type="button"
            className="td-link-btn"
            onClick={() => { onOpenDiscussion(validationId); onNavigate('discussions'); }}
          >
            <MessageSquare size={12} /> {t('techDebt.validationLink')}
          </button>
        )}
        {linkedTasks > 0 && (
          <button type="button" className="td-link-btn" onClick={() => onNavigate('planning')}>
            <ListChecks size={12} /> {t('techDebt.tasksLink', linkedTasks)}
          </button>
        )}
      </div>
      {open && (
        <Suspense fallback={null}>
          <TechDebtReview
            project={project}
            list={list}
            agents={agents}
            modelTiers={modelTiers}
            toast={toast}
            onReload={reload}
            onValidate={() => validate()}
            onClose={() => setOpen(false)}
            onOpenDiscussion={onOpenDiscussion}
            onNavigate={onNavigate}
          />
        </Suspense>
      )}
    </section>
  );
}
