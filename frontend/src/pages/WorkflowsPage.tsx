import { useState, useRef, useMemo, useEffect } from 'react';
import { useIsMobile } from '../hooks/useMediaQuery';
import { useT } from '../lib/I18nContext';
import { workflows as workflowsApi, discussions as discussionsApi, quickPrompts as quickPromptsApi } from '../lib/api';
import { userError } from '../lib/userError';
import { useApi } from '../hooks/useApi';
import type {
  Project, WorkflowSummary, Workflow, WorkflowRun,
  AgentType, AgentsConfig, StepResult, QuickPrompt, CreateQuickPromptRequest,
} from '../types/generated';
import {
  Plus, Trash2, Play, Loader2, ChevronLeft, ChevronRight, ChevronDown,
  Clock, GitBranch, Zap, Eye, Layers, X,
  ToggleLeft, ToggleRight,
} from 'lucide-react';
import { WorkflowDetail } from '../components/workflows/WorkflowDetail';
import { WorkflowWizard } from '../components/workflows/WorkflowWizard';
import { QuickPromptForm } from '../components/workflows/QuickPromptForm';
import './WorkflowsPage.css';

interface WorkflowsPageProps {
  projects: Project[];
  installedAgentTypes?: AgentType[];
  agentAccess?: AgentsConfig;
  configLanguage?: string;
  onNavigateDiscussion?: (discId: string) => void;
  /** Called after a batch launch — parent marks every returned disc id as
   * "sending" in the shared sendingMap so the sidebar spinner lights up for
   * all of them, not just the one we navigate to. Without this prop the
   * batch still works but only the navigated disc looks like it's running. */
  onBatchLaunched?: (discIds: string[]) => void;
  /** When the user clicks a batch pastille in the discussion sidebar, the
   * Dashboard switches to this tab and sets this prop to the parent workflow
   * id. We auto-open its detail panel + switch to the 'workflows' sub-tab
   * so the user lands exactly on the run that spawned their batch. */
  initialSelectedWorkflowId?: string | null;
  /** Ack callback — Dashboard clears the id after we've consumed it so the
   * same click doesn't re-open on every render. */
  onInitialSelectionConsumed?: () => void;
  /** Reverse direction: when "📋 N conversations" is clicked on a workflow run,
   * jump to the discussions tab and focus that batch group. */
  onNavigateToBatch?: (batchRunId: string) => void;
  toast?: (msg: string, type?: 'success' | 'error' | 'info') => void;
}

const TRIGGER_LABELS: Record<string, string> = {
  cron: 'Cron',
  tracker: 'Tracker',
  manual: 'Manuel',
};

const STATUS_COLORS: Record<string, string> = {
  Pending: '#ffc800',
  Running: '#00d4ff',
  Success: '#34d399',
  Failed: '#ff4d6a',
  Cancelled: 'var(--kr-cancelled)',
  WaitingApproval: '#c8ff00',
};

