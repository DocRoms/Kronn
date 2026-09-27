import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  livePageMosaicLayouts,
  openStandaloneDiscussion,
  standaloneDiscussionId,
  standaloneDiscussionUrl,
  standaloneDiscussionMessageId,
  standaloneDiscussionMessageUrl,
  standaloneLivePageId,
  standaloneLivePageMosaic,
  standaloneLivePageMosaicUrl,
  standaloneLivePageUrl,
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
