import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { projects } from '../../lib/api';
import { ProjectGitPanel } from '../ProjectGitPanel';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ locale: 'fr', t: (key: string) => key }),
}));

vi.mock('../../lib/api', () => ({
  projects: {
    gitBranches: vi.fn(),
    gitSwitchBranch: vi.fn(),
  },
}));

const overview = {
  current_branch: 'main',
  default_branch: 'main',
  branches: [
    {
      name: 'main',
      ref_name: 'refs/heads/main',
      commit: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
      subject: 'Initial commit',
      author: 'Ada',
      committed_at: 1_700_000_000,
      is_current: true,
      is_remote: false,
      upstream: 'origin/main',
      ahead: 0,
      behind: 0,
    },
    {
      name: 'feature/safe',
      ref_name: 'refs/heads/feature/safe',
      commit: 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
      subject: 'Safe branch',
      author: 'Grace',
      committed_at: 1_700_000_100,
      is_current: false,
      is_remote: false,
      upstream: null,
      ahead: 1,
      behind: 0,
    },
    {
      name: 'origin/review',
      ref_name: 'refs/remotes/origin/review',
      commit: 'cccccccccccccccccccccccccccccccccccccccc',
      subject: 'Remote review',
      author: 'Linus',
      committed_at: 1_700_000_200,
      is_current: false,
      is_remote: true,
      upstream: null,
      ahead: 0,
      behind: 1,
    },
  ],
  commits: [
    {
      hash: 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
      short_hash: 'bbbbbbbb',
      parents: ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'],
      refs: ['feature/safe'],
      subject: 'Safe branch',
      author: 'Grace',
      committed_at: 1_700_000_100,
    },
  ],
  truncated: false,
};

describe('ProjectGitPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(projects.gitBranches).mockResolvedValue(overview);
    vi.mocked(projects.gitSwitchBranch).mockResolvedValue({ branch: 'feature/safe' });
  });

  it('shows local and remote branches with a bounded recent graph', async () => {
    render(<ProjectGitPanel projectId="project-1" onBranchChanged={vi.fn()} />);

    expect(await screen.findByText('Initial commit')).toBeInTheDocument();
    expect(screen.getAllByText('feature/safe').length).toBeGreaterThan(0);
    expect(screen.getAllByText('origin/review').length).toBeGreaterThan(0);
    expect(screen.getByText('bbbbbbbb')).toBeInTheDocument();
    expect(screen.getByText('projects.git.safeSwitchHint')).toBeInTheDocument();
  });

  it('switches the selected branch then refreshes the graph', async () => {
    const onBranchChanged = vi.fn();
    render(
      <ProjectGitPanel projectId="project-1" onBranchChanged={onBranchChanged} />,
    );

    const select = await screen.findByRole('combobox', {
      name: 'projects.git.switchTitle',
    });
    fireEvent.change(select, { target: { value: 'feature/safe' } });
    fireEvent.click(screen.getByRole('button', { name: 'projects.git.switch' }));

    await waitFor(() => {
      expect(projects.gitSwitchBranch).toHaveBeenCalledWith(
        'project-1',
        'feature/safe',
      );
      expect(projects.gitBranches).toHaveBeenCalledTimes(2);
      expect(onBranchChanged).toHaveBeenCalledWith('feature/safe');
    });
  });

  it('keeps an actionable backend refusal visible', async () => {
    vi.mocked(projects.gitSwitchBranch).mockRejectedValueOnce(
      new Error('Le projet contient des modifications locales.'),
    );
    render(<ProjectGitPanel projectId="project-1" onBranchChanged={vi.fn()} />);

    const select = await screen.findByRole('combobox', {
      name: 'projects.git.switchTitle',
    });
    fireEvent.change(select, { target: { value: 'feature/safe' } });
    fireEvent.click(screen.getByRole('button', { name: 'projects.git.switch' }));

    expect(
      await screen.findByText('Le projet contient des modifications locales.'),
    ).toBeInTheDocument();
  });
});
