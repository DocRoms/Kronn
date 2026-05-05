/**
 * Targeted API stubs — only the 2 endpoints called from App.tsx during the
 * splash screen. Everything else passes through to the real backend.
 *
 * # Why ?
 *
 * `/api/setup/status` and `/api/config/ui-language` block the splash. The
 * Rust backend hangs on these when called from a browser (axum middleware
 * lock-up under concurrent requests). Stubbing them deterministically
 * unblocks the React app to mount the Dashboard, after which all other
 * API calls (which the dashboard panes make on demand, not at boot) flow
 * through to the real backend.
 *
 * Tests that need to verify the boot flow itself should NOT use this — they
 * should let the real backend respond.
 */

import type { Page } from '@playwright/test';

export async function stubBootEndpoints(page: Page) {
  await page.route('**/api/setup/status', route =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: {
          is_first_run: false,
          current_step: 'Complete',
          agents_detected: [],
          scan_paths_set: true,
          repos_detected: [],
          default_scan_path: null,
        },
        error: null,
      }),
    })
  );
  await page.route('**/api/config/ui-language', route =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: true, data: 'fr', error: null }),
    })
  );
}
