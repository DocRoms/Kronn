/**
 * Browser E2E — a sent message's SSE-streamed agent reply renders as a bubble.
 *
 * P0-7 (test-quality audit, 2026-06-17): every other discussion spec is
 * `{request}` API-only — they POST `/run` and read the DB back, never driving
 * the BROWSER chat loop. The frontend's SSE *parsing* is densely unit-tested
 * (streaming.test.ts, stream-flush, useRafBatchedStream), but NOTHING pins the
 * end-to-end wiring a real user exercises: type → click send →
 * `sendMessageStream` → chunks accumulate into a live bubble → the reply
 * settles into the message list. A break in that chain (a routing regression,
 * a broken send-button gate, a streamingMap render bug) ships green today.
 *
 * # Why fully stubbed (no real agent)
 *
 * The real send endpoint `POST /api/discussions/:id/messages` BOTH persists the
 * user message and streams the agent reply from a live CLI — flaky, costs
 * tokens, and a mocked stream against a real backend loses the reply on the
 * post-send re-fetch (the backend never saw the mocked message). So we stub
 * the three discussion endpoints this view touches: the sidebar list, the
 * single-disc load (a counter returns the agent reply only AFTER the send),
 * and the SSE stream itself. That makes the test hermetic AND deterministic —
 * it asserts exactly the FRONTEND rendering of a stream, which is the gap.
 *
 * Boot endpoints + tour are stubbed by `kronn-fixture`; every other dashboard
 * call flows through to the real backend, same as the other specs.
 */
import { test, expect } from '../fixtures/kronn-fixture';
import type { Discussion, DiscussionMessage } from '../../src/types/generated';
import { DashboardPage } from '../pages/DashboardPage';

const DISC_ID = 'e2e-stream-disc';
const SEED_USER_MSG = 'message de depart e2e';
// Distinctive, lowercase-ASCII so the MatrixText reveal settles to an exact
// substring the locator can match.
const AGENT_REPLY = 'bonjour ceci est la reponse mockee du flux sse';

function mkMsg(role: DiscussionMessage['role'], content: string, id: string): DiscussionMessage {
  return {
    id,
    role,
    content,
    agent_type: role === 'Agent' ? 'ClaudeCode' : null,
    timestamp: '2026-06-17T10:00:00Z',
    tokens_used: 0,
    auth_mode: null,
    lint_report: null,
  };
}

function mkDisc(messages: DiscussionMessage[]): Discussion {
  return {
    id: DISC_ID,
    project_id: null,
    title: 'E2E Stream Render',
    agent: 'ClaudeCode',
    language: 'fr',
    participants: ['ClaudeCode'],
    messages,
    message_count: messages.length,
    non_system_message_count: messages.length,
    archived: false,
    pinned: false,
    workspace_mode: 'Direct',
    created_at: '2026-06-17T09:00:00Z',
    updated_at: '2026-06-17T10:00:00Z',
  };
}

const envelope = (data: unknown) => JSON.stringify({ success: true, data, error: null });

test.describe('Discussion chat — streamed agent reply renders in the browser', () => {
  test('type + send → the SSE-streamed agent reply appears as a message bubble', async ({ page }) => {
    // Flipped by the POST handler; the GET handler returns the agent reply
    // only once the send has happened — so the reply is a RESULT of sending,
    // not present on initial load.
    let sent = false;

    // Agents → ClaudeCode usable, so the composer ENABLES. The real
    // `/api/agents` depends on whether the runner has the `claude` binary
    // (CI doesn't), which marks ClaudeCode not-usable → `activeAgentDisabled`
    // → the textarea is `disabled` and this hermetic test can't type. Stub it.
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

    // Sidebar list → exactly our disc.
    await page.route('**/api/discussions', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope([mkDisc([mkMsg('User', SEED_USER_MSG, 'm-seed')])]),
      });
    });

    // Single-disc load (initial open + every post-send / 5s re-fetch).
    await page.route(`**/api/discussions/${DISC_ID}`, route => {
      if (route.request().method() !== 'GET') return route.continue();
      const messages = sent
        ? [mkMsg('User', SEED_USER_MSG, 'm-seed'), mkMsg('Agent', AGENT_REPLY, 'm-agent')]
        : [mkMsg('User', SEED_USER_MSG, 'm-seed')];
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope(mkDisc(messages)),
      });
    });

    // The send: flip the flag, then stream the reply as SSE chunks. Three
    // chunks exercise the accumulation path (not a single-shot reply).
    await page.route(`**/api/discussions/${DISC_ID}/messages`, route => {
      if (route.request().method() !== 'POST') return route.continue();
      sent = true;
      const sse =
        `event: chunk\ndata: ${JSON.stringify({ text: 'bonjour ceci ' })}\n\n` +
        `event: chunk\ndata: ${JSON.stringify({ text: 'est la reponse ' })}\n\n` +
        `event: chunk\ndata: ${JSON.stringify({ text: 'mockee du flux sse' })}\n\n` +
        `event: done\ndata: {}\n\n`;
      return route.fulfill({ status: 200, contentType: 'text/event-stream', body: sse });
    });

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.navDiscussions.click();

    // Open the disc from the sidebar. The row's aria-label is a plain
    // attribute (not MatrixText-animated) so it matches immediately.
    await page.getByRole('button', { name: /E2E Stream Render/ }).click();

    // The disc loaded: the seed user message is on screen.
    await expect(
      page.locator('.disc-msg-bubble').filter({ hasText: SEED_USER_MSG }),
    ).toBeVisible({ timeout: 10_000 });

    // Type a message — the send button gates on non-empty text.
    const input = page.locator('.disc-composer-textarea');
    await input.fill('un nouveau message a envoyer');
    const send = page.locator('.disc-send-btn');
    await expect(send).toBeEnabled();
    await send.click();

    // The streamed agent reply renders as a bubble. This is the wiring the
    // unit tests can't reach: chunks → streamingMap → rendered DOM.
    await expect(
      page.locator('.disc-msg-bubble').filter({ hasText: AGENT_REPLY }),
    ).toBeVisible({ timeout: 15_000 });
  });
});
