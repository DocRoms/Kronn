import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, test, type Page } from '@playwright/test';
import { framePolicyMiddleware } from '../../vite-frame-policy.ts';
import { planLivePageEmbeds } from '../../src/lib/live-page-embeds';
import { parseServedFrameSources } from '../../src/lib/served-frame-policy';

// The host frame policy in a real Chromium. The document is served through the
// native-mode middleware, reading its sources from a stand-in backend exactly as
// it reads Kronn's `/api/embed-origins/frame-src` (the desktop serves the same
// policy, built by `document_frame_policy`). Each third-party origin is a real
// loopback server that counts the requests it receives.

const SANDBOX = 'allow-scripts allow-same-origin allow-popups allow-presentation';

interface Origin { url: string; hits: string[]; server: Server }

async function listen(handler: (req: IncomingMessage, res: ServerResponse) => void): Promise<Server> {
  const server = createServer(handler);
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  return server;
}
// Each server has its own port, hence its own origin.
const urlOf = (server: Server) => `http://127.0.0.1:${(server.address() as AddressInfo).port}`;

async function thirdParty(routes: (origin: Origin, req: IncomingMessage, res: ServerResponse) => void): Promise<Origin> {
  const origin = { url: '', hits: [] as string[], server: null as unknown as Server };
  origin.server = await listen((req, res) => { origin.hits.push(req.url ?? ''); routes(origin, req, res); });
  origin.url = urlOf(origin.server);
  return origin;
}

const html = (res: ServerResponse, body: string) => { res.statusCode = 200; res.setHeader('content-type', 'text/html'); res.end(body); };

let allowed: Origin;
let forbidden: Origin;
let backend: Server;
let app: Server;
let appUrl = '';
let lastPolicy = '';

test.beforeAll(async () => {
  forbidden = await thirdParty((_origin, _req, res) => html(res, '<body>forbidden</body>'));
  allowed = await thirdParty((_origin, req, res) => {
    if (req.url === '/redirect') { res.writeHead(302, { location: `${forbidden.url}/landed` }); res.end(); return; }
    if (req.url === '/navigate') { html(res, `<script>location.href = ${JSON.stringify(`${forbidden.url}/navigated`)};</script>`); return; }
    if (req.url === '/subframe') { html(res, `<body>allowed<iframe id="nested" src="${forbidden.url}/nested"></iframe></body>`); return; }
    html(res, '<body>allowed</body>');
  });
  backend = await listen((req, res) => {
    if (req.url !== '/api/embed-origins/frame-src') { res.writeHead(404); res.end(); return; }
    res.writeHead(204, { 'x-kronn-frame-src': `'self' ${allowed.url}` });
    res.end();
  });
  const appDocument = (req: IncomingMessage) => {
    const frames = new URL(req.url ?? '/', 'http://app').searchParams.get('frames') ?? '';
    return '<!doctype html><html><head></head><body>' + frames.split(',').filter(Boolean).map((path, index) =>
      `<iframe id="f${index}" sandbox="${SANDBOX}" src="${path.startsWith('forbidden:') ? forbidden.url + path.slice(10) : allowed.url + path}"></iframe>`).join('') + '</body></html>';
  };
  // The app document is rendered by the middleware, as in the Vite plugin.
  const policy = framePolicyMiddleware(urlOf(backend), fetch, async req => appDocument(req));
  app = await listen(async (req, res) => {
    if (req.url?.startsWith('/unprotected')) { html(res, appDocument(req)); return; }
    await policy(req, res, () => { res.statusCode = 404; res.end(); });
    lastPolicy = String(res.getHeader('content-security-policy') ?? '');
  });
  appUrl = urlOf(app);
});

test.afterAll(async () => {
  for (const server of [allowed?.server, forbidden?.server, backend, app]) await new Promise(resolve => server?.close(resolve));
});

test.beforeEach(() => {
  allowed.hits.length = 0;
  forbidden.hits.length = 0;
});

