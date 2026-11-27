/**
 * UI text check (KT-1879) — the glossary and the French register.
 *
 * Reads `src/lib/i18n/glossary.json` (one term per concept and per locale, the
 * synonyms to avoid, the register) and reports every label of the four locale
 * files that uses a synonym, or a « tu » form in French.
 *
 * Existing labels are not rewritten by this check: `terms-baseline.json`
 * records each label that already breaks a rule (hash of its text + the rules
 * it breaks) with the count per cluster. The check fails only on:
 *   - new          a label absent from the baseline that breaks a rule;
 *   - touched      a recorded label whose text changed and still breaks a rule;
 *   - new-violation a recorded, unchanged label that breaks a rule the baseline
 *                  does not list (the glossary grew).
 *
 * Usage (no dependency; Node >= 23.6 strips the locale files' types):
 *   node frontend/scripts/i18n-terms.mjs                     check
 *   node frontend/scripts/i18n-terms.mjs --update-baseline   drop the labels that were fixed
 *   node frontend/scripts/i18n-terms.mjs --update-baseline --accept-new
 *                                                            record every current violation
 *                                                            (only after a glossary change)
 * The same check runs in vitest: src/lib/__tests__/i18n-terms.test.ts.
 * Rules: docs/conventions/ui-text.md
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const LOCALES = ['en', 'fr', 'es', 'zh'];

const GLOSSARY_PATH = 'frontend/src/lib/i18n/glossary.json';
const BASELINE_PATH = 'frontend/src/lib/i18n/terms-baseline.json';
const DOC_PATH = 'docs/conventions/ui-text.md';

// ── Text preparation ─────────────────────────────────────────────────────
// Code, templates and URLs are not prose: a `git worktree` command or a
// {{state.run}} variable is not a word choice.
function stripNonProse(text) {
  return text
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/`[^`\n]*`/g, ' ')
    .replace(/\{\{[^}]*\}\}/g, ' ')
    .replace(/https?:\/\/\S+/g, ' ');
}

/** FNV-1a, 32 bits: enough to tell whether a label's text changed. */
export function hashLabel(text) {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h.toString(16).padStart(8, '0');
}

// ── Patterns ─────────────────────────────────────────────────────────────
// A synonym is a whole word: a letter, digit, '_', '-', '/' or '.' next to it
// makes it part of a compound, a path or an identifier. zh has no spaces
// between words, so only Latin characters count there.
function synonymRegex(locale, source) {
  const near = locale === 'zh' ? 'A-Za-z0-9_\\-/' : '\\p{L}\\p{N}_\\-/';
  return new RegExp(`(?<![${near}.])(?:${source})(?![${near}])(?!\\.[${near}])`, 'giu');
}

// The register rule keeps « veux-tu » in view: only letters and digits join.
function registerRegex(source) {
  return new RegExp(`(?<![\\p{L}\\p{N}_])(?:${source})(?![\\p{L}\\p{N}_])`, 'giu');
}

function allowedRanges(text, allow) {
  const ranges = [];
  for (const phrase of allow ?? []) {
    if (!phrase) continue;
    let from = 0;
    for (;;) {
      const at = text.indexOf(phrase, from);
      if (at < 0) break;
      ranges.push([at, at + phrase.length]);
      from = at + 1;
    }
  }
  return ranges;
}

function matchesOutside(re, text, ranges) {
  re.lastIndex = 0;
  let m;
  while ((m = re.exec(text))) {
    const start = m.index;
    const end = start + m[0].length;
    if (m[0].length === 0) re.lastIndex++;
    if (!ranges.some(([a, b]) => start < b && end > a)) return true;
  }
  return false;
}

// ── Glossary ─────────────────────────────────────────────────────────────
export function compileGlossary(glossary) {
  const rules = Object.fromEntries(LOCALES.map(locale => [locale, []]));
  for (const concept of glossary.concepts ?? []) {
    for (const locale of LOCALES) {
      for (const source of concept.synonyms?.[locale] ?? []) {
        rules[locale].push({
          id: `term:${concept.id}:${source}`,
          cluster: concept.id,
          source,
          re: synonymRegex(locale, source),
          allow: concept.allow?.[locale] ?? [],
        });
      }
    }
  }
  for (const [locale, register] of Object.entries(glossary.register ?? {})) {
    for (const source of register.forbidden ?? []) {
      rules[locale]?.push({
        id: `register:${locale}:${source}`,
        cluster: `register:${locale}`,
        source,
        re: registerRegex(source),
        allow: register.allow ?? [],
      });
    }
  }
  return rules;
}

/** Rule ids broken by one label, in glossary order. */
export function findViolations(compiled, locale, text) {
  const prose = stripNonProse(text);
  const found = [];
  for (const rule of compiled[locale] ?? []) {
    if (matchesOutside(rule.re, prose, allowedRanges(prose, rule.allow))) found.push(rule.id);
  }
  return found;
}

