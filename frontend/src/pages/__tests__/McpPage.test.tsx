// Note: assertions use French strings because the default UI locale is 'fr'.
// If the default locale changes, these assertions must be updated.
import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, cleanup, fireEvent, act, within } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { I18nProvider } from '../../lib/I18nContext';

// Mock API
vi.mock('../../lib/api', () => ({
  mcps: {
    overview: vi.fn().mockResolvedValue({ servers: [], configs: [], project_links: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] }),
    registry: vi.fn().mockResolvedValue([]),
    refresh: vi.fn(),
    createConfig: vi.fn(),
    updateConfig: vi.fn(),
    probeConfig: vi.fn(),
    testAll: vi.fn(),
    updateCustomSpec: vi.fn(),
    cleanupOrphanEnv: vi.fn(),
    deleteConfig: vi.fn(),
    setConfigProjects: vi.fn(),
    revealSecrets: vi.fn(),
    listContexts: vi.fn(),
    getContext: vi.fn(),
    updateContext: vi.fn(),
  },
  config: {
    getUiLanguage: vi.fn().mockResolvedValue('fr'),
    saveUiLanguage: vi.fn().mockResolvedValue(undefined),
  },
  apiCallLogs: {
    drift: vi.fn().mockResolvedValue([]),
  },
}));

import { McpPage } from '../McpPage';
import { mcps as mcpsApi } from '../../lib/api';
import type { McpOverview, McpConfigDisplay, McpServer, McpDefinition, Project, AgentType, McpProbeResponse } from '../../types/generated';

// Use fake timers to prevent the setTimeout in handleAddDuplicateConfig (50ms
// scroll animation) from leaking across tests and causing timeout issues.
beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
  localStorage.clear();
  vi.stubGlobal('confirm', () => true);
  vi.stubGlobal('matchMedia', vi.fn().mockImplementation((query: string) => ({ matches: false, media: query, addEventListener: vi.fn(), removeEventListener: vi.fn() })));
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  cleanup();
});

const noop = () => {};

const makeServer = (id: string, name: string): McpServer => ({
  id,
  name,
  description: `${name} server`,
  transport: { Stdio: { command: 'npx', args: ['-y', `@mcp/${id}`] } },
  source: 'Registry',
});

const makeConfig = (id: string, serverId: string, serverName: string, opts?: Partial<McpConfigDisplay>): McpConfigDisplay => ({
  id,
  server_id: serverId,
  server_name: serverName,
  label: opts?.label ?? serverName,
  env_keys: opts?.env_keys ?? [],
  env_masked: [],
  args_override: null,
  is_global: opts?.is_global ?? false,
  include_general: opts?.include_general ?? true,
  config_hash: 'abc123',
  project_ids: opts?.project_ids ?? [],
  project_names: opts?.project_names ?? [],
  secrets_broken: opts?.secrets_broken ?? false,
  host_sync: opts?.host_sync ?? 'None',
  preferred_interface: opts?.preferred_interface ?? 'mcp',
  registry_drift: opts?.registry_drift,
  // KT-828 — these mirror what the backend computes once
  // (`registry::effective_plugin_kind` / `available_plugin_interfaces` /
  // `credential_source`); tests that exercise a specific classification
  // override them explicitly instead of relying on a derivation from
  // `mcpRegistry`/`mcpOverview.servers` the components no longer do.
  interfaces: opts?.interfaces ?? ['mcp'],
  effective_kind: opts?.effective_kind ?? 'mcp',
  effective_preferred_interface: opts?.effective_preferred_interface ?? opts?.preferred_interface ?? 'mcp',
  credential_source: opts?.credential_source ?? 'stored',
  last_probes: opts?.last_probes ?? [],
});

const makeProject = (id: string, name: string): Project => ({
  id,
  name,
  path: `/repos/${name}`,
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
});

const wrap = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);
const getAddPluginButton = () => {
  const button = document.querySelector<HTMLButtonElement>('[data-tour-id="add-plugin-btn"]');
  if (!button) throw new Error('Add plugin button not found');
  return button;
};
const openPlugin = (label: string) => {
  fireEvent.click(screen.getAllByRole('button', { name: `${label} — Voir les détails` })[0]);
};
const openFilters = () => {
  const trigger = screen.getByRole('button', { name: 'Filtrer les plugins' });
  if (trigger.getAttribute('aria-expanded') !== 'true') fireEvent.click(trigger);
};
const setFilter = (name: string, value: string) => {
  openFilters();
  fireEvent.change(screen.getByRole('combobox', { name }), { target: { value } });
};
const listedConfigIds = (container: HTMLElement) => Array.from(
  container.querySelectorAll<HTMLElement>('.mcp-sidebar-plugin-row'),
  row => row.dataset.configId,
);

