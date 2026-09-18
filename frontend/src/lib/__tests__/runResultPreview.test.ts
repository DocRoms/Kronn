import { describe, expect, it } from 'vitest';
import { flatEntries, quickApiPreview } from '../runResultPreview';

// Shapes recorded by the broker on a live instance.
const pullRequests = {
  status: 'OK',
  summary: 'POST https://api.github.com/graphql → 46 items',
  data: Array.from({ length: 46 }, (_, index) => ({
    headRefName: `feat/EW-${7000 + index}`,
    isDraft: false,
    number: 1900 + index,
    reviewDecision: null,
    title: `EW-${7000 + index} ${'x'.repeat(index === 0 ? 120 : 10)}`,
    url: `https://github.com/org/repo/pull/${1900 + index}`,
  })),
};

describe('quickApiPreview', () => {
  it('reads a list of records as its summary and a table of the first rows', () => {
    const preview = quickApiPreview(pullRequests)!;
    expect(preview.status).toBe('OK');
    expect(preview.summary).toBe('POST https://api.github.com/graphql → 46 items');
    // Recognisable fields first; a null field is never picked as a column.
    expect(preview.table!.columns).toEqual(['number', 'title', 'url', 'headRefName']);
    expect(preview.table!.rows).toHaveLength(5);
    expect(preview.table!.total).toBe(46);
    expect(preview.table!.rows[0][1].endsWith('…')).toBe(true);
    expect(preview.table!.rows[0][1].length).toBe(81);
  });

  it('finds the rows a payload wraps, and names where they came from', () => {
    const preview = quickApiPreview({
      status: 'OK',
      summary: 'GET https://example.atlassian.net/rest/api/3/search/jql',
      data: { total: 2, issues: [{ id: '1', key: 'EW-7791', self: 'https://x/1', fields: { summary: 'a' } }, { id: '2', key: 'EW-7787', self: 'https://x/2', fields: {} }] },
    })!;
    expect(preview.table).toEqual({
      label: 'issues',
      columns: ['key', 'id', 'self'],
      rows: [['EW-7791', '1', 'https://x/1'], ['EW-7787', '2', 'https://x/2']],
      total: 2,
    });
  });

  it('reads a list of plain values as a list, and a flat object as key → value', () => {
    expect(quickApiPreview({ data: ['a', 'b', 'c'] })!.list).toEqual({ label: null, items: ['a', 'b', 'c'], total: 3 });
    expect(quickApiPreview({ data: { version: '1.2', healthy: true } })!.entries)
      .toEqual([['version', '1.2'], ['healthy', 'true']]);
  });

  it('leaves anything that is not a broker result to the raw view', () => {
    expect(quickApiPreview('plain text')).toBeNull();
    expect(quickApiPreview({ progress: { completed: 1 } })).toBeNull();
    const nested = quickApiPreview({ data: { a: { b: 1 } } })!;
    expect([nested.table, nested.list, nested.entries]).toEqual([null, null, null]);
  });
});

describe('flatEntries', () => {
  it('reads a Quick Exec JSON output as key → value', () => {
    expect(flatEntries({ ok: true })).toEqual([['ok', 'true']]);
    expect(flatEntries({ cwd: '/repo', python: '3.14.7', missing: null }))
      .toEqual([['cwd', '/repo'], ['python', '3.14.7'], ['missing', '—']]);
  });

  it('declines anything nested or empty', () => {
    expect(flatEntries({ a: [1] })).toBeNull();
    expect(flatEntries({})).toBeNull();
    expect(flatEntries('text')).toBeNull();
  });
});
