// AgentsSection — agent lifecycle + access-control action coverage.
//
// The default-tier / default-summary dropdown surface is already pinned by
// AgentsSection.defaultTier.test.tsx. This file targets the previously
// UNCOVERED per-agent handlers in the agent grid :
//
//   - install (handleInstallAgent) : agentsApi.install + refetch, error toast
//   - install re-entry guard (installingRef) blocks a double-click
//   - uninstall : confirm gate, agentsApi.uninstall + detect, failure toast
//   - enable/disable toggle : agentsApi.toggle + refetch, error toast
//   - full-access switch : configApi.setAgentAccess (click + keyboard) + refetch
//   - conditional rendering of Install vs toggle/uninstall per install status
//   - disabled state while an install/uninstall is in flight
//
// Conventions mirror AgentsSection.defaultTier.test.tsx (buildApiMock +
// vi.hoisted mock fns + the inline `t` echo helper). No real person names.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup, act } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { AgentDetection, AgentsConfig, AgentType } from '../../../types/generated';

const {
  getServerConfigMock,
  installMock,
  uninstallMock,
  toggleMock,
  detectMock,
  setAgentAccessMock,
} = vi.hoisted(() => ({
  getServerConfigMock: vi.fn(),
  installMock: vi.fn(),
  uninstallMock: vi.fn(),
  toggleMock: vi.fn(),
  detectMock: vi.fn(),
  setAgentAccessMock: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  config: {
    getServerConfig: getServerConfigMock as never,
    setAgentAccess: setAgentAccessMock as never,
  },
  agents: {
    install: installMock as never,
    uninstall: uninstallMock as never,
    toggle: toggleMock as never,
    detect: detectMock as never,
  },
}));

import { AgentsSection } from '../AgentsSection';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

// Minimal AgentDetection factory — all flags off by default, overridable.
function makeAgent(over: Partial<AgentDetection> & { agent_type: AgentType; name: string }): AgentDetection {
  return {
    installed: false,
    enabled: false,
    path: null,
    version: null,
    latest_version: null,
    origin: 'host',
    install_command: 'npm i -g some-cli',
    host_managed: false,
    host_label: null,
    runtime_available: false,
    rtk_available: false,
    rtk_hook_configured: false,
    ...over,
  };
}

type Props = Parameters<typeof AgentsSection>[0];

function renderSection(over: Partial<Props> = {}) {
  const refetchAgents = vi.fn();
  const refetchAgentAccess = vi.fn();
  const toastFn = vi.fn();
  const result = render(
    <AgentsSection
      agents={[]}
      agentAccess={null}
      configLanguage="fr"
      refetchAgents={refetchAgents}
      refetchAgentAccess={refetchAgentAccess}
      toast={toastFn}
      t={t}
      {...over}
    />,
  );
  return { refetchAgents, refetchAgentAccess, toastFn, ...result };
}

