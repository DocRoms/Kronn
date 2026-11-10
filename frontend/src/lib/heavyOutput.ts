// KT-1525 — a workflow step output that grows unnoticed slows every run, the run
// storage and the page that receives it. The backend records each step's output
// size (and, per source, a CollectApiData step's); the UI flags the heavy ones.

/** Smallest and largest threshold (KB) the backend accepts. */
export const MIN_HEAVY_OUTPUT_KB = 1;
export const MAX_HEAVY_OUTPUT_KB = 102_400;
/** Out-of-the-box default, mirrored from `DEFAULT_HEAVY_OUTPUT_KB`. */
export const DEFAULT_HEAVY_OUTPUT_KB = 256;

/** Whether an output of `bytes` is heavy for a `thresholdKb` KB threshold. */
export function isHeavyOutput(bytes: number | null | undefined, thresholdKb: number): boolean {
  return typeof bytes === 'number' && bytes > thresholdKb * 1024;
}

/** The size of a step result's output; falls back to the stored text for a run
 *  recorded before the size was stamped. */
export function stepOutputBytes(result: { output_bytes?: number | null; output: string }): number {
  return result.output_bytes ?? new TextEncoder().encode(result.output).length;
}

export interface SourceWeight {
  alias: string;
  bytes: number;
}

/** The payload size of each source of a CollectApiData step, from its output
 *  envelope (`data.meta.sources[].output_bytes`); empty when absent. */
export function collectSourceWeights(output: string | null | undefined): SourceWeight[] {
  if (!output) return [];
  const start = output.indexOf('---STEP_OUTPUT---');
  if (start < 0) return [];
  let raw = output.slice(start + '---STEP_OUTPUT---'.length);
  const end = raw.lastIndexOf('---END_STEP_OUTPUT---');
  if (end >= 0) raw = raw.slice(0, end);
  try {
    const meta = (JSON.parse(raw.trim()) as { data?: { meta?: { sources?: unknown } } })?.data?.meta;
    if (!Array.isArray(meta?.sources)) return [];
    return (meta.sources as Record<string, unknown>[])
      .filter(source => typeof source.alias === 'string' && typeof source.output_bytes === 'number')
      .map(source => ({ alias: source.alias as string, bytes: source.output_bytes as number }));
  } catch {
    return [];
  }
}
