import { describe, expect, it, vi } from 'vitest';
import {
  MAX_LIVE_PAGE_EMBEDS,
  MAX_LIVE_PAGE_EMBED_REPORTS,
  embedUrlOrigin,
  normalizeEmbedOrigin,
  planLivePageEmbeds,
} from '../live-page-embeds';
import {
  buildSandboxDocument,
  createLivePageOpenLinkRelay,
  parseLivePageEmbeds,
  type LivePageEmbedPlacement,
} from '../live-page-sandbox';

const SUNO = 'https://suno.com/embed/08fca036-317a-4cd5-9860-166c62c0180f';

function placement(over: Partial<LivePageEmbedPlacement> = {}): LivePageEmbedPlacement {
  return { key: 'eabc:0', url: SUNO, rect: { left: 0, top: 0, width: 320, height: 152 }, visible: true, ...over };
}

describe('embed origins', () => {
  it('reduces an embed URL to the origin the allowed sites compare', () => {
    expect(embedUrlOrigin(SUNO)).toBe('https://suno.com');
    expect(embedUrlOrigin('https://Suno.com:443/embed/x?a=1#t')).toBe('https://suno.com');
    expect(embedUrlOrigin('https://player.example.com:8443/v')).toBe('https://player.example.com:8443');
    expect(embedUrlOrigin('http://localhost:3000/x')).toBe('http://localhost:3000');
    expect(embedUrlOrigin('https://bücher.example/x')).toBe('https://xn--bcher-kva.example');
  });

  it('refuses what Kronn would never draw', () => {
    for (const raw of [
      '', '  ', 'javascript:alert(1)', 'data:text/html,hi', 'file:///etc/passwd', 'ftp://example.com/x',
      'https://user:pass@example.com/x', 'https://user@example.com/x', '/embed/x', '//example.com/x',
      `https://example.com/${'a'.repeat(3000)}`, 42, null,
    ]) expect(embedUrlOrigin(raw)).toBeNull();
  });

  it('normalizes a typed origin and refuses paths, queries and fragments', () => {
    expect(normalizeEmbedOrigin(' https://Player.Example.com/ ')).toBe('https://player.example.com');
    for (const raw of ['player.example.com', 'https://player.example.com/embed', 'https://player.example.com/?a=1',
      'https://player.example.com/#x', 'https://player.example.com?', 'https://u:p@player.example.com']) {
      expect(normalizeEmbedOrigin(raw)).toBeNull();
    }
  });
});

describe('planLivePageEmbeds', () => {
  const allowed = new Set(['https://suno.com']);

  it('draws nothing before the allowed sites are known', () => {
    expect(planLivePageEmbeds([placement()], null)).toEqual({ players: [], blocked: [] });
  });

  it('splits allowed content from refused sites and drops malformed URLs', () => {
    const plan = planLivePageEmbeds([
      placement(),
      placement({ key: 'eaaa:0', url: 'https://vimeo.com/123' }),
      placement({ key: 'ebbb:0', url: 'javascript:alert(1)' }),
      placement({ key: 'eabc:0', url: 'https://vimeo.com/dup' }), // duplicate key
    ], allowed);
    expect(plan.players.map(p => [p.placement.key, p.origin, p.url])).toEqual([['eabc:0', 'https://suno.com', SUNO]]);
    expect(plan.blocked.map(p => [p.placement.key, p.origin])).toEqual([['eaaa:0', 'https://vimeo.com']]);
  });

  it('applies the players quota after the check, so refused placeholders never crowd out an allowed one', () => {
    const refused = Array.from({ length: MAX_LIVE_PAGE_EMBEDS }, (_, i) => placement({ key: `ev${i}:0`, url: `https://vimeo.com/${i}` }));
    const plan = planLivePageEmbeds([...refused, placement({ key: 'eok:0' })], allowed);
    expect(plan.players.map(p => p.placement.key)).toEqual(['eok:0']);
    expect(plan.blocked).toHaveLength(MAX_LIVE_PAGE_EMBEDS);
  });

  it('still caps the allowed players', () => {
    const many = Array.from({ length: 12 }, (_, i) => placement({ key: `es:${i}` }));
    expect(planLivePageEmbeds(many, allowed).players).toHaveLength(MAX_LIVE_PAGE_EMBEDS);
  });

  it('draws a third service as soon as its site is allowed, with no code change', () => {
    const other = placement({ key: 'eo:0', url: 'https://player.example.org/v/42' });
    expect(planLivePageEmbeds([other], allowed).players).toHaveLength(0);
    const plan = planLivePageEmbeds([other], new Set([...allowed, 'https://player.example.org']));
    expect(plan.players[0]).toMatchObject({ url: 'https://player.example.org/v/42', origin: 'https://player.example.org' });
  });
});

