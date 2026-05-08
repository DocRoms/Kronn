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
  Ollama: '#60A5FA',
};

export const AGENT_LABELS: Record<string, string> = {
  ClaudeCode: 'Claude Code',
  Codex: 'Codex',
  Vibe: 'Vibe',
  GeminiCli: 'Gemini CLI',
  Kiro: 'Kiro',
  CopilotCli: 'GitHub Copilot',
  Ollama: 'Ollama',
};

export const ALL_AGENT_TYPES: AgentType[] = ['ClaudeCode', 'Codex', 'Vibe', 'GeminiCli', 'Kiro', 'CopilotCli', 'Ollama'];

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
    Ollama: agentAccess.ollama?.full_access,
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

/** Whether the agent can introspect the discussion it's running in
 *  (`disc_meta`, `disc_get_message`, `disc_summarize`).
 *
 *  Two paths exist on the backend (cf. `disc_prompts.rs` —
 *  `agent_speaks_mcp` / `agent_uses_slash_markers` gates):
 *    - MCP tools (single-turn, fast) for Claude Code, Kiro, Gemini,
 *      Copilot — see `mcp_scanner::inject_kronn_internal`.
 *    - Slash markers (multi-turn: agent emits `KRONN:DISC_*`, Kronn
 *      resolves on next turn) for Vibe + Ollama — see
 *      `slash_markers.rs`.
 *
 *  Returns `false` only when the agent has NEITHER path:
 *    - **Codex (0.121)**: reads `~/.codex/config.toml` and *attempts*
 *      the MCP tool call but its exec-mode sandbox cancels the spawn
 *      before the bridge runs. Slash-marker fallback would also work
 *      but exec-mode strips the verbose stdout we'd need to parse.
 *      Tracked in TD-20260510-codex-mcp-sandbox-block — flip back
 *      when the upstream sandbox/approval gate is fixed.
 *
 *  Custom agents are treated as supporting — the user knows their
 *  setup. */
export function agentSupportsIntrospection(agentType: AgentType): boolean {
  return agentType !== 'Codex';
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
    Ollama: agentAccess.ollama?.full_access,
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

/** A briefing discussion is created by the backend with a localized
 *  title. Pre-fix the per-page detector used `startsWith('Briefing')`
 *  which only matched FR (`Briefing projet`) and ES (`Briefing del
 *  proyecto`); EN's `Project Briefing` was missed, so English users
 *  saw none of the briefing-specific UI (Zap icon, completion CTA,
 *  refetch-on-open effect). `includes` covers all three localized
 *  shapes and is safe — no other system-created title contains the
 *  word "Briefing". */
export const isBriefingDisc = (title: string) => title.includes('Briefing');

/** A bootstrap discussion always opens with the literal `Bootstrap: `
 *  prefix on every locale (the backend hard-codes the string and
 *  appends the project name). Using `startsWith` keeps user-named
 *  discussions like "About bootstrap testing" out of this branch. */
export const isBootstrapDisc = (title: string) => title.startsWith('Bootstrap: ');
