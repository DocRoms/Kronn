import { useCallback, useLayoutEffect, useRef } from 'react';
import type { WsMessage } from '../types/generated';
import { useWebSocket } from './useWebSocket';

/**
 * Calls `onChange` as soon as the backend announces new data for `pageId`
 * (KT-1030), so an open Page follows a publish without waiting for its poll.
 */
export function useLivePageDataPush(pageId: string | null | undefined, onChange: () => void): void {
  const changeRef = useRef(onChange);
  useLayoutEffect(() => { changeRef.current = onChange; }, [onChange]);
  const onMessage = useCallback((message: WsMessage) => {
    if (message.type === 'live_page_data_changed' && pageId && message.page_id === pageId) {
      changeRef.current();
    }
  }, [pageId]);
  useWebSocket(onMessage, undefined, Boolean(pageId));
}
