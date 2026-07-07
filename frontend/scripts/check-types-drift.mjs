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
// simplifications never trip it. Run after `cargo test export_bindings`.
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

/** Top-level field names in an object-type body. Handles nested objects by
 *  tracking brace depth — only depth-1 `name:` / `name?:` are fields. */
function fieldNames(body) {
  const clean = stripComments(body);
  const names = new Set();
  let depth = 0;
  // Walk token by token, recording identifiers at depth 1 immediately before a `:`.
  const re = /([{}])|(\b[a-zA-Z_][a-zA-Z0-9_]*)\s*\??\s*:/g;
  let m;
  while ((m = re.exec(clean))) {
    if (m[1] === '{') depth++;
    else if (m[1] === '}') depth--;
    else if (m[2] && depth === 1) names.add(m[2]);
  }
  return names;
}

/** Extract the object body `{ … }` for `export (type|interface) Name` from src,
 *  or null if it isn't a single object type (union / alias). */
function objectBody(src, name) {
  const re = new RegExp(`export (?:interface|type) ${name}\\b[^{]*(\\{)`, 'm');
  const m = re.exec(src);
  if (!m) return null;
  // Balance braces from the opening `{`.
  let i = m.index + m[0].length - 1;
  let depth = 0;
  const start = i;
  for (; i < src.length; i++) {
    if (src[i] === '{') depth++;
    else if (src[i] === '}') { depth--; if (depth === 0) return src.slice(start, i + 1); }
  }
  return null;
}

const generated = readFileSync(GENERATED, 'utf8');
const declared = new Set([...generated.matchAll(/export (?:interface|type) (\w+)/g)].map((m) => m[1]));

const drift = [];
for (const f of walk(BINDINGS)) {
  const name = basename(f, '.ts');
  if (!declared.has(name)) continue; // backend-only type not in the aggregate → out of scope
  const bindingSrc = readFileSync(f, 'utf8');
  const bindingBody = objectBody(bindingSrc, name);
  const genBody = objectBody(generated, name);
  if (!bindingBody || !genBody) continue; // union/alias — no field comparison
  const want = fieldNames(bindingBody);
  const have = fieldNames(genBody);
  const missing = [...want].filter((fld) => !have.has(fld));
  if (missing.length) drift.push({ name, missing });
}

if (drift.length) {
  console.error(`[types-drift] ${drift.length} frontend type(s) are MISSING fields present in the Rust model:`);
  for (const d of drift) console.error(`  ${d.name}: ${d.missing.join(', ')}`);
  console.error(`\nAdd the field(s) to src/types/generated.ts (see backend/bindings/<Name>.ts).`);
  process.exit(1);
}
console.log(`[types-drift] OK — no field drift on the ${declared.size} frontend-declared types.`);
