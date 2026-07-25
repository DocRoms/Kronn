import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Check,
  ChevronRight,
  Circle,
  ExternalLink,
  Link2,
  ListTodo,
  Loader2,
  Plus,
} from 'lucide-react';
import { planning } from '../lib/api';
import { useT } from '../lib/I18nContext';
import type { PlanningTaskSummary } from '../types/generated';
import { CopyIdPill } from './CopyIdPill';
import './ProjectTasksPanel.css';

interface ProjectTasksPanelProps {
  projectId: string;
  onOpenPlanning: (taskId?: string) => void;
  onCountChange?: (count: number) => void;
  toast: (message: string, type: 'success' | 'error' | 'warning' | 'info') => void;
}

export function ProjectTasksPanel({
  projectId,
  onOpenPlanning,
  onCountChange,
  toast,
}: ProjectTasksPanelProps) {
  const { t } = useT();
  const [tasks, setTasks] = useState<PlanningTaskSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [quickTitle, setQuickTitle] = useState('');
  const [creating, setCreating] = useState(false);
  const [updatingId, setUpdatingId] = useState<string | null>(null);

  const loadTasks = useCallback(async () => {
    setLoading(true);
    try {
      const response = await planning.list({ projectId, limit: 100 });
      setTasks(response.items);
      onCountChange?.(response.items.length);
    } catch {
      toast(t('projects.tasks.loadError'), 'error');
    } finally {
      setLoading(false);
    }
  }, [onCountChange, projectId, t, toast]);

  useEffect(() => {
    let active = true;
    planning.list({ projectId, limit: 100 })
      .then(response => {
        if (active) {
          setTasks(response.items);
          onCountChange?.(response.items.length);
        }
      })
      .catch(() => {
        if (active) toast(t('projects.tasks.loadError'), 'error');
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => { active = false; };
  }, [onCountChange, projectId, t, toast]);

  const activeTasks = useMemo(
    () => tasks.filter(task => task.status !== 'done' && task.status !== 'archived'),
    [tasks],
  );
  const completedTasks = useMemo(
    () => tasks.filter(task => task.status === 'done'),
    [tasks],
  );

  const createTask = async () => {
    const title = quickTitle.trim();
    if (!title || creating) return;
    setCreating(true);
    try {
      await planning.create({
        title,
        status: 'todo',
        priority: 'normal',
        project_ids: [projectId],
      });
      setQuickTitle('');
      await loadTasks();
    } catch {
      toast(t('projects.tasks.createError'), 'error');
    } finally {
      setCreating(false);
    }
  };

  const toggleTask = async (task: PlanningTaskSummary) => {
    if (updatingId) return;
    setUpdatingId(task.id);
    try {
      await planning.update(task.id, {
        status: task.status === 'done' ? 'todo' : 'done',
      });
      await loadTasks();
    } catch {
      toast(t('projects.tasks.updateError'), 'error');
    } finally {
      setUpdatingId(null);
    }
  };

  const renderTask = (task: PlanningTaskSummary) => (
    <article className="project-task-card" key={task.id} data-status={task.status}>
      <button
        type="button"
        className="project-task-check"
        onClick={() => void toggleTask(task)}
        disabled={updatingId !== null}
        aria-label={task.status === 'done' ? t('planning.markTodo') : t('planning.markDone')}
      >
        {updatingId === task.id
          ? <Loader2 size={14} className="spin" />
          : task.status === 'done'
            ? <Check size={14} />
            : <Circle size={14} />}
      </button>
      <div className="project-task-main">
        <strong>{task.title}</strong>
        <div className="project-task-meta">
          <CopyIdPill
            id={task.id}
            label={task.reference}
            title={t('planning.copyTaskId', task.reference)}
          />
          <span className="planning-status" data-status={task.status}>
            {t(`planning.status.${task.status}`)}
          </span>
          <span className="project-task-priority" data-priority={task.priority}>
            {t(`planning.priority.${task.priority}`)}
          </span>
          {task.total_subtasks > 0 && (
            <span>{task.completed_subtasks}/{task.total_subtasks}</span>
          )}
          {task.discussion_ids.length > 0 && (
            <span><Link2 size={10} /> {task.discussion_ids.length}</span>
          )}
        </div>
      </div>
      <button
        type="button"
        className="dash-icon-btn project-task-open"
        onClick={() => onOpenPlanning(task.id)}
        aria-label={t('projects.tasks.openTask', task.reference)}
        title={t('projects.tasks.openTask', task.reference)}
      >
        <ChevronRight size={12} />
      </button>
    </article>
  );

  return (
    <div className="project-tasks-panel">
      <header className="project-tasks-header">
        <div>
          <h3><ListTodo size={17} /> {t('planning.title')}</h3>
          <p>{t('projects.tasks.subtitle')}</p>
        </div>
        <button type="button" className="btn btn-sm" onClick={() => onOpenPlanning()}>
          <ExternalLink size={13} /> {t('projects.tasks.openPlanning')}
        </button>
      </header>

      <div className="project-tasks-create">
        <input
          className="input"
          value={quickTitle}
          onChange={event => setQuickTitle(event.target.value)}
          onKeyDown={event => {
            if (event.key === 'Enter') void createTask();
          }}
          placeholder={t('planning.quickCreate')}
          aria-label={t('planning.quickCreate')}
        />
        <button
          type="button"
          className="btn btn-primary"
          onClick={() => void createTask()}
          disabled={!quickTitle.trim() || creating}
          aria-label={t('planning.newIdea')}
        >
          {creating ? <Loader2 size={14} className="spin" /> : <Plus size={14} />}
        </button>
      </div>

      {loading ? (
        <div className="project-tasks-state"><Loader2 size={18} className="spin" /></div>
      ) : tasks.length === 0 ? (
        <div className="project-tasks-state">
          <ListTodo size={20} />
          <span>{t('projects.tasks.empty')}</span>
        </div>
      ) : (
        <div className="project-task-groups">
          <section>
            <h4>{t('projects.tasks.active')} <span>{activeTasks.length}</span></h4>
            <div className="project-task-list">{activeTasks.map(renderTask)}</div>
          </section>
          {completedTasks.length > 0 && (
            <details className="project-task-completed">
              <summary>{t('projects.tasks.completed')} <span>{completedTasks.length}</span></summary>
              <div className="project-task-list">{completedTasks.map(renderTask)}</div>
            </details>
          )}
        </div>
      )}
    </div>
  );
}
