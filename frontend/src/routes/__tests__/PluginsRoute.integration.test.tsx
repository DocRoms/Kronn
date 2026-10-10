// The Plugins page behind the real router and the real dashboard shell, with
// only the API boundary simulated: what a direct link to a plugin does while
// its list is still loading, once it has loaded, and when it names nothing.
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import { withDashboardRoutes } from '../../test/routerWrapper';
import { tourAlreadyTaken } from '../../test/tour';
import type { McpConfigDisplay, McpOverview } from '../../types/generated';

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })),
}));
vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { mcps as mcpsApi, pages as pagesApi } from '../../lib/api';
import { Dashboard } from '../../pages/Dashboard';

const config = (id: string, label: string): McpConfigDisplay => ({
  id, server_id: 'github', server_name: 'GitHub', label,
  env_keys: [], env_masked: [], args_override: null, is_global: false, include_general: true,
  config_hash: 'abc123', project_ids: [], project_names: [], secrets_broken: false, host_sync: 'None',
  preferred_interface: 'mcp', interfaces: ['mcp'], effective_kind: 'mcp', effective_preferred_interface: 'mcp',
  credential_source: 'stored', last_probes: [],
});

const overview: McpOverview = {
  servers: [{ id: 'github', name: 'GitHub', description: 'GitHub server', transport: { Stdio: { command: 'npx', args: [] } }, source: 'Registry' }],
  configs: [config('c1', 'GitHub Main'), config('c2', 'GitHub Docs')],
  customized_contexts: [], incompatibilities: [], incomplete_configs: [],
};

async function renderDashboard(initialPath: string) {
  await act(async () => {
    render(<I18nProvider>{withDashboardRoutes(<Dashboard onReset={vi.fn()} />, initialPath)}</I18nProvider>);
  });
}

const openDetail = () => document.querySelector('.mcp-detail-inline');

beforeEach(() => {
  localStorage.clear();
  tourAlreadyTaken();
  vi.mocked(pagesApi.capability).mockResolvedValue({ activated: false, activated_at: null });
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.clearAllMocks();
});

describe('Plugins route — a direct link to a plugin', () => {
  it('keeps its target while the list is still loading, then opens it', async () => {
    let deliver!: (value: McpOverview) => void;
    vi.mocked(mcpsApi.overview).mockReturnValue(new Promise(resolve => { deliver = resolve; }));
    await renderDashboard('/plugins/c1');

    expect(window.location.pathname).toBe('/plugins/c1');
    expect(openDetail()).toBeNull();

    await act(async () => deliver(overview));

    expect(window.location.pathname).toBe('/plugins/c1');
    await waitFor(() => expect(openDetail()).not.toBeNull());
    expect(screen.getByRole('button', { name: 'GitHub Main — Voir les détails' })).toHaveAttribute('aria-current', 'true');
  });

  it('opens the plugin when the list is already there, as on a reload', async () => {
    vi.mocked(mcpsApi.overview).mockResolvedValue(overview);
    await renderDashboard('/plugins/c2');

    await waitFor(() => expect(openDetail()).not.toBeNull());
    expect(window.location.pathname).toBe('/plugins/c2');
    expect(screen.getByRole('button', { name: 'GitHub Docs — Voir les détails' })).toHaveAttribute('aria-current', 'true');
  });

  it('lets go of a plugin the loaded list does not know, in place of the address', async () => {
    vi.mocked(mcpsApi.overview).mockResolvedValue(overview);
    const depth = window.history.length;
    await renderDashboard('/plugins/gone');

    await waitFor(() => expect(window.location.pathname).toBe('/plugins'));
    expect(window.history.length).toBe(depth);
    expect(openDetail()).toBeNull();
  });
});
