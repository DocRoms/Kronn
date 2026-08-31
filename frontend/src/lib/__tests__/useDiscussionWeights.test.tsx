/**
 * KT-541 — the loader's two hard guarantees.
 *
 * Disabling the indicator must remove the cost, not just the pixels: summing
 * message content scans the messages table, so a disabled indicator that still
 * queried would be the worst of both worlds.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor, cleanup } from '@testing-library/react';

const getServerConfig = vi.fn();
const discussionWeights = vi.fn();

vi.mock('../api', () => ({
  config: {
    getServerConfig: (...a: unknown[]) => getServerConfig(...a),
    discussionWeights: (...a: unknown[]) => discussionWeights(...a),
  },
}));

const { useDiscussionWeights, boundedWeightIds, MAX_BATCH } = await import('../useDiscussionWeights');

beforeEach(() => {
  getServerConfig.mockReset();
  discussionWeights.mockReset();
});
afterEach(cleanup);

const enabledConfig = { discussion_weight: { enabled: true, amber_bytes: 100, red_bytes: 200 } };
const disabledConfig = { discussion_weight: { enabled: false, amber_bytes: 100, red_bytes: 200 } };

function weightsPage(ids: string[]) {
  return {
    weights: Object.fromEntries(
      ids.map(id => [id, { discussion_id: id, disk_bytes: 1, extracted_text_bytes: 0, message_bytes: 0, total_bytes: 1, reclaimable_bytes: 1, level: 'green' }]),
    ),
    thresholds: { amber_bytes: 100, red_bytes: 200 },
    thresholds_from_defaults: false,
  };
}

describe('boundedWeightIds', () => {
  it('caps the selection at the server limit', () => {
    const many = Array.from({ length: 900 }, (_, i) => ({ id: `d${i}` }));
    const ids = boundedWeightIds(many);
    expect(ids).toHaveLength(MAX_BATCH);
    expect(ids[0]).toBe('d0');
    // A list view reads from the top; the tail is simply not requested.
    expect(ids).not.toContain('d899');
  });

  it('passes a short list through untouched', () => {
    expect(boundedWeightIds([{ id: 'a' }, { id: 'b' }])).toEqual(['a', 'b']);
    expect(boundedWeightIds([])).toEqual([]);
  });
});

describe('useDiscussionWeights', () => {
  it('never reports ready before the config is known', async () => {
    // Regression: treating "config not read yet" as "disabled" returned a
    // settled state with empty weights, so every card rendered a
    // measured-looking "0 o" during the first paint.
    let release: (v: unknown) => void = () => {};
    getServerConfig.mockReturnValue(new Promise(r => { release = r; }));

    const { result } = renderHook(() => useDiscussionWeights(['a']));
    expect(result.current.state).toBe('loading');
    expect(result.current.enabled).toBe(false);
    expect(result.current.weights).toEqual({});

    release(enabledConfig);
    await waitFor(() => expect(result.current.enabled).toBe(true));
  });


  it('reports rows beyond the bound as unmeasured, never as a settled zero', async () => {
    // The bound limits the QUERY, not the list. Treating the remainder as
    // ready-and-empty made every discussion past the 200th render "0 o".
    getServerConfig.mockResolvedValue(enabledConfig);
    discussionWeights.mockImplementation((ids: string[]) => Promise.resolve(weightsPage(ids)));

    const all = Array.from({ length: 260 }, (_, i) => ({ id: `d${String(i).padStart(3, '0')}` }));
    const asked = boundedWeightIds(all);
    const { result } = renderHook(() => useDiscussionWeights(asked));
    await waitFor(() => expect(result.current.state).toBe('ready'));

    const inside = asked[0];
    const outside = all.at(-1)!.id;
    expect(asked).not.toContain(outside);

    expect(result.current.stateFor(inside)).toBe('ready');
    expect(result.current.stateFor(outside)).toBe('unmeasured');
    // And nothing pretends to know its weight.
    expect(result.current.weights[outside]).toBeUndefined();
  });

  it('never reports ready for any row while the batch is still loading', async () => {
    let release: (v: unknown) => void = () => {};
    getServerConfig.mockResolvedValue(enabledConfig);
    discussionWeights.mockReturnValue(new Promise(r => { release = r; }));

    const { result } = renderHook(() => useDiscussionWeights(['a', 'b']));
    await waitFor(() => expect(result.current.enabled).toBe(true));
    expect(result.current.stateFor('a')).toBe('loading');
    // Even an id that was never in the batch stays non-committal.
    expect(result.current.stateFor('zzz')).toBe('loading');

    release(weightsPage(['a', 'b']));
    await waitFor(() => expect(result.current.stateFor('a')).toBe('ready'));
    expect(result.current.stateFor('zzz')).toBe('unmeasured');
  });

  it('issues ZERO batch call when the indicator is disabled', async () => {
    getServerConfig.mockResolvedValue(disabledConfig);
    const { result } = renderHook(() => useDiscussionWeights(['a', 'b']));

    await waitFor(() => expect(getServerConfig).toHaveBeenCalled());
    await new Promise(r => setTimeout(r, 0));

    expect(discussionWeights).not.toHaveBeenCalled();
    expect(result.current.enabled).toBe(false);
  });

  it('issues no batch call either when the config cannot be read', async () => {
    getServerConfig.mockRejectedValue(new Error('offline'));
    renderHook(() => useDiscussionWeights(['a']));

    await waitFor(() => expect(getServerConfig).toHaveBeenCalled());
    await new Promise(r => setTimeout(r, 0));

    expect(discussionWeights).not.toHaveBeenCalled();
  });

  it('loads the requested ids once enabled', async () => {
    getServerConfig.mockResolvedValue(enabledConfig);
    discussionWeights.mockImplementation((ids: string[]) => Promise.resolve(weightsPage(ids)));

    const { result } = renderHook(() => useDiscussionWeights(['a', 'b']));
    await waitFor(() => expect(result.current.state).toBe('ready'));

    expect(discussionWeights).toHaveBeenCalledTimes(1);
    expect(result.current.weights.a).toBeTruthy();
    expect(result.current.weights.b).toBeTruthy();
  });

  it('slices past the server cap instead of sending one oversized request', async () => {
    getServerConfig.mockResolvedValue(enabledConfig);
    discussionWeights.mockImplementation((ids: string[]) => Promise.resolve(weightsPage(ids)));

    const ids = Array.from({ length: 450 }, (_, i) => `d${String(i).padStart(3, '0')}`);
    const { result } = renderHook(() => useDiscussionWeights(ids));
    await waitFor(() => expect(result.current.state).toBe('ready'));

    expect(discussionWeights).toHaveBeenCalledTimes(3);
    for (const call of discussionWeights.mock.calls) {
      expect((call[0] as string[]).length).toBeLessThanOrEqual(200);
    }
  });

  it('degrades to unavailable rather than reporting empty weights', async () => {
    getServerConfig.mockResolvedValue(enabledConfig);
    discussionWeights.mockRejectedValue(new Error('boom'));

    const { result } = renderHook(() => useDiscussionWeights(['a']));
    await waitFor(() => expect(result.current.state).toBe('unavailable'));
    // No fabricated zeros: the map stays empty and the state says why.
    expect(Object.keys(result.current.weights)).toHaveLength(0);
  });

  it('does not refetch when the same ids arrive in a different order', async () => {
    getServerConfig.mockResolvedValue(enabledConfig);
    discussionWeights.mockImplementation((ids: string[]) => Promise.resolve(weightsPage(ids)));

    const { result, rerender } = renderHook(({ ids }) => useDiscussionWeights(ids), {
      initialProps: { ids: ['a', 'b'] },
    });
    await waitFor(() => expect(result.current.state).toBe('ready'));
    expect(discussionWeights).toHaveBeenCalledTimes(1);

    rerender({ ids: ['b', 'a'] });
    await new Promise(r => setTimeout(r, 0));
    expect(discussionWeights).toHaveBeenCalledTimes(1);
  });
});
