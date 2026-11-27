// KT-1936 (review round 1, finding 3) — one resolver for the model the header
// trigger, the discussion list row and the activity bubbles name: the exact
// model, else the model the mode points to, else the last reply's, else the
// « default model » state. Before it, the row and the bubbles read the exact
// model alone and said « default model » while the header wrote the mode's.
import { describe, expect, it } from 'vitest';
import {
  discussionAgentModel, lastReplyModel, modeModelIdFor, resolveIdentityModel,
} from '../agentModelResolution';
import type { ExternalApiConnectionView } from '../api';
import type { ModelTiersConfig } from '../../types/generated';

const empty = { economy: null, default: null, reasoning: null };
const FAMILY_TIERS: ModelTiersConfig = {
  claude_code: empty, codex: empty, gemini_cli: empty, kiro: empty, vibe: empty,
  copilot_cli: empty, ollama: empty, lite_llm: empty, nvidia: empty,
  open_code: { economy: null, default: null, reasoning: 'opencode-go/kimi-k3.1-pro' },
};

const connection = (overrides: Partial<ExternalApiConnectionView> = {}): ExternalApiConnectionView => ({
  id: 'conn-1', display_name: 'OpenRouter', mention_alias: 'openrouter', endpoint: null,
  credential_slug: 'or', origin_preset: 'open_router',
  economy_model: null, default_model: 'openai/gpt-5.5', reasoning_model: 'anthropic/claude-fable-5-2',
  created_at: '', updated_at: '', has_credential: true,
  ...overrides,
});

describe('resolveIdentityModel', () => {
  it('takes the exact model, then the mode, then the last reply, then the unknown state', () => {
    expect(resolveIdentityModel({ exact: 'x', mode: 'm', lastReply: 'r' })).toEqual({ kind: 'id', id: 'x' });
    expect(resolveIdentityModel({ exact: ' ', mode: 'm', lastReply: 'r' })).toEqual({ kind: 'id', id: 'm' });
    expect(resolveIdentityModel({ mode: '', lastReply: ' r ' })).toEqual({ kind: 'id', id: 'r' });
    expect(resolveIdentityModel({})).toEqual({ kind: 'providerDefault' });
  });
});

describe('discussionAgentModel', () => {
  it('names the model the mode points to when the discussion holds no exact model (the room of the review)', () => {
    const room = { agent: 'OpenCode' as const, tier: 'reasoning' as const, model: null, messages: [] };
    expect(discussionAgentModel(room, { familyTiers: FAMILY_TIERS }))
      .toEqual({ kind: 'id', id: 'opencode-go/kimi-k3.1-pro' });
  });

  it('prefers the exact model over the mode', () => {
    const disc = { agent: 'OpenCode' as const, tier: 'reasoning' as const, model: 'opencode-go/glm-5' };
    expect(discussionAgentModel(disc, { familyTiers: FAMILY_TIERS })).toEqual({ kind: 'id', id: 'opencode-go/glm-5' });
  });

  it('falls back to the last reply on the same mode, then to the unknown state', () => {
    const messages = [
      { role: 'Agent', agent_type: 'Codex' as const, model_tier: 'default', model: 'gpt-5.5-codex' },
      { role: 'Agent', agent_type: 'Codex' as const, model_tier: 'reasoning', model: 'gpt-5.5-pro' },
    ];
    expect(discussionAgentModel({ agent: 'Codex', tier: 'default', messages }, { familyTiers: FAMILY_TIERS }))
      .toEqual({ kind: 'id', id: 'gpt-5.5-codex' });
    expect(discussionAgentModel({ agent: 'Codex', tier: 'economy', messages }, { familyTiers: FAMILY_TIERS }))
      .toEqual({ kind: 'providerDefault' });
    expect(discussionAgentModel({ agent: 'Codex' })).toEqual({ kind: 'providerDefault' });
  });

  it('reads the mode models of the discussion\'s HTTP connection', () => {
    const disc = { agent: 'Custom' as const, tier: 'reasoning' as const };
    expect(discussionAgentModel(disc, { connection: connection() }))
      .toEqual({ kind: 'id', id: 'anthropic/claude-fable-5-2' });
    // An HTTP connection without a model for this mode runs its default one.
    expect(discussionAgentModel(disc, { connection: connection({ reasoning_model: null }) }))
      .toEqual({ kind: 'id', id: 'openai/gpt-5.5' });
  });

  it('is what the picker trigger resolves for the same mode', () => {
    expect(modeModelIdFor({ agent: 'OpenCode' }, 'reasoning', FAMILY_TIERS)).toBe('opencode-go/kimi-k3.1-pro');
    expect(modeModelIdFor({ agent: 'OpenCode' }, 'default', FAMILY_TIERS)).toBe('');
  });
});

describe('lastReplyModel', () => {
  it('skips user messages and other agents, newest first', () => {
    const messages = [
      { role: 'Agent', agent_type: 'Codex' as const, model: 'old' },
      { role: 'Agent', agent_type: 'Codex' as const, model: 'new' },
      { role: 'Agent', agent_type: 'Kiro' as const, model: 'kiro-model' },
      { role: 'User', agent_type: null, model: null },
    ];
    expect(lastReplyModel(messages, 'Codex', 'default')).toBe('new');
    expect(lastReplyModel(null, 'Codex', 'default')).toBeNull();
  });
});

