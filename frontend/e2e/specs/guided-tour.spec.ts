/**
 * Guided tour ("présentation mode") — E2E coverage.
 *
 * The tour is the first-run onboarding overlay: 24 steps spanning Projects →
 * Plugins → Discussions → Automation → Config. It auto-launches 800 ms after
 * mount for a genuinely new user and persists versioned progress so an
 * interrupted walkthrough can be resumed explicitly from Settings.
 *
 * The default kronn-fixture pre-marks the tour as completed so other specs
 * are not intercepted by the welcome modal. This file deliberately uses the
 * raw `@playwright/test` import so the auto-launch is observable, and only
 * stubs the two boot endpoints (cf. `fixtures/api-stubs.ts`).
 *
 * Coverage targets:
 *   - Auto-launch on first visit (no flag) and welcome step renders.
 *   - Skip persists resumable progress → no unsolicited re-launch on refresh.
 *   - Forward navigation via the Next button advances the step counter.
 *   - Escape key dismisses the overlay.
 *   - Help button (?) replays the tour from step 0 even after completion.
 *   - Legacy partial progress migrates and resumes from the Settings CTA.
 */

import { test, expect, type Page } from '@playwright/test';
import { stubBootEndpoints } from '../fixtures/api-stubs';

const PROGRESS_KEY = 'kronn:tour-progress:v1';
const LEGACY_COMPLETED_KEY = 'kronn:tour-completed';
const LEGACY_STEP_KEY = 'kronn:tour-step';
const AUTO_START_DELAY = 800;

test.beforeEach(async ({ page }) => {
  await stubBootEndpoints(page);
});

// Clear localStorage AFTER the initial navigation (so the page's origin is
// resolved) but BEFORE the React app reads it. Doing this via addInitScript
// re-runs on every navigation, which causes the Skip-then-reload test to
// fail (the post-Skip flag gets wiped on reload). page.evaluate is one-shot
// and survives reloads within the same context.
async function freshTourState(page: Page) {
  await page.goto('/');
  await page.evaluate(({ progressKey, doneKey, stepKey }) => {
    try {
      window.localStorage.removeItem(progressKey);
      window.localStorage.removeItem(doneKey);
      window.localStorage.removeItem(stepKey);
    } catch { /* incognito / disabled storage */ }
  }, {
    progressKey: PROGRESS_KEY,
    doneKey: LEGACY_COMPLETED_KEY,
    stepKey: LEGACY_STEP_KEY,
  });
  await page.reload();
}

// TourProvider is mounted INSIDE Dashboard, so its auto-launch effect only
// schedules the 800 ms timer after the Dashboard finishes mounting. On a
// dev backend with thousands of seeded discussions the Dashboard render is
// slow enough that AUTO_START_DELAY + 2 s is too tight. Wait for the nav
// to be visible (= Dashboard mounted) before we start counting.
const TOOLTIP_TIMEOUT = AUTO_START_DELAY + 4_000;

async function waitForDashboardMounted(page: Page) {
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

  test('Skip preserves resumable progress and prevents unsolicited re-launch', async ({ page }) => {
    await freshTourState(page);
    await waitForDashboardMounted(page);
    const tooltip = page.getByRole('dialog').filter({ has: page.locator('.tour-step-counter') });
    await expect(tooltip).toBeVisible({ timeout: TOOLTIP_TIMEOUT });

    await tooltip.locator('.tour-btn-skip').click();
    await expect(tooltip).toBeHidden();

    const progress = await page.evaluate((k) => {
      const raw = window.localStorage.getItem(k);
      return raw ? JSON.parse(raw) : null;
    }, PROGRESS_KEY);
    expect(progress).toMatchObject({
      completedStepIds: [],
      currentStepId: 'welcome',
      hasStarted: true,
    });

    // Reload — an interruption remains available from Settings, but must not
    // unexpectedly take over the application again.
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
    await page.addInitScript((k) => { window.localStorage.setItem(k, 'true'); }, LEGACY_COMPLETED_KEY);
    await page.goto('/');
    await waitForDashboardMounted(page);
    await page.waitForTimeout(AUTO_START_DELAY + 200);

    // Welcome tooltip should NOT be visible because the flag is set.
    const tooltip = page.getByRole('dialog').filter({ has: page.locator('.tour-step-counter') });
    await expect(tooltip).toHaveCount(0);

    // Click the "?" help button in the top nav.
    const helpBtn = page.getByRole('button', {
      name: /Relancer le tour guidé|Replay guided tour|Repetir tour guiado/i,
    });
    await expect(helpBtn).toBeVisible();
    await helpBtn.click();

    // Tour overlay re-mounts at step 1.
    await expect(helpBtn).toBeHidden();
    await expect(tooltip).toBeVisible({ timeout: 5_000 });
    await expect(tooltip.locator('.tour-step-counter')).toContainText('1 / ');
  });

  test('migrates legacy partial progress and resumes it from Settings', async ({ page }) => {
    // Seed the old step-index format. The provider must migrate it without
    // auto-opening the overlay, then expose the explicit resume CTA.
    await page.addInitScript(([key]) => {
      window.localStorage.setItem(key, '2');
    }, [LEGACY_STEP_KEY]);

    await page.goto('/');
    const tooltip = page.getByRole('dialog').filter({ has: page.locator('.tour-step-counter') });
    await page.waitForTimeout(AUTO_START_DELAY + 500);
    await expect(tooltip).toHaveCount(0);

    await page.locator('[data-tour-id="nav-settings"]').click();
    const cta = page.locator('[data-testid="settings-tour-progress"]:visible');
    await expect(cta).toBeVisible();
    await cta.click();

    await expect(tooltip).toBeVisible({ timeout: 5_000 });
    await expect(tooltip.locator('.tour-step-counter')).toContainText('3 / ', { timeout: 5_000 });
  });
});
