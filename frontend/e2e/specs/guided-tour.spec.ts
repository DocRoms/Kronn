/**
 * Guided tour ("présentation mode") — E2E coverage.
 *
 * The tour is the first-run onboarding overlay: 17 steps spanning Projects →
 * Plugins → Discussions → Automation → Config. It auto-launches 800 ms after
 * mount when `localStorage['kronn:tour-completed']` is unset, and persists
 * the current step under `kronn:tour-step` so a refresh resumes mid-flow.
 *
 * The default kronn-fixture pre-marks the tour as completed so other specs
 * are not intercepted by the welcome modal. This file deliberately uses the
 * raw `@playwright/test` import so the auto-launch is observable, and only
 * stubs the two boot endpoints (cf. `fixtures/api-stubs.ts`).
 *
 * Coverage targets:
 *   - Auto-launch on first visit (no flag) and welcome step renders.
 *   - Skip persists the completion flag → no re-launch on refresh.
 *   - Forward navigation via the Next button advances the step counter.
 *   - Escape key dismisses the overlay.
 *   - Help button (?) replays the tour from step 0 even after completion.
 *   - Resume from a saved step when the completion flag is missing.
 */

import { test, expect } from '@playwright/test';
import { stubBootEndpoints } from '../fixtures/api-stubs';

const STORAGE_KEY = 'kronn:tour-completed';
const STEP_KEY = 'kronn:tour-step';
const AUTO_START_DELAY = 800;

test.beforeEach(async ({ page }) => {
  await stubBootEndpoints(page);
});

// Clear localStorage AFTER the initial navigation (so the page's origin is
// resolved) but BEFORE the React app reads it. Doing this via addInitScript
// re-runs on every navigation, which causes the Skip-then-reload test to
// fail (the post-Skip flag gets wiped on reload). page.evaluate is one-shot
// and survives reloads within the same context.
async function freshTourState(page: import('@playwright/test').Page) {
  await page.goto('/');
  await page.evaluate(({ doneKey, stepKey }) => {
    try {
      window.localStorage.removeItem(doneKey);
      window.localStorage.removeItem(stepKey);
    } catch { /* incognito / disabled storage */ }
  }, { doneKey: STORAGE_KEY, stepKey: STEP_KEY });
  await page.reload();
}

// TourProvider is mounted INSIDE Dashboard, so its auto-launch effect only
// schedules the 800 ms timer after the Dashboard finishes mounting. On a
// dev backend with thousands of seeded discussions the Dashboard render is
// slow enough that AUTO_START_DELAY + 2 s is too tight. Wait for the nav
// to be visible (= Dashboard mounted) before we start counting.
const TOOLTIP_TIMEOUT = AUTO_START_DELAY + 4_000;

async function waitForDashboardMounted(page: import('@playwright/test').Page) {
  await page.locator('[data-tour-id="nav-projects"]').waitFor({ state: 'visible', timeout: 10_000 });
}

test.describe('Guided tour — first launch', () => {
  test('auto-launches with the welcome step on a fresh visit', async ({ page }) => {
    await freshTourState(page);
    await waitForDashboardMounted(page);

    // Tooltip mounts under document.body via createPortal — the role+aria
    // markup is the most stable hook (i18n / locale-agnostic).
    const tooltip = page.getByRole('dialog').filter({ has: page.locator('.tour-step-counter') });
    await expect(tooltip).toBeVisible({ timeout: TOOLTIP_TIMEOUT });
    await expect(tooltip.locator('.tour-step-counter')).toContainText('1 / ');
  });

  test('Skip button persists kronn:tour-completed and prevents re-launch', async ({ page }) => {
    await freshTourState(page);
    await waitForDashboardMounted(page);
    const tooltip = page.getByRole('dialog').filter({ has: page.locator('.tour-step-counter') });
    await expect(tooltip).toBeVisible({ timeout: TOOLTIP_TIMEOUT });

    await tooltip.locator('.tour-btn-skip').click();
    await expect(tooltip).toBeHidden();

    // Flag set in localStorage.
    const flag = await page.evaluate((k) => window.localStorage.getItem(k), STORAGE_KEY);
    expect(flag).toBe('true');

    // Reload — the auto-launch effect must short-circuit on the flag.
    await page.reload();
    await waitForDashboardMounted(page);
    await page.waitForTimeout(AUTO_START_DELAY + 500);
    await expect(tooltip).toHaveCount(0);
  });

  test('Next button advances the step counter', async ({ page }) => {
    await freshTourState(page);
    await waitForDashboardMounted(page);
    const tooltip = page.getByRole('dialog').filter({ has: page.locator('.tour-step-counter') });
    await expect(tooltip).toBeVisible({ timeout: TOOLTIP_TIMEOUT });
    await expect(tooltip.locator('.tour-step-counter')).toContainText('1 / ');

    // Step 1 (welcome) has no selector → Next advances synchronously.
    await tooltip.locator('.tour-btn-next').click();
    await expect(tooltip.locator('.tour-step-counter')).toContainText('2 / ', { timeout: 5_000 });
  });

  test('Escape key dismisses the overlay', async ({ page }) => {
    await freshTourState(page);
    await waitForDashboardMounted(page);
    const tooltip = page.getByRole('dialog').filter({ has: page.locator('.tour-step-counter') });
    await expect(tooltip).toBeVisible({ timeout: TOOLTIP_TIMEOUT });

    await page.keyboard.press('Escape');
    await expect(tooltip).toBeHidden();
  });
});

test.describe('Guided tour — replay', () => {
  test('Help button (?) replays the tour from step 0 even after completion', async ({ page }) => {
    // Pre-mark completed so auto-launch is suppressed.
    await page.addInitScript((k) => { window.localStorage.setItem(k, 'true'); }, STORAGE_KEY);
    await page.goto('/');
    await page.waitForTimeout(AUTO_START_DELAY + 200);

    // Welcome tooltip should NOT be visible because the flag is set.
    const tooltip = page.getByRole('dialog').filter({ has: page.locator('.tour-step-counter') });
    await expect(tooltip).toHaveCount(0);

    // Click the "?" help button in the top nav.
    await page.locator('button[aria-label]').filter({ hasText: '' }).first().waitFor();
    const helpBtn = page.locator('button[aria-label*="tour" i], button[title*="tour" i]').first();
    await helpBtn.click();

    // Tour overlay re-mounts at step 1.
    await expect(tooltip).toBeVisible({ timeout: 3_000 });
    await expect(tooltip.locator('.tour-step-counter')).toContainText('1 / ');
  });

  test('resumes from a saved step when the completion flag is missing', async ({ page }) => {
    // The provider's auto-resume picks up `kronn:tour-step` when the
    // completion flag is absent. We seed step 2 (welcome=0, concept=1,
    // scan=2) and let the auto-launch read it.
    await page.addInitScript(([key]) => {
      window.localStorage.setItem(key, '2');
    }, [STEP_KEY]);

    await page.goto('/');
    const tooltip = page.getByRole('dialog').filter({ has: page.locator('.tour-step-counter') });
    await expect(tooltip).toBeVisible({ timeout: AUTO_START_DELAY + 4_000 });
    // After the resume kick (300 ms page wait + 2 s waitForElement), the
    // counter should reach the resume step (3 / 17 = 1-indexed of step 2).
    await expect(tooltip.locator('.tour-step-counter')).toContainText('3 / ', { timeout: 5_000 });
  });
});
