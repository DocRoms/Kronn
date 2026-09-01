import { writeActiveDiscussionId, writeDashboardPage } from './dashboard-navigation';

export const STANDALONE_LIVE_PAGE_HASH_PREFIX = '#page/';
export const STANDALONE_LIVE_PAGE_MOSAIC_HASH_PREFIX = '#pages/mosaic?';

export type LivePageMosaicLayout =
  | 'auto'
  | 'two-columns'
  | 'two-rows'
  | 'three-top'
  | 'three-bottom'
  | 'three-left'
  | 'three-right';

export interface LivePageMosaicRoute {
  pageIds: string[];
  layout: LivePageMosaicLayout;
}

const TWO_PAGE_LAYOUTS: LivePageMosaicLayout[] = ['auto', 'two-columns', 'two-rows'];
const THREE_PAGE_LAYOUTS: LivePageMosaicLayout[] = [
  'auto',
  'three-top',
  'three-bottom',
  'three-left',
  'three-right',
];

export function livePageMosaicLayouts(pageCount: number): LivePageMosaicLayout[] {
  if (pageCount === 2) return TWO_PAGE_LAYOUTS;
  if (pageCount === 3) return THREE_PAGE_LAYOUTS;
  return ['auto'];
}

export function standaloneLivePageId(hash: string): string | null {
  if (!hash.startsWith(STANDALONE_LIVE_PAGE_HASH_PREFIX)) return null;
  const encodedId = hash.slice(STANDALONE_LIVE_PAGE_HASH_PREFIX.length);
  if (!encodedId) return null;
  try {
    const pageId = decodeURIComponent(encodedId).trim();
    return pageId || null;
  } catch {
    return null;
  }
}

export function standaloneLivePageUrl(
  pageId: string,
  location: Pick<Location, 'origin' | 'pathname'> = window.location,
): string {
  return `${location.origin}${location.pathname}${STANDALONE_LIVE_PAGE_HASH_PREFIX}${encodeURIComponent(pageId)}`;
}

export function standaloneLivePageMosaic(hash: string): LivePageMosaicRoute | null {
  if (!hash.startsWith(STANDALONE_LIVE_PAGE_MOSAIC_HASH_PREFIX)) return null;
  const params = new URLSearchParams(hash.slice(STANDALONE_LIVE_PAGE_MOSAIC_HASH_PREFIX.length));
  const pageIds = [...new Set(params.getAll('page').map(id => id.trim()).filter(Boolean))];
  if (pageIds.length < 2) return null;

  const requestedLayout = params.get('layout') as LivePageMosaicLayout | null;
  const availableLayouts = livePageMosaicLayouts(pageIds.length);
  const layout = requestedLayout && availableLayouts.includes(requestedLayout)
    ? requestedLayout
    : 'auto';
  return { pageIds, layout };
}

export function standaloneLivePageMosaicUrl(
  pageIds: string[],
  layout: LivePageMosaicLayout = 'auto',
  location: Pick<Location, 'origin' | 'pathname'> = window.location,
): string {
  const uniquePageIds = [...new Set(pageIds.map(id => id.trim()).filter(Boolean))];
  const params = new URLSearchParams();
  uniquePageIds.forEach(pageId => params.append('page', pageId));
  const compatibleLayout = livePageMosaicLayouts(uniquePageIds.length).includes(layout) ? layout : 'auto';
  params.set('layout', compatibleLayout);
  return `${location.origin}${location.pathname}${STANDALONE_LIVE_PAGE_MOSAIC_HASH_PREFIX}${params.toString()}`;
}

/**
 * A standalone/mosaic Live Page tab has no Dashboard shell to navigate
 * within, so an action's "open discussion" jump seeds the same session-
 * storage checkpoint Dashboard already reads on mount (`dashboard-
 * navigation.ts`) and opens it in a fresh tab. `window.open` without
 * `noopener` keeps the opener relationship the browser needs to clone
 * session storage into that new tab.
 */
export function openStandaloneDiscussion(
  discussionId: string,
  location: Pick<Location, 'origin' | 'pathname'> = window.location,
  open: typeof window.open = window.open.bind(window),
): void {
  writeDashboardPage('discussions');
  writeActiveDiscussionId(discussionId);
  open(`${location.origin}${location.pathname}`, '_blank');
}
