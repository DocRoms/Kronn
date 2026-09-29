// Filters of the global Automation page: type, state, project and free-text
// search. Pure and in-memory, so the sidebar tree, the counters of the filter
// bar and the tests all read one definition of "matches".

export type AutomationKind = 'workflows' | 'quickPrompts' | 'quickApis' | 'quickExecs';
export type AutomationKindFilter = AutomationKind | 'all';
/** `active` / `inactive` only ever exclude a disabled workflow: a Quick
 *  Prompt, API or Exec has no off switch, so it is always usable. */
export type AutomationStateFilter = 'all' | 'favorites' | 'active' | 'inactive';

/** Order of the type chips: same as the tour and the creation dialog. */
export const AUTOMATION_KIND_FILTERS: AutomationKind[] = ['workflows', 'quickApis', 'quickPrompts', 'quickExecs'];
export const AUTOMATION_STATE_FILTERS: AutomationStateFilter[] = ['all', 'favorites', 'active', 'inactive'];
/** Value of the project filter that keeps the automations without a project. */
export const AUTOMATION_NO_PROJECT = '__global__';

export interface FilterableAutomation {
  kind: AutomationKind;
  projectId: string | null;
  pinned: boolean;
  searchText: string;
  workflow?: { enabled: boolean };
}

export interface AutomationFilters {
  kind: AutomationKindFilter;
  state: AutomationStateFilter;
  /** `all`, `AUTOMATION_NO_PROJECT`, or a project id. */
  projectId: string;
  query: string;
}

export const NO_AUTOMATION_FILTERS: AutomationFilters = {
  kind: 'all',
  state: 'all',
  projectId: 'all',
  query: '',
};

/** The part of a filter set a counter leaves out, so that a chip counts what
 *  choosing it would show given every other filter. */
export type AutomationFilterAxis = keyof AutomationFilters;

export function matchesAutomationState(resource: FilterableAutomation, state: AutomationStateFilter): boolean {
  switch (state) {
    case 'favorites': return resource.pinned;
    case 'active': return resource.workflow?.enabled !== false;
    case 'inactive': return resource.workflow?.enabled === false;
    default: return true;
  }
}

export function matchesAutomationProject(resource: FilterableAutomation, projectId: string): boolean {
  if (projectId === 'all') return true;
  return projectId === AUTOMATION_NO_PROJECT ? !resource.projectId : resource.projectId === projectId;
}

/** The shell's own search rule, kept here to count what a search leaves. */
export function matchesAutomationQuery(resource: FilterableAutomation, query: string): boolean {
  const needle = query.trim().toLocaleLowerCase();
  return !needle || resource.searchText.toLocaleLowerCase().includes(needle);
}

export function matchesAutomationFilters(
  resource: FilterableAutomation,
  filters: AutomationFilters,
  ignore: readonly AutomationFilterAxis[] = [],
): boolean {
  return (ignore.includes('kind') || filters.kind === 'all' || resource.kind === filters.kind)
    && (ignore.includes('state') || matchesAutomationState(resource, filters.state))
    && (ignore.includes('projectId') || matchesAutomationProject(resource, filters.projectId))
    && (ignore.includes('query') || matchesAutomationQuery(resource, filters.query));
}

/** How many of the chips-and-select filters are set (the search is not one:
 *  it stays visible on its own). */
export function activeAutomationFilterCount(filters: AutomationFilters): number {
  return [filters.kind !== 'all', filters.state !== 'all', filters.projectId !== 'all']
    .filter(Boolean).length;
}

export function countAutomationKinds(
  resources: readonly FilterableAutomation[],
  filters: AutomationFilters,
): Record<AutomationKindFilter, number> {
  const counts: Record<AutomationKindFilter, number> = {
    all: 0, workflows: 0, quickPrompts: 0, quickApis: 0, quickExecs: 0,
  };
  for (const resource of resources) {
    if (!matchesAutomationFilters(resource, filters, ['kind'])) continue;
    counts.all += 1;
    counts[resource.kind] += 1;
  }
  return counts;
}

export function countAutomationStates(
  resources: readonly FilterableAutomation[],
  filters: AutomationFilters,
): Record<AutomationStateFilter, number> {
  const counts: Record<AutomationStateFilter, number> = { all: 0, favorites: 0, active: 0, inactive: 0 };
  for (const resource of resources) {
    if (!matchesAutomationFilters(resource, filters, ['state'])) continue;
    for (const state of AUTOMATION_STATE_FILTERS) {
      if (matchesAutomationState(resource, state)) counts[state] += 1;
    }
  }
  return counts;
}
