import { expect, test } from '@playwright/test';

test.describe('Source-code HTML preview isolation', () => {
  for (const mutation of ['<set attributeName="href" to="https://preview-probe.invalid/svg" begin="0s" />', '<animate attributeName="href" values="https://preview-probe.invalid/svg" dur="1s" repeatCount="indefinite" />']) {
    test(`prevents SVG link mutation: ${mutation.startsWith('<set') ? 'set' : 'animate'}`, async ({ page }) => {
      const requests: string[] = [];
      await page.context().route('https://preview-probe.invalid/**', async route => {
        requests.push(route.request().url());
        await route.fulfill({ status: 200, contentType: 'text/html', body: 'Intercepted locally' });
      });
      await page.goto('/');
      const preview = await page.evaluate(async source => {
        const { buildHtmlPreviewDocument } = await import('/src/lib/html-preview.ts');
        return buildHtmlPreviewDocument(source);
      }, `<p id="safe">Static preview</p><svg xmlns="http://www.w3.org/2000/svg"><a id="animated"><text x="0" y="30">Open</text>${mutation}</a></svg>`);
      await page.setContent('<iframe title="HTML preview" sandbox=""></iframe>');
      await page.locator('iframe').evaluate((frame, srcdoc) => {
        (frame as HTMLIFrameElement).srcdoc = srcdoc;
      }, preview);
      const rendered = page.frameLocator('iframe');
      await expect(rendered.locator('#safe')).toHaveText('Static preview');
      // SVG SMIL href can be navigable without exposing an ARIA link role.
      // Click the actual anchor, and retain requests from parsing onward.
      await expect(rendered.locator('svg set, svg animate')).toHaveCount(0);
      await rendered.locator('svg a').click();
      await expect(rendered.locator('#safe')).toHaveText('Static preview');
      expect(requests).toEqual([]);
    });
  }

  test('renders inline styles without executing scripts, navigating, submitting, or fetching', async ({ page }) => {
    const requests: string[] = [];
    await page.context().route('https://preview-probe.invalid/**', async route => {
      requests.push(route.request().url());
      await route.fulfill({ status: 204 });
    });
    await page.goto('/');
    const preview = await page.evaluate(async source => {
      const { buildHtmlPreviewDocument } = await import('/src/lib/html-preview.ts');
      return buildHtmlPreviewDocument(source);
    }, `
      <!-- <head> --><html><head><style>#safe { color: rgb(255, 0, 0); }</style></head><body>
        <p id="safe">Safe preview</p>
        <script>parent.postMessage('script-ran', '*');</script>
        <img src="https://preview-probe.invalid/comment-head">
        <a id="go" href="https://preview-probe.invalid/self-navigation">Go</a>
        <a/href="https://preview-probe.invalid/slash-navigation">Malformed link</a>
        <div><template shadowrootmode="open"><a href="https://preview-probe.invalid/template">Template link</a></template></div>
        <img id="data-image" src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9JQX8AAAAASUVORK5CYII=">
        <meta http-equiv="refresh" content="0; url=https://preview-probe.invalid/refresh"><base href="https://preview-probe.invalid/">
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
    await expect(rendered.locator('#data-image')).toBeVisible();
    expect(await page.evaluate(() => (window as typeof window & { __scriptRan: boolean }).__scriptRan)).toBe(false);

    expect(preview).not.toContain('https://preview-probe.invalid');
    await expect(rendered.locator('#go')).not.toHaveAttribute('href');
    await expect(rendered.locator('#submit')).toHaveCount(0);
    await expect(rendered.locator('template')).toHaveCount(0);
    await expect(rendered.getByText('Template link')).toHaveCount(0);
    await rendered.locator('#go').click();
    await expect(rendered.locator('#safe')).toHaveText('Safe preview');
    expect(requests).toEqual([]);
  });
});
