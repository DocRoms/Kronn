import { useEffect } from 'react';

/** Re-reads a lock state when its lockout ends, so a screen never keeps
 *  counting down a lock that is over (KT-1492: strong confirmation). */
export function useLockExpiry(lockedForSecs: number, reload: () => void) {
  useEffect(() => {
    if (lockedForSecs <= 0) return undefined;
    const timer = window.setTimeout(reload, lockedForSecs * 1000 + 250);
    return () => window.clearTimeout(timer);
  }, [lockedForSecs, reload]);
}
