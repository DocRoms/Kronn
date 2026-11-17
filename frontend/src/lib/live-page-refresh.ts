import type { LivePage, LivePageDataDelta, LivePageDetail } from '../types/generated';
import { pages as pagesApi } from './api';

const PAGE_FIELDS: (keyof LivePage)[] = [
  'id', 'project_id', 'title', 'slug', 'current_revision_id', 'data_revision', 'created_at',
  'updated_at', 'last_published_at', 'pinned', 'archived', 'html_size_bytes',
];

/**
 * `current` brought to the revision of `delta`: the changed datasets replace
 * their previous value, every other one is kept. `null` when the delta names
 * a dataset or a Page `current` does not hold: only a full read is safe then.
 */
export function mergeLivePageData(current: LivePageDetail, delta: LivePageDataDelta): LivePageDetail | null {
  if (delta.full) return delta.full;
  if (delta.page.id !== current.id || delta.page.current_revision_id !== current.revision.id) return null;
  const changed = new Map(delta.datasets.map(dataset => [dataset.name, dataset]));
  if (delta.datasets.some(dataset => !current.datasets.some(known => known.name === dataset.name))) return null;
  // Same object when nothing moved, so the host does not re-render.
  if (changed.size === 0 && PAGE_FIELDS.every(field => current[field] === delta.page[field])) return current;
  return {
    ...current,
    ...delta.page,
    datasets: current.datasets.map(dataset => changed.get(dataset.name) ?? dataset),
  };
}

/**
 * The Page as it is now. With a `current` snapshot only what changed since
 * its revision is transferred; without one, or when the snapshot cannot be
 * brought up to date, the whole Page is read.
 */
export async function refreshLivePageDetail(pageRef: string, current: LivePageDetail | null): Promise<LivePageDetail> {
  if (!current) return pagesApi.get(pageRef);
  const delta = await pagesApi.data(pageRef, current.data_revision, current.revision.id);
  return mergeLivePageData(current, delta) ?? pagesApi.get(pageRef);
}

/** A response that arrives late never replaces a newer snapshot of the same Page. */
export function newestLivePageDetail(current: LivePageDetail | null, next: LivePageDetail): LivePageDetail {
  return current && current.id === next.id && next.data_revision < current.data_revision ? current : next;
}
