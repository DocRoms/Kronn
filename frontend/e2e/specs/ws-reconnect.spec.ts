/**
 * WS / health reconnect — UI must surface backend unreachable + recover.
 *
 * Validates the outage entry of the bell's Notifications tab against
 * `/api/health` (it was the BackendStatus pill until KT-1684):
 *
 *   1. Steady-state: the bell carries no alert (zero chrome noise on happy path).
 *   2. /api/health starts failing (fulfilled with 503 to simulate Tailscale
 *      drop, gateway crash, network partition) → the bell turns red with "!"
 *      and, opened, shows ONE System entry on the Notifications tab.
 *   3. /api/health recovers → the alert clears on the next poll and the entry
 *      moves to the history ("Back after …").
 *
 * Closes the health-surface part of
 * TD-20260715-ws-dead-after-restart-silent-send. WebSocket heartbeat and
 * resynchronisation are covered at hook/page level; this browser test pins the
 * global signal that remains visible on pages without a live subscription.
 *
 * # Why route-mocked rather than real disconnect
 *
 * `page.context().setOffline(true)` works for navigation-driven flows
 * but is flaky for in-flight `fetch()` polls — Playwright's
 * intercepting layer doesn't always cancel an already-in-flight
 * request. Routing the specific endpoint gives a deterministic signal.
 *
 * # Cost
 *
 * Zero $. No agent runs. Offline health checks retry after 2s; healthy checks
 * stay sparse so the happy path remains cheap.
 */
import { test, expect } from '../fixtures/kronn-fixture';

test.describe.configure({ timeout: 120_000, retries: 0 });

test.describe('Bell outage entry — surfaces health failures + auto-recovers', () => {
  test('the bell carries no alert in steady state', async ({ page }) => {
    await page.goto('/');
    // Give the first health check a beat to fire and resolve OK.
    await page.waitForTimeout(2_000);
    await expect(page.getByTestId('activity-bell')).not.toHaveAttribute('data-alert', 'outage');
    await expect(page.getByTestId('activity-bell-alert')).toHaveCount(0);
  });

  test('the bell turns red when /api/health starts failing and clears on recovery', async ({ page }) => {
    let mockedRequests = 0;
    // Start failing /api/health BEFORE navigation so the very first poll fails.
    // A 503 reliably throws through the existing `!res.ok` check, whereas
    // `route.abort` can sometimes hang the fetch in the harness.
    await page.route('**/api/health', route => {
      mockedRequests++;
      void route.fulfill({
        status: 503,
        contentType: 'application/json',
        body: JSON.stringify({ ok: false, error: 'simulated outage' }),
      });
    });
    await page.goto('/');
    await page.waitForTimeout(2_000);
    // eslint-disable-next-line no-console
    console.log(`[ws-reconnect] /api/health intercepts so far: ${mockedRequests}`);

    const bell = page.getByTestId('activity-bell');
    await expect(bell, 'the bell should turn red after the first /api/health failure')
      .toHaveAttribute('data-alert', 'outage', { timeout: 30_000 });
    await expect(page.getByTestId('activity-bell-alert')).toHaveText('!');

    // The panel opens on Notifications by itself once the outage lasts; open it
    // by hand if the test got there first. One entry, whatever the retries.
    if (!(await page.getByTestId('notifications-tab').isVisible())) {
      await bell.click();
    }
    const entry = page.getByTestId('notif-outage');
    await expect(entry).toHaveCount(1);
    // A restart is the usual cause: the entry says it reconnects, not that it broke.
    await expect(entry).toContainText(/reconnexion|reconnecting|reconectando|重新连接/);

    // Restore /api/health. Offline polling is deliberately accelerated, so a
    // restarted backend clears the alert in roughly two seconds.
    await page.unroute('**/api/health');
    await page.route('**/api/health', route => {
      void route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ ok: true, in_docker: false, version: 'e2e' }),
      });
    });
    await expect(bell, 'the alert should clear once /api/health recovers')
      .not.toHaveAttribute('data-alert', 'outage', { timeout: 15_000 });
    await expect(page.getByTestId('notif-outage')).toHaveCount(0);
  });
});
