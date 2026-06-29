import { useEffect, useRef, useState, useCallback } from 'react';
import { getAuthToken } from '../lib/api';
import type { WsMessage } from '../types/generated';

export type WsEventHandler = (msg: WsMessage) => void;

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
): { connected: boolean } {
  const [connected, setConnected] = useState(false);
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimeout = useRef<ReturnType<typeof setTimeout>>(undefined);
  const backoff = useRef(1000);
  const onMessageRef = useRef(onMessage);
  onMessageRef.current = onMessage;
  const onConnectRef = useRef(onConnect);
  onConnectRef.current = onConnect;

  const connect = useCallback(() => {
    // Build WS URL from current page location
    const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const host = window.location.host;
    const token = getAuthToken();
    const url = `${proto}//${host}/api/ws${token ? `?token=${encodeURIComponent(token)}` : ''}`;

    const ws = new WebSocket(url);
    wsRef.current = ws;

    ws.onopen = () => {
      setConnected(true);
      backoff.current = 1000;
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
      try {
        const msg = JSON.parse(event.data) as WsMessage;
        onMessageRef.current(msg);
      } catch {
        // Ignore non-JSON messages
      }
    };

    ws.onclose = () => {
      setConnected(false);
      wsRef.current = null;
      // Reconnect with exponential backoff
      reconnectTimeout.current = setTimeout(() => {
        backoff.current = Math.min(backoff.current * 2, 60000);
        connect();
      }, backoff.current);
    };

    ws.onerror = () => {
      // onclose will fire after onerror, triggering reconnect
      ws.close();
    };
  }, []);

  useEffect(() => {
    connect();

    // Heartbeat: send ping every 30s
    const pingInterval = setInterval(() => {
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        wsRef.current.send(JSON.stringify({ type: 'ping', timestamp: Date.now() }));
      }
    }, 30000);

    return () => {
      clearInterval(pingInterval);
      clearTimeout(reconnectTimeout.current);
      wsRef.current?.close();
      wsRef.current = null;
    };
  }, [connect]);

  return { connected };
}
