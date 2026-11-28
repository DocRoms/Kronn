import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const css = readFileSync(resolve(process.cwd(), 'src/pages/SettingsPage.css'), 'utf8');
const navCss = readFileSync(resolve(process.cwd(), 'src/components/settings/SettingsNav.css'), 'utf8');

/** The declarations of the rules for `selector`, in order (inside media queries too). */
const ruleBodies = (selector: string, text = css): string[] => {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return [...text.matchAll(new RegExp(`(?:^|[}\\s])${escaped}\\s*\\{([^}]+)\\}`, 'g'))].map(match => match[1]);
};

const variable = (body: string, name: string) => body.match(new RegExp(`${name}:\\s*([^;]+);`))?.[1].trim();

/**
 * KT-1923 — an anchor of Settings (`/config/system#settings-debug-logs`,
 * `#settings-languages`, an agent's row) once landed under the fixed top bar,
 * its heading covered while it held the focus. One rule gives every element
 * with an id inside Settings the offset of the top bar.
 * KT-2017 — the links to a sub-page's sections now stick in a band under the
 * top bar: the offset grows by the band's height, so nothing lands under it.
 */
describe('Settings anchor offset', () => {
  it('gives every element with an id inside Settings the offset, and the targets of the guided tour too', () => {
    const [rule] = ruleBodies('.settings-page [id]');
    expect(rule).toBeDefined();
    expect(rule).toContain('scroll-margin-top: var(--set-anchor-offset)');
    expect(css).toMatch(/\.settings-page \[data-tour-id\],\s*\.settings-page \[id\]\s*\{/);
  });

  it('stops an anchor under the top bar and the band, plus a breath, at every width', () => {
    const [page, ...others] = ruleBodies('.settings-page');
    expect(others.map(body => variable(body, '--set-anchor-offset')).filter(Boolean)).toEqual([]);
    expect(variable(page, '--set-anchor-offset')).toBe('calc(var(--set-band-top) + var(--set-band-height) + var(--kr-space-4))');
    // The band sits right under the top bar, whose height the app measures.
    expect(variable(page, '--set-band-top')).toBe('var(--app-bar-h, 51px)');
    expect(variable(page, '--set-band-height')).toBe('44px');
    expect(css).not.toContain('--set-anchor-offset: 112px');
  });

  it('gives the band the very top and height the offset adds', () => {
    const [band] = ruleBodies('.set-subpage-band', navCss);
    expect(band).toContain('position: sticky');
    expect(band).toContain('top: var(--set-band-top)');
    expect(band).toContain('height: var(--set-band-height)');
    expect(band).not.toMatch(/min-height|max-height/);
  });

  it('leaves no card its own scroll margin that could drift from the shared one', () => {
    const own = [...css.matchAll(/scroll-margin-top:\s*([^;]+);/g)].map(match => match[1].trim());
    expect(own).toEqual(['var(--set-anchor-offset)']);
  });
});
