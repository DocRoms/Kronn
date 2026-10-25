/** KT-1449 — a shadow run carries its variant and its normal run in its trigger context. */
import type { RunStatus, StepResult, WorkflowRun, WorkflowShadow } from '../types/generated';

const SKIPPED_SIGNAL = '[SIGNAL: SKIPPED_SHADOW]';

function context(run: WorkflowRun): Record<string, unknown> {
  const value: unknown = run.trigger_context;
  return value && typeof value === 'object' ? (value as Record<string, unknown>) : {};
}

/** How many skill and Quick Prompt versions a shadow configuration tests. */
export function shadowTestedCount(shadow: WorkflowShadow | null | undefined): number {
  return Object.keys(shadow?.skills ?? {}).length + Object.keys(shadow?.quick_prompt_versions ?? {}).length;
}

export function isShadowRun(run: WorkflowRun): boolean {
  return context(run).__variant__ === 'shadow';
}

/** The normal run a shadow run was started alongside, if any. */
export function shadowOf(run: WorkflowRun): string | null {
  const id = context(run).__shadow_of__;
  return typeof id === 'string' ? id : null;
}

/** Each normal run id → its shadow run, among the runs loaded. */
export function shadowsByNormalRun(runs: readonly WorkflowRun[]): Map<string, WorkflowRun> {
  const map = new Map<string, WorkflowRun>();
  for (const run of runs) {
    const normal = shadowOf(run);
    if (normal) map.set(normal, run);
  }
  return map;
}

/** The shadow run recorded this step without running it (it acts outside Kronn). */
export function isSkippedInShadow(step: StepResult): boolean {
  return step.output.trimEnd().endsWith(SKIPPED_SIGNAL);
}

/** Run list rows carry no step output: the comparison needs the full runs. */
export function needsRunDetail(run: WorkflowRun): boolean {
  return run.step_results.some(step => step.output === '');
}

export type StepVerdict = 'same' | 'different' | 'skipped' | 'missing';

export interface ComparedStep {
  key: string;
  name: string;
  /** 1-based, shown only when a step ran more than once (Goto, retry loop). */
  occurrence: number;
  occurrences: number;
  normal?: StepResult;
  shadow?: StepResult;
  verdict: StepVerdict;
}

export interface RunComparison {
  steps: ComparedStep[];
  counts: Record<StepVerdict, number>;
  tokens: { normal: number; shadow: number };
  durationMs: { normal: number; shadow: number };
}

function occurrences(run: WorkflowRun): Map<string, StepResult[]> {
  const byName = new Map<string, StepResult[]>();
  for (const step of run.step_results) {
    if (step.is_rollback) continue;
    byName.set(step.step_name, [...(byName.get(step.step_name) ?? []), step]);
  }
  return byName;
}

const ENVELOPE = /---STEP_OUTPUT---\s*([\s\S]*?)\s*---END_STEP_OUTPUT---/;

/** A step output as a person compares it: a JSON envelope pretty-printed one
 *  field per line (so a line diff points at the field), the signal dropped. */
export function readableOutput(output: string): string {
  const body = output.replace(/\n?\[SIGNAL: [A-Z_]+\]\s*$/, '').trim();
  const envelope = ENVELOPE.exec(body);
  const candidate = envelope ? envelope[1] : body;
  try {
    const parsed: unknown = JSON.parse(candidate);
    if (parsed && typeof parsed === 'object') return JSON.stringify(parsed, null, 2);
  } catch {
    // Not JSON: compared as text.
  }
  return body;
}

/** Measurements that differ on every run: a step is not « different » for them. */
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

/** The output as compared for the verdict: timings and timestamps left out. */
export function comparableOutput(output: string): string {
  const text = readableOutput(output);
  try {
    return JSON.stringify(withoutMeasurements(JSON.parse(text)));
  } catch {
    return text.replace(TIMINGS, '…');
  }
}

function sameOutput(a: StepResult, b: StepResult): boolean {
  return a.status === b.status && comparableOutput(a.output) === comparableOutput(b.output);
}

/** Step by step, occurrence by occurrence: what each run produced and whether it differs. */
export function compareRuns(normal: WorkflowRun, shadow: WorkflowRun): RunComparison {
  const left = occurrences(normal);
  const right = occurrences(shadow);
  const order = [...new Set([...normal.step_results, ...shadow.step_results].filter(s => !s.is_rollback).map(s => s.step_name))];
  const steps: ComparedStep[] = [];
  for (const name of order) {
    const a = left.get(name) ?? [];
    const b = right.get(name) ?? [];
    const total = Math.max(a.length, b.length);
    for (let i = 0; i < total; i += 1) {
      const n = a[i];
      const s = b[i];
      const verdict: StepVerdict = s && isSkippedInShadow(s) ? 'skipped'
        : !n || !s ? 'missing'
          : sameOutput(n, s) ? 'same' : 'different';
      steps.push({ key: `${name}#${i}`, name, occurrence: i + 1, occurrences: total, normal: n, shadow: s, verdict });
    }
  }
  const counts: Record<StepVerdict, number> = { same: 0, different: 0, skipped: 0, missing: 0 };
  for (const step of steps) counts[step.verdict] += 1;
  const sum = (run: WorkflowRun, pick: (s: StepResult) => number) => run.step_results.reduce((acc, s) => acc + pick(s), 0);
  return {
    steps,
    counts,
    tokens: { normal: normal.tokens_used || sum(normal, s => s.tokens_used ?? 0), shadow: shadow.tokens_used || sum(shadow, s => s.tokens_used ?? 0) },
    durationMs: { normal: sum(normal, s => s.duration_ms), shadow: sum(shadow, s => s.duration_ms) },
  };
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

/** Shadows whose normal run is loaded: they share its row in the run list. */
export function pairedShadows(runs: readonly WorkflowRun[]): Map<string, WorkflowRun> {
  const ids = new Set(runs.map(run => run.id));
  return new Map([...shadowsByNormalRun(runs)].filter(([normalId]) => ids.has(normalId)));
}

/** The runs listed as rows: a paired shadow folds into its normal run, which is
 *  listed when either side matches the filter. An orphan shadow keeps its row. */
export function runListRows(
  runs: readonly WorkflowRun[],
  matching: readonly WorkflowRun[],
  pairs: ReadonlyMap<string, WorkflowRun>,
): WorkflowRun[] {
  const matched = new Set(matching.map(run => run.id));
  const folded = new Set([...pairs.values()].map(run => run.id));
  return runs.filter(run => !folded.has(run.id)
    && (matched.has(run.id) || matched.has(pairs.get(run.id)?.id ?? '')));
}

/** The views of a run row pairing a normal run with its shadow. */
export type PairView = 'compare' | 'normal' | 'shadow';
export const pairTabId = (runId: string, view: PairView) => `wf-run-pair-${runId}-${view}-tab`;
export const pairPanelId = (runId: string) => `wf-run-pair-${runId}-panel`;
