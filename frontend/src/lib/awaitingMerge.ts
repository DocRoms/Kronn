// KT-1519 — a task integrated into a campaign or feature branch awaits that
// branch's merge. The branch name shows in a narrow status pill: long names are
// cut from the front (the end — `kt-1495` — is what tells branches apart) and the
// full name stays on the pill's title.

import type { PlanningAwaitingMerge } from '../types/generated';

const MAX_BRANCH_CHARS = 24;

export function shortBranch(branch: string): string {
  return branch.length <= MAX_BRANCH_CHARS
    ? branch
    : `…${branch.slice(-(MAX_BRANCH_CHARS - 1))}`;
}

export function awaitingMergeTitle(awaiting: PlanningAwaitingMerge): string {
  return `${awaiting.branch} @ ${awaiting.sha.slice(0, 8)}`;
}
