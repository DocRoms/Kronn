import { expect, type Page } from '@playwright/test';
import { openDiscussionPanel } from './DiscussionPanels';

/** Open the discussion's Assets panel: the header's « Détails » button, then the rail. */
export async function openAssetsPanel(page: Page) {
  await openDiscussionPanel(page, 'assets');

  const panel = page.getByRole('complementary', { name: 'Assets' });
  await expect(panel).toBeVisible();
  return panel;
}

/** Drive the real panel rail and select only this test's local provider. */
export async function openMediaLauncher(page: Page, connectionId: string) {
  const panel = await openAssetsPanel(page);
  await panel.getByTestId('assets-tab-creator').click();
  const form = panel.getByTestId('media-generate-form');
  await expect(form).toBeVisible();
  const slot = form.getByTestId(`media-slot-${connectionId}:image`);
  await slot.click();
  await expect(slot).toHaveAttribute('aria-checked', 'true');
  return form;
}
