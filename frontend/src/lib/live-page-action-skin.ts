// A Page may dress ITS action card, never its content. The card is rendered by
// the host, outside the sandbox, and its "Launch" button is a trust boundary:
// the Page therefore supplies validated tokens, one by one, and never CSS,
// selectors, url(), HTML or text. Every token that fails a check is dropped
// alone and the card keeps Kronn's value for it.

import { DARK_THEMES } from '../styles/darkThemes';

export type ActionSkinTheme = 'light' | 'dark';

/** Tokens a Page may set. Anything else is ignored. */
export const ACTION_SKIN_COLOR_TOKENS = [
  'surface', 'surface-alt', 'text', 'text-muted', 'accent', 'accent-text',
  'border', 'field-bg', 'field-text',
] as const;
export const ACTION_SKIN_FONTS = ['system', 'mono', 'pixel'] as const;
export type ActionSkinFont = typeof ACTION_SKIN_FONTS[number];


/** Which of the skin's `light` / `dark` sets applies to the host's `data-theme`. */
export function skinThemeFor(dataTheme: string | null): ActionSkinTheme {
  if (dataTheme) return DARK_THEMES.has(dataTheme) ? 'dark' : 'light';
  return typeof window !== 'undefined' && window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

/** The declaration as the Page wrote it: shared tokens plus per-theme overrides. */
export interface ActionSkin {
  shared: Record<string, unknown>;
  light: Record<string, unknown>;
  dark: Record<string, unknown>;
}

/** Fixed local stacks: choosing `pixel` never loads anything from the network. */
export const ACTION_SKIN_FONT_STACKS: Record<ActionSkinFont, string> = {
  system: 'system-ui, -apple-system, "Segoe UI", Roboto, sans-serif',
  mono: 'ui-monospace, "SF Mono", Menlo, Consolas, monospace',
  pixel: '"Courier New", Courier, ui-monospace, Menlo, Consolas, monospace',
};

/** Every custom property a skin can set on the card root, spelled out so the
 * theme audit sees them declared (the stylesheet reads them with fallbacks). */
export const ACTION_SKIN_PROPERTIES = {
  '--kac-surface': 'surface', '--kac-surface-alt': 'surface-alt', '--kac-text': 'text',
  '--kac-text-muted': 'text-muted', '--kac-accent': 'accent', '--kac-accent-text': 'accent-text',
  '--kac-accent-ink': 'accent', '--kac-border': 'border', '--kac-field-bg': 'field-bg',
  '--kac-field-text': 'field-text', '--kac-radius': 'radius', '--kac-border-width': 'border-width',
  '--kac-font': 'font',
} as const;

const MAX_BLOCK_BYTES = 4096;
const MIN_CONTRAST = 4.5;
const RADIUS_MAX = 16;
const BORDER_WIDTH_MAX = 4;
/** Surfaces and the accent must be opaque: a translucent one would let the
 * Page behind the card decide what the text sits on. */
const OPAQUE = new Set<string>(['surface', 'surface-alt', 'accent', 'field-bg']);

type Rgb = { r: number; g: number; b: number; a: number };

const HEX = /^#(?:[0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;
const NUM = String.raw`\d{1,3}(?:\.\d+)?`;
const ALPHA = String.raw`(?:\d{1,3}(?:\.\d+)?%|\d*\.?\d+)`;
const RGB_FN = new RegExp(String.raw`^rgb\(\s*(${NUM})\s*,\s*(${NUM})\s*,\s*(${NUM})\s*\)$`, 'i');
const RGBA_FN = new RegExp(String.raw`^rgba\(\s*(${NUM})\s*,\s*(${NUM})\s*,\s*(${NUM})\s*,\s*(${ALPHA})\s*\)$`, 'i');

/** Strictly numeric colours only; returns `null` for anything else. */
export function parseSkinColor(raw: unknown): Rgb | null {
  if (typeof raw !== 'string') return null;
  const value = raw.trim();
  if (value.length > 40) return null;
  if (HEX.test(value)) {
    let hex = value.slice(1);
    if (hex.length <= 4) hex = hex.replace(/./g, c => c + c);
    const n = (i: number) => parseInt(hex.slice(i, i + 2), 16);
    return { r: n(0), g: n(2), b: n(4), a: hex.length === 8 ? n(6) / 255 : 1 };
  }
  const match = RGB_FN.exec(value) ?? RGBA_FN.exec(value);
  if (!match) return null;
  const channels = [match[1], match[2], match[3]].map(Number);
  if (channels.some(c => c > 255)) return null;
  let a = 1;
  if (match[4] !== undefined) {
    a = match[4].endsWith('%') ? Number.parseFloat(match[4]) / 100 : Number(match[4]);
    if (!(a >= 0 && a <= 1)) return null;
  }
  return { r: channels[0], g: channels[1], b: channels[2], a };
}

const toCss = ({ r, g, b, a }: Rgb): string => {
  const c = (v: number) => Math.round(v);
  return a >= 1
    ? `rgb(${c(r)}, ${c(g)}, ${c(b)})`
    : `rgba(${c(r)}, ${c(g)}, ${c(b)}, ${Math.round(a * 1000) / 1000})`;
};

const channel = (v: number) => {
  const s = v / 255;
  return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
};
const luminance = ({ r, g, b }: Rgb) => 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
const over = (fg: Rgb, bg: Rgb): Rgb => ({
  r: fg.r * fg.a + bg.r * (1 - fg.a),
  g: fg.g * fg.a + bg.g * (1 - fg.a),
  b: fg.b * fg.a + bg.b * (1 - fg.a),
  a: 1,
});

/** WCAG 2 contrast ratio of `fg` (composited over the opaque `bg`) on `bg`. */
export function skinContrast(fg: Rgb, bg: Rgb): number {
  const top = fg.a < 1 ? over(fg, bg) : fg;
  const [hi, lo] = [luminance(top), luminance(bg)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

function readLength(raw: unknown, max: number): number | null {
  const value = typeof raw === 'string' && /^\d{1,2}(?:\.\d+)?(?:px)?$/i.test(raw.trim())
    ? Number.parseFloat(raw) : raw;
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= max ? value : null;
}

/**
 * Extract the Page's `<script type="application/json" kronn-action-skin>` block.
 * The HTML is parsed, never run. Only the first block counts; an oversized or
 * malformed one yields `null`.
 */
export function extractActionSkin(html: string): ActionSkin | null {
  if (!html || !html.includes('kronn-action-skin')) return null;
  let text: string | null = null;
  try {
    const doc = new DOMParser().parseFromString(html, 'text/html');
    for (const script of Array.from(doc.querySelectorAll('script[kronn-action-skin]'))) {
      if ((script.getAttribute('type') ?? '').trim().toLowerCase() === 'application/json') {
        text = script.textContent;
        break;
      }
    }
  } catch {
    return null;
  }
  if (text === null || new TextEncoder().encode(text).length > MAX_BLOCK_BYTES) return null;
  let parsed: unknown;
  try { parsed = JSON.parse(text); } catch { return null; }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return null;
  const { light, dark, ...shared } = parsed as Record<string, unknown>;
  const plain = (v: unknown) => (v && typeof v === 'object' && !Array.isArray(v) ? v as Record<string, unknown> : {});
  return { shared, light: plain(light), dark: plain(dark) };
}

/**
 * Validate the declaration for one theme into CSS custom properties for the
 * card root (`--kac-*`). The result only ever holds values this function built
 * itself (normalised colours, numbers with `px`, a fixed font stack).
 */
export function resolveActionSkin(skin: ActionSkin | null, theme: ActionSkinTheme): Record<string, string> {
  if (!skin) return {};
  const merged: Record<string, unknown> = { ...skin.shared, ...skin[theme] };
  const colors = new Map<string, Rgb>();
  for (const name of ACTION_SKIN_COLOR_TOKENS) {
    if (!(name in merged)) continue;
    const color = parseSkinColor(merged[name]);
    if (!color || (OPAQUE.has(name) && color.a < 1)) continue;
    colors.set(name, color);
  }
  const drop = (...names: string[]) => names.forEach(n => colors.delete(n));
  const readable = (fg: string, bg: string) => {
    const f = colors.get(fg);
    const b = colors.get(bg);
    return Boolean(f && b && skinContrast(f, b) >= MIN_CONTRAST);
  };

  // Text and surfaces travel together: one without the other would sit on a
  // Kronn colour the Page cannot see.
  const surfaces = ['surface', 'surface-alt'].filter(n => colors.has(n));
  if (!colors.has('surface') || !colors.has('text') || !surfaces.every(s => readable('text', s))) {
    drop('surface', 'surface-alt', 'text', 'text-muted');
  } else if (colors.has('text-muted') && !surfaces.every(s => readable('text-muted', s))) {
    drop('text-muted');
  }
  if (!readable('accent-text', 'accent')) drop('accent', 'accent-text');
  if (!readable('field-text', 'field-bg')) drop('field-bg', 'field-text');

  const out: Record<string, string> = {};
  for (const [name, color] of colors) out[`--kac-${name}`] = toCss(color);
  // Once the Page owns the surface, no Kronn colour may stay under or on it:
  // the details block falls back to the checked surface, muted text to text.
  if (out['--kac-surface'] && !out['--kac-surface-alt']) out['--kac-surface-alt'] = out['--kac-surface'];
  if (out['--kac-surface'] && !out['--kac-text-muted']) out['--kac-text-muted'] = out['--kac-text'];

  // The eyebrow and the details links print the accent as text: only when it
  // reads on every surface the card paints.
  const accent = colors.get('accent');
  const inked = ['surface', 'surface-alt'].map(n => colors.get(n)).filter((c): c is Rgb => Boolean(c));
  if (accent && colors.has('surface') && inked.every(bg => skinContrast(accent, bg) >= MIN_CONTRAST)) {
    out['--kac-accent-ink'] = toCss(accent);
  }

  const radius = readLength(merged.radius, RADIUS_MAX);
  if (radius !== null) out['--kac-radius'] = `${radius}px`;
  const width = readLength(merged['border-width'], BORDER_WIDTH_MAX);
  if (width !== null) out['--kac-border-width'] = `${width}px`;
  const font = merged.font;
  if (typeof font === 'string' && (ACTION_SKIN_FONTS as readonly string[]).includes(font)) {
    out['--kac-font'] = ACTION_SKIN_FONT_STACKS[font as ActionSkinFont];
  }
  return out;
}
