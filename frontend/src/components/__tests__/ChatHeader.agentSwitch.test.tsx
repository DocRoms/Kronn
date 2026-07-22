import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
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
  toast?: ToastFn;
} = {}) {
  const sending = options.sending ?? false;
  const onAgentSwitch = options.onAgentSwitch ?? vi.fn<(agent: AgentType) => void>();
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
      availableSkills={[]}
      availableProfiles={[]}
      availableDirectives={[]}
      mcpConfigs={[]}
      mcpIncompatibilities={[]}
      showGitPanel={false}
      isMobile={false}
      sending={sending}
      pendingFilesCount={0}
      onRequestTestMode={noop}
      onToggleGitPanel={noop}
      onToggleSidebar={noop}
      onDelete={noop}
      onDiscussionUpdated={noop}
      onAgentSwitch={onAgentSwitch}
      contacts={[]}
      onShare={noop}
      toast={toast}
      t={t}
    />,
  );
  return { onAgentSwitch, toast };
}

describe('ChatHeader — shared agent switcher', () => {
  beforeEach(() => {
    vi.mocked(discussionsApi.update).mockReset().mockResolvedValue(undefined);
  });

  it('uses the workflow picker and persists a usable agent directly', async () => {
    const { onAgentSwitch } = renderHeader();
    const trigger = screen.getByRole('button', { name: 'disc.switchAgent' });

    expect(trigger).toHaveClass('kr-agent-switch-btn');
    fireEvent.click(trigger);
    expect(screen.getByRole('menuitem', { name: 'Claude Code' })).toBeDisabled();
    expect(screen.getByRole('menuitem', { name: 'Codex' })).toBeEnabled();
    expect(screen.queryByRole('menuitem', { name: 'Gemini CLI' })).toBeNull();

    const codexOption = screen.getByRole('menuitem', { name: 'Codex' });
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

  it('keeps the choices open and reports the API error', async () => {
    vi.mocked(discussionsApi.update).mockRejectedValue(new Error('offline'));
    const { toast, onAgentSwitch } = renderHeader();

    fireEvent.click(screen.getByRole('button', { name: 'disc.switchAgent' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Codex' }));

    await waitFor(() => expect(toast).toHaveBeenCalledWith('Error: offline', 'error'));
    expect(onAgentSwitch).not.toHaveBeenCalled();
    expect(screen.getByRole('menu')).toBeInTheDocument();
  });
});
