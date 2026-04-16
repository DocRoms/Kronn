import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, waitFor, act } from '@testing-library/react';
import { useQpChain } from '../useQpChain';
import type { QuickPrompt } from '../../types/generated';

const listMock = vi.fn();

vi.mock('../../lib/api', () => ({
  quickPrompts: {
    list: () => listMock(),
  },
}));

function makeQP(overrides: Partial<QuickPrompt> = {}): QuickPrompt {
  return {
    id: 'qp-1',
    name: 'Review',
    icon: '🔍',
    description: 'Agent review',
    prompt_template: 'Please review.',
    variables: [],
    category: null,
    ...overrides,
  } as unknown as QuickPrompt;
}

describe('useQpChain', () => {
  beforeEach(() => {
    listMock.mockReset();
  });

  it('loads QPs once and exposes only chainable ones (no variables)', async () => {
    const chainable = makeQP({ id: 'c1', variables: [] });
    const withVars = makeQP({
      id: 'v1',
      variables: [{ name: 'x', description: 'test' }],
    } as unknown as Partial<QuickPrompt>);
    listMock.mockResolvedValue([chainable, withVars]);

    const { result } = renderHook(() =>
      useQpChain({ sending: false, onFire: vi.fn() }),
    );

    await waitFor(() => expect(result.current.chainableQPs.length).toBe(1));
    expect(result.current.chainableQPs[0].id).toBe('c1');
    expect(listMock).toHaveBeenCalledOnce();
  });

  it('swallows list() errors silently (feature is non-critical)', async () => {
    listMock.mockRejectedValue(new Error('boom'));
    const { result } = renderHook(() =>
      useQpChain({ sending: false, onFire: vi.fn() }),
    );
    // Give the promise a tick to settle.
    await waitFor(() => expect(listMock).toHaveBeenCalled());
    expect(result.current.chainableQPs).toEqual([]);
  });

  it('queueQP sets the queued QP; cancelQueuedQP clears it', () => {
    listMock.mockResolvedValue([]);
    const { result } = renderHook(() =>
      useQpChain({ sending: false, onFire: vi.fn() }),
    );
    const qp = makeQP();
    act(() => result.current.queueQP(qp));
    expect(result.current.queuedQP).toBe(qp);
    act(() => result.current.cancelQueuedQP());
    expect(result.current.queuedQP).toBeNull();
  });

  it('fires the queued QP when sending transitions true → false', async () => {
    listMock.mockResolvedValue([]);
    const onFire = vi.fn();
    const { result, rerender } = renderHook(
      ({ sending }: { sending: boolean }) => useQpChain({ sending, onFire }),
      { initialProps: { sending: false } },
    );

    // Queue while idle.
    const qp = makeQP({ prompt_template: 'the chained prompt' });
    act(() => result.current.queueQP(qp));

    // Agent starts streaming.
    rerender({ sending: true });
    expect(onFire).not.toHaveBeenCalled();

    // Agent finishes — this is the edge the hook watches.
    rerender({ sending: false });
    expect(onFire).toHaveBeenCalledWith('the chained prompt');
    expect(onFire).toHaveBeenCalledTimes(1);
    // Queue clears after firing so a second edge doesn't refire.
    expect(result.current.queuedQP).toBeNull();
  });

  it('does NOT fire when sending stays false (no true→false edge)', () => {
    listMock.mockResolvedValue([]);
    const onFire = vi.fn();
    const { result } = renderHook(() =>
      useQpChain({ sending: false, onFire }),
    );
    // Queue without any prior sending=true — the initial `prevSendingRef` is
    // false, so the first render with sending=false must NOT count as an edge.
    act(() => result.current.queueQP(makeQP()));
    expect(onFire).not.toHaveBeenCalled();
  });

  it('does NOT fire when nothing is queued at the sending edge', () => {
    listMock.mockResolvedValue([]);
    const onFire = vi.fn();
    const { rerender } = renderHook(
      ({ sending }: { sending: boolean }) => useQpChain({ sending, onFire }),
      { initialProps: { sending: false } },
    );
    rerender({ sending: true });
    rerender({ sending: false });
    expect(onFire).not.toHaveBeenCalled();
  });

  it('uses the latest onFire even if the caller does not memoize it', () => {
    // Regression: the hook stores `onFire` in a ref so a freshly-created
    // lambda on each render is still the one that gets called.
    listMock.mockResolvedValue([]);
    const first = vi.fn();
    const second = vi.fn();
    const { result, rerender } = renderHook(
      ({ onFire, sending }: { onFire: (m: string) => void; sending: boolean }) =>
        useQpChain({ sending, onFire }),
      { initialProps: { onFire: first, sending: false } },
    );
    act(() => result.current.queueQP(makeQP({ prompt_template: 'go' })));
    // Caller swaps the handler between renders — classic non-memoized case.
    rerender({ onFire: second, sending: true });
    rerender({ onFire: second, sending: false });
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledWith('go');
  });
});