// KT-2013 — the last reply names only a model the next turn runs.
describe('lastReplyModel scope (KT-2013)', () => {
  const reply = (id: string, model: string, replyTo?: string) => ({
    id, role: 'Agent', agent_type: 'Custom' as const, model, reply_to_message_id: replyTo ?? null,
  });
  const targets = {
    'ask-a': [{ kind: 'discussion_agent' as const, agent_type: 'Custom' as const, connection_id: 'conn-a' }],
    'ask-b': [{ kind: 'discussion_agent' as const, agent_type: 'Custom' as const, connection_id: 'conn-b' }],
  };

  it('skips the exact models the discussion dropped and reads the reply before them', () => {
    const messages = [
      { role: 'Agent', agent_type: 'Codex' as const, model: 'gpt-5.5-codex' },
      { role: 'Agent', agent_type: 'Codex' as const, model: 'gpt-exact-pin' },
    ];
    expect(lastReplyModel(messages, 'Codex', 'default')).toBe('gpt-exact-pin');
    expect(lastReplyModel(messages, 'Codex', 'default', { exclude: ['gpt-exact-pin'] })).toBe('gpt-5.5-codex');
    expect(lastReplyModel(messages, 'Codex', 'default', { exclude: ['gpt-exact-pin', 'gpt-5.5-codex'] })).toBeNull();
  });

  it('skips only the reply to a mention of another connection', () => {
    const messages = [reply('r-a', 'model-of-a', 'ask-a'), reply('r-b', 'model-of-b', 'ask-b')];
    expect(lastReplyModel(messages, 'Custom', 'default', { connectionId: 'conn-a', messageTargets: targets })).toBe('model-of-a');
    expect(lastReplyModel(messages, 'Custom', 'default', { connectionId: 'conn-b', messageTargets: targets })).toBe('model-of-b');
    // Both triggers name another connection: nothing.
    expect(lastReplyModel(messages, 'Custom', 'default', { connectionId: 'conn-c', messageTargets: targets })).toBeNull();
    // A Custom agent whose connection is unknown names no reply.
    expect(lastReplyModel(messages, 'Custom', 'default', { messageTargets: targets })).toBeNull();
  });

  // The review measured it on real data: an ordinary turn's trigger names no
  // connection (no target, or a discussion-agent target without one); the turn
  // inherits the discussion's connection, so its reply is this connection's.
  const litellm = (id: string, model: string, replyTo?: string) => ({
    id, role: 'Agent', agent_type: 'LiteLlm' as const, model, reply_to_message_id: replyTo ?? null,
  });
  const ordinaryTargets = {
    'ask-plain-target': [{ kind: 'discussion_agent' as const, agent_type: 'LiteLlm' as const }],
    'ask-null-target': [{ kind: 'discussion_agent' as const, agent_type: 'LiteLlm' as const, connection_id: null }],
    'ask-other': [{ kind: 'discussion_agent' as const, agent_type: 'LiteLlm' as const, connection_id: 'conn-other' }],
  };

  it('reads an ordinary reply of a connection-backed discussion', () => {
    const scope = { connectionId: 'conn-1', messageTargets: ordinaryTargets };
    // A trigger with no target at all, and a reply with no trigger.
    expect(lastReplyModel([litellm('r1', 'llm-no-target', 'ask-without-target')], 'LiteLlm', 'default', scope))
      .toBe('llm-no-target');
    expect(lastReplyModel([litellm('r1', 'llm-no-trigger')], 'LiteLlm', 'default', scope)).toBe('llm-no-trigger');
    // A discussion-agent target that names no connection.
    expect(lastReplyModel([litellm('r1', 'llm-plain', 'ask-plain-target')], 'LiteLlm', 'default', scope))
      .toBe('llm-plain');
    expect(lastReplyModel([litellm('r1', 'llm-null', 'ask-null-target')], 'LiteLlm', 'default', scope))
      .toBe('llm-null');
    // No targets map at all.
    expect(lastReplyModel([litellm('r1', 'llm-bare', 'ask-x')], 'LiteLlm', 'default', { connectionId: 'conn-1' }))
      .toBe('llm-bare');
  });

  it('skips the reply to a mention of another connection and reads the ordinary one before it', () => {
    const messages = [litellm('r1', 'llm-own', 'ask-plain-target'), litellm('r2', 'llm-other', 'ask-other')];
    expect(lastReplyModel(messages, 'LiteLlm', 'default', { connectionId: 'conn-1', messageTargets: ordinaryTargets }))
      .toBe('llm-own');
    // The connection unknown (the page knows it for a Custom agent only): no reply is skipped for it.
    expect(lastReplyModel(messages, 'LiteLlm', 'default', { messageTargets: ordinaryTargets })).toBe('llm-other');
  });

  it('the list row and the bubbles read the connection of the discussion the same way', () => {
    const none = { economy_model: null, default_model: null, reasoning_model: null };
    const disc = {
      agent: 'Custom' as const, tier: 'economy' as const,
      messages: [reply('r-a', 'model-of-a', 'ask-a'), reply('r-b', 'model-of-b', 'ask-b')],
      message_targets: targets,
    };
    expect(discussionAgentModel(disc, { connection: connection({ id: 'conn-a', ...none }) }))
      .toEqual({ kind: 'id', id: 'model-of-a' });
    expect(discussionAgentModel(disc)).toEqual({ kind: 'providerDefault' });
    // An ordinary LiteLlm discussion on a connection mapping no model for its mode.
    const ordinary = {
      agent: 'LiteLlm' as const, tier: 'default' as const,
      messages: [litellm('r1', 'llm-plain', 'ask-plain-target'), litellm('r2', 'llm-no-target', 'ask-none'), litellm('r3', 'llm-other', 'ask-other')],
      message_targets: ordinaryTargets,
    };
    expect(discussionAgentModel(ordinary, { connection: connection({ id: 'conn-1', ...none }) }))
      .toEqual({ kind: 'id', id: 'llm-no-target' });
  });
});
