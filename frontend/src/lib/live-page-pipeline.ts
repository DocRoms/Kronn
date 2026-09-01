import type { RunStatus, StepResult, WorkflowRun } from '../types/generated';

/**
 * Client-side reshape of a workflow run into the `pipeline` shape a Live Page
 * renders. The backend can't express this fold (TransformData has no array→array
 * map, and JsonData can't template), so a binding carries the declarative
 * `phase_map` / `meta_map` and the trusted parent applies this pure function on
 * each refresh. See docs/architecture/live-page-workflow-interaction.md.
 */

export type PipelineStepStatus = 'done' | 'wait' | 'current' | 'pending' | 'failed';

export interface PipelineStep {
  n: string;
  tag?: string;
  s: PipelineStepStatus;
  d: string;
  at?: string;
  dur?: string;
  link?: string;
}

export interface PipelinePhase {
  name: string;
  emoji?: string;
  steps: PipelineStep[];
}

export interface Pipeline {
  meta: Record<string, unknown>;
  phases: PipelinePhase[];
}

/** One phase of the dashboard and the run steps it groups. */
export interface PhaseMapEntry {
  name: string;
  emoji?: string;
  steps: Array<{ step: string; tag?: string; label?: string; link?: string }>;
}

/**
 * `pipeline.meta` field → source spec. A spec is `trigger.<key>` (read from the
 * run's trigger_context), `run.<field>` (id/status/started_at), or, when it
 * matches no prefix, a literal string. `runUrl` falls back to the current origin.
 */
export type MetaMap = Record<string, string>;

const TERMINAL_FAILURE: ReadonlySet<RunStatus> = new Set<RunStatus>([
  'Failed', 'Cancelled', 'StoppedByGuard', 'Interrupted',
]);

export function mapStepStatus(status: RunStatus): PipelineStepStatus {
  if (status === 'Success' || status === 'Partial') return 'done';
  if (status === 'WaitingApproval') return 'wait';
  if (status === 'Running') return 'current';
  if (TERMINAL_FAILURE.has(status)) return 'failed';
  return 'pending'; // Pending or anything unforeseen
}

/** True for step-output envelope delimiters like `---STEP_OUTPUT---`. */
function isEnvelopeMarker(line: string): boolean {
  return /^-{3,}[A-Z0-9_ ]*-{3,}$/.test(line);
}

