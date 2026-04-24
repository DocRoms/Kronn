// Tests for the auto-derived path suggestions. Pure logic — no DOM.
//
// Coverage focuses on the three response shapes that drive 90% of real
// workflows: (a) array of objects (Chartbeat top-pages, Jira issues),
// (b) flat scalar response (single counter), (c) nested array under a
// top-level key.

import { describe, it, expect } from 'vitest';
import { suggestPaths } from '../apiCallSuggestions';

describe('suggestPaths', () => {
  it('returns an empty list for null / scalar / undefined inputs', () => {
    expect(suggestPaths(null)).toEqual([]);
    expect(suggestPaths(42 as unknown)).toEqual([]);
    expect(suggestPaths(undefined as unknown)).toEqual([]);
  });

  it('surfaces "All <field>" + "Iterate" + "First item" for an array of objects (Chartbeat shape)', () => {
    const chartbeatPages = {
      pages: [
        { title: 'Article A', path: '/a', visitors: 120 },
        { title: 'Article B', path: '/b', visitors: 99 },
      ],
    };
    const sugg = suggestPaths(chartbeatPages);

    // The "All titles" suggestion is the most useful for fan-out → must
    // be present and use the priority-ranked scalar field (`title` wins
    // over `path` because of SCALAR_FIELD_PRIORITY ordering).
    const allTitles = sugg.find(s => s.path === '$.pages[*].title');
    expect(allTitles).toBeDefined();
    expect(allTitles!.kind).toBe('array-field');
    expect(allTitles!.args).toEqual(['title', 2]);

    // Iterate → wildcard path on the array.
    expect(sugg.find(s => s.path === '$.pages[*]')).toBeDefined();

    // First item — handy "tester avant fan-out" path.
    expect(sugg.find(s => s.path === '$.pages[0]')).toBeDefined();
  });

  it('detects counter fields by name (total / count / totalCount)', () => {
    // Jira-style flat counter alongside an array.
    const jiraSearch = { issues: [], total: 173, maxResults: 50 };
    const sugg = suggestPaths(jiraSearch);

    const counter = sugg.find(s => s.path === '$.total');
    expect(counter).toBeDefined();
    expect(counter!.kind).toBe('count');
    expect(counter!.args).toEqual(['total', 173]);
    expect(counter!.sample).toBe('173');
  });

  it('falls back to the first scalar key when no priority field matches', () => {
    // Custom API with a non-priority scalar — algorithm still picks one.
    const odd = { items: [{ uuid: 'x-123', score: 0.42 }] };
    const sugg = suggestPaths(odd);
    const allField = sugg.find(s => s.kind === 'array-field');
    expect(allField).toBeDefined();
    // `uuid` is not in SCALAR_FIELD_PRIORITY but is the first scalar
    // encountered → it wins over `score`.
    expect(allField!.path).toBe('$.items[*].uuid');
  });

  it('caps the suggestion list at MAX_SUGGESTIONS to avoid wall-of-chips', () => {
    // Build a response with many arrays at different depths.
    const wide = {
      a: [{ id: 1 }, { id: 2 }],
      b: [{ name: 'x' }, { name: 'y' }],
      c: [{ key: 'k1' }],
      d: [{ slug: 's1' }],
      total: 7,
      count: 7,
    };
    const sugg = suggestPaths(wide);
    expect(sugg.length).toBeLessThanOrEqual(6);
  });

  it('does not duplicate the same path across heuristics', () => {
    const dupe = { items: [{ id: 1 }] };
    const sugg = suggestPaths(dupe);
    const paths = sugg.map(s => s.path);
    expect(new Set(paths).size).toBe(paths.length);
  });
});
