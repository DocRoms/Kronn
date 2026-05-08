/**
 * WS / health reconnect — UI must surface backend unreachable + recover.
 *
 * Validates the BackendStatus pill behaviour against `/api/health`:
 *
 *   1. Steady-state: pill is hidden (zero chrome noise on happy path).
 *   2. /api/health starts failing (route-aborted to simulate
 *      Tailscale drop, gateway crash, network partition) → pill
 *      becomes visible with `[role="status"]` and the
 *      `kronn-backend-status` class.
 *   3. /api/health recovers → pill auto-hides on the next poll.
 *
 * Closes regression scope of TD-20260504-ws-reconnect-stale-ui (the
 * pill is the only persistent feedback loop the user has when the
 * gateway between their browser and the backend is dropping packets).
 *
 * # Why route-mocked rather than real disconnect
 *
 * `page.context().setOffline(true)` works for navigation-driven flows
 * but is flaky for in-flight `fetch()` polls — Playwright's
 * intercepting layer doesn't always cancel an already-in-flight
 * request. Routing the specific endpoint to abort gives a
 * deterministic signal.
 *
 * # Cost
 *
 * Zero $. No agent runs. ~30s test wall time (one poll cycle to
 * fail + one to recover; we override `POLL_INTERVAL_MS` indirectly
 * by aborting requests immediately so the cycle is just the
 * setTimeout(30s) jitter).
 */
import { test, expect } from '@playwright/test';

test.describe.configure({ timeout: 120_000, retries: 0 });

test.describe('Backend status pill — surfaces health failures + auto-recovers', () => {
  test('pill is hidden in steady state', async ({ page }) => {
    await page.addInitScript(() => {
      try { window.localStorage.setItem('kronn:tour-completed', 'true'); } catch { /* noop */ }
    });
    await page.goto('/');
    // Give the first health check a beat to fire and resolve OK.
    await page.waitForTimeout(2_000);
    const pill = page.locator('.kronn-backend-status');
    await expect(pill).toHaveCount(0);
  });

  test('pill appears when /api/health starts failing and disappears on recovery', async ({ page }) => {
    await page.addInitScript(() => {
      try { window.localStorage.setItem('kronn:tour-completed', 'true'); } catch { /* noop */ }
    });

    let mockedRequests = 0;
    // Start failing /api/health BEFORE navigation so the very first
    // poll fails. We fulfill with 503 rather than abort because fetch's
    // catch handler in `BackendStatus` reads `res.ok` — a 503 reliably
    // throws via the existing `if (!res.ok)` check, whereas
    // `route.abort` can sometimes hang the fetch in jsdom-style harness.
    await page.route('**/api/health', route => {
      mockedRequests++;
      void route.fulfill({
        status: 503,
        contentType: 'application/json',
        body: JSON.stringify({ ok: false, error: 'simulated outage' }),
      });
    });
    await page.goto('/');
    // Sanity log: the route should have intercepted at least one
    // health call by the time we look for the pill.
    await page.waitForTimeout(2_000);
    // eslint-disable-next-line no-console
    console.log(`[ws-reconnect] /api/health intercepts so far: ${mockedRequests}`);

    // Wait for the pill to mount. We poll on innerHTML rather than the
    // `.kronn-backend-status` selector directly because PW's locator
    // engine sometimes loses the element when React re-renders the
    // Suspense boundary above it (Dashboard mount sequence). Polling
    // via `evaluate` sidesteps that.
    await expect.poll(async () => {
      return await page.evaluate(() => !!document.querySelector('.kronn-backend-status'));
    }, {
      timeout: 30_000,
      intervals: [500, 1_000, 2_000],
      message: 'pill should mount after the first /api/health failure',
    }).toBe(true);

    const pill = page.locator('.kronn-backend-status').first();
    await expect(pill).toHaveAttribute('role', 'status');
    await expect(pill).toContainText(/Backend (unreachable|injoignable|inalcanzable)/);

    // Now restore /api/health so the next poll succeeds. The
    // BackendStatus component schedules a setTimeout(POLL_INTERVAL_MS
    // + jitter) after each check — worst case 35s. We give it 60s.
    await page.unroute('**/api/health');
    await expect(pill, 'pill should detach once /api/health recovers').toHaveCount(0, { timeout: 60_000 });
  });
});
