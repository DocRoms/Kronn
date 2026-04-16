import { useCallback, useRef } from 'react';

/**
 * Buffer high-frequency stream chunks per key and flush them on the next
 * animation frame. Used to collapse dozens of SSE token deltas per second
 * into a single React state update per frame — without this, typing out
 * a long agent response triggers hundreds of re-renders and drops frames.
 *
 * Returns a single `appendChunk(key, text)` function. A flush schedules
 * one `requestAnimationFrame`; subsequent chunks within the same frame
 * just accumulate into the buffer.
 *
 * The `flush` callback receives the whole `{key → concatenatedText}`
 * snapshot and is responsible for merging it into whatever state shape
 * the caller uses (discussion map, orchestration tree, etc.).
 *
 * `flush` does NOT need to be memoized by the caller — we ref it so the
 * identity doesn't matter between renders.
 */
export function useRafBatchedStream(
  flush: (snapshot: Record<string, string>) => void,
) {
  const bufferRef = useRef<Record<string, string>>({});
  const rafIdRef = useRef<number | null>(null);
  const flushRef = useRef(flush);
  flushRef.current = flush;

  const flushBuffer = useCallback(() => {
    rafIdRef.current = null;
    const buf = bufferRef.current;
    if (Object.keys(buf).length === 0) return;
    const snapshot = { ...buf };
    bufferRef.current = {};
    flushRef.current(snapshot);
  }, []);

  const appendChunk = useCallback(
    (key: string, text: string) => {
      bufferRef.current[key] = (bufferRef.current[key] ?? '') + text;
      if (rafIdRef.current === null) {
        rafIdRef.current = requestAnimationFrame(flushBuffer);
      }
    },
    [flushBuffer],
  );

  return appendChunk;
}
