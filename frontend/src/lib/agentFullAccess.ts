import type { AgentsConfig, AgentType } from '../types/generated';

type AgentConfigKey = {
  [K in keyof AgentsConfig]: AgentsConfig[K] extends { full_access: boolean } ? K : never
}[keyof AgentsConfig];

interface FullAccessSupport {
  key: AgentConfigKey;
  /** Literal CLI flag, when the agent has one (OpenCode widens ACP approvals instead). */
  flag?: string;
  descKey: string;
}

// Mirrors the backend `AgentsConfig::set_full_access`: Vibe and Kiro ignore the
// flag at launch, so they are not offered.
export const FULL_ACCESS_AGENTS: Partial<Record<AgentType, FullAccessSupport>> = {
  ClaudeCode: { key: 'claude_code', flag: '--dangerously-skip-permissions', descKey: 'config.fullAccess' },
  Codex: { key: 'codex', flag: '--sandbox=danger-full-access', descKey: 'config.fullAccess' },
  GeminiCli: { key: 'gemini_cli', flag: '--yolo', descKey: 'config.fullAccess' },
  CopilotCli: { key: 'copilot_cli', flag: '--allow-all-tools', descKey: 'config.fullAccess' },
  OpenCode: { key: 'open_code', descKey: 'config.fullAccessAcp' },
};

export function supportsFullAccess(agent: AgentType): boolean {
  return FULL_ACCESS_AGENTS[agent] !== undefined;
}

export function isFullAccess(config: AgentsConfig | null | undefined, agent: AgentType): boolean {
  const support = FULL_ACCESS_AGENTS[agent];
  return support ? config?.[support.key]?.full_access ?? false : false;
}
