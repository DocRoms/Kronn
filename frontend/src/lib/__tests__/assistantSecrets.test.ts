import { describe, it, expect } from 'vitest';
import {
  SECRET_MASK,
  isPlaceholder,
  looksSensitiveName,
  maskSensitiveValues,
  sanitizeForAssistant,
  scrubSecrets,
  secretForms,
} from '../assistantSecrets';

describe('assistantSecrets', () => {
  const secret = 'p@ss word/Ünï';

  it('masks every wire form of a secret', () => {
    const forms = secretForms(secret);
    expect(forms).toContain(secret);
    expect(forms).toContain(encodeURIComponent(secret));
    for (const form of forms) {
      expect(scrubSecrets(`x ${form} y`, [secret])).toBe(`x ${SECRET_MASK} y`);
    }
  });

  it('masks a short secret only as a whole token', () => {
    expect(scrubSecrets('pin 1234 in 912345', ['1234'])).toBe(`pin ${SECRET_MASK} in 912345`);
  });

  it('ignores blank secrets and leaves other text intact', () => {
    expect(scrubSecrets('nothing here', ['', '  '])).toBe('nothing here');
  });

  it('also masks common token shapes not typed in the form', () => {
    const out = sanitizeForAssistant('Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345', []);
    expect(out).not.toContain('abcdefghijklmnopqrstuvwxyz012345');
  });

  it('masks sensitive header and query values but keeps placeholders', () => {
    expect(looksSensitiveName('X-Api-Key')).toBe(true);
    expect(looksSensitiveName('Accept')).toBe(false);
    expect(isPlaceholder('{{token}}')).toBe(true);
    expect(maskSensitiveValues({ Authorization: 'Bearer x', 'X-Token': '{{token}}', Accept: 'json' }))
      .toEqual({ Authorization: SECRET_MASK, 'X-Token': '{{token}}', Accept: 'json' });
  });
});
