/**
 * REAL backend — an image attached in the CREATION POPUP (NewDiscussionForm)
 * renders on the FIRST message (0.8.8 bug fix).
 *
 * Bug: the popup uploads files AFTER the first message (initial_prompt) is
 * created, then runs the agent via `run_agent` — which never links pending
 * files. So a popup attachment stayed pending and got vacuumed into message #2
 * on the next send, never appearing on the message it was attached to.
 *
 * Fix: after the popup upload, the frontend links the pending files to the
 * first message via POST …/context-files/link-pending. This test drives the
 * real popup and asserts the thumbnail lands on the first bubble.
 *
 * Cheap + deterministic: we UNCHECK "launch agent now" (disc-first mode), so
 * NO CLI runs and NO tokens are spent — the linking happens before the
 * run-agent branch, so the fix is exercised either way. Real backend only
 * (create + upload + link + byte route), so it's CI-skipped like the other
 * real-backend specs; the hermetic render spec covers CI.
 */
import { test, expect, type APIRequestContext } from '@playwright/test';
import { DashboardPage } from '../pages/DashboardPage';

test.describe.configure({ timeout: 120_000, retries: 0 });
test.skip(!!process.env.CI, 'real-backend spec — local-only (needs the live backend on :3140)');

const TITLE = `Popup Attach E2E ${Date.now()}`;
const PROMPT = 'tu vois cette image jointe au tout premier message ?';
const PNG_1x1 = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==',
  'base64',
);

async function findDiscIdByTitle(request: APIRequestContext, title: string): Promise<string | null> {
  const r = await request.get('/api/discussions');
  if (!r.ok()) return null;
  const j = await r.json();
  const discs: Array<{ id: string; title: string }> = j?.data ?? [];
  return discs.find(d => d.title === title)?.id ?? null;
}

test.describe('Creation popup — attachment lands on the first message', () => {
  test.afterAll(async ({ request }) => {
    const id = await findDiscIdByTitle(request, TITLE);
    if (id) await request.delete(`/api/discussions/${id}`).catch(() => { /* idempotent */ });
  });

  test('attach PNG in popup + create (no agent) → thumbnail on the first bubble', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('kronn:tour-completed', 'true'));
    const dashboard = new DashboardPage(page);
    // Own the boot wait: the real instance can hold many discussions, so the
    // shell render can exceed DashboardPage.goto's 15s on a cold first compile.
    await page.goto('/');
    await dashboard.navDiscussions.waitFor({ state: 'visible', timeout: 45_000 });
    await dashboard.navDiscussions.click();

    // Open the creation popup.
    await page.locator('[data-tour-id="new-disc-btn"]').click();
    const dialog = page.locator('.disc-new-card');
    await expect(dialog).toBeVisible();

    // Title (so we can find+clean up the disc) + prompt (becomes message #1).
    // The title field is optional in the form; fill prompt which is required.
    await dialog.locator('.disc-textarea-styled').fill(PROMPT);

    // Attach the image through the popup's own file input.
    await dialog.locator('input[type="file"]').setInputFiles({
      name: 'firstshot.png',
      mimeType: 'image/png',
      buffer: PNG_1x1,
    });
    await expect(dialog.locator('.disc-context-file-badge').filter({ hasText: 'firstshot.png' }))
      .toBeVisible({ timeout: 10_000 });

    // Disc-first mode: uncheck "launch agent now" so no CLI runs (cheap).
    const launchToggle = dialog.locator('input[type="checkbox"]').first();
    if (await launchToggle.isChecked()) await launchToggle.uncheck();

    // Set a unique title (placeholder "Auto-generated from prompt…" across
    // locales) so afterAll can find + delete the disc.
    await dialog.getByPlaceholder(/Auto-g/).fill(TITLE);

    await dialog.locator('.disc-create-btn').click();

    // The disc opens with the first message visible.
    await expect(
      page.locator('.disc-msg-bubble').filter({ hasText: PROMPT }),
    ).toBeVisible({ timeout: 15_000 });

    // The popup attachment is pinned to THAT first message — thumbnail renders.
    const thumb = page.locator('.disc-msg-attachments img[alt="firstshot.png"]');
    await expect(thumb).toBeVisible({ timeout: 15_000 });
    await expect(thumb).toHaveAttribute('src', /^blob:/);
  });
});
