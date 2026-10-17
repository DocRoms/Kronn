import type { AgentDetection, AgentType, ModelTier } from '../types/generated';
import type { ExternalApiConnectionView } from './api';
import { canRunAudit } from './agentCapabilities';

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

/** HTTP providers a named connection stands for (by its origin preset). */
export const PRESET_AGENT: Record<string, AgentType> = {
  lite_llm: 'LiteLlm', nvidia: 'Nvidia', open_router: 'Custom', other: 'Custom',
};

const HTTP_AGENTS = new Set<AgentType>(['LiteLlm', 'Nvidia', 'Custom']);

export const connectionAgent = (connection: Pick<ExternalApiConnectionView, 'origin_preset'>): AgentType =>
  PRESET_AGENT[connection.origin_preset] ?? 'Custom';

/** An agent a launch can run on: a detection, or one named connection. */
export interface AuditTarget {
  agent: AgentType;
  connectionId: string | null;
}

/**
 * The targets the agent picker offers, usable ones only: the detected agents
 * (a named connection replaces the bare provider slot of its preset) and every
 * named connection that has an endpoint. The picker and the run's inheritance
 * read this one inventory, so the panel never shows a target a launch cannot
 * use, nor a launch one the panel cannot show.
 */
export function usableAuditTargets(
  agents: readonly AgentDetection[],
  connections: readonly ExternalApiConnectionView[],
): AuditTarget[] {
  const connected = new Set(connections.map(connectionAgent));
  const detected = agents
    .filter(canRunAudit)
    .filter(a => !(HTTP_AGENTS.has(a.agent_type) && connected.has(a.agent_type)))
    .map(a => ({ agent: a.agent_type, connectionId: null }));
  const named = connections
    .filter(c => !!c.endpoint)
    .map(c => ({ agent: connectionAgent(c), connectionId: c.id }));
  return [...detected, ...named];
}

/**
 * KT-1320 — the agent, tier and connection of the run a retry or a resume
 * continues, when the picker offers that target. `null` when the run did not
 * keep a tier (recorded before), its agent is gone (`repository` for a run a
 * colleague committed) or no longer usable: the picker's own choice applies
 * then. A run whose named connection was deleted since is shown on its agent's
 * default connection, which is what the server falls back to.
 */
export function runLaunchChoice(
  run: RunLaunch | null,
  targets: readonly AuditTarget[],
): AuditLaunchChoice | null {
  if (!run) return null;
  const tier = TIERS.find(candidate => candidate === run.tier);
  if (!tier) return null;
  const wanted = run.connection_id ?? null;
  const target = targets.find(t => t.agent === run.agent_type && t.connectionId === wanted)
    ?? targets.find(t => t.agent === run.agent_type && t.connectionId === null);
  if (!target) return null;
  return { agent: target.agent, tier, connectionId: target.connectionId };
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
