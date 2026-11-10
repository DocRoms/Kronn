// KT-1657 — a run paused on a provider quota reads as paused, never as running.

import type { ActiveRunSummary } from '../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

/** Just what a label needs of a quota wait (`ActiveRunQuota` and `QuotaWait` both fit). */
export interface QuotaWakeInfo {
  wake_at?: string | null;
}

/**
 * "Paused — provider quota, resumes automatically at <time>", or the parked
 * wording when no wake-up is scheduled (a human resumes it). With no wait
 * known yet, the plain paused label: still never "running".
 */
export function quotaPauseLabel(t: Translate, quota?: QuotaWakeInfo | null): string {
  if (!quota) return t('wf.quota.paused');
  if (quota.wake_at) return t('wf.quota.pausedUntil', new Date(quota.wake_at).toLocaleString());
  return t('wf.quota.pausedParked');
}

/** A sub-workflow child is resumed through its parent: that is the run to resume. */
export function quotaResumeTarget(run: Pick<ActiveRunSummary, 'run_id' | 'parent_run_id'>): string {
  return run.parent_run_id ?? run.run_id;
}

/** The quota-paused runs a person resumes: top-level ones (a child mirrors its parent's wait). */
export function quotaPausedRuns(runs: readonly ActiveRunSummary[] | null | undefined): ActiveRunSummary[] {
  return (runs ?? []).filter(run => run.status === 'WaitingQuota' && !run.parent_run_id);
}
