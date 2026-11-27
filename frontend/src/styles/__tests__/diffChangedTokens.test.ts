// KT-2004 — the three tokens of a changed diff line (--kr-diff-changed-bg,
// -border, -text): declared in every block of tokens.css that declares the
// other diff tokens, resolved in every theme, and the text holds 4.5:1 on its
// tinted row over the code surface and the elevated surface, like the added
// and removed text (designTokens.test.ts).
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { composite, contrastRatio, loadThemes, themeColor } from '../themeAudit';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const THEMES = loadThemes(SRC);
const THEME_NAMES = ['dark', 'arcade', 'light', 'matrix', 'gotham', 'sakura', 'euronews'];
const CHANGED = ['--kr-diff-changed-bg', '--kr-diff-changed-border', '--kr-diff-changed-text'];

/** The blocks of tokens.css (selector and body), comments removed. */
function blocks(): { selector: string; body: string }[] {
  const css = readFileSync(join(SRC, 'styles', 'tokens.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
  return [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map(([, selector, body]) => ({ selector: selector.trim(), body }));
}

describe('--kr-diff-changed-* (KT-2004)', () => {
  it('is declared next to the other diff tokens, in every block that declares them', () => {
    const withDiff = blocks().filter(b => /--kr-diff-(added|removed)-/.test(b.body));
    expect(withDiff.length).toBeGreaterThanOrEqual(2);
    for (const block of withDiff) {
      const kinds = new Set([...block.body.matchAll(/--kr-diff-(?:added|removed)-(bg|border|text)\s*:/g)].map(m => m[1]));
      for (const part of kinds) {
        expect(block.body, `${block.selector} declares --kr-diff-changed-${part}`).toMatch(new RegExp(`--kr-diff-changed-${part}\\s*:`));
      }
    }
  });

  it('resolves to a colour in every theme', () => {
    for (const theme of THEME_NAMES) {
      for (const name of CHANGED) expect(themeColor(THEMES.get(theme)!, name), `${theme} ${name}`).not.toBeNull();
    }
  });

  it('reads the changed text at WCAG AA on its row, over the code and the elevated surfaces', () => {
    const misses: string[] = [];
    for (const theme of THEME_NAMES) {
      const tokens = THEMES.get(theme)!;
      for (const surface of ['--kr-code-surface', '--kr-bg-elevated']) {
        const row = composite(themeColor(tokens, '--kr-diff-changed-bg')!, themeColor(tokens, surface)!);
        const ratio = contrastRatio(themeColor(tokens, '--kr-diff-changed-text')!, row);
        if (ratio < 4.5) misses.push(`${theme} on ${surface}: ${ratio.toFixed(2)}`);
      }
    }
    expect(misses).toEqual([]);
  });
});
