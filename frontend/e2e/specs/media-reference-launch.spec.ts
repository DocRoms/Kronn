/**
 * KT-551 / KT-555 — a generation started from pictures of the room, end to end.
 *
 * Drives the real backend against a local stub standing in for the provider —
 * no credentials, no cost, deterministic. What no unit test can prove: that
 * the picture chosen in the launcher reaches the provider as an inline `data:`
 * payload in the documented field, that NOTHING about where the file lives
 * ever leaves Kronn, and that the finished bubble points back at the source.
 *
 * The stub records the exact request body, which is the only way to know what
 * actually left the process.
 */
import { createServer, type Server } from 'node:http';
import { expect, test } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { openMediaLauncher } from '../pages/MediaLauncher';

const ONE_PIXEL_PNG_B64 =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=';

type Recorded = { url: string; body: Record<string, unknown> };

function startStubProvider(seen: Recorded[]): Promise<{ server: Server; port: number }> {
  return new Promise((resolve, reject) => {
    const server = createServer((req, res) => {
      const chunks: Buffer[] = [];
      req.on('data', chunk => chunks.push(chunk));
      req.on('end', () => {
        const raw = Buffer.concat(chunks).toString('utf-8');
        if (req.method === 'POST' && req.url?.startsWith('/v1/images')) {
          seen.push({ url: req.url, body: raw ? JSON.parse(raw) : {} });
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(JSON.stringify({
            id: 'gen-ref-1',
            data: [{ b64_json: ONE_PIXEL_PNG_B64 }],
            usage: { cost: 0.01, is_byok: false },
          }));
          return;
        }
        // The capability catalogue: an unreachable one is not a refusal, but
        // answering here is what exercises the advertised ceiling.
        if (req.method === 'GET' && req.url?.includes('/images/models')) {
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(JSON.stringify({
            data: [{
              id: 'stub/e2e-image',
              supported_parameters: {
                aspect_ratio: { type: 'enum', values: ['1:1'] },
                input_references: { type: 'range', min: 0, max: 3 },
              },
            }],
          }));
          return;
        }
        res.writeHead(404, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ error: 'no stub route for ' + req.url }));
      });
    });
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (!address || typeof address === 'string') {
        reject(new Error('stub provider did not bind a port'));
        return;
      }
      resolve({ server, port: address.port });
    });
  });
}

let stub: { server: Server; port: number } | null = null;
const createdDiscussionIds = new Set<string>();
const createdConnectionIds = new Set<string>();

test.afterEach(async ({ request }) => {
  for (const id of createdDiscussionIds) {
    await request.delete(`/api/discussions/${id}`).catch(() => {});
  }
  createdDiscussionIds.clear();
  for (const id of createdConnectionIds) {
    await request.delete(`/api/external-api/connections/${id}`).catch(() => {});
  }
  createdConnectionIds.clear();
  if (stub) {
    await new Promise<void>(done => stub!.server.close(() => done()));
    stub = null;
  }
});

