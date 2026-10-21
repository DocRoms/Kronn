import { useCallback, useEffect, useRef, useState } from 'react';
import { projects as projectsApi } from '../lib/api';
import { MERGE_CANDIDATE_STATUSES, offersTemplateMerge } from '../lib/templateMerge';
import type { AiAuditStatus, TemplateMergeView } from '../types/generated';

export type TemplateMergeMode = 'closed' | 'browse' | 'gate';

export interface TemplateMergeOffer {
  eligible: boolean;
  mode: TemplateMergeMode;
  open: () => void;
  close: () => void;
  /** Runs `launch` at once, unless the repo is documented and not merged: then the merge panel opens first. */
  gateLaunch: (launch: () => void) => Promise<void>;
  /** From the gate: run the audit that was held back, and stop offering for this session. */
  launchAuditAnyway: () => void;
}

/**
 * Whether "Lancer l'audit" should offer the template v2 merge first. The
 * backend has no "documented / merged" flag, so the read-only plan decides.
 */
export function useTemplateMergeOffer(
  projectId: string,
  auditStatus: AiAuditStatus,
  enabled: boolean,
): TemplateMergeOffer {
  const candidate = enabled && MERGE_CANDIDATE_STATUSES.includes(auditStatus);
  const [plan, setPlan] = useState<TemplateMergeView | null>(null);
  const [mode, setMode] = useState<TemplateMergeMode>('closed');
  const [skipped, setSkipped] = useState(false);
  const request = useRef<Promise<TemplateMergeView | null> | null>(null);
  const pendingLaunch = useRef<(() => void) | null>(null);
  const planRef = useRef(plan);
  useEffect(() => { planRef.current = plan; }, [plan]);

  useEffect(() => {
    if (!candidate) {
      request.current = null;
      return;
    }
    let cancelled = false;
    // A read failure only means "no offer": the audit must stay launchable.
    const pending = Promise.resolve()
      .then(() => projectsApi.templateMergePlan(projectId))
      .then(p => p ?? null, () => null);
    request.current = pending;
    // No state update when nothing changed: most projects read back no plan at all.
    void pending.then(p => { if (!cancelled && (p !== null || planRef.current !== null)) setPlan(p); });
    return () => { cancelled = true; };
  }, [candidate, projectId]);

  const eligible = candidate && offersTemplateMerge(plan);

  const gateLaunch = useCallback(async (launch: () => void) => {
    if (!candidate || skipped) { launch(); return; }
    const current = request.current ? await request.current : plan;
    if (offersTemplateMerge(current)) {
      pendingLaunch.current = launch;
      setMode('gate');
      return;
    }
    launch();
  }, [candidate, skipped, plan]);

  const open = useCallback(() => setMode('browse'), []);
  const close = useCallback(() => {
    pendingLaunch.current = null;
    setMode('closed');
  }, []);
  const launchAuditAnyway = useCallback(() => {
    const launch = pendingLaunch.current;
    pendingLaunch.current = null;
    setSkipped(true);
    setMode('closed');
    launch?.();
  }, []);

  return { eligible, mode, open, close, gateLaunch, launchAuditAnyway };
}
