/**
 * Workflow wizard — Save error banner.
 *
 * Regression guard for the 0.7+ silent-Create UX fix : when `handleSave`
 * throws (backend rejects the request), the wizard must surface the error
 * via the red banner above the Create button (instead of `console.warn`-ing
 * silently). Without this, the user clicks Create and nothing happens —
 * the original "dead button" bug we fixed in 0.7+.
 *
 * Strategy : intercept the workflow SAVE call at the network layer and
 * fulfill it with a simulated backend rejection. The frontend's
 * `handleSave` catches the thrown error and renders the banner. Since the
 * decomposed-presets work (0.8.6), a preset carrying `childWorkflows`
 * (e.g. 🎫 Ticket Autopilot) routes the save to `POST /api/workflows/bundle`
 * instead of `POST /api/workflows`, so we intercept BOTH endpoints.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';
import { WorkflowWizardPage } from '../pages/WorkflowWizardPage';

test.describe('Wizard — save error banner', () => {
  test('backend reject surfaces the error banner', async ({ page }) => {
    // Intercept the workflow create endpoint and simulate a backend
    // validation rejection. Done BEFORE goto so the route is registered
    // when the request fires. Matches both the flat create endpoint
    // (`/api/workflows`) and the decomposed-preset bundle endpoint
    // (`/api/workflows/bundle`) — Ticket Autopilot now saves via the latter.
    await page.route(/\/api\/workflows(\/bundle)?(\?.*)?$/, async (route, request) => {
      if (request.method() === 'POST') {
        await route.fulfill({
          status: 400,
          contentType: 'application/json',
          body: JSON.stringify({
            success: false,
            data: null,
            error: 'Workflow validation failed: simulated for E2E test',
          }),
        });
        return;
      }
      await route.continue();
    });

    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    const wizard = new WorkflowWizardPage(page);

    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.openNewWorkflowWizard();

    // 0.8.5 — apply Ticket Autopilot via the unified QuickStart picker on
    // step 0. Gives us a complete valid workflow shape; the 400 we inject
    // targets the SAVE call, not validation, so we don't care about
    // pre-save predicate logic here. The wizard auto-jumps to step 2
    // (Steps) after applying.
    await wizard.applyQuickStart('e2e-save-error', /🎫\s*Ticket Autopilot/i);

    // Steps → Config → Résumé.
    await wizard.nextButton.click();
    await wizard.nextButton.click();

    // Click Create — the route handler answers 400 → handleSave catches
    // → setSaveError → red banner appears.
    await wizard.createButton.click();

    // The banner should now be visible with the simulated error message.
    await expect(wizard.saveErrorBanner).toBeVisible({ timeout: 5_000 });
    await expect(wizard.saveErrorBanner).toContainText(
      /Workflow validation failed: simulated for E2E test/
    );
  });
});
