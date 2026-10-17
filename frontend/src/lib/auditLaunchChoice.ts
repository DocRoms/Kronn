import type { AgentType, ModelTier } from '../types/generated';
import type { ExternalApiConnectionView } from './api';

const TIERS: readonly ModelTier[] = ['economy', 'default', 'reasoning'];

const AGENT_TYPES: readonly AgentType[] = [
  'ClaudeCode', 'Codex', 'OpenCode', 'Vibe', 'GeminiCli', 'Kiro', 'CopilotCli',
  'Ollama', 'LiteLlm', 'Nvidia', 'Custom',
];

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

export const connectionAgent = (connection: Pick<ExternalApiConnectionView, 'origin_preset'>): AgentType =>
  PRESET_AGENT[connection.origin_preset] ?? 'Custom';

/** Whether the named connections are known yet: a list still loading, or one that failed, is not an empty one. */
export type ConnectionsStatus = 'loading' | 'ready' | 'error';

export interface ConnectionInventory {
  status: ConnectionsStatus;
  list: readonly Pick<ExternalApiConnectionView, 'id'>[];
}

/** The run's choice, and whether its named connection is known to be deleted. */
export interface RunChoice extends AuditLaunchChoice {
  /** `true` only once the connection list loaded and the run's connection is not in it. */
  connectionGone: boolean;
}

/**
 * KT-1320 — the agent, tier and connection of the run a retry or a resume
 * continues. The agent and the tier are the run's, always: nothing here (a
 * connection list still loading, one that failed, a connection deleted since, an
 * agent not detected) may change them without a human choice. `null` only when
 * the run kept nothing to inherit: no tier (recorded before), or an agent that is
 * not one (`repository` for a run a colleague committed).
 *
 * The connection follows what is known: kept while the list loads or failed to
 * load (the server verifies it on a resume), kept when it is in the list, and
 * dropped with `connectionGone` once the list says it was deleted — the run then
 * goes on the agent's default connection, which is what the server falls back to.
 */
export function runLaunchChoice(
  run: RunLaunch | null,
  connections: ConnectionInventory,
): RunChoice | null {
  if (!run) return null;
  const tier = TIERS.find(candidate => candidate === run.tier);
  const agent = AGENT_TYPES.find(candidate => candidate === run.agent_type);
  if (!tier || !agent) return null;
  const wanted = run.connection_id ?? null;
  if (wanted === null) return { agent, tier, connectionId: null, connectionGone: false };
  const gone = connections.status === 'ready' && !connections.list.some(c => c.id === wanted);
  return { agent, tier, connectionId: gone ? null : wanted, connectionGone: gone };
}

/**
 * The choice a launch runs on, and the one the picker shows: the human's once
 * they touched the picker, else the run being continued, else the picker's.
 */
export function effectiveLaunchChoice(
  picked: AuditLaunchChoice,
  touched: boolean,
  fromRun: RunChoice | null,
): { choice: AuditLaunchChoice; fromRun: boolean; connectionGone: boolean } {
  if (!touched && fromRun) {
    const { connectionGone, ...choice } = fromRun;
    return { choice, fromRun: true, connectionGone };
  }
  return { choice: picked, fromRun: false, connectionGone: false };
}
