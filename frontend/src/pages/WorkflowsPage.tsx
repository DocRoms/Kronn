import { useState, useRef, useEffect } from 'react';
import { useT } from '../lib/I18nContext';
import { workflows as workflowsApi, skills as skillsApi } from '../lib/api';
import { useApi } from '../hooks/useApi';
import { AGENT_COLORS, AGENT_LABELS, ALL_AGENT_TYPES, isAgentRestricted } from '../lib/constants';
import type {
  Project, WorkflowSummary, Workflow, WorkflowRun, WorkflowTrigger,
  WorkflowStep, AgentType, WorkflowSafety, AgentsConfig,
  WorkspaceConfig, StepConditionRule, StepResult,
  CreateWorkflowRequest, Skill,
} from '../types/generated';
import {
  Plus, Trash2, Play, Loader2, Check, X, ChevronRight, ChevronDown,
  Clock, GitBranch, Zap, Eye, HelpCircle, Settings, Shield,
  ToggleLeft, ToggleRight, RefreshCw, AlertTriangle,
} from 'lucide-react';

interface WorkflowsPageProps {
  projects: Project[];
  installedAgentTypes?: AgentType[];
  agentAccess?: AgentsConfig;
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

// Agent access check imported from shared constants (isAgentRestricted)
const checkAgentRestricted = isAgentRestricted;

export function WorkflowsPage({ projects, installedAgentTypes, agentAccess }: WorkflowsPageProps) {
  const { t } = useT();
  const { data: workflowList, refetch } = useApi(() => workflowsApi.list(), []);
  const [selectedId, setSelectedId] = useState<string | null>(null);
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
      console.error(e);
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
        console.error('Workflow trigger error:', error);
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
      console.error(e);
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
      console.error(e);
    }
  };

  return (
    <div>
      <style>{`
        @keyframes spin { to { transform: rotate(360deg) } }
        @keyframes pulse { 0%, 100% { opacity: 1 } 50% { opacity: 0.3 } }
      `}</style>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 20 }}>
        <div>
          <h1 style={ws.h1}>{t('wf.title')}</h1>
          <p style={ws.meta}>{t('wf.subtitle')}</p>
        </div>
        <button style={ws.createBtn} onClick={() => setShowCreate(true)}>
          <Plus size={14} /> {t('wf.new')}
        </button>
      </div>

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
        <div style={ws.empty}>
          <Zap size={32} style={{ color: 'rgba(255,255,255,0.15)', marginBottom: 8 }} />
          <p>{t('wf.empty')}</p>
          <p style={{ fontSize: 11, color: 'rgba(255,255,255,0.3)', marginTop: 4 }}>
            {t('wf.emptyHint')}
          </p>
        </div>
      )}

      {!showCreate && !editingWorkflow && workflows.length > 0 && (
        <div style={{ display: 'flex', gap: 16 }}>
          {/* List */}
          <div style={{ flex: '0 0 380px' }}>
            {workflows.map(wf => (
              <div
                key={wf.id}
                style={{
                  ...ws.card(selectedId === wf.id),
                  cursor: 'pointer',
                  marginBottom: 8,
                }}
                onClick={() => openDetail(wf.id)}
              >
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6 }}>
                  <span style={{ fontWeight: 600, fontSize: 13, flex: 1 }}>{wf.name}</span>
                  <button
                    style={{ ...ws.iconBtn, color: wf.enabled ? '#34d399' : 'rgba(255,255,255,0.3)' }}
                    onClick={(e) => { e.stopPropagation(); handleToggle(wf); }}
                    title={wf.enabled ? t('wf.active') : t('wf.inactive')}
                  >
                    {wf.enabled ? <ToggleRight size={16} /> : <ToggleLeft size={16} />}
                  </button>
                </div>

                <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 11, color: 'rgba(255,255,255,0.4)' }}>
                  <span style={ws.triggerBadge(wf.trigger_type)}>
                    {wf.trigger_type === 'cron' && <Clock size={10} />}
                    {wf.trigger_type === 'tracker' && <GitBranch size={10} />}
                    {wf.trigger_type === 'manual' && <Zap size={10} />}
                    {TRIGGER_LABELS[wf.trigger_type] ?? wf.trigger_type}
                  </span>
                  <span>{wf.step_count} step{wf.step_count > 1 ? 's' : ''}</span>
                  {wf.project_name && <span>· {wf.project_name}</span>}
                </div>

                {wf.last_run && (
                  <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginTop: 6, fontSize: 10 }}>
                    <span style={{ color: STATUS_COLORS[wf.last_run.status] ?? '#888' }}>
                      {wf.last_run.status}
                    </span>
                    <span style={{ color: 'rgba(255,255,255,0.2)' }}>
                      {new Date(wf.last_run.started_at).toLocaleString()}
                    </span>
                    {wf.last_run.tokens_used > 0 && (
                      <span style={{ color: 'rgba(255,255,255,0.2)' }}>
                        · {wf.last_run.tokens_used} tokens
                      </span>
                    )}
                  </div>
                )}

                <div style={{ display: 'flex', gap: 4, marginTop: 8 }}>
                  <button
                    style={ws.smallBtn}
                    onClick={(e) => { e.stopPropagation(); handleTrigger(wf.id); }}
                    disabled={!wf.enabled || triggering === wf.id}
                  >
                    {triggering === wf.id ? <Loader2 size={10} className="spin" /> : <Play size={10} />}
                    {t('wf.trigger')}
                  </button>
                  <button
                    style={{ ...ws.smallBtn, color: '#ff4d6a', borderColor: 'rgba(255,77,106,0.2)' }}
                    onClick={(e) => { e.stopPropagation(); handleDelete(wf.id); }}
                  >
                    <Trash2 size={10} /> {t('wf.delete')}
                  </button>
                </div>
              </div>
            ))}
          </div>

          {/* Detail panel */}
          <div style={{ flex: 1, minWidth: 0 }}>
            {selectedId && loadingDetail && (
              <div style={{ ...ws.empty, padding: 40 }}>
                <Loader2 size={24} style={{ color: '#c8ff00', animation: 'spin 1s linear infinite' }} />
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
              <div style={ws.empty}>
                <Eye size={24} style={{ color: 'rgba(255,255,255,0.15)', marginBottom: 8 }} />
                <p>{t('wf.selectOne')}</p>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// ─── Workflow Detail ─────────────────────────────────────────────────────────

interface LiveRunState {
  workflowId: string;
  currentStep: string | null;
  stepIndex: number;
  totalSteps: number;
  completedSteps: StepResult[];
  finished: boolean;
  status: string | null;
}

function WorkflowDetail({ workflow, runs, liveRun, onTrigger, onRefresh, onEdit, onDeleteRun, onDeleteAllRuns, triggering, agentAccess }: {
  workflow: Workflow;
  runs: WorkflowRun[];
  liveRun: LiveRunState | null;
  onTrigger: () => void;
  onRefresh: () => void;
  onEdit: () => void;
  onDeleteRun: (runId: string) => void;
  onDeleteAllRuns: () => void;
  triggering: boolean;
  agentAccess?: AgentsConfig;
}) {
  const { t } = useT();
  const [showRuns, setShowRuns] = useState(true);

  const triggerLabel = (() => {
    switch (workflow.trigger.type) {
      case 'Cron': return `Cron: ${workflow.trigger.schedule}`;
      case 'Tracker': return `Tracker: ${(workflow.trigger.source as any)?.owner}/${(workflow.trigger.source as any)?.repo}`;
      case 'Manual': return t('wf.manual');
      default: return t('wf.unknown');
    }
  })();

  return (
    <div style={ws.detailPanel}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 16 }}>
        <h2 style={{ fontSize: 16, fontWeight: 700, flex: 1, margin: 0 }}>{workflow.name}</h2>
        <button style={ws.smallBtn} onClick={onEdit}>
          <Settings size={10} /> {t('wf.edit')}
        </button>
        <button style={ws.smallBtn} onClick={onRefresh}>
          <RefreshCw size={10} /> {t('wf.refresh')}
        </button>
        <button
          style={{ ...ws.smallBtn, background: 'rgba(200,255,0,0.08)' }}
          onClick={onTrigger}
          disabled={!workflow.enabled || triggering}
        >
          {triggering ? <Loader2 size={10} /> : <Play size={10} />}
          {t('wf.launch')}
        </button>
      </div>

      {/* Trigger info */}
      <div style={ws.infoRow}>
        <span style={ws.infoLabel}>Trigger</span>
        <span>{triggerLabel}</span>
      </div>

      {workflow.concurrency_limit && (
        <div style={ws.infoRow}>
          <span style={ws.infoLabel}>Concurrence max</span>
          <span>{workflow.concurrency_limit}</span>
        </div>
      )}

      {/* Steps */}
      <h3 style={ws.sectionTitle}>Steps ({workflow.steps.length})</h3>
      {workflow.steps.map((step, i) => (
        <div key={i} style={ws.stepCard}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{
              width: 20, height: 20, borderRadius: '50%', display: 'flex', alignItems: 'center', justifyContent: 'center',
              fontSize: 10, fontWeight: 700, background: 'rgba(200,255,0,0.1)', color: '#c8ff00',
            }}>{i + 1}</span>
            <span style={{ fontWeight: 600, fontSize: 13 }}>{step.name}</span>
            <span style={{ fontSize: 10, color: AGENT_COLORS[step.agent] ?? '#888', fontWeight: 600 }}>
              {AGENT_LABELS[step.agent] ?? step.agent}
            </span>
            {checkAgentRestricted(agentAccess, step.agent) && (
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3, fontSize: 10, color: '#ffc800' }}>
                <AlertTriangle size={10} />
                {t('config.restrictedStep')}
              </span>
            )}
          </div>
          <div style={{ marginTop: 6, fontSize: 11, color: 'rgba(255,255,255,0.4)', fontFamily: 'monospace', whiteSpace: 'pre-wrap', maxHeight: 60, overflow: 'hidden' }}>
            {step.prompt_template.slice(0, 200)}{step.prompt_template.length > 200 ? '...' : ''}
          </div>
          {step.on_result && step.on_result.length > 0 && (
            <div style={{ marginTop: 4, fontSize: 10, color: '#ffc800' }}>
              {step.on_result.map((r, j) => (
                <span key={j}>{t('wiz.ifContains')} "{r.contains}" → {r.action.type} </span>
              ))}
            </div>
          )}
        </div>
      ))}

      {/* Live run progress */}
      {liveRun && liveRun.workflowId === workflow.id && !liveRun.finished && (
        <div style={{
          marginTop: 16, padding: '12px 14px', borderRadius: 8,
          background: 'linear-gradient(135deg, rgba(0,212,255,0.04), rgba(200,255,0,0.04))',
          border: '1px solid rgba(0,212,255,0.15)',
        }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 10 }}>
            <Loader2 size={12} style={{ color: '#00d4ff', animation: 'spin 1s linear infinite' }} />
            <span style={{ fontSize: 12, fontWeight: 700, color: '#00d4ff' }}>
              {t('wf.running')}
            </span>
            {liveRun.totalSteps > 0 && (
              <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.4)' }}>
                ({liveRun.completedSteps.length}/{liveRun.totalSteps} steps)
              </span>
            )}
          </div>

          {/* Progress bar */}
          {liveRun.totalSteps > 0 && (
            <div style={{
              width: '100%', height: 3, borderRadius: 2,
              background: 'rgba(255,255,255,0.06)', marginBottom: 10, overflow: 'hidden',
            }}>
              <div style={{
                height: '100%', borderRadius: 2,
                background: 'linear-gradient(90deg, #00d4ff, #c8ff00)',
                width: `${(liveRun.completedSteps.length / liveRun.totalSteps) * 100}%`,
                transition: 'width 0.5s ease',
              }} />
            </div>
          )}

          {/* Step indicators */}
          {workflow.steps.map((step, i) => {
            const completed = liveRun.completedSteps.find(s => s.step_name === step.name);
            const isCurrent = liveRun.currentStep === step.name;
            const isPending = !completed && !isCurrent;

            return (
              <div key={i} style={{
                display: 'flex', alignItems: 'center', gap: 8,
                padding: '5px 8px', marginTop: 2, borderRadius: 6,
                background: isCurrent ? 'rgba(0,212,255,0.06)' : 'transparent',
                transition: 'background 0.3s',
              }}>
                {/* Status icon */}
                {completed ? (
                  <span style={{
                    width: 16, height: 16, borderRadius: '50%', display: 'flex',
                    alignItems: 'center', justifyContent: 'center', flexShrink: 0,
                    background: completed.status === 'Success' ? 'rgba(52,211,153,0.15)' : 'rgba(255,77,106,0.15)',
                  }}>
                    {completed.status === 'Success'
                      ? <Check size={9} style={{ color: '#34d399' }} />
                      : <X size={9} style={{ color: '#ff4d6a' }} />
                    }
                  </span>
                ) : isCurrent ? (
                  <span style={{
                    width: 16, height: 16, borderRadius: '50%', display: 'flex',
                    alignItems: 'center', justifyContent: 'center', flexShrink: 0,
                    background: 'rgba(0,212,255,0.15)',
                  }}>
                    <span style={{
                      width: 6, height: 6, borderRadius: '50%', background: '#00d4ff',
                      animation: 'pulse 1.5s ease-in-out infinite',
                    }} />
                  </span>
                ) : (
                  <span style={{
                    width: 16, height: 16, borderRadius: '50%', display: 'flex',
                    alignItems: 'center', justifyContent: 'center', flexShrink: 0,
                    background: 'rgba(255,255,255,0.04)',
                  }}>
                    <span style={{
                      width: 4, height: 4, borderRadius: '50%',
                      background: 'rgba(255,255,255,0.15)',
                    }} />
                  </span>
                )}

                {/* Step name */}
                <span style={{
                  fontSize: 11, fontWeight: isCurrent ? 600 : 500, flex: 1,
                  color: completed ? 'rgba(255,255,255,0.5)' : isCurrent ? '#fff' : 'rgba(255,255,255,0.25)',
                }}>
                  {step.name}
                </span>

                {/* Duration for completed */}
                {completed && completed.duration_ms > 0 && (
                  <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.2)' }}>
                    {(completed.duration_ms / 1000).toFixed(1)}s
                  </span>
                )}

                {/* Current step indicator */}
                {isCurrent && (
                  <span style={{
                    fontSize: 9, color: '#00d4ff', fontWeight: 600,
                    animation: 'pulse 1.5s ease-in-out infinite',
                  }}>
                    {t('wf.inProgress')}
                  </span>
                )}

                {isPending && (
                  <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.1)' }}>
                    {t('wf.pending')}
                  </span>
                )}
              </div>
            );
          })}
        </div>
      )}

      {/* Live run finished banner */}
      {liveRun && liveRun.workflowId === workflow.id && liveRun.finished && (
        <div style={{
          marginTop: 16, padding: '10px 14px', borderRadius: 8,
          background: liveRun.status === 'Success' ? 'rgba(52,211,153,0.06)' : 'rgba(255,77,106,0.06)',
          border: `1px solid ${liveRun.status === 'Success' ? 'rgba(52,211,153,0.2)' : 'rgba(255,77,106,0.2)'}`,
          display: 'flex', alignItems: 'center', gap: 8,
        }}>
          {liveRun.status === 'Success'
            ? <Check size={12} style={{ color: '#34d399' }} />
            : <X size={12} style={{ color: '#ff4d6a' }} />
          }
          <span style={{
            fontSize: 12, fontWeight: 600,
            color: liveRun.status === 'Success' ? '#34d399' : '#ff4d6a',
          }}>
            {t('wf.runDone', liveRun.status ?? '')}
          </span>
          <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)' }}>
            {t('wf.stepsExecuted', liveRun.completedSteps.length)}
          </span>
        </div>
      )}

      {/* Runs */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 16 }}>
        <h3 style={{ ...ws.sectionTitle, margin: 0, flex: 1 }}>
          Runs ({runs.length})
        </h3>
        {runs.length > 0 && (
          <button
            style={{ ...ws.smallBtn, color: '#ff4d6a', background: 'rgba(255,77,106,0.06)', border: '1px solid rgba(255,77,106,0.15)' }}
            onClick={onDeleteAllRuns}
          >
            <Trash2 size={9} /> {t('wf.deleteAll')}
          </button>
        )}
        <button style={ws.iconBtn} onClick={() => setShowRuns(!showRuns)}>
          <ChevronRight size={12} style={{ transform: showRuns ? 'rotate(90deg)' : 'none', transition: 'transform 0.15s' }} />
        </button>
      </div>

      {showRuns && runs.length === 0 && (
        <p style={{ fontSize: 11, color: 'rgba(255,255,255,0.25)', marginTop: 8 }}>{t('wf.noRuns')}</p>
      )}

      {showRuns && runs.map(run => (
        <RunDetail key={run.id} run={run} onDelete={() => onDeleteRun(run.id)} />
      ))}
    </div>
  );
}

