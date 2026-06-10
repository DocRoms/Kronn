// Note: assertions use French strings because the default UI locale is 'fr'.
// If the default locale changes, these assertions must be updated.
import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, cleanup, fireEvent, act } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock API
vi.mock('../../lib/api', () => ({
  mcps: {
    overview: vi.fn().mockResolvedValue({ servers: [], configs: [], project_links: [], customized_contexts: [], incompatibilities: [] }),
    registry: vi.fn().mockResolvedValue([]),
    refresh: vi.fn(),
    createConfig: vi.fn(),
    updateConfig: vi.fn(),
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
}));

import { McpPage } from '../McpPage';
import { mcps as mcpsApi } from '../../lib/api';
import type { McpOverview, McpConfigDisplay, McpServer, McpDefinition, Project, AgentType } from '../../types/generated';

// Use fake timers to prevent the setTimeout in handleAddDuplicateConfig (50ms
// scroll animation) from leaking across tests and causing timeout issues.
beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
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
  include_general: false,
  config_hash: 'abc123',
  project_ids: opts?.project_ids ?? [],
  project_names: opts?.project_names ?? [],
  secrets_broken: opts?.secrets_broken ?? false,
  host_sync: opts?.host_sync ?? 'None',
});

const makeProject = (id: string, name: string): Project => ({
  id,
  name,
  path: `/repos/${name}`,
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
});

const wrap = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

