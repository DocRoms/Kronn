import { describe, expect, it } from 'vitest';
import { discussionMosaicRoute, discussionMosaicUrl } from '../discussion-mosaic-navigation';

const location = { origin: 'https://kronn.example', pathname: '/app/' };
describe('discussion mosaic navigation', () => {
  it('round-trips selection order and layout without opener storage', () => {
    const url = new URL(discussionMosaicUrl([' b ', 'a/é?', 'b', 'c'], 'three-left', location));
    expect(url.origin + url.pathname).toBe('https://kronn.example/app/');
    expect(discussionMosaicRoute(url.hash)).toEqual({ discussionIds: ['b', 'a/é?', 'c'], layout: 'three-left' });
  });
  it('falls back to auto for an incompatible preset', () => {
    expect(discussionMosaicRoute('#discussions/mosaic?discussion=a&discussion=b&layout=three-left')?.layout).toBe('auto');
  });
  it('accepts exactly twelve, rejects single, duplicate-only, malformed and excessive selections', () => {
    const ids = Array.from({ length: 12 }, (_, i) => `room-${i}`);
    expect(discussionMosaicRoute(new URL(discussionMosaicUrl(ids, 'auto', location)).hash)?.discussionIds).toEqual(ids);
    for (const selected of [[], ['a'], ['a', 'a'], ['a,b', 'c'], ['x'.repeat(129), 'b'], [...ids, 'overflow']]) {
      expect(discussionMosaicRoute(new URL(discussionMosaicUrl(selected, 'auto', location)).hash)).toBeNull();
    }
    expect(discussionMosaicRoute('#pages/mosaic?page=a&page=b')).toBeNull();
  });
});
