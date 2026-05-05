/**
 * Workflow wizard — step type buttons coloring (regression: 0.6.0+).
 *
 * In 0.6.0 only Agent / ApiCall / Gate / Exec had a "selected" color
 * variant on their step type button. Sprint 1.5 added selected styling
 * for BatchQuickPrompt / BatchApiCall / Notify / JsonData. This spec
 * locks the convention: every step type button gets a `data-selected`
 * attribute that flips to "true" when the user picks it.
 *
 * Locked here so a future refactor that drops the attribute (say, switch
 * to React state-only styling) catches our regression net.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';
import { WorkflowWizardPage } from '../pages/WorkflowWizardPage';

test.describe('Workflow wizard — step type buttons', () => {
  test('every step type button exposes data-selected for styling', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    const wizard = new WorkflowWizardPage(page);

    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.openNewWorkflowWizard();
    await wizard.gotoStepsPage('e2e-step-types');

    // The Steps page renders one default step. Its step type bar carries
    // 8 buttons (one per StepType variant). All MUST have data-selected.
    const stepTypeBar = page.locator('.wf-step-type-btn').first();
    await expect(stepTypeBar).toBeVisible({ timeout: 5_000 });

    const expectedTypes = [
      'agent', 'api', 'batch-qp', 'notify',
      'gate', 'exec', 'batch-api', 'json-data',
    ];
    for (const dataType of expectedTypes) {
      const btn = page.locator(`.wf-step-type-btn[data-type="${dataType}"]`).first();
      await expect(btn, `step type "${dataType}" should be present`).toBeVisible();
      // data-selected must exist (true OR false — we just check it's set,
      // styling regression would be losing the attribute entirely).
      const sel = await btn.getAttribute('data-selected');
      expect(sel, `step type "${dataType}" missing data-selected`).not.toBeNull();
    }
  });

  test('clicking JsonData step type marks it selected', async ({ page }) => {
    // 0.7+ — JsonData was added with a cyan selected variant (Sprint 1.5).
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    const wizard = new WorkflowWizardPage(page);

    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.openNewWorkflowWizard();
    await wizard.gotoStepsPage('e2e-jsondata');

    const jsonDataBtn = page.locator('.wf-step-type-btn[data-type="json-data"]').first();
    await jsonDataBtn.click();
    await expect(jsonDataBtn).toHaveAttribute('data-selected', 'true');
  });
});
