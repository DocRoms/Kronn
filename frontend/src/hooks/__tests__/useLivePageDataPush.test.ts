import { renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { WsMessage } from '../../types/generated';

const handlers: Array<(message: WsMessage) => void> = [];
vi.mock('../useWebSocket', () => ({
  useWebSocket: (onMessage: (message: WsMessage) => void) => { handlers.push(onMessage); return { connected: true, connectionState: 'connected' }; },
}));

import { useLivePageDataPush } from '../useLivePageDataPush';

const send = (message: WsMessage) => handlers[handlers.length - 1](message);

beforeEach(() => { handlers.length = 0; });

describe('useLivePageDataPush', () => {
  it('re-reads only the open Page when its data changes', () => {
    const onChange = vi.fn();
    renderHook(() => useLivePageDataPush('page-1', onChange));
    send({ type: 'live_page_data_changed', page_id: 'other', data_revision: 3 });
    send({ type: 'embed_origins_changed' });
    expect(onChange).not.toHaveBeenCalled();
    send({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 4 });
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('does nothing without an open Page', () => {
    const onChange = vi.fn();
    renderHook(() => useLivePageDataPush(null, onChange));
    send({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 1 });
    expect(onChange).not.toHaveBeenCalled();
  });
});
