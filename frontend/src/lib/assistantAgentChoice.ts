// KT-1885 — the agent a configuration assistant (workflow API step, custom API
// form) talks to, as chosen in its shared agent picker.
import type { AgentType, ModelTier, ModelTierConfig } from '../types/generated';

/** An agent or a connection, and a tier (`null`: the agent's own default). */
export interface AssistantAgentChoice {
  agent: AgentType;
  connectionId?: string | null;
  label?: string;
  modelTiers?: ModelTierConfig;
  tier: ModelTier | null;
}

export const sameAssistantChoice = (left: AssistantAgentChoice | null, right: AssistantAgentChoice) =>
  !!left && left.agent === right.agent && (left.connectionId ?? null) === (right.connectionId ?? null) && left.tier === right.tier;

/** What a discussion created for this choice carries: its connection and an explicit tier. */
export function assistantDiscussionTarget(choice: AssistantAgentChoice) {
  return {
    agent: choice.agent,
    ...(choice.connectionId ? { connection_id: choice.connectionId } : {}),
    ...(choice.tier ? { tier: choice.tier } : {}),
  };
}

/**
 * The choice a kept conversation was created with, read back from its
 * discussion: its agent, its connection and its tier. A discussion created
 * without a tier stores `default`, which runs as the assistant's « agent
 * default » (`null`), the choice it opens on.
 */
export function assistantChoiceFromDiscussion(
  fallbackAgent: AgentType,
  discussion: { agent?: AgentType | null; connection_id?: string | null; tier?: ModelTier | null } | null | undefined,
): AssistantAgentChoice {
  const tier = discussion?.tier ?? null;
  return {
    agent: discussion?.agent ?? fallbackAgent,
    connectionId: discussion?.connection_id ?? null,
    tier: tier === 'default' ? null : tier,
  };
}