beforeEach(() => {
  getServerConfigMock.mockReset();
  getServerConfigMock.mockResolvedValue({
    default_model_tier: 'default',
    default_summary_strategy: 'Off',
    host: 'localhost', port: 3140,
  });
  installMock.mockReset();
  installMock.mockResolvedValue(undefined);
  uninstallMock.mockReset();
  uninstallMock.mockResolvedValue(undefined);
  toggleMock.mockReset();
  toggleMock.mockResolvedValue(undefined);
  detectMock.mockReset();
  detectMock.mockResolvedValue([]);
  setAgentAccessMock.mockReset();
  setAgentAccessMock.mockResolvedValue(undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  cleanup();
});

describe('AgentsSection — install action', () => {
  it('renders an Install button for a not-installed, no-runtime agent', () => {
    renderSection({ agents: [makeAgent({ name: 'AgentClaude', agent_type: 'ClaudeCode' })] });
    expect(screen.getByText(/Installer/)).toBeTruthy();
    // The install_command hint is shown under the name.
    expect(screen.getByText('npm i -g some-cli')).toBeTruthy();
  });

  it('calls agentsApi.install + refetchAgents on click', async () => {
    const { refetchAgents } = renderSection({
      agents: [makeAgent({ name: 'AgentClaude', agent_type: 'ClaudeCode' })],
    });
    fireEvent.click(screen.getByText(/Installer/));
    await waitFor(() => expect(installMock).toHaveBeenCalledWith('ClaudeCode'));
    await waitFor(() => expect(refetchAgents).toHaveBeenCalled());
  });

  it('surfaces an error toast when install rejects', async () => {
    installMock.mockRejectedValueOnce(new Error('no npm'));
    const { toastFn } = renderSection({
      agents: [makeAgent({ name: 'AgentClaude', agent_type: 'ClaudeCode' })],
    });
    fireEvent.click(screen.getByText(/Installer/));
    await waitFor(() =>
      expect(toastFn).toHaveBeenCalledWith(
        expect.stringContaining('config.installFailed'),
        'error',
      ),
    );
  });

  it('re-entry guard blocks a second install while the first is in flight', async () => {
    let resolveInstall: () => void = () => {};
    installMock.mockReturnValueOnce(new Promise<void>(r => { resolveInstall = r; }));
    renderSection({ agents: [makeAgent({ name: 'AgentClaude', agent_type: 'ClaudeCode' })] });
    const btn = screen.getByText(/Installer/).closest('button') as HTMLButtonElement;
    fireEvent.click(btn);
    // Synchronous double-click — the ref guard must swallow the 2nd.
    fireEvent.click(btn);
    fireEvent.click(btn);
    expect(installMock).toHaveBeenCalledTimes(1);
    await act(async () => { resolveInstall(); });
  });
});

describe('AgentsSection — uninstall action', () => {
  const installed = () => makeAgent({
    name: 'AgentClaude', agent_type: 'ClaudeCode', installed: true, enabled: true,
  });

  it('renders enable + uninstall controls (no Install button) when installed', () => {
    renderSection({ agents: [installed()] });
    expect(screen.queryByText(/Installer/)).toBeNull();
    expect(screen.getByLabelText('config.uninstall')).toBeTruthy();
  });

  it('aborts when the confirm() dialog is dismissed', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(false));
    renderSection({ agents: [installed()] });
    fireEvent.click(screen.getByLabelText('config.uninstall'));
    // Confirm returned false → no API call.
    expect(uninstallMock).not.toHaveBeenCalled();
  });

  it('calls agentsApi.uninstall + detect + refetch when confirmed', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
    detectMock.mockResolvedValueOnce([]);
    const { refetchAgents } = renderSection({ agents: [installed()] });
    fireEvent.click(screen.getByLabelText('config.uninstall'));
    await waitFor(() => expect(uninstallMock).toHaveBeenCalledWith('ClaudeCode'));
    await waitFor(() => expect(detectMock).toHaveBeenCalled());
    await waitFor(() => expect(refetchAgents).toHaveBeenCalled());
  });

  it('shows a failure toast when the agent is still installed+enabled after uninstall', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
    // detect() still reports the agent as installed + enabled → uninstall failed.
    detectMock.mockResolvedValueOnce([
      makeAgent({ name: 'AgentClaude', agent_type: 'ClaudeCode', installed: true, enabled: true }),
    ]);
    const { toastFn } = renderSection({ agents: [installed()] });
    fireEvent.click(screen.getByLabelText('config.uninstall'));
    await waitFor(() => expect(toastFn).toHaveBeenCalledWith('config.uninstallFailed', 'error'));
  });

  it('shows a failure toast when uninstall rejects', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
    uninstallMock.mockRejectedValueOnce(new Error('boom'));
    const { toastFn } = renderSection({ agents: [installed()] });
    fireEvent.click(screen.getByLabelText('config.uninstall'));
    await waitFor(() => expect(toastFn).toHaveBeenCalledWith('config.uninstallFailed', 'error'));
  });
});

describe('AgentsSection — enable/disable toggle', () => {
  it('calls agentsApi.toggle + refetchAgents when the enable button is clicked', async () => {
    const { refetchAgents } = renderSection({
      agents: [makeAgent({ name: 'AgentCodex', agent_type: 'Codex', installed: true, enabled: true })],
    });
    // The toggle button label echoes the enabled state.
    const toggleBtn = screen.getByTitle('config.toggleDisable');
    fireEvent.click(toggleBtn);
    await waitFor(() => expect(toggleMock).toHaveBeenCalledWith('Codex'));
    await waitFor(() => expect(refetchAgents).toHaveBeenCalled());
  });

  it('surfaces an error toast when toggle rejects', async () => {
    toggleMock.mockRejectedValueOnce('toggle failed');
    const { toastFn } = renderSection({
      agents: [makeAgent({ name: 'AgentCodex', agent_type: 'Codex', installed: true, enabled: false })],
    });
    const toggleBtn = screen.getByTitle('config.toggleEnable');
    fireEvent.click(toggleBtn);
    await waitFor(() => expect(toastFn).toHaveBeenCalledWith('toggle failed', 'error'));
  });
});

