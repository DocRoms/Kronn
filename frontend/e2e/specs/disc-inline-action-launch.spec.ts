/**
 * Discussion inline Kronn action — launch (E2E).
 *
 * Seeds a real Agent message carrying a `kronn-action` fence through
 * `/api/disc/append` — the SAME production endpoint a real Claude Code/Codex
 * CLI bridge uses to push its transcript into Kronn, so the fence goes
 * through the exact `insert_message` -> `ingest_message_actions` transaction
 * a live agent turn would (see `backend/src/db/discussion_actions.rs`).
 * `/api/disc/create` is a pure DB insert with no agent dispatch, so seeding
 * costs no tokens and stays deterministic. The launched target is a Quick
 * Exec (`echo`) — no external network call, no LLM involved — so its
 * terminal state is reached quickly and deterministically.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

test.describe('Discussion inline action — launch', () => {
  test('launches a proposed Quick Exec action from a real discussion message and reaches its terminal state', async ({ page }) => {
    const qeResponse = await page.request.post('/api/quick-execs', {
      data: {
        name: `E2E collector ${Date.now()}`,
        command: 'echo',
        args: ['{"ok":true}'],
        timeout_secs: 10,
        output_format: 'json',
        variables: [],
      },
    });
    expect(qeResponse.ok()).toBe(true);
    const qe = (await qeResponse.json()) as { data: { id: string } };
    const targetId = qe.data.id;

    const discResponse = await page.request.post('/api/disc/create', {
      data: { title: 'E2E inline action', agent: 'ClaudeCode' },
    });
    expect(discResponse.ok()).toBe(true);
    const disc = (await discResponse.json()) as { data: { disc_id: string } };
    const discId = disc.data.disc_id;

    const content = `\`\`\`kronn-action\n{"kind":"quick_exec","target_id":"${targetId}"}\n\`\`\``;
    const appendResponse = await page.request.post('/api/disc/append', {
      data: {
        disc_id: discId,
        messages: [{ source_msg_id: 'e2e-action-1', role: 'Agent', content }],
      },
    });
    expect(appendResponse.ok()).toBe(true);
    const appended = (await appendResponse.json()) as { data: { appended: number } };
    expect(appended.data.appended).toBe(1);

    const dashboard = new DashboardPage(page);
    // Direct navigation to the same #discussion-<id> deep link a Live Page's
    // "open discussion" action produces (`openStandaloneDiscussion` in
    // `frontend/src/lib/live-page-navigation.ts`) — proves it is a real,
    // standalone entry point, not only reachable via `window.open`.
    await page.goto(`/#discussion-${discId}`);
    await dashboard.navDiscussions.waitFor({ state: 'visible', timeout: 15_000 });

    const card = page.locator('[data-testid^="discussion-action-"]');
    await expect(card).toBeVisible();
    await card.locator('.discussion-action-card__toggle').click();
    await card.locator('.discussion-action-card__launch').click();

    await expect(card).toHaveAttribute('data-state', 'succeeded', { timeout: 10_000 });
  });
});
