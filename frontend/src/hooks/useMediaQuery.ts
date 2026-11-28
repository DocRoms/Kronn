import { useCallback, useSyncExternalStore } from 'react';
import { BREAKPOINTS } from '../styles/designTokens';

/** Returns true when viewport width is below the given breakpoint (default: `--kr-bp-md`, 768px) */
export function useIsMobile(breakpoint: number = BREAKPOINTS.md): boolean {
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
