import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { readFileSync } from 'node:fs';
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
    summary_strategy: 'OnDemand',
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
  onOpenDetails?: () => void;
  settingsPanelOpen?: boolean;
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
      agents={[
        makeAgent('ClaudeCode'),
        makeAgent('Codex'),
        makeAgent('GeminiCli', false),
      ]}
      modelTiers={options.modelTiers}
      externalConnections={options.externalConnections}
      isMobile={false}
      sending={sending}
      onOpenDetails={options.onOpenDetails ?? noop}
      settingsPanelOpen={options.settingsPanelOpen}
      onToggleSidebar={noop}
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

    expect(title?.nextElementSibling).toBe(edit);
    expect(edit.closest('.disc-chat-header-title')).not.toBeNull();
    expect(edit.closest('.disc-chat-header-invite')).toBeNull();
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
    const picker = screen.getByRole('dialog', { name: 'disc.switchAgentAndTier' });
    expect(picker.parentElement).toBe(document.body);
    expect(picker).toContainElement(menu);
    expect(menu).not.toContainElement(screen.getByRole('searchbox', { name: 'agentPicker.search' }));
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
        { agent: 'Codex', tier: 'reasoning', connection_id: null },
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
        { agent: 'ClaudeCode', tier: 'economy', connection_id: null },
      );
      expect(onAgentSwitch).toHaveBeenCalledWith('ClaudeCode');
    });
  });

  it('shows the resolved concrete model in hover titles', async () => {
    const blank = { economy: null, default: null, reasoning: null };
    renderHeader({
      modelTiers: {
        claude_code: { ...blank, default: 'sonnet' },
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

  it('shows a saved discussion model before the catalogue is opened', async () => {
    renderHeader({ discussion: { ...makeDiscussion(), model: 'explicit-disc-model' } });
    const trigger = screen.getByRole('button', { name: 'disc.switchAgentAndTier' });
    await waitFor(() => expect(trigger).toBeEnabled());
    expect(trigger).toHaveAttribute('title', 'disc.switchAgentAndTier · disc.tier.default · explicit-disc-model');
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
  /// KT-581 — the header was reading as three rows. KT-1344 — it is two now:
  /// the third row was a fold repeating the side panel, so "Details" opens that
  /// panel instead. These pin the placement, because a later change that
  /// quietly puts a control back on its own row is exactly the regression the
  /// rework was for.
  describe('two-row layout', () => {
    it('puts the invite on the title row, not on a row of its own', () => {
      renderHeader();
      const invite = document.querySelector('.disc-participants-invite-btn');
      expect(invite).not.toBeNull();
      expect(invite!.closest('.disc-chat-header-top')).not.toBeNull();
    });

    it('pairs Invite (green, primary) and Details side by side, Details at the far right (KT-1530)', () => {
      renderHeader();
      const group = document.querySelector('.disc-chat-header-top .disc-chat-header-buttons')!;
      const invite = document.querySelector('.disc-participants-invite-btn')!;
      const details = screen.getByTestId('disc-header-details-btn');
      expect([...group.children]).toEqual([invite, details]);
      expect(invite).toHaveClass('disc-chat-header-btn', 'disc-chat-header-btn--primary');
      expect(details).toHaveClass('disc-chat-header-btn');
      expect(details).not.toHaveClass('disc-chat-header-btn--primary');
    });

    it('gives the agent on/off control the first position on row 2', async () => {
      renderHeader();
      const sub = document.querySelector('.disc-chat-header-sub')!;
      await waitFor(() => {
        expect(sub.firstElementChild?.classList.contains('disc-native-agent-control')).toBe(true);
      });
    });

    it('has one Details button, on the title row, with the open-side-panel icon', () => {
      renderHeader();
      const buttons = screen.getAllByTestId('disc-header-details-btn');
      expect(buttons).toHaveLength(1);
      const button = buttons[0];
      expect(button.closest('.disc-chat-header-top')).not.toBeNull();
      expect(button.closest('.disc-chat-header-sub')).toBeNull();
      expect(button).toHaveTextContent('disc.headerDetails');
      expect(button.querySelector('svg.lucide-panel-right-open')).not.toBeNull();
    });

    it('opens the side panel instead of unfolding a third row', () => {
      const onOpenDetails = vi.fn();
      renderHeader({ onOpenDetails });
      fireEvent.click(screen.getByTestId('disc-header-details-btn'));
      expect(onOpenDetails).toHaveBeenCalledTimes(1);
      expect(document.querySelector('.disc-chat-header-details')).toBeNull();
      expect(screen.queryByTestId('disc-header-details-toggle')).toBeNull();
    });

    it('reads as expanded while the panel is showing', () => {
      renderHeader({ settingsPanelOpen: true });
      expect(screen.getByTestId('disc-header-details-btn')).toHaveAttribute('aria-expanded', 'true');
    });

    it('keeps the id next to the title, and the other details out of the header', () => {
      renderHeader();
      const pill = document.querySelector('.disc-id-pill');
      expect(pill).not.toBeNull();
      expect(pill!.closest('.disc-chat-header-title')).not.toBeNull();
      expect(pill).toHaveAttribute('data-tour-id', 'discussion-id-pill');
      expect(document.querySelector('.disc-chat-context-project')).toBeNull();
      expect(document.querySelector('.disc-worktree-badge')).toBeNull();
    });

    it('gives both header buttons one size, the primary one in the green success tokens', () => {
      const css = readFileSync('src/pages/DiscussionsPage.css', 'utf8');
      const rule = (selector: string) => css.match(new RegExp(`${selector.replace(/[.\\-]/g, '\\$&')}\\s*\\{([^}]*)\\}`))?.[1] ?? '';
      const base = rule('.disc-chat-header-btn');
      expect(base).toContain('min-height: 30px');
      expect(base).toContain('border: 1px solid');
      // Not a per-button size: the details button adds colours only.
      expect(rule('.disc-chat-header-details-btn')).toBe('');
      const primary = rule('.disc-chat-header-btn--primary');
      expect(primary).toContain('var(--kr-success)');
      expect(primary).toContain('var(--kr-on-success)');
      expect(primary).not.toMatch(/#[0-9a-f]{3,6}\b|rgb\(/i);
    });
  });
});
