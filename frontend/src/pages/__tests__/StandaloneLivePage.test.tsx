import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePageAction, LivePageDetail } from '../../types/generated';

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
  last_published_at: '2026-08-26T10:00:00Z', pinned: false, archived: false,
  revision: {
    id: 'rev-1', page_id: 'page-1', revision: 1,
    html: '<main><h1>Production health</h1></main>',
    created_by_agent: 'Ollama', created_at: '2026-08-26T10:00:00Z',
  },
  datasets: [],
};

vi.mock('../../lib/api', () => ({
  pages: { get: vi.fn(), actions: vi.fn(), actionLaunches: vi.fn(() => Promise.resolve([])), getAction: vi.fn(), cancelAction: vi.fn(), launchAction: vi.fn() },
  runsApi: {
    outcome: vi.fn(() => Promise.resolve({ discussion_count: 0, discussions: [] })),
    discussionOutcome: vi.fn((id: string) => Promise.resolve({ discussion_count: 1, discussions: [{ id, title: 'Result', agent: 'ClaudeCode', agent_status: 'answered', answer_excerpt: null, answer_truncated: false, answered_at: null, diagnostic: null, updated_at: '2026-09-18T10:00:00Z' }] })),
  },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, ...args: (string | number)[]) => args.length ? `${key}:${args.join(',')}` : key }),
}));
vi.mock('../../lib/live-page-sandbox', async importOriginal => ({
  ...await importOriginal<Record<string, unknown>>(),
  createLivePageOpenLinkRelay: vi.fn((_channel, _open, onAction) => {
    linkRelay.onAction = onAction;
    return linkRelay;
  }),
}));
vi.mock('../../components/RunStatusCard', () => ({
  RunStatusCard: ({ runId }: { runId?: string }) => <div data-testid="run-card">{runId}</div>,
}));

import { pages as pagesApi } from '../../lib/api';
import { LIVE_PAGE_CSP } from '../../lib/live-page-sandbox';
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

beforeEach(() => {
  linkRelay.connect.mockClear();
  linkRelay.dispose.mockClear();
  linkRelay.onAction = null;
  vi.mocked(pagesApi.get).mockResolvedValue(detail);
  vi.mocked(pagesApi.actions).mockResolvedValue([]);
});

afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

describe('StandaloneLivePage', () => {
  it('pushes newly published data into the open frame without rebuilding it', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      render(<StandaloneLivePage pageId="page-1" />);
      const frame = await screen.findByTestId('standalone-live-page-frame') as HTMLIFrameElement;
      const documentBefore = frame.getAttribute('srcdoc');
      const post = vi.spyOn(frame.contentWindow!, 'postMessage');
      vi.mocked(pagesApi.get).mockResolvedValue({ ...detail, data_revision: 3 });

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

    expect(sessionStorage.getItem('kronn:navigation:page')).toBe('discussions');
    expect(sessionStorage.getItem('kronn:navigation:discussion')).toBe('disc-99');
    // The address carries the discussion, so the new tab needs nothing cloned
    // from this one — which is what lets it open with no opener at all.
    const openedUrl = openSpy.mock.calls[0][0] as string;
    expect(openedUrl.startsWith(`${window.location.origin}${window.location.pathname}#discussion-`)).toBe(true);
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
