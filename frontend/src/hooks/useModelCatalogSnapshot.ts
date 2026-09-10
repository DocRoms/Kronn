import { useEffect, useState } from 'react';
import { modelCatalogApi } from '../lib/api';
import type { ModelCatalogSnapshot } from '../types/generated';

/** One read per opening, never per keystroke. Failed reloads preserve identities. */
export function useModelCatalogSnapshot(open: boolean) {
  const [catalog, setCatalog] = useState<ModelCatalogSnapshot | null>(null);
  const [catalogError, setCatalogError] = useState(false);
  useEffect(() => {
    if (!open) return;
    let active = true;
    void modelCatalogApi.list().then(snapshot => {
      if (!active) return;
      setCatalog(snapshot);
      setCatalogError(false);
    }).catch(() => {
      if (!active) return;
      setCatalogError(true);
      setCatalog(previous => previous ? {
        ...previous,
        targets: previous.targets.map(target => ({ ...target, stale: true, live_refresh_ok: false })),
      } : null);
    });
    return () => { active = false; };
  }, [open]);
  return { catalog, catalogError };
}
