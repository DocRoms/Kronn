import { useEffect, useLayoutEffect, useRef, useState, useCallback } from 'react';
import { getApiBase, getAuthToken } from '../lib/api';
import type { WsMessage } from '../types/generated';

export type WsEventHandler = (msg: WsMessage) => void;
export type WsConnectionState = 'connecting' | 'connected' | 'reconnecting';

const INITIAL_RECONNECT_DELAY_MS = 1_000;
const MAX_RECONNECT_DELAY_MS = 60_000;
const HEARTBEAT_INTERVAL_MS = 30_000;
const HEARTBEAT_TIMEOUT_MS = 10_000;

/**
 * React hook that maintains a WebSocket connection to the local backend.
 *
 * - Auto-reconnects with exponential backoff (1s → 60s).
 * - Sends a heartbeat ping every 30s to keep the connection alive.
 * - Calls `onMessage` for every parsed WsMessage received.
 * - Calls `onConnect` on every (re)connect, so the caller can RE-SYNC state it
 *   may have missed while the socket was down (a backend restart or dropped
 *   connection means federated messages / presence events fired with no
 *   listener — without a catch-up the UI silently stays stale until the next
 *   live event, which is why a peer's messages "don't appear" after a rebuild).
 */
export function useWebSocket(
  onMessage: WsEventHandler,
  onConnect?: () => void,
): { connected: boolean; connectionState: WsConnectionState } {
  const [connected, setConnected] = useState(false);
  const [connectionState, setConnectionState] = useState<WsConnectionState>('connecting');
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimeout = useRef<ReturnType<typeof setTimeout>>(undefined);
  const heartbeatTimeout = useRef<ReturnType<typeof setTimeout>>(undefined);
  const pendingHeartbeat = useRef<number | null>(null);
  const backoff = useRef(INITIAL_RECONNECT_DELAY_MS);
  const mounted = useRef(false);
  const onMessageRef = useRef(onMessage);
  const onConnectRef = useRef(onConnect);

  useLayoutEffect(() => {
    onMessageRef.current = onMessage;
  }, [onMessage]);

  useLayoutEffect(() => {
    onConnectRef.current = onConnect;
  }, [onConnect]);

  const connect = useCallback(function openSocket() {
    if (!mounted.current) return;
    clearTimeout(reconnectTimeout.current);

    // Tauri can keep serving its bundled frontend while reusing a compatible
    // CLI backend. In that case API base and page origin intentionally differ.
    const apiBase = getApiBase();
    const endpoint = apiBase ? new URL(apiBase) : window.location;
    const proto = endpoint.protocol === 'https:' ? 'wss:' : 'ws:';
    const host = endpoint.host;
    const token = getAuthToken();
    const url = `${proto}//${host}/api/ws${token ? `?token=${encodeURIComponent(token)}` : ''}`;

    const ws = new WebSocket(url);
    wsRef.current = ws;

    ws.onopen = () => {
      if (!mounted.current || wsRef.current !== ws) {
        ws.close();
        return;
      }
      setConnected(true);
      setConnectionState('connected');
      backoff.current = INITIAL_RECONNECT_DELAY_MS;
      // Send Presence as the very first frame so the backend's recv-task
      // verifies the connection (cf. ws.rs handshake). Local connections
      // pass an empty invite_code — accepted on the loopback path. Without
      // this, the backend stays `verified=false` for the lifetime of the
      // local connection (mitigated for heartbeats by Phase 2 of 2026-05-07,
      // but still required for any future local→server broadcast).
      // TD-20260507-ws-no-presence-on-open.
      try {
        ws.send(JSON.stringify({
          type: 'presence',
          from_pseudo: 'local',
          from_invite_code: '',
          online: true,
        }));
      } catch {
        // ignore — onclose will retry
      }
      // Re-sync after every (re)connect so the UI catches up on anything that
      // happened while the socket was down (missed federated messages, presence).
      try {
        onConnectRef.current?.();
      } catch {
        // a caller error must never tear the socket back down
      }
    };

    ws.onmessage = (event) => {
      if (!mounted.current || wsRef.current !== ws) return;
      try {
        const msg = JSON.parse(event.data) as WsMessage;
        if (msg.type === 'pong' && msg.timestamp === pendingHeartbeat.current) {
          pendingHeartbeat.current = null;
          clearTimeout(heartbeatTimeout.current);
        }
        onMessageRef.current(msg);
      } catch {
        // Ignore non-JSON messages
      }
    };

    ws.onclose = () => {
      if (wsRef.current !== ws) return;
      setConnected(false);
      wsRef.current = null;
      pendingHeartbeat.current = null;
      clearTimeout(heartbeatTimeout.current);
      if (!mounted.current) return;
      setConnectionState('reconnecting');
      // Reconnect with exponential backoff
      const delay = backoff.current;
      backoff.current = Math.min(delay * 2, MAX_RECONNECT_DELAY_MS);
      reconnectTimeout.current = setTimeout(() => {
        openSocket();
      }, delay);
    };

    ws.onerror = () => {
      // onclose will fire after onerror, triggering reconnect
      if (wsRef.current === ws) ws.close();
    };
  }, []);

  useEffect(() => {
    mounted.current = true;
    connect();

    // Heartbeat: send ping every 30s
    const pingInterval = setInterval(() => {
      const ws = wsRef.current;
      if (ws?.readyState === WebSocket.OPEN) {
        const timestamp = Date.now();
        pendingHeartbeat.current = timestamp;
        try {
          ws.send(JSON.stringify({ type: 'ping', timestamp }));
        } catch {
          ws.close();
          return;
        }
        clearTimeout(heartbeatTimeout.current);
        heartbeatTimeout.current = setTimeout(() => {
          if (
            mounted.current
            && wsRef.current === ws
            && pendingHeartbeat.current === timestamp
          ) {
            ws.close();
          }
        }, HEARTBEAT_TIMEOUT_MS);
      }
    }, HEARTBEAT_INTERVAL_MS);

    return () => {
      mounted.current = false;
      clearInterval(pingInterval);
      clearTimeout(reconnectTimeout.current);
      clearTimeout(heartbeatTimeout.current);
      pendingHeartbeat.current = null;
      const ws = wsRef.current;
      wsRef.current = null;
      if (ws) {
        ws.onopen = null;
        ws.onclose = null;
        ws.onmessage = null;
        ws.onerror = null;
        ws.close();
      }
    };
  }, [connect]);

  return { connected, connectionState };
}
