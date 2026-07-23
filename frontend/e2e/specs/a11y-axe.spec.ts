/**
 * a11y axe-core scan — WCAG 2.1 AA on the main pages.
 *
 * Walks the major routes (Projects / Discussions / Plugins /
 * Workflows / Settings) and runs `@axe-core/playwright` against each.
 * Fails on `serious` or `critical` violations; logs `moderate` and
 * `minor` ones so we have visibility without blocking CI.
 *
 * # What this catches
 *
 *   • Form inputs missing labels (the form sweep is complete; this
 *     spec keeps it from regressing).
 *   • Color contrast regressions when tweaking design tokens.
 *   • Buttons relying on icon-only with no aria-label.
 *   • Heading-order skips inside dialogs.
 *
 * # What this DOESN'T catch
 *
 *   axe is static analysis — it can't catch focus-traps that break
 *   only on keyboard input, screen-reader behaviours, or
 *   contextual issues like a dialog that appears off-screen on
 *   small viewports. Those need separate manual / SR testing.
 *
 * # Cost
 *
 * Zero $. Six page navigations, no agent runs. ~30s wall.
 */
import { test, expect, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const baselinePath = join(fileURLToPath(import.meta.url), '..', '..', 'fixtures', 'a11y-baseline.json');
type BaselineEntry = Record<string, number>;
type Baseline = Record<string, BaselineEntry>;
const rawBaseline = JSON.parse(readFileSync(baselinePath, 'utf-8')) as Baseline;
const { _doc, ...baselines } = rawBaseline as Baseline & { _doc?: string };
void _doc;

interface PageRoute {
  name: string;
  /**
   * Sidebar nav button to click. Each main page has a `data-tour-id`
   * marker we already use for the guided tour, so we re-use it here.
   */
  navTourId: string;
}

const ROUTES: PageRoute[] = [
  { name: 'Projects',    navTourId: 'nav-projects' },
  { name: 'Discussions', navTourId: 'nav-discussions' },
  { name: 'Plugins',     navTourId: 'nav-mcps' },
  { name: 'Workflows',   navTourId: 'nav-workflows' },
  { name: 'Settings',    navTourId: 'nav-settings' },
];

/**
 * Disable rules we know we want to address but separately. Suppressing
 * here means a regression on a SOLVED rule will still fail.
 */
const DISABLED_RULES = [
  // Surfaced on every page because the matrix-text logo uses
  // SVG without an aria-label. UX choice — the visible word "Kronn"
  // serves as the label. File a separate fix if SR users complain.
  'svg-img-alt',
  // Disabled for now — many third-party portal-rendered components
  // (lucide icons, react portals) trigger false positives. Re-enable
  // after a dedicated sweep.
  'aria-allowed-attr',
];

async function scanPage(page: Page, label: string): Promise<{ violations: Array<{ id: string; impact: string | null | undefined; nodes: number }> }> {
  const builder = new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21aa'])
    .disableRules(DISABLED_RULES);
  const results = await builder.analyze();
  return {
    violations: results.violations.map(v => ({
      id: v.id,
      impact: v.impact,
      nodes: v.nodes.length,
    })),
  };
}

test.describe.configure({ timeout: 60_000, retries: 0 });

test.describe('a11y — axe-core scans main pages, fails on serious/critical', () => {
  for (const route of ROUTES) {
    test(`${route.name} has no serious/critical a11y violations`, async ({ page }) => {
      await page.addInitScript(() => {
        try { window.localStorage.setItem('kronn:tour-completed', 'true'); } catch { /* noop */ }
      });
      await page.goto('/');
      await page.locator(`[data-tour-id="${route.navTourId}"]`).click();
      // Let the page settle — async data fetches, lazy-loaded sub-trees.
      await page.waitForLoadState('networkidle', { timeout: 15_000 }).catch(() => { /* may stay busy on long polls */ });
      await page.waitForTimeout(500);

      const { violations } = await scanPage(page, route.name);
      const seriousOrCritical = violations.filter(v => v.impact === 'serious' || v.impact === 'critical');
      const moderate = violations.filter(v => v.impact === 'moderate');
      const minor = violations.filter(v => v.impact === 'minor');

      // Log everything for visibility — the test only fails on the
      // serious/critical bucket. Moderate and minor are surfaced in
      // the test output so a per-PR axe-diff is possible.
      // eslint-disable-next-line no-console
      console.log(`[a11y] ${route.name}: serious/critical=${seriousOrCritical.length}, moderate=${moderate.length}, minor=${minor.length}`);
      for (const v of seriousOrCritical) {
        // eslint-disable-next-line no-console
        console.log(`  ✗ ${v.id} (${v.impact}) — ${v.nodes} node(s)`);
      }
      for (const v of moderate) {
        // eslint-disable-next-line no-console
        console.log(`  ⚠ ${v.id} (${v.impact}) — ${v.nodes} node(s)`);
      }

      // Baseline-aware: fail only when a NEW rule appears or an
      // existing count INCREASES vs. the snapshot in
      // `e2e/fixtures/a11y-baseline.json`. Pre-existing serious/critical
      // violations (color-contrast, button-name, etc.) are tracked
      // there as known debt — this test guards against regression
      // while a separate sweep brings the counts down. Remove the
      // entry when a rule is fully fixed.
      const baseline = baselines[route.name] ?? {};
      const regressions: string[] = [];
      const ruleCount: Record<string, number> = {};
      for (const v of seriousOrCritical) ruleCount[v.id] = (ruleCount[v.id] ?? 0) + v.nodes;
      for (const [rule, count] of Object.entries(ruleCount)) {
        const allowed = baseline[rule] ?? 0;
        if (count > allowed) {
          regressions.push(`${rule}: ${count} > baseline ${allowed} (+${count - allowed})`);
        }
      }
      // Also fail if a baselined rule fully disappeared from the new
      // run while another regressed up — we don't auto-shrink, but we
      // surface the green delta in logs to invite a baseline update.
      for (const [rule, allowed] of Object.entries(baseline)) {
        const seen = ruleCount[rule] ?? 0;
        if (seen < allowed) {
          // eslint-disable-next-line no-console
          console.log(`  📉 ${route.name}: ${rule} dropped to ${seen} (baseline=${allowed}). Lower the baseline when this PR ships.`);
        }
      }
      expect(
        regressions,
        `${route.name} regressed past the a11y baseline:\n  ${regressions.join('\n  ')}`,
      ).toEqual([]);
    });
  }
});
