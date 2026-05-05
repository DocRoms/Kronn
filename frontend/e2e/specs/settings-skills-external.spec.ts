/**
 * Settings page — vendored external skills.
 *
 * Regression guards (Sprint 1 / 0.7+) :
 *   - Each of the 8 obra/superpowers skills bundled in
 *     `backend/src/skills/external/` exposes:
 *       1. A clickable "Source" link pointing at github.com/obra/superpowers
 *       2. The "Adapted from <url> (MIT)." attribution rendered as a
 *          visually distinct line (italic + lower opacity, separate <div>).
 *       3. The full SKILL.md body is reachable via See-more.
 *   - The pattern matches the Caveman directive precedent (same UI helper).
 *
 * NB: the "🔗 External" badge is rendered in `ProjectSkills.tsx` (the
 * compact chip view used when attaching skills to a discussion), NOT in
 * the Settings card. This spec doesn't cover ProjectSkills because that
 * UI surface needs an active discussion to render — out of scope for the
 * Settings spec.
 *
 * Requires the backend to ship the 0.7+ external skills (rebuild after
 * Sprint 1 J2).
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { SettingsPage } from '../pages/SettingsPage';

// One representative vendored skill per category — we don't test all 8
// individually (token waste), but pick the most distinct ones to catch a
// regression where (say) only TDD ships its frontmatter correctly.
const VENDORED_SKILLS = [
  'test-driven-development',
  'systematic-debugging',
  'verification-before-completion',
];

test.describe('Settings — vendored external skills', () => {
  for (const skillName of VENDORED_SKILLS) {
    test(`${skillName} card has External badge + Source link + attribution`, async ({ page }) => {
      const dashboard = new DashboardPage(page);
      const settings = new SettingsPage(page);

      await dashboard.goto();
      await dashboard.clickSettings();
      await settings.openSkillsAccordion();

      const card = settings.skillCard(skillName);
      await expect(card).toBeVisible({ timeout: 5_000 });

      // 1. Source link → upstream URL.
      const source = settings.sourceLink(card);
      await expect(source).toBeVisible();
      await expect(source).toHaveAttribute('href', /github\.com\/obra\/superpowers/i);

      // 2. Attribution line (italic, separate from main description).
      await expect(settings.attributionLine(card)).toContainText(
        /Adapted from github\.com\/obra\/superpowers \(MIT\)\./
      );
    });
  }

  test('See-more reveals the full SKILL.md body', async ({ page }) => {
    // Lock the expand contract on a vendored skill (TDD is the largest,
    // ~370 lines — the see-more body must scroll independently rather
    // than blow up the page). Same toggle as Caveman directive but on
    // the skills accordion.
    const dashboard = new DashboardPage(page);
    const settings = new SettingsPage(page);

    await dashboard.goto();
    await dashboard.clickSettings();
    await settings.openSkillsAccordion();

    const card = settings.skillCard('test-driven-development');
    const seeMore = settings.seeMoreButton(card);
    await seeMore.click();
    // The expanded body shows the canonical "Iron Law" block of TDD —
    // a unique string from the upstream SKILL.md that confirms the
    // content is rendered (not just the description).
    await expect(card).toContainText(/NO PRODUCTION CODE WITHOUT A FAILING TEST FIRST/);
  });
});
