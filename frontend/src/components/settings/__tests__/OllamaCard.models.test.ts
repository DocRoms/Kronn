import { describe, it, expect } from 'vitest';
import { SUGGESTED_MODELS } from '../OllamaCard';
import { dictionaries } from '../../../lib/i18n';

// Guards Kronn's hardcoded Ollama first-pull suggestions: real tags, a hardware
// range that includes no-GPU machines, and full FR/EN/ES coverage.
describe('OllamaCard suggested models', () => {
  it('drops the bogus gemma4 entry (was "gemma4:26b" — never existed in the registry)', () => {
    for (const m of SUGGESTED_MODELS) {
      expect(m.name, `unexpected gemma4 tag: ${m.name}`).not.toMatch(/gemma4/);
    }
  });

  it('lists current, real ollama tags spanning the hardware range', () => {
    const names = SUGGESTED_MODELS.map(m => m.name);
    expect(names).toContain('llama3.2:1b');      // CPU / no-GPU floor
    expect(names).toContain('qwen2.5-coder:14b');
    expect(names).toContain('gemma3:27b');
    expect(names).toContain('qwen3:30b');
  });

  it('includes at least one CPU-friendly (no-GPU) option — Kronn runs on WSL boxes too', () => {
    expect(SUGGESTED_MODELS.some(m => m.tier === 'cpu')).toBe(true);
  });

  it('every entry has a valid hardware tier and a sensible size', () => {
    for (const m of SUGGESTED_MODELS) {
      expect(['cpu', 'mid', 'power'], `bad tier for ${m.name}`).toContain(m.tier);
      expect(m.size, `bad size for ${m.name}`).toMatch(/G[Bo]/); // "GB" or "Go"
    }
  });

  it('every model descKey + tier label resolves in all locales (fr/en/es)', () => {
    const locales = ['fr', 'en', 'es'] as const;
    for (const loc of locales) {
      const dict = dictionaries[loc] as Record<string, string>;
      for (const m of SUGGESTED_MODELS) {
        expect(dict[m.descKey], `${loc}: missing ${m.descKey}`).toBeTruthy();
        expect(dict[`ollama.tier.${m.tier}`], `${loc}: missing ollama.tier.${m.tier}`).toBeTruthy();
      }
    }
  });
});
