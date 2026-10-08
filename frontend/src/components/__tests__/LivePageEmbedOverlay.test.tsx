// @vitest-environment-options {"settings":{"disableIframePageLoading":true}}
// LivePageEmbedOverlay: host-drawn third-party content over a Live Page.
//
// The Page iframe is sandboxed without allow-same-origin, so a player nested in
// it never plays. The host draws it instead, only for sites this Kronn allows:
// these tests pin that nothing loads before that list is known, that a refused
// site gets a Kronn warning (never the content), that revoking a site takes the
// content down, that the content is cut to the Page's own scrolling containers,
// and that a player survives position updates (a re-mounted iframe would reload).

import { readFileSync } from 'node:fs';
import { createRef } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, waitFor } from '@testing-library/react';

vi.mock('../../lib/api', () => ({
  config: {
    getEmbedOrigins: vi.fn(),
    changeEmbedOrigins: vi.fn(),
    getUiLanguage: vi.fn(() => Promise.resolve('en')),
    saveUiLanguage: vi.fn(() => Promise.resolve()),
  },
}));

import { config as configApi } from '../../lib/api';
import { LivePageEmbedOverlay } from '../LivePageEmbedOverlay';
import { I18nProvider } from '../../lib/I18nContext';
import type { LivePageEmbedPlacement } from '../../lib/live-page-sandbox';
import { changeEmbedAllowedOrigins, resetEmbedAllowedOriginsForTests } from '../../hooks/useEmbedAllowedOrigins';

const CSS = readFileSync('src/components/LivePageEmbedOverlay.css', 'utf8');
const ACTION_CSS = readFileSync('src/components/LivePageActionOverlay.css', 'utf8');
const SUNO = 'https://suno.com/embed/08fca036-317a-4cd5-9860-166c62c0180f';

function placement(over: Partial<LivePageEmbedPlacement> = {}): LivePageEmbedPlacement {
  return {
    key: 'esuno:0',
    url: SUNO,
    rect: { left: 12, top: 40, width: 320, height: 152 },
    visible: true,
    ...over,
  };
}

function renderOverlay(embeds: LivePageEmbedPlacement[], onConfigureOrigin = vi.fn()) {
  const frameRef = createRef<HTMLIFrameElement>();
  const ui = (list: LivePageEmbedPlacement[]) => (
    <I18nProvider>
      <iframe ref={frameRef} title="page" sandbox="allow-scripts" />
      <LivePageEmbedOverlay frameRef={frameRef} embeds={list} onConfigureOrigin={onConfigureOrigin} />
    </I18nProvider>
  );
  const result = render(ui(embeds));
  return { ...result, onConfigureOrigin, update: (list: LivePageEmbedPlacement[]) => result.rerender(ui(list)) };
}

const players = (container: HTMLElement) =>
  Array.from(container.querySelectorAll<HTMLIFrameElement>('.live-page-embed-overlay__player'));
const warnings = (container: HTMLElement) =>
  Array.from(container.querySelectorAll<HTMLElement>('.live-page-embed-overlay__blocked'));

beforeEach(() => {
  resetEmbedAllowedOriginsForTests();
  vi.mocked(configApi.getEmbedOrigins).mockResolvedValue(['https://suno.com']);
});

afterEach(() => {
  vi.clearAllMocks();
});

