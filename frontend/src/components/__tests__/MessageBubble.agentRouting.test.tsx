/** KT-1109: how a turn was routed (parallel or orchestrated), and which agents
 *  a reply named without launching them. */
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return { ...real, config: { getUiLanguage: vi.fn().mockResolvedValue('fr') } };
});

import { MessageBubble } from '../MessageBubble';
import type { AgentType, DiscussionMessage, MessageTarget } from '../../types/generated';
import type { InertMention } from '../../lib/agentDelegation';

const t = (key: string, ...args: (string | number)[]) => [key, ...args].join('|');

const baseProps = {
  idx: 0, isLastUser: false, isLastAgent: false, isEditing: false, isCopied: false,
  isTtsActive: false, ttsState: 'idle' as const, isExpandedSummary: false, prevUserTs: null,
  defaultAgent: 'ClaudeCode' as const, summaryCache: null, language: 'fr', sending: false,
  editingText: '', hasFullAccess: false, onCopy: () => {}, onTts: () => {}, onEditStart: () => {},
  onEditCancel: () => {}, onEditSubmit: () => {}, onEditTextChange: () => {}, onRetry: () => {},
  onExpandSummary: () => {}, discussionId: 'disc-test', projectId: null, onNavigate: () => {},
  t,
};

const agent = (type: AgentType): MessageTarget => ({
  kind: 'agent', agent_type: type, cli_session_id: null, tier: null,
});

function message(over: Partial<DiscussionMessage>): DiscussionMessage {
  return {
    id: 'm1', role: 'User', channel: 'main', content: '', agent_type: null,
    timestamp: new Date().toISOString(), tokens_used: 0, auth_mode: null,
    author_pseudo: 'Romu', author_avatar_email: null, ...over,
  };
}

function renderBubble(msg: DiscussionMessage, targets: MessageTarget[] = [], inertMentions?: InertMention[]) {
  return render(
    <I18nProvider>
      <MessageBubble {...baseProps} msg={msg} targets={targets} inertMentions={inertMentions} />
    </I18nProvider>,
  );
}

describe('MessageBubble — multi-agent routing', () => {
  it('says a message naming several agents launched them in parallel', () => {
    renderBubble(message({ content: '@codex @claude une blague' }), [agent('Codex'), agent('ClaudeCode')]);
    expect(screen.getByTestId('message-routing-parallel').textContent).toContain('disc.routingParallel|2');
    expect(screen.queryByTestId('message-routing-orchestrated')).toBeNull();
  });

  it('names the agents an orchestrated turn delegates to', () => {
    renderBubble(message({ content: '@opencode lance @codex et @claude' }), [agent('OpenCode')]);
    expect(screen.getByTestId('message-routing-orchestrated').textContent)
      .toContain('disc.routingOrchestrates|@codex, @claude');
    expect(screen.queryByTestId('message-routing-parallel')).toBeNull();
  });

  it('notes each inert mention with its reason', () => {
    renderBubble(
      message({ role: 'Agent', agent_type: 'Codex', author_pseudo: null, content: '@claude, à toi. Demande à @opencode.' }),
      [],
      [{ agent: 'ClaudeCode', reason: 'scheduled' }, { agent: 'OpenCode', reason: 'prose' }],
    );
    const note = screen.getByTestId('message-inert-mentions');
    expect(note.getAttribute('role')).toBe('note');
    expect(note.textContent).toContain('disc.inertMentionScheduled|@claude');
    expect(note.textContent).toContain('disc.inertMentionProse|@opencode');
  });

  it('renders no note without inert mentions', () => {
    renderBubble(message({ role: 'Agent', agent_type: 'Codex', author_pseudo: null, content: 'Voilà.' }));
    expect(screen.queryByTestId('message-inert-mentions')).toBeNull();
  });
});
