// Unit tests for CompressionSection — the RTK (Rust Token Killer) "Eco mode"
// banner at the top of the Agents list.
//
// Scope: the 3 activation states (none/partial/all), hidden when no
// applicable agent is installed, install modal flow when the binary is
// missing, API-only agents excluded from the count.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { AgentDetection, AgentType } from '../../../types/generated';

const { activateMock, savingsMock } = vi.hoisted(() => ({
  activateMock: vi.fn(),
  savingsMock: vi.fn(),
}));
vi.mock('../../../lib/api', () => buildApiMock({
  rtk: {
    activate: activateMock as never,
    savings: savingsMock as never,
  },
}));

import { CompressionSection } from '../CompressionSection';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

function mkAgent(over: Partial<AgentDetection> & { agent_type: AgentType }): AgentDetection {
  return {
    name: over.name ?? 'Agent',
    agent_type: over.agent_type,
    installed: over.installed ?? true,
    enabled: over.enabled ?? true,
    path: over.path ?? '/usr/bin/fake',
    version: over.version ?? null,
    latest_version: null,
    origin: 'US',
    install_command: null,
    host_managed: false,
    host_label: null,
    runtime_available: over.runtime_available ?? true,
    rtk_available: over.rtk_available ?? true,
    rtk_hook_configured: over.rtk_hook_configured ?? false,
  };
}

