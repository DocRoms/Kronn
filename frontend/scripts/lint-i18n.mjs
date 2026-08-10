#!/usr/bin/env node
/**
 * i18n key lint — verifies translation parity across every shipped UI locale.
 *
 * Catches three classes of bugs:
 *
 *   1. Key in `t('foo')` but missing from `fr` → runtime fallback to the
 *      key itself, broken UX (sees "qp.compareAgents.button" instead of
 *      "Comparer sur 7 agents installés").
 *   2. Key in `fr` but missing from another locale → users of that locale
 *      see the key string instead of a translation.
 *   3. Key declared in a locale but unused anywhere in the source → dead
 *      weight, gets imported into the bundle. Reported as a warning, not
 *      an error (some keys are used dynamically via `t(\`prefix.${kind}\`)`).
 *
 * Usage:
 *   node frontend/scripts/lint-i18n.mjs
 *   pnpm run lint:i18n
 *
 * Exits non-zero on missing keys; warnings only for unused keys.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(fileURLToPath(import.meta.url), '..', '..');
const SRC = join(ROOT, 'src');
const I18N_DIR = join(SRC, 'lib', 'i18n', 'locales');
const LOCALES = ['fr', 'en', 'es', 'zh'];

// ── 1. Parse locale dictionaries ──────────────────────────────────────────
// Each locale is a `const fr: TranslationDict = { ... };` block. We walk
// its dedicated lazy module and slice out the block by its declaration line +
// the matching closing `};` at column 0.
function extractLocaleBlock(name) {
  const i18nText = readFileSync(join(I18N_DIR, `${name}.ts`), 'utf-8');
  const startMatch = i18nText.match(new RegExp(`^const ${name}: TranslationDict = \\{`, 'm'));
  if (!startMatch) throw new Error(`Locale '${name}' not found`);
  const startIdx = startMatch.index + startMatch[0].length;
  // Find the matching `};` line in the original text (line at column 0).
  // The dict bodies are flat key:value entries — there's no nested `}`
  // outside string literals — so the simplest correct scan is "first `};\n`
  // at column 0 after startIdx". This sidesteps the brittle depth-counting
  // approach which was tripped by `}` characters inside translation
  // strings (e.g. an `{0}` placeholder followed by some other char).
  const tail = i18nText.slice(startIdx);
  const endMatch = tail.match(/\n};\n/);
  if (!endMatch) throw new Error(`Locale '${name}' block has no closing brace`);
  return tail.slice(0, endMatch.index);
}

function keysIn(block) {
  // `'key.name':` or `"key.name":` at line start (with leading whitespace tolerated).
  // Translation keys are namespaced (and therefore contain a dot). Requiring
  // that dot avoids mistaking JSON examples embedded in multiline prompts for
  // dictionary entries.
  const keys = new Set();
  const re = /^[\t ]+(['"])([^'"\n]*\.[^'"\n]*)\1:/gm;
  let m;
  while ((m = re.exec(block))) keys.add(m[2]);
  return keys;
}

const localeBlocks = Object.fromEntries(LOCALES.map(locale => [locale, extractLocaleBlock(locale)]));
const localeKeys = Object.fromEntries(LOCALES.map(locale => [locale, keysIn(localeBlocks[locale])]));

// ── 2. Parity check ──────────────────────────────────────────────────────
// FR is the reference (most complete historically). Every key in FR must
// also exist in every other shipped locale.
const errors = [];
const ref = localeKeys.fr;
for (const lang of LOCALES.filter(locale => locale !== 'fr')) {
  const set = localeKeys[lang];
  for (const k of ref) {
    if (!set.has(k)) errors.push(`[parity] '${k}' present in fr but missing from ${lang}`);
  }
  // Reverse direction — keys defined in en/es but not in fr are also
  // suspect (typo? old key never deleted from fr but added to en?).
  for (const k of set) {
    if (!ref.has(k)) errors.push(`[parity] '${k}' present in ${lang} but missing from fr`);
  }
}

// ── 3. Source usage scan ─────────────────────────────────────────────────
// Walk src/ and collect every `t('key.name')` literal. Dynamic keys
// (`t(\`prefix.${x}\`)` and `t(variable)`) are skipped — too brittle to
// guess at lint time. We log them so a human can review.
const FILE_EXT = /\.(ts|tsx|js|jsx)$/;
const SKIP_DIRS = new Set(['node_modules', 'dist', 'build', 'coverage', '.vite', '__tests__', 'test-results']);

function* walk(dir) {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const st = statSync(full);
    if (st.isDirectory()) {
      if (SKIP_DIRS.has(entry)) continue;
      yield* walk(full);
    } else if (FILE_EXT.test(entry)) {
      yield full;
    }
  }
}

const usedKeys = new Map(); // key → first file:line where it was used
let dynamicCount = 0;
const T_LITERAL = /\bt\(\s*['"]([\w.\-:]+)['"]/g;
const T_TEMPLATE = /\bt\(\s*`/g;
const T_VAR = /\bt\(\s*[a-zA-Z_$][\w$]*\s*[,)]/g;
for (const file of walk(SRC)) {
  if (file.startsWith(I18N_DIR)) continue; // don't self-scan dictionaries
  const text = readFileSync(file, 'utf-8');
  let m;
  T_LITERAL.lastIndex = 0;
  while ((m = T_LITERAL.exec(text))) {
    const key = m[1];
    if (!usedKeys.has(key)) {
      const line = text.slice(0, m.index).split('\n').length;
      usedKeys.set(key, `${relative(ROOT, file)}:${line}`);
    }
  }
  // Count dynamic usages once per file (rough proxy for "key set is broader than what we statically see").
  T_TEMPLATE.lastIndex = 0;
  T_VAR.lastIndex = 0;
  if (T_TEMPLATE.test(text) || T_VAR.test(text)) dynamicCount++;
}

// Used keys must exist in fr.
for (const [key, where] of usedKeys) {
  if (!ref.has(key)) errors.push(`[missing] '${key}' used at ${where} but not defined in any locale`);
}

// Unused keys (warn only — dynamic usages aren't tracked).
const warnings = [];
for (const k of ref) {
  if (!usedKeys.has(k)) {
    // Skip a few well-known prefixes that are routinely used dynamically
    // (kept defensively as "open" categories — we'd rather not nag).
    if (/^(agent\.[a-z_]+\.|tour\.step\.\d|nav\.\w+|wf\.step\.|qa\.[a-z_]+\.)/i.test(k)) continue;
    warnings.push(`[unused?] '${k}' defined in fr but not found via static t('…') scan`);
  }
}

// ── 4. Report ────────────────────────────────────────────────────────────
const totalKeys = ref.size;
console.log(`i18n lint — ${LOCALES.map(locale => `${locale}=${localeKeys[locale].size}`).join(', ')}`);
console.log(`Source: ${usedKeys.size} static t('…') usages across ${SRC} (+${dynamicCount} files with dynamic keys)`);

for (const e of errors) console.error(e);
for (const w of warnings.slice(0, 50)) console.warn(w);
if (warnings.length > 50) {
  console.warn(`… and ${warnings.length - 50} more unused warnings (truncated)`);
}

if (errors.length) {
  console.error(`\n✗ ${errors.length} hard error(s).`);
  process.exit(1);
}
if (warnings.length) {
  console.warn(`\n⚠ ${warnings.length} warning(s) — review for dead keys.`);
}
console.log(`\n✓ ${totalKeys} keys localised in ${LOCALES.length} languages.`);
