/**
 * KT-1707 — what the rows of a shadowed step say: whether the step reads as a block
 * (Principal row + shadows), and the `Agent · model` a row names.
 */
import { AGENT_LABELS } from '../../lib/constants';
import type { AgentSwitchTarget } from '../AgentSwitchPicker';
import { stepCanCarryShadows } from './StepShadowsEditor';
import type { MainAgent } from './ShadowAgentPicker';
import type { ShadowAgent, WorkflowStep } from '../../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

/** Whether the step reads as a block (Principal row + shadows) rather than as its single agent line. */
export const hasShadowBlock = (step: WorkflowStep) => stepCanCarryShadows(step) && (step.shadows?.length ?? 0) > 0;

/**
 * What a row shows for an agent: `Agent · model`. A shadow without an agent of its
 * own runs on the principal, so it shows the principal's agent and model resolved.
 */
export function agentModelLabel(value: ShadowAgent | undefined, main: MainAgent, choices: readonly AgentSwitchTarget[], t: Translate): string {
  const resolved: MainAgent = value ? { agent: value.agent, connectionId: value.connection_id, model: value.model } : main;
  const connectionId = resolved.connectionId ?? null;
  const known = choices.find(target => target.agent === resolved.agent && (target.connectionId ?? null) === connectionId);
  const agent = known?.label ?? AGENT_LABELS[resolved.agent] ?? resolved.agent;
  return `${agent} · ${resolved.model || t('wf.shadow.rows.defaultModel')}`;
}
