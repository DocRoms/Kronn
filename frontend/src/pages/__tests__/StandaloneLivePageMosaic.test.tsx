import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePageAction, LivePageDetail } from '../../types/generated';

const details: Record<string, LivePageDetail> = Object.fromEntries(['page-1', 'page-2', 'page-3'].map((id, index) => [id, {
  id,
  project_id: null,
  title: `Report ${index + 1}`,
  slug: `report-${index + 1}`,
  current_revision_id: `rev-${index + 1}`,
  data_revision: 1,
  created_at: '2026-08-29T10:00:00Z',
  updated_at: '2026-08-29T10:00:00Z',
  last_published_at: null,
  pinned: false,
  archived: false,
  revision: {
    id: `rev-${index + 1}`,
    page_id: id,
    revision: 1,
    html: `<main><h1>Report ${index + 1}</h1></main>`,
    created_by_agent: null,
    created_at: '2026-08-29T10:00:00Z',
  },
  datasets: [],
}]));
const relays = vi.hoisted(() => [] as {
  connect: ReturnType<typeof vi.fn>;
  dispose: ReturnType<typeof vi.fn>;
  onAction: ((intent: {
    actionRef: string;
    bindings: Record<string, string>;
    anchor: { left: number; top: number; width: number; height: number };
  }) => void) | null;
}[]);

vi.mock('../../lib/api', () => ({
  pages: { get: vi.fn(), actions: vi.fn(), getAction: vi.fn(), cancelAction: vi.fn(), launchAction: vi.fn() },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, ...args: (string | number)[]) => args.length ? `${key}:${args.join(',')}` : key }),
}));
vi.mock('../../lib/live-page-sandbox', async importOriginal => ({
  ...await importOriginal<Record<string, unknown>>(),
  createLivePageOpenLinkRelay: vi.fn((_channel, _open, onAction) => {
    const relay = { connect: vi.fn(), dispose: vi.fn(), onAction };
    relays.push(relay);
    return relay;
  }),
}));
vi.mock('../../components/RunStatusCard', () => ({
  RunStatusCard: ({ runId }: { runId?: string }) => <div data-testid="run-card">{runId}</div>,
}));

import { pages as pagesApi } from '../../lib/api';
import { StandaloneLivePageMosaic } from '../StandaloneLivePageMosaic';

function pageAction(pageId: string, actionRef: string, overrides: Partial<LivePageAction> = {}): LivePageAction {
  return {
    id: `page-action:${pageId}:${actionRef}`, live_page_id: pageId, live_page_revision_id: `rev-${pageId}`,
    action_ref: actionRef, kind: 'workflow', target_id: 'wf-1', target_name: `Refresh ${pageId}`,
    project_id: null, state: 'proposed', values: [], shared_run_id: null,
    result_discussion_id: null, deep_link: null, diagnostic: null, launched_at: null,
    finished_at: null, created_at: '2026-08-29T10:00:00Z', updated_at: '2026-08-29T10:00:00Z',
    stale_source: false,
    ...overrides,
  };
}

const anchor = { left: 12, top: 16, width: 90, height: 24 };

beforeEach(() => {
  relays.length = 0;
  vi.mocked(pagesApi.get).mockImplementation(async pageId => details[pageId]);
  vi.mocked(pagesApi.actions).mockResolvedValue([]);
});

afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

