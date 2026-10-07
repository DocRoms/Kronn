import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, fireEvent, act, cleanup } from '@testing-library/react';
import { RouterProvider } from 'react-router/dom';
import { createAppRouter } from '../router';
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
  StandaloneLivePage: ({ pageId, params }: { pageId: string; params?: Record<string, string> }) => (
    <div data-testid="standalone-page" data-params={JSON.stringify(params ?? {})}>{pageId}</div>
  ),
}));

vi.mock('../pages/StandaloneLivePageMosaic', () => ({
  StandaloneLivePageMosaic: ({ pageIds, layout }: { pageIds: string[]; layout: string }) => (
    <div data-testid="standalone-page-mosaic">{layout}:{pageIds.join(',')}</div>
  ),
}));

vi.mock('../pages/StandaloneDiscussionMosaic', () => ({
  StandaloneDiscussionMosaic: ({ discussionIds, layout, onLayoutChange }: { discussionIds: string[]; layout: string; onLayoutChange: (layout: string) => void }) => (
    <div data-testid="standalone-discussion-mosaic">
      {layout}:{discussionIds.join(',')}
      <button onClick={() => onLayoutChange('two-columns')}>two columns</button>
    </div>
  ),
}));

// The banners around the dashboard poll the backend; inert here.
vi.mock('../components/UpdateBanner', () => ({ UpdateBanner: () => null }));
vi.mock('../components/BackendStatus', () => ({ BackendStatus: () => null }));

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
    getRecoveryStatus: vi.fn().mockResolvedValue({ key_locked: true }),
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

type AppRouter = ReturnType<typeof createAppRouter>;
let router: AppRouter | null = null;

/** The whole app, as `main.tsx` mounts it, opened at `path`. */
function renderApp(path = '/') {
  window.history.replaceState(null, '', path);
  router = createAppRouter();
  return render(<RouterProvider router={router} useTransitions={false} />);
}

const setupComplete = {
  is_first_run: false,
  current_step: 'Complete' as const,
  agents_detected: [],
  scan_paths_set: true,
  scan_paths_explored: [],
  config_set_aside: null,
  repos_detected: [],
  default_scan_path: '/home',
};

beforeEach(() => {
  vi.clearAllMocks();
  clearCachedSetupStatus();
  setRetryDelay(0); // instant retries in tests
  resetBootScreenForTests();
  setStatusTimeout(20); // short boot timeout so hangs resolve fast in tests
  // Default: backend unreachable on the fast probe too (matches the existing
  // "backend down" expectations). Individual tests override.
  (configApi.getLanguage as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('down'));
});

afterEach(() => {
  cleanup();
  router?.dispose();
  router = null;
  window.history.replaceState(null, '', '/');
});

