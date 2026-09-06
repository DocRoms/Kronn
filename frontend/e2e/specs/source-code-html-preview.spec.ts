import { expect, test } from '@playwright/test';
import { buildHtmlPreviewDocument } from '../../src/lib/html-preview';

test.describe('Source-code HTML preview isolation', () => {
  test('renders inline styles without executing scripts, navigating, submitting, or fetching', async ({ page }) => {
    const requests: string[] = [];
    await page.context().route('**/*', async route => {
      requests.push(route.request().url());
      await route.fulfill({ status: 204 });
    });
    const preview = buildHtmlPreviewDocument(`
      <!-- <head> --><html><head><style>#safe { color: rgb(255, 0, 0); }</style></head><body>
        <p id="safe">Safe preview</p>
        <script>parent.postMessage('script-ran', '*');</script>
        <img src="https://preview-probe.invalid/comment-head">
        <a id="go" href="https://preview-probe.invalid/self-navigation">Go</a>
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

    await rendered.locator('#go').click();
    await rendered.locator('#submit').click();
    await expect(rendered.locator('#safe')).toHaveText('Safe preview');
    expect(requests).toEqual([]);
  });
});
