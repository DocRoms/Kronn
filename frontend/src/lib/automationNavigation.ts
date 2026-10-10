import type { AutomationTab } from './routes';

/**
 * Where the previous visit left the Automation page: the tab and the resource
 * open in it. The page writes it on every change; the bare `/workflows`
 * address reopens there.
 */
export interface AutomationLastVisit {
  tab: AutomationTab;
  resourceId: string | null;
}

export const AUTOMATION_TABS: readonly AutomationTab[] = ['workflows', 'quickPrompts', 'quickApis', 'quickExecs', 'skills'];
const STORAGE_KEY = 'kronn:automationNavigation';
const FIRST_VISIT: AutomationLastVisit = { tab: 'workflows', resourceId: null };

export function isAutomationTab(value: unknown): value is AutomationTab {
  return typeof value === 'string' && AUTOMATION_TABS.includes(value as AutomationTab);
}

/** The last visit, or the workflows list when there is none this browser remembers. */
export function readAutomationLastVisit(): AutomationLastVisit {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return FIRST_VISIT;
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    return {
      tab: isAutomationTab(parsed.tab) ? parsed.tab : 'workflows',
      resourceId: typeof parsed.resourceId === 'string' && parsed.resourceId.trim() ? parsed.resourceId : null,
    };
  } catch {
    return FIRST_VISIT;
  }
}

export function writeAutomationLastVisit(visit: AutomationLastVisit): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(visit));
  } catch {
    // localStorage may be unavailable in private/restricted browser modes.
  }
}
