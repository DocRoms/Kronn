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
