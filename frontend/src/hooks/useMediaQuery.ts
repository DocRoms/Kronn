import { useCallback, useSyncExternalStore } from 'react';

/** Returns true when viewport width is below the given breakpoint (default: 768px) */
export function useIsMobile(breakpoint = 768): boolean {
  const subscribe = useCallback((notify: () => void) => {
    const mql = window.matchMedia(`(max-width: ${breakpoint - 1}px)`);
    const handler = () => notify();
    mql.addEventListener('change', handler);
    return () => mql.removeEventListener('change', handler);
  }, [breakpoint]);

  const getSnapshot = useCallback(
    () => window.matchMedia(`(max-width: ${breakpoint - 1}px)`).matches,
    [breakpoint],
  );

  return useSyncExternalStore(subscribe, getSnapshot, () => false);
}
