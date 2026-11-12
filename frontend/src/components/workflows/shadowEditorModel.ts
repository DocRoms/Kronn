/**
 * What the step shadow editor shares with the rest of the UI without being a
 * component: the cap on shadows, which steps may carry them, and the versions
 * of a skill or QP.
 */
import { useEffect, useState } from 'react';
import type { WorkflowStep } from '../../types/generated';

export type Versions = { state: 'loading' | 'error' | 'ready'; list: number[] };

export const MAX_SHADOWS = 8;

/** Whether a step may carry shadows: an Agent step that stays inside Kronn. */
export function stepCanCarryShadows(step: WorkflowStep): boolean {
  return (!step.step_type || step.step_type.type === 'Agent') && !step.external_effect;
}

/** Versions of one skill or QP, newest first. */
export function useVersions(load: () => Promise<{ version_index: number }[] | null | undefined>, key: string): Versions {
  const [loaded, setLoaded] = useState<{ key: string; versions: Versions } | null>(null);
  useEffect(() => {
    let cancelled = false;
    load()
      .then(list => {
        const sorted = (list ?? []).map(v => v.version_index).sort((a, b) => b - a);
        if (!cancelled) setLoaded({ key, versions: { state: 'ready', list: sorted } });
      })
      .catch(() => { if (!cancelled) setLoaded({ key, versions: { state: 'error', list: [] } }); });
    return () => { cancelled = true; };
    // `load` is rebuilt each render; the key names what it loads.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return loaded?.key === key ? loaded.versions : { state: 'loading', list: [] };
}
