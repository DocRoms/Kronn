import { describe, expect, it } from 'vitest';
import {
  activeAutomationFilterCount,
  AUTOMATION_FLAT_GROUP_KEY,
  AUTOMATION_GROUP_BYS,
  AUTOMATION_KIND_FILTERS,
  AUTOMATION_NO_PROJECT,
  automationProjectIds,
  countAutomationKinds,
  countAutomationProjects,
  DEFAULT_AUTOMATION_GROUP_BY,
  groupAutomations,
  isAutomationGroupBy,
  matchesAutomationFilters,
  NO_AUTOMATION_FILTERS,
  type AutomationFilterAxis,
  type AutomationFilters,
  type FilterableAutomation,
} from '../automationFilters';

const item = (over: Partial<FilterableAutomation> & Pick<FilterableAutomation, 'kind' | 'searchText'>): FilterableAutomation => ({
  projectId: null,
  pinned: false,
  ...over,
});

const library: FilterableAutomation[] = [
  item({ kind: 'workflows', searchText: 'Nightly Alpha cron', projectId: 'alpha', pinned: true, workflow: { enabled: true }, lastOpenedAt: 300 }),
  item({ kind: 'workflows', searchText: 'Weekly report manual', workflow: { enabled: false }, lastOpenedAt: 100 }),
  item({ kind: 'quickPrompts', searchText: 'Review prompt Claude', projectId: 'alpha' }),
  item({ kind: 'quickPrompts', searchText: 'Summarise prompt Codex', pinned: true, lastOpenedAt: 200 }),
  item({ kind: 'quickApis', searchText: 'Create ticket jira' }),
  item({ kind: 'quickExecs', searchText: 'CloudWatch aws json', projectId: 'beta' }),
];
const filters = (over: Partial<AutomationFilters> = {}): AutomationFilters => ({ ...NO_AUTOMATION_FILTERS, ...over });
const names = (set: AutomationFilters, ignore: readonly AutomationFilterAxis[] = []) => (
  library.filter(entry => matchesAutomationFilters(entry, set, ignore)).map(entry => entry.searchText.split(' ')[0])
);

describe('automation filters', () => {
  it('keeps everything when no filter is set, "All" types being the default', () => {
    expect(NO_AUTOMATION_FILTERS.kind).toBe('all');
    expect(names(filters())).toHaveLength(6);
    expect(activeAutomationFilterCount(filters())).toBe(0);
  });

  it('filters by type, by pinned, by project and by recent', () => {
    expect(names(filters({ kind: 'quickPrompts' }))).toEqual(['Review', 'Summarise']);
    expect(names(filters({ pinned: true }))).toEqual(['Nightly', 'Summarise']);
    expect(names(filters({ projectId: 'alpha' }))).toEqual(['Nightly', 'Review']);
    expect(names(filters({ projectId: AUTOMATION_NO_PROJECT }))).toEqual(['Weekly', 'Summarise', 'Create']);
    expect(names(filters({ recent: true }))).toEqual(['Nightly', 'Weekly', 'Summarise']);
  });

  it('treats only a disabled workflow as not active: a Quick item has no off switch', () => {
    expect(names(filters({ active: true }))).toHaveLength(5);
    expect(names(filters({ active: true }))).not.toContain('Weekly');
    expect(names(filters({ active: false }))).toHaveLength(6);
  });

  it('stacks type, pinned, active, project, recent and search', () => {
    expect(names(filters({ kind: 'quickPrompts', pinned: true }))).toEqual(['Summarise']);
    expect(names(filters({ kind: 'quickPrompts', projectId: 'alpha', query: 'claude' }))).toEqual(['Review']);
    expect(names(filters({ kind: 'workflows', query: 'claude' }))).toEqual([]);
    expect(names(filters({ kind: 'workflows', active: true, recent: true }))).toEqual(['Nightly']);
    expect(names(filters({ pinned: true, recent: true, projectId: AUTOMATION_NO_PROJECT }))).toEqual(['Summarise']);
    expect(names(filters({ pinned: true, active: true, recent: true, query: 'cron' }))).toEqual(['Nightly']);
    // A blank search is no search.
    expect(names(filters({ query: '   ' }))).toHaveLength(6);
  });

  it('can leave the search to the shell, which filters on it by itself', () => {
    expect(names(filters({ kind: 'quickPrompts', query: 'claude' }), ['query'])).toEqual(['Review', 'Summarise']);
  });

  it('counts the chips that are set, not the search', () => {
    expect(activeAutomationFilterCount(filters({ kind: 'quickApis', query: 'x' }))).toBe(1);
    expect(activeAutomationFilterCount(filters({ query: 'x' }))).toBe(0);
    expect(activeAutomationFilterCount(filters({ kind: 'quickApis', pinned: true, projectId: 'alpha' }))).toBe(3);
    expect(activeAutomationFilterCount(filters({
      kind: 'skills', pinned: true, active: true, projectId: AUTOMATION_NO_PROJECT, recent: true,
    }))).toBe(5);
  });

  it('counts each type given the other filters', () => {
    expect(countAutomationKinds(library, filters())).toEqual({
      all: 6, workflows: 2, quickPrompts: 2, quickApis: 1, quickExecs: 1, skills: 0,
    });
    // The chosen type does not shrink its own list of types: they all stay reachable.
    expect(countAutomationKinds(library, filters({ kind: 'workflows', pinned: true }))).toEqual({
      all: 2, workflows: 1, quickPrompts: 1, quickApis: 0, quickExecs: 0, skills: 0,
    });
    expect(countAutomationKinds(library, filters({ projectId: 'alpha', query: 'prompt' }))).toEqual({
      all: 1, workflows: 0, quickPrompts: 1, quickApis: 0, quickExecs: 0, skills: 0,
    });
    expect(countAutomationKinds(library, filters({ recent: true }))).toEqual({
      all: 3, workflows: 2, quickPrompts: 1, quickApis: 0, quickExecs: 0, skills: 0,
    });
  });

  it('counts each project given the other filters, "no project" included', () => {
    expect(countAutomationProjects(library, filters())).toEqual({
      all: 6, alpha: 2, beta: 1, [AUTOMATION_NO_PROJECT]: 3,
    });
    // The chosen project does not shrink its own list either.
    expect(countAutomationProjects(library, filters({ projectId: 'alpha', kind: 'workflows' }))).toEqual({
      all: 2, alpha: 1, [AUTOMATION_NO_PROJECT]: 1,
    });
    expect(countAutomationProjects(library, filters({ pinned: true }))).toEqual({
      all: 2, alpha: 1, [AUTOMATION_NO_PROJECT]: 1,
    });
  });
});

