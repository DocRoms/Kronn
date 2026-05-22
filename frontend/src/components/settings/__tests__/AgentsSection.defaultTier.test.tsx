// 0.8.6 phase 4 — Default model tier dropdown in AgentsSection.
//
// Pins the contract for the new "Niveau de raisonnement par défaut"
// dropdown that lives right under the RTK CompressionSection :
//
//   - reads the value from `configApi.getServerConfig()` on mount
//   - clicking an option calls `setServerConfig({default_model_tier: X})`
//   - dropdown is disabled while the value is loading (avoids saving
//     a wrong tier before the GET response arrives)
//   - optimistic update + revert on save error
//
// The actual `default_model_tier` consumer logic (NewDiscussionForm,
// QuickPromptForm, WorkflowWizard.blankStep) is covered by their own
// existing test suites with the new `getServerConfig` mock returning
// the chosen tier. This file focuses just on the new dropdown surface.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { AgentDetection } from '../../../types/generated';

const { getServerConfigMock, setServerConfigMock } = vi.hoisted(() => ({
  getServerConfigMock: vi.fn(),
  setServerConfigMock: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  config: {
    getServerConfig: getServerConfigMock as never,
    setServerConfig: setServerConfigMock as never,
  },
}));

import { AgentsSection } from '../AgentsSection';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

const EMPTY_AGENTS: AgentDetection[] = [];
const toastFn = vi.fn();

beforeEach(() => {
  getServerConfigMock.mockReset();
  setServerConfigMock.mockReset();
  toastFn.mockReset();
});

describe('AgentsSection — default model tier dropdown (0.8.6 phase 4)', () => {
  it('renders the dropdown with 3 tier options', async () => {
    getServerConfigMock.mockResolvedValue({
      default_model_tier: 'default',
      host: 'localhost', port: 3140,
    });
    render(
      <AgentsSection
        agents={EMPTY_AGENTS}
        agentAccess={null}
        configLanguage="fr"
        refetchAgents={vi.fn()}
        refetchAgentAccess={vi.fn()}
        toast={toastFn}
        t={t}
      />,
    );
    // All 3 tier buttons render.
    await waitFor(() => {
      expect(screen.getByTestId('default-tier-btn-economy')).toBeTruthy();
      expect(screen.getByTestId('default-tier-btn-default')).toBeTruthy();
      expect(screen.getByTestId('default-tier-btn-reasoning')).toBeTruthy();
    });
  });

  it('marks the saved tier as active on mount', async () => {
    getServerConfigMock.mockResolvedValue({
      default_model_tier: 'reasoning',
      host: 'localhost', port: 3140,
    });
    render(
      <AgentsSection
        agents={EMPTY_AGENTS}
        agentAccess={null}
        configLanguage="fr"
        refetchAgents={vi.fn()}
        refetchAgentAccess={vi.fn()}
        toast={toastFn}
        t={t}
      />,
    );
    await waitFor(() => {
      const reasoningBtn = screen.getByTestId('default-tier-btn-reasoning');
      expect(reasoningBtn.getAttribute('data-active')).toBe('true');
      const defaultBtn = screen.getByTestId('default-tier-btn-default');
      expect(defaultBtn.getAttribute('data-active')).toBe('false');
    });
  });

  it('PATCHes /config/server when a tier button is clicked', async () => {
    getServerConfigMock.mockResolvedValue({
      default_model_tier: 'default',
      host: 'localhost', port: 3140,
    });
    setServerConfigMock.mockResolvedValue(undefined);
    render(
      <AgentsSection
        agents={EMPTY_AGENTS}
        agentAccess={null}
        configLanguage="fr"
        refetchAgents={vi.fn()}
        refetchAgentAccess={vi.fn()}
        toast={toastFn}
        t={t}
      />,
    );
    await waitFor(() => {
      expect(screen.getByTestId('default-tier-btn-economy')).toBeTruthy();
    });

    fireEvent.click(screen.getByTestId('default-tier-btn-economy'));

    await waitFor(() => {
      expect(setServerConfigMock).toHaveBeenCalledWith({ default_model_tier: 'economy' });
    });
    // Toast confirms the save.
    expect(toastFn).toHaveBeenCalledWith('config.saved', 'success');
  });

  it('optimistically marks the new tier active before the API resolves', async () => {
    getServerConfigMock.mockResolvedValue({
      default_model_tier: 'default',
      host: 'localhost', port: 3140,
    });
    // Make the API hang so the optimistic update is visible.
    let resolveSet: () => void = () => {};
    setServerConfigMock.mockReturnValue(new Promise<void>(r => { resolveSet = r; }));
    render(
      <AgentsSection
        agents={EMPTY_AGENTS}
        agentAccess={null}
        configLanguage="fr"
        refetchAgents={vi.fn()}
        refetchAgentAccess={vi.fn()}
        toast={toastFn}
        t={t}
      />,
    );
    await waitFor(() => {
      expect(screen.getByTestId('default-tier-btn-reasoning')).toBeTruthy();
    });

    fireEvent.click(screen.getByTestId('default-tier-btn-reasoning'));

    // BEFORE setServerConfig resolves, the new tier is already
    // displayed as active. Optimistic UX guarantees no perceived lag.
    expect(screen.getByTestId('default-tier-btn-reasoning').getAttribute('data-active')).toBe('true');
    expect(screen.getByTestId('default-tier-btn-default').getAttribute('data-active')).toBe('false');

    resolveSet();
    await waitFor(() => expect(toastFn).toHaveBeenCalledWith('config.saved', 'success'));
  });

  it('reverts the optimistic update when the save fails', async () => {
    getServerConfigMock.mockResolvedValue({
      default_model_tier: 'default',
      host: 'localhost', port: 3140,
    });
    setServerConfigMock.mockRejectedValue(new Error('boom'));
    render(
      <AgentsSection
        agents={EMPTY_AGENTS}
        agentAccess={null}
        configLanguage="fr"
        refetchAgents={vi.fn()}
        refetchAgentAccess={vi.fn()}
        toast={toastFn}
        t={t}
      />,
    );
    await waitFor(() => {
      expect(screen.getByTestId('default-tier-btn-default').getAttribute('data-active')).toBe('true');
    });

    fireEvent.click(screen.getByTestId('default-tier-btn-reasoning'));

    await waitFor(() => {
      expect(toastFn).toHaveBeenCalledWith('config.saveError', 'error');
    });
    // Reverted : the previous tier ('default') is active again.
    expect(screen.getByTestId('default-tier-btn-default').getAttribute('data-active')).toBe('true');
    expect(screen.getByTestId('default-tier-btn-reasoning').getAttribute('data-active')).toBe('false');
  });

  it('disables the buttons while the initial value is loading', async () => {
    // Long-pending GET → no value yet → buttons disabled to prevent
    // a save firing with stale 'default' before the real value arrives.
    let resolveGet: (v: { default_model_tier: string; host: string; port: number }) => void = () => {};
    getServerConfigMock.mockReturnValue(new Promise(r => { resolveGet = r; }));
    render(
      <AgentsSection
        agents={EMPTY_AGENTS}
        agentAccess={null}
        configLanguage="fr"
        refetchAgents={vi.fn()}
        refetchAgentAccess={vi.fn()}
        toast={toastFn}
        t={t}
      />,
    );
    const ecoBtn = screen.getByTestId('default-tier-btn-economy') as HTMLButtonElement;
    expect(ecoBtn.disabled).toBe(true);

    // Once the GET resolves, the buttons become clickable.
    resolveGet({ default_model_tier: 'default', host: 'localhost', port: 3140 });
    await waitFor(() => {
      expect((screen.getByTestId('default-tier-btn-economy') as HTMLButtonElement).disabled).toBe(false);
    });
  });
});