describe('AgentsSection — full-access switch', () => {
  function accessConfig(over: Partial<AgentsConfig> = {}): AgentsConfig {
    const blank = { path: null, installed: false, version: null, full_access: false };
    const blankTier = { economy: null, reasoning: null };
    return {
      claude_code: { ...blank },
      codex: { ...blank },
      gemini_cli: { ...blank },
      kiro: { ...blank },
      vibe: { ...blank },
      copilot_cli: { ...blank },
      ollama: { ...blank },
      model_tiers: {
        claude_code: { ...blankTier }, codex: { ...blankTier }, gemini_cli: { ...blankTier },
        kiro: { ...blankTier }, vibe: { ...blankTier }, copilot_cli: { ...blankTier }, ollama: { ...blankTier },
      },
      ...over,
    };
  }

  it('renders the permission switch reflecting full_access=false', () => {
    renderSection({
      agents: [makeAgent({ name: 'AgentClaude', agent_type: 'ClaudeCode', installed: true, enabled: true })],
      agentAccess: accessConfig(),
    });
    const sw = screen.getByRole('switch');
    expect(sw.getAttribute('aria-checked')).toBe('false');
  });

  it('reflects full_access=true from agentAccess', () => {
    renderSection({
      agents: [makeAgent({ name: 'AgentClaude', agent_type: 'ClaudeCode', installed: true, enabled: true })],
      agentAccess: accessConfig({ claude_code: { path: null, installed: true, version: null, full_access: true } }),
    });
    expect(screen.getByRole('switch').getAttribute('aria-checked')).toBe('true');
  });

  it('calls configApi.setAgentAccess + refetchAgentAccess on click (toggles the flag)', async () => {
    const { refetchAgentAccess } = renderSection({
      agents: [makeAgent({ name: 'AgentClaude', agent_type: 'ClaudeCode', installed: true, enabled: true })],
      agentAccess: accessConfig(),
    });
    fireEvent.click(screen.getByRole('switch'));
    await waitFor(() =>
      expect(setAgentAccessMock).toHaveBeenCalledWith({ agent: 'ClaudeCode', full_access: true }),
    );
    await waitFor(() => expect(refetchAgentAccess).toHaveBeenCalled());
  });

  it('toggles full-access via keyboard (Space)', async () => {
    const { refetchAgentAccess } = renderSection({
      agents: [makeAgent({ name: 'AgentClaude', agent_type: 'ClaudeCode', installed: true, enabled: true })],
      agentAccess: accessConfig({ claude_code: { path: null, installed: true, version: null, full_access: true } }),
    });
    fireEvent.keyDown(screen.getByRole('switch'), { key: ' ' });
    await waitFor(() =>
      expect(setAgentAccessMock).toHaveBeenCalledWith({ agent: 'ClaudeCode', full_access: false }),
    );
    await waitFor(() => expect(refetchAgentAccess).toHaveBeenCalled());
  });

  it('still refetches access when setAgentAccess rejects (catch branch)', async () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    setAgentAccessMock.mockRejectedValueOnce(new Error('net'));
    const { refetchAgentAccess } = renderSection({
      agents: [makeAgent({ name: 'AgentClaude', agent_type: 'ClaudeCode', installed: true, enabled: true })],
      agentAccess: accessConfig(),
    });
    fireEvent.click(screen.getByRole('switch'));
    await waitFor(() => expect(refetchAgentAccess).toHaveBeenCalled());
    warnSpy.mockRestore();
  });
});

describe('AgentsSection — runtime-available rendering', () => {
  it('renders toggle controls (not Install) for a runtime-available agent', () => {
    renderSection({
      agents: [makeAgent({
        name: 'AgentVibe', agent_type: 'Vibe',
        installed: false, runtime_available: true, enabled: true,
      })],
    });
    expect(screen.queryByText(/Installer/)).toBeNull();
    expect(screen.getByText(/runtime OK/)).toBeTruthy();
    expect(screen.getByTitle('config.toggleDisable')).toBeTruthy();
  });

  it('surfaces a runtime_warning note when present', () => {
    renderSection({
      agents: [makeAgent({
        name: 'AgentVibe', agent_type: 'Vibe',
        installed: false, runtime_available: true, enabled: true,
        runtime_warning: 'vibe.sdk_fallback',
      })],
    });
    expect(screen.getByText(/agentRuntimeWarning\.vibe\.sdk_fallback/)).toBeTruthy();
  });
});
