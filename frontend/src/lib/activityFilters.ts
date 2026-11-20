// The panel's category filters (KT-1684): All / Discussions / Automations /
// Kronn under each tab, the last choice remembered per tab.

import type { ActivityRow } from '../types/generated';
import type { NotificationCategory } from './notifications';

export type ActivityFilter = 'all' | NotificationCategory;
export type ActivityTab = 'progress' | 'notifications';

/** Chips in reading order; the sections of "All" follow the same order. */
export const ACTIVITY_FILTERS: readonly ActivityFilter[] = ['all', 'discussions', 'automations', 'kronn'];
export const ACTIVITY_CATEGORIES: readonly NotificationCategory[] = ['discussions', 'automations', 'kronn'];

/** Which section a row sits in: a conversation, an automation, or Kronn's own work
 *  (audits, task workers, and any type this build does not know). */
export function categoryOfRow(row: ActivityRow): NotificationCategory {
  switch (row.type) {
    case 'agent_turn': return 'discussions';
    case 'workflow_run': return 'automations';
    case 'audit':
    case 'task_worker': return 'kronn';
  }
  switch (row.link?.target) {
    case 'discussion': return 'discussions';
    case 'workflow': return 'automations';
    default: return 'kronn';
  }
}

export function filterStorageKey(tab: ActivityTab): string {
  return `kronn:activity-panel:filter:${tab}`;
}

export function readFilter(tab: ActivityTab): ActivityFilter {
  try {
    const stored = localStorage.getItem(filterStorageKey(tab));
    return ACTIVITY_FILTERS.includes(stored as ActivityFilter) ? (stored as ActivityFilter) : 'all';
  } catch {
    return 'all';
  }
}

export function writeFilter(tab: ActivityTab, filter: ActivityFilter): void {
  try {
    localStorage.setItem(filterStorageKey(tab), filter);
  } catch {
    // Storage may be blocked: the filter still works for this visit.
  }
}

/** How many items each chip shows; `all` is the total. */
export function countByCategory<T>(
  items: readonly T[],
  categoryOf: (item: T) => NotificationCategory,
): Record<ActivityFilter, number> {
  const counts: Record<ActivityFilter, number> = { all: items.length, discussions: 0, automations: 0, kronn: 0 };
  for (const item of items) counts[categoryOf(item)] += 1;
  return counts;
}
