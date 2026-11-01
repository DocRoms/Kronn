/**
 * KT-1530 — the discussion and Artifact headers carry the same pair of buttons
 * and keep their copyable id next to the title.
 *
 *   discussion: « Inviter » (green) + « Détails » (panel icon)
 *   Artifact:   « Synchroniser » (green) + « Détails » (panel icon)
 *
 * The id is one click away at all times (people paste it to an agent), so it is
 * asserted while NO panel is open. « Détails » opens the vertical side panel;
 * on an Artifact its « Détails » tab holds the metadata the third row used to
 * fold. Light and dark captures land in `test-results/` for the review.
 */

import { type Locator, type Page } from '@playwright/test';
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

const stamp = Date.now();
const createdDiscussions: string[] = [];
const createdPages: string[] = [];

test.afterAll(async ({ request }) => {
  for (const id of createdDiscussions) await request.delete(`/api/discussions/${id}`).catch(() => {});
  for (const id of createdPages) await request.delete(`/api/pages/${id}`).catch(() => {});
});

async function pin(page: Page, theme: 'light' | 'dark') {
  await page.addInitScript(name => {
    try {
      window.localStorage.setItem('kronn:tour-completed', 'true');
      window.localStorage.setItem('kronn:theme', name);
    } catch { /* storage disabled: the theme assertion below fails loudly */ }
  }, theme);
  await page.routeWebSocket(/\/api\/ws/, () => { /* never connected to the server */ });
}

/** Same box, same row: the two buttons read as halves of one control. */
async function expectPair(primary: Locator, details: Locator) {
  const a = await primary.boundingBox();
  const b = await details.boundingBox();
  if (!a || !b) throw new Error('the primary action and « Détails » must both be on screen');
  expect(Math.abs(a.height - b.height), 'same height').toBeLessThanOrEqual(1);
  expect(Math.abs(a.y - b.y), 'same row').toBeLessThanOrEqual(1);
  expect(a.x + a.width, 'Détails sits to the right of the primary action').toBeLessThanOrEqual(b.x + 1);
  expect(a.height, 'not the old small toggle').toBeGreaterThanOrEqual(28);
}

/** « Détails » is the rightmost control, at the header's own right padding — the same edge on both pages. */
async function expectFlushRight(header: Locator, details: Locator) {
  const h = await header.boundingBox();
  const d = await details.boundingBox();
  if (!h || !d) throw new Error('the header and « Détails » must both be on screen');
  const padding = await header.evaluate(el => parseFloat(getComputedStyle(el).paddingRight));
  expect(Math.abs(h.x + h.width - padding - (d.x + d.width)), 'Détails sits on the header right padding edge').toBeLessThanOrEqual(1);
}

for (const theme of ['light', 'dark'] as const) {
  test.describe(`detail headers (${theme})`, () => {
    test.beforeEach(async ({ page }) => { await pin(page, theme); });

    test('discussion: id next to the title, Inviter + Détails side by side', async ({ request, page }) => {
      const created = await request.post('/api/discussions', {
        data: { title: `Header pair PW ${stamp} ${theme}`, agent: 'ClaudeCode', language: 'fr', initial_prompt: 'Aucun modèle ne tourne.' },
      });
      expect(created.ok()).toBe(true);
      const discId: string = (await created.json()).data.id;
      createdDiscussions.push(discId);

      const dashboard = new DashboardPage(page);
      await dashboard.goto();
      await dashboard.openDiscussion(discId);
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);

      const header = page.locator('[data-tour-id="disc-header-controls"]');
      const title = header.locator('.disc-chat-header-title');
      const pill = title.locator('[data-tour-id="discussion-id-pill"]');
      await expect(pill, 'the id is visible with no panel open').toBeVisible();
      await expect(pill).toContainText(`#${discId.slice(0, 8)}`);
      await expect(page.getByRole('group', { name: 'Panneaux de la discussion' })).toHaveCount(0);

      const invite = header.locator('.disc-participants-invite-btn');
      const details = page.getByTestId('disc-header-details-btn');
      await expectPair(invite, details);
      await expectFlushRight(header, details);
      await expect(header.locator('.disc-chat-header-details')).toHaveCount(0);

      await page.screenshot({ path: `test-results/kt1530-discussion-${theme}.png` });

      await details.click();
      await expect(details).toHaveAttribute('aria-expanded', 'true');
      await expect(page.locator('#disc-settings-panel')).toBeVisible();
      // Still there, and no longer duplicated inside the panel.
      await expect(page.locator('[data-tour-id="discussion-id-pill"]')).toHaveCount(1);
    });

    test('Artifact: id next to the title, Synchroniser + Détails, metadata in the Détails tab', async ({ request, page }) => {
      const created = await request.post('/api/pages', {
        data: { title: `Header pair Artifact PW ${stamp} ${theme}`, html: '<h1>KT-1530</h1>' },
      });
      expect(created.ok()).toBe(true);
      const pageId: string = (await created.json()).data.id;
      createdPages.push(pageId);

      await page.goto(`/pages/${encodeURIComponent(pageId)}`);
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);

      const header = page.locator('.live-pages-viewer-header');
      const pill = header.locator('.disc-chat-header-title .copy-id-pill');
      await expect(pill, 'the id is visible with no panel open').toBeVisible();
      await expect(pill).toContainText(`#${pageId.slice(0, 8)}`);
      await expect(page.locator('#live-page-side-panel')).toHaveCount(0);

      const sync = page.getByTestId('live-page-refresh-menu');
      const details = page.getByTestId('live-page-header-details-toggle');
      await expectPair(sync, details);
      await expectFlushRight(header, details);
      // The freshness is visible text right before « Synchroniser », not only a tooltip.
      const fresh = page.getByTestId('live-page-sync-freshness');
      await expect(fresh).toBeVisible();
      const f = await fresh.boundingBox();
      const sb = await sync.boundingBox();
      if (!f || !sb) throw new Error('freshness and sync must be on screen');
      expect(f.x + f.width, 'freshness sits left of Synchroniser').toBeLessThanOrEqual(sb.x + 1);
      // No third header row.
      await expect(header.locator('.disc-chat-header-details')).toHaveCount(0);

      await page.screenshot({ path: `test-results/kt1530-artifact-${theme}.png` });

      await details.click();
      const panel = page.locator('#live-page-side-panel');
      await expect(panel).toBeVisible();
      await expect(page.getByTestId('live-page-panel-tab-details')).toHaveAttribute('aria-selected', 'true');
      await expect(panel).toContainText(/data r\d+ · HTML r\d+/);
      await expect(panel.locator('.live-page-slug-pill')).toBeVisible();
      await page.screenshot({ path: `test-results/kt1530-artifact-panel-${theme}.png` });

      await sync.click();
      await expect(page.getByTestId('live-page-sync-workflows')).toBeVisible();
    });
  });
}
