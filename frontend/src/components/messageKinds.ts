import { createContext, type ReactNode } from 'react';

/** KT-1902 — every message of a discussion is one of these. */
export type MessageVariant =
  | 'user'
  | 'agent'
  | 'system'
  | 'error'
  | 'orchestrator'
  | 'streaming'
  | 'queued'
  | 'note'
  | 'deleted'
  | 'media';

export type MessageActionKind = 'reply' | 'copy' | 'listen' | 'edit' | 'retry' | 'delete';

/** The footer order. Edit (a user message) and Retry (an agent reply) share
 *  one place; listening sits with Copy, both read the message. An action that
 *  does not apply is absent: the others never move into its place. */
export const MESSAGE_ACTION_ORDER: readonly MessageActionKind[] = ['reply', 'copy', 'listen', 'edit', 'retry', 'delete'];

export interface MessageAction {
  /** The accessible name, the tooltip and the menu entry. */
  label: string;
  icon: ReactNode;
  onSelect: () => void;
  disabled?: boolean;
  className?: string;
  testId?: string;
  /** Extra state for the stylesheet (`data-state`), e.g. a copy done. */
  state?: string;
}

export type MessageActionSet = Partial<Record<MessageActionKind, MessageAction>>;

/** The actions that apply, in the footer order. */
export function orderedActions(actions: MessageActionSet): Array<[MessageActionKind, MessageAction]> {
  return MESSAGE_ACTION_ORDER.flatMap(kind => {
    const action = actions[kind];
    return action ? [[kind, action] as [MessageActionKind, MessageAction]] : [];
  });
}

/** Rendered height (px) above which a message folds behind « Show more ». */
export const MESSAGE_FOLD_THRESHOLD_PX = 640;

/** The messages unfolded in the open thread. A page provides one per
 *  discussion, so a message keeps its state while the thread stays open
 *  (it is not stored: a reload folds again). */
export const MessageFoldContext = createContext<Set<string> | null>(null);

/** Under the first breakpoint, actions move into the message's menu. The page
 *  says when, once, rather than each message listening to the viewport. */
export const MessageCompactContext = createContext(false);
