/**
 * a11y axe-core scan — WCAG 2.1 AA on the main pages.
 *
 * Walks the major routes (Projects / Discussions / Planning / Plugins /
 * Workflows / Pages / Settings) and runs `@axe-core/playwright` against each.
 * Fails on `serious` or `critical` violations; logs `moderate` and
 * `minor` ones so we have visibility without blocking CI.
 *
 * KT-1887: each page also opens one dialog and one form, and axe scans
 * what opened (`include`), so a dialog or a form is held to the same bar
 * as the page around it. Their baseline keys are `<Page> › <state>`.
 *
 * KT-1923: Settings is five sub-pages; each is scanned with its folds
 * open, all against the one `Settings` baseline entry.
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
 * Zero $. Seven page navigations plus two per page for the open dialog
 * and form, no agent runs.
 */
import { test, expect, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { UsageReport } from '../../src/types/generated';
import { DashboardPage, SETTINGS_SUB_PAGES, type SettingsSubPage } from '../pages/DashboardPage';

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
  /** A rendered page boundary; collection routes use the shared sidebar class. */
  readySelector?: string;
  /** Embedded live-page documents are user-authored, not app chrome. */
  excludeSelectors?: string[];
  enablePages?: boolean;
  /** KT-1923 — a page made of sub-pages (Settings): each is scanned, its
   *  folds open, against the page's one baseline entry. */
  subPages?: readonly SettingsSubPage[];
  /** KT-1887 — the dialog and the form this page opens for the scan. */
  states: OpenState[];
}

/**
 * KT-1887 — a dialog or a form the page opens. `open` clicks what a user
 * clicks and returns the CSS selector of the surface that opened; axe scans
 * that surface only, so the page's own baseline is not counted twice.
 * `kind` lists what the surface is: the import dialog of Pages holds its
 * form, so it covers both.
 */
interface OpenState {
  name: string;
  kind: Array<'dialog' | 'form'>;
  open: (page: Page) => Promise<string>;
}

/** Opens the first `?` help popover of the page (ContextHelp, role="dialog"). */
async function openContextHelp(page: Page): Promise<string> {
  await page.locator('.kr-context-help-trigger').first().click();
  const selector = '.kr-context-help-panel[role="dialog"]';
  await expect(page.locator(selector)).toBeVisible();
  return selector;
}

async function opened(page: Page, selector: string): Promise<string> {
  await expect(page.locator(selector).first()).toBeVisible();
  return selector;
}

/**
 * KT-1923 — Settings is five sub-pages and its tab opens the first. Shows
 * one, opens its folds (catalogue, usage, API call log, debug log viewer) so
 * axe scans what they hold, and waits for the usage report on the agents
 * sub-page (the fixture of `openRoute`).
 */
async function showSettingsSubPage(page: Page, subPage: SettingsSubPage): Promise<void> {
  const dashboard = new DashboardPage(page);
  await dashboard.selectSettingsSubPage(subPage);
  await dashboard.openSettingsFolds();
  if (subPage === 'agents') {
    await expect(page.getByTestId('usage-total-cost')).toBeVisible();
    await expect(page.getByTestId('usage-agent-filter')).toBeVisible();
    await expect(page.getByTestId('usage-model-filter')).toBeVisible();
  }
  await page.waitForLoadState('networkidle', { timeout: 10_000 });
}

const BOOTSTRAP_MODAL = '[role="dialog"][aria-labelledby="bootstrap-modal-title"]';

