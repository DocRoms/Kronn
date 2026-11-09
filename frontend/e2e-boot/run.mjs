// `pnpm test:boot`: runs the boot suite against a kronn you built locally.
// Defaults: the debug binary of this checkout (CARGO_TARGET_DIR when set) and
// frontend/dist. `--empty-home` gives Kronn an empty home directory, so its
// first-run scan does not walk your repositories. `--single-process-chromium`
// is for a sandbox that denies Chromium's multi-process rendezvous. Other
// arguments go to Playwright.
import { spawnSync } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const frontend = resolve(import.meta.dirname, '..');
const targetDir = process.env.CARGO_TARGET_DIR ?? resolve(frontend, '..', 'target');
const exe = process.platform === 'win32' ? 'kronn.exe' : 'kronn';

const args = process.argv.slice(2);
const emptyHome = args.includes('--empty-home');
const env = {
  ...process.env,
  KRONN_BIN: process.env.KRONN_BIN ?? join(targetDir, 'debug', exe),
  KRONN_FRONTEND_DIST: process.env.KRONN_FRONTEND_DIST ?? join(frontend, 'dist'),
};
if (emptyHome) env.KRONN_E2E_HOME = mkdtempSync(join(tmpdir(), 'kronn-boot-home-'));
if (args.includes('--single-process-chromium')) env.KRONN_E2E_CHROMIUM_SINGLE_PROCESS = '1';

const result = spawnSync(
  process.execPath,
  [
    join(frontend, 'node_modules', '@playwright', 'test', 'cli.js'),
    'test',
    '--config=playwright.boot.config.ts',
    ...args.filter((arg) => arg !== '--empty-home' && arg !== '--single-process-chromium'),
  ],
  { cwd: frontend, stdio: 'inherit', env },
);
process.exit(result.status ?? 1);
