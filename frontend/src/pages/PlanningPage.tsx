import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Archive,
  Check,
  ChevronDown,
  ChevronRight,
  Circle,
  Filter,
  GripVertical,
  History,
  Link2,
  Loader2,
  Plus,
  Search,
  Target,
  X,
} from 'lucide-react';
import { planning } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import { CopyIdPill } from '../components/CopyIdPill';
import type { ToastFn } from '../hooks/useToast';
import type {
  Discussion,
  PlanningTaskDetail,
  PlanningTaskPriority,
  PlanningTaskStatus,
  PlanningTaskSummary,
  Project,
} from '../types/generated';
import './PlanningPage.css';

interface Props {
  initialSelectedTaskId?: string | null;
  projects: Project[];
  discussions: Discussion[];
  toast: ToastFn;
  onNavigateDiscussion: (discussionId: string) => void;
}

const PRIORITIES: PlanningTaskPriority[] = ['critical', 'high', 'normal', 'low'];
const ACTIVE_STATUSES: PlanningTaskStatus[] = ['idea', 'todo', 'in_progress', 'blocked'];

function titleTokens(value: string): Set<string> {
  return new Set(
    value.normalize('NFD')
      .replace(/\p{Diacritic}/gu, '')
      .toLowerCase()
      .split(/[^\p{Letter}\p{Number}]+/u)
      .filter(token => token.length > 2),
  );
}

