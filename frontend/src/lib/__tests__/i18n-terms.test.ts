// KT-1879 — the glossary and the French register, checked on the locale files.
//
// Existing labels are not rewritten here: `terms-baseline.json` records each
// label that already breaks a rule (hash of its text + what it breaks), and
// the check fails only on a new label, or on a touched one, that does.

import { describe, expect, it } from 'vitest';
// @ts-expect-error no declaration file for the check script
import * as termsRaw from '../../../scripts/i18n-terms.mjs';
import glossaryJson from '../i18n/glossary.json';
import baselineJson from '../i18n/terms-baseline.json';
import { dictionaries } from '../i18n/testing';

type Dicts = Record<string, Record<string, string>>;
type Baseline = {
  version: number;
  counts: Record<string, Record<string, Record<string, number>>>;
  labels: Record<string, Record<string, string[]>>;
};
type Failure = { locale: string; key: string; reason: 'new' | 'touched' | 'new-violation'; violations: string[] };
type Report = { failures: Failure[]; resolved: string[]; counts: Baseline['counts'] };
type Glossary = typeof glossaryJson;

const terms = termsRaw as {
  LOCALES: string[];
  hashLabel: (text: string) => string;
  compileGlossary: (g: unknown) => unknown;
  validateGlossary: (g: unknown) => string[];
  findViolations: (compiled: unknown, locale: string, text: string) => string[];
  buildBaseline: (args: { dictionaries: Dicts; glossary: unknown; previous?: Baseline; acceptNew?: boolean }) => Baseline;
  checkTerms: (args: { dictionaries: Dicts; glossary: unknown; baseline: Baseline }) => Report;
  serializeBaseline: (b: Baseline) => string;
  formatReport: (r: Report) => string;
};

const glossary = glossaryJson as Glossary;
const compiled = terms.compileGlossary(glossary);
const violations = (locale: string, text: string) => terms.findViolations(compiled, locale, text);

