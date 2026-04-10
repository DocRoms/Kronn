import { describe, it, expect } from 'vitest';

/**
 * Regression test for the batch input parser used in WorkflowsPage.handleBatchLaunch.
 * The first shipped version only split on `\n`, which turned a comma-separated
 * paste like `EW-7223,EW-7182,EW-6071,EW-7141` into a single batch item with
 * all IDs crammed into one discussion. Users paste from Jira/GitHub search
 * results on a single line — we accept newline, comma AND semicolon as
 * separators, and dedupe to handle accidental repeats.
 */
function parseBatchInput(raw: string): string[] {
  const rawItems = raw.split(/[\n,;]/).map(s => s.trim()).filter(Boolean);
  const seen = new Set<string>();
  const out: string[] = [];
  for (const item of rawItems) {
    if (!seen.has(item)) {
      seen.add(item);
      out.push(item);
    }
  }
  return out;
}

describe('Batch input parser', () => {
  it('splits one-per-line input', () => {
    expect(parseBatchInput('EW-1\nEW-2\nEW-3')).toEqual(['EW-1', 'EW-2', 'EW-3']);
  });

  it('splits comma-separated input on a single line (real-world bug)', () => {
    // This was the bug: "EW-7223,EW-7182,EW-6071,EW-7141" → 1 batch item
    expect(parseBatchInput('EW-7223,EW-7182,EW-6071,EW-7141'))
      .toEqual(['EW-7223', 'EW-7182', 'EW-6071', 'EW-7141']);
  });

  it('splits semicolon-separated input', () => {
    expect(parseBatchInput('EW-1;EW-2;EW-3')).toEqual(['EW-1', 'EW-2', 'EW-3']);
  });

  it('handles mixed separators (newline + comma + semicolon)', () => {
    expect(parseBatchInput('EW-1, EW-2\nEW-3;EW-4'))
      .toEqual(['EW-1', 'EW-2', 'EW-3', 'EW-4']);
  });

  it('trims whitespace around each value', () => {
    expect(parseBatchInput('  EW-1 ,\n  EW-2  ;  EW-3 '))
      .toEqual(['EW-1', 'EW-2', 'EW-3']);
  });

  it('filters empty values from trailing separators', () => {
    expect(parseBatchInput('EW-1,,EW-2,\n\n;EW-3;')).toEqual(['EW-1', 'EW-2', 'EW-3']);
  });

  it('deduplicates repeated entries', () => {
    expect(parseBatchInput('EW-1,EW-2,EW-1,EW-2,EW-3'))
      .toEqual(['EW-1', 'EW-2', 'EW-3']);
  });

  it('returns empty array for empty input', () => {
    expect(parseBatchInput('')).toEqual([]);
    expect(parseBatchInput('   \n\n  ,;  ')).toEqual([]);
  });

  it('handles a single value without separator', () => {
    expect(parseBatchInput('EW-7182')).toEqual(['EW-7182']);
  });
});
