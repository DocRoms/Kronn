import hljs from 'highlight.js/lib/core';
import xml from 'highlight.js/lib/languages/xml';

hljs.registerLanguage('xml', xml);

/** Highlight one HTML line while keeping the editor bundle XML-only. */
export function highlightHtmlLine(line: string): string {
  if (!line) return '';
  try {
    return hljs.highlight(line, { language: 'xml', ignoreIllegals: true }).value;
  } catch {
    return line
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }
}
