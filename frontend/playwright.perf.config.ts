import { defineConfig, devices } from '@playwright/test';

/**
 * Playwright config for the **performance regression** suite.
 *
 * Separate from `playwright.config.ts` (which targets the smoke / E2E
 * specs) because perf tests need a seeded sandbox backend on a different
 * port and shouldn't run on every CI green check. See
 * `frontend/e2e/perf/README.md` for the run book.
 *
 * Differences vs the default config:
 *   - testDir: `./e2e/perf` (perf specs only)
 *   - timeout bumped (cold render of 500 discs is slow on CI hardware)
 *   - retries: 0 (timing flake = real signal, retrying hides it)
 *   - reuseExistingServer: true (we *expect* the user to have spawned a
 *     sandbox Vite + backend manually per the runbook — Playwright should
 *     not auto-spawn its own and accidentally hit the user's real backend)
 */
export default defineConfig({
  testDir: './e2e/perf',
  testMatch: '**/*.perf.spec.ts',
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 1,
  reporter: 'list',
  timeout: 60_000,
  expect: { timeout: 10_000 },
  use: {
    baseURL: process.env.PLAYWRIGHT_BASE_URL ?? 'http://localhost:5173',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
    locale: 'fr-FR',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  ],
});
