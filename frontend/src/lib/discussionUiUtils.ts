import type { Discussion } from '../types/generated';

export function sourceAgentShortLabel(sourceAgent: string): string {
  const known: Record<string, string> = {
    ClaudeCode: 'CC',
    Codex: 'CX',
    GeminiCli: 'GM',
    Copilot: 'CP',
    Kiro: 'KI',
    Vibe: 'VI',
    Ollama: 'OL',
    Cursor: 'CU',
  };
  return known[sourceAgent] ?? sourceAgent.slice(0, 2).toUpperCase();
}

export function unseenBasis(
  discussion: Pick<Discussion, 'message_count' | 'messages' | 'non_system_message_count'>,
): number {
  if (typeof discussion.non_system_message_count === 'number') {
    return discussion.non_system_message_count;
  }
  if (discussion.messages?.length) {
    return discussion.messages.filter(message => message.role !== 'System').length;
  }
  return discussion.message_count ?? 0;
}
