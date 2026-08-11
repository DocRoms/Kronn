/**
 * Browser E2E — a send that fails before the durable `accepted` receipt must
 * never look sent or consume the user's text.
 *
 * The endpoints are hermetically stubbed: no CLI is launched and no token is
 * consumed. A 503 on the message stream models a backend restart or proxy
 * outage between clicking Send and persistence.
 */
import { test, expect } from '../fixtures/kronn-fixture';
import type { Discussion, DiscussionMessage } from '../../src/types/generated';
import { DashboardPage } from '../pages/DashboardPage';

const DISC_ID = 'e2e-send-receipt-disc';
const SEED_MESSAGE = 'message persiste avant la coupure';
const UNSENT_MESSAGE = 'ce texte ne doit surtout pas disparaitre';

const envelope = (data: unknown) => JSON.stringify({ success: true, data, error: null });

function message(): DiscussionMessage {
  return {
    id: 'm-seed',
    role: 'User',
    channel: 'main',
    content: SEED_MESSAGE,
    agent_type: null,
    timestamp: '2026-08-11T10:00:00Z',
    tokens_used: 0,
    auth_mode: null,
    lint_report: null,
  };
}

function discussion(messages: DiscussionMessage[]): Discussion {
  return {
    id: DISC_ID,
    project_id: null,
    title: 'E2E durable send receipt',
    agent: 'ClaudeCode',
    language: 'fr',
    participants: ['ClaudeCode'],
    messages,
    message_count: messages.length,
    non_system_message_count: messages.length,
    archived: false,
    pinned: false,
    workspace_mode: 'Direct',
    created_at: '2026-08-11T09:00:00Z',
    updated_at: '2026-08-11T10:00:00Z',
  };
}

test.describe('Discussion chat — durable send receipt', () => {
  test('a pre-receipt outage restores the draft and removes the optimistic row', async ({ page }) => {
    const persisted = discussion([message()]);
    let sendAttempts = 0;

    await page.route('**/api/agents', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope([{
          name: 'Claude Code', agent_type: 'ClaudeCode',
          installed: true, enabled: true, runtime_available: true,
          path: '/usr/bin/claude', version: '1.0.0', latest_version: null,
          origin: 'test', install_command: null,
          host_managed: false, host_label: null,
          rtk_available: false, rtk_hook_configured: false,
        }]),
      });
    });

    await page.route('**/api/discussions', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope([persisted]),
      });
    });

    await page.route(`**/api/discussions/${DISC_ID}`, route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope(persisted),
      });
    });

    await page.route(`**/api/discussions/${DISC_ID}/messages`, route => {
      if (route.request().method() !== 'POST') return route.continue();
      sendAttempts += 1;
      return route.fulfill({
        status: 503,
        contentType: 'application/json',
        body: JSON.stringify({ error: 'simulated backend restart' }),
      });
    });

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.navDiscussions.click();
    await page.getByRole('button', { name: /E2E durable send receipt/ }).click();

    const composer = page.locator('.disc-composer-textarea');
    await expect(composer).toBeEnabled();
    await composer.fill(UNSENT_MESSAGE);
    await page.locator('.disc-send-btn').click();

    await expect.poll(() => sendAttempts).toBe(1);
    await expect(composer).toHaveValue(UNSENT_MESSAGE);
    await expect(
      page.locator('.disc-msg-bubble').filter({ hasText: UNSENT_MESSAGE }),
    ).toHaveCount(0);
    await expect(
      page.locator('.disc-msg-bubble').filter({ hasText: SEED_MESSAGE }),
    ).toBeVisible();
  });
});
