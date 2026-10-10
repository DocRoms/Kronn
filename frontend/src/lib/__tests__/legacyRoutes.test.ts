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
    expect(legacyHashToPath('#page/page%2F%C3%A9quipe?tv=1')).toBe('/standalone/pages/page%2F%C3%A9quipe?tv=1');
    expect(legacyHashToPath('#discussion-disc%20a?message=m%201')).toBe('/discussions/disc%20a?message=m%201');
  });

  it.each([
    ['#page/%E0%A4%A', '/pages'],
    ['#page/%E0%A4%A?tv=1', '/pages'],
    ['#discussion-%E0%A4%A', '/discussions'],
    ['#discussion-%E0%A4%A?message=m-1', '/discussions'],
    ['#project-%E0%A4%A', '/projects'],
  ])('sends a link whose id cannot be decoded, %s, to its page', (hash, path) => {
    expect(legacyHashToPath(hash)).toBe(path);
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
