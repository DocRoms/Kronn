// KT-1887 — the stylesheet half of the off-scale guard (themeAudit.ts, run by
// `pnpm lint:theme`): font sizes, radii, colours and `@media` widths off the
// design scales. The values of the code as it was are frozen per file and
// kind in src/styles/themeAuditBaseline.json; one more fails, and a count
// that went down must be pruned.
import { describe, expect, it } from 'vitest';
import { spawnSync } from 'node:child_process';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { BREAKPOINTS } from '../designTokens';
import {
  auditRepo, BREAKPOINT_PX, checkBaseline, dropFallbacks, offScaleValues, type ThemeAuditBaseline,
} from '../themeAudit';

const FRONTEND = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const AUDIT = auditRepo(join(FRONTEND, 'src'));

const kinds = (css: string, file = 'components/X.css') => offScaleValues(file, css).map(f => [f.kind, f.line]);

describe('off-scale values in a stylesheet', () => {
  it('flags a font size, a radius, a colour and a breakpoint off the scales, on their lines', () => {
    const css = [
      '.a { font-size: 11px; }',
      '.b { border-radius: 6px; }',
      '.c { color: #e8eaed; }',
      '@media (max-width: 760px) { .d { display: none; } }',
      '.e {',
      '  padding: 4px;',
      '  border-top-left-radius: 999px;',
      '  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.25);',
      '}',
    ].join('\n');
    expect(kinds(css)).toEqual([
      ['font-size', 1], ['radius', 2], ['colour', 3], ['breakpoint', 4], ['radius', 7], ['colour', 8],
    ]);
  });

  it('leaves alone the tokens, their fallbacks, the scale breakpoints and colourless values', () => {
    const css = [
      '.a { font-size: var(--kr-type-sm); border-radius: var(--kr-radius-control) var(--kr-radius-control) 0 0; }',
      '.b { font-size: var(--kr-fs-base, 14px); border-radius: 50%; }',
      '.c { color: var(--kr-purple-soft); background: rgba(var(--kr-purple-rgb), 0.1); }',
      '.d { color: var(--kr-purple, rgb(var(--kr-purple-rgb))); border-color: transparent; }',
      '@media (max-width: 639px) { .e { font-size: inherit; } }',
      '@media (min-width: 900px) { .f { box-shadow: 0 0 0 0 rgba(0, 0, 0, 0); } }',
      '.g { animation: pulse-red 1s; background-image: url(#gradient); }',
    ].join('\n');
    expect(kinds(css)).toEqual([]);
  });

  it('reads no colour in the files that paint a theme', () => {
    expect(kinds('.a { color: #fff; }', 'styles/tokens.css')).toEqual([]);
    expect(kinds('.a { color: #fff; }', 'styles/neon.css')).toEqual([]);
    expect(kinds('.a { font-size: 9px; }', 'styles/neon.css')).toEqual([['font-size', 1]]);
  });

  it('drops nested fallbacks to the token they guard', () => {
    expect(dropFallbacks('var(--a, rgb(var(--b, 1 2 3))) 0 var(--c)')).toBe('var(--a) 0 var(--c)');
  });

  it('holds the breakpoints equal to BREAKPOINTS', () => {
    expect(BREAKPOINT_PX).toEqual(Object.values(BREAKPOINTS));
  });
});

describe('the off-scale baseline', () => {
  const findings = offScaleValues('components/X.css', '.a { font-size: 11px; }\n.b { font-size: 12px; }');
  const baseline = (count: number): ThemeAuditBaseline => ({ contrast: [], offScale: { 'components/X.css': { 'font-size': count } } });

  it('passes at the baseline, fails on one more, and asks to prune one fewer', () => {
    expect(checkBaseline(baseline(2), findings, [])).toEqual({ newOffScale: [], staleOffScale: [], newContrast: [], staleContrast: [] });
    expect(checkBaseline(baseline(1), findings, []).newOffScale.map(f => f.line)).toEqual([1, 2]);
    expect(checkBaseline(baseline(3), findings, []).staleOffScale).toEqual(['components/X.css font-size: 3 → 2']);
  });

  it('fails on a file the baseline does not know', () => {
    expect(checkBaseline({ contrast: [], offScale: {} }, findings, []).newOffScale).toHaveLength(2);
  });

  it('matches the tree today', () => {
    expect(AUDIT.offScale.length).toBeGreaterThan(0);
    expect(AUDIT.baseline.newOffScale).toEqual([]);
    expect(AUDIT.baseline.staleOffScale).toEqual([]);
  });

  it('prints the off-scale summary and the frozen cells (~), and exits 0 on the tree', () => {
    // The script reads the real tree: a value beyond the baseline is the
    // checkBaseline case above, counted as a failure by the script.
    const run = spawnSync(process.execPath, ['scripts/theme-audit.mjs', 'light'], { cwd: FRONTEND, encoding: 'utf8' });
    expect(run.status, run.stdout + run.stderr).toBe(0);
    expect(run.stdout).toContain('Off-scale values beyond the baseline');
    expect(run.stdout).toMatch(/~/);
  });
});
