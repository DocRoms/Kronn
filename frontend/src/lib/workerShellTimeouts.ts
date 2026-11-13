// KT-1718 — Claude Code cuts a shell command at 10 minutes unless told otherwise,
// and a cold test-profile build outlasts that: each timeout kills the compiler and
// the worker loses its turn. Settings > Workers sets the limits Kronn gives a
// Claude task worker (`BASH_DEFAULT_TIMEOUT_MS` / `BASH_MAX_TIMEOUT_MS`).

/** Smallest and largest per-command limit (minutes) the backend accepts; the
 *  largest equals the largest generation limit, since a command outliving the
 *  generation is cut with it. */
export const MIN_BASH_TIMEOUT_MIN = 1;
export const MAX_BASH_TIMEOUT_MIN = 240;
/** Out-of-the-box defaults, mirrored from `DEFAULT_BASH_DEFAULT_TIMEOUT_MIN` and
 *  `DEFAULT_BASH_MAX_TIMEOUT_MIN`. */
export const DEFAULT_BASH_DEFAULT_TIMEOUT_MIN = 15;
export const DEFAULT_BASH_MAX_TIMEOUT_MIN = 60;

/** Whether a command allowed `maxMin` minutes can finish inside a generation
 *  limited to `generationMin` minutes. */
export function commandOutlivesGeneration(maxMin: number, generationMin: number | null | undefined): boolean {
  return typeof generationMin === 'number' && generationMin > 0 && maxMin >= generationMin;
}
