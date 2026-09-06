import { expect, type Page } from '@playwright/test';

/** Drive the real panel rail and select only this test's local provider. */
export async function openMediaLauncher(page: Page, connectionId: string) {
  const rail = page.getByTestId('panel-switcher');
  const toggle = rail.getByTestId('panel-open-toggle');
  if (await toggle.getAttribute('aria-expanded') !== 'true') await toggle.click();
  const assets = rail.locator('[data-panel="assets"]');
  await expect(assets).toBeVisible();
  // Opening the rail restores the last panel; clicking it again would close it.
  if (await assets.getAttribute('aria-expanded') !== 'true') await assets.click();
  await expect(assets).toHaveAttribute('aria-expanded', 'true');

  const panel = page.getByRole('complementary', { name: 'Assets' });
  await expect(panel).toBeVisible();
  await panel.getByTestId('assets-generate-toggle').click();
  const form = panel.getByTestId('media-generate-form');
  await expect(form).toBeVisible();
  const slot = form.getByTestId(`media-slot-${connectionId}:image`);
  await slot.click();
  await expect(slot).toHaveAttribute('aria-checked', 'true');
  return form;
}
