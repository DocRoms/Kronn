/**
 * REAL end-to-end — per-message image attachment on a real DOCROMS_WEB agent
 * discussion (0.8.8).
 *
 * Unlike the hermetic `disc-message-attachment-render.spec.ts` (which stubs
 * every endpoint), this one drives the ENTIRE real stack on a real project:
 *
 *   1. Find the DOCROMS_WEB project via the API, create a real ClaudeCode
 *      discussion bound to it.
 *   2. In the browser, attach a real PNG through the composer's file input
 *      (real multipart upload → saved to the project worktree → context_files
 *      row, message_id NULL = pending).
 *   3. Type + send. The real backend pins the pending file to the new user
 *      message (link_pending_context_files_to_message) and fires ClaudeCode.
 *   4. Assert the uploaded image renders as a thumbnail INSIDE the user's
 *      message bubble — proving the full chain: upload → link-on-send →
 *      auth'd byte route → blob → <img>. The composer is left empty (the file
 *      moved out of the input and into the message).
 *
 * We don't assert on the agent's reply — the feature under test is the
 * attachment pipeline, not the model output. We stop the run right after the
 * thumbnail appears to bound token cost (~$0.01).
 *
 * # Why real (not just the stubbed spec)
 *
 * The stub can't catch a regression in the real multipart handler, the
 * worktree image-save path, the send-time link UPDATE, or the byte-serving
 * route's DB lookup. Same rationale as `disc-cancel-recovery.spec.ts`.
 *
 * Local-only: needs Anthropic creds + the Claude binary + a clean DOCROMS_WEB
 * checkout. CI-skipped (tokens billed per run).
 */
import { test, expect, type APIRequestContext } from '@playwright/test';
import { DashboardPage } from '../pages/DashboardPage';

test.describe.configure({ timeout: 240_000, retries: 0 });
test.skip(!!process.env.CI, 'real-agent spec — local-only (Anthropic creds + Claude binary required, tokens billed per run)');

const TITLE = `Attach E2E ${Date.now()}`;
// Smallest valid PNG (1×1 transparent pixel) — a real binary upload.
const PNG_1x1 = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==',
  'base64',
);

async function findDocromsProjectId(request: APIRequestContext): Promise<string | null> {
  const r = await request.get('/api/projects');
  if (!r.ok()) return null;
  const j = await r.json();
  const projects: Array<{ id: string; name: string }> = j?.data ?? [];
  return projects.find(p => p.name === 'DOCROMS_WEB')?.id ?? null;
}

let discId: string | null = null;

test.describe('DOCROMS_WEB — attached image renders in the real message bubble', () => {
  test.afterAll(async ({ request }) => {
    if (discId) {
      await request.delete(`/api/discussions/${discId}`).catch(() => { /* idempotent */ });
    }
  });

  test('upload a PNG → send → it appears as a thumbnail in the user bubble', async ({ page, request }) => {
    const projectId = await findDocromsProjectId(request);
    test.skip(!projectId, 'DOCROMS_WEB project not registered in this Kronn instance');

    // 1. Create the real discussion bound to DOCROMS_WEB.
    const create = await request.post('/api/discussions', {
      data: {
        title: TITLE,
        agent: 'ClaudeCode',
        language: 'fr',
        project_id: projectId,
        initial_prompt: 'Discussion de test pour les pièces jointes.',
      },
    });
    expect(create.ok()).toBe(true);
    const cj = await create.json();
    discId = cj?.data?.id;
    expect(discId, 'discussion must be created').toBeTruthy();

    // 2. Open it in the browser. We hit the REAL backend (no API stubs), but
    //    still pre-dismiss the onboarding tour — otherwise its backdrop
    //    intercepts the nav click (same flag the kronn-fixture sets).
    await page.addInitScript(() => {
      localStorage.setItem('kronn:tour-completed', 'true');
    });
    const dashboard = new DashboardPage(page);
    // Own the boot wait: the real instance can hold many discussions, so the
    // shell render can exceed DashboardPage.goto's 15s on a cold first compile.
    await page.goto('/');
    await dashboard.navDiscussions.waitFor({ state: 'visible', timeout: 45_000 });
    await dashboard.navDiscussions.click();
    await page.getByRole('button', { name: new RegExp(TITLE.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) }).click();

    // 3. Attach a real PNG through the (hidden) composer file input.
    await page.locator('input[type="file"]').setInputFiles({
      name: 'capture.png',
      mimeType: 'image/png',
      buffer: PNG_1x1,
    });

    // The pending file shows in the composer before sending.
    await expect(
      page.locator('.disc-context-files').filter({ hasText: 'capture.png' }),
    ).toBeVisible({ timeout: 15_000 });

    // 4. Type + send. The real backend links the file to this message.
    const input = page.locator('.disc-composer-textarea');
    await input.fill('Reponds juste OK. Je joins une capture.');
    const send = page.locator('.disc-send-btn');
    await expect(send).toBeEnabled();
    await send.click();

    // 5. The uploaded image renders as a thumbnail inside a USER bubble — the
    //    whole upload → link-on-send → byte-route → blob chain, for real.
    // onStart (real message persisted + reload) fires after the worktree is
    // set up and the stream opens — a few seconds on DOCROMS_WEB — so give it
    // room. We do NOT wait for the agent's full reply: the attachment binds at
    // send, not at done.
    const thumb = page.locator('.disc-msg-attachments img[alt="capture.png"]');
    await expect(thumb).toBeVisible({ timeout: 60_000 });
    await expect(thumb).toHaveAttribute('src', /^blob:/);

    // The file left the composer — it belongs to the message now.
    await expect(
      page.locator('.disc-context-files').filter({ hasText: 'capture.png' }),
    ).toHaveCount(0);

    // 6. Verify the backend truly pinned it (message_id set), not just the UI.
    const filesResp = await request.get(`/api/discussions/${discId}/context-files`);
    const filesJson = await filesResp.json();
    const files: Array<{ filename: string; message_id: string | null }> = filesJson?.data ?? [];
    const attached = files.find(f => f.filename === 'capture.png');
    expect(attached, 'the file must exist').toBeTruthy();
    expect(attached?.message_id, 'the file must be pinned to a message, not pending').toBeTruthy();

    // Bound token cost: stop the agent now that the attachment is proven.
    await request.post(`/api/discussions/${discId}/stop`).catch(() => { /* best-effort */ });
  });
});
