import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({ read: vi.fn(), change: vi.fn(), ws: vi.fn() }));
vi.mock('../../lib/api', () => ({ config: { getEmbedOrigins: mocks.read, changeEmbedOrigins: mocks.change } }));
vi.mock('../useWebSocket', () => ({ useWebSocket: mocks.ws }));

import {
  changeEmbedAllowedOrigins,
  embedOriginsFramableByThisDocument,
  invalidateEmbedAllowedOrigins,
  refreshEmbedAllowedOrigins,
  resetEmbedAllowedOriginsForTests,
  useEmbedAllowedOrigins,
} from '../useEmbedAllowedOrigins';
import { resetServedFrameOriginsForTests } from '../../lib/served-frame-policy';

/** A server answer the test releases when it wants. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

const listed = (result: { current: ReadonlySet<string> | null }) => result.current ? [...result.current] : null;

beforeEach(() => {
  resetEmbedAllowedOriginsForTests();
  mocks.read.mockReset();
  mocks.change.mockReset();
  mocks.ws.mockReset();
  mocks.ws.mockReturnValue({ connected: true, connectionState: 'connected' });
});
afterEach(() => resetEmbedAllowedOriginsForTests());

describe('allowed embed sites store', () => {
  it('re-reads on the backend event and on reconnect; the framable list is the served one, never a read', async () => {
    const meta = document.createElement('meta');
    meta.name = 'kronn-served-frame-src';
    meta.content = "'self' https://suno.com https://b.example";
    document.head.append(meta);
    resetServedFrameOriginsForTests();
    mocks.read.mockResolvedValueOnce(['https://suno.com']);
    const { result } = renderHook(() => useEmbedAllowedOrigins());
    await waitFor(() => expect(listed(result)).toEqual(['https://suno.com']));
    const [onMessage, onConnect] = mocks.ws.mock.calls.at(-1)!;

    mocks.read.mockResolvedValueOnce([]);
    await act(async () => { onMessage({ type: 'audit_finished' }); });
    expect(mocks.read).toHaveBeenCalledTimes(1);
    // Two mounted views get the same frame: a single read.
    await act(async () => {
      onMessage({ type: 'embed_origins_changed' });
      onMessage({ type: 'embed_origins_changed' });
    });
    await waitFor(() => expect(listed(result)).toEqual([]));
    expect(mocks.read).toHaveBeenCalledTimes(2);

    mocks.read.mockResolvedValueOnce(['https://suno.com', 'https://player.example']);
    act(() => { onConnect(); });
    await waitFor(() => expect(listed(result)).toEqual(['https://suno.com', 'https://player.example']));
    expect([...embedOriginsFramableByThisDocument()!]).toEqual(['https://suno.com', 'https://b.example']);
    meta.remove();
    resetServedFrameOriginsForTests();
  });

  it('a read that started before a revocation cannot bring the site back', async () => {
    mocks.read.mockResolvedValueOnce(['https://suno.com']);
    const { result } = renderHook(() => useEmbedAllowedOrigins());
    await waitFor(() => expect(listed(result)).toEqual(['https://suno.com']));

    const slowRead = deferred<string[]>();
    mocks.read.mockReturnValueOnce(slowRead.promise);
    const reading = refreshEmbedAllowedOrigins();
    mocks.change.mockResolvedValueOnce([]);
    await act(async () => { await changeEmbedAllowedOrigins({ add: [], remove: ['https://suno.com'] }); });
    expect(listed(result)).toEqual([]);

    await act(async () => { slowRead.resolve(['https://suno.com']); await reading; });
    expect(listed(result)).toEqual([]);
  });

  it('a read that started before an add cannot drop the confirmed site', async () => {
    mocks.read.mockResolvedValueOnce([]);
    const { result } = renderHook(() => useEmbedAllowedOrigins());
    await waitFor(() => expect(listed(result)).toEqual([]));

    const slowRead = deferred<string[]>();
    mocks.read.mockReturnValueOnce(slowRead.promise);
    const reading = refreshEmbedAllowedOrigins();
    mocks.change.mockResolvedValueOnce(['https://player.example.com']);
    await act(async () => { await changeEmbedAllowedOrigins({ add: ['https://player.example.com'], remove: [] }); });

    await act(async () => { slowRead.resolve([]); await reading; });
    expect(listed(result)).toEqual(['https://player.example.com']);
  });

  it('a read that started while a change is in flight is dropped too', async () => {
    mocks.read.mockResolvedValueOnce(['https://suno.com']);
    const { result } = renderHook(() => useEmbedAllowedOrigins());
    await waitFor(() => expect(listed(result)).toEqual(['https://suno.com']));

    const slowChange = deferred<string[]>();
    mocks.change.mockReturnValueOnce(slowChange.promise);
    const changing = changeEmbedAllowedOrigins({ add: [], remove: ['https://suno.com'] });
    mocks.read.mockResolvedValueOnce(['https://suno.com']);
    await act(async () => { await refreshEmbedAllowedOrigins(); });
    expect(listed(result)).toEqual(['https://suno.com']);
    await act(async () => { slowChange.resolve([]); await changing; });
    expect(listed(result)).toEqual([]);
  });

  it('after a change made elsewhere (an import), drops the read in flight and reads again', async () => {
    const slowRead = deferred<string[]>();
    mocks.read.mockReturnValueOnce(slowRead.promise);
    const { result } = renderHook(() => useEmbedAllowedOrigins());
    await waitFor(() => expect(mocks.read).toHaveBeenCalledTimes(1));

    mocks.read.mockResolvedValueOnce(['https://player.example.com']);
    await act(async () => { await invalidateEmbedAllowedOrigins(); });
    expect(mocks.read).toHaveBeenCalledTimes(2);
    expect(listed(result)).toEqual(['https://player.example.com']);

    await act(async () => { slowRead.resolve([]); await slowRead.promise; });
    expect(listed(result)).toEqual(['https://player.example.com']);
  });

  it('applies changes in the order they were made', async () => {
    mocks.read.mockResolvedValueOnce([]);
    const { result } = renderHook(() => useEmbedAllowedOrigins());
    await waitFor(() => expect(listed(result)).toEqual([]));
    const first = deferred<string[]>();
    mocks.change.mockReturnValueOnce(first.promise).mockResolvedValueOnce(['https://a.example', 'https://b.example']);
    const a = changeEmbedAllowedOrigins({ add: ['https://a.example'], remove: [] });
    const b = changeEmbedAllowedOrigins({ add: ['https://b.example'], remove: [] });
    // The second change is not sent before the first has landed.
    await waitFor(() => expect(mocks.change).toHaveBeenCalledTimes(1));
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 20)); });
    expect(mocks.change).toHaveBeenCalledTimes(1);
    await act(async () => { first.resolve(['https://a.example']); await a; await b; });
    expect(mocks.change).toHaveBeenCalledTimes(2);
    expect(listed(result)).toEqual(['https://a.example', 'https://b.example']);
  });

  it('reads the list again after a failed change', async () => {
    mocks.read.mockResolvedValueOnce(['https://suno.com']);
    const { result } = renderHook(() => useEmbedAllowedOrigins());
    await waitFor(() => expect(listed(result)).toEqual(['https://suno.com']));
    mocks.change.mockRejectedValueOnce(new Error('disk full'));
    mocks.read.mockResolvedValueOnce(['https://suno.com', 'https://other-tab.example']);
    await act(async () => {
      await expect(changeEmbedAllowedOrigins({ add: ['https://x.example'], remove: [] })).rejects.toThrow('disk full');
    });
    await waitFor(() => expect(listed(result)).toEqual(['https://suno.com', 'https://other-tab.example']));
  });
});
