/**
 * Routing — every view has an address, and the address is the truth.
 *
 * Real browser, real backend: the pages, the resources open in them, Back and
 * Forward, a reload, and the links written before pages had addresses. The
 * fixtures are created through the API so the spec owns what it opens.
 */
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test, expect } from '../fixtures/kronn-fixture';
import type { APIRequestContext, Page } from '@playwright/test';
import { DashboardPage } from '../pages/DashboardPage';
import { WorkflowsPage } from '../pages/WorkflowsPage';

const stamp = Date.now();
const discA = { id: '', title: `Routing A ${stamp}` };
const discB = { id: '', title: `Routing B ${stamp}` };
let taskId = '';
const taskTitle = `Routing task ${stamp}`;
let qpId = '';
const qpName = `Routing prompt ${stamp}`;
let projectId = '';
const projectName = `Routing project ${stamp}`;
let artifactId = '';
const artifactTitle = `Routing artifact ${stamp}`;

async function created(request: APIRequestContext, path: string, data: unknown): Promise<{ id: string }> {
  const response = await request.post(path, { data });
  expect(response.ok(), `${path} answered ${response.status()}`).toBe(true);
  const body = await response.json();
  expect(body?.success, `${path}: ${JSON.stringify(body?.error)}`).toBe(true);
  return body.data;
}

test.beforeAll(async ({ request }) => {
  const discussion = (title: string) => created(request, '/api/discussions', {
    title, agent: 'Codex', language: 'fr', initial_prompt: 'Routing fixture, no agent.', no_agent: true,
  });
  discA.id = (await discussion(discA.title)).id;
  discB.id = (await discussion(discB.title)).id;
  taskId = (await created(request, '/api/planning/tasks', { title: taskTitle })).id;
  qpId = (await created(request, '/api/quick-prompts', {
    name: qpName, prompt_template: 'Say hello to {{name}}', agent: 'Codex',
  })).id;
  // An Artifact with a related discussion: its links between resources.
  artifactId = (await created(request, '/api/pages', { title: artifactTitle, html: '<h1>Routing</h1>', datasets: [] })).id;
  await created(request, `/api/pages/${artifactId}/discussions`, { discussion_id: discB.id, relation: 'attached' });
  // A project needs a folder the backend may scan: only inside an owned
  // repositories directory (the sandbox launcher sets one).
  const reposBase = process.env.KRONN_REPOS_DIR ?? '';
  if (reposBase && existsSync(reposBase)) {
    const folder = mkdtempSync(join(reposBase, 'routing-'));
    mkdirSync(join(folder, '.git'));
    writeFileSync(join(folder, 'README.md'), '# routing fixture\n');
    projectId = (await created(request, '/api/projects/add-folder', { path: folder, name: projectName })).id;
  }
});

const current = (page: Page, tourId: string) => page.locator(`[data-tour-id="${tourId}"]`);

