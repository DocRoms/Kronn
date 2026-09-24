import { describe, expect, it, vi } from 'vitest';
import {
  buildSandboxDocument,
  createLivePageOpenLinkRelay,
  LIVE_PAGE_CSP,
  liveActionBindingKey,
  postLivePageActionStates,
  requestRenderedPageHtml,
  runtimeData,
} from '../live-page-sandbox';
import type { LivePageDetail } from '../../types/generated';

const detail = {
  id: 'page-1', project_id: null, title: 'Adobe', slug: 'adobe',
  current_revision_id: 'rev-1', data_revision: 4,
  created_at: '2026-08-13T10:00:00Z', updated_at: '2026-08-13T10:00:00Z',
  last_published_at: '2026-08-13T10:00:00Z',
  pinned: false, archived: false,
  revision: { id: 'rev-1', page_id: 'page-1', revision: 2, html: '<h1>Adobe</h1>', created_by_agent: null, created_at: '2026-08-13T10:00:00Z' },
  datasets: [{
    id: 'data-1', page_id: 'page-1', name: 'latency', kind: 'time_series',
    current: null, schema: null, max_points: 100, max_age_days: 30,
    data_size_bytes: 9,
    updated_at: '2026-08-13T10:00:00Z',
    points: [{ id: 'pt-1', dataset_id: 'data-1', observed_at: '2026-08-13T10:00:00Z', payload: { ms: 87 }, workflow_run_id: null }],
  }],
} satisfies LivePageDetail;

