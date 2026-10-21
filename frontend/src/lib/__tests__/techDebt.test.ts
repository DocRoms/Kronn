import { describe, it, expect } from 'vitest';
import {
  groupTds, isTdSettled, nextUnsettled, shortTdId, visibleTds,
} from '../techDebt';
import type { TdReviewItem } from '../../types/generated';
import { tdItem } from '../../test/tdFixtures';

describe('techDebt helpers', () => {
  it('treats deferred as still to settle', () => {
    expect(isTdSettled('pending')).toBe(false);
    expect(isTdSettled('deferred')).toBe(false);
    expect(isTdSettled('confirmed')).toBe(true);
    expect(isTdSettled('accepted_decision')).toBe(true);
    expect(isTdSettled('rejected')).toBe(true);
  });

  it('filters by state, severity and a query over id, title and files', () => {
    const items = [
      tdItem('TD-20261001-a', 'Critical'),
      tdItem('TD-20261001-b', 'High', 'deferred'),
      tdItem('TD-20261001-c', 'Low', 'confirmed', { pointers: [{ path: 'frontend/App.tsx', line: null }] }),
    ];
    const ids = (list: TdReviewItem[]) => list.map(i => i.id);
    expect(ids(visibleTds(items, 'todo', new Set(), ''))).toEqual(['TD-20261001-a', 'TD-20261001-b']);
    expect(ids(visibleTds(items, 'done', new Set(), ''))).toEqual(['TD-20261001-c']);
    expect(ids(visibleTds(items, 'all', new Set(['High']), ''))).toEqual(['TD-20261001-b']);
    expect(ids(visibleTds(items, 'all', new Set(), 'app.tsx'))).toEqual(['TD-20261001-c']);
    expect(ids(visibleTds(items, 'all', new Set(), 'titre td-20261001-a'))).toEqual(['TD-20261001-a']);
  });

  it('groups by severity order and finds the next unsettled item, wrapping', () => {
    const items = [
      tdItem('TD-1', 'Low'),
      tdItem('TD-2', 'Critical', 'confirmed'),
      tdItem('TD-3', 'High'),
    ];
    const groups = groupTds(items);
    expect(groups.map(g => g.severity)).toEqual(['Critical', 'High', 'Low']);
    const ordered = groups.flatMap(g => g.items);
    expect(nextUnsettled(ordered, 'TD-2')?.id).toBe('TD-3');
    expect(nextUnsettled(ordered, 'TD-1')?.id).toBe('TD-3');
    expect(nextUnsettled([tdItem('TD-9', 'Low')], 'TD-9')).toBeNull();
    expect(shortTdId('TD-20261001-cache-leak')).toBe('cache-leak');
  });
});