describe('CompressionSection', () => {
  beforeEach(() => {
    activateMock.mockReset();
    activateMock.mockResolvedValue({ success: true, stdout: '', stderr: '' });
    savingsMock.mockReset();
    savingsMock.mockResolvedValue({ available: false, total_tokens_saved: 0, ratio_percent: 0, sample_count: 0 });
  });

  it('hides entirely when no RTK-applicable agent is installed', () => {
    const { container } = render(
      <CompressionSection
        agents={[mkAgent({ agent_type: 'Vibe' }), mkAgent({ agent_type: 'Ollama' })]}
        t={t}
      />,
    );
    // Vibe + Ollama are both API-only / hookless — nothing to show.
    expect(container.firstChild).toBeNull();
  });

  it('renders the "none activated" state when no hook is configured', () => {
    render(
      <CompressionSection
        agents={[
          mkAgent({ agent_type: 'ClaudeCode', rtk_hook_configured: false }),
          mkAgent({ agent_type: 'Codex', rtk_hook_configured: false }),
        ]}
        t={t}
      />,
    );
    expect(screen.getByText('config.rtk.stateNone')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /config\.rtk\.activateAll/ })).toBeInTheDocument();
  });

  it('renders the partial state with the remaining count', () => {
    render(
      <CompressionSection
        agents={[
          mkAgent({ agent_type: 'ClaudeCode', rtk_hook_configured: true }),
          mkAgent({ agent_type: 'Codex', rtk_hook_configured: false }),
          mkAgent({ agent_type: 'GeminiCli', rtk_hook_configured: false }),
        ]}
        t={t}
      />,
    );
    // 1 configured out of 3 applicable.
    expect(screen.getByText('config.rtk.statePartial:1,3')).toBeInTheDocument();
    // "Activate on the 2 remaining"
    expect(screen.getByRole('button', { name: /config\.rtk\.activateRemaining:2/ })).toBeInTheDocument();
  });

  it('renders the "all active" green state and hides the CTA when everything is configured', () => {
    render(
      <CompressionSection
        agents={[
          mkAgent({ agent_type: 'ClaudeCode', rtk_hook_configured: true }),
          mkAgent({ agent_type: 'Codex', rtk_hook_configured: true }),
        ]}
        t={t}
      />,
    );
    expect(screen.getByText('config.rtk.stateAll')).toBeInTheDocument();
    // No activation CTA when all configured — prevents noise.
    expect(screen.queryByRole('button', { name: /config\.rtk\.activate/ })).not.toBeInTheDocument();
  });

  it('excludes API-only and hookless agents from the count', () => {
    render(
      <CompressionSection
        agents={[
          mkAgent({ agent_type: 'ClaudeCode', rtk_hook_configured: true }),
          // Vibe (API) and Ollama (no hook) must NOT count against the total.
          mkAgent({ agent_type: 'Vibe', rtk_hook_configured: false }),
          mkAgent({ agent_type: 'Ollama', rtk_hook_configured: false }),
        ]}
        t={t}
      />,
    );
    expect(screen.getByText('config.rtk.stateAll')).toBeInTheDocument();
  });

  it('shows "install RTK" CTA when the binary is missing', () => {
    render(
      <CompressionSection
        agents={[mkAgent({ agent_type: 'ClaudeCode', rtk_available: false, rtk_hook_configured: false })]}
        t={t}
      />,
    );
    expect(screen.getByText('config.rtk.notInstalled')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /config\.rtk\.installCta/ })).toBeInTheDocument();
  });

  it('opens the install modal with the command and a GitHub link when the install CTA is clicked', () => {
    render(
      <CompressionSection
        agents={[mkAgent({ agent_type: 'ClaudeCode', rtk_available: false, rtk_hook_configured: false })]}
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /config\.rtk\.installCta/ }));
    expect(screen.getByText('config.rtk.installHelp')).toBeInTheDocument();
    // Install command is shown in a <pre> — we assert by substring so the
    // exact URL/script form can evolve without breaking the test.
    expect(document.body.textContent).toContain('install.sh');
    expect(screen.getByRole('link', { name: /config\.rtk\.viewOnGithub/ })).toHaveAttribute('href', expect.stringContaining('github.com/rtk-ai/rtk'));
  });

  it('activate button calls rtk.activate and notifies parent via onActivated', async () => {
    const onActivated = vi.fn();
    render(
      <CompressionSection
        agents={[mkAgent({ agent_type: 'ClaudeCode', rtk_available: true, rtk_hook_configured: false })]}
        onActivated={onActivated}
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /config\.rtk\.activateAll/ }));
    await waitFor(() => expect(activateMock).toHaveBeenCalledTimes(1));
    expect(onActivated).toHaveBeenCalledTimes(1);
  });

  it('disables the activate button and shows the spinner label while the request is in flight', async () => {
    activateMock.mockImplementation(() => new Promise(() => { /* never resolves */ }));
    render(
      <CompressionSection
        agents={[mkAgent({ agent_type: 'ClaudeCode', rtk_hook_configured: false })]}
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /config\.rtk\.activateAll/ }));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /config\.rtk\.activating/ })).toBeDisabled();
    });
    // Second click while disabled must not fire a second request.
    fireEvent.click(screen.getByRole('button', { name: /config\.rtk\.activating/ }));
    expect(activateMock).toHaveBeenCalledTimes(1);
  });

  it('renders the savings counter when RTK reports availability with a non-zero total', async () => {
    savingsMock.mockResolvedValue({ available: true, total_tokens_saved: 42_000, ratio_percent: 0.89, sample_count: 120 });
    render(
      <CompressionSection
        agents={[mkAgent({ agent_type: 'ClaudeCode', rtk_hook_configured: true })]}
        t={t}
      />,
    );
    await waitFor(() => {
      // formatTokens(42000) = "42k"
      expect(document.body.textContent).toMatch(/config\.rtk\.savings:42k/);
    });
  });

  it('hides the savings line when RTK is unavailable', () => {
    savingsMock.mockResolvedValue({ available: false, total_tokens_saved: 0, ratio_percent: 0, sample_count: 0 });
    render(
      <CompressionSection
        agents={[mkAgent({ agent_type: 'ClaudeCode', rtk_hook_configured: false })]}
        t={t}
      />,
    );
    expect(document.body.textContent).not.toMatch(/config\.rtk\.savings:/);
  });

  it('shows a persistent error toast with the stderr payload as copyable when activate returns success:false', async () => {
    activateMock.mockResolvedValue({ success: false, stdout: '', stderr: 'rtk error: HOME not writable' });
    const toast = vi.fn();
    render(
      <CompressionSection
        agents={[mkAgent({ agent_type: 'ClaudeCode', rtk_hook_configured: false })]}
        toast={toast}
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /config\.rtk\.activateAll/ }));
    await waitFor(() => expect(toast).toHaveBeenCalledTimes(1));
    // Human-facing title is the i18n key (stable); stderr ships in `copyable`
    // for the user to select/copy into Slack. `persistent: true` is the
    // hook's default for `error`, so we don't pass it explicitly.
    expect(toast).toHaveBeenCalledWith('config.rtk.activateError', 'error', {
      copyable: 'rtk error: HOME not writable',
    });
  });

  it('calls toast success (ephemeral) when activate returns success:true', async () => {
    activateMock.mockResolvedValue({ success: true, stdout: 'wired 3 agents', stderr: '' });
    const toast = vi.fn();
    render(
      <CompressionSection
        agents={[mkAgent({ agent_type: 'ClaudeCode', rtk_hook_configured: false })]}
        toast={toast}
        t={t}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /config\.rtk\.activateAll/ }));
    await waitFor(() => expect(toast).toHaveBeenCalledTimes(1));
    expect(toast).toHaveBeenCalledWith('config.rtk.activateSuccess', 'success');
  });

  it('expanding details reveals the 3 stat cards (tokens / ratio / samples)', async () => {
    savingsMock.mockResolvedValue({ available: true, total_tokens_saved: 50_000, ratio_percent: 87, sample_count: 1234 });
    render(
      <CompressionSection
        agents={[mkAgent({ agent_type: 'ClaudeCode', rtk_hook_configured: true })]}
        t={t}
      />,
    );
    // Details toggle only appears when savings.total > 0; wait for fetch.
    const toggle = await screen.findByRole('button', { name: /config\.rtk\.detailsToggle/ });
    // Collapsed by default: stats invisible.
    expect(screen.queryByText('config.rtk.statTokens')).not.toBeInTheDocument();
    fireEvent.click(toggle);
    expect(screen.getByText('config.rtk.statTokens')).toBeInTheDocument();
    expect(screen.getByText('config.rtk.statRatio')).toBeInTheDocument();
    expect(screen.getByText('config.rtk.statSamples')).toBeInTheDocument();
    // Ratio rendered rounded.
    expect(screen.getByText('87%')).toBeInTheDocument();
  });

  it('sobriety info button toggles a paragraph about responsible AI usage', () => {
    render(
      <CompressionSection
        agents={[mkAgent({ agent_type: 'ClaudeCode', rtk_hook_configured: false })]}
        t={t}
      />,
    );
    // Collapsed by default.
    expect(screen.queryByText('config.rtk.sobrietyBody')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /config\.rtk\.sobrietyTitle/ }));
    expect(screen.getByText('config.rtk.sobrietyBody')).toBeInTheDocument();
    // Re-click collapses.
    fireEvent.click(screen.getByRole('button', { name: /config\.rtk\.sobrietyTitle/ }));
    expect(screen.queryByText('config.rtk.sobrietyBody')).not.toBeInTheDocument();
  });

  it('always surfaces the "Powered by RTK" attribution link', () => {
    render(
      <CompressionSection
        agents={[mkAgent({ agent_type: 'ClaudeCode', rtk_hook_configured: true })]}
        t={t}
      />,
    );
    const link = screen.getByRole('link', { name: /RTK/ });
    expect(link).toHaveAttribute('href', expect.stringContaining('github.com/rtk-ai/rtk'));
    expect(link).toHaveAttribute('target', '_blank');
  });
});
