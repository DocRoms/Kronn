import { useEffect, useState } from 'react';
import { workflows as workflowsApi } from '../../lib/api';
import { mergeInProgress } from '../../lib/shadowRuns';
import type { StepShadowComparison } from '../../types/generated';

const POLL_MS = 3000;

/** The comparison of a run's steps; refreshed while a shadow still runs. */
export function useStepShadows(workflowId: string, runId: string, runStatus: string): StepShadowComparison[] {
  const [steps, setSteps] = useState<StepShadowComparison[]>([]);
  const running = steps.some(step => step.shadows.some(shadow => shadow.status === 'running') || mergeInProgress(step.merge));
  // Read on arrival and whenever the run moves on; the shadows outlive the run's own steps.
  useEffect(() => {
    let active = true;
    workflowsApi.stepShadows(workflowId, runId)
      .then(list => { if (active) setSteps(list ?? []); })
      .catch(() => { /* nothing to compare is nothing to show */ });
    return () => { active = false; };
  }, [workflowId, runId, runStatus]);
  // Then every few seconds, for as long as one shadow still runs.
  useEffect(() => {
    if (!running) return undefined;
    let active = true;
    const timer = setInterval(() => {
      workflowsApi.stepShadows(workflowId, runId)
        .then(list => { if (active) setSteps(list ?? []); })
        .catch(() => { /* the next tick tries again */ });
    }, POLL_MS);
    return () => { active = false; clearInterval(timer); };
  }, [workflowId, runId, running]);
  return steps;
}
