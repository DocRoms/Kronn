import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePage, LivePageAction, LivePageDataDelta, LivePageDatasetView, LivePageDetail } from '../../types/generated';

const linkRelay = vi.hoisted(() => ({
  connect: vi.fn(),
  dispose: vi.fn(),
  onAction: null as ((intent: {
    actionRef: string;
    bindings: Record<string, string>;
    anchor: { left: number; top: number; width: number; height: number };
  }) => void) | null,
}));

const detail: LivePageDetail = {
  id: 'page-1', project_id: null, title: 'Production health', slug: 'production-health',
  current_revision_id: 'rev-1', data_revision: 2,
  created_at: '2026-08-26T10:00:00Z', updated_at: '2026-08-26T10:00:00Z',
  last_published_at: '2026-08-26T10:00:00Z', pinned: false, archived: false, html_size_bytes: 0,
  revision: {
    id: 'rev-1', page_id: 'page-1', revision: 1,
    html: '<main><h1>Production health</h1></main>',
    created_by_agent: 'Ollama', created_at: '2026-08-26T10:00:00Z',
  },
  datasets: [],
  slug_aliases: [],
};

vi.mock('../../lib/api', () => ({
  pages: { get: vi.fn(), data: vi.fn(), actions: vi.fn(), actionLaunches: vi.fn(() => Promise.resolve([])), getAction: vi.fn(), cancelAction: vi.fn(), launchAction: vi.fn() },
  // The card reads a workflow's Agent steps (KT-1025); none here.
  workflows: { get: vi.fn(() => Promise.resolve({ steps: [] })) },
  agents: { detect: vi.fn(() => Promise.resolve([])) },
  runsApi: {
    outcome: vi.fn(() => Promise.resolve({ discussion_count: 0, discussions: [] })),
    discussionOutcome: vi.fn((id: string) => Promise.resolve({ discussion_count: 1, discussions: [{ id, title: 'Result', agent: 'ClaudeCode', agent_status: 'answered', answer_excerpt: null, answer_truncated: false, answered_at: null, diagnostic: null, updated_at: '2026-09-18T10:00:00Z' }] })),
  },
}));
vi.mock('../../hooks/useWebSocket', () => ({ useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })) }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, ...args: (string | number)[]) => args.length ? `${key}:${args.join(',')}` : key }),
}));
vi.mock('../../lib/live-page-sandbox', async importOriginal => ({
  ...await importOriginal<Record<string, unknown>>(),
  createLivePageOpenLinkRelay: vi.fn((_channel, options) => {
    linkRelay.onAction = options?.onAction ?? null;
    return linkRelay;
  }),
}));
vi.mock('../../components/RunStatusCard', () => ({
  RunStatusCard: ({ runId }: { runId?: string }) => <div data-testid="run-card">{runId}</div>,
}));

import { pages as pagesApi } from '../../lib/api';
import { useWebSocket as useWebSocketMock } from '../../hooks/useWebSocket';
import { LIVE_PAGE_CSP, runtimeData } from '../../lib/live-page-sandbox';
import { StandaloneLivePage } from '../StandaloneLivePage';

function pageAction(overrides: Partial<LivePageAction> = {}): LivePageAction {
  return {
    id: 'page-action:page-1:refresh', live_page_id: 'page-1', live_page_revision_id: 'rev-1',
    action_ref: 'refresh', kind: 'workflow', target_id: 'wf-1', target_name: 'Refresh report',
    project_id: null, project_name: null, state: 'proposed', values: [], shared_run_id: null,
    result_discussion_id: null, deep_link: null, diagnostic: null, launched_at: null,
    finished_at: null, created_at: detail.created_at, updated_at: detail.updated_at,
    stale_source: false, binding_key: null,
    ...overrides,
  };
}

const anchor = { left: 24, top: 40, width: 120, height: 32 };

function summary({ revision: _revision, datasets: _datasets, slug_aliases: _aliases, ...page }: LivePageDetail): LivePage {
  return page;
}

