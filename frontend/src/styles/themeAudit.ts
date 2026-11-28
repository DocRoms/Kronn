// Static theme checks shared by the vitest gates. They read the real CSS, so a
// theme or a rule added later is held to the same contract.
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

export type Rgba = { r: number; g: number; b: number; a: number };
export type ThemeTokens = Map<string, string>;
export type Source = { file: string; text: string };
export type Finding = { file: string; line: number; detail: string };

/** Themes whose surfaces are dark. */
export const DARK_THEMES: ReadonlySet<string> = new Set(['dark', 'arcade', 'gotham', 'matrix']);

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

/** KT-1887 — the semantic status colours (`StatusBadge`, notices), measured
 *  with the status colours above in every theme, light ones included. */
export const SEMANTIC_STATUS_TOKENS = [
  '--kr-status-success', '--kr-status-warning', '--kr-status-danger', '--kr-status-info', '--kr-status-neutral',
] as const;

/** KT-1936 — the agents' names written as text (`agentTextColor`), measured
 *  at 4.5:1 on the ramp of every theme with no baseline. */
export const AGENT_TEXT_TOKENS = [
  '--kr-agent-claude-text', '--kr-agent-codex-text', '--kr-agent-opencode-text', '--kr-agent-vibe-text',
  '--kr-agent-gemini-text', '--kr-agent-kiro-text', '--kr-agent-copilot-text', '--kr-agent-ollama-text',
  '--kr-agent-litellm-text', '--kr-agent-fallback-text',
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

/** A nested selector resolved against its parent rule's (`&` or a
 *  descendant), every combination of their comma lists. */
function nestSelector(parent: string, child: string): string {
  const parents = parent.split(',').map(s => s.trim());
  return child.split(',').map(s => s.trim()).flatMap(c =>
    parents.map(p => (c.includes('&') ? c.replace(/&/g, p) : `${p} ${c}`))).join(', ');
}

/** `selector { declarations }` blocks; an at-rule wrapper with nothing but
 *  rules inside is skipped. KT-2016 — a rule that holds nested rules
 *  (CSS nesting) is read too: its body is its own declarations, each nested
 *  rule blanked out (line breaks kept, so a line counted in the body stays
 *  true), and the nested rules come out on their own with their selector
 *  resolved against the parent's (`&`, or a descendant). */
export function cssRules(css: string): CssRule[] {
  const text = stripComments(css);
  const rules: Array<CssRule & { open: number }> = [];
  type Frame = { prelude: number; open: number; line: number; selector: string; nested: Array<[number, number]> };
  const stack: Frame[] = [];
  let segment = 0;
  let line = 1;
  const blank = (s: string) => s.replace(/[^\n]/g, ' ');
  for (let i = 0; i < text.length; i += 1) {
    const c = text[i];
    if (c === '\n') line += 1;
    else if (c === ';') segment = i + 1;
    else if (c === '{') {
      const raw = text.slice(segment, i).trim().replace(/\s+/g, ' ');
      const parent = [...stack].reverse().find(f => !f.selector.startsWith('@'));
      // A conditional at-rule nested in a rule applies to the rule's own
      // selector: `.a { @media (…) { color: x } }` styles `.a`.
      const selector = !parent ? raw
        : !raw.startsWith('@') ? nestSelector(parent.selector, raw)
          : /^@(?:media|supports|container|layer)\b/.test(raw) ? parent.selector : raw;
      stack.push({ prelude: segment, open: i, line, selector, nested: [] });
      segment = i + 1;
    } else if (c === '}') {
      const frame = stack.pop();
      segment = i + 1;
      if (!frame) continue;
      let body = '';
      let from = frame.open + 1;
      for (const [start, end] of frame.nested) {
        body += text.slice(from, start) + blank(text.slice(start, end));
        from = end;
      }
      body += text.slice(from, i);
      if (frame.nested.length === 0 || body.trim() !== '') {
        rules.push({ selector: frame.selector, body, line: frame.line, open: frame.open });
      }
      stack.at(-1)?.nested.push([frame.prelude, i + 1]);
    }
  }
  // In the order the blocks open, as they are written.
  return rules.sort((a, b) => a.open - b.open).map(({ selector, body, line: at }) => ({ selector, body, line: at }));
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

/** The key of a contrast cell in the baseline: `theme text on background`. */
export const contrastKey = (r: Pick<ContrastRow, 'theme' | 'text' | 'background'>) =>
  `${r.theme} ${r.text} on ${r.background}`;

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
 *  theme, and status colours (4.5:1: they print as small text) on the ramp of
 *  every theme, light ones included (KT-1887). */
export function measureTextContrast(themes: Map<string, ThemeTokens>): ContrastRow[] {
  const rows: ContrastRow[] = [];
  for (const [theme, tokens] of themes) {
    const pairs: Array<[string, number, boolean]> = TEXT_TOKENS.map(t => [t, textFloor(theme, t), true]);
    pairs.push(...[...STATUS_TOKENS, ...SEMANTIC_STATUS_TOKENS, ...AGENT_TEXT_TOKENS].map(t => [t, 4.5, false] as [string, number, boolean]));
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

// ── KT-1887 — values off the design scales ────────────────────────────────
// The stylesheet half of the off-scale guard; the inline-style half is the
// ESLint rule `ui-guards/no-off-scale-style` (scripts/eslint-ui-guards.mjs),
// which reads the same definitions from here.

export type OffScaleKind = 'font-size' | 'radius' | 'colour' | 'breakpoint';
export const OFF_SCALE_KINDS: readonly OffScaleKind[] = ['font-size', 'radius', 'colour', 'breakpoint'];
export type OffScaleFinding = Finding & { kind: OffScaleKind };

/** `BREAKPOINTS` of designTokens.ts in px (the test holds them equal): a
 *  stylesheet writes `@media (max-width: <bp - 1>px)` or `(min-width: <bp>px)`. */
export const BREAKPOINT_PX: readonly number[] = [640, 768, 900];

/** A font size on the scale: a `--kr-type-*` (or legacy `--kr-fs-*`) token,
 *  or a keyword that keeps the parent's size. */
export const FONT_SIZE_OK =
  /^(?:var\(--kr-(?:type|fs)-[a-z0-9-]+\)|inherit|initial|unset|revert|1em|100%|smaller|larger)(?:\s*!important)?$/;

/** One part of a radius on the scale: 0, a circle, or a radius token. */
export const RADIUS_PART_OK = /^(?:0|0px|50%|inherit|initial|unset|var\(--kr-(?:radius|r)-[a-z0-9-]+\))$/;

/** `var(--x, fallback)` → `var(--x)`: a fallback is not what a theme paints. */
export function dropFallbacks(value: string): string {
  let out = '';
  let i = 0;
  for (let at = value.indexOf('var(', i); at >= 0; at = value.indexOf('var(', i)) {
    let depth = 0;
    let end = at + 3;
    for (; end < value.length; end += 1) {
      if (value[end] === '(') depth += 1;
      else if (value[end] === ')' && --depth === 0) break;
    }
    const name = /^var\(\s*(--[a-z0-9-]+)/i.exec(value.slice(at))?.[1];
    out += value.slice(i, at) + (name ? `var(${name})` : value.slice(at, end + 1));
    i = end + 1;
  }
  return out + value.slice(i);
}

export function radiusOk(value: string): boolean {
  return dropFallbacks(value).replace(/\s*!important$/, '').split(/\s*\/\s*|\s+/).filter(Boolean)
    .every(part => RADIUS_PART_OK.test(part));
}

/** A colour written as a value (hex, a colour function on numbers, a common
 *  name) instead of read from a token; `rgba(var(--kr-x-rgb), a)` reads one. */
export const LITERAL_COLOUR =
  /#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(\s*[\d.]|\b(?:white|black|red|green|blue|gray|grey|orange|yellow|purple|pink|silver|gold|navy|teal)\b/i;

/** A declaration value with its token reads, fallbacks and `url()` blanked:
 *  `var(--kr-purple-soft)` names no colour. */
export function withoutTokens(value: string): string {
  return dropFallbacks(value).replace(/var\(\s*--[a-z0-9-]+\s*\)/gi, 'var()').replace(/url\([^)]*\)/gi, 'url()')
    // A fully transparent black is `transparent`, written for a transition.
    .replace(/rgba?\(\s*0\s*[, ]\s*0\s*[, ]\s*0\s*[,/]\s*0\s*\)/gi, 'transparent');
}

/** A literal colour in a declaration value. */
export function hasLiteralColour(value: string): boolean {
  return LITERAL_COLOUR.test(withoutTokens(value));
}

/** Where a theme is painted: literal colours are their job. */
export const COLOUR_EXEMPT: ReadonlySet<string> = new Set([
  'styles/tokens.css', 'styles/atmosphere.css', 'styles/neon.css', 'styles/theme-effects.css',
]);

/** Hex and colour functions on numbers: a colour in any property. */
const VALUE_COLOUR = /#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(\s*[\d.]/i;

const RADIUS_PROP = /^border(?:-(?:top|bottom|start|end)-(?:left|right|start|end))?-radius$/;
// A literal colour in an animation or font name is not a colour, so only the
// properties that paint one are read for colour names; hex and colour
// functions count anywhere.
const NAMED_COLOUR_PROP =
  /^(?:color|background(?:-color)?|border(?:-(?:top|right|bottom|left))?(?:-color)?|outline(?:-color)?|fill|stroke|(?:box|text)-shadow|caret-color|accent-color|text-decoration(?:-color)?|column-rule(?:-color)?|--.*)$/;

/** One width a `@media` or `@container` query tests: where it sits in the
 *  prelude, the breakpoint it stands for (the first width of the wider
 *  layout; NaN for a width not in px) and the feature as written. */
export type QueryWidth = { index: number; bp: number; feature: string };

const LENGTH = String.raw`([\d.]+)(px|rem|em)`;
const RANGE_FEATURE = new RegExp(
  String.raw`\(\s*(?:${LENGTH}\s*([<>]=?)\s*)?(?:width|inline-size)\s*(?:([<>]=?)\s*${LENGTH})?\s*\)`, 'g');
const FLIP: Record<string, string> = { '<': '>', '<=': '>=', '>': '<', '>=': '<=' };

/** `width OP N`, as a breakpoint: `<= N` and `max-width: N` → N + 1; `< N`
 *  and `>= N` and `min-width: N` → N; `> N` → N + 1. */
function rangeBreakpoint(op: string, value: number, unit: string): number {
  if (unit !== 'px') return Number.NaN;
  return op === '<=' || op === '>' ? value + 1 : value;
}

/** KT-2016 — the widths of a query prelude: `(max-width: N)`, `(min-width:
 *  N)`, and the range syntax (`width <= N`, `N < width`, `A < width <= B`). */
export function queryWidths(prelude: string): QueryWidth[] {
  const out: QueryWidth[] = [];
  for (const w of prelude.matchAll(/\(\s*(max|min)-(?:width|inline-size)\s*:\s*([\d.]+)(px|rem|em)\s*\)/g)) {
    out.push({ index: w.index ?? 0, bp: rangeBreakpoint(w[1] === 'max' ? '<=' : '>=', Number(w[2]), w[3]), feature: w[0] });
  }
  for (const w of prelude.matchAll(RANGE_FEATURE)) {
    const [, leftValue, leftUnit, leftOp, rightOp, rightValue, rightUnit] = w;
    if (leftOp) out.push({ index: w.index ?? 0, bp: rangeBreakpoint(FLIP[leftOp], Number(leftValue), leftUnit), feature: w[0] });
    if (rightOp) out.push({ index: w.index ?? 0, bp: rangeBreakpoint(rightOp, Number(rightValue), rightUnit), feature: w[0] });
  }
  return out.sort((a, b) => a.index - b.index);
}

/** The global keywords and system fonts of a `font:` shorthand: no size. */
const FONT_WITHOUT_SIZE = /^(?:inherit|initial|unset|revert|revert-layer|caption|icon|menu|message-box|small-caption|status-bar)$/i;
const FONT_SIZE_TOKEN =
  /^(?:[\d.]+(?:px|rem|em|%|pt|pc|vw|vh|vmin|vmax|ch|ex|cap|lh|rlh|q|mm|cm|in)|(?:xx?x?-)?(?:small|large)|medium|smaller|larger|(?:calc|clamp|min|max)\(.*\)|var\(--kr-(?:type|fs)-[a-z0-9-]+\))$/i;

/** KT-2016 — the size inside a `font:` shorthand: the part before
 *  `/line-height`, or else the first length, size keyword or type token;
 *  null for a keyword or a system font, or a size it cannot tell. */
export function fontShorthandSize(value: string): string | null {
  const v = dropFallbacks(value).replace(/\s*!important$/i, '').trim();
  if (FONT_WITHOUT_SIZE.test(v)) return null;
  const parts: string[] = v.match(/(?:[^\s()/]+|\([^()]*\))+|\//g) ?? [];
  const slash = parts.indexOf('/');
  if (slash > 0) return parts[slash - 1];
  return parts.find(p => FONT_SIZE_TOKEN.test(p)) ?? null;
}

/** Font sizes, radii, colours and `@media` widths of one stylesheet that sit
 *  off the design scales (docs/conventions/design-tokens.md). KT-2016: the
 *  size in a `font:` shorthand, `@media` range syntax and `@container`
 *  widths are read too. */
export function offScaleValues(file: string, css: string): OffScaleFinding[] {
  const findings: OffScaleFinding[] = [];
  const text = stripComments(css);
  for (const query of text.matchAll(/@(?:media|container)[^{]*/g)) {
    const at = query[0].startsWith('@media') ? '@media' : '@container';
    for (const w of queryWidths(query[0])) {
      if (!BREAKPOINT_PX.includes(w.bp)) {
        findings.push({
          file, kind: 'breakpoint', line: lineOf(text, (query.index ?? 0) + w.index),
          detail: `${at} ${w.feature.replace(/\s+/g, ' ')}`,
        });
      }
    }
  }
  for (const rule of cssRules(css)) {
    let offset = 0;
    for (const raw of rule.body.split(';')) {
      const at = offset;
      offset += raw.length + 1;
      const i = raw.indexOf(':');
      if (i < 0) continue;
      const prop = raw.slice(0, i).trim().toLowerCase();
      const value = raw.slice(i + 1).trim();
      if (!value) continue;
      const lead = raw.length - raw.trimStart().length;
      const line = rule.line + rule.body.slice(0, at + lead).split('\n').length - 1;
      const push = (kind: OffScaleKind) =>
        findings.push({ file, kind, line, detail: `${prop}: ${value.replace(/\s+/g, ' ').slice(0, 60)}` });
      if (prop === 'font-size') {
        if (!FONT_SIZE_OK.test(dropFallbacks(value))) push('font-size');
      } else if (prop === 'font') {
        const size = fontShorthandSize(value);
        if (size !== null && !FONT_SIZE_OK.test(size)) push('font-size');
      } else if (RADIUS_PROP.test(prop)) {
        if (!radiusOk(value)) push('radius');
      } else if (!COLOUR_EXEMPT.has(file)) {
        if (VALUE_COLOUR.test(withoutTokens(value)) || (NAMED_COLOUR_PROP.test(prop) && hasLiteralColour(value))) push('colour');
      }
    }
  }
  return findings.sort((a, b) => a.line - b.line);
}

/** Off-scale counts per file and kind, the shape of the baseline. */
export type OffScaleCounts = Record<string, Partial<Record<OffScaleKind, number>>>;

export function countOffScale(findings: OffScaleFinding[]): OffScaleCounts {
  const counts: OffScaleCounts = {};
  for (const f of findings) {
    counts[f.file] ??= {};
    counts[f.file][f.kind] = (counts[f.file][f.kind] ?? 0) + 1;
  }
  return counts;
}

/** The debt frozen when the guards came in (KT-1887): `src/styles/
 *  themeAuditBaseline.json`. A file may hold as many off-scale values of a
 *  kind as recorded, no more; a contrast cell under its floor must be listed. */
export type ThemeAuditBaseline = { contrast: string[]; offScale: OffScaleCounts };

export const BASELINE_FILE = 'styles/themeAuditBaseline.json';

export function loadBaseline(srcDir: string): ThemeAuditBaseline {
  return JSON.parse(readFileSync(join(srcDir, BASELINE_FILE), 'utf8')) as ThemeAuditBaseline;
}

/** What the baseline lets through, what goes beyond it, and what it holds
 *  that no longer occurs (to prune, as ESLint's bulk suppressions do). */
export type BaselineCheck = {
  /** Off-scale values in a file/kind whose count exceeds the baseline. */
  newOffScale: OffScaleFinding[];
  /** `file kind: baseline → now`, for a count that went down. */
  staleOffScale: string[];
  /** Contrast cells under their floor, not in the baseline. */
  newContrast: ContrastRow[];
  /** Baseline contrast keys that now pass or no longer exist. */
  staleContrast: string[];
};

export function checkBaseline(
  baseline: ThemeAuditBaseline, offScale: OffScaleFinding[], contrast: ContrastRow[],
): BaselineCheck {
  const counts = countOffScale(offScale);
  const newOffScale: OffScaleFinding[] = [];
  const staleOffScale: string[] = [];
  const files = new Set([...Object.keys(counts), ...Object.keys(baseline.offScale)]);
  for (const file of [...files].sort()) {
    for (const kind of OFF_SCALE_KINDS) {
      const now = counts[file]?.[kind] ?? 0;
      const allowed = baseline.offScale[file]?.[kind] ?? 0;
      if (now > allowed) newOffScale.push(...offScale.filter(f => f.file === file && f.kind === kind));
      else if (now < allowed) staleOffScale.push(`${file} ${kind}: ${allowed} → ${now}`);
    }
  }
  const frozen = new Set(baseline.contrast);
  const misses = contrast.filter(r => r.ratio < r.floor);
  const missKeys = new Set(misses.map(contrastKey));
  return {
    newOffScale,
    staleOffScale,
    newContrast: misses.filter(r => !frozen.has(contrastKey(r))),
    staleContrast: baseline.contrast.filter(k => !missKeys.has(k)),
  };
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
  /** Every off-scale value of the stylesheets, baseline included. */
  offScale: OffScaleFinding[];
  /** The off-scale values and contrast misses checked against the baseline. */
  baseline: BaselineCheck;
};

/** Every static check against `frontend/src`, as the CLI and the tests run it. */
export function auditRepo(srcDir: string): RepoAudit {
  const themes = loadThemes(srcDir);
  const sources = appSources(srcDir);
  const css = sources.filter(s => s.file.endsWith('.css'));
  const contrast = measureTextContrast(themes);
  const offScale = css.flatMap(s => offScaleValues(s.file, s.text));
  return {
    themes,
    contrast,
    offScale,
    baseline: checkBaseline(loadBaseline(srcDir), offScale, contrast),
    undefinedTokens: undefinedTokenUsages(sources, new Set(themes.get('dark')?.keys() ?? [])),
    undeclaredProperties: undeclaredCustomProperties(sources),
    forcedText: [
      ...css.flatMap(s => forcedTextOnTokenBackground(s.file, s.text)),
      ...sources.filter(s => s.file.endsWith('.tsx')).flatMap(s => forcedTextInInlineStyles(s.file, s.text)),
    ],
    focusRemovals: css.flatMap(s => focusRingRemovals(s.file, s.text)),
  };
}
