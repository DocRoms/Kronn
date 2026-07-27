import type { APIRequestContext } from '@playwright/test';
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { PluginsPage } from '../pages/PluginsPage';

const fixtureSuffix = `${Date.now()}`;
const alphaLabel = `PW Panel Alpha ${fixtureSuffix}`;
const bravoLabel = `PW Panel Bravo ${fixtureSuffix}`;
const configIds: string[] = [];

async function createChartbeatConfig(
  request: APIRequestContext,
  label: string,
  host: string,
): Promise<string> {
  const response = await request.post('/api/mcps/configs', {
    data: {
      server_id: 'api-chartbeat',
      label,
      env: {
        CHARTBEAT_API_KEY: `pw-key-${label}`,
        CHARTBEAT_HOST: host,
      },
      args_override: null,
      is_global: false,
      project_ids: [],
    },
  });
  expect(response.ok(), `create plugin config returned ${response.status()}`).toBe(true);
  const payload = await response.json();
  expect(payload?.success, JSON.stringify(payload)).toBe(true);
  return payload.data.id;
}

test.describe.configure({ timeout: 30_000, retries: 0 });

test.describe('Plugins — non-blocking detail panel', () => {
  test.beforeAll(async ({ request }) => {
    configIds.push(
      await createChartbeatConfig(request, alphaLabel, `alpha-${fixtureSuffix}.example.test`),
      await createChartbeatConfig(request, bravoLabel, `bravo-${fixtureSuffix}.example.test`),
    );
  });

  test.afterAll(async ({ request }) => {
    for (const id of configIds) {
      await request.delete(`/api/mcps/configs/${id}`).catch(() => {
        // Best-effort idempotent cleanup.
      });
    }
  });

  test('keeps the list interactive, swaps cards in one click and probes readiness', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const plugins = new PluginsPage(page);
    await page.route('**/api/mcps/configs/*/probe', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: {
            server_id: 'api-chartbeat',
            ready: false,
            checks: [{
              id: 'api',
              label: 'Authenticated API',
              ok: false,
              required: true,
              detail: 'The safe authentication request failed; check credentials and plugin scope',
            }],
          },
          error: null,
        }),
      });
    });

    await dashboard.goto();
    await dashboard.navMcps.click();

    await plugins.open(alphaLabel);
    await expect(plugins.panel).toHaveAttribute('aria-label', alphaLabel);
    await expect(page.locator('.mcp-modal-overlay')).toHaveCount(0);
    await expect(plugins.card(bravoLabel)).toBeVisible();

    await plugins.card(bravoLabel).click();
    await expect(plugins.panel).toHaveCount(1);
    await expect(plugins.panel).toHaveAttribute('aria-label', bravoLabel);

    await expect(plugins.preferredInterface).toBeDisabled();
    await expect(plugins.preferredInterface).toHaveValue('api');
    await expect(plugins.preferredInterface.locator('option')).toHaveCount(1);

    await plugins.probeButton.click();
    await expect(plugins.probeStatus).toHaveText(/Action requise|Action required|Acción requerida/, {
      timeout: 20_000,
    });
    await expect(plugins.panel).toContainText(/API authentifiée|Authenticated API|API autenticada/);

    await page.keyboard.press('Escape');
    await expect(plugins.panel).toHaveCount(0);
  });
});