/** What the data route answers when only `datasets` changed. */
function delta(page: Partial<LivePage> = {}, datasets: LivePageDatasetView[] = []): LivePageDataDelta {
  return { page: { ...summary(detail), ...page }, full: null, datasets };
}

function dataset(name: string, current: unknown): LivePageDatasetView {
  return {
    id: `dataset-${name}`, page_id: 'page-1', name, kind: 'snapshot', current, schema: null,
    max_points: 50_000, max_age_days: null, updated_at: detail.updated_at, points: [], data_size_bytes: 1,
  } as LivePageDatasetView;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: Error) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

function pageDataPosts(post: { mock: { calls: unknown[][] } }): unknown[] {
  return post.mock.calls
    .map(([message]) => message as { type?: string; data?: unknown })
    .filter(message => message.type === 'kronn:page-data')
    .map(message => message.data);
}

beforeEach(() => {
  linkRelay.connect.mockClear();
  linkRelay.dispose.mockClear();
  linkRelay.onAction = null;
  // Handlers of the views earlier specs rendered must not answer this one's events.
  vi.mocked(useWebSocketMock).mockClear();
  vi.mocked(pagesApi.get).mockReset().mockResolvedValue(detail);
  vi.mocked(pagesApi.data).mockReset().mockResolvedValue(delta());
  vi.mocked(pagesApi.actions).mockResolvedValue([]);
});

afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

