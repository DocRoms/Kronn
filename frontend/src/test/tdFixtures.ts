import type { TdReviewItem, TdReviewList, TdReviewState } from '../types/generated';
import { groupTds, isTdSettled } from '../lib/techDebt';

export function tdItem(id: string, severity: string, state: TdReviewState = 'pending', extra: Partial<TdReviewItem> = {}): TdReviewItem {
  return {
    id, title: `Titre ${id}`, area: 'Backend', severity, status: 'Verified in source',
    effort: 'M', blast_radius: 'Module', state, problem: 'It breaks.', impact: null,
    verification: 'It no longer breaks.', pointers: [{ path: 'backend/src/lib.rs', line: 10 }],
    source_path: `docs/tech-debt/${id}.md`, task: null, talk_count: 0, talk_discussion_id: null,
    ...extra,
  };
}

/** A review payload whose counts and gate follow its items. */
export function tdList(items: TdReviewItem[], extra: Partial<TdReviewList> = {}): TdReviewList {
  const count = (state: TdReviewState) => items.filter(i => i.state === state).length;
  const blocking = items.filter(i => i.severity === 'Critical' || i.severity === 'High');
  const settled = blocking.filter(i => isTdSettled(i.state)).length;
  return {
    items,
    counts: {
      pending: count('pending'), confirmed: count('confirmed'), accepted_decision: count('accepted_decision'),
      deferred: count('deferred'), rejected: count('rejected'),
    },
    by_severity: groupTds(items).map(g => ({
      severity: g.severity, total: g.items.length, remaining: g.items.filter(i => !isTdSettled(i.state)).length,
    })),
    gate: { blocking_total: blocking.length, blocking_settled: settled, met: items.length > 0 && settled === blocking.length },
    validated: false,
    audit_date: '2026-10-01T10:00:00Z',
    validation_discussion_id: 'disc-validation',
    ...extra,
  };
}
