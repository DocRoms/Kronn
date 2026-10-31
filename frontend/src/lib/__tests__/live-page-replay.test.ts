import { describe, expect, it } from 'vitest';
import { replayQueue } from '../live-page-replay';

const link = (id: string, enabled = true) => ({ id, name: id, enabled, step_names: [] });

describe('replayQueue', () => {
  it('keeps enabled workflows and drops disabled and running ones, in link order', () => {
    const linked = [link('a'), link('b', false), link('c'), link('d')];
    expect(replayQueue(linked, id => id === 'c').map(w => w.id)).toEqual(['a', 'd']);
  });

  it('is empty when nothing is linked', () => {
    expect(replayQueue([], () => false)).toEqual([]);
  });
});
