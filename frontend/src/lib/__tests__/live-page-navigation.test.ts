import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  embedSettingsHash,
  embedSettingsRoute,
  livePageMosaicLayouts,
  openEmbedSettings,
  openStandaloneDiscussion,
  standaloneDiscussionId,
  standaloneDiscussionUrl,
  standaloneDiscussionMessageId,
  standaloneDiscussionMessageUrl,
  standaloneLivePageId,
  standaloneLivePageMosaic,
  standaloneLivePageMosaicUrl,
  standaloneLivePageRoute,
  standaloneLivePageUrl,
  livePageViewParams,
} from '../live-page-navigation';

describe('standalone Live Page navigation', () => {
  it('keeps message provenance shareable without changing the discussion identity', () => {
    const url = standaloneDiscussionMessageUrl('disc?é', 'msg/🦀', { origin: 'http://localhost:5173', pathname: '/' });
    const hash = new URL(url).hash;
    expect(standaloneDiscussionId(hash)).toBe('disc?é');
    expect(standaloneDiscussionMessageId(hash)).toBe('msg/🦀');
    expect(standaloneDiscussionMessageId('#discussion-one')).toBeNull();
    expect(standaloneDiscussionMessageId('#page/one?message=msg')).toBeNull();
    expect(standaloneDiscussionMessageId('#discussion-one?message=')).toBeNull();
  });
  afterEach(() => {
    sessionStorage.clear();
  });

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

  it('splits view parameters from the Page id, which stays percent-encoded', () => {
    expect(standaloneLivePageRoute('#page/4f38f114?tv=1')).toEqual({ pageId: '4f38f114', params: { tv: '1' } });
    expect(standaloneLivePageRoute('#page/4f38f114')).toEqual({ pageId: '4f38f114', params: {} });
    expect(standaloneLivePageId('#page/4f38f114?tv=1&scene=standup')).toBe('4f38f114');
    // A literal « ? » inside an id is always encoded by standaloneLivePageUrl.
    const url = standaloneLivePageUrl('a?b', { origin: 'http://localhost:5173', pathname: '/' } as Location);
    expect(standaloneLivePageRoute(new URL(url).hash + '?tv=1')).toEqual({ pageId: 'a?b', params: { tv: '1' } });
    expect(standaloneLivePageRoute('#page/?tv=1')).toBeNull();
  });

  it('keeps only short plain view parameters', () => {
    expect(livePageViewParams('tv=1&names=all&scene=stand-up.v2')).toEqual({ tv: '1', names: 'all', scene: 'stand-up.v2' });
    // markup, unicode, bad keys and oversized values are dropped, the first duplicate wins
    expect(livePageViewParams('x=<script>&é=1&Tv=1&1a=1&long=' + 'a'.repeat(65) + '&tv=1&tv=2&empty=')).toEqual({ tv: '1', empty: '' });
    const many = Array.from({ length: 12 }, (_, i) => `k${i}=v`).join('&');
    expect(Object.keys(livePageViewParams(many))).toHaveLength(8);
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

  it('opens a shareable address, and needs no back-reference to do it', () => {
    const open = vi.fn();

    openStandaloneDiscussion('disc-42', { origin: 'http://localhost:5173', pathname: '/index.html' } as Location, open);

    // Still seeded: a reload of the ORIGINAL tab must keep its place.
    expect(sessionStorage.getItem('kronn:navigation:page')).toBe('discussions');
    expect(sessionStorage.getItem('kronn:navigation:discussion')).toBe('disc-42');
    // The new tab carries the discussion in its own URL, so nothing has to be
    // cloned across windows — which is what lets it open with no opener.
    expect(open).toHaveBeenCalledWith(
      'http://localhost:5173/index.html#discussion-disc-42',
      '_blank',
      'noopener,noreferrer',
    );
  });

  it('builds a discussion address that survives a copy-paste, and reads it back', () => {
    const url = standaloneDiscussionUrl('disc/é 42', {
      origin: 'http://localhost:5173',
      pathname: '/index.html',
    } as Location);

    expect(url).toBe('http://localhost:5173/index.html#discussion-disc%2F%C3%A9%2042');
    expect(standaloneDiscussionId(new URL(url).hash)).toBe('disc/é 42');
  });

  it('ignores a hash that names no discussion', () => {
    expect(standaloneDiscussionId('#page/one')).toBeNull();
    expect(standaloneDiscussionId('#discussion-')).toBeNull();
    expect(standaloneDiscussionId('#discussion-%E0%A4%A')).toBeNull();
    expect(standaloneDiscussionId('')).toBeNull();
  });
});

describe('allowed-sites settings link', () => {
  it('round-trips the origin to prefill, and ignores other hashes', () => {
    expect(embedSettingsHash('https://player.example.com:8443')).toBe('#settings/artifacts?origin=https%3A%2F%2Fplayer.example.com%3A8443');
    expect(embedSettingsRoute(embedSettingsHash('https://player.example.com:8443'))).toEqual({ origin: 'https://player.example.com:8443' });
    expect(embedSettingsRoute('#settings/artifacts')).toEqual({ origin: '' });
    expect(embedSettingsRoute('#settings/artifactsx')).toBeNull();
    expect(embedSettingsRoute('#page/abc')).toBeNull();
  });

  it('keeps a standalone Page running and opens the settings in a new tab', () => {
    const open = vi.fn();
    openEmbedSettings('https://vimeo.com', { origin: 'http://localhost:3140', pathname: '/', hash: '#page/wall?tv=1' }, open);
    expect(open).toHaveBeenCalledWith('http://localhost:3140/#settings/artifacts?origin=https%3A%2F%2Fvimeo.com', '_blank', 'noopener,noreferrer');
    open.mockClear();
    openEmbedSettings('https://vimeo.com', { origin: 'http://localhost:3140', pathname: '/', hash: '#pages/mosaic?page=a&page=b' }, open);
    expect(open).toHaveBeenCalledTimes(1);
  });

  it('navigates the current tab from inside the app', () => {
    const open = vi.fn();
    openEmbedSettings('https://vimeo.com', { origin: 'http://localhost:3140', pathname: '/', hash: '' }, open);
    expect(open).not.toHaveBeenCalled();
    expect(window.location.hash).toBe('#settings/artifacts?origin=https%3A%2F%2Fvimeo.com');
    window.location.hash = '';
  });
});
