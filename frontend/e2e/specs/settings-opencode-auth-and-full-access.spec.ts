/**
 * Settings — OpenCode auth diagnostic and full-access toggle (KT-543).
 *
 * OpenCode runs over ACP (`opencode acp`), so its full-access toggle widens
 * the live AcpPermissionBroker's auto-approval scope instead of a static CLI
 * flag — the UI must describe that honestly rather than fabricate a flag.
 *
 * OpenCode also accepts environment credentials, provider config, and
 * local/no-auth providers Kronn cannot see from `~/.local/share/opencode/
 * auth.json` alone, so the backend never reports a confirmed `auth_ready:
 * false` for it — a missing/invalid auth file is "unknown, assume
 * runnable" (`auth_ready` omitted from the payload), never a hard negative.
 * The `auth_ready: false` case below still exercises real UI contract (the
 * warning banner and setup action must render correctly whenever the field
 * IS a confirmed `false`, for any agent), it just is not OpenCode's routine
 * default. Deterministic, fully mocked (no real CLI, no agent tokens).
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

const openCodeAgent = {
  name: 'OpenCode',
  agent_type: 'OpenCode',
  installed: true,
  enabled: true,
  path: '/usr/local/bin/opencode',
  version: '1.18.25',
  latest_version: null,
  origin: 'OpenCode',
  install_command: 'npm install -g opencode-ai',
  host_managed: false,
  host_label: null,
  runtime_available: false,
  auth_ready: false,
  auth_setup_command: 'opencode auth login',
  rtk_available: false,
  rtk_hook_configured: false,
  runtime_warning: null,
};

test('OpenCode with a confirmed auth failure shows its own setup action, never a generic default', async ({ page }) => {
  await page.route('**/api/agents', route => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({ success: true, data: [openCodeAgent], error: null }),
  }));

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.clickSettings();

  const agents = page.locator('#settings-agent-config');
  const openCode = agents.locator('.set-agent-row[data-agent-type="OpenCode"]');
  await expect(openCode).toBeVisible();

  await expect(openCode.getByRole('note')).toContainText(
    /Authentification requise|Authentication required|Autenticación necesaria/,
  );
  await expect(openCode.getByRole('button', { name: /opencode auth login/i })).toBeVisible();
  await expect(openCode.locator('.set-dot[data-state="auth-required"]')).toBeVisible();
});

test('OpenCode with no confirmed auth signal still reads as ready, not auth-required', async ({ page }) => {
  // Routine default: no auth.json, no other signal Kronn can confirm.
  // `auth_ready` is omitted entirely, same as an older API payload — the
  // agent must render exactly like a confirmed `true`, never blocked.
  const { auth_ready: _omitted, ...openCodeUnknownAuth } = openCodeAgent;
  await page.route('**/api/agents', route => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({ success: true, data: [openCodeUnknownAuth], error: null }),
  }));

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.clickSettings();

  const agents = page.locator('#settings-agent-config');
  const openCode = agents.locator('.set-agent-row[data-agent-type="OpenCode"]');
  await expect(openCode).toBeVisible();

  await expect(openCode.getByRole('note')).toHaveCount(0);
  await expect(openCode.locator('.set-dot[data-state="auth-required"]')).toHaveCount(0);
  await expect(openCode.locator('.set-dot[data-state="ready"]')).toBeVisible();
});

test('OpenCode full-access panel describes ACP permission scope, not a fabricated CLI flag', async ({ page }) => {
  await page.route('**/api/agents', route => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({
      success: true,
      data: [{ ...openCodeAgent, auth_ready: true, auth_setup_command: null }],
      error: null,
    }),
  }));

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.clickSettings();

  const agents = page.locator('#settings-agent-config');
  const panel = agents.locator('.set-agent-row[data-agent-type="OpenCode"] .set-agent-panel-access');
  await expect(panel).toBeVisible();
  await expect(panel.locator('code')).toHaveCount(0);
  await expect(panel.getByRole('switch')).toBeVisible();
});
