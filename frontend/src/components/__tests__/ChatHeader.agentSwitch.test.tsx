import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

import { discussions as discussionsApi } from '../../lib/api';
import { ChatHeader } from '../ChatHeader';
import type { AgentDetection, AgentType, Discussion } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';

const noop = () => {};
const t = (key: string) => key;

function makeDiscussion(): Discussion {
  return {
    id: 'disc-agent-switch',
    project_id: null,
    title: 'Agent picker',
    agent: 'ClaudeCode',
    language: 'fr',
    participants: ['ClaudeCode'],
    messages: [],
    message_count: 0,
    non_system_message_count: 0,
    tier: 'default',
    summary_strategy: 'Auto',
    introspection_call_count: 0,
    archived: false,
    pinned: false,
    pin_first_message: false,
    workspace_mode: 'Direct',
    workspace_path: null,
    created_at: '2026-07-24T00:00:00Z',
    updated_at: '2026-07-24T00:00:00Z',
    awaiting_agent: false,
  };
}

function makeAgent(agentType: AgentType, enabled = true): AgentDetection {
  return {
    name: agentType,
    agent_type: agentType,
    installed: true,
    enabled,
    path: `/bin/${agentType}`,
    version: '1.0.0',
    latest_version: null,
    origin: 'test',
    install_command: null,
    host_managed: false,
    host_label: null,
    runtime_available: false,
    rtk_available: false,
    rtk_hook_configured: false,
  };
}

function renderHeader(options: {
  sending?: boolean;
  onAgentSwitch?: (agent: AgentType) => void;
  onDiscussionUpdated?: () => void;
  toast?: ToastFn;
} = {}) {
  const sending = options.sending ?? false;
  const onAgentSwitch = options.onAgentSwitch ?? vi.fn<(agent: AgentType) => void>();
  const onDiscussionUpdated = options.onDiscussionUpdated ?? vi.fn();
  const toast = options.toast ?? vi.fn<ToastFn>();
  render(
    <ChatHeader
      discussion={makeDiscussion()}
      projects={[]}
      agents={[
        makeAgent('ClaudeCode'),
        makeAgent('Codex'),
        makeAgent('GeminiCli', false),
      ]}
      showGitPanel={false}
      isMobile={false}
      sending={sending}
      pendingFilesCount={0}
      onRequestTestMode={noop}
      onToggleGitPanel={noop}
      onToggleSettingsPanel={noop}
      onToggleSidebar={noop}
      onDelete={noop}
      onDiscussionUpdated={onDiscussionUpdated}
      onAgentSwitch={onAgentSwitch}
      toast={toast}
      t={t}
    />,
  );
  return { onAgentSwitch, onDiscussionUpdated, toast };
}

