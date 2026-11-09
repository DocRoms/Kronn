import { defineConfig, devices } from '@playwright/test';

/**
 * Playwright config for the **boot** suite (KT-1629): a real `kronn` binary on
 * a fresh data directory and on the previous release's database, and the UI
 * that binary serves, with no agent configured.
 *
 * Unlike `playwright.config.ts`, nothing is proxied or stubbed and no dev
 * server starts: each spec boots the executable named by `KRONN_BIN` itself
 * (see e2e-boot/kronn-process.ts). The windows-latest `test-boot-windows` job
 * of ci-build.yml runs it; `pnpm test:boot` runs it anywhere.
 *
 * Required environment: `KRONN_BIN` (the kronn executable) and
 * `KRONN_FRONTEND_DIST` (the built `frontend/dist`). Optional:
 * `KRONN_E2E_EXPECT_NO_AGENT=1` fails the run when an agent CLI is installed
 * on the machine, `KRONN_E2E_HOME` gives Kronn an empty home directory.
 */
export default defineConfig({
  testDir: './e2e-boot',
  testMatch: '**/*.spec.ts',
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  // A boot that fails once is a boot that can fail: no retry hides it.
  retries: 0,
  workers: 1,
  reporter: process.env.CI ? [['github'], ['list']] : 'list',
  timeout: 300_000,
  expect: { timeout: 15_000 },
  use: {
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    locale: 'en-US',
  },
  projects: [
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        // A task worker's sandbox denies Chromium's multi-process rendezvous.
        ...(process.env.KRONN_E2E_CHROMIUM_SINGLE_PROCESS === '1'
          ? { launchOptions: { args: ['--single-process', '--no-zygote'] } }
          : {}),
      },
    },
  ],
});
