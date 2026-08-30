/**
 * MessageBubble — broad rendering / branch coverage suite.
 *
 * The pre-existing MessageBubble specs each pin one feature (emoji,
 * kronn-tool badge, lint pill, seed toggle, validation CTA). This file
 * fills the gap on the *structural* render branches that were otherwise
 * only exercised transitively: role-based bubble variants, author
 * pseudo/avatar, the agent + footer copy buttons, TTS button states,
 * timestamp / token / auth-mode / duration / model-tier / full-access
 * footer chips, edit mode, the auth-error & partial-response CTAs, the
 * summary-cached system message + expand toggle, and the last-user /
 * last-agent edit & retry affordances.
 *
 * Plus the exported MarkdownContent helpers: CopyableBlock (pre + table
 * copy paths through extractText) and the doc-data / mermaid fence
 * branches that only fire when a discussionId is supplied.
 *
 * Convention: mirrors the kronnTool / lintPill specs — I18nProvider
 * wrapper, mocked boot-config call, identity translator `t`, a base
 * props bag spread onto every render. No real person names.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, within, waitFor, act } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { LocalIdentityProvider } from '../../lib/LocalIdentityContext';

// Mock the boot config call so I18nProvider doesn't try to fetch.
vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return {
    ...real,
    config: {
      getUiLanguage: vi.fn().mockResolvedValue('fr'),
      // KT-47 — the @user chip resolves the local identity through this call.
      getServerConfig: vi.fn().mockResolvedValue({ pseudo: null, avatar_email: null }),
      getAgentAccess: vi.fn().mockResolvedValue(null),
    },
  };
});

import {
  MessageBubble,
  MarkdownContent,
  type MessageBubbleProps,
} from '../MessageBubble';
import { parseModelErrorEvent } from '../../lib/modelErrorEvent';
import { DELETED_MESSAGE_MARKER } from '../../lib/messageContent';
import { config as configApi } from '../../lib/api';
import type { DiscussionMessage, MessageRole } from '../../types/generated';

function makeMessage(overrides: Partial<DiscussionMessage> = {}): DiscussionMessage {
  return {
    id: 'msg-test',
    role: 'Agent',
    channel: 'main',
    content: 'Some answer.',
    agent_type: 'ClaudeCode',
    timestamp: '2026-05-29T10:30:00.000Z',
    tokens_used: 0,
    auth_mode: null,
    model_tier: null,
    author_pseudo: null,
    author_avatar_email: null,
    lint_report: null,
    ...overrides,
  };
}

const baseProps: Omit<MessageBubbleProps, 'msg'> = {
  idx: 0,
  isLastUser: false,
  isLastAgent: false,
  isEditing: false,
  isCopied: false,
  isTtsActive: false,
  ttsState: 'idle' as const,
  isExpandedSummary: false,
  prevUserTs: null,
  defaultAgent: 'ClaudeCode' as const,
  summaryCache: null as string | null,
  language: 'fr',
  sending: false,
  editingText: '',
  hasFullAccess: false,
  onCopy: () => {},
  onTts: () => {},
  onEditStart: () => {},
  onEditCancel: () => {},
  onEditSubmit: () => {},
  onEditTextChange: () => {},
  onRetry: () => {},
  onExpandSummary: () => {},
  onNavigate: () => {},
  discussionId: 'disc-test',
  projectId: null,
  t: (key: string) => key,
};

function renderBubble(
  msg: DiscussionMessage,
  props: Partial<typeof baseProps> = {},
) {
  return render(
    <I18nProvider>
      <MessageBubble {...baseProps} {...props} msg={msg} />
    </I18nProvider>,
  );
}

describe('MessageBubble — role-based bubble variant', () => {
  it('tags a User message bubble with data-role="user"', () => {
    const { container } = renderBubble(makeMessage({ role: 'User', content: 'hi' }));
    const bubble = container.querySelector('.disc-msg-bubble');
    expect(bubble?.getAttribute('data-role')).toBe('user');
    expect(bubble?.hasAttribute('data-variant')).toBe(false);
  });

  it('tags an Agent message bubble with data-role="agent"', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent' }));
    expect(container.querySelector('.disc-msg-bubble')?.getAttribute('data-role')).toBe('agent');
  });

  it('renders orchestrator turns as a collapsed automated orchestration variant', () => {
    const { container } = renderBubble(makeMessage({
      role: 'User',
      agent_type: null,
      author_pseudo: 'Orchestrateur',
      content: '**Execution KT-42 completed**\n\nTechnical delivery details',
    }));

    expect(container.querySelector('.disc-msg-row')).toHaveAttribute('data-role', 'orchestrator');
    const bubble = container.querySelector('.disc-msg-bubble');
    expect(bubble).toHaveAttribute('data-role', 'orchestrator');
    expect(bubble).toHaveAttribute('data-variant', 'orchestration');
    expect(bubble).toHaveClass('disc-msg-bubble-full');
    expect(screen.getByText('disc.orchestrator')).toBeInTheDocument();
    expect(screen.getByText(/disc\.orchestratorKind/)).toBeInTheDocument();
    expect(screen.queryByText(/· humain/)).toBeNull();
    expect(screen.getByText('Execution KT-42 completed')).toBeInTheDocument();
    expect(screen.queryByText('Technical delivery details')).toBeNull();

    const toggle = screen.getByRole('button', { name: 'disc.orchestratorShowDetails' });
    expect(toggle.closest('.disc-msg-header-row')).not.toBeNull();
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(toggle);
    expect(screen.getByText('Technical delivery details')).toBeInTheDocument();
    const expandedToggle = screen.getByRole('button', { name: 'disc.orchestratorHideDetails' });
    expect(expandedToggle).toBe(toggle);
    expect(expandedToggle).toHaveAttribute('aria-expanded', 'true');
  });

  it('renders deleted content as a compact tombstone without message actions', () => {
    const onDelete = vi.fn();
    const onReply = vi.fn();
    const { container } = renderBubble(makeMessage({
      role: 'User',
      content: `${DELETED_MESSAGE_MARKER} hidden payload`,
      author_pseudo: 'Peer',
    }), { onDelete, onReply });

    expect(container.querySelector('.disc-msg-row')).toHaveAttribute('data-role', 'deleted');
    expect(container.querySelector('.disc-msg-row')).toHaveAttribute('data-former-role', 'user');
    expect(screen.getByText('disc.deletedMessage')).toBeInTheDocument();
    expect(screen.queryByText(/hidden payload/)).toBeNull();
    expect(screen.queryByRole('button', { name: 'disc.deleteMessage' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'disc.reply' })).toBeNull();
  });

  it('exposes an accessible reply action on human and agent messages', () => {
    const onReply = vi.fn();
    const message = makeMessage({ role: 'Agent' });
    renderBubble(message, { onReply });

    fireEvent.click(screen.getByRole('button', { name: 'disc.reply' }));

    expect(onReply).toHaveBeenCalledWith(message);
  });

  it('renders a durable reply header and navigates to the source message', () => {
    const onReplyNavigate = vi.fn();
    const source = makeMessage({
      id: '12345678-source',
      role: 'Agent',
      agent_type: 'Codex',
      content: 'Original answer',
    });
    renderBubble(
      makeMessage({ id: 'reply', reply_to_message_id: source.id }),
      { replyTarget: source, onReplyNavigate },
    );

    const header = screen.getByTitle('disc.openReplyTarget');
    expect(header).toHaveTextContent('#12345678');
    fireEvent.click(header);
    expect(onReplyNavigate).toHaveBeenCalledWith(source.id);
  });

  it('keeps a missing reply target visible without broken navigation', () => {
    renderBubble(
      makeMessage({
        id: 'reply',
        reply_to_message_id: '87654321-missing',
      }),
      { replyTarget: null, onReplyNavigate: vi.fn() },
    );

    const header = screen.getByTitle('disc.replyTargetMissing');
    expect(header).toBeDisabled();
    expect(header).toHaveTextContent('#87654321');
  });

  it('shows human and agent replies as backlinks without exposing native tool events', () => {
    const onReplyNavigate = vi.fn();
    const original = makeMessage({ id: 'original', role: 'User' });
    const agentReply = makeMessage({
      id: 'reply-agent',
      agent_type: 'Codex',
      content: 'Agent follow-up',
      reply_to_message_id: original.id,
    });
    const humanReply = makeMessage({
      id: 'reply-human',
      role: 'User',
      agent_type: null,
      author_pseudo: 'Peer',
      content: 'Human follow-up',
      reply_to_message_id: original.id,
    });
    const nativeToolEvent = makeMessage({
      id: 'reply-tool',
      role: 'System',
      content: '[agent-native: Read({})]',
      reply_to_message_id: original.id,
    });
    renderBubble(original, {
      replies: [agentReply, nativeToolEvent, humanReply],
      onReplyNavigate,
      t: (key: string, ...args: (string | number)[]) => `${key} ${args.join(' ')}`,
    });

    const backlinks = screen.getAllByRole('button', { name: /disc\.repliedBy/ });
    expect(backlinks).toHaveLength(2);
    expect(backlinks[0]).toHaveTextContent('@codex');
    expect(backlinks[1]).toHaveTextContent('@Peer');
    fireEvent.click(backlinks[1]);
    expect(onReplyNavigate).toHaveBeenCalledWith(humanReply.id);
  });

  it('keeps a System reply relationship internal to thread ordering', () => {
    renderBubble(makeMessage({
      role: 'System',
      content: '[agent-native: Bash({})]',
      reply_to_message_id: '12345678-source',
    }));

    expect(screen.queryByText('disc.inReplyTo')).not.toBeInTheDocument();
  });

  it('tags an error System message with data-variant="error"', () => {
    const { container } = renderBubble(makeMessage({ role: 'System', content: 'API exploded' }));
    const bubble = container.querySelector('.disc-msg-bubble');
    expect(bubble?.getAttribute('data-role')).toBe('system');
    expect(bubble?.getAttribute('data-variant')).toBe('error');
    expect(screen.getByText('disc.system')).toBeInTheDocument();
  });

  it('tags a summary-cached System message with data-variant="summary"', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'System', content: 'summary cached for last 10 messages' }),
    );
    expect(container.querySelector('.disc-msg-bubble')?.getAttribute('data-variant')).toBe('summary');
    expect(screen.getByText('disc.summaryCached')).toBeInTheDocument();
  });

  it('renders a planning receipt as an informational System message', () => {
    const { container } = renderBubble(
      makeMessage({
        role: 'System',
        content: '[kronn-planning:proposal-1:item-1] accepted KT-28',
      }),
    );
    expect(container.querySelector('.disc-msg-bubble')?.getAttribute('data-variant'))
      .toBe('kronn-planning');
    expect(screen.getByText('planning.receipt')).toBeInTheDocument();
    expect(screen.queryByText('disc.system')).not.toBeInTheDocument();
  });
});

describe('MessageBubble — author pseudo / avatar (User)', () => {
  it('renders a leading agent mention as a color-coded chip', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User', content: '@codex vérifie ce patch' }),
    );
    const chip = container.querySelector<HTMLElement>('.disc-agent-mention-chip');
    expect(chip).not.toBeNull();
    expect(chip?.dataset.agent).toBe('Codex');
    expect(chip?.textContent).toBe('@codex · agent ponctuel');
    expect(chip?.style.color).toBe('#10a37f');
    expect(screen.getByText('vérifie ce patch')).toBeInTheDocument();
  });

  it('renders a joined CLI alias as the same agent chip with an explicit CLI identity', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User', content: '@codex-cli vérifie ce patch' }),
    );
    const chip = container.querySelector<HTMLElement>('.disc-agent-mention-chip');
    expect(chip?.dataset.agent).toBe('Codex');
    expect(chip?.textContent).toBe('@codex · CLI');
  });

  it('renders an agent mention inline wherever it appears in the message', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User', content: 'demande à @codex plus tard' }),
    );
    const chip = container.querySelector<HTMLElement>('.disc-agent-mention-chip');
    expect(chip?.textContent).toBe('@codex · agent ponctuel');
    expect(container).toHaveTextContent('demande à @codex · agent ponctuel plus tard');
  });

  it('does not turn an unknown word or code literal into an agent chip', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User', content: 'garde @inconnu et `@codex` littéraux' }),
    );
    expect(container.querySelector('.disc-agent-mention-chip')).toBeNull();
    expect(container).toHaveTextContent('garde @inconnu et @codex littéraux');
  });

  it('renders gravatar <img> when author_avatar_email is set', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User', content: 'hello', author_pseudo: 'PeerAlpha', author_avatar_email: 'peer@example.com' }),
    );
    const img = container.querySelector('img.disc-msg-author-avatar') as HTMLImageElement | null;
    expect(img).not.toBeNull();
    expect(img?.getAttribute('src')).toContain('gravatar.com');
    // Name still rendered alongside the avatar.
    expect(screen.getByText('PeerAlpha')).toBeInTheDocument();
  });

  it('renders 2-letter uppercased initials when only author_pseudo is set', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User', content: 'hello', author_pseudo: 'peerbravo' }),
    );
    expect(container.querySelector('img.disc-msg-author-avatar')).toBeNull();
    const initials = container.querySelector('.disc-msg-author-initials');
    expect(initials?.textContent).toBe('PE');
  });

  it('still renders a HUMAN attribution (anonyme) when neither pseudo nor avatar is present', () => {
    // F11: a user message must ALWAYS read as a human (vs an agent), even with
    // no pseudo (federated from a peer whose pseudo is unset) — it falls back to
    // "anonyme · humain" rather than rendering no attribution at all.
    const { container } = renderBubble(makeMessage({ role: 'User', content: 'hello' }));
    const author = container.querySelector('.disc-msg-author');
    expect(author).not.toBeNull();
    expect(author?.textContent).toContain('anonyme');
    expect(author?.textContent).toContain('humain');
  });

  it('does not label the Orchestrateur system persona as a human', () => {
    const { container } = renderBubble(makeMessage({
      role: 'User',
      content: 'Sous-tâche lancée',
      author_pseudo: 'Orchestrateur',
    }));
    const author = container.querySelector('.disc-msg-author');
    expect(author).toHaveTextContent('disc.orchestrator');
    expect(author).not.toHaveTextContent('humain');
    expect(container.querySelector('.disc-msg-author-kind')).toHaveTextContent('disc.orchestratorKind');
  });
});

describe('MessageBubble — durable routing receipt', () => {
  it('keeps every requested native agent and reasoning icon on the user message', () => {
    renderBubble(
      makeMessage({ role: 'User', content: 'Comparez vos réponses' }),
      {
        targets: [
          { kind: 'agent', agent_type: 'ClaudeCode', tier: 'economy' },
          { kind: 'agent', agent_type: 'Codex', tier: 'reasoning' },
        ],
      },
    );

    const receipt = screen.getByTestId('message-routing-receipt');
    expect(receipt.closest('.disc-msg-author')).not.toBeNull();
    expect(receipt).toHaveAccessibleName('disc.routingRequested');
    expect(receipt).not.toHaveTextContent('disc.routingRequested');
    expect(receipt).toHaveTextContent('@claude · ⚡ disc.tier.economy');
    expect(receipt).toHaveTextContent('@codex · 🧠 disc.tier.reasoning');
  });

  it('does not invent a reasoning icon for a legacy target without a tier', () => {
    renderBubble(
      makeMessage({ role: 'User', content: 'Ancien message' }),
      { targets: [{ kind: 'discussion_agent', agent_type: 'ClaudeCode' }] },
    );

    const receipt = screen.getByTestId('message-routing-receipt');
    expect(receipt).toHaveTextContent('@claude');
    expect(receipt).not.toHaveTextContent('⚡');
    expect(receipt).not.toHaveTextContent('🎯');
    expect(receipt).not.toHaveTextContent('🧠');
  });

  it('uses the configured alias for a dynamic connection target', () => {
    renderBubble(
      makeMessage({ role: 'User', content: '@openrouter traduis ceci' }),
      {
        targets: [{
          kind: 'discussion_agent',
          agent_type: 'Custom',
          connection_id: 'conn-openrouter',
          tier: 'default',
        }],
        targetConnectionAliases: { 'conn-openrouter': 'openrouter' },
      },
    );

    expect(screen.getByTestId('message-routing-receipt'))
      .toHaveTextContent('@openrouter · 🎯 disc.tier.default');
  });

  it('identifies an exact CLI route without assigning it a Kronn tier', () => {
    renderBubble(
      makeMessage({ role: 'User', content: 'CLI uniquement' }),
      {
        targets: [{
          kind: 'cli',
          agent_type: 'ClaudeCode',
          cli_session_id: 42,
        }],
      },
    );

    const receipt = screen.getByTestId('message-routing-receipt');
    expect(receipt).toHaveTextContent('@claude-cli · disc.targetCli');
    expect(receipt).not.toHaveTextContent('🎯');
  });

  it('stays hidden when no durable routing data exists', () => {
    renderBubble(makeMessage({ role: 'User', content: 'Sans métadonnées' }));
    expect(screen.queryByTestId('message-routing-receipt')).not.toBeInTheDocument();
  });
});

describe('MessageBubble — agent mentions', () => {
  it('renders a mention written by an Agent with the same chip as a User mention', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', content: 'Je demande à @vibe de vérifier.' }),
    );
    const chip = container.querySelector<HTMLElement>('.disc-agent-mention-chip');
    expect(chip).not.toBeNull();
    expect(chip?.dataset.agent).toBe('Vibe');
    expect(chip?.textContent).toBe('@vibe · agent ponctuel');
    expect(container).toHaveTextContent('Je demande à @vibe · agent ponctuel de vérifier.');
  });

  it('keeps mentions in Agent code literals unstyled', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', content: 'Utilise `@vibe` mais appelle @codex.' }),
    );
    const chips = container.querySelectorAll<HTMLElement>('.disc-agent-mention-chip');
    expect(chips).toHaveLength(1);
    expect(chips[0]?.dataset.agent).toBe('Codex');
    expect(container.querySelector('code')).toHaveTextContent('@vibe');
  });
});

describe('MessageBubble — human mention (@user)', () => {
  it('renders @user as its own chip, not an agent chip', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', content: '@user peux-tu valider ?' }),
    );
    const chip = container.querySelector<HTMLElement>('.disc-user-mention-chip');
    expect(chip).not.toBeNull();
    expect(chip?.textContent).toBe('@user');
    expect(chip?.dataset.mention).toBe('user');
    expect(container.querySelector('.disc-agent-mention-chip')).toBeNull();
    expect(container).toHaveTextContent('@user peux-tu valider ?');
  });

  it('keeps @user in a code literal unstyled', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', content: 'écris `@user` littéralement' }),
    );
    expect(container.querySelector('.disc-user-mention-chip')).toBeNull();
    expect(container.querySelector('code')).toHaveTextContent('@user');
  });

  it('exposes the dispatch target so the awaited mention can be singled out', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User', content: '@codex et @vibe, avis ?', target_agent: 'Codex' }),
    );
    expect(container.querySelector('.disc-msg-row')?.getAttribute('data-target-agent')).toBe('Codex');
    // Both mentions still render — only their weight differs.
    expect(container.querySelectorAll('.disc-agent-mention-chip')).toHaveLength(2);
  });

  it('carries no dispatch target when the message addresses nobody in particular', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User', content: 'note pour plus tard' }),
    );
    expect(container.querySelector('.disc-msg-row')?.hasAttribute('data-target-agent')).toBe(false);
  });
});

describe('MessageBubble — @user carries the real human identity', () => {
  const renderWithIdentity = async (
    identity: { pseudo: string | null; avatar_email: string | null },
    content = '@user peux-tu valider ?',
  ) => {
    (configApi.getServerConfig as ReturnType<typeof vi.fn>).mockResolvedValue(identity);
    const rendered = render(
      <I18nProvider>
        <LocalIdentityProvider>
          <MessageBubble {...baseProps} msg={makeMessage({ role: 'Agent', content })} />
        </LocalIdentityProvider>
      </I18nProvider>,
    );
    // Let the identity fetch settle so the chip re-renders with it.
    await act(async () => { await Promise.resolve(); });
    return rendered;
  };

  it('shows the configured Kronn pseudo instead of a generic label', async () => {
    const { container } = await renderWithIdentity({ pseudo: 'Romu - mac', avatar_email: null });
    const chip = container.querySelector<HTMLElement>('.disc-user-mention-chip');
    expect(chip?.textContent).toBe('@Romu - mac');
    expect(chip?.getAttribute('title')).toBe('Romu - mac');
  });

  it('shows the Gravatar when an avatar e-mail is configured', async () => {
    const { container } = await renderWithIdentity({
      pseudo: 'Romu - mac',
      avatar_email: 'romu@example.com',
    });
    const img = container.querySelector<HTMLImageElement>('img.disc-user-mention-avatar');
    expect(img).not.toBeNull();
    expect(img?.getAttribute('src')).toContain('gravatar.com');
  });

  it('falls back to the canonical @user when no identity is configured', async () => {
    const { container } = await renderWithIdentity({ pseudo: null, avatar_email: null });
    const chip = container.querySelector<HTMLElement>('.disc-user-mention-chip');
    expect(chip?.textContent).toBe('@user');
    expect(container.querySelector('img.disc-user-mention-avatar')).toBeNull();
  });

  it('applies the configured color to agent mentions and the agent label', async () => {
    (configApi.getAgentAccess as ReturnType<typeof vi.fn>).mockResolvedValue({
      codex: { mention_color: '#123abc' },
    });
    const rendered = render(
      <I18nProvider>
        <LocalIdentityProvider>
          <MessageBubble
            {...baseProps}
            msg={makeMessage({ role: 'Agent', agent_type: 'Codex', content: '@codex prêt.' })}
          />
        </LocalIdentityProvider>
      </I18nProvider>,
    );
    const chip = await waitFor(() => {
      const element = rendered.container.querySelector<HTMLElement>('.disc-agent-mention-chip');
      expect(element?.style.color).toBe('#123abc');
      return element;
    });
    expect(chip).not.toBeNull();
    expect(rendered.container.querySelector<HTMLElement>('.disc-msg-agent-label')?.style.color)
      .toBe('#123abc');
  });
});

describe('MessageBubble — agent label + copy buttons', () => {
  it('uses the dynamic discussion alias instead of the Custom wire type', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', agent_type: 'Custom' }),
      { defaultAgent: 'Custom', defaultAgentAlias: '@openrouter' },
    );
    expect(container.querySelector('.disc-msg-agent-label')).toHaveTextContent(
      '@openrouter · disc.targetDiscussionAgent',
    );
    expect(container.querySelector('.disc-msg-agent-label')).not.toHaveTextContent('Custom');
  });

  it('renders a non-default native responder as a punctual agent', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent', agent_type: 'Codex' }));
    expect(container.querySelector('.disc-msg-agent-label')).toHaveTextContent(
      '@codex · disc.targetPunctualAgent',
    );
    expect(container.querySelector<HTMLElement>('.disc-msg-bubble')?.style.borderLeftColor)
      .toBeTruthy();
  });

  it('falls back to defaultAgent when agent_type is null', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', agent_type: null }),
      { defaultAgent: 'Vibe' },
    );
    expect(container.querySelector('.disc-msg-agent-label')).toHaveTextContent(
      '@vibe · disc.targetDiscussionAgent',
    );
  });

  it('labels an imported or appended CLI response explicitly', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', agent_type: 'Codex', source_msg_id: 'live-cli-1' }),
    );
    expect(container.querySelector('.disc-msg-agent-label')).toHaveTextContent(
      '@codex · disc.targetCli',
    );
  });

  it('labels the agentless guided-tour document as preloaded Kronn content', () => {
    const { container } = renderBubble(
      makeMessage({
        role: 'Agent',
        agent_type: 'ClaudeCode',
        source_msg_id: 'kronn-guided-tour-demo-preview',
      }),
    );
    expect(container.querySelector('.disc-msg-agent-label')).toHaveTextContent(
      'disc.tourDemoAuthor · disc.tourDemoKind',
    );
    expect(container.querySelector('.disc-msg-agent-label')).not.toHaveTextContent('@claude');
  });

  it('invokes onCopy(msgId, content) from the footer copy button', () => {
    const onCopy = vi.fn();
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', content: 'copy me' }),
      { onCopy },
    );
    // Two copy buttons exist for an Agent msg (label + footer); clicking
    // either must forward the same (id, content) pair.
    const btns = container.querySelectorAll('button.disc-copy-btn');
    expect(btns.length).toBe(2);
    fireEvent.click(btns[0]);
    expect(onCopy).toHaveBeenCalledWith('msg-test', 'copy me');
  });

  it('reflects the copied state on the copy button', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent' }), { isCopied: true });
    const btn = container.querySelector('button.disc-copy-btn');
    expect(btn?.getAttribute('data-copied')).toBe('true');
    expect(btn?.textContent).toContain('disc.copied');
  });
});

describe('MessageBubble — TTS button states (Agent only)', () => {
  it('shows the default TTS label when not active', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent' }));
    const tts = container.querySelector('button.disc-tts-btn');
    expect(tts?.textContent).toContain('TTS');
  });

  it('shows the loading state', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent' }),
      { isTtsActive: true, ttsState: 'loading' },
    );
    const tts = container.querySelector('button.disc-tts-btn');
    expect(tts?.textContent).toContain('TTS');
    expect(tts?.querySelector('svg')).not.toBeNull();
  });

  it('shows Pause while playing', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent' }),
      { isTtsActive: true, ttsState: 'playing' },
    );
    expect(container.querySelector('button.disc-tts-btn')?.textContent).toContain('Pause');
  });

  it('shows Reprendre while paused', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent' }),
      { isTtsActive: true, ttsState: 'paused' },
    );
    expect(container.querySelector('button.disc-tts-btn')?.textContent).toContain('Reprendre');
  });

  it('invokes onTts(msgId, content, language) on click', () => {
    const onTts = vi.fn();
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', content: 'read aloud' }),
      { onTts, language: 'es' },
    );
    fireEvent.click(container.querySelector('button.disc-tts-btn')!);
    expect(onTts).toHaveBeenCalledWith('msg-test', 'read aloud', 'es');
  });

  it('does NOT render a TTS button for User messages', () => {
    const { container } = renderBubble(makeMessage({ role: 'User', content: 'hi' }));
    expect(container.querySelector('button.disc-tts-btn')).toBeNull();
  });
});

describe('MessageBubble — footer chips', () => {
  it('renders the formatted timestamp', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent' }));
    const timeEl = container.querySelector('.disc-msg-time');
    // Locale-formatted HH:MM — exact value depends on TZ, so just assert
    // it looks like a clock value rather than empty.
    expect(timeEl?.textContent).toMatch(/\d{1,2}:\d{2}/);
  });

  it('renders the standard short ID pill in the header and copies the full ID', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      configurable: true,
    });
    const id = '12345678-90ab-cdef-1234-567890abcdef';
    const { container } = renderBubble(makeMessage({ role: 'System', id }));

    const pill = container.querySelector<HTMLButtonElement>('.disc-message-id-pill');
    expect(pill).not.toBeNull();
    expect(pill?.textContent).toContain('#12345678');
    expect(pill?.classList.contains('disc-id-pill')).toBe(true);
    expect(pill?.closest('.disc-msg-header-row')).not.toBeNull();
    expect(pill?.closest('.disc-msg-footer')).toBeNull();

    fireEvent.click(pill!);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(id));
    expect(pill?.getAttribute('data-copied')).toBe('true');
  });

  it('renders the token count only when tokens_used > 0', () => {
    const { container, rerender } = renderBubble(makeMessage({ role: 'Agent', tokens_used: 0 }));
    expect(container.querySelector('.disc-msg-token-count')).toBeNull();

    rerender(
      <I18nProvider>
        <MessageBubble {...baseProps} msg={makeMessage({ role: 'Agent', tokens_used: 12345 })} />
      </I18nProvider>,
    );
    const tok = container.querySelector('.disc-msg-token-count');
    expect(tok?.textContent).toContain('tok');
    // Locale-grouped number (12,345 / 12 345 / 12.345 depending on locale).
    expect(tok?.textContent).toMatch(/12[\s,.]?345/);
  });

  it('renders the auth-mode chip as "API key" for override', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent', auth_mode: 'override' }));
    const chip = container.querySelector('.disc-msg-auth-mode');
    expect(chip?.getAttribute('data-mode')).toBe('override');
    expect(chip?.textContent).toContain('API key');
  });

  it('renders the auth-mode chip as "auth locale" for any other mode', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent', auth_mode: 'local' }));
    const chip = container.querySelector('.disc-msg-auth-mode');
    expect(chip?.getAttribute('data-mode')).toBe('local');
    expect(chip?.textContent).toContain('auth locale');
  });

  it('renders a duration chip for an Agent message with prevUserTs', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', timestamp: '2026-05-29T10:30:42.000Z' }),
      { prevUserTs: '2026-05-29T10:30:00.000Z' },
    );
    const dur = container.querySelector('.disc-msg-duration');
    expect(dur?.textContent).toContain('42s');
  });

  it('formats a sub-minute-and-over duration as Xm Ys', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'Agent', timestamp: '2026-05-29T10:31:30.000Z' }),
      { prevUserTs: '2026-05-29T10:30:00.000Z' },
    );
    expect(container.querySelector('.disc-msg-duration')?.textContent).toContain('1m 30s');
  });

  it('omits the duration chip when prevUserTs is null', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent' }), { prevUserTs: null });
    expect(container.querySelector('.disc-msg-duration')).toBeNull();
  });

  it('renders the economy model-tier badge', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent', model_tier: 'economy' }));
    const badge = container.querySelector('.disc-model-tier-badge');
    expect(badge?.getAttribute('data-tier')).toBe('economy');
    expect(badge?.textContent).toContain('⚡');
    expect(badge?.textContent).toContain('disc.tier.economy');
  });

  it('renders a non-economy model-tier badge with the brain glyph', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent', model_tier: 'premium' }));
    const badge = container.querySelector('.disc-model-tier-badge');
    expect(badge?.getAttribute('data-tier')).toBe('premium');
    expect(badge?.textContent).toContain('🧠');
  });

  it('renders the full-access badge when hasFullAccess is true', () => {
    const { container } = renderBubble(makeMessage({ role: 'Agent' }), { hasFullAccess: true });
    const badge = container.querySelector('.disc-full-access-badge');
    expect(badge?.textContent).toContain('config.fullAccessBadge');
  });
});

describe('MessageBubble — last-message affordances', () => {
  it('offers deletion for a durable message and wires the selected message', () => {
    const onDelete = vi.fn();
    const message = makeMessage({ role: 'Agent', content: 'remove me' });
    renderBubble(message, { onDelete });

    fireEvent.click(screen.getByRole('button', { name: 'disc.deleteMessage' }));

    expect(onDelete).toHaveBeenCalledWith(message);
  });

  it('keeps deletion visible but disabled while an agent is running', () => {
    renderBubble(makeMessage({ role: 'User' }), { onDelete: vi.fn(), sending: true });

    expect(screen.getByRole('button', { name: 'disc.deleteMessage' })).toBeDisabled();
  });

  it('shows the edit pencil on the last user message', () => {
    const onEditStart = vi.fn();
    renderBubble(
      makeMessage({ role: 'User', content: 'oops typo' }),
      { isLastUser: true, isLastAgent: false },
    );
    const pencil = screen.getByLabelText('disc.editResend');
    fireEvent.click(pencil);
    expect(onEditStart).not.toHaveBeenCalled(); // (default noop in baseProps)
  });

  it('wires the edit pencil to onEditStart(id, content)', () => {
    const onEditStart = vi.fn();
    renderBubble(
      makeMessage({ role: 'User', content: 'oops typo' }),
      { isLastUser: true, onEditStart },
    );
    fireEvent.click(screen.getByLabelText('disc.editResend'));
    expect(onEditStart).toHaveBeenCalledWith('msg-test', 'oops typo');
  });

  it('shows the retry button on the last agent message and calls onRetry', () => {
    const onRetry = vi.fn();
    renderBubble(makeMessage({ role: 'Agent' }), { isLastAgent: true, onRetry });
    fireEvent.click(screen.getByLabelText('disc.retryResponse'));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it('hides edit/retry affordances while sending', () => {
    renderBubble(makeMessage({ role: 'User' }), { isLastUser: true, sending: true });
    expect(screen.queryByLabelText('disc.editResend')).toBeNull();
  });
});

describe('MessageBubble — edit mode', () => {
  it('renders the textarea bound to editingText', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User', content: 'orig' }),
      { isEditing: true, editingText: 'edited text' },
    );
    const ta = container.querySelector('textarea.disc-edit-textarea') as HTMLTextAreaElement;
    expect(ta).not.toBeNull();
    expect(ta.value).toBe('edited text');
    expect(ta).toHaveAttribute('rows', '1');
    expect(ta).toHaveAccessibleName('disc.editResend');
    expect(ta.closest('.disc-msg-bubble')).toHaveAttribute('data-editing', 'true');
  });

  it('fires onEditTextChange while typing', () => {
    const onEditTextChange = vi.fn();
    const { container } = renderBubble(
      makeMessage({ role: 'User' }),
      { isEditing: true, editingText: 'x', onEditTextChange },
    );
    fireEvent.change(container.querySelector('textarea')!, { target: { value: 'new' } });
    expect(onEditTextChange).toHaveBeenCalledWith('new');
  });

  it('grows with edited content and caps at the same height as the composer', async () => {
    const { container } = renderBubble(
      makeMessage({ role: 'User' }),
      { isEditing: true, editingText: 'x' },
    );
    const textarea = container.querySelector('textarea.disc-edit-textarea') as HTMLTextAreaElement;
    Object.defineProperty(textarea, 'scrollHeight', { configurable: true, value: 420 });

    fireEvent.change(textarea, { target: { value: 'a much longer edited message' } });

    await waitFor(() => expect(textarea.style.height).toBe('160px'));
  });

  it('fires onEditCancel from the cancel button', () => {
    const onEditCancel = vi.fn();
    renderBubble(
      makeMessage({ role: 'User' }),
      { isEditing: true, editingText: 'x', onEditCancel },
    );
    fireEvent.click(screen.getByText('disc.cancel'));
    expect(onEditCancel).toHaveBeenCalledTimes(1);
  });

  it('disables the resend button when the edited text is blank', () => {
    renderBubble(
      makeMessage({ role: 'User' }),
      { isEditing: true, editingText: '   ' },
    );
    const resend = screen.getByText('disc.resend').closest('button') as HTMLButtonElement;
    expect(resend.disabled).toBe(true);
  });

  it('submits on Ctrl+Enter (not during IME composition)', () => {
    const onEditSubmit = vi.fn();
    const { container } = renderBubble(
      makeMessage({ role: 'User' }),
      { isEditing: true, editingText: 'go', onEditSubmit },
    );
    fireEvent.keyDown(container.querySelector('textarea')!, { key: 'Enter', ctrlKey: true });
    expect(onEditSubmit).toHaveBeenCalledTimes(1);
  });
});

describe('MessageBubble — inline CTAs', () => {
  it('renders the override-key CTA on an auth-error message and navigates to settings', () => {
    const onNavigate = vi.fn();
    renderBubble(
      makeMessage({ role: 'Agent', content: 'Error: invalid API key, please authenticate.' }),
      { onNavigate },
    );
    fireEvent.click(screen.getByText('disc.overrideKey'));
    expect(onNavigate).toHaveBeenCalledWith('settings');
  });

  it('renders the edit-timeout CTA on a partial-response message and scrolls to server config', () => {
    const onNavigate = vi.fn();
    renderBubble(
      makeMessage({ role: 'Agent', content: "Réponse partielle — l'agent a été interrompu." }),
      { onNavigate },
    );
    fireEvent.click(screen.getByText('disc.editTimeout'));
    expect(onNavigate).toHaveBeenCalledWith('settings', { scrollTo: 'settings-server' });
  });

  it('renders a compact model error with collapsible diagnostics and a tier shortcut', () => {
    const onNavigate = vi.fn();
    const content = '[kronn:model-error]\n' + JSON.stringify({
      kind: 'model_error',
      status: 404,
      summary: 'LiteLlm returned HTTP 404 for model-a.',
      detail: 'LiteLLM error 404 Not Found: nested upstream details',
      tier: 'default',
    });
    sessionStorage.removeItem('kronn:model-config-target');
    renderBubble(makeMessage({
      role: 'System',
      content,
      agent_type: 'LiteLlm',
      model: 'model-a',
      model_tier: 'default',
    }), { onNavigate });

    expect(screen.getByTestId('disc-model-error-content')).toHaveTextContent('HTTP 404');
    expect(screen.getByText('disc.modelErrorDetails')).toBeInTheDocument();
    expect(screen.getByText(/nested upstream details/)).toBeInTheDocument();
    fireEvent.click(screen.getByText('disc.changeTierModel'));

    expect(onNavigate).toHaveBeenCalledWith('settings', { scrollTo: 'settings-agent-config' });
    expect(JSON.parse(sessionStorage.getItem('kronn:model-config-target') ?? '{}')).toEqual({
      agentType: 'LiteLlm',
      tier: 'default',
    });
  });

  it('renders an unreachable LiteLLM failure with a targeted one-agent retry', () => {
    const onRetryAgentDispatch = vi.fn();
    const content = '[kronn:agent-error]\n' + JSON.stringify({
      kind: 'agent_error',
      status: null,
      summary: 'LiteLlm is temporarily unreachable.',
      detail: 'LiteLLM unreachable at http://proxy: connection refused',
      tier: 'default',
      retry_dispatch_id: 'job-lite',
      retried: false,
    });
    renderBubble(makeMessage({
      role: 'System',
      content,
      agent_type: 'LiteLlm',
      model: 'model-a',
      model_tier: 'default',
    }), { onRetryAgentDispatch });

    expect(screen.getByTestId('disc-model-error-content')).toHaveTextContent('temporarily unreachable');
    fireEvent.click(screen.getByText('disc.retryAgent'));
    expect(onRetryAgentDispatch).toHaveBeenCalledWith('job-lite', 'LiteLlm');
  });

  it('marks a retried failure without offering a second immediate retry', () => {
    const onRetryAgentDispatch = vi.fn();
    const content = '[kronn:agent-error]\n' + JSON.stringify({
      kind: 'agent_error', status: null, summary: 'unreachable', detail: 'vpn',
      tier: 'default', retry_dispatch_id: 'job-lite', retried: true,
    });
    renderBubble(makeMessage({ role: 'System', content, agent_type: 'LiteLlm' }), {
      onRetryAgentDispatch,
    });

    expect(screen.getByText('disc.agentRetryQueued')).toBeInTheDocument();
    expect(screen.queryByText('disc.retryAgent')).not.toBeInTheDocument();
  });

  it('rejects malformed model-error events instead of hiding their raw content', () => {
    expect(parseModelErrorEvent('[kronn:model-error]\n{"status":404}')).toBeNull();
    expect(parseModelErrorEvent('ordinary system error')).toBeNull();
  });

  it('upgrades a legacy raw LiteLLM 404 into the same compact CTA', () => {
    const onNavigate = vi.fn();
    renderBubble(makeMessage({
      role: 'System',
      content: 'Erreur: LiteLLM error 404 Not Found: giant nested payload',
      agent_type: 'LiteLlm',
      model: 'legacy-model',
      model_tier: null,
    }), { onNavigate });

    expect(screen.getByTestId('disc-model-error-content')).toHaveTextContent('disc.modelErrorSummary');
    fireEvent.click(screen.getByText('disc.changeTierModel'));
    expect(JSON.parse(sessionStorage.getItem('kronn:model-config-target') ?? '{}')).toEqual({
      agentType: 'LiteLlm', tier: 'default',
    });
  });
});

describe('MessageBubble — summary-cached expand toggle', () => {
  it('shows the view-summary toggle when a summaryCache is present', () => {
    renderBubble(
      makeMessage({ role: 'System', content: 'summary cached for last 10 messages' }),
      { summaryCache: 'condensed history' },
    );
    expect(screen.getByText('disc.viewSummary')).toBeInTheDocument();
  });

  it('calls onExpandSummary when the toggle is clicked', () => {
    const onExpandSummary = vi.fn();
    renderBubble(
      makeMessage({ role: 'System', content: 'summary cached' }),
      { summaryCache: 'condensed history', onExpandSummary },
    );
    fireEvent.click(screen.getByText('disc.viewSummary'));
    expect(onExpandSummary).toHaveBeenCalledWith('msg-test');
  });

  it('renders the expanded summary body when isExpandedSummary is true', () => {
    const { container } = renderBubble(
      makeMessage({ role: 'System', content: 'summary cached' }),
      { summaryCache: 'condensed history body', isExpandedSummary: true },
    );
    const expanded = container.querySelector('.disc-summary-expanded');
    expect(expanded?.textContent).toContain('condensed history body');
    expect(screen.getByText('disc.hideSummary')).toBeInTheDocument();
  });
});

// ─── MarkdownContent helpers (CopyableBlock + fence branches) ────────────────

describe('MarkdownContent — CopyableBlock copy paths', () => {
  beforeEach(() => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      configurable: true,
    });
  });

  it('copies a fenced code block via the overlay button', () => {
    const { container } = render(
      <MarkdownContent content={'```\nconst x = 1;\n```'} />,
    );
    const btn = container.querySelector('button.disc-copyable-block-btn') as HTMLButtonElement;
    expect(btn).not.toBeNull();
    fireEvent.click(btn);
    expect(navigator.clipboard.writeText).toHaveBeenCalled();
    const arg = (navigator.clipboard.writeText as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    expect(arg).toContain('const x = 1;');
  });

  it('copies a markdown table as tab/newline-joined text (extractText TABLE path)', () => {
    const md = '| A | B |\n| --- | --- |\n| 1 | 2 |';
    const { container } = render(<MarkdownContent content={md} />);
    // The table is wrapped in its own CopyableBlock; grab the button
    // inside the element that contains a <table>.
    const tableWrapper = container.querySelector('table')!.closest('.relative') as HTMLElement;
    const btn = within(tableWrapper).getByRole('button');
    fireEvent.click(btn);
    const arg = (navigator.clipboard.writeText as ReturnType<typeof vi.fn>).mock.calls[0][0] as string;
    // Header + cells flattened with tab separators and newline between rows.
    expect(arg).toContain('A\tB');
    expect(arg).toContain('1\t2');
  });
});

describe('MarkdownContent — fence branches (discussionId-gated)', () => {
  it('renders a normal code block through CopyableBlock when no discussionId', () => {
    const { container } = render(<MarkdownContent content={'```js\nfoo()\n```'} />);
    expect(container.querySelector('pre')).not.toBeNull();
    expect(container.querySelector('button.disc-copyable-block-btn')).not.toBeNull();
  });

  it('falls through to a code block for a malformed kronn-doc-data fence', () => {
    // Invalid JSON inside the fence → the try/catch falls through to the
    // standard CopyableBlock render rather than crashing.
    const md = '```kronn-doc-data\n{ not valid json\n```';
    const { container } = render(<MarkdownContent content={md} discussionId="disc-x" />);
    expect(container.querySelector('pre')).not.toBeNull();
  });

  it('falls through to a code block for an unknown kronn-doc-data format', () => {
    const md = '```kronn-doc-data\n{"format":"docx","rows":[]}\n```';
    const { container } = render(<MarkdownContent content={md} discussionId="disc-x" />);
    // Unknown format → no DocDataExport, plain code block instead.
    expect(container.querySelector('pre')).not.toBeNull();
  });
});

describe('MessageBubble — role exhaustiveness sanity', () => {
  it.each<[MessageRole, string]>([
    ['User', 'user'],
    ['Agent', 'agent'],
    ['System', 'system'],
  ])('role %s → data-role %s', (role, expected) => {
    const { container } = renderBubble(makeMessage({ role, content: 'x' }));
    expect(container.querySelector('.disc-msg-row')?.getAttribute('data-role')).toBe(expected);
  });
});