async function open(page: Page, frames: string[], path = '/') {
  await page.goto(`${appUrl}${path}?frames=${frames.join(',')}`);
  // Let each frame finish (or fail) its navigation chain.
  await page.waitForLoadState('load');
  await page.waitForTimeout(500);
}

const frameText = (page: Page, id: string) => page.frameLocator(`#${id}`).locator('body').textContent({ timeout: 2_000 }).catch(() => null);

test.describe('host frame policy', () => {
  test('lists exactly the allowed site, nothing wider', async ({ page }) => {
    await open(page, ['/ok']);
    expect(lastPolicy).toBe(`frame-src 'self' ${allowed.url}; child-src 'self' ${allowed.url}; worker-src 'self' blob:`);
    expect(await frameText(page, 'f0')).toBe('allowed');
  });

  test('a forbidden site never receives a request, directly or through an allowed site’s redirect or navigation', async ({ page }) => {
    await open(page, ['forbidden:/direct', '/redirect', '/navigate']);
    expect(allowed.hits).toEqual(expect.arrayContaining(['/redirect', '/navigate']));
    expect(forbidden.hits).toEqual([]);
    expect(await frameText(page, 'f0')).not.toBe('forbidden');
    expect(await frameText(page, 'f1')).not.toBe('forbidden');
    expect(await frameText(page, 'f2')).not.toBe('forbidden');
  });

  test('without the policy the same redirect reaches the forbidden site (the test can fail)', async ({ page }) => {
    await open(page, ['/redirect'], '/unprotected');
    await expect.poll(() => forbidden.hits).toContain('/landed');
  });

  test('known limit: a frame the allowed site creates inside its own page is not governed by the host', async ({ page }) => {
    await open(page, ['/subframe']);
    expect(await frameText(page, 'f0')).toContain('allowed');
    // CSP does not reach into a cross-origin document: this request is not blockable by Kronn.
    await expect.poll(() => forbidden.hits).toContain('/nested');
  });
});

