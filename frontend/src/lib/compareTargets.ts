import type { AgentSwitchTarget } from '../components/AgentSwitchPicker';
import type { AgentType, MessageTarget, ModelTier } from '../types/generated';

/** One row of a multi-agent selection: agent or connection, with its tier. */
export type CompareTarget = {
  agent: AgentType;
  tier: ModelTier;
  connection_id?: string;
};

export function compareTargetKey(target: Pick<CompareTarget, 'agent' | 'connection_id'>) {
  return `${target.agent}:${target.connection_id ?? ''}`;
}

/** One target per available choice, each at `tier`: the "everyone" default. */
export function defaultCompareTargets(choices: AgentSwitchTarget[], tier: ModelTier): CompareTarget[] {
  return choices.map(choice => ({
    agent: choice.agent,
    tier,
    ...(choice.connectionId ? { connection_id: choice.connectionId } : {}),
  }));
}

/** The selection as one-shot message recipients, each at its own tier. */
export function toMessageTargets(targets: CompareTarget[]): MessageTarget[] {
  return targets.map(target => ({
    kind: 'agent',
    agent_type: target.agent,
    tier: target.tier,
    ...(target.connection_id ? { connection_id: target.connection_id } : {}),
  }));
}
