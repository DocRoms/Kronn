/**
 * Workflow wizard — Create button validation predicate.
 *
 * Regression guard for the 0.7+ silent-Create bug : the `disabled`
 * predicate must accept the new step types (JsonData / quick_api_id /
 * quick_prompt_id) without falling through to the default
 * `!s.prompt_template => disabled` branch.
 *
 * Test path : apply the Ticket Autopilot preset (whose first step is a
 * `JsonData` with a hardcoded payload — the predicate must NOT disable
 * the Create button on that ground), navigate to the Résumé step, and
 * assert the Create button is enabled.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';
import { WorkflowWizardPage } from '../pages/WorkflowWizardPage';

test.describe('Wizard — Create button validation', () => {
  test('Ticket Autopilot preset enables Create on the summary step', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    const wizard = new WorkflowWizardPage(page);

    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.openNewWorkflowWizard();

    // 0.8.5 — applies the Ticket Autopilot preset via the unified
    // QuickStartPicker on step 0 (Infos). The wizard auto-jumps to
    // advanced step 2 (Steps) after applying, matching the pre-0.8.5
    // flow. The preset's first step is JsonData with a
    // `json_data_payload` — the predicate would wrongly disable Create
    // if it fell back to the `prompt_template` branch.
    await wizard.applyQuickStart('e2e-create-validation', /🎫\s*Ticket Autopilot/i);

    // Walk to the last step (Résumé) where the Create button lives :
    //   Steps → Config → Résumé
    await wizard.nextButton.click(); // Steps → Config
    await wizard.nextButton.click(); // Config → Résumé

    // The Create button should be ENABLED. If we regress on the
    // predicate, this is where the user lands and stares at a grey
    // unclickable button.
    await expect(wizard.createButton).toBeVisible({ timeout: 5_000 });
    await expect(wizard.createButton).toBeEnabled();
  });
});
