/**
 * Workflow wizard — Infos step — name validation.
 *
 * Regression guard (memory: feedback_ux_regressions): the "Suivant" button
 * on the Infos step must be DISABLED when the workflow name is empty.
 * Without this guard the user advances to the Steps page, builds steps,
 * tries to save, and only then discovers the name is missing — wasted
 * 5 minutes of work.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';
import { WorkflowWizardPage } from '../pages/WorkflowWizardPage';

test.describe('Workflow wizard — name input validation', () => {
  test('Suivant disabled with empty name, enabled once filled', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    const wizard = new WorkflowWizardPage(page);

    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.openNewWorkflowWizard();

    // On the Infos page, with empty name, Next must be disabled.
    await expect(wizard.nextButton).toBeDisabled();

    // Fill the name → Next becomes enabled.
    await wizard.nameInput.fill('e2e-name-test');
    await expect(wizard.nextButton).toBeEnabled();

    // Clearing it again disables Next.
    await wizard.nameInput.fill('');
    await expect(wizard.nextButton).toBeDisabled();
  });
});
