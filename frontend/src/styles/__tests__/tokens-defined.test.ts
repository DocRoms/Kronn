// Guard against "phantom" CSS custom properties: a `var(--kr-…)` used in src/
// that tokens.css never defines. Without a fallback the browser drops the whole
// declaration; with one, the fallback silently wins in every theme — which is
// how `var(--kr-on-accent, #fff)` put white text on the lime and yellow accents.
import { describe, it, expect } from 'vitest';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  appSources, contrastRatio, cssRules, declarations, forcedTextOnTokenBackground,
  loadThemes, themeColor, undefinedTokenUsages,
} from '../themeAudit';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const THEMES = loadThemes(SRC);
const BASE = new Set(THEMES.get('dark')?.keys() ?? []);
const SOURCES = appSources(SRC);
const THEME_NAMES = ['dark', 'light', 'matrix', 'gotham', 'sakura', 'euronews'];

const list = (findings: Array<{ file: string; line: number; detail: string }>) =>
  findings.map(f => `  src/${f.file}:${f.line}  ${f.detail}`).join('\n');

describe('CSS design tokens', () => {
  it('parses every theme, so the checks below are not vacuous', () => {
    expect([...THEMES.keys()].sort()).toEqual([...THEME_NAMES].sort());
    expect(BASE.size).toBeGreaterThan(100);
    expect(SOURCES.some(s => s.file === 'components/DiscussionActionCard.css')).toBe(true);
  });

  it('uses no --kr-* var that tokens.css does not define, fallback or not', () => {
    const offenders = undefinedTokenUsages(SOURCES, BASE);
    expect(offenders, `Undefined --kr-* vars (define them in tokens.css for every theme, `
      + `or use an existing token):\n${list(offenders)}`).toEqual([]);
  });

  it('rejects an undefined token even when a fallback hides it', () => {
    const fixture = [{ file: 'x.css', text: '.a { color: var(--kr-on-accent, #fff); }\n.b { color: var(--kr-text-on-accent); }' }];
    expect(undefinedTokenUsages(fixture, BASE)).toEqual([
      { file: 'x.css', line: 1, detail: '--kr-on-accent' },
    ]);
  });

  it('ignores a token mentioned only inside a CSS comment', () => {
    const fixture = [{ file: 'x.css', text: '/* var(--kr-gone) */\n.a { color: var(--kr-text-primary); }' }];
    expect(undefinedTokenUsages(fixture, BASE)).toEqual([]);
  });

  it('never pins white text on an accent-filled rule', () => {
    const offenders = SOURCES.filter(s => s.file.endsWith('.css'))
      .flatMap(s => forcedTextOnTokenBackground(s.file, s.text));
    expect(offenders, `Use var(--kr-text-on-accent) on an accent fill:\n${list(offenders)}`).toEqual([]);
  });

  it('flags white text on an accent fill, and nothing else', () => {
    const css = [
      '.bad { background: var(--kr-accent); color: white; }',
      '.bad2 { background-color: rgba(var(--kr-accent-rgb), 0.9); color: #FFF !important; }',
      '.ok { background: var(--kr-accent); color: var(--kr-text-on-accent); }',
      '.photo { background: #000; color: #fff; }',
    ].join('\n');
    expect(forcedTextOnTokenBackground('x.css', css).map(f => f.line)).toEqual([1, 2]);
  });

  it('keeps Euronews text overlays on its light palette', () => {
    const euronewsOnly = cssRules(readTokens())
      .filter(rule => rule.selector === ':root[data-theme="euronews"]')
      .flatMap(rule => declarations(rule.body).map(([prop]) => prop));
    for (const token of ['--kr-text-rgb', '--kr-text-faint-rgb', '--kr-text-ghost-rgb']) {
      expect(euronewsOnly, `${token} must not inherit the dark theme RGB triple`).toContain(token);
    }
  });

  it('keeps NVIDIA text at WCAG AA contrast in every theme', () => {
    for (const name of THEME_NAMES) {
      const tokens = THEMES.get(name)!;
      const foreground = themeColor(tokens, '--kr-agent-nvidia-text');
      const background = themeColor(tokens, '--kr-bg-surface');
      expect(foreground && background, `${name} NVIDIA colours must be opaque`).toBeTruthy();
      expect(
        contrastRatio(foreground!, background!),
        `${name} NVIDIA text must reach WCAG AA on its card surface`,
      ).toBeGreaterThanOrEqual(4.5);
    }
  });

  it('reads text-on-accent over the accent at WCAG AA in every theme', () => {
    for (const name of THEME_NAMES) {
      const tokens = THEMES.get(name)!;
      const text = themeColor(tokens, '--kr-text-on-accent');
      const fill = themeColor(tokens, '--kr-accent');
      expect(text && fill, `${name} accent pairing must be opaque`).toBeTruthy();
      expect(contrastRatio(text!, fill!), `${name}: text-on-accent over accent`)
        .toBeGreaterThanOrEqual(4.5);
    }
  });

  it.each([
    ['components/DiscussionActionCard.css', '.discussion-action-card__launch, .discussion-action-card__open'],
    ['pages/Dashboard.css', '.project-git-switcher > button'],
    ['pages/Dashboard.css', ".project-git-branch-list article[data-current='true'] .project-git-branch-node"],
  ])('%s %s paints its label with text-on-accent', (file, selector) => {
    const source = SOURCES.find(s => s.file === file)!;
    const rule = cssRules(source.text).find(r => r.selector === selector);
    expect(rule, `${selector} not found in ${file}`).toBeTruthy();
    const decls = new Map(declarations(rule!.body));
    expect(decls.get('background')).toBe('var(--kr-accent)');
    expect(decls.get('color')).toBe('var(--kr-text-on-accent)');
  });
});

function readTokens(): string {
  return SOURCES.find(s => s.file === join('styles', 'tokens.css'))!.text;
}
