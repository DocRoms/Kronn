import { isUsable } from './constants';
import type { AgentDetection } from '../types/generated';

const AUDIT_AGENT_TYPES = new Set<AgentDetection['agent_type']>([
  'ClaudeCode',
  'Codex',
  'GeminiCli',
  'Kiro',
  'CopilotCli',
]);

/**
 * Agents that can run audits, which write `docs/` and therefore require
 * filesystem access. This positive allowlist mirrors the backend's
 * `agent_can_audit`: an unknown agent is not audit-capable by default.
 */
export function canRunAudit(agent: AgentDetection): boolean {
  return isUsable(agent) && AUDIT_AGENT_TYPES.has(agent.agent_type);
}

/**
 * Agents that can run the pre-audit briefing. The endpoint only creates a
 * discussion and performs no filesystem writes, so Ollama remains eligible;
 * only API-only Vibe is excluded.
 */
export function canRunBriefing(agent: AgentDetection): boolean {
  return isUsable(agent) && agent.agent_type !== 'Vibe';
}
