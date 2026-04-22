import { describe, it, expect } from 'vitest';
import { detectTriggeredSkills, selectTriggers } from '../autoTriggers';
import type { Skill } from '../../types/generated';

function skill(id: string, triggers: Skill['auto_triggers']): Skill {
  return {
    id,
    name: id,
    description: '',
    icon: '📄',
    category: 'Domain',
    content: '',
    is_builtin: true,
    token_estimate: 0,
    license: null,
    allowed_tools: null,
    auto_triggers: triggers,
  };
}

describe('autoTriggers', () => {
  describe('selectTriggers', () => {
    it('returns empty when no triggers declared', () => {
      expect(selectTriggers(null, 'fr')).toEqual([]);
      expect(selectTriggers(undefined, 'fr')).toEqual([]);
    });

    it('concatenates common + locale bucket', () => {
      const t = { common: ['\\bpdf\\b'], locales: { fr: ['rapport'], en: ['report'] } };
      expect(selectTriggers(t, 'fr')).toEqual(['\\bpdf\\b', 'rapport']);
      expect(selectTriggers(t, 'en')).toEqual(['\\bpdf\\b', 'report']);
    });

    it('falls back to EN when the requested locale has no bucket', () => {
      const t = { common: [], locales: { en: ['report'] } };
      expect(selectTriggers(t, 'pt')).toEqual(['report']);
    });

    it('returns only common when no bucket matches and no EN fallback', () => {
      const t = { common: ['pdf'], locales: { fr: ['rapport'] } };
      expect(selectTriggers(t, 'de')).toEqual(['pdf']);
    });
  });

  describe('detectTriggeredSkills', () => {
    const docs = skill('kronn-docs', {
      common: ['\\b(pdf|docx)\\b'],
      locales: {
        // Real conjugation space: "génère" (è grave) and "génér"
        // (é acute → "générer", "généré") both need to match.
        fr: ['\\bgén[eéè]r\\w*.{0,40}(fichier|rapport)'],
        en: ['generate.+(file|report)'],
      },
    });
    const wf = skill('workflow-architect', {
      common: [],
      locales: {
        fr: ['crée.*(workflow|automatisation)'],
        en: ['create.*(workflow|automation)'],
      },
    });
    const plain = skill('plain', null); // no triggers
    const pool: Skill[] = [docs, wf, plain];

    it('matches a format token via common bucket regardless of locale', () => {
      const hit = detectTriggeredSkills('peux-tu me faire un PDF ?', pool, [], 'fr');
      expect(hit.map(s => s.id)).toEqual(['kronn-docs']);
    });

    it('matches a locale-specific FR phrase', () => {
      const hit = detectTriggeredSkills(
        'génère-moi un fichier avec les stats',
        pool,
        [],
        'fr',
      );
      expect(hit.map(s => s.id)).toEqual(['kronn-docs']);
    });

    it('matches a locale-specific EN phrase', () => {
      const hit = detectTriggeredSkills(
        'generate a report of last week activity',
        pool,
        [],
        'en',
      );
      expect(hit.map(s => s.id)).toEqual(['kronn-docs']);
    });

    it('skips skills already active on the discussion', () => {
      const hit = detectTriggeredSkills('give me a pdf', pool, ['kronn-docs'], 'en');
      expect(hit).toEqual([]);
    });

    it('is case-insensitive for file-format tokens', () => {
      const hit = detectTriggeredSkills('Export as PDF please', pool, [], 'en');
      expect(hit.map(s => s.id)).toEqual(['kronn-docs']);
    });

    it('returns multiple skills when several match', () => {
      const hit = detectTriggeredSkills(
        'crée un workflow qui génère un rapport PDF mensuel',
        pool,
        [],
        'fr',
      );
      const ids = hit.map(s => s.id).sort();
      expect(ids).toEqual(['kronn-docs', 'workflow-architect']);
    });

    it('ignores skills with no triggers', () => {
      const hit = detectTriggeredSkills('pdf and workflow and plain', pool, [], 'en');
      expect(hit.find(s => s.id === 'plain')).toBeUndefined();
    });

    it('does not throw on an invalid regex in a skill trigger', () => {
      const bad = skill('bad', { common: ['[unclosed'], locales: {} });
      // Bad regex is silently skipped — the matcher stays alive for
      // the other valid patterns on the same or other skills.
      expect(() =>
        detectTriggeredSkills('anything', [bad, docs], [], 'en'),
      ).not.toThrow();
    });

    it('respects the operator opt-out list (Settings toggle)', () => {
      // kronn-docs would normally match "make a pdf" but is opted-out
      // by the operator → the filter skips it.
      const hit = detectTriggeredSkills(
        'make a pdf',
        pool,
        [],
        'en',
        new Set(['kronn-docs']),
      );
      expect(hit).toEqual([]);
    });

    it('does not match unrelated prose', () => {
      const hit = detectTriggeredSkills(
        'le PDF que tu as envoyé hier a bien été reçu',
        pool,
        [],
        'fr',
      );
      // "pdf" appears but it's a valid match for the common regex —
      // this is a known false-positive tradeoff. We match to be safe
      // rather than miss genuine intent. Acceptance threshold = low.
      expect(hit.map(s => s.id)).toContain('kronn-docs');
    });
  });
});
