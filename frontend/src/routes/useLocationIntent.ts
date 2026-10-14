import { useCallback, useLayoutEffect, useRef } from 'react';
import { useLocation, useNavigate } from 'react-router';

/**
 * The one-shot intent a navigation left in history state, and a stable way
 * to consume it: the entry is replaced without its state, so a reload or a
 * Back never replays what has already been done.
 */
export function useLocationIntent<T extends object>(): [intent: T | null, consume: () => void] {
  const location = useLocation();
  const navigate = useNavigate();
  // Both read through refs: `consume` keeps its identity for the component's
  // whole life, whatever route it is rendered under.
  const locationRef = useRef(location);
  const navigateRef = useRef(navigate);
  useLayoutEffect(() => { locationRef.current = location; navigateRef.current = navigate; }, [location, navigate]);
  const consume = useCallback(() => {
    const { pathname, search, hash } = locationRef.current;
    void navigateRef.current({ pathname, search, hash }, { replace: true, state: null });
  }, []);
  return [(location.state as T | null) ?? null, consume];
}
