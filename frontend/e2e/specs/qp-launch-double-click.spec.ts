/**
 * QP launch — double-click race regression (E2E).
 *
 * The unit tests pin the in-component ref guard, but the full UX path
 * (Workflows tab → Quick Prompts → inline launch button, two clicks
 * fast) involves React lazy loading, the I18n provider, and a real
 * `discussionsApi.create` POST against the backend. This spec exercises
 * that full chain to make sure the guard survives bundling, suspense,
 * and the real network round-trip.
 *
 * Strategy:
 * 1. Stub `POST /api/discussions` so the response is held until we
 *    manually release it — that's what gives us a window to fire the
 *    second click.
 * 2. Navigate to Workflows → Quick Prompts.
 * 3. Fire two synchronous clicks on the launch button of the seeded,
 *    variable-free QP (found by `data-qp-id`) and assert the create
 *    endpoint was called exactly once.
 *
 * The spec seeds a variable-free Quick Prompt of its own: a fresh CI
 * database has none, and another spec's leftovers are not a fixture.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';
import { seedQuickPrompt, deleteQuickPrompt } from '../fixtures/quick-prompt-seed';

test.describe('QP launch — double-click race', () => {
  let qpId = '';
  test.beforeAll(async ({ request }) => {
    qpId = await seedQuickPrompt(request, `Double-click PW ${Date.now()}`);
  });
  test.afterAll(async ({ request }) => {
    await deleteQuickPrompt(request, qpId);
  });

  test('two synchronous Launch clicks on a no-variable QP create exactly one discussion', async ({ page }) => {
    // Hold POST /api/discussions open so we can fire a second click
    // before the first request finishes. We count the hits and respond
    // after both clicks have fired.
    let createHits = 0;
    let releaseFirst: (() => void) | undefined;
    await page.route('**/api/discussions', async (route, req) => {
      if (req.method() !== 'POST') return route.fallback();
      createHits += 1;
      // Hold the first request open until the second click has had a
      // chance to fire (give the browser ~150ms to deliver both clicks
      // before we let React see the success response).
      await new Promise<void>(resolve => { releaseFirst = resolve; });
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: {
            id: 'disc-test-1',
            project_id: null,
            title: 'E2E test',
            agent: 'ClaudeCode',
            language: 'fr',
            messages: [],
            tier: 'default',
            archived: false,
            pinned: false,
            workspace_path: null,
            skill_ids: [],
            profile_ids: [],
            directive_ids: [],
            created_at: new Date().toISOString(),
            updated_at: new Date().toISOString(),
            message_count: 0,
            tokens_used: 0,
          },
          error: null,
        }),
      });
    });

    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();

    // Switch to the Quick Prompts tab.
    await workflows.selectKind('quickPrompts');

    // Target the seeded card by `data-qp-id` (WorkflowsPage.tsx sets it on
    // each `.qp-card`): on a populated database another prompt may come
    // first, with variables, and the launch would open its form instead of
    // the no-variable case this test is about.
    const card = page.locator(`[data-qp-id="${qpId}"]`);
    const launchButton = card.locator('button.qp-launch-btn');
    await expect(launchButton, 'the seeded Quick Prompt must list a launch button').toBeVisible();

    // Fire two synchronous click events on that launch button via
    // evaluate. We can't use locator.click() twice because after the
    // first click the button is disabled — Playwright would wait for it
    // to re-enable, which never happens during the in-flight POST. The
    // race we care about is "two click events arriving back-to-back
    // before React commits the disabled state", which happens at the
    // browser level regardless of disabled (the closure-stale bug
    // pre-fix). Dispatching them directly mimics that pattern.
    // The seeded prompt has no variables, so the button calls the create
    // endpoint directly (WorkflowsPage.tsx, `qp.variables.length === 0`):
    // no launch form to go through.
    await launchButton.evaluate(btn => {
      // Two synchronous click events. Browsers don't dispatch click on a
      // disabled <button>, so we use the underlying onClick handler via
      // dispatchEvent with bubbles:true. React listens at the document
      // root and runs the handler regardless of disabled.
      btn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      btn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    // Give React + the network stub a moment to register both clicks.
    await page.waitForTimeout(300);

    // Assert exactly ONE POST hit before we release the first request.
    expect(createHits).toBe(1);

    // Release so the page can clean up gracefully.
    releaseFirst?.();
  });
});
