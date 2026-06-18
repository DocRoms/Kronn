/**
 * Performance regression — opening a discussion with a multi-MB message.
 *
 * 2026-06-23: a killed Codex run left a 2.4 MB stderr/reasoning dump as its
 * reply. Opening that discussion pushed the content through ReactMarkdown +
 * remark-gfm + syntax highlight (super-linear) → the browser tab FROZE then
 * CRASHED. Unit tests render in jsdom (no paint) so they can't catch a real
 * OOM; this opens the disc in a real Chromium and asserts the page stays
 * responsive thanks to the plain-text size guard in `MarkdownContent`.
 *
 * Fixture: `seed_large_message.py` (run after the sandbox boot — see
 * frontend/e2e/perf/README.md). Skips cleanly if the fixture isn't seeded.
 *
 * Pre-fix this test would TIME OUT (the disc never finished rendering / the
 * tab crashed). Post-fix the guarded plain-text render shows within budget.
 */
import { test, expect } from '@playwright/test';
import { stubBootEndpoints } from '../fixtures/api-stubs';

const FIXTURE_TITLE = 'PERF Large message (multi-MB)';
// Budget: from clicking the disc row to the guard banner being visible.
// A healthy plain-text render is near-instant; pre-fix it never rendered.
// Generous to absorb headless CI variance while still tripping on a true
// regression (someone removing the size guard → freeze → timeout).
const OPEN_RENDER_BUDGET_MS = 8_000;

test.beforeEach(async ({ page }) => {
  await stubBootEndpoints(page);
  await page.addInitScript(() => {
    try { window.localStorage.setItem('kronn:tour-completed', 'true'); } catch { /* incognito */ }
  });
});

test.describe('Large message render — perf', () => {
  test('opening a multi-MB discussion stays responsive (no freeze/crash)', async ({ page }) => {
    await page.goto('/');
    await page.locator('[data-tour-id="nav-discussions"]').waitFor({ state: 'visible' });

    // Skip cleanly if the sandbox wasn't seeded with the fixture.
    const seeded = await page.evaluate(async (title) => {
      const r = await fetch('/api/discussions?page=1&per_page=500');
      const j = await r.json();
      return (j?.data ?? []).some((d: { title?: string }) => d.title === title);
    }, FIXTURE_TITLE);
    test.skip(!seeded, 'Fixture not present — run seed_large_message.py against the perf sandbox');

    await page.locator('[data-tour-id="nav-discussions"]').click();

    const row = page.getByRole('button', { name: new RegExp(FIXTURE_TITLE.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) });
    await row.waitFor({ state: 'visible' });

    const t0 = Date.now();
    await row.click();

    // The size guard renders a notice banner instead of a 2.7 MB markdown
    // tree. Its appearance within budget proves the tab didn't freeze.
    const banner = page.getByRole('note').filter({ hasText: /volumineux|large message/i });
    await expect(banner).toBeVisible({ timeout: OPEN_RENDER_BUDGET_MS });
    const elapsed = Date.now() - t0;

    // The full 2.7 MB must NOT be in the DOM (guard truncates the inline view).
    const renderedChars = await page.evaluate(() => {
      const pre = document.querySelector('.disc-md pre');
      return pre?.textContent?.length ?? 0;
    });
    expect(renderedChars).toBeLessThan(200_000); // capped well under the full payload

    // Page is still interactive (a crashed tab can't evaluate this).
    expect(await page.evaluate(() => document.title.length >= 0)).toBe(true);

    console.log(`[perf] large-message disc opened + guarded in ${elapsed} ms (rendered ${renderedChars} chars)`);
  });
});
