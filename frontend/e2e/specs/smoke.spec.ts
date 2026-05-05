/**
 * Smoke tests — minimum viable Playwright spec.
 *
 * Refactored on Sprint 1.5 J2 to use the kronn-fixture (auto-stubs + tour
 * skip) and page objects. Each spec stays declarative; the boilerplate
 * lives in `fixtures/` and `pages/`.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';

test.describe('Kronn smoke', () => {
  test('app loads and renders the dashboard nav', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await expect(dashboard.navWorkflows).toBeVisible();
    await expect(dashboard.navDiscussions).toBeVisible();
    await expect(dashboard.navProjects).toBeVisible();
    await expect(dashboard.navSettings).toBeVisible();
  });

  test('navigates to Automatisation page', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();
    await expect(workflows.tabQuickPrompts).toBeVisible({ timeout: 5_000 });
    await expect(workflows.tabQuickApis).toBeVisible();
  });

  test('Quick APIs tab shows its own create button (regression: 0.6.0)', async ({ page }) => {
    // Regression guard: in 0.6.0 the Quick APIs tab showed the Quick Prompts
    // header buttons (ternary leak). Fixed by switching the header to a
    // 3-way conditional.
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.clickQuickApisTab();
    await expect(workflows.header).not.toContainText(/Nouveau prompt/i);
    await expect(workflows.header).not.toContainText(/Créer prompt/i);
  });
});
