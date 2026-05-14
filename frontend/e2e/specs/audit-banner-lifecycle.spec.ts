/**
 * Audit banner lifecycle — frontend-pure E2E (0.8.3 #282).
 *
 * This spec asserts the cross-component state machine that the 0.8.3
 * unit tests cover in pieces:
 *
 *   1. Audit launched on project X → discussion opened on X shows the
 *      "audit running — MCPs filtered" banner.
 *   2. Audit finishes → banner auto-disappears within ~10s (8s poll
 *      + render).
 *   3. Discussion on a DIFFERENT project (no audit) never shows the
 *      banner.
 *
 * Why a Playwright spec on top of vitest?
 *   - Vitest covers the banner render in isolation (5 tests already).
 *   - This spec exercises the full browser cycle: SSE poll + state
 *     setState + i18n string + class assertion + transition. It's
 *     the only place where a regression in poll-interval, missed
 *     useEffect cleanup, or vite HMR weirdness surfaces.
 *
 * Cost model: ZERO Claude tokens. We mock the backend audit-status
 * endpoint via Playwright route interception — no real agent runs.
 * Setup ~1s, runs ~5s, deterministic across machines.
 */
import { test, expect, type Page, type Route } from '@playwright/test';
import { DashboardPage } from '../pages/DashboardPage';

interface AuditProgress {
  project_id: string;
  phase: string;
  step_index: number;
  total_steps: number;
  current_file: string | null;
  started_at: string;
  kind: string;
}

/** Wrap a value so we can flip it from `null` ↔ progress between
 *  poll requests during the same test. Playwright's `page.route`
 *  fires for every matching request, so the closure captures
 *  whatever the current state is at request-time. */
function mockAuditStatus(page: Page, projectId: string) {
  // Mutable holder — the test calls setRunning / setIdle to flip
  // what the next poll returns. Initial: idle (no audit).
  const state: { progress: AuditProgress | null } = { progress: null };
  page.route(`**/api/projects/${projectId}/audit-status`, async (route: Route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: true, data: state.progress, error: null }),
    });
  });
  return {
    setRunning: (overrides: Partial<AuditProgress> = {}) => {
      state.progress = {
        project_id: projectId,
        phase: 'auditing',
        step_index: 3,
        total_steps: 10,
        current_file: 'docs/AGENTS.md',
        started_at: new Date().toISOString(),
        kind: 'full_audit',
        ...overrides,
      };
    },
    setIdle: () => { state.progress = null; },
  };
}

/** Project list endpoint that returns one project. Banner needs a
 *  real `project_id` matching the active discussion. Same data
 *  shape as the real API. */
function mockProjectsList(page: Page, projectId: string) {
  page.route('**/api/projects', async (route: Route) => {
    // Only mock GET. POST (project creation) goes through.
    if (route.request().method() !== 'GET') {
      await route.continue();
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: [{
          id: projectId,
          name: 'audit-banner-fixture',
          path: '/tmp/audit-banner-fixture',
          repo_url: null,
          token_override: null,
          ai_config: { detected: false, configs: [] },
          audit_status: 'Audited',
          ai_todo_count: 0,
          tech_debt_count: 0,
          needs_docs_migration: false,
          default_skill_ids: [],
          briefing_notes: 'fixture project',
          linked_repos: [],
          created_at: '2026-01-01T00:00:00Z',
          updated_at: '2026-01-01T00:00:00Z',
        }],
        error: null,
      }),
    });
  });
}

/** Discussions list endpoint that returns one project-bound
 *  discussion. The DiscussionsPage opens it and starts polling
 *  audit-status as a result. */
function mockDiscussionsList(page: Page, discId: string, projectId: string) {
  page.route('**/api/discussions', async (route: Route) => {
    if (route.request().method() !== 'GET') {
      await route.continue();
      return;
    }
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: [{
          id: discId,
          project_id: projectId,
          title: 'Question about the project',
          agent: 'ClaudeCode',
          language: 'fr',
          participants: ['ClaudeCode'],
          messages: [],
          message_count: 1,
          archived: false,
          pinned: false,
          workspace_mode: 'Direct',
          created_at: '2026-01-01T00:00:00Z',
          updated_at: '2026-01-01T00:00:00Z',
        }],
        error: null,
      }),
    });
  });
}

/** Locale for stable selector text. Banner copy is i18n'd; we ship
 *  with FR as the default in dev. Switching to EN if the test envs
 *  diverge later is a one-line tweak. */
const BANNER_TEXT_RE = /Audit IA en cours sur ce projet/i;

test.describe('Audit running → MCP filter banner lifecycle (0.8.3 #282)', () => {
  test.skip(!!process.env.CI,
    'route-mocked spec — local-only for now. Mocks cover the SSE poll + DOM transition; CI version would need a fake-disc seed.');

  test('banner appears when audit starts and disappears when it ends', async ({ page }) => {
    const projectId = 'pw-audit-banner-fixture';
    const discId = 'pw-audit-banner-disc';

    // Layer the route handlers BEFORE goto so the very first
    // requests fired by Dashboard mount are caught.
    mockProjectsList(page, projectId);
    mockDiscussionsList(page, discId, projectId);
    const audit = mockAuditStatus(page, projectId);

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.navDiscussions.click();

    // Open the fixture discussion — we click its title from the
    // sidebar. Real selector: it's just a discussion link in the
    // sidebar list, accessible by title.
    await page.getByText('Question about the project').first().click();

    // Phase 1: no audit running → banner must NOT be there.
    // Give the first poll a chance to fire (it's the mount tick).
    await page.waitForTimeout(500);
    await expect(page.getByText(BANNER_TEXT_RE)).toHaveCount(0);

    // Phase 2: flip audit-status to "running". The next poll (8s
    // interval) picks it up. We extend the test timeout to 10s.
    audit.setRunning();
    await expect(page.getByText(BANNER_TEXT_RE)).toBeVisible({ timeout: 10_000 });

    // Phase 3: audit finishes → flip back to null. Banner must
    // disappear by the next poll cycle.
    audit.setIdle();
    await expect(page.getByText(BANNER_TEXT_RE)).toHaveCount(0, { timeout: 10_000 });
  });

  test('banner stays hidden when project has no audit running', async ({ page }) => {
    // Regression guard: a project with audit_status === Audited but
    // NO live run should never trigger the MCP-filter warning.
    // Pre-fix #277, the localStorage-keyed resume logic could flag
    // false positives if a stale checkpoint lingered. Now the
    // backend is the source of truth — null = no banner, full stop.
    const projectId = 'pw-no-banner-fixture';
    const discId = 'pw-no-banner-disc';

    mockProjectsList(page, projectId);
    mockDiscussionsList(page, discId, projectId);
    mockAuditStatus(page, projectId);  // Stays null forever.

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.navDiscussions.click();
    await page.getByText('Question about the project').first().click();

    // Wait long enough for at least one poll tick to fire.
    await page.waitForTimeout(1_000);
    await expect(page.getByText(BANNER_TEXT_RE)).toHaveCount(0);
  });
});
