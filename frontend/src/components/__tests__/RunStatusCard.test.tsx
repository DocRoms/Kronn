import { act, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RunStatusCard, workflowRunStatusCardModel } from '../RunStatusCard';
import { runsApi, getApiBase, getAuthToken } from '../../lib/api';
import { activeWebSocketCountForTests } from '../../hooks/useWebSocket';
import type { SharedRun } from '../../types/generated';

vi.mock('../../lib/api', () => ({
  runsApi: { get: vi.fn(), list: vi.fn() },
  getApiBase: vi.fn(() => ''),
  getAuthToken: vi.fn(() => null),
}));

// ─── Mock WebSocket — mirrors hooks/__tests__/useWebSocket.test.ts so the
// shared-socket registry (hooks/useWebSocket.ts) runs for real against a
// controllable transport instead of a live connection. ──────────────────
class MockWebSocket {
  static instances: MockWebSocket[] = [];
  url: string;
  readyState = 0;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  sent: string[] = [];
  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
  }
  send(data: string) { this.sent.push(data); }
  close() { this.readyState = 3; this.onclose?.(); }
  simulateOpen() { this.readyState = 1; this.onopen?.(); }
  simulateMessage(data: string) { this.onmessage?.({ data }); }
  static readonly OPEN = 1;
}

