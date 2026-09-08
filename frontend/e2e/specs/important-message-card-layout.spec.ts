import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';

// KT-619 — a layout-only fixture, same pattern as
// discussion-question-layout.spec.ts: the real CSS and markup shape, no
// backend and no mounted React component. Filter/counter/navigation LOGIC is
// covered deterministically by ImportantMessageCard.test.tsx; this spec only
// asks whether the full-width card and the bar above it stay usable in a real
// browser viewport — no horizontal overflow, and the prev/next buttons remain
// clickable at their own center rather than hidden under something else.
const styles = [
  '../../src/styles/tokens.css',
  '../../src/components/ImportantMessageCard.css',
].map(path => readFileSync(new URL(path, import.meta.url), 'utf8')).join('\n');

const markup = `
  <div class="disc-important-bar" role="group" aria-label="Important messages">
    <button type="button" class="disc-important-count" aria-label="Go to the current important message">3 important</button>
    <label>
      <span class="sr-only">Filter by category</span>
      <select aria-label="Filter by category">
        <option value="">All categories</option>
        <option value="blocking_alert">Blocking alert</option>
      </select>
    </label>
    <button type="button" aria-label="Previous important message">‹</button>
    <button type="button" id="next" aria-label="Next important message">›</button>
    <span class="disc-important-position">1 of 3</span>
  </div>
  <article class="disc-important-card" data-category="blocking_alert" id="important-i-1">
    <div class="disc-important-head">
      <span class="disc-important-category">Blocking alert</span>
      <h3 class="disc-important-title">Un titre assez long pour vérifier le retour à la ligne du bandeau</h3>
      <span class="disc-important-meta">Orchestrator · Codex · 07/09/2026</span>
    </div>
    <p class="disc-important-highlight">La release part sans KT-610, ce qui doit rester lisible d'un coup d'œil.</p>
    <div class="disc-important-body">
      <div>
        <span class="disc-important-label">Impact</span>
        <span class="disc-important-value">Les captures d'écran arrivent en 0.13.1.</span>
      </div>
      <div class="disc-important-action">
        <span class="disc-important-label">Action</span>
        <span class="disc-important-value">Trancher · Romu · lundi</span>
      </div>
      <ul class="disc-important-refs">
        <li>Task: KT-619</li>
        <li>Commit: d44900db</li>
      </ul>
    </div>
  </article>
`;

for (const width of [360, 1200]) {
  test(`the bar and the card stay usable at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 700 });
    await page.setContent(`<style>* { box-sizing: border-box; } body { margin: 0; }</style>${markup}`);
    await page.addStyleTag({ content: styles });

    // Full-width card, real bandeau: neither may force the page to scroll
    // sideways, at a phone width or a desktop one.
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);

    const next = page.locator('#next');
    expect(await next.evaluate(button => {
      const box = button.getBoundingClientRect();
      return button.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
    })).toBe(true);

    // Keyboard users must see where they landed.
    await page.getByLabel('Filter by category').focus();
    const outline = await page.getByLabel('Filter by category').evaluate(
      el => getComputedStyle(el).outlineStyle,
    );
    expect(outline).not.toBe('none');
  });
}