const ROUTES: PageRoute[] = [
  {
    name: 'Projects', navTourId: 'nav-projects', readySelector: 'aside.collection-shell-sidebar',
    states: [
      {
        // Dashboard.tsx — the "+ Project" modal, on its Bootstrap tab.
        name: 'new project dialog', kind: ['dialog'],
        open: async page => {
          await page.locator('[data-tour-id="new-project-btn"]').click();
          return opened(page, BOOTSTRAP_MODAL);
        },
      },
      {
        // The same modal on its Clone tab: the git URL form.
        name: 'clone form', kind: ['form'],
        open: async page => {
          await page.locator('[data-tour-id="new-project-btn"]').click();
          await page.locator('[data-tour-id="tab-clone"]').click();
          await expect(page.locator(`${BOOTSTRAP_MODAL} input`).first()).toBeVisible();
          return BOOTSTRAP_MODAL;
        },
      },
    ],
  },
  {
    name: 'Discussions', navTourId: 'nav-discussions', readySelector: 'aside.collection-shell-sidebar',
    states: [
      {
        // NewDiscussionForm.tsx — the new discussion modal.
        name: 'new discussion dialog', kind: ['dialog'],
        open: async page => {
          await page.locator('[data-tour-id="new-disc-btn"]').click();
          return opened(page, '.disc-new-overlay[role="dialog"]');
        },
      },
      {
        // GlobalSearchPanel.tsx — Enter in the sidebar search opens the
        // message search and its filters (DiscussionSidebar onSearchSubmit).
        // The sidebar replaces the shell's search field with its own
        // `.disc-search-input` (DiscussionSidebar renderSearch), present
        // with no discussion.
        name: 'message search form', kind: ['form'],
        open: async page => {
          await page.locator('aside.collection-shell-sidebar .disc-search-input').press('Enter');
          return opened(page, '[data-testid="global-search-panel"]');
        },
      },
    ],
  },
  {
    name: 'Planning', navTourId: 'nav-planning', readySelector: 'aside.collection-shell-sidebar',
    states: [
      { name: 'help dialog', kind: ['dialog'], open: openContextHelp },
      {
        // PlanningPage.tsx — the "+" of the sidebar opens the task creation form.
        name: 'new task form', kind: ['form'],
        open: async page => {
          await page.locator('aside.collection-shell-sidebar .collection-shell-primary-action').click();
          return opened(page, '.planning-create-modal[role="dialog"]');
        },
      },
    ],
  },
  {
    name: 'Plugins', navTourId: 'nav-mcps', readySelector: 'aside.collection-shell-sidebar',
    states: [
      {
        // PluginPortabilityModal.tsx in import mode. Its toolbar button is
        // always rendered (PluginToolbar.tsx); the export one only once a
        // plugin is configured, which a fresh backend has not.
        name: 'import dialog', kind: ['dialog'],
        open: async page => {
          await page.locator('.mcp-collection-toolbar-actions').getByRole('button', { name: 'Importer', exact: true }).click();
          await expect(page.getByRole('dialog', { name: 'Importer des plugins' })).toBeVisible();
          return '.mcp-portability-modal[role="dialog"]';
        },
      },
      {
        // AddPluginModal.tsx — the catalogue search and the plugin form.
        name: 'add plugin form', kind: ['form'],
        open: async page => {
          await page.locator('[data-tour-id="add-plugin-btn"]').click();
          return opened(page, '.mcp-add-modal[role="dialog"]');
        },
      },
    ],
  },
  {
    name: 'Workflows', navTourId: 'nav-workflows', readySelector: 'aside.collection-shell-sidebar',
    states: [
      {
        // WorkflowsPage.tsx — the "+" of the sidebar: what can be created.
        name: 'create dialog', kind: ['dialog'],
        open: async page => {
          await page.locator('[data-tour-id="automation-actions"]').click();
          return opened(page, '.automation-action-modal[role="dialog"]');
        },
      },
      {
        // QuickPromptForm.tsx — "Nouveau prompt" (qp.new) in that dialog.
        name: 'quick prompt form', kind: ['form'],
        open: async page => {
          await page.locator('[data-tour-id="automation-actions"]').click();
          await page.locator('.automation-action-modal').getByRole('button', { name: 'Nouveau prompt' }).click();
          return opened(page, '.qp-form');
        },
      },
    ],
  },
  {
    name: 'Pages', navTourId: 'nav-pages', readySelector: 'aside.collection-shell-sidebar', enablePages: true, excludeSelectors: ['iframe'],
    states: [
      {
        // ArtifactImportDialog.tsx — the import dialog holds the import
        // form (file, project): with no page, the only form of the route.
        name: 'import dialog and form', kind: ['dialog', 'form'],
        open: async page => {
          await page.getByRole('button', { name: 'Importer un Artifact' }).click();
          return opened(page, '.artifact-import-dialog[role="dialog"]');
        },
      },
    ],
  },
  {
    name: 'Settings', navTourId: 'nav-settings', readySelector: '.settings-page', subPages: SETTINGS_SUB_PAGES,
    states: [
      {
        // IdentitySection.tsx — the help of the bio field, on « Vous »,
        // the first help of the page as on the single page.
        name: 'help dialog', kind: ['dialog'],
        open: async page => {
          await showSettingsSubPage(page, 'you');
          await page.locator('.set-subpage:not([hidden]) .kr-context-help-trigger').first().click();
          return opened(page, '.kr-context-help-panel[role="dialog"]');
        },
      },
      {
        // ExternalApiSection.tsx — "add a connection" opens its form; it is
        // in the agents section, on « Agents et modèles ».
        name: 'connection form', kind: ['form'],
        open: async page => {
          await showSettingsSubPage(page, 'agents');
          const add = page.getByTestId('ext-api-add-connection');
          await add.scrollIntoViewIfNeeded();
          await add.click();
          return opened(page, '[data-testid="ext-api-form"]');
        },
      },
    ],
  },
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

async function scanPage(page: Page, excludeSelectors: string[] = [], include?: string): Promise<{ violations: Array<{ id: string; impact: string | null | undefined; nodes: number; targets: string[] }> }> {
  const builder = new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21aa'])
    .disableRules(DISABLED_RULES);
  if (include) builder.include(include);
  excludeSelectors.forEach(selector => builder.exclude(selector));
  const results = await builder.analyze();
  return {
    violations: results.violations.map(v => ({
      id: v.id,
      impact: v.impact,
      nodes: v.nodes.length,
      targets: v.nodes.flatMap(node => node.target.map(String)),
    })),
  };
}

test.describe.configure({ timeout: 60_000, retries: 0 });

/** Opens a route as a user does, with its fixtures, and waits for its data. */
async function openRoute(page: Page, route: PageRoute): Promise<void> {
  await page.addInitScript(() => {
    try { window.localStorage.setItem('kronn:tour-completed', 'true'); } catch { /* noop */ }
  });
  if (route.enablePages) {
    await page.route('**/api/pages/capability', async request => {
      await request.fulfill({ json: { success: true, data: { activated: true, activated_at: '2026-08-29T00:00:00Z' }, error: null } });
    });
  }
  if (route.name === 'Settings') {
    // ccusage is an external CLI, not app chrome. A cold/offline collector
    // can wait on its provider catalogue indefinitely. Give axe a populated,
    // typed report so it scans the real filters and controls, not a loader
    // (nor the operator's private usage history). All other data stays live.
    await page.route('**/api/usage?*', async request => {
      const totals = {
        input_tokens: 7000, output_tokens: 1000, cache_creation_tokens: 500,
        cache_read_tokens: 4000, total_tokens: 12500, total_cost: 0.42,
      };
      const report: UsageReport = {
        period_kind: new URL(request.request().url()).searchParams.get('period') ?? 'daily',
        rows: [{
          period: '2026-05-28', agent: 'claude', models_used: ['claude-opus-4-7'],
          model_breakdowns: [{
            model_name: 'claude-opus-4-7', cost: totals.total_cost,
            input_tokens: totals.input_tokens, output_tokens: totals.output_tokens,
            cache_creation_tokens: totals.cache_creation_tokens,
            cache_read_tokens: totals.cache_read_tokens, total_tokens: totals.total_tokens,
          }],
          ...totals,
        }],
        totals, agents_detected: ['claude'],
      };
      await request.fulfill({ json: { success: true, data: report, error: null } });
    });
  }
  await page.goto('/');
  const nav = page.locator(`[data-tour-id="${route.navTourId}"]`);
  await nav.click();
  await expect(nav).toHaveAttribute('aria-current', 'page');
  const readySelector = route.readySelector;
  if (!readySelector) throw new Error(`Missing readiness selector for ${route.name}`);
  await expect(page.locator(readySelector)).toBeVisible();
  // The shell selector proves that the lazy route mounted. The bounded
  // network-idle wait is fail-closed so axe never scans partial data.
  await page.waitForLoadState('networkidle', { timeout: 10_000 });
}

type Violations = Awaited<ReturnType<typeof scanPage>>['violations'];

/** Logs a scan and fails on a serious/critical rule beyond the baseline of
 *  `name` (a page, or `<Page> › <state>`). */
async function checkAgainstBaseline(name: string, violations: Violations, label = name): Promise<void> {
  const slug = label.toLowerCase().replace(/[^a-z0-9]+/g, '-');
  const seriousOrCritical = violations.filter(v => v.impact === 'serious' || v.impact === 'critical');
  const moderate = violations.filter(v => v.impact === 'moderate');
  const minor = violations.filter(v => v.impact === 'minor');

  // Log everything for visibility — the test only fails on the
  // serious/critical bucket. Moderate and minor are surfaced in
  // the test output so a per-PR axe-diff is possible.
  // eslint-disable-next-line no-console
  console.log(`[a11y] ${label}: serious/critical=${seriousOrCritical.length}, moderate=${moderate.length}, minor=${minor.length}`);
  for (const v of seriousOrCritical) {
    // eslint-disable-next-line no-console
    console.log(`  ✗ ${v.id} (${v.impact}) — ${v.nodes} node(s): ${v.targets.join(', ')}`);
  }
  await test.info().attach(`${slug}-axe-serious-critical`, {
    body: JSON.stringify(seriousOrCritical, null, 2),
    contentType: 'application/json',
  });
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
  const baseline = baselines[name] ?? {};
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
      console.log(`  📉 ${label}: ${rule} dropped to ${seen} (baseline=${allowed}). Lower the baseline when this PR ships.`);
    }
  }
  expect(
    regressions,
    `${label} regressed past the a11y baseline:\n  ${regressions.join('\n  ')}`,
  ).toEqual([]);
}

