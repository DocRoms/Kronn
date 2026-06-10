// 2026-06-12 — per-item surfacing of a SubWorkflow fan-out
// (`sub_workflow_foreach_file`). The engine aggregates the child runs into a
// `{mode:'foreach', total, succeeded, failed, items[]}` envelope inside the
// step output; until now the UI showed it as a raw JSON blob. This parses it
// back so RunDetail can render a per-task table + a PARTIAL chip (failed > 0
// keeps the step green since PARTIAL=Success engine-side — the chip is how a
// human notices the partial without reading JSON).

export interface ForeachItem {
  item: number;
  id: string;
  /** Success | MechanicalApplied | Failed | Skipped(...) — engine statuses. */
  status: string;
  /** Null for engine-applied (mechanical) items — no child run was spawned. */
  child_run_id: string | null;
  /** Files written when the engine applied the item directly. */
  files?: number;
}

export interface ForeachSummary {
  total: number;
  succeeded: number;
  failed: number;
  items: ForeachItem[];
}

/** Parse a step output's foreach envelope; null when the output is not a
 *  foreach aggregate (single-child SubWorkflow, agent step, garbage…). */
export function parseForeachEnvelope(output: string | null | undefined): ForeachSummary | null {
  if (!output) return null;
  const marker = output.indexOf('---STEP_OUTPUT---');
  if (marker < 0) return null;
  let raw = output.slice(marker + '---STEP_OUTPUT---'.length);
  // 2026-06-13 — use the LAST end marker, not the first: the foreach envelope's
  // `last_output` field embeds a child step's output that itself contains a
  // nested `---END_STEP_OUTPUT---`. Slicing at the FIRST end marker cut the JSON
  // mid-string → parse failed → the per-task table silently vanished, leaving
  // only the raw (truncated) JSON. The OUTER envelope's end is always the last.
  const end = raw.lastIndexOf('---END_STEP_OUTPUT---');
  if (end >= 0) raw = raw.slice(0, end);
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw.trim());
  } catch {
    // Defensive fallback: if the outer slice still won't parse (e.g. trailing
    // junk), try the first end marker as the legacy single-envelope case.
    const firstEnd = raw.indexOf('---END_STEP_OUTPUT---');
    try { parsed = JSON.parse((firstEnd >= 0 ? raw.slice(0, firstEnd) : raw).trim()); }
    catch { return null; }
  }
  const data = (parsed as { data?: unknown })?.data as Record<string, unknown> | undefined;
  if (!data || data.mode !== 'foreach' || !Array.isArray(data.items)) return null;
  const items: ForeachItem[] = (data.items as Record<string, unknown>[]).map((it, idx) => ({
    item: typeof it.item === 'number' ? it.item : idx,
    id: typeof it.id === 'string' ? it.id : `item-${idx}`,
    status: typeof it.status === 'string' ? it.status : '?',
    child_run_id: typeof it.child_run_id === 'string' ? it.child_run_id : null,
    files: typeof it.files === 'number' ? it.files : undefined,
  }));
  return {
    total: typeof data.total === 'number' ? data.total : items.length,
    succeeded: typeof data.succeeded === 'number' ? data.succeeded : items.filter(i => i.status !== 'Failed').length,
    failed: typeof data.failed === 'number' ? data.failed : items.filter(i => i.status === 'Failed').length,
    items,
  };
}

/** True when the item ran at zero token cost (engine-applied or skipped
 *  because an earlier run already committed it). */
export function isZeroTokenItem(status: string): boolean {
  return status === 'MechanicalApplied' || status.startsWith('Skipped');
}
