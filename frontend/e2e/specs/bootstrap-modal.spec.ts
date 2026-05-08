/**
 * Smoke coverage for the project Bootstrap modal.
 *
 * Until 2026-05-10 the Bootstrap flow had **zero** E2E coverage. The
 * modal is the entry point for "create a new project + auto-fill its
 * docs/ via an AI architect" — the most-used onboarding path for
 * brand-new repos. A regression here = the user can't bootstrap a
 * project at all.
 *
 * Scope: open the modal from the Dashboard nav button, assert the
 * modal renders with its 3 tabs (bootstrap / clone / folder), close
 * via overlay-click (the cheapest dismiss path).
 */
import { test, expect } from '../fixtures/kronn-fixture';

test.describe('Project Bootstrap modal', () => {
  test('opens from the nav "+ Project" button and surfaces the 3 tabs', async ({ page }) => {
    await page.goto('/');
    // The "+ Project" CTA in the dashboard nav is `data-tour-id="new-project-btn"`.
    // It opens a modal overlay (`.dash-modal-overlay`) with three tabs:
    // Bootstrap (AI), Clone (git URL), Folder (existing path).
    await page.locator('[data-tour-id="new-project-btn"]').click();

    // Modal overlay should mount within a frame.
    await expect(page.locator('.dash-modal-overlay')).toBeVisible({ timeout: 3_000 });

    // The 3 tabs are tagged with tour-ids — pin those so a tab rename
    // doesn't silently break the smoke.
    await expect(page.locator('[data-tour-id="tab-bootstrap"]')).toBeVisible();
    await expect(page.locator('[data-tour-id="tab-clone"]')).toBeVisible();
    await expect(page.locator('[data-tour-id="tab-folder"]')).toBeVisible();
  });

  test('closes when the overlay is clicked outside the modal body', async ({ page }) => {
    await page.goto('/');
    await page.locator('[data-tour-id="new-project-btn"]').click();
    await expect(page.locator('.dash-modal-overlay')).toBeVisible();

    // Click the overlay (not the modal body) to dismiss. We click at
    // an offset to land on the overlay area, not the centred modal.
    await page.locator('.dash-modal-overlay').click({ position: { x: 5, y: 5 } });
    await expect(page.locator('.dash-modal-overlay')).not.toBeVisible({ timeout: 2_000 });
  });
});
