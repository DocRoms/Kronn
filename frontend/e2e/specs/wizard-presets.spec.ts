/**
 * Workflow wizard preset cards — rendering + click flow.
 *
 * Regression guards :
 *   - All 6 v07 presets render in the unified QuickStart picker (0.8.5+) :
 *     AUTO_DEV, PR_GATE, DEPLOY_ROLLBACK, FEATURE_PLANNER,
 *     DAILY_HOST_AUDIT, TICKET_TO_PR (the last is the 0.7+ Sprint 1 add).
 *   - The picker is gated by the workflow name (disabled when empty) so
 *     `openQuickStartPicker(name)` is the canonical entry path.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';
import { WorkflowWizardPage } from '../pages/WorkflowWizardPage';

test.describe('Workflow wizard — preset cards', () => {
  test('all 6 v07 presets are visible in the QuickStart picker', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    const wizard = new WorkflowWizardPage(page);

    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.openNewWorkflowWizard();
    // 0.8.5 — picker lives on step 0 (Infos) now. Open it after filling
    // the name (the toggle is disabled until then).
    await wizard.openQuickStartPicker('e2e-test');

    // Each preset's title should be visible in the picker. Regex tolerate
    // FR/EN. The page object's preset getters return the `<li>` row whose
    // title `<span>` matches the regex.
    await expect(wizard.presetAutoDev).toBeVisible({ timeout: 5_000 });
    await expect(wizard.presetTicketToPr).toBeVisible();
    await expect(wizard.presetDailyHostAudit).toBeVisible();

    // The other 3 presets — match by their canonical FR title via
    // `page.getByText` (the title sits in a `<span>`, not a button).
    await expect(page.getByText(/Pipeline PR avec Gate humain|Pipeline PR with human Gate/i)).toBeVisible();
    await expect(page.getByText(/Déploiement avec rollback|Deployment with rollback/i)).toBeVisible();
    await expect(page.getByText(/Feature \/ Epic Planner/i)).toBeVisible();
  });

  test('Ticket Autopilot preset is the latest add (0.7+)', async ({ page }) => {
    // Smoke check that the 0.7+ Sprint 1 preset shipped. If this regresses
    // we know v07-presets.ts lost the TICKET_TO_PR entry OR the picker
    // adapter dropped the preset source.
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    const wizard = new WorkflowWizardPage(page);

    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.openNewWorkflowWizard();
    await wizard.openQuickStartPicker('e2e-test');

    const ticketToPr = wizard.presetTicketToPr;
    await expect(ticketToPr).toBeVisible();
    // The row contains "🎫 Ticket Autopilot" in its title.
    await expect(ticketToPr).toContainText(/Ticket Autopilot/i);
  });
});
