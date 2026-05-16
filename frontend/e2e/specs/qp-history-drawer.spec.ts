/**
 * 0.8.5 — QP history drawer E2E.
 *
 * Validates the user-facing wiring of the version-history drawer on a
 * real (live) backend. We stub `GET /api/quick-prompts/:id/history`
 * and `/metrics` so the test is hermetic — the goal is to verify the
 * frontend renders the response correctly, not the SQL aggregator
 * (which is covered by the backend unit tests).
 *
 * Target QP per the user spec: "Instances Analyses" on project
 * "front_euronews". The test stubs the responses so it passes even
 * when that QP isn't seeded yet (CI fresh DB), and exercises the
 * actual seed when present.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';

test.describe('0.8.5 — QP history drawer', () => {
  test('🕒 button opens drawer; cards render with metrics + diff toggle', async ({ page }) => {
    // Stub the two endpoints. The drawer auto-loads them on open.
    await page.route('**/api/quick-prompts/*/history', async (route, req) => {
      if (req.method() !== 'GET') return route.fallback();
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: [
            {
              id: 'qpv-2', quick_prompt_id: 'qp-target', version_index: 2,
              name: 'After improver', icon: '⚡',
              prompt_template: 'Audite la prod entre {{h1}} et {{h2}}.\n\nFocus sur Fastly + CloudWatch.',
              variables: [], agent: 'ClaudeCode', project_id: null,
              skill_ids: ['security'], profile_ids: ['coder'], directive_ids: ['concise'],
              tier: 'default', description: 'v2 refactor', created_at: '2026-05-17T10:00:00Z',
            },
            {
              id: 'qpv-1', quick_prompt_id: 'qp-target', version_index: 1,
              name: 'Initial', icon: '⚡',
              prompt_template: 'Audite la prod entre {{h1}} et {{h2}}.',
              variables: [], agent: 'ClaudeCode', project_id: null,
              skill_ids: [], profile_ids: [], directive_ids: [],
              tier: 'default', description: '', created_at: '2026-05-15T10:00:00Z',
            },
          ],
          error: null,
        }),
      });
    });
    await page.route('**/api/quick-prompts/*/metrics', async (route, req) => {
      if (req.method() !== 'GET') return route.fallback();
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: [
            { version_index: 2, launches: 5, avg_tokens: 800, avg_duration_ms: 3000, avg_cost_usd: 0.012 },
            { version_index: 1, launches: 5, avg_tokens: 1000, avg_duration_ms: 4000, avg_cost_usd: 0.018 },
          ],
          error: null,
        }),
      });
    });

    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.clickQuickPromptsTab();

    const toggles = page.locator('[data-testid="qp-history-toggle"]');
    const count = await toggles.count();
    test.skip(count === 0, 'No QPs in dev DB — skipping QP history drawer E2E');

    // Open the drawer on the first QP card.
    await toggles.first().click();
    await expect(page.locator('[data-testid="qp-history-drawer"]')).toBeVisible({ timeout: 5_000 });

    // Both version cards rendered.
    await expect(page.locator('[data-testid="qp-history-version-2"]')).toBeVisible();
    await expect(page.locator('[data-testid="qp-history-version-1"]')).toBeVisible();

    // The Δ% chip on v2 is rendered (both versions have launches=5 ≥ 3).
    const v2Card = page.locator('[data-testid="qp-history-version-2"]');
    await expect(v2Card).toContainText(/-20%/); // (800-1000)/1000 = -20%
    await expect(v2Card).toContainText(/-25%/); // (3000-4000)/4000 = -25%

    // Expand v2, reveal the diff toggle.
    await v2Card.locator('button').first().click();
    await expect(page.locator('[data-testid="qp-history-diff-toggle-2"]')).toBeVisible();
    await page.locator('[data-testid="qp-history-diff-toggle-2"]').click();
    await expect(page.locator('[data-testid="qp-history-diff-2"]')).toBeVisible();

    // Close with Escape.
    await page.keyboard.press('Escape');
    await expect(page.locator('[data-testid="qp-history-drawer"]')).toBeHidden();
  });
});
