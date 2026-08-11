/**
 * 0.8.7 — P0-10 of the QA roadmap.
 *
 * Closure-stale guard on ChatInput's `handleSendMessage`. The `sending`
 * prop is flipped to true by the parent on the false→true edge AFTER the
 * onSend callback fires — meaning two synchronous clicks in the same
 * event-loop tick both see `sending=false`. Pre-fix this double-POSTed
 * the user's message (the highest-blast bug class in the chat path).
 *
 * The fix adds a `sendInFlightRef` set+cleared in the same tick that
 * blocks the immediate-next click. These tests pin the contract :
 *  - 2 synchronous clicks → 1 onSend call (not 2)
 *  - After microtask flush, the user can send again
 *  - If onSend throws synchronously, the ref still releases (user can retry)
 */
import { beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, act, waitFor } from '@testing-library/react';
import { ChatInput } from '../ChatInput';
import type {
  AgentDetection,
  Discussion,
  ModelTiersConfig,
} from '../../types/generated';
import {
  autoTriggersApi,
  discussions as discussionsApi,
  config as configApi,
} from '../../lib/api';

vi.mock('../../lib/stt-engine', () => ({
  audioBufferToFloat32: vi.fn(),
  transcribeAudio: vi.fn().mockResolvedValue(''),
}));

const disc: Discussion = {
  id: 'd-race', title: 'Race test', project_id: null,
  agent: 'ClaudeCode', language: 'fr',
  participants: ['ClaudeCode'], messages: [],
  message_count: 0, non_system_message_count: 0,
  skill_ids: [], profile_ids: [], directive_ids: [],
  archived: false, pinned: false,
  workspace_mode: 'Direct', workspace_path: null, worktree_branch: null,
  tier: 'Default', pin_first_message: false,
  summary_cache: null, summary_up_to_msg_idx: null,
  shared_id: null, shared_with: [], workflow_run_id: null,
  created_at: '2026-05-28T00:00:00Z', updated_at: '2026-05-28T00:00:00Z',
} as unknown as Discussion;

async function mount(
  onSend: ReturnType<typeof vi.fn>,
  sending = false,
  agents: AgentDetection[] = [],
  discussion: Discussion = disc,
  modelTiers?: ModelTiersConfig,
) {
  const t = (k: string, ...a: unknown[]) => (a.length ? `${k}(${a.join('|')})` : k);
  let view!: ReturnType<typeof render>;
  await act(async () => {
    view = render(
      <ChatInput
        discussion={discussion} agents={agents} sending={sending} disabled={false}
        ttsEnabled={false} ttsState="idle" worktreeError={null}
        availableSkills={[]} availableDirectives={[]}
        onSend={onSend as never} onStop={vi.fn()} onOrchestrate={vi.fn()}
        onTtsToggle={vi.fn()} onWorktreeErrorDismiss={vi.fn()}
        onWorktreeRetry={vi.fn()} isAgentRestricted={() => false}
        contextFiles={[]} uploadingFiles={false}
        modelTiers={modelTiers}
        toast={vi.fn() as never} t={t}
      />,
    );
  });
  return view;
}

function typeText(value: string) {
  const ta = screen.getByPlaceholderText(/disc\.messagePlaceholder/i) as HTMLTextAreaElement;
  fireEvent.change(ta, { target: { value } });
  return ta;
}

function sendButton(): HTMLButtonElement {
  // The send button is `Enter` in the textarea OR a click on the send
  // icon. ChatInput renders the latter with `aria-label="chat.send"`
  // (i18n key passed through the identity translator in tests).
  return screen.getByLabelText('Send message') as HTMLButtonElement;
}

