// KT-1878 — the design scales: tokens.css declares them for every theme, the
// typed table the Settings "Design system" section renders lists exactly them,
// the text scale is in rem with a 12px floor, the new semantic colours read at
// WCAG AA, and one global `*:focus-visible` rule remains (the tokens.css one).
import { describe, it, expect } from 'vitest';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  appSources, composite, contrastRatio, cssRules, declarations, loadThemes, resolveValue, themeColor,
} from '../themeAudit';
import { BREAKPOINTS, DESIGN_TOKEN_GROUPS, DESIGN_TOKENS, OVERLAY_WIDTHS, remToPx } from '../designTokens';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const THEMES = loadThemes(SRC);
const BASE = THEMES.get('dark')!;
const THEME_NAMES = ['dark', 'arcade', 'light', 'matrix', 'gotham', 'sakura', 'euronews'];
const CSS = appSources(SRC).filter(s => s.file.endsWith('.css'));

const groupOf = (scale: string) => DESIGN_TOKEN_GROUPS.find(g => g.scale === scale)!;
const names = (scale: string) => groupOf(scale).tokens.map(t => t.name);
const resolved = (theme: string, name: string) => resolveValue(THEMES.get(theme)!.get(name)!, THEMES.get(theme)!);

describe('design scales (KT-1878)', () => {
  it('lists every scale the page shows, in order', () => {
    expect(DESIGN_TOKEN_GROUPS.map(g => g.scale)).toEqual([
      'text', 'spacing', 'radius', 'breakpoint', 'duration', 'weight', 'controlHeight', 'overlayWidth', 'textRole', 'semanticColour',
    ]);
    for (const group of DESIGN_TOKEN_GROUPS) expect(group.tokens.length, group.scale).toBeGreaterThan(0);
    expect(new Set(DESIGN_TOKENS.map(t => t.name)).size).toBe(DESIGN_TOKENS.length);
  });

  it('declares every token of the table in tokens.css, for every theme', () => {
    expect([...THEMES.keys()].sort()).toEqual([...THEME_NAMES].sort());
    for (const theme of THEME_NAMES) {
      const missing = DESIGN_TOKENS.filter(t => !THEMES.get(theme)!.has(t.name)).map(t => t.name);
      expect(missing, `${theme}: tokens of the scales missing from tokens.css`).toEqual([]);
      for (const token of DESIGN_TOKENS) {
        expect(resolved(theme, token.name), `${theme} ${token.name} resolves`).not.toMatch(/var\(|^$/);
      }
    }
  });

  it('lists in the table every scale token tokens.css declares, so the page cannot drift', () => {
    const listed = new Set(DESIGN_TOKENS.map(t => t.name));
    const unlisted = [...BASE.keys()].filter(name =>
      DESIGN_TOKEN_GROUPS.some(g => g.match.test(name)) && !listed.has(name));
    expect(unlisted, 'Add these tokens to src/styles/designTokens.ts (and a use line in each locale)').toEqual([]);
    for (const group of DESIGN_TOKEN_GROUPS) {
      for (const token of group.tokens) expect(group.match.test(token.name), `${token.name} in ${group.scale}`).toBe(true);
    }
  });

  it('writes the five text sizes in rem, from a 12px floor, strictly increasing', () => {
    const sizes = names('text');
    expect(sizes).toHaveLength(5);
    const px = sizes.map(name => {
      const value = BASE.get(name)!;
      expect(value, `${name} must be in rem so the browser text size applies`).toMatch(/^\d*\.?\d+rem$/);
      return remToPx(value)!;
    });
    expect(px[0]).toBe(12);
    expect(Math.min(...px)).toBeGreaterThanOrEqual(12);
    expect(px).toEqual([...px].sort((a, b) => a - b));
    expect(new Set(px).size).toBe(5);
  });

  it('has three radii, three breakpoints, three text roles', () => {
    expect(names('radius')).toHaveLength(3);
    expect(names('breakpoint')).toHaveLength(3);
    expect(names('textRole')).toEqual(['--kr-text-primary', '--kr-text-secondary', '--kr-text-muted']);
  });

  it('keeps the breakpoints of scripts equal to the tokens', () => {
    expect(names('breakpoint').map(n => BASE.get(n))).toEqual(
      Object.values(BREAKPOINTS).map(px => `${px}px`));
  });

  it('keeps the overlay widths of scripts equal to the tokens (KT-1881)', () => {
    expect(names('overlayWidth').map(n => BASE.get(n))).toEqual(
      Object.values(OVERLAY_WIDTHS).map(px => `${px}px`));
  });

  it('keeps spacing on a 4px grid and every scale strictly increasing', () => {
    const numeric = (scale: string) => names(scale).map(n => parseFloat(resolved('dark', n)));
    expect(numeric('spacing').every(v => v % 4 === 0)).toBe(true);
    for (const scale of ['spacing', 'radius', 'breakpoint', 'duration', 'weight', 'controlHeight', 'overlayWidth']) {
      const values = numeric(scale);
      expect(values.every(Number.isFinite), scale).toBe(true);
      expect(values, `${scale} increases`).toEqual([...values].sort((a, b) => a - b));
      expect(new Set(values).size, `${scale} has no duplicate step`).toBe(values.length);
    }
  });

  it('gives the three text roles three distinct values in every theme', () => {
    for (const theme of THEME_NAMES) {
      const values = names('textRole').map(n => resolved(theme, n));
      expect(new Set(values).size, `${theme}: ${values.join(' / ')}`).toBe(3);
    }
  });

  it('resolves every semantic colour to a colour in every theme', () => {
    for (const theme of THEME_NAMES) {
      for (const name of names('semanticColour')) {
        expect(themeColor(THEMES.get(theme)!, name), `${theme} ${name}`).not.toBeNull();
      }
    }
  });

  it('reads code and diff text at WCAG AA on the code surface of every theme', () => {
    const fg = names('semanticColour').filter(n => /^--kr-code-(?!surface)/.test(n));
    const misses: string[] = [];
    for (const theme of THEME_NAMES) {
      const tokens = THEMES.get(theme)!;
      for (const surface of ['--kr-code-surface', '--kr-bg-elevated']) {
        const bg = themeColor(tokens, surface)!;
        expect(bg.a, `${theme} ${surface} is opaque`).toBe(1);
        for (const name of fg) {
          const ratio = contrastRatio(themeColor(tokens, name)!, bg);
          if (ratio < 4.5) misses.push(`${theme} ${name} on ${surface}: ${ratio.toFixed(2)}`);
        }
        for (const kind of ['added', 'removed']) {
          const row = composite(themeColor(tokens, `--kr-diff-${kind}-bg`)!, bg);
          const ratio = contrastRatio(themeColor(tokens, `--kr-diff-${kind}-text`)!, row);
          if (ratio < 4.5) misses.push(`${theme} --kr-diff-${kind}-text on its row over ${surface}: ${ratio.toFixed(2)}`);
        }
      }
    }
    expect(misses).toEqual([]);
  });

  it('keeps the existing px text, spacing and radius tokens at their values', () => {
    const before: Record<string, string> = {
      '--kr-fs-2xs': '9px', '--kr-fs-xs': '10px', '--kr-fs-sm': '11px', '--kr-fs-base': '12px',
      '--kr-fs-md': '13px', '--kr-fs-lg': '14px', '--kr-fs-xl': '17px', '--kr-fs-2xl': '20px',
      '--kr-fs-3xl': '22px', '--kr-fs-4xl': '24px',
      '--kr-sp-1': '2px', '--kr-sp-2': '4px', '--kr-sp-3': '6px', '--kr-sp-4': '8px', '--kr-sp-5': '10px',
      '--kr-sp-6': '12px', '--kr-sp-7': '14px', '--kr-sp-8': '16px', '--kr-sp-9': '20px', '--kr-sp-10': '24px',
      '--kr-sp-12': '32px',
      '--kr-r-xs': '3px', '--kr-r-sm': '4px', '--kr-r-md': '6px', '--kr-r-lg': '8px', '--kr-r-xl': '10px',
      '--kr-r-2xl': '12px', '--kr-r-3xl': '16px', '--kr-r-full': '50%', '--kr-r-pill': '999px',
      '--kr-radius-sm': '4px', '--kr-radius-md': '6px', '--kr-radius-pill': '999px',
    };
    for (const theme of THEME_NAMES) {
      for (const [name, value] of Object.entries(before)) expect(resolved(theme, name), `${theme} ${name}`).toBe(value);
    }
  });

  it('removes no --kr- token that tokens.css declared before KT-1878', () => {
    const before = [
      'accent', 'accent-bg', 'accent-bg-hover', 'accent-bg-strong', 'accent-border', 'accent-border-light',
      'accent-fill', 'accent-fill-rgb', 'accent-ink', 'accent-rgb', 'accent-text', 'accent-tint-rgb',
      'agent-nvidia-text', 'bevel-top', 'bg', 'bg-accent-soft', 'bg-accent-subtle', 'bg-active', 'bg-base',
      'bg-card', 'bg-card-subtle', 'bg-code', 'bg-code-panel', 'bg-elevated', 'bg-hover', 'bg-hover-strong',
      'bg-input', 'bg-overlay', 'bg-raised', 'bg-scrim', 'bg-selected', 'bg-subtle', 'bg-surface', 'border',
      'border-emphasis', 'border-faint', 'border-ghost', 'border-heavy', 'border-light', 'border-medium',
      'border-soft', 'border-strong', 'border-subtle', 'cancelled', 'code-bg', 'cyan', 'cyan-rgb', 'danger',
      'danger-rgb', 'error', 'error-rgb', 'error-soft', 'font-mono', 'font-sans', 'fs-2xl', 'fs-2xs', 'fs-3xl',
      'fs-4xl', 'fs-base', 'fs-lg', 'fs-md', 'fs-sm', 'fs-xl', 'fs-xs', 'glow-electric-md', 'glow-electric-sm',
      'glow-lila-sm', 'glow-lime-lg', 'glow-lime-md', 'glow-lime-sm', 'glow-magenta-md', 'glow-magenta-sm',
      'info', 'info-bg', 'info-rgb', 'on-cyan', 'on-error', 'on-success', 'on-warning', 'panel-w', 'pink',
      'pink-rgb', 'purple', 'purple-rgb', 'purple-soft', 'purple-soft-rgb', 'r-2xl', 'r-3xl', 'r-full', 'r-lg',
      'r-md', 'r-pill', 'r-sm', 'r-xl', 'r-xs', 'radius-md', 'radius-pill', 'radius-sm', 'shadow-lg',
      'shadow-md', 'shadow-modal', 'shadow-popover', 'shadow-sm', 'shadow-xl', 'sp-1', 'sp-10', 'sp-12', 'sp-2',
      'sp-3', 'sp-4', 'sp-5', 'sp-6', 'sp-7', 'sp-8', 'sp-9', 'success', 'success-bg', 'success-rgb', 'text-dim',
      'text-faint', 'text-faint-rgb', 'text-ghost', 'text-ghost-rgb', 'text-glow-lime', 'text-glow-magenta',
      'text-muted', 'text-on-accent', 'text-on-dark', 'text-primary', 'text-rgb', 'text-secondary',
      'text-tertiary', 'warning', 'warning-amber', 'warning-amber-rgb', 'warning-bg', 'warning-rgb',
      'warning-soft', 'z-drawer', 'z-modal', 'z-modal-nested', 'z-popover', 'z-sticky', 'z-toast', 'z-tour',
      'z-tour-tooltip',
    ].map(n => `--kr-${n}`);
    expect(before).toHaveLength(146);
    expect(before.filter(n => !BASE.has(n))).toEqual([]);
  });
});

describe('global focus ring (KT-1878)', () => {
  const isGlobal = (selector: string) => selector.split(',').map(s => s.trim())
    .some(s => s === '*:focus-visible' || s === ':focus-visible');
  const globalRules = CSS.flatMap(s => cssRules(s.text).filter(r => isGlobal(r.selector))
    .map(r => ({ file: s.file, ...r })));

  it('keeps one global *:focus-visible rule, the tokens.css one', () => {
    expect(globalRules.map(r => `${r.file}:${r.line}`)).toHaveLength(1);
    const [rule] = globalRules;
    expect(rule.file).toBe(join('styles', 'tokens.css'));
    const decls = new Map(declarations(rule.body));
    expect(decls.get('outline')).toBe('2px solid var(--kr-accent-ink)');
    expect(decls.get('box-shadow')).toContain('var(--kr-accent-rgb)');
  });

  it('counts a second global rule, so the check above is not vacuous', () => {
    const css = '*:focus-visible { outline: 2px solid red; }\n.a:focus-visible { outline: 0; }\n'
      + 'a, :focus-visible { outline: 1px solid blue; }';
    expect(cssRules(css).filter(r => isGlobal(r.selector))).toHaveLength(2);
  });
});

describe('wrong fallbacks fixed by KT-1878', () => {
  it('reads the injected-context toggle size from --kr-fs-xs without a contradicting fallback', () => {
    const source = CSS.find(s => s.file === join('pages', 'DiscussionsPage.css'))!;
    const rule = cssRules(source.text).find(r => r.selector === '.disc-injected-context-toggle');
    expect(rule).toBeTruthy();
    expect(new Map(declarations(rule!.body)).get('font-size')).toBe('var(--kr-fs-xs)');
  });
});
