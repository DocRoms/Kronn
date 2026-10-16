import { lazy, memo, Suspense, useCallback, useEffect, useMemo, useState } from 'react';
import { ExternalLink, FileCode2, ListChecks, MessageSquare, Search, ShieldCheck, X } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { externalApi, projects as projectsApi } from '../lib/api';
import { useApi } from '../hooks/useApi';
import { isUsable } from '../lib/constants';
import { externalAgentTargets, unusableExternalAgentTargets } from '../lib/externalAgentIdentity';
import { defaultCompareTargets, type CompareTarget } from '../lib/compareTargets';
import type { AgentSwitchTarget } from './AgentSwitchPicker';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import { useDialogFocus } from '../hooks/useDialogFocus';
import {
  groupTds, isTdSettled, nextUnsettled, shortTdId, TD_DECISIONS, TD_STATE_OF_DECISION, visibleTds,
  type TdFilter,
} from '../lib/techDebt';
import type {
  AgentDetection, ModelTiersConfig, Project, TdDecisionItem, TdPointer, TdReviewDecision,
  TdReviewItem, TdReviewList,
} from '../types/generated';
import { TdMarkdown } from './TdMarkdown';
import { TdProgressBar } from './TdProgressBar';
import { TdTalkPanel } from './TdTalkPanel';
import './TechDebt.css';

const SourceCodeViewer = lazy(() => import('./SourceCodeViewer').then(m => ({ default: m.SourceCodeViewer })));

export interface TechDebtReviewProps {
  project: Project;
  list: TdReviewList;
  agents: AgentDetection[];
  modelTiers?: ModelTiersConfig | null;
  toast: (msg: string, type: 'success' | 'error' | 'warning' | 'info') => void;
  onReload: () => Promise<void>;
  onValidate: () => void;
  onClose: () => void;
  onOpenDiscussion: (discId: string) => void;
  onNavigate: (page: string) => void;
}

interface TdRowProps {
  item: TdReviewItem;
  current: boolean;
  checked: boolean;
  onSelect: (id: string) => void;
  onToggle: (id: string) => void;
}

const TdRow = memo(function TdRow({ item, current, checked, onSelect, onToggle }: TdRowProps) {
  const { t } = useT();
  return (
    <li className="td-row" data-current={current ? 'true' : 'false'} data-testid={`td-row-${item.id}`}>
      <input
        type="checkbox"
        className="td-row-check"
        checked={checked}
        onChange={() => onToggle(item.id)}
        aria-label={item.id}
      />
      <button
        type="button"
        className="td-row-main"
        aria-current={current ? 'true' : undefined}
        onClick={() => onSelect(item.id)}
        title={item.title}
      >
        <span className="td-sev-dot" data-severity={item.severity} aria-hidden="true" />
        <span className="td-row-id">{shortTdId(item.id)}</span>
        <span className="td-row-title">{item.title}</span>
        <span className="td-chip">{item.area}</span>
        <span className="td-pill" data-state={item.state}>{t(`techDebt.state.${item.state}`)}</span>
        {item.talk_count > 0 && (
          <span className="td-row-talk" title={t('techDebt.talkCount', item.talk_count)}>
            <MessageSquare size={11} aria-label={t('techDebt.talkCount', item.talk_count)} />
          </span>
        )}
      </button>
    </li>
  );
});

