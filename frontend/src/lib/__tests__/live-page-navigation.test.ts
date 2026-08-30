import { describe, expect, it } from 'vitest';
import {
  livePageMosaicLayouts,
  standaloneLivePageId,
  standaloneLivePageMosaic,
  standaloneLivePageMosaicUrl,
  standaloneLivePageUrl,
} from '../live-page-navigation';

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

  it('builds and parses a multi-Page mosaic URL without losing Page ids', () => {
    const url = standaloneLivePageMosaicUrl(
      ['page/équipe', 'page 2', 'page/équipe'],
      'two-columns',
      { origin: 'http://localhost:5173', pathname: '/index.html' } as Location,
    );

    expect(url).toBe('http://localhost:5173/index.html#pages/mosaic?page=page%2F%C3%A9quipe&page=page+2&layout=two-columns');
    expect(standaloneLivePageMosaic(new URL(url).hash)).toEqual({
      pageIds: ['page/équipe', 'page 2'],
      layout: 'two-columns',
    });
  });

  it('offers count-specific presets and falls back to Auto for an incompatible URL', () => {
    expect(livePageMosaicLayouts(2)).toEqual(['auto', 'two-columns', 'two-rows']);
    expect(livePageMosaicLayouts(3)).toEqual([
      'auto', 'three-top', 'three-bottom', 'three-left', 'three-right',
    ]);
    expect(livePageMosaicLayouts(4)).toEqual(['auto']);
    expect(standaloneLivePageMosaic('#pages/mosaic?page=one&page=two&page=three&layout=two-columns'))
      .toEqual({ pageIds: ['one', 'two', 'three'], layout: 'auto' });
    expect(standaloneLivePageMosaic('#pages/mosaic?page=one&layout=auto')).toBeNull();
  });
});
