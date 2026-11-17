import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';
import { ApiRequestError } from '../../lib/apiRequestError';

const { getMock, setMock, envMock } = vi.hoisted(() => ({
  getMock: vi.fn(),
  setMock: vi.fn(),
  envMock: vi.fn(),
}));

vi.mock('../../lib/api', () => buildApiMock({
  projects: { workspace: getMock as never, setWorkspace: setMock as never },
  config: { getFirstBuildEnvSecs: envMock as never },
}));

import { ProjectLaneWarmSetting } from '../ProjectLaneWarmSetting';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

beforeEach(() => {
  envMock.mockResolvedValue(null);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ProjectLaneWarmSetting (KT-1742)', () => {
  it('lists the Rust roots to warm next to the command and stores them with the override', async () => {
    getMock.mockResolvedValue({ hooks: {}, lane_warm: { rust_roots: ['backend'] } });
    setMock.mockResolvedValue(true);
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-warm-roots')).not.toBeDisabled());
    expect(screen.getByTestId('project-lane-warm-roots')).toHaveValue('backend');

    fireEvent.change(screen.getByTestId('project-lane-warm-roots'), { target: { value: 'backend\nredis-flush' } });
    fireEvent.click(screen.getByTestId('project-lane-warm-save'));

    await waitFor(() => expect(setMock).toHaveBeenCalledWith('p1', {
      hooks: {},
      lane_warm: { rust_roots: ['backend', 'redis-flush'] },
    }));
  });

  it('keeps the commit identity and the managed roots when it saves the warm-up', async () => {
    const commit = { name: 'Bot', email: 'bot@corp.example', signoff: 'always' };
    getMock.mockResolvedValue({
      hooks: {},
      lane_warm: { rust_roots: ['backend'] },
      managed_worktree_roots: ['/srv/worktrees'],
      commit,
    });
    setMock.mockResolvedValue(true);
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-warm-roots')).not.toBeDisabled());

    fireEvent.change(screen.getByTestId('project-lane-warm-roots'), { target: { value: 'backend\nredis-flush' } });
    fireEvent.click(screen.getByTestId('project-lane-warm-save'));

    await waitFor(() => expect(setMock).toHaveBeenCalledWith('p1', {
      hooks: {},
      lane_warm: { rust_roots: ['backend', 'redis-flush'] },
      managed_worktree_roots: ['/srv/worktrees'],
      commit,
    }));
  });

  // KT-1747 — `dependency_dirs` is set through the workspace API (the form has no
  // input for it yet): a save of the other fields must not delete it.
  it('keeps the dependency directories the project names when it saves the warm-up', async () => {
    getMock.mockResolvedValue({
      hooks: {},
      lane_warm: { rust_roots: ['backend'], dependency_dirs: ['vendor', 'assets/front/node_modules'] },
    });
    setMock.mockResolvedValue(true);
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-warm-roots')).not.toBeDisabled());

    fireEvent.change(screen.getByTestId('project-lane-warm-roots'), { target: { value: 'backend\nredis-flush' } });
    fireEvent.click(screen.getByTestId('project-lane-warm-save'));

    await waitFor(() => expect(setMock).toHaveBeenCalledWith('p1', {
      hooks: {},
      lane_warm: {
        dependency_dirs: ['vendor', 'assets/front/node_modules'],
        rust_roots: ['backend', 'redis-flush'],
      },
    }));
  });

  // KT-1751 review — the executables a project's validations may run are set
  // here, where Quick Exec's refusal sends the human.
  it('lists the validation executables and stores them with the override', async () => {
    getMock.mockResolvedValue({ hooks: {}, lane_warm: { validation_executables: ['composer'] } });
    setMock.mockResolvedValue(true);
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-warm-executables')).not.toBeDisabled());
    expect(screen.getByTestId('project-lane-warm-executables')).toHaveValue('composer');

    fireEvent.change(screen.getByTestId('project-lane-warm-executables'), {
      target: { value: 'composer\n./vendor/bin/phpunit\n./gradlew\n' },
    });
    fireEvent.click(screen.getByTestId('project-lane-warm-save'));

    await waitFor(() => expect(setMock).toHaveBeenCalledWith('p1', {
      hooks: {},
      lane_warm: { validation_executables: ['composer', 'vendor/bin/phpunit', './gradlew'] },
    }));
  });

  it('refuses a validation executable outside the repository before saving', async () => {
    getMock.mockResolvedValue(null);
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-warm-executables')).not.toBeDisabled());

    fireEvent.change(screen.getByTestId('project-lane-warm-executables'), { target: { value: '/usr/bin/php' } });

    expect(screen.getByTestId('project-lane-warm-executables-invalid')).toBeInTheDocument();
    expect(screen.getByTestId('project-lane-warm-save')).toBeDisabled();
  });

  it('refuses a root outside the repository before saving', async () => {
    getMock.mockResolvedValue(null);
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-warm-roots')).not.toBeDisabled());

    fireEvent.change(screen.getByTestId('project-lane-warm-roots'), { target: { value: '../elsewhere' } });

    expect(screen.getByTestId('project-lane-warm-roots-invalid')).toBeInTheDocument();
    expect(screen.getByTestId('project-lane-warm-save')).toBeDisabled();
  });

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

  it('keeps every field in its own group, so a label never touches the previous help line', async () => {
    getMock.mockResolvedValue(null);
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-warm-command')).not.toBeDisabled());

    const groups = screen.getByTestId('project-lane-warm-fields').querySelectorAll(':scope > .lane-warm-group');
    expect(groups).toHaveLength(6);
    for (const group of Array.from(groups)) {
      expect(group.querySelectorAll('label')).toHaveLength(1);
      expect(group.querySelector('p.text-faint')).not.toBeNull();
    }
    expect(screen.getByTestId('project-lane-warm-title')).toHaveTextContent('laneWarm.projectTitle');
  });

  it('says in every locale that a root wrapper is typed with ./ while a bare name is a PATH program (KT-1751)', async () => {
    const locales = await Promise.all([
      import('../../lib/i18n/locales/en'),
      import('../../lib/i18n/locales/fr'),
      import('../../lib/i18n/locales/es'),
      import('../../lib/i18n/locales/zh'),
    ]);
    for (const { default: dict } of locales) {
      const help = dict['laneWarm.executablesHelp'];
      expect(help).toContain('./gradlew');
      expect(help).toContain('PATH');
    }
  });

  it('tells when KRONN_WORKER_FIRST_BUILD_TIMEOUT_SECS is set on the host, as a fallback for an empty field', async () => {
    envMock.mockResolvedValue(5400);
    getMock.mockResolvedValue(null);
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);

    expect(await screen.findByTestId('project-lane-warm-timeout-env')).toHaveTextContent('laneWarm.timeoutEnvNotice:90');
    expect(screen.getByTestId('project-lane-warm-timeout')).toHaveValue(null);
  });

  it('shows no notice when the variable is not set', async () => {
    getMock.mockResolvedValue(null);
    render(<ProjectLaneWarmSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-warm-command')).not.toBeDisabled());
    expect(envMock).toHaveBeenCalled();
    expect(screen.queryByTestId('project-lane-warm-timeout-env')).not.toBeInTheDocument();
  });
});
