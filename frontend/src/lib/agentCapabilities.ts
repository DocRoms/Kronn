import { isUsable } from './constants';
import type { AgentDetection } from '../types/generated';

const AUDIT_AGENT_TYPES = new Set<AgentDetection['agent_type']>([
  'ClaudeCode',
  'Codex',
  'OpenCode',
  'GeminiCli',
  'Kiro',
  'CopilotCli',
  // HTTP agents whose file tools Kronn runs scoped to the project (KT-924).
  // Every provider is the user's choice, with a data-transfer warning
  // (KT-980); Custom only runs with a named connection.
  'Ollama',
  'LiteLlm',
  'Nvidia',
  'Custom',
]);

/**
 * Agents that can run audits, which write `docs/` and therefore need to reach
 * the project's files: a CLI through its own filesystem, an HTTP agent
 * through Kronn's native file tools. This positive allowlist mirrors the
 * backend's `agent_can_audit`: an unknown agent is not audit-capable by default.
 */
export function canRunAudit(agent: AgentDetection): boolean {
  return isUsable(agent) && AUDIT_AGENT_TYPES.has(agent.agent_type);
}

/**
 * Agents that can run the pre-audit briefing. The endpoint only creates a
 * discussion and performs no filesystem writes, so any agent that holds a
 * conversation is eligible; only API-only Vibe is excluded.
 */
export function canRunBriefing(agent: AgentDetection): boolean {
  return isUsable(agent) && agent.agent_type !== 'Vibe';
}
