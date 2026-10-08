import type { SharedRun } from '../types/generated';
import { automationPath, discussionPath, workflowPath, type AutomationTab } from './routes';

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
  execDetails?: SharedRun['exec_details'];
  freshness?: 'live' | 'rehydrated' | 'unavailable';
  href?: string | null;
};

// The Automation tab a source of each kind lives in. A media run has no page
// of its own, so it has no address.
const RUN_SOURCE_TABS: Partial<Record<RunStatusCardKind, AutomationTab>> = {
  quick_prompt: 'quickPrompts',
  quick_api: 'quickApis',
  quick_exec: 'quickExecs',
};

/** Where a run is read: its discussion, else its workflow with the run revealed, else its source. */
export function sharedRunAddress(run: Pick<SharedRun, 'id' | 'kind' | 'source_id' | 'discussion_id'>): string | null {
  if (run.discussion_id) return discussionPath(run.discussion_id);
  if (run.kind === 'workflow') return workflowPath(run.source_id, run.id);
  const tab = RUN_SOURCE_TABS[run.kind];
  return tab ? automationPath({ tab, resourceId: run.source_id }) : null;
}

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
    execDetails: run.exec_details,
    freshness,
    href: sharedRunAddress(run),
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
