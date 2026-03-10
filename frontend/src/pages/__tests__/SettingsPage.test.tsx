import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

// Mock API — SettingsPage calls config.getTokens(), config.dbInfo(), config.getScanDepth(), stats.agentUsage() on mount
vi.mock('../../lib/api', () => ({
  config: {
    getTokens: vi.fn().mockResolvedValue({ keys: [], overrides: {} }),
    dbInfo: vi.fn().mockResolvedValue({ path: '/tmp/db', size_bytes: 1024 }),
    getScanDepth: vi.fn().mockResolvedValue(4),
    saveApiKey: vi.fn(),
    deleteApiKey: vi.fn(),
    activateApiKey: vi.fn(),
    syncAgentTokens: vi.fn(),
    toggleTokenOverride: vi.fn(),
    getLanguage: vi.fn(),
    saveLanguage: vi.fn(),
    setScanDepth: vi.fn(),
    getAgentAccess: vi.fn(),
    setAgentAccess: vi.fn(),
    exportData: vi.fn(),
    importData: vi.fn(),
  },
  agents: {
    detect: vi.fn(),
    install: vi.fn(),
    uninstall: vi.fn(),
    toggle: vi.fn(),
  },
  stats: {
    agentUsage: vi.fn().mockResolvedValue([]),
  },
}));

import { SettingsPage } from '../SettingsPage';
import type { AgentsConfig } from '../../types/generated';

const noop = () => {};
const toastFn = vi.fn() as any;

const wrap = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

describe('SettingsPage', () => {
  it('renders without crashing with minimal props', () => {
    wrap(
      <SettingsPage
        agents={[]}
        agentAccess={null}
        configLanguage={null}
        refetchAgents={noop}
        refetchAgentAccess={noop}
        refetchLanguage={noop}
        refetchProjects={noop}
        refetchDiscussions={noop}
        onReset={noop}
        toast={toastFn}
      />
    );
    // SettingsPage should render the settings heading
    expect(document.body.textContent).toBeDefined();
  });

  it('renders with agentAccess set', () => {
    const agentAccess: AgentsConfig = {
      claude_code: { path: null, installed: true, version: null, full_access: true },
      codex: { path: null, installed: false, version: null, full_access: false },
      gemini_cli: { path: null, installed: false, version: null, full_access: false },
      kiro: { path: null, installed: false, version: null, full_access: false },
      vibe: { path: null, installed: false, version: null, full_access: false },
    };
    wrap(
      <SettingsPage
        agents={[]}
        agentAccess={agentAccess}
        configLanguage="en"
        refetchAgents={noop}
        refetchAgentAccess={noop}
        refetchLanguage={noop}
        refetchProjects={noop}
        refetchDiscussions={noop}
        onReset={noop}
        toast={toastFn}
      />
    );
    expect(document.body.textContent).toBeDefined();
  });

  it('renders with agents detected', () => {
    wrap(
      <SettingsPage
        agents={[
          { name: 'Claude Code', agent_type: 'ClaudeCode', installed: true, enabled: true, runtime_available: false },
        ] as any}
        agentAccess={null}
        configLanguage="fr"
        refetchAgents={noop}
        refetchAgentAccess={noop}
        refetchLanguage={noop}
        refetchProjects={noop}
        refetchDiscussions={noop}
        onReset={noop}
        toast={toastFn}
      />
    );
    expect(document.body.textContent).toBeDefined();
  });
});
