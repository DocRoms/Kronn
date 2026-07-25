import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Archive,
  Check,
  ChevronDown,
  ChevronRight,
  Circle,
  Flag,
  Loader2,
  Plus,
  Target,
  X,
} from 'lucide-react';
import { planning } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import { CopyIdPill } from './CopyIdPill';
import type {
  DiscussionPlan,
  PlanningDiscussionRelation,
  PlanningTaskDetail,
  PlanningTaskStatus,
} from '../types/generated';
import type { ToastFn } from '../hooks/useToast';
import './DiscussionToolPanel.css';
import './DiscussionPlanPanel.css';

interface Props {
  discussionId: string;
  onClose: () => void;
  onChanged?: (plan: DiscussionPlan) => void;
  toast: ToastFn;
}

export function DiscussionPlanPanel({ discussionId, onClose, onChanged, toast }: Props) {
  const { t } = useT();
  const [plan, setPlan] = useState<DiscussionPlan | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [quickTitle, setQuickTitle] = useState('');
  const [creating, setCreating] = useState(false);
  const [showCompleted, setShowCompleted] = useState(false);
  const [primaryOpen, setPrimaryOpen] = useState(true);
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
      const [next, nextDetail] = await Promise.all([
        planning.discussionPlan(discussionId),
        selectedId ? planning.get(selectedId) : Promise.resolve(null),
      ]);
      setPlan(next);
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

  useEffect(() => {
    void refresh();
    const interval = window.setInterval(() => {
      if (!document.hidden) void refresh(true);
    }, 5_000);
    return () => window.clearInterval(interval);
  }, [refresh]);

  const completed = useMemo(
    () => plan?.active.filter(item => item.task.status === 'done') ?? [],
    [plan],
  );
  const current = useMemo(
    () => plan?.active.filter(item => item.task.status === 'in_progress') ?? [],
    [plan],
  );
  const upcoming = useMemo(
    () => plan?.active.filter(item => item.task.status !== 'done' && item.task.status !== 'in_progress') ?? [],
    [plan],
  );

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
      if (selectedTask?.id === relation.task.id) {
        setSelectedTask(await planning.get(relation.task.id));
      }
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
    return (
      <article
        className="plan-task"
        data-status={relation.task.status}
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
        <div
          className="plan-task-main"
          role="button"
          tabIndex={0}
          onClick={() => void openDetail(relation.task.id)}
          onKeyDown={event => {
            if (event.key === 'Enter' || event.key === ' ') {
              event.preventDefault();
              void openDetail(relation.task.id);
            }
          }}
        >
          <span className="plan-task-title">{relation.task.title}</span>
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

      <div className="disc-tool-panel-body plan-panel-body">
        {loading && !plan && <div className="plan-panel-state"><Loader2 size={16} className="spin" /> {t('common.loading')}</div>}
        {error && <div className="plan-panel-error">{error}</div>}
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

        {plan && plan.active.length === 0 && plan.later.length === 0 && (
          <div className="plan-panel-empty">
            <Flag size={24} />
            <strong>{t('planning.emptyTitle')}</strong>
            <span>{t('planning.emptyHint')}</span>
          </div>
        )}

        {completed.length > 0 && (
          <section className="plan-timeline-section" data-kind="done">
            <h3><Check size={13} /> {t('planning.completed')}</h3>
            {showCompleted
              ? completed.map(renderTask)
              : renderTask(completed[completed.length - 1])}
            {completed.length > 1 && (
              <button
                type="button"
                className="plan-see-completed"
                onClick={() => setShowCompleted(value => !value)}
              >
                {showCompleted
                  ? t('planning.collapseCompleted')
                  : t('planning.seeCompleted', completed.length - 1)}
              </button>
            )}
          </section>
        )}

        {current.length > 0 && (
          <section className="plan-timeline-section" data-kind="current">
            <h3><Circle size={13} /> {t('planning.current')}</h3>
            {current.map(renderTask)}
          </section>
        )}

        {upcoming.length > 0 && (
          <section className="plan-timeline-section" data-kind="upcoming">
            <h3><Flag size={13} /> {t('planning.upcoming')}</h3>
            {upcoming.map(renderTask)}
          </section>
        )}

        {plan && plan.later.length > 0 && (
          <section className="plan-timeline-section plan-later">
            <h3><Archive size={13} /> {t('planning.later')}</h3>
            {plan.later.map(renderTask)}
          </section>
        )}

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
