/**
 * Workflow wizard — Save error banner.
 *
 * Regression guard for the 0.7+ silent-Create UX fix : when `handleSave`
 * throws (backend rejects the request), the wizard must surface the error
 * via the red banner above the Create button (instead of `console.warn`-ing
 * silently). Without this, the user clicks Create and nothing happens —
 * the original "dead button" bug we fixed in 0.7+.
 *
 * Strategy : intercept `POST /api/workflows` at the network layer and
 * fulfill it with a simulated backend rejection. The frontend's
 * `handleSave` catches the thrown error and renders the banner.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';
import { WorkflowWizardPage } from '../pages/WorkflowWizardPage';

test.describe('Wizard — save error banner', () => {
  test('backend reject surfaces the error banner', async ({ page }) => {
    // Intercept the workflow create endpoint and simulate a backend
    // validation rejection. Done BEFORE goto so the route is registered
    // when the request fires.
    await page.route('**/api/workflows', async (route, request) => {
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
    await wizard.gotoStepsPage('e2e-save-error');

    // Apply Ticket Autopilot to have a complete valid workflow shape.
    // The 400 we inject targets the SAVE call, not validation, so we
    // don't care about pre-save predicate logic here.
    await wizard.presetTicketToPr.click();

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
