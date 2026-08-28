/**
 * Settings — Portable agent library (KT-498/5).
 *
 * Locks the operator-facing UI/API contract for the `.agents/` portable
 * library section end-to-end: global vs project scope, sync/check/approve,
 * migrate, and import (both scopes — global import must NOT require a
 * carrier project, per the KT-503 review-round-1 fix).
 *
 * The spec stubs `/api/projects` and every `/api/portable-library*` endpoint
 * with an in-memory store (same pattern as `settings-user-context.spec.ts` /
 * `project-migration-banner.spec.ts`) so it stays hermetic: no real disk
 * writes, no dependency on a pre-existing project or `.agents` tree. The
 * actual filesystem contract (sync idempotence, frozen-hash check, TOFU
 * approval, lossless migration) is covered by the Rust integration tests in
 * `backend/src/api_tests.rs` and `backend/src/core/portable_library.rs`;
 * this spec only locks the UI wiring on top of it.
 *
 * NOTE: this sandbox worker cannot launch a browser (no network binding
 * available), so this spec is written and reviewed but not executed here.
 * It follows the exact fixture/page-object conventions of the specs above
 * and should run the same way against a real `pnpm test:e2e` invocation.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { SettingsPage } from '../pages/SettingsPage';
import { writeFileSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const NOW = '2026-01-01T00:00:00Z';

const PROJECT_PAYLOAD = [
  {
    id: 'proj-1',
    name: 'Demo project',
    path: '/tmp/demo-project',
    repo_url: null,
    token_override: null,
    ai_config: { detected: false, configs: [] },
    audit_status: 'NoTemplate',
    ai_todo_count: 0,
    needs_docs_migration: false,
    default_skill_ids: [],
    default_profile_id: null,
    briefing_notes: null,
    created_at: NOW,
    updated_at: NOW,
  },
];

function globalSkillItem(id: string) {
  return {
    kind: 'skill',
    id,
    scope: 'global',
    source: `skills/${id}/SKILL.md`,
    content_sha256: `sha-${id}`,
    content: `---\nname: ${id}\ndescription: demo\n---\n\nBody.\n`,
    data: null,
  };
}

/** In-memory stub for the whole `/api/portable-library*` surface. Mirrors the
 *  backend's scope/drift/approval state machine closely enough to exercise
 *  the UI contract without touching a real filesystem. */
function installPortableLibraryStubs(page: import('@playwright/test').Page) {
  const store: {
    global: { items: Array<Record<string, unknown>> };
    projects: Record<string, { items: Array<Record<string, unknown>>; drift: string; approved: boolean; syncedOnce: boolean }>;
  } = {
    global: { items: [globalSkillItem('demo-global-skill')] },
    projects: { 'proj-1': { items: [], drift: 'unsynced', approved: false, syncedOnce: false } },
  };
  let migratedOnce = false;

  page.route(
    url => url.pathname.includes('/api/portable-library'),
    async route => {
      const reqUrl = new URL(route.request().url());
      const method = route.request().method();
      const projectId = reqUrl.searchParams.get('project_id') || undefined;
      const path = reqUrl.pathname;
      const respond = (data: unknown, error: string | null = null) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ success: error === null, data, error }),
        });

      if (path.endsWith('/sync') && method === 'POST') {
        if (!projectId) return respond(null, 'project_id is required for sync');
        const p = store.projects[projectId];
        const first = !p.syncedOnce;
        p.syncedOnce = true;
        p.drift = 'clean';
        return respond({
          created: first ? ['kronn.lock'] : [],
          modified: [],
          deleted: [],
          unchanged: first ? [] : ['kronn.lock'],
        });
      }
      if (path.endsWith('/check') && method === 'POST') {
        if (!projectId) return respond(null, 'project_id is required');
        const p = store.projects[projectId];
        if (p.drift !== 'clean') return respond(null, 'frozen hash mismatch');
        return respond({ version: 1, resources: [], files: {} });
      }
      if (path.endsWith('/approve') && method === 'POST') {
        if (!projectId) return respond(null, 'project_id is required');
        const p = store.projects[projectId];
        if (p.drift !== 'clean') return respond(null, 'frozen hash mismatch');
        p.approved = true;
        return respond(true);
      }
      if (path.endsWith('/migrate') && method === 'POST') {
        const first = !migratedOnce;
        migratedOnce = true;
        return respond({
          created: first ? ['skills/legacy-thing/SKILL.md'] : [],
          unchanged: first ? [] : ['skills/legacy-thing/SKILL.md'],
        });
      }
      if (path.endsWith('/import') && method === 'POST') {
        const body = JSON.parse(route.request().postData() ?? '{}') as {
          project_id?: string;
          items: Array<Record<string, unknown>>;
        };
        for (const item of body.items) {
          if (item.scope === 'project') {
            if (!body.project_id) return respond(null, 'project_id is required to import project-scope items');
            store.projects[body.project_id].items.push(item);
          } else {
            store.global.items.push(item);
          }
        }
        return respond({ created: [], modified: [], deleted: [], unchanged: [] });
      }
      if (path.endsWith('/export') && method === 'GET') {
        const items = projectId
          ? [...store.global.items, ...store.projects[projectId].items]
          : store.global.items;
        return respond(items);
      }
      // GET /api/portable-library (state)
      const search = reqUrl.searchParams.get('search') ?? '';
      const items = (projectId ? [...store.global.items, ...store.projects[projectId].items] : store.global.items)
        .filter(item => !search || String(item.id).toLowerCase().includes(search.toLowerCase()));
      return respond({
        scope: projectId ? 'project' : 'global',
        project_id: projectId ?? null,
        items,
        drift: projectId ? store.projects[projectId].drift : 'not_applicable',
        approved: projectId ? store.projects[projectId].approved : false,
      });
    }
  );
}

