import type { IncomingMessage, ServerResponse } from 'node:http';
import { describe, expect, it, vi } from 'vitest';
import type { UserConfig } from 'vite';
import viteConfig from './vite.config';
import { documentFramePolicy, FRAME_SRC_PATH, framePolicyMiddleware, injectServedFrameSources } from './vite-frame-policy.ts';

const SELF_ONLY = "frame-src 'self'; child-src 'self'; worker-src 'self' blob:";

function request(headers: Record<string, string>, url = '/', method = 'GET'): IncomingMessage {
  return { method, url, headers: { ...headers } } as unknown as IncomingMessage;
}

function response() {
  const headers = new Map<string, string>();
  const sent: unknown[] = [];
  const res = {
    setHeader: (name: string, value: string) => headers.set(name.toLowerCase(), value),
    getHeader: (name: string) => headers.get(name.toLowerCase()),
    removeHeader: (name: string) => headers.delete(name.toLowerCase()),
    end: (chunk?: unknown) => { sent.push(chunk); },
  };
  return { headers, sent, res: res as unknown as ServerResponse };
}

function backend(status: number, sources?: string) {
  return vi.fn(async () => new Response(null, { status, headers: sources ? { 'x-kronn-frame-src': sources } : {} }));
}

describe('native document frame policy', () => {
  it('matches the backend policy and lists exactly the sources it returned', () => {
    expect(documentFramePolicy("'self' https://suno.com https://player.example:8443")).toBe(
      "frame-src 'self' https://suno.com https://player.example:8443; child-src 'self' https://suno.com https://player.example:8443; worker-src 'self' blob:",
    );
  });

  it('never widens: anything but plain host sources falls back to self', () => {
    for (const hostile of [
      null, '', '*', "'self' *", "'self' https://*.example.com", "'self' https:", "https://suno.com",
      "'self' https://a.example; script-src *", "'self' 'unsafe-inline'", "'self' data:", "'self' blob:",
      "'self'  https://a.example", "'self' https://A.example", `'self' https://${'a'.repeat(17_000)}.example`,
    ]) {
      expect(documentFramePolicy(hostile), String(hostile)).toBe(SELF_ONLY);
    }
  });

  it('puts the policy read from the backend on documents and drops validators', async () => {
    const fetchImpl = backend(204, "'self' https://suno.com");
    const req = request({ 'sec-fetch-dest': 'document', 'if-none-match': 'W/"x"', 'if-modified-since': 'x' });
    const { headers, res } = response();
    const next = vi.fn();
    await framePolicyMiddleware('http://127.0.0.1:3140', fetchImpl)(req, res, next);
    expect(String(fetchImpl.mock.calls[0]?.[0])).toBe(`http://127.0.0.1:3140${FRAME_SRC_PATH}`);
    expect(headers.get('content-security-policy')).toBe(
      "frame-src 'self' https://suno.com; child-src 'self' https://suno.com; worker-src 'self' blob:",
    );
    expect(req.headers['if-none-match']).toBeUndefined();
    expect(req.headers['if-modified-since']).toBeUndefined();
    expect(next).toHaveBeenCalledOnce();
  });

  it('renders the app document with a marker equal to its CSP header, escaped', async () => {
    const render = async () => '<!DOCTYPE html>\n<html>\n  <head>\n<title>k</title></head></html>';
    const { headers, sent, res } = response();
    await framePolicyMiddleware('http://127.0.0.1:3140', backend(204, "'self' https://suno.com"), render)(request({ accept: 'text/html' }, '/settings'), res, vi.fn());
    const html = String(sent[0]);
    const marker = /<meta name="kronn-served-frame-src" content="([^"]*)">/.exec(html)?.[1];
    expect(html.indexOf('<meta')).toBe(html.indexOf('<head>') + '<head>'.length);
    const decoded = marker!.replace(/&#39;/g, "'");
    expect(headers.get('content-security-policy')).toBe(documentFramePolicy(decoded));
    expect(decoded).toBe("'self' https://suno.com");
    // A malformed backend value is never echoed: the marker says what the CSP says.
    const down = response();
    await framePolicyMiddleware('http://127.0.0.1:3140', backend(204, '"><script>x</script>'), async () => '<head>')(request({ accept: 'text/html' }), down.res, vi.fn());
    expect(String(down.sent[0])).toBe('<head><meta name="kronn-served-frame-src" content="&#39;self&#39;">');
    expect(injectServedFrameSources('<head>', '"<&')).toBe('<head><meta name="kronn-served-frame-src" content="&quot;&lt;&amp;">');
    expect(injectServedFrameSources('<header>x</header>', "'self'")).toBe('<header>x</header>');
    // Another HTML file keeps the header and gets no marker.
    const other = response();
    const next = vi.fn();
    await framePolicyMiddleware('http://127.0.0.1:3140', backend(204, "'self'"), render)(request({ accept: 'text/html' }, '/docs/page.html'), other.res, next);
    expect(next).toHaveBeenCalledOnce();
    expect(other.sent).toEqual([]);
    expect(other.headers.get('content-security-policy')).toBe(SELF_ONLY);
  });

  it('frames only the app itself when the backend is down or refuses', async () => {
    for (const fetchImpl of [vi.fn(async () => { throw new Error('down'); }), backend(500, "'self' https://suno.com")]) {
      const { headers, res } = response();
      await framePolicyMiddleware('http://127.0.0.1:3140', fetchImpl as typeof fetch)(request({ accept: 'text/html' }), res, vi.fn());
      expect(headers.get('content-security-policy')).toBe(SELF_ONLY);
    }
  });

  it('leaves modules and API calls alone', async () => {
    const fetchImpl = backend(204, "'self'");
    for (const req of [
      request({ 'sec-fetch-dest': 'script' }, '/src/main.tsx'),
      request({ accept: 'text/html' }, '/api/health'),
      request({ accept: 'text/html' }, '/', 'POST'),
    ]) {
      const { headers, res } = response();
      const next = vi.fn();
      await framePolicyMiddleware('http://127.0.0.1:3140', fetchImpl)(req, res, next);
      expect(headers.size).toBe(0);
      expect(next).toHaveBeenCalledOnce();
    }
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it('is registered on the Vite server', () => {
    const names = ((viteConfig as UserConfig).plugins ?? []).flat().map(plugin => (plugin && typeof plugin === 'object' && 'name' in plugin ? plugin.name : null));
    expect(names).toContain('kronn-frame-policy');
  });
});