describe('parseLivePageEmbeds', () => {
  const entry = (over: Record<string, unknown> = {}) => ({
    key: 'e1a2b:0', url: SUNO, rect: { left: 10, top: 20, width: 300, height: 120 }, visible: true, ...over,
  });

  it('keeps well-formed entries and drops the rest', () => {
    const parsed = parseLivePageEmbeds([
      entry({ radius: '12px' }),
      entry({ key: 'bad key' }),
      entry({ key: 'e1a2b:0' }), // duplicate key
      entry({ key: 'ex:1', rect: { left: Number.NaN, top: 0, width: 1, height: 1 } }),
      entry({ key: 'ex:2', rect: { left: 0, top: 0, width: -1, height: 1 } }),
      entry({ key: 'ex:3', rect: { left: 0, top: 0, width: 50_000, height: 1 } }),
      entry({ key: 'ex:4', visible: 'yes', radius: 'url(x)' }),
      entry({ key: 'ex:5', url: 42 }),
      entry({ key: 'ex:6', url: `https://x.example/${'a'.repeat(3000)}` }),
      null,
      'nope',
    ]);
    expect(parsed).toEqual([
      { ...entry(), radius: '12px' },
      { ...entry({ key: 'ex:4' }), visible: false },
    ]);
  });

  it('keeps a clip, and turns a malformed one into an empty clip rather than none', () => {
    const clip = { left: 10, top: 20, width: 300, height: 30 };
    expect(parseLivePageEmbeds([entry({ clip })])?.[0].clip).toEqual(clip);
    expect(parseLivePageEmbeds([entry({ clip: { left: 'x' } })])?.[0].clip).toEqual({ left: 10, top: 20, width: 0, height: 0 });
    expect(parseLivePageEmbeds([entry()])?.[0]).not.toHaveProperty('clip');
  });

  it('bounds the report, not the players, and rejects a non-array', () => {
    const many = Array.from({ length: 100 }, (_, index) => entry({ key: `e1:${index}` }));
    expect(parseLivePageEmbeds(many)).toHaveLength(MAX_LIVE_PAGE_EMBED_REPORTS);
    expect(MAX_LIVE_PAGE_EMBED_REPORTS).toBeGreaterThan(MAX_LIVE_PAGE_EMBEDS);
    expect(parseLivePageEmbeds({ length: 1 })).toBeNull();
  });

  it('is relayed without user activation, on this channel only', async () => {
    const postMessage = vi.fn();
    const onEmbeds = vi.fn();
    const onAction = vi.fn();
    const relay = createLivePageOpenLinkRelay('channel-1', { openExternal: vi.fn(), onAction, onEmbeds });
    relay.connect({ postMessage } as unknown as Window);
    const port = (postMessage.mock.calls[0][2] as MessagePort[])[0];
    Object.defineProperty(navigator, 'userActivation', { configurable: true, value: { isActive: false, hasBeenActive: true } });
    port.postMessage({ type: 'kronn:page-embeds', version: 1, channel_id: 'other', embeds: [entry()] });
    port.postMessage({ type: 'kronn:page-embeds', version: 1, channel_id: 'channel-1', embeds: 'x' });
    port.postMessage({ type: 'kronn:page-embeds', version: 1, channel_id: 'channel-1', embeds: [entry()] });
    await vi.waitFor(() => expect(onEmbeds).toHaveBeenCalledWith([entry()]));
    expect(onEmbeds).toHaveBeenCalledTimes(1);
    expect(onAction).not.toHaveBeenCalled();
    relay.dispose();
  });
});

