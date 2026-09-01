import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePageAction } from '../../types/generated';

vi.mock('../../lib/api', () => ({ pages: { actions: vi.fn() } }));

import { pages as pagesApi } from '../../lib/api';
import { useLivePageActions } from '../useLivePageActions';

function action(overrides: Partial<LivePageAction> = {}): LivePageAction {
  return {
    id: 'page-action:page-1:refresh', live_page_id: 'page-1', live_page_revision_id: 'rev-1',
    action_ref: 'refresh', kind: 'workflow', target_id: 'wf-1', target_name: 'Refresh report',
    project_id: null, state: 'proposed', values: [], shared_run_id: null,
    result_discussion_id: null, deep_link: null, diagnostic: null, launched_at: null,
    finished_at: null, created_at: '2026-09-01T08:00:00Z', updated_at: '2026-09-01T08:00:00Z',
    stale_source: false,
    ...overrides,
  };
}

const anchor = { left: 10, top: 20, width: 100, height: 30 };

beforeEach(() => {
  vi.mocked(pagesApi.actions).mockReset();
});

describe('useLivePageActions', () => {
  it('activates a known action_ref and derives the matching action', async () => {
    vi.mocked(pagesApi.actions).mockResolvedValue([action()]);
    const onUnavailable = vi.fn();
    const { result } = renderHook(() => useLivePageActions(onUnavailable));

    await act(() => result.current.reload('page-1'));
    expect(result.current.actions).toEqual([action()]);

    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'KT-538' }, anchor }));

    expect(onUnavailable).not.toHaveBeenCalled();
    expect(result.current.selectedAction).toEqual(action());
    expect(result.current.activeAction).toMatchObject({ actionRef: 'refresh', bindings: { ticket: 'KT-538' }, anchor });
  });

  it('fails closed and never activates an unknown action_ref', async () => {
    vi.mocked(pagesApi.actions).mockResolvedValue([action()]);
    const onUnavailable = vi.fn();
    const { result } = renderHook(() => useLivePageActions(onUnavailable));
    await act(() => result.current.reload('page-1'));

    act(() => result.current.handleIntent({ actionRef: 'ghost', bindings: {}, anchor }));

    expect(onUnavailable).toHaveBeenCalledOnce();
    expect(result.current.activeAction).toBeNull();
    expect(result.current.selectedAction).toBeNull();
  });

  it('bumps activation on every intent so the same ref remounts a fresh card', async () => {
    vi.mocked(pagesApi.actions).mockResolvedValue([action()]);
    const { result } = renderHook(() => useLivePageActions(vi.fn()));
    await act(() => result.current.reload('page-1'));

    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: {}, anchor }));
    const firstActivation = result.current.activeAction?.activation;
    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: {}, anchor }));

    expect(result.current.activeAction?.activation).toBe((firstActivation ?? 0) + 1);
  });

  it('reload clears a pending activation and replaces the actions list', async () => {
    vi.mocked(pagesApi.actions).mockResolvedValueOnce([action()]);
    const { result } = renderHook(() => useLivePageActions(vi.fn()));
    await act(() => result.current.reload('page-1'));
    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: {}, anchor }));
    expect(result.current.activeAction).not.toBeNull();

    vi.mocked(pagesApi.actions).mockResolvedValueOnce([]);
    await act(() => result.current.reload('page-2'));

    expect(pagesApi.actions).toHaveBeenLastCalledWith('page-2');
    expect(result.current.actions).toEqual([]);
    expect(result.current.activeAction).toBeNull();
  });

  it('reload with a null page id clears state without calling the API', async () => {
    const { result } = renderHook(() => useLivePageActions(vi.fn()));
    await act(() => result.current.reload(null));

    expect(pagesApi.actions).not.toHaveBeenCalled();
    expect(result.current.actions).toEqual([]);
  });

  it('handleChanged updates the stored action in place so a re-derived selection reflects it', async () => {
    vi.mocked(pagesApi.actions).mockResolvedValue([action()]);
    const { result } = renderHook(() => useLivePageActions(vi.fn()));
    await act(() => result.current.reload('page-1'));
    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: {}, anchor }));

    const launching = action({ state: 'launching' });
    act(() => result.current.handleChanged(launching));

    await waitFor(() => expect(result.current.selectedAction).toEqual(launching));
    expect(result.current.actions).toEqual([launching]);
  });
});