// ─── Run Detail (expandable steps) ───────────────────────────────────────────

function RunDetail({ run, onDelete }: { run: WorkflowRun; onDelete: () => void }) {
  const { t } = useT();
  const [expandedStep, setExpandedStep] = useState<number | null>(null);

  const CONDITION_LABELS: Record<string, string> = {
    Stop: 'Stop',
    Skip: t('wf.skipNext'),
  };

  const conditionLabel = (cr: string): string => {
    if (cr.startsWith('Goto:')) return `Goto → ${cr.slice(5)}`;
    return CONDITION_LABELS[cr] ?? cr;
  };

  return (
    <div style={ws.runCard}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <span style={{
          width: 8, height: 8, borderRadius: '50%',
          background: STATUS_COLORS[run.status] ?? '#888',
        }} />
        <span style={{ fontWeight: 600, fontSize: 12, color: STATUS_COLORS[run.status] ?? '#888' }}>
          {run.status}
        </span>
        <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', flex: 1 }}>
          {new Date(run.started_at).toLocaleString()}
          {run.finished_at && ` — ${new Date(run.finished_at).toLocaleString()}`}
        </span>
        {run.tokens_used > 0 && (
          <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)' }}>{run.tokens_used} tokens</span>
        )}
        <button
          style={{
            background: 'none', border: 'none', cursor: 'pointer', padding: 2,
            color: 'rgba(255,255,255,0.2)', display: 'flex', alignItems: 'center',
          }}
          onClick={(e) => { e.stopPropagation(); onDelete(); }}
          title={t('wf.deleteRun')}
        >
          <Trash2 size={10} />
        </button>
      </div>

      {/* Step results */}
      {run.step_results.length > 0 && (
        <div style={{ marginTop: 8 }}>
          {run.step_results.map((sr, i) => {
            const isExpanded = expandedStep === i;
            return (
              <div key={i} style={{
                marginTop: 4, borderRadius: 6,
                border: isExpanded ? '1px solid rgba(255,255,255,0.08)' : '1px solid transparent',
                background: isExpanded ? 'rgba(255,255,255,0.02)' : 'transparent',
              }}>
                <div
                  style={{
                    display: 'flex', alignItems: 'center', gap: 6,
                    padding: isExpanded ? '6px 8px' : '2px 0',
                    fontSize: 11, cursor: 'pointer',
                  }}
                  onClick={() => setExpandedStep(isExpanded ? null : i)}
                >
                  <span style={{
                    width: 6, height: 6, borderRadius: '50%', flexShrink: 0,
                    background: STATUS_COLORS[sr.status] ?? '#888',
                  }} />
                  <span style={{ fontWeight: 600 }}>{sr.step_name}</span>
                  <span style={{ color: 'rgba(255,255,255,0.25)' }}>
                    {sr.duration_ms > 0 ? `${(sr.duration_ms / 1000).toFixed(1)}s` : ''}
                  </span>
                  {sr.condition_result && (
                    <span style={{
                      fontSize: 9, padding: '1px 6px', borderRadius: 4, fontWeight: 600,
                      background: sr.condition_result === 'Stop' ? 'rgba(255,200,0,0.1)' : 'rgba(0,212,255,0.1)',
                      color: sr.condition_result === 'Stop' ? '#ffc800' : '#00d4ff',
                      border: `1px solid ${sr.condition_result === 'Stop' ? 'rgba(255,200,0,0.2)' : 'rgba(0,212,255,0.2)'}`,
                    }}>
                      {conditionLabel(sr.condition_result)}
                    </span>
                  )}
                  {!sr.condition_result && sr.status === 'Success' && i < run.step_results.length - 1 && (
                    <span style={{ fontSize: 9, color: 'rgba(52,211,153,0.5)' }}>→ suite</span>
                  )}
                  <span style={{ flex: 1 }} />
                  {!isExpanded && sr.output && (
                    <span style={{
                      color: 'rgba(255,255,255,0.15)', fontFamily: 'monospace', fontSize: 10,
                      maxWidth: 200, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                    }}>
                      {sr.output.slice(0, 80)}
                    </span>
                  )}
                  <ChevronRight size={10} style={{
                    color: 'rgba(255,255,255,0.2)', flexShrink: 0,
                    transform: isExpanded ? 'rotate(90deg)' : 'none',
                    transition: 'transform 0.15s',
                  }} />
                </div>

                {isExpanded && (
                  <div style={{
                    padding: '8px 10px', borderTop: '1px solid rgba(255,255,255,0.06)',
                  }}>
                    <div style={{
                      padding: '10px 12px', borderRadius: 6,
                      background: 'rgba(0,0,0,0.3)', border: '1px solid rgba(255,255,255,0.06)',
                      fontFamily: 'monospace', fontSize: 11, lineHeight: 1.6,
                      color: 'rgba(255,255,255,0.6)', whiteSpace: 'pre-wrap',
                      maxHeight: 400, overflowY: 'auto',
                      wordBreak: 'break-word',
                    }}>
                      {sr.output || t('wf.noOutput')}
                    </div>
                    <div style={{ display: 'flex', gap: 12, marginTop: 6, fontSize: 10, color: 'rgba(255,255,255,0.25)' }}>
                      <span>{t('wf.status')}: <span style={{ color: STATUS_COLORS[sr.status] ?? '#888' }}>{sr.status}</span></span>
                      {sr.duration_ms > 0 && <span>{t('wf.duration')}: {(sr.duration_ms / 1000).toFixed(1)}s</span>}
                      {sr.tokens_used > 0 && <span>Tokens: {sr.tokens_used}</span>}
                      {sr.condition_result && <span>Condition: <span style={{ color: '#ffc800' }}>{conditionLabel(sr.condition_result)}</span></span>}
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

// ─── Workflow Wizard (Create + Edit) ─────────────────────────────────────────

/** Parse a cron expression back into visual builder values */
function parseCronExpr(expr: string): { every: number; unit: 'minutes' | 'hours' | 'days' | 'weeks' | 'months'; at: string } {
  const parts = expr.split(' ');
  if (parts.length !== 5) return { every: 5, unit: 'minutes', at: '00:00' };
  const [min, hour, dom, , ] = parts;
  if (min.startsWith('*/')) return { every: parseInt(min.slice(2)) || 5, unit: 'minutes', at: '00:00' };
  if (hour.startsWith('*/')) return { every: parseInt(hour.slice(2)) || 1, unit: 'hours', at: `00:${min.padStart(2, '0')}` };
  if (dom.startsWith('*/')) return { every: parseInt(dom.slice(2)) || 1, unit: 'days', at: `${hour.padStart(2, '0')}:${min.padStart(2, '0')}` };
  return { every: 1, unit: 'days', at: `${hour.padStart(2, '0')}:${min.padStart(2, '0')}` };
}

function WorkflowWizard({ projects, editWorkflow, onDone, onCancel, installedAgentTypes, agentAccess }: {
  projects: Project[];
  editWorkflow?: Workflow;
  onDone: () => void;
  onCancel: () => void;
  installedAgentTypes?: AgentType[];
  agentAccess?: AgentsConfig;
}) {
  const { t } = useT();
  const availableAgents = (installedAgentTypes && installedAgentTypes.length > 0
    ? installedAgentTypes
    : ALL_AGENT_TYPES
  ).map(at => ({ type: at, label: AGENT_LABELS[at] ?? at }));
  const isEdit = !!editWorkflow;
  const initTrigger = editWorkflow?.trigger;
  const initCron = initTrigger?.type === 'Cron' ? parseCronExpr(initTrigger.schedule) : null;
  const initTracker = initTrigger?.type === 'Tracker' ? initTrigger : null;

  const [wizardStep, setWizardStep] = useState(0);
  const [name, setName] = useState(editWorkflow?.name ?? '');
  const [projectId, setProjectId] = useState<string>(editWorkflow?.project_id ?? '');
  const [triggerType, setTriggerType] = useState<'Cron' | 'Tracker' | 'Manual'>(initTrigger?.type ?? 'Manual');
  const [cronEvery, setCronEvery] = useState(initCron?.every ?? 5);
  const [cronUnit, setCronUnit] = useState<'minutes' | 'hours' | 'days' | 'weeks' | 'months'>(initCron?.unit ?? 'minutes');
  const [cronAt, setCronAt] = useState(initCron?.at ?? '00:00');
  const [trackerOwner, setTrackerOwner] = useState((initTracker?.source as any)?.owner ?? '');
  const [trackerRepo, setTrackerRepo] = useState((initTracker?.source as any)?.repo ?? '');
  const [trackerLabels, setTrackerLabels] = useState(initTracker?.labels?.join(', ') ?? '');
  const [trackerInterval, setTrackerInterval] = useState(initTracker?.interval ?? '*/5 * * * *');
  const [showVarHelp, setShowVarHelp] = useState(false);
  const [expandedStepAdvanced, setExpandedStepAdvanced] = useState<number | null>(null);

  // Safety state
  const [safety, setSafety] = useState<WorkflowSafety>(editWorkflow?.safety ?? {
    sandbox: false, max_files: null, max_lines: null, require_approval: false,
  });

  // Workspace config state
  const initHooks = editWorkflow?.workspace_config?.hooks;
  const [wsHookAfterCreate, setWsHookAfterCreate] = useState(initHooks?.after_create ?? '');
  const [wsHookBeforeRun, setWsHookBeforeRun] = useState(initHooks?.before_run ?? '');
  const [wsHookAfterRun, setWsHookAfterRun] = useState(initHooks?.after_run ?? '');
  const [wsHookBeforeRemove, setWsHookBeforeRemove] = useState(initHooks?.before_remove ?? '');

  // Concurrency
  const [concurrencyLimit, setConcurrencyLimit] = useState<string>(editWorkflow?.concurrency_limit?.toString() ?? '');

  // Build cron expression from visual inputs
  const buildCronExpr = (): string => {
    const [hh, mm] = cronAt.split(':').map(Number);
    const h = isNaN(hh) ? 0 : hh;
    const m = isNaN(mm) ? 0 : mm;
    switch (cronUnit) {
      case 'minutes': return `*/${cronEvery} * * * *`;
      case 'hours':   return `${m} */${cronEvery} * * *`;
      case 'days':    return `${m} ${h} */${cronEvery} * *`;
      case 'weeks':   return `${m} ${h} * * 1`;
      case 'months':  return `${m} ${h} 1 */${cronEvery} *`;
      default:        return '*/5 * * * *';
    }
  };

  const cronHumanLabel = (): string => {
    const [hh, mm] = cronAt.split(':');
    const atStr = `${hh ?? '00'}:${mm ?? '00'}`;
    const unitLabels: Record<string, string> = { minutes: t('wiz.minutes'), hours: t('wiz.hours'), days: t('wiz.days'), weeks: t('wiz.weeks'), months: t('wiz.months') };
    if (cronUnit === 'minutes' || cronUnit === 'hours') {
      return `${t('wiz.every')} ${cronEvery} ${unitLabels[cronUnit]}`;
    }
    return `${t('wiz.every')} ${cronEvery} ${unitLabels[cronUnit]} ${t('wiz.at')} ${atStr}`;
  };

  const [steps, setSteps] = useState<WorkflowStep[]>(editWorkflow?.steps ?? [{
    name: 'main',
    agent: 'ClaudeCode',
    prompt_template: '',
    mode: { type: 'Normal' },
  }]);
  const [saving, setSaving] = useState(false);
  const [availableSkills, setAvailableSkills] = useState<Skill[]>([]);

  useEffect(() => {
    skillsApi.list().then(setAvailableSkills).catch(() => {});
  }, []);

  const addStep = () => {
    setSteps([...steps, {
      name: `step-${steps.length + 1}`,
      agent: 'ClaudeCode',
      prompt_template: '',
      mode: { type: 'Normal' },
    }]);
  };

  const updateStep = (idx: number, patch: Partial<WorkflowStep>) => {
    setSteps(steps.map((s, i) => i === idx ? { ...s, ...patch } : s));
  };

  const removeStep = (idx: number) => {
    if (steps.length > 1) setSteps(steps.filter((_, i) => i !== idx));
  };

  // --- on_result helpers ---
  const addCondition = (stepIdx: number) => {
    const step = steps[stepIdx];
    const conditions: StepConditionRule[] = [...(step.on_result ?? []), { contains: '', action: { type: 'Stop' } }];
    updateStep(stepIdx, { on_result: conditions });
  };

  const updateCondition = (stepIdx: number, condIdx: number, patch: Partial<StepConditionRule>) => {
    const step = steps[stepIdx];
    const conditions = (step.on_result ?? []).map((c, i) => i === condIdx ? { ...c, ...patch } : c);
    updateStep(stepIdx, { on_result: conditions });
  };

  const removeCondition = (stepIdx: number, condIdx: number) => {
    const step = steps[stepIdx];
    updateStep(stepIdx, { on_result: (step.on_result ?? []).filter((_, i) => i !== condIdx) });
  };

  const buildTrigger = (): WorkflowTrigger => {
    switch (triggerType) {
      case 'Cron': return { type: 'Cron', schedule: buildCronExpr() };
      case 'Tracker': return {
        type: 'Tracker',
        source: { type: 'GitHub', owner: trackerOwner, repo: trackerRepo },
        query: '',
        labels: trackerLabels.split(',').map(l => l.trim()).filter(Boolean),
        interval: trackerInterval,
      };
      case 'Manual': return { type: 'Manual' };
    }
  };

  const buildWorkspaceConfig = (): WorkspaceConfig | null => {
    const hasHooks = wsHookAfterCreate || wsHookBeforeRun || wsHookAfterRun || wsHookBeforeRemove;
    if (!hasHooks) return null;
    return {
      hooks: {
        after_create: wsHookAfterCreate || null,
        before_run: wsHookBeforeRun || null,
        after_run: wsHookAfterRun || null,
        before_remove: wsHookBeforeRemove || null,
      }
    };
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      const trigger = buildTrigger();
      const wsConfig = buildWorkspaceConfig();
      const safetyVal = (safety.sandbox || safety.require_approval || safety.max_files || safety.max_lines) ? safety : undefined;
      const concurrency = concurrencyLimit ? parseInt(concurrencyLimit) : undefined;

      if (isEdit && editWorkflow) {
        await workflowsApi.update(editWorkflow.id, {
          name,
          trigger,
          steps,
          actions: [],
          safety: safetyVal ?? editWorkflow.safety,
          workspace_config: wsConfig ?? undefined,
          concurrency_limit: concurrency ?? null,
        });
      } else {
        const req: CreateWorkflowRequest = {
          name,
          project_id: projectId || null,
          trigger,
          steps,
          actions: [],
          safety: safetyVal,
          workspace_config: wsConfig ?? undefined,
          concurrency_limit: concurrency,
        };
        await workflowsApi.create(req);
      }
      onDone();
    } catch (e) {
      console.error(e);
    } finally {
      setSaving(false);
    }
  };

  const WIZARD_STEPS = [t('wiz.infos'), t('wiz.trigger'), t('wiz.steps'), t('wiz.config'), t('wiz.summary')];
  const lastStep = WIZARD_STEPS.length - 1;

  return (
    <div style={ws.wizardCard}>
      {/* Progress bar */}
      <div style={{ display: 'flex', gap: 4, marginBottom: 20 }}>
        {WIZARD_STEPS.map((label, i) => (
          <div key={i} style={{ flex: 1, textAlign: 'center' }}>
            <div style={{
              height: 3, borderRadius: 2, marginBottom: 4,
              background: i <= wizardStep ? '#c8ff00' : 'rgba(255,255,255,0.08)',
            }} />
            <span style={{ fontSize: 10, color: i <= wizardStep ? '#c8ff00' : 'rgba(255,255,255,0.3)' }}>
              {label}
            </span>
          </div>
        ))}
      </div>

      {/* Step 0: Name + Project */}
      {wizardStep === 0 && (
        <div>
          <label style={ws.label}>{t('wiz.name')}</label>
          <input
            style={ws.input}
            value={name}
            onChange={e => setName(e.target.value)}
            placeholder={t('wiz.namePlaceholder')}
          />

          <label style={{ ...ws.label, marginTop: 12 }}>{isEdit ? t('wiz.project') : t('wiz.projectOptional')}</label>
          <select style={ws.select} value={projectId} onChange={e => {
            const pid = e.target.value;
            setProjectId(pid);
            const proj = projects.find(p => p.id === pid);
            if (proj?.default_skill_ids?.length) {
              setSteps(prev => prev.map(s => ({ ...s, skill_ids: s.skill_ids?.length ? s.skill_ids : proj.default_skill_ids })));
            }
          }} disabled={isEdit}>
            <option value="">{t('wiz.noProject')}</option>
            {projects.map(p => (
              <option key={p.id} value={p.id}>{p.name}</option>
            ))}
          </select>
        </div>
      )}

      {/* Step 1: Trigger */}
      {wizardStep === 1 && (
        <div>
          <label style={ws.label}>{t('wiz.triggerType')}</label>
          <div style={{ display: 'flex', gap: 8, marginBottom: 12 }}>
            {(['Manual', 'Cron', 'Tracker'] as const).map(tt => (
              <button
                key={tt}
                style={{
                  ...ws.triggerBtn,
                  background: triggerType === tt ? 'rgba(200,255,0,0.1)' : 'rgba(255,255,255,0.03)',
                  borderColor: triggerType === tt ? 'rgba(200,255,0,0.3)' : 'rgba(255,255,255,0.08)',
                  color: triggerType === tt ? '#c8ff00' : 'rgba(255,255,255,0.4)',
                }}
                onClick={() => setTriggerType(tt)}
              >
                {tt === 'Manual' && <Zap size={12} />}
                {tt === 'Cron' && <Clock size={12} />}
                {tt === 'Tracker' && <GitBranch size={12} />}
                {tt}
              </button>
            ))}
          </div>

          {triggerType === 'Cron' && (
            <>
              <label style={ws.label}>{t('wiz.frequency')}</label>
              <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
                <span style={{ fontSize: 12, color: 'rgba(255,255,255,0.5)' }}>{t('wiz.every')}</span>
                <input
                  type="number" min={1} max={60}
                  style={{ ...ws.input, width: 60, textAlign: 'center' as const }}
                  value={cronEvery}
                  onChange={e => setCronEvery(Math.max(1, parseInt(e.target.value) || 1))}
                />
                <select
                  style={{ ...ws.select, width: 130 }}
                  value={cronUnit}
                  onChange={e => setCronUnit(e.target.value as typeof cronUnit)}
                >
                  <option value="minutes">{t('wiz.minutes')}</option>
                  <option value="hours">{t('wiz.hours')}</option>
                  <option value="days">{t('wiz.days')}</option>
                  <option value="weeks">{t('wiz.weeks')}</option>
                  <option value="months">{t('wiz.months')}</option>
                </select>
                {(cronUnit === 'days' || cronUnit === 'weeks' || cronUnit === 'months') && (
                  <>
                    <span style={{ fontSize: 12, color: 'rgba(255,255,255,0.5)' }}>{t('wiz.at')}</span>
                    <input
                      type="time"
                      style={{ ...ws.input, width: 100 }}
                      value={cronAt}
                      onChange={e => setCronAt(e.target.value)}
                    />
                  </>
                )}
              </div>
              <div style={{
                display: 'flex', alignItems: 'center', gap: 8, padding: '8px 12px',
                borderRadius: 6, background: 'rgba(200,255,0,0.04)', border: '1px solid rgba(200,255,0,0.1)',
              }}>
                <Clock size={12} style={{ color: '#c8ff00', flexShrink: 0 }} />
                <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.5)' }}>{cronHumanLabel()}</span>
                <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.2)', fontFamily: 'monospace', marginLeft: 'auto' }}>
                  {buildCronExpr()}
                </span>
              </div>
            </>
          )}

          {triggerType === 'Tracker' && (
            <>
              <div style={{ display: 'flex', gap: 8 }}>
                <div style={{ flex: 1 }}>
                  <label style={ws.label}>Owner</label>
                  <input style={ws.input} value={trackerOwner} onChange={e => setTrackerOwner(e.target.value)} placeholder="owner" />
                </div>
                <div style={{ flex: 1 }}>
                  <label style={ws.label}>Repo</label>
                  <input style={ws.input} value={trackerRepo} onChange={e => setTrackerRepo(e.target.value)} placeholder="repo" />
                </div>
              </div>
              <label style={{ ...ws.label, marginTop: 8 }}>{t('wiz.labels')}</label>
              <input style={ws.input} value={trackerLabels} onChange={e => setTrackerLabels(e.target.value)} placeholder="bug-5xx, auto-fix" />
              <label style={{ ...ws.label, marginTop: 8 }}>{t('wiz.pollInterval')}</label>
              <input style={ws.input} value={trackerInterval} onChange={e => setTrackerInterval(e.target.value)} placeholder="*/5 * * * *" />
            </>
          )}
        </div>
      )}

      {/* Step 2: Steps (with advanced per-step config) */}
      {wizardStep === 2 && (
        <div>
          {/* Variable help toggle */}
          <button
            style={{
              ...ws.smallHelpBtn,
              marginBottom: 12,
              background: showVarHelp ? 'rgba(200,255,0,0.06)' : 'rgba(255,255,255,0.03)',
              borderColor: showVarHelp ? 'rgba(200,255,0,0.2)' : 'rgba(255,255,255,0.08)',
              color: showVarHelp ? '#c8ff00' : 'rgba(255,255,255,0.4)',
            }}
            onClick={() => setShowVarHelp(!showVarHelp)}
          >
            <HelpCircle size={12} />
            {t('wiz.availableVars')}
            <ChevronRight size={10} style={{ transform: showVarHelp ? 'rotate(90deg)' : 'none', transition: 'transform 0.15s' }} />
          </button>

          {showVarHelp && (
            <div style={ws.helpPanel}>
              <div style={ws.helpSection}>
                <div style={ws.helpTitle}>{t('wiz.triggerVars')}</div>
                <div style={ws.helpGrid}>
                  {[
                    ['{{issue.title}}', t('wiz.issueTitle')],
                    ['{{issue.body}}', t('wiz.issueBody')],
                    ['{{issue.number}}', t('wiz.issueNumber')],
                    ['{{issue.url}}', t('wiz.issueUrl')],
                    ['{{issue.labels}}', t('wiz.issueLabels')],
                  ].map(([v, d]) => (
                    <div key={v} style={ws.helpRow}>
                      <code style={ws.helpCode}>{v}</code>
                      <span style={ws.helpDesc}>{d}</span>
                    </div>
                  ))}
                </div>
              </div>

              <div style={ws.helpSection}>
                <div style={ws.helpTitle}>{t('wiz.stepChaining')}</div>
                <div style={ws.helpGrid}>
                  {[
                    ['{{previous_step.output}}', t('wiz.prevOutput')],
                    ['{{steps.<nom>.output}}', t('wiz.namedOutput')],
                  ].map(([v, d]) => (
                    <div key={v} style={ws.helpRow}>
                      <code style={ws.helpCode}>{v}</code>
                      <span style={ws.helpDesc}>{d}</span>
                    </div>
                  ))}
                </div>
              </div>

              <div style={ws.helpSection}>
                <div style={ws.helpTitle}>{t('wiz.example')}</div>
                <div style={{
                  padding: '10px 12px', borderRadius: 6,
                  background: 'rgba(0,0,0,0.3)', border: '1px solid rgba(255,255,255,0.06)',
                  fontSize: 11, fontFamily: 'monospace', lineHeight: 1.6, color: 'rgba(255,255,255,0.6)',
                }}>
                  <div><span style={{ color: 'rgba(255,255,255,0.3)' }}>{'// Step 1 : "analyze"'}</span></div>
                  <div>Analyse le bug <span style={{ color: '#c8ff00' }}>{'{{issue.title}}'}</span> dans <span style={{ color: '#c8ff00' }}>{'{{issue.url}}'}</span>.</div>
                  <div>Trouve la cause racine.</div>
                  <div style={{ height: 8 }} />
                  <div><span style={{ color: 'rgba(255,255,255,0.3)' }}>{'// Step 2 : "fix"'}</span></div>
                  <div>Analyse : <span style={{ color: '#c8ff00' }}>{'{{previous_step.output}}'}</span></div>
                  <div>Ecris le correctif.</div>
                  <div style={{ height: 8 }} />
                  <div><span style={{ color: 'rgba(255,255,255,0.3)' }}>{'// Step 3 : "verify"'}</span></div>
                  <div>Contexte : <span style={{ color: '#c8ff00' }}>{'{{steps.analyze.output}}'}</span></div>
                  <div>Fix : <span style={{ color: '#c8ff00' }}>{'{{steps.fix.output}}'}</span></div>
                  <div>Lance les tests et verifie.</div>
                </div>
              </div>
            </div>
          )}

          {steps.map((step, i) => {
            const isAdvOpen = expandedStepAdvanced === i;
            const hasAdvanced = (step.on_result && step.on_result.length > 0) ||
              step.agent_settings ||
              step.stall_timeout_secs || step.retry || step.delay_after_secs;

            return (
              <div key={i} style={{ ...ws.stepEditCard, marginBottom: 12 }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
                  <span style={{ width: 20, height: 20, borderRadius: '50%', display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: 10, fontWeight: 700, background: 'rgba(200,255,0,0.1)', color: '#c8ff00' }}>
                    {i + 1}
                  </span>
                  <input
                    style={{ ...ws.input, flex: 1 }}
                    value={step.name}
                    onChange={e => updateStep(i, { name: e.target.value })}
                    placeholder={t('wiz.stepName')}
                  />
                  <select
                    style={{ ...ws.select, width: 120 }}
                    value={step.agent}
                    onChange={e => updateStep(i, { agent: e.target.value as AgentType })}
                  >
                    {availableAgents.map(a => (
                      <option key={a.type} value={a.type}>{a.label}</option>
                    ))}
                  </select>
                  {steps.length > 1 && (
                    <button style={ws.iconBtn} onClick={() => removeStep(i)}>
                      <X size={12} />
                    </button>
                  )}
                </div>
                {checkAgentRestricted(agentAccess, step.agent) && (
                  <div style={{
                    display: 'flex', alignItems: 'center', gap: 6, padding: '6px 10px', marginBottom: 6,
                    borderRadius: 6, background: 'rgba(255,200,0,0.06)', border: '1px solid rgba(255,200,0,0.15)',
                    fontSize: 11, color: '#ffc800',
                  }}>
                    <AlertTriangle size={12} />
                    <span>{t('config.restrictedStep')}</span>
                    <span style={{ cursor: 'pointer', textDecoration: 'underline', marginLeft: 4 }}
                      onClick={() => window.location.hash = '#config'}
                    >{t('config.restrictedAgentLink')}</span>
                  </div>
                )}
                <textarea
                  style={ws.textarea}
                  rows={3}
                  value={step.prompt_template}
                  onChange={e => updateStep(i, { prompt_template: e.target.value })}
                  placeholder={i === 0
                    ? 'Prompt template... ex: Analyse le bug {{issue.title}}. Trouve la cause racine.'
                    : `Prompt template... ex: Voici l'analyse : {{previous_step.output}}. Ecris le correctif.`
                  }
                />
                {/* Hint: available variables for this step */}
                {i > 0 && (
                  <div style={{ marginTop: 4, fontSize: 10, color: 'rgba(255,255,255,0.2)' }}>
                    Dispo : <code style={{ color: 'rgba(200,255,0,0.5)' }}>{'{{previous_step.output}}'}</code>
                    {steps.slice(0, i).map(prev => (
                      <span key={prev.name}>{' '}<code style={{ color: 'rgba(200,255,0,0.5)' }}>{`{{steps.${prev.name}.output}}`}</code></span>
                    ))}
                  </div>
                )}

                {/* Skills selector per step */}
                {availableSkills.length > 0 && (
                  <div style={{ marginTop: 8 }}>
                    <label style={{ fontSize: 10, fontWeight: 600, color: 'rgba(255,255,255,0.4)', display: 'flex', alignItems: 'center', gap: 4 }}>
                      <Zap size={9} /> {t('skills.selectSkills')}
                    </label>
                    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4, marginTop: 4 }}>
                      {availableSkills.map(skill => {
                        const ids = step.skill_ids ?? [];
                        const selected = ids.includes(skill.id);
                        return (
                          <button
                            key={skill.id}
                            type="button"
                            onClick={() => {
                              const newIds = selected ? ids.filter(id => id !== skill.id) : [...ids, skill.id];
                              updateStep(i, { skill_ids: newIds.length > 0 ? newIds : undefined });
                            }}
                            style={{
                              padding: '3px 8px', borderRadius: 10, fontSize: 10, fontFamily: 'inherit',
                              fontWeight: selected ? 600 : 400, cursor: 'pointer',
                              border: selected ? '1px solid rgba(200,255,0,0.4)' : '1px solid rgba(255,255,255,0.08)',
                              background: selected ? 'rgba(200,255,0,0.1)' : 'rgba(255,255,255,0.03)',
                              color: selected ? '#c8ff00' : 'rgba(255,255,255,0.4)',
                              display: 'flex', alignItems: 'center', gap: 3,
                              transition: 'all 0.15s',
                            }}
                            title={skill.description}
                          >
                            {selected && <Check size={8} />}
                            {skill.name}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                )}

                {/* Advanced toggle */}
                <button
                  style={{
                    ...ws.advancedToggle,
                    color: hasAdvanced ? '#c8ff00' : 'rgba(255,255,255,0.25)',
                  }}
                  onClick={() => setExpandedStepAdvanced(isAdvOpen ? null : i)}
                >
                  <Settings size={10} />
                  {t('wiz.advanced')}{hasAdvanced ? ' *' : ''}
                  {isAdvOpen ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
                </button>

                {isAdvOpen && (
                  <div style={ws.advancedPanel}>
                    {/* Agent settings */}
                    <div style={{ marginBottom: 10 }}>
                      <label style={ws.label}>{t('wiz.agentSettings')}</label>
                      <div style={{ display: 'flex', gap: 6 }}>
                        <div style={{ flex: 1 }}>
                          <label style={{ ...ws.label, fontSize: 9 }}>{t('wiz.model')}</label>
                          <input
                            style={ws.input}
                            value={step.agent_settings?.model ?? ''}
                            onChange={e => updateStep(i, {
                              agent_settings: { ...step.agent_settings, model: e.target.value || null }
                            })}
                            placeholder="ex: o3"
                          />
                        </div>
                        <div style={{ flex: 1 }}>
                          <label style={{ ...ws.label, fontSize: 9 }}>Reasoning effort</label>
                          <select
                            style={ws.select}
                            value={step.agent_settings?.reasoning_effort ?? ''}
                            onChange={e => updateStep(i, {
                              agent_settings: { ...step.agent_settings, reasoning_effort: e.target.value || null }
                            })}
                          >
                            <option value="">default</option>
                            <option value="low">low</option>
                            <option value="medium">medium</option>
                            <option value="high">high</option>
                          </select>
                        </div>
                        <div style={{ flex: 1 }}>
                          <label style={{ ...ws.label, fontSize: 9 }}>Max tokens</label>
                          <input
                            type="number"
                            style={ws.input}
                            value={step.agent_settings?.max_tokens ?? ''}
                            onChange={e => updateStep(i, {
                              agent_settings: { ...step.agent_settings, max_tokens: e.target.value ? parseInt(e.target.value) : null }
                            })}
                            placeholder="ex: 16000"
                          />
                        </div>
                      </div>
                    </div>

                    {/* Stall timeout */}
                    <div style={{ display: 'flex', gap: 12, marginBottom: 10 }}>
                      <div>
                        <label style={ws.label}>{t('wiz.stallTimeout')}</label>
                        <input
                          type="number" min={0}
                          style={{ ...ws.input, width: 90 }}
                          value={step.stall_timeout_secs ?? ''}
                          onChange={e => updateStep(i, {
                            stall_timeout_secs: e.target.value ? parseInt(e.target.value) : null,
                          })}
                          placeholder="600"
                        />
                      </div>

                      <div>
                        <label style={ws.label}>{t('wiz.delayAfter')}</label>
                        <input
                          type="number" min={0}
                          style={{ ...ws.input, width: 90 }}
                          value={step.delay_after_secs ?? ''}
                          onChange={e => updateStep(i, {
                            delay_after_secs: e.target.value ? parseInt(e.target.value) : null,
                          })}
                          placeholder="0"
                        />
                      </div>

                      {/* Retry */}
                      <div style={{ flex: 1 }}>
                        <label style={ws.label}>{t('wiz.retry')}</label>
                        <div style={{ display: 'flex', gap: 6 }}>
                          <input
                            type="number" min={0} max={10}
                            style={{ ...ws.input, width: 60 }}
                            value={step.retry?.max_retries ?? ''}
                            onChange={e => {
                              const val = e.target.value ? parseInt(e.target.value) : 0;
                              updateStep(i, {
                                retry: val > 0 ? { max_retries: val, backoff: step.retry?.backoff ?? 'exponential' } : null,
                              });
                            }}
                            placeholder="0"
                          />
                          <select
                            style={{ ...ws.select, width: 120 }}
                            value={step.retry?.backoff ?? 'exponential'}
                            onChange={e => {
                              if (step.retry) {
                                updateStep(i, { retry: { ...step.retry, backoff: e.target.value } });
                              }
                            }}
                            disabled={!step.retry}
                          >
                            <option value="exponential">exponential</option>
                            <option value="fixed">fixed</option>
                          </select>
                        </div>
                      </div>
                    </div>

                    {/* on_result conditions */}
                    <div>
                      <label style={ws.label}>{t('wiz.conditions')}</label>
                      {(step.on_result ?? []).map((cond, j) => (
                        <div key={j} style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 4 }}>
                          <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', whiteSpace: 'nowrap' }}>{t('wiz.ifContains')}</span>
                          <input
                            style={{ ...ws.input, flex: 1, fontSize: 11, borderColor: !cond.contains ? 'rgba(255,77,106,0.4)' : undefined }}
                            value={cond.contains}
                            onChange={e => updateCondition(i, j, { contains: e.target.value })}
                            placeholder="NO_RESULTS (obligatoire)"
                          />
                          <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)' }}>&rarr;</span>
                          <select
                            style={{ ...ws.select, width: 100, fontSize: 11 }}
                            value={cond.action.type}
                            onChange={e => {
                              const type = e.target.value as 'Stop' | 'Skip' | 'Goto';
                              const action = type === 'Goto' ? { type: 'Goto' as const, step_name: '' } : { type };
                              updateCondition(i, j, { action: action as StepConditionRule['action'] });
                            }}
                          >
                            <option value="Stop">Stop</option>
                            <option value="Skip">Skip</option>
                            <option value="Goto">Goto</option>
                          </select>
                          {cond.action.type === 'Goto' && (
                            <input
                              style={{ ...ws.input, width: 80, fontSize: 11 }}
                              value={(cond.action as any).step_name ?? ''}
                              onChange={e => updateCondition(i, j, { action: { type: 'Goto', step_name: e.target.value } })}
                              placeholder="step name"
                            />
                          )}
                          <button style={ws.iconBtn} onClick={() => removeCondition(i, j)}>
                            <X size={10} />
                          </button>
                        </div>
                      ))}
                      {(step.on_result ?? []).length === 0 && (
                        <div style={{ display: 'flex', gap: 4, marginTop: 4, flexWrap: 'wrap' }}>
                          <button style={{ ...ws.addStepBtn, padding: '4px 8px', fontSize: 10, width: 'auto' }} onClick={() => addCondition(i)}>
                            <Plus size={10} /> Condition custom
                          </button>
                          <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.2)', alignSelf: 'center' }}>ou :</span>
                          <button
                            style={{ ...ws.addStepBtn, padding: '3px 8px', fontSize: 9, width: 'auto', borderStyle: 'solid' }}
                            onClick={() => updateStep(i, { on_result: [{ contains: 'NO_RESULTS', action: { type: 'Stop' } }] })}
                          >{t('wiz.noResultsStop')}</button>
                        </div>
                      )}
                      <p style={{ fontSize: 9, color: 'rgba(255,255,255,0.2)', margin: '4px 0 0' }}>
                        L'agent recevra l'instruction de terminer par <code style={{ color: 'rgba(200,255,0,0.4)' }}>[SIGNAL: mot-cle]</code> en derniere ligne.
                      </p>
                    </div>
                  </div>
                )}
              </div>
            );
          })}
          <button style={ws.addStepBtn} onClick={addStep}>
            <Plus size={12} /> {t('wiz.addStep')}
          </button>
        </div>
      )}

      {/* Step 3: Config (Safety + Workspace + Concurrency) */}
      {wizardStep === 3 && (
        <div>
          {/* Safety */}
          <div style={{ marginBottom: 16 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 8 }}>
              <Shield size={14} style={{ color: 'rgba(255,255,255,0.4)' }} />
              <span style={{ fontSize: 13, fontWeight: 600, color: 'rgba(255,255,255,0.6)' }}>{t('wiz.security')}</span>
            </div>

            <div style={{ display: 'flex', gap: 12, marginBottom: 8 }}>
              <label style={ws.checkboxLabel}>
                <input type="checkbox" checked={safety.sandbox} onChange={e => setSafety({ ...safety, sandbox: e.target.checked })} />
                <span>{t('wiz.sandbox')}</span>
              </label>
              <label style={ws.checkboxLabel}>
                <input type="checkbox" checked={safety.require_approval} onChange={e => setSafety({ ...safety, require_approval: e.target.checked })} />
                <span>{t('wiz.requireApproval')}</span>
              </label>
            </div>

            <div style={{ display: 'flex', gap: 8 }}>
              <div>
                <label style={ws.label}>{t('wiz.maxFiles')}</label>
                <input
                  type="number" min={0}
                  style={{ ...ws.input, width: 90 }}
                  value={safety.max_files ?? ''}
                  onChange={e => setSafety({ ...safety, max_files: e.target.value ? parseInt(e.target.value) : null })}
                  placeholder="illimite"
                />
              </div>
              <div>
                <label style={ws.label}>{t('wiz.maxLines')}</label>
                <input
                  type="number" min={0}
                  style={{ ...ws.input, width: 90 }}
                  value={safety.max_lines ?? ''}
                  onChange={e => setSafety({ ...safety, max_lines: e.target.value ? parseInt(e.target.value) : null })}
                  placeholder="illimite"
                />
              </div>
            </div>
          </div>

          {/* Concurrency */}
          <div style={{ marginBottom: 16 }}>
            <label style={ws.label}>{t('wiz.concurrency')}</label>
            <input
              type="number" min={1} max={20}
              style={{ ...ws.input, width: 90 }}
              value={concurrencyLimit}
              onChange={e => setConcurrencyLimit(e.target.value)}
              placeholder="illimite"
            />
          </div>

          {/* Workspace hooks */}
          <div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 8 }}>
              <GitBranch size={14} style={{ color: 'rgba(255,255,255,0.4)' }} />
              <span style={{ fontSize: 13, fontWeight: 600, color: 'rgba(255,255,255,0.6)' }}>{t('wiz.hooks')}</span>
            </div>
            <p style={{ fontSize: 10, color: 'rgba(255,255,255,0.25)', margin: '0 0 8px' }}>
              {t('wiz.hooksHint')}
            </p>

            {([
              ['after_create', t('wiz.hookAfterCreate'), wsHookAfterCreate, setWsHookAfterCreate, 'npm install'],
              ['before_run', t('wiz.hookBeforeRun'), wsHookBeforeRun, setWsHookBeforeRun, 'git pull origin main'],
              ['after_run', t('wiz.hookAfterRun'), wsHookAfterRun, setWsHookAfterRun, 'npm run lint'],
              ['before_remove', t('wiz.hookBeforeRemove'), wsHookBeforeRemove, setWsHookBeforeRemove, 'git stash'],
            ] as [string, string, string, (v: string) => void, string][]).map(([key, label, value, setter, placeholder]) => (
              <div key={key} style={{ marginBottom: 6 }}>
                <label style={{ ...ws.label, fontSize: 10 }}>{label} ({key})</label>
                <input
                  style={ws.input}
                  value={value}
                  onChange={e => setter(e.target.value)}
                  placeholder={placeholder}
                />
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Step 4: Summary */}
      {wizardStep === 4 && (
        <div>
          <div style={ws.summaryRow}><span style={ws.summaryLabel}>Nom</span> {name}</div>
          <div style={ws.summaryRow}><span style={ws.summaryLabel}>Projet</span> {projects.find(p => p.id === projectId)?.name ?? 'Aucun'}</div>
          <div style={ws.summaryRow}>
            <span style={ws.summaryLabel}>Trigger</span>
            {triggerType === 'Cron' ? `${cronHumanLabel()} (${buildCronExpr()})` : triggerType === 'Tracker' ? `Tracker: ${trackerOwner}/${trackerRepo}` : 'Manuel'}
          </div>
          {concurrencyLimit && (
            <div style={ws.summaryRow}><span style={ws.summaryLabel}>Concurrence</span> max {concurrencyLimit} runs</div>
          )}
          <div style={ws.summaryRow}><span style={ws.summaryLabel}>Steps</span> {steps.length}</div>
          {steps.map((s, i) => (
            <div key={i} style={{ ...ws.summaryRow, paddingLeft: 20 }}>
              {i + 1}. <span style={{ color: AGENT_COLORS[s.agent] ?? '#888', fontWeight: 600 }}>{s.name}</span> ({AGENT_LABELS[s.agent] ?? s.agent})
              {s.on_result && s.on_result.length > 0 && <span style={{ color: 'rgba(255,255,255,0.3)', fontSize: 10 }}> [{s.on_result.length} condition{s.on_result.length > 1 ? 's' : ''}]</span>}
              {s.retry && <span style={{ color: 'rgba(255,255,255,0.3)', fontSize: 10 }}> [retry x{s.retry.max_retries}]</span>}
              {s.stall_timeout_secs && <span style={{ color: 'rgba(255,255,255,0.3)', fontSize: 10 }}> [timeout {s.stall_timeout_secs}s]</span>}
              {s.delay_after_secs && <span style={{ color: 'rgba(255,255,255,0.3)', fontSize: 10 }}> [delai {s.delay_after_secs}s]</span>}
            </div>
          ))}
          {(safety.sandbox || safety.require_approval || safety.max_files || safety.max_lines) && (
            <div style={ws.summaryRow}>
              <span style={ws.summaryLabel}>Securite</span>
              {[
                safety.sandbox && 'sandbox',
                safety.require_approval && 'approbation',
                safety.max_files && `max ${safety.max_files} fichiers`,
                safety.max_lines && `max ${safety.max_lines} lignes`,
              ].filter(Boolean).join(', ')}
            </div>
          )}
          {(wsHookAfterCreate || wsHookBeforeRun || wsHookAfterRun || wsHookBeforeRemove) && (
            <div style={ws.summaryRow}>
              <span style={ws.summaryLabel}>Hooks</span>
              {[
                wsHookAfterCreate && 'after_create',
                wsHookBeforeRun && 'before_run',
                wsHookAfterRun && 'after_run',
                wsHookBeforeRemove && 'before_remove',
              ].filter(Boolean).join(', ')}
            </div>
          )}
        </div>
      )}

      {/* Validation errors */}
      {wizardStep === lastStep && (() => {
        const errors: string[] = [];
        if (!name) errors.push(t('wiz.errorNoName'));
        steps.forEach((s, i) => {
          if (!s.prompt_template) errors.push(t('wiz.errorNoPrompt').replace('{0}', s.name || `step-${i + 1}`));
          (s.on_result ?? []).forEach((r, j) => {
            if (!r.contains) errors.push(t('wiz.errorNoCondition').replace('{0}', s.name || `step-${i + 1}`).replace('{1}', String(j + 1)));
          });
        });
        return errors.length > 0 ? (
          <div style={{ padding: '8px 12px', marginTop: 8, borderRadius: 6, background: 'rgba(255,77,106,0.06)', border: '1px solid rgba(255,77,106,0.15)' }}>
            {errors.map((err, i) => (
              <div key={i} style={{ fontSize: 10, color: 'rgba(255,77,106,0.8)', padding: '2px 0' }}>• {err}</div>
            ))}
          </div>
        ) : null;
      })()}

      {/* Navigation */}
      <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: 20 }}>
        <button style={ws.cancelBtn} onClick={wizardStep === 0 ? onCancel : () => setWizardStep(wizardStep - 1)}>
          {wizardStep === 0 ? t('common.cancel') : t('wiz.previous')}
        </button>
        {wizardStep < lastStep ? (
          <button
            style={ws.nextBtn}
            onClick={() => setWizardStep(wizardStep + 1)}
            disabled={wizardStep === 0 && !name}
          >
            {t('wiz.next')} <ChevronRight size={12} />
          </button>
        ) : (
          <button
            style={ws.nextBtn}
            onClick={handleSave}
            disabled={saving || !name || steps.some(s => !s.prompt_template || (s.on_result ?? []).some(r => !r.contains))}
          >
            {saving ? <Loader2 size={12} /> : <Check size={12} />}
            {isEdit ? t('wiz.save') : t('wiz.create')}
          </button>
        )}
      </div>
    </div>
  );
}

// ─── Styles ──────────────────────────────────────────────────────────────────

const ws = {
  h1: { fontSize: 22, fontWeight: 800, letterSpacing: '-0.02em', color: '#e8eaed', margin: 0 } as const,
  meta: { fontSize: 12, color: 'rgba(255,255,255,0.35)', margin: '4px 0 0' } as const,
  empty: { display: 'flex', flexDirection: 'column' as const, alignItems: 'center', justifyContent: 'center', padding: 60, color: 'rgba(255,255,255,0.25)', fontSize: 13 },
  card: (active: boolean) => ({
    padding: '14px 16px', borderRadius: 10,
    background: active ? 'rgba(200,255,0,0.03)' : 'rgba(255,255,255,0.02)',
    border: `1px solid ${active ? 'rgba(200,255,0,0.15)' : 'rgba(255,255,255,0.06)'}`,
  }),
  createBtn: {
    padding: '8px 16px', borderRadius: 8, border: '1px solid rgba(200,255,0,0.2)',
    background: 'rgba(200,255,0,0.08)', color: '#c8ff00', cursor: 'pointer',
    display: 'flex', alignItems: 'center', gap: 6, fontSize: 12, fontFamily: 'inherit', fontWeight: 600,
  } as const,
  smallBtn: {
    padding: '4px 10px', borderRadius: 6, border: '1px solid rgba(255,255,255,0.08)',
    background: 'rgba(255,255,255,0.03)', color: 'rgba(255,255,255,0.5)', cursor: 'pointer',
    display: 'flex', alignItems: 'center', gap: 4, fontSize: 10, fontFamily: 'inherit',
  } as const,
  iconBtn: {
    background: 'none', border: 'none', cursor: 'pointer', padding: 4,
    color: 'rgba(255,255,255,0.3)', display: 'flex', alignItems: 'center',
  } as const,
  triggerBadge: (type: string) => ({
    display: 'flex', alignItems: 'center', gap: 4, padding: '2px 8px', borderRadius: 4,
    fontSize: 10, fontWeight: 600,
    background: type === 'cron' ? 'rgba(0,212,255,0.08)' : type === 'tracker' ? 'rgba(139,92,246,0.08)' : 'rgba(200,255,0,0.08)',
    color: type === 'cron' ? '#00d4ff' : type === 'tracker' ? '#8b5cf6' : '#c8ff00',
    border: `1px solid ${type === 'cron' ? 'rgba(0,212,255,0.15)' : type === 'tracker' ? 'rgba(139,92,246,0.15)' : 'rgba(200,255,0,0.15)'}`,
  }),
  // Detail panel
  detailPanel: {
    padding: '16px 20px', borderRadius: 10,
    background: 'rgba(255,255,255,0.02)', border: '1px solid rgba(255,255,255,0.06)',
  } as const,
  infoRow: { display: 'flex', alignItems: 'center', gap: 8, fontSize: 12, marginBottom: 6, color: 'rgba(255,255,255,0.5)' } as const,
  infoLabel: { fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.3)', width: 100 } as const,
  sectionTitle: { fontSize: 12, fontWeight: 700, color: 'rgba(255,255,255,0.5)', marginTop: 16, marginBottom: 8, textTransform: 'uppercase' as const, letterSpacing: '0.04em' },
  stepCard: {
    padding: '10px 14px', borderRadius: 8, marginBottom: 6,
    background: 'rgba(255,255,255,0.02)', border: '1px solid rgba(255,255,255,0.06)',
  } as const,
  runCard: {
    padding: '10px 14px', borderRadius: 8, marginTop: 8,
    background: 'rgba(255,255,255,0.02)', border: '1px solid rgba(255,255,255,0.06)',
  } as const,
  // Wizard
  wizardCard: {
    maxWidth: 560, margin: '0 auto', padding: 24, borderRadius: 12,
    background: 'rgba(255,255,255,0.02)', border: '1px solid rgba(255,255,255,0.08)',
  } as const,
  label: { display: 'block', fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', marginBottom: 4 } as const,
  input: {
    width: '100%', padding: '9px 12px', background: '#1a1d26',
    border: '1px solid rgba(255,255,255,0.12)', borderRadius: 8, color: '#e8eaed',
    fontSize: 13, fontFamily: 'inherit', outline: 'none', boxSizing: 'border-box' as const,
  } as const,
  select: {
    padding: '9px 12px', background: '#1a1d26',
    border: '1px solid rgba(255,255,255,0.12)', borderRadius: 8, color: '#e8eaed',
    fontSize: 13, fontFamily: 'inherit', outline: 'none', cursor: 'pointer',
    appearance: 'none' as const,
    backgroundImage: `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%23888' stroke-width='2'%3E%3Cpolyline points='6 9 12 15 18 9'%3E%3C/polyline%3E%3C/svg%3E")`,
    backgroundRepeat: 'no-repeat' as const, backgroundPosition: 'right 10px center',
    paddingRight: 32,
  } as const,
  textarea: {
    width: '100%', padding: '9px 12px', background: '#1a1d26',
    border: '1px solid rgba(255,255,255,0.12)', borderRadius: 8, color: '#e8eaed',
    fontSize: 13, fontFamily: 'inherit', outline: 'none', resize: 'vertical' as const,
    boxSizing: 'border-box' as const, lineHeight: 1.5,
  } as const,
  triggerBtn: {
    flex: 1, padding: '10px 12px', borderRadius: 8, border: '1px solid',
    cursor: 'pointer', display: 'flex', alignItems: 'center', justifyContent: 'center',
    gap: 6, fontSize: 12, fontFamily: 'inherit', fontWeight: 600,
  } as const,
  stepEditCard: {
    padding: '12px 14px', borderRadius: 8,
    background: 'rgba(255,255,255,0.02)', border: '1px solid rgba(255,255,255,0.08)',
  } as const,
  addStepBtn: {
    width: '100%', padding: '8px 12px', borderRadius: 8,
    border: '1px dashed rgba(255,255,255,0.12)', background: 'transparent',
    color: 'rgba(255,255,255,0.35)', cursor: 'pointer', display: 'flex',
    alignItems: 'center', justifyContent: 'center', gap: 6, fontSize: 11, fontFamily: 'inherit',
  } as const,
  summaryRow: { fontSize: 12, padding: '4px 0', color: 'rgba(255,255,255,0.5)' } as const,
  summaryLabel: { fontWeight: 600, color: 'rgba(255,255,255,0.3)', marginRight: 8, display: 'inline-block', width: 70 } as const,
  // Help panel
  smallHelpBtn: {
    display: 'flex', alignItems: 'center', gap: 6, padding: '6px 12px',
    borderRadius: 6, border: '1px solid', fontSize: 11, fontFamily: 'inherit',
    cursor: 'pointer', fontWeight: 500,
  } as const,
  helpPanel: {
    padding: '14px 16px', borderRadius: 8, marginBottom: 14,
    background: 'rgba(200,255,0,0.02)', border: '1px solid rgba(200,255,0,0.08)',
  } as const,
  helpSection: { marginBottom: 12 } as const,
  helpTitle: { fontSize: 11, fontWeight: 700, color: 'rgba(255,255,255,0.45)', marginBottom: 6, textTransform: 'uppercase' as const, letterSpacing: '0.03em' } as const,
  helpGrid: { display: 'flex', flexDirection: 'column' as const, gap: 3 } as const,
  helpRow: { display: 'flex', alignItems: 'center', gap: 10, fontSize: 11 } as const,
  helpCode: { fontFamily: 'monospace', fontSize: 11, color: '#c8ff00', background: 'rgba(200,255,0,0.06)', padding: '1px 6px', borderRadius: 4, whiteSpace: 'nowrap' as const } as const,
  helpDesc: { color: 'rgba(255,255,255,0.4)' } as const,
  cancelBtn: {
    padding: '8px 16px', borderRadius: 8, border: '1px solid rgba(255,255,255,0.08)',
    background: 'transparent', color: 'rgba(255,255,255,0.4)', cursor: 'pointer',
    fontSize: 12, fontFamily: 'inherit',
  } as const,
  nextBtn: {
    padding: '8px 16px', borderRadius: 8, border: '1px solid rgba(200,255,0,0.2)',
    background: 'rgba(200,255,0,0.08)', color: '#c8ff00', cursor: 'pointer',
    display: 'flex', alignItems: 'center', gap: 6, fontSize: 12, fontFamily: 'inherit', fontWeight: 600,
  } as const,
  // Advanced step config
  advancedToggle: {
    display: 'flex', alignItems: 'center', gap: 4, marginTop: 8, padding: '4px 8px',
    border: 'none', background: 'none', cursor: 'pointer',
    fontSize: 10, fontFamily: 'inherit',
  } as const,
  advancedPanel: {
    marginTop: 8, padding: '10px 12px', borderRadius: 6,
    background: 'rgba(255,255,255,0.015)', border: '1px solid rgba(255,255,255,0.06)',
  } as const,
  modeBtn: {
    padding: '4px 12px', borderRadius: 6, border: '1px solid rgba(255,255,255,0.08)',
    background: 'transparent', color: 'rgba(255,255,255,0.4)', cursor: 'pointer',
    fontSize: 11, fontFamily: 'inherit', fontWeight: 500,
  } as const,
  modeBtnActive: {
    background: 'rgba(200,255,0,0.06)', borderColor: 'rgba(200,255,0,0.2)', color: '#c8ff00',
  } as const,
  checkboxLabel: {
    display: 'flex', alignItems: 'center', gap: 6, fontSize: 11, color: 'rgba(255,255,255,0.5)', cursor: 'pointer',
  } as const,
};
