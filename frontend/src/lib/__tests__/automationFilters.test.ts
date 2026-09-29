import { describe, expect, it } from 'vitest';
import {
  activeAutomationFilterCount,
  AUTOMATION_NO_PROJECT,
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
      all: 6, workflows: 2, quickPrompts: 2, quickApis: 1, quickExecs: 1,
    });
    // The chosen type does not shrink its own row of chips: they all stay reachable.
    expect(countAutomationKinds(library, filters({ kind: 'workflows', state: 'favorites' }))).toEqual({
      all: 2, workflows: 1, quickPrompts: 1, quickApis: 0, quickExecs: 0,
    });
    expect(countAutomationKinds(library, filters({ projectId: 'alpha', query: 'prompt' }))).toEqual({
      all: 1, workflows: 0, quickPrompts: 1, quickApis: 0, quickExecs: 0,
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
