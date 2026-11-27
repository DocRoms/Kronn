// KT-2129 — the longest a validation or Exec command may run. Mirrors the
// backend bounds (`MIN/MAX/DEFAULT_VALIDATION_MAX_TIMEOUT_SECS`).
export const MIN_VALIDATION_CEILING_SECS = 60;
export const MAX_VALIDATION_CEILING_SECS = 14_400;
export const DEFAULT_VALIDATION_CEILING_SECS = 1_800;

/** Whether `raw` is a whole number of seconds inside the bounds. */
export function validCeiling(raw: string): boolean {
  const value = Number(raw);
  return raw.trim() !== '' && Number.isInteger(value)
    && value >= MIN_VALIDATION_CEILING_SECS && value <= MAX_VALIDATION_CEILING_SECS;
}
