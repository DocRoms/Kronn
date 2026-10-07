import { describe, expect, it } from 'vitest';
import { legacyHashToPath } from '../legacyRoutes';

describe('legacyHashToPath', () => {
  it.each([
    ['#page/page-1', '/standalone/pages/page-1'],
    ['#page/page%2F%C3%A9quipe', '/standalone/pages/page%2F%C3%A9quipe'],
    ['#pages/mosaic?page=a&page=b&layout=two-columns', '/standalone/pages/mosaic?page=a&page=b&layout=two-columns'],
    ['#discussions/mosaic?discussion=a&discussion=b&layout=auto', '/standalone/discussions/mosaic?discussion=a&discussion=b&layout=auto'],
    ['#config', '/config'],
    ['#project-proj-1', '/projects/proj-1'],
    ['#discussion-disc-1', '/discussions/disc-1'],
    ['#discussion-disc%2F%C3%A9?message=msg%2F1', '/discussions/disc%2F%C3%A9?message=msg%2F1'],
  ])('redirects %s to %s', (hash, path) => {
    expect(legacyHashToPath(hash)).toBe(path);
  });

  it('keeps the encoding of what follows the prefix untouched', () => {
    // The route decodes it once, exactly as the browser did for the hash.
    expect(legacyHashToPath('#page/%E0%A4%A')).toBe('/standalone/pages/%E0%A4%A');
  });

  it.each([
    [''],
    ['#'],
    ['#page/'],
    ['#project-'],
    ['#discussion-'],
    ['#pages/mosaic?'],
    ['#discussions/mosaic?'],
    ['#pages/mosaic'],
    ['#config/agents'],
    ['#configuration'],
    ['#settings-api-audit'],
    ['#pagex/1'],
    ['page/page-1'],
  ])('leaves %j alone', (hash) => {
    expect(legacyHashToPath(hash)).toBeNull();
  });
});
