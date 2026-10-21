/**
 * Template v2 merge into a documented repo (KT-1181), in a real browser.
 *
 * "Lancer l'audit" on a documented, never-merged project opens the merge
 * first; a failed launch shows what to fix and "Reprendre" (KT-1169); the
 * reviewer accepts only the files they keep.
 *
 * Cost model: ZERO tokens and no write on the backend — every template-merge
 * route is mocked, and any other non-GET request is answered locally.
 */
import { test, expect } from '../fixtures/kronn-fixture';
import type { Page, Route } from '@playwright/test';
import { DashboardPage } from '../pages/DashboardPage';

const PROJECT_ID = 'pw-template-merge-fixture';
const SHOTS = process.env.KRONN_E2E_SHOTS_DIR;

const ok = (route: Route, data: unknown) => route.fulfill({
  status: 200,
  contentType: 'application/json',
  body: JSON.stringify({ success: true, data, error: null }),
});

const planView = {
  docs_dir: 'docs',
  base: 'local',
  base_ref: 'HEAD',
  worktree_path: null,
  branch: null,
  resumed: false,
  correspondence: [
    { source: 'docs/README.md', section: 'Running the tests', start_line: 4, end_line: 20, target: 'docs/testing-quality.md', action: 'extracted' },
    { source: 'docs/README.md', section: 'Release process', start_line: 21, end_line: 35, target: 'docs/workflow/ci-cd.md', action: 'extracted' },
    { source: 'docs/README.md', section: 'Team rituals', start_line: 36, end_line: 41, target: null, action: 'unmapped' },
    { source: 'docs/architecture.png', section: null, start_line: 0, end_line: 0, target: null, action: 'not_prose' },
    { source: 'CLAUDE.md', section: null, start_line: 1, end_line: 12, target: null, action: 'kronn_managed' },
    { source: 'docs/AGENTS.md', section: 'Rules', start_line: 1, end_line: 9, target: 'docs/AGENTS.md', action: 'target_exists' },
  ],
  files: [
    { path: 'docs/testing-quality.md', outcome: null, imported_sections: 1, diff: '' },
    { path: 'docs/workflow/ci-cd.md', outcome: null, imported_sections: 1, diff: '' },
  ],
  existing_targets: ['docs/AGENTS.md'],
  untracked_in_checkout: ['docs/draft-notes.md'],
  // The backend always sends `warnings` (KT-1310); a plan without it crashes the panel.
  warnings: [],
  table_markdown: '',
  // KT-1212: the plan always says where the merge stands on disk. Never
  // started here: no worktree, no branch, nothing to integrate.
  merge: {
    branch: 'kronn/template-v2-merge',
    worktree_exists: false,
    worktree_path: null,
    branch_exists: false,
    branch_ahead_commits: 0,
    pending_files: 0,
    checkout_branch: 'main',
  },
};

const resultView = {
  ...planView,
  worktree_path: '/repos/front_apollo/.kronn/worktrees/template-v2-merge',
  branch: 'kronn/template-v2-merge',
  // Started: the worktree holds both proposed files, none accepted yet.
  merge: {
    ...planView.merge,
    worktree_exists: true,
    worktree_path: '/repos/front_apollo/.kronn/worktrees/template-v2-merge',
    branch_exists: true,
    pending_files: 2,
  },
  files: [
    {
      path: 'docs/testing-quality.md', outcome: 'created', imported_sections: 1,
      diff: '--- /dev/null\n+++ b/docs/testing-quality.md\n@@ -0,0 +1,4 @@\n+# Testing & quality\n+\n+<!-- kronn:imported-start -->\n+Run `pnpm test` before every push.\n',
    },
    {
      path: 'docs/workflow/ci-cd.md', outcome: 'created', imported_sections: 1,
      diff: '--- /dev/null\n+++ b/docs/workflow/ci-cd.md\n@@ -0,0 +1,3 @@\n+# CI / CD\n+\n+Releases leave from main, never on Fridays.\n',
    },
  ],
};

