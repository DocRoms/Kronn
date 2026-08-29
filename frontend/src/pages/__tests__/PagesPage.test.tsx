import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePage, LivePageDetail, LivePagePublication } from '../../types/generated';

const page: LivePage = {
  id: 'page-1', project_id: null, title: 'Adobe Signals', slug: 'adobe-signals',
  current_revision_id: 'rev-2', data_revision: 3,
  created_at: '2026-08-13T10:00:00Z', updated_at: '2026-08-13T10:00:00Z',
  last_published_at: '2026-08-13T10:00:00Z',
  pinned: false, archived: false,
};
const detail: LivePageDetail = {
  ...page,
  revision: { id: 'rev-2', page_id: page.id, revision: 2, html: '<main>\n<h1>Adobe</h1>\n</main>', created_by_agent: 'Codex', created_at: page.created_at },
  datasets: [{
    id: 'dataset-1', page_id: page.id, name: 'summary', kind: 'snapshot',
    current: { total: 1240 }, schema: null, max_points: 50_000, max_age_days: null,
    updated_at: page.updated_at, points: [], data_size_bytes: 1536,
  }],
};
const publications: LivePagePublication[] = [3, 2, 1].map(dataRevision => ({
  id: `publication-${dataRevision}`,
  page_id: page.id,
  data_revision: dataRevision,
  workflow_id: 'wf-1',
  workflow_name: 'Adobe cron',
  workflow_run_id: `run-${dataRevision}`,
  datasets_updated: ['summary'],
  content_changed: dataRevision !== 2,
  changed_datasets: dataRevision !== 2 ? ['summary'] : [],
  unchanged_datasets: dataRevision === 2 ? ['summary'] : [],
  points_added: 0,
  points_removed: 0,
  published_at: `2026-08-13T0${dataRevision}:00:00Z`,
}));
const linkRelay = vi.hoisted(() => ({ connect: vi.fn(), dispose: vi.fn() }));

vi.mock('../../lib/api', () => ({
  docs: { generatePdf: vi.fn(), generateDocx: vi.fn(), generateCsv: vi.fn() },
  pages: {
    list: vi.fn(), get: vi.fn(), revisions: vi.fn(), workflows: vi.fn(), publications: vi.fn(), discussions: vi.fn(),
    update: vi.fn(), delete: vi.fn(), updateHtml: vi.fn(),
  },
  workflows: { triggerStream: vi.fn() },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    locale: 'fr',
    t: (key: string, ...args: (string | number)[]) => args.length > 0 ? `${key}:${args.join(',')}` : key,
  }),
}));
vi.mock('../../lib/live-page-sandbox', async importOriginal => ({
  ...await importOriginal<Record<string, unknown>>(),
  createLivePageOpenLinkRelay: vi.fn(() => linkRelay),
  requestRenderedPageHtml: vi.fn(),
}));

import { docs as docsApi, pages as pagesApi, workflows as workflowsApi } from '../../lib/api';
import { requestRenderedPageHtml } from '../../lib/live-page-sandbox';
import { PagesPage } from '../PagesPage';

beforeEach(() => {
  localStorage.removeItem('kronn:pageNavigation');
  localStorage.removeItem('kronn:pageCollapsedSections');
  linkRelay.connect.mockClear();
  linkRelay.dispose.mockClear();
  vi.mocked(pagesApi.list).mockResolvedValue([page]);
  vi.mocked(pagesApi.get).mockResolvedValue(detail);
  vi.mocked(pagesApi.revisions).mockResolvedValue([
    detail.revision,
    { id: 'rev-1', page_id: page.id, revision: 1, html: '<h1>Adobe legacy</h1>', created_by_agent: 'Claude', created_at: '2026-08-12T10:00:00Z' },
  ]);
  vi.mocked(pagesApi.workflows).mockResolvedValue([{ id: 'wf-1', name: 'Adobe cron', enabled: true, step_names: ['publish'] }]);
  vi.mocked(pagesApi.publications).mockResolvedValue(publications);
  vi.mocked(pagesApi.discussions).mockResolvedValue([]);
  vi.mocked(pagesApi.update).mockImplementation(async (_id, request) => ({
    ...detail,
    title: request.title ?? detail.title,
    pinned: request.pinned ?? detail.pinned,
    archived: request.archived ?? detail.archived,
  }));
  vi.mocked(pagesApi.delete).mockResolvedValue(undefined);
  vi.mocked(pagesApi.updateHtml).mockResolvedValue({ ...detail.revision, id: 'rev-3', revision: 3 });
  vi.mocked(docsApi.generatePdf).mockResolvedValue({ path: '/tmp/adobe-signals.pdf', download_url: '/api/docs/file/page-1/adobe-signals.pdf', size_bytes: 123 });
  vi.mocked(docsApi.generateDocx).mockResolvedValue({ path: '/tmp/adobe-signals.docx', download_url: '/api/docs/file/page-1/adobe-signals.docx', size_bytes: 456 });
  vi.mocked(docsApi.generateCsv).mockResolvedValue({ path: '/tmp/adobe-signals-summary.csv', download_url: '/api/docs/file/page-1/adobe-signals-summary.csv', size_bytes: 42 });
  vi.mocked(requestRenderedPageHtml).mockResolvedValue({
    html: '<html><body>Rendered report</body></html>',
    pageImages: ['data:image/png;base64,cGFnZQ=='],
  });
  vi.mocked(workflowsApi.triggerStream).mockImplementation(async (_id, _onStart, _onStep, onDone) => {
    onDone({ status: 'Completed' });
  });
});

