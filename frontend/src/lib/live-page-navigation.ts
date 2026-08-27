export const STANDALONE_LIVE_PAGE_HASH_PREFIX = '#page/';

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