describe('StandaloneLivePage', () => {
  it('reads only the dataset a publication changed and posts the same full snapshot', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    const loaded = { ...detail, datasets: [dataset('summary', { count: 1 }), dataset('heavy', { blob: 'kept' })] };
    vi.mocked(pagesApi.get).mockResolvedValue(loaded);
    render(<StandaloneLivePage pageId="page-1" />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const documentBefore = frame.getAttribute('srcdoc');
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');
    vi.mocked(pagesApi.data).mockResolvedValue(delta({ data_revision: 3 }, [dataset('summary', { count: 2 })]));

    const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
    await act(async () => {
      handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 3 }));
    });

    // What a full read of the Page would have given the frame.
    const full = { ...loaded, data_revision: 3, datasets: [dataset('summary', { count: 2 }), dataset('heavy', { blob: 'kept' })] };
    await waitFor(() => expect(pageDataPosts(post)).toEqual([runtimeData(full, {})]));
    expect(pagesApi.data).toHaveBeenCalledWith('page-1', 2, 'rev-1');
    expect(pagesApi.get).toHaveBeenCalledTimes(1);
    expect(frame.getAttribute('srcdoc')).toBe(documentBefore);
  });

  it('takes a full answer even when its revision is lower than the snapshot shown', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    render(<StandaloneLivePage pageId="page-1" />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');
    // The Page went back (restored, replaced): the server says so with the full state.
    const restored: LivePageDetail = { ...detail, data_revision: 1, datasets: [dataset('summary', { restored: true })] };
    vi.mocked(pagesApi.data).mockResolvedValue({ page: summary(restored), full: restored, datasets: [] });

    const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
    await act(async () => {
      handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 1 }));
    });

    await waitFor(() => expect(pageDataPosts(post)).toEqual([runtimeData(restored, {})]));
  });

  it('drops the answer of an older read that arrives after a newer one', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    render(<StandaloneLivePage pageId="page-1" />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');
    const older = deferred<LivePageDataDelta>();
    let newerAsked = false;
    vi.mocked(pagesApi.data).mockImplementation(() => (
      newerAsked ? Promise.resolve(delta({ data_revision: 4 })) : older.promise
    ));
    const publish = async (revision: number) => {
      const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
      await act(async () => {
        handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: revision }));
      });
    };
    await publish(3);
    newerAsked = true;
    await publish(4);
    await waitFor(() => expect(pageDataPosts(post)).toHaveLength(1));

    await act(async () => { older.resolve(delta({ data_revision: 3 })); });

    expect(pageDataPosts(post)).toEqual([expect.objectContaining({ page: expect.objectContaining({ data_revision: 4 }) })]);
  });

  it('keeps the read a publish started when the 30 s poll it overlapped answers late', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      render(<StandaloneLivePage pageId="page-1" />);
      const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
      const post = vi.spyOn(frame.contentWindow!, 'postMessage');
      const poll = deferred<LivePageDataDelta>();
      let published = false;
      vi.mocked(pagesApi.data).mockImplementation(() => (
        published ? Promise.resolve(delta({ data_revision: 3 })) : poll.promise
      ));
      await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
      expect(pagesApi.data).toHaveBeenCalledTimes(1);
      published = true;
      const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
      await act(async () => {
        handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 3 }));
      });
      await waitFor(() => expect(pageDataPosts(post)).toHaveLength(1));

      // The poll was asked before the publish: it still says revision 2.
      await act(async () => { poll.resolve(delta()); });

      expect(pageDataPosts(post)).toEqual([expect.objectContaining({ page: expect.objectContaining({ data_revision: 3 }) })]);
    } finally {
      vi.useRealTimers();
    }
  });

  it('ignores the late answer of the Page it left', async () => {
    const first = deferred<LivePageDetail>();
    const second: LivePageDetail = { ...detail, id: 'page-2', title: 'Second page', slug: 'second' };
    vi.mocked(pagesApi.get).mockImplementation(id => (id === 'page-1' ? first.promise : Promise.resolve(second)));
    const view = render(<StandaloneLivePage pageId="page-1" />);
    view.rerender(<StandaloneLivePage pageId="page-2" />);
    const frame = await screen.findByTestId('standalone-live-page-frame');
    expect(frame).toHaveAttribute('title', 'Second page');

    await act(async () => { first.resolve(detail); });

    expect(screen.getByTestId('standalone-live-page-frame')).toHaveAttribute('title', 'Second page');
  });

  it('shows nothing of the Page it left while the next one is still loading', async () => {
    const first = deferred<LivePageDetail>();
    const second = deferred<LivePageDetail>();
    vi.mocked(pagesApi.get).mockImplementation(id => (id === 'page-1' ? first.promise : second.promise));
    const view = render(<StandaloneLivePage pageId="page-1" />);
    view.rerender(<StandaloneLivePage pageId="page-2" />);

    await act(async () => { first.resolve(detail); });

    expect(screen.queryByTestId('standalone-live-page-frame')).toBeNull();
  });

  it('drops a read that was following the Page when the tab moves to another', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    const view = render(<StandaloneLivePage pageId="page-1" />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');
    const following = deferred<LivePageDataDelta>();
    vi.mocked(pagesApi.data).mockReturnValue(following.promise);
    const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
    await act(async () => {
      handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 3 }));
    });
    vi.mocked(pagesApi.get).mockReturnValue(deferred<LivePageDetail>().promise);
    view.rerender(<StandaloneLivePage pageId="page-2" />);

    await act(async () => { following.resolve(delta({ data_revision: 3 })); });

    // The frame may still post what it loaded with; the dropped read is revision 3.
    const dropped = (pageDataPosts(post) as { page: { data_revision: number } }[])
      .filter(data => data.page.data_revision === 3);
    expect(dropped).toEqual([]);
  });

  it('does not go back to older data when two reads are answered out of order', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    render(<StandaloneLivePage pageId="page-1" />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');
    const first = deferred<LivePageDataDelta>();
    const second = deferred<LivePageDataDelta>();
    let asked = false;
    vi.mocked(pagesApi.data).mockImplementation(() => (asked ? second.promise : first.promise));
    const publish = async () => {
      const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
      await act(async () => {
        handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 3 }));
      });
    };
    await publish();
    asked = true;
    await publish();
    const shown = () => (pageDataPosts(post).at(-1) as { page?: { data_revision?: number } } | undefined)?.page?.data_revision;

    // The read started first was served after the publication, the second before it.
    await act(async () => { first.resolve(delta({ data_revision: 3 })); });
    await act(async () => { second.resolve(delta()); });
    expect(shown()).toBe(3);
  });

  it('compares an answer with what is shown, even one shown in the same turn', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    render(<StandaloneLivePage pageId="page-1" />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');
    const first = deferred<LivePageDataDelta>();
    const second = deferred<LivePageDataDelta>();
    let asked = false;
    vi.mocked(pagesApi.data).mockImplementation(() => (asked ? second.promise : first.promise));
    const publish = async () => {
      const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
      await act(async () => {
        handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 3 }));
      });
    };
    await publish();
    asked = true;
    await publish();
    const shown = () => (pageDataPosts(post).at(-1) as { page?: { data_revision?: number } } | undefined)?.page?.data_revision;

    // Both answers land before React renders the first.
    await act(async () => {
      first.resolve(delta({ data_revision: 3 }));
      second.resolve(delta());
    });
    expect(shown()).toBe(3);
  });

  it('shows the answer of an older read when the read started after it fails', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    render(<StandaloneLivePage pageId="page-1" />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');
    const older = deferred<LivePageDataDelta>();
    let newerAsked = false;
    vi.mocked(pagesApi.data).mockImplementation(() => (
      newerAsked ? Promise.reject(new Error('network')) : older.promise
    ));
    const publish = async () => {
      const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
      await act(async () => {
        handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 3 }));
      });
    };
    await publish();
    newerAsked = true;
    await publish();

    await act(async () => { older.resolve(delta({ data_revision: 3 })); });

    expect(pageDataPosts(post)).toEqual([expect.objectContaining({ page: expect.objectContaining({ data_revision: 3 }) })]);
  });

  it('keeps updating during a burst of publishes that each start before the last one answered', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    render(<StandaloneLivePage pageId="page-1" />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');
    const answers = [deferred<LivePageDataDelta>(), deferred<LivePageDataDelta>(), deferred<LivePageDataDelta>()];
    let burst = 0;
    vi.mocked(pagesApi.data).mockImplementation(() => answers[burst].promise);
    const publish = async () => {
      const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
      await act(async () => {
        handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 3 }));
      });
    };
    const shown = () => (pageDataPosts(post).at(-1) as { page?: { data_revision?: number } } | undefined)?.page?.data_revision;
    await publish();
    burst = 1;
    await publish();
    burst = 2;
    await publish();

    // Each answers while later reads are still out: the view follows.
    await act(async () => { answers[0].resolve(delta({ data_revision: 3 })); });
    expect(shown()).toBe(3);
    await act(async () => { answers[1].resolve(delta({ data_revision: 4 })); });
    expect(shown()).toBe(4);
  });

  it('shows why the Page could not be opened, until a later read brings it', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    vi.mocked(pagesApi.get).mockRejectedValueOnce(new Error('backend away'));
    render(<StandaloneLivePage pageId="page-1" />);
    expect(await screen.findByRole('alert')).toHaveTextContent('backend away');

    const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
    await act(async () => {
      handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 2 }));
    });

    expect(await screen.findByTestId('standalone-live-page-frame')).toHaveAttribute('title', detail.title);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('does not report a first read that fails after a later read opened the Page', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    const first = deferred<LivePageDetail>();
    vi.mocked(pagesApi.get).mockReturnValueOnce(first.promise);
    render(<StandaloneLivePage pageId="page-1" />);
    const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
    await act(async () => {
      handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 2 }));
    });
    await screen.findByTestId('standalone-live-page-frame');

    await act(async () => { first.reject(new Error('too late')); });

    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByTestId('standalone-live-page-frame')).toBeInTheDocument();
  });

  it('polls the revision alone: an unchanged Page reads no dataset and posts nothing', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      vi.mocked(pagesApi.get).mockResolvedValue({ ...detail, datasets: [dataset('summary', { count: 1 })] });
      render(<StandaloneLivePage pageId="page-1" />);
      const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
      const post = vi.spyOn(frame.contentWindow!, 'postMessage');

      await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
      await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });

      expect(pagesApi.data).toHaveBeenCalledTimes(2);
      expect(pagesApi.data).toHaveBeenLastCalledWith('page-1', 2, 'rev-1');
      expect(pagesApi.get).toHaveBeenCalledTimes(1);
      expect(pageDataPosts(post)).toEqual([]);
    } finally {
      vi.useRealTimers();
    }
  });

  it('takes the full state when the data route answers with it', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    render(<StandaloneLivePage pageId="page-1" />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const rewritten: LivePageDetail = {
      ...detail, current_revision_id: 'rev-2', data_revision: 3,
      revision: { ...detail.revision, id: 'rev-2', revision: 2, html: '<main><h1>Rewritten</h1></main>' },
      datasets: [dataset('added', [1])],
    };
    vi.mocked(pagesApi.data).mockResolvedValue({ page: summary(rewritten), full: rewritten, datasets: [] });

    const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
    await act(async () => {
      handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 3 }));
    });

    await waitFor(() => expect(frame.getAttribute('srcdoc')).toContain('Rewritten'));
    expect(pagesApi.get).toHaveBeenCalledTimes(1);
  });

  it('pushes newly published data into the open frame without rebuilding it', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      render(<StandaloneLivePage pageId="page-1" />);
      const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
      const documentBefore = frame.getAttribute('srcdoc');
      const post = vi.spyOn(frame.contentWindow!, 'postMessage');
      vi.mocked(pagesApi.data).mockResolvedValue(delta({ data_revision: 3 }));

      await act(async () => { vi.advanceTimersByTime(30_000); });

      await waitFor(() => expect(post).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'kronn:page-data',
          data: expect.objectContaining({ page: expect.objectContaining({ data_revision: 3 }) }),
        }),
        '*',
      ));
      expect(screen.getByTestId('standalone-live-page-frame')).toBe(frame);
      expect(frame.getAttribute('srcdoc')).toBe(documentBefore);
    } finally {
      vi.useRealTimers();
    }
  });

  it('follows a publish at once, without its poll and without rebuilding the frame', async () => {
    const { useWebSocket } = await import('../../hooks/useWebSocket');
    render(<StandaloneLivePage pageId="page-1" />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const documentBefore = frame.getAttribute('srcdoc');
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');
    vi.mocked(pagesApi.data).mockResolvedValue(delta({ data_revision: 7 }));
    const handlers = vi.mocked(useWebSocket).mock.calls.map(([handler]) => handler);
    await act(async () => {
      handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'other', data_revision: 9 }));
    });
    expect(post).not.toHaveBeenCalledWith(expect.objectContaining({ type: 'kronn:page-data' }), '*');
    await act(async () => {
      handlers.forEach(handler => handler({ type: 'live_page_data_changed', page_id: 'page-1', data_revision: 7 }));
    });
    await waitFor(() => expect(post).toHaveBeenCalledWith(
      expect.objectContaining({
        type: 'kronn:page-data',
        data: expect.objectContaining({ page: expect.objectContaining({ data_revision: 7 }) }),
      }),
      '*',
    ));
    expect(frame.getAttribute('srcdoc')).toBe(documentBefore);
  });

  it('hands the URL view parameters to the Page with its data', async () => {
    render(<StandaloneLivePage pageId="page-1" params={{ tv: '1' }} />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');
    fireEvent.load(frame);
    await waitFor(() => expect(post).toHaveBeenCalledWith(
      expect.objectContaining({
        type: 'kronn:page-data',
        data: expect.objectContaining({ page: expect.objectContaining({ id: 'page-1', params: { tv: '1' } }) }),
      }),
      '*',
    ));
  });

  it('republishes new view parameters without a reload, and identical ones not at all', async () => {
    const view = render(<StandaloneLivePage pageId="page-1" params={{ scene: 'standup' }} />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    // The frame's own load posts the data once (`onLoad`). The DOM fires that
    // load on the next animation frame after the frame is inserted, which can
    // still be ahead when the frame is found: let that frame pass, so that only
    // what is posted from now on is counted as a republication.
    await act(async () => { await new Promise(resolve => requestAnimationFrame(resolve)); });
    const documentBefore = frame.getAttribute('srcdoc');
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');
    const dataPosts = () => post.mock.calls.filter(([message]) => (message as { type?: string }).type === 'kronn:page-data');

    // Same parameters, new object: nothing to send.
    view.rerender(<StandaloneLivePage pageId="page-1" params={{ scene: 'standup' }} />);
    await act(async () => {});
    expect(dataPosts()).toHaveLength(0);

    view.rerender(<StandaloneLivePage pageId="page-1" params={{ scene: 'retro' }} />);
    await waitFor(() => expect(dataPosts()).toHaveLength(1));
    expect(dataPosts()[0][0]).toMatchObject({ data: { page: { params: { scene: 'retro' } } } });
    expect(screen.getByTestId('standalone-live-page-frame')).toBe(frame);
    expect(frame.getAttribute('srcdoc')).toBe(documentBefore);
  });

  it('delivers a data: image published in a dataset without widening the frame CSP', async () => {
    const thumbnail = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';
    vi.mocked(pagesApi.get).mockResolvedValue({
      ...detail,
      revision: { ...detail.revision, html: '<img id="thumb" alt="">' },
      datasets: [{
        id: 'data-images', page_id: 'page-1', name: 'ticket_images', kind: 'snapshot',
        current: [{ input: { id: '10001' }, status: 'OK', response: thumbnail }],
        schema: null, max_points: 100, max_age_days: null, data_size_bytes: thumbnail.length,
        updated_at: '2026-08-26T10:00:00Z', points: [],
      }],
    });
    render(<StandaloneLivePage pageId="page-1" />);
    const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
    const post = vi.spyOn(frame.contentWindow!, 'postMessage');

    fireEvent.load(frame);

    await waitFor(() => expect(post).toHaveBeenCalledWith(
      expect.objectContaining({
        type: 'kronn:page-data',
        data: expect.objectContaining({
          datasets: { ticket_images: expect.objectContaining({
            current: [{ input: { id: '10001' }, status: 'OK', response: thumbnail }],
          }) },
        }),
      }),
      '*',
    ));
    const csp = frame.getAttribute('srcdoc')!.match(/Content-Security-Policy" content="([^"]+)"/)![1];
    expect(csp).toBe(LIVE_PAGE_CSP);
    expect(csp.split('; ').filter(directive => directive.startsWith('img-src'))).toEqual(['img-src data: blob:']);
    expect(csp).not.toMatch(/https?:|\*|'self'/);
  });

  it('renders the requested Page full-screen inside the opaque sandbox', async () => {
    const previousTitle = document.title;
    const view = render(<StandaloneLivePage pageId="page-1" />);

    expect(screen.getByRole('status')).toHaveTextContent('pages.standaloneLoading');
    const frame = await screen.findByTestId('standalone-live-page-frame');
    expect(pagesApi.get).toHaveBeenCalledWith('page-1');
    expect(frame).toHaveAttribute('title', 'Production health');
    expect(frame).toHaveAttribute('sandbox', 'allow-scripts');
    expect(frame).not.toHaveAttribute('allow-same-origin');
    expect(frame.getAttribute('srcdoc')).toContain("connect-src 'none'");
    expect(frame.getAttribute('srcdoc')).toContain('<h1>Production health</h1>');
    await waitFor(() => expect(linkRelay.connect).toHaveBeenCalledWith(
      (frame as HTMLIFrameElement).contentWindow,
    ));
    await waitFor(() => expect(document.title).toBe('Production health · Kronn'));

    view.unmount();
    expect(document.title).toBe(previousTitle);
  });

  it('opens the shared native action card from a sandbox intention', async () => {
    vi.mocked(pagesApi.actions).mockResolvedValue([pageAction()]);
    render(<StandaloneLivePage pageId="page-1" />);
    await screen.findByTestId('standalone-live-page-frame');

    act(() => linkRelay.onAction?.({ actionRef: 'refresh', bindings: {}, anchor }));

    expect(await screen.findByTestId(`live-page-action-${pageAction().id}`)).toBeInTheDocument();
  });

  it('fails closed and shows no card for an action removed from the current Page revision', async () => {
    render(<StandaloneLivePage pageId="page-1" />);
    await screen.findByTestId('standalone-live-page-frame');

    act(() => linkRelay.onAction?.({ actionRef: 'ghost', bindings: {}, anchor }));

    expect(await screen.findByText('disc.action.unavailablePageAction')).toBeInTheDocument();
    expect(screen.queryByTestId(/^live-page-action-/)).not.toBeInTheDocument();
  });

  it('reloads actions and clears a pending activation when the Page id changes', async () => {
    const otherDetail: LivePageDetail = {
      ...detail,
      id: 'page-2',
      title: 'Other report',
      revision: { ...detail.revision, page_id: 'page-2', html: '<main><h1>Other report</h1></main>' },
    };
    vi.mocked(pagesApi.get).mockImplementation(async id => id === 'page-2' ? otherDetail : detail);
    vi.mocked(pagesApi.actions).mockImplementation(async id => id === 'page-1' ? [pageAction()] : []);

    const view = render(<StandaloneLivePage pageId="page-1" />);
    await screen.findByTestId('standalone-live-page-frame');
    act(() => linkRelay.onAction?.({ actionRef: 'refresh', bindings: {}, anchor }));
    expect(await screen.findByTestId(`live-page-action-${pageAction().id}`)).toBeInTheDocument();

    view.rerender(<StandaloneLivePage pageId="page-2" />);
    await waitFor(() => expect(pagesApi.actions).toHaveBeenCalledWith('page-2'));
    await waitFor(() => expect(screen.getByTestId('standalone-live-page-frame')).toHaveAttribute('title', 'Other report'));
    expect(screen.queryByTestId(`live-page-action-${pageAction().id}`)).not.toBeInTheDocument();
  });

  it('opens a succeeded Quick Prompt action result discussion in a new same-origin tab', async () => {
    const openSpy = vi.spyOn(window, 'open').mockReturnValue(null);
    const succeeded = pageAction({
      id: 'page-action:page-1:qp', action_ref: 'qp', kind: 'quick_prompt',
      state: 'succeeded', shared_run_id: 'run-1', result_discussion_id: 'disc-99',
    });
    vi.mocked(pagesApi.actions).mockResolvedValue([succeeded]);
    render(<StandaloneLivePage pageId="page-1" />);
    await screen.findByTestId('standalone-live-page-frame');
    act(() => linkRelay.onAction?.({ actionRef: 'qp', bindings: {}, anchor }));
    await screen.findByTestId(`live-page-action-${succeeded.id}`);

    fireEvent.click(await screen.findByRole('button', { name: /disc\.action\.openDiscussion/ }));

    // The address carries the discussion, so the new tab needs nothing cloned
    // from this one — which is what lets it open with no opener at all.
    const openedUrl = openSpy.mock.calls[0][0] as string;
    expect(openedUrl.startsWith(`${window.location.origin}/discussions/`)).toBe(true);
    expect(openSpy.mock.calls[0].slice(1)).toEqual(['_blank', 'noopener,noreferrer']);
  });

  it('renders a failed workflow action in its terminal state without a discussion link', async () => {
    const failed = pageAction({
      kind: 'workflow', state: 'failed', shared_run_id: 'run-2',
      diagnostic: 'The linked workflow failed.', result_discussion_id: null,
    });
    vi.mocked(pagesApi.actions).mockResolvedValue([failed]);
    render(<StandaloneLivePage pageId="page-1" />);
    await screen.findByTestId('standalone-live-page-frame');
    act(() => linkRelay.onAction?.({ actionRef: 'refresh', bindings: {}, anchor }));

    expect(await screen.findByText('The linked workflow failed.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /disc\.action\.openDiscussion/ })).not.toBeInTheDocument();
  });
});
