// Static theme checks shared by the vitest gates. They read the real CSS, so a
// theme or a rule added later is held to the same contract.
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

export type Rgba = { r: number; g: number; b: number; a: number };
export type ThemeTokens = Map<string, string>;
export type Source = { file: string; text: string };
export type Finding = { file: string; line: number; detail: string };

export function stripComments(css: string): string {
  // Keep line breaks so reported line numbers stay true.
  return css.replace(/\/\*[\s\S]*?\*\//g, m => m.replace(/[^\n]/g, ' '));
}

export type CssRule = { selector: string; body: string; line: number };

/** Innermost `selector { declarations }` blocks; at-rule wrappers are skipped. */
export function cssRules(css: string): CssRule[] {
  const text = stripComments(css);
  const rules: CssRule[] = [];
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    const selector = m[1].trim().replace(/\s+/g, ' ');
    const line = text.slice(0, m.index + m[0].indexOf('{')).split('\n').length;
    rules.push({ selector, body: m[2], line });
  }
  return rules;
}

export function declarations(body: string): Array<[string, string]> {
  return body.split(';')
    .map(d => d.trim())
    .filter(d => d.includes(':'))
    .map(d => {
      const i = d.indexOf(':');
      return [d.slice(0, i).trim().toLowerCase(), d.slice(i + 1).trim()] as [string, string];
    });
}

/** Per-theme token maps. `dark` is the bare `:root`; every other theme is
 *  `:root` overridden by its `[data-theme]` blocks, as the cascade does. */
export function parseThemes(tokensCss: string): Map<string, ThemeTokens> {
  const bySelector = new Map<string, ThemeTokens>();
  for (const rule of cssRules(tokensCss)) {
    for (const raw of rule.selector.split(',')) {
      const selector = raw.trim();
      const theme = selector === ':root'
        ? 'dark'
        : /^:root\[data-theme="([a-z0-9-]+)"\]$/.exec(selector)?.[1];
      if (!theme) continue;
      const map = bySelector.get(theme) ?? new Map<string, string>();
      for (const [prop, value] of declarations(rule.body)) {
        if (prop.startsWith('--')) map.set(prop, value);
      }
      bySelector.set(theme, map);
    }
  }
  const base = bySelector.get('dark') ?? new Map<string, string>();
  const themes = new Map<string, ThemeTokens>();
  for (const [theme, overrides] of bySelector) {
    themes.set(theme, theme === 'dark' ? new Map(base) : new Map([...base, ...overrides]));
  }
  return themes;
}

/** Substitutes `var(--x[, fallback])` against one theme, recursively. */
export function resolveValue(value: string, tokens: ThemeTokens, depth = 0): string {
  if (depth > 16) return value;
  const next = value.replace(/var\(\s*(--[a-z0-9-]+)\s*(?:,\s*([^()]*(?:\([^()]*\))?[^()]*))?\)/gi,
    (_all, name: string, fallback?: string) => tokens.get(name) ?? fallback?.trim() ?? '');
  return next === value ? value : resolveValue(next, tokens, depth + 1);
}

export function parseColor(raw: string): Rgba | null {
  const value = raw.trim().toLowerCase();
  if (value === 'white') return { r: 255, g: 255, b: 255, a: 1 };
  if (value === 'black') return { r: 0, g: 0, b: 0, a: 1 };
  const hex = /^#([0-9a-f]{3,8})$/.exec(value)?.[1];
  if (hex) {
    const full = hex.length <= 4 ? hex.replace(/./g, c => c + c) : hex;
    if (full.length !== 6 && full.length !== 8) return null;
    const n = (i: number) => parseInt(full.slice(i, i + 2), 16);
    return { r: n(0), g: n(2), b: n(4), a: full.length === 8 ? n(6) / 255 : 1 };
  }
  const fn = /^rgba?\(([^)]*)\)$/.exec(value)?.[1];
  if (!fn) return null;
  const parts = fn.split(/[\s,/]+/).filter(Boolean).map(Number);
  if (parts.length < 3 || parts.some(Number.isNaN)) return null;
  return { r: parts[0], g: parts[1], b: parts[2], a: parts[3] ?? 1 };
}