test('a picture attached in the launcher reaches the provider inline, and the bubble points back at it', async ({ page, request }) => {
  test.setTimeout(90_000);
  const seen: Recorded[] = [];
  stub = await startStubProvider(seen);

  const connection = await request.post('/api/external-api/connections', {
    data: {
      display_name: `E2E ref media ${Date.now()}`,
      mention_alias: `e2eref${Date.now()}`,
      endpoint: `http://127.0.0.1:${stub.port}`,
      origin_preset: 'open_router',
      api_key: 'sk-or-v1-e2e-stub-not-a-real-key',
      image_model: 'stub/e2e-image',
    },
  });
  expect(connection.ok()).toBe(true);
  const connectionId = (await connection.json())?.data?.id as string;
  expect(connectionId).toBeTruthy();
  createdConnectionIds.add(connectionId);

  const discussion = await request.post('/api/discussions', {
    data: {
      title: `KT-551 reference launch ${Date.now()}`,
      agent: 'Codex',
      language: 'fr',
      initial_prompt: 'Discussion de test KT-551, sans agent.',
      no_agent: true,
    },
  });
  expect(discussion.ok()).toBe(true);
  const discussionId = (await discussion.json())?.data?.id as string;
  expect(discussionId).toBeTruthy();
  createdDiscussionIds.add(discussionId);

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.openDiscussion(discussionId);

  const form = await openMediaLauncher(page, connectionId);

  // Wide enough for the provider: measured on 02/09, a source under 300 px
  // comes back `400 InvalidParameter`, and the launcher blocks it before the
  // click. Painted by the browser rather than hard-coded, so the bytes are a
  // real PNG of a real size.
  const wideePng = await page.evaluate(async () => {
    const canvas = document.createElement('canvas');
    canvas.width = 320;
    canvas.height = 320;
    const paint = canvas.getContext('2d')!;
    paint.fillStyle = 'rgb(200, 120, 40)';
    paint.fillRect(0, 0, 320, 320);
    const blob = await new Promise<Blob | null>(done => canvas.toBlob(done, 'image/png'));
    const bytes = new Uint8Array(await blob!.arrayBuffer());
    return btoa(String.fromCharCode(...bytes));
  });

  // The room holds no picture yet: attaching one from the launcher is the
  // path that has to work, and it must be picked without a second step.
  const attach = form.getByTestId('media-reference-attach');
  await expect(attach).toBeVisible({ timeout: 15_000 });
  await attach.locator('input[type=file]').setInputFiles({
    name: 'origami.png',
    mimeType: 'image/png',
    buffer: Buffer.from(wideePng, 'base64'),
  });
  await expect(form.getByTestId('media-reference-picker')).toBeVisible();
  await expect(form.locator('[data-testid^="media-reference-drop-"]')).toHaveCount(1);

  const prompt = 'un renard origami KT-551';
  await form.locator('textarea').fill(prompt);
  // The width guard must not be standing: a picture the provider would accept
  // has to leave the button usable.
  await expect(form.getByTestId('media-reference-too-narrow')).toHaveCount(0);
  const generate = form.getByRole('button', { name: /Générer/ });
  await expect(generate).toBeEnabled();
  await generate.click();

  const bubble = page.locator('[data-message-id][data-media-run-id]', { hasText: prompt });
  await expect(bubble).toBeVisible({ timeout: 15_000 });
  await expect(bubble.getByTestId('run-status-card'))
    .toHaveAttribute('data-status', 'success', { timeout: 45_000 });

  // What actually left the process.
  expect(seen.length, 'the provider must have been called exactly once').toBe(1);
  const body = seen[0].body as { input_references?: Array<{ image_url?: { url?: string } }> };
  const references = body.input_references ?? [];
  expect(references).toHaveLength(1);
  const url = references[0]?.image_url?.url ?? '';
  // Inline, because Kronn listens on 127.0.0.1: there is no address a provider
  // could fetch, and publishing one would hand a private file to the internet.
  expect(url.startsWith('data:image/png;base64,')).toBe(true);
  const wire = JSON.stringify(body);
  expect(wire).not.toContain('/tmp/');
  expect(wire).not.toContain('127.0.0.1');
  expect(wire).not.toContain('sk-or-v1');
  // A frame belongs to a clip; nothing of it may reach the images route.
  expect(wire).not.toContain('frame_type');

  // The finished bubble says what it was built on, and opens it.
  await expect(bubble.getByTestId('media-bubble-source')).toBeVisible({ timeout: 15_000 });
  await bubble.getByTestId('media-bubble-source-open').click();
  const viewer = page.getByRole('dialog', { name: /Visionneuse|viewer/i });
  await expect(viewer).toBeVisible();
  await expect(viewer).toHaveAttribute('data-asset-id', /.+/);
});
