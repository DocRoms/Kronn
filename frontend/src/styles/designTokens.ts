// KT-1878 — the design scales of tokens.css, as one typed table.
//
// Settings > Design system renders this table (name, computed value in the
// current theme, one line on its use), and src/styles/__tests__/designTokens.test.ts
// compares it with tokens.css both ways: a token of a scale that tokens.css
// declares but this table omits, or the reverse, fails the build. The page
// therefore cannot drift from the stylesheet.

/** The scales, in the order the page shows them. */
export type DesignScale =
  | 'text'
  | 'spacing'
  | 'radius'
  | 'breakpoint'
  | 'duration'
  | 'weight'
  | 'controlHeight'
  | 'overlayWidth'
  | 'textRole'
  | 'semanticColour';

/** How the page previews a token. */
export type TokenPreview = 'text' | 'space' | 'radius' | 'width' | 'duration' | 'weight' | 'height' | 'colour';

export interface DesignToken {
  /** The custom property, `--kr-…`. */
  name: string;
  /** i18n key of the one line on its use. */
  use: string;
}

export interface DesignTokenGroup {
  scale: DesignScale;
  /** i18n key of the group title. */
  title: string;
  preview: TokenPreview;
  /** A token of tokens.css whose name matches belongs to this group: the
   *  test fails when tokens.css declares one the table does not list. */
  match: RegExp;
  tokens: DesignToken[];
}

const tokens = (names: string[]): DesignToken[] =>
  names.map(name => ({ name, use: `designSystem.use.${name.replace(/^--kr-/, '')}` }));

export const DESIGN_TOKEN_GROUPS: readonly DesignTokenGroup[] = [
  {
    scale: 'text',
    title: 'designSystem.scale.text',
    preview: 'text',
    match: /^--kr-type-/,
    tokens: tokens(['--kr-type-sm', '--kr-type-md', '--kr-type-lg', '--kr-type-xl', '--kr-type-2xl']),
  },
  {
    scale: 'spacing',
    title: 'designSystem.scale.spacing',
    preview: 'space',
    match: /^--kr-space-/,
    tokens: tokens([
      '--kr-space-1', '--kr-space-2', '--kr-space-3', '--kr-space-4',
      '--kr-space-5', '--kr-space-6', '--kr-space-7', '--kr-space-8',
    ]),
  },
  {
    scale: 'radius',
    title: 'designSystem.scale.radius',
    preview: 'radius',
    match: /^--kr-radius-(control|surface|overlay)$/,
    tokens: tokens(['--kr-radius-control', '--kr-radius-surface', '--kr-radius-overlay']),
  },
  {
    scale: 'breakpoint',
    title: 'designSystem.scale.breakpoint',
    preview: 'width',
    match: /^--kr-bp-/,
    tokens: tokens(['--kr-bp-sm', '--kr-bp-md', '--kr-bp-lg']),
  },
  {
    scale: 'duration',
    title: 'designSystem.scale.duration',
    preview: 'duration',
    match: /^--kr-duration-/,
    tokens: tokens(['--kr-duration-fast', '--kr-duration-base', '--kr-duration-slow']),
  },
  {
    scale: 'weight',
    title: 'designSystem.scale.weight',
    preview: 'weight',
    match: /^--kr-weight-/,
    tokens: tokens(['--kr-weight-regular', '--kr-weight-medium', '--kr-weight-semibold', '--kr-weight-bold']),
  },
  {
    scale: 'controlHeight',
    title: 'designSystem.scale.controlHeight',
    preview: 'height',
    match: /^--kr-control-h-/,
    tokens: tokens(['--kr-control-h-sm', '--kr-control-h-md', '--kr-control-h-touch', '--kr-control-h-lg']),
  },
  {
    // KT-1881 — the three widths of a Dialog and a Drawer (components/ui/).
    scale: 'overlayWidth',
    title: 'designSystem.scale.overlayWidth',
    preview: 'width',
    match: /^--kr-overlay-w-/,
    tokens: tokens(['--kr-overlay-w-sm', '--kr-overlay-w-md', '--kr-overlay-w-lg']),
  },
  {
    scale: 'textRole',
    title: 'designSystem.scale.textRole',
    preview: 'colour',
    match: /^--kr-text-(primary|secondary|muted)$/,
    tokens: tokens(['--kr-text-primary', '--kr-text-secondary', '--kr-text-muted']),
  },
  {
    scale: 'semanticColour',
    title: 'designSystem.scale.semanticColour',
    preview: 'colour',
    // `--kr-code-bg` is the 0.8.11 compat alias of `--kr-bg-code`, not a scale token.
    match: /^--kr-(status|diff|code)-(?!bg$)/,
    tokens: tokens([
      '--kr-status-success', '--kr-status-warning', '--kr-status-danger', '--kr-status-info', '--kr-status-neutral',
      '--kr-diff-added-bg', '--kr-diff-added-border', '--kr-diff-added-text',
      '--kr-diff-removed-bg', '--kr-diff-removed-border', '--kr-diff-removed-text',
      '--kr-code-surface', '--kr-code-text', '--kr-code-comment', '--kr-code-keyword', '--kr-code-string',
      '--kr-code-number', '--kr-code-function', '--kr-code-variable', '--kr-code-symbol',
    ]),
  },
];

/** Every token of the scales, in page order. */
export const DESIGN_TOKENS: readonly DesignToken[] = DESIGN_TOKEN_GROUPS.flatMap(g => g.tokens);

/** The breakpoints in px, for `matchMedia` and stylesheets: a custom property
 *  cannot be read inside `@media`. Equal to `--kr-bp-*` (checked by the test).
 *  A breakpoint is the first width of the wider layout, as in `useIsMobile(bp)`
 *  (narrower than bp): a stylesheet writes `@media (max-width: <bp - 1>px)`. */
export const BREAKPOINTS = { sm: 640, md: 768, lg: 900 } as const;

/** The widths of a Dialog and a Drawer in px (`--kr-overlay-w-*`, checked by
 *  the test), for the gallery's captions. */
export const OVERLAY_WIDTHS = { sm: 480, md: 640, lg: 960 } as const;

/** The root size the rem text scale is written against. */
export const ROOT_FONT_PX = 16;

/** `0.75rem` → 12 at the default root; null for a value that is not in rem. */
export function remToPx(value: string, rootPx = ROOT_FONT_PX): number | null {
  const m = /^\s*(\d*\.?\d+)rem\s*$/.exec(value);
  return m ? Number(m[1]) * rootPx : null;
}
