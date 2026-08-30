import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { projects } from '../../lib/api';
import { SourceCodeViewer } from '../SourceCodeViewer';
import type { SourceFileNode } from '../../types/generated';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(next => { resolve = next; });
  return { promise, resolve };
}

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: Array<string | number>) =>
      args.length ? `${key} ${args.join(' ')}` : key,
  }),
}));

vi.mock('../../lib/api', () => ({
  projects: {
    listSourceFiles: vi.fn(),
    readSourceFile: vi.fn(),
    searchSourceFiles: vi.fn(),
    getSourceExclusions: vi.fn(),
    setSourceExclusions: vi.fn(),
    gitStatus: vi.fn(),
    gitBlame: vi.fn(),
    gitCommitDetail: vi.fn(),
  },
}));

describe('SourceCodeViewer', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(projects.listSourceFiles).mockResolvedValue([
      {
        path: 'src',
        name: 'src',
        is_dir: true,
        children: [
          { path: 'src/main.rs', name: 'main.rs', is_dir: false },
          { path: 'src/local.rules', name: 'local.rules', is_dir: false, git_ignored: true },
        ],
      },
    ]);
    vi.mocked(projects.readSourceFile).mockResolvedValue({
      path: 'src/main.rs',
      content: 'fn main() {\n    println!("hello");\n}',
    });
    vi.mocked(projects.searchSourceFiles).mockResolvedValue([
      { path: 'src/main.rs', match_count: 1 },
    ]);
    vi.mocked(projects.getSourceExclusions).mockResolvedValue([]);
    vi.mocked(projects.setSourceExclusions).mockImplementation(async (_id, paths) => paths);
    vi.mocked(projects.gitStatus).mockResolvedValue({
      branch: 'feature/source-browser',
      default_branch: 'main',
      is_default_branch: false,
      files: [],
      committed_files: [],
      commits: [],
      commits_total: 0,
      commits_offset: 0,
      commits_truncated: false,
      workspace: null,
      empty_reason: null,
      ahead: 0,
      behind: 0,
      has_upstream: true,
      upstream: 'origin/feature/source-browser',
      provider: 'github',
      remote_url: 'https://github.com/team/demo',
      pull_requests_url: 'https://github.com/team/demo/pulls',
      last_tag: null,
      pr_url: null,
      languages: [],
      languages_checked_at: null,
      languages_cached: false,
    });
    vi.mocked(projects.gitBlame).mockResolvedValue({
      path: 'src/main.rs',
      lines: [{
        line_number: 1,
        commit: '0123456789abcdef',
        author: 'Ada Lovelace',
        author_time: 1710000000,
      }],
    });
    vi.mocked(projects.gitCommitDetail).mockResolvedValue({
      sha: '0123456789abcdef0123456789abcdef01234567',
      short_sha: '0123456',
      author_name: 'Ada Lovelace',
      author_email: 'ada@example.com',
      author_time: 1710000000,
      committer_name: 'Ada Lovelace',
      commit_time: 1710000000,
      subject: 'Réécrit la boucle principale',
      body: 'Le corps du message\nsur deux lignes',
      branches: ['main', 'feature/source-browser'],
      branches_truncated: false,
      files_changed: 3,
    });
  });

  it('loads the source tree, highlights code and displays the current branch', async () => {
    const { container } = render(<SourceCodeViewer projectId="project-1" />);

    expect(await screen.findByText('main.rs')).toBeInTheDocument();
    expect(projects.listSourceFiles).toHaveBeenCalledWith('project-1', true);
    expect(projects.listSourceFiles).toHaveBeenCalledWith('project-1');
    expect(await screen.findByText('feature/source-browser')).toBeInTheDocument();
    await waitFor(() => expect(projects.readSourceFile).toHaveBeenCalledWith('project-1', 'src/main.rs'));
    expect(container.querySelector('.source-code .hljs-keyword')).toBeInTheDocument();
    expect(screen.getByText('ignored')).toHaveAttribute('title', 'Git ignored');
  });

  it('selects a deep-linked root configuration file instead of the default source', async () => {
    vi.mocked(projects.listSourceFiles).mockResolvedValue([
      { path: 'compose.yaml', name: 'compose.yaml', is_dir: false },
      {
        path: 'src',
        name: 'src',
        is_dir: true,
        children: [{ path: 'src/main.rs', name: 'main.rs', is_dir: false }],
      },
    ]);
    vi.mocked(projects.readSourceFile).mockImplementation(async (_id, path) => ({
      path,
      content: path === 'compose.yaml' ? 'services:\n  web:' : 'fn main() {}',
    }));

    render(<SourceCodeViewer projectId="project-1" initialPath="compose.yaml" />);

    await waitFor(() => {
      expect(projects.readSourceFile).toHaveBeenCalledWith('project-1', 'compose.yaml');
    });
    expect(screen.getAllByText('compose.yaml').length).toBeGreaterThan(0);
    expect(await screen.findByText(/services:/)).toBeInTheDocument();
  });

  it('renders repository-root entries before the complete tree finishes loading', async () => {
    let resolveFull!: (nodes: SourceFileNode[]) => void;
    const fullTree = new Promise<SourceFileNode[]>(resolve => { resolveFull = resolve; });
    vi.mocked(projects.listSourceFiles).mockImplementation(async (_id, shallow) => {
      if (shallow) {
        return [
          { path: 'src', name: 'src', is_dir: true, children: [] },
          { path: 'scripts', name: 'scripts', is_dir: true, children: [] },
          { path: 'README.md', name: 'README.md', is_dir: false },
        ];
      }
      return fullTree;
    });

    render(<SourceCodeViewer projectId="project-1" />);

    expect(await screen.findByText('src')).toBeInTheDocument();
    expect(screen.getAllByText('README.md')).not.toHaveLength(0);
    expect(screen.queryByText('main.rs')).not.toBeInTheDocument();
    expect(screen.getByLabelText('projects.source.loadingTreeBackground')).toBeInTheDocument();

    await act(async () => {
      resolveFull([{
        path: 'src',
        name: 'src',
        is_dir: true,
        children: [{ path: 'src/main.rs', name: 'main.rs', is_dir: false }],
      }]);
    });

    expect(await screen.findByText('main.rs')).toBeInTheDocument();
    expect(screen.getByText('scripts')).toBeInTheDocument();
    expect(screen.getAllByText('README.md')).not.toHaveLength(0);
    expect(screen.queryByLabelText('projects.source.loadingTreeBackground')).not.toBeInTheDocument();
  });

  it('recovers from a transient source-tree failure when Retry succeeds', async () => {
    vi.mocked(projects.listSourceFiles)
      .mockRejectedValueOnce(new Error('temporary failure'))
      .mockResolvedValue([{
        path: 'src',
        name: 'src',
        is_dir: true,
        children: [{ path: 'src/main.rs', name: 'main.rs', is_dir: false }],
      }]);

    render(<SourceCodeViewer projectId="project-1" />);
    expect(await screen.findByText('projects.source.error')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'projects.docAi.retry' }));

    expect(await screen.findByText('main.rs')).toBeInTheDocument();
    expect(screen.queryByText('projects.source.error')).not.toBeInTheDocument();
  });

  it('toggles Git annotations and displays author metadata per line', async () => {
    render(<SourceCodeViewer projectId="project-1" />);
    await screen.findByText('main.rs');

    fireEvent.click(screen.getByRole('button', { name: 'projects.source.annotate' }));

    await waitFor(() => {
      expect(projects.gitBlame).toHaveBeenCalledWith('project-1', 'src/main.rs');
    });
    expect(await screen.findByText(/Ada Lovelace/)).toBeInTheDocument();
  });

  it('searches across the project source through the Rust endpoint', async () => {
    render(<SourceCodeViewer projectId="project-1" />);
    const input = await screen.findByRole('textbox', { name: 'projects.source.search' });
    fireEvent.change(input, { target: { value: 'println' } });

    await waitFor(
      () => expect(projects.searchSourceFiles).toHaveBeenCalledWith('project-1', 'println'),
      { timeout: 1000 },
    );
    expect(await screen.findByText('1 / 1')).toBeInTheDocument();
    expect(screen.getByText('projects.source.filesCount 1')).toBeInTheDocument();
  });

  it('ignores an older search response that resolves after the active query', async () => {
    const older = deferred<Array<{ path: string; match_count: number }>>();
    const newer = deferred<Array<{ path: string; match_count: number }>>();
    vi.mocked(projects.searchSourceFiles)
      .mockImplementationOnce(() => older.promise)
      .mockImplementationOnce(() => newer.promise);

    render(<SourceCodeViewer projectId="project-1" />);
    const input = await screen.findByRole('textbox', { name: 'projects.source.search' });

    fireEvent.change(input, { target: { value: 'older' } });
    await waitFor(() => {
      expect(projects.searchSourceFiles).toHaveBeenCalledWith('project-1', 'older');
    });
    fireEvent.change(input, { target: { value: 'newer' } });
    await waitFor(() => {
      expect(projects.searchSourceFiles).toHaveBeenCalledWith('project-1', 'newer');
    });

    await act(async () => {
      newer.resolve([{ path: 'src/main.rs', match_count: 2 }]);
      await newer.promise;
    });
    expect(await screen.findByText('1 / 2')).toBeInTheDocument();

    await act(async () => {
      older.resolve([{ path: 'README.md', match_count: 1 }]);
      await older.promise;
    });
    expect(screen.getByText('1 / 2')).toBeInTheDocument();
    expect(vi.mocked(projects.readSourceFile).mock.calls.at(-1))
      .toEqual(['project-1', 'src/main.rs']);
  });

  it('navigates to the next highlighted occurrence', async () => {
    vi.mocked(projects.readSourceFile).mockResolvedValue({
      path: 'src/main.rs',
      content: 'println!("one");\nprintln!("two");',
    });
    vi.mocked(projects.searchSourceFiles).mockResolvedValue([
      { path: 'src/main.rs', match_count: 2 },
    ]);
    const { container } = render(<SourceCodeViewer projectId="project-1" />);
    const input = await screen.findByRole('textbox', { name: 'projects.source.search' });
    fireEvent.change(input, { target: { value: 'println' } });

    const next = await screen.findByTitle('Enter');
    await waitFor(() => {
      expect(container.querySelector('mark[data-source-hl="0"]')).toHaveAttribute('data-active', 'true');
    });
    fireEvent.click(next);
    await waitFor(() => {
      expect(container.querySelector('mark[data-source-hl="1"]')).toHaveAttribute('data-active', 'true');
    });
  });

  it('excludes a folder from the project browser and can restore it', async () => {
    vi.mocked(projects.getSourceExclusions)
      .mockResolvedValueOnce([])
      .mockResolvedValue(['src']);
    render(<SourceCodeViewer projectId="project-1" />);
    await screen.findByText('main.rs');

    fireEvent.click(screen.getByRole('button', {
      name: 'projects.source.excludeFolder src',
    }));

    await waitFor(() => {
      expect(projects.setSourceExclusions).toHaveBeenCalledWith('project-1', ['src']);
    });
    expect(await screen.findByText('src', { selector: '.source-exclusions button span' }))
      .toBeInTheDocument();

    fireEvent.click(screen.getByTitle('projects.source.restoreFolder src'));
    await waitFor(() => {
      expect(projects.setSourceExclusions).toHaveBeenLastCalledWith('project-1', []);
    });
  });
  // ─── KT-67 — from an annotated line to its commit ────────────────────────

  it('opens the commit behind an annotated line and shows its story', async () => {
    render(<SourceCodeViewer projectId="project-1" />);
    await screen.findByText('main.rs');
    fireEvent.click(screen.getByRole('button', { name: 'projects.source.annotate' }));
    await screen.findByText(/Ada Lovelace/);

    fireEvent.click(screen.getByTestId('source-blame-button'));

    await waitFor(() => {
      // The sha comes from blame, not from anything typed by the user.
      expect(projects.gitCommitDetail).toHaveBeenCalledWith('project-1', '0123456789abcdef');
    });
    const panel = await screen.findByTestId('source-commit-detail');
    expect(panel).toHaveTextContent('Réécrit la boucle principale');
    expect(panel).toHaveTextContent('ada@example.com');
    expect(panel).toHaveTextContent('main, feature/source-browser');
    // Full hash available for copy/paste, not just the short one.
    expect(panel).toHaveTextContent('0123456789abcdef0123456789abcdef01234567');
  });

  it('says so when the branch list was truncated', async () => {
    vi.mocked(projects.gitCommitDetail).mockResolvedValue({
      sha: 'abc1234abc1234abc1234abc1234abc1234abc12',
      short_sha: 'abc1234',
      author_name: 'Ada',
      author_email: 'ada@example.com',
      author_time: 1710000000,
      committer_name: 'Ada',
      commit_time: 1710000000,
      subject: 'sujet',
      body: '',
      branches: ['main'],
      branches_truncated: true,
      files_changed: 1,
    });
    render(<SourceCodeViewer projectId="project-1" />);
    await screen.findByText('main.rs');
    fireEvent.click(screen.getByRole('button', { name: 'projects.source.annotate' }));
    await screen.findByText(/Ada/);
    fireEvent.click(screen.getByTestId('source-blame-button'));

    const panel = await screen.findByTestId('source-commit-detail');
    // A capped list must never look complete.
    expect(panel).toHaveTextContent('projects.source.commitBranchesMore');
  });

  // ─── KT-75 — from the commit's story to the change itself ────────────────

  it('hands the full sha to the host so the patch opens in its own tab', async () => {
    const onOpenCommit = vi.fn();
    render(<SourceCodeViewer projectId="project-1" onOpenCommit={onOpenCommit} />);
    await screen.findByText('main.rs');
    fireEvent.click(screen.getByRole('button', { name: 'projects.source.annotate' }));
    await screen.findByText(/Ada Lovelace/);
    fireEvent.click(screen.getByTestId('source-blame-button'));

    fireEvent.click(await screen.findByTestId('source-commit-open-patch'));
    // The FULL sha, not the abbreviated one blame reported.
    expect(onOpenCommit).toHaveBeenCalledWith('0123456789abcdef0123456789abcdef01234567');
  });

  it('hides the patch action when the host cannot open a tab', async () => {
    render(<SourceCodeViewer projectId="project-1" />);
    await screen.findByText('main.rs');
    fireEvent.click(screen.getByRole('button', { name: 'projects.source.annotate' }));
    await screen.findByText(/Ada Lovelace/);
    fireEvent.click(screen.getByTestId('source-blame-button'));

    await screen.findByTestId('source-commit-detail');
    expect(screen.queryByTestId('source-commit-open-patch')).toBeNull();
  });

  it('reports a failed lookup instead of an empty panel, and closes on Escape', async () => {
    vi.mocked(projects.gitCommitDetail).mockRejectedValue(new Error('bad object'));
    render(<SourceCodeViewer projectId="project-1" />);
    await screen.findByText('main.rs');
    fireEvent.click(screen.getByRole('button', { name: 'projects.source.annotate' }));
    await screen.findByText(/Ada Lovelace/);
    fireEvent.click(screen.getByTestId('source-blame-button'));

    const panel = await screen.findByTestId('source-commit-detail');
    await waitFor(() => expect(panel).toHaveTextContent('bad object'));

    await act(async () => { fireEvent.keyDown(window, { key: 'Escape' }); });
    expect(screen.queryByTestId('source-commit-detail')).toBeNull();
  });
});
