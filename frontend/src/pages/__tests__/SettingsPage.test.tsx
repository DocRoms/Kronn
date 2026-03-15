import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, act, cleanup } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock API
vi.mock('../../lib/api', () => ({
  setAuthToken: vi.fn(),
  config: {
    getTokens: vi.fn().mockResolvedValue({ keys: [], overrides: {} }),
    dbInfo: vi.fn().mockResolvedValue({
      size_bytes: 1024,
      project_count: 5,
      discussion_count: 12,
      message_count: 150,
      mcp_count: 3,
      workflow_count: 2,
      workflow_run_count: 8,
      custom_skill_count: 4,
      custom_profile_count: 2,
      custom_directive_count: 1,
    }),
    getScanDepth: vi.fn().mockResolvedValue(4),
    getScanPaths: vi.fn().mockResolvedValue(['/home/user/repos']),
    getScanIgnore: vi.fn().mockResolvedValue(['node_modules', '.git']),
    getServerConfig: vi.fn().mockResolvedValue({ host: '127.0.0.1', port: 3140, domain: null, max_concurrent_agents: 5, auth_enabled: true }),
    setServerConfig: vi.fn().mockResolvedValue(undefined),
    regenerateAuthToken: vi.fn().mockResolvedValue('new-token-456'),
    saveApiKey: vi.fn(),
    deleteApiKey: vi.fn(),
    activateApiKey: vi.fn(),
    syncAgentTokens: vi.fn(),
    toggleTokenOverride: vi.fn(),
    getLanguage: vi.fn(),
    saveLanguage: vi.fn(),
    setScanDepth: vi.fn(),
    setScanPaths: vi.fn(),
    setScanIgnore: vi.fn(),
    getAgentAccess: vi.fn(),
    setAgentAccess: vi.fn(),
    exportData: vi.fn(),
    importData: vi.fn(),
    discoverKeys: vi.fn().mockResolvedValue({ discovered: [], imported_count: 0 }),
  },
  agents: {
    detect: vi.fn(),
    install: vi.fn(),
    uninstall: vi.fn(),
    toggle: vi.fn(),
  },
  stats: {
    agentUsage: vi.fn().mockResolvedValue([
      { agent_type: 'ClaudeCode', total_tokens: 5000, message_count: 10, by_project: [] },
    ]),
  },
  skills: {
    list: vi.fn().mockResolvedValue([
      { id: 'rust', name: 'Rust', description: 'Systems programming', icon: 'Zap', category: 'Language', content: 'Be concise.', is_builtin: true },
      { id: 'custom-security', name: 'Security', description: 'Security auditing', icon: 'Shield', category: 'Domain', content: 'Focus on security.', is_builtin: false },
    ]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
  projects: {
    setDefaultSkills: vi.fn().mockResolvedValue(true),
    setDefaultProfile: vi.fn().mockResolvedValue(true),
  },
  profiles: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
    updatePersonaName: vi.fn(),
  },
  directives: {
    list: vi.fn().mockResolvedValue([
      { id: 'dir-terse', name: 'Terse', description: 'Short answers', icon: 'MessageSquare', category: 'Output', content: 'Be brief.', is_builtin: true, conflicts: ['dir-verbose'] },
    ]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
}));

import { SettingsPage } from '../SettingsPage';
import type { AgentsConfig, AgentDetection } from '../../types/generated';

const noop = () => {};
const toastFn = vi.fn() as any;

const sampleAgent: AgentDetection = {
  name: 'Claude Code',
  agent_type: 'ClaudeCode',
  installed: true,
  enabled: true,
  path: '/usr/bin/claude',
  version: '1.0.0',
  latest_version: null,
  origin: 'host',
  install_command: null,
  host_managed: false,
  host_label: null,
  runtime_available: false,
};

afterEach(cleanup);

const wrap = async (ui: React.ReactElement) => {
  let result: ReturnType<typeof render>;
  await act(async () => {
    result = render(<I18nProvider>{ui}</I18nProvider>);
  });
  return result!;
};

const defaultProps = {
  agents: [] as AgentDetection[],
  agentAccess: null as AgentsConfig | null,
  configLanguage: null as string | null,
  projects: [],
  refetchAgents: noop,
  refetchAgentAccess: noop,
  refetchLanguage: noop,
  refetchProjects: noop,
  refetchDiscussions: noop,
  onReset: noop,
  toast: toastFn,
};

describe('SettingsPage', () => {
  it('renders all main sections', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    const body = document.body.textContent!;

    // Configuration heading
    expect(body).toContain('Configuration');
    // Database section
    expect(body).toContain('Base de donnees');
    // Agents section
    expect(body).toContain('Agents');
    // Skills section
    expect(body).toContain('Skills');
    // Directives section
    expect(body).toContain('Directives');
    // Profiles section
    expect(body).toContain('Profils agent');
  });

  it('renders skill cards with name and description', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    const body = document.body.textContent!;

    expect(body).toContain('Rust');
    expect(body).toContain('Systems programming');
    expect(body).toContain('Security');
    expect(body).toContain('Security auditing');
  });

  it('renders directive cards with name, description, and conflicts', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    const body = document.body.textContent!;

    expect(body).toContain('Terse');
    expect(body).toContain('Short answers');
  });

  it('DB info shows all counters when > 0', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    const body = document.body.textContent!;

    // Check counter values are rendered
    expect(body).toContain('5');   // project_count
    expect(body).toContain('12');  // discussion_count
    expect(body).toContain('150'); // message_count
    expect(body).toContain('3');   // mcp_count

    // Check labels (French default)
    expect(body).toContain('Projets');
    expect(body).toContain('Discussions');
    expect(body).toContain('Messages');
    expect(body).toContain('MCPs');
    expect(body).toContain('Workflows');
    expect(body).toContain('Skills custom');
    expect(body).toContain('Profils custom');
    expect(body).toContain('Directives custom');
  });

  it('export button exists', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    expect(screen.getByText('Exporter')).toBeTruthy();
  });

  it('renders scan configuration sections in the same card', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    const body = document.body.textContent!;
    expect(body).toContain('Profondeur de scan');
    expect(body).toContain('Dossiers a scanner');
    expect(body).toContain('Patterns a ignorer');
  });

  it('renders agent token usage when agents are detected', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    const body = document.body.textContent!;
    expect(body).toContain('Estimation tokens');
    expect(body).toContain('5,000 tok');
  });

  it('renders the auto-detect button for API keys', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    expect(screen.getByText('Auto-detecter')).toBeDefined();
  });
});
