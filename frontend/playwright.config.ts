import { defineConfig, devices } from '@playwright/test';

const devPort = Number(process.env.VITE_DEV_PORT ?? 5173);
const devUrl = `http://localhost:${devPort}`;

/**
 * Playwright config for Kronn E2E tests (Sprint 1.5).
 *
 * # Architecture
 *
 * Tests target the Vite dev server on `VITE_DEV_PORT` (5173 by default).
 * Vite proxies `/api` and `/ws` to `KRONN_BACKEND_URL` (:3140 by default,
 * cf. `vite.config.ts`). Playwright therefore drives the full stack as the
 * user does — no API mocking, real SSE streaming, real DB writes.
 *
 * # Backend / frontend lifecycle
 *
 * Playwright auto-spawns the **Vite dev server** before tests
 * via the `webServer` block below. `reuseExistingServer: true` means it
 * won't re-spawn if you already have `pnpm dev` running in another terminal
 * — useful when iterating on tests + UI in parallel.
 *
 * The **Rust backend** must be running separately. Two ways:
 *   - Docker:  `./kronn start` or `make start`
 *   - Native:  `make dev-backend` (cargo watch with auto-reload)
 *
 * If the backend is down, tests fail at the first /api call with a 502 from
 * Vite. We don't auto-spawn cargo because cold-start is ~30s and breaks the
 * fast-feedback loop. CI (J4) will add a separate webServer entry.
 *
 * # Auth
 *
 * Kronn skips auth for localhost requests (cf. `backend/src/lib.rs:247`).
 * Playwright runs from 127.0.0.1, no token or login flow needed.
 *
 * # Browser scope
 *
 * Chromium only for now. Firefox + WebKit add ~200 MB of cache + ~50% CI
 * time for marginal coverage on a self-hosted dev tool. Add them later if
 * we ship a hosted version.
 */
export default defineConfig({
  testDir: './e2e/specs',
  // Run tests in parallel WITHIN a worker, but only 1 worker overall.
  // Reason: Kronn's Rust backend is single-process and serializes some
  // operations (config writes, agent detection, MCP scans). 3 parallel
  // Playwright workers slamming /api/setup/status concurrently hangs the
  // backend. 1 worker = ~30s for the smoke suite, acceptable.
  fullyParallel: false,
  // Don't allow `.only` on CI — caught test slip would skip everything.
  forbidOnly: !!process.env.CI,
  // 1 retry on flake. The retried run re-uses the same worker so backend
  // state is consistent.
  retries: 1,
  workers: 1,
  reporter: process.env.CI ? 'github' : 'list',
  // Default action timeout (click, fill, etc.) — Kronn's API can be slow on
  // first cold-cache request, 10s leaves margin without making tests sleep.
  timeout: 30_000,
  expect: {
    timeout: 5_000,
  },
  use: {
    baseURL: devUrl,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
    // Locale matters: Kronn defaults to FR, our specs assert FR strings.
    // Switch to 'en-US' if you reset config.toml language between runs.
    locale: 'fr-FR',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  // Auto-spawn Vite. Reuses an existing instance when the configured port is
  // already serving the expected URL and the run is not in CI.
  // another terminal — devs iterating on UI + specs at the same time keep
  // their hot-reload session.
  webServer: {
    command: `pnpm exec vite --port ${devPort}`,
    url: devUrl,
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});
