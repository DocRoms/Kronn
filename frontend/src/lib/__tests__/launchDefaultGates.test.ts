// KT-1654 — the launch dialog's default gates name the exact CI lint gate in
// every locale, so a principal never guesses `--lib` / `--lib --tests`.
import { describe, expect, it } from 'vitest';
import en from '../i18n/locales/en';
import fr from '../i18n/locales/fr';
import es from '../i18n/locales/es';
import zh from '../i18n/locales/zh';

describe('orch.config.validationsDefault', () => {
  const locales = { en, fr, es, zh } as Record<string, Record<string, string>>;

  for (const [name, dict] of Object.entries(locales)) {
    it(`${name} names the exact fmt and clippy gates`, () => {
      const text = dict['orch.config.validationsDefault'];
      expect(text).toContain('cargo fmt --check');
      expect(text).toContain('cargo clippy --all-targets -- -D warnings');
    });
  }
});
