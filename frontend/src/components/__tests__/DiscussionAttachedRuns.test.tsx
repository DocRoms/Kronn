import { act, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DiscussionAttachedRuns } from '../DiscussionAttachedRuns';
import { runsApi, getApiBase, getAuthToken } from '../../lib/api';
import type { SharedRun } from '../../types/generated';

vi.mock('../../lib/api', () => ({
  runsApi: { get: vi.fn(), list: vi.fn() },
  getApiBase: vi.fn(() => ''),
  getAuthToken: vi.fn(() => null),
}));

// RunStatusCard (rendered per attached run) opens its own live socket via
// hooks/useWebSocket.ts — stub the transport so jsdom doesn't attempt a real
// connection.
class MockWebSocket {
  readyState = 0;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  send() {}
  close() { this.readyState = 3; this.onclose?.(); }
  static readonly OPEN = 1;
}

function sharedRun(overrides: Partial<SharedRun>): SharedRun {
  return {
    id: 'run-1',
    kind: 'quick_prompt',
    source_id: 'qp-1',
    project_id: null,
    discussion_id: 'disc-1',
    status: 'running',
    started_at: '2026-08-31T10:00:00Z',
    finished_at: null,
    duration_ms: null,
    result: null,
    diagnostic: null,
    created_at: '2026-08-31T10:00:00Z',
    updated_at: '2026-08-31T10:00:00Z',
    ...overrides,
  };
}

