import { livePageMosaicLayouts, type LivePageMosaicLayout } from './live-page-navigation';
import { STANDALONE_PATHS } from './routes';

export const MAX_MOSAIC_DISCUSSIONS = 12;
export type DiscussionMosaicLayout = LivePageMosaicLayout;

/** The discussions and layout a mosaic address asks for, or null when it is not a valid mosaic. */
export function discussionMosaicRoute(params: URLSearchParams): { discussionIds: string[]; layout: DiscussionMosaicLayout } | null {
  const discussionIds = [...new Set(params.getAll('discussion').map(id => id.trim()).filter(Boolean))];
  if (discussionIds.length < 2 || discussionIds.length > MAX_MOSAIC_DISCUSSIONS
    || discussionIds.some(id => id.length > 128 || id.includes(','))) return null;
  const requested = params.get('layout') as DiscussionMosaicLayout;
  return {
    discussionIds,
    layout: livePageMosaicLayouts(discussionIds.length).includes(requested) ? requested : 'auto',
  };
}

export function discussionMosaicSearch(ids: string[], layout: DiscussionMosaicLayout = 'auto'): URLSearchParams {
  const unique = [...new Set(ids.map(id => id.trim()).filter(Boolean))];
  const params = new URLSearchParams();
  unique.forEach(id => params.append('discussion', id));
  params.set('layout', livePageMosaicLayouts(unique.length).includes(layout) ? layout : 'auto');
  return params;
}

/** The shareable address of a mosaic of discussions. */
export function discussionMosaicUrl(
  ids: string[],
  layout: DiscussionMosaicLayout = 'auto',
  location: Pick<Location, 'origin'> = window.location,
): string {
  return `${location.origin}${STANDALONE_PATHS.discussionsMosaic}?${discussionMosaicSearch(ids, layout)}`;
}
