import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../test/apiMock';
import type { ModelCatalogSnapshot, ModelCatalogView } from '../../types/generated';

vi.mock('../../lib/api', () => buildApiMock());
import { modelCatalogApi } from '../../lib/api';
import { useModelCatalogSnapshot } from '../useModelCatalogSnapshot';

const target = (overrides: Partial<ModelCatalogView> = {}): ModelCatalogView => ({
  runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', target_label: 'Claude',
  models: [], stale: true, live_refresh_ok: false, last_attempt_at: null,
  last_live_success_at: null, last_error_reason: null, last_error_detail: null,
  ...overrides,
});
const snapshot = (view = target()): ModelCatalogSnapshot => ({ targets: [view] });

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(modelCatalogApi.list).mockResolvedValue(snapshot());
  vi.mocked(modelCatalogApi.refresh).mockResolvedValue(target({ stale: false, live_refresh_ok: true }));
});
afterEach(cleanup);

it('refreshes a stale CLI on opening and publishes the returned catalogue', async () => {
  const fresh = target({ stale: false, live_refresh_ok: true, target_label: 'Fresh Claude' });
  vi.mocked(modelCatalogApi.refresh).mockResolvedValue(fresh);
  const { result } = renderHook(() => useModelCatalogSnapshot(true));
  await waitFor(() => expect(modelCatalogApi.refresh).toHaveBeenCalledWith({ runtime_target_id: 'agent:claude-code', agent_type: 'ClaudeCode', force: false }));
  await waitFor(() => expect(result.current.catalog?.targets[0]).toEqual(fresh));
});

it('retains the visible snapshot while refreshing and after an error', async () => {
  let reject!: (error: Error) => void;
  vi.mocked(modelCatalogApi.refresh).mockReturnValue(new Promise((_, fail) => { reject = fail; }));
  const { result } = renderHook(() => useModelCatalogSnapshot(true));
  await waitFor(() => expect(result.current.catalog).toEqual(snapshot()));
  await waitFor(() => expect(modelCatalogApi.refresh).toHaveBeenCalledTimes(1));
  await act(async () => reject(new Error('offline')));
  await waitFor(() => expect(result.current.catalogError).toBe(true));
  expect(result.current.catalog?.targets[0].runtime_target_id).toBe('agent:claude-code');
});

it('deduplicates overlapping openings of the same target', async () => {
  let resolve!: (value: ModelCatalogView) => void;
  vi.mocked(modelCatalogApi.refresh).mockReturnValue(new Promise(done => { resolve = done; }));
  const left = renderHook(() => useModelCatalogSnapshot(true));
  const right = renderHook(() => useModelCatalogSnapshot(true));
  await waitFor(() => expect(left.result.current.catalog).not.toBeNull());
  await waitFor(() => expect(right.result.current.catalog).not.toBeNull());
  expect(modelCatalogApi.refresh).toHaveBeenCalledTimes(1);
  await act(async () => resolve(target({ stale: false, live_refresh_ok: true })));
  await waitFor(() => expect(right.result.current.catalog?.targets[0].stale).toBe(false));
});

it('does not discover closed selectors, fresh targets or HTTP connections', async () => {
  const closed = renderHook(() => useModelCatalogSnapshot(false));
  expect(modelCatalogApi.list).not.toHaveBeenCalled();
  closed.unmount();
  vi.mocked(modelCatalogApi.list).mockResolvedValue({ targets: [target({ stale: false, live_refresh_ok: true }), target({ runtime_target_id: 'http:private', agent_type: 'ClaudeCode' })] });
  const { result } = renderHook(() => useModelCatalogSnapshot(true));
  await waitFor(() => expect(result.current.catalog?.targets).toHaveLength(2));
  expect(modelCatalogApi.refresh).not.toHaveBeenCalled();
});

it('ignores a late catalogue after closing and refetches on a later opening', async () => {
  let finish!: (value: ModelCatalogView) => void;
  vi.mocked(modelCatalogApi.refresh).mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
  const hook = renderHook(({ open }) => useModelCatalogSnapshot(open), { initialProps: { open: true } });
  await waitFor(() => expect(modelCatalogApi.refresh).toHaveBeenCalledTimes(1));
  hook.rerender({ open: false });
  await act(async () => finish(target({ stale: false, live_refresh_ok: true })));
  expect(hook.result.current.catalog?.targets[0].stale).toBe(true);
  hook.rerender({ open: true });
  await waitFor(() => expect(modelCatalogApi.refresh).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(hook.result.current.catalog?.targets[0].stale).toBe(false));
});

it('rejects a refreshed view belonging to another runtime', async () => {
  vi.mocked(modelCatalogApi.refresh).mockResolvedValue(target({ runtime_target_id: 'http:other', stale: false, live_refresh_ok: true }));
  const { result } = renderHook(() => useModelCatalogSnapshot(true));
  await waitFor(() => expect(result.current.catalogError).toBe(true));
  expect(result.current.catalog?.targets[0].runtime_target_id).toBe('agent:claude-code');
  expect(result.current.catalog?.targets[0].stale).toBe(true);
});

it('preserves the display label when the refresh response only names the runtime', async () => {
  vi.mocked(modelCatalogApi.refresh).mockResolvedValue(target({ target_label: null, stale: false, live_refresh_ok: true }));
  const { result } = renderHook(() => useModelCatalogSnapshot(true));
  await waitFor(() => expect(result.current.catalog?.targets[0].stale).toBe(false));
  expect(result.current.catalog?.targets[0].target_label).toBe('Claude');
});

it('bounds concurrent refreshes and leaves HTTP targets untouched', async () => {
  const views = [target(), target({ runtime_target_id: 'agent:codex', agent_type: 'Codex' }), target({ runtime_target_id: 'agent:opencode', agent_type: 'OpenCode' })];
  vi.mocked(modelCatalogApi.list).mockResolvedValue({ targets: views });
  const finishes: Array<() => void> = [];
  vi.mocked(modelCatalogApi.refresh).mockImplementation(request => new Promise(resolve => {
    finishes.push(() => resolve({ ...views.find(view => view.runtime_target_id === request.runtime_target_id)!, stale: false, live_refresh_ok: true }));
  }));
  const { result } = renderHook(() => useModelCatalogSnapshot(true));
  await waitFor(() => expect(modelCatalogApi.refresh).toHaveBeenCalledTimes(2));
  await act(async () => finishes[0]());
  await waitFor(() => expect(modelCatalogApi.refresh).toHaveBeenCalledTimes(3));
  await act(async () => { finishes[1](); finishes[2](); });
  await waitFor(() => expect(result.current.catalog?.targets.every(view => !view.stale)).toBe(true));
});