describe('DiscussionAttachedRuns', () => {
  let originalWebSocket: typeof WebSocket;

  beforeEach(() => {
    originalWebSocket = globalThis.WebSocket;
    (globalThis as any).WebSocket = MockWebSocket;
    vi.mocked(getApiBase).mockReturnValue('');
    vi.mocked(getAuthToken).mockReturnValue(null);
    vi.mocked(runsApi.list).mockReset();
    vi.mocked(runsApi.get).mockReset();
  });

  afterEach(() => {
    globalThis.WebSocket = originalWebSocket;
  });

  it('renders nothing when the discussion has no attached runs', async () => {
    vi.mocked(runsApi.list).mockResolvedValue([]);
    render(<DiscussionAttachedRuns discussionId="disc-1" />);
    await waitFor(() => expect(runsApi.list).toHaveBeenCalledWith({ discussionId: 'disc-1', limit: 20 }));
    expect(screen.queryByTestId('disc-attached-runs')).not.toBeInTheDocument();
  });

  it('renders a RunStatusCard, via the shared model, for each run attached to the discussion', async () => {
    vi.mocked(runsApi.list).mockResolvedValue([
      sharedRun({ id: 'run-a', kind: 'quick_api', status: 'running' }),
      sharedRun({ id: 'run-b', kind: 'workflow', status: 'success', source_id: 'wf-9', discussion_id: null }),
    ]);
    vi.mocked(runsApi.get).mockImplementation(async (id: string) =>
      sharedRun({ id, kind: id === 'run-b' ? 'workflow' : 'quick_api', status: id === 'run-b' ? 'success' : 'running', source_id: id === 'run-b' ? 'wf-9' : 'qp-1', discussion_id: id === 'run-b' ? null : 'disc-1' }),
    );

    render(<DiscussionAttachedRuns discussionId="disc-1" />);

    await waitFor(() => expect(screen.getByTestId('disc-attached-runs')).toBeInTheDocument());
    const cards = await screen.findAllByTestId('run-status-card');
    expect(cards).toHaveLength(2);
  });

  it('relists when an unknown run_id event arrives (a run started elsewhere just got attached)', async () => {
    vi.mocked(runsApi.list).mockResolvedValueOnce([]);
    const { rerender } = render(<DiscussionAttachedRuns discussionId="disc-1" runEvent={undefined} />);
    await waitFor(() => expect(runsApi.list).toHaveBeenCalledTimes(1));
    expect(screen.queryByTestId('disc-attached-runs')).not.toBeInTheDocument();

    vi.mocked(runsApi.list).mockResolvedValueOnce([sharedRun({ id: 'run-late' })]);
    vi.mocked(runsApi.get).mockResolvedValue(sharedRun({ id: 'run-late' }));
    rerender(<DiscussionAttachedRuns discussionId="disc-1" runEvent={{ runId: 'run-late', seq: 1 }} />);

    await waitFor(() => expect(runsApi.list).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByTestId('disc-attached-runs')).toBeInTheDocument());
  });

  it('does NOT relist for an event targeting an already-known run_id (its own card self-hydrates instead)', async () => {
    vi.mocked(runsApi.list).mockResolvedValueOnce([sharedRun({ id: 'run-known' })]);
    vi.mocked(runsApi.get).mockResolvedValue(sharedRun({ id: 'run-known' }));
    const { rerender } = render(<DiscussionAttachedRuns discussionId="disc-1" runEvent={undefined} />);
    await waitFor(() => expect(runsApi.list).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByTestId('disc-attached-runs')).toBeInTheDocument());

    rerender(<DiscussionAttachedRuns discussionId="disc-1" runEvent={{ runId: 'run-known', seq: 1 }} />);
    // Give the debounce window a chance to fire, if it were (wrongly) scheduled.
    await new Promise(resolve => setTimeout(resolve, 300));
    expect(runsApi.list).toHaveBeenCalledTimes(1);
  });

  it('coalesces a burst of unknown-run_id events into a single debounced relist', async () => {
    vi.mocked(runsApi.list).mockResolvedValueOnce([]);
    const { rerender } = render(<DiscussionAttachedRuns discussionId="disc-1" runEvent={undefined} />);
    await waitFor(() => expect(runsApi.list).toHaveBeenCalledTimes(1));

    vi.mocked(runsApi.list).mockResolvedValue([sharedRun({ id: 'run-x' }), sharedRun({ id: 'run-y' })]);
    vi.mocked(runsApi.get).mockResolvedValue(sharedRun({ id: 'run-x' }));
    // Three rapid unknown events for two different new runs — a burst.
    act(() => { rerender(<DiscussionAttachedRuns discussionId="disc-1" runEvent={{ runId: 'run-x', seq: 1 }} />); });
    act(() => { rerender(<DiscussionAttachedRuns discussionId="disc-1" runEvent={{ runId: 'run-y', seq: 2 }} />); });
    act(() => { rerender(<DiscussionAttachedRuns discussionId="disc-1" runEvent={{ runId: 'run-x', seq: 3 }} />); });

    await waitFor(() => expect(runsApi.list).toHaveBeenCalledTimes(2));
    // Only ONE relist fired for the whole burst, not one per event.
    await new Promise(resolve => setTimeout(resolve, 300));
    expect(runsApi.list).toHaveBeenCalledTimes(2);
  });

  it('does not duplicate the initial fetch when runEvent is present from the very first render', async () => {
    vi.mocked(runsApi.list).mockResolvedValue([]);
    render(<DiscussionAttachedRuns discussionId="disc-1" runEvent={{ runId: 'irrelevant', seq: 0 }} />);
    await waitFor(() => expect(runsApi.list).toHaveBeenCalledTimes(1));
    // No extra fetch fires just because runEvent was already defined on mount
    // — seq 0 never advances past the initial baseline.
    await new Promise(resolve => setTimeout(resolve, 300));
    expect(runsApi.list).toHaveBeenCalledTimes(1);
  });

  it('reloads its own list when navigating to a different discussion', async () => {
    vi.mocked(runsApi.list).mockResolvedValueOnce([sharedRun({ id: 'run-a', discussion_id: 'disc-1' })]);
    vi.mocked(runsApi.get).mockResolvedValue(sharedRun({ id: 'run-a', discussion_id: 'disc-1' }));
    const { rerender } = render(<DiscussionAttachedRuns discussionId="disc-1" />);
    await waitFor(() => expect(screen.getByTestId('disc-attached-runs')).toBeInTheDocument());

    vi.mocked(runsApi.list).mockResolvedValueOnce([]);
    rerender(<DiscussionAttachedRuns discussionId="disc-2" />);
    await waitFor(() => expect(runsApi.list).toHaveBeenCalledWith({ discussionId: 'disc-2', limit: 20 }));
    await waitFor(() => expect(screen.queryByTestId('disc-attached-runs')).not.toBeInTheDocument());
  });
});