describe('grouping the automation list (KT-916)', () => {
  const entry = (name: string, kind: FilterableAutomation['kind'], projectId: string | null, projectIds?: string[]) => ({
    name, kind, projectId, projectIds,
  });
  const list = [
    entry('Nightly', 'workflows', 'alpha'),
    entry('Weekly', 'workflows', null),
    entry('Review prompt', 'quickPrompts', 'beta'),
    entry('Ticket', 'quickApis', null),
    entry('Review skill', 'skills', null, ['alpha', 'beta']),
    entry('Orphan skill', 'skills', null, []),
  ];
  const summary = (groupBy: Parameters<typeof groupAutomations>[1], order: string[] = ['alpha', 'beta', 'gamma']) => (
    groupAutomations(list, groupBy, order).map(group => `${group.key}=${group.items.map(member => member.name).join('|')}`)
  );

  it('offers Type, Project and None, Type being the default', () => {
    expect(AUTOMATION_GROUP_BYS).toEqual(['kind', 'project', 'none']);
    expect(DEFAULT_AUTOMATION_GROUP_BY).toBe('kind');
    expect(AUTOMATION_GROUP_BYS.every(isAutomationGroupBy)).toBe(true);
    expect(isAutomationGroupBy('type')).toBe(false);
    expect(isAutomationGroupBy(null)).toBe(false);
  });

  it('groups by type in the order of the type list, leaving out the empty ones', () => {
    expect(AUTOMATION_KIND_FILTERS).toEqual(['workflows', 'quickPrompts', 'quickApis', 'quickExecs', 'skills']);
    expect(summary('kind')).toEqual([
      'kind:workflows=Nightly|Weekly',
      'kind:quickPrompts=Review prompt',
      'kind:quickApis=Ticket',
      'kind:skills=Review skill|Orphan skill',
    ]);
  });

  it('groups by project, then "no project", a skill listed by several projects sitting under each', () => {
    expect(summary('project')).toEqual([
      'project:alpha=Nightly|Review skill',
      'project:beta=Review prompt|Review skill',
      `project:${AUTOMATION_NO_PROJECT}=Weekly|Ticket|Orphan skill`,
    ]);
    const groups = groupAutomations(list, 'project', ['alpha', 'beta']);
    expect(groups.map(group => group.id)).toEqual(['alpha', 'beta', null]);
    expect(groups.every(group => group.by === 'project')).toBe(true);
  });

  it('follows the project order it is given, and leaves out a project it does not know', () => {
    expect(summary('project', ['beta', 'alpha'])[0]).toBe('project:beta=Review prompt|Review skill');
    // `beta` is hidden from the page: what only it holds is not listed.
    expect(summary('project', ['alpha'])).toEqual([
      'project:alpha=Nightly|Review skill',
      `project:${AUTOMATION_NO_PROJECT}=Weekly|Ticket|Orphan skill`,
    ]);
  });

  it('lists everything flat, in the given order, when there is no grouping', () => {
    const groups = groupAutomations(list, 'none');
    expect(groups).toHaveLength(1);
    expect(groups[0]).toMatchObject({ key: AUTOMATION_FLAT_GROUP_KEY, by: 'none', id: null });
    expect(groups[0].items.map(member => member.name)).toEqual(list.map(member => member.name));
  });

  it('builds no group out of an empty list, whatever the grouping', () => {
    for (const groupBy of AUTOMATION_GROUP_BYS) expect(groupAutomations([], groupBy, ['alpha'])).toEqual([]);
  });

  it('namespaces the keys by grouping, so a collapse state never crosses over', () => {
    const keys = AUTOMATION_GROUP_BYS.flatMap(groupBy => groupAutomations(list, groupBy, ['alpha', 'beta']).map(group => group.key));
    expect(new Set(keys).size).toBe(keys.length);
  });
});

