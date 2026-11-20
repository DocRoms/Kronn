// The overview the bell polls, shared with what else counts running work (the
// menu badges), so two places never count the same thing two ways.
import { useSyncExternalStore } from 'react';
import type { ActivityOverview } from '../types/generated';

let shared: ActivityOverview | null = null;
const listeners = new Set<() => void>();

/** The bell read a new overview. */
export function publishActivity(overview: ActivityOverview | null): void {
  if (shared === overview) return;
  shared = overview;
  for (const listener of [...listeners]) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

const read = () => shared;

export function useSharedActivity(): ActivityOverview | null {
  return useSyncExternalStore(subscribe, read, read);
}

/** The workflow runs in progress as the bell lists them: a sub-workflow shows
 *  through its parent. `fallback` stands until the first overview is read. */
export function runningWorkflowRuns(overview: ActivityOverview | null, fallback: number): number {
  if (!overview) return fallback;
  return overview.rows.filter(row => row.type === 'workflow_run' && row.compartment === 'running').length;
}
