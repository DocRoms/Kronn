// What the Automation sidebar remembers for the user, in this browser: the
// "Group by" choice and the history of what was opened. Every access is
// guarded — localStorage may be unavailable (private mode) or full, and the
// sidebar stays usable from memory when it is.

import {
  DEFAULT_AUTOMATION_GROUP_BY,
  isAutomationGroupBy,
  type AutomationGroupBy,
} from './automationFilters';

export const AUTOMATION_GROUP_BY_STORAGE_KEY = 'kronn:automationGroupBy';
export const AUTOMATION_OPENED_STORAGE_KEY = 'kronn:automationLastOpened';
/** How many openings the "Recent" history keeps: the oldest one drops out. */
export const AUTOMATION_OPENED_LIMIT = 20;

export function readAutomationGroupBy(): AutomationGroupBy {
  try {
    const stored = localStorage.getItem(AUTOMATION_GROUP_BY_STORAGE_KEY);
    return isAutomationGroupBy(stored) ? stored : DEFAULT_AUTOMATION_GROUP_BY;
  } catch {
    return DEFAULT_AUTOMATION_GROUP_BY;
  }
}

export function writeAutomationGroupBy(groupBy: AutomationGroupBy): void {
  try {
    localStorage.setItem(AUTOMATION_GROUP_BY_STORAGE_KEY, groupBy);
  } catch {
    // The choice then only lasts for this session.
  }
}

/** Automation id (`kind:resourceId`) → epoch ms of its last opening. */
export type AutomationLastOpened = Readonly<Record<string, number>>;

export function readAutomationLastOpened(): AutomationLastOpened {
  try {
    const parsed = JSON.parse(localStorage.getItem(AUTOMATION_OPENED_STORAGE_KEY) ?? '{}') as unknown;
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed).filter((entry): entry is [string, number] => (
        typeof entry[1] === 'number' && Number.isFinite(entry[1]) && entry[1] > 0
      )),
    );
  } catch {
    return {};
  }
}

export function writeAutomationLastOpened(history: AutomationLastOpened): void {
  try {
    localStorage.setItem(AUTOMATION_OPENED_STORAGE_KEY, JSON.stringify(history));
  } catch {
    // The history then only lasts for this session.
  }
}

/** The history once `id` has just been opened: it is the most recent, and
 *  only the `AUTOMATION_OPENED_LIMIT` latest openings are kept. */
export function withAutomationOpened(
  history: AutomationLastOpened,
  id: string,
  now: number = Date.now(),
): AutomationLastOpened {
  // Two openings in the same millisecond must still have an order.
  const latest = Math.max(0, ...Object.values(history));
  const openedAt = Math.max(now, latest + 1);
  const entries = Object.entries({ ...history, [id]: openedAt })
    .sort(([, left], [, right]) => right - left)
    .slice(0, AUTOMATION_OPENED_LIMIT);
  return Object.fromEntries(entries);
}
