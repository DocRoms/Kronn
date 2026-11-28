// KT-1879 — one plural function for every UI locale.
//
// `pluralForm(locale, count)` picks the form a label stores (`one` or
// `other`); `tPlural` reads `<key>.one` / `<key>.other` and passes the count
// as `{0}`. It replaces the three mechanisms found in the locales: `(s)` in
// the text, a `{1}` argument the caller fills with 's', and `…Plural` /
// `…One` / `…Many` key pairs chosen with `=== 1` or `> 1`.

import { afterEach, describe, expect, it } from 'vitest';
import { pluralForm, PLURAL_FORMS } from '../i18n/plural';
import { __setLocaleLoadersForTests, loadLocale, t, tPlural, UI_LOCALES, type TranslationDict } from '../i18n';

describe('pluralForm', () => {
  // 0, 1, 2 and "many" (a large count) for each locale.
  const cases: Array<[string, number, 'one' | 'other']> = [
    ['en', 0, 'other'], ['en', 1, 'one'], ['en', 2, 'other'], ['en', 1_000_000, 'other'],
    ['fr', 0, 'one'], ['fr', 1, 'one'], ['fr', 2, 'other'], ['fr', 1_000_000, 'other'],
    ['es', 0, 'other'], ['es', 1, 'one'], ['es', 2, 'other'], ['es', 1_000_000, 'other'],
    ['zh', 0, 'other'], ['zh', 1, 'other'], ['zh', 2, 'other'], ['zh', 1_000_000, 'other'],
  ];
  for (const [locale, count, expected] of cases) {
    it(`${locale}: ${count} → ${expected}`, () => {
      expect(pluralForm(locale as 'en', count)).toBe(expected);
    });
  }

  it('reads the French "one" up to 2 excluded, as CLDR does (1,5 jour)', () => {
    expect(pluralForm('fr', 1.5)).toBe('one');
    expect(pluralForm('en', 1.5)).toBe('other');
    expect(pluralForm('es', 1.5)).toBe('other');
  });

  it('uses the absolute value of a negative count', () => {
    expect(pluralForm('en', -1)).toBe('one');
    expect(pluralForm('fr', -1)).toBe('one');
    expect(pluralForm('en', -2)).toBe('other');
  });

  it('falls back to "other" for a count that is not a finite number', () => {
    for (const { code } of UI_LOCALES) {
      expect(pluralForm(code, Number.NaN)).toBe('other');
      expect(pluralForm(code, Number.POSITIVE_INFINITY)).toBe('other');
    }
  });

  it('agrees with Intl.PluralRules on integers once "many" is folded into "other"', () => {
    // Intl ships the CLDR rules; French and Spanish add a "many" category for
    // round millions that the UI writes with the ordinary plural.
    const mismatches: string[] = [];
    for (const { code } of UI_LOCALES) {
      const rules = new Intl.PluralRules(code);
      for (const n of [...Array.from({ length: 202 }, (_, i) => i), 1_000, 1_000_000, 2_000_000]) {
        const cldr = rules.select(n);
        const expected = cldr === 'one' ? 'one' : 'other';
        if (pluralForm(code, n) !== expected) mismatches.push(`${code} ${n}: ${cldr}`);
      }
    }
    expect(mismatches).toEqual([]);
  });

  it('stores exactly the two forms a label declares', () => {
    expect(PLURAL_FORMS).toEqual(['one', 'other']);
  });
});

describe('tPlural', () => {
  let restore: (() => void) | undefined;
  afterEach(() => {
    restore?.();
    restore = undefined;
  });

  const dict = (one: string, other: string): { default: TranslationDict } => ({
    default: { 'demo.files.one': one, 'demo.files.other': other },
  });

  it('reads <key>.one or <key>.other and passes the count as {0}', async () => {
    restore = __setLocaleLoadersForTests({
      fr: async () => dict('{0} fichier', '{0} fichiers'),
      en: async () => dict('{0} file', '{0} files'),
      es: async () => dict('{0} archivo', '{0} archivos'),
      zh: async () => dict('{0} 个文件', '{0} 个文件'),
    });
    for (const { code } of UI_LOCALES) await loadLocale(code);

    expect([0, 1, 2, 5].map(n => tPlural('fr', 'demo.files', n))).toEqual(
      ['0 fichier', '1 fichier', '2 fichiers', '5 fichiers'],
    );
    expect([0, 1, 2, 5].map(n => tPlural('en', 'demo.files', n))).toEqual(
      ['0 files', '1 file', '2 files', '5 files'],
    );
    expect([0, 1, 2, 5].map(n => tPlural('es', 'demo.files', n))).toEqual(
      ['0 archivos', '1 archivo', '2 archivos', '5 archivos'],
    );
    expect([0, 1, 2, 5].map(n => tPlural('zh', 'demo.files', n))).toEqual(
      ['0 个文件', '1 个文件', '2 个文件', '5 个文件'],
    );
  });

  it('passes further arguments as {1}, {2}…', async () => {
    restore = __setLocaleLoadersForTests({
      fr: async () => ({ default: { 'demo.left.one': '{0} projet restant dans {1}', 'demo.left.other': '{0} projets restants dans {1}' } }),
      en: async () => ({ default: {} }),
      es: async () => ({ default: {} }),
      zh: async () => ({ default: {} }),
    });
    await loadLocale('fr');
    expect(tPlural('fr', 'demo.left', 3, 'Kronn')).toBe('3 projets restants dans Kronn');
    expect(t('fr', 'demo.left.one', 1, 'Kronn')).toBe('1 projet restant dans Kronn');
  });
});
