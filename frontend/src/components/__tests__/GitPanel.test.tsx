import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { GitPanel } from '../GitPanel';
import { projects as projectsApi } from '../../lib/api';

// ─── Mock API ────────────────────────────────────────────────────────────────

let gitStatusOverride: ReturnType<typeof makeMockGitStatus> | null = null;

function makeMockGitStatus(extra?: Partial<ReturnType<typeof baseGitStatus>>) {
  return { ...baseGitStatus(), ...(extra || {}) };
}

function baseGitStatus() {
  return {
    branch: 'feat/new-feature',
    default_branch: 'main',
    is_default_branch: false,
    files: [
      { path: 'src/main.rs', status: 'modified', staged: false },
      { path: 'src/lib.rs', status: 'added', staged: false },
      { path: 'old.txt', status: 'deleted', staged: true },
    ] as Array<{ path: string; status: string; staged: boolean }>,
    committed_files: [] as Array<{ path: string; status: string; staged: boolean }>,
    commits: [] as Array<{
      sha: string;
      short_sha: string;
      subject: string;
      author_name: string;
      author_time: number;
    }>,
    commits_total: 0,
    commits_offset: 0,
    commits_truncated: false,
    workspace: null,
    empty_reason: null,
    ahead: 2,
    behind: 0,
    has_upstream: true,
    upstream: 'origin/feat/new-feature',
    provider: 'github',
    remote_url: 'https://github.com/test/repo',
    pull_requests_url: 'https://github.com/test/repo/pulls',
    last_tag: null,
    pr_url: null,
    languages: [],
    languages_checked_at: null,
    languages_cached: false,
  };
}

vi.mock('../../lib/api', () => ({
  projects: {
    gitStatus: vi.fn().mockImplementation(() => Promise.resolve(gitStatusOverride ?? makeMockGitStatus())),
    gitDiff: vi.fn().mockResolvedValue({ diff: '@@ -1,3 +1,4 @@\n+new line' }),
    gitCommit: vi.fn().mockResolvedValue({}),
    gitPush: vi.fn().mockResolvedValue({}),
    gitCreateBranch: vi.fn().mockResolvedValue({}),
    gitPr: vi.fn().mockResolvedValue({ url: 'https://github.com/test/pr/1' }),
    prTemplate: vi.fn().mockResolvedValue({ title: '', body: '' }),
  },
  discussions: {
    workspaces: vi.fn().mockResolvedValue([]),
    gitStatus: vi.fn().mockImplementation(() => Promise.resolve(gitStatusOverride ?? makeMockGitStatus())),
    gitDiff: vi.fn().mockResolvedValue({ diff: '@@ diff content @@' }),
    gitCommit: vi.fn().mockResolvedValue({}),
    gitPush: vi.fn().mockResolvedValue({}),
  },
}));

// Mock I18nContext
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

// ─── Tests ───────────────────────────────────────────────────────────────────

