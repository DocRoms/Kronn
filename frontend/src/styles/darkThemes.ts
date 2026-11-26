// Browser-safe on purpose: the theme audit imports node:fs, the app cannot.

/** Themes whose surfaces are dark. */
export const DARK_THEMES: ReadonlySet<string> = new Set(['dark', 'arcade', 'gotham', 'matrix']);
