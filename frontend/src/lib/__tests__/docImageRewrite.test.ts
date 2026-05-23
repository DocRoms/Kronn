import { describe, it, expect } from 'vitest';
import { resolveDocAssetUrl, rehypeRewriteDocImages } from '../docImageRewrite';

const P = 'p1';

describe('resolveDocAssetUrl', () => {
  it('rewrites a relative path from a root README (baseDir empty)', () => {
    expect(resolveDocAssetUrl(P, '', 'docs/screenshots/foo.png', '')).toBe(
      '/api/projects/p1/doc-asset?path=docs%2Fscreenshots%2Ffoo.png',
    );
  });

  it('resolves `..` against the current doc directory', () => {
    expect(resolveDocAssetUrl(P, 'docs/architecture/sequences', '../../diagram.png', '')).toBe(
      '/api/projects/p1/doc-asset?path=docs%2Fdiagram.png',
    );
  });

  it('resolves `./` against the current doc directory', () => {
    expect(resolveDocAssetUrl(P, 'docs', './img/x.png', '')).toBe(
      '/api/projects/p1/doc-asset?path=docs%2Fimg%2Fx.png',
    );
  });

  it('treats a leading slash as repo-root-absolute', () => {
    expect(resolveDocAssetUrl(P, 'docs/x', '/logo.png', '')).toBe(
      '/api/projects/p1/doc-asset?path=logo.png',
    );
  });

  it('prefixes the configured API base', () => {
    expect(resolveDocAssetUrl(P, '', 'a.png', 'http://host:3140')).toBe(
      'http://host:3140/api/projects/p1/doc-asset?path=a.png',
    );
  });

  it('leaves external / data / protocol-relative URLs untouched (returns null)', () => {
    expect(resolveDocAssetUrl(P, '', 'https://img.shields.io/badge/x', '')).toBeNull();
    expect(resolveDocAssetUrl(P, '', 'http://e/x.png', '')).toBeNull();
    expect(resolveDocAssetUrl(P, '', 'data:image/png;base64,AAAA', '')).toBeNull();
    expect(resolveDocAssetUrl(P, '', '//cdn.example.com/x.png', '')).toBeNull();
  });

  it('returns null when the path escapes the project root', () => {
    expect(resolveDocAssetUrl(P, 'docs', '../../../etc/passwd.png', '')).toBeNull();
    expect(resolveDocAssetUrl(P, '', '', '')).toBeNull();
  });
});

describe('rehypeRewriteDocImages', () => {
  it('rewrites relative img src nodes and leaves external ones alone', () => {
    const tree = {
      type: 'root',
      children: [
        { type: 'element', tagName: 'img', properties: { src: 'docs/a.png' }, children: [] },
        { type: 'element', tagName: 'img', properties: { src: 'https://x/b.png' }, children: [] },
      ],
    };
    rehypeRewriteDocImages({ projectId: P, baseDir: '', apiBase: '' })(tree);
    expect(tree.children[0].properties.src).toBe('/api/projects/p1/doc-asset?path=docs%2Fa.png');
    expect(tree.children[1].properties.src).toBe('https://x/b.png'); // untouched
  });
});
