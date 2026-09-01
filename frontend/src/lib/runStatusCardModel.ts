import type { SharedRun } from '../types/generated';

// `media` is ONE kind for both image and video: the execution family is
// identical and only the output differs, so the modality is read from
// `result.modality` rather than split into two kinds.
export type RunStatusCardKind = 'quick_prompt' | 'quick_api' | 'quick_exec' | 'workflow' | 'media';
export type RunStatusCardStatus =
  | 'preflight_failed'
  | 'queued'
  | 'running'
  | 'success'
  | 'partial'
  | 'failed'
  | 'cancelled'
  | 'timeout';

export type RunStatusCardProgress = {
  completed: number;
  total: number;
  currentLabel?: string | null;
};

/** A server-derived run projection. Optional fields deliberately remain absent
 * until measured by the source; consumers must not infer them client-side. */
export type RunStatusCardModel = {
  id: string;
  kind: RunStatusCardKind;
  status: RunStatusCardStatus;
  startedAt?: string | null;
  finishedAt?: string | null;
  durationMs?: number | null;
  progress?: RunStatusCardProgress | null;
  result?: unknown;
  diagnostic?: string | null;
  freshness?: 'live' | 'rehydrated' | 'unavailable';
  href?: string | null;
};

/** The single SharedRun -> RunStatusCardModel projection. Every consumer must
 * reuse this mapper so href and progress semantics cannot drift. */
export function sharedRunStatusCardModel(
  run: SharedRun,
  freshness: RunStatusCardModel['freshness'] = 'live',
): RunStatusCardModel {
  const result = run.result as {
    progress?: { completed: number; total: number; current_label?: string | null };
  } | null;
  return {
    id: run.id,
    kind: run.kind,
    status: run.status,
    startedAt: run.started_at,
    finishedAt: run.finished_at,
    durationMs: run.duration_ms,
    progress: result?.progress
      ? { ...result.progress, currentLabel: result.progress.current_label }
      : null,
    result: run.result,
    diagnostic: run.diagnostic,
    freshness,
    href: run.discussion_id
      ? `/discussions/${run.discussion_id}`
      : run.kind === 'workflow'
        ? `/workflows/${run.source_id}?run=${run.id}`
        : `/workflows?kind=${run.kind}&source=${run.source_id}&run=${run.id}`,
  };
}

export function workflowRunStatusCardModel(run: {
  id: string;
  status: string;
  started_at: string;
  finished_at: string | null;
  step_results: Array<{ status: string; step_name: string }>;
}): RunStatusCardModel {
  const statuses: Record<string, RunStatusCardStatus> = {
    Pending: 'queued',
    Running: 'running',
    Success: 'success',
    Partial: 'partial',
    Failed: 'failed',
    Cancelled: 'cancelled',
    StoppedByGuard: 'timeout',
    Interrupted: 'failed',
    WaitingApproval: 'running',
  };
  const completed = run.step_results.filter(
    step => !['Pending', 'Running', 'WaitingApproval'].includes(step.status),
  ).length;
  const current = run.step_results.find(step =>
    ['Pending', 'Running', 'WaitingApproval'].includes(step.status),
  );
  return {
    id: run.id,
    kind: 'workflow',
    status: statuses[run.status] ?? 'failed',
    startedAt: run.started_at,
    finishedAt: run.finished_at,
    progress:
      run.step_results.length > 0
        ? {
            completed,
            total: run.step_results.length,
            currentLabel: current?.step_name,
          }
        : null,
    freshness: 'rehydrated',
  };
}
