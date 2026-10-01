import { useCallback, useState } from 'react';

function readChoice(storageKey: string): boolean | null {
  try {
    const stored = localStorage.getItem(storageKey);
    return stored === '1' ? true : stored === '0' ? false : null;
  } catch {
    return null;
  }
}

/**
 * A foldable block whose state follows a default until the user decides.
 *
 * Only an explicit toggle is written to browser storage, never the default:
 * a block that opens itself while a list is empty must go on opening itself
 * for someone who never touched it, and stay put for someone who did.
 */
export function usePersistentFold(storageKey: string, defaultOpen: boolean) {
  const [choice, setChoice] = useState<boolean | null>(() => readChoice(storageKey));
  const open = choice ?? defaultOpen;

  const toggle = useCallback(() => {
    const next = !open;
    setChoice(next);
    try {
      localStorage.setItem(storageKey, next ? '1' : '0');
    } catch {
      // Storage can be disabled or full — the fold stays usable in memory.
    }
  }, [open, storageKey]);

  return [open, toggle] as const;
}