describe('StandaloneLivePageMosaic', () => {
  it('loads every selected Page in its own opaque sandbox and applies the preset', async () => {
    const previousTitle = document.title;
    const view = render(
      <StandaloneLivePageMosaic
        pageIds={['page-1', 'page-2', 'page-3']}
        layout="three-left"
      />,
    );

    expect(screen.getByTestId('standalone-live-page-mosaic')).toHaveAttribute('data-layout', 'three-left');
    const frames = await screen.findAllByTestId('standalone-live-page-mosaic-frame');
    expect(pagesApi.get).toHaveBeenCalledTimes(3);
    expect(frames.map(frame => frame.getAttribute('title'))).toEqual(['Report 1', 'Report 2', 'Report 3']);
    frames.forEach((frame, index) => {
      expect(frame).toHaveAttribute('sandbox', 'allow-scripts');
      expect(frame).not.toHaveAttribute('allow-same-origin');
      expect(frame.getAttribute('srcdoc')).toContain("connect-src 'none'");
      expect(frame.getAttribute('srcdoc')).toContain(`Report ${index + 1}`);
    });
    await waitFor(() => expect(relays.every(relay => relay.connect.mock.calls.length > 0)).toBe(true));
    expect(document.title).toBe('pages.mosaic.documentTitle · Kronn');

    view.unmount();
    expect(document.title).toBe(previousTitle);
    expect(relays.every(relay => relay.dispose.mock.calls.length === 1)).toBe(true);
  });

  it('keeps each tile\'s actions isolated: a valid click in one tile never leaks into a sibling', async () => {
    vi.mocked(pagesApi.actions).mockImplementation(async pageId => {
      if (pageId === 'page-1') return [pageAction('page-1', 'ticket')];
      if (pageId === 'page-2') return [pageAction('page-2', 'refresh')];
      return [];
    });
    render(<StandaloneLivePageMosaic pageIds={['page-1', 'page-2', 'page-3']} layout="three-left" />);
    await screen.findAllByTestId('standalone-live-page-mosaic-frame');
    await waitFor(() => expect(relays).toHaveLength(3));
    const tiles = document.querySelectorAll('.standalone-live-page-mosaic-tile');
    expect(tiles).toHaveLength(3);

    // Valid click in tile 1 (page-1) only shows its own card.
    act(() => relays[0].onAction?.({ actionRef: 'ticket', bindings: {}, anchor }));
    expect(await within(tiles[0] as HTMLElement).findByTestId('live-page-action-page-action:page-1:ticket'))
      .toBeInTheDocument();
    expect(within(tiles[1] as HTMLElement).queryByTestId(/^live-page-action-/)).not.toBeInTheDocument();
    expect(within(tiles[2] as HTMLElement).queryByTestId(/^live-page-action-/)).not.toBeInTheDocument();

    // `refresh` only exists on page-2: clicking it from tile 1 (page-1) fails
    // closed even though the ref is valid for a sibling tile.
    act(() => relays[0].onAction?.({ actionRef: 'refresh', bindings: {}, anchor }));
    expect(await within(tiles[0] as HTMLElement).findByText('disc.action.unavailablePageAction')).toBeInTheDocument();

    // Tile 2 (page-2) can still launch its own `refresh` action independently.
    act(() => relays[1].onAction?.({ actionRef: 'refresh', bindings: {}, anchor }));
    expect(await within(tiles[1] as HTMLElement).findByTestId('live-page-action-page-action:page-2:refresh')).toBeInTheDocument();
    expect(within(tiles[2] as HTMLElement).queryByTestId(/^live-page-action-/)).not.toBeInTheDocument();
    expect(within(tiles[2] as HTMLElement).queryByText('disc.action.unavailablePageAction')).not.toBeInTheDocument();
  });

  it('opens a succeeded Quick Prompt tile action result discussion in a new same-origin tab', async () => {
    const openSpy = vi.spyOn(window, 'open').mockReturnValue(null);
    const succeeded = pageAction('page-1', 'qp', {
      kind: 'quick_prompt', state: 'succeeded', shared_run_id: 'run-1', result_discussion_id: 'disc-77',
    });
    vi.mocked(pagesApi.actions).mockImplementation(async pageId => pageId === 'page-1' ? [succeeded] : []);
    render(<StandaloneLivePageMosaic pageIds={['page-1', 'page-2']} layout="two-columns" />);
    await screen.findAllByTestId('standalone-live-page-mosaic-frame');
    await waitFor(() => expect(relays).toHaveLength(2));

    act(() => relays[0].onAction?.({ actionRef: 'qp', bindings: {}, anchor }));
    await screen.findByTestId(`live-page-action-${succeeded.id}`);
    fireEvent.click(screen.getByRole('button', { name: /disc\.action\.openDiscussion/ }));

    expect(sessionStorage.getItem('kronn:navigation:page')).toBe('discussions');
    expect(sessionStorage.getItem('kronn:navigation:discussion')).toBe('disc-77');
    expect(openSpy).toHaveBeenCalledWith(`${window.location.origin}${window.location.pathname}`, '_blank');
  });

  it('renders a failed non-QP tile action in its terminal state without a discussion link', async () => {
    const failed = pageAction('page-2', 'sync', {
      kind: 'quick_exec', state: 'failed', shared_run_id: 'run-2',
      diagnostic: 'The Quick Exec failed.', result_discussion_id: null,
    });
    vi.mocked(pagesApi.actions).mockImplementation(async pageId => pageId === 'page-2' ? [failed] : []);
    render(<StandaloneLivePageMosaic pageIds={['page-1', 'page-2']} layout="two-columns" />);
    await screen.findAllByTestId('standalone-live-page-mosaic-frame');
    await waitFor(() => expect(relays).toHaveLength(2));

    act(() => relays[1].onAction?.({ actionRef: 'sync', bindings: {}, anchor }));

    expect(await screen.findByText('The Quick Exec failed.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /disc\.action\.openDiscussion/ })).not.toBeInTheDocument();
  });
});
