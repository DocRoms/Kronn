import { describe, expect, it } from 'vitest';
import type { ContextFile } from '../../types/generated';
import {
  clipCostUsd, clipDurationMs, filmTotals, formatUsd, gapAt, moveClip, moveItem, orderVideos, slotForGap,
} from '../videoSequence';

function clip(id: string, createdAt: string): ContextFile {
  return {
    id,
    discussion_id: 'disc-1',
    filename: `${id}.mp4`,
    mime_type: 'video/mp4',
    original_size: 1,
    extracted_size: 1,
    disk_path: `/tmp/${id}.mp4`,
    message_id: null,
    ai_generation: null,
    created_at: createdAt,
  };
}

const ids = (videos: ContextFile[]) => videos.map(video => video.id);

describe('orderVideos', () => {
  const first = clip('first', '2026-09-01T10:00:00Z');
  const second = clip('second', '2026-09-02T10:00:00Z');
  const third = clip('third', '2026-09-03T10:00:00Z');

  it('plays unarranged clips in the order they were made', () => {
    const zones = orderVideos([third, first, second], [], []);
    expect(ids(zones.included)).toEqual(['first', 'second', 'third']);
    expect(zones.excluded).toEqual([]);
  });

  it('keeps an arranged order, and puts clips made since at the end', () => {
    expect(ids(orderVideos([first, second, third], ['third', 'first'], []).included))
      .toEqual(['third', 'first', 'second']);
  });

  it('keeps a clip set aside out of the film, even when it is made later', () => {
    const zones = orderVideos([first, second, third], ['first'], ['third']);
    expect(ids(zones.included)).toEqual(['first', 'second']);
    expect(ids(zones.excluded)).toEqual(['third']);
  });

  it('drops a clip that no longer exists from both lists', () => {
    const zones = orderVideos([first, second], ['gone', 'second', 'first'], ['lost']);
    expect(ids(zones.included)).toEqual(['second', 'first']);
    expect(zones.excluded).toEqual([]);
  });
});

describe('moveItem', () => {
  it('moves one item and leaves the input untouched', () => {
    const items = ['a', 'b', 'c', 'd'];
    expect(moveItem(items, 0, 2)).toEqual(['b', 'c', 'a', 'd']);
    expect(moveItem(items, 3, 1)).toEqual(['a', 'd', 'b', 'c']);
    expect(items).toEqual(['a', 'b', 'c', 'd']);
  });

  it('returns the same array when nothing moves, so no save is sent', () => {
    const items = ['a', 'b'];
    expect(moveItem(items, 1, 1)).toBe(items);
    expect(moveItem(items, 0, -1)).toBe(items);
    expect(moveItem(items, 1, 2)).toBe(items);
  });
});

describe('moveClip', () => {
  const zones = { included: ['a', 'b', 'c'], excluded: ['x'] };

  it('reorders within the film', () => {
    expect(moveClip(zones, { zone: 'included', index: 2 }, { zone: 'included', index: 0 }))
      .toEqual({ included: ['c', 'a', 'b'], excluded: ['x'] });
  });

  it('sets a clip aside, and brings one back where it is dropped', () => {
    expect(moveClip(zones, { zone: 'included', index: 1 }, { zone: 'excluded', index: 1 }))
      .toEqual({ included: ['a', 'c'], excluded: ['x', 'b'] });
    expect(moveClip(zones, { zone: 'excluded', index: 0 }, { zone: 'included', index: 1 }))
      .toEqual({ included: ['a', 'x', 'b', 'c'], excluded: [] });
  });

  it('sends a clip dropped on its own zone to the end of it', () => {
    expect(moveClip(zones, { zone: 'included', index: 0 }, { zone: 'included', index: 3 }))
      .toEqual({ included: ['b', 'c', 'a'], excluded: ['x'] });
  });

  it('returns the same object when nothing moves, so no save is sent', () => {
    expect(moveClip(zones, { zone: 'included', index: 2 }, { zone: 'included', index: 3 })).toBe(zones);
    expect(moveClip(zones, { zone: 'excluded', index: 4 }, { zone: 'included', index: 0 })).toBe(zones);
  });
});

describe('gapAt', () => {
  const row = { top: 100, height: 40 };

  it('points before a row from its upper half, after it from its lower half', () => {
    expect(gapAt('included', 2, 110, row)).toEqual({ zone: 'included', before: 2 });
    expect(gapAt('included', 2, 130, row)).toEqual({ zone: 'included', before: 3 });
  });
});

describe('slotForGap', () => {
  const second = { zone: 'included' as const, index: 1 };

  it('lands where the line was drawn, counting the place the clip leaves', () => {
    expect(slotForGap(second, { zone: 'included', before: 0 })).toEqual({ zone: 'included', index: 0 });
    // Below the next clip: one place down.
    expect(slotForGap(second, { zone: 'included', before: 3 })).toEqual({ zone: 'included', index: 2 });
  });

  it('draws no line on either side of the dragged clip, where it would not move', () => {
    expect(slotForGap(second, { zone: 'included', before: 1 })).toBeNull();
    expect(slotForGap(second, { zone: 'included', before: 2 })).toBeNull();
  });

  it('inserts at the gap in the other zone', () => {
    expect(slotForGap(second, { zone: 'excluded', before: 0 })).toEqual({ zone: 'excluded', index: 0 });
    expect(slotForGap(second, { zone: 'excluded', before: 1 })).toEqual({ zone: 'excluded', index: 1 });
  });

  it('agrees with moveClip on the order that results', () => {
    const zones = { included: ['a', 'b', 'c', 'd'], excluded: [] as string[] };
    const from = { zone: 'included' as const, index: 0 };
    const to = slotForGap(from, { zone: 'included', before: 3 });
    // The line was between c and d: a lands there.
    expect(to && moveClip(zones, from, to).included).toEqual(['b', 'c', 'a', 'd']);
  });
});

describe('film totals', () => {
  const generated = (id: string, durationMs: number | null, costUsd: number | null, isByok = false) => ({
    ...clip(id, '2026-09-01T10:00:00Z'),
    ai_generation: { model: 'provider/video', prompt: id, duration_ms: durationMs, cost_usd: costUsd, is_byok: isByok },
  });

  it('adds the measured lengths and the declared prices', () => {
    const totals = filmTotals([generated('a', 5042, 0.07), generated('b', 8000, 0.12)], {});
    expect(totals).toEqual({ durationMs: 13042, unknownDurations: 0, costUsd: 0.19, uncountedCosts: 0 });
  });

  it('reads an upload\'s length from the browser, and never prices it', () => {
    const upload = clip('upload', '2026-09-01T10:00:00Z');
    expect(clipDurationMs(upload, {})).toBeNull();
    expect(clipDurationMs(upload, { upload: 4200 })).toBe(4200);
    expect(clipCostUsd(upload)).toBeNull();
  });

  it('counts apart, rather than as free, a clip with no price or billed on the user\'s key', () => {
    const totals = filmTotals([generated('a', 6000, 0.2), generated('b', 6000, null), generated('c', null, 0.5, true)], {});
    expect(totals.costUsd).toBe(0.2);
    expect(totals.uncountedCosts).toBe(2);
    expect(totals.durationMs).toBe(12000);
    expect(totals.unknownDurations).toBe(1);
  });

  it('shows a price below a cent with enough digits not to read as free', () => {
    expect(formatUsd(0.0708932)).toBe('$0.07');
    expect(formatUsd(0.0042)).toBe('$0.0042');
    expect(formatUsd(0)).toBe('$0.00');
    expect(formatUsd(1.5)).toBe('$1.50');
  });
});
