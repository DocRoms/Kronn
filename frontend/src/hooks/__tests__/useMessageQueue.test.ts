import { describe, it, expect, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useMessageQueue } from '../useMessageQueue';

describe('useMessageQueue', () => {
  it('enqueue appends messages in order; queue exposes them', () => {
    const { result } = renderHook(() =>
      useMessageQueue({ discId: 'd1', sending: true, onFire: vi.fn() }),
    );
    act(() => result.current.enqueue('first'));
    act(() => result.current.enqueue('second', 'Codex'));
    expect(result.current.queue.map(m => m.content)).toEqual(['first', 'second']);
    expect(result.current.queue[1].targetAgent).toBe('Codex');
    // Each message gets a distinct id.
    expect(result.current.queue[0].id).not.toBe(result.current.queue[1].id);
  });

  it('enqueue ignores empty / whitespace-only content', () => {
    const { result } = renderHook(() =>
      useMessageQueue({ discId: 'd1', sending: true, onFire: vi.fn() }),
    );
    act(() => result.current.enqueue('   '));
    act(() => result.current.enqueue(''));
    expect(result.current.queue).toEqual([]);
  });

  it('removeQueued drops one by id; clearQueue empties it', () => {
    const { result } = renderHook(() =>
      useMessageQueue({ discId: 'd1', sending: true, onFire: vi.fn() }),
    );
    act(() => result.current.enqueue('a'));
    act(() => result.current.enqueue('b'));
    const firstId = result.current.queue[0].id;
    act(() => result.current.removeQueued(firstId));
    expect(result.current.queue.map(m => m.content)).toEqual(['b']);
    act(() => result.current.clearQueue());
    expect(result.current.queue).toEqual([]);
  });

  it('merges ALL pending parts into ONE message on the completion edge', () => {
    const onFire = vi.fn();
    const { result, rerender } = renderHook(
      ({ sending }: { sending: boolean }) => useMessageQueue({ discId: 'd1', sending, onFire }),
      { initialProps: { sending: false } },
    );
    act(() => result.current.enqueue('one'));
    act(() => result.current.enqueue('two'));
    act(() => result.current.enqueue('three'));

    rerender({ sending: true });
    expect(onFire).not.toHaveBeenCalled();

    rerender({ sending: false }); // completion edge → fire ONE merged message
    expect(onFire).toHaveBeenCalledTimes(1);
    expect(onFire).toHaveBeenCalledWith('one\n\ntwo\n\nthree', undefined);
    // The whole pending set is consumed.
    expect(result.current.queue).toEqual([]);
  });

  it('uses the first @mention among the merged parts as the target agent', () => {
    const onFire = vi.fn();
    const { result, rerender } = renderHook(
      ({ sending }: { sending: boolean }) => useMessageQueue({ discId: 'd1', sending, onFire }),
      { initialProps: { sending: false } },
    );
    act(() => result.current.enqueue('plain part'));
    act(() => result.current.enqueue('codex part', 'Codex'));
    rerender({ sending: true });
    rerender({ sending: false });
    expect(onFire).toHaveBeenCalledWith('plain part\n\ncodex part', 'Codex');
  });

  it('a part added during the merged turn fires on the NEXT edge', () => {
    const onFire = vi.fn();
    const { result, rerender } = renderHook(
      ({ sending }: { sending: boolean }) => useMessageQueue({ discId: 'd1', sending, onFire }),
      { initialProps: { sending: false } },
    );
    act(() => result.current.enqueue('m1'));
    rerender({ sending: true });
    rerender({ sending: false }); // fires 'm1'
    act(() => result.current.enqueue('m2')); // queued during the merged turn
    rerender({ sending: true });
    rerender({ sending: false }); // fires 'm2'

    expect(onFire).toHaveBeenCalledTimes(2);
    expect(onFire).toHaveBeenNthCalledWith(1, 'm1', undefined);
    expect(onFire).toHaveBeenNthCalledWith(2, 'm2', undefined);
    expect(result.current.queue).toEqual([]);
  });

  it('does NOT fire when nothing is queued at the edge', () => {
    const onFire = vi.fn();
    const { rerender } = renderHook(
      ({ sending }: { sending: boolean }) => useMessageQueue({ discId: 'd1', sending, onFire }),
      { initialProps: { sending: false } },
    );
    rerender({ sending: true });
    rerender({ sending: false });
    expect(onFire).not.toHaveBeenCalled();
  });

  it('resets the queue when the active discussion changes', () => {
    const { result, rerender } = renderHook(
      ({ discId }: { discId: string }) => useMessageQueue({ discId, sending: true, onFire: vi.fn() }),
      { initialProps: { discId: 'd1' } },
    );
    act(() => result.current.enqueue('belongs to d1'));
    expect(result.current.queue).toHaveLength(1);
    rerender({ discId: 'd2' });
    expect(result.current.queue).toEqual([]);
  });

  it('uses the latest onFire even when the caller does not memoize it', () => {
    const first = vi.fn();
    const second = vi.fn();
    const { result, rerender } = renderHook(
      ({ onFire, sending }: { onFire: (c: string, a?: string) => void; sending: boolean }) =>
        useMessageQueue({ discId: 'd1', sending, onFire: onFire as never }),
      { initialProps: { onFire: first, sending: false } },
    );
    act(() => result.current.enqueue('go'));
    rerender({ onFire: second, sending: true });
    rerender({ onFire: second, sending: false });
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledWith('go', undefined);
  });
});
