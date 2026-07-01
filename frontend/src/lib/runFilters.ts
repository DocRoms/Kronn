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
    case 'failed': return run.status === 'Failed';
    case 'waiting': return run.status === 'WaitingApproval';
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
