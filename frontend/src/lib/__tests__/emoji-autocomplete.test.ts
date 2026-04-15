import { describe, it, expect } from 'vitest';
import {
  findEmojiQuery,
  searchEmojis,
  applyEmojiReplacement,
  EMOJI_SUGGESTION_LIMIT,
} from '../emoji-autocomplete';

describe('findEmojiQuery', () => {
  it('detects a query at the start of the text', () => {
    const q = findEmojiQuery(':ta', 3);
    expect(q).toEqual({ query: 'ta', colonIndex: 0 });
  });

  it('detects a query mid-sentence preceded by a space', () => {
    const text = 'hello :sm';
    const q = findEmojiQuery(text, text.length);
    expect(q).toEqual({ query: 'sm', colonIndex: 6 });
  });

  it('returns null when the query is shorter than the 2-char threshold', () => {
    expect(findEmojiQuery(':t', 2)).toBeNull();
    expect(findEmojiQuery(':', 1)).toBeNull();
    expect(findEmojiQuery('', 0)).toBeNull();
  });

  it('returns null when no colon is open before the cursor', () => {
    expect(findEmojiQuery('hello world', 11)).toBeNull();
  });

  it('ignores colons preceded by a non-delimiter (blocks URLs and times)', () => {
    // Timestamps like "12:30" must not trigger the popover.
    expect(findEmojiQuery('12:30', 5)).toBeNull();
    // URLs like "http://example" (after "p://") — the "://" sequence
    // means the colon is followed by `/` which is not a shortcode char,
    // so matching stops earlier anyway.
    expect(findEmojiQuery('see http://ex', 'see http://ex'.length)).toBeNull();
  });

  it('returns null if a non-word char sits inside the would-be shortcode', () => {
    // "he: llo" — the space between `:` and `llo` breaks the match.
    expect(findEmojiQuery('he: llo', 7)).toBeNull();
  });

  it('stops the backward walk after MAX_LOOKBACK chars', () => {
    // 50 chars of `x` with no colon — should not freeze or match anything.
    const text = ':' + 'x'.repeat(60);
    expect(findEmojiQuery(text, text.length)).toBeNull();
  });

  it('accepts colon right after an opening bracket (GitHub pattern)', () => {
    // "(:ta" is a legit spot: you might be inside an MD link title.
    const q = findEmojiQuery('(:ta', 4);
    expect(q).toEqual({ query: 'ta', colonIndex: 1 });
  });

  it('respects cursor position — ignores text AFTER the cursor', () => {
    const text = 'hi :ta and :sm';
    // Cursor positioned right after `:ta` (index 6). Must match `ta`,
    // not run across to `:sm`.
    const q = findEmojiQuery(text, 6);
    expect(q).toEqual({ query: 'ta', colonIndex: 3 });
  });
});

describe('searchEmojis', () => {
  it('returns suggestions ranked by prefix match first', () => {
    const results = searchEmojis('tad');
    expect(results.length).toBeGreaterThan(0);
    // `tada` starts with `tad` → must be among the first results.
    const tadaIdx = results.findIndex(r => r.shortcode === 'tada');
    expect(tadaIdx).toBeGreaterThanOrEqual(0);
    expect(tadaIdx).toBeLessThanOrEqual(2);
  });

  it('each result carries both the shortcode and the emoji character', () => {
    const results = searchEmojis('sm');
    expect(results.length).toBeGreaterThan(0);
    for (const r of results) {
      expect(typeof r.shortcode).toBe('string');
      expect(r.shortcode.length).toBeGreaterThan(0);
      expect(typeof r.emoji).toBe('string');
      // At least one Unicode codepoint — emoji rarely fit a single char.
      expect(r.emoji.length).toBeGreaterThan(0);
    }
  });

  it('caps output at EMOJI_SUGGESTION_LIMIT by default', () => {
    // `e` is a common substring → yields many matches.
    const results = searchEmojis('e');
    expect(results.length).toBeLessThanOrEqual(EMOJI_SUGGESTION_LIMIT);
  });

  it('honours an explicit limit argument', () => {
    const results = searchEmojis('e', 3);
    expect(results.length).toBeLessThanOrEqual(3);
  });

  it('returns [] for empty query', () => {
    expect(searchEmojis('')).toEqual([]);
  });

  it('exact-match shortcode ranks first', () => {
    const results = searchEmojis('heart');
    // `heart` exists as a shortcode — must be the #1 result despite
    // many substring matches (`:heart_eyes:`, `:green_heart:`, etc.).
    expect(results[0]?.shortcode).toBe('heart');
  });
});

describe('applyEmojiReplacement', () => {
  it('replaces :query with the Unicode emoji glyph + trailing space', () => {
    const text = ':ta';
    const query = { query: 'ta', colonIndex: 0 };
    const out = applyEmojiReplacement(text, query, '🎉');
    expect(out.text).toBe('🎉 ');
    // Cursor sits right after the inserted glyph + space so typing
    // continues naturally. `.length` on a single-code-point emoji
    // counts UTF-16 surrogates, mirroring textarea semantics.
    expect(out.cursor).toBe('🎉 '.length);
  });

  it('preserves text before and after the replaced segment', () => {
    const text = 'hey :ta and more';
    const query = { query: 'ta', colonIndex: 4 };
    const out = applyEmojiReplacement(text, query, '🎉');
    expect(out.text).toBe('hey 🎉  and more');
    expect(out.cursor).toBe('hey 🎉 '.length);
  });

  it('handles multi-codepoint emoji (skin tones / ZWJ sequences)', () => {
    const text = 'send :heart';
    const query = { query: 'heart', colonIndex: 5 };
    // Red heart (U+2764 U+FE0F) has a variation selector — the helper
    // must not split it on any arbitrary index.
    const out = applyEmojiReplacement(text, query, '❤️');
    expect(out.text).toBe('send ❤️ ');
  });
});