describe('ChatHeader — shared agent switcher', () => {
  beforeEach(() => {
    vi.mocked(discussionsApi.update).mockReset().mockResolvedValue(undefined);
    vi.mocked(discussionsApi.nativeAgentMode).mockReset().mockResolvedValue({ disabled: false });
    vi.mocked(discussionsApi.workspaces).mockReset().mockResolvedValue([]);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('keeps the title edit action immediately beside the visible title', () => {
    renderHeader();

    const title = document.querySelector('.disc-chat-header-title-text');
    const edit = screen.getByRole('button', { name: 'disc.editTitle' });
    const id = screen.getByRole('button', { name: 'disc.idPillTooltip' });

    expect(title?.nextElementSibling).toBe(edit);
    expect(edit.nextElementSibling).toBe(id);
    expect(edit.closest('.disc-chat-header-title')).not.toBeNull();
    expect(edit.closest('.disc-chat-header-presence')).toBeNull();
  });

  it('uses the workflow picker and persists a usable agent directly', async () => {
    const { onAgentSwitch } = renderHeader();
    const trigger = screen.getByRole('button', { name: 'disc.switchAgent' });

    expect(trigger).toHaveClass('kr-agent-switch-btn');
    expect(trigger).toHaveTextContent('@claude · disc.targetDiscussionAgent');
    await waitFor(() => expect(trigger).toBeEnabled());
    fireEvent.click(trigger);
    const menu = screen.getByRole('menu');
    expect(menu.parentElement).toBe(document.body);
    expect(screen.getByRole('menuitem', { name: 'Claude Code' })).toBeDisabled();
    expect(screen.getByRole('menuitem', { name: 'Codex' })).toBeEnabled();
    expect(screen.queryByRole('menuitem', { name: 'Gemini CLI' })).toBeNull();

    const codexOption = screen.getByRole('menuitem', { name: 'Codex' });
    fireEvent.mouseDown(codexOption);
    fireEvent.click(codexOption);
    fireEvent.click(codexOption);

    await waitFor(() => {
      expect(discussionsApi.update).toHaveBeenCalledTimes(1);
      expect(discussionsApi.update).toHaveBeenCalledWith(
        'disc-agent-switch',
        { agent: 'Codex' },
      );
      expect(onAgentSwitch).toHaveBeenCalledTimes(1);
      expect(onAgentSwitch).toHaveBeenCalledWith('Codex');
    });
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('is disabled while the discussion is sending', () => {
    renderHeader({ sending: true });
    expect(screen.getByRole('button', { name: 'disc.switchAgent' })).toBeDisabled();
  });

  it('shows declared joined-CLI worktrees in the discussion header', async () => {
    vi.mocked(discussionsApi.workspaces).mockResolvedValue([{
      id: 'workspace-1',
      disc_id: 'disc-agent-switch',
      session_pk: 42,
      session_agent_type: 'Codex',
      task_id: 'task-140',
      task_reference: 'KT-140',
      project_id: 'project-1',
      workspace_path: '/tmp/kronn-kt140',
      canonical_path: '/tmp/kronn-kt140',
      branch: 'feature/kt140',
      head_sha: 'abc123',
      ownership: 'external',
      state: 'attached',
      created_at: '2026-01-01',
      updated_at: '2026-01-01',
    }]);

    renderHeader();

    expect(await screen.findByText('feature/kt140')).toBeInTheDocument();
    expect(screen.getByText('KT-140')).toBeInTheDocument();
    expect(screen.getByText('Codex')).toBeInTheDocument();
  });

  it('keeps the choices open and reports the API error', async () => {
    vi.mocked(discussionsApi.update).mockRejectedValue(new Error('offline'));
    const { toast, onAgentSwitch } = renderHeader();

    const trigger = screen.getByRole('button', { name: 'disc.switchAgent' });
    await waitFor(() => expect(trigger).toBeEnabled());
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Codex' }));

    await waitFor(() => expect(toast).toHaveBeenCalledWith('Error: offline', 'error'));
    expect(onAgentSwitch).not.toHaveBeenCalled();
    expect(screen.getByRole('menu')).toBeInTheDocument();
  });

  it('loads a persistent peer-only mode and can reactivate the native agent', async () => {
    vi.mocked(discussionsApi.nativeAgentMode).mockResolvedValue({ disabled: true });
    const { onDiscussionUpdated } = renderHeader();

    const enable = await screen.findByRole('button', { name: 'disc.nativeAgentEnable' });
    expect(enable).toHaveTextContent('disc.nativeAgentDisabled');
    expect(screen.queryByRole('button', { name: 'disc.switchAgent' })).toBeNull();

    fireEvent.click(enable);
    fireEvent.click(enable);

    await waitFor(() => {
      expect(discussionsApi.update).toHaveBeenCalledTimes(1);
      expect(discussionsApi.update).toHaveBeenCalledWith(
        'disc-agent-switch',
        { no_agent: false },
      );
      expect(onDiscussionUpdated).toHaveBeenCalledTimes(1);
    });
    expect(screen.getByRole('button', { name: 'disc.switchAgent' })).toBeInTheDocument();
  });

  it('keeps the native control disabled until a failed mode read can be retried', async () => {
    vi.useFakeTimers();
    vi.mocked(discussionsApi.nativeAgentMode)
      .mockRejectedValueOnce(new Error('backend restarting'))
      .mockResolvedValueOnce({ disabled: true });
    renderHeader();

    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByRole('button', { name: 'disc.switchAgent' })).toBeDisabled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(screen.getByRole('button', { name: 'disc.nativeAgentEnable' })).toHaveTextContent(
      'disc.nativeAgentDisabled',
    );
    expect(discussionsApi.nativeAgentMode).toHaveBeenCalledTimes(2);
  });

  it('disables the native fallback without changing the configured agent', async () => {
    const { onAgentSwitch } = renderHeader();
    const disable = await screen.findByRole('button', { name: 'disc.nativeAgentDisable' });

    fireEvent.click(disable);
    fireEvent.click(disable);

    await waitFor(() => {
      expect(discussionsApi.update).toHaveBeenCalledTimes(1);
      expect(discussionsApi.update).toHaveBeenCalledWith(
        'disc-agent-switch',
        { no_agent: true },
      );
    });
    expect(screen.getByTestId('disc-native-agent-disabled')).toHaveTextContent(
      'disc.nativeAgentDisabled',
    );
    expect(onAgentSwitch).not.toHaveBeenCalled();
  });
});
