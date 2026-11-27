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

  // KT-2016 — the forms the probe of the guards found unreported.
  it.each([
    ['a literal in braces', wrap("<p>{'Texte'}</p>")],
    ['a template without expression in braces', wrap('<p>{`Saved changes`}</p>')],
    ['a literal after &&', wrap("<p>{empty && 'Nothing yet'}</p>")],
    ['a literal after ||', wrap("<p>{label || 'Untitled'}</p>")],
    ['a literal after ??', wrap("<p>{label ?? 'Untitled'}</p>")],
    ['a literal branch of a ternary beside t()', wrap("<p>{ok ? t('a.saved') : 'Failed'}</p>")],
    ['a template with text in a child', wrap('<p>{`${count} items left`}</p>')],
    ['a template with an expression in an aria-label', wrap('<button aria-label={`Delete ${name}`}>{t(\'a\')}</button>')],
    ['a concatenation in a title', wrap("<span title={'Open ' + name}>{t('a')}</span>")],
    ['a literal branch in a placeholder', wrap("<input aria-label={t('a')} placeholder={busy ? t('a.wait') : 'Type here'} />")],
    ['the description of a component', wrap("<EmptyState description=\"Nothing to show yet\" />", "import { EmptyState } from './ui/EmptyState';")],
    ['the confirmLabel of a component', wrap("<ConfirmDialog confirmLabel={'Delete it'} />", "import { ConfirmDialog } from './ui/ConfirmDialog';")],
  ])('fails on %s', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([2]);
  });

  it('reports each literal branch of a ternary', async () => {
    expect(await guard(rule, wrap("<p>{ok ? 'Saved' : 'Failed'}</p>"))).toEqual([2, 2]);
  });

  it.each([
    ['punctuation, a path, a unit or a product in braces', wrap("<p>{' · '}{'~/.kronn'}{n > 1 ? 'ms' : 's'}{'Kronn'}</p>")],
    ['a template of values only', wrap('<p title={`${agent} · ${model}`}>{`${n}px`}</p>')],
    ['an elapsed time', wrap('<p>{`${m}m${s}s`}</p>')],
    ['a ternary of t() calls', wrap("<p>{ok ? t('a.saved') : t('a.failed')}</p>")],
    ['a literal in braces inside code', wrap("<code>{'pnpm lint --fix'}</code>")],
    ['content on a native element', wrap('<meta content="width=device-width, initial scale" />')],
    ['the error of a component that is not a string', wrap('<Field error={hasError} />', "import { Field } from './ui/Field';")],
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

  // KT-2016 — an element that plays a button, and buttons whose children only
  // draw icons through an expression.
  const more = "import { X, ChevronUp, ChevronDown, Loader2, Check } from 'lucide-react';";
  it.each([
    ['a div role="button" with an icon only', wrap('<div role="button" tabIndex={0} onClick={() => {}}><X size={12} /></div>', more)],
    ['a span role="button" with no child', wrap('<span role="button" tabIndex={0} onClick={() => {}} />', more)],
    ['a ternary of two icons', wrap('<button onClick={() => {}}>{open ? <ChevronUp /> : <ChevronDown />}</button>', more)],
    ['an icon behind &&', wrap('<button>{busy && <Loader2 />}<Check /></button>', more)],
    ['an icon held in a variable', wrap('<button>{icon}</button>', 'const icon = null;')],
    ['an icon held in a property', wrap('<button>{item.icon}</button>', 'const item = { icon: null };')],
    ['a spinner component', wrap('<button><Spinner /></button>', "import { Spinner } from './Spinner';")],
  ])('fails on %s', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([2]);
  });

  it.each([
    ['a role="button" named by its text', wrap("<div role=\"button\" tabIndex={0}><X />{t('a.open')}</div>", more)],
    ['a role="button" named by a variable child', wrap('<div role="button" tabIndex={0}><X /><span>{node.name}</span></div>', more)],
    ['a role="button" with aria-label', wrap("<div role=\"button\" aria-label={t('a')}><X /></div>", more)],
    ['a ternary with a text branch', wrap("<button>{busy ? <Loader2 /> : t('a.save')}</button>", more)],
    ['an icon and a label behind &&', wrap("<button>{busy && <Loader2 />}{t('a.save')}</button>", more)],
    ['a variable that is not an icon', wrap('<button>{label}</button>', "const label = 'x';")],
    ['an unknown component', wrap('<button><Label /></button>', "import { Label } from './Label';")],
    ['a component with role="button", whose props may name it', wrap('<Row role="button" />', "import { Row } from './Row';")],
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
    ['branches on the scale', wrap("<p style={{ fontSize: compact ? 'var(--kr-type-xs)' : 'var(--kr-type-sm)', color: on && 'var(--kr-accent)' }} />", 'const compact = true; const on = true;')],
  ])('leaves alone %s', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([]);
  });

  // KT-2016 — a value behind a condition is read, branch by branch.
  it.each([
    ['a px font size in a ternary branch', wrap("<p style={{ fontSize: compact ? 11 : 'var(--kr-type-sm)' }} />", 'const compact = true;')],
    ['a literal colour behind &&', wrap("<p style={{ color: on && '#ff0000' }} />", 'const on = true;')],
    ['a radius behind ??', wrap('<p style={{ borderRadius: radius ?? 6 }} />', 'const radius = undefined;')],
    ['a literal colour in a --custom key', wrap("<p style={{ '--tone': '#ff0000' } as React.CSSProperties} />")],
  ])('fails on %s', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([2]);
  });
});

