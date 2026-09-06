import { expect, test } from '@playwright/test';
import { buildHtmlPreviewDocument } from '../../src/components/SourceCodeViewer';

test.describe('Source-code HTML preview isolation', () => {
  test('renders inline styles while the sandbox blocks scripts and form submission', async ({ page }) => {
    const preview = buildHtmlPreviewDocument(`
      <html><head><style>#safe { color: rgb(255, 0, 0); }</style></head><body>
        <p id="safe">Safe preview</p>
        <script>parent.postMessage('script-ran', '*');</script>
        <form action="https://example.test/submit"><button id="submit">Submit</button></form>
      </body></html>
    `);
    await page.setContent(`
      <script>window.__scriptRan = false; addEventListener('message', () => { window.__scriptRan = true; });</script>
      <iframe title="HTML preview" sandbox=""></iframe>
    `);
    await page.locator('iframe').evaluate((frame, srcdoc) => {
      (frame as HTMLIFrameElement).srcdoc = srcdoc;
    }, preview);

    const iframe = page.locator('iframe');
    await expect(iframe).toHaveAttribute('sandbox', '');
    const rendered = page.frameLocator('iframe');
    await expect(rendered.locator('#safe')).toHaveText('Safe preview');
    await expect(rendered.locator('#safe')).toHaveCSS('color', 'rgb(255, 0, 0)');
    expect(await page.evaluate(() => (window as typeof window & { __scriptRan: boolean }).__scriptRan)).toBe(false);

    let submitted = false;
    await page.context().route('https://example.test/submit', route => {
      submitted = true;
      return route.fulfill({ status: 204 });
    });
    await rendered.locator('#submit').click();
    await expect(rendered.locator('#safe')).toHaveText('Safe preview');
    expect(submitted).toBe(false);
  });
});
