// i18n isomorphism test.
//
// Guards the easy-to-miss case where a developer adds a key to `fr` but
// forgets the matching entry in `en` and/or `es`. The `t()` function
// silently falls back to the FR value, which ships an untranslated
// string to English/Spanish users.
//
// When this test fails it prints the missing keys for each target locale
// — fix them in i18n.ts and rerun.

import { describe, it, expect } from 'vitest';
import { dictionaries, UI_LOCALES } from '../i18n';

describe('i18n key parity', () => {
  const referenceLocale = 'fr';
  const reference = dictionaries[referenceLocale];
  const referenceKeys = Object.keys(reference).sort();

  for (const { code } of UI_LOCALES) {
    if (code === referenceLocale) continue;

    describe(`locale "${code}"`, () => {
      const dict = dictionaries[code];
      const keys = Object.keys(dict).sort();

      it(`has the same number of keys as ${referenceLocale}`, () => {
        expect(keys.length, `${code} key count must match ${referenceLocale}`).toBe(referenceKeys.length);
      });

      it(`has no missing keys compared to ${referenceLocale}`, () => {
        const missing = referenceKeys.filter((k) => !(k in dict));
        expect(missing, `${code} is missing keys: ${missing.join(', ')}`).toEqual([]);
      });

      it(`has no extra keys compared to ${referenceLocale}`, () => {
        // An extra key in a non-reference locale is almost always a typo
        // (e.g. `common.save` vs `common.Save`). Flag it — if it's
        // intentional, add the same key to fr.
        const extra = keys.filter((k) => !(k in reference));
        expect(extra, `${code} has keys absent from ${referenceLocale}: ${extra.join(', ')}`).toEqual([]);
      });

      it('has non-empty values for every key', () => {
        const empty = keys.filter((k) => !dict[k] || dict[k].trim() === '');
        expect(empty, `${code} has empty/blank values for: ${empty.join(', ')}`).toEqual([]);
      });
    });
  }

  it('en and es never introduce placeholders absent from the fr reference', () => {
    // Languages legitimately differ in pluralization/gender markers — FR
    // often has more placeholders (e.g. `{1}`, `{2}` for "s"/"x" suffixes)
    // than EN. The real bug is a target locale that uses a placeholder
    // the REFERENCE doesn't provide: the calling component sizes its
    // arguments from FR, so an EN-only `{5}` renders as a literal "{5}"
    // in the UI.
    const placeholderRe = /\{\d+\}/g;
    const mismatches: string[] = [];
    for (const key of referenceKeys) {
      const frSet = new Set(reference[key].match(placeholderRe) ?? []);
      for (const code of ['en', 'es'] as const) {
        const other = dictionaries[code][key];
        if (!other) continue; // missing-key case covered above
        const extras = (other.match(placeholderRe) ?? []).filter((p) => !frSet.has(p));
        if (extras.length > 0) {
          mismatches.push(`  ${code}."${key}" uses ${extras.join(', ')} but fr does not provide them`);
        }
      }
    }
    expect(mismatches, `Dangling placeholders in translations:\n${mismatches.join('\n')}`).toEqual([]);
  });
});
