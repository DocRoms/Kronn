import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { App, setRetryDelay, setStatusTimeout } from '../App';

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
}));

import { setup as setupApi, config as configApi } from '../lib/api';

beforeEach(() => {
  vi.clearAllMocks();
  setRetryDelay(0); // instant retries in tests
  setStatusTimeout(20); // short boot timeout so hangs resolve fast in tests
  // Default: backend unreachable on the fast probe too (matches the existing
  // "backend down" expectations). Individual tests override.
  (configApi.getLanguage as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('down'));
});

describe('App', () => {
  it('shows loading screen initially', () => {
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(() => {}));
    render(<App />);
    // The splash cycles through hints every 1.5s — the first one always
    // starts with "Entering the grid". The trailing character is a
    // unicode ellipsis (…), not three dots.
    expect(screen.getByText(/^Entering the grid…?$/)).toBeDefined();
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

  it('shows API error screen after exhausting auto-retries', async () => {
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('Network error'));

    render(<App />);

    await waitFor(() => expect(screen.getByText('Cannot connect to backend')).toBeDefined());
    expect(screen.queryByTestId('setup-wizard')).toBeNull();
    // 1 initial + 5 retries = 6 calls
    expect(setupApi.getStatus).toHaveBeenCalledTimes(6);
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

  it('shows the error screen when setup/status hangs AND the backend is unreachable', async () => {
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(() => {})); // hang
    // configApi.getLanguage rejects by default (beforeEach) → backend down.
    render(<App />);
    await waitFor(() => expect(screen.getByText('Cannot connect to backend')).toBeDefined());
    expect(screen.queryByTestId('dashboard')).toBeNull();
  });

  it('retries connection when clicking Retry on API error screen', async () => {
    const mockGetStatus = setupApi.getStatus as ReturnType<typeof vi.fn>;
    mockGetStatus.mockRejectedValue(new Error('Network error'));

    render(<App />);

    await waitFor(() => expect(screen.getByText('Cannot connect to backend')).toBeDefined());

    // Manual retry: success
    mockGetStatus.mockResolvedValueOnce({
      is_first_run: false,
      current_step: 'Complete',
      agents_detected: [],
      scan_paths_set: true,
      repos_detected: [],
      default_scan_path: '/home',
    });

    fireEvent.click(screen.getByText('Retry'));
    await waitFor(() => expect(screen.getByTestId('dashboard')).toBeDefined());
  });
});
