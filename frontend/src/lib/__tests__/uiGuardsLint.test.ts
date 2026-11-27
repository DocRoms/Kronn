// @vitest-environment node
// KT-1887 — the ESLint guards that keep new code on the design system
// (scripts/eslint-ui-guards.mjs, docs/conventions/ui-lint-guards.md). Each
// rule fails on an injected violation and leaves the right pattern alone; the
// violations of the code as it was are recorded in eslint-suppressions.json,
// so `pnpm lint` passes on the baseline and fails on one more. And
// components/ui accepts no warning at all: a rule that only warns elsewhere
// is an error there.
import { describe, expect, it } from 'vitest';
import { existsSync, readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { ESLint } from 'eslint';

const FRONTEND = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const eslint = new ESLint({ cwd: FRONTEND });

type Suppressions = Record<string, Record<string, { count: number }>>;
const BASELINE: Suppressions = JSON.parse(readFileSync(join(FRONTEND, 'eslint-suppressions.json'), 'utf8'));
const RULES = [
  'ui-guards/no-literal-ui-string', 'ui-guards/control-has-name', 'ui-guards/no-adhoc-dialog',
  'ui-guards/no-foreign-page-stylesheet', 'ui-guards/no-off-scale-style',
] as const;

async function lint(code: string, file = 'src/components/Example.tsx') {
  const [result] = await eslint.lintText(code, { filePath: join(FRONTEND, file) });
  return result.messages;
}

async function guard(rule: string, code: string, file?: string) {
  return (await lint(code, file)).filter(m => m.ruleId === rule).map(m => m.severity);
}

const wrap = (jsx: string, head = '') =>
  `${head}\nexport function Example({ t }: { t: (k: string) => string }) {\n  return (${jsx});\n}\n`;

describe('ui-guards/no-literal-ui-string', () => {
  const rule = 'ui-guards/no-literal-ui-string';
  it.each([
    ['JSX text', wrap('<p>Save changes</p>')],
    ['an aria-label', wrap('<button aria-label="Close panel">x</button>')],
    ['a placeholder', wrap('<input placeholder="Search projects" />')],
    ['a title in braces', wrap("<span title={'Open the file'}>{t('a.b')}</span>")],
  ])('fails on %s', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([2]);
  });

  it.each([
    ['t()', wrap("<p title={t('a.title')}>{t('a.text')}</p>")],
    ['punctuation and numbers', wrap('<p>· — 42 % →</p>')],
    ['code', wrap('<p><code>pnpm lint --fix</code></p>')],
    ['a path, an acronym, a unit, a product name', wrap('<p><span>~/.codex/config.toml</span><span>JSON</span><span>ms</span><span>Kronn</span></p>')],
    ['an example identifier', wrap('<input placeholder="my-project" aria-label={t("a.b")} />')],
  ])('leaves alone %s', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([]);
  });
});

describe('ui-guards/control-has-name', () => {
  const rule = 'ui-guards/control-has-name';
  const icons = "import { X } from 'lucide-react';";
  it.each([
    ['an icon-only button', wrap('<button onClick={() => {}}><X size={12} /></button>', icons)],
    ['an input with a label beside it, not bound', wrap("<div><label>{t('a')}</label><input value=\"\" onChange={() => {}} /></div>")],
    ['a select with no name', wrap('<select value="a" onChange={() => {}}><option value="a">{t("a")}</option></select>')],
    ['a button named by an empty aria-label', wrap('<button aria-label=""><X /></button>', icons)],
    ['an input named by its placeholder alone', wrap("<input placeholder={t('a.search')} value=\"\" onChange={() => {}} />")],
    ['a textarea named by its placeholder alone', wrap("<textarea placeholder={t('a.note')} />")],
  ])('fails on %s', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([2]);
  });

  it.each([
    ['a button with text', wrap("<button><X />{t('a.close')}</button>", icons)],
    ['an icon button with aria-label', wrap("<button aria-label={t('a.close')}><X /></button>", icons)],
    ['a placeholder beside an aria-label', wrap("<input placeholder={t('a.search')} aria-label={t('a.search')} />")],
    ['an input inside its label', wrap("<label>{t('a')}<input /></label>")],
    ['an input bound by htmlFor', wrap("<div><label htmlFor=\"name\">{t('a')}</label><input id=\"name\" /></div>")],
    ['the child of Field', wrap("<Field label={t('a')}><input type=\"text\" /></Field>", "import { Field } from './ui/Field';")],
    ['a hidden file input', wrap("<input type=\"file\" style={{ display: 'none' }} />")],
    ['a submit input', wrap('<input type="submit" />')],
    ['spread props, which may carry the name', wrap('<input {...props} />', 'const props = {};')],
  ])('leaves alone %s', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([]);
  });
});

describe('ui-guards/no-adhoc-dialog', () => {
  const rule = 'ui-guards/no-adhoc-dialog';
  it.each([
    ['a modal div', wrap("<div role=\"dialog\" aria-modal=\"true\" aria-label={t('a')} />")],
    ['an alertdialog', wrap("<div role=\"alertdialog\" aria-modal=\"true\" aria-label={t('a')} />")],
    ['a native dialog', wrap("<dialog open>{t('a')}</dialog>")],
  ])('fails once on %s outside components/ui', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([2]);
  });

  it('leaves alone a non-modal popover and components/ui itself', async () => {
    expect(await guard(rule, wrap("<div role=\"dialog\" aria-label={t('a')} />"))).toEqual([]);
    expect(await guard(rule, wrap("<div role=\"dialog\" aria-modal=\"true\" aria-label={t('a')} />"), 'src/components/ui/Example.tsx')).toEqual([]);
  });
});