describe('McpPage', () => {
  it('keeps the built-in plugin visible when no configurable plugin exists', () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    expect(screen.getByTestId('mcp-kronn-internal-card')).toHaveTextContent('Intégré');
    // The sidebar's own empty-filter message (KT-855: Plugins now has its own
    // "mcp.filter.empty" copy, distinct from Automation's) is expected here —
    // the static built-in card below the shell stays the one thing to check.
  });

  it('flags an endpoint that keeps failing, and says which kind of fault it is', async () => {
    // A spec is written once and never re-checked. When it is wrong — or the
    // API moves — the calls just keep failing and nothing surfaces it. The two
    // cases need different words: an endpoint that NEVER answered is a spec
    // that was wrong, one that also succeeds is a call that is.
    const { apiCallLogs } = await import('../../lib/api');
    vi.mocked(apiCallLogs.drift).mockResolvedValueOnce([
      {
        plugin_slug: 'github',
        endpoint_path: '/v1/gone',
        http_status: 404,
        failures: 9,
        successes: 0,
        last_seen: '2026-08-20T10:00:00Z',
      },
    ]);
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub')],
      configs: [makeConfig('c1', 'github', 'GitHub')],
      customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    // Fake timers here: findBy* would poll on a clock that never advances.
    await act(async () => {});

    const badge = screen.getByTestId('mcp-endpoint-drift-github');
    expect(badge.getAttribute('title')).toContain('/v1/gone');
    expect(badge.getAttribute('title')).toContain('HTTP 404');
    // "never succeeded" wording, not the "parameters do not match" one.
    expect(badge.getAttribute('title')).toContain('aucun n\u2019a jamais abouti');
  });

  it('stays quiet when no endpoint is drifting', async () => {
    // The badge must earn attention: if it appears on healthy plugins nobody
    // reads it when it matters.
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub')],
      configs: [makeConfig('c1', 'github', 'GitHub')],
      customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    await act(async () => {});
    expect(screen.queryByTestId('mcp-endpoint-drift-github')).toBeNull();
  });

  it('renders server names as group headers', () => {
    const servers = [makeServer('github', 'GitHub'), makeServer('slack', 'Slack')];
    const configs = [
      makeConfig('c1', 'github', 'GitHub'),
      makeConfig('c2', 'slack', 'Slack'),
    ];
    const overview: McpOverview = { servers, configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    expect(screen.getAllByText('GitHub')).not.toHaveLength(0);
    expect(screen.getAllByText('Slack')).not.toHaveLength(0);
  });

  it('renders config labels as cards', () => {
    const configs = [
      makeConfig('c1', 'github', 'GitHub', { label: 'GitHub Main' }),
      makeConfig('c2', 'github', 'GitHub', { label: 'GitHub Secondary' }),
    ];
    const overview: McpOverview = { servers: [makeServer('github', 'GitHub')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Cards are always visible (no accordion)
    expect(container.textContent).toContain('GitHub Main');
    expect(container.textContent).toContain('GitHub Secondary');
  });

  it('shows the project access overview until a plugin is selected', () => {
    const config = makeConfig('c1', 'github', 'GitHub');
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub')],
      configs: [config],
      customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    expect(screen.getByRole('heading', { name: 'Ce que reçoivent les agents de tous les projets' }))
      .toBeInTheDocument();
    expect(screen.getByText('Chargés dans l’agent')).toBeInTheDocument();
  });

  it('shows project counters and per-access health with the last test timestamp', () => {
    const config = makeConfig('c1', 'github', 'GitHub', {
      label: 'GitHub hybrid',
      include_general: false,
      project_ids: ['p1'],
      project_names: ['Alpha'],
      interfaces: ['mcp', 'api', 'cli'],
      effective_kind: 'hybrid',
      effective_preferred_interface: 'api',
      last_probes: [
        { access: 'mcp', ok: true, code: 'ok', summary: 'raw backend text', tested_at: '2026-09-27T12:00:00Z' },
        { access: 'api', ok: false, code: 'unauthorized', summary: 'raw backend text', tested_at: '2026-09-27T12:01:00Z' },
      ],
    });
    const other = makeConfig('c2', 'slack', 'Slack', {
      include_general: false, project_ids: ['p2'], project_names: ['Beta'],
    });
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub'), makeServer('slack', 'Slack')], configs: [config, other], customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    const { container } = wrap(<McpPage projects={[makeProject('p1', 'Alpha'), makeProject('p2', 'Beta')]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    setFilter('Projet', 'p1');
    expect(listedConfigIds(container)).toEqual(['c1']);
    expect(screen.getByRole('heading', { name: 'Ce que reçoivent les agents de Alpha' })).toBeInTheDocument();
    expect(screen.getByLabelText('Résumé de santé des plugins')).toHaveTextContent('1 plugins');
    expect(screen.getByLabelText('Résumé de santé des plugins')).toHaveTextContent('1 en erreur');
    expect(container.querySelector('.mcp-access-lane[data-access="mcp"]')).toHaveTextContent('Accès vérifié.');
    expect(container.querySelector('.mcp-access-lane[data-access="api"]')).toHaveTextContent('L’authentification a été refusée.');
    expect(container.querySelector('.mcp-access-lane[data-access="api"] time')).toHaveAttribute('datetime', '2026-09-27T12:01:00Z');
    expect(container.querySelector('.mcp-access-lane[data-access="cli"]')).toHaveTextContent('Cet accès n’a pas encore été testé.');
    expect(screen.getByText('Utilisé par l’agent')).toBeInTheDocument();
    expect(container).not.toHaveTextContent('raw backend text');
  });

  it('tests only the selected project, and uses the bulk endpoint for all plugins', async () => {
    const alpha = makeConfig('alpha-config', 'alpha-server', 'Alpha plugin', {
      include_general: false, project_ids: ['p1'], project_names: ['Alpha'],
    });
    const beta = makeConfig('beta-config', 'beta-server', 'Beta plugin', {
      include_general: false, project_ids: ['p2'], project_names: ['Beta'],
    });
    const overview: McpOverview = {
      servers: [makeServer('alpha-server', 'Alpha plugin'), makeServer('beta-server', 'Beta plugin')],
      configs: [alpha, beta], customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    vi.mocked(mcpsApi.probeConfig).mockResolvedValue({
      server_id: 'alpha-server', ready: true, checks: [{ id: 'mcp', label: 'MCP', ok: true, required: true, detail: 'raw', code: 'ok' }],
    });
    vi.mocked(mcpsApi.testAll).mockResolvedValue({ results: [] });
    wrap(<McpPage projects={[makeProject('p1', 'Alpha'), makeProject('p2', 'Beta')]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    expect(screen.getByTestId('mcp-test-project')).toHaveTextContent('Tester tout');

    setFilter('Projet', 'p1');
    expect(screen.getByTestId('mcp-test-project')).toHaveTextContent('Tester le projet');
    await act(async () => { fireEvent.click(screen.getByTestId('mcp-test-project')); });
    expect(mcpsApi.probeConfig).toHaveBeenCalledTimes(1);
    expect(mcpsApi.probeConfig).toHaveBeenCalledWith('alpha-config');

    setFilter('Projet', '__all__');
    expect(screen.getByTestId('mcp-test-project')).toHaveTextContent('Tester tout');
    await act(async () => { fireEvent.click(screen.getByTestId('mcp-test-project')); });
    expect(mcpsApi.testAll).toHaveBeenCalledTimes(1);
  });

  it('previews a rescan before applying it', async () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    vi.mocked(mcpsApi.refresh)
      .mockResolvedValueOnce({ dry_run: true, configs_created: 2, configs_merged: 1, configs_deleted: 0, projects_affected: 1, overview })
      .mockResolvedValueOnce({ dry_run: false, configs_created: 2, configs_merged: 1, configs_deleted: 0, projects_affected: 1, projects_rewritten: 1, overview });
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    await act(async () => { fireEvent.click(screen.getByTestId('mcp-rescan-preview-button')); });
    expect(mcpsApi.refresh).toHaveBeenCalledWith(true);
    expect(screen.getByRole('region', { name: 'Aperçu du rescan' })).toHaveTextContent('2');
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Appliquer le rescan' })); });
    expect(mcpsApi.refresh).toHaveBeenLastCalledWith(false);
  });

  it('surfaces a label save failure and keeps the editor open', async () => {
    const config = makeConfig('c1', 'github', 'GitHub');
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub')], configs: [config], customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    vi.mocked(mcpsApi.updateConfig).mockRejectedValueOnce(new Error('save unavailable'));
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    openPlugin('GitHub');
    fireEvent.click(screen.getByRole('heading', { name: /GitHub/ }));
    const input = screen.getByDisplayValue('GitHub');
    fireEvent.change(input, { target: { value: 'GitHub renamed' } });
    await act(async () => { fireEvent.blur(input); });

    expect(screen.getByText(/save unavailable/)).toBeInTheDocument();
    expect(screen.getByDisplayValue('GitHub renamed')).toBeInTheDocument();
  });

  it('covers the narrow rail, empty overview, selection, and open menu on the real Plugins page', async () => {
    vi.stubGlobal('matchMedia', vi.fn().mockImplementation((query: string) => ({ matches: true, media: query, addEventListener: vi.fn(), removeEventListener: vi.fn() })));
    const emptyOverview: McpOverview = {
      servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    const view = wrap(<McpPage projects={[]} mcpOverview={emptyOverview} mcpRegistry={[]} refetchMcps={noop} />);
    expect(screen.getByTestId('mcp-kronn-internal-card')).toBeInTheDocument();

    const config = makeConfig('c1', 'github', 'GitHub', { label: 'GitHub Main' });
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub')], configs: [config], customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    view.rerender(<I18nProvider><McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} /></I18nProvider>);
    fireEvent.click(screen.getByRole('button', { name: 'Plus d’actions' }));
    expect(screen.getByRole('menu')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Fermer la liste' }));
    expect(screen.getByRole('button', { name: 'Ouvrir la liste' })).toHaveClass('collection-shell-open');
    fireEvent.click(screen.getByRole('button', { name: 'Ouvrir la liste' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Sélection multiple' }));
    fireEvent.click(screen.getByRole('checkbox', { name: /GitHub Main.*sélectionné/ }));
    expect(screen.getByRole('checkbox', { name: /GitHub Main.*sélectionné/ })).toBeChecked();
  });

  it('uses the shared title, favorites, collapse, and bulk-delete flow', async () => {
    const configs = [
      makeConfig('c1', 'github', 'GitHub', { label: 'GitHub Main' }),
      makeConfig('c2', 'slack', 'Slack', { label: 'Slack Main' }),
    ];
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub'), makeServer('slack', 'Slack')],
      configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={vi.fn()} />);
    await act(async () => {});
    expect(screen.getByText('Plugins').closest('.collection-shell-title')).toHaveTextContent('Plugins · 2');
    const addPluginButton = document.querySelector('[data-tour-id="add-plugin-btn"]');
    expect(addPluginButton).toHaveClass('collection-shell-primary-action');
    expect(addPluginButton).toHaveAccessibleName('Ajouter');
    expect(addPluginButton).toHaveTextContent(/^$/);
    fireEvent.click(screen.getByRole('button', { name: 'Ajouter aux favoris · GitHub Main' }));
    expect(localStorage.getItem('kronn:collection-favorites:plugins')).toContain('c1');
    const favoritesSection = container.querySelector('.disc-sidebar-favorites') as HTMLElement;
    expect(favoritesSection).toBeInTheDocument();
    expect(within(favoritesSection).getByRole('button', { name: 'GitHub Main — Voir les détails' })).toBeInTheDocument();
    const favoritesHeader = within(favoritesSection).getByRole('button', { name: 'Favoris 1' });
    expect(favoritesHeader).toHaveClass('collection-favorites-header');
    fireEvent.click(favoritesHeader);
    expect(favoritesHeader).toHaveAttribute('aria-expanded', 'false');
    expect(within(favoritesSection).queryByRole('button', { name: 'GitHub Main — Voir les détails' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Plus d’actions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Sélection multiple' }));
    fireEvent.click(screen.getByRole('checkbox', { name: /GitHub Main.*sélectionné/ }));
    fireEvent.click(screen.getByRole('checkbox', { name: /Slack Main.*sélectionné/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Supprimer la sélection' }));
    await act(async () => {});
    expect(mcpsApi.deleteConfig).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByRole('button', { name: 'Fermer la liste' }));
    expect(screen.queryByRole('complementary', { name: 'Plugins' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Ouvrir la liste' })).toHaveClass('collection-shell-sidebar-rail');
  });

  it('keeps restored favorites while the plugin overview is still loading', () => {
    localStorage.setItem('kronn:collection-favorites:plugins', JSON.stringify(['c1']));
    const emptyOverview: McpOverview = {
      servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    const loadedOverview: McpOverview = {
      servers: [makeServer('github', 'GitHub')],
      configs: [makeConfig('c1', 'github', 'GitHub', { label: 'GitHub Main' })],
      customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    const { rerender } = wrap(<McpPage projects={[]} mcpOverview={emptyOverview} mcpRegistry={[]} refetchMcps={noop} favoritesReady={false} />);

    expect(JSON.parse(localStorage.getItem('kronn:collection-favorites:plugins') ?? '[]')).toEqual(['c1']);

    rerender(<I18nProvider><McpPage projects={[]} mcpOverview={loadedOverview} mcpRegistry={[]} refetchMcps={noop} favoritesReady /></I18nProvider>);
    const favoritesSection = document.querySelector('.disc-sidebar-favorites') as HTMLElement;
    expect(within(favoritesSection).getByRole('button', { name: 'GitHub Main — Voir les détails' })).toBeInTheDocument();
  });

  it('uses the shared row menu and complete keyboard footer', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    const config = makeConfig('c1', 'github', 'GitHub', { label: 'GitHub Main' });
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub')],
      configs: [config], customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    const refetchMcps = vi.fn();
    const { container } = wrap(
      <McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={refetchMcps} />,
    );
    const row = screen.getByRole('button', { name: 'GitHub Main — Voir les détails' }).closest('.disc-item') as HTMLElement;
    fireEvent.click(within(row).getByRole('button', { name: 'Plus d’actions · GitHub Main' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copier l’ID' }));
    await act(async () => {});
    expect(writeText).toHaveBeenCalledWith('c1');
    expect(screen.queryByRole('menuitem', { name: 'Supprimer cette config' })).toBeNull();
    expect(mcpsApi.deleteConfig).not.toHaveBeenCalled();

    const footer = container.querySelector('.disc-sidebar-footer') as HTMLElement;
    expect(footer).toHaveTextContent('Bibliothèque de plugins');
    expect(within(footer).getByText('↑↓')).toBeInTheDocument();
    expect(within(footer).getByText('/')).toBeInTheDocument();
  });

  it('lists every plugin once, Favorites first, with no project tree', () => {
    localStorage.setItem('kronn:collection-favorites:plugins', JSON.stringify(['general-config']));
    const configs = [
      makeConfig('global-config', 'global', 'Global plugin', {
        is_global: true,
        include_general: false,
        project_ids: ['p1', 'p2', 'p3'],
        project_names: ['Alpha', 'Beta', 'Gamma'],
      }),
      makeConfig('general-config', 'general', 'General plugin'),
      makeConfig('shared-config', 'shared', 'Shared plugin', {
        include_general: false,
        project_ids: ['p1', 'p2'],
        project_names: ['Alpha', 'Beta'],
      }),
      makeConfig('orphan-config', 'orphan', 'Orphan plugin', {
        include_general: false,
      }),
    ];
    const overview: McpOverview = {
      servers: configs.map(config => makeServer(config.server_id, config.server_name)),
      configs,
      customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    const projects = [makeProject('p1', 'Alpha'), makeProject('p2', 'Beta'), makeProject('p3', 'Gamma')];
    const first = wrap(<McpPage projects={projects} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    const list = first.container.querySelector('.mcp-sidebar-items') as HTMLElement;
    const topSections = list.querySelectorAll(':scope > .disc-sidebar-section');
    expect(topSections).toHaveLength(2);
    expect(topSections[0]).toHaveClass('disc-sidebar-favorites');
    expect(topSections[1]).toHaveClass('mcp-sidebar-all');
    expect(within(topSections[0] as HTMLElement).getByRole('button', { name: 'General plugin — Voir les détails' })).toBeInTheDocument();

    // A global plugin visible to three projects is still one row, and so is every other plugin.
    for (const label of ['Global plugin', 'General plugin', 'Shared plugin', 'Orphan plugin']) {
      expect(screen.getAllByRole('button', { name: `${label} — Voir les détails` })).toHaveLength(1);
    }
    expect(listedConfigIds(first.container)).toHaveLength(4);
    const globalRow = list.querySelector('[data-config-id="global-config"]') as HTMLElement;
    expect(within(globalRow).getByText('Tous les projets · 3 projets')).toBeInTheDocument();

    // No per-project tree, group or selector any more.
    expect(list.querySelector('.disc-sidebar-projects, .disc-project-tree')).toBeNull();
    expect(document.querySelector('.mcp-project-selector')).toBeNull();
    expect(screen.queryByRole('button', { name: /Sans projet/ })).toBeNull();
    expect(screen.queryByRole('button', { name: /Alpha/ })).toBeNull();

    const allHeader = screen.getByRole('button', { name: /Tous les plugins/ });
    fireEvent.click(allHeader);
    expect(allHeader).toHaveAttribute('aria-expanded', 'false');
    expect(within(topSections[1] as HTMLElement).queryByRole('button', { name: 'Shared plugin — Voir les détails' })).toBeNull();
    expect(JSON.parse(localStorage.getItem('kronn:mcpCollapsedGroups') ?? '[]')).toEqual(['all']);

    first.unmount();
    wrap(<McpPage projects={projects} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    const restoredHeader = screen.getByRole('button', { name: /Tous les plugins/ });
    expect(restoredHeader).toHaveAttribute('aria-expanded', 'false');
    fireEvent.change(screen.getByRole('textbox', { name: 'Rechercher un plugin ou un projet...' }), {
      target: { value: 'Shared plugin' },
    });
    expect(restoredHeader).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByRole('button', { name: 'Shared plugin — Voir les détails' })).toBeInTheDocument();
  });

  it('keeps favorite and recently tested plugins out of the full list', () => {
    localStorage.setItem('kronn:collection-favorites:plugins', JSON.stringify(['fav-config']));
    const probe = [{ access: 'mcp' as const, ok: true, code: 'ok' as const, summary: '', tested_at: '2026-09-27T12:00:00Z' }];
    const configs = [
      makeConfig('fav-config', 'fav', 'Fav plugin', { last_probes: probe }),
      makeConfig('tested-config', 'tested', 'Tested plugin', { last_probes: probe }),
      makeConfig('plain-config', 'plain', 'Plain plugin'),
    ];
    const overview: McpOverview = {
      servers: configs.map(config => makeServer(config.server_id, config.server_name)),
      configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    const idsIn = (selector: string) => Array.from(
      container.querySelectorAll<HTMLElement>(`${selector} .mcp-sidebar-plugin-row`),
      row => row.dataset.configId,
    );
    expect(idsIn('.disc-sidebar-favorites')).toEqual(['fav-config']);
    expect(idsIn('.mcp-sidebar-recent')).toEqual(['tested-config']);
    expect(idsIn('.mcp-sidebar-all')).toEqual(['plain-config']);
    expect(listedConfigIds(container)).toHaveLength(3);
  });

  it('filters by project, health and local sync, and clears every filter at once', () => {
    const alpha = makeConfig('alpha-config', 'alpha', 'Alpha plugin', {
      include_general: false, project_ids: ['p1'], project_names: ['Alpha'],
      last_probes: [{ access: 'mcp', ok: true, code: 'ok', summary: '', tested_at: '2026-09-27T12:00:00Z' }],
    });
    const beta = makeConfig('beta-config', 'beta', 'Beta plugin', {
      include_general: false, project_ids: ['p2'], project_names: ['Beta'], secrets_broken: true,
    });
    const shared = makeConfig('shared-config', 'shared', 'Shared plugin', {
      is_global: true, include_general: false, host_sync: 'GlobalOnly',
    });
    const overview: McpOverview = {
      servers: [makeServer('alpha', 'Alpha plugin'), makeServer('beta', 'Beta plugin'), makeServer('shared', 'Shared plugin')],
      configs: [alpha, beta, shared], customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    const { container } = wrap(<McpPage projects={[makeProject('p1', 'Alpha'), makeProject('p2', 'Beta')]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    const ids = () => listedConfigIds(container).sort();
    const trigger = screen.getByRole('button', { name: 'Filtrer les plugins' });
    const panelClear = () => within(document.getElementById('mcp-filter-options') as HTMLElement)
      .queryByRole('button', { name: 'Effacer les filtres' });

    expect(ids()).toEqual(['alpha-config', 'beta-config', 'shared-config']);
    expect(trigger).toHaveAttribute('data-active', 'false');

    // The panel offers Type, Project, Health and Local sync.
    openFilters();
    expect(screen.getByRole('combobox', { name: 'Filtrer les plugins par type' })).toBeInTheDocument();
    const projectOptions = within(screen.getByRole('combobox', { name: 'Projet' })).getAllByRole('option');
    expect(projectOptions.map(option => option.textContent)).toEqual(['Tous les projets', 'Sans projet', 'Alpha', 'Beta']);
    expect(within(screen.getByRole('combobox', { name: 'Santé' })).getAllByRole('option')).toHaveLength(4);
    expect(screen.getByRole('combobox', { name: 'Synchro locale' })).toBeInTheDocument();
    expect(panelClear()).toBeNull();

    // Project: restricts the list, the summary and the test button.
    setFilter('Projet', 'p2');
    expect(ids()).toEqual(['beta-config', 'shared-config']);
    expect(screen.getByRole('heading', { name: 'Ce que reçoivent les agents de Beta' })).toBeInTheDocument();
    expect(screen.getByLabelText('Résumé de santé des plugins')).toHaveTextContent('2 plugins');
    expect(screen.getByTestId('mcp-test-project')).toHaveTextContent('Tester le projet');
    expect(localStorage.getItem('kronn:mcpSelectedProject')).toBe('p2');

    // Health, then local sync, narrow it down further.
    setFilter('Santé', 'error');
    expect(ids()).toEqual(['beta-config']);
    expect(screen.getByLabelText('Résumé de santé des plugins')).toHaveTextContent('1 en erreur');
    setFilter('Synchro locale', 'local');
    expect(ids()).toEqual([]);
    expect(screen.getByText('Aucun plugin ne correspond à ce filtre.')).toBeInTheDocument();

    // The filter icon stays active while the panel is closed.
    fireEvent.click(trigger);
    expect(trigger).toHaveAttribute('aria-expanded', 'false');
    expect(trigger).toHaveAttribute('data-active', 'true');

    // Clearing from the empty state resets every filter.
    fireEvent.click(screen.getByRole('button', { name: 'Effacer les filtres' }));
    expect(ids()).toEqual(['alpha-config', 'beta-config', 'shared-config']);
    expect(trigger).toHaveAttribute('data-active', 'false');
    expect(screen.getByTestId('mcp-test-project')).toHaveTextContent('Tester tout');
    expect(localStorage.getItem('kronn:mcpSelectedProject')).toBe('__all__');
    openFilters();
    expect(screen.getByRole('combobox', { name: 'Filtrer les plugins par type' })).toHaveValue('all');
    expect(screen.getByRole('combobox', { name: 'Projet' })).toHaveValue('__all__');
    expect(screen.getByRole('combobox', { name: 'Santé' })).toHaveValue('all');
    expect(screen.getByRole('combobox', { name: 'Synchro locale' })).toHaveValue('all');

    // ... and so does the panel's own button.
    setFilter('Projet', 'p1');
    setFilter('Santé', 'ok');
    expect(ids()).toEqual(['alpha-config']);
    fireEvent.click(panelClear() as HTMLElement);
    expect(ids()).toEqual(['alpha-config', 'beta-config', 'shared-config']);
    expect(screen.getByRole('combobox', { name: 'Projet' })).toHaveValue('__all__');
    expect(screen.getByRole('combobox', { name: 'Santé' })).toHaveValue('all');
    expect(panelClear()).toBeNull();
  });

  it('falls back to all projects when the filtered project no longer exists', () => {
    const configs = [
      makeConfig('alpha-config', 'alpha', 'Alpha plugin', { include_general: false, project_ids: ['p1'], project_names: ['Alpha'] }),
      makeConfig('beta-config', 'beta', 'Beta plugin', { include_general: false, project_ids: ['p2'], project_names: ['Beta'] }),
    ];
    const overview: McpOverview = {
      servers: [makeServer('alpha', 'Alpha plugin'), makeServer('beta', 'Beta plugin')],
      configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    const view = wrap(<McpPage projects={[makeProject('p1', 'Alpha'), makeProject('p2', 'Beta')]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    setFilter('Projet', 'p1');
    expect(listedConfigIds(view.container)).toEqual(['alpha-config']);

    view.rerender(<I18nProvider>
      <McpPage projects={[makeProject('p2', 'Beta')]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />
    </I18nProvider>);
    expect(listedConfigIds(view.container).sort()).toEqual(['alpha-config', 'beta-config']);
    expect(screen.getByTestId('mcp-test-project')).toHaveTextContent('Tester tout');
  });

  it('opens plugin creation in an accessible modal and restores focus when it closes', () => {
    const overview: McpOverview = {
      servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    const trigger = getAddPluginButton();

    fireEvent.click(trigger);
    const dialog = screen.getByRole('dialog', { name: 'Ajouter un plugin' });
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    expect(dialog).toHaveClass('mcp-add-modal');
    expect(dialog.parentElement).toHaveClass('mcp-add-modal-backdrop');
    expect(dialog.querySelector('input')).toHaveFocus();

    fireEvent.keyDown(dialog, { key: 'Escape' });
    expect(screen.queryByRole('dialog', { name: 'Ajouter un plugin' })).toBeNull();
    expect(trigger).toHaveFocus();

    fireEvent.click(trigger);
    fireEvent.mouseDown(screen.getByTestId('mcp-add-modal-backdrop'));
    expect(screen.queryByRole('dialog', { name: 'Ajouter un plugin' })).toBeNull();
    expect(trigger).toHaveFocus();
  });

  it('uses compact Discussion-style identity, metadata, and actions on plugin rows', () => {
    const configs = [
      makeConfig('c1', 'github', 'GitHub', {
        label: 'GitHub Main',
        env_keys: ['GITHUB_TOKEN'],
        project_ids: ['p1'],
        project_names: ['my-app'],
        host_sync: 'GlobalOnly',
      }),
    ];
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub')],
      configs,
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };
    wrap(
      <McpPage projects={[makeProject('p1', 'my-app')]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />,
    );

    const openRow = screen.getAllByRole('button', { name: 'GitHub Main — Voir les détails' })[0];
    const pluginRow = openRow.closest('.mcp-sidebar-plugin-row');
    expect(pluginRow).toHaveClass('disc-item');
    expect(pluginRow).toHaveAttribute('data-kind', 'mcp');
    expect(pluginRow).toHaveTextContent('GitHub Main');
    expect(pluginRow).toHaveTextContent('GitHub · Général · 1 projet');
    expect(pluginRow).toHaveTextContent('MCP');
    expect(openRow).toHaveClass('collection-shell-row-button', 'disc-item-open');
    expect(within(pluginRow as HTMLElement).getByRole('button', { name: 'Ajouter aux favoris · GitHub Main' })).toBeInTheDocument();
  });

  it('opens plugin details in the shared collection detail pane and renders the real probe result', async () => {
    const server = makeServer('mcp-fastly', 'Fastly');
    const config = makeConfig('fastly-config', 'mcp-fastly', 'Fastly');
    const overview: McpOverview = {
      servers: [server],
      configs: [config],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };
    const probeResponse: McpProbeResponse = {
      server_id: 'mcp-fastly',
      ready: true,
      checks: [
        { id: 'cli', label: 'Fastly CLI', ok: true, required: true, detail: 'Fastly CLI version 15.4.0', code: 'ok' },
        { id: 'api', label: 'Fastly API', ok: true, required: true, detail: 'Authenticated API request succeeded', code: 'ok' },
        { id: 'mcp', label: 'Fastly MCP', ok: false, required: false, detail: 'Optional exploratory MCP is unavailable', code: 'cli_missing' },
      ],
    };
    let resolveProbe!: (response: McpProbeResponse) => void;
    const probePromise = new Promise<McpProbeResponse>((resolve) => {
      resolveProbe = resolve;
    });
    (mcpsApi.probeConfig as ReturnType<typeof vi.fn>).mockReturnValueOnce(probePromise);

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByRole('button', { name: 'Fastly — Voir les détails' }));

    expect(document.querySelector('.collection-shell')).not.toBeNull();
    expect(document.querySelector('.mcp-detail-inline')).not.toBeNull();
    expect(document.querySelector('.mcp-modal-overlay')).toBeNull();
    expect(document.querySelector('[data-testid="mcp-plugin-probe"]')).not.toBeNull();

    const probeButton = screen.getByTestId('mcp-probe-button');
    fireEvent.click(probeButton);
    fireEvent.click(probeButton);
    expect(mcpsApi.probeConfig).toHaveBeenCalledTimes(1);

    await act(async () => resolveProbe(probeResponse));

    expect(mcpsApi.probeConfig).toHaveBeenCalledWith('fastly-config');
    expect(screen.getByTestId('mcp-probe-status')).toHaveTextContent('Prêt');
    expect(screen.getAllByText('Accès vérifié.').length).toBeGreaterThanOrEqual(2);
    expect(screen.getByText('Optionnel')).toBeTruthy();
    expect(screen.getAllByText('Le CLI requis n’est pas installé.').length).toBeGreaterThan(0);
  });

  it('surfaces registry drift with stored and expected key names', () => {
    const server = makeServer('api-chartbeat', 'Chartbeat');
    server.transport = 'ApiOnly';
    const config = makeConfig('chartbeat-config', 'api-chartbeat', 'Chartbeat', {
      env_keys: ['CHARTBEAT_API_KEY', 'LEGACY_HOST'],
      registry_drift: {
        orphaned: false,
        stored_env_keys: ['CHARTBEAT_API_KEY', 'LEGACY_HOST'],
        expected_env_keys: ['CHARTBEAT_API_KEY', 'CHARTBEAT_HOST'],
        unexpected_env_keys: ['LEGACY_HOST'],
        missing_env_keys: ['CHARTBEAT_HOST'],
      },
    });
    const overview: McpOverview = {
      servers: [server], configs: [config], customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    expect(screen.getByRole('button', { name: 'Chartbeat — Voir les détails' })).toHaveTextContent('Config obsolète');
    fireEvent.click(screen.getByRole('button', { name: 'Chartbeat — Voir les détails' }));

    const warning = screen.getByTestId('mcp-registry-drift');
    expect(warning).toHaveTextContent('Clés de configuration désynchronisées');
    expect(warning).toHaveTextContent('CHARTBEAT_API_KEY, LEGACY_HOST');
    expect(warning).toHaveTextContent('CHARTBEAT_API_KEY, CHARTBEAT_HOST');
    expect(warning).toHaveTextContent('Kronn ne renomme ni ne supprime');
  });

  it('names the known replacement for an orphaned Microsoft 365 config', () => {
    const config = makeConfig('legacy-m365', 'mcp-microsoft-365', 'Microsoft 365', {
      registry_drift: {
        orphaned: true,
        stored_env_keys: ['MICROSOFT_CLIENT_ID'],
        expected_env_keys: [],
        unexpected_env_keys: ['MICROSOFT_CLIENT_ID'],
        missing_env_keys: [],
        replacement_server_id: 'api-microsoft-365',
      },
    });
    const overview: McpOverview = {
      servers: [makeServer('mcp-microsoft-365', 'Microsoft 365')],
      configs: [config], customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByRole('button', { name: 'Microsoft 365 — Voir les détails' }));

    const warning = screen.getByTestId('mcp-registry-drift');
    expect(warning).toHaveTextContent('Ancienne configuration de plugin détectée');
    expect(warning).toHaveTextContent('mcp-microsoft-365');
    expect(warning).toHaveTextContent('api-microsoft-365');
  });

  it('uses shared collection search and selection for plugin details', async () => {
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub')],
      configs: [makeConfig('github-config', 'github', 'GitHub')],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    const search = screen.getByRole('textbox', { name: 'Rechercher un plugin ou un projet...' });
    fireEvent.change(search, { target: { value: 'missing' } });
    expect(screen.queryByRole('button', { name: 'GitHub — Voir les détails' })).toBeNull();
    fireEvent.change(search, { target: { value: 'github' } });
    fireEvent.click(screen.getByRole('button', { name: 'GitHub — Voir les détails' }));
    expect(document.querySelector('.mcp-sidebar-plugin-row[data-active="true"]')).not.toBeNull();
    expect(document.querySelector('.mcp-detail-inline')).not.toBeNull();
  });

  it('keeps plugin search compact and reveals filtering and sorting below it', () => {
    const githubServer = makeServer('github', 'GitHub');
    const chartbeatServer = makeServer('api-chartbeat', 'Chartbeat');
    chartbeatServer.transport = 'ApiOnly';
    const overview: McpOverview = {
      servers: [githubServer, chartbeatServer],
      configs: [
        makeConfig('github-config', 'github', 'GitHub'),
        makeConfig('chartbeat-config', 'api-chartbeat', 'Chartbeat', {
          interfaces: ['api'],
          effective_kind: 'api',
          effective_preferred_interface: 'api',
        }),
      ],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };

    const { container } = wrap(
      <McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />,
    );
    const search = screen.getByRole('textbox', { name: 'Rechercher un plugin ou un projet...' });
    const searchHeader = search.closest('.collection-shell-header');
    const filterTrigger = screen.getByRole('button', { name: 'Filtrer les plugins' });
    const sortTrigger = screen.getByRole('button', { name: 'Trier les plugins' });

    expect(filterTrigger).toHaveClass('collection-shell-search-action', 'collection-shell-search-action-icon');
    expect(sortTrigger).toHaveClass('collection-shell-search-action', 'collection-shell-search-action-icon');
    expect(filterTrigger).not.toHaveClass('collection-shell-icon');
    expect(filterTrigger.querySelector('span')).toBeNull();
    expect(sortTrigger.querySelector('span')).toBeNull();
    expect(filterTrigger).toHaveAttribute('aria-expanded', 'false');
    expect(sortTrigger).toHaveAttribute('aria-expanded', 'false');
    expect(searchHeader?.querySelector('.list-controls')).toBeNull();
    expect(document.querySelector('.collection-shell-search-options')).toBeNull();

    fireEvent.keyDown(window, { key: '/' });
    expect(search).toHaveFocus();
    fireEvent.change(search, { target: { value: 'missing' } });
    const clearSearch = searchHeader?.querySelector<HTMLButtonElement>(
      'button[aria-label="Effacer les filtres"]',
    );
    expect(clearSearch).not.toBeNull();
    fireEvent.click(clearSearch!);
    expect(search).toHaveValue('');

    fireEvent.click(filterTrigger);
    expect(filterTrigger).toHaveAttribute('aria-expanded', 'true');
    expect(sortTrigger).toHaveAttribute('aria-expanded', 'false');
    const filterOptions = document.querySelector('.collection-shell-search-options');
    expect(filterOptions).toBe(searchHeader?.nextElementSibling);
    expect(filterOptions?.nextElementSibling).toHaveClass('mcp-collection-toolbar');
    expect(filterOptions?.querySelector('.mcp-filter-stack')).not.toBeNull();

    fireEvent.change(screen.getByRole('combobox', { name: 'Filtrer les plugins par type' }), {
      target: { value: 'api' },
    });
    expect(screen.getByRole('button', { name: 'Chartbeat — Voir les détails' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'GitHub — Voir les détails' })).toBeNull();

    fireEvent.change(screen.getByRole('combobox', { name: 'Filtrer les plugins par type' }), {
      target: { value: 'all' },
    });

    fireEvent.click(sortTrigger);
    expect(filterTrigger).toHaveAttribute('aria-expanded', 'false');
    expect(sortTrigger).toHaveAttribute('aria-expanded', 'true');
    expect(screen.queryByRole('combobox', { name: 'Filtrer les plugins par type' })).toBeNull();
    expect(screen.getByRole('combobox', { name: 'Trier les plugins' })).toBeInTheDocument();
    const rowLabels = () => [...container.querySelectorAll('.collection-shell-row-button')]
      .map(row => row.textContent);
    expect(rowLabels()[0]).toContain('Chartbeat');
    fireEvent.click(screen.getByRole('button', { name: 'Inverser l’ordre' }));
    expect(rowLabels()[0]).toContain('GitHub');

    fireEvent.click(sortTrigger);
    expect(sortTrigger).toHaveAttribute('aria-expanded', 'false');
    expect(document.querySelector('.collection-shell-search-options')).toBeNull();
  });

  it('contains plugin search options without imposing an intrinsic sidebar width', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/pages/McpPage.css'), 'utf8');
    const panelRule = css.match(/\.mcp-page \.collection-shell-search-options\s*\{([^}]+)\}/)?.[1] ?? '';
    const controlsRule = css.match(/\.mcp-page \.collection-shell-search-options \.list-controls\s*\{([^}]+)\}/)?.[1] ?? '';
    const controlRule = css.match(/\.mcp-page \.collection-shell-search-options \.list-control\s*\{([^}]+)\}/)?.[1] ?? '';
    const selectRule = css.match(/\.mcp-page \.collection-shell-search-options \.list-control select\s*\{([^}]+)\}/)?.[1] ?? '';

    expect(panelRule).toContain('min-width: 0');
    expect(panelRule).toContain('overflow-x: clip');
    expect(panelRule).not.toContain('padding:');
    expect(panelRule).not.toContain('background:');
    expect(controlsRule).toContain('min-width: 0');
    expect(controlsRule).not.toContain('display: grid');
    expect(controlRule).toContain('flex: 1');
    expect(selectRule).toContain('flex: 1');
    expect(selectRule).toContain('min-width: 0');
    expect(selectRule).toContain('max-width: 100%');
  });

  it('keeps plugin rows in the shared keyboard and active-row contract', () => {
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub'), makeServer('slack', 'Slack')],
      configs: [
        makeConfig('github-config', 'github', 'GitHub'),
        makeConfig('slack-config', 'slack', 'Slack'),
      ],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    const github = screen.getByRole('button', { name: 'GitHub — Voir les détails' });
    const slack = screen.getByRole('button', { name: 'Slack — Voir les détails' });

    github.focus();
    fireEvent.keyDown(github, { key: 'ArrowDown' });
    expect(slack).toHaveFocus();
    fireEvent.click(slack);
    expect(slack).toHaveAttribute('aria-current', 'true');
    expect(document.querySelector('.mcp-detail-inline')).not.toBeNull();
  });

  it('clears a plugin selection when filtering hides its configuration', () => {
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub')],
      configs: [makeConfig('github-config', 'github', 'GitHub')],
      customized_contexts: [], incompatibilities: [], incomplete_configs: [],
    };

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByRole('button', { name: 'GitHub — Voir les détails' }));
    expect(document.querySelector('.mcp-detail-inline')).not.toBeNull();
    act(() => {
      fireEvent.change(screen.getByRole('textbox', { name: 'Rechercher un plugin ou un projet...' }), { target: { value: 'missing' } });
    });
    expect(document.querySelector('.mcp-detail-inline')).toBeNull();
  });

  it('starts bulk selection through the shared collection menu', () => {
    const overview: McpOverview = {
      servers: [makeServer('github', 'GitHub')],
      configs: [makeConfig('github-config', 'github', 'GitHub')],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByRole('button', { name: 'Plus d’actions' }));
    expect(screen.getByRole('menuitem', { name: 'Sélection multiple' })).toBeTruthy();
  });

  it('shows real plugin capabilities and persists only selectable preferences', async () => {
    vi.useRealTimers();
    const server: McpServer = {
      ...makeServer('mcp-fastly', 'Fastly'),
      api_spec: {
        base_url: 'https://api.fastly.com',
        auth: 'None',
        endpoints: [],
        docs_url: null,
        config_keys: [],
      },
    };
    const definition: McpDefinition = {
      id: 'mcp-fastly',
      name: 'Fastly',
      description: 'Fastly hybrid',
      transport: server.transport,
      env_keys: [],
      tags: ['cli', 'api'],
      token_url: null,
      token_help: null,
      publisher: 'Fastly',
      official: true,
      api_spec: server.api_spec,
    };
    const config = makeConfig('fastly-config', 'mcp-fastly', 'Fastly', {
      preferred_interface: 'api',
      interfaces: ['api', 'mcp', 'cli'],
      effective_kind: 'cli',
      effective_preferred_interface: 'api',
    });
    const overview: McpOverview = {
      servers: [server],
      configs: [config],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };
    const refetch = vi.fn().mockResolvedValue(undefined);
    (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mockResolvedValue(config);

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[definition]} refetchMcps={refetch} />);
    fireEvent.click(screen.getByRole('button', { name: 'Fastly — Voir les détails' }));

    const chips = document.querySelectorAll('.mcp-interface-chip[data-available="true"]');
    expect(Array.from(chips).map(chip => chip.getAttribute('data-interface'))).toEqual([
      'api',
      'mcp',
      'cli',
    ]);
    const select = screen.getByTestId('mcp-preferred-interface') as HTMLSelectElement;
    expect(Array.from(select.options).map(option => option.value)).toEqual(['api', 'mcp', 'cli']);

    fireEvent.change(select, { target: { value: 'cli' } });
    await act(async () => {});
    expect(mcpsApi.updateConfig).toHaveBeenCalledWith('fastly-config', {
      preferred_interface: 'cli',
    });
    expect(refetch).toHaveBeenCalledTimes(1);
  });

  it('collapses preference choices for a single-interface plugin', () => {
    const server: McpServer = {
      ...makeServer('api-only', 'API only'),
      transport: 'ApiOnly',
      api_spec: {
        base_url: 'https://example.test',
        auth: 'None',
        endpoints: [],
        docs_url: null,
        config_keys: [],
      },
    };
    const overview: McpOverview = {
      servers: [server],
      configs: [makeConfig('api-config', 'api-only', 'API only', {
        preferred_interface: 'api',
        interfaces: ['api'],
        effective_kind: 'api',
        effective_preferred_interface: 'api',
      })],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByRole('button', { name: 'API only — Voir les détails' }));
    const select = screen.getByTestId('mcp-preferred-interface') as HTMLSelectElement;
    expect(select).toBeDisabled();
    expect(Array.from(select.options).map(option => option.value)).toEqual(['api']);
  });

  it('swaps the open panel to another plugin in a single click', () => {
    const overview: McpOverview = {
      servers: [makeServer('a', 'Alpha'), makeServer('b', 'Bravo')],
      configs: [makeConfig('cfg-a', 'a', 'Alpha'), makeConfig('cfg-b', 'b', 'Bravo')],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByRole('button', { name: 'Alpha — Voir les détails' }));
    expect(document.querySelector('[data-testid="mcp-plugin-detail"]')).toHaveAttribute('aria-label', 'Alpha');
    expect(document.querySelector('.mcp-detail-header')).toHaveClass('collection-detail-header');

    // No close step: the second card is reachable because nothing blocks it.
    fireEvent.click(screen.getByRole('button', { name: 'Bravo — Voir les détails' }));
    const details = document.querySelectorAll('[data-testid="mcp-plugin-detail"]');
    expect(details).toHaveLength(1);
    expect(details[0]).toHaveAttribute('aria-label', 'Bravo');
  });

  it('combines plugin type filtering with criterion and direction sorting', () => {
    const mcpServer = makeServer('mcp', 'MCP server');
    const apiServer: McpServer = {
      ...makeServer('api', 'API server'),
      transport: 'ApiOnly',
    };
    const cliServer = makeServer('cli', 'CLI server');
    const cliDefinition: McpDefinition = {
      id: 'cli',
      name: 'CLI server',
      description: 'CLI server',
      transport: cliServer.transport,
      env_keys: [],
      tags: ['cli'],
      token_url: null,
      token_help: null,
      publisher: 'Kronn',
      official: false,
    };
    const overview: McpOverview = {
      servers: [mcpServer, apiServer, cliServer],
      configs: [
        makeConfig('mcp-config', 'mcp', 'MCP server', { label: 'Zulu MCP', interfaces: ['mcp'], effective_kind: 'mcp' }),
        makeConfig('api-config', 'api', 'API server', { label: 'Alpha API', interfaces: ['api'], effective_kind: 'api' }),
        makeConfig('cli-config', 'cli', 'CLI server', { label: 'Beta CLI', interfaces: ['mcp', 'cli'], effective_kind: 'cli' }),
      ],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };
    const { container } = wrap(
      <McpPage projects={[]} mcpOverview={overview} mcpRegistry={[cliDefinition]} refetchMcps={noop} />,
    );
    const cardOrder = () => Array.from(
      container.querySelectorAll<HTMLElement>('.mcp-sidebar-plugin-row'),
      card => card.dataset.configId,
    ).filter((id): id is string => id !== undefined);

    expect(cardOrder()).toEqual(['api-config', 'cli-config', 'mcp-config']);
    fireEvent.click(screen.getByRole('button', { name: 'Trier les plugins' }));
    fireEvent.change(screen.getByRole('combobox', { name: 'Trier les plugins' }), {
      target: { value: 'kind' },
    });
    expect(cardOrder()).toEqual(['mcp-config', 'api-config', 'cli-config']);

    fireEvent.click(screen.getByRole('button', { name: 'Inverser l’ordre' }));
    expect(cardOrder()).toEqual(['cli-config', 'api-config', 'mcp-config']);

    fireEvent.click(screen.getByRole('button', { name: 'Filtrer les plugins' }));
    fireEvent.change(screen.getByRole('combobox', { name: 'Filtrer les plugins par type' }), {
      target: { value: 'api' },
    });
    expect(cardOrder()).toEqual(['api-config']);
    expect(screen.queryByTestId('mcp-kronn-internal-card')).not.toBeInTheDocument();
  });

  it('shows global scope badge on global config', () => {
    const configs = [
      makeConfig('c1', 'github', 'GitHub', { is_global: true }),
    ];
    const overview: McpOverview = { servers: [makeServer('github', 'GitHub')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Global badge should be rendered on the card
    expect(container.textContent).toContain('Tous les projets');
  });

  it('flags an orphan config and repairs it by enabling General scope', async () => {
    vi.mocked(mcpsApi.updateConfig).mockClear();
    const config = makeConfig('resend-config', 'resend', 'Resend', {
      include_general: false,
      project_ids: [],
    });
    vi.mocked(mcpsApi.updateConfig).mockResolvedValue({
      ...config,
      include_general: true,
    });
    const overview: McpOverview = {
      servers: [makeServer('resend', 'Resend')],
      configs: [config],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    fireEvent.click(screen.getByRole('button', { name: 'Resend — Voir les détails' }));
    expect(screen.getByRole('alert')).toHaveTextContent('Cette configuration n’est visible par aucun agent');

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Rendre visible dans Général' }));
    });
    expect(mcpsApi.updateConfig).toHaveBeenCalledWith('resend-config', {
      include_general: true,
    });
  });

  it('does not let the user remove the last agent scope', async () => {
    vi.mocked(mcpsApi.updateConfig).mockClear();
    const config = makeConfig('general-only', 'resend', 'Resend', {
      include_general: true,
      project_ids: [],
    });
    const overview: McpOverview = {
      servers: [makeServer('resend', 'Resend')],
      configs: [config],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByRole('button', { name: 'Resend — Voir les détails' }));

    await act(async () => {
      fireEvent.click(screen.getByTitle('Désactiver pour les discussions sans projet'));
    });
    expect(mcpsApi.updateConfig).not.toHaveBeenCalled();
    expect(screen.getByText('Choisis au moins une portée avant de retirer la dernière.')).toBeInTheDocument();
  });

  it('"Add MCP" button opens the add form', () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const registry: McpDefinition[] = [
      { id: 'test-mcp', name: 'Test MCP', description: 'A test server', transport: { Stdio: { command: 'node', args: [] } }, env_keys: [], tags: ['core'], token_url: null, token_help: null, publisher: 'Anthropic', official: false },
    ];
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={registry} refetchMcps={noop} />);

    const addBtn = getAddPluginButton();
    fireEvent.click(addBtn);

    // The registry entry should now be visible
    expect(document.body.textContent).toContain('Test MCP');
    expect(document.body.textContent).toContain('A test server');
  });

  it('shows the read-only built-in kronn-internal entry as a standard plugin card', () => {
    // kronn-internal is auto-injected into every project — surfaced as a
    // read-only system card on the MAIN Plugins view (not behind "Ajouter"),
    // visible even with zero configs.
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // No interaction: the card is present immediately on the default view.
    const card = screen.getByTestId('mcp-kronn-internal-card');
    expect(card).toBeTruthy();
    expect(card).toHaveClass('mcp-installed-card');
    expect(card).not.toHaveClass('mcp-card');
    // FR strings: name + built-in badge.
    expect(card.textContent).toContain('Kronn Internal');
    expect(card.textContent).toContain('Intégré');
  });

  it('uses the real kronn-internal config in each of its declared scope groups', () => {
    const config = makeConfig('kronn-config', 'detected:kronn-internal', 'kronn-internal', {
      is_global: true,
    });
    const overview: McpOverview = {
      servers: [makeServer('detected:kronn-internal', 'kronn-internal')],
      configs: [config],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    const rows = screen.getAllByTestId('mcp-kronn-internal-card');
    expect(rows).toHaveLength(1);
    expect(rows.every(row => row.dataset.configId === 'kronn-config')).toBe(true);
    expect(rows.every(row => row.textContent?.includes('Intégré'))).toBe(true);
  });

  it('shows incompatibility badge in detail panel when card is clicked', () => {
    const servers = [makeServer('mcp-gitlab', 'GitLab')];
    const configs = [makeConfig('c1', 'mcp-gitlab', 'GitLab')];
    const overview: McpOverview = {
      servers, configs, customized_contexts: [],
      incompatibilities: [{ server_id: 'mcp-gitlab', agent: 'Kiro' as AgentType, reason: 'Empty tool schemas — incompatible with Bedrock' }], incomplete_configs: [],
    };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Click the plugin card to open the detail panel
    openPlugin('GitLab');

    // The incompatibility badge should show the agent name in the detail panel
    expect(container.textContent).toContain('Kiro');
  });

  it('does not show incompatibility badge for compatible servers', () => {
    const servers = [makeServer('mcp-github', 'GitHub')];
    const configs = [makeConfig('c1', 'mcp-github', 'GitHub')];
    const overview: McpOverview = {
      servers, configs, customized_contexts: [],
      incompatibilities: [{ server_id: 'mcp-gitlab', agent: 'Kiro' as AgentType, reason: 'test' }], incomplete_configs: [],
    };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // GitHub should NOT show Kiro warning (only gitlab is incompatible)
    // The page should render GitHub but the warning badge should not appear
    expect(container.textContent).toContain('GitHub');
    expect(container.textContent).not.toContain('Kiro');
  });

  it('shows project count badge when linked to projects', () => {
    const projects = [makeProject('p1', 'my-app'), makeProject('p2', 'my-api')];
    const configs = [
      makeConfig('c1', 'github', 'GitHub', { project_ids: ['p1'], project_names: ['my-app'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('github', 'GitHub')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={projects} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Card shows project count badge
    expect(container.textContent).toContain('1 projet');
  });

  it('shows all plugin cards without needing to expand', () => {
    const servers = [makeServer('context7', 'Context7')];
    const configs = [
      makeConfig('c1', 'context7', 'Context7', { label: 'Context7 Main' }),
      makeConfig('c2', 'context7', 'Context7', { label: 'Context7 Dev', env_keys: ['CONTEXT7_KEY'] }),
    ];
    const overview: McpOverview = { servers, configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // All cards are visible immediately (no accordion to expand)
    expect(container.textContent).toContain('Context7 Main');
    expect(container.textContent).toContain('Context7 Dev');
  });

  /* ── Edit button and eye icon regression tests ── */

  it('shows env key count badge on card for configs with env_keys', () => {
    const configs = [
      makeConfig('c1', 'gitlab', 'GitLab', { env_keys: ['GITLAB_API_URL', 'GITLAB_TOKEN'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Card shows key count (2 env keys)
    expect(container.textContent).toContain('2');
  });

  it('shows env vars section with edit pencil button in detail panel', () => {
    const configs = [
      makeConfig('c1', 'gitlab', 'GitLab', { env_keys: ['GITLAB_TOKEN'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Click card to expand detail panel
    openPlugin('GitLab');

    // Direct API credentials are explicitly distinguished from local CLI auth.
    expect(container.textContent).toContain("Identifiants utilisés par l’API");
    // Edit pencil button should exist (title = "Modifier les clés")
    const editBtn = container.querySelector('button[title="Modifier les clés"]');
    expect(editBtn).toBeTruthy();
  });

  it('shows eye icon for each env field in detail panel (before entering edit mode)', () => {
    const configs = [
      makeConfig('c1', 'gitlab', 'GitLab', { env_keys: ['GITLAB_API_URL', 'GITLAB_TOKEN'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail panel
    openPlugin('GitLab');

    // Eye buttons should be present for each env field (title = "Afficher")
    const eyeButtons = container.querySelectorAll('button[title="Afficher"]');
    expect(eyeButtons.length).toBe(2);
  });

  it('pencil edit button enters edit mode and shows save/cancel buttons', async () => {
    vi.mocked(mcpsApi.revealSecrets).mockResolvedValue([
      { key: 'GITLAB_TOKEN', masked_value: 'glpat-secret123' },
    ]);
    const configs = [
      makeConfig('c1', 'gitlab', 'GitLab', { env_keys: ['GITLAB_TOKEN'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail panel
    openPlugin('GitLab');

    // Click pencil edit button
    const editBtn = container.querySelector('button[title="Modifier les clés"]') as HTMLElement;
    await act(async () => { fireEvent.click(editBtn); });

    // Save/cancel buttons should appear
    expect(container.textContent).toContain('Sauvegarder');
    expect(container.textContent).toContain('Annuler');

    // Pencil button should be hidden while editing
    const editBtnAfter = container.querySelector('button[title="Modifier les clés"]');
    expect(editBtnAfter).toBeNull();
  });

  it('eye icon reveals token value when clicked', async () => {
    vi.mocked(mcpsApi.revealSecrets).mockResolvedValue([
      { key: 'GITLAB_TOKEN', masked_value: 'glpat-secret123' },
    ]);
    const configs = [
      makeConfig('c1', 'gitlab', 'GitLab', { env_keys: ['GITLAB_TOKEN'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail panel
    openPlugin('GitLab');

    // All inputs should be password type initially
    const inputBefore = container.querySelector('input.mcp-input-mono') as HTMLInputElement;
    expect(inputBefore.type).toBe('password');

    // Click eye button (triggers edit mode + toggle visibility)
    const eyeBtn = container.querySelector('button[title="Afficher"]') as HTMLElement;
    await act(async () => { fireEvent.click(eyeBtn); });

    // Input should now be text (visible)
    const input = container.querySelector('input.mcp-input-mono') as HTMLInputElement;
    expect(input.type).toBe('text');
  });

  it('eye icon toggles between show and hide', async () => {
    vi.mocked(mcpsApi.revealSecrets).mockResolvedValue([
      { key: 'MY_KEY', masked_value: 'secret-value' },
    ]);
    const configs = [
      makeConfig('c1', 'test', 'TestMCP', { env_keys: ['MY_KEY'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('test', 'TestMCP')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail panel
    openPlugin('TestMCP');

    // Click eye to reveal (enters edit mode + shows)
    const eyeBtn = container.querySelector('button[title="Afficher"]') as HTMLElement;
    await act(async () => { fireEvent.click(eyeBtn); });

    const input = container.querySelector('input.mcp-input-mono') as HTMLInputElement;
    expect(input.type).toBe('text');

    // Click eye again to hide (title is now "Masquer")
    const hideBtn = container.querySelector('button[title="Masquer"]') as HTMLElement;
    fireEvent.click(hideBtn);

    expect(input.type).toBe('password');
  });

  it('cancel button exits edit mode and hides save/cancel', async () => {
    vi.mocked(mcpsApi.revealSecrets).mockResolvedValue([
      { key: 'TOKEN', masked_value: 'val' },
    ]);
    const configs = [
      makeConfig('c1', 'test', 'TestMCP', { env_keys: ['TOKEN'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('test', 'TestMCP')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail, enter edit mode
    openPlugin('TestMCP');
    const editBtn = container.querySelector('button[title="Modifier les clés"]') as HTMLElement;
    await act(async () => { fireEvent.click(editBtn); });

    expect(container.textContent).toContain('Annuler');

    // Click cancel
    fireEvent.click(screen.getByText('Annuler'));

    // Save/cancel should disappear, pencil should reappear
    expect(container.textContent).not.toContain('Sauvegarder');
    const editBtnBack = container.querySelector('button[title="Modifier les clés"]');
    expect(editBtnBack).toBeTruthy();
  });

  it('env field labels are visible in detail panel', () => {
    const configs = [
      makeConfig('c1', 'gitlab', 'GitLab', { env_keys: ['GITLAB_API_URL', 'GITLAB_PERSONAL_ACCESS_TOKEN'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail
    openPlugin('GitLab');

    // Field labels should be visible
    expect(container.textContent).toContain('GITLAB_API_URL');
    expect(container.textContent).toContain('GITLAB_PERSONAL_ACCESS_TOKEN');
  });

  it('shows warning and enters edit mode when revealSecrets fails', async () => {
    vi.mocked(mcpsApi.revealSecrets).mockRejectedValue(new Error('Decryption failed'));
    const configs = [
      makeConfig('c1', 'gitlab', 'GitLab', { env_keys: ['GITLAB_TOKEN'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail panel
    openPlugin('GitLab');

    // Click pencil edit button
    const editBtn = container.querySelector('button[title="Modifier les clés"]') as HTMLElement;
    await act(async () => { fireEvent.click(editBtn); });

    // Warning message should be visible
    expect(container.textContent).toContain('déchiffr');
    // Edit mode should still be active (save/cancel visible) so user can re-enter values
    expect(container.textContent).toContain('Sauvegarder');
    expect(container.textContent).toContain('Annuler');
  });

  it('eye icon enters edit mode with empty values when revealSecrets fails', async () => {
    vi.mocked(mcpsApi.revealSecrets).mockRejectedValue(new Error('Decryption failed'));
    const configs = [
      makeConfig('c1', 'test', 'TestMCP', { env_keys: ['TOKEN'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('test', 'TestMCP')], configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail panel
    openPlugin('TestMCP');

    // Click eye button
    const eyeBtn = container.querySelector('button[title="Afficher"]') as HTMLElement;
    await act(async () => { fireEvent.click(eyeBtn); });

    // Edit mode should be active (user can type new values)
    expect(container.textContent).toContain('Sauvegarder');
    // Input should be text (eye reveals) with empty value
    const input = container.querySelector('input.mcp-input-mono') as HTMLInputElement;
    expect(input.type).toBe('text');
    expect(input.value).toBe('');
    // Warning should be shown
    expect(container.textContent).toContain('déchiffr');
  });

  /* ── Publisher / official badge tests ── */

  it('shows official badge for vendor-built MCP in registry', () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const registry: McpDefinition[] = [
      { id: 'mcp-fastly', name: 'Fastly', description: 'CDN server', transport: { Stdio: { command: 'fastly-mcp', args: [] } }, env_keys: [], tags: ['cdn'], token_url: null, token_help: null, publisher: 'Fastly', official: true },
    ];
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={registry} refetchMcps={noop} />);
    fireEvent.click(getAddPluginButton());
    expect(document.body.textContent).toContain('Officiel');
    expect(document.body.textContent).toContain('Fastly');
  });

  it('distinguishes CLI authentication from credentials used directly by an API', () => {
    const fastlyDefinition: McpDefinition = {
      id: 'mcp-fastly',
      name: 'Fastly',
      description: 'CDN server',
      transport: { Stdio: { command: 'fastly-mcp', args: [] } },
      env_keys: [],
      tags: ['cli', 'api', 'cdn'],
      token_url: 'https://manage.fastly.com/account/personal/tokens',
      token_help: 'Fastly CLI authentication',
      publisher: 'Fastly',
      official: true,
      api_spec: {
        base_url: 'https://api.fastly.com',
        auth: {
          CliToken: {
            command: 'fastly',
            args: ['auth', 'token'],
            inject: { CustomHeader: { name: 'Fastly-Key' } },
            fallback_env_key: 'FASTLY_API_TOKEN',
          },
        },
        endpoints: [],
        config_keys: [],
      },
    };
    const apiDefinition: McpDefinition = {
      id: 'api-direct',
      name: 'Direct API',
      description: 'Direct API plugin',
      transport: 'ApiOnly',
      env_keys: ['DIRECT_API_TOKEN'],
      tags: ['api'],
      token_url: null,
      token_help: 'API token',
      publisher: 'Vendor',
      official: true,
      api_spec: null,
    };
    const overview: McpOverview = {
      servers: [makeServer('mcp-fastly', 'Fastly'), makeServer('api-direct', 'Direct API')],
      configs: [
        makeConfig('fastly-config', 'mcp-fastly', 'Fastly', {
          interfaces: ['api', 'mcp', 'cli'],
          effective_kind: 'cli',
          credential_source: 'cli_token',
        }),
        makeConfig('api-config', 'api-direct', 'Direct API', {
          env_keys: ['DIRECT_API_TOKEN'],
          interfaces: ['api'],
          effective_kind: 'api',
          credential_source: 'stored',
        }),
      ],
      customized_contexts: [],
      incompatibilities: [],
      incomplete_configs: [],
    };
    const { container } = wrap(
      <McpPage
        projects={[]}
        mcpOverview={overview}
        mcpRegistry={[fastlyDefinition, apiDefinition]}
        refetchMcps={noop}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Fastly — Voir les détails' }));
    const cliBackup = container.querySelector('.mcp-credential-fields[data-kind="cli"]');
    expect(cliBackup).toHaveTextContent('Sauvegarde facultative');
    expect(cliBackup).toHaveTextContent('Kronn exécute d’abord la commande du CLI local');
    expect(cliBackup).toContainElement(screen.getByPlaceholderText('Non enregistré — session CLI locale utilisée'));
    expect(screen.getByPlaceholderText('Non enregistré — session CLI locale utilisée')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Direct API — Voir les détails' }));
    expect(container.querySelector('[data-testid="mcp-plugin-detail"]')).toHaveTextContent('Identifiants utilisés par l’API');
    expect(container.querySelector('[data-testid="mcp-plugin-detail"]')).toHaveTextContent('Utilisé directement par Kronn');
  });

  it('shows community badge for third-party MCP in registry', () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const registry: McpDefinition[] = [
      { id: 'mcp-github', name: 'GitHub', description: 'GitHub server', transport: { Stdio: { command: 'npx', args: ['-y', 'server'] } }, env_keys: ['TOKEN'], tags: ['git'], token_url: null, token_help: null, publisher: 'Anthropic', official: false },
    ];
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={registry} refetchMcps={noop} />);
    fireEvent.click(getAddPluginButton());
    expect(document.body.textContent).toContain('Communautaire');
    expect(document.body.textContent).toContain('Anthropic');
  });

  it('shows publisher badge in detail panel of installed MCP', () => {
    const servers = [makeServer('mcp-redis', 'Redis')];
    const configs = [makeConfig('c1', 'mcp-redis', 'Redis')];
    const overview: McpOverview = { servers, configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const registry: McpDefinition[] = [
      { id: 'mcp-redis', name: 'Redis', description: 'Cache server', transport: { Stdio: { command: 'uvx', args: ['redis-mcp'] } }, env_keys: [], tags: ['cache'], token_url: null, token_help: null, publisher: 'Redis Ltd', official: true },
    ];
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={registry} refetchMcps={noop} />);
    openPlugin('Redis');
    expect(container.textContent).toContain('Officiel');
    expect(container.textContent).toContain('Redis Ltd');
  });

  // ─── Delete confirmation in the Advanced danger zone ──────
  it('danger-zone cancel keeps the plugin sheet open without deleting', async () => {
    const servers = [makeServer('mcp-redis', 'Redis')];
    const configs = [makeConfig('c1', 'mcp-redis', 'Redis')];
    const overview: McpOverview = { servers, configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    openPlugin('Redis');
    expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual(['État', 'Accès', 'Identifiants', 'Avancé']);
    fireEvent.click(screen.getByRole('tab', { name: 'Avancé' }));

    const deleteBtn = screen.getByText(/Supprimer cette config/);
    await act(async () => { fireEvent.click(deleteBtn); });
    expect(screen.getByText(/Cette action est irréversible/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Annuler' }));

    expect(mcpsApi.deleteConfig).not.toHaveBeenCalled();
    expect(screen.getByTestId('mcp-plugin-detail')).toBeInTheDocument();
    expect(screen.queryByText(/Cette action est irréversible/)).toBeNull();
  });

  it('Delete confirmed → API called + success toast', async () => {
    const servers = [makeServer('mcp-redis', 'Redis')];
    const configs = [makeConfig('c1', 'mcp-redis', 'Redis')];
    const overview: McpOverview = { servers, configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };

    vi.mocked(mcpsApi.deleteConfig).mockResolvedValue(undefined);

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    openPlugin('Redis');
    fireEvent.click(screen.getByRole('tab', { name: 'Avancé' }));

    const deleteBtn = screen.getByText(/Supprimer cette config/);
    fireEvent.click(deleteBtn);
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Supprimer définitivement' }));
      await Promise.resolve();
    });

    expect(mcpsApi.deleteConfig).toHaveBeenCalledWith('c1');
  });

  it('Incomplete-config banner lists each broken plugin with its missing keys', async () => {
    // Pin user-reported behaviour 2026-05-10: when a plugin's config is
    // incomplete (e.g. Adobe Analytics missing ADOBE_COMPANY_ID), Kronn
    // SKIPS writing it to project-level files (so Gemini/Claude don't
    // choke at boot) AND surfaces it as a UI warning so the operator
    // knows what to fix.
    const servers = [makeServer('adobe-analytics', 'Adobe Analytics')];
    const configs = [makeConfig('cfg-broken', 'adobe-analytics', 'Adobe Analytics')];
    const overview: McpOverview = {
      servers, configs, customized_contexts: [], incompatibilities: [],
      incomplete_configs: [{
        config_id: 'cfg-broken',
        label: 'Adobe Analytics',
        server_name: 'Adobe Analytics',
        missing_keys: ['ADOBE_COMPANY_ID', 'ADOBE_RSID'],
        reason: '2 clé(s) requise(s) manquante(s) ou vide(s)',
      }],
    };

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    const banner = screen.getByTestId('mcp-incomplete-banner');
    expect(banner).toBeDefined();
    expect(banner.textContent).toMatch(/Adobe Analytics/);
    expect(banner.textContent).toMatch(/ADOBE_COMPANY_ID, ADOBE_RSID/);
    // The `1 plugin(s) not operational` count surfaces in FR locale.
    expect(banner.textContent).toMatch(/1 plugin/);
  });

  it('Incomplete-config banner is hidden when no broken configs', async () => {
    // Default mock fixtures from earlier tests don't pass incomplete_configs;
    // the banner must not appear unless explicitly populated.
    const servers = [makeServer('mcp-redis', 'Redis')];
    const configs = [makeConfig('c1', 'mcp-redis', 'Redis')];
    const overview: McpOverview = { servers, configs, customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    expect(screen.queryByTestId('mcp-incomplete-banner')).toBeNull();
  });

  // ── Custom API flow ────────────────────────────────────────────────
  // The Custom API plugin lets users define their own REST endpoint with
  // a freeform Name/Base URL/Description + arbitrary fields. These tests
  // pin the submit shape (must include `custom_spec`) and the field
  // slugifier on the client side so the wire payload matches the backend
  // contract (see `materialize_custom_server` in backend/src/api/mcps.rs).

  it('Custom API: clicking the pinned tile opens the freeform form', async () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const customApi: McpDefinition = {
      id: 'api-custom',
      name: 'Custom API',
      description: 'Define your own API.',
      transport: 'ApiOnly',
      env_keys: [],
      tags: ['custom', 'api'],
      token_url: null,
      token_help: null,
      publisher: 'You',
      official: false,
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[customApi]} refetchMcps={noop} />);

    // Open the drawer
    const addBtn = getAddPluginButton();
    fireEvent.click(addBtn);

    // Pinned Custom API tile should be present in the registry grid
    const tile = document.querySelector('[data-tour-id="custom-api-tile"]') as HTMLElement | null;
    expect(tile).toBeTruthy();
    fireEvent.click(tile!);

    // The freeform form is visible (Name + Base URL labels, asterisks for required)
    expect(screen.getByPlaceholderText(/Salesforce Sales API/)).toBeTruthy();
    expect(screen.getByPlaceholderText(/my-org\.salesforce\.com/)).toBeTruthy();
  });

  it('Custom API: submit posts custom_spec with the form payload', async () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const customApi: McpDefinition = {
      id: 'api-custom',
      name: 'Custom API',
      description: 'Define your own API.',
      transport: 'ApiOnly',
      env_keys: [],
      tags: ['custom', 'api'],
      token_url: null,
      token_help: null,
      publisher: 'You',
      official: false,
    };
    (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mockResolvedValue({});

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[customApi]} refetchMcps={noop} />);

    fireEvent.click(getAddPluginButton());
    const tile = document.querySelector('[data-tour-id="custom-api-tile"]') as HTMLElement;
    fireEvent.click(tile);

    // Fill required fields
    fireEvent.change(screen.getByPlaceholderText(/Salesforce Sales API/), { target: { value: 'MyAPI' } });
    fireEvent.change(screen.getByPlaceholderText(/my-org\.salesforce\.com/), { target: { value: 'https://my.example.com' } });

    // Fill the first (default) field row
    const labelInputs = screen.getAllByPlaceholderText(/Bearer Token/);
    fireEvent.change(labelInputs[0], { target: { value: 'My Token' } });
    const valueInputs = screen.getAllByPlaceholderText(/Valeur/);
    fireEvent.change(valueInputs[0], { target: { value: 'secret123' } });

    // Submit — the Save button reads "Enregistrer" in FR
    const saveBtn = screen.getByText('Enregistrer');
    fireEvent.click(saveBtn);

    await act(async () => { await Promise.resolve(); });

    expect(mcpsApi.createConfig).toHaveBeenCalledTimes(1);
    const payload = (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(payload.server_id).toBe('api-custom');
    expect(payload.custom_spec).toBeDefined();
    expect(payload.custom_spec.name).toBe('MyAPI');
    expect(payload.custom_spec.base_url).toBe('https://my.example.com');
    expect(payload.custom_spec.fields).toEqual([{ label: 'My Token', value: 'secret123' }]);
  });

  // ─── KT-831 — single scope editor at add time, opened fiche, merge ──

  it('Add flow: the scope editor proposes specific projects, and the created fiche opens once it appears', async () => {
    const project = makeProject('p1', 'Website');
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const customApi: McpDefinition = {
      id: 'api-custom',
      name: 'Custom API',
      description: 'Define your own API.',
      transport: 'ApiOnly',
      env_keys: [],
      tags: ['custom', 'api'],
      token_url: null,
      token_help: null,
      publisher: 'You',
      official: false,
    };
    (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mockClear();
    const created = makeConfig('new-cfg-1', 'custom-myapi-abc', 'MyAPI', { label: 'MyAPI', project_ids: ['p1'] });
    (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mockResolvedValue(created);

    const view = wrap(<McpPage projects={[project]} mcpOverview={overview} mcpRegistry={[customApi]} refetchMcps={noop} />);

    fireEvent.click(getAddPluginButton());
    fireEvent.click(document.querySelector('[data-tour-id="custom-api-tile"]') as HTMLElement);
    fireEvent.change(screen.getByPlaceholderText(/Salesforce Sales API/), { target: { value: 'MyAPI' } });
    fireEvent.change(screen.getByPlaceholderText(/my-org\.salesforce\.com/), { target: { value: 'https://my.example.com' } });

    // Pick a specific project; the pre-KT-831 add form only offered an
    // all-projects on/off toggle and could never populate `project_ids`.
    fireEvent.click(screen.getByRole('button', { name: 'Website' }));
    fireEvent.click(screen.getByText('Enregistrer'));
    await act(async () => { await Promise.resolve(); });

    const payload = (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(payload.is_global).toBe(false);
    expect(payload.project_ids).toEqual(['p1']);

    // The fiche can't select a config `mcpOverview` doesn't know about yet —
    // simulate `refetchMcps()` landing by re-rendering with the created
    // config now present, then it should open on its own.
    view.rerender(
      <I18nProvider>
        <McpPage projects={[project]} mcpOverview={{ ...overview, configs: [created] }} mcpRegistry={[customApi]} refetchMcps={noop} />
      </I18nProvider>,
    );
    await act(async () => { await Promise.resolve(); });
    expect(screen.getByTestId('mcp-plugin-detail')).toHaveTextContent('MyAPI');
  });

  it('Add flow: an MCP can select both projects and local CLI sync before creation', async () => {
    const project = makeProject('p1', 'Website');
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const definition: McpDefinition = {
      id: 'test-mcp',
      name: 'Test MCP',
      description: 'A test server',
      transport: { Stdio: { command: 'node', args: [] } },
      env_keys: [],
      tags: ['core'],
      token_url: null,
      token_help: null,
      publisher: 'Anthropic',
      official: false,
    };
    const created = makeConfig('new-mcp-1', definition.id, definition.name, {
      project_ids: [project.id],
      host_sync: 'GlobalOnly',
    });
    vi.mocked(mcpsApi.createConfig).mockClear();
    vi.mocked(mcpsApi.createConfig).mockResolvedValue(created);
    wrap(<McpPage projects={[project]} mcpOverview={overview} mcpRegistry={[definition]} refetchMcps={noop} />);

    fireEvent.click(getAddPluginButton());
    fireEvent.click(screen.getByText('Test MCP'));
    const scope = screen.getByTestId('mcp-add-scope');
    fireEvent.click(within(scope).getByRole('button', { name: 'Website' }));
    fireEvent.click(within(scope).getByRole('checkbox', { name: 'Aussi disponible dans mes CLIs locaux' }));
    fireEvent.click(within(screen.getByRole('dialog', { name: 'Configurer Test MCP' })).getByRole('button', { name: 'Ajouter' }));
    await act(async () => { await Promise.resolve(); });

    expect(mcpsApi.createConfig).toHaveBeenCalledWith(expect.objectContaining({
      project_ids: ['p1'],
      host_sync: 'GlobalOnly',
    }));
  });

  it('Add flow: merging into an existing identical config is announced, not silently reported as "created"', async () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    const customApi: McpDefinition = {
      id: 'api-custom',
      name: 'Custom API',
      description: 'Define your own API.',
      transport: 'ApiOnly',
      env_keys: [],
      tags: ['custom', 'api'],
      token_url: null,
      token_help: null,
      publisher: 'You',
      official: false,
    };
    (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mockClear();
    const merged: McpConfigDisplay = { ...makeConfig('existing-1', 'custom-myapi-abc', 'MyAPI', { label: 'MyAPI', is_global: true }), merged_into_existing: 'existing-1' };
    (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mockResolvedValue(merged);

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[customApi]} refetchMcps={noop} />);

    fireEvent.click(getAddPluginButton());
    fireEvent.click(document.querySelector('[data-tour-id="custom-api-tile"]') as HTMLElement);
    fireEvent.change(screen.getByPlaceholderText(/Salesforce Sales API/), { target: { value: 'MyAPI' } });
    fireEvent.change(screen.getByPlaceholderText(/my-org\.salesforce\.com/), { target: { value: 'https://my.example.com' } });
    fireEvent.click(screen.getByText('Enregistrer'));
    await act(async () => { await Promise.resolve(); });

    expect(screen.getByText(/existait déjà avec les mêmes identifiants/)).toBeTruthy();
  });

  // ─── 0.8.6 — Unified Custom plugin edit form ─────────────────────────
  //
  // Regression guards for the live Didomi debug 2026-05-19/20:
  //  - the "Modifier le plugin" button must open the form pre-filled
  //    with name / base_url / fields / endpoints / auth
  //  - submit must call updateCustomSpec (PUT) AND updateConfig (PATCH)
  //    when both server_id and config_id are tracked
  //  - the toast must use the captured name (not the post-reset empty
  //    string — that was the silent-toast bug)
  //
  // The detail panel renders inside a card; the Edit button only shows
  // when the cfg.server_id starts with "custom-" AND the matching
  // server has an api_spec. Both must be true for the form to mount.

  const makeCustomServer = (serverId: string, name: string): McpServer => ({
    id: serverId,
    name,
    description: `${name} API`,
    transport: 'ApiOnly',
    source: 'Manual',
    api_spec: {
      base_url: 'https://api.example.com/v1',
      auth: 'None',
      docs_url: 'https://docs.example.com',
      endpoints: [
        { path: '/users', method: 'GET', description: 'List users' },
        { path: '/users/{id}', method: 'GET', description: 'Get one user' },
      ],
      config_keys: [
        { env_key: 'API_KEY', label: 'API Key', placeholder: 'sk-…', description: '' },
      ],
    },
  });

  const openEditDrawer = async (serverId: string, cfgId: string) => {
    const customServer = makeCustomServer(serverId, 'ExampleAPI');
    const config = makeConfig(cfgId, serverId, 'ExampleAPI', {
      env_keys: ['API_KEY'],
    });
    const overview: McpOverview = {
      servers: [customServer],
      configs: [config],
      customized_contexts: [],
      incompatibilities: [], incomplete_configs: [],
    };

    (mcpsApi.revealSecrets as ReturnType<typeof vi.fn>).mockResolvedValue([
      { key: 'API_KEY', masked_value: 'sk-secret-123', secret: true },
    ]);

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    fireEvent.click(screen.getByRole('button', { name: 'ExampleAPI — Voir les détails' }));

    // Click "Modifier le plugin" (FR). The label appears in 3 places
    // (button text, button title, helper sentence), so target the
    // actual <button> via title to be unambiguous.
    const editBtn = screen.getByTitle('Modifier le plugin');
    await act(async () => {
      fireEvent.click(editBtn);
    });
    // The handler awaits revealSecrets BEFORE setting showAddMcp +
    // addMcpSelected → the form renders one microtask after the await
    // resolves. In EDIT mode the save button reads "Enregistrer les
    // modifications" (saveEdit i18n key), not just "Enregistrer".
    await screen.findByText(/Enregistrer les modifications/, undefined, { timeout: 2000 });
  };

  it('Modifier le plugin: opens the form pre-filled with spec values', async () => {
    vi.useRealTimers();
    (mcpsApi.revealSecrets as ReturnType<typeof vi.fn>).mockClear();
    await openEditDrawer('custom-example-abc12345', 'cfg-example-1');

    // Name + Base URL pre-filled.
    const nameInput = screen.getByPlaceholderText(/Salesforce Sales API/) as HTMLInputElement;
    expect(nameInput.value).toBe('ExampleAPI');
    const baseUrlInput = screen.getByPlaceholderText(/my-org\.salesforce\.com/) as HTMLInputElement;
    expect(baseUrlInput.value).toBe('https://api.example.com/v1');

    // Field label pre-filled from spec.config_keys.
    const labelInput = screen.getByPlaceholderText(/Bearer Token/) as HTMLInputElement;
    expect(labelInput.value).toBe('API Key');

    // Endpoint paths pre-filled (rendered as <input value="…">, so we
    // scrape the input values rather than textContent).
    const endpointPathValues = Array.from(
      document.querySelectorAll<HTMLInputElement>('input'),
    ).map(i => i.value);
    expect(endpointPathValues).toContain('/users');
    expect(endpointPathValues).toContain('/users/{id}');

    // 2026-06-09 UX fix: a stored secret renders as a read-only masked
    // indicator + "Remplacer" — NOT a pre-filled (un-round-trippable) nor a
    // blank (looks-wiped) input. So the user always sees a key exists. No
    // value input until they click Remplacer; revealSecrets is never called.
    expect(screen.getByText('Remplacer')).toBeTruthy();
    expect(screen.queryByPlaceholderText('Valeur')).toBeNull();
    expect(mcpsApi.revealSecrets).not.toHaveBeenCalled();
  });

  it('Modifier le plugin: the 👁 reveals the stored value read-only (coherent with the card)', async () => {
    // Coherence with the card: a stored field can be peeked via the eye —
    // fetched on demand (read-only display, never round-tripped on save).
    vi.useRealTimers();
    (mcpsApi.revealSecrets as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.revealSecrets as ReturnType<typeof vi.fn>).mockResolvedValue([
      { key: 'API_KEY', masked_value: 'sk-secret-123', secret: true },
    ]);
    await openEditDrawer('custom-example-abc12345', 'cfg-example-1');

    // Hidden by default — no plaintext on screen, no reveal call yet.
    expect(screen.queryByDisplayValue('sk-secret-123')).toBeNull();
    expect(mcpsApi.revealSecrets).not.toHaveBeenCalled();

    // Click the eye on the stored field → fetch + show the value read-only.
    fireEvent.click(screen.getByLabelText('Afficher'));
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });

    expect(mcpsApi.revealSecrets).toHaveBeenCalledWith('cfg-example-1');
    const revealed = screen.getByDisplayValue('sk-secret-123') as HTMLInputElement;
    expect(revealed.readOnly).toBe(true);
  });

  it('Modifier le plugin: Remplacer → typing a new value PATCHes the env', async () => {
    // "Modifier le plugin" is THE one place to edit structure AND
    // credentials (the card is read-only). Clicking "Remplacer" reveals an
    // empty input; typing a value replaces the stored key on save.
    vi.useRealTimers();
    (mcpsApi.updateCustomSpec as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.updateCustomSpec as ReturnType<typeof vi.fn>).mockResolvedValue({});
    (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mockResolvedValue({});

    await openEditDrawer('custom-example-abc12345', 'cfg-example-1');

    // No value input until the user explicitly chooses to replace.
    expect(screen.queryByPlaceholderText('Valeur')).toBeNull();
    fireEvent.click(screen.getByText('Remplacer'));
    const valueInput = screen.getByPlaceholderText('Valeur') as HTMLInputElement;
    expect(valueInput.value).toBe('');
    fireEvent.change(valueInput, { target: { value: 'sk-NEW-rotation' } });

    const saveBtn = screen.getByText(/Enregistrer les modifications/);
    fireEvent.click(saveBtn);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    // PUT spec + PATCH env (slugged env_key) both fire.
    expect(mcpsApi.updateCustomSpec).toHaveBeenCalledTimes(1);
    expect(mcpsApi.updateConfig).toHaveBeenCalledTimes(1);
    const [cfgId, envPayload] = (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mock.calls[0];
    expect(cfgId).toBe('cfg-example-1');
    expect(envPayload.env).toEqual({ API_KEY: 'sk-NEW-rotation' });
  });

  it('Modifier le plugin: not replacing keeps the stored key — no env PATCH (no wipe)', async () => {
    // The desync-killer: if the user doesn't click "Remplacer", the stored
    // secret is untouched — the save skips the env PATCH entirely.
    vi.useRealTimers();
    (mcpsApi.updateCustomSpec as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.updateCustomSpec as ReturnType<typeof vi.fn>).mockResolvedValue({});

    await openEditDrawer('custom-example-abc12345', 'cfg-example-2');

    // Don't click Remplacer — submit straight away.
    const saveBtn = screen.getByText(/Enregistrer les modifications/);
    fireEvent.click(saveBtn);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mcpsApi.updateCustomSpec).toHaveBeenCalledTimes(1);
    // Stored key untouched → env never patched.
    expect(mcpsApi.updateConfig).not.toHaveBeenCalled();
  });

  // ─── KT-831 — the scope block inside "Modifier le plugin" ───────────

  it('Modifier le plugin: the Global toggle reflects the real config (not always unchecked) and PATCHes on change', async () => {
    // Pre-KT-831 bug: the Global checkbox was never seeded from `cfg.is_global`
    // (always opened unchecked) and `updateCustomSpec` has no scope fields, so
    // toggling it silently did nothing on save.
    vi.useRealTimers();
    (mcpsApi.updateCustomSpec as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.setConfigProjects as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.updateCustomSpec as ReturnType<typeof vi.fn>).mockResolvedValue({});
    (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mockResolvedValue({});
    (mcpsApi.setConfigProjects as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);

    await openEditDrawer('custom-example-abc12345', 'cfg-example-3');

    const globalToggle = screen.getByRole('button', { name: 'Tous les projets' });
    expect(globalToggle).toHaveClass('mcp-project-toggle-off');
    fireEvent.click(globalToggle);

    const saveBtn = screen.getByText(/Enregistrer les modifications/);
    fireEvent.click(saveBtn);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mcpsApi.updateConfig).toHaveBeenCalledWith('cfg-example-3', {
      is_global: true,
      include_general: true,
    });
    expect(mcpsApi.setConfigProjects).toHaveBeenCalledWith('cfg-example-3', { project_ids: [] });
  });

  // ─── 0.8.6 (#60) Orphan env warning ──────────────────────────────────

  it('Modifier le plugin: prompts the user when updateCustomSpec reports orphan env keys', async () => {
    vi.useRealTimers();
    (mcpsApi.updateCustomSpec as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.cleanupOrphanEnv as ReturnType<typeof vi.fn>).mockClear();
    // Backend reports an orphan (e.g. another config of the same plugin
    // still carries OLD_API_KEY after the rename).
    (mcpsApi.updateCustomSpec as ReturnType<typeof vi.fn>).mockResolvedValue({
      server: {},
      orphan_env_keys: ['OLD_API_KEY'],
    });
    (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mockResolvedValue({});
    (mcpsApi.cleanupOrphanEnv as ReturnType<typeof vi.fn>).mockResolvedValue({
      configs_updated: 2,
      total_keys_removed: 2,
    });
    // User confirms the cleanup prompt.
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);

    await openEditDrawer('custom-example-abc12345', 'cfg-example-1');
    fireEvent.click(screen.getByText(/Enregistrer les modifications/));
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(confirmSpy).toHaveBeenCalled();
    expect(mcpsApi.cleanupOrphanEnv).toHaveBeenCalledWith(
      'custom-example-abc12345',
      ['OLD_API_KEY'],
    );
    confirmSpy.mockRestore();
  });

  it('Modifier le plugin: cleanup is SKIPPED when user dismisses the orphan prompt', async () => {
    vi.useRealTimers();
    (mcpsApi.updateCustomSpec as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.cleanupOrphanEnv as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.updateCustomSpec as ReturnType<typeof vi.fn>).mockResolvedValue({
      server: {},
      orphan_env_keys: ['OLD_API_KEY'],
    });
    (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mockResolvedValue({});
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);

    await openEditDrawer('custom-example-abc12345', 'cfg-example-1');
    fireEvent.click(screen.getByText(/Enregistrer les modifications/));
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(confirmSpy).toHaveBeenCalled();
    // Dismissed → cleanup NEVER called.
    expect(mcpsApi.cleanupOrphanEnv).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });

  it('Modifier le plugin: no orphan keys → no prompt at all', async () => {
    vi.useRealTimers();
    (mcpsApi.updateCustomSpec as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.cleanupOrphanEnv as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.updateCustomSpec as ReturnType<typeof vi.fn>).mockResolvedValue({
      server: {},
      orphan_env_keys: [],
    });
    (mcpsApi.updateConfig as ReturnType<typeof vi.fn>).mockResolvedValue({});
    const confirmSpy = vi.spyOn(window, 'confirm');

    await openEditDrawer('custom-example-abc12345', 'cfg-example-1');
    fireEvent.click(screen.getByText(/Enregistrer les modifications/));
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    // Happy path : empty orphan list → no confirm + no cleanup call.
    expect(confirmSpy).not.toHaveBeenCalled();
    expect(mcpsApi.cleanupOrphanEnv).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });

  it('Modifier le plugin: button is HIDDEN for non-custom plugins', () => {
    // Sanity guard: the edit button must NOT show for vendor-built
    // plugins (mcp-github, api-chartbeat, etc.) — those are owned by
    // the registry, not the user.
    const vendorServer: McpServer = {
      id: 'api-chartbeat',
      name: 'Chartbeat',
      description: 'Chartbeat',
      transport: 'ApiOnly',
      source: 'Registry',
      api_spec: {
        base_url: 'https://api.chartbeat.com',
        auth: 'None',
        endpoints: [],
        config_keys: [],
      },
    };
    const cfg = makeConfig('cfg-cb', 'api-chartbeat', 'Chartbeat');
    const overview: McpOverview = {
      servers: [vendorServer],
      configs: [cfg],
      customized_contexts: [],
      incompatibilities: [], incomplete_configs: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByRole('button', { name: 'Chartbeat — Voir les détails' }));
    expect(screen.queryByTitle('Modifier le plugin')).toBeNull();
  });

  // ─── 0.8.6 (#29) — Endpoints autodiscovery banner ─────────────────────
  //
  // On legacy Custom plugins (`server_id` startsWith `custom-`) whose
  // `api_spec.endpoints[]` is empty, the detail panel surfaces a banner
  // pushing the user toward the AI helper (re-uses the existing edit
  // form which embeds the CustomApiAiHelper). For registry plugins OR
  // for Custom plugins with declared endpoints, the banner stays hidden.

  it('autodiscovery banner: shown for Custom plugins with no endpoints', async () => {
    const server: McpServer = {
      id: 'custom-legacy-abc12345',
      name: 'LegacyAPI',
      description: 'A legacy custom plugin',
      transport: 'ApiOnly',
      source: 'Manual',
      api_spec: {
        base_url: 'https://api.legacy.com',
        auth: 'None',
        docs_url: 'https://docs.legacy.com',
        endpoints: [], // ← the trigger
        config_keys: [],
      },
    };
    const cfg = makeConfig('cfg-legacy', 'custom-legacy-abc12345', 'LegacyAPI');
    const overview: McpOverview = {
      servers: [server],
      configs: [cfg],
      customized_contexts: [],
      incompatibilities: [], incomplete_configs: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    openPlugin('LegacyAPI');
    const banner = document.querySelector('[data-testid="mcp-autodiscovery-banner"]');
    expect(banner).not.toBeNull();
    // Banner has the CTA button (uses Sparkles icon + i18n key).
    const ctaBtn = banner!.querySelector('.mcp-autodiscovery-banner-cta');
    expect(ctaBtn).not.toBeNull();
  });

  it('autodiscovery banner: HIDDEN for Custom plugins WITH endpoints declared', async () => {
    const server: McpServer = {
      id: 'custom-good-xyz98765',
      name: 'GoodAPI',
      description: 'Custom plugin already enriched',
      transport: 'ApiOnly',
      source: 'Manual',
      api_spec: {
        base_url: 'https://api.good.com',
        auth: 'None',
        endpoints: [
          { path: '/things', method: 'GET', description: 'List things' },
        ],
        config_keys: [],
      },
    };
    const cfg = makeConfig('cfg-good', 'custom-good-xyz98765', 'GoodAPI');
    const overview: McpOverview = {
      servers: [server],
      configs: [cfg],
      customized_contexts: [],
      incompatibilities: [], incomplete_configs: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    openPlugin('GoodAPI');
    expect(
      document.querySelector('[data-testid="mcp-autodiscovery-banner"]'),
    ).toBeNull();
  });

  // ─── KT-833 — one export/import flow (the bundle) and a labelled rescan ───

  const customOverview = (): McpOverview => {
    const server: McpServer = {
      id: 'custom-exportme-aaa11111',
      name: 'ExportMe',
      description: 'A custom plugin',
      transport: 'ApiOnly',
      source: 'Manual',
      api_spec: {
        base_url: 'https://api.exportme.com',
        auth: 'None',
        docs_url: null,
        endpoints: [],
        config_keys: [],
      },
    };
    return {
      servers: [server],
      configs: [makeConfig('cfg-exportme', 'custom-exportme-aaa11111', 'ExportMe')],
      customized_contexts: [],
      incompatibilities: [], incomplete_configs: [],
    };
  };

  it('a custom plugin has no per-plugin JSON export, only the bundle export', () => {
    wrap(<McpPage projects={[]} mcpOverview={customOverview()} mcpRegistry={[]} refetchMcps={noop} />);
    openPlugin('ExportMe');
    expect(document.querySelector('[data-testid="mcp-custom-export-json"]')).toBeNull();
    expect(document.querySelector('[data-testid="mcp-export-modal"]')).toBeNull();
  });

  it('the Add panel import tile opens the bundle import instead of a paste form', () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(getAddPluginButton());
    fireEvent.click(document.querySelector('[data-testid="mcp-import-bundle-tile"]') as HTMLElement);
    expect(document.querySelector('[data-testid="mcp-import-json-form"]')).toBeNull();
    expect(screen.getByRole('dialog', { name: 'Importer des plugins' })).toBeInTheDocument();
    expect(document.querySelector('.mcp-add-modal-backdrop')).toBeNull();
  });

  it('labels the toolbar rescan instead of calling it a sync', () => {
    wrap(<McpPage projects={[]} mcpOverview={customOverview()} mcpRegistry={[]} refetchMcps={noop} />);
    const toolbarButton = document.querySelector('.mcp-collection-toolbar button[aria-label="Rescanner les .mcp.json des projets"]');
    expect(toolbarButton).not.toBeNull();
    expect(screen.queryByRole('button', { name: 'Synchroniser' })).toBeNull();
  });

  it('reports created, merged, rewritten and removed after applying a rescan', async () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    vi.mocked(mcpsApi.refresh)
      .mockResolvedValueOnce({ dry_run: true, configs_created: 2, configs_merged: 1, configs_deleted: 3, projects_affected: 4, overview })
      .mockResolvedValueOnce({ dry_run: false, configs_created: 2, configs_merged: 1, configs_deleted: 3, projects_affected: 4, projects_rewritten: 5, overview });
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    await act(async () => { fireEvent.click(screen.getByTestId('mcp-rescan-preview-button')); });
    expect(screen.getByRole('region', { name: 'Aperçu du rescan' })).toHaveTextContent('4 projets concernés');
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Appliquer le rescan' })); });
    expect(screen.getByText(/2 créées, 1 fusionnées, 5 fichiers de projet réécrits, 3 doublons supprimés/)).toBeInTheDocument();
  });

  it('shows a rescan failure on screen and keeps the preview', async () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
    vi.mocked(mcpsApi.refresh)
      .mockResolvedValueOnce({ dry_run: true, configs_created: 0, configs_merged: 0, configs_deleted: 0, projects_affected: 0, overview })
      .mockRejectedValueOnce(new Error('disk full'));
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    await act(async () => { fireEvent.click(screen.getByTestId('mcp-rescan-preview-button')); });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Appliquer le rescan' })); });
    expect(screen.getByText(/Rescan impossible : .*disk full/)).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Aperçu du rescan' })).toBeInTheDocument();
  });


  it('autodiscovery banner: HIDDEN for registry (non-custom) plugins', async () => {
    // Registry plugins (mcp-github, api-chartbeat...) are owned by the
    // registry, not by the user — the banner has no business surfacing
    // on them, even when they have no endpoints (which would be a
    // registry catalog bug, not a user-actionable state).
    const server: McpServer = {
      id: 'api-chartbeat',
      name: 'Chartbeat',
      description: 'Chartbeat (registry)',
      transport: 'ApiOnly',
      source: 'Registry',
      api_spec: {
        base_url: 'https://api.chartbeat.com',
        auth: 'None',
        endpoints: [], // hypothetically empty
        config_keys: [],
      },
    };
    const cfg = makeConfig('cfg-cb', 'api-chartbeat', 'Chartbeat');
    const overview: McpOverview = {
      servers: [server],
      configs: [cfg],
      customized_contexts: [],
      incompatibilities: [], incomplete_configs: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    openPlugin('Chartbeat');
    expect(
      document.querySelector('[data-testid="mcp-autodiscovery-banner"]'),
    ).toBeNull();
  });

  // ── 0.8.6 phase 4 — type filter (MCP / API / CLI) on the Add MCP
  //    discovery panel (audit feedback 2026-05-22). Pins the contract :
  //    user can narrow the registry to one transport kind, the `cli`
  //    bucket isolates the wrapper plugins (Fastly, GitLab) that look
  //    like MCP but require a local CLI binary install.
  describe('Add MCP discovery — type filter (0.8.6 phase 4)', () => {
    const mcpServer: McpDefinition = {
      id: 'mcp-postgres', name: 'PostgreSQL',
      description: 'SQL',
      transport: { Stdio: { command: 'npx', args: ['-y', '@mcp/postgres'] } },
      env_keys: [], tags: ['database', 'sql'],
      token_url: null, token_help: null,
      publisher: 'Anthropic', official: false,
      api_spec: null,
    };
    const apiServer: McpDefinition = {
      id: 'api-resend', name: 'Resend',
      description: 'Transactional email',
      transport: 'ApiOnly',
      env_keys: [], tags: ['email', 'communication'],
      token_url: null, token_help: null,
      publisher: 'Resend', official: true,
      api_spec: { base_url: 'https://api.resend.com', auth: 'None',
        endpoints: [], config_keys: [] },
    };
    const cliServer: McpDefinition = {
      id: 'mcp-fastly', name: 'Fastly',
      description: 'CDN management',
      transport: { Stdio: { command: 'fastly-mcp', args: [] } },
      env_keys: [], tags: ['cli', 'cdn', 'cache'],
      token_url: null, token_help: null,
      publisher: 'Fastly', official: true,
      api_spec: null,
    };

    const openAddMcpPanel = async () => {
      // Locate the "Ajouter" / "Add" CTA via its stable test attribute :
      // `data-tour-id="add-plugin-btn"` was added for the onboarding
      // tour and survives localisation changes.
      await act(async () => { fireEvent.click(getAddPluginButton()); });
    };

    it('renders 4 filter pills (All / MCP / API / CLI) with All active by default', async () => {
      const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
      wrap(<McpPage projects={[]} mcpOverview={overview}
        mcpRegistry={[mcpServer, apiServer, cliServer]} refetchMcps={noop} />);
      await openAddMcpPanel();
      const all = document.querySelector('[data-testid="mcp-kind-filter-all"]');
      const mcp = document.querySelector('[data-testid="mcp-kind-filter-mcp"]');
      const api = document.querySelector('[data-testid="mcp-kind-filter-api"]');
      const cli = document.querySelector('[data-testid="mcp-kind-filter-cli"]');
      expect(all).toBeTruthy();
      expect(mcp).toBeTruthy();
      expect(api).toBeTruthy();
      expect(cli).toBeTruthy();
      expect(all?.getAttribute('data-active')).toBe('true');
      expect(cli?.getAttribute('data-active')).toBe('false');
    });

    it('CLI filter narrows registry to only plugins with the cli tag', async () => {
      const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
      wrap(<McpPage projects={[]} mcpOverview={overview}
        mcpRegistry={[mcpServer, apiServer, cliServer]} refetchMcps={noop} />);
      await openAddMcpPanel();
      // Click CLI filter.
      const cliBtn = document.querySelector('[data-testid="mcp-kind-filter-cli"]') as HTMLElement;
      await act(async () => { fireEvent.click(cliBtn); });
      // Fastly visible.
      expect(document.body.textContent).toContain('Fastly');
      // PostgreSQL (pure MCP) + Resend (API) hidden.
      expect(document.body.textContent).not.toContain('PostgreSQL');
      expect(document.body.textContent).not.toContain('Resend');
    });

    it('API filter narrows to ApiOnly plugins', async () => {
      const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
      wrap(<McpPage projects={[]} mcpOverview={overview}
        mcpRegistry={[mcpServer, apiServer, cliServer]} refetchMcps={noop} />);
      await openAddMcpPanel();
      const apiBtn = document.querySelector('[data-testid="mcp-kind-filter-api"]') as HTMLElement;
      await act(async () => { fireEvent.click(apiBtn); });
      expect(document.body.textContent).toContain('Resend');
      expect(document.body.textContent).not.toContain('PostgreSQL');
      expect(document.body.textContent).not.toContain('Fastly');
    });

    it('MCP filter narrows to non-CLI non-API plugins (pure MCP + hybrid)', async () => {
      const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
      wrap(<McpPage projects={[]} mcpOverview={overview}
        mcpRegistry={[mcpServer, apiServer, cliServer]} refetchMcps={noop} />);
      await openAddMcpPanel();
      const mcpBtn = document.querySelector('[data-testid="mcp-kind-filter-mcp"]') as HTMLElement;
      await act(async () => { fireEvent.click(mcpBtn); });
      expect(document.body.textContent).toContain('PostgreSQL');
      // CLI wrapper (Fastly) is bucketed separately and MUST NOT appear
      // under MCP filter — the whole point of the new type split.
      expect(document.body.textContent).not.toContain('Fastly');
      // API-only also excluded.
      expect(document.body.textContent).not.toContain('Resend');
    });

    it('pinned Custom API tile follows the kind filter (visible under All/API, hidden under MCP/CLI)', async () => {
      const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [] };
      wrap(<McpPage projects={[]} mcpOverview={overview}
        mcpRegistry={[mcpServer, apiServer, cliServer]} refetchMcps={noop} />);
      await openAddMcpPanel();
      const tile = () => document.querySelector('[data-tour-id="custom-api-tile"]');
      const click = async (kind: string) => {
        const btn = document.querySelector(`[data-testid="mcp-kind-filter-${kind}"]`) as HTMLElement;
        await act(async () => { fireEvent.click(btn); });
      };
      // Default (All): the Custom API tile is an API-only plugin → shown.
      expect(tile()).toBeTruthy();
      // MCP / CLI: it is NOT an MCP nor a CLI wrapper → hidden.
      await click('mcp');
      expect(tile()).toBeNull();
      await click('cli');
      expect(tile()).toBeNull();
      // API: shown again.
      await click('api');
      expect(tile()).toBeTruthy();
    });
  });
});
