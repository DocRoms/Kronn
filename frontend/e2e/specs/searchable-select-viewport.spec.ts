import { test, expect, type Page } from '@playwright/test';

// KT-1032 — the real component in Chromium; no backend needed.
async function openMenu(page: Page, viewport: { width: number; height: number }, top: number, count: number) {
  await page.setViewportSize(viewport);
  await page.route('**/api/**', route => route.abort());
  await page.goto('/');
  await page.evaluate(async ([controlTop, options]) => {
    const harness = await import('/e2e/fixtures/searchable-select-harness.tsx');
    harness.mount(controlTop, options);
  }, [top, count] as const);
  await page.getByRole('combobox', { name: 'Model' }).focus();
  const menu = page.getByRole('listbox', { name: 'Model' });
  await menu.waitFor();
  return page.evaluate(() => {
    const root = document.querySelector<HTMLElement>('.searchable-select');
    const control = root?.querySelector('.searchable-select-control')?.getBoundingClientRect();
    const box = root?.querySelector('.searchable-select-menu')?.getBoundingClientRect();
    if (!root || !control || !box) throw new Error('SearchableSelect not mounted');
    return {
      placement: root.dataset.placement,
      control: { top: control.top, bottom: control.bottom },
      menu: { top: box.top, bottom: box.bottom, height: box.height },
      viewport: window.innerHeight,
    };
  });
}

test.describe('SearchableSelect menu stays inside the viewport', () => {
  test('a long catalogue with 320 px below the control (1000×900 repro)', async ({ page }) => {
    const { menu, viewport } = await openMenu(page, { width: 1000, height: 900 }, 550, 60);
    expect(menu.top).toBeGreaterThanOrEqual(0);
    expect(menu.bottom).toBeLessThanOrEqual(viewport);
    expect(menu.height).toBeGreaterThan(200);
  });

  test('plenty of room below keeps the full-height menu under the control', async ({ page }) => {
    const { placement, control, menu, viewport } = await openMenu(page, { width: 1000, height: 900 }, 100, 60);
    expect(placement).toBe('bottom');
    expect(menu.top).toBeGreaterThan(control.bottom);
    expect(menu.bottom).toBeLessThanOrEqual(viewport);
    expect(menu.height).toBeCloseTo(420, -1);
  });

  test('a short list that fits below opens below', async ({ page }) => {
    const { placement, menu, viewport } = await openMenu(page, { width: 1000, height: 900 }, 700, 2);
    expect(placement).toBe('bottom');
    expect(menu.bottom).toBeLessThanOrEqual(viewport);
  });

  test('a small window caps the menu to the room it has', async ({ page }) => {
    const { menu, viewport } = await openMenu(page, { width: 600, height: 360 }, 160, 60);
    expect(menu.top).toBeGreaterThanOrEqual(0);
    expect(menu.bottom).toBeLessThanOrEqual(viewport);
  });
});
