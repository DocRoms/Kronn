/**
 * Audit card & tab states — after audit / around revalidation (0.8.13).
 *
 * Born from a live dogfooding session where every one of these states
 * broke at least once in a single day:
 *
 *   1. An audit launched OUTSIDE the UI (MCP bridge) was invisible on the
 *      ProjectCard — it even offered to launch a second audit (F7).
 *   2. While an audit runs, the drift-update button must hide; when idle,
 *      badges + CTAs must come back (no phantom "in progress" leftovers).
 *   3. The Projets nav tab carries the fleet-wide running-audits badge.
 *   4. The drift-refresh cycle: "N section(s) obsolète(s)" + update button
 *      shown when stale; after a successful partial refresh the drift
 *      re-fetch clears them.
 *
 * Cost model: ZERO Claude tokens — audit-status/drift/partial-audit are
 * route-mocked; unmocked routes hit the real local backend as usual.
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

function progressFixture(projectId: string, overrides: Partial<AuditProgress> = {}): AuditProgress {
  return {
    project_id: projectId,
    phase: 'auditing',
    step_index: 2,
    total_steps: 3,
    current_file: 'docs/repo-map.md',
    started_at: new Date().toISOString(),
    kind: 'partial',
    ...overrides,
  };
}

/** Per-project + fleet audit-status, flippable between polls. */
function mockAuditStatus(page: Page, projectId: string) {
  const state: { progress: AuditProgress | null } = { progress: null };
  page.route(`**/api/projects/${projectId}/audit-status`, async (route: Route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ success: true, data: state.progress, error: null }),
    });
  });
  // Fleet endpoint drives the Projets nav badge + per-card adoption.
  page.route('**/api/audit-status', async (route: Route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: state.progress ? [state.progress] : [],
        error: null,
      }),
    });
  });
  return {
    setRunning: (overrides: Partial<AuditProgress> = {}) => {
      state.progress = progressFixture(projectId, overrides);
    },
    setIdle: () => { state.progress = null; },
  };
}

function mockProjectsList(page: Page, projectId: string) {
  page.route('**/api/projects', async (route: Route) => {
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
          name: 'audit-card-fixture',
          path: '/tmp/audit-card-fixture',
          repo_url: null,
          token_override: null,
          ai_config: { detected: false, configs: [] },
          audit_status: 'Validated',
          ai_todo_count: 0,
          tech_debt_count: 44,
          needs_docs_migration: false,
          default_skill_ids: [],
          briefing_notes: null,
          linked_repos: [],
          created_at: '2026-01-01T00:00:00Z',
          updated_at: '2026-01-01T00:00:00Z',
        }],
        error: null,
      }),
    });
  });
}

/** Drift endpoint, flippable stale ↔ clean between fetches. */
function mockDrift(page: Page, projectId: string) {
  const state = { stale: true };
  page.route(`**/api/projects/${projectId}/drift`, async (route: Route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        success: true,
        data: {
          stale_sections: state.stale
            ? [
                { ai_file: 'docs/repo-map.md', audit_step: 3, changed_sources: ['__GIT_LS_FILES__'] },
                { ai_file: 'docs/inconsistencies-tech-debt.md', audit_step: 8, changed_sources: ['__GIT_HEAD__'] },
                { ai_file: 'docs/decisions.md', audit_step: 9, changed_sources: ['__GIT_HEAD__'] },
              ]
            : [],
          audit_date: '2026-07-19',
        },
        error: null,
      }),
    });
  });
  return { setClean: () => { state.stale = false; } };
}

/** Partial-audit SSE: sequential `progress` alongside the canonical `step`
 *  ("Étape 8/3" regression — the UI must read progress, not step). */
