/**
 * Audit card & tab states — after audit / around revalidation (0.9.0).
 *
 * Born from a live dogfooding session where every one of these states
 * broke at least once in a single day:
 *
 *   1. An audit launched OUTSIDE the UI (MCP bridge) was invisible on the
 *      ProjectCard — it even offered to launch a second audit (F7).
 *   2. While an audit runs, the compact header keeps one honest next action;
 *      when idle, the single verification entry point comes back.
 *   3. The Projets nav tab carries the fleet-wide running-audits badge.
 *   4. The drift-refresh cycle runs through “Vérifier la doc”; after a
 *      successful partial refresh the four-count summary becomes clean.
 *
 * Cost model: ZERO Claude tokens — audit-status/drift/partial-audit are
 * route-mocked; unmocked routes hit the real local backend as usual.
 */
import { test, expect } from '../fixtures/kronn-fixture';
import type { Page, Route } from '@playwright/test';
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

/** Select the mocked project explicitly: mobile starts on the master list,
 *  and persisted desktop selection may point at a project outside this fixture. */
async function selectFixtureProject(page: Page, projectId: string) {
  const row = page.getByTestId(`project-list-item-${projectId}`);
  await expect(row).toBeVisible({ timeout: 10_000 });
  await row.locator('.disc-item-open').click();
  await expect(page.locator(`#project-detail-${projectId}`)).toBeVisible({ timeout: 10_000 });
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
          fresh_sections: state.stale ? [] : ['docs/repo-map.md'],
          total_sections: 3,
          sources_state: state.stale ? 'needs_reverification' : 'clean',
          broken_citations: 0,
          placeholder_docs: [],
        },
        error: null,
      }),
    });
  });
  return { setClean: () => { state.stale = false; } };
}

