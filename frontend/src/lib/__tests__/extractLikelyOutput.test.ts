import { describe, it, expect } from 'vitest';
import { extractLikelyOutput } from '../extractLikelyOutput';

describe('extractLikelyOutput', () => {
  // ── Empty / null-ish ────────────────────────────────────────────────
  it('returns empty for empty string', () => {
    expect(extractLikelyOutput('')).toEqual({ value: '', extracted: false });
  });

  it('returns empty for whitespace-only', () => {
    expect(extractLikelyOutput('   \n  \n  ')).toEqual({ value: '', extracted: false });
  });

  // ── Structured envelope ─────────────────────────────────────────────
  it('extracts JSON from STEP_OUTPUT envelope', () => {
    const raw = `Here is the analysis.\n---STEP_OUTPUT---\n{"data":{"tickets":["T-1","T-2"]},"status":"OK"}\n---END_STEP_OUTPUT---`;
    const result = extractLikelyOutput(raw);
    expect(result.extracted).toBe(true);
    expect(result.value).toContain('"tickets"');
    expect(result.value).not.toContain('---STEP_OUTPUT---');
  });

  it('extracts multiline content from envelope', () => {
    const raw = `Blabla\n---STEP_OUTPUT---\n{\n  "status": "OK",\n  "summary": "done"\n}\n---END_STEP_OUTPUT---\nTrailing text`;
    const result = extractLikelyOutput(raw);
    expect(result.extracted).toBe(true);
    expect(result.value).toContain('"status": "OK"');
  });

  // ── Single line → no extraction ─────────────────────────────────────
  it('returns single line as-is without extraction flag', () => {
    const result = extractLikelyOutput('PROJ-123');
    expect(result).toEqual({ value: 'PROJ-123', extracted: false });
  });

  // ── Last line heuristic ─────────────────────────────────────────────
  it('extracts last line if it contains commas (list-like)', () => {
    const raw = 'Here is my analysis of the tickets.\n\nThe priorities are:\nT-1, T-2, T-3';
    const result = extractLikelyOutput(raw);
    expect(result.extracted).toBe(true);
    expect(result.value).toBe('T-1, T-2, T-3');
  });

  it('extracts last line if it contains semicolons', () => {
    const raw = 'Analysis done.\nT-1; T-2; T-3';
    const result = extractLikelyOutput(raw);
    expect(result.extracted).toBe(true);
    expect(result.value).toBe('T-1; T-2; T-3');
  });

  it('extracts last line if it starts with JSON array bracket', () => {
    const raw = 'Found 3 items.\n["T-1","T-2","T-3"]';
    const result = extractLikelyOutput(raw);
    expect(result.extracted).toBe(true);
    expect(result.value).toBe('["T-1","T-2","T-3"]');
  });

  it('extracts last line if it starts with JSON object bracket', () => {
    const raw = 'Result:\n{"count": 3}';
    const result = extractLikelyOutput(raw);
    expect(result.extracted).toBe(true);
    expect(result.value).toBe('{"count": 3}');
  });

  it('extracts last line if short (< 200 chars)', () => {
    const raw = 'This is a very long analysis paragraph that goes on and on about the architecture of the system and the various trade-offs involved in the migration.\nDone.';
    const result = extractLikelyOutput(raw);
    expect(result.extracted).toBe(true);
    expect(result.value).toBe('Done.');
  });

  // ── Full fallback ───────────────────────────────────────────────────
  it('falls back to full text when last line is long prose', () => {
    const longLine = 'A'.repeat(250);
    const raw = `First paragraph.\n${longLine}`;
    const result = extractLikelyOutput(raw);
    expect(result.extracted).toBe(false);
    expect(result.value).toBe(raw);
  });

  // ── Edge cases ──────────────────────────────────────────────────────
  it('handles emoji in output', () => {
    const raw = 'Analyse terminee.\n🎯 T-1, T-2';
    const result = extractLikelyOutput(raw);
    expect(result.extracted).toBe(true);
    expect(result.value).toBe('🎯 T-1, T-2');
  });

  it('handles markdown link in last line (contains [ but not at start)', () => {
    const longMarkdown = 'A'.repeat(250) + ' [see docs](https://example.com)';
    const raw = `Intro.\n${longMarkdown}`;
    // Long line with no comma/semicolon and doesn't start with [ → fallback
    const result = extractLikelyOutput(raw);
    expect(result.extracted).toBe(false);
  });

  it('trims whitespace from envelope content', () => {
    const raw = '---STEP_OUTPUT---\n   {"ok": true}   \n---END_STEP_OUTPUT---';
    expect(extractLikelyOutput(raw).value).toBe('{"ok": true}');
  });

  it('ignores blank lines between content lines', () => {
    const raw = 'First\n\n\nSecond\n\n\nT-1, T-2';
    const result = extractLikelyOutput(raw);
    expect(result.extracted).toBe(true);
    expect(result.value).toBe('T-1, T-2');
  });
});
