import type { Route } from '@playwright/test';
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import type { DiscussionAction, DiscussionMessage, SharedRun } from '../../src/types/generated';

/**
 * RunStatusCard matrix through the current inline discussion actions.
 *
 * The separate attached-runs strip was intentionally removed in fc12e85a.
 * Each action now owns its run card inside its source message. Keep all four
 * kinds, independent statuses and deep links covered through that real page
 * consumer, and guard against a duplicate strip outside the conversation.
 */

const DISC_ID = 'e2e-runs-disc';
const TITLE = 'E2E — attached runs matrix';

const discussion = {
  id: DISC_ID,
  project_id: null,
  title: TITLE,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: ['qp', 'qa', 'qe', 'wf'].map(kind => ({
    id: `msg-${kind}`,
    role: 'Agent',
    channel: 'main',
    content: `Action ${kind}`,
    agent_type: 'ClaudeCode',
    timestamp: '2026-01-01T00:00:00Z',
    tokens_used: 0,
    auth_mode: null,
  } satisfies DiscussionMessage)),
  message_count: 4,
  non_system_message_count: 4,
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

const listRow = { ...discussion };

function sharedRun(overrides: Pick<SharedRun, 'id' | 'kind' | 'source_id' | 'status'> & Partial<SharedRun>): SharedRun {
  return {
    project_id: null,
    discussion_id: DISC_ID,
    started_at: '2026-01-01T00:00:00Z',
    finished_at: null,
    duration_ms: null,
    result: null,
    diagnostic: null,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  };
}

const RUNS = [
  sharedRun({ id: 'run-qp', kind: 'quick_prompt', source_id: 'qp-1', status: 'success' }),
  sharedRun({ id: 'run-qa', kind: 'quick_api', source_id: 'qa-1', status: 'running' }),
  sharedRun({ id: 'run-qe', kind: 'quick_exec', source_id: 'qe-1', status: 'failed', diagnostic: 'Command exited 1' }),
  sharedRun({ id: 'run-wf', kind: 'workflow', source_id: 'wf-1', status: 'preflight_failed', diagnostic: 'Missing required variable' }),
];

const ACTIONS: DiscussionAction[] = RUNS.map((run, index) => ({
  id: `action-${run.id}`,
  discussion_id: DISC_ID,
  source_message_id: discussion.messages[index].id,
  fence_index: 0,
  kind: run.kind === 'media' ? 'invalid' : run.kind,
  target_id: run.source_id,
  target_name: `Target ${run.id}`,
  project_id: null,
  project_name: null,
  state: run.status === 'success' ? 'succeeded'
    : run.status === 'running' ? 'running'
      : run.status === 'preflight_failed' ? 'preflight_failed' : 'failed',
  values: [],
  shared_run_id: run.id,
  result_discussion_id: DISC_ID,
  deep_link: `/discussions/${DISC_ID}`,
  diagnostic: run.diagnostic,
  launched_at: run.started_at,
  finished_at: run.finished_at,
  created_at: run.created_at,
  updated_at: run.updated_at,
}));

test.describe.configure({ timeout: 60_000, retries: 0 });

test.describe('RunStatusCard matrix — Discussions surfaces all four inline run kinds', () => {
  test('QP/QA/QE/Workflow actions each render their own inline run card and deep link', async ({ page }) => {
    await page.route('**/api/discussions', async (route: Route) => {
      if (route.request().method() !== 'GET') return route.fallback();
      await route.fulfill({ json: { success: true, data: [listRow] } });
    });
    await page.route(`**/api/discussions/${DISC_ID}`, async (route: Route) => {
      if (route.request().method() !== 'GET') return route.fallback();
      await route.fulfill({ json: { success: true, data: discussion } });
    });
    await page.route(`**/api/runs?discussion_id=${DISC_ID}*`, async (route: Route) => {
      if (route.request().method() !== 'GET') return route.fallback();
      await route.fulfill({ json: { success: true, data: RUNS, error: null } });
    });
    await page.route(`**/api/discussions/${DISC_ID}/actions`, async route => {
      await route.fulfill({ json: { success: true, data: ACTIONS, error: null } });
    });
    for (const action of ACTIONS) {
      await page.route(`**/api/discussion-actions/${action.id}`, async route => {
        await route.fulfill({ json: { success: true, data: action, error: null } });
      });
    }
    // Each card also self-hydrates by id — serve the matching single-run
    // fixture for every one of the four kinds.
    for (const run of RUNS) {
      await page.route(`**/api/runs/${run.id}`, async (route: Route) => {
        if (route.request().method() !== 'GET') return route.fallback();
        await route.fulfill({ json: { success: true, data: run, error: null } });
      });
    }

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.navDiscussions.click();
    await page.getByText(TITLE, { exact: false }).first().click();

    const attached = page.locator('.disc-messages');
    await expect(attached).toBeVisible();
    await expect(page.getByTestId('disc-attached-runs')).toHaveCount(0);
    await expect(attached.locator('.discussion-action-card')).toHaveCount(4);
    // A real scroll cancels the discussion's initial bottom-settling window;
    // Playwright's automatic element scroll does not express that user intent.
    await attached.hover();
    await page.mouse.wheel(0, -1000);
    for (const [index, action] of ACTIONS.entries()) {
      const sourceMessage = attached.locator(`[data-message-id="${action.source_message_id}"]`);
      const card = sourceMessage.getByTestId(`discussion-action-${action.id}`);
      await expect(card).toHaveAttribute('data-state', action.state);
      await card.locator('.discussion-action-card__toggle').click();
      const runCard = card.getByTestId('run-status-card');
      await expect(runCard).toHaveCount(1);
      // Cards deliberately suspend hydration outside the viewport. Read each
      // one as a user would before scrolling to the next action.
      await runCard.scrollIntoViewIfNeeded();
      await expect(runCard).toHaveAttribute('data-kind', RUNS[index].kind);
      await expect(runCard).toHaveAttribute('data-status', RUNS[index].status);
    }
    const cards = attached.locator('[data-testid="run-status-card"]');
    await expect(cards).toHaveCount(4);

    // Each kind is present with its own status, independent of the others.
    await expect(attached.locator('[data-kind="quick_prompt"][data-status="success"]')).toBeVisible();
    await expect(attached.locator('[data-kind="quick_api"][data-status="running"]')).toBeVisible();
    await expect(attached.locator('[data-kind="quick_exec"][data-status="failed"]')).toBeVisible();
    await expect(attached.locator('[data-kind="workflow"][data-status="preflight_failed"]')).toBeVisible();

    // Deep links: attached runs are always scoped to this discussion.
    const qpLink = attached.locator('[data-kind="quick_prompt"] .run-status-card-link');
    await expect(qpLink).toHaveAttribute('href', `/discussions/${DISC_ID}`);
    const qaLink = attached.locator('[data-kind="quick_api"] .run-status-card-link');
    await expect(qaLink).toHaveAttribute('href', `/discussions/${DISC_ID}`);

    // Terminal inline cards expose the diagnostic as well as the status.
    await expect(attached.locator('[data-kind="workflow"] .run-status-card-status')).toContainText(/./);
    await expect(attached.locator('[data-kind="workflow"] .run-status-card-diagnostic'))
      .toContainText('Missing required variable');
    await expect(attached.locator('[data-kind="quick_exec"] .run-status-card-diagnostic'))
      .toContainText('Command exited 1');
  });
});
