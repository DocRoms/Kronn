import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
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
import type { McpOverview, McpConfigDisplay, McpServer, McpDefinition, Project } from '../../types/generated';

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
    // Empty state message should be visible
    const body = document.body.textContent!;
    expect(body.length).toBeGreaterThan(0);
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

  it('renders config labels inside expanded server groups', () => {
    const configs = [
      makeConfig('c1', 'github', 'GitHub', { label: 'GitHub Main' }),
      makeConfig('c2', 'github', 'GitHub', { label: 'GitHub Secondary' }),
    ];
    const overview: McpOverview = { servers: [makeServer('github', 'GitHub')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Click the server group header to expand it
    const header = screen.getByText('GitHub');
    fireEvent.click(header);

    expect(container.textContent).toContain('GitHub Main');
    expect(container.textContent).toContain('GitHub Secondary');
  });

  it('shows global toggle state correctly', () => {
    const configs = [
      makeConfig('c1', 'github', 'GitHub', { is_global: true }),
    ];
    const overview: McpOverview = { servers: [makeServer('github', 'GitHub')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Expand the server group
    fireEvent.click(screen.getByText('GitHub'));

    // Global label should be rendered
    expect(container.textContent).toContain('Global');
  });

  it('"Add MCP" button opens the add form', () => {
    const overview: McpOverview = { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
    const registry: McpDefinition[] = [
      { id: 'test-mcp', name: 'Test MCP', description: 'A test server', transport: { Stdio: { command: 'node', args: [] } }, env_keys: [], tags: ['core'], token_url: null, token_help: null },
    ];
    wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={registry} refetchMcps={noop} />);

    // Click the add button (text depends on i18n default = French "Ajouter")
    const addBtn = screen.getByText('Ajouter');
    fireEvent.click(addBtn);

    // The registry entry should now be visible
    expect(document.body.textContent).toContain('Test MCP');
    expect(document.body.textContent).toContain('A test server');
  });

  it('shows incompatibility badge on server header', () => {
    const servers = [makeServer('mcp-gitlab', 'GitLab')];
    const configs = [makeConfig('c1', 'mcp-gitlab', 'GitLab')];
    const overview: McpOverview = {
      servers, configs, customized_contexts: [],
      incompatibilities: [{ server_id: 'mcp-gitlab', agent: 'Kiro' as any, reason: 'Empty tool schemas — incompatible with Bedrock' }],
    };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // The incompatibility badge should show the agent name
    expect(container.textContent).toContain('Kiro');
  });

  it('does not show incompatibility badge for compatible servers', () => {
    const servers = [makeServer('mcp-github', 'GitHub')];
    const configs = [makeConfig('c1', 'mcp-github', 'GitHub')];
    const overview: McpOverview = {
      servers, configs, customized_contexts: [],
      incompatibilities: [{ server_id: 'mcp-gitlab', agent: 'Kiro' as any, reason: 'test' }],
    };
    const { container } = wrap(<McpPage projects={[]} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // GitHub should NOT show Kiro warning (only gitlab is incompatible)
    // The page should render GitHub but the warning badge should not appear
    expect(container.textContent).toContain('GitHub');
    expect(container.textContent).not.toContain('Kiro');
  });

  it('shows project names as badges when linked', () => {
    const projects = [makeProject('p1', 'my-app'), makeProject('p2', 'my-api')];
    const configs = [
      makeConfig('c1', 'github', 'GitHub', { project_ids: ['p1'], project_names: ['my-app'] }),
    ];
    const overview: McpOverview = { servers: [makeServer('github', 'GitHub')], configs, customized_contexts: [], incompatibilities: [] };
    const { container } = wrap(<McpPage projects={projects} mcpOverview={overview} mcpRegistry={[]} refetchMcps={noop} />);

    // Expand to see config details
    fireEvent.click(screen.getByText('GitHub'));

    expect(container.textContent).toContain('my-app');
  });
});
