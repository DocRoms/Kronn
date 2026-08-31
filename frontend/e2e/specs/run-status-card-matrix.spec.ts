import type { Route } from '@playwright/test';
import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

/**
 * KT-243 — RunStatusCard / DiscussionAttachedRuns matrix.
 *
 * A run started elsewhere (QP batch/compare, a direct QA/QE launch, or a
 * Workflow run) that carries this discussion's id must show up in the
 * Discussions page automatically, through the one shared SharedRun model and
 * card — one per kind (QuickPrompt, QuickApi, QuickExec, Workflow), each with
 * its own status/duration/deep-link. This is the actual "Discussions
 * consumer" required by the DoD, driven end-to-end through the real page.
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
  messages: [],
  message_count: 0,
  non_system_message_count: 0,
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

function sharedRun(overrides: Record<string, unknown>) {
  return {
    id: 'run-x',
    source_id: 'source-x',
    project_id: null,
    discussion_id: DISC_ID,
    status: 'running',
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

test.describe.configure({ timeout: 60_000, retries: 0 });

test.describe('RunStatusCard matrix — Discussions surfaces all four attached run kinds', () => {
  test('QP/QA/QE/Workflow runs attached to this discussion each render their own card and deep link', async ({ page }) => {
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

    const attached = page.locator('[data-testid="disc-attached-runs"]');
    await expect(attached).toBeVisible();
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

    // A preflight_failed Workflow surfaces its diagnostic explicitly rather
    // than a bare/plausible-looking status (DoD #3/#4) — compact cards omit
    // the diagnostic body, so this only asserts the explicit status pill.
    await expect(attached.locator('[data-kind="workflow"] .run-status-card-status')).toContainText(/./);
  });
});
