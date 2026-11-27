// ─── One plural function for every UI locale (KT-1879) ─────────────────────
//
// A countable label stores two forms, `<key>.one` and `<key>.other`, in every
// locale, and is read with `tPlural(locale, key, count, ...args)` (see
// `../i18n`). This file only decides which form a count takes.
//
// The rules are the CLDR cardinal rules of the four locales, written out so
// the result does not depend on the ICU data a browser ships; "many" (French
// and Spanish round millions) uses the ordinary plural, so it folds into
// `other`. A test compares them with `Intl.PluralRules`.
//
//   en, es: one = exactly 1                 (0 files, 1 file, 2 files)
//   fr:     one = 0 ≤ n < 2                 (0 fichier, 1 fichier, 1,5 jour, 2 fichiers)
//   zh:     no grammatical plural → other   (0 个文件, 1 个文件)
//
// Convention: docs/conventions/ui-text.md

import type { UILocale } from '../i18n';

export const PLURAL_FORMS = ['one', 'other'] as const;
export type PluralForm = typeof PLURAL_FORMS[number];

export function pluralForm(locale: UILocale, count: number): PluralForm {
  if (!Number.isFinite(count)) return 'other';
  const n = Math.abs(count);
  switch (locale) {
    case 'fr':
      return n < 2 ? 'one' : 'other';
    case 'en':
    case 'es':
      return n === 1 ? 'one' : 'other';
    case 'zh':
      return 'other';
  }
}
