import type { IncomingMessage, ServerResponse } from 'node:http';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import type { Plugin } from 'vite';

/** Backend route that lists the allowed embed sites as CSP sources. */
export const FRAME_SRC_PATH = '/api/embed-origins/frame-src';
const FRAME_SRC_HEADER = 'x-kronn-frame-src';
// Mirrors MAX_FRAME_SRC_BYTES and is_csp_host_source in core/embed_origins.rs.
const MAX_FRAME_SRC_BYTES = 16 * 1024;
const HOST_SOURCE = /^https?:\/\/[a-z0-9.:-]+$/;

/**
 * The document policy the backend puts on desktop documents
 * (`document_frame_policy`), from the sources it returns. Anything that is not
 * exactly `'self'` plus plain host sources falls back to `'self'` alone.
 */
export function servedFrameSources(sources: string | null | undefined): string {
  const tokens = sources ? sources.split(' ') : [];
  const valid = !!sources
    && sources.length <= MAX_FRAME_SRC_BYTES
    && tokens[0] === "'self'"
    && tokens.slice(1).every(token => HOST_SOURCE.test(token));
  return valid ? sources : "'self'";
}

export function documentFramePolicy(sources: string | null | undefined): string {
  const list = servedFrameSources(sources);
  return `frame-src ${list}; child-src ${list}; worker-src 'self' blob:`;
}

/** Mirrors SERVED_FRAME_SRC_META in core/embed_origins.rs. */
export const SERVED_FRAME_SRC_META = 'kronn-served-frame-src';

const escapeAttribute = (value: string) => value.replace(/[&"'<>]/g, c => ({ '&': '&amp;', '"': '&quot;', "'": '&#39;', '<': '&lt;', '>': '&gt;' })[c]!);

/** `html` with the served-sources marker after its `<head>` tag, unchanged when it has none. */
export function injectServedFrameSources(html: string, sources: string): string {
  const head = /<head(?=[>\s])[^>]*>/i.exec(html);
  if (!head) return html;
  const end = head.index + head[0].length;
  return `${html.slice(0, end)}<meta name="${SERVED_FRAME_SRC_META}" content="${escapeAttribute(sources)}">${html.slice(end)}`;
}

function isDocumentRequest(req: IncomingMessage): boolean {
  if (req.method !== 'GET' && req.method !== 'HEAD') return false;
  const path = req.url ?? '/';
  if (path.startsWith('/api/') || path.startsWith('/@')) return false;
  const dest = req.headers['sec-fetch-dest'];
  if (typeof dest === 'string') return dest === 'document' || dest === 'iframe' || dest === 'frame';
  return (req.headers.accept ?? '').includes('text/html');
}

type Next = (error?: unknown) => void;
/** The app document for this request, before the marker is added. */
export type RenderIndex = (req: IncomingMessage) => Promise<string>;

// Paths the dev server answers with the app document (SPA fallback included).
function servesAppDocument(req: IncomingMessage): boolean {
  const path = (req.url ?? '/').split(/[?#]/)[0];
  const last = path.slice(path.lastIndexOf('/') + 1);
  return last === '' || last === 'index.html' || !last.includes('.');
}

/**
 * Native mode serves the app from Vite: each document gets the host frame
 * policy read from the backend at request time (`'self'` when it is down),
 * and the app document is rendered here so its marker and its CSP header come
 * from the same value, set before anything is written. Other documents get
 * the header and no marker, so the page treats its policy as unknown.
 */
export function framePolicyMiddleware(backendUrl: string, fetchImpl: typeof fetch = fetch, renderIndex?: RenderIndex) {
  return async (req: IncomingMessage, res: ServerResponse, next: Next): Promise<void> => {
    if (!isDocumentRequest(req)) { next(); return; }
    let sources: string | null = null;
    try {
      const response = await fetchImpl(new URL(FRAME_SRC_PATH, backendUrl), { signal: AbortSignal.timeout(2_000) });
      if (response.ok) sources = response.headers.get(FRAME_SRC_HEADER);
    } catch { /* Backend unreachable: frame only the app itself. */ }
    const served = servedFrameSources(sources);
    // A reload must carry the current list, never a 304 for an older one.
    delete req.headers['if-none-match'];
    delete req.headers['if-modified-since'];
    if (!renderIndex || !servesAppDocument(req)) {
      res.setHeader('Content-Security-Policy', documentFramePolicy(served));
      next();
      return;
    }
    let html: string;
    try {
      html = injectServedFrameSources(await renderIndex(req), served);
    } catch (error) {
      next(error);
      return;
    }
    res.statusCode = 200;
    res.setHeader('Content-Type', 'text/html; charset=utf-8');
    res.setHeader('Cache-Control', 'no-cache');
    res.setHeader('Content-Security-Policy', documentFramePolicy(served));
    res.end(req.method === 'HEAD' ? undefined : html);
  };
}

/**
 * The dev server (`pnpm dev`, used by `./kronn start-dev` and the desktop dev
 * URL). `vite preview` is not used by Kronn and gets no policy: its documents
 * carry no marker, so embeds stay down there.
 */
export function framePolicyPlugin(backendUrl: string, fetchImpl: typeof fetch = (...args) => fetch(...args)): Plugin {
  return {
    name: 'kronn-frame-policy',
    configureServer(server) {
      server.middlewares.use(framePolicyMiddleware(backendUrl, fetchImpl, async req => {
        const raw = await readFile(join(server.config.root, 'index.html'), 'utf8');
        return server.transformIndexHtml(req.url ?? '/', raw, (req as IncomingMessage & { originalUrl?: string }).originalUrl);
      }));
    },
  };
}
