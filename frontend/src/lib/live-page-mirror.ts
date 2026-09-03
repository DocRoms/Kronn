import { pages as pagesApi, workflows as workflowsApi } from './api';
import {
  runToPipeline,
  type MetaMap,
  type PhaseMapEntry,
  type Pipeline,
} from './live-page-pipeline';
import { TERMINAL_RUN } from './live-page-run-status';

/**
 * Normalize a binding's `phase_map`/`meta_map`. The backend stores them as a
 * `serde_json::Value` and normally returns a real array/object, but a binding
 * written through the MCP `page_bind_workflow` tool can arrive double-encoded as
 * a JSON string. Parse that shape back so the fold sees a value, not a string.
 */
function asJsonValue(raw: unknown): unknown {
  if (typeof raw !== 'string') return raw;
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

export type MirrorResult = { pipelines: Record<string, Pipeline>; active: boolean };

/**
 * Mirror each of the Page's workflow bindings into a `{ dataset → pipeline }`
 * map by reading the bound run live and reshaping it client-side (Voie B — the
 * backend can't express this fold). `active` is true while any mirrored run is
 * non-terminal, which keeps a refresh loop on its fast cadence so steps appear
 * to validate one by one.
 *
 * Shared by both surfaces that embed a Page: the published `StandaloneLivePage`
 * and the in-app Pages editor preview (`PagesPage`), so the editor shows the
 * same live run as the published page instead of the page's mock fallback.
 */
export async function resolveBindingPipelines(pageId: string): Promise<MirrorResult> {
  const bindings = await pagesApi.bindings(pageId);
  let active = false;
  const entries = await Promise.all(bindings.map(async binding => {
    try {
      // A trigger-only binding (Phase 4) carries no phase grouping to fold — it
      // exists to authorize a trigger button, not to mirror a run. Skip it so we
      // never overwrite its dataset with an empty pipeline.
      // `phase_map`/`meta_map` are `serde_json::Value` blobs; a binding created
      // through the MCP `page_bind_workflow` tool can land them double-encoded as
      // a JSON string, so parse that shape back into a value before folding.
      const phaseMap = asJsonValue(binding.phase_map) as PhaseMapEntry[] | null;
      if (!Array.isArray(phaseMap) || phaseMap.length === 0) return null;
      const runs = await workflowsApi.listRuns(binding.workflow_id, 5);
      if (!runs.length) return null;
      const sorted = [...runs].sort((a, b) => (b.started_at ?? '').localeCompare(a.started_at ?? ''));
      const picked = binding.run_selector === 'latest_active'
        ? (sorted.find(run => !TERMINAL_RUN.has(run.status)) ?? sorted[0])
        : sorted[0];
      if (!TERMINAL_RUN.has(picked.status)) active = true;
      const full = await workflowsApi.getRun(binding.workflow_id, picked.id);
      const pipeline = runToPipeline(
        full,
        phaseMap,
        (asJsonValue(binding.meta_map) ?? {}) as MetaMap,
        { dataset: binding.dataset },
      );
      return [binding.dataset, pipeline] as const;
    } catch {
      return null; // one broken binding never blanks the others
    }
  }));
  const pipelines = Object.fromEntries(
    entries.filter((entry): entry is [string, Pipeline] => entry !== null),
  );
  return { pipelines, active };
}
