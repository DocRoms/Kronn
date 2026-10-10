import { describe, expect, it } from 'vitest';
import type { AgentType, DiscussionMessage, MessageTarget } from '../../types/generated';
import {
  detectDelegation,
  inertAgentMentions,
  orchestratedAgents,
} from '../agentDelegation';
import { composerMentions, positionedTargetsFromComposerText } from '../messageTargets';

const labels = { discussionAgent: 'principal', punctualAgent: 'punctual', cli: 'CLI', all: 'all' };
const mentions = composerMentions(
  'ClaudeCode',
  ['ClaudeCode', 'Codex', 'OpenCode'],
  [],
  labels,
);

function detect(text: string) {
  return detectDelegation(text, positionedTargetsFromComposerText(text, mentions));
}

const agent = (type: AgentType): MessageTarget => ({
  kind: 'agent', agent_type: type, cli_session_id: null, tier: 'default',
});

function message(partial: Partial<DiscussionMessage> & Pick<DiscussionMessage, 'id' | 'role'>): DiscussionMessage {
  return {
    channel: 'main',
    content: '',
    agent_type: null,
    timestamp: '2026-10-09T10:00:00Z',
    tokens_used: 0,
    auth_mode: null,
    ...partial,
  } as DiscussionMessage;
}

describe('detectDelegation', () => {
  it('proposes the first agent as orchestrator of the others (the reported message)', () => {
    const proposal = detect('@opencode tu peux lancer @codex et @claude, leur demander une blague et juger la meilleure');
    expect(proposal?.orchestrator.agent_type).toBe('OpenCode');
    expect(proposal?.delegated.map(target => target.agent_type)).toEqual(['Codex', 'ClaudeCode']);
  });

  it.each([
    '@opencode demande à @codex une revue',
    '@opencode fais juger par @codex',
    '@opencode please ask @codex for a joke',
    '@opencode pídele a @codex una broma',
    '@opencode 让 @codex 讲个笑话',
  ])('detects "%s"', text => {
    expect(detect(text)?.orchestrator.agent_type).toBe('OpenCode');
  });

  it.each([
    '@codex @claude donnez-moi chacun une blague',
    '@codex et @claude, demandez-vous qui a raison',
    '@codex fais une revue, @claude aussi',
    '@opencode lance les tests',
    '@opencode `lance @codex` est un exemple',
  ])('stays silent on "%s"', text => {
    expect(detect(text)).toBeNull();
  });

  it('never proposes orchestration for @all', () => {
    expect(detect('@all lance @codex')).toBeNull();
  });
});

describe('inertAgentMentions', () => {
  const user = message({ id: 'u1', role: 'User', content: '@codex @claude une blague' });

  it('flags a sibling already scheduled on the same turn', () => {
    const reply = message({
      id: 'a1', role: 'Agent', agent_type: 'Codex', reply_to_message_id: 'u1',
      content: 'Voici ma blague. @claude, à toi.',
    });
    const map = inertAgentMentions([user, reply], { u1: [agent('Codex'), agent('ClaudeCode')] }, 'ClaudeCode');
    expect(map.get('a1')).toEqual([{ agent: 'ClaudeCode', reason: 'scheduled' }]);
  });

  it('flags a prose mention of an agent nobody launched', () => {
    const single = message({ id: 'u2', role: 'User', content: 'Une blague ?' });
    const reply = message({
      id: 'a2', role: 'Agent', agent_type: 'ClaudeCode', reply_to_message_id: 'u2',
      content: 'Demande aussi à @codex.',
    });
    const map = inertAgentMentions([single, reply], {}, 'ClaudeCode');
    expect(map.get('a2')).toEqual([{ agent: 'Codex', reason: 'prose' }]);
  });

  it('stays silent on a real handoff, a self mention, code and CLI authors', () => {
    const reply = message({
      id: 'a3', role: 'Agent', agent_type: 'OpenCode', reply_to_message_id: 'u1',
      content: '@codex, une blague. Moi, @opencode, je juge. Exemple : `@claude`',
    });
    const cli = message({
      id: 'a4', role: 'Agent', agent_type: 'Codex', reply_to_message_id: 'u1',
      author_cli_ordinal: 1, content: '@claude regarde',
    });
    const map = inertAgentMentions([user, reply, cli], { u1: [agent('OpenCode')], a3: [agent('Codex')] }, 'ClaudeCode');
    expect(map.size).toBe(0);
  });
});

describe('orchestratedAgents', () => {
  it('names the delegated agents of a single-target delegation', () => {
    const user = message({ id: 'u1', role: 'User', content: '@opencode lance @codex et @claude' });
    expect(orchestratedAgents(user, [agent('OpenCode')])).toEqual(['Codex', 'ClaudeCode']);
  });

  it('is empty for a parallel turn or a passing mention', () => {
    const parallel = message({ id: 'u2', role: 'User', content: '@opencode lance @codex' });
    expect(orchestratedAgents(parallel, [agent('OpenCode'), agent('Codex')])).toEqual([]);
    const passing = message({ id: 'u3', role: 'User', content: '@opencode comme @kiro hier' });
    expect(orchestratedAgents(passing, [agent('OpenCode')])).toEqual([]);
  });
});
