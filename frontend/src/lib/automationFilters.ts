// Filters and grouping of the global Automation page: type, pinned, active,
// project, recent and free-text search. Pure and in-memory, so the sidebar
// list, the counters of the chips and the tests all read one definition of
// "matches".

export type AutomationKind = 'workflows' | 'quickPrompts' | 'quickApis' | 'quickExecs' | 'skills';
export type AutomationKindFilter = AutomationKind | 'all';
/** How the sidebar list is split into groups: one per type, one per project,
 *  or one flat list. */
export type AutomationGroupBy = 'kind' | 'project' | 'none';

/** Order of the type chips and of the type groups. */
export const AUTOMATION_KIND_FILTERS: AutomationKind[] = ['workflows', 'quickPrompts', 'quickApis', 'quickExecs', 'skills'];
/** i18n key of each type's name: on the type chip, in its list and on the
 *  group headers. */
export const AUTOMATION_KIND_LABEL_KEYS = {
  workflows: 'wf.tabWorkflows',
  quickPrompts: 'wf.tabQuickPrompts',
  quickApis: 'wf.tabQuickApis',
  quickExecs: 'wf.tabQuickExecs',
  skills: 'wf.tabSkills',
} as const satisfies Record<AutomationKind, string>;
export const AUTOMATION_GROUP_BYS: AutomationGroupBy[] = ['kind', 'project', 'none'];
export const DEFAULT_AUTOMATION_GROUP_BY: AutomationGroupBy = 'kind';
/** Value of the project filter that keeps the automations without a project. */
export const AUTOMATION_NO_PROJECT = '__global__';

export interface FilterableAutomation {
  kind: AutomationKind;
  projectId: string | null;
  /** A skill can be attached to several projects at once; every other kind
   *  belongs to one (`projectId`). Both are read by the project filter. */
  projectIds?: readonly string[];
  pinned: boolean;
  searchText: string;
  workflow?: { enabled: boolean };
  /** When the user last opened it from the sidebar (epoch ms); absent or null
   *  when it has never been, or fell out of the recent history. */
  lastOpenedAt?: number | null;
}

export interface AutomationFilters {
  kind: AutomationKindFilter;
  /** "★ Épinglés": keeps the pinned automations (a skill is pinned when starred). */
  pinned: boolean;
  /** "Actifs": leaves out a disabled workflow. A Quick Prompt, API, Exec or
   *  Skill has no off switch, so it is always active. */
  active: boolean;
  /** `all`, `AUTOMATION_NO_PROJECT`, or a project id. */
  projectId: string;
  /** "Récents": keeps what the user opened lately (the list is then ordered
   *  by last opening, see `sortAutomationResources`). */
  recent: boolean;
  query: string;
}

export const NO_AUTOMATION_FILTERS: AutomationFilters = {
  kind: 'all',
  pinned: false,
  active: false,
  projectId: 'all',
  recent: false,
  query: '',
};

/** The part of a filter set a counter leaves out, so that a chip counts what
 *  choosing it would show given every other filter. */
export type AutomationFilterAxis = keyof AutomationFilters;

export function isAutomationGroupBy(value: unknown): value is AutomationGroupBy {
  return typeof value === 'string' && AUTOMATION_GROUP_BYS.includes(value as AutomationGroupBy);
}

export function matchesAutomationActive(resource: FilterableAutomation): boolean {
  return resource.workflow?.enabled !== false;
}

/** Every project an automation is listed under; empty means "no project". */
export function automationProjectIds(resource: Pick<FilterableAutomation, 'projectId' | 'projectIds'>): string[] {
  const ids = new Set(resource.projectIds ?? []);
  if (resource.projectId) ids.add(resource.projectId);
  return [...ids];
}

export function matchesAutomationProject(resource: FilterableAutomation, projectId: string): boolean {
  if (projectId === 'all') return true;
  const ids = automationProjectIds(resource);
  return projectId === AUTOMATION_NO_PROJECT ? ids.length === 0 : ids.includes(projectId);
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
    && (ignore.includes('pinned') || !filters.pinned || resource.pinned)
    && (ignore.includes('active') || !filters.active || matchesAutomationActive(resource))
    && (ignore.includes('projectId') || matchesAutomationProject(resource, filters.projectId))
    && (ignore.includes('recent') || !filters.recent || resource.lastOpenedAt != null)
    && (ignore.includes('query') || matchesAutomationQuery(resource, filters.query));
}

