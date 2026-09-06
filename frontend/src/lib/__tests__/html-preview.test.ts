import { describe, expect, it } from 'vitest';
import { buildHtmlPreviewDocument } from '../html-preview';

describe('static HTML preview', () => {
  it.each(['set', 'animate', 'animateMotion', 'animateTransform', 'discard', 'animation'])(
    'removes SVG %s mutations while retaining static SVG content',
    tag => {
      const source = `<svg xmlns="http://www.w3.org/2000/svg"><a><text x="0" y="30">Open</text><${tag} attributeName="href" to="https://preview-probe.invalid/animated" values="https://preview-probe.invalid/animated" /></a><rect width="10" height="10" /></svg>`;
      const preview = buildHtmlPreviewDocument(source);
      const parsed = new DOMParser().parseFromString(preview, 'text/html');
      expect(parsed.querySelector(tag)).toBeNull();
      expect(preview).not.toContain('preview-probe.invalid');
      expect(parsed.querySelector('text')?.textContent).toBe('Open');
      expect(parsed.querySelector('rect')?.getAttribute('width')).toBe('10');
    },
  );

  it('removes template contents instead of preserving declarative shadow roots', () => {
    const preview = buildHtmlPreviewDocument('<div><template shadowrootmode="open"><a href="https://preview-probe.invalid/shadow">Hidden link</a></template><p>Visible</p></div>');
    expect(preview).not.toContain('<template');
    expect(preview).not.toContain('Hidden link');
    expect(preview).toContain('<p>Visible</p>');
  });
});
