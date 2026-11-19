import { describe, expect, it } from 'vitest';
import { comparableOutput, readableOutput, shadowLabel, shadowName } from '../shadowRuns';
import type { ShadowExecution } from '../../types/generated';

describe('shadow outputs compared field by field (key order ignored)', () => {
  const main = '{"data":{"pr":2084,"verdict":"approve","blockers":[{"path":"a.ts","line":3}]},"status":"OK"}';
  const reordered = '{"status":"OK","data":{"blockers":[{"line":3,"path":"a.ts"}],"verdict":"approve","pr":2084}}';

  it('two outputs with the same fields in another order are the same', () => {
    expect(comparableOutput(reordered)).toBe(comparableOutput(main));
    expect(readableOutput(reordered)).toBe(readableOutput(main));
  });

  it('shows the keys in alphabetical order at every depth', () => {
    expect(readableOutput(reordered)).toBe([
      '{',
      '  "data": {',
      '    "blockers": [',
      '      {',
      '        "line": 3,',
      '        "path": "a.ts"',
      '      }',
      '    ],',
      '    "pr": 2084,',
      '    "verdict": "approve"',
      '  },',
      '  "status": "OK"',
      '}',
    ].join('\n'));
  });

  it('keeps the order of a list: it can carry meaning', () => {
    expect(comparableOutput('{"pistes":["a","b"]}')).not.toBe(comparableOutput('{"pistes":["b","a"]}'));
  });

  it('still tells a different value apart', () => {
    expect(comparableOutput(reordered.replace('approve', 'request_changes'))).not.toBe(comparableOutput(main));
  });
});

describe('an execution is named with the agent and model that answered', () => {
  const execution = (shadow_id: string | null, label: string, agent: string, model?: string) =>
    ({ shadow_id, label, identity: { agent, model } }) as unknown as ShadowExecution;

  it('adds the agent and the model after the name', () => {
    expect(shadowLabel(execution(null, '', 'ClaudeCode', 'claude-fable-5-2'), 'Main')).toBe('Main (ClaudeCode · claude-fable-5-2)');
    expect(shadowLabel(execution('shadow-2', '', 'LiteLlm', 'glm-5.2'), 'Main')).toBe('shadow-2 (LiteLlm · glm-5.2)');
    expect(shadowLabel(execution('shadow', 'Local qwen', 'Ollama'), 'Main')).toBe('Local qwen (Ollama)');
  });

  it('keeps a short name for sentences that already say who', () => {
    expect(shadowName(execution('shadow', 'Local qwen', 'Ollama', 'qwen3.8'), 'Main')).toBe('Local qwen');
  });
});
