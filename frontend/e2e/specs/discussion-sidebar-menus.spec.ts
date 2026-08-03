import { expect, test } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

const title = `Sidebar menu PW ${Date.now()}`;
let discussionId: string | undefined;

test.afterAll(async ({ request }) => {
  if (discussionId) {
    await request.delete(`/api/discussions/${discussionId}`).catch(() => {});
  }
});

test('header and row overflow menus open on a direct click', async ({ page, request }) => {
  const created = await request.post('/api/discussions', {
    data: {
      title,
      agent: 'Codex',
      language: 'fr',
      initial_prompt: 'Discussion de validation des menus de sidebar.',
      no_agent: true,
    },
  });
  expect(created.ok()).toBe(true);
  discussionId = (await created.json())?.data?.id as string;

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.navDiscussions.click();

  const headerButton = page.getByRole('button', { name: /autres actions|more actions/i });
  await expect(headerButton).toBeVisible();
  await headerButton.click();
  const headerMenu = page.locator('.disc-sidebar-header-menu');
  await expect(headerMenu).toBeVisible();

  await page.keyboard.press('Escape');
  const row = page.locator(`[data-tour-disc-id="${discussionId}"]`).first();
  await expect(row).toBeVisible();
  const rowButton = row.locator('.disc-item-more-btn');
  await rowButton.click();
  const rowMenu = row.locator('.disc-item-action-menu');
  await expect(rowMenu).toBeVisible();
  const copyItem = rowMenu.getByRole('menuitem', { name: /copier l.id|copy id/i });
  await expect(copyItem).toBeVisible();
  // `toBeVisible()` does not detect another row painted over the popover.
  // A trial click performs the browser hit-test without triggering Clipboard.
  await copyItem.click({ trial: true });
});
