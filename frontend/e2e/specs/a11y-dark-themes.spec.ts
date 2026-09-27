/**
 * Dark-theme contrast — axe `color-contrast` on the main screens in every dark
 * theme (default, gotham, matrix), with the native action card open.
 *
 * `a11y-axe.spec.ts` scans the theme the browser happens to resolve; this one
 * pins each dark theme through the same localStorage keys the theme picker
 * writes, so a token tuned for one theme cannot regress another.
 *
 * Read-only by construction: every non-GET request is answered 503 and the
 * WebSocket never reaches the server, so it can run against a live backend:
 *   VITE_DEV_PORT=5199 pnpm test:e2e e2e/specs/a11y-dark-themes.spec.ts
 */
import { test, expect, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import type { DiscussionAction } from '../../src/types/generated';

const DARK_THEMES = ['dark', 'gotham', 'matrix'] as const;
const SECRET_THEMES = ['matrix', 'sakura', 'gotham', 'euronews'];

interface Route {
  name: string;
  navTourId: string;
  readySelector: string;
  exclude?: string[];
}

const ROUTES: Route[] = [
  { name: 'Projects', navTourId: 'nav-projects', readySelector: 'aside.collection-shell-sidebar' },
  { name: 'Discussions', navTourId: 'nav-discussions', readySelector: 'aside.collection-shell-sidebar' },
  { name: 'Planning', navTourId: 'nav-planning', readySelector: 'aside.collection-shell-sidebar' },
  { name: 'Plugins', navTourId: 'nav-mcps', readySelector: 'aside.collection-shell-sidebar' },
  { name: 'Workflows', navTourId: 'nav-workflows', readySelector: 'aside.collection-shell-sidebar' },
  // Embedded Page documents are user-authored, not app chrome.
  { name: 'Pages', navTourId: 'nav-pages', readySelector: 'aside.collection-shell-sidebar', exclude: ['iframe'] },
  { name: 'Settings', navTourId: 'nav-settings', readySelector: '.settings-page' },
];

type ContrastNode = { target: string; fg?: string; bg?: string; ratio?: number; expected?: number };

async function readOnly(page: Page, theme: string) {
  await page.addInitScript(([name, secret]) => {
    try {
      window.localStorage.setItem('kronn:tour-completed', 'true');
      window.localStorage.setItem('kronn:theme', name as string);
      window.localStorage.setItem('kronn:unlockedThemes', JSON.stringify(secret));
    } catch { /* storage disabled: the theme assertion below fails loudly */ }
  }, [theme, SECRET_THEMES] as const);
  await page.routeWebSocket(/\/api\/ws/, () => { /* never connected to the server */ });
  await page.route('**/*', route => {
    const method = route.request().method();
    if (method === 'GET' || method === 'HEAD') return route.fallback();
    return route.fulfill({ status: 503, body: 'read-only contrast scan' });
  });
  await page.route('**/api/pages/capability', route => route.fulfill({
    json: { success: true, data: { activated: true, activated_at: '2026-08-29T00:00:00Z' }, error: null },
  }));
}

/** axe leaves text over a decorative layer (a gradient tint, a pseudo-element
 *  accent bar, an overlapping sibling) undecided; those reasons are re-measured
 *  here against the stack of opaque and translucent backgrounds behind them. */
const REMEASURED = new Set(['bgGradient', 'pseudoContent', 'bgOverlap']);

type Remeasured = { target: string; fg: string; bg: string; large: boolean };

async function remeasure(page: Page, targets: string[]): Promise<Remeasured[]> {
  return page.evaluate((selectors) => {
    // `rgb(r g b / a)` in 0-255, or `color(srgb r g b / a)` in 0-1 (color-mix).
    const parse = (css: string) => {
      const [r, g, b, a] = (css.replace(/^color\(srgb/, '').match(/[\d.]+(e-?\d+)?/g) ?? ['0', '0', '0']).map(Number);
      const scale = css.startsWith('color(srgb') ? 255 : 1;
      return { r: r * scale, g: g * scale, b: b * scale, a: a ?? 1 };
    };
    const over = (top: { r: number; g: number; b: number; a: number }, under: { r: number; g: number; b: number }) => ({
      r: top.r * top.a + under.r * (1 - top.a),
      g: top.g * top.a + under.g * (1 - top.a),
      b: top.b * top.a + under.b * (1 - top.a),
    });
    const rgb = (c: { r: number; g: number; b: number }) =>
      `rgb(${Math.round(c.r)}, ${Math.round(c.g)}, ${Math.round(c.b)})`;
    const out: Array<{ target: string; fg: string; bg: string; large: boolean }> = [];
    for (const selector of selectors) {
      const el = document.querySelector(selector) as HTMLElement | null;
      if (!el || el.closest('[disabled], [aria-disabled="true"]')) continue;
      const layers: Array<{ r: number; g: number; b: number; a: number }> = [];
      let opacity = 1;
      for (let node: HTMLElement | null = el; node; node = node.parentElement) {
        const style = getComputedStyle(node);
        opacity *= Number(style.opacity);
        const bg = parse(style.backgroundColor);
        if (style.backgroundColor === 'transparent' || bg.a === 0) continue;
        layers.push(bg);
        if (bg.a >= 1) break;
      }
      const opaque = layers.at(-1);
      if (opaque && opaque.a >= 1) layers.pop();
      let base = opaque && opaque.a >= 1 ? opaque : parse(getComputedStyle(document.body).backgroundColor);
      for (const layer of layers.reverse()) base = { ...over(layer, base), a: 1 };
      const style = getComputedStyle(el);
      const fg = parse(style.color);
      const px = Number.parseFloat(style.fontSize);
      const bold = Number(style.fontWeight) >= 700;
      out.push({
        target: selector,
        fg: rgb(over({ ...fg, a: fg.a * opacity }, base)),
        bg: rgb(base),
        large: px >= 24 || (bold && px >= 18.66),
      });
    }
    return out;
  }, targets);
}

async function contrastViolations(page: Page, label: string, exclude: string[] = [], include?: string): Promise<ContrastNode[]> {
  const builder = new AxeBuilder({ page }).withRules(['color-contrast']);
  if (include) builder.include(include);
  exclude.forEach(selector => builder.exclude(selector));
  const results = await builder.analyze();
  const checked = results.passes.reduce((n, r) => n + r.nodes.length, 0);
  const reasons = new Map<string, number>();
  const undecided: string[] = [];
  for (const node of results.incomplete.flatMap(r => r.nodes)) {
    const key = String((node.any[0]?.data as { messageKey?: string } | undefined)?.messageKey ?? 'other');
    reasons.set(key, (reasons.get(key) ?? 0) + 1);
    const target = node.target.length === 1 ? String(node.target[0]) : null;
    if (REMEASURED.has(key) && target) undecided.push(target);
  }
  const remeasured = await remeasure(page, undecided);
  const failing = remeasured
    .map(m => ({ ...m, ratio: contrast(m.fg, m.bg), expected: m.large ? 3 : 4.5 }))
    .filter(m => m.ratio < m.expected);
  // eslint-disable-next-line no-console
  console.log(`[contrast] ${label}: axe passed ${checked}, left ${[...reasons.values()].reduce((a, b) => a + b, 0)} `
    + `undecided ${JSON.stringify(Object.fromEntries(reasons))}, re-measured ${remeasured.length}`);
  expect(checked, `${label}: axe checked no text at all`).toBeGreaterThan(0);
  const axeFailures = results.violations.flatMap(v => v.nodes.map(node => {
    const data = (node.any[0]?.data ?? {}) as {
      fgColor?: string; bgColor?: string; contrastRatio?: number; expectedContrastRatio?: string;
    };
    return {
      target: node.target.map(String).join(' '),
      fg: data.fgColor, bg: data.bgColor, ratio: data.contrastRatio,
      expected: data.expectedContrastRatio ? Number.parseFloat(data.expectedContrastRatio) : undefined,
    };
  }));
  return [...axeFailures, ...failing.map(m => ({
    target: `${m.target} (re-measured)`, fg: m.fg, bg: m.bg, ratio: Number(m.ratio.toFixed(2)), expected: m.expected,
  }))];
}

function report(label: string, nodes: ContrastNode[]) {
  // eslint-disable-next-line no-console
  console.log(`[contrast] ${label}: ${nodes.length} violation(s)`);
  for (const n of nodes) {
    // eslint-disable-next-line no-console
    console.log(`  ✗ ${n.target} — ${n.fg} on ${n.bg} = ${n.ratio} (needs ${n.expected})`);
  }
}

/** The smallest discussion that holds an Agent message, to anchor a card on. */
async function discussionWithAgentMessage(page: Page): Promise<{ id: string; messageId: string } | null> {
  const list = await page.request.get('/api/discussions');
  if (!list.ok()) return null;
  const items = ((await list.json()) as { data: Array<{ id: string; message_count: number; archived: boolean }> }).data
    .filter(d => !d.archived && d.message_count > 0)
    .sort((a, b) => a.message_count - b.message_count)
    .slice(0, 12);
  for (const item of items) {
    const detail = await page.request.get(`/api/discussions/${item.id}`);
    if (!detail.ok()) continue;
    const messages = ((await detail.json()) as { data: { messages: Array<{ id: string; role: string }> } }).data.messages;
    const agent = [...messages].reverse().find(m => m.role === 'Agent');
    if (agent) return { id: item.id, messageId: agent.id };
  }
  return null;
}

function proposedAction(discussionId: string, messageId: string): DiscussionAction {
  const now = new Date().toISOString();
  return {
    id: 'contrast-scan-action', discussion_id: discussionId, source_message_id: messageId,
    fence_index: 0, kind: 'quick_prompt', target_id: 'contrast-scan-target',
    target_name: 'Contrast scan', project_id: null, project_name: null, state: 'proposed',
    values: [{
      name: 'ticket', label: 'Ticket', placeholder: 'e.g. KT-812', description: 'Ticket to review',
      required: false, allow_manual_override: false, provenance: 'user_input',
    }],
    shared_run_id: null, result_discussion_id: null, deep_link: null, diagnostic: null,
    launched_at: null, finished_at: null, created_at: now, updated_at: now,
  };
}

/** WCAG ratio of two opaque `rgb(…)` strings, as getComputedStyle reports them. */
function contrast(fg: string, bg: string): number {
  const lum = (css: string) => {
    const [r, g, b] = (css.match(/[\d.]+/g) ?? []).slice(0, 3).map(Number).map(v => {
      const s = v / 255;
      return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const [hi, lo] = [lum(fg), lum(bg)].sort((a, b) => b - a);
  return (hi + 0.05) / (lo + 0.05);
}

async function openRoute(page: Page, theme: string, route: Route) {
  await page.goto('/');
  await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  const nav = page.locator(`[data-tour-id="${route.navTourId}"]`);
  await nav.click();
  await expect(nav).toHaveAttribute('aria-current', 'page');
  await expect(page.locator(route.readySelector)).toBeVisible();
  // Bounded: live polling can keep the network busy; the ready selector above
  // already proves the route mounted.
  await page.waitForLoadState('networkidle', { timeout: 10_000 }).catch(() => undefined);
}

/** Tabs through the first `presses` stops; returns how many were checked and
 *  the ones whose indicator is missing or under 3:1 against the surface behind. */
async function focusStops(page: Page, presses: number): Promise<{ checked: number; misses: string[] }> {
  await page.locator('body').focus();
  const misses: string[] = [];
  let checked = 0;
  for (let i = 0; i < presses; i += 1) {
    await page.keyboard.press('Tab');
    const stop = await page.evaluate(() => {
      const el = document.activeElement as HTMLElement | null;
      if (!el || el === document.body || el.tagName === 'IFRAME') return null;
      const style = getComputedStyle(el);
      let behind = 'rgb(0, 0, 0)';
      for (let node: HTMLElement | null = el.parentElement; node; node = node.parentElement) {
        const bg = getComputedStyle(node).backgroundColor;
        const alpha = bg.startsWith('rgba') ? Number(bg.split(',')[3]?.replace(')', '')) : 1;
        if (bg !== 'transparent' && alpha >= 1) { behind = bg; break; }
      }
      const name = `${el.tagName.toLowerCase()}${el.className && typeof el.className === 'string' ? `.${el.className.trim().split(/\s+/)[0]}` : ''}`;
      return {
        name, behind, ring: style.boxShadow !== 'none',
        outline: style.outlineStyle !== 'none' && Number.parseFloat(style.outlineWidth) >= 1 ? style.outlineColor : null,
      };
    });
    if (!stop) continue;
    checked += 1;
    const visible = stop.ring || (stop.outline !== null && contrast(stop.outline, stop.behind) >= 3);
    if (!visible) misses.push(`${stop.name} (outline ${stop.outline ?? 'none'} on ${stop.behind})`);
  }
  return { checked, misses };
}

test.describe.configure({ timeout: 90_000, retries: 0 });

test.describe('a11y — no contrast violation in the dark themes', () => {
  for (const theme of DARK_THEMES) {
    for (const route of ROUTES) {
      test(`${theme}: ${route.name}`, async ({ page }) => {
        await readOnly(page, theme);
        await openRoute(page, theme, route);

        const nodes = await contrastViolations(page, `${theme} ${route.name}`, route.exclude);
        report(`${theme} ${route.name}`, nodes);
        expect(nodes, `${theme} ${route.name} has contrast violations`).toEqual([]);
      });
    }

    test(`${theme}: discussion with its action card open`, async ({ page }) => {
      await readOnly(page, theme);
      const anchor = await discussionWithAgentMessage(page);
      test.skip(!anchor, 'no discussion with an Agent message to anchor the card');
      if (!anchor) return;
      const action = proposedAction(anchor.id, anchor.messageId);
      await page.route(`**/api/discussions/${anchor.id}/actions`, route => route.fulfill({
        json: { success: true, data: [action], error: null },
      }));
      await page.route(`**/api/discussion-actions/${action.id}`, route => route.fulfill({
        json: { success: true, data: action, error: null },
      }));

      await page.goto(`/#discussion-${anchor.id}`);
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      const card = page.locator(`[data-testid="discussion-action-${action.id}"]`);
      await expect(card).toBeVisible({ timeout: 15_000 });
      const launch = card.locator('.discussion-action-card__launch');
      if (!(await launch.isVisible())) await card.locator('.discussion-action-card__toggle').click();
      await expect(launch).toBeVisible();
      await card.scrollIntoViewIfNeeded();

      await expect(launch).toBeEnabled();
      const label = await launch.evaluate(el => {
        const style = getComputedStyle(el);
        return { color: style.color, background: style.backgroundColor };
      });
      const ratio = contrast(label.color, label.background);
      // eslint-disable-next-line no-console
      console.log(`[contrast] ${theme} launch button: ${label.color} on ${label.background} = ${ratio.toFixed(2)}`);
      expect(ratio, `${theme} launch label`).toBeGreaterThanOrEqual(4.5);

      const cardNodes = await contrastViolations(page, `${theme} action card`, [], `[data-testid="discussion-action-${action.id}"]`);
      const pageNodes = await contrastViolations(page, `${theme} open discussion`);
      report(`${theme} action card`, cardNodes);
      report(`${theme} open discussion`, pageNodes);
      expect(cardNodes, `${theme} action card has contrast violations`).toEqual([]);
      expect(pageNodes, `${theme} open discussion has contrast violations`).toEqual([]);
    });

    test(`${theme}: keyboard focus stays visible`, async ({ page }) => {
      test.setTimeout(180_000);
      await readOnly(page, theme);
      const misses: string[] = [];
      let checked = 0;
      for (const route of ROUTES) {
        await openRoute(page, theme, route);
        const result = await focusStops(page, 15);
        checked += result.checked;
        misses.push(...result.misses.map(m => `${route.name}: ${m}`));
      }
      // eslint-disable-next-line no-console
      console.log(`[focus] ${theme}: ${checked} stop(s) checked, ${misses.length} invisible\n  ${misses.join('\n  ')}`);
      expect(checked, `${theme}: no focus stop reached`).toBeGreaterThan(ROUTES.length);
      expect(misses, `${theme} focus indicators`).toEqual([]);
    });
  }
});
