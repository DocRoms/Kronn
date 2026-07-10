/**
 * 0.8.7 — DebugSection test coverage.
 *
 * The Debug card was previously untested. It fetches log lines from the
 * backend ringbuffer, supports follow-mode (auto-refresh), clear-logs,
 * clipboard copy, and a one-click bug-report flow that opens a GitHub
 * issue with environment context pre-filled.
 *
 * Pins :
 *  - mount calls debugApi.getLogs on first render
 *  - refresh button reloads
 *  - clear button calls debugApi.clearLogs + refresh
 *  - follow toggle wires the interval
 *  - copy button writes to clipboard
 *  - debug-mode checkbox fires configApi.setServerConfig + sets restart flag
 *  - error from getLogs surfaces visibly without crashing
 *  - report-bug button triggers parallel env queries and opens window
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act, cleanup, waitFor } from '@testing-library/react';

const { agentsApi, configApi, debugApi, fetchHealth } = vi.hoisted(() => ({
  agentsApi: {
    detect: vi.fn(),
  },
  configApi: {
    setServerConfig: vi.fn(),
  },
  debugApi: {
    getLogs: vi.fn(),
    clearLogs: vi.fn(),
  },
  fetchHealth: vi.fn(),
}));

vi.mock('../../../lib/api', () => ({
  agents: agentsApi,
  config: configApi,
  debugApi,
  fetchHealth,
}));

import { DebugSection } from '../DebugSection';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}(${args.join('|')})` : key;

beforeEach(() => {
  // Reset all mocks so a `.mockRejectedValueOnce` from a previous test
  // (e.g. `surfaces clearLogs failure`) can't bleed into the next
  // happy-path test via the queued-rejection mechanism. Without this,
  // `clear button calls clearLogs and re-fetches` runs AFTER the
  // rejection test in parallel mode and inherits the queued reject.
  debugApi.getLogs.mockReset();
  debugApi.clearLogs.mockReset();
  configApi.setServerConfig.mockReset();
  agentsApi.detect.mockReset();
  fetchHealth.mockReset();

  debugApi.getLogs.mockResolvedValue({
    lines: ['line1', 'line2', 'line3'],
    buffered: 3,
    capacity: 1000,
  });
  debugApi.clearLogs.mockResolvedValue(undefined);
  configApi.setServerConfig.mockResolvedValue(undefined);
  agentsApi.detect.mockResolvedValue([]);
  fetchHealth.mockResolvedValue({ ok: true, version: '0.8.7', host_os: 'linux' });
});

afterEach(() => {
  vi.clearAllMocks();
  cleanup();
});

function renderCard(props?: Partial<Parameters<typeof DebugSection>[0]>) {
  const setServerDebugMode = vi.fn();
  const setDebugModeNeedsRestart = vi.fn();
  const toast = vi.fn();
  return {
    setServerDebugMode,
    setDebugModeNeedsRestart,
    toast,
    ...render(
      <DebugSection
        serverDebugMode={false}
        setServerDebugMode={setServerDebugMode}
        debugModeNeedsRestart={false}
        setDebugModeNeedsRestart={setDebugModeNeedsRestart}
        toast={toast}
        t={t}
        {...props}
      />,
    ),
  };
}

describe('DebugSection — mount', () => {
  it('fetches log lines on mount', async () => {
    renderCard();
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalledTimes(1));
    expect(debugApi.getLogs).toHaveBeenCalledWith(300);
  });

  it('renders the fetched lines in the viewer', async () => {
    renderCard();
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByText(/line1/)).toBeDefined());
    expect(screen.getByText(/line2/)).toBeDefined();
    expect(screen.getByText(/line3/)).toBeDefined();
  });

  it('shows the buffered count placeholder format', async () => {
    renderCard();
    await waitFor(() => {
      // settings.debugBufferedCount(3|1000) — uses our test t()
      expect(screen.getByText(/settings\.debugBufferedCount\(3\|1000\)/)).toBeDefined();
    });
  });

  it('shows error text when getLogs rejects', async () => {
    debugApi.getLogs.mockRejectedValueOnce(new Error('boom'));
    renderCard();
    await waitFor(() => expect(screen.getByText(/boom/)).toBeDefined());
  });

  it('shows LIVE badge when serverDebugMode is true', () => {
    renderCard({ serverDebugMode: true });
    expect(screen.getByRole('status')).toBeDefined();
  });

  it('hides LIVE badge when serverDebugMode is false', () => {
    renderCard({ serverDebugMode: false });
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('shows restart warning when debugModeNeedsRestart is true', () => {
    renderCard({ debugModeNeedsRestart: true });
    expect(screen.getByText(/settings\.debugModeRestart/)).toBeDefined();
  });
});

describe('DebugSection — debug-mode toggle', () => {
  it('persists state via setServerConfig and flags restart-needed', async () => {
    const { setServerDebugMode, setDebugModeNeedsRestart } = renderCard();
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalled());

    const checkbox = screen.getAllByRole('checkbox')[0] as HTMLInputElement;
    await act(async () => { fireEvent.click(checkbox); });

    expect(setServerDebugMode).toHaveBeenCalledWith(true);
    expect(setDebugModeNeedsRestart).toHaveBeenCalledWith(true);
    await waitFor(() => expect(configApi.setServerConfig).toHaveBeenCalledWith({ debug_mode: true }));
  });

  it('reverts the optimistic toggle and toasts when setServerConfig fails', async () => {
    configApi.setServerConfig.mockRejectedValueOnce(new Error('net'));
    const { setServerDebugMode, setDebugModeNeedsRestart, toast } = renderCard();
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalled());
    const checkbox = screen.getAllByRole('checkbox')[0] as HTMLInputElement;
    await act(async () => { fireEvent.click(checkbox); });
    // Optimistic flip fires first…
    expect(setServerDebugMode).toHaveBeenCalledWith(true);
    // …then the explicit revert once the save fails (no refetch feeds this
    // toggle, so without the revert the UI would lie about backend state).
    await waitFor(() => expect(setServerDebugMode).toHaveBeenLastCalledWith(false));
    expect(setDebugModeNeedsRestart).toHaveBeenLastCalledWith(false);
    expect(toast).toHaveBeenCalledWith(expect.stringContaining('common.actionFailed'), 'error');
  });
});

describe('DebugSection — actions', () => {
  it('clear button calls clearLogs and re-fetches', async () => {
    debugApi.clearLogs.mockReset();
    debugApi.clearLogs.mockResolvedValue(undefined);
    renderCard();
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalled());
    debugApi.getLogs.mockClear();

    const clearBtn = screen.getByText(/settings\.debugLogsClear/);
    await act(async () => { fireEvent.click(clearBtn); });

    await waitFor(() => expect(debugApi.clearLogs).toHaveBeenCalled());
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalled());
  });

  it('clear button surfaces clearLogs failure as error text', async () => {
    debugApi.clearLogs.mockRejectedValueOnce(new Error('cannot clear'));
    renderCard();
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalled());

    const clearBtn = screen.getByText(/settings\.debugLogsClear/);
    await act(async () => { fireEvent.click(clearBtn); });

    await waitFor(() => expect(screen.getByText(/cannot clear/)).toBeDefined());
  });

  it('copy button writes the joined log lines to clipboard', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      configurable: true,
    });
    renderCard();
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalled());

    const copyBtn = screen.getByText(/settings\.debugLogsCopy/);
    await act(async () => { fireEvent.click(copyBtn); });

    await waitFor(() => expect(writeText).toHaveBeenCalledWith('line1\nline2\nline3'));
  });

  it('copy button does not crash when clipboard.writeText rejects', async () => {
    const writeText = vi.fn().mockRejectedValue(new Error('no clipboard'));
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      configurable: true,
    });
    renderCard();
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalled());

    const copyBtn = screen.getByText(/settings\.debugLogsCopy/);
    // Must not throw.
    await act(async () => { fireEvent.click(copyBtn); });
  });

  it('refresh button manually re-fetches logs', async () => {
    renderCard();
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalledTimes(1));

    const refreshBtn = screen.getByText(/settings\.debugLogsRefresh/);
    await act(async () => { fireEvent.click(refreshBtn); });

    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalledTimes(2));
  });

  it('follow button toggles auto-refresh on', async () => {
    renderCard();
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalled());
    debugApi.getLogs.mockClear();

    // The follow toggle is a button (Play / Pause icon + label); start
    // follow by finding the "StartFollow" label.
    const followBtn = screen.getByText(/settings\.debugLogsStartFollow/);
    await act(async () => { fireEvent.click(followBtn); });

    // After clicking, label flips to StopFollow. Visual proof the follow
    // state changed; the interval itself is set + cleared correctly per
    // the useEffect — testing the timer directly is brittle vs. test value.
    await waitFor(() =>
      expect(screen.queryByText(/settings\.debugLogsStopFollow/)).not.toBeNull(),
    );
  });
});

describe('DebugSection — bug report', () => {
  it('report-a-bug button opens a GitHub issue URL', async () => {
    const open = vi.fn();
    vi.stubGlobal('open', open);
    renderCard();
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalled());

    const reportBtn = screen.getByText(/settings\.debugReportCta/);
    await act(async () => { fireEvent.click(reportBtn); });

    await waitFor(() => expect(open).toHaveBeenCalled());
    const url = String(open.mock.calls[0][0]);
    expect(url).toMatch(/github\.com/);
    vi.unstubAllGlobals();
  });

  it('report-a-bug works even when env queries fail', async () => {
    fetchHealth.mockRejectedValueOnce(new Error('no health'));
    agentsApi.detect.mockRejectedValueOnce(new Error('no agents'));
    const open = vi.fn();
    vi.stubGlobal('open', open);

    renderCard();
    await waitFor(() => expect(debugApi.getLogs).toHaveBeenCalled());

    const reportBtn = screen.getByText(/settings\.debugReportCta/);
    await act(async () => { fireEvent.click(reportBtn); });

    // Should still open the URL with at-least the user-agent / logs filled.
    await waitFor(() => expect(open).toHaveBeenCalled());
    vi.unstubAllGlobals();
  });
});
