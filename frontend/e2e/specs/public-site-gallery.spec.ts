import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
// Vanilla fixture is intentional: this exercises the standalone static site,
// not the Kronn SPA boot flow served by the shared E2E fixture.
import { expect, test } from '@playwright/test';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');

const locales = [
  { file: 'index.html', dialog: 'Galerie de captures Kronn' },
  { file: 'en.html', dialog: 'Kronn screenshot gallery' },
  { file: 'es.html', dialog: 'Galería de capturas de Kronn' },
] as const;

for (const locale of locales) {
  test(`public screenshot gallery is accessible in ${locale.file}`, async ({ page }) => {
    const html = await readFile(path.join(repoRoot, 'site', locale.file), 'utf8');
    await page.setContent(html, { waitUntil: 'domcontentloaded' });

    // The count was pinned at eight and broke the day the gallery grew. What
    // the carousel owes the reader is that it opens on the card clicked and
    // moves one slide at a time, whatever the total happens to be.
    const total = await page.locator('.gallery-card a').count();
    expect(total).toBeGreaterThanOrEqual(8);

    const firstLink = page.locator('.gallery-card a').first();
    await firstLink.click();

    const dialog = page.getByRole('dialog', { name: locale.dialog });
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('.lb-count')).toHaveText(`1 / ${total}`);
    await expect(dialog.locator('.lb-img')).toHaveAttribute('alt', /\S/);

    await page.keyboard.press('ArrowRight');
    await expect(dialog.locator('.lb-count')).toHaveText(`2 / ${total}`);

    // The trap wraps from the first control to the last one. Naming that
    // control pins the toolbar's order instead of the behaviour, and broke the
    // day a button was added; what matters is that focus stays inside.
    const controls = dialog.locator('button');
    await page.keyboard.press('Shift+Tab');
    await expect(controls.last()).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(dialog.locator('.lb-close')).toBeFocused();

    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    await expect(firstLink).toBeFocused();
  });
}
