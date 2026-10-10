import { describe, it, expect } from 'vitest';
import { filterRuns, runMatchesSearch, runMatchesStatusFilter, RUN_PAGE_SIZE, groupRunsByParent, foldNoOpRuns } from '../runFilters';
import type { WorkflowRun } from '../../types/generated';

const mk = (over: Partial<WorkflowRun>): WorkflowRun => ({
  id: 'run-x', workflow_id: 'wf', status: 'Success', trigger_context: null,
  step_results: [], tokens_used: 0, workspace_path: null,
  started_at: '2026-07-06T06:13:00Z', finished_at: null,
  run_type: 'linear', batch_total: 0, batch_completed: 0, batch_failed: 0, batch_no_response: 0,
  batch_name: null, parent_run_id: null, state: {}, produced_branches: [], ...over,
});

describe('runFilters — status', () => {
  it('all passes everything; specific filters match exactly', () => {
    const failed = mk({ status: 'Failed' });
    const partial = mk({ status: 'Partial' });
    const ok = mk({ status: 'Success' });
    const waiting = mk({ status: 'WaitingApproval' });
    const guard = mk({ status: 'StoppedByGuard' });
    expect([failed, ok, waiting, guard].every(r => runMatchesStatusFilter(r, 'all'))).toBe(true);
    expect(runMatchesStatusFilter(failed, 'failed')).toBe(true);
    expect(runMatchesStatusFilter(partial, 'failed')).toBe(true);
    expect(runMatchesStatusFilter(ok, 'failed')).toBe(false);
    expect(runMatchesStatusFilter(waiting, 'waiting')).toBe(true);
    expect(runMatchesStatusFilter(guard, 'stopped')).toBe(true);
  });
});

describe('runFilters — search', () => {
  it('empty query matches all', () => {
    expect(runMatchesSearch(mk({}), '  ')).toBe(true);
  });
  it('matches run id, parent WF name, and step output (e.g. a PR number)', () => {
    const run = mk({
      id: 'abc12345-run',
      parent_workflow_name: 'PR Review cron v2',
      step_results: [{ step_name: 'reason', status: 'Success', output: 'review of PR #1841', tokens_used: 0, duration_ms: 1, is_rollback: false }],
    });
    expect(runMatchesSearch(run, 'abc123')).toBe(true);
    expect(runMatchesSearch(run, 'cron v2')).toBe(true);
    expect(runMatchesSearch(run, '1841')).toBe(true);
    expect(runMatchesSearch(run, 'nope')).toBe(false);
  });
});

describe('runFilters — combined', () => {
  it('applies status AND search together', () => {
    const runs = [
      mk({ id: 'a', status: 'Failed', step_results: [{ step_name: 's', status: 'Failed', output: 'PR #10', tokens_used: 0, duration_ms: 1, is_rollback: false }] }),
      mk({ id: 'b', status: 'Success', step_results: [{ step_name: 's', status: 'Success', output: 'PR #10', tokens_used: 0, duration_ms: 1, is_rollback: false }] }),
      mk({ id: 'c', status: 'Failed', step_results: [{ step_name: 's', status: 'Failed', output: 'PR #99', tokens_used: 0, duration_ms: 1, is_rollback: false }] }),
    ];
    const out = filterRuns(runs, 'failed', '10');
    expect(out.map(r => r.id)).toEqual(['a']);
  });
  it('RUN_PAGE_SIZE is a sane fold threshold', () => {
    expect(RUN_PAGE_SIZE).toBeGreaterThan(0);
  });
});

describe('groupRunsByParent', () => {
  it('groups contiguous same-parent sub-runs, keeps standalone runs separate', () => {
    const runs = [
      mk({ id: 'a', parent_run_id: 'tick1', parent_workflow_name: 'Cron', parent_run_started_at: '2026-07-06T07:00:00Z' }),
      mk({ id: 'b', parent_run_id: 'tick1', parent_workflow_name: 'Cron' }),
      mk({ id: 'c', parent_run_id: 'tick1', parent_workflow_name: 'Cron' }),
      mk({ id: 'solo' }), // no parent
      mk({ id: 'd', parent_run_id: 'tick2', parent_workflow_name: 'Cron' }),
    ];
    const groups = groupRunsByParent(runs);
    expect(groups.map(g => g.key)).toEqual(['tick1', 'solo', 'tick2']);
    expect(groups[0].runs.map(r => r.id)).toEqual(['a', 'b', 'c']);
    expect(groups[0].parentName).toBe('Cron');
    expect(groups[0].tickAt).toBe('2026-07-06T07:00:00Z');
    expect(groups[1].parentRunId).toBeNull();
    expect(groups[1].runs).toHaveLength(1);
    expect(groups[2].runs.map(r => r.id)).toEqual(['d']);
  });

  it('returns an empty array for no runs', () => {
    expect(groupRunsByParent([])).toEqual([]);
  });
});

describe('runFilters — runs without changes (KT-1100)', () => {
  const at = (id: string, minute: number, extra: Partial<WorkflowRun> = {}) =>
    mk({ id, started_at: `2026-10-09T08:${String(minute).padStart(2, '0')}:00Z`, ...extra });

  it('folds a streak of two or more and keeps its oldest start', () => {
    const runs = [
      at('a', 9, { outcome: 'no_op' }),
      at('b', 8, { outcome: 'no_op' }),
      at('c', 7, { outcome: 'changed' }),
      at('d', 6, { outcome: 'no_op' }),
    ];
    const items = foldNoOpRuns(groupRunsByParent(runs));
    expect(items.map(i => i.kind)).toEqual(['noop', 'group', 'group']);
    const fold = items[0];
    expect(fold.kind === 'noop' && fold.runs.map(r => r.id)).toEqual(['a', 'b']);
    expect(fold.kind === 'noop' && fold.since).toBe('2026-10-09T08:08:00Z');
  });

  it('never folds sub-runs of a parent tick or unclassified runs', () => {
    const runs = [
      at('child-1', 9, { outcome: 'no_op', parent_run_id: 'p' }),
      at('child-2', 8, { outcome: 'no_op', parent_run_id: 'p' }),
      at('legacy-1', 7),
      at('legacy-2', 6),
    ];
    const items = foldNoOpRuns(groupRunsByParent(runs));
    expect(items.every(i => i.kind === 'group')).toBe(true);
    expect(items).toHaveLength(3);
  });
});
