import { useCallback, useEffect, useReducer, useRef } from 'react';

interface Entry {
  value: unknown;
  at: number;
}

// Survives unmounts: reopening a project shows its last result at once.
const cache = new Map<string, Entry>();
// The newest request ticket per key, shared by every mount: a response that is
// not from the newest ticket is stale and must not reach the cache.
const latest = new Map<string, number>();
let tickets = 0;

/** Cache key of a project's overview Git status. */
export const projectGitCacheKey = (projectId: string) => `git-status:${projectId}`;

/** Drop one resource whose last value is no longer true (the branch just changed). */
export function invalidateCachedResource(key: string) {
  cache.delete(key);
  // A request still in flight predates the invalidation.
  latest.set(key, ++tickets);
}

/** Test hook: forget every cached resource. */
export function clearCachedResources() {
  cache.clear();
  latest.clear();
}

interface Options<T> {
  /** `null` disables the resource (nothing is fetched, nothing shown). */
  key: string | null;
  /** `publish` shows an early partial result while the full one is still loading. */
  load: (force: boolean, publish: (value: T) => void) => Promise<T>;
}

/**
 * Stale-while-revalidate: the last known value is returned immediately and
 * refreshed in the background. A failed refresh never discards the value, it
 * only sets `error`.
 */
export function useCachedResource<T>({ key, load }: Options<T>) {
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  const [error, setError] = useReducer((_: boolean, next: boolean) => next, false);
  const [refreshing, setRefreshing] = useReducer((_: boolean, next: boolean) => next, false);
  const loadRef = useRef(load);
  useEffect(() => { loadRef.current = load; });
  const inFlight = useRef<{ key: string; id: number } | null>(null);
  const runs = useRef(0);

  const run = useCallback(async (resource: string, force: boolean) => {
    if (inFlight.current?.key === resource) return;
    const me = { key: resource, id: ++runs.current };
    inFlight.current = me;
    setRefreshing(true);
    setError(false);
    const ticket = ++tickets;
    latest.set(resource, ticket);
    const publish = (value: T) => {
      if (latest.get(resource) !== ticket) return;
      cache.set(resource, { value, at: Date.now() });
      rerender();
    };
    try {
      publish(await loadRef.current(force, publish));
    } catch {
      if (inFlight.current === me) setError(true);
    } finally {
      if (inFlight.current === me) {
        inFlight.current = null;
        setRefreshing(false);
      }
    }
  }, []);

  useEffect(() => {
    if (key === null) return;
    void run(key, false);
    return () => {
      // A result for a project we left must not flip this one's indicators.
      inFlight.current = null;
    };
  }, [key, run]);

  const refresh = useCallback(() => (key === null ? Promise.resolve() : run(key, true)), [key, run]);
  const set = useCallback((value: T) => {
    if (key === null) return;
    latest.set(key, ++tickets);
    cache.set(key, { value, at: Date.now() });
    rerender();
  }, [key]);

  const entry = key === null ? undefined : cache.get(key);
  return {
    data: (entry?.value as T | undefined) ?? null,
    fetchedAt: entry?.at ?? null,
    refreshing,
    error,
    refresh,
    set,
  };
}
