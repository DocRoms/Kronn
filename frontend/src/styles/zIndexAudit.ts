// Static audit of the stacking layers (KT-1701). tokens.css owns ONE z-index
// scale; a raw number above `STICKY_MAX` anywhere else makes two overlays
// compete on different scales — the agent picker's popover (120) once opened
// under the modal overlay (1000). Shared by the lint test and by the component
// tests that assert a popover sits above the dialog it opens from (CSS is not
// loaded under vitest, so those read the stylesheets from disk).
import {
  appSources, cssRules, declarations, loadThemes, stripComments,
  type Finding, type Source,
} from './themeAudit';

/** The highest raw z-index a stylesheet may carry: local stacking inside a component. */
export const STICKY_MAX = 50;

/** Layer tokens, lowest to highest. */
export const LAYER_ORDER = [
  '--kr-z-sticky', '--kr-z-drawer', '--kr-z-modal', '--kr-z-modal-nested',
  '--kr-z-popover', '--kr-z-toast', '--kr-z-tour', '--kr-z-tour-tooltip',
] as const;

type Tokens = ReadonlyMap<string, string>;

/** A layer token, a `calc()` offset of one, or a plain integer, as a number; null when not static. */
export function evalZIndex(raw: string, tokens: Tokens, depth = 0): number | null {
  if (depth > 8) return null;
  let expr = raw.replace(/!important/i, '').trim();
  for (let i = 0; i < 8 && /var\(/.test(expr); i++) {
    expr = expr.replace(/var\(\s*(--[\w-]+)\s*\)/g, (whole, name: string) => tokens.get(name) ?? whole);
  }
  expr = expr.replace(/^calc\((.*)\)$/, '$1').trim();
  if (!/^-?\d+(\s*[+-]\s*\d+)*$/.test(expr)) return null;
  return [...expr.matchAll(/([+-]?)\s*(\d+)/g)]
    .reduce((sum, [, sign, n]) => sum + (sign === '-' ? -1 : 1) * Number(n), 0);
}

/** Every raw number above `STICKY_MAX` used as a z-index in CSS, inline TSX styles or style strings. */
export function hardCodedZIndex(sources: Source[]): Finding[] {
  const out: Finding[] = [];
  for (const { file, text } of sources) {
    const code = file.endsWith('.css') ? stripComments(text) : text;
    const re = /\bz-?[iI]ndex\b['"]?\s*:\s*([^;,}\n]+)/g;
    let m: RegExpExecArray | null;
    while ((m = re.exec(code)) !== null) {
      const value = m[1].trim().replace(/^['"`]|['"`]$/g, '');
      const offsets = /var\(--kr-z-sticky\)\s*\+\s*(\d+)/.exec(value);
      // Without a token in it, every number is a raw layer; sticky + n must stay below the drawer.
      const raw = /var\(/.test(value) ? [] : [...value.matchAll(/\d+/g)].map(n => Number(n[0]));
      const bad = raw.some(n => n > STICKY_MAX) || (offsets !== null && Number(offsets[1]) >= STICKY_MAX);
      if (bad) {
        out.push({ file, line: code.slice(0, m.index).split('\n').length, detail: `z-index: ${value}` });
      }
    }
  }
  return out;
}

export function loadBaseTokens(srcDir: string): Tokens {
  return loadThemes(srcDir).get('dark') ?? new Map();
}

/** The z-index a class gets from the app stylesheets (last declaration wins), or null. */
export function classZIndex(className: string, srcDir: string): number | null {
  const tokens = loadBaseTokens(srcDir);
  let found: number | null = null;
  for (const source of appSources(srcDir).filter(s => s.file.endsWith('.css'))) {
    for (const rule of cssRules(source.text)) {
      if (!rule.selector.split(',').some(s => s.trim() === `.${className}`)) continue;
      for (const [prop, value] of declarations(rule.body)) {
        if (prop === 'z-index') found = evalZIndex(value, tokens) ?? found;
      }
    }
  }
  return found;
}
