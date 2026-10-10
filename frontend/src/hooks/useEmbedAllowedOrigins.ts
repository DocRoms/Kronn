import { useCallback, useEffect, useSyncExternalStore } from 'react';
import { config as configApi } from '../lib/api';
import { useWebSocket } from './useWebSocket';
import { servedFrameOrigins } from '../lib/served-frame-policy';
import type { EmbedOriginsChange, WsMessage } from '../types/generated';

/**
 * The sites Live Pages may embed content from, shared by every Page view, the
 * import dialog and Configuration → Artifacts. One store per tab: a change made
 * in Configuration reaches the Pages of the same tab at once, and every other
 * open tab re-reads it on the backend's `embed_origins_changed` event (and on
 * reconnect or focus, for an event it missed).
 * `null` until the first read succeeds, and nothing is embedded meanwhile.
 */
let origins: ReadonlySet<string> | null = null;
// Set by the backend's change event, cleared only by a list read or change
// that lands after it: until then no player is drawn, even if the read fails.
let suspended = false;
const listeners = new Set<() => void>();

// A read only reflects the server as it was when the read started. Every
// change bumps `generation` when it starts and when it lands, and a read is
// published only if no change started or landed meanwhile and none is in
// flight: a slow read can never undo a revocation or drop a confirmed add.
// Changes are serialized, so their answers land in the order they were made.
let generation = 0;
let changesInFlight = 0;
let changes: Promise<unknown> = Promise.resolve();
let inflight: { generation: number; read: Promise<void> } | null = null;

function publish(next: readonly string[]): void {
  origins = new Set(next);
  suspended = false;
  for (const listener of listeners) listener();
}

export function refreshEmbedAllowedOrigins(): Promise<void> {
  if (inflight && inflight.generation === generation) return inflight.read;
  const startedAt = generation;
  const read = Promise.resolve()
    .then(() => configApi.getEmbedOrigins())
    .then(next => {
      if (startedAt === generation && changesInFlight === 0) publish(next);
    })
    // Keep the last known list; a failed first read keeps embeds hidden.
    .catch(() => {})
    .finally(() => { if (inflight?.read === read) inflight = null; });
  inflight = { generation: startedAt, read };
  return read;
}

/**
 * The list changed on the server through another path (an import that allowed
 * sites): reads already in flight are stale and dropped, and a fresh read
 * publishes the new list.
 */
export function invalidateEmbedAllowedOrigins(): Promise<void> {
  generation += 1;
  return refreshEmbedAllowedOrigins();
}

// Every mounted view receives the same frame: one read per frame, not per view.
let invalidationQueued = false;
function invalidateOnce(): void {
  if (invalidationQueued) return;
  invalidationQueued = true;
  queueMicrotask(() => {
    invalidationQueued = false;
    void invalidateEmbedAllowedOrigins();
  });
}

/** Apply a change on the backend and share the resulting list. */
export function changeEmbedAllowedOrigins(change: EmbedOriginsChange): Promise<string[]> {
  generation += 1;
  changesInFlight += 1;
  const run = changes.then(() => configApi.changeEmbedOrigins(change));
  changes = run.catch(() => {});
  let failed = false;
  return run
    .then(next => {
      publish(next);
      return next;
    }, (error: unknown) => {
      failed = true;
      throw error;
    })
    .finally(() => {
      changesInFlight -= 1;
      generation += 1;
      // A failed change publishes nothing: read the server's list again so
      // reads dropped meanwhile are not lost.
      if (failed && changesInFlight === 0) void refreshEmbedAllowedOrigins();
    });
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

const snapshot = () => origins;
const suspendedSnapshot = () => suspended;

/** The list changed on the server: suspend every player until it is read again. */
function suspendAndInvalidate(): void {
  // Reads already in flight predate the change: none of them may lift it.
  generation += 1;
  if (!suspended) {
    suspended = true;
    for (const listener of listeners) listener();
  }
  invalidateOnce();
}

/** Whether a change was announced and the new list is not read yet. */
export function useEmbedOriginsSuspended(): boolean {
  return useSyncExternalStore(subscribe, suspendedSnapshot, suspendedSnapshot);
}

/** Test-only: forget the shared list. */
export function resetEmbedAllowedOriginsForTests(): void {
  origins = null;
  suspended = false;
  inflight = null;
  generation = 0;
  changesInFlight = 0;
  changes = Promise.resolve();
}

/** Sites this document's CSP was served with, `null` when unknown. */
export function embedOriginsFramableByThisDocument(): ReadonlySet<string> | null {
  return servedFrameOrigins();
}

export function useEmbedAllowedOrigins(): ReadonlySet<string> | null {
  const current = useSyncExternalStore(subscribe, snapshot, snapshot);
  const onMessage = useCallback((message: WsMessage) => {
    if (message.type === 'embed_origins_changed') suspendAndInvalidate();
  }, []);
  // A reconnect may have missed a revocation: read the list again.
  useWebSocket(onMessage, invalidateOnce);
  useEffect(() => {
    void refreshEmbedAllowedOrigins();
    const onVisible = () => { if (document.visibilityState === 'visible') void refreshEmbedAllowedOrigins(); };
    document.addEventListener('visibilitychange', onVisible);
    window.addEventListener('focus', onVisible);
    return () => {
      document.removeEventListener('visibilitychange', onVisible);
      window.removeEventListener('focus', onVisible);
    };
  }, []);
  return current;
}
