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

for (const width of [360, 1200]) {
  for (const searchOpen of [false, true]) {
    test(`arbitration reveal button stays clear of the floating rail: ${width}px, search ${searchOpen}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 600 });
      await page.setContent(`
        <style>* { box-sizing: border-box; } body { margin: 0; }</style>
        <div class="disc-messages-git-row" data-rail-floating="true" data-search-open="${searchOpen}" style="height: 400px">
          <div class="disc-messages-col">
            ${searchOpen ? '<div class="disc-message-search"><input type="search" aria-label="Search"></div>' : ''}
            <div class="disc-question-banner">
              <svg width="13" height="13" aria-hidden="true"></svg>
              <span class="disc-question-banner-count">1 arbitrage attendu</span>
              <span class="disc-question-banner-text">Une question assez longue pour vérifier la place du bouton de navigation</span>
              <button id="reveal" onclick="this.dataset.clicked='true'">Voir</button>
            </div>
            <div class="disc-messages">Conversation</div>
          </div>
          <div class="disc-utility-col" data-floating="true">
            <div class="disc-panel-switcher" data-open="false">
              <button class="disc-panel-switcher-item" aria-label="Open panel"><svg width="14" height="14"></svg></button>
              <button class="disc-panel-switcher-item" aria-label="Search"><svg width="14" height="14"></svg></button>
            </div>
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
}
