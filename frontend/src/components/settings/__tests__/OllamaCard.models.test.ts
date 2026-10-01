import { describe, it, expect } from 'vitest';
import { SUGGESTED_MODELS, MLX_SUGGESTED_MODELS, suggestedModelsFor } from '../ollamaModels';
import { dictionaries } from '../../../lib/i18n/testing';

// Guards Kronn's hardcoded Ollama first-pull suggestions: tags that something
// in this repository shows exist and ran, a hardware range that includes no-GPU
// machines, MLX builds that only appear where the backend says they run, and
// full UI-locale coverage.
describe('OllamaCard suggested models', () => {
  it('never lists a tag known not to exist (was "gemma4:26b" — never in the registry)', () => {
    for (const m of [...SUGGESTED_MODELS, ...MLX_SUGGESTED_MODELS]) {
      expect(m.name, `unexpected tag: ${m.name}`).not.toBe('gemma4:26b');
    }
  });

  it('offers exactly one portable suggestion per hardware tier', () => {
    expect(SUGGESTED_MODELS.map(m => m.tier)).toEqual(['cpu', 'mid', 'power']);
  });

  it('includes at least one CPU-friendly (no-GPU) option — Kronn runs on WSL boxes too', () => {
    expect(SUGGESTED_MODELS.some(m => m.tier === 'cpu')).toBe(true);
  });

  it('cites, for every tag, the repository file it was taken from', () => {
    for (const m of [...SUGGESTED_MODELS, ...MLX_SUGGESTED_MODELS]) {
      expect(m.source, `no source for ${m.name}`).toMatch(/^(docs|backend)\//);
      // An exact tag: `family:variant`, no spaces, no wildcard.
      expect(m.name, `not an exact tag: ${m.name}`).toMatch(/^[a-z0-9._-]+:[a-z0-9._-]+$/);
    }
  });

  it('keeps no size figure: the real one comes from the download and the installed list', () => {
    for (const m of [...SUGGESTED_MODELS, ...MLX_SUGGESTED_MODELS]) {
      expect(Object.keys(m), `${m.name} carries a size`).not.toContain('size');
    }
  });

  it('every entry has a valid hardware tier', () => {
    for (const m of [...SUGGESTED_MODELS, ...MLX_SUGGESTED_MODELS]) {
      expect(['cpu', 'mid', 'power'], `bad tier for ${m.name}`).toContain(m.tier);
    }
  });

  it('MLX builds are tagged -mlx and flagged; portable ones are neither', () => {
    expect(MLX_SUGGESTED_MODELS.length).toBeGreaterThan(0);
    for (const m of MLX_SUGGESTED_MODELS) {
      expect(m.name, m.name).toMatch(/-mlx$/);
      expect(m.mlx, m.name).toBe(true);
    }
    for (const m of SUGGESTED_MODELS) {
      expect(m.name, m.name).not.toMatch(/mlx/);
      expect(m.mlx, m.name).toBeFalsy();
    }
  });

  it('no tag appears twice, across both lists', () => {
    const names = [...SUGGESTED_MODELS, ...MLX_SUGGESTED_MODELS].map(m => m.name);
    expect(new Set(names).size).toBe(names.length);
  });

  it('every model descKey + tier label + MLX badge resolves in all UI locales', () => {
    const locales = ['fr', 'en', 'es', 'zh'] as const;
    for (const loc of locales) {
      const dict = dictionaries[loc] as Record<string, string>;
      expect(dict['ollama.mlxBadge'], `${loc}: missing ollama.mlxBadge`).toBeTruthy();
      for (const m of [...SUGGESTED_MODELS, ...MLX_SUGGESTED_MODELS]) {
        expect(dict[m.descKey], `${loc}: missing ${m.descKey}`).toBeTruthy();
        expect(dict[`ollama.tier.${m.tier}`], `${loc}: missing ollama.tier.${m.tier}`).toBeTruthy();
      }
    }
  });

  it('labels automatic resolution without promising an embedded model fallback', () => {
    for (const [locale, expected] of Object.entries({ fr: 'Automatique', en: 'Automatic', es: 'Automático', zh: '自动' })) {
      expect(dictionaries[locale as keyof typeof dictionaries]['ollama.tierAuto']).toBe(expected);
    }
  });
});

describe('suggestedModelsFor — MLX only where the backend says it runs', () => {
  it('is the portable list, unchanged, when MLX is not available', () => {
    expect(suggestedModelsFor(false)).toEqual(SUGGESTED_MODELS);
    expect(suggestedModelsFor(false).some(m => m.mlx)).toBe(false);
  });

  it('puts every MLX build first, then the portable list, when MLX is available', () => {
    const list = suggestedModelsFor(true);
    const firstPortable = list.findIndex(m => !m.mlx);
    expect(firstPortable).toBe(MLX_SUGGESTED_MODELS.length);
    expect(list.slice(0, firstPortable)).toEqual(MLX_SUGGESTED_MODELS);
    expect(list.slice(firstPortable)).toEqual(SUGGESTED_MODELS);
  });

  it('never lets an MLX build push the portable suggestions out', () => {
    for (const portable of SUGGESTED_MODELS) {
      expect(suggestedModelsFor(true)).toContainEqual(portable);
    }
  });
});

describe('every key of the download and update surface exists in all locales', () => {
  it('has the fold summary, the update action and its honest hint', () => {
    const keys = [
      'ollama.pullSummarySuggestions', 'ollama.pullSummaryActive',
      'ollama.updateButton', 'ollama.updateFor', 'ollama.updateHint',
    ];
    for (const loc of ['fr', 'en', 'es', 'zh'] as const) {
      const dict = dictionaries[loc] as Record<string, string>;
      for (const key of keys) expect(dict[key], `${loc}: missing ${key}`).toBeTruthy();
      expect(dict['ollama.pullSummarySuggestions'], loc).toContain('{0}');
      expect(dict['ollama.pullSummaryActive'], loc).toContain('{0}');
      expect(dict['ollama.updateFor'], loc).toContain('{0}');
    }
  });
});
