/**
 * Run detail — WaitingApproval status badge (amber, NOT red).
 *
 * Regression guard for the 0.7.0 banner colour fix : a paused-on-Gate run
 * must surface as amber + Hand icon ("À VALIDER") rather than the
 * previous red-bucket "Run terminé" treatment that screamed failure.
 *
 * The LIVE banner (in WorkflowDetail.tsx) appears only after an SSE
 * trigger emits run_done with status=WaitingApproval — non-trivial to
 * stub end-to-end. The same colour mapping renders on the persistent
 * RunDetail (the per-run card in the runs list), which is the easier
 * surface to drive : we stub the run row and assert the badge.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

const WF_ID = 'e2e-wf-waiting';
const RUN_ID = 'e2e-run-waiting';

const PAUSED_RUN = {
  id: RUN_ID,
  workflow_id: WF_ID,
  status: 'WaitingApproval',
  trigger_context: null,
  step_results: [
    {
      step_name: 'plan_gate',
      status: 'WaitingApproval',
      output: '## Plan validation\n\nApprove to start implementation, or request changes.',
      tokens_used: 0,
      duration_ms: 0,
      condition_result: null,
      envelope_detected: null,
      step_kind: 'Gate',
      step_agent: null,
      step_api_plugin_slug: null,
      step_api_endpoint_path: null,
    },
  ],
  tokens_used: 0,
  workspace_path: null,
  started_at: '2026-05-05T10:00:00Z',
  finished_at: null,
  run_type: 'linear',
  batch_total: 0,
  batch_completed: 0,
  batch_failed: 0,
  batch_name: null,
  parent_run_id: null,
  state: {},
};

const WORKFLOW_FIXTURE = {
  id: WF_ID,
  name: 'E2E Waiting Banner',
  description: 'fixture',
  trigger: { type: 'manual' },
  steps: [{
    name: 'plan_gate',
    step_type: { type: 'Gate' },
    description: null,
    agent: 'ClaudeCode',
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
    gate_message: 'Validate the plan',
    gate_request_changes_target: null,
    gate_notify_url: null,
    exec_command: null,
    exec_args: [],
    exec_timeout_secs: null,
    quick_prompt_id: null,
    json_data_payload: null,
    quick_api_id: null,
    batch_concurrent_limit: null,
  }],
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

test.describe('Run detail — WaitingApproval surfaced as amber (not red)', () => {
  test('paused-on-Gate run shows the À VALIDER badge with amber tone', async ({ page }) => {
    await page.route('**/api/workflows', route =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: [{
            ...WORKFLOW_FIXTURE,
            step_count: 1,
            trigger_type: 'manual',
            last_run: {
              id: RUN_ID,
              status: 'WaitingApproval',
              started_at: PAUSED_RUN.started_at,
              finished_at: null,
              tokens_used: 0,
            },
          }],
          error: null,
        }),
      })
    );

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
        body: JSON.stringify({ success: true, data: [PAUSED_RUN], error: null }),
      })
    );

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickWorkflows();
    await page.locator('.wf-card').filter({ hasText: 'E2E Waiting Banner' }).click();

    // The run row should be visible and tagged with the WaitingApproval
    // badge. We assert the badge class (which encodes the amber tone)
    // rather than the localized text, to stay locale-independent.
    const runCard = page.locator('.wf-run-card').first();
    await expect(runCard).toBeVisible({ timeout: 5_000 });

    // The dedicated "to-review" badge — distinct from the generic
    // status pill that fires on Success/Failed/Cancelled.
    await expect(runCard.locator('.wf-run-status-badge--gate')).toBeVisible();
    // Sanity: the failed-bucket pill should NOT be present.
    await expect(
      runCard.locator('span').filter({ hasText: /^Failed$/ })
    ).toHaveCount(0);
  });
});
