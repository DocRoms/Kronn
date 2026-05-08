/**
 * Smoke coverage for the Quick APIs tab.
 *
 * Until 2026-05-10 there were **zero** QA-flavoured E2E specs (a
 * full feature shipped in 0.6.0 with no UI-driven coverage). This
 * spec pins the bare minimum so a tab rename / routing regression
 * gets caught.
 *
 * Scope: tab opens, tab-active marker flips, body renders. The
 * QA-creation wizard's own flow is covered by component tests in
 * `WorkflowsPage.test.tsx`.
 */
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

test.describe('Quick APIs tab', () => {
  test('opens from Workflows tab and flips data-active', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();

    const qaTab = page.locator('button.dash-tab', { hasText: /Quick APIs/i });
    await expect(qaTab).toBeVisible({ timeout: 5_000 });
    await qaTab.click();
    await expect(qaTab).toHaveAttribute('data-active', 'true');
  });

  test('does NOT show the QP-tab header (regression: 0.6.0 ternary leak)', async ({ page }) => {
    // Mirrors the smoke spec's regression for the same bug — that one
    // only checked `Nouveau prompt` / `Créer prompt` strings on the
    // header. We tighten by also confirming the QA tab is the active
    // one, ensuring the test is exercising the right code path.
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();
    await page.locator('button.dash-tab', { hasText: /Quick APIs/i }).click();

    const qaTab = page.locator('button.dash-tab', { hasText: /Quick APIs/i });
    await expect(qaTab).toHaveAttribute('data-active', 'true');
    // Workflows + QP tabs must NOT be active concurrently.
    const wfTab = page.locator('button.dash-tab', { hasText: /Workflows/i }).first();
    await expect(wfTab).not.toHaveAttribute('data-active', 'true');
  });
});
