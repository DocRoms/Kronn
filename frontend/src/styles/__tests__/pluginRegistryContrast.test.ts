// KT-2006 — the « add a plugin » form: the category pills and the registry
// cards read at WCAG AA (4.5:1) in every theme. axe found 12 nodes under the
// floor in light: the pill counts (halved by an opacity), the « official » and
// « community » badges (`--kr-success` / `--kr-warning` as text, 4.3:1 on a
// card), and the cards' metadata. The origin badge is shared with the Plugins
// overview, so it is measured on every opaque surface as well.
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  composite, contrastRatio, cssRules, declarations, loadThemes, parseColor, resolveValue,
  SURFACE_TOKENS, themeColor, type Rgba, type ThemeTokens,
} from '../themeAudit';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const THEMES = loadThemes(SRC);
const THEME_NAMES = ['dark', 'arcade', 'light', 'matrix', 'gotham', 'sakura', 'euronews'];
const RULES = [
  ...cssRules(readFileSync(join(SRC, 'pages', 'McpPage.css'), 'utf8')),
  ...cssRules(readFileSync(join(SRC, 'styles', 'components.css'), 'utf8')),
];

/** The last value of `prop` in the rules written exactly as `selector`. */
function declared(selector: string, prop: string): string | null {
  let value: string | null = null;
  for (const rule of RULES) {
    if (!rule.selector.split(',').map(s => s.trim()).includes(selector)) continue;
    for (const [name, v] of declarations(rule.body)) if (name === prop) value = v;
  }
  return value;
}

/** The colour token a rule writes its text in. */
function textToken(selector: string): string {
  const value = declared(selector, 'color');
  const token = value && /^var\((--kr-[a-z0-9-]+)\)$/.exec(value)?.[1];
  if (!token) throw new Error(`${selector} writes its text in ${value ?? 'nothing'}, not in one --kr-* token`);
  return token;
}

const surface = (tokens: ThemeTokens, name: string): Rgba => themeColor(tokens, name)!;
/** A registry card: `--kr-bg-hover` over the panel it sits on. */
const card = (tokens: ThemeTokens, under: string) => composite(surface(tokens, '--kr-bg-hover'), surface(tokens, under));
/** The active pill: a 12 % accent tint over the panel. */
const activePill = (tokens: ThemeTokens) =>
  composite(parseColor(`rgba(${resolveValue('var(--kr-accent-rgb)', tokens)}, 0.12)`)!, surface(tokens, '--kr-bg-surface'));

function misses(selector: string, backgrounds: (tokens: ThemeTokens) => Array<[string, Rgba]>): string[] {
  const token = textToken(selector);
  const out: string[] = [];
  for (const theme of THEME_NAMES) {
    const tokens = THEMES.get(theme)!;
    const fg = themeColor(tokens, token);
    if (!fg) { out.push(`${theme}: ${token} does not resolve`); continue; }
    for (const [name, bg] of backgrounds(tokens)) {
      const ratio = contrastRatio(composite(fg, bg), bg);
      if (ratio < 4.5) out.push(`${theme} ${selector} (${token}) on ${name}: ${ratio.toFixed(2)}`);
    }
  }
  return out;
}

const panel = (tokens: ThemeTokens): Array<[string, Rgba]> => [['--kr-bg-surface', surface(tokens, '--kr-bg-surface')]];
const cardSurfaces = (tokens: ThemeTokens): Array<[string, Rgba]> => [
  ['card on --kr-bg-surface', card(tokens, '--kr-bg-surface')],
  // An installed card rests on the panel itself.
  ['--kr-bg-surface', surface(tokens, '--kr-bg-surface')],
];
const everySurface = (tokens: ThemeTokens): Array<[string, Rgba]> => [
  ...SURFACE_TOKENS.map(name => [name, surface(tokens, name)] as [string, Rgba]),
  ['card on --kr-bg-surface', card(tokens, '--kr-bg-surface')],
  ['card on --kr-bg-elevated', card(tokens, '--kr-bg-elevated')],
];

describe('add-plugin form contrast (KT-2006)', () => {
  it('reads the category pills, and their counts, at 4.5:1 on the panel and on the active tint', () => {
    expect(misses('.mcp-cat-pill', panel)).toEqual([]);
    expect(misses('.mcp-cat-pill-active', t => [['active tint', activePill(t)]])).toEqual([]);
    // The count keeps its pill's colour: no colour, no opacity of its own.
    expect(declared('.mcp-cat-pill-count', 'color')).toBeNull();
    expect(declared('.mcp-cat-pill-count', 'opacity')).toBeNull();
  });

  it('reads the card metadata at 4.5:1 on a card and on the bare panel', () => {
    for (const selector of [
      '.mcp-registry-card-cat', '.mcp-registry-card-desc', '.mcp-registry-card-meta',
      '.mcp-kind-badge-mcp', '.mcp-kind-badge-api', '.mcp-credential-kind',
    ]) expect(misses(selector, cardSurfaces)).toEqual([]);
  });

  it('reads the origin badge at 4.5:1 on every surface, the origin hue left to its dot and border', () => {
    expect(misses('.mcp-origin-badge', everySurface)).toEqual([]);
    for (const origin of ['.mcp-origin-official', '.mcp-origin-community']) {
      expect(declared(origin, 'color'), `${origin} must not colour the label`).toBeNull();
      expect(declared(`${origin}::before`, 'background')).toMatch(/^var\(--kr-(success|warning)\)$/);
    }
  });

  it('marks an installed card without fading its text', () => {
    expect(declared('.mcp-registry-card-installed', 'opacity')).toBeNull();
    expect(declared('.mcp-registry-card-installed:hover', 'opacity')).toBeNull();
  });
});
