import { describe, expect, it } from 'vitest';
import { comparableOutput, mergeTally, readableOutput, shadowLabel, shadowName } from '../shadowRuns';
import type { ShadowExecution, ShadowMergeReport } from '../../types/generated';

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

describe('the merge read point by point', () => {
  const point = (id: string, verdict: string, found_by: string[], duplicate_of?: string) => ({ id, summary: id, verdict, found_by, verified: false, duplicate_of });
  // The shape of a real review merge: three points found by all, one by main and reworded by both shadows.
  const report = {
    points: [
      point('p1', 'kept', ['main', 'shadow:a', 'shadow:b']),
      point('p2', 'kept', ['main', 'shadow:a', 'shadow:b']),
      point('p3', 'kept', ['main']),
      point('p4', 'kept', ['main']),
      point('p5', 'duplicate', ['shadow:a', 'shadow:b'], 'p4'),
      point('p6', 'duplicate', ['shadow:b'], 'p4'),
      point('p7', 'dropped_unverified', ['shadow:a', 'shadow:b']),
      point('p8', 'dropped_unverified', ['shadow:a']),
      point('p9', 'kept', ['shadow:b']),
    ],
    contributions: ['main', 'shadow:a', 'shadow:b'].map(execution => ({ execution, kept: 0, unique: 0, duplicate: 0, dropped_unverified: 0 })),
  } as unknown as ShadowMergeReport;
  const tally = mergeTally(report);
  const row = (execution: string) => tally.rows.find(candidate => candidate.execution === execution)!;

  it('credits a point of the result to every execution that found it', () => {
    expect(row('main')).toMatchObject({ raised: 4, inResult: 4, alone: 1, dropped: 0 });
    expect(row('shadow:a')).toMatchObject({ raised: 5, inResult: 3, alone: 0, dropped: 2 });
  });

  it('counts a point reworded twice by one execution once, and what a shadow found alone', () => {
    expect(row('shadow:b')).toMatchObject({ raised: 5, inResult: 4, alone: 1, dropped: 1 });
  });

  it('every row adds up, and the summary counts distinct points', () => {
    for (const each of tally.rows) expect(each.raised).toBe(each.inResult + each.dropped);
    expect(tally).toMatchObject({ result: 5, alone: 2, dropped: 2 });
  });

  it('a rewording of a point that was left out is left out too', () => {
    const orphan = { ...report, points: [point('p1', 'dropped_unverified', ['main']), point('p2', 'duplicate', ['shadow:a'], 'p1')] } as unknown as ShadowMergeReport;
    expect(mergeTally(orphan)).toMatchObject({ result: 0, dropped: 2 });
  });
});
