import type { DiscussionMessage } from '../types/generated';

/**
 * Find the most recent Agent message in a slice of messages, scanning
 * from the end. Used by the auto-TTS effect: when new messages arrive,
 * the LAST agent reply is the one to read aloud (User/System messages and
 * earlier agent turns are skipped).
 *
 * Extracted from DiscussionsPage so the "which message gets spoken"
 * selection is unit-tested independently of the TTS side-effects.
 *
 * Returns `null` when the slice has no Agent message.
 */
export function findLastAgentMessage(
  messages: DiscussionMessage[],
): DiscussionMessage | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === 'Agent') return messages[i];
  }
  return null;
}
