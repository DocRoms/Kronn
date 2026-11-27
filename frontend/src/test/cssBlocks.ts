// Read a stylesheet the way the tests check it: comments removed, and the
// body of one `@media (max-width: <px>)` block (jsdom/happy-dom cannot
// evaluate a media query, so the rule is read from the file).
import { readFileSync } from 'node:fs';

export function cssText(path: string): string {
  return readFileSync(path, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
}

/** The body of the `@media (max-width: <px>px)` block of a stylesheet, or ''. */
export function maxWidthBlock(text: string, px: number): string {
  const start = text.indexOf(`@media (max-width: ${px}px)`);
  if (start < 0) return '';
  let depth = 0;
  for (let i = text.indexOf('{', start); i < text.length; i++) {
    if (text[i] === '{') depth++;
    else if (text[i] === '}' && --depth === 0) return text.slice(start, i);
  }
  return '';
}
