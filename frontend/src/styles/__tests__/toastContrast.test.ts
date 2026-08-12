// KT-252 — every toast must be readable on every theme.
//
// Reported as "the Batman theme shows yellow on yellow". Measuring showed it was
// far broader: gotham rendered #ffd400 text on #ffc800 (contrast 1.00 — the same
// colour), the DEFAULT theme was 1.55, and matrix 1.12. Four of six themes were
// unreadable on most toast types.
//
// The cause was two roles sharing one token. `--kr-text-on-dark` means "text on a
// DARK surface"; a toast filled with bright yellow is not a dark surface. So each
// accent now has its own paired text token, and this test reads the real CSS to
// prove the pairing holds — including for a theme added tomorrow, which is what
// stops the bug recurring rather than just fixing the reported instance.

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const TOKENS = readFileSync(
  join(__dirname, '..', 'tokens.css'),
  'utf8',
);

/** Toast background types, as `ToastItem` derives them. */
const TOAST_TYPES = ['warning', 'error', 'success', 'cyan'] as const;

/** WCAG AA for normal text. Toast text is 13px, so AA is the right bar. */
const MIN_CONTRAST = 4.5;

function relativeLuminance(hex: string): number | null {
  let value = hex.trim().replace('#', '');
  if (value.length === 3) value = value.split('').map((c) => c + c).join('');
  if (!/^[0-9a-fA-F]{6}$/.test(value)) return null;
  const channels = [0, 2, 4].map((offset) => {
    const part = parseInt(value.slice(offset, offset + 2), 16) / 255;
    return part <= 0.03928 ? part / 12.92 : ((part + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
}

function contrast(a: string, b: string): number | null {
  const first = relativeLuminance(a);
  const second = relativeLuminance(b);
  if (first === null || second === null) return null;
  const light = Math.max(first, second);
  const dark = Math.min(first, second);
  return (light + 0.05) / (dark + 0.05);
}

/** Every `:root…{ }` block that defines a theme, keyed by its selector. */
function themeBlocks(): Map<string, string> {
  const blocks = new Map<string, string>();
  const pattern = /(:root(?:\[data-theme="[^"]+"\])?)\s*\{([^}]*)\}/g;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(TOKENS)) !== null) {
    // A theme is a block that defines the accents; other :root blocks (z-index
    // scales and the like) are not themes and must not be asserted on.
    if (match[2].includes('--kr-warning:')) blocks.set(match[1], match[2]);
  }
  return blocks;
}

function token(block: string, name: string): string | null {
  const match = new RegExp(`--${name}:\\s*([^;]+);`).exec(block);
  return match ? match[1].trim() : null;
}

describe('toast contrast', () => {
  const themes = themeBlocks();

  it('finds the themes at all', () => {
    // Guards the test itself: a selector-parsing change that matched nothing
    // would make every assertion below vacuous — the tautology trap.
    expect(themes.size).toBeGreaterThanOrEqual(5);
  });

  it.each([...themes.keys()])('%s pairs readable text with every toast', (selector) => {
    const block = themes.get(selector)!;
    for (const type of TOAST_TYPES) {
      const background = token(block, `kr-${type}`);
      if (!background || !background.startsWith('#')) continue;
      const text = token(block, `kr-on-${type}`);
      expect(text, `${selector} has no --kr-on-${type}`).toBeTruthy();
      const ratio = contrast(background, text!);
      expect(
        ratio,
        `${selector} ${type}: ${text} on ${background} is ${ratio?.toFixed(2)}:1`,
      ).toBeGreaterThanOrEqual(MIN_CONTRAST);
    }
  });

  it('the old token would still fail, which is why it was replaced', () => {
    // Keeps the reason visible: someone reverting to --kr-text-on-dark should
    // see this test explain why that token cannot carry this role.
    const gotham = themes.get(':root[data-theme="gotham"]');
    expect(gotham).toBeTruthy();
    const warning = token(gotham!, 'kr-warning')!;
    const onDark = token(gotham!, 'kr-text-on-dark')!;
    expect(contrast(warning, onDark)).toBeLessThan(1.5);
  });
});