test.describe('Routing — addresses', () => {
  test('every page opens at its own address, and the nav says so', async ({ page }) => {
    for (const [path, nav] of [
      ['/projects', 'nav-projects'], ['/discussions', 'nav-discussions'], ['/planning', 'nav-planning'],
      ['/workflows', 'nav-workflows'], ['/plugins', 'nav-mcps'], ['/config', 'nav-settings'],
    ] as const) {
      await page.goto(path);
      await expect(current(page, nav)).toHaveAttribute('aria-current', 'page');
      expect(new URL(page.url()).pathname).toBe(path);
    }
  });

  test('the bare and unknown addresses land on Projects', async ({ page }) => {
    // With projects, the page opens the first one on its own and that one
    // replaces the bare address; without, the bare address is the final state.
    const settled = projectId ? /\/projects\/[^/]+$/ : /\/projects$/;
    for (const start of ['/', '/nowhere/at/all']) {
      await page.goto(start);
      await expect(current(page, 'nav-projects')).toHaveAttribute('aria-current', 'page');
      await page.waitForURL(settled);
      if (projectId) await expect(page.locator('.project-detail-header')).toBeVisible();
      else await expect(page.locator('.project-detail-empty')).toBeVisible();
    }
  });

  test('the nav writes the address; Back and Forward follow it', async ({ page }) => {
    const dashboard = new DashboardPage(page);
    await page.goto('/projects');
    await current(page, 'nav-planning').click();
    await page.waitForURL(/\/planning$/);
    await dashboard.openWorkflows();
    await page.waitForURL(/\/workflows/);

    await page.goBack();
    await page.waitForURL(/\/planning$/);
    await expect(current(page, 'nav-planning')).toHaveAttribute('aria-current', 'page');

    await page.goForward();
    await page.waitForURL(/\/workflows/);
    await expect(current(page, 'nav-workflows')).toHaveAttribute('aria-current', 'page');
  });

  test('a discussion address opens the discussion, and a reload keeps it', async ({ page }) => {
    await page.goto(`/discussions/${discA.id}`);
    await expect(page.locator('.disc-chat-header-title')).toContainText(discA.title);

    await page.reload();
    await expect(page.locator('.disc-chat-header-title')).toContainText(discA.title);
    expect(new URL(page.url()).pathname).toBe(`/discussions/${discA.id}`);
  });

  test('picking a discussion writes its address, a step Back undoes', async ({ page }) => {
    await page.goto(`/discussions/${discA.id}`);
    await expect(page.locator('.disc-chat-header-title')).toContainText(discA.title);

    await page.locator(`[data-tour-disc-id="${discB.id}"] .disc-item-open`).first().click();
    await page.waitForURL(new RegExp(`/discussions/${discB.id}$`));
    await expect(page.locator('.disc-chat-header-title')).toContainText(discB.title);

    await page.goBack();
    await page.waitForURL(new RegExp(`/discussions/${discA.id}$`));
    await expect(page.locator('.disc-chat-header-title')).toContainText(discA.title);
  });

  test('a modified or middle click opens a page or a row in a new tab, and leaves this one where it is', async ({ page, context }) => {
    await page.goto(`/discussions/${discA.id}`);
    await expect(page.locator('.disc-chat-header-title')).toContainText(discA.title);

    // A nav tab is a link: Ctrl/Cmd-click is the browser's own new tab.
    const [planningTab] = await Promise.all([
      context.waitForEvent('page'),
      current(page, 'nav-planning').click({ modifiers: ['ControlOrMeta'] }),
    ]);
    await planningTab.waitForLoadState();
    expect(new URL(planningTab.url()).pathname).toBe('/planning');
    await planningTab.close();

    // A sidebar row keeps its element; the app opens the new tab itself.
    const row = page.locator(`[data-tour-disc-id="${discB.id}"] .disc-item-open`).first();
    const [discussionTab] = await Promise.all([
      context.waitForEvent('page'),
      row.click({ modifiers: ['ControlOrMeta'] }),
    ]);
    await discussionTab.waitForLoadState();
    expect(new URL(discussionTab.url()).pathname).toBe(`/discussions/${discB.id}`);
    await expect(discussionTab.locator('.disc-chat-header-title')).toContainText(discB.title);
    await discussionTab.close();

    const [middleTab] = await Promise.all([
      context.waitForEvent('page'),
      row.click({ button: 'middle' }),
    ]);
    await middleTab.waitForLoadState();
    expect(new URL(middleTab.url()).pathname).toBe(`/discussions/${discB.id}`);
    await middleTab.close();

    // This tab never moved.
    expect(new URL(page.url()).pathname).toBe(`/discussions/${discA.id}`);
    await expect(page.locator('.disc-chat-header-title')).toContainText(discA.title);
  });

  test('an Artifact\'s related discussion is a link: a plain click follows it here, a modified or middle click opens it beside', async ({ page, context }) => {
    await page.goto(`/pages/${artifactId}`);
    const link = page.locator(`.live-pages-workflows a[href="/discussions/${discB.id}"]`);
    await expect(link).toContainText(discB.title);

    for (const options of [{ modifiers: ['ControlOrMeta'] as const }, { button: 'middle' as const }]) {
      const [tab] = await Promise.all([context.waitForEvent('page'), link.click(options)]);
      await tab.waitForLoadState();
      expect(new URL(tab.url()).pathname).toBe(`/discussions/${discB.id}`);
      await expect(tab.locator('.disc-chat-header-title')).toContainText(discB.title);
      await tab.close();
      // This tab never moved.
      expect(new URL(page.url()).pathname).toBe(`/pages/${artifactId}`);
    }

    await link.click();
    await page.waitForURL(new RegExp(`/discussions/${discB.id}$`));
    await expect(page.locator('.disc-chat-header-title')).toContainText(discB.title);
    await page.goBack();
    await page.waitForURL(new RegExp(`/pages/${artifactId}$`));
    await expect(link).toContainText(discB.title);
  });

  test('a planning task address opens its detail, and a reload keeps it', async ({ page }) => {
    await page.goto(`/planning/${taskId}`);
    await expect(page.locator('.planning-detail-title')).toHaveValue(taskTitle);

    await page.reload();
    await expect(page.locator('.planning-detail-title')).toHaveValue(taskTitle);
    expect(new URL(page.url()).pathname).toBe(`/planning/${taskId}`);
  });

  test('the Automation tab and resource are in the address', async ({ page }) => {
    const workflows = new WorkflowsPage(page);
    await page.goto(`/workflows/qp/${qpId}`);
    await expect(page.locator('.automation-viewer .qp-card[data-detail="true"]')).toContainText(qpName);

    await page.reload();
    await expect(page.locator('.automation-viewer .qp-card[data-detail="true"]')).toContainText(qpName);

    // The reader's choice of a tab moves the address; Back brings the prompt back.
    await workflows.selectKind('quickApis');
    await page.waitForURL(/\/workflows\/qa$/);
    await page.goBack();
    await page.waitForURL(new RegExp(`/workflows/qp/${qpId}$`));
    await expect(page.locator('.automation-viewer .qp-card[data-detail="true"]')).toContainText(qpName);
  });

  test('a project address opens the project, and the legacy #project- link redirects there', async ({ page }) => {
    test.skip(!projectId, 'needs an owned repositories directory (KRONN_REPOS_DIR)');
    await page.goto(`/projects/${projectId}`);
    await expect(page.locator('.project-detail-header')).toContainText(projectName);

    await page.goto(`/#project-${projectId}`);
    await page.waitForURL(new RegExp(`/projects/${projectId}$`));
    expect(new URL(page.url()).hash).toBe('');
    await expect(page.locator('.project-detail-header')).toContainText(projectName);
  });

  test('the links written before pages had addresses still land right', async ({ page }) => {
    await page.goto(`/#discussion-${discA.id}`);
    await page.waitForURL(new RegExp(`/discussions/${discA.id}$`));
    expect(new URL(page.url()).hash).toBe('');
    await expect(page.locator('.disc-chat-header-title')).toContainText(discA.title);

    await page.goto('/planning#config');
    await page.waitForURL(/\/config$/);
    await expect(current(page, 'nav-settings')).toHaveAttribute('aria-current', 'page');

    await page.goto(`/#discussions/mosaic?discussion=${discA.id}&discussion=${discB.id}&layout=two-columns`);
    await page.waitForURL(/\/standalone\/discussions\/mosaic\?/);
    const url = new URL(page.url());
    expect(url.searchParams.getAll('discussion')).toEqual([discA.id, discB.id]);
    expect(url.searchParams.get('layout')).toBe('two-columns');
    await expect(page.locator('main.discussion-mosaic')).toBeVisible();
    await expect(current(page, 'nav-projects')).toHaveCount(0);
  });
});
