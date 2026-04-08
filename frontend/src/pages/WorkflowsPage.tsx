import { useState, useRef, useMemo } from 'react';
import { useIsMobile } from '../hooks/useMediaQuery';
import { useT } from '../lib/I18nContext';
import { workflows as workflowsApi, discussions as discussionsApi, quickPrompts as quickPromptsApi } from '../lib/api';
import { useApi } from '../hooks/useApi';
import type {
  Project, WorkflowSummary, Workflow, WorkflowRun,
  AgentType, AgentsConfig, StepResult, QuickPrompt, CreateQuickPromptRequest,
} from '../types/generated';
import {
  Plus, Trash2, Play, Loader2, ChevronLeft, ChevronRight, ChevronDown,
  Clock, GitBranch, Zap, Eye,
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
  Cancelled: 'rgba(255,255,255,0.3)',
  WaitingApproval: '#c8ff00',
};

export function WorkflowsPage({ projects, installedAgentTypes, agentAccess, configLanguage, onNavigateDiscussion }: WorkflowsPageProps) {
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

  return (
    <div>
      <div className="flex-between mb-4">
        <div>
          <h1 className="wf-h1">{t('wf.title')}</h1>
        </div>
        {tab === 'workflows' ? (
        <div className="flex-row gap-3">
          {onNavigateDiscussion && (
            <button className="wf-create-ai-btn" onClick={async () => {
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
          <button className="wf-create-btn" onClick={() => setShowCreate(true)}>
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
                        style={{ color: wf.enabled ? '#34d399' : 'rgba(255,255,255,0.3)' }}
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