describe('LivePageEmbedOverlay', () => {
  it('loads nothing before the allowed sites are known', async () => {
    let answer: (origins: string[]) => void = () => {};
    vi.mocked(configApi.getEmbedOrigins).mockReturnValue(new Promise(resolve => { answer = resolve; }));
    const { container } = renderOverlay([placement()]);
    expect(players(container)).toHaveLength(0);
    expect(warnings(container)).toHaveLength(0);
    await act(async () => { answer(['https://suno.com']); });
    expect(players(container)).toHaveLength(1);
  });

  it('draws the Page URL for an allowed site, at the reported rectangle, as a separate-origin player', async () => {
    const { container } = renderOverlay([placement({ radius: '12px' })]);
    await waitFor(() => expect(players(container)).toHaveLength(1));
    const [player] = players(container);
    expect(player.getAttribute('src')).toBe(SUNO);
    await waitFor(() => expect(player.getAttribute('title')).toMatch(/suno\.com$/));
    expect(player.getAttribute('loading')).toBe('lazy');
    expect(player.getAttribute('allow')).toBe('autoplay; encrypted-media; fullscreen; picture-in-picture');
    expect(player.getAttribute('referrerpolicy')).toBe('strict-origin-when-cross-origin');
    expect(player.getAttribute('sandbox')).toBe('allow-scripts allow-same-origin allow-popups allow-presentation');
    expect(player.style.left).toBe('12px');
    expect(player.style.top).toBe('40px');
    expect(player.style.width).toBe('320px');
    expect(player.style.height).toBe('152px');
    expect(player.style.borderRadius).toBe('12px');
    expect(player.style.visibility).toBe('visible');
    expect(player.style.clipPath).toBe('');
  });

  it('replaces content from a refused site with a Kronn warning that opens the settings for that site', async () => {
    const { container, onConfigureOrigin } = renderOverlay([
      placement({ key: 'evimeo:0', url: 'https://vimeo.com/123' }),
      placement({ key: 'ebad:0', url: 'javascript:alert(1)' }),
    ]);
    await waitFor(() => expect(warnings(container)).toHaveLength(1));
    expect(players(container)).toHaveLength(0);
    expect(container.querySelector('iframe[src*="vimeo"]')).toBeNull();
    const [warning] = warnings(container);
    expect(warning.dataset.embedOrigin).toBe('https://vimeo.com');
    expect(warning.textContent).toContain('https://vimeo.com');
    fireEvent.click(warning.querySelector('button')!);
    expect(onConfigureOrigin).toHaveBeenCalledWith('https://vimeo.com');
  });

  it('takes the content down when its site is revoked, and draws it once allowed again', async () => {
    vi.mocked(configApi.changeEmbedOrigins).mockResolvedValueOnce([]).mockResolvedValueOnce(['https://suno.com']);
    const { container } = renderOverlay([placement()]);
    await waitFor(() => expect(players(container)).toHaveLength(1));
    await act(async () => { await changeEmbedAllowedOrigins({ add: [], remove: ['https://suno.com'] }); });
    expect(players(container)).toHaveLength(0);
    expect(warnings(container)).toHaveLength(1);
    await act(async () => { await changeEmbedAllowedOrigins({ add: ['https://suno.com'], remove: [] }); });
    expect(players(container)).toHaveLength(1);
    expect(warnings(container)).toHaveLength(0);
  });

  it('cuts the content to what the Page’s scrolling container leaves visible', async () => {
    const { container, update } = renderOverlay([placement({
      rect: { left: 12, top: 30, width: 320, height: 152 },
      clip: { left: 12, top: 30, width: 320, height: 50 },
    })]);
    await waitFor(() => expect(players(container)).toHaveLength(1));
    const [player] = players(container);
    expect(player.style.clipPath).toBe('inset(0px 0px 102px 0px)');

    // Scrolled inside the container: same iframe, new cut.
    update([placement({
      rect: { left: 12, top: -20, width: 320, height: 152 },
      clip: { left: 12, top: 30, width: 320, height: 50 },
    })]);
    expect(players(container)[0]).toBe(player);
    expect(player.style.clipPath).toBe('inset(50px 0px 52px 0px)');

    // Scrolled out of the container: hidden, still not reloaded.
    update([placement({ visible: false, clip: { left: 12, top: 30, width: 320, height: 0 } })]);
    expect(players(container)[0]).toBe(player);
    expect(player.style.visibility).toBe('hidden');
    expect(player.getAttribute('tabindex')).toBe('-1');
  });

  it('keeps the same iframe while the Page scrolls, hides it out of view, and drops it when removed', async () => {
    vi.mocked(configApi.getEmbedOrigins).mockResolvedValue(['https://suno.com', 'https://www.youtube-nocookie.com']);
    const youtube = placement({ key: 'eyt:0', url: 'https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ' });
    const { container, update } = renderOverlay([placement(), youtube]);
    await waitFor(() => expect(players(container)).toHaveLength(2));
    const [suno, video] = players(container);

    update([placement({ rect: { left: 12, top: -60, width: 320, height: 152 } }), youtube]);
    expect(players(container)[0]).toBe(suno);
    expect(suno.style.top).toBe('-60px');

    // The Page reorders its blocks: neither player moves in the DOM.
    update([youtube, placement({ visible: false })]);
    expect(players(container)).toEqual([suno, video]);
    expect(suno.style.visibility).toBe('hidden');

    update([youtube]);
    expect(players(container)).toEqual([video]);
    expect(suno.isConnected).toBe(false);
  });

  it('clips to the Page frame and stays under the action card', () => {
    const layer = /\.live-page-embed-overlay\s*\{([^}]*)\}/.exec(CSS)?.[1] ?? '';
    expect(layer).toMatch(/overflow:\s*hidden/);
    expect(layer).toMatch(/pointer-events:\s*none/);
    const z = Number(/z-index:\s*(\d+)/.exec(layer)?.[1]);
    const actionZ = Number(/\.live-page-action-overlay\s*\{[^}]*z-index:\s*(\d+)/.exec(ACTION_CSS)?.[1]);
    expect(z).toBeLessThan(actionZ);
  });
});
