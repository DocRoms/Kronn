// KT-2017 — the band of section links of a Settings sub-page, read from its
// stylesheet (happy-dom has no layout): one line on an opaque surface in every
// theme, a line under it once stuck, a mark that does not rest on colour
// alone, and a layer under the top bar and everything that floats.
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { cssText } from '../../test/cssBlocks';
import { loadThemes, themeColor } from '../themeAudit';
import { evalZIndex, loadBaseTokens } from '../zIndexAudit';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const css = cssText(join(SRC, 'components/settings/SettingsNav.css'));
const THEMES = loadThemes(SRC);
const TOKENS = loadBaseTokens(SRC);

const rule = (selector: string): string => {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`(?:^|[}\\s])${escaped}\\s*\\{([^}]+)\\}`).exec(css)?.[1] ?? '';
};

describe('the band of section links of a Settings sub-page', () => {
  it('stands on its own surface, opaque in each of the seven themes', () => {
    expect(rule('.set-subpage-band')).toContain('background: var(--kr-bg-sticky)');
    expect([...THEMES.keys()].sort()).toEqual(['arcade', 'dark', 'euronews', 'gotham', 'light', 'matrix', 'sakura']);
    for (const [theme, tokens] of THEMES) {
      const surface = themeColor(tokens, '--kr-bg-sticky');
      expect({ theme, opaque: surface?.a }).toEqual({ theme, opaque: 1 });
      // The page's own ground: the band reads as part of the page, not a card.
      expect({ theme, surface }).toEqual({ theme, surface: themeColor(tokens, '--kr-bg-base') });
    }
  });

  it('draws a line and a shadow under it once stuck, none before', () => {
    expect(rule('.set-subpage-band')).toContain('border-bottom: 1px solid transparent');
    const stuck = rule('.set-subpage-band[data-stuck]');
    expect(stuck).toContain('border-bottom-color: var(--kr-border)');
    expect(stuck).toContain('box-shadow: var(--kr-shadow-sm)');
  });

  it('sits under the top bar and the menus an agent row lifts, and far under dialogs, popovers and toasts', () => {
    const own = evalZIndex(/z-index:\s*([^;]+);/.exec(rule('.set-subpage-band'))![1], TOKENS)!;
    expect(own).toBeGreaterThan(10); // over the content and the sticky left list (10)
    expect(own).toBeLessThan(40); // an open model menu lifts its agent row to 40
    for (const layer of ['--kr-z-sticky', '--kr-z-modal', '--kr-z-popover', '--kr-z-toast']) {
      expect(own).toBeLessThan(evalZIndex(`var(${layer})`, TOKENS)!);
    }
  });

  it('keeps its links on one line, and marks the one being read with more than a colour', () => {
    expect(rule('.set-subpage-links')).toContain('flex-wrap: nowrap');
    expect(rule('.set-subpage-links a')).toContain('white-space: nowrap');
    const current = rule(".set-subpage-links a[aria-current='location']");
    expect(current).toContain('text-decoration: underline');
    expect(current).toContain('text-decoration-thickness: 2px');
    // No change of weight or padding: the mark moves no link in the line.
    expect(current).not.toMatch(/font-weight|padding/);
  });
});
