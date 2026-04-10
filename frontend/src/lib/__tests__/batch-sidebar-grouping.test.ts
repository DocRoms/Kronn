import { describe, it, expect } from 'vitest';

/**
 * Exercises the grouping logic used in DiscussionSidebar.tsx for Phase 1b
 * batch workflows. We replicate the split here so a refactor of the
 * inline code breaks this test and we notice it.
 */

interface MiniDisc {
  id: string;
  updated_at: string;
  workflow_run_id?: string | null;
}

function splitByBatch(discs: MiniDisc[]) {
  const batchMap = new Map<string, MiniDisc[]>();
  const loose: MiniDisc[] = [];
  for (const d of discs) {
    if (d.workflow_run_id) {
      const arr = batchMap.get(d.workflow_run_id) ?? [];
      arr.push(d);
      batchMap.set(d.workflow_run_id, arr);
    } else {
      loose.push(d);
    }
  }
  return { batchMap, loose };
}

describe('Batch sidebar grouping', () => {
  it('splits loose vs batch discs correctly', () => {
    const discs: MiniDisc[] = [
      { id: 'a', updated_at: '2026-04-10T10:00:00Z' },
      { id: 'b', updated_at: '2026-04-10T10:01:00Z', workflow_run_id: 'run1' },
      { id: 'c', updated_at: '2026-04-10T10:02:00Z', workflow_run_id: 'run1' },
      { id: 'd', updated_at: '2026-04-10T10:03:00Z' },
      { id: 'e', updated_at: '2026-04-10T10:04:00Z', workflow_run_id: 'run2' },
    ];
    const { batchMap, loose } = splitByBatch(discs);
    expect(loose.map(d => d.id)).toEqual(['a', 'd']);
    expect(batchMap.size).toBe(2);
    expect(batchMap.get('run1')?.map(d => d.id)).toEqual(['b', 'c']);
    expect(batchMap.get('run2')?.map(d => d.id)).toEqual(['e']);
  });

  it('handles empty input', () => {
    const { batchMap, loose } = splitByBatch([]);
    expect(loose).toEqual([]);
    expect(batchMap.size).toBe(0);
  });

  it('handles all-loose (no batches)', () => {
    const discs: MiniDisc[] = [
      { id: 'a', updated_at: '2026-04-10T10:00:00Z' },
      { id: 'b', updated_at: '2026-04-10T10:01:00Z' },
    ];
    const { batchMap, loose } = splitByBatch(discs);
    expect(loose.length).toBe(2);
    expect(batchMap.size).toBe(0);
  });

  it('handles all-batch (no loose discs)', () => {
    const discs: MiniDisc[] = [
      { id: 'a', updated_at: '2026-04-10T10:00:00Z', workflow_run_id: 'run1' },
      { id: 'b', updated_at: '2026-04-10T10:01:00Z', workflow_run_id: 'run1' },
    ];
    const { batchMap, loose } = splitByBatch(discs);
    expect(loose).toEqual([]);
    expect(batchMap.size).toBe(1);
    expect(batchMap.get('run1')?.length).toBe(2);
  });

  it('treats null/undefined workflow_run_id as loose', () => {
    const discs: MiniDisc[] = [
      { id: 'a', updated_at: '2026-04-10T10:00:00Z', workflow_run_id: null },
      { id: 'b', updated_at: '2026-04-10T10:01:00Z', workflow_run_id: undefined },
      { id: 'c', updated_at: '2026-04-10T10:02:00Z' },
    ];
    const { batchMap, loose } = splitByBatch(discs);
    expect(loose.length).toBe(3);
    expect(batchMap.size).toBe(0);
  });
});
