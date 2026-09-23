import { livePageMosaicLayouts, type LivePageMosaicLayout } from './live-page-navigation';

export const DISCUSSION_MOSAIC_PREFIX = '#discussions/mosaic?';
export const MAX_MOSAIC_DISCUSSIONS = 12;
export type DiscussionMosaicLayout = LivePageMosaicLayout;

export function discussionMosaicRoute(hash: string): { discussionIds: string[]; layout: DiscussionMosaicLayout } | null {
  if (!hash.startsWith(DISCUSSION_MOSAIC_PREFIX)) return null;
  const params = new URLSearchParams(hash.slice(DISCUSSION_MOSAIC_PREFIX.length));
  const discussionIds = [...new Set(params.getAll('discussion').map(id => id.trim()).filter(Boolean))];
  if (discussionIds.length < 2 || discussionIds.length > MAX_MOSAIC_DISCUSSIONS
    || discussionIds.some(id => id.length > 128 || id.includes(','))) return null;
  const requested = params.get('layout') as DiscussionMosaicLayout;
  return {
    discussionIds,
    layout: livePageMosaicLayouts(discussionIds.length).includes(requested) ? requested : 'auto',
  };
}

export function discussionMosaicUrl(
  ids: string[],
  layout: DiscussionMosaicLayout = 'auto',
  location: Pick<Location, 'origin' | 'pathname'> = window.location,
): string {
  const unique = [...new Set(ids.map(id => id.trim()).filter(Boolean))];
  const params = new URLSearchParams();
  unique.forEach(id => params.append('discussion', id));
  params.set('layout', livePageMosaicLayouts(unique.length).includes(layout) ? layout : 'auto');
  return `${location.origin}${location.pathname}${DISCUSSION_MOSAIC_PREFIX}${params.toString()}`;
}
