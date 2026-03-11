// ─── Shared constants across pages ──────────────────────────────────────────

import type { AgentType } from '../types/generated';

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
