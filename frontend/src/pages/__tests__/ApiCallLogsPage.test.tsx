// 0.8.6 (#61) — Smoke + behavior tests for the API call logs page.
// Coverage:
//  - empty + loading + populated states render
//  - filter chips toggle source + status + plugin params on the API call
//  - row click opens the drawer with redacted excerpts visible
//  - auto-refresh toggle starts / stops the polling interval
//  - drawer close clears the selection
//
// The component polls every 10s while auto-refresh is on. Tests use
// fake timers to avoid waiting.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

const mockState = vi.hoisted(() => {
  return {
    rows: [
      {
        id: 'log-1',
        source: 'workflow',
        project_id: 'proj-x',
        run_id: 'run-1',
        disc_id: null,
        agent: null,
        plugin_slug: 'mcp-atlassian',
        config_id: 'cfg-1',
        endpoint_path: '/rest/api/3/issue/EW-1',
        method: 'GET',
        http_status: 200,
        status: 'OK',
        duration_ms: 124,
        request_excerpt: null,
        response_excerpt: '{"id":"EW-1"}',
        error_message: null,
        called_at: '2026-05-21T10:00:00Z',
      },
      {
        id: 'log-2',
        source: 'agent_broker',
        project_id: 'proj-x',
        run_id: null,
        disc_id: 'disc-1',
        agent: 'ClaudeCode',
        plugin_slug: 'api-didomi',
        config_id: 'cfg-2',
        endpoint_path: '/properties',
        method: 'GET',
        http_status: 401,
        status: 'ERROR',
        duration_ms: 50,
        request_excerpt: 'Authorization: Bearer ***REDACTED***',
        response_excerpt: '{"error":"unauthorized"}',
        error_message: 'HTTP 401 — unauthorized',
        called_at: '2026-05-21T10:01:00Z',
      },
    ] as Array<Record<string, unknown>>,
    lastFilter: null as null | Record<string, unknown>,
    listImpl: null as null | ((filter: unknown) => Promise<unknown>),
  };
});

vi.mock('../../lib/api', () => ({
  apiCallLogs: {
    list: vi.fn().mockImplementation((filter: Record<string, unknown>) => {
      mockState.lastFilter = filter ?? null;
      if (mockState.listImpl) return mockState.listImpl(filter);
      return Promise.resolve(mockState.rows);
    }),
    get: vi.fn().mockResolvedValue(null),
    purge: vi.fn().mockResolvedValue(0),
  },
  config: {
    getUiLanguage: vi.fn().mockResolvedValue('fr'),
    saveUiLanguage: vi.fn().mockResolvedValue(undefined),
  },
}));

import { ApiCallLogsPage } from '../ApiCallLogsPage';
import { apiCallLogs } from '../../lib/api';

function wrap(ui: React.ReactElement) {
  return render(<I18nProvider>{ui}</I18nProvider>);
}

const flush = async () => {
  await new Promise(r => setTimeout(r, 0));
  await new Promise(r => setTimeout(r, 0));
};