afterEach(() => {
  localStorage.removeItem('kronn:pageNavigation');
  localStorage.removeItem('kronn:pageCollapsedSections');
});

describe('PagesPage', () => {
  it('keeps the historical responsive sidebar classes on the shared shell', async () => {
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    expect(screen.getByRole('complementary', { name: 'pages.title' }))
      .toHaveClass('disc-sidebar', 'live-pages-list');
  });

  it('renders authored HTML in a script-only opaque sandbox', async () => {
    const onNavigateWorkflow = vi.fn();
    render(<PagesPage onNavigateWorkflow={onNavigateWorkflow} />);
    const frame = await screen.findByTestId('live-page-frame');
    await waitFor(() => expect(pagesApi.get).toHaveBeenCalledWith(page.id));
    expect(frame).toHaveAttribute('sandbox', 'allow-scripts');
    expect(frame).not.toHaveAttribute('allow-same-origin');
    expect(frame.getAttribute('srcdoc')).toContain("connect-src 'none'");
    expect(frame.getAttribute('srcdoc')).toContain('<h1>Adobe</h1>');
    expect(linkRelay.connect).toHaveBeenCalledWith((frame as HTMLIFrameElement).contentWindow);
    const standaloneLink = screen.getByRole('link', { name: 'pages.openInNewTab:Adobe Signals' });
    expect(standaloneLink).toHaveAttribute('href', `${window.location.origin}${window.location.pathname}#page/page-1`);
    expect(standaloneLink).toHaveAttribute('target', '_blank');
    expect(standaloneLink).toHaveAttribute('rel', 'noopener noreferrer');
    expect(screen.getByText('data r3 · HTML r2')).toBeInTheDocument();
    expect(screen.getAllByText('1.5 KB')).toHaveLength(2);
    expect(screen.getByTitle('pages.datasetSizeTitle:summary,1.5 KB')).toBeInTheDocument();
    const refreshMenu = screen.getByTestId('live-page-refresh-menu');
    const refreshDetails = screen.getByTestId('live-page-recent-refreshes');
    expect(refreshMenu).not.toHaveAttribute('open');
    expect(refreshDetails).not.toBeVisible();
    fireEvent.click(screen.getByLabelText('pages.autoRefresh'));
    expect(refreshMenu).toHaveAttribute('open');
    expect(refreshDetails).toBeVisible();
    expect(screen.getByRole('button', { name: 'Adobe cron' })).toBeInTheDocument();
    expect(screen.getByText('#page-1')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Adobe cron' }));
    expect(onNavigateWorkflow).toHaveBeenCalledWith('wf-1');
  });

  it('shows the three latest successful workflow refreshes and opens their workflow', async () => {
    const onNavigateWorkflow = vi.fn();
    render(<PagesPage onNavigateWorkflow={onNavigateWorkflow} />);

    await screen.findByTestId('live-page-recent-refreshes');
    expect(pagesApi.publications).toHaveBeenCalledWith(page.id);
    fireEvent.click(screen.getByLabelText('pages.autoRefresh'));
    expect(screen.getAllByLabelText('pages.openRefreshRun:Adobe cron')).toHaveLength(3);
    expect(screen.getByText('data r3')).toBeInTheDocument();
    expect(screen.getByText('data r2')).toBeInTheDocument();
    expect(screen.getByText('data r1')).toBeInTheDocument();
    expect(screen.getAllByText('pages.refreshChanged')).toHaveLength(2);
    expect(screen.getByText('pages.refreshUnchanged')).toBeInTheDocument();
    expect(screen.getByText('pages.unchangedDataset:summary')).toBeInTheDocument();

    fireEvent.click(screen.getAllByLabelText('pages.openRefreshRun:Adobe cron')[0]);
    expect(onNavigateWorkflow).toHaveBeenCalledWith('wf-1', 'run-3');
  });

  it('creates an immutable HTML revision from the Page editor', async () => {
    const { container } = render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    fireEvent.click(screen.getByText('pages.editHtml'));
    const editor = screen.getByLabelText('pages.htmlTitle');
    expect(container.querySelectorAll('.live-pages-code-gutter > span')).toHaveLength(3);
    expect(container.querySelector('.live-pages-code-highlight .hljs-tag')).toBeInTheDocument();
    fireEvent.change(editor, { target: { value: '<h1>Nouvelle révision</h1>' } });
    const save = screen.getByText('pages.saveRevision');
    fireEvent.click(save);
    fireEvent.click(save);
    await waitFor(() => expect(pagesApi.updateHtml).toHaveBeenCalledWith(page.id, {
      html: '<h1>Nouvelle révision</h1>',
      created_by_agent: null,
    }));
    expect(pagesApi.updateHtml).toHaveBeenCalledTimes(1);
  });

  it('renames a Page, normalizes the title and restores it after a reload', async () => {
    const view = render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    fireEvent.click(screen.getByLabelText('pages.renameTitle:Adobe Signals'));
    const input = screen.getByLabelText('pages.titleInput');
    fireEvent.change(input, { target: { value: '  Production health  ' } });
    fireEvent.click(screen.getByLabelText('pages.saveTitle'));

    await waitFor(() => expect(pagesApi.update).toHaveBeenCalledWith(page.id, {
      title: 'Production health',
    }));
    expect(await screen.findByRole('heading', { name: 'Production health' })).toBeInTheDocument();
    expect(screen.getByLabelText('pages.open:Production health')).toBeInTheDocument();

    view.unmount();
    const persisted = { ...detail, title: 'Production health' };
    vi.mocked(pagesApi.list).mockResolvedValueOnce([persisted]);
    vi.mocked(pagesApi.get).mockResolvedValueOnce(persisted);
    render(<PagesPage />);
    expect(await screen.findByRole('heading', { name: 'Production health' })).toBeInTheDocument();
  });

  it('refuses empty and overlong Page titles before calling the backend', async () => {
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    fireEvent.click(screen.getByLabelText('pages.renameTitle:Adobe Signals'));
    const input = screen.getByLabelText('pages.titleInput');
    const updateCallsBeforeValidation = vi.mocked(pagesApi.update).mock.calls.length;

    fireEvent.change(input, { target: { value: '   ' } });
    fireEvent.click(screen.getByLabelText('pages.saveTitle'));
    expect(screen.getByRole('alert')).toHaveTextContent('pages.titleRequired');
    expect(pagesApi.update).toHaveBeenCalledTimes(updateCallsBeforeValidation);

    fireEvent.change(input, { target: { value: 'x'.repeat(201) } });
    fireEvent.click(screen.getByLabelText('pages.saveTitle'));
    expect(screen.getByRole('alert')).toHaveTextContent('pages.titleTooLong');
    expect(pagesApi.update).toHaveBeenCalledTimes(updateCallsBeforeValidation);
  });

  it('rolls the visible draft back to the persisted title when rename fails', async () => {
    vi.mocked(pagesApi.update).mockRejectedValueOnce(new Error('Backend unavailable'));
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    fireEvent.click(screen.getByLabelText('pages.renameTitle:Adobe Signals'));
    const input = screen.getByLabelText('pages.titleInput');
    fireEvent.change(input, { target: { value: 'Unsaved title' } });
    fireEvent.click(screen.getByLabelText('pages.saveTitle'));

    expect(await screen.findByRole('alert')).toHaveTextContent('Backend unavailable');
    expect(input).toHaveValue('Adobe Signals');
    expect(screen.getByLabelText('pages.open:Adobe Signals')).toBeInTheDocument();
  });

  it('exports the data-materialized Page DOM as PDF from the header', async () => {
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    const renderedHtml = '<html><head><style>.metric{color:red}</style></head><body><strong class="metric">1240</strong></body></html>';
    vi.mocked(requestRenderedPageHtml).mockResolvedValueOnce({
      html: renderedHtml,
      pageImages: ['data:image/png;base64,cGFnZQ=='],
    });
    fireEvent.click(screen.getByText('pages.export'));
    fireEvent.click(screen.getByRole('button', { name: 'PDF' }));
    await waitFor(() => expect(docsApi.generatePdf).toHaveBeenCalledWith({
      discussion_id: page.id,
      html: renderedHtml,
      page_images: ['data:image/png;base64,cGFnZQ=='],
      filename: 'adobe-signals.pdf',
      page_size: 'A4',
    }));
    expect(await screen.findByText('adobe-signals.pdf')).toBeInTheDocument();
  });

  it('uses the same materialized Page DOM for DOCX export', async () => {
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    fireEvent.click(screen.getByText('pages.export'));
    const renderedHtml = '<html><head><style>body{background:#123}</style></head><body>Rendered report</body></html>';
    vi.mocked(requestRenderedPageHtml).mockResolvedValueOnce({
      html: renderedHtml,
      pageImages: ['data:image/png;base64,cGFnZQ=='],
    });
    fireEvent.click(screen.getByRole('button', { name: 'DOCX' }));
    await waitFor(() => expect(docsApi.generateDocx).toHaveBeenCalledWith({
      discussion_id: page.id,
      html: renderedHtml,
      page_images: ['data:image/png;base64,cGFnZQ=='],
      filename: 'adobe-signals.docx',
    }));
  });

  it('opens retained dataset values and exports a flattened CSV', async () => {
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined);
    vi.mocked(pagesApi.get).mockResolvedValue({
      ...detail,
      datasets: [
        ...detail.datasets,
        {
          id: 'dataset-2', page_id: page.id, name: 'errors', kind: 'collection',
          current: [{ code: 500 }], schema: null, max_points: 50_000, max_age_days: null,
          updated_at: page.updated_at, points: [], data_size_bytes: 20,
        },
      ],
    });
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    fireEvent.click(screen.getByRole('button', { name: 'pages.datasetStorage' }));
    expect(screen.getByRole('combobox', { name: 'pages.datasetView:summary' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'summary · 1.5 KB' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'errors · 20 B' })).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: 'total' })).toBeInTheDocument();
    expect(screen.getByRole('cell', { name: '1240' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'CSV' }));
    await waitFor(() => expect(docsApi.generateCsv).toHaveBeenCalledWith({
      discussion_id: page.id,
      rows: [['total'], [1240]],
      delimiter: ';',
      filename: 'adobe-signals-summary.csv',
    }));
    expect(click).toHaveBeenCalled();
    click.mockRestore();
  });

  it('compares an older HTML revision and can restore it into the editor', async () => {
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    fireEvent.click(screen.getByText('pages.editHtml'));

    fireEvent.change(screen.getByLabelText('pages.compareRevision'), { target: { value: 'rev-1' } });
    expect(screen.getByTestId('live-page-html-diff')).toHaveTextContent('Adobe legacy');
    fireEvent.click(screen.getByText('pages.restoreRevision:1'));
    expect(screen.getByLabelText('pages.htmlTitle')).toHaveValue('<h1>Adobe legacy</h1>');
  });

  it('shows distinct running, successful and failed Sync states', async () => {
    let finishRun: ((result: { status: string }) => void) | undefined;
    let failRun: ((message: string) => void) | undefined;
    vi.mocked(workflowsApi.triggerStream).mockImplementation(async (
      _id, _onStart, _onStep, onDone, onError,
    ) => {
      finishRun = onDone;
      failRun = onError;
    });
    render(<PagesPage />);
    fireEvent.click(await screen.findByLabelText('pages.autoRefresh'));
    const sync = screen.getByLabelText('pages.runLinkedWorkflow:Adobe cron');
    expect(sync).toHaveAttribute('data-state', 'idle');
    fireEvent.click(sync);

    await waitFor(() => expect(workflowsApi.triggerStream).toHaveBeenCalledWith(
      'wf-1',
      expect.any(Function),
      expect.any(Function),
      expect.any(Function),
      expect.any(Function),
    ));
    expect(sync).toHaveAttribute('data-state', 'running');
    expect(sync.querySelector('.lucide-loader-circle')).toBeInTheDocument();

    await act(async () => { finishRun?.({ status: 'Completed' }); });
    expect(sync).toHaveAttribute('data-state', 'success');
    expect(screen.getByTestId('live-page-sync-success-icon')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('pages.workflowRunSuccess');

    fireEvent.click(sync);
    await act(async () => { failRun?.('Network unavailable'); });
    expect(sync).toHaveAttribute('data-state', 'error');
    expect(screen.getByTestId('live-page-sync-error-icon')).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('Network unavailable');
  });

  it('searches, favorites and archives Pages with the library interactions', async () => {
    const other = { ...page, id: 'page-2', title: 'Jira Delivery', slug: 'jira-delivery' };
    vi.mocked(pagesApi.list).mockResolvedValue([page, other]);
    const { container } = render(<PagesPage />);
    await screen.findByTestId('live-page-frame');

    fireEvent.change(screen.getByLabelText('pages.search'), { target: { value: 'Jira' } });
    expect(screen.getByText('Jira Delivery')).toBeInTheDocument();
    expect(screen.queryByLabelText('pages.open:Adobe Signals')).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('pages.search'), { target: { value: '' } });

    const favoriteButtons = screen.getAllByLabelText(/^pages\.favorite/);
    fireEvent.click(favoriteButtons[0]);
    await waitFor(() => expect(pagesApi.update).toHaveBeenCalledWith(page.id, { pinned: true }));
    await waitFor(() => {
      // The Page appears in both Favorites and the complete list, but each row
      // must render one star only: the stateful favorite action itself.
      const rows = Array.from(container.querySelectorAll('.live-page-row'));
      expect(rows).toHaveLength(3);
      expect(rows.every(row => row.querySelectorAll('.lucide-star').length === 1)).toBe(true);
    });

    fireEvent.click(screen.getByLabelText('pages.bulk.start'));
    fireEvent.click(screen.getByLabelText('pages.open:Adobe Signals'));
    vi.stubGlobal('confirm', vi.fn(() => true));
    fireEvent.click(screen.getByTitle('pages.archive'));
    await waitFor(() => expect(pagesApi.update).toHaveBeenCalledWith(page.id, { archived: true }));
  });

  it('restores the selected Page and collapsed sidebar sections', async () => {
    const other = { ...page, id: 'page-2', title: 'Jira Delivery', slug: 'jira-delivery' };
    const otherDetail = { ...detail, ...other };
    localStorage.setItem('kronn:pageNavigation', JSON.stringify({ resourceId: other.id }));
    localStorage.setItem('kronn:pageCollapsedSections', JSON.stringify(['pages', 'archives']));
    vi.mocked(pagesApi.list).mockResolvedValue([page, other]);
    vi.mocked(pagesApi.get).mockImplementation(async id => id === other.id ? otherDetail : detail);

    render(<PagesPage />);

    await waitFor(() => expect(pagesApi.get).toHaveBeenCalledWith(other.id));
    expect(screen.getByRole('heading', { name: other.title })).toBeInTheDocument();
    expect(screen.getByText('pages.filter.active').closest('button')).toHaveAttribute('aria-expanded', 'false');
    expect(JSON.parse(localStorage.getItem('kronn:pageNavigation') ?? '{}')).toEqual({ resourceId: other.id });
  });

  it('temporarily expands matching sections without losing their saved state', async () => {
    localStorage.setItem('kronn:pageCollapsedSections', JSON.stringify(['pages', 'archives']));
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');

    const activeSection = screen.getByText('pages.filter.active').closest('button');
    expect(activeSection).toHaveAttribute('aria-expanded', 'false');
    fireEvent.change(screen.getByLabelText('pages.search'), { target: { value: 'Adobe' } });
    expect(activeSection).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByLabelText('pages.open:Adobe Signals')).toBeInTheDocument();
    expect(JSON.parse(localStorage.getItem('kronn:pageCollapsedSections') ?? '[]')).toEqual(['pages', 'archives']);
    fireEvent.change(screen.getByLabelText('pages.search'), { target: { value: '' } });
    expect(activeSection).toHaveAttribute('aria-expanded', 'false');
  });

  it('falls back safely when the saved Page no longer exists', async () => {
    localStorage.setItem('kronn:pageNavigation', JSON.stringify({ resourceId: 'deleted-page' }));
    render(<PagesPage />);

    await waitFor(() => expect(pagesApi.get).toHaveBeenCalledWith(page.id));
    await waitFor(() => expect(JSON.parse(localStorage.getItem('kronn:pageNavigation') ?? '{}'))
      .toEqual({ resourceId: page.id }));
  });
});

