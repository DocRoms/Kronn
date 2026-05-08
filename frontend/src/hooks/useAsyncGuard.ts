import { useRef, useCallback } from 'react';

/**
 * Wraps an async fn with a synchronous re-entry guard backed by a ref.
 *
 * `disabled={busy}` on a button is closure-stale between two synchronous
 * clicks — React hasn't re-rendered yet, so the second click reads the
 * pre-update value of `busy` and fires a second invocation. This produces
 * duplicate POSTs (discussions, skills, contacts, …) and racing state
 * updates. The pattern is documented in feedback_race_guards.md.
 *
 * Usage:
 * ```ts
 * const handleSave = useAsyncGuard(async (req: SkillRequest) => {
 *   await skillsApi.create(req);
 *   refetch();
 * });
 * <button onClick={() => handleSave(req)}>Save</button>
 * ```
 *
 * The returned callback resolves to `undefined` on a re-entry hit so callers
 * that don't `.then()` work as expected. If you need to know whether the
 * call actually fired, check the return value.
 */
export function useAsyncGuard<TArgs extends unknown[], TResult>(
  fn: (...args: TArgs) => Promise<TResult>,
): (...args: TArgs) => Promise<TResult | undefined> {
  const inFlightRef = useRef(false);
  return useCallback(async (...args: TArgs): Promise<TResult | undefined> => {
    if (inFlightRef.current) return undefined;
    inFlightRef.current = true;
    try {
      return await fn(...args);
    } finally {
      inFlightRef.current = false;
    }
    // Intentionally do not depend on `fn` — the ref protects across renders
    // even if the caller passes a fresh arrow function each time. Recreating
    // the callback on every render would also recreate the ref-binding,
    // which is harmless but noisy in deps lists.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}
