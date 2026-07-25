import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { projects } from '../../lib/api';
import type { GitStatusResponse } from '../../types/generated';
import { ProjectCodePanel } from '../ProjectCodePanel';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

vi.mock('../SourceCodeViewer', () => ({
  SourceCodeViewer: ({ projectId }: { projectId: string }) => (
    <div data-testid="source-viewer">{projectId}</div>
  ),
}));

vi.mock('../../lib/api', () => ({
  projects: {
    gitStatus: vi.fn(),
    gitDiff: vi.fn(),
  },
}));

const status: GitStatusResponse = {
  branch: 'feature/planning',
  default_branch: 'main',
  is_default_branch: false,
  files: [{ path: 'src/current.ts', status: 'modified', staged: false }],
  committed_files: [{ path: 'src/committed.ts', status: 'added', staged: true }],
  ahead: 1,
  behind: 0,
  has_upstream: true,
  upstream: 'origin/feature/planning',
  provider: 'github',
  remote_url: 'https://github.com/team/repo',
  pull_requests_url: 'https://github.com/team/repo/pulls',
  last_tag: 'v0.9.0',
  pr_url: null,
  languages: [],
  languages_checked_at: null,
  languages_cached: false,
};

describe('ProjectCodePanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(projects.gitStatus).mockResolvedValue(status);
    vi.mocked(projects.gitDiff).mockImplementation(async (_id, path) => ({
      path,
      diff: `@@ -1 +1 @@\n+${path}`,
    }));
  });

  it('keeps source browsing as the default and lazily loads Git changes', async () => {
    render(<ProjectCodePanel projectId="project-1" />);
    expect(screen.getByTestId('source-viewer')).toHaveTextContent('project-1');
    expect(projects.gitStatus).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: /projects.code.changes/ }));

    await waitFor(() => {
      expect(projects.gitStatus).toHaveBeenCalledWith('project-1');
      expect(projects.gitDiff).toHaveBeenCalledWith('project-1', 'src/current.ts', false);
    });
    expect(await screen.findByText('src/committed.ts')).toBeInTheDocument();
  });

  it('loads the cumulative branch diff when a committed file is selected', async () => {
    render(<ProjectCodePanel projectId="project-1" />);
    fireEvent.click(screen.getByRole('button', { name: /projects.code.changes/ }));

    fireEvent.click(await screen.findByRole('button', { name: /src\/committed.ts/ }));
    await waitFor(() => {
      expect(projects.gitDiff).toHaveBeenLastCalledWith(
        'project-1',
        'src/committed.ts',
        true,
      );
    });
  });

  it('shows a bounded error state when Git status cannot be read', async () => {
    vi.mocked(projects.gitStatus).mockRejectedValueOnce(new Error('not a repository'));
    render(<ProjectCodePanel projectId="project-1" />);

    fireEvent.click(screen.getByRole('button', { name: /projects.code.changes/ }));

    expect(await screen.findByText('projects.code.loadError')).toBeInTheDocument();
  });
});
