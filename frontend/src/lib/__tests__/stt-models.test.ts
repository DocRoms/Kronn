import { describe, it, expect, beforeEach } from 'vitest';
import { STT_MODELS, DEFAULT_STT_MODEL, getSttModelId, setSttModelId } from '../stt-models';

beforeEach(() => {
  localStorage.clear();
});

describe('STT_MODELS', () => {
  it('has at least 2 models', () => {
    expect(STT_MODELS.length).toBeGreaterThanOrEqual(2);
  });

  it('each model has id, label, size, description', () => {
    for (const m of STT_MODELS) {
      expect(m.id).toBeTruthy();
      expect(m.label).toBeTruthy();
      expect(m.size).toBeTruthy();
      expect(m.description).toBeTruthy();
    }
  });

  it('default model is the first (smallest)', () => {
    expect(DEFAULT_STT_MODEL).toBe(STT_MODELS[0].id);
  });

  it('all model IDs are unique', () => {
    const ids = STT_MODELS.map(m => m.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe('getSttModelId', () => {
  it('returns default when nothing stored', () => {
    expect(getSttModelId()).toBe(DEFAULT_STT_MODEL);
  });

  it('returns stored value when valid', () => {
    const alt = STT_MODELS[1];
    setSttModelId(alt.id);
    expect(getSttModelId()).toBe(alt.id);
  });

  it('ignores invalid stored value and returns default', () => {
    localStorage.setItem('kronn:sttModel', 'nonexistent-model');
    expect(getSttModelId()).toBe(DEFAULT_STT_MODEL);
  });
});
