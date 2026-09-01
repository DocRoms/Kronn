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
  setAgentMentionColorMock,
  setAgentConcurrencyMock,
  usageGetMock,
  getModelTiersMock,
} = vi.hoisted(() => ({
  getServerConfigMock: vi.fn(),
  installMock: vi.fn(),
  uninstallMock: vi.fn(),
  toggleMock: vi.fn(),
  detectMock: vi.fn(),
  setAgentAccessMock: vi.fn(),
  setAgentMentionColorMock: vi.fn(),
  setAgentConcurrencyMock: vi.fn(),
  usageGetMock: vi.fn(),
  getModelTiersMock: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  config: {
    getServerConfig: getServerConfigMock as never,
    setAgentAccess: setAgentAccessMock as never,
    setAgentMentionColor: setAgentMentionColorMock as never,
    setAgentConcurrency: setAgentConcurrencyMock as never,
    getModelTiers: getModelTiersMock as never,
  },
  agents: {
    install: installMock as never,
    uninstall: uninstallMock as never,
    toggle: toggleMock as never,
    detect: detectMock as never,
  },
  usage: {
    get: usageGetMock as never,
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
  setAgentConcurrencyMock.mockReset();
  setAgentConcurrencyMock.mockResolvedValue(undefined);
  setAgentAccessMock.mockReset();
  setAgentAccessMock.mockResolvedValue(undefined);
  setAgentMentionColorMock.mockReset();
  setAgentMentionColorMock.mockResolvedValue(undefined);
  usageGetMock.mockReset();
  usageGetMock.mockResolvedValue({
    period_kind: 'monthly',
    rows: [],
    totals: {
      input_tokens: 0, output_tokens: 0,
      cache_creation_tokens: 0, cache_read_tokens: 0,
      total_tokens: 0, total_cost: 0,
    },
    agents_detected: [],
  });
  const emptyTier = { economy: null, default: null, reasoning: null };
  getModelTiersMock.mockReset();
  getModelTiersMock.mockResolvedValue({
    claude_code: { economy: 'haiku', default: 'sonnet', reasoning: 'opus' },
    codex: { ...emptyTier },
    gemini_cli: { ...emptyTier },
    kiro: { ...emptyTier },
    vibe: { ...emptyTier },
    copilot_cli: { ...emptyTier },
    ollama: { ...emptyTier },
    lite_llm: { ...emptyTier },
    nvidia: { ...emptyTier },
  });
  sessionStorage.removeItem('kronn:model-config-target');
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  cleanup();
  sessionStorage.removeItem('kronn:model-config-target');
});

describe('AgentsSection — model-error deep link', () => {
  it('focuses the exact agent and reasoning-tier picker left by a System CTA', async () => {
    sessionStorage.setItem('kronn:model-config-target', JSON.stringify({
      agentType: 'ClaudeCode',
      tier: 'reasoning',
    }));
    renderSection({
      agents: [makeAgent({
        name: 'AgentClaude',
        agent_type: 'ClaudeCode',
        installed: true,
        enabled: true,
      })],
    });

    await waitFor(() => {
      const select = document.querySelector<HTMLSelectElement>(
        '[data-model-tier-agent="ClaudeCode"][data-model-tier="reasoning"]',
      );
      expect(select).toBeTruthy();
      expect(document.activeElement).toBe(select);
      expect(select?.classList.contains('set-model-tier-focus')).toBe(true);
    });
    expect(sessionStorage.getItem('kronn:model-config-target')).toBeNull();
  });
});

describe('AgentsSection — per-agent concurrency', () => {
  it('renders the control on Ollama, whose dedicated card is not the generic one', () => {
    // Ollama and LiteLLM render through their own cards; a control mounted only
    // in the generic body silently skips exactly the agent that needs it most.
    renderSection({ agents: [makeAgent({ name: 'Ollama', agent_type: 'Ollama', installed: true })] });
    const input = screen.getByTestId('agent-concurrency-Ollama') as HTMLInputElement;
    expect(input).toBeTruthy();
    // One inference slot: the placeholder must show the default that applies.
    expect(input.placeholder).toBe('1');
  });

  it('drops NVIDIA from the fleet (it moved to the External API zone) and shows 5 for a CLI', () => {
    // KT-339 \u2014 NVIDIA is now a connection in the unified External API zone,
    // not a standalone fleet row, so it no longer has a fleet concurrency
    // control. A CLI still shows its 5-slot default.
    renderSection({
      agents: [
        makeAgent({ name: 'Nvidia', agent_type: 'Nvidia', installed: true }),
        makeAgent({ name: 'AgentClaude', agent_type: 'ClaudeCode', installed: true }),
      ],
    });
    expect(screen.queryByTestId('agent-concurrency-Nvidia')).toBeNull();
    expect((screen.getByTestId('agent-concurrency-ClaudeCode') as HTMLInputElement).placeholder).toBe('5');
  });

  it('persists a new value and refetches', async () => {
    const { refetchAgentAccess } = renderSection({
      agents: [makeAgent({ name: 'Ollama', agent_type: 'Ollama', installed: true })],
    });
    fireEvent.change(screen.getByTestId('agent-concurrency-Ollama'), { target: { value: '3' } });
    await waitFor(() =>
      expect(setAgentConcurrencyMock).toHaveBeenCalledWith({ agent: 'Ollama', concurrency: 3 }),
    );
    await waitFor(() => expect(refetchAgentAccess).toHaveBeenCalled());
  });

  it('clearing the field restores the family default rather than unlimiting it', async () => {
    // Start from a set value: clearing an already-empty number input fires no
    // change event, so the real scenario is overridden -> cleared.
    renderSection({
      agents: [makeAgent({ name: 'Ollama', agent_type: 'Ollama', installed: true })],
      agentAccess: { ollama: { concurrency: 4 } } as never,
    });
    fireEvent.change(screen.getByTestId('agent-concurrency-Ollama'), { target: { value: '' } });
    await waitFor(() =>
      expect(setAgentConcurrencyMock).toHaveBeenCalledWith({ agent: 'Ollama', concurrency: null }),
    );
  });

  it('never sends 0, which would mean an agent that is enabled but never runs', async () => {
    renderSection({ agents: [makeAgent({ name: 'Ollama', agent_type: 'Ollama', installed: true })] });
    fireEvent.change(screen.getByTestId('agent-concurrency-Ollama'), { target: { value: '0' } });
    await waitFor(() =>
      expect(setAgentConcurrencyMock).toHaveBeenCalledWith({ agent: 'Ollama', concurrency: 1 }),
    );
  });
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
      open_code: { ...blank },
      gemini_cli: { ...blank },
      kiro: { ...blank },
      vibe: { ...blank },
      copilot_cli: { ...blank },
      ollama: { ...blank },
      lite_llm: { ...blank },
      nvidia: { ...blank },
      model_tiers: {
        claude_code: { ...blankTier }, codex: { ...blankTier }, open_code: { ...blankTier }, gemini_cli: { ...blankTier },
        kiro: { ...blankTier }, vibe: { ...blankTier }, copilot_cli: { ...blankTier }, ollama: { ...blankTier },
        lite_llm: { ...blankTier },
        nvidia: { ...blankTier },
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

  it('describes the effective Codex sandbox flag', () => {
    renderSection({
      agents: [makeAgent({ name: 'AgentCodex', agent_type: 'Codex', installed: true, enabled: true })],
      agentAccess: accessConfig(),
    });
    expect(screen.getByText('--sandbox=danger-full-access')).toBeTruthy();
    expect(screen.queryByText('--full-auto')).toBeNull();
  });

  it('shows the ACP permission description for OpenCode instead of a fabricated CLI flag (KT-543)', () => {
    const { container } = renderSection({
      agents: [makeAgent({ name: 'AgentOpenCode', agent_type: 'OpenCode', installed: true, enabled: true })],
      agentAccess: accessConfig(),
    });
    const panel = container.querySelector('[data-agent-type="OpenCode"] .set-agent-panel-access');
    expect(panel).toBeTruthy();
    expect(screen.getByText('config.fullAccessAcp')).toBeTruthy();
    expect(panel?.querySelector('code')).toBeNull();
  });

  it('toggles OpenCode full_access via open_code, not a missing agentAccess key', async () => {
    const { refetchAgentAccess } = renderSection({
      agents: [makeAgent({ name: 'AgentOpenCode', agent_type: 'OpenCode', installed: true, enabled: true })],
      agentAccess: accessConfig(),
    });
    expect(screen.getByRole('switch').getAttribute('aria-checked')).toBe('false');
    fireEvent.click(screen.getByRole('switch'));
    await waitFor(() =>
      expect(setAgentAccessMock).toHaveBeenCalledWith({ agent: 'OpenCode', full_access: true }),
    );
    await waitFor(() => expect(refetchAgentAccess).toHaveBeenCalled());
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

describe('AgentsSection — observed model costs', () => {
  it('adds an approximate monthly ccUsage rate only to observed models', async () => {
    usageGetMock.mockResolvedValueOnce({
      period_kind: 'monthly',
      rows: [{
        period: '2026-08',
        agent: 'claude',
        models_used: ['claude-sonnet-4-5', 'claude-opus-4-7'],
        model_breakdowns: [
          {
            model_name: 'claude-sonnet-4-5',
            input_tokens: 500_000,
            output_tokens: 100_000,
            cache_creation_tokens: 100_000,
            cache_read_tokens: 300_000,
            total_tokens: 1_000_000,
            cost: 12.345,
          },
          {
            model_name: 'claude-opus-4-7',
            input_tokens: 250_000,
            output_tokens: 50_000,
            cache_creation_tokens: 50_000,
            cache_read_tokens: 150_000,
            total_tokens: 500_000,
            cost: 12.345,
          },
        ],
        input_tokens: 750_000,
        output_tokens: 150_000,
        cache_creation_tokens: 150_000,
        cache_read_tokens: 450_000,
        total_tokens: 1_500_000,
        total_cost: 24.69,
      }],
      totals: {
        input_tokens: 750_000,
        output_tokens: 150_000,
        cache_creation_tokens: 150_000,
        cache_read_tokens: 450_000,
        total_tokens: 1_500_000,
        total_cost: 24.69,
      },
      agents_detected: ['claude'],
    });

    renderSection({
      agents: [makeAgent({
        name: 'AgentClaude', agent_type: 'ClaudeCode', installed: true, enabled: true,
      })],
    });

    await waitFor(() => expect(usageGetMock).toHaveBeenCalledWith('monthly'));
    for (const tier of ['economy', 'default', 'reasoning']) {
      const picker = await screen.findByLabelText(`disc.modelTier ${tier}`);
      fireEvent.focus(picker);
      await waitFor(() => expect(screen.getByRole('option', { name: 'sonnet' }))
        .toHaveTextContent('config.modelCostObserved:$12.3'));
      expect(screen.getByRole('option', { name: 'fable' })).not.toHaveTextContent('≈');
      fireEvent.keyDown(picker, { key: 'Escape' });
    }
    expect(screen.getByRole('button', { name: 'config.modelCostObservedTitle' })).toBeTruthy();

    expect(screen.getByTestId('model-cost-display')).toBeTruthy();
    expect(screen.getByRole('link', { name: /ccusage/i })).toHaveAttribute(
      'href',
      'https://github.com/ccusage/ccusage',
    );
    fireEvent.click(screen.getByTestId('model-cost-mode-relative'));
    expect(screen.getByTestId('model-cost-reference')).toHaveValue('claude-sonnet-4-5');
    const defaultPicker = screen.getByLabelText('disc.modelTier default');
    fireEvent.focus(defaultPicker);
    expect(screen.getByRole('option', { name: 'sonnet' })).toHaveTextContent('≈ ×1');
    expect(screen.getByRole('option', { name: 'opus' })).toHaveTextContent('≈ ×2');
    fireEvent.keyDown(defaultPicker, { key: 'Escape' });

    fireEvent.change(screen.getByTestId('model-cost-reference'), {
      target: { value: 'claude-opus-4-7' },
    });
    fireEvent.focus(defaultPicker);
    expect(screen.getByRole('option', { name: 'sonnet' })).toHaveTextContent('≈ ×0.5');
  });

  it('offers every known model in every reasoning tier', async () => {
    renderSection({
      agents: [makeAgent({
        name: 'AgentClaude', agent_type: 'ClaudeCode', installed: true, enabled: true,
      })],
    });

    const expectedModels = ['', 'haiku', 'sonnet', 'fable', 'opus'];
    for (const tier of ['economy', 'default', 'reasoning']) {
      const select = await screen.findByLabelText(`disc.modelTier ${tier}`);
      fireEvent.focus(select);
      expect(screen.getAllByRole('option').map(option => option.dataset.value)).toEqual(expectedModels);
      fireEvent.keyDown(select, { key: 'Escape' });
    }
  });
});

describe('AgentsSection — configurable mention colors', () => {
  it('uses the configured mention color as the matching agent card accent', () => {
    const agentAccess = {
      claude_code: {
        path: null,
        installed: true,
        version: null,
        full_access: true,
        mention_color: '#123abc',
      },
    } as AgentsConfig;
    const { container } = renderSection({
      agents: [makeAgent({
        name: 'AgentClaude',
        agent_type: 'ClaudeCode',
        installed: true,
        runtime_available: true,
        enabled: true,
      })],
      agentAccess,
    });

    const card = container.querySelector<HTMLElement>('[data-agent-type="ClaudeCode"]');
    expect(card?.style.getPropertyValue('--agent-color')).toBe('#123abc');
    expect(card?.querySelector('.set-agent-card-header')).toBeTruthy();
    expect(card?.querySelector('[data-testid="mention-color-ClaudeCode"]')).toBeTruthy();
    expect(card?.querySelector('.set-agent-panel-access')).toHaveTextContent('config.fullAccessBadge');
    expect(card?.querySelector('.set-agent-panel-auth')).toHaveTextContent('config.apiKeys');
  });

  it('keeps Ollama in the same accented card system', async () => {
    const { container } = renderSection({
      agents: [makeAgent({
        name: 'Ollama',
        agent_type: 'Ollama',
        installed: true,
        runtime_available: true,
        enabled: true,
      })],
    });
    await act(async () => {
      await Promise.resolve();
    });

    const card = container.querySelector<HTMLElement>('[data-agent-type="Ollama"]');
    expect(card).toHaveClass('set-agent-row-ollama');
    expect(card?.style.getPropertyValue('--agent-color').toLowerCase()).toBe('#60a5fa');
    expect(card?.querySelector('.set-ollama-card')).toBeTruthy();
    expect(card?.querySelector('.set-ollama-header-actions [data-testid="mention-color-Ollama"]')).toBeTruthy();
  });

  it('renders LiteLLM in the unified External API zone, not as its own fleet card', async () => {
    // KT-339 — LiteLLM and NVIDIA are unified into one "External API" zone, so
    // the standalone LiteLLM fleet card no longer renders.
    const { container } = renderSection({
      agents: [makeAgent({
        name: 'LiteLLM',
        agent_type: 'LiteLlm',
        installed: true,
        runtime_available: true,
        enabled: true,
      })],
    });
    await act(async () => {
      await Promise.resolve();
    });

    expect(container.querySelector('[data-agent-type="LiteLlm"]')).toBeNull();
    expect(screen.getByTestId('external-api-section')).toBeTruthy();
  });

  it('persists the selected agent color, refreshes config, and notifies renderers', async () => {
    const eventSpy = vi.fn();
    window.addEventListener('kronn:agent-mention-colors-changed', eventSpy);
    const { refetchAgentAccess, toastFn } = renderSection({
      agents: [makeAgent({ name: 'AgentCodex', agent_type: 'Codex' })],
    });

    fireEvent.change(screen.getByTestId('mention-color-Codex'), {
      target: { value: '#123abc' },
    });

    await waitFor(() => {
      expect(setAgentMentionColorMock).toHaveBeenCalledWith({
        agent: 'Codex',
        color: '#123abc',
      });
    });
    expect(refetchAgentAccess).toHaveBeenCalled();
    expect(eventSpy).toHaveBeenCalled();
    expect(toastFn).toHaveBeenCalledWith('config.saved', 'success');
    window.removeEventListener('kronn:agent-mention-colors-changed', eventSpy);
  });

  it('restores the displayed color when persistence fails', async () => {
    setAgentMentionColorMock.mockRejectedValueOnce(new Error('offline'));
    const { toastFn } = renderSection({
      agents: [makeAgent({ name: 'AgentCodex', agent_type: 'Codex' })],
    });
    const input = screen.getByTestId<HTMLInputElement>('mention-color-Codex');
    expect(input.value).toBe('#10a37f');

    fireEvent.change(input, { target: { value: '#123abc' } });

    await waitFor(() => expect(input.value).toBe('#10a37f'));
    expect(toastFn).toHaveBeenCalledWith('config.saveError', 'error');
  });
});

describe('AgentsSection — runtime-available rendering', () => {
  it('offers Install (not the enable toggle) for a runtime-only agent, keeping the via-npx hint', () => {
    // npx-reachable but not installed in the container: the user never
    // installed it, so it must be offered for install — never shown as
    // "Activé" with a toggle. The "runtime OK — via npx" hint stays so the
    // info that it's still usable isn't lost.
    renderSection({
      agents: [makeAgent({
        name: 'AgentCodex', agent_type: 'Codex',
        installed: false, runtime_available: true, enabled: true,
      })],
    });
    expect(screen.getByText(/Installer/)).toBeTruthy();
    expect(screen.getByText(/runtime OK/)).toBeTruthy();
    expect(screen.queryByTitle('config.toggleDisable')).toBeNull();
    expect(screen.queryByTitle('config.toggleEnable')).toBeNull();
  });

  it('disables Install + shows the host-CLI note under Docker', () => {
    // Under Docker the backend can't install on the host, so the button is
    // disabled and the note points to the host-side kronn CLI.
    renderSection({
      agents: [makeAgent({ name: 'AgentCodex', agent_type: 'Codex', installed: false, runtime_available: true })],
      inDocker: true,
    });
    const installBtn = screen.getByText(/Installer/).closest('button') as HTMLButtonElement;
    expect(installBtn.disabled).toBe(true);
    expect(screen.getByText(/config\.dockerInstallNote/)).toBeTruthy();
  });

  it('keeps Install enabled and hides the note when not under Docker', () => {
    renderSection({
      agents: [makeAgent({ name: 'AgentCodex', agent_type: 'Codex', installed: false, runtime_available: true })],
      inDocker: false,
    });
    const installBtn = screen.getByText(/Installer/).closest('button') as HTMLButtonElement;
    expect(installBtn.disabled).toBe(false);
    expect(screen.queryByText(/config\.dockerInstallNote/)).toBeNull();
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

  it('shows authentication required separately from installed and copies the setup command', () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    renderSection({
      agents: [makeAgent({
        name: 'AgentVibe',
        agent_type: 'Vibe',
        installed: true,
        runtime_available: true,
        enabled: true,
        auth_ready: false,
        auth_setup_command: 'vibe --setup',
      })],
    });

    expect(screen.getAllByText('config.agentAuthRequired')).toHaveLength(2);
    fireEvent.click(screen.getByText('vibe --setup').closest('button')!);
    expect(writeText).toHaveBeenCalledWith('vibe --setup');
  });
});
