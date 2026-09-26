// Static theme checks shared by the vitest gates. They read the real CSS, so a
// theme or a rule added later is held to the same contract.
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

export type Rgba = { r: number; g: number; b: number; a: number };
export type ThemeTokens = Map<string, string>;
export type Source = { file: string; text: string };
export type Finding = { file: string; line: number; detail: string };

/** Themes whose surfaces are dark. */
export const DARK_THEMES: ReadonlySet<string> = new Set(['dark', 'gotham', 'matrix']);

/** Opaque surface ramp the text roles sit on. */
export const SURFACE_TOKENS = [
  '--kr-bg-base', '--kr-bg-raised', '--kr-bg-surface', '--kr-bg-elevated', '--kr-bg-input',
] as const;

export const TEXT_TOKENS = [
  '--kr-text-primary', '--kr-text-secondary', '--kr-text-tertiary', '--kr-text-muted',
  '--kr-text-faint', '--kr-text-dim', '--kr-text-ghost',
] as const;

const LOW_EMPHASIS = new Set(['--kr-text-faint', '--kr-text-dim', '--kr-text-ghost']);

/** Status colours that dark themes print as small text (badges, chips, states). */
export const STATUS_TOKENS = [
  '--kr-accent', '--kr-accent-text', '--kr-success', '--kr-error', '--kr-error-soft',
  '--kr-warning', '--kr-warning-soft', '--kr-warning-amber', '--kr-info', '--kr-cyan',
  '--kr-purple', '--kr-purple-soft', '--kr-pink', '--kr-cancelled', '--kr-agent-nvidia-text',
] as const;

/** The global `:focus-visible` ring colour: non-text, so 3:1 (WCAG 1.4.11). */
export const FOCUS_RING_TOKEN = '--kr-accent-ink';

/** A chip resting on `--kr-bg-hover` over an elevated panel: the lightest
 *  resting background of a dark theme. */
export const HOVER_ON_ELEVATED = '--kr-bg-hover@--kr-bg-elevated';

/** WCAG AA floor of a text role. Dark themes use every role for small text, so
 *  every role needs 4.5:1 there; elsewhere faint/dim/ghost may stay at 3:1
 *  (large text and non-text only). */
export function textFloor(theme: string, token: string): number {
  return !DARK_THEMES.has(theme) && LOW_EMPHASIS.has(token) ? 3 : 4.5;
}

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

export type ContrastRow = {
  theme: string; text: string; background: string; ratio: number; floor: number;
};

function backgroundsOf(theme: string, tokens: ThemeTokens, withHover: boolean): Array<[string, Rgba]> {
  const out: Array<[string, Rgba]> = [];
  for (const name of SURFACE_TOKENS) {
    const bg = themeColor(tokens, name);
    if (bg && bg.a >= 1) out.push([name, bg]);
  }
  const hover = themeColor(tokens, '--kr-bg-hover');
  const elevated = themeColor(tokens, '--kr-bg-elevated');
  if (withHover && DARK_THEMES.has(theme) && hover && elevated) {
    out.push([HOVER_ON_ELEVATED, composite(hover, elevated)]);
  }
  return out;
}

/** Text roles on the opaque ramp (plus the hover chip in dark themes) of every
 *  theme, and status colours on the ramp of every dark theme. */