/** Structural errors of the glossary file (empty when it is valid). */
export function validateGlossary(glossary) {
  const errors = [];
  const ids = new Set();
  const compile = (what, build) => {
    try {
      return build();
    } catch (e) {
      errors.push(`${what} does not compile: ${e.message}`);
      return null;
    }
  };
  const synonyms = Object.fromEntries(LOCALES.map(locale => [locale, []]));
  for (const concept of glossary.concepts ?? []) {
    if (!concept.id) errors.push('a concept has no id');
    if (ids.has(concept.id)) errors.push(`concept '${concept.id}' is declared twice`);
    ids.add(concept.id);
    for (const locale of LOCALES) {
      if (!concept.terms?.[locale]) errors.push(`concept '${concept.id}': no term for ${locale}`);
      for (const source of concept.synonyms?.[locale] ?? []) {
        const re = compile(`concept '${concept.id}': ${locale} synonym '${source}'`, () => synonymRegex(locale, source));
        if (re) synonyms[locale].push({ concept: concept.id, source, re });
      }
    }
  }
  for (const [locale, register] of Object.entries(glossary.register ?? {})) {
    if (!LOCALES.includes(locale)) errors.push(`register: unknown locale ${locale}`);
    if (!register.use) errors.push(`register ${locale}: no 'use'`);
    for (const source of register.forbidden ?? []) {
      compile(`register ${locale}: '${source}'`, () => registerRegex(source));
    }
  }
  // A glossary term must never be reported, in its own concept or another.
  for (const locale of LOCALES) {
    const seen = new Map();
    for (const concept of glossary.concepts ?? []) {
      const term = concept.terms?.[locale];
      if (!term) continue;
      if (seen.has(term)) errors.push(`${locale} term '${term}' names both '${seen.get(term)}' and '${concept.id}'`);
      seen.set(term, concept.id);
      for (const syn of synonyms[locale]) {
        if (!matchesOutside(syn.re, term, [])) continue;
        errors.push(syn.concept === concept.id
          ? `concept '${concept.id}': ${locale} term '${term}' matches its own synonym '${syn.source}'`
          : `concept '${concept.id}': ${locale} term '${term}' matches the synonym '${syn.source}' of '${syn.concept}'`);
      }
    }
  }
  return errors;
}

// ── Baseline ─────────────────────────────────────────────────────────────
function countClusters(labels, compiled) {
  const counts = {};
  for (const locale of LOCALES) {
    const byId = new Map((compiled[locale] ?? []).map(rule => [rule.id, rule]));
    const perLocale = {};
    for (const entry of Object.values(labels[locale] ?? {})) {
      for (const id of entry.slice(1)) {
        const rule = byId.get(id);
        if (!rule) continue;
        perLocale[rule.cluster] ??= {};
        perLocale[rule.cluster][rule.source] = (perLocale[rule.cluster][rule.source] ?? 0) + 1;
      }
    }
    counts[locale] = sortObject(Object.fromEntries(
      Object.entries(perLocale).map(([cluster, bySource]) => [cluster, sortObject(bySource)]),
    ));
  }
  return counts;
}

