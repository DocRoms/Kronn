import { lazy, Suspense, useCallback, useEffect, useState } from 'react';
import { AlertTriangle, FileText, ListChecks, MessageSquare, ShieldCheck } from 'lucide-react';
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

// Every mounted entry to the review (overview block, audit step 4) shows the same
// list: a fetch from any of them is published to all the others of that project.
const listeners = new Map<string, Set<(list: TdReviewList) => void>>();

function publishList(projectId: string, list: TdReviewList) {
  listeners.get(projectId)?.forEach(listener => listener(list));
}

// Reads are ordered per project by start time: a response only gets published if no
// read that started after it has already been published, so a read begun before a
// decision's reload can never overwrite the reload's result.
const readsStarted = new Map<string, number>();
const readsPublished = new Map<string, number>();

/** Fetches the project's list and publishes it to every entry, unless a newer read already did. */
async function fetchAndPublish(projectId: string): Promise<TdReviewList> {
  const ticket = (readsStarted.get(projectId) ?? 0) + 1;
  readsStarted.set(projectId, ticket);
  const data = await projectsApi.auditTds(projectId);
  if (ticket > (readsPublished.get(projectId) ?? 0)) {
    readsPublished.set(projectId, ticket);
    publishList(projectId, data);
  }
  return data;
}

/** The project's debt list, its reload and the Validated action: one source for every entry to the review. */
function useTechDebtList(
  project: Project,
  auditActive: boolean,
  toast: TechDebtBlockProps['toast'],
  onRefetch: () => void,
) {
  const { t } = useT();
  const [list, setList] = useState<TdReviewList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const hasDebt = (project.tech_debt_count ?? 0) > 0;

  useEffect(() => {
    const set = listeners.get(project.id) ?? new Set();
    const listener = (next: TdReviewList) => { setList(next); setError(null); };
    set.add(listener);
    listeners.set(project.id, set);
    return () => {
      set.delete(listener);
      if (set.size === 0) listeners.delete(project.id);
    };
  }, [project.id]);

  const reload = useCallback(async () => {
    try {
      await fetchAndPublish(project.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [project.id]);

  useEffect(() => {
    if (!hasDebt || auditActive) return;
    let cancelled = false;
    fetchAndPublish(project.id)
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

  return { list, error, reload, validate };
}

interface TechDebtReviewButtonProps extends Pick<TechDebtBlockProps,
  'project' | 'agents' | 'modelTiers' | 'toast' | 'onOpenDiscussion' | 'onNavigate'> {
  list: TdReviewList;
  onReload: () => Promise<void>;
  onValidate: () => Promise<unknown>;
  testId: string;
}

/** The control that opens the tech-debt review screen: the overview and the audit timeline both render this one. */
function TechDebtReviewButton({
  project, list, agents, modelTiers, toast, onReload, onValidate, onOpenDiscussion, onNavigate, testId,
}: TechDebtReviewButtonProps) {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const remaining = list.counts.pending + list.counts.deferred;

  return (
    <>
      <button type="button" className="td-primary-btn" data-testid={testId} onClick={() => setOpen(true)}>
        {remaining > 0 ? t('techDebt.open', remaining) : t('techDebt.openAll')}
      </button>
      {open && (
        <Suspense fallback={null}>
          <TechDebtReview
            project={project}
            list={list}
            agents={agents}
            modelTiers={modelTiers}
            toast={toast}
            onReload={onReload}
            onValidate={() => onValidate()}
            onClose={() => setOpen(false)}
            onOpenDiscussion={onOpenDiscussion}
            onNavigate={onNavigate}
          />
        </Suspense>
      )}
    </>
  );
}

export interface TechDebtTimelineEntryProps extends TechDebtBlockProps {
  /** Opens the raw `inconsistencies-tech-debt.md` in the docs viewer. */
  onOpenFile: () => void;
}

/** Step 4 of the audit timeline: the overview's review control, the raw file as a secondary link, an explicit empty state. */
export function TechDebtTimelineEntry({
  project, auditActive, agents, modelTiers, toast, onOpenDiscussion, onNavigate, onRefetch, onOpenFile,
}: TechDebtTimelineEntryProps) {
  const { t } = useT();
  const { list, error, reload, validate } = useTechDebtList(project, auditActive, toast, onRefetch);
  const hasDebt = (project.tech_debt_count ?? 0) > 0;

  if (!hasDebt || (list && list.items.length === 0)) {
    return <p className="audit-tl-muted" data-testid="audit-timeline-td-empty">{t('auditTimeline.validated.noDebt')}</p>;
  }
  return (
    <div className="td-block-actions" data-testid="audit-timeline-td-entry">
      {list ? (
        <TechDebtReviewButton
          project={project}
          list={list}
          agents={agents}
          modelTiers={modelTiers}
          toast={toast}
          onReload={reload}
          onValidate={validate}
          onOpenDiscussion={onOpenDiscussion}
          onNavigate={onNavigate}
          testId="audit-timeline-td-open-review"
        />
      ) : error ? (
        <span className="td-block-error">{t('techDebt.loadError', error)}</span>
      ) : null}
      <button type="button" className="td-link-btn" data-testid="audit-timeline-td-open-file" onClick={onOpenFile}>
        <FileText size={12} /> {t('auditTimeline.validated.openFile')}
      </button>
    </div>
  );
}

/** "Dette technique" block of the project card: progress, the Validated rule, and the review entry. */
export function TechDebtBlock({
  project, auditActive, agents, modelTiers, toast, onOpenDiscussion, onNavigate, onRefetch,
}: TechDebtBlockProps) {
  const { t } = useT();
  const { list, error, reload, validate } = useTechDebtList(project, auditActive, toast, onRefetch);
  const hasDebt = (project.tech_debt_count ?? 0) > 0;

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
  // The debts the project card counts: a rejected finding or a kept-on-purpose
  // choice is settled, not active debt.
  const active = list.counts.pending + list.counts.confirmed + list.counts.deferred;
  const linkedTasks = list.items.filter(item => item.task).length;
  const { gate } = list;
  const validationId = list.validation_discussion_id;

  return (
    <section className="td-block" data-testid="td-block">
      <header className="td-block-head">
        <AlertTriangle size={15} />
        <strong>{t('techDebt.title')}</strong>
        <span className="td-block-active" data-testid="td-active-count">{t('techDebt.active', active)}</span>
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
      {gate.open_disputes > 0 && (
        <p className="td-disputes" data-testid="td-disputes">
          <AlertTriangle size={13} />
          {t('techDebt.disputes', gate.open_disputes)}
          {list.dispute_discussion_id && (
            <button
              type="button"
              className="td-link-btn"
              data-testid="td-disputes-link"
              onClick={() => { onOpenDiscussion(list.dispute_discussion_id as string); }}
            >
              <MessageSquare size={12} /> {t('techDebt.disputesLink')}
            </button>
          )}
        </p>
      )}
      <div className="td-block-actions">
        <TechDebtReviewButton
          project={project}
          list={list}
          agents={agents}
          modelTiers={modelTiers}
          toast={toast}
          onReload={reload}
          onValidate={validate}
          onOpenDiscussion={onOpenDiscussion}
          onNavigate={onNavigate}
          testId="td-open-review"
        />
        {gate.met && !list.validated && (
          <button type="button" className="td-secondary-btn" data-testid="td-validate" onClick={() => { void validate(); }}>
            <ShieldCheck size={13} /> {t('techDebt.validate')}
          </button>
        )}
        {validationId && (
          <button
            type="button"
            className="td-link-btn"
            onClick={() => { onOpenDiscussion(validationId); }}
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
    </section>
  );
}
