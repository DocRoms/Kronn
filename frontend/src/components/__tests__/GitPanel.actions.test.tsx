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
 *  - handleExec mini-terminal (success + rejected)
 *  - toggleFile / selectAll selection logic
 *  - loading spinners / disabled states
 *  - on-default-branch warning + create-branch shortcut
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup, act } from '@testing-library/react';

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
  it('opens the diff view on file click and fetches via projects.gitDiff', async () => {
    renderPanel();
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/main.rs'));
    await waitFor(() => expect(projectsApi.gitDiff).toHaveBeenCalledWith('p1', 'src/main.rs'));
    // Diff header shows path + Back button.
    await waitFor(() => expect(screen.getByLabelText('Back')).toBeDefined());
    await waitFor(() => expect(screen.getByText(/added line/)).toBeDefined());
  });

  it('Back button returns to the main view', async () => {
    renderPanel();
    await waitFor(() => expect(screen.getByText('src/main.rs')).toBeDefined());
    fireEvent.click(screen.getByText('src/main.rs'));
    await waitFor(() => expect(screen.getByLabelText('Back')).toBeDefined());
    fireEvent.click(screen.getByLabelText('Back'));
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
    await waitFor(() => expect(discussionsApi.gitDiff).toHaveBeenCalledWith('d1', 'src/main.rs'));
  });
});

// ─── Mini terminal ──────────────────────────────────────────────────────────

describe('GitPanel — terminal', () => {
  it('runs a command via projects.exec and renders stdout', async () => {
    renderPanel({ terminalEnabled: true });
    await waitFor(() => expect(screen.getByText('git.terminal')).toBeDefined());
    fireEvent.click(screen.getByText('git.terminal'));
    const input = await screen.findByPlaceholderText('git.terminalPlaceholder');
    fireEvent.change(input, { target: { value: 'ls -la' } });
    await act(async () => { fireEvent.keyDown(input, { key: 'Enter' }); });
    await waitFor(() => expect(projectsApi.exec).toHaveBeenCalledWith('p1', 'ls -la'));
    await waitFor(() => expect(screen.getByText(/ok-out/)).toBeDefined());
    expect(screen.getByText('$ ls -la')).toBeDefined();
  });

  it('renders stderr from a failed exec (catch branch)', async () => {
    projectsApi.exec.mockRejectedValueOnce(new Error('exec boom'));
    renderPanel({ terminalEnabled: true });
    await waitFor(() => expect(screen.getByText('git.terminal')).toBeDefined());
    fireEvent.click(screen.getByText('git.terminal'));
    const input = await screen.findByPlaceholderText('git.terminalPlaceholder');
    fireEvent.change(input, { target: { value: 'boom' } });
    await act(async () => { fireEvent.keyDown(input, { key: 'Enter' }); });
    await waitFor(() => expect(screen.getByText(/exec boom/)).toBeDefined());
  });

  it('no-ops exec on empty input', async () => {
    renderPanel({ terminalEnabled: true });
    await waitFor(() => expect(screen.getByText('git.terminal')).toBeDefined());
    fireEvent.click(screen.getByText('git.terminal'));
    const input = await screen.findByPlaceholderText('git.terminalPlaceholder');
    await act(async () => { fireEvent.keyDown(input, { key: 'Enter' }); });
    expect(projectsApi.exec).not.toHaveBeenCalled();
  });

  it('terminal section absent when terminalEnabled is false', async () => {
    renderPanel({ terminalEnabled: false });
    await waitFor(() => expect(screen.getByText('feat/new-feature')).toBeDefined());
    expect(screen.queryByText('git.terminal')).toBeNull();
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