describe('McpPage', () => {
  it('renders empty state when no configs exist', () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    // Empty state message (mcp.empty in FR: "Aucun plugin configure...") should be visible
    const body = document.body.textContent!;
    expect(body).toContain('Aucun plugin');
  });

  it('renders server names as group headers', () => {
    const servers = [makeServer('github', 'GitHub'), makeServer('slack', 'Slack')];
    const configs = [
      makeConfig('c1', 'github', 'GitHub'),
      makeConfig('c2', 'slack', 'Slack'),
    ];
    const overview: McpOverview = { servers, configs, customized_contexts: [], incompatibilities: [] };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    expect(screen.getByText('GitHub')).toBeTruthy();
    expect(screen.getByText('Slack')).toBeTruthy();
  });

  it('renders config labels as cards', () => {
    const configs = [
      makeConfig('c1', 'github', 'GitHub', { label: 'GitHub Main' }),
      makeConfig('c2', 'github', 'GitHub', { label: 'GitHub Secondary' }),
    ];
    const overview: McpOverview = { servers: [makeServer('github', 'GitHub')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Cards are always visible (no accordion)
    expect(container.textContent).toContain('GitHub Main');
    expect(container.textContent).toContain('GitHub Secondary');
  });

  it('shows global scope badge on global config', () => {
    const configs = [
      makeConfig('c1', 'github', 'GitHub', { is_global: true }),
    ];
    const overview: McpOverview = { servers: [makeServer('github', 'GitHub')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Global badge should be rendered on the card
    expect(container.textContent).toContain('Global');
  });

  it('"Add MCP" button opens the add form', () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
    const registry: McpDefinition[] = [
      { id: 'test-mcp', name: 'Test MCP', description: 'A test server', transport: { Stdio: { command: 'node', args: [] } }, env_keys: [], tags: ['core'], token_url: null, token_help: null, publisher: 'Anthropic', official: false },
    ];
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={registry} refetchMcps={noop} />);

    // Click the add button (text depends on i18n default = French "Ajouter")
    const addBtn = screen.getByText('Ajouter');
    fireEvent.click(addBtn);

    // The registry entry should now be visible
    expect(document.body.textContent).toContain('Test MCP');
    expect(document.body.textContent).toContain('A test server');
  });

  it('shows incompatibility badge in detail panel when card is clicked', () => {
    const servers = [makeServer('mcp-gitlab', 'GitLab')];
    const configs = [makeConfig('c1', 'mcp-gitlab', 'GitLab')];
    const overview: McpOverview = {
      servers, configs, customized_contexts: [],
      incompatibilities: [{ server_id: 'mcp-gitlab', agent: 'Kiro' as AgentType, reason: 'Empty tool schemas — incompatible with Bedrock' }],
    };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Click the plugin card to open the detail panel
    fireEvent.click(screen.getByText('GitLab'));

    // The incompatibility badge should show the agent name in the detail panel
    expect(container.textContent).toContain('Kiro');
  });

  it('does not show incompatibility badge for compatible servers', () => {
    const servers = [makeServer('mcp-github', 'GitHub')];
    const configs = [makeConfig('c1', 'mcp-github', 'GitHub')];
    const overview: McpOverview = {
      servers, configs, customized_contexts: [],
      incompatibilities: [{ server_id: 'mcp-gitlab', agent: 'Kiro' as AgentType, reason: 'test' }],
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
    const overview: McpOverview = { servers: [makeServer('github', 'GitHub')], configs, customized_contexts: [], incompatibilities: [] };
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
    const overview: McpOverview = { servers, configs, customized_contexts: [], incompatibilities: [] };
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
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Card shows key count (2 env keys)
    expect(container.textContent).toContain('2');
  });

  it('shows env vars section with edit pencil button in detail panel', () => {
    const configs = [
      makeConfig('c1', 'gitlab', 'GitLab', { env_keys: ['GITLAB_TOKEN'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Click card to expand detail panel
    fireEvent.click(screen.getByText('GitLab'));

    // Env vars section title should be visible
    expect(container.textContent).toContain("Variables d'environnement");
    // Edit pencil button should exist (title = "Modifier les clés")
    const editBtn = container.querySelector('button[title="Modifier les clés"]');
    expect(editBtn).toBeTruthy();
  });

  it('shows eye icon for each env field in detail panel (before entering edit mode)', () => {
    const configs = [
      makeConfig('c1', 'gitlab', 'GitLab', { env_keys: ['GITLAB_API_URL', 'GITLAB_TOKEN'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail panel
    fireEvent.click(screen.getByText('GitLab'));

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
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail panel
    fireEvent.click(screen.getByText('GitLab'));

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
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail panel
    fireEvent.click(screen.getByText('GitLab'));

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
    const overview: McpOverview = { servers: [makeServer('test', 'TestMCP')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail panel
    fireEvent.click(screen.getByText('TestMCP'));

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
    const overview: McpOverview = { servers: [makeServer('test', 'TestMCP')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail, enter edit mode
    fireEvent.click(screen.getByText('TestMCP'));
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
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail
    fireEvent.click(screen.getByText('GitLab'));

    // Field labels should be visible
    expect(container.textContent).toContain('GITLAB_API_URL');
    expect(container.textContent).toContain('GITLAB_PERSONAL_ACCESS_TOKEN');
  });

  it('shows warning and enters edit mode when revealSecrets fails', async () => {
    vi.mocked(mcpsApi.revealSecrets).mockRejectedValue(new Error('Decryption failed'));
    const configs = [
      makeConfig('c1', 'gitlab', 'GitLab', { env_keys: ['GITLAB_TOKEN'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('gitlab', 'GitLab')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail panel
    fireEvent.click(screen.getByText('GitLab'));

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
    const overview: McpOverview = { servers: [makeServer('test', 'TestMCP')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open detail panel
    fireEvent.click(screen.getByText('TestMCP'));

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
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
    const registry: McpDefinition[] = [
      { id: 'mcp-fastly', name: 'Fastly', description: 'CDN server', transport: { Stdio: { command: 'fastly-mcp', args: [] } }, env_keys: [], tags: ['cdn'], token_url: null, token_help: null, publisher: 'Fastly', official: true },
    ];
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={registry} refetchMcps={noop} />);
    fireEvent.click(screen.getByText('Ajouter'));
    expect(document.body.textContent).toContain('Officiel');
    expect(document.body.textContent).toContain('Fastly');
  });

  it('shows community badge for third-party MCP in registry', () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
    const registry: McpDefinition[] = [
      { id: 'mcp-github', name: 'GitHub', description: 'GitHub server', transport: { Stdio: { command: 'npx', args: ['-y', 'server'] } }, env_keys: ['TOKEN'], tags: ['git'], token_url: null, token_help: null, publisher: 'Anthropic', official: false },
    ];
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={registry} refetchMcps={noop} />);
    fireEvent.click(screen.getByText('Ajouter'));
    expect(document.body.textContent).toContain('Communautaire');
    expect(document.body.textContent).toContain('Anthropic');
  });

  it('shows publisher badge in detail panel of installed MCP', () => {
    const servers = [makeServer('mcp-redis', 'Redis')];
    const configs = [makeConfig('c1', 'mcp-redis', 'Redis')];
    const overview: McpOverview = { servers, configs, customized_contexts: [], incompatibilities: [] };
    const registry: McpDefinition[] = [
      { id: 'mcp-redis', name: 'Redis', description: 'Cache server', transport: { Stdio: { command: 'uvx', args: ['redis-mcp'] } }, env_keys: [], tags: ['cache'], token_url: null, token_help: null, publisher: 'Redis Ltd', official: true },
    ];
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={registry} refetchMcps={noop} />);
    fireEvent.click(screen.getByText('Redis'));
    expect(container.textContent).toContain('Officiel');
    expect(container.textContent).toContain('Redis Ltd');
  });

  // ─── Delete confirmation (regression: previously fired on click) ──────
  it('Delete config button asks for confirmation before deleting', async () => {
    const servers = [makeServer('mcp-redis', 'Redis')];
    const configs = [makeConfig('c1', 'mcp-redis', 'Redis')];
    const overview: McpOverview = { servers, configs, customized_contexts: [], incompatibilities: [] };

    // Reject the confirm dialog → handleDeleteMcpConfig must NOT call the API.
    // happy-dom doesn't ship `window.confirm`, so install a stub before spying.
    window.confirm = vi.fn();
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByText('Redis'));

    const deleteBtn = screen.getByText(/Supprimer cette config/);
    await act(async () => { fireEvent.click(deleteBtn); });

    expect(confirmSpy).toHaveBeenCalled();
    expect(mcpsApi.deleteConfig).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });

  it('Delete confirmed → API called + success toast', async () => {
    const servers = [makeServer('mcp-redis', 'Redis')];
    const configs = [makeConfig('c1', 'mcp-redis', 'Redis')];
    const overview: McpOverview = { servers, configs, customized_contexts: [], incompatibilities: [] };

    window.confirm = vi.fn();
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
    vi.mocked(mcpsApi.deleteConfig).mockResolvedValue(undefined);

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByText('Redis'));

    const deleteBtn = screen.getByText(/Supprimer cette config/);
    await act(async () => {
      fireEvent.click(deleteBtn);
      await Promise.resolve();
    });

    expect(confirmSpy).toHaveBeenCalled();
    expect(mcpsApi.deleteConfig).toHaveBeenCalledWith('c1');
    confirmSpy.mockRestore();
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
    const overview: McpOverview = { servers, configs, customized_contexts: [], incompatibilities: [] };
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
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
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
    const addBtn = screen.getByText(/Ajouter$/);
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
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
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

    fireEvent.click(screen.getByText(/Ajouter$/));
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
      incompatibilities: [],
    };

    (mcpsApi.revealSecrets as ReturnType<typeof vi.fn>).mockResolvedValue([
      { key: 'API_KEY', masked_value: 'sk-secret-123', secret: true },
    ]);

    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Open the detail panel — the card-level div carries the click
    // handler. The plugin name also appears in the host-discovery
    // banner ("Configurer 'ExampleAPI' →"), so we target the actual
    // installed-card container by class to disambiguate.
    const installedCard = document.querySelector('.mcp-installed-card') as HTMLElement | null;
    expect(installedCard).toBeTruthy();
    fireEvent.click(installedCard!);

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
      incompatibilities: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    const installedCard = document.querySelector('.mcp-installed-card') as HTMLElement | null;
    expect(installedCard).toBeTruthy();
    fireEvent.click(installedCard!);
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
      incompatibilities: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByText('LegacyAPI'));
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
      incompatibilities: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByText('GoodAPI'));
    expect(
      document.querySelector('[data-testid="mcp-autodiscovery-banner"]'),
    ).toBeNull();
  });

  // ─── 0.8.6 (#33) — Custom plugin clipboard-JSON import/export ─────────
  //
  // Export: on a Custom plugin detail panel, a "Copier comme JSON" button
  // serializes the spec (no credentials) to the clipboard.
  // Import: an "Importer depuis JSON" tile in the registry grid switches
  // the Add panel to a paste-area; on submit it POSTs the parsed spec via
  // `createConfig({ server_id: 'api-custom', custom_spec: …, env: {} })`.

  it('export button: writes spec-only JSON to clipboard on Custom plugins', async () => {
    const server: McpServer = {
      id: 'custom-exportme-aaa11111',
      name: 'ExportMe',
      description: 'A custom plugin to export',
      transport: 'ApiOnly',
      source: 'Manual',
      api_spec: {
        base_url: 'https://api.exportme.com',
        auth: 'None',
        docs_url: 'https://docs.exportme.com',
        endpoints: [{ path: '/things', method: 'GET', description: 'List' }],
        config_keys: [
          { label: 'API Key', env_key: 'EXPORTME_API_KEY', placeholder: '', description: '' },
        ],
      },
    };
    const cfg = makeConfig('cfg-exportme', 'custom-exportme-aaa11111', 'ExportMe');
    const overview: McpOverview = {
      servers: [server],
      configs: [cfg],
      customized_contexts: [],
      incompatibilities: [],
    };
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByText('ExportMe'));
    const exportBtn = document.querySelector('[data-testid="mcp-custom-export-json"]') as HTMLButtonElement | null;
    expect(exportBtn).not.toBeNull();
    await act(async () => {
      fireEvent.click(exportBtn!);
      await Promise.resolve();
      await Promise.resolve();
    });
    // 0.8.6 fix 2026-05-21 — export now ALSO renders an inline modal
    // with the JSON in a readonly textarea, regardless of clipboard
    // outcome. Pre-fix the export was clipboard-only and silently
    // dead in Tauri webviews ("STRICTEMENT rien" — user, 2026-05-21).
    const modal = document.querySelector('[data-testid="mcp-export-modal"]');
    expect(modal).not.toBeNull();
    const textarea = document.querySelector('[data-testid="mcp-export-modal-textarea"]') as HTMLTextAreaElement | null;
    expect(textarea).not.toBeNull();
    const parsedFromTextarea = JSON.parse(textarea!.value);
    expect(parsedFromTextarea.name).toBe('ExportMe');
    expect(parsedFromTextarea.base_url).toBe('https://api.exportme.com');
    expect(parsedFromTextarea.endpoints).toEqual([
      { path: '/things', method: 'GET', description: 'List' },
    ]);
    // Critical contract: fields[].value MUST be '' so credentials never leak.
    expect(parsedFromTextarea.fields).toEqual([{ label: 'API Key', value: '' }]);
    // Clipboard write IS still attempted in the background (best-effort).
    // Same shape goes to the clipboard helper as into the modal.
    expect(writeText).toHaveBeenCalled();
    expect(JSON.parse(writeText.mock.calls[0][0])).toEqual(parsedFromTextarea);
  });

  it('export modal: closes via the X button without leaking state', async () => {
    const server: McpServer = {
      id: 'custom-modal-bbb22222',
      name: 'ModalMe',
      description: 'For the close-button test',
      transport: 'ApiOnly',
      source: 'Manual',
      api_spec: {
        base_url: 'https://api.modalme.com',
        auth: 'None',
        endpoints: [],
        config_keys: [],
      },
    };
    const cfg = makeConfig('cfg-modal', 'custom-modal-bbb22222', 'ModalMe');
    const overview: McpOverview = {
      servers: [server],
      configs: [cfg],
      customized_contexts: [],
      incompatibilities: [],
    };
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: vi.fn().mockResolvedValue(undefined) }, configurable: true });
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByText('ModalMe'));
    await act(async () => {
      fireEvent.click(document.querySelector('[data-testid="mcp-custom-export-json"]') as HTMLButtonElement);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(document.querySelector('[data-testid="mcp-export-modal"]')).not.toBeNull();
    fireEvent.click(document.querySelector('[data-testid="mcp-export-modal-close"]') as HTMLButtonElement);
    expect(document.querySelector('[data-testid="mcp-export-modal"]')).toBeNull();
  });

  it('export modal: survives a clipboard failure (still renders the JSON)', async () => {
    const server: McpServer = {
      id: 'custom-failclip-ccc33333',
      name: 'FailClip',
      description: 'Clipboard always rejects',
      transport: 'ApiOnly',
      source: 'Manual',
      api_spec: {
        base_url: 'https://api.failclip.com',
        auth: 'None',
        endpoints: [],
        config_keys: [],
      },
    };
    const cfg = makeConfig('cfg-failclip', 'custom-failclip-ccc33333', 'FailClip');
    const overview: McpOverview = {
      servers: [server],
      configs: [cfg],
      customized_contexts: [],
      incompatibilities: [],
    };
    // Clipboard rejects (Tauri sandboxed webview case).
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockRejectedValue(new Error('permission denied')) },
      configurable: true,
    });
    // Also force execCommand fallback to fail so we exercise the "failed" UI state.
    const origExec = document.execCommand;
    document.execCommand = vi.fn().mockReturnValue(false) as unknown as typeof document.execCommand;
    try {
      wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
      fireEvent.click(screen.getByText('FailClip'));
      await act(async () => {
        fireEvent.click(document.querySelector('[data-testid="mcp-custom-export-json"]') as HTMLButtonElement);
        await Promise.resolve();
        await Promise.resolve();
      });
      // Modal MUST still render — that's the whole point of the fix.
      expect(document.querySelector('[data-testid="mcp-export-modal"]')).not.toBeNull();
      const textarea = document.querySelector('[data-testid="mcp-export-modal-textarea"]') as HTMLTextAreaElement;
      expect(JSON.parse(textarea.value).name).toBe('FailClip');
    } finally {
      document.execCommand = origExec;
    }
  });

  it('export button: HIDDEN on registry (non-custom) plugins', async () => {
    const server: McpServer = {
      id: 'api-chartbeat',
      name: 'Chartbeat',
      description: 'Chartbeat (registry)',
      transport: 'ApiOnly',
      source: 'Registry',
      api_spec: { base_url: 'https://api.chartbeat.com', auth: 'None', endpoints: [], config_keys: [] },
    };
    const cfg = makeConfig('cfg-cb2', 'api-chartbeat', 'Chartbeat');
    const overview: McpOverview = {
      servers: [server],
      configs: [cfg],
      customized_contexts: [],
      incompatibilities: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByText('Chartbeat'));
    expect(document.querySelector('[data-testid="mcp-custom-export-json"]')).toBeNull();
  });

  it('import tile: switches Add panel to JSON paste form', async () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    // Open the Add MCP panel (button labeled with FR "Ajouter un plugin").
    const addBtn = Array.from(document.querySelectorAll('button')).find(b =>
      (b.textContent ?? '').toLowerCase().includes('ajouter')
    );
    expect(addBtn).toBeTruthy();
    fireEvent.click(addBtn!);
    const importTile = document.querySelector('[data-testid="mcp-import-json-tile"]') as HTMLElement | null;
    expect(importTile).not.toBeNull();
    fireEvent.click(importTile!);
    expect(document.querySelector('[data-testid="mcp-import-json-form"]')).not.toBeNull();
    expect(document.querySelector('[data-testid="mcp-import-json-textarea"]')).not.toBeNull();
  });

  it('import: POSTs createConfig with parsed custom_spec on valid JSON', async () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
    (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mockClear();
    (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mockResolvedValueOnce({});
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    const addBtn = Array.from(document.querySelectorAll('button')).find(b =>
      (b.textContent ?? '').toLowerCase().includes('ajouter')
    );
    fireEvent.click(addBtn!);
    fireEvent.click(document.querySelector('[data-testid="mcp-import-json-tile"]') as HTMLElement);
    const textarea = document.querySelector('[data-testid="mcp-import-json-textarea"]') as HTMLTextAreaElement;
    const validJson = JSON.stringify({
      name: 'ImportedAPI',
      base_url: 'https://api.imported.test',
      description: 'Imported plugin',
      docs_url: 'https://docs.imported.test',
      fields: [
        { label: 'API Key', value: 'should-be-discarded' },
      ],
      endpoints: [
        { path: '/items', method: 'GET', description: 'List items' },
      ],
      auth: 'None',
    });
    fireEvent.change(textarea, { target: { value: validJson } });
    const submitBtn = document.querySelector('[data-testid="mcp-import-submit"]') as HTMLButtonElement;
    await act(async () => {
      fireEvent.click(submitBtn);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(mcpsApi.createConfig).toHaveBeenCalledTimes(1);
    const payload = (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(payload.server_id).toBe('api-custom');
    expect(payload.label).toBe('ImportedAPI');
    expect(payload.custom_spec.name).toBe('ImportedAPI');
    expect(payload.custom_spec.endpoints).toHaveLength(1);
    // Critical contract: imported values must be stripped (never planted).
    expect(payload.custom_spec.fields).toEqual([{ label: 'API Key', value: '' }]);
  });

  it('import: surfaces a parse error for invalid JSON', async () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
    const callsBefore = (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mock.calls.length;
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    const addBtn = Array.from(document.querySelectorAll('button')).find(b =>
      (b.textContent ?? '').toLowerCase().includes('ajouter')
    );
    fireEvent.click(addBtn!);
    fireEvent.click(document.querySelector('[data-testid="mcp-import-json-tile"]') as HTMLElement);
    const textarea = document.querySelector('[data-testid="mcp-import-json-textarea"]') as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: '{ not valid json' } });
    fireEvent.click(document.querySelector('[data-testid="mcp-import-submit"]') as HTMLButtonElement);
    expect(document.querySelector('[data-testid="mcp-import-error"]')).not.toBeNull();
    // Note: createConfig may have been called by earlier tests; we only
    // assert that the parse-error branch did NOT trigger a new call.
    expect((mcpsApi.createConfig as ReturnType<typeof vi.fn>).mock.calls.length).toBe(callsBefore);
  });

  it('import: rejects JSON missing required fields (name)', async () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
    const callsBefore = (mcpsApi.createConfig as ReturnType<typeof vi.fn>).mock.calls.length;
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    const addBtn = Array.from(document.querySelectorAll('button')).find(b =>
      (b.textContent ?? '').toLowerCase().includes('ajouter')
    );
    fireEvent.click(addBtn!);
    fireEvent.click(document.querySelector('[data-testid="mcp-import-json-tile"]') as HTMLElement);
    const textarea = document.querySelector('[data-testid="mcp-import-json-textarea"]') as HTMLTextAreaElement;
    fireEvent.change(textarea, { target: { value: JSON.stringify({ base_url: 'https://x.test' }) } });
    fireEvent.click(document.querySelector('[data-testid="mcp-import-submit"]') as HTMLButtonElement);
    expect(document.querySelector('[data-testid="mcp-import-error"]')).not.toBeNull();
    // Note: createConfig may have been called by earlier tests; we only
    // assert that the parse-error branch did NOT trigger a new call.
    expect((mcpsApi.createConfig as ReturnType<typeof vi.fn>).mock.calls.length).toBe(callsBefore);
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
      incompatibilities: [],
    };
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);
    fireEvent.click(screen.getByText('Chartbeat'));
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
      const addBtn = document.querySelector('[data-tour-id="add-plugin-btn"]') as HTMLElement | null;
      if (!addBtn) throw new Error('Add MCP button not found in DOM');
      await act(async () => { fireEvent.click(addBtn); });
    };

    it('renders 4 filter pills (All / MCP / API / CLI) with All active by default', async () => {
      const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
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
      const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
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
      const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
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
      const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
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
      const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
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
