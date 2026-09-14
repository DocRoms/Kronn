/** Release metadata is independent of RTK installation and fits narrow cards. */
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import type { AgentDetection, RtkVersionInfo } from '../../src/types/generated';

for (const width of [1440, 390]) {
  test(`release errors wrap and ccUsage remains visible without RTK at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    const error = 'Official source returned HTTP 429 Too Many Requests.';
    const agent: AgentDetection = {
      name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true,
      path: null, version: '1.0.0', latest_version: '1.0.2', origin: 'host',
      install_command: null, host_managed: false, host_label: null,
      runtime_available: true, auth_ready: true, rtk_available: false,
      rtk_hook_configured: false, version_check_error: error,
      version_checked_at: '2026-09-14T00:00:00Z',
    };
    const versions: RtkVersionInfo = {
      available: false, installed: null, latest_known: null, checked_at: null,
      check_error: error, update_available: false, update_command: '',
      ccusage: { installed: '20.1.0', latest: '20.1.2', checked_at: '2026-09-14T00:00:00Z', check_error: error, update_available: true },
    };
    await page.route('**/api/agents', route => route.fulfill({ json: { success: true, data: [agent], error: null } }));
    await page.route('**/api/rtk/version', route => route.fulfill({ json: { success: true, data: versions, error: null } }));
    await page.route('**/api/model-catalogs', route => route.fulfill({ json: { success: true, data: { targets: [] }, error: null } }));
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();
    const heading = page.locator('.set-agent-row[data-agent-type="ClaudeCode"] .set-agent-title-row');
    await expect(heading).toContainText(error);
    const compression = page.locator('.set-compression').filter({ hasText: /ccusage.*20\.1\.0/i });
    await expect(compression).toContainText('20.1.2');
    await expect(compression).toContainText(error);
    for (const container of [heading, compression]) {
      await expect.poll(() => container.evaluate(node => node.scrollWidth - node.clientWidth)).toBeLessThanOrEqual(1);
    }
  });
}
