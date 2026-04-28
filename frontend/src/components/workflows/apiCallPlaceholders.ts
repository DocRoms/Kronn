// Pure helpers for ApiCallStepCard's "Test the call" placeholder flow.
// Walk the request shape (path, query, headers, body) to find every
// `{{name}}` token, exclude runtime-only namespaces (`steps.X`,
// `previous_step.X`, `state.X`, `batch.X`, `issue.X`, `artifacts.X`),
// and substitute the rest with user-provided test values.
//
// Extracted from the React component so the namespace-exclusion rules
// stay pinned by unit tests — they're easy to silently regress when
// adding a new runtime namespace.
import type { WorkflowStep } from '../../types/generated';

const RUNTIME_NAMESPACES = ['steps', 'previous_step', 'state', 'batch', 'issue', 'artifacts'];

export function isRuntimeToken(name: string): boolean {
  return RUNTIME_NAMESPACES.some(ns => name === ns || name.startsWith(`${ns}.`));
}

/** Collect every `{{var}}` token in the step's request shape that is NOT a
 *  runtime-resolved namespace. Used by the "Test the call" button to know
 *  which variables to prompt for. Returns sorted unique names. */
export function collectPlaceholders(s: WorkflowStep): string[] {
  const found = new Set<string>();
  const scan = (str: string | null | undefined) => {
    if (!str) return;
    const matches = str.match(/\{\{([\w.]+)\}\}/g) ?? [];
    for (const m of matches) {
      const name = m.slice(2, -2);
      if (!isRuntimeToken(name)) found.add(name);
    }
  };
  scan(s.api_endpoint_path);
  Object.values(s.api_query ?? {}).forEach(scan);
  Object.values(s.api_path_params ?? {}).forEach(scan);
  Object.values(s.api_headers ?? {}).forEach(scan);
  if (typeof s.api_body === 'string') {
    scan(s.api_body);
  } else if (s.api_body) {
    const walk = (v: unknown): void => {
      if (typeof v === 'string') scan(v);
      else if (Array.isArray(v)) v.forEach(walk);
      else if (v && typeof v === 'object') Object.values(v as Record<string, unknown>).forEach(walk);
    };
    walk(s.api_body);
  }
  return [...found].sort();
}

/** Substitute user-supplied values for the non-runtime `{{var}}` tokens.
 *  Runtime tokens (`{{steps.X}}` etc.) are left untouched — the workflow
 *  runner resolves them at run time. Used by the "Test the call" path to
 *  produce a step shape with literal values that the backend can fire as
 *  a real HTTP request. */
export function substitutePlaceholders(
  s: WorkflowStep,
  vars: Record<string, string>,
): WorkflowStep {
  const sub = (str: string | null | undefined): string | null | undefined => {
    if (str == null) return str;
    return str.replace(/\{\{([\w.]+)\}\}/g, (raw, name) => {
      if (isRuntimeToken(name)) return raw;
      return vars[name] ?? raw;
    });
  };
  const subRecord = (r: Record<string, string> | null | undefined) => {
    if (!r) return r ?? null;
    const out: Record<string, string> = {};
    for (const [k, v] of Object.entries(r)) out[k] = sub(v) as string;
    return out;
  };
  const subBody = (v: unknown): unknown => {
    if (typeof v === 'string') return sub(v);
    if (Array.isArray(v)) return v.map(subBody);
    if (v && typeof v === 'object') {
      const out: Record<string, unknown> = {};
      for (const [k, val] of Object.entries(v as Record<string, unknown>)) out[k] = subBody(val);
      return out;
    }
    return v;
  };
  return {
    ...s,
    api_endpoint_path: sub(s.api_endpoint_path) ?? null,
    api_query: subRecord(s.api_query) ?? null,
    api_path_params: subRecord(s.api_path_params) ?? null,
    api_headers: subRecord(s.api_headers) ?? null,
    api_body: subBody(s.api_body),
  };
}
