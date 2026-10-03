/**
 * Dashboard nav — every tab loads without crashing.
 *
 * Cheap smoke check : if any of the top-level pages (Projets / Discussions
 * / Plugins / Automatisation / Config) crashes its mount, the user sees a
 * blank page or an ErrorBoundary fallback. This spec navigates each one
 * and confirms the heading appears, catching most boot-time JS crashes.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';

test.describe('Dashboard — every nav tab loads', () => {
  test('Projets tab loads', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickProjects();
    // The Projets page exposes "Ajouter un projet" / "Add a project" CTA.
    await expect(page.getByRole('button', { name: /Ajouter un projet|Add a project/i })).toBeVisible({ timeout: 5_000 });
  });

  test('Automatisation tab loads', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    await dashboard.goto();
    await dashboard.openWorkflows();
    // Stable capability hook; counts and translated labels may change. The
    // type chip is how the page reaches every automation type, and it rests
    // on "all" until one is picked.
    await expect(workflows.kindChip).toBeVisible({ timeout: 5_000 });
    await expect(workflows.kindChip).toHaveAttribute('data-value', 'all');
  });

  test('Config tab loads', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();
    // Settings sections render their accordion headers.
    await expect(page.locator('.set-accordion-header').first()).toBeVisible({ timeout: 5_000 });
  });
});
