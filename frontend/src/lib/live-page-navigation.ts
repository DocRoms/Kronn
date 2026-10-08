import { writeActiveDiscussionId, writeDashboardPage } from './dashboard-navigation';

export const STANDALONE_DISCUSSION_HASH_PREFIX = '#discussion-';
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

export interface StandaloneLivePageRoute {
  pageId: string;
  /** View parameters from `#page/<id>?key=value`, handed to the Page as
   * `KronnPageData.page.params` (e.g. `tv=1` for a wall screen). */
  params: Record<string, string>;
}

// Display hints only: short, plain tokens so a link can never smuggle markup or
// large payloads into the sandbox. Anything else is dropped, not rejected.
const LIVE_PAGE_PARAM_KEY = /^[a-z][a-z0-9_]{0,31}$/;
const LIVE_PAGE_PARAM_VALUE = /^[A-Za-z0-9_.-]{0,64}$/;
const MAX_LIVE_PAGE_PARAMS = 8;

export function livePageViewParams(query: string): Record<string, string> {
  const params: Record<string, string> = {};
  for (const [key, value] of new URLSearchParams(query)) {
    if (Object.keys(params).length >= MAX_LIVE_PAGE_PARAMS) break;
    if (LIVE_PAGE_PARAM_KEY.test(key) && LIVE_PAGE_PARAM_VALUE.test(value) && !(key in params)) params[key] = value;
  }
  return params;
}

/** `#page/<encoded id>[?view params]`. The id is percent-encoded by
 * `standaloneLivePageUrl`, so a literal `?` can only start the parameters. */
export function standaloneLivePageRoute(hash: string): StandaloneLivePageRoute | null {
  if (!hash.startsWith(STANDALONE_LIVE_PAGE_HASH_PREFIX)) return null;
  const rest = hash.slice(STANDALONE_LIVE_PAGE_HASH_PREFIX.length);
  const queryAt = rest.indexOf('?');
  const encodedId = queryAt < 0 ? rest : rest.slice(0, queryAt);
  if (!encodedId) return null;
  try {
    const pageId = decodeURIComponent(encodedId).trim();
    if (!pageId) return null;
    return { pageId, params: queryAt < 0 ? {} : livePageViewParams(rest.slice(queryAt + 1)) };
  } catch {
    return null;
  }
}

export function standaloneLivePageId(hash: string): string | null {
  return standaloneLivePageRoute(hash)?.pageId ?? null;
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

/** The discussion a `#discussion-<id>` URL names, or null. */
export function standaloneDiscussionId(hash: string): string | null {
  if (!hash.startsWith(STANDALONE_DISCUSSION_HASH_PREFIX)) return null;
  const encodedId = hash.slice(STANDALONE_DISCUSSION_HASH_PREFIX.length).split('?')[0];
  if (!encodedId) return null;
  try {
    const discussionId = decodeURIComponent(encodedId).trim();
    return discussionId || null;
  } catch {
    return null;
  }
}

export function standaloneDiscussionUrl(
  discussionId: string,
  location: Pick<Location, 'origin' | 'pathname'> = window.location,
): string {
  return `${location.origin}${location.pathname}${STANDALONE_DISCUSSION_HASH_PREFIX}${encodeURIComponent(discussionId)}`;
}

export function standaloneDiscussionMessageId(hash: string): string | null {
  if (!standaloneDiscussionId(hash) || !hash.includes('?')) return null;
  const id = new URLSearchParams(hash.slice(hash.indexOf('?') + 1)).get('message')?.trim();
  return id && id.length <= 128 ? id : null;
}

export function standaloneDiscussionMessageUrl(discussionId: string, messageId: string,
  location: Pick<Location, 'origin' | 'pathname'> = window.location): string {
  return `${standaloneDiscussionUrl(discussionId, location)}?message=${encodeURIComponent(messageId)}`;
}

/**
 * A standalone/mosaic Live Page tab has no Dashboard shell to navigate within,
 * so an action's "open discussion" jump opens the target in a fresh tab.
 *
 * The URL carries the discussion in its hash, like the Live Page routes above.
 * Session storage is still seeded — a reload of the ORIGINAL tab must keep its
 * place — but the new tab no longer depends on it, which is what lets this
 * open with `noopener,noreferrer`: nothing has to be cloned across windows, so
 * nothing needs a back-reference to the opener. The address is also the point:
 * it can be copied, pasted and sent, which a session-storage checkpoint never
 * could.
 */
export function openStandaloneDiscussion(
  discussionId: string,
  location: Pick<Location, 'origin' | 'pathname'> = window.location,
  open: typeof window.open = window.open.bind(window),
): void {
  writeDashboardPage('discussions');
  writeActiveDiscussionId(discussionId);
  open(standaloneDiscussionUrl(discussionId, location), '_blank', 'noopener,noreferrer');
}

export const EMBED_SETTINGS_HASH = '#settings/artifacts';

/** Configuration → Artifacts → External content, with `origin` prefilled (never added). */
export function embedSettingsHash(origin?: string): string {
  return origin ? `${EMBED_SETTINGS_HASH}?origin=${encodeURIComponent(origin)}` : EMBED_SETTINGS_HASH;
}

/** `#settings/artifacts[?origin=…]`: the origin to prefill (`''` for none), or null for another hash. */
export function embedSettingsRoute(hash: string): { origin: string } | null {
  if (hash !== EMBED_SETTINGS_HASH && !hash.startsWith(`${EMBED_SETTINGS_HASH}?`)) return null;
  const origin = new URLSearchParams(hash.slice(EMBED_SETTINGS_HASH.length + 1)).get('origin')?.trim() ?? '';
  return { origin: origin.length <= 2048 ? origin : '' };
}

/**
 * Open the allowed-sites settings for a blocked origin. Inside the app the
 * current tab navigates; a standalone Page (a wall screen, a mosaic) keeps
 * running and the settings open in a new tab.
 */
export function openEmbedSettings(
  origin: string,
  location: Pick<Location, 'origin' | 'pathname' | 'hash'> = window.location,
  open: (url: string, target: string, features: string) => unknown = window.open.bind(window),
): void {
  const standalone = standaloneLivePageRoute(location.hash) !== null || standaloneLivePageMosaic(location.hash) !== null;
  if (standalone) {
    open(`${location.origin}${location.pathname}${embedSettingsHash(origin)}`, '_blank', 'noopener,noreferrer');
    return;
  }
  window.location.hash = embedSettingsHash(origin);
}
