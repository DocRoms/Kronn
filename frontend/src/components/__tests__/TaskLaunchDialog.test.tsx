import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

const mocks = vi.hoisted(() => ({
  detect: vi.fn(),
  profiles: vi.fn(),
  createCampaign: vi.fn(),
  launch: vi.fn(),
  catalog: vi.fn(),
}));

vi.mock('../../lib/api', () => buildApiMock({
  agents: { detect: mocks.detect as never },
  profiles: { list: mocks.profiles as never },
  orchestration: {
    createCampaign: mocks.createCampaign as never,
    launch: mocks.launch as never,
  },
  modelCatalogApi: { list: mocks.catalog as never },
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

  it('closes with Escape without starting work', () => {
    const props = renderDialog();
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(props.onClose).toHaveBeenCalledTimes(1);
    expect(mocks.launch).not.toHaveBeenCalled();
  });

  it('maps runtime failures to an actionable recovery instead of raw prose', () => {
    expect(orchestrationResolution('Fast-forward conflict')).toBe('resolve_git');
    expect(orchestrationResolution('Validation command failed')).toBe('fix_tests');
    expect(orchestrationResolution('Provider quota expired')).toBe('reassign_agent');
    expect(orchestrationResolution('Workspace is missing')).toBe('restore_workspace');
    expect(orchestrationResolution('Unexpected response')).toBe('retry');
  });
});
