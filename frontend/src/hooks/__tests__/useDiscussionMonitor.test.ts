import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useDiscussionMonitor } from '../useDiscussionMonitor';
import { discussions } from '../../lib/api';
import type { DiscussionMonitorItem } from '../../types/generated';

vi.mock('../../lib/api', () => ({
  discussions: { monitor: vi.fn() }, getApiBase: () => '', getAuthToken: () => null,
}));
class Socket {
  static instances: Socket[] = [];
  static OPEN = 1;
  readyState = 0;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  sent: string[] = [];
  constructor() { Socket.instances.push(this); }
  send(data: string) { this.sent.push(data); }
  close() { this.readyState = 3; this.onclose?.(); }
  open() { this.readyState = 1; this.onopen?.(); }
  event(data: object) { this.onmessage?.({ data: JSON.stringify(data) }); }
}
const items: DiscussionMonitorItem[] = [{ id: 'a', preview: null, error: 'not_found' }, { id: 'b', preview: null, error: 'unavailable' }];
const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));
const hide = (hidden: boolean) => {
  Object.defineProperty(document, 'hidden', { configurable: true, value: hidden });
  act(() => document.dispatchEvent(new Event('visibilitychange')));
};
beforeEach(() => {
  vi.useFakeTimers(); vi.clearAllMocks();
  Socket.instances = []; vi.stubGlobal('WebSocket', Socket);
  Object.defineProperty(document, 'hidden', { configurable: true, value: false });
  vi.mocked(discussions.monitor).mockResolvedValue(items);
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); });

describe('discussion monitor reader', () => {
  it('uses one socket and one scoped batch, throttles selected events, ignores unrelated ones', async () => {
    const storage = vi.spyOn(Storage.prototype, 'setItem');
    const { result, rerender, unmount } = renderHook(() => useDiscussionMonitor(['a', 'b']));
    await advance(0);
    expect(discussions.monitor).toHaveBeenCalledWith(['a', 'b'], expect.any(AbortSignal));
    expect(Socket.instances).toHaveLength(1);
    expect(result.current.items).toEqual(items);
    rerender();
    const socket = Socket.instances[0];
    act(() => socket.event({ type: 'discussion_updated', discussion_id: 'elsewhere' }));
    await advance(1000);
    expect(discussions.monitor).toHaveBeenCalledTimes(1);
    for (let i = 0; i < 5; i++) {
      act(() => socket.event({ type: 'discussion_updated', discussion_id: 'a' }));
      await advance(50);
    }
    expect(discussions.monitor).toHaveBeenCalledTimes(2);
    expect(Socket.instances).toHaveLength(1);
    expect(storage).not.toHaveBeenCalled();
    unmount();
    expect(socket.readyState).toBe(3);
    expect(vi.getTimerCount()).toBe(0);
  });
  it('never overlaps reads, aborts and clears all timers on unmount', async () => {
    let resolve!: (value: DiscussionMonitorItem[]) => void;
    vi.mocked(discussions.monitor).mockReturnValue(new Promise(done => { resolve = done; }));
    const { result, unmount } = renderHook(() => useDiscussionMonitor(['a', 'b']));
    await advance(0);
    const signal = vi.mocked(discussions.monitor).mock.calls[0][1]!;
    act(() => result.current.refresh());
    await advance(1000);
    expect(discussions.monitor).toHaveBeenCalledTimes(1);
    unmount();
    expect(signal.aborted).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
    await act(async () => resolve(items));
    await advance(20000);
    expect(discussions.monitor).toHaveBeenCalledTimes(1);
  });
  it('retains previews on failure, pauses hidden polling and catches up on visibility/reconnect', async () => {
    const { result } = renderHook(() => useDiscussionMonitor(['a', 'b']));
    await advance(0);
    vi.mocked(discussions.monitor).mockRejectedValueOnce(new Error('offline'));
    await advance(5000);
    expect(result.current.error).toBe(true);
    expect(result.current.items).toEqual(items);
    hide(true); await advance(20000);
    expect(discussions.monitor).toHaveBeenCalledTimes(2);
    hide(false); await advance(0);
    expect(discussions.monitor).toHaveBeenCalledTimes(3);
    expect(result.current.error).toBe(false);
    act(() => Socket.instances[0].close());
    await advance(1000);
    expect(Socket.instances).toHaveLength(2);
    act(() => Socket.instances[1].open());
    await advance(250);
    expect(discussions.monitor).toHaveBeenCalledTimes(4);
    expect(result.current.connectionState).toBe('connected');
    expect(Socket.instances[1].sent.map(value => JSON.parse(value).type)).toEqual(['presence']);
  });
  it('bounds a hung request and recovers on the next refresh', async () => {
    vi.mocked(discussions.monitor).mockImplementationOnce((_ids, signal) => new Promise((_resolve, reject) => {
      signal?.addEventListener('abort', () => reject(new Error('timeout')));
    }));
    const { result } = renderHook(() => useDiscussionMonitor(['a', 'b']));
    await advance(15000);
    expect(result.current.error).toBe(true);
    await advance(5000);
    expect(result.current.items).toEqual(items);
    expect(result.current.error).toBe(false);
  });
});