// 0.8.6 phase 4 — Default summary strategy dropdown (gap from #1.15).
// Same surface contract as the tier picker : Off by default, 3
// options, optimistic update, revert on error.
describe('AgentsSection — default summary strategy (0.8.6 phase 4)', () => {
  it('renders 3 strategy options with Off marked active by default', async () => {
    getServerConfigMock.mockResolvedValue({
      default_model_tier: 'default',
      default_summary_strategy: 'Off',
      host: 'localhost', port: 3140,
    });
    render(
      <AgentsSection
        agents={EMPTY_AGENTS}
        agentAccess={null}
        configLanguage="fr"
        refetchAgents={vi.fn()}
        refetchAgentAccess={vi.fn()}
        toast={toastFn}
        t={t}
      />,
    );
    await waitFor(() => {
      expect(screen.getByTestId('default-summary-btn-off')).toBeTruthy();
      expect(screen.getByTestId('default-summary-btn-auto')).toBeTruthy();
      expect(screen.getByTestId('default-summary-btn-ondemand')).toBeTruthy();
    });
    await waitFor(() => {
      expect(screen.getByTestId('default-summary-btn-off').getAttribute('data-active')).toBe('true');
      expect(screen.getByTestId('default-summary-btn-auto').getAttribute('data-active')).toBe('false');
    });
  });

  it('PATCHes /config/server with default_summary_strategy on click', async () => {
    getServerConfigMock.mockResolvedValue({
      default_model_tier: 'default',
      default_summary_strategy: 'Off',
      host: 'localhost', port: 3140,
    });
    setServerConfigMock.mockResolvedValue(undefined);
    render(
      <AgentsSection
        agents={EMPTY_AGENTS}
        agentAccess={null}
        configLanguage="fr"
        refetchAgents={vi.fn()}
        refetchAgentAccess={vi.fn()}
        toast={toastFn}
        t={t}
      />,
    );
    await waitFor(() => expect(screen.getByTestId('default-summary-btn-auto')).toBeTruthy());

    fireEvent.click(screen.getByTestId('default-summary-btn-auto'));

    await waitFor(() => {
      expect(setServerConfigMock).toHaveBeenCalledWith({ default_summary_strategy: 'Auto' });
    });
    expect(toastFn).toHaveBeenCalledWith('config.saved', 'success');
  });

  it('reverts on save error', async () => {
    getServerConfigMock.mockResolvedValue({
      default_model_tier: 'default',
      default_summary_strategy: 'Off',
      host: 'localhost', port: 3140,
    });
    setServerConfigMock.mockRejectedValue(new Error('boom'));
    render(
      <AgentsSection
        agents={EMPTY_AGENTS}
        agentAccess={null}
        configLanguage="fr"
        refetchAgents={vi.fn()}
        refetchAgentAccess={vi.fn()}
        toast={toastFn}
        t={t}
      />,
    );
    await waitFor(() => expect(screen.getByTestId('default-summary-btn-auto')).toBeTruthy());

    fireEvent.click(screen.getByTestId('default-summary-btn-auto'));

    await waitFor(() => expect(toastFn).toHaveBeenCalledWith('config.saveError', 'error'));
    // Reverted to Off.
    expect(screen.getByTestId('default-summary-btn-off').getAttribute('data-active')).toBe('true');
    expect(screen.getByTestId('default-summary-btn-auto').getAttribute('data-active')).toBe('false');
  });
});
