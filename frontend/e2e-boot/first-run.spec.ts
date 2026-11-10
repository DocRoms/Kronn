/**
 * KT-1629 — Kronn boots, and its UI works, with no agent configured.
 *
 * Two boots of the real binary, each on its own data directory:
 *  - a fresh one: onboarding, the dashboard, a project, a manual backup and a
 *    sealed secret, all read back after a restart;
 *  - the database the previous release left behind, with migrations pending:
 *    the boot backs it up first (the path KT-1625 broke on Windows, os error 5).
 *
 * The UI is the one the binary itself serves (KRONN_FRONTEND_DIST). Nothing is
 * stubbed, so a 5xx from the API or an error screen is a real failure.
 */
import { expect, test, type Page, type TestInfo } from '@playwright/test';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, readSync, closeSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { dataFile, KronnInstance } from './kronn-process';

const PREVIOUS_RELEASE_DB =
  process.env.KRONN_UPGRADE_FIXTURE ?? resolve('..', 'backend', 'tests', 'fixtures', 'db-previous-release.sqlite');

/** What the UI shows when a screen crashed (ErrorBoundary, one string per locale). */
const ERROR_SCREEN_TEXTS = ['Something went wrong.', 'Une erreur est survenue.', 'Algo salió mal.'];

const NAV = {
  projects: '[data-tour-id="nav-projects"]',
  discussions: '[data-tour-id="nav-discussions"]',
  settings: '[data-tour-id="nav-settings"]',
};

const PROBE_PROVIDER = 'kt1629-probe';
const PROBE_SECRET = 'kt1629-sealed-SECRET-0123456789';

interface ApiResponse<T> {
  success: boolean;
  data: T | null;
  error: string | null;
}

function scratchRoot(): string {
  return mkdtempSync(join(tmpdir(), 'kronn-boot-'));
}