function sortObject(obj) {
  return Object.fromEntries(Object.entries(obj).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
}

/**
 * Baseline of the current labels. Without `acceptNew`, a label is kept only
 * if `previous` recorded it with the same text, and only with the violations
 * `previous` listed: a refresh drops what was fixed and never hides new work.
 */
export function buildBaseline({ dictionaries, glossary, previous, acceptNew = false }) {
  const compiled = compileGlossary(glossary);
  const labels = {};
  for (const locale of LOCALES) {
    const entries = {};
    for (const [key, text] of Object.entries(dictionaries[locale] ?? {})) {
      const hash = hashLabel(text);
      let found = findViolations(compiled, locale, text);
      if (!acceptNew) {
        const before = previous?.labels?.[locale]?.[key];
        found = before && before[0] === hash ? found.filter(id => before.includes(id)) : [];
      }
      if (found.length) entries[key] = [hash, ...found];
    }
    labels[locale] = sortObject(entries);
  }
  return {
    version: 1,
    about: `Labels that broke a rule of glossary.json when the check was introduced or the glossary last grew. Refresh with node frontend/scripts/i18n-terms.mjs --update-baseline. See ${DOC_PATH}.`,
    counts: countClusters(labels, compiled),
    labels,
  };
}

export function checkTerms({ dictionaries, glossary, baseline }) {
  const compiled = compileGlossary(glossary);
  const failures = [];
  const resolved = [];
  for (const locale of LOCALES) {
    const dict = dictionaries[locale] ?? {};
    const recorded = baseline?.labels?.[locale] ?? {};
    for (const [key, text] of Object.entries(dict)) {
      const found = findViolations(compiled, locale, text);
      if (!found.length) continue;
      const before = recorded[key];
      if (!before) {
        failures.push({ locale, key, reason: 'new', violations: found });
      } else if (before[0] !== hashLabel(text)) {
        failures.push({ locale, key, reason: 'touched', violations: found });
      } else if (found.some(id => !before.includes(id))) {
        failures.push({ locale, key, reason: 'new-violation', violations: found });
      }
    }
    for (const key of Object.keys(recorded)) {
      if (!(key in dict) || findViolations(compiled, locale, dict[key]).length === 0) {
        resolved.push(`${locale} ${key}`);
      }
    }
  }
  return { failures, resolved, counts: baseline?.counts ?? {} };
}

/** JSON with one label per line, so a review diff shows each label that changed. */
export function serializeBaseline(baseline) {
  const lines = ['{', `  "version": ${JSON.stringify(baseline.version)},`];
  if (baseline.about) lines.push(`  "about": ${JSON.stringify(baseline.about)},`);
  const counts = JSON.stringify(baseline.counts, null, 2).split('\n').join('\n  ');
  lines.push(`  "counts": ${counts},`, '  "labels": {');
  LOCALES.forEach((locale, i) => {
    const entries = Object.entries(baseline.labels[locale] ?? {});
    if (!entries.length) {
      lines.push(`    ${JSON.stringify(locale)}: {}${i < LOCALES.length - 1 ? ',' : ''}`);
      return;
    }
    lines.push(`    ${JSON.stringify(locale)}: {`);
    entries.forEach(([key, entry], j) => {
      const items = entry.map(item => JSON.stringify(item)).join(',');
      lines.push(`      ${JSON.stringify(key)}: [${items}]${j < entries.length - 1 ? ',' : ''}`);
    });
    lines.push(`    }${i < LOCALES.length - 1 ? ',' : ''}`);
  });
  lines.push('  }', '}', '');
  return lines.join('\n');
}

export function formatReport({ failures, resolved }) {
  const out = [];
  for (const f of failures) out.push(`${f.locale} '${f.key}' (${f.reason}): ${f.violations.join(', ')}`);
  if (failures.length) {
    out.push(
      '',
      `${failures.length} label(s) use a synonym of a term of ${GLOSSARY_PATH} or « tu » in French.`,
      'Use the glossary term (or « vous »). If the word means something else in this label, add the exact phrase',
      `to the concept's "allow" list. Rules: ${DOC_PATH}`,
    );
  }
  if (resolved.length) {
    out.push('', `${resolved.length} recorded label(s) no longer break a rule: run node frontend/scripts/i18n-terms.mjs --update-baseline to drop them from ${BASELINE_PATH}.`);
  }
  return out.join('\n');
}

// ── CLI ──────────────────────────────────────────────────────────────────
async function loadDictionaries(root) {
  const dictionaries = {};
  for (const locale of LOCALES) {
    const url = pathToFileURL(join(root, 'src', 'lib', 'i18n', 'locales', `${locale}.ts`)).href;
    dictionaries[locale] = (await import(url)).default;
  }
  return dictionaries;
}

async function main(argv) {
  const root = join(fileURLToPath(import.meta.url), '..', '..');
  const glossaryFile = join(root, 'src', 'lib', 'i18n', 'glossary.json');
  const baselineFile = join(root, 'src', 'lib', 'i18n', 'terms-baseline.json');
  const glossary = JSON.parse(readFileSync(glossaryFile, 'utf-8'));
  const errors = validateGlossary(glossary);
  if (errors.length) {
    for (const e of errors) console.error(`[glossary] ${e}`);
    return 1;
  }
  const dictionaries = await loadDictionaries(root);
  let previous;
  try {
    previous = JSON.parse(readFileSync(baselineFile, 'utf-8'));
  } catch {
    previous = undefined;
  }

  if (argv.includes('--update-baseline')) {
    const next = buildBaseline({ dictionaries, glossary, previous, acceptNew: argv.includes('--accept-new') });
    writeFileSync(baselineFile, serializeBaseline(next));
    const total = LOCALES.reduce((n, locale) => n + Object.keys(next.labels[locale]).length, 0);
    console.log(`Wrote ${BASELINE_PATH}: ${total} recorded label(s).`);
    for (const locale of LOCALES) console.log(`  ${locale}: ${JSON.stringify(next.counts[locale])}`);
    return 0;
  }

  const report = checkTerms({ dictionaries, glossary, baseline: previous });
  const text = formatReport(report);
  if (report.failures.length) {
    console.error(text);
    return 1;
  }
  if (text) console.warn(text);
  console.log(`✓ No new synonym or « tu » form (${glossary.concepts.length} concepts, ${LOCALES.length} locales).`);
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await main(process.argv.slice(2));
}
