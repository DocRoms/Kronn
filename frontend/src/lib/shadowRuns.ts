/**
 * KT-1449, KT-1459 — step shadows: a step re-runs beside its main execution on
 * the same input. This reads what a run recorded about them: what differs
 * between the executions of a step, what each spent, whether the outputs agree.
 */
import type {
  ExecutionIdentity, MergeStatus, RunStatus, ShadowExecution, ShadowMerge, ShadowMergeReport, ShadowStatus, StepShadow, StepShadowComparison,
  WorkflowRun, WorkflowStep,
} from '../types/generated';

/** The variant of a normal run; a run of the retired workflow-level shadow reads `shadow`. */
export const NORMAL_VARIANT = 'normal';

/** A run recorded by the retired workflow-level shadow (KT-1449): listed, never started again. */
export function isShadowRun(run: WorkflowRun): boolean {
  const value: unknown = run.trigger_context;
  const variant = value && typeof value === 'object' ? (value as Record<string, unknown>).__variant__ : undefined;
  return typeof variant === 'string' && variant !== NORMAL_VARIANT;
}

/** The steps that carry shadows, with how many each. */
export function shadowedSteps(steps: readonly WorkflowStep[] | null | undefined): { step: WorkflowStep; shadows: readonly StepShadow[] }[] {
  return (steps ?? []).filter(step => (step.shadows ?? []).length > 0).map(step => ({ step, shadows: step.shadows ?? [] }));
}

/** How many skill and Quick Prompt versions the shadows of the steps test. */
export function shadowTestedCount(steps: readonly WorkflowStep[] | null | undefined): number {
  return shadowedSteps(steps).reduce((total, { shadows }) => total + shadows.reduce((sum, shadow) => sum
    + Object.keys(shadow.skills ?? {}).length + Object.keys(shadow.quick_prompt_versions ?? {}).length
    + (shadow.prompt?.type === 'quick_prompt' ? 1 : 0), 0), 0);
}

/** Whether the shadow changes nothing of its step. */
export function isIdenticalShadow(shadow: StepShadow): boolean {
  return !shadow.agent && !shadow.prompt
    && Object.keys(shadow.skills ?? {}).length === 0
    && Object.keys(shadow.quick_prompt_versions ?? {}).length === 0;
}

/** The fields a header says may differ between the executions of a step. */
export type IdentityField = 'agent' | 'model' | 'connection' | 'prompt' | 'skills';
export const IDENTITY_FIELDS: readonly IdentityField[] = ['agent', 'model', 'connection', 'prompt', 'skills'];

/** The model an execution ran on: the one it resolved, else the one it was configured with. */
export function modelOf(execution: ShadowExecution): string {
  return execution.model ?? execution.identity.model ?? '';
}

/** What an execution loaded for one field, as text (empty when it names none). */
export function fieldValue(execution: ShadowExecution, field: IdentityField): string {
  const identity: ExecutionIdentity = execution.identity;
  switch (field) {
    case 'agent': return identity.agent;
    case 'model': return modelOf(execution);
    case 'connection': return identity.connection_id ?? '';
    case 'prompt': return identity.prompt;
    case 'skills': return [...(identity.skills ?? [])].sort().join(', ');
  }
}

export interface IdentityDiff {
  /** The fields every execution shares, with their common value (empty ones left out). */
  identical: { field: IdentityField; value: string }[];
  /** The fields on which the executions differ. */
  differing: IdentityField[];
}

/** What differs between the executions of a step, and what they share. */
export function compareIdentities(executions: readonly ShadowExecution[]): IdentityDiff {
  const identical: IdentityDiff['identical'] = [];
  const differing: IdentityField[] = [];
  for (const field of IDENTITY_FIELDS) {
    const values = executions.map(execution => fieldValue(execution, field));
    if (values.every(value => value === values[0])) {
      if (values[0] !== '') identical.push({ field, value: values[0] });
    } else {
      differing.push(field);
    }
  }
  return { identical, differing };
}

/** Whether `execution` loaded exactly what `main` did. */
export function sameIdentity(main: ShadowExecution, execution: ShadowExecution): boolean {
  return IDENTITY_FIELDS.every(field => (field === 'model'
    ? (main.identity.model ?? '') === (execution.identity.model ?? '')
    : fieldValue(main, field) === fieldValue(execution, field)));
}

const ENVELOPE = /---STEP_OUTPUT---\s*([\s\S]*?)\s*---END_STEP_OUTPUT---/;

/** A step output as a person compares it: a JSON envelope pretty-printed one
 *  field per line (so a line diff points at the field), the signal dropped. */
/** Keys in alphabetical order at every depth: two models write the same fields in different orders. */
function withSortedKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(withSortedKeys);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
      .map(([key, v]) => [key, withSortedKeys(v)]));
  }
  return value;
}

