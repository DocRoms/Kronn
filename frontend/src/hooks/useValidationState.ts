import { useEffect, useState } from 'react';
import { projects as projectsApi } from '../lib/api';
import type { ValidationState } from '../types/generated';

/**
 * KT-1122 — the server's verdict on validating a project's latest audit run:
 * which discussion validates it, whether it finished (durably), whether the
 * server would accept "validate" now and, if not, why. The UI never re-parses
 * the signal to decide that; it reads this.
 *
 * `refreshKey` re-reads it when the observed discussion changes (a new message
 * may carry the signal). `null` while loading or when it cannot be read: the
 * callers then offer nothing rather than an action the server may refuse.
 */
export function useValidationState(
  projectId: string | null | undefined,
  refreshKey: string | number,
): ValidationState | null {
  const [state, setState] = useState<ValidationState | null>(null);
  useEffect(() => {
    // No project, or a narrow API double without the reader: nothing to read
    // (the verdict stays null below, never a stale one of another project).
    if (!projectId || typeof projectsApi.validationState !== 'function') return;
    let live = true;
    projectsApi.validationState(projectId)
      .then(value => { if (live) setState(value); })
      .catch(() => { if (live) setState(null); });
    return () => { live = false; };
  }, [projectId, refreshKey]);
  return state && state.project_id === projectId ? state : null;
}
