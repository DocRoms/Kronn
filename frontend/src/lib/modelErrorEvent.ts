const MODEL_ERROR_PREFIX = '[kronn:model-error]\n';

export interface ModelErrorEvent {
  kind: 'model_error';
  status: number;
  summary: string;
  detail: string;
  tier: 'economy' | 'default' | 'reasoning';
}

export function parseModelErrorEvent(content: string): ModelErrorEvent | null {
  if (!content.startsWith(MODEL_ERROR_PREFIX)) return null;
  try {
    const parsed = JSON.parse(content.slice(MODEL_ERROR_PREFIX.length)) as Partial<ModelErrorEvent>;
    if (parsed.kind !== 'model_error'
      || typeof parsed.status !== 'number'
      || typeof parsed.summary !== 'string'
      || typeof parsed.detail !== 'string'
      || !['economy', 'default', 'reasoning'].includes(parsed.tier ?? '')) return null;
    return parsed as ModelErrorEvent;
  } catch {
    return null;
  }
}
