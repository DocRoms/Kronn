/**
 * Blockquote-aware multiline paste.
 *
 * When the caret sits on a Markdown blockquote line (`> …`) and the pasted text
 * spans multiple lines, the default paste only quotes the first line — lines 2+
 * land flush-left and break out of the quote. This prefixes every continuation
 * line with `> ` so the whole paste stays inside the blockquote.
 *
 * Returns the transformed text, or `null` when no transform applies (single
 * line, or the caret line isn't a blockquote) so the caller can let the native
 * paste run untouched.
 *
 * @param currentLine the text of the line the caret is on, from line start up
 *                     to the caret (i.e. `value.slice(lineStart, caret)`)
 * @param pasted      the raw clipboard text
 */
export function quoteMultilinePaste(currentLine: string, pasted: string): string | null {
  if (!pasted.includes('\n')) return null;        // single line — nothing to do
  if (!/^\s*>\s/.test(currentLine)) return null;   // caret not in a `> ` blockquote
  // Normalise CRLF so we don't emit "\r\n> "; prefix each subsequent line.
  return pasted.replace(/\r\n?|\n/g, '\n> ');
}
