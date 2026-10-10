import { afterEach, describe, expect, it } from 'vitest';
import { parseServedFrameSources, resetServedFrameOriginsForTests, servedFrameOrigins } from '../served-frame-policy';

afterEach(() => {
  document.querySelectorAll('meta[name="kronn-served-frame-src"]').forEach(meta => meta.remove());
  resetServedFrameOriginsForTests();
});

describe('served frame policy marker', () => {
  it('reads exactly the origins the document CSP was served with', () => {
    expect([...parseServedFrameSources("'self' https://suno.com http://localhost:3000")!]).toEqual(['https://suno.com', 'http://localhost:3000']);
    expect([...parseServedFrameSources("'self'")!]).toEqual([]);
  });

  it('treats an absent or malformed marker as unknown', () => {
    for (const value of [null, undefined, '', 'https://suno.com', "'self' *", "'self' https://*.x", "'self'  https://a.example", "'self' https://a.example; script-src *"]) {
      expect(parseServedFrameSources(value), String(value)).toBeNull();
    }
    expect(servedFrameOrigins()).toBeNull();
  });

  it('reads the meta once, from the document head', () => {
    const meta = document.createElement('meta');
    meta.name = 'kronn-served-frame-src';
    meta.content = "'self' https://suno.com";
    document.head.append(meta);
    expect([...servedFrameOrigins()!]).toEqual(['https://suno.com']);
    meta.content = "'self' https://other.example";
    expect([...servedFrameOrigins()!]).toEqual(['https://suno.com']);
  });
});
