import type { TaskExecutionStatus } from '../types/generated';

/** Which controls a state actually allows. Kept as data rather than scattered
 *  conditions: an action offered in the wrong state is a promise the backend
 *  will refuse, and the user only finds out after clicking. */
export function allowedActions(
  status: TaskExecutionStatus,
  interruptedFromStatus: TaskExecutionStatus | null = null,
): {
  approve: boolean;
  requestChanges: boolean;
  stop: boolean;
  reassign: boolean;
} {
  const terminal = status === 'Done' || status === 'Failed' || status === 'Cancelled';
  const reviewable = status === 'AwaitingReview';
  return {
    approve: reviewable,
    requestChanges: reviewable,
    // Stopping a finished execution is not a no-op, it is a wrong idea.
    stop: !terminal,
    // Mirrors `reassign_execution_worker`: a live worker may be replaced, and
    // an Interrupted checkpoint may be resumed with another worker. Blocked
    // and Escalated are policy/human holds, not reassignment states.
    reassign: status === 'Working'
      || status === 'ChangesRequested'
      || (status === 'Interrupted'
        && (interruptedFromStatus === 'Working' || interruptedFromStatus === 'ChangesRequested')),
  };
}
