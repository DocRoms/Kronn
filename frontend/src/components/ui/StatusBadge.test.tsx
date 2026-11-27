// KT-1880 — StatusBadge: one tone per --kr-status-* colour, the status always
// in words beside the colour.
import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { StatusBadge } from './StatusBadge';
import { STATUS_TONES } from './statusTones';
import { DESIGN_TOKEN_GROUPS } from '../../styles/designTokens';
import {
  composite, contrastRatio, cssRules, declarations, loadThemes, parseColor, resolveValue, SURFACE_TOKENS, themeColor,
} from '../../styles/themeAudit';

const HERE = dirname(fileURLToPath(import.meta.url));
const CSS = readFileSync(join(HERE, 'StatusBadge.css'), 'utf8');
const THEMES = loadThemes(join(HERE, '..', '..'));
const THEME_NAMES = ['dark', 'arcade', 'light', 'matrix', 'gotham', 'sakura', 'euronews'];

/** The declarations a badge of `tone` gets from StatusBadge.css: the base rule,
 *  then its tone rule. */
function badgeDeclarations(tone: string): Map<string, string> {
  const out = new Map<string, string>();
  for (const selector of ['.ui-status-badge', `.ui-status-badge[data-tone="${tone}"]`]) {
    for (const rule of cssRules(CSS).filter(r => r.selector === selector)) {
      for (const [prop, value] of declarations(rule.body)) out.set(prop, value);
    }
  }
  return out;
}

describe('StatusBadge', () => {
  it('has one tone per status colour of the design tokens', () => {
    const status = DESIGN_TOKEN_GROUPS.find(g => g.scale === 'semanticColour')!.tokens
      .map(t => t.name).filter(n => n.startsWith('--kr-status-')).map(n => n.replace('--kr-status-', ''));
    expect([...STATUS_TONES].sort()).toEqual(status.sort());
    for (const tone of STATUS_TONES) expect(CSS, tone).toContain(`var(--kr-status-${tone})`);
  });

  it('reads its label at 4.5:1 on its own fill, for every tone, theme and surface', () => {
    // The label is 12 px text: 4.5:1 is the floor (docs/conventions/design-tokens.md).
    const misses: string[] = [];
    for (const theme of THEME_NAMES) {
      const themeTokens = THEMES.get(theme)!;
      for (const tone of STATUS_TONES) {
        const badge = badgeDeclarations(tone);
        const tokens = new Map([...themeTokens, ...badge]);
        const label = parseColor(resolveValue(badge.get('color')!, tokens));
        const fill = parseColor(resolveValue(badge.get('background-color')!, tokens));
        expect(label, `${theme} ${tone} label`).not.toBeNull();
        expect(fill, `${theme} ${tone} fill`).not.toBeNull();
        for (const surface of SURFACE_TOKENS) {
          const ratio = contrastRatio(label!, composite(fill!, themeColor(themeTokens, surface)!));
          if (ratio < 4.5) misses.push(`${theme} ${tone} on ${surface}: ${ratio.toFixed(2)}`);
        }
      }
    }
    expect(misses).toEqual([]);
  });

  it.each(STATUS_TONES)('shows the %s status in words, with a dot hidden from assistive technology', tone => {
    const { container } = render(<StatusBadge tone={tone}>State {tone}</StatusBadge>);
    const badge = container.firstElementChild!;
    expect(badge).toHaveAttribute('data-tone', tone);
    expect(badge).toHaveTextContent(`State ${tone}`);
    expect(badge.querySelector('.ui-status-badge-dot')).toHaveAttribute('aria-hidden', 'true');
  });

  it('takes an icon instead of the dot', () => {
    render(<StatusBadge tone="success" icon={<svg data-testid="icon" />}>Merged</StatusBadge>);
    expect(screen.getByText('Merged').parentElement!.querySelector('.ui-status-badge-dot')).toBeNull();
    expect(screen.getByTestId('icon').parentElement).toHaveAttribute('aria-hidden', 'true');
  });
});
