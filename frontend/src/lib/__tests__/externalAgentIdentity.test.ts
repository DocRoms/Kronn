import { describe, it, expect } from 'vitest';
import { externalAgentColor } from '../externalAgentIdentity';

describe('externalAgentColor', () => {
  it('gives two connections two different colours', () => {
    // Every external connection is `Custom` on the wire, so the static colour
    // table gave them all the same fallback violet.
    const a = externalAgentColor('@openrouter');
    const b = externalAgentColor('@groq');
    expect(a).not.toBeNull();
    expect(a).not.toBe(b);
  });

  it('is stable for a given alias', () => {
    expect(externalAgentColor('@openrouter')).toBe(externalAgentColor('openrouter'));
  });

  it('returns null with nothing to derive from, so the caller keeps its fallback', () => {
    expect(externalAgentColor(null)).toBeNull();
    expect(externalAgentColor('  ')).toBeNull();
    expect(externalAgentColor('@')).toBeNull();
  });

  it('stays within a legible band on both themes', () => {
    // Only the hue varies; saturation and lightness are fixed on purpose.
    expect(externalAgentColor('@anything')).toMatch(/^hsl\(\d{1,3}, 62%, 52%\)$/);
  });
});
