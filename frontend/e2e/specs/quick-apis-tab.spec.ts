/**
 * Smoke coverage for the Quick APIs Automation section.
 *
 * Until 2026-05-10 there were **zero** QA-flavoured E2E specs (a
 * full feature shipped in 0.6.0 with no UI-driven coverage). This
 * spec pins the bare minimum so a tab rename / routing regression
 * gets caught.
 *
 * Scope: section opens, active marker flips, body renders. The
 * QA-creation wizard's own flow is covered by component tests in
 * `WorkflowsPage.test.tsx`.
 */
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';

test.describe('Quick APIs section', () => {
  test('opens from the Automation sidebar and flips data-active', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();

    // The type chip rests on "all"; choosing Quick APIs flips it to active.
    await expect(workflows.kindChip).toBeVisible({ timeout: 5_000 });
    await expect(workflows.kindChip).toHaveAttribute('data-active', 'false');
    await workflows.selectKind('quickApis');
    await expect(workflows.kindChip).toHaveAttribute('data-value', 'quickApis');
    await expect(workflows.kindChip).toHaveAttribute('data-active', 'true');
  });

  test('does NOT activate the Quick Prompt section (regression: 0.6.0 ternary leak)', async ({ page }) => {
    // Confirm the Quick API section is the only active category, ensuring the
    // shared detail panel exercises the correct code path.
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.selectKind('quickApis');

    await expect(workflows.kindChip).toHaveAttribute('data-value', 'quickApis');
    // Reopen the listbox: Quick APIs is the one selected type, so neither the
    // Workflow nor the Quick Prompt type may be selected alongside it.
    await workflows.openKindMenu();
    await expect(workflows.kindOption('quickApis')).toHaveAttribute('aria-selected', 'true');
    await expect(workflows.kindOption('workflows')).toHaveAttribute('aria-selected', 'false');
    await expect(workflows.kindOption('quickPrompts')).toHaveAttribute('aria-selected', 'false');
  });
});
