/** `850 ms`, `12.4 s`, `6 min 48 s`: a step is read at the scale it took. */
export function formatWarmingStepDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(Math.floor(ms / 100) / 10).toFixed(1)} s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.floor((ms % 60_000) / 1000);
  return `${minutes} min ${seconds} s`;
}