export function readableOutput(output: string): string {
  const body = output.replace(/\n?\[SIGNAL: [A-Za-z0-9_]+\]\s*$/, '').trim();
  const envelope = ENVELOPE.exec(body);
  const candidate = envelope ? envelope[1] : body;
  try {
    const parsed: unknown = JSON.parse(candidate);
    if (parsed && typeof parsed === 'object') return JSON.stringify(withSortedKeys(parsed), null, 2);
  } catch {
    // Not JSON: compared as text.
  }
  return body;
}

/** Measurements that differ on every run: an output is not « different » for them. */
const VOLATILE_KEYS = new Set(['duration_ms', 'elapsed_ms', 'started_at', 'finished_at', 'published_at', 'observed_at', 'data_revision']);
const TIMINGS = /\b\d+(?:\.\d+)?\s?(?:ms|s)\b/g;

function withoutMeasurements(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(withoutMeasurements);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value as Record<string, unknown>)
      .filter(([key]) => !VOLATILE_KEYS.has(key))
      .map(([key, v]) => [key, withoutMeasurements(v)]));
  }
  return typeof value === 'string' ? value.replace(TIMINGS, '…') : value;
}

/** The output as compared: timings and timestamps left out. */
export function comparableOutput(output: string): string {
  const text = readableOutput(output);
  try {
    return JSON.stringify(withoutMeasurements(JSON.parse(text)));
  } catch {
    return text.replace(TIMINGS, '…');
  }
}

/** Whether an execution produced an output worth comparing. */
export function hasOutput(execution: ShadowExecution): boolean {
  return (execution.status === 'success' || execution.status === 'over_budget') && execution.output.trim() !== '';
}

export interface Determinism {
  /** Executions compared: the main one and the identical shadows that produced an output. */
  total: number;
  /** How many different outputs they gave. */
  distinct: number;
  /** Identical shadows that produced none (failed, still running, skipped). */
  missing: number;
}

/** For the shadows identical to the main execution: how many distinct outputs
 *  out of N+1. `null` when the step has no identical shadow. */
export function determinism(step: StepShadowComparison): Determinism | null {
  const twins = step.shadows.filter(shadow => sameIdentity(step.main, shadow));
  if (twins.length === 0) return null;
  const compared = [step.main, ...twins].filter(hasOutput);
  return {
    total: compared.length,
    distinct: new Set(compared.map(execution => comparableOutput(execution.output))).size,
    missing: twins.length + 1 - compared.length,
  };
}

/** The label of an execution: `main`'s is the caller's, a shadow's its label, else its id. */
export function shadowName(execution: ShadowExecution, mainLabel: string): string {
  return execution.shadow_id == null ? mainLabel : execution.label || execution.shadow_id;
}

/** `shadowName` followed by the agent and the model that answered. */
export function shadowLabel(execution: ShadowExecution, mainLabel: string): string {
  const name = shadowName(execution, mainLabel);
  // Which agent and model answered: « shadow-2 » alone says nothing in a pick list.
  // The model that answered when the run resolved one; else the one asked for (a tier).
  const who = [execution.identity.agent, execution.model || execution.identity.model].filter(Boolean).join(' · ');
  return who ? `${name} (${who})` : name;
}

/** A stable key for an execution inside its step. */
export const executionKey = (execution: ShadowExecution): string => execution.shadow_id ?? '';

/** `shadow.status.*` key of a stored execution. */
export function shadowStatusKey(status: ShadowStatus): string {
  return `wf.shadow.status.${status}`;
}

/** `run.status.*` key of a run or step status. */
export function runStatusKey(status: RunStatus): string {
  switch (status) {
    case 'Pending': return 'run.status.queued';
    case 'WaitingApproval': return 'run.status.waiting_approval';
    case 'StoppedByGuard': return 'run.status.stopped_by_guard';
    case 'WaitingQuota': return 'run.status.quota';
    default: return `run.status.${status.toLowerCase()}`;
  }
}

/** Cut on a code point, never inside a surrogate pair. */
export function clip(text: string, max: number): { text: string; clipped: boolean } {
  const chars = Array.from(text);
  return chars.length > max ? { text: chars.slice(0, max).join(''), clipped: true } : { text, clipped: false };
}

/** Cost as shown: `null` is unknown, never free. */
export function formatCost(cost: number | null | undefined): string {
  if (cost == null) return '—';
  return `$${cost < 0.01 ? cost.toFixed(4) : cost.toFixed(2)}`;
}

/** The change of `to` against `from`, as « (+12 %) », « (=) » or nothing when `from` is empty. */
export function percent(from: number | null | undefined, to: number | null | undefined): string {
  if (!from || from <= 0 || to == null) return '';
  const delta = Math.round(((to - from) / from) * 100);
  return delta === 0 ? ' (=)' : ` (${delta > 0 ? '+' : '−'}${Math.abs(delta)} %)`;
}

