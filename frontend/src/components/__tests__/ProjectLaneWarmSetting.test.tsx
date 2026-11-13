import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';
import { ApiRequestError } from '../../lib/apiRequestError';

const { getMock, setMock } = vi.hoisted(() => ({
  getMock: vi.fn(),
  setMock: vi.fn(),
}));

vi.mock('../../lib/api', () => buildApiMock({
  projects: { workspace: getMock as never, setWorkspace: setMock as never },
}));

import { ProjectLaneWarmSetting } from '../ProjectLaneWarmSetting';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ProjectLaneWarmSetting (KT-1742)', () => {
  it('starts on « inherit » for a project that has no workspace settings', async () => {
    getMock.mockResolvedValue(null);
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);

    await waitFor(() => expect(screen.getByTestId('project-lane-warm-command')).not.toBeDisabled());
    expect(getMock).toHaveBeenCalledWith('p1');
    expect(screen.getByTestId('project-lane-warm-alignment')).toHaveValue('');
    expect(screen.getByTestId('project-lane-warm-env-mode')).toHaveValue('inherit');
    expect(screen.getByTestId('project-lane-warm-save')).toBeDisabled();
  });

  it('stores the project override and sends the worktree hooks back as they were read', async () => {
    getMock.mockResolvedValue({ hooks: { after_create: 'make setup' }, lane_warm: {} });
    setMock.mockResolvedValue(true);
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-warm-command')).not.toBeDisabled());

    fireEvent.change(screen.getByTestId('project-lane-warm-command'), { target: { value: 'composer install' } });
    fireEvent.change(screen.getByTestId('project-lane-warm-timeout'), { target: { value: '20' } });
    fireEvent.change(screen.getByTestId('project-lane-warm-alignment'), { target: { value: 'off' } });
    fireEvent.click(screen.getByTestId('project-lane-warm-save'));

    await waitFor(() => expect(setMock).toHaveBeenCalledWith('p1', {
      hooks: { after_create: 'make setup' },
      lane_warm: { seed_alignment: false, first_build_command: 'composer install', first_build_timeout_min: 20 },
    }));
    await waitFor(() => expect(screen.getByTestId('project-lane-warm-save')).toBeDisabled());
  });

  it('shows what the project already overrides', async () => {
    getMock.mockResolvedValue({ hooks: {}, lane_warm: { compiler_env: [], first_build_timeout_min: 120 } });
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);

    await waitFor(() => expect(screen.getByTestId('project-lane-warm-env-mode')).toHaveValue('none'));
    expect(screen.getByTestId('project-lane-warm-timeout')).toHaveValue(120);
  });

  it('says why a save was refused', async () => {
    getMock.mockResolvedValue(null);
    setMock.mockRejectedValue(new ApiRequestError('compiler_env cannot set `KRONN_PORT`: the lane owns it', 'validation'));
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-warm-command')).not.toBeDisabled());

    fireEvent.change(screen.getByTestId('project-lane-warm-command'), { target: { value: 'npm ci' } });
    fireEvent.click(screen.getByTestId('project-lane-warm-save'));

    expect(await screen.findByRole('alert')).toHaveTextContent('the lane owns it');
  });
});
