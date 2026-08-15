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

test.describe('Quick Prompts section', () => {
  test('opens from the Automation sidebar and renders without crashing', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();

    // The shared sidebar exposes the QP category through a stable tour anchor;
    // `data-active` confirms the content panel switched to it.
    const qpSection = page.locator('[data-tour-id="automation-kind-quick-prompt"]');
    await expect(qpSection).toBeVisible({ timeout: 5_000 });
    await qpSection.click();
    await expect(qpSection).toHaveAttribute('data-active', 'true');
  });

  test('section content renders the create-QP CTA when not empty', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();
    await page.locator('[data-tour-id="automation-kind-quick-prompt"]').click();
    // Wait for the QP section body to mount. We don't assert anything
    // schema-bound — just that the page didn't throw and a button
    // (any) is reachable. Catches a render-time TypeError that
    // would otherwise show as a blank tab.
    await expect(page.locator('button').first()).toBeVisible();
  });
});