describe('GitPanel', () => {
  const onClose = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
    gitStatusOverride = null;
  });

  it('renders loading state initially', () => {
    render(<GitPanel projectId="p1" onClose={onClose} />);
    // The panel header with git.title should be rendered immediately
    expect(screen.getByText('git.title')).toBeDefined();
  });

  it('renders branch name after loading', async () => {
    render(<GitPanel projectId="p1" onClose={onClose} />);
    await waitFor(() => {
      expect(screen.getByText('feat/new-feature')).toBeDefined();
    });
  });

  it('reports expanded state and restores it when the panel unmounts', async () => {
    const onExpandedChange = vi.fn();
    const { unmount } = render(
      <GitPanel
        projectId="p1"
        onClose={onClose}
        onExpandedChange={onExpandedChange}
      />,
    );
    await screen.findByText('feat/new-feature');

    fireEvent.click(screen.getByLabelText('git.expandPanel'));
    expect(onExpandedChange).toHaveBeenCalledWith(true);

    unmount();
    expect(onExpandedChange).toHaveBeenLastCalledWith(false);
  });

  it('renders file list with correct statuses', async () => {
    render(<GitPanel projectId="p1" onClose={onClose} />);
    await waitFor(() => {
      expect(screen.getByText('src/main.rs')).toBeDefined();
      expect(screen.getByText('src/lib.rs')).toBeDefined();
      expect(screen.getByText('old.txt')).toBeDefined();
    });
  });

  it('shows ahead badge when commits ahead', async () => {
    render(<GitPanel projectId="p1" onClose={onClose} />);
    await waitFor(() => {
      expect(screen.getByText('↑2')).toBeDefined();
    });
  });

  it('calls onClose when close button is clicked', async () => {
    render(<GitPanel projectId="p1" onClose={onClose} />);
    await waitFor(() => {
      expect(screen.getByText('feat/new-feature')).toBeDefined();
    });
    const closeBtn = screen.getByLabelText('Close git panel');
    fireEvent.click(closeBtn);
    expect(onClose).toHaveBeenCalled();
  });

  it('renders error when no project or discussion ID', async () => {
    render(<GitPanel onClose={onClose} />);
    await waitFor(() => {
      expect(screen.getByText(/no project|error/i)).toBeDefined();
    });
  });

  it('works with discussionId instead of projectId', async () => {
    render(<GitPanel discussionId="d1" onClose={onClose} />);
    await waitFor(() => {
      expect(screen.getByText('feat/new-feature')).toBeDefined();
    });
  });

  it('shows file selection checkboxes in commit mode', async () => {
    render(<GitPanel projectId="p1" onClose={onClose} />);
    await waitFor(() => {
      expect(screen.getByText('src/main.rs')).toBeDefined();
    });
    // Click commit button to show commit form
    const commitBtn = screen.getAllByRole('button').find(b =>
      b.textContent?.toLowerCase().includes('commit')
    );
    expect(commitBtn).toBeTruthy();
    fireEvent.click(commitBtn!);
    // Should show checkboxes for file selection
    await waitFor(() => {
      const checkboxes = screen.getAllByRole('checkbox');
      expect(checkboxes.length).toBeGreaterThan(0);
    });
  });

  it('shows committed-on-branch section when committed_files present', async () => {
    gitStatusOverride = makeMockGitStatus({
      committed_files: [
        { path: 'committed-feature.rs', status: 'added', staged: true },
        { path: 'lib.rs', status: 'modified', staged: true },
      ],
    });
    render(<GitPanel projectId="p1" onClose={onClose} />);
    await waitFor(() => {
      expect(screen.getByTestId('git-committed-section')).toBeDefined();
      expect(screen.getByText('committed-feature.rs')).toBeDefined();
      expect(screen.getByText('lib.rs')).toBeDefined();
    });
  });

  it('hides committed section when committed_files is empty', async () => {
    gitStatusOverride = makeMockGitStatus({ committed_files: [] });
    render(<GitPanel projectId="p1" onClose={onClose} />);
    await waitFor(() => {
      expect(screen.getByText('feat/new-feature')).toBeDefined();
    });
    expect(screen.queryByTestId('git-committed-section')).toBeNull();
  });

  it('renders no commit history for an honest zero total', async () => {
    render(<GitPanel projectId="p1" onClose={onClose} />);
    await screen.findByText('feat/new-feature');
    expect(screen.queryByTestId('git-commit-history')).toBeNull();
  });

  it('renders a small history without a pagination CTA', async () => {
    gitStatusOverride = makeMockGitStatus({
      commits: [
        { sha: 'b'.repeat(40), short_sha: 'bbbbbbb', subject: 'second', author_name: 'Ada', author_time: 2 },
        { sha: 'a'.repeat(40), short_sha: 'aaaaaaa', subject: 'first', author_name: 'Ada', author_time: 1 },
      ],
      commits_total: 2,
    });
    const { container } = render(<GitPanel projectId="p1" onClose={onClose} />);
    await screen.findByText('second');
    expect(container.querySelectorAll('.git-commit-history-row')).toHaveLength(2);
    expect(screen.queryByText('git.loadMoreCommits')).toBeNull();
  });

  it('loads a 300+ history by bounded pages without losing file diff actions', async () => {
    const commits = (start: number, count: number) => Array.from({ length: count }, (_, index) => {
      const number = start - index;
      return {
        sha: number.toString(16).padStart(40, '0'),
        short_sha: number.toString(16).padStart(7, '0'),
        subject: `history ${number}`,
        author_name: 'Ada',
        author_time: number,
      };
    });
    const first = makeMockGitStatus({
      commits: commits(304, 40),
      commits_total: 305,
      commits_offset: 0,
      commits_truncated: true,
    });
    const second = makeMockGitStatus({
      commits: commits(264, 40),
      commits_total: 305,
      commits_offset: 40,
      commits_truncated: true,
    });
    vi.mocked(projectsApi.gitStatus)
      .mockResolvedValueOnce(first)
      .mockResolvedValueOnce(second);

    const { container } = render(<GitPanel projectId="p1" onClose={onClose} />);
    await screen.findByText('history 304');
    expect(container.querySelectorAll('.git-commit-history-row')).toHaveLength(40);
    expect(screen.getByText('git.commitsShown')).toBeDefined();

    fireEvent.click(screen.getByText('git.loadMoreCommits'));
    await waitFor(() => {
      expect(projectsApi.gitStatus).toHaveBeenLastCalledWith('p1', false, 40, 40);
      expect(container.querySelectorAll('.git-commit-history-row')).toHaveLength(80);
    });
    expect(screen.getByText('history 264')).toBeDefined();

    fireEvent.click(screen.getByText('src/main.rs'));
    await waitFor(() => {
      expect(projectsApi.gitDiff).toHaveBeenCalledWith('p1', 'src/main.rs', false);
    });
  });

  it('shows committed section even when uncommitted files list is empty', async () => {
    gitStatusOverride = makeMockGitStatus({
      files: [],
      committed_files: [{ path: 'only-committed.md', status: 'added', staged: true }],
    });
    render(<GitPanel projectId="p1" onClose={onClose} />);
    await waitFor(() => {
      expect(screen.getByTestId('git-committed-section')).toBeDefined();
      expect(screen.getByText('only-committed.md')).toBeDefined();
    });
    // git.noChanges (empty-state for uncommitted) should NOT appear when committed_files has items.
    expect(screen.queryByText('git.noChanges')).toBeNull();
  });

  it('does not embed the terminal inside the Git panel', async () => {
    render(<GitPanel projectId="p1" onClose={onClose} />);
    await waitFor(() => {
      expect(screen.getByText('feat/new-feature')).toBeDefined();
    });
    expect(screen.queryByText('git.terminal')).toBeNull();
  });
});
