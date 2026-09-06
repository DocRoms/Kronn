import { test, expect } from '@playwright/test';
import { BootPage } from '../pages/BootPage';

// Hold setup pending to inspect the real splash CSS, without exercising the
// backend or changing its state. App.test.tsx covers timeout and retry paths.
for (const colorScheme of ['dark', 'light'] as const) {
  test(`startup mark rotates slowly and respects reduced motion (${colorScheme})`, async ({ page }) => {
    const pageErrors: string[] = [];
    page.on('pageerror', error => pageErrors.push(error.message));
    await page.emulateMedia({ colorScheme, reducedMotion: 'no-preference' });
    await page.route('**/api/**', route => {
      const path = new URL(route.request().url()).pathname;
      if (path === '/api/setup/status') return;
      if (path === '/api/config/ui-language') {
        return route.fulfill({ json: { success: true, data: 'en', error: null } });
      }
      return route.fulfill({ status: 503, json: { success: false, data: null, error: 'Unavailable during boot fixture' } });
    });
    const boot = new BootPage(page);
    await boot.goto();
    expect(await page.evaluate(() => document.documentElement.dataset.theme)).toBe(colorScheme);

    await expect(boot.mark).toHaveAttribute('width', '100');
    await expect(boot.mark).toHaveAttribute('height', '100');
    await expect(boot.mark).toHaveAttribute('aria-hidden', 'true');
    await expect(boot.mark).toHaveCSS('animation-name', 'kronn-startup-rotate');
    await expect(boot.mark).toHaveCSS('animation-duration', '6s');
    await expect(boot.mark).toHaveCSS('animation-timing-function', 'linear');
    await expect(boot.mark).toHaveCSS('animation-iteration-count', 'infinite');
    await expect(boot.status).toContainText(/Entering the grid|Loading config|Detecting agents|Almost ready/);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await expect(boot.mark).toHaveCSS('animation-name', 'none');
    await expect(boot.mark).toHaveCSS('transform', 'none');
    await page.setViewportSize({ width: 360, height: 640 });
    await expect(boot.mark).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    const screenshotPath = test.info().outputPath(`startup-${colorScheme}.png`);
    await page.screenshot({ path: screenshotPath });
    await test.info().attach(`startup-${colorScheme}`, { path: screenshotPath, contentType: 'image/png' });
    expect(pageErrors).toEqual([]);
  });
}
