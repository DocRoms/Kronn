/**
 * 0.8.5 QP feature smoke E2E.
 *
 * Covers the THREE user-facing affordances introduced in 0.8.5:
 *   1. QP form exposes the bindings accordion (skills + profiles + directives).
 *   2. QP card exposes the new "✨ Improve with AI" button.
 *   3. Clicking the improve button POSTs `/api/discussions` with the
 *      qp-improver skill pinned and the QP body in the initial prompt.
 *
 * We stub `POST /api/discussions` so the test doesn't depend on real
 * agents being installed in the dev backend — the goal is to verify the
 * frontend wiring (skill_ids + initial_prompt shape + navigation), not
 * the agent's response. The agent-side critique is exercised in the
 * skill unit tests (see `core/skills.rs::qp_improver_skill_teaches_strict_output_protocol`)
 * and the signal-parsing logic is locked in
 * `frontend/src/lib/__tests__/qp-improver-signal.test.ts`.
 *
 * The spec auto-skips when no QP is present in the dev DB (devs without
 * Workflows pre-seeded shouldn't get red CI runs locally).
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';

test.describe('0.8.5 — QP AI Improver + bindings (frontend wiring)', () => {
  test('QP card exposes ✨ Improve button + clicking it POSTs /api/discussions with the qp-improver skill', async ({ page }) => {
    // Stub the discussion-create endpoint so the test is hermetic.
    let lastCreatePayload: Record<string, unknown> | null = null;
    await page.route('**/api/discussions', async (route, req) => {
      if (req.method() !== 'POST') return route.fallback();
      try {
        lastCreatePayload = req.postDataJSON() as Record<string, unknown>;
      } catch {
        lastCreatePayload = null;
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: {
            id: 'disc-improve-test',
            project_id: null,
            title: '[Improve QP qp-test] Test',
            agent: 'ClaudeCode',
            language: 'fr',
            messages: [],
            tier: 'default',
            archived: false,
            pinned: false,
            workspace_path: null,
            skill_ids: ['qp-improver'],
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
    await workflows.clickQuickPromptsTab();

    const improveBtns = page.locator('[data-testid="qp-improve-btn"]');
    const count = await improveBtns.count();
    test.skip(count === 0, 'No Quick Prompts in dev DB — skipping 0.8.5 improver E2E');

    // Click the ✨ button on the first QP card.
    await improveBtns.first().click();

    // Verify the create POST hit with the expected shape: skill_ids
    // must contain 'qp-improver', and the initial_prompt must carry
    // both the human-language instruction AND a fenced ```json block.
    await expect.poll(() => lastCreatePayload, { timeout: 5_000 }).not.toBeNull();
    const payload = lastCreatePayload!;
    expect(Array.isArray(payload.skill_ids)).toBe(true);
    expect(payload.skill_ids as string[]).toContain('qp-improver');
    expect(typeof payload.initial_prompt).toBe('string');
    expect(payload.initial_prompt as string).toMatch(/```json/);
    // Title is prefixed for the deploy banner to recover the target id.
    expect(payload.title as string).toMatch(/^\[Improve QP /);
  });

  test('QP form exposes the bindings accordion (skills + profiles + directives)', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    const workflows = new WorkflowsPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();
    await workflows.clickQuickPromptsTab();

    // If a QP exists, edit it; otherwise click "New prompt" via the eye-
    // edit on any first QP card. Either path opens the form.
    const eyeBtns = page.locator('button[title="Edit"]');
    const hasQp = await eyeBtns.count();
    if (hasQp === 0) {
      // No QPs: open the new-prompt form via the header button.
      await workflows.newPromptButton.click();
    } else {
      await eyeBtns.first().click();
    }

    // The form must render. Bindings block is OPTIONAL — it only shows
    // when at least one catalog (skills / profiles / directives) is
    // non-empty. In dev DBs we usually have builtins, so we assert it
    // exists. Scope the locators inside the bindings block — if the
    // full-suite run scrolled the form off-screen, scrollIntoViewIfNeeded
    // brings it back before the assertion.
    const bindings = page.locator('[data-testid="qp-bindings"]');
    // 15s budget: catalogs load async + form mount can be slow when
    // the full PW suite has loaded many prior specs.
    await expect(bindings).toBeVisible({ timeout: 15_000 });
    await bindings.scrollIntoViewIfNeeded();

    // Pin the contract: AT LEAST the skills toggle must be there
    // (builtin skills always ship). Profile + directive sections are
    // visible only when those catalogs have entries — both come pre-
    // seeded in fresh dev DBs but could be wiped by a prior spec.
    await expect(bindings.locator('[data-testid="qp-bindings-skills-toggle"]')).toBeVisible();

    // Opening the skills section expands the chip row — this is the
    // load-bearing UX behaviour the test really cares about.
    await bindings.locator('[data-testid="qp-bindings-skills-toggle"]').click();
    await expect(bindings.locator('[data-testid="qp-bindings-skills-chips"]')).toBeVisible();
  });
});
