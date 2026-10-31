import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';

// A layout-only fixture: use the shipped selectors and CSS without opening a
// real room, answering its question, or writing any application state.
const styles = [
  '../../src/styles/tokens.css',
  '../../src/pages/DiscussionsPage.css',
  '../../src/components/DiscussionPanelSwitcher.css',
  '../../src/components/DiscussionQuestionBanner.css',
].map(path => readFileSync(new URL(path, import.meta.url), 'utf8')).join('\n');

// KT-1344 — nothing floats over the conversation any more: the panel rail lives
// in the side panel's own column, and the in-conversation search is gone. What
// is left to guard is the arbitration banner's reveal button inside its column.
for (const width of [360, 1200]) {
  test(`arbitration reveal button stays reachable in the conversation column: ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 600 });
    await page.setContent(`
      <style>* { box-sizing: border-box; } body { margin: 0; }</style>
      <div class="disc-messages-git-row" style="height: 400px">
        <div class="disc-messages-col">
          <div class="disc-question-banner">
            <svg width="13" height="13" aria-hidden="true"></svg>
            <span class="disc-question-banner-count">1 arbitrage attendu</span>
            <span class="disc-question-banner-text">Une question assez longue pour vérifier la place du bouton de navigation</span>
            <button id="reveal" onclick="this.dataset.clicked='true'">Voir</button>
          </div>
          <div class="disc-messages">Conversation</div>
        </div>
      </div>
    `);
    await page.addStyleTag({ content: styles });
    const reveal = page.locator('#reveal');
    expect(await reveal.evaluate(button => {
      const box = button.getBoundingClientRect();
      return button.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
    })).toBe(true);
    await reveal.click();
    await expect(reveal).toHaveAttribute('data-clicked', 'true');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}
