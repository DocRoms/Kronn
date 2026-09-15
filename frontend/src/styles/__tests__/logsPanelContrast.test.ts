// The running-agent log panel must be readable on every theme.
//
// Reported as "when an agent runs, the logs are unreadable — dark on dark".
// Measuring confirmed it, and only in the light theme: `.disc-logs-panel` paired
// `--kr-bg-code` (#1a1d23 there — code blocks stay dark on purpose) with
// `--kr-text-muted` (#51596b, a slate tuned to sit on WHITE). 2.40:1.
//
// Same root cause as the toast bug: a token name states a ROLE, not a colour.
// "muted" is dark in a light theme. This test reads the real CSS so a theme added
// tomorrow cannot reintroduce the pairing.

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const TOKENS = readFileSync(join(__dirname, '..', 'tokens.css'), 'utf8');
const PAGE_CSS = readFileSync(
  join(__dirname, '..', '..', 'pages', 'DiscussionsPage.css'),
  'utf8',
);

const hexToRgb = (hex: string): [number, number, number] | null => {
  // Both spellings occur in tokens.css: `#fff` and `#1a1d23`.
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return null;
  const h = m[1].length === 3 ? m[1].replace(/./g, (c) => c + c) : m[1];
  const n = parseInt(h, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
};

const luminance = ([r, g, b]: [number, number, number]) => {
  const ch = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b);
};

const contrast = (fg: string, bg: string): number | null => {
  const a = hexToRgb(fg);
  const b = hexToRgb(bg);
  if (!a || !b) return null; // rgba() needs its parent surface: not decidable here
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};

/** tokens.css splits one theme across SEVERAL `:root` blocks — backgrounds in
 *  one, text roles in another. Concatenate every block of a given selector so a
 *  pairing that spans two of them can still be resolved. */
const themeBlocks = (): Map<string, string> => {
  const blocks = new Map<string, string>();
  const re = /(:root(?:\[data-theme="[^"]+"\])?)\s*\{([^}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(TOKENS)) !== null) {
    const selector = m[1].trim();
    blocks.set(selector, (blocks.get(selector) ?? '') + m[2]);
  }
  return blocks;
};

const token = (block: string, name: string): string | null => {
  const m = new RegExp(`--${name}\\s*:\\s*([^;]+);`).exec(block);
  return m ? m[1].trim() : null;
};

/** The declared value of one property inside `.disc-logs-panel`. */
const logsPanelProp = (prop: string): string | null => {
  const rule = /\.disc-logs-panel\s*\{([\s\S]*?)\}/.exec(PAGE_CSS);
  if (!rule) return null;
  const m = new RegExp(`(?:^|[;{\\s])${prop}\\s*:\\s*([^;]+);`).exec(rule[1]);
  return m ? m[1].trim() : null;
};

const tokenName = (value: string | null): string | null => {
  if (!value) return null;
  const m = /var\(\s*--([a-z0-9-]+)\s*\)/i.exec(value);
  return m ? m[1] : null;
};

describe('running-agent logs panel contrast', () => {
  it('pairs its background with a text token meant for dark grounds', () => {
    expect(tokenName(logsPanelProp('background'))).toBe('kr-bg-code');
    // The regression was `kr-text-muted`, which is dark in the light theme.
    expect(tokenName(logsPanelProp('color'))).toBe('kr-text-on-dark');
  });

  it('stays readable in every theme that declares both as opaque colours', () => {
    const bgToken = tokenName(logsPanelProp('background'))!;
    const fgToken = tokenName(logsPanelProp('color'))!;
    let checked = 0;
    for (const [selector, block] of themeBlocks()) {
      const bg = token(block, bgToken);
      const fg = token(block, fgToken);
      if (!bg || !fg) continue;
      const ratio = contrast(fg, bg);
      if (ratio === null) continue; // translucent background, see contrast()
      checked += 1;
      expect(ratio, `${selector}: ${fg} on ${bg}`).toBeGreaterThanOrEqual(4.5);
    }
    // Guard the guard: a refactor that stopped resolving tokens would make the
    // loop vacuous and the test would pass while measuring nothing.
    expect(checked).toBeGreaterThan(0);
  });
});
