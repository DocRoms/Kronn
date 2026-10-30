// KT-1490 — what a Page action or a discussion card would fail on.
import { useEffect, useState } from 'react';
import { quickExecs as quickExecsApi, workflows as workflowsApi } from '../lib/api';
import type { DiscussionActionKind } from '../types/generated';
import type { PendingActivation, PendingStepLine } from '../components/QuickExecActivation';

/** What a Page action or a discussion card would fail on: the Quick Exec it
 * runs, or the Quick Execs and Exec lines of the workflow it launches, that
 * still wait for a human. Empty while loading and on any read failure. */
export function usePendingActivations(
  kind: DiscussionActionKind,
  targetId: string,
  targetName: string,
): { items: PendingActivation[]; steps: PendingStepLine[]; reload: () => void } {
  const [items, setItems] = useState<PendingActivation[]>([]);
  const [steps, setSteps] = useState<PendingStepLine[]>([]);
  const [version, setVersion] = useState(0);

  useEffect(() => {
    if (kind !== 'quick_exec' && kind !== 'workflow') return;
    let cancelled = false;
    const load = async () => {
      // The line a notice shows is read here, with what waits, and is the line
      // it sends: an item whose Quick Exec cannot be read is not offered.
      const saved = new Map((await quickExecsApi.list()).map(exec => [exec.id, exec]));
      const withLine = (id: string, name: string, program: string): PendingActivation[] => {
        const exec = saved.get(id);
        return exec ? [{ id, name, program, command: exec.command, args: exec.args }] : [];
      };
      if (kind === 'quick_exec') {
        const pending = await quickExecsApi.pendingApprovals();
        const found = (pending ?? []).filter(item => item.id === targetId);
        return { items: found.flatMap(item => withLine(item.id, targetName, item.program)), steps: [] };
      }
      const readiness = await workflowsApi.readiness(targetId);
      const blockers = (readiness?.blockers ?? []).filter(
        blocker => blocker.kind === 'human_approval' && blocker.reason === 'unmodelled_program',
      );
      const lines: PendingActivation[] = [];
      const stepLines: PendingStepLine[] = [];
      for (const blocker of blockers) {
        if (blocker.quick_exec) {
          lines.push(...withLine(blocker.quick_exec.id, blocker.quick_exec.name, blocker.quick_exec.program));
        } else if (blocker.step) {
          stepLines.push({ workflowId: blocker.workflow_id, step: blocker.step });
        }
      }
      return { items: lines, steps: stepLines };
    };
    Promise.resolve()
      .then(load)
      .then(found => {
        if (cancelled) return;
        setItems(found.items);
        setSteps(found.steps);
      })
      .catch(() => {
        if (cancelled) return;
        setItems([]);
        setSteps([]);
      });
    return () => { cancelled = true; };
  }, [kind, targetId, targetName, version]);

  return { items, steps, reload: () => setVersion(value => value + 1) };
}
