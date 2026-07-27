import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { projects } from '../../lib/api';
import type { GitStatusResponse } from '../../types/generated';
import { ProjectCodePanel } from '../ProjectCodePanel';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

// The stub exposes the KT-75 callback so the panel's tab can be driven without
// mounting the real blame gutter.
vi.mock('../SourceCodeViewer', () => ({
  SourceCodeViewer: ({ projectId, onOpenCommit }: {
    projectId: string;
    onOpenCommit?: (sha: string) => void;
  }) => (
    <div data-testid="source-viewer">
      {projectId}
      <button type="button" data-testid="stub-open-a" onClick={() => onOpenCommit?.('aaaaaaa1')}>a</button>
      <button type="button" data-testid="stub-open-b" onClick={() => onOpenCommit?.('bbbbbbb2')}>b</button>
    </div>
  ),
}));

vi.mock('../../lib/api', () => ({
  projects: {
    gitStatus: vi.fn(),
    gitDiff: vi.fn(),
    gitCommitPatch: vi.fn(),
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

  // KT-75 — the commit tab.
  describe('temporary commit tab', () => {
    const patch = (sha: string, extra: Record<string, unknown> = {}) => ({
      sha,
      short_sha: sha.slice(0, 7),
      subject: `sujet de ${sha}`,
      patch: `diff --git a/src/x.ts b/src/x.ts\n@@ -1 +1 @@\n+touched by ${sha}`,
      truncated: false,
      files_changed: 1,
      is_root: false,
      ...extra,
    });

    // KT-87 — a two-file commit, to check only the selected one is rendered.
    const twoFilePatch = [
      'diff --git a/src/first.ts b/src/first.ts',
      '@@ -1 +1 @@',
      '+first file content',
      'diff --git a/docs/second.md b/docs/second.md',
      '@@ -1 +1 @@',
      '+second file content',
    ].join('\n');

    it('opens the patch in a closable tab and renders the real hunks', async () => {
      vi.mocked(projects.gitCommitPatch).mockResolvedValue(patch('aaaaaaa1'));
      render(<ProjectCodePanel projectId="project-1" />);
      expect(screen.queryByTestId('project-code-commit-tab')).toBeNull();

      fireEvent.click(screen.getByTestId('stub-open-a'));

      await waitFor(() => {
        expect(projects.gitCommitPatch).toHaveBeenCalledWith('project-1', 'aaaaaaa1');
      });
      expect(await screen.findByTestId('project-code-commit-tab')).toBeInTheDocument();
      expect(await screen.findByText(/touched by aaaaaaa1/)).toBeInTheDocument();
      expect(screen.getByTestId('project-code-commit-view')).toHaveTextContent('sujet de aaaaaaa1');

      fireEvent.click(screen.getByTestId('project-code-commit-close'));
      expect(screen.queryByTestId('project-code-commit-tab')).toBeNull();
      expect(screen.queryByTestId('project-code-commit-view')).toBeNull();
      // Closing lands back on the sources, not on a blank panel.
      expect(screen.getByTestId('source-viewer')).toBeInTheDocument();
    });

    it('replaces the tab instead of stacking one per commit', async () => {
      vi.mocked(projects.gitCommitPatch).mockImplementation(async (_id, sha) => patch(sha));
      render(<ProjectCodePanel projectId="project-1" />);

      fireEvent.click(screen.getByTestId('stub-open-a'));
      expect(await screen.findByText(/touched by aaaaaaa1/)).toBeInTheDocument();

      // Back to sources so the stub is mounted again, then open another commit.
      fireEvent.click(screen.getByRole('button', { name: /projects.code.source/ }));
      fireEvent.click(screen.getByTestId('stub-open-b'));

      expect(await screen.findByText(/touched by bbbbbbb2/)).toBeInTheDocument();
      expect(screen.queryByText(/touched by aaaaaaa1/)).toBeNull();
      expect(screen.getAllByTestId('project-code-commit-tab')).toHaveLength(1);
    });

    // Codex's review: my replacement test above awaits between clicks, so it
    // never exercised the interleaving. Two requests in flight, the FIRST
    // answering last, must not paint commit A under tab B.
    it('ignores a slow first response once another commit was opened', async () => {
      const resolvers: Array<() => void> = [];
      vi.mocked(projects.gitCommitPatch).mockImplementation(
        (_id, sha) => new Promise(resolve => {
          resolvers.push(() => resolve(patch(sha)));
        }),
      );
      render(<ProjectCodePanel projectId="project-1" />);

      fireEvent.click(screen.getByTestId('stub-open-a'));
      // Still on the commit tab, so reach B through the source tab.
      fireEvent.click(screen.getByRole('button', { name: /projects.code.source/ }));
      fireEvent.click(screen.getByTestId('stub-open-b'));
      await waitFor(() => expect(resolvers).toHaveLength(2));

      // B answers, then the stale A.
      await act(async () => { resolvers[1](); });
      expect(await screen.findByText(/touched by bbbbbbb2/)).toBeInTheDocument();
      await act(async () => { resolvers[0](); });

      expect(screen.queryByText(/touched by aaaaaaa1/)).toBeNull();
      expect(screen.getByText(/touched by bbbbbbb2/)).toBeInTheDocument();
    });

    // Same guard, exercised across a close: the abandoned request must not
    // resurface in the tab the user opened afterwards.
    it('does not let a request abandoned by closing land in the next tab', async () => {
      const resolvers: Array<() => void> = [];
      vi.mocked(projects.gitCommitPatch).mockImplementation(
        (_id, sha) => new Promise(resolve => { resolvers.push(() => resolve(patch(sha))); }),
      );
      render(<ProjectCodePanel projectId="project-1" />);

      fireEvent.click(screen.getByTestId('stub-open-a'));
      await waitFor(() => expect(resolvers).toHaveLength(1));
      fireEvent.click(screen.getByTestId('project-code-commit-close'));
      expect(screen.queryByTestId('project-code-commit-tab')).toBeNull();

      fireEvent.click(screen.getByTestId('stub-open-b'));
      await waitFor(() => expect(resolvers).toHaveLength(2));
      // B answers, and only then the request abandoned by the close.
      await act(async () => { resolvers[1](); });
      await act(async () => { resolvers[0](); });

      expect(await screen.findByText(/touched by bbbbbbb2/)).toBeInTheDocument();
      expect(screen.queryByText(/touched by aaaaaaa1/)).toBeNull();
    });

    it('lists the patch files and renders only the selected one', async () => {
      vi.mocked(projects.gitCommitPatch).mockResolvedValue(
        patch('aaaaaaa1', { patch: twoFilePatch, files_changed: 2 }),
      );
      render(<ProjectCodePanel projectId="project-1" />);
      fireEvent.click(screen.getByTestId('stub-open-a'));

      const list = await screen.findByTestId('project-code-commit-files');
      expect(list.querySelectorAll('button')).toHaveLength(2);

      // First file selected by default — the pane is never blank.
      expect(await screen.findByText(/first file content/)).toBeInTheDocument();
      expect(screen.queryByText(/second file content/)).toBeNull();

      fireEvent.click(screen.getByTitle('docs/second.md'));
      expect(await screen.findByText(/second file content/)).toBeInTheDocument();
      expect(
        screen.queryByText(/first file content/),
        'the previous file must be unmounted, not just scrolled past',
      ).toBeNull();
    });

    it('keeps a 200-file list and its diff in independently bounded panes', async () => {
      const largePatch = Array.from({ length: 200 }, (_, index) => [
        `diff --git a/src/file-${index}.ts b/src/file-${index}.ts`,
        '@@ -1 +1 @@',
        `+content for file ${index}`,
      ].join('\n')).join('\n');
      vi.mocked(projects.gitCommitPatch).mockResolvedValue(
        patch('aaaaaaa1', { patch: largePatch, files_changed: 200 }),
      );
      render(<ProjectCodePanel projectId="project-1" />);
      fireEvent.click(screen.getByTestId('stub-open-a'));

      const list = await screen.findByTestId('project-code-commit-files');
      expect(list.querySelectorAll('button')).toHaveLength(200);
      fireEvent.click(screen.getByTitle('src/file-199.ts'));
      await waitFor(() => {
        expect(screen.getByTestId('project-code-commit-view')).toHaveTextContent('content for file 199');
        expect(screen.getByTestId('project-code-commit-view')).not.toHaveTextContent('content for file 0');
      });

      // happy-dom does not calculate scrollHeight, so pin the CSS contract that
      // makes the two real-browser scrollports independent: a bounded grid row,
      // global overflow clipped, then one overflow-y owner per pane.
      const css = readFileSync('src/components/ProjectCodePanel.css', 'utf8');
      const layoutRule = css.match(/\.project-code-commit-layout\s*\{([^}]*)\}/)?.[1];
      const filesRule = css.match(/\.project-code-commit-files\s*\{([^}]*)\}/)?.[1];
      expect(layoutRule).toContain('height: min(620px, calc(100dvh - 340px));');
      expect(layoutRule).toContain('grid-template-rows: minmax(0, 1fr);');
      expect(layoutRule).toContain('overflow: hidden;');
      expect(filesRule).toContain('overflow-y: auto;');
    });

    it('resets the selected file when another commit is opened', async () => {
      vi.mocked(projects.gitCommitPatch).mockImplementation(async (_id, sha) => (
        sha === 'aaaaaaa1'
          ? patch(sha, { patch: twoFilePatch, files_changed: 2 })
          : patch(sha)
      ));
      render(<ProjectCodePanel projectId="project-1" />);
      fireEvent.click(screen.getByTestId('stub-open-a'));
      fireEvent.click(await screen.findByTitle('docs/second.md'));
      expect(await screen.findByText(/second file content/)).toBeInTheDocument();

      fireEvent.click(screen.getByRole('button', { name: /projects.code.source/ }));
      fireEvent.click(screen.getByTestId('stub-open-b'));

      // A stale selection must not leave the new commit's pane empty.
      expect(await screen.findByText(/touched by bbbbbbb2/)).toBeInTheDocument();
    });

    it('says when the patch was cut and when the commit is a root commit', async () => {
      vi.mocked(projects.gitCommitPatch).mockResolvedValue(
        patch('aaaaaaa1', { truncated: true, is_root: true }),
      );
      render(<ProjectCodePanel projectId="project-1" />);
      fireEvent.click(screen.getByTestId('stub-open-a'));

      const view = await screen.findByTestId('project-code-commit-view');
      await waitFor(() => {
        expect(view).toHaveTextContent('projects.code.commitTruncated');
        expect(view).toHaveTextContent('projects.code.commitRoot');
      });
    });

    it('reports an unreadable commit instead of an empty diff', async () => {
      vi.mocked(projects.gitCommitPatch).mockRejectedValueOnce(new Error('git show failed'));
      render(<ProjectCodePanel projectId="project-1" />);
      fireEvent.click(screen.getByTestId('stub-open-a'));

      expect(await screen.findByText('projects.code.commitLoadError')).toBeInTheDocument();
    });

    it('distinguishes an empty commit from a failure', async () => {
      vi.mocked(projects.gitCommitPatch).mockResolvedValue(patch('aaaaaaa1', { patch: '', files_changed: 0 }));
      render(<ProjectCodePanel projectId="project-1" />);
      fireEvent.click(screen.getByTestId('stub-open-a'));

      expect(await screen.findByText('projects.code.commitEmpty')).toBeInTheDocument();
      expect(screen.queryByText('projects.code.commitLoadError')).toBeNull();
    });
  });

  it('shows a bounded error state when Git status cannot be read', async () => {
    vi.mocked(projects.gitStatus).mockRejectedValueOnce(new Error('not a repository'));
    render(<ProjectCodePanel projectId="project-1" />);

    fireEvent.click(screen.getByRole('button', { name: /projects.code.changes/ }));

    expect(await screen.findByText('projects.code.loadError')).toBeInTheDocument();
  });
});
