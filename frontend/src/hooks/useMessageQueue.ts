import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { MessageTarget } from '../types/generated';

/** A free-text message the user typed while the agent was still streaming. */
export interface QueuedMessage {
  id: string;
  content: string;
  targets?: MessageTarget[];
  targetAll?: boolean;
  replyToMessageId?: string;
}

let _seq = 0;
function nextId(): string {
  _seq += 1;
  return `q${_seq}`;
}

/**
 * CLI-style message queue — type follow-up messages while the agent is still
 * streaming and have them auto-fire as ONE merged message when the response
 * completes.
 *
 * Mirrors {@link useQpChain} (same `sending: true → false` edge-trigger + ref
 * pattern) but holds an ORDERED ARRAY of plain-text parts instead of a single
 * Quick Prompt. On the completion edge ALL pending parts are MERGED into one
 * message (joined by blank lines) and fired as a single turn — so the agent
 * produces ONE response covering every addition, instead of N separate full
 * responses. A part added while that merged turn is streaming starts a fresh
 * pending set for the next edge.
 *
 * Scope: the queue is bound to the ACTIVE discussion and reset when the user
 * switches discussions (a follow-up belongs to the conversation it was typed in).
 *
 * `onFire` is kept in a ref so the caller (DiscussionsPage) doesn't have to
 * memoize its big non-memoized send handler. Clearing the dequeued item BEFORE
 * firing avoids re-triggering the effect into a loop on the next run's edge.
 */
export function useMessageQueue({
  discId,
  sending,
  onFire,
}: {
  discId: string | null;
  sending: boolean;
  onFire: (
    content: string,
    targets?: MessageTarget[],
    targetAll?: boolean,
    replyToMessageId?: string,
  ) => void;
}) {
  const [queue, setQueue] = useState<QueuedMessage[]>([]);
  const queueRef = useRef<QueuedMessage[]>([]);
  const setBoth = useCallback((next: QueuedMessage[]) => {
    queueRef.current = next;
    setQueue(next);
  }, []);

  const onFireRef = useRef(onFire);
  useLayoutEffect(() => {
    onFireRef.current = onFire;
  }, [onFire]);

  // Reset when the active discussion changes — queued follow-ups are bound to
  // the conversation they were typed in, not carried across discussions.
  const prevDiscRef = useRef(discId);
  useEffect(() => {
    if (prevDiscRef.current !== discId) {
      prevDiscRef.current = discId;
      setBoth([]);
    }
  }, [discId, setBoth]);

  // On the sending true→false edge, MERGE all pending parts into one message
  // and fire it as a single turn.
  const prevSendingRef = useRef(false);
  useEffect(() => {
    const wasSending = prevSendingRef.current;
    prevSendingRef.current = sending;
    if (!wasSending || sending) return;
    const q = queueRef.current;
    if (q.length === 0) return;
    // Parts are sent TOGETHER (one turn → one response). Blank-line separator
    // so the agent reads them as a clear multi-point message. Preserve every
    // explicit target in queue order, once each.
    const merged = q.map(m => m.content).join('\n\n');
    const targets = q
      .flatMap(message => message.targets ?? [])
      .filter((target, index, all) => {
        const identity = `${target.kind}:${target.agent_type}:${target.cli_session_id ?? ''}`;
        return all.findIndex(candidate =>
          `${candidate.kind}:${candidate.agent_type}:${candidate.cli_session_id ?? ''}` === identity
        ) === index;
      });
    const targetAll = q.some(message => message.targetAll);
    const replyToMessageId = q.find(m => m.replyToMessageId)?.replyToMessageId;
    // Clear BEFORE firing so the next run's edge doesn't re-read these parts.
    setBoth([]);
    onFireRef.current?.(
      merged,
      targets.length > 0 ? targets : undefined,
      targetAll || undefined,
      replyToMessageId,
    );
  }, [sending, setBoth]);

  const enqueue = useCallback((
    content: string,
    targets?: MessageTarget[],
    targetAll?: boolean,
    replyToMessageId?: string,
  ) => {
    const trimmed = content.trim();
    if (!trimmed) return;
    setBoth([
      ...queueRef.current,
      { id: nextId(), content: trimmed, targets, targetAll, replyToMessageId },
    ]);
  }, [setBoth]);

  const removeQueued = useCallback((id: string) => {
    setBoth(queueRef.current.filter(m => m.id !== id));
  }, [setBoth]);

  const clearQueue = useCallback(() => setBoth([]), [setBoth]);

  return { queue, enqueue, removeQueued, clearQueue };
}