describe('ui-guards/no-foreign-page-stylesheet', () => {
  const rule = 'ui-guards/no-foreign-page-stylesheet';
  it('fails when a component or another page imports a page stylesheet', async () => {
    expect(await guard(rule, "import '../pages/DiscussionsPage.css';\n", 'src/components/Example.tsx')).toEqual([2]);
    expect(await guard(rule, "import './DiscussionsPage.css';\n", 'src/pages/WorkflowsPage.tsx')).toEqual([2]);
  });

  it('lets a page import its own stylesheet, and a component its own', async () => {
    expect(await guard(rule, "import './DiscussionsPage.css';\n", 'src/pages/DiscussionsPage.tsx')).toEqual([]);
    expect(await guard(rule, "import './Example.css';\n", 'src/components/Example.tsx')).toEqual([]);
  });
});

describe('ui-guards/no-off-scale-style', () => {
  const rule = 'ui-guards/no-off-scale-style';
  it.each([
    ['a px font size', wrap('<p style={{ fontSize: 11 }} />')],
    ['an em font size', wrap("<p style={{ fontSize: '0.85em' }} />")],
    ['a px radius', wrap('<p style={{ borderRadius: 6 }} />')],
    ['a hex colour', wrap("<p style={{ color: '#e8eaed' }} />")],
    ['an rgba background', wrap("<p style={{ background: 'rgba(0, 0, 0, 0.5)' }} />")],
    ['a breakpoint off BREAKPOINTS', 'import { useIsMobile } from "../hooks/useMediaQuery";\nexport function Example() { return useIsMobile(1100); }\n'],
  ])('fails on %s', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([2]);
  });

  it.each([
    ['tokens', wrap("<p style={{ fontSize: 'var(--kr-type-sm)', borderRadius: 'var(--kr-radius-control)', color: 'var(--kr-purple-soft)' }} />")],
    ['a token with a fallback', wrap("<p style={{ borderRadius: 'var(--kr-radius-md, 6px)', background: 'rgba(var(--kr-error-rgb), 0.05)' }} />")],
    ['a zero radius and a circle', wrap("<p style={{ borderRadius: 0, borderTopLeftRadius: '50%' }} />")],
    ['a computed size', wrap('<p style={{ fontSize: `${size}px` }} />', 'const size = 12;')],
    ['a breakpoint of the scale', 'import { useIsMobile } from "../hooks/useMediaQuery";\nexport function Example() { return useIsMobile(900); }\n'],
  ])('leaves alone %s', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([]);
  });
});

describe('the baseline of the guards (eslint-suppressions.json)', () => {
  it('records the guards on files that exist, the fourteen DiscussionsPage.css imports among them', () => {
    const imports: string[] = [];
    for (const [file, rules] of Object.entries(BASELINE)) {
      expect(existsSync(join(FRONTEND, file)), file).toBe(true);
      if (rules['ui-guards/no-foreign-page-stylesheet']
        && readFileSync(join(FRONTEND, file), 'utf8').includes('DiscussionsPage.css')
        && !file.endsWith('pages/DiscussionsPage.tsx')) imports.push(file);
    }
    expect(imports).toHaveLength(14);
    for (const rule of RULES) expect(Object.values(BASELINE).some(r => r[rule]), rule).toBe(true);
  });

  it('keeps a test file out of the guards', async () => {
    const code = wrap('<button style={{ fontSize: 11 }}><X /> Close</button>', "import { X } from 'lucide-react';");
    const messages = await lint(code, 'src/components/__tests__/Example.test.tsx');
    expect(messages.filter(m => m.ruleId?.startsWith('ui-guards/'))).toEqual([]);
  });
});

describe('components/ui accepts no warning (KT-1887)', () => {
  const setStateInEffect = [
    "import { useEffect, useState } from 'react';",
    'export function Example({ value }: { value: string }) {',
    '  const [copy, setCopy] = useState(value);',
    '  useEffect(() => { setCopy(value); }, [value]);',
    '  return copy;',
    '}',
    '',
  ].join('\n');

  it('reports set-state-in-effect as an error in components/ui, a warning elsewhere', async () => {
    const severity = async (file: string) =>
      (await lint(setStateInEffect, file)).filter(m => m.ruleId === 'react-hooks/set-state-in-effect').map(m => m.severity);
    expect(await severity('src/components/ui/Example.tsx')).toEqual([2]);
    expect(await severity('src/components/Example.tsx')).toEqual([1]);
  });

  it('reports every rule that warns elsewhere as an error there', async () => {
    const code = "export function Example(v: any) { console.log(v); return v!; }\n";
    const ui = await lint(code, 'src/components/ui/Example.tsx');
    expect(ui.filter(m => m.severity === 1)).toEqual([]);
    expect(ui.filter(m => m.severity === 2).map(m => m.ruleId).sort())
      .toEqual(['@typescript-eslint/no-explicit-any', '@typescript-eslint/no-non-null-assertion', 'no-console']);
  });

  it('finds no warning in components/ui today', async () => {
    const results = await eslint.lintFiles([join(FRONTEND, 'src/components/ui')]);
    const warnings = results.flatMap(r => r.messages.filter(m => m.severity === 1)
      .map(m => `${r.filePath.slice(FRONTEND.length + 1)}:${m.line} ${m.ruleId}`));
    expect(warnings).toEqual([]);
  }, 120_000);
});
