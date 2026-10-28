// #252: Kronn sets no size limit on a Page's HTML. It grades the weight so a
// heavy page is noticed before it gets slow. Same thresholds as the backend's
// write notice (backend/src/api/live_pages.rs).
export const PAGE_HTML_HEAVY_BYTES = 1024 * 1024;
export const PAGE_HTML_VERY_HEAVY_BYTES = 4 * 1024 * 1024;

export type PageHtmlWeight = 'ok' | 'heavy' | 'very-heavy';

export function pageHtmlWeight(bytes: number): PageHtmlWeight {
  if (bytes > PAGE_HTML_VERY_HEAVY_BYTES) return 'very-heavy';
  if (bytes > PAGE_HTML_HEAVY_BYTES) return 'heavy';
  return 'ok';
}

/** Binary units, as the Pages UI has always printed sizes. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}
