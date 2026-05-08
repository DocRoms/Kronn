// Stale-stream watchdog (TD-20260504).
//
// Pure helper that picks out discussions whose stream has stalled long
// enough to be considered dead (laptop suspend / Docker restart / WS
// channel break). The Dashboard orchestrates the consequences (clear
// sendingMap, refetch discussions, toast); this module just decides
// "who's stale".
//
// Extracting the decision keeps Dashboard's effect simple and gives
// us a tiny pure surface to unit-test exhaustively without mounting
// the React tree.

/** Default: 5 minutes between activity events before we consider a
 *  stream dead. Chosen to be longer than any plausible legit gap
 *  between chunks, even on slow LLMs / contended hosts. */
export const DEFAULT_STREAM_STALE_MS = 5 * 60 * 1000;

export interface StaleStreamInputs {
  /** Per-discussion "is the spinner currently spinning?" flag. */
  sendingMap: Record<string, boolean>;
  /** Per-discussion timestamp of the LAST observed chunk. May be
   *  absent if the stream never produced anything. */
  lastTickMap: Record<string, number>;
  /** Per-discussion timestamp of when sending started. Used as the
   *  fallback "time of last activity" when no chunk has arrived yet. */
  sendingStartMap: Record<string, number>;
  /** Wall-clock now, in ms — passed in (rather than read internally)
   *  so tests can drive arbitrary time scenarios. */
  now: number;
  /** Override threshold (defaults to `DEFAULT_STREAM_STALE_MS`). */
  thresholdMs?: number;
}

/** Returns the discussion ids whose stream has gone silent for longer
 *  than the threshold. Order is undefined; the caller should treat it
 *  as a set. */
export function detectStaleStreams(input: StaleStreamInputs): string[] {
  const threshold = input.thresholdMs ?? DEFAULT_STREAM_STALE_MS;
  const stale: string[] = [];
  for (const [discId, isSending] of Object.entries(input.sendingMap)) {
    if (!isSending) continue;
    const lastTick = input.lastTickMap[discId] ?? input.sendingStartMap[discId];
    if (lastTick == null) {
      // Spinner is on but we have no record of when it started —
      // can't decide, leave alone (observability bug, not a stuck UI).
      continue;
    }
    if (input.now - lastTick > threshold) {
      stale.push(discId);
    }
  }
  return stale;
}
