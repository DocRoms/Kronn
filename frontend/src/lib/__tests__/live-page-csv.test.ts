import { describe, expect, it } from 'vitest';
import type { LivePageDatasetView } from '../../types/generated';
import { datasetRecords, recordsToRows, valueRecords } from '../live-page-csv';

function dataset(
  current: unknown,
  kind: LivePageDatasetView['kind'] = 'snapshot',
): LivePageDatasetView {
  return {
    id: 'dataset-1', page_id: 'page-1', name: 'metrics', kind,
    current, schema: null, max_points: 50_000, max_age_days: null,
    updated_at: '2026-08-16T10:00:00Z', points: [], data_size_bytes: 42,
  };
}

describe('Live Page CSV normalization', () => {
  it('exports a top-level collection as one flattened row per item', () => {
    expect(recordsToRows(datasetRecords(dataset([
      { url: '/one', metrics: { views: 12 } },
      { url: '/two', metrics: { views: 8 } },
    ], 'collection')))).toEqual([
      ['url', 'metrics.views'],
      ['/one', 12],
      ['/two', 8],
    ]);
  });

  it('expands an object envelope array while repeating its scalar metadata', () => {
    expect(recordsToRows(valueRecords({ total: 2, items: [
      { url: '/one', views: 12 },
      { url: '/two', views: 8 },
    ] }))).toEqual([
      ['total', 'url', 'views'],
      [2, '/one', 12],
      [2, '/two', 8],
    ]);
  });

  it('zips nested parallel arrays into columns instead of JSON cells', () => {
    expect(recordsToRows(valueRecords({
      report: 'Adobe',
      labels: ['10:00', '11:00'],
      metrics: { views: [12, 8], errors: [1, 0] },
    }))).toEqual([
      ['report', 'labels', 'metrics.views', 'metrics.errors'],
      ['Adobe', '10:00', 12, 1],
      ['Adobe', '11:00', 8, 0],
    ]);
  });

  it('turns matrix rows into explicit CSV columns', () => {
    expect(recordsToRows(valueRecords([
      ['France', 12, true],
      ['Spain', 8, false],
    ]))).toEqual([
      ['column_1', 'column_2', 'column_3'],
      ['France', 12, true],
      ['Spain', 8, false],
    ]);
  });

  it('expands time-series payload rows and preserves observation metadata', () => {
    const timeSeries = dataset(null, 'time_series');
    timeSeries.points = [{
      id: 'point-1', dataset_id: timeSeries.id,
      observed_at: '2026-08-16T10:00:00Z', workflow_run_id: 'run-1',
      payload: { hosts: ['fr', 'de'], views: [12, 8] },
    }];
    expect(recordsToRows(datasetRecords(timeSeries))).toEqual([
      ['hosts', 'views', 'observed_at', 'workflow_run_id'],
      ['fr', 12, '2026-08-16T10:00:00Z', 'run-1'],
      ['de', 8, '2026-08-16T10:00:00Z', 'run-1'],
    ]);
  });
});
