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
