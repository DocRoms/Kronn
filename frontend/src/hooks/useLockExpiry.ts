import { useEffect, useRef, useState } from 'react';

/** Retries after the first re-read, each one later than the last (250 ms,
 *  500 ms, 1 s, 2 s, 4 s), then it waits for the value to change. */
const MAX_RECHECKS = 5;
const FIRST_RECHECK_MS = 250;

/** Re-reads a lock state when its lockout ends, so a screen never keeps
 *  counting down a lock that is over (KT-1492: strong confirmation).
 *
 *  The backend rounds the remaining time up and a timer can fire early: the
 *  re-read may still say the same number of seconds. React sees no change
 *  then, so the hook re-arms itself with a growing delay — bounded, so a value
 *  that never moves cannot make it poll for ever. */
export function useLockExpiry(lockedForSecs: number, reload: () => void) {
  const [rechecks, setRechecks] = useState(0);
  const seen = useRef(lockedForSecs);
  const attempts = useRef(0);
  // The latest callback, read when the timer fires: a caller that passes a new
  // function on every render must not re-arm the timer (and shorten the
  // back-off) each time it re-renders.
  const reloadRef = useRef(reload);
  useEffect(() => {
    reloadRef.current = reload;
  }, [reload]);

  useEffect(() => {
    if (seen.current !== lockedForSecs) {
      // A new value is a real change: start over.
      seen.current = lockedForSecs;
      attempts.current = 0;
    }
    if (lockedForSecs <= 0) {
      attempts.current = 0;
      return undefined;
    }
    if (attempts.current > MAX_RECHECKS) return undefined;
    const delay =
      attempts.current === 0
        ? lockedForSecs * 1000 + FIRST_RECHECK_MS
        : FIRST_RECHECK_MS * 2 ** (attempts.current - 1);
    const timer = window.setTimeout(() => {
      attempts.current += 1;
      reloadRef.current();
      setRechecks((count) => count + 1);
    }, delay);
    return () => window.clearTimeout(timer);
  }, [lockedForSecs, rechecks]);
}
