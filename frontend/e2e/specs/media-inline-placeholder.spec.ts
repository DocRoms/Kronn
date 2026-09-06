/**
 * KT-549 — the inline media placeholder, end to end.
 *
 * Drives the real backend (worker sweep included) against a local stub HTTP
 * server standing in for the provider — no real credentials, no external
 * cost, fully deterministic. Proves the part a screenshot pass or a unit test
 * cannot: that launching a generation from an open discussion immediately
 * shows a placeholder in the transcript, at a message dedicated to that job
 * (not the previous one), that the launcher is reusable right away for a
 * second, independent job, and that the placeholder becomes the real asset in
 * that same spot — surviving a reload.
 */
import { createServer, type Server } from 'node:http';
import { expect, test } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { openMediaLauncher } from '../pages/MediaLauncher';

// A real, minimal 1x1 PNG — so the browser's <img> actually decodes it
// instead of just receiving opaque bytes.
const ONE_PIXEL_PNG_B64 =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=';

function startStubProvider(answerDelayMs = 0): Promise<{ server: Server; port: number }> {
  return new Promise((resolve, reject) => {
    const server = createServer((req, res) => {
      const chunks: Buffer[] = [];
      req.on('data', chunk => chunks.push(chunk));
      req.on('end', () => {
        if (req.method === 'POST' && req.url?.startsWith('/v1/images')) {
          // A delay is what makes "the client was closed WHILE it generated"
          // testable at all: an instant answer leaves no such window.
          const answer = () => {
            res.writeHead(200, { 'content-type': 'application/json' });
            res.end(JSON.stringify({
              id: 'gen-e2e-1',
              data: [{ b64_json: ONE_PIXEL_PNG_B64 }],
              usage: { cost: 0.01, is_byok: false },
            }));
          };
          if (answerDelayMs > 0) setTimeout(answer, answerDelayMs);
          else answer();
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
    await new Promise<void>(resolveClose => stub!.server.close(() => resolveClose()));
    stub = null;
  }
});

test('a launch materializes an inline placeholder that becomes the asset in place, and the launcher stays usable', async ({ page, request }) => {
  test.setTimeout(90_000);
  stub = await startStubProvider();

  const connection = await request.post('/api/external-api/connections', {
    data: {
      display_name: `E2E stub media ${Date.now()}`,
      mention_alias: `e2estub${Date.now()}`,
      endpoint: `http://127.0.0.1:${stub.port}`,
      origin_preset: 'open_router',
      api_key: 'sk-or-v1-e2e-stub-not-a-real-key',
      image_model: 'stub/e2e-image',
    },
  });
  expect(connection.ok()).toBe(true);
  const connectionBody = await connection.json();
  const connectionId = connectionBody?.data?.id as string;
  expect(connectionId).toBeTruthy();
  createdConnectionIds.add(connectionId);

  const discussion = await request.post('/api/discussions', {
    data: {
      title: `KT-549 media placeholder ${Date.now()}`,
      agent: 'Codex',
      language: 'fr',
      initial_prompt: 'Discussion de test KT-549, sans agent.',
      no_agent: true,
    },
  });
  expect(discussion.ok()).toBe(true);
  const discussionBody = await discussion.json();
  const discussionId = discussionBody?.data?.id as string;
  expect(discussionId).toBeTruthy();
  createdDiscussionIds.add(discussionId);

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.openDiscussion(discussionId);

  const form = await openMediaLauncher(page, connectionId);

  const promptOne = 'un chat en origami KT-549';
  await form.locator('textarea').fill(promptOne);
  await form.getByRole('button', { name: /Générer/ }).click();

  // Immediately materialized as ONE media bubble — never a generic User
  // message followed by a detached run card.
  const firstBubble = page.locator('[data-message-id][data-media-run-id]', { hasText: promptOne });
  await expect(firstBubble).toBeVisible({ timeout: 10_000 });
  const firstCard = firstBubble.getByTestId('run-status-card');
  await expect(firstCard).toBeVisible({ timeout: 10_000 });
  await expect(page.getByRole('progressbar')).toHaveCount(0);

  // The launcher is a launcher, not a tracker: reusable immediately for a
  // second, independent job — which must get its OWN message, not share the
  // first one's.
  await expect(form).toBeVisible();
  const promptTwo = 'un chien en origami KT-549';
  await form.locator('textarea').fill(promptTwo);
  await form.getByRole('button', { name: /Générer/ }).click();
  const secondBubble = page.locator('[data-message-id][data-media-run-id]', { hasText: promptTwo });
  await expect(secondBubble).toBeVisible({ timeout: 10_000 });
  const firstId = await firstBubble.getAttribute('data-message-id');
  const secondId = await secondBubble.getAttribute('data-message-id');
  expect(firstId).toBeTruthy();
  expect(secondId).toBeTruthy();
  expect(firstId).not.toBe(secondId);

  // The backend worker sweeps every 5s; the stub answers synchronously, so
  // both settle within one or two sweeps. The SAME bubble becomes successful
  // and exposes the exact asset to the canonical Assets viewer.
  await expect(firstCard).toHaveAttribute('data-status', 'success', { timeout: 30_000 });
  await expect(secondBubble.getByTestId('run-status-card')).toHaveAttribute('data-status', 'success', { timeout: 30_000 });
  // The status card hydrates from its own run subscription while the bubble's
  // asset comes from the relist, so the asset lands just after the success.
  // Waiting for it is the real contract; reading it once only tests timing.
  await expect(firstBubble).toHaveAttribute('data-media-asset-id', /.+/, { timeout: 15_000 });
  const firstAssetId = await firstBubble.getAttribute('data-media-asset-id');
  // KT-553 — the produced picture shows up in the bubble itself, and the
  // bubble offers ONE destination: the card's generic "open the run" link is
  // recomputed on every rehydration, so its suppression has to hold here.
  await expect(firstBubble.getByTestId('media-bubble-preview').locator('img'))
    .toBeVisible({ timeout: 15_000 });
  await expect(firstCard.locator('a')).toHaveCount(0);
  await expect(firstBubble.getByTestId('media-bubble-open-asset')).toHaveCount(1);
  await firstBubble.getByTestId('media-bubble-open-asset').click();
  await expect(page.getByRole('complementary', { name: 'Assets' })).toBeVisible();
  const viewer = page.getByRole('dialog', { name: /Visionneuse|viewer/i });
  await expect(viewer).toBeVisible();
  await expect(viewer).toHaveAttribute('data-asset-id', firstAssetId!);
  await viewer.getByRole('button', { name: /Fermer la visionneuse|close viewer/i }).click();

  // Reopening (here: a full reload) must rehydrate to the exact same result.
  await page.reload();
  await dashboard.navDiscussions.waitFor({ state: 'visible', timeout: 15_000 });
  await dashboard.openDiscussion(discussionId);
  const firstBubbleAfterReload = page.locator('[data-message-id][data-media-run-id]', { hasText: promptOne });
  await expect(firstBubbleAfterReload.getByTestId('run-status-card')).toHaveAttribute('data-status', 'success', { timeout: 10_000 });
  await expect(firstBubbleAfterReload.getByTestId('media-bubble-open-asset')).toBeVisible();
  const secondBubbleAfterReload = page.locator('[data-message-id][data-media-run-id]', { hasText: promptTwo });
  await expect(secondBubbleAfterReload.getByTestId('run-status-card')).toHaveAttribute('data-status', 'success', { timeout: 10_000 });
});

test('a generation that finished while the client was away rehydrates from the server alone', async ({ page, request }) => {
  test.setTimeout(120_000);
  // The provider answers only after the client is gone, so the success can
  // reach the browser through a server read and nothing else — no live event
  // ever carried it.
  stub = await startStubProvider(8_000);

  const connection = await request.post('/api/external-api/connections', {
    data: {
      display_name: `E2E stub away ${Date.now()}`,
      mention_alias: `e2eaway${Date.now()}`,
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
      title: `KT-549 away rehydration ${Date.now()}`,
      agent: 'Codex',
      language: 'fr',
      initial_prompt: 'Discussion de test KT-549, sans agent.',
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

  const prompt = 'un renard en origami KT-549 pendant absence';
  await form.locator('textarea').fill(prompt);
  await form.getByRole('button', { name: /Générer/ }).click();

  // Still running when we walk away — otherwise this test would prove nothing
  // more than the reload case already does.
  const bubble = page.locator('[data-message-id][data-media-run-id]', { hasText: prompt });
  await expect(bubble).toBeVisible({ timeout: 10_000 });
  const card = bubble.getByTestId('run-status-card');
  await expect(card).not.toHaveAttribute('data-status', 'success');

  // Leave: the socket closes and no live update can reach this client.
  await page.goto('about:blank');
  await expect(page.locator('body')).toBeVisible();

  // The job settles server-side while nobody is watching.
  await request.get(`/api/discussions/${discussionId}`);
  await expect
    .poll(
      async () => {
        const runs = await request.get(`/api/runs?kind=media&discussion_id=${discussionId}`);
        const body = await runs.json();
        const rows = (body?.data ?? []) as Array<{ status?: string }>;
        return rows[0]?.status ?? 'none';
      },
      { timeout: 60_000, intervals: [1_000] },
    )
    .toBe('success');

  // Come back to a client that never saw the transition happen.
  await dashboard.goto();
  await dashboard.openDiscussion(discussionId);
  const bubbleOnReturn = page.locator('[data-message-id][data-media-run-id]', { hasText: prompt });
  await expect(bubbleOnReturn.getByTestId('run-status-card')).toHaveAttribute('data-status', 'success', { timeout: 15_000 });
  await expect(bubbleOnReturn.getByTestId('media-bubble-open-asset')).toBeVisible();
  await expect(bubbleOnReturn).toHaveAttribute('data-media-asset-id', /.+/);
});

test('an unreachable connection settles as a clear terminal failure, not a silent hang', async ({ page, request }) => {
  test.setTimeout(60_000);

  const connection = await request.post('/api/external-api/connections', {
    data: {
      display_name: `E2E unreachable media ${Date.now()}`,
      mention_alias: `e2eunreach${Date.now()}`,
      // Nothing listens here: deterministic, zero-cost failure.
      endpoint: 'http://127.0.0.1:1',
      origin_preset: 'open_router',
      api_key: 'sk-or-v1-e2e-stub-not-a-real-key',
      image_model: 'stub/e2e-image',
    },
  });
  const connectionBody = await connection.json();
  const connectionId = connectionBody?.data?.id as string;
  createdConnectionIds.add(connectionId);

  const discussion = await request.post('/api/discussions', {
    data: {
      title: `KT-549 media failure ${Date.now()}`,
      agent: 'Codex',
      language: 'fr',
      initial_prompt: 'Discussion de test KT-549 (échec), sans agent.',
      no_agent: true,
    },
  });
  const discussionBody = await discussion.json();
  const discussionId = discussionBody?.data?.id as string;
  createdDiscussionIds.add(discussionId);

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.openDiscussion(discussionId);
  const form = await openMediaLauncher(page, connectionId);
  const prompt = 'un paysage KT-549 échec';
  await form.locator('textarea').fill(prompt);
  await form.getByRole('button', { name: /Générer/ }).click();

  const bubble = page.locator('[data-message-id][data-media-run-id]', { hasText: prompt });
  await expect(bubble).toBeVisible({ timeout: 10_000 });
  const card = bubble.getByTestId('run-status-card');
  await expect(card).toHaveAttribute('data-status', 'failed', { timeout: 30_000 });
  // A clear reason, not a raw payload nor an infinite spinner.
  await expect(card.locator('.run-status-card-diagnostic')).toBeVisible();
});
