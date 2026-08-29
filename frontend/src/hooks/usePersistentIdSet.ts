import { useCallback, useEffect, useMemo, useState } from 'react';

function readIds(storageKey: string): Set<string> {
  try {
    const parsed = JSON.parse(localStorage.getItem(storageKey) ?? '[]');
    return new Set(Array.isArray(parsed) ? parsed.filter((id): id is string => typeof id === 'string') : []);
  } catch {
    return new Set();
  }
}

/** Shared browser-local persistence for collection favorites.
 *
 * IDs absent from a completed server refresh are excluded and pruned from
 * browser storage.
 */
export function usePersistentIdSet(storageKey: string, availableIds: readonly string[], ready = true) {
  const [storedIds, setStoredIds] = useState<Set<string>>(() => readIds(storageKey));
  const available = useMemo(() => new Set(availableIds), [availableIds]);
  const ids = useMemo(
    () => ready ? new Set([...storedIds].filter(id => available.has(id))) : storedIds,
    [available, ready, storedIds],
  );

  useEffect(() => {
    try {
      localStorage.setItem(storageKey, JSON.stringify([...ids]));
    } catch {
      // Storage can be disabled or full. Favorites remain usable in memory.
    }
  }, [ids, storageKey]);

  const toggle = useCallback((id: string) => {
    setStoredIds(current => {
      const next = ready ? new Set([...current].filter(candidate => available.has(candidate))) : new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, [available, ready]);

  return { ids, toggle };
}
