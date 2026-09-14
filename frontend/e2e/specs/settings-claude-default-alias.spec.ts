import { test, expect } from '../fixtures/kronn-fixture';

for (const width of [1440, 390]) {
  test(`Claude default alias warning is readable at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    const at = '2026-09-14T00:00:00Z';
    const target = {
      runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode',
      target_label: 'Claude Code', live_refresh_ok: true, stale: false,
      models: [{
        id: 'agent:claude-code:default', runtime_target_id: 'agent:claude-code',
        agent_type: 'ClaudeCode', model_id: 'default', display_name: 'Default',
        provenance: 'live', availability: 'available', capabilities: ['chat'],
        reasoning_modes: [], manual_origin: false, first_seen_at: at,
        last_seen_at: at, last_checked_at: at, created_at: at, updated_at: at,
      }],
    };
    await page.route('**/api/model-catalogs', route => route.fulfill({
      json: { success: true, data: { targets: [target] }, error: null },
    }));
    await page.route('**/api/model-catalogs/refresh', route => route.fulfill({
      json: { success: true, data: target, error: null },
    }));
    await page.goto('/');
    await page.locator('[data-tour-id="nav-settings"]').click();
    const row = page.getByTestId('model-catalog-row-agent:claude-code:default');
    const note = row.getByText('Défaut de la CLI — peut changer', { exact: true });
    await note.scrollIntoViewIfNeeded();
    await expect(note).toBeVisible();
    const dimensions = await note.evaluate(node => ({
      width: node.clientWidth, contentWidth: node.scrollWidth,
      height: node.clientHeight, contentHeight: node.scrollHeight,
    }));
    expect(dimensions.contentWidth).toBeLessThanOrEqual(dimensions.width + 1);
    expect(dimensions.contentHeight).toBeLessThanOrEqual(dimensions.height + 1);
  });
}
