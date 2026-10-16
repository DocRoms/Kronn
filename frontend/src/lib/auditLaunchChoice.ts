import type { AgentType, ModelTier } from '../types/generated';

const TIERS: readonly ModelTier[] = ['economy', 'default', 'reasoning'];

/** What a run keeps of how it was launched (the fields of an audit run row). */
export interface RunLaunch {
  agent_type?: string;
  tier?: string | null;
  connection_id?: string | null;
}

export interface AuditLaunchChoice {
  agent: AgentType;
  tier: ModelTier;
  connectionId: string | null;
}

/**
 * KT-1320 — the agent, tier and connection of the run a retry or a resume
 * continues, when it can run on them again. `null` when the run did not keep a
 * tier (recorded before), its agent is gone (`repository` for a run a colleague
 * committed) or no longer usable: the picker's own choice applies then.
 */
export function runLaunchChoice(
  run: RunLaunch | null,
  usableAgents: readonly AgentType[],
): AuditLaunchChoice | null {
  if (!run) return null;
  const tier = TIERS.find(candidate => candidate === run.tier);
  const connectionId = run.connection_id ?? null;
  const agent = usableAgents.find(candidate => candidate === run.agent_type);
  if (!tier || !agent) return null;
  return { agent, tier, connectionId };
}

/**
 * The choice a launch runs on, and the one the picker shows: the human's once
 * they touched the picker, else the run being continued, else the picker's.
 */
export function effectiveLaunchChoice(
  picked: AuditLaunchChoice,
  touched: boolean,
  fromRun: AuditLaunchChoice | null,
): { choice: AuditLaunchChoice; fromRun: boolean } {
  if (!touched && fromRun) return { choice: fromRun, fromRun: true };
  return { choice: picked, fromRun: false };
}