// KT-1936 — an agent is named with its model and its role by AgentIdentity
// only: an element written in an agent's colour that also writes a model or a
// role is the ad-hoc markup each surface used to build.
describe('ui-guards/no-adhoc-agent-identity', () => {
  const rule = 'ui-guards/no-adhoc-agent-identity';
  const head = "import { agentTextColor, agentColor } from '../lib/constants';\nimport { AgentIdentity } from './ui/AgentIdentity';";
  it.each([
    ['a name and the message model', wrap("<div style={{ color: agentTextColor('Codex') }}>@codex <span>· {msg.model}</span></div>", head)],
    ['a name and a tier', wrap("<span style={{ color: agentColor(a) }}>{name} · {msg.model_tier}</span>", head)],
    ['a name and a role label', wrap("<div style={{ color: agentTextColor(a), gap: 4 }}>{name}<span> · {t('disc.targetDiscussionAgent')}</span></div>", head)],
    ['a name and a model variable', wrap("<strong style={{ color: agentTextColor(a) }}>{label} · {currentModel}</strong>", head)],
    // KT-1936 review, finding 5 — the model or the role as a SIBLING of the coloured name.
    ['a model next to the coloured name, in the same parent', wrap("<div><span style={{ color: agentTextColor(m.agent) }}>{m.agent}</span><i>{m.model}</i></div>", head)],
    ['a role next to the coloured name, in a fragment', wrap("<><strong style={{ color: agentColor(a) }}>{name}</strong><small>{t('ui.agentIdentity.role.cli')}</small></>", head)],
    ['a coloured name behind a condition, its model next to it', wrap("<p>{show && <span style={{ color: agentTextColor(a) }}>{name}</span>}<em>{p.declaredModel}</em></p>", head)],
  ])('fails on %s', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([2]);
  });

  it.each([
    ['AgentIdentity inside the coloured label', wrap("<div style={{ color: agentTextColor(a) }}><AgentIdentity name={n} model={{ kind: 'id', id: msg.model }} role='main' /></div>", head)],
    ['a name alone in its colour', wrap("<span style={{ color: agentTextColor(a) }}>{label}</span>", head)],
    ['a model next to a name not in an agent colour', wrap("<span>{label} · {msg.model}</span>", head)],
    ['AgentIdentity next to a coloured name in the same parent', wrap("<div><span style={{ color: agentTextColor(a) }}>{label}</span><AgentIdentity name={n} model={{ kind: 'id', id: msg.model }} /></div>", head)],
    ['a coloured name whose siblings write no model and no role', wrap("<div><span style={{ color: agentTextColor(a) }}>{label}</span><time>{when}</time></div>", head)],
    ['a model in the parent of the parent: the heuristic stops at the parent', wrap("<section><div><span style={{ color: agentTextColor(a) }}>{label}</span></div><footer>{msg.model}</footer></section>", head)],
  ])('leaves alone %s', async (_name, code) => {
    expect(await guard(rule, code)).toEqual([]);
  });

  it('is off in components/ui, where AgentIdentity is built', async () => {
    const code = wrap("<span style={{ color: agentTextColor(a) }}>{name} · {model}</span>", head);
    expect(await guard(rule, code, 'src/components/ui/Example.tsx')).toEqual([]);
  });

  it('finds no ad-hoc agent identity in the app, and has no baseline', () => {
    expect(Object.values(BASELINE).some(r => r[rule])).toBe(false);
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
