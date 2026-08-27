import { describe, expect, it } from 'vitest';
import { standaloneLivePageId, standaloneLivePageUrl } from '../live-page-navigation';

describe('standalone Live Page navigation', () => {
  it('builds a stable same-origin URL and decodes its Page id', () => {
    const url = standaloneLivePageUrl('page/équipe', {
      origin: 'http://localhost:5173',
      pathname: '/index.html',
    } as Location);

    expect(url).toBe('http://localhost:5173/index.html#page/page%2F%C3%A9quipe');
    expect(standaloneLivePageId(new URL(url).hash)).toBe('page/équipe');
  });

  it('ignores unrelated, empty and malformed hashes', () => {
    expect(standaloneLivePageId('#project-page-1')).toBeNull();
    expect(standaloneLivePageId('#page/')).toBeNull();
    expect(standaloneLivePageId('#page/%E0%A4%A')).toBeNull();
  });
});
