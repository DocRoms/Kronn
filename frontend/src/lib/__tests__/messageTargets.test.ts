import { describe, expect, it } from 'vitest';
import {
  composerMentions,
  messagesInConversationOrder,
  nativeDiscussionTargets,
  pendingAgentReplies,
  targetsFromComposerText,
} from '../messageTargets';
import type { Discussion, ParticipantView } from '../../types/generated';

// `cli_ordinal` defaults to null so the existing cases keep exercising the
// positional fallback they were written for; pass it explicitly to pin the
// backend-ranked behaviour.
const cli = (id: number, agent_type: string, cli_ordinal: number | null = null): ParticipantView => ({
  id,
  cli_ordinal,
  disc_id: 'disc-1',
  agent_type,
  session_id: `session-${id}`,
  role: 'peer',
  status: 'active',
  joined_at: '2026-07-29T00:00:00Z',
  left_at: null,
  last_seen: null,
  activity: null,
  presence_state: 'listening',
  read_live: true,
  write_state: 'ok',
  wake_mode: 'external_poll',
  next_poll_at: null,
  last_write_at: null,
  resume_reason: null,
  resume_since: null,
  model: null,
  conversation_id: null,
});

const labels = {
  discussionAgent: 'agent de discussion',
  punctualAgent: 'agent ponctuel',
  cli: 'CLI',
  all: 'tous les intervenants',
};

