/**
 * Extended Playwright `test` fixture for Kronn E2E.
 *
 * Replaces the vanilla `import { test } from '@playwright/test'` with one
 * that:
 *   1. Auto-stubs the 2 backend endpoints called from the splash screen
 *      (`/api/setup/status` + `/api/config/ui-language`). Without this the
 *      Rust backend's axum middleware locks under browser-driven concurrent
 *      load — the splash never resolves.
 *   2. Pre-marks the guided tour as completed so the welcome modal doesn't
 *      intercept clicks on the dashboard nav.
 *
 * Usage in a spec:
 *
 *   import { test, expect } from '../fixtures/kronn-fixture';
 *   test('something', async ({ page }) => { ... });
 *
 * # Bypassing the auto-stubs
 *
 * Some specs WILL want to test the boot flow itself (real backend response,
 * setup wizard, etc.). They re-import `test` from `@playwright/test` and
 * skip this fixture. We don't expose a runtime opt-out flag — explicit
 * import = clear intent.
 */

import { test as base, expect } from '@playwright/test';
import { stubBootEndpoints } from './api-stubs';

export const test = base.extend({
  page: async ({ page }, use) => {
    await stubBootEndpoints(page);
    await page.addInitScript(() => {
      localStorage.setItem('kronn:tour-completed', 'true');
    });
    await use(page);
  },
});

export { expect };
