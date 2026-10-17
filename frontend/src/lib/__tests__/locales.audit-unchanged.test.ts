// KT-1130 — the inherited "no template to restore" advice is gone from every
// locale, and the neutral « Inchangé » outcome exists in each.
import { describe, it, expect } from 'vitest';
import fr from '../i18n/locales/fr';
import en from '../i18n/locales/en';
import es from '../i18n/locales/es';
import zh from '../i18n/locales/zh';

const LOCALES = { fr, en, es, zh } as Record<string, Record<string, string>>;

describe('audit locales — unchanged outcome', () => {
  it.each(Object.entries(LOCALES))('%s drops the legacy template advice', (_lang, locale) => {
    expect(locale['audit.stepWarningNotRepaired']).toBeUndefined();
    expect(locale['audit.stepWarningRepaired']).toBeUndefined();
    const all = Object.values(locale).join('\n');
    for (const legacy of ['Aucun template à restaurer', 'No template to restore', 'No hay plantilla para restaurar', '无模板可恢复']) {
      expect(all).not.toContain(legacy);
    }
  });

  it.each(Object.entries(LOCALES))('%s names the unchanged outcome and its toast', (_lang, locale) => {
    expect(locale['auditTimeline.status.unchanged']).toBeTruthy();
    expect(locale['auditTimeline.reason.verified']).toBeTruthy();
    expect(locale['audit.stepUnchanged']).toContain('{0}');
  });

  it('reads « Inchangé » in French', () => {
    expect(fr['auditTimeline.status.unchanged']).toBe('Inchangé');
  });
});
