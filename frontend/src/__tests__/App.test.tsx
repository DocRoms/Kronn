import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { App, setRetryDelay } from '../App';

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
}));

import { setup as setupApi } from '../lib/api';

beforeEach(() => {
  vi.clearAllMocks();
  setRetryDelay(0); // instant retries in tests
});

describe('App', () => {
  it('shows loading screen initially', () => {
    (setupApi.getStatus as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(() => {}));
    render(<App />);
    expect(screen.getByText('Entering the grid...')).toBeDefined();
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
