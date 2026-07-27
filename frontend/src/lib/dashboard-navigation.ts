/**
 * Reload/HMR navigation checkpoint.
 *
 * `sessionStorage` is intentional: a full Vite reload in the current tab keeps
 * the user's place, while a genuinely new app/browser session still starts on
 * Projects. This is not a router or a shareable deep-link contract.
 */

export type DashboardPage =
  | 'projects'
  | 'mcps'
  | 'workflows'
  | 'discussions'
  | 'planning'
  | 'settings';

const PAGE_KEY = 'kronn:navigation:page';
const DISCUSSION_KEY = 'kronn:navigation:discussion';
const DEFAULT_PAGE: DashboardPage = 'projects';

const PAGES = new Set<DashboardPage>([
  'projects',
  'mcps',
  'workflows',
  'discussions',
  'planning',
  'settings',
]);

function storage(): Storage | null {
  try {
    return typeof sessionStorage === 'undefined' ? null : sessionStorage;
  } catch {
    return null;
  }
}

export function readDashboardPage(): DashboardPage {
  try {
    const value = storage()?.getItem(PAGE_KEY);
    return value && PAGES.has(value as DashboardPage)
      ? value as DashboardPage
      : DEFAULT_PAGE;
  } catch {
    return DEFAULT_PAGE;
  }
}

export function writeDashboardPage(page: DashboardPage): void {
  try {
    storage()?.setItem(PAGE_KEY, page);
  } catch {
    // A storage failure must never prevent navigation.
  }
}

export function readActiveDiscussionId(): string | null {
  try {
    const value = storage()?.getItem(DISCUSSION_KEY)?.trim();
    return value || null;
  } catch {
    return null;
  }
}

export function writeActiveDiscussionId(discussionId: string | null): void {
  try {
    const target = storage();
    if (!target) return;
    if (discussionId) {
      target.setItem(DISCUSSION_KEY, discussionId);
    } else {
      target.removeItem(DISCUSSION_KEY);
    }
  } catch {
    // The in-memory selection remains usable when storage is unavailable.
  }
}
