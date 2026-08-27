import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

let workflowId: string | null = null;

test.describe('Workflow detail scrolling', () => {
  test.afterEach(async ({ request }) => {
    if (workflowId) {
      await request.delete(`/api/workflows/${workflowId}`).catch(() => undefined);
      workflowId = null;
    }
  });

  test('the wheel drives one viewer scrollbar, without a nested pane scrollbar', async ({ page, request }) => {
    const name = `PW workflow scroll ${Date.now()}`;
    const create = await request.post('/api/workflows', {
      data: {
        name,
        project_id: null,
        trigger: { type: 'Manual' },
        steps: Array.from({ length: 20 }, (_, index) => ({
          name: `long_step_${String(index + 1).padStart(2, '0')}`,
          step_type: { type: 'JsonData' },
          agent: 'ClaudeCode',
          prompt_template: '',
          mode: { type: 'Normal' },
          json_data_payload: { index, purpose: 'force deterministic vertical overflow' },
        })),
      },
    });
    expect(create.ok(), `create returned ${create.status()}`).toBe(true);
    const payload = await create.json();
    workflowId = payload?.data?.id ?? null;
    expect(workflowId, `create body: ${JSON.stringify(payload)}`).toBeTruthy();

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.navWorkflows.click();
    const automationKinds = page.locator('[data-tour-id="automation-kinds"]');
    const activeRunsFooter = page.locator('.wf-active-runs-footer');
    await Promise.race([
      automationKinds.waitFor({ state: 'visible', timeout: 10_000 }),
      activeRunsFooter.waitFor({ state: 'visible', timeout: 10_000 }),
    ]);
    if (await activeRunsFooter.isVisible()) {
      await activeRunsFooter.click();
    }
    await expect(automationKinds).toBeVisible({ timeout: 10_000 });
    await page.locator('.wf-card').filter({ hasText: name }).click();

    const detail = page.locator('.wf-workflow-detail-pane');
    const viewer = page.locator('.automation-viewer');
    await expect(detail).toBeVisible();
    await expect
      .poll(() => viewer.evaluate(node => node.scrollHeight - node.clientHeight))
      .toBeGreaterThan(100);

    const scrollingAncestors = await detail.evaluate(node => {
      const result: string[] = [];
      for (let current = node.parentElement; current; current = current.parentElement) {
        const overflowY = getComputedStyle(current).overflowY;
        if (
          current.scrollHeight > current.clientHeight + 1
          && (overflowY === 'auto' || overflowY === 'scroll')
        ) {
          result.push(current.className);
        }
      }
      return result;
    });
    expect(scrollingAncestors).toEqual(['automation-viewer']);

    await detail.hover();
    await page.mouse.wheel(0, 700);
    await expect.poll(() => viewer.evaluate(node => node.scrollTop)).toBeGreaterThan(0);
  });
});
