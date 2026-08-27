// KT-323 DoD-7 — the campaign panel's collapsed state, per campaign run.
//
// Collapsing is a decision about a specific campaign ("I know what is running,
// stop showing me"), so it is keyed by run id rather than global: opening
// another campaign should not inherit the answer given about this one. A reload
// that reopens everything the user deliberately folded away is the same
// annoyance as one that folds away what they were watching.

const STORAGE_KEY_SUFFIX = ':orchCollapsed';
const PLAN_STATE_PREFIX = 'kronn:plan-orchestration:';

export interface PlanOrchestrationState {
  selectedTaskId: string | null;
  viewMode: 'focus' | 'all';
}

export function collapsedKey(runId: string): string {
  return `${runId}${STORAGE_KEY_SUFFIX}`;
}

/** Whether the user folded this campaign away. Absent storage reads as open:
 *  a panel the user never touched should show its content. */
export function readCollapsed(runId: string): boolean {
  try {
    return localStorage.getItem(collapsedKey(runId)) === '1';
  } catch {
    // Private mode, disabled storage, quota — never break the panel over a
    // preference. The default (open) is the safe one.
    return false;
  }
}

export function writeCollapsed(runId: string, collapsed: boolean): void {
  try {
    if (collapsed) {
      localStorage.setItem(collapsedKey(runId), '1');
    } else {
      // Remove rather than store '0', so a campaign that was never folded and
      // one that was unfolded read identically.
      localStorage.removeItem(collapsedKey(runId));
    }
  } catch {
    // Losing the preference is acceptable; failing the interaction is not.
  }
}

export function readPlanOrchestrationState(discussionId: string): PlanOrchestrationState {
  try {
    const parsed = JSON.parse(sessionStorage.getItem(`${PLAN_STATE_PREFIX}${discussionId}`) ?? '{}') as Partial<PlanOrchestrationState>;
    return {
      selectedTaskId: typeof parsed.selectedTaskId === 'string' ? parsed.selectedTaskId : null,
      viewMode: parsed.viewMode === 'all' ? 'all' : 'focus',
    };
  } catch {
    return { selectedTaskId: null, viewMode: 'focus' };
  }
}

export function writePlanOrchestrationState(
  discussionId: string,
  state: PlanOrchestrationState,
): void {
  try {
    sessionStorage.setItem(`${PLAN_STATE_PREFIX}${discussionId}`, JSON.stringify(state));
  } catch {
    // A reload preference must never block the plan itself.
  }
}
