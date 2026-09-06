/**
 * Builds the document used by the source browser's static HTML preview.
 *
 * The CSP is emitted in a document shell we control before any repository
 * content. Sanitising below deliberately complements (rather than replaces)
 * that policy: it removes the interactive and URL-bearing markup that is not
 * useful in a static source preview.
 */
const PREVIEW_CSP = "default-src 'none'; base-uri 'none'; form-action 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:";

const NON_STATIC_ELEMENTS = /<\/?(?:base|embed|fencedframe|form|frame|iframe|link|meta|object|portal|script)\b[^>]*>/gi;
const URL_ATTRIBUTES = /\s(?:action|background|cite|data|formaction|href|imagesrcset|ping|poster|src|srcset|xlink:href)\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'=<>`]+)/gi;

function sanitizeForStaticPreview(html: string): string {
  return html
    // Script contents have no place in a static preview; remove them before
    // dropping the tag so a literal closing tag cannot restore executable JS.
    .replace(/<script\b[^>]*>[\s\S]*?<\/script\s*>/gi, '')
    .replace(NON_STATIC_ELEMENTS, '')
    // A CSP blocks fetches as defence in depth. Removing URL attributes also
    // prevents link navigation, refresh-like controls, and form targets.
    .replace(URL_ATTRIBUTES, '');
}

export function buildHtmlPreviewDocument(html: string): string {
  const staticHtml = sanitizeForStaticPreview(html);
  return `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="${PREVIEW_CSP}"></head><body>${staticHtml}</body></html>`;
}
