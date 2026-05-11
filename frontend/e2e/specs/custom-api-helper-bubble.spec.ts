/**
 * E2E smoke for the Custom API AI helper bubble (0.8.1).
 *
 * Covers the UX scaffold ONLY — open the bubble, see welcome state,
 * click a starter chip, see input populated, open the agent dropdown.
 * We deliberately do NOT fire the agent: a real Claude/Codex round-trip
 * is expensive and tested separately by component unit tests (parser +
 * applyToCustomForm wire contract). What's left to pin here is the
 * "bubble appears with the right shape" part — the bit that depends on
 * cross-page CSS being loaded (the 0.8.1 regression that motivated this
 * spec: `wf-apicall-ai-*` styles lived in WorkflowsPage.css and didn't
 * apply on McpPage; the helper rendered unstyled).
 */
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

test.describe('Custom API helper bubble', () => {
  test('opens from McpPage Custom API form with welcome state + starter chips + agent dropdown', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.navMcps.click();

    // Open the Add-plugin drawer
    await page.locator('[data-tour-id="add-plugin-btn"]').click();

    // Click the pinned Custom API tile
    const customTile = page.locator('[data-tour-id="custom-api-tile"]');
    await expect(customTile).toBeVisible({ timeout: 5_000 });
    await customTile.click();

    // The Custom API form is open. The trigger button for the AI helper
    // sits next to Save / Back; it's labelled via the i18n
    // `mcp.custom.helper.trigger` key ("Construire avec l'IA" in FR,
    // "Build with AI" in EN). We match either.
    //
    // GUARD: the helper is wrapped in `installedAgentTypes && length > 0`
    // in McpPage.tsx. If the CI sandbox has no installed agents, the
    // trigger never renders. Skip cleanly in that case — the unit tests
    // still pin the open-bubble path. We give the DOM one short tick to
    // settle after the tile click before counting.
    await page.waitForTimeout(200);
    const helperTrigger = page.locator('button.wf-apicall-ai-trigger', {
      hasText: /Construire avec l'IA|Build with AI|Construir con IA/,
    });
    if ((await helperTrigger.count()) === 0) {
      test.skip(true, 'No installed agents in sandbox; helper trigger not rendered.');
      return;
    }
    await expect(helperTrigger).toBeVisible({ timeout: 5_000 });

    await helperTrigger.click();

    // Bubble dialog appears. Use the shared `wf-apicall-ai-bubble` class
    // (the same one used by ApiCallAiHelper) as the stable hook.
    const bubble = page.locator('.wf-apicall-ai-bubble');
    await expect(bubble).toBeVisible({ timeout: 5_000 });

    // Welcome state visible: 3 starter chips.
    const starterChips = bubble.locator('.wf-apicall-ai-starter-chip');
    await expect(starterChips).toHaveCount(3, { timeout: 2_000 });

    // Sanity check: the bubble has its styles applied. If the CSS file
    // failed to load (the 0.8.1 regression this spec exists to catch),
    // the bubble would be positioned `static` instead of `fixed` and
    // the trigger button would be a plain rectangle. We assert one
    // computed style as a proxy.
    const bubblePosition = await bubble.evaluate(el => getComputedStyle(el).position);
    expect(bubblePosition).toBe('fixed');

    // Click the first starter chip (Coller un curl / Paste a curl /
    // Pegar un curl).
    await starterChips.first().click();

    // The textarea should now be non-empty (template prefilled).
    const textarea = bubble.locator('textarea');
    const value = await textarea.inputValue();
    expect(value.length).toBeGreaterThan(0);

    // Open the agent dropdown via aria-haspopup="listbox".
    const agentTrigger = bubble.locator('button[aria-haspopup="listbox"]');
    await expect(agentTrigger).toBeVisible();
    await agentTrigger.click();
    await expect(bubble.locator('[role="listbox"]')).toBeVisible();

    // Close the bubble (Close button has aria-label tied to the i18n
    // `helper.close` key).
    await bubble.locator('button[aria-label*="ferme" i], button[aria-label*="close" i], button[aria-label*="cerrar" i]').first().click();
    await expect(bubble).not.toBeVisible({ timeout: 2_000 });
  });
});
