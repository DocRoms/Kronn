import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Archive,
  ArrowRight,
  Check,
  ChevronDown,
  ChevronRight,
  Circle,
  Flag,
  Focus,
  FolderKanban,
  Loader2,
  Inbox,
  List,
  MessageCircle,
  Plus,
  Target,
  X,
} from 'lucide-react';
import { planning } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import { CopyIdPill } from './CopyIdPill';
import { PlanAllTasksView } from './PlanAllTasksView';
import { PlanningProposalReview } from './PlanningProposalReview';
import type {
  DiscussionPlan,
  PlanningDiscussionRelation,
  PlanningProposal,
  PlanningTaskDetail,
  PlanningTaskSummary,
  PlanningTaskStatus,
  ProposalListResponse,
} from '../types/generated';
import type { ToastFn } from '../hooks/useToast';
import './DiscussionToolPanel.css';
import './DiscussionPlanPanel.css';

interface Props {
  discussionId: string;
  onClose: () => void;
  onChanged?: (plan: DiscussionPlan) => void;
  onNavigateDiscussion?: (discussionId: string) => void;
  onNavigateProject?: (projectId: string) => void;
  toast: ToastFn;
}

type PlanFilter = 'ready' | 'blocked' | 'done' | 'later';

export function DiscussionPlanPanel({
  discussionId,
  onClose,
  onChanged,
  onNavigateDiscussion,
  onNavigateProject,
  toast,
}: Props) {
  const { t } = useT();
  const [plan, setPlan] = useState<DiscussionPlan | null>(null);
  const [proposalInbox, setProposalInbox] = useState<ProposalListResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [quickTitle, setQuickTitle] = useState('');
  const [creating, setCreating] = useState(false);
  const [primaryOpen, setPrimaryOpen] = useState(true);
  const [viewMode, setViewMode] = useState<'focus' | 'all'>('focus');
  const [allQuery, setAllQuery] = useState('');
  const [planFilter, setPlanFilter] = useState<PlanFilter | null>(null);
  const [selectedTask, setSelectedTask] = useState<PlanningTaskDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [dodSavingIndex, setDodSavingIndex] = useState<number | null>(null);
  const selectedTaskIdRef = useRef<string | null>(null);

  useEffect(() => {
    selectedTaskIdRef.current = selectedTask?.id ?? null;
  }, [selectedTask?.id]);

  const refresh = useCallback(async (silent = false) => {
    if (!silent) {
      setLoading(true);
      setError('');
    }
    try {
      const selectedId = selectedTaskIdRef.current;
      const [next, nextDetail, nextProposals] = await Promise.all([
        planning.discussionPlan(discussionId),
        selectedId ? planning.get(selectedId) : Promise.resolve(null),
        planning.proposals(discussionId),
      ]);
      setPlan(next);
      setProposalInbox(nextProposals);
      if (nextDetail && selectedTaskIdRef.current === selectedId) {
        setSelectedTask(nextDetail);
      }
      onChanged?.(next);
    } catch (cause) {
      if (!silent) setError(userError(cause));
    } finally {
      if (!silent) setLoading(false);
    }
  }, [discussionId, onChanged]);

  const updateProposal = (updated: PlanningProposal) => {
    setProposalInbox(previous => {
      if (!previous) return previous;
      const proposals = previous.proposals
        .map(candidate => candidate.id === updated.id ? updated : candidate)
        .filter(candidate => (
          candidate.aggregate_state === 'pending'
          || candidate.aggregate_state === 'partial'
        ));
      return {
        proposals,
        pending_proposal_count: proposals.length,
        pending_item_count: proposals.reduce(
          (count, candidate) => count
            + candidate.items.filter(item => item.state === 'pending').length,
          0,
        ),
      };
    });
  };

  useEffect(() => {
    void refresh();
    const refreshProposals = (event: Event) => {
      const changedDiscussionId = (
        event as CustomEvent<{ discussionId?: string }>
      ).detail?.discussionId;
      if (changedDiscussionId === discussionId) void refresh(true);
    };
    window.addEventListener('kronn:plan-proposals-changed', refreshProposals);
    const interval = window.setInterval(() => {
      if (!document.hidden) void refresh(true);
    }, 5_000);
    return () => {
      window.removeEventListener('kronn:plan-proposals-changed', refreshProposals);
      window.clearInterval(interval);
    };
  }, [discussionId, refresh]);

  const current = useMemo(
    () => plan?.active.filter(item => (
      item.task.status === 'in_progress' && item.active_blockers.length === 0
    )) ?? [],
    [plan],
  );
  const upcoming = useMemo(
    () => plan?.active.filter(item => item.actionable) ?? [],
    [plan],
  );
  const visibleCurrent = current.slice(0, 3);
  const visibleUpcoming = upcoming.slice(0, 5);
  const linkedTaskIds = useMemo(
    () => new Set([
      ...(plan?.active ?? []).map(item => item.task.id),
      ...(plan?.later ?? []).map(item => item.task.id),
    ]),
    [plan],
  );
  const relationByTaskId = useMemo(
    () => new Map([
      ...(plan?.active ?? []),
      ...(plan?.later ?? []),
    ].map(relation => [relation.task.id, relation])),
    [plan],
  );
  const selectedRelation = selectedTask
    ? relationByTaskId.get(selectedTask.id) ?? null
    : null;
  const filteredAllRelations = useMemo(() => {
    if (!plan) return { active: [], later: [] };
    if (planFilter === 'ready') {
      return {
        active: plan.active.filter(relation => relation.actionable),
        later: [],
      };
    }
    if (planFilter === 'blocked') {
      return {
        active: plan.active.filter(relation => (
          relation.task.status === 'blocked' || relation.active_blockers.length > 0
        )),
        later: [],
      };
    }
    if (planFilter === 'done') {
      return {
        active: plan.active.filter(relation => relation.task.status === 'done'),
        later: [],
      };
    }
    if (planFilter === 'later') {
      return { active: [], later: plan.later };
    }
    return { active: plan.active, later: plan.later };
  }, [plan, planFilter]);

  const togglePlanFilter = (filter: PlanFilter) => {
    setPlanFilter(previous => previous === filter ? null : filter);
    setViewMode('all');
  };

  const createQuickTask = async () => {
    const title = quickTitle.trim();
    if (!title || creating) return;
    setCreating(true);
    try {
      const task = await planning.create({ title, status: 'todo' });
      const next = await planning.linkDiscussion(task.id, {
        discussion_id: discussionId,
        placement: 'active',
        is_primary: !plan?.primary_objective,
      });
      setPlan(next);
      onChanged?.(next);
      setQuickTitle('');
      toast(t('planning.created'), 'success');
    } catch (cause) {
      toast(userError(cause), 'error');
    } finally {
      setCreating(false);
    }
  };

  const toggleDone = async (relation: PlanningDiscussionRelation) => {
    const status: PlanningTaskStatus = relation.task.status === 'done' ? 'todo' : 'done';
    try {
      await planning.update(relation.task.id, { status });
      await refresh();
    } catch (cause) {
      toast(userError(cause), 'error');
    }
  };

  const openDetail = async (taskId: string) => {
    if (selectedTask?.id === taskId) {
      setSelectedTask(null);
      return;
    }
    setDetailLoading(true);
    try {
      setSelectedTask(await planning.get(taskId));
    } catch (cause) {
      toast(userError(cause), 'error');
    } finally {
      setDetailLoading(false);
    }
  };

  const updateRelation = async (
    relation: PlanningDiscussionRelation,
    patch: { placement?: 'active' | 'later'; isPrimary?: boolean },
  ) => {
    try {
      await planning.linkDiscussion(relation.task.id, {
        discussion_id: discussionId,
        placement: patch.placement ?? relation.placement,
        is_primary: patch.isPrimary ?? relation.is_primary,
      });
      await refresh();
    } catch (cause) {
      toast(userError(cause), 'error');
    }
  };

  const toggleDod = async (index: number) => {
    if (!selectedTask || dodSavingIndex !== null) return;
    const item = selectedTask.definition_of_done[index];
    if (!item) return;
    setDodSavingIndex(index);
    try {
      const updated = await planning.updateDod(selectedTask.id, item.id, {
        completed: !item.completed,
      });
      setSelectedTask(updated);
      await refresh();
    } catch (cause) {
      toast(userError(cause), 'error');
    } finally {
      setDodSavingIndex(null);
    }
  };

  const renderTask = (relation: PlanningDiscussionRelation) => {
    const done = relation.task.status === 'done';
    const blocked = relation.task.status === 'blocked' || relation.task.blocker_count > 0;
    const nested = relation.task.parent_id !== null && linkedTaskIds.has(relation.task.parent_id);
    return (
      <article
        className="plan-task"
        data-status={relation.task.status}
        data-depth={nested ? 1 : 0}
        key={relation.task.id}
      >
        <button
          type="button"
          className="plan-task-check"
          onClick={() => void toggleDone(relation)}
          aria-label={done ? t('planning.markTodo') : t('planning.markDone')}
        >
          {done ? <Check size={13} /> : <Circle size={13} />}
        </button>
        <div className="plan-task-main">
          <button
            type="button"
            className="plan-task-open"
            onClick={() => void openDetail(relation.task.id)}
          >
            {relation.task.parent_reference && (
              <span className="plan-task-parent">
                {relation.task.parent_reference}
                {relation.task.parent_title ? ` · ${relation.task.parent_title}` : ''}
              </span>
            )}
            <span className="plan-task-title">{relation.task.title}</span>
          </button>
          <span className="plan-task-meta">
            <CopyIdPill
              id={relation.task.id}
              label={relation.task.reference}
              title={t('planning.copyTaskId', relation.task.reference)}
              className="plan-task-id"
            />
            {relation.task.total_subtasks > 0 && (
              <span>{relation.task.completed_subtasks}/{relation.task.total_subtasks}</span>
            )}
            {blocked && <span className="plan-blocked">{t('planning.blocked')}</span>}
          </span>
        </div>
        <button
          type="button"
          className="plan-task-action"
          data-active={relation.is_primary}
          title={relation.is_primary ? t('planning.removePrimary') : t('planning.makePrimary')}
          onClick={() => void updateRelation(relation, { isPrimary: !relation.is_primary })}
        >
          <Target size={11} />
        </button>
        <button
          type="button"
          className="plan-task-action"
          title={relation.placement === 'active' ? t('planning.moveLater') : t('planning.moveActive')}
          onClick={() => void updateRelation(relation, {
            placement: relation.placement === 'active' ? 'later' : 'active',
          })}
        >
          {relation.placement === 'active' ? <Archive size={11} /> : <Flag size={11} />}
        </button>
        <span className="plan-priority-dot" data-priority={relation.task.priority} />
      </article>
    );
  };

  const renderAllTask = (
    relation: PlanningDiscussionRelation,
    { selected }: { selected: boolean },
  ) => {
    return (
      <div className="plan-all-task-content" data-selected={selected}>
        <span className="plan-all-task-title">{relation.task.title}</span>
        <span className="plan-all-task-meta">
          <span>{relation.task.reference}</span>
          <span data-status={relation.task.status}>
            {t(`planning.status.${relation.task.status}`)}
          </span>
          {relation.active_blockers.length > 0 && (
            <span data-kind="blocked">
              {t('planning.blockerCount', relation.active_blockers.length)}
            </span>
          )}
        </span>
        {relation.task.parent_reference && (
          <span className="plan-all-task-parent">
            {relation.task.parent_reference}
            {relation.task.parent_title ? ` · ${relation.task.parent_title}` : ''}
          </span>
        )}
      </div>
    );
  };

  const renderDependencyNode = (
    dependency: PlanningTaskSummary,
    direction: 'blocker' | 'blocking',
  ) => {
    const internal = relationByTaskId.has(dependency.id);
    const targetDiscussionId = dependency.discussion_ids.find(id => id !== discussionId)
      ?? dependency.discussion_ids[0];
    const targetProjectId = dependency.project_ids[0];
    const canOpenDiscussion = Boolean(targetDiscussionId && onNavigateDiscussion);
    const canOpenProject = Boolean(targetProjectId && onNavigateProject);
    return (
      <div
        className="plan-dependency-node"
        data-kind={internal ? 'internal' : 'external'}
        data-status={dependency.status}
        key={`${direction}-${dependency.id}`}
      >
        <div className="plan-dependency-node-main">
          <span>{dependency.title}</span>
          <small>
            {dependency.reference} · {t(`planning.status.${dependency.status}`)}
          </small>
        </div>
        {internal ? (
          <button
            type="button"
            onClick={() => void openDetail(dependency.id)}
            aria-label={t('planning.selectDependency', dependency.title)}
          >
            <ArrowRight size={12} />
          </button>
        ) : (
          <div className="plan-dependency-targets">
            {targetDiscussionId && onNavigateDiscussion && (
              <button
                type="button"
                onClick={() => onNavigateDiscussion(targetDiscussionId)}
                aria-label={t('planning.openDependencyDiscussion', dependency.title)}
              >
                <MessageCircle size={12} />
              </button>
            )}
            {targetProjectId && onNavigateProject && (
              <button
                type="button"
                onClick={() => onNavigateProject(targetProjectId)}
                aria-label={t('planning.openDependencyProject', dependency.title)}
              >
                <FolderKanban size={12} />
              </button>
            )}
            {!canOpenDiscussion && !canOpenProject && (
              <span>{t('planning.noDependencyTarget')}</span>
            )}
          </div>
        )}
      </div>
    );
  };

  return (
    <aside className="disc-tool-panel plan-panel" aria-label={t('planning.discussionPlan')}>
      <header className="disc-tool-panel-header plan-panel-header">
        <div>
          <div className="disc-tool-panel-title plan-panel-title"><Target size={15} /> <span>{t('planning.discussionPlan')}</span></div>
          {plan && (
            <div className="plan-panel-progress">
              {t('planning.progress', plan.completed_active, plan.total_active)}
              {plan.later.length > 0 ? ` · +${plan.later.length}` : ''}
            </div>
          )}
        </div>
        <button type="button" className="disc-tool-panel-icon plan-panel-close" onClick={onClose} aria-label={t('common.close')}>
          <X size={16} />
        </button>
      </header>

      <div className="plan-quick-create">
        <Plus size={14} />
        <input
          value={quickTitle}
          onChange={event => setQuickTitle(event.target.value)}
          onKeyDown={event => {
            if (event.key === 'Enter') void createQuickTask();
          }}
          placeholder={t('planning.quickCreate')}
          maxLength={240}
        />
        <button
          type="button"
          onClick={() => void createQuickTask()}
          disabled={!quickTitle.trim() || creating}
        >
          {creating ? <Loader2 size={13} className="spin" /> : <Check size={13} />}
        </button>
      </div>

      <div
        className="disc-tool-panel-body plan-panel-body"
        data-detail-open={Boolean(selectedTask || detailLoading)}
      >
        <div className="plan-panel-list" data-view-mode={viewMode}>
          {loading && !plan && <div className="plan-panel-state"><Loader2 size={16} className="spin" /> {t('common.loading')}</div>}
          {error && <div className="plan-panel-error">{error}</div>}
          {proposalInbox && proposalInbox.pending_item_count > 0 && (
            <section className="plan-proposal-inbox">
              <h3>
                <Inbox size={13} />
                <span>{t('planning.proposalInbox')}</span>
                <span>{proposalInbox.pending_item_count}</span>
              </h3>
              {proposalInbox.proposals.map(proposal => (
                <article className="plan-proposal-card" key={proposal.id}>
                  <div className="plan-proposal-card-meta">
                    {t('planning.proposalFromAgent', proposal.items.length)}
                  </div>
                  <PlanningProposalReview
                    proposal={proposal}
                    discussionId={discussionId}
                    onChanged={updateProposal}
                  />
                </article>
              ))}
            </section>
          )}
          {plan && plan.primary_objective && (
            <section className="plan-primary">
              <button type="button" onClick={() => setPrimaryOpen(value => !value)}>
                {primaryOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                <Target size={13} />
                <span>{t('planning.primaryObjective')}</span>
              </button>
              {primaryOpen && (
                <div className="plan-primary-card">
                  <strong>{plan.primary_objective.title}</strong>
                  <CopyIdPill
                    id={plan.primary_objective.id}
                    label={plan.primary_objective.reference}
                    title={t('planning.copyTaskId', plan.primary_objective.reference)}
                  />
                </div>
              )}
            </section>
          )}

          {plan && (plan.active.length > 0 || plan.later.length > 0) && (
            <div className="plan-view-switch" role="group" aria-label={t('planning.viewMode')}>
              <button
                type="button"
                data-active={viewMode === 'focus'}
                onClick={() => setViewMode('focus')}
              >
                <Focus size={12} />
                {t('planning.focusView')}
              </button>
              <button
                type="button"
                data-active={viewMode === 'all'}
                onClick={() => setViewMode('all')}
              >
                <List size={12} />
                {t('planning.allView')}
              </button>
            </div>
          )}

          {plan && (plan.active.length > 0 || plan.later.length > 0) && (
            <div
              className="plan-focus-stats"
              aria-label={t(
                'planning.focusSummary',
                plan.stats.ready,
                plan.stats.blocked,
                plan.stats.done,
              )}
            >
              <button
                type="button"
                data-kind="ready"
                data-active={planFilter === 'ready'}
                aria-pressed={planFilter === 'ready'}
                aria-label={t('planning.filterReady')}
                disabled={plan.stats.ready === 0 && planFilter !== 'ready'}
                onClick={() => togglePlanFilter('ready')}
              >
                {t('planning.focusReady', plan.stats.ready)}
              </button>
              <button
                type="button"
                data-kind="blocked"
                data-active={planFilter === 'blocked'}
                aria-pressed={planFilter === 'blocked'}
                aria-label={t('planning.filterBlocked')}
                disabled={plan.stats.blocked === 0 && planFilter !== 'blocked'}
                onClick={() => togglePlanFilter('blocked')}
              >
                {t('planning.focusBlocked', plan.stats.blocked)}
              </button>
              <button
                type="button"
                data-kind="done"
                data-active={planFilter === 'done'}
                aria-pressed={planFilter === 'done'}
                aria-label={t('planning.filterDone')}
                disabled={plan.stats.done === 0 && planFilter !== 'done'}
                onClick={() => togglePlanFilter('done')}
              >
                {t('planning.focusDone', plan.stats.done)}
              </button>
              {plan.stats.ideas > 0 && (
                <span data-kind="ideas">{t('planning.focusIdeas', plan.stats.ideas)}</span>
              )}
              {plan.stats.later > 0 && (
                <button
                  type="button"
                  data-kind="later"
                  data-active={planFilter === 'later'}
                  aria-pressed={planFilter === 'later'}
                  aria-label={t('planning.filterLater')}
                  onClick={() => togglePlanFilter('later')}
                >
                  {t('planning.focusLater', plan.stats.later)}
                </button>
              )}
            </div>
          )}

          {plan && plan.active.length === 0 && plan.later.length === 0 && (
            <div className="plan-panel-empty">
              <Flag size={24} />
              <strong>{t('planning.emptyTitle')}</strong>
              <span>{t('planning.emptyHint')}</span>
            </div>
          )}

          {viewMode === 'focus' && visibleCurrent.length > 0 && (
            <section className="plan-timeline-section" data-kind="current">
              <h3><Circle size={13} /> {t('planning.current')}</h3>
              {visibleCurrent.map(renderTask)}
              {current.length > visibleCurrent.length && (
                <div className="plan-focus-overflow">
                  {t('planning.moreCurrent', current.length - visibleCurrent.length)}
                </div>
              )}
            </section>
          )}

          {viewMode === 'focus' && visibleUpcoming.length > 0 && (
            <section className="plan-timeline-section" data-kind="upcoming">
              <h3><Flag size={13} /> {t('planning.upcoming')}</h3>
              {visibleUpcoming.map(renderTask)}
              {upcoming.length > visibleUpcoming.length && (
                <div className="plan-focus-overflow">
                  {t('planning.moreReady', upcoming.length - visibleUpcoming.length)}
                </div>
              )}
            </section>
          )}

          {viewMode === 'focus' && plan && plan.active.length > 0
            && visibleCurrent.length === 0
            && visibleUpcoming.length === 0 && (
            <div className="plan-focus-empty">
              {t('planning.noActionable')}
            </div>
          )}

          {viewMode === 'all' && plan && (
            <PlanAllTasksView
              active={filteredAllRelations.active}
              later={filteredAllRelations.later}
              query={allQuery}
              onQueryChange={setAllQuery}
              selectedTaskId={selectedTask?.id ?? null}
              onSelect={taskId => void openDetail(taskId)}
              renderTask={renderAllTask}
              t={t}
            />
          )}
        </div>

        {detailLoading && <div className="plan-panel-state"><Loader2 size={14} className="spin" /></div>}
        {selectedTask && (
          <section className="plan-detail">
            <div className="plan-detail-head">
              <CopyIdPill
                id={selectedTask.id}
                label={selectedTask.reference}
                title={t('planning.copyTaskId', selectedTask.reference)}
              />
              <button type="button" onClick={() => setSelectedTask(null)}><X size={13} /></button>
            </div>
            <h3>{selectedTask.title}</h3>
            {selectedTask.description && <p>{selectedTask.description}</p>}
            {selectedRelation && (
              <div className="plan-detail-actions" aria-label={t('planning.taskActions')}>
                <button
                  type="button"
                  onClick={() => void toggleDone(selectedRelation)}
                >
                  {selectedRelation.task.status === 'done'
                    ? <Circle size={12} />
                    : <Check size={12} />}
                  {selectedRelation.task.status === 'done'
                    ? t('planning.markTodo')
                    : t('planning.markDone')}
                </button>
                <button
                  type="button"
                  data-active={selectedRelation.is_primary}
                  onClick={() => void updateRelation(selectedRelation, {
                    isPrimary: !selectedRelation.is_primary,
                  })}
                >
                  <Target size={12} />
                  {selectedRelation.is_primary
                    ? t('planning.removePrimary')
                    : t('planning.makePrimary')}
                </button>
                <button
                  type="button"
                  onClick={() => void updateRelation(selectedRelation, {
                    placement: selectedRelation.placement === 'active' ? 'later' : 'active',
                  })}
                >
                  {selectedRelation.placement === 'active'
                    ? <Archive size={12} />
                    : <Flag size={12} />}
                  {selectedRelation.placement === 'active'
                    ? t('planning.moveLater')
                    : t('planning.moveActive')}
                </button>
              </div>
            )}
            {(selectedTask.blockers.length > 0 || selectedTask.blocking.length > 0) && (
              <div
                className="plan-dependency-neighborhood"
                aria-label={t('planning.dependencyNeighborhood')}
              >
                {selectedTask.blockers.length > 0 && (
                  <div className="plan-dependency-column" data-direction="blockers">
                    <strong>{t('planning.blockedBy')}</strong>
                    {selectedTask.blockers.map(dependency => (
                      renderDependencyNode(dependency, 'blocker')
                    ))}
                  </div>
                )}
                <div className="plan-dependency-selected">
                  <span>{selectedTask.title}</span>
                  <small>{selectedTask.reference}</small>
                </div>
                {selectedTask.blocking.length > 0 && (
                  <div className="plan-dependency-column" data-direction="blocking">
                    <strong>{t('planning.blocks')}</strong>
                    {selectedTask.blocking.map(dependency => (
                      renderDependencyNode(dependency, 'blocking')
                    ))}
                  </div>
                )}
              </div>
            )}
            {selectedTask.definition_of_done.length > 0 && (
              <div className="plan-dod">
                <strong>
                  {t('planning.definitionOfDone')} ·{' '}
                  {selectedTask.definition_of_done.filter(item => item.completed).length}
                  /{selectedTask.definition_of_done.length}
                </strong>
                {selectedTask.definition_of_done.map((item, index) => (
                  <button
                    type="button"
                    key={item.id}
                    data-done={item.completed}
                    disabled={dodSavingIndex !== null}
                    onClick={() => void toggleDod(index)}
                    aria-label={item.completed
                      ? t('planning.uncheckDod', item.sentence)
                      : t('planning.checkDod', item.sentence)}
                  >
                    {dodSavingIndex === index
                      ? <Loader2 size={11} className="spin" />
                      : item.completed
                        ? <Check size={11} />
                        : <Circle size={11} />}
                    {item.sentence}
                  </button>
                ))}
              </div>
            )}
          </section>
        )}
      </div>
    </aside>
  );
}
