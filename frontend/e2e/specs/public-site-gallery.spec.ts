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

    const firstLink = page.locator('.gallery-card a').first();
    await firstLink.click();

    const dialog = page.getByRole('dialog', { name: locale.dialog });
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('.lb-count')).toHaveText('1 / 8');
    await expect(dialog.locator('.lb-img')).toHaveAttribute('alt', /\S/);

    await page.keyboard.press('ArrowRight');
    await expect(dialog.locator('.lb-count')).toHaveText('2 / 8');

    await page.keyboard.press('Shift+Tab');
    await expect(dialog.locator('.lb-next')).toBeFocused();

    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    await expect(firstLink).toBeFocused();
  });
}