function firstLine(text: string, max = 140): string {
  const line = (text ?? '')
    .split('\n')
    .map(candidate => candidate.trim())
    .find(candidate => candidate.length > 0 && !isEnvelopeMarker(candidate)) ?? '';
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

/** First http(s) URL in a step's output, if any — surfaced as the step's link. */
function firstUrl(text: string): string | undefined {
  const match = /https?:\/\/[^\s"'<>)]+/.exec(text ?? '');
  return match ? match[0] : undefined;
}

/** Strip envelope-marker lines and trim, preserving the remaining line breaks. */
function cleanBlock(text: string): string {
  return (text ?? '')
    .split('\n')
    .filter(line => !isEnvelopeMarker(line.trim()))
    .join('\n')
    .trim();
}

/**
 * Human-readable description for a step: the step-output envelope's own `summary`
 * when present (so an ApiCall shows "GET … → object" instead of a JSON wall),
 * else the raw output. `full` keeps the whole multi-line block (used for a gate
 * awaiting a decision, whose message — e.g. a ready-to-copy announcement — must
 * be shown in full); otherwise it collapses to the first meaningful line.
 */
function stepDescription(output: string, full = false): string {
  const json = stepOutputJson(output);
  if (json && typeof json === 'object') {
    const summary = (json as Record<string, unknown>).summary;
    if (typeof summary === 'string' && summary.trim()) {
      return full ? cleanBlock(summary) : firstLine(summary);
    }
  }
  return full ? cleanBlock(output) : firstLine(output);
}

/** `2026-08-28T12:41:03Z` → `12:41` (local clock). Empty when unparseable. */
function clock(iso: string | null | undefined): string | undefined {
  if (!iso) return undefined;
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return undefined;
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
}

/** Human duration: `3s`, `8m`, `1h12`. Undefined for non-positive input. */
function duration(ms: number | null | undefined): string | undefined {
  if (!ms || ms <= 0) return undefined;
  const totalSeconds = Math.round(ms / 1000);
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const minutes = Math.floor(totalSeconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const restMinutes = minutes % 60;
  return restMinutes ? `${hours}h${String(restMinutes).padStart(2, '0')}` : `${hours}h`;
}

/** Extract the JSON envelope a step wraps in `---STEP_OUTPUT---` … `---END…`. */
function stepOutputJson(output: string | undefined): unknown {
  if (!output) return undefined;
  let body = output;
  const start = output.indexOf('---STEP_OUTPUT---');
  if (start >= 0) {
    body = output.slice(start + '---STEP_OUTPUT---'.length);
    const end = body.indexOf('---END_STEP_OUTPUT---');
    if (end >= 0) body = body.slice(0, end);
  }
  body = body.trim();
  if (!body.startsWith('{') && !body.startsWith('[')) {
    const match = body.match(/[{[][\s\S]*[}\]]/);
    if (!match) return undefined;
    body = match[0];
  }
  try {
    return JSON.parse(body);
  } catch {
    return undefined;
  }
}

function dig(value: unknown, path: string): unknown {
  return path.split('.').reduce<unknown>(
    (acc, key) => (acc && typeof acc === 'object' ? (acc as Record<string, unknown>)[key] : undefined),
    value,
  );
}

/**
 * Resolve one `meta_map` source spec:
 * - `trigger.<key>` — the run's trigger_context
 * - `run.<field>` — id / status / started_at / origin
 * - `step:<name>:json:<dotpath>` — a JSON path into that step's output envelope
 * - `step:<name>:re:<pattern>` — regex capture group 1 (else whole match) over the raw output
 * - anything else — a literal string
 */
function resolveMeta(
  spec: string,
  run: WorkflowRun,
  byName: Map<string, StepResult>,
  origin: string,
): unknown {
  if (spec.startsWith('trigger.')) {
    const key = spec.slice('trigger.'.length);
    const context = run.trigger_context;
    if (context && typeof context === 'object' && key in (context as Record<string, unknown>)) {
      return (context as Record<string, unknown>)[key];
    }
    return undefined;
  }
  if (spec.startsWith('run.')) {
    const key = spec.slice('run.'.length);
    if (key === 'id') return run.id;
    if (key === 'status') return run.status;
    if (key === 'started_at') return clock(run.started_at);
    if (key === 'origin') return origin;
    return undefined;
  }
  if (spec.startsWith('step:')) {
    const rest = spec.slice('step:'.length);
    const sep = rest.indexOf(':');
    if (sep < 0) return undefined;
    const result = byName.get(rest.slice(0, sep));
    if (!result) return undefined;
    const selector = rest.slice(sep + 1);
    if (selector.startsWith('json:')) {
      return dig(stepOutputJson(result.output), selector.slice('json:'.length));
    }
    if (selector.startsWith('re:')) {
      try {
        const match = result.output?.match(new RegExp(selector.slice('re:'.length)));
        return match ? (match[1] ?? match[0]) : undefined;
      } catch {
        return undefined;
      }
    }
    return undefined;
  }
  return spec; // literal
}

export interface RunToPipelineOptions {
  /** Base URL used for `meta.runUrl` when the map doesn't set it. */
  origin?: string;
  /** Dataset name of the binding — surfaced as `meta.dataset` so the page can
   * reference it when brokering a gate decision. */
  dataset?: string;
}

/**
 * Fold a run's `step_results` into the Page pipeline contract. Steps not yet
 * reached are `pending`; the first not-yet-recorded step of a `Running` run is
 * surfaced as `current` so a single step animates as "in progress".
 */
export function runToPipeline(
  run: WorkflowRun,
  phaseMap: PhaseMapEntry[],
  metaMap: MetaMap = {},
  options: RunToPipelineOptions = {},
): Pipeline {
  const origin = options.origin
    ?? (typeof window !== 'undefined' ? window.location.origin : '');
  const byName = new Map<string, StepResult>();
  for (const result of run.step_results) byName.set(result.step_name, result);

  const running = run.status === 'Running';
  let currentAssigned = false;

  const phases: PipelinePhase[] = phaseMap.map(phase => ({
    name: phase.name,
    ...(phase.emoji ? { emoji: phase.emoji } : {}),
    steps: phase.steps.map(mapped => {
      const result = byName.get(mapped.step);
      let s: PipelineStepStatus;
      if (result) {
        s = mapStepStatus(result.status);
        // A step recorded as Running already claims "current"; don't also hand
        // it to the first unrecorded step, or the page shows two live steps.
        if (s === 'current') currentAssigned = true;
      } else if (running && !currentAssigned) {
        s = 'current';
        currentAssigned = true;
      } else {
        s = 'pending';
      }
      const step: PipelineStep = {
        n: mapped.step,
        s,
        // A waiting gate's message (often multi-line, e.g. a copy-ready
        // announcement) is shown in full; every other step stays a one-liner.
        d: result ? stepDescription(result.output, s === 'wait') : (mapped.label ?? '—'),
      };
      if (mapped.tag) step.tag = mapped.tag;
      const at = clock(result?.started_at);
      if (at) step.at = at;
      const dur = duration(result?.duration_ms);
      if (dur) step.dur = dur;
      // Static link from the phase map wins; otherwise surface the first URL the
      // step printed (e.g. a GitHub Actions run or a release) as a clickable link.
      const link = mapped.link ?? (result ? firstUrl(result.output) : undefined);
      if (link) step.link = link;
      return step;
    }),
  }));

  const meta: Record<string, unknown> = {};
  for (const [field, spec] of Object.entries(metaMap)) {
    const value = resolveMeta(spec, run, byName, origin);
    if (value !== undefined && value !== null && value !== '') meta[field] = value;
  }
  if (meta.runUrl == null) meta.runUrl = origin;
  if (meta.run == null) meta.run = `#${run.id.slice(0, 8)}`;
  if (meta.started == null) {
    const started = clock(run.started_at);
    if (started) meta.started = started;
  }
  // Blank (never `undefined`) for the display fields a pipeline page may read,
  // so unmapped KPIs render empty instead of the literal string "undefined".
  for (const key of ['title', 'type', 'pr', 'prUrl', 'branch', 'tag', 'prev', 'started']) {
    if (meta[key] == null) meta[key] = '';
  }
  // Identifiers the page needs to broker a gate decision back to Kronn.
  meta.run_id = run.id;
  if (options.dataset) meta.dataset = options.dataset;

  return { meta, phases };
}
