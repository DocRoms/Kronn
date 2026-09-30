import { describe, expect, it } from 'vitest';
import {
  activeAutomationFilterCount,
  AUTOMATION_KIND_FILTERS,
  AUTOMATION_NO_PROJECT,
  automationProjectIds,
  countAutomationKinds,
  countAutomationStates,
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
  item({ kind: 'workflows', searchText: 'Nightly Alpha cron', projectId: 'alpha', pinned: true, workflow: { enabled: true } }),
  item({ kind: 'workflows', searchText: 'Weekly report manual', workflow: { enabled: false } }),
  item({ kind: 'quickPrompts', searchText: 'Review prompt Claude', projectId: 'alpha' }),
  item({ kind: 'quickPrompts', searchText: 'Summarise prompt Codex', pinned: true }),
  item({ kind: 'quickApis', searchText: 'Create ticket jira' }),
  item({ kind: 'quickExecs', searchText: 'CloudWatch aws json', projectId: 'beta' }),
];
const filters = (over: Partial<AutomationFilters> = {}): AutomationFilters => ({ ...NO_AUTOMATION_FILTERS, ...over });
const names = (set: AutomationFilters, ignore: readonly AutomationFilterAxis[] = []) => (
  library.filter(entry => matchesAutomationFilters(entry, set, ignore)).map(entry => entry.searchText.split(' ')[0])
);

describe('automation filters', () => {
  it('keeps everything when no filter is set', () => {
    expect(names(filters())).toHaveLength(6);
    expect(activeAutomationFilterCount(filters())).toBe(0);
  });

  it('filters by type, by state and by project', () => {
    expect(names(filters({ kind: 'quickPrompts' }))).toEqual(['Review', 'Summarise']);
    expect(names(filters({ state: 'favorites' }))).toEqual(['Nightly', 'Summarise']);
    expect(names(filters({ projectId: 'alpha' }))).toEqual(['Nightly', 'Review']);
    expect(names(filters({ projectId: AUTOMATION_NO_PROJECT }))).toEqual(['Weekly', 'Summarise', 'Create']);
  });

  it('treats only a disabled workflow as inactive: a Quick item has no off switch', () => {
    expect(names(filters({ state: 'inactive' }))).toEqual(['Weekly']);
    expect(names(filters({ state: 'active' }))).toHaveLength(5);
    expect(names(filters({ state: 'active' }))).not.toContain('Weekly');
  });

  it('stacks type, state, project and search', () => {
    expect(names(filters({ kind: 'quickPrompts', state: 'favorites' }))).toEqual(['Summarise']);
    expect(names(filters({ kind: 'quickPrompts', projectId: 'alpha', query: 'claude' }))).toEqual(['Review']);
    expect(names(filters({ kind: 'workflows', query: 'claude' }))).toEqual([]);
    // A blank search is no search.
    expect(names(filters({ query: '   ' }))).toHaveLength(6);
  });

  it('can leave the search to the shell, which filters on it by itself', () => {
    expect(names(filters({ kind: 'quickPrompts', query: 'claude' }), ['query'])).toEqual(['Review', 'Summarise']);
  });

  it('counts the set filters, not the search', () => {
    expect(activeAutomationFilterCount(filters({ kind: 'quickApis', query: 'x' }))).toBe(1);
    expect(activeAutomationFilterCount(filters({ kind: 'quickApis', state: 'favorites', projectId: 'alpha' }))).toBe(3);
  });

  it('counts each type chip given the other filters', () => {
    expect(countAutomationKinds(library, filters())).toEqual({
      all: 6, workflows: 2, quickPrompts: 2, quickApis: 1, quickExecs: 1, skills: 0,
    });
    // The chosen type does not shrink its own row of chips: they all stay reachable.
    expect(countAutomationKinds(library, filters({ kind: 'workflows', state: 'favorites' }))).toEqual({
      all: 2, workflows: 1, quickPrompts: 1, quickApis: 0, quickExecs: 0, skills: 0,
    });
    expect(countAutomationKinds(library, filters({ projectId: 'alpha', query: 'prompt' }))).toEqual({
      all: 1, workflows: 0, quickPrompts: 1, quickApis: 0, quickExecs: 0, skills: 0,
    });
  });

  it('counts each state chip given the other filters', () => {
    expect(countAutomationStates(library, filters())).toEqual({ all: 6, favorites: 2, active: 5, inactive: 1 });
    expect(countAutomationStates(library, filters({ kind: 'workflows' }))).toEqual({
      all: 2, favorites: 1, active: 1, inactive: 1,
    });
    expect(countAutomationStates(library, filters({ kind: 'quickApis', state: 'inactive' }))).toEqual({
      all: 1, favorites: 0, active: 1, inactive: 0,
    });
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
    expect(AUTOMATION_KIND_FILTERS).toEqual(['workflows', 'quickApis', 'quickPrompts', 'quickExecs', 'skills']);
    expect(first(filters({ kind: 'skills' }))).toEqual(['Review', 'Rust', 'Orphan']);
  });

  it('counts the skills once each, whatever the number of projects they belong to', () => {
    expect(countAutomationKinds(mixed, filters())).toEqual({
      all: 4, workflows: 1, quickPrompts: 0, quickApis: 0, quickExecs: 0, skills: 3,
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

  it('gives a skill no off switch: it is always active, and a favorite when starred', () => {
    expect(first(filters({ kind: 'skills', state: 'active' }))).toHaveLength(3);
    expect(first(filters({ kind: 'skills', state: 'inactive' }))).toEqual([]);
    expect(first(filters({ kind: 'skills', state: 'favorites' }))).toEqual(['Rust']);
  });

  it('lists the projects of an automation once, merging its single project and its several', () => {
    expect(automationProjectIds({ projectId: null })).toEqual([]);
    expect(automationProjectIds({ projectId: 'alpha' })).toEqual(['alpha']);
    expect(automationProjectIds({ projectId: null, projectIds: ['alpha', 'beta', 'alpha'] })).toEqual(['alpha', 'beta']);
  });
});
