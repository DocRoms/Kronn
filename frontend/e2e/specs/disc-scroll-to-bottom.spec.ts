import type { Route } from '@playwright/test';
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

/**
 * KT-49 — one click on the jump arrow must land on the real last message.
 *
 * The bug reported on 2026-07-27 ("on clique 3/4 fois sur la flèche pour
 * arriver tout en bas") only shows on a list tall enough that its height is
 * still settling when the scroll fires. We serve a synthetic 60-message
 * discussion through `page.route` rather than seeding the real DB: the height
 * is then reproducible, and the assertion is on the real scroll code.
 */

const DISC_ID = 'e2e-scroll-disc';
const TITLE = 'E2E — scroll to bottom';

const longBody = (index: number) =>
  [
    `Message ${index} — bloc de contenu volontairement long pour donner de la hauteur.`,
    '',
    ...Array.from({ length: 6 }, (_, line) => `Ligne ${line + 1} du message ${index}. `.repeat(4)),
  ].join('\n');

const discussion = {
  id: DISC_ID,
  project_id: null,
  title: TITLE,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: Array.from({ length: 60 }, (_, i) => ({
    id: `e2e-msg-${i}`,
    role: i % 2 === 0 ? 'User' : 'Agent',
    content: longBody(i),
    agent_type: i % 2 === 0 ? null : 'ClaudeCode',
    timestamp: '2026-01-01T00:00:00Z',
    tokens_used: 0,
    auth_mode: null,
  })),
  message_count: 60,
  non_system_message_count: 60,
  archived: false,
  pinned: false,
  pin_first_message: false,
  tier: 'default',
  summary_strategy: 'Auto',
  introspection_call_count: 0,
  workspace_mode: 'Direct',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  awaiting_agent: false,
};

const listRow = { ...discussion, messages: [] };

test.describe.configure({ timeout: 60_000, retries: 0 });

test.describe('Discussions — jump arrow reaches the last message', () => {
  test('one click lands at the bottom of a long discussion', async ({ page }) => {
    await page.route('**/api/discussions', async (route: Route) => {
      if (route.request().method() !== 'GET') return route.fallback();
      await route.fulfill({ json: { success: true, data: [listRow] } });
    });
    await page.route(`**/api/discussions/${DISC_ID}`, async (route: Route) => {
      if (route.request().method() !== 'GET') return route.fallback();
      await route.fulfill({ json: { success: true, data: discussion } });
    });

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.navDiscussions.click();
    await page.getByText(TITLE, { exact: false }).first().click();

    const messages = page.locator('.disc-messages').first();
    await expect(messages).toBeVisible();
    // Wait until the list is actually taller than its viewport, otherwise the
    // arrow never appears and the test would pass for the wrong reason.
    await expect
      .poll(async () => messages.evaluate(el => el.scrollHeight - el.clientHeight), { timeout: 15_000 })
      .toBeGreaterThan(400);

    // Opening a discussion pins the bottom for a short settle window; scroll up
    // only once it has elapsed, the way a reader would.
    await page.waitForTimeout(1_000);
    await messages.hover();
    await page.mouse.wheel(0, -60_000);
    await messages.evaluate(el => { el.scrollTop = 0; });
    const arrow = page.getByTestId('disc-scroll-to-bottom');
    await expect(arrow).toBeVisible();

    await arrow.click();

    await expect
      .poll(
        async () => messages.evaluate(el => el.scrollHeight - el.scrollTop - el.clientHeight),
        { timeout: 5_000 },
      )
      .toBeLessThanOrEqual(2);

    // And it took exactly one click: the arrow hides once we're at the bottom.
    await expect(arrow).toHaveCount(0);
  });
});
