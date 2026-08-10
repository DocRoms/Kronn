/**
 * Settings — installed runner without authentication.
 *
 * Keeps the operator-facing distinction between "installed" and "ready":
 * Vibe must expose an actionable setup command instead of a green status.
 * The backend readiness/routing branch is covered by Rust tests; this spec
 * stays deterministic and spends no agent tokens.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

test('Vibe installed without auth shows the setup action', async ({ page }) => {
  await page.route('**/api/agents', route => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({
      success: true,
      data: [{
        name: 'Vibe',
        agent_type: 'Vibe',
        installed: true,
        enabled: true,
        path: '/usr/local/bin/vibe',
        version: '1.0.0',
        latest_version: null,
        origin: 'host',
        install_command: null,
        host_managed: true,
        host_label: 'macOS',
        runtime_available: true,
        auth_ready: false,
        auth_setup_command: 'vibe --setup',
        rtk_available: false,
        rtk_hook_configured: false,
        runtime_warning: null,
      }],
      error: null,
    }),
  }));

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.clickSettings();

  const agents = page.locator('#settings-agent-config');
  const vibe = agents.locator('.set-agent-row[data-agent-type="Vibe"]');
  await expect(vibe).toBeVisible();

  await expect(vibe.getByRole('note')).toContainText(
    /Authentification requise|Authentication required|Autenticación necesaria/,
  );
  await expect(vibe.getByRole('button', { name: /vibe --setup/i })).toBeVisible();
  await expect(vibe.locator('.set-dot[data-state="auth-required"]')).toBeVisible();
});
