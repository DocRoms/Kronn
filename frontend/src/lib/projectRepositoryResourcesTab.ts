export type ProjectRepositoryResourcesTab = 'skills' | 'automation' | 'artifacts';

const RESOURCES_TAB_STORAGE_KEY = 'kronn:projectRepositoryResourcesTab';
const RESOURCE_TABS: ProjectRepositoryResourcesTab[] = ['skills', 'automation', 'artifacts'];

export function readProjectRepositoryResourcesTab(): ProjectRepositoryResourcesTab {
  try {
    const saved = localStorage.getItem(RESOURCES_TAB_STORAGE_KEY);
    return RESOURCE_TABS.includes(saved as ProjectRepositoryResourcesTab)
      ? saved as ProjectRepositoryResourcesTab
      : 'skills';
  } catch {
    return 'skills';
  }
}

export function rememberProjectRepositoryResourcesTab(tab: ProjectRepositoryResourcesTab) {
  try {
    localStorage.setItem(RESOURCES_TAB_STORAGE_KEY, tab);
  } catch {
    // localStorage may be unavailable in private/restricted browser modes.
  }
}

const CATALOG_OPEN_STORAGE_PREFIX = 'kronn:projectRepositoryCatalogOpen:';

/** Whether the "available in Kronn, not in this project" catalog was left open
 *  for this project. Folded until someone opens it. */
export function readProjectRepositoryCatalogOpen(projectId: string): boolean {
  try {
    return localStorage.getItem(`${CATALOG_OPEN_STORAGE_PREFIX}${projectId}`) === 'open';
  } catch {
    return false;
  }
}

export function rememberProjectRepositoryCatalogOpen(projectId: string, open: boolean) {
  try {
    if (open) localStorage.setItem(`${CATALOG_OPEN_STORAGE_PREFIX}${projectId}`, 'open');
    else localStorage.removeItem(`${CATALOG_OPEN_STORAGE_PREFIX}${projectId}`);
  } catch {
    // localStorage may be unavailable in private/restricted browser modes.
  }
}