/** How many of the chip filters are set (the search is not one: it stays
 *  visible on its own, with its own clear button). The type chip counts only
 *  once it has left "Tout". */
export function activeAutomationFilterCount(filters: AutomationFilters): number {
  return [
    filters.kind !== 'all',
    filters.pinned,
    filters.active,
    filters.projectId !== 'all',
    filters.recent,
  ].filter(Boolean).length;
}

export function countAutomationKinds(
  resources: readonly FilterableAutomation[],
  filters: AutomationFilters,
): Record<AutomationKindFilter, number> {
  const counts: Record<AutomationKindFilter, number> = {
    all: 0, workflows: 0, quickPrompts: 0, quickApis: 0, quickExecs: 0, skills: 0,
  };
  for (const resource of resources) {
    if (!matchesAutomationFilters(resource, filters, ['kind'])) continue;
    counts.all += 1;
    counts[resource.kind] += 1;
  }
  return counts;
}

/** How many automations each project choice would show, given the other
 *  filters: keyed by project id and `AUTOMATION_NO_PROJECT`, plus `all`. A
 *  skill listed by several projects counts once under each of them, and once
 *  in `all`. */
export function countAutomationProjects(
  resources: readonly FilterableAutomation[],
  filters: AutomationFilters,
): Record<string, number> {
  const counts: Record<string, number> = { all: 0 };
  for (const resource of resources) {
    if (!matchesAutomationFilters(resource, filters, ['projectId'])) continue;
    counts.all += 1;
    const ids = automationProjectIds(resource);
    for (const id of ids.length === 0 ? [AUTOMATION_NO_PROJECT] : ids) {
      counts[id] = (counts[id] ?? 0) + 1;
    }
  }
  return counts;
}

export interface AutomationGroup<T> {
  /** Stable, namespaced by the grouping: safe to persist as a collapse key. */
  key: string;
  by: AutomationGroupBy;
  /** The kind (`by: 'kind'`), the project id (`by: 'project'`, `null` for
   *  "no project"), or `null` for the flat list. */
  id: string | null;
  items: T[];
}

export const AUTOMATION_FLAT_GROUP_KEY = 'none:all';

/**
 * Splits the already filtered and sorted list into the groups the sidebar
 * shows, each keeping the order of `resources`. An empty group is left out.
 *
 *  - `kind`: one group per type, in `AUTOMATION_KIND_FILTERS` order.
 *  - `project`: one group per project of `projectOrder`, then "no project".
 *    A skill listed by several projects appears under each of them (KT-914);
 *    an automation whose only projects are not in `projectOrder` (a project
 *    hidden from this page) is left out, as the project tree always did.
 *  - `none`: a single flat group.
 */
export function groupAutomations<T extends Pick<FilterableAutomation, 'kind' | 'projectId' | 'projectIds'>>(
  resources: readonly T[],
  groupBy: AutomationGroupBy,
  projectOrder: readonly string[] = [],
): AutomationGroup<T>[] {
  if (groupBy === 'none') {
    return resources.length === 0
      ? []
      : [{ key: AUTOMATION_FLAT_GROUP_KEY, by: 'none', id: null, items: [...resources] }];
  }
  if (groupBy === 'kind') {
    return AUTOMATION_KIND_FILTERS
      .map(kind => ({
        key: `kind:${kind}`, by: 'kind' as const, id: kind as string | null,
        items: resources.filter(resource => resource.kind === kind),
      }))
      .filter(group => group.items.length > 0);
  }
  const groups: AutomationGroup<T>[] = [];
  for (const projectId of [...projectOrder, null]) {
    const items = resources.filter(resource => {
      const ids = automationProjectIds(resource);
      return projectId === null ? ids.length === 0 : ids.includes(projectId);
    });
    if (items.length > 0) {
      groups.push({ key: `project:${projectId ?? AUTOMATION_NO_PROJECT}`, by: 'project', id: projectId, items });
    }
  }
  return groups;
}
