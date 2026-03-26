import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useWebSocket } from '../useWebSocket';

// Mock getAuthToken
import { getAuthToken } from '../../lib/api';
vi.mock('../../lib/api', () => ({
  getAuthToken: vi.fn(() => null),
}));

// ─── Mock WebSocket ──────────────────────────────────────────────────────

class MockWebSocket {
  static instances: MockWebSocket[] = [];
  url: string;
  readyState = 0; // CONNECTING
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  sent: string[] = [];

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
  }

  send(data: string) {
    this.sent.push(data);
  }

  close() {
    this.readyState = 3; // CLOSED
    this.onclose?.();
  }

  // Test helpers
  simulateOpen() {
    this.readyState = 1; // OPEN
    this.onopen?.();
  }

  simulateMessage(data: string) {
    this.onmessage?.({ data });
  }

  static readonly OPEN = 1;
}

describe('useWebSocket', () => {
  let originalWebSocket: typeof WebSocket;

  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllTimers();
    MockWebSocket.instances = [];
    originalWebSocket = globalThis.WebSocket;
    (globalThis as any).WebSocket = MockWebSocket;
  });

  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    globalThis.WebSocket = originalWebSocket;
  });

  it('connects on mount and reports connected status', () => {
    const handler = vi.fn();
    const { result } = renderHook(() => useWebSocket(handler));

    expect(result.current.connected).toBe(false);
    expect(MockWebSocket.instances).toHaveLength(1);

    // Simulate open
    act(() => {
      MockWebSocket.instances[0].simulateOpen();
    });

    expect(result.current.connected).toBe(true);
  });

  it('dispatches parsed WsMessage to handler', () => {
    const handler = vi.fn();
    renderHook(() => useWebSocket(handler));

    const ws = MockWebSocket.instances[0];
    act(() => ws.simulateOpen());

    const presenceMsg = JSON.stringify({
      type: 'presence',
      from_pseudo: 'PeerAlpha',
      from_invite_code: 'kronn:PeerAlpha@100.64.1.5:3456',
      online: true,
    });

    act(() => ws.simulateMessage(presenceMsg));

    expect(handler).toHaveBeenCalledWith({
      type: 'presence',
      from_pseudo: 'PeerAlpha',
      from_invite_code: 'kronn:PeerAlpha@100.64.1.5:3456',
      online: true,
    });
  });

  it('ignores non-JSON messages', () => {
    const handler = vi.fn();
    renderHook(() => useWebSocket(handler));

    const ws = MockWebSocket.instances[0];
    act(() => ws.simulateOpen());
    act(() => ws.simulateMessage('not json'));

    expect(handler).not.toHaveBeenCalled();
  });

  it('builds correct WS URL without auth token', () => {
    const handler = vi.fn();
    renderHook(() => useWebSocket(handler));

    const ws = MockWebSocket.instances[0];
    // Default JSDOM location is http://localhost:3000
    expect(ws.url).toContain('/api/ws');
    expect(ws.url).not.toContain('token=');
  });

  it('reconnects with backoff on close', () => {
    const handler = vi.fn();
    const { result } = renderHook(() => useWebSocket(handler));

    const ws = MockWebSocket.instances[0];
    act(() => ws.simulateOpen());
    expect(result.current.connected).toBe(true);

    // Simulate disconnect
    act(() => ws.close());
    expect(result.current.connected).toBe(false);

    // After 1s backoff, should try to reconnect
    act(() => { vi.advanceTimersByTime(1100); });
    expect(MockWebSocket.instances).toHaveLength(2);
  });

  it('cleans up on unmount', () => {
    const handler = vi.fn();
    const { unmount } = renderHook(() => useWebSocket(handler));

    const ws = MockWebSocket.instances[0];
    act(() => ws.simulateOpen());

    unmount();

    // WebSocket should be closed
    expect(ws.readyState).toBe(3); // CLOSED
  });

  it('includes auth token in URL when available', () => {
    vi.mocked(getAuthToken).mockReturnValueOnce('my-secret-token');

    const handler = vi.fn();
    renderHook(() => useWebSocket(handler));

    const ws = MockWebSocket.instances[0];
    expect(ws.url).toContain('token=my-secret-token');
  });

  it('sends heartbeat ping every 30s when connected', () => {
    const handler = vi.fn();
    renderHook(() => useWebSocket(handler));

    const ws = MockWebSocket.instances[0];
    act(() => ws.simulateOpen());

    expect(ws.sent).toHaveLength(0);

    // Advance 30s — should send ping
    act(() => { vi.advanceTimersByTime(30000); });
    expect(ws.sent).toHaveLength(1);

    const parsed = JSON.parse(ws.sent[0]);
    expect(parsed.type).toBe('ping');
    expect(typeof parsed.timestamp).toBe('number');
  });

  it('backoff doubles after each disconnect', () => {
    const handler = vi.fn();
    const { unmount } = renderHook(() => useWebSocket(handler));

    // WS[0] created on mount
    const ws0 = MockWebSocket.instances[0];
    act(() => ws0.simulateOpen());

    // First disconnect — backoff starts at 1s
    act(() => ws0.close());
    const countBefore = MockWebSocket.instances.length;

    // After 500ms → no reconnect yet
    act(() => { vi.advanceTimersByTime(500); });
    expect(MockWebSocket.instances.length).toBe(countBefore);

    // After 600ms more (total 1100ms) → reconnect fires
    act(() => { vi.advanceTimersByTime(600); });
    expect(MockWebSocket.instances.length).toBe(countBefore + 1);

    unmount();
  });

  it('resets backoff on successful connection', () => {
    const handler = vi.fn();
    renderHook(() => useWebSocket(handler));

    // Connect, disconnect, wait for reconnect
    act(() => MockWebSocket.instances[0].simulateOpen());
    act(() => MockWebSocket.instances[0].close());
    act(() => { vi.advanceTimersByTime(1100); });
    expect(MockWebSocket.instances).toHaveLength(2);

    // Successful reconnect — backoff should reset
    act(() => MockWebSocket.instances[1].simulateOpen());
    act(() => MockWebSocket.instances[1].close());

    // Should reconnect after 1s again (not 2s), because backoff was reset
    act(() => { vi.advanceTimersByTime(1100); });
    expect(MockWebSocket.instances).toHaveLength(3);
  });

  it('dispatches multiple message types correctly', () => {
    const handler = vi.fn();
    renderHook(() => useWebSocket(handler));

    const ws = MockWebSocket.instances[0];
    act(() => ws.simulateOpen());

    // Presence message
    act(() => ws.simulateMessage(JSON.stringify({
      type: 'presence', from_pseudo: 'PeerAlpha',
      from_invite_code: 'kronn:PeerAlpha@10.0.0.1:3456', online: true,
    })));

    // Pong message
    act(() => ws.simulateMessage(JSON.stringify({
      type: 'pong', timestamp: 1711000000,
    })));

    expect(handler).toHaveBeenCalledTimes(2);
    expect(handler.mock.calls[0][0].type).toBe('presence');
    expect(handler.mock.calls[1][0].type).toBe('pong');
  });
});
