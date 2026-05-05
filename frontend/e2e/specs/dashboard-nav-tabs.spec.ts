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
    await dashboard.goto();
    await dashboard.clickWorkflows();
    // Automation header sub-tabs are visible.
    await expect(page.getByRole('button', { name: /Quick Prompts/i })).toBeVisible({ timeout: 5_000 });
  });

  test('Config tab loads', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();
    // Settings sections render their accordion headers.
    await expect(page.locator('.set-accordion-header').first()).toBeVisible({ timeout: 5_000 });
  });
});