describe('App', () => {
  it('shows the key-restore screen, not the loader, when the API is auth locked', async () => {
    const { ApiRequestError } = await import('../lib/apiRequestError');
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockRejectedValue(
      new ApiRequestError('API authentication is locked', 'auth_locked'),
    );
    renderApp();
    await waitFor(() => expect(screen.getByTestId('auth-locked-screen')).toBeTruthy());
    // The panel appears once the screen has read the recovery status.
    expect(await screen.findByTestId('recovery-restore-panel')).toBeTruthy();
    expect(screen.queryByRole('status')).toBeNull();
    expect(setupApi.getStatus).toHaveBeenCalledTimes(1);
  });

  it('shows loading screen initially', () => {
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(() => {}));
    renderApp();
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

    renderApp();
    await waitFor(() => expect(screen.getByTestId('setup-wizard')).toBeDefined());
  });

  it('shows a failed reset with the backend message (C3-14)', async () => {
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockResolvedValue({
      is_first_run: false,
      current_step: 'Complete',
      agents_detected: [],
      scan_paths_set: true,
      repos_detected: [],
      default_scan_path: '/home',
    });
    (setupApi.reset as ReturnType<typeof vi.fn>).mockRejectedValue(
      new Error('Reset cleared the data but not the stored keys: disk full'),
    );
    renderApp();
    fireEvent.click(await screen.findByText('Reset'));
    await waitFor(() =>
      expect(screen.getByTestId('reset-error').textContent).toContain('not the stored keys'),
    );
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

    renderApp();
    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeDefined());
  });

  it('shows a config.toml set aside at start on the dashboard (C5-02)', async () => {
    vi.mocked(configApi.getRecoveryStatus).mockResolvedValueOnce({
      key_locked: false,
      config_set_aside: 'config.toml could not be read and was kept as config.toml.corrupt.1',
    } as never);
    vi.mocked(setupApi.getStatus).mockResolvedValue({ is_first_run: false, current_step: 'Complete', agents_detected: [], scan_paths_set: true, scan_paths_explored: [], config_set_aside: null, repos_detected: [], default_scan_path: '/home' });
    renderApp();
    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeInTheDocument());
    await waitFor(() => expect(screen.getByTestId('config-set-aside-banner')).toHaveTextContent('config.toml.corrupt.1'));
  });

  it('renders the last known setup state while refreshing it in the background', async () => {
    const cached = {
      is_first_run: false,
      current_step: 'Complete' as const,
      agents_detected: [],
      scan_paths_set: true,
      scan_paths_explored: [], config_set_aside: null,
      repos_detected: [],
      default_scan_path: '/home',
    };
    cacheSetupStatus(cached);
    setStatusTimeout(2_000);
    let finishRefresh!: (status: typeof cached) => void;
    vi.mocked(setupApi.getStatus).mockReturnValue(new Promise(resolve => { finishRefresh = resolve; }));

    renderApp();

    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeInTheDocument());
    expect(screen.queryByText(bootMessage('connecting'))).not.toBeInTheDocument();
    expect(setupApi.getStatus).toHaveBeenCalledTimes(1);
    await act(async () => { finishRefresh(cached); });
  });

  it('opens a direct Live Page address without mounting the dashboard chrome', async () => {
    vi.mocked(setupApi.getStatus).mockResolvedValue(setupComplete);

    renderApp('/standalone/pages/page%2F%C3%A9quipe');

    await waitFor(() => expect(screen.getByTestId('standalone-page')).toHaveTextContent('page/équipe'));
    expect(screen.queryByTestId('dashboard')).toBeNull();
  });

  it('hands a Live Page the view parameters its address carries, and only safe ones', async () => {
    vi.mocked(setupApi.getStatus).mockResolvedValue(setupComplete);

    renderApp('/standalone/pages/wall?tv=1&scene=standup&x=%3Cb%3E');

    await waitFor(() => expect(screen.getByTestId('standalone-page')).toHaveTextContent('wall'));
    expect(screen.getByTestId('standalone-page')).toHaveAttribute('data-params', JSON.stringify({ tv: '1', scene: 'standup' }));
  });

  it('keeps the view parameters of a legacy #page/ link', async () => {
    vi.mocked(setupApi.getStatus).mockResolvedValue(setupComplete);

    renderApp('/#page/wall?tv=1');

    await waitFor(() => expect(screen.getByTestId('standalone-page')).toHaveAttribute('data-params', JSON.stringify({ tv: '1' })));
    expect(window.location.pathname).toBe('/standalone/pages/wall');
    expect(window.location.search).toBe('?tv=1');
  });

  it.each([
    ['#page/page-1', '/standalone/pages/page-1', 'standalone-page', 'page-1'],
    ['#pages/mosaic?page=page-1&page=page-2&layout=two-columns', '/standalone/pages/mosaic', 'standalone-page-mosaic', 'two-columns:page-1,page-2'],
    ['#discussions/mosaic?discussion=a&discussion=b&layout=two-rows', '/standalone/discussions/mosaic', 'standalone-discussion-mosaic', 'two-rows:a,b'],
  ])('still honours the legacy %s deep link, at its new address', async (hash, path, testId, content) => {
    vi.mocked(setupApi.getStatus).mockResolvedValue(setupComplete);

    renderApp(`/${hash}`);

    await waitFor(() => expect(screen.getByTestId(testId)).toHaveTextContent(content));
    expect(window.location.pathname).toBe(path);
    expect(window.location.hash).toBe('');
    expect(screen.queryByTestId('dashboard')).toBeNull();
  });

  it.each([
    ['#config', '/config', ''],
    ['#settings/artifacts?origin=https%3A%2F%2Fvimeo.com', '/config/artifacts', '?origin=https%3A%2F%2Fvimeo.com'],
    ['#project-proj-7', '/projects/proj-7', ''],
    ['#discussion-disc-42', '/discussions/disc-42', ''],
    ['#discussion-disc%2F42?message=msg-1', '/discussions/disc%2F42', '?message=msg-1'],
  ])('sends the legacy %s link to %s, on the dashboard', async (hash, path, search) => {
    vi.mocked(setupApi.getStatus).mockResolvedValue(setupComplete);

    renderApp(`/planning${hash}`);

    await waitFor(() => expect(window.location.pathname).toBe(path));
    expect(window.location.search).toBe(search);
    expect(window.location.hash).toBe('');
    expect(screen.getByTestId('dashboard')).toBeInTheDocument();
  });

  it('follows a legacy link set while the app is open', async () => {
    vi.mocked(setupApi.getStatus).mockResolvedValue(setupComplete);
    renderApp('/projects');
    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeInTheDocument());

    // What `window.location.hash = '#config'` does in a browser.
    await act(async () => {
      window.history.pushState(null, '', '#config');
      window.dispatchEvent(new PopStateEvent('popstate'));
    });

    await waitFor(() => expect(window.location.pathname).toBe('/config'));
  });

  it('leaves a bad Live Page address for the default page', async () => {
    vi.mocked(setupApi.getStatus).mockResolvedValue(setupComplete);

    renderApp('/standalone/pages/mosaic?page=only-one');

    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeInTheDocument());
    expect(window.location.pathname).toBe('/projects');
  });

  it('writes a mosaic layout change back into the address, in place', async () => {
    vi.mocked(setupApi.getStatus).mockResolvedValue(setupComplete);
    renderApp('/standalone/discussions/mosaic?discussion=a&discussion=b&layout=two-rows');
    await screen.findByTestId('standalone-discussion-mosaic');
    const depth = window.history.length;

    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'two columns' })); });

    await waitFor(() => expect(screen.getByTestId('standalone-discussion-mosaic')).toHaveTextContent('two-columns:a,b'));
    expect(window.location.search).toBe('?discussion=a&discussion=b&layout=two-columns');
    expect(window.history.length).toBe(depth);
  });

  it('opens an internal Live Page link in the current application tab', async () => {
    vi.mocked(setupApi.getStatus).mockResolvedValue(setupComplete);
    renderApp('/pages');
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
    expect(window.location.pathname).toBe('/standalone/pages/page-in-place');
    expect(openExternal).not.toHaveBeenCalled();
    relay.dispose();
  });

  it('opens a direct Page mosaic address without mounting the dashboard chrome', async () => {
    vi.mocked(setupApi.getStatus).mockResolvedValue(setupComplete);

    renderApp('/standalone/pages/mosaic?page=page-1&page=page-2&layout=two-columns');

    await waitFor(() => expect(screen.getByTestId('standalone-page-mosaic'))
      .toHaveTextContent('two-columns:page-1,page-2'));
    expect(screen.queryByTestId('dashboard')).toBeNull();
  });

  it('opens a direct discussion mosaic without mounting the dashboard', async () => {
    vi.mocked(setupApi.getStatus).mockResolvedValue(setupComplete);

    renderApp('/standalone/discussions/mosaic?discussion=a&discussion=b&layout=two-rows');

    await waitFor(() => expect(screen.getByTestId('standalone-discussion-mosaic')).toHaveTextContent('two-rows:a,b'));
    expect(screen.queryByTestId('dashboard')).toBeNull();
  });

  it('starts the setup over from the dashboard', async () => {
    vi.mocked(setupApi.getStatus).mockResolvedValue(setupComplete);
    vi.mocked(setupApi.reset).mockResolvedValue(undefined);
    renderApp('/projects');
    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeInTheDocument());
    vi.mocked(setupApi.getStatus).mockResolvedValue({ ...setupComplete, is_first_run: true, current_step: 'Agents' });

    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Reset' })); });

    expect(setupApi.reset).toHaveBeenCalledOnce();
    await waitFor(() => expect(screen.getByTestId('setup-wizard')).toBeInTheDocument());
  });

  it('keeps the loading screen and keeps retrying while the backend is unreachable', async () => {
    // A restart with migrations outlasts the quick retries: the user must keep
    // seeing Kronn starting, never an error that reads as a crash.
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('Network error'));

    renderApp();

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
    renderApp();
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

    renderApp();
    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeDefined());
    expect(screen.queryByText('Cannot connect to backend')).toBeNull();
  });

  it('keeps the loading screen when setup/status hangs AND the backend is unreachable', async () => {
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(() => {})); // hang
    // configApi.getLanguage rejects by default (beforeEach) → backend down.
    renderApp();
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(bootMessage('slow')));
    expect(screen.queryByTestId('dashboard')).toBeNull();
  });

  it('retries at once when the user clicks Retry under a slow start', async () => {
    const mockGetStatus = setupApi.getStatus as ReturnType<typeof vi.fn>;
    mockGetStatus.mockRejectedValue(new Error('Network error'));
    renderApp();
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
