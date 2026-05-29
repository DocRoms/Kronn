import { useCallback, useEffect, useRef, useState } from 'react';
import type { AgentType } from '../types/generated';

/** A free-text message the user typed while the agent was still streaming. */
export interface QueuedMessage {
  id: string;
  content: string;
  targetAgent?: AgentType;
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
  onFire: (content: string, targetAgent?: AgentType) => void;
}) {
  const [queue, setQueue] = useState<QueuedMessage[]>([]);
  const queueRef = useRef<QueuedMessage[]>([]);
  const setBoth = useCallback((next: QueuedMessage[]) => {
    queueRef.current = next;
    setQueue(next);
  }, []);

  const onFireRef = useRef(onFire);
  onFireRef.current = onFire;

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
    // so the agent reads them as a clear multi-point message. The target agent
    // is the first explicit @mention among the parts (falls back to the disc
    // default when none).
    const merged = q.map(m => m.content).join('\n\n');
    const targetAgent = q.find(m => m.targetAgent)?.targetAgent;
    // Clear BEFORE firing so the next run's edge doesn't re-read these parts.
    setBoth([]);
    onFireRef.current?.(merged, targetAgent);
  }, [sending, setBoth]);

  const enqueue = useCallback((content: string, targetAgent?: AgentType) => {
    const trimmed = content.trim();
    if (!trimmed) return;
    setBoth([...queueRef.current, { id: nextId(), content: trimmed, targetAgent }]);
  }, [setBoth]);

  const removeQueued = useCallback((id: string) => {
    setBoth(queueRef.current.filter(m => m.id !== id));
  }, [setBoth]);

  const clearQueue = useCallback(() => setBoth([]), [setBoth]);

  return { queue, enqueue, removeQueued, clearQueue };
}
