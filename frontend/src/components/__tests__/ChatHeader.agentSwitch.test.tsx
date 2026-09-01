import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../test/apiMock';

vi.mock('../../lib/api', () => buildApiMock());
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key }),
}));

import { discussions as discussionsApi } from '../../lib/api';
import { ChatHeader } from '../ChatHeader';
import type { AgentDetection, AgentType, Discussion, ModelTiersConfig } from '../../types/generated';
import type { ToastFn } from '../../hooks/useToast';
import type { ExternalApiConnectionView } from '../../lib/api';

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
  modelTiers?: ModelTiersConfig;
  discussion?: Discussion;
  externalConnections?: ExternalApiConnectionView[];
} = {}) {
  const sending = options.sending ?? false;
  const onAgentSwitch = options.onAgentSwitch ?? vi.fn<(agent: AgentType) => void>();
  const onDiscussionUpdated = options.onDiscussionUpdated ?? vi.fn();
  const toast = options.toast ?? vi.fn<ToastFn>();
  render(
    <ChatHeader
      discussion={options.discussion ?? makeDiscussion()}
      projects={[]}
      agents={[
        makeAgent('ClaudeCode'),
        makeAgent('Codex'),
        makeAgent('GeminiCli', false),
      ]}
      modelTiers={options.modelTiers}
      externalConnections={options.externalConnections}
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

    expect(document.querySelector('.disc-chat-header')).toHaveClass('collection-detail-header');
    const title = document.querySelector('.disc-chat-header-title-text');
    const edit = screen.getByRole('button', { name: 'disc.editTitle' });
    const id = screen.getByRole('button', { name: 'disc.idPillTooltip' });

    expect(title?.nextElementSibling).toBe(edit);
    expect(edit.nextElementSibling).toBe(id);
    expect(edit.closest('.disc-chat-header-title')).not.toBeNull();
    expect(edit.closest('.disc-chat-header-presence')).toBeNull();
  });

  it('renders the durable OpenRouter alias instead of the Custom wire type', async () => {
    const discussion = makeDiscussion() as Discussion & {
      message_targets: Record<string, Array<{
        kind: 'discussion_agent';
        agent_type: 'Custom';
        connection_id: string;
        tier: 'default';
      }>>;
    };
    discussion.agent = 'Custom';
    discussion.messages = [{
      id: 'initial-message',
      role: 'User',
      channel: 'main',
      content: '@openrouter translate this',
      agent_type: null,
      timestamp: '2026-08-30T12:00:00Z',
      tokens_used: 0,
      auth_mode: null,
    }];
    discussion.message_targets = {
      'initial-message': [{
        kind: 'discussion_agent',
        agent_type: 'Custom',
        connection_id: 'conn-openrouter',
        tier: 'default',
      }],
    };
    renderHeader({
      discussion,
      externalConnections: [{
        id: 'conn-openrouter',
        display_name: 'OpenRouter',
        mention_alias: 'openrouter',
        endpoint: 'https://openrouter.ai/api/v1',
        credential_slug: 'openrouter',
        origin_preset: 'open_router',
        economy_model: 'qwen/qwen3.8-flash',
        default_model: 'z-ai/glm-5.3',
        reasoning_model: 'z-ai/glm-5.3',
        created_at: '2026-08-30T12:00:00Z',
        updated_at: '2026-08-30T12:00:00Z',
        has_credential: true,
      }],
    });

    const trigger = screen.getByRole('button', { name: 'disc.switchAgentAndTier' });
    await waitFor(() => expect(trigger).toBeEnabled());
    expect(trigger).toHaveTextContent('@openrouter');
    expect(trigger).not.toHaveTextContent('Custom');
  });

  it('persists an agent and AI mode together from the quick picker', async () => {
    const { onAgentSwitch } = renderHeader();
    const trigger = screen.getByRole('button', { name: 'disc.switchAgentAndTier' });

    expect(trigger).toHaveClass('kr-agent-switch-btn');
    expect(trigger).toHaveTextContent('@claude');
    expect(trigger).toHaveTextContent('disc.targetDiscussionAgent');
    expect(trigger).toHaveTextContent('🎯');
    await waitFor(() => expect(trigger).toBeEnabled());
    fireEvent.click(trigger);
    const menu = screen.getByRole('menu');
    expect(menu.parentElement).toBe(document.body);
    expect(screen.getByRole('menuitem', { name: 'Claude Code · disc.tier.default' })).toBeDisabled();
    expect(screen.getByRole('menuitem', { name: 'Codex · disc.tier.reasoning' })).toBeEnabled();
    expect(screen.queryByRole('group', { name: 'Gemini CLI' })).toBeNull();

    const codexOption = screen.getByRole('menuitem', { name: 'Codex · disc.tier.reasoning' });
    fireEvent.mouseDown(codexOption);
    fireEvent.click(codexOption);
    fireEvent.click(codexOption);

    await waitFor(() => {
      expect(discussionsApi.update).toHaveBeenCalledTimes(1);
      expect(discussionsApi.update).toHaveBeenCalledWith(
        'disc-agent-switch',
        { agent: 'Codex', tier: 'reasoning' },
      );
      expect(onAgentSwitch).toHaveBeenCalledTimes(1);
      expect(onAgentSwitch).toHaveBeenCalledWith('Codex');
    });
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('changes only the AI mode without forcing an agent switch', async () => {
    const { onAgentSwitch } = renderHeader();
    const trigger = screen.getByRole('button', { name: 'disc.switchAgentAndTier' });
    await waitFor(() => expect(trigger).toBeEnabled());
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('menuitem', {
      name: 'Claude Code · disc.tier.economy',
    }));

    await waitFor(() => {
      expect(discussionsApi.update).toHaveBeenCalledWith(
        'disc-agent-switch',
        { agent: 'ClaudeCode', tier: 'economy' },
      );
      expect(onAgentSwitch).toHaveBeenCalledWith('ClaudeCode');
    });
  });

  it('shows the resolved concrete model in hover titles', async () => {
    const blank = { economy: null, default: null, reasoning: null };
    renderHeader({
      modelTiers: {
        claude_code: { ...blank },
        codex: { ...blank, reasoning: 'gpt-company-review' },
        open_code: { ...blank },
        gemini_cli: { ...blank },
        kiro: { ...blank },
        vibe: { ...blank },
        copilot_cli: { ...blank },
        ollama: { ...blank },
        lite_llm: { ...blank },
        nvidia: { ...blank },
      },
    });
    const trigger = screen.getByRole('button', { name: 'disc.switchAgentAndTier' });
    await waitFor(() => expect(trigger).toBeEnabled());
    expect(trigger).toHaveAttribute(
      'title',
      'disc.switchAgentAndTier · disc.tier.default · sonnet',
    );

    fireEvent.click(trigger);
    expect(screen.getByRole('menuitem', {
      name: 'Codex · disc.tier.reasoning',
    })).toHaveAttribute('title', 'disc.tier.reasoning · gpt-company-review');
  });

  it('is disabled while the discussion is sending', () => {
    renderHeader({ sending: true });
    expect(screen.getByRole('button', { name: 'disc.switchAgentAndTier' })).toBeDisabled();
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
      parent_discussion_id: null,
      base_sha: null,
      task_execution_id: null,
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

    const trigger = screen.getByRole('button', { name: 'disc.switchAgentAndTier' });
    await waitFor(() => expect(trigger).toBeEnabled());
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Codex · disc.tier.default' }));

    await waitFor(() => expect(toast).toHaveBeenCalledWith('Error: offline', 'error'));
    expect(onAgentSwitch).not.toHaveBeenCalled();
    expect(screen.getByRole('menu')).toBeInTheDocument();
  });

  it('loads a persistent peer-only mode and can reactivate the native agent', async () => {
    vi.mocked(discussionsApi.nativeAgentMode).mockResolvedValue({ disabled: true });
    const { onDiscussionUpdated } = renderHeader();

    const enable = await screen.findByRole('button', { name: 'disc.nativeAgentEnable' });
    expect(enable).toHaveTextContent('disc.nativeAgentDisabled');
    expect(screen.queryByRole('button', { name: 'disc.switchAgentAndTier' })).toBeNull();

    await act(async () => {
      fireEvent.click(enable);
      fireEvent.click(enable);
    });

    await waitFor(() => {
      expect(discussionsApi.update).toHaveBeenCalledTimes(1);
      expect(discussionsApi.update).toHaveBeenCalledWith(
        'disc-agent-switch',
        { no_agent: false },
      );
      expect(onDiscussionUpdated).toHaveBeenCalledTimes(1);
    });
    expect(screen.getByRole('button', { name: 'disc.switchAgentAndTier' })).toBeInTheDocument();
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
    expect(screen.getByRole('button', { name: 'disc.switchAgentAndTier' })).toBeDisabled();

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
    await waitFor(() => expect(disable).toBeEnabled());

    await act(async () => {
      fireEvent.click(disable);
      fireEvent.click(disable);
    });

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
