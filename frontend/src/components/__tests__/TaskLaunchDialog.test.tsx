import { beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

const mocks = vi.hoisted(() => ({
  detect: vi.fn(),
  profiles: vi.fn(),
  createCampaign: vi.fn(),
  launch: vi.fn(),
  catalog: vi.fn(),
  workers: vi.fn(),
}));

vi.mock('../../lib/api', () => buildApiMock({
  agents: { detect: mocks.detect as never },
  profiles: { list: mocks.profiles as never },
  orchestration: {
    createCampaign: mocks.createCampaign as never,
    launch: mocks.launch as never,
  },
  modelCatalogApi: { list: mocks.catalog as never },
  config: { getWorkers: mocks.workers as never },
}));

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

import { TaskLaunchDialog } from '../TaskLaunchDialog';
import { orchestrationResolution } from '../taskLaunchResolution';
import type { CampaignView } from '../../lib/api';

const campaign = {
  run: {
    id: 'campaign-1',
    allowed_agents: ['Codex'],
    validations: [],
  },
  candidates: [],
  principal_attention: {},
} as unknown as CampaignView;

describe('TaskLaunchDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.detect.mockResolvedValue([{
      name: 'Codex', agent_type: 'Codex', installed: true, enabled: true,
      runtime_available: false, auth_ready: true,
    }]);
    mocks.profiles.mockResolvedValue([]);
    mocks.createCampaign.mockResolvedValue(campaign);
    mocks.launch.mockResolvedValue({ execution: { id: 'exec-1' } });
    mocks.catalog.mockResolvedValue({ targets: [] });
    mocks.workers.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: false });
  });

  const renderDialog = (overrides: Partial<React.ComponentProps<typeof TaskLaunchDialog>> = {}) => {
    const props: React.ComponentProps<typeof TaskLaunchDialog> = {
      open: true,
      discussionId: 'disc-1',
      projectId: 'project-1',
      taskReference: 'KT-323',
      defaultAgent: 'Codex',
      defaultBranch: 'main',
      workspaces: [],
      campaign: null,
      onClose: vi.fn(),
      onLaunched: vi.fn(),
      ...overrides,
    };
    render(<TaskLaunchDialog {...props} />);
    return props;
  };

  it('creates one campaign and launches once even after a double click', async () => {
    const props = renderDialog();
    await waitFor(() => expect(mocks.detect).toHaveBeenCalled());

    const launch = screen.getByRole('button', { name: 'orch.launch' });
    fireEvent.click(launch);
    fireEvent.click(launch);

    await waitFor(() => expect(props.onLaunched).toHaveBeenCalledWith('exec-1', campaign));
    expect(mocks.createCampaign).toHaveBeenCalledTimes(1);
    expect(mocks.createCampaign).toHaveBeenCalledWith(expect.objectContaining({
      discussion_id: 'disc-1',
      target_branch: 'main',
      integration_strategy: 'two_phase_ff_only',
      allowed_agents: ['Codex'],
    }));
    expect(mocks.launch).toHaveBeenCalledTimes(1);
    expect(mocks.launch).toHaveBeenCalledWith('campaign-1', 'KT-323', expect.objectContaining({
      worker: expect.objectContaining({ target: expect.objectContaining({ agent_type: 'Codex' }) }),
    }));
  });

  it('searches the saved catalogue controls and clears an old model only after an explicit target change', async () => {
    mocks.detect.mockResolvedValue([
      { name: 'Codex', agent_type: 'Codex', installed: true, enabled: true, runtime_available: false, auth_ready: true },
      { name: 'OpenCode', agent_type: 'OpenCode', installed: true, enabled: true, runtime_available: false, auth_ready: true },
    ]);
    mocks.catalog.mockResolvedValue({ targets: [{
      runtime_target_id: 'agent:opencode', agent_type: 'OpenCode', stale: false, live_refresh_ok: true,
      models: [{ id: 'open-model', runtime_target_id: 'agent:opencode', agent_type: 'OpenCode',
        model_id: 'open-model', display_name: 'Open model', tier_assignment: null, provenance: 'live',
        availability: 'available', capabilities: ['chat'], reasoning_modes: [], manual_origin: false,
        first_seen_at: '2026-09-09T00:00:00Z', last_checked_at: '2026-09-09T00:00:00Z',
        created_at: '2026-09-09T00:00:00Z', updated_at: '2026-09-09T00:00:00Z' }],
    }] });
    const props = renderDialog();
    await waitFor(() => expect(mocks.detect).toHaveBeenCalled());

    const model = screen.getByRole('combobox', { name: 'wiz.model' });
    fireEvent.focus(model);
    fireEvent.change(model, { target: { value: 'legacy-override' } });
    fireEvent.keyDown(model, { key: 'Enter' });
    expect(model).toHaveValue('legacy-override — modelCatalog.notInCatalog');

    fireEvent.click(screen.getByRole('button', { name: 'orch.config.agent' }));
    const search = screen.getByRole('searchbox', { name: 'agentPicker.search' });
    fireEvent.change(search, { target: { value: 'opencode' } });
    fireEvent.click(screen.getByRole('menuitem', { name: 'OpenCode' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'orch.config.agent' })).toHaveTextContent('OpenCode'));
    expect(screen.getByRole('combobox', { name: 'wiz.model' })).toHaveValue('');

    fireEvent.focus(screen.getByRole('combobox', { name: 'wiz.model' }));
    fireEvent.change(screen.getByRole('combobox', { name: 'wiz.model' }), { target: { value: 'Open' } });
    fireEvent.click(screen.getByRole('option', { name: 'Open model' }));
    fireEvent.click(screen.getByRole('button', { name: 'orch.launch' }));

    await waitFor(() => expect(props.onLaunched).toHaveBeenCalled());
    expect(mocks.createCampaign).toHaveBeenCalledWith(expect.objectContaining({
      allowed_agents: expect.arrayContaining(['OpenCode']),
      default_worker: expect.objectContaining({
        target: expect.objectContaining({
          kind: 'agent', agent_type: 'OpenCode', cli_session_id: null,
        }),
        model: 'open-model',
      }),
    }));
    expect(mocks.launch).toHaveBeenCalledWith('campaign-1', 'KT-323', expect.objectContaining({
      worker: expect.objectContaining({
        target: expect.objectContaining({
          kind: 'agent', agent_type: 'OpenCode', cli_session_id: null,
        }),
      }),
    }));
  });

  // KT-1345 — the review-round limit starts from Settings > Workers.
  it('pre-fills the review rounds from Settings > Workers and still lets the run override it', async () => {
    mocks.workers.mockResolvedValue({ default_max_review_rounds: 9, review_rounds_unlimited: false });
    const props = renderDialog();
    const rounds = screen.getByTestId('launch-review-rounds');
    await waitFor(() => expect(rounds).toHaveValue(9));
    expect(screen.queryByTestId('launch-review-warning')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'orch.launch' }));
    await waitFor(() => expect(props.onLaunched).toHaveBeenCalled());
    expect(mocks.createCampaign).toHaveBeenLastCalledWith(expect.objectContaining({ max_review_rounds: 9 }));

    fireEvent.change(rounds, { target: { value: '2' } });
    expect(rounds).toHaveValue(2);
  });

  it('sends the per-run override instead of the default', async () => {
    mocks.workers.mockResolvedValue({ default_max_review_rounds: 9, review_rounds_unlimited: false });
    const props = renderDialog();
    const rounds = screen.getByTestId('launch-review-rounds');
    await waitFor(() => expect(rounds).toHaveValue(9));
    fireEvent.change(rounds, { target: { value: '2' } });
    fireEvent.click(screen.getByRole('button', { name: 'orch.launch' }));
    await waitFor(() => expect(props.onLaunched).toHaveBeenCalled());
    expect(mocks.createCampaign).toHaveBeenLastCalledWith(expect.objectContaining({ max_review_rounds: 2 }));
  });

  it('opens unlimited with the token-burn warning when that is the Workers default', async () => {
    mocks.workers.mockResolvedValue({ default_max_review_rounds: 5, review_rounds_unlimited: true });
    const props = renderDialog();
    expect(await screen.findByTestId('launch-review-warning')).toHaveTextContent('config.workers.unlimitedWarning');
    expect(screen.getByTestId('launch-review-unlimited')).toBeChecked();
    fireEvent.click(screen.getByRole('button', { name: 'orch.launch' }));
    await waitFor(() => expect(props.onLaunched).toHaveBeenCalled());
    expect(mocks.createCampaign).toHaveBeenLastCalledWith(expect.objectContaining({ max_review_rounds: 'unlimited' }));
  });

  it('does not let a late Workers default overwrite what the user already typed', async () => {
    let resolveWorkers: (value: unknown) => void = () => {};
    mocks.workers.mockReturnValue(new Promise(resolve => { resolveWorkers = resolve; }));
    renderDialog();
    const rounds = screen.getByTestId('launch-review-rounds');
    fireEvent.change(rounds, { target: { value: '4' } });
    resolveWorkers({ default_max_review_rounds: 9, review_rounds_unlimited: false });
    await waitFor(() => expect(mocks.workers).toHaveBeenCalled());
    await Promise.resolve();
    expect(rounds).toHaveValue(4);
  });

  it('leaves the limit out while the Workers default is still loading, so the server applies it', async () => {
    mocks.workers.mockReturnValue(new Promise(() => {}));
    const props = renderDialog();
    expect(screen.getByTestId('launch-review-rounds')).toHaveValue(null);
    fireEvent.click(screen.getByRole('button', { name: 'orch.launch' }));
    await waitFor(() => expect(props.onLaunched).toHaveBeenCalled());
    expect(mocks.createCampaign.mock.calls[0][0]).not.toHaveProperty('max_review_rounds');
  });

  it('never substitutes a built-in figure when the Workers default cannot be read', async () => {
    mocks.workers.mockRejectedValue(new Error('boom'));
    const props = renderDialog();
    expect(await screen.findByTestId('launch-review-unavailable')).toHaveTextContent(
      'orch.config.reviewRoundsUnavailable',
    );
    expect(screen.getByTestId('launch-review-rounds')).toHaveValue(null);
    fireEvent.click(screen.getByRole('button', { name: 'orch.launch' }));
    await waitFor(() => expect(props.onLaunched).toHaveBeenCalled());
    expect(mocks.createCampaign.mock.calls[0][0]).not.toHaveProperty('max_review_rounds');
  });

  it('still sends what the human typed when the Workers default could not be read', async () => {
    mocks.workers.mockRejectedValue(new Error('boom'));
    const props = renderDialog();
    await screen.findByTestId('launch-review-unavailable');
    fireEvent.change(screen.getByTestId('launch-review-rounds'), { target: { value: '7' } });
    expect(screen.queryByTestId('launch-review-unavailable')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'orch.launch' }));
    await waitFor(() => expect(props.onLaunched).toHaveBeenCalled());
    expect(mocks.createCampaign).toHaveBeenLastCalledWith(expect.objectContaining({ max_review_rounds: 7 }));
  });

  it('closes with Escape without starting work', () => {
    const props = renderDialog();
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(props.onClose).toHaveBeenCalledTimes(1);
    expect(mocks.launch).not.toHaveBeenCalled();
  });

  it('keeps the dialog open when Escape closes the model picker, then closes and restores focus outside a picker', async () => {
    const trigger = document.createElement('button');
    document.body.appendChild(trigger);
    trigger.focus();
    const props = renderDialog();
    const model = screen.getByRole('combobox', { name: 'wiz.model' });

    fireEvent.focus(model);
    fireEvent.keyDown(model, { key: 'Escape' });
    expect(props.onClose).not.toHaveBeenCalled();
    expect(mocks.launch).not.toHaveBeenCalled();

    fireEvent.keyDown(window, { key: 'Escape' });
    expect(props.onClose).toHaveBeenCalledTimes(1);
    cleanup();
    expect(document.activeElement).toBe(trigger);
    trigger.remove();
  });

  it('keeps the dialog open when Escape closes the agent picker portal and restores focus to its trigger', async () => {
    mocks.detect.mockResolvedValue([
      { name: 'Codex', agent_type: 'Codex', installed: true, enabled: true, runtime_available: false, auth_ready: true },
      { name: 'OpenCode', agent_type: 'OpenCode', installed: true, enabled: true, runtime_available: false, auth_ready: true },
    ]);
    const props = renderDialog();
    const agent = await screen.findByRole('button', { name: 'orch.config.agent' });
    fireEvent.click(agent);
    const search = screen.getByRole('searchbox', { name: 'agentPicker.search' });
    fireEvent.keyDown(search, { key: 'Escape' });

    await waitFor(() => expect(screen.queryByRole('searchbox', { name: 'agentPicker.search' })).not.toBeInTheDocument());
    expect(props.onClose).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(agent);
    expect(mocks.launch).not.toHaveBeenCalled();
  });

  it('does not nest catalogue picker labels', () => {
    renderDialog();
    const dialog = screen.getByRole('dialog');
    expect(dialog.querySelector('label label')).toBeNull();
    expect(screen.getByText('Codex')).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'wiz.model' })).toBeInTheDocument();
  });

  it('shows a known unavailable model as non-selectable', async () => {
    mocks.catalog.mockResolvedValue({ targets: [{
      runtime_target_id: 'agent:codex', agent_type: 'Codex', stale: false, live_refresh_ok: true,
      models: [{ id: 'offline-model', runtime_target_id: 'agent:codex', agent_type: 'Codex',
        model_id: 'offline-model', display_name: 'Offline model', tier_assignment: null, provenance: 'live',
        availability: 'unavailable', capabilities: ['chat'], reasoning_modes: [], manual_origin: false,
        first_seen_at: '2026-09-09T00:00:00Z', last_checked_at: '2026-09-09T00:00:00Z',
        created_at: '2026-09-09T00:00:00Z', updated_at: '2026-09-09T00:00:00Z' }],
    }] });
    renderDialog();
    const model = screen.getByRole('combobox', { name: 'wiz.model' });
    fireEvent.focus(model);
    expect(await screen.findByRole('option', { name: '❌ Offline model — modelCatalog.unavailable' })).toBeDisabled();
  });

  it('keeps and submits a historical model override when the saved catalogue is unavailable', async () => {
    mocks.catalog.mockRejectedValue(new Error('catalogue unavailable'));
    const historicalCampaign = {
      ...campaign,
      run: {
        ...campaign.run,
        default_worker: {
          target: { kind: 'agent', agent_type: 'Codex', cli_session_id: null, tier: null },
          model: 'historical-override', profile_id: null,
        },
      },
    } as CampaignView;
    const props = renderDialog({ campaign: historicalCampaign });
    const model = screen.getByRole('combobox', { name: 'wiz.model' });
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('modelCatalog.loadError'));
    expect(model).toHaveValue('historical-override — modelCatalog.notInCatalog');

    fireEvent.click(screen.getByRole('button', { name: 'orch.launch' }));
    await waitFor(() => expect(props.onLaunched).toHaveBeenCalled());
    expect(mocks.catalog).toHaveBeenCalledTimes(1);
    expect(mocks.createCampaign).not.toHaveBeenCalled();
    expect(mocks.launch).toHaveBeenCalledWith('campaign-1', 'KT-323', expect.objectContaining({
      worker: expect.objectContaining({ model: 'historical-override' }),
    }));
  });

  it('does not offer agents outside a locked campaign policy', async () => {
    mocks.detect.mockResolvedValue([
      { name: 'Codex', agent_type: 'Codex', installed: true, enabled: true, runtime_available: false, auth_ready: true },
      { name: 'OpenCode', agent_type: 'OpenCode', installed: true, enabled: true, runtime_available: false, auth_ready: true },
    ]);
    renderDialog({ campaign });
    await waitFor(() => expect(mocks.detect).toHaveBeenCalled());
    expect(screen.queryByRole('button', { name: 'orch.config.agent' })).not.toBeInTheDocument();
    expect(screen.queryByText('OpenCode')).not.toBeInTheDocument();
  });

  it('maps runtime failures to an actionable recovery instead of raw prose', () => {
    expect(orchestrationResolution('Fast-forward conflict')).toBe('resolve_git');
    expect(orchestrationResolution('Validation command failed')).toBe('fix_tests');
    expect(orchestrationResolution('Provider quota expired')).toBe('reassign_agent');
    expect(orchestrationResolution('Workspace is missing')).toBe('restore_workspace');
    expect(orchestrationResolution('Unexpected response')).toBe('retry');
  });
});
