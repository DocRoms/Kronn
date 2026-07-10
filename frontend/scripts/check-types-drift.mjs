#!/usr/bin/env node
// C8 (0.8.11) — FIELD-level anti-drift guard for the hand-curated
// `src/types/generated.ts`.
//
// `generated.ts` is (still) hand-maintained with deliberate frontend-friendly
// simplifications (u64→number, serde-default→optional, JsonValue widening); a
// faithful ts-rs regen is a separate migration (see assemble-generated-types.mjs
// + TD-20260701). Many Rust types are backend-only and INTENTIONALLY absent from
// the aggregate, so a missing-whole-type check is all false positives.
//
// The REAL recurring pain (hit 4× in 0.8.x) is: a Rust type the frontend DOES
// use gains a field, and generated.ts is never updated → the field is silently
// invisible to the UI (parent_run_id, on_timeout, Interrupted…). So this guard,
// for every type that generated.ts ALREADY declares, compares its field NAMES
// against the ts-rs binding and fails if the binding has fields the aggregate is
// missing. Field NAMES only (not types) — so the deliberate bigint/optional
// simplifications never trip it. Tagged unions are compared too: the union of
// field names across ALL top-level `{ … }` variants on each side (so drift in
// a non-first variant is caught). Pure aliases (no object body) are skipped —
// and reported, so "OK" can't silently mean "compared nothing".
// Run after `cargo test export_bindings`.
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { join, basename, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const BINDINGS = join(HERE, '..', '..', 'backend', 'bindings');
const GENERATED = join(HERE, '..', 'src', 'types', 'generated.ts');

if (!existsSync(BINDINGS)) {
  console.error(`[types-drift] bindings dir missing — run: (cd backend && cargo test export_bindings)`);
  process.exit(2);
}

function walk(dir) {
  const out = [];
  for (const n of readdirSync(dir)) {
    const p = join(dir, n);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else if (n.endsWith('.ts')) out.push(p);
  }
  return out;
}

/** Strip `/* … *​/` block comments + `// …` line comments so field scans don't
 *  pick up doc-comment prose. */
function stripComments(s) {
  return s.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
}

/** Skip a string literal starting at src[i] (a quote char); returns the index
 *  of the closing quote (or end of src). Keeps braces/pipes inside strings
 *  from confusing the depth trackers below. */
function skipString(src, i) {
  const q = src[i];
  for (i++; i < src.length; i++) {
    if (src[i] === '\\') i++;
    else if (src[i] === q) return i;
  }
  return i;
}

/** Top-level field names in an object-type body. Handles nested objects by
 *  tracking brace depth — only depth-1 `name:` / `name?:` are fields.
 *  Handles ts-rs quoted keys too (`"type": "Agent"`). */
function fieldNames(body) {
  const clean = stripComments(body);
  const names = new Set();
  let depth = 0;
  // Walk token by token, recording keys at depth 1 immediately before a `:`.
  // Alternatives: brace | quoted key | bare string (consumed, no depth effect) | bare key.
  const re = /([{}])|"((?:[^"\\]|\\.)*)"\s*\??\s*:|("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')|(\b[a-zA-Z_$][a-zA-Z0-9_$]*)\s*\??\s*:/g;
  let m;
  while ((m = re.exec(clean))) {
    if (m[1] === '{') depth++;
    else if (m[1] === '}') depth--;
    else if (m[2] !== undefined && depth === 1) names.add(m[2]);
    else if (m[4] && depth === 1) names.add(m[4]);
  }
  return names;
}

/** Extract the declaration for `export (interface|type) Name` from src.
 *  - interface → { kind: 'interface', rhs: balanced `{ … }` body }
 *  - type alias → { kind: 'type', rhs: right-hand side after `=` }, bounded at
 *    the first top-level `;` (or balanced end) so it can NEVER run past the
 *    current declaration into the next type.
 *  null if the name isn't declared. */
