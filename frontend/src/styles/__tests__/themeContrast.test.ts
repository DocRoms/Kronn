// WCAG AA of the theme tokens, measured from the real tokens.css so a theme
// added or retuned later is held to the same floors. The rendered counterpart
// is e2e/specs/a11y-dark-themes.spec.ts (axe on the main screens).
import { describe, it, expect } from 'vitest';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  auditRepo, focusRingRemovals, forcedTextInInlineStyles, forcedTextOnTokenBackground, measureTextContrast,
  parseThemes, STATUS_TOKENS, TEXT_TOKENS, undeclaredCustomProperties,
} from '../themeAudit';

const FRONTEND = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const SRC = join(FRONTEND, 'src');
const AUDIT = auditRepo(SRC);
const TOKENS = readFileSync(join(SRC, 'styles', 'tokens.css'), 'utf8');

const lines = (findings: Array<{ file: string; line: number; detail: string }>) =>
  findings.map(f => `  src/${f.file}:${f.line}  ${f.detail}`).join('\n');

describe('theme contrast', () => {
  it('measures every role of every theme, so the floor check is not vacuous', () => {
    const themes = [...new Set(AUDIT.contrast.map(r => r.theme))].sort();
    expect(themes).toEqual(['dark', 'euronews', 'gotham', 'light', 'matrix', 'sakura']);
    for (const theme of themes) {
      for (const role of TEXT_TOKENS) {
        expect(AUDIT.contrast.some(r => r.theme === theme && r.text === role), `${theme} ${role}`).toBe(true);
      }
    }
    for (const theme of ['dark', 'gotham', 'matrix']) {
      for (const role of STATUS_TOKENS) {
        expect(AUDIT.contrast.some(r => r.theme === theme && r.text === role), `${theme} ${role}`).toBe(true);
      }
    }
  });

  it('keeps every text role and dark-theme status colour above its WCAG floor', () => {
    const misses = AUDIT.contrast.filter(r => r.ratio < r.floor)
      .map(r => `  ${r.theme} ${r.text} on ${r.background}: ${r.ratio.toFixed(2)} < ${r.floor}`);
    expect(misses, `Tokens under their floor (node scripts/theme-audit.mjs):\n${misses.join('\n')}`)
      .toEqual([]);
  });

  it('asks 4.5:1 of every role in a dark theme and 3:1 of ghost in a light one', () => {
    const floor = (theme: string, text: string) =>
      AUDIT.contrast.find(r => r.theme === theme && r.text === text)?.floor;
    expect(floor('gotham', '--kr-text-ghost')).toBe(4.5);
    expect(floor('matrix', '--kr-text-faint')).toBe(4.5);
    expect(floor('sakura', '--kr-text-ghost')).toBe(3);
    expect(floor('euronews', '--kr-text-muted')).toBe(4.5);
  });

  it('would have refused the gotham and matrix values it replaced', () => {
    const before = TOKENS
      .replace(/(\[data-theme="gotham"\][\s\S]*?--kr-text-muted:)[^;]+;/, '$1 rgba(232, 235, 240, 0.48);')
      .replace(/(\[data-theme="matrix"\][\s\S]*?--kr-text-ghost:)[^;]+;/, '$1 rgba(57, 255, 20, 0.30);');
    const rows = measureTextContrast(parseThemes(before));
    const at = (theme: string, text: string, bg: string) =>
      rows.find(r => r.theme === theme && r.text === text && r.background === bg)!.ratio;
    expect(at('gotham', '--kr-text-muted', '--kr-bg-elevated')).toBeCloseTo(4.02, 2);
    expect(at('matrix', '--kr-text-ghost', '--kr-bg-base')).toBeCloseTo(2.15, 2);
  });

  it('holds the focus ring colour to 3:1 on every surface of every theme', () => {
    const rings = AUDIT.contrast.filter(r => r.text === '--kr-accent-ink');
    expect(rings.length).toBeGreaterThanOrEqual(6 * 5);
    expect(rings.every(r => r.floor === 3 && r.ratio >= 3)).toBe(true);
  });
});

describe('theme guards', () => {
  it('finds no custom property read but declared nowhere', () => {
    expect(AUDIT.undeclaredProperties, lines(AUDIT.undeclaredProperties)).toEqual([]);
  });

  it('flags an undeclared property and accepts inline style declarations', () => {
    const sources = [
      { file: 'a.css', text: '.a { border-radius: var(--radius-sm); width: var(--local-w); }' },
      { file: 'a.tsx', text: "<div style={{ ['--local-w' as string]: '4px', '--tone': c }} />" },
      { file: 'b.css', text: '.b { color: var(--tone); }' },
    ];
    expect(undeclaredCustomProperties(sources).map(f => f.detail)).toEqual(['--radius-sm']);
  });

  it('finds no literal white or black text on a token background', () => {
    expect(AUDIT.forcedText, lines(AUDIT.forcedText)).toEqual([]);
  });

  it('flags white or black text on any --kr-* fill, not on a literal one', () => {
    const css = [
      '.a { background: var(--kr-error); color: #fff; }',
      '.b { background-color: var(--kr-bg-code); color: black; }',
      '.c { background: var(--kr-error); color: var(--kr-on-error); }',
      '.video { background: rgba(0, 0, 0, 0.6); color: #fff; }',
    ].join('\n');
    expect(forcedTextOnTokenBackground('x.css', css).map(f => f.line)).toEqual([1, 2]);
  });

  it('flags the same pairing in an inline style object', () => {
    const tsx = [
      "<div style={{ background: 'var(--kr-accent, #3b82f6)', color: '#fff' }} />",
      '<div style={{ backgroundColor: "var(--kr-bg-code)", padding: 4, color: "black" }} />',
      "<div style={{ background: 'var(--kr-accent)', color: 'var(--kr-text-on-accent)' }} />",
      "<div style={{ background: 'rgba(0, 0, 0, 0.6)', color: '#fff' }} />",
      "<div style={{ borderColor: 'var(--kr-accent)', color: `${tone}` }} />",
    ].join('\n');
    expect(forcedTextInInlineStyles('x.tsx', tsx).map(f => f.line)).toEqual([1, 2]);
  });

  it('finds no :focus-visible rule that removes the ring', () => {
    expect(AUDIT.focusRemovals, lines(AUDIT.focusRemovals)).toEqual([]);
  });

  it('flags a focus-visible outline removal unless a ring replaces it', () => {
    const css = [
      '.a:hover, .a:focus-visible { background: var(--kr-bg-hover); outline: none; }',
      '.b:focus-visible { outline: 0; box-shadow: 0 0 0 2px var(--kr-accent); }',
      '.field:focus { outline: none; border-color: var(--kr-accent); }',
    ].join('\n');
    expect(focusRingRemovals('x.css', css).map(f => f.line)).toEqual([1]);
  });

  it('runs as a script that exits 0 on the current tree', () => {
    const run = spawnSync(process.execPath, ['scripts/theme-audit.mjs', 'gotham'], {
      cwd: FRONTEND, encoding: 'utf8',
    });
    expect(run.status, run.stdout + run.stderr).toBe(0);
    expect(run.stdout).toContain('gotham');
    expect(run.stdout.trim().endsWith('OK')).toBe(true);
  });
});
