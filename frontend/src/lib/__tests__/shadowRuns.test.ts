import { describe, expect, it } from 'vitest';
import { comparableOutput, readableOutput } from '../shadowRuns';

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
