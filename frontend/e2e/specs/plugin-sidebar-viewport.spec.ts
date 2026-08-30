import { test, expect } from '../fixtures/kronn-fixture';

test.describe('Plugins sidebar viewport containment', () => {
  for (const width of [767, 900]) {
    test(`keeps its header inside the page frame at ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 800 });
      await page.addInitScript(() => {
        localStorage.setItem('kronn:layoutDensity', 'medium');
      });

      await page.goto('/');
      const pluginsNav = page.locator('[data-tour-id="nav-mcps"]');
      await expect(pluginsNav).toHaveAttribute('data-mobile', 'true');
      await pluginsNav.click();

      const header = page.locator('.mcp-page .collection-shell-titlebar');
      await expect(header).toBeVisible();
      const geometry = await page.evaluate(() => {
        const nav = document.querySelector('.dash-nav')?.getBoundingClientRect();
        const titlebar = document.querySelector('.mcp-page .collection-shell-titlebar')?.getBoundingClientRect();
        return {
          viewportWidth: window.innerWidth,
          bodyWidth: document.body.scrollWidth,
          navBottom: nav?.bottom ?? -1,
          titlebarX: titlebar?.x ?? -1,
          titlebarY: titlebar?.y ?? -1,
          titlebarRight: titlebar?.right ?? Number.POSITIVE_INFINITY,
        };
      });

      expect(geometry.bodyWidth).toBe(geometry.viewportWidth);
      expect(geometry.titlebarX).toBeGreaterThanOrEqual(0);
      expect(geometry.titlebarRight).toBeLessThanOrEqual(geometry.viewportWidth);
      expect(geometry.titlebarY).toBeGreaterThanOrEqual(geometry.navBottom);
    });
  }
});
