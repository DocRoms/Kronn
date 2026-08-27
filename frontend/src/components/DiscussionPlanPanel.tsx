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
  GitBranch,
  Loader2,
  Inbox,
  List,
  MessageCircle,
  MessagesSquare,
  Plus,
  Target,
  X,
} from 'lucide-react';
import {
  discussions,
  orchestration,
  planning,
  type CampaignView,
} from '../lib/api';
import { queueDiscussionWorkspaceTarget } from '../lib/discussion-navigation';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import { readPlanOrchestrationState, writePlanOrchestrationState } from '../lib/orch-panel-state';
import { CopyIdPill } from './CopyIdPill';
import { PlanAllTasksView } from './PlanAllTasksView';
import { PlanningProposalReview } from './PlanningProposalReview';
import { TaskCampaignPanel } from './TaskCampaignPanel';
import { TaskExecutionCard } from './TaskExecutionCard';
import { TaskLaunchDialog } from './TaskLaunchDialog';
import { orchestrationResolution } from './taskLaunchResolution';
import type {
  AgentType,
  Discussion,
  DiscussionPlan,
  DiscussionWorkspace,
  ExecutionDiscussionLink,
  PlanningDiscussionRelation,
  PlanningProposal,
  PlanningTaskDetail,
  PlanningTaskSummary,
  PlanningTaskStatus,
  ProposalListResponse,
  TaskExecutionDetail,
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
  const initialPanelState = useMemo(
    () => readPlanOrchestrationState(discussionId),
    [discussionId],
  );
  const [plan, setPlan] = useState<DiscussionPlan | null>(null);
  const [proposalInbox, setProposalInbox] = useState<ProposalListResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [quickTitle, setQuickTitle] = useState('');
  const [creating, setCreating] = useState(false);
  const [primaryOpen, setPrimaryOpen] = useState(true);
  const [viewMode, setViewMode] = useState<'focus' | 'all'>(initialPanelState.viewMode);
  const [currentExpandedFor, setCurrentExpandedFor] = useState<string | null>(null);
  const [upcomingExpandedFor, setUpcomingExpandedFor] = useState<string | null>(null);
  const [allQuery, setAllQuery] = useState('');
  const [planFilter, setPlanFilter] = useState<PlanFilter | null>(null);
  const [selectedTask, setSelectedTask] = useState<PlanningTaskDetail | null>(null);
  const [selectedTaskPreference, setSelectedTaskPreference] = useState<string | null>(
    initialPanelState.selectedTaskId,
  );
  const [detailLoading, setDetailLoading] = useState(false);
  const [dodSavingIndex, setDodSavingIndex] = useState<number | null>(null);
  const [campaign, setCampaign] = useState<CampaignView | null>(null);
  const [executionLinks, setExecutionLinks] = useState<ExecutionDiscussionLink[]>([]);
  const [executionDetail, setExecutionDetail] = useState<TaskExecutionDetail | null>(null);
  const [discussion, setDiscussion] = useState<Discussion | null>(null);
  const [workspaces, setWorkspaces] = useState<DiscussionWorkspace[]>([]);
  const [launchTaskReference, setLaunchTaskReference] = useState<string | null>(null);
  const [executionLoading, setExecutionLoading] = useState(false);
  const [actionBusy, setActionBusy] = useState('');
  const [actionError, setActionError] = useState('');
  const [approveOpen, setApproveOpen] = useState(false);
  const [approvalConfirmed, setApprovalConfirmed] = useState<Record<string, boolean>>({});
  const [approvalEvidence, setApprovalEvidence] = useState<Record<string, string>>({});
  const [requestChangesOpen, setRequestChangesOpen] = useState(false);
  const [reviewComment, setReviewComment] = useState('');
  const [confirmStop, setConfirmStop] = useState(false);
  const [reassignOpen, setReassignOpen] = useState(false);
  const [reassignAgent, setReassignAgent] = useState<AgentType>('Codex');
  const [reassignModel, setReassignModel] = useState('');
  const selectedTaskIdRef = useRef<string | null>(initialPanelState.selectedTaskId);

  useEffect(() => {
    writePlanOrchestrationState(discussionId, {
      selectedTaskId: selectedTaskIdRef.current,
      viewMode,
    });
  }, [discussionId, selectedTaskPreference, viewMode]);

  const readPlan = useCallback(async () => {
    const selectedId = selectedTaskIdRef.current;
    const [next, nextDetail, nextProposals, nextCampaign, nextLinks, nextDiscussion, nextWorkspaces] = await Promise.all([
      planning.discussionPlan(discussionId),
      selectedId ? planning.get(selectedId) : Promise.resolve(null),
      planning.proposals(discussionId),
      orchestration.discussionCampaign(discussionId),
      orchestration.discussionLinks(),
      discussions.get(discussionId),
      discussions.workspaces(discussionId),
    ]);
    return {
      next,
      nextDetail,
      nextProposals,
      nextCampaign,
      nextLinks,
      nextDiscussion,
      nextWorkspaces,
      selectedId,
    };
  }, [discussionId]);

  const applyPlan = useCallback((result: Awaited<ReturnType<typeof readPlan>>) => {
    setPlan(result.next);
    setProposalInbox(result.nextProposals);
    setCampaign(result.nextCampaign);
    setExecutionLinks(result.nextLinks.filter(link => link.parent_discussion_id === discussionId));
    setDiscussion(result.nextDiscussion);
    setWorkspaces(result.nextWorkspaces);
    if (result.nextDetail && selectedTaskIdRef.current === result.selectedId) {
      setSelectedTask(result.nextDetail);
    }
    onChanged?.(result.next);
  }, [discussionId, onChanged]);

  const refresh = useCallback(async (silent = false) => {
    if (!silent) {
      setLoading(true);
      setError('');
    }
    try {
      applyPlan(await readPlan());
    } catch (cause) {
      if (!silent) setError(userError(cause));
    } finally {
      if (!silent) setLoading(false);
    }
  }, [applyPlan, readPlan]);

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
    let active = true;
    readPlan()
      .then(result => {
        if (active) applyPlan(result);
      })
      .catch(cause => {
        if (active) setError(userError(cause));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
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
      active = false;
      window.removeEventListener('kronn:plan-proposals-changed', refreshProposals);
      window.clearInterval(interval);
    };
  }, [applyPlan, discussionId, readPlan, refresh]);

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
  const currentExpanded = currentExpandedFor === discussionId;
  const upcomingExpanded = upcomingExpandedFor === discussionId;
  const visibleCurrent = currentExpanded ? current : current.slice(0, 3);
  const visibleUpcoming = upcomingExpanded ? upcoming : upcoming.slice(0, 5);
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
  const selectedExecutionLink = useMemo(() => {
    if (!selectedTask) return null;
    return [...executionLinks].reverse().find(link => link.task_id === selectedTask.id) ?? null;
  }, [executionLinks, selectedTask]);
  const selectedCandidate = selectedTask
    ? campaign?.candidates.find(candidate => candidate.task.id === selectedTask.id) ?? null
    : null;
  // Jumping to a worker's discussion used to require opening the execution
  // detail first. The row itself is where someone scanning the plan asks the
  // question, so the shortcut belongs there. Last link wins: a re-run leaves
  // older links behind, and the current one is what "go see it" means.
  const subDiscussionByTaskId = useMemo(() => {
    const byTask = new Map<string, string>();
    for (const link of executionLinks) {
      if (link.sub_discussion_id) byTask.set(link.task_id, link.sub_discussion_id);
    }
    return byTask;
  }, [executionLinks]);

  useEffect(() => {
    const executionId = selectedExecutionLink?.execution_id;
    let active = true;
    if (!executionId) {
      queueMicrotask(() => {
        if (active) {
          setExecutionDetail(null);
          setExecutionLoading(false);
        }
      });
      return () => { active = false; };
    }
    queueMicrotask(() => {
      if (active) setExecutionLoading(true);
    });
    orchestration.execution(executionId)
      .then(detail => {
        if (active) setExecutionDetail(detail);
      })
      .catch(cause => {
        if (active) setActionError(userError(cause));
      })
      .finally(() => {
        if (active) setExecutionLoading(false);
      });
    return () => { active = false; };
  }, [selectedExecutionLink?.execution_id]);

  const runExecutionAction = async (name: string, action: () => Promise<unknown>) => {
    if (actionBusy) return;
    setActionBusy(name);
    setActionError('');
    try {
      await action();
      setApproveOpen(false);
      setApprovalConfirmed({});
      setApprovalEvidence({});
      setRequestChangesOpen(false);
      setReviewComment('');
      setConfirmStop(false);
      setReassignOpen(false);
      await refresh(true);
    } catch (cause) {
      setActionError(userError(cause));
    } finally {
      setActionBusy('');
    }
  };

  const reviewSelectedExecution = (decision: 'approve' | 'request_changes') => {
    const execution = executionDetail?.lineage.execution;
    if (!execution || !selectedTask) return;
    const delivery = executionDetail.attempts
      .find(attempt => attempt.attempt_no === execution.attempt_no)
      ?.delivery;
    if (decision === 'approve' && !delivery) {
      setActionError(t('orch.approveMissingDelivery'));
      return;
    }
    void runExecutionAction(decision, () => orchestration.reviewExecution(execution.id, {
      version: '1',
      task_ref: selectedTask.reference,
      decision,
      reviewed_head_sha: decision === 'approve' ? delivery?.head_sha : undefined,
      dod_verifications: decision === 'approve'
        ? executionDetail.definition_of_done.map(item => ({
          dod_id: item.id,
          met: approvalConfirmed[item.id] === true,
          evidence: approvalEvidence[item.id]?.trim() ?? '',
        }))
        : undefined,
      comment: decision === 'request_changes' ? reviewComment.trim() : undefined,
    }));
  };

  const openApproval = () => {
    const items = executionDetail?.definition_of_done ?? [];
    setApprovalConfirmed(Object.fromEntries(items.map(item => [item.id, false])));
    setApprovalEvidence(Object.fromEntries(items.map(item => [item.id, ''])));
    setApproveOpen(true);
  };

  const currentAttemptNo = executionDetail?.lineage.execution.attempt_no;
  const currentDelivery = executionDetail?.attempts
    .find(attempt => attempt.attempt_no === currentAttemptNo)
    ?.delivery ?? null;
  const approvalReady = Boolean(currentDelivery)
    && (executionDetail?.definition_of_done ?? []).every(item => (
      approvalConfirmed[item.id] === true
      && Boolean(approvalEvidence[item.id]?.trim())
    ));

  const reassignSelectedExecution = () => {
    const execution = executionDetail?.lineage.execution;
    if (!execution) return;
    void runExecutionAction('reassign', () => orchestration.reassignExecution(
      execution.id,
      {
        target: { kind: 'agent', agent_type: reassignAgent },
        model: reassignModel.trim() || null,
        profile_id: null,
      },
      'Réassignation demandée depuis le panneau d’orchestration',
    ));
  };
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
      selectedTaskIdRef.current = null;
      setSelectedTaskPreference(null);
      return;
    }
    selectedTaskIdRef.current = taskId;
    setSelectedTaskPreference(taskId);
    setDetailLoading(true);
    try {
      setSelectedTask(await planning.get(taskId));
    } catch (cause) {
      toast(userError(cause), 'error');
    } finally {
      setDetailLoading(false);
    }
  };

  const closeDetail = () => {
    setSelectedTask(null);
    selectedTaskIdRef.current = null;
    setSelectedTaskPreference(null);
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
        {subDiscussionByTaskId.get(relation.task.id) && onNavigateDiscussion && (
          <button
            type="button"
            className="plan-task-action"
            title={t('orch.openSubDiscussion')}
            aria-label={t('orch.openSubDiscussion')}
            onClick={() => {
              const target = subDiscussionByTaskId.get(relation.task.id);
              if (target) onNavigateDiscussion(target);
            }}
          >
            <MessagesSquare size={11} />
          </button>
        )}
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
          <TaskCampaignPanel
            view={campaign}
            onLaunch={setLaunchTaskReference}
            busyTaskReference={launchTaskReference}
          />
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
              {current.length > 3 && (
                <button
                  type="button"
                  className="plan-focus-overflow"
                  aria-expanded={currentExpanded}
                  onClick={() => setCurrentExpandedFor(currentExpanded ? null : discussionId)}
                >
                  {currentExpanded
                    ? t('planning.collapseCurrent')
                    : t('planning.moreCurrent', current.length - visibleCurrent.length)}
                </button>
              )}
            </section>
          )}

          {viewMode === 'focus' && visibleUpcoming.length > 0 && (
            <section className="plan-timeline-section" data-kind="upcoming">
              <h3><Flag size={13} /> {t('planning.upcoming')}</h3>
              {visibleUpcoming.map(renderTask)}
              {upcoming.length > 5 && (
                <button
                  type="button"
                  className="plan-focus-overflow"
                  aria-expanded={upcomingExpanded}
                  onClick={() => setUpcomingExpandedFor(upcomingExpanded ? null : discussionId)}
                >
                  {upcomingExpanded
                    ? t('planning.collapseReady')
                    : t('planning.moreReady', upcoming.length - visibleUpcoming.length)}
                </button>
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
              <button type="button" onClick={closeDetail}><X size={13} /></button>
            </div>
            <h3>{selectedTask.title}</h3>
            {selectedTask.description && <p>{selectedTask.description}</p>}
            {!selectedExecutionLink && (
              <div className="plan-orch-launch">
                {(selectedCandidate?.launchable || (!campaign && selectedRelation?.actionable)) && (
                  <button
                    type="button"
                    className="btn btn-primary"
                    onClick={() => setLaunchTaskReference(selectedTask.reference)}
                  >
                    {t('orch.launch')}
                  </button>
                )}
                {selectedCandidate && !selectedCandidate.launchable && (
                  <ul className="orch-campaign-reasons" role="status">
                    {selectedCandidate.reasons.map(reason => (
                      <li key={reason.code}>{reason.detail}</li>
                    ))}
                  </ul>
                )}
              </div>
            )}
            {executionLoading && (
              <div className="plan-panel-state"><Loader2 size={13} className="spin" /> {t('common.loading')}</div>
            )}
            <TaskExecutionCard
              detail={executionDetail}
              onOpen={onNavigateDiscussion}
              onStop={() => setConfirmStop(true)}
              onReassign={() => {
                const execution = executionDetail?.lineage.execution;
                setReassignAgent((execution?.worker_agent_type as AgentType | null) ?? discussion?.agent ?? 'Codex');
                setReassignModel(execution?.worker_model ?? '');
                setReassignOpen(true);
              }}
              onApprove={openApproval}
              onRequestChanges={() => setRequestChangesOpen(true)}
            />
            {actionError && (
              <div className="orch-action-error" role="alert">
                <strong>{actionError}</strong>
                <span>{t(`orch.resolution.${orchestrationResolution(actionError)}`)}</span>
              </div>
            )}
            {confirmStop && executionDetail && (
              <div className="orch-inline-action" role="alertdialog" aria-label={t('orch.exec.stop')}>
                <p>{t('orch.stopConfirm')}</p>
                <div>
                  <button type="button" onClick={() => setConfirmStop(false)} disabled={Boolean(actionBusy)}>{t('common.cancel')}</button>
                  <button
                    type="button"
                    className="btn btn-danger"
                    disabled={Boolean(actionBusy)}
                    onClick={() => void runExecutionAction('stop', () => orchestration.cancelExecution(
                      executionDetail.lineage.execution.id,
                      'Arrêt demandé depuis le panneau d’orchestration',
                    ))}
                  >
                    {t('orch.exec.stop')}
                  </button>
                </div>
              </div>
            )}
            {approveOpen && executionDetail && (
              <div className="orch-inline-action" role="dialog" aria-label={t('orch.exec.approve')}>
                <p>{t('orch.approveEvidenceIntro')}</p>
                <code className="orch-approval-head">
                  HEAD {currentDelivery?.head_sha ?? t('orch.approveMissingDelivery')}
                </code>
                <div className="orch-approval-dod-list">
                  {executionDetail.definition_of_done.map(item => (
                    <fieldset className="orch-approval-dod" key={item.id}>
                      <label className="orch-approval-confirm">
                        <input
                          type="checkbox"
                          checked={approvalConfirmed[item.id] === true}
                          onChange={event => setApprovalConfirmed(previous => ({
                            ...previous,
                            [item.id]: event.target.checked,
                          }))}
                        />
                        <span>{item.sentence}</span>
                      </label>
                      <label>
                        <span>{t('orch.approveEvidence')}</span>
                        <textarea
                          value={approvalEvidence[item.id] ?? ''}
                          onChange={event => setApprovalEvidence(previous => ({
                            ...previous,
                            [item.id]: event.target.value,
                          }))}
                        />
                      </label>
                    </fieldset>
                  ))}
                </div>
                <div>
                  <button type="button" onClick={() => setApproveOpen(false)} disabled={Boolean(actionBusy)}>{t('common.cancel')}</button>
                  <button
                    type="button"
                    className="btn btn-primary"
                    disabled={Boolean(actionBusy) || !approvalReady}
                    onClick={() => reviewSelectedExecution('approve')}
                  >
                    {t('orch.exec.approve')}
                  </button>
                </div>
              </div>
            )}
            {requestChangesOpen && (
              <div className="orch-inline-action" role="dialog" aria-label={t('orch.exec.requestChanges')}>
                <label>
                  <span>{t('orch.reviewComment')}</span>
                  <textarea autoFocus value={reviewComment} onChange={event => setReviewComment(event.target.value)} />
                </label>
                <div>
                  <button type="button" onClick={() => setRequestChangesOpen(false)} disabled={Boolean(actionBusy)}>{t('common.cancel')}</button>
                  <button
                    type="button"
                    className="btn btn-primary"
                    disabled={Boolean(actionBusy) || !reviewComment.trim()}
                    onClick={() => reviewSelectedExecution('request_changes')}
                  >
                    {t('orch.exec.requestChanges')}
                  </button>
                </div>
              </div>
            )}
            {reassignOpen && (
              <div className="orch-inline-action" role="dialog" aria-label={t('orch.exec.reassign')}>
                <label>
                  <span>{t('orch.config.agent')}</span>
                  <select value={reassignAgent} onChange={event => setReassignAgent(event.target.value as AgentType)}>
                    {Array.from(new Set([
                      ...(campaign?.run.allowed_agents ?? []),
                      discussion?.agent ?? 'Codex',
                    ])).map(value => <option value={value} key={value}>{value}</option>)}
                  </select>
                </label>
                <label>
                  <span>{t('orch.config.model')}</span>
                  <input value={reassignModel} onChange={event => setReassignModel(event.target.value)} />
                </label>
                <div>
                  <button type="button" onClick={() => setReassignOpen(false)} disabled={Boolean(actionBusy)}>{t('common.cancel')}</button>
                  <button type="button" className="btn btn-primary" disabled={Boolean(actionBusy)} onClick={reassignSelectedExecution}>
                    {t('orch.exec.reassign')}
                  </button>
                </div>
              </div>
            )}
            {(selectedTask.workspaces?.length ?? 0) > 0 && (
              <div className="plan-task-workspaces">
                <strong>{t('planning.workspaces')}</strong>
                {selectedTask.workspaces?.map(workspace => (
                  <button
                    type="button"
                    key={workspace.id}
                    onClick={() => {
                      queueDiscussionWorkspaceTarget(workspace.disc_id, workspace.id);
                      onNavigateDiscussion?.(workspace.disc_id);
                    }}
                    title={t('planning.workspaceViewFiles')}
                  >
                    <GitBranch size={11} />
                    <span>{workspace.branch}</span>
                    <small>
                      {workspace.session_agent_type ?? t('git.workspaceManaged')}
                      {' · '}
                      {t(`planning.workspaceState.${workspace.state}`)}
                    </small>
                  </button>
                ))}
              </div>
            )}
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
      <TaskLaunchDialog
        open={launchTaskReference !== null}
        discussionId={discussionId}
        projectId={discussion?.project_id ?? null}
        taskReference={launchTaskReference ?? ''}
        defaultAgent={discussion?.agent ?? 'Codex'}
        defaultBranch={workspaces[0]?.branch ?? 'main'}
        workspaces={workspaces}
        campaign={campaign}
        onClose={() => setLaunchTaskReference(null)}
        onLaunched={(_executionId, nextCampaign) => {
          setCampaign(nextCampaign);
          setLaunchTaskReference(null);
          void refresh(true);
        }}
      />
    </aside>
  );
}
