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
 * IDs absent from an authoritative completed server refresh are excluded and
 * pruned from browser storage. Consumers backed by a filtered or paginated
 * list must disable pruning: an omitted item is not evidence that its saved
 * favorite is stale.
 */
export function usePersistentIdSet(
  storageKey: string,
  availableIds: readonly string[],
  ready: boolean,
  pruneUnavailable = true,
) {
  const [storedIds, setStoredIds] = useState<Set<string>>(() => readIds(storageKey));
  const available = useMemo(() => new Set(availableIds), [availableIds]);
  const canPrune = ready && pruneUnavailable;
  const ids = useMemo(
    () => canPrune ? new Set([...storedIds].filter(id => available.has(id))) : storedIds,
    [available, canPrune, storedIds],
  );

  useEffect(() => {
    // An empty list while its request is pending is not evidence that an id is
    // stale. Waiting here prevents a first render from overwriting favorites
    // restored from storage before the collection has loaded.
    if (!ready) return;
    try {
      localStorage.setItem(storageKey, JSON.stringify([...ids]));
    } catch {
      // Storage can be disabled or full. Favorites remain usable in memory.
    }
  }, [ids, ready, storageKey]);

  const toggle = useCallback((id: string) => {
    setStoredIds(current => {
      const next = canPrune ? new Set([...current].filter(candidate => available.has(candidate))) : new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, [available, canPrune]);

  return { ids, toggle };
}
