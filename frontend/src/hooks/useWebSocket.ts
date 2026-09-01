import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { getApiBase, getAuthToken } from '../lib/api';
import type { WsMessage } from '../types/generated';

export type WsEventHandler = (msg: WsMessage) => void;
export type WsConnectionState = 'connecting' | 'connected' | 'reconnecting';
type Subscriber = { message: () => WsEventHandler; connect: () => void; state: (value: WsConnectionState) => void };
const subscribers = new Set<Subscriber>();
let socket: WebSocket | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
let heartbeatTimer: ReturnType<typeof setInterval> | undefined;
let heartbeatTimeout: ReturnType<typeof setTimeout> | undefined;
let pendingHeartbeat: number | null = null;
let reconnectDelay = 1_000;
let connectionState: WsConnectionState = 'connecting';

function publishState(value: WsConnectionState) {
  connectionState = value;
  subscribers.forEach(subscriber => subscriber.state(value));
}
function stopSocket() {
  clearTimeout(reconnectTimer); clearTimeout(heartbeatTimeout); clearInterval(heartbeatTimer);
  heartbeatTimer = undefined;
  const active = socket; socket = null;
  if (active) { active.onopen = null; active.onclose = null; active.onmessage = null; active.onerror = null; active.close(); }
}
function connect() {
  if (subscribers.size === 0 || socket) return;
  clearTimeout(reconnectTimer);
  const endpoint = getApiBase() ? new URL(getApiBase()) : window.location;
  const token = getAuthToken();
  const ws = new WebSocket(`${endpoint.protocol === 'https:' ? 'wss:' : 'ws:'}//${endpoint.host}/api/ws${token ? `?token=${encodeURIComponent(token)}` : ''}`);
  socket = ws;
  ws.onopen = () => {
    if (socket !== ws) return;
    reconnectDelay = 1_000; publishState('connected');
    ws.send(JSON.stringify({ type: 'presence', from_pseudo: 'local', from_invite_code: '', online: true }));
    subscribers.forEach(subscriber => subscriber.connect());
  };
  ws.onmessage = event => {
    if (socket !== ws) return;
    try {
      const message = JSON.parse(event.data) as WsMessage;
      if (message.type === 'pong' && message.timestamp === pendingHeartbeat) { pendingHeartbeat = null; clearTimeout(heartbeatTimeout); }
      subscribers.forEach(subscriber => subscriber.message()(message));
    } catch { /* Ignore malformed peer frames. */ }
  };
  ws.onclose = () => {
    if (socket !== ws) return;
    socket = null; publishState('reconnecting');
    if (subscribers.size > 0) { const delay = reconnectDelay; reconnectDelay = Math.min(delay * 2, 60_000); reconnectTimer = setTimeout(connect, delay); }
  };
  ws.onerror = () => ws.close();
  if (!heartbeatTimer) heartbeatTimer = setInterval(() => {
    if (socket?.readyState !== WebSocket.OPEN) return;
    pendingHeartbeat = Date.now(); socket.send(JSON.stringify({ type: 'ping', timestamp: pendingHeartbeat }));
    clearTimeout(heartbeatTimeout); heartbeatTimeout = setTimeout(() => socket?.close(), 10_000);
  }, 30_000);
}

/** One process-wide WebSocket fan-outs events to all mounted consumers. */
export function useWebSocket(onMessage: WsEventHandler, onConnect?: () => void, enabled = true): { connected: boolean; connectionState: WsConnectionState } {
  const messageRef = useRef(onMessage); const connectRef = useRef(onConnect);
  const [state, setState] = useState(connectionState);
  useLayoutEffect(() => { messageRef.current = onMessage; }, [onMessage]);
  useLayoutEffect(() => { connectRef.current = onConnect; }, [onConnect]);
  useEffect(() => {
    if (!enabled) return;
    const subscriber: Subscriber = { message: () => messageRef.current, connect: () => connectRef.current?.(), state: setState };
    subscribers.add(subscriber); setState(connectionState); connect();
    return () => { subscribers.delete(subscriber); if (subscribers.size === 0) stopSocket(); };
  }, [enabled]);
  return { connected: state === 'connected', connectionState: state };
}

export function activeWebSocketCountForTests(): number { return socket ? 1 : 0; }
