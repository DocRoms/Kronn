import { STANDALONE_PATHS, discussionPath, embedSettingsPath, isStandalonePath, standalonePagePath } from './routes';

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

// View parameters (`/standalone/pages/<id>?tv=1&scene=standup`) are display
// hints handed to the Page as `KronnPageData.page.params`: short, plain tokens
// so a link can never smuggle markup or large payloads into the sandbox.
// Anything else is dropped, not rejected.
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

/** The shareable address of a Live Page shown on its own. */
export function standaloneLivePageUrl(
  pageId: string,
  location: Pick<Location, 'origin'> = window.location,
): string {
  return `${location.origin}${standalonePagePath(pageId)}`;
}

/** The pages and layout a mosaic address asks for, or null when it names fewer than two. */
export function livePageMosaicRoute(params: URLSearchParams): LivePageMosaicRoute | null {
  const pageIds = [...new Set(params.getAll('page').map(id => id.trim()).filter(Boolean))];
  if (pageIds.length < 2) return null;

  const requestedLayout = params.get('layout') as LivePageMosaicLayout | null;
  const availableLayouts = livePageMosaicLayouts(pageIds.length);
  const layout = requestedLayout && availableLayouts.includes(requestedLayout)
    ? requestedLayout
    : 'auto';
  return { pageIds, layout };
}

export function livePageMosaicSearch(pageIds: string[], layout: LivePageMosaicLayout = 'auto'): URLSearchParams {
  const uniquePageIds = [...new Set(pageIds.map(id => id.trim()).filter(Boolean))];
  const params = new URLSearchParams();
  uniquePageIds.forEach(pageId => params.append('page', pageId));
  const compatibleLayout = livePageMosaicLayouts(uniquePageIds.length).includes(layout) ? layout : 'auto';
  params.set('layout', compatibleLayout);
  return params;
}

/** The shareable address of a mosaic of Live Pages. */
export function standaloneLivePageMosaicUrl(
  pageIds: string[],
  layout: LivePageMosaicLayout = 'auto',
  location: Pick<Location, 'origin'> = window.location,
): string {
  return `${location.origin}${STANDALONE_PATHS.pagesMosaic}?${livePageMosaicSearch(pageIds, layout)}`;
}

/** The shareable address of a discussion. */
export function standaloneDiscussionUrl(
  discussionId: string,
  location: Pick<Location, 'origin'> = window.location,
): string {
  return `${location.origin}${discussionPath(discussionId)}`;
}

/** The shareable address of one message inside its discussion. */
export function standaloneDiscussionMessageUrl(
  discussionId: string,
  messageId: string,
  location: Pick<Location, 'origin'> = window.location,
): string {
  return `${location.origin}${discussionPath(discussionId, messageId)}`;
}

/**
 * A standalone/mosaic Live Page tab has no Dashboard shell to navigate within,
 * so an action's "open discussion" jump opens the target in a fresh tab.
 *
 * The address carries the discussion, so nothing has to be cloned across
 * windows: that is what lets this open with `noopener,noreferrer`, with no
 * back-reference to the opener. The address is also the point: it can be
 * copied, pasted and sent.
 */
export function openStandaloneDiscussion(
  discussionId: string,
  location: Pick<Location, 'origin'> = window.location,
  open: typeof window.open = window.open.bind(window),
): void {
  open(standaloneDiscussionUrl(discussionId, location), '_blank', 'noopener,noreferrer');
}

const MAX_EMBED_ORIGIN_CHARS = 2048;

/** The site an allowed-sites address asks to type in (`''` for none). */
export function embedSettingsOrigin(search: string): string {
  const origin = new URLSearchParams(search).get('origin')?.trim() ?? '';
  return origin.length <= MAX_EMBED_ORIGIN_CHARS ? origin : '';
}

/**
 * Follow an address of this app in the current tab, from code that has no
 * handle on the router: push the entry and announce it the way the browser
 * announces Back, so the router follows the address.
 */
export function navigateAppTab(path: string): void {
  window.history.pushState(null, '', path);
  window.dispatchEvent(new PopStateEvent('popstate'));
}

/**
 * Open the allowed-sites settings for a blocked origin. Inside the app the
 * current tab navigates; a standalone Page (a wall screen, a mosaic) keeps
 * running and the settings open in a new tab.
 */
export function openEmbedSettings(
  origin: string,
  location: Pick<Location, 'origin' | 'pathname'> = window.location,
  open: (url: string, target: string, features: string) => unknown = window.open.bind(window),
  navigate: (path: string) => unknown = navigateAppTab,
): void {
  const path = embedSettingsPath(origin);
  if (isStandalonePath(location.pathname)) {
    open(`${location.origin}${path}`, '_blank', 'noopener,noreferrer');
    return;
  }
  navigate(path);
}