describe('skills in the automation filters (KT-914)', () => {
  const skill = (searchText: string, projectIds: string[], pinned = false) => item({
    kind: 'skills', searchText, projectIds, pinned,
  });
  const mixed: FilterableAutomation[] = [
    item({ kind: 'workflows', searchText: 'Nightly Alpha cron', projectId: 'alpha', workflow: { enabled: true } }),
    skill('Review checks pull requests', ['alpha', 'beta']),
    skill('Rust idioms and tooling', ['alpha'], true),
    skill('Orphan never attached', []),
  ];
  const first = (set: AutomationFilters) => (
    mixed.filter(entry => matchesAutomationFilters(entry, set)).map(entry => entry.searchText.split(' ')[0])
  );

  it('offers Skills as a type, last, next to the four existing ones', () => {
    expect(AUTOMATION_KIND_FILTERS).toEqual(['workflows', 'quickPrompts', 'quickApis', 'quickExecs', 'skills']);
    expect(first(filters({ kind: 'skills' }))).toEqual(['Review', 'Rust', 'Orphan']);
  });

  it('counts the skills once each, whatever the number of projects they belong to', () => {
    expect(countAutomationKinds(mixed, filters())).toEqual({
      all: 4, workflows: 1, quickPrompts: 0, quickApis: 0, quickExecs: 0, skills: 3,
    });
    expect(countAutomationProjects(mixed, filters())).toEqual({
      all: 4, alpha: 3, beta: 1, [AUTOMATION_NO_PROJECT]: 1,
    });
  });

  it('reads a skill under every project that lists it, and under "no project" when none does', () => {
    expect(first(filters({ projectId: 'alpha' }))).toEqual(['Nightly', 'Review', 'Rust']);
    expect(first(filters({ projectId: 'beta' }))).toEqual(['Review']);
    expect(first(filters({ projectId: AUTOMATION_NO_PROJECT }))).toEqual(['Orphan']);
  });

  it('finds a skill by its name and by its description', () => {
    expect(first(filters({ query: 'review' }))).toEqual(['Review']);
    expect(first(filters({ query: 'pull requests' }))).toEqual(['Review']);
    expect(first(filters({ kind: 'skills', query: 'tooling' }))).toEqual(['Rust']);
  });

  it('gives a skill no off switch: it is always active, and pinned when starred', () => {
    expect(first(filters({ kind: 'skills', active: true }))).toHaveLength(3);
    expect(first(filters({ kind: 'skills', pinned: true }))).toEqual(['Rust']);
  });

  it('lists the projects of an automation once, merging its single project and its several', () => {
    expect(automationProjectIds({ projectId: null })).toEqual([]);
    expect(automationProjectIds({ projectId: 'alpha' })).toEqual(['alpha']);
    expect(automationProjectIds({ projectId: null, projectIds: ['alpha', 'beta', 'alpha'] })).toEqual(['alpha', 'beta']);
  });
});
