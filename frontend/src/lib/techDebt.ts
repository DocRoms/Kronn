import type { TdReviewDecision, TdReviewItem, TdReviewState } from '../types/generated';

export const TD_SEVERITIES = ['Critical', 'High', 'Medium', 'Low', 'Unknown'] as const;
export type TdFilter = 'todo' | 'done' | 'all';

/** Pending and deferred sheets still wait for a decision. */
export function isTdSettled(state: TdReviewState): boolean {
  return state !== 'pending' && state !== 'deferred';
}

export function matchesTdFilter(item: TdReviewItem, filter: TdFilter): boolean {
  if (filter === 'all') return true;
  return filter === 'done' ? isTdSettled(item.state) : !isTdSettled(item.state);
}

/** Search over id, title, area and cited files, case-insensitive. */
export function matchesTdQuery(item: TdReviewItem, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return item.id.toLowerCase().includes(q)
    || item.title.toLowerCase().includes(q)
    || item.area.toLowerCase().includes(q)
    || item.pointers.some(p => p.path.toLowerCase().includes(q));
}

export function visibleTds(
  items: TdReviewItem[],
  filter: TdFilter,
  severities: ReadonlySet<string>,
  query: string,
): TdReviewItem[] {
  return items.filter(item =>
    matchesTdFilter(item, filter)
    && (severities.size === 0 || severities.has(item.severity))
    && matchesTdQuery(item, query));
}

/** Visible sheets grouped by severity, in severity order, empty groups left out. */
export function groupTds(items: TdReviewItem[]): Array<{ severity: string; items: TdReviewItem[] }> {
  return TD_SEVERITIES
    .map(severity => ({ severity, items: items.filter(item => item.severity === severity) }))
    .filter(group => group.items.length > 0);
}

/** Next unsettled sheet after `currentId` in display order, wrapping around. */
export function nextUnsettled(ordered: TdReviewItem[], currentId: string | null): TdReviewItem | null {
  const start = currentId ? ordered.findIndex(item => item.id === currentId) : -1;
  for (let step = 1; step <= ordered.length; step += 1) {
    const candidate = ordered[(start + step + ordered.length) % ordered.length];
    if (candidate && candidate.id !== currentId && !isTdSettled(candidate.state)) return candidate;
  }
  return null;
}

/** Short display id: `TD-20261001-cache-leak` → `cache-leak`. */
export function shortTdId(id: string): string {
  return id.replace(/^TD-\d{8}-/, '') || id;
}

export const TD_DECISIONS: TdReviewDecision[] = ['confirm', 'accept_decision', 'defer', 'reject'];

export const TD_STATE_OF_DECISION: Record<TdReviewDecision, TdReviewState> = {
  confirm: 'confirmed',
  accept_decision: 'accepted_decision',
  defer: 'deferred',
  reject: 'rejected',
};