function declaration(src, name) {
  const re = new RegExp(`export (interface|type) ${name}\\b`);
  const m = re.exec(src);
  if (!m) return null;
  let i = m.index + m[0].length;
  if (m[1] === 'interface') {
    // Skip to the opening `{` (past any extends clause), then balance.
    while (i < src.length && src[i] !== '{' && src[i] !== ';') i++;
    if (src[i] !== '{') return null;
    let depth = 0;
    const open = i;
    for (; i < src.length; i++) {
      const c = src[i];
      if (c === '"' || c === "'" || c === '`') { i = skipString(src, i); continue; }
      if (c === '{') depth++;
      else if (c === '}') { depth--; if (depth === 0) return { kind: 'interface', rhs: src.slice(open, i + 1) }; }
    }
    return null;
  }
  // type alias: skip generic params to the `=` at angle-depth 0.
  let angle = 0;
  for (;; i++) {
    if (i >= src.length) return null;
    const c = src[i];
    if (c === '<') angle++;
    else if (c === '>') angle--;
    else if (c === '=' && angle === 0) { i++; break; }
    else if (c === ';') return null;
  }
  // Right-hand side ends at the first `;` outside any bracket/string nesting.
  let depth = 0;
  const start = i;
  for (; i < src.length; i++) {
    const c = src[i];
    if (c === '"' || c === "'" || c === '`') { i = skipString(src, i); continue; }
    if (c === '{' || c === '(' || c === '[') depth++;
    else if (c === '}' || c === ')' || c === ']') depth--;
    else if (c === ';' && depth === 0) break;
  }
  return { kind: 'type', rhs: src.slice(start, i) };
}

/** Split a type-alias right-hand side on TOP-LEVEL `|` only (depth-aware, so
 *  pipes nested in variant bodies / generics / strings don't split). */
function splitUnion(rhs) {
  const parts = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < rhs.length; i++) {
    const c = rhs[i];
    if (c === '"' || c === "'" || c === '`') { i = skipString(rhs, i); continue; }
    if (c === '{' || c === '(' || c === '[' || c === '<') depth++;
    else if (c === '}' || c === ')' || c === ']' || c === '>') depth--;
    else if (c === '|' && depth === 0) { parts.push(rhs.slice(start, i)); start = i + 1; }
  }
  parts.push(rhs.slice(start));
  return parts;
}

/** Union of field names across ALL top-level object variants of a declaration
 *  (tagged unions included — every `{ … }` variant contributes, not just the
 *  first). Returns null when the declaration has no object body at all
 *  (pure alias, e.g. `type OnInvalid = "Continue" | "Fail"`). */
function unionFields(decl) {
  if (!decl) return null;
  if (decl.kind === 'interface') return fieldNames(decl.rhs);
  let hasObjectVariant = false;
  const names = new Set();
  for (const part of splitUnion(decl.rhs)) {
    const t = part.trim();
    if (t.startsWith('{')) {
      hasObjectVariant = true;
      for (const n of fieldNames(t)) names.add(n);
    }
  }
  return hasObjectVariant ? names : null;
}

const generated = stripComments(readFileSync(GENERATED, 'utf8'));
const declared = new Set([...generated.matchAll(/export (?:interface|type) (\w+)/g)].map((m) => m[1]));

const drift = [];
let compared = 0;
const skipped = []; // { name, reason } — declared but not field-comparable
for (const f of walk(BINDINGS)) {
  const name = basename(f, '.ts');
  if (!declared.has(name)) continue; // backend-only type not in the aggregate → out of scope
  const bindingSrc = stripComments(readFileSync(f, 'utf8'));
  const want = unionFields(declaration(bindingSrc, name));
  const have = unionFields(declaration(generated, name));
  if (!want || !have) {
    const side = !want && !have ? 'both sides' : !want ? 'binding side' : 'generated.ts side';
    skipped.push({ name, reason: `pure alias (no object body) on ${side}` });
    continue;
  }
  compared++;
  const missing = [...want].filter((fld) => !have.has(fld));
  if (missing.length) drift.push({ name, missing });
}

const frontendOnly = declared.size - compared - skipped.length; // declared in generated.ts, no binding file
const coverage =
  `[types-drift] coverage: ${compared} type(s) field-compared, ${skipped.length} skipped` +
  ` (alias/no object body), ${frontendOnly} frontend-only (no binding), of ${declared.size} declared.`;

if (drift.length) {
  console.error(`[types-drift] ${drift.length} frontend type(s) are MISSING fields present in the Rust model:`);
  for (const d of drift) console.error(`  ${d.name}: ${d.missing.join(', ')}`);
  console.error(`\nAdd the field(s) to src/types/generated.ts (see backend/bindings/<Name>.ts).`);
  console.error(coverage);
  process.exit(1);
}
if (skipped.length) {
  console.log(`[types-drift] skipped (no field comparison): ${skipped.map((s) => s.name).sort().join(', ')}`);
}
console.log(coverage);
console.log(`[types-drift] OK — no field drift on the ${compared} field-compared frontend-declared types.`);
