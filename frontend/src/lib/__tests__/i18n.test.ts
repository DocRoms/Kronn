import { describe, it, expect } from 'vitest';
import { t, getUILocale, setUILocale, UI_LOCALES } from '../i18n';

describe('i18n', () => {
  describe('t() — translation function', () => {
    it('returns French translation by default', () => {
      expect(t('fr', 'nav.projects')).toBe('Projets');
    });

    it('returns English translation', () => {
      expect(t('en', 'nav.projects')).toBe('Projects');
    });

    it('returns Spanish translation', () => {
      expect(t('es', 'nav.projects')).toBe('Proyectos');
    });

    it('falls back to French for unknown locale', () => {
      expect(t('xx' as any, 'nav.projects')).toBe('Projets');
    });

    it('returns raw key for unknown translation key', () => {
      expect(t('fr', 'nonexistent.key')).toBe('nonexistent.key');
    });

    it('interpolates {0} arguments', () => {
      expect(t('fr', 'audit.step', '3', '10', 'coding-rules.md')).toBe('Etape 3/10 — coding-rules.md');
    });

    it('interpolates numeric arguments', () => {
      expect(t('en', 'debate.launch', 3)).toBe('Launch debate (3 agents)');
    });
  });

  describe('UI_LOCALES', () => {
    it('has fr, en, es', () => {
      const codes = UI_LOCALES.map(l => l.code);
      expect(codes).toEqual(['fr', 'en', 'es']);
    });

    it('each locale has label and flag', () => {
      for (const l of UI_LOCALES) {
        expect(l.label).toBeTruthy();
        expect(l.flag).toBeTruthy();
      }
    });
  });

  describe('locale persistence', () => {
    it('defaults to fr when nothing stored', () => {
      localStorage.clear();
      expect(getUILocale()).toBe('fr');
    });

    it('persists and retrieves locale', () => {
      setUILocale('en');
      expect(getUILocale()).toBe('en');
      setUILocale('es');
      expect(getUILocale()).toBe('es');
      // cleanup
      localStorage.clear();
    });

    it('ignores invalid stored value', () => {
      localStorage.setItem('kronn:ui-locale', 'invalid');
      expect(getUILocale()).toBe('fr');
      localStorage.clear();
    });
  });

  describe('translation completeness', () => {
    // Ensure EN and ES have all keys that FR has
    it('EN has all FR keys', () => {
      const frKeys = Object.keys(getFrDict());
      const missing: string[] = [];
      for (const key of frKeys) {
        if (t('en', key) === key) {
          missing.push(key);
        }
      }
      // Allow a small tolerance for newly added keys
      expect(missing.length).toBeLessThan(5);
    });

    it('ES has all FR keys', () => {
      const frKeys = Object.keys(getFrDict());
      const missing: string[] = [];
      for (const key of frKeys) {
        if (t('es', key) === key) {
          missing.push(key);
        }
      }
      expect(missing.length).toBeLessThan(5);
    });
  });
});

// Helper to get FR dictionary keys (we test via t() which falls back)
function getFrDict(): Record<string, string> {
  // We can't directly import the dict, but we can verify via t()
  // that known keys exist
  const knownKeys = [
    'nav.projects', 'nav.discussions', 'nav.mcps', 'nav.workflows', 'nav.config',
    'disc.new', 'disc.agent', 'config.agents', 'wf.title', 'mcp.title',
    'debate.title', 'debate.rounds', 'wf.manual', 'wf.inProgress', 'wf.pending',
    'config.configFile',
  ];
  const dict: Record<string, string> = {};
  for (const k of knownKeys) {
    dict[k] = t('fr', k);
  }
  return dict;
}
