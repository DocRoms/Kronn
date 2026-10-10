// Pure run-list filtering for the workflow run history. Extracted from
// WorkflowDetail so the (status filter + free-text search) logic is unit-tested
// without rendering. All in-memory over the runs already fetched — zero backend.
import type { WorkflowRun } from '../types/generated';

export type RunStatusFilter = 'all' | 'failed' | 'waiting' | 'stopped';

/** How many runs to show before the "show older" fold kicks in. */
export const RUN_PAGE_SIZE = 10;

export function runMatchesStatusFilter(run: WorkflowRun, f: RunStatusFilter): boolean {
  switch (f) {
    case 'all': return true;
    case 'failed': return run.status === 'Failed' || run.status === 'Partial';
    case 'waiting': return run.status === 'WaitingApproval' || run.status === 'WaitingQuota';
    case 'stopped': return run.status === 'StoppedByGuard';
    default: return true;
  }
}

/** Free-text match across id, status, ISO date, parent WF name, and each step's
 *  name + output (so typing a PR number found in a step's output locates it). */
export function runMatchesSearch(run: WorkflowRun, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const hay: string[] = [
    run.id,
    run.status,
    run.started_at ?? '',
    run.parent_workflow_name ?? '',
    ...(run.step_results ?? []).map(s => `${s.step_name ?? ''} ${s.output ?? ''}`),
  ];
  return hay.some(h => h.toLowerCase().includes(q));
}

export function filterRuns(runs: WorkflowRun[], f: RunStatusFilter, query: string): WorkflowRun[] {
  return runs.filter(r => runMatchesStatusFilter(r, f) && runMatchesSearch(r, query));
}

/** A run and its sub-runs of the same parent tick, grouped for the accordion.
 *  `parentRunId === null` means a standalone run (its own single-item group). */
export interface RunGroup {
  key: string;
  parentRunId: string | null;
  parentName: string | null;
  tickAt: string | null;
  runs: WorkflowRun[];
}

/** Group CONTIGUOUS runs that share the same `parent_run_id` (i.e. spawned by
 *  the same parent tick). Runs are already sorted started_at DESC, so a tick's
 *  children are contiguous. Standalone runs (no parent) each form a 1-item
 *  group. Preserves the incoming order. */
export function groupRunsByParent(runs: WorkflowRun[]): RunGroup[] {
  const groups: RunGroup[] = [];
  for (const run of runs) {
    const pid = run.parent_run_id || null;
    const last = groups[groups.length - 1];
    if (pid && last && last.parentRunId === pid) {
      last.runs.push(run);
    } else {
      groups.push({
        key: pid ?? run.id,
        parentRunId: pid,
        parentName: run.parent_workflow_name ?? null,
        tickAt: run.parent_run_started_at ?? null,
        runs: [run],
      });
    }
  }
  return groups;
}

/** A row of the run list: a parent-tick group, or a streak of runs that
 *  changed nothing folded into one line (KT-1100). */
export type RunListItem =
  | { kind: 'group'; group: RunGroup }
  | { kind: 'noop'; key: string; runs: WorkflowRun[]; since: string };

export function isNoOpRun(run: WorkflowRun): boolean {
  return run.outcome === 'no_op';
}

/** Fold every streak of at least two consecutive standalone no-op runs into
 *  one row. `since` is the oldest start of the streak (runs come newest first). */
export function foldNoOpRuns(groups: RunGroup[]): RunListItem[] {
  const items: RunListItem[] = [];
  let streak: WorkflowRun[] = [];
  const flush = () => {
    if (streak.length >= 2) {
      items.push({ kind: 'noop', key: `noop-${streak[0].id}`, runs: streak, since: streak[streak.length - 1].started_at });
    } else {
      for (const run of streak) {
        items.push({ kind: 'group', group: { key: run.id, parentRunId: null, parentName: null, tickAt: null, runs: [run] } });
      }
    }
    streak = [];
  };
  for (const group of groups) {
    const lone = group.parentRunId === null && group.runs.length === 1 ? group.runs[0] : null;
    if (lone && isNoOpRun(lone)) {
      streak.push(lone);
      continue;
    }
    flush();
    items.push({ kind: 'group', group });
  }
  flush();
  return items;
}

/** Whether this automation is working right now.
 *
 *  `Pending` counts: a run that is claimed but not yet streaming is already
 *  the operator's answer to "which one is going?", and excluding it would
 *  leave the card blank for exactly the seconds they are looking. */
export function isWorkflowRunning(lastRunStatus: string | null | undefined): boolean {
  return lastRunStatus === 'Running' || lastRunStatus === 'Pending';
}
