import { useEffect, useState } from 'react';

/** `flag`, but only once it has stayed true for `delayMs`: a state that clears
 *  itself quickly never reaches the screen. */
export function useSustainedFlag(flag: boolean, delayMs: number): boolean {
  const [sustained, setSustained] = useState(false);
  useEffect(() => {
    if (!flag) {
      setSustained(false);
      return;
    }
    const timer = setTimeout(() => setSustained(true), delayMs);
    return () => clearTimeout(timer);
  }, [flag, delayMs]);
  return flag && sustained;
}