/** Full-screen review of the audit's tech-debt sheets: filters, grouped list, bulk bar, detail and talk. */
export function TechDebtReview({
  project, list, agents, modelTiers, toast, onReload, onValidate, onClose, onOpenDiscussion, onNavigate,
}: TechDebtReviewProps) {
  const { t, locale } = useT();
  const [filter, setFilter] = useState<TdFilter>('todo');
  const [severities, setSeverities] = useState<ReadonlySet<string>>(new Set());
  const [query, setQuery] = useState('');
  const [checked, setChecked] = useState<ReadonlySet<string>>(new Set());
  const ordered = useMemo(() => groupTds(list.items).flatMap(group => group.items), [list.items]);
  const [currentId, setCurrentId] = useState<string | null>(
    () => ordered.find(item => !isTdSettled(item.state))?.id ?? ordered[0]?.id ?? null,
  );
  const [reasonFor, setReasonFor] = useState<{ id: string | null; text: string }>({ id: null, text: '' });
  const [viewer, setViewer] = useState<TdPointer | null>(null);
  const [busy, setBusy] = useState(false);
  // The viewer covers the review, so it is modal too and takes Escape first.
  const dialogRef = useDialogFocus<HTMLDivElement>({ onEscape: onClose });
  const viewerRef = useDialogFocus<HTMLDivElement>({ active: viewer !== null, onEscape: () => setViewer(null) });
  const { data: connections } = useApi(() => externalApi.list(), []);
  const installedAgentTypes = useMemo(() => agents.filter(isUsable).map(a => a.agent_type), [agents]);
  const talkChoices = useMemo<AgentSwitchTarget[]>(() => [
    ...installedAgentTypes.map(agent => ({ agent })),
    ...externalAgentTargets(connections ?? []).map(target => ({
      agent: target.agent,
      connectionId: target.connectionId,
      label: target.label,
      modelTiers: target.modelTiers,
    })),
  ], [installedAgentTypes, connections]);
  const unusableTalkChoices = useMemo(() => unusableExternalAgentTargets(connections ?? []), [connections]);
  const [talkTargets, setTalkTargets] = useState<CompareTarget[] | null>(null);

  const visible = useMemo(
    () => visibleTds(ordered, filter, severities, query),
    [ordered, filter, severities, query],
  );
  const groups = useMemo(() => groupTds(visible), [visible]);
  const current = useMemo(
    () => list.items.find(item => item.id === currentId) ?? null,
    [list.items, currentId],
  );
  const reason = reasonFor.id === currentId ? reasonFor.text : '';
  const total = list.items.length;
  const todoCount = list.counts.pending + list.counts.deferred;

  useEffect(() => { dialogRef.current?.focus(); }, [dialogRef]);

  const onSelect = useCallback((id: string) => setCurrentId(id), []);
  const onToggle = useCallback((id: string) => {
    setChecked(previous => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const toggleGroup = (items: TdReviewItem[], select: boolean) => {
    setChecked(previous => {
      const next = new Set(previous);
      for (const item of items) {
        if (select) next.add(item.id);
        else next.delete(item.id);
      }
      return next;
    });
  };

  const toggleSeverity = (severity: string) => {
    setSeverities(previous => {
      const next = new Set(previous);
      if (next.has(severity)) next.delete(severity);
      else next.add(severity);
      return next;
    });
  };

  const decide = useAsyncGuard(async (items: TdDecisionItem[]) => {
    setBusy(true);
    try {
      const outcome = await projectsApi.decideTds(project.id, items);
      await onReload();
      return outcome;
    } catch (e) {
      toast(t('techDebt.decideError', e instanceof Error ? e.message : String(e)), 'error');
      return null;
    } finally {
      setBusy(false);
    }
  });

  const decideCurrent = async (decision: TdReviewDecision) => {
    if (!current) return;
    const next = nextUnsettled(ordered, current.id);
    const text = reason.trim();
    const outcome = await decide([{ td_id: current.id, decision, ...(text ? { reason: text } : {}) }]);
    if (outcome && next && decision !== 'defer') setCurrentId(next.id);
  };

  const decideChecked = async (decision: TdReviewDecision) => {
    const items = ordered.filter(item => checked.has(item.id)).map(item => ({ td_id: item.id, decision }));
    if (items.length === 0) return;
    const outcome = await decide(items);
    if (outcome) {
      toast(t('techDebt.bulk.done', outcome.results.length), 'success');
      setChecked(new Set());
    }
  };

  const createTask = useAsyncGuard(async () => {
    if (!current) return;
    try {
      const outcome = await projectsApi.createTdTask(project.id, current.id);
      const row = outcome.items.find(item => item.td_id === current.id);
      if (row?.task_reference) {
        toast(t('techDebt.taskCreated', row.task_reference, current.id), 'success');
      } else {
        toast(t('techDebt.taskSkipped'), 'info');
      }
      await onReload();
    } catch (e) {
      toast(t('techDebt.taskError', e instanceof Error ? e.message : String(e)), 'error');
    }
  });

  const goNext = () => {
    const next = nextUnsettled(ordered, currentId);
    if (next) setCurrentId(next.id);
  };

  const openValidation = () => {
    if (!list.validation_discussion_id) return;
    onOpenDiscussion(list.validation_discussion_id);
    onNavigate('discussions');
    onClose();
  };

  const auditDate = list.audit_date
    ? new Date(list.audit_date).toLocaleDateString(locale, { day: 'numeric', month: 'long', year: 'numeric' })
    : null;
  const position = current ? visible.findIndex(item => item.id === current.id) : -1;
  const { gate } = list;
  const severityRows = list.by_severity.filter(row => row.total > 0);

  return (
    <div className="td-review-overlay" role="presentation">
      <div
        className="td-review"
        role="dialog"
        aria-modal="true"
        aria-label={t('techDebt.review.title')}
        tabIndex={-1}
        ref={dialogRef}
        data-testid="td-review"
      >
        <header className="td-review-head">
          <div className="td-review-titles">
            <h2>{t('techDebt.review.title')}</h2>
            <span className="td-review-sub">
              {project.name} · {auditDate
                ? t('techDebt.review.subtitle', auditDate, total)
                : t('techDebt.review.subtitleNoDate', total)}
            </span>
          </div>
          <div className="td-review-progress">
            <TdProgressBar counts={list.counts} total={total} />
            <p className="td-rule" data-met={gate.met ? 'true' : 'false'}>
              <ShieldCheck size={13} />
              {list.validated
                ? t('techDebt.validated')
                : gate.met
                  ? t('techDebt.ruleMet', gate.blocking_total)
                  : t('techDebt.rule', gate.blocking_total, gate.blocking_settled)}
              {gate.met && !list.validated && (
                <button type="button" className="td-secondary-btn" onClick={onValidate}>
                  {t('techDebt.validate')}
                </button>
              )}
            </p>
          </div>
          <button type="button" className="td-icon-btn" onClick={onClose} aria-label={t('techDebt.review.close')}>
            <X size={16} />
          </button>
        </header>

        <div className="td-review-filters">
          <div className="td-segmented" role="group">
            {([
              ['todo', t('techDebt.filter.todo', todoCount)],
              ['done', t('techDebt.filter.done', total - todoCount)],
              ['all', t('techDebt.filter.all', total)],
            ] as Array<[TdFilter, string]>).map(([id, label]) => (
              <button
                key={id}
                type="button"
                aria-pressed={filter === id}
                data-testid={`td-filter-${id}`}
                onClick={() => setFilter(id)}
              >
                {label}
              </button>
            ))}
          </div>
          <div className="td-sev-chips" role="group" aria-label={t('techDebt.filter.severity')}>
            {severityRows.map(row => (
              <button
                key={row.severity}
                type="button"
                className="td-sev-chip"
                aria-pressed={severities.has(row.severity)}
                onClick={() => toggleSeverity(row.severity)}
              >
                <span className="td-sev-dot" data-severity={row.severity} aria-hidden="true" />
                {row.severity} {row.remaining}/{row.total}
              </button>
            ))}
          </div>
          <label className="td-search">
            <Search size={13} aria-hidden="true" />
            <input
              type="search"
              value={query}
              onChange={event => setQuery(event.target.value)}
              placeholder={t('techDebt.search')}
              aria-label={t('techDebt.search')}
            />
          </label>
        </div>

        {checked.size > 0 && (
          <div className="td-bulk" data-testid="td-bulk">
            <span>{t('techDebt.bulk.selected', checked.size)}</span>
            {TD_DECISIONS.map(decision => (
              <button
                key={decision}
                type="button"
                className="td-bulk-btn"
                data-decision={decision}
                disabled={busy}
                onClick={() => { void decideChecked(decision); }}
              >
                {t(`techDebt.decision.${decision}`)}
              </button>
            ))}
            <button type="button" className="td-link-btn" onClick={() => setChecked(new Set())}>
              {t('techDebt.bulk.cancel')}
            </button>
          </div>
        )}

        <div className="td-review-body">
          <div className="td-list" aria-label={t('techDebt.title')}>
            {groups.length === 0 && <p className="td-empty">{t('techDebt.noMatch')}</p>}
            {groups.map(group => {
              const allChecked = group.items.every(item => checked.has(item.id));
              return (
                <section key={group.severity} className="td-group">
                  <div className="td-group-head">
                    <span className="td-sev-dot" data-severity={group.severity} aria-hidden="true" />
                    <strong>{group.severity}</strong>
                    <span className="td-group-count">{group.items.length}</span>
                    <button
                      type="button"
                      className="td-link-btn"
                      data-testid={`td-group-toggle-${group.severity}`}
                      onClick={() => toggleGroup(group.items, !allChecked)}
                    >
                      {allChecked ? t('techDebt.group.selectNone') : t('techDebt.group.selectAll')}
                    </button>
                  </div>
                  <ul className="td-rows">
                    {group.items.map(item => (
                      <TdRow
                        key={item.id}
                        item={item}
                        current={item.id === currentId}
                        checked={checked.has(item.id)}
                        onSelect={onSelect}
                        onToggle={onToggle}
                      />
                    ))}
                  </ul>
                </section>
              );
            })}
          </div>

          <div className="td-detail" data-testid="td-detail">
            {!current ? (
              <p className="td-empty">{t('techDebt.detail.empty')}</p>
            ) : (
              <>
                <div className="td-detail-top">
                  <span className="td-sev-dot" data-severity={current.severity} aria-hidden="true" />
                  <code>{current.id}</code>
                  <span className="td-pill" data-state={current.state}>{t(`techDebt.state.${current.state}`)}</span>
                  {position >= 0 && (
                    <span className="td-detail-pos">{position + 1} / {visible.length}</span>
                  )}
                </div>
                <h3 className="td-detail-title">{current.title}</h3>
                <div className="td-detail-chips">
                  <span className="td-chip">{t('techDebt.detail.severity')} : {current.severity}</span>
                  <span className="td-chip">{t('techDebt.detail.area')} : {current.area}</span>
                  {current.effort && <span className="td-chip">{t('techDebt.detail.effort')} : {current.effort}</span>}
                  {current.blast_radius && <span className="td-chip">{t('techDebt.detail.blast')} : {current.blast_radius}</span>}
                </div>
                {current.problem && (
                  <section className="td-detail-section">
                    <h4>{t('techDebt.detail.problem')}</h4>
                    <p>{current.problem}</p>
                  </section>
                )}
                {current.impact && (
                  <section className="td-detail-section">
                    <h4>{t('techDebt.detail.impact')}</h4>
                    <TdMarkdown text={current.impact} />
                  </section>
                )}
                {current.pointers.length > 0 && (
                  <section className="td-detail-section">
                    <h4>{t('techDebt.detail.where')}</h4>
                    <div className="td-pointers">
                      {current.pointers.map(pointer => (
                        <button
                          key={`${pointer.path}:${pointer.line ?? ''}`}
                          type="button"
                          className="td-pointer"
                          onClick={() => setViewer(pointer)}
                        >
                          <FileCode2 size={11} />
                          {pointer.path}{pointer.line ? `:${pointer.line}` : ''}
                        </button>
                      ))}
                    </div>
                  </section>
                )}
                <section className="td-detail-section" data-testid="td-verification">
                  <h4>{t('techDebt.detail.verification')}</h4>
                  {current.verification
                    ? <p>{current.verification}</p>
                    : <p className="td-missing" data-testid="td-no-verification">{t('techDebt.detail.noVerification')}</p>}
                </section>
                <p className="td-detail-source">{t('techDebt.detail.source', current.source_path)}</p>

                <section className="td-detail-section">
                  <h4>{t('techDebt.detail.yourDecision')}</h4>
                  <div className="td-decisions">
                    {TD_DECISIONS.map(decision => (
                      <button
                        key={decision}
                        type="button"
                        className="td-decision"
                        data-decision={decision}
                        data-testid={`td-decide-${decision}`}
                        aria-pressed={current.state === TD_STATE_OF_DECISION[decision]}
                        disabled={busy}
                        onClick={() => { void decideCurrent(decision); }}
                      >
                        <strong>{t(`techDebt.decision.${decision}`)}</strong>
                        <span>{t(`techDebt.decision.${decision}Hint`)}</span>
                      </button>
                    ))}
                  </div>
                  <label className="td-reason">
                    <span>{t('techDebt.detail.reason')}</span>
                    <textarea
                      value={reason}
                      maxLength={2000}
                      onChange={event => setReasonFor({ id: currentId, text: event.target.value })}
                      placeholder={t('techDebt.detail.reasonPlaceholder')}
                      rows={2}
                    />
                  </label>
                </section>

                <div className="td-detail-links">
                  {current.task ? (
                    <button
                      type="button"
                      className="td-link-btn"
                      data-testid="td-task-link"
                      onClick={() => { if (current.task) onNavigate(`planning:${current.task.id}`); onClose(); }}
                    >
                      <ListChecks size={12} /> {t('techDebt.link.task', current.task.reference)}
                    </button>
                  ) : current.id.startsWith('TD-') && (
                    <button
                      type="button"
                      className="td-link-btn"
                      data-testid="td-task-create"
                      onClick={() => { void createTask(); }}
                    >
                      {t('techDebt.link.createTask')}
                    </button>
                  )}
                  <button type="button" className="td-link-btn" disabled title={t('techDebt.link.trackerSoon')}>
                    {t('techDebt.link.github')}
                  </button>
                  <button type="button" className="td-link-btn" disabled title={t('techDebt.link.trackerSoon')}>
                    {t('techDebt.link.jira')}
                  </button>
                  <button
                    type="button"
                    className="td-secondary-btn td-next"
                    data-testid="td-next"
                    disabled={!nextUnsettled(ordered, currentId)}
                    onClick={goNext}
                  >
                    {nextUnsettled(ordered, currentId) ? t('techDebt.detail.next') : t('techDebt.detail.noneLeft')}
                  </button>
                </div>

                <TdTalkPanel
                  key={current.id}
                  projectId={project.id}
                  item={current}
                  choices={talkChoices}
                  unusable={unusableTalkChoices}
                  installedAgentTypes={installedAgentTypes}
                  targets={talkTargets ?? defaultCompareTargets(talkChoices.slice(0, 1), 'default')}
                  onTargetsChange={targets => setTalkTargets(targets ?? defaultCompareTargets(talkChoices, 'default'))}
                  modelTiers={modelTiers}
                  onTalkChanged={onReload}
                  onOpenDiscussion={(id) => { onOpenDiscussion(id); onNavigate('discussions'); onClose(); }}
                />
                {list.validation_discussion_id && (
                  <button type="button" className="td-link-btn" onClick={openValidation}>
                    <ExternalLink size={12} /> {t('techDebt.talk.openValidation')}
                  </button>
                )}
              </>
            )}
          </div>
        </div>

        {viewer && (
          <div className="td-viewer" role="dialog" aria-modal="true" aria-label={viewer.path} ref={viewerRef}>
            <div className="td-viewer-head">
              <code>{viewer.path}{viewer.line ? `:${viewer.line}` : ''}</code>
              <button type="button" className="td-icon-btn" onClick={() => setViewer(null)} aria-label={t('techDebt.viewer.close')}>
                <X size={15} />
              </button>
            </div>
            <div className="td-viewer-body">
              <Suspense fallback={null}>
                <SourceCodeViewer projectId={project.id} initialPath={viewer.path} initialLine={viewer.line ?? null} />
              </Suspense>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
