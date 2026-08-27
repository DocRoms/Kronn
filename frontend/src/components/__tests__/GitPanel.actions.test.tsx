/**
 * GitPanel — action-handler coverage.
 *
 * The sibling GitPanel.test.tsx pins render/empty/committed-section states.
 * This file targets the UNCOVERED imperative handlers + their catch branches,
 * which is where Functions coverage was being lost:
 *
 *  - handleCommit (projects.* AND discussions.* path, correct files+message)
 *  - handlePush (success + rejected → error text)
 *  - handleCreateBranch (success + rejected)
 *  - openPrForm + handleCreatePr (template fetch, auto-push when no upstream)
 *  - openDiff on file-button click (success + rejected → "Error:" content)
 *  - toggleFile / selectAll selection logic
 *  - loading spinners / disabled states
 *  - on-default-branch warning + create-branch shortcut
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react';
import { readFileSync } from 'node:fs';

// ─── Mock API (vi.hoisted, DebugSection gold-standard pattern) ────────────────

const { projectsApi, discussionsApi, baseStatus } = vi.hoisted(() => {
  const baseStatus = () => ({
    branch: 'feat/new-feature',
    default_branch: 'main',
    is_default_branch: false,
    files: [
      { path: 'src/main.rs', status: 'modified', staged: false },
      { path: 'src/lib.rs', status: 'added', staged: false },
    ] as Array<{ path: string; status: string; staged: boolean }>,
    committed_files: [] as Array<{ path: string; status: string; staged: boolean }>,
    ahead: 2,
    behind: 0,
    has_upstream: true,
    provider: 'github',
    pr_url: null as string | null,
    workspace: null as { ownership: string; state: string; head_sha?: string | null } | null,
  });
  return {
    baseStatus,
    projectsApi: {
      gitStatus: vi.fn(),
      gitDiff: vi.fn(),
      gitCommit: vi.fn(),
      gitPush: vi.fn(),
      gitCreateBranch: vi.fn(),
      createPr: vi.fn(),
      prTemplate: vi.fn(),
      exec: vi.fn(),
    },
    discussionsApi: {
      workspaces: vi.fn(),
      gitStatus: vi.fn(),
      gitDiff: vi.fn(),
      gitCommit: vi.fn(),
      gitPush: vi.fn(),
      createPr: vi.fn(),
      prTemplate: vi.fn(),
      exec: vi.fn(),
    },
  };
});

vi.mock('../../lib/api', () => ({
  projects: projectsApi,
  discussions: discussionsApi,
}));

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key}(${args.join('|')})` : key,
  }),
}));

// ReactMarkdown is ESM-heavy; stub it to a passthrough so the PR preview tab
// doesn't pull the whole markdown pipeline into the jsdom run.
vi.mock('react-markdown', () => ({
  default: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

import type { ComponentProps } from 'react';
import { GitPanel } from '../GitPanel';

type GitPanelProps = ComponentProps<typeof GitPanel>;

const onClose = vi.fn();

function renderPanel(props?: Partial<GitPanelProps>) {
  const merged: GitPanelProps = { projectId: 'p1', onClose, ...props };
  return render(<GitPanel {...merged} />);
}

beforeEach(() => {
  vi.clearAllMocks();
  projectsApi.gitStatus.mockResolvedValue(baseStatus());
  projectsApi.gitDiff.mockResolvedValue({ path: 'src/main.rs', diff: '@@ -1,2 +1,3 @@\n+added line\n context\n-removed line' });
  projectsApi.gitCommit.mockResolvedValue({ hash: 'abc123', message: 'done' });
  projectsApi.gitPush.mockResolvedValue({ success: true, message: 'pushed' });
  projectsApi.gitCreateBranch.mockResolvedValue({ branch: 'feat/x' });
  projectsApi.createPr.mockResolvedValue({ url: 'https://github.com/acme/repo/pull/1' });
  projectsApi.prTemplate.mockResolvedValue({ template: '## Summary\nbody', source: 'project' });
  projectsApi.exec.mockResolvedValue({ stdout: 'ok-out', stderr: '', exit_code: 0 });

  discussionsApi.gitStatus.mockResolvedValue(baseStatus());
  discussionsApi.workspaces.mockResolvedValue([]);
  discussionsApi.gitDiff.mockResolvedValue({ path: 'src/main.rs', diff: '@@ diff @@' });
  discussionsApi.gitCommit.mockResolvedValue({ hash: 'def456', message: 'done' });
  discussionsApi.gitPush.mockResolvedValue({ success: true, message: 'pushed' });
  discussionsApi.createPr.mockResolvedValue({ url: 'https://github.com/acme/repo/pull/2' });
  discussionsApi.prTemplate.mockResolvedValue({ template: 'tmpl', source: 'kronn' });
  discussionsApi.exec.mockResolvedValue({ stdout: 'd-out', stderr: '', exit_code: 0 });
});

afterEach(() => {
  cleanup();
});

describe('GitPanel — discussion workspace selection', () => {
  it('selects a declared CLI worktree and scopes every status refresh to it', async () => {
    discussionsApi.workspaces.mockResolvedValue([{
      id: 'workspace-1',
      disc_id: 'd1',
      session_pk: 42,
      session_agent_type: 'Codex',
      task_id: 'task-1',
      task_reference: 'KT-140',
      project_id: 'p1',
      workspace_path: '/tmp/kronn-kt140',
      canonical_path: '/tmp/kronn-kt140',
      branch: 'feature/kt140',
      head_sha: 'abc123',
      ownership: 'external',
      state: 'attached',
      created_at: '2026-01-01',
      updated_at: '2026-01-01',
    }]);

    renderPanel({ projectId: 'p1', discussionId: 'd1' });

    const picker = await screen.findByRole('combobox', { name: 'git.workspaceSelector' });
    expect(picker).toHaveValue('workspace-1');
    await waitFor(() => {
      expect(discussionsApi.gitStatus).toHaveBeenCalledWith('d1', 'workspace-1');
    });
  });

  it('honours the workspace selected from a Planning task', async () => {
    const workspace = (id: string, branch: string) => ({
      id,
      disc_id: 'd1',
      session_pk: id === 'workspace-1' ? 41 : 42,
      session_agent_type: 'Codex',
      task_id: 'task-1',
      task_reference: 'KT-140',
      project_id: 'p1',
      workspace_path: `/tmp/${id}`,
      canonical_path: `/tmp/${id}`,
      branch,
      head_sha: 'abc123',
      ownership: 'external',
      state: 'attached',
      created_at: '2026-01-01',
      updated_at: '2026-01-01',
    });
    discussionsApi.workspaces.mockResolvedValue([
      workspace('workspace-1', 'feature/one'),
      workspace('workspace-2', 'feature/two'),
    ]);

    renderPanel({
      projectId: 'p1',
      discussionId: 'd1',
      initialWorkspaceId: 'workspace-2',
    });

    const picker = await screen.findByRole('combobox', { name: 'git.workspaceSelector' });
    expect(picker).toHaveValue('workspace-2');
    await waitFor(() => {
      expect(discussionsApi.gitStatus).toHaveBeenCalledWith('d1', 'workspace-2');
    });
  });

  it('does not fall back when a Planning-linked workspace is missing', async () => {
    discussionsApi.workspaces.mockResolvedValue([{
      id: 'workspace-missing',
      disc_id: 'd1',
      session_pk: 42,
      session_agent_type: 'Codex',
      task_id: 'task-1',
      task_reference: 'KT-140',
      project_id: 'p1',
      workspace_path: '/tmp/gone',
      canonical_path: '/tmp/gone',
      branch: 'feature/gone',
      head_sha: 'abc123',
      ownership: 'external',
      state: 'missing',
      created_at: '2026-01-01',
      updated_at: '2026-01-01',
    }]);
    discussionsApi.gitStatus.mockRejectedValue(new Error('Workspace is missing'));

    renderPanel({
      projectId: 'p1',
      discussionId: 'd1',
      initialWorkspaceId: 'workspace-missing',
    });

    const picker = await screen.findByRole('combobox', { name: 'git.workspaceSelector' });
    expect(picker).toHaveValue('workspace-missing');
    await waitFor(() => {
      expect(discussionsApi.gitStatus).toHaveBeenCalledWith('d1', 'workspace-missing');
    });
    expect(discussionsApi.gitStatus).not.toHaveBeenCalledWith('d1');
    expect(await screen.findByText('Error: Workspace is missing')).toBeInTheDocument();
  });

  it('renders cleaned child-workspace provenance, commits and files read-only in the parent', async () => {
    discussionsApi.workspaces.mockResolvedValue([{
      id: 'workspace-cleaned',
      disc_id: 'd-child',
      session_pk: null,
      session_agent_type: null,
      task_id: 'task-451',
      task_reference: 'KT-451',
      project_id: 'p1',
      workspace_path: '/tmp/removed-worker',
      canonical_path: null,
      branch: 'kronn/task/KT-451-worker',
      head_sha: 'abcdef1234567890',
      ownership: 'managed',
      state: 'detached',
      parent_discussion_id: 'd-parent',
      base_sha: '0000000000000000',
      task_execution_id: 'exec-451',
      created_at: '2026-01-01',
      updated_at: '2026-01-02',
    }]);
    discussionsApi.gitStatus.mockResolvedValue({
      ...baseStatus(),
      branch: 'kronn/task/KT-451-worker',
      files: [],
      committed_files: [{ path: 'src/provenance.rs', status: 'added', staged: true }],
      commits: [{
        sha: 'abcdef1234567890', short_sha: 'abcdef1',
        subject: 'fix: preserve provenance', author_name: 'Ollama', author_time: 1,
      }],
      workspace: {
        workspace_id: 'workspace-cleaned', ownership: 'managed', state: 'detached',
        path: '/tmp/removed-worker', branch: 'kronn/task/KT-451-worker',
        base_sha: '0000000000000000', head_sha: 'abcdef1234567890',
        integrated_sha: 'fedcba9876543210', task_execution_id: 'exec-451',
        task_reference: 'KT-451',
      },
    });

    renderPanel({ discussionId: 'd-parent' });

    const picker = await screen.findByRole('combobox', { name: 'git.workspaceSelector' });
    await waitFor(() => expect(picker).toHaveValue('workspace-cleaned'));
    expect(await screen.findByText('/tmp/removed-worker')).toBeInTheDocument();
    expect(screen.getByTestId('git-commit-history')).toHaveTextContent('fix: preserve provenance');
    expect(screen.getByTestId('git-committed-section')).toHaveTextContent('src/provenance.rs');
    expect(screen.queryByText('git.push')).toBeNull();
    expect(screen.queryByText('git.createPr')).toBeNull();

    fireEvent.click(screen.getByText('src/provenance.rs'));
    await waitFor(() => expect(discussionsApi.gitDiff).toHaveBeenCalledWith(
      'd-parent', 'src/provenance.rs', true, 'workspace-cleaned',
    ));
  });
});

// ─── Commit ───────────────────────────────────────────────────────────────────

describe('GitPanel — commit', () => {
  async function openCommit() {
    renderPanel();
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());
    fireEvent.click(screen.getByText('git.commit'));
    await waitFor(() => expect(screen.getAllByRole('checkbox').length).toBeGreaterThan(0));
  }

  it('calls projects.gitCommit with selected files + trimmed message', async () => {
    await openCommit();
    // Commit-shortcut pre-selects all files.
    const input = screen.getByPlaceholderText('git.commitMessage');
    fireEvent.change(input, { target: { value: '  my commit  ' } });
    fireEvent.click(screen.getByText(/git\.commitSelected/));

    await waitFor(() => expect(projectsApi.gitCommit).toHaveBeenCalledTimes(1));
    expect(projectsApi.gitCommit).toHaveBeenCalledWith('p1', {
      files: ['src/main.rs', 'src/lib.rs'],
      message: 'my commit',
      amend: false,
      sign: false,
    });
    // Re-fetch after success.
    await waitFor(() => expect(projectsApi.gitStatus).toHaveBeenCalledTimes(2));
  });

  it('passes amend + sign flags through', async () => {
    await openCommit();
    const checks = screen.getAllByRole('checkbox') as HTMLInputElement[];
    // Last two checkboxes are amend + sign options (file checkboxes come first).
    fireEvent.click(screen.getByText('git.amend').querySelector('input')!);
    fireEvent.click(screen.getByText('git.sign').querySelector('input')!);
    fireEvent.change(screen.getByPlaceholderText('git.commitMessage'), { target: { value: 'x' } });
    fireEvent.click(screen.getByText(/git\.commitSelected/));

    await waitFor(() => expect(projectsApi.gitCommit).toHaveBeenCalledWith('p1', {
      files: expect.any(Array),
      message: 'x',
      amend: true,
      sign: true,
    }));
    expect(checks.length).toBeGreaterThanOrEqual(2);
  });

  it('routes commit to discussions.gitCommit when discussionId is set', async () => {
    renderPanel({ projectId: undefined, discussionId: 'd1' });
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());
    fireEvent.click(screen.getByText('git.commit'));
    await waitFor(() => expect(screen.getAllByRole('checkbox').length).toBeGreaterThan(0));
    fireEvent.change(screen.getByPlaceholderText('git.commitMessage'), { target: { value: 'disc msg' } });
    fireEvent.click(screen.getByText(/git\.commitSelected/));

    await waitFor(() => expect(discussionsApi.gitCommit).toHaveBeenCalledWith('d1', expect.objectContaining({ message: 'disc msg' })));
    expect(projectsApi.gitCommit).not.toHaveBeenCalled();
  });

  it('toggleFile deselects a file → commit button disabled when none selected', async () => {
    await openCommit();
    // The two amend/sign option checkboxes live in their own labels; the
    // per-file selection checkboxes are the ones inside the file rows. Target
    // file checkboxes by their sibling file-path text rather than DOM order.
    const fileCheckboxes = screen.getAllByRole('checkbox').filter(cb =>
      (cb as HTMLInputElement).style.marginRight === '6px',
    ) as HTMLInputElement[];
    expect(fileCheckboxes.length).toBe(2);
    fileCheckboxes.forEach(cb => fireEvent.click(cb));
    fireEvent.change(screen.getByPlaceholderText('git.commitMessage'), { target: { value: 'msg' } });
    const submit = screen.getByText(/git\.commitSelected/).closest('button')!;
    expect(submit.disabled).toBe(true);
  });

  it('selectAll toggles between all and none', async () => {
    await openCommit();
    // Starts all-selected → label is deselectAll.
    expect(screen.getByText('git.deselectAll')).toBeDefined();
    fireEvent.click(screen.getByText('git.deselectAll'));
    await waitFor(() => expect(screen.getByText('git.selectAll')).toBeDefined());
    fireEvent.click(screen.getByText('git.selectAll'));
    await waitFor(() => expect(screen.getByText('git.deselectAll')).toBeDefined());
  });

  it('surfaces a commit failure as error text', async () => {
    projectsApi.gitCommit.mockRejectedValueOnce(new Error('commit boom'));
    await openCommit();
    fireEvent.change(screen.getByPlaceholderText('git.commitMessage'), { target: { value: 'msg' } });
    fireEvent.click(screen.getByText(/git\.commitSelected/));
    await waitFor(() => expect(screen.getByText(/commit boom/)).toBeDefined());
  });

  it('no-ops commit when message is empty', async () => {
    await openCommit();
    // No message typed → handler early-returns.
    const submit = screen.getByText(/git\.commitSelected/).closest('button')!;
    expect(submit.disabled).toBe(true);
    expect(projectsApi.gitCommit).not.toHaveBeenCalled();
  });
});

// ─── Push ───────────────────────────────────────────────────────────────────

describe('GitPanel — push', () => {
  it('calls projects.gitPush and shows success', async () => {
    renderPanel();
    await waitFor(() => expect(screen.getByText('git.push')).toBeDefined());
    fireEvent.click(screen.getByText('git.push'));
    await waitFor(() => expect(projectsApi.gitPush).toHaveBeenCalledWith('p1'));
    await waitFor(() => expect(screen.getByText('git.pushSuccess')).toBeDefined());
    // re-fetch after push
    await waitFor(() => expect(projectsApi.gitStatus).toHaveBeenCalledTimes(2));
  });

  it('routes to discussions.gitPush when discussionId is set', async () => {
    renderPanel({ projectId: undefined, discussionId: 'd1' });
    await waitFor(() => expect(screen.getByText('git.push')).toBeDefined());
    fireEvent.click(screen.getByText('git.push'));
    await waitFor(() => expect(discussionsApi.gitPush).toHaveBeenCalledWith('d1'));
  });

  it('surfaces a push failure as error text (catch branch)', async () => {
    projectsApi.gitPush.mockRejectedValueOnce(new Error('push rejected'));
    renderPanel();
    await waitFor(() => expect(screen.getByText('git.push')).toBeDefined());
    fireEvent.click(screen.getByText('git.push'));
    await waitFor(() => expect(screen.getByText(/push rejected/)).toBeDefined());
  });

  it('hides push button when not ahead', async () => {
    projectsApi.gitStatus.mockResolvedValue({ ...baseStatus(), ahead: 0 });
    renderPanel();
    await waitFor(() => expect(screen.getByText('feat/new-feature')).toBeDefined());
    expect(screen.queryByText('git.push')).toBeNull();
  });
});

// ─── Create branch ────────────────────────────────────────────────────────────

describe('GitPanel — create branch', () => {
  it('shows on-default-branch warning and opens branch form', async () => {
    projectsApi.gitStatus.mockResolvedValue({ ...baseStatus(), is_default_branch: true });
    renderPanel();
    await waitFor(() => expect(screen.getByText('git.onDefaultBranch')).toBeDefined());
    fireEvent.click(screen.getByText('git.createBranch'));
    await waitFor(() => expect(screen.getByPlaceholderText('git.branchName')).toBeDefined());
  });

  it('calls projects.gitCreateBranch with trimmed name then re-fetches', async () => {
    projectsApi.gitStatus.mockResolvedValue({ ...baseStatus(), is_default_branch: true });
    renderPanel();
    await waitFor(() => expect(screen.getByText('git.createBranch')).toBeDefined());
    fireEvent.click(screen.getByText('git.createBranch'));
    const input = await screen.findByPlaceholderText('git.branchName');
    fireEvent.change(input, { target: { value: '  feat/added  ' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    await waitFor(() => expect(projectsApi.gitCreateBranch).toHaveBeenCalledWith('p1', { name: 'feat/added' }));
    await waitFor(() => expect(projectsApi.gitStatus).toHaveBeenCalledTimes(2));
  });

  it('surfaces a create-branch failure as error text', async () => {
    projectsApi.gitStatus.mockResolvedValue({ ...baseStatus(), is_default_branch: true });
    projectsApi.gitCreateBranch.mockRejectedValueOnce(new Error('branch boom'));
    renderPanel();
    await waitFor(() => expect(screen.getByText('git.createBranch')).toBeDefined());
    fireEvent.click(screen.getByText('git.createBranch'));
    const input = await screen.findByPlaceholderText('git.branchName');
    fireEvent.change(input, { target: { value: 'feat/x' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(screen.getByText(/branch boom/)).toBeDefined());
  });

  it('no-ops create-branch when name is empty', async () => {
    projectsApi.gitStatus.mockResolvedValue({ ...baseStatus(), is_default_branch: true });
    renderPanel();
    await waitFor(() => expect(screen.getByText('git.createBranch')).toBeDefined());
    fireEvent.click(screen.getByText('git.createBranch'));
    const input = await screen.findByPlaceholderText('git.branchName');
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(projectsApi.gitCreateBranch).not.toHaveBeenCalled();
  });
});

// ─── Create PR ────────────────────────────────────────────────────────────────

describe('GitPanel — create PR', () => {
  async function openPrForm() {
    renderPanel();
    await waitFor(() => expect(screen.getByText('git.createPr')).toBeDefined());
    fireEvent.click(screen.getByText('git.createPr'));
    await waitFor(() => expect(screen.getByPlaceholderText('git.prTitle')).toBeDefined());
  }

  it('fetches the template and prefills body + title on open', async () => {
    await openPrForm();
    await waitFor(() => expect(projectsApi.prTemplate).toHaveBeenCalledWith('p1'));
    const titleInput = screen.getByPlaceholderText('git.prTitle') as HTMLInputElement;
    // branch "feat/new-feature" → "feat/new feature" (kronn/ stripped, - → space)
    expect(titleInput.value).toContain('new feature');
    const bodyArea = screen.getByPlaceholderText('git.prBodyPlaceholder') as HTMLTextAreaElement;
    expect(bodyArea.value).toContain('## Summary');
  });

  it('creates the PR and shows the PR url', async () => {
    await openPrForm();
    fireEvent.click(screen.getByText('git.submitPr'));
    await waitFor(() => expect(projectsApi.createPr).toHaveBeenCalledWith('p1', {
      title: expect.any(String),
      body: expect.stringContaining('## Summary'),
      base: 'main',
    }));
    await waitFor(() => expect(screen.getByText(/pull\/1/)).toBeDefined());
  });

  it('auto-pushes before PR when branch has no upstream', async () => {
    projectsApi.gitStatus.mockResolvedValue({ ...baseStatus(), has_upstream: false });
    await openPrForm();
    fireEvent.click(screen.getByText('git.submitPr'));
    await waitFor(() => expect(projectsApi.gitPush).toHaveBeenCalledWith('p1'));
    await waitFor(() => expect(projectsApi.createPr).toHaveBeenCalled());
  });

  it('does NOT auto-push when upstream already exists', async () => {
    await openPrForm();
    fireEvent.click(screen.getByText('git.submitPr'));
    await waitFor(() => expect(projectsApi.createPr).toHaveBeenCalled());
    expect(projectsApi.gitPush).not.toHaveBeenCalled();
  });

  it('surfaces a createPr failure as error text', async () => {
    projectsApi.createPr.mockRejectedValueOnce(new Error('pr boom'));
    await openPrForm();
    fireEvent.click(screen.getByText('git.submitPr'));
    await waitFor(() => expect(screen.getByText(/pr boom/)).toBeDefined());
  });

  it('tolerates a prTemplate fetch failure (empty body, form still opens)', async () => {
    projectsApi.prTemplate.mockRejectedValueOnce(new Error('no template'));
    await openPrForm();
    const bodyArea = screen.getByPlaceholderText('git.prBodyPlaceholder') as HTMLTextAreaElement;
    expect(bodyArea.value).toBe('');
  });

  it('preview tab renders the markdown body', async () => {
    await openPrForm();
    fireEvent.click(screen.getByText('git.prPreview'));
    await waitFor(() => expect(screen.getByText(/## Summary/)).toBeDefined());
  });

  it('routes PR creation to discussions api when discussionId set', async () => {
    renderPanel({ projectId: undefined, discussionId: 'd1' });
    await waitFor(() => expect(screen.getByText('git.createPr')).toBeDefined());
    fireEvent.click(screen.getByText('git.createPr'));
    await waitFor(() => expect(screen.getByPlaceholderText('git.prTitle')).toBeDefined());
    fireEvent.click(screen.getByText('git.submitPr'));
    await waitFor(() => expect(discussionsApi.createPr).toHaveBeenCalledWith('d1', expect.any(Object)));
  });

  it('shows gitlab MR labels when provider is gitlab', async () => {
    projectsApi.gitStatus.mockResolvedValue({ ...baseStatus(), provider: 'gitlab' });
    renderPanel();
    await waitFor(() => expect(screen.getByText('git.createMr')).toBeDefined());
  });

  it('renders existing PR link when pr_url is set', async () => {
    projectsApi.gitStatus.mockResolvedValue({ ...baseStatus(), pr_url: 'https://github.com/acme/repo/pull/9' });
    renderPanel();
    await waitFor(() => {
      const link = screen.getByText('acme/repo/pull/9') as HTMLAnchorElement;
      expect(link.getAttribute('href')).toBe('https://github.com/acme/repo/pull/9');
    });
    // PR button hidden when a PR already exists.
    expect(screen.queryByText('git.createPr')).toBeNull();
  });
});

// ─── Diff view ────────────────────────────────────────────────────────────────

describe('GitPanel — diff', () => {
  it('expanding the list view (no file selected yet) does not jump into a diff', async () => {
    const { container } = renderPanel();
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());

    fireEvent.click(screen.getByLabelText('git.expandPanel'));

    // The whole panel grows (list view, just bigger) — it must NOT swap onto
    // the diff-viewer branch merely because it was expanded with nothing
    // selected. That used to auto-open the first file's diff and hide the
    // workspace recap + commit history, which only render in the list view.
    await waitFor(() =>
      expect(container.querySelector('.git-panel')?.getAttribute('data-expanded')).toBe('true'));
    expect(projectsApi.gitDiff).not.toHaveBeenCalled();
    expect(screen.queryByLabelText('git.back')).toBeNull();
    expect(screen.getByText('src/main.rs')).toBeDefined();
    expect(screen.getByText('src/lib.rs')).toBeDefined();
  });

  it('clicking a file then expanding shows the split diff and file-list view', async () => {
    const { container } = renderPanel();
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/main.rs'));
    await waitFor(() => expect(projectsApi.gitDiff).toHaveBeenCalledWith('p1', 'src/main.rs', false));

    fireEvent.click(screen.getByLabelText('git.expandPanel'));

    expect(container.querySelector('.git-panel')?.getAttribute('data-expanded')).toBe('true');
    expect(screen.getByLabelText('git.changedFilesList')).toBeDefined();
    expect(screen.getByLabelText('git.collapsePanel')).toBeDefined();
  });

  it('constrains the expanded grid row so a long file list remains scrollable', async () => {
    projectsApi.gitStatus.mockResolvedValue({
      ...baseStatus(),
      files: Array.from({ length: 124 }, (_, index) => ({
        path: `src/file-${index}.ts`,
        status: 'modified',
        staged: false,
      })),
    });
    renderPanel();

    await waitFor(() => expect(screen.getByText('src/file-0.ts')).toBeDefined());
    fireEvent.click(screen.getByText('src/file-0.ts'));
    await waitFor(() => expect(screen.getByLabelText('git.back')).toBeDefined());
    fireEvent.click(screen.getByLabelText('git.expandPanel'));
    await waitFor(() => expect(screen.getByLabelText('git.changedFilesList')).toBeDefined());

    expect(screen.getByText('src/file-123.ts')).toBeDefined();
    const gitPanelCss = readFileSync('src/components/GitPanel.css', 'utf8');
    const expandedLayoutRule = gitPanelCss.match(/\.git-expanded-layout\s*\{([^}]*)\}/)?.[1];
    const expandedFilesRule = gitPanelCss.match(/\.git-expanded-files\s*\{([^}]*)\}/)?.[1];
    expect(expandedLayoutRule).toContain('grid-template-rows: minmax(0, 1fr);');
    expect(expandedFilesRule).toContain('min-height: 0;');
  });

  it('opens the diff view on file click and fetches via projects.gitDiff', async () => {
    renderPanel();
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/main.rs'));
    await waitFor(() => expect(projectsApi.gitDiff).toHaveBeenCalledWith('p1', 'src/main.rs', false));
    // Diff header shows path + Back button.
    await waitFor(() => expect(screen.getByLabelText('git.back')).toBeDefined());
    await waitFor(() => expect(screen.getByText(/added line/)).toBeDefined());
  });

  it('Back button returns to the main view', async () => {
    renderPanel();
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/main.rs'));
    await waitFor(() => expect(screen.getByLabelText('git.back')).toBeDefined());
    fireEvent.click(screen.getByLabelText('git.back'));
    await waitFor(() => expect(screen.getByText('git.title')).toBeDefined());
  });

  it('renders "Error:" content when gitDiff rejects (catch branch)', async () => {
    projectsApi.gitDiff.mockRejectedValueOnce(new Error('diff boom'));
    renderPanel();
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/main.rs'));
    await waitFor(() => expect(screen.getByText(/Error:.*diff boom/)).toBeDefined());
  });

  it('routes diff fetch to discussions.gitDiff when discussionId set', async () => {
    renderPanel({ projectId: undefined, discussionId: 'd1' });
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/main.rs'));
    await waitFor(() => expect(discussionsApi.gitDiff).toHaveBeenCalledWith('d1', 'src/main.rs', false));
  });

  it('clicks a COMMITTED file → fetches the committed diff (committed=true)', async () => {
    // Committed files have a clean working tree, so a plain diff is empty — the
    // click must request the committed diff (`<default>...HEAD`).
    const withCommitted = baseStatus();
    withCommitted.committed_files = [{ path: 'src/feature.rs', status: 'A', staged: true }];
    projectsApi.gitStatus.mockResolvedValue(withCommitted);
    renderPanel();
    await waitFor(() => expect(screen.getByText('src/feature.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/feature.rs'));
    await waitFor(() => expect(projectsApi.gitDiff).toHaveBeenCalledWith('p1', 'src/feature.rs', true));
  });
});

// ─── KT-453: "Talk about it in the discussion" from a diff selection ───────

describe('GitPanel — diff comment reference (KT-453)', () => {
  const REALISTIC_DIFF = [
    '@@ -10,2 +10,2 @@',
    '-old line',
    '+new line',
  ].join('\n');

  it('is wired only when discussion-scoped — a project-only panel gets no comment affordance', async () => {
    projectsApi.gitDiff.mockResolvedValue({ path: 'src/main.rs', diff: REALISTIC_DIFF });
    renderPanel(); // project-only (no discussionId)
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/main.rs'));
    await waitFor(() => expect(screen.getByLabelText('git.back')).toBeDefined());
    expect(screen.queryAllByLabelText('git.diffCommentLine')).toHaveLength(0);
  });

  it('builds a single-line reference with the workspace HEAD sha for a committed diff', async () => {
    const dispatchSpy = vi.spyOn(window, 'dispatchEvent');
    const withWorkspace = baseStatus();
    withWorkspace.committed_files = [{ path: 'src/feature.rs', status: 'A', staged: true }];
    withWorkspace.workspace = { ownership: 'managed', state: 'attached', head_sha: 'abc1234567890' };
    discussionsApi.gitStatus.mockResolvedValue(withWorkspace);
    discussionsApi.gitDiff.mockResolvedValue({ path: 'src/feature.rs', diff: REALISTIC_DIFF });

    renderPanel({ projectId: undefined, discussionId: 'd1' });
    await waitFor(() => expect(screen.getByText('src/feature.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/feature.rs'));
    await waitFor(() => expect(screen.getAllByLabelText('git.diffCommentLine')).toHaveLength(2));

    // The `+new line` add — not the del — so this test stays about the sha,
    // independent of the (old) disambiguation covered separately below.
    fireEvent.click(screen.getAllByLabelText('git.diffCommentLine')[1]!);
    fireEvent.click(screen.getByText('git.diffTalkAboutIt'));

    const call = dispatchSpy.mock.calls.find(([event]) => (event as CustomEvent).type === 'kronn:composer-prefill');
    expect(call).toBeDefined();
    const detail = (call![0] as CustomEvent).detail;
    expect(detail.discussionId).toBe('d1');
    expect(detail.text).toContain('```diff\n+new line\n```');
    expect(detail.text).toContain('git.diffCommentIntro(src/feature.rs:10 · HEAD abc1234567)');
  });

  it('builds a start-end range reference with no sha when the panel has no workspace at all', async () => {
    // Line numbers must genuinely differ across the selection (context @10,
    // an inserted line lands at new-line 11) to exercise the range branch —
    // a 1-for-1 del/add pair at the same source line collapses to a single
    // line number instead, which is correct there but not what this test
    // wants to pin.
    const rangeDiff = ['@@ -10,1 +10,2 @@', ' context', '+added'].join('\n');
    const dispatchSpy = vi.spyOn(window, 'dispatchEvent');
    discussionsApi.gitDiff.mockResolvedValue({ path: 'src/main.rs', diff: rangeDiff });

    renderPanel({ projectId: undefined, discussionId: 'd1' });
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/main.rs'));
    await waitFor(() => expect(screen.getAllByLabelText('git.diffCommentLine')).toHaveLength(2));

    const buttons = screen.getAllByLabelText('git.diffCommentLine');
    fireEvent.click(buttons[0]!);
    fireEvent.click(buttons[1]!);
    fireEvent.click(screen.getByText('git.diffTalkAboutIt'));

    const call = dispatchSpy.mock.calls.find(([event]) => (event as CustomEvent).type === 'kronn:composer-prefill');
    const detail = (call![0] as CustomEvent).detail;
    // No workspace at all (the fixture's default) → nothing to reference.
    expect(detail.text).not.toContain('HEAD');
    expect(detail.text).toContain('git.diffCommentIntro(src/main.rs:10-11)');
  });

  it('labels the workspace base HEAD as WORKTREE (not an exact commit) for an uncommitted diff', async () => {
    const rangeDiff = ['@@ -10,1 +10,2 @@', ' context', '+added'].join('\n');
    const withWorkspace = baseStatus();
    withWorkspace.workspace = { ownership: 'managed', state: 'attached', head_sha: 'def9876543210' };
    discussionsApi.gitStatus.mockResolvedValue(withWorkspace);
    const dispatchSpy = vi.spyOn(window, 'dispatchEvent');
    discussionsApi.gitDiff.mockResolvedValue({ path: 'src/main.rs', diff: rangeDiff });

    renderPanel({ projectId: undefined, discussionId: 'd1' });
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/main.rs'));
    await waitFor(() => expect(screen.getAllByLabelText('git.diffCommentLine')).toHaveLength(2));

    const buttons = screen.getAllByLabelText('git.diffCommentLine');
    fireEvent.click(buttons[0]!);
    fireEvent.click(buttons[1]!);
    fireEvent.click(screen.getByText('git.diffTalkAboutIt'));

    const call = dispatchSpy.mock.calls.find(([event]) => (event as CustomEvent).type === 'kronn:composer-prefill');
    const detail = (call![0] as CustomEvent).detail;
    expect(detail.text).toContain('git.diffCommentIntro(src/main.rs:10-11 · WORKTREE · base HEAD def9876543)');
  });

  it('flags a pure-deletion selection as (old) so its line number is never misread as post-image', async () => {
    const withCommitted = baseStatus();
    withCommitted.committed_files = [{ path: 'src/feature.rs', status: 'A', staged: true }];
    withCommitted.workspace = { ownership: 'managed', state: 'attached', head_sha: 'abc1234567890' };
    discussionsApi.gitStatus.mockResolvedValue(withCommitted);
    const dispatchSpy = vi.spyOn(window, 'dispatchEvent');
    const twoDeletions = ['@@ -10,2 +10,0 @@', '-old line one', '-old line two'].join('\n');
    discussionsApi.gitDiff.mockResolvedValue({ path: 'src/feature.rs', diff: twoDeletions });

    renderPanel({ projectId: undefined, discussionId: 'd1' });
    await waitFor(() => expect(screen.getByText('src/feature.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/feature.rs'));
    await waitFor(() => expect(screen.getAllByLabelText('git.diffCommentLine')).toHaveLength(2));

    const buttons = screen.getAllByLabelText('git.diffCommentLine');
    fireEvent.click(buttons[0]!);
    fireEvent.click(buttons[1]!);
    fireEvent.click(screen.getByText('git.diffTalkAboutIt'));

    const call = dispatchSpy.mock.calls.find(([event]) => (event as CustomEvent).type === 'kronn:composer-prefill');
    const detail = (call![0] as CustomEvent).detail;
    expect(detail.text).toContain('git.diffCommentIntro(src/feature.rs:10-11 (old) · HEAD abc1234567)');
  });
});

// ─── Refresh ──────────────────────────────────────────────────────────────────

describe('GitPanel — refresh', () => {
  it('refresh button re-fetches status', async () => {
    renderPanel();
    await waitFor(() => expect(projectsApi.gitStatus).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByLabelText('git.refresh'));
    await waitFor(() => expect(projectsApi.gitStatus).toHaveBeenCalledTimes(2));
  });
});
