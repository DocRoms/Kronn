import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { QuickPrompt } from '../types/generated';
import { quickPrompts as quickPromptsApi } from '../lib/api';

/**
 * QP Chain Phase 1 — queue a Quick Prompt while the agent is streaming so it
 * auto-fires once the current response completes. The hook:
 *
 * 1. Loads all QPs once on mount (fails silently — feature is a nice-to-have).
 * 2. Exposes `chainableQPs` = the subset with no variables (the only ones
 *    that can auto-fire without user input).
 * 3. Watches the `sending: true → false` transition via a ref-tracked
 *    previous value. On that edge, if a QP is queued, it clears the queue
 *    BEFORE calling the caller's send handler — clearing after would re-
 *    trigger the effect on the next run and loop forever.
 *
 * The `onFire` callback is kept in a ref so callers don't have to memoize
 * it (the send handler in DiscussionsPage is intentionally non-memoized
 * because it captures a lot of local state).
 */
export function useQpChain({
  sending,
  onFire,
}: {
  sending: boolean;
  onFire: (msg: string) => void;
}) {
  const [allQuickPrompts, setAllQuickPrompts] = useState<QuickPrompt[]>([]);
  const [queuedQP, setQueuedQP] = useState<QuickPrompt | null>(null);

  // Ref mirror of `queuedQP` so the sending-transition effect reads the
  // latest value without needing to depend on it (depending on it would
  // make the effect fire on every queue/cancel, not just on sending edges).
  const queuedQPRef = useRef<QuickPrompt | null>(null);
  useEffect(() => {
    queuedQPRef.current = queuedQP;
  }, [queuedQP]);

  // Keep the caller's send handler behind a ref so we don't require them
  // to memoize it. The send handler in DiscussionsPage closes over ~20
  // pieces of local state and is recreated on every render on purpose.
  const onFireRef = useRef(onFire);
  onFireRef.current = onFire;

  // Load QPs once.
  useEffect(() => {
    quickPromptsApi
      .list()
      .then(setAllQuickPrompts)
      .catch(() => {});
  }, []);

  // Auto-fire on sending true→false.
  const prevSendingRef = useRef(false);
  useEffect(() => {
    const wasSending = prevSendingRef.current;
    prevSendingRef.current = sending;
    if (!wasSending || sending) return;
    const qp = queuedQPRef.current;
    if (!qp) return;
    // Clear BEFORE firing — otherwise the effect could re-trigger when
    // sending flips back to true→false on the next run.
    setQueuedQP(null);
    queuedQPRef.current = null;
    onFireRef.current?.(qp.prompt_template);
  }, [sending]);

  const chainableQPs = useMemo(
    () => allQuickPrompts.filter(qp => qp.variables.length === 0),
    [allQuickPrompts],
  );

  const queueQP = useCallback((qp: QuickPrompt) => {
    setQueuedQP(qp);
    queuedQPRef.current = qp;
  }, []);

  const cancelQueuedQP = useCallback(() => {
    setQueuedQP(null);
    queuedQPRef.current = null;
  }, []);

  return { chainableQPs, queuedQP, queueQP, cancelQueuedQP };
}
