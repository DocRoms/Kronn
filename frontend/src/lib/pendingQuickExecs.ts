// KT-1695 — the Quick Execs a workflow waits on, listed once each: a Quick
// Exec several steps use is one thing to activate, with the steps that use it.
import type { BlockedQuickExec, WorkflowBlocker } from '../types/generated';

export interface PendingQuickExecGroup {
  quickExec: BlockedQuickExec;
  /** The steps that use it, in workflow order. */
  steps: string[];
}

/** The blockers that name a Quick Exec, grouped by Quick Exec. */
export function groupQuickExecBlockers(blockers: ReadonlyArray<WorkflowBlocker>): PendingQuickExecGroup[] {
  const groups = new Map<string, PendingQuickExecGroup>();
  for (const blocker of blockers) {
    if (!blocker.quick_exec) continue;
    const group = groups.get(blocker.quick_exec.id)
      ?? { quickExec: blocker.quick_exec, steps: [] };
    const step = blocker.step ?? blocker.workflow_name;
    if (!group.steps.includes(step)) group.steps.push(step);
    groups.set(blocker.quick_exec.id, group);
  }
  return [...groups.values()];
}
