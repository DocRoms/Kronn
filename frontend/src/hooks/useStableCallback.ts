import { useCallback, useLayoutEffect, useRef } from 'react';

/** A callback whose identity never changes but always runs the latest closure,
 *  so memoized children are not re-rendered because a handler was recreated. */
export function useStableCallback<Args extends unknown[], R>(callback: (...args: Args) => R): (...args: Args) => R {
  const latest = useRef(callback);
  useLayoutEffect(() => {
    latest.current = callback;
  });
  return useCallback((...args: Args) => latest.current(...args), []);
}
