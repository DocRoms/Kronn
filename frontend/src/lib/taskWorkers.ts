// KT-1632 — the cap on local task workers running at once (Settings > Workers).
// A launch beyond it waits `Queued`; the backend leaves the reason on the
// execution's progress as `task_worker_limit:<running>/<cap>`.

/** Smallest and largest cap the backend accepts. */
export const MIN_TASK_WORKERS = 1;
export const MAX_TASK_WORKERS = 64;
/** Out-of-the-box default, mirrored from `DEFAULT_MAX_CONCURRENT_TASK_WORKERS`. */
export const DEFAULT_TASK_WORKERS = 2;

const QUEUE_REASON = /^task_worker_limit:(\d+)\/(\d+)$/;

export interface TaskWorkerQueue {
  running: number;
  cap: number;
}

/** The queue a progress reason describes, or `null` when it is not the cap. */
export function parseTaskWorkerQueue(reason: string | null | undefined): TaskWorkerQueue | null {
  const match = reason ? QUEUE_REASON.exec(reason) : null;
  return match ? { running: Number(match[1]), cap: Number(match[2]) } : null;
}
