import { describe, it, expect } from 'vitest';
import { isValidElement } from 'react';
import { linkify } from '../linkify';

/** Count how many parts are anchor elements vs plain strings. */
function classify(nodes: ReturnType<typeof linkify>) {
  const anchors = nodes.filter(n => isValidElement(n));
  const strings = nodes.filter(n => typeof n === 'string' && n !== '');
  return { anchors, strings };
}

describe('linkify', () => {
  it('returns a single plain-string part when there is no URL', () => {
    const { anchors, strings } = classify(linkify('just some text'));
    expect(anchors).toHaveLength(0);
    expect(strings.map(s => s as string)).toEqual(['just some text']);
  });

  it('wraps a lone URL in an anchor with the right href + security attrs', () => {
    const { anchors } = classify(linkify('https://example.com/docs'));
    expect(anchors).toHaveLength(1);
    const a = anchors[0] as React.ReactElement<React.AnchorHTMLAttributes<HTMLAnchorElement>>;
    expect(a.props.href).toBe('https://example.com/docs');
    expect(a.props.target).toBe('_blank');
    expect(a.props.rel).toBe('noopener noreferrer');
    expect(a.props.children).toBe('https://example.com/docs');
  });

  it('splits text around a URL in the middle', () => {
    const { anchors, strings } = classify(linkify('see https://x.io now'));
    expect(anchors).toHaveLength(1);
    // "see " and " now" remain as plain text
    expect(strings.map(s => s as string)).toEqual(['see ', ' now']);
  });

  it('handles multiple URLs', () => {
    const { anchors } = classify(linkify('a https://one.com b https://two.com c'));
    expect(anchors).toHaveLength(2);
    const hrefs = anchors.map(a => (a as React.ReactElement<{ href: string }>).props.href);
    expect(hrefs).toEqual(['https://one.com', 'https://two.com']);
  });

  it('matches http as well as https', () => {
    const { anchors } = classify(linkify('http://insecure.local/x'));
    expect(anchors).toHaveLength(1);
  });

  it('stops the URL at a closing paren (does not swallow trailing ")")', () => {
    const { anchors } = classify(linkify('(see https://x.io)'));
    expect(anchors).toHaveLength(1);
    expect((anchors[0] as React.ReactElement<{ href: string }>).props.href).toBe('https://x.io');
  });

  it('returns an empty-ish array for empty input without throwing', () => {
    expect(() => linkify('')).not.toThrow();
  });
});
