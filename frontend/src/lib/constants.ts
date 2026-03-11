// ─── Shared constants across pages ──────────────────────────────────────────

import type { AgentType, AgentsConfig } from '../types/generated';

export const AGENT_COLORS: Record<string, string> = {
  ClaudeCode: '#D4714E',
  'Claude Code': '#D4714E',
  Codex: '#10a37f',
  Vibe: '#FF7000',
  GeminiCli: '#4285f4',
  'Gemini CLI': '#4285f4',
  Kiro: '#7B61FF',
};

export const AGENT_LABELS: Record<string, string> = {
  ClaudeCode: 'Claude Code',
  Codex: 'Codex',
  Vibe: 'Vibe',
  GeminiCli: 'Gemini CLI',
  Kiro: 'Kiro',
};

export const ALL_AGENT_TYPES: AgentType[] = ['ClaudeCode', 'Codex', 'Vibe', 'GeminiCli', 'Kiro'];

export const agentColor = (agentType: string | null | undefined): string =>
  AGENT_COLORS[agentType ?? ''] ?? '#8b5cf6';

/** Check if an agent has full_access disabled (restricted mode). */
export function isAgentRestricted(agentAccess: AgentsConfig | undefined, agentType: AgentType): boolean {
  if (!agentAccess) return false;
  const map: Record<string, boolean | undefined> = {
    ClaudeCode: agentAccess.claude_code?.full_access,
    Codex: agentAccess.codex?.full_access,
    GeminiCli: agentAccess.gemini_cli?.full_access,
    Vibe: agentAccess.vibe?.full_access,
    Kiro: undefined,
  };
  return map[agentType] === false;
}

/** Check if an agent has full_access enabled. */
export function hasAgentFullAccess(agentAccess: AgentsConfig | undefined, agentType: AgentType): boolean {
  if (!agentAccess) return false;
  const map: Record<string, boolean | undefined> = {
    ClaudeCode: agentAccess.claude_code?.full_access,
    Codex: agentAccess.codex?.full_access,
    GeminiCli: agentAccess.gemini_cli?.full_access,
    Vibe: agentAccess.vibe?.full_access,
    Kiro: undefined,
  };
  return map[agentType] === true;
}
