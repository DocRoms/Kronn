/**
 * Builds the document used by the source browser's static HTML preview.
 *
 * Repository markup is parsed in an inert document, reduced to static nodes,
 * then serialized inside a document shell whose policy is emitted first.
 */
const PREVIEW_CSP = "default-src 'none'; base-uri 'none'; form-action 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:";

const BLOCKED_ELEMENTS = new Set([
  'base', 'embed', 'fencedframe', 'form', 'frame', 'iframe', 'link', 'meta',
  'object', 'portal', 'script',
]);

const URL_ATTRIBUTES = new Set([
  'action', 'background', 'cite', 'data', 'formaction', 'href', 'imagesrcset',
  'ping', 'poster', 'src', 'srcset', 'xlink:href',
]);

function isSafeDataImage(element: Element, attribute: Attr): boolean {
  return element.localName === 'img'
    && attribute.name.toLowerCase() === 'src'
    && /^data:image\//i.test(attribute.value.trim());
}

function sanitizeForStaticPreview(html: string): { head: string; body: string } {
  // Parsing a string with DOMParser creates a detached, inert document. No
  // repository node is connected to the preview frame until after this pass.
  const parsed = new DOMParser().parseFromString(html, 'text/html');
  for (const element of Array.from(parsed.querySelectorAll('*'))) {
    if (BLOCKED_ELEMENTS.has(element.localName)) {
      element.remove();
      continue;
    }

    for (const attribute of Array.from(element.attributes)) {
      const name = attribute.name.toLowerCase();
      if (name.startsWith('on') || URL_ATTRIBUTES.has(name)) {
        if (!isSafeDataImage(element, attribute)) element.removeAttribute(attribute.name);
      }
    }
  }

  return { head: parsed.head.innerHTML, body: parsed.body.innerHTML };
}

export function buildHtmlPreviewDocument(html: string): string {
  const staticHtml = sanitizeForStaticPreview(html);
  return `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="${PREVIEW_CSP}">${staticHtml.head}</head><body>${staticHtml.body}</body></html>`;
}
