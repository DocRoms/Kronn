import { describe, expect, it } from 'vitest';
import { quoteMultilinePaste } from '../quoteMultilinePaste';

describe('quoteMultilinePaste', () => {
  it('prefixes continuation lines when the caret line is a blockquote', () => {
    expect(quoteMultilinePaste('> ', 'line1\nline2\nline3')).toBe('line1\n> line2\n> line3');
  });

  it('works when the blockquote already has text before the caret', () => {
    expect(quoteMultilinePaste('> some intro ', 'a\nb')).toBe('a\n> b');
  });

  it('tolerates leading whitespace before the > marker', () => {
    expect(quoteMultilinePaste('   > ', 'x\ny')).toBe('x\n> y');
  });

  it('normalises CRLF so no stray \\r leaks into the quote', () => {
    expect(quoteMultilinePaste('> ', 'a\r\nb')).toBe('a\n> b');
  });

  it('returns null for a single-line paste (let the native paste run)', () => {
    expect(quoteMultilinePaste('> ', 'just one line')).toBeNull();
  });

  it('returns null when the caret line is NOT a blockquote', () => {
    expect(quoteMultilinePaste('normal text ', 'a\nb')).toBeNull();
    expect(quoteMultilinePaste('', 'a\nb')).toBeNull();
    // ">" without the trailing space is not a blockquote marker in our flow.
    expect(quoteMultilinePaste('>no space', 'a\nb')).toBeNull();
  });
});
