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

/** Default: 15 minutes of silence before a stream counts as dead.
 *
 *  This mirrors the backend's own ceiling — `NON_STREAMING_STALL_TIMEOUT`
 *  (backend/src/api/discussions/mod.rs) gives a non-streaming agent, Codex
 *  `exec` among them, a full 15 minutes of silent stdout before abandoning it.
 *  The previous 5 minutes meant the UI declared dead, every single time, a run
 *  the server was still happily waiting on: any Codex turn past five minutes
 *  got the toast. Two budgets for one run, neither aware of the other.
 *
 *  Below the server's own ceiling this threshold can only ever be wrong; at it,
 *  it fires when the server would give up too. */
export const DEFAULT_STREAM_STALE_MS = 15 * 60 * 1000;

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
  /** Discussion ids the SERVER reports as still running, from the
   *  `getRunning()` poll. Silence is not death: a run spending minutes in tool
   *  calls emits no text, so the local tick never advances — while the server
   *  knows perfectly well it is alive. When the server says a run is in flight,
   *  no local timer may overrule it. Omit when the poll has never answered;
   *  an empty array then means "the server says nothing is running". */
  serverRunningIds?: readonly string[];
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
    if (input.serverRunningIds?.includes(discId)) {
      // The server is the authority on whether work is happening. Asking it
      // costs nothing here — the answer is already polled every 5 s.
      continue;
    }
    if (input.now - lastTick > threshold) {
      stale.push(discId);
    }
  }
  return stale;
}

/** Stop the stale streams the detector found, then let the caller forget them.
 *
 *  The order is the whole point. `cleanupStream` deletes the discussion's entry
 *  from the abort-controller map, so calling it first left the abort reading
 *  `undefined`: the watchdog hid the spinner, announced "connection lost", and
 *  the request kept running. Whatever the server does with an aborted client,
 *  never asking it is the one case that always wastes the run. */
export function abortStaleStreams(
  stale: readonly string[],
  controllers: Record<string, { abort: () => void } | undefined>,
  forget: (discId: string) => void,
): void {
  for (const discId of stale) {
    try {
      controllers[discId]?.abort();
    } catch {
      // An already-aborted controller must not stop us forgetting the rest.
    }
    forget(discId);
  }
}
