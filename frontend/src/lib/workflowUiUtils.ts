import type { WorkflowRun, WorkflowStep } from '../types/generated';

const RUN_RESUME_HISTORY_KEY = '__kronn.resume_history';

export function appendLiveBuffer(prev: string, chunks: string, max: number): string {
  const merged = prev + chunks;
  return merged.length <= max ? merged : merged.slice(merged.length - max);
}

export function buildBlankStep(
  existingCount: number,
  defaultTier: 'economy' | 'default' | 'reasoning' | null,
): WorkflowStep {
  const step: WorkflowStep = {
    name: `step-${existingCount + 1}`,
    step_type: { type: 'Agent' },
    agent: 'ClaudeCode',
    prompt_template: '',
    mode: { type: 'Normal' },
    output_format: { type: 'Structured' },
  };
  if (defaultTier && defaultTier !== 'default') {
    step.agent_settings = { tier: defaultTier };
  }
  return step;
}

/** Produce a useful output key from a picked JSONPath. The collector wrapper
 * (`sources`) is transport metadata, so it is intentionally omitted. */
export function jsonPathToTarget(path: string): string {
  const segments: string[] = [];
  const matcher = /\.([A-Za-z_][A-Za-z0-9_-]*)|\[['"]((?:\\.|[^'"\\])*)['"]\]|\[(?:\d+|\*)\]/g;
  let match: RegExpExecArray | null;
  while ((match = matcher.exec(path)) !== null) {
    const value = (match[1] ?? match[2] ?? '').replace(/\\(['"\\])/g, '$1');
    if (value) segments.push(value);
  }
  if (segments[0] === 'sources') segments.shift();
  return segments
    .map(segment => segment.replace(/[^A-Za-z0-9_]/g, '_').replace(/^([0-9])/, '_$1'))
    .filter(Boolean)
    .join('.') || 'value';
}

export function liveStepWaitingKey(step: Pick<WorkflowStep, 'step_type'>): string {
  const isAgentLike = !step.step_type || step.step_type.type === 'Agent';
  return isAgentLike ? 'wf.live.stepStreamingWaiting' : 'wf.live.stepRunningNoStream';
}

function truncatePreview(value: string, max: number): string {
  return value.length > max ? `${value.slice(0, max)}…` : value;
}

export function previewString(value: unknown, depth = 0): string {
  if (value == null) return 'null';
  if (typeof value === 'string') {
    return `"${truncatePreview(value, depth === 0 ? 60 : 30)}"`;
  }
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  if (Array.isArray(value)) {
    if (value.length === 0) return '[]';
    if (depth >= 1) return `[${value.length}]`;
    const items = value.slice(0, 3).map(item => previewString(item, depth + 1));
    const more = value.length > 3 ? `, … (+${value.length - 3})` : '';
    return `[${items.join(', ')}${more}]`;
  }
  if (typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 0) return '{}';
    if (depth >= 1) return '{…}';
    const items = entries
      .slice(0, 3)
      .map(([key, item]) => `${key}: ${previewString(item, depth + 1)}`);
    const more = entries.length > 3 ? `, … (+${entries.length - 3})` : '';
    return `{${items.join(', ')}${more}}`;
  }
  return String(value);
}

export function runStatusTimeline(run: WorkflowRun): string[] {
  const raw = run.state?.[RUN_RESUME_HISTORY_KEY];
  if (!raw) return [run.status];
  try {
    const parsed = JSON.parse(raw) as { events?: Array<{ status?: unknown }> };
    const timeline = (parsed.events ?? [])
      .map(event => event.status)
      .filter((status): status is string => typeof status === 'string' && status.length > 0);
    if (timeline[timeline.length - 1] !== run.status) timeline.push(run.status);
    return timeline.length > 0 ? timeline : [run.status];
  } catch {
    return [run.status];
  }
}

type TriageClear = { id: string; what: string; where?: string };
type TriageDecided = {
  id: string;
  what: string;
  chosen: string;
  why: string;
  options_considered?: string[];
};
type TriageMocked = {
  id: string;
  what: string;
  placeholder: string;
  strategy?: string;
  revisit_when?: string;
};
type TriageBlocked = {
  id: string;
  what: string;
  why: string;
  needed_from: string;
  workaround?: string;
};
export type TriageManifest = {
  clear: TriageClear[];
  decided: TriageDecided[];
  mocked: TriageMocked[];
  blocked: TriageBlocked[];
  files_touched?: string[];
};

export function tryParseTriageManifest(message: string): TriageManifest | null {
  if (!message) return null;
  const openIndex = message.indexOf('{');
  if (openIndex < 0) return null;
  let depth = 0;
  let endIndex = -1;
  let inString = false;
  let escaped = false;
  for (let index = openIndex; index < message.length; index++) {
    const character = message[index];
    if (inString) {
      if (escaped) escaped = false;
      else if (character === '\\') escaped = true;
      else if (character === '"') inString = false;
      continue;
    }
    if (character === '"') inString = true;
    else if (character === '{') depth++;
    else if (character === '}') {
      depth--;
      if (depth === 0) {
        endIndex = index + 1;
        break;
      }
    }
  }
  if (endIndex < 0) return null;
  try {
    const parsed = JSON.parse(message.slice(openIndex, endIndex)) as Partial<TriageManifest>;
    if (
      !parsed ||
      !Array.isArray(parsed.clear) ||
      !Array.isArray(parsed.decided) ||
      !Array.isArray(parsed.mocked) ||
      !Array.isArray(parsed.blocked)
    ) return null;
    return parsed as TriageManifest;
  } catch {
    return null;
  }
}
