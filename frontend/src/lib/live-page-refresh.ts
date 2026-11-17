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

/**
 * Orders the reads of one view. An answer is applied unless a read started
 * after it was applied first, so a failed or slow read never costs the view
 * an older good answer, and nothing is applied for a page the view left.
 */
export interface RequestSequence {
  /** A ticket for a read of `page`. Reading another page drops every read in flight. */
  next: (page: string) => number;
  /** A ticket for a read that follows `page`, `null` when the view is not on it. */
  follow: (page: string) => number | null;
  /** Whether the answer of this read can still be used. */
  wanted: (ticket: number) => boolean;
  /** `true` when the answer of this read is the one to show; it then counts as shown. */
  apply: (ticket: number) => boolean;
  /** The view closed or shows no page: every read in flight is dropped. */
  leave: () => void;
}

export function createRequestSequence(): RequestSequence {
  let issued = 0;
  let applied = 0;
  let page: string | null = null;
  const wanted = (ticket: number) => ticket > applied;
  return {
    next: target => {
      if (target !== page) { page = target; applied = issued; }
      issued += 1;
      return issued;
    },
    follow: target => {
      if (target !== page) return null;
      issued += 1;
      return issued;
    },
    wanted,
    apply: ticket => {
      if (!wanted(ticket)) return false;
      applied = ticket;
      return true;
    },
    leave: () => { page = null; applied = issued; },
  };
}
