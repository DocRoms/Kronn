import { describe, expect, it } from 'vitest';
import { notCarriedIn } from '../exportReport';

const item = { kind: 'skill', id: 'custom-gone', named_in: 'PR review › publish' };

describe('notCarriedIn', () => {
  it('reads the list of an exported file', () => {
    expect(notCarriedIn(JSON.stringify({ version: 1, not_carried: [item] }))).toEqual([item]);
  });

  it('drops malformed entries', () => {
    const content = JSON.stringify({ not_carried: [item, null, 'x', { kind: 'skill', id: 3, named_in: 'a' }, { kind: 'skill', id: 'b' }] });
    expect(notCarriedIn(content)).toEqual([item]);
  });

  it('is empty for a file without the list, with another shape, or that is no JSON', () => {
    expect(notCarriedIn('{"version":1}')).toEqual([]);
    expect(notCarriedIn('{"not_carried":{"kind":"skill"}}')).toEqual([]);
    expect(notCarriedIn('null')).toEqual([]);
    expect(notCarriedIn('not json')).toEqual([]);
  });
});
