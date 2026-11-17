/**
 * KT-1684 — before/after captures of the header (light and dark). Opt-in: it
 * only runs when `CAPTURE_DIR` is set, so it never weighs on CI.
 *
 *   CAPTURE_PHASE=after  CAPTURE_DIR=test-results/kt-1684 pnpm exec playwright test e2e/specs/kt-1684-captures.spec.ts
 *   CAPTURE_PHASE=before ...same, from a checkout of the base commit (this file copied in)
 *
 * The whole API is mocked, so no backend is needed. Per theme it writes the
 * header, the In progress panel, and the outage (panel auto-opened on
 * Notifications after ~10 s on the new build).
 */
import { test, expect } from '@playwright/test';

test.skip(!process.env.CAPTURE_DIR, 'captures run on demand (set CAPTURE_DIR)');

const PHASE = process.env.CAPTURE_PHASE ?? 'after';
const OUT = process.env.CAPTURE_DIR ?? 'test-results/kt-1684';

const ok = (data: unknown) => JSON.stringify({ success: true, data, error: null });
const now = Date.now();
const iso = (minAgo: number) => new Date(now - minAgo * 60_000).toISOString();

const rows = [
  { id: 'a:1', type: 'agent_turn', compartment: 'running', state: 'running', source: 'agent_dispatch_jobs', title: 'Refactor the billing module', project_name: 'Kronn', started_at: iso(6), stalled: false, actions: [] },
  { id: 'a:2', type: 'agent_turn', compartment: 'needs_decision', state: 'running', source: 'agent_dispatch_jobs', title: 'Which migration strategy?', project_name: 'Euronews', started_at: iso(22), stalled: false, actions: [] },
  { id: 'w:1', type: 'workflow_run', compartment: 'running', state: 'running', source: 'workflow_runs', title: 'Nightly changelog digest', detail: 'Step 3 / 5', started_at: iso(2), stalled: false, actions: [] },
  { id: 'w:2', type: 'workflow_run', compartment: 'running', state: 'queued', source: 'workflow_runs', title: 'Weekly dependency check', started_at: iso(1), stalled: false, actions: [] },
  { id: 'k:1', type: 'audit', compartment: 'running', state: 'running', source: 'audit_tracker', title: 'Audit of the mobile app', project_name: 'Euronews', started_at: iso(9), stalled: false, actions: [] },
  { id: 'k:2', type: 'task_worker', compartment: 'running', state: 'running', source: 'task_executions', title: 'KT-1684 worker', project_name: 'Kronn', started_at: iso(31), stalled: false, actions: [] },
];

for (const theme of ['light', 'dark'] as const) {
  test(`capture ${PHASE} ${theme}`, async ({ page }) => {
    test.setTimeout(120_000);
    await page.setViewportSize({ width: 1440, height: 860 });
    await page.addInitScript(name => {
      localStorage.setItem('kronn:tour-completed', 'true');
      localStorage.setItem('kronn:theme', name);
    }, theme);
    await page.routeWebSocket(/\/api\/ws/, () => { /* offline */ });

    let healthy = true;
    // Everything unknown is an empty success; specifics are registered after (they win).
    await page.route('**/api/**', route => route.fulfill({ status: 200, contentType: 'application/json', body: ok([]) }));
    await page.route('**/api/setup/status', route => route.fulfill({ status: 200, contentType: 'application/json', body: ok({ is_first_run: false, current_step: 'Complete', agents_detected: [], scan_paths_set: true, repos_detected: [], default_scan_path: null }) }));
    await page.route('**/api/config/ui-language', route => route.fulfill({ status: 200, contentType: 'application/json', body: ok('en') }));
    await page.route('**/api/ui-preferences', route => route.fulfill({ status: 200, contentType: 'application/json', body: ok({}) }));
    await page.route('**/api/version/check', route => route.fulfill({ status: 200, contentType: 'application/json', body: ok({ current: '0.15.3', latest: '0.15.4', up_to_date: false, release_url: 'https://github.com/DocRoms/Kronn/releases' }) }));
    await page.route('**/api/activity', route => route.fulfill({ status: 200, contentType: 'application/json', body: ok({ rows, stall_after_secs: 900, generated_at: new Date().toISOString(), unavailable_sources: [] }) }));
    await page.route('**/api/health', route => healthy
      ? route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ok: true, in_docker: false, version: '0.15.3' }) })
      : route.fulfill({ status: 503, contentType: 'application/json', body: '{"ok":false}' }));
    // The two standing notices of the bell (KT-1686): a key beside its data, a refused token.
    await page.route('**/api/config/recovery/status', route => route.fulfill({ status: 200, contentType: 'application/json', body: ok({ key_locked: false, config_holds_key: true, clear_credentials: [], config_set_aside: 'config.toml could not be read and was kept as config.toml.corrupt.1' }) }));
    await page.route('**/api/mcps', route => route.fulfill({ status: 200, contentType: 'application/json', body: ok({ servers: [], incompatibilities: [], configs: [{ id: 'cfg-1', label: 'Cloudflare API', server_id: 's', env_keys: [], project_ids: [], is_global: true, include_general: true, auth_failure: { kind: 'refused', http_status: 401, message: 'refused', failed_at: iso(5) } }] }) }));
    // Two failing reads: the same route and status, so they group (×N).
    await page.route('**/api/mcps/registry**', route => route.fulfill({ status: 500, contentType: 'application/json', body: JSON.stringify({ success: false, data: null, error: 'internal error: pool timed out' }) }));
    await page.route('**/api/stats/tokens**', route => route.fulfill({ status: 500, contentType: 'application/json', body: JSON.stringify({ success: false, data: null, error: 'internal error: pool timed out' }) }));

    await page.goto('/');
    await page.waitForTimeout(2500);
    await page.screenshot({ path: `${OUT}/${PHASE}-${theme}-1-header.png`, clip: { x: 0, y: 0, width: 1440, height: 110 } });

    const bell = page.getByTestId('activity-bell');
    await bell.click();
    await page.waitForTimeout(600);
    await page.screenshot({ path: `${OUT}/${PHASE}-${theme}-2-panel-in-progress.png` });
    await page.keyboard.press('Escape');

    healthy = false;
    await page.waitForTimeout(PHASE === 'after' ? 16_000 : 9_000);
    await page.screenshot({ path: `${OUT}/${PHASE}-${theme}-3-outage.png` });
    if (PHASE === 'before') {
      await bell.click().catch(() => {});
      await page.waitForTimeout(500);
      await page.screenshot({ path: `${OUT}/${PHASE}-${theme}-4-outage-panel.png` });
    }
    await expect(page.locator('body')).toBeVisible();
  });
}