/** Read-only prerequisites of the single verification flow. */
function mockVerificationSurface(page: Page, projectId: string) {
  const json = (data: unknown) => ({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({ success: true, data, error: null }),
  });
  page.route(`**/api/projects/${projectId}/doc-provenance*`, route => route.fulfill(json({ docs: [], to_reconfirm: 0 })));
  page.route(`**/api/projects/${projectId}/audit-timeline`, route => route.fulfill(json({
    runs: [{
      id: 'run-1', project_id: projectId, kind: 'Full', agent_type: 'ClaudeCode',
      started_at: '2026-07-19T10:00:00Z', ended_at: '2026-07-19T10:03:00Z',
      duration_ms: 180_000, status: 'Completed', td_total: 0,
    }],
    steps: [3, 8, 9].map(step => ({
      audit_run_id: 'run-1', step_index: step, file_label: `docs/step-${step}.md`,
      started_at: '2026-07-19T10:00:00Z', ended_at: '2026-07-19T10:01:00Z',
      duration_ms: 60_000, cli_success: true, cost_usd_micros: 10_000,
    })),
    recorded_audits: [], recorded_validated_at: null, latest_validation: null,
    lease_orphaned: false,
  })));
  page.route('**/api/audit/steps', route => route.fulfill(json([
    { index: 3, target_file: 'docs/repo-map.md' },
    { index: 8, target_file: 'docs/inconsistencies-tech-debt.md' },
    { index: 9, target_file: 'docs/decisions.md' },
  ])));
  page.route(`**/api/projects/${projectId}/revalidation`, route => route.fulfill(json({
    sources_state: 'needs_reverification', sections: [], agent_steps: [],
    reanchor_count: 0, human_count: 0,
  })));
  page.route('**/api/audit-runs/run-1/cost', route => route.fulfill(json({
    run_id: 'run-1', briefing: {}, audit: {}, validation: {}, total: { cost_usd_micros: 30_000 },
  })));
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

test.describe('Audit card & tab states (0.9.0 dogfooding)', () => {
  test('card adopts an externally-launched audit, locks CTAs, then releases', async ({ page }) => {
    const projectId = 'pw-audit-card-fixture';
    mockProjectsList(page, projectId);
    mockDrift(page, projectId);
    mockVerificationSurface(page, projectId);
    const audit = mockAuditStatus(page, projectId);

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await selectFixtureProject(page, projectId);

    // Idle: one doc-state summary and one verification entry point.
    await expect(page.getByTestId('project-doc-state-summary')).toContainText('3 à vérifier', { timeout: 10_000 });
    await expect(page.getByTestId('project-revalidate')).toContainText('Vérifier la doc');
    await expect(page.getByTestId('project-audit-progress')).toHaveCount(0);

    // An audit starts OUTSIDE this card (MCP bridge): the fleet poll sees
    // it, the card adopts it — orange badge with the SEQUENTIAL counter and
    // the primary action becomes a link to progress, never another launch.
    audit.setRunning({ step_index: 2, total_steps: 3 });
    await expect(page.getByTestId('project-audit-progress')).toContainText('2/3', { timeout: 15_000 });
    await expect(page.getByTestId('project-revalidate')).toContainText('Voir la progression');

    // Projets nav tab carries the fleet badge (1 running audit).
    await expect(page.locator('[data-tour-id="nav-projects"] .dash-nav-badge'))
      .toHaveText('1', { timeout: 10_000 });

    // Audit ends server-side: badge and lock must release — no phantom
    // "Audits en cours" leftovers (the drop-guard regression family).
    audit.setIdle();
    await expect(page.getByTestId('project-audit-progress')).toHaveCount(0, { timeout: 15_000 });
    await expect(page.getByTestId('project-revalidate')).toContainText('Vérifier la doc', { timeout: 10_000 });
  });

  test('drift refresh cycle: stale badges → update → clean card', async ({ page }) => {
    const projectId = 'pw-drift-cycle-fixture';
    mockProjectsList(page, projectId);
    const drift = mockDrift(page, projectId);
    mockVerificationSurface(page, projectId);
    mockAuditStatus(page, projectId); // stays idle — the SSE drives the UI here
    mockPartialAuditStream(page, projectId);

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await selectFixtureProject(page, projectId);

    // Stale state is explained behind the one verification entry point.
    await expect(page.getByTestId('project-doc-state-summary')).toContainText('3 à vérifier', { timeout: 10_000 });
    await page.getByTestId('project-revalidate').click();
    const updateBtn = page.getByTestId('revalidate-stale-update');
    await expect(page.getByTestId('revalidate-stale-details')).toContainText('Le code cité par ces sections a changé');
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
    await expect(page.locator('.project-detail-card')).toHaveCount(0, { timeout: 10_000 });

    // Back on Projets: the card is remounted, clean, with no progress chip.
    await dashboard.clickProjects();
    await selectFixtureProject(page, projectId);
    await expect(page.getByRole('heading', { name: 'audit-card-fixture' }))
      .toBeVisible({ timeout: 10_000 });
    await expect(page.getByTestId('project-doc-state-summary')).toContainText('à jour', { timeout: 15_000 });
    await expect(page.getByTestId('project-audit-progress')).toHaveCount(0);
  });

  test('compact audit header controls stay inside a 360 px viewport without overlap', async ({ page }) => {
    const projectId = 'pw-audit-narrow-fixture';
    await page.setViewportSize({ width: 360, height: 800 });
    mockProjectsList(page, projectId);
    mockDrift(page, projectId);
    mockVerificationSurface(page, projectId);
    mockAuditStatus(page, projectId);

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await selectFixtureProject(page, projectId);
    await expect(page.getByTestId('project-doc-state-summary')).toContainText('3 à vérifier', { timeout: 10_000 });

    const controls = page.locator([
      '.project-detail-actions > .project-status-chip',
      '.project-detail-actions > .project-audit-primary',
      '.project-detail-actions > .doc-state-summary',
      '.project-detail-actions > .project-audit-details > summary',
      '.dash-card-header .project-status-chip',
      '.dash-card-header .project-audit-primary',
      '.dash-card-header .doc-state-summary',
      '.dash-card-header .project-audit-details > summary',
    ].join(', '));
    await expect(controls).toHaveCount(4);
    const boxes = await controls.evaluateAll(elements => elements.map(element => {
      const box = element.getBoundingClientRect();
      return { left: box.left, right: box.right, top: box.top, bottom: box.bottom };
    }));
    for (const box of boxes) {
      expect(box.left).toBeGreaterThanOrEqual(0);
      expect(box.right).toBeLessThanOrEqual(360);
    }
    for (let left = 0; left < boxes.length; left += 1) {
      for (let right = left + 1; right < boxes.length; right += 1) {
        const overlapWidth = Math.min(boxes[left].right, boxes[right].right) - Math.max(boxes[left].left, boxes[right].left);
        const overlapHeight = Math.min(boxes[left].bottom, boxes[right].bottom) - Math.max(boxes[left].top, boxes[right].top);
        expect(Math.max(0, overlapWidth) * Math.max(0, overlapHeight)).toBe(0);
      }
    }
  });
});