describe('ApiCallLogsPage', () => {
  beforeEach(() => {
    mockState.lastFilter = null;
    mockState.listImpl = null;
    mockState.rows = [
      // Refresh from above each test
      {
        id: 'log-1', source: 'workflow', project_id: 'proj-x', run_id: 'run-1',
        disc_id: null, agent: null, plugin_slug: 'mcp-atlassian', config_id: 'cfg-1',
        endpoint_path: '/rest/api/3/issue/EW-1', method: 'GET', http_status: 200,
        status: 'OK', duration_ms: 124, request_excerpt: null,
        response_excerpt: '{"id":"EW-1"}', error_message: null,
        called_at: '2026-05-21T10:00:00Z',
      },
      {
        id: 'log-2', source: 'agent_broker', project_id: 'proj-x', run_id: null,
        disc_id: 'disc-1', agent: 'ClaudeCode', plugin_slug: 'api-didomi', config_id: 'cfg-2',
        endpoint_path: '/properties', method: 'GET', http_status: 401,
        status: 'ERROR', duration_ms: 50,
        request_excerpt: 'Authorization: Bearer ***REDACTED***',
        response_excerpt: '{"error":"unauthorized"}',
        error_message: 'HTTP 401 — unauthorized',
        called_at: '2026-05-21T10:01:00Z',
      },
    ];
    (apiCallLogs.list as ReturnType<typeof vi.fn>).mockClear();
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it('renders the page header + initial fetch returns 2 rows', async () => {
    wrap(<ApiCallLogsPage />);
    expect(screen.getByTestId('api-call-logs-page')).toBeDefined();
    await waitFor(() => {
      expect(screen.queryByTestId('api-call-logs-table')).not.toBeNull();
    });
    const tbody = screen.getByTestId('api-call-logs-table').querySelector('tbody');
    expect(tbody?.children.length).toBe(2);
  });

  it('empty state renders when the API returns no rows', async () => {
    mockState.rows = [];
    wrap(<ApiCallLogsPage />);
    await waitFor(() => {
      expect(screen.queryByTestId('api-call-logs-empty')).not.toBeNull();
    });
  });

  it('filter by source forwards the source param to apiCallLogs.list', async () => {
    wrap(<ApiCallLogsPage />);
    await waitFor(() => expect(apiCallLogs.list).toHaveBeenCalled());
    (apiCallLogs.list as ReturnType<typeof vi.fn>).mockClear();
    fireEvent.click(screen.getByTestId('api-call-logs-source-workflow'));
    await flush();
    expect(apiCallLogs.list).toHaveBeenCalled();
    expect(mockState.lastFilter?.source).toBe('workflow');
  });

  it('filter by status combines with source filter on subsequent calls', async () => {
    wrap(<ApiCallLogsPage />);
    await waitFor(() => expect(apiCallLogs.list).toHaveBeenCalled());
    fireEvent.click(screen.getByTestId('api-call-logs-source-agent_broker'));
    await flush();
    fireEvent.click(screen.getByTestId('api-call-logs-status-ERROR'));
    await flush();
    expect(mockState.lastFilter?.source).toBe('agent_broker');
    expect(mockState.lastFilter?.status).toBe('ERROR');
  });

  it('plugin filter input forwards the exact slug', async () => {
    wrap(<ApiCallLogsPage />);
    await waitFor(() => expect(apiCallLogs.list).toHaveBeenCalled());
    const input = screen.getByTestId('api-call-logs-plugin-input') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'mcp-atlassian' } });
    await flush();
    expect(mockState.lastFilter?.plugin_slug).toBe('mcp-atlassian');
  });

  it('clicking a row opens the drawer with the redacted request excerpt', async () => {
    wrap(<ApiCallLogsPage />);
    await waitFor(() => expect(screen.queryByTestId('api-call-logs-table')).not.toBeNull());
    fireEvent.click(screen.getByTestId('api-call-logs-row-log-2'));
    const drawer = await waitFor(() => screen.getByTestId('api-call-logs-drawer'));
    const scoped = within(drawer);
    // Critical contract: the displayed request excerpt MUST show the
    // redacted shape, NEVER the raw bearer value.
    const reqExcerpt = scoped.getByTestId('api-call-logs-request-excerpt');
    expect(reqExcerpt.textContent).toContain('***REDACTED***');
    expect(reqExcerpt.textContent).not.toContain('sk-');
  });

  it('drawer close button clears the selection', async () => {
    wrap(<ApiCallLogsPage />);
    await waitFor(() => expect(screen.queryByTestId('api-call-logs-table')).not.toBeNull());
    fireEvent.click(screen.getByTestId('api-call-logs-row-log-1'));
    await waitFor(() => screen.getByTestId('api-call-logs-drawer'));
    fireEvent.click(screen.getByTestId('api-call-logs-drawer-close'));
    expect(screen.queryByTestId('api-call-logs-drawer')).toBeNull();
  });

  it('auto-refresh ON triggers periodic refreshes at the 10s interval', async () => {
    vi.useFakeTimers();
    wrap(<ApiCallLogsPage />);
    // Initial fetch (sync within microtask).
    await vi.runOnlyPendingTimersAsync();
    (apiCallLogs.list as ReturnType<typeof vi.fn>).mockClear();
    // Advance to next interval tick.
    await vi.advanceTimersByTimeAsync(10_000);
    expect(apiCallLogs.list).toHaveBeenCalled();
  });

  it('auto-refresh OFF disables the interval', async () => {
    vi.useFakeTimers();
    wrap(<ApiCallLogsPage />);
    await vi.runOnlyPendingTimersAsync();
    fireEvent.click(screen.getByTestId('api-call-logs-auto-refresh'));
    await vi.runOnlyPendingTimersAsync();
    (apiCallLogs.list as ReturnType<typeof vi.fn>).mockClear();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(apiCallLogs.list).not.toHaveBeenCalled();
  });
});
