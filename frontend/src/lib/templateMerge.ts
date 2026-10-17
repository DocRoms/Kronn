import type { AiAuditStatus, MappingAction, TemplateMergeState, TemplateMergeView } from '../types/generated';

/** Statuses a full audit still treats as "no Kronn audit yet": only these get the merge offer. */
export const MERGE_CANDIDATE_STATUSES: readonly AiAuditStatus[] = ['NoTemplate', 'TemplateInstalled'];

export const MAPPING_ACTIONS: readonly MappingAction[] = [
  'extracted', 'target_exists', 'unmapped', 'not_prose', 'kronn_managed',
];

/**
 * `in_progress`: a merge worktree still holds files to review (or nothing was
 * accepted yet). `to_integrate`: accepted commits wait on the merge branch for
 * the checkout's branch. `none`: no merge, or the branch is already integrated.
 */
export type TemplateMergeStatus = 'none' | 'in_progress' | 'to_integrate';

export function templateMergeStatus(state: TemplateMergeState | null | undefined): TemplateMergeStatus {
  if (!state) return 'none';
  // A worktree with nothing proposed and nothing ahead is what an integrated
  // merge leaves behind: the merge is done, not in progress.
  if (state.worktree_exists && state.pending_files > 0) return 'in_progress';
  if (state.branch_ahead_commits > 0) return 'to_integrate';
  return 'none';
}

/**
 * An unfinished merge is offered again; an accepted one waiting to be
 * integrated never is. Otherwise a repo is "documented and not merged yet"
 * when the merge would copy real content into new v2 files. A lone root README does not count: a full audit
 * already reads it. Docs only present untracked also count, since the user
 * must commit them before anything can be merged.
 */
export function offersTemplateMerge(plan: TemplateMergeView | null | undefined): boolean {
  if (!plan) return false;
  const status = templateMergeStatus(plan.merge);
  if (status === 'to_integrate') return false;
  if (status === 'in_progress') return true;
  if (plan.untracked_in_checkout.length > 0) return true;
  if (plan.files.length === 0) return false;
  return plan.correspondence.some(row =>
    row.action === 'extracted' && row.source.toLowerCase() !== 'readme.md');
}

export function countByAction(plan: TemplateMergeView): Record<MappingAction, number> {
  const counts = Object.fromEntries(MAPPING_ACTIONS.map(a => [a, 0])) as Record<MappingAction, number>;
  for (const row of plan.correspondence) counts[row.action] += 1;
  return counts;
}

export function errorText(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  return typeof error === 'string' && error ? error : String(error);
}
