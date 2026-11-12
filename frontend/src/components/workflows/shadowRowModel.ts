/**
 * KT-1707 — what the rows of a shadowed step say: whether the step reads as a block
 * (Principal row + shadows), and the `Agent · model` a row names.
 */
import { AGENT_LABELS } from '../../lib/constants';
import type { AgentSwitchTarget } from '../AgentSwitchPicker';
import { stepCanCarryShadows } from './shadowEditorModel';
import type { MainAgent } from './ShadowAgentPicker';
import { resolveCatalogTier } from '../../lib/modelCatalogSelection';
import type { ModelTiersConfig, ShadowAgent, WorkflowStep } from '../../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

/** Whether the step reads as a block (Principal row + shadows) rather than as its single agent line. */
export const hasShadowBlock = (step: WorkflowStep) => stepCanCarryShadows(step) && (step.shadows?.length ?? 0) > 0;

/** The agent a step runs on: what its picker shows, and what a shadow without an agent of its own runs on. */
export const stepMainAgent = (step: WorkflowStep): MainAgent => ({
  agent: step.agent,
  connectionId: step.agent_settings?.connection_id,
  model: step.agent_settings?.model,
  tier: step.agent_settings?.tier ?? 'default',
});

/**
 * The model an agent runs, resolved the way the step's own picker names it: the explicit
 * model, else the one configured for its tier (the connection's, or the agent family's).
 * Empty when nothing is configured.
 */
export function resolveAgentModel(agent: MainAgent, choices: readonly AgentSwitchTarget[], familyTiers?: ModelTiersConfig | null): string {
  const connectionId = agent.connectionId ?? null;
  const known = choices.find(target => target.agent === agent.agent && (target.connectionId ?? null) === connectionId);
  const target = known ?? { agent: agent.agent, connectionId };
  return resolveCatalogTier(null, target, agent.tier ?? 'default', familyTiers, agent.model).model || '';
}

/**
 * What a row, or a branch-map line, shows for an agent: `Agent · model`. A shadow without
 * an agent of its own runs on the principal, so it shows the principal's agent and model
 * resolved. « default model » only stands for a model nothing configures.
 */
export function agentModelLabel(
  value: ShadowAgent | undefined,
  main: MainAgent,
  choices: readonly AgentSwitchTarget[],
  t: Translate,
  familyTiers?: ModelTiersConfig | null,
): string {
  const resolved: MainAgent = value ? { agent: value.agent, connectionId: value.connection_id, model: value.model } : main;
  const connectionId = resolved.connectionId ?? null;
  const known = choices.find(target => target.agent === resolved.agent && (target.connectionId ?? null) === connectionId);
  const agent = known?.label ?? AGENT_LABELS[resolved.agent] ?? resolved.agent;
  return `${agent} · ${resolveAgentModel(resolved, choices, familyTiers) || t('wf.shadow.rows.defaultModel')}`;
}
