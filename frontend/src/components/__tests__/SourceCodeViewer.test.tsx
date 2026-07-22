import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { projects } from '../../lib/api';
import { SourceCodeViewer } from '../SourceCodeViewer';
import type { SourceFileNode } from '../../types/generated';

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

  it('renders repository-root entries before the complete tree finishes loading', async () => {
    let resolveFull!: (nodes: SourceFileNode[]) => void;
    const fullTree = new Promise<SourceFileNode[]>(resolve => { resolveFull = resolve; });
    vi.mocked(projects.listSourceFiles).mockImplementation(async (_id, shallow) => {
      if (shallow) {
        return [
          { path: 'src', name: 'src', is_dir: true, children: [] },
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
});
