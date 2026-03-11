import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, act, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock API — SettingsPage calls config.getTokens(), config.dbInfo(), config.getScanDepth(), stats.agentUsage(), skills.list() on mount
vi.mock('../../lib/api', () => ({
  config: {
    getTokens: vi.fn().mockResolvedValue({ keys: [], overrides: {} }),
    dbInfo: vi.fn().mockResolvedValue({ path: '/tmp/db', size_bytes: 1024 }),
    getScanDepth: vi.fn().mockResolvedValue(4),
    getScanPaths: vi.fn().mockResolvedValue(['/home/user/repos']),
    getScanIgnore: vi.fn().mockResolvedValue(['node_modules', '.git']),
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
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
  projects: {
    setDefaultSkills: vi.fn().mockResolvedValue(true),
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
  it('renders without crashing with minimal props', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    expect(document.body.textContent).toBeDefined();
  });

  it('renders with agentAccess set', async () => {
    const agentAccess: AgentsConfig = {
      claude_code: { path: null, installed: true, version: null, full_access: true },
      codex: { path: null, installed: false, version: null, full_access: false },
      gemini_cli: { path: null, installed: false, version: null, full_access: false },
      kiro: { path: null, installed: false, version: null, full_access: false },
      vibe: { path: null, installed: false, version: null, full_access: false },
    };
    await wrap(<SettingsPage {...defaultProps} agentAccess={agentAccess} configLanguage="en" />);
    expect(document.body.textContent).toBeDefined();
  });

  it('renders with agents detected', async () => {
    await wrap(
      <SettingsPage {...defaultProps} agents={[sampleAgent]} configLanguage="fr" />
    );
    expect(document.body.textContent).toBeDefined();
  });

  // ─── Scan sections merged into one card ──────────────────────────────────

  it('renders scan depth, paths and ignore in the same card', async () => {
    await wrap(<SettingsPage {...defaultProps} />);
    // All three sub-sections should be present
    const body = document.body.textContent!;
    expect(body).toContain('Profondeur de scan');
    expect(body).toContain('Dossiers a scanner');
    expect(body).toContain('Patterns a ignorer');

    // They should be inside the SAME card: find the card-level heading,
    // go up to the card container, and verify all sub-sections are there.
    const scanPathsHeadings = screen.getAllByText('Dossiers a scanner');
    // The card-level heading is the first one (14px), the sub-section is the second (12px)
    const cardHeading = scanPathsHeadings[0];
    const card = cardHeading.closest('div[style]')?.parentElement;
    expect(card).toBeTruthy();
    expect(card!.textContent).toContain('Profondeur de scan');
    expect(card!.textContent).toContain('Patterns a ignorer');
  });

  // ─── Agents + token usage merged into one card ───────────────────────────

  it('renders estimated token usage inside each agent block', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    // Find the agents heading
    const agentsHeading = screen.getByText('Agents');
    const agentsCard = agentsHeading.closest('div[style]')?.parentElement;
    expect(agentsCard).toBeTruthy();
    // Estimated token usage should be shown per agent inside their block
    expect(agentsCard!.textContent).toContain('Estimation tokens');
    expect(agentsCard!.textContent).toContain('5,000 tok');
  });

  // ─── Discover keys button ─────────────────────────────────────────────

  it('renders the auto-detect button for API keys', async () => {
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);
    // The discover keys button should be present
    expect(screen.getByText('Auto-detecter')).toBeDefined();
  });

  it('calls discoverKeys API when auto-detect button is clicked', async () => {
    const { config } = await import('../../lib/api');
    await wrap(<SettingsPage {...defaultProps} agents={[sampleAgent]} />);

    const btn = screen.getByText('Auto-detecter');
    await act(async () => { fireEvent.click(btn); });

    await waitFor(() => {
      expect(config.discoverKeys).toHaveBeenCalled();
    });
  });
});
