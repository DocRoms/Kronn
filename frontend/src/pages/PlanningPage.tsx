import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Archive,
  Check,
  ChevronRight,
  Circle,
  Filter,
  GitBranch,
  GripVertical,
  History,
  Link2,
  Loader2,
  Plus,
  Target,
  X,
} from 'lucide-react';
import { planning } from '../lib/api';
import { queueDiscussionWorkspaceTarget } from '../lib/discussion-navigation';
import { useT } from '../lib/I18nContext';
import { useIsMobile } from '../hooks/useMediaQuery';
import { userError } from '../lib/userError';
import { CopyIdPill } from '../components/CopyIdPill';
import { ContextHelp } from '../components/ContextHelp';
import { CollectionShell } from '../components/CollectionShell';
import { usePersistentIdSet } from '../hooks/usePersistentIdSet';
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
  const isMobile = useIsMobile();
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [tasks, setTasks] = useState<PlanningTaskSummary[]>([]);
  const [search, setSearch] = useState('');
  const [status, setStatus] = useState<PlanningTaskStatus | ''>('');
  const [priorityFilter, setPriorityFilter] = useState<PlanningTaskPriority | ''>('');
  const [projectId, setProjectId] = useState('');
  const [withDiscussion, setWithDiscussion] = useState<'' | 'yes' | 'no'>('');
  const [tag, setTag] = useState('');
  const [quickTitle, setQuickTitle] = useState('');
  const [quickPriority, setQuickPriority] = useState<PlanningTaskPriority>('normal');
  const [createModalOpen, setCreateModalOpen] = useState(false);
  const [creating, setCreating] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(initialSelectedTaskId ?? null);
  const [detail, setDetail] = useState<PlanningTaskDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(Boolean(initialSelectedTaskId));
  const [saving, setSaving] = useState(false);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [tasksLoaded, setTasksLoaded] = useState(false);
  const [favoritesOnly, setFavoritesOnly] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const createTriggerRef = useRef<HTMLButtonElement>(null);
  const createTitleRef = useRef<HTMLInputElement>(null);
  const createModalRef = useRef<HTMLElement>(null);
  const createWasOpenRef = useRef(false);
  const creatingRef = useRef(false);
  const taskIds = useMemo(() => tasks.map(task => task.id), [tasks]);
  const { ids: favoriteIds, toggle: toggleFavorite } = usePersistentIdSet('kronn:collection-favorites:planning', taskIds, tasksLoaded);
  const activeFilterCount = [status, priorityFilter, projectId, withDiscussion, tag]
    .filter(Boolean).length;

  const fetchTasks = useCallback(async (cursor?: number) => {
    const append = cursor !== undefined;
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
      if (!append) setTasksLoaded(true);
    } catch (cause) {
      toast(userError(cause), 'error');
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
      .catch(cause => {
        if (cancelled) return;
        setSelectedId(null);
        setDetail(null);
        setDetailLoading(false);
        toast(userError(cause), 'error');
      })
      .finally(() => {
        if (!cancelled) setDetailLoading(false);
      });
    return () => { cancelled = true; };
  }, [selectedId, toast]);

  useEffect(() => {
    if (createModalOpen) {
      createWasOpenRef.current = true;
      createTitleRef.current?.focus();
      return;
    }
    if (createWasOpenRef.current) {
      createWasOpenRef.current = false;
      createTriggerRef.current?.focus();
    }
  }, [createModalOpen]);

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
    if (!title || creatingRef.current) return;
    creatingRef.current = true;
    setCreating(true);
    try {
      const created = await planning.create({ title, priority: quickPriority, status: 'idea' });
      setTasks(previous => [...previous, created]);
      setQuickTitle('');
      setCreateModalOpen(false);
      toast(t('planning.created'), 'success');
    } catch (cause) {
      toast(userError(cause), 'error');
    } finally {
      creatingRef.current = false;
      setCreating(false);
    }
  };

  const closeCreateModal = () => {
    if (!creatingRef.current) setCreateModalOpen(false);
  };

  const handleCreateModalKeyDown = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      closeCreateModal();
      return;
    }
    if (event.key !== 'Tab') return;
    const focusable = [...(createModalRef.current?.querySelectorAll<HTMLElement>(
      'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
    ) ?? [])];
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
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

  const archiveSelectedTasks = async (selected: PlanningTaskSummary[]) => {
    if (selected.length === 0 || !confirm(t('collection.archiveConfirm', selected.length))) return;
    try {
      const archived = await Promise.all(selected.map(task => planning.update(task.id, { status: 'archived' })));
      const archivedIds = new Set(archived.map(task => task.id));
      setTasks(previous => previous.map(task => archived.find(item => item.id === task.id) ?? task));
      if (selectedId && archivedIds.has(selectedId)) {
        setSelectedId(null);
        setDetail(null);
      }
      toast(t('collection.archiveSuccess', selected.length), 'success');
    } catch (cause) {
      toast(t('collection.deleteError', userError(cause)), 'error');
      throw cause;
    }
  };

  const renderCard = (task: PlanningTaskSummary, selected: boolean) => (
    <div
      className="planning-card"
      data-selected={selected}
      data-status={task.status}
    >
      <GripVertical size={13} className="planning-grip" />
      <span className="planning-check" aria-hidden="true">
        {task.status === 'done' ? <Check size={13} /> : <Circle size={13} />}
      </span>
      <div className="planning-card-main">
        {task.parent_reference && (
          <div className="planning-card-parent">
            {task.parent_reference} · {task.parent_title}
          </div>
        )}
        <div className="planning-card-title">{task.title}</div>
        <div className="planning-card-meta">
          <span>{task.reference}</span>
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
    </div>
  );

  return (
    <div className="planning-page">
      <div className="planning-shell">
        <CollectionShell<PlanningTaskSummary>
          ariaLabel={t('planning.title')}
          title={<><Target size={17} /> {t('planning.title')}</>}
          titleCount={tasks.length}
          headerActions={<>
            <button
              ref={createTriggerRef}
              type="button"
              className="collection-shell-icon collection-shell-primary-action"
              onClick={() => setCreateModalOpen(true)}
              aria-label={t('planning.quickCreate')}
              title={t('planning.quickCreate')}
            >
              <Plus size={16} />
            </button>
            <ContextHelp title={t('contextHelp.planning.title')}>
              <p>{t('contextHelp.planning.intro')}</p>
              <ul>
                <li>{t('contextHelp.planning.global')}</li>
                <li>{t('contextHelp.planning.discussions')}</li>
                <li>{t('contextHelp.planning.priority')}</li>
                <li>{t('contextHelp.planning.orchestration')}</li>
              </ul>
              <p className="kr-context-help-agent-note">{t('contextHelp.planning.mcp')}</p>
            </ContextHelp>
          </>}
          items={tasks}
          getId={task => task.id}
          getLabel={task => `${task.title} ${task.reference} ${task.tags.join(' ')}`}
          isFavorite={task => favoriteIds.has(task.id)}
          onToggleFavorite={task => toggleFavorite(task.id)}
          persistence={{
            query: search,
            onQueryChange: setSearch,
            favoritesOnly,
            onFavoritesOnlyChange: setFavoritesOnly,
          }}
          selectedId={selectedId}
          onSelect={id => selectTask(id)}
          selectedIds={selectedIds}
          onSelectedIdsChange={setSelectedIds}
          actions={[{
            id: 'archive',
            label: t('collection.archiveSelected'),
            icon: <Archive size={15} />,
            danger: true,
            disabled: selected => selected.length === 0,
            onSelect: archiveSelectedTasks,
          }]}
          isMobile={isMobile}
          sidebarOpen={sidebarOpen}
          onSidebarOpenChange={setSidebarOpen}
          globalSearchShortcut
          showSearchClear
          labels={{
            search: t('planning.search'),
            favorites: t('collection.favorites'),
            clearFilters: t('collection.clearFilters'),
            moreActions: t('collection.moreActions'),
            openCollection: t('collection.openCollection'),
            closeCollection: t('collection.closeCollection'),
            selectItem: t('collection.selectItem'),
            selectMultiple: t('collection.selectMultiple'),
            cancelSelection: t('collection.cancelSelection'),
            selectedCount: count => t('collection.selectedCount', count),
          }}
          slots={{
            sidebarHeaderEnd: <button
              type="button"
              className="collection-shell-search-action"
              data-active={filtersOpen}
              onClick={() => setFiltersOpen(value => !value)}
              aria-expanded={filtersOpen}
              aria-controls="planning-search-options"
              aria-label={t('planning.filters')}
              title={t('planning.filters')}
            >
              <Filter size={14} />
              <span>{t('planning.filters')}</span>
              {activeFilterCount > 0 && <strong>{activeFilterCount}</strong>}
            </button>,
            afterSidebarHeader: <>
              {filtersOpen && <div id="planning-search-options" className="collection-shell-search-options planning-filters">
                <select value={status} onChange={event => setStatus(event.target.value as PlanningTaskStatus | '')} aria-label={t('planning.allStatuses')}>
                  <option value="">{t('planning.allStatuses')}</option>
                  {(['idea', 'todo', 'in_progress', 'blocked', 'done', 'archived'] as PlanningTaskStatus[]).map(value => <option key={value} value={value}>{t(`planning.status.${value}`)}</option>)}
                </select>
                <select value={projectId} onChange={event => setProjectId(event.target.value)} aria-label={t('planning.allProjects')}>
                  <option value="">{t('planning.allProjects')}</option>
                  {projects.map(project => <option key={project.id} value={project.id}>{project.name}</option>)}
                </select>
                <select value={priorityFilter} onChange={event => setPriorityFilter(event.target.value as PlanningTaskPriority | '')} aria-label={t('planning.allPriorities')}>
                  <option value="">{t('planning.allPriorities')}</option>
                  {PRIORITIES.map(value => <option key={value} value={value}>{t(`planning.priority.${value}`)}</option>)}
                </select>
                <select value={withDiscussion} onChange={event => setWithDiscussion(event.target.value as typeof withDiscussion)} aria-label={t('planning.allLinks')}>
                  <option value="">{t('planning.allLinks')}</option><option value="yes">{t('planning.withDiscussion')}</option><option value="no">{t('planning.withoutDiscussion')}</option>
                </select>
                {tags.length > 0 && <select value={tag} onChange={event => setTag(event.target.value)} aria-label={t('planning.allTags')}>
                  <option value="">{t('planning.allTags')}</option>{tags.map(value => <option key={value} value={value}>{value}</option>)}
                </select>}
              </div>}
              <div className="planning-sidebar-summary">
                <span><strong>{activeTasks.length}</strong>{t('planning.activeCount')}</span>
                <span><strong>{completedTasks.length}</strong>{t('planning.doneCount')}</span>
              </div>
            </>,
            renderEmpty: () => <div className="planning-state"><Target size={24} /> {t('planning.emptyBacklog')}</div>,
            renderItem: (task, { selected }) => renderCard(task, selected),
            renderDetail: () => {
              if (!selectedId && !detailLoading) {
                return <div className="planning-detail-empty-hint">{t('planning.selectHint')}</div>;
              }
              return (
                <aside className="planning-detail" aria-label={t('planning.taskActions')}>
                  <header>
                    {detail && (
                      <CopyIdPill
                        id={detail.id}
                        label={detail.reference}
                        title={t('planning.copyTaskId', detail.reference)}
                      />
                    )}
                    <button type="button" className="planning-detail-close" onClick={() => {
                      setSelectedId(null);
                      setDetail(null);
                      setDetailLoading(false);
                    }} aria-label={t('common.close')}><X size={16} /></button>
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
                      onRemoveBlocker={async blockerTaskId => {
                        try {
                          const refreshed = await planning.removeBlocker(detail.id, blockerTaskId);
                          setDetail(refreshed);
                          setTasks(previous => previous.map(item => item.id === refreshed.id ? refreshed : item));
                          toast(t('planning.blockerRemoved'), 'success');
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
              );
            },
          }}
        />
      </div>
      {createModalOpen && (
        <div
          className="planning-create-modal-backdrop"
          role="presentation"
          onMouseDown={event => {
            if (event.target === event.currentTarget) closeCreateModal();
          }}
        >
          <section
            ref={createModalRef}
            className="planning-create-modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="planning-create-modal-title"
            aria-describedby="planning-create-modal-hint"
            onKeyDown={handleCreateModalKeyDown}
          >
            <header className="planning-create-modal-header">
              <div className="planning-create-modal-heading">
                <span className="planning-create-modal-icon" aria-hidden="true"><Target size={16} /></span>
                <span>
                  <strong id="planning-create-modal-title">{t('planning.createTitle')}</strong>
                  <small id="planning-create-modal-hint">{t('planning.createHint')}</small>
                </span>
              </div>
              <button
                type="button"
                className="planning-create-modal-close"
                onClick={closeCreateModal}
                disabled={creating}
                aria-label={t('common.close')}
              >
                <X size={16} />
              </button>
            </header>
            <form onSubmit={event => { event.preventDefault(); void createQuickTask(); }}>
              <div className="planning-create-modal-body">
                <label htmlFor="planning-create-title">{t('planning.taskTitle')}</label>
                <input
                  ref={createTitleRef}
                  id="planning-create-title"
                  value={quickTitle}
                  onChange={event => setQuickTitle(event.target.value)}
                  placeholder={t('planning.newIdea')}
                  maxLength={240}
                  required
                />
                <label htmlFor="planning-create-priority">{t('planning.priorityField')}</label>
                <select
                  id="planning-create-priority"
                  value={quickPriority}
                  onChange={event => setQuickPriority(event.target.value as PlanningTaskPriority)}
                >
                  {PRIORITIES.map(value => <option key={value} value={value}>{t(`planning.priority.${value}`)}</option>)}
                </select>
                {quickTitle.trim() && duplicateCandidates.length > 0 && <div className="planning-duplicates">
                  <span>{t('planning.possibleDuplicates')}</span>
                  {duplicateCandidates.map(task => <button type="button" key={task.id} onClick={() => {
                    selectTask(task.id);
                    setCreateModalOpen(false);
                  }}>{task.reference} · {task.title}</button>)}
                </div>}
              </div>
              <footer className="planning-create-modal-footer">
                <button type="button" className="btn btn-ghost" onClick={closeCreateModal} disabled={creating}>{t('common.cancel')}</button>
                <button type="submit" className="planning-create-modal-submit" disabled={!quickTitle.trim() || creating}>
                  {creating ? <Loader2 size={15} className="spin" /> : <Plus size={15} />}
                  {t('planning.createAction')}
                </button>
              </footer>
            </form>
          </section>
        </div>
      )}
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
  onRemoveBlocker: (blockerTaskId: string) => Promise<void>;
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
  onRemoveBlocker,
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
      {(task.workspaces?.length ?? 0) > 0 && (
        <section className="planning-task-workspaces">
          <h3>{t('planning.workspaces')}</h3>
          {task.workspaces?.map(workspace => (
            <button
              type="button"
              key={workspace.id}
              onClick={() => {
                queueDiscussionWorkspaceTarget(workspace.disc_id, workspace.id);
                onNavigateDiscussion(workspace.disc_id);
              }}
              title={t('planning.workspaceViewFiles')}
            >
              <GitBranch size={12} />
              <span>{workspace.branch}</span>
              <small>
                {workspace.session_agent_type ?? t('git.workspaceManaged')}
                {' · '}
                {t(`planning.workspaceState.${workspace.state}`)}
              </small>
            </button>
          ))}
        </section>
      )}
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
          <div className="planning-blocker-row" key={blocker.id}>
            <button type="button" onClick={() => onOpenTask(blocker.id)}>
              <Circle size={12} /> {blocker.reference} · {blocker.title}
              <ChevronRight size={12} />
            </button>
            <button
              type="button"
              className="planning-blocker-remove"
              disabled={linking}
              aria-label={t('planning.removeBlocker', blocker.reference)}
              title={t('planning.removeBlocker', blocker.reference)}
              onClick={() => {
                setLinking(true);
                void onRemoveBlocker(blocker.id)
                  .catch(() => undefined)
                  .finally(() => setLinking(false));
              }}
            >
              <X size={13} />
            </button>
          </div>
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
