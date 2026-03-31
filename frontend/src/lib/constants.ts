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
  CopilotCli: '#238636',
  'GitHub Copilot': '#238636',
};

export const AGENT_LABELS: Record<string, string> = {
  ClaudeCode: 'Claude Code',
  Codex: 'Codex',
  Vibe: 'Vibe',
  GeminiCli: 'Gemini CLI',
  Kiro: 'Kiro',
  CopilotCli: 'GitHub Copilot',
};

export const ALL_AGENT_TYPES: AgentType[] = ['ClaudeCode', 'Codex', 'Vibe', 'GeminiCli', 'Kiro', 'CopilotCli'];

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
    CopilotCli: agentAccess.copilot_cli?.full_access,
  };
  return map[agentType] === false;
}

/** Extract org/owner from a project's repo_url for grouping.
 *  Returns the org name (e.g. "acme-org") or a fallback label. */
export function getProjectGroup(p: { repo_url: string | null }, localLabel = 'Local', otherLabel = 'Other'): string {
  if (!p.repo_url) return localLabel;
  try {
    const url = p.repo_url.replace('git@github.com:', 'https://github.com/')
      .replace('git@gitlab.com:', 'https://gitlab.com/');
    const parts = new URL(url).pathname.split('/').filter(Boolean);
    return parts[0] || otherLabel;
  } catch { return otherLabel; }
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
    CopilotCli: agentAccess.copilot_cli?.full_access,
  };
  return map[agentType] === true;
}

// ─── Shared predicates (used by Dashboard, DiscussionsPage, McpPage) ────────

/** Check if a path contains a hidden segment (starts with '.') */
export const isHiddenPath = (path: string) => path.split('/').some(s => s.startsWith('.'));

/** Agent is usable: locally installed OR available via npx/uvx runtime fallback */
export const isUsable = (a: { installed: boolean; runtime_available: boolean; enabled: boolean }) =>
  (a.installed || a.runtime_available) && a.enabled;

/** Check if a discussion title matches the validation audit title */
export const isValidationDisc = (title: string) => title === 'Validation audit AI';
