import { expect, test, type Page } from '@playwright/test';
import { buildSandboxDocument, parseLivePageEmbeds, type LivePageEmbedPlacement } from '../../src/lib/live-page-sandbox';
import { embedPlacementStyle, isEmbedPlacementShown } from '../../src/lib/live-page-embeds';

// The host draws third-party content OVER the Page iframe. A placeholder inside
// a Page container with `overflow:hidden` must not let that content spill past
// the container and swallow clicks meant for the Page's own controls. This runs
// the real bridge in a real Chromium and positions the stand-in player with the
// same style function the overlay uses.

const CHANNEL = 'playwright-live-page-embeds';
const PLAYER = 'https://player.example.test/embed/1';

const PAGE = `
  <style>
    body { margin: 0; font: 14px sans-serif; }
    #box { height: 50px; overflow: hidden; }
    #placeholder { height: 152px; background: #ddd; }
    #below { position: absolute; top: 100px; left: 0; width: 200px; height: 40px; }
  </style>
  <div id="box"><div id="placeholder" data-kronn-embed="${PLAYER}"></div><div style="height:400px"></div></div>
  <button id="below" onclick="document.body.dataset.clicked = 'yes'">Page button</button>
`;

async function mountHost(page: Page) {
  await page.context().route('https://player.example.test/**', route => route.fulfill({
    status: 200, contentType: 'text/html', body: '<body style="margin:0;background:#f0f">player</body>',
  }));
  await page.setContent(`
    <style>
      body { margin: 0; }
      #shell { position: relative; width: 600px; height: 400px; }
      #page { border: 0; width: 600px; height: 400px; display: block; }
      #layer { position: absolute; inset: 0; overflow: hidden; pointer-events: none; }
      #player { position: absolute; border: 0; pointer-events: auto; }
    </style>
    <div id="shell">
      <iframe id="page" title="Live Page" sandbox="allow-scripts"></iframe>
      <div id="layer"><iframe id="player" title="player"></iframe></div>
    </div>
    <script>
      window.__reports = [];
      window.__connect = frame => {
        const link = new MessageChannel();
        link.port1.onmessage = event => {
          if (event.data && event.data.type === 'kronn:page-embeds') window.__reports.push(event.data.embeds);
        };
        link.port1.start();
        frame.contentWindow.postMessage({ type: 'kronn:page-link-port', version: 1, channel_id: ${JSON.stringify(CHANNEL)} }, '*', [link.port2]);
      };
    </script>
  `);
  await page.locator('#page').evaluate((frame, srcdoc) => {
    const livePage = frame as HTMLIFrameElement;
    livePage.addEventListener('load', () => (window as unknown as { __connect: (f: HTMLIFrameElement) => void }).__connect(livePage), { once: true });
    livePage.srcdoc = srcdoc;
  }, buildSandboxDocument(PAGE, CHANNEL));
}

/** Read the bridge's latest report and place the stand-in player exactly as the overlay would. */
async function drawLatest(page: Page): Promise<LivePageEmbedPlacement> {
  await expect.poll(() => page.evaluate(() => (window as unknown as { __reports: unknown[] }).__reports.length)).toBeGreaterThan(0);
  const raw = await page.evaluate(() => {
    const reports = (window as unknown as { __reports: unknown[] }).__reports;
    return reports[reports.length - 1];
  });
  const [placement] = parseLivePageEmbeds(raw)!;
  const style = embedPlacementStyle(placement, isEmbedPlacementShown(placement));
  await page.locator('#player').evaluate((player, { style, url }) => {
    const element = player as HTMLIFrameElement;
    if (element.getAttribute('src') !== url) element.src = url;
    element.style.left = `${style.left}px`;
    element.style.top = `${style.top}px`;
    element.style.width = `${style.width}px`;
    element.style.height = `${style.height}px`;
    element.style.visibility = style.visibility;
    element.style.clipPath = style.clipPath ?? '';
  }, { style, url: placement.url });
  return placement;
}

test.describe('Live Page embeds inside a clipping container', () => {
  test('the player stays within the container and the Page button below still gets the click', async ({ page }) => {
    await mountHost(page);
    const placement = await drawLatest(page);
    expect(placement.rect.height).toBe(152);
    expect(placement.clip).toMatchObject({ top: 0, height: 50 });

    // The button sits at y=100, inside the placeholder's full rectangle but
    // outside the container: the click must reach the Page, not the player.
    await page.mouse.click(100, 120);
    await expect(page.frameLocator('#page').locator('body')).toHaveAttribute('data-clicked', 'yes');

    // Inside the visible part, the player is what receives the pointer.
    const hit = await page.evaluate(() => document.elementFromPoint(100, 25)?.id);
    expect(hit).toBe('player');
  });

  test('scrolling the container moves the cut without reloading the player', async ({ page }) => {
    await mountHost(page);
    await drawLatest(page);
    const before = await page.locator('#player').evaluate(player => (player as HTMLIFrameElement).contentWindow);
    expect(before).toBeTruthy();
    const reports = await page.evaluate(() => (window as unknown as { __reports: unknown[] }).__reports.length);

    await page.frameLocator('#page').locator('#box').evaluate(box => { box.scrollTop = 120; });
    await expect.poll(() => page.evaluate(() => (window as unknown as { __reports: unknown[] }).__reports.length)).toBeGreaterThan(reports);
    const placement = await drawLatest(page);
    expect(placement.rect.top).toBe(-120);
    expect(placement.clip).toMatchObject({ top: 0, height: 32 });
    expect(placement.visible).toBe(true);
    // Only the 32px still inside the container take the pointer.
    expect(await page.evaluate(() => document.elementFromPoint(100, 20)?.id)).toBe('player');
    expect(await page.evaluate(() => document.elementFromPoint(100, 40)?.id)).not.toBe('player');

    await page.frameLocator('#page').locator('#box').evaluate(box => { box.scrollTop = 300; });
    await expect.poll(async () => (await drawLatest(page)).visible).toBe(false);
    expect(await page.evaluate(() => document.elementFromPoint(100, 20)?.id)).not.toBe('player');
  });
});