function mockPartialAuditStream(page: Page, projectId: string) {
  page.route(`**/api/projects/${projectId}/partial-audit`, async (route: Route) => {
    // EXACT mirror of the backend stream (matrix v2): `start` carries the
    // canonical requested_steps, a fully-successful partial emits
    // `validation_created` before `done`, and `done` carries the exact
    // partition + both ids — a payload without them is refused by the
    // runtime validator (onError), not silently accepted.
    const sse = [
      ['start', { total_steps: 3, requested_steps: [3, 8, 9] }],
      ['step_start', { step: 3, progress: 1, total: 3, file: 'docs/repo-map.md' }],
      ['step_done', { step: 3, success: true }],
      ['step_start', { step: 8, progress: 2, total: 3, file: 'docs/inconsistencies-tech-debt.md' }],
      ['step_done', { step: 8, success: true }],
      ['step_start', { step: 9, progress: 3, total: 3, file: 'docs/decisions.md' }],
      ['step_done', { step: 9, success: true }],
      ['validation_created', { discussion_id: 'd-scoped' }],
      ['done', { status: 'complete', total_steps: 3,
                 succeeded_steps: [3, 8, 9], unchanged_steps: [], failed_steps: [],
                 discussion_id: 'd-scoped', audit_run_id: 'run-1' }],
    ].map(([event, data]) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`).join('');
    await route.fulfill({
      status: 200,
      contentType: 'text/event-stream',
      body: sse,
    });
  });
}

test.describe('Audit card & tab states (0.8.13 dogfooding)', () => {
  test.skip(!!process.env.CI,
    'route-mocked spec — local-only for now (same rationale as audit-banner-lifecycle).');

  test('card adopts an externally-launched audit, locks CTAs, then releases', async ({ page }) => {
    const projectId = 'pw-audit-card-fixture';
    mockProjectsList(page, projectId);
    mockDrift(page, projectId);
    const audit = mockAuditStatus(page, projectId);

    const dashboard = new DashboardPage(page);
    await dashboard.goto();

    // Idle: drift affordances visible, no running badge anywhere.
    await expect(page.getByText('3 section(s) obsolète(s)')).toBeVisible({ timeout: 10_000 });
    await expect(page.getByText(/AI audit \d+\/\d+/)).toHaveCount(0);

    // An audit starts OUTSIDE this card (MCP bridge): the fleet poll sees
    // it, the card adopts it — orange badge with the SEQUENTIAL counter,
    // drift-update affordance hidden while running.
    audit.setRunning({ step_index: 2, total_steps: 3 });
    await expect(page.getByText('AI audit 2/3')).toBeVisible({ timeout: 15_000 });
    await expect(page.getByText('Mettre à jour (3 sections)')).toHaveCount(0);

    // Projets nav tab carries the fleet badge (1 running audit).
    await expect(page.locator('nav').getByText('1', { exact: true })).toBeVisible({ timeout: 10_000 });

    // Audit ends server-side: badge and lock must release — no phantom
    // "Audits en cours" leftovers (the drop-guard regression family).
    audit.setIdle();
    await expect(page.getByText(/AI audit \d+\/\d+/)).toHaveCount(0, { timeout: 15_000 });
    await expect(page.getByText('Mettre à jour (3 sections)')).toBeVisible({ timeout: 10_000 });
  });

  test('drift refresh cycle: stale badges → update → clean card', async ({ page }) => {
    const projectId = 'pw-drift-cycle-fixture';
    mockProjectsList(page, projectId);
    const drift = mockDrift(page, projectId);
    mockAuditStatus(page, projectId); // stays idle — the SSE drives the UI here
    mockPartialAuditStream(page, projectId);

    const dashboard = new DashboardPage(page);
    await dashboard.goto();

    // Stale state renders both affordances.
    const updateBtn = page.getByText('Mettre à jour (3 sections)').first();
    await expect(page.getByText('3 section(s) obsolète(s)')).toBeVisible({ timeout: 10_000 });
    await expect(updateBtn).toBeVisible();

    // Launch the refresh; the mocked stream completes instantly and the
    // card re-fetches drift — which is clean by then.
    drift.setClean();
    await updateBtn.click();

    // A fully-successful partial: success toast (never the stream-error
    // one), then the card NAVIGATES to Discussions — asserting "stale text
    // absent" alone would be a false positive on an unmounted card.
    await expect(
      page.getByText('3 section(s) mises à jour — discussion de validation créée, on y va'),
    ).toBeVisible({ timeout: 15_000 });
    await expect(page.getByText(/L'audit partiel s'est arrêté sur une erreur/)).toHaveCount(0);
    await expect(page.locator('.dash-proj-name')).toHaveCount(0, { timeout: 10_000 });

    // Back on Projets: the card is remounted, clean, with no progress chip.
    await dashboard.clickProjects();
    await expect(page.locator('.dash-proj-name', { hasText: 'audit-card-fixture' }))
      .toBeVisible({ timeout: 10_000 });
    await expect(page.getByText('3 section(s) obsolète(s)')).toHaveCount(0, { timeout: 15_000 });
    await expect(page.getByText(/AI audit \d+\/\d+/)).toHaveCount(0);
  });
});