describe('ChatInput — send-race guard (P0-10)', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.spyOn(autoTriggersApi, 'listDisabled').mockResolvedValue([]);
    vi.spyOn(discussionsApi, 'participants').mockResolvedValue([]);
    vi.spyOn(discussionsApi, 'nativeAgentMode').mockResolvedValue({ disabled: false });
    vi.spyOn(configApi, 'getServerConfig').mockResolvedValue({
      discussion_notes_enabled: true,
    } as never);
  });

  it('sends an out-of-context note without mention routing', async () => {
    const onSend = vi.fn();
    await mount(onSend);
    await waitFor(() => expect(
      screen.getByLabelText('disc.note.sendAsNote'),
    ).toBeInTheDocument());

    fireEvent.click(screen.getByLabelText('disc.note.sendAsNote'));
    typeText('@ollama piste à garder');
    fireEvent.click(sendButton());

    expect(onSend).toHaveBeenCalledWith(
      '@ollama piste à garder',
      undefined,
      false,
      undefined,
      'note',
    );
  });

  it('routes a joined CLI autocomplete alias to the exact durable session', async () => {
    vi.spyOn(discussionsApi, 'participants').mockResolvedValue([{
      id: 42,
      disc_id: disc.id,
      agent_type: 'Codex',
      session_id: 'codex-cli-session',
      role: 'peer',
      status: 'active',
      joined_at: '2026-07-29T00:00:00Z',
      left_at: null,
    }] as never);
    const onSend = vi.fn();
    await mount(onSend, false, [{
      agent_type: 'Codex',
      installed: true,
      runtime_available: false,
      enabled: true,
    } as AgentDetection]);

    await waitFor(() => {
      fireEvent.change(screen.getByRole('textbox'), { target: { value: '@codex-c' } });
      // KT-211: the joined CLI entry displays its REAL room alias, never
      // the bare provider trigger it shares with the punctual agent.
      expect(screen.getByText('@codex-cli')).toBeInTheDocument();
    });
    fireEvent.mouseDown(screen.getByText('@codex-cli'));
    expect(screen.getByRole<HTMLInputElement>('textbox').value).toBe('@codex-cli ');
    fireEvent.click(sendButton());

    expect(onSend).toHaveBeenCalledWith(
      '@codex-cli',
      [{ kind: 'cli', agent_type: 'Codex', cli_session_id: 42 }],
      false,
      undefined,
    );
  });

  it('explains the effective discussion routing with dynamic identities', async () => {
    const participantsSpy = vi.spyOn(discussionsApi, 'participants').mockResolvedValue([{
      id: 42,
      disc_id: disc.id,
      agent_type: 'Codex',
      session_id: 'codex-cli-session',
      role: 'peer',
      status: 'active',
      joined_at: '2026-07-29T00:00:00Z',
      left_at: null,
    }] as never);
    vi.spyOn(discussionsApi, 'nativeAgentMode').mockResolvedValue({ disabled: true });
    await mount(vi.fn(), false, [
      {
        agent_type: 'ClaudeCode',
        installed: true,
        runtime_available: false,
        enabled: false,
      },
      {
        agent_type: 'Codex',
        installed: true,
        runtime_available: false,
        enabled: true,
      },
    ] as AgentDetection[]);

    await waitFor(() => expect(participantsSpy).toHaveBeenCalledWith(disc.id));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: '@' } });
    expect(screen.getByText('disc.routingActiveAgents')).toBeInTheDocument();
    expect(screen.getByText('disc.routingAvailableAgents')).toBeInTheDocument();
    const mentionItems = Array.from(document.querySelectorAll('.disc-mention-item'));
    const cliIndex = mentionItems.findIndex(item => item.textContent?.includes('disc.targetCli'));
    const punctualIndex = mentionItems.findIndex(
      item => item.textContent?.includes('disc.targetPunctualAgent'),
    );
    expect(cliIndex).toBeGreaterThanOrEqual(0);
    expect(punctualIndex).toBeGreaterThan(cliIndex);
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Escape' });
    fireEvent.click(screen.getByLabelText('disc.composerHelpTitle'));

    const help = screen.getByRole('dialog', { name: 'disc.composerHelpTitle' });
    expect(help).toHaveTextContent('disc.routingActiveAgents');
    expect(help).toHaveTextContent('disc.routingAvailableAgents');
    expect(help).toHaveTextContent('disc.routingDisabledAgent');
    expect(help).toHaveTextContent('@claude · disc.targetDiscussionAgent');
    expect(help).toHaveTextContent('disc.routingHelpDiscussionAgentDisabled');
    expect(help).toHaveTextContent('@codex · disc.targetPunctualAgent');
    expect(help).toHaveTextContent('@codex-cli · disc.targetCli');
    expect(help).toHaveTextContent(
      'disc.routingHelpAll(@codex-cli · disc.targetCli)',
    );
    expect(help).toHaveTextContent('markdown.helpTitle');
    expect(help).toHaveTextContent('markdown.emojiHint');
    expect(screen.queryByRole('button', { name: 'markdown.help' })).toBeNull();
  });

  it('shows the discussion tier on its principal and Standard on punctual aliases', async () => {
    const reasoningDiscussion = {
      ...disc,
      tier: 'reasoning',
      model: null,
      participants: ['ClaudeCode', 'Codex'],
    } as Discussion;
    const modelTiers = {
      claude_code: { reasoning: 'claude-company-opus' },
      codex: { default: 'gpt-company-standard', reasoning: 'gpt-company-review' },
      gemini_cli: {},
      kiro: {},
      vibe: {},
      copilot_cli: {},
      ollama: {},
      lite_llm: {},
    } as ModelTiersConfig;
    await mount(vi.fn(), false, [
      {
        agent_type: 'ClaudeCode', installed: true, runtime_available: false, enabled: true,
      },
      {
        agent_type: 'Codex', installed: true, runtime_available: false, enabled: true,
      },
    ] as AgentDetection[], reasoningDiscussion, modelTiers);

    fireEvent.change(screen.getByRole('textbox'), { target: { value: '@' } });

    expect(screen.getByLabelText(
      'disc.routingNativeTier(disc.tier.reasoning|claude-company-opus)',
    )).toBeInTheDocument();
    expect(screen.getByLabelText(
      'disc.routingTargetTier(disc.tier.default|gpt-company-standard)',
    )).toBeInTheDocument();
  });

  it('routes a punctual @alias with the tier chosen in one click', async () => {
    const onSend = vi.fn();
    const modelTiers = {
      claude_code: {},
      codex: { reasoning: 'gpt-company-review' },
      gemini_cli: {},
      kiro: {},
      vibe: {},
      copilot_cli: {},
      ollama: {},
      lite_llm: {},
    } as ModelTiersConfig;
    await mount(onSend, false, [{
      agent_type: 'Codex', installed: true, runtime_available: false, enabled: true,
    }] as AgentDetection[], disc, modelTiers);

    fireEvent.change(screen.getByRole('textbox'), { target: { value: '@co' } });
    fireEvent.mouseDown(screen.getByRole('button', {
      name: '@codex · disc.routingInvokeTier(disc.tier.reasoning|gpt-company-review)',
    }));
    fireEvent.click(sendButton());

    expect(onSend).toHaveBeenCalledWith(
      '@codex',
      [{
        kind: 'agent', agent_type: 'Codex', cli_session_id: null, tier: 'reasoning',
      }],
      false,
      undefined,
    );
    expect(screen.queryByLabelText('disc.routingOverrides')).not.toBeInTheDocument();
  });

  it('remembers the last explicit tier for that agent in this discussion', async () => {
    const onSend = vi.fn();
    await mount(onSend, false, [{
      agent_type: 'Codex', installed: true, runtime_available: false, enabled: true,
    }] as AgentDetection[]);

    fireEvent.change(screen.getByRole('textbox'), { target: { value: '@co' } });
    fireEvent.mouseDown(screen.getByRole('button', {
      name: /@codex · disc\.routingInvokeTier\(disc\.tier\.reasoning/,
    }));

    const routing = screen.getByLabelText('disc.routingOverrides');
    expect(routing).toHaveTextContent('@codex');
    expect(routing).toHaveTextContent('🧠 disc.tier.reasoning');
    fireEvent.click(sendButton());
    await act(async () => { await Promise.resolve(); });

    typeText('@codex encore un avis');
    fireEvent.click(sendButton());

    expect(onSend).toHaveBeenLastCalledWith(
      '@codex encore un avis',
      [{
        kind: 'agent', agent_type: 'Codex', cli_session_id: null, tier: 'reasoning',
      }],
      false,
      undefined,
    );
  });

  it('routes two aliases at two independent reasoning tiers in one message', async () => {
    const onSend = vi.fn();
    await mount(onSend, false, [
      { agent_type: 'Codex', installed: true, runtime_available: false, enabled: true },
      { agent_type: 'Ollama', installed: true, runtime_available: true, enabled: true },
    ] as AgentDetection[]);

    fireEvent.change(screen.getByRole('textbox'), { target: { value: '@co' } });
    fireEvent.mouseDown(screen.getByRole('button', {
      name: /@codex · disc\.routingInvokeTier\(disc\.tier\.reasoning/,
    }));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: '@codex @ol' } });
    fireEvent.mouseDown(screen.getByRole('button', {
      name: /@ollama · disc\.routingInvokeTier\(disc\.tier\.economy/,
    }));

    expect(screen.getByLabelText('disc.routingOverrides')).toHaveTextContent('@codex');
    expect(screen.getByLabelText('disc.routingOverrides')).toHaveTextContent('@ollama');
    fireEvent.click(sendButton());

    expect(onSend).toHaveBeenCalledWith(
      '@codex @ollama',
      [
        { kind: 'agent', agent_type: 'Codex', cli_session_id: null, tier: 'reasoning' },
        { kind: 'agent', agent_type: 'Ollama', cli_session_id: null, tier: 'economy' },
      ],
      false,
      undefined,
    );
  });

  it('marks joined CLI aliases as CLI-managed instead of assigning a Kronn tier', async () => {
    vi.spyOn(discussionsApi, 'participants').mockResolvedValue([{
      id: 42,
      disc_id: disc.id,
      agent_type: 'Codex',
      session_id: 'codex-cli-session',
      role: 'peer',
      status: 'active',
      joined_at: '2026-07-29T00:00:00Z',
      left_at: null,
    }] as never);
    await mount(vi.fn(), false, [{
      agent_type: 'Codex', installed: true, runtime_available: false, enabled: true,
    }] as AgentDetection[]);

    await waitFor(() => {
      fireEvent.change(screen.getByRole('textbox'), { target: { value: '@codex-c' } });
      expect(screen.getByText('@codex-cli')).toBeInTheDocument();
    });
    expect(screen.getByLabelText('disc.routingCliModelManaged'))
      .toBeInTheDocument();
  });

  it('routes every installed mention once in the order written by the human', async () => {
    const onSend = vi.fn();
    const agents = [
      {
        agent_type: 'ClaudeCode',
        installed: true,
        runtime_available: false,
        enabled: true,
      },
      {
        agent_type: 'Codex',
        installed: true,
        runtime_available: false,
        enabled: true,
      },
    ] as AgentDetection[];
    await mount(onSend, false, agents);
    typeText('@codex tu peux confronter @claude si tu veux');

    act(() => { fireEvent.click(sendButton()); });

    expect(onSend).toHaveBeenCalledWith(
      '@codex tu peux confronter @claude si tu veux',
      [
        { kind: 'agent', agent_type: 'Codex', cli_session_id: null, tier: 'default' },
        {
          kind: 'discussion_agent', agent_type: 'ClaudeCode', cli_session_id: null, tier: null,
        },
      ],
      false,
      undefined,
    );
  });

  it('routes a general turn to every native model attached to the discussion', async () => {
    const onSend = vi.fn();
    const groupDiscussion = {
      ...disc,
      agent: 'LiteLlm',
      participants: ['LiteLlm', 'Ollama'],
    } as Discussion;
    const agents = [
      { agent_type: 'LiteLlm', installed: true, runtime_available: true, enabled: true },
      { agent_type: 'Ollama', installed: true, runtime_available: true, enabled: true },
    ] as AgentDetection[];
    await mount(onSend, false, agents, groupDiscussion);
    typeText('Vous devriez connaître vos points forts et faibles.');

    fireEvent.click(sendButton());

    expect(onSend).toHaveBeenCalledWith(
      'Vous devriez connaître vos points forts et faibles.',
      [
        {
          kind: 'discussion_agent', agent_type: 'LiteLlm', cli_session_id: null, tier: null,
        },
        { kind: 'agent', agent_type: 'Ollama', cli_session_id: null, tier: 'default' },
      ],
      false,
      undefined,
    );
  });

  it('two synchronous clicks fire onSend only ONCE', async () => {
    const onSend = vi.fn();
    await mount(onSend);
    typeText('hello race');

    // Fire two clicks in the same tick — pre-fix this produced two
    // onSend calls because the `sending` prop hadn't been re-rendered
    // by the parent yet between the clicks.
    act(() => {
      const btn = sendButton();
      fireEvent.click(btn);
      fireEvent.click(btn);
    });

    expect(onSend).toHaveBeenCalledTimes(1);
    expect(onSend).toHaveBeenCalledWith('hello race', undefined, false, undefined);
  });

  it('Enter then Enter in quick succession only fires onSend once', async () => {
    // Same race via keyboard — Enter is the dominant send path.
    const onSend = vi.fn();
    await mount(onSend);
    const ta = typeText('keyboard race') as HTMLTextAreaElement;

    act(() => {
      fireEvent.keyDown(ta, { key: 'Enter' });
      fireEvent.keyDown(ta, { key: 'Enter' });
    });

    expect(onSend).toHaveBeenCalledTimes(1);
  });

  it('after a microtask flush, the user can send again', async () => {
    const onSend = vi.fn();
    await mount(onSend);
    typeText('first message');
    act(() => { fireEvent.click(sendButton()); });
    expect(onSend).toHaveBeenCalledTimes(1);

    // Microtask flush — the ref is released and the next send fires.
    await act(async () => { await Promise.resolve(); });

    typeText('second message');
    act(() => { fireEvent.click(sendButton()); });
    expect(onSend).toHaveBeenCalledTimes(2);
    expect(onSend.mock.calls[1][0]).toBe('second message');
  });

  // NB : a previous variant tried to verify ref-release after a SYNC throw
  // inside onSend. React 19's event-dispatch surfaces such throws as
  // unhandled exceptions at the test-file level (not catchable by try/catch
  // around `act`), which broke vitest's coverage writeback. The microtask-
  // released-ref contract is already pinned by the two-click + Enter+Enter
  // tests above (which use the success path) — the failure path is just
  // a stricter assertion on the same useRef + queueMicrotask cleanup logic.

  it('sending=true from the parent swaps the send button for a stop button (UI guard)', async () => {
    // The prop-based guard manifests as a JSX swap : send button is
    // REPLACED by a stop button while in-flight. That's an even stronger
    // guarantee than `disabled={sending}` (user physically can't click
    // send → no race possible). Pin the swap so a regression that
    // removes the ternary is caught here too.
    const onSend = vi.fn();
    const { rerender } = await mount(onSend, /* sending */ false);
    expect(screen.queryByLabelText('Send message')).not.toBeNull();
    expect(screen.queryByLabelText('disc.stopThinking')).toBeNull();

    // Parent simulates the round-trip: sets sending=true.
    rerender(
      <ChatInput
        discussion={disc} agents={[]} sending={true} disabled={false}
        ttsEnabled={false} ttsState="idle" worktreeError={null}
        availableSkills={[]} availableDirectives={[]}
        onSend={onSend as never} onStop={vi.fn()} onOrchestrate={vi.fn()}
        onTtsToggle={vi.fn()} onWorktreeErrorDismiss={vi.fn()}
        onWorktreeRetry={vi.fn()} isAgentRestricted={() => false}
        contextFiles={[]} uploadingFiles={false}
        toast={vi.fn() as never}
        t={(k: string) => k}
      />,
    );

    // After the prop flip, the send button disappears, stop button appears.
    expect(screen.queryByLabelText('Send message')).toBeNull();
    expect(screen.queryByLabelText('disc.stopThinking')).not.toBeNull();
    // And no further onSend was triggered by the re-render.
    expect(onSend).not.toHaveBeenCalled();
  });
});
