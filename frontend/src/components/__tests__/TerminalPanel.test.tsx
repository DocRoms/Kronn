import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { DiscussionWorkspace } from '../../types/generated';

const discussionsApi = vi.hoisted(() => ({
  workspaces: vi.fn(),
  exec: vi.fn(),
}));

vi.mock('../../lib/api', () => ({ discussions: discussionsApi }));

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key}(${args.join('|')})` : key,
  }),
}));

import { TerminalPanel } from '../TerminalPanel';

function workspace(overrides: Partial<DiscussionWorkspace> = {}): DiscussionWorkspace {
  return {
    id: 'workspace-1',
    disc_id: 'discussion-1',
    session_pk: 12,
    session_agent_type: 'Codex',
    task_id: null,
    task_reference: null,
    project_id: 'project-1',
    workspace_path: '/tmp/workspace-1',
    canonical_path: '/tmp/workspace-1',
    branch: 'feat/terminal-panel',
    head_sha: 'abc123',
    ownership: 'external',
    state: 'attached',
    parent_discussion_id: null,
    base_sha: null,
    task_execution_id: null,
    created_at: '2026-08-27T00:00:00Z',
    updated_at: '2026-08-27T00:00:00Z',
    ...overrides,
  };
}

function renderPanel(onClose = vi.fn()) {
  return {
    onClose,
    ...render(<TerminalPanel discussionId="discussion-1" onClose={onClose} />),
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  discussionsApi.workspaces.mockResolvedValue([]);
  discussionsApi.exec.mockResolvedValue({ stdout: 'command output', stderr: '', exit_code: 0 });
});

afterEach(cleanup);

describe('TerminalPanel', () => {
  it('runs a command in the discussion default workspace and renders stdout', async () => {
    renderPanel();
    const input = screen.getByPlaceholderText('git.terminalPlaceholder');
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.change(input, { target: { value: 'git status' } });
    fireEvent.submit(input.closest('form')!);

    await waitFor(() => expect(discussionsApi.exec).toHaveBeenCalledWith(
      'discussion-1',
      'git status',
    ));
    expect(await screen.findByText('command output')).toBeInTheDocument();
    expect(screen.getByText('$ git status')).toBeInTheDocument();
  });

  it('targets the selected attached workspace', async () => {
    discussionsApi.workspaces.mockResolvedValue([workspace()]);
    renderPanel();

    const picker = await screen.findByRole('combobox', { name: 'git.workspaceSelector' });
    await waitFor(() => expect(picker).toHaveValue('workspace-1'));
    const input = screen.getByPlaceholderText('git.terminalPlaceholder');
    fireEvent.change(input, { target: { value: 'pwd' } });
    fireEvent.submit(input.closest('form')!);

    await waitFor(() => expect(discussionsApi.exec).toHaveBeenCalledWith(
      'discussion-1',
      'pwd',
      'workspace-1',
    ));
  });

  it('does not execute twice when the command is submitted twice synchronously', async () => {
    let resolveExec: ((result: { stdout: string; stderr: string; exit_code: number }) => void) | undefined;
    discussionsApi.exec.mockReturnValueOnce(new Promise(resolve => {
      resolveExec = resolve;
    }));
    renderPanel();
    const input = screen.getByPlaceholderText('git.terminalPlaceholder');
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.change(input, { target: { value: 'git status' } });
    const form = input.closest('form')!;

    await act(async () => {
      form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
      form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    });

    expect(discussionsApi.exec).toHaveBeenCalledTimes(1);
    await act(async () => {
      resolveExec?.({ stdout: 'done', stderr: '', exit_code: 0 });
    });
    expect(await screen.findByText('done')).toBeInTheDocument();
  });

  it('keeps historical workspace evidence read-only', async () => {
    discussionsApi.workspaces.mockResolvedValue([
      workspace({ state: 'detached', task_reference: 'KT-467' }),
    ]);
    renderPanel();

    const input = await screen.findByPlaceholderText('git.terminalPlaceholder');
    await waitFor(() => expect(input).toBeDisabled());
    expect(screen.getByText('git.terminalHistorical')).toBeInTheDocument();
    expect(discussionsApi.exec).not.toHaveBeenCalled();
  });

  it('renders command failures without losing the attempted command', async () => {
    discussionsApi.exec.mockRejectedValueOnce(new Error('exec boom'));
    renderPanel();
    const input = screen.getByPlaceholderText('git.terminalPlaceholder');
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.change(input, { target: { value: 'false' } });
    fireEvent.submit(input.closest('form')!);

    expect(await screen.findByText(/exec boom/)).toBeInTheDocument();
    expect(screen.getByText('$ false')).toBeInTheDocument();
  });

  it('fails closed when workspace discovery is unavailable', async () => {
    discussionsApi.workspaces.mockRejectedValueOnce(new Error('backend offline'));
    renderPanel();

    expect(await screen.findByText('git.terminalWorkspaceUnavailable')).toBeInTheDocument();
    expect(screen.getByPlaceholderText('git.terminalPlaceholder')).toBeDisabled();
    expect(discussionsApi.exec).not.toHaveBeenCalled();
  });

  it('ignores an empty command and exposes the panel close action', async () => {
    const onClose = vi.fn();
    renderPanel(onClose);
    const input = screen.getByPlaceholderText('git.terminalPlaceholder');
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.submit(input.closest('form')!);
    expect(discussionsApi.exec).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'common.close' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
