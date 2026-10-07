/**
 * Canonical addresses of the dashboard pages.
 *
 * Single source of truth for the route table, the navigation hook and the nav
 * bar: a path is never spelled out anywhere else. A page key is the stable
 * internal identifier (tour steps, `data-tour-id`); its path is what the user
 * sees, so the two are allowed to differ (`mcps` lives at `/plugins`).
 */

export type DashboardPage =
  | 'projects'
  | 'discussions'
  | 'planning'
  | 'workflows'
  | 'pages'
  | 'mcps'
  | 'settings';

export const DEFAULT_PAGE: DashboardPage = 'projects';

export const PAGE_PATHS: Record<DashboardPage, string> = {
  projects: '/projects',
  discussions: '/discussions',
  planning: '/planning',
  workflows: '/workflows',
  pages: '/pages',
  mcps: '/plugins',
  settings: '/config',
};

const PAGE_BY_PATH = Object.entries(PAGE_PATHS) as [DashboardPage, string][];

/** The address of one project, open in its master/detail workspace. */
export function projectPath(projectId: string): string {
  return `${PAGE_PATHS.projects}/${encodeURIComponent(projectId)}`;
}

/** The address of one planning task, open in its detail pane. */
export function planningTaskPath(taskId: string): string {
  return `${PAGE_PATHS.planning}/${encodeURIComponent(taskId)}`;
}

/** The address of one Page (Artifact) open on the Artifacts page. */
export function livePagePath(pageId: string): string {
  return `${PAGE_PATHS.pages}/${encodeURIComponent(pageId)}`;
}

/** The address of one plugin config, open in its detail panel. */
export function pluginPath(configId: string): string {
  return `${PAGE_PATHS.mcps}/${encodeURIComponent(configId)}`;
}

/** The Configuration section of the sites Live Pages may embed content from. */
export const EMBED_SETTINGS_SEGMENT = '/artifacts';
export const EMBED_SETTINGS_PATH = `${PAGE_PATHS.settings}${EMBED_SETTINGS_SEGMENT}`;

/** That section, optionally with a site typed in: only typed, never added, the user confirms. */
export function embedSettingsPath(origin?: string | null): string {
  return origin ? `${EMBED_SETTINGS_PATH}?origin=${encodeURIComponent(origin)}` : EMBED_SETTINGS_PATH;
}

/** The tabs of the Automation page. */
export type AutomationTab = 'workflows' | 'quickPrompts' | 'quickApis' | 'quickExecs' | 'skills';

/** What the Automation page shows: a tab, the resource open in it, and for a workflow the run to reveal. */
export interface AutomationSelection {
  tab: AutomationTab;
  resourceId: string | null;
  runId?: string | null;
}

/** What a navigation to Automation asks the page to do once, on arrival. */
export interface AutomationIntent {
  /** Open the creation wizard with this preset, bound to this project. */
  preset?: { presetId: string; projectId: string };
}

// The path segment of each tab but the first, which is the page itself.
const AUTOMATION_SEGMENTS: Record<Exclude<AutomationTab, 'workflows'>, string> = {
  quickPrompts: 'qp',
  quickApis: 'qa',
  quickExecs: 'qe',
  skills: 'skills',
};

/** The address of an Automation selection. */
export function automationPath({ tab, resourceId, runId }: AutomationSelection): string {
  if (tab === 'workflows') {
    if (!resourceId) return PAGE_PATHS.workflows;
    const path = `${PAGE_PATHS.workflows}/${encodeURIComponent(resourceId)}`;
    return runId ? `${path}/runs/${encodeURIComponent(runId)}` : path;
  }
  const path = `${PAGE_PATHS.workflows}/${AUTOMATION_SEGMENTS[tab]}`;
  return resourceId ? `${path}/${encodeURIComponent(resourceId)}` : path;
}

/** The address of one workflow, optionally with one of its runs revealed. */
export function workflowPath(workflowId: string, runId?: string | null): string {
  return automationPath({ tab: 'workflows', resourceId: workflowId, runId });
}

/** The Automation tab a pathname under `/workflows` names. */
export function automationTabFromPath(pathname: string): AutomationTab {
  const segment = pathname.slice(PAGE_PATHS.workflows.length + 1).split('/')[0];
  const tab = (Object.keys(AUTOMATION_SEGMENTS) as Exclude<AutomationTab, 'workflows'>[])
    .find(candidate => AUTOMATION_SEGMENTS[candidate] === segment);
  return tab ?? 'workflows';
}

export function sameAutomationSelection(a: AutomationSelection, b: AutomationSelection): boolean {
  return a.tab === b.tab && a.resourceId === b.resourceId && (a.runId ?? null) === (b.runId ?? null);
}

/** The query parameter naming the message to reveal inside a discussion. */
export const DISCUSSION_MESSAGE_PARAM = 'message';

/** The address of one discussion, optionally scrolled to one of its messages. */
export function discussionPath(discussionId: string, messageId?: string | null): string {
  const path = `${PAGE_PATHS.discussions}/${encodeURIComponent(discussionId)}`;
  return messageId ? `${path}?${DISCUSSION_MESSAGE_PARAM}=${encodeURIComponent(messageId)}` : path;
}

/**
 * What a navigation to Discussions asks the page to do once, on arrival.
 * It travels as history state, so a reload or a Back never replays it.
 */
export interface DiscussionsIntent {
  /** Run the agent on the discussion the address names. */
  autoRun?: boolean;
  /** Expand this batch group in the sidebar, as a batch or a comparison. */
  focusBatch?: { id: string; mode: 'batch' | 'compare' };
}

/**
 * The page a pathname belongs to, or `null` when it is not a dashboard
 * address. A page owns its whole subtree (`/projects/abc` is `projects`), but
 * not its look-alikes (`/projectsfoo` is nothing).
 */
export function pathToPage(pathname: string): DashboardPage | null {
  const normalized = pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname;
  for (const [page, path] of PAGE_BY_PATH) {
    if (normalized === path || normalized.startsWith(`${path}/`)) return page;
  }
  return null;
}

/**
 * Views that stand alone, outside the dashboard shell: a Live Page in its own
 * tab, and the mosaics. `page` takes a `/:pageId` segment; the mosaics carry
 * their members and layout in the query string.
 */
const STANDALONE_ROOT = '/standalone';

export const STANDALONE_PATHS = {
  page: `${STANDALONE_ROOT}/pages`,
  pagesMosaic: `${STANDALONE_ROOT}/pages/mosaic`,
  discussionsMosaic: `${STANDALONE_ROOT}/discussions/mosaic`,
} as const;

export function standalonePagePath(pageId: string): string {
  return `${STANDALONE_PATHS.page}/${encodeURIComponent(pageId)}`;
}

/** Whether a pathname is a whole-window view, outside the dashboard shell. */
export function isStandalonePath(pathname: string): boolean {
  return pathname === STANDALONE_ROOT || pathname.startsWith(`${STANDALONE_ROOT}/`);
}

/** Whether a pathname is an address of this app, dashboard or standalone. */
export function isAppPath(pathname: string): boolean {
  return pathname === '/' || pathToPage(pathname) !== null || isStandalonePath(pathname);
}
