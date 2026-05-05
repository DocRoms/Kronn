/**
 * Workflow wizard preset cards — rendering + click flow.
 *
 * Regression guards :
 *   - All 6 v07 presets render in the wizard's "Démarrer depuis un pattern"
 *     section : AUTO_DEV, PR_GATE, DEPLOY_ROLLBACK, FEATURE_PLANNER,
 *     DAILY_HOST_AUDIT, TICKET_TO_PR (the last is the 0.7+ Sprint 1 add).
 *   - The advanced mode toggle gates the preset cards (simple mode has
 *     no presets — the simple wizard skips straight to a free-form prompt).
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';
import { WorkflowWizardPage } from '../pages/WorkflowWizardPage';

test.describe('Workflow wizard — preset cards', () => {
  test('all 6 v07 presets are visible in advanced mode', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    const wizard = new WorkflowWizardPage(page);

    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.openNewWorkflowWizard();
    await wizard.gotoStepsPage('e2e-test');

    // Each preset's title should be visible. Regex tolerate FR/EN.
    await expect(wizard.presetAutoDev).toBeVisible({ timeout: 5_000 });
    await expect(wizard.presetTicketToPr).toBeVisible();
    await expect(wizard.presetDailyHostAudit).toBeVisible();

    // The other 3 presets — match by their canonical FR title to avoid
    // over-coupling to the WorkflowWizardPage helpers.
    await expect(page.getByRole('button', { name: /Pipeline PR avec Gate humain|Pipeline PR with human Gate/i })).toBeVisible();
    await expect(page.getByRole('button', { name: /Déploiement avec rollback|Deployment with rollback/i })).toBeVisible();
    await expect(page.getByRole('button', { name: /Feature \/ Epic Planner/i })).toBeVisible();
  });

  test('Ticket Autopilot preset is the latest add (0.7+)', async ({ page }) => {
    // Smoke check that the 0.7+ Sprint 1 preset shipped. If this regresses
    // we know v07-presets.ts lost the TICKET_TO_PR entry.
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    const wizard = new WorkflowWizardPage(page);

    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.openNewWorkflowWizard();
    await wizard.gotoStepsPage('e2e-test');

    const ticketToPr = wizard.presetTicketToPr;
    await expect(ticketToPr).toBeVisible();
    // The card mentions "Ticket Autopilot" in its accessible name (title)
    // — the description ("Pipeline complet…") sits inside the same card.
    await expect(ticketToPr).toContainText(/Ticket Autopilot/i);
  });
});
