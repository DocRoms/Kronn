import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { GitPanel } from '../GitPanel';

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
    ahead: 2,
    behind: 0,
    has_upstream: true,
    provider: 'github',
    pr_url: null,
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

  it('does not show terminal by default', async () => {
    render(<GitPanel projectId="p1" onClose={onClose} />);
    await waitFor(() => {
      expect(screen.getByText('feat/new-feature')).toBeDefined();
    });
    // Terminal input should not be visible initially
    expect(screen.queryByPlaceholderText(/command|terminal/i)).toBeNull();
  });
});
