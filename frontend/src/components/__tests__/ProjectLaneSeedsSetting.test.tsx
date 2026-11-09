import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

const { getMock, setMock, candidatesMock } = vi.hoisted(() => ({
  getMock: vi.fn(),
  setMock: vi.fn(),
  candidatesMock: vi.fn(),
}));

vi.mock('../../lib/api', () => buildApiMock({
  projects: {
    laneSeeds: getMock as never,
    setLaneSeeds: setMock as never,
    laneSeedCandidates: candidatesMock as never,
  },
}));

import { ProjectLaneSeedsSetting } from '../ProjectLaneSeedsSetting';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

const view = (over: Record<string, unknown> = {}) => ({
  seeds: [],
  source: 'none',
  needs_confirmation: [],
  profile_file_written: false,
  ...over,
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('ProjectLaneSeedsSetting (KT-1438)', () => {
  it('lists, adds and removes seeds, sending the whole list each time', async () => {
    getMock.mockResolvedValue(view({ seeds: ['application/public/images'], source: 'project' }));
    setMock
      .mockResolvedValueOnce(view({ seeds: ['application/public/images', 'application/public/build'], source: 'project' }))
      .mockResolvedValueOnce(view({ seeds: ['application/public/build'], source: 'project' }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByText('application/public/images')).toBeInTheDocument());

    fireEvent.change(screen.getByTestId('project-lane-seeds-input'), { target: { value: ' application/public/build ' } });
    fireEvent.click(screen.getByTestId('project-lane-seeds-add'));
    await waitFor(() => expect(screen.getByText('application/public/build')).toBeInTheDocument());
    expect(setMock).toHaveBeenLastCalledWith('p1', ['application/public/images', 'application/public/build'], []);

    fireEvent.click(screen.getByTestId('project-lane-seeds-remove-application/public/images'));
    await waitFor(() => expect(screen.queryByText('application/public/images')).toBeNull());
    expect(setMock).toHaveBeenLastCalledWith('p1', ['application/public/build'], []);
  });

  it('suggests candidates with their sizes and adds one on a click', async () => {
    getMock.mockResolvedValue(view());
    candidatesMock.mockResolvedValue([
      { path: 'application/public/images', size_bytes: 16 * 1024 * 1024, reason: 'gitignored; tracked code mentions `images/`' },
      { path: 'application/public/build', size_bytes: null, reason: 'gitignored; tracked code mentions `build/`' },
    ]);
    setMock.mockResolvedValue(view({ seeds: ['application/public/images'], source: 'project' }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-empty')).toBeInTheDocument());

    fireEvent.click(screen.getByTestId('project-lane-seeds-suggest'));
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-candidates')).toBeInTheDocument());
    expect(screen.getByText(/16\.0 MB/)).toBeInTheDocument();
    expect(screen.getByText(/projects\.laneSeeds\.sizeUnknown/)).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('project-lane-seeds-candidate-application/public/images'));
    await waitFor(() => expect(setMock).toHaveBeenCalledWith('p1', ['application/public/images'], []));
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-candidate-application/public/images')).toBeDisabled());
  });

  it('holds a secret-looking path until the human confirms it', async () => {
    getMock.mockResolvedValue(view({ seeds: ['a/b'], source: 'project' }));
    setMock
      .mockResolvedValueOnce(view({ seeds: ['a/b'], source: 'project', needs_confirmation: ['.env'] }))
      .mockResolvedValueOnce(view({ seeds: ['a/b', '.env'], source: 'project' }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByText('a/b')).toBeInTheDocument());

    fireEvent.change(screen.getByTestId('project-lane-seeds-input'), { target: { value: '.env' } });
    fireEvent.click(screen.getByTestId('project-lane-seeds-add'));
    const warning = await screen.findByTestId('project-lane-seeds-secret-warning');
    expect(warning).toHaveTextContent('.env');
    expect(setMock).toHaveBeenLastCalledWith('p1', ['a/b', '.env'], []);
    expect(screen.queryByTestId('project-lane-seeds-remove-.env')).toBeNull();

    fireEvent.click(screen.getByTestId('project-lane-seeds-secret-confirm'));
    await waitFor(() => expect(screen.queryByTestId('project-lane-seeds-secret-warning')).toBeNull());
    expect(setMock).toHaveBeenLastCalledWith('p1', ['a/b', '.env'], ['.env']);
    expect(screen.getByText('.env')).toBeInTheDocument();
  });

  it('drops a held secret-looking path when the human cancels', async () => {
    getMock.mockResolvedValue(view());
    setMock.mockResolvedValue(view({ needs_confirmation: ['config/server.key'] }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-empty')).toBeInTheDocument());

    fireEvent.change(screen.getByTestId('project-lane-seeds-input'), { target: { value: 'config/server.key' } });
    fireEvent.click(screen.getByTestId('project-lane-seeds-add'));
    await screen.findByTestId('project-lane-seeds-secret-warning');
    fireEvent.click(screen.getByText('projects.laneSeeds.cancel'));
    expect(screen.queryByTestId('project-lane-seeds-secret-warning')).toBeNull();
    expect(setMock).toHaveBeenCalledTimes(1);
  });

  it('says why a seed was skipped in the last seeding', async () => {
    getMock.mockResolvedValue(view({
      seeds: ['application/public/images', 'application/public/build'],
      source: 'project',
      last_run: {
        execution_id: 'e1',
        at: '2026-11-09T10:00:00Z',
        seeds: [
          { path: 'application/public/images', outcome: 'seeded' },
          { path: 'application/public/build', outcome: 'skipped', detail: 'the main checkout does not have it (generate it there first)' },
        ],
      },
    }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);

    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-last-run')).toBeInTheDocument());
    expect(screen.getByTestId('project-lane-seeds-outcome-application/public/build'))
      .toHaveTextContent('the main checkout does not have it (generate it there first)');
    expect(screen.getByTestId('project-lane-seeds-outcome-application/public/build'))
      .toHaveTextContent('projects.laneSeeds.outcome.skipped');
    expect(screen.getByTestId('project-lane-seeds-outcome-application/public/images'))
      .toHaveTextContent('projects.laneSeeds.outcome.seeded');
  });

  it('shows the project.toml declaration, offers to adopt it and says when the file was rewritten', async () => {
    getMock.mockResolvedValue(view({
      seeds: ['application/public/images'],
      source: 'project',
      profile_seeds: ['application/public/images', 'application/public/build'],
    }));
    setMock.mockResolvedValue(view({
      seeds: ['application/public/images', 'application/public/build'],
      source: 'project',
      profile_seeds: ['application/public/images', 'application/public/build'],
      profile_file_written: true,
    }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);

    const differs = await screen.findByTestId('project-lane-seeds-profile-differs');
    expect(differs).toHaveTextContent('application/public/images, application/public/build');
    fireEvent.click(screen.getByTestId('project-lane-seeds-use-profile'));
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-file-written')).toBeInTheDocument());
    expect(setMock).toHaveBeenCalledWith('p1', ['application/public/images', 'application/public/build'], []);
    expect(screen.queryByTestId('project-lane-seeds-profile-differs')).toBeNull();
  });

  it('says the file applies when nothing is declared in Kronn', async () => {
    getMock.mockResolvedValue(view({
      seeds: ['application/public/images'],
      source: 'profile',
      profile_seeds: ['application/public/images'],
    }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByTestId('project-lane-seeds-from-profile')).toBeInTheDocument());
    expect(screen.queryByTestId('project-lane-seeds-profile-differs')).toBeNull();
  });

  it('shows a file that cannot be kept in step', async () => {
    getMock.mockResolvedValue(view({ profile_error: 'kronn/project.toml is invalid' }));
    render(<ProjectLaneSeedsSetting projectId="p1" t={t} />);
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('kronn/project.toml is invalid'));
  });
});
