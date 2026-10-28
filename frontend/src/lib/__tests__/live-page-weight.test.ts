import { describe, expect, it } from 'vitest';
import { PAGE_HTML_HEAVY_BYTES, PAGE_HTML_VERY_HEAVY_BYTES, pageHtmlWeight } from '../live-page-weight';

describe('pageHtmlWeight', () => {
  it('grades the HTML size against the 1 MB and 4 MB thresholds', () => {
    expect(pageHtmlWeight(0)).toBe('ok');
    expect(pageHtmlWeight(PAGE_HTML_HEAVY_BYTES)).toBe('ok');
    expect(pageHtmlWeight(PAGE_HTML_HEAVY_BYTES + 1)).toBe('heavy');
    expect(pageHtmlWeight(PAGE_HTML_VERY_HEAVY_BYTES)).toBe('heavy');
    expect(pageHtmlWeight(PAGE_HTML_VERY_HEAVY_BYTES + 1)).toBe('very-heavy');
    expect(pageHtmlWeight(20 * 1024 * 1024)).toBe('very-heavy');
  });
});
