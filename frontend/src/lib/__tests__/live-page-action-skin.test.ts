import { describe, expect, it } from 'vitest';
import {
  extractActionSkin, parseSkinColor, resolveActionSkin, skinContrast, skinThemeFor, type ActionSkin,
} from '../live-page-action-skin';

const skin = (shared: Record<string, unknown>, light = {}, dark = {}): ActionSkin => ({ shared, light, dark });

const PIXEL = {
  surface: '#1a1330', 'surface-alt': '#241a44', text: '#f3ecff', accent: '#ffcc33',
  'accent-text': '#1a1330', border: '#6b4fd8', radius: 0, 'border-width': 3, font: 'pixel',
};

describe('parseSkinColor', () => {
  it.each(['#fff', '#ffcc33', '#ffcc3380', '#fffa', 'rgb(1, 2, 3)', 'rgba(1,2,3,.5)', 'rgba(1, 2, 3, 50%)'])(
    'accepts %s', value => expect(parseSkinColor(value)).not.toBeNull(),
  );
  it.each([
    'red', 'transparent', 'var(--kr-accent)', 'url(x)', '#12', '#ggg', 'rgb(300,0,0)', 'rgba(0,0,0,2)',
    'rgb(0,0,0); color: red', '#fff} body{display:none', 'rgb(0 0 0)', 'hsl(0, 0%, 0%)', 12,
    'expression(alert(1))', '#fff url(https://evil)',
  ])('rejects %s', value => expect(parseSkinColor(value)).toBeNull());
});

describe('extractActionSkin', () => {
  const block = (json: string, attrs = 'type="application/json" kronn-action-skin') =>
    `<html><body><script ${attrs}>${json}</script></body></html>`;

  it('reads shared tokens and per-theme sets', () => {
    expect(extractActionSkin(block('{"accent":"#fc3","dark":{"text":"#fff"},"light":{"text":"#000"}}'))).toEqual({
      shared: { accent: '#fc3' }, dark: { text: '#fff' }, light: { text: '#000' },
    });
  });
  it('ignores pages without block, wrong type, malformed or oversized JSON', () => {
    expect(extractActionSkin('<p>nothing</p>')).toBeNull();
    expect(extractActionSkin(block('{"accent":"#fc3"}', 'type="text/javascript" kronn-action-skin'))).toBeNull();
    expect(extractActionSkin(block('{nope'))).toBeNull();
    expect(extractActionSkin(block('[1]'))).toBeNull();
    expect(extractActionSkin(block(JSON.stringify({ pad: 'x'.repeat(5000) })))).toBeNull();
  });
  it('does not trust a mention inside text', () => {
    expect(extractActionSkin('<p>kronn-action-skin</p>')).toBeNull();
  });
});