test.describe('a11y — axe-core scans main pages, fails on serious/critical', () => {
  for (const route of ROUTES) {
    if (route.subPages) {
      // KT-1923 — every sub-page, folds open, each held to the page's baseline.
      for (const subPage of route.subPages) {
        const label = `${route.name} › ${subPage}`;
        test(`${label} has no serious/critical a11y violations`, async ({ page }) => {
          await openRoute(page, route);
          await showSettingsSubPage(page, subPage);
          const { violations } = await scanPage(page, route.excludeSelectors);
          await checkAgainstBaseline(route.name, violations, label);
        });
      }
      continue;
    }
    test(`${route.name} has no serious/critical a11y violations`, async ({ page }) => {
      await openRoute(page, route);
      const { violations } = await scanPage(page, route.excludeSelectors);
      await checkAgainstBaseline(route.name, violations);
    });
  }
});

// KT-1887 — a dialog and a form per page, scanned open.
test.describe('a11y — axe-core scans an open dialog and an open form on each page', () => {
  test('every page declares a dialog and a form to open', () => {
    for (const route of ROUTES) {
      const kinds = new Set(route.states.flatMap(state => state.kind));
      expect([...kinds].sort(), route.name).toEqual(['dialog', 'form']);
    }
  });

  for (const route of ROUTES) {
    for (const state of route.states) {
      const name = `${route.name} › ${state.name}`;
      test(`${name} has no serious/critical a11y violations`, async ({ page }) => {
        await openRoute(page, route);
        const surface = await state.open(page);
        const { violations } = await scanPage(page, route.excludeSelectors, surface);
        await checkAgainstBaseline(name, violations);
      });
    }
  }
});
