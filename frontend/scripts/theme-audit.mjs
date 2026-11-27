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
 * KT-1887: the status colours (`--kr-success`…, `--kr-status-*`) are measured
 * in every theme, light ones included, and the stylesheets are read for font
 * sizes, radii, colours and `@media` widths off the design scales. The debt
 * that existed when these checks came in is frozen in
 * `src/styles/themeAuditBaseline.json`: a contrast cell it lists is printed
 * with `~` and does not fail; a file may hold as many off-scale values of a
 * kind as it records, no more. A baseline entry that no longer occurs fails
 * too, so the file only shrinks: `--prune-baseline` lowers it to the tree
 * (it never raises a count or adds a cell).
 *
 * Usage (from `frontend/`, Node >= 23.6 for the TypeScript import):
 *   node scripts/theme-audit.mjs            # every theme
 *   node scripts/theme-audit.mjs gotham     # one or more themes
 *   pnpm run lint:theme
 *   node scripts/theme-audit.mjs --prune-baseline   # after fixing baselined debt
 *
 * Floors: 4.5:1 for every role in the dark themes (all of them carry small
 * text there), 3:1 for faint/dim/ghost elsewhere. Exits non-zero when a cell
 * misses its floor or any finding is listed. The same checks gate vitest
 * (`src/styles/__tests__/themeContrast.test.ts`); the rendered check is
 * `e2e/specs/a11y-dark-themes.spec.ts`.
 */
/* global console, process */
import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  auditRepo, BASELINE_FILE, contrastKey, countOffScale, FOCUS_RING_TOKEN, HOVER_ON_ELEVATED, loadBaseline,
  OFF_SCALE_KINDS, SEMANTIC_STATUS_TOKENS, STATUS_TOKENS, SURFACE_TOKENS, TEXT_TOKENS,
} from '../src/styles/themeAudit.ts';

const SRC = join(dirname(fileURLToPath(import.meta.url)), '..', 'src');
const audit = auditRepo(SRC);
const args = process.argv.slice(2);
const prune = args.includes('--prune-baseline');
const wanted = args.filter(a => !a.startsWith('--'));
const frozen = new Set(loadBaseline(SRC).contrast);
const themes = [...audit.themes.keys()].filter(t => wanted.length === 0 || wanted.includes(t));
let failures = 0;

const COLUMNS = [...SURFACE_TOKENS, HOVER_ON_ELEVATED];
const short = name => name === HOVER_ON_ELEVATED ? 'hover/elev'
  : name.replace('--kr-bg-', '').replace('--kr-text-', '').replace('--kr-', '');
for (const theme of themes) {
  console.log(`\n${theme}`);
  console.log(`  ${'role'.padEnd(18)} floor${COLUMNS.map(s => short(s).padStart(11)).join('')}`);
  for (const text of [...TEXT_TOKENS, ...STATUS_TOKENS, ...SEMANTIC_STATUS_TOKENS, FOCUS_RING_TOKEN]) {
    const rows = audit.contrast.filter(r => r.theme === theme && r.text === text);
    if (rows.length === 0) continue;
    const cells = COLUMNS.map(bg => {
      const row = rows.find(r => r.background === bg);
      if (!row) return '-'.padStart(11);
      const under = row.ratio < row.floor;
      const baselined = under && frozen.has(contrastKey(row));
      if (under && !baselined) failures += 1;
      return `${row.ratio.toFixed(2)}${under ? (baselined ? '~' : '!') : ' '}`.padStart(11);
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

// KT-1887 — off-scale values beyond the baseline, and a baseline to prune.
const { baseline } = audit;
report('Off-scale values beyond the baseline (font-size, radius, colour, breakpoint; see docs/conventions/ui-lint-guards.md)',
  baseline.newOffScale.map(f => ({ ...f, detail: `[${f.kind}] ${f.detail}` })));
const total = audit.offScale.length;
console.log(`  (${total} off-scale value(s) in the stylesheets, ${total - baseline.newOffScale.length} frozen in src/${BASELINE_FILE})`);
const stale = [...baseline.staleOffScale, ...baseline.staleContrast.map(k => `contrast ${k}`)];
if (prune) {
  const current = loadBaseline(SRC);
  const counts = countOffScale(audit.offScale);
  const offScale = {};
  for (const file of Object.keys(current.offScale).sort()) {
    for (const kind of OFF_SCALE_KINDS) {
      const kept = Math.min(current.offScale[file][kind] ?? 0, counts[file]?.[kind] ?? 0);
      if (kept > 0) (offScale[file] ??= {})[kind] = kept;
    }
  }
  const misses = new Set(audit.contrast.filter(r => r.ratio < r.floor).map(contrastKey));
  const contrast = current.contrast.filter(k => misses.has(k));
  writeFileSync(join(SRC, BASELINE_FILE), `${JSON.stringify({ contrast, offScale }, null, 2)}\n`);
  console.log(`\nBaseline pruned: ${stale.length} entr${stale.length === 1 ? 'y' : 'ies'} lowered or removed.`);
} else {
  console.log(`\nBaseline entries that no longer occur (run with --prune-baseline): ${stale.length}`);
  for (const s of stale) console.log(`  ${s}`);
  failures += stale.length;
}

console.log(failures === 0 ? '\nOK' : `\n${failures} problem(s) (cells marked ! miss their floor)`);
process.exit(failures === 0 ? 0 : 1);