export function WorkflowsPage({ projects, installedAgentTypes, agentAccess, configLanguage, onNavigateDiscussion, onBatchLaunched, initialSelectedWorkflowId, onInitialSelectionConsumed, onNavigateToBatch, toast: toastProp }: WorkflowsPageProps) {
  const { t } = useT();
  const isMobile = useIsMobile();
  const [tab, setTab] = useState<'workflows' | 'quickPrompts'>('workflows');
  const { data: workflowList, refetch } = useApi(() => workflowsApi.list(), []);
  const { data: quickPromptList, refetch: refetchQP } = useApi(() => quickPromptsApi.list(), []);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [showCreateQP, setShowCreateQP] = useState(false);
  const [editingQP, setEditingQP] = useState<QuickPrompt | null>(null);
  const [launchingQP, setLaunchingQP] = useState<QuickPrompt | null>(null);
  const [launchVars, setLaunchVars] = useState<Record<string, string>>({});
  const [launching, setLaunching] = useState(false);
  // Batch launch state — when the user clicks "Batch" on a QP, we show a
  // modal that asks for one value of the first variable per line, then
  // fans out N discussions via POST /api/quick-prompts/:id/batch.
  const [batchingQP, setBatchingQP] = useState<QuickPrompt | null>(null);
  const [batchInputLines, setBatchInputLines] = useState('');
  const [batchLaunching, setBatchLaunching] = useState(false);
  const [batchIsolated, setBatchIsolated] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [editingWorkflow, setEditingWorkflow] = useState<Workflow | null>(null);
  const [detailWorkflow, setDetailWorkflow] = useState<Workflow | null>(null);
  const [detailRuns, setDetailRuns] = useState<WorkflowRun[]>([]);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [triggering, setTriggering] = useState<string | null>(null);

  // Live run state for SSE streaming
  const [liveRun, setLiveRun] = useState<{
    workflowId: string;
    currentStep: string | null;
    stepIndex: number;
    totalSteps: number;
    completedSteps: StepResult[];
    finished: boolean;
    status: string | null;
  } | null>(null);
  const abortRef = useRef<AbortController | null>(null);

  const workflows = workflowList ?? [];
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});

  const groupedWorkflows = useMemo(() => {
    const groups: { key: string; label: string; workflows: WorkflowSummary[] }[] = [];
    const byProject = new Map<string, WorkflowSummary[]>();
    const noProject: WorkflowSummary[] = [];
    for (const wf of workflows) {
      if (wf.project_id) {
        const arr = byProject.get(wf.project_id) ?? [];
        arr.push(wf);
        byProject.set(wf.project_id, arr);
      } else {
        noProject.push(wf);
      }
    }
    // Projects in order they appear
    for (const [pid, wfs] of byProject) {
      const label = wfs[0].project_name ?? pid;
      groups.push({ key: pid, label, workflows: wfs });
    }
    if (noProject.length > 0) {
      groups.push({ key: '__none__', label: t('wf.noProject'), workflows: noProject });
    }
    return groups;
  }, [workflows, t]);

  const toggleGroup = (key: string) => {
    setCollapsedGroups(prev => ({ ...prev, [key]: !prev[key] }));
  };

  const openDetail = async (id: string) => {
    setSelectedId(id);
    setLoadingDetail(true);
    try {
      const [wf, runs] = await Promise.all([
        workflowsApi.get(id),
        workflowsApi.listRuns(id),
      ]);
      setDetailWorkflow(wf);
      setDetailRuns(runs);
    } catch (e) {
      console.warn('Workflow action failed:', e);
    } finally {
      setLoadingDetail(false);
    }
  };

  // Cross-page navigation: when the sidebar's batch pastille is clicked, the
  // Dashboard passes the parent workflow id here. We auto-switch to the
  // "workflows" sub-tab, open its detail panel, then ack so the same click
  // doesn't re-fire on every render.
  useEffect(() => {
    if (!initialSelectedWorkflowId) return;
    setTab('workflows');
    openDetail(initialSelectedWorkflowId);
    onInitialSelectionConsumed?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialSelectedWorkflowId]);

  const handleTrigger = async (id: string) => {
    setTriggering(id);

    // Reset live run
    setLiveRun({
      workflowId: id,
      currentStep: null,
      stepIndex: 0,
      totalSteps: 0,
      completedSteps: [],
      finished: false,
      status: null,
    });

    const abort = new AbortController();
    abortRef.current = abort;

    await workflowsApi.triggerStream(
      id,
      // onStepStart
      (data) => {
        setLiveRun(prev => prev ? {
          ...prev,
          currentStep: data.step_name,
          stepIndex: data.step_index,
          totalSteps: data.total_steps,
        } : prev);
      },
      // onStepDone
      (stepResult) => {
        setLiveRun(prev => prev ? {
          ...prev,
          currentStep: null,
          completedSteps: [...prev.completedSteps, stepResult],
        } : prev);
      },
      // onRunDone
      (data) => {
        setLiveRun(prev => prev ? { ...prev, finished: true, status: data.status, currentStep: null } : prev);
        setTriggering(null);
        refetch();
        if (selectedId === id) openDetail(id);
      },
      // onError
      (error) => {
        console.warn('Workflow trigger error:', error);
        setLiveRun(prev => prev ? { ...prev, finished: true, status: 'Failed', currentStep: null } : prev);
        setTriggering(null);
      },
      abort.signal,
    );
  };

  const handleToggle = async (wf: WorkflowSummary) => {
    try {
      await workflowsApi.update(wf.id, { enabled: !wf.enabled });
      refetch();
    } catch (e) {
      console.warn('Workflow action failed:', e);
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await workflowsApi.delete(id);
      if (selectedId === id) {
        setSelectedId(null);
        setDetailWorkflow(null);
      }
      refetch();
    } catch (e) {
      console.warn('Workflow action failed:', e);
    }
  };

  const handleSaveQP = async (req: CreateQuickPromptRequest) => {
    if (editingQP) {
      await quickPromptsApi.update(editingQP.id, req);
    } else {
      await quickPromptsApi.create(req);
    }
    setShowCreateQP(false);
    setEditingQP(null);
    refetchQP();
  };

  const renderTemplate = (template: string, vars: Record<string, string>): string => {
    let rendered = template;
    // 1. Process conditional sections: {{#var}}content{{/var}} — removed if var is empty
    rendered = rendered.replace(/\{\{#(\w+)\}\}([\s\S]*?)\{\{\/\1\}\}/g, (_, name, content) => {
      return vars[name]?.trim() ? content : '';
    });
    // 2. Replace remaining {{var}} placeholders
    rendered = rendered.replace(/\{\{(\w+)\}\}/g, (_, name) => vars[name] ?? '');
    // 3. Clean up double spaces/commas from removed sections
    rendered = rendered.replace(/  +/g, ' ').replace(/, ,/g, ',').trim();
    return rendered;
  };

  const handleLaunchQP = async (qp: QuickPrompt) => {
    setLaunching(true);
    try {
      const rendered = renderTemplate(qp.prompt_template, launchVars);
      // Build dynamic title with first non-empty variable value
      const firstVal = qp.variables.map(v => launchVars[v.name]).find(v => v?.trim());
      const title = firstVal ? `${qp.name} — ${firstVal}` : qp.name;
      const disc = await discussionsApi.create({
        project_id: qp.project_id ?? null,
        title,
        agent: qp.agent,
        language: configLanguage || 'fr',
        initial_prompt: rendered,
        skill_ids: qp.skill_ids?.length ? qp.skill_ids : undefined,
        tier: qp.tier !== 'default' ? qp.tier : undefined,
      });
      setLaunchingQP(null);
      setLaunchVars({});
      onNavigateDiscussion?.(disc.id);
    } catch (e) {
      console.warn('Launch failed:', e);
    } finally {
      setLaunching(false);
    }
  };

  /**
   * Fan-out launch: parse the user's list (one value per line), render the
   * QP template for each, call POST /quick-prompts/:id/batch which creates
   * N child discussions linked to a batch WorkflowRun, then kick off the
   * agent runs in parallel (backend semaphore limits concurrency to 5).
   */
  const handleBatchLaunch = async (qp: QuickPrompt) => {
    // Accept newline, comma AND semicolon as separators — users often paste
    // a comma-separated list from Jira/GitHub search results on a single line
    // (real-world bug report from the first batch run that shipped a single
    // disc with title "EW-7223,EW-7182,EW-6071,EW-7141").
    // Also deduplicate so accidental repeats don't create duplicate discussions.
    const rawItems = batchInputLines
      .split(/[\n,;]/)
      .map(s => s.trim())
      .filter(Boolean);
    const seen = new Set<string>();
    const lines: string[] = [];
    for (const item of rawItems) {
      if (!seen.has(item)) {
        seen.add(item);
        lines.push(item);
      }
    }
    const notify = toastProp ?? ((msg: string) => alert(msg));
    if (lines.length === 0) {
      notify(t('qp.batch.emptyInput'), 'error');
      return;
    }
    if (lines.length > 50) {
      notify(t('qp.batch.tooManyItems', lines.length), 'error');
      return;
    }
    if (qp.variables.length === 0) {
      notify(t('qp.batch.needsVariable'), 'error');
      return;
    }
    // Use the FIRST variable as the batch key. Each line becomes the value
    // of that variable for one child discussion. For now we only support
    // 1 variable per batch — multi-var comes in Phase 2 with auto-fetch.
    const keyVar = qp.variables[0];
    // Confirm before firing — N agents × ~X tokens is not cheap.
    const estimateMsg = t('qp.batch.confirm', lines.length, qp.name);
    if (!confirm(estimateMsg)) return;

    setBatchLaunching(true);
    try {
      const items = lines.map(line => {
        const vars: Record<string, string> = { [keyVar.name]: line };
        const prompt = renderTemplate(qp.prompt_template, vars);
        return { title: `${qp.name} — ${line}`, prompt };
      });
      const now = new Date();
      const batchName = `${qp.name} — ${now.toLocaleString(configLanguage || 'fr', {
        day: '2-digit', month: 'short', hour: '2-digit', minute: '2-digit',
      })}`;
      const res = await quickPromptsApi.batchRun(qp.id, {
        items,
        batch_name: batchName,
        project_id: qp.project_id ?? null,
        workspace_mode: batchIsolated ? 'Isolated' : 'Direct',
      });
      // Kick off each child discussion's agent run in parallel.
      //
      // Two gotchas:
      // 1. The backend spawns the agent in a detached tokio task that
      //    outlives the HTTP request, so we don't need the SSE stream
      //    at all. We abort the fetch right after dispatch to release
      //    the browser's per-host HTTP/1.1 connection slot (~6 max).
      //    Without this, launching >5 discs saturates the connection
      //    pool and later POSTs queue on the client side, making the
      //    batch look sequential.
      // 2. We mark every disc as "sending" in the shared sendingMap so
      //    the sidebar spinner lights up for all of them — otherwise
      //    only the navigated disc looks like it's running.
      res.discussion_ids.forEach(discId => {
        const controller = new AbortController();
        fetch(`/api/discussions/${discId}/run`, {
          method: 'POST',
          signal: controller.signal,
          // keepalive lets the browser finish dispatching even if we
          // abort the read side almost immediately.
          keepalive: true,
        }).catch(() => {
          // Aborted or network blip — backend run is independent, so
          // we just log and move on.
        });
        // Release the connection slot after the request is definitely
        // on the wire. 500ms is enough for the browser to flush the
        // headers + body; the backend `tokio::spawn` inside
        // make_agent_stream has already captured the work.
        setTimeout(() => controller.abort(), 500);
      });
      // Tell the parent (Dashboard) to mark all these disc ids as
      // sending AND to open the first one — this is what lights up
      // the spinner in the sidebar and routes the user to the live
      // batch view. We deliberately do NOT call onNavigateDiscussion
      // here: that path triggers an auto-run on the navigated disc,
      // which would create a second POST /run on top of the one we
      // already fired in the fan-out loop, doubling its response.
      // `onBatchLaunched` in Dashboard uses `setOpenDiscussionId`
      // which opens without auto-running.
      onBatchLaunched?.(res.discussion_ids);
      setBatchingQP(null);
      setBatchInputLines('');
    } catch (e) {
      console.warn('Batch launch failed:', e);
      notify(t('qp.batch.failed', userError(e)), 'error');
    } finally {
      setBatchLaunching(false);
    }
  };

  return (
    <div>
      <div className="flex-between mb-4">
        <div>
          <h1 className="wf-h1">{t('wf.title')}</h1>
        </div>
        {tab === 'workflows' ? (
        <div className="flex-row gap-3">
          {onNavigateDiscussion && (
            <button className="wf-create-ai-btn" title={t('wf.createWithAIHint')} onClick={async () => {
              try {
                // Inject project list as compact lookup table (max 20, saves tokens)
                const shown = projects.slice(0, 20);
                const projectContext = shown.length > 0
                  ? '\n\n---\nProjets[name=id]: ' + shown.map(p => `${p.name}=${p.id}`).join(' | ')
                    + (projects.length > 20 ? ` (+${projects.length - 20})` : '')
                  : '';
                const disc = await discussionsApi.create({
                  project_id: null,
                  title: 'Workflow Architect',
                  agent: 'ClaudeCode',
                  language: configLanguage || 'fr',
                  initial_prompt: t('wf.aiArchitectPrompt') + projectContext,
                  skill_ids: ['workflow-architect'],
                  profile_ids: [],
                  directive_ids: [],
                  tier: 'reasoning',
                });
                onNavigateDiscussion(disc.id);
              } catch (e) {
                console.warn('Failed to create AI discussion:', e);
              }
            }}>
              <Zap size={14} /> {t('wf.createWithAI')}
            </button>
          )}
          <button className="wf-create-btn" title={t('wf.newHint')} onClick={() => setShowCreate(true)}>
            <Plus size={14} /> {t('wf.new')}
          </button>
        </div>
        ) : (
        <button className="wf-create-btn" onClick={() => { setShowCreateQP(true); setEditingQP(null); }}>
          <Plus size={14} /> {t('qp.new')}
        </button>
        )}
      </div>

      {/* Tab bar */}
      <div className="dash-tab-bar mb-6">
        <button className="dash-tab" data-active={tab === 'workflows'} onClick={() => setTab('workflows')}>
          {t('wf.tabWorkflows')} {workflowList ? `(${workflowList.length})` : ''}
        </button>
        <button className="dash-tab" data-active={tab === 'quickPrompts'} onClick={() => setTab('quickPrompts')}>
          {t('wf.tabQuickPrompts')} {quickPromptList ? `(${quickPromptList.length})` : ''}
        </button>
      </div>

      {/* ═══ WORKFLOWS TAB ═══ */}
      {tab === 'workflows' && (<>

      {/* Create wizard */}
      {showCreate && (
        <WorkflowWizard
          projects={projects}
          installedAgentTypes={installedAgentTypes}
          agentAccess={agentAccess}
          onDone={() => { setShowCreate(false); refetch(); }}
          onCancel={() => setShowCreate(false)}
        />
      )}

      {/* Edit wizard */}
      {editingWorkflow && (
        <WorkflowWizard
          key={editingWorkflow.id}
          projects={projects}
          installedAgentTypes={installedAgentTypes}
          agentAccess={agentAccess}
          editWorkflow={editingWorkflow}
          onDone={() => { setEditingWorkflow(null); refetch(); if (editingWorkflow) openDetail(editingWorkflow.id); }}
          onCancel={() => setEditingWorkflow(null)}
        />
      )}

      {/* Workflow list */}
      {!showCreate && !editingWorkflow && workflows.length === 0 && (
        <div className="wf-empty">
          <Zap size={32} className="text-ghost mb-4" />
          <p>{t('wf.empty')}</p>
          <p className="text-sm text-muted mt-2">
            {t('wf.emptyHint')}
          </p>
        </div>
      )}

      {!showCreate && !editingWorkflow && workflows.length > 0 && (
        <div className="flex-col gap-8" style={isMobile ? undefined : { flexDirection: 'row' }}>
          {/* List — grouped by project */}
          {!(isMobile && selectedId) && (
          <div style={{ flex: isMobile ? '1 1 auto' : '0 0 380px' }}>
            {groupedWorkflows.map(group => (
              <div key={group.key} className="mb-6">
                {/* Group header */}
                <button
                  className="wf-group-header"
                  onClick={() => toggleGroup(group.key)}
                  aria-expanded={!collapsedGroups[group.key]}
                >
                  {collapsedGroups[group.key] ? <ChevronRight size={12} /> : <ChevronDown size={12} />}
                  <span className="flex-1">{group.label}</span>
                  <span className="text-xs text-secondary">{group.workflows.length}</span>
                </button>

                {/* Workflow cards */}
                {!collapsedGroups[group.key] && group.workflows.map(wf => (
                  <div
                    key={wf.id}
                    className="wf-card"
                    data-active={selectedId === wf.id}
                    onClick={() => openDetail(wf.id)}
                  >
                    <div className="flex-row gap-4 mb-3">
                      <span className="font-semibold text-md flex-1">{wf.name}</span>
                      <button
                        className="wf-icon-btn"
                        style={{ color: wf.enabled ? '#34d399' : 'var(--kr-text-dim)' }}
                        onClick={(e) => { e.stopPropagation(); handleToggle(wf); }}
                        title={wf.enabled ? t('wf.active') : t('wf.inactive')}
                        aria-pressed={wf.enabled}
                        aria-label={wf.enabled ? t('wf.active') : t('wf.inactive')}
                      >
                        {wf.enabled ? <ToggleRight size={16} /> : <ToggleLeft size={16} />}
                      </button>
                    </div>

                    <div className="flex-row gap-4 text-sm text-muted">
                      <span className="wf-trigger-badge" data-type={wf.trigger_type}>
                        {wf.trigger_type === 'cron' && <Clock size={10} />}
                        {wf.trigger_type === 'tracker' && <GitBranch size={10} />}
                        {wf.trigger_type === 'manual' && <Zap size={10} />}
                        {TRIGGER_LABELS[wf.trigger_type] ?? wf.trigger_type}
                      </span>
                      <span>{wf.step_count} step{wf.step_count > 1 ? 's' : ''}</span>
                    </div>

                    {wf.last_run && (
                      <div className="flex-row gap-3 mt-3 text-xs">
                        <span style={{ color: STATUS_COLORS[wf.last_run.status] ?? '#888' }}>
                          {wf.last_run.status}
                        </span>
                        <span className="text-ghost">
                          {new Date(wf.last_run.started_at).toLocaleString()}
                        </span>
                        {wf.last_run.tokens_used > 0 && (
                          <span className="text-ghost">
                            · {wf.last_run.tokens_used} tokens
                          </span>
                        )}
                      </div>
                    )}

                    <div className="flex-row gap-2 mt-4">
                      <button
                        className="wf-small-btn"
                        onClick={(e) => { e.stopPropagation(); handleTrigger(wf.id); }}
                        disabled={!wf.enabled || triggering === wf.id}
                      >
                        {triggering === wf.id ? <Loader2 size={10} className="spin" /> : <Play size={10} />}
                        {t('wf.trigger')}
                      </button>
                      <button
                        className="wf-small-btn wf-small-btn-danger"
                        onClick={(e) => { e.stopPropagation(); handleDelete(wf.id); }}
                      >
                        <Trash2 size={10} /> {t('wf.delete')}
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            ))}
          </div>
          )}

          {/* Detail panel */}
          <div className="flex-1 min-w-0">
            {isMobile && selectedId && (
              <button
                className="wf-back-btn"
                onClick={() => setSelectedId(null)}
              >
                <ChevronLeft size={14} /> {t('wf.back') ?? 'Back'}
              </button>
            )}
            {selectedId && loadingDetail && (
              <div className="wf-empty" style={{ padding: 40 }}>
                <Loader2 size={24} className="text-accent wf-spin" />
              </div>
            )}

            {selectedId && !loadingDetail && detailWorkflow && (
              <WorkflowDetail
                workflow={detailWorkflow}
                runs={detailRuns}
                liveRun={liveRun?.workflowId === detailWorkflow.id ? liveRun : null}
                onTrigger={() => handleTrigger(detailWorkflow.id)}
                onRefresh={() => openDetail(detailWorkflow.id)}
                onEdit={() => setEditingWorkflow(detailWorkflow)}
                onDeleteRun={async (runId) => {
                  await workflowsApi.deleteRun(detailWorkflow.id, runId);
                  openDetail(detailWorkflow.id);
                  refetch();
                }}
                onDeleteAllRuns={async () => {
                  await workflowsApi.deleteAllRuns(detailWorkflow.id);
                  openDetail(detailWorkflow.id);
                  refetch();
                }}
                triggering={triggering === detailWorkflow.id}
                agentAccess={agentAccess}
                onNavigateToBatch={onNavigateToBatch}
              />
            )}

            {!selectedId && (
              <div className="wf-empty">
                <Eye size={24} className="text-ghost mb-4" />
                <p>{t('wf.selectOne')}</p>
              </div>
            )}
          </div>
        </div>
      )}
      </>)}

      {/* ═══ QUICK PROMPTS TAB ═══ */}
      {tab === 'quickPrompts' && (
        <div>
          {/* Create/Edit form */}
          {(showCreateQP || editingQP) && (
            <QuickPromptForm
              editPrompt={editingQP ?? undefined}
              projects={projects}
              onSave={handleSaveQP}
              onCancel={() => { setShowCreateQP(false); setEditingQP(null); }}
            />
          )}

          {/* Quick prompt list */}
          {!showCreateQP && !editingQP && (
            <>
              {(!quickPromptList || quickPromptList.length === 0) ? (
                <div className="wf-empty">
                  <p className="wf-empty-title">{t('qp.empty')}</p>
                  <p className="wf-empty-hint">{t('qp.emptyHint')}</p>
                </div>
              ) : (
                <div className="qp-list">
                  {quickPromptList.map(qp => (
                    <div key={qp.id} className="qp-card">
                      <div className="qp-card-header">
                        <span className="qp-card-icon">{qp.icon}</span>
                        <span className="qp-card-name">{qp.name}</span>
                        {qp.variables.length > 0 && (
                          <span className="qp-card-vars">{t('qp.vars', qp.variables.length)}</span>
                        )}
                        <div className="qp-card-actions">
                          <button className="wf-icon-btn" onClick={() => setEditingQP(qp)} title="Edit">
                            <Eye size={12} />
                          </button>
                          <button className="wf-icon-btn" onClick={async () => {
                            if (!confirm(t('qp.deleteConfirm'))) return;
                            await quickPromptsApi.delete(qp.id);
                            refetchQP();
                          }} title={t('qp.delete')}>
                            <Trash2 size={12} />
                          </button>
                          {/* Batch button — fans out N discussions from a list
                              of values for the first QP variable. Only meaningful
                              for QPs with at least one variable. */}
                          {qp.variables.length > 0 && (
                            <button
                              className="wf-icon-btn"
                              onClick={() => {
                                setBatchingQP(batchingQP?.id === qp.id ? null : qp);
                                setBatchInputLines('');
                              }}
                              title={t('qp.batch.launch')}
                            >
                              <Layers size={12} />
                            </button>
                          )}
                          <button
                            className="qp-launch-btn"
                            onClick={() => {
                              if (qp.variables.length === 0) {
                                // No variables — launch directly
                                setLaunchingQP(qp);
                                setLaunchVars({});
                                // Immediate launch
                                handleLaunchQP(qp);
                              } else {
                                setLaunchingQP(launchingQP?.id === qp.id ? null : qp);
                                setLaunchVars({});
                              }
                            }}
                          >
                            <Play size={12} /> {t('qp.launch')}
                          </button>
                        </div>
                      </div>

                      {/* Batch launch form — list of values, one per line,
                          each becomes the first variable for one child disc. */}
                      {batchingQP?.id === qp.id && (
                        <div className="qp-launch-form">
                          {qp.description && (
                            <p className="qp-batch-hint">{qp.description}</p>
                          )}
                          <label className="qp-launch-label">
                            {t('qp.batch.inputLabel', qp.variables[0]?.label || qp.variables[0]?.name || 'value')}
                            {qp.variables[0]?.description && (
                              <span className="qp-batch-var-desc"> — {qp.variables[0].description}</span>
                            )}
                          </label>
                          <textarea
                            className="wf-textarea"
                            rows={6}
                            value={batchInputLines}
                            onChange={e => setBatchInputLines(e.target.value)}
                            placeholder={t('qp.batch.inputPlaceholder')}
                            autoFocus
                          />
                          <p className="qp-syntax-hint">
                            {/* Same multi-separator split as handleBatchLaunch
                                so the live count matches what will actually run */}
                            {t(
                              'qp.batch.countHint',
                              new Set(
                                batchInputLines.split(/[\n,;]/).map(s => s.trim()).filter(Boolean)
                              ).size
                            )}
                          </p>
                          {/* Isolated-worktree toggle — only meaningful if the QP is project-linked */}
                          <label
                            className="flex-row gap-2 text-xs mt-2 mb-2"
                            style={{ alignItems: 'center', opacity: qp.project_id ? 1 : 0.5 }}
                            title={qp.project_id ? t('qp.batch.worktreeHint') : t('qp.batch.worktreeNoProject')}
                          >
                            <input
                              type="checkbox"
                              checked={batchIsolated}
                              disabled={!qp.project_id}
                              onChange={e => setBatchIsolated(e.target.checked)}
                            />
                            {t('qp.batch.worktree')}
                          </label>
                          <div className="flex-row gap-4">
                            <button
                              className="qp-launch-go-btn"
                              onClick={() => handleBatchLaunch(qp)}
                              disabled={batchLaunching || !batchInputLines.trim()}
                            >
                              {batchLaunching ? <Loader2 size={14} className="spin" /> : <Layers size={14} />}
                              {batchLaunching ? '...' : t('qp.batch.launch')}
                            </button>
                            <button
                              className="wf-icon-btn"
                              onClick={() => { setBatchingQP(null); setBatchInputLines(''); }}
                              title={t('qp.batch.cancel')}
                            >
                              <X size={14} />
                            </button>
                          </div>
                        </div>
                      )}

                      {/* Launch form with variable inputs */}
                      {launchingQP?.id === qp.id && qp.variables.length > 0 && (
                        <div className="qp-launch-form">
                          {qp.variables.map(v => (
                            <div key={v.name} className="qp-launch-field">
                              <label className="qp-launch-label">{v.label || v.name}</label>
                              <input
                                className="wf-input flex-1"
                                value={launchVars[v.name] ?? ''}
                                onChange={e => setLaunchVars(prev => ({ ...prev, [v.name]: e.target.value }))}
                                placeholder={v.placeholder}
                                autoFocus={qp.variables.indexOf(v) === 0}
                                onKeyDown={e => { if (e.key === 'Enter') handleLaunchQP(qp); }}
                              />
                            </div>
                          ))}
                          <button
                            className="qp-launch-go-btn"
                            onClick={() => handleLaunchQP(qp)}
                            disabled={launching}
                          >
                            {launching ? <Loader2 size={14} className="spin" /> : <Play size={14} />}
                            {launching ? '...' : t('qp.launch')}
                          </button>
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}