// Revocation under a policy that still admits the revoked site: the document
// keeps the CSP it was served with, so a player whose own site stays allowed
// may sit on, or return to, the revoked one. The host plan (the real planner)
// must take every player down; the overlay renders exactly `plan.players`.
test.describe('revoking a site the document policy admits', () => {
  let a: Origin;
  let b: Origin;
  let revokeBackend: Server;
  let host: Server;
  let hostUrl = '';

  test.beforeAll(async () => {
    b = await thirdParty((_origin, _req, res) => html(res, '<body>site-b</body>'));
    a = await thirdParty((_origin, req, res) => {
      if (req.url === '/to-b') { res.writeHead(302, { location: `${b.url}/landed` }); res.end(); return; }
      html(res, '<body>site-a</body>');
    });
    revokeBackend = await listen((_req, res) => { res.writeHead(204, { 'x-kronn-frame-src': `'self' ${a.url} ${b.url}` }); res.end(); });
    const policy = framePolicyMiddleware(urlOf(revokeBackend), fetch, async req => (req.url === '/lazy'
      ? '<!doctype html><html><head><title>host</title></head><body></body></html>'
      : `<!doctype html><html><head></head><body><iframe id="player" sandbox="${SANDBOX}" src="${a.url}/to-b"></iframe></body></html>`));
    host = await listen((req, res) => { void policy(req, res, () => { res.statusCode = 404; res.end(); }); });
    hostUrl = urlOf(host);
  });

  test.afterAll(async () => {
    for (const server of [a?.server, b?.server, revokeBackend, host]) await new Promise(resolve => server?.close(resolve));
  });

  test.beforeEach(() => { a.hits.length = 0; b.hits.length = 0; });

  // The fixtures run over http: A is allowed with its https address too, as
  // the host requires before it frames an http site.
  const aAllowed = () => [a.url, a.url.replace('http:', 'https:')];

  const placement = () => [{ key: 'ea:0', url: `${a.url}/to-b`, rect: { left: 0, top: 0, width: 300, height: 150 }, visible: true }];
  const onB = (page: Page) => page.frames().some(frame => frame.url().startsWith(b.url));

  /** Keep exactly the players the plan draws, as the overlay does. */
  async function apply(page: Page, keys: string[]) {
    await page.evaluate(keep => { if (!keep.includes('ea:0')) document.getElementById('player')?.remove(); }, keys);
  }

  test('every player is taken down, B’s document is gone and B gets no new request', async ({ page }) => {
    await page.goto(hostUrl);
    await expect.poll(() => onB(page)).toBe(true);
    expect(b.hits).toEqual(['/landed']);

    // B revoked, A still allowed; the document's policy (first read) admitted both.
    const plan = planLivePageEmbeds(placement(), new Set(aAllowed()), new Set([a.url, b.url]));
    expect(plan.players).toEqual([]);
    expect(plan.reload.map(entry => [entry.origin, entry.reason])).toEqual([[a.url, 'revoked']]);
    await apply(page, plan.players.map(player => player.placement.key));

    expect(onB(page)).toBe(false);
    await page.waitForTimeout(800);
    expect(b.hits).toEqual(['/landed']);
  });

  test('control: a plan keyed on the starting origin keeps B on screen and lets A reach B again', async ({ page }) => {
    await page.goto(hostUrl);
    await expect.poll(() => onB(page)).toBe(true);
    const naive = planLivePageEmbeds(placement(), new Set(aAllowed()), new Set([a.url]));
    expect(naive.players.map(player => player.placement.key)).toEqual(['ea:0']);
    await apply(page, naive.players.map(player => player.placement.key));
    expect(onB(page)).toBe(true);
    expect(b.hits).toEqual(['/landed']);
    // A remount under the same document policy: A redirects to B once more.
    await page.evaluate(() => { const frame = document.getElementById('player') as HTMLIFrameElement; frame.src = frame.src; });
    await expect.poll(() => b.hits.length).toBe(2);
  });

  /** Mount the players a plan draws, as the overlay does when the Page opens. */
  async function mount(page: Page, keys: string[]) {
    await page.evaluate(({ keep, src, sandbox }) => {
      if (!keep.includes('ea:0')) return;
      const frame = document.createElement('iframe');
      frame.id = 'player';
      frame.setAttribute('sandbox', sandbox);
      frame.src = src;
      document.body.append(frame);
    }, { keep: keys, src: `${a.url}/to-b`, sandbox: SANDBOX });
  }

  test('a site revoked between serving the document and opening the Page: the served marker keeps players down', async ({ page }) => {
    await page.goto(`${hostUrl}/lazy`);
    // The document was served admitting A and B; B is revoked before the first list read.
    const marker = await page.evaluate(() => document.querySelector<HTMLMetaElement>('meta[name="kronn-served-frame-src"]')?.content ?? null);
    expect(marker).toBe(`'self' ${a.url} ${b.url}`);
    const plan = planLivePageEmbeds(placement(), new Set(aAllowed()), parseServedFrameSources(marker));
    expect(plan.players).toEqual([]);
    expect(plan.reload.map(entry => entry.reason)).toEqual(['revoked']);
    await mount(page, plan.players.map(player => player.placement.key));
    await page.waitForTimeout(800);
    expect(a.hits).toEqual([]);
    expect(b.hits).toEqual([]);
  });

  test('control: taking the first list read as the policy mounts A and A reaches B', async ({ page }) => {
    await page.goto(`${hostUrl}/lazy`);
    const firstRead = new Set(aAllowed());
    const plan = planLivePageEmbeds(placement(), firstRead, firstRead);
    await mount(page, plan.players.map(player => player.placement.key));
    await expect.poll(() => b.hits).toEqual(['/landed']);
  });
});
