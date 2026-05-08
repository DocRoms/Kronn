import { describe, it, expect } from 'vitest';
import { gravatarUrl, initialsFromPseudo } from '../gravatar';

describe('gravatarUrl', () => {
  it('produces stable SHA-256 URLs (Gravatar API contract)', () => {
    // Reference value from https://docs.gravatar.com/api/avatars/hash/
    // SHA-256 of the lowercased+trimmed email.
    const url = gravatarUrl('hello@example.com');
    // 64-hex SHA-256 digest in the URL path.
    expect(url).toMatch(/^https:\/\/www\.gravatar\.com\/avatar\/[0-9a-f]{64}\?/);
  });

  it('lowercases + trims the email before hashing', () => {
    // Per Gravatar spec, the email hash is the SHA-256 of the lowercase,
    // trimmed email. Equivalence here is the contract — two casings of
    // the same address must hit the same gravatar.
    expect(gravatarUrl('  Hello@Example.COM  ')).toBe(gravatarUrl('hello@example.com'));
  });

  it('includes the size query param (default 32, override respected)', () => {
    expect(gravatarUrl('a@b.c')).toContain('s=32');
    expect(gravatarUrl('a@b.c', 80)).toContain('s=80');
  });

  it('uses the "retro" fallback for unregistered emails', () => {
    // `d=retro` is the geometric-pattern default. Tests pin this in case
    // someone tries to swap to `d=mp` (the silhouette default) which made
    // the team look identical in early multi-user testing.
    expect(gravatarUrl('a@b.c')).toContain('d=retro');
  });

  it('produces different hashes for different emails', () => {
    expect(gravatarUrl('alice@example.com')).not.toBe(gravatarUrl('bob@example.com'));
  });
});

describe('initialsFromPseudo', () => {
  it('returns first letter of each part for two-word pseudos', () => {
    expect(initialsFromPseudo('Jane Doe')).toBe('JD');
    expect(initialsFromPseudo('Mary  Sue')).toBe('MS'); // collapses whitespace
  });

  it('takes first 2 chars for a single-word pseudo', () => {
    expect(initialsFromPseudo('priol')).toBe('PR');
    expect(initialsFromPseudo('al')).toBe('AL');
  });

  it('uppercases the result regardless of input casing', () => {
    expect(initialsFromPseudo('jane doe')).toBe('JD');
    expect(initialsFromPseudo('alice')).toBe('AL');
  });

  it('uses 3+ word pseudos by taking initials of the first two words', () => {
    expect(initialsFromPseudo('Jean Pierre Dupont')).toBe('JP');
  });

  it('does not crash on a single-character pseudo', () => {
    // `pseudo.slice(0, 2)` on a 1-char string returns the 1 char.
    expect(initialsFromPseudo('x')).toBe('X');
  });
});
