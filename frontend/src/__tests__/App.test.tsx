import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent, act } from '@testing-library/react';
import { App } from '../App';
import {
  cacheSetupStatus,
  clearCachedSetupStatus,
  setRetryDelay,
  setStatusTimeout,
} from '../lib/appBoot';
import { createLivePageOpenLinkRelay } from '../lib/live-page-sandbox';
import { bootMessage, resetBootScreenForTests } from '../lib/bootScreen';

// Mock the lazy-loaded pages to avoid loading the full component trees
vi.mock('../pages/SetupWizard', () => ({
  SetupWizard: ({ onComplete }: { onComplete: () => void }) => (
    <div data-testid="setup-wizard">
      <button onClick={onComplete}>Complete</button>
    </div>
  ),
}));

vi.mock('../pages/Dashboard', () => ({
  Dashboard: ({ onReset }: { onReset: () => void }) => (
    <div data-testid="dashboard">
      <button onClick={onReset}>Reset</button>
    </div>
  ),
}));

vi.mock('../pages/StandaloneLivePage', () => ({
  StandaloneLivePage: ({ pageId }: { pageId: string }) => (
    <div data-testid="standalone-page">{pageId}</div>
  ),
}));

vi.mock('../pages/StandaloneLivePageMosaic', () => ({
  StandaloneLivePageMosaic: ({ pageIds, layout }: { pageIds: string[]; layout: string }) => (
    <div data-testid="standalone-page-mosaic">{layout}:{pageIds.join(',')}</div>
  ),
}));

vi.mock('../pages/StandaloneDiscussionMosaic', () => ({
  StandaloneDiscussionMosaic: ({ discussionIds, layout }: { discussionIds: string[]; layout: string }) => (
    <div data-testid="standalone-discussion-mosaic">{layout}:{discussionIds.join(',')}</div>
  ),
}));

// Mock the API
vi.mock('../lib/api', () => ({
  setup: {
    getStatus: vi.fn(),
    reset: vi.fn(),
  },
  // The boot's timeout-and-proceed path probes a fast endpoint to tell
  // "backend slow" from "backend down".
  config: {
    getLanguage: vi.fn(),
  },
  // UpdateBanner is rendered inside Dashboard via App's tree and calls
  // version.check on mount. The Dashboard component is itself mocked
  // above so it never actually mounts UpdateBanner, BUT in the real
  // App tree (e.g. when the lazy import resolves before mocks take
  // effect in the test runner) the import chain still asks for
  // `version`. Mocking it as a never-resolving promise keeps things
  // inert without forcing the Dashboard mock to handle it.
  version: {
    check: vi.fn().mockReturnValue(new Promise(() => {})),
  },
  // App probes /api/health on mount to learn whether it runs under Docker
  // (gates the wizard's Install button). Inert in tests.
  health: {
    get: vi.fn().mockResolvedValue({ ok: true, version: 'test', host_os: 'test', in_docker: false }),
  },
}));

import { setup as setupApi, config as configApi } from '../lib/api';

beforeEach(() => {
  vi.clearAllMocks();
  window.location.hash = '';
  clearCachedSetupStatus();
  setRetryDelay(0); // instant retries in tests
  resetBootScreenForTests();
  setStatusTimeout(20); // short boot timeout so hangs resolve fast in tests
  // Default: backend unreachable on the fast probe too (matches the existing
  // "backend down" expectations). Individual tests override.
  (configApi.getLanguage as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('down'));
});

