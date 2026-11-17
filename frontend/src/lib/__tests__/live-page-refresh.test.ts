import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePage, LivePageDataDelta, LivePageDatasetView, LivePageDetail } from '../../types/generated';

vi.mock('../api', () => ({ pages: { get: vi.fn(), data: vi.fn() } }));

import { pages as pagesApi } from '../api';
import { mergeLivePageData, newestLivePageDetail, refreshLivePageDetail } from '../live-page-refresh';
import { runtimeData } from '../live-page-sandbox';

const page: LivePage = {
  id: 'page-1', project_id: null, title: 'Board', slug: 'board', current_revision_id: 'rev-1',
  data_revision: 4, created_at: '2026-08-13T10:00:00Z', updated_at: '2026-08-13T10:00:00Z',
  last_published_at: '2026-08-13T10:00:00Z', pinned: false, archived: false, html_size_bytes: 12,
};

function dataset(name: string, current: unknown, points: number[] = []): LivePageDatasetView {
  return {
    id: `dataset-${name}`, page_id: page.id, name, kind: points.length ? 'time_series' : 'snapshot',
    current, schema: null, max_points: 50_000, max_age_days: null, updated_at: page.updated_at,
    points: points.map(value => ({
      id: `point-${value}`, dataset_id: `dataset-${name}`, observed_at: `2026-08-13T10:0${value}:00Z`,
      payload: value, workflow_run_id: null,
    })),
    data_size_bytes: 1,
  } as LivePageDatasetView;
}

const held: LivePageDetail = {
  ...page,
  revision: { id: 'rev-1', page_id: page.id, revision: 1, html: '<p>board</p>', created_by_agent: null, created_at: page.created_at },
  datasets: [dataset('summary', { count: 1 }), dataset('heavy', { blob: 'kept' }), dataset('traffic', null, [1, 2])],
  slug_aliases: ['old-board'],
};

function delta(pagePatch: Partial<LivePage>, datasets: LivePageDatasetView[]): LivePageDataDelta {
  return { page: { ...page, ...pagePatch }, full: null, datasets };
}

beforeEach(() => {
  vi.mocked(pagesApi.get).mockReset();
  vi.mocked(pagesApi.data).mockReset();
});

describe('mergeLivePageData', () => {
  it('gives the Page the snapshot a full read would have given it', () => {
    const published = { data_revision: 5, last_published_at: '2026-08-13T11:00:00Z', updated_at: '2026-08-13T11:00:00Z' };
    const full: LivePageDetail = {
      ...held, ...published,
      datasets: [dataset('summary', { count: 2 }), dataset('heavy', { blob: 'kept' }), dataset('traffic', null, [1, 2, 3])],
    };

    const merged = mergeLivePageData(held, delta(published, [full.datasets[2], full.datasets[0]]));

    expect(merged).toEqual(full);
    expect(runtimeData(merged!)).toEqual(runtimeData(full));
    expect(merged!.datasets[1]).toBe(held.datasets[1]);
  });

  it('keeps the same snapshot when nothing moved, and follows the Page facts when they did', () => {
    expect(mergeLivePageData(held, delta({}, []))).toBe(held);
    const renamed = mergeLivePageData(held, delta({ title: 'Renamed' }, []));
    expect(renamed).toEqual({ ...held, title: 'Renamed' });
  });

  it('takes the full state as it is', () => {
    const full: LivePageDetail = { ...held, data_revision: 9, datasets: [] };
    expect(mergeLivePageData(held, { page: { ...page, data_revision: 9 }, full, datasets: [] })).toBe(full);
  });

  it('refuses a delta it cannot place: another Page, another HTML, an unknown dataset', () => {
    expect(mergeLivePageData(held, delta({ id: 'page-2' }, []))).toBeNull();
    expect(mergeLivePageData(held, delta({ current_revision_id: 'rev-2' }, []))).toBeNull();
    expect(mergeLivePageData(held, delta({ data_revision: 5 }, [dataset('unknown', 1)]))).toBeNull();
  });
});

describe('refreshLivePageDetail', () => {
  it('reads the whole Page when it holds nothing', async () => {
    vi.mocked(pagesApi.get).mockResolvedValue(held);
    await expect(refreshLivePageDetail('board', null)).resolves.toBe(held);
    expect(pagesApi.data).not.toHaveBeenCalled();
  });

  it('asks only for what changed since the revision and HTML it holds', async () => {
    vi.mocked(pagesApi.data).mockResolvedValue(delta({ data_revision: 5 }, [dataset('summary', { count: 2 })]));
    const next = await refreshLivePageDetail('board', held);
    expect(pagesApi.data).toHaveBeenCalledWith('board', 4, 'rev-1');
    expect(pagesApi.get).not.toHaveBeenCalled();
    expect(next.data_revision).toBe(5);
    expect(next.datasets.map(item => item.current)).toEqual([{ count: 2 }, { blob: 'kept' }, null]);
  });

  it('reads the whole Page when the delta cannot be placed', async () => {
    const replaced: LivePageDetail = { ...held, id: 'page-2' };
    vi.mocked(pagesApi.data).mockResolvedValue(delta({ id: 'page-2' }, []));
    vi.mocked(pagesApi.get).mockResolvedValue(replaced);
    await expect(refreshLivePageDetail('board', held)).resolves.toBe(replaced);
    expect(pagesApi.get).toHaveBeenCalledWith('board');
  });
});

describe('newestLivePageDetail', () => {
  it('drops a late answer older than the snapshot shown, for the same Page only', () => {
    const older = { ...held, data_revision: 3 };
    expect(newestLivePageDetail(held, older)).toBe(held);
    expect(newestLivePageDetail(held, { ...held, title: 'Same revision' }).title).toBe('Same revision');
    expect(newestLivePageDetail(held, { ...older, id: 'page-2' }).id).toBe('page-2');
    expect(newestLivePageDetail(null, older)).toBe(older);
  });
});
