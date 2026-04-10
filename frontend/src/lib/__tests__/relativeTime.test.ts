import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { formatRelativeTime } from '../relativeTime';

describe('formatRelativeTime', () => {
  beforeEach(() => {
    // Freeze time at 2026-04-10 14:00:00 UTC so assertions are deterministic.
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-04-10T14:00:00Z'));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('returns "à l\'instant" for <60s ago in French', () => {
    expect(formatRelativeTime('2026-04-10T13:59:30Z', 'fr')).toBe("à l'instant");
  });

  it('returns "just now" for <60s ago in English', () => {
    expect(formatRelativeTime('2026-04-10T13:59:30Z', 'en')).toBe('just now');
  });

  it('returns minutes for <60min', () => {
    expect(formatRelativeTime('2026-04-10T13:55:00Z', 'fr')).toBe('il y a 5 min');
    expect(formatRelativeTime('2026-04-10T13:55:00Z', 'en')).toBe('5m ago');
    expect(formatRelativeTime('2026-04-10T13:55:00Z', 'es')).toBe('hace 5 min');
  });

  it('returns hours for <24h', () => {
    expect(formatRelativeTime('2026-04-10T11:00:00Z', 'fr')).toBe('il y a 3 h');
    expect(formatRelativeTime('2026-04-10T11:00:00Z', 'en')).toBe('3h ago');
  });

  it('returns "hier" / "yesterday" for exactly 1 day', () => {
    expect(formatRelativeTime('2026-04-09T14:00:00Z', 'fr')).toBe('hier');
    expect(formatRelativeTime('2026-04-09T14:00:00Z', 'en')).toBe('yesterday');
    expect(formatRelativeTime('2026-04-09T14:00:00Z', 'es')).toBe('ayer');
  });

  it('returns days for 2–6 days ago', () => {
    expect(formatRelativeTime('2026-04-07T14:00:00Z', 'fr')).toBe('il y a 3 j');
    expect(formatRelativeTime('2026-04-07T14:00:00Z', 'en')).toBe('3d ago');
  });

  it('returns short date for ≥ 7 days ago (same year)', () => {
    // 2026-04-01 = 9 days ago from 2026-04-10. Expect "1 avr." / "Apr 1".
    const fr = formatRelativeTime('2026-04-01T14:00:00Z', 'fr');
    const en = formatRelativeTime('2026-04-01T14:00:00Z', 'en');
    // Intl formatting varies slightly between Node versions, so use
    // contains instead of strict equality for the robustness.
    expect(fr).toMatch(/avr/i);
    expect(en).toMatch(/apr/i);
    // No year when same year
    expect(fr).not.toMatch(/2026/);
    expect(en).not.toMatch(/2026/);
  });

  it('includes the year for dates from a different year', () => {
    const fr = formatRelativeTime('2025-12-15T14:00:00Z', 'fr');
    expect(fr).toMatch(/2025/);
  });

  it('handles future dates (clock skew) as "just now"', () => {
    expect(formatRelativeTime('2026-04-10T14:05:00Z', 'fr')).toBe("à l'instant");
  });

  it('returns empty string for invalid dates', () => {
    expect(formatRelativeTime('not-a-date', 'fr')).toBe('');
  });

  it('defaults to French when language is unknown', () => {
    expect(formatRelativeTime('2026-04-10T13:55:00Z', 'de')).toBe('il y a 5 min');
  });
});
