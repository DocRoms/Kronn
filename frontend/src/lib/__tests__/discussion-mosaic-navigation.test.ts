import { describe, expect, it } from 'vitest';
import { discussionMosaicRoute, discussionMosaicSearch, discussionMosaicUrl } from '../discussion-mosaic-navigation';

const location = { origin: 'https://kronn.example' };
const route = (url: string) => discussionMosaicRoute(new URL(url).searchParams);

describe('discussion mosaic navigation', () => {
  it('round-trips selection order and layout through a same-origin address', () => {
    const url = new URL(discussionMosaicUrl([' b ', 'a/é?', 'b', 'c'], 'three-left', location));
    expect(url.origin + url.pathname).toBe('https://kronn.example/standalone/discussions/mosaic');
    expect(discussionMosaicRoute(url.searchParams)).toEqual({ discussionIds: ['b', 'a/é?', 'c'], layout: 'three-left' });
  });
  it('writes the address wherever the reader stands', () => {
    // The mosaic is a whole-window view: its address does not depend on the
    // page it was opened from.
    expect(discussionMosaicUrl(['a', 'b'], 'auto', location)).toBe(discussionMosaicUrl(['a', 'b'], 'auto', { origin: 'https://kronn.example' }));
    expect(discussionMosaicUrl(['a', 'b'], 'auto', { origin: 'http://localhost:5173' })).toBe('http://localhost:5173/standalone/discussions/mosaic?discussion=a&discussion=b&layout=auto');
  });
  it('falls back to auto for an incompatible preset', () => {
    expect(discussionMosaicRoute(new URLSearchParams('discussion=a&discussion=b&layout=three-left'))?.layout).toBe('auto');
    expect(discussionMosaicSearch(['a', 'b'], 'three-left').get('layout')).toBe('auto');
  });
  it('accepts exactly twelve, rejects single, duplicate-only, malformed and excessive selections', () => {
    const ids = Array.from({ length: 12 }, (_, i) => `room-${i}`);
    expect(route(discussionMosaicUrl(ids, 'auto', location))?.discussionIds).toEqual(ids);
    for (const selected of [[], ['a'], ['a', 'a'], ['a,b', 'c'], ['x'.repeat(129), 'b'], [...ids, 'overflow']]) {
      expect(route(discussionMosaicUrl(selected, 'auto', location))).toBeNull();
    }
    expect(discussionMosaicRoute(new URLSearchParams('page=a&page=b'))).toBeNull();
  });
});
