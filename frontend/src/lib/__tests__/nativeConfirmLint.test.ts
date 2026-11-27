// @vitest-environment node
// KT-1881 — no new code calls the native confirm(): ESLint refuses
// `confirm(…)`, `window.confirm(…)`, `globalThis.confirm(…)` and
// `self.confirm(…)` (no-restricted-globals, no-restricted-properties), and the
// calls that existed when the rule came in are its baseline, recorded in
// eslint-suppressions.json (ESLint bulk suppressions). `pnpm lint` fails on a
// call beyond the baseline and on a baseline entry no call matches any more.
import { describe, expect, it } from 'vitest';
import { existsSync, readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { ESLint } from 'eslint';

const FRONTEND = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const RULES = ['no-restricted-globals', 'no-restricted-properties'] as const;
const eslint = new ESLint({ cwd: FRONTEND });

type Suppressions = Record<string, Record<string, { count: number }>>;
const BASELINE: Suppressions = JSON.parse(readFileSync(join(FRONTEND, 'eslint-suppressions.json'), 'utf8'));

async function confirmErrors(code: string, file = 'src/components/Example.tsx') {
  const [result] = await eslint.lintText(code, { filePath: join(FRONTEND, file) });
  return result.messages.filter(m => (RULES as readonly string[]).includes(m.ruleId ?? ''));
}

describe('the native confirm() lint rule (KT-1881)', () => {
  it.each([
    ['window.confirm', "export function drop() { if (!window.confirm('Delete?')) return; }", 'no-restricted-properties'],
    ['a bare confirm', "export const drop = () => confirm('Delete?');", 'no-restricted-globals'],
    ['globalThis.confirm', "export const drop = () => globalThis.confirm('Delete?');", 'no-restricted-properties'],
  ])('fails on a new %s call, pointing to ConfirmDialog', async (_name, code, rule) => {
    const errors = await confirmErrors(code);
    expect(errors.map(e => [e.ruleId, e.severity])).toEqual([[rule, 2]]);
    expect(errors[0].message).toMatch(/ConfirmDialog/);
    expect(errors[0].message).toMatch(/useConfirm/);
  });

  it('leaves alone a local function named confirm', async () => {
    const code = "export function Panel(ok: boolean) { const confirm = (ids: string[]) => ids.length > 0 && ok; return confirm(['a']); }";
    expect(await confirmErrors(code)).toEqual([]);
  });

  it('leaves alone a test that stubs the native confirm', async () => {
    const code = "import { vi } from 'vitest';\nwindow.confirm = vi.fn(() => true);\nvi.stubGlobal('confirm', vi.fn());\n";
    expect(await confirmErrors(code, 'src/components/__tests__/Example.test.tsx')).toEqual([]);
  });

  it('records only these two rules in the baseline, on files that exist', () => {
    const files = Object.keys(BASELINE);
    expect(files.length).toBeGreaterThan(0);
    for (const [file, rules] of Object.entries(BASELINE)) {
      expect(existsSync(join(FRONTEND, file)), file).toBe(true);
      for (const rule of Object.keys(rules)) expect(RULES, `${file}: ${rule}`).toContain(rule);
    }
  });

  it('matches each baseline entry with the calls the file still holds', async () => {
    const results = await eslint.lintFiles(Object.keys(BASELINE).map(file => join(FRONTEND, file)));
    const found: Suppressions = {};
    for (const result of results) {
      const file = result.filePath.slice(FRONTEND.length + 1);
      for (const message of result.messages) {
        if (!message.ruleId || !(RULES as readonly string[]).includes(message.ruleId)) continue;
        found[file] ??= {};
        found[file][message.ruleId] = { count: (found[file][message.ruleId]?.count ?? 0) + 1 };
      }
    }
    expect(found).toEqual(BASELINE);
  }, 60_000);

  it('no longer holds the three calls ConfirmDialog replaced (KT-1456)', () => {
    expect(BASELINE['src/components/QPHistoryDrawer.tsx']).toBeUndefined();
    expect(BASELINE['src/components/SkillHistoryDrawer.tsx']).toBeUndefined();
    // The shadow removal there stays native until its lot: one call, not two.
    expect(BASELINE['src/components/LivePageResources.tsx']).toEqual({ 'no-restricted-properties': { count: 1 } });
  });
});
