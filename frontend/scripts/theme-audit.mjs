#!/usr/bin/env node
/**
 * Theme audit — the static half of the WCAG AA pass on the design tokens.
 *
 * Prints, for every theme in `src/styles/tokens.css`, the contrast of each
 * `--kr-text-*` role on each opaque surface (`--kr-bg-base|raised|surface|
 * elevated|input`) — plus, in the dark themes, on a hover chip over elevated
 * and the status colours (error, purple, cancelled…) on the ramp — then lists:
 *   - `var(--kr-…)` names the base `:root` never declares (a fallback does
 *     not excuse one);
 *   - custom properties read with `var()` but declared nowhere;
 *   - rules pairing a `--kr-*` background with literal white/black text;
 *   - `:focus-visible` rules that drop the outline without a ring of their own.
 * The focus ring colour (`--kr-accent-ink`) is held to 3:1 on the ramp.
 *
 * Usage (from `frontend/`, Node >= 23.6 for the TypeScript import):
 *   node scripts/theme-audit.mjs            # every theme
 *   node scripts/theme-audit.mjs gotham     # one or more themes
 *   pnpm run lint:theme
 *
 * Floors: 4.5:1 for every role in the dark themes (all of them carry small
 * text there), 3:1 for faint/dim/ghost elsewhere. Exits non-zero when a cell
 * misses its floor or any finding is listed. The same checks gate vitest
 * (`src/styles/__tests__/themeContrast.test.ts`); the rendered check is
 * `e2e/specs/a11y-dark-themes.spec.ts`.
 */
/* global console, process */
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  auditRepo, FOCUS_RING_TOKEN, HOVER_ON_ELEVATED, STATUS_TOKENS, SURFACE_TOKENS, TEXT_TOKENS,
} from '../src/styles/themeAudit.ts';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', 'src');
const audit = auditRepo(SRC);
const wanted = process.argv.slice(2);
const themes = [...audit.themes.keys()].filter(t => wanted.length === 0 || wanted.includes(t));
let failures = 0;

const COLUMNS = [...SURFACE_TOKENS, HOVER_ON_ELEVATED];
const short = name => name === HOVER_ON_ELEVATED ? 'hover/elev'
  : name.replace('--kr-bg-', '').replace('--kr-text-', '').replace('--kr-', '');
for (const theme of themes) {
  console.log(`\n${theme}`);
  console.log(`  ${'role'.padEnd(18)} floor${COLUMNS.map(s => short(s).padStart(11)).join('')}`);
  for (const text of [...TEXT_TOKENS, ...STATUS_TOKENS, FOCUS_RING_TOKEN]) {
    const rows = audit.contrast.filter(r => r.theme === theme && r.text === text);
    if (rows.length === 0) continue;
    const cells = COLUMNS.map(bg => {
      const row = rows.find(r => r.background === bg);
      if (!row) return '-'.padStart(11);
      const bad = row.ratio < row.floor;
      if (bad) failures += 1;
      return `${row.ratio.toFixed(2)}${bad ? '!' : ' '}`.padStart(11);
    });
    console.log(`  ${short(text).padEnd(18)} ${rows[0].floor.toFixed(1).padStart(4)}${cells.join('')}`);
  }
}

const report = (title, findings) => {
  console.log(`\n${title}: ${findings.length}`);
  for (const f of findings) console.log(`  src/${f.file}:${f.line}  ${f.detail}`);
  failures += findings.length;
};
report('Undefined --kr-* tokens', audit.undefinedTokens);
report('Custom properties declared nowhere', audit.undeclaredProperties);
report('Literal white/black text on a --kr-* background', audit.forcedText);
report(':focus-visible rules that remove the ring', audit.focusRemovals);

console.log(failures === 0 ? '\nOK' : `\n${failures} problem(s) (cells marked ! miss their floor)`);
process.exit(failures === 0 ? 0 : 1);
