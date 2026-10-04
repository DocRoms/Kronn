import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import { clearCachedResources } from '../../../hooks/useCachedResource';

const { gitStatusMock, dependencyUpdatesMock } = vi.hoisted(() => ({
  gitStatusMock: vi.fn(),
  dependencyUpdatesMock: vi.fn(),
}));

vi.mock('../../../lib/api', () => buildApiMock({
  projects: {
    gitStatus: gitStatusMock as never,
    dependencyUpdates: dependencyUpdatesMock as never,
    setDependencyMonitoring: vi.fn() as never,
  },
}));

import { ProjectGitBlock } from '../ProjectGitBlock';
import { ProjectDependenciesBlock } from '../ProjectDependenciesBlock';

const git = (over: Record<string, unknown> = {}) => ({
  branch: 'main', default_branch: 'main', is_default_branch: true, files: [], committed_files: [],
  ahead: 0, behind: 0, has_upstream: true, upstream: 'origin/main', provider: 'github',
  remote_url: 'https://github.com/team/demo', pull_requests_url: null, last_tag: 'v1', pr_url: null,
  languages: [], languages_checked_at: null, languages_cached: false, ...over,
});

const deps = (over: Record<string, unknown> = {}) => ({
  managers: [{ manager: 'npm', manifest: 'package.json', status: 'UpToDate', outdated: 0, major: 0, packages: [] }],
  total_outdated: 0, total_major: 0, checked_at: '2026-10-01T10:00:00Z', cached: true,
  monitoring_interval_days: null, next_check_at: null, ...over,
});

beforeEach(() => {
  clearCachedResources();
  gitStatusMock.mockReset();
  dependencyUpdatesMock.mockReset();
});

describe('ProjectDependenciesBlock', () => {
  it('shows the last known result immediately on reopening, never the loading state', async () => {
    dependencyUpdatesMock.mockResolvedValueOnce(deps());
    const first = render(<ProjectDependenciesBlock projectId="p1" enabled />);
    await screen.findByText('projects.master.overview.dependenciesUpToDate', { selector: 'strong' });
    first.unmount();

    dependencyUpdatesMock.mockReturnValueOnce(new Promise(() => {}));
    render(<ProjectDependenciesBlock projectId="p1" enabled />);
    expect(screen.getAllByText('projects.master.overview.dependenciesUpToDate').length).toBeGreaterThan(0);
    expect(screen.queryByText('projects.master.overview.dependenciesChecking')).toBeNull();
    expect(screen.getByText(/dependenciesCheckedAt/)).toBeInTheDocument();
    expect(screen.getByTestId('dependencies-refreshing')).toBeInTheDocument();
  });

  it('keeps the data on screen when the background refresh fails', async () => {
    dependencyUpdatesMock.mockResolvedValueOnce(deps({ total_outdated: 3, total_major: 0, managers: [] }));
    const first = render(<ProjectDependenciesBlock projectId="p2" enabled />);
    await waitFor(() => expect(screen.getAllByText('projects.master.overview.dependenciesNone').length).toBeGreaterThan(0));
    first.unmount();

    dependencyUpdatesMock.mockRejectedValueOnce(new Error('registry down'));
    render(<ProjectDependenciesBlock projectId="p2" enabled />);
    await screen.findByText('projects.master.overview.refreshFailed');
    expect(screen.queryByText('projects.master.overview.dependenciesUnavailable')).toBeNull();
    expect(screen.getByText(/dependenciesCheckedAt/)).toBeInTheDocument();
  });

  it('says unavailable only when nothing was ever known', async () => {
    dependencyUpdatesMock.mockRejectedValueOnce(new Error('down'));
    render(<ProjectDependenciesBlock projectId="p3" enabled />);
    await waitFor(() =>
      expect(screen.getAllByText('projects.master.overview.dependenciesUnavailable').length).toBeGreaterThan(0));
  });

  it('fetches nothing while disabled', () => {
    render(<ProjectDependenciesBlock projectId="p4" enabled={false} />);
    expect(dependencyUpdatesMock).not.toHaveBeenCalled();
  });
});

describe('ProjectGitBlock', () => {
  it('renders the local status before the PR lookup finishes, then completes it', async () => {
    let finish!: (value: unknown) => void;
    gitStatusMock
      .mockResolvedValueOnce(git({ branch: 'feature', is_default_branch: false }))
      .mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    render(<ProjectGitBlock projectId="g1" repoUrl={null} enabled />);

    expect(await screen.findByText('feature')).toBeInTheDocument();
    expect(gitStatusMock).toHaveBeenNthCalledWith(1, 'g1', false, undefined, undefined, true);
    expect(screen.queryByRole('link', { name: /pullRequests/ })).toBeNull();
    expect(screen.getByTestId('git-refreshing')).toBeInTheDocument();

    await act(async () => {
      finish(git({ branch: 'feature', is_default_branch: false, pull_requests_url: 'https://github.com/team/demo/pulls' }));
    });
    expect(gitStatusMock).toHaveBeenNthCalledWith(2, 'g1');
    expect(screen.getByRole('link', { name: /pullRequests/ })).toHaveAttribute('href', 'https://github.com/team/demo/pulls');
    expect(screen.queryByTestId('git-refreshing')).toBeNull();
  });

  it('keeps the last status when a later refresh fails', async () => {
    gitStatusMock.mockResolvedValueOnce(git({ branch: 'kept' })).mockResolvedValueOnce(git({ branch: 'kept' }));
    const first = render(<ProjectGitBlock projectId="g2" repoUrl={null} enabled />);
    await screen.findByText('kept');
    await waitFor(() => expect(screen.queryByTestId('git-refreshing')).toBeNull());
    first.unmount();

    gitStatusMock.mockRejectedValue(new Error('gh missing'));
    render(<ProjectGitBlock projectId="g2" repoUrl={null} enabled />);
    expect(screen.getByText('kept')).toBeInTheDocument();
    await screen.findByText('projects.master.overview.refreshFailed');
    expect(screen.getByText('kept')).toBeInTheDocument();
    expect(screen.queryByText('projects.master.overview.gitUnavailable')).toBeNull();
  });

  it('forces a language refresh from the button and ignores a double click', async () => {
    gitStatusMock.mockResolvedValue(git({ languages_checked_at: '2026-10-01T10:00:00Z' }));
    render(<ProjectGitBlock projectId="g3" repoUrl={null} enabled />);
    const button = await screen.findByRole('button', { name: 'projects.master.overview.languagesRefresh' });
    await waitFor(() => expect(button).not.toBeDisabled());
    gitStatusMock.mockClear();
    fireEvent.click(button);
    fireEvent.click(button);
    await waitFor(() => expect(gitStatusMock).toHaveBeenCalledWith('g3', true));
    expect(gitStatusMock).toHaveBeenCalledTimes(1);
  });
});
