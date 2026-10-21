// KT-1330 — fold the orchestrator's own room posts behind one marker.
//
// A campaign room fills with review requests, delivery summaries, terminal
// events, bounces and timeouts. "Hide orchestration messages" collapses each
// run of consecutive ones into a single expandable marker, so nothing is
// silently lost and the conversation stays readable.
//
// Display only: the transcript, the agents' read paths and the unread basis
// are decided elsewhere (the backend never filters on this). Pure — no DOM, no
// storage — so the rules are pinned by unit tests.

import type { DiscussionMessage } from '../types/generated';
import { localCalendarDayKey } from './discussionDates';
import type { DiscussionRenderItem } from './discussionMessageGrouping';

/** A render item once orchestration runs are folded. */
export type FoldedRenderItem =
  | DiscussionRenderItem
  | {
      kind: 'orchestration-marker';
      /** Id of the run's first message — stable across re-renders, so the
       *  expanded state survives new messages landing after the run. */
      runId: string;
      messages: DiscussionMessage[];
      expanded: boolean;
    };

/** The backend stamps `kind: 'orchestration'` (and resolves legacy rows to it
 *  on read); the field is omitted for ordinary messages. */
export function isOrchestrationMessage(message: DiscussionMessage): boolean {
  return message.kind === 'orchestration';
}

/** A fenced `kronn-question` card asks the human something. */
const QUESTION_FENCE = /^[ \t]*```+[ \t]*kronn-question\b/m;

/** The campaign parked on a human: `orch-campaign-gate:{run}:{exec}:awaiting:{n}`.
 *  Its release (`:resumed:`) asks nothing and folds like any other post. */
const HUMAN_GATE_ID = /^orch-campaign-gate:[^:]*:[^:]*:awaiting:/;
/** Generated notices that ask a person to decide: an escalation, an exhausted
 * provider quota, disk pressure, a refused integration, a worker that left
 * without delivering, a review waiting too long. */
const HUMAN_DECISION_ID = /^(orch-escalation|provider-quota-escalation|orch-disk-pressure|orch-integration-refused|orch-worker-undelivered|orch-principal-timeout):/;

/**
 * A message that needs the user's action is never hidden: a question card, a
 * message carrying actions the user can still take, or a human gate. Folding
 * it would leave the campaign waiting on something nobody can see.
 */
export function requiresUserAction(
  message: DiscussionMessage,
  hasActions: (messageId: string) => boolean = () => false,
): boolean {
  return QUESTION_FENCE.test(message.content)
    || HUMAN_GATE_ID.test(message.id)
    || HUMAN_DECISION_ID.test(message.id)
    || hasActions(message.id);
}

export interface OrchestrationFoldOptions {
  /** The toggle: `true` folds, `false` returns the items untouched. */
  hide: boolean;
  /** Runs (by first message id) the user opened. */
  expandedRunIds: ReadonlySet<string>;
  /** Messages with an open action attached (see `requiresUserAction`). */
  hasActions?: (messageId: string) => boolean;
  /** Messages that must stay visible regardless (search hit, deep link). */
  forceVisibleIds?: ReadonlySet<string>;
}

/**
 * Replace each run of consecutive hidden orchestration messages by one marker.
 * Anything else — a human or agent message, a tool group, a note, an
 * actionable orchestration message — ends the run. An expanded run keeps its
 * marker (so it can be collapsed again) followed by its messages.
 */
export function foldOrchestrationRuns(
  items: DiscussionRenderItem[],
  options: OrchestrationFoldOptions,
): FoldedRenderItem[] {
  if (!options.hide) return items;
  const { expandedRunIds, hasActions, forceVisibleIds } = options;
  const folded: FoldedRenderItem[] = [];
  let run: DiscussionRenderItem[] = [];

  const flush = () => {
    if (run.length === 0) return;
    const messages = run.map(item => (item as { msg: DiscussionMessage }).msg);
    const runId = messages[0].id;
    const expanded = expandedRunIds.has(runId);
    folded.push({ kind: 'orchestration-marker', runId, messages, expanded });
    if (expanded) folded.push(...run);
    run = [];
  };

  for (const item of items) {
    const foldable = item.kind === 'message'
      && item.msg.channel !== 'note'
      && isOrchestrationMessage(item.msg)
      && !requiresUserAction(item.msg, hasActions)
      && !forceVisibleIds?.has(item.msg.id);
    if (foldable) {
      // A day separator hangs off the first message of an item, so a run never
      // spans two calendar days (same rule as the tool-call fold).
      const previous = run.at(-1);
      const previousDay = previous ? localCalendarDayKey((previous as { msg: DiscussionMessage }).msg.timestamp) : null;
      const currentDay = localCalendarDayKey(item.msg.timestamp);
      if (previousDay !== null && currentDay !== null && previousDay !== currentDay) flush();
      run.push(item);
    } else {
      flush();
      folded.push(item);
    }
  }
  flush();
  return folded;
}
