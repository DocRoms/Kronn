import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { useMessageQueue, type QueuedMessage, type QueuedMessageControl } from '../useMessageQueue';
import type { AgentType, MessageTarget } from '../../types/generated';

const punctual = (agent_type: AgentType): MessageTarget => ({
  kind: 'agent',
  agent_type,
  cli_session_id: null,
});

const key = (discId: string) => `kronn:message-outbox:${discId}`;

describe('useMessageQueue — durable outbox', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.useRealTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('persists a stable UUID before asking the backend to accept it', async () => {
    let persistedBeforeCall: QueuedMessage[] = [];
    const onPersist = vi.fn(async (discId: string) => {
      persistedBeforeCall = JSON.parse(localStorage.getItem(key(discId)) ?? '[]');
    });
    const { result } = renderHook(() => useMessageQueue({ discId: 'd1', onPersist }));

    act(() => {
      result.current.enqueue('first', [punctual('Codex')]);
    });

    const durableId = (JSON.parse(localStorage.getItem(key('d1')) ?? '[]') as QueuedMessage[])[0].id;
    expect(durableId).toMatch(/^[0-9a-f]{8}-[0-9a-f-]{27}$/i);
    await waitFor(() => expect(onPersist).toHaveBeenCalledTimes(1));
    expect(persistedBeforeCall).toEqual([
      expect.objectContaining({
        id: durableId,
        content: 'first',
        targets: [punctual('Codex')],
        status: 'sending',
      }),
    ]);
    await waitFor(() => expect(result.current.queue).toEqual([]));
    expect(localStorage.getItem(key('d1'))).toBeNull();
  });

  it('keeps a pre-acceptance failure visible and retries the same identity', async () => {
    vi.useFakeTimers();
    const calls: string[] = [];
    const onPersist = vi.fn(async (_discId: string, message: QueuedMessage) => {
      calls.push(message.id);
      if (calls.length === 1) throw new Error('backend offline');
    });
    const { result } = renderHook(() => useMessageQueue({ discId: 'd1', onPersist }));

    act(() => { result.current.enqueue('survive restart'); });
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    expect(result.current.queue[0]).toEqual(expect.objectContaining({
      status: 'failed',
      error: 'backend offline',
    }));
    const stableId = result.current.queue[0].id;

    await act(async () => {
      vi.advanceTimersByTime(2_000);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(calls).toEqual([stableId, stableId]);
    expect(result.current.queue).toEqual([]);
    expect(localStorage.getItem(key('d1'))).toBeNull();
  });

  it('restores an interrupted sending entry and reconciles it once after remount', async () => {
    const stableId = '77777777-7777-4777-8777-777777777777';
    localStorage.setItem(key('d1'), JSON.stringify([{
      id: stableId,
      content: 'receipt was lost',
      status: 'sending',
      attempts: 1,
      createdAt: '2026-08-27T12:00:00.000Z',
    }]));
    const onPersist = vi.fn().mockResolvedValue(undefined);

    const { result } = renderHook(() => useMessageQueue({ discId: 'd1', onPersist }));

    await waitFor(() => expect(onPersist).toHaveBeenCalledWith(
      'd1',
      expect.objectContaining({ id: stableId, status: 'sending', attempts: 2 }),
      expect.objectContaining({ signal: expect.any(AbortSignal), beginPersist: expect.any(Function) }),
    ));
    await waitFor(() => expect(result.current.queue).toEqual([]));
    expect(onPersist).toHaveBeenCalledTimes(1);
  });

  it('keeps independent persisted queues when the active discussion changes', () => {
    const never = () => new Promise<void>(() => undefined);
    const { result, rerender } = renderHook(
      ({ discId }: { discId: string }) => useMessageQueue({ discId, onPersist: never }),
      { initialProps: { discId: 'd1' } },
    );
    act(() => { result.current.enqueue('belongs to d1'); });
    const d1Id = result.current.queue[0].id;

    rerender({ discId: 'd2' });
    expect(result.current.queue).toEqual([]);
    act(() => { result.current.enqueue('belongs to d2'); });
    expect(result.current.queue[0].id).not.toBe(d1Id);

    rerender({ discId: 'd1' });
    expect(result.current.queue).toEqual([
      expect.objectContaining({ id: d1Id, content: 'belongs to d1' }),
    ]);
  });

  it('retries failed entries manually and protects every in-flight write', async () => {
    let resolvePersist!: () => void;
    let rejectPersist!: (reason: Error) => void;
    const onPersist = vi.fn(() => new Promise<void>((resolve, reject) => {
      resolvePersist = resolve;
      rejectPersist = reject;
    }));
    const { result } = renderHook(() => useMessageQueue({ discId: 'd1', onPersist }));
    act(() => { result.current.enqueue('one'); });
    await waitFor(() => expect(result.current.queue[0]?.status).toBe('sending'));
    const id = result.current.queue[0].id;

    act(() => result.current.removeQueued(id));
    expect(result.current.queue).toHaveLength(1);

    await act(async () => { rejectPersist(new Error('offline')); });
    await waitFor(() => expect(result.current.queue[0]?.status).toBe('failed'));
    act(() => result.current.retryQueued(id));
    await waitFor(() => expect(onPersist).toHaveBeenCalledTimes(2));
    expect(result.current.queue[0]?.status).toBe('sending');
    act(() => result.current.removeQueued(id));
    expect(result.current.queue).toHaveLength(1);
    await act(async () => { resolvePersist(); });
    await waitFor(() => expect(result.current.queue).toEqual([]));
  });

  it('releases cancelled preparation without waiting for an unresponsive proof service', async () => {
    const controls: QueuedMessageControl[] = [];
    const onPersist = vi.fn(async (_disc: string, _message: QueuedMessage, control: QueuedMessageControl) => {
      controls.push(control);
      if (controls.length === 1) await new Promise<void>(() => undefined);
      else expect(control.beginPersist()).toBe(true);
    });
    const { result } = renderHook(() => useMessageQueue({ discId: 'd1', onPersist, prepareBeforePersist: true }));
    act(() => { result.current.enqueue('first'); });
    await waitFor(() => expect(onPersist).toHaveBeenCalledTimes(1));
    const firstId = result.current.queue[0].id;
    expect(result.current.queue[0].status).toBe('preparing');
    act(() => { result.current.removeQueued(firstId); result.current.enqueue('second'); });
    await waitFor(() => expect(onPersist).toHaveBeenCalledTimes(2));
    expect(controls[0].signal.aborted).toBe(true);
    expect(controls[0].beginPersist()).toBe(false);
    await waitFor(() => expect(result.current.queue).toEqual([]));
  });

  it('protects an already claimed write while clearing other unsent entries', async () => {
    let finish!: () => void;
    let control!: QueuedMessageControl;
    const onPersist = vi.fn(async (_disc: string, _message: QueuedMessage, current: QueuedMessageControl) => {
      control = current;
      expect(control.beginPersist()).toBe(true);
      await new Promise<void>(resolve => { finish = resolve; });
    });
    const { result } = renderHook(() => useMessageQueue({ discId: 'd1', onPersist, prepareBeforePersist: true }));
    act(() => { result.current.enqueue('claimed'); });
    await waitFor(() => expect(result.current.queue[0]?.status).toBe('sending'));
    act(() => { result.current.enqueue('unsent'); result.current.clearQueue(); result.current.cancelPreparing(); });
    expect(result.current.queue.map(message => message.content)).toEqual(['claimed']);
    expect(control.signal.aborted).toBe(false);
    await act(async () => { finish(); });
    await waitFor(() => expect(result.current.queue).toEqual([]));
  });
});
