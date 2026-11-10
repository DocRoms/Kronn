import { describe, expect, it } from 'vitest';
import {
  DEFAULT_HEAVY_OUTPUT_KB,
  collectSourceWeights,
  isHeavyOutput,
  stepOutputBytes,
} from '../heavyOutput';

describe('isHeavyOutput', () => {
  it('defaults to 256 KB and flags only what is strictly above the threshold', () => {
    expect(DEFAULT_HEAVY_OUTPUT_KB).toBe(256);
    expect(isHeavyOutput(256 * 1024, 256)).toBe(false);
    expect(isHeavyOutput(256 * 1024 + 1, 256)).toBe(true);
  });

  it('follows a configured threshold', () => {
    expect(isHeavyOutput(2048, 1)).toBe(true);
    expect(isHeavyOutput(2048, 4)).toBe(false);
  });

  it('never flags an unknown size', () => {
    expect(isHeavyOutput(null, 1)).toBe(false);
    expect(isHeavyOutput(undefined, 1)).toBe(false);
  });
});

describe('stepOutputBytes', () => {
  it('prefers the stamped size and falls back to the UTF-8 length of the output', () => {
    expect(stepOutputBytes({ output_bytes: 42, output: 'abc' })).toBe(42);
    expect(stepOutputBytes({ output_bytes: 0, output: 'abc' })).toBe(0);
    expect(stepOutputBytes({ output: 'é' })).toBe(2);
    expect(stepOutputBytes({ output_bytes: null, output: 'abc' })).toBe(3);
  });
});

describe('collectSourceWeights', () => {
  const envelope = (data: unknown) => `---STEP_OUTPUT---\n${JSON.stringify({ data })}\n---END_STEP_OUTPUT---`;

  it('reads each source\'s payload size from the output envelope', () => {
    const output = envelope({ meta: { sources: [
      { alias: 'jira', output_bytes: 1000 },
      { alias: 'usage', output_bytes: 12 },
    ] } });
    expect(collectSourceWeights(output)).toEqual([
      { alias: 'jira', bytes: 1000 },
      { alias: 'usage', bytes: 12 },
    ]);
  });

  it('ignores a run recorded before the size existed, and any output that is not an envelope', () => {
    expect(collectSourceWeights(envelope({ meta: { sources: [{ alias: 'jira' }] } }))).toEqual([]);
    expect(collectSourceWeights(envelope({ sources: {} }))).toEqual([]);
    expect(collectSourceWeights('plain text')).toEqual([]);
    expect(collectSourceWeights('---STEP_OUTPUT---{not json')).toEqual([]);
    expect(collectSourceWeights(null)).toEqual([]);
  });
});