// ─── Popover dismiss (KT-463) ──────────────────────────────────────────────
// The Export and Sync popovers are native <details>/<summary>: nothing about
// them closes on an outside click or Escape by default.

describe('PagesPage — popover dismiss (KT-463)', () => {
  it('clicking outside the Export popover closes it', async () => {
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    fireEvent.click(screen.getByText('pages.export'));
    expect(screen.getByTestId('live-page-export-menu')).toHaveAttribute('open');

    fireEvent.mouseDown(document.body);
    expect(screen.getByTestId('live-page-export-menu')).not.toHaveAttribute('open');
  });

  it('clicking outside the Sync popover closes it', async () => {
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    fireEvent.click(screen.getByLabelText('pages.autoRefresh'));
    expect(screen.getByTestId('live-page-refresh-menu')).toHaveAttribute('open');

    fireEvent.mouseDown(document.body);
    expect(screen.getByTestId('live-page-refresh-menu')).not.toHaveAttribute('open');
  });

  it('a click on a button inside the Export popover does not close it prematurely, and still exports', async () => {
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    fireEvent.click(screen.getByText('pages.export'));

    fireEvent.mouseDown(screen.getByRole('button', { name: 'PDF' }));
    expect(screen.getByTestId('live-page-export-menu')).toHaveAttribute('open');
    fireEvent.click(screen.getByRole('button', { name: 'PDF' }));
    await waitFor(() => expect(docsApi.generatePdf).toHaveBeenCalled());
  });

  it('a click on the linked-workflow button inside the Sync popover does not close it prematurely', async () => {
    const onNavigateWorkflow = vi.fn();
    render(<PagesPage onNavigateWorkflow={onNavigateWorkflow} />);
    await screen.findByTestId('live-page-frame');
    fireEvent.click(screen.getByLabelText('pages.autoRefresh'));

    fireEvent.mouseDown(screen.getByRole('button', { name: 'Adobe cron' }));
    expect(screen.getByTestId('live-page-refresh-menu')).toHaveAttribute('open');
    fireEvent.click(screen.getByRole('button', { name: 'Adobe cron' }));
    expect(onNavigateWorkflow).toHaveBeenCalledWith('wf-1');
  });

  it('clicking inside the open Export popover leaves it open even while the Sync popover is also open and gets closed', async () => {
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    fireEvent.click(screen.getByText('pages.export'));
    fireEvent.click(screen.getByLabelText('pages.autoRefresh'));
    expect(screen.getByTestId('live-page-export-menu')).toHaveAttribute('open');
    expect(screen.getByTestId('live-page-refresh-menu')).toHaveAttribute('open');

    // Outside the Sync popover, but inside the Export popover.
    fireEvent.mouseDown(screen.getByRole('button', { name: 'PDF' }));
    expect(screen.getByTestId('live-page-export-menu')).toHaveAttribute('open');
    expect(screen.getByTestId('live-page-refresh-menu')).not.toHaveAttribute('open');
  });

  it('Escape closes the Export popover and returns focus to its summary', async () => {
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    const summary = screen.getByText('pages.export').closest('summary')!;
    fireEvent.click(summary);
    expect(screen.getByTestId('live-page-export-menu')).toHaveAttribute('open');

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.getByTestId('live-page-export-menu')).not.toHaveAttribute('open');
    expect(document.activeElement).toBe(summary);
  });

  it('Escape closes the Sync popover and returns focus to its summary', async () => {
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');
    const summary = screen.getByLabelText('pages.autoRefresh');
    fireEvent.click(summary);
    expect(screen.getByTestId('live-page-refresh-menu')).toHaveAttribute('open');

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.getByTestId('live-page-refresh-menu')).not.toHaveAttribute('open');
    expect(document.activeElement).toBe(summary);
  });

  it('Escape and an outside click are no-ops when no popover is open', async () => {
    render(<PagesPage />);
    await screen.findByTestId('live-page-frame');

    expect(() => {
      fireEvent.keyDown(document, { key: 'Escape' });
      fireEvent.mouseDown(document.body);
    }).not.toThrow();
    expect(screen.getByTestId('live-page-export-menu')).not.toHaveAttribute('open');
    expect(screen.getByTestId('live-page-refresh-menu')).not.toHaveAttribute('open');
  });
});
