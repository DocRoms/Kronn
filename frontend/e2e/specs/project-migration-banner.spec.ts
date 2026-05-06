/**
 * ProjectCard — docs migration banner (legacy `ai/` → `docs/`).
 *
 * Locks the operator-facing entry-point to the `migrate-docs` backend
 * endpoint shipped in 0.7.1. We stub `/api/projects` to return one
 * legacy + one already-migrated project, expand both, and assert that:
 *   - the banner only renders on the legacy one
 *   - clicking "Migrer" hits `POST /:id/migrate-docs` with the operator's
 *     `create_symlink` choice
 *   - on success the project list is refetched (banner disappears)
 *
 * Stubs only — no backend disk writes — keeps the spec hermetic.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

const NOW = '2026-01-01T00:00:00Z';

const PROJECTS_PAYLOAD_INITIAL = [
  {
    id: 'p-legacy',
    name: 'LegacyApp',
    path: '/tmp/legacy-app',
    repo_url: null,
    token_override: null,
    ai_config: { detected: false, configs: [] },
    audit_status: 'NoTemplate',
    ai_todo_count: 0,
    needs_docs_migration: true,
    default_skill_ids: [],
    default_profile_id: null,
    briefing_notes: null,
    created_at: NOW,
    updated_at: NOW,
  },
  {
    id: 'p-modern',
    name: 'ModernApp',
    path: '/tmp/modern-app',
    repo_url: null,
    token_override: null,
    ai_config: { detected: false, configs: [] },
    audit_status: 'Audited',
    ai_todo_count: 0,
    needs_docs_migration: false,
    default_skill_ids: [],
    default_profile_id: null,
    briefing_notes: null,
    created_at: NOW,
    updated_at: NOW,
  },
];

const PROJECTS_PAYLOAD_AFTER_MIGRATION = PROJECTS_PAYLOAD_INITIAL.map(p =>
  p.id === 'p-legacy' ? { ...p, needs_docs_migration: false } : p
);

test.describe('Project — docs migration banner', () => {
  test('banner appears only on legacy projects, migration round-trip clears it', async ({ page }) => {
    let migrated = false;
    let migrateCalledWith: { id: string; create_symlink?: boolean } | null = null;

    await page.route('**/api/projects', route => {
      // List endpoint: switches to "all migrated" payload after a successful migrate.
      const payload = migrated ? PROJECTS_PAYLOAD_AFTER_MIGRATION : PROJECTS_PAYLOAD_INITIAL;
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: payload, error: null }),
      });
    });

    await page.route('**/api/projects/p-legacy/migrate-docs', async route => {
      migrated = true;
      const body = route.request().postData();
      migrateCalledWith = body ? { id: 'p-legacy', ...JSON.parse(body) } : { id: 'p-legacy' };
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: { status: 'Migrated', files_moved: 5, refs_rewritten: 2, symlink_created: false },
          error: null,
        }),
      });
    });

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickProjects();

    // ProjectList only allows one expanded card at a time (isOpen ===
    // expandedId), so we assert the modern-no-banner case FIRST while
    // it's the only one open, then switch to legacy.
    const modernHeader = page.locator('#project-p-modern .dash-card-header');
    await modernHeader.click();
    await expect(page.getByTestId('migration-banner-p-modern')).toHaveCount(0);

    // Now expand legacy — this auto-collapses modern. Banner appears.
    const legacyHeader = page.locator('#project-p-legacy .dash-card-header');
    await legacyHeader.click();
    const legacyBanner = page.getByTestId('migration-banner-p-legacy');
    await expect(legacyBanner).toBeVisible();

    // Operator opts out of the symlink, then clicks Migrate.
    await legacyBanner.locator('input[type="checkbox"]').uncheck();
    await page.getByTestId('migrate-docs-btn-p-legacy').click();

    // Refetch after migration removes the banner. The success row is
    // held briefly (~1.6 s); allow extra time before the disappearance.
    await expect(page.getByTestId('migration-banner-p-legacy')).toHaveCount(0, { timeout: 8_000 });

    expect(migrateCalledWith).toEqual({ id: 'p-legacy', create_symlink: false });
  });

  test('Failed outcome surfaces the inline reason without clearing the banner', async ({ page }) => {
    await page.route('**/api/projects', route =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: PROJECTS_PAYLOAD_INITIAL, error: null }),
      })
    );

    await page.route('**/api/projects/p-legacy/migrate-docs', route =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: { status: 'Failed', reason: 'docs/ exists with non-empty content — manual merge required' },
          error: null,
        }),
      })
    );

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.clickProjects();

    await page.locator('#project-p-legacy .dash-card-header').click();
    await page.getByTestId('migrate-docs-btn-p-legacy').click();

    const banner = page.getByTestId('migration-banner-p-legacy');
    await expect(banner).toBeVisible();
    await expect(banner.getByRole('alert')).toContainText(/manual merge required/);
  });
});