describe('glossary file', () => {
  it('is valid: one term per concept in en, fr, es and zh, compilable synonyms, no term flagged as its own synonym', () => {
    expect(terms.validateGlossary(glossary)).toEqual([]);
  });

  it('records the owner decisions', () => {
    expect(glossary.register.fr.use).toBe('vous');
    const byId = Object.fromEntries(glossary.concepts.map(c => [c.id, c]));
    expect(byId['main-agent'].terms).toEqual({ en: 'main agent', fr: 'agent principal', es: 'agente principal', zh: '主代理' });
    expect(byId.task.terms.fr).toBe('tâche');
    expect(byId.settings.terms.fr).toBe('réglages');
    expect(byId['quick-action'].types).toEqual({ api: 'API', cli: 'CLI' });
  });

  it('proposes a distinct term for each use of the homonym « principal », none equal to « agent principal »', () => {
    const uses = glossary.homonyms.find(h => h.word === 'principal')!.uses;
    expect(uses.map(u => u.id)).toEqual(['reviewing-agent', 'step-agent']);
    for (const locale of terms.LOCALES as Array<'en' | 'fr' | 'es' | 'zh'>) {
      const proposals = uses.map(u => u.proposal[locale]);
      expect(new Set(proposals).size, locale).toBe(2);
      const main = glossary.concepts.find(c => c.id === 'main-agent')!.terms[locale];
      expect(proposals, locale).not.toContain(main);
    }
  });

  it('rejects a concept without a term in one locale and a synonym that does not compile', () => {
    const broken = {
      ...glossary,
      concepts: [
        { id: 'x', terms: { en: 'x', fr: 'x', es: 'x' }, synonyms: { fr: ['('] } },
        { id: 'y', terms: { en: 'run', fr: 'run', es: 'run', zh: 'run' }, synonyms: { en: ['runs?'] } },
      ],
    };
    const errors = terms.validateGlossary(broken).join('\n');
    expect(errors).toMatch(/x.*zh/);
    expect(errors).toMatch(/x.*fr.*\(/);
    expect(errors).toMatch(/y.*en.*own synonym/);
  });
});

describe('findViolations', () => {
  it('reports a synonym of a glossary term, whole words only', () => {
    expect(violations('fr', 'Ouvrir la conversation')).toEqual(['term:discussion:conversations?']);
    expect(violations('fr', 'Ouvrir la discussion')).toEqual([]);
    expect(violations('en', 'Open the room')).toEqual(['term:discussion:rooms?']);
    expect(violations('en', 'Open the bathrooms')).toEqual([]);
  });

  it('reports the owner synonyms of « agent principal » in every locale', () => {
    expect(violations('fr', "Désactiver l'agent de discussion")).toEqual(['term:main-agent:agents? de discussion']);
    expect(violations('fr', 'Ce message part à l’agent de la discussion')).toEqual(['term:main-agent:agents? de la discussion']);
    expect(violations('en', 'Disable the discussion agent')).toEqual(['term:main-agent:discussion agents?']);
    expect(violations('es', 'Desactivar el agente de la discusión')).toEqual(['term:main-agent:agentes? de la discusión']);
    expect(violations('zh', '禁用讨论代理')).toEqual(['term:main-agent:讨论代理']);
    expect(violations('fr', "Désactiver l'agent principal")).toEqual([]);
  });

  it('ignores paths, identifiers, hyphenated words, code and templates', () => {
    expect(violations('fr', 'Chemin : /Users/vous/Repos/mon-projet')).toEqual([]);
    expect(violations('fr', 'Contexte cross-repo et repo-map')).toEqual([]);
    expect(violations('fr', 'Lancez un dry-run')).toEqual([]);
    expect(violations('fr', 'Tapez `git worktree add` puis {{state.run}}')).toEqual([]);
    expect(violations('fr', 'Voir https://example.com/run/repo')).toEqual([]);
    expect(violations('zh', '已完成的 step_done 事件')).toEqual([]);
  });

  it('skips a match that overlaps an allowed phrase (case-sensitive)', () => {
    expect(violations('fr', 'Ouvrez l’onglet Automatisation')).toEqual([]);
    expect(violations('fr', 'Créer une automatisation')).toEqual(['term:workflow:automatisations?']);
    expect(violations('fr', 'via le MCP kronn-internal')).toEqual([]);
    expect(violations('fr', 'Les MCP liés')).toEqual(['term:plugin:MCPs?']);
    expect(violations('zh', '关闭对话框')).toEqual([]);
  });

  it('reports « tu » forms in French and nothing else, under the register rule', () => {
    expect(violations('fr', 'Décris ton besoin')).toEqual(['register:fr:ton']);
    expect(violations('fr', 'Comment veux-tu décrire ton API ?')).toEqual(['register:fr:tu', 'register:fr:ton']);
    expect(violations('fr', 'Un agent relit tes réponses et te demande')).toEqual(['register:fr:te', 'register:fr:tes']);
    expect(violations('fr', 'Il t’aide')).toEqual(["register:fr:t(?=['’])"]);
    expect(violations('fr', 'Décrivez votre besoin')).toEqual([]);
    expect(violations('fr', 'Garder le même ton, régler le ton')).toEqual([]);
    expect(violations('fr', 'Tests et statuts')).toEqual([]);
    expect(violations('en', 'Tell me what you need')).toEqual([]);
  });
});

describe('checkTerms against a baseline', () => {
  const glossaryMini = {
    version: 1,
    register: { fr: { use: 'vous', forbidden: ['tu', 'ton'], allow: [] } },
    concepts: [
      {
        id: 'discussion',
        terms: { en: 'discussion', fr: 'discussion', es: 'discusión', zh: '讨论' },
        synonyms: { en: ['conversations?'], fr: ['conversations?'], es: [], zh: [] },
      },
    ],
  };
  const before: Dicts = {
    fr: { 'a.old': 'Une conversation', 'a.ok': 'Une discussion', 'a.tu': 'Décris ton besoin' },
    en: { 'a.old': 'A conversation', 'a.ok': 'A discussion', 'a.tu': 'Describe your need' },
    es: {}, zh: {},
  };
  const baseline = terms.buildBaseline({ dictionaries: before, glossary: glossaryMini, acceptNew: true });

  it('records each breaking label with the hash of its text, and the count per cluster', () => {
    expect(baseline.labels.fr).toEqual({
      'a.old': [terms.hashLabel('Une conversation'), 'term:discussion:conversations?'],
      'a.tu': [terms.hashLabel('Décris ton besoin'), 'register:fr:ton'],
    });
    expect(baseline.counts.fr).toEqual({ discussion: { 'conversations?': 1 }, 'register:fr': { ton: 1 } });
    expect(baseline.counts.en).toEqual({ discussion: { 'conversations?': 1 } });
  });

  it('passes while the recorded labels are untouched', () => {
    expect(terms.checkTerms({ dictionaries: before, glossary: glossaryMini, baseline }).failures).toEqual([]);
  });

  it('fails on a new label that uses a synonym or « tu »', () => {
    const after = { ...before, fr: { ...before.fr, 'a.new': 'Nouvelle conversation', 'a.new2': 'Tu peux' } };
    const { failures } = terms.checkTerms({ dictionaries: after, glossary: glossaryMini, baseline });
    expect(failures).toEqual([
      { locale: 'fr', key: 'a.new', reason: 'new', violations: ['term:discussion:conversations?'] },
      { locale: 'fr', key: 'a.new2', reason: 'new', violations: ['register:fr:tu'] },
    ]);
  });

  it('fails on a touched label that still breaks a rule, passes once it is fixed', () => {
    const touched = { ...before, fr: { ...before.fr, 'a.old': 'Une longue conversation' } };
    expect(terms.checkTerms({ dictionaries: touched, glossary: glossaryMini, baseline }).failures).toEqual([
      { locale: 'fr', key: 'a.old', reason: 'touched', violations: ['term:discussion:conversations?'] },
    ]);
    const fixed = { ...before, fr: { ...before.fr, 'a.old': 'Une discussion', 'a.tu': 'Décrivez votre besoin' } };
    const report = terms.checkTerms({ dictionaries: fixed, glossary: glossaryMini, baseline });
    expect(report.failures).toEqual([]);
    expect(report.resolved).toEqual(['fr a.old', 'fr a.tu']);
  });

  it('fails when a glossary change finds a new violation in an untouched label', () => {
    const wider = { ...glossaryMini, register: { fr: { use: 'vous', forbidden: ['tu', 'ton', 'une'], allow: [] } } };
    const { failures } = terms.checkTerms({ dictionaries: before, glossary: wider, baseline });
    expect(failures.map(f => [f.key, f.reason, f.violations])).toEqual([
      ['a.old', 'new-violation', ['term:discussion:conversations?', 'register:fr:une']],
      ['a.ok', 'new', ['register:fr:une']],
    ]);
  });

  it('refreshing the baseline without acceptNew drops resolved labels but never records a new one', () => {
    const after = { ...before, fr: { 'a.old': 'Une discussion', 'a.ok': 'Une discussion', 'a.tu': 'Décris ton besoin', 'a.new': 'Tu peux' } };
    const next = terms.buildBaseline({ dictionaries: after, glossary: glossaryMini, previous: baseline });
    expect(Object.keys(next.labels.fr)).toEqual(['a.tu']);
    expect(terms.checkTerms({ dictionaries: after, glossary: glossaryMini, baseline: next }).failures.map(f => f.key)).toEqual(['a.new']);
  });

  it('serializes one label per line, sorted, so a diff shows what changed', () => {
    const text = terms.serializeBaseline(baseline);
    expect(JSON.parse(text)).toEqual(baseline);
    expect(text).toContain(`      "a.old": ["${terms.hashLabel('Une conversation')}","term:discussion:conversations?"],\n`);
  });

  it('names the glossary, the baseline and the command in a failure report', () => {
    const after = { ...before, fr: { ...before.fr, 'a.new': 'Nouvelle conversation' } };
    const out = terms.formatReport(terms.checkTerms({ dictionaries: after, glossary: glossaryMini, baseline }));
    expect(out).toContain("fr 'a.new' (new): term:discussion:conversations?");
    expect(out).toContain('glossary.json');
    expect(out).toContain('docs/conventions/ui-text.md');
  });
});

describe('the shipped locales', () => {
  it('add no synonym and no « tu » beyond the recorded baseline', () => {
    const report = terms.checkTerms({ dictionaries: dictionaries as Dicts, glossary, baseline: baselineJson as Baseline });
    expect(report.failures, terms.formatReport(report)).toEqual([]);
  });

  it('keep the baseline in step: no recorded label has been fixed without refreshing it', () => {
    const report = terms.checkTerms({ dictionaries: dictionaries as Dicts, glossary, baseline: baselineJson as Baseline });
    expect(report.resolved, 'run node frontend/scripts/i18n-terms.mjs --update-baseline').toEqual([]);
  });

  it('records the count per cluster that the baseline holds', () => {
    const recount = terms.buildBaseline({ dictionaries: dictionaries as Dicts, glossary, previous: baselineJson as Baseline });
    expect(recount.counts).toEqual((baselineJson as Baseline).counts);
  });
});
