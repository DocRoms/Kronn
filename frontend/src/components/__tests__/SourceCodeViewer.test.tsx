import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { projects } from '../../lib/api';
import { SourceCodeViewer } from '../SourceCodeViewer';
import { buildHtmlPreviewDocument } from '../../lib/html-preview';
import type { SourceDirectoryListing, SourceFileNode } from '../../types/generated';

// KT-605 — the endpoint answers with the directory AND whether it is all of
// it, so the bound it keeps stops being silent. Tests say what they mean and
// the wrapper carries the rest.
function listing(entries: SourceFileNode[], truncated = false): SourceDirectoryListing {
  return { entries, truncated };
}

/** Say what each directory holds; the wrapper carries the rest of the shape. */
function mockDirectories(impl: (path?: string) => SourceFileNode[] | Promise<SourceFileNode[]>) {
  vi.mocked(projects.listSourceFiles).mockImplementation(async (_id, _shallow, path) => (
    listing(await impl(path))
  ));
}

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
    // KT-605 — the endpoint answers for ONE directory. The root lists `src`
    // without its contents; `src` is fetched because it opens on arrival.
    mockDirectories(async path => {
      if (!path) return [{ path: 'src', name: 'src', is_dir: true, children: [] }];
      if (path === 'src') {
        return [
          { path: 'src/main.rs', name: 'main.rs', is_dir: false },
          { path: 'src/local.rules', name: 'local.rules', is_dir: false, git_ignored: true },
        ];
      }
      return [];
    });
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
    // And the folder that opens on arrival, by name — never the whole tree.
    expect(projects.listSourceFiles).toHaveBeenCalledWith('project-1', true, 'src');
    expect(await screen.findByText('feature/source-browser')).toBeInTheDocument();
    await waitFor(() => expect(projects.readSourceFile).toHaveBeenCalledWith('project-1', 'src/main.rs'));
    expect(container.querySelector('.source-code .hljs-keyword')).toBeInTheDocument();
    expect(screen.getByText('ignored')).toHaveAttribute('title', 'Git ignored');
  });

  it('selects a deep-linked root configuration file instead of the default source', async () => {
    mockDirectories(() => [
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

  it('renders an uppercase HTML file in an isolated preview and restores its source', async () => {
    mockDirectories(() => [
      { path: 'site/INDEX.HTM', name: 'INDEX.HTM', is_dir: false },
    ]);
    vi.mocked(projects.readSourceFile).mockResolvedValue({
      path: 'site/INDEX.HTM',
      content: '<!doctype html><html><head><style>p { color: red; }</style></head><body><p>Hello</p></body></html>',
    });

    render(<SourceCodeViewer projectId="project-1" initialPath="site/INDEX.HTM" />);

    expect(await screen.findByRole('button', { name: 'projects.source.preview' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'projects.source.code' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'projects.source.preview' })).toHaveAttribute('aria-pressed', 'false');
    expect(await screen.findByText(/Hello/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'projects.source.preview' }));
    // KT-605 — the tree loads a folder at a time now, so the frame arrives on
    // its own schedule rather than in the same tick as the click.
    const frame = await screen.findByTestId('source-html-preview-frame');
    expect(frame).toHaveAttribute('sandbox', '');
    expect(frame).toHaveAttribute('srcdoc', expect.stringContaining("default-src 'none'"));
    expect(frame).toHaveAttribute('srcdoc', expect.stringContaining('color: red'));
    expect(screen.getByRole('button', { name: 'projects.source.code' })).toHaveAttribute('aria-pressed', 'false');
    expect(screen.getByRole('button', { name: 'projects.source.preview' })).toHaveAttribute('aria-pressed', 'true');

    fireEvent.click(screen.getByRole('button', { name: 'projects.source.code' }));
    expect(screen.queryByTestId('source-html-preview-frame')).not.toBeInTheDocument();
    expect(screen.getByText(/Hello/)).toBeInTheDocument();
  });

  it('parses malformed markup, removes navigation and retains data images in the static preview', () => {
    const preview = buildHtmlPreviewDocument(`<!-- <head> --><html><head>
      <meta http-equiv="refresh" content="0; url=https://preview-probe.invalid/refresh"><base href="https://preview-probe.invalid/">
    </head><body><img SRC="data:image/png;base64,AAAA"><a/href="https://preview-probe.invalid/slash-navigation">Open</a>
      <form action="https://preview-probe.invalid/form"><button formaction="https://preview-probe.invalid/button">Submit</button></form>
      <div><template shadowrootmode="open"><a href="https://preview-probe.invalid/template">Template link</a></template></div>
      <img src="https://preview-probe.invalid/comment-head"></body></html>`);

    expect(preview).toMatch(/^<!doctype html><html><head><meta http-equiv="Content-Security-Policy"/i);
    expect(preview).toContain("default-src 'none'");
    expect(preview).not.toContain('https://preview-probe.invalid');
    expect(preview).toContain('src="data:image/png;base64,AAAA"');
    expect(preview).toContain('Open');
    expect(preview).not.toContain('Template link');
  });

  it('returns to code and displays a loading error when a non-HTML file is selected', async () => {
    mockDirectories(() => [
      { path: 'index.html', name: 'index.html', is_dir: false },
      { path: 'notes.txt', name: 'notes.txt', is_dir: false },
    ]);
    vi.mocked(projects.readSourceFile).mockImplementation(async (_id, path) => {
      if (path === 'notes.txt') throw new Error('unreadable');
      return { path, content: '<p>Preview source</p>' };
    });

    render(<SourceCodeViewer projectId="project-1" initialPath="index.html" />);
    fireEvent.click(await screen.findByRole('button', { name: 'projects.source.preview' }));
    expect(await screen.findByTestId('source-html-preview-frame')).toBeInTheDocument();

    fireEvent.click(screen.getByText('notes.txt'));
    expect(screen.queryByRole('button', { name: 'projects.source.preview' })).not.toBeInTheDocument();
    expect(await screen.findByText('projects.source.fileError')).toBeInTheDocument();
    expect(screen.queryByTestId('source-html-preview-frame')).not.toBeInTheDocument();
  });

  /// KT-605 — the root arrives on its own, and a folder costs a request only
  /// when someone opens it. Before, the panel fetched the WHOLE tree behind
  /// this listing, bounded at 10 000 files, and went silent past that bound.
  it('shows the root at once and fetches a folder only when it is opened', async () => {
    let resolveScripts!: (nodes: SourceFileNode[]) => void;
    mockDirectories(async path => {
      if (!path) {
        return [
          { path: 'scripts', name: 'scripts', is_dir: true, children: [] },
          { path: 'README.md', name: 'README.md', is_dir: false },
        ];
      }
      if (path === 'scripts') {
        return new Promise<SourceFileNode[]>(resolve => { resolveScripts = resolve; });
      }
      return [];
    });

    render(<SourceCodeViewer projectId="project-1" />);

    expect(await screen.findByText('scripts')).toBeInTheDocument();
    expect(screen.getAllByText('README.md')).not.toHaveLength(0);
    // `scripts` is not one of the folders that open on arrival, so nothing has
    // been asked about it yet.
    expect(projects.listSourceFiles).not.toHaveBeenCalledWith('project-1', true, 'scripts');

    fireEvent.click(screen.getByText('scripts'));
    await waitFor(() => {
      expect(projects.listSourceFiles).toHaveBeenCalledWith('project-1', true, 'scripts');
    });

    await act(async () => {
      resolveScripts([{ path: 'scripts/build.sh', name: 'build.sh', is_dir: false }]);
    });
    expect(await screen.findByText('build.sh')).toBeInTheDocument();

    // Closed and reopened, it is not fetched again: it is already on screen.
    const calls = vi.mocked(projects.listSourceFiles).mock.calls.length;
    fireEvent.click(screen.getByText('scripts'));
    fireEvent.click(screen.getByText('scripts'));
    expect(vi.mocked(projects.listSourceFiles).mock.calls).toHaveLength(calls);
  });

  /// A deep link names a file several folders down. Its ancestors have to be
  /// fetched, in order, or the tree has nowhere to put it.
  it('opens the folders a deep link passes through', async () => {
    mockDirectories(async path => {
      if (!path) return [{ path: 'site', name: 'site', is_dir: true, children: [] }];
      if (path === 'site') return [{ path: 'site/assets', name: 'assets', is_dir: true, children: [] }];
      if (path === 'site/assets') return [{ path: 'site/assets/logo.svg', name: 'logo.svg', is_dir: false }];
      return [];
    });
    vi.mocked(projects.readSourceFile).mockResolvedValue({
      path: 'site/assets/logo.svg',
      content: '<svg />',
    });

    render(<SourceCodeViewer projectId="project-1" initialPath="site/assets/logo.svg" />);

    expect(await screen.findByText('logo.svg')).toBeInTheDocument();
    expect(projects.listSourceFiles).toHaveBeenCalledWith('project-1', true, 'site');
    expect(projects.listSourceFiles).toHaveBeenCalledWith('project-1', true, 'site/assets');
    await waitFor(() => {
      expect(projects.readSourceFile).toHaveBeenCalledWith('project-1', 'site/assets/logo.svg');
    });
  });

  /// KT-594 — the reported bug. The root listing gives every folder empty
  /// children, so an unfinished folder used to open onto nothing at all: the
  /// interface said "empty" when it meant "not yet". Romuald clicked `site`,
  /// saw nothing, and watched its contents appear four seconds later.
  /// KT-605, review @codex-cli-4 — a deep link into a folder that opens on
  /// arrival. `src` is already being fetched when the chain reaches it; the
  /// chain must WAIT for that request, not sail past it on a resolved promise.
  /// Before the fix `src/nested` answered first, found no parent to attach to,
  /// and its contents were dropped without a trace.
  it('waits for an already loading default folder before fetching its descendants', async () => {
    let resolveSrc!: (nodes: SourceFileNode[]) => void;
    const asked: string[] = [];
    mockDirectories(async path => {
      if (!path) return [{ path: 'src', name: 'src', is_dir: true, children: [] }];
      asked.push(path);
      if (path === 'src') return new Promise<SourceFileNode[]>(r => { resolveSrc = r; });
      if (path === 'src/nested') {
        return [{ path: 'src/nested/deep.ts', name: 'deep.ts', is_dir: false }];
      }
      return [];
    });
    vi.mocked(projects.readSourceFile).mockResolvedValue({
      path: 'src/nested/deep.ts',
      content: 'export {};',
    });

    render(<SourceCodeViewer projectId="project-1" initialPath="src/nested/deep.ts" />);
    await screen.findByText('src');

    // `src` is suspended, so nothing below it may have been asked for yet.
    await waitFor(() => expect(asked).toContain('src'));
    expect(asked).not.toContain('src/nested');

    await act(async () => {
      resolveSrc([{ path: 'src/nested', name: 'nested', is_dir: true, children: [] }]);
    });

    // Only now, and the file it pointed at is really in the tree.
    await waitFor(() => expect(asked).toContain('src/nested'));
    expect(await screen.findByText('deep.ts')).toBeInTheDocument();
  });

  /// KT-605, review @codex-cli-4 — removing the ceiling from the TREE left it
  /// on one ANSWER: a folder with more direct entries than the bound still
  /// stopped, and stopped in silence. That is the failure Romuald spent a
  /// morning on, in a rarer shape. A limit is defensible; hiding it is not.
  it('says a folder is showing less than it holds', async () => {
    vi.mocked(projects.listSourceFiles).mockImplementation(async (_id, _shallow, path) => {
      if (!path) return listing([{ path: 'huge', name: 'huge', is_dir: true, children: [] }]);
      if (path === 'huge') {
        return listing([{ path: 'huge/a.rs', name: 'a.rs', is_dir: false }], true);
      }
      return listing([]);
    });

    render(<SourceCodeViewer projectId="project-1" />);
    fireEvent.click(await screen.findByText('huge'));

    // The entries it did return are there…
    expect(await screen.findByText('a.rs')).toBeInTheDocument();
    // …and so is the fact that they are not all of them.
    const cut = await screen.findByTestId('source-tree-truncated-huge');
    expect(cut).toHaveTextContent('projects.source.folderTruncated');
  });

  /// The repository root is a directory like any other, and can reach the
  /// bound too. Looking complete would be the same lie one level up.
  it('says the root itself is showing less than it holds', async () => {
    vi.mocked(projects.listSourceFiles).mockImplementation(async (_id, _shallow, path) => {
      if (!path) return listing([{ path: 'a.rs', name: 'a.rs', is_dir: false }], true);
      return listing([]);
    });

    render(<SourceCodeViewer projectId="project-1" />);
    expect(await screen.findByTestId('source-tree-truncated-root'))
      .toHaveTextContent('projects.source.folderTruncated');
  });

  it('says a folder is still loading instead of rendering it as empty', async () => {
    let resolveSite!: (nodes: SourceFileNode[]) => void;
    mockDirectories(async path => {
      if (!path) return [{ path: 'site', name: 'site', is_dir: true, children: [] }];
      if (path === 'site') return new Promise<SourceFileNode[]>(r => { resolveSite = r; });
      return [];
    });

    render(<SourceCodeViewer projectId="project-1" />);
    fireEvent.click(await screen.findByText('site'));

    const pending = await screen.findByTestId('source-tree-pending-site');
    expect(pending).toHaveTextContent('projects.source.loadingFolder');
    expect(pending).not.toHaveAttribute('data-failed');

    await act(async () => {
      resolveSite([{ path: 'site/index.html', name: 'index.html', is_dir: false }]);
    });

    // And once the children are there, the row makes way for them.
    expect(await screen.findByText('index.html')).toBeInTheDocument();
    expect(screen.queryByTestId('source-tree-pending-site')).not.toBeInTheDocument();
  });

  /// A failed enrichment leaves the root listing usable on purpose, but its
  /// folders stay empty forever. Saying nothing there is the same lie as
  /// saying nothing during the wait — with no second request coming to undo it.
  it('says a folder is unreachable when its own request fails, and lets it be retried', async () => {
    let attempts = 0;
    mockDirectories(async path => {
      if (!path) return [{ path: 'site', name: 'site', is_dir: true, children: [] }];
      if (path === 'site') {
        attempts += 1;
        if (attempts === 1) throw new Error('folder unreachable');
        return [{ path: 'site/index.html', name: 'index.html', is_dir: false }];
      }
      return [];
    });

    render(<SourceCodeViewer projectId="project-1" />);
    fireEvent.click(await screen.findByText('site'));

    const pending = await screen.findByTestId('source-tree-pending-site');
    await waitFor(() => expect(pending).toHaveAttribute('data-failed'));
    expect(pending).toHaveTextContent('projects.source.folderUnavailable');
    // One folder failing is not a failed page: the rest of the tree stands.
    expect(screen.getByText('site')).toBeInTheDocument();
    expect(screen.queryByText('projects.source.error')).not.toBeInTheDocument();

    // KT-605 — closing and reopening is the gesture a reader will try, so it
    // has to be the one that retries.
    fireEvent.click(screen.getByText('site'));
    fireEvent.click(screen.getByText('site'));
    expect(await screen.findByText('index.html')).toBeInTheDocument();
  });

  it('recovers from a transient source-tree failure when Retry succeeds', async () => {
    vi.mocked(projects.listSourceFiles)
      .mockRejectedValueOnce(new Error('temporary failure'))
      .mockResolvedValue(listing([{
        path: 'src',
        name: 'src',
        is_dir: true,
        children: [{ path: 'src/main.rs', name: 'main.rs', is_dir: false }],
      }]));

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
