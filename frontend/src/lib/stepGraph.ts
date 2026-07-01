// Static branch analysis of a workflow's steps: the Goto edges declared via
// each step's `on_result` rules. Powers the #15 mini branch-map so a user can
// see where each Goto jumps (a linear chip row hides these back/forward edges).
import type { WorkflowStep } from '../types/generated';

export interface GotoEdge {
  fromIndex: number;
  fromName: string;
  toIndex: number;   // -1 when the target name doesn't resolve (dangling)
  toName: string;
  label: string;     // the `contains` trigger that fires this Goto
  backward: boolean; // true when the jump goes to an earlier step (a loop)
}

/** Extract every `on_result` Goto as a resolved edge. Non-Goto actions (Stop,
 *  Skip, LoopDetection…) are ignored — this map is about jumps. */
export function computeGotoEdges(steps: WorkflowStep[]): GotoEdge[] {
  const indexByName = new Map<string, number>();
  steps.forEach((s, i) => indexByName.set(s.name, i));

  const edges: GotoEdge[] = [];
  steps.forEach((step, fromIndex) => {
    for (const rule of step.on_result ?? []) {
      if (rule.action?.type !== 'Goto') continue;
      const toName = rule.action.step_name;
      const toIndex = indexByName.has(toName) ? indexByName.get(toName)! : -1;
      edges.push({
        fromIndex,
        fromName: step.name,
        toIndex,
        toName,
        label: rule.contains ?? '',
        backward: toIndex >= 0 && toIndex < fromIndex,
      });
    }
  });
  return edges;
}

/** True when the workflow has any Goto edge — i.e. the branch map is useful. */
export function hasBranches(steps: WorkflowStep[]): boolean {
  return computeGotoEdges(steps).length > 0;
}