describe('resolveActionSkin', () => {
  it('emits only normalised values for the studio pixel theme', () => {
    expect(resolveActionSkin(skin(PIXEL), 'dark')).toEqual({
      '--kac-surface': 'rgb(26, 19, 48)', '--kac-surface-alt': 'rgb(36, 26, 68)', '--kac-text': 'rgb(243, 236, 255)',
      '--kac-text-muted': 'rgb(243, 236, 255)', '--kac-accent': 'rgb(255, 204, 51)', '--kac-accent-text': 'rgb(26, 19, 48)',
      '--kac-accent-ink': 'rgb(255, 204, 51)', '--kac-border': 'rgb(107, 79, 216)', '--kac-radius': '0px',
      '--kac-border-width': '3px', '--kac-font': '"Courier New", Courier, ui-monospace, Menlo, Consolas, monospace',
    });
  });

  it('returns nothing without a declaration', () => expect(resolveActionSkin(null, 'dark')).toEqual({}));

  it('drops an invalid token alone, keeps the others', () => {
    const out = resolveActionSkin(skin({ ...PIXEL, border: 'url(https://x/y.png)', accent: 'red' }), 'dark');
    expect(out['--kac-border']).toBeUndefined();
    expect(out['--kac-accent']).toBeUndefined();
    expect(out['--kac-accent-text']).toBeUndefined();
    expect(out['--kac-surface']).toBe('rgb(26, 19, 48)');
  });

  it.each(['url(x)', 'a;b', '1px}', '-1px', '17', 99, -2, 'NaN'])('refuses radius %s', radius => {
    expect(resolveActionSkin(skin({ radius }), 'dark')['--kac-radius']).toBeUndefined();
  });
  it('bounds radius and border-width', () => {
    expect(resolveActionSkin(skin({ radius: '16px', 'border-width': 4 }), 'dark'))
      .toMatchObject({ '--kac-radius': '16px', '--kac-border-width': '4px' });
    expect(resolveActionSkin(skin({ 'border-width': 5 }), 'dark')['--kac-border-width']).toBeUndefined();
  });

  it('only accepts the declared fonts and never forwards the string', () => {
    expect(resolveActionSkin(skin({ font: 'Papyrus, url(x)' }), 'dark')['--kac-font']).toBeUndefined();
    expect(resolveActionSkin(skin({ font: 'mono' }), 'dark')['--kac-font']).toContain('monospace');
    expect(resolveActionSkin(skin({ font: 'system' }), 'dark')['--kac-font']).toContain('system-ui');
  });

  it('never emits an injection payload, whatever the token', () => {
    const payload = 'red; background: url(https://evil/x)}';
    const out = resolveActionSkin(skin(Object.fromEntries(
      ['surface', 'text', 'accent', 'accent-text', 'border', 'field-bg', 'field-text', 'radius', 'font'].map(k => [k, payload]),
    )), 'dark');
    expect(out).toEqual({});
  });

  it('ignores unknown tokens, including ones that look like CSS', () => {
    const out = resolveActionSkin(skin({ ...PIXEL, 'box-shadow': '0 0 9px red', display: 'none', css: 'x{}' }), 'dark');
    expect(Object.keys(out).every(k => k.startsWith('--kac-'))).toBe(true);
    expect(JSON.stringify(out)).not.toMatch(/display|box-shadow|red/);
  });

  it('falls back to Kronn when text on surface is under 4.5:1', () => {
    const out = resolveActionSkin(skin({ ...PIXEL, text: '#2a2050', 'text-muted': '#fff' }), 'dark');
    for (const k of ['surface', 'surface-alt', 'text', 'text-muted']) expect(out[`--kac-${k}`]).toBeUndefined();
    expect(out['--kac-accent']).toBe('rgb(255, 204, 51)');
  });
  it('checks text against surface-alt too', () => {
    const out = resolveActionSkin(skin({ ...PIXEL, 'surface-alt': '#9a8ad0' }), 'dark');
    expect(out['--kac-surface']).toBeUndefined();
  });
  it('drops text-muted alone when only it is unreadable, and reuses text', () => {
    const out = resolveActionSkin(skin({ ...PIXEL, 'text-muted': '#3a2f60' }), 'dark');
    expect(out['--kac-text-muted']).toBe(out['--kac-text']);
    expect(out['--kac-surface']).toBeDefined();
  });
  it('drops accent and accent-text together when the button would not read', () => {
    const out = resolveActionSkin(skin({ ...PIXEL, 'accent-text': '#ffe066' }), 'dark');
    expect(out['--kac-accent']).toBeUndefined();
    expect(out['--kac-accent-text']).toBeUndefined();
    expect(out['--kac-text']).toBeDefined();
  });
  it('requires a surface with the text, and the reverse', () => {
    expect(resolveActionSkin(skin({ text: '#fff' }), 'dark')).toEqual({});
    expect(resolveActionSkin(skin({ surface: '#000' }), 'dark')).toEqual({});
  });
  it('requires both halves of the accent and field pairs', () => {
    expect(resolveActionSkin(skin({ accent: '#ffcc33' }), 'dark')).toEqual({});
    expect(resolveActionSkin(skin({ 'field-bg': '#000', 'field-text': '#111' }), 'dark')).toEqual({});
    expect(resolveActionSkin(skin({ 'field-bg': '#000', 'field-text': '#fff' }), 'dark'))
      .toEqual({ '--kac-field-bg': 'rgb(0, 0, 0)', '--kac-field-text': 'rgb(255, 255, 255)' });
  });
  it('refuses translucent surfaces, keeps a translucent border', () => {
    const out = resolveActionSkin(skin({ ...PIXEL, surface: 'rgba(26,19,48,0.5)', border: '#6b4fd880' }), 'dark');
    expect(out['--kac-surface']).toBeUndefined();
    expect(out['--kac-border']).toMatch(/^rgba\(107, 79, 216, 0\.5/);
  });
  it('does not print the accent as eyebrow text when it does not read on the surface', () => {
    const out = resolveActionSkin(skin({ ...PIXEL, accent: '#2a2060', 'accent-text': '#ffffff' }), 'dark');
    expect(out['--kac-accent']).toBeDefined();
    expect(out['--kac-accent-ink']).toBeUndefined();
  });

  it('keeps light and dark apart, light/dark overriding shared', () => {
    const s = skin({ radius: 4, accent: '#ffcc33', 'accent-text': '#1a1330' },
      { surface: '#ffffff', text: '#111111' }, { surface: '#1a1330', text: '#f3ecff' });
    expect(resolveActionSkin(s, 'light')['--kac-surface']).toBe('rgb(255, 255, 255)');
    expect(resolveActionSkin(s, 'dark')['--kac-surface']).toBe('rgb(26, 19, 48)');
    expect(resolveActionSkin(s, 'light')['--kac-radius']).toBe('4px');
  });
  it('validates each theme on its own', () => {
    const s = skin({}, { surface: '#ffffff', text: '#f3ecff' }, { surface: '#101010', text: '#f3ecff' });
    expect(resolveActionSkin(s, 'light')['--kac-text']).toBeUndefined();
    expect(resolveActionSkin(s, 'dark')['--kac-text']).toBeDefined();
  });
});

describe('skinContrast / skinThemeFor', () => {
  it('matches WCAG reference values', () => {
    const c = (s: string) => parseSkinColor(s)!;
    expect(skinContrast(c('#000'), c('#fff'))).toBeCloseTo(21, 1);
    expect(skinContrast(c('#777'), c('#fff'))).toBeCloseTo(4.48, 1);
  });
  it('maps Kronn themes to light/dark', () => {
    expect(skinThemeFor('dark')).toBe('dark');
    expect(skinThemeFor('matrix')).toBe('dark');
    expect(skinThemeFor('light')).toBe('light');
  });
});

describe('resolveActionSkin — the details block', () => {
  it('paints the details block with the surface when the Page declares no surface-alt', () => {
    // Light text on a light surface would otherwise sit on --kr-bg-raised,
    // which is dark under a dark Kronn theme: unreadable right above Launch.
    const vars = resolveActionSkin(skin({ surface: '#ffffff', text: '#111111' }), 'dark');
    expect(vars['--kac-surface-alt']).toBe(vars['--kac-surface']);
  });

  it('keeps a declared surface-alt', () => {
    const vars = resolveActionSkin(skin({ surface: '#ffffff', 'surface-alt': '#eeeeee', text: '#111111' }), 'light');
    expect(vars['--kac-surface-alt']).toBe('rgb(238, 238, 238)');
  });

  it('emits no surface-alt when the Page owns no surface', () => {
    expect(resolveActionSkin(skin({ accent: '#000000', 'accent-text': '#ffffff' }), 'light')['--kac-surface-alt']).toBeUndefined();
  });

  it('prints the accent as text only if it reads on surface-alt too', () => {
    // #8a6d00 reads on white (4.9:1) but not on the darker alt surface.
    const base = { surface: '#ffffff', text: '#000000', accent: '#8a6d00', 'accent-text': '#ffffff' };
    expect(resolveActionSkin(skin(base), 'light')['--kac-accent-ink']).toBeDefined();
    expect(resolveActionSkin(skin({ ...base, 'surface-alt': '#c8c8c8' }), 'light')['--kac-accent-ink']).toBeUndefined();
  });
});

describe('skinThemeFor without a host theme', () => {
  it('follows prefers-color-scheme', () => {
    const original = window.matchMedia;
    try {
      for (const dark of [true, false]) {
        window.matchMedia = ((query: string) => ({ matches: dark && query.includes('dark') })) as unknown as typeof window.matchMedia;
        expect(skinThemeFor(null)).toBe(dark ? 'dark' : 'light');
      }
    } finally {
      window.matchMedia = original;
    }
  });
});
