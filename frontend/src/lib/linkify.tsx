import React from 'react';

/**
 * Turn plain text containing URLs into an array of React nodes where each
 * `http(s)://…` run becomes a clickable, `noopener` external link and the
 * surrounding text is left as plain strings.
 *
 * Extracted from McpPage (the secrets/help panel) so the URL-splitting
 * regex can be unit-tested against edge cases (no URL, leading/trailing
 * URL, multiple URLs, trailing punctuation) without mounting the page.
 *
 * The split keeps capture groups so the URL tokens land in the array;
 * `urlRe.test()` per part re-classifies them. `urlRe` is declared with
 * the `g` flag — `.test()` is only called on individual parts (never in a
 * loop that depends on `lastIndex`), so the stateful-regex footgun does
 * not apply here.
 */
export function linkify(text: string): React.ReactNode[] {
  const urlRe = /(https?:\/\/[^\s)]+)/g;
  const parts = text.split(urlRe);
  return parts.map((part, i) =>
    urlRe.test(part)
      ? <a key={i} href={part} target="_blank" rel="noopener noreferrer" className="mcp-secrets-token-link" style={{ display: 'inline' }}>{part}</a>
      : part
  );
}
