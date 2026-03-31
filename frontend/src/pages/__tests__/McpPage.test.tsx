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
    deleteConfig: vi.fn(),
    setConfigProjects: vi.fn(),
    revealSecrets: vi.fn(),
    listContexts: vi.fn(),
    getContext: vi.fn(),
    updateContext: vi.fn(),
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
});
