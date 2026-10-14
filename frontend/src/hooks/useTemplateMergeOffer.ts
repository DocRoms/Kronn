import { useCallback, useEffect, useRef, useState } from 'react';
import { projects as projectsApi } from '../lib/api';
import {
  MERGE_CANDIDATE_STATUSES, offersTemplateMerge, templateMergeStatus, type TemplateMergeStatus,
} from '../lib/templateMerge';
import type { AiAuditStatus, TemplateMergeState, TemplateMergeView } from '../types/generated';

export type TemplateMergeMode = 'closed' | 'browse' | 'gate';

export interface TemplateMergeOffer {
  eligible: boolean;
  /** Where the merge stands on disk, from the last plan read. */
  status: TemplateMergeStatus;
  merge: TemplateMergeState | null;
  mode: TemplateMergeMode;
  open: () => void;
  close: () => void;
  /** Holds launch while accepted template commits await integration, or while an optional merge offer is open. */
  gateLaunch: (launch: () => void) => Promise<void>;
  /** Skips an optional offer; accepted commits awaiting integration cannot be bypassed. */
  launchAuditAnyway: () => void;
}

/**
 * Whether "Lancer l'audit" should offer the template v2 merge first, from the
 * read-only plan: its merge state first, then its content.
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
  // Bumped on close: the panel may have started, accepted or abandoned a merge.
  const [reads, setReads] = useState(0);
  const request = useRef<Promise<TemplateMergeView | null> | null>(null);
  const pendingLaunch = useRef<(() => void) | null>(null);
  const planRef = useRef(plan);
  useEffect(() => { planRef.current = plan; }, [plan]);

  useEffect(() => {
    if (!enabled) {
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
  }, [enabled, projectId, reads]);

  const eligible = candidate && offersTemplateMerge(plan);
  const merge = enabled ? plan?.merge ?? null : null;
  const status = templateMergeStatus(merge);

  const gateLaunch = useCallback(async (launch: () => void) => {
    const current = request.current ? await request.current
      : await Promise.resolve(projectsApi.templateMergePlan(projectId)).catch(() => null);
    planRef.current = current ?? null;
    if (current) setPlan(current);
    if ((current?.merge.branch_ahead_commits ?? 0) > 0 || (candidate && !skipped && offersTemplateMerge(current))) {
      pendingLaunch.current = launch;
      setMode('gate');
      return;
    }
    launch();
  }, [projectId, candidate, skipped]);

  const open = useCallback(() => setMode('browse'), []);
  const close = useCallback(() => {
    pendingLaunch.current = null;
    setMode('closed');
    setReads(n => n + 1);
  }, []);
  const launchAuditAnyway = useCallback(() => {
    if ((planRef.current?.merge.branch_ahead_commits ?? 0) > 0) return;
    const launch = pendingLaunch.current;
    pendingLaunch.current = null;
    setSkipped(true);
    setMode('closed');
    launch?.();
  }, []);

  return { eligible, status, merge, mode, open, close, gateLaunch, launchAuditAnyway };
}
