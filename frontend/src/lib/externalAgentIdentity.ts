import type { ExternalApiConnectionView } from './api';
import type {
  AgentType,
  Discussion,
  DiscussionDetail,
  MessageTarget,
  ModelTierConfig,
} from '../types/generated';

export type DiscussionWithMessageTargets = Discussion
  & Partial<Pick<DiscussionDetail, 'message_targets'>>;

export interface ExternalAgentTarget {
  agent: AgentType;
  connectionId: string;
  label: string;
  trigger: string;
  modelTiers: ModelTierConfig;
}

/** Dynamic OpenAI-compatible connections use AgentType::Custom on the wire.
 * Native LiteLLM/NVIDIA configurations already have their own AgentType and
 * must not be duplicated in selectors. */
export function externalAgentTargets(
  connections: ExternalApiConnectionView[],
): ExternalAgentTarget[] {
  return connections
    .filter(connection => (
      (connection.origin_preset === 'open_router' || connection.origin_preset === 'other')
      && Boolean(connection.endpoint)
      && Boolean(connection.economy_model || connection.default_model || connection.reasoning_model)
    ))
    .map(connection => ({
      agent: 'Custom',
      connectionId: connection.id,
      label: connection.display_name,
      trigger: `@${connection.mention_alias}`,
      modelTiers: {
        economy: connection.economy_model,
        default: connection.default_model,
        reasoning: connection.reasoning_model,
      },
    }));
}

/** Resolve the durable connection identity from message routing metadata.
 * This deliberately does not parse the discussion title: titles are editable
 * user content, while MessageTarget.connection_id is the execution contract. */
export function discussionConnectionId(
  discussion: DiscussionWithMessageTargets,
): string | null {
  if (discussion.agent !== 'Custom') return null;

  const targetsForMessage = (messageId: string): MessageTarget[] =>
    discussion.message_targets?.[messageId] ?? [];
  for (const message of discussion.messages) {
    const target = targetsForMessage(message.id).find(candidate => (
      candidate.agent_type === 'Custom'
      && candidate.kind === 'discussion_agent'
      && Boolean(candidate.connection_id)
    ));
    if (target?.connection_id) return target.connection_id;
  }

  for (const targets of Object.values(discussion.message_targets ?? {})) {
    const target = targets.find(candidate => (
      candidate.agent_type === 'Custom' && Boolean(candidate.connection_id)
    ));
    if (target?.connection_id) return target.connection_id;
  }
  return null;
}

export function externalConnectionForDiscussion(
  discussion: DiscussionWithMessageTargets,
  connections: ExternalApiConnectionView[],
): ExternalApiConnectionView | null {
  const connectionId = discussionConnectionId(discussion);
  return connectionId
    ? connections.find(connection => connection.id === connectionId) ?? null
    : null;
}

/** Brand colour for a message written by an external connection.
 *
 *  `Custom` is the wire type of EVERY external connection, so AGENT_COLORS
 *  cannot hold it — it would need one entry per connection the operator
 *  creates. Left alone, every provider rendered in the same fallback violet
 *  and two of them were indistinguishable at a glance.
 *
 *  The hue is derived from the alias rather than stored: stable for a given
 *  connection, different between two, and needing no new field. Saturation and
 *  lightness are fixed so every generated hue stays legible on both themes
 *  instead of landing wherever the hash fell.
 *
 *  Returns null when there is no alias to derive from, so the caller keeps its
 *  existing fallback rather than inventing a colour from nothing. */
export function externalAgentColor(alias: string | null | undefined): string | null {
  const seed = alias?.trim().replace(/^@/, '') ?? '';
  if (!seed) return null;
  let hue = 0;
  for (const char of seed) {
    hue = (hue * 31 + char.codePointAt(0)!) % 360;
  }
  return `hsl(${hue}, 62%, 52%)`;
}
