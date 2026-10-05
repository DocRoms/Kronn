import { useEffect, useMemo, useState } from 'react';
import { RotateCcw } from 'lucide-react';
import { agents as agentsApi, workflows as workflowsApi } from '../lib/api';
import { AGENT_LABELS } from '../lib/constants';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import type { AgentDetection, AgentType, StepAgentOverride, WorkflowStep } from '../types/generated';
import { AgentSwitchPicker } from './AgentSwitchPicker';
import { ModelCatalogPicker } from './ModelCatalogPicker';

export type StepAgentChoices = Record<string, StepAgentOverride>;

interface Props {
  workflowId: string;
  /** Only the steps the reader changed; empty sends today's request. */
  value: StepAgentChoices;
  onChange: (next: StepAgentChoices) => void;
  disabled?: boolean;
}

/** Same rule as the run preflight: installed (or reachable) and enabled. */
function usable(detection: AgentDetection): boolean {
  return detection.enabled && (detection.installed || detection.runtime_available);
}

function stepKey(step: WorkflowStep): string {
  return step.id || step.name;
}

function planned(step: WorkflowStep): StepAgentOverride {
  return {
    agent: step.agent,
    model: step.agent_settings?.model || undefined,
    reasoning_effort: step.agent_settings?.reasoning_effort || undefined,
  };
}

function sameChoice(left: StepAgentOverride, right: StepAgentOverride): boolean {
  return left.agent === right.agent
    && (left.model || undefined) === (right.model || undefined)
    && (left.reasoning_effort || undefined) === (right.reasoning_effort || undefined);
}

/** KT-1025 — the agent, model and effort each Agent step will run on, with
 * the shared catalogue selectors to change them for this launch only. */
export function WorkflowStepAgents({ workflowId, value, onChange, disabled = false }: Props) {
  const { t } = useT();
  const [steps, setSteps] = useState<WorkflowStep[] | null>(null);
  const [available, setAvailable] = useState<AgentType[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    Promise.all([workflowsApi.get(workflowId), agentsApi.detect()])
      .then(([workflow, detections]) => {
        if (!active) return;
        setSteps(workflow.steps.filter(step => step.step_type.type === 'Agent'));
        setAvailable(detections.filter(usable).map(detection => detection.agent_type));
      })
      .catch(cause => {
        if (active) setError(userError(cause));
      });
    return () => { active = false; };
  }, [workflowId]);

  const changed = useMemo(() => Object.keys(value).length, [value]);

  const choose = (step: WorkflowStep, next: StepAgentOverride) => {
    const key = stepKey(step);
    const rest = Object.fromEntries(Object.entries(value).filter(([name]) => name !== key));
    onChange(sameChoice(next, planned(step)) ? rest : { ...rest, [key]: next });
  };

  if (error) {
    return <p className="discussion-action-card__diagnostic" role="alert">{t('disc.action.stepAgents.loadError', error)}</p>;
  }
  if (!steps || steps.length === 0) return null;

  return (
    <div className="discussion-action-card__fields">
      <details className="discussion-action-card__details" data-testid="action-card-step-agents">
        <summary>
          {t('disc.action.stepAgents.summary', steps.length)}
          {changed > 0 && <> · <strong>{t('disc.action.stepAgents.changed', changed)}</strong></>}
        </summary>
        <ul className="discussion-action-card__step-agents">
          {steps.map(step => {
            const key = stepKey(step);
            const plan = planned(step);
            const current = value[key] ?? plan;
            // A named HTTP connection belongs to the step's own agent only.
            const connectionId = current.agent === step.agent ? step.agent_settings?.connection_id : null;
            const agents = Array.from(new Set<AgentType>([step.agent, current.agent, ...available]))
              .filter(agent => agent !== 'Custom' || agent === step.agent);
            const plannedLabel = [
              AGENT_LABELS[plan.agent] ?? plan.agent,
              plan.model || t('config.defaultModel'),
              plan.reasoning_effort,
            ].filter(Boolean).join(' · ');
            return (
              <li key={key} data-testid={`step-agent-${key}`} data-changed={Boolean(value[key])}>
                <div className="discussion-action-card__step-agent-head">
                  <strong>{step.name}</strong>
                  <small>{t('disc.action.stepAgents.planned', plannedLabel)}</small>
                  {value[key] && (
                    <button
                      type="button"
                      className="discussion-action-card__reset"
                      disabled={disabled}
                      onClick={() => choose(step, plan)}
                    >
                      <RotateCcw size={11} aria-hidden /> {t('disc.action.stepAgents.reset')}
                    </button>
                  )}
                </div>
                <AgentSwitchPicker
                  currentAgent={current.agent}
                  availableAgents={agents}
                  currentConnectionId={connectionId}
                  // A model is target-scoped: a new agent starts from its default.
                  onChange={async agent => choose(step, agent === step.agent ? plan : { agent })}
                  disabled={disabled}
                  title={t('disc.action.stepAgents.agentFor', step.name)}
                  ariaLabel={t('disc.action.stepAgents.agentFor', step.name)}
                />
                <ModelCatalogPicker
                  agent={current.agent}
                  connectionId={connectionId}
                  value={current.model ?? ''}
                  onChange={model => choose(step, { ...current, model: model || undefined })}
                  reasoningEffort={current.reasoning_effort ?? ''}
                  onReasoningChange={effort => choose(step, { ...current, reasoning_effort: effort || undefined })}
                  disabled={disabled}
                />
              </li>
            );
          })}
        </ul>
      </details>
    </div>
  );
}
