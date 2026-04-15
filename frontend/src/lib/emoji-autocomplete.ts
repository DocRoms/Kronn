/**
 * Emoji shortcode autocomplete for the ChatInput.
 *
 * Clones the `@mention` pattern already used in the composer, but matches
 * `:word` anywhere in the textarea (not just at the start) so users can
 * drop `:tada:` mid-sentence — the expected behavior on GitHub, Slack,
 * Discord, etc.
 *
 * The actual rendering of `:tada:` → 🎉 is handled by `remark-emoji` in
 * `MessageBubble`, so the DB / agent context keeps the ASCII shortcode.
 * That way the agent prompt stays portable (CLI agents sometimes choke
 * on raw emoji bytes) and full-text search still matches `tada`.
 */

import * as nodeEmoji from 'node-emoji';

/**
 * Minimum query length before the popover opens. Two chars avoids a
 * popover on every single `:` a user might type (timestamps, URLs,
 * code snippets). Matches the GitHub threshold.
 */
const MIN_QUERY_LEN = 2;

/** Upper bound on popover entries — keeps rendering snappy. */
export const EMOJI_SUGGESTION_LIMIT = 20;

export interface EmojiQuery {
  /** The text between `:` and the cursor, e.g. `"ta"` for `"hello :ta|"`. */
  query: string;
  /** Absolute index of the `:` char in the full text. Used to replace cleanly. */
  colonIndex: number;
}

export interface EmojiSuggestion {
  /** Unicode emoji character(s), e.g. `"🎉"`. */
  emoji: string;
  /** Shortcode WITHOUT surrounding colons, e.g. `"tada"`. */
  shortcode: string;
}

/**
 * Detect an open `:word` query at (or before) the cursor.
 *
 * Rules — tuned so common typing patterns don't trigger the popover
 * spuriously:
 *  - Query must be `\w{MIN_QUERY_LEN,}` (letters / digits / underscore).
 *  - The `:` must be at the start of the text OR preceded by whitespace /
 *    `(` / `[` / `{`. This excludes timestamps `12:30`, URLs `http://`,
 *    time-of-day notes, etc. — `:30` in `12:30` doesn't pop up.
 *  - Returns `null` for anything else.
 *
 * Returns the query and the absolute index of the colon so the caller
 * can replace the segment cleanly without scanning again.
 */
export function findEmojiQuery(
  text: string,
  cursorPos: number,
): EmojiQuery | null {
  if (cursorPos < MIN_QUERY_LEN + 1) return null; // need at least ":xy"
  const before = text.slice(0, cursorPos);

  // Walk backwards from the cursor looking for the nearest `:`. Bail out
  // early if we hit whitespace or too many chars — the popover should
  // never span a huge stretch of text.
  const MAX_LOOKBACK = 40; // generous upper bound; real shortcodes are < 30 chars
  let colonIndex = -1;
  const lowerBound = Math.max(0, before.length - MAX_LOOKBACK);
  for (let i = before.length - 1; i >= lowerBound; i--) {
    const ch = before[i];
    if (ch === ':') { colonIndex = i; break; }
    // Stop walking as soon as we hit anything that isn't a valid shortcode
    // char — whitespace, punctuation, newlines. `\w` covers the rest.
    if (!/[A-Za-z0-9_+-]/.test(ch)) return null;
  }
  if (colonIndex === -1) return null;

  // Character to the left of the colon must be either absent (start of
  // text) or a word-boundary-ish delimiter. This blocks `http://` and
  // `12:30`.
  if (colonIndex > 0) {
    const prev = text[colonIndex - 1];
    if (!/[\s([{<]/.test(prev)) return null;
  }

  const query = before.slice(colonIndex + 1);
  if (query.length < MIN_QUERY_LEN) return null;
  if (!/^[A-Za-z0-9_+-]+$/.test(query)) return null;

  return { query, colonIndex };
}

interface EmojiSearchResult {
  name?: string;
  emoji?: string;
  key?: string;
}

/**
 * Rank `node-emoji.search()` results so exact-prefix matches bubble to
 * the top before substring matches. The library's default ordering is
 * alphabetical, which is less useful for live typing ("ja…" showing
 * `aja:, :aja_meeting:, ...` before `japan`).
 */
export function searchEmojis(
  query: string,
  limit: number = EMOJI_SUGGESTION_LIMIT,
): EmojiSuggestion[] {
  if (!query) return [];
  const q = query.toLowerCase();

  // `nodeEmoji.search` accepts a keyword and returns every shortcode
  // matching it as a substring. We re-rank ourselves.
  const raw: EmojiSearchResult[] = (nodeEmoji.search(q) as EmojiSearchResult[]) ?? [];
  const ranked = raw
    .filter(r => typeof r.emoji === 'string' && typeof (r.name ?? r.key) === 'string')
    .map(r => {
      const shortcode = (r.name ?? r.key) as string;
      // Lower score = better rank.
      let score = 2;
      if (shortcode === q) score = 0;
      else if (shortcode.startsWith(q)) score = 1;
      return { shortcode, emoji: r.emoji as string, score };
    });

  ranked.sort((a, b) => {
    if (a.score !== b.score) return a.score - b.score;
    // Shorter shortcodes win a tie (`:tada:` before `:tadaaaa_special:`).
    if (a.shortcode.length !== b.shortcode.length) {
      return a.shortcode.length - b.shortcode.length;
    }
    return a.shortcode.localeCompare(b.shortcode);
  });

  return ranked
    .slice(0, limit)
    .map(({ shortcode, emoji }) => ({ shortcode, emoji }));
}

/**
 * Replace the `:query` segment with the given Unicode emoji (plus a
 * trailing space so typing continues naturally). Returns the new full
 * text plus the new cursor position — pure, DOM-free, testable.
 *
 * Design note (2026-04-15): earlier iterations inserted the `:shortcode:`
 * form to keep the message text portable for CLI agents. But users
 * expect "what you picked is what you see" (Discord/Slack pattern), so
 * the textarea now gets the actual emoji glyph. `remark-emoji` still
 * runs on the render path for the other direction: agents that output
 * `:tada:` in their replies still render as 🎉 at display time.
 */
export function applyEmojiReplacement(
  text: string,
  query: EmojiQuery,
  emoji: string,
): { text: string; cursor: number } {
  const replacement = `${emoji} `;
  const before = text.slice(0, query.colonIndex);
  const after = text.slice(query.colonIndex + 1 + query.query.length);
  const next = before + replacement + after;
  return { text: next, cursor: before.length + replacement.length };
}