export function PlanningPage({
  initialSelectedTaskId,
  projects,
  discussions,
  toast,
  onNavigateDiscussion,
}: Props) {
  const { t } = useT();
  const [tasks, setTasks] = useState<PlanningTaskSummary[]>([]);
  const [nextCursor, setNextCursor] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [search, setSearch] = useState('');
  const [status, setStatus] = useState<PlanningTaskStatus | ''>('');
  const [priorityFilter, setPriorityFilter] = useState<PlanningTaskPriority | ''>('');
  const [projectId, setProjectId] = useState('');
  const [withDiscussion, setWithDiscussion] = useState<'' | 'yes' | 'no'>('');
  const [tag, setTag] = useState('');
  const [showCompleted, setShowCompleted] = useState(false);
  const [quickTitle, setQuickTitle] = useState('');
  const [quickPriority, setQuickPriority] = useState<PlanningTaskPriority>('normal');
  const [creating, setCreating] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(initialSelectedTaskId ?? null);
  const [detail, setDetail] = useState<PlanningTaskDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(Boolean(initialSelectedTaskId));
  const [saving, setSaving] = useState(false);
  const [draggedId, setDraggedId] = useState<string | null>(null);
  const [filtersOpen, setFiltersOpen] = useState(false);

  const fetchTasks = useCallback(async (cursor?: number) => {
    const append = cursor !== undefined;
    if (append) setLoadingMore(true);
    else setLoading(true);
    try {
      const result = await planning.list({
        search: search.trim() || undefined,
        status: status || undefined,
        priority: priorityFilter || undefined,
        projectId: projectId || undefined,
        tag: tag || undefined,
        withDiscussion: withDiscussion === '' ? undefined : withDiscussion === 'yes',
        cursor,
        limit: 100,
      });
      setTasks(previous => append ? [...previous, ...result.items] : result.items);
      setNextCursor(result.next_cursor);
    } catch (cause) {
      toast(userError(cause), 'error');
    } finally {
      setLoading(false);
      setLoadingMore(false);
    }
  }, [priorityFilter, projectId, search, status, tag, toast, withDiscussion]);

  useEffect(() => {
    const timer = setTimeout(() => { void fetchTasks(); }, 180);
    return () => clearTimeout(timer);
  }, [fetchTasks]);

  useEffect(() => {
    if (!selectedId) return;
    let cancelled = false;
    planning.get(selectedId)
      .then(item => {
        if (!cancelled) setDetail(item);
      })
      .catch(cause => toast(userError(cause), 'error'))
      .finally(() => {
        if (!cancelled) setDetailLoading(false);
      });
    return () => { cancelled = true; };
  }, [selectedId, toast]);

  const tags = useMemo(
    () => [...new Set(tasks.flatMap(task => task.tags))].sort((a, b) => a.localeCompare(b)),
    [tasks],
  );
  const activeTasks = tasks.filter(task => ACTIVE_STATUSES.includes(task.status));
  const completedTasks = tasks.filter(task => task.status === 'done');
  const duplicateCandidates = useMemo(() => {
    const query = titleTokens(quickTitle);
    if (query.size === 0) return [];
    return tasks
      .map(task => {
        const candidate = titleTokens(task.title);
        const overlap = [...query].filter(token => candidate.has(token)).length;
        const union = new Set([...query, ...candidate]).size;
        return { task, score: union > 0 ? overlap / union : 0 };
      })
      .filter(item => item.score >= 0.5)
      .sort((a, b) => b.score - a.score)
      .slice(0, 3)
      .map(item => item.task);
  }, [quickTitle, tasks]);

  const selectTask = (taskId: string) => {
    setDetail(null);
    setDetailLoading(true);
    setSelectedId(taskId);
  };

  const createQuickTask = async () => {
    const title = quickTitle.trim();
    if (!title || creating) return;
    setCreating(true);
    try {
      const created = await planning.create({ title, priority: quickPriority, status: 'idea' });
      setTasks(previous => [...previous, created]);
      setQuickTitle('');
      toast(t('planning.created'), 'success');
    } catch (cause) {
      toast(userError(cause), 'error');
    } finally {
      setCreating(false);
    }
  };

  const updateTask = async (
    taskId: string,
    patch: Parameters<typeof planning.update>[1],
  ) => {
    const updated = await planning.update(taskId, patch);
    setTasks(previous => previous.map(item => item.id === taskId ? updated : item));
    if (selectedId === taskId) setDetail(updated);
    return updated;
  };

  const moveTask = async (
    taskId: string,
    priority: PlanningTaskPriority,
    beforeTaskId?: string,
  ) => {
    const band = tasks
      .filter(task => task.priority === priority && task.id !== taskId)
      .sort((a, b) => a.rank - b.rank);
    const beforeIndex = beforeTaskId ? band.findIndex(task => task.id === beforeTaskId) : band.length;
    const previous = beforeIndex > 0 ? band[beforeIndex - 1] : undefined;
    const next = beforeIndex >= 0 && beforeIndex < band.length ? band[beforeIndex] : undefined;
    let rank = 1024;
    if (previous && next) rank = Math.floor((previous.rank + next.rank) / 2);
    else if (previous) rank = previous.rank + 1024;
    else if (next) rank = next.rank - 1024;
    try {
      await updateTask(taskId, { priority, rank });
      // The backend atomically rebalances the whole priority band so ranks
      // never collapse after repeated midpoint inserts. Reload the compact
      // list to keep every sibling's local rank aligned with that rebalance.
      await fetchTasks();
    } catch (cause) {
      toast(userError(cause), 'error');
      await fetchTasks();
    }
  };

  const renderCard = (task: PlanningTaskSummary) => (
    <article
      key={task.id}
      className="planning-card"
      data-selected={selectedId === task.id}
      data-status={task.status}
      draggable
      onDragStart={() => setDraggedId(task.id)}
      onDragEnd={() => setDraggedId(null)}
      onDragOver={event => event.preventDefault()}
      onDrop={event => {
        event.preventDefault();
        if (draggedId && draggedId !== task.id) {
          void moveTask(draggedId, task.priority, task.id);
        }
      }}
      onClick={() => selectTask(task.id)}
    >
      <GripVertical size={13} className="planning-grip" />
      <button
        type="button"
        className="planning-check"
        onClick={event => {
          event.stopPropagation();
          void updateTask(task.id, { status: task.status === 'done' ? 'todo' : 'done' });
        }}
        aria-label={task.status === 'done' ? t('planning.markTodo') : t('planning.markDone')}
      >
        {task.status === 'done' ? <Check size={13} /> : <Circle size={13} />}
      </button>
      <div className="planning-card-main">
        {task.parent_reference && (
          <div className="planning-card-parent">
            {task.parent_reference} · {task.parent_title}
          </div>
        )}
        <div className="planning-card-title">{task.title}</div>
        <div className="planning-card-meta">
          <CopyIdPill
            id={task.id}
            label={task.reference}
            title={t('planning.copyTaskId', task.reference)}
          />
          <span className="planning-status" data-status={task.status}>
            {t(`planning.status.${task.status}`)}
          </span>
          {task.total_subtasks > 0 && (
            <span>{task.completed_subtasks}/{task.total_subtasks}</span>
          )}
          {task.project_ids.map(id => (
            <span key={id}>{projects.find(project => project.id === id)?.name ?? id.slice(0, 8)}</span>
          ))}
          {task.discussion_ids.length > 0 && (
            <span><Link2 size={10} /> {task.discussion_ids.length}</span>
          )}
          {task.tags.map(value => <span className="planning-tag" key={value}>{value}</span>)}
        </div>
      </div>
      <ChevronRight size={14} className="planning-card-chevron" />
    </article>
  );

  return (
    <div className="planning-page">
      <header className="planning-header">
        <div>
          <h1><Target size={20} /> {t('planning.title')}</h1>
          <p>{t('planning.subtitle')}</p>
        </div>
        <div className="planning-summary">
          <span><strong>{activeTasks.length}</strong>{t('planning.activeCount')}</span>
          <span><strong>{completedTasks.length}</strong>{t('planning.doneCount')}</span>
        </div>
      </header>

      <div className="planning-toolbar">
        <label className="planning-search">
          <Search size={14} />
          <input
            value={search}
            onChange={event => setSearch(event.target.value)}
            placeholder={t('planning.search')}
          />
        </label>
        <button
          type="button"
          className="btn btn-sm"
          data-active={filtersOpen}
          onClick={() => setFiltersOpen(value => !value)}
        >
          <Filter size={14} /> {t('planning.filters')}
        </button>
        <div className="planning-create">
          <input
            value={quickTitle}
            onChange={event => setQuickTitle(event.target.value)}
            onKeyDown={event => {
              if (event.key === 'Enter') void createQuickTask();
            }}
            placeholder={t('planning.newIdea')}
            maxLength={240}
          />
          <select
            value={quickPriority}
            onChange={event => setQuickPriority(event.target.value as PlanningTaskPriority)}
          >
            {PRIORITIES.map(value => (
              <option key={value} value={value}>{t(`planning.priority.${value}`)}</option>
            ))}
          </select>
          <button type="button" onClick={() => void createQuickTask()} disabled={!quickTitle.trim() || creating}>
            {creating ? <Loader2 size={14} className="spin" /> : <Plus size={14} />}
          </button>
        </div>
      </div>
      {quickTitle.trim() && duplicateCandidates.length > 0 && (
        <div className="planning-duplicates">
          <span>{t('planning.possibleDuplicates')}</span>
          {duplicateCandidates.map(task => (
            <button type="button" key={task.id} onClick={() => selectTask(task.id)}>
              {task.reference} · {task.title}
            </button>
          ))}
        </div>
      )}

      {filtersOpen && (
        <div className="planning-filters">
          <select value={status} onChange={event => setStatus(event.target.value as PlanningTaskStatus | '')}>
            <option value="">{t('planning.allStatuses')}</option>
            {(['idea', 'todo', 'in_progress', 'blocked', 'done', 'archived'] as PlanningTaskStatus[])
              .map(value => <option key={value} value={value}>{t(`planning.status.${value}`)}</option>)}
          </select>
          <select value={projectId} onChange={event => setProjectId(event.target.value)}>
            <option value="">{t('planning.allProjects')}</option>
            {projects.map(project => <option key={project.id} value={project.id}>{project.name}</option>)}
          </select>
          <select
            value={priorityFilter}
            onChange={event => setPriorityFilter(event.target.value as PlanningTaskPriority | '')}
          >
            <option value="">{t('planning.allPriorities')}</option>
            {PRIORITIES.map(value => (
              <option key={value} value={value}>{t(`planning.priority.${value}`)}</option>
            ))}
          </select>
          <select value={withDiscussion} onChange={event => setWithDiscussion(event.target.value as typeof withDiscussion)}>
            <option value="">{t('planning.allLinks')}</option>
            <option value="yes">{t('planning.withDiscussion')}</option>
            <option value="no">{t('planning.withoutDiscussion')}</option>
          </select>
          {tags.length > 0 && (
            <select value={tag} onChange={event => setTag(event.target.value)}>
              <option value="">{t('planning.allTags')}</option>
              {tags.map(value => <option key={value} value={value}>{value}</option>)}
            </select>
          )}
        </div>
      )}

      <div className="planning-workspace">
        <main className="planning-backlog">
          {loading && <div className="planning-state"><Loader2 size={17} className="spin" /> {t('common.loading')}</div>}
          {!loading && activeTasks.length === 0 && (
            <div className="planning-state"><Target size={24} /> {t('planning.emptyBacklog')}</div>
          )}
          {!loading && PRIORITIES.map(priority => {
            const items = activeTasks
              .filter(task => task.priority === priority)
              .sort((a, b) => a.rank - b.rank);
            return (
              <section
                className="planning-band"
                data-priority={priority}
                key={priority}
                onDragOver={event => event.preventDefault()}
                onDrop={event => {
                  event.preventDefault();
                  if (draggedId) void moveTask(draggedId, priority);
                }}
              >
                <header>
                  <span className="planning-priority-mark" />
                  <h2>{t(`planning.priority.${priority}`)}</h2>
                  <span>{items.length}</span>
                </header>
                <div className="planning-band-list">
                  {items.map(renderCard)}
                  {items.length === 0 && <div className="planning-band-empty">{t('planning.dropHere')}</div>}
                </div>
              </section>
            );
          })}

          {completedTasks.length > 0 && (
            <section className="planning-completed">
              <button type="button" onClick={() => setShowCompleted(value => !value)}>
                {showCompleted ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                <Check size={14} />
                {t('planning.completed')} · {completedTasks.length}
              </button>
              {showCompleted && <div>{completedTasks.map(renderCard)}</div>}
            </section>
          )}
          {nextCursor !== null && (
            <button
              type="button"
              className="btn planning-load-more"
              onClick={() => void fetchTasks(nextCursor)}
              disabled={loadingMore}
            >
              {loadingMore ? <Loader2 size={14} className="spin" /> : t('planning.loadMore')}
            </button>
          )}
        </main>

        {(selectedId || detailLoading) && (
          <aside className="planning-detail">
            <header>
              {detail && (
                <CopyIdPill
                  id={detail.id}
                  label={detail.reference}
                  title={t('planning.copyTaskId', detail.reference)}
                />
              )}
              <button type="button" onClick={() => {
                setSelectedId(null);
                setDetail(null);
                setDetailLoading(false);
              }}><X size={16} /></button>
            </header>
            {detailLoading && <div className="planning-state"><Loader2 size={16} className="spin" /></div>}
            {detail && (
              <PlanningDetailForm
                key={detail.id}
                task={detail}
                projects={projects}
                discussions={discussions}
                saving={saving}
                onNavigateDiscussion={onNavigateDiscussion}
                onOpenTask={selectTask}
                onToggleDod={async (dodId, completed) => {
                  try {
                    const refreshed = await planning.updateDod(detail.id, dodId, { completed });
                    setDetail(refreshed);
                    setTasks(previous => previous.map(item => item.id === refreshed.id ? refreshed : item));
                  } catch (cause) {
                    toast(userError(cause), 'error');
                    throw cause;
                  }
                }}
                onAddBlocker={async blockerTaskId => {
                  try {
                    const refreshed = await planning.addBlocker(detail.id, {
                      blocker_task_id: blockerTaskId,
                    });
                    setDetail(refreshed);
                    setTasks(previous => previous.map(item => item.id === refreshed.id ? refreshed : item));
                    toast(t('planning.blockerAdded'), 'success');
                  } catch (cause) {
                    toast(userError(cause), 'error');
                    throw cause;
                  }
                }}
                onLinkDiscussion={async discussionId => {
                  try {
                    await planning.linkDiscussion(detail.id, {
                      discussion_id: discussionId,
                      placement: 'active',
                      is_primary: false,
                    });
                    const refreshed = await planning.get(detail.id);
                    setDetail(refreshed);
                    setTasks(previous => previous.map(item => item.id === refreshed.id ? refreshed : item));
                    toast(t('planning.discussionLinked'), 'success');
                  } catch (cause) {
                    toast(userError(cause), 'error');
                    throw cause;
                  }
                }}
                onCreateSubtask={async title => {
                  try {
                    const created = await planning.create({
                      title,
                      status: 'todo',
                      priority: detail.priority,
                      parent_id: detail.id,
                      project_ids: detail.project_ids,
                    });
                    const refreshed = await planning.get(detail.id);
                    setDetail(refreshed);
                    setTasks(previous => [
                      ...previous.map(item => item.id === refreshed.id ? refreshed : item),
                      created,
                    ]);
                    toast(t('planning.subtaskCreated'), 'success');
                  } catch (cause) {
                    toast(userError(cause), 'error');
                    throw cause;
                  }
                }}
                onSave={async patch => {
                  setSaving(true);
                  try {
                    await updateTask(detail.id, patch);
                    toast(t('planning.saved'), 'success');
                  } catch (cause) {
                    toast(userError(cause), 'error');
                  } finally {
                    setSaving(false);
                  }
                }}
              />
            )}
          </aside>
        )}
      </div>
    </div>
  );
}

interface DetailProps {
  task: PlanningTaskDetail;
  projects: Project[];
  discussions: Discussion[];
  saving: boolean;
  onSave: (patch: Parameters<typeof planning.update>[1]) => Promise<void>;
  onToggleDod: (dodId: string, completed: boolean) => Promise<void>;
  onCreateSubtask: (title: string) => Promise<void>;
  onAddBlocker: (blockerTaskId: string) => Promise<void>;
  onLinkDiscussion: (discussionId: string) => Promise<void>;
  onOpenTask: (taskId: string) => void;
  onNavigateDiscussion: (discussionId: string) => void;
}

function PlanningDetailForm({
  task,
  projects,
  discussions,
  saving,
  onSave,
  onToggleDod,
  onCreateSubtask,
  onAddBlocker,
  onLinkDiscussion,
  onOpenTask,
  onNavigateDiscussion,
}: DetailProps) {
  const { t } = useT();
  const [title, setTitle] = useState(task.title);
  const [description, setDescription] = useState(task.description);
  const [status, setStatus] = useState(task.status);
  const [priority, setPriority] = useState(task.priority);
  const [blockedReason, setBlockedReason] = useState(task.blocked_reason ?? '');
  const [tags, setTags] = useState(task.tags.join(', '));
  const [projectIds, setProjectIds] = useState(task.project_ids);
  const [definitionOfDone, setDefinitionOfDone] = useState(
    task.definition_of_done.map(item => ({
      id: item.id,
      sentence: item.sentence,
      completed: item.completed,
    })),
  );
  const [links, setLinks] = useState(
    task.links.map(link => ({ label: link.label, url: link.url })),
  );
  const [subtaskTitle, setSubtaskTitle] = useState('');
  const [creatingSubtask, setCreatingSubtask] = useState(false);
  const [blockerReference, setBlockerReference] = useState('');
  const [linking, setLinking] = useState(false);

  return (
    <div className="planning-detail-form">
      {task.parent_id && (
        <button
          type="button"
          className="planning-parent-link"
          onClick={() => onOpenTask(task.parent_id as string)}
        >
          <ChevronRight size={12} />
          {t('planning.parentTask')} · {task.parent_reference} · {task.parent_title}
        </button>
      )}
      <input className="planning-detail-title" value={title} onChange={event => setTitle(event.target.value)} />
      <div className="planning-detail-row">
        <select value={status} onChange={event => setStatus(event.target.value as PlanningTaskStatus)}>
          {(['idea', 'todo', 'in_progress', 'blocked', 'done', 'archived'] as PlanningTaskStatus[])
            .map(value => <option value={value} key={value}>{t(`planning.status.${value}`)}</option>)}
        </select>
        <select value={priority} onChange={event => setPriority(event.target.value as PlanningTaskPriority)}>
          {PRIORITIES.map(value => <option value={value} key={value}>{t(`planning.priority.${value}`)}</option>)}
        </select>
      </div>
      {task.total_subtasks > 0
        && task.completed_subtasks === task.total_subtasks
        && status !== 'done'
        && (
          <div className="planning-suggestion">
            <Check size={14} />
            <span>{t('planning.allSubtasksDone')}</span>
            <button type="button" onClick={() => {
              setStatus('done');
              void onSave({ status: 'done' });
            }}>
              {t('planning.completeParent')}
            </button>
          </div>
        )}
      {status === 'blocked'
        && task.blockers.length > 0
        && task.blockers.every(blocker =>
          blocker.status === 'done' || blocker.status === 'archived'
        )
        && (
          <div className="planning-suggestion">
            <Check size={14} />
            <span>{t('planning.allBlockersDone')}</span>
            <button type="button" onClick={() => {
              setStatus('todo');
              setBlockedReason('');
              void onSave({ status: 'todo', blocked_reason: null });
            }}>
              {t('planning.unblock')}
            </button>
          </div>
        )}
      <label>
        <span>{t('planning.description')}</span>
        <textarea rows={7} value={description} onChange={event => setDescription(event.target.value)} />
      </label>
      {(status === 'blocked' || blockedReason) && (
        <label>
          <span>{t('planning.blockedReason')}</span>
          <textarea rows={2} value={blockedReason} onChange={event => setBlockedReason(event.target.value)} />
        </label>
      )}
      <label>
        <span>{t('planning.tags')}</span>
        <input value={tags} onChange={event => setTags(event.target.value)} />
      </label>

      <section className="planning-detail-dod">
        <h3>{t('planning.definitionOfDone')}</h3>
        {definitionOfDone.map((item, index) => (
          <div className="planning-edit-row" key={index}>
            <button
              type="button"
              onClick={() => {
                const completed = !item.completed;
                const definition_of_done = definitionOfDone.map((candidate, candidateIndex) => ({
                  ...candidate,
                  completed: candidateIndex === index ? completed : candidate.completed,
                }));
                setDefinitionOfDone(definition_of_done);
                if (item.id) {
                  void onToggleDod(item.id, completed);
                } else {
                  void onSave({ definition_of_done });
                }
              }}
            >
              {item.completed ? <Check size={13} /> : <Circle size={13} />}
            </button>
            <input
              value={item.sentence}
              onChange={event => setDefinitionOfDone(items => items.map((candidate, candidateIndex) =>
                candidateIndex === index ? { ...candidate, sentence: event.target.value } : candidate
              ))}
              data-done={item.completed}
            />
            <button
              type="button"
              aria-label={t('common.delete')}
              onClick={() => setDefinitionOfDone(items => items.filter((_, candidateIndex) => candidateIndex !== index))}
            >
              <X size={12} />
            </button>
          </div>
        ))}
        <button
          type="button"
          className="planning-add-row"
          onClick={() => setDefinitionOfDone(items => [
            ...items,
            { id: '', sentence: '', completed: false },
          ])}
        >
          <Plus size={12} /> {t('planning.addDod')}
        </button>
      </section>

      <section className="planning-detail-links">
        <h3>{t('planning.resourceLinks')}</h3>
        {links.map((link, index) => (
          <div className="planning-edit-row planning-link-row" key={index}>
            <input
              value={link.label}
              placeholder={t('planning.linkLabel')}
              onChange={event => setLinks(items => items.map((candidate, candidateIndex) =>
                candidateIndex === index ? { ...candidate, label: event.target.value } : candidate
              ))}
            />
            <input
              value={link.url}
              placeholder="https://…"
              onChange={event => setLinks(items => items.map((candidate, candidateIndex) =>
                candidateIndex === index ? { ...candidate, url: event.target.value } : candidate
              ))}
            />
            <button
              type="button"
              aria-label={t('common.delete')}
              onClick={() => setLinks(items => items.filter((_, candidateIndex) => candidateIndex !== index))}
            >
              <X size={12} />
            </button>
          </div>
        ))}
        <button
          type="button"
          className="planning-add-row"
          onClick={() => setLinks(items => [...items, { label: '', url: '' }])}
        >
          <Plus size={12} /> {t('planning.addLink')}
        </button>
      </section>

      <section className="planning-detail-projects">
        <h3>{t('planning.linkedProjects')}</h3>
        <div className="planning-chip-list">
          {projectIds.map(id => (
            <button type="button" key={id} onClick={() => setProjectIds(ids => ids.filter(value => value !== id))}>
              {projects.find(project => project.id === id)?.name ?? id}
              <X size={10} />
            </button>
          ))}
        </div>
        <select
          value=""
          onChange={event => {
            if (event.target.value) setProjectIds(ids => [...new Set([...ids, event.target.value])]);
          }}
        >
          <option value="">{t('planning.addProject')}</option>
          {projects.filter(project => !projectIds.includes(project.id)).map(project => (
            <option key={project.id} value={project.id}>{project.name}</option>
          ))}
        </select>
      </section>

      <section className="planning-detail-subtasks">
        <h3>{t('planning.subtasks')} · {task.completed_subtasks}/{task.total_subtasks}</h3>
        {task.subtasks.map(subtask => (
          <button type="button" key={subtask.id} onClick={() => onOpenTask(subtask.id)}>
            {subtask.status === 'done' ? <Check size={13} /> : <Circle size={13} />}
            <span data-done={subtask.status === 'done'}>{subtask.title}</span>
            <small>{subtask.reference}</small>
            <ChevronRight size={12} />
          </button>
        ))}
        <div className="planning-subtask-create">
          <Plus size={13} />
          <input
            value={subtaskTitle}
            onChange={event => setSubtaskTitle(event.target.value)}
            onKeyDown={event => {
              if (event.key !== 'Enter' || !subtaskTitle.trim() || creatingSubtask) return;
              setCreatingSubtask(true);
              void onCreateSubtask(subtaskTitle.trim())
                .then(() => setSubtaskTitle(''))
                .catch(() => undefined)
                .finally(() => setCreatingSubtask(false));
            }}
            placeholder={t('planning.addSubtask')}
          />
          {creatingSubtask && <Loader2 size={13} className="spin" />}
        </div>
      </section>

      {task.discussion_ids.length > 0 && (
        <section className="planning-detail-links">
          <h3>{t('planning.linkedDiscussions')}</h3>
          {task.discussion_ids.map(id => (
            <button type="button" key={id} onClick={() => onNavigateDiscussion(id)}>
              <Link2 size={12} />
              {discussions.find(discussion => discussion.id === id)?.title ?? id}
              <ChevronRight size={12} />
            </button>
          ))}
        </section>
      )}
      <select
        value=""
        disabled={linking}
        onChange={event => {
          const discussionId = event.target.value;
          if (!discussionId) return;
          setLinking(true);
          void onLinkDiscussion(discussionId)
            .catch(() => undefined)
            .finally(() => setLinking(false));
        }}
      >
        <option value="">{t('planning.linkDiscussion')}</option>
        {discussions.filter(discussion => !task.discussion_ids.includes(discussion.id)).map(discussion => (
          <option key={discussion.id} value={discussion.id}>{discussion.title}</option>
        ))}
      </select>

      <section className="planning-detail-blockers">
        <h3>{t('planning.blockers')}</h3>
        {task.blockers.map(blocker => (
          <button type="button" key={blocker.id} onClick={() => onOpenTask(blocker.id)}>
            <Circle size={12} /> {blocker.reference} · {blocker.title}
            <ChevronRight size={12} />
          </button>
        ))}
        <div className="planning-blocker-create">
          <input
            value={blockerReference}
            onChange={event => setBlockerReference(event.target.value)}
            placeholder="KT-…"
          />
          <button
            type="button"
            disabled={!blockerReference.trim() || linking}
            onClick={() => {
              setLinking(true);
              void onAddBlocker(blockerReference.trim())
                .then(() => setBlockerReference(''))
                .catch(() => undefined)
                .finally(() => setLinking(false));
            }}
          >
            <Plus size={12} />
          </button>
        </div>
      </section>

      {task.events.length > 0 && (
        <section className="planning-detail-events">
          <h3><History size={12} /> {t('planning.activity')}</h3>
          {task.events.slice(0, 8).map(event => (
            <div key={event.id}>
              <span>{event.action}</span>
              <small>
                {event.actor_kind === 'agent'
                  ? event.actor_id ?? t('planning.agentActor')
                  : t('planning.humanActor')}
                {' · '}
                {new Date(event.created_at).toLocaleString()}
              </small>
            </div>
          ))}
        </section>
      )}

      <button
        type="button"
        className="btn btn-primary planning-save"
        disabled={saving || !title.trim()}
        onClick={() => void onSave({
          title: title.trim(),
          description,
          status,
          priority,
          blocked_reason: blockedReason.trim() || null,
          tags: tags.split(',').map(value => value.trim()).filter(Boolean),
          project_ids: projectIds,
          definition_of_done: definitionOfDone.filter(item => item.sentence.trim()).map(item => ({
            ...item,
            sentence: item.sentence.trim(),
          })),
          links: links.filter(link => link.label.trim() && link.url.trim()).map(link => ({
            label: link.label.trim(),
            url: link.url.trim(),
          })),
        })}
      >
        {saving ? <Loader2 size={14} className="spin" /> : <Check size={14} />}
        {t('common.save')}
      </button>
      <button
        type="button"
        className="planning-archive"
        onClick={() => void onSave({ status: 'archived' })}
      >
        <Archive size={13} /> {t('planning.archive')}
      </button>
    </div>
  );
}
