import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

const mocks = vi.hoisted(() => ({
  detect: vi.fn(),
  profiles: vi.fn(),
  createCampaign: vi.fn(),
  launch: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  agents: { detect: mocks.detect },
  profiles: { list: mocks.profiles },
  orchestration: {
    createCampaign: mocks.createCampaign,
    launch: mocks.launch,
  },
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