describe('typed composer targets', () => {
  it('distinguishes the configured agent, punctual agents and an exact CLI', () => {
    const mentions = composerMentions(
      'ClaudeCode',
      ['ClaudeCode', 'Codex', 'Vibe'],
      [cli(42, 'Codex')],
      labels,
    );

    expect(mentions).toEqual(expect.arrayContaining([
      expect.objectContaining({
        trigger: '@claude',
        label: 'agent de discussion',
        target: expect.objectContaining({ kind: 'discussion_agent', agent_type: 'ClaudeCode' }),
      }),
      expect.objectContaining({
        trigger: '@codex',
        label: 'agent ponctuel',
        target: expect.objectContaining({
          kind: 'agent', agent_type: 'Codex', tier: 'default',
        }),
      }),
      expect.objectContaining({
        trigger: '@codex-cli',
        // KT-211: the CLI entry shows its real alias, never the bare
        // provider trigger it would share with the punctual agent.
        displayTrigger: '@codex-cli',
        label: 'CLI',
        target: expect.objectContaining({ kind: 'cli', cli_session_id: 42 }),
      }),
    ]));
  });

  it('ranks CLIs by the backend ordinal, not by their position in the list', () => {
    // The whole point of KT-247: the ordinal is a property of the session, not
    // of the rendered order. Here the session ranked 2 by the backend comes
    // FIRST in the list — a positional counter would label it `CLI` and rename
    // the other one, rewriting who authored what in past messages.
    const mentions = composerMentions(
      'ClaudeCode',
      ['ClaudeCode'],
      [cli(80, 'ClaudeCode', 2), cli(66, 'ClaudeCode', 1)],
      labels,
    );

    expect(mentions).toEqual(expect.arrayContaining([
      expect.objectContaining({
        trigger: '@claude-cli-2',
        label: 'CLI 2',
        target: expect.objectContaining({ kind: 'cli', cli_session_id: 80 }),
      }),
      expect.objectContaining({
        trigger: '@claude-cli',
        label: 'CLI',
        target: expect.objectContaining({ kind: 'cli', cli_session_id: 66 }),
      }),
    ]));
  });

  it('keeps only the plural target that has not answered the latest turn', () => {
    const discussion = {
      id: 'disc-1', agent: 'LiteLlm', participants: ['LiteLlm', 'Ollama'],
      awaiting_agent: true,
      messages: [
        {
          id: 'u1', role: 'User', channel: 'main',
          content: 'Qui est le plus malin entre @litellm et @ollama ?',
          agent_type: null, timestamp: '2026-08-10T09:53:05Z', tokens_used: 0, auth_mode: null,
        },
        {
          id: 'a1', role: 'Agent', channel: 'main', content: 'LiteLLM answer',
          agent_type: 'LiteLlm', timestamp: '2026-08-10T09:53:16Z', tokens_used: 1,
          auth_mode: 'local',
        },
      ],
    } as Discussion;

    expect(pendingAgentReplies(discussion).map(reply => reply.agent)).toEqual(['Ollama']);
  });

  it('addresses every attached native model by default but excludes CLI sessions', () => {
    expect(nativeDiscussionTargets({
      agent: 'LiteLlm',
      participants: ['LiteLlm', 'Ollama', 'LiteLlm'],
    })).toEqual([
      {
        kind: 'discussion_agent', agent_type: 'LiteLlm', cli_session_id: null, tier: null,
      },
      {
        kind: 'agent', agent_type: 'Ollama', cli_session_id: null, tier: 'default',
      },
    ]);
  });

  it('keeps all unanswered native models visible for a general group turn', () => {
    const discussion = {
      id: 'disc-general', agent: 'LiteLlm', participants: ['LiteLlm', 'Ollama'],
      awaiting_agent: true,
      messages: [{
        id: 'u-general', role: 'User', channel: 'main',
        content: 'Vous devriez connaître vos points forts et faibles.',
        agent_type: null, timestamp: '2026-08-10T10:01:53Z', tokens_used: 0, auth_mode: null,
      }],
    } as Discussion;

    expect(pendingAgentReplies(discussion).map(reply => reply.agent)).toEqual(['LiteLlm', 'Ollama']);
  });

  it('does not mistake an alias shown in code for the only pending model', () => {
    const discussion = {
      id: 'disc-code', agent: 'LiteLlm', participants: ['LiteLlm', 'Ollama'],
      awaiting_agent: true,
      messages: [{
        id: 'u-code', role: 'User', channel: 'main',
        content: 'Documente la syntaxe `@ollama` sans cibler un seul modèle.',
        agent_type: null, timestamp: '2026-08-10T10:01:53Z', tokens_used: 0, auth_mode: null,
      }],
    } as Discussion;

    expect(pendingAgentReplies(discussion).map(reply => reply.agent)).toEqual(['LiteLlm', 'Ollama']);
  });

  it('keeps duplicate agents separate when two turns both have active jobs', () => {
    const discussion = {
      id: 'disc-overlap', agent: 'LiteLlm', participants: ['LiteLlm', 'Ollama'],
      awaiting_agent: true,
      messages: [],
      active_agent_dispatches: [
        { id: 'job-old', trigger_message_id: 'u-old', agent_type: 'Ollama', status: 'Running' },
        { id: 'job-new', trigger_message_id: 'u-new', agent_type: 'Ollama', status: 'Pending' },
      ],
    } as unknown as Discussion & { active_agent_dispatches: Array<{
      id: string; trigger_message_id: string; agent_type: 'Ollama'; status: string;
    }> };

    expect(pendingAgentReplies(discussion)).toEqual([
      { id: 'job-old', triggerMessageId: 'u-old', agent: 'Ollama', status: 'Running' },
      { id: 'job-new', triggerMessageId: 'u-new', agent: 'Ollama', status: 'Pending' },
    ]);
  });

  it('places a late agent reply inside its source turn without rewriting durable order', () => {
    const messages = [
      { id: 'u-old', role: 'User', channel: 'main', content: 'first' },
      { id: 'a-fast', role: 'Agent', channel: 'main', content: 'fast', reply_to_message_id: 'u-old' },
      { id: 'u-new', role: 'User', channel: 'main', content: 'second' },
      { id: 'a-late', role: 'Agent', channel: 'main', content: 'late', reply_to_message_id: 'u-old' },
      { id: 'a-new', role: 'Agent', channel: 'main', content: 'new', reply_to_message_id: 'u-new' },
    ] as Discussion['messages'];

    expect(messagesInConversationOrder(messages).map(message => message.id)).toEqual([
      'u-old', 'a-fast', 'a-late', 'u-new', 'a-new',
    ]);
    expect(messages.map(message => message.id)).toEqual([
      'u-old', 'a-fast', 'u-new', 'a-late', 'a-new',
    ]);
  });

  it('keeps a late nested agent handoff inside the originating human turn', () => {
    const messages = [
      { id: 'u-old', role: 'User', channel: 'main', content: 'first' },
      { id: 'a-source', role: 'Agent', channel: 'main', content: '@ollama help', reply_to_message_id: 'u-old' },
      { id: 'u-new', role: 'User', channel: 'main', content: 'second' },
      { id: 'a-child', role: 'Agent', channel: 'main', content: 'handoff answer', reply_to_message_id: 'a-source' },
    ] as Discussion['messages'];

    expect(messagesInConversationOrder(messages).map(message => message.id)).toEqual([
      'u-old', 'a-source', 'a-child', 'u-new',
    ]);
  });

  it('falls back to durable order for missing or cyclic reply chains', () => {
    const messages = [
      { id: 'u-1', role: 'User', channel: 'main', content: 'first' },
      { id: 'a-cycle-1', role: 'Agent', channel: 'main', content: 'one', reply_to_message_id: 'a-cycle-2' },
      { id: 'a-cycle-2', role: 'Agent', channel: 'main', content: 'two', reply_to_message_id: 'a-cycle-1' },
      { id: 'a-missing', role: 'Agent', channel: 'main', content: 'missing', reply_to_message_id: 'unknown' },
    ] as Discussion['messages'];

    expect(messagesInConversationOrder(messages).map(message => message.id)).toEqual([
      'u-1', 'a-cycle-1', 'a-cycle-2', 'a-missing',
    ]);
  });

  it('does not move a human threaded reply out of chronological order', () => {
    const messages = [
      { id: 'u-old', role: 'User', channel: 'main', content: 'first' },
      { id: 'a-old', role: 'Agent', channel: 'main', content: 'answer' },
      { id: 'u-new', role: 'User', channel: 'main', content: 'reply', reply_to_message_id: 'u-old' },
    ] as Discussion['messages'];

    expect(messagesInConversationOrder(messages).map(message => message.id)).toEqual([
      'u-old', 'a-old', 'u-new',
    ]);
  });

  it('preserves textual order, deduplicates and keeps @all explicit', () => {
    const mentions = composerMentions(
      'ClaudeCode',
      ['ClaudeCode', 'Codex', 'Vibe'],
      [cli(42, 'Codex')],
      labels,
    );
    const result = targetsFromComposerText(
      '@vibe puis @codex-cli et @codex-cli — @all',
      mentions,
    );

    expect(result.targetAll).toBe(true);
    expect(result.targets).toEqual([
      expect.objectContaining({ kind: 'agent', agent_type: 'Vibe' }),
      expect.objectContaining({ kind: 'cli', agent_type: 'Codex', cli_session_id: 42 }),
    ]);
  });

  it('treats mentions in inline and fenced code as documentation', () => {
    const mentions = composerMentions(
      'ClaudeCode',
      ['ClaudeCode', 'Codex', 'Vibe'],
      [cli(42, 'Codex')],
      labels,
    );
    const result = targetsFromComposerText(
      'Explique `@codex` puis ```md\n@codex-cli\n@all\n``` à @vibe.',
      mentions,
    );

    expect(result.targetAll).toBe(false);
    expect(result.targets).toEqual([
      expect.objectContaining({ kind: 'agent', agent_type: 'Vibe' }),
    ]);
  });
});
