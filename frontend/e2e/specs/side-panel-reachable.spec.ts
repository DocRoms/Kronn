/**
 * KT-1884 — the discussion side panel shows ONE header above its tabs, and
 * at every width, in every one of its tabs, the rail, the « ⋯ » menu button,
 * the entries of the open menu and the close button stay reachable.
 *
 * Found on 2026-11-21 in a 400px window: the Files view (`position: absolute;
 * inset: 0` under 768px) was drawn over the whole rail and the menu button,
 * and the menu entries opened under a view's own header. "Reachable" is read
 * the way a click lands: the element at the centre of the control is the
 * control (or inside it).
 */

import { type Locator, type Page } from '@playwright/test';
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

const stamp = Date.now();
const createdDiscussions: string[] = [];

test.afterAll(async ({ request }) => {
  for (const id of createdDiscussions) await request.delete(`/api/discussions/${id}`).catch(() => {});
});

async function pin(page: Page, theme: 'light' | 'dark') {
  await page.addInitScript(name => {
    try {
      window.localStorage.setItem('kronn:tour-completed', 'true');
      window.localStorage.setItem('kronn:theme', name);
    } catch { /* storage disabled: the panel still opens */ }
  }, theme);
  await page.routeWebSocket(/\/api\/ws/, () => { /* never connected to the server */ });
}

/** What a click at the centre of `control` would land on is `control` itself. */
async function expectReachable(control: Locator, what: string) {
  await expect(control, what).toBeVisible();
  const hit = await control.evaluate(node => {
    const box = node.getBoundingClientRect();
    const top = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2);
    return top === node || node.contains(top)
      ? null
      : `${top?.tagName.toLowerCase()}.${(top?.className || '').toString().split(' ').join('.')}`;
  });
  expect(hit, `${what} is covered by ${hit}`).toBeNull();
}

for (const { width, height, theme } of [
  { width: 400, height: 760, theme: 'light' as const },
  { width: 400, height: 760, theme: 'dark' as const },
  { width: 1280, height: 800, theme: 'light' as const },
]) {
  test(`side panel at ${width}px (${theme}): one header, rail, menu and close reachable in every tab`, async ({ request, page }) => {
    await pin(page, theme);
    await page.setViewportSize({ width, height });
    const created = await request.post('/api/discussions', {
      data: { title: `Reachable panel PW ${stamp} ${width} ${theme}`, agent: 'ClaudeCode', language: 'fr', initial_prompt: 'Aucun modèle ne tourne.' },
    });
    expect(created.ok()).toBe(true);
    const discId: string = (await created.json()).data.id;
    createdDiscussions.push(discId);

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.openDiscussion(discId);
    await page.getByTestId('disc-header-details-btn').click();

    const panel = page.getByTestId('panel-switcher');
    await expect(panel).toBeVisible();
    const tabs = panel.locator('.side-panel-shell-rail [role="tab"]');
    const count = await tabs.count();
    expect(count, 'the rail lists the views').toBeGreaterThanOrEqual(5);

    for (let i = 0; i < count; i++) {
      const tab = tabs.nth(i);
      const name = (await tab.getAttribute('data-panel')) ?? String(i);
      await tab.click();
      await expect(tab).toHaveAttribute('aria-selected', 'true');
      // Let a view that loads settle into its final layout.
      await page.waitForTimeout(300);

      await expect(panel.locator('.ui-header'), `${name}: one header`).toHaveCount(1);
      await expect(
        panel.locator('.disc-tool-panel-header, .disc-assets-panel-header, .disc-notes-panel-head'),
        `${name}: no header of its own`,
      ).toHaveCount(0);

      for (let j = 0; j < count; j++) await expectReachable(tabs.nth(j), `${name}: rail tab ${j}`);
      const close = page.getByTestId('panel-open-toggle');
      await expectReachable(close, `${name}: close button`);
      const menuButton = page.getByTestId('side-panel-menu-btn');
      await expectReachable(menuButton, `${name}: menu button`);

      await menuButton.click();
      const entries = page.getByRole('menu').getByRole('menuitem');
      await expect(entries.first()).toBeVisible();
      const entryCount = await entries.count();
      expect(entryCount, `${name}: export, copy the id, delete`).toBe(3);
      for (let k = 0; k < entryCount; k++) await expectReachable(entries.nth(k), `${name}: menu entry ${k}`);
      await page.keyboard.press('Escape');
      await expect(page.getByRole('menu')).toHaveCount(0);
      await expect(panel, `${name}: Escape closes the menu, not the panel`).toBeVisible();

      if (width === 400) {
        await page.screenshot({ path: `test-results/kt1884-side-panel-${name}-${width}-${theme}.png` });
      }
    }

    await page.getByTestId('panel-open-toggle').click();
    await expect(panel).toHaveCount(0);
  });
}