describe('embed bridge', () => {
  interface Box { left: number; top: number; width: number; height: number }
  async function runFrame(page: string, boxes: Record<string, Box> = {}) {
    const { Window } = await import('happy-dom');
    const frame = new Window({ width: 1024, height: 768 });
    frame.document.write(buildSandboxDocument(page, 'channel-1'));
    const received: unknown[] = [];
    const natives = frame as unknown as {
      Element: { prototype: { getBoundingClientRect: () => unknown } };
      MessagePort: { prototype: { postMessage: (m: unknown) => void; start: () => void } };
    };
    // A placeholder sits at its `data-top`; an element with an id may get its own box.
    natives.Element.prototype.getBoundingClientRect = function (this: Element) {
      const own = this.id ? boxes[this.id] : undefined;
      const top = Number(this.getAttribute?.('data-top') ?? 0);
      const box = own ?? { left: 16, top, width: 320, height: 152 };
      return { ...box, right: box.left + box.width, bottom: box.top + box.height };
    };
    // No layout in happy-dom: an element with its own box has that inner size.
    for (const dimension of ['clientWidth', 'clientHeight'] as const) {
      Object.defineProperty((frame as unknown as { HTMLElement: { prototype: object } }).HTMLElement.prototype, dimension, {
        configurable: true,
        get(this: Element) {
          const own = this.id ? boxes[this.id] : undefined;
          return own ? (dimension === 'clientWidth' ? own.width : own.height) : 0;
        },
      });
    }
    natives.MessagePort.prototype.postMessage = (m: unknown) => { received.push(m); };
    natives.MessagePort.prototype.start = () => {};
    for (const script of Array.from(frame.document.querySelectorAll('script:not([type])'))) {
      (frame as unknown as { eval: (code: string) => void }).eval(script.textContent ?? '');
    }
    frame.dispatchEvent(new frame.MessageEvent('message', {
      data: { type: 'kronn:page-link-port', version: 1, channel_id: 'channel-1' },
      ports: [{} as never],
    }));
    const settle = () => new Promise(resolve => setTimeout(resolve, 60));
    await settle();
    const embeds = () => received
      .filter(m => (m as { type?: string }).type === 'kronn:page-embeds')
      .map(m => (m as { embeds: Array<Record<string, unknown>> }).embeds);
    return { frame, embeds, settle };
  }

  it('reports each http(s) placeholder with its URL and a stable key, and skips the rest', async () => {
    const { frame, embeds } = await runFrame('<html><head></head><body>'
      + `<div data-kronn-embed="${SUNO}" data-top="40"></div>`
      + `<div data-kronn-embed="${SUNO}" data-top="900"></div>`
      + '<div data-kronn-embed="javascript:alert(1)"></div>'
      + '<div data-kronn-embed="suno"></div>'
      + '</body></html>');
    const [report] = embeds();
    expect(embeds()).toHaveLength(1);
    expect(report).toHaveLength(2);
    const [first, second] = report;
    expect(first).toMatchObject({ url: SUNO, rect: { left: 16, top: 40, width: 320, height: 152 }, visible: true });
    // Below the frame's viewport: reported, but not visible.
    expect(second).toMatchObject({ url: SUNO, rect: { left: 16, top: 900, width: 320, height: 152 }, visible: false });
    expect(first.key).toMatch(/^e[0-9a-z]+:0$/);
    expect(second.key).toBe(String(first.key).replace(/:0$/, ':1'));
    expect(parseLivePageEmbeds(report)).toHaveLength(2);
    expect(first).not.toHaveProperty('clip');
    await frame.happyDOM.close();
  });

  it('reports what a clipping container leaves visible, and hides a placeholder scrolled out of it', async () => {
    const page = '<html><head></head><body>'
      + '<div id="box" style="height:50px;overflow:hidden">'
      + `<div id="player" data-kronn-embed="${SUNO}"></div>`
      + '</div></body></html>';
    const boxes = {
      box: { left: 0, top: 30, width: 400, height: 50 },
      player: { left: 16, top: 30, width: 320, height: 152 },
    };
    const { frame, embeds, settle } = await runFrame(page, boxes);
    expect(embeds()[0][0]).toMatchObject({
      rect: { left: 16, top: 30, width: 320, height: 152 },
      clip: { left: 16, top: 30, width: 320, height: 50 },
      visible: true,
    });
    // The container scrolls its content away: the placeholder is still in the
    // frame's viewport, but nothing of it can be seen any more. (A real
    // container's scroll reaches the bridge's capturing listener; happy-dom does
    // not propagate it, so the frame is told directly.)
    boxes.player = { left: 16, top: 100, width: 320, height: 152 };
    frame.dispatchEvent(new frame.Event('scroll'));
    await settle();
    expect(embeds().at(-1)![0]).toMatchObject({ clip: { height: 0 }, visible: false });
    await frame.happyDOM.close();
  });

  it('follows placeholders the Page adds, moves and removes, only when something changed', async () => {
    const { frame, embeds, settle } = await runFrame('<html><head></head><body><main id="root"></main></body></html>');
    expect(embeds()).toEqual([[]]);
    const root = frame.document.querySelector('#root')!;
    root.innerHTML = '<div id="a" data-kronn-embed="https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ" data-top="10"></div>';
    await settle();
    expect(embeds()).toHaveLength(2);
    // A scroll that moves nothing is not re-sent.
    frame.dispatchEvent(new frame.Event('scroll'));
    await settle();
    expect(embeds()).toHaveLength(2);
    frame.document.querySelector('#a')!.setAttribute('data-top', '200');
    frame.dispatchEvent(new frame.Event('scroll'));
    await settle();
    expect(embeds()).toHaveLength(3);
    expect((embeds()[2][0].rect as { top: number }).top).toBe(200);
    // A URL changed by script is a new placeholder the host checks again.
    frame.document.querySelector('#a')!.setAttribute('data-kronn-embed', 'https://player.example.org/v/1');
    await settle();
    expect(embeds().at(-1)![0].url).toBe('https://player.example.org/v/1');
    root.innerHTML = '';
    await settle();
    expect(embeds().at(-1)).toEqual([]);
    await frame.happyDOM.close();
  });

  it('reports refused placeholders past the players quota, so an allowed one after them still shows', async () => {
    const refused = Array.from({ length: MAX_LIVE_PAGE_EMBEDS }, (_, i) => `<div data-kronn-embed="https://vimeo.com/${i}"></div>`).join('');
    const { frame, embeds } = await runFrame(`<html><head></head><body>${refused}<div data-kronn-embed="${SUNO}"></div></body></html>`);
    const report = parseLivePageEmbeds(embeds()[0])!;
    expect(report).toHaveLength(MAX_LIVE_PAGE_EMBEDS + 1);
    const plan = planLivePageEmbeds(report, new Set(['https://suno.com']));
    expect(plan.players.map(p => p.url)).toEqual([SUNO]);
    await frame.happyDOM.close();
  });

  it('caps the report itself', async () => {
    const many = Array.from({ length: MAX_LIVE_PAGE_EMBED_REPORTS + 6 }, (_, i) => `<div data-kronn-embed="https://vimeo.com/${i}"></div>`).join('');
    const { frame, embeds } = await runFrame(`<html><head></head><body>${many}</body></html>`);
    expect(embeds()[0]).toHaveLength(MAX_LIVE_PAGE_EMBED_REPORTS);
    await frame.happyDOM.close();
  });
});
