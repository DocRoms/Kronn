import { describe, expect, it, vi } from 'vitest';
import {
  buildSandboxDocument,
  createLivePageOpenLinkRelay,
  LIVE_PAGE_CSP,
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

  it('exposes time-series points without JSON stringification', () => {
    const data = runtimeData(detail);
    expect(data.datasets.latency.points[0].value).toEqual({ ms: 87 });
    expect(data.page.data_revision).toBe(4);
  });
});
