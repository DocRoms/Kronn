// @vitest-environment node
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer as createHttpServer, request as httpRequest, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { gunzipSync } from 'node:zlib';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createServer, preview, type PreviewServer, type ViteDevServer } from 'vite';
import { documentFramePolicy, framePolicyPlugin } from './vite-frame-policy.ts';

// The plugin inside real Vite servers, not only its middleware: the document
// must be served whole, its marker equal to its CSP header.

const MARKER = /<meta name="kronn-served-frame-src" content="([^"]*)">/;
let root = '';
let backend: Server;
let backendUrl = '';
let sources = "'self' https://suno.com";

interface Answer { status: number; headers: Record<string, string | string[] | undefined>; body: string }

function get(url: string, headers: Record<string, string>): Promise<Answer> {
  return new Promise((resolve, reject) => {
    httpRequest(url, { headers }, res => {
      const chunks: Buffer[] = [];
      res.on('data', chunk => chunks.push(chunk));
      res.on('end', () => {
        const raw = Buffer.concat(chunks);
        const body = res.headers['content-encoding'] === 'gzip' ? gunzipSync(raw) : raw;
        resolve({ status: res.statusCode ?? 0, headers: res.headers, body: body.toString() });
      });
    }).on('error', reject).end();
  });
}

// The suite's setup forbids global fetch; the backend is reached over node:http.
const backendFetch = ((input: string | URL) => new Promise<Response>((resolve, reject) => {
  httpRequest(String(input), res => {
    res.resume();
    res.on('end', () => {
      const value = res.headers['x-kronn-frame-src'];
      resolve(new Response(null, { status: res.statusCode ?? 0, headers: typeof value === 'string' ? { 'x-kronn-frame-src': value } : {} }));
    });
  }).on('error', reject).end();
})) as typeof fetch;

const DOCUMENT = { accept: 'text/html', 'sec-fetch-dest': 'document', 'accept-encoding': 'gzip, deflate, br' };

beforeAll(async () => {
  root = await mkdtemp(join(tmpdir(), 'kronn-frame-policy-'));
  await writeFile(join(root, 'index.html'), '<!doctype html><html><head><title>app</title></head><body><script type="module" src="/main.js"></script></body></html>');
  // Large enough for compression to apply where a server compresses.
  await writeFile(join(root, 'large.html'), `<!doctype html><html><head><title>large</title></head><body>${'<p>filler</p>'.repeat(4000)}</body></html>`);
  await writeFile(join(root, 'main.js'), 'export {};');
  backend = createHttpServer((_req, res) => { res.writeHead(204, { 'x-kronn-frame-src': sources }); res.end(); });
  await new Promise<void>(resolve => backend.listen(0, '127.0.0.1', resolve));
  backendUrl = `http://127.0.0.1:${(backend.address() as AddressInfo).port}`;
});

afterAll(async () => {
  await new Promise(resolve => backend.close(resolve));
  await rm(root, { recursive: true, force: true });
});

describe('frame policy plugin in a real Vite dev server', () => {
  let server: ViteDevServer;
  let base = '';

  beforeAll(async () => {
    server = await createServer({
      configFile: false, root, logLevel: 'silent', plugins: [framePolicyPlugin(backendUrl, backendFetch)],
      server: { host: '127.0.0.1', port: 0, strictPort: false, hmr: false, ws: false },
    });
    await server.listen();
    base = server.resolvedUrls!.local[0].replace(/\/$/, '');
  });
  afterAll(async () => { await server.close(); });

  it('serves the app document whole, its marker equal to its CSP header, small or large, identity or gzip', async () => {
    const small = await readFile(join(root, 'index.html'), 'utf8');
    for (const [size, html] of [['small', small], ['large', await readFile(join(root, 'large.html'), 'utf8')]] as const) {
      await writeFile(join(root, 'index.html'), html);
      for (const encoding of ['identity', 'gzip']) {
        for (const path of ['/', '/index.html', '/settings/x']) {
          const answer = await get(`${base}${path}`, { ...DOCUMENT, 'accept-encoding': encoding });
          const label = `${size} ${encoding} ${path}`;
          expect(answer.status, label).toBe(200);
          const marker = MARKER.exec(answer.body)?.[1]?.replace(/&#39;/g, "'");
          expect(marker, label).toBe(sources);
          expect(answer.headers['content-security-policy'], label).toBe(documentFramePolicy(marker));
          expect(answer.body.length, label).toBeGreaterThan(html.length);
        }
      }
    }
    await writeFile(join(root, 'index.html'), small);
    for (const path of ['/']) {
      const answer = await get(`${base}${path}`, DOCUMENT);
      expect(answer.status, path).toBe(200);
      const marker = MARKER.exec(answer.body)?.[1]?.replace(/&#39;/g, "'");
      expect(marker, path).toBe(sources);
      expect(answer.headers['content-security-policy']).toBe(documentFramePolicy(marker));
      // Vite's own transforms still ran.
      expect(answer.body).toContain('/@vite/client');
    }
  });

  it('follows the list per request and never answers 304', async () => {
    sources = "'self' https://other.example";
    const answer = await get(`${base}/`, { ...DOCUMENT, 'if-none-match': '*', 'if-modified-since': 'Sat, 01 Jan 2100 00:00:00 GMT' });
    expect(answer.status).toBe(200);
    expect(MARKER.exec(answer.body)?.[1]).toBe('&#39;self&#39; https://other.example');
    sources = "'self' https://suno.com";
  });

  it('leaves modules to Vite', async () => {
    const answer = await get(`${base}/main.js`, { accept: '*/*', 'sec-fetch-dest': 'script' });
    expect(answer.status).toBe(200);
    expect(answer.headers['content-security-policy']).toBeUndefined();
  });
});

describe('vite preview, not used by Kronn', () => {
  let server: PreviewServer;

  beforeAll(async () => {
    const outDir = join(root, 'dist');
    await mkdir(outDir, { recursive: true });
    await writeFile(join(outDir, 'index.html'), '<!doctype html><html><head><title>app</title></head></html>');
    await writeFile(join(outDir, 'large.html'), `<!doctype html><html><head><title>large</title></head><body>${'<p>filler</p>'.repeat(4000)}</body></html>`);
    server = await preview({
      configFile: false, root, logLevel: 'silent', plugins: [framePolicyPlugin(backendUrl, backendFetch)],
      build: { outDir }, preview: { host: '127.0.0.1', port: 0, strictPort: false },
    });
  });
  afterAll(async () => { await new Promise<void>(resolve => server.httpServer.close(() => resolve())); });

  it('the plugin is a no-op there: every document is served, small or large, identity or gzip, with no marker', async () => {
    const base = server.resolvedUrls!.local[0].replace(/\/$/, '');
    for (const [path, title] of [['/', 'app'], ['/large.html', 'large']]) {
      for (const encoding of ['identity', 'gzip']) {
        const answer = await get(`${base}${path}`, { ...DOCUMENT, 'accept-encoding': encoding });
        const label = `${path} ${encoding}`;
        expect(answer.status, label).toBe(200);
        expect(answer.body, label).toContain(`<title>${title}</title>`);
        expect(MARKER.test(answer.body), label).toBe(false);
        expect(answer.headers['content-security-policy'], label).toBeUndefined();
      }
    }
  });
});
