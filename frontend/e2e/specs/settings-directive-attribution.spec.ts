/**
 * Settings page — Directive cards (Caveman built-in) attribution + see-more.
 *
 * Regression guards (Sprint 1.5) :
 *   - Caveman directive shipped with `source_url` exposes a clickable
 *     "🔗 Source" link.
 *   - The "Adapted from <url> (<license>)." attribution suffix is rendered
 *     visually distinct from the rest of the description (italic + lower
 *     opacity, separate paragraph).
 *   - The full body has a "See more / See less" toggle.
 *
 * Caveman is shipped with the backend regardless of the 0.7+ external
 * skills bundling, so this spec works against any backend version.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { SettingsPage } from '../pages/SettingsPage';

test.describe('Settings — Caveman directive attribution', () => {
  test('Caveman card has source link + attribution + see-more', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const settings = new SettingsPage(page);

    await dashboard.goto();
    await dashboard.clickSettings();
    await settings.openDirectivesAccordion();

    const card = settings.directiveCard(/Caveman/i);
    await expect(card).toBeVisible({ timeout: 5_000 });

    // 1. Source link is rendered + targets the upstream project URL.
    const source = settings.sourceLink(card);
    await expect(source).toBeVisible();
    await expect(source).toHaveAttribute('href', /github\.com\/JuliusBrussee\/caveman/i);

    // 2. Attribution line is visually distinct (italic), and contains the
    //    canonical "Adapted from <url> (<license>)." string.
    const attribution = settings.attributionLine(card);
    await expect(attribution).toBeVisible();
    await expect(attribution).toContainText(/Adapted from .+ \(MIT\)\./);

    // 3. See more button is rendered (Caveman body is > 100 chars).
    const seeMore = settings.seeMoreButton(card);
    await expect(seeMore).toBeVisible();
  });

  test('See-more toggle reveals + hides Caveman body', async ({ page }) => {
    // Locks the expand/collapse contract so a future refactor (e.g. inline
    // markdown rendering instead of pre-wrap) keeps the toggle working.
    const dashboard = new DashboardPage(page);
    const settings = new SettingsPage(page);

    await dashboard.goto();
    await dashboard.clickSettings();
    await settings.openDirectivesAccordion();

    const card = settings.directiveCard(/Caveman/i);
    const seeMore = settings.seeMoreButton(card);

    await seeMore.click();
    // After expand, the button label flips to "See less" / "Voir moins".
    await expect(seeMore).toHaveText(/Moins|Less/i);

    await seeMore.click();
    // Back to "See more" / "Voir plus".
    await expect(seeMore).toHaveText(/Plus|More/i);
  });
});
