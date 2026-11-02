import { expect, type Locator, type Page } from '@playwright/test';

export type DiscussionPanelId = 'plan' | 'assets' | 'notes' | 'git' | 'terminal' | 'settings';

/** The vertical rail of the open side panel. It only exists while a panel is open. */
export function discussionPanelRail(page: Page): Locator {
  return page.getByRole('tablist', { name: 'Panneaux de la discussion' });
}

/**
 * Open a discussion side panel through the real controls: the header's single
 * « Détails » button (lands on « Paramètres »), then the rail to switch.
 * Idempotent: a panel already showing is left alone (« Détails » again would
 * close it).
 */
export async function openDiscussionPanel(page: Page, panel: DiscussionPanelId = 'settings') {
  const rail = discussionPanelRail(page);
  if (!(await rail.isVisible())) {
    await page.getByTestId('disc-header-details-btn').click();
    await expect(rail).toBeVisible();
  }
  const item = rail.locator(`[data-panel="${panel}"]`);
  await expect(item).toBeVisible();
  if (await item.getAttribute('aria-selected') !== 'true') await item.click();
  await expect(item).toHaveAttribute('aria-selected', 'true');
  return rail;
}
