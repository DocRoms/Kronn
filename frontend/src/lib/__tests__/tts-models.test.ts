import { describe, it, expect, beforeEach } from 'vitest';
import { TTS_VOICES, getTtsVoiceId, setTtsVoiceId } from '../tts-models';

beforeEach(() => {
  localStorage.clear();
});

describe('TTS_VOICES', () => {
  it('has entries for fr, en, es', () => {
    expect(TTS_VOICES.fr).toBeDefined();
    expect(TTS_VOICES.en).toBeDefined();
    expect(TTS_VOICES.es).toBeDefined();
  });

  it('each language has at least 2 voices', () => {
    for (const lang of ['fr', 'en', 'es']) {
      expect(TTS_VOICES[lang].voices.length).toBeGreaterThanOrEqual(2);
    }
  });

  it('each language has at least one female and one male voice', () => {
    for (const lang of ['fr', 'en', 'es']) {
      const genders = TTS_VOICES[lang].voices.map(v => v.gender);
      expect(genders).toContain('F');
      expect(genders).toContain('M');
    }
  });

  it('default voice exists in the voices list', () => {
    for (const lang of ['fr', 'en', 'es']) {
      const lv = TTS_VOICES[lang];
      expect(lv.voices.some(v => v.id === lv.default)).toBe(true);
    }
  });

  it('all voice IDs are unique', () => {
    const allIds = Object.values(TTS_VOICES).flatMap(lv => lv.voices.map(v => v.id));
    expect(new Set(allIds).size).toBe(allIds.length);
  });
});

describe('getTtsVoiceId', () => {
  it('returns default when nothing stored', () => {
    expect(getTtsVoiceId('fr')).toBe(TTS_VOICES.fr.default);
    expect(getTtsVoiceId('en')).toBe(TTS_VOICES.en.default);
  });

  it('returns stored value when valid', () => {
    const altVoice = TTS_VOICES.fr.voices.find(v => v.id !== TTS_VOICES.fr.default)!;
    setTtsVoiceId('fr', altVoice.id);
    expect(getTtsVoiceId('fr')).toBe(altVoice.id);
  });

  it('ignores invalid stored value and returns default', () => {
    localStorage.setItem('kronn:ttsVoice:fr', 'nonexistent-voice-id');
    expect(getTtsVoiceId('fr')).toBe(TTS_VOICES.fr.default);
  });

  it('falls back to FR default for unknown language', () => {
    expect(getTtsVoiceId('de')).toBe(TTS_VOICES.fr.default);
  });

  it('stores per-language independently', () => {
    const frAlt = TTS_VOICES.fr.voices[1];
    const enAlt = TTS_VOICES.en.voices[1];
    setTtsVoiceId('fr', frAlt.id);
    setTtsVoiceId('en', enAlt.id);
    expect(getTtsVoiceId('fr')).toBe(frAlt.id);
    expect(getTtsVoiceId('en')).toBe(enAlt.id);
    expect(getTtsVoiceId('es')).toBe(TTS_VOICES.es.default); // untouched
  });
});
