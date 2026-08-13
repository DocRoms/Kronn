const AGENT_ERROR_PREFIX = '[kronn:agent-error]\n';
const LEGACY_MODEL_ERROR_PREFIX = '[kronn:model-error]\n';

export interface AgentErrorEvent {
  kind: 'model_error' | 'agent_error';
  status: number | null;
  summary: string;
  detail: string;
  tier: 'economy' | 'default' | 'reasoning';
  retry_dispatch_id: string | null;
  retried: boolean;
}

export function parseAgentErrorEvent(content: string): AgentErrorEvent | null {
  const payload = content.startsWith(AGENT_ERROR_PREFIX)
    ? content.slice(AGENT_ERROR_PREFIX.length)
    : content.startsWith(LEGACY_MODEL_ERROR_PREFIX)
      ? content.slice(LEGACY_MODEL_ERROR_PREFIX.length)
      : null;
  if (payload === null) return null;
  try {
    const parsed = JSON.parse(payload) as Partial<AgentErrorEvent>;
    if (!['model_error', 'agent_error'].includes(parsed.kind ?? '')
      || !(parsed.status === null || typeof parsed.status === 'number')
      || typeof parsed.summary !== 'string'
      || typeof parsed.detail !== 'string'
      || !['economy', 'default', 'reasoning'].includes(parsed.tier ?? '')) return null;
    return {
      ...parsed,
      retry_dispatch_id: typeof parsed.retry_dispatch_id === 'string'
        ? parsed.retry_dispatch_id
        : null,
      retried: parsed.retried === true,
    } as AgentErrorEvent;
  } catch {
    return null;
  }
}

export const parseModelErrorEvent = parseAgentErrorEvent;
