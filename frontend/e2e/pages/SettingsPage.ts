import type { Page, Locator } from '@playwright/test';

/**
 * Config / Settings page (`Config` tab). Skills + Directives are inside
 * collapsible accordions (`#settings-skills` / `#settings-directives`).
 * Tests must `openSkillsAccordion()` / `openDirectivesAccordion()` before
 * interacting with cards.
 *
 * Both skills and directives use the same CSS class `.set-item-card`.
 * To disambiguate we scope locators inside the accordion's `id` parent.
 */
export class SettingsPage {
  constructor(private readonly page: Page) {}

  // ─── Accordion toggles ──────────────────────────────────────────────
  get skillsAccordionHeader(): Locator {
    return this.page.locator('#settings-skills .set-accordion-header');
  }
  get directivesAccordionHeader(): Locator {
    return this.page.locator('#settings-directives .set-accordion-header');
  }

  /** Click to expand the Skills accordion if collapsed. Idempotent.
   *  Scrolls into view first because the accordion sits low on the page
   *  (Settings has many sections above it). */
  async openSkillsAccordion() {
    const header = this.skillsAccordionHeader;
    await header.scrollIntoViewIfNeeded();
    if ((await header.getAttribute('aria-expanded')) !== 'true') {
      await header.click();
    }
    // Wait for the body to actually render (the accordion is conditional —
    // not just a CSS toggle).
    await this.page.locator('#settings-skills .set-item-card').first().waitFor({ state: 'visible' });
  }

  async openDirectivesAccordion() {
    const header = this.directivesAccordionHeader;
    await header.scrollIntoViewIfNeeded();
    if ((await header.getAttribute('aria-expanded')) !== 'true') {
      await header.click();
    }
    await this.page.locator('#settings-directives .set-item-card').first().waitFor({ state: 'visible' });
  }

  // ─── Card locators (scoped to their accordion) ─────────────────────
  /** Skill card by its exact name. Match goes through the card's heading
   *  span (`.font-semibold`) which holds only the skill name. Without this
   *  exact-match strategy a substring like "test-driven-development" also
   *  matches `testing` and `systematic-debugging` cards. */
  skillCard(name: string): Locator {
    return this.page
      .locator('#settings-skills .set-item-card')
      .filter({
        has: this.page.locator('.font-semibold').getByText(name, { exact: true }),
      });
  }

  /** Directive card — same exact-match logic. The directive heading
   *  contains "{icon} {name}" so we match the trailing name; an exact
   *  match wouldn't include the leading icon glyph. */
  directiveCard(nameRe: RegExp | string): Locator {
    return this.page
      .locator('#settings-directives .set-item-card')
      .filter({ hasText: nameRe });
  }

  // ─── Card-level locators (used by specs) ────────────────────────────
  /** "🔗 External" badge on a skill card. Only present when skill.external === true. */
  externalBadge(card: Locator): Locator {
    return card.getByText(/🔗 External/);
  }

  /** "Source" link (anchor) — clickable, opens upstream URL in a new tab. */
  sourceLink(card: Locator): Locator {
    return card.getByRole('link', { name: /Source/i });
  }

  /** "See more" / "See less" toggle button on the card body. */
  seeMoreButton(card: Locator): Locator {
    return card.locator('.set-see-more-btn');
  }

  /** Italic attribution text rendered by AttributedDescription helper. Matches
   *  the canonical "Adapted from <url> (<license>)." suffix. */
  attributionLine(card: Locator): Locator {
    return card.locator('div').filter({ hasText: /Adapted from .+ \(.+\)\./ }).first();
  }
}
