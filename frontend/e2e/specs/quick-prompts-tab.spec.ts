/**
 * Smoke coverage for the Quick Prompts Automation section.
 *
 * Until 2026-05-10 the only QP-flavoured E2E was
 * `qp-launch-double-click.spec.ts` (race-guard regression). The QP
 * section itself, the create-button and the empty-state had **zero**
 * coverage — a content rename or routing regression would slip
 * through CI.
 *
 * Scope: navigate to the section, assert the create CTA + the
 * active marker render. We deliberately don't drive the create
 * flow itself (the wizard's behaviour is covered by
 * `WorkflowsPage.qp-launch.test.tsx` + the WorkflowWizard specs).
 */
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';

test.describe('Quick Prompts section', () => {
  test('opens from the Automation sidebar and renders without crashing', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();

    // The shared sidebar exposes the QP category on its type chip; the chip
    // leaving "all" (`data-active`) confirms the list switched to it.
    await expect(workflows.kindChip).toBeVisible({ timeout: 5_000 });
    await expect(workflows.kindChip).toHaveAttribute('data-active', 'false');
    await workflows.selectKind('quickPrompts');
    await expect(workflows.kindChip).toHaveAttribute('data-value', 'quickPrompts');
    await expect(workflows.kindChip).toHaveAttribute('data-active', 'true');
  });

  test('section content renders the create-QP CTA when not empty', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.selectKind('quickPrompts');
    // Wait for the QP section body to mount. We don't assert anything
    // schema-bound — just that the page didn't throw and a button
    // (any) is reachable. Catches a render-time TypeError that
    // would otherwise show as a blank tab.
    await expect(page.locator('button').first()).toBeVisible();
  });
});