async function mockBackend(page: Page) {
  const calls = { start: 0, fullAudit: 0, accepted: [] as string[][] };
  // Registered first, so every specific mock below takes precedence.
  await page.route('**/api/**', route => (
    route.request().method() === 'GET' ? route.fallback() : ok(route, null)
  ));
  await page.route('**/api/projects', route => (
    route.request().method() !== 'GET' ? route.fallback() : ok(route, [{
      id: PROJECT_ID,
      name: 'template-merge-fixture',
      path: '/repos/front_apollo',
      repo_url: null,
      token_override: null,
      ai_config: { detected: true, configs: [] },
      audit_status: 'TemplateInstalled',
      ai_todo_count: 0,
      tech_debt_count: 0,
      needs_docs_migration: false,
      default_skill_ids: [],
      // KT-1192: no audit starts without a saved briefing, so the launch the
      // merge hooks into stays disabled on a project that has none.
      briefing_notes: 'Documented front_apollo repo, merged from template v2.',
      linked_repos: [],
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    }])
  ));
  await page.route(`**/api/projects/${PROJECT_ID}/audit-status`, route => ok(route, null));
  await page.route('**/api/audit-status', route => ok(route, []));
  await page.route(`**/api/projects/${PROJECT_ID}/full-audit**`, route => {
    calls.fullAudit += 1;
    return ok(route, null);
  });
  await page.route(`**/api/projects/${PROJECT_ID}/template-merge/plan`, route => ok(route, planView));
  await page.route(`**/api/projects/${PROJECT_ID}/template-merge`, route => {
    if (route.request().method() === 'DELETE') return ok(route, null);
    calls.start += 1;
    if (calls.start === 1) {
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: false,
          data: null,
          error: 'git fetch --prune origin failed: could not read Username — check the remote and its credentials, then resume',
        }),
      });
    }
    return ok(route, resultView);
  });
  await page.route(`**/api/projects/${PROJECT_ID}/template-merge/accept`, route => {
    const paths = (route.request().postDataJSON() as { paths: string[] }).paths;
    calls.accepted.push(paths);
    return ok(route, { branch: 'kronn/template-v2-merge', commit: '9f3c2a17d0e4b5a6', committed: paths });
  });
  return calls;
}

async function shot(page: Page, name: string) {
  if (!SHOTS) return;
  await page.getByTestId('template-merge-panel').screenshot({ path: `${SHOTS}/${name}.png` });
}

for (const theme of ['light', 'dark'] as const) {
  test(`documented repo: the audit offers the merge first, resume after an error, accept per file (${theme})`, async ({ page }) => {
    await page.addInitScript(([t]) => {
      localStorage.setItem('kronn:theme', t);
      localStorage.setItem('kronn:projectDetailView', 'audit');
    }, [theme]);
    const calls = await mockBackend(page);
    const dashboard = new DashboardPage(page);
    await dashboard.goto();

    await expect(page.getByTestId('template-merge-offer')).toBeVisible({ timeout: 15_000 });
    const launchAudit = page.getByTestId('audit-timeline-launch');
    await expect(launchAudit).toBeEnabled({ timeout: 15_000 });
    await launchAudit.click();

    const panel = page.getByTestId('template-merge-panel');
    await expect(panel.getByTestId('template-merge-gate')).toBeVisible();
    expect(calls.fullAudit).toBe(0);
    await expect(panel.getByTestId('template-merge-untracked')).toContainText('docs/draft-notes.md');
    await expect(panel.locator('tr.tm-row-unmapped')).toHaveCount(1);
    await expect(panel.locator('tr.tm-row-unmapped')).toContainText('Laissé en place');
    await shot(page, `plan-${theme}`);

    await panel.getByRole('radio', { name: /Distante/ }).check();
    await panel.getByTestId('template-merge-launch').click();
    const error = panel.getByTestId('template-merge-error');
    await expect(error).toContainText('check the remote and its credentials');
    await shot(page, `error-${theme}`);
    await error.getByRole('button', { name: 'Reprendre' }).click();

    await expect(panel.getByTestId('template-merge-file')).toHaveCount(2);
    await expect(panel.getByText('Releases leave from main, never on Fridays.')).toBeVisible();
    await panel.getByRole('checkbox', { name: 'Accepter docs/workflow/ci-cd.md' }).uncheck();
    await shot(page, `result-${theme}`);
    await panel.getByTestId('template-merge-accept-selected').click();
    await expect(panel.getByTestId('template-merge-accepted')).toContainText('kronn/template-v2-merge (9f3c2a17)');
    expect(calls.accepted).toEqual([['docs/testing-quality.md']]);
    await expect(panel.getByTestId('template-merge-file')).toHaveCount(1);
    await shot(page, `accepted-${theme}`);
  });
}