export function measureTextContrast(themes: Map<string, ThemeTokens>): ContrastRow[] {
  const rows: ContrastRow[] = [];
  for (const [theme, tokens] of themes) {
    const pairs: Array<[string, number, boolean]> = TEXT_TOKENS.map(t => [t, textFloor(theme, t), true]);
    if (DARK_THEMES.has(theme)) pairs.push(...STATUS_TOKENS.map(t => [t, 4.5, false] as [string, number, boolean]));
    pairs.push([FOCUS_RING_TOKEN, 3, false]);
    for (const [text, floor, withHover] of pairs) {
      const fg = themeColor(tokens, text);
      if (!fg) continue;
      for (const [background, bg] of backgroundsOf(theme, tokens, withHover)) {
        rows.push({ theme, text, background, ratio: contrastRatio(fg, bg), floor });
      }
    }
  }
  return rows;
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

const LITERAL_WHITE_OR_BLACK =
  /^(white|black|#fff|#ffffff|#000|#000000|rgb\(\s*(255\s*,\s*255\s*,\s*255|0\s*,\s*0\s*,\s*0)\s*\))(\s*!important)?$/i;

/** A rule painted from a `--kr-*` token whose text is pinned to literal white
 *  or black: right in the theme it was written for, unreadable in another. */
export function forcedTextOnTokenBackground(file: string, css: string): Finding[] {
  const findings: Finding[] = [];
  for (const rule of cssRules(css)) {
    const decls = declarations(rule.body);
    const background = decls.find(([p, v]) => /^background(-color)?$/.test(p) && /var\(\s*--kr-/.test(v));
    const color = decls.find(([p, v]) => p === 'color' && LITERAL_WHITE_OR_BLACK.test(v));
    if (background && color) {
      findings.push({
        file, line: rule.line,
        detail: `${rule.selector} { background: ${background[1]}; color: ${color[1]} }`,
      });
    }
  }
  return findings;
}

/** The same pairing in a React inline `style={{ … }}` object, which no
 *  stylesheet check sees. */
export function forcedTextInInlineStyles(file: string, code: string): Finding[] {
  const findings: Finding[] = [];
  for (const m of code.matchAll(/style=\{\{/g)) {
    const start = (m.index ?? 0) + m[0].length;
    let depth = 1;
    let end = start;
    while (end < code.length && depth > 0) {
      if (code[end] === '{') depth += 1;
      else if (code[end] === '}') depth -= 1;
      end += 1;
    }
    const body = code.slice(start, end - 1);
    const value = (prop: string) =>
      body.match(new RegExp(`(?:^|[\\s,{])${prop}\\s*:\\s*(['"\`])([^'"\`]*)\\1`))?.[2].trim();
    const background = value('background') ?? value('backgroundColor');
    const color = value('color');
    if (background && /var\(\s*--kr-/.test(background) && color && LITERAL_WHITE_OR_BLACK.test(color)) {
      findings.push({
        file, line: lineOf(code, m.index ?? 0),
        detail: `style={{ background: ${background}, color: ${color} }}`,
      });
    }
  }
  return findings;
}

/** A `:focus-visible` rule that drops the outline without drawing a ring of
 *  its own (box-shadow): keyboard focus becomes a faint background change. */
export function focusRingRemovals(file: string, css: string): Finding[] {
  const findings: Finding[] = [];
  for (const rule of cssRules(css)) {
    if (!rule.selector.includes(':focus-visible')) continue;
    const decls = new Map(declarations(rule.body));
    const outline = decls.get('outline');
    if (outline && /^(none|0)(\s*!important)?$/.test(outline) && !decls.has('box-shadow')) {
      findings.push({ file, line: rule.line, detail: `${rule.selector} { outline: ${outline} }` });
    }
  }
  return findings;
}

/** Custom properties of any prefix read with `var()` but declared nowhere,
 *  neither in a stylesheet nor as an inline style property. */
export function undeclaredCustomProperties(sources: Source[]): Finding[] {
  const declared = new Set<string>();
  for (const source of sources) {
    const code = codeOf(source);
    for (const m of code.matchAll(/(--[a-z0-9-]+)['"]?\s*(?:as string\]\s*)?:/gi)) declared.add(m[1]);
    for (const m of code.matchAll(/setProperty\(\s*['"`](--[a-z0-9-]+)/gi)) declared.add(m[1]);
  }
  const findings: Finding[] = [];
  for (const source of sources) {
    const code = codeOf(source);
    for (const m of code.matchAll(/var\(\s*(--[a-z0-9-]+)/gi)) {
      if (!declared.has(m[1])) findings.push({ file: source.file, line: lineOf(code, m.index ?? 0), detail: m[1] });
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

export type RepoAudit = {
  themes: Map<string, ThemeTokens>;
  contrast: ContrastRow[];
  undefinedTokens: Finding[];
  undeclaredProperties: Finding[];
  forcedText: Finding[];
  focusRemovals: Finding[];
};

/** Every static check against `frontend/src`, as the CLI and the tests run it. */
export function auditRepo(srcDir: string): RepoAudit {
  const themes = loadThemes(srcDir);
  const sources = appSources(srcDir);
  const css = sources.filter(s => s.file.endsWith('.css'));
  return {
    themes,
    contrast: measureTextContrast(themes),
    undefinedTokens: undefinedTokenUsages(sources, new Set(themes.get('dark')?.keys() ?? [])),
    undeclaredProperties: undeclaredCustomProperties(sources),
    forcedText: [
      ...css.flatMap(s => forcedTextOnTokenBackground(s.file, s.text)),
      ...sources.filter(s => s.file.endsWith('.tsx')).flatMap(s => forcedTextInInlineStyles(s.file, s.text)),
    ],
    focusRemovals: css.flatMap(s => focusRingRemovals(s.file, s.text)),
  };
}
