export type Page = 'projects' | 'mcps' | 'workflows' | 'discussions' | 'planning' | 'settings';

export const PAGE_PATHS: Record<Page, string> = {
  projects: '/projects',
  discussions: '/discussions',
  planning: '/planning',
  mcps: '/plugins',
  workflows: '/workflows',
  settings: '/config',
};

const PATH_TO_PAGE: Record<string, Page> = Object.fromEntries(
  Object.entries(PAGE_PATHS).map(([page, path]) => [path, page as Page]),
) as Record<string, Page>;

export function pathToPage(pathname: string): Page | null {
  for (const [path, page] of Object.entries(PATH_TO_PAGE)) {
    if (pathname === path || pathname.startsWith(path + '/')) return page;
  }
  if (pathname === '/') return 'projects';
  return null;
}
