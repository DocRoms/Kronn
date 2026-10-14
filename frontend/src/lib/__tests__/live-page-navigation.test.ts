import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  embedSettingsOrigin,
  livePageMosaicLayouts,
  livePageMosaicRoute,
  livePageMosaicSearch,
  openEmbedSettings,
  openStandaloneDiscussion,
  standaloneDiscussionUrl,
  standaloneDiscussionMessageUrl,
  standaloneLivePageMosaicUrl,
  standaloneLivePageUrl,
  livePageViewParams,
} from '../live-page-navigation';
import { legacyHashToPath } from '../legacyRoutes';
import { embedSettingsPath } from '../routes';

const mosaicRoute = (url: string) => livePageMosaicRoute(new URL(url).searchParams);

describe('standalone Live Page navigation', () => {
  it('keeps message provenance shareable without changing the discussion identity', () => {
    const url = new URL(standaloneDiscussionMessageUrl('disc?é', 'msg/🦀', { origin: 'http://localhost:5173' }));
    expect(url.pathname).toBe('/discussions/disc%3F%C3%A9');
    expect(url.searchParams.get('message')).toBe('msg/🦀');
    expect(new URL(standaloneDiscussionUrl('disc?é', { origin: 'http://localhost:5173' })).search).toBe('');
  });
  afterEach(() => {
    sessionStorage.clear();
  });

  it('gives a Page a same-origin address that encodes its id, wherever the reader stands', () => {
    const url = standaloneLivePageUrl('page/équipe', { origin: 'http://localhost:5173' });

    expect(url).toBe('http://localhost:5173/standalone/pages/page%2F%C3%A9quipe');
    expect(decodeURIComponent(new URL(url).pathname.split('/').pop()!)).toBe('page/équipe');
  });

  it('carries view parameters in the query, apart from the Page id, which stays percent-encoded', () => {
    // A literal « ? » inside an id is always encoded by standaloneLivePageUrl.
    const url = new URL(`${standaloneLivePageUrl('a?b', { origin: 'http://localhost:5173' })}?tv=1`);
    expect(decodeURIComponent(url.pathname.split('/').pop()!)).toBe('a?b');
    expect(livePageViewParams(url.search)).toEqual({ tv: '1' });
    // The links written before keep their parameters.
    expect(legacyHashToPath('#page/4f38f114?tv=1&scene=standup')).toBe('/standalone/pages/4f38f114?tv=1&scene=standup');
    expect(legacyHashToPath('#page/?tv=1')).toBe('/standalone/pages/?tv=1');
  });

  it('keeps only short plain view parameters', () => {
    expect(livePageViewParams('tv=1&names=all&scene=stand-up.v2')).toEqual({ tv: '1', names: 'all', scene: 'stand-up.v2' });
    // markup, unicode, bad keys and oversized values are dropped, the first duplicate wins
    expect(livePageViewParams('x=<script>&é=1&Tv=1&1a=1&long=' + 'a'.repeat(65) + '&tv=1&tv=2&empty=')).toEqual({ tv: '1', empty: '' });
    const many = Array.from({ length: 12 }, (_, i) => `k${i}=v`).join('&');
    expect(Object.keys(livePageViewParams(many))).toHaveLength(8);
  });

  it('builds and parses a multi-Page mosaic address without losing Page ids', () => {
    const url = standaloneLivePageMosaicUrl(
      ['page/équipe', 'page 2', 'page/équipe'],
      'two-columns',
      { origin: 'http://localhost:5173' },
    );

    expect(url).toBe('http://localhost:5173/standalone/pages/mosaic?page=page%2F%C3%A9quipe&page=page+2&layout=two-columns');
    expect(mosaicRoute(url)).toEqual({
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
    expect(livePageMosaicRoute(new URLSearchParams('page=one&page=two&page=three&layout=two-columns')))
      .toEqual({ pageIds: ['one', 'two', 'three'], layout: 'auto' });
    expect(livePageMosaicRoute(new URLSearchParams('page=one&layout=auto'))).toBeNull();
    expect(livePageMosaicRoute(new URLSearchParams(''))).toBeNull();
    expect(livePageMosaicSearch(['one', 'two', 'three'], 'two-columns').get('layout')).toBe('auto');
  });

  it('opens a shareable address, and needs no back-reference to do it', () => {
    const open = vi.fn();

    openStandaloneDiscussion('disc-42', { origin: 'http://localhost:5173' }, open);

    // The new tab carries the discussion in its own URL, so nothing has to be
    // cloned across windows — which is what lets it open with no opener.
    expect(sessionStorage.getItem('kronn:navigation:discussion')).toBeNull();
    expect(open).toHaveBeenCalledWith(
      'http://localhost:5173/discussions/disc-42',
      '_blank',
      'noopener,noreferrer',
    );
  });

  it('builds a discussion address that survives a copy-paste, wherever the reader stands', () => {
    const url = standaloneDiscussionUrl('disc/é 42', { origin: 'http://localhost:5173' });

    expect(url).toBe('http://localhost:5173/discussions/disc%2F%C3%A9%2042');
    expect(decodeURIComponent(new URL(url).pathname.split('/').pop()!)).toBe('disc/é 42');
  });
});

describe('allowed-sites settings link', () => {
  it('round-trips the origin to prefill', () => {
    const path = embedSettingsPath('https://player.example.com:8443');
    expect(path).toBe('/config/artifacts?origin=https%3A%2F%2Fplayer.example.com%3A8443');
    expect(embedSettingsOrigin(new URL(path, 'http://localhost').search)).toBe('https://player.example.com:8443');
    expect(embedSettingsPath()).toBe('/config/artifacts');
    expect(embedSettingsOrigin('')).toBe('');
    expect(embedSettingsOrigin(`?origin=${'a'.repeat(2049)}`)).toBe('');
  });

  it('keeps the links written before pointing at the section', () => {
    expect(legacyHashToPath('#settings/artifacts?origin=https%3A%2F%2Fvimeo.com')).toBe('/config/artifacts?origin=https%3A%2F%2Fvimeo.com');
    expect(legacyHashToPath('#settings/artifacts')).toBe('/config/artifacts');
    expect(legacyHashToPath('#settings/artifactsx')).toBeNull();
  });

  it('keeps a standalone Page running and opens the settings in a new tab', () => {
    const open = vi.fn();
    const navigate = vi.fn();
    openEmbedSettings('https://vimeo.com', { origin: 'http://localhost:3140', pathname: '/standalone/pages/wall' }, open, navigate);
    expect(open).toHaveBeenCalledWith('http://localhost:3140/config/artifacts?origin=https%3A%2F%2Fvimeo.com', '_blank', 'noopener,noreferrer');
    open.mockClear();
    openEmbedSettings('https://vimeo.com', { origin: 'http://localhost:3140', pathname: '/standalone/pages/mosaic' }, open, navigate);
    expect(open).toHaveBeenCalledTimes(1);
    expect(navigate).not.toHaveBeenCalled();
  });

  it('navigates the current tab from inside the app', () => {
    const open = vi.fn();
    const navigate = vi.fn();
    openEmbedSettings('https://vimeo.com', { origin: 'http://localhost:3140', pathname: '/pages/wall' }, open, navigate);
    expect(open).not.toHaveBeenCalled();
    expect(navigate).toHaveBeenCalledWith('/config/artifacts?origin=https%3A%2F%2Fvimeo.com');
  });
});