describe('Live Page sandbox', () => {
  it('injects a network-denying CSP before authored head content', () => {
    const output = buildSandboxDocument('<html><head><script>window.authored=true</script></head></html>', 'channel-1');
    expect(output.indexOf('Content-Security-Policy')).toBeLessThan(output.indexOf('window.authored'));
    expect(output).toContain("connect-src 'none'");
    expect(output).toContain("object-src 'none'");
    expect(LIVE_PAGE_CSP).not.toContain('same-origin');
  });

  it('exposes a rendered-DOM export bridge and rasterizes canvas charts', () => {
    const output = buildSandboxDocument('<main>Report</main>', 'channel-1');
    expect(output).toContain("message.type!=='kronn:page-export-request'");
    expect(output).toContain("type:'kronn:page-export'");
    expect(output).toContain("source.toDataURL('image/png')");
    expect(output).toContain('viewport_width:width');
    expect(output).toContain('content_height:totalHeight');
    expect(output).toContain("root.querySelectorAll('script').forEach(script=>script.remove())");
    expect(output).toContain("type:'kronn:page-open-link'");
    expect(output).toContain("message.type==='kronn:page-link-port'");
    expect(output).toContain('stopImmediate.call(event)');
    expect(output).toContain('portPost.call(linkPort');
    expect(output).toContain("element.closest('a[href]')");
    expect(output).toContain("closest.call(element,'[data-kronn-action]')");
    expect(output).toContain("type:'kronn:page-action'");
    // La card s'ouvre sous la LIGNE : l'ancrage remonte au tr/li du bouton,
    // et se réémet au défilement puisque le host ne défile pas avec la page.
    expect(output).toContain("closest.call(element,'tr,li')");
    expect(output).toContain("type:'kronn:page-action-anchor'");
    expect(output).toContain("addEventListener('scroll',queueAnchor");
    // La card se loge dans une VRAIE ligne du DOM de la page, qui pousse les
    // suivantes — elle ne flotte plus au-dessus.
    expect(output).toContain("kronn:page-action-slot");
    expect(output).toContain("data-kronn-action-slot");
    expect(output).toContain("row.parentNode.insertBefore(slotEl,row.nextSibling)");
    // L'iframe est une origine opaque : elle n'herite pas du theme de l'hote,
    // et `prefers-color-scheme` repond pour l'OS. On le lui dit donc.
    expect(output).toContain("kronn:page-theme");
    expect(output).toContain("document.documentElement.setAttribute('data-theme',t)");
    expect(output).toContain("anchor.target.toLowerCase()!=='_blank'");
    expect(output).toContain('userActivation&&!userActivation.isActive');
    expect(output).toContain("Object.defineProperty(window,'open'");
    const script = output.match(/<script>([\s\S]*?)<\/script>/)?.[1];
    expect(script).toBeDefined();
    expect(() => new Function(script!)).not.toThrow();
  });

  it('opens only an HTTP(S) link received through the private port', async () => {
    const postMessage = vi.fn();
    const target = { postMessage } as unknown as Window;
    const openExternal = vi.fn();
    const relay = createLivePageOpenLinkRelay('channel-1', openExternal);
    relay.connect(target);
    const port = (postMessage.mock.calls[0][2] as MessagePort[])[0];
    port.postMessage({
      type: 'kronn:page-open-link',
      version: 1,
      channel_id: 'channel-1',
      url: 'https://example.com/report?period=7d',
    });

    await vi.waitFor(() => expect(openExternal).toHaveBeenCalledWith(
      'https://example.com/report?period=7d',
      '_blank',
      'noopener,noreferrer',
    ));
    relay.dispose();
  });

  it('rejects forged, active-scheme, credentialed and oversized link requests', async () => {
    const postMessage = vi.fn();
    const target = { postMessage } as unknown as Window;
    const openExternal = vi.fn();
    const relay = createLivePageOpenLinkRelay('channel-1', openExternal);
    relay.connect(target);
    const port = (postMessage.mock.calls[0][2] as MessagePort[])[0];
    const valid = {
      type: 'kronn:page-open-link',
      version: 1,
      channel_id: 'channel-1',
      url: 'https://example.com/report',
    };

    window.dispatchEvent(new MessageEvent('message', { source: window, data: valid }));
    port.postMessage({ ...valid, channel_id: 'forged' });
    port.postMessage({ ...valid, type: 'kronn:page-export' });
    port.postMessage({ ...valid, version: 2 });
    port.postMessage({ ...valid, url: 'javascript:alert(1)' });
    port.postMessage({ ...valid, url: 'data:text/html,boom' });
    port.postMessage({ ...valid, url: 'https://user:secret@example.com/' });
    port.postMessage({ ...valid, url: `https://example.com/${'x'.repeat(9 * 1024)}` });
    port.postMessage({ ...valid, url: '/relative' });

    await new Promise(resolve => setTimeout(resolve, 0));
    expect(openExternal).not.toHaveBeenCalled();
    relay.dispose();
    port.postMessage(valid);
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(openExternal).not.toHaveBeenCalled();
  });

  it('forwards a typed action intention through the private port without executing it', async () => {
    const postMessage = vi.fn();
    const onAction = vi.fn();
    const relay = createLivePageOpenLinkRelay('channel-1', vi.fn(), onAction);
    relay.connect({ postMessage } as unknown as Window);
    const port = (postMessage.mock.calls[0][2] as MessagePort[])[0];
    port.postMessage({
      type: 'kronn:page-action',
      version: 1,
      channel_id: 'channel-1',
      action_ref: 'frame-ticket',
      bindings: { ticket: 'KT-538', ignored: { forged: true } },
      anchor: { left: 12, top: 40, width: 100, height: 32 },
    });

    await vi.waitFor(() => expect(onAction).toHaveBeenCalledWith({
      actionRef: 'frame-ticket',
      bindings: { ticket: 'KT-538' },
      anchor: { left: 12, top: 40, width: 100, height: 32 },
    }));
    relay.dispose();
  });

  it('carries the host theme into the document before the Page paints', () => {
    const out = buildSandboxDocument('<html><head></head><body>x</body></html>', 'channel-1', 'light');
    // Avant le markup de la Page : sinon elle peint une frame dans le mauvais theme.
    expect(out.indexOf("setAttribute('data-theme',\"light\")")).toBeLessThan(out.indexOf('<body'));
    expect(out).toContain("kronn:page-theme");
  });

  it('refuses a theme value that is not a plain identifier', () => {
    const out = buildSandboxDocument('<html><head></head><body>x</body></html>', 'channel-1',
      '"></script><script>alert(1)</script>');
    expect(out).not.toContain('alert(1)');
    expect(out).not.toContain(`setAttribute('data-theme',"`);
  });

  it('omits the theme entirely when the host has none yet', () => {
    const out = buildSandboxDocument('<html><head></head><body>x</body></html>', 'channel-1', null);
    expect(out).not.toContain(`setAttribute('data-theme',"`);
  });

  it('moves an open card without user activation, and cannot launch anything', async () => {
    const postMessage = vi.fn();
    const onAction = vi.fn();
    const onAnchor = vi.fn();
    const relay = createLivePageOpenLinkRelay('channel-1', vi.fn(), onAction, onAnchor);
    relay.connect({ postMessage } as unknown as Window);
    const port = (postMessage.mock.calls[0][2] as MessagePort[])[0];
    // A scroll carries no activation. The card must still follow its row.
    Object.defineProperty(navigator, 'userActivation', {
      configurable: true,
      value: { isActive: false, hasBeenActive: true },
    });
    port.postMessage({
      type: 'kronn:page-action-anchor',
      version: 1,
      channel_id: 'channel-1',
      anchor: { left: 12, top: 400, width: 880, height: 28 },
      action_ref: 'frame-ticket',
    });

    await vi.waitFor(() => expect(onAnchor).toHaveBeenCalledWith({
      left: 12, top: 400, width: 880, height: 28, slot: false,
    }));
    expect(onAction).not.toHaveBeenCalled();
    relay.dispose();
  });

  it('paints the Page in the host palette before its own markup parses', () => {
    const out = buildSandboxDocument('<html><head></head><body>x</body></html>', 'channel-1', 'dark',
      { 'bg-surface': '#1f1140', 'border-medium': 'rgba(255, 255, 255, 0.1)' });
    expect(out.indexOf('--kr-bg-surface:#1f1140')).toBeGreaterThan(-1);
    expect(out.indexOf('--kr-bg-surface:#1f1140')).toBeLessThan(out.indexOf('<body'));
    expect(out).toContain('--kr-border-medium:rgba(255, 255, 255, 0.1)');
  });

  it('drops anything that is not a plain colour value, and any unknown token', () => {
    const out = buildSandboxDocument('<html><head></head><body>x</body></html>', 'channel-1', 'dark', {
      'bg-surface': 'red;}</style><script>alert(1)</script>',
      'bg-base': 'url(https://evil.example/x.png)',
      'text-primary': '#e8eaed',
      'not-a-token': '#000',
    });
    expect(out).not.toContain('alert(1)');
    expect(out).not.toContain('evil.example');
    expect(out).not.toContain('--kr-not-a-token');
    expect(out).toContain('--kr-text-primary:#e8eaed');
  });

  it('applies the palette the host sends with a theme change, and only safe values', async () => {
    const { Window } = await import('happy-dom');
    const frame = new Window();
    frame.document.write(buildSandboxDocument('<html><head></head><body>x</body></html>', 'channel-1'));
    for (const script of Array.from(frame.document.querySelectorAll('script:not([type])'))) {
      (frame as unknown as { eval: (code: string) => void }).eval(script.textContent ?? '');
    }
    frame.dispatchEvent(new frame.MessageEvent('message', { data: {
      type: 'kronn:page-theme', version: 1, channel_id: 'channel-1', theme: 'dark',
      tokens: { 'bg-surface': '#1f1140', 'bg-base': 'url(x)', 'not-a-token': '#000' },
    } }));
    const root = frame.document.documentElement;
    expect(root.getAttribute('data-theme')).toBe('dark');
    expect(root.style.getPropertyValue('--kr-bg-surface')).toBe('#1f1140');
    expect(root.style.getPropertyValue('--kr-bg-base')).toBe('');
    expect(root.style.getPropertyValue('--kr-not-a-token')).toBe('');
    await frame.happyDOM.close();
  });

  it('opens the collapse over exactly the row\'s columns', async () => {
    // A colspan larger than the row adds phantom columns: a table-layout:fixed table then
    // shares its free width with them and its auto column collapses to a few pixels.
    const { Window } = await import('happy-dom');
    const frame = new Window();
    const page = '<html><head></head><body><table style="table-layout:fixed"><tbody>'
      + '<tr><td>a</td><td>b</td><td colspan="2">c</td><td>d</td><td>e</td>'
      + '<td><button data-kronn-action="autocode-ticket" data-kronn-bindings=\'{"ticketKey":"EW-1"}\'>x</button></td></tr>'
      + '<tr><td colspan="7">next</td></tr></tbody></table></body></html>';
    frame.document.write(buildSandboxDocument(page, 'channel-1'));
    // `document.write` builds the DOM without running it; run the injected scripts ourselves.
    for (const script of Array.from(frame.document.querySelectorAll('script:not([type])'))) {
      (frame as unknown as { eval: (code: string) => void }).eval(script.textContent ?? '');
    }
    frame.dispatchEvent(new frame.MessageEvent('message', { data: {
      type: 'kronn:page-action-slot', version: 1, channel_id: 'channel-1',
      slot: { action_ref: 'autocode-ticket', binding_key: liveActionBindingKey({ ticketKey: 'EW-1' }), height: 120 },
    } }));
    const cell = frame.document.querySelector('[data-kronn-action-slot] > td') as unknown as HTMLTableCellElement | null;
    expect(cell).not.toBeNull();
    expect(cell!.colSpan).toBe(7);
    expect(cell!.style.height).toBe('120px');
    await frame.happyDOM.close();
  });

  it('opens the collapse inside the block the Page names, right under its CTA', async () => {
    const { Window } = await import('happy-dom');
    const frame = new Window();
    const page = '<html><head></head><body><table><tbody><tr><td>'
      + '<div class="step" data-kronn-action-slot-host><p>x</p>'
      + '<button data-kronn-action="autocode-implem" data-kronn-bindings=\'{"ticketKey":"EW-1"}\'>go</button></div>'
      + '<p class="after">description</p></td></tr></tbody></table></body></html>';
    frame.document.write(buildSandboxDocument(page, 'channel-1'));
    for (const script of Array.from(frame.document.querySelectorAll('script:not([type])'))) {
      (frame as unknown as { eval: (code: string) => void }).eval(script.textContent ?? '');
    }
    frame.dispatchEvent(new frame.MessageEvent('message', { data: {
      type: 'kronn:page-action-slot', version: 1, channel_id: 'channel-1',
      slot: { action_ref: 'autocode-implem', binding_key: liveActionBindingKey({ ticketKey: 'EW-1' }), height: 90 },
    } }));
    const slot = frame.document.querySelector('[data-kronn-action-slot]') as unknown as HTMLElement | null;
    expect(slot?.tagName).toBe('DIV');
    expect(slot?.parentElement?.classList.contains('step')).toBe(true);
    expect(slot?.style.height).toBe('90px');
    // Not after the table row: the description stays below the card.
    expect(frame.document.querySelectorAll('tr').length).toBe(1);
    await frame.happyDOM.close();
  });

  it('reports an anchor that is the collapse the Page opened, not its row', async () => {
    const postMessage = vi.fn();
    const onAnchor = vi.fn();
    const relay = createLivePageOpenLinkRelay('channel-1', vi.fn(), vi.fn(), onAnchor);
    relay.connect({ postMessage } as unknown as Window);
    const port = (postMessage.mock.calls[0][2] as MessagePort[])[0];
    port.postMessage({
      type: 'kronn:page-action-anchor',
      version: 1,
      channel_id: 'channel-1',
      anchor: { left: 12, top: 400, width: 880, height: 186, slot: true },
    });
    await vi.waitFor(() => expect(onAnchor).toHaveBeenCalledWith({
      left: 12, top: 400, width: 880, height: 186, slot: true,
    }));
    relay.dispose();
  });

  it('ignores an anchor whose rectangle is not finite', async () => {
    const postMessage = vi.fn();
    const onAnchor = vi.fn();
    const relay = createLivePageOpenLinkRelay('channel-1', vi.fn(), vi.fn(), onAnchor);
    relay.connect({ postMessage } as unknown as Window);
    const port = (postMessage.mock.calls[0][2] as MessagePort[])[0];
    port.postMessage({
      type: 'kronn:page-action-anchor',
      version: 1,
      channel_id: 'channel-1',
      anchor: { left: 12, top: Number.NaN, width: 880, height: 28 },
    });
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(onAnchor).not.toHaveBeenCalled();
    relay.dispose();
  });

  it('rejects action references that cannot form a stable URL-safe server id', async () => {
    const postMessage = vi.fn();
    const onAction = vi.fn();
    const relay = createLivePageOpenLinkRelay('channel-1', vi.fn(), onAction);
    relay.connect({ postMessage } as unknown as Window);
    const port = (postMessage.mock.calls[0][2] as MessagePort[])[0];
    const request = {
      type: 'kronn:page-action',
      version: 1,
      channel_id: 'channel-1',
      bindings: {},
      anchor: { left: 12, top: 40, width: 100, height: 32 },
    };
    port.postMessage({ ...request, action_ref: '../route' });
    port.postMessage({ ...request, action_ref: 'contains space' });
    port.postMessage({ ...request, action_ref: 'x'.repeat(257) });

    await new Promise(resolve => setTimeout(resolve, 0));
    expect(onAction).not.toHaveBeenCalled();
    relay.dispose();
  });

  it('rejects a valid private-port request without active user activation', async () => {
    const activationDescriptor = Object.getOwnPropertyDescriptor(navigator, 'userActivation');
    Object.defineProperty(navigator, 'userActivation', {
      configurable: true,
      value: { isActive: false },
    });
    const postMessage = vi.fn();
    const openExternal = vi.fn();
    const relay = createLivePageOpenLinkRelay(
      'channel-1',
      openExternal,
    );
    try {
      relay.connect({ postMessage } as unknown as Window);
      const port = (postMessage.mock.calls[0][2] as MessagePort[])[0];
      port.postMessage({
        type: 'kronn:page-open-link',
        version: 1,
        channel_id: 'channel-1',
        url: 'https://example.com/background',
      });

      await new Promise(resolve => setTimeout(resolve, 0));
      expect(openExternal).not.toHaveBeenCalled();
    } finally {
      relay.dispose();
      if (activationDescriptor) {
        Object.defineProperty(navigator, 'userActivation', activationDescriptor);
      } else {
        Reflect.deleteProperty(navigator, 'userActivation');
      }
    }
  });

  it('accepts only the matching rendered document from the opaque frame', async () => {
    const frame = document.createElement('iframe');
    document.body.append(frame);
    const target = frame.contentWindow;
    expect(target).not.toBeNull();
    const postMessage = vi.spyOn(target!, 'postMessage').mockImplementation(() => undefined);

    const capture = vi.fn().mockResolvedValue(['data:image/png;base64,cGFnZQ==']);
    const result = requestRenderedPageHtml(frame, 'channel-1', 500, capture);
    const request = postMessage.mock.calls[0][0] as Record<string, unknown>;
    window.dispatchEvent(new MessageEvent('message', {
      source: target,
      data: { ...request, type: 'kronn:page-export', channel_id: 'forged', html: '<p>wrong</p>' },
    }));
    window.dispatchEvent(new MessageEvent('message', {
      source: target,
      data: {
        ...request,
        type: 'kronn:page-export',
        html: '<html><body><p>Rendered 1240</p></body></html>',
        viewport_width: 1000,
        content_height: 760,
      },
    }));

    await expect(result).resolves.toEqual({
      html: '<html><body><p>Rendered 1240</p></body></html>',
      pageImages: ['data:image/png;base64,cGFnZQ=='],
    });
    expect(capture).toHaveBeenCalledWith(
      '<html><body><p>Rendered 1240</p></body></html>',
      1000,
      760,
    );
    frame.remove();
  });

  it('spells a row exactly as the backend stores a launch binding', () => {
    // Sorted `name=selector` pairs joined by U+001F, whatever the key order.
    expect(liveActionBindingKey({ b: '2', a: '1' })).toBe(['a=1', 'b=2'].join(String.fromCharCode(0x1f)));
    expect(liveActionBindingKey({})).toBe('');
  });

  it('marks each button with its own row state, including rows rendered later', async () => {
    // Runs the real bridge against this document. It pins `KronnPageData` and
    // `window.open` for good, as it must in a Page; `window` is its only use of
    // that name, so a throwaway one keeps the pins off the test environment.
    const script = buildSandboxDocument('<main></main>', 'chan-states').match(/<script>([\s\S]*?)<\/script>/)?.[1];
    new Function('window', script!)({});
    document.body.innerHTML = `
      <button data-kronn-action="frame" data-kronn-bindings='{"ticket":"EW-1"}'>EW-1</button>
      <button data-kronn-action="frame" data-kronn-bindings='{"ticket":"EW-2"}'>EW-2</button>
      <button data-kronn-action="frame" data-kronn-bindings='{"ticket":"EW-3"}'>EW-3</button>`;
    const post = (channel: string, states: unknown[]) => window.dispatchEvent(new MessageEvent('message', {
      data: { type: 'kronn:page-action-states', version: 1, channel_id: channel, states },
    }));
    const button = (ticket: string) => document.querySelector(`[data-kronn-bindings*="${ticket}"]`)!;

    post('chan-states', [
      { action_ref: 'frame', binding_key: 'ticket=EW-1', state: 'running' },
      { action_ref: 'frame', binding_key: 'ticket=EW-2', state: 'succeeded' },
    ]);
    expect(button('EW-1').getAttribute('data-kronn-action-state')).toBe('running');
    expect(button('EW-1').getAttribute('aria-busy')).toBe('true');
    expect(button('EW-2').getAttribute('data-kronn-action-state')).toBe('succeeded');
    expect(button('EW-2').hasAttribute('aria-busy')).toBe(false);
    expect(button('EW-3').hasAttribute('data-kronn-action-state')).toBe(false);

    // Pages draw their rows from data, often after the states arrived: a row
    // that appears later is marked without any new message.
    post('chan-states', [{ action_ref: 'frame', binding_key: 'ticket=EW-9', state: 'failed' }]);
    expect(button('EW-1').hasAttribute('data-kronn-action-state')).toBe(false);
    document.body.insertAdjacentHTML('beforeend',
      `<button data-kronn-action="frame" data-kronn-bindings='{"ticket":"EW-9"}'>EW-9</button>`);
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(button('EW-9').getAttribute('data-kronn-action-state')).toBe('failed');

    // Another channel cannot mark this Page's buttons.
    post('someone-else', [{ action_ref: 'frame', binding_key: 'ticket=EW-3', state: 'succeeded' }]);
    expect(button('EW-3').hasAttribute('data-kronn-action-state')).toBe(false);
  });

  it('posts the launch states the iframe expects', () => {
    const target = { postMessage: vi.fn() } as unknown as Window;
    postLivePageActionStates(target, 'chan-1', [
      { action_ref: 'frame', binding_key: 'ticket=EW-1', state: 'running' },
      { action_ref: 'refresh', binding_key: null, state: 'succeeded' },
    ]);
    expect(target.postMessage).toHaveBeenCalledWith({
      type: 'kronn:page-action-states', version: 1, channel_id: 'chan-1',
      states: [
        { action_ref: 'frame', binding_key: 'ticket=EW-1', state: 'running' },
        { action_ref: 'refresh', binding_key: '', state: 'succeeded' },
      ],
    }, '*');
  });

  it('exposes time-series points without JSON stringification', () => {
    const data = runtimeData(detail);
    expect(data.datasets.latency.points[0].value).toEqual({ ms: 87 });
    expect(data.page.data_revision).toBe(4);
  });
});