/** `fg` painted over an opaque `bg`. */
export function composite(fg: Rgba, bg: Rgba): Rgba {
  const mix = (f: number, b: number) => f * fg.a + b * (1 - fg.a);
  return { r: mix(fg.r, bg.r), g: mix(fg.g, bg.g), b: mix(fg.b, bg.b), a: 1 };
}

function luminance({ r, g, b }: Rgba): number {
  // Same channel threshold as axe-core, so the numbers compare with its report.
  const ch = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b);
}

export function contrastRatio(fg: Rgba, bg: Rgba): number {
  const top = fg.a < 1 ? composite(fg, bg) : fg;
  const [hi, lo] = [luminance(top), luminance(bg)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

export function themeColor(tokens: ThemeTokens, name: string): Rgba | null {
  const value = tokens.get(name);
  return value ? parseColor(resolveValue(value, tokens)) : null;
}

function lineOf(text: string, index: number): number {
  return text.slice(0, index).split('\n').length;
}

function codeOf({ file, text }: Source): string {
  return file.endsWith('.css') ? stripComments(text) : text;
}

/** `var(--kr-…)` whose name the base `:root` of tokens.css never declares.
 *  A fallback does not excuse it: a fallback is what hid `--kr-on-accent`. */
export function undefinedTokenUsages(sources: Source[], baseTokens: ReadonlySet<string>): Finding[] {
  const findings: Finding[] = [];
  for (const source of sources) {
    const code = codeOf(source);
    for (const m of code.matchAll(/var\(\s*(--kr-[a-z0-9-]+)/gi)) {
      if (!baseTokens.has(m[1])) {
        findings.push({ file: source.file, line: lineOf(code, m.index ?? 0), detail: m[1] });
      }
    }
  }
  return findings;
}

const LITERAL_WHITE = /^(white|#fff|#ffffff|rgb\(\s*255\s*,\s*255\s*,\s*255\s*\))(\s*!important)?$/i;

/** A rule filled with an accent token whose text is pinned to literal white:
 *  right on a dark accent, unreadable on the lime or yellow ones. */
export function forcedTextOnTokenBackground(file: string, css: string): Finding[] {
  const findings: Finding[] = [];
  for (const rule of cssRules(css)) {
    const decls = declarations(rule.body);
    const background = decls.find(([p, v]) => /^background(-color)?$/.test(p) && /var\(\s*--kr-accent/.test(v));
    const color = decls.find(([p, v]) => p === 'color' && LITERAL_WHITE.test(v));
    if (background && color) {
      findings.push({
        file, line: rule.line,
        detail: `${rule.selector} { background: ${background[1]}; color: ${color[1]} }`,
      });
    }
  }
  return findings;
}

export function walkFiles(dir: string, exts: string[]): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules' || name === 'dist') continue;
    const path = join(dir, name);
    if (statSync(path).isDirectory()) out.push(...walkFiles(path, exts));
    else if (exts.some(e => name.endsWith(e))) out.push(path);
  }
  return out;
}

/** App sources under `srcDir`; tests and this module are fixtures, not app code. */
export function appSources(srcDir: string): Source[] {
  return walkFiles(srcDir, ['.css', '.ts', '.tsx'])
    .filter(f => !/__tests__|\.test\.tsx?$|[\\/]src[\\/]test[\\/]|themeAudit\.ts$/.test(f))
    .map(f => ({ file: relative(srcDir, f), text: readFileSync(f, 'utf8') }));
}

export function loadThemes(srcDir: string): Map<string, ThemeTokens> {
  return parseThemes(readFileSync(join(srcDir, 'styles', 'tokens.css'), 'utf8'));
}
