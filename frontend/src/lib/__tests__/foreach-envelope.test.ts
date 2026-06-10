import { describe, it, expect } from 'vitest';
import { parseForeachEnvelope, isZeroTokenItem } from '../foreach-envelope';

const envelope = (data: unknown) =>
  `child finished\n---STEP_OUTPUT---\n${JSON.stringify({ data, status: 'OK', summary: 's' })}\n---END_STEP_OUTPUT---`;

describe('parseForeachEnvelope', () => {
  it('parses a foreach aggregate with mixed item statuses', () => {
    const out = envelope({
      mode: 'foreach',
      total: 3,
      succeeded: 3,
      failed: 0,
      items: [
        { item: 0, id: 'scss-tokens', status: 'MechanicalApplied', child_run_id: null, files: 1 },
        { item: 1, id: 'brand-context', status: 'Success', child_run_id: 'abc12345-0000' },
        { item: 2, id: 'twig-loader', status: 'Skipped(committed)', child_run_id: null },
      ],
    });
    const f = parseForeachEnvelope(out)!;
    expect(f.total).toBe(3);
    expect(f.failed).toBe(0);
    expect(f.items.map(i => i.id)).toEqual(['scss-tokens', 'brand-context', 'twig-loader']);
    expect(f.items[0].child_run_id).toBeNull();
    expect(f.items[1].child_run_id).toBe('abc12345-0000');
  });

  it('reports partial failures (the PARTIAL=Success engine case)', () => {
    const out = envelope({
      mode: 'foreach', total: 2, succeeded: 1, failed: 1,
      items: [
        { item: 0, id: 'ok-item', status: 'Success', child_run_id: 'r1' },
        { item: 1, id: 'bad-item', status: 'Failed', child_run_id: 'r2' },
      ],
    });
    const f = parseForeachEnvelope(out)!;
    expect(f.failed).toBe(1);
    expect(f.items[1].status).toBe('Failed');
  });

  it('parses when a child output embedded a nested ---END_STEP_OUTPUT--- marker (run-15 regression)', () => {
    // The foreach envelope's `last_output` field embeds a child step's output
    // that itself contains ---END_STEP_OUTPUT---. Slicing at the FIRST end
    // marker cut the JSON → table vanished. Must slice at the OUTER (last) end.
    const inner = 'drift done\\n---STEP_OUTPUT---\\n{"data":{},"status":"OK","summary":"exit 0"}\\n---END_STEP_OUTPUT---';
    const out = `---STEP_OUTPUT---\n${JSON.stringify({ data: { mode: 'foreach', total: 2, succeeded: 2, failed: 0, last_output: inner, items: [ { item: 0, id: 'a', status: 'Success', child_run_id: 'r1' }, { item: 1, id: 'b', status: 'MechanicalApplied', child_run_id: null } ] }, status: 'OK', summary: 's' })}\n---END_STEP_OUTPUT---\n[SIGNAL: OK]`;
    const f = parseForeachEnvelope(out)!;
    expect(f).not.toBeNull();
    expect(f.total).toBe(2);
    expect(f.items.map(i => i.id)).toEqual(['a', 'b']);
  });

  it('returns null for non-foreach outputs', () => {
    // single-child SubWorkflow envelope (no mode/items)
    expect(parseForeachEnvelope(envelope({ child_run_id: 'x', child_status: 'Success' }))).toBeNull();
    // agent free text
    expect(parseForeachEnvelope('All done [SIGNAL: CONTINUE]')).toBeNull();
    // broken JSON after the marker
    expect(parseForeachEnvelope('---STEP_OUTPUT---\n{not json')).toBeNull();
    expect(parseForeachEnvelope(null)).toBeNull();
    expect(parseForeachEnvelope('')).toBeNull();
  });

  it('survives missing optional fields with safe fallbacks', () => {
    const out = envelope({ mode: 'foreach', items: [{ status: 'Success' }, { id: 'x', status: 'Failed' }] });
    const f = parseForeachEnvelope(out)!;
    expect(f.total).toBe(2);
    expect(f.failed).toBe(1);
    expect(f.items[0].id).toBe('item-0');
  });
});

describe('isZeroTokenItem', () => {
  it('flags engine-applied and skip-if-committed items', () => {
    expect(isZeroTokenItem('MechanicalApplied')).toBe(true);
    expect(isZeroTokenItem('Skipped(committed)')).toBe(true);
    expect(isZeroTokenItem('Success')).toBe(false);
    expect(isZeroTokenItem('Failed')).toBe(false);
  });
});
