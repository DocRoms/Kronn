// 0.8.6 phase 4 — Pure grouping algorithm for the discussion message list.
//
// Walks `messages` chronologically and folds consecutive `[kronn-internal:]`
// / `[agent-native:]` System messages into one logical "tool group" that
// the renderer wraps in a single collapsible banner. Everything else
// (User / Agent / non-tool System) renders as its own message bubble.
//
// Extracted from DiscussionsPage's inline fold so unit tests can pin the
// algorithm (audit gap #5, 2026-05-22). Before the extraction, the
// grouping ran inside a multi-page JSX block — a refactor could break
// the "tool group renders BEFORE the next non-tool message" position,
// making the disc transcript visually nonsensical, and no test would
// catch it.

import type { DiscussionMessage } from '../types/generated';
import { isKronnToolMessage } from './kronnToolParser';

export type DiscussionRenderItem =
  | {
      kind: 'message';
      msg: DiscussionMessage;
      /** Index in the original messages array — preserved so the
       *  renderer can pass it as the `idx` prop. */
      idx: number;
    }
  | {
      kind: 'tool-group';
      /** Consecutive `[kronn-internal:]` / `[agent-native:]` System
       *  messages bundled into one banner. Always non-empty. */
      messages: DiscussionMessage[];
    };

export interface GroupingOptions {
  /** Caller's hook for hiding the auto-generated User message of
   *  Briefing / Validation / Bootstrap discs. Idx-only predicate so
   *  the pure fn doesn't need to know the disc title heuristics. */
  isAutoPrompt?: (idx: number) => boolean;
}

/**
 * Fold the message list into render items :
 *   - User / Agent / non-tool System → `{kind: 'message', msg, idx}`
 *   - Consecutive `[kronn-internal:]` / `[agent-native:]` System
 *     messages → ONE `{kind: 'tool-group', messages: [...]}` placed
 *     BEFORE the next non-tool message
 *   - A tool group at the very end of the list (no following non-tool
 *     message) is still emitted via a tail flush
 *
 * Pure : no DOM, no side effects, deterministic on input.
 */
export function groupMessagesWithToolFold(
  messages: DiscussionMessage[],
  options: GroupingOptions = {},
): DiscussionRenderItem[] {
  const { isAutoPrompt } = options;
  const items: DiscussionRenderItem[] = [];
  let toolBuffer: DiscussionMessage[] = [];

  const flushToolBuffer = () => {
    if (toolBuffer.length === 0) return;
    items.push({ kind: 'tool-group', messages: toolBuffer });
    toolBuffer = [];
  };

  for (let idx = 0; idx < messages.length; idx++) {
    const msg = messages[idx];
    if (isAutoPrompt?.(idx)) continue;
    if (msg.role === 'System' && isKronnToolMessage(msg.content)) {
      toolBuffer.push(msg);
      continue;
    }
    // Non-tool message → flush the current buffer FIRST (so the banner
    // renders above this message) then push the message itself.
    flushToolBuffer();
    items.push({ kind: 'message', msg, idx });
  }
  // Tail flush — covers the rare edge case of a disc that ends on tool
  // calls without a final agent / user message.
  flushToolBuffer();

  return items;
}
