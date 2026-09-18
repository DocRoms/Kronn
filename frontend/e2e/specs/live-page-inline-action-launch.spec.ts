/**
 * Live Page inline Kronn action — launch + secure discussion deep link (E2E).
 *
 * Drives the REAL sandboxed-iframe protocol production Live Pages use:
 * `POST /api/pages` seeds a Page whose HTML contains a
 * `data-kronn-action="..."` button plus its matching
 * `<script type="application/kronn-action">` declaration block — the exact
 * shape `backend/src/db/live_page_actions.rs::extract_page_action_blocks`
 * parses. Clicking that button inside the sandboxed iframe posts a real
 * `kronn:page-action` intent (`frontend/src/lib/live-page-sandbox.ts`) that
 * the standalone page's shared `useLivePageActions` hook picks up and
 * renders as a native `LivePageActionCard` OUTSIDE the iframe.
 *
 * The target is a Quick Prompt: launching one creates its result discussion
 * (`qp_run` in `backend/src/api/mcp_remote.rs`) and queues the agent's turn.
 * The launch follows that turn to its end: `succeeded` once an agent has
 * answered, `failed` when none can — which is what an isolated E2E backend,
 * with no agent installed, reports within a second. Either way the result
 * discussion exists and must be reachable from the card.
 *
 * The terminal "open discussion" button exercises
 * `openStandaloneDiscussion` (`frontend/src/lib/live-page-navigation.ts`):
 * a deterministic same-origin `#discussion-<id>` URL opened with
 * `noopener,noreferrer`. This spec proves both halves live in a real
 * browser — the new tab lands on the right discussion, AND it never
 * receives a `window.opener` back-reference — a guarantee Vitest/happy-dom
 * cannot simulate (see memory: cross-window storage/opener behavior needs a
 * real browser).
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

test.describe('Live Page inline action — launch and secure discussion deep link', () => {
  test('launches a Quick Prompt action from a real Live Page and opens its result discussion without an opener back-reference', async ({ page }) => {
    const qpResponse = await page.request.post('/api/quick-prompts', {
      data: {
        name: `E2E frame ${Date.now()}`,
        prompt_template: 'Frame this issue.',
        variables: [],
      },
    });
    expect(qpResponse.ok()).toBe(true);
    const qp = (await qpResponse.json()) as { data: { id: string } };
    const targetId = qp.data.id;

    const html = '<button data-kronn-action="frame">Frame</button>'
      + '<script type="application/kronn-action" data-action-id="frame">'
      + `{"kind":"quick_prompt","target_id":"${targetId}"}`
      + '</script>';
    const pageResponse = await page.request.post('/api/pages', {
      data: { title: `E2E Live Page ${Date.now()}`, html, datasets: [] },
    });
    expect(pageResponse.ok()).toBe(true);
    const created = (await pageResponse.json()) as { data: { id: string } };
    const pageId = created.data.id;

    await page.goto(`/#page/${pageId}`);
    const frame = page.frameLocator('[data-testid="standalone-live-page-frame"]');
    await frame.locator('[data-kronn-action="frame"]').click();

    const card = page.locator('[data-testid^="live-page-action-"]');
    await expect(card).toBeVisible();
    await card.locator('.discussion-action-card__launch').click();
    await expect(card).toHaveAttribute('data-state', /^(succeeded|failed)$/, { timeout: 10_000 });

    const popupPromise = page.waitForEvent('popup');
    // The result discussion is reached from the card's outcome, which names it.
    await card.getByTestId('run-outcome-open').first().click();
    const popup = await popupPromise;
    await popup.waitForLoadState();

    expect(popup.url()).toMatch(/#discussion-.+/);
    const hasOpener = await popup.evaluate(() => window.opener !== null);
    expect(hasOpener, 'the new tab must never receive a window.opener back-reference').toBe(false);

    const popupDashboard = new DashboardPage(popup);
    await popupDashboard.navDiscussions.waitFor({ state: 'visible', timeout: 15_000 });
    await popup.close();
  });
});
