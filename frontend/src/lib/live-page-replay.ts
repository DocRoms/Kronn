// KT-1344 — what the sync panel's "Sync the N workflows" will replay.
//
// One definition, two readers: the button label counts it, the click replays
// it. A count worked out anywhere else could promise runs the click then
// skips (a disabled workflow, one already running).
import type { LivePageWorkflowLink } from '../types/generated';

/** The linked workflows a run-all launches: enabled, and not already running. */
export function replayQueue(
  linked: readonly LivePageWorkflowLink[],
  isRunning: (workflowId: string) => boolean,
): LivePageWorkflowLink[] {
  return linked.filter(workflow => workflow.enabled && !isRunning(workflow.id));
}
