import { describe, it, expect } from 'vitest';
import { levelTier } from '../onboarding-progress';

describe('levelTier', () => {
  it('maps French registry levels (accents optional) to tiers', () => {
    expect(levelTier('débutant')).toBe('beginner');
    expect(levelTier('debutant')).toBe('beginner');
    expect(levelTier('Intermédiaire')).toBe('intermediate');
    expect(levelTier('avancé')).toBe('advanced');
    expect(levelTier('AVANCE')).toBe('advanced');
  });

  it('maps common English levels too', () => {
    expect(levelTier('beginner')).toBe('beginner');
    expect(levelTier('Intermediate')).toBe('intermediate');
    expect(levelTier('advanced')).toBe('advanced');
    expect(levelTier('expert')).toBe('advanced');
  });

  it('returns null for empty / unrecognised values (neutral rendering)', () => {
    expect(levelTier(null)).toBeNull();
    expect(levelTier(undefined)).toBeNull();
    expect(levelTier('')).toBeNull();
    expect(levelTier('non précisé')).toBeNull();
    expect(levelTier('wizard')).toBeNull();
  });
});