async function call<T>(kronn: KronnInstance, method: 'GET' | 'POST', path: string, body?: unknown): Promise<T> {
  const response = await fetch(`${kronn.url}${path}`, {
    method,
    headers: body === undefined ? undefined : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  expect(response.status, `${method} ${path}`).toBe(200);
  const envelope = (await response.json()) as ApiResponse<T>;
  expect(envelope.success, `${method} ${path}: ${envelope.error}`).toBe(true);
  return envelope.data as T;
}

async function expectHealthy(kronn: KronnInstance): Promise<void> {
  const response = await fetch(`${kronn.url}/api/health`);
  expect(response.status).toBe(200);
  const health = (await response.json()) as { ok: boolean; schema?: { status: string } };
  expect(health.ok).toBe(true);
  expect(health.schema?.status ?? 'ok').not.toBe('degraded');
}

/** No agent CLI is configured: always the case in a fresh install and in CI. */
async function expectNoAgentConfigured(kronn: KronnInstance): Promise<void> {
  const status = await call<{ agents_detected: { name: string; installed: boolean }[] }>(
    kronn,
    'GET',
    '/api/setup/status',
  );
  const installed = status.agents_detected.filter((agent) => agent.installed).map((agent) => agent.name);
  if (process.env.KRONN_E2E_EXPECT_NO_AGENT === '1') {
    expect(installed, 'an agent CLI is installed on this machine').toEqual([]);
  }
}

/** Collects every 5xx the API answers to this page. */
function watchServerErrors(page: Page, kronn: KronnInstance): string[] {
  const failures: string[] = [];
  page.on('response', (response) => {
    if (response.url().startsWith(kronn.url) && response.status() >= 500) {
      failures.push(`${response.status()} ${response.request().method()} ${response.url()}`);
    }
  });
  return failures;
}

async function expectNoErrorScreen(page: Page): Promise<void> {
  for (const text of ERROR_SCREEN_TEXTS) {
    await expect(page.getByText(text)).toHaveCount(0);
  }
  await expect(page.getByTestId('setup-config-set-aside')).toHaveCount(0);
}

/** The first-run wizard, answered the way a user with no agent does: skip and continue. */
async function reachDashboard(page: Page, kronn: KronnInstance): Promise<void> {
  await page.addInitScript(() => localStorage.setItem('kronn:tour-completed', 'true'));
  await page.goto(`${kronn.url}/`);
  const settingsTab = page.locator(NAV.settings);
  const onboarding = page.locator('.setup-container');
  await expect(onboarding.or(settingsTab)).toBeVisible({ timeout: 60_000 });
  const deadline = Date.now() + 180_000;
  while (!(await settingsTab.isVisible())) {
    if (Date.now() > deadline) throw new Error('the onboarding never reached the dashboard');
    const next = page.locator('.setup-btn-primary:enabled');
    if ((await next.count()) > 0) await next.first().click();
    await page.waitForTimeout(500);
  }
}

async function openEveryPage(page: Page): Promise<void> {
  await page.locator(NAV.settings).click();
  await expect(page).toHaveURL(/\/config/);
  await expect(page.locator('.set-accordion-header').first()).toBeVisible();
  await expectNoErrorScreen(page);

  await page.locator(NAV.projects).click();
  await expect(page).toHaveURL(/\/projects/);
  await expect(page.getByRole('button', { name: /Add a project|Ajouter un projet/i })).toBeVisible();
  await expectNoErrorScreen(page);

  await page.locator(NAV.discussions).click();
  await expect(page).toHaveURL(/\/discussions/);
  await expect(page.locator('.disc-sidebar')).toBeVisible();
  await expectNoErrorScreen(page);
}

function sqliteHeader(path: string): string {
  const handle = openSync(path, 'r');
  try {
    const buffer = Buffer.alloc(15);
    readSync(handle, buffer, 0, 15, 0);
    return buffer.toString('latin1');
  } finally {
    closeSync(handle);
  }
}

async function attachLog(kronn: KronnInstance, testInfo: TestInfo): Promise<void> {
  if (existsSync(kronn.logFile)) await testInfo.attach('kronn.log', { path: kronn.logFile });
}

test('fresh data directory: Kronn boots and its UI works with no agent', async ({ page }, testInfo) => {
  const root = scratchRoot();
  const kronn = new KronnInstance(join(root, 'data'), join(root, 'kronn.log'));
  const workspace = join(root, 'workspace', 'kt1629-demo');
  mkdirSync(workspace, { recursive: true });
  try {
    await kronn.start();
    await expectHealthy(kronn);
    await expectNoAgentConfigured(kronn);
    const serverErrors = watchServerErrors(page, kronn);

    // The UI the binary serves: onboarding first, then the dashboard.
    await page.goto(`${kronn.url}/`);
    await expect(page.locator('.setup-container')).toBeVisible({ timeout: 60_000 });
    await expectNoErrorScreen(page);
    await reachDashboard(page, kronn);
    await openEveryPage(page);

    // A project, a secret kept in the sealed config, and a manual backup.
    const project = await call<{ id: string; name: string }>(kronn, 'POST', '/api/projects/add-folder', {
      path: workspace,
      name: 'KT-1629 demo project',
    });
    await call(kronn, 'POST', '/api/config/api-keys', {
      name: 'KT-1629 probe',
      provider: PROBE_PROVIDER,
      value: PROBE_SECRET,
    });
    const backup = await call<{ backup_path: string; size_bytes: number }>(kronn, 'POST', '/api/db/backup');
    expect(backup.size_bytes).toBeGreaterThan(0);
    expect(statSync(backup.backup_path).size).toBe(backup.size_bytes);
    expect(sqliteHeader(backup.backup_path)).toBe('SQLite format 3');
    expect(readFileSync(dataFile(kronn, 'config.toml'), 'utf8')).not.toContain(PROBE_SECRET);

    // Restart, then read everything back.
    await kronn.stop();
    await kronn.start();
    await expectHealthy(kronn);
    const projects = await call<{ id: string; name: string }[]>(kronn, 'GET', '/api/projects');
    expect(projects.map((p) => p.id)).toContain(project.id);
    const tokens = await call<{ keys: { provider: string; masked_value: string }[] }>(
      kronn,
      'GET',
      '/api/config/tokens',
    );
    const probe = tokens.keys.find((key) => key.provider === PROBE_PROVIDER);
    expect(probe?.masked_value, 'the sealed secret was not read back').toBe('kt16...6789');
    expect(readFileSync(dataFile(kronn, 'config.toml'), 'utf8')).not.toContain(PROBE_SECRET);

    // The restarted instance serves the same UI over the same data.
    const reopened = await page.context().newPage();
    const reopenedErrors = watchServerErrors(reopened, kronn);
    await reopened.goto(`${kronn.url}/projects`);
    await expect(reopened.getByText('KT-1629 demo project').first()).toBeVisible({ timeout: 60_000 });
    await expectNoErrorScreen(reopened);

    expect([...serverErrors, ...reopenedErrors], 'the API answered with a 5xx').toEqual([]);
  } finally {
    await kronn.stop();
    await attachLog(kronn, testInfo);
  }
});

test('previous release database with pending migrations: Kronn backs it up, upgrades it and boots', async ({
  page,
}, testInfo) => {
  const root = scratchRoot();
  const kronn = new KronnInstance(join(root, 'data'), join(root, 'kronn.log'));
  mkdirSync(kronn.dataDir, { recursive: true });
  copyFileSync(PREVIOUS_RELEASE_DB, dataFile(kronn, 'kronn.db'));
  try {
    await kronn.start();
    await expectHealthy(kronn);
    await expectNoAgentConfigured(kronn);

    // The pre-migration backup is the step that stopped on os error 5.
    const backup = dataFile(kronn, 'kronn.db.backup');
    expect(existsSync(backup), 'no pre-migration backup was written').toBe(true);
    expect(sqliteHeader(backup)).toBe('SQLite format 3');

    const projects = await call<{ id: string; name: string }[]>(kronn, 'GET', '/api/projects');
    expect(projects.map((p) => p.name)).toContain('Fixture project');

    const serverErrors = watchServerErrors(page, kronn);
    await reachDashboard(page, kronn);
    await openEveryPage(page);
    await page.locator(NAV.projects).click();
    await expect(page.getByText('Fixture project').first()).toBeVisible();
    expect(serverErrors, 'the API answered with a 5xx').toEqual([]);
  } finally {
    await kronn.stop();
    await attachLog(kronn, testInfo);
  }
});
