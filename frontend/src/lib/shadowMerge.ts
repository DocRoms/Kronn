/**
 * KT-1696 — the shadows of a step and their merge, edited from the workflow
 * overview. Every edit returns the new shadows and the merge to write with
 * them, so a toggle is one save.
 *
 * The merge is always written as an explicit `shadows` list: an empty list with
 * `enabled` set would mean « every shadow » to the backend. The merge is on iff
 * the list is not empty; with no shadow merged it is switched off and keeps its
 * other settings (condense agent, timeout).
 */
import { mergedShadowIds } from './shadowRuns';
import type { ShadowMerge, StepShadow, StepShadowComparison, WorkflowStep } from '../types/generated';

/** What an edit writes: `merge` is left alone when `undefined`, removed when `null`. */
export interface ShadowEdit {
  shadows: StepShadow[];
  merge: ShadowMerge | null | undefined;
}

/** The shadows whose answer goes into the step result: none while the merge is off. */
export function activeMergedIds(merge: ShadowMerge | undefined, shadows: readonly StepShadow[]): string[] {
  if (!merge || merge.enabled === false) return [];
  const known = new Set(shadows.map(shadow => shadow.id));
  return mergedShadowIds(merge, shadows).filter(id => known.has(id));
}

/** The merge written for the ids that merge; off, with its settings kept, when there is none. */
function explicitMerge(merge: ShadowMerge | undefined, ids: readonly string[]): ShadowMerge | undefined {
  if (!merge && ids.length === 0) return undefined;
  return { ...(merge ?? {}), enabled: ids.length > 0, shadows: [...ids] };
}

/** The merge once the shadows changed: it follows a rename, forgets a removal, and stays explicit. */
function settleMerge(
  merge: ShadowMerge | undefined, before: readonly StepShadow[], after: readonly StepShadow[],
): ShadowMerge | null | undefined {
  if (!merge) return undefined;
  if (after.length === 0) return null;
  const sameShape = after.length === before.length;
  const ids = activeMergedIds(merge, before)
    .map(id => {
      const index = before.findIndex(shadow => shadow.id === id);
      return sameShape && index >= 0 ? after[index].id : id;
    })
    .filter(id => after.some(shadow => shadow.id === id));
  return explicitMerge(merge, ids);
}

/** Replace one shadow, found by its current id. */
export function withShadowChanged(
  shadows: readonly StepShadow[], merge: ShadowMerge | undefined, id: string, next: StepShadow,
): ShadowEdit {
  const after = shadows.map(shadow => (shadow.id === id ? next : shadow));
  return { shadows: after, merge: settleMerge(merge, shadows, after) };
}

/** Add a shadow: it runs, and is compared only until its Merge is turned on. */
export function withShadowAdded(
  shadows: readonly StepShadow[], merge: ShadowMerge | undefined, added: StepShadow,
): ShadowEdit {
  const after = [...shadows, added];
  return { shadows: after, merge: settleMerge(merge, shadows, after) };
}

export function withShadowRemoved(
  shadows: readonly StepShadow[], merge: ShadowMerge | undefined, id: string,
): ShadowEdit {
  const after = shadows.filter(shadow => shadow.id !== id);
  return { shadows: after, merge: settleMerge(merge, shadows, after) };
}

/** Whether a shadow may be paused: a shadow that merges always runs. */
export function canPauseShadow(merge: ShadowMerge | undefined, shadows: readonly StepShadow[], id: string): boolean {
  return !activeMergedIds(merge, shadows).includes(id);
}

/** Pause or resume a shadow. Pausing one that merges is refused: the edit changes nothing. */
export function withShadowEnabled(
  shadows: readonly StepShadow[], merge: ShadowMerge | undefined, id: string, enabled: boolean,
): ShadowEdit {
  if (!enabled && !canPauseShadow(merge, shadows, id)) return { shadows: [...shadows], merge: undefined };
  const after = shadows.map(shadow => (shadow.id === id ? { ...shadow, enabled } : shadow));
  return { shadows: after, merge: settleMerge(merge, shadows, after) };
}

/**
 * Put a shadow's answer into the step result, or take it out. Turning it on
 * also resumes a paused shadow; turning off the last one switches the merge off.
 */
export function withShadowMerged(
  shadows: readonly StepShadow[], merge: ShadowMerge | undefined, id: string, on: boolean,
): ShadowEdit {
  if (!shadows.some(shadow => shadow.id === id)) return { shadows: [...shadows], merge: undefined };
  const current = activeMergedIds(merge, shadows);
  const ids = shadows.map(shadow => shadow.id).filter(other => (other === id ? on : current.includes(other)));
  const after = shadows.map(shadow => (on && shadow.id === id && shadow.enabled === false ? { ...shadow, enabled: true } : shadow));
  return { shadows: after, merge: explicitMerge(merge, ids) };
}

/** The agent that condenses the merge; `undefined` runs it on the step's own agent. */
export function withMergeAgent(
  shadows: readonly StepShadow[], merge: ShadowMerge | undefined, agent: ShadowMerge['agent'],
): ShadowEdit {
  return { shadows: [...shadows], merge: merge ? { ...merge, agent } : undefined };
}

/** A shadow id nobody uses yet in the step: `shadow-1`, `shadow-2`… */
export function freshShadowId(shadows: readonly StepShadow[]): string {
  for (let n = shadows.length + 1; ; n += 1) {
    if (!shadows.some(shadow => shadow.id === `shadow-${n}`)) return `shadow-${n}`;
  }
}

/** The transport an agent runs through, as the fallback chain badges it. */
export type AgentTransport = 'cli' | 'http' | 'local';

export function agentTransport(agent: string, connectionId?: string | null): AgentTransport {
  if (agent === 'Ollama') return 'local';
  if (connectionId || agent === 'LiteLlm' || agent === 'Nvidia' || agent === 'Custom') return 'http';
  return 'cli';
}

/**
 * What a run's list row says of its shadows: how many ran, and how their merge
 * went: `ok` when every merge produced its result with all the shadows it waited
 * for, `partial` when one did not (a shadow missing, a merge refused or failed).
 */
export function runShadowSummary(comparisons: readonly StepShadowComparison[]): { shadows: number; merge: 'ok' | 'partial' | null } | null {
  const shadows = comparisons.reduce((total, step) => total + step.shadows.length, 0);
  if (shadows === 0) return null;
  const reports = comparisons.flatMap(step => (step.merge ? [step.merge] : []));
  if (reports.length === 0) return { shadows, merge: null };
  const whole = reports.every(report => report.status === 'merged' && report.missing.length === 0);
  return { shadows, merge: whole ? 'ok' : 'partial' };
}

/** What the header button counts: live shadows, the steps that carry them, whether any merges. */
export function shadowTally(steps: readonly WorkflowStep[] | null | undefined): { shadows: number; steps: number; merged: boolean } {
  let shadows = 0;
  let doubled = 0;
  let merged = false;
  for (const step of steps ?? []) {
    const own = step.shadows ?? [];
    const live = own.filter(shadow => shadow.enabled !== false).length;
    if (live === 0) continue;
    shadows += live;
    doubled += 1;
    if (activeMergedIds(step.shadow_merge, own).length > 0) merged = true;
  }
  return { shadows, steps: doubled, merged };
}
