// Guard against "phantom" CSS custom properties: a `var(--kr-…)` used somewhere
// in src/ that is never defined in tokens.css. An undefined var without a
// fallback makes the browser DROP the whole declaration → text turns black on a
// dark background, borders/backgrounds vanish, silently. The 2026-07 audit found
// ~20 of these; this test keeps the count at zero (see CHANGELOG 0.8.11).
import { describe, it, expect } from 'vitest';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', '..');

function walk(dir: string, exts: string[]): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules' || name === 'dist') continue;
    const p = join(dir, name);
    const s = statSync(p);
    if (s.isDirectory()) out.push(...walk(p, exts));
    else if (exts.some(e => name.endsWith(e))) out.push(p);
  }
  return out;
}

function definedVars(): Set<string> {
  const defined = new Set<string>();
  for (const f of walk(SRC, ['.css'])) {
    const txt = readFileSync(f, 'utf8');
    for (const m of txt.matchAll(/(--kr-[a-z0-9-]+)\s*:/g)) defined.add(m[1]);
  }
  return defined;
}

/** Every `var(--kr-…)` usage WITHOUT a fallback, with its file. */
function usagesWithoutFallback(): { name: string; file: string }[] {
  const out: { name: string; file: string }[] = [];
  for (const f of walk(SRC, ['.css', '.tsx', '.ts'])) {
    if (f.endsWith('tokens-defined.test.ts')) continue;
    const txt = readFileSync(f, 'utf8');
    // var(--kr-xxx) or var(--kr-xxx, fallback) — capture the name and whether a comma follows.
    for (const m of txt.matchAll(/var\(\s*(--kr-[a-z0-9-]+)\s*(,)?/g)) {
      if (!m[2]) out.push({ name: m[1], file: f.replace(SRC, 'src') });
    }
  }
  return out;
}

describe('CSS design tokens', () => {
  it('has no phantom --kr-* var used without a fallback and without a definition', () => {
    const defined = definedVars();
    const offenders = usagesWithoutFallback().filter(u => !defined.has(u.name));
    const unique = [...new Map(offenders.map(o => [`${o.name}@${o.file}`, o])).values()];
    expect(unique, `Undefined --kr-* vars (define in tokens.css or add a fallback):\n` +
      unique.map(o => `  ${o.name}  (${o.file})`).join('\n')).toEqual([]);
  });
});
