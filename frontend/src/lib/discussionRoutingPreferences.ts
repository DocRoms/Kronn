/**
 * Last explicit reasoning mode selected for each native agent in a discussion.
 *
 * This is a composer convenience, not application configuration: changing a
 * mode from an `@alias` must not rewrite the agent defaults or another room.
 * The durable message target still carries the selected tier when the message
 * is sent; this local preference only decides what the next invocation uses.
 */

import type { AgentType, ModelTier } from '../types/generated';

const KEY_PREFIX = 'kronn:discRouting:';
const VALID_TIERS = new Set<ModelTier>(['economy', 'default', 'reasoning']);

export type DiscussionRoutingPreferences = Partial<Record<AgentType, ModelTier>>;

function storageKey(discussionId: string): string {
  return `${KEY_PREFIX}${discussionId}`;
}

function validated(value: unknown): DiscussionRoutingPreferences {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  return Object.fromEntries(
    Object.entries(value).filter((entry): entry is [AgentType, ModelTier] => (
      typeof entry[1] === 'string' && VALID_TIERS.has(entry[1] as ModelTier)
    )),
  );
}

export function loadDiscussionRoutingPreferences(
  discussionId: string,
): DiscussionRoutingPreferences {
  if (!discussionId) return {};
  try {
    const raw = localStorage.getItem(storageKey(discussionId));
    return raw ? validated(JSON.parse(raw)) : {};
  } catch {
    return {};
  }
}

export function saveDiscussionRoutingPreferences(
  discussionId: string,
  preferences: DiscussionRoutingPreferences,
): void {
  if (!discussionId) return;
  try {
    const safe = validated(preferences);
    if (Object.keys(safe).length === 0) {
      localStorage.removeItem(storageKey(discussionId));
    } else {
      localStorage.setItem(storageKey(discussionId), JSON.stringify(safe));
    }
  } catch {
    // localStorage may be disabled or full. Routing still works for this send.
  }
}

export const DISCUSSION_ROUTING_PREFERENCES_KEY_PREFIX = KEY_PREFIX;
