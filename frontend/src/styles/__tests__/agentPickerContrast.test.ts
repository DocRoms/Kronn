// KT-2010 — the agent picker's band and « Free » badge at 4.5:1 in every
// theme, measured from the real tokens.css with the audit's own maths
// (a translucent tint composited over the opaque surface under it).
import { describe, expect, it } from 'vitest';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { composite, contrastRatio, loadThemes, themeColor, type Rgba } from '../themeAudit';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const THEMES = loadThemes(SRC);

const color = (theme: string, name: string): Rgba => {
  const value = themeColor(THEMES.get(theme)!, name);
  expect(value, `${theme} ${name}`).not.toBeNull();
  return value!;
};

describe('agent picker band and « Free » badge contrast (KT-2010)', () => {
  it('measures every theme', () => {
    expect([...THEMES.keys()].sort()).toEqual(['arcade', 'dark', 'euronews', 'gotham', 'light', 'matrix', 'sakura']);
  });

  for (const theme of THEMES.keys()) {
    it(`${theme}: the band is opaque and its name, kind and count read at 4.5:1`, () => {
      const band = color(theme, '--kr-bg-raised');
      expect(band.a).toBe(1);
      for (const text of ['--kr-text-primary', '--kr-text-tertiary']) {
        expect(contrastRatio(color(theme, text), band), `${theme} ${text} on the band`).toBeGreaterThanOrEqual(4.5);
      }
    });

    it(`${theme}: the « Free » badge reads at 4.5:1 on a row at rest, hovered and current`, () => {
      const elevated = color(theme, '--kr-bg-elevated');
      const rows: Record<string, Rgba> = {
        rest: elevated,
        hover: composite(color(theme, '--kr-bg-hover'), elevated),
        current: composite(color(theme, '--kr-accent-bg'), elevated),
      };
      for (const [state, row] of Object.entries(rows)) {
        const chip = composite(color(theme, '--kr-success-bg'), row);
        expect(contrastRatio(color(theme, '--kr-text-secondary'), chip), `${theme} badge on a ${state} row`)
          .toBeGreaterThanOrEqual(4.5);
      }
    });
  }
});