describe('App', () => {
  it('shows loading screen initially', () => {
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(() => {}));
    render(<App />);
    expect(screen.getByRole('status')).toHaveTextContent(bootMessage('connecting'));
    const status = screen.getByRole('status');
    const mark = status.querySelector('svg');
    expect(mark).toHaveAttribute('width', '100');
    expect(mark).toHaveAttribute('height', '100');
    expect(mark).toHaveAttribute('aria-hidden', 'true');
    expect(mark?.querySelectorAll('circle')).toHaveLength(4);
  });

  it('shows SetupWizard when setup is incomplete', async () => {
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockResolvedValue({
      is_first_run: true,
      current_step: 'Agents',
      agents_detected: [],
      scan_paths_set: false,
      repos_detected: [],
      default_scan_path: null,
    });

    render(<App />);
    await waitFor(() => expect(screen.getByTestId('setup-wizard')).toBeDefined());
  });

  it('shows Dashboard when setup is complete', async () => {
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockResolvedValue({
      is_first_run: false,
      current_step: 'Complete',
      agents_detected: [],
      scan_paths_set: true,
      repos_detected: [],
      default_scan_path: '/home',
    });

    render(<App />);
    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeDefined());
  });

  it('renders the last known setup state while refreshing it in the background', async () => {
    const cached = {
      is_first_run: false,
      current_step: 'Complete' as const,
      agents_detected: [],
      scan_paths_set: true,
      scan_paths_explored: [],
      repos_detected: [],
      default_scan_path: '/home',
    };
    cacheSetupStatus(cached);
    setStatusTimeout(2_000);
    let finishRefresh!: (status: typeof cached) => void;
    vi.mocked(setupApi.getStatus).mockReturnValue(new Promise(resolve => { finishRefresh = resolve; }));

    render(<App />);

    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeInTheDocument());
    expect(screen.queryByText(bootMessage('connecting'))).not.toBeInTheDocument();
    expect(setupApi.getStatus).toHaveBeenCalledTimes(1);
    await act(async () => { finishRefresh(cached); });
  });

  it('opens a direct Live Page URL without mounting the dashboard chrome', async () => {
    window.location.hash = '#page/page-1';
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockResolvedValue({
      is_first_run: false,
      current_step: 'Complete',
      agents_detected: [],
      scan_paths_set: true,
      repos_detected: [],
      default_scan_path: '/home',
    });

    render(<App />);
    await waitFor(() => expect(screen.getByTestId('standalone-page')).toHaveTextContent('page-1'));
    expect(screen.queryByTestId('dashboard')).toBeNull();
  });

  it('opens an internal Live Page link in the current application tab', async () => {
    vi.mocked(setupApi.getStatus).mockResolvedValue({
      is_first_run: false,
      current_step: 'Complete',
      agents_detected: [],
      scan_paths_set: true,
      scan_paths_explored: [],
      repos_detected: [],
      default_scan_path: '/home',
    });
    render(<App />);
    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeInTheDocument());

    const postMessage = vi.fn();
    const openExternal = vi.fn();
    const relay = createLivePageOpenLinkRelay('channel-1', { openExternal });
    relay.connect({ postMessage } as unknown as Window);
    const port = (postMessage.mock.calls[0][2] as MessagePort[])[0];
    port.postMessage({
      type: 'kronn:page-open-link',
      version: 1,
      channel_id: 'channel-1',
      url: `${window.location.origin}${window.location.pathname}#page/page-in-place`,
    });

    expect(await screen.findByTestId('standalone-page')).toHaveTextContent('page-in-place');
    expect(openExternal).not.toHaveBeenCalled();
    relay.dispose();
  });

  it('opens a direct Page mosaic URL without mounting the dashboard chrome', async () => {
    window.location.hash = '#pages/mosaic?page=page-1&page=page-2&layout=two-columns';
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockResolvedValue({
      is_first_run: false,
      current_step: 'Complete',
      agents_detected: [],
      scan_paths_set: true,
      repos_detected: [],
      default_scan_path: '/home',
    });

    render(<App />);
    await waitFor(() => expect(screen.getByTestId('standalone-page-mosaic'))
      .toHaveTextContent('two-columns:page-1,page-2'));
    expect(screen.queryByTestId('dashboard')).toBeNull();
  });

  it('opens a direct discussion mosaic without mounting the dashboard', async () => {
    window.location.hash = '#discussions/mosaic?discussion=a&discussion=b&layout=two-rows';
    vi.mocked(setupApi.getStatus).mockResolvedValue({ is_first_run: false, current_step: 'Complete', agents_detected: [], scan_paths_set: true, scan_paths_explored: [], repos_detected: [], default_scan_path: '/home' });
    render(<App />);
    await waitFor(() => expect(screen.getByTestId('standalone-discussion-mosaic')).toHaveTextContent('two-rows:a,b'));
    expect(screen.queryByTestId('dashboard')).toBeNull();
  });

  it('keeps the loading screen and keeps retrying while the backend is unreachable', async () => {
    // A restart with migrations outlasts the quick retries: the user must keep
    // seeing Kronn starting, never an error that reads as a crash.
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('Network error'));

    render(<App />);

    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(bootMessage('slow')));
    expect(screen.queryByText('Cannot connect to backend')).toBeNull();
    expect(screen.queryByTestId('setup-wizard')).toBeNull();
    expect(screen.getByRole('status').querySelector('svg')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: bootMessage('retry') })).toBeEnabled();
    // Past the 1 + 5 quick attempts, it still tries.
    await waitFor(() => expect(vi.mocked(setupApi.getStatus).mock.calls.length).toBeGreaterThan(7));
  });

  it('opens the app on its own once the backend answers after a long start', async () => {
    const mockGetStatus = setupApi.getStatus as ReturnType<typeof vi.fn>;
    mockGetStatus.mockRejectedValue(new Error('Network error'));
    render(<App />);
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(bootMessage('slow')));

    mockGetStatus.mockResolvedValue({
      is_first_run: false,
      current_step: 'Complete',
      agents_detected: [],
      scan_paths_set: true,
      repos_detected: [],
      default_scan_path: '/home',
    });
    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeDefined());
  });

  it('proceeds to the dashboard when setup/status HANGS but the backend is reachable', async () => {
    // Regression: a hung setup/status (never resolves) used to freeze the boot
    // on "Almost ready…" forever — the retry only fired on rejection. The
    // timeout now converts the hang into retries, and since the fast probe
    // answers, the app proceeds optimistically instead of staying stuck.
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(() => {})); // hang
    (configApi.getLanguage as ReturnType<typeof vi.fn>).mockResolvedValue('fr'); // backend up

    render(<App />);
    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeDefined());
    expect(screen.queryByText('Cannot connect to backend')).toBeNull();
  });

  it('keeps the loading screen when setup/status hangs AND the backend is unreachable', async () => {
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(() => {})); // hang
    // configApi.getLanguage rejects by default (beforeEach) → backend down.
    render(<App />);
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(bootMessage('slow')));
    expect(screen.queryByTestId('dashboard')).toBeNull();
  });

  it('retries at once when the user clicks Retry under a slow start', async () => {
    const mockGetStatus = setupApi.getStatus as ReturnType<typeof vi.fn>;
    mockGetStatus.mockRejectedValue(new Error('Network error'));
    render(<App />);
    await waitFor(() => expect(screen.getByRole('button', { name: bootMessage('retry') })).toBeDefined());
    // Automatic retries out of the way: from here only the click can retry.
    setRetryDelay(60_000);
    await act(() => new Promise(resolve => setTimeout(resolve, 50)));
    const callsBeforeClick = mockGetStatus.mock.calls.length;

    mockGetStatus.mockResolvedValue({
      is_first_run: false,
      current_step: 'Complete',
      agents_detected: [],
      scan_paths_set: true,
      repos_detected: [],
      default_scan_path: '/home',
    });
    expect(screen.queryByTestId('dashboard')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: bootMessage('retry') }));
    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeDefined());
    expect(mockGetStatus.mock.calls.length).toBe(callsBeforeClick + 1);
  });
});
