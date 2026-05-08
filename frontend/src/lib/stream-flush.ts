// Stream-flush helper (TD-20260504).
//
// Pure builder for the rAF flush callback used by `DiscussionsPage`'s
// `appendStreamChunk`. Lifted into its own module so we can unit-test
// (a) chunks are merged into the correct discussion's running text,
// (b) the watchdog `noteStreamTick` is fired for every discussion
//     whose stream actually moved.
//
// Without this helper the flush body lives inside a `useRafBatchedStream`
// closure which is awkward to exercise — testing here keeps the
// streaming-state contract explicit.

import type { Dispatch, SetStateAction } from 'react';

/** Build the rAF flush callback wired by `DiscussionsPage`.
 *  Each tick:
 *  - merges the incoming `{discId → newText}` snapshot into the
 *    existing `streamingMap` state (concatenating per discussion);
 *  - calls `noteStreamTick(discId)` for every discussion the snapshot
 *    touched, so the Dashboard-side stale-stream watchdog knows the
 *    channel is alive. */
export function buildStreamingFlush(
  setStreamingMap: Dispatch<SetStateAction<Record<string, string>>>,
  noteStreamTick: (discId: string) => void,
): (snapshot: Record<string, string>) => void {
  return (snapshot) => {
    setStreamingMap((prev) => {
      const next = { ...prev };
      for (const [k, v] of Object.entries(snapshot)) {
        next[k] = (next[k] ?? '') + v;
        noteStreamTick(k);
      }
      return next;
    });
  };
}