function sharedRun(overrides: Partial<SharedRun>): SharedRun {
  return {
    id: 'run-1',
    kind: 'quick_api',
    source_id: 'source-1',
    project_id: null,
    discussion_id: null,
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

// ─── IntersectionObserver mock — captures the callback so tests can flip
// visibility deterministically instead of relying on real layout. ────────
class MockIntersectionObserver {
  static instances: MockIntersectionObserver[] = [];
  callback: (entries: Array<{ isIntersecting: boolean }>) => void;
  constructor(callback: (entries: Array<{ isIntersecting: boolean }>) => void) {
    this.callback = callback;
    MockIntersectionObserver.instances.push(this);
  }
  observe() {}
  disconnect() {}
  setIntersecting(value: boolean) { this.callback([{ isIntersecting: value }]); }
}

describe('RunStatusCard', () => {
  let originalWebSocket: typeof WebSocket;
  let originalIntersectionObserver: typeof IntersectionObserver | undefined;

  beforeEach(() => {
    MockWebSocket.instances = [];
    MockIntersectionObserver.instances = [];
    originalWebSocket = globalThis.WebSocket;
    (globalThis as any).WebSocket = MockWebSocket;
    originalIntersectionObserver = globalThis.IntersectionObserver;
    (globalThis as any).IntersectionObserver = MockIntersectionObserver;
    vi.mocked(getApiBase).mockReturnValue('');
    vi.mocked(getAuthToken).mockReturnValue(null);
    vi.mocked(runsApi.get).mockReset();
  });

  afterEach(() => {
    globalThis.WebSocket = originalWebSocket;
    if (originalIntersectionObserver) globalThis.IntersectionObserver = originalIntersectionObserver;
    else delete (globalThis as any).IntersectionObserver;
  });

  it('renders only measured workflow progress and duration', () => {
    const model = workflowRunStatusCardModel({
      id: 'run-1',
      status: 'Running',
      started_at: '2026-08-31T10:00:00Z',
      finished_at: null,
      step_results: [
        { step_name: 'collect', status: 'Success' },
        { step_name: 'publish', status: 'Running' },
      ],
    });

    render(<RunStatusCard model={model} />);

    expect(screen.getByTestId('run-status-card')).toHaveAttribute('data-kind', 'workflow');
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '1');
    expect(screen.getByRole('progressbar')).toHaveAttribute('aria-valuemax', '2');
    expect(screen.getByText('publish')).toBeInTheDocument();
  });

  it('makes unavailable duration and diagnostics explicit for a failed direct run', () => {
    render(
      <RunStatusCard
        model={{
          id: 'qa-1',
          kind: 'quick_api',
          status: 'preflight_failed',
          diagnostic: 'The configured API is unavailable.',
          freshness: 'unavailable',
        }}
      />,
    );

    expect(screen.getByText('run.durationUnavailable')).toBeInTheDocument();
    expect(screen.getByText('The configured API is unavailable.')).toBeInTheDocument();
    expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
  });

  it('rehydrates from server state via runId and marks freshness as rehydrated', async () => {
    vi.mocked(runsApi.get).mockResolvedValue(sharedRun({ id: 'run-2', kind: 'quick_exec', status: 'success', finished_at: '2026-08-31T10:00:05Z', duration_ms: 5000 }));

    render(<RunStatusCard runId="run-2" />);

    await waitFor(() => expect(runsApi.get).toHaveBeenCalledWith('run-2'));
    await waitFor(() => expect(screen.getByTestId('run-status-card')).toHaveAttribute('data-status', 'success'));
    expect(screen.getByTestId('run-status-card')).toHaveAttribute('data-kind', 'quick_exec');
    expect(screen.getByText('run.freshness.rehydrated')).toBeInTheDocument();
  });

  it('renders a QP run linked to its discussion and a workflow run linked to its detail page', async () => {
    vi.mocked(runsApi.get).mockResolvedValueOnce(sharedRun({ id: 'run-qp', kind: 'quick_prompt', status: 'success', discussion_id: 'disc-9' }));
    render(<RunStatusCard runId="run-qp" />);
    await waitFor(() => expect(screen.getByTestId('run-status-card').querySelector('.run-status-card-link')).toHaveAttribute('href', '/discussions/disc-9'));

    vi.mocked(runsApi.get).mockResolvedValueOnce(sharedRun({ id: 'run-wf', kind: 'workflow', status: 'running', source_id: 'wf-1', discussion_id: null }));
    render(<RunStatusCard runId="run-wf" />);
    const cards = screen.getAllByTestId('run-status-card');
    const workflowCard = cards[cards.length - 1];
    await waitFor(() => expect(workflowCard.querySelector('.run-status-card-link')).toHaveAttribute('href', '/workflows/wf-1?run=run-wf'));
  });

  it('isolates two concurrent runs over one shared WebSocket — each hydrates only on its own SharedRunUpdated event', async () => {
    vi.mocked(runsApi.get).mockImplementation(async (id: string) => {
      if (id === 'run-a') return sharedRun({ id: 'run-a', kind: 'quick_api', status: 'running' });
      return sharedRun({ id: 'run-b', kind: 'quick_exec', status: 'running' });
    });

    render(
      <>
        <div data-testid="card-a"><RunStatusCard runId="run-a" /></div>
        <div data-testid="card-b"><RunStatusCard runId="run-b" /></div>
      </>,
    );

    await waitFor(() => expect(runsApi.get).toHaveBeenCalledWith('run-a'));
    await waitFor(() => expect(runsApi.get).toHaveBeenCalledWith('run-b'));

    // Both cards are active (running) → they share the single underlying socket.
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));
    const callsBefore = vi.mocked(runsApi.get).mock.calls.length;

    vi.mocked(runsApi.get).mockImplementation(async (id: string) => {
      if (id === 'run-a') return sharedRun({ id: 'run-a', kind: 'quick_api', status: 'success', finished_at: '2026-08-31T10:00:01Z', duration_ms: 1000 });
      return sharedRun({ id: 'run-b', kind: 'quick_exec', status: 'running' });
    });

    act(() => {
      MockWebSocket.instances[0].simulateMessage(JSON.stringify({ type: 'shared_run_updated', run_id: 'run-a' }));
    });

    await waitFor(() => expect(within(screen.getByTestId('card-a')).getByTestId('run-status-card')).toHaveAttribute('data-status', 'success'));
    // run-b never received a matching event — it must still be running, untouched.
    expect(within(screen.getByTestId('card-b')).getByTestId('run-status-card')).toHaveAttribute('data-status', 'running');
    // Exactly one extra fetch happened (run-a's rehydrate) — run-b was not re-fetched.
    expect(vi.mocked(runsApi.get).mock.calls.length).toBe(callsBefore + 1);
  });

  it('stops rehydrating on live events once the card leaves the viewport', async () => {
    vi.mocked(runsApi.get).mockResolvedValue(sharedRun({ id: 'run-3', kind: 'quick_api', status: 'running' }));

    render(<RunStatusCard runId="run-3" />);
    await waitFor(() => expect(runsApi.get).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));

    // Leave the viewport.
    act(() => { MockIntersectionObserver.instances[0].setIntersecting(false); });

    // The shared socket registry drops its only subscriber and tears down —
    // an off-screen card must not keep a live connection open (DoD #6).
    await waitFor(() => expect(activeWebSocketCountForTests()).toBe(0));

    const callsBefore = vi.mocked(runsApi.get).mock.calls.length;
    act(() => {
      MockWebSocket.instances[0].simulateMessage(JSON.stringify({ type: 'shared_run_updated', run_id: 'run-3' }));
    });
    // No re-fetch: the card suspended its subscription while off-screen.
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(vi.mocked(runsApi.get).mock.calls.length).toBe(callsBefore);
  });

  it('rehydrates on WebSocket reconnect (restart) so progress is never invented across a reconnect gap', async () => {
    vi.mocked(runsApi.get).mockResolvedValue(sharedRun({ id: 'run-4', kind: 'workflow', status: 'running' }));

    render(<RunStatusCard runId="run-4" />);
    await waitFor(() => expect(runsApi.get).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(MockWebSocket.instances).toHaveLength(1));

    act(() => { MockWebSocket.instances[0].simulateOpen(); });
    await waitFor(() => expect(vi.mocked(runsApi.get).mock.calls.length).toBeGreaterThanOrEqual(2));
  });
});
