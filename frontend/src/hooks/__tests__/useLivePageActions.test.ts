import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePageAction } from '../../types/generated';

vi.mock('../../lib/api', () => ({ pages: { actions: vi.fn(), actionLaunches: vi.fn(() => Promise.resolve([])) } }));

import { pages as pagesApi } from '../../lib/api';
import { useLivePageActions } from '../useLivePageActions';

function action(overrides: Partial<LivePageAction> = {}): LivePageAction {
  return {
    id: 'page-action:page-1:refresh', live_page_id: 'page-1', live_page_revision_id: 'rev-1',
    action_ref: 'refresh', kind: 'workflow', target_id: 'wf-1', target_name: 'Refresh report',
    project_id: null, project_name: null, state: 'proposed', values: [], shared_run_id: null,
    result_discussion_id: null, deep_link: null, diagnostic: null, launched_at: null,
    finished_at: null, created_at: '2026-09-01T08:00:00Z', updated_at: '2026-09-01T08:00:00Z',
    stale_source: false, binding_key: null,
    ...overrides,
  };
}

const anchor = { left: 10, top: 20, width: 100, height: 30 };

beforeEach(() => {
  vi.mocked(pagesApi.actions).mockReset();
  vi.mocked(pagesApi.actionLaunches).mockReset().mockResolvedValue([]);
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

  it('a second click on the same button closes its card, the next one opens a fresh one', async () => {
    vi.mocked(pagesApi.actions).mockResolvedValue([action()]);
    const { result } = renderHook(() => useLivePageActions(vi.fn()));
    await act(() => result.current.reload('page-1'));

    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-1' }, anchor }));
    const firstActivation = result.current.activeAction?.activation ?? 0;
    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-1' }, anchor }));
    expect(result.current.activeAction).toBeNull();

    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-1' }, anchor }));
    expect(result.current.activeAction?.activation).toBeGreaterThan(firstActivation);
  });

  it('another row of the same block switches the card instead of closing it', async () => {
    vi.mocked(pagesApi.actions).mockResolvedValue([action()]);
    const { result } = renderHook(() => useLivePageActions(vi.fn()));
    await act(() => result.current.reload('page-1'));

    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-1' }, anchor }));
    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-2' }, anchor }));

    expect(result.current.activeAction?.bindings).toEqual({ ticket: 'EW-2' });
  });

  it('a row that has run reopens on its latest run, running or finished', async () => {
    const running = action({ id: 'page-launch:1', state: 'running', shared_run_id: 'run-1', binding_key: 'ticket=EW-1' });
    const done = action({ id: 'page-launch:2', state: 'succeeded', binding_key: 'ticket=EW-2' });
    vi.mocked(pagesApi.actions).mockResolvedValue([action()]);
    vi.mocked(pagesApi.actionLaunches).mockResolvedValue([running, done]);
    const { result } = renderHook(() => useLivePageActions(vi.fn()));
    await act(() => result.current.reload('page-1'));
    expect(result.current.launches).toEqual([running, done]);

    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-1' }, anchor }));
    expect(result.current.selectedAction).toEqual(running);
    expect(result.current.selectedOffer).toEqual(action());

    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-2' }, anchor }));
    expect(result.current.selectedAction).toEqual(done);

    // A row that never ran opens on the offer.
    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-3' }, anchor }));
    expect(result.current.selectedAction).toEqual(action());
  });

  it('a launch seen on a card updates the button states, a decline does not', async () => {
    vi.mocked(pagesApi.actions).mockResolvedValue([action()]);
    const { result } = renderHook(() => useLivePageActions(vi.fn()));
    await act(() => result.current.reload('page-1'));
    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-1' }, anchor }));
    const activation = result.current.activeAction!.activation;

    act(() => result.current.handleChanged(action({ id: 'page-launch:c', state: 'cancelled', binding_key: '' }), activation));
    expect(result.current.launches).toEqual([]);

    const launching = action({ id: 'page-launch:1', state: 'launching', binding_key: 'ticket=EW-1' });
    act(() => result.current.handleChanged(launching, activation));
    expect(result.current.launches).toEqual([launching]);
  });

  it('the buttons keep their actions when their states cannot be read', async () => {
    vi.mocked(pagesApi.actions).mockResolvedValue([action()]);
    vi.mocked(pagesApi.actionLaunches).mockRejectedValue(new Error('offline'));
    const { result } = renderHook(() => useLivePageActions(vi.fn()));
    await act(() => result.current.reload('page-1'));

    expect(result.current.actions).toEqual([action()]);
    expect(result.current.launches).toEqual([]);
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

  it('handleChanged follows the open card without touching the offer', async () => {
    vi.mocked(pagesApi.actions).mockResolvedValue([action()]);
    const { result } = renderHook(() => useLivePageActions(vi.fn()));
    await act(() => result.current.reload('page-1'));
    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: {}, anchor }));

    const launching = action({ id: 'page-launch:1', state: 'launching' });
    act(() => result.current.handleChanged(launching, result.current.activeAction!.activation));

    await waitFor(() => expect(result.current.selectedAction).toEqual(launching));
    expect(result.current.actions).toEqual([action()]);
  });

  it('a click on another row of the same block opens the offer, not the last launch', async () => {
    // The reported shape: one "Framer" block, a button per ticket. After the
    // first ticket's launch succeeded, every other button reopened on it.
    vi.mocked(pagesApi.actions).mockResolvedValue([action()]);
    const { result } = renderHook(() => useLivePageActions(vi.fn()));
    await act(() => result.current.reload('page-1'));

    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-7706' }, anchor }));
    act(() => result.current.handleChanged(
      action({ id: 'page-launch:7706', state: 'succeeded' }),
      result.current.activeAction!.activation,
    ));
    expect(result.current.selectedAction?.state).toBe('succeeded');

    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-7704' }, anchor }));

    expect(result.current.selectedAction).toEqual(action());
    expect(result.current.activeAction?.bindings).toEqual({ ticket: 'EW-7704' });
  });

  it('a launch answering after the user moved to another row stays on its own card', async () => {
    vi.mocked(pagesApi.actions).mockResolvedValue([action()]);
    const { result } = renderHook(() => useLivePageActions(vi.fn()));
    await act(() => result.current.reload('page-1'));
    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-7706' }, anchor }));
    const firstClick = result.current.activeAction!.activation;
    act(() => result.current.handleIntent({ actionRef: 'refresh', bindings: { ticket: 'EW-7704' }, anchor }));

    act(() => result.current.handleChanged(action({ id: 'page-launch:7706', state: 'running' }), firstClick));

    expect(result.current.selectedAction).toEqual(action());
  });
});
