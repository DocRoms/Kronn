// Local-only guard for `pnpm test:e2e`: installs the Playwright headless
// shell when missing. Skips in CI — the container job pre-bakes browsers
// (PLAYWRIGHT_BROWSERS_PATH) precisely to avoid the flaky CDN download.
import { spawnSync } from 'node:child_process';

if (process.env.CI || process.env.PLAYWRIGHT_BROWSERS_PATH) {
  process.exit(0);
}

const r = spawnSync('playwright', ['install', 'chromium', '--only-shell'], {
  stdio: 'inherit',
  shell: process.platform === 'win32',
});
process.exit(r.status ?? 1);
