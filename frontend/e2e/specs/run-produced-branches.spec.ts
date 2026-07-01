/**
 * Run detail — Produced branches panel.
 *
 * Regression guard for the 0.7.0 P1 + P2 work : when a workflow run
 * carries `produced_branches`, the run detail page surfaces an amber
 * panel listing each branch with copy-checkout / copy-diff / "test in
 * worktree" buttons. Clicking "test" hits POST /test-worktree and the
 * returned path appears inline.
 *
 * Strategy : stub the workflow listing + run detail endpoints so the test
 * is deterministic (no dependency on whatever runs happen to be in the
 * dev DB). The test-worktree POST is also stubbed so we don't actually
 * spawn a real `git worktree add` from within the test.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

const WF_ID = 'e2e-wf-produced';
const RUN_ID = 'e2e-run-produced';

const BRANCH = {
  branch_name: 'kronn/Autobot/e2erun1',
  head_sha: 'abcdef1234567890fedcba0987654321deadbeef',
  ahead: 1,
  pushed_upstream: false,
};

const RUN_FIXTURE = {
  id: RUN_ID,
  workflow_id: WF_ID,
  status: 'Success',
  trigger_context: null,
  step_results: [
    {
      step_name: 'implement',
      status: 'Success',
      output: 'Done.',
      tokens_used: 100,
      duration_ms: 1234,
      condition_result: null,
      envelope_detected: null,
      step_kind: 'Agent',
      step_agent: 'ClaudeCode',
      step_api_plugin_slug: null,
      step_api_endpoint_path: null,
    },
  ],
  tokens_used: 100,
  workspace_path: null,
  started_at: '2026-05-05T10:00:00Z',
  finished_at: '2026-05-05T10:05:00Z',
  run_type: 'linear',
  batch_total: 0,
  batch_completed: 0,
  batch_failed: 0,
  batch_name: null,
  parent_run_id: null,
  state: {},
  produced_branches: [BRANCH],
};

const WORKFLOW_FIXTURE = {
  id: WF_ID,
  name: 'E2E Produced Branches',
  description: 'fixture',
  trigger: { type: 'manual' },
  steps: RUN_FIXTURE.step_results.map(r => ({
    name: r.step_name,
    step_type: { type: r.step_kind },
    description: null,
    agent: r.step_agent,
    prompt_template: '',
    mode: { type: 'Normal' },
    output_format: { type: 'FreeText' },
    mcp_config_ids: [],
    agent_settings: null,
    on_result: [],
    stall_timeout_secs: null,
    retry: null,
    delay_after_secs: null,
    skill_ids: [],
    profile_ids: [],
    directive_ids: [],
    batch_quick_prompt_id: null,
    batch_items_from: null,
    batch_wait_for_completion: null,
    batch_max_items: null,
    batch_workspace_mode: null,
    batch_chain_prompt_ids: [],
    notify_config: null,
    api_plugin_slug: null,
    api_config_id: null,
    api_endpoint_path: null,
    api_method: null,
    api_query: null,
    api_headers: null,
    api_body: null,
    api_extract: null,
    api_pagination: null,
    api_timeout_ms: null,
    api_max_retries: null,
    api_output_var: null,
    gate_message: null,
    gate_request_changes_target: null,
    gate_notify_url: null,
    exec_command: null,
    exec_args: [],
    exec_timeout_secs: null,
    quick_prompt_id: null,
    json_data_payload: null,
    quick_api_id: null,
    batch_concurrent_limit: null,
  })),
  actions: [],
  enabled: true,
  project_id: null,
  project_name: null,
  created_at: '2026-05-05T10:00:00Z',
  updated_at: '2026-05-05T10:00:00Z',
  variables: [],
  exec_allowlist: [],
  on_failure: [],
  artifacts: {},
};

test.describe('Run detail — produced branches panel', () => {
  test('displays branch + actions, and shows path after Test-in-worktree', async ({ page }) => {
    // Stub the listing + detail endpoints.
    await page.route('**/api/workflows', async (route, req) => {
      if (req.method() === 'GET') {
        return route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            success: true,
            data: [{
              ...WORKFLOW_FIXTURE,
              step_count: WORKFLOW_FIXTURE.steps.length,
              trigger_type: 'manual',
              last_run: {
                id: RUN_ID,
                status: 'Success',
                started_at: RUN_FIXTURE.started_at,
                finished_at: RUN_FIXTURE.finished_at,
                tokens_used: RUN_FIXTURE.tokens_used,
              },
            }],
            error: null,
          }),
        });
      }
      return route.continue();
    });

    await page.route(`**/api/workflows/${WF_ID}`, route =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: WORKFLOW_FIXTURE, error: null }),
      })
    );

    await page.route(`**/api/workflows/${WF_ID}/runs`, route =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: [RUN_FIXTURE], error: null }),
      })
    );

    await page.route(`**/api/workflows/${WF_ID}/runs/${RUN_ID}`, route => {
      if (route.request().method() === 'GET') {
        return route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ success: true, data: RUN_FIXTURE, error: null }),
        });
      }
      return route.continue();
    });

    // The "Test in worktree" button POSTs here. Stub a synthetic path.
    await page.route(`**/api/workflows/${WF_ID}/runs/${RUN_ID}/test-worktree`, route => {
      if (route.request().method() === 'POST') {
        return route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            success: true,
            data: {
              worktree_path: '/tmp/e2e-test-worktree/kronn-Autobot-e2erun1',
              branch_name: BRANCH.branch_name,
              head_sha: BRANCH.head_sha,
            },
            error: null,
          }),
        });
      }
      return route.continue();
    });

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();

    // Open the workflow card → loads detail + runs.
    await page.locator('.wf-card').filter({ hasText: 'E2E Produced Branches' }).click();

    // #6 compact rows — a terminal (Success) run renders as a collapsed
    // one-line row; the full RunDetail (.wf-run-card) only mounts after
    // expanding it. Click the compact row first.
    const compactRow = page.locator('.wf-run-compact').first();
    await expect(compactRow).toBeVisible({ timeout: 5_000 });
    await compactRow.click();

    const runRow = page.locator('.wf-run-card').first();
    await expect(runRow).toBeVisible({ timeout: 5_000 });

    // ProducedBranches panel should be visible inside the run card.
    await expect(runRow.locator('.wf-produced-branches')).toBeVisible({ timeout: 5_000 });
    // Branch name + short SHA + "local only" pill.
    await expect(runRow.locator('.wf-produced-branches')).toContainText(BRANCH.branch_name);
    await expect(runRow.locator('.wf-produced-branches')).toContainText(BRANCH.head_sha.slice(0, 8));

    // Click "Tester en worktree" button.
    const testBtn = runRow.locator('.wf-produced-test-btn').first();
    await expect(testBtn).toBeVisible();
    await testBtn.click();

    // The synthetic path should appear inline after the POST resolves.
    await expect(runRow.locator('.wf-produced-test-path'))
      .toContainText('/tmp/e2e-test-worktree/kronn-Autobot-e2erun1', { timeout: 5_000 });
  });
});
