// C1 (plan 0.9, review Codex) — fidelity of the serde→TS optional transform.
// The hygiene test validates the CURRENT generated.ts; these validate the
// GENERATOR itself against fixture Rust source: which fields get `?`, which
// types are exempt, and how renames map.
import { describe, it, expect } from 'vitest';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
// Plain-JS module without a .d.ts — typed locally at the boundary.
// @ts-expect-error no declaration file for the build script
import { scanRustOptionals as scanRaw, applyOptionals as applyRaw } from '../../../scripts/assemble-generated-types.mjs';

const scanRustOptionals = scanRaw as (dir: string) => Map<string, Set<string>>;
const applyOptionals = applyRaw as (name: string, block: string, optionals: Map<string, Set<string>>) => string;

const FIXTURE_RS = `
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateThingRequest {
    pub title: String,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    pub note: Option<String>,
    #[serde(default, rename = "aliasName")]
    pub alias_name: String,
    #[serde(skip)]
    pub internal: bool,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, rename = "RenamedRequest")]
#[serde(default)]
pub struct InnerName {
    pub every: u32,
    pub field: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CamelRequest {
    #[serde(default)]
    pub long_field_name: u32,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ThingResponse {
    #[serde(default)]
    pub message_count: u32,
    pub maybe: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateThingRequest {
    pub name: Option<String>,
}
`;

function scanFixture(): Map<string, Set<string>> {
  const dir = mkdtempSync(join(tmpdir(), 'typegen-fixture-'));
  try {
    writeFileSync(join(dir, 'models.rs'), FIXTURE_RS);
    return scanRustOptionals(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

describe('typegen serde→optional transform', () => {
  const optionals = scanFixture();

  it('marks serde(default) and Option fields on a Deserialize-only request', () => {
    const set = optionals.get('CreateThingRequest')!;
    expect(set).toBeDefined();
    expect(set.has('skill_ids')).toBe(true); // #[serde(default)]
    expect(set.has('note')).toBe(true); // Option<T> — omittable on the wire
    expect(set.has('title')).toBe(false); // plain required field
    expect(set.has('internal')).toBe(false); // #[serde(skip)] never exported
  });

  it('honors per-field serde(rename) — the TS name gets the ?', () => {
    const set = optionals.get('CreateThingRequest')!;
    expect(set.has('aliasName')).toBe(true);
    expect(set.has('alias_name')).toBe(false);
  });

  it('container serde(default) marks every field, keyed by ts(rename)', () => {
    const set = optionals.get('RenamedRequest')!;
    expect(set).toBeDefined();
    expect(set.has('every')).toBe(true);
    expect(set.has('field')).toBe(true);
    expect(optionals.has('InnerName')).toBe(false);
  });

  it('applies container rename_all = camelCase to field names', () => {
    expect(optionals.get('CamelRequest')!.has('longFieldName')).toBe(true);
  });

  it('leaves response/bidirectional types strict', () => {
    expect(optionals.has('ThingResponse')).toBe(false);
  });

  it('includes bidirectional types explicitly named …Request', () => {
    expect(optionals.get('UpdateThingRequest')!.has('name')).toBe(true);
  });

  it('applyOptionals rewrites only the listed fields, idempotently', () => {
    const block = 'export type CreateThingRequest = { title: string, skill_ids: Array<string>, note: string | null, aliasName: string, };';
    const once = applyOptionals('CreateThingRequest', block, optionals);
    expect(once).toContain('skill_ids?:');
    expect(once).toContain('note?:');
    expect(once).toContain('aliasName?:');
    expect(once).toContain('{ title: string,'); // untouched
    expect(applyOptionals('CreateThingRequest', once, optionals)).toBe(once);
  });
});
