import type { AiAuditStatus, MappingAction, TemplateMergeView } from '../types/generated';

/** Statuses a full audit still treats as "no Kronn audit yet": only these get the merge offer. */
export const MERGE_CANDIDATE_STATUSES: readonly AiAuditStatus[] = ['NoTemplate', 'TemplateInstalled'];

export const MAPPING_ACTIONS: readonly MappingAction[] = [
  'extracted', 'target_exists', 'unmapped', 'not_prose', 'kronn_managed',
];

/**
 * A repo is "documented and not merged yet" when the merge would copy real
 * content into new v2 files. A lone root README does not count: a full audit
 * already reads it. Docs only present untracked also count, since the user
 * must commit them before anything can be merged.
 */
export function offersTemplateMerge(plan: TemplateMergeView | null | undefined): boolean {
  if (!plan) return false;
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
