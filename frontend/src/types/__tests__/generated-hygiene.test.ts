// C1 (plan 0.9) — hygiene contract on the AUTO-GENERATED aggregate. The CI
// diff guard proves generated.ts matches the Rust models; these invariants
// prove the assembler's OUTPUT stays clean (they fired on real regressions:
// trailing whitespace from ts-rs doc-comment fields, duplicate declarations
// when a binding is exported under two names).
import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const src = readFileSync(join(HERE, '..', 'generated.ts'), 'utf8');

describe('generated.ts hygiene', () => {
  it('has no trailing whitespace', () => {
    const offenders = src
      .split('\n')
      .map((l, i) => (/[ \t]$/.test(l) ? i + 1 : 0))
      .filter(Boolean);
    expect(offenders, `lines with trailing whitespace: ${offenders.slice(0, 10).join(', ')}…`).toEqual([]);
  });

  it('declares each type exactly once', () => {
    const names = [...src.matchAll(/^export (?:type|interface|enum) (\w+)/gm)].map(m => m[1]);
    const dupes = names.filter((n, i) => names.indexOf(n) !== i);
    expect(dupes).toEqual([]);
  });

  it('carries the auto-generated banner (manual edits are forbidden)', () => {
    expect(src.startsWith('// ╔')).toBe(true);
    expect(src).toContain('AUTO-GENERATED');
  });
});
