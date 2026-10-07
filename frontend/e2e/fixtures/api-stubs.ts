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
  // Auto-update banner check fires once on Dashboard mount. Stub it to
  // a "no update" response so tests don't depend on either GitHub
  // reachability or the prod backend already running 0.7.2+. Tests
  // that specifically verify the banner override this stub.
  await page.route('**/api/version/check', route =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: { current: '0.7.1', latest: null, release_url: null, up_to_date: true },
        error: null,
      }),
    })
  );
  // The interface preferences are mirrored server-side (KT-972) and a fresh
  // browser context hydrates from that copy. Against the shared e2e backend,
  // one spec's tour progress or theme would leak into the next. Each page
  // gets its own in-memory mirror instead.
  let uiPreferences: Record<string, string> = {};
  await page.route('**/api/ui-preferences', route => {
    if (route.request().method() === 'PUT') {
      uiPreferences = route.request().postDataJSON() ?? {};
    }
    return route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: route.request().method() === 'PUT' ? null : uiPreferences,
        error: null,
      }),
    });
  });
  // Backend health pill polls `/api/health` every 30s. Stub a healthy
  // reply so the BackendStatus pill stays hidden during E2E runs —
  // tests that want to assert the offline pill override this stub.
  await page.route('**/api/health', route =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ ok: true, version: '0.7.1' }),
    })
  );
}
