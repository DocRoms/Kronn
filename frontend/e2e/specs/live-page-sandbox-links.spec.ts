import { expect, test } from '@playwright/test';
import { buildSandboxDocument } from '../../src/lib/live-page-sandbox';

test.describe('Live Page sandbox links', () => {
  test('opens explicit links through the parent without granting iframe popups', async ({ page, context }) => {
    await context.route('https://example.test/**', route => route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: '<title>Opened safely</title><h1>Opened safely</h1>',
    }));

    const channel = 'playwright-live-page-links';
    const sandboxDocument = buildSandboxDocument(`
      <script>
        parent.postMessage({
          type: 'kronn:page-open-link',
          version: 1,
          channel_id: ${JSON.stringify(channel)},
          url: 'https://example.test/forged-background-popup'
        }, '*');
      </script>
      <a id="report-link" href="https://example.test/report" target="_blank">Open report</a>
      <button id="script-link" onclick="window.open('https://example.test/scripted')">Open scripted report</button>
      <a id="active-link" href="javascript:alert('blocked')" target="_blank">Blocked active link</a>
    `, channel);

    await page.setContent(`
      <script>
        window.__openedLinks = [];
        window.__forgedMessages = [];
        addEventListener('message', event => {
          const message = event.data;
          if (message && message.type === 'kronn:page-open-link') {
            window.__forgedMessages.push(message.url);
          }
        });
        window.__connectLivePage = frame => {
          const linkChannel = new MessageChannel();
          linkChannel.port1.onmessage = event => {
            const message = event.data;
            if (
              !message
              || message.type !== 'kronn:page-open-link'
              || message.version !== 1
              || message.channel_id !== ${JSON.stringify(channel)}
            ) return;
            if (navigator.userActivation && !navigator.userActivation.isActive) return;
            let url;
            try { url = new URL(message.url); } catch (_error) { return; }
            if (
              (url.protocol !== 'http:' && url.protocol !== 'https:')
              || url.username
              || url.password
            ) return;
            window.__openedLinks.push(url.href);
            window.open(url.href, '_blank', 'noopener,noreferrer');
          };
          linkChannel.port1.start();
          frame.contentWindow.postMessage({
            type: 'kronn:page-link-port',
            version: 1,
            channel_id: ${JSON.stringify(channel)},
          }, '*', [linkChannel.port2]);
        };
      </script>
      <iframe title="Live Page" sandbox="allow-scripts"></iframe>
    `);
    await page.locator('iframe').evaluate((frame, srcdoc) => {
      const livePage = frame as HTMLIFrameElement;
      livePage.addEventListener('load', () => {
        (window as typeof window & {
          __connectLivePage: (target: HTMLIFrameElement) => void;
        }).__connectLivePage(livePage);
      }, { once: true });
      livePage.srcdoc = srcdoc;
    }, sandboxDocument);

    const iframe = page.locator('iframe');
    await expect(iframe).toHaveAttribute('sandbox', 'allow-scripts');
    const livePage = page.frameLocator('iframe');
    await expect.poll(() => page.evaluate(() => (
      window as typeof window & { __forgedMessages: string[] }
    ).__forgedMessages)).toEqual(['https://example.test/forged-background-popup']);
    expect(await page.evaluate(() => (
      window as typeof window & { __openedLinks: string[] }
    ).__openedLinks)).toEqual([]);

    const anchorPopup = page.waitForEvent('popup');
    await livePage.locator('#report-link').click();
    const openedAnchor = await anchorPopup;
    await expect(openedAnchor).toHaveURL('https://example.test/report');
    await expect(openedAnchor.locator('h1')).toHaveText('Opened safely');
    await openedAnchor.close();

    const scriptedPopup = page.waitForEvent('popup');
    await livePage.locator('#script-link').click();
    const openedScript = await scriptedPopup;
    await expect(openedScript).toHaveURL('https://example.test/scripted');
    await openedScript.close();

    await livePage.locator('#active-link').click();
    await livePage.locator('#active-link').evaluate(() => new Promise<void>(resolve => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    }));
    expect(await page.evaluate(() => (
      window as typeof window & { __openedLinks: string[] }
    ).__openedLinks)).toEqual([
      'https://example.test/report',
      'https://example.test/scripted',
    ]);
  });
});
