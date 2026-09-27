import { useCallback, useEffect, useRef, useState } from 'react';
import { discussions } from '../lib/api';
import type { DiscussionMonitorItem, WsMessage } from '../types/generated';
import { useWebSocket } from './useWebSocket';

/** One batch reader for the whole mosaic. WS events shorten the wait, while a
 * scoped refresh also catches persisted checkpoints and local CLI messages. */
export function useDiscussionMonitor(discussionIds: string[]) {
  const selectionKey = JSON.stringify(discussionIds);
  const [items, setItems] = useState<DiscussionMonitorItem[]>([]);
  const [error, setError] = useState(false);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const requestRefresh = useRef<() => void>(() => {});

  useEffect(() => {
    const ids: string[] = JSON.parse(selectionKey);
    let disposed = false;
    let inFlight = false;
    let dirty = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let scheduledAt = 0;
    let controller: AbortController | undefined;
    let deadline: ReturnType<typeof setTimeout> | undefined;
    const schedule = (delay: number) => {
      clearTimeout(timer);
      timer = undefined;
      if (!disposed && !document.hidden) {
        scheduledAt = Date.now() + delay;
        timer = setTimeout(() => { timer = undefined; void load(); }, delay);
      }
    };
    async function load() {
      if (disposed || document.hidden) return;
      if (inFlight) { dirty = true; return; }
      inFlight = true;
      dirty = false;
      const request = new AbortController();
      controller = request;
      deadline = setTimeout(() => request.abort(), 15000);
      try {
        const next = await discussions.monitor(ids, request.signal);
        if (disposed) return;
        setItems(previous => next.map(item => {
          const old = previous.find(value => value.id === item.id);
          return old && JSON.stringify(old) === JSON.stringify(item) ? old : item;
        }));
        setError(false);
        setUpdatedAt(Date.now());
      } catch {
        if (!disposed) setError(true);
      } finally {
        clearTimeout(deadline);
        inFlight = false;
        schedule(dirty ? 250 : 5000);
      }
    }
    requestRefresh.current = () => {
      // Throttle a continuous stream: newer events must not postpone a refresh.
      if (!timer || scheduledAt > Date.now() + 250) schedule(250);
    };
    const visibility = () => { if (document.hidden) { clearTimeout(timer); timer = undefined; } else schedule(0); };
    document.addEventListener('visibilitychange', visibility);
    schedule(0);
    return () => {
      disposed = true;
      clearTimeout(timer);
      clearTimeout(deadline);
      controller?.abort();
      requestRefresh.current = () => {};
      document.removeEventListener('visibilitychange', visibility);
    };
  }, [selectionKey]);

  const refresh = useCallback(() => requestRefresh.current(), []);
  const onMessage = useCallback((message: WsMessage) => {
    const ids: string[] = JSON.parse(selectionKey);
    const matchingId = 'discussion_id' in message && message.discussion_id != null && ids.includes(message.discussion_id);
    const matchingIds = 'discussion_ids' in message && message.discussion_ids.some(id => ids.includes(id));
    const matchingShared = 'shared_discussion_id' in message && (
      ids.includes(message.shared_discussion_id)
      || items.some(item => item.preview?.shared_id === message.shared_discussion_id)
    );
    if (matchingId || matchingIds || matchingShared) refresh();
  }, [selectionKey, items, refresh]);
  const { connectionState } = useWebSocket(onMessage, refresh);
  return { items, error, updatedAt, connectionState, refresh };
}
