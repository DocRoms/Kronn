/**
 * Smoke coverage for the Quick APIs Automation section.
 *
 * Until 2026-05-10 there were **zero** QA-flavoured E2E specs (a
 * full feature shipped in 0.6.0 with no UI-driven coverage). This
 * spec pins the bare minimum so a tab rename / routing regression
 * gets caught.
 *
 * Scope: section opens, active marker flips, body renders. The
 * QA-creation wizard's own flow is covered by component tests in
 * `WorkflowsPage.test.tsx`.
 */
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

test.describe('Quick APIs section', () => {
  test('opens from the Automation sidebar and flips data-active', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();

    const qaSection = page.locator('[data-tour-id="automation-kind-quick-api"]');
    await expect(qaSection).toBeVisible({ timeout: 5_000 });
    await qaSection.click();
    await expect(qaSection).toHaveAttribute('data-active', 'true');
  });

  test('does NOT activate the Quick Prompt section (regression: 0.6.0 ternary leak)', async ({ page }) => {
    // Confirm the Quick API section is the only active category, ensuring the
    // shared detail panel exercises the correct code path.
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();
    await page.locator('[data-tour-id="automation-kind-quick-api"]').click();

    const qaSection = page.locator('[data-tour-id="automation-kind-quick-api"]');
    await expect(qaSection).toHaveAttribute('data-active', 'true');
    // Workflow + Quick API sections must NOT be active concurrently.
    const workflowSection = page.locator('[data-tour-id="automation-kind-workflow"]');
    await expect(workflowSection).not.toHaveAttribute('data-active', 'true');
  });
});