// ─── KT-1544 — the merge of a step's executions ────────────────────────────

/** The execution name the condense pass uses for the main execution. */
export const MERGE_MAIN = 'main';

/** Whether a merge is still going: the comparison keeps refreshing. */
export function mergeInProgress(merge: ShadowMergeReport | undefined | null): boolean {
  return merge?.status === 'waiting' || merge?.status === 'condensing';
}

/** The i18n key naming how a merge stands. */
export function mergeStatusKey(status: MergeStatus): string {
  return `wf.shadow.merge.status.${status}`;
}

/** The label of an execution named in a merge (`main`, `shadow:<id>`). */
export function mergeExecutionLabel(token: string, comparison: StepShadowComparison, mainLabel: string): string {
  if (token === MERGE_MAIN) return shadowLabel(comparison.main, mainLabel);
  const id = token.startsWith('shadow:') ? token.slice('shadow:'.length) : token;
  const shadow = comparison.shadows.find(candidate => candidate.shadow_id === id);
  return shadow ? shadowLabel(shadow, mainLabel) : id;
}

/** What one execution brought to a merge. `raised` is always `inResult + dropped`. */
export interface MergeRow {
  execution: string;
  /** Distinct points it raised, kept or not. */
  raised: number;
  /** Of those, the points that stand in the result: found alone, with others, or reworded. */
  inResult: number;
  /** Of those, the points no other execution found. */
  alone: number;
  /** Raised but left out: no evidence held. */
  dropped: number;
}

export interface MergeTally {
  rows: MergeRow[];
  /** Points of the result. */
  result: number;
  /** Of those, found by a single execution. */
  alone: number;
  /** Distinct points left out. */
  dropped: number;
}

/**
 * The merge read point by point. A point of the result belongs to every
 * execution that found it, not to the first one named: a shadow that confirms
 * three points of the main execution found three points, not zero.
 */
export function mergeTally(report: ShadowMergeReport): MergeTally {
  const byId = new Map(report.points.map(point => [point.id, point]));
  // The kept point a point stands for: itself, or the one it rewords.
  const rootOf = (point: ShadowMergeReport['points'][number]) => {
    let current = point;
    for (let hop = 0; hop < report.points.length && current.verdict === 'duplicate'; hop += 1) {
      const next = current.duplicate_of ? byId.get(current.duplicate_of) : undefined;
      if (!next) return undefined;
      current = next;
    }
    return current.verdict === 'kept' ? current : undefined;
  };
  const finders = new Map<string, Set<string>>();
  const droppedBy = new Map<string, number>();
  let dropped = 0;
  for (const point of report.points) {
    const root = rootOf(point);
    if (!root) {
      dropped += 1;
      for (const execution of point.found_by) droppedBy.set(execution, (droppedBy.get(execution) ?? 0) + 1);
      continue;
    }
    const found = finders.get(root.id) ?? new Set<string>();
    point.found_by.forEach(execution => found.add(execution));
    finders.set(root.id, found);
  }
  const rows = report.contributions.map(({ execution }) => {
    // A point reworded twice by one execution is one point of the result.
    const mine = [...finders.values()].filter(found => found.has(execution));
    const left = droppedBy.get(execution) ?? 0;
    return { execution, raised: mine.length + left, inResult: mine.length, alone: mine.filter(found => found.size === 1).length, dropped: left };
  });
  return { rows, result: finders.size, alone: [...finders.values()].filter(found => found.size === 1).length, dropped };
}

/** The longest wait the backend accepts, in minutes. */
export const MAX_MERGE_MINUTES = 60;
/** The wait when the step sets none, in minutes. */
export const DEFAULT_MERGE_MINUTES = 10;

/** The shadows a merge selects: none named means every shadow of the step. */
export function mergedShadowIds(merge: ShadowMerge | undefined, shadows: readonly StepShadow[]): string[] {
  const named = merge?.shadows ?? [];
  return named.length > 0 ? named : shadows.map(shadow => shadow.id);
}

/** The merge once a shadow was renamed or removed: it names only shadows the step still carries. */
export function pruneMerge(
  merge: ShadowMerge | undefined, before: readonly StepShadow[], after: readonly StepShadow[],
): ShadowMerge | null | undefined {
  if (!merge) return undefined;
  if (after.length === 0) return null;
  const named = merge.shadows ?? [];
  if (named.length === 0) return undefined;
  const renamed = named.map(id => {
    const index = before.findIndex(shadow => shadow.id === id);
    return index >= 0 && after.length === before.length && after[index] ? after[index].id : id;
  });
  const kept = renamed.filter(id => after.some(shadow => shadow.id === id));
  if (kept.length === named.length && kept.every((id, i) => id === named[i])) return undefined;
  return { ...merge, shadows: kept.length === after.length ? [] : kept };
}
