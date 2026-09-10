import { describe, expect, it } from 'vitest';
import { agentSettingsForSelection } from '../agentSelection';

describe('agentSettingsForSelection', () => {
  it('clears only model-specific choices and pins the exact requested connection', () => {
    const original = { model: 'old-model', reasoning_effort: 'xhigh', max_tokens: 12345, connection_id: 'one', tier: 'default' as const };
    expect(agentSettingsForSelection(original, 'reasoning', 'two')).toEqual({
      model: null, reasoning_effort: null, max_tokens: 12345, connection_id: 'two', tier: 'reasoning',
    });
    expect(original).toEqual({ model: 'old-model', reasoning_effort: 'xhigh', max_tokens: 12345, connection_id: 'one', tier: 'default' });
  });

  it('represents a native selection explicitly without inventing a model or token limit', () => {
    expect(agentSettingsForSelection(null, 'economy')).toEqual({
      model: null, reasoning_effort: null, connection_id: null, tier: 'economy',
    });
  });
});