test.describe('Settings — portable agent library', () => {
  test('global scope lists items and is not gated behind a carrier project', async ({ page }) => {
    installPortableLibraryStubs(page);
    await page.route('**/api/projects', route =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: PROJECT_PAYLOAD, error: null }),
      })
    );

    const dashboard = new DashboardPage(page);
    const settings = new SettingsPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();

    const section = settings.portableLibrarySection;
    await section.scrollIntoViewIfNeeded();
    await expect(section).toBeVisible();

    // Global scope (default) lists the global item.
    await expect(settings.portableItemCard('demo-global-skill')).toBeVisible();

    // Regression guard (KT-503 review round 1): import must work in global
    // scope without requiring a carrier project.
    await expect(settings.portableImportInput).toBeEnabled();

    // Sync/check/approve are project-scoped operations — disabled while no
    // project is selected.
    await expect(settings.portableActionButton('Sync')).toBeDisabled();
    await expect(settings.portableActionButton('Check frozen')).toBeDisabled();
    await expect(settings.portableActionButton('Approve')).toBeDisabled();
  });

  test('project scope runs sync -> check -> approve -> migrate end-to-end', async ({ page }) => {
    installPortableLibraryStubs(page);
    await page.route('**/api/projects', route =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: PROJECT_PAYLOAD, error: null }),
      })
    );

    const dashboard = new DashboardPage(page);
    const settings = new SettingsPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();

    const section = settings.portableLibrarySection;
    await section.scrollIntoViewIfNeeded();
    await settings.selectPortableScope('proj-1');

    await expect(settings.portableDriftChip).toContainText('unsynced');
    await expect(settings.portableTrustChip).toContainText('approval required');

    await settings.portableActionButton('Sync').click();
    await expect(settings.portableDriftChip).toContainText('clean');

    await settings.portableActionButton('Check frozen').click();
    await expect(section.locator('.set-warning-callout')).toHaveCount(0);

    await settings.portableActionButton('Approve').click();
    await expect(settings.portableTrustChip).toContainText('approved');

    await settings.portableActionButton('Migrate').click();
    await expect(section.locator('.set-warning-callout')).toHaveCount(0);
  });

  test('imports a project-scope item and a global-scope item from a file', async ({ page }) => {
    installPortableLibraryStubs(page);
    await page.route('**/api/projects', route =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ success: true, data: PROJECT_PAYLOAD, error: null }),
      })
    );

    const dashboard = new DashboardPage(page);
    const settings = new SettingsPage(page);
    await dashboard.goto();
    await dashboard.clickSettings();

    const section = settings.portableLibrarySection;
    await section.scrollIntoViewIfNeeded();

    // Global-scope import first — no project selected, no project_id sent.
    const globalFile = join(tmpdir(), `kronn-pw-portable-global-${Date.now()}.json`);
    writeFileSync(
      globalFile,
      JSON.stringify({
        version: 1,
        items: [globalSkillItem('imported-global-skill')],
      })
    );
    try {
      await settings.portableImportInput.setInputFiles(globalFile);
      await expect(settings.portableItemCard('imported-global-skill')).toBeVisible();
    } finally {
      unlinkSync(globalFile);
    }

    // Project-scope import — requires the project to be selected.
    await settings.selectPortableScope('proj-1');
    const projectFile = join(tmpdir(), `kronn-pw-portable-project-${Date.now()}.json`);
    writeFileSync(
      projectFile,
      JSON.stringify({
        version: 1,
        items: [{ ...globalSkillItem('imported-project-skill'), scope: 'project' }],
      })
    );
    try {
      await settings.portableImportInput.setInputFiles(projectFile);
      await expect(settings.portableItemCard('imported-project-skill')).toBeVisible();
    } finally {
      unlinkSync(projectFile);
    }
  });
});
